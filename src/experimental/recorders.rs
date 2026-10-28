//! Strict experimental framing of the observed StateRecorderBlobs array and
//! staged root/recorder schema validation for `FProspectSaveState`.
//!
//! This accepts one shared StateRecorderBlob struct tag and exact tagged element
//! boundaries. Class strings and recorder bodies remain borrowed opaque bytes;
//! no class lookup, gameplay interpretation, object construction or import occurs.
use anyhow::{Context, Result, bail, ensure};
use serde::Serialize;

use super::archive::{ByteOrder, decode_string};
use super::checksum_utf16;
use super::component::{
    BaseComponent, MAX_BOUNDED_COMPONENT_BYTES, read_base_component_with_limit,
};
use super::property_tree::{BaseComponentTree, MAX_TREE_DEPTH, MAX_TREE_NODES, TreeLimits};
use super::prospect_info::{
    AssociatedMember, CustomGameSetting, MISSION_DIFFICULTIES, PROSPECT_LOCATIONS, PROSPECT_STATES,
    ProspectInfo, SavedActorInventory, SavedModifierState, saved_actor_state,
};
use super::restore_rules::{
    LobbyPrivacy, OwnerResolvePolicy, PROSPECT_SAVE_STATE_CURRENT_VERSION,
    ProspectAssociatedMember, RecorderCtorDefaults, evaluate_requires_stitching, names_match,
    no_recorder_record_valid, non_blob_recorder_ctor_defaults, player_record_valid,
    prospect_ids_match, recorder_ctor_defaults, resolve_prospect_player_list,
    resource_deposit_record_valid, rocket_record_valid, validate_rocket_spawn_record,
    voxel_record_valid,
};
use super::struct_array::{MAX_BOUNDED_STRUCT_ARRAY_BYTES, read_tagged_struct_array_with_limit};
use super::tagged::{
    MAX_HEADER_STRING_UNITS, MAX_INPUT_BYTES, MAX_ROOT_TAGS, ParsedRecord, PropertyRecord, RootTag,
    TagKind, TagProfile, is_name, read_record, read_root,
};
use super::typed_value::TypedComponent;

pub const MAX_RECORDERS: usize = 16_384;
pub const MAX_RECORDER_FIELDS: usize = 65_536;

/// Source spans only; deliberately neither Debug nor Serialize.
pub struct Recorder<'a> {
    /// Complete element, including its terminal name and every unknown field.
    pub record: &'a [u8],
    fields: Vec<RootTag<'a>>,
    pub terminator: &'a [u8],
    /// Complete encoded FString, not a decoded class identifier.
    pub class_value: &'a [u8],
    /// Opaque bytes after the validated BinaryData count.
    pub binary_data: &'a [u8],
}

pub struct RecorderArray<'a> {
    pub root_record: &'a [u8],
    pub root_header: &'a [u8],
    pub count_prefix: &'a [u8],
    pub shared_header: &'a [u8],
    records: Vec<Recorder<'a>>,
    profile: TagProfile,
}

#[derive(Debug, Serialize)]
pub struct RecorderSummary {
    pub profile: &'static str,
    pub recorder_count: usize,
    pub field_tags: usize,
    pub class_fields: usize,
    pub binary_fields: usize,
    pub unknown_fields: usize,
    pub opaque_binary_bytes: usize,
    pub class_value_bytes: usize,
    pub world_imported: bool,
}

/// Privacy-preserving summary of a staged, schema-validated `FProspectSaveState`
/// root and recorder payload after `PostLoadSerialize` migration (`F-328`,
/// `F-330`, `F-403`, `F-715`, `F-718`, `F-726`, `F-733`) and per-recorder
/// `Validate` (`+0x408`) / `C033` `bIsPhysicsDynamic` evaluation (`F-652`,
/// `F-663`, `F-680`, `F-681`).
///
/// Never exposes `ProspectID`, `ProspectDTKey`, `ProspectMapName`, account IDs
/// or recorder class names.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct StagedProspectSaveState {
    pub profile: &'static str,
    pub loaded_version: i32,
    pub migrated_version: i32,
    pub lobby_privacy: LobbyPrivacy,
    pub root_tag_count: usize,
    pub prospect_info_tag_count: usize,
    pub associated_member_count: usize,
    pub has_prospect_id: bool,
    pub has_prospect_dt_key: bool,
    pub has_map_name: bool,
    pub has_last_saved_date_time: bool,
    pub has_expire_time: bool,
    pub parsed_recorder_count: usize,
    pub invalid_recorder_count: usize,
    pub retained_recorder_count: usize,
    pub reset_rocket_spawn_recorder_count: usize,
    pub physics_dynamic_tree_recorder_count: usize,
    pub skip_default_owner_resolution_count: usize,
    pub find_only_owner_count: usize,
    pub respawn_only_owner_count: usize,
    pub find_or_respawn_owner_count: usize,
    pub manually_resolved_owner_count: usize,
    pub requires_stitching_count: usize,
    pub total_component_field_tags: usize,
    pub world_imported: bool,
}

/// Which root fields were supplied, independently of their effective defaults.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StagedRootPresence {
    pub version: bool,
    pub lobby_privacy: bool,
    pub prospect_info: bool,
    pub prospect_map_name: bool,
    pub last_saved_date_time: bool,
    pub legacy_prospect_id: bool,
    pub legacy_prospect_dt_key: bool,
    pub state_recorder_blobs: bool,
}

/// Qualified structural migration only; no gameplay acceptance is implied.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StagedRootMigration {
    CurrentVersion,
    Version1LegacyOverlay,
    /// Reset ProspectInfo, force FriendsOnly, and discard all recorders.
    /// Map and timestamp are retained.
    Version0Reset,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StagedLegacyDisposition {
    Absent,
    AppliedToProspectInfo,
    AppliedThenReset,
    IgnoredAtCurrentVersion,
}

/// Locations within the source ProspectInfo; member indices are source order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StagedRootEnumField {
    ProspectState,
    Difficulty,
    AssociatedMemberStatus(usize),
}

/// Exact accepted enum input. Deliberately neither Debug nor Serialize.
#[derive(Clone, PartialEq, Eq)]
pub enum StagedRootEnumValue {
    Byte(u8),
    Name(Vec<u16>),
}

/// An accepted source enum and its qualified DTO mapping, when known.
/// Unrecognized names remain accepted by strict staging, with a zero DTO
/// placeholder and `resolved_value = None`; unrecognized raw bytes remain
/// unchanged in the DTO, also with `resolved_value = None`. Consumers must
/// check this qualification before treating the DTO as semantically resolved.
/// These source values survive even when version 0 resets the effective DTO.
#[derive(Clone, PartialEq, Eq)]
pub struct StagedRootEnum {
    pub field: StagedRootEnumField,
    /// Distinguishes named EnumProperty from named/raw ByteProperty inputs.
    pub source_kind: TagKind,
    pub source_value: StagedRootEnumValue,
    pub resolved_value: Option<u8>,
}

/// Owned, strictly staged root values for incremental import preparation.
/// Strings retain exact UTF-16 units, including accepted unpaired surrogates.
/// No Serialize implementation; Debug reports counts only. Structural staging
/// and migration dispositions do not qualify gameplay restoration.
#[derive(Clone, PartialEq, Eq)]
pub struct StagedRootValues {
    /// Effective values after the existing qualified version migrations.
    /// See source_enum_values for enum mappings that remain unqualified.
    prospect_info: ProspectInfo,
    lobby_privacy: LobbyPrivacy,
    prospect_map_name: Vec<u16>,
    /// Exact signed saved value; absence is distinct from an explicit zero.
    last_saved_date_time: Option<i64>,
    /// None when omitted (the existing migration then uses version 0).
    loaded_version: Option<i32>,
    source_presence: StagedRootPresence,
    legacy_prospect_id: Option<Vec<u16>>,
    legacy_prospect_dt_key: Option<Vec<u16>>,
    migration: StagedRootMigration,
    legacy_prospect_id_disposition: StagedLegacyDisposition,
    legacy_prospect_dt_key_disposition: StagedLegacyDisposition,
    source_enum_values: Vec<StagedRootEnum>,
}

impl StagedRootValues {
    /// Structurally accepted DTO. Unknown named enums have placeholders; inspect
    /// `source_enum_values` or use `resolved_prospect_info` before consuming them.
    pub fn prospect_info(&self) -> &ProspectInfo {
        &self.prospect_info
    }

    /// Effective DTO only when all effective enum values have known mappings.
    /// Version 0 resets the DTO, so unresolved source enums no longer affect it.
    /// This qualifies enum mapping only, not gameplay restoration.
    pub fn resolved_prospect_info(&self) -> Option<&ProspectInfo> {
        (self.migration == StagedRootMigration::Version0Reset
            || self
                .source_enum_values
                .iter()
                .all(|value| value.resolved_value.is_some()))
        .then_some(&self.prospect_info)
    }

    pub fn lobby_privacy(&self) -> LobbyPrivacy {
        self.lobby_privacy
    }

    pub fn prospect_map_name(&self) -> &[u16] {
        &self.prospect_map_name
    }

    pub fn last_saved_date_time(&self) -> Option<i64> {
        self.last_saved_date_time
    }

    pub fn loaded_version(&self) -> Option<i32> {
        self.loaded_version
    }

    pub fn source_presence(&self) -> StagedRootPresence {
        self.source_presence
    }

    pub fn legacy_prospect_id(&self) -> Option<&[u16]> {
        self.legacy_prospect_id.as_deref()
    }

    pub fn legacy_prospect_dt_key(&self) -> Option<&[u16]> {
        self.legacy_prospect_dt_key.as_deref()
    }

    pub fn migration(&self) -> StagedRootMigration {
        self.migration
    }

    pub fn legacy_prospect_id_disposition(&self) -> StagedLegacyDisposition {
        self.legacy_prospect_id_disposition
    }

    pub fn legacy_prospect_dt_key_disposition(&self) -> StagedLegacyDisposition {
        self.legacy_prospect_dt_key_disposition
    }

    pub fn source_enum_values(&self) -> &[StagedRootEnum] {
        &self.source_enum_values
    }

    /// Retained allocation accounting, not RSS or transient parsing memory.
    /// Includes Self, all vector capacities (including unused slots), and the
    /// nested allocations of initialized members, settings, enums and legacy
    /// strings. Presence and scalar options are inline and counted in Self.
    pub fn owned_bytes(&self) -> Result<usize> {
        fn add_capacity<T>(total: &mut usize, capacity: usize) -> Result<()> {
            let bytes = capacity
                .checked_mul(std::mem::size_of::<T>())
                .context("staged root allocation size overflow")?;
            *total = total
                .checked_add(bytes)
                .context("staged root allocation size overflow")?;
            Ok(())
        }

        let mut total = std::mem::size_of::<Self>();
        for units in [
            &self.prospect_info.prospect_id,
            &self.prospect_info.claimed_account_id,
            &self.prospect_info.prospect_dt_key,
            &self.prospect_info.faction_mission_dt_key,
            &self.prospect_info.lobby_name,
            &self.prospect_map_name,
        ] {
            add_capacity::<u16>(&mut total, units.capacity())?;
        }
        add_capacity::<AssociatedMember>(
            &mut total,
            self.prospect_info.associated_members.capacity(),
        )?;
        for member in &self.prospect_info.associated_members {
            for units in [
                &member.account_name,
                &member.character_name,
                &member.user_id,
            ] {
                add_capacity::<u16>(&mut total, units.capacity())?;
            }
        }
        add_capacity::<CustomGameSetting>(
            &mut total,
            self.prospect_info.custom_settings.capacity(),
        )?;
        for setting in &self.prospect_info.custom_settings {
            add_capacity::<u16>(&mut total, setting.setting_row_name.capacity())?;
        }
        for units in [&self.legacy_prospect_id, &self.legacy_prospect_dt_key]
            .into_iter()
            .flatten()
        {
            add_capacity::<u16>(&mut total, units.capacity())?;
        }
        add_capacity::<StagedRootEnum>(&mut total, self.source_enum_values.capacity())?;
        for value in &self.source_enum_values {
            if let StagedRootEnumValue::Name(units) = &value.source_value {
                add_capacity::<u16>(&mut total, units.capacity())?;
            }
        }
        Ok(total)
    }
}

impl std::fmt::Debug for StagedRootValues {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("StagedRootValues")
            .field(
                "associated_members",
                &self.prospect_info.associated_members.len(),
            )
            .field("custom_settings", &self.prospect_info.custom_settings.len())
            .field("source_enum_values", &self.source_enum_values.len())
            .finish_non_exhaustive()
    }
}

impl<'a> Recorder<'a> {
    pub fn fields(&self) -> &[RootTag<'a>] {
        &self.fields
    }
}

impl<'a> RecorderArray<'a> {
    pub fn records(&self) -> &[Recorder<'a>] {
        &self.records
    }

    pub fn summary(&self) -> RecorderSummary {
        let count = self.records.len();
        let fields = self.records.iter().map(|record| record.fields.len()).sum();
        RecorderSummary {
            profile: match self.profile {
                TagProfile::MemoryArchive522 => "experimental_memory_archive_522",
            },
            recorder_count: count,
            field_tags: fields,
            class_fields: count,
            binary_fields: count,
            unknown_fields: fields - count * 2,
            opaque_binary_bytes: self
                .records
                .iter()
                .map(|record| record.binary_data.len())
                .sum(),
            class_value_bytes: self
                .records
                .iter()
                .map(|record| record.class_value.len())
                .sum(),
            world_imported: false,
        }
    }
}

fn count(input: &[u8]) -> Result<usize> {
    let bytes = input.get(..4).context("truncated recorder count")?;
    let value = i32::from_le_bytes(bytes.try_into()?);
    ensure!(value >= 0, "negative recorder count");
    Ok(usize::try_from(value)?)
}

fn extra_is(property: &PropertyRecord<'_>, expected: &str) -> bool {
    property
        .extra
        .as_ref()
        .is_some_and(|name| is_name(name, expected))
}

fn validate_class_value(input: &[u8]) -> Result<()> {
    let prefix = input.get(..4).context("truncated recorder string")?;
    let count = i32::from_le_bytes(prefix.try_into()?);
    ensure!(
        count.unsigned_abs() as usize <= MAX_HEADER_STRING_UNITS,
        "recorder string exceeds research unit limit"
    );
    // Empty and raw UTF-16 strings are framing only, not valid-class assertions.
    let value = decode_string(input, ByteOrder::Little)?;
    ensure!(
        value.consumed == input.len(),
        "trailing recorder string data"
    );
    Ok(())
}

/// Inspect the explicit MemoryArchive522 framing subset after envelope integrity
/// and root framing validation. Exactly one selected root and exactly one of each
/// known field per element are required. Unknown supported-header fields retain
/// their complete original bytes. No unknown header or malformed data is skipped.
pub fn read_recorders<'a>(roots: &[RootTag<'a>], profile: TagProfile) -> Result<RecorderArray<'a>> {
    ensure!(
        roots.len() <= MAX_ROOT_TAGS,
        "recorder root count exceeds research limit"
    );
    let total = roots.iter().try_fold(0usize, |size, root| {
        size.checked_add(root.record.len())
            .context("recorder input length overflow")
    })?;
    ensure!(
        total <= MAX_INPUT_BYTES,
        "recorder input exceeds 32 MiB limit"
    );
    let mut selected = None;
    for root in roots {
        // Reparse bounded metadata without trusting caller-constructed RootTag fields.
        let ParsedRecord::Property(property) = read_record(root.record, profile)? else {
            bail!("unexpected recorder root terminator");
        };
        ensure!(
            property.tag.record.len() == root.record.len(),
            "trailing recorder root data"
        );
        if is_name(&property.name, "StateRecorderBlobs") {
            ensure!(selected.is_none(), "duplicate recorder root");
            ensure!(
                property.tag.kind == TagKind::Array
                    && extra_is(&property, "StructProperty")
                    && property.index == 0,
                "unsupported recorder root header"
            );
            selected = Some(property);
        }
    }
    let selected = selected.context("missing recorder root")?;
    let payload = selected.tag.payload;
    let declared = count(payload)?;
    ensure!(
        declared <= MAX_RECORDERS,
        "recorder count exceeds research limit"
    );
    let ParsedRecord::Property(shared) = read_record(&payload[4..], profile)? else {
        bail!("missing shared recorder struct header");
    };
    ensure!(
        is_name(&shared.name, "StateRecorderBlobs")
            && shared.tag.kind == TagKind::Struct
            && extra_is(&shared, "StateRecorderBlob")
            && shared.index == 0,
        "unsupported shared recorder struct header"
    );
    ensure!(
        shared.tag.record.len() == payload.len() - 4,
        "shared recorder aggregate size mismatch"
    );
    let body = shared.tag.payload;
    let mut position = 0;
    let mut field_count = 0;
    let mut records = Vec::new();
    for _ in 0..declared {
        let start = position;
        let mut fields = Vec::new();
        let mut class_value = None;
        let mut binary_data = None;
        let terminator = loop {
            match read_record(&body[position..], profile)? {
                ParsedRecord::Terminator(bytes) => {
                    position += bytes.len();
                    break bytes;
                }
                ParsedRecord::Property(property) => {
                    ensure!(
                        field_count < MAX_RECORDER_FIELDS,
                        "recorder field count exceeds research limit"
                    );
                    field_count += 1;
                    let tag = property.tag;
                    if is_name(&property.name, "ComponentClassName") {
                        ensure!(class_value.is_none(), "duplicate recorder class field");
                        ensure!(
                            tag.kind == TagKind::String && property.index == 0,
                            "unsupported recorder class header"
                        );
                        validate_class_value(tag.payload)?;
                        class_value = Some(tag.payload);
                    } else if is_name(&property.name, "BinaryData") {
                        ensure!(binary_data.is_none(), "duplicate recorder binary field");
                        ensure!(
                            tag.kind == TagKind::Array
                                && property.index == 0
                                && property
                                    .extra
                                    .as_ref()
                                    .is_some_and(|name| is_name(name, "ByteProperty")),
                            "unsupported recorder binary header"
                        );
                        let bytes = count(tag.payload)?;
                        ensure!(
                            bytes == tag.payload.len() - 4,
                            "recorder byte array count mismatch"
                        );
                        binary_data = Some(&tag.payload[4..]);
                    }
                    position += tag.record.len();
                    fields.push(tag);
                }
            }
        };
        records.push(Recorder {
            record: &body[start..position],
            fields,
            terminator,
            class_value: class_value.context("missing recorder class field")?,
            binary_data: binary_data.context("missing recorder binary field")?,
        });
    }
    ensure!(position == body.len(), "trailing recorder aggregate data");
    Ok(RecorderArray {
        root_record: selected.tag.record,
        root_header: selected.tag.header,
        count_prefix: &payload[..4],
        shared_header: shared.tag.header,
        records,
        profile,
    })
}

fn decode_exact_string_units(payload: &[u8], context_label: &'static str) -> Result<Vec<u16>> {
    let prefix = payload.get(..4).context(context_label)?;
    let count = i32::from_le_bytes(prefix.try_into()?);
    ensure!(
        count.unsigned_abs() as usize <= MAX_HEADER_STRING_UNITS,
        "string exceeds research unit limit"
    );
    let decoded = decode_string(payload, ByteOrder::Little).context(context_label)?;
    ensure!(decoded.consumed == payload.len(), "trailing string bytes");
    Ok(decoded.units)
}

fn units_eq_ascii(units: &[u16], ascii: &str) -> bool {
    units.len() == ascii.len()
        && units
            .iter()
            .copied()
            .zip(ascii.bytes())
            .all(|(unit, byte)| unit == u16::from(byte))
}

fn bool_tag_value(property: &PropertyRecord<'_>) -> Result<bool> {
    ensure!(
        property.tag.kind == TagKind::Bool && property.tag.payload.is_empty(),
        "invalid BoolProperty tag"
    );
    property
        .bool_value
        .context("missing BoolProperty header value")
}

fn parse_lobby_privacy(property: &PropertyRecord<'_>) -> Result<LobbyPrivacy> {
    ensure!(property.index == 0, "nonzero LobbyPrivacy array index");
    match property.tag.kind {
        TagKind::Byte if property.tag.payload.len() == 1 => {
            LobbyPrivacy::from_saved(property.tag.payload[0]).context("unknown LobbyPrivacy byte")
        }
        TagKind::Enum | TagKind::Byte => {
            ensure!(
                extra_is(property, "ELobbyPrivacy"),
                "unexpected LobbyPrivacy enum type"
            );
            let units = decode_exact_string_units(
                property.tag.payload,
                "invalid LobbyPrivacy enum string",
            )?;
            if units_eq_ascii(&units, "ELobbyPrivacy::Unknown") || units_eq_ascii(&units, "Unknown")
            {
                Ok(LobbyPrivacy::Unknown)
            } else if units_eq_ascii(&units, "ELobbyPrivacy::FriendsOnly")
                || units_eq_ascii(&units, "FriendsOnly")
            {
                Ok(LobbyPrivacy::FriendsOnly)
            } else if units_eq_ascii(&units, "ELobbyPrivacy::Private")
                || units_eq_ascii(&units, "Private")
            {
                Ok(LobbyPrivacy::Private)
            } else {
                bail!("unknown LobbyPrivacy enumerator");
            }
        }
        _ => bail!("unsupported LobbyPrivacy tag kind"),
    }
}

fn parse_owner_resolve_policy(property: &PropertyRecord<'_>) -> Result<OwnerResolvePolicy> {
    ensure!(
        property.index == 0,
        "nonzero OwnerResolvePolicy array index"
    );
    match property.tag.kind {
        TagKind::Byte if property.tag.payload.len() == 1 => {
            OwnerResolvePolicy::from_saved(property.tag.payload[0])
                .context("unknown OwnerResolvePolicy byte")
        }
        TagKind::Enum | TagKind::Byte => {
            ensure!(
                extra_is(property, "EStateRecorderOwnerResolvePolicy"),
                "unexpected OwnerResolvePolicy enum type"
            );
            let units = decode_exact_string_units(
                property.tag.payload,
                "invalid OwnerResolvePolicy enum string",
            )?;
            if units_eq_ascii(&units, "EStateRecorderOwnerResolvePolicy::FindOnly")
                || units_eq_ascii(&units, "FindOnly")
            {
                Ok(OwnerResolvePolicy::FindOnly)
            } else if units_eq_ascii(&units, "EStateRecorderOwnerResolvePolicy::RespawnOnly")
                || units_eq_ascii(&units, "RespawnOnly")
            {
                Ok(OwnerResolvePolicy::RespawnOnly)
            } else if units_eq_ascii(&units, "EStateRecorderOwnerResolvePolicy::FindOrRespawn")
                || units_eq_ascii(&units, "FindOrRespawn")
            {
                Ok(OwnerResolvePolicy::FindOrRespawn)
            } else if units_eq_ascii(&units, "EStateRecorderOwnerResolvePolicy::ManuallyResolved")
                || units_eq_ascii(&units, "ManuallyResolved")
            {
                Ok(OwnerResolvePolicy::ManuallyResolved)
            } else {
                bail!("unknown OwnerResolvePolicy enumerator");
            }
        }
        _ => bail!("unsupported OwnerResolvePolicy tag kind"),
    }
}

fn retain_root_enum(
    property: &PropertyRecord<'_>,
    enum_name: &str,
    names: &[&str],
    field: StagedRootEnumField,
    source_values: &mut Vec<StagedRootEnum>,
) -> Result<u8> {
    let (wrong_type, invalid_name, invalid_byte_name, wrong_kind) = match field {
        StagedRootEnumField::ProspectState => (
            "ProspectInfo.ProspectState must use EProspectState",
            "invalid ProspectState enum",
            "invalid ProspectState byte enum",
            "ProspectInfo.ProspectState must be EnumProperty or ByteProperty",
        ),
        StagedRootEnumField::Difficulty => (
            "ProspectInfo.Difficulty must use EMissionDifficulty",
            "invalid Difficulty enum",
            "invalid Difficulty byte enum",
            "ProspectInfo.Difficulty must be EnumProperty or ByteProperty",
        ),
        StagedRootEnumField::AssociatedMemberStatus(_) => (
            "AssociatedMemberInfo.Status must use EProspectLocation",
            "invalid AssociatedMemberInfo.Status enum string",
            "invalid AssociatedMemberInfo.Status byte enum",
            "AssociatedMemberInfo.Status must be EnumProperty or ByteProperty",
        ),
    };
    let source_value = match property.tag.kind {
        TagKind::Enum => {
            ensure!(extra_is(property, enum_name), wrong_type);
            StagedRootEnumValue::Name(decode_exact_string_units(
                property.tag.payload,
                invalid_name,
            )?)
        }
        TagKind::Byte if property.tag.payload.len() == 1 => {
            StagedRootEnumValue::Byte(property.tag.payload[0])
        }
        TagKind::Byte => StagedRootEnumValue::Name(decode_exact_string_units(
            property.tag.payload,
            invalid_byte_name,
        )?),
        _ => bail!(wrong_kind),
    };
    let resolved_value = match &source_value {
        StagedRootEnumValue::Byte(value) => (usize::from(*value) < names.len()).then_some(*value),
        StagedRootEnumValue::Name(units) => names
            .iter()
            .position(|name| {
                units_eq_ascii(units, name)
                    || units_eq_ascii(units, &format!("{enum_name}::{name}"))
            })
            .and_then(|index| u8::try_from(index).ok()),
    };
    let dto_value = match &source_value {
        StagedRootEnumValue::Byte(value) => *value,
        StagedRootEnumValue::Name(_) => resolved_value.unwrap_or(0),
    };
    source_values.push(StagedRootEnum {
        field,
        source_kind: property.tag.kind,
        source_value,
        resolved_value,
    });
    Ok(dto_value)
}

/// Validates `ProspectInfo.AssociatedMembers` against the reflected
/// `FAssociatedMemberInfo` descriptor (`0x145fad8e0` / `0x143dbc380`):
/// shared `StructProperty` header `"AssociatedMemberInfo"` and up to the 8
/// reflected fields (`AccountName`, `CharacterName`, `UserID`, `ChrSlot`,
/// `Experience`, `Status`, `Settled`, `IsCurrentlyPlaying`), returning the
/// complete post-default DTOs (`FAssociatedMemberInfo::Construct`
/// `0x143dbc230` zero-initializes `UserID = ""` and `ChrSlot = 0`).
fn validate_associated_members(
    payload: &[u8],
    profile: TagProfile,
    component_byte_limit: usize,
    source_enum_values: &mut Vec<StagedRootEnum>,
) -> Result<Vec<AssociatedMember>> {
    let members = read_tagged_struct_array_with_limit(payload, profile, component_byte_limit)
        .context("invalid AssociatedMembers struct array")?;
    let ParsedRecord::Property(shared_prop) = read_record(members.shared().record, profile)? else {
        bail!("invalid AssociatedMembers shared struct header");
    };
    ensure!(
        shared_prop.index == 0
            && is_name(&shared_prop.name, "AssociatedMembers")
            && extra_is(&shared_prop, "AssociatedMemberInfo"),
        "AssociatedMembers shared header must be StructProperty(AssociatedMemberInfo)"
    );
    let mut parsed_members = Vec::with_capacity(members.elements().len());
    for (member_index, element) in members.elements().iter().enumerate() {
        let mut seen: Vec<Vec<u16>> = Vec::with_capacity(element.fields().len());
        let mut member = AssociatedMember::default();
        for field_tag in element.fields() {
            let ParsedRecord::Property(field_prop) = read_record(field_tag.record, profile)? else {
                bail!("unexpected AssociatedMemberInfo terminator");
            };
            ensure!(
                field_prop.tag.record.len() == field_tag.record.len() && field_prop.index == 0,
                "invalid AssociatedMemberInfo field header"
            );
            ensure!(
                !seen.iter().any(|prior| prior == &field_prop.name),
                "duplicate AssociatedMemberInfo property"
            );
            seen.push(field_prop.name.clone());
            if is_name(&field_prop.name, "UserID") {
                ensure!(
                    field_prop.tag.kind == TagKind::String,
                    "AssociatedMemberInfo.UserID must be StrProperty"
                );
                member.user_id = decode_exact_string_units(
                    field_prop.tag.payload,
                    "invalid AssociatedMemberInfo.UserID",
                )?;
            } else if is_name(&field_prop.name, "AccountName")
                || is_name(&field_prop.name, "CharacterName")
            {
                ensure!(
                    field_prop.tag.kind == TagKind::String,
                    "AssociatedMemberInfo string field must be StrProperty"
                );
                let units = decode_exact_string_units(
                    field_prop.tag.payload,
                    "invalid AssociatedMemberInfo string",
                )?;
                if is_name(&field_prop.name, "AccountName") {
                    member.account_name = units;
                } else {
                    member.character_name = units;
                }
            } else if is_name(&field_prop.name, "ChrSlot") {
                ensure!(
                    field_prop.tag.kind == TagKind::Int && field_prop.tag.payload.len() == 4,
                    "AssociatedMemberInfo.ChrSlot must be 4-byte IntProperty"
                );
                member.chr_slot = i32::from_le_bytes(field_prop.tag.payload.try_into()?);
            } else if is_name(&field_prop.name, "Experience") {
                ensure!(
                    field_prop.tag.kind == TagKind::Int && field_prop.tag.payload.len() == 4,
                    "AssociatedMemberInfo int field must be 4-byte IntProperty"
                );
                member.experience = i32::from_le_bytes(field_prop.tag.payload.try_into()?);
            } else if is_name(&field_prop.name, "Status") {
                member.status = retain_root_enum(
                    &field_prop,
                    "EProspectLocation",
                    &PROSPECT_LOCATIONS,
                    StagedRootEnumField::AssociatedMemberStatus(member_index),
                    source_enum_values,
                )?;
            } else if is_name(&field_prop.name, "Settled")
                || is_name(&field_prop.name, "IsCurrentlyPlaying")
            {
                ensure!(
                    field_prop.tag.kind == TagKind::Bool && field_prop.tag.payload.is_empty(),
                    "AssociatedMemberInfo bool field must be BoolProperty"
                );
                let value = bool_tag_value(&field_prop)?;
                if is_name(&field_prop.name, "Settled") {
                    member.settled = value;
                } else {
                    member.is_currently_playing = value;
                }
            } else {
                bail!("unmapped AssociatedMemberInfo property");
            }
        }
        parsed_members.push(member);
    }
    Ok(parsed_members)
}

/// Validates `ProspectInfo.CustomSettings` against the reflected
/// `FCustomGameSetting` descriptor (`0x145fadbf0` / `0x143dbcf70`):
/// shared `StructProperty` header `"CustomGameSetting"` and the 2 reflected
/// fields (`SettingRowName`, `SettingValue`).
fn validate_custom_settings(
    payload: &[u8],
    profile: TagProfile,
    component_byte_limit: usize,
) -> Result<(Vec<CustomGameSetting>, Vec<Vec<u16>>)> {
    let mut parsed_settings = Vec::new();
    let mut row_names = Vec::new();
    let settings = read_tagged_struct_array_with_limit(payload, profile, component_byte_limit)
        .context("invalid CustomSettings struct array")?;
    let ParsedRecord::Property(shared_prop) = read_record(settings.shared().record, profile)?
    else {
        bail!("invalid CustomSettings shared struct header");
    };
    ensure!(
        shared_prop.index == 0
            && is_name(&shared_prop.name, "CustomSettings")
            && extra_is(&shared_prop, "CustomGameSetting"),
        "CustomSettings shared header must be StructProperty(CustomGameSetting)"
    );
    for element in settings.elements() {
        let mut setting = CustomGameSetting::default();
        let mut seen: Vec<Vec<u16>> = Vec::with_capacity(element.fields().len());
        for field_tag in element.fields() {
            let ParsedRecord::Property(field_prop) = read_record(field_tag.record, profile)? else {
                bail!("unexpected CustomGameSetting terminator");
            };
            ensure!(
                field_prop.tag.record.len() == field_tag.record.len() && field_prop.index == 0,
                "invalid CustomGameSetting field header"
            );
            ensure!(
                !seen.iter().any(|prior| prior == &field_prop.name),
                "duplicate CustomGameSetting property"
            );
            seen.push(field_prop.name.clone());
            if is_name(&field_prop.name, "SettingRowName") {
                ensure!(
                    field_prop.tag.kind == TagKind::String,
                    "CustomGameSetting.SettingRowName must be StrProperty"
                );
                setting.setting_row_name = decode_exact_string_units(
                    field_prop.tag.payload,
                    "invalid CustomGameSetting.SettingRowName",
                )?;
                row_names.push(setting.setting_row_name.clone());
            } else if is_name(&field_prop.name, "SettingValue") {
                ensure!(
                    field_prop.tag.kind == TagKind::Int && field_prop.tag.payload.len() == 4,
                    "CustomGameSetting.SettingValue must be 4-byte IntProperty"
                );
                setting.setting_value = i32::from_le_bytes(field_prop.tag.payload.try_into()?);
            } else {
                bail!("unmapped CustomGameSetting property");
            }
        }
        parsed_settings.push(setting);
    }
    Ok((parsed_settings, row_names))
}

#[derive(Default)]
struct ParsedProspectInfo {
    tag_count: usize,
    values: ProspectInfo,
    source_enum_values: Vec<StagedRootEnum>,
    setting_row_names: Vec<Vec<u16>>,
    has_expire_time: bool,
}

fn validate_prospect_info(
    payload: &[u8],
    profile: TagProfile,
    component_byte_limit: usize,
) -> Result<ParsedProspectInfo> {
    let nested = read_root(payload, profile).context("invalid ProspectInfo tagged sequence")?;
    let mut seen_names: Vec<Vec<u16>> = Vec::with_capacity(nested.tags.len());
    let mut info = ProspectInfo::default();
    let mut source_enum_values = Vec::new();
    let mut setting_row_names = Vec::new();
    let mut has_expire_time = false;

    for tag in &nested.tags {
        let ParsedRecord::Property(prop) = read_record(tag.record, profile)? else {
            bail!("unexpected ProspectInfo terminator");
        };
        ensure!(
            prop.tag.record.len() == tag.record.len() && prop.index == 0,
            "invalid ProspectInfo field header"
        );
        ensure!(
            !seen_names.iter().any(|prior| prior == &prop.name),
            "duplicate ProspectInfo property"
        );
        seen_names.push(prop.name.clone());

        if is_name(&prop.name, "ProspectID") {
            ensure!(
                prop.tag.kind == TagKind::String,
                "ProspectInfo.ProspectID must be StrProperty"
            );
            info.prospect_id = decode_exact_string_units(prop.tag.payload, "invalid ProspectID")?;
        } else if is_name(&prop.name, "ProspectDTKey") {
            ensure!(
                prop.tag.kind == TagKind::String,
                "ProspectInfo.ProspectDTKey must be StrProperty"
            );
            info.prospect_dt_key =
                decode_exact_string_units(prop.tag.payload, "invalid ProspectDTKey")?;
        } else if is_name(&prop.name, "FactionMissionDTKey") {
            ensure!(
                prop.tag.kind == TagKind::String,
                "ProspectInfo string field must be StrProperty"
            );
            info.faction_mission_dt_key =
                decode_exact_string_units(prop.tag.payload, "invalid ProspectInfo string")?;
        } else if is_name(&prop.name, "ClaimedAccountID") || is_name(&prop.name, "LobbyName") {
            ensure!(
                prop.tag.kind == TagKind::String,
                "ProspectInfo string field must be StrProperty"
            );
            let units = decode_exact_string_units(prop.tag.payload, "invalid ProspectInfo string")?;
            if is_name(&prop.name, "ClaimedAccountID") {
                info.claimed_account_id = units;
            } else {
                info.lobby_name = units;
            }
        } else if is_name(&prop.name, "ClaimedAccountCharacter")
            || is_name(&prop.name, "Cost")
            || is_name(&prop.name, "Reward")
            || is_name(&prop.name, "ElapsedTime")
            || is_name(&prop.name, "SelectedDropPoint")
        {
            ensure!(
                prop.tag.kind == TagKind::Int && prop.tag.payload.len() == 4,
                "ProspectInfo int field must be 4-byte IntProperty"
            );
            let value = i32::from_le_bytes(prop.tag.payload.try_into()?);
            if is_name(&prop.name, "ClaimedAccountCharacter") {
                info.claimed_account_character = value;
            } else if is_name(&prop.name, "Cost") {
                info.cost = value;
            } else if is_name(&prop.name, "Reward") {
                info.reward = value;
            } else if is_name(&prop.name, "ElapsedTime") {
                info.elapsed_time = value;
            } else {
                info.selected_drop_point = value;
            }
        } else if is_name(&prop.name, "ExpireTime") {
            ensure!(
                prop.tag.kind == TagKind::Int64 && prop.tag.payload.len() == 8,
                "ProspectInfo.ExpireTime must be 8-byte Int64Property"
            );
            info.expire_time = i64::from_le_bytes(prop.tag.payload.try_into()?);
            has_expire_time = true;
        } else if is_name(&prop.name, "ProspectState") {
            info.prospect_state = retain_root_enum(
                &prop,
                "EProspectState",
                &PROSPECT_STATES,
                StagedRootEnumField::ProspectState,
                &mut source_enum_values,
            )?;
        } else if is_name(&prop.name, "Difficulty") {
            info.difficulty = retain_root_enum(
                &prop,
                "EMissionDifficulty",
                &MISSION_DIFFICULTIES,
                StagedRootEnumField::Difficulty,
                &mut source_enum_values,
            )?;
        } else if is_name(&prop.name, "Insurance") || is_name(&prop.name, "NoRespawns") {
            ensure!(
                prop.tag.kind == TagKind::Bool && prop.tag.payload.is_empty(),
                "ProspectInfo bool field must be BoolProperty"
            );
            let value = bool_tag_value(&prop)?;
            if is_name(&prop.name, "Insurance") {
                info.insurance = value;
            } else {
                info.no_respawns = value;
            }
        } else if is_name(&prop.name, "AssociatedMembers") {
            ensure!(
                prop.tag.kind == TagKind::Array && extra_is(&prop, "StructProperty"),
                "ProspectInfo.AssociatedMembers must be ArrayProperty<StructProperty>"
            );
            info.associated_members = validate_associated_members(
                prop.tag.payload,
                profile,
                component_byte_limit,
                &mut source_enum_values,
            )?;
        } else if is_name(&prop.name, "CustomSettings") {
            ensure!(
                prop.tag.kind == TagKind::Array && extra_is(&prop, "StructProperty"),
                "ProspectInfo.CustomSettings must be ArrayProperty<StructProperty>"
            );
            (info.custom_settings, setting_row_names) =
                validate_custom_settings(prop.tag.payload, profile, component_byte_limit)?;
        } else {
            bail!("unmapped ProspectInfo property");
        }
    }

    Ok(ParsedProspectInfo {
        tag_count: nested.tags.len(),
        values: info,
        source_enum_values,
        setting_row_names,
        has_expire_time,
    })
}

const BLOB_CLASS_UTF16_CRCS: [u32; 38] = [
    0x95ebf1cb, 0x001c9e29, 0x4873a81f, 0x324de345, 0x9c43c322, 0x30638715, 0xc95eeade, 0x15a8bf7b,
    0xa3827c97, 0x1e074606, 0x524eff9e, 0x47fe1e70, 0x765c03dc, 0x469d0a2c, 0xc2f3dd4b, 0x67c586aa,
    0xe62af98d, 0x00ec1192, 0x9d1516f6, 0xd54e9abd, 0xbbf0cd55, 0x5a062917, 0x58b4ab5c, 0x191d6d2e,
    0x3ce30648, 0x343449f1, 0xd8007aeb, 0x13e2d008, 0x23018b5e, 0xeda63d84, 0x97fe0e5d, 0x96a5d2af,
    0xf214af89, 0x1b4f4011, 0x179955df, 0xa98583bb, 0x516fb5b4, 0x7926eefe,
];

const NON_BLOB_CLASS_UTF16_CRCS: [u32; 27] = [
    0x9e5ca79a, 0x8ea53657, 0xf4037559, 0xab6785f9, 0x317a7c00, 0x3aaecdc1, 0x2f9dd47f, 0xb278323f,
    0xf616acbc, 0x4ca7220a, 0x0c42acca, 0x776c69d6, 0x410ad4f7, 0x21c98fa4, 0x4b3d103d, 0xf087575f,
    0x3ab7a6a0, 0x9389d141, 0x7f184a56, 0xedb652f7, 0xa67dfe44, 0xc3e4f570, 0x85dc5a0e, 0xb5929f89,
    0x59cc7a1d, 0x93f08633, 0x1d51a6d6,
];

/// Resolves the constructor defaults (`+0xb0..+0xb2`, `+0xd8..+0xd9`) and
/// virtual-slot `+0x470` / `+0x498` / `+0x4a8` profile for a
/// `/Script/Icarus.*RecorderComponent` UTF-16 class path across all 65
/// registered classes (`C001..C038` and `N001..N027`), or `None` if the class
/// path is unrecognized (F-746, F-747).
pub fn lookup_recorder_ctor_defaults(class_units: &[u16]) -> Option<RecorderCtorDefaults> {
    let crc = checksum_utf16(class_units, 0);
    if let Some(index) = BLOB_CLASS_UTF16_CRCS.iter().position(|&entry| entry == crc) {
        return recorder_ctor_defaults((index + 1) as u8);
    }
    if let Some(index) = NON_BLOB_CLASS_UTF16_CRCS
        .iter()
        .position(|&entry| entry == crc)
    {
        return non_blob_recorder_ctor_defaults((index + 1) as u8);
    }
    None
}

/// The project pseudonym (`C001..C038` blob classes, `N001..N027` others) of a
/// `/Script/Icarus.*RecorderComponent` UTF-16 class path, or `None` if the path
/// is not one of the 65 registered classes. Pseudonyms keep reports free of
/// supplied class strings.
pub fn recorder_class_label(class_units: &[u16]) -> Option<String> {
    let crc = checksum_utf16(class_units, 0);
    if let Some(index) = BLOB_CLASS_UTF16_CRCS.iter().position(|&entry| entry == crc) {
        return Some(format!("C{:03}", index + 1));
    }
    NON_BLOB_CLASS_UTF16_CRCS
        .iter()
        .position(|&entry| entry == crc)
        .map(|index| format!("N{:03}", index + 1))
}

struct StagedRecorderEval {
    is_valid: bool,
    was_reset_rocket_spawn: bool,
    is_physics_dynamic_tree: bool,
    skip_default_owner_resolution: bool,
    owner_resolve_policy: OwnerResolvePolicy,
    requires_stitching: bool,
}

/// Validates that every top-level property in a staged recorder's `BaseComponent`
/// has `index == 0` (`array_dim == 1` across all 65 `/Script/Icarus.*RecorderComponent`
/// classes) and a unique property name (`UStruct::SerializeVersionedTaggedProperties`).
fn validate_staged_component_properties(
    component: &BaseComponent<'_>,
    profile: TagProfile,
) -> Result<()> {
    let mut seen_names: Vec<Vec<u16>> = Vec::with_capacity(component.fields().len());
    for field_tag in component.fields() {
        let ParsedRecord::Property(prop) = read_record(field_tag.record, profile)? else {
            bail!("unexpected staged recorder component terminator");
        };
        ensure!(
            prop.tag.record.len() == field_tag.record.len() && prop.index == 0,
            "invalid staged recorder component field header"
        );
        ensure!(
            !seen_names.iter().any(|prior| prior == &prop.name),
            "duplicate staged recorder component property"
        );
        seen_names.push(prop.name);
    }
    Ok(())
}

fn parse_player_character_id_struct(
    payload: &[u8],
    profile: TagProfile,
) -> Result<(Vec<u16>, i32)> {
    let nested = read_root(payload, profile).context("invalid PlayerCharacterID struct")?;
    let mut seen_names: Vec<Vec<u16>> = Vec::with_capacity(nested.tags.len());
    let mut player_id = Vec::new();
    let mut chr_slot = -1_i32;
    for tag in &nested.tags {
        let ParsedRecord::Property(prop) = read_record(tag.record, profile)? else {
            bail!("unexpected PlayerCharacterID terminator");
        };
        ensure!(
            prop.tag.record.len() == tag.record.len() && prop.index == 0,
            "invalid PlayerCharacterID field header"
        );
        ensure!(
            !seen_names.iter().any(|prior| prior == &prop.name),
            "duplicate PlayerCharacterID property"
        );
        seen_names.push(prop.name.clone());
        if is_name(&prop.name, "PlayerID") {
            ensure!(
                prop.tag.kind == TagKind::String,
                "PlayerCharacterID.PlayerID must be StrProperty"
            );
            player_id =
                decode_exact_string_units(prop.tag.payload, "invalid PlayerCharacterID.PlayerID")?;
        } else if is_name(&prop.name, "ChrSlot") {
            ensure!(
                prop.tag.kind == TagKind::Int && prop.tag.payload.len() == 4,
                "PlayerCharacterID.ChrSlot must be 4-byte IntProperty"
            );
            chr_slot = i32::from_le_bytes(prop.tag.payload.try_into()?);
        } else {
            bail!("unmapped PlayerCharacterID property");
        }
    }
    Ok((player_id, chr_slot))
}

fn evaluate_staged_recorder_component(
    class_units: &[u16],
    component: &BaseComponent<'_>,
    profile: TagProfile,
    active_prospect_id: &[u16],
    associated_members: &[ProspectAssociatedMember<'_>],
) -> Result<StagedRecorderEval> {
    let defaults = lookup_recorder_ctor_defaults(class_units)
        .ok_or_else(|| anyhow::anyhow!("unrecognized recorder ComponentClassName"))?;
    let mut skip_default_owner_resolution = defaults.b_skip_default_owner_resolution;
    let mut owner_resolve_policy = defaults.owner_resolve_policy;
    let mut parent_character_uid = -1_i32;
    for field_tag in component.fields() {
        let ParsedRecord::Property(prop) = read_record(field_tag.record, profile)? else {
            bail!("unexpected staged recorder component terminator");
        };
        if is_name(&prop.name, "bSkipDefaultOwnerResolution") {
            skip_default_owner_resolution = bool_tag_value(&prop)?;
        } else if is_name(&prop.name, "OwnerResolvePolicy") {
            owner_resolve_policy = parse_owner_resolve_policy(&prop)?;
        } else if is_name(&prop.name, "ParentCharacterUID") {
            ensure!(
                prop.tag.kind == TagKind::Int && prop.index == 0 && prop.tag.payload.len() == 4,
                "ParentCharacterUID must be 4-byte IntProperty"
            );
            parent_character_uid = i32::from_le_bytes(prop.tag.payload.try_into()?);
        }
    }
    let requires_stitching =
        evaluate_requires_stitching(defaults.requires_stitching, parent_character_uid);
    let build_eval =
        |is_valid: bool, was_reset_rocket_spawn: bool, is_physics_dynamic_tree: bool| {
            StagedRecorderEval {
                is_valid,
                was_reset_rocket_spawn,
                is_physics_dynamic_tree,
                skip_default_owner_resolution,
                owner_resolve_policy,
                requires_stitching,
            }
        };

    if units_eq_ascii(class_units, "/Script/Icarus.NoRecorderComponent") {
        return Ok(build_eval(no_recorder_record_valid(), false, false));
    }
    if units_eq_ascii(
        class_units,
        "/Script/Icarus.DynamicRocketSpawnRecorderComponent",
    ) || units_eq_ascii(class_units, "/Script/Icarus.RocketSpawnRecorderComponent")
    {
        let mut assigned_player_id = Vec::new();
        let mut chr_slot = -1_i32;
        let mut version = 0_i32;
        for field_tag in component.fields() {
            let ParsedRecord::Property(prop) = read_record(field_tag.record, profile)? else {
                bail!("unexpected RocketSpawnRecorderComponent terminator");
            };
            if is_name(&prop.name, "SpawnRecord") {
                ensure!(
                    prop.tag.kind == TagKind::Struct
                        && prop.index == 0
                        && extra_is(&prop, "RocketSpawnStateRecord"),
                    "SpawnRecord must be StructProperty(RocketSpawnStateRecord)"
                );
                let nested = read_root(prop.tag.payload, profile)
                    .context("invalid RocketSpawnStateRecord struct")?;
                let mut seen_inner: Vec<Vec<u16>> = Vec::with_capacity(nested.tags.len());
                for inner_tag in &nested.tags {
                    let ParsedRecord::Property(inner_prop) =
                        read_record(inner_tag.record, profile)?
                    else {
                        bail!("unexpected RocketSpawnStateRecord terminator");
                    };
                    ensure!(
                        inner_prop.tag.record.len() == inner_tag.record.len()
                            && inner_prop.index == 0,
                        "invalid RocketSpawnStateRecord field header"
                    );
                    ensure!(
                        !seen_inner.iter().any(|prior| prior == &inner_prop.name),
                        "duplicate RocketSpawnStateRecord property"
                    );
                    seen_inner.push(inner_prop.name.clone());
                    if is_name(&inner_prop.name, "AssignedPlayerID") {
                        ensure!(
                            inner_prop.tag.kind == TagKind::String,
                            "RocketSpawnStateRecord.AssignedPlayerID must be StrProperty"
                        );
                        assigned_player_id = decode_exact_string_units(
                            inner_prop.tag.payload,
                            "invalid RocketSpawnStateRecord.AssignedPlayerID",
                        )?;
                    } else if is_name(&inner_prop.name, "ChrSlot") {
                        ensure!(
                            inner_prop.tag.kind == TagKind::Int
                                && inner_prop.tag.payload.len() == 4,
                            "RocketSpawnStateRecord.ChrSlot must be 4-byte IntProperty"
                        );
                        chr_slot = i32::from_le_bytes(inner_prop.tag.payload.try_into()?);
                    }
                }
            } else if is_name(&prop.name, "RocketSpawnRecorderVersion") {
                ensure!(
                    prop.tag.kind == TagKind::Int && prop.index == 0 && prop.tag.payload.len() == 4,
                    "RocketSpawnRecorderVersion must be 4-byte IntProperty"
                );
                version = i32::from_le_bytes(prop.tag.payload.try_into()?);
            }
        }
        let list = resolve_prospect_player_list(
            true,
            active_prospect_id,
            associated_members,
            &assigned_player_id,
            chr_slot,
        );
        let (is_valid, was_reset) =
            validate_rocket_spawn_record(version, list, &mut assigned_player_id, &mut chr_slot);
        return Ok(build_eval(is_valid, was_reset, false));
    }

    if units_eq_ascii(class_units, "/Script/Icarus.PlayerRecorderComponent") {
        let mut player_id = Vec::new();
        let mut chr_slot = -1_i32;
        let mut version = 0_i32;
        for field_tag in component.fields() {
            let ParsedRecord::Property(prop) = read_record(field_tag.record, profile)? else {
                bail!("unexpected PlayerRecorderComponent terminator");
            };
            if is_name(&prop.name, "PlayerCharacterID") {
                ensure!(
                    prop.tag.kind == TagKind::Struct
                        && prop.index == 0
                        && extra_is(&prop, "PlayerCharacterID"),
                    "PlayerRecorderComponent.PlayerCharacterID must be StructProperty(PlayerCharacterID)"
                );
                (player_id, chr_slot) =
                    parse_player_character_id_struct(prop.tag.payload, profile)?;
            } else if is_name(&prop.name, "PlayerRecorderVersion") {
                ensure!(
                    prop.tag.kind == TagKind::Int && prop.index == 0 && prop.tag.payload.len() == 4,
                    "PlayerRecorderVersion must be 4-byte IntProperty"
                );
                version = i32::from_le_bytes(prop.tag.payload.try_into()?);
            }
        }
        let list = resolve_prospect_player_list(
            true,
            active_prospect_id,
            associated_members,
            &player_id,
            chr_slot,
        );
        return Ok(build_eval(player_record_valid(version, list), false, false));
    }

    if units_eq_ascii(class_units, "/Script/Icarus.PlayerStateRecorderComponent") {
        let mut player_id = Vec::new();
        let mut chr_slot = -1_i32;
        let mut version = 0_i32;
        for field_tag in component.fields() {
            let ParsedRecord::Property(prop) = read_record(field_tag.record, profile)? else {
                bail!("unexpected PlayerStateRecorderComponent terminator");
            };
            if is_name(&prop.name, "PlayerCharacterID") {
                ensure!(
                    prop.tag.kind == TagKind::Struct
                        && prop.index == 0
                        && extra_is(&prop, "PlayerCharacterID"),
                    "PlayerStateRecorderComponent.PlayerCharacterID must be StructProperty(PlayerCharacterID)"
                );
                (player_id, chr_slot) =
                    parse_player_character_id_struct(prop.tag.payload, profile)?;
            } else if is_name(&prop.name, "PlayerStateRecorderVersion") {
                ensure!(
                    prop.tag.kind == TagKind::Int && prop.index == 0 && prop.tag.payload.len() == 4,
                    "PlayerStateRecorderVersion must be 4-byte IntProperty"
                );
                version = i32::from_le_bytes(prop.tag.payload.try_into()?);
            }
        }
        let list = resolve_prospect_player_list(
            true,
            active_prospect_id,
            associated_members,
            &player_id,
            chr_slot,
        );
        return Ok(build_eval(player_record_valid(version, list), false, false));
    }

    if units_eq_ascii(
        class_units,
        "/Script/Icarus.ResourceDepositRecorderComponent",
    ) {
        let mut resource_dt_key = Vec::new();
        for field_tag in component.fields() {
            let ParsedRecord::Property(prop) = read_record(field_tag.record, profile)? else {
                bail!("unexpected ResourceDepositRecorderComponent terminator");
            };
            if is_name(&prop.name, "ResourceDTKey") {
                ensure!(
                    prop.tag.kind == TagKind::Name && prop.index == 0,
                    "ResourceDepositRecorderComponent.ResourceDTKey must be NameProperty"
                );
                resource_dt_key = decode_exact_string_units(
                    prop.tag.payload,
                    "invalid ResourceDepositRecorderComponent.ResourceDTKey",
                )?;
            }
        }
        return Ok(build_eval(
            resource_deposit_record_valid(&resource_dt_key),
            false,
            false,
        ));
    }

    if units_eq_ascii(class_units, "/Script/Icarus.RocketRecorderComponent") {
        let mut player_id = Vec::new();
        let mut chr_slot = -1_i32;
        for field_tag in component.fields() {
            let ParsedRecord::Property(prop) = read_record(field_tag.record, profile)? else {
                bail!("unexpected RocketRecorderComponent terminator");
            };
            if is_name(&prop.name, "AssignedPlayerCharacterID") {
                ensure!(
                    prop.tag.kind == TagKind::Struct
                        && prop.index == 0
                        && extra_is(&prop, "PlayerCharacterID"),
                    "RocketRecorderComponent.AssignedPlayerCharacterID must be StructProperty(PlayerCharacterID)"
                );
                (player_id, chr_slot) =
                    parse_player_character_id_struct(prop.tag.payload, profile)?;
            } else if is_name(&prop.name, "RocketRecorderVersion") {
                ensure!(
                    prop.tag.kind == TagKind::Int && prop.index == 0 && prop.tag.payload.len() == 4,
                    "RocketRecorderVersion must be 4-byte IntProperty"
                );
            }
        }
        let list = resolve_prospect_player_list(
            true,
            active_prospect_id,
            associated_members,
            &player_id,
            chr_slot,
        );
        return Ok(build_eval(rocket_record_valid(list), false, false));
    }

    if units_eq_ascii(class_units, "/Script/Icarus.SpawnedVoxelRecorderComponent")
        || units_eq_ascii(class_units, "/Script/Icarus.VoxelRecorderComponent")
    {
        let mut fully_mined = false;
        for field_tag in component.fields() {
            let ParsedRecord::Property(prop) = read_record(field_tag.record, profile)? else {
                bail!("unexpected VoxelRecorderComponent terminator");
            };
            if is_name(&prop.name, "VoxelSaveData") {
                ensure!(
                    prop.tag.kind == TagKind::Struct
                        && prop.index == 0
                        && extra_is(&prop, "VoxelSaveData"),
                    "VoxelSaveData must be StructProperty(VoxelSaveData)"
                );
                let nested =
                    read_root(prop.tag.payload, profile).context("invalid VoxelSaveData struct")?;
                let mut seen_inner: Vec<Vec<u16>> = Vec::with_capacity(nested.tags.len());
                for inner_tag in &nested.tags {
                    let ParsedRecord::Property(inner_prop) =
                        read_record(inner_tag.record, profile)?
                    else {
                        bail!("unexpected VoxelSaveData terminator");
                    };
                    ensure!(
                        inner_prop.tag.record.len() == inner_tag.record.len()
                            && inner_prop.index == 0,
                        "invalid VoxelSaveData field header"
                    );
                    ensure!(
                        !seen_inner.iter().any(|prior| prior == &inner_prop.name),
                        "duplicate VoxelSaveData property"
                    );
                    seen_inner.push(inner_prop.name.clone());
                    if is_name(&inner_prop.name, "bIsVoxelFullyMined") {
                        fully_mined = bool_tag_value(&inner_prop)?;
                    }
                }
            }
        }
        return Ok(build_eval(
            voxel_record_valid(owner_resolve_policy, fully_mined),
            false,
            false,
        ));
    }

    if units_eq_ascii(class_units, "/Script/Icarus.TreeRecorderComponent") {
        let mut is_physics_dynamic = false;
        for field_tag in component.fields() {
            let ParsedRecord::Property(prop) = read_record(field_tag.record, profile)? else {
                bail!("unexpected TreeRecorderComponent terminator");
            };
            if is_name(&prop.name, "bIsPhysicsDynamic") {
                ensure!(
                    prop.index == 0,
                    "TreeRecorderComponent.bIsPhysicsDynamic must have index 0"
                );
                is_physics_dynamic = bool_tag_value(&prop)?;
            }
        }
        return Ok(build_eval(true, false, is_physics_dynamic));
    }

    Ok(build_eval(true, false, false))
}

/// Stages and validates a complete `FProspectSaveState` root tag slice and all
/// of its `StateRecorderBlobs` bodies before any state publication (`F-328`,
/// `F-330`, `F-403`, `F-715`, `F-718`, `F-726`, `F-733`).
///
/// - Accepts out-of-order and omitted default root properties (`F-718`),
///   rejecting duplicate or unmapped root property names (`F-403`).
/// - Applies `FProspectSaveState::PostLoadSerialize` schema migrations (`F-715`):
///   copies legacy root `ProspectID`/`ProspectDTKey` when `Version < 2`, and
///   resets `ProspectInfo`, sets `LobbyPrivacy = FriendsOnly`, and clears
///   `StateRecorderBlobs` when `Version == 0`.
/// - Frames every recorder's `BinaryData` via [`read_base_component_with_limit`],
///   rejects any recorder with unknown outer fields or duplicate/nonzero-indexed
///   component properties (`F-403`, `F-741`).
/// - Evaluates the 7 non-trivial `UIcarusStateRecorderComponent::Validate`
///   (`+0x408`) overrides (`C011`, `C025`, `C026`, `C028`, `C029`, `C031`, `C034`)
///   and `C033` `bIsPhysicsDynamic` against the post-migration `ProspectInfo`
///   (`F-652`, `F-663`, `F-680`, `F-681`).
/// - When `expected_prospect_id` is `Some`, verifies that the post-migration
///   `ProspectInfo.ProspectID` is non-empty and matches ASCII case-insensitively
///   (`UProspectSubsystem::SetActiveProspectSaveState` `0x1418371c0`, `F-717`,
///   `F-733`).
pub fn stage_prospect_save_state(
    roots: &[RootTag<'_>],
    profile: TagProfile,
    component_byte_limit: usize,
    expected_prospect_id: Option<&[u16]>,
) -> Result<StagedProspectSaveState> {
    stage_prospect_save_state_with_decisions(
        roots,
        profile,
        component_byte_limit,
        expected_prospect_id,
    )
    .map(|(staged, _)| staged)
}

/// One saved player character a returning player's pawn restores from: a
/// `C026` `PlayerStateRecorderComponent` that `Validate` retains, with its
/// `PlayerCharacterID`, the `IcarusActorGUID` its restore claims (F-950) and
/// the fields its restore applies to the pawn (F-960).
#[derive(Clone, PartialEq)]
pub struct SavedPlayerCharacter {
    pub player_id: Vec<u16>,
    pub chr_slot: i32,
    /// `-1` when the record carries none.
    pub icarus_uid: i32,
    pub state: SavedPlayerState,
    /// The inherited actor-state inventories the base restore applies
    /// (F-965, F-969), in saved order.
    pub inventories: Vec<SavedActorInventory>,
    /// The inherited actor-state modifiers, in saved order.
    pub modifiers: Vec<SavedModifierState>,
}

impl std::fmt::Debug for SavedPlayerCharacter {
    // Player IDs are private identifiers; keep them out of debug output.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SavedPlayerCharacter")
            .field("player_id_units", &self.player_id.len())
            .field("chr_slot", &self.chr_slot)
            .field("icarus_uid", &self.icarus_uid)
            .field("state", &self.state)
            .field("inventories", &self.inventories)
            .field("modifiers", &self.modifiers.len())
            .finish()
    }
}

/// The `C026` fields `UpdateOwnerToDatabaseState` applies to a survival pawn
/// (F-960), by their reflected names. A scalar the record omits is `None`:
/// the original then keeps the recorder's constructed default (F-718), which
/// has not been read. An omitted array is empty.
#[derive(Clone, Default, PartialEq)]
pub struct SavedPlayerState {
    pub location: Option<[f32; 3]>,
    /// `[Pitch, Yaw, Roll]`.
    pub rotation: Option<[f32; 3]>,
    pub is_alive: Option<bool>,
    pub health: Option<i32>,
    /// `-1` is unrecorded (F-648).
    pub food_level: Option<i32>,
    pub water_level: Option<i32>,
    pub oxygen_level: Option<i32>,
    pub radiation_level: Option<i32>,
    pub stomach_contents: Vec<SavedStomachContent>,
    pub respawn_count: Option<i32>,
    pub has_granted_loadout: Option<bool>,
    /// Item-static row names.
    pub cosmetic_armour_overrides: Vec<Vec<u16>>,
}

impl std::fmt::Debug for SavedPlayerState {
    // Saved values are supplied data; show only which fields are present.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SavedPlayerState")
            .field("location", &self.location.is_some())
            .field("rotation", &self.rotation.is_some())
            .field("is_alive", &self.is_alive.is_some())
            .field("health", &self.health.is_some())
            .field("food_level", &self.food_level.is_some())
            .field("water_level", &self.water_level.is_some())
            .field("oxygen_level", &self.oxygen_level.is_some())
            .field("radiation_level", &self.radiation_level.is_some())
            .field("stomach_contents", &self.stomach_contents.len())
            .field("respawn_count", &self.respawn_count.is_some())
            .field("has_granted_loadout", &self.has_granted_loadout.is_some())
            .field(
                "cosmetic_armour_overrides",
                &self.cosmetic_armour_overrides.len(),
            )
            .finish()
    }
}

/// One saved `FStomachContentSaveData`: its food's item-static row name and
/// its modifier's row name; an omitted name is empty.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct SavedStomachContent {
    pub food_row_name: Vec<u16>,
    pub modifier_name: Vec<u16>,
}

impl std::fmt::Debug for SavedStomachContent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SavedStomachContent")
    }
}

fn int_tag_value(property: &PropertyRecord<'_>, message: &'static str) -> Result<i32> {
    ensure!(
        property.tag.kind == TagKind::Int && property.tag.payload.len() == 4,
        message
    );
    Ok(i32::from_le_bytes(property.tag.payload.try_into()?))
}

/// A natively serialized `Vector` or `Rotator` struct: three 4-byte floats.
fn float3_struct_value(
    property: &PropertyRecord<'_>,
    struct_name: &str,
    message: &'static str,
) -> Result<[f32; 3]> {
    ensure!(
        property.tag.kind == TagKind::Struct
            && extra_is(property, struct_name)
            && property.tag.payload.len() == 12,
        message
    );
    let (words, _) = property.tag.payload.as_chunks::<4>();
    Ok([0, 1, 2].map(|index| f32::from_le_bytes(words[index])))
}

/// An `ArrayProperty<NameProperty>`: a count, then that many names.
fn name_array_value(property: &PropertyRecord<'_>, message: &'static str) -> Result<Vec<Vec<u16>>> {
    ensure!(
        property.tag.kind == TagKind::Array && extra_is(property, "NameProperty"),
        message
    );
    let payload = property.tag.payload;
    let count = payload.get(..4).context(message)?;
    let count = usize::try_from(i32::from_le_bytes(count.try_into()?)).context(message)?;
    let mut rest = &payload[4..];
    // Every name takes at least its 4-byte length.
    ensure!(count <= rest.len() / 4, message);
    let mut names = Vec::with_capacity(count);
    for _ in 0..count {
        let length = rest.get(..4).context(message)?;
        ensure!(
            i32::from_le_bytes(length.try_into()?).unsigned_abs() as usize
                <= MAX_HEADER_STRING_UNITS,
            "string exceeds research unit limit"
        );
        let decoded = decode_string(rest, ByteOrder::Little).context(message)?;
        names.push(decoded.units);
        rest = &rest[decoded.consumed..];
    }
    ensure!(rest.is_empty(), message);
    Ok(names)
}

fn parse_stomach_contents(
    property: &PropertyRecord<'_>,
    profile: TagProfile,
    component_byte_limit: usize,
) -> Result<Vec<SavedStomachContent>> {
    ensure!(
        property.tag.kind == TagKind::Array && extra_is(property, "StructProperty"),
        "StomachContents must be ArrayProperty<StructProperty>"
    );
    let array =
        read_tagged_struct_array_with_limit(property.tag.payload, profile, component_byte_limit)
            .context("invalid StomachContents struct array")?;
    let ParsedRecord::Property(shared) = read_record(array.shared().record, profile)? else {
        bail!("invalid StomachContents shared struct header");
    };
    ensure!(
        shared.index == 0
            && is_name(&shared.name, "StomachContents")
            && extra_is(&shared, "StomachContentSaveData"),
        "StomachContents shared header must be StructProperty(StomachContentSaveData)"
    );
    let mut contents = Vec::with_capacity(array.elements().len());
    for element in array.elements() {
        let mut seen: Vec<Vec<u16>> = Vec::with_capacity(element.fields().len());
        let mut content = SavedStomachContent::default();
        for field in element.fields() {
            let ParsedRecord::Property(prop) = read_record(field.record, profile)? else {
                bail!("unexpected StomachContentSaveData terminator");
            };
            ensure!(
                prop.tag.record.len() == field.record.len() && prop.index == 0,
                "invalid StomachContentSaveData field header"
            );
            ensure!(
                !seen.iter().any(|prior| prior == &prop.name),
                "duplicate StomachContentSaveData property"
            );
            seen.push(prop.name.clone());
            let target = if is_name(&prop.name, "FoodRowName") {
                &mut content.food_row_name
            } else if is_name(&prop.name, "ModifierName") {
                &mut content.modifier_name
            } else {
                bail!("unmapped StomachContentSaveData property");
            };
            ensure!(
                prop.tag.kind == TagKind::Name,
                "StomachContentSaveData names must be NameProperty"
            );
            *target =
                decode_exact_string_units(prop.tag.payload, "invalid StomachContentSaveData name")?;
        }
        contents.push(content);
    }
    Ok(contents)
}

/// Reads one restored `C026` field into `state`; `false` when `prop` is not
/// one of them. A restored field of the wrong shape rejects the save.
fn read_saved_player_state_field(
    state: &mut SavedPlayerState,
    prop: &PropertyRecord<'_>,
    profile: TagProfile,
    component_byte_limit: usize,
) -> Result<bool> {
    let name = prop.name.as_slice();
    let int = |message| int_tag_value(prop, message).map(Some);
    if is_name(name, "Location") {
        state.location = Some(float3_struct_value(
            prop,
            "Vector",
            "Location must be StructProperty(Vector)",
        )?);
    } else if is_name(name, "Rotation") {
        state.rotation = Some(float3_struct_value(
            prop,
            "Rotator",
            "Rotation must be StructProperty(Rotator)",
        )?);
    } else if is_name(name, "bIsAlive") {
        state.is_alive = Some(bool_tag_value(prop)?);
    } else if is_name(name, "Health") {
        state.health = int("Health must be 4-byte IntProperty")?;
    } else if is_name(name, "FoodLevel") {
        state.food_level = int("FoodLevel must be 4-byte IntProperty")?;
    } else if is_name(name, "WaterLevel") {
        state.water_level = int("WaterLevel must be 4-byte IntProperty")?;
    } else if is_name(name, "OxygenLevel") {
        state.oxygen_level = int("OxygenLevel must be 4-byte IntProperty")?;
    } else if is_name(name, "RadiationLevel") {
        state.radiation_level = int("RadiationLevel must be 4-byte IntProperty")?;
    } else if is_name(name, "StomachContents") {
        state.stomach_contents = parse_stomach_contents(prop, profile, component_byte_limit)?;
    } else if is_name(name, "RespawnCount") {
        state.respawn_count = int("RespawnCount must be 4-byte IntProperty")?;
    } else if is_name(name, "HasGrantedLoadout") {
        state.has_granted_loadout = Some(bool_tag_value(prop)?);
    } else if is_name(name, "CosmeticArmourOverrides") {
        state.cosmetic_armour_overrides = name_array_value(
            prop,
            "CosmeticArmourOverrides must be ArrayProperty<NameProperty>",
        )?;
    } else {
        return Ok(false);
    }
    Ok(true)
}

/// A retained `C025` controller identity used to choose the returning-player
/// path (F-947). Identifiers never appear in Debug output.
#[derive(Clone, PartialEq, Eq)]
pub struct SavedPlayerController {
    pub player_id: Vec<u16>,
    pub chr_slot: i32,
}

impl std::fmt::Debug for SavedPlayerController {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SavedPlayerController")
    }
}

/// Every C025 controller retained by strict staging and Validate, in saved
/// order. Refused saves return no partial identities.
pub fn saved_player_controllers(
    roots: &[RootTag<'_>],
    profile: TagProfile,
    component_byte_limit: usize,
) -> Result<Vec<SavedPlayerController>> {
    let (staged, decisions) =
        stage_prospect_save_state_with_decisions(roots, profile, component_byte_limit, None)?;
    if staged.parsed_recorder_count == 0 {
        return Ok(Vec::new());
    }
    let recorders = read_recorders(roots, profile)?;
    ensure!(
        recorders.records().len() == decisions.len(),
        "staged decisions do not cover every recorder"
    );
    let mut controllers = Vec::new();
    for (recorder, decision) in recorders.records().iter().zip(&decisions) {
        if !decision.retained {
            continue;
        }
        let class =
            decode_exact_string_units(recorder.class_value, "invalid saved recorder class string")?;
        if !units_eq_ascii(&class, "/Script/Icarus.PlayerRecorderComponent") {
            continue;
        }
        let component =
            read_base_component_with_limit(recorder.binary_data, profile, component_byte_limit)?;
        let mut identity = SavedPlayerController {
            player_id: Vec::new(),
            chr_slot: -1,
        };
        for field in component.fields() {
            let ParsedRecord::Property(prop) = read_record(field.record, profile)? else {
                bail!("unexpected saved controller terminator");
            };
            if is_name(&prop.name, "PlayerCharacterID") {
                (identity.player_id, identity.chr_slot) =
                    parse_player_character_id_struct(prop.tag.payload, profile)?;
            }
        }
        controllers.push(identity);
    }
    Ok(controllers)
}

/// Every retained `C026` player character, in recorder-array order, after the
/// same strict staging and `Validate` (`v >= 1`, member list) as
/// [`stage_prospect_save_state`]. Staging errors, and a restored field
/// (including an inherited `SavedInventories` or `Modifiers` element) of the
/// wrong type or shape, reject the whole save.
pub fn saved_player_characters(
    roots: &[RootTag<'_>],
    profile: TagProfile,
    component_byte_limit: usize,
) -> Result<Vec<SavedPlayerCharacter>> {
    let (staged, decisions) =
        stage_prospect_save_state_with_decisions(roots, profile, component_byte_limit, None)?;
    if staged.parsed_recorder_count == 0 {
        return Ok(Vec::new());
    }
    let recorders = read_recorders(roots, profile)?;
    ensure!(
        recorders.records().len() == decisions.len(),
        "staged decisions do not cover every recorder"
    );
    let mut characters = Vec::new();
    for (recorder, decision) in recorders.records().iter().zip(&decisions) {
        if !decision.retained {
            continue;
        }
        let class_units =
            decode_exact_string_units(recorder.class_value, "invalid saved recorder class string")?;
        if !units_eq_ascii(&class_units, "/Script/Icarus.PlayerStateRecorderComponent") {
            continue;
        }
        let component =
            read_base_component_with_limit(recorder.binary_data, profile, component_byte_limit)
                .context("invalid saved player character component")?;
        let mut player_id = Vec::new();
        let mut chr_slot = -1_i32;
        let mut icarus_uid = -1_i32;
        let mut state = SavedPlayerState::default();
        for field in component.fields() {
            let ParsedRecord::Property(prop) = read_record(field.record, profile)? else {
                bail!("unexpected saved player character terminator");
            };
            if read_saved_player_state_field(&mut state, &prop, profile, component_byte_limit)? {
                continue;
            }
            if is_name(&prop.name, "PlayerCharacterID") {
                (player_id, chr_slot) =
                    parse_player_character_id_struct(prop.tag.payload, profile)?;
            } else if is_name(&prop.name, "IcarusActorGUID") {
                ensure!(
                    prop.tag.kind == TagKind::Int && prop.index == 0 && prop.tag.payload.len() == 4,
                    "IcarusActorGUID must be 4-byte IntProperty"
                );
                icarus_uid = i32::from_le_bytes(prop.tag.payload.try_into()?);
            }
        }
        let tree = BaseComponentTree::parse(
            recorder.binary_data,
            profile,
            component_byte_limit,
            TreeLimits {
                max_depth: MAX_TREE_DEPTH,
                max_nodes: MAX_TREE_NODES,
                struct_array_bytes: MAX_BOUNDED_STRUCT_ARRAY_BYTES,
                container_struct_bytes: None,
            },
        )
        .context("invalid saved player character tree")?;
        let (typed, _) = TypedComponent::from_component(&tree)?;
        let (inventories, modifiers) = saved_actor_state(&typed.tree)?;
        characters.push(SavedPlayerCharacter {
            player_id,
            chr_slot,
            icarus_uid,
            state,
            inventories,
            modifiers,
        });
    }
    Ok(characters)
}

/// The first saved character whose `PlayerCharacterID` matches, as the
/// original's restore scan takes the first unresolved accepting recorder:
/// equal `ChrSlot` and `PlayerID` under `Stricmp` (F-949). `skip` lists
/// characters already restored this session.
pub fn find_saved_player_character<'a>(
    characters: &'a [SavedPlayerCharacter],
    player_id: &[u16],
    chr_slot: i32,
    skip: &[usize],
) -> Option<(usize, &'a SavedPlayerCharacter)> {
    characters.iter().enumerate().find(|(index, character)| {
        !skip.contains(index)
            && character.chr_slot == chr_slot
            && names_match(&character.player_id, player_id)
    })
}

/// One recorder's staging outcome, in recorder-array order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StagedRecorderDecision {
    /// Kept after `Validate` (`+0x408`); always `false` for a version-0 save.
    pub retained: bool,
    pub owner_resolve_policy: OwnerResolvePolicy,
    pub skip_default_owner_resolution: bool,
    pub requires_stitching: bool,
    /// A `C033` `bIsPhysicsDynamic` tree the original drops on its next save.
    pub physics_dynamic_tree: bool,
    pub reset_rocket_spawn: bool,
}

/// [`stage_prospect_save_state`], also returning every recorder's decision.
pub fn stage_prospect_save_state_with_decisions(
    roots: &[RootTag<'_>],
    profile: TagProfile,
    component_byte_limit: usize,
    expected_prospect_id: Option<&[u16]>,
) -> Result<(StagedProspectSaveState, Vec<StagedRecorderDecision>)> {
    let staged = stage_prospect_save_state_for_import(
        roots,
        profile,
        component_byte_limit,
        expected_prospect_id,
    )?;
    Ok((staged.summary, staged.decisions))
}

/// Strict staging with owned root values and recorder decisions. This performs
/// the same admission and migrations as stage_prospect_save_state; the returned
/// values do not authorize restoration or native-world publication.
pub fn stage_prospect_save_state_with_root_values(
    roots: &[RootTag<'_>],
    profile: TagProfile,
    component_byte_limit: usize,
    expected_prospect_id: Option<&[u16]>,
) -> Result<(
    StagedProspectSaveState,
    StagedRootValues,
    Vec<StagedRecorderDecision>,
)> {
    let staged = stage_prospect_save_state_for_import(
        roots,
        profile,
        component_byte_limit,
        expected_prospect_id,
    )?;
    Ok((staged.summary, staged.root_values, staged.decisions))
}

/// Internal staging output with migrated content references. Deliberately has no
/// Debug or Serialize implementation: supplied values stay out of reports.
pub(crate) struct StagedImportState {
    pub(crate) summary: StagedProspectSaveState,
    pub(crate) decisions: Vec<StagedRecorderDecision>,
    pub(crate) root_values: StagedRootValues,
    pub(crate) prospect_dt_key: Vec<u16>,
    pub(crate) prospect_map_name: Vec<u16>,
    pub(crate) faction_mission_dt_key: Vec<u16>,
    pub(crate) setting_row_names: Vec<Vec<u16>>,
}

pub(crate) fn stage_prospect_save_state_for_import(
    roots: &[RootTag<'_>],
    profile: TagProfile,
    component_byte_limit: usize,
    expected_prospect_id: Option<&[u16]>,
) -> Result<StagedImportState> {
    ensure!(
        (1..=MAX_BOUNDED_COMPONENT_BYTES).contains(&component_byte_limit),
        "component byte limit must be between 1 byte and 1 MiB"
    );
    ensure!(
        roots.len() <= MAX_ROOT_TAGS,
        "prospect root count exceeds research limit"
    );
    let total = roots.iter().try_fold(0usize, |size, root| {
        size.checked_add(root.record.len())
            .context("prospect root input length overflow")
    })?;
    ensure!(
        total <= MAX_INPUT_BYTES,
        "prospect root input exceeds 32 MiB limit"
    );

    let mut seen_root_names: Vec<Vec<u16>> = Vec::with_capacity(roots.len());
    let mut loaded_version: Option<i32> = None;
    let mut lobby_privacy: Option<LobbyPrivacy> = None;
    let mut prospect_info: Option<ParsedProspectInfo> = None;
    let mut has_map_name = false;
    let mut prospect_map_name = Vec::new();
    let mut last_saved_date_time = None;
    let mut source_presence = StagedRootPresence::default();
    let mut legacy_prospect_id: Option<Vec<u16>> = None;
    let mut legacy_prospect_dt_key: Option<Vec<u16>> = None;
    let mut has_recorders_root = false;

    for root in roots {
        let ParsedRecord::Property(property) = read_record(root.record, profile)? else {
            bail!("unexpected prospect root terminator");
        };
        ensure!(
            property.tag.record.len() == root.record.len(),
            "trailing prospect root data"
        );
        ensure!(
            !seen_root_names.iter().any(|prior| prior == &property.name),
            "duplicate prospect root property"
        );
        seen_root_names.push(property.name.clone());

        if is_name(&property.name, "Version") {
            ensure!(
                property.tag.kind == TagKind::Int
                    && property.index == 0
                    && property.tag.payload.len() == 4,
                "invalid Version root header"
            );
            let version = i32::from_le_bytes(property.tag.payload.try_into()?);
            ensure!(
                (0..=PROSPECT_SAVE_STATE_CURRENT_VERSION).contains(&version),
                "unsupported FProspectSaveState Version"
            );
            loaded_version = Some(version);
        } else if is_name(&property.name, "LobbyPrivacy") {
            lobby_privacy = Some(parse_lobby_privacy(&property)?);
        } else if is_name(&property.name, "ProspectInfo") {
            ensure!(
                property.tag.kind == TagKind::Struct
                    && property.index == 0
                    && extra_is(&property, "ProspectInfo"),
                "invalid ProspectInfo root header"
            );
            source_presence.prospect_info = true;
            prospect_info = Some(validate_prospect_info(
                property.tag.payload,
                profile,
                component_byte_limit,
            )?);
        } else if is_name(&property.name, "ProspectMapName") {
            ensure!(
                property.tag.kind == TagKind::String && property.index == 0,
                "invalid ProspectMapName root header"
            );
            let units = decode_exact_string_units(property.tag.payload, "invalid ProspectMapName")?;
            source_presence.prospect_map_name = true;
            has_map_name = !units.is_empty();
            prospect_map_name = units;
        } else if is_name(&property.name, "LastSavedDateTime") {
            ensure!(
                property.tag.kind == TagKind::Struct
                    && property.index == 0
                    && extra_is(&property, "DateTime")
                    && property.tag.payload.len() == 8,
                "invalid LastSavedDateTime root header"
            );
            last_saved_date_time = Some(i64::from_le_bytes(property.tag.payload.try_into()?));
        } else if is_name(&property.name, "ProspectID") {
            ensure!(
                property.tag.kind == TagKind::String && property.index == 0,
                "invalid legacy ProspectID root header"
            );
            legacy_prospect_id = Some(decode_exact_string_units(
                property.tag.payload,
                "invalid legacy ProspectID",
            )?);
        } else if is_name(&property.name, "ProspectDTKey") {
            ensure!(
                property.tag.kind == TagKind::String && property.index == 0,
                "invalid legacy ProspectDTKey root header"
            );
            legacy_prospect_dt_key = Some(decode_exact_string_units(
                property.tag.payload,
                "invalid legacy ProspectDTKey",
            )?);
        } else if is_name(&property.name, "StateRecorderBlobs") {
            has_recorders_root = true;
        } else {
            bail!("unmapped FProspectSaveState root property");
        }
    }

    let version = loaded_version.unwrap_or(0);
    let mut privacy = lobby_privacy.unwrap_or(LobbyPrivacy::Unknown);
    source_presence.version = loaded_version.is_some();
    source_presence.lobby_privacy = lobby_privacy.is_some();
    source_presence.last_saved_date_time = last_saved_date_time.is_some();
    source_presence.legacy_prospect_id = legacy_prospect_id.is_some();
    source_presence.legacy_prospect_dt_key = legacy_prospect_dt_key.is_some();
    source_presence.state_recorder_blobs = has_recorders_root;
    let mut info = prospect_info.unwrap_or_default();

    if version < PROSPECT_SAVE_STATE_CURRENT_VERSION {
        if let Some(legacy_id) = &legacy_prospect_id {
            info.values.prospect_id.clone_from(legacy_id);
        }
        if let Some(legacy_dt_key) = &legacy_prospect_dt_key {
            info.values.prospect_dt_key.clone_from(legacy_dt_key);
        }
    }
    if version == 0 {
        info.tag_count = 0;
        info.values = ProspectInfo::default();
        info.setting_row_names.clear();
        info.has_expire_time = false;
        privacy = LobbyPrivacy::FriendsOnly;
    }

    let member_refs: Vec<ProspectAssociatedMember<'_>> = info
        .values
        .associated_members
        .iter()
        .map(|member| ProspectAssociatedMember {
            user_id: member.user_id.as_slice(),
            chr_slot: member.chr_slot,
        })
        .collect();

    let mut parsed_recorder_count = 0usize;
    let mut invalid_recorder_count = 0usize;
    let mut reset_rocket_spawn_recorder_count = 0usize;
    let mut physics_dynamic_tree_recorder_count = 0usize;
    let mut skip_default_owner_resolution_count = 0usize;
    let mut find_only_owner_count = 0usize;
    let mut respawn_only_owner_count = 0usize;
    let mut find_or_respawn_owner_count = 0usize;
    let mut manually_resolved_owner_count = 0usize;
    let mut requires_stitching_count = 0usize;
    let mut total_component_field_tags = 0usize;
    let mut decisions = Vec::new();
    if has_recorders_root {
        let recorders = read_recorders(roots, profile)?;
        parsed_recorder_count = recorders.records().len();
        decisions.reserve_exact(parsed_recorder_count);
        for recorder in recorders.records() {
            ensure!(
                recorder.fields().len() == 2,
                "unmapped StateRecorderBlob field"
            );
            let class_units = decode_exact_string_units(
                recorder.class_value,
                "invalid staged recorder class string",
            )?;
            let component =
                read_base_component_with_limit(recorder.binary_data, profile, component_byte_limit)
                    .context("invalid staged recorder base component")?;
            validate_staged_component_properties(&component, profile)?;
            total_component_field_tags = total_component_field_tags
                .checked_add(component.fields().len())
                .context("component field count overflow")?;
            if version == 0 {
                // Version 0 drops every recorder without evaluating its class,
                // so an unregistered class still stages; it takes the base
                // constructor defaults (FindOrRespawn, no skip).
                let defaults = lookup_recorder_ctor_defaults(&class_units);
                decisions.push(StagedRecorderDecision {
                    retained: false,
                    owner_resolve_policy: defaults.map_or(OwnerResolvePolicy::FindOrRespawn, |d| {
                        d.owner_resolve_policy
                    }),
                    skip_default_owner_resolution: defaults
                        .is_some_and(|d| d.b_skip_default_owner_resolution),
                    requires_stitching: false,
                    physics_dynamic_tree: false,
                    reset_rocket_spawn: false,
                });
            } else {
                let eval = evaluate_staged_recorder_component(
                    &class_units,
                    &component,
                    profile,
                    &info.values.prospect_id,
                    &member_refs,
                )?;
                if !eval.is_valid {
                    invalid_recorder_count += 1;
                } else {
                    if eval.skip_default_owner_resolution {
                        skip_default_owner_resolution_count += 1;
                    } else {
                        match eval.owner_resolve_policy {
                            OwnerResolvePolicy::FindOnly => find_only_owner_count += 1,
                            OwnerResolvePolicy::RespawnOnly => respawn_only_owner_count += 1,
                            OwnerResolvePolicy::FindOrRespawn => find_or_respawn_owner_count += 1,
                            OwnerResolvePolicy::ManuallyResolved => {
                                manually_resolved_owner_count += 1;
                            }
                        }
                    }
                    if eval.requires_stitching {
                        requires_stitching_count += 1;
                    }
                }
                if eval.was_reset_rocket_spawn {
                    reset_rocket_spawn_recorder_count += 1;
                }
                if eval.is_physics_dynamic_tree {
                    physics_dynamic_tree_recorder_count += 1;
                }
                decisions.push(StagedRecorderDecision {
                    retained: eval.is_valid,
                    owner_resolve_policy: eval.owner_resolve_policy,
                    skip_default_owner_resolution: eval.skip_default_owner_resolution,
                    requires_stitching: eval.requires_stitching,
                    physics_dynamic_tree: eval.is_physics_dynamic_tree,
                    reset_rocket_spawn: eval.was_reset_rocket_spawn,
                });
            }
        }
    }

    let retained_recorder_count = if version == 0 {
        0
    } else {
        parsed_recorder_count - invalid_recorder_count
    };

    if let Some(expected) = expected_prospect_id {
        ensure!(
            !info.values.prospect_id.is_empty()
                && prospect_ids_match(expected, &info.values.prospect_id),
            "staged ProspectID mismatch"
        );
    }

    let staged = StagedProspectSaveState {
        profile: match profile {
            TagProfile::MemoryArchive522 => "experimental_memory_archive_522",
        },
        loaded_version: version,
        migrated_version: PROSPECT_SAVE_STATE_CURRENT_VERSION,
        lobby_privacy: privacy,
        root_tag_count: roots.len(),
        prospect_info_tag_count: info.tag_count,
        associated_member_count: info.values.associated_members.len(),
        has_prospect_id: !info.values.prospect_id.is_empty(),
        has_prospect_dt_key: !info.values.prospect_dt_key.is_empty(),
        has_map_name,
        has_last_saved_date_time: last_saved_date_time.is_some(),
        has_expire_time: info.has_expire_time,
        parsed_recorder_count,
        invalid_recorder_count,
        retained_recorder_count,
        reset_rocket_spawn_recorder_count,
        physics_dynamic_tree_recorder_count,
        skip_default_owner_resolution_count,
        find_only_owner_count,
        respawn_only_owner_count,
        find_or_respawn_owner_count,
        manually_resolved_owner_count,
        requires_stitching_count,
        total_component_field_tags,
        world_imported: false,
    };
    let legacy_disposition = |present: bool| {
        if !present {
            StagedLegacyDisposition::Absent
        } else if version == 0 {
            StagedLegacyDisposition::AppliedThenReset
        } else if version < PROSPECT_SAVE_STATE_CURRENT_VERSION {
            StagedLegacyDisposition::AppliedToProspectInfo
        } else {
            StagedLegacyDisposition::IgnoredAtCurrentVersion
        }
    };
    let root_values = StagedRootValues {
        prospect_info: info.values,
        lobby_privacy: privacy,
        prospect_map_name,
        last_saved_date_time,
        loaded_version,
        source_presence,
        legacy_prospect_id,
        legacy_prospect_dt_key,
        migration: match version {
            0 => StagedRootMigration::Version0Reset,
            1 => StagedRootMigration::Version1LegacyOverlay,
            _ => StagedRootMigration::CurrentVersion,
        },
        legacy_prospect_id_disposition: legacy_disposition(source_presence.legacy_prospect_id),
        legacy_prospect_dt_key_disposition: legacy_disposition(
            source_presence.legacy_prospect_dt_key,
        ),
        source_enum_values: info.source_enum_values,
    };
    Ok(StagedImportState {
        summary: staged,
        decisions,
        prospect_dt_key: root_values.prospect_info.prospect_dt_key.clone(),
        prospect_map_name: root_values.prospect_map_name.clone(),
        faction_mission_dt_key: root_values.prospect_info.faction_mission_dt_key.clone(),
        setting_row_names: info.setting_row_names,
        root_values,
    })
}

#[cfg(test)]
mod owned_root_allocation_tests {
    use super::*;

    #[test]
    fn owned_bytes_counts_nested_spare_capacity_and_inline_storage() {
        fn units(capacity: usize) -> Vec<u16> {
            let mut result = Vec::with_capacity(capacity);
            result.push(0xd800);
            result
        }
        let mut members = Vec::with_capacity(2);
        members.push(AssociatedMember {
            account_name: units(17),
            character_name: units(19),
            user_id: units(23),
            ..Default::default()
        });
        let mut settings = Vec::with_capacity(3);
        settings.push(CustomGameSetting {
            setting_row_name: units(29),
            setting_value: 1,
        });
        let mut enums = Vec::with_capacity(5);
        enums.push(StagedRootEnum {
            field: StagedRootEnumField::ProspectState,
            source_kind: TagKind::Enum,
            source_value: StagedRootEnumValue::Name(units(43)),
            resolved_value: None,
        });
        enums.push(StagedRootEnum {
            field: StagedRootEnumField::Difficulty,
            source_kind: TagKind::Byte,
            source_value: StagedRootEnumValue::Byte(250),
            resolved_value: None,
        });
        let root = StagedRootValues {
            prospect_info: ProspectInfo {
                prospect_id: units(3),
                claimed_account_id: units(5),
                prospect_dt_key: units(7),
                faction_mission_dt_key: units(11),
                lobby_name: units(13),
                associated_members: members,
                custom_settings: settings,
                ..Default::default()
            },
            lobby_privacy: LobbyPrivacy::Private,
            prospect_map_name: units(31),
            last_saved_date_time: Some(i64::MAX),
            loaded_version: Some(2),
            source_presence: StagedRootPresence::default(),
            legacy_prospect_id: Some(units(37)),
            legacy_prospect_dt_key: Some(units(41)),
            migration: StagedRootMigration::CurrentVersion,
            legacy_prospect_id_disposition: StagedLegacyDisposition::IgnoredAtCurrentVersion,
            legacy_prospect_dt_key_disposition: StagedLegacyDisposition::IgnoredAtCurrentVersion,
            source_enum_values: enums,
        };
        // Thirteen UTF-16 allocations have one initialized unit each, but
        // jointly reserve 279 units. Array allocations also have spare slots.
        let expected = std::mem::size_of::<StagedRootValues>()
            + 279 * std::mem::size_of::<u16>()
            + 2 * std::mem::size_of::<AssociatedMember>()
            + 3 * std::mem::size_of::<CustomGameSetting>()
            + 5 * std::mem::size_of::<StagedRootEnum>();
        assert_eq!(root.owned_bytes().unwrap(), expected);
    }

    #[test]
    fn owned_bytes_includes_empty_root_inline_storage() {
        let staged =
            stage_prospect_save_state_for_import(&[], TagProfile::MemoryArchive522, 1, None)
                .unwrap();
        assert_eq!(
            staged.root_values.owned_bytes().unwrap(),
            std::mem::size_of::<StagedRootValues>()
        );
    }
}

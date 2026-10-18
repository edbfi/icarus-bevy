//! Strict experimental framing of the observed StateRecorderBlobs array and
//! staged root/recorder schema validation for `FProspectSaveState`.
//!
//! This accepts one shared StateRecorderBlob struct tag and exact tagged element
//! boundaries. Class strings and recorder bodies remain borrowed opaque bytes;
//! no class lookup, gameplay interpretation, object construction or import occurs.
use anyhow::{Context, Result, bail, ensure};
use serde::Serialize;

use super::archive::{ByteOrder, decode_string};
use super::component::{
    BaseComponent, MAX_BOUNDED_COMPONENT_BYTES, read_base_component_with_limit,
};
use super::restore_rules::{
    LobbyPrivacy, OwnerResolvePolicy, PROSPECT_SAVE_STATE_CURRENT_VERSION,
    ProspectAssociatedMember, player_record_valid, prospect_ids_match,
    resolve_prospect_player_list, resource_deposit_record_valid, rocket_record_valid,
    validate_rocket_spawn_record, voxel_record_valid,
};
use super::struct_array::read_tagged_struct_array_with_limit;
use super::tagged::{
    MAX_HEADER_STRING_UNITS, MAX_INPUT_BYTES, MAX_ROOT_TAGS, ParsedRecord, PropertyRecord, RootTag,
    TagKind, TagProfile, is_name, read_record, read_root,
};

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
    pub total_component_field_tags: usize,
    pub world_imported: bool,
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

fn bool_tag_value(tag: &RootTag<'_>) -> Result<bool> {
    ensure!(
        tag.kind == TagKind::Bool && tag.payload.is_empty() && tag.header.len() >= 2,
        "invalid BoolProperty tag"
    );
    let bool_idx = if tag.header[tag.header.len() - 1] == 0 {
        tag.header.len() - 2
    } else {
        ensure!(tag.header.len() >= 18, "truncated BoolProperty GUID header");
        tag.header.len() - 18
    };
    Ok(tag.header[bool_idx] != 0)
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

/// Validates `ProspectInfo.AssociatedMembers` against the reflected
/// `FAssociatedMemberInfo` descriptor (`0x145fad8e0` / `0x143dbc380`):
/// shared `StructProperty` header `"AssociatedMemberInfo"` and up to the 8
/// reflected fields (`AccountName`, `CharacterName`, `UserID`, `ChrSlot`,
/// `Experience`, `Status`, `Settled`, `IsCurrentlyPlaying`), returning the
/// post-default `(UserID, ChrSlot)` pairs (`FAssociatedMemberInfo::Construct`
/// `0x143dbc230` zero-initializes `UserID = ""` and `ChrSlot = 0`).
fn validate_associated_members(
    payload: &[u8],
    profile: TagProfile,
    component_byte_limit: usize,
) -> Result<Vec<(Vec<u16>, i32)>> {
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
    for element in members.elements() {
        let mut seen: Vec<Vec<u16>> = Vec::with_capacity(element.fields().len());
        let mut user_id = Vec::new();
        let mut chr_slot = 0_i32;
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
                user_id = decode_exact_string_units(
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
                decode_exact_string_units(
                    field_prop.tag.payload,
                    "invalid AssociatedMemberInfo string",
                )?;
            } else if is_name(&field_prop.name, "ChrSlot") {
                ensure!(
                    field_prop.tag.kind == TagKind::Int && field_prop.tag.payload.len() == 4,
                    "AssociatedMemberInfo.ChrSlot must be 4-byte IntProperty"
                );
                chr_slot = i32::from_le_bytes(field_prop.tag.payload.try_into()?);
            } else if is_name(&field_prop.name, "Experience") {
                ensure!(
                    field_prop.tag.kind == TagKind::Int && field_prop.tag.payload.len() == 4,
                    "AssociatedMemberInfo int field must be 4-byte IntProperty"
                );
            } else if is_name(&field_prop.name, "Status") {
                match field_prop.tag.kind {
                    TagKind::Enum => {
                        ensure!(
                            extra_is(&field_prop, "EProspectLocation"),
                            "AssociatedMemberInfo.Status must use EProspectLocation"
                        );
                        decode_exact_string_units(
                            field_prop.tag.payload,
                            "invalid AssociatedMemberInfo.Status enum string",
                        )?;
                    }
                    TagKind::Byte => {
                        if field_prop.tag.payload.len() != 1 {
                            decode_exact_string_units(
                                field_prop.tag.payload,
                                "invalid AssociatedMemberInfo.Status byte enum",
                            )?;
                        }
                    }
                    _ => bail!("AssociatedMemberInfo.Status must be EnumProperty or ByteProperty"),
                }
            } else if is_name(&field_prop.name, "Settled")
                || is_name(&field_prop.name, "IsCurrentlyPlaying")
            {
                ensure!(
                    field_prop.tag.kind == TagKind::Bool && field_prop.tag.payload.is_empty(),
                    "AssociatedMemberInfo bool field must be BoolProperty"
                );
            } else {
                bail!("unmapped AssociatedMemberInfo property");
            }
        }
        parsed_members.push((user_id, chr_slot));
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
) -> Result<()> {
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
                decode_exact_string_units(
                    field_prop.tag.payload,
                    "invalid CustomGameSetting.SettingRowName",
                )?;
            } else if is_name(&field_prop.name, "SettingValue") {
                ensure!(
                    field_prop.tag.kind == TagKind::Int && field_prop.tag.payload.len() == 4,
                    "CustomGameSetting.SettingValue must be 4-byte IntProperty"
                );
            } else {
                bail!("unmapped CustomGameSetting property");
            }
        }
    }
    Ok(())
}

struct ParsedProspectInfo {
    tag_count: usize,
    associated_members: Vec<(Vec<u16>, i32)>,
    prospect_id: Vec<u16>,
    prospect_dt_key: Vec<u16>,
    has_expire_time: bool,
}

fn validate_prospect_info(
    payload: &[u8],
    profile: TagProfile,
    component_byte_limit: usize,
) -> Result<ParsedProspectInfo> {
    let nested = read_root(payload, profile).context("invalid ProspectInfo tagged sequence")?;
    let mut seen_names: Vec<Vec<u16>> = Vec::with_capacity(nested.tags.len());
    let mut associated_members = Vec::new();
    let mut prospect_id = Vec::new();
    let mut prospect_dt_key = Vec::new();
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
            prospect_id = decode_exact_string_units(prop.tag.payload, "invalid ProspectID")?;
        } else if is_name(&prop.name, "ProspectDTKey") {
            ensure!(
                prop.tag.kind == TagKind::String,
                "ProspectInfo.ProspectDTKey must be StrProperty"
            );
            prospect_dt_key = decode_exact_string_units(prop.tag.payload, "invalid ProspectDTKey")?;
        } else if is_name(&prop.name, "ClaimedAccountID")
            || is_name(&prop.name, "FactionMissionDTKey")
            || is_name(&prop.name, "LobbyName")
        {
            ensure!(
                prop.tag.kind == TagKind::String,
                "ProspectInfo string field must be StrProperty"
            );
            decode_exact_string_units(prop.tag.payload, "invalid ProspectInfo string")?;
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
        } else if is_name(&prop.name, "ExpireTime") {
            ensure!(
                prop.tag.kind == TagKind::Int64 && prop.tag.payload.len() == 8,
                "ProspectInfo.ExpireTime must be 8-byte Int64Property"
            );
            has_expire_time = true;
        } else if is_name(&prop.name, "ProspectState") {
            match prop.tag.kind {
                TagKind::Enum => {
                    ensure!(
                        extra_is(&prop, "EProspectState"),
                        "ProspectInfo.ProspectState must use EProspectState"
                    );
                    decode_exact_string_units(prop.tag.payload, "invalid ProspectState enum")?;
                }
                TagKind::Byte => {
                    if prop.tag.payload.len() != 1 {
                        decode_exact_string_units(
                            prop.tag.payload,
                            "invalid ProspectState byte enum",
                        )?;
                    }
                }
                _ => bail!("ProspectInfo.ProspectState must be EnumProperty or ByteProperty"),
            }
        } else if is_name(&prop.name, "Difficulty") {
            match prop.tag.kind {
                TagKind::Enum => {
                    ensure!(
                        extra_is(&prop, "EMissionDifficulty"),
                        "ProspectInfo.Difficulty must use EMissionDifficulty"
                    );
                    decode_exact_string_units(prop.tag.payload, "invalid Difficulty enum")?;
                }
                TagKind::Byte => {
                    if prop.tag.payload.len() != 1 {
                        decode_exact_string_units(
                            prop.tag.payload,
                            "invalid Difficulty byte enum",
                        )?;
                    }
                }
                _ => bail!("ProspectInfo.Difficulty must be EnumProperty or ByteProperty"),
            }
        } else if is_name(&prop.name, "Insurance") || is_name(&prop.name, "NoRespawns") {
            ensure!(
                prop.tag.kind == TagKind::Bool && prop.tag.payload.is_empty(),
                "ProspectInfo bool field must be BoolProperty"
            );
        } else if is_name(&prop.name, "AssociatedMembers") {
            ensure!(
                prop.tag.kind == TagKind::Array && extra_is(&prop, "StructProperty"),
                "ProspectInfo.AssociatedMembers must be ArrayProperty<StructProperty>"
            );
            associated_members =
                validate_associated_members(prop.tag.payload, profile, component_byte_limit)?;
        } else if is_name(&prop.name, "CustomSettings") {
            ensure!(
                prop.tag.kind == TagKind::Array && extra_is(&prop, "StructProperty"),
                "ProspectInfo.CustomSettings must be ArrayProperty<StructProperty>"
            );
            validate_custom_settings(prop.tag.payload, profile, component_byte_limit)?;
        } else {
            bail!("unmapped ProspectInfo property");
        }
    }

    Ok(ParsedProspectInfo {
        tag_count: nested.tags.len(),
        associated_members,
        prospect_id,
        prospect_dt_key,
        has_expire_time,
    })
}

struct StagedRecorderEval {
    is_valid: bool,
    was_reset_rocket_spawn: bool,
    is_physics_dynamic_tree: bool,
}

fn parse_player_character_id_struct(
    payload: &[u8],
    profile: TagProfile,
) -> Result<(Vec<u16>, i32)> {
    let nested = read_root(payload, profile).context("invalid PlayerCharacterID struct")?;
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
    if units_eq_ascii(
        class_units,
        "/Script/Icarus.DynamicRocketSpawnRecorderComponent",
    ) || units_eq_ascii(class_units, "/Script/Icarus.RocketSpawnRecorderComponent")
    {
        let mut assigned_player_id = Vec::new();
        let mut chr_slot = 0_i32;
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
                for inner_tag in &nested.tags {
                    let ParsedRecord::Property(inner_prop) =
                        read_record(inner_tag.record, profile)?
                    else {
                        bail!("unexpected RocketSpawnStateRecord terminator");
                    };
                    if is_name(&inner_prop.name, "AssignedPlayerID") {
                        ensure!(
                            inner_prop.tag.kind == TagKind::String && inner_prop.index == 0,
                            "RocketSpawnStateRecord.AssignedPlayerID must be StrProperty"
                        );
                        assigned_player_id = decode_exact_string_units(
                            inner_prop.tag.payload,
                            "invalid RocketSpawnStateRecord.AssignedPlayerID",
                        )?;
                    } else if is_name(&inner_prop.name, "ChrSlot") {
                        ensure!(
                            inner_prop.tag.kind == TagKind::Int
                                && inner_prop.index == 0
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
        let orig_player_id = assigned_player_id.clone();
        let orig_chr_slot = chr_slot;
        let is_valid =
            validate_rocket_spawn_record(version, list, &mut assigned_player_id, &mut chr_slot);
        let was_reset = assigned_player_id.is_empty()
            && chr_slot == -1
            && (version < 1 || (!orig_player_id.is_empty() || orig_chr_slot != -1));
        return Ok(StagedRecorderEval {
            is_valid,
            was_reset_rocket_spawn: was_reset,
            is_physics_dynamic_tree: false,
        });
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
        return Ok(StagedRecorderEval {
            is_valid: player_record_valid(version, list),
            was_reset_rocket_spawn: false,
            is_physics_dynamic_tree: false,
        });
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
        return Ok(StagedRecorderEval {
            is_valid: player_record_valid(version, list),
            was_reset_rocket_spawn: false,
            is_physics_dynamic_tree: false,
        });
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
        return Ok(StagedRecorderEval {
            is_valid: resource_deposit_record_valid(&resource_dt_key),
            was_reset_rocket_spawn: false,
            is_physics_dynamic_tree: false,
        });
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
        return Ok(StagedRecorderEval {
            is_valid: rocket_record_valid(list),
            was_reset_rocket_spawn: false,
            is_physics_dynamic_tree: false,
        });
    }

    let is_spawned_voxel =
        units_eq_ascii(class_units, "/Script/Icarus.SpawnedVoxelRecorderComponent");
    if is_spawned_voxel || units_eq_ascii(class_units, "/Script/Icarus.VoxelRecorderComponent") {
        // USpawnedVoxelRecorderComponent ctor (0x1418c24c0) defaults OwnerResolvePolicy (+0xb2) to
        // RespawnOnly (1); UVoxelRecorderComponent ctor (0x1418c2650) defaults to ManuallyResolved (3).
        let mut owner_resolve_policy = if is_spawned_voxel {
            OwnerResolvePolicy::RespawnOnly
        } else {
            OwnerResolvePolicy::ManuallyResolved
        };
        let mut fully_mined = false;
        for field_tag in component.fields() {
            let ParsedRecord::Property(prop) = read_record(field_tag.record, profile)? else {
                bail!("unexpected VoxelRecorderComponent terminator");
            };
            if is_name(&prop.name, "OwnerResolvePolicy") {
                owner_resolve_policy = parse_owner_resolve_policy(&prop)?;
            } else if is_name(&prop.name, "VoxelSaveData") {
                ensure!(
                    prop.tag.kind == TagKind::Struct
                        && prop.index == 0
                        && extra_is(&prop, "VoxelSaveData"),
                    "VoxelSaveData must be StructProperty(VoxelSaveData)"
                );
                let nested =
                    read_root(prop.tag.payload, profile).context("invalid VoxelSaveData struct")?;
                for inner_tag in &nested.tags {
                    let ParsedRecord::Property(inner_prop) =
                        read_record(inner_tag.record, profile)?
                    else {
                        bail!("unexpected VoxelSaveData terminator");
                    };
                    if is_name(&inner_prop.name, "bIsVoxelFullyMined") {
                        ensure!(
                            inner_prop.index == 0,
                            "VoxelSaveData.bIsVoxelFullyMined must have index 0"
                        );
                        fully_mined = bool_tag_value(&inner_prop.tag)?;
                    }
                }
            }
        }
        return Ok(StagedRecorderEval {
            is_valid: voxel_record_valid(owner_resolve_policy, fully_mined),
            was_reset_rocket_spawn: false,
            is_physics_dynamic_tree: false,
        });
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
                is_physics_dynamic = bool_tag_value(&prop.tag)?;
            }
        }
        return Ok(StagedRecorderEval {
            is_valid: true,
            was_reset_rocket_spawn: false,
            is_physics_dynamic_tree: is_physics_dynamic,
        });
    }

    Ok(StagedRecorderEval {
        is_valid: true,
        was_reset_rocket_spawn: false,
        is_physics_dynamic_tree: false,
    })
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
/// - Frames every recorder's `BinaryData` via [`read_base_component_with_limit`]
///   and rejects any recorder with unknown outer fields (`F-403`).
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
    let mut has_last_saved_date_time = false;
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
            has_map_name = !units.is_empty();
        } else if is_name(&property.name, "LastSavedDateTime") {
            ensure!(
                property.tag.kind == TagKind::Struct
                    && property.index == 0
                    && extra_is(&property, "DateTime")
                    && property.tag.payload.len() == 8,
                "invalid LastSavedDateTime root header"
            );
            has_last_saved_date_time = true;
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
    let mut info = prospect_info.unwrap_or(ParsedProspectInfo {
        tag_count: 0,
        associated_members: Vec::new(),
        prospect_id: Vec::new(),
        prospect_dt_key: Vec::new(),
        has_expire_time: false,
    });

    if version < PROSPECT_SAVE_STATE_CURRENT_VERSION {
        if let Some(legacy_id) = legacy_prospect_id {
            info.prospect_id = legacy_id;
        }
        if let Some(legacy_dt_key) = legacy_prospect_dt_key {
            info.prospect_dt_key = legacy_dt_key;
        }
    }
    if version == 0 {
        info.tag_count = 0;
        info.associated_members.clear();
        info.prospect_id.clear();
        info.prospect_dt_key.clear();
        info.has_expire_time = false;
        privacy = LobbyPrivacy::FriendsOnly;
    }

    let member_refs: Vec<ProspectAssociatedMember<'_>> = info
        .associated_members
        .iter()
        .map(|(user_id, chr_slot)| ProspectAssociatedMember {
            user_id: user_id.as_slice(),
            chr_slot: *chr_slot,
        })
        .collect();

    let mut parsed_recorder_count = 0usize;
    let mut invalid_recorder_count = 0usize;
    let mut reset_rocket_spawn_recorder_count = 0usize;
    let mut physics_dynamic_tree_recorder_count = 0usize;
    let mut total_component_field_tags = 0usize;
    if has_recorders_root {
        let recorders = read_recorders(roots, profile)?;
        parsed_recorder_count = recorders.records().len();
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
            total_component_field_tags = total_component_field_tags
                .checked_add(component.fields().len())
                .context("component field count overflow")?;
            if version != 0 {
                let eval = evaluate_staged_recorder_component(
                    &class_units,
                    &component,
                    profile,
                    &info.prospect_id,
                    &member_refs,
                )?;
                if !eval.is_valid {
                    invalid_recorder_count += 1;
                }
                if eval.was_reset_rocket_spawn {
                    reset_rocket_spawn_recorder_count += 1;
                }
                if eval.is_physics_dynamic_tree {
                    physics_dynamic_tree_recorder_count += 1;
                }
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
            !info.prospect_id.is_empty() && prospect_ids_match(expected, &info.prospect_id),
            "staged ProspectID mismatch"
        );
    }

    Ok(StagedProspectSaveState {
        profile: match profile {
            TagProfile::MemoryArchive522 => "experimental_memory_archive_522",
        },
        loaded_version: version,
        migrated_version: PROSPECT_SAVE_STATE_CURRENT_VERSION,
        lobby_privacy: privacy,
        root_tag_count: roots.len(),
        prospect_info_tag_count: info.tag_count,
        associated_member_count: info.associated_members.len(),
        has_prospect_id: !info.prospect_id.is_empty(),
        has_prospect_dt_key: !info.prospect_dt_key.is_empty(),
        has_map_name,
        has_last_saved_date_time,
        has_expire_time: info.has_expire_time,
        parsed_recorder_count,
        invalid_recorder_count,
        retained_recorder_count,
        reset_rocket_spawn_recorder_count,
        physics_dynamic_tree_recorder_count,
        total_component_field_tags,
        world_imported: false,
    })
}

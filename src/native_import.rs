//! Strict, independently authored C001 inert-placement fixtures. This adapter
//! accepts no gameplay state and establishes no official-save compatibility.
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    fs::OpenOptions,
    io::{Read, Write},
    os::unix::fs::OpenOptionsExt,
    path::Path,
    sync::Arc,
};

use anyhow::{Result, ensure};
use serde::{Deserialize, Deserializer, Serialize};

use crate::{
    experimental::{
        archive::{ByteOrder, decode_string, encode_string},
        component::{MAX_BOUNDED_COMPONENT_BYTES, read_base_component_with_limit},
        recorders::{
            StagedRootMigration, StagedRootPresence, StagedRootValues, read_recorders,
            stage_prospect_save_state_with_root_values,
        },
        restore_rules::{LobbyPrivacy, OwnerResolvePolicy},
        tagged::{ParsedRecord, PropertyRecord, TagKind, TagProfile, read_record, read_root},
    },
    import_candidate::DecodedImportCandidate,
    native_world::{
        CompiledNativeCatalog, MAX_NATIVE_KEY_BYTES, MAX_NATIVE_PLACEMENTS,
        MAX_NATIVE_STRING_UNITS, NativeActorRecord, NativeClassDefinition, NativeClassKind,
        NativePlacement, NativeTileDefinition, NativeTransform, NativeWorldSnapshot,
        prepare_authored_world,
    },
    persistence::{SaveReceipt, SnapshotStore},
    preservation::PreservedSection,
    simulation::{DomainId, SNAPSHOT_SCHEMA_C001_INERT, WorldSnapshot},
};

pub const C001_INERT_METADATA_FORMAT: u32 = 1;
pub const MAX_C001_BINDINGS_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_C001_METADATA_BYTES: usize = 4 * 1024 * 1024;
const PROFILE: TagProfile = TagProfile::MemoryArchive522;
const C001: &str = "/Script/Icarus.ActorStateRecorderComponent";

macro_rules! object_dto {
    (pub struct $name:ident { $(pub $field:ident: $ty:ty),* $(,)? }) => {
        #[derive(Clone, PartialEq, Serialize)]
        pub struct $name { $(pub $field: $ty),* }
        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
                #[derive(Deserialize)]
                #[serde(deny_unknown_fields)]
                struct Fields { $($field: $ty),* }
                let fields: Fields = crate::json::object(deserializer)
                    .map_err(|_| <D::Error as serde::de::Error>::custom("C001 object invalid"))?;
                Ok(Self { $($field: fields.$field),* })
            }
        }
    };
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum C001InertAdapter {
    #[serde(rename = "c001_inert_placement_v1")]
    C001InertPlacementV1,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum AuthoredCoordinatesV1 {
    #[serde(rename = "right_handed_z_up_meters_quat_xyzw_world_space")]
    RightHandedZUpMetersQuatXyzwWorldSpace,
}

object_dto! {
    pub struct C001InertBindingsV1 {
        pub format: u32,
        pub expected_payload_sha256: String,
        pub native_catalog_id: String,
        pub source_map: Vec<u16>,
        pub native_map: String,
        pub coordinates: AuthoredCoordinatesV1,
        pub owners: Vec<C001PlacementBindingV1>,
    }
}
object_dto! {
    pub struct C001PlacementBindingV1 {
        pub source_actor_class: Vec<u16>,
        pub source_actor_path: Vec<u16>,
        pub source_object_name: Vec<u16>,
        pub source_icarus_guid: i32,
        pub native_placement: String,
        pub native_class: String,
    }
}
object_dto! {
    pub struct C001RootPresence {
        pub version: bool,
        pub lobby_privacy: bool,
        pub prospect_info: bool,
        pub prospect_map_name: bool,
        pub last_saved_date_time: bool,
        pub legacy_prospect_id: bool,
        pub legacy_prospect_dt_key: bool,
        pub state_recorder_blobs: bool,
    }
}
object_dto! {
    pub struct ResolvedC001Placement {
        pub source_index: usize,
        pub id: DomainId,
        pub source_actor_class: Vec<u16>,
        pub source_actor_path: Vec<u16>,
        pub source_object_name: Vec<u16>,
        pub source_icarus_guid: i32,
        pub native_placement: String,
        pub native_class: String,
    }
}
object_dto! {
    pub struct C001InertImportMetadataV1 {
        pub format: u32,
        pub adapter: C001InertAdapter,
        pub source_version: i32,
        pub source_payload_sha256: String,
        pub native_catalog_id: String,
        pub native_map: String,
        pub coordinates: AuthoredCoordinatesV1,
        pub source_map: Vec<u16>,
        pub source_prospect_id: Vec<u16>,
        pub source_lobby_name: Vec<u16>,
        pub source_last_saved_date_time: Option<i64>,
        pub root_presence: C001RootPresence,
        pub owners: Vec<ResolvedC001Placement>,
    }
}

macro_rules! counts_debug {
    ($name:ident, $($field:ident),* $(,)?) => {
        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                let mut debug = f.debug_struct(stringify!($name));
                $(debug.field(stringify!($field), &self.$field.len());)*
                debug.finish_non_exhaustive()
            }
        }
    };
}
counts_debug!(C001InertBindingsV1, owners);
counts_debug!(
    C001PlacementBindingV1,
    source_actor_class,
    source_actor_path,
    source_object_name
);
counts_debug!(
    ResolvedC001Placement,
    source_actor_class,
    source_actor_path,
    source_object_name
);
counts_debug!(C001InertImportMetadataV1, owners);
impl fmt::Debug for C001RootPresence {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("C001RootPresence").finish_non_exhaustive()
    }
}

#[derive(Clone)]
pub struct CompiledC001InertBindings {
    definition: C001InertBindingsV1,
}
impl fmt::Debug for CompiledC001InertBindings {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CompiledC001InertBindings")
            .field("owners", &self.definition.owners.len())
            .finish_non_exhaustive()
    }
}

fn digest_valid(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn key_valid(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_NATIVE_KEY_BYTES
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
}
fn units_valid(value: &[u16], nonempty: bool) -> bool {
    (!nonempty || !value.is_empty()) && value.len() <= MAX_NATIVE_STRING_UNITS
}
fn exact(units: &[u16], expected: &str) -> bool {
    units.iter().copied().eq(expected.encode_utf16())
}

/// Count serialized bytes without allocating an unbounded serialization buffer.
fn serialized_bound(value: &impl Serialize, limit: usize) -> Result<usize> {
    struct Counter {
        bytes: usize,
        limit: usize,
    }
    impl Write for Counter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.bytes = self
                .bytes
                .checked_add(bytes.len())
                .filter(|n| *n <= self.limit)
                .ok_or_else(|| std::io::Error::other("C001 serialized byte limit"))?;
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut writer = Counter { bytes: 0, limit };
    serde_json::to_writer(&mut writer, value)
        .map_err(|_| anyhow::anyhow!("C001 serialization or byte limit"))?;
    Ok(writer.bytes)
}

impl CompiledC001InertBindings {
    pub fn compile(definition: C001InertBindingsV1) -> Result<Self> {
        ensure!(
            definition.format == C001_INERT_METADATA_FORMAT,
            "C001 binding format unsupported"
        );
        ensure!(
            digest_valid(&definition.expected_payload_sha256)
                && digest_valid(&definition.native_catalog_id),
            "C001 binding digest invalid"
        );
        ensure!(
            units_valid(&definition.source_map, true),
            "C001 binding source map invalid"
        );
        ensure!(
            key_valid(&definition.native_map),
            "C001 binding map invalid"
        );
        ensure!(
            !definition.owners.is_empty() && definition.owners.len() <= MAX_NATIVE_PLACEMENTS,
            "C001 binding owner count invalid"
        );
        let (mut paths, mut guids, mut placements) =
            (BTreeSet::new(), BTreeSet::new(), BTreeSet::new());
        for owner in &definition.owners {
            ensure!(
                units_valid(&owner.source_actor_class, true)
                    && units_valid(&owner.source_actor_path, true)
                    && units_valid(&owner.source_object_name, true),
                "C001 binding source identity invalid"
            );
            ensure!(
                key_valid(&owner.native_class) && key_valid(&owner.native_placement),
                "C001 binding native identity invalid"
            );
            ensure!(
                owner.source_icarus_guid > 0
                    && guids.insert(owner.source_icarus_guid)
                    && paths.insert(&owner.source_actor_path)
                    && placements.insert(&owner.native_placement),
                "C001 binding identity duplicate or invalid"
            );
        }
        serialized_bound(&definition, MAX_C001_BINDINGS_BYTES)?;
        Ok(Self { definition })
    }
    pub fn definition(&self) -> &C001InertBindingsV1 {
        &self.definition
    }
    pub fn parse(text: &str) -> Result<Self> {
        ensure!(
            text.len() <= MAX_C001_BINDINGS_BYTES,
            "C001 binding byte limit"
        );
        let definition = if matches!(text.trim_start().as_bytes().first(), Some(b'{' | b'[')) {
            serde_json::from_str(text).map_err(|_| anyhow::anyhow!("C001 binding JSON invalid"))?
        } else {
            toml::from_str(text).map_err(|_| anyhow::anyhow!("C001 binding TOML invalid"))?
        };
        Self::compile(definition)
    }
    pub fn load(path: &Path) -> Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(crate::nonblocking_flag())
            .open(path)
            .map_err(|e| anyhow::anyhow!("C001 binding open: {:?}", e.kind()))?;
        let metadata = file
            .metadata()
            .map_err(|e| anyhow::anyhow!("C001 binding metadata: {:?}", e.kind()))?;
        ensure!(metadata.is_file(), "C001 binding requires regular file");
        ensure!(
            metadata.len() <= MAX_C001_BINDINGS_BYTES as u64,
            "C001 binding byte limit"
        );
        let mut bytes = Vec::new();
        file.take(MAX_C001_BINDINGS_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| anyhow::anyhow!("C001 binding read: {:?}", e.kind()))?;
        ensure!(
            bytes.len() <= MAX_C001_BINDINGS_BYTES,
            "C001 binding byte limit"
        );
        Self::parse(
            std::str::from_utf8(&bytes)
                .map_err(|_| anyhow::anyhow!("C001 binding encoding invalid"))?,
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SemanticImportCategory {
    Missing,
    Unsupported,
    Header,
    Type,
    Value,
    Binding,
    Limit,
    Validation,
}

/// Only source ordinal, a static schema field and a fixed category cross the
/// diagnostic boundary. No underlying parser or supplied value is retained.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SemanticImportError {
    pub recorder_index: Option<usize>,
    pub field: &'static str,
    pub category: SemanticImportCategory,
}
impl fmt::Display for SemanticImportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "C001 inert import {:?}: field {}, recorder {:?}",
            self.category, self.field, self.recorder_index
        )
    }
}
impl std::error::Error for SemanticImportError {}
type SemanticResult<T> = std::result::Result<T, SemanticImportError>;
fn error(
    index: Option<usize>,
    field: &'static str,
    category: SemanticImportCategory,
) -> SemanticImportError {
    SemanticImportError {
        recorder_index: index,
        field,
        category,
    }
}
fn require(
    ok: bool,
    index: Option<usize>,
    field: &'static str,
    category: SemanticImportCategory,
) -> SemanticResult<()> {
    if ok {
        Ok(())
    } else {
        Err(error(index, field, category))
    }
}

impl From<StagedRootPresence> for C001RootPresence {
    fn from(p: StagedRootPresence) -> Self {
        Self {
            version: p.version,
            lobby_privacy: p.lobby_privacy,
            prospect_info: p.prospect_info,
            prospect_map_name: p.prospect_map_name,
            last_saved_date_time: p.last_saved_date_time,
            legacy_prospect_id: p.legacy_prospect_id,
            legacy_prospect_dt_key: p.legacy_prospect_dt_key,
            state_recorder_blobs: p.state_recorder_blobs,
        }
    }
}

fn validate_root(root: &StagedRootValues) -> SemanticResult<()> {
    use SemanticImportCategory::{Missing, Unsupported, Value};
    let p = root.source_presence();
    require(
        p.version
            && root.loaded_version() == Some(2)
            && root.migration() == StagedRootMigration::CurrentVersion,
        None,
        "Version",
        Unsupported,
    )?;
    require(
        !p.legacy_prospect_id && !p.legacy_prospect_dt_key,
        None,
        "legacy_root",
        Unsupported,
    )?;
    require(
        p.prospect_info && p.prospect_map_name && p.state_recorder_blobs,
        None,
        "root_presence",
        Missing,
    )?;
    require(
        root.lobby_privacy() == LobbyPrivacy::Unknown,
        None,
        "LobbyPrivacy",
        Unsupported,
    )?;
    require(
        units_valid(root.prospect_map_name(), true),
        None,
        "ProspectMapName",
        Value,
    )?;
    require(
        root.resolved_prospect_info().is_some(),
        None,
        "ProspectInfo.enum",
        Unsupported,
    )?;
    let info = root.prospect_info();
    require(
        units_valid(&info.prospect_id, true) && units_valid(&info.lobby_name, false),
        None,
        "ProspectInfo.identity",
        Value,
    )?;
    // The two descriptive strings are retained; every other property must be neutral.
    require(
        info.claimed_account_id.is_empty()
            && info.claimed_account_character == 0
            && info.prospect_dt_key.is_empty()
            && info.faction_mission_dt_key.is_empty()
            && info.expire_time == 0
            && info.prospect_state == 0
            && info.associated_members.is_empty()
            && info.cost == 0
            && info.reward == 0
            && info.difficulty == 0
            && !info.insurance
            && !info.no_respawns
            && info.elapsed_time == 0
            && info.selected_drop_point == 0
            && info.custom_settings.is_empty(),
        None,
        "ProspectInfo.state",
        Unsupported,
    )
}

/// Check the original header, including index and both kinds of GUID extension.
/// Values and encoded names are never formatted into an error.
fn header_valid(header: &[u8], kind: TagKind) -> bool {
    fn string_end(bytes: &[u8], offset: &mut usize) -> Option<()> {
        let value = decode_string(bytes.get(*offset..)?, ByteOrder::Little).ok()?;
        *offset = offset.checked_add(value.consumed)?;
        Some(())
    }
    fn check(header: &[u8], kind: TagKind) -> Option<bool> {
        let mut offset = 0;
        string_end(header, &mut offset)?;
        string_end(header, &mut offset)?;
        let ints = header.get(offset..offset.checked_add(8)?)?;
        if ints[4..] != [0; 4] {
            return Some(false);
        }
        offset += 8;
        match kind {
            TagKind::Struct => {
                string_end(header, &mut offset)?;
                if header.get(offset..offset.checked_add(16)?)? != [0; 16] {
                    return Some(false);
                }
                offset += 16;
            }
            TagKind::Array | TagKind::Byte | TagKind::Enum | TagKind::Set => {
                string_end(header, &mut offset)?
            }
            TagKind::Map => {
                string_end(header, &mut offset)?;
                string_end(header, &mut offset)?;
            }
            TagKind::Bool => {
                header.get(offset)?;
                offset += 1;
            }
            _ => {}
        }
        Some(header.get(offset..) == Some(&[0][..]))
    }
    check(header, kind) == Some(true)
}

fn property<'a>(
    bytes: &'a [u8],
    index: Option<usize>,
    field: &'static str,
) -> SemanticResult<PropertyRecord<'a>> {
    let ParsedRecord::Property(p) = read_record(bytes, PROFILE)
        .map_err(|_| error(index, field, SemanticImportCategory::Header))?
    else {
        return Err(error(index, field, SemanticImportCategory::Header));
    };
    require(
        header_valid(p.tag.header, p.tag.kind),
        index,
        field,
        SemanticImportCategory::Header,
    )?;
    Ok(p)
}

/// Reject ignored wrapper fields and header extensions at every admitted root
/// boundary. Staging already supplies exact schema/type/duplicate validation.
fn validate_source_headers(payload: &[u8]) -> SemanticResult<()> {
    let roots = read_root(payload, PROFILE)
        .map_err(|_| error(None, "root", SemanticImportCategory::Header))?;
    for tag in roots.tags {
        let p = property(tag.record, None, "root.header")?;
        if exact(&p.name, "ProspectInfo") {
            let info = read_root(p.tag.payload, PROFILE)
                .map_err(|_| error(None, "ProspectInfo", SemanticImportCategory::Header))?;
            for child in info.tags {
                let p = property(child.record, None, "ProspectInfo.header")?;
                if p.tag.kind == TagKind::Array {
                    let shared = p.tag.payload.get(4..).ok_or_else(|| {
                        error(None, "ProspectInfo.array", SemanticImportCategory::Header)
                    })?;
                    property(shared, None, "ProspectInfo.array.header")?;
                }
            }
        } else if exact(&p.name, "StateRecorderBlobs") {
            let bytes =
                p.tag.payload.get(4..).ok_or_else(|| {
                    error(None, "StateRecorderBlobs", SemanticImportCategory::Header)
                })?;
            property(bytes, None, "StateRecorderBlobs.header")?;
        }
    }
    Ok(())
}

fn validate_wrapper(bytes: &[u8], index: usize) -> SemanticResult<()> {
    let roots = read_root(bytes, PROFILE)
        .map_err(|_| error(Some(index), "recorder", SemanticImportCategory::Header))?;
    require(
        roots.tags.len() == 2,
        Some(index),
        "recorder.fields",
        SemanticImportCategory::Unsupported,
    )?;
    for tag in roots.tags {
        property(tag.record, Some(index), "recorder.header")?;
    }
    Ok(())
}

struct SourceOwner {
    actor_class: Vec<u16>,
    actor_path: Vec<u16>,
    object_name: Vec<u16>,
    guid: i32,
    transform: NativeTransform,
}

fn source_string(
    p: &PropertyRecord<'_>,
    kind: TagKind,
    index: usize,
    field: &'static str,
) -> SemanticResult<Vec<u16>> {
    require(
        p.tag.kind == kind,
        Some(index),
        field,
        SemanticImportCategory::Type,
    )?;
    let value = decode_string(p.tag.payload, ByteOrder::Little)
        .map_err(|_| error(Some(index), field, SemanticImportCategory::Value))?;
    require(
        value.consumed == p.tag.payload.len()
            && units_valid(&value.units, true)
            && canonical_source_string(p.tag.payload, &value.units),
        Some(index),
        field,
        SemanticImportCategory::Value,
    )?;
    Ok(value.units)
}

// Persisted metadata can be constructed without preparation. Keep its source
// admission aligned with preparation's zero-gap typing contract: forced-wide
// ASCII and other noncanonical scalar encodings are not part of this adapter.
fn canonical_source_string(payload: &[u8], units: &[u16]) -> bool {
    encode_string(units, ByteOrder::Little).is_ok_and(|encoded| encoded == payload)
}
fn source_int(p: &PropertyRecord<'_>, index: usize, field: &'static str) -> SemanticResult<i32> {
    require(
        p.tag.kind == TagKind::Int,
        Some(index),
        field,
        SemanticImportCategory::Type,
    )?;
    let bytes = p
        .tag
        .payload
        .try_into()
        .map_err(|_| error(Some(index), field, SemanticImportCategory::Value))?;
    Ok(i32::from_le_bytes(bytes))
}
fn false_bool(p: &PropertyRecord<'_>, index: usize, field: &'static str) -> SemanticResult<()> {
    require(
        p.tag.kind == TagKind::Bool && p.tag.payload.is_empty() && p.bool_value == Some(false),
        Some(index),
        field,
        SemanticImportCategory::Unsupported,
    )
}
fn struct_name(
    p: &PropertyRecord<'_>,
    name: &str,
    index: usize,
    field: &'static str,
) -> SemanticResult<()> {
    require(
        p.tag.kind == TagKind::Struct && p.extra.as_ref().is_some_and(|units| exact(units, name)),
        Some(index),
        field,
        SemanticImportCategory::Type,
    )
}
fn source_transform(p: &PropertyRecord<'_>, index: usize) -> SemanticResult<NativeTransform> {
    struct_name(p, "Transform", index, "ActorTransform")?;
    let roots = read_root(p.tag.payload, PROFILE)
        .map_err(|_| error(Some(index), "ActorTransform", SemanticImportCategory::Type))?;
    let (mut translation, mut rotation, mut scale) = (None, None, None);
    for tag in roots.tags {
        let p = property(tag.record, Some(index), "ActorTransform.header")?;
        let (slot, width, type_name, field) = if exact(&p.name, "Translation") {
            (&mut translation, 3, "Vector", "ActorTransform.Translation")
        } else if exact(&p.name, "Rotation") {
            (&mut rotation, 4, "Quat", "ActorTransform.Rotation")
        } else if exact(&p.name, "Scale3D") {
            (&mut scale, 3, "Vector", "ActorTransform.Scale3D")
        } else {
            return Err(error(
                Some(index),
                "ActorTransform.field",
                SemanticImportCategory::Unsupported,
            ));
        };
        require(
            slot.is_none(),
            Some(index),
            field,
            SemanticImportCategory::Header,
        )?;
        struct_name(&p, type_name, index, field)?;
        require(
            p.tag.payload.len() == width * 4,
            Some(index),
            field,
            SemanticImportCategory::Type,
        )?;
        *slot = Some(
            p.tag
                .payload
                .as_chunks::<4>()
                .0
                .iter()
                .map(|v| f32::from_le_bytes(*v))
                .collect::<Vec<_>>(),
        );
    }
    let missing = || {
        error(
            Some(index),
            "ActorTransform",
            SemanticImportCategory::Missing,
        )
    };
    let transform = NativeTransform {
        translation: translation
            .ok_or_else(missing)?
            .try_into()
            .map_err(|_| missing())?,
        rotation: rotation
            .ok_or_else(missing)?
            .try_into()
            .map_err(|_| missing())?,
        scale: scale
            .ok_or_else(missing)?
            .try_into()
            .map_err(|_| missing())?,
    };
    transform
        .validate()
        .map_err(|_| error(Some(index), "ActorTransform", SemanticImportCategory::Value))?;
    Ok(transform)
}

fn source_owner(bytes: &[u8], index: usize) -> SemanticResult<SourceOwner> {
    let component = read_base_component_with_limit(bytes, PROFILE, MAX_BOUNDED_COMPONENT_BYTES)
        .map_err(|_| error(Some(index), "component", SemanticImportCategory::Header))?;
    require(
        component.trailer() == [0; 4],
        Some(index),
        "component.trailer",
        SemanticImportCategory::Unsupported,
    )?;
    let (mut actor_class, mut actor_path, mut object_name, mut guid, mut transform) =
        (None, None, None, None, None);
    let (mut moved, mut version) = (false, false);
    let mut seen = BTreeSet::new();
    for tag in component.fields() {
        let p = property(tag.record, Some(index), "component.header")?;
        require(
            seen.insert(p.name.clone()),
            Some(index),
            "component.field",
            SemanticImportCategory::Header,
        )?;
        if exact(&p.name, "ActorClassName") {
            actor_class = Some(source_string(&p, TagKind::Name, index, "ActorClassName")?);
        } else if exact(&p.name, "ActorPathName") {
            actor_path = Some(source_string(&p, TagKind::String, index, "ActorPathName")?);
        } else if exact(&p.name, "ObjectFName") {
            object_name = Some(source_string(&p, TagKind::Name, index, "ObjectFName")?);
        } else if exact(&p.name, "IcarusActorGUID") {
            let value = source_int(&p, index, "IcarusActorGUID")?;
            require(
                value > 0,
                Some(index),
                "IcarusActorGUID",
                SemanticImportCategory::Value,
            )?;
            guid = Some(value);
        } else if exact(&p.name, "RecordedVersion") {
            require(
                source_int(&p, index, "RecordedVersion")? == 3,
                Some(index),
                "RecordedVersion",
                SemanticImportCategory::Unsupported,
            )?;
            version = true;
        } else if exact(&p.name, "bWasMovedToSubLevel") {
            false_bool(&p, index, "bWasMovedToSubLevel")?;
            moved = true;
        } else if exact(&p.name, "ActorTransform") {
            transform = Some(source_transform(&p, index)?);
        } else if exact(&p.name, "bSkipDefaultOwnerResolution") {
            false_bool(&p, index, "bSkipDefaultOwnerResolution")?;
        } else if exact(&p.name, "OwnerResolvePolicy") {
            // Staging qualifies the accepted byte/enum representations. Check
            // them again here so persisted metadata cannot expand the subset.
            let supported = match p.tag.kind {
                TagKind::Byte if p.tag.payload.len() == 1 => {
                    p.extra.as_ref().is_some_and(|u| exact(u, "None"))
                        && matches!(p.tag.payload[0], 0 | 2)
                }
                TagKind::Byte | TagKind::Enum
                    if p.extra
                        .as_ref()
                        .is_some_and(|u| exact(u, "EStateRecorderOwnerResolvePolicy")) =>
                {
                    decode_string(p.tag.payload, ByteOrder::Little)
                        .ok()
                        .is_some_and(|v| {
                            v.consumed == p.tag.payload.len()
                                && canonical_source_string(p.tag.payload, &v.units)
                                && [
                                    "FindOnly",
                                    "FindOrRespawn",
                                    "EStateRecorderOwnerResolvePolicy::FindOnly",
                                    "EStateRecorderOwnerResolvePolicy::FindOrRespawn",
                                ]
                                .iter()
                                .any(|s| exact(&v.units, s))
                        })
                }
                _ => false,
            };
            require(
                supported,
                Some(index),
                "OwnerResolvePolicy",
                SemanticImportCategory::Unsupported,
            )?;
        } else {
            return Err(error(
                Some(index),
                "component.field",
                SemanticImportCategory::Unsupported,
            ));
        }
    }
    require(
        moved,
        Some(index),
        "bWasMovedToSubLevel",
        SemanticImportCategory::Missing,
    )?;
    require(
        version,
        Some(index),
        "RecordedVersion",
        SemanticImportCategory::Missing,
    )?;
    let missing = |field| error(Some(index), field, SemanticImportCategory::Missing);
    Ok(SourceOwner {
        actor_class: actor_class.ok_or_else(|| missing("ActorClassName"))?,
        actor_path: actor_path.ok_or_else(|| missing("ActorPathName"))?,
        object_name: object_name.ok_or_else(|| missing("ObjectFName"))?,
        guid: guid.ok_or_else(|| missing("IcarusActorGUID"))?,
        transform: transform.ok_or_else(|| missing("ActorTransform"))?,
    })
}

fn source_matches(owner: &SourceOwner, binding: &ResolvedC001Placement) -> bool {
    owner.actor_class == binding.source_actor_class
        && owner.actor_path == binding.source_actor_path
        && owner.object_name == binding.source_object_name
        && owner.guid == binding.source_icarus_guid
}

fn root_matches(metadata: &C001InertImportMetadataV1, root: &StagedRootValues) -> bool {
    metadata.source_version == root.loaded_version().unwrap_or_default()
        && metadata.source_map == root.prospect_map_name()
        && metadata.source_prospect_id == root.prospect_info().prospect_id
        && metadata.source_lobby_name == root.prospect_info().lobby_name
        && metadata.source_last_saved_date_time == root.last_saved_date_time()
        && metadata.root_presence == C001RootPresence::from(root.source_presence())
}

// Index once rather than scanning all placements for each source owner. The
// sorted keys establish canonical ranks even when stored definitions are reordered.
struct NativeBindingIndex<'a> {
    placements: BTreeMap<&'a str, (usize, &'a NativePlacement)>,
    classes: BTreeMap<&'a str, &'a NativeClassDefinition>,
    tiles: BTreeMap<&'a str, &'a NativeTileDefinition>,
    actors: BTreeMap<&'a str, &'a NativeActorRecord>,
}
impl<'a> NativeBindingIndex<'a> {
    fn new(native: &'a NativeWorldSnapshot) -> Option<Self> {
        let map = native
            .definitions
            .maps
            .iter()
            .find(|map| map.key == native.map)?;
        let sorted: BTreeMap<_, _> = map.placements.iter().map(|p| (p.key.as_str(), p)).collect();
        Some(Self {
            placements: sorted
                .into_iter()
                .enumerate()
                .map(|(rank, (key, p))| (key, (rank, p)))
                .collect(),
            classes: native
                .definitions
                .classes
                .iter()
                .map(|c| (c.key.as_str(), c))
                .collect(),
            tiles: map.tiles.iter().map(|t| (t.key.as_str(), t)).collect(),
            actors: native
                .actors
                .iter()
                .map(|a| (a.placement.as_str(), a))
                .collect(),
        })
    }
    fn matches(&self, owner: &ResolvedC001Placement, source_pose: &NativeTransform) -> bool {
        let Some((rank, placement)) = self.placements.get(owner.native_placement.as_str()) else {
            return false;
        };
        let Some(class) = self.classes.get(owner.native_class.as_str()) else {
            return false;
        };
        let Some(actor) = self.actors.get(owner.native_placement.as_str()) else {
            return false;
        };
        let Some(tile) = self.tiles.get(placement.tile.as_str()) else {
            return false;
        };
        let Ok(bounds) = source_pose.transformed_bounds(&class.local_bounds) else {
            return false;
        };
        owner.id == DomainId(*rank as u64 + 1)
            && actor.id == owner.id
            && actor.class == owner.native_class
            && placement.class == owner.native_class
            && actor.tile == placement.tile
            && class.kind == NativeClassKind::SpatialObjectV1
            && class.variables.is_empty()
            && placement.initial_values.is_empty()
            && actor.variables.is_empty()
            && (0..3)
                .all(|i| tile.bounds.min[i] <= bounds.min[i] && tile.bounds.max[i] >= bounds.max[i])
    }
}

impl C001InertImportMetadataV1 {
    /// Validate source immutability and complete binding agreement. The enclosing
    /// WorldSnapshot validates the native snapshot independently first; this
    /// method deliberately does not recompile its embedded native catalog.
    /// Saved source poses stay in the preserved payload, while native poses may
    /// change within the independently validated fixed topology.
    pub fn validate(
        &self,
        native: &NativeWorldSnapshot,
        preserved: &PreservedSection,
    ) -> Result<()> {
        serialized_bound(self, MAX_C001_METADATA_BYTES)?;
        ensure!(
            self.format == C001_INERT_METADATA_FORMAT && self.source_version == 2,
            "C001 metadata format or version invalid"
        );
        ensure!(
            digest_valid(&self.source_payload_sha256) && digest_valid(&self.native_catalog_id),
            "C001 metadata digest invalid"
        );
        ensure!(
            self.source_payload_sha256 == crate::hex_lower(&preserved.payload_sha256()),
            "C001 metadata preserved digest mismatch"
        );
        ensure!(
            self.native_catalog_id == native.catalog_id
                && self.native_map == native.map
                && key_valid(&self.native_map),
            "C001 metadata catalog or map mismatch"
        );
        ensure!(
            units_valid(&self.source_map, true)
                && units_valid(&self.source_prospect_id, true)
                && units_valid(&self.source_lobby_name, false),
            "C001 metadata source identity invalid"
        );
        let p = &self.root_presence;
        ensure!(
            p.version
                && p.prospect_info
                && p.prospect_map_name
                && p.state_recorder_blobs
                && !p.legacy_prospect_id
                && !p.legacy_prospect_dt_key
                && p.last_saved_date_time == self.source_last_saved_date_time.is_some(),
            "C001 metadata root presence invalid"
        );
        ensure!(
            !self.owners.is_empty()
                && self.owners.len() <= MAX_NATIVE_PLACEMENTS
                && self.owners.len() == native.actors.len(),
            "C001 metadata owner count invalid"
        );
        let (mut indices, mut ids, mut paths, mut guids, mut placements) = (
            BTreeSet::new(),
            BTreeSet::new(),
            BTreeSet::new(),
            BTreeSet::new(),
            BTreeSet::new(),
        );
        for owner in &self.owners {
            ensure!(
                owner.source_index < self.owners.len()
                    && indices.insert(owner.source_index)
                    && owner.id.0 > 0
                    && ids.insert(owner.id)
                    && owner.source_icarus_guid > 0
                    && guids.insert(owner.source_icarus_guid)
                    && paths.insert(&owner.source_actor_path)
                    && placements.insert(&owner.native_placement),
                "C001 metadata identity duplicate or invalid"
            );
            ensure!(
                units_valid(&owner.source_actor_class, true)
                    && units_valid(&owner.source_actor_path, true)
                    && units_valid(&owner.source_object_name, true)
                    && key_valid(&owner.native_class)
                    && key_valid(&owner.native_placement),
                "C001 metadata binding identity invalid"
            );
        }
        // Recheck exact source values instead of trusting a digest next to mutable
        // metadata. Every source ordinal is checked even after native pose edits.
        let payload = preserved
            .decode()
            .and_then(|tree| tree.to_inner_payload())
            .map_err(|_| anyhow::anyhow!("C001 metadata preserved reconstruction invalid"))?;
        let roots = read_root(&payload, PROFILE)
            .map_err(|_| anyhow::anyhow!("C001 metadata root framing invalid"))?;
        let (_, root, decisions) = stage_prospect_save_state_with_root_values(
            &roots.tags,
            PROFILE,
            MAX_BOUNDED_COMPONENT_BYTES,
            None,
        )
        .map_err(|_| anyhow::anyhow!("C001 metadata source staging invalid"))?;
        validate_root(&root)?;
        validate_source_headers(&payload)?;
        ensure!(
            root_matches(self, &root),
            "C001 metadata immutable root mismatch"
        );
        let recorders = read_recorders(&roots.tags, PROFILE)
            .map_err(|_| anyhow::anyhow!("C001 metadata source recorders invalid"))?;
        ensure!(
            recorders.records().len() == self.owners.len(),
            "C001 metadata source owner count mismatch"
        );
        let owners: BTreeMap<_, _> = self.owners.iter().map(|o| (o.source_index, o)).collect();
        let native_bindings = NativeBindingIndex::new(native)
            .ok_or_else(|| anyhow::anyhow!("C001 metadata native map missing"))?;
        for (index, recorder) in recorders.records().iter().enumerate() {
            let decision = &decisions[index];
            ensure!(
                decision.retained
                    && !decision.reset_rocket_spawn
                    && !decision.physics_dynamic_tree
                    && !decision.skip_default_owner_resolution
                    && !decision.requires_stitching
                    && matches!(
                        decision.owner_resolve_policy,
                        OwnerResolvePolicy::FindOnly | OwnerResolvePolicy::FindOrRespawn
                    ),
                "C001 metadata source decision unsupported"
            );
            let class = decode_string(recorder.class_value, ByteOrder::Little)
                .map_err(|_| anyhow::anyhow!("C001 metadata source class invalid"))?;
            ensure!(
                class.consumed == recorder.class_value.len() && exact(&class.units, C001),
                "C001 metadata source class unsupported"
            );
            validate_wrapper(recorder.record, index)?;
            let source = source_owner(recorder.binary_data, index)?;
            let owner = owners[&index];
            ensure!(
                source_matches(&source, owner) && native_bindings.matches(owner, &source.transform),
                "C001 metadata immutable owner mismatch"
            );
        }
        Ok(())
    }
}

/// Counts only: no source names, identifiers, poses, digests or supplied values.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct C001InertInspection {
    pub recorders: usize,
    pub bindings: usize,
    pub placements: usize,
    pub payload_bytes: usize,
    pub metadata_bytes: usize,
}

pub struct PreparedC001InertImport {
    snapshot: WorldSnapshot,
    inspection: C001InertInspection,
}
impl PreparedC001InertImport {
    pub fn snapshot(&self) -> &WorldSnapshot {
        &self.snapshot
    }
    pub fn inspection(&self) -> C001InertInspection {
        self.inspection.clone()
    }
}
impl fmt::Debug for PreparedC001InertImport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.inspection.fmt(f)
    }
}

/// Prepare a complete inert authored placement world without a destination or
/// any filesystem side effect. There is no filtering, respawn or stitching.
pub fn prepare_c001_inert_import(
    candidate: &DecodedImportCandidate,
    catalog: Arc<CompiledNativeCatalog>,
    bindings: &CompiledC001InertBindings,
) -> SemanticResult<PreparedC001InertImport> {
    use SemanticImportCategory::{Binding, Limit, Unsupported, Validation};
    let bindings = bindings.definition();
    let root = candidate.root_values();
    validate_root(root)?;
    validate_source_headers(candidate.payload())?;
    require(
        bindings.expected_payload_sha256
            == crate::hex_lower(&candidate.preserved().payload_sha256()),
        None,
        "payload_sha256",
        Binding,
    )?;
    require(
        bindings.native_catalog_id == catalog.identity(),
        None,
        "native_catalog_id",
        Binding,
    )?;
    require(
        bindings.source_map == root.prospect_map_name(),
        None,
        "ProspectMapName",
        Binding,
    )?;
    let map = catalog
        .map(&bindings.native_map)
        .ok_or_else(|| error(None, "native_map", Binding))?;
    let count = candidate.recorders().len();
    require(
        count > 0 && count <= MAX_NATIVE_PLACEMENTS,
        None,
        "recorders",
        Limit,
    )?;
    require(
        count == bindings.owners.len() && count == map.placements.len(),
        None,
        "owners",
        Binding,
    )?;
    let mut native = prepare_authored_world(catalog.clone(), &bindings.native_map)
        .map_err(|_| error(None, "native_world", Validation))?
        .snapshot()
        .clone();
    let bindings_by_path: BTreeMap<_, _> = bindings
        .owners
        .iter()
        .map(|o| (o.source_actor_path.as_slice(), o))
        .collect();
    let placement_ranks: BTreeMap<_, _> = native
        .actors
        .iter()
        .enumerate()
        .map(|(i, a)| (a.placement.clone(), i))
        .collect();
    let (mut paths, mut guids, mut selected) = (BTreeSet::new(), BTreeSet::new(), BTreeSet::new());
    let mut owners = Vec::with_capacity(count);
    let native_bindings =
        NativeBindingIndex::new(&native).ok_or_else(|| error(None, "native_map", Binding))?;
    let mut poses = Vec::with_capacity(count);
    for (index, recorder) in candidate.recorders().iter().enumerate() {
        let at = Some(index);
        require(
            recorder.source_index() == index && exact(recorder.class_units(), C001),
            at,
            "ComponentClassName",
            Unsupported,
        )?;
        let d = recorder.decision();
        require(
            d.retained
                && !d.reset_rocket_spawn
                && !d.physics_dynamic_tree
                && !d.skip_default_owner_resolution
                && !d.requires_stitching
                && matches!(
                    d.owner_resolve_policy,
                    OwnerResolvePolicy::FindOnly | OwnerResolvePolicy::FindOrRespawn
                ),
            at,
            "recorder.decision",
            Unsupported,
        )?;
        let (_, summary) = recorder
            .typing()
            .lossless()
            .ok_or_else(|| error(at, "recorder.typing", Unsupported))?;
        require(
            summary.uninterpreted == 0,
            at,
            "recorder.typing",
            Unsupported,
        )?;
        validate_wrapper(
            candidate
                .recorder_bytes(index)
                .ok_or_else(|| error(at, "recorder", Validation))?,
            index,
        )?;
        let source = source_owner(
            candidate
                .recorder_binary_data(index)
                .ok_or_else(|| error(at, "component", Validation))?,
            index,
        )?;
        require(
            paths.insert(source.actor_path.clone()) && guids.insert(source.guid),
            at,
            "owner.identity",
            Binding,
        )?;
        let binding = bindings_by_path
            .get(source.actor_path.as_slice())
            .ok_or_else(|| error(at, "owner.binding", Binding))?;
        require(
            binding.source_actor_class == source.actor_class
                && binding.source_object_name == source.object_name
                && binding.source_icarus_guid == source.guid
                && selected.insert(binding.native_placement.clone()),
            at,
            "owner.binding",
            Binding,
        )?;
        let rank = *placement_ranks
            .get(&binding.native_placement)
            .ok_or_else(|| error(at, "native_placement", Binding))?;
        let owner = ResolvedC001Placement {
            source_index: index,
            id: native.actors[rank].id,
            source_actor_class: source.actor_class.clone(),
            source_actor_path: source.actor_path.clone(),
            source_object_name: source.object_name.clone(),
            source_icarus_guid: source.guid,
            native_placement: binding.native_placement.clone(),
            native_class: binding.native_class.clone(),
        };
        require(
            native_bindings.matches(&owner, &source.transform),
            at,
            "native_placement",
            Binding,
        )?;
        poses.push((rank, source.transform));
        owners.push(owner);
    }
    drop(native_bindings);
    for (rank, transform) in poses {
        native.actors[rank].transform = transform;
    }
    let metadata = C001InertImportMetadataV1 {
        format: C001_INERT_METADATA_FORMAT,
        adapter: C001InertAdapter::C001InertPlacementV1,
        source_version: 2,
        source_payload_sha256: bindings.expected_payload_sha256.clone(),
        native_catalog_id: bindings.native_catalog_id.clone(),
        native_map: bindings.native_map.clone(),
        coordinates: bindings.coordinates,
        source_map: root.prospect_map_name().to_vec(),
        source_prospect_id: root.prospect_info().prospect_id.clone(),
        source_lobby_name: root.prospect_info().lobby_name.clone(),
        source_last_saved_date_time: root.last_saved_date_time(),
        root_presence: root.source_presence().into(),
        owners,
    };
    let metadata_bytes = serialized_bound(&metadata, MAX_C001_METADATA_BYTES)
        .map_err(|_| error(None, "metadata", Limit))?;
    let preserved = candidate.preserved();
    let preserved = PreservedSection::from_native_encoding(
        preserved.native_encoding().to_vec(),
        preserved.payload_bytes(),
        preserved.payload_sha256(),
    )
    .map_err(|_| error(None, "preserved_prospect", Validation))?;
    let snapshot = WorldSnapshot {
        schema_version: SNAPSHOT_SCHEMA_C001_INERT,
        tick: 0,
        entities: Vec::new(),
        preserved_prospect: Some(Arc::new(preserved)),
        native_world: Some(native),
        native_import: Some(metadata),
    };
    // Preflight validates the entire snapshot before encoding it. Avoid a
    // separate validation that would reconstruct and check the source twice.
    SnapshotStore::preflight(&snapshot)
        .map_err(|_| error(None, "snapshot.preflight", Validation))?;
    Ok(PreparedC001InertImport {
        snapshot,
        inspection: C001InertInspection {
            recorders: count,
            bindings: count,
            placements: count,
            payload_bytes: candidate.payload().len(),
            metadata_bytes,
        },
    })
}

/// Preflight before opening the destination; storage owns exclusive creation and
/// atomic publication and refuses existing or recoverable snapshot stores.
pub fn publish_new_c001_inert_import(
    prepared: PreparedC001InertImport,
    destination: &Path,
) -> Result<SaveReceipt> {
    SnapshotStore::preflight(prepared.snapshot())?;
    SnapshotStore::open(destination)?.save_new(prepared.snapshot())
}

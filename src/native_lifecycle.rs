//! Independently authored owner lifecycles and atomic spatial recorder restores.
//! This is not an original-save adapter or a claim about original owner lookup.
use std::{
    collections::BTreeSet,
    fmt,
    fs::OpenOptions,
    io::{Read, Write},
    os::unix::fs::OpenOptionsExt,
    path::Path,
};

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Deserializer, Serialize};
use sha2::{Digest, Sha256};

use crate::{
    experimental::restore_rules::{
        AttachRecorderOutcome, OwnerResolvePolicy, OwnerResolveStep,
        ResolveOwnerRecorderStateInputs, ResolveOwnerRecorderStateOutcome,
        does_actor_recorder_match_actor, evaluate_attach_recorder_to_actor,
        evaluate_owner_resolve_step, evaluate_resolve_owner_recorder_state,
    },
    native_relationships::{CompiledNativeRelationships, NativeRelationshipsSnapshot},
    native_world::{
        CompiledNativeCatalog, MAX_NATIVE_CATALOG_BYTES, MAX_NATIVE_CLASSES, MAX_NATIVE_KEY_BYTES,
        MAX_NATIVE_MAPS, MAX_NATIVE_PLACEMENTS, MAX_NATIVE_STRING_UNITS, MAX_NATIVE_TILES,
        MAX_NATIVE_VARIABLE_BYTES, MAX_NATIVE_VARIABLE_VALUES, MAX_NATIVE_VARIABLES_PER_CLASS,
        NativeActorRecord, NativeActorView, NativeCatalogDefinition, NativeTransform, NativeValue,
        NativeVariableValue, NativeWorldSnapshot,
    },
    persistence::{SaveReceipt, SnapshotStore},
    simulation::{
        DomainId, SNAPSHOT_SCHEMA_LIFECYCLE, SNAPSHOT_SCHEMA_NATIVE, SNAPSHOT_SCHEMA_RELATIONSHIPS,
        WorldSnapshot,
    },
};

pub const NATIVE_LIFECYCLE_FORMAT: u32 = 1;
pub const MAX_LIFECYCLE_DEFINITION_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_LIFECYCLE_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_RESTORE_BATCH_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_LIFECYCLE_NAME_UNITS: usize = 1024;
pub const MAX_LIFECYCLE_NAME_TOTAL_UNITS: usize = 1 << 20;

// All named boundaries reject sequences and replace serde diagnostics, which
// otherwise include unknown field names and caller-supplied enum strings.
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
                    .map_err(|_| <D::Error as serde::de::Error>::custom("native lifecycle object invalid"))?;
                Ok(Self { $($field: fields.$field),* })
            }
        }
        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.debug_struct(stringify!($name)).finish_non_exhaustive()
            }
        }
    };
}

macro_rules! string_enum {
    ($name:ident { $($variant:ident => $text:literal),* $(,)? }) => {
        #[derive(Clone, Copy, PartialEq, Eq, Serialize)]
        #[serde(rename_all = "snake_case")]
        pub enum $name { $($variant),* }
        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
                struct EnumVisitor;
                impl serde::de::Visitor<'_> for EnumVisitor {
                    type Value = $name;
                    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                        f.write_str("a native lifecycle enum string")
                    }
                    fn visit_str<E: serde::de::Error>(self, value: &str) -> std::result::Result<Self::Value, E> {
                        match value {
                            $($text => Ok($name::$variant),)*
                            _ => Err(E::custom("native lifecycle enum invalid")),
                        }
                    }
                }
                deserializer.deserialize_str(EnumVisitor)
                    .map_err(|_| <D::Error as serde::de::Error>::custom("native lifecycle enum invalid"))
            }
        }
        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.debug_struct(stringify!($name)).finish_non_exhaustive()
            }
        }
    };
}

string_enum!(RecorderKind { SpatialStateV1 => "spatial_state_v1" });
string_enum!(Presence { Absent => "absent", Live => "live" });
string_enum!(ResolvePolicy {
    FindOnly => "find_only", FindOrRespawn => "find_or_respawn", RespawnOnly => "respawn_only",
});
string_enum!(SpawnComponent { Missing => "missing", DeclaredKind => "declared_kind" });
string_enum!(RestoreDisposition {
    KeptBound => "kept_bound", Attached => "attached", Spawned => "spawned", Replaced => "replaced",
});
string_enum!(ComponentDisposition { Existing => "existing", Created => "created" });

object_dto! { pub struct RecorderComponentRecord { pub kind: RecorderKind, pub guid: i32 } }
object_dto! { pub struct SlotDeclaration {
    pub placement: String, pub owner_name: Vec<u16>, pub recorder_kind: RecorderKind,
    pub initial_presence: Presence, pub initial_component: Option<RecorderComponentRecord>,
    pub spawn_component: SpawnComponent,
} }
object_dto! { pub struct LifecycleDefinition {
    pub lifecycle_format: u32, pub native_catalog_id: String, pub map: String,
    pub slots: Vec<SlotDeclaration>,
} }
object_dto! { pub struct SlotRecord {
    pub id: DomainId, pub presence: Presence, pub incarnation: u64,
    pub component: Option<RecorderComponentRecord>,
} }
object_dto! { pub struct OwnerHandle { pub id: DomainId, pub incarnation: u64 } }
impl Copy for OwnerHandle {}
impl Eq for OwnerHandle {}
object_dto! { pub struct RecorderBinding {
    pub guid: i32, pub kind: RecorderKind, pub owner: OwnerHandle,
} }
object_dto! { pub struct LifecycleSnapshot {
    pub lifecycle_id: String, pub definition: LifecycleDefinition, pub slots: Vec<SlotRecord>,
    pub bindings: Vec<RecorderBinding>, pub restore_sequence: u64,
    pub completed: Option<CompletedRestore>,
} }

#[derive(Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum RecorderSource {
    Bound,
    Detached {
        expected_prior_binding: Option<OwnerHandle>,
    },
}
impl<'de> Deserialize<'de> for RecorderSource {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
        enum Tagged {
            Bound,
            Detached {
                expected_prior_binding: Option<OwnerHandle>,
            },
        }
        let tagged = crate::json::object(deserializer).map_err(|_| {
            <D::Error as serde::de::Error>::custom("native lifecycle source invalid")
        })?;
        Ok(match tagged {
            Tagged::Bound => Self::Bound,
            Tagged::Detached {
                expected_prior_binding,
            } => Self::Detached {
                expected_prior_binding,
            },
        })
    }
}
impl fmt::Debug for RecorderSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RecorderSource").finish_non_exhaustive()
    }
}

object_dto! { pub struct RestoreRecorder {
    pub guid: i32, pub kind: RecorderKind, pub target_placement: String,
    pub owner_name: Vec<u16>, pub policy: ResolvePolicy, pub source: RecorderSource,
    pub create_if_missing: bool, pub expected_presence: Presence, pub expected_incarnation: u64,
    pub transform: NativeTransform, pub variables: Vec<NativeVariableValue>,
} }
object_dto! { pub struct RestoreBatch {
    pub lifecycle_id: String, pub sequence: u64, pub recorders: Vec<RestoreRecorder>,
} }
object_dto! { pub struct CompletedRestore {
    pub sequence: u64, pub request_digest: String, pub outcomes: Vec<CompletedRecorder>,
} }
object_dto! { pub struct CompletedRecorder {
    pub guid: i32, pub target: DomainId, pub policy: ResolvePolicy, pub source: RecorderSource,
    pub before_presence: Presence, pub before_incarnation: u64, pub owner: OwnerHandle,
    pub disposition: RestoreDisposition, pub component_disposition: ComponentDisposition,
} }

fn key_valid(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= MAX_NATIVE_KEY_BYTES
        && key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
}
fn digest_valid(digest: &str) -> bool {
    digest.len() == 64
        && digest
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn add_count(total: &mut usize, count: usize, limit: usize) -> Result<()> {
    *total = total
        .checked_add(count)
        .context("native lifecycle aggregate overflow")?;
    ensure!(*total <= limit, "native lifecycle aggregate limit");
    Ok(())
}
fn validate_name(name: &Vec<u16>, total: &mut usize) -> Result<()> {
    ensure!(
        !name.is_empty() && name.capacity() <= MAX_LIFECYCLE_NAME_UNITS,
        "native lifecycle name length invalid"
    );
    add_count(total, name.capacity(), MAX_LIFECYCLE_NAME_TOTAL_UNITS)
}

fn key_capacity(key: &String) -> Result<()> {
    ensure!(
        key_valid(key) && key.capacity() <= MAX_NATIVE_KEY_BYTES,
        "native lifecycle key capacity invalid"
    );
    Ok(())
}
fn digest_capacity(value: &String) -> Result<()> {
    ensure!(
        digest_valid(value) && value.capacity() <= 64,
        "native lifecycle digest capacity invalid"
    );
    Ok(())
}

struct BoundedWriter {
    count: usize,
    limit: usize,
}
impl Write for BoundedWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.count = self
            .count
            .checked_add(bytes.len())
            .filter(|n| *n <= self.limit)
            .ok_or_else(|| std::io::Error::other("native lifecycle byte limit"))?;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
fn encoded_size<T: Serialize + ?Sized>(value: &T, limit: usize) -> Result<usize> {
    let mut writer = BoundedWriter { count: 0, limit };
    serde_json::to_writer(&mut writer, value)
        .map_err(|_| anyhow::anyhow!("native lifecycle serialization or byte limit"))?;
    Ok(writer.count)
}
fn digest<T: Serialize>(value: &T, limit: usize) -> Result<String> {
    struct HashWriter {
        bound: BoundedWriter,
        hash: Sha256,
    }
    impl Write for HashWriter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            let n = self.bound.write(bytes)?;
            self.hash.update(bytes);
            Ok(n)
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut writer = HashWriter {
        bound: BoundedWriter { count: 0, limit },
        hash: Sha256::new(),
    };
    serde_json::to_writer(&mut writer, value)
        .map_err(|_| anyhow::anyhow!("native lifecycle serialization or byte limit"))?;
    Ok(crate::hex_lower(&writer.hash.finalize()))
}
fn parse_document<T: serde::de::DeserializeOwned>(text: &str, limit: usize) -> Result<T> {
    ensure!(text.len() <= limit, "native lifecycle input byte limit");
    if text.trim_start().starts_with('{') {
        serde_json::from_str(text).map_err(|_| anyhow::anyhow!("native lifecycle JSON invalid"))
    } else {
        toml::from_str(text).map_err(|_| anyhow::anyhow!("native lifecycle TOML invalid"))
    }
}
fn load_document(path: &Path, limit: usize) -> Result<String> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(crate::nonblocking_flag())
        .open(path)
        .map_err(|e| anyhow::anyhow!("native lifecycle open: {:?}", e.kind()))?;
    let metadata = file
        .metadata()
        .map_err(|e| anyhow::anyhow!("native lifecycle metadata: {:?}", e.kind()))?;
    ensure!(metadata.is_file(), "native lifecycle requires regular file");
    ensure!(
        metadata.len() <= limit as u64,
        "native lifecycle input byte limit"
    );
    let mut bytes = Vec::new();
    file.take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| anyhow::anyhow!("native lifecycle read: {:?}", e.kind()))?;
    ensure!(bytes.len() <= limit, "native lifecycle input byte limit");
    String::from_utf8(bytes).map_err(|_| anyhow::anyhow!("native lifecycle encoding invalid"))
}

fn validate_definition(definition: &LifecycleDefinition) -> Result<()> {
    ensure!(
        definition.lifecycle_format == NATIVE_LIFECYCLE_FORMAT,
        "native lifecycle format unsupported"
    );
    digest_capacity(&definition.native_catalog_id)?;
    key_capacity(&definition.map)?;
    ensure!(
        !definition.slots.is_empty() && definition.slots.capacity() <= MAX_NATIVE_PLACEMENTS,
        "native lifecycle slot count invalid"
    );
    let mut units = 0;
    for slot in &definition.slots {
        key_capacity(&slot.placement)?;
        validate_name(&slot.owner_name, &mut units)?;
        ensure!(
            slot.initial_presence != Presence::Absent || slot.initial_component.is_none(),
            "native lifecycle absent initial component"
        );
        if let Some(component) = &slot.initial_component {
            ensure!(
                component.guid > 0 && component.kind == slot.recorder_kind,
                "native lifecycle initial component invalid"
            );
        }
    }
    encoded_size(definition, MAX_LIFECYCLE_DEFINITION_BYTES)?;
    Ok(())
}

/// Holds the already compiled spatial catalog so capture validates without
/// recompiling or hashing immutable definitions.
#[derive(Clone)]
pub struct CompiledNativeLifecycle {
    identity: String,
    definition: LifecycleDefinition,
    catalog: CompiledNativeCatalog,
}
impl fmt::Debug for CompiledNativeLifecycle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CompiledNativeLifecycle")
            .field("slots", &self.definition.slots.len())
            .finish_non_exhaustive()
    }
}
impl CompiledNativeLifecycle {
    pub fn compile(
        mut definition: LifecycleDefinition,
        catalog: &CompiledNativeCatalog,
    ) -> Result<Self> {
        validate_definition(&definition)?;
        validate_catalog_capacities(catalog.definition())?;
        ensure!(
            definition.native_catalog_id == catalog.identity(),
            "native lifecycle catalog mismatch"
        );
        let map = catalog
            .map(&definition.map)
            .context("native lifecycle map missing")?;
        ensure!(
            definition.slots.len() == map.placements.len(),
            "native lifecycle coverage incomplete"
        );
        definition
            .slots
            .sort_by(|a, b| a.placement.cmp(&b.placement));
        let mut guids = BTreeSet::new();
        for (slot, placement) in definition.slots.iter().zip(&map.placements) {
            ensure!(
                slot.placement == placement.key,
                "native lifecycle placement coverage invalid"
            );
            if let Some(component) = &slot.initial_component {
                ensure!(
                    guids.insert(component.guid),
                    "native lifecycle duplicate initial GUID"
                );
            }
        }
        let identity = digest(&definition, MAX_LIFECYCLE_DEFINITION_BYTES)?;
        Ok(Self {
            identity,
            definition,
            catalog: catalog.clone(),
        })
    }
    pub fn parse(text: &str, catalog: &CompiledNativeCatalog) -> Result<Self> {
        Self::compile(
            parse_document(text, MAX_LIFECYCLE_DEFINITION_BYTES)?,
            catalog,
        )
    }
    pub fn load(path: &Path, catalog: &CompiledNativeCatalog) -> Result<Self> {
        Self::parse(
            &load_document(path, MAX_LIFECYCLE_DEFINITION_BYTES)?,
            catalog,
        )
    }
    pub fn identity(&self) -> &str {
        &self.identity
    }
    pub fn definition(&self) -> &LifecycleDefinition {
        &self.definition
    }
    pub fn initial_snapshot(&self) -> LifecycleSnapshot {
        let slots: Vec<_> = self
            .definition
            .slots
            .iter()
            .enumerate()
            .map(|(rank, slot)| SlotRecord {
                id: DomainId(rank as u64 + 1),
                presence: slot.initial_presence,
                incarnation: u64::from(slot.initial_presence == Presence::Live),
                component: slot.initial_component.clone(),
            })
            .collect();
        let mut bindings: Vec<_> = slots
            .iter()
            .filter_map(|slot| {
                slot.component.as_ref().map(|component| RecorderBinding {
                    guid: component.guid,
                    kind: component.kind,
                    owner: OwnerHandle {
                        id: slot.id,
                        incarnation: slot.incarnation,
                    },
                })
            })
            .collect();
        sort_bindings(&mut bindings);
        LifecycleSnapshot {
            lifecycle_id: self.identity.clone(),
            definition: self.definition.clone(),
            slots,
            bindings,
            restore_sequence: 0,
            completed: None,
        }
    }
    pub fn from_snapshot(
        lifecycle: &LifecycleSnapshot,
        native: &NativeWorldSnapshot,
    ) -> Result<Self> {
        // Bound all programmatic payloads before cloning embedded definitions.
        validate_snapshot_bounds(lifecycle)?;
        validate_native_bounds(native)?;
        let catalog = CompiledNativeCatalog::compile(native.definitions.clone())?;
        let compiled = Self::compile(lifecycle.definition.clone(), &catalog)?;
        compiled.validate_snapshot(lifecycle, native)?;
        Ok(compiled)
    }
    pub fn validate_snapshot(
        &self,
        lifecycle: &LifecycleSnapshot,
        native: &NativeWorldSnapshot,
    ) -> Result<()> {
        validate_snapshot_bounds(lifecycle)?;
        validate_native_bounds(native)?;
        ensure!(
            lifecycle.lifecycle_id == self.identity && lifecycle.definition == self.definition,
            "native lifecycle immutable definition mismatch"
        );
        ensure!(
            native.catalog_id == self.catalog.identity()
                && native.definitions == *self.catalog.definition()
                && native.map == self.definition.map,
            "native lifecycle spatial binding mismatch"
        );
        self.catalog
            .validate_records(&native.map, native.actors.iter().map(NativeActorView::from))?;
        ensure!(
            lifecycle.slots.len() == self.definition.slots.len(),
            "native lifecycle state coverage incomplete"
        );
        let mut component_guids = BTreeSet::new();
        for (rank, (slot, declaration)) in lifecycle
            .slots
            .iter()
            .zip(&self.definition.slots)
            .enumerate()
        {
            ensure!(
                slot.id == DomainId(rank as u64 + 1),
                "native lifecycle slot order invalid"
            );
            validate_presence(slot.presence, slot.incarnation, declaration)?;
            if slot.presence == Presence::Absent {
                ensure!(
                    slot.component.is_none(),
                    "native lifecycle absent component"
                );
            }
            if let Some(component) = &slot.component {
                ensure!(
                    component.guid > 0
                        && component.kind == declaration.recorder_kind
                        && component_guids.insert(component.guid),
                    "native lifecycle component invalid"
                );
            }
        }
        ensure!(
            lifecycle.bindings.len() == component_guids.len(),
            "native lifecycle binding coverage invalid"
        );
        let mut previous = None;
        for binding in &lifecycle.bindings {
            ensure!(
                binding.guid > 0 && previous.is_none_or(|guid| guid < binding.guid),
                "native lifecycle binding order or GUID invalid"
            );
            previous = Some(binding.guid);
            let slot = slot_by_id(lifecycle, binding.owner.id)?;
            ensure!(
                slot.presence == Presence::Live
                    && slot.incarnation == binding.owner.incarnation
                    && slot
                        .component
                        .as_ref()
                        .is_some_and(|component| component.guid == binding.guid
                            && component.kind == binding.kind),
                "native lifecycle authoritative binding mismatch"
            );
        }
        if let Some(completed) = &lifecycle.completed {
            ensure!(
                lifecycle.restore_sequence == completed.sequence,
                "native lifecycle completion sequence mismatch"
            );
            validate_completed(lifecycle, completed)?;
        } else {
            ensure!(
                lifecycle.restore_sequence == 0,
                "native lifecycle completion missing"
            );
            // There is no V1 mutation path before the first committed restore.
            for (slot, declaration) in lifecycle.slots.iter().zip(&self.definition.slots) {
                ensure!(
                    slot.presence == declaration.initial_presence
                        && slot.incarnation
                            == u64::from(declaration.initial_presence == Presence::Live)
                        && slot.component == declaration.initial_component,
                    "native lifecycle uncompleted initial state mismatch"
                );
            }
        }
        Ok(())
    }
}

fn sort_bindings(bindings: &mut [RecorderBinding]) {
    bindings.sort_by_key(|binding| (binding.guid, binding.owner.id, binding.owner.incarnation));
}
fn slot_by_id(lifecycle: &LifecycleSnapshot, id: DomainId) -> Result<&SlotRecord> {
    let index =
        id.0.checked_sub(1)
            .and_then(|n| usize::try_from(n).ok())
            .context("native lifecycle owner ID invalid")?;
    lifecycle
        .slots
        .get(index)
        .filter(|slot| slot.id == id)
        .context("native lifecycle owner ID missing")
}
fn validate_presence(
    presence: Presence,
    incarnation: u64,
    declaration: &SlotDeclaration,
) -> Result<()> {
    match presence {
        Presence::Absent => ensure!(
            declaration.initial_presence == Presence::Absent && incarnation == 0,
            "native lifecycle absent incarnation invalid"
        ),
        Presence::Live => ensure!(incarnation > 0, "native lifecycle live incarnation invalid"),
    }
    Ok(())
}
fn validate_snapshot_bounds(lifecycle: &LifecycleSnapshot) -> Result<()> {
    validate_definition(&lifecycle.definition)?;
    digest_capacity(&lifecycle.lifecycle_id)?;
    ensure!(
        lifecycle.slots.capacity() <= MAX_NATIVE_PLACEMENTS
            && lifecycle.bindings.capacity() <= MAX_NATIVE_PLACEMENTS,
        "native lifecycle state count limit"
    );
    if let Some(completed) = &lifecycle.completed {
        digest_capacity(&completed.request_digest)?;
        ensure!(
            completed.outcomes.capacity() <= MAX_NATIVE_PLACEMENTS,
            "native lifecycle outcome count limit"
        );
    }
    encoded_size(lifecycle, MAX_LIFECYCLE_BYTES)?;
    Ok(())
}
fn validate_native_bounds(native: &NativeWorldSnapshot) -> Result<()> {
    ensure!(
        native.actors.capacity() <= MAX_NATIVE_PLACEMENTS,
        "native lifecycle spatial count limit"
    );
    digest_capacity(&native.catalog_id)?;
    key_capacity(&native.map)?;
    validate_catalog_capacities(&native.definitions)?;
    let mut values = 0;
    let mut bytes = 0;
    for actor in &native.actors {
        key_capacity(&actor.placement)?;
        key_capacity(&actor.class)?;
        key_capacity(&actor.tile)?;
        validate_variables(&actor.variables, &mut values, &mut bytes)?;
    }
    Ok(())
}

fn validate_value_capacity(value: &NativeValue) -> Result<usize> {
    match value {
        NativeValue::F32(value) => {
            ensure!(value.is_finite(), "native lifecycle variable nonfinite");
            Ok(0)
        }
        NativeValue::String(value) => {
            ensure!(
                value.capacity() <= MAX_NATIVE_STRING_UNITS * 4
                    && value
                        .encode_utf16()
                        .take(MAX_NATIVE_STRING_UNITS + 1)
                        .count()
                        <= MAX_NATIVE_STRING_UNITS,
                "native lifecycle variable string limit"
            );
            Ok(value.capacity())
        }
        _ => Ok(0),
    }
}

fn validate_catalog_capacities(definition: &NativeCatalogDefinition) -> Result<()> {
    ensure!(
        definition.classes.capacity() <= MAX_NATIVE_CLASSES
            && definition.maps.capacity() <= MAX_NATIVE_MAPS,
        "native lifecycle catalog capacity limit"
    );
    let mut values = 0;
    let mut bytes = 0;
    for class in &definition.classes {
        key_capacity(&class.key)?;
        ensure!(
            class.variables.capacity() <= MAX_NATIVE_VARIABLES_PER_CLASS,
            "native lifecycle class variable capacity limit"
        );
        add_count(
            &mut values,
            class.variables.capacity(),
            MAX_NATIVE_VARIABLE_VALUES,
        )?;
        for variable in &class.variables {
            key_capacity(&variable.key)?;
            add_count(
                &mut bytes,
                validate_value_capacity(&variable.default)? + variable.key.capacity(),
                MAX_NATIVE_VARIABLE_BYTES,
            )?;
        }
    }
    let mut placements = 0;
    let mut tiles = 0;
    for map in &definition.maps {
        key_capacity(&map.key)?;
        add_count(
            &mut placements,
            map.placements.capacity(),
            MAX_NATIVE_PLACEMENTS,
        )?;
        add_count(&mut tiles, map.tiles.capacity(), MAX_NATIVE_TILES)?;
        for tile in &map.tiles {
            key_capacity(&tile.key)?;
        }
        for placement in &map.placements {
            key_capacity(&placement.key)?;
            key_capacity(&placement.class)?;
            key_capacity(&placement.tile)?;
            validate_variables(&placement.initial_values, &mut values, &mut bytes)?;
        }
    }
    encoded_size(definition, MAX_NATIVE_CATALOG_BYTES)?;
    Ok(())
}

fn validate_relationship_capacities(relationships: &NativeRelationshipsSnapshot) -> Result<()> {
    digest_capacity(&relationships.relationship_id)?;
    digest_capacity(&relationships.definition.native_catalog_id)?;
    key_capacity(&relationships.definition.map)?;
    ensure!(
        relationships.definition.actors.capacity() <= MAX_NATIVE_PLACEMENTS
            && relationships.foundations.capacity() <= MAX_NATIVE_PLACEMENTS,
        "native lifecycle relationship capacity limit"
    );
    let mut names = 0;
    for declaration in &relationships.definition.actors {
        key_capacity(&declaration.placement)?;
        validate_name(&declaration.identity.name, &mut names)?;
        if let Some(link) = &declaration.foundation {
            validate_name(&link.name, &mut names)?;
        }
    }
    encoded_size(
        &relationships.definition,
        crate::native_relationships::MAX_NATIVE_RELATIONSHIP_BYTES,
    )?;
    Ok(())
}

fn validate_completed(lifecycle: &LifecycleSnapshot, completed: &CompletedRestore) -> Result<()> {
    ensure!(
        completed.sequence > 0
            && digest_valid(&completed.request_digest)
            && !completed.outcomes.is_empty(),
        "native lifecycle completion header invalid"
    );
    let mut guids = BTreeSet::new();
    let mut previous = None;
    for outcome in &completed.outcomes {
        ensure!(
            outcome.guid > 0
                && guids.insert(outcome.guid)
                && previous.is_none_or(|id| id < outcome.target),
            "native lifecycle completion duplicate or order invalid"
        );
        previous = Some(outcome.target);
        let slot = slot_by_id(lifecycle, outcome.target)?;
        let declaration = &lifecycle.definition.slots[(outcome.target.0 - 1) as usize];
        validate_presence(
            outcome.before_presence,
            outcome.before_incarnation,
            declaration,
        )?;
        ensure!(
            outcome.owner.id == outcome.target
                && slot.presence == Presence::Live
                && outcome.owner.incarnation == slot.incarnation
                && slot
                    .component
                    .as_ref()
                    .is_some_and(|component| component.guid == outcome.guid),
            "native lifecycle completion disagrees with current owner"
        );
        if let RecorderSource::Detached {
            expected_prior_binding: Some(prior),
        } = &outcome.source
        {
            ensure!(
                outcome.before_presence == Presence::Live
                    && prior.id == outcome.target
                    && prior.incarnation == outcome.before_incarnation,
                "native lifecycle completion prior binding invalid"
            );
        }
        let existing_incarnation = outcome.before_presence == Presence::Live
            && outcome.owner.incarnation == outcome.before_incarnation;
        let incremented =
            outcome.before_incarnation.checked_add(1) == Some(outcome.owner.incarnation);
        let legal = match (&outcome.source, outcome.disposition) {
            (RecorderSource::Bound, RestoreDisposition::KeptBound) => {
                existing_incarnation
                    && outcome.component_disposition == ComponentDisposition::Existing
            }
            (RecorderSource::Detached { .. }, RestoreDisposition::Attached) => {
                existing_incarnation
                    && matches!(
                        outcome.policy,
                        ResolvePolicy::FindOnly | ResolvePolicy::FindOrRespawn
                    )
            }
            (RecorderSource::Detached { .. }, RestoreDisposition::Spawned) => {
                incremented
                    && outcome.before_presence == Presence::Absent
                    && matches!(
                        outcome.policy,
                        ResolvePolicy::FindOrRespawn | ResolvePolicy::RespawnOnly
                    )
            }
            (RecorderSource::Detached { .. }, RestoreDisposition::Replaced) => {
                incremented
                    && outcome.before_presence == Presence::Live
                    && outcome.policy == ResolvePolicy::RespawnOnly
            }
            _ => false,
        };
        ensure!(legal, "native lifecycle completion transition invalid");
        if matches!(
            outcome.disposition,
            RestoreDisposition::Spawned | RestoreDisposition::Replaced
        ) {
            let expected = match declaration.spawn_component {
                SpawnComponent::Missing => ComponentDisposition::Created,
                SpawnComponent::DeclaredKind => ComponentDisposition::Existing,
            };
            ensure!(
                outcome.component_disposition == expected,
                "native lifecycle completion spawn component invalid"
            );
        }
    }
    Ok(())
}

pub fn validate_lifecycle_relationships(
    native: &NativeWorldSnapshot,
    lifecycle: &LifecycleSnapshot,
    relationships: Option<&NativeRelationshipsSnapshot>,
) -> Result<()> {
    if let Some(relationships) = relationships {
        validate_relationship_capacities(relationships)?;
        CompiledNativeRelationships::from_snapshot(relationships, native)?;
        for edge in &relationships.foundations {
            let owner = slot_by_id(lifecycle, edge.owner)?;
            if owner.presence == Presence::Absent {
                ensure!(
                    edge.foundation.is_none(),
                    "native lifecycle absent deployable has edge"
                );
            }
            if let Some(target) = edge.foundation {
                ensure!(
                    owner.presence == Presence::Live
                        && slot_by_id(lifecycle, target)?.presence == Presence::Live,
                    "native lifecycle relationship endpoint absent"
                );
            }
        }
    }
    Ok(())
}

pub struct PreparedLifecycleWorld {
    snapshot: WorldSnapshot,
}
impl PreparedLifecycleWorld {
    pub fn snapshot(&self) -> &WorldSnapshot {
        &self.snapshot
    }
    pub fn into_snapshot(self) -> WorldSnapshot {
        self.snapshot
    }
}
pub fn prepare_lifecycle_world(
    mut base: WorldSnapshot,
    compiled: &CompiledNativeLifecycle,
) -> Result<PreparedLifecycleWorld> {
    ensure!(
        matches!(
            base.schema_version,
            SNAPSHOT_SCHEMA_NATIVE | SNAPSHOT_SCHEMA_RELATIONSHIPS
        ),
        "native lifecycle requires schema 4 or 6 base"
    );
    ensure!(
        base.native_import.is_none(),
        "native lifecycle cannot accept import metadata"
    );
    validate_native_bounds(
        base.native_world
            .as_ref()
            .context("native lifecycle spatial state missing")?,
    )?;
    if let Some(relationships) = &base.native_relationships {
        validate_relationship_capacities(relationships)?;
    }
    base.validate()?;
    let lifecycle = compiled.initial_snapshot();
    let native = base
        .native_world
        .as_ref()
        .context("native lifecycle spatial state missing")?;
    compiled.validate_snapshot(&lifecycle, native)?;
    validate_lifecycle_relationships(native, &lifecycle, base.native_relationships.as_ref())?;
    base.schema_version = SNAPSHOT_SCHEMA_LIFECYCLE;
    base.native_lifecycle = Some(lifecycle);
    base.validate()?;
    SnapshotStore::preflight(&base)?;
    Ok(PreparedLifecycleWorld { snapshot: base })
}
pub fn publish_new_lifecycle_world(
    prepared: PreparedLifecycleWorld,
    destination: &Path,
) -> Result<SaveReceipt> {
    SnapshotStore::open(destination)?.save_new(prepared.snapshot())
}

fn validate_variables(
    variables: &Vec<NativeVariableValue>,
    values: &mut usize,
    bytes: &mut usize,
) -> Result<()> {
    ensure!(
        variables.capacity() <= MAX_NATIVE_VARIABLES_PER_CLASS,
        "native lifecycle variable count limit"
    );
    add_count(values, variables.capacity(), MAX_NATIVE_VARIABLE_VALUES)?;
    let mut keys = BTreeSet::new();
    for variable in variables {
        key_capacity(&variable.key)?;
        ensure!(
            keys.insert(variable.key.as_str()),
            "native lifecycle variable key invalid or duplicate"
        );
        let capacity_bytes = validate_value_capacity(&variable.value)? + variable.key.capacity();
        add_count(
            bytes,
            encoded_size(variable, MAX_NATIVE_VARIABLE_BYTES)?.max(capacity_bytes),
            MAX_NATIVE_VARIABLE_BYTES,
        )?;
    }
    Ok(())
}
impl RestoreBatch {
    pub fn parse(text: &str) -> Result<Self> {
        let batch: Self = parse_document(text, MAX_RESTORE_BATCH_BYTES)?;
        batch.validate_bounds()?;
        Ok(batch)
    }
    pub fn load(path: &Path) -> Result<Self> {
        Self::parse(&load_document(path, MAX_RESTORE_BATCH_BYTES)?)
    }
    fn validate_bounds(&self) -> Result<()> {
        digest_capacity(&self.lifecycle_id)?;
        ensure!(self.sequence > 0, "native restore header invalid");
        ensure!(
            !self.recorders.is_empty() && self.recorders.capacity() <= MAX_NATIVE_PLACEMENTS,
            "native restore recorder count invalid"
        );
        let mut names = 0;
        let mut values = 0;
        let mut bytes = 0;
        let mut guids = BTreeSet::new();
        let mut targets = BTreeSet::new();
        for recorder in &self.recorders {
            ensure!(
                recorder.guid > 0 && guids.insert(recorder.guid),
                "native restore GUID invalid or duplicate"
            );
            key_capacity(&recorder.target_placement)?;
            ensure!(
                targets.insert(recorder.target_placement.as_str()),
                "native restore target invalid or duplicate"
            );
            validate_name(&recorder.owner_name, &mut names)?;
            recorder.transform.validate()?;
            validate_variables(&recorder.variables, &mut values, &mut bytes)?;
            if let RecorderSource::Detached {
                expected_prior_binding: Some(prior),
            } = &recorder.source
            {
                ensure!(
                    prior.id.0 > 0 && prior.incarnation > 0,
                    "native restore prior handle invalid"
                );
            }
        }
        encoded_size(self, MAX_RESTORE_BATCH_BYTES)?;
        Ok(())
    }
    fn canonical(&self) -> Result<Self> {
        self.validate_bounds()?;
        let mut canonical = self.clone();
        canonical
            .recorders
            .sort_by(|a, b| a.target_placement.cmp(&b.target_placement));
        for recorder in &mut canonical.recorders {
            recorder.variables.sort_by(|a, b| a.key.cmp(&b.key));
        }
        Ok(canonical)
    }
}

pub struct PreparedRestore {
    snapshot: WorldSnapshot,
    already_completed: bool,
}
impl PreparedRestore {
    pub fn snapshot(&self) -> &WorldSnapshot {
        &self.snapshot
    }
    pub fn into_snapshot(self) -> WorldSnapshot {
        self.snapshot
    }
    pub fn already_completed(&self) -> bool {
        self.already_completed
    }
    pub fn completion(&self) -> &CompletedRestore {
        // Construction always installs or validates an existing completion.
        self.snapshot
            .native_lifecycle
            .as_ref()
            .expect("prepared lifecycle")
            .completed
            .as_ref()
            .expect("prepared completion")
    }
}

pub fn prepare_owner_restore(
    base: &WorldSnapshot,
    batch: &RestoreBatch,
) -> Result<PreparedRestore> {
    // Validate the entire current state even for an exact retry. No expectations
    // or request payload can hide corrupt unrelated slots or relationship edges.
    base.validate()?;
    ensure!(
        base.schema_version == SNAPSHOT_SCHEMA_LIFECYCLE && base.native_import.is_none(),
        "native restore requires schema 7 without import metadata"
    );
    ensure!(
        base.entities.capacity() <= MAX_NATIVE_PLACEMENTS,
        "native restore legacy entity capacity limit"
    );
    let lifecycle = base
        .native_lifecycle
        .as_ref()
        .context("native restore lifecycle missing")?;
    let native = base
        .native_world
        .as_ref()
        .context("native restore spatial state missing")?;
    let compiled = CompiledNativeLifecycle::from_snapshot(lifecycle, native)?;
    validate_lifecycle_relationships(native, lifecycle, base.native_relationships.as_ref())?;
    SnapshotStore::preflight(base)?;
    let canonical = batch.canonical()?;
    ensure!(
        canonical.lifecycle_id == lifecycle.lifecycle_id,
        "native restore lifecycle identity mismatch"
    );
    let request_digest = digest(&canonical, MAX_RESTORE_BATCH_BYTES)?;
    if let Some(completed) = &lifecycle.completed {
        if canonical.sequence == completed.sequence {
            ensure!(
                request_digest == completed.request_digest,
                "native restore sequence conflict"
            );
            return Ok(PreparedRestore {
                snapshot: base.clone(),
                already_completed: true,
            });
        }
        ensure!(
            completed.sequence.checked_add(1) == Some(canonical.sequence),
            "native restore sequence invalid or exhausted"
        );
    } else {
        ensure!(
            canonical.sequence == 1,
            "native restore initial sequence must be one"
        );
    }
    // Each command is checked against the same pre-batch state; duplicate
    // targets and GUIDs were rejected before canonicalization or staging.
    let mut snapshot = base.clone();
    let staged_lifecycle = snapshot
        .native_lifecycle
        .as_mut()
        .context("native restore lifecycle missing")?;
    let staged_native = snapshot
        .native_world
        .as_mut()
        .context("native restore spatial state missing")?;
    let mut outcomes = Vec::with_capacity(canonical.recorders.len());
    for command in &canonical.recorders {
        outcomes.push(stage_recorder(
            &compiled,
            lifecycle,
            native,
            staged_lifecycle,
            staged_native,
            command,
        )?);
    }
    sort_bindings(&mut staged_lifecycle.bindings);
    staged_lifecycle.restore_sequence = canonical.sequence;
    staged_lifecycle.completed = Some(CompletedRestore {
        sequence: canonical.sequence,
        request_digest,
        outcomes,
    });
    compiled.validate_snapshot(staged_lifecycle, staged_native)?;
    validate_lifecycle_relationships(
        staged_native,
        staged_lifecycle,
        snapshot.native_relationships.as_ref(),
    )?;
    snapshot.validate()?;
    SnapshotStore::preflight(&snapshot)?;
    Ok(PreparedRestore {
        snapshot,
        already_completed: false,
    })
}

fn evaluator_policy(policy: ResolvePolicy) -> OwnerResolvePolicy {
    match policy {
        ResolvePolicy::FindOnly => OwnerResolvePolicy::FindOnly,
        ResolvePolicy::FindOrRespawn => OwnerResolvePolicy::FindOrRespawn,
        ResolvePolicy::RespawnOnly => OwnerResolvePolicy::RespawnOnly,
    }
}

/// Derive both attachment evaluators' inputs from admitted private facts.
fn attach_component(
    component_kind: Option<RecorderKind>,
    command: &RestoreRecorder,
    spawning: bool,
) -> Result<ComponentDisposition> {
    let present = component_kind.is_some();
    let matching = component_kind == Some(command.kind);
    let attachment = evaluate_attach_recorder_to_actor(
        present,
        command.create_if_missing,
        matching,
        false,
        !spawning,
    );
    let created = match attachment {
        AttachRecorderOutcome::RestoredAndUpdatedOwner {
            created_component, ..
        } => created_component,
        _ => anyhow::bail!("native restore component attachment rejected"),
    };
    let resolution = evaluate_resolve_owner_recorder_state(&ResolveOwnerRecorderStateInputs {
        deserialized_recorder_present: true,
        deserialized_recorder_pending_kill: false,
        owner_actor_present: true,
        owner_actor_pending_kill: false,
        owner_has_matching_component: matching,
        b_can_dynamically_attach_to_owner: command.create_if_missing,
        dynamic_component_created: created,
        registered_subsystem_uid: None,
    });
    ensure!(
        matches!(resolution, ResolveOwnerRecorderStateOutcome::AttachedAndUpdated {
        created_dynamic_component, transferred_subsystem_uid: None
    } if created_dynamic_component == created),
        "native restore owner recorder resolution rejected"
    );
    Ok(if created {
        ComponentDisposition::Created
    } else {
        ComponentDisposition::Existing
    })
}

fn stage_recorder(
    compiled: &CompiledNativeLifecycle,
    before: &LifecycleSnapshot,
    native: &NativeWorldSnapshot,
    staged: &mut LifecycleSnapshot,
    staged_native: &mut NativeWorldSnapshot,
    command: &RestoreRecorder,
) -> Result<CompletedRecorder> {
    let rank = compiled
        .definition
        .slots
        .binary_search_by(|slot| slot.placement.cmp(&command.target_placement))
        .ok()
        .context("native restore target undeclared")?;
    let declaration = &compiled.definition.slots[rank];
    let slot = &before.slots[rank];
    ensure!(
        slot.presence == command.expected_presence
            && slot.incarnation == command.expected_incarnation,
        "native restore stale owner expectation"
    );
    ensure!(
        declaration.recorder_kind == command.kind,
        "native restore recorder kind mismatch"
    );
    let binding = before
        .bindings
        .iter()
        .find(|binding| binding.guid == command.guid);
    let current = native
        .actors
        .iter()
        .find(|actor| actor.id == slot.id)
        .context("native restore spatial actor missing")?;
    let (disposition, component_disposition, incarnation) = match &command.source {
        RecorderSource::Bound => {
            let bound = binding.context("native restore bound GUID missing")?;
            // The snapshot validator already establishes the component/binding
            // bijection. Bound resolution uses GUID and handle, never the name.
            ensure!(
                bound.kind == command.kind
                    && bound.owner.id == slot.id
                    && bound.owner.incarnation == slot.incarnation
                    && slot.presence == Presence::Live,
                "native restore bound owner mismatch"
            );
            let has_valid_owner =
                slot.presence == Presence::Live && bound.owner.incarnation == slot.incarnation;
            let mut candidates = before.slots.iter().zip(&compiled.definition.slots).filter(
                |(candidate, declaration)| {
                    candidate.presence == Presence::Live
                        && does_actor_recorder_match_actor(
                            has_valid_owner,
                            command.guid,
                            &command.owner_name,
                            candidate.component.as_ref().map(|component| component.guid),
                            &declaration.owner_name,
                        )
                },
            );
            ensure!(
                candidates
                    .next()
                    .is_some_and(|(candidate, _)| candidate.id == slot.id)
                    && candidates.next().is_none(),
                "native restore bound GUID candidate invalid"
            );
            ensure!(
                current.transform == command.transform
                    && current.variables.len() == command.variables.len()
                    && command
                        .variables
                        .iter()
                        .all(|value| current.variables.iter().any(|existing| existing == value)),
                "native restore bound state differs"
            );
            ensure!(
                evaluate_owner_resolve_step(
                    has_valid_owner,
                    evaluator_policy(command.policy),
                    false,
                    true,
                    false,
                    false
                ) == OwnerResolveStep::KeptAlreadyBoundOwner,
                "native restore bound owner rejected"
            );
            (
                RestoreDisposition::KeptBound,
                ComponentDisposition::Existing,
                slot.incarnation,
            )
        }
        RecorderSource::Detached {
            expected_prior_binding,
        } => {
            // Search all declarations, including absent slots. An explicit
            // target does not disambiguate otherwise ambiguous owner names.
            let mut matches =
                compiled
                    .definition
                    .slots
                    .iter()
                    .enumerate()
                    .filter(|(index, candidate)| {
                        does_actor_recorder_match_actor(
                            matches!(&command.source, RecorderSource::Bound),
                            command.guid,
                            &command.owner_name,
                            before.slots[*index]
                                .component
                                .as_ref()
                                .map(|component| component.guid),
                            &candidate.owner_name,
                        )
                    });
            ensure!(
                matches.next().is_some_and(|(index, _)| index == rank) && matches.next().is_none(),
                "native restore owner name unresolved or ambiguous"
            );
            match (expected_prior_binding, binding) {
                (None, None) => {}
                (Some(expected), Some(actual)) => ensure!(
                    expected == &actual.owner && expected.id == slot.id,
                    "native restore prior binding mismatch"
                ),
                _ => anyhow::bail!("native restore detached binding expectation mismatch"),
            }
            let live = slot.presence == Presence::Live;
            let spawning = !live || command.policy == ResolvePolicy::RespawnOnly;
            ensure!(
                live || command.policy != ResolvePolicy::FindOnly,
                "native restore find-only owner absent"
            );
            let component_kind = if spawning {
                match declaration.spawn_component {
                    SpawnComponent::Missing => None,
                    SpawnComponent::DeclaredKind => Some(declaration.recorder_kind),
                }
            } else {
                slot.component.as_ref().map(|component| component.kind)
            };
            // Failure on a live FindOrRespawn attachment is terminal here;
            // never feed it to the evaluator as permission to respawn.
            let component_disposition = attach_component(component_kind, command, spawning)?;
            let step = evaluate_owner_resolve_step(
                false,
                evaluator_policy(command.policy),
                false,
                live,
                !spawning,
                spawning,
            );
            let disposition = match step {
                OwnerResolveStep::AttachedToFoundOwner if !spawning => RestoreDisposition::Attached,
                OwnerResolveStep::RespawnedAndAttached {
                    destroyed_preexisting_owner,
                } if spawning => {
                    ensure!(
                        destroyed_preexisting_owner == live,
                        "native restore replacement decision invalid"
                    );
                    if live {
                        RestoreDisposition::Replaced
                    } else {
                        RestoreDisposition::Spawned
                    }
                }
                _ => anyhow::bail!("native restore owner resolution dropped"),
            };
            let incarnation = if spawning {
                slot.incarnation
                    .checked_add(1)
                    .context("native restore incarnation exhausted")?
            } else {
                slot.incarnation
            };
            (disposition, component_disposition, incarnation)
        }
    };
    let owner = OwnerHandle {
        id: slot.id,
        incarnation,
    };
    if disposition != RestoreDisposition::KeptBound {
        let actor = staged_native
            .actors
            .iter_mut()
            .find(|actor| actor.id == slot.id)
            .context("native restore staged actor missing")?;
        if matches!(
            disposition,
            RestoreDisposition::Spawned | RestoreDisposition::Replaced
        ) {
            // Start with the catalog placement recipe, never dormant state.
            let placement = &compiled
                .catalog
                .map(&compiled.definition.map)
                .context("native restore recipe map missing")?
                .placements[rank];
            let class = compiled
                .catalog
                .class(&placement.class)
                .context("native restore recipe class missing")?;
            *actor = NativeActorRecord {
                id: slot.id,
                placement: placement.key.clone(),
                class: placement.class.clone(),
                tile: placement.tile.clone(),
                transform: placement.transform.clone(),
                variables: class
                    .variables
                    .iter()
                    .map(|variable| NativeVariableValue {
                        key: variable.key.clone(),
                        value: placement
                            .initial_values
                            .iter()
                            .find(|v| v.key == variable.key)
                            .map_or_else(|| variable.default.clone(), |v| v.value.clone()),
                    })
                    .collect(),
            };
        }
        actor.transform = command.transform.clone();
        actor.variables = command.variables.clone();
        staged.slots[rank] = SlotRecord {
            id: slot.id,
            presence: Presence::Live,
            incarnation,
            component: Some(RecorderComponentRecord {
                kind: command.kind,
                guid: command.guid,
            }),
        };
        // Retire the incumbent atomically with installation of the new binding.
        staged
            .bindings
            .retain(|binding| binding.owner.id != slot.id);
        staged.bindings.push(RecorderBinding {
            guid: command.guid,
            kind: command.kind,
            owner,
        });
    }
    Ok(CompletedRecorder {
        guid: command.guid,
        target: slot.id,
        policy: command.policy,
        source: command.source.clone(),
        before_presence: slot.presence,
        before_incarnation: slot.incarnation,
        owner,
        disposition,
        component_disposition,
    })
}

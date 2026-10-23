//! Strict, independently authored C001 owner imports. Source poses are immutable
//! provenance; detached lifecycle restores alone decide initial owner outcomes.
use crate::{
    experimental::{
        archive::{ByteOrder, decode_string, encode_string},
        component::MAX_BOUNDED_COMPONENT_BYTES,
        recorders::{
            StagedRecorderDecision, StagedRootValues, read_recorders,
            stage_prospect_save_state_with_root_values,
        },
        restore_rules::OwnerResolvePolicy,
        tagged::{TagProfile, read_root},
    },
    import_candidate::DecodedImportCandidate,
    native_import::{
        AuthoredCoordinatesV1, C001RootPresence, SemanticImportCategory, SemanticImportError,
        c001_source::{
            C001SourceProfile, SourceOwner, source_owner, validate_root, validate_source_headers,
            validate_wrapper,
        },
    },
    native_lifecycle::{
        CompiledNativeLifecycle, CompletedRestore, LifecycleSnapshot,
        MAX_LIFECYCLE_NAME_TOTAL_UNITS, OwnerHandle, Presence, RecorderKind, RecorderSource,
        ResolvePolicy, RestoreBatch, RestoreDisposition, RestoreRecorder, prepare_owner_restore,
        validate_native_bounds, validate_relationship_capacities, validate_snapshot_bounds,
    },
    native_world::{
        CompiledNativeCatalog, MAX_NATIVE_KEY_BYTES, MAX_NATIVE_PLACEMENTS,
        MAX_NATIVE_STRING_UNITS, NativeClassKind, NativeWorldSnapshot, prepare_authored_world,
    },
    persistence::{SaveReceipt, SnapshotStore},
    preservation::PreservedSection,
    simulation::{DomainId, SNAPSHOT_SCHEMA_C001_OWNERS, SNAPSHOT_SCHEMA_LIFECYCLE, WorldSnapshot},
};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Deserializer, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    fs::OpenOptions,
    io::{Read, Write},
    os::unix::fs::OpenOptionsExt,
    path::Path,
    sync::Arc,
};

pub const C001_OWNER_IMPORT_FORMAT: u32 = 1;
pub const MAX_C001_OWNER_BINDINGS_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_C001_OWNER_METADATA_BYTES: usize = 4 * 1024 * 1024;
const PROFILE: TagProfile = TagProfile::MemoryArchive522;
const C001: &str = "/Script/Icarus.ActorStateRecorderComponent";
// Four names per binding plus three descriptive root strings. The allowance
// covers JSON/TOML doubling and minimum-four growth without counting slack as
// logical source identity. Both limits apply before cloning any supplied vector.
const MAX_NAME_CAPACITY_UNITS: usize =
    2 * MAX_LIFECYCLE_NAME_TOTAL_UNITS + 2 * (4 * MAX_NATIVE_PLACEMENTS + 3);

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
                    .map_err(|_| <D::Error as serde::de::Error>::custom("C001 owner object invalid"))?;
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

#[derive(Clone, Copy, PartialEq, Eq, Serialize)]
pub enum C001OwnerAdapter {
    #[serde(rename = "c001_authored_owners_v1")]
    C001AuthoredOwnersV1,
}
impl<'de> Deserialize<'de> for C001OwnerAdapter {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        struct EnumVisitor;
        impl serde::de::Visitor<'_> for EnumVisitor {
            type Value = C001OwnerAdapter;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a C001 owner adapter string")
            }
            fn visit_str<E: serde::de::Error>(
                self,
                value: &str,
            ) -> std::result::Result<Self::Value, E> {
                match value {
                    "c001_authored_owners_v1" => Ok(C001OwnerAdapter::C001AuthoredOwnersV1),
                    _ => Err(E::custom("C001 owner adapter invalid")),
                }
            }
        }
        deserializer
            .deserialize_str(EnumVisitor)
            .map_err(|_| <D::Error as serde::de::Error>::custom("C001 owner adapter invalid"))
    }
}
impl fmt::Debug for C001OwnerAdapter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("C001OwnerAdapter").finish_non_exhaustive()
    }
}
object_dto! { pub struct C001OwnerBindingsV1 {
    pub format: u32, pub adapter: C001OwnerAdapter, pub expected_payload_sha256: String,
    pub native_catalog_id: String, pub native_lifecycle_id: String, pub source_map: Vec<u16>,
    pub native_map: String, pub coordinates: AuthoredCoordinatesV1, pub owners: Vec<C001OwnerBindingV1>,
} }
object_dto! { pub struct C001OwnerBindingV1 {
    pub source_actor_class: Vec<u16>, pub source_actor_path: Vec<u16>, pub source_object_name: Vec<u16>,
    pub source_icarus_guid: i32, pub native_placement: String, pub native_class: String,
    pub native_owner_name: Vec<u16>, pub native_recorder_guid: i32, pub create_if_missing: bool,
    pub expected_presence: Presence, pub expected_incarnation: u64, pub expected_prior_binding: Option<OwnerHandle>,
} }
object_dto! { pub struct ResolvedC001OwnerV1 {
    pub source_index: usize, pub id: DomainId, pub binding: C001OwnerBindingV1, pub source_policy: ResolvePolicy,
} }
object_dto! { pub struct C001OwnerImportMetadataV1 {
    pub format: u32, pub adapter: C001OwnerAdapter, pub source_version: i32, pub source_payload_sha256: String,
    pub native_catalog_id: String, pub native_lifecycle_id: String, pub native_map: String,
    pub coordinates: AuthoredCoordinatesV1, pub source_map: Vec<u16>, pub source_prospect_id: Vec<u16>,
    pub source_lobby_name: Vec<u16>, pub source_last_saved_date_time: Option<i64>, pub root_presence: C001RootPresence,
    pub owners: Vec<ResolvedC001OwnerV1>, pub initial_completion: CompletedRestore,
} }

fn encoded_size(value: &impl Serialize, limit: usize) -> Result<usize> {
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
                .ok_or_else(|| std::io::Error::other("C001 owner byte limit"))?;
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut writer = Counter { bytes: 0, limit };
    serde_json::to_writer(&mut writer, value)
        .map_err(|_| anyhow::anyhow!("C001 owner serialization or byte limit"))?;
    Ok(writer.bytes)
}
fn add(total: &mut usize, amount: usize, limit: usize) -> Result<()> {
    *total = total
        .checked_add(amount)
        .filter(|n| *n <= limit)
        .context("C001 owner allocation limit")?;
    Ok(())
}
#[derive(Default)]
struct Names {
    logical: usize,
    capacity: usize,
}
impl Names {
    fn check(&mut self, value: &Vec<u16>, nonempty: bool) -> Result<()> {
        ensure!(
            (!nonempty || !value.is_empty())
                && value.len() <= MAX_NATIVE_STRING_UNITS
                && value.capacity() <= 2 * MAX_NATIVE_STRING_UNITS + 2,
            "C001 owner name limit"
        );
        add(
            &mut self.logical,
            value.len(),
            MAX_LIFECYCLE_NAME_TOTAL_UNITS,
        )?;
        add(
            &mut self.capacity,
            value.capacity(),
            MAX_NAME_CAPACITY_UNITS,
        )
    }
}
fn key(value: &String) -> Result<()> {
    ensure!(
        !value.is_empty()
            && value.capacity() <= MAX_NATIVE_KEY_BYTES
            && value
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b)),
        "C001 owner native key invalid"
    );
    Ok(())
}
fn digest(value: &String) -> Result<()> {
    ensure!(
        value.len() == 64
            && value.capacity() <= 64
            && value
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
        "C001 owner digest invalid"
    );
    Ok(())
}
fn binding_bounds(binding: &C001OwnerBindingV1, names: &mut Names) -> Result<()> {
    names.check(&binding.source_actor_class, true)?;
    names.check(&binding.source_actor_path, true)?;
    names.check(&binding.source_object_name, true)?;
    names.check(&binding.native_owner_name, true)?;
    key(&binding.native_placement)?;
    key(&binding.native_class)?;
    ensure!(
        binding.source_icarus_guid > 0 && binding.native_recorder_guid > 0,
        "C001 owner GUID invalid"
    );
    ensure!(
        match binding.expected_presence {
            Presence::Absent => binding.expected_incarnation == 0,
            Presence::Live => binding.expected_incarnation == 1,
        },
        "C001 owner initial expectation invalid"
    );
    if let Some(prior) = binding.expected_prior_binding {
        ensure!(
            prior.id.0 > 0 && prior.id.0 <= MAX_NATIVE_PLACEMENTS as u64 && prior.incarnation == 1,
            "C001 owner prior binding invalid"
        );
    }
    Ok(())
}
fn unique_bindings<'a>(owners: impl Iterator<Item = &'a C001OwnerBindingV1>) -> Result<()> {
    let (mut paths, mut source_guids, mut placements, mut native_guids) = (
        BTreeSet::new(),
        BTreeSet::new(),
        BTreeSet::new(),
        BTreeSet::new(),
    );
    for binding in owners {
        ensure!(
            paths.insert(&binding.source_actor_path)
                && source_guids.insert(binding.source_icarus_guid)
                && placements.insert(&binding.native_placement)
                && native_guids.insert(binding.native_recorder_guid),
            "C001 owner duplicate binding"
        );
    }
    Ok(())
}

#[derive(Clone)]
pub struct CompiledC001OwnerBindings {
    definition: C001OwnerBindingsV1,
}
impl fmt::Debug for CompiledC001OwnerBindings {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CompiledC001OwnerBindings")
            .finish_non_exhaustive()
    }
}
impl CompiledC001OwnerBindings {
    pub fn compile(definition: C001OwnerBindingsV1) -> Result<Self> {
        ensure!(
            definition.format == C001_OWNER_IMPORT_FORMAT,
            "C001 owner binding format invalid"
        );
        digest(&definition.expected_payload_sha256)?;
        digest(&definition.native_catalog_id)?;
        digest(&definition.native_lifecycle_id)?;
        key(&definition.native_map)?;
        ensure!(
            !definition.owners.is_empty() && definition.owners.capacity() <= MAX_NATIVE_PLACEMENTS,
            "C001 owner binding count limit"
        );
        let mut names = Names::default();
        names.check(&definition.source_map, true)?;
        for binding in &definition.owners {
            binding_bounds(binding, &mut names)?;
        }
        encoded_size(&definition, MAX_C001_OWNER_BINDINGS_BYTES)?;
        unique_bindings(definition.owners.iter())?;
        Ok(Self { definition })
    }
    pub fn parse(text: &str) -> Result<Self> {
        ensure!(
            text.len() <= MAX_C001_OWNER_BINDINGS_BYTES,
            "C001 owner binding input limit"
        );
        let definition = if text.trim_start().starts_with('{') {
            serde_json::from_str(text)
                .map_err(|_| anyhow::anyhow!("C001 owner binding JSON invalid"))?
        } else {
            toml::from_str(text).map_err(|_| anyhow::anyhow!("C001 owner binding TOML invalid"))?
        };
        Self::compile(definition)
    }
    pub fn load(path: &Path) -> Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(crate::nonblocking_flag())
            .open(path)
            .map_err(|e| anyhow::anyhow!("C001 owner binding open: {:?}", e.kind()))?;
        let metadata = file
            .metadata()
            .map_err(|e| anyhow::anyhow!("C001 owner binding metadata: {:?}", e.kind()))?;
        ensure!(
            metadata.is_file(),
            "C001 owner bindings require regular file"
        );
        ensure!(
            metadata.len() <= MAX_C001_OWNER_BINDINGS_BYTES as u64,
            "C001 owner binding input limit"
        );
        let mut bytes = Vec::new();
        file.take(MAX_C001_OWNER_BINDINGS_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| anyhow::anyhow!("C001 owner binding read: {:?}", e.kind()))?;
        ensure!(
            bytes.len() <= MAX_C001_OWNER_BINDINGS_BYTES,
            "C001 owner binding input limit"
        );
        Self::parse(
            std::str::from_utf8(&bytes)
                .map_err(|_| anyhow::anyhow!("C001 owner binding encoding invalid"))?,
        )
    }
    pub fn definition(&self) -> &C001OwnerBindingsV1 {
        &self.definition
    }
}

fn native_policy(policy: OwnerResolvePolicy) -> Result<ResolvePolicy> {
    match policy {
        OwnerResolvePolicy::FindOnly => Ok(ResolvePolicy::FindOnly),
        OwnerResolvePolicy::FindOrRespawn => Ok(ResolvePolicy::FindOrRespawn),
        OwnerResolvePolicy::RespawnOnly => Ok(ResolvePolicy::RespawnOnly),
        OwnerResolvePolicy::ManuallyResolved => {
            anyhow::bail!("C001 owner manual policy unsupported")
        }
    }
}
fn decision_policy(
    decision: &StagedRecorderDecision,
    source: &SourceOwner,
) -> Result<ResolvePolicy> {
    ensure!(
        decision.retained
            && !decision.reset_rocket_spawn
            && !decision.physics_dynamic_tree
            && !decision.skip_default_owner_resolution
            && !decision.requires_stitching
            && C001SourceProfile::AuthoredOwnersV1.admits(decision.owner_resolve_policy)
            && source.effective_policy() == decision.owner_resolve_policy,
        "C001 owner staged decision unsupported"
    );
    native_policy(source.effective_policy())
}
fn source_matches(source: &SourceOwner, binding: &C001OwnerBindingV1) -> bool {
    source.actor_class == binding.source_actor_class
        && source.actor_path == binding.source_actor_path
        && source.object_name == binding.source_object_name
        && source.guid == binding.source_icarus_guid
}
fn recorder(source: SourceOwner, owner: &ResolvedC001OwnerV1) -> RestoreRecorder {
    let binding = &owner.binding;
    RestoreRecorder {
        guid: binding.native_recorder_guid,
        kind: RecorderKind::SpatialStateV1,
        target_placement: binding.native_placement.clone(),
        owner_name: binding.native_owner_name.clone(),
        policy: owner.source_policy,
        source: RecorderSource::Detached {
            expected_prior_binding: binding.expected_prior_binding,
        },
        create_if_missing: binding.create_if_missing,
        expected_presence: binding.expected_presence,
        expected_incarnation: binding.expected_incarnation,
        transform: source.transform,
        variables: vec![],
    }
}
fn exact(units: &[u16], text: &str) -> bool {
    units.iter().copied().eq(text.encode_utf16())
}

impl C001OwnerImportMetadataV1 {
    pub(crate) fn validate_bounds(&self) -> Result<()> {
        ensure!(
            self.format == C001_OWNER_IMPORT_FORMAT && self.source_version == 2,
            "C001 owner metadata format invalid"
        );
        digest(&self.source_payload_sha256)?;
        digest(&self.native_catalog_id)?;
        digest(&self.native_lifecycle_id)?;
        digest(&self.initial_completion.request_digest)?;
        key(&self.native_map)?;
        ensure!(
            !self.owners.is_empty()
                && self.owners.capacity() <= MAX_NATIVE_PLACEMENTS
                && self.initial_completion.outcomes.capacity() <= MAX_NATIVE_PLACEMENTS,
            "C001 owner metadata count limit"
        );
        let mut names = Names::default();
        names.check(&self.source_map, true)?;
        names.check(&self.source_prospect_id, true)?;
        names.check(&self.source_lobby_name, false)?;
        for owner in &self.owners {
            binding_bounds(&owner.binding, &mut names)?;
        }
        encoded_size(self, MAX_C001_OWNER_METADATA_BYTES)?;
        Ok(())
    }
    fn root_matches(&self, root: &StagedRootValues) -> bool {
        self.source_version == root.loaded_version().unwrap_or_default()
            && self.source_map == root.prospect_map_name()
            && self.source_prospect_id == root.prospect_info().prospect_id
            && self.source_lobby_name == root.prospect_info().lobby_name
            && self.source_last_saved_date_time == root.last_saved_date_time()
            && self.root_presence == C001RootPresence::from(root.source_presence())
    }
    /// Full storage-boundary validation. Replay is detached from the loaded
    /// world and uses only an initial schema-7 world, preventing recursion.
    pub fn validate(
        &self,
        native: &NativeWorldSnapshot,
        lifecycle: &LifecycleSnapshot,
        preserved: &PreservedSection,
    ) -> Result<()> {
        self.validate_bounds()?;
        validate_native_bounds(native)?;
        validate_snapshot_bounds(lifecycle)?;
        let compiled = CompiledNativeLifecycle::from_snapshot(lifecycle, native)?;
        self.validate_declarations(native, lifecycle)?;
        unique_bindings(self.owners.iter().map(|o| &o.binding))?;
        ensure!(
            self.source_payload_sha256 == crate::hex_lower(&preserved.payload_sha256()),
            "C001 owner preserved digest mismatch"
        );
        let payload = preserved
            .decode()
            .and_then(|tree| tree.to_inner_payload())
            .map_err(|_| anyhow::anyhow!("C001 owner preserved reconstruction invalid"))?;
        let roots = read_root(&payload, PROFILE)
            .map_err(|_| anyhow::anyhow!("C001 owner source root invalid"))?;
        let (_, root, decisions) = stage_prospect_save_state_with_root_values(
            &roots.tags,
            PROFILE,
            MAX_BOUNDED_COMPONENT_BYTES,
            None,
        )
        .map_err(|_| anyhow::anyhow!("C001 owner source staging invalid"))?;
        validate_root(&root).map_err(|_| anyhow::anyhow!("C001 owner source root unsupported"))?;
        validate_source_headers(&payload)
            .map_err(|_| anyhow::anyhow!("C001 owner source headers invalid"))?;
        ensure!(
            self.root_matches(&root),
            "C001 owner immutable root mismatch"
        );
        let recorders = read_recorders(&roots.tags, PROFILE)
            .map_err(|_| anyhow::anyhow!("C001 owner source recorders invalid"))?;
        ensure!(
            recorders.records().len() == self.owners.len() && decisions.len() == self.owners.len(),
            "C001 owner source coverage mismatch"
        );
        let by_index: BTreeMap<_, _> = self.owners.iter().map(|o| (o.source_index, o)).collect();
        ensure!(
            by_index.len() == self.owners.len()
                && by_index.keys().copied().eq(0..self.owners.len()),
            "C001 owner source ordinals invalid"
        );
        let mut commands = Vec::with_capacity(self.owners.len());
        for (index, record) in recorders.records().iter().enumerate() {
            let class = decode_string(record.class_value, ByteOrder::Little)
                .map_err(|_| anyhow::anyhow!("C001 owner source class invalid"))?;
            ensure!(
                class.consumed == record.class_value.len()
                    && exact(&class.units, C001)
                    && encode_string(&class.units, ByteOrder::Little)
                        .is_ok_and(|v| v == record.class_value),
                "C001 owner source class unsupported"
            );
            validate_wrapper(record.record, index)
                .map_err(|_| anyhow::anyhow!("C001 owner wrapper unsupported"))?;
            let source = source_owner(
                record.binary_data,
                index,
                C001SourceProfile::AuthoredOwnersV1,
            )
            .map_err(|_| anyhow::anyhow!("C001 owner source owner unsupported"))?;
            let owner = by_index[&index];
            ensure!(
                source_matches(&source, &owner.binding)
                    && decision_policy(&decisions[index], &source)? == owner.source_policy,
                "C001 owner immutable source mismatch"
            );
            commands.push(recorder(source, owner));
        }
        // Rebuild only authored geometry and the declared initial slots. Current
        // relationships and poses are validated independently and never replayed.
        let catalog = Arc::new(CompiledNativeCatalog::compile(native.definitions.clone())?);
        let mut initial = prepare_authored_world(catalog, &self.native_map)?.initial_snapshot();
        initial.schema_version = SNAPSHOT_SCHEMA_LIFECYCLE;
        initial.native_lifecycle = Some(compiled.initial_snapshot());
        let replay = prepare_owner_restore(
            &initial,
            &RestoreBatch {
                lifecycle_id: self.native_lifecycle_id.clone(),
                sequence: 1,
                recorders: commands,
            },
        )
        .map_err(|_| anyhow::anyhow!("C001 owner initial replay invalid"))?;
        ensure!(
            replay.completion() == &self.initial_completion,
            "C001 owner initial receipt mismatch"
        );
        self.validate_current_state(native, lifecycle)
    }
    // This check has no source authority: callers have already validated the
    // pinned catalog/lifecycle. It neither recompiles nor hashes declarations.
    pub(crate) fn validate_current_state(
        &self,
        native: &NativeWorldSnapshot,
        lifecycle: &LifecycleSnapshot,
    ) -> Result<()> {
        ensure!(
            self.native_catalog_id == native.catalog_id
                && self.native_lifecycle_id == lifecycle.lifecycle_id
                && self.native_map == native.map
                && lifecycle.restore_sequence >= 1,
            "C001 owner current identity or sequence invalid"
        );
        let completed = lifecycle
            .completed
            .as_ref()
            .context("C001 owner completion missing")?;
        ensure!(
            completed.sequence == lifecycle.restore_sequence,
            "C001 owner completion sequence invalid"
        );
        if lifecycle.restore_sequence == 1 {
            ensure!(
                completed == &self.initial_completion,
                "C001 owner initial completion changed"
            );
        }
        ensure!(
            self.initial_completion.sequence == 1
                && self.initial_completion.outcomes.len() == self.owners.len()
                && lifecycle.slots.len() == self.owners.len()
                && native.actors.len() == self.owners.len(),
            "C001 owner current coverage mismatch"
        );
        let floors: BTreeMap<_, _> = self
            .initial_completion
            .outcomes
            .iter()
            .map(|o| (o.target, o.owner.incarnation))
            .collect();
        let actors: BTreeMap<_, _> = native.actors.iter().map(|a| (a.id, a)).collect();
        for owner in &self.owners {
            let rank = owner
                .id
                .0
                .checked_sub(1)
                .and_then(|r| usize::try_from(r).ok())
                .context("C001 owner slot invalid")?;
            let slot = lifecycle
                .slots
                .get(rank)
                .context("C001 owner slot missing")?;
            let actor = actors.get(&owner.id).context("C001 owner actor missing")?;
            let floor = floors
                .get(&owner.id)
                .context("C001 owner initial outcome missing")?;
            ensure!(
                slot.id == owner.id
                    && slot.presence == Presence::Live
                    // Every admitted recorder finishes attached. V1 supports
                    // retirement by replacement, but never removal without a
                    // successor; lifecycle validation checks its exact binding.
                    && slot.component.is_some()
                    && slot.incarnation >= *floor
                    && actor.placement == owner.binding.native_placement
                    && actor.class == owner.binding.native_class
                    && actor.variables.is_empty(),
                "C001 owner current state invalid"
            );
        }
        Ok(())
    }
    fn validate_declarations(
        &self,
        native: &NativeWorldSnapshot,
        lifecycle: &LifecycleSnapshot,
    ) -> Result<()> {
        ensure!(
            self.native_catalog_id == native.catalog_id
                && self.native_lifecycle_id == lifecycle.lifecycle_id
                && self.native_map == native.map
                && self.native_map == lifecycle.definition.map,
            "C001 owner declaration identity mismatch"
        );
        let map = native
            .definitions
            .maps
            .iter()
            .find(|m| m.key == self.native_map)
            .context("C001 owner map missing")?;
        ensure!(
            map.placements.len() == self.owners.len()
                && lifecycle.slots.len() == self.owners.len()
                && native.actors.len() == self.owners.len(),
            "C001 owner declaration coverage mismatch"
        );
        let placements: BTreeMap<_, _> =
            map.placements.iter().map(|p| (p.key.as_str(), p)).collect();
        let ranks: BTreeMap<_, _> = placements
            .into_iter()
            .enumerate()
            .map(|(i, (k, p))| (k, (i, p)))
            .collect();
        let classes: BTreeMap<_, _> = native
            .definitions
            .classes
            .iter()
            .map(|c| (c.key.as_str(), c))
            .collect();
        let actors: BTreeMap<_, _> = native.actors.iter().map(|a| (a.id, a)).collect();
        let mut ids = BTreeSet::new();
        for owner in &self.owners {
            let b = &owner.binding;
            let (rank, placement) = ranks
                .get(b.native_placement.as_str())
                .context("C001 owner placement missing")?;
            let declaration = lifecycle
                .definition
                .slots
                .get(*rank)
                .context("C001 owner slot declaration missing")?;
            let class = classes
                .get(b.native_class.as_str())
                .context("C001 owner class missing")?;
            let actor = actors.get(&owner.id).context("C001 owner actor missing")?;
            ensure!(
                owner.source_index < self.owners.len()
                    && owner.id == DomainId(*rank as u64 + 1)
                    && ids.insert(owner.id)
                    && declaration.placement == b.native_placement
                    && declaration.owner_name == b.native_owner_name
                    && declaration.recorder_kind == RecorderKind::SpatialStateV1
                    && declaration.initial_presence == b.expected_presence
                    && b.expected_incarnation
                        == u64::from(declaration.initial_presence == Presence::Live)
                    && placement.class == b.native_class
                    && actor.placement == b.native_placement
                    && actor.class == b.native_class
                    && actor.tile == placement.tile
                    && class.kind == NativeClassKind::SpatialObjectV1
                    && class.variables.is_empty()
                    && placement.initial_values.is_empty()
                    && actor.variables.is_empty(),
                "C001 owner declaration mismatch"
            );
        }
        Ok(())
    }
}

/// Counts only; never exposes identities, digests, source values or transforms.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct C001OwnerInspection {
    pub recorders: usize,
    pub bindings: usize,
    pub slots: usize,
    pub live_owners: usize,
    pub attached: usize,
    pub spawned: usize,
    pub replaced: usize,
    pub payload_bytes: usize,
    pub metadata_bytes: usize,
}
pub struct PreparedC001OwnerImport {
    snapshot: WorldSnapshot,
    inspection: C001OwnerInspection,
}
impl PreparedC001OwnerImport {
    pub fn snapshot(&self) -> &WorldSnapshot {
        &self.snapshot
    }
    pub fn inspection(&self) -> C001OwnerInspection {
        self.inspection.clone()
    }
    pub fn into_snapshot(self) -> WorldSnapshot {
        self.snapshot
    }
}
impl fmt::Debug for PreparedC001OwnerImport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PreparedC001OwnerImport")
            .field("inspection", &self.inspection)
            .finish_non_exhaustive()
    }
}
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
fn source_error(error: SemanticImportError) -> SemanticImportError {
    SemanticImportError {
        field: "c001_owners.source",
        ..error
    }
}
pub fn prepare_c001_owner_import(
    candidate: &DecodedImportCandidate,
    base: WorldSnapshot,
    bindings: &CompiledC001OwnerBindings,
) -> std::result::Result<PreparedC001OwnerImport, SemanticImportError> {
    use SemanticImportCategory::{Binding, Limit, Unsupported, Validation};
    let fail = |field, category| error(None, field, category);
    if base.schema_version != SNAPSHOT_SCHEMA_LIFECYCLE
        || base.preserved_prospect.is_some()
        || base.native_import.is_some()
        || base.native_owner_import.is_some()
        || !base.entities.is_empty()
    {
        return Err(fail("c001_owners.base", Unsupported));
    }
    if base.entities.capacity() > MAX_NATIVE_PLACEMENTS {
        return Err(fail("c001_owners.base", Limit));
    }
    let native = base
        .native_world
        .as_ref()
        .ok_or_else(|| fail("c001_owners.native_world", Binding))?;
    let lifecycle = base
        .native_lifecycle
        .as_ref()
        .ok_or_else(|| fail("c001_owners.lifecycle", Binding))?;
    validate_native_bounds(native).map_err(|_| fail("c001_owners.native_world", Limit))?;
    validate_snapshot_bounds(lifecycle).map_err(|_| fail("c001_owners.lifecycle", Limit))?;
    if let Some(relationships) = &base.native_relationships {
        validate_relationship_capacities(relationships)
            .map_err(|_| fail("c001_owners.relationships", Limit))?;
    }
    base.validate()
        .map_err(|_| fail("c001_owners.base", Validation))?;
    if lifecycle.restore_sequence != 0 || lifecycle.completed.is_some() {
        return Err(fail("c001_owners.base", Binding));
    }
    let binding = bindings.definition();
    validate_root(candidate.root_values()).map_err(source_error)?;
    validate_source_headers(candidate.payload()).map_err(source_error)?;
    if binding.expected_payload_sha256 != crate::hex_lower(&candidate.preserved().payload_sha256())
        || binding.native_catalog_id != native.catalog_id
        || binding.native_lifecycle_id != lifecycle.lifecycle_id
        || binding.source_map != candidate.root_values().prospect_map_name()
        || binding.native_map != native.map
    {
        return Err(fail("c001_owners.manifest", Binding));
    }
    // Root and binding vectors share the retained metadata budget. Check the
    // combined logical units and spare allocation before cloning either set.
    let root = candidate.root_values();
    let mut names = Names::default();
    names
        .check(&binding.source_map, true)
        .and_then(|()| names.check(&root.prospect_info().prospect_id, true))
        .and_then(|()| names.check(&root.prospect_info().lobby_name, false))
        .map_err(|_| fail("c001_owners.source_names", Limit))?;
    for owner in &binding.owners {
        binding_bounds(owner, &mut names).map_err(|_| fail("c001_owners.source_names", Limit))?;
    }
    let count = candidate.recorders().len();
    if count == 0 || count > MAX_NATIVE_PLACEMENTS {
        return Err(fail("c001_owners.recorders", Limit));
    }
    if count != binding.owners.len() || count != lifecycle.slots.len() {
        return Err(fail("c001_owners.coverage", Binding));
    }
    let by_path: BTreeMap<_, _> = binding
        .owners
        .iter()
        .map(|b| (b.source_actor_path.as_slice(), b))
        .collect();
    let by_placement: BTreeMap<_, _> = native
        .actors
        .iter()
        .map(|a| (a.placement.as_str(), a.id))
        .collect();
    let (mut paths, mut guids) = (BTreeSet::new(), BTreeSet::new());
    let mut owners = Vec::with_capacity(count);
    let mut commands = Vec::with_capacity(count);
    for (index, record) in candidate.recorders().iter().enumerate() {
        let fail_at = |field, category| error(Some(index), field, category);
        if record.source_index() != index || !exact(record.class_units(), C001) {
            return Err(fail_at("c001_owners.component_class", Unsupported));
        }
        let (_, summary) = record
            .typing()
            .lossless()
            .ok_or_else(|| fail_at("c001_owners.typing", Unsupported))?;
        if summary.uninterpreted != 0 {
            return Err(fail_at("c001_owners.typing", Unsupported));
        }
        validate_wrapper(
            candidate
                .recorder_bytes(index)
                .ok_or_else(|| fail_at("c001_owners.recorder", Validation))?,
            index,
        )
        .map_err(source_error)?;
        let source = source_owner(
            candidate
                .recorder_binary_data(index)
                .ok_or_else(|| fail_at("c001_owners.component", Validation))?,
            index,
            C001SourceProfile::AuthoredOwnersV1,
        )
        .map_err(source_error)?;
        let policy = decision_policy(record.decision(), &source)
            .map_err(|_| fail_at("c001_owners.policy", Unsupported))?;
        if !paths.insert(source.actor_path.clone()) || !guids.insert(source.guid) {
            return Err(fail_at("c001_owners.source_identity", Binding));
        }
        let b = by_path
            .get(source.actor_path.as_slice())
            .ok_or_else(|| fail_at("c001_owners.binding", Binding))?;
        if !source_matches(&source, b) {
            return Err(fail_at("c001_owners.binding", Binding));
        }
        let id = *by_placement
            .get(b.native_placement.as_str())
            .ok_or_else(|| fail_at("c001_owners.placement", Binding))?;
        let owner = ResolvedC001OwnerV1 {
            source_index: index,
            id,
            binding: (*b).clone(),
            source_policy: policy,
        };
        commands.push(recorder(source, &owner));
        owners.push(owner);
    }
    let root = candidate.root_values();
    // Metadata is bounded before either the detached restore or any source copy.
    let mut metadata = C001OwnerImportMetadataV1 {
        format: C001_OWNER_IMPORT_FORMAT,
        adapter: C001OwnerAdapter::C001AuthoredOwnersV1,
        source_version: 2,
        source_payload_sha256: binding.expected_payload_sha256.clone(),
        native_catalog_id: binding.native_catalog_id.clone(),
        native_lifecycle_id: binding.native_lifecycle_id.clone(),
        native_map: binding.native_map.clone(),
        coordinates: binding.coordinates,
        source_map: root.prospect_map_name().to_vec(),
        source_prospect_id: root.prospect_info().prospect_id.clone(),
        source_lobby_name: root.prospect_info().lobby_name.clone(),
        source_last_saved_date_time: root.last_saved_date_time(),
        root_presence: root.source_presence().into(),
        owners,
        initial_completion: CompletedRestore {
            sequence: 1,
            request_digest: "0".repeat(64),
            outcomes: vec![],
        },
    };
    metadata
        .validate_bounds()
        .map_err(|_| fail("c001_owners.metadata", Limit))?;
    metadata
        .validate_declarations(native, lifecycle)
        .map_err(|_| fail("c001_owners.declarations", Binding))?;
    let prepared = prepare_owner_restore(
        &base,
        &RestoreBatch {
            lifecycle_id: binding.native_lifecycle_id.clone(),
            sequence: 1,
            recorders: commands,
        },
    )
    .map_err(|_| fail("c001_owners.restore", Validation))?;
    metadata.initial_completion = prepared.completion().clone();
    let metadata_bytes = encoded_size(&metadata, MAX_C001_OWNER_METADATA_BYTES)
        .map_err(|_| fail("c001_owners.metadata", Limit))?;
    let inspection = C001OwnerInspection {
        recorders: count,
        bindings: count,
        slots: count,
        live_owners: count,
        attached: metadata
            .initial_completion
            .outcomes
            .iter()
            .filter(|o| o.disposition == RestoreDisposition::Attached)
            .count(),
        spawned: metadata
            .initial_completion
            .outcomes
            .iter()
            .filter(|o| o.disposition == RestoreDisposition::Spawned)
            .count(),
        replaced: metadata
            .initial_completion
            .outcomes
            .iter()
            .filter(|o| o.disposition == RestoreDisposition::Replaced)
            .count(),
        payload_bytes: candidate.payload().len(),
        metadata_bytes,
    };
    let source = candidate.preserved();
    let preserved = PreservedSection::from_native_encoding(
        source.native_encoding().to_vec(),
        source.payload_bytes(),
        source.payload_sha256(),
    )
    .map_err(|_| fail("c001_owners.preserved", Validation))?;
    let mut snapshot = prepared.into_snapshot();
    snapshot.schema_version = SNAPSHOT_SCHEMA_C001_OWNERS;
    snapshot.native_owner_import = Some(metadata);
    snapshot.preserved_prospect = Some(Arc::new(preserved));
    SnapshotStore::preflight(&snapshot).map_err(|_| fail("c001_owners.preflight", Validation))?;
    Ok(PreparedC001OwnerImport {
        snapshot,
        inspection,
    })
}
pub fn publish_new_c001_owner_import(
    prepared: PreparedC001OwnerImport,
    destination: &Path,
) -> Result<SaveReceipt> {
    SnapshotStore::open(destination)?.save_new(prepared.snapshot())
}

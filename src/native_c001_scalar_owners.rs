//! Strict authored scalar-owner source adaptation and immutable provenance.
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
    native_c001_owners::{C001OwnerBindingV1, C001OwnerInspection},
    native_import::{
        AuthoredCoordinatesV1, C001RootPresence, SemanticImportCategory, SemanticImportError,
        c001_source::{
            C001SourceProfile, SourceOwner, SourceScalarOwner, source_scalar_owner, validate_root,
            validate_source_headers, validate_wrapper,
        },
    },
    native_lifecycle::{
        CompiledNativeLifecycle, CompletedRestore, LifecycleSnapshot,
        MAX_LIFECYCLE_NAME_TOTAL_UNITS, OwnerHandle, Presence, RecorderKind, RecorderSource,
        ResolvePolicy, RestoreBatch, RestoreDisposition, RestoreRecorder, prepare_owner_restore,
        validate_native_bounds, validate_relationship_capacities, validate_snapshot_bounds,
    },
    native_scalar_owners::{
        CompiledScalarOwnerContracts, ScalarOwnerContractsSnapshotV1, canonical_digest, digest,
        encoded_size, key, load_text,
    },
    native_world::{
        CompiledNativeCatalog, MAX_NATIVE_PLACEMENTS, MAX_NATIVE_STRING_UNITS, NativeValue,
        NativeVariableValue, NativeWorldSnapshot, prepare_authored_world,
    },
    persistence::{SaveReceipt, SnapshotStore},
    preservation::PreservedSection,
    simulation::{
        DomainId, SNAPSHOT_SCHEMA_C001_SCALAR_OWNERS, SNAPSHOT_SCHEMA_LIFECYCLE, WorldSnapshot,
    },
};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Deserializer, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    path::Path,
    sync::Arc,
};
pub const C001_SCALAR_OWNER_IMPORT_FORMAT: u32 = 1;
pub const MAX_C001_SCALAR_OWNER_BINDINGS_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_C001_SCALAR_OWNER_METADATA_BYTES: usize = 4 * 1024 * 1024;
const PROFILE: TagProfile = TagProfile::MemoryArchive522;
const C001: &str = "/Script/Icarus.ActorStateRecorderComponent";
const MAX_NAME_CAPACITY_UNITS: usize =
    2 * MAX_LIFECYCLE_NAME_TOTAL_UNITS + 2 * (6 * MAX_NATIVE_PLACEMENTS + 11);
macro_rules! object_dto {
    (pub struct $name:ident { $($(#[$attr:meta])* pub $field:ident: $ty:ty),* $(,)? }) => {
        #[derive(Clone, PartialEq, Serialize)]
        pub struct $name { $($(#[$attr])* pub $field: $ty),* }
        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
                #[derive(Deserialize)]
                #[serde(deny_unknown_fields)]
                struct Fields { $($(#[$attr])* $field: $ty),* }
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
pub enum C001ScalarOwnerAdapter {
    #[serde(rename = "c001_authored_scalar_owners_v1")]
    C001AuthoredScalarOwnersV1,
}
impl<'de> Deserialize<'de> for C001ScalarOwnerAdapter {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        struct EnumVisitor;
        impl serde::de::Visitor<'_> for EnumVisitor {
            type Value = C001ScalarOwnerAdapter;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a C001 owner adapter string")
            }
            fn visit_str<E: serde::de::Error>(
                self,
                value: &str,
            ) -> std::result::Result<Self::Value, E> {
                match value {
                    "c001_authored_scalar_owners_v1" => {
                        Ok(C001ScalarOwnerAdapter::C001AuthoredScalarOwnersV1)
                    }
                    _ => Err(E::custom("C001 owner adapter invalid")),
                }
            }
        }
        deserializer
            .deserialize_str(EnumVisitor)
            .map_err(|_| <D::Error as serde::de::Error>::custom("C001 owner adapter invalid"))
    }
}
impl fmt::Debug for C001ScalarOwnerAdapter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("C001ScalarOwnerAdapter")
            .finish_non_exhaustive()
    }
}

object_dto! { pub struct C001ScalarArrayV1 { pub array_name:Vec<u16>, pub element_tag_name:Vec<u16>, pub record_type:Vec<u16>, pub value_field:Vec<u16> } }
object_dto! { pub struct C001ScalarLayoutV1 { pub enabled:C001ScalarArrayV1, pub remaining_uses:C001ScalarArrayV1 } }
object_dto! { pub struct C001ScalarOwnerBindingV1 { pub owner:C001OwnerBindingV1, pub enabled_source_name:Vec<u16>, pub remaining_uses_source_name:Vec<u16> } }
object_dto! { pub struct C001ScalarOwnerBindingsV1 {
 pub format:u32, pub adapter:C001ScalarOwnerAdapter, pub expected_payload_sha256:String,
 pub native_catalog_id:String, pub native_lifecycle_id:String, pub native_contract_id:String,
 pub source_map:Vec<u16>, pub native_map:String, #[serde(deserialize_with="coordinates")] pub coordinates:AuthoredCoordinatesV1,
 pub layout:C001ScalarLayoutV1, pub owners:Vec<C001ScalarOwnerBindingV1>
} }
object_dto! { pub struct ResolvedC001ScalarOwnerV1 { pub source_index:u32, pub binding_index:u32, pub id:DomainId, pub source_policy:ResolvePolicy } }
object_dto! { pub struct C001ScalarOwnerImportMetadataV1 {
 pub format:u32, pub bindings_id:String, pub bindings:C001ScalarOwnerBindingsV1,
 pub source_version:i32, pub source_prospect_id:Vec<u16>, pub source_lobby_name:Vec<u16>,
 pub source_last_saved_date_time:Option<i64>, pub root_presence:C001RootPresence,
 pub resolved:Vec<ResolvedC001ScalarOwnerV1>, pub initial_completion:CompletedRestore
} }
// Same wire shape as retained metadata, borrowing every input allocation. Count
// the complete receipt and source metadata before copying them into the result.
#[derive(Serialize)]
struct BorrowedMetadata<'a> {
    format: u32,
    bindings_id: &'a str,
    bindings: &'a C001ScalarOwnerBindingsV1,
    source_version: i32,
    source_prospect_id: &'a [u16],
    source_lobby_name: &'a [u16],
    source_last_saved_date_time: Option<i64>,
    root_presence: C001RootPresence,
    resolved: &'a [ResolvedC001ScalarOwnerV1],
    initial_completion: &'a CompletedRestore,
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
            && C001SourceProfile::AuthoredScalarOwnersV1.admits(decision.owner_resolve_policy)
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
fn exact(units: &[u16], text: &str) -> bool {
    units.iter().copied().eq(text.encode_utf16())
}
fn array_bounds(array: &C001ScalarArrayV1, names: &mut Names) -> Result<()> {
    for name in [
        &array.array_name,
        &array.element_tag_name,
        &array.record_type,
        &array.value_field,
    ] {
        names.check(name, true)?;
    }
    ensure!(
        !exact(&array.value_field, "VariableName"),
        "scalar value declaration collision"
    );
    Ok(())
}
fn layout_bounds(layout: &C001ScalarLayoutV1, names: &mut Names) -> Result<()> {
    array_bounds(&layout.enabled, names)?;
    array_bounds(&layout.remaining_uses, names)?;
    ensure!(
        layout.enabled.array_name != layout.remaining_uses.array_name,
        "scalar array declaration collision"
    );
    for array in [&layout.enabled, &layout.remaining_uses] {
        ensure!(
            ![
                "ActorClassName",
                "ActorPathName",
                "ObjectFName",
                "IcarusActorGUID",
                "RecordedVersion",
                "bWasMovedToSubLevel",
                "ActorTransform",
                "bSkipDefaultOwnerResolution",
                "OwnerResolvePolicy"
            ]
            .iter()
            .any(|name| exact(&array.array_name, name)),
            "scalar array owner-field collision"
        );
    }
    Ok(())
}
fn scalar_binding_bounds(binding: &C001ScalarOwnerBindingV1, names: &mut Names) -> Result<()> {
    binding_bounds(&binding.owner, names)?;
    names.check(&binding.enabled_source_name, true)?;
    names.check(&binding.remaining_uses_source_name, true)?;
    ensure!(
        binding.enabled_source_name != binding.remaining_uses_source_name,
        "scalar source variable collision"
    );
    Ok(())
}
impl C001ScalarOwnerBindingsV1 {
    fn bounds(&self, names: &mut Names) -> Result<()> {
        ensure!(
            self.format == 1
                && !self.owners.is_empty()
                && self.owners.capacity() <= MAX_NATIVE_PLACEMENTS,
            "scalar bindings format or count invalid"
        );
        for d in [
            &self.expected_payload_sha256,
            &self.native_catalog_id,
            &self.native_lifecycle_id,
            &self.native_contract_id,
        ] {
            digest(d)?;
        }
        key(&self.native_map)?;
        names.check(&self.source_map, true)?;
        layout_bounds(&self.layout, names)?;
        for binding in &self.owners {
            scalar_binding_bounds(binding, names)?;
        }
        encoded_size(self, MAX_C001_SCALAR_OWNER_BINDINGS_BYTES)?;
        Ok(())
    }
}
#[derive(Clone, Debug)]
pub struct CompiledC001ScalarLayout {
    definition: C001ScalarLayoutV1,
}
impl CompiledC001ScalarLayout {
    pub fn compile(definition: C001ScalarLayoutV1) -> Result<Self> {
        layout_bounds(&definition, &mut Names::default())?;
        for array in [&definition.enabled, &definition.remaining_uses] {
            for name in [
                &array.array_name,
                &array.element_tag_name,
                &array.record_type,
                &array.value_field,
            ] {
                ensure!(
                    name.len() < crate::experimental::tagged::MAX_HEADER_STRING_UNITS
                        && name.iter().all(|u| *u != 0 && *u != u16::MAX),
                    "scalar layout header name invalid"
                );
            }
        }
        encoded_size(&definition, MAX_C001_SCALAR_OWNER_BINDINGS_BYTES)?;
        Ok(Self { definition })
    }

    pub fn definition(&self) -> &C001ScalarLayoutV1 {
        &self.definition
    }
}
#[derive(Clone, Debug)]
pub struct CompiledC001ScalarOwnerBindings {
    identity: String,
    definition: C001ScalarOwnerBindingsV1,
    layout: CompiledC001ScalarLayout,
}
impl CompiledC001ScalarOwnerBindings {
    pub fn compile(mut definition: C001ScalarOwnerBindingsV1) -> Result<Self> {
        definition.bounds(&mut Names::default())?;
        unique_bindings(definition.owners.iter().map(|b| &b.owner))?;
        definition
            .owners
            .sort_by(|a, b| a.owner.native_placement.cmp(&b.owner.native_placement));
        let identity = canonical_digest(b"icarus/c001_scalar_owner_bindings/v1\0", &definition)?;
        let layout = CompiledC001ScalarLayout {
            definition: definition.layout.clone(),
        };
        Ok(Self {
            identity,
            definition,
            layout,
        })
    }
    pub fn parse(text: &str) -> Result<Self> {
        ensure!(
            text.len() <= MAX_C001_SCALAR_OWNER_BINDINGS_BYTES,
            "scalar bindings input limit"
        );
        let definition = if text.trim_start().starts_with('{') {
            serde_json::from_str(text)
                .map_err(|_| anyhow::anyhow!("scalar bindings JSON invalid"))?
        } else {
            toml::from_str(text).map_err(|_| anyhow::anyhow!("scalar bindings TOML invalid"))?
        };
        Self::compile(definition)
    }
    pub fn load(path: &Path) -> Result<Self> {
        Self::parse(&load_text(path, MAX_C001_SCALAR_OWNER_BINDINGS_BYTES)?)
    }
    pub fn identity(&self) -> &str {
        &self.identity
    }
    pub fn definition(&self) -> &C001ScalarOwnerBindingsV1 {
        &self.definition
    }
}
fn recorder(
    source: SourceScalarOwner,
    binding: &C001ScalarOwnerBindingV1,
    policy: ResolvePolicy,
) -> Result<RestoreRecorder> {
    ensure!(
        source_matches(&source.owner, &binding.owner)
            && source.enabled_source_name == binding.enabled_source_name
            && source.remaining_uses_source_name == binding.remaining_uses_source_name,
        "scalar immutable source identity mismatch"
    );
    let b = &binding.owner;
    Ok(RestoreRecorder {
        guid: b.native_recorder_guid,
        kind: RecorderKind::SpatialStateV1,
        target_placement: b.native_placement.clone(),
        owner_name: b.native_owner_name.clone(),
        policy,
        source: RecorderSource::Detached {
            expected_prior_binding: b.expected_prior_binding,
        },
        create_if_missing: b.create_if_missing,
        expected_presence: b.expected_presence,
        expected_incarnation: b.expected_incarnation,
        transform: source.owner.transform,
        variables: vec![
            NativeVariableValue {
                key: "enabled".into(),
                value: NativeValue::Bool(source.enabled),
            },
            NativeVariableValue {
                key: "remaining_uses".into(),
                value: NativeValue::I32(source.remaining_uses),
            },
        ],
    })
}
impl C001ScalarOwnerImportMetadataV1 {
    pub(crate) fn validate_bounds(&self) -> Result<()> {
        ensure!(
            self.format == 1
                && self.source_version == 2
                && !self.resolved.is_empty()
                && self.resolved.capacity() <= MAX_NATIVE_PLACEMENTS
                && self.initial_completion.outcomes.capacity() <= MAX_NATIVE_PLACEMENTS,
            "scalar metadata format or count invalid"
        );
        digest(&self.bindings_id)?;
        digest(&self.initial_completion.request_digest)?;
        let mut names = Names::default();
        self.bindings.bounds(&mut names)?;
        names.check(&self.source_prospect_id, true)?;
        names.check(&self.source_lobby_name, false)?;
        encoded_size(self, MAX_C001_SCALAR_OWNER_METADATA_BYTES)?;
        Ok(())
    }
    fn root_matches(&self, root: &StagedRootValues) -> bool {
        self.source_version == root.loaded_version().unwrap_or_default()
            && self.bindings.source_map == root.prospect_map_name()
            && self.source_prospect_id == root.prospect_info().prospect_id
            && self.source_lobby_name == root.prospect_info().lobby_name
            && self.source_last_saved_date_time == root.last_saved_date_time()
            && self.root_presence == C001RootPresence::from(root.source_presence())
    }
    fn validate_declarations(
        &self,
        native: &NativeWorldSnapshot,
        lifecycle: &LifecycleSnapshot,
        contracts: &ScalarOwnerContractsSnapshotV1,
    ) -> Result<()> {
        let b = &self.bindings;
        ensure!(
            b.native_catalog_id == native.catalog_id
                && b.native_lifecycle_id == lifecycle.lifecycle_id
                && b.native_contract_id == contracts.contract_id
                && b.native_map == native.map
                && b.native_map == lifecycle.definition.map
                && contracts.definition.native_map == native.map
                && contracts.definition.native_catalog_id == native.catalog_id,
            "scalar declaration identity mismatch"
        );
        let map = native
            .definitions
            .maps
            .iter()
            .find(|m| m.key == native.map)
            .context("scalar declared map missing")?;
        ensure!(
            map.placements.len() == self.resolved.len()
                && lifecycle.definition.slots.len() == self.resolved.len()
                && b.owners.len() == self.resolved.len(),
            "scalar declaration coverage mismatch"
        );
        let mut ids = BTreeSet::new();
        let mut indices = BTreeSet::new();
        let mut sources = BTreeSet::new();
        for resolved in &self.resolved {
            let binding = b
                .owners
                .get(resolved.binding_index as usize)
                .context("scalar binding index invalid")?;
            let owner = &binding.owner;
            let rank = map
                .placements
                .iter()
                .position(|p| p.key == owner.native_placement)
                .context("scalar declared placement missing")?;
            let placement = &map.placements[rank];
            let slot = &lifecycle.definition.slots[rank];
            ensure!(
                resolved.id == DomainId(rank as u64 + 1)
                    && ids.insert(resolved.id)
                    && indices.insert(resolved.binding_index)
                    && sources.insert(resolved.source_index)
                    && resolved.source_index < (self.resolved.len() as u32)
                    && slot.placement == owner.native_placement
                    && slot.owner_name == owner.native_owner_name
                    && slot.recorder_kind == RecorderKind::SpatialStateV1
                    && slot.initial_presence == owner.expected_presence
                    && owner.expected_incarnation
                        == u64::from(slot.initial_presence == Presence::Live)
                    && placement.class == owner.native_class,
                "scalar declaration mismatch"
            );
        }
        Ok(())
    }
    /// Validate immutable source provenance by replaying an initial schema-7 base.
    pub fn validate(
        &self,
        native: &NativeWorldSnapshot,
        lifecycle: &LifecycleSnapshot,
        contracts: &ScalarOwnerContractsSnapshotV1,
        preserved: &PreservedSection,
    ) -> Result<()> {
        self.validate_bounds()?;
        contracts.validate_bounds()?;
        validate_native_bounds(native)?;
        validate_snapshot_bounds(lifecycle)?;
        let scalar = CompiledScalarOwnerContracts::from_snapshot(contracts, native)?;
        let compiled = CompiledNativeLifecycle::from_snapshot(lifecycle, native)?;
        let bindings = CompiledC001ScalarOwnerBindings::compile(self.bindings.clone())?;
        ensure!(
            bindings.identity() == self.bindings_id && bindings.definition() == &self.bindings,
            "scalar canonical bindings mismatch"
        );
        self.validate_declarations(native, lifecycle, contracts)?;
        ensure!(
            self.bindings.expected_payload_sha256 == crate::hex_lower(&preserved.payload_sha256()),
            "scalar preserved digest mismatch"
        );
        let payload = preserved
            .decode()
            .and_then(|tree| tree.to_inner_payload())
            .map_err(|_| anyhow::anyhow!("scalar source reconstruction invalid"))?;
        let roots = read_root(&payload, PROFILE)
            .map_err(|_| anyhow::anyhow!("scalar source root invalid"))?;
        let (_, root, decisions) = stage_prospect_save_state_with_root_values(
            &roots.tags,
            PROFILE,
            MAX_BOUNDED_COMPONENT_BYTES,
            None,
        )
        .map_err(|_| anyhow::anyhow!("scalar source staging invalid"))?;
        validate_root(&root).map_err(|_| anyhow::anyhow!("scalar source root unsupported"))?;
        validate_source_headers(&payload)
            .map_err(|_| anyhow::anyhow!("scalar source headers invalid"))?;
        ensure!(self.root_matches(&root), "scalar immutable root mismatch");
        let records = read_recorders(&roots.tags, PROFILE)
            .map_err(|_| anyhow::anyhow!("scalar source recorders invalid"))?;
        ensure!(
            records.records().len() == self.resolved.len()
                && decisions.len() == self.resolved.len(),
            "scalar source coverage mismatch"
        );
        let by_index: BTreeMap<_, _> = self
            .resolved
            .iter()
            .map(|r| (r.source_index as usize, r))
            .collect();
        let mut commands = Vec::with_capacity(self.resolved.len());
        for (index, record) in records.records().iter().enumerate() {
            let class = decode_string(record.class_value, ByteOrder::Little)
                .map_err(|_| anyhow::anyhow!("scalar source class invalid"))?;
            ensure!(
                class.consumed == record.class_value.len()
                    && exact(&class.units, C001)
                    && encode_string(&class.units, ByteOrder::Little)
                        .is_ok_and(|v| v == record.class_value),
                "scalar source class unsupported"
            );
            validate_wrapper(record.record, index)
                .map_err(|_| anyhow::anyhow!("scalar source wrapper invalid"))?;
            let source = source_scalar_owner(record.binary_data, index, &bindings.layout)
                .map_err(|_| anyhow::anyhow!("scalar source component invalid"))?;
            let resolved = by_index
                .get(&index)
                .context("scalar source ordinal missing")?;
            ensure!(
                decision_policy(&decisions[index], &source.owner)? == resolved.source_policy,
                "scalar source policy mismatch"
            );
            commands.push(recorder(
                source,
                &self.bindings.owners[resolved.binding_index as usize],
                resolved.source_policy,
            )?);
        }
        let catalog = Arc::new(CompiledNativeCatalog::compile(native.definitions.clone())?);
        let mut initial =
            prepare_authored_world(catalog, &self.bindings.native_map)?.initial_snapshot();
        initial.schema_version = SNAPSHOT_SCHEMA_LIFECYCLE;
        initial.native_lifecycle = Some(compiled.initial_snapshot());
        let replay = prepare_owner_restore(
            &initial,
            &RestoreBatch {
                lifecycle_id: self.bindings.native_lifecycle_id.clone(),
                sequence: 1,
                recorders: commands,
            },
        )
        .map_err(|_| anyhow::anyhow!("scalar initial replay invalid"))?;
        scalar.validate_native(
            replay
                .snapshot()
                .native_world
                .as_ref()
                .context("scalar replay native missing")?,
        )?;
        ensure!(
            replay.completion() == &self.initial_completion,
            "scalar initial receipt mismatch"
        );
        self.validate_current_state(native, lifecycle, contracts)
    }
    /// Pinned live-state check; does not reconstruct source or compile definitions.
    pub(crate) fn validate_current_state(
        &self,
        native: &NativeWorldSnapshot,
        lifecycle: &LifecycleSnapshot,
        contracts: &ScalarOwnerContractsSnapshotV1,
    ) -> Result<()> {
        self.validate_bounds()?;
        contracts.validate_bounds()?;
        validate_native_bounds(native)?;
        validate_snapshot_bounds(lifecycle)?;
        self.validate_declarations(native, lifecycle, contracts)?;
        ensure!(
            lifecycle.restore_sequence >= 1
                && self.initial_completion.sequence == 1
                && self.initial_completion.outcomes.len() == self.resolved.len()
                && lifecycle.slots.len() == self.resolved.len()
                && native.actors.len() == self.resolved.len()
                && lifecycle.bindings.len() == self.resolved.len(),
            "scalar current coverage or sequence invalid"
        );
        let completed = lifecycle
            .completed
            .as_ref()
            .context("scalar completion missing")?;
        ensure!(
            completed.sequence == lifecycle.restore_sequence
                && (lifecycle.restore_sequence != 1 || completed == &self.initial_completion),
            "scalar current completion invalid"
        );
        let floors: BTreeMap<_, _> = self
            .initial_completion
            .outcomes
            .iter()
            .map(|o| (o.target, o.owner.incarnation))
            .collect();
        ensure!(
            floors.len() == self.resolved.len(),
            "scalar initial floor coverage invalid"
        );
        let actors: BTreeMap<_, _> = native.actors.iter().map(|a| (a.id, a)).collect();
        ensure!(
            actors.len() == self.resolved.len(),
            "scalar current actor duplicate"
        );
        for resolved in &self.resolved {
            let b = &self.bindings.owners[resolved.binding_index as usize].owner;
            let rank = usize::try_from(
                resolved
                    .id
                    .0
                    .checked_sub(1)
                    .context("scalar slot id invalid")?,
            )?;
            let slot = lifecycle.slots.get(rank).context("scalar slot missing")?;
            let actor = actors.get(&resolved.id).context("scalar actor missing")?;
            let floor = *floors
                .get(&resolved.id)
                .context("scalar incarnation floor missing")?;
            let component = slot
                .component
                .as_ref()
                .context("scalar recorder component missing")?;
            ensure!(
                floor >= 1
                    && slot.id == resolved.id
                    && slot.presence == Presence::Live
                    && slot.incarnation >= floor
                    && component.kind == RecorderKind::SpatialStateV1
                    && component.guid > 0
                    && actor.placement == b.native_placement
                    && actor.class == b.native_class
                    && actor.variables.capacity() <= 4,
                "scalar current owner invalid"
            );
            ensure!(
                lifecycle
                    .bindings
                    .iter()
                    .filter(|v| v.owner
                        == OwnerHandle {
                            id: slot.id,
                            incarnation: slot.incarnation
                        }
                        && v.guid == component.guid
                        && v.kind == component.kind)
                    .count()
                    == 1,
                "scalar current recorder binding invalid"
            );
            let class = contracts
                .definition
                .classes
                .iter()
                .find(|c| c.native_class == actor.class)
                .context("scalar current contract missing")?;
            crate::native_scalar_owners::evaluate_values(class.authored_max, &actor.variables)?;
        }
        Ok(())
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct C001ScalarOwnerInspection {
    pub owners: C001OwnerInspection,
    pub scalar_properties: usize,
    pub contract_bytes: usize,
}
#[derive(Debug)]
pub struct PreparedC001ScalarOwnerImport {
    snapshot: WorldSnapshot,
    inspection: C001ScalarOwnerInspection,
}
impl PreparedC001ScalarOwnerImport {
    pub fn snapshot(&self) -> &WorldSnapshot {
        &self.snapshot
    }
    pub fn inspection(&self) -> &C001ScalarOwnerInspection {
        &self.inspection
    }
    pub fn into_snapshot(self) -> WorldSnapshot {
        self.snapshot
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
fn source_error(e: SemanticImportError) -> SemanticImportError {
    SemanticImportError {
        field: "c001_scalar_owners.source",
        ..e
    }
}
pub fn prepare_c001_scalar_owner_import(
    candidate: &DecodedImportCandidate,
    base: WorldSnapshot,
    bindings: &CompiledC001ScalarOwnerBindings,
    contracts: &CompiledScalarOwnerContracts,
) -> std::result::Result<PreparedC001ScalarOwnerImport, SemanticImportError> {
    use SemanticImportCategory::{Binding, Limit, Unsupported, Validation};
    let fail = |field, category| error(None, field, category);
    if base.schema_version != SNAPSHOT_SCHEMA_LIFECYCLE
        || base.preserved_prospect.is_some()
        || base.native_import.is_some()
        || base.native_owner_import.is_some()
        || base.native_scalar_contracts.is_some()
        || base.native_scalar_owner_import.is_some()
        || !base.entities.is_empty()
    {
        return Err(fail("c001_scalar_owners.base", Unsupported));
    }
    if base.entities.capacity() > MAX_NATIVE_PLACEMENTS {
        return Err(fail("c001_scalar_owners.base", Limit));
    }
    let native = base
        .native_world
        .as_ref()
        .ok_or_else(|| fail("c001_scalar_owners.native", Binding))?;
    let lifecycle = base
        .native_lifecycle
        .as_ref()
        .ok_or_else(|| fail("c001_scalar_owners.lifecycle", Binding))?;
    validate_native_bounds(native).map_err(|_| fail("c001_scalar_owners.native", Limit))?;
    validate_snapshot_bounds(lifecycle).map_err(|_| fail("c001_scalar_owners.lifecycle", Limit))?;
    // Scalar-specific allocations are rejected before any snapshot or definition copy.
    if native.actors.iter().any(|a| a.variables.capacity() > 4) {
        return Err(fail("c001_scalar_owners.variables", Limit));
    }
    if let Some(r) = &base.native_relationships {
        validate_relationship_capacities(r)
            .map_err(|_| fail("c001_scalar_owners.relationships", Limit))?;
    }
    base.validate()
        .map_err(|_| fail("c001_scalar_owners.base", Validation))?;
    contracts
        .validate_native(native)
        .map_err(|_| fail("c001_scalar_owners.contracts", Binding))?;
    if lifecycle.restore_sequence != 0 || lifecycle.completed.is_some() {
        return Err(fail("c001_scalar_owners.base", Binding));
    }
    let binding = bindings.definition();
    validate_root(candidate.root_values()).map_err(source_error)?;
    validate_source_headers(candidate.payload()).map_err(source_error)?;
    if binding.expected_payload_sha256 != crate::hex_lower(&candidate.preserved().payload_sha256())
        || binding.native_catalog_id != native.catalog_id
        || binding.native_lifecycle_id != lifecycle.lifecycle_id
        || binding.native_contract_id != contracts.identity()
        || binding.source_map != candidate.root_values().prospect_map_name()
        || binding.native_map != native.map
    {
        return Err(fail("c001_scalar_owners.manifest", Binding));
    }
    let root = candidate.root_values();
    let mut names = Names::default();
    binding
        .bounds(&mut names)
        .and_then(|()| names.check(&root.prospect_info().prospect_id, true))
        .and_then(|()| names.check(&root.prospect_info().lobby_name, false))
        .map_err(|_| fail("c001_scalar_owners.names", Limit))?;
    let count = candidate.recorders().len();
    if count == 0 || count > MAX_NATIVE_PLACEMENTS {
        return Err(fail("c001_scalar_owners.recorders", Limit));
    }
    if count != binding.owners.len() || count != lifecycle.slots.len() {
        return Err(fail("c001_scalar_owners.coverage", Binding));
    }
    let by_path: BTreeMap<_, _> = binding
        .owners
        .iter()
        .enumerate()
        .map(|(i, b)| (b.owner.source_actor_path.as_slice(), (i, b)))
        .collect();
    let map = native
        .definitions
        .maps
        .iter()
        .find(|m| m.key == native.map)
        .ok_or_else(|| fail("c001_scalar_owners.map", Binding))?;
    let by_placement: BTreeMap<_, _> = map
        .placements
        .iter()
        .enumerate()
        .map(|(i, p)| (p.key.as_str(), DomainId(i as u64 + 1)))
        .collect();
    let mut resolved = Vec::with_capacity(count);
    let mut commands = Vec::with_capacity(count);
    let (mut paths, mut guids) = (BTreeSet::new(), BTreeSet::new());
    for (index, record) in candidate.recorders().iter().enumerate() {
        let fail_at = |field, category| error(Some(index), field, category);
        if record.source_index() != index || !exact(record.class_units(), C001) {
            return Err(fail_at("c001_scalar_owners.class", Unsupported));
        }
        let (_, summary) = record
            .typing()
            .lossless()
            .ok_or_else(|| fail_at("c001_scalar_owners.typing", Unsupported))?;
        if summary.uninterpreted != 0 {
            return Err(fail_at("c001_scalar_owners.typing", Unsupported));
        }
        validate_wrapper(
            candidate
                .recorder_bytes(index)
                .ok_or_else(|| fail_at("c001_scalar_owners.recorder", Validation))?,
            index,
        )
        .map_err(source_error)?;
        let source = source_scalar_owner(
            candidate
                .recorder_binary_data(index)
                .ok_or_else(|| fail_at("c001_scalar_owners.component", Validation))?,
            index,
            &bindings.layout,
        )
        .map_err(source_error)?;
        let policy = decision_policy(record.decision(), &source.owner)
            .map_err(|_| fail_at("c001_scalar_owners.policy", Unsupported))?;
        if !paths.insert(source.owner.actor_path.clone()) || !guids.insert(source.owner.guid) {
            return Err(fail_at("c001_scalar_owners.source_identity", Binding));
        }
        let (binding_index, b) = by_path
            .get(source.owner.actor_path.as_slice())
            .ok_or_else(|| fail_at("c001_scalar_owners.binding", Binding))?;
        let id = *by_placement
            .get(b.owner.native_placement.as_str())
            .ok_or_else(|| fail_at("c001_scalar_owners.placement", Binding))?;
        let command = recorder(source, b, policy)
            .map_err(|_| fail_at("c001_scalar_owners.binding", Binding))?;
        contracts
            .evaluate(&b.owner.native_class, &command.variables)
            .map_err(|_| fail_at("c001_scalar_owners.values", Validation))?;
        commands.push(command);
        resolved.push(ResolvedC001ScalarOwnerV1 {
            source_index: index as u32,
            binding_index: *binding_index as u32,
            id,
            source_policy: policy,
        });
    }
    let contract_snapshot = contracts.snapshot();
    let prepared = prepare_owner_restore(
        &base,
        &RestoreBatch {
            lifecycle_id: binding.native_lifecycle_id.clone(),
            sequence: 1,
            recorders: commands,
        },
    )
    .map_err(|_| fail("c001_scalar_owners.restore", Validation))?;
    contracts
        .validate_native(
            prepared
                .snapshot()
                .native_world
                .as_ref()
                .ok_or_else(|| fail("c001_scalar_owners.native", Validation))?,
        )
        .map_err(|_| fail("c001_scalar_owners.callback", Validation))?;
    let metadata_bytes = encoded_size(
        &BorrowedMetadata {
            format: 1,
            bindings_id: bindings.identity(),
            bindings: binding,
            source_version: 2,
            source_prospect_id: &root.prospect_info().prospect_id,
            source_lobby_name: &root.prospect_info().lobby_name,
            source_last_saved_date_time: root.last_saved_date_time(),
            root_presence: root.source_presence().into(),
            resolved: &resolved,
            initial_completion: prepared.completion(),
        },
        MAX_C001_SCALAR_OWNER_METADATA_BYTES,
    )
    .map_err(|_| fail("c001_scalar_owners.metadata", Limit))?;
    let metadata = C001ScalarOwnerImportMetadataV1 {
        format: 1,
        bindings_id: bindings.identity().into(),
        bindings: binding.clone(),
        source_version: 2,
        source_prospect_id: root.prospect_info().prospect_id.clone(),
        source_lobby_name: root.prospect_info().lobby_name.clone(),
        source_last_saved_date_time: root.last_saved_date_time(),
        root_presence: root.source_presence().into(),
        resolved,
        initial_completion: prepared.completion().clone(),
    };
    metadata
        .validate_bounds()
        .map_err(|_| fail("c001_scalar_owners.metadata", Limit))?;
    metadata
        .validate_declarations(native, lifecycle, &contract_snapshot)
        .map_err(|_| fail("c001_scalar_owners.declarations", Binding))?;
    let inspection = C001ScalarOwnerInspection {
        owners: C001OwnerInspection {
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
        },
        scalar_properties: count * 2,
        contract_bytes: encoded_size(
            &contract_snapshot,
            crate::native_scalar_owners::MAX_SCALAR_OWNER_CONTRACT_BYTES,
        )
        .map_err(|_| fail("c001_scalar_owners.contracts", Limit))?,
    };
    let source = candidate.preserved();
    let preserved = PreservedSection::from_native_encoding(
        source.native_encoding().to_vec(),
        source.payload_bytes(),
        source.payload_sha256(),
    )
    .map_err(|_| fail("c001_scalar_owners.preserved", Validation))?;
    let mut snapshot = prepared.into_snapshot();
    snapshot.schema_version = SNAPSHOT_SCHEMA_C001_SCALAR_OWNERS;
    snapshot.native_scalar_contracts = Some(contract_snapshot);
    snapshot.native_scalar_owner_import = Some(metadata);
    snapshot.preserved_prospect = Some(Arc::new(preserved));
    SnapshotStore::preflight(&snapshot)
        .map_err(|_| fail("c001_scalar_owners.preflight", Validation))?;
    Ok(PreparedC001ScalarOwnerImport {
        snapshot,
        inspection,
    })
}
pub fn publish_new_c001_scalar_owner_import(
    prepared: PreparedC001ScalarOwnerImport,
    destination: &Path,
) -> Result<SaveReceipt> {
    // Revalidate and encode the complete result before the destination is opened.
    SnapshotStore::preflight(prepared.snapshot())?;
    SnapshotStore::open(destination)?.save_new(prepared.snapshot())
}

fn coordinates<'de, D: Deserializer<'de>>(
    d: D,
) -> std::result::Result<AuthoredCoordinatesV1, D::Error> {
    struct Visitor;
    impl serde::de::Visitor<'_> for Visitor {
        type Value = AuthoredCoordinatesV1;
        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("authored coordinate string")
        }
        fn visit_str<E: serde::de::Error>(self, v: &str) -> std::result::Result<Self::Value, E> {
            match v {
                "right_handed_z_up_meters_quat_xyzw_world_space" => {
                    Ok(AuthoredCoordinatesV1::RightHandedZUpMetersQuatXyzwWorldSpace)
                }
                _ => Err(E::custom("scalar coordinates invalid")),
            }
        }
    }
    d.deserialize_str(Visitor)
        .map_err(|_| <D::Error as serde::de::Error>::custom("scalar coordinates invalid"))
}

#[cfg(test)]
mod metadata_budget_tests {
    use super::*;

    #[test]
    fn borrowed_metadata_preflight_matches_retained_encoding_and_exact_limit() {
        let state = crate::scalar_owner_fixture::imported();
        let metadata = state.native_scalar_owner_import.as_ref().unwrap();
        let borrowed = BorrowedMetadata {
            format: metadata.format,
            bindings_id: &metadata.bindings_id,
            bindings: &metadata.bindings,
            source_version: metadata.source_version,
            source_prospect_id: &metadata.source_prospect_id,
            source_lobby_name: &metadata.source_lobby_name,
            source_last_saved_date_time: metadata.source_last_saved_date_time,
            root_presence: metadata.root_presence.clone(),
            resolved: &metadata.resolved,
            initial_completion: &metadata.initial_completion,
        };
        let bytes = serde_json::to_vec(metadata).unwrap();
        assert_eq!(serde_json::to_vec(&borrowed).unwrap(), bytes);
        assert_eq!(encoded_size(&borrowed, bytes.len()).unwrap(), bytes.len());
        assert!(encoded_size(&borrowed, bytes.len() - 1).is_err());
    }
}

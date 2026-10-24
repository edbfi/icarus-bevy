//! Bounded authored C001 SOURCE -> native parts and immutable provenance.
//! Parts cannot publish themselves. The caller retains the original candidate's
//! preserved section and preflights the complete generation before publication.
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
    native_c001_owners::C001OwnerBindingV1,
    native_c001_scalar_owners::{
        C001ScalarArrayV1, C001ScalarLayoutV1, C001ScalarOwnerBindingV1, CompiledC001ScalarLayout,
        ResolvedC001ScalarOwnerV1,
    },
    native_import::{
        AuthoredCoordinatesV1, C001RootPresence, SemanticImportCategory, SemanticImportError,
        c001_inventory_source::{
            C001InventoryLayoutV1, CompiledC001InventoryLayout, SourceInventoryOwner,
            StructArrayShapeV1, StructShapeV1, source_inventory_owner,
        },
        c001_source::{
            C001SourceProfile, validate_root, validate_source_headers, validate_wrapper,
        },
    },
    native_inventory::{
        CompiledInventoryContractsV1, InitialInventoryItemV1, InitialInventoryItemsV1,
        InventoryContractsSnapshotV1, InventoryDecodeScope, InventoryInitialTotalsV1,
        InventoryLiveOwnersV1, InventoryStateSnapshotV1, inventory_collection_bytes,
        prepare_initial_inventory, validate_inventory_state,
    },
    native_inventory_restore::{
        InventoryOwnerRestoreV1, InventoryRestoreBatchV1, add_bytes, element_bytes,
        lifecycle_bytes, sequence, text, vector_bytes,
    },
    native_lifecycle::{
        CompiledNativeLifecycle, CompletedRecorder, CompletedRestore, LifecycleSnapshot,
        OwnerHandle, Presence, RecorderKind, RecorderSource, ResolvePolicy, RestoreBatch,
        RestoreRecorder, stage_initial_owner_restore_parts, validate_native_bounds,
        validate_relationship_capacities, validate_snapshot_bounds,
    },
    native_relationships::NativeRelationshipsSnapshot,
    native_scalar_owners::{
        CompiledScalarOwnerContracts, ScalarOwnerContractsSnapshotV1, canonical_digest, digest,
        encoded_size, key, load_text,
    },
    native_world::{
        CompiledNativeCatalog, NativeValue, NativeVariableValue, NativeWorldSnapshot,
        prepare_authored_world,
    },
    preservation::PreservedSection,
    simulation::{DomainId, SNAPSHOT_SCHEMA_LIFECYCLE, WorldSnapshot},
};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Deserializer, Serialize, de};
use std::{
    cell::Cell,
    collections::{BTreeMap, BTreeSet},
    fmt,
    mem::size_of,
    path::Path,
    sync::Arc,
};

pub const C001_INVENTORY_OWNER_IMPORT_FORMAT: u32 = 1;
pub const MAX_C001_INVENTORY_OWNER_BINDINGS_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_C001_INVENTORY_OWNER_METADATA_BYTES: usize = 4 * 1024 * 1024;
const OWNERS: usize = 16_384;
const NAMES: usize = 1_048_576;
const PROFILE: TagProfile = TagProfile::MemoryArchive522;
const C001: &str = "/Script/Icarus.ActorStateRecorderComponent";

thread_local! {
    static NAME_DEPTH:Cell<usize>=const { Cell::new(0) };
    static NAME_UNITS:Cell<usize>=const { Cell::new(0) };
}
struct NameScope;
impl NameScope {
    fn enter() -> Self {
        NAME_DEPTH.with(|d| {
            if d.get() == 0 {
                NAME_UNITS.with(|u| u.set(0));
            }
            d.set(d.get() + 1);
        });
        Self
    }
}
impl Drop for NameScope {
    fn drop(&mut self) {
        NAME_DEPTH.with(|d| d.set(d.get() - 1));
    }
}
fn units<'de, D: Deserializer<'de>>(d: D) -> std::result::Result<Vec<u16>, D::Error> {
    // Charge units while reading, before allocation, across every nested DTO.
    struct Unit(u16);
    impl<'de> Deserialize<'de> for Unit {
        fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
            NAME_UNITS.with(|u| {
                let n = u
                    .get()
                    .checked_add(1)
                    .filter(|n| *n <= NAMES)
                    .ok_or_else(|| {
                        <D::Error as de::Error>::custom("inventory source name aggregate limit")
                    })?;
                u.set(n);
                Ok::<_, D::Error>(())
            })?;
            u16::deserialize(d).map(Self)
        }
    }
    let v: Vec<Unit> = sequence::<D, Unit, 1024>(d)?;
    // Unit has the same layout but conversion is safe and explicitly bounded.
    InventoryDecodeScope::charge_bytes(
        element_bytes::<u16>(v.len()).map_err(|_| {
            <D::Error as de::Error>::custom("inventory source name allocation limit")
        })?,
    )
    .map_err(|_| <D::Error as de::Error>::custom("inventory source name allocation limit"))?;
    Ok(v.into_iter().map(|v| v.0).collect())
}
macro_rules! dto {
    (pub struct $name:ident { $($(#[$attr:meta])* pub $field:ident:$ty:ty),* $(,)? } $(; $check:ident)?)=>{
        #[derive(Clone,PartialEq,Serialize)] pub struct $name { $($(#[$attr])* pub $field:$ty),* }
        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D:Deserializer<'de>>(d:D)->std::result::Result<Self,D::Error> {
                let _scope=InventoryDecodeScope::enter(); let _names=NameScope::enter();
                #[derive(Deserialize)] #[serde(deny_unknown_fields)] struct Fields { $($(#[$attr])* $field:$ty),* }
                let f:Fields=crate::json::object(d).map_err(|_|<D::Error as de::Error>::custom("C001 inventory object invalid"))?;
                let value = Self {$($field:f.$field),*};
                $(value.$check().map_err(|_|<D::Error as de::Error>::custom("C001 inventory object invalid"))?;)?
                Ok(value)
            }
        }
        impl fmt::Debug for $name { fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result {f.debug_struct(stringify!($name)).finish_non_exhaustive()} }
    };
}
#[derive(Clone, Copy, PartialEq, Eq, Serialize)]
pub enum C001InventoryOwnerAdapter {
    #[serde(rename = "c001_authored_inventory_owners_v1")]
    C001AuthoredInventoryOwnersV1,
}
impl fmt::Debug for C001InventoryOwnerAdapter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("C001InventoryOwnerAdapter(..)")
    }
}
impl<'de> Deserialize<'de> for C001InventoryOwnerAdapter {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        struct V;
        impl de::Visitor<'_> for V {
            type Value = C001InventoryOwnerAdapter;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("inventory adapter string")
            }
            fn visit_str<E: de::Error>(self, v: &str) -> std::result::Result<Self::Value, E> {
                match v {
                    "c001_authored_inventory_owners_v1" => {
                        Ok(C001InventoryOwnerAdapter::C001AuthoredInventoryOwnersV1)
                    }
                    _ => Err(E::custom("inventory adapter invalid")),
                }
            }
        }
        d.deserialize_str(V)
            .map_err(|_| <D::Error as de::Error>::custom("inventory adapter invalid"))
    }
}
dto! { pub struct ItemSourceBindingV1 { #[serde(deserialize_with="units")] pub source_row:Vec<u16>, #[serde(deserialize_with="text::<_,64>")] pub item:String } }
dto! { pub struct InventorySourceBindingV1 { #[serde(deserialize_with="units")] pub source_key:Vec<u16>, #[serde(deserialize_with="text::<_,64>")] pub inventory:String } }
dto! { pub struct C001InventoryOwnerBindingV1 {
    #[serde(deserialize_with="scalar_binding")] pub scalar:C001ScalarOwnerBindingV1,
    #[serde(deserialize_with="sequence::<_,_,8>")] pub inventories:Vec<InventorySourceBindingV1>
} }
dto! { pub struct C001InventoryOwnerBindingsV1 {
    pub format:u32,pub adapter:C001InventoryOwnerAdapter,
    #[serde(deserialize_with="text::<_,64>")] pub expected_payload_sha256:String,
    #[serde(deserialize_with="text::<_,64>")] pub native_catalog_id:String,
    #[serde(deserialize_with="text::<_,64>")] pub native_lifecycle_id:String,
    #[serde(deserialize_with="text::<_,64>")] pub native_scalar_contract_id:String,
    #[serde(deserialize_with="text::<_,64>")] pub native_inventory_contract_id:String,
    #[serde(deserialize_with="units")] pub source_map:Vec<u16>,
    #[serde(deserialize_with="text::<_,64>")] pub native_map:String,
    #[serde(deserialize_with="coordinates")] pub coordinates:AuthoredCoordinatesV1,
    #[serde(deserialize_with="scalar_layout")] pub scalar_layout:C001ScalarLayoutV1,
    #[serde(deserialize_with="inventory_layout")] pub inventory_layout:C001InventoryLayoutV1,
    #[serde(deserialize_with="sequence::<_,_,1024>")] pub items:Vec<ItemSourceBindingV1>,
    #[serde(deserialize_with="sequence::<_,_,16384>")] pub owners:Vec<C001InventoryOwnerBindingV1>
} }
dto! { pub struct InventoryInitialReceiptV1 {
    #[serde(deserialize_with="text::<_,64>")] pub initial_state_digest:String,
    pub initial_next_stack_id:u64,pub initial_totals:InventoryInitialTotalsV1
} }
dto! { pub struct C001InventoryOwnerImportMetadataV1 {
    pub format:u32,
    #[serde(deserialize_with="text::<_,64>")] pub bindings_id:String,
    pub bindings:C001InventoryOwnerBindingsV1,pub source_version:i32,
    #[serde(deserialize_with="units")] pub source_prospect_id:Vec<u16>,
    #[serde(deserialize_with="units")] pub source_lobby_name:Vec<u16>,
    pub source_last_saved_date_time:Option<i64>,pub root_presence:C001RootPresence,
    #[serde(deserialize_with="sequence::<_,_,16384>")] pub resolved:Vec<ResolvedC001ScalarOwnerV1>,
    #[serde(deserialize_with="completion")] pub initial_completion:CompletedRestore,
    pub inventory_initial:InventoryInitialReceiptV1
} ; validate_bounds }

// Bounded mirrors for inherited declarations. They reject positional objects,
// duplicate and unknown members while preserving every exact UTF16 unit.
fn scalar_binding<'de, D: Deserializer<'de>>(
    d: D,
) -> std::result::Result<C001ScalarOwnerBindingV1, D::Error> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct F {
        #[serde(deserialize_with = "owner_binding")]
        owner: C001OwnerBindingV1,
        #[serde(deserialize_with = "units")]
        enabled_source_name: Vec<u16>,
        #[serde(deserialize_with = "units")]
        remaining_uses_source_name: Vec<u16>,
    }
    let f: F = crate::json::object(d)
        .map_err(|_| <D::Error as de::Error>::custom("inventory scalar binding invalid"))?;
    Ok(C001ScalarOwnerBindingV1 {
        owner: f.owner,
        enabled_source_name: f.enabled_source_name,
        remaining_uses_source_name: f.remaining_uses_source_name,
    })
}
fn owner_binding<'de, D: Deserializer<'de>>(
    d: D,
) -> std::result::Result<C001OwnerBindingV1, D::Error> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct F {
        #[serde(deserialize_with = "units")]
        source_actor_class: Vec<u16>,
        #[serde(deserialize_with = "units")]
        source_actor_path: Vec<u16>,
        #[serde(deserialize_with = "units")]
        source_object_name: Vec<u16>,
        source_icarus_guid: i32,
        #[serde(deserialize_with = "text::<_,64>")]
        native_placement: String,
        #[serde(deserialize_with = "text::<_,64>")]
        native_class: String,
        #[serde(deserialize_with = "units")]
        native_owner_name: Vec<u16>,
        native_recorder_guid: i32,
        create_if_missing: bool,
        expected_presence: Presence,
        expected_incarnation: u64,
        expected_prior_binding: Option<OwnerHandle>,
    }
    let f: F = crate::json::object(d)
        .map_err(|_| <D::Error as de::Error>::custom("inventory owner binding invalid"))?;
    Ok(C001OwnerBindingV1 {
        source_actor_class: f.source_actor_class,
        source_actor_path: f.source_actor_path,
        source_object_name: f.source_object_name,
        source_icarus_guid: f.source_icarus_guid,
        native_placement: f.native_placement,
        native_class: f.native_class,
        native_owner_name: f.native_owner_name,
        native_recorder_guid: f.native_recorder_guid,
        create_if_missing: f.create_if_missing,
        expected_presence: f.expected_presence,
        expected_incarnation: f.expected_incarnation,
        expected_prior_binding: f.expected_prior_binding,
    })
}
fn scalar_array<'de, D: Deserializer<'de>>(
    d: D,
) -> std::result::Result<C001ScalarArrayV1, D::Error> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct F {
        #[serde(deserialize_with = "units")]
        array_name: Vec<u16>,
        #[serde(deserialize_with = "units")]
        element_tag_name: Vec<u16>,
        #[serde(deserialize_with = "units")]
        record_type: Vec<u16>,
        #[serde(deserialize_with = "units")]
        value_field: Vec<u16>,
    }
    let f: F = crate::json::object(d)
        .map_err(|_| <D::Error as de::Error>::custom("inventory scalar layout invalid"))?;
    Ok(C001ScalarArrayV1 {
        array_name: f.array_name,
        element_tag_name: f.element_tag_name,
        record_type: f.record_type,
        value_field: f.value_field,
    })
}
fn scalar_layout<'de, D: Deserializer<'de>>(
    d: D,
) -> std::result::Result<C001ScalarLayoutV1, D::Error> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct F {
        #[serde(deserialize_with = "scalar_array")]
        enabled: C001ScalarArrayV1,
        #[serde(deserialize_with = "scalar_array")]
        remaining_uses: C001ScalarArrayV1,
    }
    let f: F = crate::json::object(d)
        .map_err(|_| <D::Error as de::Error>::custom("inventory scalar layout invalid"))?;
    Ok(C001ScalarLayoutV1 {
        enabled: f.enabled,
        remaining_uses: f.remaining_uses,
    })
}
fn coordinates<'de, D: Deserializer<'de>>(
    d: D,
) -> std::result::Result<AuthoredCoordinatesV1, D::Error> {
    struct V;
    impl de::Visitor<'_> for V {
        type Value = AuthoredCoordinatesV1;
        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("authored coordinate string")
        }
        fn visit_str<E: de::Error>(self, v: &str) -> std::result::Result<Self::Value, E> {
            match v {
                "right_handed_z_up_meters_quat_xyzw_world_space" => {
                    Ok(AuthoredCoordinatesV1::RightHandedZUpMetersQuatXyzwWorldSpace)
                }
                _ => Err(E::custom("inventory coordinates invalid")),
            }
        }
    }
    d.deserialize_str(V)
        .map_err(|_| <D::Error as de::Error>::custom("inventory coordinates invalid"))
}
fn completion<'de, D: Deserializer<'de>>(d: D) -> std::result::Result<CompletedRestore, D::Error> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct F {
        sequence: u64,
        #[serde(deserialize_with = "text::<_,64>")]
        request_digest: String,
        #[serde(deserialize_with = "sequence::<_,_,16384>")]
        outcomes: Vec<CompletedRecorder>,
    }
    let f: F = crate::json::object(d)
        .map_err(|_| <D::Error as de::Error>::custom("inventory completion invalid"))?;
    Ok(CompletedRestore {
        sequence: f.sequence,
        request_digest: f.request_digest,
        outcomes: f.outcomes,
    })
}

#[derive(Default)]
struct Bounds {
    names: usize,
    bytes: usize,
}
impl Bounds {
    fn name(&mut self, v: &Vec<u16>, nonempty: bool) -> Result<()> {
        ensure!(
            (!nonempty || !v.is_empty()) && v.len() <= 1024 && v.capacity() <= 2050,
            "inventory source name limit"
        );
        self.names = self
            .names
            .checked_add(v.len())
            .filter(|n| *n <= NAMES)
            .context("inventory source aggregate name limit")?;
        add_bytes(&mut self.bytes, vector_bytes(v, 1024)?)?;
        // units reads Vec<Unit> then explicitly charges len * sizeof(u16)
        // for conversion. A capacity maximum alone misses that second charge.
        add_bytes(&mut self.bytes, element_bytes::<u16>(v.len())?)
    }
    fn key(&mut self, v: &String) -> Result<()> {
        key(v)?;
        add_bytes(&mut self.bytes, v.capacity())
    }
    fn header(&mut self, v: &Vec<u16>) -> Result<()> {
        self.name(v, true)?;
        ensure!(
            v.len() <= 1023 && v.iter().all(|u| *u != 0 && *u != u16::MAX),
            "inventory declaration header limit"
        );
        Ok(())
    }
    fn digest(&mut self, v: &String) -> Result<()> {
        digest(v)?;
        add_bytes(&mut self.bytes, v.capacity())
    }
    fn vector<T>(&mut self, v: &Vec<T>, limit: usize) -> Result<()> {
        add_bytes(&mut self.bytes, vector_bytes(v, limit)?)
    }
}
impl C001InventoryOwnerBindingsV1 {
    pub(crate) fn allocation_bytes(&self) -> Result<usize> {
        let mut b = Bounds {
            names: 0,
            bytes: size_of::<Self>(),
        };
        self.bounds(&mut b)?;
        Ok(b.bytes)
    }
    fn bounds(&self, b: &mut Bounds) -> Result<()> {
        ensure!(
            self.format == 1 && !self.owners.is_empty(),
            "inventory bindings format or coverage invalid"
        );
        for d in [
            &self.expected_payload_sha256,
            &self.native_catalog_id,
            &self.native_lifecycle_id,
            &self.native_scalar_contract_id,
            &self.native_inventory_contract_id,
        ] {
            b.digest(d)?;
        }
        b.key(&self.native_map)?;
        b.name(&self.source_map, true)?;
        for array in [
            &self.scalar_layout.enabled,
            &self.scalar_layout.remaining_uses,
        ] {
            for v in [
                &array.array_name,
                &array.element_tag_name,
                &array.record_type,
                &array.value_field,
            ] {
                b.header(v)?;
            }
        }
        let l = &self.inventory_layout;
        for a in [
            &l.inventories,
            &l.slots,
            &l.dynamic,
            &l.additional_stats,
            &l.alterations,
            &l.living_item_slots,
        ] {
            for v in [&a.field_name, &a.element_tag_name, &a.record_type] {
                b.header(v)?;
            }
        }
        b.header(&l.inventory_key_field)?;
        b.header(&l.inventory_data.field_name)?;
        b.header(&l.inventory_data.record_type)?;
        b.vector(&self.items, 1024)?;
        b.vector(&self.owners, OWNERS)?;
        let (mut rows, mut items) = (BTreeSet::new(), BTreeSet::new());
        for i in &self.items {
            b.name(&i.source_row, true)?;
            b.key(&i.item)?;
            ensure!(
                rows.insert(i.source_row.as_slice()) && items.insert(i.item.as_str()),
                "inventory duplicate item mapping"
            );
        }
        let (mut paths, mut guids, mut placements, mut native_guids) = (
            BTreeSet::new(),
            BTreeSet::new(),
            BTreeSet::new(),
            BTreeSet::new(),
        );
        let mut instances = 0_usize;
        for o in &self.owners {
            let s = &o.scalar;
            let v = &s.owner;
            for n in [
                &v.source_actor_class,
                &v.source_actor_path,
                &v.source_object_name,
                &v.native_owner_name,
                &s.enabled_source_name,
                &s.remaining_uses_source_name,
            ] {
                b.name(n, true)?;
            }
            b.key(&v.native_placement)?;
            b.key(&v.native_class)?;
            ensure!(
                s.enabled_source_name != s.remaining_uses_source_name
                    && v.source_icarus_guid > 0
                    && v.native_recorder_guid > 0
                    && match v.expected_presence {
                        Presence::Absent => v.expected_incarnation == 0,
                        Presence::Live => v.expected_incarnation == 1,
                    },
                "inventory owner declaration invalid"
            );
            if let Some(p) = v.expected_prior_binding {
                ensure!(
                    p.id.0 > 0 && p.id.0 <= OWNERS as u64 && p.incarnation == 1,
                    "inventory prior binding invalid"
                );
            }
            ensure!(
                paths.insert(v.source_actor_path.as_slice())
                    && guids.insert(v.source_icarus_guid)
                    && placements.insert(v.native_placement.as_str())
                    && native_guids.insert(v.native_recorder_guid),
                "inventory duplicate owner mapping"
            );
            b.vector(&o.inventories, 8)?;
            ensure!(
                !o.inventories.is_empty(),
                "inventory source declaration coverage missing"
            );
            instances = instances
                .checked_add(o.inventories.len())
                .filter(|n| *n <= 65_536)
                .context("inventory instance limit")?;
            let (mut source, mut native) = (BTreeSet::new(), BTreeSet::new());
            for i in &o.inventories {
                b.name(&i.source_key, true)?;
                b.key(&i.inventory)?;
                ensure!(
                    source.insert(i.source_key.as_slice()) && native.insert(i.inventory.as_str()),
                    "inventory duplicate inventory mapping"
                );
            }
        }
        encoded_size(self, MAX_C001_INVENTORY_OWNER_BINDINGS_BYTES)?;
        Ok(())
    }
}
#[derive(Clone)]
pub struct CompiledC001InventoryOwnerBindings {
    identity: String,
    definition: C001InventoryOwnerBindingsV1,
    scalar: CompiledC001ScalarLayout,
    inventory: CompiledC001InventoryLayout,
}
impl fmt::Debug for CompiledC001InventoryOwnerBindings {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CompiledC001InventoryOwnerBindings")
            .finish_non_exhaustive()
    }
}
impl CompiledC001InventoryOwnerBindings {
    pub fn compile(mut definition: C001InventoryOwnerBindingsV1) -> Result<Self> {
        definition.allocation_bytes()?;
        let scalar = CompiledC001ScalarLayout::compile(definition.scalar_layout.clone())?;
        let inventory =
            CompiledC001InventoryLayout::compile(definition.inventory_layout.clone(), &scalar)?;
        // Source rows/keys sort by exact UTF16 units; owner declarations by
        // native placement. Source occurrence order is never canonicalized.
        definition
            .items
            .sort_by(|a, b| a.source_row.cmp(&b.source_row));
        definition.owners.sort_by(|a, b| {
            a.scalar
                .owner
                .native_placement
                .cmp(&b.scalar.owner.native_placement)
        });
        for o in &mut definition.owners {
            o.inventories
                .sort_by(|a, b| a.source_key.cmp(&b.source_key));
        }
        encoded_size(&definition, MAX_C001_INVENTORY_OWNER_BINDINGS_BYTES)?;
        let identity = canonical_digest(b"icarus/c001_inventory_owner_bindings/v1\0", &definition)?;
        Ok(Self {
            identity,
            definition,
            scalar,
            inventory,
        })
    }
    pub fn parse(input: &str) -> Result<Self> {
        ensure!(
            input.len() <= MAX_C001_INVENTORY_OWNER_BINDINGS_BYTES,
            "inventory bindings input limit"
        );
        let d = if input.trim_start().starts_with('{') {
            serde_json::from_str(input)
                .map_err(|_| anyhow::anyhow!("inventory bindings JSON invalid"))?
        } else {
            toml::from_str(input).map_err(|_| anyhow::anyhow!("inventory bindings TOML invalid"))?
        };
        Self::compile(d)
    }
    pub fn load(path: &Path) -> Result<Self> {
        Self::parse(&load_text(path, MAX_C001_INVENTORY_OWNER_BINDINGS_BYTES)?)
    }
    pub fn identity(&self) -> &str {
        &self.identity
    }
    pub fn definition(&self) -> &C001InventoryOwnerBindingsV1 {
        &self.definition
    }
}

fn exact(units: &[u16], s: &str) -> bool {
    units.iter().copied().eq(s.encode_utf16())
}
fn policy(d: &StagedRecorderDecision, s: &SourceInventoryOwner) -> Result<ResolvePolicy> {
    ensure!(
        d.retained
            && !d.reset_rocket_spawn
            && !d.physics_dynamic_tree
            && !d.skip_default_owner_resolution
            && !d.requires_stitching
            && C001SourceProfile::AuthoredInventoryOwnersV1.admits(d.owner_resolve_policy)
            && s.effective_policy() == d.owner_resolve_policy,
        "inventory staged policy unsupported"
    );
    Ok(match s.effective_policy() {
        OwnerResolvePolicy::FindOnly => ResolvePolicy::FindOnly,
        OwnerResolvePolicy::FindOrRespawn => ResolvePolicy::FindOrRespawn,
        OwnerResolvePolicy::RespawnOnly => ResolvePolicy::RespawnOnly,
        OwnerResolvePolicy::ManuallyResolved => {
            anyhow::bail!("inventory manual resolution unsupported")
        }
    })
}
fn command(
    s: &SourceInventoryOwner,
    b: &C001ScalarOwnerBindingV1,
    p: ResolvePolicy,
) -> Result<RestoreRecorder> {
    let o = &b.owner;
    ensure!(
        s.actor_class == o.source_actor_class
            && s.actor_path == o.source_actor_path
            && s.object_name == o.source_object_name
            && s.guid == o.source_icarus_guid
            && s.enabled_source_name == b.enabled_source_name
            && s.remaining_uses_source_name == b.remaining_uses_source_name,
        "inventory immutable source mismatch"
    );
    Ok(RestoreRecorder {
        guid: o.native_recorder_guid,
        kind: RecorderKind::SpatialStateV1,
        target_placement: o.native_placement.clone(),
        owner_name: o.native_owner_name.clone(),
        policy: p,
        source: RecorderSource::Detached {
            expected_prior_binding: o.expected_prior_binding,
        },
        create_if_missing: o.create_if_missing,
        expected_presence: o.expected_presence,
        expected_incarnation: o.expected_incarnation,
        transform: s.transform.clone(),
        variables: vec![
            NativeVariableValue {
                key: "enabled".into(),
                value: NativeValue::Bool(s.enabled),
            },
            NativeVariableValue {
                key: "remaining_uses".into(),
                value: NativeValue::I32(s.remaining_uses),
            },
        ],
    })
}

/// One occurrence stream, with independently bounded cumulative source names,
/// inventories and items. Source order controls native StackId assignment.
struct Admission<'a> {
    bindings: &'a CompiledC001InventoryOwnerBindings,
    scalar: &'a CompiledScalarOwnerContracts,
    commands: Vec<RestoreRecorder>,
    resolved: Vec<ResolvedC001ScalarOwnerV1>,
    items: InitialInventoryItemsV1,
    names: usize,
    instances: usize,
    bytes: usize,
    seen: BTreeSet<usize>,
}
impl<'a> Admission<'a> {
    fn new(
        bindings: &'a CompiledC001InventoryOwnerBindings,
        scalar: &'a CompiledScalarOwnerContracts,
    ) -> Self {
        Self {
            bindings,
            scalar,
            commands: Vec::new(),
            resolved: Vec::new(),
            items: InitialInventoryItemsV1 { items: Vec::new() },
            names: 0,
            instances: 0,
            bytes: 0,
            seen: BTreeSet::new(),
        }
    }
    fn admit(
        &mut self,
        index: usize,
        s: SourceInventoryOwner,
        decision: &StagedRecorderDecision,
        native: &NativeWorldSnapshot,
    ) -> Result<()> {
        let b = &self.bindings.definition;
        let (binding_index, binding) = b
            .owners
            .iter()
            .enumerate()
            .find(|(_, b)| b.scalar.owner.source_actor_path == s.actor_path)
            .context("inventory source owner unmapped")?;
        ensure!(
            self.seen.insert(binding_index),
            "inventory source owner repeated"
        );
        let p = policy(decision, &s)?;
        let placement = native
            .definitions
            .maps
            .iter()
            .find(|m| m.key == native.map)
            .context("inventory selected map missing")?
            .placements
            .iter()
            .position(|v| v.key == binding.scalar.owner.native_placement)
            .context("inventory placement missing")?;
        let id = DomainId(placement as u64 + 1);
        let mut charge_name = |n: &Vec<u16>| -> Result<()> {
            self.names = self
                .names
                .checked_add(n.len())
                .filter(|n| *n <= NAMES)
                .context("inventory source aggregate name limit")?;
            Ok(())
        };
        for n in [
            &s.actor_class,
            &s.actor_path,
            &s.object_name,
            &s.enabled_source_name,
            &s.remaining_uses_source_name,
        ] {
            charge_name(n)?;
        }
        ensure!(
            s.inventories.len() == binding.inventories.len(),
            "inventory source inventory coverage incomplete"
        );
        self.instances = self
            .instances
            .checked_add(s.inventories.len())
            .filter(|n| *n <= 65_536)
            .context("inventory source instance limit")?;
        let mut inventory_keys = BTreeSet::new();
        for source in &s.inventories {
            charge_name(&source.source_key)?;
            let mapped = binding
                .inventories
                .iter()
                .find(|i| i.source_key == source.source_key)
                .context("inventory source key unmapped")?;
            ensure!(
                inventory_keys.insert(mapped.inventory.as_str()),
                "inventory source alias"
            );
            ensure!(
                self.items
                    .items
                    .len()
                    .checked_add(source.items.len())
                    .is_some_and(|n| n <= 65_536),
                "inventory source stack limit"
            );
            for item in &source.items {
                charge_name(&item.source_row)?;
                let mapped_item = b
                    .items
                    .iter()
                    .find(|i| i.source_row == item.source_row)
                    .context("inventory source row unmapped")?;
                add_bytes(&mut self.bytes, element_bytes::<InitialInventoryItemV1>(2)?)?;
                add_bytes(&mut self.bytes, mapped.inventory.len())?;
                add_bytes(&mut self.bytes, mapped_item.item.len())?;
                self.items.items.push(InitialInventoryItemV1 {
                    owner: id,
                    inventory: mapped.inventory.clone(),
                    requested_slot: item.requested_slot,
                    item: mapped_item.item.clone(),
                    count: item.count,
                });
            }
        }
        let c = command(&s, &binding.scalar, p)?;
        self.scalar
            .evaluate(&binding.scalar.owner.native_class, &c.variables)?;
        add_bytes(&mut self.bytes, element_bytes::<RestoreRecorder>(2)?)?;
        add_bytes(&mut self.bytes, binding.scalar.owner.native_placement.len())?;
        add_bytes(
            &mut self.bytes,
            element_bytes::<u16>(binding.scalar.owner.native_owner_name.len())?,
        )?;
        add_bytes(&mut self.bytes, element_bytes::<NativeVariableValue>(2)?)?;
        self.commands.push(c);
        self.resolved.push(ResolvedC001ScalarOwnerV1 {
            source_index: index as u32,
            binding_index: binding_index as u32,
            id,
            source_policy: p,
        });
        Ok(())
    }
}

fn declarations(
    b: &C001InventoryOwnerBindingsV1,
    n: &NativeWorldSnapshot,
    l: &LifecycleSnapshot,
    s: &ScalarOwnerContractsSnapshotV1,
    i: &InventoryContractsSnapshotV1,
) -> Result<()> {
    ensure!(
        b.native_catalog_id == n.catalog_id
            && b.native_lifecycle_id == l.lifecycle_id
            && b.native_scalar_contract_id == s.contract_id
            && b.native_inventory_contract_id == i.contract_id
            && b.native_map == n.map
            && b.native_map == l.definition.map
            && b.native_map == s.definition.native_map
            && b.native_map == i.definition.native_map
            && n.catalog_id == s.definition.native_catalog_id
            && n.catalog_id == i.definition.native_catalog_id,
        "inventory declaration identity mismatch"
    );
    let map = n
        .definitions
        .maps
        .iter()
        .find(|m| m.key == n.map)
        .context("inventory map missing")?;
    ensure!(
        map.placements.len() == b.owners.len() && l.definition.slots.len() == b.owners.len(),
        "inventory declaration coverage incomplete"
    );
    // Item mappings are a bijection onto all authored definitions. Unused
    // mappings are permitted only when their native definition exists.
    let mapped: BTreeSet<_> = b.items.iter().map(|v| v.item.as_str()).collect();
    let defined: BTreeSet<_> = i.definition.items.iter().map(|v| v.key.as_str()).collect();
    ensure!(
        mapped == defined
            && mapped.len() == b.items.len()
            && defined.len() == i.definition.items.len(),
        "inventory item mapping coverage invalid"
    );
    for o in &b.owners {
        let owner = &o.scalar.owner;
        let rank = map
            .placements
            .iter()
            .position(|p| p.key == owner.native_placement)
            .context("inventory declared placement missing")?;
        let slot = &l.definition.slots[rank];
        ensure!(
            slot.placement == owner.native_placement
                && slot.owner_name == owner.native_owner_name
                && slot.recorder_kind == RecorderKind::SpatialStateV1
                && slot.initial_presence == owner.expected_presence
                && owner.expected_incarnation == u64::from(slot.initial_presence == Presence::Live)
                && map.placements[rank].class == owner.native_class,
            "inventory owner declaration mismatch"
        );
        let class = i
            .definition
            .classes
            .iter()
            .find(|c| c.native_class == owner.native_class)
            .context("inventory class contract missing")?;
        let wanted: BTreeSet<_> = class.inventories.iter().map(|v| v.key.as_str()).collect();
        let supplied: BTreeSet<_> = o.inventories.iter().map(|v| v.inventory.as_str()).collect();
        ensure!(
            wanted == supplied && supplied.len() == o.inventories.len(),
            "inventory declared key coverage incomplete"
        );
    }
    Ok(())
}

struct InitialParts {
    native: NativeWorldSnapshot,
    lifecycle: LifecycleSnapshot,
    inventories: InventoryStateSnapshotV1,
    receipt: InventoryInitialReceiptV1,
}
fn initial_parts(
    native: &NativeWorldSnapshot,
    lifecycle: &LifecycleSnapshot,
    relationships: Option<&NativeRelationshipsSnapshot>,
    admission: &Admission<'_>,
    inventory: &CompiledInventoryContractsV1,
) -> Result<InitialParts> {
    // Validate the borrowed occurrence graph before making the complete request.
    let batch = RestoreBatch {
        lifecycle_id: lifecycle.lifecycle_id.clone(),
        sequence: 1,
        recorders: admission.commands.clone(),
    };
    batch.validate_bounds()?;
    let mut inventory_result = None;
    let staged = stage_initial_owner_restore_parts(
        native,
        lifecycle,
        relationships,
        &batch,
        |final_native, outcomes| {
            admission.scalar.validate_native(final_native)?;
            ensure!(
                outcomes.len() == admission.resolved.len()
                    && outcomes.len() == final_native.actors.len(),
                "inventory initial live owner coverage invalid"
            );
            let live = InventoryLiveOwnersV1 {
                owners: outcomes.iter().map(|o| o.owner).collect(),
            };
            let prepared = prepare_initial_inventory(inventory, &live, &admission.items)?;
            let mut bytes = size_of::<InventoryRestoreBatchV1>();
            add_bytes(&mut bytes, 128)?;
            add_bytes(&mut bytes, lifecycle_bytes(&batch)?)?;
            let outcome_states: BTreeMap<_, _> = prepared
                .state()
                .owners
                .iter()
                .map(|o| (o.owner.id, o))
                .collect();
            let outcome_handles: BTreeMap<_, _> =
                outcomes.iter().map(|o| (o.guid, o.owner)).collect();
            // Charge every borrowed graph before the first inventory copy.
            let mut reload = 0_usize;
            while reload < outcomes.len() {
                reload = reload
                    .checked_mul(2)
                    .context("inventory vector allocation limit")?
                    .clamp(4, OWNERS);
            }
            add_bytes(
                &mut bytes,
                element_bytes::<InventoryOwnerRestoreV1>(reload)?,
            )?;
            for r in &batch.recorders {
                let handle = outcome_handles
                    .get(&r.guid)
                    .context("inventory initial owner outcome missing")?;
                let state = outcome_states
                    .get(&handle.id)
                    .context("inventory initial owner state missing")?;
                ensure!(
                    state.owner == *handle,
                    "inventory initial owner outcome mismatch"
                );
                add_bytes(&mut bytes, r.target_placement.capacity())?;
                add_bytes(&mut bytes, inventory_collection_bytes(&state.inventories)?)?;
            }
            let mut owners = Vec::with_capacity(outcomes.len());
            for r in &batch.recorders {
                let handle = outcome_handles
                    .get(&r.guid)
                    .context("inventory initial owner outcome missing")?;
                let state = outcome_states
                    .get(&handle.id)
                    .context("inventory initial owner state missing")?;
                owners.push(InventoryOwnerRestoreV1 {
                    target_placement: r.target_placement.clone(),
                    inventories: state.inventories.clone(),
                });
            }
            let request = InventoryRestoreBatchV1 {
                format: 1,
                scalar_contract_id: admission.scalar.identity().into(),
                inventory_contract_id: inventory.identity().into(),
                expected_inventory_revision: 0,
                lifecycle: batch.clone(),
                owners,
            };
            let digest = request.request_digest()?;
            inventory_result = Some(prepared);
            Ok(digest)
        },
    )?;
    let (inventories, totals, digest) = inventory_result
        .context("inventory initial callback missing")?
        .into_parts();
    let receipt = InventoryInitialReceiptV1 {
        initial_state_digest: digest,
        initial_next_stack_id: inventories.next_stack_id,
        initial_totals: totals,
    };
    Ok(InitialParts {
        native: staged.native,
        lifecycle: staged.lifecycle,
        inventories,
        receipt,
    })
}

#[derive(Serialize)]
struct BorrowedMetadata<'a> {
    format: u32,
    bindings_id: &'a str,
    bindings: &'a C001InventoryOwnerBindingsV1,
    source_version: i32,
    source_prospect_id: &'a [u16],
    source_lobby_name: &'a [u16],
    source_last_saved_date_time: Option<i64>,
    root_presence: C001RootPresence,
    resolved: &'a [ResolvedC001ScalarOwnerV1],
    initial_completion: &'a CompletedRestore,
    inventory_initial: &'a InventoryInitialReceiptV1,
}
/// Private fields prevent fabrication. These parts have no persistence or runtime
/// authority; `prepare_c001_inventory_owner_import` composes them with unchanged
/// preservation/relationships.
pub struct PreparedC001InventoryOwnerParts {
    native: NativeWorldSnapshot,
    lifecycle: LifecycleSnapshot,
    inventories: InventoryStateSnapshotV1,
    metadata: C001InventoryOwnerImportMetadataV1,
}
impl fmt::Debug for PreparedC001InventoryOwnerParts {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PreparedC001InventoryOwnerParts")
            .finish_non_exhaustive()
    }
}
impl PreparedC001InventoryOwnerParts {
    pub fn native(&self) -> &NativeWorldSnapshot {
        &self.native
    }
    pub fn lifecycle(&self) -> &LifecycleSnapshot {
        &self.lifecycle
    }
    pub fn inventories(&self) -> &InventoryStateSnapshotV1 {
        &self.inventories
    }
    pub fn metadata(&self) -> &C001InventoryOwnerImportMetadataV1 {
        &self.metadata
    }
    pub fn into_parts(
        self,
    ) -> (
        NativeWorldSnapshot,
        LifecycleSnapshot,
        InventoryStateSnapshotV1,
        C001InventoryOwnerImportMetadataV1,
    ) {
        (self.native, self.lifecycle, self.inventories, self.metadata)
    }
}
fn semantic(
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
pub fn prepare_c001_inventory_owner_parts(
    candidate: &DecodedImportCandidate,
    base: &WorldSnapshot,
    bindings: &CompiledC001InventoryOwnerBindings,
    scalar: &CompiledScalarOwnerContracts,
    inventory: &CompiledInventoryContractsV1,
) -> std::result::Result<PreparedC001InventoryOwnerParts, SemanticImportError> {
    use SemanticImportCategory::{Binding, Limit, Unsupported, Validation};
    let fail = |field, category| semantic(None, field, category);
    if base.schema_version != SNAPSHOT_SCHEMA_LIFECYCLE
        || base.preserved_prospect.is_some()
        || base.native_import.is_some()
        || base.native_owner_import.is_some()
        || base.native_scalar_contracts.is_some()
        || base.native_scalar_owner_import.is_some()
        || base.native_inventory_contracts.is_some()
        || base.native_inventories.is_some()
        || base.native_inventory_owner_import.is_some()
        || !base.entities.is_empty()
    {
        return Err(fail("c001_inventory_owners.base", Unsupported));
    }
    if base.entities.capacity() > OWNERS {
        return Err(fail("c001_inventory_owners.base", Limit));
    }
    let n = base
        .native_world
        .as_ref()
        .ok_or_else(|| fail("c001_inventory_owners.native", Binding))?;
    let l = base
        .native_lifecycle
        .as_ref()
        .ok_or_else(|| fail("c001_inventory_owners.lifecycle", Binding))?;
    validate_native_bounds(n)
        .and_then(|()| validate_snapshot_bounds(l))
        .map_err(|_| fail("c001_inventory_owners.base", Limit))?;
    if n.actors.iter().any(|a| a.variables.capacity() > 4) {
        return Err(fail("c001_inventory_owners.variables", Limit));
    }
    if let Some(r) = &base.native_relationships {
        validate_relationship_capacities(r)
            .map_err(|_| fail("c001_inventory_owners.relationships", Limit))?;
    }
    base.validate()
        .map_err(|_| fail("c001_inventory_owners.base", Validation))?;
    if l.restore_sequence != 0 || l.completed.is_some() {
        return Err(fail("c001_inventory_owners.base", Binding));
    }
    scalar
        .validate_native(n)
        .map_err(|_| fail("c001_inventory_owners.scalar", Binding))?;
    let b = bindings.definition();
    let scalar_snapshot = scalar.snapshot();
    let inventory_snapshot = inventory.snapshot();
    declarations(b, n, l, &scalar_snapshot, &inventory_snapshot)
        .map_err(|_| fail("c001_inventory_owners.declarations", Binding))?;
    validate_root(candidate.root_values())?;
    validate_source_headers(candidate.payload())?;
    if b.expected_payload_sha256 != crate::hex_lower(&candidate.preserved().payload_sha256())
        || b.source_map != candidate.root_values().prospect_map_name()
    {
        return Err(fail("c001_inventory_owners.manifest", Binding));
    }
    let root = candidate.root_values();
    let mut bounds = Bounds {
        names: 0,
        bytes: size_of::<C001InventoryOwnerImportMetadataV1>(),
    };
    add_bytes(&mut bounds.bytes, bindings.identity().len())
        .map_err(|_| fail("c001_inventory_owners.metadata", Limit))?;
    b.bounds(&mut bounds)
        .and_then(|()| bounds.name(&root.prospect_info().prospect_id, true))
        .and_then(|()| bounds.name(&root.prospect_info().lobby_name, false))
        .map_err(|_| fail("c001_inventory_owners.names", Limit))?;
    if candidate.recorders().is_empty()
        || candidate.recorders().len() != b.owners.len()
        || candidate.recorders().len() > OWNERS
    {
        return Err(fail("c001_inventory_owners.coverage", Binding));
    }
    let mut admission = Admission::new(bindings, scalar);
    for (index, r) in candidate.recorders().iter().enumerate() {
        let fail_at = |field, category| semantic(Some(index), field, category);
        if r.source_index() != index || !exact(r.class_units(), C001) {
            return Err(fail_at("c001_inventory_owners.class", Unsupported));
        }
        let (_, summary) = r
            .typing()
            .lossless()
            .ok_or_else(|| fail_at("c001_inventory_owners.typing", Unsupported))?;
        if summary.uninterpreted != 0 {
            return Err(fail_at("c001_inventory_owners.typing", Unsupported));
        }
        validate_wrapper(
            candidate
                .recorder_bytes(index)
                .ok_or_else(|| fail_at("c001_inventory_owners.recorder", Validation))?,
            index,
        )?;
        let source = source_inventory_owner(
            candidate
                .recorder_binary_data(index)
                .ok_or_else(|| fail_at("c001_inventory_owners.component", Validation))?,
            index,
            &bindings.scalar,
            &bindings.inventory,
        )?;
        admission
            .admit(index, source, r.decision(), n)
            .map_err(|_| fail_at("c001_inventory_owners.source", Binding))?;
    }
    let parts = initial_parts(
        n,
        l,
        base.native_relationships.as_ref(),
        &admission,
        inventory,
    )
    .map_err(|_| fail("c001_inventory_owners.restore", Validation))?;
    let completion = parts
        .lifecycle
        .completed
        .as_ref()
        .ok_or_else(|| fail("c001_inventory_owners.completion", Validation))?;
    // The complete borrowed receipt is charged before copying source roots,
    // bindings, receipt or outcome allocations into immutable provenance.
    bounds
        .vector(&admission.resolved, OWNERS)
        .and_then(|()| bounds.vector(&completion.outcomes, OWNERS))
        .and_then(|()| bounds.digest(&completion.request_digest))
        .and_then(|()| bounds.digest(&parts.receipt.initial_state_digest))
        .and_then(|()| bounds.vector(&parts.receipt.initial_totals.items, 1024))
        .map_err(|_| fail("c001_inventory_owners.metadata", Limit))?;
    for total in &parts.receipt.initial_totals.items {
        bounds
            .key(&total.item)
            .map_err(|_| fail("c001_inventory_owners.metadata", Limit))?;
    }
    encoded_size(
        &BorrowedMetadata {
            format: 1,
            bindings_id: bindings.identity(),
            bindings: b,
            source_version: 2,
            source_prospect_id: &root.prospect_info().prospect_id,
            source_lobby_name: &root.prospect_info().lobby_name,
            source_last_saved_date_time: root.last_saved_date_time(),
            root_presence: root.source_presence().into(),
            resolved: &admission.resolved,
            initial_completion: completion,
            inventory_initial: &parts.receipt,
        },
        MAX_C001_INVENTORY_OWNER_METADATA_BYTES,
    )
    .map_err(|_| fail("c001_inventory_owners.metadata", Limit))?;
    let metadata = C001InventoryOwnerImportMetadataV1 {
        format: 1,
        bindings_id: bindings.identity().into(),
        bindings: b.clone(),
        source_version: 2,
        source_prospect_id: root.prospect_info().prospect_id.clone(),
        source_lobby_name: root.prospect_info().lobby_name.clone(),
        source_last_saved_date_time: root.last_saved_date_time(),
        root_presence: root.source_presence().into(),
        resolved: admission.resolved,
        initial_completion: completion.clone(),
        inventory_initial: parts.receipt,
    };
    metadata
        .validate_current_state(
            &parts.native,
            &parts.lifecycle,
            &scalar_snapshot,
            &inventory_snapshot,
            &parts.inventories,
        )
        .map_err(|_| fail("c001_inventory_owners.metadata", Validation))?;
    Ok(PreparedC001InventoryOwnerParts {
        native: parts.native,
        lifecycle: parts.lifecycle,
        inventories: parts.inventories,
        metadata,
    })
}

impl C001InventoryOwnerImportMetadataV1 {
    pub(crate) fn validate_bounds(&self) -> Result<()> {
        self.allocation_bytes().map(|_| ())
    }
    /// Includes the enclosing metadata, bindings, names and initial totals.
    /// Callers must add this whole bound to their other graphs before cloning.
    pub(crate) fn allocation_bytes(&self) -> Result<usize> {
        ensure!(
            self.format == 1 && self.source_version == 2 && !self.resolved.is_empty(),
            "inventory metadata format or coverage invalid"
        );
        let mut b = Bounds {
            names: 0,
            bytes: size_of::<Self>(),
        };
        b.digest(&self.bindings_id)?;
        self.bindings.bounds(&mut b)?;
        b.name(&self.source_prospect_id, true)?;
        b.name(&self.source_lobby_name, false)?;
        b.vector(&self.resolved, OWNERS)?;
        b.vector(&self.initial_completion.outcomes, OWNERS)?;
        b.digest(&self.initial_completion.request_digest)?;
        b.digest(&self.inventory_initial.initial_state_digest)?;
        self.inventory_initial.initial_totals.validate_bounds()?;
        b.vector(&self.inventory_initial.initial_totals.items, 1024)?;
        for t in &self.inventory_initial.initial_totals.items {
            b.key(&t.item)?;
        }
        ensure!(
            (1..=65_537).contains(&self.inventory_initial.initial_next_stack_id),
            "inventory initial allocator invalid"
        );
        encoded_size(self, MAX_C001_INVENTORY_OWNER_METADATA_BYTES)?;
        Ok(b.bytes)
    }
    fn root_matches(&self, r: &StagedRootValues) -> bool {
        self.source_version == r.loaded_version().unwrap_or_default()
            && self.bindings.source_map == r.prospect_map_name()
            && self.source_prospect_id == r.prospect_info().prospect_id
            && self.source_lobby_name == r.prospect_info().lobby_name
            && self.source_last_saved_date_time == r.last_saved_date_time()
            && self.root_presence == r.source_presence().into()
    }
    /// Current provenance floors and declaration checks only. No source replay,
    /// definition compilation or hashing. The pinned caller validates lifecycle,
    /// scalar contracts and inventory conservation with its compiled models.
    pub(crate) fn validate_current_state(
        &self,
        n: &NativeWorldSnapshot,
        l: &LifecycleSnapshot,
        s: &ScalarOwnerContractsSnapshotV1,
        i: &InventoryContractsSnapshotV1,
        state: &InventoryStateSnapshotV1,
    ) -> Result<()> {
        self.validate_bounds()?;
        validate_native_bounds(n)?;
        validate_snapshot_bounds(l)?;
        s.validate_bounds()?;
        i.validate_bounds()?;
        state.validate_bounds()?;
        declarations(&self.bindings, n, l, s, i)?;
        ensure!(
            l.restore_sequence >= 1
                && state.revision >= l.restore_sequence
                && state.next_stack_id >= self.inventory_initial.initial_next_stack_id
                && state.contract_id == i.contract_id
                && self.initial_completion.sequence == 1
                && self.resolved.len() == self.bindings.owners.len()
                && self.initial_completion.outcomes.len() == self.resolved.len()
                && l.slots.len() == self.resolved.len()
                && n.actors.len() == self.resolved.len()
                && l.bindings.len() == self.resolved.len()
                && state.owners.len() == self.resolved.len(),
            "inventory current coverage or sequence invalid"
        );
        let completed = l
            .completed
            .as_ref()
            .context("inventory current completion missing")?;
        ensure!(
            completed.sequence == l.restore_sequence
                && (l.restore_sequence != 1 || completed == &self.initial_completion),
            "inventory current completion invalid"
        );
        let map = n
            .definitions
            .maps
            .iter()
            .find(|m| m.key == n.map)
            .context("inventory current map missing")?;
        let mut binding_indices = BTreeSet::new();
        let mut ids = BTreeSet::new();
        let mut initial_outcomes = BTreeMap::new();
        for o in &self.initial_completion.outcomes {
            ensure!(
                o.target == o.owner.id
                    && o.owner.incarnation > 0
                    && initial_outcomes.insert(o.target, o).is_none(),
                "inventory initial outcome duplicate or invalid"
            );
        }
        let actors: BTreeMap<_, _> = n.actors.iter().map(|a| (a.id, a)).collect();
        let current_inventories: BTreeMap<_, _> =
            state.owners.iter().map(|o| (o.owner.id, o)).collect();
        ensure!(
            actors.len() == self.resolved.len() && current_inventories.len() == self.resolved.len(),
            "inventory current owner duplicate"
        );
        for (source_index, r) in self.resolved.iter().enumerate() {
            let b = &self
                .bindings
                .owners
                .get(r.binding_index as usize)
                .context("inventory binding index invalid")?
                .scalar
                .owner;
            let rank = map
                .placements
                .iter()
                .position(|p| p.key == b.native_placement)
                .context("inventory placement missing")?;
            ensure!(
                r.source_index as usize == source_index
                    && r.id == DomainId(rank as u64 + 1)
                    && binding_indices.insert(r.binding_index)
                    && ids.insert(r.id),
                "inventory resolution coverage invalid"
            );
            let first = initial_outcomes
                .get(&r.id)
                .context("inventory initial owner outcome missing")?;
            ensure!(
                first.guid == b.native_recorder_guid
                    && first.policy == r.source_policy
                    && first.source
                        == (RecorderSource::Detached {
                            expected_prior_binding: b.expected_prior_binding
                        })
                    && first.before_presence == b.expected_presence
                    && first.before_incarnation == b.expected_incarnation,
                "inventory initial resolution declaration mismatch"
            );
            let expected_incarnation = match first.disposition {
                crate::native_lifecycle::RestoreDisposition::Attached => {
                    ensure!(
                        b.expected_presence == Presence::Live,
                        "inventory initial attachment invalid"
                    );
                    b.expected_incarnation
                }
                crate::native_lifecycle::RestoreDisposition::Spawned => {
                    ensure!(
                        b.expected_presence == Presence::Absent,
                        "inventory initial spawn invalid"
                    );
                    b.expected_incarnation
                        .checked_add(1)
                        .context("inventory initial incarnation overflow")?
                }
                crate::native_lifecycle::RestoreDisposition::Replaced => {
                    ensure!(
                        b.expected_presence == Presence::Live,
                        "inventory initial replacement invalid"
                    );
                    b.expected_incarnation
                        .checked_add(1)
                        .context("inventory initial incarnation overflow")?
                }
                _ => anyhow::bail!("inventory initial disposition unsupported"),
            };
            ensure!(
                first.owner.incarnation == expected_incarnation,
                "inventory initial incarnation invalid"
            );
            let slot = l
                .slots
                .get(rank)
                .context("inventory current slot missing")?;
            let actor = actors
                .get(&r.id)
                .context("inventory current actor missing")?;
            let component = slot
                .component
                .as_ref()
                .context("inventory current component missing")?;
            let handle = OwnerHandle {
                id: r.id,
                incarnation: slot.incarnation,
            };
            ensure!(
                slot.id == r.id
                    && slot.presence == Presence::Live
                    && slot.incarnation >= first.owner.incarnation
                    && component.kind == RecorderKind::SpatialStateV1
                    && component.guid > 0
                    && actor.placement == b.native_placement
                    && actor.class == b.native_class
                    && actor.variables.capacity() <= 4
                    && current_inventories
                        .get(&r.id)
                        .is_some_and(|o| o.owner == handle),
                "inventory current owner invalid"
            );
            // Original native recorder GUIDs may be retired by later replacement.
            // Require the current binding to the current component and handle.
            ensure!(
                l.bindings
                    .iter()
                    .filter(|v| v.owner == handle
                        && v.guid == component.guid
                        && v.kind == component.kind)
                    .count()
                    == 1,
                "inventory current recorder binding invalid"
            );
            let class = s
                .definition
                .classes
                .iter()
                .find(|c| c.native_class == actor.class)
                .context("inventory scalar declaration missing")?;
            crate::native_scalar_owners::evaluate_values(class.authored_max, &actor.variables)?;
        }
        let defined: BTreeSet<_> = i.definition.items.iter().map(|v| v.key.as_str()).collect();
        let mut totals = BTreeSet::new();
        for t in &self.inventory_initial.initial_totals.items {
            ensure!(
                defined.contains(t.item.as_str())
                    && totals.insert(t.item.as_str())
                    && t.count <= 65_536_u64 * 1_000_000,
                "inventory initial totals invalid"
            );
        }
        ensure!(
            totals == defined,
            "inventory initial total coverage incomplete"
        );
        Ok(())
    }
    /// Reconstruct immutable source and replay the SAME private initial path.
    /// Relationships need not be reconstructed: owner receipts are independent
    /// of edges; the caller validates complete current relationships separately.
    pub fn validate(
        &self,
        n: &NativeWorldSnapshot,
        l: &LifecycleSnapshot,
        s: &ScalarOwnerContractsSnapshotV1,
        i: &InventoryContractsSnapshotV1,
        state: &InventoryStateSnapshotV1,
        preserved: &PreservedSection,
    ) -> Result<()> {
        self.validate_current_state(n, l, s, i, state)?;
        let catalog = Arc::new(CompiledNativeCatalog::compile(n.definitions.clone())?);
        let scalar = CompiledScalarOwnerContracts::from_snapshot(s, n)?;
        let inventory = CompiledInventoryContractsV1::from_snapshot(i, &catalog)?;
        let lifecycle = CompiledNativeLifecycle::from_snapshot(l, n)?;
        let bindings = CompiledC001InventoryOwnerBindings::compile(self.bindings.clone())?;
        ensure!(
            bindings.identity() == self.bindings_id && bindings.definition() == &self.bindings,
            "inventory canonical bindings mismatch"
        );
        ensure!(
            self.bindings.expected_payload_sha256 == crate::hex_lower(&preserved.payload_sha256()),
            "inventory preserved identity mismatch"
        );
        let payload = preserved
            .decode()
            .and_then(|p| p.to_inner_payload())
            .map_err(|_| anyhow::anyhow!("inventory source reconstruction invalid"))?;
        let roots = read_root(&payload, PROFILE)
            .map_err(|_| anyhow::anyhow!("inventory source root invalid"))?;
        let (_, root, decisions) = stage_prospect_save_state_with_root_values(
            &roots.tags,
            PROFILE,
            MAX_BOUNDED_COMPONENT_BYTES,
            None,
        )
        .map_err(|_| anyhow::anyhow!("inventory source staging invalid"))?;
        validate_root(&root).map_err(|_| anyhow::anyhow!("inventory source root unsupported"))?;
        validate_source_headers(&payload)
            .map_err(|_| anyhow::anyhow!("inventory source headers invalid"))?;
        ensure!(
            self.root_matches(&root),
            "inventory immutable root mismatch"
        );
        let records = read_recorders(&roots.tags, PROFILE)
            .map_err(|_| anyhow::anyhow!("inventory source recorder framing invalid"))?;
        ensure!(
            records.records().len() == self.resolved.len()
                && decisions.len() == self.resolved.len(),
            "inventory source coverage mismatch"
        );
        let initial =
            prepare_authored_world(catalog, &self.bindings.native_map)?.initial_snapshot();
        let initial_native = initial
            .native_world
            .as_ref()
            .context("inventory initial native missing")?;
        let initial_lifecycle = lifecycle.initial_snapshot();
        let mut admission = Admission::new(&bindings, &scalar);
        for (index, r) in records.records().iter().enumerate() {
            let class = decode_string(r.class_value, ByteOrder::Little)
                .map_err(|_| anyhow::anyhow!("inventory source class invalid"))?;
            ensure!(
                class.consumed == r.class_value.len()
                    && exact(&class.units, C001)
                    && encode_string(&class.units, ByteOrder::Little)
                        .is_ok_and(|v| v == r.class_value),
                "inventory source class unsupported"
            );
            validate_wrapper(r.record, index)
                .map_err(|_| anyhow::anyhow!("inventory source wrapper invalid"))?;
            let source =
                source_inventory_owner(r.binary_data, index, &bindings.scalar, &bindings.inventory)
                    .map_err(|_| anyhow::anyhow!("inventory source component invalid"))?;
            admission.admit(index, source, &decisions[index], initial_native)?;
        }
        ensure!(
            admission.resolved == self.resolved,
            "inventory immutable resolution mismatch"
        );
        let replay = initial_parts(
            initial_native,
            &initial_lifecycle,
            None,
            &admission,
            &inventory,
        )?;
        ensure!(
            replay.lifecycle.completed.as_ref() == Some(&self.initial_completion)
                && replay.receipt == self.inventory_initial,
            "inventory immutable initial receipt mismatch"
        );
        let live = InventoryLiveOwnersV1 {
            owners: l
                .slots
                .iter()
                .map(|v| OwnerHandle {
                    id: v.id,
                    incarnation: v.incarnation,
                })
                .collect(),
        };
        scalar.validate_native(n)?;
        validate_inventory_state(
            &inventory,
            state,
            &self.inventory_initial.initial_totals,
            &live,
        )?;
        Ok(())
    }
}

fn struct_array_shape<'de, D: Deserializer<'de>>(
    d: D,
) -> std::result::Result<StructArrayShapeV1, D::Error> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct F {
        #[serde(deserialize_with = "units")]
        field_name: Vec<u16>,
        #[serde(deserialize_with = "units")]
        element_tag_name: Vec<u16>,
        #[serde(deserialize_with = "units")]
        record_type: Vec<u16>,
    }
    let f: F = crate::json::object(d)
        .map_err(|_| <D::Error as de::Error>::custom("inventory array shape invalid"))?;
    Ok(StructArrayShapeV1 {
        field_name: f.field_name,
        element_tag_name: f.element_tag_name,
        record_type: f.record_type,
    })
}
fn struct_shape<'de, D: Deserializer<'de>>(d: D) -> std::result::Result<StructShapeV1, D::Error> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct F {
        #[serde(deserialize_with = "units")]
        field_name: Vec<u16>,
        #[serde(deserialize_with = "units")]
        record_type: Vec<u16>,
    }
    let f: F = crate::json::object(d)
        .map_err(|_| <D::Error as de::Error>::custom("inventory struct shape invalid"))?;
    Ok(StructShapeV1 {
        field_name: f.field_name,
        record_type: f.record_type,
    })
}
fn inventory_layout<'de, D: Deserializer<'de>>(
    d: D,
) -> std::result::Result<C001InventoryLayoutV1, D::Error> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct F {
        #[serde(deserialize_with = "struct_array_shape")]
        inventories: StructArrayShapeV1,
        #[serde(deserialize_with = "units")]
        inventory_key_field: Vec<u16>,
        #[serde(deserialize_with = "struct_shape")]
        inventory_data: StructShapeV1,
        #[serde(deserialize_with = "struct_array_shape")]
        slots: StructArrayShapeV1,
        #[serde(deserialize_with = "struct_array_shape")]
        dynamic: StructArrayShapeV1,
        #[serde(deserialize_with = "struct_array_shape")]
        additional_stats: StructArrayShapeV1,
        #[serde(deserialize_with = "struct_array_shape")]
        alterations: StructArrayShapeV1,
        #[serde(deserialize_with = "struct_array_shape")]
        living_item_slots: StructArrayShapeV1,
    }
    let f: F = crate::json::object(d)
        .map_err(|_| <D::Error as de::Error>::custom("inventory layout invalid"))?;
    Ok(C001InventoryLayoutV1 {
        inventories: f.inventories,
        inventory_key_field: f.inventory_key_field,
        inventory_data: f.inventory_data,
        slots: f.slots,
        dynamic: f.dynamic,
        additional_stats: f.additional_stats,
        alterations: f.alterations,
        living_item_slots: f.living_item_slots,
    })
}

/// Value-free report of a complete authored fixture preparation.
#[derive(Debug, serde::Serialize)]
pub struct C001InventoryOwnerInspection {
    pub owners: usize,
    pub inventories: usize,
    pub stacks: usize,
    pub deferred_stacks: usize,
    pub payload_bytes: usize,
    pub world_imported: bool,
    pub gameplay_ready: bool,
}

pub struct PreparedC001InventoryOwnerImport {
    snapshot: crate::simulation::WorldSnapshot,
    inspection: C001InventoryOwnerInspection,
}
impl PreparedC001InventoryOwnerImport {
    pub fn snapshot(&self) -> &crate::simulation::WorldSnapshot {
        &self.snapshot
    }
    pub fn inspection(&self) -> &C001InventoryOwnerInspection {
        &self.inspection
    }
    pub fn into_snapshot(self) -> crate::simulation::WorldSnapshot {
        self.snapshot
    }
}

pub fn prepare_c001_inventory_owner_import(
    candidate: &crate::import_candidate::DecodedImportCandidate,
    mut base: crate::simulation::WorldSnapshot,
    bindings: &CompiledC001InventoryOwnerBindings,
    scalar: &crate::native_scalar_owners::CompiledScalarOwnerContracts,
    inventory: &crate::native_inventory::CompiledInventoryContractsV1,
) -> std::result::Result<PreparedC001InventoryOwnerImport, crate::native_import::SemanticImportError>
{
    use crate::native_import::SemanticImportCategory::{Limit, Validation};
    let fail = |field, category| semantic(None, field, category);
    let parts = prepare_c001_inventory_owner_parts(candidate, &base, bindings, scalar, inventory)?;
    let inventory_contracts = inventory.snapshot();
    let allocation = crate::native_inventory::inventory_snapshot_allocation_bytes(
        &inventory_contracts,
        parts.inventories(),
    )
    .and_then(|n| {
        n.checked_add(parts.metadata().allocation_bytes()?)
            .context("inventory snapshot allocation limit")
    })
    .map_err(|_| fail("c001_inventory_owners.allocation", Limit))?;
    if allocation > crate::native_inventory::MAX_INVENTORY_GRAPH_BYTES {
        return Err(fail("c001_inventory_owners.allocation", Limit));
    }
    let inspection = C001InventoryOwnerInspection {
        owners: parts.inventories().owners.len(),
        inventories: parts
            .inventories()
            .owners
            .iter()
            .map(|o| o.inventories.len())
            .sum(),
        stacks: parts
            .inventories()
            .owners
            .iter()
            .flat_map(|o| &o.inventories)
            .map(|i| i.slots.iter().flatten().count() + i.overflow.len())
            .sum(),
        deferred_stacks: parts
            .inventories()
            .owners
            .iter()
            .flat_map(|o| &o.inventories)
            .map(|i| i.overflow.len())
            .sum(),
        payload_bytes: candidate.payload().len(),
        world_imported: false,
        gameplay_ready: false,
    };
    let source = candidate.preserved();
    let preserved = crate::preservation::PreservedSection::from_native_encoding(
        source.native_encoding().to_vec(),
        source.payload_bytes(),
        source.payload_sha256(),
    )
    .map_err(|_| fail("c001_inventory_owners.preserved", Validation))?;
    let (native, lifecycle, inventories, metadata) = parts.into_parts();
    base.schema_version = crate::simulation::SNAPSHOT_SCHEMA_C001_INVENTORY_OWNERS;
    base.native_world = Some(native);
    base.native_lifecycle = Some(lifecycle);
    base.native_scalar_contracts = Some(scalar.snapshot());
    base.native_inventory_contracts = Some(inventory_contracts.as_ref().clone());
    base.native_inventories = Some(inventories);
    base.native_inventory_owner_import = Some(metadata);
    base.preserved_prospect = Some(std::sync::Arc::new(preserved));
    crate::persistence::SnapshotStore::preflight(&base)
        .map_err(|_| fail("c001_inventory_owners.preflight", Validation))?;
    Ok(PreparedC001InventoryOwnerImport {
        snapshot: base,
        inspection,
    })
}

pub fn publish_new_c001_inventory_owner_import(
    prepared: PreparedC001InventoryOwnerImport,
    destination: &std::path::Path,
) -> anyhow::Result<crate::persistence::SaveReceipt> {
    crate::persistence::SnapshotStore::preflight(prepared.snapshot())?;
    crate::persistence::SnapshotStore::open(destination)?.save_new(prepared.snapshot())
}

#[cfg(test)]
mod allocation_tests;
#[cfg(test)]
mod joint_budget_tests;

#[cfg(test)]
mod name_allocation_tests {
    use super::*;

    #[test]
    fn name_capacity_and_decoder_conversion_are_separate_charges() {
        for capacity in [5, 17] {
            let mut name = Vec::with_capacity(capacity);
            name.extend([65; 5]);
            let mut bounds = Bounds {
                names: 0,
                bytes: size_of::<C001InventoryOwnerImportMetadataV1>(),
            };
            bounds.name(&name, true).unwrap();
            // Five units grow to eight in Vec<Unit>, then the decoder charges
            // five more units for conversion. Capacity seventeen must survive.
            assert_eq!(
                bounds.bytes,
                size_of::<C001InventoryOwnerImportMetadataV1>() + capacity.max(8) * 2 + 5 * 2
            );
            assert_eq!(bounds.names, 5);
        }
    }

    #[test]
    fn name_quota_failure_does_not_poison_next_standalone_decode() {
        let invalid = format!(
            "{{\"source_row\":[{}],\"item\":\"private_sentinel\"}}",
            vec!["65"; 1025].join(",")
        );
        let error = serde_json::from_str::<ItemSourceBindingV1>(&invalid).unwrap_err();
        assert!(!error.to_string().contains("private_sentinel"));
        let good: ItemSourceBindingV1 =
            serde_json::from_str(r#"{"source_row":[65],"item":"item"}"#).unwrap();
        assert_eq!(good.source_row, [65]);
    }
}

//! Complete inventory-aware owner restore requests. This module prepares data;
//! lifecycle/runtime composition remains the caller's single transaction.
use crate::{
    native_inventory::{
        InventoryDecodeScope, InventoryStateV1, MAX_INVENTORY_GRAPH_BYTES, MAX_INVENTORY_INSTANCES,
        MAX_INVENTORY_STACKS, inventory_collection_bytes,
    },
    native_lifecycle::{
        Presence, RecorderKind, RecorderSource, ResolvePolicy, RestoreBatch, RestoreRecorder,
    },
    native_scalar_owners::{digest, encoded_size, key, load_text},
    native_world::{NativeTransform, NativeValue, NativeVariableValue},
};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Deserializer, Serialize, de};
use sha2::{Digest, Sha256};
use std::{collections::BTreeSet, fmt, marker::PhantomData, mem::size_of, path::Path};

pub const MAX_INVENTORY_RESTORE_BYTES: usize = 64 * 1024 * 1024;
const OWNERS: usize = 16_384;

/// Shared bounded sequence reader. The seed rejects the next element before
/// decoding it; explicit growth matches the retained-allocation calculation.
pub(crate) fn sequence<'de, D, T, const N: usize>(d: D) -> std::result::Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    struct V<T, const N: usize>(PhantomData<T>);
    struct Seed<T, const N: usize>(usize, PhantomData<T>);
    impl<'de, T: Deserialize<'de>, const N: usize> de::DeserializeSeed<'de> for Seed<T, N> {
        type Value = T;
        fn deserialize<D: Deserializer<'de>>(self, d: D) -> std::result::Result<T, D::Error> {
            if self.0 >= N {
                return Err(<D::Error as de::Error>::custom("inventory sequence limit"));
            }
            T::deserialize(d)
        }
    }
    impl<'de, T: Deserialize<'de>, const N: usize> de::Visitor<'de> for V<T, N> {
        type Value = Vec<T>;
        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("bounded sequence")
        }
        fn visit_seq<A: de::SeqAccess<'de>>(
            self,
            mut a: A,
        ) -> std::result::Result<Self::Value, A::Error> {
            if a.size_hint().is_some_and(|n| n > N) {
                return Err(<A::Error as de::Error>::custom("inventory sequence limit"));
            }
            let mut values = Vec::new();
            while let Some(value) = a.next_element_seed(Seed::<T, N>(values.len(), PhantomData))? {
                if values.len() == values.capacity() {
                    let cap = values
                        .capacity()
                        .checked_mul(2)
                        .map(|n| n.max(4).min(N))
                        .filter(|n| *n > values.capacity())
                        .ok_or_else(|| {
                            <A::Error as de::Error>::custom("inventory sequence limit")
                        })?;
                    let bytes = (cap - values.capacity())
                        .checked_mul(size_of::<T>())
                        .ok_or_else(|| {
                            <A::Error as de::Error>::custom("inventory allocation limit")
                        })?;
                    InventoryDecodeScope::charge_bytes(bytes).map_err(|_| {
                        <A::Error as de::Error>::custom("inventory decode allocation limit")
                    })?;
                    values.try_reserve_exact(cap - values.len()).map_err(|_| {
                        <A::Error as de::Error>::custom("inventory allocation limit")
                    })?;
                }
                values.push(value);
            }
            Ok(values)
        }
    }
    d.deserialize_seq(V::<T, N>(PhantomData))
}
pub(crate) fn text<'de, D: Deserializer<'de>, const N: usize>(
    d: D,
) -> std::result::Result<String, D::Error> {
    struct V<const N: usize>;
    impl<const N: usize> de::Visitor<'_> for V<N> {
        type Value = String;
        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("bounded string")
        }
        fn visit_str<E: de::Error>(self, value: &str) -> std::result::Result<String, E> {
            if value.len() > N {
                return Err(E::custom("inventory string limit"));
            }
            InventoryDecodeScope::charge_bytes(value.len())
                .map_err(|_| E::custom("inventory decode allocation limit"))?;
            Ok(value.to_owned())
        }
    }
    d.deserialize_str(V::<N>)
}

macro_rules! dto {
    (pub struct $name:ident { $($(#[$attr:meta])* pub $field:ident: $ty:ty),* $(,)? }) => {
        #[derive(Clone, PartialEq, Serialize)]
        pub struct $name { $($(#[$attr])* pub $field:$ty),* }
        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(d:D)->std::result::Result<Self,D::Error> {
                let _scope = InventoryDecodeScope::enter();
                #[derive(Deserialize)] #[serde(deny_unknown_fields)]
                struct Fields { $($(#[$attr])* $field:$ty),* }
                let f:Fields=crate::json::object(d).map_err(|_| <D::Error as de::Error>::custom("inventory restore object invalid"))?;
                Ok(Self { $($field:f.$field),* })
            }
        }
        impl fmt::Debug for $name {
            fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result { f.debug_struct(stringify!($name)).finish_non_exhaustive() }
        }
    };
}
dto! { pub struct InventoryOwnerRestoreV1 {
    #[serde(deserialize_with="text::<_,64>")] pub target_placement:String,
    #[serde(deserialize_with="sequence::<_,_,8>")] pub inventories:Vec<InventoryStateV1>
} }
dto! { pub struct InventoryRestoreBatchV1 {
    pub format:u32,
    #[serde(deserialize_with="text::<_,64>")] pub scalar_contract_id:String,
    #[serde(deserialize_with="text::<_,64>")] pub inventory_contract_id:String,
    pub expected_inventory_revision:u64,
    #[serde(deserialize_with="lifecycle")] pub lifecycle:RestoreBatch,
    #[serde(deserialize_with="sequence::<_,_,16384>")] pub owners:Vec<InventoryOwnerRestoreV1>
} }

// Read inherited DTOs through bounded mirrors; their existing standalone
// readers predate streaming nested quotas. Wire shapes and semantics stay exact.
fn lifecycle<'de, D: Deserializer<'de>>(d: D) -> std::result::Result<RestoreBatch, D::Error> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Fields {
        #[serde(deserialize_with = "text::<_,64>")]
        lifecycle_id: String,
        sequence: u64,
        #[serde(deserialize_with = "sequence::<_,_,16384>")]
        recorders: Vec<Command>,
    }
    let f: Fields = crate::json::object(d)
        .map_err(|_| <D::Error as de::Error>::custom("inventory lifecycle object invalid"))?;
    // Convert in place conceptually, without a second element graph. The vector
    // allocation is charged before constructing the native command vector.
    InventoryDecodeScope::charge_bytes(
        f.recorders
            .len()
            .checked_mul(size_of::<RestoreRecorder>())
            .ok_or_else(|| <D::Error as de::Error>::custom("inventory allocation limit"))?,
    )
    .map_err(|_| <D::Error as de::Error>::custom("inventory allocation limit"))?;
    Ok(RestoreBatch {
        lifecycle_id: f.lifecycle_id,
        sequence: f.sequence,
        recorders: f.recorders.into_iter().map(|v| v.0).collect(),
    })
}
struct Command(RestoreRecorder);
impl<'de> Deserialize<'de> for Command {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Fields {
            guid: i32,
            kind: RecorderKind,
            #[serde(deserialize_with = "text::<_,64>")]
            target_placement: String,
            #[serde(deserialize_with = "sequence::<_,_,1024>")]
            owner_name: Vec<u16>,
            policy: ResolvePolicy,
            source: RecorderSource,
            create_if_missing: bool,
            expected_presence: Presence,
            expected_incarnation: u64,
            transform: NativeTransform,
            #[serde(deserialize_with = "sequence::<_,_,64>")]
            variables: Vec<Variable>,
        }
        let f: Fields = crate::json::object(d)
            .map_err(|_| <D::Error as de::Error>::custom("inventory command object invalid"))?;
        InventoryDecodeScope::charge_bytes(
            element_bytes::<NativeVariableValue>(f.variables.len())
                .map_err(|_| <D::Error as de::Error>::custom("inventory allocation limit"))?,
        )
        .map_err(|_| <D::Error as de::Error>::custom("inventory allocation limit"))?;
        Ok(Self(RestoreRecorder {
            guid: f.guid,
            kind: f.kind,
            target_placement: f.target_placement,
            owner_name: f.owner_name,
            policy: f.policy,
            source: f.source,
            create_if_missing: f.create_if_missing,
            expected_presence: f.expected_presence,
            expected_incarnation: f.expected_incarnation,
            transform: f.transform,
            variables: f.variables.into_iter().map(|v| v.0).collect(),
        }))
    }
}
struct Variable(NativeVariableValue);
impl<'de> Deserialize<'de> for Variable {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Fields {
            #[serde(deserialize_with = "text::<_,64>")]
            key: String,
            #[serde(deserialize_with = "value")]
            value: NativeValue,
        }
        let f: Fields = crate::json::object(d)
            .map_err(|_| <D::Error as de::Error>::custom("inventory variable object invalid"))?;
        Ok(Self(NativeVariableValue {
            key: f.key,
            value: f.value,
        }))
    }
}
fn value<'de, D: Deserializer<'de>>(d: D) -> std::result::Result<NativeValue, D::Error> {
    #[derive(Deserialize)]
    #[serde(
        tag = "type",
        content = "value",
        rename_all = "snake_case",
        deny_unknown_fields
    )]
    enum Value {
        Bool(bool),
        I32(i32),
        F32(f32),
        String(#[serde(deserialize_with = "text::<_,4096>")] String),
    }
    Ok(
        match crate::json::object(d)
            .map_err(|_| <D::Error as de::Error>::custom("inventory variable value invalid"))?
        {
            Value::Bool(v) => NativeValue::Bool(v),
            Value::I32(v) => NativeValue::I32(v),
            Value::F32(v) => NativeValue::F32(v),
            Value::String(v) => NativeValue::String(v),
        },
    )
}

pub(crate) fn add_bytes(total: &mut usize, n: usize) -> Result<()> {
    *total = total
        .checked_add(n)
        .filter(|v| *v <= MAX_INVENTORY_GRAPH_BYTES)
        .context("inventory graph allocation limit")?;
    Ok(())
}
pub(crate) fn vector_bytes<T>(values: &Vec<T>, limit: usize) -> Result<usize> {
    let capacity_limit = limit
        .checked_mul(2)
        .and_then(|n| n.checked_add(4))
        .context("inventory vector allocation limit")?;
    ensure!(
        values.len() <= limit && values.capacity() <= capacity_limit,
        "inventory vector allocation limit"
    );
    let mut reload = 0_usize;
    while reload < values.len() {
        reload = reload
            .checked_mul(2)
            .context("inventory vector allocation limit")?
            .max(4)
            .min(limit);
    }
    element_bytes::<T>(values.capacity().max(reload))
}

pub(crate) fn element_bytes<T>(len: usize) -> Result<usize> {
    len.checked_mul(size_of::<T>())
        .context("inventory vector allocation limit")
}

pub(crate) fn lifecycle_bytes(batch: &RestoreBatch) -> Result<usize> {
    batch.validate_bounds()?;
    // RestoreBatch is inline in InventoryRestoreBatchV1, whose enclosing size
    // is charged by both callers. Only its heap graph belongs here.
    let mut bytes = 0;
    add_bytes(&mut bytes, batch.lifecycle_id.capacity())?;
    add_bytes(&mut bytes, vector_bytes(&batch.recorders, OWNERS)?)?;
    // The bounded mirrors charge their growth and the conversion separately,
    // even when collect can reuse a mirror's allocation.
    add_bytes(
        &mut bytes,
        element_bytes::<RestoreRecorder>(batch.recorders.len())?,
    )?;
    for c in &batch.recorders {
        add_bytes(&mut bytes, c.target_placement.capacity())?;
        add_bytes(&mut bytes, vector_bytes(&c.owner_name, 1024)?)?;
        add_bytes(&mut bytes, vector_bytes(&c.variables, 64)?)?;
        add_bytes(
            &mut bytes,
            element_bytes::<NativeVariableValue>(c.variables.len())?,
        )?;
        for v in &c.variables {
            add_bytes(&mut bytes, v.key.capacity())?;
            if let NativeValue::String(s) = &v.value {
                add_bytes(&mut bytes, s.capacity())?;
            }
        }
    }
    Ok(bytes)
}
impl InventoryRestoreBatchV1 {
    pub fn parse(input: &str) -> Result<Self> {
        ensure!(
            input.len() <= MAX_INVENTORY_RESTORE_BYTES,
            "inventory restore input limit"
        );
        let value: Self = if input.trim_start().starts_with('{') {
            serde_json::from_str(input)
                .map_err(|_| anyhow::anyhow!("inventory restore JSON invalid"))?
        } else {
            toml::from_str(input).map_err(|_| anyhow::anyhow!("inventory restore TOML invalid"))?
        };
        value.validate_bounds()?;
        Ok(value)
    }
    pub fn load(path: &Path) -> Result<Self> {
        Self::parse(&load_text(path, MAX_INVENTORY_RESTORE_BYTES)?)
    }
    pub(crate) fn validate_bounds(&self) -> Result<()> {
        self.allocation_bytes().map(|_| ())
    }
    /// Borrowed retained-capacity and decoder-charge bound, checked before clone.
    pub(crate) fn allocation_bytes(&self) -> Result<usize> {
        ensure!(
            self.format == 1 && !self.owners.is_empty(),
            "inventory restore format or coverage invalid"
        );
        digest(&self.scalar_contract_id)?;
        digest(&self.inventory_contract_id)?;
        self.lifecycle.validate_bounds()?;
        let mut bytes = size_of::<Self>();
        add_bytes(&mut bytes, vector_bytes(&self.owners, OWNERS)?)?;
        add_bytes(&mut bytes, lifecycle_bytes(&self.lifecycle)?)?;
        add_bytes(&mut bytes, self.scalar_contract_id.capacity())?;
        add_bytes(&mut bytes, self.inventory_contract_id.capacity())?;
        let targets: BTreeSet<_> = self
            .lifecycle
            .recorders
            .iter()
            .map(|c| c.target_placement.as_str())
            .collect();
        let mut seen = BTreeSet::new();
        let (mut instances, mut stacks) = (0_usize, 0_usize);
        for owner in &self.owners {
            key(&owner.target_placement)?;
            ensure!(
                targets.contains(owner.target_placement.as_str())
                    && seen.insert(owner.target_placement.as_str()),
                "inventory restore target coverage invalid"
            );
            add_bytes(&mut bytes, owner.target_placement.capacity())?;
            add_bytes(&mut bytes, inventory_collection_bytes(&owner.inventories)?)?;
            instances = instances
                .checked_add(owner.inventories.len())
                .filter(|n| *n <= MAX_INVENTORY_INSTANCES)
                .context("inventory instance limit")?;
            let mut keys = BTreeSet::new();
            for inv in &owner.inventories {
                ensure!(
                    keys.insert(inv.key.as_str()),
                    "inventory restore duplicate key"
                );
                for stack in inv
                    .slots
                    .iter()
                    .flatten()
                    .chain(inv.overflow.iter().map(|d| &d.stack))
                {
                    ensure!(
                        (1..=1_000_000).contains(&stack.count),
                        "inventory restore stack count limit"
                    );
                }
                ensure!(
                    inv.overflow.iter().all(|d| d.requested_slot <= 255),
                    "inventory restore deferred location limit"
                );
                stacks = stacks
                    .checked_add(
                        inv.slots
                            .iter()
                            .filter(|v| v.is_some())
                            .count()
                            .checked_add(inv.overflow.len())
                            .context("inventory stack limit")?,
                    )
                    .filter(|n| *n <= MAX_INVENTORY_STACKS)
                    .context("inventory stack limit")?;
            }
        }
        ensure!(
            seen.len() == targets.len(),
            "inventory restore target coverage incomplete"
        );
        encoded_size(self, MAX_INVENTORY_RESTORE_BYTES)?;
        Ok(bytes)
    }
    /// Sort declarations only. Slots and deferred order remain significant.
    pub fn canonical(&self) -> Result<Self> {
        self.validate_bounds()?;
        let mut value = self.clone();
        value.lifecycle = self.lifecycle.canonical()?;
        value
            .owners
            .sort_by(|a, b| a.target_placement.cmp(&b.target_placement));
        for owner in &mut value.owners {
            owner.inventories.sort_by(|a, b| a.key.cmp(&b.key));
        }
        Ok(value)
    }
    pub fn request_digest(&self) -> Result<String> {
        let canonical = self.canonical()?;
        encoded_size(&canonical, MAX_INVENTORY_RESTORE_BYTES)?;
        // Stream canonical bytes directly into SHA256 after count-only preflight.
        struct Writer(Sha256);
        impl std::io::Write for Writer {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.0.update(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let mut w = Writer(Sha256::new());
        w.0.update(b"icarus/native_inventory_owner_restore/v1\0");
        serde_json::to_writer(&mut w, &canonical)
            .map_err(|_| anyhow::anyhow!("inventory restore encoding invalid"))?;
        Ok(crate::hex_lower(&w.0.finalize()))
    }
}

/// Complete owner/inventory preparation, private until all validation succeeds.
pub struct PreparedInventoryOwnerRestore {
    snapshot: crate::simulation::WorldSnapshot,
    already_completed: bool,
}

impl PreparedInventoryOwnerRestore {
    pub fn snapshot(&self) -> &crate::simulation::WorldSnapshot {
        &self.snapshot
    }
    pub fn into_snapshot(self) -> crate::simulation::WorldSnapshot {
        self.snapshot
    }
    pub fn already_completed(&self) -> bool {
        self.already_completed
    }
}

/// Prepare owner and inventory state under one complete request identity.
/// This does not acknowledge durability or mutate a running app.
pub fn prepare_inventory_owner_restore(
    current: &crate::simulation::WorldSnapshot,
    request: &InventoryRestoreBatchV1,
) -> anyhow::Result<PreparedInventoryOwnerRestore> {
    use crate::{
        native_inventory::{
            CompiledInventoryContractsV1, InventoryLiveOwnersV1, InventoryRestoreRequestV1,
            OwnerInventoriesV1, prepare_inventory_restore,
        },
        native_lifecycle::{OwnerHandle, Presence, RecorderSource, stage_owner_restore_parts},
        native_world::CompiledNativeCatalog,
        persistence::SnapshotStore,
        simulation::SNAPSHOT_SCHEMA_C001_INVENTORY_OWNERS,
    };
    ensure!(
        current.schema_version == SNAPSHOT_SCHEMA_C001_INVENTORY_OWNERS,
        "inventory owner schema required"
    );
    // Full publication admission also enforces the combined decoder allowance.
    SnapshotStore::preflight(current)?;
    let inventory_bytes = crate::native_inventory::inventory_snapshot_allocation_bytes(
        current
            .native_inventory_contracts
            .as_ref()
            .context("inventory contracts missing")?,
        current
            .native_inventories
            .as_ref()
            .context("inventory state missing")?,
    )?;
    let request_bytes = request.allocation_bytes()?;
    let combined = inventory_bytes
        .checked_add(
            current
                .native_inventory_owner_import
                .as_ref()
                .context("inventory provenance missing")?
                .allocation_bytes()?,
        )
        .and_then(|n| n.checked_add(request_bytes))
        .context("inventory restore allocation limit")?;
    ensure!(
        combined <= crate::native_inventory::MAX_INVENTORY_GRAPH_BYTES,
        "inventory restore allocation limit"
    );
    let request = request.canonical()?;
    let native = current
        .native_world
        .as_ref()
        .context("inventory native world missing")?;
    let lifecycle = current
        .native_lifecycle
        .as_ref()
        .context("inventory lifecycle missing")?;
    let state = current
        .native_inventories
        .as_ref()
        .context("inventory state missing")?;
    let contracts = current
        .native_inventory_contracts
        .as_ref()
        .context("inventory contracts missing")?;
    let metadata = current
        .native_inventory_owner_import
        .as_ref()
        .context("inventory provenance missing")?;
    ensure!(
        request.scalar_contract_id
            == current
                .native_scalar_contracts
                .as_ref()
                .context("inventory scalar contracts missing")?
                .contract_id
            && request.inventory_contract_id == contracts.contract_id,
        "inventory restore contract mismatch"
    );
    let staged = stage_owner_restore_parts(
        native,
        lifecycle,
        current.native_relationships.as_ref(),
        &request.lifecycle,
        &request.request_digest()?,
    )?;
    // Exact retry precedes stale revision/handle expectations, but follows full
    // current-world validation. It preserves all subsequent consumption/edits.
    if staged.already_completed {
        return Ok(PreparedInventoryOwnerRestore {
            snapshot: current.clone(),
            already_completed: true,
        });
    }
    let mut replacements = Vec::with_capacity(request.owners.len());
    for owner in &request.owners {
        let actor = native
            .actors
            .iter()
            .find(|a| a.placement == owner.target_placement)
            .context("inventory restore placement missing")?;
        let slot = staged
            .lifecycle
            .slots
            .iter()
            .find(|s| s.id == actor.id)
            .context("inventory restored slot missing")?;
        let command = request
            .lifecycle
            .recorders
            .iter()
            .find(|c| c.target_placement == owner.target_placement)
            .context("inventory restore command missing")?;
        if matches!(command.source, RecorderSource::Bound) {
            let previous = state
                .owners
                .iter()
                .find(|o| o.owner.id == actor.id)
                .context("bound inventory owner missing")?;
            ensure!(
                previous.inventories.len() == owner.inventories.len()
                    && owner
                        .inventories
                        .iter()
                        .all(|i| previous.inventories.iter().any(|p| p == i)),
                "bound inventory state differs"
            );
        }
        replacements.push(OwnerInventoriesV1 {
            owner: OwnerHandle {
                id: slot.id,
                incarnation: slot.incarnation,
            },
            inventories: owner.inventories.clone(),
        });
    }
    let live = |snapshot: &crate::native_lifecycle::LifecycleSnapshot| InventoryLiveOwnersV1 {
        owners: snapshot
            .slots
            .iter()
            .filter(|s| s.presence == Presence::Live)
            .map(|s| OwnerHandle {
                id: s.id,
                incarnation: s.incarnation,
            })
            .collect(),
    };
    let catalog = CompiledNativeCatalog::compile(native.definitions.clone())?;
    let compiled = CompiledInventoryContractsV1::from_snapshot(contracts, &catalog)?;
    let inventories = prepare_inventory_restore(
        &compiled,
        state,
        &metadata.inventory_initial.initial_totals,
        &live(lifecycle),
        &live(&staged.lifecycle),
        &InventoryRestoreRequestV1 {
            expected_inventory_revision: request.expected_inventory_revision,
            owners: replacements,
        },
    )?;
    let mut snapshot = current.clone();
    snapshot.native_world = Some(staged.native);
    snapshot.native_lifecycle = Some(staged.lifecycle);
    snapshot.native_inventories = Some(inventories.into_state());
    SnapshotStore::preflight(&snapshot)?;
    Ok(PreparedInventoryOwnerRestore {
        snapshot,
        already_completed: false,
    })
}

#[cfg(test)]
mod allocation_tests {
    use super::*;

    #[test]
    fn borrowed_request_counts_capacities_growth_and_both_mirror_conversions() {
        let mut owner_name = Vec::with_capacity(17);
        owner_name.extend([65; 5]);
        let mut text = String::with_capacity(64);
        text.push_str("value");
        let mut variables = Vec::with_capacity(7);
        variables.extend([
            NativeVariableValue {
                key: "enabled".into(),
                value: NativeValue::Bool(true),
            },
            NativeVariableValue {
                key: "label".into(),
                value: NativeValue::String(text),
            },
        ]);
        let command = RestoreRecorder {
            guid: 1,
            kind: RecorderKind::SpatialStateV1,
            target_placement: "owner".into(),
            owner_name,
            policy: ResolvePolicy::FindOnly,
            source: RecorderSource::Bound,
            create_if_missing: false,
            expected_presence: Presence::Live,
            expected_incarnation: 1,
            transform: NativeTransform {
                translation: [0.; 3],
                rotation: [0., 0., 0., 1.],
                scale: [1.; 3],
            },
            variables,
        };
        let mut owners = Vec::with_capacity(6);
        owners.push(InventoryOwnerRestoreV1 {
            target_placement: "owner".into(),
            inventories: Vec::new(),
        });
        let request = InventoryRestoreBatchV1 {
            format: 1,
            scalar_contract_id: "a".repeat(64),
            inventory_contract_id: "b".repeat(64),
            expected_inventory_revision: 0,
            lifecycle: RestoreBatch {
                lifecycle_id: "c".repeat(64),
                sequence: 1,
                recorders: vec![command],
            },
            owners,
        };
        let c = &request.lifecycle.recorders[0];
        // Independent totals for this concrete shape: the JSON readers grow
        // one-element vectors to four; spare caller capacity wins where larger.
        let expected = size_of::<InventoryRestoreBatchV1>()
            + request.scalar_contract_id.capacity()
            + request.inventory_contract_id.capacity()
            + request.lifecycle.lifecycle_id.capacity()
            + 4 * size_of::<RestoreRecorder>()
            + size_of::<RestoreRecorder>()
            + c.target_placement.capacity()
            + 17 * size_of::<u16>()
            + 7 * size_of::<NativeVariableValue>()
            + 2 * size_of::<NativeVariableValue>()
            + c.variables.iter().map(|v| v.key.capacity()).sum::<usize>()
            + 64
            + 6 * size_of::<InventoryOwnerRestoreV1>()
            + size_of::<Vec<InventoryStateV1>>()
            + request.owners[0].target_placement.capacity();
        assert_eq!(request.allocation_bytes().unwrap(), expected);
        let json = serde_json::to_string(&request).unwrap();
        assert_eq!(
            request.request_digest().unwrap(),
            InventoryRestoreBatchV1::parse(&json)
                .unwrap()
                .request_digest()
                .unwrap()
        );
    }

    #[test]
    fn arithmetic_overflow_is_value_free() {
        assert!(element_bytes::<RestoreRecorder>(usize::MAX).is_err());
        assert!(vector_bytes(&Vec::<u16>::new(), usize::MAX).is_err());
        let mut total = usize::MAX;
        let error = add_bytes(&mut total, 1).unwrap_err();
        assert_eq!(error.to_string(), "inventory graph allocation limit");
    }
}

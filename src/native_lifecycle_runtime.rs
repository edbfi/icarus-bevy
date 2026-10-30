//! Authored declared-slot owners. Live spatial values belong to ECS components;
//! dormant values have a disjoint resource. Nothing here imports official state.
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};

use anyhow::{Context, Result, ensure};
use bevy::prelude::{Component, Entity, Resource, World};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::{
    native_c001_inventory_owners::C001InventoryOwnerImportMetadataV1,
    native_c001_owners::C001OwnerImportMetadataV1,
    native_c001_scalar_owners::C001ScalarOwnerImportMetadataV1,
    native_inventory::{
        CompiledInventoryContractsV1, InventoryActionOutcomeV1, InventoryActionRequestV1,
        InventoryContractsSnapshotV1, InventoryLiveOwnersV1, InventoryStateSnapshotV1,
        InventoryStateV1, ItemTotalV1, MAX_INVENTORY_GRAPH_BYTES, MAX_INVENTORY_INSTANCES,
        MAX_INVENTORY_ITEMS, MAX_INVENTORY_OWNERS, MAX_INVENTORY_STACKS, MAX_INVENTORY_STATE_BYTES,
        OwnerInventoriesV1, StackId, StackV1, inventory_collection_bytes,
        inventory_snapshot_allocation_bytes, prepare_inventory_action, validate_inventory_state,
    },
    native_inventory_restore::{InventoryRestoreBatchV1, prepare_inventory_owner_restore},
    native_lifecycle::{
        CompiledNativeLifecycle, CompletedRestore, LifecycleSnapshot, OwnerHandle, Presence,
        RecorderComponentRecord, RestoreBatch, prepare_owner_restore,
    },
    native_modifiers::{
        MAX_MODIFIER_OWNERS, MAX_MODIFIER_WORLD_BYTES, MAX_WORLD_MODIFIERS, ModifierAction,
        ModifierRequest, ModifierWorldSnapshot, OwnerModifierState, prepare_modifiers,
    },
    native_relationships::{
        CompiledNativeRelationships, NativeFoundationRecord, NativeRelationshipRole,
        NativeRelationshipsSnapshot,
    },
    native_runtime::{
        NativeActorIdentity, NativeFoundation, NativeFoundationDependents, NativeLocalBounds,
        NativeRelationshipIdentity, NativeRelationshipsContext, NativeRotation, NativeScale,
        NativeVariables, NativeWorldContext,
    },
    native_scalar_owners::{
        CompiledScalarOwnerContracts, ScalarOwnerContractsSnapshotV1, ScalarOwnerState,
        ScalarUseOutcome, ScalarValues,
    },
    native_world::{
        Aabb3, CompiledNativeCatalog, NativeActorRecord, NativeActorView, NativeTransform,
        NativeValue, NativeVariableValue, NativeWorldSnapshot,
    },
    persistence::SnapshotStore,
    simulation::{
        DomainId, Position, PreservedProspectState, SNAPSHOT_SCHEMA_C001_INVENTORY_OWNERS,
        SNAPSHOT_SCHEMA_C001_OWNERS, SNAPSHOT_SCHEMA_C001_SCALAR_OWNERS, SNAPSHOT_SCHEMA_LIFECYCLE,
        SNAPSHOT_SCHEMA_MODIFIER_OWNERS, SimulationClock, WorldSnapshot,
    },
};

#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub struct NativeOwnerIncarnation(pub u64);

/// Exact authored identity, separate from foundation link names and GUIDs.
#[derive(Component, Clone, PartialEq)]
pub struct NativeOwnerName(pub Vec<u16>);

#[derive(Component, Clone, PartialEq)]
pub struct NativeOwnerRecorder(pub RecorderComponentRecord);

/// Immutable authored source provenance. Capture checks this resource against
/// the privately pinned activation; it never becomes mutable owner authority.
#[derive(Resource, Clone)]
pub struct C001OwnerImportState(pub Arc<C001OwnerImportMetadataV1>);

/// Immutable declarations and provenance, checked against the activation pins.
#[derive(Resource, Clone)]
pub struct NativeScalarOwnerContractsState(pub Arc<ScalarOwnerContractsSnapshotV1>);

#[derive(Resource, Clone)]
pub struct C001ScalarOwnerImportState(pub Arc<C001ScalarOwnerImportMetadataV1>);

#[derive(Resource, Clone)]
pub struct NativeInventoryContractsState(pub Arc<InventoryContractsSnapshotV1>);

#[derive(Resource, Clone)]
pub struct C001InventoryOwnerImportState(pub Arc<C001InventoryOwnerImportMetadataV1>);

/// Inventory values have exactly one authority, on the registered live owner.
#[derive(Component, Clone)]
struct InventoryOwnerState {
    activation: Arc<Definitions>,
    generation: u64,
    owner: OwnerHandle,
    inventories: Vec<InventoryStateV1>,
}

#[derive(Component, Clone)]
struct ModifierOwnerState {
    activation: Arc<Definitions>,
    generation: u64,
    state: OwnerModifierState,
}

/// Non-clonable runtime authority, independent of public capture/context copies.
#[derive(Resource)]
struct ModifierRuntimeAnchor {
    definitions: Arc<Definitions>,
}

#[derive(Clone)]
struct InventoryHeader {
    revision: u64,
    next_stack_id: u64,
    consumed: Vec<ItemTotalV1>,
    // Covers the complete borrowed graph, including header and every owner.
    // Capture checks this before cloning caller-corruptible component values.
    fingerprint: [u8; 32],
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SourceMode {
    Lifecycle,
    Owners,
    ScalarOwners,
    InventoryOwners,
    ModifierOwners,
}

impl SourceMode {
    fn schema(self) -> u32 {
        match self {
            Self::Lifecycle => SNAPSHOT_SCHEMA_LIFECYCLE,
            Self::Owners => SNAPSHOT_SCHEMA_C001_OWNERS,
            Self::ScalarOwners => SNAPSHOT_SCHEMA_C001_SCALAR_OWNERS,
            Self::InventoryOwners => SNAPSHOT_SCHEMA_C001_INVENTORY_OWNERS,
            Self::ModifierOwners => SNAPSHOT_SCHEMA_MODIFIER_OWNERS,
        }
    }
}

#[derive(Component, Clone)]
struct ScalarOwnerDerived {
    activation: Arc<Definitions>,
    generation: u64,
    owner: OwnerHandle,
    input_fingerprint: [u8; 32],
    available: bool,
}

struct Definitions {
    // Shared by every activation pin, including clones of SnapshotMode. A
    // stale resource cannot roll this back by restoring its observed generation.
    current_generation: AtomicU64,
    modifier_failed: AtomicBool,
    mode: SourceMode,
    catalog: Arc<CompiledNativeCatalog>,
    lifecycle: CompiledNativeLifecycle,
    relationships: Option<CompiledNativeRelationships>,
    identities: BTreeMap<DomainId, NativeRelationshipIdentity>,
    imported: Option<Arc<C001OwnerImportMetadataV1>>,
    scalar_contracts: Option<CompiledScalarOwnerContracts>,
    scalar_snapshot: Option<Arc<ScalarOwnerContractsSnapshotV1>>,
    scalar_imported: Option<Arc<C001ScalarOwnerImportMetadataV1>>,
    inventory_contracts: Option<CompiledInventoryContractsV1>,
    inventory_snapshot: Option<Arc<InventoryContractsSnapshotV1>>,
    inventory_imported: Option<Arc<C001InventoryOwnerImportMetadataV1>>,
    inventory_pinned_bytes: usize,
}

/// Private fields prevent callers from updating a registry independently of its
/// actors. Cloning a context never grants authority to a different activation.
#[derive(Resource, Clone)]
pub struct NativeLifecycleContext {
    definitions: Arc<Definitions>,
    generation: u64,
    lifecycle: LifecycleSnapshot,
    live: BTreeMap<DomainId, Entity>,
    inventory: Option<InventoryHeader>,
}

impl NativeLifecycleContext {
    pub(crate) fn same_activation(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.definitions, &other.definitions)
            && other.generation == self.definitions.current_generation.load(Ordering::Relaxed)
    }

    pub(crate) fn replication_identity(&self) -> (&str, &str, &str) {
        (
            self.definitions.catalog.identity(),
            &self.lifecycle.definition.map,
            self.definitions.lifecycle.identity(),
        )
    }
}

/// Validated authored slot, including dormant placements with no live entity.
/// The incarnation belongs to the live owner, never to its spatial declaration.
pub(crate) struct LifecycleReplicationSlot {
    pub id: DomainId,
    pub placement: String,
    pub class: String,
    pub tile: String,
    pub transform: NativeTransform,
    pub live: Option<(Entity, u64)>,
}

/// Capture the complete lifecycle world once before exposing any spatial slot.
/// A retained activation pin accepts legitimate generation advances, but never
/// a different loaded contract or a stale context reinserted into the world.
pub(crate) fn replication_slots(
    world: &World,
    activation: &NativeLifecycleContext,
) -> Result<Vec<LifecycleReplicationSlot>> {
    use bevy::ecs::entity_disabling::Disabled;

    let context = world
        .get_resource::<NativeLifecycleContext>()
        .context("lifecycle replication context missing")?;
    ensure!(
        activation.same_activation(context),
        "lifecycle replication activation changed"
    );
    ensure!(
        matches!(
            context.definitions.mode,
            SourceMode::Lifecycle | SourceMode::ModifierOwners
        ),
        "lifecycle replication requires an authored schema-7 or schema-11 world"
    );
    let snapshot = capture_lifecycle(world)?;
    ensure!(
        matches!(
            snapshot.schema_version,
            SNAPSHOT_SCHEMA_LIFECYCLE | SNAPSHOT_SCHEMA_MODIFIER_OWNERS
        ) && snapshot.preserved_prospect.is_none()
            && snapshot.native_import.is_none()
            && snapshot.native_owner_import.is_none()
            && snapshot.native_scalar_contracts.is_none()
            && snapshot.native_scalar_owner_import.is_none()
            && snapshot.native_inventory_contracts.is_none()
            && snapshot.native_inventory_owner_import.is_none()
            && snapshot.native_inventories.is_none(),
        "lifecycle replication requires authored state without imports or preserved state"
    );
    let lifecycle = snapshot
        .native_lifecycle
        .context("lifecycle replication slots missing")?;
    let native = snapshot
        .native_world
        .context("lifecycle replication spatial state missing")?;
    let mut actors: BTreeMap<_, _> = native
        .actors
        .into_iter()
        .map(|actor| (actor.id, actor))
        .collect();
    ensure!(
        actors.len() == lifecycle.slots.len(),
        "lifecycle replication spatial membership differs from slots"
    );
    let mut slots = Vec::with_capacity(lifecycle.slots.len());
    for slot in lifecycle.slots {
        let actor = actors
            .remove(&slot.id)
            .context("lifecycle replication slot lacks spatial state")?;
        let live = match slot.presence {
            Presence::Absent => {
                ensure!(
                    !context.live.contains_key(&slot.id),
                    "dormant replication slot has a live owner"
                );
                None
            }
            Presence::Live => {
                let entity = *context
                    .live
                    .get(&slot.id)
                    .context("live replication slot lacks an owner")?;
                ensure!(
                    world.get::<Disabled>(entity).is_none(),
                    "live replication owner is disabled"
                );
                Some((entity, slot.incarnation))
            }
        };
        slots.push(LifecycleReplicationSlot {
            id: actor.id,
            placement: actor.placement,
            class: actor.class,
            tile: actor.tile,
            transform: actor.transform,
            live,
        });
    }
    slots.sort_unstable_by_key(|slot| slot.id);
    Ok(slots)
}

#[derive(Resource, Clone)]
pub struct DormantNativeSlots {
    definitions: Arc<Definitions>,
    generation: u64,
    records: BTreeMap<DomainId, NativeActorRecord>,
}

/// An in-memory transition is distinct from a durable SaveReceipt.
#[derive(Clone, Debug, PartialEq)]
pub enum NativeRestoreResult {
    AppliedInMemory(CompletedRestore),
    AlreadyCompleted(CompletedRestore),
}

pub(crate) fn ensure_no_lifecycle_state(world: &World) -> Result<()> {
    ensure!(
        !world.contains_resource::<ModifierRuntimeAnchor>()
            && !world.contains_resource::<NativeLifecycleContext>()
            && !world.contains_resource::<DormantNativeSlots>()
            && !world.contains_resource::<C001OwnerImportState>()
            && !world.contains_resource::<NativeScalarOwnerContractsState>()
            && !world.contains_resource::<C001ScalarOwnerImportState>()
            && !world.contains_resource::<NativeInventoryContractsState>()
            && !world.contains_resource::<C001InventoryOwnerImportState>(),
        "world contains unexpected lifecycle resources"
    );
    // Resolve from this world on every call: even a previously unregistered
    // type may have been inserted since the last clean validation.
    let forbidden = [
        world.component_id::<ModifierRuntimeAnchor>(),
        world.component_id::<NativeOwnerIncarnation>(),
        world.component_id::<NativeOwnerName>(),
        world.component_id::<NativeOwnerRecorder>(),
        world.component_id::<NativeLifecycleContext>(),
        world.component_id::<DormantNativeSlots>(),
        world.component_id::<C001OwnerImportState>(),
        world.component_id::<NativeScalarOwnerContractsState>(),
        world.component_id::<C001ScalarOwnerImportState>(),
        world.component_id::<ScalarOwnerDerived>(),
        world.component_id::<NativeInventoryContractsState>(),
        world.component_id::<C001InventoryOwnerImportState>(),
        world.component_id::<InventoryOwnerState>(),
        world.component_id::<ModifierOwnerState>(),
    ];
    for archetype in world.archetypes().iter().filter(|a| !a.is_empty()) {
        ensure!(
            !forbidden.iter().flatten().any(|id| archetype.contains(*id)),
            "world contains unexpected lifecycle components"
        );
    }
    Ok(())
}

fn require_separate_mode(world: &World) -> Result<()> {
    ensure!(
        !world.contains_resource::<NativeWorldContext>()
            && !world.contains_resource::<NativeRelationshipsContext>(),
        "lifecycle world contains fixed-topology contexts"
    );
    Ok(())
}

/// Commit rewrites every live actor, including owners outside a restore batch,
/// and updates singleton values. Admit no observable lifecycle
/// callbacks on that boundary: a hook/observer can write another owner or queue
/// a despawn which Bevy flushes before the next `entity_mut` in commit.
///
/// This is deliberately conservative about hooks on an affected archetype.
/// Unflushed registrations/commands and arbitrary Drop side effects are outside
/// this check; callers must keep those out of the mutation boundary. Activation has
/// its own caller-owned world setup boundary; this checks subsequent mutations.
fn require_callback_free_commit(world: &World, context: &NativeLifecycleContext) -> Result<()> {
    use bevy::ecs::lifecycle::{ADD, DESPAWN, DISCARD, INSERT, REMOVE};

    let mut targets: BTreeSet<_> = context.live.values().copied().collect();
    targets.extend(world.resource_entity::<NativeLifecycleContext>());
    targets.extend(world.resource_entity::<DormantNativeSlots>());
    targets.extend(world.resource_entity::<NativeFoundationDependents>());
    let mut components = BTreeSet::new();
    for target in context.live.values() {
        let entity = world.get_entity(*target).context("commit target missing")?;
        let archetype = entity.archetype();
        ensure!(
            !archetype.has_add_hook()
                && !archetype.has_insert_hook()
                && !archetype.has_discard_hook()
                && !archetype.has_remove_hook()
                && !archetype.has_despawn_hook(),
            "lifecycle commit has unsupported ECS component hook"
        );
        components.extend(entity.archetype().components().iter().copied());
    }
    // Include optional components which may first be inserted by this commit,
    // so component observers do not escape admission simply by watching a type
    // absent from the current live actors.
    components.extend(
        [
            world.component_id::<DomainId>(),
            world.component_id::<Position>(),
            world.component_id::<NativeActorIdentity>(),
            world.component_id::<NativeRotation>(),
            world.component_id::<NativeScale>(),
            world.component_id::<NativeVariables>(),
            world.component_id::<NativeLocalBounds>(),
            world.component_id::<NativeOwnerName>(),
            world.component_id::<NativeOwnerIncarnation>(),
            world.component_id::<NativeOwnerRecorder>(),
            world.component_id::<NativeRelationshipIdentity>(),
            world.component_id::<NativeFoundation>(),
            world.component_id::<ScalarOwnerDerived>(),
            world.component_id::<InventoryOwnerState>(),
            world.component_id::<ModifierOwnerState>(),
        ]
        .into_iter()
        .flatten(),
    );
    // Even hook-free required components can execute arbitrary constructors
    // when an optional component first appears. Reject requirements entirely:
    // their constructors cannot be staged without executing caller-owned code.
    // Probe cloned hook metadata through its fallible setters, observing occupied
    // slots without installing or calling hooks in the actual world.
    fn probe(_: bevy::ecs::world::DeferredWorld, _: bevy::ecs::lifecycle::HookContext) {}
    for component in &components {
        let info = world
            .components()
            .get_info(*component)
            .context("commit component metadata missing")?;
        ensure!(
            info.required_components().iter_ids().next().is_none(),
            "lifecycle commit has unsupported ECS component requirement"
        );
        let mut hooks = info.hooks().clone();
        ensure!(
            hooks.try_on_add(probe).is_some()
                && hooks.try_on_insert(probe).is_some()
                && hooks.try_on_discard(probe).is_some()
                && hooks.try_on_remove(probe).is_some()
                && hooks.try_on_despawn(probe).is_some(),
            "lifecycle commit has unsupported ECS component hook"
        );
    }
    // Resources have Bevy's required IsResource hooks. Commit updates their
    // existing values directly instead of inserting/replacing ECS components,
    // so neither those hooks nor user resource hooks are invoked.
    components.extend(
        [
            world.component_id::<NativeLifecycleContext>(),
            world.component_id::<DormantNativeSlots>(),
            world.component_id::<NativeFoundationDependents>(),
        ]
        .into_iter()
        .flatten(),
    );
    // Archetype observer flags only index component observers; they do not
    // cover global or entity-only observers. Use the public central caches and
    // match entity targets explicitly, including disabled managed owners.
    for event in [ADD, INSERT, DISCARD, REMOVE, DESPAWN] {
        let Some(observers) = world.observers().try_get_observers(event) else {
            continue;
        };
        ensure!(
            observers.global_observers().is_empty()
                && !observers
                    .entity_observers()
                    .iter()
                    .any(|(entity, callbacks)| {
                        targets.contains(entity) && !callbacks.is_empty()
                    })
                && !observers
                    .component_observers()
                    .iter()
                    .any(|(component, callbacks)| {
                        components.contains(component)
                            && (!callbacks.global_observers().is_empty()
                                || callbacks.entity_component_observers().iter().any(
                                    |(entity, callbacks)| {
                                        targets.contains(entity) && !callbacks.is_empty()
                                    },
                                ))
                    }),
            "lifecycle commit has unsupported ECS lifecycle observer"
        );
    }
    Ok(())
}

struct StagedActor {
    record: NativeActorRecord,
    bounds: Aabb3,
    name: Vec<u16>,
    incarnation: u64,
    recorder: Option<RecorderComponentRecord>,
    relationship: Option<NativeRelationshipIdentity>,
    foundation: Option<NativeFoundation>,
    scalar: Option<ScalarOwnerDerived>,
    inventory: Option<InventoryOwnerState>,
    modifier: Option<ModifierOwnerState>,
}

struct StagedWorld {
    generation: u64,
    actors: Vec<StagedActor>,
    dormant: BTreeMap<DomainId, NativeActorRecord>,
    dependents: Option<NativeFoundationDependents>,
    inventory: Option<InventoryHeader>,
}

#[derive(Serialize)]
struct BorrowedInventoryOwner<'a> {
    owner: OwnerHandle,
    inventories: &'a Vec<InventoryStateV1>,
}

#[derive(Serialize)]
struct BorrowedInventories<'a> {
    format: u32,
    contract_id: &'a str,
    revision: u64,
    next_stack_id: u64,
    consumed: &'a Vec<ItemTotalV1>,
    owners: Vec<BorrowedInventoryOwner<'a>>,
}

fn inventory_fingerprint(state: &BorrowedInventories<'_>) -> Result<[u8; 32]> {
    struct CounterHash {
        bytes: usize,
        hash: Sha256,
    }
    impl std::io::Write for CounterHash {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.bytes = self
                .bytes
                .checked_add(bytes.len())
                .filter(|bytes| *bytes <= MAX_INVENTORY_STATE_BYTES)
                .ok_or_else(|| std::io::Error::other("inventory state byte limit"))?;
            self.hash.update(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut writer = CounterHash {
        bytes: 0,
        hash: Sha256::new(),
    };
    writer.hash.update(b"icarus/native_inventory_runtime/v1\0");
    serde_json::to_writer(&mut writer, state)
        .map_err(|_| anyhow::anyhow!("inventory state encoding limit"))?;
    Ok(writer.hash.finalize().into())
}

fn inventory_header(state: &InventoryStateSnapshotV1) -> Result<InventoryHeader> {
    let mut owners: Vec<_> = state
        .owners
        .iter()
        .map(|owner| BorrowedInventoryOwner {
            owner: owner.owner,
            inventories: &owner.inventories,
        })
        .collect();
    owners.sort_by_key(|owner| owner.owner.id);
    let fingerprint = inventory_fingerprint(&BorrowedInventories {
        format: state.format,
        contract_id: &state.contract_id,
        revision: state.revision,
        next_stack_id: state.next_stack_id,
        consumed: &state.consumed,
        owners,
    })?;
    Ok(InventoryHeader {
        revision: state.revision,
        next_stack_id: state.next_stack_id,
        consumed: state.consumed.clone(),
        fingerprint,
    })
}

fn live_inventory_owners(lifecycle: &LifecycleSnapshot) -> InventoryLiveOwnersV1 {
    InventoryLiveOwnersV1 {
        owners: lifecycle
            .slots
            .iter()
            .filter(|slot| slot.presence == Presence::Live)
            .map(|slot| OwnerHandle {
                id: slot.id,
                incarnation: slot.incarnation,
            })
            .collect(),
    }
}

/// Shared by activation, staging and capture. Schema 9 and 10 deliberately
/// share scalar behavior while retaining distinct provenance validators.
fn validate_current_values(
    definitions: &Definitions,
    native: &NativeWorldSnapshot,
    lifecycle: &LifecycleSnapshot,
    inventories: Option<&InventoryStateSnapshotV1>,
) -> Result<()> {
    ensure!(
        definitions.mode == SourceMode::InventoryOwners || inventories.is_none(),
        "legacy source mode has inventory state"
    );
    if let Some(contracts) = &definitions.scalar_contracts {
        contracts.validate_native(native)?;
    }
    match definitions.mode {
        SourceMode::ScalarOwners => definitions
            .scalar_imported
            .as_ref()
            .context("scalar provenance missing")?
            .validate_current_state(
                native,
                lifecycle,
                definitions
                    .scalar_snapshot
                    .as_deref()
                    .context("scalar contracts missing")?,
            )?,
        SourceMode::InventoryOwners => {
            ensure!(
                lifecycle
                    .slots
                    .iter()
                    .all(|slot| slot.presence == Presence::Live),
                "inventory owners must all be live"
            );
            let state = inventories.context("inventory state missing")?;
            let contracts = definitions
                .inventory_contracts
                .as_ref()
                .context("inventory contracts missing")?;
            let metadata = definitions
                .inventory_imported
                .as_deref()
                .context("inventory provenance missing")?;
            let bytes = inventory_snapshot_allocation_bytes(
                definitions
                    .inventory_snapshot
                    .as_deref()
                    .context("inventory declarations missing")?,
                state,
            )?
            .checked_add(metadata.allocation_bytes()?)
            .context("inventory allocation overflow")?;
            ensure!(
                bytes <= MAX_INVENTORY_GRAPH_BYTES,
                "inventory allocation limit"
            );
            validate_inventory_state(
                contracts,
                state,
                &metadata.inventory_initial.initial_totals,
                &live_inventory_owners(lifecycle),
            )?;
            metadata.validate_current_state(
                native,
                lifecycle,
                definitions
                    .scalar_snapshot
                    .as_deref()
                    .context("scalar contracts missing")?,
                definitions
                    .inventory_snapshot
                    .as_deref()
                    .context("inventory declarations missing")?,
                state,
            )?;
        }
        _ => ensure!(inventories.is_none(), "unexpected inventory state"),
    }
    Ok(())
}

fn scalar_fingerprint(
    contracts: &CompiledScalarOwnerContracts,
    owner: OwnerHandle,
    values: ScalarValues,
) -> [u8; 32] {
    let mut hash = Sha256::new();
    hash.update(b"icarus/native_scalar_owner_inputs/v1\0");
    hash.update(contracts.identity().as_bytes());
    hash.update(owner.id.0.to_le_bytes());
    hash.update(owner.incarnation.to_le_bytes());
    hash.update([u8::from(values.enabled)]);
    hash.update(values.remaining_uses.to_le_bytes());
    hash.finalize().into()
}

#[cfg(test)]
thread_local! {
    // Only staging consults this private fault: capture can still verify the
    // unchanged world while testing a failure on the final prepared actor.
    static SCALAR_STAGE_FAILURE: std::cell::Cell<Option<DomainId>> = const { std::cell::Cell::new(None) };
}

fn staged_scalar_values(
    contracts: &CompiledScalarOwnerContracts,
    record: &NativeActorRecord,
) -> Result<ScalarValues> {
    #[cfg(test)]
    ensure!(
        !SCALAR_STAGE_FAILURE.with(|failure| failure.get() == Some(record.id)),
        "injected scalar callback staging failure"
    );
    contracts.evaluate(&record.class, &record.variables)
}

/// Every lookup and validation precedes entity mutation, including the reverse
/// index allocation. The commit only moves already prepared component values.
fn stage(snapshot: &WorldSnapshot, definitions: &Arc<Definitions>) -> Result<StagedWorld> {
    ensure!(
        snapshot.schema_version == definitions.mode.schema(),
        "lifecycle source mode changed"
    );
    // Exhaustion is a preparation failure, before any ECS mutation. All runtime
    // operations use exclusive World access; the atomic is a shared pin, not a
    // concurrent writer protocol.
    let generation = definitions
        .current_generation
        .load(Ordering::Relaxed)
        .checked_add(1)
        .context("lifecycle runtime generation exhausted")?;
    let native = snapshot
        .native_world
        .as_ref()
        .context("lifecycle spatial records missing")?;
    let lifecycle = snapshot
        .native_lifecycle
        .as_ref()
        .context("lifecycle state missing")?;
    ensure!(
        snapshot.native_owner_import.as_ref() == definitions.imported.as_deref(),
        "owner source provenance differs from loaded activation"
    );
    if let Some(metadata) = &definitions.imported {
        metadata.validate_current_state(native, lifecycle)?;
    }
    ensure!(
        snapshot.native_scalar_contracts.as_ref() == definitions.scalar_snapshot.as_deref()
            && snapshot.native_scalar_owner_import.as_ref()
                == definitions.scalar_imported.as_deref(),
        "scalar contracts or provenance differ from loaded activation"
    );
    ensure!(
        snapshot.native_inventory_contracts.as_ref() == definitions.inventory_snapshot.as_deref()
            && snapshot.native_inventory_owner_import.as_ref()
                == definitions.inventory_imported.as_deref(),
        "inventory contracts or provenance differ from loaded activation"
    );
    validate_current_values(
        definitions,
        native,
        lifecycle,
        snapshot.native_inventories.as_ref(),
    )?;
    match definitions.mode {
        SourceMode::ModifierOwners => snapshot
            .native_modifiers
            .as_ref()
            .context("modifier state missing")?
            .validate(lifecycle)?,
        _ => ensure!(
            snapshot.native_modifiers.is_none(),
            "unexpected modifier state"
        ),
    }
    let modifiers: BTreeMap<_, _> = snapshot
        .native_modifiers
        .as_ref()
        .map(|state| {
            state
                .owners
                .iter()
                .map(|owner| (owner.owner.id, owner))
                .collect()
        })
        .unwrap_or_default();
    let inventory = snapshot
        .native_inventories
        .as_ref()
        .map(inventory_header)
        .transpose()?;
    let inventory_owners: BTreeMap<_, _> = snapshot
        .native_inventories
        .as_ref()
        .map(|state| {
            state
                .owners
                .iter()
                .map(|owner| (owner.owner.id, owner))
                .collect()
        })
        .unwrap_or_default();
    definitions.lifecycle.validate_snapshot(lifecycle, native)?;
    let by_id: BTreeMap<_, _> = native
        .actors
        .iter()
        .map(|record| (record.id, record))
        .collect();
    let names: BTreeMap<_, _> = lifecycle
        .definition
        .slots
        .iter()
        .map(|slot| (slot.placement.as_str(), slot.owner_name.as_slice()))
        .collect();
    let foundations: BTreeMap<_, _> = snapshot
        .native_relationships
        .as_ref()
        .map(|relationships| {
            relationships
                .foundations
                .iter()
                .map(|row| (row.owner, row.foundation))
                .collect()
        })
        .unwrap_or_default();
    let mut actors = Vec::new();
    let mut dormant = BTreeMap::new();
    for slot in &lifecycle.slots {
        let record = *by_id
            .get(&slot.id)
            .context("lifecycle slot has no spatial record")?;
        if slot.presence == Presence::Absent {
            dormant.insert(slot.id, record.clone());
            continue;
        }
        let relationship = definitions.identities.get(&slot.id).cloned();
        let foundation = if relationship
            .as_ref()
            .is_some_and(|identity| identity.role == NativeRelationshipRole::Deployable)
        {
            Some(NativeFoundation(
                *foundations
                    .get(&slot.id)
                    .context("deployable edge missing")?,
            ))
        } else {
            None
        };
        actors.push(StagedActor {
            record: record.clone(),
            bounds: definitions
                .catalog
                .class(&record.class)
                .context("lifecycle class missing")?
                .local_bounds
                .clone(),
            name: names
                .get(record.placement.as_str())
                .context("lifecycle slot declaration missing")?
                .to_vec(),
            incarnation: slot.incarnation,
            recorder: slot.component.clone(),
            relationship,
            foundation,
            scalar: definitions
                .scalar_contracts
                .as_ref()
                .map(|contracts| {
                    let values = staged_scalar_values(contracts, record)?;
                    let owner = OwnerHandle {
                        id: slot.id,
                        incarnation: slot.incarnation,
                    };
                    Ok::<_, anyhow::Error>(ScalarOwnerDerived {
                        activation: Arc::clone(definitions),
                        generation,
                        owner,
                        input_fingerprint: scalar_fingerprint(contracts, owner, values),
                        available: values.available,
                    })
                })
                .transpose()?,
            modifier: modifiers.get(&slot.id).map(|state| ModifierOwnerState {
                activation: Arc::clone(definitions),
                generation,
                state: (*state).clone(),
            }),
            inventory: inventory_owners
                .get(&slot.id)
                .map(|owner| InventoryOwnerState {
                    activation: Arc::clone(definitions),
                    generation,
                    owner: owner.owner,
                    inventories: owner.inventories.clone(),
                }),
        });
    }
    let dependents = validate_relationships(
        definitions,
        lifecycle,
        snapshot.native_relationships.as_ref(),
    )?;
    Ok(StagedWorld {
        generation,
        actors,
        dormant,
        dependents,
        inventory,
    })
}

fn insert_actor(world: &mut World, entity: Entity, actor: StagedActor) {
    let record = actor.record;
    let mut entity = world.entity_mut(entity);
    entity.insert((
        record.id,
        Position(record.transform.translation),
        NativeActorIdentity {
            placement: record.placement,
            class: record.class,
            tile: record.tile,
        },
        NativeRotation(record.transform.rotation),
        NativeScale(record.transform.scale),
        NativeVariables(record.variables),
        NativeLocalBounds(actor.bounds),
        NativeOwnerName(actor.name),
        NativeOwnerIncarnation(actor.incarnation),
    ));
    if let Some(component) = actor.recorder {
        entity.insert(NativeOwnerRecorder(component));
    } else {
        entity.remove::<NativeOwnerRecorder>();
    }
    if let Some(identity) = actor.relationship {
        entity.insert(identity);
    }
    if let Some(foundation) = actor.foundation {
        entity.insert(foundation);
    }
    if let Some(scalar) = actor.scalar {
        entity.insert(scalar);
    } else {
        entity.remove::<ScalarOwnerDerived>();
    }
    if let Some(modifier) = actor.modifier {
        entity.insert(modifier);
    } else {
        entity.remove::<ModifierOwnerState>();
    }
    if let Some(inventory) = actor.inventory {
        entity.insert(inventory);
    } else {
        entity.remove::<InventoryOwnerState>();
    }
}

pub(crate) fn activate_lifecycle(world: &mut World, snapshot: &WorldSnapshot) -> Result<()> {
    if matches!(
        snapshot.schema_version,
        SNAPSHOT_SCHEMA_C001_SCALAR_OWNERS | SNAPSHOT_SCHEMA_C001_INVENTORY_OWNERS
    ) {
        SnapshotStore::preflight(snapshot)?;
    } else {
        snapshot.validate()?;
    }
    ensure!(
        matches!(
            snapshot.schema_version,
            SNAPSHOT_SCHEMA_LIFECYCLE
                | SNAPSHOT_SCHEMA_C001_OWNERS
                | SNAPSHOT_SCHEMA_C001_SCALAR_OWNERS
                | SNAPSHOT_SCHEMA_C001_INVENTORY_OWNERS
                | SNAPSHOT_SCHEMA_MODIFIER_OWNERS
        ),
        "lifecycle schema required"
    );
    ensure_no_lifecycle_state(world)?;
    require_separate_mode(world)?;
    crate::native_runtime::ensure_no_relationship_state(world)?;
    let native = snapshot
        .native_world
        .as_ref()
        .context("lifecycle spatial records missing")?;
    let lifecycle = snapshot
        .native_lifecycle
        .as_ref()
        .context("lifecycle state missing")?;
    let mode = match snapshot.schema_version {
        SNAPSHOT_SCHEMA_LIFECYCLE => SourceMode::Lifecycle,
        SNAPSHOT_SCHEMA_C001_OWNERS => SourceMode::Owners,
        SNAPSHOT_SCHEMA_C001_SCALAR_OWNERS => SourceMode::ScalarOwners,
        SNAPSHOT_SCHEMA_C001_INVENTORY_OWNERS => SourceMode::InventoryOwners,
        SNAPSHOT_SCHEMA_MODIFIER_OWNERS => SourceMode::ModifierOwners,
        _ => unreachable!("validated lifecycle schema"),
    };
    // Admit the joint borrowed graph before any declaration/provenance clone.
    let inventory_pinned_bytes = if mode == SourceMode::InventoryOwners {
        let declarations = snapshot
            .native_inventory_contracts
            .as_ref()
            .context("inventory contracts missing")?;
        let state = snapshot
            .native_inventories
            .as_ref()
            .context("inventory state missing")?;
        let metadata = snapshot
            .native_inventory_owner_import
            .as_ref()
            .context("inventory provenance missing")?;
        let metadata_bytes = metadata.allocation_bytes()?;
        let bytes = inventory_snapshot_allocation_bytes(declarations, state)?
            .checked_add(metadata_bytes)
            .context("inventory allocation overflow")?;
        ensure!(
            bytes <= MAX_INVENTORY_GRAPH_BYTES,
            "inventory allocation limit"
        );
        // Empty state is used only for allocation accounting, never validation
        // or publication. Count current component/header allocations on capture.
        let empty = InventoryStateSnapshotV1 {
            format: 1,
            contract_id: declarations.contract_id.clone(),
            revision: 1,
            next_stack_id: 1,
            consumed: Vec::new(),
            owners: Vec::new(),
        };
        inventory_snapshot_allocation_bytes(declarations, &empty)?
            .checked_add(metadata_bytes)
            .context("inventory allocation overflow")?
    } else {
        0
    };
    let catalog = Arc::new(CompiledNativeCatalog::compile(native.definitions.clone())?);
    let compiled = CompiledNativeLifecycle::from_snapshot(lifecycle, native)?;
    let relationships = snapshot
        .native_relationships
        .as_ref()
        .map(|relationships| CompiledNativeRelationships::from_snapshot(relationships, native))
        .transpose()?;
    let by_placement: BTreeMap<_, _> = native
        .actors
        .iter()
        .map(|actor| (actor.placement.as_str(), actor.id))
        .collect();
    let mut identities = BTreeMap::new();
    if let Some(relationships) = &relationships {
        for actor in &relationships.definition().actors {
            identities.insert(
                *by_placement
                    .get(actor.placement.as_str())
                    .context("relationship slot missing")?,
                NativeRelationshipIdentity {
                    identity: actor.identity.clone(),
                    role: actor.role,
                },
            );
        }
    }
    let definitions = Arc::new(Definitions {
        current_generation: AtomicU64::new(0),
        modifier_failed: AtomicBool::new(false),
        mode,
        inventory_pinned_bytes,
        inventory_contracts: snapshot
            .native_inventory_contracts
            .as_ref()
            .map(|contracts| CompiledInventoryContractsV1::from_snapshot(contracts, &catalog))
            .transpose()?,
        inventory_snapshot: snapshot
            .native_inventory_contracts
            .as_ref()
            .map(|contracts| Arc::new(contracts.clone())),
        inventory_imported: snapshot
            .native_inventory_owner_import
            .as_ref()
            .map(|metadata| Arc::new(metadata.clone())),
        catalog,
        lifecycle: compiled,
        relationships,
        identities,
        imported: snapshot
            .native_owner_import
            .as_ref()
            .map(|metadata| Arc::new(metadata.clone())),
        scalar_contracts: snapshot
            .native_scalar_contracts
            .as_ref()
            .map(|contracts| CompiledScalarOwnerContracts::from_snapshot(contracts, native))
            .transpose()?,
        scalar_snapshot: snapshot
            .native_scalar_contracts
            .as_ref()
            .map(|contracts| Arc::new(contracts.clone())),
        scalar_imported: snapshot
            .native_scalar_owner_import
            .as_ref()
            .map(|metadata| Arc::new(metadata.clone())),
    });
    let staged = stage(snapshot, &definitions)?;
    if mode == SourceMode::ModifierOwners {
        world.insert_resource(ModifierRuntimeAnchor {
            definitions: Arc::clone(&definitions),
        });
    }
    if let Some(metadata) = &definitions.imported {
        world.insert_resource(C001OwnerImportState(Arc::clone(metadata)));
    }
    if let Some(contracts) = &definitions.scalar_snapshot {
        world.insert_resource(NativeScalarOwnerContractsState(Arc::clone(contracts)));
    }
    if let Some(metadata) = &definitions.scalar_imported {
        world.insert_resource(C001ScalarOwnerImportState(Arc::clone(metadata)));
    }
    if let Some(contracts) = &definitions.inventory_snapshot {
        world.insert_resource(NativeInventoryContractsState(Arc::clone(contracts)));
    }
    if let Some(metadata) = &definitions.inventory_imported {
        world.insert_resource(C001InventoryOwnerImportState(Arc::clone(metadata)));
    }
    let mut live = BTreeMap::new();
    for actor in staged.actors {
        let id = actor.record.id;
        let entity = world.spawn_empty().id();
        insert_actor(world, entity, actor);
        live.insert(id, entity);
    }
    world.insert_resource(DormantNativeSlots {
        definitions: Arc::clone(&definitions),
        generation: staged.generation,
        records: staged.dormant,
    });
    if let Some(dependents) = staged.dependents {
        world.insert_resource(dependents);
    }
    definitions
        .current_generation
        .store(staged.generation, Ordering::Relaxed);
    world.insert_resource(NativeLifecycleContext {
        definitions,
        generation: staged.generation,
        lifecycle: lifecycle.clone(),
        live,
        inventory: staged.inventory,
    });
    Ok(())
}

fn validate_relationships(
    definitions: &Definitions,
    lifecycle: &LifecycleSnapshot,
    relationships: Option<&NativeRelationshipsSnapshot>,
) -> Result<Option<NativeFoundationDependents>> {
    let Some(compiled) = &definitions.relationships else {
        ensure!(
            relationships.is_none(),
            "unexpected lifecycle relationship state"
        );
        return Ok(None);
    };
    let relationships = relationships.context("required lifecycle relationships missing")?;
    ensure!(
        relationships.relationship_id == compiled.identity()
            && &relationships.definition == compiled.definition(),
        "lifecycle relationship declarations changed"
    );
    compiled.validate_foundations(&relationships.foundations)?;
    let live: BTreeSet<_> = lifecycle
        .slots
        .iter()
        .filter(|slot| slot.presence == Presence::Live)
        .map(|slot| slot.id)
        .collect();
    let mut reverse: BTreeMap<_, Vec<_>> = definitions
        .identities
        .iter()
        .filter(|(id, identity)| {
            live.contains(id) && identity.role == NativeRelationshipRole::Foundation
        })
        .map(|(id, _)| (*id, Vec::new()))
        .collect();
    for row in &relationships.foundations {
        if let Some(target) = row.foundation {
            ensure!(
                live.contains(&row.owner),
                "dormant deployable has a foundation edge"
            );
            reverse
                .get_mut(&target)
                .context("foundation target is not live")?
                .push(row.owner);
        }
    }
    for dependents in reverse.values_mut() {
        dependents.sort_unstable();
    }
    Ok(Some(NativeFoundationDependents(reverse)))
}

fn capture_inventories(
    world: &World,
    context: &NativeLifecycleContext,
) -> Result<Option<InventoryStateSnapshotV1>> {
    let definitions = &context.definitions;
    if definitions.mode != SourceMode::InventoryOwners {
        ensure!(
            context.inventory.is_none(),
            "legacy world has inventory header"
        );
        return Ok(None);
    }
    let header = context
        .inventory
        .as_ref()
        .context("inventory header missing")?;
    let contracts = definitions
        .inventory_snapshot
        .as_deref()
        .context("inventory contracts missing")?;
    ensure!(
        context.live.len() <= MAX_INVENTORY_OWNERS
            && header.consumed.len() <= MAX_INVENTORY_ITEMS
            && header.consumed.capacity() <= 2 * MAX_INVENTORY_ITEMS + 4,
        "inventory header allocation limit"
    );
    let mut bytes = definitions.inventory_pinned_bytes;
    let mut charge = |n: usize| -> Result<()> {
        bytes = bytes
            .checked_add(n)
            .filter(|bytes| *bytes <= MAX_INVENTORY_GRAPH_BYTES)
            .context("inventory allocation limit")?;
        Ok(())
    };
    // Include caller capacities and a conservative decoded-vector allowance.
    charge(
        header
            .consumed
            .capacity()
            .max(header.consumed.len() * 2)
            .checked_mul(size_of::<ItemTotalV1>())
            .context("inventory allocation overflow")?,
    )?;
    charge(
        context
            .live
            .len()
            .checked_mul(2 * size_of::<OwnerInventoriesV1>())
            .context("inventory allocation overflow")?,
    )?;
    for total in &header.consumed {
        ensure!(
            !total.item.is_empty() && total.item.len() <= 64 && total.item.capacity() <= 132,
            "inventory consumed key limit"
        );
        charge(total.item.capacity())?;
    }
    let mut owners = Vec::with_capacity(context.live.len());
    let slots: BTreeMap<_, _> = context
        .lifecycle
        .slots
        .iter()
        .map(|slot| (slot.id, slot))
        .collect();
    let mut instances = 0usize;
    let mut stacks = 0usize;
    for (id, entity) in &context.live {
        let state = world
            .get::<InventoryOwnerState>(*entity)
            .context("owner lacks inventory state")?;
        let slot = slots.get(id).context("inventory lifecycle owner missing")?;
        ensure!(
            Arc::ptr_eq(&state.activation, definitions)
                && state.generation == context.generation
                && state.owner
                    == OwnerHandle {
                        id: *id,
                        incarnation: slot.incarnation
                    }
                && slot.presence == Presence::Live,
            "inventory state differs from its activation or owner"
        );
        charge(inventory_collection_bytes(&state.inventories)?)?;
        instances = instances
            .checked_add(state.inventories.len())
            .filter(|count| *count <= MAX_INVENTORY_INSTANCES)
            .context("inventory instance limit")?;
        for inventory in &state.inventories {
            stacks = stacks
                .checked_add(inventory.slots.iter().flatten().count())
                .and_then(|count| count.checked_add(inventory.overflow.len()))
                .filter(|count| *count <= MAX_INVENTORY_STACKS)
                .context("inventory stack limit")?;
        }
        owners.push(BorrowedInventoryOwner {
            owner: state.owner,
            inventories: &state.inventories,
        });
    }
    let borrowed = BorrowedInventories {
        format: 1,
        contract_id: &contracts.contract_id,
        revision: header.revision,
        next_stack_id: header.next_stack_id,
        consumed: &header.consumed,
        owners,
    };
    ensure!(
        inventory_fingerprint(&borrowed)? == header.fingerprint,
        "inventory state differs from its committed checksum"
    );
    // All capacities, aggregate sizes and bytes were checked before cloning.
    Ok(Some(InventoryStateSnapshotV1 {
        format: 1,
        contract_id: contracts.contract_id.clone(),
        revision: header.revision,
        next_stack_id: header.next_stack_id,
        consumed: header.consumed.clone(),
        owners: borrowed
            .owners
            .into_iter()
            .map(|owner| OwnerInventoriesV1 {
                owner: owner.owner,
                inventories: owner.inventories.clone(),
            })
            .collect(),
    }))
}

pub(crate) fn capture_lifecycle(world: &World) -> Result<WorldSnapshot> {
    modifier_runtime_health(world)?;
    let context = world
        .get_resource::<NativeLifecycleContext>()
        .context("world lacks lifecycle context")?;
    crate::simulation::validate_lifecycle_activation(world, context)?;
    require_separate_mode(world)?;
    let dormant = world
        .get_resource::<DormantNativeSlots>()
        .context("world lacks dormant slot state")?;
    ensure!(
        Arc::ptr_eq(&context.definitions, &dormant.definitions)
            && dormant.generation == context.generation,
        "dormant state differs from loaded activation"
    );
    let definitions = &context.definitions;
    ensure!(
        context.generation == definitions.current_generation.load(Ordering::Relaxed),
        "lifecycle context generation is stale"
    );
    ensure!(
        match (
            world.get_resource::<NativeInventoryContractsState>(),
            &definitions.inventory_snapshot
        ) {
            (None, None) => true,
            (Some(actual), Some(expected)) =>
                Arc::ptr_eq(&actual.0, expected) || actual.0 == *expected,
            _ => false,
        },
        "inventory contracts differ from loaded activation"
    );
    ensure!(
        match (
            world.get_resource::<C001InventoryOwnerImportState>(),
            &definitions.inventory_imported
        ) {
            (None, None) => true,
            (Some(actual), Some(expected)) =>
                Arc::ptr_eq(&actual.0, expected) || actual.0 == *expected,
            _ => false,
        },
        "inventory provenance differs from loaded activation"
    );
    ensure!(
        match (
            world.get_resource::<NativeScalarOwnerContractsState>(),
            &definitions.scalar_snapshot
        ) {
            (None, None) => true,
            (Some(actual), Some(expected)) =>
                Arc::ptr_eq(&actual.0, expected) || actual.0 == *expected,
            _ => false,
        },
        "scalar contracts differ from loaded activation"
    );
    ensure!(
        match (
            world.get_resource::<C001ScalarOwnerImportState>(),
            &definitions.scalar_imported
        ) {
            (None, None) => true,
            (Some(actual), Some(expected)) =>
                Arc::ptr_eq(&actual.0, expected) || actual.0 == *expected,
            _ => false,
        },
        "scalar source provenance differs from loaded activation"
    );
    ensure!(
        match (
            world.get_resource::<C001OwnerImportState>(),
            &definitions.imported
        ) {
            (None, None) => true,
            (Some(actual), Some(expected)) =>
                Arc::ptr_eq(&actual.0, expected) || actual.0 == *expected,
            _ => false,
        },
        "owner source provenance differs from loaded activation"
    );
    let slots: BTreeMap<_, _> = context
        .lifecycle
        .slots
        .iter()
        .map(|slot| (slot.id, slot))
        .collect();
    let expected_live: BTreeSet<_> = slots
        .iter()
        .filter(|(_, slot)| slot.presence == Presence::Live)
        .map(|(id, _)| *id)
        .collect();
    let expected_dormant: BTreeSet<_> = slots
        .iter()
        .filter(|(_, slot)| slot.presence == Presence::Absent)
        .map(|(id, _)| *id)
        .collect();
    ensure!(
        context.live.len() == expected_live.len()
            && context.live.keys().copied().collect::<BTreeSet<_>>() == expected_live,
        "lifecycle live registry incomplete"
    );
    ensure!(
        dormant.records.len() == expected_dormant.len()
            && dormant.records.keys().copied().collect::<BTreeSet<_>>() == expected_dormant,
        "lifecycle dormant partition incomplete"
    );
    require_callback_free_commit(world, context)?;
    let expected_entities: BTreeSet<_> = context.live.values().copied().collect();
    for entity in world.iter_entities() {
        if expected_entities.contains(&entity.id()) {
            // Lifecycle modes 7–11 have no ECS lifetime relationships. Inspect
            // both ends, including empty targets and disabled owners, before
            // staging: despawning one owner must not cascade into another.
            // Foundation domain links are ordinary components, unaffected here.
            // Empty targets can have an uninitialized relationship accessor;
            // their hooks are covered by require_callback_free_commit above.
            for component in entity.archetype().components() {
                let info = world
                    .components()
                    .get_info(*component)
                    .context("managed owner component metadata missing")?;
                ensure!(
                    !info
                        .relationship_accessor()
                        .is_some_and(|accessor| accessor.linked_spawn()),
                    "managed owner has unsupported ECS lifetime relationship"
                );
            }
        }
        if let Some(anchor) = entity.get::<ModifierRuntimeAnchor>() {
            ensure!(
                world
                    .get_resource::<ModifierRuntimeAnchor>()
                    .is_some_and(|expected| std::ptr::eq(anchor, expected)),
                "unregistered modifier runtime anchor"
            );
        }
        if let Some(actual) = entity.get::<NativeLifecycleContext>() {
            ensure!(
                std::ptr::eq(actual, context),
                "unregistered lifecycle context"
            );
        }
        if let Some(actual) = entity.get::<DormantNativeSlots>() {
            ensure!(std::ptr::eq(actual, dormant), "unregistered dormant slots");
        }
        if let Some(actual) = entity.get::<NativeInventoryContractsState>() {
            ensure!(
                world
                    .get_resource::<NativeInventoryContractsState>()
                    .is_some_and(|resource| std::ptr::eq(actual, resource)),
                "unregistered inventory contracts"
            );
        }
        if let Some(actual) = entity.get::<C001InventoryOwnerImportState>() {
            ensure!(
                world
                    .get_resource::<C001InventoryOwnerImportState>()
                    .is_some_and(|resource| std::ptr::eq(actual, resource)),
                "unregistered inventory provenance"
            );
        }
        if entity.contains::<ModifierOwnerState>() {
            ensure!(
                definitions.mode == SourceMode::ModifierOwners
                    && expected_entities.contains(&entity.id()),
                "unregistered modifier owner state"
            );
        }
        if entity.contains::<InventoryOwnerState>() {
            ensure!(
                definitions.mode == SourceMode::InventoryOwners
                    && expected_entities.contains(&entity.id()),
                "unregistered inventory owner state"
            );
        }
        if let Some(contracts) = entity.get::<NativeScalarOwnerContractsState>() {
            ensure!(
                world
                    .get_resource::<NativeScalarOwnerContractsState>()
                    .is_some_and(|resource| std::ptr::eq(contracts, resource)),
                "lifecycle world contains unregistered scalar contracts"
            );
        }
        if let Some(metadata) = entity.get::<C001ScalarOwnerImportState>() {
            ensure!(
                world
                    .get_resource::<C001ScalarOwnerImportState>()
                    .is_some_and(|resource| std::ptr::eq(metadata, resource)),
                "lifecycle world contains unregistered scalar provenance"
            );
        }
        if let Some(metadata) = entity.get::<C001OwnerImportState>() {
            ensure!(
                world
                    .get_resource::<C001OwnerImportState>()
                    .is_some_and(|resource| std::ptr::eq(metadata, resource)),
                "lifecycle world contains unregistered owner provenance"
            );
        }
        if let Some(id) = entity.get::<DomainId>() {
            ensure!(
                context.live.get(id) == Some(&entity.id()),
                "lifecycle world contains an unexpected domain entity"
            );
        }
        if entity.contains::<NativeActorIdentity>()
            || entity.contains::<NativeRotation>()
            || entity.contains::<NativeScale>()
            || entity.contains::<NativeVariables>()
            || entity.contains::<NativeLocalBounds>()
            || entity.contains::<NativeOwnerIncarnation>()
            || entity.contains::<NativeOwnerName>()
            || entity.contains::<NativeOwnerRecorder>()
            || entity.contains::<NativeRelationshipIdentity>()
            || entity.contains::<NativeFoundation>()
            || entity.contains::<ScalarOwnerDerived>()
            || entity.contains::<InventoryOwnerState>()
            || entity.contains::<ModifierOwnerState>()
        {
            ensure!(
                expected_entities.contains(&entity.id()),
                "lifecycle world contains unregistered actor components"
            );
        }
    }
    let declarations: BTreeMap<_, _> = context
        .lifecycle
        .definition
        .slots
        .iter()
        .map(|slot| (slot.placement.as_str(), slot))
        .collect();
    let mut records: BTreeMap<_, _> = dormant
        .records
        .iter()
        .map(|(id, record)| (*id, NativeActorView::from(record)))
        .collect();
    let mut foundations = Vec::new();
    for (id, entity) in &context.live {
        let slot = slots.get(id).context("live lifecycle slot missing")?;
        ensure!(
            world.get::<DomainId>(*entity) == Some(id),
            "live owner lacks expected domain identity"
        );
        ensure!(
            world.get::<NativeOwnerIncarnation>(*entity)
                == Some(&NativeOwnerIncarnation(slot.incarnation)),
            "owner incarnation differs from its registry"
        );
        let identity = world
            .get::<NativeActorIdentity>(*entity)
            .context("live owner lacks spatial identity")?;
        let declaration = declarations
            .get(identity.placement.as_str())
            .context("live owner placement undeclared")?;
        ensure!(
            world
                .get::<NativeOwnerName>(*entity)
                .is_some_and(|name| name.0 == declaration.owner_name),
            "owner name differs from its declaration"
        );
        ensure!(
            world
                .get::<NativeOwnerRecorder>(*entity)
                .map(|record| &record.0)
                == slot.component.as_ref(),
            "owner recorder differs from its binding"
        );
        let bounds = world
            .get::<NativeLocalBounds>(*entity)
            .context("owner lacks local bounds")?;
        ensure!(
            bounds.0
                == definitions
                    .catalog
                    .class(&identity.class)
                    .context("owner class undeclared")?
                    .local_bounds,
            "owner local bounds differ from catalog"
        );
        let variables = &world
            .get::<NativeVariables>(*entity)
            .context("owner lacks variables")?
            .0;
        if definitions.scalar_contracts.is_some() {
            ensure!(variables.capacity() <= 4, "scalar variable capacity limit");
        }
        let record = NativeActorView {
            id: *id,
            placement: &identity.placement,
            class: &identity.class,
            tile: &identity.tile,
            transform: NativeTransform {
                translation: world
                    .get::<Position>(*entity)
                    .context("owner lacks position")?
                    .0,
                rotation: world
                    .get::<NativeRotation>(*entity)
                    .context("owner lacks rotation")?
                    .0,
                scale: world
                    .get::<NativeScale>(*entity)
                    .context("owner lacks scale")?
                    .0,
            },
            variables,
        };
        match &definitions.scalar_contracts {
            Some(contracts) => {
                let values = contracts.evaluate(record.class, record.variables)?;
                let owner = OwnerHandle {
                    id: *id,
                    incarnation: slot.incarnation,
                };
                let derived = world
                    .get::<ScalarOwnerDerived>(*entity)
                    .context("scalar owner lacks derived state")?;
                ensure!(
                    Arc::ptr_eq(&derived.activation, definitions)
                        && derived.generation == context.generation
                        && derived.owner == owner
                        && derived.input_fingerprint
                            == scalar_fingerprint(contracts, owner, values)
                        && derived.available == values.available,
                    "scalar derived state differs from its activation or inputs"
                );
            }
            None => ensure!(
                world.get::<ScalarOwnerDerived>(*entity).is_none(),
                "owner has unexpected scalar derived state"
            ),
        }
        records.insert(*id, record);
        match definitions.identities.get(id) {
            Some(expected) => {
                ensure!(
                    world.get::<NativeRelationshipIdentity>(*entity) == Some(expected),
                    "owner relationship identity differs from declaration"
                );
                if expected.role == NativeRelationshipRole::Deployable {
                    foundations.push(NativeFoundationRecord {
                        owner: *id,
                        foundation: world
                            .get::<NativeFoundation>(*entity)
                            .context("owner lacks foundation state")?
                            .0,
                    });
                } else {
                    ensure!(
                        world.get::<NativeFoundation>(*entity).is_none(),
                        "foundation component has an invalid owner role"
                    );
                }
            }
            None => ensure!(
                world.get::<NativeRelationshipIdentity>(*entity).is_none()
                    && world.get::<NativeFoundation>(*entity).is_none(),
                "owner has undeclared relationship state"
            ),
        }
    }
    for id in expected_dormant {
        if definitions
            .identities
            .get(&id)
            .is_some_and(|identity| identity.role == NativeRelationshipRole::Deployable)
        {
            foundations.push(NativeFoundationRecord {
                owner: id,
                foundation: None,
            });
        }
    }
    foundations.sort_by_key(|row| row.owner);
    let relationships =
        definitions
            .relationships
            .as_ref()
            .map(|compiled| NativeRelationshipsSnapshot {
                relationship_id: compiled.identity().to_owned(),
                definition: compiled.definition().clone(),
                foundations,
            });
    let reverse = validate_relationships(definitions, &context.lifecycle, relationships.as_ref())?;
    ensure!(
        world.get_resource::<NativeFoundationDependents>() == reverse.as_ref(),
        "lifecycle foundation reverse index is inconsistent"
    );
    // Bound and validate every borrowed actor payload before cloning any values
    // into snapshot output. Public ECS edits cannot make capture allocate an
    // oversized or invalid variable tree first and reject it afterward.
    definitions
        .catalog
        .validate_records(&context.lifecycle.definition.map, records.values().cloned())?;
    let modifiers = capture_modifiers(world, context)?;
    let inventories = capture_inventories(world, context)?;
    let native = NativeWorldSnapshot {
        catalog_id: definitions.catalog.identity().to_owned(),
        definitions: definitions.catalog.definition().clone(),
        map: context.lifecycle.definition.map.clone(),
        actors: records
            .into_values()
            .map(|actor| NativeActorRecord {
                id: actor.id,
                placement: actor.placement.to_owned(),
                class: actor.class.to_owned(),
                tile: actor.tile.to_owned(),
                transform: actor.transform,
                variables: actor.variables.to_vec(),
            })
            .collect(),
    };
    definitions
        .lifecycle
        .validate_snapshot(&context.lifecycle, &native)?;
    if let Some(metadata) = &definitions.imported {
        metadata.validate_current_state(&native, &context.lifecycle)?;
    }
    validate_current_values(
        definitions,
        &native,
        &context.lifecycle,
        inventories.as_ref(),
    )?;
    Ok(WorldSnapshot {
        schema_version: definitions.mode.schema(),
        tick: world
            .get_resource::<SimulationClock>()
            .context("world lacks simulation clock")?
            .tick,
        entities: Vec::new(),
        native_world: Some(native),
        native_import: None,
        native_relationships: relationships,
        native_lifecycle: Some(context.lifecycle.clone()),
        native_owner_import: definitions.imported.as_deref().cloned(),
        native_scalar_contracts: definitions.scalar_snapshot.as_deref().cloned(),
        native_scalar_owner_import: definitions.scalar_imported.as_deref().cloned(),
        native_inventory_contracts: definitions.inventory_snapshot.as_deref().cloned(),
        native_inventory_owner_import: definitions.inventory_imported.as_deref().cloned(),
        native_inventories: inventories,
        native_modifiers: modifiers,
        preserved_prospect: world
            .get_resource::<PreservedProspectState>()
            .map(|state| Arc::clone(&state.0)),
    })
}

fn require_handle(snapshot: &WorldSnapshot, handle: OwnerHandle) -> Result<()> {
    let lifecycle = snapshot
        .native_lifecycle
        .as_ref()
        .context("lifecycle state missing")?;
    ensure!(
        lifecycle.slots.iter().any(|slot| slot.id == handle.id
            && slot.presence == Presence::Live
            && slot.incarnation == handle.incarnation),
        "owner handle is stale or absent"
    );
    Ok(())
}

/// Read a validated live handle; its incarnation is required by every mutation.
pub fn native_owner_handle(world: &World, id: DomainId) -> Result<OwnerHandle> {
    let snapshot = capture_lifecycle(world)?;
    let slot = snapshot
        .native_lifecycle
        .as_ref()
        .context("lifecycle state missing")?
        .slots
        .iter()
        .find(|slot| slot.id == id && slot.presence == Presence::Live)
        .context("owner is absent or undeclared")?;
    Ok(OwnerHandle {
        id,
        incarnation: slot.incarnation,
    })
}

#[derive(Clone, PartialEq)]
pub struct InventoryOwnerQueryV1 {
    pub revision: u64,
    pub owner: OwnerInventoriesV1,
}

impl std::fmt::Debug for InventoryOwnerQueryV1 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InventoryOwnerQueryV1")
            .finish_non_exhaustive()
    }
}

/// Every query validates all registered owners and the global header first.
pub fn inventory_owner_state(world: &World, owner: OwnerHandle) -> Result<InventoryOwnerQueryV1> {
    let snapshot = capture_lifecycle(world)?;
    require_handle(&snapshot, owner)?;
    let state = snapshot
        .native_inventories
        .context("world has no inventory owner contract")?;
    let row = state
        .owners
        .into_iter()
        .find(|row| row.owner == owner)
        .context("inventory owner missing")?;
    Ok(InventoryOwnerQueryV1 {
        revision: state.revision,
        owner: row,
    })
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum InventoryStackLocationV1 {
    Slot { index: u16 },
    Overflow { index: usize, requested_slot: u16 },
}

impl std::fmt::Debug for InventoryStackLocationV1 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = match self {
            Self::Slot { .. } => "Slot",
            Self::Overflow { .. } => "Overflow",
        };
        f.debug_struct(name).finish_non_exhaustive()
    }
}

/// An owned stack view at the world's current global inventory revision.
#[derive(Clone, PartialEq)]
pub struct InventoryStackQueryV1 {
    pub revision: u64,
    pub owner: OwnerHandle,
    pub inventory: String,
    pub stack: StackV1,
    pub location: InventoryStackLocationV1,
}

impl std::fmt::Debug for InventoryStackQueryV1 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InventoryStackQueryV1")
            .finish_non_exhaustive()
    }
}

/// Validate the entire world and handle before resolving an exact endpoint.
/// Retired stack identities never resolve, and the revision is not a caller CAS.
pub fn inventory_stack(
    world: &World,
    owner: OwnerHandle,
    inventory_key: &str,
    stack_id: StackId,
) -> Result<InventoryStackQueryV1> {
    let snapshot = capture_lifecycle(world)?;
    require_handle(&snapshot, owner)?;
    ensure!(
        !inventory_key.is_empty()
            && inventory_key.len() <= 64
            && inventory_key
                .bytes()
                .all(|value| value.is_ascii_alphanumeric() || b"-_.".contains(&value)),
        "inventory key invalid"
    );
    let state = snapshot
        .native_inventories
        .context("world has no inventory owner contract")?;
    let row = state
        .owners
        .into_iter()
        .find(|row| row.owner == owner)
        .context("inventory owner missing")?;
    let inventory = row
        .inventories
        .into_iter()
        .find(|inventory| inventory.key == inventory_key)
        .context("inventory endpoint instance invalid")?;
    for (index, stack) in inventory.slots.into_iter().enumerate() {
        if let Some(stack) = stack.filter(|stack| stack.id == stack_id) {
            return Ok(InventoryStackQueryV1 {
                revision: state.revision,
                owner,
                inventory: inventory.key,
                stack,
                location: InventoryStackLocationV1::Slot {
                    index: u16::try_from(index).context("inventory slot index invalid")?,
                },
            });
        }
    }
    for (index, deferred) in inventory.overflow.into_iter().enumerate() {
        if deferred.stack.id == stack_id {
            return Ok(InventoryStackQueryV1 {
                revision: state.revision,
                owner,
                inventory: inventory.key,
                stack: deferred.stack,
                location: InventoryStackLocationV1::Overflow {
                    index,
                    requested_slot: deferred.requested_slot,
                },
            });
        }
    }
    anyhow::bail!("inventory endpoint stack missing")
}

/// Prepare and preflight the entire resulting world before changing any ECS
/// value. No-change requests still validate unrelated owners and provenance.
pub fn apply_native_inventory_action(
    world: &mut World,
    request: &InventoryActionRequestV1,
) -> Result<InventoryActionOutcomeV1> {
    let mut snapshot = capture_lifecycle(world)?;
    let definitions = &world.resource::<NativeLifecycleContext>().definitions;
    let prepared = prepare_inventory_action(
        definitions
            .inventory_contracts
            .as_ref()
            .context("world has no inventory owner contract")?,
        snapshot
            .native_inventories
            .as_ref()
            .context("inventory state missing")?,
        &definitions
            .inventory_imported
            .as_ref()
            .context("inventory provenance missing")?
            .inventory_initial
            .initial_totals,
        &live_inventory_owners(
            snapshot
                .native_lifecycle
                .as_ref()
                .context("lifecycle state missing")?,
        ),
        request,
    )?;
    let outcome = prepared.outcome();
    if outcome == InventoryActionOutcomeV1::NoChange {
        return Ok(outcome);
    }
    snapshot.native_inventories = Some(prepared.into_state());
    SnapshotStore::preflight(&snapshot)?;
    let staged = stage(&snapshot, definitions)?;
    commit(world, snapshot, staged);
    Ok(outcome)
}

pub fn restore_native_inventory_owners(
    world: &mut World,
    batch: &InventoryRestoreBatchV1,
) -> Result<NativeRestoreResult> {
    let current = capture_lifecycle(world)?;
    ensure!(
        current.schema_version == SNAPSHOT_SCHEMA_C001_INVENTORY_OWNERS,
        "inventory restore requires schema 10"
    );
    let prepared = prepare_inventory_owner_restore(&current, batch)?;
    let completion = prepared
        .snapshot()
        .native_lifecycle
        .as_ref()
        .and_then(|lifecycle| lifecycle.completed.as_ref())
        .context("prepared inventory restore completion missing")?
        .clone();
    if prepared.already_completed() {
        return Ok(NativeRestoreResult::AlreadyCompleted(completion));
    }
    SnapshotStore::preflight(prepared.snapshot())?;
    let staged = stage(
        prepared.snapshot(),
        &world.resource::<NativeLifecycleContext>().definitions,
    )?;
    commit(world, prepared.into_snapshot(), staged);
    Ok(NativeRestoreResult::AppliedInMemory(completion))
}

fn scalar_state_from_snapshot(
    world: &World,
    snapshot: &WorldSnapshot,
    owner: OwnerHandle,
) -> Result<ScalarOwnerState> {
    require_handle(snapshot, owner)?;
    let contracts = world
        .resource::<NativeLifecycleContext>()
        .definitions
        .scalar_contracts
        .as_ref()
        .context("world has no scalar owner contract")?;
    let actor = snapshot
        .native_world
        .as_ref()
        .context("spatial slots missing")?
        .actors
        .iter()
        .find(|actor| actor.id == owner.id)
        .context("owner spatial record missing")?;
    let values = contracts.evaluate(&actor.class, &actor.variables)?;
    Ok(ScalarOwnerState {
        owner,
        enabled: values.enabled,
        remaining_uses: values.remaining_uses,
        available: values.available,
    })
}

/// Validate the entire activation before reading an owner's native state.
pub fn scalar_owner_state(world: &World, owner: OwnerHandle) -> Result<ScalarOwnerState> {
    let snapshot = capture_lifecycle(world)?;
    scalar_state_from_snapshot(world, &snapshot, owner)
}

fn commit_scalar_values(
    world: &mut World,
    mut snapshot: WorldSnapshot,
    owner: OwnerHandle,
    enabled: bool,
    remaining_uses: i32,
) -> Result<ScalarOwnerState> {
    let actor = snapshot
        .native_world
        .as_mut()
        .context("spatial slots missing")?
        .actors
        .iter_mut()
        .find(|actor| actor.id == owner.id)
        .context("owner spatial record missing")?;
    // Capture established the exact pair before this helper was entered. Keep
    // the canonical order and capacities; evaluate still checks the new range.
    for variable in &mut actor.variables {
        variable.value = match variable.key.as_str() {
            "enabled" => NativeValue::Bool(enabled),
            "remaining_uses" => NativeValue::I32(remaining_uses),
            _ => anyhow::bail!("unexpected scalar variable"),
        };
    }
    let result = scalar_state_from_snapshot(world, &snapshot, owner)?;
    // This deliberately retains whole-store validation and encoding preflight,
    // including provenance replay. Runtime capture itself never replays source.
    SnapshotStore::preflight(&snapshot)?;
    let staged = stage(
        &snapshot,
        &world.resource::<NativeLifecycleContext>().definitions,
    )?;
    commit(world, snapshot, staged);
    Ok(result)
}

/// A synchronous native attempt; callers must not retry an ambiguous external
/// acknowledgment. Disabled and exhausted attempts are validated no-ops.
pub fn try_use(world: &mut World, owner: OwnerHandle) -> Result<ScalarUseOutcome> {
    let snapshot = capture_lifecycle(world)?;
    let current = scalar_state_from_snapshot(world, &snapshot, owner)?;
    if !current.enabled {
        return Ok(ScalarUseOutcome::Disabled(current));
    }
    if current.remaining_uses == 0 {
        return Ok(ScalarUseOutcome::Exhausted(current));
    }
    let next = commit_scalar_values(world, snapshot, owner, true, current.remaining_uses - 1)?;
    Ok(ScalarUseOutcome::Used(next))
}

/// Set the authoritative Bool while preserving the count. Equal values still
/// validate the entire world, then return without advancing its generation.
pub fn set_enabled(
    world: &mut World,
    owner: OwnerHandle,
    enabled: bool,
) -> Result<ScalarOwnerState> {
    let snapshot = capture_lifecycle(world)?;
    let current = scalar_state_from_snapshot(world, &snapshot, owner)?;
    if current.enabled == enabled {
        return Ok(current);
    }
    commit_scalar_values(world, snapshot, owner, enabled, current.remaining_uses)
}

/// Exclusive commit of a fully prepared state. Existing entities survive find
/// and edits; a changed incarnation always retires the old entity.
fn commit(world: &mut World, snapshot: WorldSnapshot, staged: StagedWorld) {
    let context = world.resource::<NativeLifecycleContext>();
    let definitions = Arc::clone(&context.definitions);
    let previous = context.live.clone();
    let incarnations: BTreeMap<_, _> = context
        .lifecycle
        .slots
        .iter()
        .map(|slot| (slot.id, slot.incarnation))
        .collect();
    let mut live = BTreeMap::new();
    for actor in staged.actors {
        let id = actor.record.id;
        let entity = match previous.get(&id) {
            Some(entity) if incarnations.get(&id) == Some(&actor.incarnation) => *entity,
            old => {
                if let Some(entity) = old {
                    world.despawn(*entity);
                }
                world.spawn_empty().id()
            }
        };
        insert_actor(world, entity, actor);
        live.insert(id, entity);
    }
    // V1 never removes a live slot. The prepared model enforces that invariant.
    if let Some(dependents) = staged.dependents {
        *world.resource_mut::<NativeFoundationDependents>() = dependents;
    }
    *world.resource_mut::<DormantNativeSlots>() = DormantNativeSlots {
        definitions: Arc::clone(&definitions),
        generation: staged.generation,
        records: staged.dormant,
    };
    definitions
        .current_generation
        .store(staged.generation, Ordering::Relaxed);
    *world.resource_mut::<NativeLifecycleContext>() = NativeLifecycleContext {
        definitions,
        generation: staged.generation,
        lifecycle: snapshot.native_lifecycle.expect("prepared lifecycle state"),
        live,
        inventory: staged.inventory,
    };
}

pub fn restore_native_owners(
    world: &mut World,
    batch: &RestoreBatch,
) -> Result<NativeRestoreResult> {
    let current = capture_lifecycle(world)?;
    ensure!(
        current.schema_version != SNAPSHOT_SCHEMA_C001_INVENTORY_OWNERS,
        "schema 10 requires composite inventory owner restore"
    );
    let prepared = prepare_owner_restore(&current, batch)?;
    let completion = prepared.completion().clone();
    if prepared.already_completed() {
        return Ok(NativeRestoreResult::AlreadyCompleted(completion));
    }
    let staged = stage(
        prepared.snapshot(),
        &world.resource::<NativeLifecycleContext>().definitions,
    )?;
    commit(world, prepared.into_snapshot(), staged);
    Ok(NativeRestoreResult::AppliedInMemory(completion))
}

pub fn update_lifecycle_actor(
    world: &mut World,
    owner: OwnerHandle,
    transform: NativeTransform,
    variables: Vec<NativeVariableValue>,
) -> Result<()> {
    let mut snapshot = capture_lifecycle(world)?;
    require_handle(&snapshot, owner)?;
    let actor = snapshot
        .native_world
        .as_mut()
        .context("spatial slots missing")?
        .actors
        .iter_mut()
        .find(|actor| actor.id == owner.id)
        .context("owner spatial record missing")?;
    if world
        .resource::<NativeLifecycleContext>()
        .definitions
        .scalar_contracts
        .is_some()
    {
        ensure!(
            variables == actor.variables,
            "scalar variables require the scalar owner action API"
        );
    }
    actor.transform = transform;
    actor.variables = variables;
    SnapshotStore::preflight(&snapshot)?;
    let staged = stage(
        &snapshot,
        &world.resource::<NativeLifecycleContext>().definitions,
    )?;
    commit(world, snapshot, staged);
    Ok(())
}

pub fn reassign_lifecycle_foundation(
    world: &mut World,
    owner: OwnerHandle,
    target: Option<OwnerHandle>,
) -> Result<()> {
    let mut snapshot = capture_lifecycle(world)?;
    require_handle(&snapshot, owner)?;
    if let Some(target) = target {
        require_handle(&snapshot, target)?;
    }
    let relationships = snapshot
        .native_relationships
        .as_mut()
        .context("lifecycle relationships missing")?;
    let row = relationships
        .foundations
        .iter_mut()
        .find(|row| row.owner == owner.id)
        .context("relationship owner is not a deployable")?;
    row.foundation = target.map(|target| target.id);
    SnapshotStore::preflight(&snapshot)?;
    let staged = stage(
        &snapshot,
        &world.resource::<NativeLifecycleContext>().definitions,
    )?;
    commit(world, snapshot, staged);
    Ok(())
}

pub fn lifecycle_foundation_of(world: &World, owner: OwnerHandle) -> Result<Option<OwnerHandle>> {
    let snapshot = capture_lifecycle(world)?;
    require_handle(&snapshot, owner)?;
    let target = snapshot
        .native_relationships
        .as_ref()
        .context("lifecycle relationships missing")?
        .foundations
        .iter()
        .find(|row| row.owner == owner.id)
        .context("relationship owner is not a deployable")?
        .foundation;
    target
        .map(|id| {
            let slot = snapshot
                .native_lifecycle
                .as_ref()
                .context("lifecycle state missing")?
                .slots
                .iter()
                .find(|slot| slot.id == id && slot.presence == Presence::Live)
                .context("foundation is not live")?;
            Ok(OwnerHandle {
                id,
                incarnation: slot.incarnation,
            })
        })
        .transpose()
}

pub fn lifecycle_dependents_of(world: &World, foundation: OwnerHandle) -> Result<Vec<OwnerHandle>> {
    let snapshot = capture_lifecycle(world)?;
    require_handle(&snapshot, foundation)?;
    let reverse = world
        .get_resource::<NativeFoundationDependents>()
        .context("lifecycle relationships missing")?;
    let owners = reverse
        .0
        .get(&foundation.id)
        .context("owner is not a live foundation")?;
    owners
        .iter()
        .map(|id| {
            let slot = snapshot
                .native_lifecycle
                .as_ref()
                .context("lifecycle state missing")?
                .slots
                .iter()
                .find(|slot| slot.id == *id && slot.presence == Presence::Live)
                .context("dependent is not live")?;
            Ok(OwnerHandle {
                id: *id,
                incarnation: slot.incarnation,
            })
        })
        .collect()
}

pub fn lifecycle_world_bounds(world: &World, owner: OwnerHandle) -> Result<Aabb3> {
    let snapshot = capture_lifecycle(world)?;
    require_handle(&snapshot, owner)?;
    let actor = snapshot
        .native_world
        .as_ref()
        .context("spatial slots missing")?
        .actors
        .iter()
        .find(|actor| actor.id == owner.id)
        .context("owner spatial record missing")?;
    let context = world.resource::<NativeLifecycleContext>();
    actor.transform.transformed_bounds(
        &context
            .definitions
            .catalog
            .class(&actor.class)
            .context("owner class missing")?
            .local_bounds,
    )
}

#[cfg(test)]
use crate::scalar_owner_fixture as scalar_fixture;

#[cfg(test)]
#[path = "../tests/common/c001_owners.rs"]
mod owner_fixture;

/// A timer failure latches the activation closed; subsequent capture and CLI
/// progress checks fail instead of publishing a partially advanced world.
pub fn modifier_runtime_health(world: &World) -> Result<()> {
    let pin = crate::simulation::lifecycle_activation(world)?;
    if let Some(anchor) = world.get_resource::<ModifierRuntimeAnchor>() {
        ensure!(
            !anchor.definitions.modifier_failed.load(Ordering::Relaxed),
            "authored modifier timer step failed"
        );
        let current = world
            .get_resource::<NativeLifecycleContext>()
            .context("modifier lifecycle context missing")?;
        ensure!(
            Arc::ptr_eq(&anchor.definitions, &current.definitions),
            "modifier lifecycle activation changed"
        );
        crate::simulation::validate_lifecycle_activation(world, current)?;
    } else {
        ensure!(
            !pin.is_some_and(|context| context.definitions.mode == SourceMode::ModifierOwners)
                && !world
                    .get_resource::<NativeLifecycleContext>()
                    .is_some_and(|context| context.definitions.mode == SourceMode::ModifierOwners)
                && !world
                    .component_id::<ModifierOwnerState>()
                    .is_some_and(|component| {
                        world
                            .archetypes()
                            .iter()
                            .any(|archetype| !archetype.is_empty() && archetype.contains(component))
                    }),
            "modifier runtime anchor missing"
        );
    }
    Ok(())
}

fn capture_modifiers(
    world: &World,
    context: &NativeLifecycleContext,
) -> Result<Option<ModifierWorldSnapshot>> {
    if context.definitions.mode != SourceMode::ModifierOwners {
        for entity in context.live.values() {
            ensure!(
                world.get::<ModifierOwnerState>(*entity).is_none(),
                "unexpected modifier state"
            );
        }
        return Ok(None);
    }
    ensure!(
        context.live.len() <= MAX_MODIFIER_OWNERS,
        "modifier owner limit"
    );
    let mut refs = Vec::with_capacity(context.live.len());
    let mut entries = 0;
    let mut bytes = context.live.len() * std::mem::size_of::<OwnerModifierState>();
    for (id, entity) in &context.live {
        let value = world
            .get::<ModifierOwnerState>(*entity)
            .context("owner modifier state missing")?;
        ensure!(
            Arc::ptr_eq(&value.activation, &context.definitions)
                && value.generation == context.generation
                && value.state.owner.id == *id
                && world.get::<NativeOwnerIncarnation>(*entity)
                    == Some(&NativeOwnerIncarnation(value.state.owner.incarnation)),
            "modifier state differs from activation"
        );
        bytes += value.state.allocation_bytes()?;
        entries += value.state.entries.len();
        ensure!(
            bytes <= MAX_MODIFIER_WORLD_BYTES && entries <= MAX_WORLD_MODIFIERS,
            "modifier world allocation or entry limit"
        );
        refs.push(&value.state);
    }
    let snapshot = ModifierWorldSnapshot {
        format: 1,
        owners: refs.into_iter().cloned().collect(),
    };
    snapshot.validate(&context.lifecycle)?;
    Ok(Some(snapshot))
}

/// Return a validated copy; callers cannot transplant mutable component authority.
pub fn native_owner_modifiers(world: &World, owner: OwnerHandle) -> Result<OwnerModifierState> {
    let snapshot = capture_lifecycle(world)?;
    require_handle(&snapshot, owner)?;
    snapshot
        .native_modifiers
        .context("authored modifiers are not enabled")?
        .owners
        .into_iter()
        .find(|state| state.owner == owner)
        .context("modifier owner missing")
}

/// Stage an explicit authored operation and validate the complete world budget
/// before replacing existing private component values. No ECS callbacks run.
pub fn apply_native_modifier(world: &mut World, request: &ModifierRequest) -> Result<()> {
    let mut snapshot = capture_lifecycle(world)?;
    require_handle(&snapshot, request.owner)?;
    let state = snapshot
        .native_modifiers
        .as_mut()
        .context("authored modifiers are not enabled")?;
    let index = state
        .owners
        .binary_search_by_key(&request.owner.id, |state| state.owner.id)
        .map_err(|_| anyhow::anyhow!("modifier owner missing"))?;
    let change = prepare_modifiers(&state.owners[index], request)?;
    state.owners[index] = change.apply(&state.owners[index])?;
    let context = world.resource::<NativeLifecycleContext>();
    state.validate(&context.lifecycle)?;
    let entity = *context
        .live
        .get(&request.owner.id)
        .context("modifier owner absent")?;
    SnapshotStore::preflight(&snapshot)?;
    let mut state = snapshot.native_modifiers.expect("prepared modifier world");
    world
        .get_mut::<ModifierOwnerState>(entity)
        .expect("admitted modifier owner")
        .state = state.owners.swap_remove(index);
    Ok(())
}

/// One all-or-nothing fixed-clock advance. Integer nanoseconds are persisted,
/// so small steps cannot disappear when a long-running timer is reloaded.
pub fn advance_native_modifiers(world: &mut World, elapsed: std::time::Duration) -> Result<()> {
    let mut snapshot = capture_lifecycle(world)?;
    let state = snapshot
        .native_modifiers
        .as_mut()
        .context("authored modifiers are not enabled")?;
    let elapsed_nanos = u64::try_from(elapsed.as_nanos()).context("modifier elapsed time limit")?;
    for owner in &mut state.owners {
        let request = ModifierRequest {
            owner: owner.owner,
            expected_revision: owner.revision,
            action: ModifierAction::Advance { elapsed_nanos },
        };
        *owner = prepare_modifiers(owner, &request)?.apply(owner)?;
    }
    let context = world.resource::<NativeLifecycleContext>();
    state.validate(&context.lifecycle)?;
    let targets: Vec<_> = state
        .owners
        .iter()
        .map(|owner| context.live[&owner.owner.id])
        .collect();
    SnapshotStore::preflight(&snapshot)?;
    let state = snapshot.native_modifiers.expect("prepared modifier world");
    for (entity, state) in targets.into_iter().zip(state.owners) {
        world
            .get_mut::<ModifierOwnerState>(entity)
            .expect("admitted modifier owner")
            .state = state;
    }
    Ok(())
}

/// Closure-owned timing authority. The persisted tick supplies fractional phase;
/// a new run's validated rate applies prospectively to retained timer durations.
pub(crate) struct ModifierTickClock {
    tick_hz: u32,
    expected_tick: u64,
    fixed_interval: std::time::Duration,
}

impl ModifierTickClock {
    /// Called only after the server configuration and snapshot are validated.
    pub(crate) fn new(config: &crate::config::ServerConfig, tick: u64) -> Self {
        Self {
            tick_hz: config.tick_hz,
            expected_tick: tick,
            fixed_interval: config.tick_interval(),
        }
    }

    fn step(&self, world: &World) -> Result<(std::time::Duration, u64)> {
        let tick = world
            .get_resource::<SimulationClock>()
            .context("modifier simulation clock missing")?
            .tick;
        ensure!(
            tick == self.expected_tick,
            "modifier simulation clock changed"
        );
        let next = tick.checked_add(1).context("modifier tick exhausted")?;
        let fixed = world
            .get_resource::<bevy::prelude::Time<bevy::prelude::Fixed>>()
            .context("modifier fixed clock missing")?;
        ensure!(
            fixed.timestep() == self.fixed_interval && fixed.delta() == self.fixed_interval,
            "modifier fixed interval changed"
        );
        // Subtract rational endpoints instead of repeatedly rounding 1 / hz.
        // Any hz consecutive steps consume exactly one second, including when
        // a save/reload splits that interval. u128 covers every persisted tick.
        let nanos = |tick| u128::from(tick) * 1_000_000_000 / u128::from(self.tick_hz);
        let elapsed = u64::try_from(nanos(next) - nanos(tick))?;
        Ok((std::time::Duration::from_nanos(elapsed), next))
    }
}

/// Installed only for schema 11, before ordinary gameplay/replication. A failed
/// preparation changes no owner state and prevents all later publication.
pub(crate) fn tick_native_modifiers(
    world: &mut World,
    activation: &NativeLifecycleContext,
    clock: &mut ModifierTickClock,
) {
    let result = clock.step(world).and_then(|(elapsed, next)| {
        advance_native_modifiers(world, elapsed)?;
        clock.expected_tick = next;
        Ok(())
    });
    if result.is_err() {
        // The system closure owns this activation, even when caller-owned
        // resources are missing or transplanted. Never latch a foreign app.
        activation
            .definitions
            .modifier_failed
            .store(true, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        config::ServerConfig,
        native_lifecycle::{
            LifecycleDefinition, RecorderKind, SlotDeclaration, SpawnComponent,
            prepare_lifecycle_world,
        },
        native_world::prepare_authored_world,
        simulation::{build_app, capture},
    };

    mod absence {
        use super::*;
        use bevy::ecs::entity_disabling::{DefaultQueryFilters, Disabled};

        #[derive(Component)]
        struct Dormant;

        #[derive(Resource)]
        struct UnrelatedResource;

        fn rejects_state(world: &World, expected: &str) {
            assert_eq!(
                ensure_no_lifecycle_state(world).unwrap_err().to_string(),
                expected
            );
        }

        fn check_component<T: Component>(make: impl Fn() -> T) {
            check_component_error(make, "world contains unexpected lifecycle components");
        }

        fn check_component_error<T: Component>(make: impl Fn() -> T, expected: &str) {
            let mut world = World::new();
            assert!(world.component_id::<T>().is_none());
            ensure_no_lifecycle_state(&world).unwrap();
            let component = world.register_component::<T>();
            // Registration alone has no live value to reject.
            ensure_no_lifecycle_state(&world).unwrap();
            let offender = world.spawn(make()).id();
            let archetype = world.entity(offender).archetype().id();
            rejects_state(&world, expected);
            world.entity_mut(offender).remove::<T>();
            assert!(world.archetypes()[archetype].is_empty());
            assert!(world.archetypes()[archetype].contains(component));
            ensure_no_lifecycle_state(&world).unwrap();
            world.despawn(offender);
            // An already observed empty archetype must be checked when reused.
            let offender = world.spawn(make()).id();
            assert_eq!(world.entity(offender).archetype().id(), archetype);
            rejects_state(&world, expected);
            world.despawn(offender);
            ensure_no_lifecycle_state(&world).unwrap();

            let offender = world.spawn((Disabled, make())).id();
            rejects_state(&world, expected);
            world.despawn(offender);
            ensure_no_lifecycle_state(&world).unwrap();
            let disabling = world.register_component::<Dormant>();
            world
                .resource_mut::<DefaultQueryFilters>()
                .register_disabling_component(disabling);
            let offender = world.spawn((Dormant, make())).id();
            rejects_state(&world, expected);
            world.despawn(offender);
            ensure_no_lifecycle_state(&world).unwrap();

            // The blacklist covers resource entities too, independently of
            // which component supplies that entity's singleton resource.
            world.insert_resource(UnrelatedResource);
            let entity = world.resource_entity::<UnrelatedResource>().unwrap();
            world.entity_mut(entity).insert(make());
            rejects_state(&world, expected);
            world.entity_mut(entity).remove::<T>();
            assert!(world.contains_resource::<UnrelatedResource>());
            ensure_no_lifecycle_state(&world).unwrap();
        }

        fn check_resource<T: Resource>(make: impl Fn() -> T) {
            let mut world = World::new();
            ensure_no_lifecycle_state(&world).unwrap();
            world.insert_resource(make());
            rejects_state(&world, "world contains unexpected lifecycle resources");
            world.remove_resource::<T>().unwrap();
            ensure_no_lifecycle_state(&world).unwrap();

            // An existing singleton marker for an unrelated type prevents the
            // required marker for T from claiming this entity. Its T value is
            // still forbidden by the component guard, independently of the
            // eight singleton resource guards.
            let mut world = World::new();
            world.insert_resource(UnrelatedResource);
            let entity = world.resource_entity::<UnrelatedResource>().unwrap();
            world.entity_mut(entity).insert(make());
            assert!(!world.contains_resource::<T>());
            rejects_state(&world, "world contains unexpected lifecycle components");
            world.entity_mut(entity).remove::<T>();
            assert!(world.contains_resource::<UnrelatedResource>());
            ensure_no_lifecycle_state(&world).unwrap();
        }

        #[test]
        fn complete_blacklist_checks_live_archetypes_and_all_resource_guards() {
            let mut lifecycle = app();
            let scalar = scalar_app();
            let inventory = inventory_app();
            let owners = build_app(&ServerConfig::default(), &owner_fixture::imported()).unwrap();
            let modifiers = build_app(
                &ServerConfig::default(),
                &crate::native_modifiers::enable_modifier_owners(
                    capture(lifecycle.world_mut()).unwrap(),
                )
                .unwrap(),
            )
            .unwrap();
            check_resource(|| ModifierRuntimeAnchor {
                definitions: Arc::clone(
                    &modifiers
                        .world()
                        .resource::<ModifierRuntimeAnchor>()
                        .definitions,
                ),
            });
            check_component(|| NativeOwnerIncarnation(1));
            check_component(|| NativeOwnerName(vec![65]));
            check_component(|| {
                NativeOwnerRecorder(RecorderComponentRecord {
                    kind: RecorderKind::SpatialStateV1,
                    guid: 1,
                })
            });
            check_resource(|| {
                lifecycle
                    .world()
                    .resource::<NativeLifecycleContext>()
                    .clone()
            });
            check_resource(|| lifecycle.world().resource::<DormantNativeSlots>().clone());
            check_resource(|| owners.world().resource::<C001OwnerImportState>().clone());
            check_resource(|| {
                scalar
                    .world()
                    .resource::<NativeScalarOwnerContractsState>()
                    .clone()
            });
            check_resource(|| {
                scalar
                    .world()
                    .resource::<C001ScalarOwnerImportState>()
                    .clone()
            });
            check_component(|| {
                scalar
                    .world()
                    .get::<ScalarOwnerDerived>(scalar_entity(scalar.world(), DomainId(2)))
                    .unwrap()
                    .clone()
            });
            check_resource(|| {
                inventory
                    .world()
                    .resource::<NativeInventoryContractsState>()
                    .clone()
            });
            check_resource(|| {
                inventory
                    .world()
                    .resource::<C001InventoryOwnerImportState>()
                    .clone()
            });
            check_component(|| {
                inventory
                    .world()
                    .get::<InventoryOwnerState>(scalar_entity(inventory.world(), DomainId(2)))
                    .unwrap()
                    .clone()
            });
            check_component(|| {
                modifiers
                    .world()
                    .get::<ModifierOwnerState>(scalar_entity(modifiers.world(), DomainId(1)))
                    .unwrap()
                    .clone()
            });
        }
    }

    fn app() -> bevy::prelude::App {
        let catalog = Arc::new(
            CompiledNativeCatalog::parse(include_str!("../config/native-world.example.toml"))
                .unwrap(),
        );
        let definition = LifecycleDefinition {
            lifecycle_format: 1,
            native_catalog_id: catalog.identity().into(),
            map: "workshop".into(),
            slots: ["marker_a", "marker_b"]
                .iter()
                .enumerate()
                .map(|(i, placement)| SlotDeclaration {
                    placement: (*placement).into(),
                    owner_name: vec![65 + i as u16],
                    recorder_kind: RecorderKind::SpatialStateV1,
                    initial_presence: if i == 0 {
                        Presence::Live
                    } else {
                        Presence::Absent
                    },
                    initial_component: None,
                    spawn_component: SpawnComponent::Missing,
                })
                .collect(),
        };
        let compiled = CompiledNativeLifecycle::compile(definition, &catalog).unwrap();
        let prepared = prepare_lifecycle_world(
            prepare_authored_world(catalog, "workshop")
                .unwrap()
                .initial_snapshot(),
            &compiled,
        )
        .unwrap();
        build_app(&ServerConfig::default(), prepared.snapshot()).unwrap()
    }

    #[test]
    fn dormant_partition_rejects_missing_extra_live_and_misidentified_records() {
        for mode in 0..4 {
            let mut app = app();
            let original = app.world().resource::<DormantNativeSlots>().clone();
            let record = original.records[&DomainId(2)].clone();
            let mut dormant = app.world_mut().resource_mut::<DormantNativeSlots>();
            match mode {
                0 => {
                    dormant.records.remove(&DomainId(2));
                }
                1 => {
                    dormant.records.insert(DomainId(3), record);
                }
                2 => {
                    dormant.records.insert(DomainId(1), record);
                }
                _ => {
                    dormant.records.get_mut(&DomainId(2)).unwrap().id = DomainId(1);
                }
            }
            assert!(capture(app.world_mut()).is_err());
            app.world_mut().insert_resource(original);
            assert!(capture(app.world_mut()).is_ok());
        }
    }

    #[test]
    fn exhausted_runtime_generation_rejects_before_mutation() {
        let mut app = app();
        let mut context = app.world_mut().resource_mut::<NativeLifecycleContext>();
        context
            .definitions
            .current_generation
            .store(u64::MAX, Ordering::Relaxed);
        context.generation = u64::MAX;
        app.world_mut()
            .resource_mut::<DormantNativeSlots>()
            .generation = u64::MAX;
        let before = capture(app.world_mut()).unwrap();
        let actor = &before.native_world.as_ref().unwrap().actors[0];
        let owner = native_owner_handle(app.world(), actor.id).unwrap();
        assert!(
            update_lifecycle_actor(
                app.world_mut(),
                owner,
                actor.transform.clone(),
                actor.variables.clone()
            )
            .is_err()
        );
        assert_eq!(capture(app.world_mut()).unwrap(), before);
    }

    #[test]
    fn registry_cannot_turn_missing_live_actors_into_dormant_state() {
        let mut app = app();
        let context = app.world().resource::<NativeLifecycleContext>().clone();
        app.world_mut()
            .resource_mut::<NativeLifecycleContext>()
            .live
            .clear();
        assert!(capture(app.world_mut()).is_err());
        app.world_mut().insert_resource(context.clone());
        let entity = context.live[&DomainId(1)];
        app.world_mut().despawn(entity);
        assert!(capture(app.world_mut()).is_err());
    }

    fn scalar_app() -> bevy::prelude::App {
        build_app(&ServerConfig::default(), &scalar_fixture::imported()).unwrap()
    }

    fn scalar_entity(world: &World, id: DomainId) -> Entity {
        world.resource::<NativeLifecycleContext>().live[&id]
    }

    fn assert_scalar_calls_reject(world: &mut World, owner: OwnerHandle) {
        assert!(capture(world).is_err());
        assert!(scalar_owner_state(world, owner).is_err());
        assert!(try_use(world, owner).is_err());
        // Both the changed and unchanged Bool paths must validate every actor.
        assert!(set_enabled(world, owner, false).is_err());
        assert!(set_enabled(world, owner, true).is_err());
        assert!(restore_native_owners(world, &scalar_fixture::fixture().initial_batch()).is_err());
    }

    #[test]
    fn missing_stale_transplanted_and_hidden_derived_state_rejects() {
        use bevy::ecs::entity_disabling::Disabled;
        for mode in 0..9 {
            let mut app = scalar_app();
            let owner = native_owner_handle(app.world(), DomainId(1)).unwrap();
            let victim = scalar_entity(app.world(), DomainId(2));
            let derived = app
                .world()
                .get::<ScalarOwnerDerived>(victim)
                .unwrap()
                .clone();
            app.world_mut().entity_mut(victim).insert(Disabled);
            match mode {
                0 => {
                    app.world_mut()
                        .entity_mut(victim)
                        .remove::<ScalarOwnerDerived>();
                }
                1 => {
                    app.world_mut()
                        .get_mut::<ScalarOwnerDerived>(victim)
                        .unwrap()
                        .generation -= 1;
                }
                2 => {
                    app.world_mut()
                        .get_mut::<ScalarOwnerDerived>(victim)
                        .unwrap()
                        .owner
                        .incarnation += 1;
                }
                3 => {
                    app.world_mut()
                        .get_mut::<ScalarOwnerDerived>(victim)
                        .unwrap()
                        .input_fingerprint[0] ^= 1;
                }
                4 => {
                    app.world_mut()
                        .get_mut::<ScalarOwnerDerived>(victim)
                        .unwrap()
                        .available = false;
                }
                5 => {
                    let other = scalar_app();
                    let other_entity = scalar_entity(other.world(), DomainId(2));
                    app.world_mut().entity_mut(victim).insert(
                        other
                            .world()
                            .get::<ScalarOwnerDerived>(other_entity)
                            .unwrap()
                            .clone(),
                    );
                }
                6 => {
                    app.world_mut().spawn((Disabled, derived));
                }
                7 => {
                    let source = scalar_entity(app.world(), DomainId(1));
                    let transplanted = app
                        .world()
                        .get::<ScalarOwnerDerived>(source)
                        .unwrap()
                        .clone();
                    app.world_mut().entity_mut(victim).insert(transplanted);
                }
                _ => {
                    set_enabled(app.world_mut(), owner, true).unwrap();
                    // The old derived record has matching scalar inputs on the
                    // untouched actor but belongs to the previous generation.
                    app.world_mut().entity_mut(victim).insert(derived);
                }
            }
            let position = app.world().get::<Position>(victim).unwrap().0;
            assert_scalar_calls_reject(app.world_mut(), owner);
            assert_eq!(app.world().get::<Position>(victim).unwrap().0, position);
        }
    }

    struct ResetScalarFailure;
    impl Drop for ResetScalarFailure {
        fn drop(&mut self) {
            SCALAR_STAGE_FAILURE.with(|failure| failure.set(None));
        }
    }

    #[test]
    fn final_scalar_callback_staging_failure_is_atomic_for_action_and_replacement() {
        let mut app = scalar_app();
        let owner = native_owner_handle(app.world(), DomainId(1)).unwrap();
        set_enabled(app.world_mut(), owner, true).unwrap();
        let before = capture(app.world_mut()).unwrap();
        let context = app.world().resource::<NativeLifecycleContext>().clone();
        let mut batch = scalar_fixture::fixture().initial_batch();
        batch.sequence = 2;
        for command in &mut batch.recorders {
            let actor = before
                .native_world
                .as_ref()
                .unwrap()
                .actors
                .iter()
                .find(|actor| actor.placement == command.target_placement)
                .unwrap();
            let slot = before
                .native_lifecycle
                .as_ref()
                .unwrap()
                .slots
                .iter()
                .find(|slot| slot.id == actor.id)
                .unwrap();
            command.guid = 900 + actor.id.0 as i32;
            command.expected_presence = Presence::Live;
            command.expected_incarnation = slot.incarnation;
            command.policy = crate::native_lifecycle::ResolvePolicy::RespawnOnly;
            command.source = crate::native_lifecycle::RecorderSource::Detached {
                expected_prior_binding: None,
            };
            command.variables = scalar_fixture::scalar_pair(false, 4);
        }
        SCALAR_STAGE_FAILURE.with(|failure| failure.set(Some(DomainId(3))));
        let _reset = ResetScalarFailure;
        let error = try_use(app.world_mut(), owner).unwrap_err();
        assert!(error.to_string().contains("injected scalar callback"));
        assert_eq!(capture(app.world_mut()).unwrap(), before);
        let error = restore_native_owners(app.world_mut(), &batch).unwrap_err();
        assert!(error.to_string().contains("injected scalar callback"));
        assert_eq!(capture(app.world_mut()).unwrap(), before);
        let current = app.world().resource::<NativeLifecycleContext>();
        assert_eq!(current.generation, context.generation);
        assert_eq!(current.live, context.live);
        for entity in current.live.values() {
            assert_eq!(
                app.world()
                    .get::<ScalarOwnerDerived>(*entity)
                    .unwrap()
                    .generation,
                context.generation
            );
        }
    }

    #[test]
    fn scalar_generation_exhaustion_and_oversized_capacity_fail_before_writes() {
        let mut app = scalar_app();
        let owner = native_owner_handle(app.world(), DomainId(1)).unwrap();
        let entities: Vec<_> = app
            .world()
            .resource::<NativeLifecycleContext>()
            .live
            .values()
            .copied()
            .collect();
        {
            let mut context = app.world_mut().resource_mut::<NativeLifecycleContext>();
            context
                .definitions
                .current_generation
                .store(u64::MAX, Ordering::Relaxed);
            context.generation = u64::MAX;
        }
        app.world_mut()
            .resource_mut::<DormantNativeSlots>()
            .generation = u64::MAX;
        for entity in &entities {
            app.world_mut()
                .get_mut::<ScalarOwnerDerived>(*entity)
                .unwrap()
                .generation = u64::MAX;
        }
        let before = capture(app.world_mut()).unwrap();
        assert!(set_enabled(app.world_mut(), owner, true).is_err());
        let enabled = native_owner_handle(app.world(), DomainId(2)).unwrap();
        assert!(try_use(app.world_mut(), enabled).is_err());
        assert_eq!(capture(app.world_mut()).unwrap(), before);
        let entity = scalar_entity(app.world(), DomainId(2));
        app.world_mut()
            .get_mut::<NativeVariables>(entity)
            .unwrap()
            .0
            .reserve(20);
        assert_scalar_calls_reject(app.world_mut(), owner);
    }

    #[test]
    fn legacy_world_refuses_even_hidden_scalar_derived_components() {
        use bevy::ecs::entity_disabling::Disabled;
        let scalar = scalar_app();
        let entity = scalar_entity(scalar.world(), DomainId(2));
        let derived = scalar
            .world()
            .get::<ScalarOwnerDerived>(entity)
            .unwrap()
            .clone();
        for mut app in [
            app(),
            build_app(&ServerConfig::default(), &WorldSnapshot::default()).unwrap(),
        ] {
            app.world_mut().spawn((Disabled, derived.clone()));
            assert!(capture(app.world_mut()).is_err());
        }
    }

    fn inventory_app() -> bevy::prelude::App {
        build_app(
            &ServerConfig::default(),
            &crate::inventory_owner_fixture::imported(),
        )
        .unwrap()
    }

    fn inventory_noop(world: &World, owner: OwnerHandle) -> InventoryActionRequestV1 {
        use crate::native_inventory::{
            InventoryActionV1, InventorySlotEndpointV1, InventoryStackEndpointV1,
        };
        let current = inventory_owner_state(world, owner).unwrap();
        let bag = current
            .owner
            .inventories
            .iter()
            .find(|inventory| inventory.key == "bag")
            .unwrap();
        InventoryActionRequestV1 {
            expected_inventory_revision: current.revision,
            action: InventoryActionV1::Move {
                source: InventoryStackEndpointV1 {
                    owner,
                    inventory: "bag".into(),
                    stack_id: bag.slots[0].as_ref().unwrap().id,
                },
                destination: InventorySlotEndpointV1 {
                    owner,
                    inventory: "bag".into(),
                    slot: 0,
                },
            },
        }
    }

    fn inventory_replacement(world: &World) -> InventoryRestoreBatchV1 {
        use crate::native_inventory_restore::InventoryOwnerRestoreV1;
        let snapshot = capture_lifecycle(world).unwrap();
        let native = snapshot.native_world.as_ref().unwrap();
        let lifecycle = snapshot.native_lifecycle.as_ref().unwrap();
        let inventories = snapshot.native_inventories.as_ref().unwrap();
        let mut batch = crate::inventory_owner_fixture::fixture().lifecycle_batch;
        batch.sequence = lifecycle.restore_sequence + 1;
        for command in &mut batch.recorders {
            let actor = native
                .actors
                .iter()
                .find(|actor| actor.placement == command.target_placement)
                .unwrap();
            let slot = lifecycle
                .slots
                .iter()
                .find(|slot| slot.id == actor.id)
                .unwrap();
            command.guid = 900 + actor.id.0 as i32;
            command.expected_presence = Presence::Live;
            command.expected_incarnation = slot.incarnation;
            command.policy = crate::native_lifecycle::ResolvePolicy::RespawnOnly;
            command.source = crate::native_lifecycle::RecorderSource::Detached {
                expected_prior_binding: None,
            };
            command.create_if_missing = true;
            command.transform = actor.transform.clone();
            command.variables = actor.variables.clone();
        }
        InventoryRestoreBatchV1 {
            format: 1,
            scalar_contract_id: snapshot
                .native_scalar_contracts
                .as_ref()
                .unwrap()
                .contract_id
                .clone(),
            inventory_contract_id: inventories.contract_id.clone(),
            expected_inventory_revision: inventories.revision,
            lifecycle: batch,
            owners: native
                .actors
                .iter()
                .map(|actor| InventoryOwnerRestoreV1 {
                    target_placement: actor.placement.clone(),
                    inventories: inventories
                        .owners
                        .iter()
                        .find(|owner| owner.owner.id == actor.id)
                        .unwrap()
                        .inventories
                        .clone(),
                })
                .collect(),
        }
    }

    fn assert_inventory_calls_reject(
        world: &mut World,
        owner: OwnerHandle,
        noop: &InventoryActionRequestV1,
        batch: &InventoryRestoreBatchV1,
    ) {
        assert!(capture(world).is_err());
        assert!(inventory_owner_state(world, owner).is_err());
        if let crate::native_inventory::InventoryActionV1::Move { source, .. } = &noop.action {
            assert!(inventory_stack(world, owner, &source.inventory, source.stack_id).is_err());
        }
        assert!(native_owner_handle(world, owner.id).is_err());
        assert!(scalar_owner_state(world, owner).is_err());
        assert!(apply_native_inventory_action(world, noop).is_err());
        assert!(set_enabled(world, owner, true).is_err());
        assert!(try_use(world, owner).is_err());
        assert!(
            update_lifecycle_actor(
                world,
                owner,
                NativeTransform::default(),
                scalar_fixture::scalar_pair(true, 1)
            )
            .is_err()
        );
        assert!(reassign_lifecycle_foundation(world, owner, None).is_err());
        assert!(restore_native_inventory_owners(world, batch).is_err());
        assert!(restore_native_owners(world, &batch.lifecycle).is_err());
    }

    #[derive(Component)]
    #[relationship(relationship_target = LifetimeDependents)]
    struct LifetimeOwner(Entity);

    #[derive(Component, Default)]
    #[relationship_target(relationship = LifetimeOwner, linked_spawn)]
    struct LifetimeDependents(Vec<Entity>);

    #[test]
    fn legacy_lifecycle_modes_also_reject_lifetime_components_on_owners() {
        use bevy::prelude::Children;
        for mut app in [app(), scalar_app()] {
            let entity = scalar_entity(app.world(), DomainId(1));
            let owner = native_owner_handle(app.world(), DomainId(1)).unwrap();
            let before = capture(app.world_mut()).unwrap();
            app.world_mut()
                .entity_mut(entity)
                .insert(Children::default());
            assert!(capture(app.world_mut()).is_err());
            assert!(native_owner_handle(app.world(), owner.id).is_err());
            app.world_mut().entity_mut(entity).remove::<Children>();
            assert!(capture(app.world_mut()).unwrap() == before);
            app.world_mut()
                .entity_mut(entity)
                .insert(LifetimeDependents::default());
            assert!(capture(app.world_mut()).is_err());
            assert!(native_owner_handle(app.world(), owner.id).is_err());
            app.world_mut()
                .entity_mut(entity)
                .remove::<LifetimeDependents>();
            assert!(capture(app.world_mut()).unwrap() == before);
        }
    }

    #[test]
    fn legacy_capture_rejects_insert_observers_for_absent_optional_components() {
        use bevy::{ecs::lifecycle::Insert, prelude::On};
        for mut app in [app(), scalar_app()] {
            let before = capture(app.world_mut()).unwrap();
            let observer = app
                .world_mut()
                .add_observer(|_: On<Insert<NativeOwnerRecorder>>| {
                    panic!("observer entered commit")
                })
                .id();
            assert!(capture(app.world_mut()).is_err());
            app.world_mut().despawn(observer);
            assert!(capture(app.world_mut()).unwrap() == before);
        }
    }

    #[test]
    fn never_instantiated_optional_component_hooks_reject_without_execution() {
        fn attack(_: bevy::ecs::world::DeferredWorld, _: bevy::ecs::lifecycle::HookContext) {
            panic!("hook entered commit")
        }
        for phase in 0..5 {
            let mut app = app();
            let before = capture(app.world_mut()).unwrap();
            let generation = app.world().resource::<NativeLifecycleContext>().generation;
            let hooks = app
                .world_mut()
                .register_component_hooks::<NativeOwnerRecorder>();
            match phase {
                0 => hooks.on_add(attack),
                1 => hooks.on_insert(attack),
                2 => hooks.on_discard(attack),
                3 => hooks.on_remove(attack),
                _ => hooks.on_despawn(attack),
            };
            assert!(capture(app.world_mut()).is_err());
            let context = app.world().resource::<NativeLifecycleContext>();
            assert_eq!(context.lifecycle, before.native_lifecycle.unwrap());
            assert_eq!(context.generation, generation);
        }
    }

    #[test]
    fn optional_component_requirements_reject_before_replacing_a_live_owner() {
        use crate::native_lifecycle::{RecorderSource, ResolvePolicy, RestoreRecorder};

        #[derive(Component, Default)]
        struct Bridge;
        #[derive(Component)]
        struct Leaf;

        for transitive in [false, true] {
            let mut app = app();
            let before = capture(app.world_mut()).unwrap();
            let original = app.world().resource::<NativeLifecycleContext>().clone();
            let dormant = app.world().resource::<DormantNativeSlots>().clone();
            let actor = &before.native_world.as_ref().unwrap().actors[0];
            let lifecycle = before.native_lifecycle.as_ref().unwrap();
            let slot = &lifecycle.slots[0];
            let request = RestoreBatch {
                lifecycle_id: lifecycle.lifecycle_id.clone(),
                sequence: lifecycle.restore_sequence + 1,
                recorders: vec![RestoreRecorder {
                    guid: 7,
                    kind: RecorderKind::SpatialStateV1,
                    target_placement: actor.placement.clone(),
                    owner_name: lifecycle.definition.slots[0].owner_name.clone(),
                    policy: ResolvePolicy::RespawnOnly,
                    source: RecorderSource::Detached {
                        expected_prior_binding: None,
                    },
                    create_if_missing: true,
                    expected_presence: slot.presence,
                    expected_incarnation: slot.incarnation,
                    transform: actor.transform.clone(),
                    variables: actor.variables.clone(),
                }],
            };
            // Prove rejection cannot come from a malformed restore request.
            let prepared = prepare_owner_restore(&before, &request).unwrap();
            assert!(
                prepared.snapshot().native_lifecycle.as_ref().unwrap().slots[0]
                    .component
                    .is_some()
            );
            let calls = Arc::new(AtomicU64::new(0));
            let constructor_calls = Arc::clone(&calls);
            let constructor = move || -> Leaf {
                constructor_calls.fetch_add(1, Ordering::Relaxed);
                panic!("required component constructor entered commit");
            };
            if transitive {
                app.world_mut()
                    .register_required_components_with::<Bridge, Leaf>(constructor);
                app.world_mut()
                    .register_required_components::<NativeOwnerRecorder, Bridge>();
            } else {
                app.world_mut()
                    .register_required_components_with::<NativeOwnerRecorder, Leaf>(constructor);
            }
            assert!(capture(app.world_mut()).is_err());
            assert!(restore_native_owners(app.world_mut(), &request).is_err());
            app.world_mut().flush();
            assert_eq!(calls.load(Ordering::Relaxed), 0);
            let current = app.world().resource::<NativeLifecycleContext>();
            assert_eq!(current.generation, original.generation);
            assert_eq!(current.lifecycle, original.lifecycle);
            assert_eq!(current.live, original.live);
            assert_eq!(
                app.world().resource::<DormantNativeSlots>().records,
                dormant.records
            );
            let entity = current.live[&actor.id];
            assert_eq!(
                app.world().get::<Position>(entity).unwrap().0,
                actor.transform.translation
            );
            assert_eq!(
                app.world().get::<NativeOwnerIncarnation>(entity).unwrap().0,
                slot.incarnation
            );
            assert!(app.world().get::<NativeOwnerRecorder>(entity).is_none());
        }
    }

    #[test]
    fn lifetime_relationships_reject_before_inventory_writes_and_repair_allows_retry() {
        use bevy::{
            ecs::entity_disabling::Disabled,
            prelude::{ChildOf, Children},
        };
        for mode in 0..8 {
            let mut app = inventory_app();
            let owner = native_owner_handle(app.world(), DomainId(3)).unwrap();
            let noop = inventory_noop(app.world(), owner);
            let batch = inventory_replacement(app.world());
            let before = capture(app.world_mut()).unwrap();
            let context = app.world().resource::<NativeLifecycleContext>().clone();
            let alpha = context.live[&DomainId(1)];
            let zulu = context.live[&DomainId(3)];
            let unrelated = app.world_mut().spawn_empty().id();
            app.world_mut().entity_mut(zulu).insert(Disabled);
            match mode {
                0 => {
                    app.world_mut().entity_mut(zulu).insert(ChildOf(alpha));
                }
                1 => {
                    app.world_mut().entity_mut(zulu).insert(ChildOf(unrelated));
                }
                2 => {
                    app.world_mut().entity_mut(unrelated).insert(ChildOf(zulu));
                }
                3 => {
                    app.world_mut().entity_mut(zulu).insert(Children::default());
                }
                4 => {
                    app.world_mut()
                        .entity_mut(zulu)
                        .insert(LifetimeOwner(alpha));
                }
                5 => {
                    app.world_mut()
                        .entity_mut(zulu)
                        .insert(LifetimeOwner(unrelated));
                }
                6 => {
                    app.world_mut()
                        .entity_mut(unrelated)
                        .insert(LifetimeOwner(zulu));
                }
                _ => {
                    app.world_mut()
                        .entity_mut(zulu)
                        .insert(LifetimeDependents::default());
                }
            }
            let entities: BTreeSet<_> = app
                .world()
                .iter_entities()
                .map(|entity| entity.id())
                .collect();
            let inventories: Vec<_> = context
                .live
                .values()
                .map(|entity| {
                    (
                        *entity,
                        app.world()
                            .get::<InventoryOwnerState>(*entity)
                            .unwrap()
                            .clone(),
                    )
                })
                .collect();
            assert!(capture(app.world_mut()).is_err(), "lifetime mode {mode}");
            assert_inventory_calls_reject(app.world_mut(), owner, &noop, &batch);
            let after = app.world().resource::<NativeLifecycleContext>();
            assert_eq!(after.generation, context.generation);
            assert_eq!(after.live, context.live);
            assert_eq!(
                after.definitions.current_generation.load(Ordering::Relaxed),
                context.generation
            );
            assert_eq!(
                after.inventory.as_ref().unwrap().revision,
                context.inventory.as_ref().unwrap().revision
            );
            assert_eq!(
                after.inventory.as_ref().unwrap().fingerprint,
                context.inventory.as_ref().unwrap().fingerprint
            );
            assert_eq!(
                after.inventory.as_ref().unwrap().next_stack_id,
                context.inventory.as_ref().unwrap().next_stack_id
            );
            assert_eq!(
                after.inventory.as_ref().unwrap().consumed,
                context.inventory.as_ref().unwrap().consumed
            );
            assert_eq!(
                app.world()
                    .iter_entities()
                    .map(|entity| entity.id())
                    .collect::<BTreeSet<_>>(),
                entities
            );
            for (entity, before) in inventories {
                let after = app.world().get::<InventoryOwnerState>(entity).unwrap();
                assert_eq!(after.inventories, before.inventories);
                assert_eq!(after.owner, before.owner);
                assert_eq!(after.generation, before.generation);
                assert!(Arc::ptr_eq(&after.activation, &before.activation));
            }
            // Remove sources first, then any deliberately empty targets.
            for entity in [alpha, zulu, unrelated] {
                app.world_mut()
                    .entity_mut(entity)
                    .remove::<(ChildOf, LifetimeOwner)>();
            }
            for entity in [alpha, zulu, unrelated] {
                app.world_mut()
                    .entity_mut(entity)
                    .remove::<(Children, LifetimeDependents)>();
            }
            assert!(capture(app.world_mut()).unwrap() == before);
            assert_eq!(
                apply_native_inventory_action(app.world_mut(), &noop).unwrap(),
                InventoryActionOutcomeV1::NoChange
            );
            assert!(matches!(
                restore_native_inventory_owners(app.world_mut(), &batch).unwrap(),
                NativeRestoreResult::AppliedInMemory(_)
            ));
            let committed = capture(app.world_mut()).unwrap();
            assert!(matches!(
                restore_native_inventory_owners(app.world_mut(), &batch).unwrap(),
                NativeRestoreResult::AlreadyCompleted(_)
            ));
            assert!(capture(app.world_mut()).unwrap() == committed);
        }
    }

    #[test]
    fn unrelated_lifetime_relationships_do_not_block_managed_owners() {
        use bevy::prelude::ChildOf;
        let mut app = inventory_app();
        let owner = native_owner_handle(app.world(), DomainId(3)).unwrap();
        let noop = inventory_noop(app.world(), owner);
        let batch = inventory_replacement(app.world());
        let before = capture(app.world_mut()).unwrap();
        let parent = app.world_mut().spawn_empty().id();
        let child = app
            .world_mut()
            .spawn((ChildOf(parent), LifetimeOwner(parent)))
            .id();
        assert!(capture(app.world_mut()).unwrap() == before);
        assert_eq!(
            apply_native_inventory_action(app.world_mut(), &noop).unwrap(),
            InventoryActionOutcomeV1::NoChange
        );
        restore_native_inventory_owners(app.world_mut(), &batch).unwrap();
        assert_eq!(app.world().get::<ChildOf>(child).unwrap().parent(), parent);
        assert_eq!(app.world().get::<LifetimeOwner>(child).unwrap().0, parent);
        assert!(
            inventory_owner_state(
                app.world(),
                native_owner_handle(app.world(), owner.id).unwrap()
            )
            .is_ok()
        );
    }

    #[test]
    fn inventory_missing_stale_cross_activation_transplanted_and_hidden_components_reject() {
        use bevy::ecs::entity_disabling::Disabled;
        for mode in 0..8 {
            let mut app = inventory_app();
            let owner = native_owner_handle(app.world(), DomainId(3)).unwrap();
            let noop = inventory_noop(app.world(), owner);
            let batch = inventory_replacement(app.world());
            let victim = scalar_entity(app.world(), DomainId(2));
            let original = app
                .world()
                .get::<InventoryOwnerState>(victim)
                .unwrap()
                .clone();
            app.world_mut().entity_mut(victim).insert(Disabled);
            match mode {
                0 => {
                    app.world_mut()
                        .entity_mut(victim)
                        .remove::<InventoryOwnerState>();
                }
                1 => {
                    app.world_mut()
                        .get_mut::<InventoryOwnerState>(victim)
                        .unwrap()
                        .generation -= 1;
                }
                2 => {
                    app.world_mut()
                        .get_mut::<InventoryOwnerState>(victim)
                        .unwrap()
                        .owner
                        .incarnation += 1;
                }
                3 => {
                    let other = inventory_app();
                    let other_entity = scalar_entity(other.world(), DomainId(2));
                    app.world_mut().entity_mut(victim).insert(
                        other
                            .world()
                            .get::<InventoryOwnerState>(other_entity)
                            .unwrap()
                            .clone(),
                    );
                }
                4 => {
                    let source = scalar_entity(app.world(), DomainId(1));
                    let transplanted = app
                        .world()
                        .get::<InventoryOwnerState>(source)
                        .unwrap()
                        .clone();
                    app.world_mut().entity_mut(victim).insert(transplanted);
                }
                5 => {
                    let alpha = native_owner_handle(app.world(), DomainId(1)).unwrap();
                    set_enabled(app.world_mut(), alpha, true).unwrap();
                    // Identical contents and owner, but a prior generation.
                    app.world_mut().entity_mut(victim).insert(original);
                }
                6 => {
                    app.world_mut().spawn((Disabled, original));
                }
                _ => {
                    app.world_mut()
                        .get_mut::<InventoryOwnerState>(victim)
                        .unwrap()
                        .inventories
                        .reserve(32);
                }
            }
            let generation = app.world().resource::<NativeLifecycleContext>().generation;
            let live = app
                .world()
                .resource::<NativeLifecycleContext>()
                .live
                .clone();
            assert!(capture(app.world_mut()).is_err(), "corruption mode {mode}");
            assert_inventory_calls_reject(app.world_mut(), owner, &noop, &batch);
            assert_eq!(
                app.world().resource::<NativeLifecycleContext>().generation,
                generation
            );
            assert_eq!(app.world().resource::<NativeLifecycleContext>().live, live);
        }
    }

    #[test]
    fn inventory_checksum_rejects_header_damage_and_conservation_valid_direct_component_edits() {
        for mode in 0..7 {
            let mut app = inventory_app();
            let owner = native_owner_handle(app.world(), DomainId(3)).unwrap();
            let noop = inventory_noop(app.world(), owner);
            let batch = inventory_replacement(app.world());
            match mode {
                0..=3 => {
                    let mut context = app.world_mut().resource_mut::<NativeLifecycleContext>();
                    let header = context.inventory.as_mut().unwrap();
                    match mode {
                        0 => header.revision += 1,
                        1 => header.next_stack_id += 1,
                        2 => header.consumed[0].count += 1,
                        _ => header.consumed.reserve(4096),
                    }
                }
                4 => {
                    app.world_mut()
                        .resource_mut::<NativeLifecycleContext>()
                        .inventory = None;
                }
                5 => {
                    let entity = scalar_entity(app.world(), DomainId(2));
                    let mut component = app
                        .world_mut()
                        .get_mut::<InventoryOwnerState>(entity)
                        .unwrap();
                    let bag = component
                        .inventories
                        .iter_mut()
                        .find(|inventory| inventory.key == "bag")
                        .unwrap();
                    // Moving to an eligible empty slot preserves conservation.
                    bag.slots.swap(0, 1);
                }
                _ => {
                    let entity = scalar_entity(app.world(), DomainId(3));
                    let mut component = app
                        .world_mut()
                        .get_mut::<InventoryOwnerState>(entity)
                        .unwrap();
                    component
                        .inventories
                        .iter_mut()
                        .find(|inventory| inventory.key == "bag")
                        .unwrap()
                        .overflow
                        .reserve(600);
                }
            }
            assert_inventory_calls_reject(app.world_mut(), owner, &noop, &batch);
        }
    }

    #[test]
    fn inventory_global_context_rollback_and_transplant_cannot_reset_generation_or_header() {
        for mode in 0..4 {
            let mut app = inventory_app();
            let owner = native_owner_handle(app.world(), DomainId(3)).unwrap();
            let noop = inventory_noop(app.world(), owner);
            let batch = inventory_replacement(app.world());
            let old = app.world().resource::<NativeLifecycleContext>().clone();
            let old_dormant = app.world().resource::<DormantNativeSlots>().clone();
            set_enabled(app.world_mut(), owner, false).unwrap();
            match mode {
                0 => {
                    app.world_mut().insert_resource(old);
                }
                1 => {
                    app.world_mut().insert_resource(old);
                    app.world_mut().insert_resource(old_dormant);
                }
                2 => {
                    let other = inventory_app();
                    app.world_mut().insert_resource(
                        other.world().resource::<NativeLifecycleContext>().clone(),
                    );
                }
                _ => {
                    use crate::native_inventory::InventoryActionV1;
                    let InventoryActionV1::Move { source, .. } = noop.action.clone() else {
                        unreachable!()
                    };
                    apply_native_inventory_action(
                        app.world_mut(),
                        &InventoryActionRequestV1 {
                            expected_inventory_revision: 1,
                            action: InventoryActionV1::Consume { source, amount: 1 },
                        },
                    )
                    .unwrap();
                    app.world_mut()
                        .resource_mut::<NativeLifecycleContext>()
                        .inventory = old.inventory;
                }
            }
            assert_inventory_calls_reject(app.world_mut(), owner, &noop, &batch);
        }
    }

    #[test]
    fn inventory_readonly_contract_and_provenance_pins_and_required_resources_reject_damage() {
        use bevy::ecs::entity_disabling::Disabled;
        for mode in 0..9 {
            let mut app = inventory_app();
            let owner = native_owner_handle(app.world(), DomainId(3)).unwrap();
            let noop = inventory_noop(app.world(), owner);
            let batch = inventory_replacement(app.world());
            match mode {
                0 => {
                    app.world_mut()
                        .remove_resource::<NativeInventoryContractsState>();
                }
                1 => {
                    app.world_mut()
                        .remove_resource::<C001InventoryOwnerImportState>();
                }
                2 => {
                    app.world_mut()
                        .remove_resource::<NativeScalarOwnerContractsState>();
                }
                3 => {
                    app.world_mut().remove_resource::<PreservedProspectState>();
                }
                4 => {
                    Arc::make_mut(
                        &mut app
                            .world_mut()
                            .resource_mut::<NativeInventoryContractsState>()
                            .0,
                    )
                    .contract_id = "f".repeat(64);
                }
                5 => {
                    Arc::make_mut(
                        &mut app
                            .world_mut()
                            .resource_mut::<C001InventoryOwnerImportState>()
                            .0,
                    )
                    .inventory_initial
                    .initial_next_stack_id += 1;
                }
                6 => {
                    let mut state = app
                        .world()
                        .resource::<NativeInventoryContractsState>()
                        .clone();
                    // Bevy removes a second resource automatically. Remove the
                    // original first so this installs a real hidden replacement.
                    Arc::make_mut(&mut state.0).contract_id = "f".repeat(64);
                    app.world_mut()
                        .remove_resource::<NativeInventoryContractsState>();
                    app.world_mut().spawn((Disabled, state));
                }
                7 => {
                    let mut metadata = app
                        .world()
                        .resource::<C001InventoryOwnerImportState>()
                        .clone();
                    Arc::make_mut(&mut metadata.0)
                        .inventory_initial
                        .initial_next_stack_id += 1;
                    app.world_mut()
                        .remove_resource::<C001InventoryOwnerImportState>();
                    app.world_mut().spawn((Disabled, metadata));
                }
                _ => {
                    app.world_mut()
                        .remove_resource::<NativeFoundationDependents>();
                }
            }
            assert!(capture(app.world_mut()).is_err(), "pin mode {mode}");
            assert_inventory_calls_reject(app.world_mut(), owner, &noop, &batch);
        }
    }

    #[test]
    fn final_owner_callback_failure_leaves_inventory_scalars_entities_and_generation_unchanged() {
        use crate::native_inventory::InventoryActionV1;
        let mut app = inventory_app();
        let owner = native_owner_handle(app.world(), DomainId(3)).unwrap();
        let noop = inventory_noop(app.world(), owner);
        let InventoryActionV1::Move { source, .. } = noop.action else {
            unreachable!()
        };
        let request = InventoryActionRequestV1 {
            expected_inventory_revision: 1,
            action: InventoryActionV1::Consume { source, amount: 1 },
        };
        let batch = inventory_replacement(app.world());
        let before = capture(app.world_mut()).unwrap();
        let context = app.world().resource::<NativeLifecycleContext>().clone();
        SCALAR_STAGE_FAILURE.with(|failure| failure.set(Some(DomainId(3))));
        let _reset = ResetScalarFailure;
        let error = apply_native_inventory_action(app.world_mut(), &request).unwrap_err();
        assert!(error.to_string().contains("injected scalar callback"));
        assert!(capture(app.world_mut()).unwrap() == before);
        let error = restore_native_inventory_owners(app.world_mut(), &batch).unwrap_err();
        assert!(error.to_string().contains("injected scalar callback"));
        assert!(capture(app.world_mut()).unwrap() == before);
        let current = app.world().resource::<NativeLifecycleContext>();
        assert_eq!(current.generation, context.generation);
        assert_eq!(current.live, context.live);
        for entity in current.live.values() {
            assert_eq!(
                app.world()
                    .get::<InventoryOwnerState>(*entity)
                    .unwrap()
                    .generation,
                context.generation
            );
            assert_eq!(
                app.world()
                    .get::<ScalarOwnerDerived>(*entity)
                    .unwrap()
                    .generation,
                context.generation
            );
        }
    }

    #[test]
    fn inventory_runtime_generation_exhaustion_is_atomic_but_validated_noop_needs_no_generation() {
        let mut app = inventory_app();
        let owner = native_owner_handle(app.world(), DomainId(3)).unwrap();
        let noop = inventory_noop(app.world(), owner);
        let batch = inventory_replacement(app.world());
        let live = app
            .world()
            .resource::<NativeLifecycleContext>()
            .live
            .clone();
        {
            let mut context = app.world_mut().resource_mut::<NativeLifecycleContext>();
            context
                .definitions
                .current_generation
                .store(u64::MAX, Ordering::Relaxed);
            context.generation = u64::MAX;
        }
        app.world_mut()
            .resource_mut::<DormantNativeSlots>()
            .generation = u64::MAX;
        for entity in live.values() {
            app.world_mut()
                .get_mut::<InventoryOwnerState>(*entity)
                .unwrap()
                .generation = u64::MAX;
            app.world_mut()
                .get_mut::<ScalarOwnerDerived>(*entity)
                .unwrap()
                .generation = u64::MAX;
        }
        let before = capture(app.world_mut()).unwrap();
        assert_eq!(
            apply_native_inventory_action(app.world_mut(), &noop).unwrap(),
            InventoryActionOutcomeV1::NoChange
        );
        let crate::native_inventory::InventoryActionV1::Move { source, .. } = noop.action else {
            unreachable!()
        };
        let consume = InventoryActionRequestV1 {
            expected_inventory_revision: 1,
            action: crate::native_inventory::InventoryActionV1::Consume { source, amount: 1 },
        };
        assert!(apply_native_inventory_action(app.world_mut(), &consume).is_err());
        assert!(set_enabled(app.world_mut(), owner, false).is_err());
        assert!(try_use(app.world_mut(), owner).is_err());
        assert!(restore_native_inventory_owners(app.world_mut(), &batch).is_err());
        assert!(capture(app.world_mut()).unwrap() == before);
        assert_eq!(app.world().resource::<NativeLifecycleContext>().live, live);
    }

    #[test]
    fn inventory_and_scalar_components_share_the_current_pin_after_every_mutation_path() {
        use crate::native_inventory::InventoryActionV1;
        fn assert_pin(world: &World, generation: u64) {
            let context = world.resource::<NativeLifecycleContext>();
            assert_eq!(context.generation, generation);
            assert_eq!(
                context
                    .definitions
                    .current_generation
                    .load(Ordering::Relaxed),
                generation
            );
            for entity in context.live.values() {
                let inventory = world.get::<InventoryOwnerState>(*entity).unwrap();
                let scalar = world.get::<ScalarOwnerDerived>(*entity).unwrap();
                assert_eq!(inventory.generation, generation);
                assert_eq!(scalar.generation, generation);
                assert!(Arc::ptr_eq(&inventory.activation, &context.definitions));
                assert!(Arc::ptr_eq(&scalar.activation, &context.definitions));
            }
        }
        let mut app = inventory_app();
        let owner = native_owner_handle(app.world(), DomainId(3)).unwrap();
        let alpha = native_owner_handle(app.world(), DomainId(1)).unwrap();
        let noop = inventory_noop(app.world(), owner);
        let InventoryActionV1::Move { source, .. } = noop.action.clone() else {
            unreachable!()
        };
        let original_entities = app
            .world()
            .resource::<NativeLifecycleContext>()
            .live
            .clone();
        assert_pin(app.world(), 1);
        apply_native_inventory_action(app.world_mut(), &noop).unwrap();
        assert_pin(app.world(), 1);
        apply_native_inventory_action(
            app.world_mut(),
            &InventoryActionRequestV1 {
                expected_inventory_revision: 1,
                action: InventoryActionV1::Consume { source, amount: 1 },
            },
        )
        .unwrap();
        assert_pin(app.world(), 2);
        set_enabled(app.world_mut(), alpha, true).unwrap();
        assert_pin(app.world(), 3);
        try_use(app.world_mut(), alpha).unwrap();
        assert_pin(app.world(), 4);
        let snapshot = capture_lifecycle(app.world()).unwrap();
        let actor = snapshot
            .native_world
            .as_ref()
            .unwrap()
            .actors
            .iter()
            .find(|actor| actor.id == owner.id)
            .unwrap();
        let mut pose = actor.transform.clone();
        pose.translation[0] += 0.125;
        update_lifecycle_actor(app.world_mut(), owner, pose, actor.variables.clone()).unwrap();
        assert_pin(app.world(), 5);
        reassign_lifecycle_foundation(app.world_mut(), owner, Some(alpha)).unwrap();
        assert_pin(app.world(), 6);
        assert_eq!(
            app.world().resource::<NativeLifecycleContext>().live,
            original_entities
        );
        let batch = inventory_replacement(app.world());
        restore_native_inventory_owners(app.world_mut(), &batch).unwrap();
        assert_pin(app.world(), 7);
        let after = capture_lifecycle(app.world()).unwrap();
        restore_native_inventory_owners(app.world_mut(), &batch).unwrap();
        assert_pin(app.world(), 7);
        assert!(capture_lifecycle(app.world()).unwrap() == after);
    }

    #[test]
    fn legacy_activations_reject_inventory_components_resources_and_private_headers() {
        use bevy::ecs::entity_disabling::Disabled;
        let inventory = inventory_app();
        let entity = scalar_entity(inventory.world(), DomainId(2));
        for mode in 0..4 {
            for mut app in [
                app(),
                scalar_app(),
                build_app(&ServerConfig::default(), &WorldSnapshot::default()).unwrap(),
            ] {
                match mode {
                    0 => {
                        app.world_mut().spawn((
                            Disabled,
                            inventory
                                .world()
                                .get::<InventoryOwnerState>(entity)
                                .unwrap()
                                .clone(),
                        ));
                    }
                    1 => {
                        app.world_mut().insert_resource(
                            inventory
                                .world()
                                .resource::<NativeInventoryContractsState>()
                                .clone(),
                        );
                    }
                    2 => {
                        app.world_mut().insert_resource(
                            inventory
                                .world()
                                .resource::<C001InventoryOwnerImportState>()
                                .clone(),
                        );
                    }
                    _ => {
                        if let Some(mut context) =
                            app.world_mut().get_resource_mut::<NativeLifecycleContext>()
                        {
                            context.inventory = inventory
                                .world()
                                .resource::<NativeLifecycleContext>()
                                .inventory
                                .clone();
                        } else {
                            app.world_mut().spawn((
                                Disabled,
                                inventory
                                    .world()
                                    .get::<InventoryOwnerState>(entity)
                                    .unwrap()
                                    .clone(),
                            ));
                        }
                    }
                }
                assert!(capture(app.world_mut()).is_err());
            }
        }
    }
}

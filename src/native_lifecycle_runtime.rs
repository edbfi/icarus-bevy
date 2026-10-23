//! Authored declared-slot owners. Live spatial values belong to ECS components;
//! dormant values have a disjoint resource. Nothing here imports official state.
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use anyhow::{Context, Result, ensure};
use bevy::prelude::{Component, Entity, Resource, World};

use crate::{
    native_lifecycle::{
        CompiledNativeLifecycle, CompletedRestore, LifecycleSnapshot, OwnerHandle, Presence,
        RecorderComponentRecord, RestoreBatch, prepare_owner_restore,
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
    native_world::{
        Aabb3, CompiledNativeCatalog, NativeActorRecord, NativeActorView, NativeTransform,
        NativeVariableValue, NativeWorldSnapshot,
    },
    persistence::SnapshotStore,
    simulation::{
        DomainId, Position, PreservedProspectState, SNAPSHOT_SCHEMA_LIFECYCLE, SimulationClock,
        WorldSnapshot,
    },
};

#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub struct NativeOwnerIncarnation(pub u64);

/// Exact authored identity, separate from foundation link names and GUIDs.
#[derive(Component, Clone, PartialEq)]
pub struct NativeOwnerName(pub Vec<u16>);

#[derive(Component, Clone, PartialEq)]
pub struct NativeOwnerRecorder(pub RecorderComponentRecord);

struct Definitions {
    // Shared by every activation pin, including clones of SnapshotMode. A
    // stale resource cannot roll this back by restoring its observed generation.
    current_generation: AtomicU64,
    catalog: Arc<CompiledNativeCatalog>,
    lifecycle: CompiledNativeLifecycle,
    relationships: Option<CompiledNativeRelationships>,
    identities: BTreeMap<DomainId, NativeRelationshipIdentity>,
}

/// Private fields prevent callers from updating a registry independently of its
/// actors. Cloning a context never grants authority to a different activation.
#[derive(Resource, Clone)]
pub struct NativeLifecycleContext {
    definitions: Arc<Definitions>,
    generation: u64,
    lifecycle: LifecycleSnapshot,
    live: BTreeMap<DomainId, Entity>,
}

impl NativeLifecycleContext {
    pub(crate) fn same_activation(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.definitions, &other.definitions)
            && other.generation == self.definitions.current_generation.load(Ordering::Relaxed)
    }
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
        !world.contains_resource::<NativeLifecycleContext>()
            && !world.contains_resource::<DormantNativeSlots>(),
        "world contains unexpected lifecycle resources"
    );
    for entity in world.iter_entities() {
        ensure!(
            !entity.contains::<NativeOwnerIncarnation>()
                && !entity.contains::<NativeOwnerName>()
                && !entity.contains::<NativeOwnerRecorder>()
                && !entity.contains::<NativeLifecycleContext>()
                && !entity.contains::<DormantNativeSlots>(),
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

struct StagedActor {
    record: NativeActorRecord,
    bounds: Aabb3,
    name: Vec<u16>,
    incarnation: u64,
    recorder: Option<RecorderComponentRecord>,
    relationship: Option<NativeRelationshipIdentity>,
    foundation: Option<NativeFoundation>,
}

struct StagedWorld {
    generation: u64,
    actors: Vec<StagedActor>,
    dormant: BTreeMap<DomainId, NativeActorRecord>,
    dependents: Option<NativeFoundationDependents>,
}

/// Every lookup and validation precedes entity mutation, including the reverse
/// index allocation. The commit only moves already prepared component values.
fn stage(snapshot: &WorldSnapshot, definitions: &Definitions) -> Result<StagedWorld> {
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
}

pub(crate) fn activate_lifecycle(world: &mut World, snapshot: &WorldSnapshot) -> Result<()> {
    snapshot.validate()?;
    ensure!(
        snapshot.schema_version == SNAPSHOT_SCHEMA_LIFECYCLE,
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
        catalog,
        lifecycle: compiled,
        relationships,
        identities,
    });
    let staged = stage(snapshot, &definitions)?;
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

pub(crate) fn capture_lifecycle(world: &World) -> Result<WorldSnapshot> {
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
    let expected_entities: BTreeSet<_> = context.live.values().copied().collect();
    for entity in world.iter_entities() {
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
            variables: &world
                .get::<NativeVariables>(*entity)
                .context("owner lacks variables")?
                .0,
        };
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
    Ok(WorldSnapshot {
        schema_version: SNAPSHOT_SCHEMA_LIFECYCLE,
        tick: world
            .get_resource::<SimulationClock>()
            .context("world lacks simulation clock")?
            .tick,
        entities: Vec::new(),
        native_world: Some(native),
        native_import: None,
        native_relationships: relationships,
        native_lifecycle: Some(context.lifecycle.clone()),
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
        world.insert_resource(dependents);
    }
    world.insert_resource(DormantNativeSlots {
        definitions: Arc::clone(&definitions),
        generation: staged.generation,
        records: staged.dormant,
    });
    definitions
        .current_generation
        .store(staged.generation, Ordering::Relaxed);
    world.insert_resource(NativeLifecycleContext {
        definitions,
        generation: staged.generation,
        lifecycle: snapshot.native_lifecycle.expect("prepared lifecycle state"),
        live,
    });
}

pub fn restore_native_owners(
    world: &mut World,
    batch: &RestoreBatch,
) -> Result<NativeRestoreResult> {
    let current = capture_lifecycle(world)?;
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
}

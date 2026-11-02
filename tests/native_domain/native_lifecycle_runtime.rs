//! Authored owners only. No source save, original runtime or network is used.
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use bevy::{
    ecs::entity_disabling::{DefaultQueryFilters, Disabled},
    prelude::*,
};
use icarus_bevy::{
    config::ServerConfig,
    native_lifecycle::{
        CompiledNativeLifecycle, LifecycleDefinition, OwnerHandle, Presence,
        RecorderComponentRecord, RecorderKind, RecorderSource, ResolvePolicy, RestoreBatch,
        RestoreRecorder, SlotDeclaration, SpawnComponent, prepare_lifecycle_world,
    },
    native_lifecycle_runtime::{
        DormantNativeSlots, NativeLifecycleContext, NativeOwnerIncarnation, NativeOwnerName,
        NativeOwnerRecorder, NativeRestoreResult, lifecycle_dependents_of, lifecycle_foundation_of,
        lifecycle_world_bounds, native_owner_handle, reassign_lifecycle_foundation,
        restore_native_owners, update_lifecycle_actor,
    },
    native_relationships::{
        CompiledNativeRelationships, NativeActorLinkV1, NativeRelationshipActorV1,
        NativeRelationshipRole, NativeRelationshipsDefinition, prepare_relationship_world,
    },
    native_runtime::{
        NativeActorIdentity, NativeFoundation, NativeFoundationDependents, NativeLocalBounds,
        NativeRelationshipIdentity, NativeRotation, NativeScale, NativeVariables,
        reassign_native_foundation, update_native_actor,
    },
    native_world::{
        Aabb3, CompiledNativeCatalog, NativeCatalogDefinition, NativeClassDefinition,
        NativeClassKind, NativeFrame, NativeMapDefinition, NativePlacement, NativeTileDefinition,
        NativeTransform, NativeValue, NativeVariableValue, VariableDefinition,
        prepare_authored_world,
    },
    persistence::{SaveWorker, SnapshotStore},
    simulation::{
        DomainId, Position, SimulationClock, SnapshotMode, WorldSnapshot, build_app, capture,
    },
};

const A: DomainId = DomainId(1);
const B: DomainId = DomainId(2);
const DEPLOYABLE: DomainId = DomainId(3);
const DORMANT: DomainId = DomainId(4);

fn fixture() -> WorldSnapshot {
    let keys = ["a", "b", "c", "d"];
    let catalog = Arc::new(
        CompiledNativeCatalog::compile(NativeCatalogDefinition {
            native_format: 1,
            frame: NativeFrame::RightHandedZUpMeters,
            classes: vec![NativeClassDefinition {
                key: "box".into(),
                kind: NativeClassKind::SpatialObjectV1,
                local_bounds: Aabb3 {
                    min: [-1.0; 3],
                    max: [1.0; 3],
                },
                variables: vec![VariableDefinition {
                    key: "count".into(),
                    default: NativeValue::I32(3),
                }],
            }],
            maps: vec![NativeMapDefinition {
                key: "map".into(),
                tiles: vec![NativeTileDefinition {
                    key: "tile".into(),
                    bounds: Aabb3 {
                        min: [-100.0; 3],
                        max: [100.0; 3],
                    },
                }],
                placements: keys
                    .iter()
                    .map(|key| NativePlacement {
                        key: (*key).into(),
                        class: "box".into(),
                        tile: "tile".into(),
                        transform: NativeTransform::default(),
                        initial_values: vec![],
                    })
                    .collect(),
            }],
        })
        .unwrap(),
    );
    let relationships = CompiledNativeRelationships::compile(
        NativeRelationshipsDefinition {
            relationship_format: 1,
            native_catalog_id: catalog.identity().into(),
            map: "map".into(),
            actors: keys
                .iter()
                .enumerate()
                .map(|(i, key)| NativeRelationshipActorV1 {
                    placement: (*key).into(),
                    identity: link(i),
                    role: match i {
                        0 | 1 => NativeRelationshipRole::Foundation,
                        2 => NativeRelationshipRole::Deployable,
                        _ => NativeRelationshipRole::Spatial,
                    },
                    foundation: (i == 2).then(|| link(0)),
                })
                .collect(),
        },
        &catalog,
    )
    .unwrap();
    let base = prepare_relationship_world(
        prepare_authored_world(Arc::clone(&catalog), "map")
            .unwrap()
            .initial_snapshot(),
        &relationships,
    )
    .unwrap();
    let lifecycle = CompiledNativeLifecycle::compile(
        LifecycleDefinition {
            lifecycle_format: 1,
            native_catalog_id: catalog.identity().into(),
            map: "map".into(),
            slots: keys
                .iter()
                .enumerate()
                .map(|(i, key)| SlotDeclaration {
                    placement: (*key).into(),
                    owner_name: format!("PrivateOwner{i}").encode_utf16().collect(),
                    recorder_kind: RecorderKind::SpatialStateV1,
                    initial_presence: if i == 3 {
                        Presence::Absent
                    } else {
                        Presence::Live
                    },
                    initial_component: (i != 3).then_some(RecorderComponentRecord {
                        kind: RecorderKind::SpatialStateV1,
                        guid: (i as i32 + 1) * 10,
                    }),
                    spawn_component: SpawnComponent::DeclaredKind,
                })
                .collect(),
        },
        &catalog,
    )
    .unwrap();
    prepare_lifecycle_world(base.snapshot().clone(), &lifecycle)
        .unwrap()
        .into_snapshot()
}

fn link(i: usize) -> NativeActorLinkV1 {
    NativeActorLinkV1 {
        name: format!("IndependentLink{i}").encode_utf16().collect(),
        guid: i as i32 + 100,
    }
}

fn app() -> App {
    build_app(&ServerConfig::default(), &fixture()).unwrap()
}

fn entity(world: &World, id: DomainId) -> Entity {
    world
        .iter_entities()
        .find(|entity| entity.get::<DomainId>() == Some(&id))
        .unwrap()
        .id()
}

fn command(snapshot: &WorldSnapshot, id: DomainId, policy: ResolvePolicy) -> RestoreRecorder {
    let native = snapshot.native_world.as_ref().unwrap();
    let lifecycle = snapshot.native_lifecycle.as_ref().unwrap();
    let actor = native.actors.iter().find(|actor| actor.id == id).unwrap();
    let slot = lifecycle.slots.iter().find(|slot| slot.id == id).unwrap();
    let declaration = lifecycle
        .definition
        .slots
        .iter()
        .find(|slot| slot.placement == actor.placement)
        .unwrap();
    RestoreRecorder {
        guid: slot
            .component
            .as_ref()
            .map_or(400, |component| component.guid),
        kind: RecorderKind::SpatialStateV1,
        target_placement: actor.placement.clone(),
        owner_name: declaration.owner_name.clone(),
        policy,
        source: RecorderSource::Detached {
            expected_prior_binding: slot.component.as_ref().map(|_| OwnerHandle {
                id,
                incarnation: slot.incarnation,
            }),
        },
        create_if_missing: true,
        expected_presence: slot.presence,
        expected_incarnation: slot.incarnation,
        transform: actor.transform.clone(),
        variables: actor.variables.clone(),
    }
}

fn batch(snapshot: &WorldSnapshot, recorders: Vec<RestoreRecorder>) -> RestoreBatch {
    let lifecycle = snapshot.native_lifecycle.as_ref().unwrap();
    RestoreBatch {
        lifecycle_id: lifecycle.lifecycle_id.clone(),
        sequence: lifecycle
            .completed
            .as_ref()
            .map_or(1, |completed| completed.sequence + 1),
        recorders,
    }
}

fn handle(world: &World, id: DomainId) -> OwnerHandle {
    native_owner_handle(world, id).unwrap()
}

#[test]
fn dormant_state_is_complete_but_has_no_entity_and_spawns_only_explicitly() {
    let mut initial = fixture();
    initial
        .native_world
        .as_mut()
        .unwrap()
        .actors
        .iter_mut()
        .find(|actor| actor.id == DORMANT)
        .unwrap()
        .transform
        .translation = [40.0, 0.0, 0.0];
    let mut app = build_app(&ServerConfig::default(), &initial).unwrap();
    assert_eq!(
        app.world()
            .iter_entities()
            .filter(|entity| entity.contains::<DomainId>())
            .count(),
        3
    );
    assert!(native_owner_handle(app.world(), DORMANT).is_err());
    assert_eq!(capture(app.world_mut()).unwrap(), initial);
    let mut restore = command(&initial, DORMANT, ResolvePolicy::FindOrRespawn);
    restore.transform.translation = [5.0, 6.0, 7.0];
    restore.transform.rotation = [0.0, 0.0, 1.0, 0.0];
    restore.transform.scale = [2.0, 3.0, 4.0];
    let request = batch(&initial, vec![restore]);
    assert!(matches!(
        restore_native_owners(app.world_mut(), &request).unwrap(),
        NativeRestoreResult::AppliedInMemory(_)
    ));
    assert_eq!(handle(app.world(), DORMANT).incarnation, 1);
    let snapshot = capture(app.world_mut()).unwrap();
    let actor = snapshot
        .native_world
        .as_ref()
        .unwrap()
        .actors
        .iter()
        .find(|actor| actor.id == DORMANT)
        .unwrap();
    assert_eq!(actor.transform.translation, [5.0, 6.0, 7.0]);
    assert_eq!(actor.transform.scale, [2.0, 3.0, 4.0]);
    assert_eq!(actor.transform.rotation, [0.0, 0.0, 1.0, 0.0]);
    assert!(lifecycle_world_bounds(app.world(), handle(app.world(), DORMANT)).is_ok());
}

#[test]
fn replacement_is_atomic_preserves_edited_edges_and_retries_without_replay() {
    let mut app = app();
    let owner = handle(app.world(), DEPLOYABLE);
    let target = handle(app.world(), B);
    reassign_lifecycle_foundation(app.world_mut(), owner, Some(target)).unwrap();
    let before = capture(app.world_mut()).unwrap();
    let old_target = entity(app.world(), B);
    let old_owner = entity(app.world(), DEPLOYABLE);
    let request = batch(
        &before,
        vec![
            command(&before, B, ResolvePolicy::RespawnOnly),
            command(&before, DEPLOYABLE, ResolvePolicy::RespawnOnly),
        ],
    );
    restore_native_owners(app.world_mut(), &request).unwrap();
    assert!(app.world().get_entity(old_target).is_err());
    assert!(app.world().get_entity(old_owner).is_err());
    assert_eq!(handle(app.world(), B).incarnation, target.incarnation + 1);
    assert_eq!(
        lifecycle_foundation_of(app.world(), handle(app.world(), DEPLOYABLE)).unwrap(),
        Some(handle(app.world(), B))
    );
    assert_eq!(
        lifecycle_dependents_of(app.world(), handle(app.world(), B)).unwrap(),
        vec![handle(app.world(), DEPLOYABLE)]
    );
    let after = capture(app.world_mut()).unwrap();
    let current_entity = entity(app.world(), B);
    assert!(matches!(
        restore_native_owners(app.world_mut(), &request).unwrap(),
        NativeRestoreResult::AlreadyCompleted(_)
    ));
    assert_eq!(entity(app.world(), B), current_entity);
    assert_eq!(capture(app.world_mut()).unwrap(), after);
    assert!(
        update_lifecycle_actor(
            app.world_mut(),
            target,
            NativeTransform::default(),
            values(5)
        )
        .is_err()
    );
    assert!(reassign_lifecycle_foundation(app.world_mut(), owner, None).is_err());
    let new_owner = handle(app.world(), DEPLOYABLE);
    assert!(reassign_lifecycle_foundation(app.world_mut(), new_owner, Some(target)).is_err());
    assert_eq!(capture(app.world_mut()).unwrap(), after);
}

fn values(count: i32) -> Vec<NativeVariableValue> {
    vec![NativeVariableValue {
        key: "count".into(),
        value: NativeValue::I32(count),
    }]
}

#[test]
fn find_preserves_entity_and_bound_respawn_is_a_checked_keep() {
    let mut app = app();
    let before = capture(app.world_mut()).unwrap();
    let original_entity = entity(app.world(), A);
    let original_handle = handle(app.world(), A);
    let mut found = command(&before, A, ResolvePolicy::FindOnly);
    found.guid = 900;
    found.source = RecorderSource::Detached {
        expected_prior_binding: None,
    };
    found.variables = values(9);
    restore_native_owners(app.world_mut(), &batch(&before, vec![found])).unwrap();
    assert_eq!(entity(app.world(), A), original_entity);
    assert_eq!(handle(app.world(), A), original_handle);
    let after = capture(app.world_mut()).unwrap();
    assert!(
        !after
            .native_lifecycle
            .as_ref()
            .unwrap()
            .bindings
            .iter()
            .any(|binding| binding.guid == 10)
    );
    let mut keep = command(&after, A, ResolvePolicy::RespawnOnly);
    keep.source = RecorderSource::Bound;
    keep.owner_name = vec![0xd800]; // Bound matching uses recorder GUID, not this name.
    let mut invalid = keep.clone();
    invalid.variables = values(11);
    assert!(restore_native_owners(app.world_mut(), &batch(&after, vec![invalid])).is_err());
    assert_eq!(capture(app.world_mut()).unwrap(), after);
    restore_native_owners(app.world_mut(), &batch(&after, vec![keep])).unwrap();
    assert_eq!(entity(app.world(), A), original_entity);
    assert_eq!(handle(app.world(), A), original_handle);
}

#[test]
fn failing_last_command_and_sequence_conflict_leave_everything_unchanged() {
    let mut app = app();
    let before = capture(app.world_mut()).unwrap();
    let old = entity(app.world(), A);
    let good = command(&before, A, ResolvePolicy::RespawnOnly);
    let mut bad = command(&before, DORMANT, ResolvePolicy::FindOnly);
    bad.variables = values(15);
    assert!(
        restore_native_owners(app.world_mut(), &batch(&before, vec![good.clone(), bad])).is_err()
    );
    assert_eq!(capture(app.world_mut()).unwrap(), before);
    assert_eq!(entity(app.world(), A), old);
    let mut request = batch(&before, vec![good]);
    restore_native_owners(app.world_mut(), &request).unwrap();
    let after = capture(app.world_mut()).unwrap();
    request.recorders[0].variables = values(99);
    assert!(restore_native_owners(app.world_mut(), &request).is_err());
    assert_eq!(capture(app.world_mut()).unwrap(), after);
}

#[test]
fn capture_worker_reload_keeps_completion_and_mutable_state() {
    let mut app = app();
    let before = capture(app.world_mut()).unwrap();
    let request = batch(
        &before,
        vec![command(&before, A, ResolvePolicy::RespawnOnly)],
    );
    restore_native_owners(app.world_mut(), &request).unwrap();
    let current = handle(app.world(), A);
    let pose = NativeTransform {
        translation: [7.0, 8.0, 9.0],
        rotation: [0.0, 1.0, 0.0, 0.0],
        scale: [2.0, 1.0, 3.0],
    };
    update_lifecycle_actor(app.world_mut(), current, pose, values(42)).unwrap();
    app.world_mut().resource_mut::<SimulationClock>().tick = 91;
    let expected = capture(app.world_mut()).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let mut worker = SaveWorker::spawn(SnapshotStore::open(directory.path()).unwrap()).unwrap();
    worker.submit(expected.clone()).unwrap();
    worker
        .finish_before(Instant::now() + Duration::from_secs(10))
        .unwrap()
        .unwrap();
    drop(worker);
    let deadline = Instant::now() + Duration::from_secs(10);
    let store = loop {
        match SnapshotStore::open(directory.path()) {
            Ok(store) => break store,
            Err(error)
                if Instant::now() < deadline && format!("{error:#}").contains("already locked") =>
            {
                std::thread::yield_now()
            }
            Err(error) => panic!("worker store did not close: {error:#}"),
        }
    };
    let restored = store.load().unwrap().unwrap().snapshot;
    let mut reloaded = build_app(&ServerConfig::default(), &restored).unwrap();
    assert_eq!(capture(reloaded.world_mut()).unwrap(), expected);
    assert!(matches!(
        restore_native_owners(reloaded.world_mut(), &request).unwrap(),
        NativeRestoreResult::AlreadyCompleted(_)
    ));
    assert_eq!(capture(reloaded.world_mut()).unwrap(), expected);
}

fn reject_operations(world: &mut World, request: &RestoreBatch) {
    assert!(capture(world).is_err());
    assert!(native_owner_handle(world, A).is_err());
    assert!(restore_native_owners(world, request).is_err());
    assert!(
        update_lifecycle_actor(
            world,
            OwnerHandle {
                id: A,
                incarnation: 1
            },
            NativeTransform::default(),
            values(3)
        )
        .is_err()
    );
    assert!(
        reassign_lifecycle_foundation(
            world,
            OwnerHandle {
                id: DEPLOYABLE,
                incarnation: 1
            },
            None
        )
        .is_err()
    );
}

#[test]
fn missing_components_deletion_and_hidden_extras_are_corruption() {
    type Damage = Box<dyn Fn(&mut World, Entity)>;
    let damages: Vec<Damage> = vec![
        Box::new(|world, e| {
            world.entity_mut(e).remove::<DomainId>();
        }),
        Box::new(|world, e| {
            world.entity_mut(e).remove::<Position>();
        }),
        Box::new(|world, e| {
            world.entity_mut(e).remove::<NativeActorIdentity>();
        }),
        Box::new(|world, e| {
            world.entity_mut(e).remove::<NativeRotation>();
        }),
        Box::new(|world, e| {
            world.entity_mut(e).remove::<NativeScale>();
        }),
        Box::new(|world, e| {
            world.entity_mut(e).remove::<NativeVariables>();
        }),
        Box::new(|world, e| {
            world.entity_mut(e).remove::<NativeLocalBounds>();
        }),
        Box::new(|world, e| {
            world.entity_mut(e).remove::<NativeOwnerName>();
        }),
        Box::new(|world, e| {
            world.entity_mut(e).remove::<NativeOwnerIncarnation>();
        }),
        Box::new(|world, e| {
            world.entity_mut(e).remove::<NativeOwnerRecorder>();
        }),
        Box::new(|world, e| {
            world.entity_mut(e).remove::<NativeRelationshipIdentity>();
        }),
        Box::new(|world, e| {
            world.despawn(e);
        }),
        Box::new(|world, _| {
            world.remove_resource::<NativeLifecycleContext>();
        }),
        Box::new(|world, _| {
            world.remove_resource::<DormantNativeSlots>();
        }),
        Box::new(|world, _| {
            world.remove_resource::<NativeFoundationDependents>();
        }),
        Box::new(|world, _| {
            world.remove_resource::<SnapshotMode>();
        }),
        Box::new(|world, _| {
            world.remove_resource::<SimulationClock>();
        }),
        Box::new(|world, _| {
            world.spawn((NativeOwnerIncarnation(1), Disabled));
        }),
        Box::new(|world, _| {
            world.spawn((NativeFoundation(None), Disabled));
        }),
    ];
    for damage in damages {
        let mut app = app();
        let before = capture(app.world_mut()).unwrap();
        let request = batch(
            &before,
            vec![command(&before, B, ResolvePolicy::RespawnOnly)],
        );
        let e = entity(app.world(), A);
        damage(app.world_mut(), e);
        reject_operations(app.world_mut(), &request);
        assert_eq!(
            app.world()
                .get::<NativeOwnerIncarnation>(entity(app.world(), B)),
            Some(&NativeOwnerIncarnation(1))
        );
    }
}

#[derive(Component)]
struct Hidden;

#[test]
fn default_filters_cannot_hide_damaged_members_or_unregistered_components() {
    let mut app = app();
    let component_id = app.world_mut().register_component::<Hidden>();
    app.world_mut()
        .resource_mut::<DefaultQueryFilters>()
        .register_disabling_component(component_id);
    let a = entity(app.world(), A);
    app.world_mut().entity_mut(a).insert((Disabled, Hidden));
    assert!(capture(app.world_mut()).is_ok());
    app.world_mut().spawn((NativeOwnerName(vec![1]), Hidden));
    assert!(capture(app.world_mut()).is_err());
}

#[test]
fn stale_same_activation_resources_cannot_replay_a_completed_find() {
    for stale in 0..4 {
        let mut app = app();
        let initial = capture(app.world_mut()).unwrap();
        let old_context = app.world().resource::<NativeLifecycleContext>().clone();
        let old_dormant = app.world().resource::<DormantNativeSlots>().clone();
        let old_mode = app
            .world()
            .resource::<icarus_bevy::simulation::SnapshotMode>()
            .clone();
        let request = batch(
            &initial,
            vec![command(&initial, A, ResolvePolicy::FindOnly)],
        );
        restore_native_owners(app.world_mut(), &request).unwrap();
        let owner = handle(app.world(), A);
        let mut pose = initial.native_world.as_ref().unwrap().actors[0]
            .transform
            .clone();
        pose.translation[0] += 1.0;
        let variables = initial.native_world.as_ref().unwrap().actors[0]
            .variables
            .clone();
        update_lifecycle_actor(app.world_mut(), owner, pose, variables).unwrap();
        let edited = capture(app.world_mut()).unwrap();
        let current_context = app.world().resource::<NativeLifecycleContext>().clone();
        let current_dormant = app.world().resource::<DormantNativeSlots>().clone();
        let current_mode = app
            .world()
            .resource::<icarus_bevy::simulation::SnapshotMode>()
            .clone();
        if stale != 1 {
            app.world_mut().insert_resource(old_context);
        }
        if stale != 0 {
            app.world_mut().insert_resource(old_dormant);
        }
        if stale == 3 {
            app.world_mut().insert_resource(old_mode);
        }
        assert!(capture(app.world_mut()).is_err());
        assert!(restore_native_owners(app.world_mut(), &request).is_err());
        app.world_mut().insert_resource(current_context);
        app.world_mut().insert_resource(current_dormant);
        app.world_mut().insert_resource(current_mode);
        assert_eq!(capture(app.world_mut()).unwrap(), edited);
        assert!(matches!(
            restore_native_owners(app.world_mut(), &request).unwrap(),
            NativeRestoreResult::AlreadyCompleted(_)
        ));
        assert_eq!(capture(app.world_mut()).unwrap(), edited);
    }
}

#[test]
fn generation_backups_survive_failure_and_retry_but_not_relationship_edits() {
    let mut app = app();
    let before = capture(app.world_mut()).unwrap();
    let request = batch(&before, vec![command(&before, A, ResolvePolicy::FindOnly)]);
    restore_native_owners(app.world_mut(), &request).unwrap();
    let context = app.world().resource::<NativeLifecycleContext>().clone();
    let dormant = app.world().resource::<DormantNativeSlots>().clone();
    let saved = capture(app.world_mut()).unwrap();
    let owner = handle(app.world(), A);
    let mut invalid = saved.native_world.as_ref().unwrap().actors[0]
        .transform
        .clone();
    invalid.scale[0] = 0.0;
    assert!(update_lifecycle_actor(app.world_mut(), owner, invalid, values(3)).is_err());
    assert!(matches!(
        restore_native_owners(app.world_mut(), &request).unwrap(),
        NativeRestoreResult::AlreadyCompleted(_)
    ));
    app.world_mut().insert_resource(context.clone());
    app.world_mut().insert_resource(dormant.clone());
    assert_eq!(capture(app.world_mut()).unwrap(), saved);
    let deployable = handle(app.world(), DEPLOYABLE);
    reassign_lifecycle_foundation(app.world_mut(), deployable, None).unwrap();
    let edited = capture(app.world_mut()).unwrap();
    let latest_context = app.world().resource::<NativeLifecycleContext>().clone();
    let latest_dormant = app.world().resource::<DormantNativeSlots>().clone();
    app.world_mut().insert_resource(context);
    app.world_mut().insert_resource(dormant);
    assert!(capture(app.world_mut()).is_err());
    app.world_mut().insert_resource(latest_context);
    app.world_mut().insert_resource(latest_dormant);
    assert_eq!(capture(app.world_mut()).unwrap(), edited);
}

#[test]
fn context_swaps_fail_even_with_identical_definitions_and_entity_numbers() {
    let mut first = app();
    let second = app();
    let before = capture(first.world_mut()).unwrap();
    let request = batch(&before, vec![command(&before, A, ResolvePolicy::FindOnly)]);
    let original = first.world().resource::<NativeLifecycleContext>().clone();
    first
        .world_mut()
        .insert_resource(second.world().resource::<NativeLifecycleContext>().clone());
    reject_operations(first.world_mut(), &request);
    first.world_mut().insert_resource(original);
    assert_eq!(capture(first.world_mut()).unwrap(), before);
    first
        .world_mut()
        .insert_resource(second.world().resource::<DormantNativeSlots>().clone());
    reject_operations(first.world_mut(), &request);
}

#[test]
fn reverse_index_damage_and_stale_identity_are_repairable_without_partial_restore() {
    let mut app = app();
    let before = capture(app.world_mut()).unwrap();
    let request = batch(
        &before,
        vec![command(&before, A, ResolvePolicy::RespawnOnly)],
    );
    let reverse = app.world().resource::<NativeFoundationDependents>().clone();
    app.world_mut()
        .resource_mut::<NativeFoundationDependents>()
        .0
        .clear();
    reject_operations(app.world_mut(), &request);
    app.world_mut().insert_resource(reverse);
    assert_eq!(capture(app.world_mut()).unwrap(), before);
    restore_native_owners(app.world_mut(), &request).unwrap();
    let a = entity(app.world(), A);
    app.world_mut()
        .entity_mut(a)
        .insert(NativeOwnerIncarnation(1));
    assert!(restore_native_owners(app.world_mut(), &request).is_err()); // retry must validate too
    app.world_mut()
        .entity_mut(a)
        .insert(NativeOwnerIncarnation(2));
    assert!(matches!(
        restore_native_owners(app.world_mut(), &request).unwrap(),
        NativeRestoreResult::AlreadyCompleted(_)
    ));
}

#[test]
fn legacy_id_only_edits_cannot_bypass_incarnation_checks() {
    let mut app = app();
    let before = capture(app.world_mut()).unwrap();
    assert!(
        update_native_actor(app.world_mut(), A, NativeTransform::default(), values(0)).is_err()
    );
    assert!(reassign_native_foundation(app.world_mut(), DEPLOYABLE, None).is_err());
    assert_eq!(capture(app.world_mut()).unwrap(), before);
    let mut legacy = build_app(&ServerConfig::default(), &WorldSnapshot::default()).unwrap();
    legacy.world_mut().spawn(NativeOwnerName(vec![1]));
    assert!(capture(legacy.world_mut()).is_err());
    let mut fixed = fixture();
    fixed.schema_version = 6;
    fixed.native_lifecycle = None;
    let mut fixed = build_app(&ServerConfig::default(), &fixed).unwrap();
    fixed
        .world_mut()
        .spawn((NativeOwnerIncarnation(1), Disabled));
    assert!(capture(fixed.world_mut()).is_err());
}

#[test]
fn invalid_variable_state_rejects_capture_and_unrelated_restore_until_repaired() {
    let mut app = app();
    let before = capture(app.world_mut()).unwrap();
    let request = batch(
        &before,
        vec![command(&before, B, ResolvePolicy::RespawnOnly)],
    );
    let target = entity(app.world(), A);
    let original = app.world().get::<NativeVariables>(target).unwrap().clone();
    app.world_mut()
        .entity_mut(target)
        .insert(NativeVariables(vec![NativeVariableValue {
            key: "count".into(),
            value: NativeValue::String("authored".repeat(1024)),
        }]));
    reject_operations(app.world_mut(), &request);
    app.world_mut().entity_mut(target).insert(original);
    assert_eq!(capture(app.world_mut()).unwrap(), before);
}

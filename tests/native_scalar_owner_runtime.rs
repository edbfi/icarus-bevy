//! Independently authored scalar consumers; no original inputs or runtime.
#[path = "common/c001_scalar_owners.rs"]
mod fixture;

use bevy::{ecs::entity_disabling::Disabled, prelude::*};
use icarus_bevy::{
    config::ServerConfig,
    native_lifecycle::{
        OwnerHandle, RecorderKind, RecorderSource, ResolvePolicy, RestoreBatch, RestoreRecorder,
    },
    native_lifecycle_runtime::{
        C001ScalarOwnerImportState, DormantNativeSlots, NativeLifecycleContext,
        NativeOwnerRecorder, NativeRestoreResult, NativeScalarOwnerContractsState,
        lifecycle_foundation_of, native_owner_handle, reassign_lifecycle_foundation,
        restore_native_owners, scalar_owner_state, set_enabled, try_use, update_lifecycle_actor,
    },
    native_runtime::{NativeRotation, NativeVariables, update_native_actor},
    native_scalar_owners::ScalarUseOutcome,
    persistence::{SaveWorker, SnapshotStore},
    simulation::{
        DomainId, Position, PreservedProspectState, SimulationClock, WorldSnapshot, build_app,
        capture,
    },
};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

const SPAWN: DomainId = DomainId(1);
const REPLACE: DomainId = DomainId(2);
const FIND: DomainId = DomainId(3);

fn entity(world: &World, id: DomainId) -> Entity {
    world
        .iter_entities()
        .find(|entity| entity.get::<DomainId>() == Some(&id))
        .unwrap()
        .id()
}

fn replacement(snapshot: &WorldSnapshot, id: DomainId, enabled: bool, count: i32) -> RestoreBatch {
    let life = snapshot.native_lifecycle.as_ref().unwrap();
    let actor = snapshot
        .native_world
        .as_ref()
        .unwrap()
        .actors
        .iter()
        .find(|a| a.id == id)
        .unwrap();
    let slot = life.slots.iter().find(|slot| slot.id == id).unwrap();
    let declaration = life
        .definition
        .slots
        .iter()
        .find(|d| d.placement == actor.placement)
        .unwrap();
    RestoreBatch {
        lifecycle_id: life.lifecycle_id.clone(),
        sequence: life.restore_sequence + 1,
        recorders: vec![RestoreRecorder {
            guid: 900,
            kind: RecorderKind::SpatialStateV1,
            target_placement: actor.placement.clone(),
            owner_name: declaration.owner_name.clone(),
            policy: ResolvePolicy::RespawnOnly,
            source: RecorderSource::Detached {
                expected_prior_binding: None,
            },
            create_if_missing: true,
            expected_presence: slot.presence,
            expected_incarnation: slot.incarnation,
            transform: actor.transform.clone(),
            variables: fixture::scalar_pair(enabled, count),
        }],
    }
}

#[test]
fn all_initial_paths_apply_saved_pair_and_disabled_exhausted_noops_do_not_mutate() {
    let initial = fixture::imported();
    let mut app = build_app(&ServerConfig::default(), &initial).unwrap();
    for (id, incarnation, enabled, count) in [
        (SPAWN, 1, false, 2),
        (REPLACE, 2, true, 2),
        (FIND, 1, true, 1),
    ] {
        let owner = native_owner_handle(app.world(), id).unwrap();
        assert_eq!(owner.incarnation, incarnation);
        let state = scalar_owner_state(app.world(), owner).unwrap();
        assert_eq!(
            (state.enabled, state.remaining_uses, state.available),
            (enabled, count, enabled)
        );
    }
    let spawn = native_owner_handle(app.world(), SPAWN).unwrap();
    let context = app.world().resource::<NativeLifecycleContext>().clone();
    assert!(matches!(
        try_use(app.world_mut(), spawn).unwrap(),
        ScalarUseOutcome::Disabled(_)
    ));
    set_enabled(app.world_mut(), spawn, false).unwrap();
    assert_eq!(capture(app.world_mut()).unwrap(), initial);
    // A validated no-op does not advance the private generation: the original
    // context remains current and usable without any repair.
    app.world_mut().insert_resource(context);
    assert_eq!(capture(app.world_mut()).unwrap(), initial);
    assert_eq!(
        set_enabled(app.world_mut(), spawn, true)
            .unwrap()
            .remaining_uses,
        2
    );
    assert!(
        matches!(try_use(app.world_mut(), spawn).unwrap(), ScalarUseOutcome::Used(state) if state.remaining_uses == 1 && state.available)
    );
    let find = native_owner_handle(app.world(), FIND).unwrap();
    assert!(
        matches!(try_use(app.world_mut(), find).unwrap(), ScalarUseOutcome::Used(state) if state.remaining_uses == 0 && !state.available)
    );
    let exhausted = capture(app.world_mut()).unwrap();
    assert!(matches!(
        try_use(app.world_mut(), find).unwrap(),
        ScalarUseOutcome::Exhausted(_)
    ));
    assert_eq!(capture(app.world_mut()).unwrap(), exhausted);
}

#[test]
fn initial_retry_preserves_consumption_pose_links_and_autosave_reload() {
    let fixture = fixture::fixture();
    let initial = fixture.snapshot();
    let mut app = build_app(&ServerConfig::default(), &initial).unwrap();
    let spawn = native_owner_handle(app.world(), SPAWN).unwrap();
    let find = native_owner_handle(app.world(), FIND).unwrap();
    set_enabled(app.world_mut(), spawn, true).unwrap();
    try_use(app.world_mut(), spawn).unwrap();
    set_enabled(app.world_mut(), spawn, false).unwrap();
    set_enabled(app.world_mut(), spawn, true).unwrap();
    try_use(app.world_mut(), find).unwrap();
    let mut pose = initial
        .native_world
        .as_ref()
        .unwrap()
        .actors
        .iter()
        .find(|a| a.id == FIND)
        .unwrap()
        .transform
        .clone();
    pose.translation = [10.0, 11.0, 12.0];
    update_lifecycle_actor(app.world_mut(), find, pose, fixture::scalar_pair(true, 0)).unwrap();
    reassign_lifecycle_foundation(app.world_mut(), find, Some(spawn)).unwrap();
    app.world_mut().resource_mut::<SimulationClock>().tick = 50;
    let edited = capture(app.world_mut()).unwrap();
    assert!(matches!(
        restore_native_owners(app.world_mut(), &fixture.initial_batch()).unwrap(),
        NativeRestoreResult::AlreadyCompleted(_)
    ));
    assert_eq!(capture(app.world_mut()).unwrap(), edited);
    let directory = tempfile::tempdir().unwrap();
    let mut worker = SaveWorker::spawn(SnapshotStore::open(directory.path()).unwrap()).unwrap();
    worker.submit(edited.clone()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    assert!(worker.finish_before(deadline).unwrap().is_some());
    drop(worker);
    let store = loop {
        match SnapshotStore::open(directory.path()) {
            Ok(store) => break store,
            Err(error)
                if Instant::now() < deadline && format!("{error:#}").contains("already locked") =>
            {
                std::thread::sleep(Duration::from_millis(2))
            }
            Err(error) => panic!("worker store did not close: {error:#}"),
        }
    };
    let loaded = store.load().unwrap().unwrap().snapshot;
    assert_eq!(loaded, edited);
    let mut restored = build_app(&ServerConfig::default(), &loaded).unwrap();
    assert_eq!(
        scalar_owner_state(restored.world(), spawn)
            .unwrap()
            .remaining_uses,
        1
    );
    assert_eq!(
        lifecycle_foundation_of(restored.world(), find).unwrap(),
        Some(spawn)
    );
    assert!(matches!(
        restore_native_owners(restored.world_mut(), &fixture.initial_batch()).unwrap(),
        NativeRestoreResult::AlreadyCompleted(_)
    ));
    assert_eq!(capture(restored.world_mut()).unwrap(), edited);
}

#[test]
fn replacement_installs_different_pair_invalidates_handle_and_retry_preserves_use() {
    let initial = fixture::imported();
    let mut app = build_app(&ServerConfig::default(), &initial).unwrap();
    let old = native_owner_handle(app.world(), FIND).unwrap();
    let old_entity = entity(app.world(), FIND);
    let batch = replacement(&initial, FIND, false, 4);
    restore_native_owners(app.world_mut(), &batch).unwrap();
    let new = native_owner_handle(app.world(), FIND).unwrap();
    assert_eq!(new.incarnation, old.incarnation + 1);
    assert_ne!(entity(app.world(), FIND), old_entity);
    assert!(scalar_owner_state(app.world(), old).is_err());
    assert!(try_use(app.world_mut(), old).is_err());
    assert!(set_enabled(app.world_mut(), old, true).is_err());
    let installed = scalar_owner_state(app.world(), new).unwrap();
    assert_eq!(
        (
            installed.enabled,
            installed.remaining_uses,
            installed.available
        ),
        (false, 4, false)
    );
    set_enabled(app.world_mut(), new, true).unwrap();
    try_use(app.world_mut(), new).unwrap();
    let edited = capture(app.world_mut()).unwrap();
    assert!(matches!(
        restore_native_owners(app.world_mut(), &batch).unwrap(),
        NativeRestoreResult::AlreadyCompleted(_)
    ));
    assert_eq!(capture(app.world_mut()).unwrap(), edited);
    let directory = tempfile::tempdir().unwrap();
    let mut store = SnapshotStore::open(directory.path()).unwrap();
    store.save_new(&edited).unwrap();
    let loaded = store.load().unwrap().unwrap().snapshot;
    let mut reloaded = build_app(&ServerConfig::default(), &loaded).unwrap();
    assert!(matches!(
        restore_native_owners(reloaded.world_mut(), &batch).unwrap(),
        NativeRestoreResult::AlreadyCompleted(_)
    ));
    assert_eq!(
        scalar_owner_state(reloaded.world(), new)
            .unwrap()
            .remaining_uses,
        3
    );
    assert_eq!(capture(reloaded.world_mut()).unwrap(), edited);
}

#[test]
fn generic_edits_cannot_change_scalars_but_pose_edits_accept_current_pair() {
    let initial = fixture::imported();
    let mut app = build_app(&ServerConfig::default(), &initial).unwrap();
    let owner = native_owner_handle(app.world(), REPLACE).unwrap();
    let actor = &initial.native_world.as_ref().unwrap().actors[1];
    let mut pose = actor.transform.clone();
    pose.translation[0] = 15.0;
    assert!(
        update_lifecycle_actor(
            app.world_mut(),
            owner,
            pose.clone(),
            fixture::scalar_pair(false, 2)
        )
        .is_err()
    );
    assert!(
        update_lifecycle_actor(
            app.world_mut(),
            owner,
            pose.clone(),
            fixture::scalar_pair(true, 1)
        )
        .is_err()
    );
    assert!(
        update_native_actor(
            app.world_mut(),
            owner.id,
            pose.clone(),
            actor.variables.clone()
        )
        .is_err()
    );
    assert_eq!(capture(app.world_mut()).unwrap(), initial);
    update_lifecycle_actor(
        app.world_mut(),
        owner,
        pose.clone(),
        actor.variables.clone(),
    )
    .unwrap();
    assert_eq!(
        app.world()
            .get::<Position>(entity(app.world(), REPLACE))
            .unwrap()
            .0,
        pose.translation
    );
}

fn assert_all_calls_reject(world: &mut World, owner: OwnerHandle, batch: &RestoreBatch) {
    assert!(capture(world).is_err());
    assert!(scalar_owner_state(world, owner).is_err());
    assert!(try_use(world, owner).is_err());
    assert!(set_enabled(world, owner, true).is_err());
    assert!(set_enabled(world, owner, false).is_err());
    assert!(restore_native_owners(world, batch).is_err());
}

#[test]
fn direct_write_with_same_availability_and_hidden_missing_components_reject_every_call() {
    let fixture = fixture::fixture();
    let initial = fixture.snapshot();
    for mode in 0..5 {
        let mut app = build_app(&ServerConfig::default(), &initial).unwrap();
        let owner = native_owner_handle(app.world(), SPAWN).unwrap();
        let victim = entity(app.world(), REPLACE);
        let position = app.world().get::<Position>(victim).unwrap().0;
        app.world_mut().entity_mut(victim).insert(Disabled);
        match mode {
            0 => {
                app.world_mut()
                    .entity_mut(victim)
                    .insert(NativeVariables(fixture::scalar_pair(true, 1)));
            }
            1 => {
                app.world_mut()
                    .entity_mut(victim)
                    .remove::<NativeVariables>();
            }
            2 => {
                app.world_mut()
                    .entity_mut(victim)
                    .remove::<NativeRotation>();
            }
            3 => {
                app.world_mut()
                    .entity_mut(victim)
                    .remove::<NativeOwnerRecorder>();
            }
            _ => {
                app.world_mut()
                    .spawn((Disabled, NativeVariables(fixture::scalar_pair(true, 2))));
            }
        }
        assert_all_calls_reject(app.world_mut(), owner, &fixture.initial_batch());
        assert_eq!(app.world().get::<Position>(victim).unwrap().0, position);
    }
}

#[test]
fn stale_and_cross_activation_contexts_and_tampered_pins_reject() {
    let fixture = fixture::fixture();
    let initial = fixture.snapshot();
    let other = fixture::imported_variant(1);
    for mode in 0..8 {
        let mut app = build_app(&ServerConfig::default(), &initial).unwrap();
        let owner = native_owner_handle(app.world(), SPAWN).unwrap();
        let old_context = app.world().resource::<NativeLifecycleContext>().clone();
        let old_dormant = app.world().resource::<DormantNativeSlots>().clone();
        match mode {
            0 => {
                app.world_mut()
                    .remove_resource::<C001ScalarOwnerImportState>();
            }
            1 => {
                app.world_mut()
                    .insert_resource(C001ScalarOwnerImportState(Arc::new(
                        other.native_scalar_owner_import.clone().unwrap(),
                    )));
            }
            2 => {
                app.world_mut()
                    .remove_resource::<NativeScalarOwnerContractsState>();
            }
            3 => {
                let mut contracts = initial.native_scalar_contracts.clone().unwrap();
                contracts.contract_id = "0".repeat(64);
                app.world_mut()
                    .insert_resource(NativeScalarOwnerContractsState(Arc::new(contracts)));
            }
            4 => {
                app.world_mut().remove_resource::<PreservedProspectState>();
            }
            5 => {
                app.world_mut().insert_resource(PreservedProspectState(
                    other.preserved_prospect.clone().unwrap(),
                ));
            }
            6 => {
                set_enabled(app.world_mut(), owner, true).unwrap();
                app.world_mut().insert_resource(old_context);
                app.world_mut().insert_resource(old_dormant);
            }
            _ => {
                let other_app = build_app(&ServerConfig::default(), &initial).unwrap();
                app.world_mut().insert_resource(
                    other_app
                        .world()
                        .resource::<NativeLifecycleContext>()
                        .clone(),
                );
                app.world_mut()
                    .insert_resource(other_app.world().resource::<DormantNativeSlots>().clone());
            }
        }
        assert_all_calls_reject(app.world_mut(), owner, &fixture.initial_batch());
    }
}

#[test]
fn earlier_modes_reject_scalar_resources_and_failed_restore_preserves_live_state() {
    let fixture = fixture::fixture();
    let scalar = fixture.snapshot();
    for initial in [WorldSnapshot::default(), fixture.base.clone()] {
        for resource in 0..2 {
            let mut app = build_app(&ServerConfig::default(), &initial).unwrap();
            if resource == 0 {
                app.world_mut()
                    .insert_resource(C001ScalarOwnerImportState(Arc::new(
                        scalar.native_scalar_owner_import.clone().unwrap(),
                    )));
            } else {
                app.world_mut()
                    .insert_resource(NativeScalarOwnerContractsState(Arc::new(
                        scalar.native_scalar_contracts.clone().unwrap(),
                    )));
            }
            assert!(capture(app.world_mut()).is_err());
        }
    }
    let mut app = build_app(&ServerConfig::default(), &scalar).unwrap();
    for count in [-1, 10] {
        let batch = replacement(&scalar, FIND, true, count);
        assert!(restore_native_owners(app.world_mut(), &batch).is_err());
        assert_eq!(capture(app.world_mut()).unwrap(), scalar);
    }
    let mut missing = replacement(&scalar, FIND, true, 2);
    missing.recorders[0].variables.pop();
    assert!(restore_native_owners(app.world_mut(), &missing).is_err());
    assert_eq!(capture(app.world_mut()).unwrap(), scalar);
}

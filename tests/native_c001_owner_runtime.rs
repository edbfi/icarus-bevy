//! Authored source fixtures only; no original files or runtime.
#[path = "common/c001_owners.rs"]
mod fixture;

use bevy::{ecs::entity_disabling::Disabled, prelude::*};
use icarus_bevy::{
    config::ServerConfig,
    native_lifecycle::{
        RecorderKind, RecorderSource, ResolvePolicy, RestoreBatch, RestoreRecorder,
    },
    native_lifecycle_runtime::{
        C001OwnerImportState, DormantNativeSlots, NativeLifecycleContext, NativeRestoreResult,
        lifecycle_foundation_of, native_owner_handle, reassign_lifecycle_foundation,
        restore_native_owners, update_lifecycle_actor,
    },
    native_runtime::NativeRotation,
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

fn next_request(
    snapshot: &WorldSnapshot,
    id: DomainId,
    guid: i32,
    policy: ResolvePolicy,
) -> RestoreBatch {
    let life = snapshot.native_lifecycle.as_ref().unwrap();
    let native = snapshot.native_world.as_ref().unwrap();
    let actor = native.actors.iter().find(|actor| actor.id == id).unwrap();
    let slot = life.slots.iter().find(|slot| slot.id == id).unwrap();
    let declaration = life
        .definition
        .slots
        .iter()
        .find(|decl| decl.placement == actor.placement)
        .unwrap();
    RestoreBatch {
        lifecycle_id: life.lifecycle_id.clone(),
        sequence: life.restore_sequence + 1,
        recorders: vec![RestoreRecorder {
            guid,
            kind: RecorderKind::SpatialStateV1,
            target_placement: actor.placement.clone(),
            owner_name: declaration.owner_name.clone(),
            policy,
            source: RecorderSource::Detached {
                expected_prior_binding: None,
            },
            create_if_missing: true,
            expected_presence: slot.presence,
            expected_incarnation: slot.incarnation,
            transform: actor.transform.clone(),
            variables: vec![],
        }],
    }
}

#[test]
fn source_policies_materialize_complete_owners_and_retry_never_resets_edits() {
    let fixture = fixture::fixture();
    let initial = fixture.snapshot();
    let mut app = build_app(&ServerConfig::default(), &initial).unwrap();
    assert_eq!(capture(app.world_mut()).unwrap(), initial);
    for (id, incarnation) in [(SPAWN, 1), (REPLACE, 2), (FIND, 1)] {
        assert_eq!(
            native_owner_handle(app.world(), id).unwrap().incarnation,
            incarnation
        );
    }
    let owner = native_owner_handle(app.world(), FIND).unwrap();
    let target = native_owner_handle(app.world(), SPAWN).unwrap();
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
    update_lifecycle_actor(app.world_mut(), owner, pose, vec![]).unwrap();
    reassign_lifecycle_foundation(app.world_mut(), owner, Some(target)).unwrap();
    app.world_mut().resource_mut::<SimulationClock>().tick = 50;
    let edited = capture(app.world_mut()).unwrap();
    let completion = initial
        .native_owner_import
        .as_ref()
        .unwrap()
        .initial_completion
        .clone();
    assert_eq!(
        restore_native_owners(app.world_mut(), &fixture.initial_batch()).unwrap(),
        NativeRestoreResult::AlreadyCompleted(completion.clone())
    );
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
    assert_eq!(capture(restored.world_mut()).unwrap(), edited);
    assert_eq!(
        restore_native_owners(restored.world_mut(), &fixture.initial_batch()).unwrap(),
        NativeRestoreResult::AlreadyCompleted(completion)
    );
    assert_eq!(capture(restored.world_mut()).unwrap(), edited);
    assert_eq!(
        lifecycle_foundation_of(restored.world(), owner).unwrap(),
        Some(target)
    );
}

#[test]
fn native_attachment_and_replacement_retire_initial_guids_but_keep_provenance() {
    let initial = fixture::imported();
    let provenance = initial.native_owner_import.clone();
    let mut app = build_app(&ServerConfig::default(), &initial).unwrap();
    let before_entity = entity(app.world(), FIND);
    let before_handle = native_owner_handle(app.world(), FIND).unwrap();
    let attach = next_request(&initial, FIND, 900, ResolvePolicy::FindOnly);
    restore_native_owners(app.world_mut(), &attach).unwrap();
    assert_eq!(entity(app.world(), FIND), before_entity);
    assert_eq!(
        native_owner_handle(app.world(), FIND).unwrap(),
        before_handle
    );
    let attached = capture(app.world_mut()).unwrap();
    let replace = next_request(&attached, FIND, 901, ResolvePolicy::RespawnOnly);
    restore_native_owners(app.world_mut(), &replace).unwrap();
    let after_handle = native_owner_handle(app.world(), FIND).unwrap();
    assert_eq!(after_handle.incarnation, before_handle.incarnation + 1);
    assert_ne!(entity(app.world(), FIND), before_entity);
    let replaced = capture(app.world_mut()).unwrap();
    assert_eq!(replaced.native_owner_import, provenance);
    assert_eq!(replaced.native_relationships, initial.native_relationships);
    assert_eq!(replaced.preserved_prospect, initial.preserved_prospect);
    assert!(
        !replaced
            .native_lifecycle
            .as_ref()
            .unwrap()
            .bindings
            .iter()
            .any(|b| b.guid == 100 || b.guid == 900)
    );
    assert!(
        update_lifecycle_actor(
            app.world_mut(),
            before_handle,
            initial.native_world.as_ref().unwrap().actors[2]
                .transform
                .clone(),
            vec![]
        )
        .is_err()
    );
    let directory = tempfile::tempdir().unwrap();
    let mut store = SnapshotStore::open(directory.path()).unwrap();
    store.save_new(&attached).unwrap();
    assert_eq!(store.load().unwrap().unwrap().snapshot, attached);
    store.save(&replaced).unwrap();
    let loaded = store.load().unwrap().unwrap().snapshot;
    let mut app = build_app(&ServerConfig::default(), &loaded).unwrap();
    assert_eq!(capture(app.world_mut()).unwrap(), replaced);
    assert!(restore_native_owners(app.world_mut(), &attach).is_err());
    assert!(matches!(
        restore_native_owners(app.world_mut(), &replace).unwrap(),
        NativeRestoreResult::AlreadyCompleted(_)
    ));
    assert_eq!(capture(app.world_mut()).unwrap(), replaced);
}

#[test]
fn missing_or_swapped_provenance_and_source_fail_before_mutation() {
    let initial = fixture::imported();
    let other = fixture::imported_variant(1);
    for corruption in 0..4 {
        let mut app = build_app(&ServerConfig::default(), &initial).unwrap();
        let owner = native_owner_handle(app.world(), FIND).unwrap();
        let pose = initial.native_world.as_ref().unwrap().actors[2]
            .transform
            .clone();
        let metadata = app.world().resource::<C001OwnerImportState>().clone();
        let source = app.world().resource::<PreservedProspectState>().clone();
        match corruption {
            0 => {
                app.world_mut().remove_resource::<C001OwnerImportState>();
            }
            1 => {
                app.world_mut()
                    .insert_resource(C001OwnerImportState(Arc::new(
                        other.native_owner_import.clone().unwrap(),
                    )));
            }
            2 => {
                app.world_mut().remove_resource::<PreservedProspectState>();
            }
            _ => {
                app.world_mut().insert_resource(PreservedProspectState(
                    other.preserved_prospect.clone().unwrap(),
                ));
            }
        }
        let id = entity(app.world(), FIND);
        let before = *app.world().get::<Position>(id).unwrap();
        assert!(capture(app.world_mut()).is_err());
        assert!(update_lifecycle_actor(app.world_mut(), owner, pose, vec![]).is_err());
        assert!(
            restore_native_owners(app.world_mut(), &fixture::fixture().initial_batch()).is_err()
        );
        assert_eq!(*app.world().get::<Position>(id).unwrap(), before);
        app.world_mut().insert_resource(metadata);
        app.world_mut().insert_resource(source);
        assert_eq!(capture(app.world_mut()).unwrap(), initial);
    }
}

#[test]
fn stale_contexts_and_missing_hidden_components_cannot_replay_source() {
    let fixture = fixture::fixture();
    let initial = fixture.snapshot();
    let mut app = build_app(&ServerConfig::default(), &initial).unwrap();
    let old_context = app.world().resource::<NativeLifecycleContext>().clone();
    let old_dormant = app.world().resource::<DormantNativeSlots>().clone();
    let owner = native_owner_handle(app.world(), FIND).unwrap();
    let mut pose = initial.native_world.as_ref().unwrap().actors[2]
        .transform
        .clone();
    pose.translation[0] = 20.0;
    update_lifecycle_actor(app.world_mut(), owner, pose, vec![]).unwrap();
    let edited = capture(app.world_mut()).unwrap();
    let current_context = app.world().resource::<NativeLifecycleContext>().clone();
    let current_dormant = app.world().resource::<DormantNativeSlots>().clone();
    app.world_mut().insert_resource(old_context);
    app.world_mut().insert_resource(old_dormant);
    assert!(capture(app.world_mut()).is_err());
    assert!(restore_native_owners(app.world_mut(), &fixture.initial_batch()).is_err());
    app.world_mut().insert_resource(current_context);
    app.world_mut().insert_resource(current_dormant);
    assert_eq!(capture(app.world_mut()).unwrap(), edited);
    let id = entity(app.world(), FIND);
    app.world_mut().entity_mut(id).insert(Disabled);
    assert_eq!(capture(app.world_mut()).unwrap(), edited);
    let rotation = app
        .world_mut()
        .entity_mut(id)
        .take::<NativeRotation>()
        .unwrap();
    assert!(capture(app.world_mut()).is_err());
    assert!(restore_native_owners(app.world_mut(), &fixture.initial_batch()).is_err());
    app.world_mut().entity_mut(id).insert(rotation);
    assert_eq!(capture(app.world_mut()).unwrap(), edited);
}

#[test]
fn earlier_modes_refuse_owner_provenance_instead_of_dropping_it() {
    let fixture = fixture::fixture();
    let metadata = fixture.snapshot().native_owner_import.unwrap();
    for initial in [WorldSnapshot::default(), fixture.base.clone()] {
        let mut invalid = initial.clone();
        invalid.native_owner_import = Some(metadata.clone());
        assert!(invalid.validate().is_err());
        let mut app = build_app(&ServerConfig::default(), &initial).unwrap();
        app.world_mut()
            .insert_resource(C001OwnerImportState(Arc::new(metadata.clone())));
        assert!(capture(app.world_mut()).is_err());
    }
}

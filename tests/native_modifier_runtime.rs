//! Independent authored lifecycle/modifier behavior; no reference data or clients.
use bevy::{ecs::entity_disabling::Disabled, prelude::*};
use icarus_bevy::{
    config::ServerConfig,
    native_lifecycle::{
        CompiledNativeLifecycle, LifecycleDefinition, OwnerHandle, Presence, RecorderKind,
        RecorderSource, ResolvePolicy, RestoreBatch, RestoreRecorder, SlotDeclaration,
        SpawnComponent, prepare_lifecycle_world,
    },
    native_lifecycle_runtime::{
        NativeOwnerIncarnation, NativeRestoreResult, advance_native_modifiers,
        apply_native_modifier, native_owner_handle, native_owner_modifiers, restore_native_owners,
        update_lifecycle_actor,
    },
    native_modifiers::*,
    native_world::{CompiledNativeCatalog, prepare_authored_world},
    persistence::SnapshotStore,
    simulation::{DomainId, WorldSnapshot, build_app, capture},
};
use std::{sync::Arc, time::Duration};

fn fixture() -> WorldSnapshot {
    let catalog = Arc::new(
        CompiledNativeCatalog::parse(include_str!("../config/native-world.example.toml")).unwrap(),
    );
    let definition = LifecycleDefinition {
        lifecycle_format: 1,
        native_catalog_id: catalog.identity().into(),
        map: "workshop".into(),
        slots: ["marker_a", "marker_b"]
            .into_iter()
            .enumerate()
            .map(|(index, placement)| SlotDeclaration {
                placement: placement.into(),
                owner_name: vec![65 + index as u16],
                recorder_kind: RecorderKind::SpatialStateV1,
                initial_presence: Presence::Live,
                initial_component: None,
                spawn_component: SpawnComponent::DeclaredKind,
            })
            .collect(),
    };
    let compiled = CompiledNativeLifecycle::compile(definition, &catalog).unwrap();
    let base = prepare_authored_world(catalog, "workshop")
        .unwrap()
        .initial_snapshot();
    enable_modifier_owners(
        prepare_lifecycle_world(base, &compiled)
            .unwrap()
            .into_snapshot(),
    )
    .unwrap()
}
fn app() -> App {
    build_app(&ServerConfig::default(), &fixture()).unwrap()
}
fn handle(app: &App, id: u64) -> OwnerHandle {
    native_owner_handle(app.world(), DomainId(id)).unwrap()
}
fn add(app: &mut App, id: u64, name: &str, seconds: Option<f32>, amount: i32) {
    let owner = handle(app, id);
    let state = native_owner_modifiers(app.world(), owner).unwrap();
    apply_native_modifier(
        app.world_mut(),
        &ModifierRequest {
            owner,
            expected_revision: state.revision,
            action: ModifierAction::Add {
                definition: ModifierDefinition {
                    name: name.into(),
                    duration: seconds.map_or(ModifierDuration::Indefinite, |seconds| {
                        ModifierDuration::Timed { seconds }
                    }),
                    contributions: vec![FlatStatContribution {
                        stat: "power".into(),
                        amount,
                    }],
                },
            },
        },
    )
    .unwrap();
}
fn replacement(snapshot: &WorldSnapshot, id: u64) -> RestoreBatch {
    let lifecycle = snapshot.native_lifecycle.as_ref().unwrap();
    let actor = snapshot
        .native_world
        .as_ref()
        .unwrap()
        .actors
        .iter()
        .find(|actor| actor.id == DomainId(id))
        .unwrap();
    let slot = lifecycle
        .slots
        .iter()
        .find(|slot| slot.id == actor.id)
        .unwrap();
    let declaration = lifecycle
        .definition
        .slots
        .iter()
        .find(|slot| slot.placement == actor.placement)
        .unwrap();
    RestoreBatch {
        lifecycle_id: lifecycle.lifecycle_id.clone(),
        sequence: lifecycle.restore_sequence + 1,
        recorders: vec![RestoreRecorder {
            guid: 101,
            kind: RecorderKind::SpatialStateV1,
            target_placement: actor.placement.clone(),
            owner_name: declaration.owner_name.clone(),
            policy: ResolvePolicy::RespawnOnly,
            source: RecorderSource::Detached {
                expected_prior_binding: None,
            },
            create_if_missing: true,
            expected_presence: Presence::Live,
            expected_incarnation: slot.incarnation,
            transform: actor.transform.clone(),
            variables: actor.variables.clone(),
        }],
    }
}

#[test]
fn live_timer_capture_durable_reload_and_expiration_preserve_current_progress() {
    let mut app = app();
    add(&mut app, 1, "timed", Some(12.0), 7);
    add(&mut app, 2, "steady", None, -3);
    advance_native_modifiers(app.world_mut(), Duration::from_millis(3500)).unwrap();
    let current = capture(app.world_mut()).unwrap();
    let owner = handle(&app, 1);
    assert_eq!(
        native_owner_modifiers(app.world(), owner).unwrap().entries[0].timer,
        ModifierTimer::Timed {
            remaining_nanos: 8_500_000_000,
            armed: true
        }
    );
    let directory = tempfile::tempdir().unwrap();
    let mut store = SnapshotStore::open(directory.path()).unwrap();
    store.save_new(&current).unwrap();
    let loaded = store.load().unwrap().unwrap().snapshot;
    assert_eq!(loaded, current);
    let mut reloaded = build_app(&ServerConfig::default(), &loaded).unwrap();
    advance_native_modifiers(reloaded.world_mut(), Duration::from_millis(8500)).unwrap();
    let expired = native_owner_modifiers(reloaded.world(), owner).unwrap();
    assert!(expired.entries.is_empty());
    assert!(expired.cache.is_empty());
    let kept = native_owner_modifiers(reloaded.world(), handle(&reloaded, 2)).unwrap();
    assert_eq!(kept.cache[0].amount, -3);
    let snapshot = capture(reloaded.world_mut()).unwrap();
    store.save(&snapshot).unwrap();
    assert_eq!(store.load().unwrap().unwrap().snapshot, snapshot);
}

#[test]
fn unrelated_pose_and_replacement_preserve_other_owners_and_exact_retry_progress() {
    let mut app = app();
    add(&mut app, 1, "retired", Some(3.0), 1);
    add(&mut app, 2, "kept", Some(8.0), 2);
    let old_handle = handle(&app, 1);
    let before = capture(app.world_mut()).unwrap();
    let request = replacement(&before, 1);
    restore_native_owners(app.world_mut(), &request).unwrap();
    assert!(native_owner_modifiers(app.world(), old_handle).is_err());
    let new_handle = handle(&app, 1);
    assert_ne!(new_handle, old_handle);
    assert!(
        native_owner_modifiers(app.world(), new_handle)
            .unwrap()
            .entries
            .is_empty()
    );
    advance_native_modifiers(app.world_mut(), Duration::from_secs(2)).unwrap();
    let owner_b = handle(&app, 2);
    let current = native_owner_modifiers(app.world(), owner_b).unwrap();
    let pose = capture(app.world_mut())
        .unwrap()
        .native_world
        .unwrap()
        .actors[0]
        .clone();
    let mut transform = pose.transform;
    transform.translation[0] += 1.0;
    update_lifecycle_actor(app.world_mut(), new_handle, transform, pose.variables).unwrap();
    assert_eq!(
        native_owner_modifiers(app.world(), owner_b).unwrap(),
        current
    );
    let after = capture(app.world_mut()).unwrap();
    assert!(matches!(
        restore_native_owners(app.world_mut(), &request).unwrap(),
        NativeRestoreResult::AlreadyCompleted(_)
    ));
    assert_eq!(capture(app.world_mut()).unwrap(), after);
    let directory = tempfile::tempdir().unwrap();
    let mut store = SnapshotStore::open(directory.path()).unwrap();
    store.save_new(&after).unwrap();
    let loaded = store.load().unwrap().unwrap().snapshot;
    let mut app = build_app(&ServerConfig::default(), &loaded).unwrap();
    assert!(matches!(
        restore_native_owners(app.world_mut(), &request).unwrap(),
        NativeRestoreResult::AlreadyCompleted(_)
    ));
    assert_eq!(capture(app.world_mut()).unwrap(), after);
}

#[test]
fn last_owner_expiration_overflow_keeps_every_timer_cache_and_revision_unchanged() {
    let mut app = app();
    add(&mut app, 1, "first", Some(1.0), 7);
    add(&mut app, 2, "max", None, i32::MAX);
    add(&mut app, 2, "cancel", Some(1.0), -1);
    add(&mut app, 2, "plus", None, 1);
    let before = capture(app.world_mut()).unwrap();
    assert!(advance_native_modifiers(app.world_mut(), Duration::from_secs(1)).is_err());
    assert_eq!(capture(app.world_mut()).unwrap(), before);
}

#[test]
fn fixed_schedule_ages_modifiers_and_latches_corrupt_disabled_owner_closed() {
    let mut app = app();
    add(&mut app, 1, "timed", Some(0.5), 7);
    app.insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
        Duration::from_millis(100),
    ));
    for _ in 0..8 {
        app.update();
    }
    assert!(
        native_owner_modifiers(app.world(), handle(&app, 1))
            .unwrap()
            .entries
            .is_empty()
    );
    let entity = app
        .world()
        .iter_entities()
        .find(|entity| entity.get::<DomainId>() == Some(&DomainId(2)))
        .unwrap()
        .id();
    app.world_mut()
        .entity_mut(entity)
        .insert(Disabled)
        .remove::<NativeOwnerIncarnation>();
    app.update();
    assert!(icarus_bevy::native_lifecycle_runtime::modifier_runtime_health(app.world()).is_err());
    assert!(capture(app.world_mut()).is_err());
}

#[test]
fn no_implicit_promotion_or_modifiers_on_older_schemas() {
    let mut snapshot = fixture();
    snapshot.schema_version = 7;
    assert!(snapshot.validate().is_err());
    snapshot.native_modifiers = None;
    snapshot.validate().unwrap();
    let mut promoted = enable_modifier_owners(snapshot).unwrap();
    promoted.native_modifiers = None;
    assert!(promoted.validate().is_err());
    assert!(enable_modifier_owners(WorldSnapshot::default()).is_err());
}

#[test]
fn failed_generation_publication_preserves_acknowledged_modifier_progress() {
    let mut app = app();
    add(&mut app, 1, "durable", Some(5.0), 4);
    let directory = tempfile::tempdir().unwrap();
    let mut store = SnapshotStore::open(directory.path()).unwrap();
    let acknowledged = capture(app.world_mut()).unwrap();
    let receipt = store.save_new(&acknowledged).unwrap();
    advance_native_modifiers(app.world_mut(), Duration::from_secs(1)).unwrap();
    let changed = capture(app.world_mut()).unwrap();
    std::fs::create_dir(
        directory
            .path()
            .join(format!("snapshot-{}.json", receipt.generation + 1)),
    )
    .unwrap();
    assert!(store.save(&changed).is_err());
    assert_eq!(store.load().unwrap().unwrap().snapshot, acknowledged);
    assert_eq!(capture(app.world_mut()).unwrap(), changed);
}

#[test]
fn failed_tick_latches_its_pinned_activation_without_poisoning_a_transplanted_context() {
    use icarus_bevy::native_lifecycle_runtime::{NativeLifecycleContext, modifier_runtime_health};
    let mut original = app();
    let other = app();
    let own_context = original
        .world()
        .resource::<NativeLifecycleContext>()
        .clone();
    let foreign_context = other.world().resource::<NativeLifecycleContext>().clone();
    original.world_mut().insert_resource(foreign_context);
    original.insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
        Duration::from_millis(100),
    ));
    for _ in 0..3 {
        original.update();
    }
    assert!(modifier_runtime_health(original.world()).is_err());
    assert!(modifier_runtime_health(other.world()).is_ok());
    original.world_mut().insert_resource(own_context);
    assert!(capture(original.world_mut()).is_err());
    assert!(advance_native_modifiers(original.world_mut(), Duration::from_secs(1)).is_err());
}

#[test]
fn missing_context_during_fixed_tick_fails_without_panicking_or_reopening_after_restore() {
    use icarus_bevy::native_lifecycle_runtime::{NativeLifecycleContext, modifier_runtime_health};
    let mut app = app();
    let context = app
        .world_mut()
        .remove_resource::<NativeLifecycleContext>()
        .unwrap();
    app.insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
        Duration::from_millis(100),
    ));
    for _ in 0..3 {
        app.update();
    }
    assert!(modifier_runtime_health(app.world()).is_err());
    app.world_mut().insert_resource(context);
    assert!(capture(app.world_mut()).is_err());
}

#[test]
fn transplanted_capture_mode_cannot_poison_another_app_or_reopen_after_restoration() {
    use icarus_bevy::native_lifecycle_runtime::modifier_runtime_health;
    use icarus_bevy::simulation::SnapshotMode;
    let mut original = app();
    let other = app();
    let own = original.world().resource::<SnapshotMode>().clone();
    original
        .world_mut()
        .insert_resource(other.world().resource::<SnapshotMode>().clone());
    original.insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
        Duration::from_millis(100),
    ));
    for _ in 0..3 {
        original.update();
    }
    assert!(modifier_runtime_health(original.world()).is_err());
    assert!(modifier_runtime_health(other.world()).is_ok());
    original.world_mut().insert_resource(own);
    assert!(capture(original.world_mut()).is_err());
    assert!(advance_native_modifiers(original.world_mut(), Duration::from_secs(1)).is_err());
}

#[test]
fn missing_capture_mode_during_fixed_tick_keeps_owning_activation_failed_after_restore() {
    use icarus_bevy::simulation::SnapshotMode;
    let mut app = app();
    let mode = app.world_mut().remove_resource::<SnapshotMode>().unwrap();
    app.insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
        Duration::from_millis(100),
    ));
    for _ in 0..3 {
        app.update();
    }
    app.world_mut().insert_resource(mode);
    assert!(capture(app.world_mut()).is_err());
}

use std::{
    fs,
    process::Command,
    sync::{Arc, Mutex},
};

use bevy::prelude::*;
use icarus_bevy::{
    config::ServerConfig,
    native_runtime::{
        NativeActorIdentity, NativeLocalBounds, NativeRotation, NativeScale, NativeVariables,
        NativeWorldContext, native_world_bounds, update_native_actor,
    },
    native_world::{
        Aabb3, CompiledNativeCatalog, MAX_NATIVE_STRING_UNITS, MAX_NATIVE_VARIABLE_BYTES,
        MAX_NATIVE_VARIABLES_PER_CLASS, NativeCatalogDefinition, NativeClassDefinition,
        NativeClassKind, NativeFrame, NativeMapDefinition, NativePlacement, NativeTileDefinition,
        NativeTransform, NativeValue, NativeVariableValue, VariableDefinition,
        prepare_authored_world,
    },
    persistence::SnapshotStore,
    simulation::{
        DomainId, Position, SimulationClock, SnapshotMode, WorldSnapshot, build_app, capture,
    },
};

// Concurrent CLI forks can retain a sibling fixture's advisory lock until
// exec. Serialize this executable's child launches and store fixtures, as in
// native_object_admission; production lock semantics remain unchanged.
static FIXTURE_LOCK: Mutex<()> = Mutex::new(());

fn definition() -> NativeCatalogDefinition {
    NativeCatalogDefinition {
        native_format: 1,
        frame: NativeFrame::RightHandedZUpMeters,
        classes: vec![NativeClassDefinition {
            key: "private_class".into(),
            kind: NativeClassKind::SpatialObjectV1,
            local_bounds: Aabb3 {
                min: [-1.0, -2.0, -1.0],
                max: [1.0, 2.0, 1.0],
            },
            variables: vec![VariableDefinition {
                key: "private_switch".into(),
                default: NativeValue::Bool(true),
            }],
        }],
        maps: vec![NativeMapDefinition {
            key: "private_map".into(),
            tiles: vec![NativeTileDefinition {
                key: "private_tile".into(),
                bounds: Aabb3 {
                    min: [-100.0; 3],
                    max: [100.0; 3],
                },
            }],
            placements: vec![NativePlacement {
                key: "private_placement".into(),
                class: "private_class".into(),
                tile: "private_tile".into(),
                transform: NativeTransform {
                    translation: [5.0, 6.0, 7.0],
                    rotation: [
                        0.0,
                        0.0,
                        std::f32::consts::FRAC_1_SQRT_2,
                        std::f32::consts::FRAC_1_SQRT_2,
                    ],
                    scale: [2.0, 1.0, 3.0],
                },
                initial_values: Vec::new(),
            }],
        }],
    }
}

fn snapshot() -> WorldSnapshot {
    prepare_authored_world(
        Arc::new(CompiledNativeCatalog::compile(definition()).unwrap()),
        "private_map",
    )
    .unwrap()
    .initial_snapshot()
}

fn actor(app: &mut App) -> Entity {
    app.world_mut()
        .query::<(Entity, &DomainId)>()
        .iter(app.world())
        .next()
        .unwrap()
        .0
}

#[test]
fn authored_world_mutations_capture_and_reload_full_pose_state_and_bounds() {
    let _fixture = FIXTURE_LOCK.lock().unwrap();
    let directory = tempfile::tempdir().unwrap();
    let initial = snapshot();
    let mut store = SnapshotStore::open(directory.path()).unwrap();
    store.save_new(&initial).unwrap();
    let mut app = build_app(
        &ServerConfig::default(),
        &store.load().unwrap().unwrap().snapshot,
    )
    .unwrap();
    let bounds = native_world_bounds(app.world_mut(), DomainId(1)).unwrap();
    for (actual, expected) in bounds.min.iter().zip([3.0, 4.0, 4.0]) {
        assert!((actual - expected).abs() < 1e-5);
    }
    for (actual, expected) in bounds.max.iter().zip([7.0, 8.0, 10.0]) {
        assert!((actual - expected).abs() < 1e-5);
    }
    let pose = NativeTransform {
        translation: [-2.0, 4.0, 8.0],
        scale: [1.0, 2.0, 1.0],
        ..Default::default()
    };
    let variables = vec![NativeVariableValue {
        key: "private_switch".into(),
        value: NativeValue::Bool(false),
    }];
    update_native_actor(
        app.world_mut(),
        DomainId(1),
        pose.clone(),
        variables.clone(),
    )
    .unwrap();
    app.world_mut().resource_mut::<SimulationClock>().tick = 13;
    let changed = capture(app.world_mut()).unwrap();
    assert_eq!(changed.schema_version, 4);
    let native = changed.native_world.as_ref().unwrap();
    assert_eq!(native.actors[0].transform, pose);
    assert_eq!(native.actors[0].variables, variables);
    let bounds = native_world_bounds(app.world_mut(), DomainId(1)).unwrap();
    assert_eq!(bounds.min, [-3.0, 0.0, 7.0]);
    assert_eq!(bounds.max, [-1.0, 8.0, 9.0]);
    store.save(&changed).unwrap();
    drop(store);
    let loaded = SnapshotStore::open(directory.path())
        .unwrap()
        .load()
        .unwrap()
        .unwrap()
        .snapshot;
    assert_eq!(loaded, changed);
    let mut reopened = build_app(&ServerConfig::default(), &loaded).unwrap();
    assert_eq!(capture(reopened.world_mut()).unwrap(), changed);
}

fn missing_component<T: Component + Clone>() {
    let directory = tempfile::tempdir().unwrap();
    let initial = snapshot();
    let mut store = SnapshotStore::open(directory.path()).unwrap();
    store.save_new(&initial).unwrap();
    let before = fs::read(directory.path().join("manifest.json")).unwrap();
    let mut app = build_app(&ServerConfig::default(), &initial).unwrap();
    let entity = actor(&mut app);
    let saved = app.world().get::<T>(entity).unwrap().clone();
    app.world_mut().entity_mut(entity).remove::<T>();
    let original = &initial.native_world.as_ref().unwrap().actors[0];
    assert!(native_world_bounds(app.world_mut(), original.id).is_err());
    assert!(
        update_native_actor(
            app.world_mut(),
            original.id,
            original.transform.clone(),
            original.variables.clone(),
        )
        .is_err()
    );
    assert!(
        capture(app.world_mut())
            .and_then(|snapshot| store.save(&snapshot))
            .is_err()
    );
    assert_eq!(
        fs::read(directory.path().join("manifest.json")).unwrap(),
        before
    );
    app.world_mut().entity_mut(entity).insert(saved);
    assert_eq!(capture(app.world_mut()).unwrap(), initial);
    assert_eq!(
        store
            .save(&capture(app.world_mut()).unwrap())
            .unwrap()
            .generation,
        2
    );
}

#[test]
fn every_required_native_component_is_checked_and_repair_permits_retry() {
    let _fixture = FIXTURE_LOCK.lock().unwrap();
    missing_component::<DomainId>();
    missing_component::<Position>();
    missing_component::<NativeActorIdentity>();
    missing_component::<NativeRotation>();
    missing_component::<NativeScale>();
    missing_component::<NativeVariables>();
    missing_component::<NativeLocalBounds>();
}

#[test]
fn missing_context_mode_clock_or_whole_actor_never_downgrades_the_snapshot() {
    let _fixture = FIXTURE_LOCK.lock().unwrap();
    let initial = snapshot();
    let mut app = build_app(&ServerConfig::default(), &initial).unwrap();
    let context = app
        .world_mut()
        .remove_resource::<NativeWorldContext>()
        .unwrap();
    assert!(capture(app.world_mut()).is_err());
    app.insert_resource(context);
    let mode = app.world_mut().remove_resource::<SnapshotMode>().unwrap();
    assert!(capture(app.world_mut()).is_err());
    app.insert_resource(mode);
    let clock = app
        .world_mut()
        .remove_resource::<SimulationClock>()
        .unwrap();
    assert!(capture(app.world_mut()).is_err());
    app.insert_resource(clock);
    assert_eq!(capture(app.world_mut()).unwrap(), initial);
    let entity = actor(&mut app);
    app.world_mut().despawn(entity);
    assert!(capture(app.world_mut()).is_err());
}

#[test]
fn duplicate_unregistered_and_corrupted_actors_fail_capture() {
    let _fixture = FIXTURE_LOCK.lock().unwrap();
    let mut app = build_app(&ServerConfig::default(), &snapshot()).unwrap();
    let duplicate = app
        .world_mut()
        .spawn((DomainId(1), Position([0.0; 3])))
        .id();
    assert!(capture(app.world_mut()).is_err());
    app.world_mut().despawn(duplicate);
    let unregistered = app.world_mut().spawn(NativeScale([1.0; 3])).id();
    assert!(capture(app.world_mut()).is_err());
    app.world_mut().despawn(unregistered);
    let entity = actor(&mut app);
    let bounds = app
        .world()
        .get::<NativeLocalBounds>(entity)
        .unwrap()
        .clone();
    app.world_mut()
        .get_mut::<NativeLocalBounds>(entity)
        .unwrap()
        .0
        .min[0] = -2.0;
    assert!(capture(app.world_mut()).is_err());
    app.world_mut().entity_mut(entity).insert(bounds);
    app.world_mut()
        .get_mut::<NativeVariables>(entity)
        .unwrap()
        .0[0]
        .value = NativeValue::I32(7);
    assert!(capture(app.world_mut()).is_err());
}

#[test]
fn disabling_components_cannot_hide_invalid_topology_from_capture() {
    let _fixture = FIXTURE_LOCK.lock().unwrap();
    use bevy::ecs::entity_disabling::{DefaultQueryFilters, Disabled};

    #[derive(Component, Default)]
    struct Dormant;

    fn check<T: Component + Default>() {
        let initial = snapshot();
        let original = &initial.native_world.as_ref().unwrap().actors[0];
        let rejects_edit_and_bounds = |world: &mut World| {
            assert!(native_world_bounds(world, original.id).is_err());
            assert!(
                update_native_actor(
                    world,
                    original.id,
                    original.transform.clone(),
                    original.variables.clone(),
                )
                .is_err()
            );
        };
        let mut app = build_app(&ServerConfig::default(), &initial).unwrap();
        let disabling = app.world_mut().register_component::<T>();
        app.world_mut()
            .resource_mut::<DefaultQueryFilters>()
            .register_disabling_component(disabling);
        let registered = actor(&mut app);
        let directory = tempfile::tempdir().unwrap();
        let mut store = SnapshotStore::open(directory.path()).unwrap();
        store.save_new(&initial).unwrap();
        let manifest = fs::read(directory.path().join("manifest.json")).unwrap();
        let duplicate = app
            .world_mut()
            .spawn((T::default(), DomainId(1), Position([0.0; 3])))
            .id();
        rejects_edit_and_bounds(app.world_mut());
        assert!(
            capture(app.world_mut())
                .and_then(|s| store.save(&s))
                .is_err()
        );
        app.world_mut().despawn(duplicate);
        let unregistered = app
            .world_mut()
            .spawn((T::default(), NativeScale([1.0; 3])))
            .id();
        rejects_edit_and_bounds(app.world_mut());
        assert!(
            capture(app.world_mut())
                .and_then(|s| store.save(&s))
                .is_err()
        );
        assert_eq!(
            fs::read(directory.path().join("manifest.json")).unwrap(),
            manifest
        );
        app.world_mut().despawn(unregistered);
        // Legitimate disabled actors remain part of the persisted topology.
        app.world_mut().entity_mut(registered).insert(T::default());
        native_world_bounds(app.world_mut(), original.id).unwrap();
        update_native_actor(
            app.world_mut(),
            original.id,
            original.transform.clone(),
            original.variables.clone(),
        )
        .unwrap();
        assert_eq!(capture(app.world_mut()).unwrap(), initial);
        assert_eq!(
            store
                .save(&capture(app.world_mut()).unwrap())
                .unwrap()
                .generation,
            2
        );
    }
    check::<Disabled>();
    check::<Dormant>();
}

#[test]
fn invalid_pose_or_state_edit_is_atomic_and_has_private_free_errors() {
    let _fixture = FIXTURE_LOCK.lock().unwrap();
    let initial = snapshot();
    let original = &initial.native_world.as_ref().unwrap().actors[0];
    let mut app = build_app(&ServerConfig::default(), &initial).unwrap();
    let mut outside = original.transform.clone();
    outside.translation = [1000.0; 3];
    let error = update_native_actor(
        app.world_mut(),
        original.id,
        outside,
        original.variables.clone(),
    )
    .unwrap_err();
    assert!(!format!("{error:#}").contains("private_"));
    assert_eq!(capture(app.world_mut()).unwrap(), initial);
    assert!(
        update_native_actor(
            app.world_mut(),
            original.id,
            original.transform.clone(),
            Vec::new()
        )
        .is_err()
    );
    assert_eq!(capture(app.world_mut()).unwrap(), initial);
    let entity = actor(&mut app);
    app.world_mut()
        .get_mut::<NativeVariables>(entity)
        .unwrap()
        .0[0]
        .value = NativeValue::I32(7);
    // A valid replacement must not hide an invalid current record.
    assert!(
        update_native_actor(
            app.world_mut(),
            original.id,
            original.transform.clone(),
            original.variables.clone(),
        )
        .is_err()
    );
    assert!(matches!(
        app.world().get::<NativeVariables>(entity).unwrap().0[0].value,
        NativeValue::I32(7)
    ));
    app.world_mut()
        .entity_mut(entity)
        .insert(NativeVariables(original.variables.clone()));
    assert_eq!(capture(app.world_mut()).unwrap(), initial);
}

fn unrelated_damage<T: Component + Clone>(damage: impl FnOnce(&mut World, Entity)) {
    let mut definition = definition();
    let mut other = definition.maps[0].placements[0].clone();
    other.key = "second_placement".into();
    definition.maps[0].placements.push(other);
    let initial = prepare_authored_world(
        Arc::new(CompiledNativeCatalog::compile(definition).unwrap()),
        "private_map",
    )
    .unwrap()
    .initial_snapshot();
    let original = &initial.native_world.as_ref().unwrap().actors[0];
    let mut app = build_app(&ServerConfig::default(), &initial).unwrap();
    let unrelated = app
        .world()
        .iter_entities()
        .find(|entity| entity.get::<DomainId>() == Some(&DomainId(2)))
        .unwrap()
        .id();
    let saved = app.world().get::<T>(unrelated).unwrap().clone();
    let bounds = native_world_bounds(app.world_mut(), original.id).unwrap();
    let mut proposed = original.transform.clone();
    proposed.translation = [0.0; 3];
    damage(app.world_mut(), unrelated);
    assert!(native_world_bounds(app.world_mut(), original.id).is_err());
    assert!(
        update_native_actor(
            app.world_mut(),
            original.id,
            proposed.clone(),
            original.variables.clone(),
        )
        .is_err()
    );
    assert!(capture(app.world_mut()).is_err());
    app.world_mut().entity_mut(unrelated).insert(saved);
    // Repairing the unrelated actor exposes any partial edit of the target.
    assert_eq!(capture(app.world_mut()).unwrap(), initial);
    assert_eq!(
        native_world_bounds(app.world_mut(), original.id).unwrap(),
        bounds
    );
    update_native_actor(
        app.world_mut(),
        original.id,
        proposed.clone(),
        original.variables.clone(),
    )
    .unwrap();
    let changed = capture(app.world_mut()).unwrap();
    assert_eq!(
        changed.native_world.as_ref().unwrap().actors[0].transform,
        proposed
    );
    changed.validate().unwrap();
}

#[test]
fn bounds_and_edits_reject_unrelated_corruption_and_retry_after_repair() {
    let _fixture = FIXTURE_LOCK.lock().unwrap();
    unrelated_damage::<NativeVariables>(|world, entity| {
        world.get_mut::<NativeVariables>(entity).unwrap().0[0].value = NativeValue::I32(7);
    });
    unrelated_damage::<Position>(|world, entity| {
        world.get_mut::<Position>(entity).unwrap().0 = [1000.0; 3];
    });
    unrelated_damage::<NativeActorIdentity>(|world, entity| {
        world.get_mut::<NativeActorIdentity>(entity).unwrap().tile = "missing_tile".into();
    });
    unrelated_damage::<DomainId>(|world, entity| {
        world.entity_mut(entity).insert(DomainId(1));
    });
    unrelated_damage::<NativeLocalBounds>(|world, entity| {
        world.get_mut::<NativeLocalBounds>(entity).unwrap().0.min[0] = -2.0;
    });
    unrelated_damage::<NativeRotation>(|world, entity| {
        world.entity_mut(entity).remove::<NativeRotation>();
    });
}

#[test]
fn proposed_edit_checks_aggregate_bytes_without_partial_writes() {
    let _fixture = FIXTURE_LOCK.lock().unwrap();
    let mut definition = definition();
    definition.classes[0].variables = (0..MAX_NATIVE_VARIABLES_PER_CLASS)
        .map(|i| VariableDefinition {
            key: format!("v{i:02}"),
            default: NativeValue::String(String::new()),
        })
        .collect();
    let placement = definition.maps[0].placements[0].clone();
    definition.maps[0].placements = (0..96)
        .map(|i| NativePlacement {
            key: format!("p{i:02}"),
            ..placement.clone()
        })
        .collect();
    let initial = prepare_authored_world(
        Arc::new(CompiledNativeCatalog::compile(definition).unwrap()),
        "private_map",
    )
    .unwrap()
    .initial_snapshot();
    let original = &initial.native_world.as_ref().unwrap().actors[0];
    let expanded: Vec<_> = original
        .variables
        .iter()
        .map(|v| NativeVariableValue {
            key: v.key.clone(),
            // JSON escaping counts toward the byte budget, independently of UTF-16.
            value: NativeValue::String("\0".repeat(MAX_NATIVE_STRING_UNITS)),
        })
        .collect();
    let bytes = |values: &[NativeVariableValue]| {
        values
            .iter()
            .map(|value| serde_json::to_vec(value).unwrap().len())
            .sum::<usize>()
    };
    let empty_bytes = bytes(&original.variables);
    let expanded_bytes = bytes(&expanded);
    let actors = initial.native_world.as_ref().unwrap().actors.len();
    let filled =
        (MAX_NATIVE_VARIABLE_BYTES - actors * empty_bytes) / (expanded_bytes - empty_bytes);
    assert!(filled < actors);
    let mut app = build_app(&ServerConfig::default(), &initial).unwrap();
    let entities: Vec<_> = app
        .world()
        .iter_entities()
        .filter_map(|entity| entity.get::<DomainId>().map(|id| (*id, entity.id())))
        .collect();
    for (id, entity) in entities {
        if id.0 > 1 && id.0 <= filled as u64 + 1 {
            app.world_mut()
                .entity_mut(entity)
                .insert(NativeVariables(expanded.clone()));
        }
    }
    let bounds = native_world_bounds(app.world_mut(), original.id).unwrap();
    let mut pose = original.transform.clone();
    pose.translation = [0.0; 3];
    assert!(update_native_actor(app.world_mut(), original.id, pose.clone(), expanded).is_err());
    assert_eq!(
        native_world_bounds(app.world_mut(), original.id).unwrap(),
        bounds
    );
    let unchanged = capture(app.world_mut()).unwrap();
    assert_eq!(
        &unchanged.native_world.as_ref().unwrap().actors[0],
        original
    );
    update_native_actor(
        app.world_mut(),
        original.id,
        pose.clone(),
        original.variables.clone(),
    )
    .unwrap();
    let changed = capture(app.world_mut()).unwrap();
    assert_eq!(
        changed.native_world.as_ref().unwrap().actors[0].transform,
        pose
    );
    changed.validate().unwrap();
}

fn create(
    catalog: &std::path::Path,
    destination: &std::path::Path,
    dry: bool,
) -> std::process::Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_icarus-server"));
    command
        .arg("create-native-world")
        .arg("--catalog")
        .arg(catalog)
        .args(["--map", "private_map", "--data-dir"])
        .arg(destination);
    if dry {
        command.arg("--dry-run");
    }
    command.output().unwrap()
}

#[test]
fn cli_prepares_without_writes_then_runs_without_the_source_catalog() {
    let _fixture = FIXTURE_LOCK.lock().unwrap();
    let directory = tempfile::tempdir().unwrap();
    let catalog = directory.path().join("authored.toml");
    fs::write(&catalog, toml::to_string(&definition()).unwrap()).unwrap();
    let destination = directory.path().join("native");
    for dry in [true, false] {
        let output = create(&catalog, &destination, dry);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(report["actors"], 1);
        assert_eq!(report["gameplay_ready"], false);
        assert_eq!(report["world_imported"], false);
        assert!(!String::from_utf8_lossy(&output.stdout).contains("private_"));
        assert_eq!(destination.exists(), !dry);
    }
    let manifest = fs::read(destination.join("manifest.json")).unwrap();
    let duplicate = create(&catalog, &destination, false);
    assert!(!duplicate.status.success());
    assert!(duplicate.stdout.is_empty());
    assert_eq!(
        fs::read(destination.join("manifest.json")).unwrap(),
        manifest
    );
    fs::remove_file(&catalog).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_icarus-server"))
        .args(["run", "--data-dir"])
        .arg(&destination)
        .args(["--max-ticks", "2", "--compressed"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let loaded = SnapshotStore::open(&destination)
        .unwrap()
        .load()
        .unwrap()
        .unwrap()
        .snapshot;
    assert_eq!(loaded.schema_version, 4);
    assert_eq!(loaded.tick, 2);
    assert_eq!(loaded.native_world, snapshot().native_world);
}

#[test]
fn cli_invalid_placement_leaves_absent_and_existing_destinations_untouched() {
    let _fixture = FIXTURE_LOCK.lock().unwrap();
    let directory = tempfile::tempdir().unwrap();
    let catalog = directory.path().join("invalid.toml");
    let mut definition = definition();
    let mut invalid = definition.maps[0].placements[0].clone();
    invalid.key = "second".into();
    invalid.tile = "private_missing_tile".into();
    definition.maps[0].placements.push(invalid);
    fs::write(&catalog, toml::to_string(&definition).unwrap()).unwrap();
    for existing in [false, true] {
        let destination = directory
            .path()
            .join(if existing { "existing" } else { "absent" });
        if existing {
            fs::create_dir(&destination).unwrap();
            fs::write(destination.join("sentinel"), b"keep").unwrap();
        }
        for dry in [false, true] {
            let output = create(&catalog, &destination, dry);
            assert!(!output.status.success());
            assert!(output.stdout.is_empty());
            assert!(!String::from_utf8_lossy(&output.stderr).contains("private_"));
            assert_eq!(destination.exists(), existing);
            if existing {
                assert_eq!(fs::read_dir(&destination).unwrap().count(), 1);
                assert_eq!(fs::read(destination.join("sentinel")).unwrap(), b"keep");
            }
        }
    }
}

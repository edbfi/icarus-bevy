use std::{
    fs,
    time::{Duration, Instant},
};

use bevy::{prelude::*, time::TimeUpdateStrategy};
use icarus_bevy::{
    config::{ConfigOptions, ServerConfig},
    persistence::{SaveWorker, SnapshotStore},
    simulation::{
        DomainId, EntityRecord, Position, RunClock, SimulationClock, TICK_HISTOGRAM_BUCKETS,
        TickMetrics, WorldSnapshot, build_app, capture,
    },
};

fn snapshot(tick: u64) -> WorldSnapshot {
    WorldSnapshot {
        tick,
        entities: vec![EntityRecord {
            id: DomainId(42),
            position: Position([1.0, 2.0, 3.0]),
        }],
        ..Default::default()
    }
}

#[test]
fn stale_snapshot_cannot_replace_acknowledged_newer_simulation_state() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = SnapshotStore::open(directory.path()).unwrap();
    let acknowledged = store.save(&snapshot(20)).unwrap();
    let manifest_before = std::fs::read(directory.path().join("manifest.json")).unwrap();
    let error = store.save(&snapshot(10)).unwrap_err();
    assert!(error.to_string().contains("stale snapshot"));
    assert_eq!(
        std::fs::read(directory.path().join("manifest.json")).unwrap(),
        manifest_before
    );
    assert!(!directory.path().join("snapshot-2.json").exists());
    assert_eq!(store.load().unwrap().unwrap().snapshot, snapshot(20));
    // Final shutdown publication may legitimately repeat the same tick.
    assert_eq!(
        store.save(&snapshot(20)).unwrap().generation,
        acknowledged.generation + 1
    );
}

#[test]
fn configuration_precedence_and_unknown_fields() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("server.toml");
    fs::write(&path, "tick_hz = 20\nautosave_seconds = 30\n").unwrap();
    let options = ConfigOptions {
        config: Some(path.clone()),
        tick_hz: Some(120),
        ..Default::default()
    };
    let config = options.resolve().unwrap();
    assert_eq!(config.tick_hz, 120);
    assert_eq!(config.autosave_seconds, 30);
    assert_eq!(config.max_catch_up_ticks, 4);
    fs::write(path, "tick_hzz = 20\n").unwrap();
    assert!(options.resolve().is_err());
}

#[test]
fn invalid_configuration_is_rejected() {
    assert!(
        ServerConfig {
            tick_hz: 0,
            ..Default::default()
        }
        .validate()
        .is_err()
    );
    assert!(
        ServerConfig {
            autosave_seconds: 0,
            ..Default::default()
        }
        .validate()
        .is_err()
    );
    assert!(
        ServerConfig {
            max_catch_up_ticks: 17,
            ..Default::default()
        }
        .validate()
        .is_err()
    );
    assert!(
        ServerConfig {
            data_dir: "".into(),
            ..Default::default()
        }
        .validate()
        .is_err()
    );
}

#[test]
fn bevy_advances_fixed_time_and_preserves_domain_state() {
    let mut app = build_app(&ServerConfig::default(), &snapshot(100)).unwrap();
    app.insert_resource(TimeUpdateStrategy::FixedTimesteps(1));
    app.update();
    assert_eq!(app.world().resource::<SimulationClock>().tick, 100);
    for _ in 0..10 {
        app.update();
    }
    assert_eq!(capture(app.world_mut()).unwrap(), snapshot(110));
}

#[test]
fn every_fixed_tick_is_recorded_in_the_bounded_tick_histogram() {
    let mut app = build_app(&ServerConfig::default(), &snapshot(0)).unwrap();
    app.insert_resource(TimeUpdateStrategy::FixedTimesteps(1));
    for _ in 0..25 {
        app.update();
    }
    let ticks = app.world().resource::<SimulationClock>().tick;
    let metrics = app.world().resource::<TickMetrics>();
    assert_eq!(ticks, 24);
    assert_eq!(metrics.tick_micros.count(), ticks);
    assert_eq!(
        metrics.tick_micros.range_micros(),
        TICK_HISTOGRAM_BUCKETS as u64
    );
    assert_eq!(
        metrics.tick_micros.max_micros(),
        Some(metrics.max_tick_micros)
    );
    let p50 = metrics.tick_micros.quantile_micros(50).unwrap();
    let p99 = metrics.tick_micros.quantile_micros(99).unwrap();
    assert!(p50 <= p99 && p99 <= metrics.max_tick_micros);
}

#[test]
fn catch_up_is_bounded_and_capture_ignores_resource_entities() {
    let config = ServerConfig {
        tick_hz: 20,
        max_catch_up_ticks: 3,
        ..Default::default()
    };
    let mut app = build_app(&config, &WorldSnapshot::default()).unwrap();
    app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs(60)));
    app.update();
    app.update();
    let state = capture(app.world_mut()).unwrap();
    assert_eq!(state.tick, 3);
    assert!(state.entities.is_empty());
}

#[test]
fn invalid_world_is_rejected_before_building_app() {
    let mut state = snapshot(0);
    state.entities.push(state.entities[0].clone());
    assert!(build_app(&ServerConfig::default(), &state).is_err());
    state.entities.pop();
    state.entities[0].position.0[0] = f32::NAN;
    assert!(state.validate().is_err());
}

#[test]
fn incomplete_domain_capture_preserves_commit_and_repair_retries_exact_state() {
    let directory = tempfile::tempdir().unwrap();
    let original = snapshot(100);
    let mut store = SnapshotStore::open(directory.path()).unwrap();
    assert_eq!(store.save(&original).unwrap().generation, 1);
    let manifest_before = fs::read(directory.path().join("manifest.json")).unwrap();
    let generation_before = fs::read(directory.path().join("snapshot-1.json")).unwrap();
    let mut app = build_app(&ServerConfig::default(), &original).unwrap();
    let entity = app
        .world_mut()
        .query::<(Entity, &DomainId)>()
        .iter(app.world())
        .find_map(|(entity, id)| (id.0 == 42).then_some(entity))
        .unwrap();
    app.world_mut().entity_mut(entity).remove::<Position>();
    app.world_mut().resource_mut::<SimulationClock>().tick = 101;
    // Exercise the same capture-before-save boundary as the native runner.
    let failed_save = capture(app.world_mut()).and_then(|state| store.save(&state));
    assert!(
        failed_save
            .unwrap_err()
            .to_string()
            .contains("required Position")
    );
    assert_eq!(
        fs::read(directory.path().join("manifest.json")).unwrap(),
        manifest_before
    );
    assert_eq!(
        fs::read(directory.path().join("snapshot-1.json")).unwrap(),
        generation_before
    );
    assert!(!directory.path().join("snapshot-2.json").exists());
    drop(store);
    let mut reopened = SnapshotStore::open(directory.path()).unwrap();
    assert_eq!(reopened.load().unwrap().unwrap().snapshot, original);

    let repaired_position = Position([4.0, 5.0, 6.0]);
    app.world_mut().entity_mut(entity).insert(repaired_position);
    // Deliberately spawn in reverse domain order and include transient state.
    app.world_mut()
        .spawn((DomainId(90), Position([9.0, 0.0, 0.0])));
    app.world_mut()
        .spawn((DomainId(7), Position([7.0, 0.0, 0.0])));
    app.world_mut().spawn(Position([f32::NAN, 0.0, 0.0]));
    let expected = WorldSnapshot {
        tick: 101,
        entities: vec![
            EntityRecord {
                id: DomainId(7),
                position: Position([7.0, 0.0, 0.0]),
            },
            EntityRecord {
                id: DomainId(42),
                position: repaired_position,
            },
            EntityRecord {
                id: DomainId(90),
                position: Position([9.0, 0.0, 0.0]),
            },
        ],
        ..Default::default()
    };
    let captured = capture(app.world_mut()).unwrap();
    assert_eq!(captured, expected);
    assert_eq!(reopened.save(&captured).unwrap().generation, 2);
    drop(reopened);
    assert_eq!(
        SnapshotStore::open(directory.path())
            .unwrap()
            .load()
            .unwrap()
            .unwrap()
            .snapshot,
        expected
    );
}

#[test]
fn incomplete_capture_never_enters_worker_and_repair_uses_same_worker() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = SnapshotStore::open(directory.path()).unwrap();
    let original = snapshot(10);
    store.save(&original).unwrap();
    let manifest_before = fs::read(directory.path().join("manifest.json")).unwrap();
    let mut worker = SaveWorker::spawn(store).unwrap();
    let mut app = build_app(&ServerConfig::default(), &original).unwrap();
    let entity = app
        .world_mut()
        .query::<(Entity, &DomainId)>()
        .iter(app.world())
        .next()
        .unwrap()
        .0;
    app.world_mut().entity_mut(entity).remove::<Position>();
    app.world_mut().resource_mut::<SimulationClock>().tick = 11;
    let submission = capture(app.world_mut()).and_then(|state| worker.submit(state));
    assert!(
        submission
            .unwrap_err()
            .to_string()
            .contains("required Position")
    );
    assert!(!worker.is_busy());
    assert!(worker.poll().unwrap().is_none());
    assert_eq!(
        fs::read(directory.path().join("manifest.json")).unwrap(),
        manifest_before
    );
    assert!(!directory.path().join("snapshot-2.json").exists());

    app.world_mut()
        .entity_mut(entity)
        .insert(original.entities[0].position);
    let repaired = snapshot(11);
    capture(app.world_mut())
        .and_then(|state| worker.submit(state))
        .unwrap();
    assert!(worker.is_busy());
    let receipt = worker
        .finish_before(Instant::now() + Duration::from_secs(5))
        .unwrap()
        .unwrap();
    assert_eq!(receipt.tick, 11);
    assert_eq!(receipt.generation, 2);
    assert!(!worker.is_busy());
    drop(worker);
    // Dropping the sender lets its owned writer exit; wait only for that lock.
    let deadline = Instant::now() + Duration::from_secs(5);
    let reopened = loop {
        match SnapshotStore::open(directory.path()) {
            Ok(store) => break store,
            Err(error) => {
                assert!(
                    error
                        .downcast_ref::<std::io::Error>()
                        .is_some_and(|error| error.kind() == std::io::ErrorKind::WouldBlock,)
                );
                assert!(
                    Instant::now() < deadline,
                    "writer did not release data lock"
                );
                std::thread::sleep(Duration::from_millis(1));
            }
        }
    };
    assert_eq!(reopened.load().unwrap().unwrap().snapshot, repaired);
}

#[test]
fn incomplete_duplicate_domain_is_rejected_instead_of_hidden() {
    let mut app = build_app(&ServerConfig::default(), &snapshot(0)).unwrap();
    app.world_mut().spawn(DomainId(42));
    assert!(
        capture(app.world_mut())
            .unwrap_err()
            .to_string()
            .contains("required Position")
    );
    // Even a complete duplicate remains invalid under the existing schema.
    let missing = app
        .world_mut()
        .query_filtered::<Entity, (With<DomainId>, Without<Position>)>()
        .iter(app.world())
        .next()
        .unwrap();
    app.world_mut()
        .entity_mut(missing)
        .insert(Position([1.0, 2.0, 3.0]));
    assert!(
        capture(app.world_mut())
            .unwrap_err()
            .to_string()
            .contains("duplicate domain ID")
    );
}

#[test]
fn capture_ignores_entities_without_domain_identity() {
    let mut app = build_app(&ServerConfig::default(), &snapshot(0)).unwrap();
    app.world_mut()
        .spawn(Position([f32::NAN, f32::INFINITY, 0.0]));
    app.world_mut().spawn_empty();
    assert_eq!(capture(app.world_mut()).unwrap(), snapshot(0));
}

#[test]
fn snapshots_round_trip_and_keep_only_two_committed_generations() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = SnapshotStore::open(directory.path()).unwrap();
    assert!(store.load().unwrap().is_none());
    for tick in 1..=4 {
        assert_eq!(store.save(&snapshot(tick)).unwrap().generation, tick);
    }
    drop(store);
    let store = SnapshotStore::open(directory.path()).unwrap();
    assert_eq!(store.load().unwrap().unwrap().snapshot, snapshot(4));
    assert!(!directory.path().join("snapshot-1.json").exists());
    assert!(!directory.path().join("snapshot-2.json").exists());
    assert!(directory.path().join("snapshot-3.json").exists());
    assert!(directory.path().join("snapshot-4.json").exists());
}

#[test]
fn save_new_publishes_once_and_refuses_existing_or_recovered_worlds() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let mut store = SnapshotStore::open(root).unwrap();
    let receipt = store.save_new(&snapshot(10)).unwrap();
    assert_eq!(receipt.generation, 1);
    assert_eq!(receipt.tick, 10);
    assert_eq!(store.load().unwrap().unwrap().snapshot, snapshot(10));
    let manifest = fs::read(root.join("manifest.json")).unwrap();
    assert!(store.save_new(&snapshot(20)).is_err());
    assert_eq!(fs::read(root.join("manifest.json")).unwrap(), manifest);
    assert!(!root.join("snapshot-2.json").exists());

    store.save(&snapshot(20)).unwrap();
    fs::write(root.join("snapshot-2.json"), b"damaged").unwrap();
    let manifest = fs::read(root.join("manifest.json")).unwrap();
    drop(store);
    let mut store = SnapshotStore::open(root).unwrap();
    assert!(store.load().unwrap().unwrap().recovered_previous);
    assert!(store.save_new(&snapshot(30)).is_err());
    assert_eq!(fs::read(root.join("manifest.json")).unwrap(), manifest);
    assert_eq!(fs::read(root.join("snapshot-2.json")).unwrap(), b"damaged");
    assert!(!root.join("snapshot-3.json").exists());
}

#[test]
fn save_new_refuses_orphan_generations_and_malformed_stores() {
    for malformed_manifest in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        fs::write(root.join("snapshot-1.json"), b"orphan").unwrap();
        if malformed_manifest {
            fs::write(root.join("manifest.json"), b"malformed").unwrap();
        }
        let mut store = SnapshotStore::open(root).unwrap();
        assert!(store.save_new(&snapshot(10)).is_err());
        assert_eq!(fs::read(root.join("snapshot-1.json")).unwrap(), b"orphan");
        if malformed_manifest {
            assert_eq!(fs::read(root.join("manifest.json")).unwrap(), b"malformed");
        } else {
            assert!(!root.join("manifest.json").exists());
        }
    }
}

#[test]
fn data_directory_has_a_single_writer() {
    let directory = tempfile::tempdir().unwrap();
    let store = SnapshotStore::open(directory.path()).unwrap();
    assert!(SnapshotStore::open(directory.path()).is_err());
    drop(store);
    assert!(SnapshotStore::open(directory.path()).is_ok());
}

#[test]
fn corrupt_current_recovers_previous_and_next_save_retains_valid_fallback() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = SnapshotStore::open(directory.path()).unwrap();
    store.save(&snapshot(10)).unwrap();
    store.save(&snapshot(20)).unwrap();
    fs::write(directory.path().join("snapshot-2.json"), b"partial").unwrap();
    let recovered = store.load().unwrap().unwrap();
    assert!(recovered.recovered_previous);
    assert_eq!(recovered.snapshot, snapshot(10));
    store.save(&snapshot(30)).unwrap();
    fs::write(directory.path().join("snapshot-3.json"), b"partial").unwrap();
    assert_eq!(store.load().unwrap().unwrap().snapshot, snapshot(10));
}

#[test]
fn unreadable_current_preserves_acknowledged_state_until_permissions_are_repaired() {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    struct RestorePermissions {
        path: std::path::PathBuf,
        permissions: fs::Permissions,
    }

    impl RestorePermissions {
        fn restore(&self) {
            fs::set_permissions(&self.path, self.permissions.clone()).unwrap();
        }
    }

    impl Drop for RestorePermissions {
        fn drop(&mut self) {
            let _ = fs::set_permissions(&self.path, self.permissions.clone());
        }
    }

    let directory = tempfile::tempdir().unwrap();
    let mut store = SnapshotStore::open(directory.path()).unwrap();
    let mut previous = snapshot(10);
    previous.entities[0].position = Position([10.0, 0.0, 0.0]);
    let mut current = snapshot(20);
    current.entities[0].position = Position([20.0, 0.0, 0.0]);
    store.save(&previous).unwrap();
    store.save(&current).unwrap();
    let manifest_path = directory.path().join("manifest.json");
    let previous_path = directory.path().join("snapshot-1.json");
    let current_path = directory.path().join("snapshot-2.json");
    let manifest_bytes = fs::read(&manifest_path).unwrap();
    let previous_bytes = fs::read(&previous_path).unwrap();
    let current_bytes = fs::read(&current_path).unwrap();
    let current_metadata = fs::metadata(&current_path).unwrap();
    let restore = RestorePermissions {
        path: current_path.clone(),
        permissions: current_metadata.permissions(),
    };
    fs::set_permissions(&current_path, fs::Permissions::from_mode(0o0)).unwrap();
    match fs::read(&current_path) {
        Err(error) => assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied),
        Ok(_) => {
            // A privileged process can bypass mode000. The deterministic reader
            // tests still exercise terminal read faults under that account.
            eprintln!("mode000 read fault unavailable to this privileged test account");
            return;
        }
    }
    for error in [
        store.load().unwrap_err(),
        store.save(&snapshot(30)).unwrap_err(),
    ] {
        assert_eq!(
            error.downcast_ref::<std::io::Error>().unwrap().kind(),
            std::io::ErrorKind::PermissionDenied
        );
    }
    assert_eq!(fs::read(&manifest_path).unwrap(), manifest_bytes);
    assert_eq!(fs::read(&previous_path).unwrap(), previous_bytes);
    assert!(!directory.path().join("snapshot-3.json").exists());
    let unreadable_metadata = fs::metadata(&current_path).unwrap();
    assert_eq!(unreadable_metadata.ino(), current_metadata.ino());
    assert_eq!(unreadable_metadata.len(), current_metadata.len());
    assert_eq!(unreadable_metadata.permissions().mode() & 0o777, 0);
    restore.restore();
    assert_eq!(fs::read(&current_path).unwrap(), current_bytes);
    let loaded = store.load().unwrap().unwrap();
    assert!(!loaded.recovered_previous);
    assert_eq!(loaded.snapshot, current);
    assert_eq!(store.save(&snapshot(30)).unwrap().generation, 3);
}

#[test]
fn missing_current_still_recovers_and_retains_the_previous_generation() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = SnapshotStore::open(directory.path()).unwrap();
    store.save(&snapshot(10)).unwrap();
    store.save(&snapshot(20)).unwrap();
    fs::remove_file(directory.path().join("snapshot-2.json")).unwrap();
    let loaded = store.load().unwrap().unwrap();
    assert!(loaded.recovered_previous);
    assert_eq!(loaded.snapshot, snapshot(10));
    assert_eq!(store.save(&snapshot(30)).unwrap().generation, 3);
    fs::write(directory.path().join("snapshot-3.json"), b"corrupt").unwrap();
    assert_eq!(store.load().unwrap().unwrap().snapshot, snapshot(10));
}

#[test]
fn interrupted_uncommitted_generation_does_not_replace_saved_state() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = SnapshotStore::open(directory.path()).unwrap();
    store.save(&snapshot(10)).unwrap();
    fs::write(directory.path().join("snapshot-2.json"), b"uncommitted").unwrap();
    assert_eq!(store.load().unwrap().unwrap().snapshot, snapshot(10));
    store.save(&snapshot(20)).unwrap();
    assert_eq!(store.load().unwrap().unwrap().snapshot, snapshot(20));
}

#[test]
fn failed_publication_keeps_the_committed_world() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = SnapshotStore::open(directory.path()).unwrap();
    store.save(&snapshot(10)).unwrap();
    fs::create_dir(directory.path().join("snapshot-2.json")).unwrap();
    assert!(store.save(&snapshot(20)).is_err());
    assert_eq!(store.load().unwrap().unwrap().snapshot, snapshot(10));
}

#[test]
fn corrupt_or_missing_manifest_never_becomes_an_empty_world() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = SnapshotStore::open(directory.path()).unwrap();
    store.save(&snapshot(10)).unwrap();
    let path = directory.path().join("manifest.json");
    fs::write(&path, b"broken").unwrap();
    assert!(store.load().is_err());
    fs::remove_file(path).unwrap();
    assert!(store.load().is_err());
}

#[test]
fn both_corrupt_generations_fail_without_reset() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = SnapshotStore::open(directory.path()).unwrap();
    store.save(&snapshot(10)).unwrap();
    store.save(&snapshot(20)).unwrap();
    for generation in 1..=2 {
        fs::write(
            directory.path().join(format!("snapshot-{generation}.json")),
            b"broken",
        )
        .unwrap();
    }
    assert!(store.load().is_err());
    assert!(store.save(&snapshot(30)).is_err());
}

#[test]
fn malformed_manifest_checksums_fail_before_recovery_or_publication() {
    for generation in ["current", "previous"] {
        for digest in [
            String::new(),
            "a".repeat(63),
            "a".repeat(65),
            "g".repeat(64),
            "A".repeat(64),
            "é".repeat(32),
        ] {
            let directory = tempfile::tempdir().unwrap();
            let mut store = SnapshotStore::open(directory.path()).unwrap();
            store.save(&snapshot(10)).unwrap();
            store.save(&snapshot(20)).unwrap();
            let manifest_path = directory.path().join("manifest.json");
            let mut manifest: serde_json::Value =
                serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
            manifest[generation]["sha256"] = digest.into();
            let manifest_bytes = serde_json::to_vec(&manifest).unwrap();
            fs::write(&manifest_path, &manifest_bytes).unwrap();
            let snapshots = [1, 2].map(|number| {
                fs::read(directory.path().join(format!("snapshot-{number}.json"))).unwrap()
            });
            assert!(
                store.load().is_err(),
                "malformed {generation} checksum admitted"
            );
            assert!(store.save(&snapshot(30)).is_err());
            assert_eq!(fs::read(&manifest_path).unwrap(), manifest_bytes);
            for (index, bytes) in snapshots.iter().enumerate() {
                assert_eq!(
                    fs::read(
                        directory
                            .path()
                            .join(format!("snapshot-{}.json", index + 1))
                    )
                    .unwrap(),
                    *bytes
                );
            }
            assert!(!directory.path().join("snapshot-3.json").exists());
        }
    }
}

#[test]
fn well_formed_checksum_mismatch_still_recovers_previous() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = SnapshotStore::open(directory.path()).unwrap();
    store.save(&snapshot(10)).unwrap();
    store.save(&snapshot(20)).unwrap();
    let manifest_path = directory.path().join("manifest.json");
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
    manifest["current"]["sha256"] = "0".repeat(64).into();
    fs::write(manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    let recovered = store.load().unwrap().unwrap();
    assert!(recovered.recovered_previous);
    assert_eq!(recovered.snapshot, snapshot(10));
    assert_eq!(store.save(&snapshot(30)).unwrap().generation, 3);
}

#[test]
fn malformed_first_generation_checksum_fails_at_manifest_admission() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = SnapshotStore::open(directory.path()).unwrap();
    store.save(&snapshot(10)).unwrap();
    let manifest_path = directory.path().join("manifest.json");
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
    manifest["current"]["sha256"] = "".into();
    let manifest_bytes = serde_json::to_vec(&manifest).unwrap();
    fs::write(&manifest_path, &manifest_bytes).unwrap();
    let saved = fs::read(directory.path().join("snapshot-1.json")).unwrap();
    for error in [
        store.load().unwrap_err(),
        store.save(&snapshot(20)).unwrap_err(),
    ] {
        assert!(
            error
                .to_string()
                .contains("invalid snapshot checksum in manifest")
        );
    }
    assert_eq!(fs::read(manifest_path).unwrap(), manifest_bytes);
    assert_eq!(
        fs::read(directory.path().join("snapshot-1.json")).unwrap(),
        saved
    );
    assert!(!directory.path().join("snapshot-2.json").exists());
}

#[test]
fn future_schema_is_not_silently_rolled_back() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = SnapshotStore::open(directory.path()).unwrap();
    store.save(&snapshot(10)).unwrap();
    store.save(&snapshot(20)).unwrap();
    // Schema 7 holds authored owner lifecycles, so the future fixture uses 8.
    let bytes = br#"{"schema_version":8,"tick":20,"entities":[],"new_field":true}"#;
    fs::write(directory.path().join("snapshot-2.json"), bytes).unwrap();
    let path = directory.path().join("manifest.json");
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    // Independently calculated with coreutils sha256sum over the fixture bytes.
    manifest["current"]["sha256"] =
        "f30a515ae8d79a4af350c455f73f190c69ab17d115a2296b11d435d1dbac635e".into();
    fs::write(path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    assert!(
        store
            .load()
            .unwrap_err()
            .to_string()
            .contains("unsupported")
    );
    assert!(store.save(&snapshot(30)).is_err());
}

#[test]
fn ambiguous_duplicate_fields_recover_previous_instead_of_selecting_a_value() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = SnapshotStore::open(directory.path()).unwrap();
    store.save(&snapshot(10)).unwrap();
    store.save(&snapshot(20)).unwrap();
    let bytes = br#"{"schema_version":1,"tick":20,"tick":21,"entities":[]}"#;
    fs::write(directory.path().join("snapshot-2.json"), bytes).unwrap();
    let path = directory.path().join("manifest.json");
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    // Independently calculated with Python hashlib.sha256 over the fixture bytes.
    manifest["current"]["sha256"] =
        "527dc97b422859e4a021519b7a031130a1c45ffde3e9bcb86344d8e643e6d576".into();
    fs::write(path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    let loaded = store.load().unwrap().unwrap();
    assert!(loaded.recovered_previous);
    assert_eq!(loaded.snapshot.tick, 10);
}

#[test]
fn worker_backpressure_and_durable_completion() {
    let directory = tempfile::tempdir().unwrap();
    let store = SnapshotStore::open(directory.path()).unwrap();
    let mut worker = SaveWorker::spawn(store).unwrap();
    worker.submit(snapshot(99)).unwrap();
    assert!(worker.is_busy());
    assert!(worker.submit(snapshot(100)).is_err());
    let receipt = worker
        .finish_before(Instant::now() + Duration::from_secs(5))
        .unwrap()
        .unwrap();
    assert_eq!(receipt.tick, 99);
    assert!(!worker.is_busy());
    assert!(directory.path().join("manifest.json").exists());
}

#[test]
fn worker_reports_write_failure_and_can_retry_without_false_receipt() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = SnapshotStore::open(directory.path()).unwrap();
    store.save(&snapshot(10)).unwrap();
    let obstruction = directory.path().join("snapshot-2.json");
    fs::create_dir(&obstruction).unwrap();
    let mut worker = SaveWorker::spawn(store).unwrap();
    worker.submit(snapshot(20)).unwrap();
    assert!(
        worker
            .finish_before(Instant::now() + Duration::from_secs(5))
            .is_err()
    );
    assert!(!worker.is_busy());
    assert!(worker.poll().unwrap().is_none());
    fs::remove_dir(obstruction).unwrap();
    worker.submit(snapshot(30)).unwrap();
    let receipt = worker
        .finish_before(Instant::now() + Duration::from_secs(5))
        .unwrap()
        .unwrap();
    assert_eq!(receipt.generation, 2);
    assert_eq!(receipt.tick, 30);
    let manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(directory.path().join("manifest.json")).unwrap()).unwrap();
    assert_eq!(manifest["previous"]["number"], 1);
}

#[test]
fn compressed_run_clock_counts_exact_simulated_time() {
    let clock = RunClock {
        compressed: true,
        start: Instant::now(),
        start_tick: 100,
        tick_hz: 30,
    };
    assert_eq!(clock.elapsed(100), Duration::ZERO);
    // 30 Hz has no exact nanosecond interval; summing a truncated one would drift.
    assert_eq!(clock.elapsed(101), Duration::from_nanos(33_333_333));
    assert_eq!(
        clock.elapsed(100 + 30 * 259_200),
        Duration::from_secs(259_200)
    );
    assert_eq!(clock.elapsed(u64::MAX), Duration::from_nanos(u64::MAX));
}

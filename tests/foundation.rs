use std::{
    fs,
    time::{Duration, Instant},
};

use bevy::{prelude::*, time::TimeUpdateStrategy};
use icarus_bevy::{
    config::{ConfigOptions, ServerConfig},
    persistence::{SaveWorker, SnapshotStore},
    simulation::{
        DomainId, EntityRecord, Position, SimulationClock, WorldSnapshot, build_app, capture,
    },
};
use sha2::{Digest, Sha256};

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
fn future_schema_is_not_silently_rolled_back() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = SnapshotStore::open(directory.path()).unwrap();
    store.save(&snapshot(10)).unwrap();
    store.save(&snapshot(20)).unwrap();
    let bytes = br#"{"schema_version":2,"tick":20,"entities":[],"new_field":true}"#;
    fs::write(directory.path().join("snapshot-2.json"), bytes).unwrap();
    let path = directory.path().join("manifest.json");
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    manifest["current"]["sha256"] = format!("{:x}", Sha256::digest(bytes)).into();
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
    manifest["current"]["sha256"] = format!("{:x}", Sha256::digest(bytes)).into();
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

use std::{
    fs,
    path::Path,
    process::{Child, Command, Output, Stdio},
    sync::Mutex,
    thread,
    time::{Duration, Instant},
};

use icarus_bevy::{
    persistence::SnapshotStore,
    simulation::{DomainId, EntityRecord, Position, WorldSnapshot},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

// A concurrent fork can temporarily inherit another fixture's advisory lock
// until exec closes its descriptors. Keep this file's child launches and
// store-opening fixtures sequential, even though their directories differ.
static FIXTURE_LOCK: Mutex<()> = Mutex::new(());

fn initial(two_generations: bool) -> tempfile::TempDir {
    let directory = tempfile::tempdir().unwrap();
    let mut store = SnapshotStore::open(directory.path()).unwrap();
    for tick in if two_generations {
        &[10, 20][..]
    } else {
        &[10][..]
    } {
        store
            .save(&WorldSnapshot {
                tick: *tick,
                entities: vec![EntityRecord {
                    id: DomainId(1),
                    position: Position([1.0, 2.0, 3.0]),
                }],
                ..Default::default()
            })
            .unwrap();
    }
    directory
}

fn manifest(root: &Path) -> Value {
    serde_json::from_slice(&fs::read(root.join("manifest.json")).unwrap()).unwrap()
}

fn write_manifest(root: &Path, value: &Value) {
    fs::write(
        root.join("manifest.json"),
        serde_json::to_vec(value).unwrap(),
    )
    .unwrap();
}

fn replace_current(root: &Path, value: &Value) {
    let mut metadata = manifest(root);
    let bytes = serde_json::to_vec(value).unwrap();
    let checksum = Sha256::digest(&bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let number = metadata["current"]["number"].as_u64().unwrap();
    fs::write(root.join(format!("snapshot-{number}.json")), bytes).unwrap();
    metadata["current"]["sha256"] = checksum.into();
    write_manifest(root, &metadata);
}

fn inventory(root: &Path) -> Vec<(std::path::PathBuf, Vec<u8>)> {
    let mut files = fs::read_dir(root)
        .unwrap()
        .map(|entry| {
            let path = entry.unwrap().path();
            let bytes = fs::read(&path).unwrap();
            (path, bytes)
        })
        .collect::<Vec<_>>();
    files.sort_by(|left, right| left.0.cmp(&right.0));
    files
}

struct OwnedChild(Option<Child>);

impl Drop for OwnedChild {
    fn drop(&mut self) {
        if let Some(child) = &mut self.0 {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn run_bounded(root: &Path) -> Output {
    let mut child = OwnedChild(Some(
        Command::new(env!("CARGO_BIN_EXE_icarus-server"))
            .args(["run", "--max-ticks", "1", "--data-dir"])
            .arg(root)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    ));
    let deadline = Instant::now() + Duration::from_secs(5);
    while child.0.as_mut().unwrap().try_wait().unwrap().is_none() {
        assert!(Instant::now() < deadline, "startup exceeded test deadline");
        thread::sleep(Duration::from_millis(1));
    }
    child.0.take().unwrap().wait_with_output().unwrap()
}

#[test]
fn positional_native_metadata_and_records_fail_without_state_writes() {
    let _fixture = FIXTURE_LOCK.lock().unwrap();
    for case in 0..5 {
        let directory = initial(case == 2 || case == 3);
        let root = directory.path();
        let mut metadata = manifest(root);
        match case {
            0 => metadata = json!([1, metadata["current"], metadata["previous"]]),
            1 | 3 => {
                metadata["current"] =
                    json!([metadata["current"]["number"], metadata["current"]["sha256"]]);
            }
            2 => {
                metadata["previous"] = json!([
                    metadata["previous"]["number"],
                    metadata["previous"]["sha256"]
                ]);
            }
            4 => replace_current(
                root,
                &json!({"schema_version":1,"tick":10,"entities":[[1,[1,2,3]]]}),
            ),
            _ => unreachable!(),
        }
        if case != 4 {
            write_manifest(root, &metadata);
        }
        let before = inventory(root);
        let mut store = SnapshotStore::open(root).unwrap();
        for error in [
            store.load().unwrap_err(),
            store.save(&WorldSnapshot::default()).unwrap_err(),
        ] {
            let message = format!("{error:#}");
            assert!(message.contains("Data at line "), "{message}");
            assert!(!message.contains("unsupported native snapshot schema"));
        }
        drop(store);
        let result = run_bounded(root);
        assert_eq!(result.status.code(), Some(1));
        assert!(result.stdout.is_empty());
        let fatal: Value = serde_json::from_slice(&result.stderr).unwrap();
        assert_eq!(fatal["event"], "fatal");
        assert!(fatal["error"].as_str().unwrap().contains("Data at line "));
        assert_eq!(inventory(root), before);
    }
}

#[test]
fn malformed_array_headers_recover_previous_instead_of_claiming_future_schema() {
    let _fixture = FIXTURE_LOCK.lock().unwrap();
    for value in [
        json!([]),
        json!([1]),
        json!([2]),
        json!([[2]]),
        json!([1, 20, []]),
    ] {
        let directory = initial(true);
        let root = directory.path();
        replace_current(root, &value);
        let before = inventory(root);
        let store = SnapshotStore::open(root).unwrap();
        let loaded = store.load().unwrap().unwrap();
        assert!(loaded.recovered_previous);
        assert_eq!(loaded.snapshot.tick, 10);
        assert_eq!(inventory(root), before);
        drop(store);
        let result = run_bounded(root);
        assert!(result.status.success(), "{:?}", result.stderr);
        let events = String::from_utf8(result.stderr).unwrap();
        let events = events
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .collect::<Vec<_>>();
        assert!(
            events
                .iter()
                .any(|event| event["event"] == "recovered_previous_generation")
        );
        assert!(
            events
                .iter()
                .any(|event| event["event"] == "started" && event["tick"] == 10)
        );
        let after = manifest(root);
        assert_eq!(after["current"]["number"], 3);
        assert_eq!(after["previous"]["number"], 1);
        let reopened = SnapshotStore::open(root).unwrap().load().unwrap().unwrap();
        assert!(!reopened.recovered_previous);
        let acknowledged = events
            .iter()
            .find(|event| event["event"] == "save_committed")
            .unwrap();
        assert_eq!(acknowledged["generation"], 3);
        assert_eq!(acknowledged["tick"], reopened.snapshot.tick);
        // A Bevy update can execute more than one catch-up tick before the
        // runner observes max_ticks; the restored starting tick is exact.
        assert!(reopened.snapshot.tick >= 11);
        assert_eq!(reopened.snapshot.entities, loaded.snapshot.entities);
    }
}

#[test]
fn proper_future_schema_objects_still_refuse_rollback_without_writes() {
    let _fixture = FIXTURE_LOCK.lock().unwrap();
    let directory = initial(true);
    let root = directory.path();
    // Schemas 2 and 3 are the defined preserved-section schemas; 4 is the future one.
    replace_current(root, &json!({"schema_version":4,"tick":20,"entities":[]}));
    let before = inventory(root);
    let mut store = SnapshotStore::open(root).unwrap();
    for error in [
        store.load().unwrap_err(),
        store.save(&WorldSnapshot::default()).unwrap_err(),
    ] {
        assert!(format!("{error:#}").contains("unsupported native snapshot schema"));
    }
    drop(store);
    let result = run_bounded(root);
    assert_eq!(result.status.code(), Some(1));
    assert_eq!(inventory(root), before);
}

#[test]
fn named_objects_position_arrays_and_optional_previous_remain_compatible() {
    let _fixture = FIXTURE_LOCK.lock().unwrap();
    for omit_previous in [false, true] {
        let directory = initial(false);
        let root = directory.path();
        let mut metadata = manifest(root);
        if omit_previous {
            metadata.as_object_mut().unwrap().remove("previous");
            write_manifest(root, &metadata);
        }
        let mut store = SnapshotStore::open(root).unwrap();
        let loaded = store.load().unwrap().unwrap();
        assert!(!loaded.recovered_previous);
        assert_eq!(
            loaded.snapshot.entities[0].position,
            Position([1.0, 2.0, 3.0])
        );
        store.save(&loaded.snapshot).unwrap();
        let serialized = serde_json::to_value(&loaded.snapshot).unwrap();
        assert!(serialized.is_object());
        assert!(serialized["entities"][0].is_object());
        assert_eq!(
            serialized["entities"][0]["position"],
            json!([1.0, 2.0, 3.0])
        );
    }
}

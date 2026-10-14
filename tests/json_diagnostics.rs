use std::{
    fs,
    process::{Child, Command, Output, Stdio},
    thread,
    time::{Duration, Instant},
};

use icarus_bevy::{persistence::SnapshotStore, simulation::WorldSnapshot};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

enum Malformed {
    ManifestType,
    ManifestKey,
    GenerationKey,
    SnapshotRoot,
    SnapshotTick,
    SnapshotKey,
    EntityKey,
    BothSnapshots,
}

fn write_snapshot(root: &std::path::Path, manifest: &mut Value, reference: &str, value: &Value) {
    let bytes = serde_json::to_vec(value).unwrap();
    let checksum = Sha256::digest(&bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let number = manifest[reference]["number"].as_u64().unwrap();
    fs::write(root.join(format!("snapshot-{number}.json")), bytes).unwrap();
    manifest[reference]["sha256"] = checksum.into();
}

fn malformed(case: &Malformed) -> (tempfile::TempDir, &'static str) {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let mut store = SnapshotStore::open(root).unwrap();
    store
        .save(&WorldSnapshot {
            tick: 10,
            ..Default::default()
        })
        .unwrap();
    if matches!(case, Malformed::BothSnapshots) {
        store
            .save(&WorldSnapshot {
                tick: 20,
                ..Default::default()
            })
            .unwrap();
    }
    drop(store);
    let mut manifest: Value =
        serde_json::from_slice(&fs::read(root.join("manifest.json")).unwrap()).unwrap();
    let mut state = json!({"schema_version":1,"tick":10,"entities":[]});
    let context = match case {
        Malformed::ManifestType => {
            manifest["schema_version"] = "AUTHORED_MANIFEST_VALUE_MARKER".into();
            "invalid snapshot manifest JSON"
        }
        Malformed::ManifestKey => {
            manifest["AUTHORED_MANIFEST_KEY_MARKER"] = true.into();
            "invalid snapshot manifest JSON"
        }
        Malformed::GenerationKey => {
            manifest["current"]["AUTHORED_GENERATION_KEY_MARKER"] = true.into();
            "invalid snapshot manifest JSON"
        }
        Malformed::SnapshotRoot => {
            write_snapshot(
                root,
                &mut manifest,
                "current",
                &json!("AUTHORED_HEADER_MARKER"),
            );
            "decode native snapshot JSON"
        }
        Malformed::SnapshotTick => {
            state["tick"] = "AUTHORED_CURRENT_TICK_MARKER".into();
            write_snapshot(root, &mut manifest, "current", &state);
            "decode native snapshot fields JSON"
        }
        Malformed::SnapshotKey => {
            state["AUTHORED_SNAPSHOT_KEY_MARKER"] = true.into();
            write_snapshot(root, &mut manifest, "current", &state);
            "decode native snapshot fields JSON"
        }
        Malformed::EntityKey => {
            state["entities"] = json!([{
                "id":1,"position":[0,0,0],"AUTHORED_ENTITY_KEY_MARKER":true
            }]);
            write_snapshot(root, &mut manifest, "current", &state);
            "decode native snapshot fields JSON"
        }
        Malformed::BothSnapshots => {
            state["tick"] = "AUTHORED_CURRENT_TICK_MARKER".into();
            write_snapshot(root, &mut manifest, "current", &state);
            state["tick"] = "AUTHORED_PREVIOUS_TICK_MARKER".into();
            write_snapshot(root, &mut manifest, "previous", &state);
            "decode native snapshot fields JSON"
        }
    };
    fs::write(
        root.join("manifest.json"),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();
    (directory, context)
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

fn run_bounded(root: &std::path::Path) -> Output {
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
        assert!(Instant::now() < deadline, "malformed state did not exit");
        thread::sleep(Duration::from_millis(1));
    }
    child.0.take().unwrap().wait_with_output().unwrap()
}

#[test]
fn native_json_errors_keep_values_and_keys_private_without_writing_state() {
    for case in [
        Malformed::ManifestType,
        Malformed::ManifestKey,
        Malformed::GenerationKey,
        Malformed::SnapshotRoot,
        Malformed::SnapshotTick,
        Malformed::SnapshotKey,
        Malformed::EntityKey,
        Malformed::BothSnapshots,
    ] {
        let (directory, context) = malformed(&case);
        let root = directory.path();
        let original = fs::read_dir(root)
            .unwrap()
            .map(|entry| {
                let path = entry.unwrap().path();
                let bytes = fs::read(&path).unwrap();
                (path, bytes)
            })
            .collect::<Vec<_>>();
        let mut store = SnapshotStore::open(root).unwrap();
        for error in [
            store.load().unwrap_err(),
            store
                .save(&WorldSnapshot {
                    tick: 30,
                    ..Default::default()
                })
                .unwrap_err(),
        ] {
            let message = format!("{error:#}");
            assert!(!message.contains("_MARKER"), "{message}");
            assert!(message.contains(context), "{message}");
            assert!(message.contains("Data at line "), "{message}");
            assert!(message.contains("column "), "{message}");
        }
        drop(store);
        let output = run_bounded(root);
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(!stderr.contains("_MARKER"), "{stderr}");
        let fatal: Value = serde_json::from_str(stderr.trim()).unwrap();
        assert_eq!(fatal["event"], "fatal");
        assert!(fatal["error"].as_str().unwrap().contains(context));
        for (path, bytes) in &original {
            assert_eq!(fs::read(path).unwrap(), *bytes);
        }
        assert_eq!(fs::read_dir(root).unwrap().count(), original.len());
    }
}

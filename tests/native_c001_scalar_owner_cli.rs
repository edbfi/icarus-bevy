//! Bounded subprocess tests using wholly authored C001 bytes.
#[path = "common/c001_scalar_owners.rs"]
mod fixture;

use icarus_bevy::{persistence::SnapshotStore, simulation::WorldSnapshot};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    io::{Read, Seek},
    path::Path,
    process::{Command, Output, Stdio},
    sync::Mutex,
    time::{Duration, Instant},
};

// Serialize store owners and subprocess launch: a sibling fork can otherwise
// briefly inherit a fixture lock before exec closes its descriptor.
static FIXTURE_LOCK: Mutex<()> = Mutex::new(());

fn inputs(root: &Path) -> fixture::Fixture {
    let fixture = fixture::fixture();
    fs::write(root.join("source.json"), &fixture.envelope).unwrap();
    fs::write(
        root.join("catalog.json"),
        serde_json::to_vec(fixture.catalog.definition()).unwrap(),
    )
    .unwrap();
    fs::write(
        root.join("bindings.json"),
        serde_json::to_vec(fixture.bindings.definition()).unwrap(),
    )
    .unwrap();
    fs::write(
        root.join("contracts.json"),
        serde_json::to_vec(fixture.contracts.definition()).unwrap(),
    )
    .unwrap();
    fs::write(
        root.join("lifecycle.json"),
        serde_json::to_vec(&fixture.base.native_lifecycle.as_ref().unwrap().definition).unwrap(),
    )
    .unwrap();
    fs::write(
        root.join("relationships.json"),
        serde_json::to_vec(
            &fixture
                .base
                .native_relationships
                .as_ref()
                .unwrap()
                .definition,
        )
        .unwrap(),
    )
    .unwrap();
    fixture
}

fn execute(command: &mut Command) -> Output {
    let mut stdout = tempfile::tempfile().unwrap();
    let mut stderr = tempfile::tempfile().unwrap();
    let mut child = command
        .stdout(Stdio::from(stdout.try_clone().unwrap()))
        .stderr(Stdio::from(stderr.try_clone().unwrap()))
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("authored CLI deadline exceeded");
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    stdout.rewind().unwrap();
    stderr.rewind().unwrap();
    let mut out = vec![];
    let mut err = vec![];
    stdout.read_to_end(&mut out).unwrap();
    stderr.read_to_end(&mut err).unwrap();
    Output {
        status,
        stdout: out,
        stderr: err,
    }
}

fn command(root: &Path, dry_run: bool) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_icarus-server"));
    command.arg("create-c001-scalar-owner-fixture");
    for name in [
        "source",
        "catalog",
        "bindings",
        "lifecycle",
        "contracts",
        "relationships",
    ] {
        command
            .arg(format!("--{name}"))
            .arg(root.join(format!("{name}.json")));
    }
    command.arg("--data-dir").arg(root.join("world"));
    if dry_run {
        command.arg("--dry-run");
    }
    command
}

fn files(path: &Path) -> BTreeMap<String, Vec<u8>> {
    fs::read_dir(path)
        .unwrap()
        .map(|entry| {
            let entry = entry.unwrap();
            (
                entry.file_name().into_string().unwrap(),
                fs::read(entry.path()).unwrap(),
            )
        })
        .collect()
}

fn private_safe(output: &Output) {
    for value in [
        "SourceCrate",
        "/Authored/Owner",
        "SourceObject",
        "SpawnOwner",
        "FindOwner",
        "ReplaceOwner",
        "Authored owner fixture",
    ] {
        assert!(!String::from_utf8_lossy(&output.stdout).contains(value));
        assert!(!String::from_utf8_lossy(&output.stderr).contains(value));
    }
}

#[test]
fn creation_dry_run_and_autosave_keep_source_receipt_without_source_files() {
    let _guard = FIXTURE_LOCK.lock().unwrap();
    let root = tempfile::tempdir().unwrap();
    let fixture = inputs(root.path());
    let expected = fixture.snapshot();
    let before = files(root.path());
    let dry = execute(&mut command(root.path(), true));
    assert!(
        dry.status.success(),
        "{}",
        String::from_utf8_lossy(&dry.stderr)
    );
    private_safe(&dry);
    assert_eq!(files(root.path()), before);
    let created = execute(&mut command(root.path(), false));
    assert!(
        created.status.success(),
        "{}",
        String::from_utf8_lossy(&created.stderr)
    );
    private_safe(&created);
    let report: Value = serde_json::from_slice(&created.stdout).unwrap();
    assert_eq!(report["schema_version"], 9);
    assert_eq!(report["world_imported"], false);
    assert_eq!(report["gameplay_ready"], false);
    let stored = SnapshotStore::open(&root.path().join("world"))
        .unwrap()
        .load()
        .unwrap()
        .unwrap()
        .snapshot;
    assert_eq!(stored, expected);
    for name in [
        "source",
        "catalog",
        "bindings",
        "lifecycle",
        "contracts",
        "relationships",
    ] {
        fs::remove_file(root.path().join(format!("{name}.json"))).unwrap();
    }
    let run = execute(
        Command::new(env!("CARGO_BIN_EXE_icarus-server"))
            .arg("run")
            .arg("--data-dir")
            .arg(root.path().join("world"))
            .args(["--compressed", "--max-ticks", "3601"]),
    );
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    private_safe(&run);
    let loaded = SnapshotStore::open(&root.path().join("world"))
        .unwrap()
        .load()
        .unwrap()
        .unwrap()
        .snapshot;
    assert_eq!(loaded.tick, 3601);
    assert_eq!(
        loaded,
        WorldSnapshot {
            tick: 3601,
            ..expected
        }
    );
    let manifest: Value =
        serde_json::from_slice(&fs::read(root.path().join("world/manifest.json")).unwrap())
            .unwrap();
    assert!(manifest["current"]["number"].as_u64().unwrap() >= 3);
}

#[test]
fn invalid_final_binding_never_touches_absent_or_existing_destination() {
    let _guard = FIXTURE_LOCK.lock().unwrap();
    for existing in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let fixture = inputs(root.path());
        let mut bindings = fixture.bindings.definition().clone();
        bindings
            .owners
            .last_mut()
            .unwrap()
            .owner
            .native_recorder_guid = 0;
        fs::write(
            root.path().join("bindings.json"),
            serde_json::to_vec(&bindings).unwrap(),
        )
        .unwrap();
        let destination = root.path().join("world");
        if existing {
            fs::create_dir(&destination).unwrap();
            fs::write(destination.join("sentinel"), b"unchanged").unwrap();
        }
        for dry in [false, true] {
            let output = execute(&mut command(root.path(), dry));
            assert!(!output.status.success());
            private_safe(&output);
            if existing {
                assert_eq!(
                    files(&destination),
                    BTreeMap::from([("sentinel".into(), b"unchanged".to_vec())])
                );
            } else {
                assert!(!destination.exists());
            }
        }
    }
}

#[test]
fn supported_subset_is_never_published_after_last_recorder_rejection() {
    let _guard = FIXTURE_LOCK.lock().unwrap();
    let root = tempfile::tempdir().unwrap();
    let mut fixture = inputs(root.path());
    fixture
        .owners
        .last_mut()
        .unwrap()
        .push(fixture::Tag::integer("UnsupportedOwnerValue", 1));
    fixture.rebuild_source();
    fs::write(root.path().join("source.json"), &fixture.envelope).unwrap();
    fs::write(
        root.path().join("bindings.json"),
        serde_json::to_vec(fixture.bindings.definition()).unwrap(),
    )
    .unwrap();
    for dry in [false, true] {
        let output = execute(&mut command(root.path(), dry));
        assert!(!output.status.success());
        private_safe(&output);
        assert!(!root.path().join("world").exists());
    }
}

#[test]
fn existing_and_recoverable_worlds_are_refused_without_changing_any_file() {
    let _guard = FIXTURE_LOCK.lock().unwrap();
    let root = tempfile::tempdir().unwrap();
    let fixture = inputs(root.path());
    let path = root.path().join("world");
    let mut store = SnapshotStore::open(&path).unwrap();
    let mut initial = fixture.snapshot();
    store.save_new(&initial).unwrap();
    initial.tick = 1;
    store.save(&initial).unwrap();
    drop(store);
    for corrupt in [false, true] {
        if corrupt {
            fs::write(path.join("snapshot-2.json"), b"corrupt").unwrap();
        }
        let before = files(&path);
        let output = execute(&mut command(root.path(), false));
        assert!(!output.status.success());
        private_safe(&output);
        assert_eq!(files(&path), before);
    }
}

#[test]
fn policy_overrides_are_not_cli_or_manifest_options() {
    let _guard = FIXTURE_LOCK.lock().unwrap();
    let root = tempfile::tempdir().unwrap();
    inputs(root.path());
    let output = execute(
        command(root.path(), false)
            .arg("--restore")
            .arg(root.path().join("source.json")),
    );
    assert!(!output.status.success());
    assert!(!root.path().join("world").exists());
    let path = root.path().join("bindings.json");
    let mut bindings: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    bindings["owners"][0]["policy"] = json!("respawn_only");
    fs::write(path, serde_json::to_vec(&bindings).unwrap()).unwrap();
    let output = execute(&mut command(root.path(), false));
    assert!(!output.status.success());
    private_safe(&output);
    assert!(!root.path().join("world").exists());
}

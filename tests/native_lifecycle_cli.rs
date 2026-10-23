//! Serialized authored CLI lifecycle tests, including destination protection.
use icarus_bevy::{
    config::ServerConfig,
    native_lifecycle::{
        CompiledNativeLifecycle, LifecycleDefinition, Presence, RecorderKind, RecorderSource,
        ResolvePolicy, RestoreBatch, RestoreRecorder, SlotDeclaration, SpawnComponent,
        prepare_lifecycle_world,
    },
    native_world::{CompiledNativeCatalog, NativeTransform, prepare_authored_world},
    persistence::SnapshotStore,
    simulation::{DomainId, WorldSnapshot, build_app, capture},
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    path::Path,
    process::{Command, Output, Stdio},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

static FIXTURE_LOCK: Mutex<()> = Mutex::new(());
const CATALOG: &str = r#"
native_format=1
frame="right_handed_z_up_meters"
[[classes]]
key="secret_box"
kind="spatial_object_v1"
local_bounds={min=[-1.0,-1.0,-1.0],max=[1.0,1.0,1.0]}
variables=[]
[[maps]]
key="secret_map"
[[maps.tiles]]
key="secret_tile"
bounds={min=[-100.0,-100.0,-100.0],max=[100.0,100.0,100.0]}
[[maps.placements]]
key="secret_owner"
class="secret_box"
tile="secret_tile"
transform={translation=[0.0,0.0,0.0],rotation=[0.0,0.0,0.0,1.0],scale=[1.0,1.0,1.0]}
"#;

fn inputs(root: &Path) -> WorldSnapshot {
    let catalog = Arc::new(CompiledNativeCatalog::parse(CATALOG).unwrap());
    let definition = LifecycleDefinition {
        lifecycle_format: 1,
        native_catalog_id: catalog.identity().into(),
        map: "secret_map".into(),
        slots: vec![SlotDeclaration {
            placement: "secret_owner".into(),
            owner_name: "SecretOwnerName".encode_utf16().collect(),
            recorder_kind: RecorderKind::SpatialStateV1,
            initial_presence: Presence::Absent,
            initial_component: None,
            spawn_component: SpawnComponent::Missing,
        }],
    };
    let compiled = CompiledNativeLifecycle::compile(definition.clone(), &catalog).unwrap();
    let prepared = prepare_lifecycle_world(
        prepare_authored_world(catalog, "secret_map")
            .unwrap()
            .initial_snapshot(),
        &compiled,
    )
    .unwrap();
    let request = RestoreBatch {
        lifecycle_id: compiled.identity().into(),
        sequence: 1,
        recorders: vec![RestoreRecorder {
            guid: 314159265,
            kind: RecorderKind::SpatialStateV1,
            target_placement: "secret_owner".into(),
            owner_name: "sECREToWNERnAME".encode_utf16().collect(),
            policy: ResolvePolicy::FindOrRespawn,
            source: RecorderSource::Detached {
                expected_prior_binding: None,
            },
            create_if_missing: true,
            expected_presence: Presence::Absent,
            expected_incarnation: 0,
            transform: NativeTransform {
                translation: [2.0, 3.0, 4.0],
                rotation: [0.0, 0.0, 1.0, 0.0],
                scale: [2.0, 3.0, 4.0],
            },
            variables: vec![],
        }],
    };
    fs::write(root.join("catalog.toml"), CATALOG).unwrap();
    fs::write(
        root.join("lifecycle.json"),
        serde_json::to_vec(&definition).unwrap(),
    )
    .unwrap();
    fs::write(
        root.join("restore.json"),
        serde_json::to_vec(&request).unwrap(),
    )
    .unwrap();
    prepared.into_snapshot()
}

fn execute(command: &mut Command) -> Output {
    // Tempfiles avoid pipe backpressure while retaining stdout/stderr on failure.
    let stdout = tempfile::tempfile().unwrap();
    let stderr = tempfile::tempfile().unwrap();
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
            panic!("bounded authored CLI deadline exceeded");
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    use std::io::{Read, Seek};
    let mut stdout = stdout;
    let mut stderr = stderr;
    stdout.rewind().unwrap();
    stderr.rewind().unwrap();
    let mut out = Vec::new();
    let mut err = Vec::new();
    stdout.read_to_end(&mut out).unwrap();
    stderr.read_to_end(&mut err).unwrap();
    Output {
        status,
        stdout: out,
        stderr: err,
    }
}

fn create(root: &Path, restore: bool, dry_run: bool) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_icarus-server"));
    command
        .arg("create-native-world")
        .arg("--catalog")
        .arg(root.join("catalog.toml"))
        .args(["--map", "secret_map", "--lifecycle"])
        .arg(root.join("lifecycle.json"))
        .arg("--data-dir")
        .arg(root.join("world"));
    if restore {
        command.arg("--restore").arg(root.join("restore.json"));
    }
    if dry_run {
        command.arg("--dry-run");
    }
    execute(&mut command)
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
    for value in ["secret_owner", "SecretOwnerName", "314159265", "secret_box"] {
        assert!(!String::from_utf8_lossy(&output.stdout).contains(value));
        assert!(!String::from_utf8_lossy(&output.stderr).contains(value));
    }
}

#[test]
fn creation_prepares_before_destination_and_run_reloads_without_inputs() {
    let _guard = FIXTURE_LOCK.lock().unwrap();
    let root = tempfile::tempdir().unwrap();
    inputs(root.path());
    let dry = create(root.path(), true, true);
    assert!(
        dry.status.success(),
        "{}",
        String::from_utf8_lossy(&dry.stderr)
    );
    assert!(!root.path().join("world").exists());
    private_safe(&dry);
    let created = create(root.path(), true, false);
    assert!(
        created.status.success(),
        "{}",
        String::from_utf8_lossy(&created.stderr)
    );
    private_safe(&created);
    let report: Value = serde_json::from_slice(&created.stdout).unwrap();
    assert_eq!(report["schema_version"], 7);
    assert_eq!(report["live_actors"], 1);
    assert_eq!(report["world_imported"], false);
    assert_eq!(report["gameplay_ready"], false);
    for name in ["catalog.toml", "lifecycle.json", "restore.json"] {
        fs::remove_file(root.path().join(name)).unwrap();
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
    let snapshot = SnapshotStore::open(&root.path().join("world"))
        .unwrap()
        .load()
        .unwrap()
        .unwrap()
        .snapshot;
    assert!(snapshot.tick >= 3601);
    assert_eq!(snapshot.schema_version, 7);
    let slot = &snapshot.native_lifecycle.as_ref().unwrap().slots[0];
    assert_eq!(slot.incarnation, 1);
    assert_eq!(slot.presence, Presence::Live);
    assert_eq!(
        snapshot
            .native_lifecycle
            .as_ref()
            .unwrap()
            .completed
            .as_ref()
            .unwrap()
            .sequence,
        1
    );
    let mut app = build_app(&ServerConfig::default(), &snapshot).unwrap();
    assert_eq!(capture(app.world_mut()).unwrap(), snapshot);
    assert_eq!(
        snapshot.native_world.as_ref().unwrap().actors[0]
            .transform
            .scale,
        [2.0, 3.0, 4.0]
    );
}

#[test]
fn initial_absence_is_persisted_without_materialization() {
    let _guard = FIXTURE_LOCK.lock().unwrap();
    let root = tempfile::tempdir().unwrap();
    let expected = inputs(root.path());
    let output = create(root.path(), false, false);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["live_actors"], 0);
    let snapshot = SnapshotStore::open(&root.path().join("world"))
        .unwrap()
        .load()
        .unwrap()
        .unwrap()
        .snapshot;
    assert_eq!(snapshot, expected);
    let app = build_app(&ServerConfig::default(), &snapshot).unwrap();
    assert!(
        !app.world()
            .iter_entities()
            .any(|entity| entity.contains::<DomainId>())
    );
}

#[test]
fn rejection_preserves_absent_and_existing_destinations_and_inputs() {
    let _guard = FIXTURE_LOCK.lock().unwrap();
    let root = tempfile::tempdir().unwrap();
    inputs(root.path());
    let path = root.path().join("restore.json");
    let mut request: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    request["recorders"][0]["policy"] = json!("find_only");
    fs::write(&path, serde_json::to_vec(&request).unwrap()).unwrap();
    let input_bytes = fs::read(&path).unwrap();
    for dry in [true, false] {
        let output = create(root.path(), true, dry);
        assert!(!output.status.success());
        private_safe(&output);
        assert!(!root.path().join("world").exists());
        assert_eq!(fs::read(&path).unwrap(), input_bytes);
    }
    fs::create_dir(root.path().join("world")).unwrap();
    fs::write(root.path().join("world/sentinel"), b"preserve me").unwrap();
    let before = files(&root.path().join("world"));
    assert!(!create(root.path(), true, false).status.success());
    assert_eq!(files(&root.path().join("world")), before);
}

#[test]
fn valid_creation_refuses_existing_committed_world() {
    let _guard = FIXTURE_LOCK.lock().unwrap();
    let root = tempfile::tempdir().unwrap();
    inputs(root.path());
    assert!(create(root.path(), true, false).status.success());
    let before = files(&root.path().join("world"));
    let output = create(root.path(), true, false);
    assert!(!output.status.success());
    private_safe(&output);
    assert_eq!(files(&root.path().join("world")), before);
}

#[test]
fn malformed_and_future_authored_inputs_leave_no_destination() {
    let _guard = FIXTURE_LOCK.lock().unwrap();
    for mutation in ["future", "unknown", "array", "duplicate"] {
        let root = tempfile::tempdir().unwrap();
        inputs(root.path());
        let path = root.path().join("lifecycle.json");
        let mut definition: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        match mutation {
            "future" => definition["lifecycle_format"] = json!(2),
            "unknown" => definition["PrivateUnknownField"] = json!("SecretOwnerName"),
            "array" => definition["slots"][0] = json!([]),
            _ => {
                let slot = definition["slots"][0].clone();
                definition["slots"].as_array_mut().unwrap().push(slot);
            }
        }
        fs::write(path, serde_json::to_vec(&definition).unwrap()).unwrap();
        let output = create(root.path(), false, false);
        assert!(!output.status.success());
        private_safe(&output);
        assert!(!root.path().join("world").exists());
    }
}

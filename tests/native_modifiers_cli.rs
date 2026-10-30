//! Authored native modifier CLI behavior. No original data or private fixtures.
use icarus_bevy::{
    native_lifecycle::{
        CompiledNativeLifecycle, LifecycleDefinition, Presence, RecorderKind, RecorderSource,
        ResolvePolicy, RestoreBatch, RestoreRecorder, SlotDeclaration, SpawnComponent,
    },
    native_world::{CompiledNativeCatalog, NativeTransform},
    persistence::SnapshotStore,
    simulation::WorldSnapshot,
};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    fs,
    io::{Read, Seek},
    os::unix::fs::symlink,
    path::Path,
    process::{Command, Output, Stdio},
    sync::Mutex,
    time::{Duration, Instant},
};

// Keep store ownership and child launches serialized so a fork cannot inherit
// another test's advisory store lock.
static FIXTURE_LOCK: Mutex<()> = Mutex::new(());
const SECRET: &str = "ModifierCliSentinel";
const CATALOG: &str = r#"
native_format=1
frame="right_handed_z_up_meters"
[[classes]]
key="ModifierCliSentinelClass"
kind="spatial_object_v1"
local_bounds={min=[-1.0,-1.0,-1.0],max=[1.0,1.0,1.0]}
variables=[]
[[maps]]
key="ModifierCliSentinelMap"
[[maps.tiles]]
key="ModifierCliSentinelTile"
bounds={min=[-100.0,-100.0,-100.0],max=[100.0,100.0,100.0]}
[[maps.placements]]
key="ModifierCliSentinelLive"
class="ModifierCliSentinelClass"
tile="ModifierCliSentinelTile"
transform={translation=[0.0,0.0,0.0],rotation=[0.0,0.0,0.0,1.0],scale=[1.0,1.0,1.0]}
[[maps.placements]]
key="ModifierCliSentinelSpawn"
class="ModifierCliSentinelClass"
tile="ModifierCliSentinelTile"
transform={translation=[2.0,0.0,0.0],rotation=[0.0,0.0,0.0,1.0],scale=[1.0,1.0,1.0]}
"#;

fn write_json(path: &Path, value: &impl serde::Serialize) {
    fs::write(path, serde_json::to_vec(value).unwrap()).unwrap();
}

fn inputs(root: &Path) {
    let catalog = CompiledNativeCatalog::parse(CATALOG).unwrap();
    let definition = LifecycleDefinition {
        lifecycle_format: 1,
        native_catalog_id: catalog.identity().into(),
        map: "ModifierCliSentinelMap".into(),
        slots: vec![
            SlotDeclaration {
                placement: "ModifierCliSentinelLive".into(),
                owner_name: "ModifierCliSentinelLiveOwner".encode_utf16().collect(),
                recorder_kind: RecorderKind::SpatialStateV1,
                initial_presence: Presence::Live,
                initial_component: None,
                spawn_component: SpawnComponent::Missing,
            },
            SlotDeclaration {
                placement: "ModifierCliSentinelSpawn".into(),
                owner_name: "ModifierCliSentinelSpawnOwner".encode_utf16().collect(),
                recorder_kind: RecorderKind::SpatialStateV1,
                initial_presence: Presence::Absent,
                initial_component: None,
                spawn_component: SpawnComponent::Missing,
            },
        ],
    };
    let lifecycle = CompiledNativeLifecycle::compile(definition.clone(), &catalog).unwrap();
    let restore = RestoreBatch {
        lifecycle_id: lifecycle.identity().into(),
        sequence: 1,
        recorders: vec![RestoreRecorder {
            guid: 314159265,
            kind: RecorderKind::SpatialStateV1,
            target_placement: "ModifierCliSentinelSpawn".into(),
            owner_name: "ModifierCliSentinelSpawnOwner".encode_utf16().collect(),
            policy: ResolvePolicy::FindOrRespawn,
            source: RecorderSource::Detached {
                expected_prior_binding: None,
            },
            create_if_missing: true,
            expected_presence: Presence::Absent,
            expected_incarnation: 0,
            transform: NativeTransform {
                translation: [3.0, 4.0, 5.0],
                rotation: [0.0, 0.0, 0.0, 1.0],
                scale: [1.0, 1.0, 1.0],
            },
            variables: vec![],
        }],
    };
    fs::write(root.join("catalog.toml"), CATALOG).unwrap();
    write_json(&root.join("lifecycle.json"), &definition);
    write_json(&root.join("restore.json"), &restore);
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
            panic!("authored modifier CLI deadline exceeded");
        }
        std::thread::sleep(Duration::from_millis(5));
    };
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

fn value_free(output: &Output) {
    for bytes in [&output.stdout, &output.stderr] {
        let text = String::from_utf8_lossy(bytes);
        for value in [SECRET, "314159265", "PrivateUnknownField"] {
            assert!(
                !text.contains(value),
                "diagnostic disclosed an authored value"
            );
        }
    }
}

fn success(output: &Output) -> Value {
    value_free(output);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

fn create(root: &Path, destination: &str, modifiers: bool, restore: bool, dry_run: bool) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_icarus-server"));
    command
        .arg("create-native-world")
        .arg("--catalog")
        .arg(root.join("catalog.toml"))
        .args(["--map", "ModifierCliSentinelMap", "--lifecycle"])
        .arg(root.join("lifecycle.json"))
        .arg("--data-dir")
        .arg(root.join(destination));
    if modifiers {
        command.arg("--modifiers");
    }
    if restore {
        command.arg("--restore").arg(root.join("restore.json"));
    }
    if dry_run {
        command.arg("--dry-run");
    }
    execute(&mut command)
}

fn apply(root: &Path, request: &Path, destination: &str) -> Output {
    execute(
        Command::new(env!("CARGO_BIN_EXE_icarus-server"))
            .arg("apply-native-modifier")
            .arg("--request")
            .arg(request)
            .arg("--data-dir")
            .arg(root.join(destination)),
    )
}

fn run(root: &Path, ticks: u64) -> Output {
    execute(
        Command::new(env!("CARGO_BIN_EXE_icarus-server"))
            .arg("run")
            .arg("--data-dir")
            .arg(root.join("world"))
            .args(["--compressed", "--max-ticks", &ticks.to_string()]),
    )
}

fn load(path: &Path) -> WorldSnapshot {
    SnapshotStore::open(path)
        .unwrap()
        .load()
        .unwrap()
        .unwrap()
        .snapshot
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

#[test]
fn create_modifiers_extends_lifecycle_after_restore_and_preserves_default_schema() {
    let _guard = FIXTURE_LOCK.lock().unwrap();
    let root = tempfile::tempdir().unwrap();
    inputs(root.path());
    for restored in [false, true] {
        let dry = success(&create(root.path(), "dry", true, restored, true));
        assert_eq!(dry["schema_version"], 11);
        assert_eq!(dry["live_actors"], if restored { 2 } else { 1 });
        assert_eq!(dry["world_imported"], false);
        assert_eq!(dry["gameplay_ready"], false);
        assert!(!root.path().join("dry").exists());
        let plain = format!("plain-{restored}");
        let enabled = format!("enabled-{restored}");
        assert_eq!(
            success(&create(root.path(), &plain, false, restored, false))["schema_version"],
            7
        );
        let report = success(&create(root.path(), &enabled, true, restored, false));
        assert_eq!(report["schema_version"], 11);
        assert_eq!(report["receipt"]["generation"], 1);
        let base = load(&root.path().join(plain));
        let snapshot = load(&root.path().join(enabled));
        assert_eq!(
            snapshot,
            icarus_bevy::native_modifiers::enable_modifier_owners(base).unwrap()
        );
        assert_eq!(
            snapshot
                .native_lifecycle
                .as_ref()
                .unwrap()
                .slots
                .iter()
                .filter(|slot| slot.presence == Presence::Live)
                .count(),
            if restored { 2 } else { 1 }
        );
    }
}

#[test]
fn modifiers_requires_lifecycle_before_destination_is_opened() {
    let _guard = FIXTURE_LOCK.lock().unwrap();
    let root = tempfile::tempdir().unwrap();
    let output = execute(
        Command::new(env!("CARGO_BIN_EXE_icarus-server"))
            .arg("create-native-world")
            .arg("--catalog")
            .arg(root.path().join("missing.toml"))
            .args(["--map", "authored", "--modifiers", "--data-dir"])
            .arg(root.path().join("world")),
    );
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("--lifecycle"));
    assert!(!root.path().join("world").exists());
}

#[test]
fn compressed_ticks_reload_and_capture_empty_modifier_owners() {
    let _guard = FIXTURE_LOCK.lock().unwrap();
    let root = tempfile::tempdir().unwrap();
    inputs(root.path());
    success(&create(root.path(), "world", true, true, false));
    let mut expected = load(&root.path().join("world"));
    for name in ["catalog.toml", "lifecycle.json", "restore.json"] {
        fs::remove_file(root.path().join(name)).unwrap();
    }
    let output = run(root.path(), 3);
    value_free(&output);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let snapshot = load(&root.path().join("world"));
    assert_eq!(snapshot.tick, 3);
    expected.tick = snapshot.tick;
    assert_eq!(snapshot, expected);
}

#[test]
fn request_reader_rejects_nonregular_symlink_oversized_and_strict_json_inputs() {
    let _guard = FIXTURE_LOCK.lock().unwrap();
    let root = tempfile::tempdir().unwrap();
    inputs(root.path());
    success(&create(root.path(), "world", true, true, false));
    let before = files(&root.path().join("world"));
    let request = root.path().join("request.json");
    for bytes in [
        vec![b' '; 64 * 1024 + 1],
        br#"{"PrivateUnknownField":"ModifierCliSentinel"}"#.to_vec(),
        b"[]".to_vec(),
        b"{} {}".to_vec(),
        vec![0xff],
    ] {
        fs::write(&request, &bytes).unwrap();
        for destination in ["world", "missing-world"] {
            let output = apply(root.path(), &request, destination);
            value_free(&output);
            assert!(!output.status.success());
            assert_eq!(files(&root.path().join("world")), before);
            assert!(!root.path().join("missing-world").exists());
        }
        assert_eq!(fs::read(&request).unwrap(), bytes);
    }
    // Exactly 64 KiB reaches JSON validation rather than the size rejection.
    fs::write(&request, vec![b' '; 64 * 1024]).unwrap();
    let boundary = apply(root.path(), &request, "world");
    value_free(&boundary);
    assert!(!boundary.status.success());
    assert!(
        String::from_utf8_lossy(&boundary.stderr).contains("invalid native modifier request JSON")
    );
    assert_eq!(files(&root.path().join("world")), before);
    let fifo = root.path().join("request.fifo");
    rustix::fs::mkfifoat(
        rustix::fs::CWD,
        &fifo,
        rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR,
    )
    .unwrap();
    let link = root.path().join("request.link");
    symlink(&request, &link).unwrap();
    let parent_link = root.path().join("linked-directory");
    symlink(root.path(), &parent_link).unwrap();
    for (path, reason) in [
        (fifo, "modifier request must be a regular file"),
        (link, "open native modifier request failed"),
        (
            root.path().to_path_buf(),
            "modifier request must be a regular file",
        ),
        (
            parent_link.join("request.json"),
            "open native modifier request failed",
        ),
    ] {
        let output = apply(root.path(), &path, "world");
        value_free(&output);
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains(reason));
        assert_eq!(files(&root.path().join("world")), before);
    }
}

#[test]
fn applied_timer_persists_progress_restarts_and_expires_without_repeating_add() {
    use icarus_bevy::native_modifiers::{
        FlatStatContribution, ModifierAction, ModifierDefinition, ModifierDuration,
        ModifierRequest, ModifierTimer,
    };
    let _guard = FIXTURE_LOCK.lock().unwrap();
    let root = tempfile::tempdir().unwrap();
    inputs(root.path());
    success(&create(root.path(), "world", true, true, false));
    let snapshot = load(&root.path().join("world"));
    let state = &snapshot.native_modifiers.as_ref().unwrap().owners[0];
    let request = ModifierRequest {
        owner: state.owner,
        expected_revision: state.revision,
        action: ModifierAction::Add {
            definition: ModifierDefinition {
                name: SECRET.into(),
                duration: ModifierDuration::Timed { seconds: 1.0 },
                contributions: vec![FlatStatContribution {
                    stat: "power".into(),
                    amount: 7,
                }],
            },
        },
    };
    let path = root.path().join("request.json");
    write_json(&path, &request);
    success(&apply(root.path(), &path, "world"));
    let loaded = load(&root.path().join("world"));
    assert_eq!(
        loaded.native_modifiers.as_ref().unwrap().owners[0].cache[0].amount,
        7
    );
    let before = files(&root.path().join("world"));
    let stale = apply(root.path(), &path, "world");
    value_free(&stale);
    assert!(!stale.status.success());
    assert_eq!(files(&root.path().join("world")), before);
    let output = run(root.path(), 10);
    value_free(&output);
    assert!(output.status.success());
    let partial = load(&root.path().join("world"));
    let partial_owner = &partial.native_modifiers.as_ref().unwrap().owners[0];
    match partial_owner.entries[0].timer {
        ModifierTimer::Timed {
            remaining_nanos,
            armed: true,
        } => assert_eq!(remaining_nanos, 666_666_667),
        _ => panic!("timer did not retain current progress"),
    }
    let output = run(root.path(), 20);
    value_free(&output);
    assert!(output.status.success());
    let expired = load(&root.path().join("world"));
    assert!(
        expired.native_modifiers.as_ref().unwrap().owners[0]
            .entries
            .is_empty()
    );
    assert!(
        expired.native_modifiers.as_ref().unwrap().owners[0]
            .cache
            .is_empty()
    );
    assert_eq!(
        expired.native_modifiers.as_ref().unwrap().owners[0].next_id,
        2
    );
}

//! Operator transactions use only independently authored inventories.
use std::{
    collections::BTreeMap,
    fs,
    io::{Read, Seek},
    path::Path,
    process::{Command, Output, Stdio},
    sync::Mutex,
    time::{Duration, Instant},
};

use icarus_bevy::{
    native_inventory::{
        InventoryActionRequestV1, InventoryActionV1, InventorySlotEndpointV1,
        InventoryStackEndpointV1, enable_inventory_owners,
    },
    persistence::SnapshotStore,
    simulation::WorldSnapshot,
};
use serde_json::{Value, json};

// Keep this suite's child launches separate from its open store descriptors.
static FIXTURE_LOCK: Mutex<()> = Mutex::new(());

fn initial() -> WorldSnapshot {
    let (base, contracts, items) = super::native_authored_inventory::inputs();
    enable_inventory_owners(base, contracts, items).unwrap()
}

fn save(root: &Path, snapshot: &WorldSnapshot) {
    SnapshotStore::open(root).unwrap().save(snapshot).unwrap();
}

fn load(root: &Path) -> WorldSnapshot {
    SnapshotStore::open(root)
        .unwrap()
        .load()
        .unwrap()
        .unwrap()
        .snapshot
}

fn disk(root: &Path) -> BTreeMap<String, Vec<u8>> {
    fs::read_dir(root)
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

fn endpoint(snapshot: &WorldSnapshot, owner_index: usize, slot: usize) -> InventoryStackEndpointV1 {
    let owner = &snapshot.native_inventories.as_ref().unwrap().owners[owner_index];
    InventoryStackEndpointV1 {
        owner: owner.owner,
        inventory: "bag".into(),
        stack_id: owner.inventories[0].slots[slot].as_ref().unwrap().id,
    }
}

fn slot(snapshot: &WorldSnapshot, owner_index: usize, slot: u16) -> InventorySlotEndpointV1 {
    InventorySlotEndpointV1 {
        owner: snapshot.native_inventories.as_ref().unwrap().owners[owner_index].owner,
        inventory: "bag".into(),
        slot,
    }
}

fn request(snapshot: &WorldSnapshot, action: InventoryActionV1) -> InventoryActionRequestV1 {
    InventoryActionRequestV1 {
        expected_inventory_revision: snapshot.native_inventories.as_ref().unwrap().revision,
        action,
    }
}

fn write_request(path: &Path, request: &InventoryActionRequestV1) {
    fs::write(path, serde_json::to_vec(request).unwrap()).unwrap();
}

fn command(root: &Path, request: &Path, dry_run: bool) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_icarus-server"));
    command
        .arg("apply-native-inventory")
        .arg("--request")
        .arg(request)
        .arg("--data-dir")
        .arg(root);
    if dry_run {
        command.arg("--dry-run");
    }
    command
}

fn execute(mut command: Command) -> Output {
    // Files keep a rejected request's diagnostics from filling a pipe while
    // the parent enforces a bound, including FIFO and lock-contention cases.
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
            panic!("inventory transaction CLI exceeded deadline");
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

fn report(output: Output) -> Value {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn durable_split_move_merge_consume_preserve_identity_and_conservation() {
    let _guard = FIXTURE_LOCK.lock().unwrap();
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("world");
    let path = directory.path().join("request.json");
    let original = initial();
    save(&root, &original);
    let original_stack = endpoint(&original, 0, 0).stack_id;
    write_request(
        &path,
        &request(
            &original,
            InventoryActionV1::Split {
                source: endpoint(&original, 0, 0),
                destination: slot(&original, 0, 1),
                amount: 3,
            },
        ),
    );
    let applied = report(execute(command(&root, &path, false)));
    assert_eq!(applied["event"], json!("native_inventory_applied"));
    assert_eq!(applied["published"], json!(true));
    assert!(applied.get("receipt").is_some());
    let split = load(&root);
    let split_stack = endpoint(&split, 0, 1).stack_id;
    assert_ne!(original_stack, split_stack);
    assert_eq!(endpoint(&split, 0, 0).stack_id, original_stack);
    write_request(
        &path,
        &request(
            &split,
            InventoryActionV1::Move {
                source: endpoint(&split, 0, 1),
                destination: slot(&split, 1, 0),
            },
        ),
    );
    report(execute(command(&root, &path, false)));
    let moved = load(&root);
    assert_eq!(endpoint(&moved, 1, 0).stack_id, split_stack);
    write_request(
        &path,
        &request(
            &moved,
            InventoryActionV1::Merge {
                source: endpoint(&moved, 1, 0),
                destination: endpoint(&moved, 0, 0),
                amount: 3,
            },
        ),
    );
    report(execute(command(&root, &path, false)));
    let merged = load(&root);
    assert_eq!(endpoint(&merged, 0, 0).stack_id, original_stack);
    assert!(
        merged.native_inventories.as_ref().unwrap().owners[1].inventories[0].slots[0].is_none()
    );
    write_request(
        &path,
        &request(
            &merged,
            InventoryActionV1::Consume {
                source: endpoint(&merged, 0, 0),
                amount: 3,
            },
        ),
    );
    let consumed = report(execute(command(&root, &path, false)));
    let loaded = load(&root);
    let state = loaded.native_inventories.as_ref().unwrap();
    assert_eq!(state.revision, 5);
    assert_eq!(consumed["revision"], json!(state.revision));
    assert_eq!(state.consumed[0].count, 3);
    assert_eq!(
        state.owners[0].inventories[0].slots[0]
            .as_ref()
            .unwrap()
            .count,
        5
    );
    assert!(state.owners[1].inventories[0].slots[0].is_none());
    assert_eq!(
        loaded.native_inventory_authority,
        original.native_inventory_authority
    );
    assert!(state.next_stack_id > split_stack.get());
}

#[test]
fn dry_run_stale_requests_and_no_change_preserve_all_store_bytes() {
    let _guard = FIXTURE_LOCK.lock().unwrap();
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("world");
    let path = directory.path().join("request.json");
    let original = initial();
    save(&root, &original);
    let mut action = request(
        &original,
        InventoryActionV1::Consume {
            source: endpoint(&original, 0, 0),
            amount: 1,
        },
    );
    write_request(&path, &action);
    let before = disk(&root);
    let prepared = report(execute(command(&root, &path, true)));
    assert_eq!(prepared["event"], json!("native_inventory_prepared"));
    assert_eq!(prepared["dry_run"], json!(true));
    assert_eq!(prepared["published"], json!(false));
    assert_eq!(prepared["outcome"], json!("applied"));
    assert_eq!(prepared["revision"], json!(2));
    assert!(prepared.get("receipt").is_none());
    assert_eq!(disk(&root), before);
    action.expected_inventory_revision += 1;
    write_request(&path, &action);
    assert!(!execute(command(&root, &path, false)).status.success());
    assert_eq!(disk(&root), before);
    let action = request(
        &original,
        InventoryActionV1::Move {
            source: endpoint(&original, 0, 0),
            destination: slot(&original, 0, 0),
        },
    );
    write_request(&path, &action);
    let unchanged = report(execute(command(&root, &path, false)));
    assert_eq!(unchanged["event"], json!("native_inventory_unchanged"));
    assert_eq!(unchanged["published"], json!(false));
    assert!(unchanged.get("receipt").is_none());
    assert_eq!(disk(&root), before);
}

#[test]
fn unknown_inventory_key_is_redacted_and_preserves_store() {
    let _guard = FIXTURE_LOCK.lock().unwrap();
    const SECRET: &str = "UnknownInventoryPrivateSentinel";
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("world");
    let path = directory.path().join("request.json");
    let original = initial();
    save(&root, &original);
    let mut source = endpoint(&original, 0, 0);
    source.inventory = SECRET.into();
    write_request(
        &path,
        &request(&original, InventoryActionV1::Consume { source, amount: 1 }),
    );
    let before = disk(&root);
    let failed = execute(command(&root, &path, false));
    assert!(!failed.status.success());
    assert!(failed.stdout.is_empty());
    assert!(!String::from_utf8_lossy(&failed.stderr).contains(SECRET));
    assert_eq!(disk(&root), before);
}

#[test]
fn symlink_directory_and_fifo_store_locks_reject_without_modification() {
    let _guard = FIXTURE_LOCK.lock().unwrap();
    for kind in ["symlink", "directory", "fifo"] {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("world");
        let path = directory.path().join("request.json");
        let original = initial();
        save(&root, &original);
        write_request(
            &path,
            &request(
                &original,
                InventoryActionV1::Consume {
                    source: endpoint(&original, 0, 0),
                    amount: 1,
                },
            ),
        );
        let manifest = fs::read(root.join("manifest.json")).unwrap();
        let snapshot = fs::read(root.join("snapshot-1.json")).unwrap();
        let lock = root.join("server.lock");
        fs::remove_file(&lock).unwrap();
        let target = directory.path().join("untouched");
        fs::write(&target, b"untouched").unwrap();
        match kind {
            "symlink" => std::os::unix::fs::symlink(&target, &lock).unwrap(),
            "directory" => fs::create_dir(&lock).unwrap(),
            "fifo" => assert!(
                Command::new("mkfifo")
                    .arg(&lock)
                    .status()
                    .unwrap()
                    .success()
            ),
            _ => unreachable!(),
        }
        let failed = execute(command(&root, &path, false));
        assert!(!failed.status.success(), "accepted {kind} lock");
        assert!(failed.stdout.is_empty());
        assert_eq!(fs::read(root.join("manifest.json")).unwrap(), manifest);
        assert_eq!(fs::read(root.join("snapshot-1.json")).unwrap(), snapshot);
        assert_eq!(fs::read(&target).unwrap(), b"untouched");
        assert_eq!(fs::read_dir(&root).unwrap().count(), 3);
        let metadata = fs::symlink_metadata(&lock).unwrap();
        match kind {
            "symlink" => assert!(metadata.file_type().is_symlink()),
            "directory" => assert!(metadata.is_dir()),
            "fifo" => {
                use std::os::unix::fs::FileTypeExt;
                assert!(metadata.file_type().is_fifo());
            }
            _ => unreachable!(),
        }
    }
}

#[test]
fn missing_locks_wrong_schema_and_concurrent_owner_never_change_store() {
    let _guard = FIXTURE_LOCK.lock().unwrap();
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("world");
    let path = directory.path().join("request.json");
    let original = initial();
    write_request(
        &path,
        &request(
            &original,
            InventoryActionV1::Consume {
                source: endpoint(&original, 0, 0),
                amount: 1,
            },
        ),
    );
    assert!(!execute(command(&root, &path, true)).status.success());
    assert!(!root.exists());
    save(&root, &original);
    let before = disk(&root);
    let owner = SnapshotStore::open(&root).unwrap();
    assert!(!execute(command(&root, &path, true)).status.success());
    assert_eq!(disk(&root), before);
    drop(owner);
    fs::remove_file(root.join("server.lock")).unwrap();
    let missing_lock = disk(&root);
    assert!(!execute(command(&root, &path, false)).status.success());
    assert_eq!(disk(&root), missing_lock);
    let unsupported = directory.path().join("unsupported");
    save(&unsupported, &super::native_authored_inventory::inputs().0);
    let before = disk(&unsupported);
    assert!(
        !execute(command(&unsupported, &path, false))
            .status
            .success()
    );
    assert_eq!(disk(&unsupported), before);
}

#[test]
fn malformed_oversize_fifo_and_symlink_requests_preserve_existing_store() {
    let _guard = FIXTURE_LOCK.lock().unwrap();
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("world");
    let path = directory.path().join("request.json");
    let original = initial();
    save(&root, &original);
    let before = disk(&root);
    fs::write(&path, b"{invalid").unwrap();
    let failed = execute(command(&root, &path, false));
    assert!(!failed.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&failed.stderr).unwrap(),
        json!({"error": "invalid native inventory request JSON", "event": "fatal"})
    );
    assert_eq!(disk(&root), before);
    fs::File::create(&path)
        .unwrap()
        .set_len(64 * 1024 + 1)
        .unwrap();
    let failed = execute(command(&root, &path, false));
    assert!(!failed.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&failed.stderr).unwrap(),
        json!({"error": "inventory request exceeds 64 KiB limit", "event": "fatal"})
    );
    assert_eq!(disk(&root), before);
    fs::remove_file(&path).unwrap();
    assert!(
        Command::new("mkfifo")
            .arg(&path)
            .status()
            .unwrap()
            .success()
    );
    let failed = execute(command(&root, &path, false));
    assert!(!failed.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&failed.stderr).unwrap(),
        json!({"error": "inventory request must be a regular file", "event": "fatal"})
    );
    assert_eq!(disk(&root), before);
    fs::remove_file(&path).unwrap();
    let actual = directory.path().join("actual.json");
    write_request(
        &actual,
        &request(
            &original,
            InventoryActionV1::Consume {
                source: endpoint(&original, 0, 0),
                amount: 1,
            },
        ),
    );
    let mut boundary = fs::read(&actual).unwrap();
    boundary.resize(64 * 1024, b' ');
    fs::write(&actual, boundary).unwrap();
    let prepared = report(execute(command(&root, &actual, true)));
    assert_eq!(prepared["outcome"], json!("applied"));
    assert_eq!(prepared["revision"], json!(2));
    assert_eq!(disk(&root), before);
    let parent_link = directory.path().join("parent_link");
    std::os::unix::fs::symlink(directory.path(), &parent_link).unwrap();
    let failed = execute(command(&root, &parent_link.join("actual.json"), false));
    assert!(!failed.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&failed.stderr).unwrap(),
        json!({"error": "open native inventory request failed", "event": "fatal"})
    );
    assert_eq!(disk(&root), before);
    std::os::unix::fs::symlink(&actual, &path).unwrap();
    let failed = execute(command(&root, &path, false));
    assert!(!failed.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&failed.stderr).unwrap(),
        json!({"error": "open native inventory request failed", "event": "fatal"})
    );
    assert_eq!(disk(&root), before);
}

#[test]
fn save_io_failure_emits_no_success_receipt_and_retains_committed_state() {
    let _guard = FIXTURE_LOCK.lock().unwrap();
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("world");
    let path = directory.path().join("request.json");
    let original = initial();
    save(&root, &original);
    write_request(
        &path,
        &request(
            &original,
            InventoryActionV1::Consume {
                source: endpoint(&original, 0, 0),
                amount: 1,
            },
        ),
    );
    fs::create_dir(root.join("snapshot-2.json")).unwrap();
    let failed = execute(command(&root, &path, false));
    assert!(!failed.status.success());
    assert!(failed.stdout.is_empty());
    assert_eq!(
        serde_json::to_value(load(&root)).unwrap(),
        serde_json::to_value(original).unwrap()
    );
}

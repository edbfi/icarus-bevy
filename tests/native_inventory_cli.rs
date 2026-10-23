//! Bounded CLI regressions for wholly authored inventory fixtures, not official compatibility.
#[path = "common/c001_inventory_owners.rs"]
mod fixture;
#[path = "common/c001_scalar_owners.rs"]
mod scalar;
#[path = "common/c001_inventory_source.rs"]
mod source;

use icarus_bevy::{
    native_c001_inventory_owners::{
        prepare_c001_inventory_owner_import, publish_new_c001_inventory_owner_import,
    },
    native_import::{SemanticImportCategory, SemanticImportError},
    native_inventory::{InventoryStateV1, StackV1},
    native_lifecycle::RestoreDisposition,
    persistence::SnapshotStore,
    preservation::is_sidecar_name,
    simulation::{DomainId, WorldSnapshot},
};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    fs,
    io::{Read, Seek},
    path::Path,
    process::{Command, Output, Stdio},
    sync::Mutex,
    time::{Duration, Instant},
};

// Every test holds this guard across fixtures, store ownership and child launches.
// Otherwise a sibling fork can briefly inherit another test's store lock.
static FIXTURE_LOCK: Mutex<()> = Mutex::new(());
const SECRET: &str = "InventoryCliPrivateSentinel";
const INPUTS: [&str; 7] = [
    "source",
    "catalog",
    "lifecycle",
    "scalar-contracts",
    "inventory-contracts",
    "bindings",
    "relationships",
];

fn write_json(path: &Path, value: &impl serde::Serialize) {
    fs::write(path, serde_json::to_vec(value).unwrap()).unwrap();
}

fn inputs(root: &Path) -> fixture::Fixture {
    let fixture = fixture::fixture();
    fs::write(root.join("source.json"), &fixture.envelope).unwrap();
    write_json(&root.join("catalog.json"), fixture.catalog.definition());
    write_json(&root.join("bindings.json"), fixture.bindings.definition());
    write_json(
        &root.join("scalar-contracts.json"),
        fixture.scalar_contracts.definition(),
    );
    write_json(
        &root.join("inventory-contracts.json"),
        fixture.inventory_contracts.definition(),
    );
    write_json(
        &root.join("lifecycle.json"),
        &fixture.base.native_lifecycle.as_ref().unwrap().definition,
    );
    write_json(
        &root.join("relationships.json"),
        &fixture
            .base
            .native_relationships
            .as_ref()
            .unwrap()
            .definition,
    );
    fixture
}

fn command(root: &Path, dry_run: bool, relationships: bool) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_icarus-server"));
    command.arg("create-c001-inventory-owner-fixture");
    for name in INPUTS {
        if name != "relationships" || relationships {
            command
                .arg(format!("--{name}"))
                .arg(root.join(format!("{name}.json")));
        }
    }
    command.arg("--data-dir").arg(root.join("world"));
    if dry_run {
        command.arg("--dry-run");
    }
    command
}

fn execute(command: &mut Command) -> Output {
    // Files avoid pipe backpressure while the parent enforces the deadline.
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
            panic!("authored inventory CLI deadline exceeded");
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

fn value_free(text: &str, fixture: &fixture::Fixture) {
    for value in [
        SECRET,
        "PrivateSentinelUnknown",
        "SourceCrate",
        "/Authored/Owner",
        "SourceObject",
        "SpawnOwner",
        "FindOwner",
        "ReplaceOwner",
        "Authored owner fixture",
        "SourceBag",
        "SourceZero",
        "AuthoredSecondRow",
        "apple",
        "stone",
        &fixture.bindings.definition().expected_payload_sha256,
    ] {
        assert!(
            !text.contains(value),
            "diagnostic disclosed an authored value"
        );
    }
}

fn output_value_free(output: &Output, fixture: &fixture::Fixture) {
    value_free(&String::from_utf8_lossy(&output.stdout), fixture);
    value_free(&String::from_utf8_lossy(&output.stderr), fixture);
}

fn success(output: &Output, fixture: &fixture::Fixture) {
    output_value_free(output, fixture);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn report(output: &Output, fixture: &fixture::Fixture, dry_run: bool) -> Value {
    success(output, fixture);
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["schema_version"], 10);
    assert_eq!(report["world_imported"], false);
    assert_eq!(report["gameplay_ready"], false);
    assert_eq!(report["inspection"]["world_imported"], false);
    assert_eq!(report["inspection"]["gameplay_ready"], false);
    assert_eq!(report["inspection"]["owners"], 3);
    assert_eq!(report["inspection"]["inventories"], 6);
    assert_eq!(report["inspection"]["stacks"], 4);
    assert_eq!(report["inspection"]["deferred_stacks"], 1);
    assert_eq!(report["inspection"]["payload_bytes"], fixture.payload.len());
    if dry_run {
        assert_eq!(report["event"], "c001_inventory_owner_fixture_prepared");
        assert_eq!(report["dry_run"], true);
        assert!(report.get("receipt").is_none());
    } else {
        assert_eq!(report["event"], "c001_inventory_owner_fixture_materialized");
        assert_eq!(report["receipt"]["generation"], 1);
        assert_eq!(report["receipt"]["tick"], 0);
    }
    report
}

fn load(path: &Path) -> WorldSnapshot {
    // Release the store before the caller can launch another child.
    SnapshotStore::open(path)
        .unwrap()
        .load()
        .unwrap()
        .unwrap()
        .snapshot
}

fn inventory<'a>(snapshot: &'a WorldSnapshot, owner: u64, key: &str) -> &'a InventoryStateV1 {
    snapshot
        .native_inventories
        .as_ref()
        .unwrap()
        .owners
        .iter()
        .find(|entry| entry.owner.id == DomainId(owner))
        .unwrap()
        .inventories
        .iter()
        .find(|entry| entry.key == key)
        .unwrap()
}

fn stack(stack: &StackV1, id: u64, item: &str, count: u32) {
    assert_eq!(stack.id.get(), id);
    assert_eq!(stack.item, item);
    assert_eq!(stack.count, count);
}

fn complete_state(snapshot: &WorldSnapshot, fixture: &fixture::Fixture) {
    assert_eq!(snapshot.schema_version, 10);
    let state = snapshot.native_inventories.as_ref().unwrap();
    assert_eq!(state.revision, 1);
    assert_eq!(state.next_stack_id, 5);
    assert_eq!(state.owners.len(), 3);
    assert!(state.consumed.iter().all(|total| total.count == 0));
    for (id, enabled, uses, incarnation) in [(1, false, 2, 1), (2, true, 2, 2), (3, true, 1, 1)] {
        let owner = state
            .owners
            .iter()
            .find(|o| o.owner.id == DomainId(id))
            .unwrap();
        assert_eq!(owner.owner.incarnation, incarnation);
        assert_eq!(owner.inventories.len(), 2);
        let actor = snapshot
            .native_world
            .as_ref()
            .unwrap()
            .actors
            .iter()
            .find(|a| a.id == DomainId(id))
            .unwrap();
        assert_eq!(actor.variables, scalar::scalar_pair(enabled, uses));
        let slot = snapshot
            .native_lifecycle
            .as_ref()
            .unwrap()
            .slots
            .iter()
            .find(|s| s.id == DomainId(id))
            .unwrap();
        assert_eq!(slot.incarnation, incarnation);
    }
    let zero = inventory(snapshot, 3, "zero");
    assert_eq!(zero.slots.len(), 1);
    stack(zero.slots[0].as_ref().unwrap(), 1, "apple", 3);
    assert_eq!(zero.overflow.len(), 1);
    assert_eq!(zero.overflow[0].requested_slot, 0);
    stack(&zero.overflow[0].stack, 2, "stone", 17);
    for (owner, id, item) in [(3, 3, "apple"), (2, 4, "stone")] {
        let bag = inventory(snapshot, owner, "bag");
        assert_eq!(bag.slots.len(), 2);
        stack(bag.slots[0].as_ref().unwrap(), id, item, 32);
        assert!(bag.slots[1].is_none());
        assert!(bag.overflow.is_empty());
    }
    for owner in [1, 2] {
        let zero = inventory(snapshot, owner, "zero");
        assert!(zero.slots.is_empty());
        assert!(zero.overflow.is_empty());
    }
    let empty = inventory(snapshot, 1, "bag");
    assert_eq!(empty.slots.len(), 2);
    assert!(empty.slots.iter().all(Option::is_none));
    assert!(empty.overflow.is_empty());
    let metadata = snapshot.native_inventory_owner_import.as_ref().unwrap();
    for disposition in [
        RestoreDisposition::Attached,
        RestoreDisposition::Spawned,
        RestoreDisposition::Replaced,
    ] {
        assert_eq!(
            metadata
                .initial_completion
                .outcomes
                .iter()
                .filter(|o| o.disposition == disposition)
                .count(),
            1
        );
    }
    assert_eq!(metadata.inventory_initial.initial_next_stack_id, 5);
    assert_eq!(
        metadata
            .inventory_initial
            .initial_totals
            .items
            .iter()
            .map(|i| (i.item.as_str(), i.count))
            .collect::<Vec<_>>(),
        vec![("apple", 35), ("stone", 49)]
    );
    let preserved = snapshot.preserved_prospect.as_ref().unwrap();
    assert_eq!(
        preserved.decode().unwrap().to_inner_payload().unwrap(),
        fixture.payload
    );
    assert_eq!(
        preserved.native_encoding(),
        fixture.candidate().preserved().native_encoding()
    );
}

#[test]
fn complete_cli_fixture_dry_run_publication_autosave_and_reload_without_inputs() {
    let _guard = FIXTURE_LOCK.lock().unwrap();
    for relationships in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let fixture = inputs(root.path());
        let mut expected = fixture.snapshot();
        if !relationships {
            expected.native_relationships = None;
            fs::remove_file(root.path().join("relationships.json")).unwrap();
        }
        let before = files(root.path());
        let dry = execute(&mut command(root.path(), true, relationships));
        report(&dry, &fixture, true);
        assert!(!root.path().join("world").exists());
        assert_eq!(files(root.path()), before);

        let created = execute(&mut command(root.path(), false, relationships));
        report(&created, &fixture, false);
        let path = root.path().join("world");
        let stored = load(&path);
        assert_eq!(stored, expected);
        complete_state(&stored, &fixture);
        assert_eq!(stored.native_relationships, expected.native_relationships);
        let wire: Value =
            serde_json::from_slice(&fs::read(path.join("snapshot-1.json")).unwrap()).unwrap();
        assert_eq!(wire["schema_version"], 10);
        assert_eq!(wire.get("native_relationships").is_some(), relationships);
        for legacy in [
            "native_import",
            "native_owner_import",
            "native_scalar_owner_import",
        ] {
            assert!(wire.get(legacy).is_none());
        }
        let sidecars: BTreeMap<_, _> = files(&path)
            .into_iter()
            .filter(|(name, _)| is_sidecar_name(name))
            .collect();
        assert_eq!(sidecars.len(), 1);
        assert_eq!(
            sidecars.values().next().unwrap().as_slice(),
            stored
                .preserved_prospect
                .as_ref()
                .unwrap()
                .native_encoding()
        );
        for name in INPUTS {
            let input = root.path().join(format!("{name}.json"));
            if input.exists() {
                fs::remove_file(input).unwrap();
            }
        }

        let run = execute(
            Command::new(env!("CARGO_BIN_EXE_icarus-server"))
                .arg("run")
                .arg("--data-dir")
                .arg(&path)
                .args([
                    "--compressed",
                    "--max-ticks",
                    "61",
                    "--autosave-seconds",
                    "1",
                ]),
        );
        success(&run, &fixture);
        let events: Vec<Value> = String::from_utf8(run.stderr)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert!(events.iter().any(|event| event["event"] == "save_committed"
            && event["tick"].as_u64().is_some_and(|tick| tick < 61)));
        assert!(
            events
                .iter()
                .any(|event| event["event"] == "save_committed" && event["tick"] == 61)
        );
        let stopped = events
            .iter()
            .find(|event| event["event"] == "stopped")
            .unwrap();
        assert_eq!(stopped["tick"], 61);
        assert_eq!(stopped["gameplay_ready"], false);
        expected.tick = 61;
        let loaded = load(&path);
        assert_eq!(loaded, expected);
        complete_state(&loaded, &fixture);
        let manifest: Value =
            serde_json::from_slice(&fs::read(path.join("manifest.json")).unwrap()).unwrap();
        assert!(manifest["current"]["number"].as_u64().unwrap() >= 3);
        assert_eq!(
            files(&path)
                .into_iter()
                .filter(|(name, _)| is_sidecar_name(name))
                .collect::<BTreeMap<_, _>>(),
            sidecars
        );

        let reload = execute(
            Command::new(env!("CARGO_BIN_EXE_icarus-server"))
                .arg("run")
                .arg("--data-dir")
                .arg(&path)
                .args(["--compressed", "--max-ticks", "1"]),
        );
        success(&reload, &fixture);
        expected.tick = 62;
        assert_eq!(load(&path), expected);
    }
}

#[test]
fn final_nested_field_and_final_binding_reject_before_destination_access() {
    let _guard = FIXTURE_LOCK.lock().unwrap();
    for nested in [false, true] {
        for existing in [false, true] {
            let root = tempfile::tempdir().unwrap();
            let mut fixture = inputs(root.path());
            if nested {
                let mut item = source::item_fields(&scalar::units("AuthoredSecondRow"), 0, 32);
                item.push(scalar::Tag::text(
                    "PrivateSentinelUnknown",
                    "StrProperty",
                    &scalar::units(SECRET),
                ));
                source::replace(
                    fixture.owners.last_mut().unwrap(),
                    "AuthoredInventories",
                    source::inventory_array(&[
                        source::inventory_fields(&scalar::units("SourceBag"), &[item]),
                        source::inventory_fields(&scalar::units("SourceZero"), &[]),
                    ]),
                );
                fixture.rebuild_source();
                fs::write(root.path().join("source.json"), &fixture.envelope).unwrap();
                write_json(
                    &root.path().join("bindings.json"),
                    fixture.bindings.definition(),
                );
            } else {
                let mut bindings = fixture.bindings.definition().clone();
                bindings
                    .owners
                    .iter_mut()
                    .find(|o| o.scalar.owner.source_icarus_guid == 702)
                    .unwrap()
                    .scalar
                    .owner
                    .source_object_name = scalar::units(SECRET);
                write_json(&root.path().join("bindings.json"), &bindings);
            }
            let destination = root.path().join("world");
            if existing {
                fs::create_dir(&destination).unwrap();
                fs::write(destination.join("sentinel"), SECRET).unwrap();
            }
            for dry_run in [false, true] {
                let output = execute(&mut command(root.path(), dry_run, true));
                output_value_free(&output, &fixture);
                assert!(!output.status.success());
                assert!(output.stdout.is_empty());
                assert!(String::from_utf8_lossy(&output.stderr).contains("recorder Some(2)"));
                if existing {
                    assert_eq!(
                        files(&destination),
                        BTreeMap::from([("sentinel".into(), SECRET.as_bytes().to_vec())])
                    );
                } else {
                    assert!(!destination.exists());
                }
            }
        }
    }
}

#[test]
fn existing_and_recovered_world_with_sentinel_is_never_overwritten() {
    let _guard = FIXTURE_LOCK.lock().unwrap();
    let root = tempfile::tempdir().unwrap();
    let fixture = inputs(root.path());
    let path = root.path().join("world");
    let first = fixture.snapshot();
    {
        let mut store = SnapshotStore::open(&path).unwrap();
        store.save_new(&first).unwrap();
        let mut second = first.clone();
        second.tick = 1;
        store.save(&second).unwrap();
    }
    fs::write(path.join("sentinel"), SECRET).unwrap();
    for corrupt in [false, true] {
        if corrupt {
            fs::write(path.join("snapshot-2.json"), b"corrupt current generation").unwrap();
            let recovered = SnapshotStore::open(&path).unwrap().load().unwrap().unwrap();
            assert!(recovered.recovered_previous);
            assert_eq!(recovered.snapshot, first);
        }
        let before = files(&path);
        let dry = execute(&mut command(root.path(), true, true));
        report(&dry, &fixture, true);
        assert_eq!(files(&path), before);
        let output = execute(&mut command(root.path(), false, true));
        output_value_free(&output, &fixture);
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert!(
            String::from_utf8_lossy(&output.stderr)
                .contains("destination already contains a world")
        );
        assert_eq!(files(&path), before);
    }
}

#[test]
fn spare_allocation_state_is_rejected_before_publication_opens_destination() {
    let _guard = FIXTURE_LOCK.lock().unwrap();
    for existing in [false, true] {
        for variables in [false, true] {
            let root = tempfile::tempdir().unwrap();
            let fixture = inputs(root.path());
            let candidate = fixture.candidate();
            let mut base = fixture.base.clone();
            if variables {
                base.native_world
                    .as_mut()
                    .unwrap()
                    .actors
                    .last_mut()
                    .unwrap()
                    .variables
                    .reserve_exact(5);
            } else {
                base.entities.reserve_exact(16_385);
            }
            // Only spare capacity changed; serialization must not hide the attack.
            assert_eq!(
                serde_json::to_vec(&base).unwrap(),
                serde_json::to_vec(&fixture.base).unwrap()
            );
            let destination = root.path().join("world");
            if existing {
                fs::create_dir(&destination).unwrap();
                fs::write(destination.join("sentinel"), SECRET).unwrap();
            }
            let result = prepare_c001_inventory_owner_import(
                &candidate,
                base,
                &fixture.bindings,
                &fixture.scalar_contracts,
                &fixture.inventory_contracts,
            )
            .map_err(anyhow::Error::new)
            .and_then(|prepared| publish_new_c001_inventory_owner_import(prepared, &destination));
            let error = result.unwrap_err();
            assert_eq!(
                error
                    .downcast_ref::<SemanticImportError>()
                    .unwrap()
                    .category,
                SemanticImportCategory::Limit
            );
            value_free(&format!("{error:#} {error:#?}"), &fixture);
            if existing {
                assert_eq!(
                    files(&destination),
                    BTreeMap::from([("sentinel".into(), SECRET.as_bytes().to_vec())])
                );
            } else {
                assert!(!destination.exists());
            }
        }
    }
}

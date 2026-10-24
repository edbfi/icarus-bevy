//! Authored inventory generations retain mutable state and immutable source.
#[path = "common/c001_inventory_owners.rs"]
mod fixture;
#[path = "common/c001_scalar_owners.rs"]
mod scalar;
#[path = "common/c001_inventory_source.rs"]
mod source;

use icarus_bevy::{
    config::ServerConfig,
    native_inventory::{
        CompiledInventoryContractsV1, InventoryActionRequestV1, InventoryActionV1,
        InventoryInitialTotalsV1, InventoryLiveOwnersV1, InventorySlotEndpointV1,
        InventoryStackEndpointV1, ItemTotalV1, prepare_inventory_action,
    },
    native_world::CompiledNativeCatalog,
    persistence::{SaveWorker, SnapshotStore},
    preservation::is_sidecar_name,
    simulation::{WorldSnapshot, build_app, capture},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    path::Path,
    time::{Duration, Instant},
};

const SECRET: &str = "InventoryStoragePrivateSentinel";

fn state(tick: u64) -> WorldSnapshot {
    let mut state = fixture::imported();
    state.tick = tick;
    state
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

fn read_json(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

fn rewrite(root: &Path, value: &Value) {
    rewrite_bytes(root, &serde_json::to_vec(value).unwrap());
}

fn rewrite_bytes(root: &Path, bytes: &[u8]) {
    fs::write(root.join("snapshot-2.json"), bytes).unwrap();
    let mut manifest = read_json(&root.join("manifest.json"));
    manifest["current"]["sha256"] = Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>()
        .into();
    fs::write(
        root.join("manifest.json"),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();
}

fn private(error: &anyhow::Error) {
    assert!(!format!("{error:#?} {error:#}").contains(SECRET));
}

fn open_after_worker(root: &Path) -> SnapshotStore {
    let until = Instant::now() + Duration::from_secs(5);
    loop {
        match SnapshotStore::open(root) {
            Ok(store) => return store,
            Err(error)
                if Instant::now() < until && format!("{error:#}").contains("already locked") =>
            {
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(error) => panic!("{error:#}"),
        }
    }
}

// Use the actual domain operations to produce an edited snapshot. The fixture
// needs a stack of at least three and one allocated eligible empty slot.
fn edited(mut state: WorldSnapshot) -> WorldSnapshot {
    let native = state.native_world.as_ref().unwrap();
    let catalog = CompiledNativeCatalog::compile(native.definitions.clone()).unwrap();
    let contracts = CompiledInventoryContractsV1::from_snapshot(
        state.native_inventory_contracts.as_ref().unwrap(),
        &catalog,
    )
    .unwrap();
    let initial = state.native_inventories.as_ref().unwrap();
    let live = InventoryLiveOwnersV1 {
        owners: initial.owners.iter().map(|owner| owner.owner).collect(),
    };
    let mut totals: BTreeMap<String, u64> = initial
        .consumed
        .iter()
        .map(|total| (total.item.clone(), total.count))
        .collect();
    for inventory in initial.owners.iter().flat_map(|owner| &owner.inventories) {
        for stack in inventory
            .slots
            .iter()
            .flatten()
            .chain(inventory.overflow.iter().map(|entry| &entry.stack))
        {
            *totals.entry(stack.item.clone()).or_default() += u64::from(stack.count);
        }
    }
    let totals = InventoryInitialTotalsV1 {
        items: totals
            .into_iter()
            .map(|(item, count)| ItemTotalV1 { item, count })
            .collect(),
    };
    let mut split = None;
    'source: for owner in &initial.owners {
        for inventory in &owner.inventories {
            for stack in inventory
                .slots
                .iter()
                .flatten()
                .filter(|stack| stack.count >= 3)
            {
                let source = InventoryStackEndpointV1 {
                    owner: owner.owner,
                    inventory: inventory.key.clone(),
                    stack_id: stack.id,
                };
                for target in &initial.owners {
                    for destination in &target.inventories {
                        for (slot, entry) in destination.slots.iter().enumerate() {
                            if entry.is_some() {
                                continue;
                            }
                            let request = InventoryActionRequestV1 {
                                expected_inventory_revision: initial.revision,
                                action: InventoryActionV1::Split {
                                    source: source.clone(),
                                    destination: InventorySlotEndpointV1 {
                                        owner: target.owner,
                                        inventory: destination.key.clone(),
                                        slot: u16::try_from(slot).unwrap(),
                                    },
                                    amount: 1,
                                },
                            };
                            if let Ok(prepared) = prepare_inventory_action(
                                &contracts, initial, &totals, &live, &request,
                            ) {
                                split = Some((prepared.into_state(), source));
                                break 'source;
                            }
                        }
                    }
                }
            }
        }
    }
    let (split, source) = split.expect("authored fixture needs an eligible split destination");
    assert_eq!(split.next_stack_id, initial.next_stack_id + 1);
    let request = InventoryActionRequestV1 {
        expected_inventory_revision: split.revision,
        action: InventoryActionV1::Consume { source, amount: 1 },
    };
    let current = prepare_inventory_action(&contracts, &split, &totals, &live, &request)
        .unwrap()
        .into_state();
    assert_eq!(current.revision, initial.revision + 2);
    assert_ne!(current.consumed, initial.consumed);
    assert_ne!(current.owners, initial.owners);
    state.native_inventories = Some(current);
    state.validate().unwrap();
    state
}

#[test]
fn schema_ten_roundtrip_keeps_complete_edited_state_and_optional_relationships() {
    for links in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let mut initial = state(10);
        if !links {
            initial.native_relationships = None;
        }
        let mut store = SnapshotStore::open(root).unwrap();
        SnapshotStore::preflight(&initial).unwrap();
        store.save_new(&initial).unwrap();
        assert_eq!(store.load().unwrap().unwrap().snapshot, initial);
        let wire = read_json(&root.join("snapshot-1.json"));
        assert_eq!(wire["schema_version"], 10);
        for field in [
            "native_import",
            "native_owner_import",
            "native_scalar_owner_import",
        ] {
            assert!(wire.get(field).is_none());
        }
        assert_eq!(wire.get("native_relationships").is_some(), links);
        assert!(wire["preserved_prospect"]["encoded_sha256"].is_string());
        let source_files: Vec<_> = disk(root)
            .into_iter()
            .filter(|(name, _)| is_sidecar_name(name))
            .collect();
        assert_eq!(source_files.len(), 1);
        let mut current = edited(initial.clone());
        current.tick = 20;
        store.save(&current).unwrap();
        assert_eq!(store.load().unwrap().unwrap().snapshot, current);
        assert_eq!(
            current.native_inventory_owner_import,
            initial.native_inventory_owner_import
        );
        assert_eq!(current.preserved_prospect, initial.preserved_prospect);
        assert_eq!(
            disk(root)
                .into_iter()
                .filter(|(name, _)| is_sidecar_name(name))
                .collect::<Vec<_>>(),
            source_files
        );
        let mut restored = build_app(&ServerConfig::default(), &current).unwrap();
        assert_eq!(capture(restored.world_mut()).unwrap(), current);
    }
}

#[test]
fn worker_and_recovery_preserve_ids_allocator_revision_and_consumption_without_source_replay() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let initial = state(10);
    let expected = edited(initial.clone());
    let mut writer = SaveWorker::spawn(SnapshotStore::open(root).unwrap()).unwrap();
    writer.submit(expected.clone()).unwrap();
    writer
        .finish_before(Instant::now() + Duration::from_secs(10))
        .unwrap();
    drop(writer);
    let mut store = open_after_worker(root);
    assert_eq!(store.load().unwrap().unwrap().snapshot, expected);
    let mut latest = initial;
    latest.tick = 30;
    store.save(&latest).unwrap();
    fs::write(root.join("snapshot-2.json"), b"damaged").unwrap();
    let loaded = store.load().unwrap().unwrap();
    assert!(loaded.recovered_previous);
    assert_eq!(loaded.snapshot, expected);
    let mut app = build_app(&ServerConfig::default(), &loaded.snapshot).unwrap();
    assert_eq!(capture(app.world_mut()).unwrap(), expected);
    let mut next = expected;
    next.tick = 40;
    store.save(&next).unwrap();
    assert_eq!(store.load().unwrap().unwrap().snapshot, next);
}

#[test]
fn future_discriminators_override_malformed_state_and_missing_source() {
    for path in [
        "/schema_version",
        "/preserved_prospect/encoding",
        "/native_world/definitions/native_format",
        "/native_world/definitions/frame",
        "/native_lifecycle/definition/lifecycle_format",
        "/native_lifecycle/completed/outcomes/0/disposition",
        "/native_relationships/definition/relationship_format",
        "/native_scalar_contracts/definition/format",
        "/native_scalar_contracts/definition/classes/0/callback",
        "/native_inventory_contracts/definition/format",
        "/native_inventories/format",
        "/native_inventory_owner_import/format",
        "/native_inventory_owner_import/bindings/format",
        "/native_inventory_owner_import/bindings/adapter",
        "/native_inventory_owner_import/bindings/coordinates",
        "/native_inventory_owner_import/bindings/owners/0/scalar/owner/expected_presence",
        "/native_inventory_owner_import/resolved/0/source_policy",
        "/native_inventory_owner_import/initial_completion/outcomes/0/policy",
        "/native_inventory_owner_import/initial_completion/outcomes/0/source/type",
        "/native_inventory_owner_import/initial_completion/outcomes/0/before_presence",
        "/native_inventory_owner_import/initial_completion/outcomes/0/disposition",
        "/native_inventory_owner_import/initial_completion/outcomes/0/component_disposition",
    ] {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let current = state(20);
        let mut store = SnapshotStore::open(root).unwrap();
        store.save(&edited(state(10))).unwrap();
        store.save(&current).unwrap();
        let mut wire = read_json(&root.join("snapshot-2.json"));
        *wire.pointer_mut(path).expect(path) = if path == "/schema_version" {
            json!(12)
        } else if path.ends_with("format") || path.ends_with("encoding") {
            json!(u64::MAX)
        } else {
            json!(format!("{SECRET}Future"))
        };
        wire["native_inventories"]["owners"] = json!({"bad": SECRET});
        wire["tick"] = json!({"bad": SECRET});
        rewrite(root, &wire);
        for name in disk(root).keys().filter(|name| is_sidecar_name(name)) {
            fs::remove_file(root.join(name)).unwrap();
        }
        let unchanged = disk(root);
        for error in [
            store.load().unwrap_err(),
            store.save(&current).unwrap_err(),
            store.save_new(&current).unwrap_err(),
        ] {
            assert!(
                format!("{error:#}").contains("unsupported native snapshot schema"),
                "{path}: {error:#}"
            );
            private(&error);
            assert_eq!(disk(root), unchanged);
        }
    }
}

#[test]
fn duplicate_members_cannot_conceal_future_inventory_discriminators() {
    for duplicate in ["nested", "parent", "header", "future_header"] {
        for future_first in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let root = dir.path();
            let current = state(20);
            let mut store = SnapshotStore::open(root).unwrap();
            store.save(&edited(state(10))).unwrap();
            store.save(&current).unwrap();
            let mut wire =
                String::from_utf8(fs::read(root.join("snapshot-2.json")).unwrap()).unwrap();
            if duplicate == "nested" {
                let known = "\"adapter\":\"c001_authored_inventory_owners_v1\"";
                let future = format!("\"adapter\":\"{SECRET}Future\"");
                let pair = if future_first {
                    format!("{future},{known}")
                } else {
                    format!("{known},{future}")
                };
                assert!(wire.contains(known));
                wire = wire.replacen(known, &pair, 1);
            } else {
                wire = wire.replace(
                    "c001_authored_inventory_owners_v1",
                    &format!("{SECRET}Future"),
                );
                let extra = match duplicate {
                    "header" => "\"schema_version\":9",
                    "future_header" => "\"schema_version\":12",
                    _ => "\"native_inventory_owner_import\":null",
                };
                if future_first {
                    wire.insert_str(wire.len() - 1, &format!(",{extra}"));
                } else {
                    wire.insert_str(1, &format!("{extra},"));
                }
            }
            rewrite_bytes(root, wire.as_bytes());
            for name in disk(root).keys().filter(|name| is_sidecar_name(name)) {
                fs::remove_file(root.join(name)).unwrap();
            }
            let unchanged = disk(root);
            for error in [
                store.load().unwrap_err(),
                store.save(&current).unwrap_err(),
                store.save_new(&current).unwrap_err(),
            ] {
                assert!(format!("{error:#}").contains("unsupported native snapshot schema"));
                private(&error);
                assert_eq!(disk(root), unchanged);
            }
        }
    }
}

#[test]
fn duplicate_known_inventory_members_remain_recoverable_corruption() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let before = edited(state(10));
    let mut store = SnapshotStore::open(root).unwrap();
    store.save(&before).unwrap();
    store.save(&state(20)).unwrap();
    let mut wire = String::from_utf8(fs::read(root.join("snapshot-2.json")).unwrap()).unwrap();
    let known = "\"adapter\":\"c001_authored_inventory_owners_v1\"";
    assert!(wire.contains(known));
    wire = wire.replacen(known, &format!("{known},{known}"), 1);
    wire.insert_str(1, "\"schema_version\":10,");
    wire.insert_str(
        1,
        &format!("\"unrelated\":{{\"adapter\":\"{SECRET}Future\"}},"),
    );
    rewrite_bytes(root, wire.as_bytes());
    let loaded = store.load().unwrap().unwrap();
    assert!(loaded.recovered_previous);
    assert_eq!(loaded.snapshot, before);
}

#[test]
fn malformed_known_inventory_metadata_and_state_recover_exact_previous_snapshot() {
    for (path, bad) in [
        (
            "/native_inventory_contracts/contract_id",
            json!("0".repeat(64)),
        ),
        ("/native_inventories/contract_id", json!("0".repeat(64))),
        ("/native_inventories/format", json!(0)),
        ("/native_inventories/next_stack_id", json!(0)),
        ("/native_inventories/revision", json!(0)),
        ("/native_inventories/owners", json!([])),
        (
            "/native_inventory_owner_import/bindings_id",
            json!("f".repeat(64)),
        ),
        (
            "/native_inventory_owner_import/bindings/format",
            json!({"bad": SECRET}),
        ),
        ("/native_inventory_owner_import/bindings/adapter", json!(7)),
        (
            "/native_inventory_owner_import/bindings/owners/0/scalar/enabled_source_name",
            json!([999]),
        ),
        (
            "/native_inventory_owner_import/resolved/0/source_index",
            json!(999),
        ),
        (
            "/native_inventory_owner_import/initial_completion/request_digest",
            json!("0".repeat(64)),
        ),
        (
            "/native_inventory_owner_import/inventory_initial/initial_state_digest",
            json!("0".repeat(64)),
        ),
        (
            "/native_inventory_owner_import/inventory_initial/initial_next_stack_id",
            json!(0),
        ),
        (
            "/native_inventory_owner_import/inventory_initial/initial_totals/items/0/count",
            json!(0),
        ),
        ("/preserved_prospect/encoding", json!(0)),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let before = edited(state(10));
        let mut store = SnapshotStore::open(root).unwrap();
        store.save(&before).unwrap();
        store.save(&state(20)).unwrap();
        let mut wire = read_json(&root.join("snapshot-2.json"));
        assert_ne!(wire.pointer(path).expect(path), &bad);
        *wire.pointer_mut(path).unwrap() = bad;
        rewrite(root, &wire);
        let loaded = store.load().unwrap().unwrap();
        assert!(loaded.recovered_previous, "{path}");
        assert_eq!(loaded.snapshot, before);
    }
}

#[test]
fn missing_required_fields_and_legacy_metadata_never_enter_schema_ten() {
    for field in [
        "native_world",
        "native_lifecycle",
        "native_scalar_contracts",
        "native_inventory_contracts",
        "native_inventories",
        "native_inventory_owner_import",
        "preserved_prospect",
    ] {
        for null in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let root = dir.path();
            let before = edited(state(10));
            let mut store = SnapshotStore::open(root).unwrap();
            store.save(&before).unwrap();
            store.save(&state(20)).unwrap();
            let mut wire = read_json(&root.join("snapshot-2.json"));
            if null {
                wire[field] = Value::Null;
            } else {
                wire.as_object_mut().unwrap().remove(field);
            }
            rewrite(root, &wire);
            let loaded = store.load().unwrap().unwrap();
            assert!(loaded.recovered_previous, "{field}");
            assert_eq!(loaded.snapshot, before);
        }
    }
    for field in [
        "native_import",
        "native_owner_import",
        "native_scalar_owner_import",
    ] {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let before = edited(state(10));
        let mut store = SnapshotStore::open(root).unwrap();
        store.save(&before).unwrap();
        store.save(&state(20)).unwrap();
        let mut wire = read_json(&root.join("snapshot-2.json"));
        wire[field] = Value::Null;
        rewrite(root, &wire);
        assert_eq!(store.load().unwrap().unwrap().snapshot, before);
    }
}

#[test]
fn missing_or_changed_source_is_refused_without_a_previous_generation() {
    for remove in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let mut store = SnapshotStore::open(root).unwrap();
        store.save(&edited(state(10))).unwrap();
        let name = disk(root)
            .keys()
            .find(|name| is_sidecar_name(name))
            .unwrap()
            .clone();
        if remove {
            fs::remove_file(root.join(name)).unwrap();
        } else {
            fs::write(root.join(name), b"changed source").unwrap();
        }
        let unchanged = disk(root);
        private(&store.load().unwrap_err());
        assert_eq!(disk(root), unchanged);
    }
}

#[test]
fn invalid_schema_ten_snapshots_never_change_empty_or_existing_stores() {
    let initial = state(10);
    let mut invalid = Vec::new();
    let mut bad = initial.clone();
    bad.native_inventory_contracts = None;
    invalid.push(bad);
    let mut bad = initial.clone();
    bad.native_inventories = None;
    invalid.push(bad);
    let mut bad = initial.clone();
    bad.native_inventory_owner_import = None;
    invalid.push(bad);
    let mut bad = initial.clone();
    bad.native_scalar_contracts = None;
    invalid.push(bad);
    let mut bad = initial.clone();
    bad.native_lifecycle = None;
    invalid.push(bad);
    let mut bad = initial.clone();
    bad.preserved_prospect = None;
    invalid.push(bad);
    let mut bad = initial.clone();
    bad.preserved_prospect = fixture::imported_variant(1).preserved_prospect;
    invalid.push(bad);
    let mut bad = initial.clone();
    bad.native_inventories.as_mut().unwrap().next_stack_id = 0;
    invalid.push(bad);
    let mut bad = initial.clone();
    bad.native_inventories
        .as_mut()
        .unwrap()
        .owners
        .reserve_exact(100_000);
    invalid.push(bad);
    let mut bad = initial.clone();
    bad.native_inventory_contracts
        .as_mut()
        .unwrap()
        .definition
        .items
        .reserve_exact(10_000);
    invalid.push(bad);
    for schema in 1..=9 {
        let mut bad = initial.clone();
        bad.schema_version = schema;
        invalid.push(bad);
    }
    for bad in invalid {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        private(&SnapshotStore::preflight(&bad).unwrap_err());
        let mut store = SnapshotStore::open(root).unwrap();
        let empty = disk(root);
        private(&store.save_new(&bad).unwrap_err());
        assert_eq!(disk(root), empty);
        store.save(&initial).unwrap();
        let saved = disk(root);
        private(&store.save(&bad).unwrap_err());
        assert_eq!(disk(root), saved);
    }
}

#[test]
fn save_new_refuses_existing_and_recoverable_inventory_generations() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let mut store = SnapshotStore::open(root).unwrap();
    store.save(&edited(state(10))).unwrap();
    store.save(&state(20)).unwrap();
    for corrupt in [false, true] {
        if corrupt {
            fs::write(root.join("snapshot-2.json"), b"bad").unwrap();
        }
        let before = disk(root);
        assert!(store.save_new(&state(30)).is_err());
        assert_eq!(disk(root), before);
    }
}

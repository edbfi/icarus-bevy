//! Source-free inventory authority, transactions and durable generations.
use std::{
    fs,
    process::{Command, Output, Stdio},
    sync::Arc,
    time::{Duration, Instant},
};

use icarus_bevy::{
    config::ServerConfig,
    native_inventory::{
        CompiledInventoryContractsV1, InitialInventoryItemV1, InitialInventoryItemsV1,
        InventoryActionRequestV1, InventoryActionV1, InventoryContractsSnapshotV1,
        InventoryContractsV1, InventoryDefinitionV1, InventoryOwnerClassV1,
        InventorySlotEndpointV1, InventoryStackEndpointV1, ItemDefinitionV1, SlotRuleV1, StackId,
        enable_inventory_owners,
    },
    native_lifecycle::{
        CompiledNativeLifecycle, LifecycleDefinition, OwnerHandle, Presence,
        RecorderComponentRecord, RecorderKind, SlotDeclaration, SpawnComponent,
        prepare_lifecycle_world,
    },
    native_lifecycle_runtime::{
        AuthoredInventoryAuthorityState, apply_native_inventory_action, native_owner_handle,
    },
    native_world::{
        Aabb3, CompiledNativeCatalog, NativeCatalogDefinition, NativeClassDefinition,
        NativeClassKind, NativeFrame, NativeMapDefinition, NativePlacement, NativeTileDefinition,
        NativeTransform, prepare_authored_world,
    },
    persistence::SnapshotStore,
    simulation::{DomainId, SNAPSHOT_SCHEMA_INVENTORY_OWNERS, WorldSnapshot, build_app, capture},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

fn inputs() -> (
    WorldSnapshot,
    InventoryContractsSnapshotV1,
    InitialInventoryItemsV1,
) {
    let catalog = Arc::new(
        CompiledNativeCatalog::compile(NativeCatalogDefinition {
            native_format: 1,
            frame: NativeFrame::RightHandedZUpMeters,
            classes: vec![NativeClassDefinition {
                key: "box".into(),
                kind: NativeClassKind::SpatialObjectV1,
                local_bounds: Aabb3 {
                    min: [-1.0; 3],
                    max: [1.0; 3],
                },
                variables: vec![],
            }],
            maps: vec![NativeMapDefinition {
                key: "map".into(),
                tiles: vec![NativeTileDefinition {
                    key: "tile".into(),
                    bounds: Aabb3 {
                        min: [-100.0; 3],
                        max: [100.0; 3],
                    },
                }],
                placements: ["a", "b"]
                    .into_iter()
                    .map(|key| NativePlacement {
                        key: key.into(),
                        class: "box".into(),
                        tile: "tile".into(),
                        transform: NativeTransform::default(),
                        initial_values: vec![],
                    })
                    .collect(),
            }],
        })
        .unwrap(),
    );
    let lifecycle = CompiledNativeLifecycle::compile(
        LifecycleDefinition {
            lifecycle_format: 1,
            native_catalog_id: catalog.identity().into(),
            map: "map".into(),
            slots: ["a", "b"]
                .into_iter()
                .enumerate()
                .map(|(index, key)| SlotDeclaration {
                    placement: key.into(),
                    owner_name: key.encode_utf16().collect(),
                    recorder_kind: RecorderKind::SpatialStateV1,
                    initial_presence: Presence::Live,
                    initial_component: Some(RecorderComponentRecord {
                        kind: RecorderKind::SpatialStateV1,
                        guid: i32::try_from(index + 1).unwrap(),
                    }),
                    spawn_component: SpawnComponent::DeclaredKind,
                })
                .collect(),
        },
        &catalog,
    )
    .unwrap();
    let base = prepare_lifecycle_world(
        prepare_authored_world(Arc::clone(&catalog), "map")
            .unwrap()
            .initial_snapshot(),
        &lifecycle,
    )
    .unwrap()
    .into_snapshot();
    let contracts = CompiledInventoryContractsV1::compile(
        InventoryContractsV1 {
            format: 1,
            native_catalog_id: catalog.identity().into(),
            native_map: "map".into(),
            items: vec![ItemDefinitionV1 {
                key: "wood".into(),
                stack_limit: 10,
            }],
            classes: vec![InventoryOwnerClassV1 {
                native_class: "box".into(),
                inventories: vec![InventoryDefinitionV1 {
                    key: "bag".into(),
                    initial_slots: 2,
                    slot_rules: vec![
                        SlotRuleV1 {
                            allowed_items: vec!["wood".into()]
                        };
                        2
                    ],
                    force_add_when_zero: false,
                    overflow_limit: 2,
                }],
            }],
        },
        &catalog,
    )
    .unwrap();
    (
        base,
        contracts.snapshot().as_ref().clone(),
        InitialInventoryItemsV1 {
            items: vec![InitialInventoryItemV1 {
                owner: DomainId(1),
                inventory: "bag".into(),
                requested_slot: 0,
                item: "wood".into(),
                count: 8,
            }],
        },
    )
}

fn initial() -> WorldSnapshot {
    let (base, contracts, items) = inputs();
    enable_inventory_owners(base, contracts, items).unwrap()
}

fn encoded(snapshot: &WorldSnapshot) -> Value {
    serde_json::to_value(snapshot).unwrap()
}

fn endpoint(owner: OwnerHandle, stack_id: StackId) -> InventoryStackEndpointV1 {
    InventoryStackEndpointV1 {
        owner,
        inventory: "bag".into(),
        stack_id,
    }
}

fn slot(owner: OwnerHandle, slot: u16) -> InventorySlotEndpointV1 {
    InventorySlotEndpointV1 {
        owner,
        inventory: "bag".into(),
        slot,
    }
}

#[test]
fn initializer_is_source_free_and_refuses_implicit_reactivation() {
    let (base, contracts, items) = inputs();
    assert_eq!(base.native_lifecycle.as_ref().unwrap().restore_sequence, 0);
    let snapshot = enable_inventory_owners(base, contracts.clone(), items.clone()).unwrap();
    assert_eq!(snapshot.schema_version, SNAPSHOT_SCHEMA_INVENTORY_OWNERS);
    assert!(snapshot.preserved_prospect.is_none());
    assert!(snapshot.native_inventory_owner_import.is_none());
    assert!(snapshot.native_scalar_contracts.is_none());
    assert!(snapshot.native_owner_import.is_none());
    assert!(snapshot.native_inventory_authority.is_some());
    SnapshotStore::preflight(&snapshot).unwrap();
    assert!(enable_inventory_owners(snapshot, contracts, items).is_err());
}

#[test]
fn equal_sized_maps_cannot_substitute_inventory_owner_classes() {
    let (original, original_contracts, items) = inputs();
    let mut definition = original.native_world.as_ref().unwrap().definitions.clone();
    let mut other_class = definition.classes[0].clone();
    other_class.key = "other_box".into();
    definition.classes.push(other_class);
    let mut other_map = definition.maps[0].clone();
    other_map.key = "other_map".into();
    for placement in &mut other_map.placements {
        placement.class = "other_box".into();
    }
    definition.maps.push(other_map);
    let catalog = Arc::new(CompiledNativeCatalog::compile(definition).unwrap());
    let base = |map: &str| {
        let mut definition = original
            .native_lifecycle
            .as_ref()
            .unwrap()
            .definition
            .clone();
        definition.native_catalog_id = catalog.identity().into();
        definition.map = map.into();
        let lifecycle = CompiledNativeLifecycle::compile(definition, &catalog).unwrap();
        prepare_lifecycle_world(
            prepare_authored_world(Arc::clone(&catalog), map)
                .unwrap()
                .initial_snapshot(),
            &lifecycle,
        )
        .unwrap()
        .into_snapshot()
    };
    let target = base("map");
    let other = base("other_map");
    assert!(
        target.native_lifecycle.as_ref().unwrap().slots
            == other.native_lifecycle.as_ref().unwrap().slots
    );
    let mut definition = original_contracts.definition;
    definition.native_catalog_id = catalog.identity().into();
    definition.native_map = "other_map".into();
    definition.classes[0].native_class = "other_box".into();
    let contracts = CompiledInventoryContractsV1::compile(definition, &catalog).unwrap();
    let contracts = contracts.snapshot().as_ref().clone();
    assert!(enable_inventory_owners(target.clone(), contracts.clone(), items.clone()).is_err());

    // Every receipt is valid for the other map. Equal owner IDs/incarnations
    // cannot make its class-specific contracts applicable to the target map.
    let valid_other = enable_inventory_owners(other, contracts, items).unwrap();
    let mut forged = valid_other.clone();
    forged.native_world = target.native_world;
    forged.native_lifecycle = target.native_lifecycle;
    forged
        .native_inventory_authority
        .as_mut()
        .unwrap()
        .lifecycle_id = forged
        .native_lifecycle
        .as_ref()
        .unwrap()
        .lifecycle_id
        .clone();
    assert!(forged.validate().is_err());
    assert!(SnapshotStore::preflight(&forged).is_err());
    assert!(build_app(&ServerConfig::default(), &forged).is_err());

    let directory = tempfile::tempdir().unwrap();
    let mut store = SnapshotStore::open(directory.path()).unwrap();
    store.save(&valid_other).unwrap();
    drop(store);
    let path = directory.path().join("snapshot-1.json");
    let mut wire: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    wire["native_world"] = serde_json::to_value(&forged.native_world).unwrap();
    wire["native_lifecycle"] = serde_json::to_value(&forged.native_lifecycle).unwrap();
    wire["native_inventory_authority"] =
        serde_json::to_value(&forged.native_inventory_authority).unwrap();
    let bytes = serde_json::to_vec(&wire).unwrap();
    fs::write(path, &bytes).unwrap();
    let path = directory.path().join("manifest.json");
    let mut manifest: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    manifest["current"]["sha256"] = json!(
        Sha256::digest(&bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    );
    fs::write(path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    assert!(
        SnapshotStore::open(directory.path())
            .unwrap()
            .load()
            .is_err()
    );
}

#[test]
fn independent_baseline_tampering_rejects_before_activation() {
    let original = encoded(&initial());
    for (pointer, replacement) in [
        (
            "/native_inventory_authority/initial_items/items/0/count",
            json!(7),
        ),
        (
            "/native_inventory_authority/initial_owners/owners/0/incarnation",
            json!(99),
        ),
        (
            "/native_inventory_authority/lifecycle_id",
            json!("0".repeat(64)),
        ),
        (
            "/native_inventory_authority/initial_state_digest",
            json!("0".repeat(64)),
        ),
        (
            "/native_inventory_authority/initial_next_stack_id",
            json!(99),
        ),
        (
            "/native_inventory_authority/initial_totals/items/0/count",
            json!(7),
        ),
    ] {
        let mut value = original.clone();
        *value.pointer_mut(pointer).unwrap() = replacement;
        if let Ok(snapshot) = serde_json::from_value::<WorldSnapshot>(value) {
            assert!(snapshot.validate().is_err(), "tamper accepted: {pointer}");
            assert!(build_app(&ServerConfig::default(), &snapshot).is_err());
        }
    }
}

#[test]
fn actions_are_atomic_and_reload_retains_conservation_and_retired_ids() {
    let mut app = build_app(&ServerConfig::default(), &initial()).unwrap();
    let owner = native_owner_handle(app.world(), DomainId(1)).unwrap();
    let other = native_owner_handle(app.world(), DomainId(2)).unwrap();
    let before = capture(app.world_mut()).unwrap();
    let state = before.native_inventories.as_ref().unwrap();
    let original_id = state.owners[0].inventories[0].slots[0].as_ref().unwrap().id;
    let split = InventoryActionRequestV1 {
        expected_inventory_revision: state.revision,
        action: InventoryActionV1::Split {
            source: endpoint(owner, original_id),
            destination: slot(owner, 1),
            amount: 3,
        },
    };
    apply_native_inventory_action(app.world_mut(), &split).unwrap();
    let after_split = capture(app.world_mut()).unwrap();
    assert!(apply_native_inventory_action(app.world_mut(), &split).is_err());
    assert_eq!(
        encoded(&capture(app.world_mut()).unwrap()),
        encoded(&after_split)
    );
    let split_id = after_split.native_inventories.as_ref().unwrap().owners[0].inventories[0].slots
        [1]
    .as_ref()
    .unwrap()
    .id;
    let actions = [
        InventoryActionV1::Move {
            source: endpoint(owner, split_id),
            destination: slot(other, 0),
        },
        InventoryActionV1::Merge {
            source: endpoint(other, split_id),
            destination: endpoint(owner, original_id),
            amount: 2,
        },
        InventoryActionV1::Consume {
            source: endpoint(other, split_id),
            amount: 1,
        },
    ];
    for action in actions {
        let revision = capture(app.world_mut())
            .unwrap()
            .native_inventories
            .unwrap()
            .revision;
        apply_native_inventory_action(
            app.world_mut(),
            &InventoryActionRequestV1 {
                expected_inventory_revision: revision,
                action,
            },
        )
        .unwrap();
    }
    let expected = capture(app.world_mut()).unwrap();
    let state = expected.native_inventories.as_ref().unwrap();
    assert_eq!(state.consumed[0].count, 1);
    assert_eq!(
        state.owners[0].inventories[0].slots[0]
            .as_ref()
            .unwrap()
            .count,
        7
    );
    assert!(state.owners[1].inventories[0].slots[0].is_none());
    let next_id = state.next_stack_id;
    let dir = tempfile::tempdir().unwrap();
    let mut store = SnapshotStore::open(dir.path()).unwrap();
    store.save(&expected).unwrap();
    drop(store);
    let store = SnapshotStore::open(dir.path()).unwrap();
    let loaded = store.load().unwrap().unwrap();
    assert!(!loaded.recovered_previous);
    assert_eq!(encoded(&loaded.snapshot), encoded(&expected));
    let mut app = build_app(&ServerConfig::default(), &loaded.snapshot).unwrap();
    apply_native_inventory_action(
        app.world_mut(),
        &InventoryActionRequestV1 {
            expected_inventory_revision: state.revision,
            action: InventoryActionV1::Split {
                source: endpoint(owner, original_id),
                destination: slot(other, 0),
                amount: 1,
            },
        },
    )
    .unwrap();
    let current = capture(app.world_mut()).unwrap();
    let allocated = current.native_inventories.unwrap().owners[1].inventories[0].slots[0]
        .as_ref()
        .unwrap()
        .id
        .get();
    assert_eq!(allocated, next_id);
    assert!(allocated > split_id.get());
}

#[test]
fn future_authority_format_never_rolls_back_to_previous_generation() {
    let dir = tempfile::tempdir().unwrap();
    let mut snapshot = initial();
    let mut store = SnapshotStore::open(dir.path()).unwrap();
    store.save(&snapshot).unwrap();
    snapshot.tick = 1;
    store.save(&snapshot).unwrap();
    drop(store);
    let path = dir.path().join("snapshot-2.json");
    let mut value: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    value["native_inventory_authority"]["format"] = json!(2);
    let bytes = serde_json::to_vec(&value).unwrap();
    fs::write(path, &bytes).unwrap();
    let manifest_path = dir.path().join("manifest.json");
    let mut manifest: Value = serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
    manifest["current"]["sha256"] = json!(
        Sha256::digest(&bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    );
    fs::write(manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    let error = SnapshotStore::open(dir.path()).unwrap().load().unwrap_err();
    assert!(format!("{error:#}").contains("unsupported native snapshot schema"));
}

#[test]
fn missing_or_disabled_authority_blocks_capture() {
    let mut app = build_app(&ServerConfig::default(), &initial()).unwrap();
    let authority = app
        .world_mut()
        .remove_resource::<AuthoredInventoryAuthorityState>()
        .unwrap();
    assert!(capture(app.world_mut()).is_err());
    app.world_mut().insert_resource(authority);
    capture(app.world_mut()).unwrap();
    let singleton = app
        .world()
        .resource_entity::<AuthoredInventoryAuthorityState>()
        .unwrap();
    app.world_mut()
        .entity_mut(singleton)
        .insert(bevy::ecs::entity_disabling::Disabled);
    assert!(capture(app.world_mut()).is_err());
    app.world_mut()
        .entity_mut(singleton)
        .remove::<bevy::ecs::entity_disabling::Disabled>();
    capture(app.world_mut()).unwrap();
}

fn execute(command: &mut Command) -> Output {
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if child.try_wait().unwrap().is_some() {
            return child.wait_with_output().unwrap();
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("authored inventory CLI exceeded deadline");
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn cli_prepares_and_creates_without_source_and_rejects_special_recipe_inputs() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let (base, contracts, items) = inputs();
    fs::write(
        root.join("catalog.toml"),
        toml::to_string(&base.native_world.as_ref().unwrap().definitions).unwrap(),
    )
    .unwrap();
    for (name, value) in [
        (
            "lifecycle",
            serde_json::to_value(&base.native_lifecycle.as_ref().unwrap().definition).unwrap(),
        ),
        (
            "contracts",
            serde_json::to_value(&contracts.definition).unwrap(),
        ),
        ("items", serde_json::to_value(&items).unwrap()),
    ] {
        fs::write(
            root.join(format!("{name}.json")),
            serde_json::to_vec(&value).unwrap(),
        )
        .unwrap();
    }
    let command = |destination: &str| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_icarus-server"));
        command
            .arg("create-native-world")
            .arg("--catalog")
            .arg(root.join("catalog.toml"))
            .args(["--map", "map"])
            .arg("--lifecycle")
            .arg(root.join("lifecycle.json"))
            .arg("--inventory-contracts")
            .arg(root.join("contracts.json"))
            .arg("--inventory-items")
            .arg(root.join("items.json"))
            .arg("--data-dir")
            .arg(root.join(destination));
        command
    };
    let prepared = execute(command("world").arg("--dry-run"));
    assert!(
        prepared.status.success(),
        "{}",
        String::from_utf8_lossy(&prepared.stderr)
    );
    let report: Value = serde_json::from_slice(&prepared.stdout).unwrap();
    assert_eq!(
        report["schema_version"],
        json!(SNAPSHOT_SCHEMA_INVENTORY_OWNERS)
    );
    assert_eq!(report["live_actors"], json!(2));
    assert!(!root.join("world").exists());
    let created = execute(&mut command("world"));
    assert!(
        created.status.success(),
        "{}",
        String::from_utf8_lossy(&created.stderr)
    );
    let loaded = SnapshotStore::open(&root.join("world"))
        .unwrap()
        .load()
        .unwrap()
        .unwrap();
    assert_eq!(
        encoded(&loaded.snapshot),
        encoded(&enable_inventory_owners(base, contracts, items).unwrap())
    );
    // Sparse oversize input is rejected before opening the destination.
    let file = fs::File::create(root.join("items.json")).unwrap();
    file.set_len(16 * 1024 * 1024 + 1).unwrap();
    drop(file);
    assert!(!execute(&mut command("oversize")).status.success());
    assert!(!root.join("oversize").exists());
    fs::remove_file(root.join("items.json")).unwrap();
    let status = Command::new("mkfifo")
        .arg(root.join("items.json"))
        .status()
        .unwrap();
    assert!(status.success());
    assert!(!execute(&mut command("fifo")).status.success());
    assert!(!root.join("fifo").exists());
}

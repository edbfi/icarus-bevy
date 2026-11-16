//! Independently authored source-only lifecycle and durable-command coverage.
use icarus_bevy::{
    config::ServerConfig,
    experimental::pawn_open::CharacterVitals,
    native_inventory::{
        InventoryActionOutcomeV1, InventoryDefinitionV1, ItemDefinitionV1, SlotRuleV1,
    },
    native_player_inventory::*,
    native_player_runtime::{
        CheckpointWriter, LeavePhase, PlayerRegistry, activate_player,
        apply_player_inventory_action,
    },
    native_players::{PlayerCheckpoint, PlayerContext, PlayerInputs, PlayerKey, PlayerSnapshot},
    persistence::SnapshotStore,
    simulation::{
        DomainId, EntityRecord, Position, SNAPSHOT_SCHEMA_PLAYER_INVENTORY, WorldSnapshot,
        build_app, capture,
    },
};
use serde_json::json;
use std::{
    collections::BTreeMap,
    fs,
    io::{Read, Seek},
    path::Path,
    process::{Command, Output, Stdio},
    sync::Mutex,
    time::{Duration, Instant},
};
static CLI_LOCK: Mutex<()> = Mutex::new(());
fn context() -> PlayerContext {
    PlayerContext::new(
        "authored-bag-world".encode_utf16().collect(),
        "/Authored/Bags".into(),
        PlayerInputs {
            authored_gameplay: [1; 32],
            spawn: [2; 32],
            authored_world: [3; 32],
            declaration: [4; 32],
            initial_values: [5; 32],
        },
        vec!["Player".into()],
    )
    .unwrap()
}
fn checkpoint() -> PlayerCheckpoint {
    PlayerCheckpoint {
        location_cm: [10., 20., 30.],
        vitals: CharacterVitals {
            health: 100,
            max_health: 100,
            stamina: 100,
            max_stamina: 100,
            oxygen: 100,
            max_oxygen: 100,
            water: 100,
            max_water: 100,
            food: 100,
            max_food: 100,
            biome_row: 0,
            growth_row: 0,
        },
    }
}
fn recipe() -> PlayerInventoryRecipeV1 {
    PlayerInventoryRecipeV1 {
        format: 1,
        items: vec![ItemDefinitionV1 {
            key: "authored-token".into(),
            stack_limit: 10,
        }],
        inventories: vec![InventoryDefinitionV1 {
            key: "bag".into(),
            initial_slots: 3,
            slot_rules: vec![
                SlotRuleV1 {
                    allowed_items: vec!["authored-token".into()]
                };
                4
            ],
            force_add_when_zero: false,
            overflow_limit: 4,
        }],
        starter: vec![PlayerStarterItemV1 {
            inventory: "bag".into(),
            requested_slot: 0,
            item: "authored-token".into(),
            count: 8,
        }],
    }
}
fn empty() -> WorldSnapshot {
    empty_recipe(recipe())
}
fn empty_recipe(recipe: PlayerInventoryRecipeV1) -> WorldSnapshot {
    WorldSnapshot {
        schema_version: SNAPSHOT_SCHEMA_PLAYER_INVENTORY,
        native_players: Some(PlayerSnapshot::empty(context())),
        player_inventory: Some(
            CompiledPlayerInventoryRecipe::compile(recipe)
                .unwrap()
                .empty(),
        ),
        ..Default::default()
    }
}
fn app() -> bevy::prelude::App {
    build_app(&ServerConfig::default(), &empty()).unwrap()
}
fn key(n: u64) -> PlayerKey {
    PlayerKey::new(9000 + n, 0).unwrap()
}
fn attach(app: &mut bevy::prelude::App, n: u64, epoch: u64) -> u64 {
    app.world_mut()
        .resource_mut::<PlayerRegistry>()
        .reserve(key(n), epoch, checkpoint())
        .unwrap();
    activate_player(app.world_mut(), epoch).unwrap().player_id
}
fn snapshot(app: &mut bevy::prelude::App) -> WorldSnapshot {
    capture(app.world_mut()).unwrap()
}
fn bag(state: &WorldSnapshot, id: u64) -> &PlayerBagV1 {
    state.player_inventory.as_ref().unwrap().bag(id).unwrap()
}
fn endpoint(state: &WorldSnapshot, id: u64, slot: usize) -> PlayerStackEndpointV1 {
    let b = bag(state, id);
    PlayerStackEndpointV1 {
        owner: b.owner,
        inventory: "bag".into(),
        stack_id: b.inventories[0].slots[slot].as_ref().unwrap().id,
    }
}
fn destination(id: u64, slot: u16) -> PlayerSlotEndpointV1 {
    PlayerSlotEndpointV1 {
        owner: PlayerInventoryOwnerV1 {
            player_id: id,
            incarnation: 1,
        },
        inventory: "bag".into(),
        slot,
    }
}
fn request(
    state: &WorldSnapshot,
    id: u64,
    action: PlayerInventoryActionV1,
) -> PlayerInventoryRequestV1 {
    PlayerInventoryRequestV1 {
        player_id: id,
        expected_player_revision: state
            .native_players
            .as_ref()
            .unwrap()
            .projection_by_id(id)
            .unwrap()
            .revision,
        expected_bag_revision: bag(state, id).revision,
        action,
    }
}
fn consume(state: &WorldSnapshot, id: u64, amount: u32) -> PlayerInventoryRequestV1 {
    request(
        state,
        id,
        PlayerInventoryActionV1::Consume {
            source: endpoint(state, id, 0),
            amount,
        },
    )
}
fn apply(
    app: &mut bevy::prelude::App,
    n: u64,
    epoch: u64,
    request: &PlayerInventoryRequestV1,
) -> InventoryActionOutcomeV1 {
    apply_player_inventory_action(app.world_mut(), key(n), epoch, request).unwrap()
}
fn reload(state: &WorldSnapshot) -> WorldSnapshot {
    let dir = tempfile::tempdir().unwrap();
    SnapshotStore::open(dir.path())
        .unwrap()
        .save(state)
        .unwrap();
    SnapshotStore::open_existing(dir.path())
        .unwrap()
        .load()
        .unwrap()
        .unwrap()
        .snapshot
}

#[test]
fn sparse_reservations_once_only_issuance_detach_restart_and_operations() {
    let mut app = app();
    {
        let mut r = app.world_mut().resource_mut::<PlayerRegistry>();
        r.reserve(key(0), 1, checkpoint()).unwrap();
        r.cancel(1);
        r.reserve(key(1), 2, checkpoint()).unwrap();
        r.reserve(key(2), 3, checkpoint()).unwrap();
        assert!(r.activate(2).is_err());
        assert_eq!(r.snapshot().len(), 0);
    }
    let b = activate_player(app.world_mut(), 3).unwrap().player_id;
    let a = activate_player(app.world_mut(), 2).unwrap().player_id;
    assert_eq!((a, b), (2, 3));
    let initial = snapshot(&mut app);
    assert_eq!(initial.player_inventory.as_ref().unwrap().next_stack_id, 3);
    activate_player(app.world_mut(), 2).unwrap();
    assert_eq!(snapshot(&mut app), initial);
    let split = request(
        &initial,
        a,
        PlayerInventoryActionV1::Split {
            source: endpoint(&initial, a, 0),
            destination: destination(a, 1),
            amount: 3,
        },
    );
    assert_eq!(
        apply(&mut app, 1, 2, &split),
        InventoryActionOutcomeV1::Applied
    );
    let state = snapshot(&mut app);
    assert_eq!(bag(&state, b), bag(&initial, b));
    assert_eq!(bag(&state, a).revision, 2);
    assert!(apply_player_inventory_action(app.world_mut(), key(1), 2, &split).is_err());
    assert_eq!(snapshot(&mut app), state);
    let noop = request(
        &state,
        a,
        PlayerInventoryActionV1::Move {
            source: endpoint(&state, a, 1),
            destination: destination(a, 1),
        },
    );
    assert_eq!(
        apply(&mut app, 1, 2, &noop),
        InventoryActionOutcomeV1::NoChange
    );
    assert_eq!(snapshot(&mut app), state);
    let mv = request(
        &state,
        a,
        PlayerInventoryActionV1::Move {
            source: endpoint(&state, a, 1),
            destination: destination(a, 2),
        },
    );
    apply(&mut app, 1, 2, &mv);
    let state = snapshot(&mut app);
    let retired = endpoint(&state, a, 2).stack_id;
    let merge = request(
        &state,
        a,
        PlayerInventoryActionV1::Merge {
            source: endpoint(&state, a, 2),
            destination: endpoint(&state, a, 0),
            amount: 3,
        },
    );
    apply(&mut app, 1, 2, &merge);
    let state = snapshot(&mut app);
    let req = consume(&state, a, 8);
    apply(&mut app, 1, 2, &req);
    let state = snapshot(&mut app);
    assert_eq!(bag(&state, a).consumed[0].count, 8);
    assert_eq!(state.player_inventory.as_ref().unwrap().next_stack_id, 4);
    {
        let mut registry = app.world_mut().resource_mut::<PlayerRegistry>();
        registry.detach(2);
        registry.detach(3);
    }
    let retained = snapshot(&mut app);
    let mut encoded = serde_json::to_value(&retained).unwrap();
    encoded["native_players"]["records"]
        .as_array_mut()
        .unwrap()
        .reverse();
    let reordered: WorldSnapshot = serde_json::from_value(encoded).unwrap();
    reordered.validate().unwrap();
    let loaded = reload(&reordered);
    assert_eq!(loaded, retained);
    let mut app = build_app(&ServerConfig::default(), &loaded).unwrap();
    assert_eq!(attach(&mut app, 1, 10), a);
    assert_eq!(attach(&mut app, 2, 11), b);
    let state = snapshot(&mut app);
    let req = request(
        &state,
        b,
        PlayerInventoryActionV1::Split {
            source: endpoint(&state, b, 0),
            destination: destination(b, 1),
            amount: 1,
        },
    );
    apply(&mut app, 2, 11, &req);
    assert!(endpoint(&snapshot(&mut app), b, 1).stack_id.get() > retired.get());
    assert_eq!(bag(&snapshot(&mut app), a).consumed[0].count, 8);
}

#[test]
fn same_player_epoch_cas_and_endpoint_authority_rejections_are_atomic() {
    let mut app = app();
    let a = attach(&mut app, 1, 1);
    let b = attach(&mut app, 2, 2);
    let state = snapshot(&mut app);
    let req = consume(&state, a, 1);
    for (k, epoch) in [(key(2), 1), (key(1), 2), (key(1), 0), (key(1), 999)] {
        assert!(apply_player_inventory_action(app.world_mut(), k, epoch, &req).is_err());
    }
    let cross = request(
        &state,
        a,
        PlayerInventoryActionV1::Move {
            source: endpoint(&state, a, 0),
            destination: destination(b, 1),
        },
    );
    assert!(apply_player_inventory_action(app.world_mut(), key(1), 1, &cross).is_err());
    for action in [
        PlayerInventoryActionV1::Split {
            source: endpoint(&state, a, 0),
            destination: destination(b, 1),
            amount: 1,
        },
        PlayerInventoryActionV1::Merge {
            source: endpoint(&state, a, 0),
            destination: endpoint(&state, b, 0),
            amount: 1,
        },
        PlayerInventoryActionV1::Consume {
            source: endpoint(&state, b, 0),
            amount: 1,
        },
        PlayerInventoryActionV1::PlaceOverflow {
            source: endpoint(&state, a, 0),
            destination: PlayerOverflowDestinationV1 {
                owner: bag(&state, b).owner,
                inventory: "bag".into(),
                slot: Some(1),
            },
        },
    ] {
        let cross = request(&state, a, action);
        assert!(apply_player_inventory_action(app.world_mut(), key(1), 1, &cross).is_err());
        assert_eq!(snapshot(&mut app), state);
    }
    let mut stale = req.clone();
    stale.expected_bag_revision += 1;
    assert!(apply_player_inventory_action(app.world_mut(), key(1), 1, &stale).is_err());
    stale = req.clone();
    stale.expected_player_revision += 1;
    assert!(apply_player_inventory_action(app.world_mut(), key(1), 1, &stale).is_err());
    {
        let mut r = app.world_mut().resource_mut::<PlayerRegistry>();
        r.detach(1);
        r.reserve(key(1), 3, checkpoint()).unwrap();
    }
    assert!(apply_player_inventory_action(app.world_mut(), key(1), 1, &req).is_err());
    assert!(apply_player_inventory_action(app.world_mut(), key(1), 3, &req).is_err());
    assert_eq!(snapshot(&mut app), state);
    activate_player(app.world_mut(), 3).unwrap();
    apply(&mut app, 1, 3, &req);
    assert!(!format!("{req:?}").contains("authored-token"));
}

#[test]
fn zero_slot_and_overflow_paths_survive_reload_without_id_reuse() {
    let mut r = recipe();
    r.inventories[0].initial_slots = 0;
    r.inventories[0].force_add_when_zero = true;
    r.starter.push(PlayerStarterItemV1 {
        inventory: "bag".into(),
        requested_slot: 255,
        item: "authored-token".into(),
        count: 2,
    });
    let mut app = build_app(&ServerConfig::default(), &empty_recipe(r)).unwrap();
    let a = attach(&mut app, 1, 1);
    let initial = snapshot(&mut app);
    let b = bag(&initial, a);
    assert_eq!(b.inventories[0].slots.len(), 1);
    assert_eq!(b.inventories[0].overflow.len(), 1);
    let source = PlayerStackEndpointV1 {
        owner: b.owner,
        inventory: "bag".into(),
        stack_id: b.inventories[0].overflow[0].stack.id,
    };
    let req = request(
        &initial,
        a,
        PlayerInventoryActionV1::PlaceOverflow {
            source,
            destination: PlayerOverflowDestinationV1 {
                owner: b.owner,
                inventory: "bag".into(),
                slot: None,
            },
        },
    );
    let loaded = reload(&initial);
    let mut app = build_app(&ServerConfig::default(), &loaded).unwrap();
    attach(&mut app, 1, 2);
    apply(&mut app, 1, 2, &req);
    let state = snapshot(&mut app);
    assert_eq!(bag(&state, a).inventories[0].slots.len(), 2);
    assert_eq!(state.player_inventory.as_ref().unwrap().next_stack_id, 3);
    assert_eq!(reload(&state), state);
    let mut invalid = recipe();
    invalid.inventories[0].initial_slots = 0;
    invalid.inventories[0].force_add_when_zero = false;
    assert!(CompiledPlayerInventoryRecipe::compile(invalid).is_err());
}

#[test]
fn player_local_conservation_rejects_two_player_redistribution_on_reload() {
    let mut app = app();
    let a = attach(&mut app, 1, 1);
    let b = attach(&mut app, 2, 2);
    let state = snapshot(&mut app);
    apply(&mut app, 1, 1, &consume(&state, a, 8));
    let mut malformed = snapshot(&mut app);
    let i = malformed.player_inventory.as_mut().unwrap();
    let moved = i
        .bags
        .iter_mut()
        .find(|o| o.owner.player_id == b)
        .unwrap()
        .inventories[0]
        .slots[0]
        .take();
    i.bags
        .iter_mut()
        .find(|o| o.owner.player_id == a)
        .unwrap()
        .inventories[0]
        .slots[0] = moved;
    assert_eq!(i.bags.iter().map(|b| b.consumed[0].count).sum::<u64>(), 8);
    assert!(malformed.validate().is_err());
    assert!(build_app(&ServerConfig::default(), &malformed).is_err());
    // Exercise the actual checksummed decoder, not just in-memory validation.
    let dir = tempfile::tempdir().unwrap();
    let mut store = SnapshotStore::open(dir.path()).unwrap();
    store.save(&state).unwrap();
    drop(store);
    replace_current(dir.path(), &serde_json::to_value(malformed).unwrap());
    assert!(
        SnapshotStore::open_existing(dir.path())
            .unwrap()
            .load()
            .is_err()
    );
}
fn replace_current(root: &Path, value: &serde_json::Value) {
    use sha2::{Digest, Sha256};
    let manifest = root.join("manifest.json");
    let mut m: serde_json::Value = serde_json::from_slice(&fs::read(&manifest).unwrap()).unwrap();
    let number = m["current"]["number"].as_u64().unwrap();
    let bytes = serde_json::to_vec(value).unwrap();
    let hash = Sha256::digest(&bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    m["current"]["sha256"] = json!(hash);
    fs::write(root.join(format!("snapshot-{number}.json")), bytes).unwrap();
    fs::write(manifest, serde_json::to_vec(&m).unwrap()).unwrap();
}

#[test]
fn strict_corruption_recipe_matching_and_future_format_refuse() {
    let mut app = app();
    let a = attach(&mut app, 1, 1);
    let state = snapshot(&mut app);
    for kind in 0..7 {
        let mut bad = state.clone();
        let i = bad.player_inventory.as_mut().unwrap();
        match kind {
            0 => i.bags.clear(),
            1 => i.bags.push(i.bags[0].clone()),
            2 => i.bags[0].owner.player_id = 99,
            3 => i.baseline.items[0].count += 1,
            4 => match &mut i.recipe {
                PlayerInventoryRecipe::V1(v) => v.starter[0].count -= 1,
                _ => unreachable!(),
            },
            5 => i.bags[0].inventories[0].slots[0].as_mut().unwrap().count += 1,
            _ => i.next_stack_id = 1,
        }
        assert!(bad.validate().is_err(), "accepted corruption {kind}");
    }
    assert_eq!(
        state
            .native_players
            .as_ref()
            .unwrap()
            .projection_by_id(a)
            .unwrap()
            .checkpoint,
        checkpoint()
    );
    let mut disabled = state.clone();
    assert!(icarus_bevy::network::open_player_snapshot(&mut disabled, None).is_err());
    assert_eq!(disabled, state);
    for field in ["section", "recipe", "schema"] {
        let dir = tempfile::tempdir().unwrap();
        {
            let mut store = SnapshotStore::open(dir.path()).unwrap();
            store.save(&state).unwrap();
            store.save(&state).unwrap();
        }
        let mut v = serde_json::to_value(&state).unwrap();
        match field {
            "section" => v["player_inventory"]["format"] = json!(2),
            "recipe" => v["player_inventory"]["recipe"]["format"] = json!(3),
            _ => v["schema_version"] = json!(16),
        }
        replace_current(dir.path(), &v);
        let store = SnapshotStore::open_existing(dir.path()).unwrap();
        assert!(store.load().is_err(), "fell back for {field}");
    }
}

#[test]
fn exact_player_leave_pin_allows_other_bags_to_advance_and_retains_detached_cutoff() {
    let mut app = app();
    let a = attach(&mut app, 1, 1);
    let b = attach(&mut app, 2, 2);
    let state = snapshot(&mut app);
    apply(&mut app, 1, 1, &consume(&state, a, 2));
    let state = snapshot(&mut app);
    let revision = state
        .native_players
        .as_ref()
        .unwrap()
        .projection_by_id(a)
        .unwrap()
        .revision;
    app.world_mut()
        .resource_mut::<PlayerRegistry>()
        .request_leave(key(1), 1, revision, Instant::now(), Duration::from_secs(30))
        .unwrap();
    assert!(
        apply_player_inventory_action(app.world_mut(), key(1), 1, &consume(&state, a, 1)).is_err()
    );
    assert!(
        app.world_mut()
            .resource_mut::<PlayerRegistry>()
            .edit(key(1), revision, checkpoint())
            .is_err()
    );
    apply(&mut app, 2, 2, &consume(&state, b, 1));
    let state = snapshot(&mut app);
    assert_eq!(bag(&state, a).consumed[0].count, 2);
    assert_eq!(bag(&state, b).consumed[0].count, 1);
    let dir = tempfile::tempdir().unwrap();
    let mut writer = CheckpointWriter::spawn(SnapshotStore::open(dir.path()).unwrap()).unwrap();
    writer.submit(app.world_mut()).unwrap();
    app.world_mut().resource_mut::<PlayerRegistry>().detach(1);
    writer
        .finish_before(app.world_mut(), Instant::now() + Duration::from_secs(30))
        .unwrap();
    assert!(matches!(
        app.world()
            .resource::<PlayerRegistry>()
            .leave_status(key(1), 1)
            .unwrap()
            .phase,
        LeavePhase::Durable { .. }
    ));
    assert_eq!(reload(&state), state);
}

#[test]
fn full_generation_split_capacity_rejection_preserves_every_domain() {
    let mut r = recipe();
    let long = "a".repeat(64);
    r.items[0].key = long.clone();
    for rule in &mut r.inventories[0].slot_rules {
        rule.allowed_items = vec![long.clone()];
    }
    r.starter[0].item = long;
    let mut app = build_app(&ServerConfig::default(), &empty_recipe(r)).unwrap();
    let a = attach(&mut app, 1, 1);
    let mut state = snapshot(&mut app);
    let cap = 64 * 1024 * 1024;
    let mut length = serde_json::to_vec(&state).unwrap().len();
    let mut id = 1u64;
    loop {
        let entity = EntityRecord {
            id: DomainId(id),
            position: Position([0., 0., 0.]),
        };
        let delta =
            serde_json::to_vec(&entity).unwrap().len() + usize::from(!state.entities.is_empty());
        if length + delta > cap {
            break;
        }
        length += delta;
        state.entities.push(entity);
        id += 1;
    }
    SnapshotStore::preflight(&state).unwrap();
    let req = request(
        &state,
        a,
        PlayerInventoryActionV1::Split {
            source: endpoint(&state, a, 0),
            destination: destination(a, 1),
            amount: 1,
        },
    );
    // Prove inventory-only state and CAS admit this exact request.
    let mut small = state.clone();
    small.entities.clear();
    assert_eq!(
        prepare_offline(&small, &req).unwrap().1,
        InventoryActionOutcomeV1::Applied
    );
    drop(small);
    let mut app = build_app(&ServerConfig::default(), &state).unwrap();
    attach(&mut app, 1, 10);
    let before = snapshot(&mut app);
    assert!(apply_player_inventory_action(app.world_mut(), key(1), 10, &req).is_err());
    assert_eq!(snapshot(&mut app), before);
    // Activation of a pending second player must obey the same complete cap.
    app.world_mut()
        .resource_mut::<PlayerRegistry>()
        .reserve(key(2), 11, checkpoint())
        .unwrap();
    let reserved = snapshot(&mut app);
    assert!(activate_player(app.world_mut(), 11).is_err());
    assert_eq!(snapshot(&mut app), reserved);
    assert!(!app.world().resource::<PlayerRegistry>().is_activated(11));
}

fn disk(root: &Path) -> BTreeMap<String, Vec<u8>> {
    fs::read_dir(root)
        .unwrap()
        .map(|e| {
            let e = e.unwrap();
            (
                e.file_name().into_string().unwrap(),
                fs::read(e.path()).unwrap(),
            )
        })
        .collect()
}
fn execute(root: &Path, path: &Path, dry: bool) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_icarus-server"));
    command
        .arg("apply-player-inventory")
        .arg("--data-dir")
        .arg(root)
        .arg("--request")
        .arg(path);
    if dry {
        command.arg("--dry-run");
    }
    let mut out = tempfile::tempfile().unwrap();
    let mut err = tempfile::tempfile().unwrap();
    let mut child = command
        .stdout(Stdio::from(out.try_clone().unwrap()))
        .stderr(Stdio::from(err.try_clone().unwrap()))
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    let status = loop {
        if let Some(s) = child.try_wait().unwrap() {
            break s;
        }
        if Instant::now() > deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("offline command exceeded deadline");
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    out.rewind().unwrap();
    err.rewind().unwrap();
    let mut stdout = vec![];
    let mut stderr = vec![];
    out.read_to_end(&mut stdout).unwrap();
    err.read_to_end(&mut stderr).unwrap();
    Output {
        status,
        stdout,
        stderr,
    }
}
#[test]
fn durable_cli_applied_receipt_reopen_and_nonwrites() {
    let _lock = CLI_LOCK.lock().unwrap();
    let mut app = app();
    let a = attach(&mut app, 1, 1);
    let state = snapshot(&mut app);
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("state");
    let req = dir.path().join("request.json");
    SnapshotStore::open(&root).unwrap().save(&state).unwrap();
    let request = consume(&state, a, 2);
    fs::write(&req, serde_json::to_vec(&request).unwrap()).unwrap();
    let before = disk(&root);
    assert!(execute(&root, &req, true).status.success());
    assert_eq!(disk(&root), before);
    {
        let _owner = SnapshotStore::open_existing(&root).unwrap();
        assert!(!execute(&root, &req, false).status.success());
    }
    assert_eq!(disk(&root), before);
    let output = execute(&root, &req, false);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["outcome"], "applied");
    assert_eq!(report["receipt"]["generation"], 2);
    assert_eq!(report["receipt"]["tick"], state.tick);
    let loaded = SnapshotStore::open_existing(&root)
        .unwrap()
        .load()
        .unwrap()
        .unwrap()
        .snapshot;
    assert_eq!(bag(&loaded, a).consumed[0].count, 2);
    let after = disk(&root);
    assert!(!execute(&root, &req, false).status.success());
    assert_eq!(disk(&root), after);
    let noop = request_for_noop(&loaded, a);
    fs::write(&req, serde_json::to_vec(&noop).unwrap()).unwrap();
    let output = execute(&root, &req, false);
    assert!(output.status.success());
    assert!(!String::from_utf8_lossy(&output.stdout).contains("receipt"));
    assert_eq!(disk(&root), after);
    for bytes in [b"{\"private_unknown\":true}".to_vec(), vec![b' '; 65537]] {
        fs::write(&req, bytes).unwrap();
        let output = execute(&root, &req, false);
        assert!(!output.status.success());
        assert!(!String::from_utf8_lossy(&output.stderr).contains("private_unknown"));
        assert_eq!(disk(&root), after);
    }
}
#[test]
fn positional_inventory_requests_are_rejected_without_publication() {
    let _lock = CLI_LOCK.lock().unwrap();
    let mut app = app();
    let player = attach(&mut app, 1, 1);
    let state = snapshot(&mut app);
    let request = consume(&state, player, 1);
    let named = serde_json::to_value(&request).unwrap();
    let positional = json!([
        request.player_id,
        request.expected_player_revision,
        request.expected_bag_revision,
        named["action"]
    ]);
    let mut positional_owner = named.clone();
    positional_owner["action"]["source"]["owner"] = json!([player, 1]);
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("state");
    let path = dir.path().join("request.json");
    SnapshotStore::open(&root).unwrap().save(&state).unwrap();
    let before = disk(&root);
    for malformed in [positional, positional_owner] {
        fs::write(&path, serde_json::to_vec(&malformed).unwrap()).unwrap();
        let output = execute(&root, &path, false);
        assert!(!output.status.success(), "positional request was published");
        assert!(!String::from_utf8_lossy(&output.stdout).contains("receipt"));
        assert_eq!(disk(&root), before);
        assert!(serde_json::from_value::<PlayerInventoryRequestV1>(malformed).is_err());
    }
    // The same named request remains valid at the unchanged revisions.
    fs::write(&path, serde_json::to_vec(&named).unwrap()).unwrap();
    assert!(execute(&root, &path, false).status.success());
    let loaded = SnapshotStore::open_existing(&root)
        .unwrap()
        .load()
        .unwrap()
        .unwrap()
        .snapshot;
    assert_eq!(bag(&loaded, player).consumed[0].count, 1);
}

#[test]
fn positional_saved_inventory_owner_is_rejected() {
    let mut app = app();
    let player = attach(&mut app, 1, 1);
    let state = snapshot(&mut app);
    let dir = tempfile::tempdir().unwrap();
    SnapshotStore::open(dir.path())
        .unwrap()
        .save(&state)
        .unwrap();
    let mut malformed = serde_json::to_value(&state).unwrap();
    malformed["player_inventory"]["bags"][0]["owner"] = json!([player, 1]);
    replace_current(dir.path(), &malformed);
    let before = disk(dir.path());
    let store = SnapshotStore::open_existing(dir.path()).unwrap();
    assert!(store.load().is_err(), "positional saved owner was admitted");
    assert_eq!(disk(dir.path()), before);
    assert!(serde_json::from_value::<WorldSnapshot>(malformed).is_err());
}

fn request_for_noop(state: &WorldSnapshot, a: u64) -> PlayerInventoryRequestV1 {
    request(
        state,
        a,
        PlayerInventoryActionV1::Move {
            source: endpoint(state, a, 0),
            destination: destination(a, 0),
        },
    )
}

#[test]
fn allocation_and_revision_exhaustion_cannot_partially_issue_or_edit() {
    let mut app = app();
    let a = attach(&mut app, 1, 1);
    let state = snapshot(&mut app);
    for kind in [
        "allocator",
        "bag_revision",
        "global_revision",
        "player_revision",
    ] {
        let mut state = state.clone();
        match kind {
            "allocator" => state.player_inventory.as_mut().unwrap().next_stack_id = u64::MAX,
            "bag_revision" => {
                let mut value = serde_json::to_value(&state).unwrap();
                value["native_players"]["records"][0]["revision"] = json!(u64::MAX);
                state = serde_json::from_value(value).unwrap();
                state.player_inventory.as_mut().unwrap().bags[0].revision = u64::MAX;
            }
            "global_revision" => state.player_inventory.as_mut().unwrap().revision = u64::MAX,
            _ => {
                let mut value = serde_json::to_value(&state).unwrap();
                value["native_players"]["records"][0]["revision"] = json!(u64::MAX);
                state = serde_json::from_value(value).unwrap();
            }
        }
        let mut app = build_app(&ServerConfig::default(), &state).unwrap();
        attach(&mut app, 1, 10);
        let before = snapshot(&mut app);
        let req = request(
            &before,
            a,
            PlayerInventoryActionV1::Split {
                source: endpoint(&before, a, 0),
                destination: destination(a, 1),
                amount: 1,
            },
        );
        assert!(
            apply_player_inventory_action(app.world_mut(), key(1), 10, &req).is_err(),
            "accepted {kind}"
        );
        assert_eq!(snapshot(&mut app), before);
        let noop = request_for_noop(&before, a);
        assert_eq!(
            apply(&mut app, 1, 10, &noop),
            InventoryActionOutcomeV1::NoChange
        );
        assert_eq!(snapshot(&mut app), before);
        if matches!(kind, "allocator" | "global_revision") {
            app.world_mut()
                .resource_mut::<PlayerRegistry>()
                .reserve(key(2), 11, checkpoint())
                .unwrap();
            let before = snapshot(&mut app);
            assert!(activate_player(app.world_mut(), 11).is_err());
            assert_eq!(snapshot(&mut app), before);
        }
    }
}

#[test]
fn recipe_and_generation_limits_admit_before_clone_and_preserve_existing_owners() {
    let mut oversized = recipe();
    oversized.inventories[0].slot_rules = vec![
        SlotRuleV1 {
            allowed_items: vec!["authored-token".into()]
        };
        257
    ];
    assert!(CompiledPlayerInventoryRecipe::compile(oversized).is_err());
    let mut app = app();
    let a = attach(&mut app, 1, 1);
    let original = snapshot(&mut app);
    // Capacity, rather than length, is part of admission for retained vectors.
    let mut oversized = original.clone();
    oversized
        .player_inventory
        .as_mut()
        .unwrap()
        .bags
        .reserve_exact(9000);
    assert!(oversized.validate().is_err());
    assert_eq!(snapshot(&mut app), original);
    let mut wrong = consume(&original, a, 1);
    if let PlayerInventoryActionV1::Consume { source, .. } = &mut wrong.action {
        source.owner.incarnation = 2;
    }
    assert!(apply_player_inventory_action(app.world_mut(), key(1), 1, &wrong).is_err());
    assert_eq!(snapshot(&mut app), original);
}

#[test]
fn global_stack_limit_rejection_preserves_all_players_and_allocator() {
    use icarus_bevy::native_inventory::StackId;
    let mut r = recipe();
    r.inventories[0].initial_slots = 256;
    r.inventories[0].slot_rules = vec![
        SlotRuleV1 {
            allowed_items: vec!["authored-token".into()]
        };
        256
    ];
    r.starter = (0..256)
        .map(|slot| PlayerStarterItemV1 {
            inventory: "bag".into(),
            requested_slot: slot,
            item: "authored-token".into(),
            count: 8,
        })
        .collect();
    let mut spare = r.inventories[0].clone();
    spare.key = "spare".into();
    spare.initial_slots = 1;
    spare.slot_rules.truncate(1);
    r.inventories.push(spare);
    let mut one = build_app(&ServerConfig::default(), &empty_recipe(r)).unwrap();
    attach(&mut one, 1, 1);
    let first = snapshot(&mut one);
    let template = bag(&first, 1).clone();
    let mut players =
        PlayerRegistry::new(PlayerSnapshot::empty(context()), &context(), 64).unwrap();
    for n in 1..=256 {
        players.reserve(key(n), n, checkpoint()).unwrap();
        players.activate(n).unwrap();
        players.detach(n);
    }
    let mut state = first;
    state.native_players = Some(players.snapshot());
    let inventory = state.player_inventory.as_mut().unwrap();
    inventory.bags.clear();
    for id in 1..=256 {
        let mut bag = template.clone();
        bag.owner.player_id = id;
        for stack in bag
            .inventories
            .iter_mut()
            .flat_map(|i| i.slots.iter_mut().flatten())
        {
            stack.id = StackId::new((id - 1) * 256 + stack.id.get()).unwrap();
        }
        inventory.bags.push(bag);
    }
    inventory.next_stack_id = 65537;
    state.validate().unwrap();
    let mut app = build_app(&ServerConfig::default(), &state).unwrap();
    attach(&mut app, 1, 300);
    let before = snapshot(&mut app);
    let mut target = destination(1, 0);
    target.inventory = "spare".into();
    let req = request(
        &before,
        1,
        PlayerInventoryActionV1::Split {
            source: endpoint(&before, 1, 0),
            destination: target,
            amount: 1,
        },
    );
    assert!(apply_player_inventory_action(app.world_mut(), key(1), 300, &req).is_err());
    assert_eq!(snapshot(&mut app), before);
}

#[test]
fn recipe_selection_is_explicit_and_never_promotes_schema14() {
    use icarus_bevy::{
        experimental::release::{DEDICATED_SERVER_APP_ID, ReleasePin, network_checksum},
        network::{PreparedPlayers, fresh_player_snapshot, open_player_snapshot},
    };
    let version = "1.0.0.1";
    let release = ReleasePin {
        app_id: DEDICATED_SERVER_APP_ID,
        build_id: 1,
        project_version: version.into(),
        network_checksum: network_checksum(version),
    };
    let mut prepared = PreparedPlayers {
        context: context(),
        initial_vitals: checkpoint().vitals,
        release,
        inventory: Some(CompiledPlayerInventoryRecipe::compile(recipe()).unwrap()),
    };
    let fresh = fresh_player_snapshot(Some(&prepared));
    assert_eq!(fresh.schema_version, 15);
    let mut unchanged = fresh.clone();
    open_player_snapshot(&mut unchanged, Some(&prepared)).unwrap();
    let mut changed = recipe();
    changed.starter[0].count -= 1;
    prepared.inventory = Some(CompiledPlayerInventoryRecipe::compile(changed).unwrap());
    let mut live = build_app(&ServerConfig::default(), &fresh).unwrap();
    assert!(
        icarus_bevy::network::install_player_registry(
            live.world_mut(),
            Some(&prepared),
            &Default::default()
        )
        .is_err()
    );
    assert_eq!(snapshot(&mut live), fresh);
    let mut copy = fresh.clone();
    assert!(
        open_player_snapshot(&mut copy, Some(&prepared))
            .unwrap_err()
            .to_string()
            .contains("network.player_inventory")
    );
    assert_eq!(copy, fresh);
    prepared.inventory = None;
    let schema14 = fresh_player_snapshot(Some(&prepared));
    assert_eq!(schema14.schema_version, 14);
    prepared.inventory = Some(CompiledPlayerInventoryRecipe::compile(recipe()).unwrap());
    let mut copy = schema14.clone();
    assert!(open_player_snapshot(&mut copy, Some(&prepared)).is_err());
    assert_eq!(copy, schema14);
    let mut copy = fresh.clone();
    prepared.inventory = None;
    assert!(open_player_snapshot(&mut copy, Some(&prepared)).is_err());
    assert_eq!(copy, fresh);
}

#[test]
fn leave_old_equal_tick_submission_retry_and_cancel_cannot_complete_new_cutoff() {
    use icarus_bevy::native_player_runtime::LeaveFailure;
    let mut app = app();
    let a = attach(&mut app, 1, 1);
    let b = attach(&mut app, 2, 2);
    let dir = tempfile::tempdir().unwrap();
    let mut writer = CheckpointWriter::spawn(SnapshotStore::open(dir.path()).unwrap()).unwrap();
    writer.submit(app.world_mut()).unwrap();
    let state = snapshot(&mut app);
    apply(&mut app, 1, 1, &consume(&state, a, 1));
    let revision = app
        .world()
        .resource::<PlayerRegistry>()
        .projection(1)
        .unwrap()
        .revision;
    app.world_mut()
        .resource_mut::<PlayerRegistry>()
        .request_leave(key(1), 1, revision, Instant::now(), Duration::from_secs(30))
        .unwrap();
    writer
        .finish_before(app.world_mut(), Instant::now() + Duration::from_secs(30))
        .unwrap();
    assert_eq!(
        app.world()
            .resource::<PlayerRegistry>()
            .leave_status(key(1), 1)
            .unwrap()
            .phase,
        LeavePhase::Pending
    );
    writer.submit(app.world_mut()).unwrap();
    assert!(
        writer
            .settle_before(app.world_mut(), Instant::now())
            .is_err()
    );
    assert_eq!(
        app.world()
            .resource::<PlayerRegistry>()
            .leave_status(key(1), 1)
            .unwrap()
            .phase,
        LeavePhase::Failed(LeaveFailure::Deadline)
    );
    app.world_mut()
        .resource_mut::<PlayerRegistry>()
        .retry_leave(key(1), 1, Instant::now(), Duration::from_secs(30))
        .unwrap();
    writer
        .finish_before(app.world_mut(), Instant::now() + Duration::from_secs(30))
        .unwrap();
    assert_eq!(
        app.world()
            .resource::<PlayerRegistry>()
            .leave_status(key(1), 1)
            .unwrap()
            .phase,
        LeavePhase::Pending
    );
    let state = snapshot(&mut app);
    apply(&mut app, 2, 2, &consume(&state, b, 1));
    writer.submit(app.world_mut()).unwrap();
    writer
        .finish_before(app.world_mut(), Instant::now() + Duration::from_secs(30))
        .unwrap();
    assert!(matches!(
        app.world()
            .resource::<PlayerRegistry>()
            .leave_status(key(1), 1)
            .unwrap()
            .phase,
        LeavePhase::Durable { .. }
    ));
    app.world_mut().resource_mut::<PlayerRegistry>().detach(1);
    app.world_mut()
        .resource_mut::<PlayerRegistry>()
        .settle_leave(key(1), 1)
        .unwrap();
    attach(&mut app, 1, 3);
    let state = snapshot(&mut app);
    apply(&mut app, 1, 3, &consume(&state, a, 1));
}

#[test]
fn durable_cli_save_failure_has_no_success_receipt() {
    let _lock = CLI_LOCK.lock().unwrap();
    let mut app = app();
    let a = attach(&mut app, 1, 1);
    let state = snapshot(&mut app);
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("state");
    SnapshotStore::open(&root).unwrap().save(&state).unwrap();
    let req = dir.path().join("request.json");
    fs::write(&req, serde_json::to_vec(&consume(&state, a, 1)).unwrap()).unwrap();
    // A directory at the next generation name makes publication fail on rename.
    fs::create_dir(root.join("snapshot-2.json")).unwrap();
    let output = execute(&root, &req, false);
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(!String::from_utf8_lossy(&output.stderr).contains("receipt"));
    let loaded = SnapshotStore::open_existing(&root)
        .unwrap()
        .load()
        .unwrap()
        .unwrap()
        .snapshot;
    assert_eq!(loaded, state);
}

#[test]
fn survival_preserves_inventory_and_capture_refuses_missing_or_substituted_authority() {
    use icarus_bevy::native_player_runtime::ConsumptionRates;
    let mut app = app();
    let a = attach(&mut app, 1, 1);
    let before = snapshot(&mut app);
    assert!(
        app.world_mut()
            .resource_mut::<PlayerRegistry>()
            .edit(key(1), 1, checkpoint())
            .is_err()
    );
    let mut edited = checkpoint();
    edited.location_cm = [99., 88., 77.];
    let projection = icarus_bevy::native_player_runtime::edit_player_checkpoint(
        app.world_mut(),
        key(1),
        1,
        1,
        edited,
    )
    .unwrap();
    assert_eq!(projection.revision, 2);
    assert!(
        icarus_bevy::native_player_runtime::edit_player_checkpoint(
            app.world_mut(),
            key(1),
            1,
            1,
            checkpoint()
        )
        .is_err()
    );
    assert_eq!(snapshot(&mut app).player_inventory, before.player_inventory);

    icarus_bevy::native_player_runtime::consume_player_vitals(
        app.world_mut(),
        ConsumptionRates::new(1, 2, 3).unwrap(),
    )
    .unwrap();
    let after = snapshot(&mut app);
    assert_eq!(after.player_inventory, before.player_inventory);
    assert_ne!(after.native_players, before.native_players);
    assert_eq!(reload(&after), after);
    let stale = consume(&before, a, 1);
    assert!(apply_player_inventory_action(app.world_mut(), key(1), 1, &stale).is_err());
    assert_eq!(snapshot(&mut app), after);
    let different = empty();
    let replacement = PlayerRegistry::new_with_inventory(
        different.native_players.unwrap(),
        &context(),
        64,
        different.player_inventory,
    )
    .unwrap();
    app.world_mut().insert_resource(replacement);
    assert!(capture(app.world_mut()).is_err());
    app.world_mut().remove_resource::<PlayerRegistry>();
    assert!(capture(app.world_mut()).is_err());
}

#[test]
fn durable_owner_capacity_refuses_reservation_without_orphan_issuance() {
    use icarus_bevy::native_inventory::StackId;
    let mut first = app();
    attach(&mut first, 1, 1);
    let mut state = snapshot(&mut first);
    let template = bag(&state, 1).clone();
    let mut players =
        PlayerRegistry::new(PlayerSnapshot::empty(context()), &context(), 64).unwrap();
    for n in 1..=4096 {
        players.reserve(key(n), n, checkpoint()).unwrap();
        players.activate(n).unwrap();
        players.detach(n);
    }
    state.native_players = Some(players.snapshot());
    let inventory = state.player_inventory.as_mut().unwrap();
    inventory.bags = (1..=4096)
        .map(|id| {
            let mut b = template.clone();
            b.owner.player_id = id;
            b.inventories[0].slots[0].as_mut().unwrap().id = StackId::new(id).unwrap();
            b
        })
        .collect();
    inventory.next_stack_id = 4097;
    state.validate().unwrap();
    let mut app = build_app(&ServerConfig::default(), &state).unwrap();
    let before = snapshot(&mut app);
    assert!(
        app.world_mut()
            .resource_mut::<PlayerRegistry>()
            .reserve(key(4097), 5000, checkpoint())
            .is_err()
    );
    assert_eq!(snapshot(&mut app), before);
}

#[test]
fn all_unknown_section_and_recipe_formats_refuse_previous_generation_recovery() {
    let mut app = app();
    attach(&mut app, 1, 1);
    let state = snapshot(&mut app);
    for recipe in [false, true] {
        for format in [
            json!(0),
            json!(2),
            json!(3),
            json!(-1),
            json!(1.0),
            json!(u32::MAX),
            json!("future-private"),
            json!(true),
            json!(null),
            json!({}),
            json!([]),
        ] {
            if recipe && format == json!(2) {
                continue;
            }
            let dir = tempfile::tempdir().unwrap();
            {
                let mut store = SnapshotStore::open(dir.path()).unwrap();
                store.save(&state).unwrap();
                store.save(&state).unwrap();
            }
            let mut value = serde_json::to_value(&state).unwrap();
            if recipe {
                value["player_inventory"]["recipe"]["format"] = format.clone();
            } else {
                value["player_inventory"]["format"] = format.clone();
            }
            replace_current(dir.path(), &value);
            let before = disk(dir.path());
            let mut store = SnapshotStore::open_existing(dir.path()).unwrap();
            let error = store.load().unwrap_err();
            assert!(
                error.to_string().contains("unsupported"),
                "{recipe} {format}: {error}"
            );
            assert!(store.save(&state).is_err());
            drop(store);
            assert_eq!(disk(dir.path()), before);
        }
    }
}

#[test]
fn sparse_player_id_high_water_exhaustion_preserves_bag_and_record() {
    let mut app = app();
    attach(&mut app, 1, 1);
    let mut value = serde_json::to_value(snapshot(&mut app)).unwrap();
    value["native_players"]["high_water"] = json!(u64::MAX);
    let state: WorldSnapshot = serde_json::from_value(value).unwrap();
    let mut app = build_app(&ServerConfig::default(), &state).unwrap();
    let before = snapshot(&mut app);
    assert!(
        app.world_mut()
            .resource_mut::<PlayerRegistry>()
            .reserve(key(2), 2, checkpoint())
            .is_err()
    );
    assert_eq!(snapshot(&mut app), before);
}

#[test]
fn schema14_generation_bytes_remain_identical_without_inventory() {
    use sha2::{Digest, Sha256};
    let state = WorldSnapshot {
        schema_version: 14,
        native_players: Some(PlayerSnapshot::empty(context())),
        ..Default::default()
    };
    let dir = tempfile::tempdir().unwrap();
    SnapshotStore::open(dir.path())
        .unwrap()
        .save(&state)
        .unwrap();
    let bytes = fs::read(dir.path().join("snapshot-1.json")).unwrap();
    // Independently calculated from the schema-14 DTO field order and authored
    // fixture, including the existing null release provenance representation.
    let digest = Sha256::digest(&bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    assert_eq!(
        digest,
        "f75406e23fbc1ec8b4c278f7e2efe1c2801625eeb4ac67a3a5d14edb083e49b2"
    );
    assert!(
        !String::from_utf8(bytes)
            .unwrap()
            .contains("player_inventory")
    );
}

#[test]
fn every_operation_continues_after_aggregate_save_reload_and_reattach() {
    let mut r = recipe();
    r.starter.push(PlayerStarterItemV1 {
        inventory: "bag".into(),
        requested_slot: 255,
        item: "authored-token".into(),
        count: 2,
    });
    let mut app = build_app(&ServerConfig::default(), &empty_recipe(r)).unwrap();
    let a = attach(&mut app, 1, 1);
    let mut state = snapshot(&mut app);
    for step in 0..5 {
        let loaded = reload(&state);
        assert_eq!(loaded, state);
        let mut next = build_app(&ServerConfig::default(), &loaded).unwrap();
        assert_eq!(attach(&mut next, 1, 100 + step), a);
        let action = match step {
            0 => PlayerInventoryActionV1::Split {
                source: endpoint(&state, a, 0),
                destination: destination(a, 1),
                amount: 3,
            },
            1 => PlayerInventoryActionV1::Move {
                source: endpoint(&state, a, 1),
                destination: destination(a, 2),
            },
            2 => PlayerInventoryActionV1::Merge {
                source: endpoint(&state, a, 2),
                destination: endpoint(&state, a, 0),
                amount: 3,
            },
            3 => PlayerInventoryActionV1::Consume {
                source: endpoint(&state, a, 0),
                amount: 2,
            },
            _ => {
                let b = bag(&state, a);
                PlayerInventoryActionV1::PlaceOverflow {
                    source: PlayerStackEndpointV1 {
                        owner: b.owner,
                        inventory: "bag".into(),
                        stack_id: b.inventories[0].overflow[0].stack.id,
                    },
                    destination: PlayerOverflowDestinationV1 {
                        owner: b.owner,
                        inventory: "bag".into(),
                        slot: Some(1),
                    },
                }
            }
        };
        let req = request(&state, a, action);
        apply(&mut next, 1, 100 + step, &req);
        next.world_mut()
            .resource_mut::<icarus_bevy::simulation::SimulationClock>()
            .tick = 10 * (step + 1);
        state = snapshot(&mut next);
    }
    assert_eq!(state.player_inventory.as_ref().unwrap().next_stack_id, 4);
    assert_eq!(bag(&state, a).consumed[0].count, 2);
    assert_eq!(reload(&state), state);
}

#[test]
fn explicit_null_inventory_is_never_an_absence_or_legacy_downgrade() {
    for schema in [14, 15] {
        let mut value = serde_json::to_value(WorldSnapshot {
            schema_version: schema,
            native_players: Some(PlayerSnapshot::empty(context())),
            ..Default::default()
        })
        .unwrap();
        value["player_inventory"] = json!(null);
        assert!(serde_json::from_value::<WorldSnapshot>(value).is_err());
    }
}

#[test]
fn cli_post_publication_output_failure_requires_reopen_before_retry() {
    let _lock = CLI_LOCK.lock().unwrap();
    let mut app = app();
    let a = attach(&mut app, 1, 1);
    let state = snapshot(&mut app);
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("state");
    SnapshotStore::open(&root).unwrap().save(&state).unwrap();
    let req = dir.path().join("request.json");
    fs::write(&req, serde_json::to_vec(&consume(&state, a, 1)).unwrap()).unwrap();
    // The durable save succeeds, then the real binary cannot deliver its
    // receipt. This is a bounded process-level uncertain operator outcome.
    let full = fs::OpenOptions::new()
        .write(true)
        .open("/dev/full")
        .unwrap();
    let mut errors = tempfile::tempfile().unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_icarus-server"))
        .arg("apply-player-inventory")
        .arg("--data-dir")
        .arg(&root)
        .arg("--request")
        .arg(&req)
        .stdout(Stdio::from(full))
        .stderr(Stdio::from(errors.try_clone().unwrap()))
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() > deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("post-publication command exceeded deadline");
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    assert!(!status.success());
    errors.rewind().unwrap();
    let mut diagnostics = String::new();
    errors.read_to_string(&mut diagnostics).unwrap();
    assert!(!diagnostics.contains("authored-token"));
    let loaded = SnapshotStore::open_existing(&root)
        .unwrap()
        .load()
        .unwrap()
        .unwrap()
        .snapshot;
    assert_eq!(bag(&loaded, a).consumed[0].count, 1);
    let before = disk(&root);
    let retry = execute(&root, &req, false);
    assert!(!retry.status.success());
    assert_eq!(disk(&root), before);
}

mod consumables {
    use super::*;
    use icarus_bevy::native_inventory::StackId;
    fn v2() -> PlayerInventoryRecipeV2 {
        let v = recipe();
        PlayerInventoryRecipeV2 {
            format: 2,
            items: v.items,
            inventories: v.inventories,
            starter: v.starter,
            consumables: vec![PlayerConsumableV2 {
                item: "authored-token".into(),
                oxygen: 15,
                water: 25,
                food: 35,
            }],
        }
    }
    fn empty_v2(r: PlayerInventoryRecipeV2) -> WorldSnapshot {
        let mut state = empty();
        state.player_inventory = Some(
            CompiledPlayerInventoryRecipe::compile_versioned(r.into())
                .unwrap()
                .empty(),
        );
        state
    }
    fn setup(r: PlayerInventoryRecipeV2) -> (bevy::prelude::App, u64) {
        let mut app = build_app(&ServerConfig::default(), &empty_v2(r)).unwrap();
        let id = attach(&mut app, 1, 1);
        (app, id)
    }
    fn use_item(state: &WorldSnapshot, id: u64) -> PlayerInventoryRequestV1 {
        request(
            state,
            id,
            PlayerInventoryActionV1::Use {
                source: endpoint(state, id, 0),
            },
        )
    }
    fn depleted(app: &mut bevy::prelude::App, id: u64) {
        let revision = snapshot(app)
            .native_players
            .unwrap()
            .projection_by_id(id)
            .unwrap()
            .revision;
        let mut next = checkpoint();
        next.vitals.health = 0;
        next.vitals.oxygen = 0;
        next.vitals.water = 90;
        next.vitals.food = 50;
        icarus_bevy::native_player_runtime::edit_player_checkpoint(
            app.world_mut(),
            key(1),
            1,
            revision,
            next,
        )
        .unwrap();
    }
    fn parse(value: &serde_json::Value) -> bool {
        CompiledPlayerInventoryRecipe::parse(&serde_json::to_string(value).unwrap()).is_ok()
    }
    #[test]
    fn v1_golden_bytes_identity_and_strict_decode_parity() {
        const GOLDEN: &str = r#"{"format":1,"items":[{"key":"authored-token","stack_limit":10}],"inventories":[{"key":"bag","initial_slots":3,"slot_rules":[{"allowed_items":["authored-token"]},{"allowed_items":["authored-token"]},{"allowed_items":["authored-token"]},{"allowed_items":["authored-token"]}],"force_add_when_zero":false,"overflow_limit":4}],"starter":[{"inventory":"bag","requested_slot":0,"item":"authored-token","count":8}]}"#;
        const ID: &str = "81656f4ae25783adac34aeca2f7219eac385b4975717a6eba7f10a2f2c53be02";
        let compiled = CompiledPlayerInventoryRecipe::compile(recipe()).unwrap();
        assert_eq!(compiled.identity(), ID);
        assert_eq!(
            serde_json::to_string(&compiled.empty().recipe).unwrap(),
            GOLDEN
        );
        let direct: PlayerInventoryRecipeV1 = serde_json::from_str(GOLDEN).unwrap();
        let versioned: PlayerInventoryRecipe = serde_json::from_str(GOLDEN).unwrap();
        assert_eq!(
            serde_json::to_string(&direct).unwrap(),
            serde_json::to_string(&versioned).unwrap()
        );
        let named = serde_json::to_value(direct).unwrap();
        let mut cases = vec![json!(null), json!([]), json!([1, [], [], []])];
        for field in ["format", "items", "inventories", "starter"] {
            let mut missing = named.clone();
            missing.as_object_mut().unwrap().remove(field);
            cases.push(missing);
            let mut null = named.clone();
            null[field] = json!(null);
            cases.push(null);
        }
        for (pointer, value) in [
            ("/items/0", json!(["authored-token", 10])),
            ("/inventories/0", json!([])),
            ("/inventories/0/slot_rules/0", json!([[]])),
            ("/starter/0", json!([])),
        ] {
            let mut bad = named.clone();
            *bad.pointer_mut(pointer).unwrap() = value;
            cases.push(bad);
        }
        let mut unknown = named.clone();
        unknown["consumables"] = json!(null);
        cases.push(unknown);
        for bad in cases {
            let text = serde_json::to_string(&bad).unwrap();
            assert!(
                serde_json::from_str::<PlayerInventoryRecipeV1>(&text).is_err(),
                "{text}"
            );
            assert!(
                serde_json::from_str::<PlayerInventoryRecipe>(&text).is_err(),
                "{text}"
            );
        }
        for suffix in [r#", "format":1}"#, r#", "items":[]}"#, r#", "unknown":{}}"#] {
            let bad = format!("{}{}", &GOLDEN[..GOLDEN.len() - 1], suffix);
            assert!(serde_json::from_str::<PlayerInventoryRecipeV1>(&bad).is_err());
            assert!(serde_json::from_str::<PlayerInventoryRecipe>(&bad).is_err());
        }
        let mut legacy = app();
        let id = attach(&mut legacy, 1, 1);
        let state = snapshot(&mut legacy);
        assert!(prepare_offline(&state, &use_item(&state, id)).is_err());
        assert_eq!(snapshot(&mut legacy), state);
    }
    #[test]
    fn v2_strict_fields_numbers_references_and_canonical_identity() {
        let named = serde_json::to_value(v2()).unwrap();
        assert!(parse(&named));
        for field in ["format", "items", "inventories", "starter", "consumables"] {
            let mut bad = named.clone();
            bad.as_object_mut().unwrap().remove(field);
            assert!(!parse(&bad), "missing {field}");
            let mut bad = named.clone();
            bad[field] = json!(null);
            assert!(!parse(&bad), "null {field}");
        }
        for field in ["item", "oxygen", "water", "food"] {
            let mut bad = named.clone();
            bad["consumables"][0].as_object_mut().unwrap().remove(field);
            assert!(!parse(&bad));
        }
        for field in ["oxygen", "water", "food"] {
            for number in [
                json!(-1),
                json!(1.0),
                json!("1"),
                json!(null),
                json!(10_000_001u64),
                json!(u64::MAX),
                json!([]),
                json!({}),
                json!(true),
            ] {
                let mut bad = named.clone();
                bad["consumables"][0][field] = number;
                assert!(!parse(&bad));
            }
        }
        for effects in [
            json!([]),
            json!([{"item":"authored-token","oxygen":0,"water":0,"food":0}]),
            json!([{"item":"Authored-token","oxygen":1,"water":0,"food":0}]),
            json!([{"item":"missing","oxygen":1,"water":0,"food":0}]),
            json!([[]]),
            json!([null]),
        ] {
            let mut bad = named.clone();
            bad["consumables"] = effects;
            assert!(!parse(&bad));
        }
        for pointer in [
            "/items/0",
            "/inventories/0",
            "/inventories/0/slot_rules/0",
            "/starter/0",
            "/consumables/0",
        ] {
            let mut bad = named.clone();
            *bad.pointer_mut(pointer).unwrap() = json!([]);
            assert!(!parse(&bad));
            let mut bad = named.clone();
            bad.pointer_mut(pointer).unwrap()["unknown"] = json!(true);
            assert!(!parse(&bad));
        }
        for field in ["items", "consumables"] {
            let mut bad = named.clone();
            let repeated = bad[field][0].clone();
            bad[field].as_array_mut().unwrap().push(repeated);
            assert!(!parse(&bad));
        }
        let mut too_many = named.clone();
        too_many["consumables"] = json!(vec![named["consumables"][0].clone(); 1025]);
        assert!(!parse(&too_many));
        for key in ["".into(), "a".repeat(65), "bad key".into(), "é".into()] {
            let mut bad = named.clone();
            bad["consumables"][0]["item"] = json!(key);
            assert!(!parse(&bad));
        }
        let mut spare = v2();
        spare.consumables.reserve_exact(3000);
        assert!(CompiledPlayerInventoryRecipe::compile_versioned(spare.into()).is_err());
        let mut spare = v2();
        spare.consumables[0].item.reserve_exact(133);
        assert!(CompiledPlayerInventoryRecipe::compile_versioned(spare.into()).is_err());
        let text = serde_json::to_string(&named).unwrap();
        for suffix in [
            r#", "format":2}"#,
            r#", "consumables":[]}"#,
            r#", "extra":0}"#,
        ] {
            assert!(
                CompiledPlayerInventoryRecipe::parse(&format!(
                    "{}{}",
                    &text[..text.len() - 1],
                    suffix
                ))
                .is_err()
            );
        }
        let effect = r#"{"item":"authored-token","oxygen":15,"water":25,"food":35,"water":25}"#;
        let mut bad = named.clone();
        bad["consumables"] = json!([]);
        assert!(
            CompiledPlayerInventoryRecipe::parse(
                &serde_json::to_string(&bad)
                    .unwrap()
                    .replace("\"consumables\":[]", &format!("\"consumables\":[{effect}]"))
            )
            .is_err()
        );
        let late = format!(
            "{{\"consumables\":{},\"starter\":{},\"inventories\":{},\"items\":{},\"format\":2}}",
            named["consumables"], named["starter"], named["inventories"], named["items"]
        );
        assert_eq!(
            CompiledPlayerInventoryRecipe::parse(&late).unwrap(),
            CompiledPlayerInventoryRecipe::parse(&text).unwrap()
        );
        assert!(
            CompiledPlayerInventoryRecipe::parse(&format!("{text}{}", " ".repeat(4 * 1024 * 1024)))
                .is_err()
        );
        let mut multi = v2();
        multi.items.push(ItemDefinitionV1 {
            key: "another".into(),
            stack_limit: 10,
        });
        multi.inventories[0].slot_rules[0]
            .allowed_items
            .push("another".into());
        multi.consumables.push(PlayerConsumableV2 {
            item: "another".into(),
            oxygen: 1,
            water: 0,
            food: 0,
        });
        let mut extra = multi.inventories[0].clone();
        extra.key = "extra".into();
        multi.inventories.push(extra);
        multi.starter.push(PlayerStarterItemV1 {
            inventory: "extra".into(),
            requested_slot: 0,
            item: "another".into(),
            count: 2,
        });
        let original =
            CompiledPlayerInventoryRecipe::compile_versioned(multi.clone().into()).unwrap();
        multi.items.reverse();
        multi.inventories.reverse();
        multi.consumables.reverse();
        for i in &mut multi.inventories {
            for r in &mut i.slot_rules {
                r.allowed_items.reverse();
            }
        }
        assert_eq!(
            CompiledPlayerInventoryRecipe::compile_versioned(multi.clone().into()).unwrap(),
            original
        );
        multi.starter.reverse();
        assert_ne!(
            CompiledPlayerInventoryRecipe::compile_versioned(multi.clone().into())
                .unwrap()
                .identity(),
            original.identity()
        );
        multi.starter.reverse();
        multi.consumables[0].food += 1;
        assert_ne!(
            CompiledPlayerInventoryRecipe::compile_versioned(multi.into())
                .unwrap()
                .identity(),
            original.identity()
        );
        let example = include_str!("../../config/player-inventory-consumables.example.json");
        assert!(CompiledPlayerInventoryRecipe::parse(example).is_ok());
    }
    #[test]
    fn use_request_has_only_named_authoritative_source() {
        let (mut app, id) = setup(v2());
        let state = snapshot(&mut app);
        let req = use_item(&state, id);
        let good = serde_json::to_value(&req).unwrap();
        assert!(serde_json::from_value::<PlayerInventoryRequestV1>(good.clone()).is_ok());
        for field in [
            "amount",
            "oxygen",
            "water",
            "food",
            "item",
            "target",
            "destination",
        ] {
            let mut bad = good.clone();
            bad["action"][field] = json!(1);
            assert!(serde_json::from_value::<PlayerInventoryRequestV1>(bad).is_err());
        }
        for pointer in ["/action", "/action/source", "/action/source/owner"] {
            let mut bad = good.clone();
            *bad.pointer_mut(pointer).unwrap() = json!([]);
            assert!(serde_json::from_value::<PlayerInventoryRequestV1>(bad).is_err());
        }
        let text = serde_json::to_string(&good).unwrap();
        assert!(
            serde_json::from_str::<PlayerInventoryRequestV1>(
                &text.replace("\"type\":\"use\"", "\"type\":\"use\",\"type\":\"use\"")
            )
            .is_err()
        );
        assert!(
            serde_json::from_str::<PlayerInventoryRequestV1>(
                &text.replace("\"source\":", "\"amount\":null,\"source\":")
            )
            .is_err()
        );
    }
    #[test]
    fn use_couples_vitals_quantity_revisions_and_preserves_unrelated_state() {
        for (count, overflow) in [(1, false), (8, false), (1, true), (8, true)] {
            let mut r = v2();
            r.starter[0].count = count;
            if overflow {
                r.starter[0].requested_slot = 3;
            }
            let (mut app, id) = setup(r);
            let other = attach(&mut app, 2, 2);
            depleted(&mut app, id);
            let before = snapshot(&mut app);
            let source = if overflow {
                PlayerStackEndpointV1 {
                    owner: bag(&before, id).owner,
                    inventory: "bag".into(),
                    stack_id: bag(&before, id).inventories[0].overflow[0].stack.id,
                }
            } else {
                endpoint(&before, id, 0)
            };
            let req = request(
                &before,
                id,
                PlayerInventoryActionV1::Use {
                    source: source.clone(),
                },
            );
            assert_eq!(
                apply(&mut app, 1, 1, &req),
                InventoryActionOutcomeV1::Applied
            );
            let after = snapshot(&mut app);
            let mut expected = before
                .native_players
                .as_ref()
                .unwrap()
                .projection_by_id(id)
                .unwrap();
            expected.revision += 1;
            expected.checkpoint.vitals.oxygen = 15;
            expected.checkpoint.vitals.water = 100;
            expected.checkpoint.vitals.food = 85;
            assert_eq!(
                after
                    .native_players
                    .as_ref()
                    .unwrap()
                    .projection_by_id(id)
                    .unwrap(),
                expected
            );
            assert_eq!(bag(&after, id).revision, bag(&before, id).revision + 1);
            assert_eq!(bag(&after, id).consumed[0].count, 1);
            let section = after.player_inventory.as_ref().unwrap();
            let old = before.player_inventory.as_ref().unwrap();
            assert_eq!(section.revision, old.revision + 1);
            assert_eq!(section.next_stack_id, old.next_stack_id);
            assert_eq!(section.recipe, old.recipe);
            assert_eq!(section.baseline, old.baseline);
            assert_eq!(bag(&after, other), bag(&before, other));
            let inv = &bag(&after, id).inventories[0];
            let remaining = inv
                .slots
                .iter()
                .flatten()
                .chain(inv.overflow.iter().map(|s| &s.stack))
                .find(|s| s.id == source.stack_id);
            if count == 1 {
                assert!(remaining.is_none());
            } else {
                assert_eq!(remaining.unwrap().count, count - 1);
            }
            assert!(apply_player_inventory_action(app.world_mut(), key(1), 1, &req).is_err());
            assert_eq!(snapshot(&mut app), after);
            assert_eq!(reload(&after), after);
            let mut restarted = build_app(&ServerConfig::default(), &after).unwrap();
            attach(&mut restarted, 1, 10);
            assert_eq!(snapshot(&mut restarted), after);
        }
        let mut r = v2();
        r.consumables[0].oxygen = 0;
        r.consumables[0].food = 0;
        let (mut app, id) = setup(r);
        depleted(&mut app, id);
        let before = snapshot(&mut app);
        let after = prepare_offline(&before, &use_item(&before, id)).unwrap().0;
        let v = after
            .native_players
            .as_ref()
            .unwrap()
            .projection_by_id(id)
            .unwrap()
            .checkpoint
            .vitals;
        assert_eq!((v.oxygen, v.water, v.food), (0, 100, 50));
        let raw = prepare_offline(&before, &consume(&before, id, 1))
            .unwrap()
            .0;
        assert_eq!(
            raw.native_players
                .as_ref()
                .unwrap()
                .projection_by_id(id)
                .unwrap()
                .checkpoint,
            before
                .native_players
                .as_ref()
                .unwrap()
                .projection_by_id(id)
                .unwrap()
                .checkpoint
        );
    }
    fn max_revisions(mut state: WorldSnapshot, kind: &str) -> WorldSnapshot {
        let mut json = serde_json::to_value(&state).unwrap();
        if kind == "player" || kind == "bag" || kind == "all" {
            json["native_players"]["records"][0]["revision"] = json!(u64::MAX);
        }
        if kind == "bag" || kind == "all" {
            json["player_inventory"]["bags"][0]["revision"] = json!(u64::MAX);
        }
        if kind == "aggregate" || kind == "all" {
            json["player_inventory"]["revision"] = json!(u64::MAX);
        }
        if kind == "allocator" || kind == "all" {
            json["player_inventory"]["next_stack_id"] = json!(u64::MAX);
        }
        state = serde_json::from_value(json).unwrap();
        state.validate().unwrap();
        state
    }
    #[test]
    fn nochange_precedes_exhaustion_but_follows_both_cas_and_source_checks() {
        let (mut initial, id) = setup(v2());
        let full = snapshot(&mut initial);
        let state = max_revisions(full.clone(), "all");
        let req = use_item(&state, id);
        let mut app = build_app(&ServerConfig::default(), &state).unwrap();
        attach(&mut app, 1, 10);
        assert_eq!(
            apply(&mut app, 1, 10, &req),
            InventoryActionOutcomeV1::NoChange
        );
        assert_eq!(snapshot(&mut app), state);
        for kind in [
            "player",
            "bag",
            "owner",
            "incarnation",
            "inventory",
            "stack",
            "id",
        ] {
            let mut bad = req.clone();
            match kind {
                "player" => bad.expected_player_revision -= 1,
                "bag" => bad.expected_bag_revision -= 1,
                "id" => bad.player_id += 1,
                _ => {
                    if let PlayerInventoryActionV1::Use { source } = &mut bad.action {
                        match kind {
                            "owner" => source.owner.player_id += 1,
                            "incarnation" => source.owner.incarnation += 1,
                            "inventory" => source.inventory = "missing".into(),
                            _ => source.stack_id = StackId::new(999).unwrap(),
                        }
                    }
                }
            }
            assert!(
                apply_player_inventory_action(app.world_mut(), key(1), 10, &bad).is_err(),
                "{kind}"
            );
            assert_eq!(snapshot(&mut app), state);
        }
        let mut r = v2();
        r.items.push(ItemDefinitionV1 {
            key: "unused".into(),
            stack_limit: 10,
        });
        r.consumables[0].item = "unused".into();
        let (mut non, id) = setup(r);
        let state = snapshot(&mut non);
        assert!(prepare_offline(&state, &use_item(&state, id)).is_err());
        depleted(&mut initial, id);
        let low = snapshot(&mut initial);
        for kind in ["player", "bag", "aggregate"] {
            let state = max_revisions(low.clone(), kind);
            let req = use_item(&state, id);
            let mut app = build_app(&ServerConfig::default(), &state).unwrap();
            attach(&mut app, 1, 10);
            assert!(
                apply_player_inventory_action(app.world_mut(), key(1), 10, &req).is_err(),
                "{kind}"
            );
            assert_eq!(snapshot(&mut app), state);
        }
        let state = max_revisions(low, "allocator");
        assert_eq!(
            prepare_offline(&state, &use_item(&state, id)).unwrap().1,
            InventoryActionOutcomeV1::Applied
        );
    }
    #[test]
    fn malformed_consumed_overflow_and_allocation_refuse_without_mutation() {
        let (mut app, id) = setup(v2());
        depleted(&mut app, id);
        let state = snapshot(&mut app);
        for kind in ["consumed", "capacity", "schema"] {
            let mut bad = state.clone();
            let req = use_item(&state, id);
            match kind {
                "consumed" => {
                    bad.player_inventory.as_mut().unwrap().bags[0].consumed[0].count = u64::MAX
                }
                "capacity" => bad
                    .player_inventory
                    .as_mut()
                    .unwrap()
                    .bags
                    .reserve_exact(9000),
                _ => {
                    bad.schema_version = 14;
                    bad.player_inventory = None;
                }
            }
            assert!(prepare_offline(&bad, &req).is_err(), "{kind}");
            assert_eq!(snapshot(&mut app), state);
        }
        // The maximum authored gain is admitted and clamps without i32 addition.
        let mut r = v2();
        r.consumables[0].oxygen = 10_000_000;
        let (mut app, id) = setup(r);
        depleted(&mut app, id);
        let state = snapshot(&mut app);
        let after = prepare_offline(&state, &use_item(&state, id)).unwrap().0;
        assert_eq!(
            after
                .native_players
                .unwrap()
                .projection_by_id(id)
                .unwrap()
                .checkpoint
                .vitals
                .oxygen,
            100
        );
    }
    #[test]
    fn live_authority_leave_pins_and_detached_cutoff_cover_use() {
        let (mut app, id) = setup(v2());
        let other = attach(&mut app, 2, 2);
        depleted(&mut app, id);
        let before = snapshot(&mut app);
        let req = use_item(&before, id);
        for (key, epoch) in [(key(2), 1), (key(1), 2), (key(1), 0), (key(1), 999)] {
            assert!(apply_player_inventory_action(app.world_mut(), key, epoch, &req).is_err());
            assert_eq!(snapshot(&mut app), before);
        }
        app.world_mut().resource_mut::<PlayerRegistry>().detach(1);
        app.world_mut()
            .resource_mut::<PlayerRegistry>()
            .reserve(key(1), 3, checkpoint())
            .unwrap();
        for epoch in [1, 3] {
            assert!(apply_player_inventory_action(app.world_mut(), key(1), epoch, &req).is_err());
        }
        assert_eq!(snapshot(&mut app), before);
        activate_player(app.world_mut(), 3).unwrap();
        apply(&mut app, 1, 3, &req);
        let cutoff = snapshot(&mut app);
        let revision = cutoff
            .native_players
            .as_ref()
            .unwrap()
            .projection_by_id(id)
            .unwrap()
            .revision;
        app.world_mut()
            .resource_mut::<PlayerRegistry>()
            .request_leave(key(1), 3, revision, Instant::now(), Duration::from_secs(30))
            .unwrap();
        let blocked = use_item(&cutoff, id);
        assert!(apply_player_inventory_action(app.world_mut(), key(1), 3, &blocked).is_err());
        // Another player's independent record and bag may advance under the pin.
        icarus_bevy::native_player_runtime::consume_player_vitals(
            app.world_mut(),
            icarus_bevy::native_player_runtime::ConsumptionRates::new(1, 1, 1).unwrap(),
        )
        .unwrap();
        let state = snapshot(&mut app);
        apply(&mut app, 2, 2, &use_item(&state, other));
        let pinned = snapshot(&mut app);
        assert_eq!(bag(&pinned, id), bag(&cutoff, id));
        assert_eq!(
            pinned.native_players.as_ref().unwrap().projection_by_id(id),
            cutoff.native_players.as_ref().unwrap().projection_by_id(id)
        );
        let dir = tempfile::tempdir().unwrap();
        let mut writer = CheckpointWriter::spawn(SnapshotStore::open(dir.path()).unwrap()).unwrap();
        writer.submit(app.world_mut()).unwrap();
        assert!(
            writer
                .settle_before(app.world_mut(), Instant::now())
                .is_err()
        );
        assert!(matches!(
            app.world()
                .resource::<PlayerRegistry>()
                .leave_status(key(1), 3)
                .unwrap()
                .phase,
            LeavePhase::Failed(_)
        ));
        assert!(apply_player_inventory_action(app.world_mut(), key(1), 3, &blocked).is_err());
        app.world_mut()
            .resource_mut::<PlayerRegistry>()
            .retry_leave(key(1), 3, Instant::now(), Duration::from_secs(30))
            .unwrap();
        writer
            .finish_before(app.world_mut(), Instant::now() + Duration::from_secs(30))
            .unwrap();
        assert_eq!(
            app.world()
                .resource::<PlayerRegistry>()
                .leave_status(key(1), 3)
                .unwrap()
                .phase,
            LeavePhase::Pending
        );
        writer.submit(app.world_mut()).unwrap();
        app.world_mut().resource_mut::<PlayerRegistry>().detach(3);
        assert!(apply_player_inventory_action(app.world_mut(), key(1), 3, &blocked).is_err());
        writer
            .finish_before(app.world_mut(), Instant::now() + Duration::from_secs(30))
            .unwrap();
        assert!(matches!(
            app.world()
                .resource::<PlayerRegistry>()
                .leave_status(key(1), 3)
                .unwrap()
                .phase,
            LeavePhase::Durable { .. }
        ));
        assert!(apply_player_inventory_action(app.world_mut(), key(1), 3, &blocked).is_err());
        assert_eq!(snapshot(&mut app), pinned);
        assert_eq!(reload(&pinned), pinned);
        // Full capture still rejects substituted/missing registry authority.
        let different = empty_v2(v2());
        let replacement = PlayerRegistry::new_with_inventory(
            different.native_players.unwrap(),
            &context(),
            64,
            different.player_inventory,
        )
        .unwrap();
        app.world_mut().insert_resource(replacement);
        assert!(apply_player_inventory_action(app.world_mut(), key(1), 3, &blocked).is_err());
        app.world_mut().remove_resource::<PlayerRegistry>();
        assert!(apply_player_inventory_action(app.world_mut(), key(1), 3, &blocked).is_err());
    }
    #[test]
    fn durable_use_cli_lock_dryrun_applied_nochange_stale_and_failure() {
        let _lock = CLI_LOCK.lock().unwrap();
        let (mut app, id) = setup(v2());
        depleted(&mut app, id);
        let state = snapshot(&mut app);
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("state");
        let path = dir.path().join("use.json");
        SnapshotStore::open(&root).unwrap().save(&state).unwrap();
        let before = disk(&root);
        fs::write(&path, serde_json::to_vec(&use_item(&state, id)).unwrap()).unwrap();
        assert!(execute(&root, &path, true).status.success());
        assert_eq!(disk(&root), before);
        {
            let _store = SnapshotStore::open_existing(&root).unwrap();
            assert!(!execute(&root, &path, false).status.success());
        }
        assert_eq!(disk(&root), before);
        let output = execute(&root, &path, false);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let receipt: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(receipt["outcome"], "applied");
        assert_eq!(receipt["receipt"]["generation"], 2);
        let mut loaded = SnapshotStore::open_existing(&root)
            .unwrap()
            .load()
            .unwrap()
            .unwrap()
            .snapshot;
        assert_eq!(
            loaded,
            prepare_offline(&state, &use_item(&state, id)).unwrap().0
        );
        let bytes = disk(&root);
        assert!(!execute(&root, &path, false).status.success());
        assert_eq!(disk(&root), bytes);
        // Repeated use eventually reaches all full; that retry writes no receipt.
        loop {
            let req = use_item(&loaded, id);
            let (next, outcome) = prepare_offline(&loaded, &req).unwrap();
            fs::write(&path, serde_json::to_vec(&req).unwrap()).unwrap();
            let before = disk(&root);
            let output = execute(&root, &path, false);
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            loaded = next;
            if outcome == InventoryActionOutcomeV1::NoChange {
                assert_eq!(disk(&root), before);
                assert!(!String::from_utf8_lossy(&output.stdout).contains("receipt"));
                break;
            }
        }
        // A failed publication cannot emit a success receipt for a coupled edit.
        let root = dir.path().join("failed");
        SnapshotStore::open(&root).unwrap().save(&state).unwrap();
        fs::write(&path, serde_json::to_vec(&use_item(&state, id)).unwrap()).unwrap();
        fs::create_dir(root.join("snapshot-2.json")).unwrap();
        let out = execute(&root, &path, false);
        assert!(!out.status.success());
        assert!(!String::from_utf8_lossy(&out.stdout).contains("receipt"));
        assert_eq!(
            SnapshotStore::open_existing(&root)
                .unwrap()
                .load()
                .unwrap()
                .unwrap()
                .snapshot,
            state
        );
    }
    #[test]
    fn recipe_reopen_matrix_preserves_provenance_and_never_promotes() {
        use icarus_bevy::{
            experimental::release::{DEDICATED_SERVER_APP_ID, ReleasePin, network_checksum},
            network::{PreparedPlayers, open_player_snapshot},
        };
        let recipes = [
            CompiledPlayerInventoryRecipe::compile(recipe()).unwrap(),
            CompiledPlayerInventoryRecipe::compile_versioned(v2().into()).unwrap(),
        ];
        for (from, selected) in recipes.iter().enumerate() {
            let mut state = empty();
            state.player_inventory = Some(selected.empty());
            let stored = reload(&state);
            for (to, target) in recipes.iter().enumerate() {
                let version = "1.0.0.9";
                let mut prepared = PreparedPlayers {
                    context: context(),
                    initial_vitals: checkpoint().vitals,
                    release: ReleasePin {
                        app_id: DEDICATED_SERVER_APP_ID,
                        build_id: 999,
                        project_version: version.into(),
                        network_checksum: network_checksum(version),
                    },
                    inventory: Some(target.clone()),
                };
                let mut candidate = stored.clone();
                let result = open_player_snapshot(&mut candidate, Some(&prepared));
                assert_eq!(result.is_ok(), from == to);
                if result.is_err() {
                    assert_eq!(candidate, stored);
                }
                let mut changed = v2();
                changed.consumables[0].water += 1;
                prepared.inventory =
                    Some(CompiledPlayerInventoryRecipe::compile_versioned(changed.into()).unwrap());
                let mut candidate = stored.clone();
                assert!(open_player_snapshot(&mut candidate, Some(&prepared)).is_err());
                assert_eq!(candidate, stored);
            }
        }
    }
    #[test]
    fn known_v2_corruption_recovers_but_future_duplicate_versions_fail_closed() {
        let (mut app, id) = setup(v2());
        depleted(&mut app, id);
        let before = snapshot(&mut app);
        let after = prepare_offline(&before, &use_item(&before, id)).unwrap().0;
        // Known recipe 2 without required effects is corruption, so previous
        // committed state remains eligible. Version 3 blocks that recovery.
        let dir = tempfile::tempdir().unwrap();
        {
            let mut store = SnapshotStore::open(dir.path()).unwrap();
            store.save(&before).unwrap();
            store.save(&after).unwrap();
        }
        let mut broken = serde_json::to_value(&after).unwrap();
        broken["player_inventory"]["recipe"]
            .as_object_mut()
            .unwrap()
            .remove("consumables");
        replace_current(dir.path(), &broken);
        assert_eq!(
            SnapshotStore::open_existing(dir.path())
                .unwrap()
                .load()
                .unwrap()
                .unwrap()
                .snapshot,
            before
        );
        for future_first in [false, true] {
            for parent_duplicate in [false, true] {
                let dir = tempfile::tempdir().unwrap();
                let mut store = SnapshotStore::open(dir.path()).unwrap();
                store.save(&before).unwrap();
                store.save(&after).unwrap();
                drop(store);
                let v = serde_json::to_value(&after).unwrap();
                let recipe = &v["player_inventory"]["recipe"];
                let body = serde_json::to_string(recipe).unwrap();
                let altered = if parent_duplicate {
                    let unknown = if future_first {
                        format!("\"recipe\":{{\"format\":3}},\"recipe\":{body}")
                    } else {
                        format!("\"recipe\":{body},\"recipe\":{{\"format\":3}}")
                    };
                    serde_json::to_string(&v)
                        .unwrap()
                        .replace(&format!("\"recipe\":{body}"), &unknown)
                } else {
                    let pair = if future_first {
                        "\"format\":3,\"format\":2"
                    } else {
                        "\"format\":2,\"format\":3"
                    };
                    serde_json::to_string(&v)
                        .unwrap()
                        .replace(&body, &body.replace("\"format\":2", pair))
                };
                replace_current_bytes(dir.path(), altered.as_bytes());
                let bytes = disk(dir.path());
                let mut store = SnapshotStore::open_existing(dir.path()).unwrap();
                let error = store.load().unwrap_err();
                assert!(error.to_string().contains("unsupported"), "{error}");
                assert!(store.save(&after).is_err());
                drop(store);
                assert_eq!(disk(dir.path()), bytes);
            }
        }
    }
    fn replace_current_bytes(root: &Path, bytes: &[u8]) {
        use sha2::{Digest, Sha256};
        let path = root.join("manifest.json");
        let mut m: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        let n = m["current"]["number"].as_u64().unwrap();
        m["current"]["sha256"] = json!(
            Sha256::digest(bytes)
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>()
        );
        fs::write(root.join(format!("snapshot-{n}.json")), bytes).unwrap();
        fs::write(path, serde_json::to_vec(&m).unwrap()).unwrap();
    }
    #[test]
    fn near_limit_semantic_v1_roundtrip_preserves_admission() {
        let mut r = recipe();
        let long = "a".repeat(64);
        r.items[0].key = long.clone();
        r.items[0].stack_limit = 1;
        r.inventories[0].initial_slots = 256;
        r.inventories[0].slot_rules = vec![
            SlotRuleV1 {
                allowed_items: vec![long.clone()]
            };
            256
        ];
        r.starter = (0..256)
            .map(|slot| PlayerStarterItemV1 {
                inventory: "bag".into(),
                requested_slot: slot,
                item: long.clone(),
                count: 1,
            })
            .collect();
        let mut initial = build_app(&ServerConfig::default(), &empty_recipe(r)).unwrap();
        attach(&mut initial, 1, 1);
        let mut state = snapshot(&mut initial);
        let template = bag(&state, 1).clone();
        let mut registry =
            PlayerRegistry::new(PlayerSnapshot::empty(context()), &context(), 64).unwrap();
        for id in 1..=256 {
            registry.reserve(key(id), id, checkpoint()).unwrap();
            registry.activate(id).unwrap();
            registry.detach(id);
        }
        state.native_players = Some(registry.snapshot());
        let section = state.player_inventory.as_mut().unwrap();
        section.bags.clear();
        for id in 1..=256 {
            let mut bag = template.clone();
            bag.owner.player_id = id;
            for (slot, stack) in bag.inventories[0].slots.iter_mut().enumerate() {
                let stack = stack.as_mut().unwrap();
                stack.id = StackId::new((id - 1) * 256 + slot as u64 + 1).unwrap();
                let mut capacity = String::with_capacity(96);
                capacity.push_str(&long);
                stack.item = capacity;
            }
            section.bags.push(bag);
        }
        section.next_stack_id = 65537;
        // The first passing state is within one 256-stack bag of the eight-copy
        // limit. Higher IDs may remain retired without being reused.
        while state.validate().is_err() {
            state.player_inventory.as_mut().unwrap().bags.pop().unwrap();
            let mut players = serde_json::to_value(state.native_players.take().unwrap()).unwrap();
            players["records"].as_array_mut().unwrap().pop().unwrap();
            state.native_players = Some(serde_json::from_value(players).unwrap());
        }
        assert!(state.player_inventory.as_ref().unwrap().bags.len() > 200);
        assert_eq!(reload(&state), state);
        let roundtrip: WorldSnapshot =
            serde_json::from_slice(&serde_json::to_vec(&state).unwrap()).unwrap();
        roundtrip.validate().unwrap();
        let req = request(
            &state,
            1,
            PlayerInventoryActionV1::Consume {
                source: endpoint(&state, 1, 0),
                amount: 1,
            },
        );
        let (after, outcome) = prepare_offline(&state, &req).unwrap();
        assert_eq!(outcome, InventoryActionOutcomeV1::Applied);
        assert_eq!(reload(&after), after);
    }
    #[test]
    fn full_generation_use_preflight_failure_never_exposes_either_domain() {
        let (mut app, id) = setup(v2());
        depleted(&mut app, id);
        let mut state = snapshot(&mut app);
        let req = use_item(&state, id);
        let mut length = serde_json::to_vec(&state).unwrap().len();
        let mut entity_id = 1;
        loop {
            let entity = EntityRecord {
                id: DomainId(entity_id),
                position: Position([0.; 3]),
            };
            let delta = serde_json::to_vec(&entity).unwrap().len()
                + usize::from(!state.entities.is_empty());
            if length + delta > 64 * 1024 * 1024 {
                break;
            }
            length += delta;
            state.entities.push(entity);
            entity_id += 1;
        }
        let mut players = serde_json::to_value(state.native_players.as_ref().unwrap()).unwrap();
        players["records"][0]["revision"] = json!(9);
        state.native_players = Some(serde_json::from_value(players).unwrap());
        let section = state.player_inventory.as_mut().unwrap();
        section.bags[0].revision = 9;
        section.revision = 9;
        let mut remaining = 64 * 1024 * 1024 - serde_json::to_vec(&state).unwrap().len();
        for entity in state.entities.iter_mut().take(20) {
            for coordinate in &mut entity.position.0 {
                let mut best = (0, 0.0);
                for digits in 1..=17 {
                    let value = format!("0.{}", "1".repeat(digits)).parse::<f32>().unwrap();
                    let gain = serde_json::to_string(&value).unwrap().len() - 3;
                    if gain <= remaining && gain > best.0 {
                        best = (gain, value);
                    }
                }
                *coordinate = best.1;
                remaining -= best.0;
            }
        }
        assert_eq!(remaining, 0);
        SnapshotStore::preflight(&state).unwrap();
        let req = request(&state, id, req.action);
        let mut small = state.clone();
        small.entities.clear();
        assert_eq!(
            prepare_offline(&small, &req).unwrap().1,
            InventoryActionOutcomeV1::Applied
        );
        drop(small);
        assert!(prepare_offline(&state, &req).is_err());
        let mut app = build_app(&ServerConfig::default(), &state).unwrap();
        attach(&mut app, 1, 10);
        let before = snapshot(&mut app);
        assert!(apply_player_inventory_action(app.world_mut(), key(1), 10, &req).is_err());
        assert_eq!(snapshot(&mut app), before);
    }
}

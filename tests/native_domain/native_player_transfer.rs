//! Authored source fixtures only; no stock RPC or compatibility assertion.
use super::native_player_inventory::{attach, checkpoint, context, key, recipe};
use icarus_bevy::{
    config::ServerConfig,
    native_inventory::{InventoryActionOutcomeV1, StackId},
    native_player_inventory::*,
    native_player_runtime::{
        CheckpointWriter, LeavePhase, PlayerRegistry, activate_player,
        apply_player_inventory_action, apply_player_inventory_transfer,
    },
    native_players::PlayerSnapshot,
    persistence::SnapshotStore,
    simulation::{SNAPSHOT_SCHEMA_PLAYER_TRANSFERS, WorldSnapshot, build_app, capture},
};
use serde_json::json;
use std::{
    fs,
    path::Path,
    time::{Duration, Instant},
};
fn app_with(overflow: bool, use_recipe: bool) -> bevy::prelude::App {
    let mut recipe = recipe();
    recipe
        .items
        .push(icarus_bevy::native_inventory::ItemDefinitionV1 {
            key: "zero".into(),
            stack_limit: 10,
        });
    recipe
        .inventories
        .push(icarus_bevy::native_inventory::InventoryDefinitionV1 {
            key: "spare".into(),
            initial_slots: 1,
            slot_rules: vec![icarus_bevy::native_inventory::SlotRuleV1 {
                allowed_items: vec!["authored-token".into()],
            }],
            force_add_when_zero: false,
            overflow_limit: 0,
        });
    if overflow {
        recipe.starter.push(PlayerStarterItemV1 {
            inventory: "bag".into(),
            requested_slot: 0,
            item: "authored-token".into(),
            count: 4,
        });
    }
    let compiled = if use_recipe {
        CompiledPlayerInventoryRecipe::compile_versioned(
            PlayerInventoryRecipeV2 {
                format: 2,
                items: recipe.items,
                inventories: recipe.inventories,
                starter: recipe.starter,
                consumables: vec![PlayerConsumableV2 {
                    item: "authored-token".into(),
                    oxygen: 1,
                    water: 2,
                    food: 3,
                }],
            }
            .into(),
        )
        .unwrap()
    } else {
        CompiledPlayerInventoryRecipe::compile(recipe).unwrap()
    };
    let state = WorldSnapshot {
        schema_version: SNAPSHOT_SCHEMA_PLAYER_TRANSFERS,
        native_players: Some(PlayerSnapshot::empty(context())),
        player_inventory: Some(compiled.empty_v2().into()),
        ..Default::default()
    };
    let mut app = build_app(&ServerConfig::default(), &state).unwrap();
    for n in 1..=3 {
        attach(&mut app, n, n);
    }
    app
}
fn state(app: &mut bevy::prelude::App) -> WorldSnapshot {
    capture(app.world_mut()).unwrap()
}
fn section(s: &WorldSnapshot) -> &PlayerInventorySnapshotV2 {
    s.player_inventory.as_ref().unwrap().as_v2().unwrap()
}
fn section_mut(s: &mut WorldSnapshot) -> &mut PlayerInventorySnapshotV2 {
    s.player_inventory.as_mut().unwrap().as_v2_mut().unwrap()
}
fn transfer(
    s: &WorldSnapshot,
    from: u64,
    to: u64,
    amount: u32,
    overflow: bool,
) -> PlayerInventoryTransferRequestV1 {
    let i = section(s);
    let a = i.bag(from).unwrap();
    let b = i.bag(to).unwrap();
    let source = if overflow {
        &a.inventories[0].overflow[0].stack
    } else {
        a.inventories[0].slots[0].as_ref().unwrap()
    };
    let cas = |id, bag_revision| PlayerInventoryRevisionPairV1 {
        expected_player_revision: s
            .native_players
            .as_ref()
            .unwrap()
            .projection_by_id(id)
            .unwrap()
            .revision,
        expected_bag_revision: bag_revision,
    };
    PlayerInventoryTransferRequestV1 {
        source: PlayerStackEndpointV1 {
            owner: a.owner,
            inventory: "bag".into(),
            stack_id: source.id,
        },
        destination: PlayerSlotEndpointV1 {
            owner: b.owner,
            inventory: "bag".into(),
            slot: 1,
        },
        amount,
        source_cas: cas(from, a.revision),
        destination_cas: cas(to, b.revision),
    }
}
fn apply(
    app: &mut bevy::prelude::App,
    r: &PlayerInventoryTransferRequestV1,
) -> AppliedPlayerInventoryTransfer {
    apply_player_inventory_transfer(app.world_mut(), key(1), 1, key(2), 2, r).unwrap()
}
fn local(s: &WorldSnapshot, id: u64, action: PlayerInventoryActionV1) -> PlayerInventoryRequestV1 {
    PlayerInventoryRequestV1 {
        player_id: id,
        expected_player_revision: s
            .native_players
            .as_ref()
            .unwrap()
            .projection_by_id(id)
            .unwrap()
            .revision,
        expected_bag_revision: section(s).bag(id).unwrap().revision,
        action,
    }
}
fn reopen(path: &Path) -> SnapshotStore {
    let deadline = Instant::now() + Duration::from_secs(5);
    for _ in 0..256 {
        match SnapshotStore::open_existing(path) {
            Ok(store) => return store,
            Err(error) => {
                let contention = error.chain().any(|e| {
                    e.downcast_ref::<std::io::Error>()
                        .is_some_and(|e| e.kind() == std::io::ErrorKind::WouldBlock)
                });
                assert!(
                    contention && Instant::now() < deadline,
                    "reopen failed: {error:#}"
                );
                std::thread::sleep(Duration::from_millis(20));
            }
        }
    }
    panic!("bounded reopen attempts exhausted")
}
#[test]
fn whole_partial_normal_overflow_atomic_and_actual_writer_reopen() {
    for overflow in [false, true] {
        for partial in [false, true] {
            for recipe2 in [false, true] {
                let mut app = app_with(overflow, recipe2);
                let before = state(&mut app);
                let r = transfer(
                    &before,
                    1,
                    2,
                    if partial {
                        2
                    } else if overflow {
                        4
                    } else {
                        8
                    },
                    overflow,
                );
                let outcome = apply(&mut app, &r);
                let after = state(&mut app);
                let a = section(&after).bag(1).unwrap();
                let b = section(&after).bag(2).unwrap();
                let stack = b.inventories[0].slots[1].as_ref().unwrap();
                assert_eq!(
                    stack.id,
                    if partial {
                        StackId::new(section(&before).next_stack_id).unwrap()
                    } else {
                        r.source.stack_id
                    }
                );
                assert_eq!(
                    section(&after).next_stack_id,
                    section(&before).next_stack_id + u64::from(partial)
                );
                assert_eq!(outcome.inventory_revision, section(&before).revision + 1);
                assert_eq!(
                    outcome.source_player_revision,
                    r.source_cas.expected_player_revision + 1
                );
                assert_eq!(
                    outcome.destination_player_revision,
                    r.destination_cas.expected_player_revision + 1
                );
                assert_eq!(a.revision, r.source_cas.expected_bag_revision + 1);
                assert_eq!(b.revision, r.destination_cas.expected_bag_revision + 1);
                assert_eq!(a.sent[0].count, u64::from(r.amount));
                assert_eq!(b.received[0].count, u64::from(r.amount));
                assert!(a.consumed.is_empty() && b.consumed.is_empty());
                assert_eq!(section(&after).bag(3), section(&before).bag(3));
                assert!(
                    apply_player_inventory_transfer(app.world_mut(), key(1), 1, key(2), 2, &r)
                        .is_err()
                );
                assert_eq!(state(&mut app), after);
                let dir = tempfile::tempdir().unwrap();
                let store = SnapshotStore::open(dir.path()).unwrap();
                let mut writer = CheckpointWriter::spawn(store).unwrap();
                writer.submit(app.world_mut()).unwrap();
                let receipt = writer
                    .finish_before(app.world_mut(), Instant::now() + Duration::from_secs(5))
                    .unwrap()
                    .unwrap();
                assert_eq!(receipt.tick, after.tick);
                drop(writer);
                let store = reopen(dir.path());
                assert_eq!(store.load().unwrap().unwrap().snapshot, after);
            }
        }
    }
}
#[test]
fn all_four_cas_and_invalid_endpoints_refuse_exactly() {
    let mut app = app_with(false, false);
    let before = state(&mut app);
    let valid = transfer(&before, 1, 2, 2, false);
    for field in 0..14 {
        let mut r = valid.clone();
        match field {
            0 => r.source_cas.expected_player_revision += 1,
            1 => r.source_cas.expected_bag_revision += 1,
            2 => r.destination_cas.expected_player_revision += 1,
            3 => r.destination_cas.expected_bag_revision += 1,
            4 => r.destination.owner = r.source.owner,
            5 => r.source.owner.incarnation = 2,
            6 => r.destination.owner.incarnation = 2,
            7 => r.amount = 0,
            8 => r.amount = 9,
            9 => r.source.stack_id = StackId::new(999).unwrap(),
            10 => r.destination.slot = 0,
            11 => r.destination.slot = 99,
            12 => r.destination.inventory = "missing".into(),
            _ => r.source.owner.player_id = 999,
        }
        assert!(
            prepare_transfer_offline(&before, &r).is_err(),
            "case {field}"
        );
        assert!(
            apply_player_inventory_transfer(app.world_mut(), key(1), 1, key(2), 2, &r).is_err(),
            "case {field}"
        );
        assert_eq!(state(&mut app), before);
    }
    for (sk, se, dk, de) in [
        (key(3), 1, key(2), 2),
        (key(1), 9, key(2), 2),
        (key(1), 1, key(3), 2),
        (key(1), 1, key(2), 9),
        (key(1), 1, key(1), 1),
    ] {
        assert!(apply_player_inventory_transfer(app.world_mut(), sk, se, dk, de, &valid).is_err());
        assert_eq!(state(&mut app), before);
    }
}
#[test]
fn accounting_roundtrip_tampering_and_wide_balances() {
    let mut app = app_with(false, true);
    let before = state(&mut app);
    apply(&mut app, &transfer(&before, 1, 2, 2, false));
    let after = state(&mut app);
    let bytes = serde_json::to_vec(&after).unwrap();
    let decoded: WorldSnapshot = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(decoded, after);
    decoded.validate().unwrap();
    for field in 0..6 {
        let mut bad = after.clone();
        let s = section_mut(&mut bad);
        match field {
            0 => {
                let stack = s.bags[1].inventories[0].slots[1].take();
                s.bags[0].inventories[0].slots[1] = stack;
            }
            1 => s.bags[0].sent[0].count += 1,
            2 => s.bags[1].received[0].count += 1,
            3 => {
                s.bags[1].received[0].count += 1;
                s.bags[1]
                    .consumed
                    .push(icarus_bevy::native_inventory::ItemTotalV1 {
                        item: "authored-token".into(),
                        count: 1,
                    });
            }
            4 => s.bags[0].sent[0].count = 0,
            _ => {
                let row = s.bags[0].sent[0].clone();
                s.bags[0].sent.push(row);
            }
        }
        assert!(bad.validate().is_err(), "case {field}");
    }
    // Balanced cumulative cycles exceed u64 when added to starter; comparisons
    // remain wide and persisted increments still refuse at exhaustion.
    let mut huge = before.clone();
    for bag in &mut section_mut(&mut huge).bags {
        let row = icarus_bevy::native_inventory::ItemTotalV1 {
            item: "authored-token".into(),
            count: u64::MAX,
        };
        bag.received.push(row.clone());
        bag.sent.push(row);
    }
    huge.validate().unwrap();
    let r = transfer(&huge, 1, 2, 1, false);
    assert!(prepare_transfer_offline(&huge, &r).is_err());
}
#[test]
fn revision_allocator_and_flow_exhaustion_are_atomic() {
    let mut app = app_with(false, false);
    let base = state(&mut app);
    for field in 0..7 {
        let mut s = base.clone();
        match field {
            0 | 1 => {
                let id = if field == 0 { 1 } else { 2 };
                set_player_revision(&mut s, id, u64::MAX);
            }
            2 | 3 => {
                let i = field - 2;
                set_player_revision(&mut s, i as u64 + 1, u64::MAX);
                section_mut(&mut s).bags[i].revision = u64::MAX;
            }
            4 => section_mut(&mut s).revision = u64::MAX,
            5 => {
                for b in &mut section_mut(&mut s).bags {
                    let row = icarus_bevy::native_inventory::ItemTotalV1 {
                        item: "authored-token".into(),
                        count: u64::MAX,
                    };
                    b.received.push(row.clone());
                    b.sent.push(row);
                }
            }
            _ => section_mut(&mut s).next_stack_id = u64::MAX,
        }
        s.validate().unwrap();
        let r = transfer(&s, 1, 2, 2, false);
        assert!(prepare_transfer_offline(&s, &r).is_err(), "case {field}");
        if field == 6 {
            let whole = transfer(&s, 1, 2, 8, false);
            assert_eq!(
                section(&prepare_transfer_offline(&s, &whole).unwrap().0).next_stack_id,
                u64::MAX
            );
        }
    }
}
#[test]
fn section_decoder_order_strict_shapes_and_required_sparse_fields() {
    let mut app = app_with(false, true);
    let before = state(&mut app);
    apply(&mut app, &transfer(&before, 1, 2, 2, false));
    let after = state(&mut app);
    let value = serde_json::to_value(section(&after)).unwrap();
    let mut fields = value
        .as_object()
        .unwrap()
        .iter()
        .map(|(k, v)| format!("{}:{}", serde_json::to_string(k).unwrap(), v))
        .collect::<Vec<_>>();
    fields.sort_by_key(|s| !s.starts_with("\"bags\""));
    let ordered = format!("{{{}}}", fields.join(","));
    let parsed: PlayerInventorySnapshot = serde_json::from_str(&ordered).unwrap();
    assert_eq!(parsed, after.player_inventory.clone().unwrap());
    for key in ["received", "sent", "consumed"] {
        for null in [false, true] {
            let mut bad = value.clone();
            if null {
                bad["bags"][0][key] = serde_json::Value::Null;
            } else {
                bad["bags"][0].as_object_mut().unwrap().remove(key);
            }
            assert!(serde_json::from_value::<PlayerInventorySnapshot>(bad).is_err());
        }
    }
    for bad in [
        json!([]),
        json!([2]),
        {
            let mut v = value.clone();
            v["extra"] = json!(1);
            v
        },
        {
            let mut v = value.clone();
            v["bags"][0] = json!([]);
            v
        },
        {
            let mut v = value.clone();
            v["bags"][0]["extra"] = json!(1);
            v
        },
        {
            let mut v = value.clone();
            v["bags"][0].as_object_mut().unwrap().remove("sent");
            v["bags"][0].as_object_mut().unwrap().remove("received");
            v
        },
    ] {
        assert!(serde_json::from_value::<PlayerInventorySnapshot>(bad).is_err());
    }
    let duplicate = ordered.replacen("\"format\":2", "\"format\":2,\"format\":2", 1);
    assert!(serde_json::from_str::<PlayerInventorySnapshot>(&duplicate).is_err());
    let request = serde_json::to_value(transfer(&after, 1, 2, 1, false)).unwrap();
    for path in [
        "root",
        "source",
        "destination",
        "source_cas",
        "destination_cas",
    ] {
        let mut v = request.clone();
        let target = if path == "root" { &mut v } else { &mut v[path] };
        target
            .as_object_mut()
            .unwrap()
            .insert("extra".into(), json!(1));
        assert!(serde_json::from_value::<PlayerInventoryTransferRequestV1>(v).is_err());
    }
    assert!(serde_json::from_value::<PlayerInventoryTransferRequestV1>(json!([])).is_err());
}

#[test]
fn received_bag_local_actions_use_consume_and_cumulative_return() {
    let mut app = app_with(false, true);
    let before = state(&mut app);
    apply(&mut app, &transfer(&before, 1, 2, 4, false));
    let source = |s: &WorldSnapshot, slot: usize| PlayerStackEndpointV1 {
        owner: section(s).bag(2).unwrap().owner,
        inventory: "bag".into(),
        stack_id: section(s).bag(2).unwrap().inventories[0].slots[slot]
            .as_ref()
            .unwrap()
            .id,
    };
    let dest = |slot| PlayerSlotEndpointV1 {
        owner: PlayerInventoryOwnerV1 {
            player_id: 2,
            incarnation: 1,
        },
        inventory: "bag".into(),
        slot,
    };
    let before = state(&mut app);
    let req = local(
        &before,
        2,
        PlayerInventoryActionV1::Use {
            source: source(&before, 1),
        },
    );
    assert_eq!(
        apply_player_inventory_action(app.world_mut(), key(2), 2, &req).unwrap(),
        InventoryActionOutcomeV1::NoChange
    );
    assert_eq!(state(&mut app), before);
    let mut checkpoint = before
        .native_players
        .as_ref()
        .unwrap()
        .projection_by_id(2)
        .unwrap()
        .checkpoint;
    checkpoint.vitals.food = 50;
    icarus_bevy::native_player_runtime::edit_player_checkpoint(
        app.world_mut(),
        key(2),
        2,
        req.expected_player_revision,
        checkpoint,
    )
    .unwrap();
    let s = state(&mut app);
    let req = local(
        &s,
        2,
        PlayerInventoryActionV1::Use {
            source: source(&s, 1),
        },
    );
    apply_player_inventory_action(app.world_mut(), key(2), 2, &req).unwrap();
    let s = state(&mut app);
    assert_eq!(section(&s).bag(2).unwrap().consumed[0].count, 1);
    assert_eq!(
        s.native_players
            .as_ref()
            .unwrap()
            .projection_by_id(2)
            .unwrap()
            .checkpoint
            .vitals
            .food,
        53
    );
    let req = local(
        &s,
        2,
        PlayerInventoryActionV1::Move {
            source: source(&s, 1),
            destination: dest(2),
        },
    );
    apply_player_inventory_action(app.world_mut(), key(2), 2, &req).unwrap();
    let s = state(&mut app);
    let req = local(
        &s,
        2,
        PlayerInventoryActionV1::Split {
            source: source(&s, 2),
            destination: dest(1),
            amount: 1,
        },
    );
    apply_player_inventory_action(app.world_mut(), key(2), 2, &req).unwrap();
    let s = state(&mut app);
    let req = local(
        &s,
        2,
        PlayerInventoryActionV1::Merge {
            source: source(&s, 1),
            destination: source(&s, 2),
            amount: 1,
        },
    );
    apply_player_inventory_action(app.world_mut(), key(2), 2, &req).unwrap();
    let s = state(&mut app);
    let req = local(
        &s,
        2,
        PlayerInventoryActionV1::Consume {
            source: source(&s, 2),
            amount: 1,
        },
    );
    apply_player_inventory_action(app.world_mut(), key(2), 2, &req).unwrap();
    let s = state(&mut app);
    let mut back = transfer(&s, 2, 1, 2, false);
    back.source = source(&s, 2);
    apply_player_inventory_transfer(app.world_mut(), key(2), 2, key(1), 1, &back).unwrap();
    let s = state(&mut app);
    assert_eq!(section(&s).bag(2).unwrap().consumed[0].count, 2);
    assert_eq!(section(&s).bag(2).unwrap().received[0].count, 4);
    assert_eq!(section(&s).bag(2).unwrap().sent[0].count, 2);
    assert_eq!(section(&s).bag(1).unwrap().sent[0].count, 4);
    assert_eq!(section(&s).bag(1).unwrap().received[0].count, 2);
    s.validate().unwrap();
}
#[test]
fn third_player_leave_before_or_during_writer_allows_unrelated_transfer() {
    for in_flight in [false, true] {
        let mut app = app_with(false, true);
        let before = state(&mut app);
        let revision = before
            .native_players
            .as_ref()
            .unwrap()
            .projection_by_id(3)
            .unwrap()
            .revision;
        app.world_mut()
            .resource_mut::<PlayerRegistry>()
            .request_leave(key(3), 3, revision, Instant::now(), Duration::from_secs(10))
            .unwrap();
        let dir = tempfile::tempdir().unwrap();
        let mut writer = CheckpointWriter::spawn(SnapshotStore::open(dir.path()).unwrap()).unwrap();
        if in_flight {
            writer.submit(app.world_mut()).unwrap();
        }
        apply(&mut app, &transfer(&before, 1, 2, 2, false));
        let expected = state(&mut app);
        if !in_flight {
            writer.submit(app.world_mut()).unwrap();
        }
        if in_flight {
            let revision = expected
                .native_players
                .as_ref()
                .unwrap()
                .projection_by_id(1)
                .unwrap()
                .revision;
            app.world_mut()
                .resource_mut::<PlayerRegistry>()
                .request_leave(key(1), 1, revision, Instant::now(), Duration::from_secs(10))
                .unwrap();
        }
        assert!(app.world_mut().resource_mut::<PlayerRegistry>().detach(3));
        let receipt = writer
            .finish_before(app.world_mut(), Instant::now() + Duration::from_secs(5))
            .unwrap()
            .unwrap();
        {
            let mut registry = app.world_mut().resource_mut::<PlayerRegistry>();
            assert_eq!(
                registry.leave_status(key(3), 3).unwrap().phase,
                LeavePhase::Durable {
                    generation: receipt.generation,
                    tick: receipt.tick
                }
            );
            if in_flight {
                assert_eq!(
                    registry.leave_status(key(1), 1).unwrap().phase,
                    LeavePhase::Pending
                );
            }
            assert_eq!(registry.settle_detached_durable_leaves(), 1);
        }
        if in_flight {
            writer.submit(app.world_mut()).unwrap();
            writer
                .finish_before(app.world_mut(), Instant::now() + Duration::from_secs(5))
                .unwrap()
                .unwrap();
        }
        drop(writer);
        let store = reopen(dir.path());
        assert_eq!(store.load().unwrap().unwrap().snapshot, expected);
    }
}
#[test]
fn either_endpoint_leave_detached_and_pending_attachments_refuse() {
    for endpoint in [1, 2] {
        let mut app = app_with(false, false);
        let before = state(&mut app);
        let r = transfer(&before, 1, 2, 1, false);
        let revision = before
            .native_players
            .as_ref()
            .unwrap()
            .projection_by_id(endpoint)
            .unwrap()
            .revision;
        app.world_mut()
            .resource_mut::<PlayerRegistry>()
            .request_leave(
                key(endpoint),
                endpoint,
                revision,
                Instant::now(),
                Duration::from_secs(5),
            )
            .unwrap();
        assert!(
            apply_player_inventory_transfer(app.world_mut(), key(1), 1, key(2), 2, &r).is_err()
        );
        assert_eq!(state(&mut app), before);
        app.world_mut()
            .resource_mut::<PlayerRegistry>()
            .detach(endpoint);
        assert!(
            apply_player_inventory_transfer(app.world_mut(), key(1), 1, key(2), 2, &r).is_err()
        );
        assert_eq!(state(&mut app), before);
    }
    let mut app = app_with(false, false);
    let before = state(&mut app);
    let r = transfer(&before, 1, 2, 1, false);
    app.world_mut().resource_mut::<PlayerRegistry>().detach(2);
    app.world_mut()
        .resource_mut::<PlayerRegistry>()
        .reserve(key(2), 4, checkpoint())
        .unwrap();
    assert!(apply_player_inventory_transfer(app.world_mut(), key(1), 1, key(2), 4, &r).is_err());
    assert_eq!(state(&mut app), before);
    activate_player(app.world_mut(), 4).unwrap();
    assert_eq!(state(&mut app), before);
    apply_player_inventory_transfer(app.world_mut(), key(1), 1, key(2), 4, &r).unwrap();
}
fn files(path: &Path) -> Vec<(String, Vec<u8>)> {
    let mut files = fs::read_dir(path)
        .unwrap()
        .map(|v| {
            let v = v.unwrap();
            (
                v.file_name().into_string().unwrap(),
                fs::read(v.path()).unwrap(),
            )
        })
        .collect::<Vec<_>>();
    files.sort();
    files
}
#[test]
fn stopped_store_cli_lock_dry_run_apply_replay_and_reopen() {
    let mut app = app_with(false, true);
    let before = state(&mut app);
    let r = transfer(&before, 1, 2, 2, false);
    let dir = tempfile::tempdir().unwrap();
    let state_dir = dir.path().join("state");
    let mut store = SnapshotStore::open(&state_dir).unwrap();
    store.save(&before).unwrap();
    let request = dir.path().join("request.json");
    fs::write(&request, serde_json::to_vec(&r).unwrap()).unwrap();
    let run = |dry: bool| {
        let mut c = std::process::Command::new(env!("CARGO_BIN_EXE_icarus-server"));
        c.args(["apply-player-inventory-transfer", "--data-dir"])
            .arg(&state_dir)
            .arg("--request")
            .arg(&request);
        if dry {
            c.arg("--dry-run");
        }
        c.output().unwrap()
    };
    let original = files(&state_dir);
    assert!(!run(false).status.success());
    assert_eq!(files(&state_dir), original);
    drop(store);
    assert!(run(true).status.success());
    assert_eq!(files(&state_dir), original);
    let result = run(false);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(report["published"], true);
    assert!(report.get("receipt").is_some());
    let store = reopen(&state_dir);
    assert_eq!(
        store.load().unwrap().unwrap().snapshot,
        prepare_transfer_offline(&before, &r).unwrap().0
    );
    drop(store);
    let saved = files(&state_dir);
    assert!(!run(false).status.success());
    assert_eq!(files(&state_dir), saved);
}

fn set_player_revision(s: &mut WorldSnapshot, id: u64, revision: u64) {
    let mut value = serde_json::to_value(&*s).unwrap();
    let record = value["native_players"]["records"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|r| r["id"] == id)
        .unwrap();
    record["revision"] = json!(revision);
    *s = serde_json::from_value(value).unwrap();
}

#[test]
fn policy_mode_pairing_reopen_matrix_and_default_false_encoding() {
    use icarus_bevy::{
        config::NetworkConfig,
        experimental::release::{DEDICATED_SERVER_APP_ID, ReleasePin, network_checksum},
        network::{PreparedPlayers, fresh_player_snapshot, open_player_snapshot},
    };
    let omitted: NetworkConfig = toml::from_str("").unwrap();
    let explicit: NetworkConfig = toml::from_str("player_inventory_transfers = false").unwrap();
    assert_eq!(omitted, explicit);
    assert_eq!(
        serde_json::to_vec(&omitted).unwrap(),
        serde_json::to_vec(&explicit).unwrap()
    );
    assert!(
        !serde_json::to_string(&omitted)
            .unwrap()
            .contains("player_inventory_transfers")
    );
    let mut invalid = omitted;
    invalid.player_inventory_transfers = true;
    assert!(invalid.validate().is_err());
    for recipe2 in [false, true] {
        let recipe = recipe();
        let compiled = if recipe2 {
            CompiledPlayerInventoryRecipe::compile_versioned(
                PlayerInventoryRecipeV2 {
                    format: 2,
                    items: recipe.items,
                    inventories: recipe.inventories,
                    starter: recipe.starter,
                    consumables: vec![PlayerConsumableV2 {
                        item: "authored-token".into(),
                        oxygen: 1,
                        water: 0,
                        food: 0,
                    }],
                }
                .into(),
            )
            .unwrap()
        } else {
            CompiledPlayerInventoryRecipe::compile(recipe).unwrap()
        };
        let mut prepared = PreparedPlayers {
            context: context(),
            initial_vitals: checkpoint().vitals,
            inventory: Some(compiled),
            inventory_mode: PlayerInventoryMode::LocalV1,
            release: ReleasePin {
                app_id: DEDICATED_SERVER_APP_ID,
                build_id: 1,
                project_version: "1.0.0.1".into(),
                network_checksum: network_checksum("1.0.0.1"),
            },
        };
        for selected in [
            PlayerInventoryMode::LocalV1,
            PlayerInventoryMode::TransfersV2,
        ] {
            prepared.inventory_mode = selected;
            let fresh = fresh_player_snapshot(Some(&prepared));
            fresh.validate().unwrap();
            assert_eq!(fresh.schema_version, selected.snapshot_schema());
            let bytes = serde_json::to_vec(&fresh).unwrap();
            let dir = tempfile::tempdir().unwrap();
            let mut store = SnapshotStore::open(dir.path()).unwrap();
            store.save(&fresh).unwrap();
            let physical = files(dir.path());
            for requested in [
                PlayerInventoryMode::LocalV1,
                PlayerInventoryMode::TransfersV2,
            ] {
                prepared.inventory_mode = requested;
                let mut copy = store.load().unwrap().unwrap().snapshot;
                let result = open_player_snapshot(&mut copy, Some(&prepared));
                assert_eq!(result.is_ok(), requested == selected);
                if let Err(error) = &result {
                    let message = error.to_string();
                    assert!(
                        message.contains("network.player_persistence")
                            && message.contains("network.player_inventory_transfers")
                    );
                }
                assert_eq!(serde_json::to_vec(&copy).unwrap(), bytes);
                assert_eq!(files(dir.path()), physical);
            }
            let mut omitted = fresh.clone();
            assert!(open_player_snapshot(&mut omitted, None).is_err());
            assert_eq!(omitted, fresh);
            let mut mismatched = fresh.clone();
            mismatched.schema_version = if selected == PlayerInventoryMode::LocalV1 {
                16
            } else {
                15
            };
            assert!(mismatched.validate().is_err());
        }
    }
}
fn replace_current(path: &Path, bytes: &[u8]) {
    use sha2::{Digest, Sha256};
    let manifest_path = path.join("manifest.json");
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
    let generation = manifest["current"]["number"].as_u64().unwrap();
    fs::write(path.join(format!("snapshot-{generation}.json")), bytes).unwrap();
    manifest["current"]["sha256"] = json!(
        Sha256::digest(bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    );
    fs::write(manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
}
#[test]
fn checksummed_tampering_known_recovery_and_terminal_future_formats() {
    let mut app = app_with(false, true);
    let before = state(&mut app);
    apply(&mut app, &transfer(&before, 1, 2, 2, false));
    let after = state(&mut app);
    for previous in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let mut store = SnapshotStore::open(dir.path()).unwrap();
        if previous {
            store.save(&before).unwrap();
        }
        store.save(&after).unwrap();
        let mut bad = after.clone();
        let i = section_mut(&mut bad);
        let stack = i.bags[1].inventories[0].slots[1].take();
        i.bags[0].inventories[0].slots[1] = stack;
        replace_current(dir.path(), &serde_json::to_vec(&bad).unwrap());
        let result = store.load();
        if previous {
            assert_eq!(result.unwrap().unwrap().snapshot, before);
        } else {
            assert!(result.is_err());
        }
    }
    for text in ["17", "16,\"schema_version\":17", "17,\"schema_version\":16"] {
        let dir = tempfile::tempdir().unwrap();
        let mut store = SnapshotStore::open(dir.path()).unwrap();
        store.save(&before).unwrap();
        store.save(&after).unwrap();
        let bytes = format!("{{\"schema_version\":{text},\"tick\":null}}");
        replace_current(dir.path(), bytes.as_bytes());
        let unchanged = files(dir.path());
        assert!(store.load().is_err());
        assert!(store.save(&after).is_err());
        assert_eq!(files(dir.path()), unchanged);
    }
    for key in ["section", "recipe"] {
        for bad in ["3", "0", "-1", "null", "true", "\"2\"", "2.0"] {
            for first in [false, true] {
                let dir = tempfile::tempdir().unwrap();
                let mut store = SnapshotStore::open(dir.path()).unwrap();
                store.save(&before).unwrap();
                store.save(&after).unwrap();
                let pair = if first {
                    format!("\"format\":{bad},\"format\":2")
                } else {
                    format!("\"format\":2,\"format\":{bad}")
                };
                let section = if key == "section" {
                    format!("{{{pair},\"bags\":null}}")
                } else {
                    format!("{{\"format\":2,\"recipe\":{{{pair}}},\"bags\":null}}")
                };
                let bytes = format!("{{\"schema_version\":16,\"player_inventory\":{section}}}");
                replace_current(dir.path(), bytes.as_bytes());
                let unchanged = files(dir.path());
                assert!(store.load().is_err());
                assert!(store.save(&after).is_err());
                assert_eq!(files(dir.path()), unchanged);
            }
        }
    }
}

#[test]
fn different_inventory_destinations_and_sparse_zero_starter_validation() {
    let mut app = app_with(false, true);
    let before = state(&mut app);
    let mut req = transfer(&before, 1, 2, 2, false);
    req.destination.inventory = "spare".into();
    req.destination.slot = 0;
    apply(&mut app, &req);
    let after = state(&mut app);
    assert_eq!(
        section(&after).bag(2).unwrap().inventories[1].slots[0]
            .as_ref()
            .unwrap()
            .count,
        2
    );
    for kind in ["zero", "unknown", "duplicate", "order"] {
        let mut bad = after.clone();
        let b = &mut section_mut(&mut bad).bags[1];
        match kind {
            "zero" => b.received[0].count = 0,
            "unknown" => b.received[0].item = "unknown".into(),
            "duplicate" => {
                let row = b.received[0].clone();
                b.received.push(row);
            }
            _ => {
                b.received.insert(
                    0,
                    icarus_bevy::native_inventory::ItemTotalV1 {
                        item: "zero".into(),
                        count: 1,
                    },
                );
            }
        }
        assert!(bad.validate().is_err());
    }
    let mut forged = before.clone();
    section_mut(&mut forged).bags[0]
        .received
        .push(icarus_bevy::native_inventory::ItemTotalV1 {
            item: "zero".into(),
            count: 1,
        });
    assert!(forged.validate().is_err());
}
#[test]
fn received_owner_overflow_placement_keeps_flow_rows_and_can_grow() {
    for grow in [false, true] {
        let mut app = app_with(true, true);
        let before = state(&mut app);
        apply(&mut app, &transfer(&before, 1, 2, 2, false));
        let before = state(&mut app);
        let b = section(&before).bag(2).unwrap();
        let req = local(
            &before,
            2,
            PlayerInventoryActionV1::PlaceOverflow {
                source: PlayerStackEndpointV1 {
                    owner: b.owner,
                    inventory: "bag".into(),
                    stack_id: b.inventories[0].overflow[0].stack.id,
                },
                destination: PlayerOverflowDestinationV1 {
                    owner: b.owner,
                    inventory: "bag".into(),
                    slot: if grow { None } else { Some(2) },
                },
            },
        );
        apply_player_inventory_action(app.world_mut(), key(2), 2, &req).unwrap();
        let after = state(&mut app);
        assert_eq!(section(&after).bag(2).unwrap().received, b.received);
        assert_eq!(section(&after).bag(2).unwrap().sent, b.sent);
        assert!(
            section(&after).bag(2).unwrap().inventories[0]
                .overflow
                .is_empty()
        );
        after.validate().unwrap();
    }
}

fn survival_config() -> ServerConfig {
    use icarus_bevy::{
        authored_world_bootstrap::{AuthoredClockConfig, AuthoredWorldBootstrap},
        config::{NetworkConfig, PlayerPersistenceConfig},
        native_player_runtime::{ConsumptionConfig, DepletionDamageConfig},
    };
    ServerConfig {
        tick_hz: 7,
        network: NetworkConfig {
            player_persistence: Some(PlayerPersistenceConfig {
                prospect_id: "authored-bag-world".into(),
                map_url: "/Authored/Bags".into(),
                prospect_row: "AuthoredBags".into(),
                difficulty: 2,
            }),
            release_profile: Some("unused-release.toml".into()),
            authored_gameplay: Some("unused-gameplay.json".into()),
            data_pak: Some("unused-catalog.pak".into()),
            player_inventory: Some("unused-recipe.json".into()),
            player_inventory_transfers: true,
            authored_world: Some(AuthoredWorldBootstrap {
                seed: 1,
                seed_initialised: true,
                flod_tile_count: 0,
                clock: Some(AuthoredClockConfig {
                    initial_minute: 480,
                    seconds_per_day: 3600,
                    tick_hz: 7,
                }),
            }),
            player_consumption: Some(ConsumptionConfig {
                oxygen: 0,
                water: 0,
                food: 7,
            }),
            player_depletion_damage: Some(DepletionDamageConfig {
                oxygen: 0,
                water: 0,
                food: 7,
            }),
            ..Default::default()
        },
        ..Default::default()
    }
}
#[test]
fn schema16_survival_scheduled_direct_phase_and_inactive_leave_exclusion() {
    use icarus_bevy::native_player_runtime::{
        ConsumptionRates, DepletionDamageRates, advance_player_survival, edit_player_checkpoint,
    };
    let mut app = app_with(false, true);
    let before = state(&mut app);
    apply(&mut app, &transfer(&before, 1, 2, 2, false));
    let mut before = state(&mut app);
    before.tick = 1;
    let config = survival_config();
    let mut scheduled = build_app(&config, &before).unwrap();
    let mut direct = build_app(&config, &before).unwrap();
    for app in [&mut scheduled, &mut direct] {
        for n in 1..=3 {
            attach(app, n, n);
        }
        let p = state(app)
            .native_players
            .unwrap()
            .projection_by_id(1)
            .unwrap();
        let mut c = p.checkpoint;
        c.vitals.food = 0;
        edit_player_checkpoint(app.world_mut(), key(1), 1, p.revision, c).unwrap();
        app.world_mut().resource_mut::<PlayerRegistry>().detach(2);
        let revision = state(app)
            .native_players
            .unwrap()
            .projection_by_id(3)
            .unwrap()
            .revision;
        app.world_mut()
            .resource_mut::<PlayerRegistry>()
            .request_leave(key(3), 3, revision, Instant::now(), Duration::from_secs(10))
            .unwrap();
    }
    let frozen = state(&mut direct);
    let inventory = frozen.player_inventory.clone();
    advance_player_survival(
        direct.world_mut(),
        ConsumptionRates::new(0, 0, 1).unwrap(),
        DepletionDamageRates::new(0, 0, 1).unwrap(),
    )
    .unwrap();
    scheduled
        .world_mut()
        .run_schedule(bevy::prelude::FixedUpdate);
    let d = state(&mut direct);
    let mut s = state(&mut scheduled);
    assert_eq!(s.tick, d.tick + 1);
    s.tick = d.tick;
    assert_eq!(s, d);
    assert_eq!(d.player_inventory, inventory);
    assert_eq!(
        d.native_players
            .as_ref()
            .unwrap()
            .projection_by_id(1)
            .unwrap()
            .checkpoint
            .vitals
            .health,
        99
    );
    for id in [2, 3] {
        assert_eq!(
            d.native_players.as_ref().unwrap().projection_by_id(id),
            frozen.native_players.as_ref().unwrap().projection_by_id(id)
        );
    }
}
#[test]
fn direct_transfer_latches_missing_fixed_owner_and_registry_replacement() {
    let mut app = app_with(false, true);
    let before = state(&mut app);
    let config = survival_config();
    let mut app = build_app(&config, &before).unwrap();
    for n in 1..=3 {
        attach(&mut app, n, n);
    }
    let request = transfer(&before, 1, 2, 1, false);
    let players = app.world().resource::<PlayerRegistry>().snapshot();
    let inventory = app
        .world()
        .resource::<PlayerRegistry>()
        .inventory_snapshot();
    let fixed = app
        .world_mut()
        .remove_resource::<bevy::prelude::Time<bevy::prelude::Fixed>>()
        .unwrap();
    assert!(
        apply_player_inventory_transfer(app.world_mut(), key(1), 1, key(2), 2, &request).is_err()
    );
    app.insert_resource(fixed);
    assert!(
        apply_player_inventory_transfer(app.world_mut(), key(1), 1, key(2), 2, &request).is_err()
    );
    assert!(capture(app.world_mut()).is_err());
    assert_eq!(app.world().resource::<PlayerRegistry>().snapshot(), players);
    assert_eq!(
        app.world()
            .resource::<PlayerRegistry>()
            .inventory_snapshot(),
        inventory
    );
    let mut app = build_app(&ServerConfig::default(), &before).unwrap();
    let registry = PlayerRegistry::new_with_inventory(players, &context(), 64, inventory).unwrap();
    app.insert_resource(registry);
    assert!(capture(app.world_mut()).is_err());
}

#[test]
fn partial_transfer_and_new_owner_refuse_global_stack_cap_without_orphans() {
    use icarus_bevy::native_inventory::{InventoryDefinitionV1, ItemDefinitionV1, SlotRuleV1};
    let compiled = CompiledPlayerInventoryRecipe::compile(PlayerInventoryRecipeV1 {
        format: 1,
        items: vec![ItemDefinitionV1 {
            key: "i".into(),
            stack_limit: 10,
        }],
        inventories: vec![
            InventoryDefinitionV1 {
                key: "bag".into(),
                initial_slots: 256,
                slot_rules: vec![
                    SlotRuleV1 {
                        allowed_items: vec!["i".into()]
                    };
                    256
                ],
                force_add_when_zero: false,
                overflow_limit: 0,
            },
            InventoryDefinitionV1 {
                key: "spare".into(),
                initial_slots: 1,
                slot_rules: vec![SlotRuleV1 {
                    allowed_items: vec!["i".into()],
                }],
                force_add_when_zero: false,
                overflow_limit: 0,
            },
        ],
        starter: (0..256)
            .map(|slot| PlayerStarterItemV1 {
                inventory: "bag".into(),
                requested_slot: slot,
                item: "i".into(),
                count: 2,
            })
            .collect(),
    })
    .unwrap();
    let empty = WorldSnapshot {
        schema_version: 16,
        native_players: Some(PlayerSnapshot::empty(context())),
        player_inventory: Some(compiled.empty_v2().into()),
        ..Default::default()
    };
    let mut app = build_app(&ServerConfig::default(), &empty).unwrap();
    attach(&mut app, 1, 1);
    let mut s = state(&mut app);
    drop(app);
    let template = section(&s).bags[0].clone();
    let mut registry =
        PlayerRegistry::new(PlayerSnapshot::empty(context()), &context(), 64).unwrap();
    for id in 1..=256 {
        registry.reserve(key(id), id, checkpoint()).unwrap();
        registry.activate(id).unwrap();
        registry.detach(id);
    }
    s.native_players = Some(registry.snapshot());
    let inventory = section_mut(&mut s);
    inventory.bags.clear();
    for id in 1..=256 {
        let mut bag = template.clone();
        bag.owner.player_id = id;
        for stack in bag.inventories[0].slots.iter_mut().flatten() {
            stack.id = StackId::new((id - 1) * 256 + stack.id.get()).unwrap();
        }
        inventory.bags.push(bag);
    }
    inventory.next_stack_id = 65537;
    inventory.revision = 257;
    s.validate().unwrap();
    let mut req = transfer(&s, 1, 2, 1, false);
    req.destination.inventory = "spare".into();
    req.destination.slot = 0;
    assert!(prepare_transfer_offline(&s, &req).is_err());
    req.amount = 2;
    prepare_transfer_offline(&s, &req).unwrap();
    let mut app = build_app(&ServerConfig::default(), &s).unwrap();
    app.world_mut()
        .resource_mut::<PlayerRegistry>()
        .reserve(key(257), 300, checkpoint())
        .unwrap();
    let before = state(&mut app);
    assert!(activate_player(app.world_mut(), 300).is_err());
    assert_eq!(state(&mut app), before);
}
#[test]
fn full_generation_transfer_preflight_failure_preserves_both_players_and_clock() {
    use icarus_bevy::simulation::{DomainId, EntityRecord, Position};
    let mut app = app_with(false, true);
    let mut s = state(&mut app);
    drop(app);
    let req = transfer(&s, 1, 2, 2, false);
    let candidate = prepare_transfer_offline(&s, &req).unwrap().0;
    let gain =
        serde_json::to_vec(&candidate).unwrap().len() - serde_json::to_vec(&s).unwrap().len();
    assert!(gain > 0);
    drop(candidate);
    let cap = 64 * 1024 * 1024;
    let mut length = serde_json::to_vec(&s).unwrap().len();
    for id in 1.. {
        let e = EntityRecord {
            id: DomainId(id),
            position: Position([0.; 3]),
        };
        let n = serde_json::to_vec(&e).unwrap().len() + usize::from(!s.entities.is_empty());
        if length + n > cap {
            break;
        }
        length += n;
        s.entities.push(e);
    }
    assert!(cap - length < gain);
    SnapshotStore::preflight(&s).unwrap();
    let mut app = build_app(&ServerConfig::default(), &s).unwrap();
    for id in 1..=3 {
        attach(&mut app, id, id);
    }
    let before = state(&mut app);
    assert!(apply_player_inventory_transfer(app.world_mut(), key(1), 1, key(2), 2, &req).is_err());
    assert!(state(&mut app) == before);
}

#[test]
fn each_flow_counter_exhausts_independently_and_global_sums_stay_wide() {
    use icarus_bevy::native_inventory::ItemTotalV1;
    let mut app = app_with(false, true);
    let base = state(&mut app);
    for owner in [1usize, 2] {
        let mut s = base.clone();
        let bag = &mut section_mut(&mut s).bags[owner - 1];
        let row = ItemTotalV1 {
            item: "authored-token".into(),
            count: u64::MAX,
        };
        bag.received.push(row.clone());
        bag.sent.push(row);
        s.validate().unwrap();
        assert!(prepare_transfer_offline(&s, &transfer(&s, 1, 2, 1, false)).is_err());
    }
    let mut s = base.clone();
    let bag = &mut section_mut(&mut s).bags[1];
    let row = ItemTotalV1 {
        item: "authored-token".into(),
        count: u64::MAX - 1,
    };
    bag.received.push(row.clone());
    bag.sent.push(row);
    s.validate().unwrap();
    let (s, _) = prepare_transfer_offline(&s, &transfer(&s, 1, 2, 1, false)).unwrap();
    let mut back = transfer(&s, 2, 1, 1, false);
    back.source.stack_id = section(&s).bag(2).unwrap().inventories[0].slots[1]
        .as_ref()
        .unwrap()
        .id;
    let (s, _) = prepare_transfer_offline(&s, &back).unwrap();
    s.validate().unwrap();
    assert_eq!(section(&s).bag(1).unwrap().received[0].count, 1);
    assert_eq!(section(&s).bag(2).unwrap().received[0].count, u64::MAX);
}
#[test]
fn zero_health_adds_no_transfer_gate_or_healing() {
    let mut app = app_with(false, true);
    let before = state(&mut app);
    let p = before
        .native_players
        .as_ref()
        .unwrap()
        .projection_by_id(1)
        .unwrap();
    let mut c = p.checkpoint;
    c.vitals.health = 0;
    icarus_bevy::native_player_runtime::edit_player_checkpoint(
        app.world_mut(),
        key(1),
        1,
        p.revision,
        c,
    )
    .unwrap();
    let before = state(&mut app);
    apply(&mut app, &transfer(&before, 1, 2, 1, false));
    let after = state(&mut app);
    assert_eq!(
        after
            .native_players
            .as_ref()
            .unwrap()
            .projection_by_id(1)
            .unwrap()
            .checkpoint,
        c
    );
}

#[test]
fn schema15_full_store_bytes_match_the_concrete_legacy_projection_for_both_recipes() {
    #[derive(serde::Serialize)]
    struct LegacyGeneration<'a> {
        schema_version: u32,
        tick: u64,
        entities: &'a [icarus_bevy::simulation::EntityRecord],
        native_players: &'a PlayerSnapshot,
        player_inventory: &'a PlayerInventorySnapshotV1,
    }
    for consumable in [false, true] {
        let r = recipe();
        let compiled = if consumable {
            CompiledPlayerInventoryRecipe::compile_versioned(
                PlayerInventoryRecipeV2 {
                    format: 2,
                    items: r.items,
                    inventories: r.inventories,
                    starter: r.starter,
                    consumables: vec![PlayerConsumableV2 {
                        item: "authored-token".into(),
                        oxygen: 1,
                        water: 0,
                        food: 0,
                    }],
                }
                .into(),
            )
            .unwrap()
        } else {
            CompiledPlayerInventoryRecipe::compile(r).unwrap()
        };
        let concrete = compiled.empty();
        let s = WorldSnapshot {
            schema_version: 15,
            native_players: Some(PlayerSnapshot::empty(context())),
            player_inventory: Some(concrete.clone().into()),
            ..Default::default()
        };
        // Frozen field types/order from the base's PlayerInventoryOut; the
        // concrete section deliberately bypasses the new carrier serializer.
        let expected = serde_json::to_vec(&LegacyGeneration {
            schema_version: 15,
            tick: 0,
            entities: &[],
            native_players: s.native_players.as_ref().unwrap(),
            player_inventory: &concrete,
        })
        .unwrap();
        let directory = tempfile::tempdir().unwrap();
        let mut store = SnapshotStore::open(directory.path()).unwrap();
        let receipt = store.save(&s).unwrap();
        let bytes = fs::read(
            directory
                .path()
                .join(format!("snapshot-{}.json", receipt.generation)),
        )
        .unwrap();
        assert_eq!(bytes, expected);
        // Empty stores omit unused installed growth-catalog rows on decode.
        // Canonical stored state is exact; runtime catalog binding happens at open.
        assert_eq!(
            serde_json::to_vec(&store.load().unwrap().unwrap().snapshot).unwrap(),
            serde_json::to_vec(&s).unwrap()
        );
    }
}
#[test]
fn request_cas_and_endpoint_nested_shapes_duplicates_and_nulls_refuse() {
    let mut app = app_with(false, false);
    let before = state(&mut app);
    let r = transfer(&before, 1, 2, 1, false);
    let value = serde_json::to_value(&r).unwrap();
    for path in ["source", "destination", "source_cas", "destination_cas"] {
        for shape in [serde_json::Value::Null, json!([])] {
            let mut v = value.clone();
            v[path] = shape;
            assert!(serde_json::from_value::<PlayerInventoryTransferRequestV1>(v).is_err());
        }
    }
    let text = serde_json::to_string(&r).unwrap();
    for field in [
        "amount",
        "expected_player_revision",
        "expected_bag_revision",
        "inventory",
        "player_id",
        "incarnation",
    ] {
        let token = format!("\"{field}\":");
        let a = text.find(&token).unwrap();
        let start = a + token.len();
        let end = text[start..].find([',', '}']).unwrap() + start;
        let encoded = &text[a..end];
        let duplicate = format!("{},{}", encoded, encoded);
        let malformed = text.replacen(encoded, &duplicate, 1);
        assert!(serde_json::from_str::<PlayerInventoryTransferRequestV1>(&malformed).is_err());
    }
}

#[test]
fn physical_generation_selection_never_searches_for_configured_policy() {
    use icarus_bevy::{
        experimental::release::{DEDICATED_SERVER_APP_ID, ReleasePin, network_checksum},
        network::{PreparedPlayers, fresh_player_snapshot, open_player_snapshot},
    };
    let mut prepared = PreparedPlayers {
        context: context(),
        initial_vitals: checkpoint().vitals,
        inventory: Some(CompiledPlayerInventoryRecipe::compile(recipe()).unwrap()),
        inventory_mode: PlayerInventoryMode::LocalV1,
        release: ReleasePin {
            app_id: DEDICATED_SERVER_APP_ID,
            build_id: 1,
            project_version: "1.0.0.1".into(),
            network_checksum: network_checksum("1.0.0.1"),
        },
    };
    let local = fresh_player_snapshot(Some(&prepared));
    prepared.inventory_mode = PlayerInventoryMode::TransfersV2;
    let transfers = fresh_player_snapshot(Some(&prepared));
    for scenario in [
        "valid_local_current",
        "corrupt_transfer_current",
        "valid_transfer_current",
        "corrupt_local_current",
    ] {
        let directory = tempfile::tempdir().unwrap();
        let mut store = SnapshotStore::open(directory.path()).unwrap();
        let (previous, current) =
            if matches!(scenario, "valid_local_current" | "corrupt_local_current") {
                (&transfers, &local)
            } else {
                (&local, &transfers)
            };
        store.save(previous).unwrap();
        store.save(current).unwrap();
        if scenario == "corrupt_transfer_current" {
            let mut value = serde_json::to_value(current).unwrap();
            value["player_inventory"]["format"] = json!(1);
            replace_current(directory.path(), &serde_json::to_vec(&value).unwrap());
        }
        if scenario == "corrupt_local_current" {
            replace_current(directory.path(), b"broken");
        }
        let expected = if scenario.starts_with("corrupt") {
            previous
        } else {
            current
        };
        let selected = store.load().unwrap().unwrap().snapshot;
        assert_eq!(
            serde_json::to_vec(&selected).unwrap(),
            serde_json::to_vec(expected).unwrap()
        );
        let disk = files(directory.path());
        for mode in [
            PlayerInventoryMode::LocalV1,
            PlayerInventoryMode::TransfersV2,
        ] {
            prepared.inventory_mode = mode;
            let mut copy = selected.clone();
            let result = open_player_snapshot(&mut copy, Some(&prepared));
            assert_eq!(
                result.is_ok(),
                mode.snapshot_schema() == expected.schema_version
            );
            if result.is_ok() {
                assert_eq!(&copy, expected);
            } else {
                assert_eq!(copy, selected);
            }
            assert_eq!(files(directory.path()), disk);
        }
    }
}

#[test]
fn received_use_nochange_needs_no_revision_or_allocator_headroom() {
    let mut app = app_with(false, true);
    let before = state(&mut app);
    apply(&mut app, &transfer(&before, 1, 2, 1, false));
    let mut full = state(&mut app);
    set_player_revision(&mut full, 2, u64::MAX);
    let inventory = section_mut(&mut full);
    inventory.bags[1].revision = u64::MAX;
    inventory.revision = u64::MAX;
    inventory.next_stack_id = u64::MAX;
    full.validate().unwrap();
    let bag = section(&full).bag(2).unwrap();
    let req = local(
        &full,
        2,
        PlayerInventoryActionV1::Use {
            source: PlayerStackEndpointV1 {
                owner: bag.owner,
                inventory: "bag".into(),
                stack_id: bag.inventories[0].slots[1].as_ref().unwrap().id,
            },
        },
    );
    let (after, outcome) = prepare_offline(&full, &req).unwrap();
    assert_eq!(outcome, InventoryActionOutcomeV1::NoChange);
    assert_eq!(after, full);
}

#[test]
fn boundary_v2_transfer_new_keys_and_decimal_growth_are_exact_and_live_atomic() {
    use icarus_bevy::native_inventory::ItemTotalV1;
    for existing_rows in [false, true] {
        let mut initial = app_with(false, true);
        let mut before = state(&mut initial);
        drop(initial);
        let inventory = section_mut(&mut before);
        inventory.revision = 9;
        inventory.next_stack_id = 9;
        for bag in &mut inventory.bags {
            bag.revision = 9;
            if existing_rows {
                bag.received = vec![ItemTotalV1 {
                    item: "authored-token".into(),
                    count: 9,
                }];
                bag.sent = bag.received.clone();
            }
        }
        let mut players = serde_json::to_value(before.native_players.take().unwrap()).unwrap();
        for record in players["records"].as_array_mut().unwrap() {
            record["revision"] = json!(9);
        }
        before.native_players = Some(serde_json::from_value(players).unwrap());
        before.validate().unwrap();
        let original = serde_json::to_vec(&before).unwrap();
        let req = transfer(&before, 1, 2, 1, false);
        let (after, receipt) = prepare_transfer_offline(&before, &req).unwrap();
        assert_eq!(serde_json::to_vec(&before).unwrap(), original);
        assert_eq!(section(&after).revision, 10);
        assert_eq!(section(&after).next_stack_id, 10);
        let sent = &section(&after).bag(1).unwrap().sent;
        let received = &section(&after).bag(2).unwrap().received;
        assert_eq!(sent, received);
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].count, if existing_rows { 10 } else { 1 });
        let moved = section(&after).bag(2).unwrap().inventories[0].slots[1]
            .as_ref()
            .unwrap();
        assert_eq!(moved.id.get(), 9);
        let flow_delta = if existing_rows {
            2
        } else {
            2 * serde_json::to_vec(&sent[0]).unwrap().len()
        };
        let expected_growth = 4 + serde_json::to_vec(moved).unwrap().len() - 4 + flow_delta;
        let before_bytes = serde_json::to_vec(section(&before)).unwrap().len();
        let after_bytes = serde_json::to_vec(section(&after)).unwrap().len();
        assert_eq!(after_bytes - before_bytes, expected_growth);
        assert_eq!(section(&after).bag(3), section(&before).bag(3));
        let mut live = build_app(&ServerConfig::default(), &before).unwrap();
        attach(&mut live, 1, 101);
        attach(&mut live, 2, 102);
        assert_eq!(state(&mut live), before);
        let applied =
            apply_player_inventory_transfer(live.world_mut(), key(1), 101, key(2), 102, &req)
                .unwrap();
        assert_eq!(applied, receipt);
        assert_eq!(state(&mut live), after);
        eprintln!(
            "V2 existing_rows={existing_rows} encoded before={before_bytes} after={after_bytes} growth={expected_growth}; revisions/allocator 9->10, flows {}->{}",
            if existing_rows { 9 } else { 0 },
            sent[0].count
        );
    }
}

//! Authored inventory activation, whole-world transactions and persistence.
#[path = "common/c001_inventory_owners.rs"]
mod fixture;
#[path = "common/c001_scalar_owners.rs"]
mod scalar;
#[path = "common/c001_inventory_source.rs"]
mod source;

use std::time::{Duration, Instant};

use bevy::prelude::App;
use icarus_bevy::{
    config::ServerConfig,
    native_inventory::{
        InventoryActionOutcomeV1, InventoryActionRequestV1, InventoryActionV1,
        InventoryOverflowDestinationV1, InventorySlotEndpointV1, InventoryStackEndpointV1,
        InventoryStateSnapshotV1, InventoryStateV1, OwnerInventoriesV1, StackId,
    },
    native_inventory_restore::{InventoryOwnerRestoreV1, InventoryRestoreBatchV1},
    native_lifecycle::{
        OwnerHandle, Presence, RecorderSource, ResolvePolicy, RestoreBatch, RestoreRecorder,
    },
    native_lifecycle_runtime::{
        C001InventoryOwnerImportState, InventoryStackLocationV1, NativeInventoryContractsState,
        NativeRestoreResult, apply_native_inventory_action, inventory_owner_state, inventory_stack,
        lifecycle_foundation_of, native_owner_handle, reassign_lifecycle_foundation,
        restore_native_inventory_owners, restore_native_owners, scalar_owner_state, set_enabled,
        try_use, update_lifecycle_actor,
    },
    native_scalar_owners::ScalarUseOutcome,
    persistence::{SaveWorker, SnapshotStore},
    simulation::{
        DomainId, PreservedProspectState, SNAPSHOT_SCHEMA_C001_INVENTORY_OWNERS, WorldSnapshot,
        build_app, capture,
    },
};

fn app() -> App {
    build_app(&ServerConfig::default(), &fixture::imported()).unwrap()
}

fn snapshot(app: &mut App) -> WorldSnapshot {
    capture(app.world_mut()).unwrap()
}

fn state(app: &mut App) -> InventoryStateSnapshotV1 {
    snapshot(app).native_inventories.unwrap()
}

fn handle(app: &App, id: u64) -> OwnerHandle {
    native_owner_handle(app.world(), DomainId(id)).unwrap()
}

fn inventory<'a>(owner: &'a OwnerInventoriesV1, key: &str) -> &'a InventoryStateV1 {
    owner
        .inventories
        .iter()
        .find(|inventory| inventory.key == key)
        .unwrap()
}

fn endpoint(owner: OwnerHandle, key: &str, id: StackId) -> InventoryStackEndpointV1 {
    InventoryStackEndpointV1 {
        owner,
        inventory: key.into(),
        stack_id: id,
    }
}

fn slot(owner: OwnerHandle, key: &str, slot: u16) -> InventorySlotEndpointV1 {
    InventorySlotEndpointV1 {
        owner,
        inventory: key.into(),
        slot,
    }
}

fn action(app: &mut App, action: InventoryActionV1) -> InventoryActionOutcomeV1 {
    let revision = state(app).revision;
    apply_native_inventory_action(
        app.world_mut(),
        &InventoryActionRequestV1 {
            expected_inventory_revision: revision,
            action,
        },
    )
    .unwrap()
}

fn bag_stack(app: &App, owner: OwnerHandle) -> StackId {
    inventory(
        &inventory_owner_state(app.world(), owner).unwrap().owner,
        "bag",
    )
    .slots
    .iter()
    .flatten()
    .next()
    .unwrap()
    .id
}

/// A replacement request carries the complete current graph and a single
/// lifecycle completion; it never reconstructs inventory from source records.
fn replacement(current: &WorldSnapshot) -> InventoryRestoreBatchV1 {
    let native = current.native_world.as_ref().unwrap();
    let lifecycle = current.native_lifecycle.as_ref().unwrap();
    let inventories = current.native_inventories.as_ref().unwrap();
    InventoryRestoreBatchV1 {
        format: 1,
        scalar_contract_id: current
            .native_scalar_contracts
            .as_ref()
            .unwrap()
            .contract_id
            .clone(),
        inventory_contract_id: inventories.contract_id.clone(),
        expected_inventory_revision: inventories.revision,
        lifecycle: RestoreBatch {
            lifecycle_id: lifecycle.lifecycle_id.clone(),
            sequence: lifecycle.restore_sequence + 1,
            recorders: native
                .actors
                .iter()
                .map(|actor| {
                    let declaration = lifecycle
                        .definition
                        .slots
                        .iter()
                        .find(|slot| slot.placement == actor.placement)
                        .unwrap();
                    let owner = lifecycle
                        .slots
                        .iter()
                        .find(|slot| slot.id == actor.id)
                        .unwrap();
                    RestoreRecorder {
                        guid: 900 + actor.id.0 as i32,
                        kind: declaration.recorder_kind,
                        target_placement: actor.placement.clone(),
                        owner_name: declaration.owner_name.clone(),
                        policy: ResolvePolicy::RespawnOnly,
                        source: RecorderSource::Detached {
                            expected_prior_binding: None,
                        },
                        create_if_missing: true,
                        expected_presence: Presence::Live,
                        expected_incarnation: owner.incarnation,
                        transform: actor.transform.clone(),
                        variables: actor.variables.clone(),
                    }
                })
                .collect(),
        },
        owners: native
            .actors
            .iter()
            .map(|actor| InventoryOwnerRestoreV1 {
                target_placement: actor.placement.clone(),
                inventories: inventories
                    .owners
                    .iter()
                    .find(|owner| owner.owner.id == actor.id)
                    .unwrap()
                    .inventories
                    .clone(),
            })
            .collect(),
    }
}

#[test]
fn saved_scalar_overrides_and_all_three_inventory_routes_activate_together() {
    let initial = fixture::imported();
    let mut app = build_app(&ServerConfig::default(), &initial).unwrap();
    assert!(snapshot(&mut app) == initial);
    assert_eq!(
        initial.schema_version,
        SNAPSHOT_SCHEMA_C001_INVENTORY_OWNERS
    );
    let owner = handle(&app, 3); // first source recorder, independent of native ID order
    let query = inventory_owner_state(app.world(), owner).unwrap();
    assert_eq!(query.revision, 1);
    let bag = inventory(&query.owner, "bag");
    let zero = inventory(&query.owner, "zero");
    assert_eq!(bag.slots.len(), 2);
    assert_eq!(bag.slots[0].as_ref().unwrap().count, 32);
    assert_eq!(bag.slots[0].as_ref().unwrap().id.get(), 3);
    assert_eq!(zero.slots.len(), 1);
    assert_eq!(zero.slots[0].as_ref().unwrap().id.get(), 1);
    assert_eq!(zero.slots[0].as_ref().unwrap().count, 3);
    assert_eq!(zero.overflow.len(), 1);
    assert_eq!(zero.overflow[0].stack.id.get(), 2);
    assert_eq!(zero.overflow[0].requested_slot, 0);
    assert_eq!(zero.overflow[0].stack.count, 17);
    assert_eq!(state(&mut app).next_stack_id, 5);
    let alpha = scalar_owner_state(app.world(), handle(&app, 1)).unwrap();
    assert!(!alpha.enabled);
    assert_eq!(alpha.remaining_uses, 2);
    assert_eq!(
        scalar_owner_state(app.world(), owner)
            .unwrap()
            .remaining_uses,
        1
    );
    assert!(!format!("{query:?}").contains("apple"));
}

#[test]
fn cross_owner_move_split_merge_consume_and_overflow_preserve_identity_and_conservation() {
    let mut app = app();
    let zulu = handle(&app, 3);
    let alpha = handle(&app, 1);
    let id = bag_stack(&app, zulu);
    let original = inventory_stack(app.world(), zulu, "bag", id).unwrap();
    assert_eq!(original.revision, 1);
    assert_eq!(original.owner, zulu);
    assert_eq!(original.inventory, "bag");
    assert_eq!(original.stack.id, id);
    assert_eq!(original.stack.item, "apple");
    assert_eq!(original.stack.count, 32);
    assert_eq!(
        original.location,
        InventoryStackLocationV1::Slot { index: 0 }
    );
    assert_eq!(format!("{original:?}"), "InventoryStackQueryV1 { .. }");
    assert_eq!(
        action(
            &mut app,
            InventoryActionV1::Move {
                source: endpoint(zulu, "bag", id),
                destination: slot(alpha, "bag", 0),
            }
        ),
        InventoryActionOutcomeV1::Applied
    );
    assert_eq!(bag_stack(&app, alpha), id);
    assert!(inventory_stack(app.world(), zulu, "bag", id).is_err());
    let moved = inventory_stack(app.world(), alpha, "bag", id).unwrap();
    assert_eq!(moved.revision, 2);
    assert_eq!(moved.owner, alpha);
    assert_eq!(moved.location, InventoryStackLocationV1::Slot { index: 0 });
    let split_id = StackId::new(state(&mut app).next_stack_id).unwrap();
    action(
        &mut app,
        InventoryActionV1::Split {
            source: endpoint(alpha, "bag", id),
            destination: slot(zulu, "bag", 1),
            amount: 7,
        },
    );
    let split = inventory_stack(app.world(), zulu, "bag", split_id).unwrap();
    assert_eq!(split.revision, 3);
    assert_eq!(split.stack.count, 7);
    assert_eq!(split.location, InventoryStackLocationV1::Slot { index: 1 });
    assert_eq!(
        inventory_stack(app.world(), alpha, "bag", id)
            .unwrap()
            .stack
            .count,
        25
    );
    action(
        &mut app,
        InventoryActionV1::Merge {
            source: endpoint(zulu, "bag", split_id),
            destination: endpoint(alpha, "bag", id),
            amount: 7,
        },
    );
    assert!(inventory_stack(app.world(), zulu, "bag", split_id).is_err());
    let merged = inventory_stack(app.world(), alpha, "bag", id).unwrap();
    assert_eq!(merged.revision, 4);
    assert_eq!(merged.stack.count, 32);
    action(
        &mut app,
        InventoryActionV1::Consume {
            source: endpoint(alpha, "bag", id),
            amount: 32,
        },
    );
    assert!(inventory_stack(app.world(), alpha, "bag", id).is_err());
    let deferred = inventory(
        &inventory_owner_state(app.world(), zulu).unwrap().owner,
        "zero",
    )
    .overflow[0]
        .stack
        .id;
    let overflow = inventory_stack(app.world(), zulu, "zero", deferred).unwrap();
    assert_eq!(overflow.revision, 5); // Global revision includes other owners' edits.
    assert_eq!(overflow.stack.count, 17);
    assert_eq!(
        overflow.location,
        InventoryStackLocationV1::Overflow {
            index: 0,
            requested_slot: 0
        }
    );
    action(
        &mut app,
        InventoryActionV1::PlaceOverflow {
            source: endpoint(zulu, "zero", deferred),
            destination: InventoryOverflowDestinationV1 {
                owner: zulu,
                inventory: "zero".into(),
                slot: None,
            },
        },
    );
    let placed = inventory_stack(app.world(), zulu, "zero", deferred).unwrap();
    assert_eq!(placed.revision, 6);
    assert_eq!(placed.stack, overflow.stack);
    assert_eq!(placed.location, InventoryStackLocationV1::Slot { index: 1 });
    assert_eq!(original.revision, 1);
    assert_eq!(original.owner, zulu); // Results own their original values.
    assert_eq!(original.stack.count, 32);
    let current = state(&mut app);
    assert_eq!(current.revision, 6);
    assert_eq!(current.next_stack_id, 6);
    assert_eq!(
        current
            .consumed
            .iter()
            .find(|total| total.item == "apple")
            .unwrap()
            .count,
        32
    );
    let query = inventory_owner_state(app.world(), alpha).unwrap();
    assert!(
        inventory(&query.owner, "bag")
            .slots
            .iter()
            .all(Option::is_none)
    );
    let query = inventory_owner_state(app.world(), zulu).unwrap();
    let zero = inventory(&query.owner, "zero");
    assert!(zero.overflow.is_empty());
    assert_eq!(zero.slots[1].as_ref().unwrap().id, deferred);
    snapshot(&mut app).validate().unwrap();
}

#[test]
fn overflow_can_move_to_an_existing_slot_on_another_owner() {
    for use_overflow_action in [false, true] {
        let mut app = app();
        let zulu = handle(&app, 3);
        let alpha = handle(&app, 1);
        let deferred = inventory(
            &inventory_owner_state(app.world(), zulu).unwrap().owner,
            "zero",
        )
        .overflow[0]
            .stack
            .id;
        action(
            &mut app,
            if use_overflow_action {
                InventoryActionV1::PlaceOverflow {
                    source: endpoint(zulu, "zero", deferred),
                    destination: InventoryOverflowDestinationV1 {
                        owner: alpha,
                        inventory: "bag".into(),
                        slot: Some(1),
                    },
                }
            } else {
                InventoryActionV1::Move {
                    source: endpoint(zulu, "zero", deferred),
                    destination: slot(alpha, "bag", 1),
                }
            },
        );
        let query = inventory_owner_state(app.world(), alpha).unwrap();
        assert_eq!(
            inventory(&query.owner, "bag").slots[1].as_ref().unwrap().id,
            deferred
        );
        let query = inventory_owner_state(app.world(), zulu).unwrap();
        assert!(inventory(&query.owner, "zero").overflow.is_empty());
        assert_eq!(state(&mut app).next_stack_id, 5);
    }
}

#[test]
fn overflow_cross_owner_append_keeps_exact_stack_through_reload() {
    let mut app = app();
    let zulu = handle(&app, 3);
    let alpha = handle(&app, 1);
    let query = inventory_owner_state(app.world(), zulu).unwrap();
    let deferred = inventory(&query.owner, "zero").overflow[0].stack.id;
    let before = inventory_stack(app.world(), zulu, "zero", deferred).unwrap();
    let target = inventory_owner_state(app.world(), alpha).unwrap();
    let index = u16::try_from(inventory(&target.owner, "zero").slots.len()).unwrap();
    action(
        &mut app,
        InventoryActionV1::PlaceOverflow {
            source: endpoint(zulu, "zero", deferred),
            destination: InventoryOverflowDestinationV1 {
                owner: alpha,
                inventory: "zero".into(),
                slot: None,
            },
        },
    );
    assert!(inventory_stack(app.world(), zulu, "zero", deferred).is_err());
    let placed = inventory_stack(app.world(), alpha, "zero", deferred).unwrap();
    assert_eq!(placed.revision, before.revision + 1);
    assert_eq!(placed.stack, before.stack);
    assert_eq!(placed.location, InventoryStackLocationV1::Slot { index });
    let current = snapshot(&mut app);
    assert_eq!(
        current.native_inventories.as_ref().unwrap().next_stack_id,
        5
    );
    let dir = tempfile::tempdir().unwrap();
    let mut store = SnapshotStore::open(dir.path()).unwrap();
    store.save(&current).unwrap();
    let loaded = store.load().unwrap().unwrap().snapshot;
    let mut reloaded = build_app(&ServerConfig::default(), &loaded).unwrap();
    assert!(snapshot(&mut reloaded) == current);
    assert_eq!(
        inventory_stack(reloaded.world(), alpha, "zero", deferred).unwrap(),
        placed
    );
    assert!(inventory_stack(reloaded.world(), zulu, "zero", deferred).is_err());
}

#[test]
fn stack_query_rejects_wrong_endpoints_stale_handles_and_invalid_keys_without_values() {
    let mut app = app();
    let owner = handle(&app, 3);
    let other = handle(&app, 1);
    let id = bag_stack(&app, owner);
    let missing = StackId::new(state(&mut app).next_stack_id).unwrap();
    let before = snapshot(&mut app);
    for (endpoint, key, stack) in [
        (other, "bag", id),
        (owner, "zero", id),
        (owner, "undeclared", id),
        (owner, "bag", missing),
        (
            OwnerHandle {
                incarnation: owner.incarnation + 1,
                ..owner
            },
            "bag",
            id,
        ),
        (
            OwnerHandle {
                id: DomainId(u64::MAX),
                ..owner
            },
            "bag",
            id,
        ),
    ] {
        assert!(inventory_stack(app.world(), endpoint, key, stack).is_err());
    }
    for key in [
        "".to_owned(),
        "k".repeat(65),
        "private:key/value".into(),
        "é".into(),
    ] {
        let error = inventory_stack(app.world(), owner, &key, id).unwrap_err();
        assert_eq!(format!("{error:#}"), "inventory key invalid");
    }
    assert!(snapshot(&mut app) == before);
}

#[test]
fn overflow_query_tracks_partial_consumption_and_retirement() {
    let mut app = app();
    let owner = handle(&app, 3);
    let query = inventory_owner_state(app.world(), owner).unwrap();
    let id = inventory(&query.owner, "zero").overflow[0].stack.id;
    let before = inventory_stack(app.world(), owner, "zero", id).unwrap();
    action(
        &mut app,
        InventoryActionV1::Consume {
            source: endpoint(owner, "zero", id),
            amount: 1,
        },
    );
    let after = inventory_stack(app.world(), owner, "zero", id).unwrap();
    assert_eq!(after.revision, before.revision + 1);
    assert_eq!(after.stack.id, before.stack.id);
    assert_eq!(after.stack.item, before.stack.item);
    assert_eq!(after.stack.count, before.stack.count - 1);
    assert_eq!(after.location, before.location);
    action(
        &mut app,
        InventoryActionV1::Consume {
            source: endpoint(owner, "zero", id),
            amount: after.stack.count,
        },
    );
    assert!(inventory_stack(app.world(), owner, "zero", id).is_err());
    assert!(
        inventory(
            &inventory_owner_state(app.world(), owner).unwrap().owner,
            "zero"
        )
        .overflow
        .is_empty()
    );
}

#[test]
fn stack_query_validates_an_unrelated_hidden_owner_before_returning_values() {
    use bevy::ecs::entity_disabling::Disabled;
    use icarus_bevy::native_lifecycle_runtime::NativeOwnerIncarnation;
    let mut app = app();
    let owner = handle(&app, 3);
    let other = handle(&app, 1);
    let id = bag_stack(&app, owner);
    let before = snapshot(&mut app);
    let entity = app
        .world()
        .iter_entities()
        .find(|entity| entity.get::<DomainId>() == Some(&other.id))
        .unwrap()
        .id();
    let original = *app.world().get::<NativeOwnerIncarnation>(entity).unwrap();
    app.world_mut()
        .entity_mut(entity)
        .insert(Disabled)
        .remove::<NativeOwnerIncarnation>();
    assert!(inventory_stack(app.world(), owner, "bag", id).is_err());
    app.world_mut().entity_mut(entity).insert(original);
    assert!(capture(app.world_mut()).unwrap() == before);
    assert_eq!(
        inventory_stack(app.world(), owner, "bag", id)
            .unwrap()
            .stack
            .count,
        32
    );
}

#[test]
fn failed_actions_and_self_move_leave_the_entire_snapshot_unchanged() {
    let mut app = app();
    let owner = handle(&app, 3);
    let id = bag_stack(&app, owner);
    let before = snapshot(&mut app);
    let noop = InventoryActionV1::Move {
        source: endpoint(owner, "bag", id),
        destination: slot(owner, "bag", 0),
    };
    assert_eq!(
        action(&mut app, noop.clone()),
        InventoryActionOutcomeV1::NoChange
    );
    assert!(snapshot(&mut app) == before);
    let stale = OwnerHandle {
        incarnation: owner.incarnation + 1,
        ..owner
    };
    for (revision, action) in [
        (0, noop),
        (
            1,
            InventoryActionV1::Consume {
                source: endpoint(stale, "bag", id),
                amount: 1,
            },
        ),
        (
            1,
            InventoryActionV1::Consume {
                source: endpoint(owner, "bag", id),
                amount: 0,
            },
        ),
        (
            1,
            InventoryActionV1::Consume {
                source: endpoint(owner, "bag", id),
                amount: 33,
            },
        ),
        (
            1,
            InventoryActionV1::Split {
                source: endpoint(owner, "bag", id),
                destination: slot(owner, "bag", 1),
                amount: 32,
            },
        ),
        (
            1,
            InventoryActionV1::Move {
                source: endpoint(owner, "bag", id),
                destination: slot(owner, "zero", 0),
            },
        ),
    ] {
        assert!(
            apply_native_inventory_action(
                app.world_mut(),
                &InventoryActionRequestV1 {
                    expected_inventory_revision: revision,
                    action
                }
            )
            .is_err()
        );
        assert!(snapshot(&mut app) == before);
    }
    assert!(inventory_owner_state(app.world(), stale).is_err());
}

#[test]
fn scalar_pose_and_foundation_edits_carry_inventory_unchanged() {
    let mut app = app();
    let zulu = handle(&app, 3);
    let alpha = handle(&app, 1);
    let before = state(&mut app);
    set_enabled(app.world_mut(), alpha, true).unwrap();
    assert!(matches!(
        try_use(app.world_mut(), alpha).unwrap(),
        ScalarUseOutcome::Used(_)
    ));
    let captured = snapshot(&mut app);
    let actor = captured
        .native_world
        .as_ref()
        .unwrap()
        .actors
        .iter()
        .find(|actor| actor.id == zulu.id)
        .unwrap();
    let mut transform = actor.transform.clone();
    transform.translation[0] += 0.125;
    update_lifecycle_actor(app.world_mut(), zulu, transform, actor.variables.clone()).unwrap();
    reassign_lifecycle_foundation(app.world_mut(), zulu, Some(alpha)).unwrap();
    assert_eq!(
        lifecycle_foundation_of(app.world(), zulu).unwrap(),
        Some(alpha)
    );
    assert_eq!(state(&mut app), before);
}

#[test]
fn replacement_and_exact_retry_after_consumption_and_reload_keep_current_state() {
    let mut app = app();
    let old = handle(&app, 3);
    let id = bag_stack(&app, old);
    action(
        &mut app,
        InventoryActionV1::Consume {
            source: endpoint(old, "bag", id),
            amount: 5,
        },
    );
    let request = replacement(&snapshot(&mut app));
    assert!(matches!(
        restore_native_inventory_owners(app.world_mut(), &request).unwrap(),
        NativeRestoreResult::AppliedInMemory(_)
    ));
    let owner = handle(&app, 3);
    assert_eq!(owner.incarnation, old.incarnation + 1);
    assert!(inventory_owner_state(app.world(), old).is_err());
    assert!(inventory_stack(app.world(), old, "bag", id).is_err());
    assert_eq!(bag_stack(&app, owner), id);
    let stack = inventory_stack(app.world(), owner, "bag", id).unwrap();
    assert_eq!(stack.stack.count, 27);
    assert_eq!(stack.owner, owner);
    action(
        &mut app,
        InventoryActionV1::Consume {
            source: endpoint(owner, "bag", id),
            amount: 27,
        },
    );
    let current = snapshot(&mut app);
    assert!(inventory_stack(app.world(), owner, "bag", id).is_err());
    assert!(matches!(
        restore_native_inventory_owners(app.world_mut(), &request).unwrap(),
        NativeRestoreResult::AlreadyCompleted(_)
    ));
    assert!(snapshot(&mut app) == current);
    let dir = tempfile::tempdir().unwrap();
    let mut store = SnapshotStore::open(dir.path()).unwrap();
    store.save(&current).unwrap();
    let loaded = store.load().unwrap().unwrap().snapshot;
    let mut reloaded = build_app(&ServerConfig::default(), &loaded).unwrap();
    assert!(snapshot(&mut reloaded) == current);
    assert!(matches!(
        restore_native_inventory_owners(reloaded.world_mut(), &request).unwrap(),
        NativeRestoreResult::AlreadyCompleted(_)
    ));
    assert!(snapshot(&mut reloaded) == current);
    let mut changed = request.clone();
    changed.owners[0].inventories[0].slots.push(None);
    assert!(restore_native_inventory_owners(reloaded.world_mut(), &changed).is_err());
    assert!(snapshot(&mut reloaded) == current);
}

#[test]
fn final_owner_restore_failure_and_legacy_api_rejection_are_atomic() {
    let mut app = app();
    let before = snapshot(&mut app);
    let request = replacement(&before);
    assert!(restore_native_owners(app.world_mut(), &request.lifecycle).is_err());
    assert!(snapshot(&mut app) == before);
    let mut bad = request;
    bad.lifecycle
        .recorders
        .last_mut()
        .unwrap()
        .expected_incarnation += 1;
    assert!(restore_native_inventory_owners(app.world_mut(), &bad).is_err());
    assert!(snapshot(&mut app) == before);
}

#[test]
fn optional_relationships_and_required_immutable_sidecars_are_enforced() {
    let mut without = fixture::imported_variant(1);
    without.native_relationships = None;
    let mut independent = build_app(&ServerConfig::default(), &without).unwrap();
    assert!(snapshot(&mut independent) == without);
    let owner = handle(&independent, 3);
    assert!(inventory_owner_state(independent.world(), owner).is_ok());
    for missing in 0..3 {
        let mut app = app();
        let owner = handle(&app, 3);
        match missing {
            0 => {
                app.world_mut()
                    .remove_resource::<NativeInventoryContractsState>();
            }
            1 => {
                app.world_mut()
                    .remove_resource::<C001InventoryOwnerImportState>();
            }
            _ => {
                app.world_mut().remove_resource::<PreservedProspectState>();
            }
        }
        assert!(inventory_owner_state(app.world(), owner).is_err());
        assert!(capture(app.world_mut()).is_err());
    }
}

#[test]
fn snapshot_worker_captures_mutations_and_reload_does_not_reset_items() {
    let mut app = app();
    let owner = handle(&app, 3);
    let id = bag_stack(&app, owner);
    let initial = snapshot(&mut app);
    action(
        &mut app,
        InventoryActionV1::Consume {
            source: endpoint(owner, "bag", id),
            amount: 9,
        },
    );
    let current = snapshot(&mut app);
    let dir = tempfile::tempdir().unwrap();
    let mut store = SnapshotStore::open(dir.path()).unwrap();
    store.save(&initial).unwrap();
    let mut worker = SaveWorker::spawn(store).unwrap();
    worker.submit(current.clone()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    assert!(worker.finish_before(deadline).unwrap().is_some());
    drop(worker);
    let store = loop {
        match SnapshotStore::open(dir.path()) {
            Ok(store) => break store,
            Err(error)
                if Instant::now() < deadline && format!("{error:#}").contains("already locked") =>
            {
                std::thread::sleep(Duration::from_millis(2));
            }
            Err(error) => panic!("worker store did not close: {error:#}"),
        }
    };
    let loaded = store.load().unwrap().unwrap().snapshot;
    let mut reloaded = build_app(&ServerConfig::default(), &loaded).unwrap();
    assert!(snapshot(&mut reloaded) == current);
    let query = inventory_owner_state(reloaded.world(), owner).unwrap();
    assert_eq!(
        inventory(&query.owner, "bag").slots[0]
            .as_ref()
            .unwrap()
            .count,
        23
    );
}

#[test]
fn exhausted_inventory_counters_fail_without_changing_world_or_allocating_ids() {
    for allocator_exhausted in [false, true] {
        let mut initial = fixture::imported();
        let inventory = initial.native_inventories.as_mut().unwrap();
        if allocator_exhausted {
            inventory.next_stack_id = u64::MAX;
        } else {
            inventory.revision = u64::MAX;
        }
        let mut app = build_app(&ServerConfig::default(), &initial).unwrap();
        let owner = handle(&app, 3);
        let id = bag_stack(&app, owner);
        let before = snapshot(&mut app);
        let revision = before.native_inventories.as_ref().unwrap().revision;
        let failed = if allocator_exhausted {
            InventoryActionV1::Split {
                source: endpoint(owner, "bag", id),
                destination: slot(owner, "bag", 1),
                amount: 1,
            }
        } else {
            InventoryActionV1::Consume {
                source: endpoint(owner, "bag", id),
                amount: 1,
            }
        };
        assert!(
            apply_native_inventory_action(
                app.world_mut(),
                &InventoryActionRequestV1 {
                    expected_inventory_revision: revision,
                    action: failed,
                }
            )
            .is_err()
        );
        assert!(snapshot(&mut app) == before);
        assert_eq!(
            apply_native_inventory_action(
                app.world_mut(),
                &InventoryActionRequestV1 {
                    expected_inventory_revision: revision,
                    action: InventoryActionV1::Move {
                        source: endpoint(owner, "bag", id),
                        destination: slot(owner, "bag", 0),
                    },
                }
            )
            .unwrap(),
            InventoryActionOutcomeV1::NoChange
        );
        assert!(snapshot(&mut app) == before);
    }
}

#[test]
fn storage_rejects_invalid_revision_allocator_and_consumption_headers() {
    for damaged in 0..3 {
        let mut initial = fixture::imported();
        let state = initial.native_inventories.as_mut().unwrap();
        match damaged {
            0 => state.revision = 0,
            1 => state.next_stack_id -= 1,
            _ => state.consumed[0].count += 1,
        }
        let dir = tempfile::tempdir().unwrap();
        let mut store = SnapshotStore::open(dir.path()).unwrap();
        assert!(store.save(&initial).is_err());
        assert!(store.load().unwrap().is_none());
    }
}

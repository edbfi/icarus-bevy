//! Complete request identity/bounds tests; no runtime or storage claims.
#[path = "common/c001_inventory_owners.rs"]
mod authored;
#[path = "common/c001_scalar_owners.rs"]
mod scalar;
#[path = "common/c001_inventory_source.rs"]
mod source;
use icarus_bevy::{
    native_inventory::{InventoryStateV1, MAX_INVENTORY_GRAPH_BYTES},
    native_inventory_restore::*,
    native_lifecycle::RestoreRecorder,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::mem::{size_of, size_of_val};

fn request() -> InventoryRestoreBatchV1 {
    let f = authored::fixture();
    let p = f.prepared_parts();
    InventoryRestoreBatchV1 {
        format: 1,
        scalar_contract_id: f.scalar_contracts.identity().into(),
        inventory_contract_id: f.inventory_contracts.identity().into(),
        expected_inventory_revision: 0,
        lifecycle: f.lifecycle_batch.clone(),
        owners: f
            .lifecycle_batch
            .recorders
            .iter()
            .map(|r| {
                let outcome = p
                    .metadata()
                    .initial_completion
                    .outcomes
                    .iter()
                    .find(|o| o.guid == r.guid)
                    .unwrap();
                InventoryOwnerRestoreV1 {
                    target_placement: r.target_placement.clone(),
                    inventories: p
                        .inventories()
                        .owners
                        .iter()
                        .find(|o| o.owner == outcome.owner)
                        .unwrap()
                        .inventories
                        .clone(),
                }
            })
            .collect(),
    }
}
#[test]
fn canonical_composite_identity_hashes_exact_prefix_and_all_fields() {
    let a = request();
    let expected = a.request_digest().unwrap();
    let mut hash = Sha256::new();
    hash.update(b"icarus/native_inventory_owner_restore/v1\0");
    hash.update(serde_json::to_vec(&a.canonical().unwrap()).unwrap());
    let hex: String = hash.finalize().iter().map(|v| format!("{v:02x}")).collect();
    assert_eq!(expected, hex);
    let f = authored::fixture();
    assert_eq!(
        expected,
        f.prepared_parts()
            .metadata()
            .initial_completion
            .request_digest
    );
    let mut b = a.clone();
    b.owners.reverse();
    b.lifecycle.recorders.reverse();
    for o in &mut b.owners {
        o.inventories.reverse();
    }
    for c in &mut b.lifecycle.recorders {
        c.variables.reverse();
    }
    assert_eq!(expected, b.request_digest().unwrap());
    assert_eq!(
        expected,
        InventoryRestoreBatchV1::parse(&serde_json::to_string(&b).unwrap())
            .unwrap()
            .request_digest()
            .unwrap()
    );
    // TOML has no null array element. Check its representable request subset
    // separately; the complete slot layout above round-trips through JSON.
    let mut toml_request = b.clone();
    for inventory in toml_request
        .owners
        .iter_mut()
        .flat_map(|o| &mut o.inventories)
    {
        inventory.slots.retain(Option::is_some);
    }
    assert_eq!(
        toml_request.request_digest().unwrap(),
        InventoryRestoreBatchV1::parse(&toml::to_string(&toml_request).unwrap())
            .unwrap()
            .request_digest()
            .unwrap()
    );
    for change in 0..8 {
        let mut b = a.clone();
        match change {
            0 => b.expected_inventory_revision = 1,
            1 => b.scalar_contract_id = "a".repeat(64),
            2 => b.inventory_contract_id = "b".repeat(64),
            3 => b.lifecycle.recorders[0].expected_incarnation += 1,
            4 => b.lifecycle.recorders[0].owner_name.push(65),
            5 => {
                b.owners
                    .iter_mut()
                    .flat_map(|o| &mut o.inventories)
                    .find(|i| !i.overflow.is_empty())
                    .unwrap()
                    .overflow[0]
                    .requested_slot = 255
            }
            6 => {
                b.owners
                    .iter_mut()
                    .flat_map(|o| &mut o.inventories)
                    .find(|i| i.slots.iter().any(Option::is_some))
                    .unwrap()
                    .slots
                    .iter_mut()
                    .flatten()
                    .next()
                    .unwrap()
                    .count += 1
            }
            _ => b.owners[0].inventories[0].slots.push(None),
        }
        assert_ne!(expected, b.request_digest().unwrap(), "case {change}");
    }
}
#[test]
fn inventory_only_completed_request_alteration_conflicts_without_pose_change() {
    let a = request();
    let mut b = a.clone();
    let inv = b
        .owners
        .iter_mut()
        .flat_map(|o| &mut o.inventories)
        .find(|v| !v.overflow.is_empty())
        .unwrap();
    inv.overflow[0].stack.count -= 1;
    assert_eq!(a.lifecycle, b.lifecycle);
    assert_ne!(a.request_digest().unwrap(), b.request_digest().unwrap());
}
#[test]
fn coverage_duplicates_unknown_fields_and_object_aliases_fail() {
    let a = request();
    for change in 0..6 {
        let mut b = a.clone();
        match change {
            0 => {
                b.owners.pop();
            }
            1 => b.owners.push(b.owners[0].clone()),
            2 => b.owners[0].target_placement = "unmapped".into(),
            3 => {
                let duplicate = b.owners[0].inventories[0].clone();
                b.owners[0].inventories.push(duplicate);
            }
            4 => b.lifecycle.recorders.push(b.lifecycle.recorders[0].clone()),
            _ => {
                let duplicate = b.lifecycle.recorders[0].variables[0].clone();
                b.lifecycle.recorders[0].variables.push(duplicate);
            }
        }
        assert!(b.request_digest().is_err(), "case {change}");
    }
    let mut j = serde_json::to_value(&a).unwrap();
    j["private_sentinel"] = json!([]);
    let err = InventoryRestoreBatchV1::parse(&j.to_string()).unwrap_err();
    assert!(!format!("{err:?}").contains("private_sentinel"));
    let mut j = serde_json::to_value(&a).unwrap();
    j["owners"][0] = Value::Array(vec![json!("alpha"), json!([])]);
    assert!(InventoryRestoreBatchV1::parse(&j.to_string()).is_err());
    let text = serde_json::to_string(&a).unwrap();
    let duplicate = text.replacen("\"format\":1", "\"format\":1,\"format\":1", 1);
    assert!(InventoryRestoreBatchV1::parse(&duplicate).is_err());
    assert!(InventoryRestoreBatchV1::parse(&" ".repeat(MAX_INVENTORY_RESTORE_BYTES + 1)).is_err());
}
#[test]
fn nested_decode_limits_and_spare_capacity_reject_before_canonical_clone() {
    let mut a = request();
    a.owners[0].inventories = Vec::<InventoryStateV1>::with_capacity(21);
    assert!(a.request_digest().is_err());
    let a = request();
    let mut j = serde_json::to_value(&a).unwrap();
    j["owners"][0]["inventories"][0]["slots"] = json!(vec![Value::Null; 257]);
    assert!(InventoryRestoreBatchV1::parse(&j.to_string()).is_err());
    let mut j = serde_json::to_value(&a).unwrap();
    j["owners"][0]["target_placement"] = json!("private_sentinel".repeat(100));
    assert!(
        !InventoryRestoreBatchV1::parse(&j.to_string())
            .unwrap_err()
            .to_string()
            .contains("private_sentinel")
    );
    assert!(!format!("{a:?}").contains(&a.inventory_contract_id));
}

#[test]
fn near_budget_empty_slot_request_roundtrips_and_over_budget_quota_resets() {
    let template = request();
    let slot_bytes = size_of_val(&template.owners[0].inventories[0].slots[0]);
    // All strings have fixed lengths; no item or variable allocations remain.
    // The independent total includes both decoder vectors for each command.
    let bytes = |owners: usize| {
        let growth = owners.next_power_of_two().max(4);
        size_of::<InventoryRestoreBatchV1>()
            + 3 * 64
            + growth * (size_of::<InventoryOwnerRestoreV1>() + size_of::<RestoreRecorder>())
            + owners
                * (
                    size_of::<RestoreRecorder>() // Command -> RestoreRecorder conversion
                + 2 * 6 // both placement strings: p00000
                + 4 * size_of::<u16>() // one owner-name unit grows to four
                + 4 * size_of::<InventoryStateV1>() // two inventories grow to four
                + size_of::<Vec<InventoryStateV1>>() // conservative collection root
                + 2 * (1 + 256 * slot_bytes)
                    // keys and allocated empty slots
                )
    };
    let count = (1..16_384)
        .take_while(|n| bytes(*n) <= MAX_INVENTORY_GRAPH_BYTES)
        .last()
        .expect("the concrete shape fits at least one owner");
    assert!(count > 1000 && count < 16_383);
    assert!(bytes(count) <= MAX_INVENTORY_GRAPH_BYTES);
    assert!(bytes(count + 1) > MAX_INVENTORY_GRAPH_BYTES);
    let make_request = |count: usize| {
        let mut r = template.clone();
        r.lifecycle.recorders.clear();
        r.owners.clear();
        let mut inventory = template.owners[0].inventories[0].clone();
        inventory.slots = vec![None; 256];
        inventory.overflow = Vec::new();
        let mut first = inventory.clone();
        first.key = "a".into();
        inventory.key = "b".into();
        r.lifecycle.recorders = (0..count)
            .map(|i| {
                let mut c = template.lifecycle.recorders[0].clone();
                c.guid = i32::try_from(i + 1).unwrap();
                c.target_placement = format!("p{i:05}").into_boxed_str().into_string();
                c.owner_name = vec![65];
                c.variables = Vec::new();
                c
            })
            .collect();
        r.owners = r
            .lifecycle
            .recorders
            .iter()
            .map(|c| InventoryOwnerRestoreV1 {
                target_placement: c.target_placement.clone(),
                inventories: vec![first.clone(), inventory.clone()],
            })
            .collect();
        r
    };

    let accepted = make_request(count);
    let digest = accepted.request_digest().unwrap();
    let json = serde_json::to_string(&accepted).unwrap();
    assert!(json.len() < MAX_INVENTORY_RESTORE_BYTES);
    assert_eq!(
        InventoryRestoreBatchV1::parse(&json)
            .unwrap()
            .request_digest()
            .unwrap(),
        digest
    );
    drop(accepted);
    drop(json);

    let rejected = make_request(count + 1);
    let error = rejected.request_digest().unwrap_err();
    assert_eq!(error.to_string(), "inventory graph allocation limit");
    drop(rejected);
    // The decoder charges heap growth/conversions; the borrowed calculator
    // also includes the enclosing object and each collection's inline Vec.
    // Cross the streaming ceiling independently of that conservative charge.
    let decode_count = (count + 1..16_384)
        .find(|n| {
            bytes(*n)
                - size_of::<InventoryRestoreBatchV1>()
                - n * size_of::<Vec<InventoryStateV1>>()
                > MAX_INVENTORY_GRAPH_BYTES
        })
        .unwrap();
    let bad_json = serde_json::to_string(&make_request(decode_count)).unwrap();
    assert!(serde_json::from_str::<InventoryRestoreBatchV1>(&bad_json).is_err());
    drop(bad_json);
    let good_json = serde_json::to_string(&template).unwrap();
    assert!(InventoryRestoreBatchV1::parse(&good_json).is_ok());
}

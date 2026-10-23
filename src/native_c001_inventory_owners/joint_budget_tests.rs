//! Real DTO-reader quota coverage, not full world/provenance qualification:
//! the expanded owners below deliberately have no matching source bindings.
use super::*;
use crate::{
    inventory_owner_fixture::fixture,
    native_inventory::{
        InventoryOwnerClassV1, InventoryStateV1, ItemDefinitionV1, ItemTotalV1,
        MAX_INVENTORY_GRAPH_BYTES, MAX_INVENTORY_STATE_BYTES, OwnerInventoriesV1, SlotRuleV1,
        StackV1, inventory_snapshot_allocation_bytes,
    },
};

fn decoded_vector<T>(len: usize) -> usize {
    if len == 0 {
        0
    } else {
        len.next_power_of_two().max(4) * size_of::<T>()
    }
}

// Independent enumeration of the fixture contract's actual reader charges.
fn contract_charge(c: &InventoryContractsSnapshotV1) -> usize {
    let d = &c.definition;
    let mut bytes = c.contract_id.len()
        + d.native_catalog_id.len()
        + d.native_map.len()
        + decoded_vector::<ItemDefinitionV1>(d.items.len())
        + d.items.iter().map(|i| i.key.len()).sum::<usize>()
        + decoded_vector::<InventoryOwnerClassV1>(d.classes.len());
    for class in &d.classes {
        bytes += class.native_class.len()
            + decoded_vector::<crate::native_inventory::InventoryDefinitionV1>(
                class.inventories.len(),
            );
        for inv in &class.inventories {
            bytes += inv.key.len() + decoded_vector::<SlotRuleV1>(inv.slot_rules.len());
            for rule in &inv.slot_rules {
                bytes += decoded_vector::<String>(rule.allowed_items.len())
                    + rule.allowed_items.iter().map(String::len).sum::<usize>();
            }
        }
    }
    bytes
}

// Empty slots consume real vector storage without approaching the stack quota
// or encoded-byte cap. Exact source capacity is smaller than reload capacity.
fn state_at_charge(mut state: InventoryStateSnapshotV1, target: usize) -> InventoryStateSnapshotV1 {
    state.owners = (1..=1024)
        .map(|id| OwnerInventoriesV1 {
            owner: OwnerHandle {
                id: DomainId(id),
                incarnation: 1,
            },
            inventories: (b'a'..=b'h')
                .map(|key| InventoryStateV1 {
                    key: char::from(key).to_string(),
                    slots: Vec::new(),
                    overflow: Vec::new(),
                })
                .collect(),
        })
        .collect();
    let base = state.contract_id.len()
        + decoded_vector::<ItemTotalV1>(state.consumed.len())
        + state.consumed.iter().map(|t| t.item.len()).sum::<usize>()
        + decoded_vector::<OwnerInventoriesV1>(state.owners.len())
        + 1024 * (decoded_vector::<InventoryStateV1>(8) + 8);
    let mut remaining = target.checked_sub(base).unwrap();
    for inv in state.owners.iter_mut().flat_map(|o| &mut o.inventories) {
        let capacity = [256, 128, 64, 32, 16, 8, 4]
            .into_iter()
            .find(|n| n * size_of::<Option<StackV1>>() <= remaining)
            .unwrap_or(0);
        if capacity != 0 {
            // 129 -> 256 is the main pressure; smaller tails tune the boundary.
            let len = if capacity == 4 { 1 } else { capacity / 2 + 1 };
            inv.slots = vec![None; len];
            remaining -= capacity * size_of::<Option<StackV1>>();
        }
    }
    assert!(remaining < 4 * size_of::<Option<StackV1>>());
    // Tune the sub-vector remainder with valid bounded inventory keys, never
    // digests. Reserve exactly so caller spare capacity cannot mask a mismatch.
    for inv in state.owners.iter_mut().flat_map(|o| &mut o.inventories) {
        let extra = remaining.min(63);
        let mut key = String::with_capacity(1 + extra);
        key.push_str(&inv.key);
        key.extend(std::iter::repeat_n('x', extra));
        inv.key = key;
        remaining -= extra;
        if remaining == 0 {
            break;
        }
    }
    assert_eq!(remaining, 0);
    assert_eq!(state.owners.len(), 1024);
    assert_eq!(state.owners[0].inventories[0].slots.len(), 129);
    assert_eq!(state.owners[0].inventories[0].slots.capacity(), 129);
    state
}

fn decode_joint(
    bytes: [&[u8]; 3],
    metadata_first: bool,
    remaining: Option<usize>,
) -> Result<(
    InventoryContractsSnapshotV1,
    InventoryStateSnapshotV1,
    C001InventoryOwnerImportMetadataV1,
)> {
    let _scope = InventoryDecodeScope::enter();
    let (mut contracts, mut state, mut metadata) = (None, None, None);
    for part in if metadata_first { [2, 0, 1] } else { [0, 1, 2] } {
        match part {
            0 => contracts = Some(serde_json::from_slice(bytes[0])?),
            1 => state = Some(serde_json::from_slice(bytes[1])?),
            2 => metadata = Some(serde_json::from_slice(bytes[2])?),
            _ => unreachable!(),
        }
    }
    if let Some(remaining) = remaining {
        // Probe the exact unused allowance after REAL reader charges. No quota
        // is seeded before decoding, and all three decoded graphs remain live.
        InventoryDecodeScope::charge_bytes(remaining).unwrap();
        assert!(InventoryDecodeScope::charge_bytes(1).is_err());
    }
    Ok((contracts.unwrap(), state.unwrap(), metadata.unwrap()))
}

#[test]
fn real_readers_share_combined_quota_in_both_orders_and_reset_after_refusal() {
    // Keep only one expanded graph alive at a time, plus two sub-16-MiB JSON
    // encodings. No serde_json::Value copies or second expanded-state clone.
    let f = fixture();
    let parts = f.prepared_parts();
    let contracts = f.inventory_contracts.snapshot();
    let contracts_json = serde_json::to_vec(contracts.as_ref()).unwrap();
    let metadata_json = serde_json::to_vec(parts.metadata()).unwrap();
    let metadata: C001InventoryOwnerImportMetadataV1 =
        serde_json::from_slice(&metadata_json).unwrap();
    // Metadata accounting is independently enumerated by allocation_tests.
    // Normalize its capacities through the real reader before using that bound.
    let metadata_bytes = metadata.allocation_bytes().unwrap();
    let other_charges = contract_charge(&contracts) + metadata_bytes
        - size_of::<C001InventoryOwnerImportMetadataV1>();
    let root_bytes = size_of::<InventoryContractsSnapshotV1>()
        + size_of::<InventoryStateSnapshotV1>()
        + size_of::<C001InventoryOwnerImportMetadataV1>();
    let target = MAX_INVENTORY_GRAPH_BYTES - other_charges - root_bytes;
    let state = state_at_charge(parts.inventories().clone(), target);
    assert_eq!(
        inventory_snapshot_allocation_bytes(&contracts, &state).unwrap() + metadata_bytes,
        MAX_INVENTORY_GRAPH_BYTES
    );
    let logical_slots: usize = state
        .owners
        .iter()
        .flat_map(|o| &o.inventories)
        .map(|i| i.slots.len())
        .sum();
    let accepted_json = serde_json::to_vec(&state).unwrap();
    drop(state);
    assert!(accepted_json.len() < MAX_INVENTORY_STATE_BYTES);

    let state = state_at_charge(parts.inventories().clone(), target + root_bytes + 1);
    assert!(
        inventory_snapshot_allocation_bytes(&contracts, &state).unwrap() + metadata_bytes
            > MAX_INVENTORY_GRAPH_BYTES
    );
    let rejected_json = serde_json::to_vec(&state).unwrap();
    drop(state);
    assert!(rejected_json.len() < MAX_INVENTORY_STATE_BYTES);
    // Every member remains independently admissible: only joint admission fails.
    drop(serde_json::from_slice::<InventoryStateSnapshotV1>(&rejected_json).unwrap());
    drop(serde_json::from_slice::<InventoryContractsSnapshotV1>(&contracts_json).unwrap());
    drop(serde_json::from_slice::<C001InventoryOwnerImportMetadataV1>(&metadata_json).unwrap());

    for metadata_first in [false, true] {
        assert!(
            decode_joint(
                [&contracts_json, &rejected_json, &metadata_json],
                metadata_first,
                None,
            )
            .is_err()
        );
        // Same thread, immediately after failure: also proves scope cleanup.
        let (c, s, m) = decode_joint(
            [&contracts_json, &accepted_json, &metadata_json],
            metadata_first,
            Some(root_bytes),
        )
        .unwrap();
        assert_eq!(s.owners.len(), 1024);
        assert!(s.owners.iter().all(|o| o.inventories.len() == 8));
        assert_eq!(s.owners[0].inventories[0].slots.len(), 129);
        assert_eq!(s.owners[0].inventories[0].slots.capacity(), 256);
        assert_eq!(
            s.owners
                .iter()
                .flat_map(|o| &o.inventories)
                .map(|i| i.slots.len())
                .sum::<usize>(),
            logical_slots
        );
        assert_eq!(
            inventory_snapshot_allocation_bytes(&c, &s).unwrap() + m.allocation_bytes().unwrap(),
            MAX_INVENTORY_GRAPH_BYTES
        );
    }
}

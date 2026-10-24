use super::*;
use crate::native_inventory::MAX_INVENTORY_GRAPH_BYTES;

#[test]
fn metadata_accounting_includes_names_conversions_object_and_initial_totals() {
    use crate::inventory_owner_fixture::fixture;
    use std::mem::size_of;
    fn vector<T>(v: &Vec<T>, limit: usize) -> usize {
        let decoded = if v.is_empty() {
            0
        } else {
            v.len().next_power_of_two().max(4).min(limit)
        };
        v.capacity().max(decoded) * size_of::<T>()
    }
    let f = fixture();
    let p = f.prepared_parts();
    let baseline = p.metadata().clone();
    let m = &baseline;
    let b = &m.bindings;
    let l = &b.inventory_layout;
    // Flatten this authored receipt independently of production Bounds. Every
    // source name is decoded through both Vec<Unit> growth and u16 conversion.
    let mut names = vec![&m.source_prospect_id, &m.source_lobby_name, &b.source_map];
    for a in [&b.scalar_layout.enabled, &b.scalar_layout.remaining_uses] {
        names.extend([
            &a.array_name,
            &a.element_tag_name,
            &a.record_type,
            &a.value_field,
        ]);
    }
    for a in [
        &l.inventories,
        &l.slots,
        &l.dynamic,
        &l.additional_stats,
        &l.alterations,
        &l.living_item_slots,
    ] {
        names.extend([&a.field_name, &a.element_tag_name, &a.record_type]);
    }
    names.extend([
        &l.inventory_key_field,
        &l.inventory_data.field_name,
        &l.inventory_data.record_type,
    ]);
    let mut strings = vec![
        &m.bindings_id,
        &b.expected_payload_sha256,
        &b.native_catalog_id,
        &b.native_lifecycle_id,
        &b.native_scalar_contract_id,
        &b.native_inventory_contract_id,
        &b.native_map,
        &m.initial_completion.request_digest,
        &m.inventory_initial.initial_state_digest,
    ];
    let mut vectors = vector(&b.items, 1024)
        + vector(&b.owners, 16_384)
        + vector(&m.resolved, 16_384)
        + vector(&m.initial_completion.outcomes, 16_384)
        + vector(&m.inventory_initial.initial_totals.items, 1024);
    for item in &b.items {
        names.push(&item.source_row);
        strings.push(&item.item);
    }
    for owner in &b.owners {
        let s = &owner.scalar;
        let o = &s.owner;
        names.extend([
            &o.source_actor_class,
            &o.source_actor_path,
            &o.source_object_name,
            &o.native_owner_name,
            &s.enabled_source_name,
            &s.remaining_uses_source_name,
        ]);
        strings.extend([&o.native_placement, &o.native_class]);
        vectors += vector(&owner.inventories, 8);
        for inv in &owner.inventories {
            names.push(&inv.source_key);
            strings.push(&inv.inventory);
        }
    }
    for total in &m.inventory_initial.initial_totals.items {
        strings.push(&total.item);
    }
    let expected = size_of::<C001InventoryOwnerImportMetadataV1>()
        + vectors
        + strings.iter().map(|s| s.capacity()).sum::<usize>()
        + names
            .iter()
            .map(|n| vector(n, 1024) + n.len() * size_of::<u16>())
            .sum::<usize>();
    assert!(expected < MAX_INVENTORY_GRAPH_BYTES);
    assert_eq!(m.allocation_bytes().unwrap(), expected);
    let mut joint = MAX_INVENTORY_GRAPH_BYTES - expected;
    add_bytes(&mut joint, m.allocation_bytes().unwrap()).unwrap();
    assert_eq!(joint, MAX_INVENTORY_GRAPH_BYTES);
    assert!(add_bytes(&mut joint, 1).is_err());
}

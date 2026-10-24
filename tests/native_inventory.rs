//! Authored domain tests only. The lead registers and validates the module.
use icarus_bevy::{
    native_inventory::*, native_lifecycle::OwnerHandle, native_world::CompiledNativeCatalog,
    simulation::DomainId,
};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};

fn catalog_with_owners(n: usize) -> CompiledNativeCatalog {
    let class = |key: &str| json!({"key":key,"kind":"spatial_object_v1","local_bounds":{"min":[-0.1,-0.1,-0.1],"max":[0.1,0.1,0.1]},"variables":[]});
    let placements:Vec<_>=(0..n).map(|i|json!({
        "key":format!("owner_{i:05}"),"class":if i%2==0 {"alpha"}else{"beta"},"tile":"tile",
        "transform":{"translation":[0.0,0.0,0.0],"rotation":[0.0,0.0,0.0,1.0],"scale":[1.0,1.0,1.0]},"initial_values":[]
    })).collect();
    CompiledNativeCatalog::parse(&json!({"native_format":1,"frame":"right_handed_z_up_meters",
        "classes":[class("alpha"),class("beta")],"maps":[{"key":"map","tiles":[{"key":"tile","bounds":{"min":[-10.0,-10.0,-10.0],"max":[10.0,10.0,10.0]}}],"placements":placements}]
    }).to_string()).unwrap()
}
fn recipe(key: &str, initial_slots: u16, force: bool) -> InventoryDefinitionV1 {
    InventoryDefinitionV1 {
        key: key.into(),
        initial_slots,
        force_add_when_zero: force,
        overflow_limit: 2,
        slot_rules: (0..4)
            .map(|_| SlotRuleV1 {
                allowed_items: vec!["apple".into(), "stone".into()],
            })
            .collect(),
    }
}
fn definition(c: &CompiledNativeCatalog) -> InventoryContractsV1 {
    InventoryContractsV1 {
        format: 1,
        native_catalog_id: c.identity().into(),
        native_map: "map".into(),
        items: vec![
            ItemDefinitionV1 {
                key: "stone".into(),
                stack_limit: 10,
            },
            ItemDefinitionV1 {
                key: "apple".into(),
                stack_limit: 10,
            },
        ],
        classes: ["beta", "alpha"]
            .into_iter()
            .map(|name| InventoryOwnerClassV1 {
                native_class: name.into(),
                inventories: vec![recipe("zero", 0, true), recipe("bag", 2, false)],
            })
            .collect(),
    }
}
fn contracts() -> CompiledInventoryContractsV1 {
    let c = catalog_with_owners(2);
    CompiledInventoryContractsV1::compile(definition(&c), &c).unwrap()
}
fn handle(id: u64) -> OwnerHandle {
    OwnerHandle {
        id: DomainId(id),
        incarnation: 1,
    }
}
fn live() -> InventoryLiveOwnersV1 {
    InventoryLiveOwnersV1 {
        owners: vec![handle(2), handle(1)],
    }
}
fn item(owner: u64, inventory: &str, slot: u16, key: &str, count: u32) -> InitialInventoryItemV1 {
    InitialInventoryItemV1 {
        owner: DomainId(owner),
        inventory: inventory.into(),
        requested_slot: slot,
        item: key.into(),
        count,
    }
}
fn prepare(
    c: &CompiledInventoryContractsV1,
    items: Vec<InitialInventoryItemV1>,
) -> PreparedInitialInventoryV1 {
    prepare_initial_inventory(c, &live(), &InitialInventoryItemsV1 { items }).unwrap()
}
fn initial(c: &CompiledInventoryContractsV1) -> PreparedInitialInventoryV1 {
    prepare(
        c,
        vec![
            item(1, "bag", 0, "apple", 8),
            item(2, "bag", 0, "apple", 2),
            item(1, "bag", 255, "stone", 3),
        ],
    )
}
fn inv<'a>(s: &'a InventoryStateSnapshotV1, id: u64, key: &str) -> &'a InventoryStateV1 {
    s.owners
        .iter()
        .find(|o| o.owner.id == DomainId(id))
        .unwrap()
        .inventories
        .iter()
        .find(|i| i.key == key)
        .unwrap()
}
fn inv_mut<'a>(
    s: &'a mut InventoryStateSnapshotV1,
    id: u64,
    key: &str,
) -> &'a mut InventoryStateV1 {
    s.owners
        .iter_mut()
        .find(|o| o.owner.id == DomainId(id))
        .unwrap()
        .inventories
        .iter_mut()
        .find(|i| i.key == key)
        .unwrap()
}
fn source(owner: u64, inventory: &str, id: u64) -> InventoryStackEndpointV1 {
    InventoryStackEndpointV1 {
        owner: handle(owner),
        inventory: inventory.into(),
        stack_id: StackId::new(id).unwrap(),
    }
}
fn slot(owner: u64, inventory: &str, slot: u16) -> InventorySlotEndpointV1 {
    InventorySlotEndpointV1 {
        owner: handle(owner),
        inventory: inventory.into(),
        slot,
    }
}
fn overflow_destination(
    owner: u64,
    inventory: &str,
    slot: Option<u16>,
) -> InventoryOverflowDestinationV1 {
    InventoryOverflowDestinationV1 {
        owner: handle(owner),
        inventory: inventory.into(),
        slot,
    }
}
fn apply(
    c: &CompiledInventoryContractsV1,
    s: &InventoryStateSnapshotV1,
    t: &InventoryInitialTotalsV1,
    a: InventoryActionV1,
) -> InventoryStateSnapshotV1 {
    let before = s.clone();
    let result = prepare_inventory_action(
        c,
        s,
        t,
        &live(),
        &InventoryActionRequestV1 {
            expected_inventory_revision: s.revision,
            action: a,
        },
    )
    .unwrap();
    assert_eq!(s, &before);
    assert_eq!(result.outcome(), InventoryActionOutcomeV1::Applied);
    validate_inventory_state(c, result.state(), t, &live()).unwrap();
    result.into_state()
}
fn assert_action_refused(
    c: &CompiledInventoryContractsV1,
    s: &InventoryStateSnapshotV1,
    t: &InventoryInitialTotalsV1,
    request: InventoryActionRequestV1,
) {
    let before = s.clone();
    assert!(prepare_inventory_action(c, s, t, &live(), &request).is_err());
    assert_eq!(s, &before);
}
fn noop() -> InventoryActionV1 {
    InventoryActionV1::Move {
        source: source(1, "bag", 1),
        destination: slot(1, "bag", 0),
    }
}

#[test]
fn canonical_identity_sorts_declarations_and_eligibility_but_preserves_slot_order() {
    let c = catalog_with_owners(2);
    let d = definition(&c);
    let a = CompiledInventoryContractsV1::compile(d.clone(), &c).unwrap();
    let mut reordered = d;
    reordered.items.reverse();
    reordered.classes.reverse();
    for cl in &mut reordered.classes {
        cl.inventories.reverse();
        for i in &mut cl.inventories {
            for r in &mut i.slot_rules {
                r.allowed_items.reverse();
            }
        }
    }
    let b = CompiledInventoryContractsV1::compile(reordered, &c).unwrap();
    assert_eq!(a.identity(), b.identity());
    let parsed =
        CompiledInventoryContractsV1::parse(&serde_json::to_string(b.definition()).unwrap(), &c)
            .unwrap();
    assert_eq!(a.identity(), parsed.identity());
    assert_eq!(
        CompiledInventoryContractsV1::from_snapshot(&a.snapshot(), &c)
            .unwrap()
            .identity(),
        a.identity()
    );
    let mut d = a.definition().clone();
    d.classes[0].inventories[0].slot_rules[0].allowed_items = vec!["apple".into()];
    let a = CompiledInventoryContractsV1::compile(d.clone(), &c).unwrap();
    d.classes[0].inventories[0].slot_rules.swap(0, 1);
    assert_ne!(
        a.identity(),
        CompiledInventoryContractsV1::compile(d, &c)
            .unwrap()
            .identity()
    );
}

#[test]
fn exact_coverage_dangling_duplicate_and_invalid_contracts_fail() {
    let c = catalog_with_owners(2);
    let d = definition(&c);
    for change in 0..14 {
        let mut bad = d.clone();
        match change {
            0 => {
                bad.classes.pop();
            }
            1 => bad.classes.push(bad.classes[0].clone()),
            2 => bad.classes[0].native_class = "unused".into(),
            3 => bad.items.push(bad.items[0].clone()),
            4 => {
                let duplicate = bad.classes[0].inventories[0].clone();
                bad.classes[0].inventories.push(duplicate);
            }
            5 => bad.classes[0].inventories[0].slot_rules[0]
                .allowed_items
                .push("missing".into()),
            6 => bad.classes[0].inventories[0].slot_rules[0]
                .allowed_items
                .push("apple".into()),
            7 => bad.items[0].stack_limit = 0,
            8 => bad.items[0].stack_limit = 1_000_001,
            9 => bad.classes[0].inventories[0].initial_slots = 5,
            10 => bad.classes[0].inventories[0].overflow_limit = 257,
            11 => bad.native_catalog_id = "a".repeat(64),
            12 => bad.native_map = "missing".into(),
            _ => bad.format = 2,
        }
        assert!(
            CompiledInventoryContractsV1::compile(bad, &c).is_err(),
            "case {change}"
        );
    }
    let mut wrong = contracts().snapshot().as_ref().clone();
    wrong.contract_id = "f".repeat(64);
    assert!(CompiledInventoryContractsV1::from_snapshot(&wrong, &c).is_err());
}

#[test]
fn initial_routes_from_actual_staged_slots_and_assigns_ids_in_source_order() {
    let c = contracts();
    let p = prepare(
        &c,
        vec![
            item(2, "zero", 255, "stone", 2),
            item(2, "zero", 255, "apple", 3),
            item(1, "bag", 1, "apple", 4),
        ],
    );
    let z = inv(p.state(), 2, "zero");
    assert_eq!(z.slots.len(), 1);
    assert_eq!(z.slots[0].as_ref().unwrap().id.get(), 1);
    assert_eq!(z.overflow[0].stack.id.get(), 2);
    assert_eq!(z.overflow[0].requested_slot, 255);
    assert_eq!(
        inv(p.state(), 1, "bag").slots[1].as_ref().unwrap().id.get(),
        3
    );
    assert_eq!(p.state().next_stack_id, 4);
    assert_eq!(p.state().revision, 1);
    assert_eq!(p.state().owners[0].owner, handle(1));
    let p = prepare(&c, vec![]);
    assert_eq!(p.state().next_stack_id, 1);
    assert_eq!(p.state().owners.len(), 2);
    assert_eq!(p.initial_totals().items.len(), 2);
    assert!(p.initial_totals().items.iter().all(|i| i.count == 0));
    assert!(p.state().owners.iter().all(|o| o.inventories.len() == 2));
    assert_eq!(
        p.initial_state_digest(),
        prepare(&c, vec![]).initial_state_digest()
    );
}

#[test]
fn refusal_of_zero_route_full_overflow_and_ineligible_first_slot_is_atomic() {
    let cat = catalog_with_owners(2);
    let mut d = definition(&cat);
    d.classes[0].inventories[0].force_add_when_zero = false;
    let c = CompiledInventoryContractsV1::compile(d.clone(), &cat).unwrap();
    assert!(
        prepare_initial_inventory(
            &c,
            &live(),
            &InitialInventoryItemsV1 {
                items: vec![item(2, "zero", 9, "apple", 1)]
            }
        )
        .is_err()
    );
    d.classes[0].inventories[0].force_add_when_zero = true;
    d.classes[0].inventories[0].slot_rules[0]
        .allowed_items
        .clear();
    let c = CompiledInventoryContractsV1::compile(d, &cat).unwrap();
    assert!(
        prepare_initial_inventory(
            &c,
            &live(),
            &InitialInventoryItemsV1 {
                items: vec![item(2, "zero", 9, "apple", 1)]
            }
        )
        .is_err()
    );
    let c = contracts();
    let input = InitialInventoryItemsV1 {
        items: vec![
            item(1, "bag", 0, "apple", 1),
            item(1, "bag", 1, "apple", 1),
            item(1, "bag", 255, "apple", 1),
            item(1, "bag", 255, "apple", 1),
            item(1, "bag", 255, "apple", 1),
        ],
    };
    let before = input.clone();
    assert!(prepare_initial_inventory(&c, &live(), &input).is_err());
    assert_eq!(before, input);
    for bad in [
        item(1, "bag", 256, "apple", 1),
        item(1, "bag", 0, "apple", 0),
        item(1, "bag", 0, "apple", 11),
        item(3, "bag", 0, "apple", 1),
        item(1, "bag", 0, "missing", 1),
    ] {
        let input = InitialInventoryItemsV1 {
            items: vec![item(1, "bag", 0, "apple", 1), bad.clone()],
        };
        let before = input.clone();
        assert!(prepare_initial_inventory(&c, &live(), &input).is_err());
        assert_eq!(input, before);
        assert!(
            prepare_initial_inventory(&c, &live(), &InitialInventoryItemsV1 { items: vec![bad] })
                .is_err()
        );
    }
}

#[test]
fn cross_owner_actions_preserve_quantities_allocate_and_retire_ids() {
    let c = contracts();
    let p = initial(&c);
    let t = p.initial_totals();
    let s = apply(
        &c,
        p.state(),
        t,
        InventoryActionV1::Move {
            source: source(1, "bag", 1),
            destination: slot(2, "bag", 1),
        },
    );
    assert!(inv(&s, 1, "bag").slots[0].is_none());
    let s = apply(
        &c,
        &s,
        t,
        InventoryActionV1::Split {
            source: source(2, "bag", 1),
            destination: slot(1, "bag", 0),
            amount: 3,
        },
    );
    assert_eq!(s.next_stack_id, 5);
    assert_eq!(inv(&s, 1, "bag").slots[0].as_ref().unwrap().id.get(), 4);
    let s = apply(
        &c,
        &s,
        t,
        InventoryActionV1::Merge {
            source: source(1, "bag", 4),
            destination: source(2, "bag", 2),
            amount: 3,
        },
    );
    assert!(inv(&s, 1, "bag").slots[0].is_none());
    assert_eq!(s.next_stack_id, 5);
    let s = apply(
        &c,
        &s,
        t,
        InventoryActionV1::Consume {
            source: source(2, "bag", 1),
            amount: 2,
        },
    );
    assert_eq!(inv(&s, 2, "bag").slots[1].as_ref().unwrap().count, 3);
    let s = apply(
        &c,
        &s,
        t,
        InventoryActionV1::Consume {
            source: source(2, "bag", 1),
            amount: 3,
        },
    );
    assert!(inv(&s, 2, "bag").slots[1].is_none());
    assert_eq!(
        s.consumed.iter().find(|v| v.item == "apple").unwrap().count,
        5
    );
    let s = apply(
        &c,
        &s,
        t,
        InventoryActionV1::PlaceOverflow {
            source: source(1, "bag", 3),
            destination: overflow_destination(1, "bag", Some(1)),
        },
    );
    assert!(inv(&s, 1, "bag").overflow.is_empty());
    assert_eq!(inv(&s, 1, "bag").slots[1].as_ref().unwrap().id.get(), 3);
    let s = apply(
        &c,
        &s,
        t,
        InventoryActionV1::Split {
            source: source(2, "bag", 2),
            destination: slot(1, "bag", 0),
            amount: 1,
        },
    );
    assert_eq!(inv(&s, 1, "bag").slots[0].as_ref().unwrap().id.get(), 5);
    assert_eq!(s.next_stack_id, 6);
}

#[test]
fn overflow_append_is_explicit_bounded_and_preserves_remaining_order() {
    let c = contracts();
    let p = prepare(
        &c,
        vec![
            item(1, "bag", 255, "apple", 2),
            item(1, "bag", 254, "stone", 3),
        ],
    );
    let s = apply(
        &c,
        p.state(),
        p.initial_totals(),
        InventoryActionV1::PlaceOverflow {
            source: source(1, "bag", 1),
            destination: overflow_destination(1, "bag", None),
        },
    );
    assert_eq!(inv(&s, 1, "bag").slots.len(), 3);
    assert_eq!(inv(&s, 1, "bag").overflow[0].stack.id.get(), 2);
    let s = apply(
        &c,
        &s,
        p.initial_totals(),
        InventoryActionV1::PlaceOverflow {
            source: source(1, "bag", 2),
            destination: overflow_destination(1, "bag", None),
        },
    );
    assert_eq!(inv(&s, 1, "bag").slots.len(), 4);
    let p = prepare(
        &c,
        vec![
            item(1, "zero", 255, "apple", 1),
            item(1, "zero", 255, "apple", 1),
            item(1, "zero", 255, "stone", 1),
        ],
    );
    let s = apply(
        &c,
        p.state(),
        p.initial_totals(),
        InventoryActionV1::PlaceOverflow {
            source: source(1, "zero", 2),
            destination: overflow_destination(1, "zero", None),
        },
    );
    assert_eq!(inv(&s, 1, "zero").overflow[0].stack.id.get(), 3);
}

#[test]
fn overflow_can_append_to_another_owner_or_inventory_and_reload_exactly() {
    let c = contracts();
    let p = prepare(
        &c,
        vec![
            item(1, "bag", 255, "stone", 3),
            item(1, "bag", 254, "apple", 2),
        ],
    );
    for owner in [1, 2] {
        assert!(inv(p.state(), owner, "zero").slots.is_empty());
        let s = apply(
            &c,
            p.state(),
            p.initial_totals(),
            InventoryActionV1::PlaceOverflow {
                source: source(1, "bag", 2),
                destination: overflow_destination(owner, "zero", None),
            },
        );
        let target = inv(&s, owner, "zero");
        assert_eq!(target.slots.len(), 1);
        // A zero-length destination reserves the decoder's four-slot growth.
        assert_eq!(target.slots.capacity(), 4);
        assert_eq!(
            target.slots[0],
            Some(inv(p.state(), 1, "bag").overflow[1].stack.clone())
        );
        assert!(target.overflow.is_empty());
        assert_eq!(
            inv(&s, 1, "bag").overflow.as_slice(),
            &inv(p.state(), 1, "bag").overflow[..1]
        );
        assert_eq!(s.revision, p.state().revision + 1);
        assert_eq!(s.next_stack_id, p.state().next_stack_id);
        assert_eq!(s.consumed, p.state().consumed);
        let reloaded =
            InventoryStateSnapshotV1::parse(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(reloaded, s);
        validate_inventory_state(&c, &reloaded, p.initial_totals(), &live()).unwrap();
    }
}

#[test]
fn overflow_placement_and_move_use_existing_cross_owner_slots_without_growth() {
    let c = contracts();
    let p = initial(&c);
    for action in [
        InventoryActionV1::PlaceOverflow {
            source: source(1, "bag", 3),
            destination: overflow_destination(2, "bag", Some(1)),
        },
        InventoryActionV1::Move {
            source: source(1, "bag", 3),
            destination: slot(2, "bag", 1),
        },
    ] {
        let s = apply(&c, p.state(), p.initial_totals(), action);
        assert!(inv(&s, 1, "bag").overflow.is_empty());
        assert_eq!(inv(&s, 2, "bag").slots.len(), 2);
        assert_eq!(
            inv(&s, 2, "bag").slots[1],
            Some(inv(p.state(), 1, "bag").overflow[0].stack.clone())
        );
        assert_eq!(s.next_stack_id, p.state().next_stack_id);
        assert_eq!(s.revision, 2);
        let reloaded =
            InventoryStateSnapshotV1::parse(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(reloaded, s);
        validate_inventory_state(&c, &reloaded, p.initial_totals(), &live()).unwrap();
    }
}

#[test]
fn overflow_destinations_reject_full_ineligible_stale_and_unknown_inputs_atomically() {
    let cat = catalog_with_owners(2);
    let mut d = definition(&cat);
    for class in &mut d.classes {
        let zero = class
            .inventories
            .iter_mut()
            .find(|i| i.key == "zero")
            .unwrap();
        zero.slot_rules[0].allowed_items = vec!["apple".into()];
    }
    let c = CompiledInventoryContractsV1::compile(d, &cat).unwrap();
    let p = initial(&c); // Overflow stack 3 is stone, so zero's next slot refuses it.
    let mut stale_source = source(1, "bag", 3);
    stale_source.owner.incarnation += 1;
    let mut stale_destination = overflow_destination(2, "bag", Some(1));
    stale_destination.owner.incarnation += 1;
    let mut oversized_key = overflow_destination(2, "bag", Some(1));
    oversized_key.inventory.reserve_exact(133);
    for (source, destination) in [
        (source(1, "bag", 3), overflow_destination(2, "zero", None)),
        (source(1, "bag", 3), overflow_destination(2, "bag", Some(0))),
        (source(1, "bag", 3), overflow_destination(2, "bag", Some(2))),
        (
            source(1, "bag", 3),
            overflow_destination(2, "zero", Some(0)),
        ),
        (
            source(1, "bag", 3),
            overflow_destination(2, "missing", None),
        ),
        (
            source(1, "missing", 3),
            overflow_destination(2, "bag", None),
        ),
        (source(1, "bag", 999), overflow_destination(2, "bag", None)),
        (source(1, "bag", 1), overflow_destination(2, "bag", None)),
        (stale_source, overflow_destination(2, "bag", Some(1))),
        (source(1, "bag", 3), stale_destination),
        (source(1, "bag", 3), overflow_destination(3, "bag", None)),
        (source(1, "bag", 3), oversized_key),
    ] {
        assert_action_refused(
            &c,
            p.state(),
            p.initial_totals(),
            InventoryActionRequestV1 {
                expected_inventory_revision: 1,
                action: InventoryActionV1::PlaceOverflow {
                    source,
                    destination,
                },
            },
        );
    }
    let action = InventoryActionV1::PlaceOverflow {
        source: source(1, "bag", 3),
        destination: overflow_destination(2, "bag", None),
    };
    for revision in [0, 2] {
        assert_action_refused(
            &c,
            p.state(),
            p.initial_totals(),
            InventoryActionRequestV1 {
                expected_inventory_revision: revision,
                action: action.clone(),
            },
        );
    }
    for change in 0..7 {
        let mut state = p.state().clone();
        match change {
            0 => inv_mut(&mut state, 2, "bag").slots.resize(4, None), // No next slot.
            1 => inv_mut(&mut state, 2, "bag")
                .overflow
                .push(DeferredStackV1 {
                    stack: StackV1 {
                        id: StackId::new(4).unwrap(),
                        item: "apple".into(),
                        count: 1,
                    },
                    requested_slot: 256,
                }),
            2 => inv_mut(&mut state, 2, "zero").key = "missing".into(),
            3 => {
                inv_mut(&mut state, 2, "bag").slots[0]
                    .as_mut()
                    .unwrap()
                    .count += 1
            }
            4 => {
                inv_mut(&mut state, 2, "bag").slots[0]
                    .as_mut()
                    .unwrap()
                    .item = "missing".into()
            }
            5 => {
                state
                    .owners
                    .iter_mut()
                    .find(|o| o.owner.id == DomainId(2))
                    .unwrap()
                    .owner
                    .incarnation += 1
            }
            _ => state.revision = u64::MAX,
        }
        assert_action_refused(
            &c,
            &state,
            p.initial_totals(),
            InventoryActionRequestV1 {
                expected_inventory_revision: state.revision,
                action: action.clone(),
            },
        );
    }
    // An unrelated bad owner must also reject a valid same-owner placement.
    let mut state = p.state().clone();
    let unrelated_overflow = inv(&state, 1, "bag").overflow[0].clone();
    inv_mut(&mut state, 2, "bag").overflow = vec![unrelated_overflow; 3];
    assert_action_refused(
        &c,
        &state,
        p.initial_totals(),
        InventoryActionRequestV1 {
            expected_inventory_revision: 1,
            action: InventoryActionV1::PlaceOverflow {
                source: source(1, "bag", 3),
                destination: overflow_destination(1, "bag", Some(1)),
            },
        },
    );
}

#[test]
fn stale_handles_revisions_and_invalid_commands_leave_input_unchanged() {
    let c = contracts();
    let p = initial(&c);
    let mut stale = source(1, "bag", 1);
    stale.owner.incarnation = 2;
    let commands = vec![
        InventoryActionV1::Move {
            source: stale,
            destination: slot(2, "bag", 1),
        },
        InventoryActionV1::Move {
            source: source(1, "bag", 1),
            destination: slot(2, "bag", 0),
        },
        InventoryActionV1::Split {
            source: source(1, "bag", 1),
            destination: slot(2, "bag", 1),
            amount: 0,
        },
        InventoryActionV1::Split {
            source: source(1, "bag", 1),
            destination: slot(2, "bag", 1),
            amount: 8,
        },
        InventoryActionV1::Merge {
            source: source(1, "bag", 1),
            destination: source(1, "bag", 1),
            amount: 1,
        },
        InventoryActionV1::Merge {
            source: source(1, "bag", 3),
            destination: source(2, "bag", 2),
            amount: 1,
        },
        InventoryActionV1::Merge {
            source: source(2, "bag", 2),
            destination: source(1, "bag", 1),
            amount: 3,
        },
        InventoryActionV1::Consume {
            source: source(1, "bag", 1),
            amount: 9,
        },
        InventoryActionV1::Consume {
            source: source(1, "bag", 1),
            amount: 0,
        },
        InventoryActionV1::PlaceOverflow {
            source: source(1, "bag", 1),
            destination: overflow_destination(1, "bag", None),
        },
        InventoryActionV1::PlaceOverflow {
            source: source(1, "bag", 3),
            destination: overflow_destination(1, "bag", Some(0)),
        },
        InventoryActionV1::PlaceOverflow {
            source: source(1, "bag", 3),
            destination: overflow_destination(1, "bag", Some(2)),
        },
    ];
    for action in commands {
        assert_action_refused(
            &c,
            p.state(),
            p.initial_totals(),
            InventoryActionRequestV1 {
                expected_inventory_revision: 1,
                action,
            },
        );
    }
    assert_action_refused(
        &c,
        p.state(),
        p.initial_totals(),
        InventoryActionRequestV1 {
            expected_inventory_revision: 2,
            action: noop(),
        },
    );
}

#[test]
fn noop_still_validates_every_owner_and_cas_and_exhaustion_is_checked() {
    let c = contracts();
    let p = initial(&c);
    let request = InventoryActionRequestV1 {
        expected_inventory_revision: 1,
        action: noop(),
    };
    let result =
        prepare_inventory_action(&c, p.state(), p.initial_totals(), &live(), &request).unwrap();
    assert_eq!(result.outcome(), InventoryActionOutcomeV1::NoChange);
    assert_eq!(result.state(), p.state());
    for change in 0..9 {
        let mut s = p.state().clone();
        match change {
            0 => inv_mut(&mut s, 2, "bag").slots[0].as_mut().unwrap().count = 0,
            1 => inv_mut(&mut s, 2, "bag").slots[0].as_mut().unwrap().item = "missing".into(),
            2 => inv_mut(&mut s, 2, "bag").slots[0].as_mut().unwrap().id = StackId::new(1).unwrap(),
            3 => s.owners[1].owner.incarnation += 1,
            4 => s.consumed[0].count = 1,
            5 => {
                s.owners[1].inventories.pop();
            }
            6 => s.next_stack_id = 3,
            7 => inv_mut(&mut s, 1, "bag").overflow[0].requested_slot = 256,
            _ => {
                s.consumed.push(s.consumed[0].clone());
            }
        }
        assert_action_refused(&c, &s, p.initial_totals(), request.clone());
    }
    let mut s = p.state().clone();
    s.revision = u64::MAX;
    let result = prepare_inventory_action(
        &c,
        &s,
        p.initial_totals(),
        &live(),
        &InventoryActionRequestV1 {
            expected_inventory_revision: u64::MAX,
            action: noop(),
        },
    )
    .unwrap();
    assert_eq!(result.state().revision, u64::MAX);
    assert_action_refused(
        &c,
        &s,
        p.initial_totals(),
        InventoryActionRequestV1 {
            expected_inventory_revision: u64::MAX,
            action: InventoryActionV1::Consume {
                source: source(1, "bag", 1),
                amount: 1,
            },
        },
    );
    let mut s = p.state().clone();
    s.next_stack_id = u64::MAX;
    assert_action_refused(
        &c,
        &s,
        p.initial_totals(),
        InventoryActionRequestV1 {
            expected_inventory_revision: 1,
            action: InventoryActionV1::Split {
                source: source(1, "bag", 1),
                destination: slot(2, "bag", 1),
                amount: 1,
            },
        },
    );
}

#[test]
fn exact_live_handle_and_total_sets_are_mandatory() {
    let c = contracts();
    let p = initial(&c);
    for handles in [
        vec![handle(1)],
        vec![handle(1), handle(1)],
        vec![handle(1), handle(3)],
        vec![
            handle(1),
            OwnerHandle {
                incarnation: 0,
                ..handle(2)
            },
        ],
    ] {
        let l = InventoryLiveOwnersV1 { owners: handles };
        assert!(validate_inventory_state(&c, p.state(), p.initial_totals(), &l).is_err());
        assert!(
            prepare_initial_inventory(&c, &l, &InitialInventoryItemsV1 { items: vec![] }).is_err()
        );
    }
    for change in 0..4 {
        let mut t = p.initial_totals().clone();
        match change {
            0 => {
                t.items.pop();
            }
            1 => t.items[1] = t.items[0].clone(),
            2 => t.items[0].count = u64::MAX,
            _ => t.items[0].item = "missing".into(),
        }
        assert!(validate_inventory_state(&c, p.state(), &t, &live()).is_err());
    }
    let mut s = p.state().clone();
    s.consumed[0].count = u64::MAX;
    assert!(validate_inventory_state(&c, &s, p.initial_totals(), &live()).is_err());
}

#[test]
fn restore_redistributes_exact_stacks_installs_new_handles_and_preserves_others() {
    let c = contracts();
    let p = initial(&c);
    let before = p.state().clone();
    let mut targets = before.owners.clone();
    let left = targets[0]
        .inventories
        .iter_mut()
        .find(|i| i.key == "bag")
        .unwrap()
        .slots[0]
        .take();
    let right = targets[1]
        .inventories
        .iter_mut()
        .find(|i| i.key == "bag")
        .unwrap()
        .slots[0]
        .take();
    targets[0]
        .inventories
        .iter_mut()
        .find(|i| i.key == "bag")
        .unwrap()
        .slots[0] = right;
    targets[1]
        .inventories
        .iter_mut()
        .find(|i| i.key == "bag")
        .unwrap()
        .slots[0] = left;
    targets[1].owner.incarnation = 2;
    let mut post = live();
    post.owners[0].incarnation = 2;
    let r = InventoryRestoreRequestV1 {
        expected_inventory_revision: 1,
        owners: targets,
    };
    let result =
        prepare_inventory_restore(&c, &before, p.initial_totals(), &live(), &post, &r).unwrap();
    assert_eq!(before, p.state().clone());
    assert_eq!(result.state().revision, 2);
    assert_eq!(result.state().next_stack_id, before.next_stack_id);
    assert_eq!(result.state().consumed, before.consumed);
    assert_eq!(result.state().owners[1].owner.incarnation, 2);
    assert_eq!(
        inv(result.state(), 1, "bag").slots[0]
            .as_ref()
            .unwrap()
            .id
            .get(),
        2
    );
    let r = InventoryRestoreRequestV1 {
        expected_inventory_revision: 1,
        owners: vec![before.owners[0].clone()],
    };
    let result =
        prepare_inventory_restore(&c, &before, p.initial_totals(), &live(), &live(), &r).unwrap();
    assert_eq!(result.state().revision, 2);
    assert_eq!(result.state().owners[1], before.owners[1]);
}

#[test]
fn restore_rejects_resurrection_quantity_changes_duplicate_targets_and_final_errors_atomically() {
    let c = contracts();
    let p = initial(&c);
    let consumed = apply(
        &c,
        p.state(),
        p.initial_totals(),
        InventoryActionV1::Consume {
            source: source(1, "bag", 1),
            amount: 8,
        },
    );
    let r = InventoryRestoreRequestV1 {
        expected_inventory_revision: consumed.revision,
        owners: p.state().owners.clone(),
    };
    assert!(
        prepare_inventory_restore(&c, &consumed, p.initial_totals(), &live(), &live(), &r).is_err()
    );
    for change in 0..9 {
        let before = p.state().clone();
        let mut r = InventoryRestoreRequestV1 {
            expected_inventory_revision: 1,
            owners: before.owners.clone(),
        };
        match change {
            0 => r.owners[1].inventories[0].slots[0].as_mut().unwrap().count += 1,
            1 => r.owners.push(r.owners[0].clone()),
            2 => {
                r.owners[1].inventories.pop();
            }
            3 => {
                r.owners[1].inventories[0].slots[0].as_mut().unwrap().id = StackId::new(99).unwrap()
            }
            4 => r.owners[1].inventories[0].slots[0].as_mut().unwrap().item = "missing".into(),
            5 => r.owners[1].inventories[0].slots.push(None),
            6 => r.owners[1].owner.incarnation = 2,
            7 => r.expected_inventory_revision = 2,
            _ => r.owners.clear(),
        }
        // Case 5 is valid capacity; make the final target structurally invalid
        // while retaining every stack triple.
        if change == 5 {
            r.owners[1].inventories[0].slots.resize(5, None);
        }
        assert!(
            prepare_inventory_restore(&c, &before, p.initial_totals(), &live(), &live(), &r)
                .is_err(),
            "case {change}"
        );
        assert_eq!(&before, p.state());
    }
    let r = InventoryRestoreRequestV1 {
        expected_inventory_revision: 1,
        owners: vec![p.state().owners[0].clone()],
    };
    let mut post = live();
    post.owners[0].incarnation = 2;
    assert!(
        prepare_inventory_restore(&c, p.state(), p.initial_totals(), &live(), &post, &r).is_err()
    );
}

fn strict<T: DeserializeOwned + Serialize>(value: T) {
    let original = serde_json::to_value(value).unwrap();
    let mut unknown = original.clone();
    unknown["private_unknown"] = json!("private_value");
    let error = serde_json::from_value::<T>(unknown)
        .err()
        .unwrap()
        .to_string();
    assert!(!error.contains("private_"));
    assert!(serde_json::from_str::<T>("[]").is_err());
    let object = original.as_object().unwrap();
    let (key, v) = object.iter().next().unwrap();
    let text = serde_json::to_string(&original).unwrap();
    let duplicate = format!(
        "{{{}:{},{}",
        serde_json::to_string(key).unwrap(),
        v,
        &text[1..]
    );
    assert!(serde_json::from_str::<T>(&duplicate).is_err());
}

#[test]
fn every_named_dto_is_strict_and_debug_and_errors_are_value_free() {
    let c = contracts();
    let p = initial(&c);
    strict(c.definition().items[0].clone());
    strict(c.definition().classes[0].inventories[0].slot_rules[0].clone());
    strict(c.definition().classes[0].inventories[0].clone());
    strict(c.definition().classes[0].clone());
    strict(c.definition().clone());
    strict(c.snapshot().as_ref().clone());
    strict(inv(p.state(), 1, "bag").slots[0].as_ref().unwrap().clone());
    strict(inv(p.state(), 1, "bag").overflow[0].clone());
    strict(inv(p.state(), 1, "bag").clone());
    strict(p.state().owners[0].clone());
    strict(p.initial_totals().items[0].clone());
    strict(p.state().clone());
    strict(item(1, "bag", 0, "apple", 1));
    strict(live());
    strict(InitialInventoryItemsV1 { items: vec![] });
    strict(p.initial_totals().clone());
    strict(source(1, "bag", 1));
    strict(slot(1, "bag", 0));
    strict(overflow_destination(2, "zero", None));
    strict(overflow_destination(2, "bag", Some(1)));
    strict(InventoryActionRequestV1 {
        expected_inventory_revision: 1,
        action: noop(),
    });
    strict(InventoryRestoreRequestV1 {
        expected_inventory_revision: 1,
        owners: vec![],
    });
    strict(noop());
    for text in [
        r#"{"type":"private_enum"}"#,
        r#"{"type":"consume","source":[],"amount":1}"#,
        r#"{"type":"consume","source":{"owner":{"id":1,"incarnation":1},"inventory":"bag","stack_id":1},"amount":1,"amount":2}"#,
    ] {
        let e = serde_json::from_str::<InventoryActionV1>(text)
            .unwrap_err()
            .to_string();
        assert!(!e.contains("private_enum"));
    }
    assert!(StackId::new(0).is_err());
    assert!(serde_json::from_str::<StackId>("0").is_err());
    let debug = format!(
        "{c:?} {p:?} {:?} {:?} {:?}",
        p.state(),
        p.initial_totals(),
        StackId::new(123456789).unwrap()
    );
    for private in ["apple", "stone", "bag", "alpha", c.identity(), "123456789"] {
        assert!(!debug.contains(private));
    }
}

#[test]
fn actions_roundtrip_with_decode_spare_capacity_and_can_continue() {
    let c = contracts();
    let p = initial(&c);
    let s = apply(
        &c,
        p.state(),
        p.initial_totals(),
        InventoryActionV1::Split {
            source: source(1, "bag", 1),
            destination: slot(2, "bag", 1),
            amount: 3,
        },
    );
    let decoded: InventoryStateSnapshotV1 =
        serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
    assert_eq!(
        InventoryStateSnapshotV1::parse(&serde_json::to_string(&s).unwrap()).unwrap(),
        s
    );
    assert_eq!(decoded, s);
    validate_inventory_state(&c, &decoded, p.initial_totals(), &live()).unwrap();
    let _ = apply(
        &c,
        &decoded,
        p.initial_totals(),
        InventoryActionV1::Merge {
            source: source(2, "bag", 4),
            destination: source(2, "bag", 2),
            amount: 3,
        },
    );
    for action in [
        noop(),
        InventoryActionV1::Split {
            source: source(1, "bag", 1),
            destination: slot(2, "bag", 1),
            amount: 1,
        },
        InventoryActionV1::Merge {
            source: source(1, "bag", 1),
            destination: source(2, "bag", 2),
            amount: 1,
        },
        InventoryActionV1::Consume {
            source: source(1, "bag", 1),
            amount: 1,
        },
        InventoryActionV1::PlaceOverflow {
            source: source(1, "bag", 3),
            destination: overflow_destination(1, "bag", None),
        },
    ] {
        assert_eq!(
            serde_json::from_str::<InventoryActionV1>(&serde_json::to_string(&action).unwrap())
                .unwrap(),
            action
        );
    }
}

#[test]
fn logical_and_spare_allocation_limits_apply_before_noops_and_clones() {
    let c = contracts();
    let p = initial(&c);
    let mut s = p.state().clone();
    inv_mut(&mut s, 2, "bag").slots.reserve_exact(517);
    assert_action_refused(
        &c,
        &s,
        p.initial_totals(),
        InventoryActionRequestV1 {
            expected_inventory_revision: 1,
            action: noop(),
        },
    );
    let mut d = c.definition().clone();
    d.items.reserve_exact(2053);
    let cat = catalog_with_owners(2);
    assert!(CompiledInventoryContractsV1::compile(d, &cat).is_err());
    let mut s = p.state().clone();
    inv_mut(&mut s, 2, "bag").key.reserve_exact(133);
    assert!(validate_inventory_state(&c, &s, p.initial_totals(), &live()).is_err());
    let items = InitialInventoryItemsV1 {
        items: (0..513).map(|_| item(1, "bag", 0, "apple", 1)).collect(),
    };
    assert!(prepare_initial_inventory(&c, &live(), &items).is_err());
    let mut s = p.state().clone();
    inv_mut(&mut s, 1, "bag").slots.resize(257, None);
    assert!(validate_inventory_state(&c, &s, p.initial_totals(), &live()).is_err());
    let input = json!({"key":"bag","slots":vec![Value::Null;257],"overflow":[]});
    assert!(serde_json::from_value::<InventoryStateV1>(input).is_err());
    let input = json!({"allowed_items":vec!["apple";1025]});
    assert!(serde_json::from_value::<SlotRuleV1>(input).is_err());
    let input = json!({"key":"x".repeat(65),"stack_limit":1});
    assert!(serde_json::from_value::<ItemDefinitionV1>(input).is_err());
    let oversized = " ".repeat(MAX_INVENTORY_CONTRACT_BYTES + 1);
    assert!(CompiledInventoryContractsV1::parse(&oversized, &cat).is_err());
}

#[test]
fn recipe_expansion_checks_global_instances_and_allocations_before_materializing() {
    let c = catalog_with_owners(8193);
    let mut d = definition(&c);
    for cl in &mut d.classes {
        cl.inventories = (0..8)
            .map(|i| recipe(&format!("inv_{i}"), 0, false))
            .collect();
    }
    assert!(CompiledInventoryContractsV1::compile(d, &c).is_err());
    let c = catalog_with_owners(4096);
    let mut d = definition(&c);
    for cl in &mut d.classes {
        cl.inventories = (0..4)
            .map(|i| {
                let mut r = recipe(&format!("inv_{i}"), 256, false);
                r.slot_rules = vec![
                    SlotRuleV1 {
                        allowed_items: vec!["apple".into()]
                    };
                    256
                ];
                r
            })
            .collect();
    }
    assert!(CompiledInventoryContractsV1::compile(d, &c).is_err());
}

#[test]
fn regular_file_loading_is_bounded_and_value_free() {
    let c = catalog_with_owners(2);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("private_path");
    std::fs::write(&path, serde_json::to_vec(&definition(&c)).unwrap()).unwrap();
    assert_eq!(
        CompiledInventoryContractsV1::load(&path, &c)
            .unwrap()
            .identity(),
        contracts().identity()
    );
    assert!(CompiledInventoryContractsV1::load(dir.path(), &c).is_err());
    std::fs::write(&path, b"{\"private_name\":\"private_value\"}").unwrap();
    let error = CompiledInventoryContractsV1::load(&path, &c)
        .unwrap_err()
        .to_string();
    assert!(!error.contains("private_"));
    let file = std::fs::File::create(&path).unwrap();
    file.set_len(MAX_INVENTORY_CONTRACT_BYTES as u64 + 1)
        .unwrap();
    assert!(CompiledInventoryContractsV1::load(&path, &c).is_err());
}

#[test]
fn noops_and_restore_preserve_admitted_noncanonical_order() {
    let c = contracts();
    let p = initial(&c);
    let mut s = p.state().clone();
    s.owners.reverse();
    s.owners[0].inventories.reverse();
    s.consumed.reverse();
    let result = prepare_inventory_action(
        &c,
        &s,
        p.initial_totals(),
        &live(),
        &InventoryActionRequestV1 {
            expected_inventory_revision: 1,
            action: noop(),
        },
    )
    .unwrap();
    assert_eq!(result.state(), &s);
    let target = s
        .owners
        .iter()
        .find(|o| o.owner.id == DomainId(1))
        .unwrap()
        .clone();
    let result = prepare_inventory_restore(
        &c,
        &s,
        p.initial_totals(),
        &live(),
        &live(),
        &InventoryRestoreRequestV1 {
            expected_inventory_revision: 1,
            owners: vec![target],
        },
    )
    .unwrap();
    assert_eq!(result.state().owners[0], s.owners[0]);
    assert_eq!(result.state().consumed, s.consumed);
}

#[test]
fn saturated_capacity_and_eligibility_refuse_explicit_placement_and_moves() {
    let cat = catalog_with_owners(2);
    let mut d = definition(&cat);
    for cl in &mut d.classes {
        let bag = cl.inventories.iter_mut().find(|i| i.key == "bag").unwrap();
        bag.slot_rules.truncate(2);
        bag.slot_rules[1].allowed_items = vec!["stone".into()];
    }
    let c = CompiledInventoryContractsV1::compile(d, &cat).unwrap();
    let p = initial(&c);
    for action in [
        InventoryActionV1::Move {
            source: source(1, "bag", 1),
            destination: slot(2, "bag", 1),
        },
        InventoryActionV1::PlaceOverflow {
            source: source(1, "bag", 3),
            destination: overflow_destination(1, "bag", None),
        },
    ] {
        assert_action_refused(
            &c,
            p.state(),
            p.initial_totals(),
            InventoryActionRequestV1 {
                expected_inventory_revision: 1,
                action,
            },
        );
    }
    let s = apply(
        &c,
        p.state(),
        p.initial_totals(),
        InventoryActionV1::PlaceOverflow {
            source: source(1, "bag", 3),
            destination: overflow_destination(1, "bag", Some(1)),
        },
    );
    assert_eq!(inv(&s, 1, "bag").slots.len(), 2);
}

#[test]
fn aggregate_spare_allocation_is_independent_of_small_logical_and_encoded_counts() {
    let cat = catalog_with_owners(2);
    let mut d = definition(&cat);
    for cl in &mut d.classes {
        cl.inventories = (0..3)
            .map(|i| InventoryDefinitionV1 {
                key: format!("inv_{i}"),
                initial_slots: 0,
                force_add_when_zero: false,
                overflow_limit: 0,
                slot_rules: (0..256)
                    .map(|_| SlotRuleV1 {
                        allowed_items: Vec::with_capacity(2052),
                    })
                    .collect(),
            })
            .collect();
    }
    assert!(serde_json::to_vec(&d).unwrap().len() < MAX_INVENTORY_CONTRACT_BYTES);
    assert!(
        CompiledInventoryContractsV1::compile(d, &cat)
            .unwrap_err()
            .to_string()
            .contains("allocation limit")
    );
}

#[test]
fn contract_encoding_limit_is_independent_of_logical_element_limits() {
    let cat = catalog_with_owners(2);
    let mut d = definition(&cat);
    d.items = (0..1024)
        .map(|i| ItemDefinitionV1 {
            key: format!("{i:04}{}", "a".repeat(60)),
            stack_limit: 1,
        })
        .collect();
    let keys: Vec<_> = d.items.iter().map(|i| i.key.clone()).collect();
    for cl in &mut d.classes {
        cl.inventories = vec![];
    }
    d.classes[0].inventories = vec![InventoryDefinitionV1 {
        key: "bag".into(),
        initial_slots: 0,
        force_add_when_zero: false,
        overflow_limit: 0,
        slot_rules: vec![
            SlotRuleV1 {
                allowed_items: keys
            };
            65
        ],
    }];
    assert!(serde_json::to_vec(&d).unwrap().len() > MAX_INVENTORY_CONTRACT_BYTES);
    assert!(CompiledInventoryContractsV1::compile(d, &cat).is_err());
}

#[test]
fn aggregate_active_stack_limit_is_enforced_in_validation_and_decoding() {
    let cat = catalog_with_owners(33);
    let mut d = definition(&cat);
    for cl in &mut d.classes {
        cl.inventories = (0..8)
            .map(|i| InventoryDefinitionV1 {
                key: format!("inv_{i}"),
                initial_slots: 0,
                force_add_when_zero: false,
                overflow_limit: 256,
                slot_rules: vec![
                    SlotRuleV1 {
                        allowed_items: vec!["apple".into()]
                    };
                    256
                ],
            })
            .collect();
    }
    let c = CompiledInventoryContractsV1::compile(d, &cat).unwrap();
    let l = InventoryLiveOwnersV1 {
        owners: (1..=33).map(handle).collect(),
    };
    let mut next_id = 1;
    let owners = l
        .owners
        .iter()
        .map(|owner| OwnerInventoriesV1 {
            owner: *owner,
            inventories: (0..8)
                .map(|i| {
                    let count = (MAX_INVENTORY_STACKS + 2 - next_id).min(256);
                    let slots = (0..count)
                        .map(|_| {
                            let stack = StackV1 {
                                id: StackId::new(next_id as u64).unwrap(),
                                item: "apple".into(),
                                count: 1,
                            };
                            next_id += 1;
                            Some(stack)
                        })
                        .collect();
                    InventoryStateV1 {
                        key: format!("inv_{i}"),
                        slots,
                        overflow: vec![],
                    }
                })
                .collect(),
        })
        .collect();
    let s = InventoryStateSnapshotV1 {
        format: 1,
        contract_id: c.identity().into(),
        revision: 1,
        next_stack_id: next_id as u64,
        consumed: vec![
            ItemTotalV1 {
                item: "apple".into(),
                count: 0,
            },
            ItemTotalV1 {
                item: "stone".into(),
                count: 0,
            },
        ],
        owners,
    };
    assert_eq!(next_id, MAX_INVENTORY_STACKS + 2);
    let totals = InventoryInitialTotalsV1 {
        items: vec![
            ItemTotalV1 {
                item: "apple".into(),
                count: MAX_INVENTORY_STACKS as u64 + 1,
            },
            ItemTotalV1 {
                item: "stone".into(),
                count: 0,
            },
        ],
    };
    assert!(validate_inventory_state(&c, &s, &totals, &l).is_err());
    assert!(
        serde_json::from_str::<InventoryStateSnapshotV1>(&serde_json::to_string(&s).unwrap())
            .is_err()
    );
}

#[test]
fn state_encoding_limit_is_checked_under_the_graph_and_stack_limits() {
    let cat = catalog_with_owners(8192);
    let mut d = definition(&cat);
    let item_key = "a".repeat(64);
    d.items = vec![ItemDefinitionV1 {
        key: item_key.clone(),
        stack_limit: 1,
    }];
    for cl in &mut d.classes {
        cl.inventories = (0..8)
            .map(|i| InventoryDefinitionV1 {
                key: format!("{i}{}", "i".repeat(63)),
                initial_slots: 16,
                force_add_when_zero: false,
                overflow_limit: 0,
                slot_rules: vec![
                    SlotRuleV1 {
                        allowed_items: vec![item_key.clone()]
                    };
                    16
                ],
            })
            .collect();
    }
    let c = CompiledInventoryContractsV1::compile(d, &cat).unwrap();
    let l = InventoryLiveOwnersV1 {
        owners: (1..=8192).map(handle).collect(),
    };
    let mut next_id = 1;
    let owners = l
        .owners
        .iter()
        .map(|owner| OwnerInventoriesV1 {
            owner: *owner,
            inventories: (0..8)
                .map(|i| {
                    let mut slots = vec![None; 16];
                    slots[0] = Some(StackV1 {
                        id: StackId::new(next_id).unwrap(),
                        item: item_key.clone(),
                        count: 1,
                    });
                    next_id += 1;
                    InventoryStateV1 {
                        key: format!("{i}{}", "i".repeat(63)),
                        slots,
                        overflow: vec![],
                    }
                })
                .collect(),
        })
        .collect();
    let s = InventoryStateSnapshotV1 {
        format: 1,
        contract_id: c.identity().into(),
        revision: 1,
        next_stack_id: next_id,
        consumed: vec![ItemTotalV1 {
            item: item_key.clone(),
            count: 0,
        }],
        owners,
    };
    let totals = InventoryInitialTotalsV1 {
        items: vec![ItemTotalV1 {
            item: item_key,
            count: 65536,
        }],
    };
    assert!(serde_json::to_vec(&s).unwrap().len() > MAX_INVENTORY_STATE_BYTES);
    let error = validate_inventory_state(&c, &s, &totals, &l)
        .unwrap_err()
        .to_string();
    assert!(error.contains("encoding limit"), "{error}");
}

#[test]
fn malformed_action_field_sets_are_strict_with_tag_last() {
    let endpoint = json!({"owner":{"id":1,"incarnation":1},"inventory":"bag","stack_id":1});
    let action = json!({"source":endpoint.clone(),"destination":{"owner":{"id":2,"incarnation":1},"inventory":"zero","slot":null},"type":"place_overflow"});
    assert!(serde_json::from_value::<InventoryActionV1>(action.clone()).is_ok());
    for extra in [
        json!({"amount":null}),
        json!({"amount":1}),
        json!({"destination":null}),
        json!({"slot":null}),
    ] {
        let mut bad = action.clone();
        bad.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        assert!(serde_json::from_value::<InventoryActionV1>(bad).is_err());
    }
    let mut missing = action;
    missing["destination"]
        .as_object_mut()
        .unwrap()
        .remove("slot");
    assert!(serde_json::from_value::<InventoryActionV1>(missing).is_err());
    let good = format!(r#"{{"source":{},"amount":1,"type":"consume"}}"#, endpoint);
    assert!(serde_json::from_str::<InventoryActionV1>(&good).is_ok());
    let bad = format!(
        r#"{{"source":{},"amount":1,"type":"consume","type":"consume"}}"#,
        endpoint
    );
    assert!(serde_json::from_str::<InventoryActionV1>(&bad).is_err());
}

#[test]
fn overflow_destination_json_requires_exact_explicit_shape_and_redacts_errors() {
    for target_slot in [None, Some(0), Some(255)] {
        let destination = overflow_destination(2, "zero", target_slot);
        let expected = json!({
            "owner":{"id":2,"incarnation":1}, "inventory":"zero", "slot":target_slot
        });
        assert_eq!(serde_json::to_value(&destination).unwrap(), expected);
        assert_eq!(
            serde_json::from_value::<InventoryOverflowDestinationV1>(expected.clone()).unwrap(),
            destination
        );
        let action = InventoryActionV1::PlaceOverflow {
            source: source(1, "bag", 3),
            destination: destination.clone(),
        };
        let expected = json!({
            "type":"place_overflow",
            "source":{"owner":{"id":1,"incarnation":1},"inventory":"bag","stack_id":3},
            "destination":expected,
        });
        assert_eq!(serde_json::to_value(&action).unwrap(), expected);
        assert_eq!(
            serde_json::from_value::<InventoryActionV1>(expected.clone()).unwrap(),
            action
        );
        let text = serde_json::to_string(&expected).unwrap();
        assert_eq!(
            serde_json::from_str::<InventoryActionV1>(&text).unwrap(),
            action
        );
        strict(action);
        for key in ["owner", "inventory", "slot"] {
            let mut bad = expected["destination"].clone();
            bad.as_object_mut().unwrap().remove(key);
            assert!(serde_json::from_value::<InventoryOverflowDestinationV1>(bad.clone()).is_err());
            let mut bad_action = expected.clone();
            bad_action["destination"] = bad;
            assert!(serde_json::from_value::<InventoryActionV1>(bad_action).is_err());
        }
        for key in ["source", "destination"] {
            let mut bad = expected.clone();
            bad.as_object_mut().unwrap().remove(key);
            assert!(serde_json::from_value::<InventoryActionV1>(bad).is_err());
        }
    }
    let good = json!({
        "type":"place_overflow",
        "source":{"owner":{"id":1,"incarnation":1},"inventory":"bag","stack_id":3},
        "destination":{"owner":{"id":2,"incarnation":1},"inventory":"zero","slot":null},
    });
    let mut bad_values = vec![json!(null), json!([])];
    for (key, value) in [
        ("owner", json!(null)),
        ("inventory", json!(null)),
        ("inventory", json!("private_invalid/value")),
        ("slot", json!("private_value")),
        ("slot", json!(-1)),
        ("slot", json!(65536)),
        ("slot", json!([])),
        ("slot", json!({"private_key":"private_value"})),
        ("stack_id", json!(1)),
        ("stack_id", json!(null)),
        ("private_unknown", json!({"private_key":"private_value"})),
    ] {
        let mut bad = good["destination"].clone();
        bad[key] = value;
        bad_values.push(bad);
    }
    for destination in bad_values {
        let error = serde_json::from_value::<InventoryOverflowDestinationV1>(destination.clone())
            .unwrap_err()
            .to_string();
        assert!(!error.contains("private"), "{error}");
        let mut action = good.clone();
        action["destination"] = destination;
        let error = serde_json::from_value::<InventoryActionV1>(action)
            .unwrap_err()
            .to_string();
        assert!(!error.contains("private"), "{error}");
    }
    for text in [
        r#"{"owner":{"id":2,"incarnation":1},"inventory":"zero","slot":null,"slot":1}"#,
        r#"{"owner":{"id":2,"incarnation":1},"inventory":"zero","slot":null,"inventory":"private_value"}"#,
        r#"{"owner":{"id":2,"incarnation":1,"private_key":0},"inventory":"zero","slot":null}"#,
    ] {
        let error = serde_json::from_str::<InventoryOverflowDestinationV1>(text)
            .unwrap_err()
            .to_string();
        assert!(!error.contains("private"), "{error}");
        let action = format!(
            r#"{{"destination":{text},"source":{},"type":"place_overflow"}}"#,
            good["source"]
        );
        let error = serde_json::from_str::<InventoryActionV1>(&action)
            .unwrap_err()
            .to_string();
        assert!(!error.contains("private"), "{error}");
    }
    // The shared action decoder must keep the other destinations exact too.
    for kind in ["move", "split", "merge"] {
        let mut bad = good.clone();
        bad["type"] = json!(kind);
        if kind != "move" {
            bad["amount"] = json!(1);
        }
        if kind == "merge" {
            bad["destination"]["stack_id"] = json!(2);
        }
        assert!(serde_json::from_value::<InventoryActionV1>(bad).is_err());
    }
    let debug = format!(
        "{:?}",
        overflow_destination(123456789, "private_inventory", None)
    );
    assert!(!debug.contains("private"));
    assert!(!debug.contains("123456789"));
}

fn wide_definition(cat: &CompiledNativeCatalog, slots: u16) -> InventoryContractsV1 {
    let mut d = definition(cat);
    for class in &mut d.classes {
        class.inventories = vec![InventoryDefinitionV1 {
            key: "bag".into(),
            initial_slots: slots,
            force_add_when_zero: false,
            overflow_limit: 1,
            slot_rules: vec![
                SlotRuleV1 {
                    allowed_items: vec!["apple".into()],
                };
                129
            ],
        }];
    }
    d
}

#[test]
fn exact_8192_owner_129_slot_reproducer_is_refused_before_expansion() {
    use std::mem::size_of;
    let cat = catalog_with_owners(8192);
    let d = wide_definition(&cat, 129);
    // Released initialization used exact capacity 129. Its reload grows every
    // slot vector to 256: 80 MiB for slots alone on the supported Linux target.
    assert_eq!(size_of::<Option<StackV1>>(), 40);
    let slot_bytes = 8192 * 129 * size_of::<Option<StackV1>>();
    let exact_bytes = slot_bytes
        + 8192 * (size_of::<OwnerInventoriesV1>() + size_of::<InventoryStateV1>() + 3)
        + size_of::<InventoryStateSnapshotV1>()
        + 64
        + 2 * size_of::<ItemTotalV1>()
        + 10;
    assert!(exact_bytes < MAX_INVENTORY_GRAPH_BYTES);
    assert!(8192 * 256 * size_of::<Option<StackV1>>() > MAX_INVENTORY_GRAPH_BYTES);
    assert!(
        CompiledInventoryContractsV1::compile(d, &cat)
            .unwrap_err()
            .to_string()
            .contains("allocation limit")
    );
    // An authored representation of the formerly published result fits the
    // encoding cap but cannot reload. No prepared result may expose it now.
    let s = InventoryStateSnapshotV1 {
        format: 1,
        contract_id: "a".repeat(64),
        revision: 1,
        next_stack_id: 1,
        consumed: ["apple", "stone"]
            .into_iter()
            .map(|item| ItemTotalV1 {
                item: item.into(),
                count: 0,
            })
            .collect(),
        owners: (1..=8192)
            .map(|id| OwnerInventoriesV1 {
                owner: handle(id),
                inventories: vec![InventoryStateV1 {
                    key: "bag".into(),
                    slots: vec![None; 129],
                    overflow: vec![],
                }],
            })
            .collect(),
    };
    assert_eq!(s.owners[0].inventories[0].slots.capacity(), 129);
    let text = serde_json::to_string(&s).unwrap();
    assert!(text.len() < MAX_INVENTORY_STATE_BYTES);
    assert!(InventoryStateSnapshotV1::parse(&text).is_err());
}

#[test]
fn admitted_near_boundary_initialization_and_contracts_reload() {
    let cat = catalog_with_owners(6200);
    let c = CompiledInventoryContractsV1::compile(wide_definition(&cat, 129), &cat).unwrap();
    let l = InventoryLiveOwnersV1 {
        owners: (1..=6200).map(handle).collect(),
    };
    let p = prepare_initial_inventory(&c, &l, &InitialInventoryItemsV1 { items: vec![] }).unwrap();
    // Reloaded slots alone consume over 94% of the unchanged graph allowance.
    assert!(
        6200 * 256 * std::mem::size_of::<Option<StackV1>>() > MAX_INVENTORY_GRAPH_BYTES * 94 / 100
    );
    let decoded =
        InventoryStateSnapshotV1::parse(&serde_json::to_string(p.state()).unwrap()).unwrap();
    assert_eq!(&decoded, p.state());
    assert_eq!(decoded.owners[0].inventories.capacity(), 4);
    assert_eq!(decoded.owners[0].inventories[0].slots.capacity(), 256);
    validate_inventory_state(&c, &decoded, p.initial_totals(), &l).unwrap();
    let totals: InventoryInitialTotalsV1 =
        serde_json::from_str(&serde_json::to_string(p.initial_totals()).unwrap()).unwrap();
    assert_eq!(&totals, p.initial_totals());
    let snapshot: InventoryContractsSnapshotV1 =
        serde_json::from_str(&serde_json::to_string(c.snapshot().as_ref()).unwrap()).unwrap();
    let reloaded = CompiledInventoryContractsV1::from_snapshot(&snapshot, &cat).unwrap();
    assert_eq!(reloaded.identity(), c.identity());
    assert_eq!(snapshot.definition.classes[0].inventories.capacity(), 4);
    assert_eq!(
        snapshot.definition.classes[0].inventories[0]
            .slot_rules
            .capacity(),
        256
    );
    assert_eq!(
        snapshot.definition.classes[0].inventories[0].slot_rules[0]
            .allowed_items
            .capacity(),
        4
    );
}

#[test]
fn append_and_restore_across_128_slots_reload_and_fail_atomically() {
    let cat = catalog_with_owners(2);
    let c = CompiledInventoryContractsV1::compile(wide_definition(&cat, 128), &cat).unwrap();
    let p = prepare(&c, vec![item(1, "bag", 255, "apple", 1)]);
    let request = InventoryActionRequestV1 {
        expected_inventory_revision: 1,
        action: InventoryActionV1::PlaceOverflow {
            source: source(1, "bag", 1),
            destination: overflow_destination(1, "bag", None),
        },
    };
    let before = serde_json::to_string(p.state()).unwrap();
    let result =
        prepare_inventory_action(&c, p.state(), p.initial_totals(), &live(), &request).unwrap();
    assert_eq!(serde_json::to_string(p.state()).unwrap(), before);
    assert_eq!(inv(result.state(), 1, "bag").slots.len(), 129);
    let decoded =
        InventoryStateSnapshotV1::parse(&serde_json::to_string(result.state()).unwrap()).unwrap();
    validate_inventory_state(&c, &decoded, p.initial_totals(), &live()).unwrap();
    assert_eq!(decoded, *result.state());
    // A second append is ineligible; revision, allocator and stack multiset
    // remain byte-for-byte unchanged on refusal.
    let mut repeat = request.clone();
    repeat.expected_inventory_revision = 2;
    assert_action_refused(&c, &decoded, p.initial_totals(), repeat);

    let mut replacement = p.state().owners[0].clone();
    replacement.inventories[0].slots.resize(129, None);
    let restore = InventoryRestoreRequestV1 {
        expected_inventory_revision: 1,
        owners: vec![replacement],
    };
    let restored = prepare_inventory_restore(
        &c,
        p.state(),
        p.initial_totals(),
        &live(),
        &live(),
        &restore,
    )
    .unwrap();
    assert_eq!(serde_json::to_string(p.state()).unwrap(), before);
    assert_eq!(inv(restored.state(), 1, "bag").slots.len(), 129);
    let decoded =
        InventoryStateSnapshotV1::parse(&serde_json::to_string(restored.state()).unwrap()).unwrap();
    validate_inventory_state(&c, &decoded, p.initial_totals(), &live()).unwrap();
    assert_eq!(decoded, *restored.state());
    let mut invalid = restore;
    invalid.owners[0].inventories[0].slots.push(None);
    assert!(
        prepare_inventory_restore(
            &c,
            p.state(),
            p.initial_totals(),
            &live(),
            &live(),
            &invalid
        )
        .is_err()
    );
    assert_eq!(serde_json::to_string(p.state()).unwrap(), before);
}

#[test]
fn json_and_toml_share_identity_strict_admission_and_size_guard() {
    let cat = catalog_with_owners(2);
    let d = wide_definition(&cat, 128);
    let json = serde_json::to_string(&d).unwrap();
    let toml = toml::to_string(&d).unwrap();
    let expected = CompiledInventoryContractsV1::compile(d.clone(), &cat).unwrap();
    for text in [&json, &toml] {
        let c = CompiledInventoryContractsV1::parse(text, &cat).unwrap();
        assert_eq!(c.identity(), expected.identity());
        assert_eq!(c.definition(), expected.definition());
        let oversized = format!(
            "{text}{}",
            " ".repeat(MAX_INVENTORY_CONTRACT_BYTES + 1 - text.len())
        );
        assert_eq!(
            CompiledInventoryContractsV1::parse(&oversized, &cat)
                .unwrap_err()
                .to_string(),
            "inventory contract byte limit"
        );
    }
    let mut duplicate = d;
    duplicate.items.push(duplicate.items[0].clone());
    for text in [
        serde_json::to_string(&duplicate).unwrap(),
        toml::to_string(&duplicate).unwrap(),
    ] {
        assert!(CompiledInventoryContractsV1::parse(&text, &cat).is_err());
    }
    for text in [
        json.replacen("\"format\":1", "\"format\":1,\"format\":1", 1),
        json.replacen("\"format\":1", "\"format\":1,\"private_unknown\":true", 1),
        json.replacen("\"stack_limit\":10", "\"stack_limit\":\"private_value\"", 1),
        format!("{json} trailing"),
        toml.replacen("format = 1", "format = 1\nformat = 1", 1),
        format!("private_unknown = true\n{toml}"),
        toml.replacen("stack_limit = 10", "stack_limit = \"private_value\"", 1),
        toml.replacen(
            "initial_slots = 128",
            "initial_slots = 128\nprivate_unknown = true",
            1,
        ),
        format!("{toml}\nbroken = ["),
    ] {
        let error = CompiledInventoryContractsV1::parse(&text, &cat)
            .unwrap_err()
            .to_string();
        assert!(!error.contains("private"), "{error}");
    }
}

#[test]
fn append_growth_cannot_use_clone_compaction_to_escape_caller_spare_budget() {
    use std::mem::size_of;
    let cat = catalog_with_owners(190);
    let mut d = wide_definition(&cat, 128);
    for class in &mut d.classes {
        let r = class.inventories.pop().unwrap();
        class.inventories = (0..8)
            .map(|i| InventoryDefinitionV1 {
                key: format!("bag_{i}"),
                ..r.clone()
            })
            .collect();
    }
    let c = CompiledInventoryContractsV1::compile(d, &cat).unwrap();
    let l = InventoryLiveOwnersV1 {
        owners: (1..=190).map(handle).collect(),
    };
    let (mut state, totals, _) = prepare_initial_inventory(
        &c,
        &l,
        &InitialInventoryItemsV1 {
            items: vec![item(1, "bag_0", 255, "apple", 1)],
        },
    )
    .unwrap()
    .into_parts();
    let request = InventoryActionRequestV1 {
        expected_inventory_revision: 1,
        action: InventoryActionV1::PlaceOverflow {
            source: source(1, "bag_0", 1),
            destination: overflow_destination(2, "bag_0", None),
        },
    };
    // This fixture has 190 owners (reload capacity 256), eight inventories
    // each, 128 slots and two totals (reload capacity four). Account explicitly
    // for that layout, then pad empty vectors without changing logical state.
    let mut charge = size_of::<InventoryStateSnapshotV1>()
        + state.contract_id.capacity()
        + 4 * size_of::<ItemTotalV1>()
        + state
            .consumed
            .iter()
            .map(|t| t.item.capacity())
            .sum::<usize>()
        + 256 * size_of::<OwnerInventoriesV1>()
        + size_of::<InventoryInitialTotalsV1>()
        + 4 * size_of::<ItemTotalV1>()
        + totals
            .items
            .iter()
            .map(|t| t.item.capacity())
            .sum::<usize>()
        + size_of::<InventoryLiveOwnersV1>()
        + 256 * size_of::<OwnerHandle>()
        + size_of::<InventoryActionRequestV1>()
        + 10; // source and destination inventory keys
    for owner in &state.owners {
        charge += 8 * size_of::<InventoryStateV1>();
        for inv in &owner.inventories {
            charge += inv.key.capacity()
                + inv.slots.capacity().max(128) * size_of::<Option<StackV1>>()
                + inv
                    .overflow
                    .capacity()
                    .max(if inv.overflow.is_empty() { 0 } else { 4 })
                    * size_of::<DeferredStackV1>()
                + inv
                    .overflow
                    .iter()
                    .map(|s| s.stack.item.capacity())
                    .sum::<usize>();
        }
    }
    let target = MAX_INVENTORY_GRAPH_BYTES - 1024;
    for owner in &mut state.owners {
        for inv in &mut owner.inventories {
            if owner.owner.id == DomainId(2) && inv.key == "bag_0" {
                continue;
            }
            let more =
                ((target - charge) / size_of::<Option<StackV1>>()).min(516 - inv.slots.capacity());
            let old = inv.slots.capacity();
            inv.slots.reserve_exact(old + more - inv.slots.len());
            charge += (inv.slots.capacity() - old) * size_of::<Option<StackV1>>();
            let more = ((target - charge) / size_of::<DeferredStackV1>())
                .min(516 - inv.overflow.capacity());
            let old = inv.overflow.capacity();
            inv.overflow.reserve_exact(old + more - inv.overflow.len());
            charge += (inv.overflow.capacity() - old) * size_of::<DeferredStackV1>();
        }
    }
    assert!(charge <= target && target - charge < size_of::<DeferredStackV1>());
    assert!(charge + 128 * size_of::<Option<StackV1>>() > MAX_INVENTORY_GRAPH_BYTES);
    validate_inventory_state(&c, &state, &totals, &l).unwrap();
    let before = serde_json::to_string(&state).unwrap();
    let decoded = InventoryStateSnapshotV1::parse(&before).unwrap();
    validate_inventory_state(&c, &decoded, &totals, &l).unwrap();
    let error = prepare_inventory_action(&c, &state, &totals, &l, &request).unwrap_err();
    assert_eq!(error.to_string(), "inventory result allocation limit");
    assert_eq!(serde_json::to_string(&state).unwrap(), before);
    assert_eq!(inv(&state, 2, "bag_0").slots.capacity(), 128);
    assert_eq!(inv(&state, 1, "bag_0").overflow.len(), 1);
    // The same logical state with the decoder's small allocations has room;
    // refusing the caller's oversized result must not silently compact it.
    let admitted = prepare_inventory_action(&c, &decoded, &totals, &l, &request).unwrap();
    assert!(inv(admitted.state(), 1, "bag_0").overflow.is_empty());
    assert_eq!(inv(admitted.state(), 2, "bag_0").slots.len(), 129);
    assert_eq!(inv(admitted.state(), 2, "bag_0").slots.capacity(), 256);
    assert_eq!(admitted.state().next_stack_id, state.next_stack_id);
    assert_eq!(admitted.state().revision, state.revision + 1);
    let reloaded =
        InventoryStateSnapshotV1::parse(&serde_json::to_string(admitted.state()).unwrap()).unwrap();
    assert_eq!(&reloaded, admitted.state());
    validate_inventory_state(&c, &reloaded, &totals, &l).unwrap();
}

#[test]
fn restore_growth_in_borrowed_targets_is_refused_before_cloning() {
    let cat = catalog_with_owners(8192);
    let c = CompiledInventoryContractsV1::compile(wide_definition(&cat, 128), &cat).unwrap();
    let l = InventoryLiveOwnersV1 {
        owners: (1..=8192).map(handle).collect(),
    };
    let p = prepare_initial_inventory(&c, &l, &InitialInventoryItemsV1 { items: vec![] }).unwrap();
    let request = InventoryRestoreRequestV1 {
        expected_inventory_revision: 1,
        owners: p
            .state()
            .owners
            .iter()
            .take(3000)
            .map(|owner| {
                let current = &owner.inventories[0];
                let mut slots = Vec::with_capacity(129);
                slots.extend_from_slice(&current.slots);
                slots.push(None);
                OwnerInventoriesV1 {
                    owner: owner.owner,
                    inventories: vec![InventoryStateV1 {
                        key: current.key.clone(),
                        slots,
                        overflow: vec![],
                    }],
                }
            })
            .collect(),
    };
    assert_eq!(request.owners[0].inventories[0].slots.capacity(), 129);
    // Exact source allocations fit; the simultaneous decoder capacities do not.
    let slots = (8192 * 128 + 3000 * 129) * std::mem::size_of::<Option<StackV1>>();
    assert!(slots + 2 * 1024 * 1024 < MAX_INVENTORY_GRAPH_BYTES);
    assert!(
        (8192 * 128 + 3000 * 256) * std::mem::size_of::<Option<StackV1>>()
            > MAX_INVENTORY_GRAPH_BYTES
    );
    let before = serde_json::to_string(p.state()).unwrap();
    assert!(
        prepare_inventory_restore(&c, p.state(), p.initial_totals(), &l, &l, &request)
            .unwrap_err()
            .to_string()
            .contains("allocation limit")
    );
    assert_eq!(serde_json::to_string(p.state()).unwrap(), before);
    let decoded = InventoryStateSnapshotV1::parse(&before).unwrap();
    validate_inventory_state(&c, &decoded, p.initial_totals(), &l).unwrap();
}

#[test]
fn declaration_item_rule_and_inventory_vectors_reload_across_growth_thresholds() {
    let cat = catalog_with_owners(2);
    let mut d = wide_definition(&cat, 0);
    d.items = std::iter::once("apple".to_owned())
        .chain((0..128).map(|i| format!("item_{i:03}")))
        .map(|key| ItemDefinitionV1 {
            key,
            stack_limit: 1,
        })
        .collect();
    let keys: Vec<_> = d.items.iter().map(|item| item.key.clone()).collect();
    for class in &mut d.classes {
        class.inventories = (0..5)
            .map(|i| InventoryDefinitionV1 {
                key: format!("bag_{i}"),
                initial_slots: 0,
                force_add_when_zero: false,
                overflow_limit: 0,
                slot_rules: vec![
                    SlotRuleV1 {
                        allowed_items: keys.clone()
                    };
                    129
                ],
            })
            .collect();
    }
    let original = CompiledInventoryContractsV1::compile(d.clone(), &cat).unwrap();
    for text in [
        serde_json::to_string(&d).unwrap(),
        toml::to_string(&d).unwrap(),
    ] {
        let reloaded = CompiledInventoryContractsV1::parse(&text, &cat).unwrap();
        assert_eq!(original.identity(), reloaded.identity());
        let definition = reloaded.definition();
        assert_eq!(definition.items.capacity(), 256);
        assert_eq!(definition.classes.capacity(), 4);
        assert_eq!(definition.classes[0].inventories.capacity(), 8);
        let inventory = &definition.classes[0].inventories[0];
        assert_eq!(inventory.slot_rules.capacity(), 256);
        assert_eq!(inventory.slot_rules[0].allowed_items.capacity(), 256);
        let p = prepare(&reloaded, vec![]);
        let state =
            InventoryStateSnapshotV1::parse(&serde_json::to_string(p.state()).unwrap()).unwrap();
        assert_eq!(&state, p.state());
        assert_eq!(state.consumed.capacity(), 256);
        assert_eq!(state.owners[0].inventories.capacity(), 8);
        let totals: InventoryInitialTotalsV1 =
            serde_json::from_str(&serde_json::to_string(p.initial_totals()).unwrap()).unwrap();
        assert_eq!(&totals, p.initial_totals());
        assert_eq!(totals.items.capacity(), 256);
        validate_inventory_state(&reloaded, &state, &totals, &live()).unwrap();
    }
}

#[test]
fn initial_item_and_deferred_stack_vectors_reload_across_growth_thresholds() {
    let cat = catalog_with_owners(2);
    let mut d = wide_definition(&cat, 1);
    for class in &mut d.classes {
        class.inventories[0].overflow_limit = 256;
    }
    let c = CompiledInventoryContractsV1::compile(d, &cat).unwrap();
    let items = InitialInventoryItemsV1 {
        items: (0..130).map(|_| item(1, "bag", 0, "apple", 1)).collect(),
    };
    let decoded_items: InitialInventoryItemsV1 =
        serde_json::from_str(&serde_json::to_string(&items).unwrap()).unwrap();
    assert_eq!(decoded_items, items);
    assert_eq!(decoded_items.items.capacity(), 256);
    let p = prepare_initial_inventory(&c, &live(), &decoded_items).unwrap();
    assert_eq!(inv(p.state(), 1, "bag").overflow.len(), 129);
    let state =
        InventoryStateSnapshotV1::parse(&serde_json::to_string(p.state()).unwrap()).unwrap();
    assert_eq!(&state, p.state());
    assert_eq!(inv(&state, 1, "bag").overflow.capacity(), 256);
    validate_inventory_state(&c, &state, p.initial_totals(), &live()).unwrap();
}

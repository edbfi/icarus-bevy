//! Authored source -> parts and immutable provenance only. No runtime/store claim.
#[path = "common/c001_inventory_owners.rs"]
mod authored;
#[path = "common/c001_scalar_owners.rs"]
mod scalar;
#[path = "common/c001_inventory_source.rs"]
mod source;
use authored::{Fixture, fixture};
use icarus_bevy::{
    import_candidate::DecodedImportCandidate,
    native_c001_inventory_owners::*,
    native_inventory::*,
    native_lifecycle::{OwnerHandle, RestoreDisposition},
    simulation::DomainId,
};
use scalar::{Tag, units};
use serde_json::{Value, json};
fn rejected(f: &mut Fixture) {
    f.rebuild_source();
    if let Ok(candidate) = DecodedImportCandidate::decode(f.envelope.as_slice()) {
        assert!(
            prepare_c001_inventory_owner_parts(
                &candidate,
                &f.base,
                &f.bindings,
                &f.scalar_contracts,
                &f.inventory_contracts
            )
            .is_err()
        );
    }
}
fn inventory_tag(f: &mut Fixture, owner: usize, tag: Tag) {
    *f.owners[owner]
        .iter_mut()
        .find(|v| v.name == "AuthoredInventories")
        .unwrap() = tag;
}
fn validate(
    f: &Fixture,
    p: &PreparedC001InventoryOwnerParts,
    m: &C001InventoryOwnerImportMetadataV1,
    state: &InventoryStateSnapshotV1,
) -> anyhow::Result<()> {
    m.validate(
        p.native(),
        p.lifecycle(),
        &f.scalar_contracts.snapshot(),
        &f.inventory_contracts.snapshot(),
        state,
        f.candidate().preserved(),
    )
}
#[test]
fn complete_source_parts_routes_and_deterministic_source_order_ids() {
    let f = fixture();
    let candidate = f.candidate();
    let bytes = candidate.preserved().native_encoding().to_vec();
    let p = f.prepared_parts();
    let m = p.metadata();
    assert_eq!(p.lifecycle().restore_sequence, 1);
    assert_eq!(p.inventories().revision, 1);
    assert_eq!(p.inventories().next_stack_id, 5);
    assert_eq!(m.inventory_initial.initial_next_stack_id, 5);
    assert_eq!(
        m.resolved.iter().map(|r| r.id).collect::<Vec<_>>(),
        vec![DomainId(3), DomainId(1), DomainId(2)]
    );
    let outcomes = &m.initial_completion.outcomes;
    assert_eq!(
        outcomes
            .iter()
            .filter(|v| v.disposition == RestoreDisposition::Attached)
            .count(),
        1
    );
    assert_eq!(
        outcomes
            .iter()
            .filter(|v| v.disposition == RestoreDisposition::Spawned)
            .count(),
        1
    );
    assert_eq!(
        outcomes
            .iter()
            .filter(|v| v.disposition == RestoreDisposition::Replaced)
            .count(),
        1
    );
    let zulu = p
        .inventories()
        .owners
        .iter()
        .find(|o| o.owner.id == DomainId(3))
        .unwrap();
    let zero = zulu.inventories.iter().find(|i| i.key == "zero").unwrap();
    assert_eq!(zero.slots.len(), 1);
    assert_eq!(zero.slots[0].as_ref().unwrap().id.get(), 1);
    assert_eq!(zero.slots[0].as_ref().unwrap().count, 3);
    assert_eq!(zero.overflow[0].stack.id.get(), 2);
    assert_eq!(zero.overflow[0].requested_slot, 0);
    let bag = zulu.inventories.iter().find(|i| i.key == "bag").unwrap();
    assert_eq!(bag.slots[0].as_ref().unwrap().id.get(), 3);
    let middle = p
        .inventories()
        .owners
        .iter()
        .find(|o| o.owner.id == DomainId(2))
        .unwrap();
    assert_eq!(
        middle
            .inventories
            .iter()
            .find(|i| i.key == "bag")
            .unwrap()
            .slots[0]
            .as_ref()
            .unwrap()
            .id
            .get(),
        4
    );
    let alpha = p
        .inventories()
        .owners
        .iter()
        .find(|o| o.owner.id == DomainId(1))
        .unwrap();
    assert!(
        alpha
            .inventories
            .iter()
            .all(|i| i.slots.iter().all(Option::is_none) && i.overflow.is_empty())
    );
    for (id, enabled, uses, incarnation) in [(1, false, 2, 1), (2, true, 2, 2), (3, true, 1, 1)] {
        let actor = p
            .native()
            .actors
            .iter()
            .find(|a| a.id == DomainId(id))
            .unwrap();
        assert_eq!(actor.variables, scalar::scalar_pair(enabled, uses));
        assert_eq!(
            p.lifecycle().slots[id as usize - 1].incarnation,
            incarnation
        );
    }
    validate(&f, &p, m, p.inventories()).unwrap();
    assert_eq!(bytes, candidate.preserved().native_encoding());
    assert_eq!(
        f.base.native_lifecycle.as_ref().unwrap().restore_sequence,
        0
    );
}
#[test]
fn declaration_canonical_json_toml_and_source_order_are_separate() {
    let mut f = fixture();
    let a = f.prepared_parts();
    let mut d = f.bindings.definition().clone();
    d.items.reverse();
    d.owners.reverse();
    for o in &mut d.owners {
        o.inventories.reverse();
    }
    let b = CompiledC001InventoryOwnerBindings::compile(d).unwrap();
    assert_eq!(f.bindings.identity(), b.identity());
    assert_eq!(
        b.identity(),
        CompiledC001InventoryOwnerBindings::parse(&serde_json::to_string(b.definition()).unwrap())
            .unwrap()
            .identity()
    );
    assert_eq!(
        b.identity(),
        CompiledC001InventoryOwnerBindings::parse(&toml::to_string(b.definition()).unwrap())
            .unwrap()
            .identity()
    );
    f.bindings = b;
    let c = f.prepared_parts();
    assert_eq!(a.inventories(), c.inventories());
    assert_eq!(a.metadata(), c.metadata());
    let encoded = serde_json::to_string(a.metadata()).unwrap();
    let decoded: C001InventoryOwnerImportMetadataV1 = serde_json::from_str(&encoded).unwrap();
    validate(&f, &a, &decoded, a.inventories()).unwrap();
}
#[test]
fn last_recorder_and_unsupported_nested_state_reject_whole_preparation() {
    for change in 0..8 {
        let mut f = fixture();
        let before = serde_json::to_vec(&f.base).unwrap();
        let row = vec![0xd804, 74];
        let mut item = source::item_fields(&row, 0, 2);
        match change {
            0 => f.owners[2].push(source::empty_unknown()),
            1 => {
                item.push(source::empty_unknown());
                inventory_tag(
                    &mut f,
                    2,
                    source::inventory_array(&[
                        source::inventory_fields(&units("SourceBag"), &[item]),
                        source::inventory_fields(&units("SourceZero"), &[]),
                    ]),
                );
            }
            2 => {
                source::replace(
                    &mut item,
                    "ItemGuid",
                    Tag::text("ItemGuid", "StrProperty", &units("PrivateSentinel")),
                );
                inventory_tag(
                    &mut f,
                    2,
                    source::inventory_array(&[
                        source::inventory_fields(&units("SourceBag"), &[item]),
                        source::inventory_fields(&units("SourceZero"), &[]),
                    ]),
                );
            }
            3 => {
                source::field(&mut item, "Location").index = 1;
                inventory_tag(
                    &mut f,
                    2,
                    source::inventory_array(&[
                        source::inventory_fields(&units("SourceBag"), &[item]),
                        source::inventory_fields(&units("SourceZero"), &[]),
                    ]),
                );
            }
            4 => {
                source::replace(
                    &mut item,
                    "ItemOwnerLookupId",
                    Tag::integer("ItemOwnerLookupId", 0),
                );
                inventory_tag(
                    &mut f,
                    2,
                    source::inventory_array(&[
                        source::inventory_fields(&units("SourceBag"), &[item]),
                        source::inventory_fields(&units("SourceZero"), &[]),
                    ]),
                );
            }
            5 => {
                let duplicate = f.owners[2][0].clone();
                f.owners[2].push(duplicate);
            }
            6 => {
                source::field(&mut f.owners[2], "ActorPathName").payload =
                    scalar::string("/Unmapped/PrivateSentinel")
            }
            _ => {
                source::field(&mut f.owners[2], "ActorTransform").struct_guid();
            }
        }
        rejected(&mut f);
        assert_eq!(
            serde_json::to_vec(&f.base).unwrap(),
            before,
            "case {change}"
        );
    }
}
#[test]
fn complete_empty_inventory_coverage_source_alias_and_unmapped_rows_reject() {
    for change in 0..5 {
        let mut f = fixture();
        let inventories = match change {
            0 => vec![source::inventory_fields(&units("SourceZero"), &[])],
            1 => vec![
                source::inventory_fields(&units("SourceBag"), &[]),
                source::inventory_fields(&units("Unknown"), &[]),
            ],
            2 => vec![
                source::inventory_fields(&units("SourceBag"), &[]),
                source::inventory_fields(&units("SourceBag"), &[]),
            ],
            3 => vec![],
            _ => vec![
                source::inventory_fields(
                    &units("SourceBag"),
                    &[source::item_fields(&units("UnmappedRow"), 0, 1)],
                ),
                source::inventory_fields(&units("SourceZero"), &[]),
            ],
        };
        inventory_tag(&mut f, 1, source::inventory_array(&inventories));
        rejected(&mut f);
    }
    let f = fixture();
    let mut d = f.bindings.definition().clone();
    d.owners[0].inventories[1].inventory = d.owners[0].inventories[0].inventory.clone();
    assert!(CompiledC001InventoryOwnerBindings::compile(d).is_err());
    let mut d = f.bindings.definition().clone();
    d.items[1].item = d.items[0].item.clone();
    assert!(CompiledC001InventoryOwnerBindings::compile(d).is_err());
    let mut d = f.bindings.definition().clone();
    d.items.pop();
    let b = CompiledC001InventoryOwnerBindings::compile(d).unwrap();
    assert!(
        prepare_c001_inventory_owner_parts(
            &f.candidate(),
            &f.base,
            &b,
            &f.scalar_contracts,
            &f.inventory_contracts
        )
        .is_err()
    );
}
#[test]
fn immutable_metadata_receipt_completion_and_source_tampering_fail() {
    let f = fixture();
    let p = f.prepared_parts();
    for change in 0..8 {
        let mut m = p.metadata().clone();
        match change {
            0 => m.source_prospect_id.push(1),
            1 => {
                m.resolved[0].source_policy =
                    icarus_bevy::native_lifecycle::ResolvePolicy::RespawnOnly
            }
            2 => m.inventory_initial.initial_next_stack_id += 1,
            3 => m.inventory_initial.initial_totals.items[0].count += 1,
            4 => m.inventory_initial.initial_state_digest = "b".repeat(64),
            5 => m.initial_completion.request_digest = "a".repeat(64),
            6 => m.bindings.owners[0].scalar.owner.source_object_name.push(2),
            _ => m.bindings_id = "f".repeat(64),
        }
        assert!(
            validate(&f, &p, &m, p.inventories()).is_err(),
            "case {change}"
        );
    }
    let mut changed = fixture();
    source::field(&mut changed.owners[2], "IcarusActorGUID").payload =
        999_i32.to_le_bytes().to_vec();
    changed.rebuild_source();
    assert!(
        p.metadata()
            .validate(
                p.native(),
                p.lifecycle(),
                &f.scalar_contracts.snapshot(),
                &f.inventory_contracts.snapshot(),
                p.inventories(),
                changed.candidate().preserved()
            )
            .is_err()
    );
}
#[test]
fn mutable_full_consumption_and_cross_owner_transfer_pass_provenance_replay() {
    let f = fixture();
    let p = f.prepared_parts();
    let live = InventoryLiveOwnersV1 {
        owners: p
            .lifecycle()
            .slots
            .iter()
            .map(|s| OwnerHandle {
                id: s.id,
                incarnation: s.incarnation,
            })
            .collect(),
    };
    let handle = |id| *live.owners.iter().find(|h| h.id == DomainId(id)).unwrap();
    let consumed = prepare_inventory_action(
        &f.inventory_contracts,
        p.inventories(),
        &p.metadata().inventory_initial.initial_totals,
        &live,
        &InventoryActionRequestV1 {
            expected_inventory_revision: 1,
            action: InventoryActionV1::Consume {
                source: InventoryStackEndpointV1 {
                    owner: handle(3),
                    inventory: "bag".into(),
                    stack_id: StackId::new(3).unwrap(),
                },
                amount: 32,
            },
        },
    )
    .unwrap()
    .into_state();
    let transferred = prepare_inventory_action(
        &f.inventory_contracts,
        &consumed,
        &p.metadata().inventory_initial.initial_totals,
        &live,
        &InventoryActionRequestV1 {
            expected_inventory_revision: 2,
            action: InventoryActionV1::Move {
                source: InventoryStackEndpointV1 {
                    owner: handle(2),
                    inventory: "bag".into(),
                    stack_id: StackId::new(4).unwrap(),
                },
                destination: InventorySlotEndpointV1 {
                    owner: handle(1),
                    inventory: "bag".into(),
                    slot: 0,
                },
            },
        },
    )
    .unwrap()
    .into_state();
    assert_eq!(transferred.revision, 3);
    validate(&f, &p, p.metadata(), &transferred).unwrap();
    let owner = transferred
        .owners
        .iter()
        .find(|o| o.owner.id == DomainId(1))
        .unwrap();
    assert_eq!(
        owner
            .inventories
            .iter()
            .find(|i| i.key == "bag")
            .unwrap()
            .slots[0]
            .as_ref()
            .unwrap()
            .id
            .get(),
        4
    );
    assert_eq!(transferred.next_stack_id, p.inventories().next_stack_id);
    let mut corrupt = transferred.clone();
    corrupt.consumed[0].count += 1;
    assert!(validate(&f, &p, p.metadata(), &corrupt).is_err());
}
#[test]
fn strict_objects_names_capacity_input_limits_and_debug_are_redacted() {
    let f = fixture();
    let d = f.bindings.definition();
    let text = serde_json::to_string(d).unwrap();
    let duplicate = text.replacen("\"format\":1", "\"format\":1,\"format\":1", 1);
    assert!(CompiledC001InventoryOwnerBindings::parse(&duplicate).is_err());
    let mut j = serde_json::to_value(d).unwrap();
    j["items"][0]["private_sentinel"] = json!([]);
    assert!(
        !CompiledC001InventoryOwnerBindings::parse(&j.to_string())
            .unwrap_err()
            .to_string()
            .contains("private_sentinel")
    );
    let mut j = serde_json::to_value(d).unwrap();
    j["owners"][0] = Value::Array(vec![]);
    assert!(CompiledC001InventoryOwnerBindings::parse(&j.to_string()).is_err());
    let mut j = serde_json::to_value(d).unwrap();
    j["source_map"] = json!(vec![65_u16; 1025]);
    assert!(CompiledC001InventoryOwnerBindings::parse(&j.to_string()).is_err());
    let mut too_large = d.clone();
    too_large.items[0].source_row = Vec::with_capacity(2051);
    too_large.items[0].source_row.push(65);
    assert!(CompiledC001InventoryOwnerBindings::compile(too_large).is_err());
    let mut too_large = d.clone();
    too_large.owners[0].inventories = Vec::with_capacity(21);
    assert!(CompiledC001InventoryOwnerBindings::compile(too_large).is_err());
    assert!(
        CompiledC001InventoryOwnerBindings::parse(
            &" ".repeat(MAX_C001_INVENTORY_OWNER_BINDINGS_BYTES + 1)
        )
        .is_err()
    );
    let debug = format!("{:?}", f.prepared_parts().metadata());
    assert!(!debug.contains("SourceObject"));
    assert!(!debug.contains(&d.expected_payload_sha256));
    assert!(!format!("{:?}", f.bindings).contains(&d.expected_payload_sha256));
}

#[test]
fn source_occurrence_reordering_changes_ids_even_when_declarations_do_not() {
    let mut f = fixture();
    let original = f.prepared_parts();
    f.owners.swap(0, 2);
    f.rebuild_source();
    let changed = f.prepared_parts();
    assert_eq!(
        changed.metadata().inventory_initial.initial_totals,
        original.metadata().inventory_initial.initial_totals
    );
    let middle = changed
        .inventories()
        .owners
        .iter()
        .find(|o| o.owner.id == DomainId(2))
        .unwrap();
    assert_eq!(
        middle
            .inventories
            .iter()
            .find(|i| i.key == "bag")
            .unwrap()
            .slots[0]
            .as_ref()
            .unwrap()
            .id
            .get(),
        1
    );
    assert_ne!(
        changed.metadata().inventory_initial.initial_state_digest,
        original.metadata().inventory_initial.initial_state_digest
    );
    assert_eq!(changed.inventories(), f.prepared_parts().inventories());
    validate(&f, &changed, changed.metadata(), changed.inventories()).unwrap();
}

#[test]
fn final_inventory_failure_or_mismatched_contract_exposes_no_parts_and_preserves_base() {
    for change in 0..3 {
        let mut f = fixture();
        let before = serde_json::to_vec(&f.base).unwrap();
        let mut definition = f.inventory_contracts.definition().clone();
        match change {
            0 => {
                definition.classes[0]
                    .inventories
                    .iter_mut()
                    .find(|i| i.key == "zero")
                    .unwrap()
                    .force_add_when_zero = false
            }
            1 => {
                definition.classes[0]
                    .inventories
                    .iter_mut()
                    .find(|i| i.key == "zero")
                    .unwrap()
                    .overflow_limit = 0
            }
            _ => {
                definition
                    .items
                    .iter_mut()
                    .find(|i| i.key == "stone")
                    .unwrap()
                    .stack_limit = 4
            }
        }
        f.inventory_contracts =
            CompiledInventoryContractsV1::compile(definition, &f.catalog).unwrap();
        let mut bindings = f.bindings.definition().clone();
        bindings.native_inventory_contract_id = f.inventory_contracts.identity().into();
        f.bindings = CompiledC001InventoryOwnerBindings::compile(bindings).unwrap();
        assert!(
            prepare_c001_inventory_owner_parts(
                &f.candidate(),
                &f.base,
                &f.bindings,
                &f.scalar_contracts,
                &f.inventory_contracts
            )
            .is_err()
        );
        assert_eq!(serde_json::to_vec(&f.base).unwrap(), before);
    }
}

#[test]
fn standalone_metadata_runs_final_shape_checks_and_resets_failed_name_quota() {
    let f = fixture();
    let p = f.prepared_parts();
    let good = serde_json::to_string(p.metadata()).unwrap();
    let decoded: C001InventoryOwnerImportMetadataV1 = serde_json::from_str(&good).unwrap();
    assert_eq!(decoded, *p.metadata());
    for case in 0..6 {
        let mut j = serde_json::to_value(p.metadata()).unwrap();
        match case {
            0 => j["format"] = json!(0),
            1 => j["source_version"] = json!(3),
            2 => j["resolved"] = json!([]),
            3 => j["inventory_initial"]["initial_next_stack_id"] = json!(0),
            4 => j["bindings"]["scalar_layout"]["enabled"]["array_name"] = json!(vec![65; 1024]),
            _ => j["source_prospect_id"] = json!(vec![65; 1025]),
        }
        j["source_lobby_name"] = json!([112, 114, 105, 118, 97, 116, 101]);
        let error =
            serde_json::from_str::<C001InventoryOwnerImportMetadataV1>(&j.to_string()).unwrap_err();
        assert!(!error.to_string().contains("private"));
        assert!(
            serde_json::from_str::<C001InventoryOwnerImportMetadataV1>(&good).is_ok(),
            "case {case}"
        );
    }
    let mut too_large = p.metadata().clone();
    let mut digest = String::with_capacity(MAX_INVENTORY_GRAPH_BYTES);
    digest.push_str(&too_large.bindings_id);
    too_large.bindings_id = digest;
    let error = validate(&f, &p, &too_large, p.inventories()).unwrap_err();
    assert_eq!(error.to_string(), "scalar owner digest invalid");
    // The capacity attack is not a different persisted identity.
    assert_eq!(serde_json::to_string(&too_large).unwrap(), good);
}

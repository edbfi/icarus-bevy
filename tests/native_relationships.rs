//! Authored fixtures only: these tests establish the native relationship
//! contract, not a save adapter or original-game compatibility.
use std::{fs, sync::Arc};

use icarus_bevy::{
    native_relationships::{
        CompiledNativeRelationships, MAX_NATIVE_LINK_NAME_UNITS, MAX_NATIVE_RELATIONSHIP_BYTES,
        MAX_NATIVE_RELATIONSHIP_LINK_UNITS, NativeActorLinkV1, NativeFoundationRecord,
        NativeRelationshipActorV1, NativeRelationshipRole, NativeRelationshipsDefinition,
        NativeRelationshipsSnapshot,
    },
    native_world::{
        Aabb3, CompiledNativeCatalog, MAX_NATIVE_PLACEMENTS, NativeCatalogDefinition,
        NativeClassDefinition, NativeClassKind, NativeFrame, NativeMapDefinition, NativePlacement,
        NativeTileDefinition, NativeTransform, NativeWorldSnapshot, prepare_authored_world,
    },
    simulation::DomainId,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

fn catalog_with_keys(keys: impl IntoIterator<Item = String>) -> CompiledNativeCatalog {
    CompiledNativeCatalog::compile(NativeCatalogDefinition {
        native_format: 1,
        frame: NativeFrame::RightHandedZUpMeters,
        classes: vec![NativeClassDefinition {
            key: "authored_box".into(),
            kind: NativeClassKind::SpatialObjectV1,
            local_bounds: Aabb3 {
                min: [-0.5; 3],
                max: [0.5; 3],
            },
            variables: vec![],
        }],
        maps: vec![NativeMapDefinition {
            key: "authored_map".into(),
            tiles: vec![NativeTileDefinition {
                key: "authored_tile".into(),
                bounds: Aabb3 {
                    min: [-10.0; 3],
                    max: [10.0; 3],
                },
            }],
            placements: keys
                .into_iter()
                .map(|key| NativePlacement {
                    key,
                    class: "authored_box".into(),
                    tile: "authored_tile".into(),
                    transform: NativeTransform::default(),
                    initial_values: vec![],
                })
                .collect(),
        }],
    })
    .unwrap()
}

fn catalog() -> CompiledNativeCatalog {
    // Reverse geometry declarations as well: the catalog supplies canonical IDs.
    catalog_with_keys(
        [
            "e_spatial",
            "d_foundation",
            "c_foundation",
            "b_deployable",
            "a_deployable",
        ]
        .map(str::to_owned),
    )
}

fn link(name: &str, guid: i32) -> NativeActorLinkV1 {
    NativeActorLinkV1 {
        name: name.encode_utf16().collect(),
        guid,
    }
}

fn actor(
    placement: &str,
    identity: NativeActorLinkV1,
    role: NativeRelationshipRole,
) -> NativeRelationshipActorV1 {
    NativeRelationshipActorV1 {
        placement: placement.into(),
        identity,
        role,
        foundation: None,
    }
}

fn definition(catalog: &CompiledNativeCatalog) -> NativeRelationshipsDefinition {
    let mut deployable = actor(
        "a_deployable",
        link("Device", 1),
        NativeRelationshipRole::Deployable,
    );
    deployable.foundation = Some(link("FOUNDATION_A", 3));
    NativeRelationshipsDefinition {
        relationship_format: 1,
        native_catalog_id: catalog.identity().into(),
        map: "authored_map".into(),
        actors: vec![
            deployable,
            actor(
                "b_deployable",
                link("Device", 2),
                NativeRelationshipRole::Deployable,
            ),
            actor(
                "c_foundation",
                link("Foundation_A", 3),
                NativeRelationshipRole::Foundation,
            ),
            actor(
                "d_foundation",
                link("Foundation_B", 4),
                NativeRelationshipRole::Foundation,
            ),
            // Sharing a native class is deliberate: roles are authored capabilities.
            actor(
                "e_spatial",
                link("Foundation_A", 5),
                NativeRelationshipRole::Spatial,
            ),
        ],
    }
}

fn compiled() -> CompiledNativeRelationships {
    let catalog = catalog();
    CompiledNativeRelationships::compile(definition(&catalog), &catalog).unwrap()
}

fn native(catalog: &CompiledNativeCatalog) -> NativeWorldSnapshot {
    prepare_authored_world(Arc::new(catalog.clone()), "authored_map")
        .unwrap()
        .snapshot()
        .clone()
}

#[test]
fn deployable_before_foundation_and_reordering_have_canonical_identity_and_state() {
    let catalog = catalog();
    let original = CompiledNativeRelationships::compile(definition(&catalog), &catalog).unwrap();
    let mut reversed = definition(&catalog);
    reversed.actors.reverse();
    let reordered = CompiledNativeRelationships::compile(reversed, &catalog).unwrap();
    assert_eq!(original.identity(), reordered.identity());
    assert_eq!(original.definition(), reordered.definition());
    assert_eq!(original.initial_snapshot(), reordered.initial_snapshot());
    let expected: String = Sha256::digest(serde_json::to_vec(original.definition()).unwrap())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    assert_eq!(original.identity(), expected);
    assert_eq!(
        original.initial_snapshot().foundations,
        vec![
            NativeFoundationRecord {
                owner: DomainId(1),
                foundation: Some(DomainId(3))
            },
            NativeFoundationRecord {
                owner: DomainId(2),
                foundation: None
            },
        ]
    );
    assert_ne!(
        original.definition().actors[2].identity.name,
        catalog.map("authored_map").unwrap().placements[2]
            .class
            .encode_utf16()
            .collect::<Vec<_>>()
    );
}

#[test]
fn matching_requires_guid_and_name_and_folds_only_ascii() {
    let catalog = catalog();
    for wrong in [link("WrongName", 3), link("Foundation_A", 900)] {
        let mut d = definition(&catalog);
        d.actors[0].foundation = Some(wrong);
        assert!(CompiledNativeRelationships::compile(d, &catalog).is_err());
    }
    let mut d = definition(&catalog);
    d.actors[0].foundation = Some(link("fOuNdAtIoN_a", 3));
    CompiledNativeRelationships::compile(d, &catalog).unwrap();
    let mut d = definition(&catalog);
    d.actors[2].identity.name = "Étage".encode_utf16().collect();
    d.actors[0].foundation = Some(link("étage", 3));
    assert!(CompiledNativeRelationships::compile(d.clone(), &catalog).is_err());
    d.actors[0].foundation = Some(link("ÉTAGE", 3));
    CompiledNativeRelationships::compile(d, &catalog).unwrap();
    let mut d = definition(&catalog);
    d.actors[2].identity.name = vec![0xd800, 0, 0xdc00, u16::from(b'A')];
    d.actors[0].foundation = Some(NativeActorLinkV1 {
        name: vec![0xd800, 0, 0xdc00, u16::from(b'a')],
        guid: 3,
    });
    let exact = CompiledNativeRelationships::compile(d.clone(), &catalog).unwrap();
    assert_eq!(
        exact.definition().actors[2].identity.name,
        d.actors[2].identity.name
    );
    let text = serde_json::to_string(exact.definition()).unwrap();
    assert_eq!(
        CompiledNativeRelationships::parse(&text, &catalog)
            .unwrap()
            .definition(),
        exact.definition()
    );
    d.actors[0].foundation.as_mut().unwrap().name[0] = 0xd801;
    assert!(CompiledNativeRelationships::compile(d, &catalog).is_err());
}

#[test]
fn authored_roles_are_exclusive_and_both_link_ends_are_checked() {
    let catalog = catalog();
    for owner_role in [
        NativeRelationshipRole::Spatial,
        NativeRelationshipRole::Foundation,
    ] {
        let mut d = definition(&catalog);
        d.actors[0].role = owner_role;
        assert!(CompiledNativeRelationships::compile(d, &catalog).is_err());
    }
    for target_role in [
        NativeRelationshipRole::Spatial,
        NativeRelationshipRole::Deployable,
    ] {
        let mut d = definition(&catalog);
        d.actors[2].role = target_role;
        assert!(CompiledNativeRelationships::compile(d, &catalog).is_err());
    }
    let mut d = definition(&catalog);
    d.actors[0].foundation = Some(link("Foundation_A", 5));
    assert!(CompiledNativeRelationships::compile(d, &catalog).is_err());
}

#[test]
fn duplicate_qualified_identities_are_rejected_before_link_resolution() {
    let catalog = catalog();
    for duplicate in [link("Device", 1), link("dEvIcE", 1)] {
        let mut d = definition(&catalog);
        d.actors[1].identity = duplicate;
        let error = CompiledNativeRelationships::compile(d, &catalog).unwrap_err();
        assert!(error.to_string().contains("ambiguous"));
    }
    // Duplicates are prohibited even when no initial link refers to them.
    let mut d = definition(&catalog);
    d.actors[3].identity = link("FOUNDATION_A", 3);
    d.actors[0].foundation = Some(link("Unresolved", 99));
    assert!(
        CompiledNativeRelationships::compile(d, &catalog)
            .unwrap_err()
            .to_string()
            .contains("ambiguous")
    );
    // The pair is the identity: same GUID with different names is allowed.
    let mut d = definition(&catalog);
    d.actors[3].identity.guid = 3;
    CompiledNativeRelationships::compile(d, &catalog).unwrap();
}

#[test]
fn every_placement_must_bind_once_and_invalid_last_link_rejects_everything() {
    let catalog = catalog();
    let mut missing = definition(&catalog);
    missing.actors.pop();
    assert!(CompiledNativeRelationships::compile(missing, &catalog).is_err());
    let mut duplicate = definition(&catalog);
    duplicate.actors[4].placement = duplicate.actors[3].placement.clone();
    assert!(CompiledNativeRelationships::compile(duplicate, &catalog).is_err());
    let mut dangling = definition(&catalog);
    dangling.actors[4].placement = "not_a_placement".into();
    assert!(CompiledNativeRelationships::compile(dangling, &catalog).is_err());
    let mut extra = definition(&catalog);
    extra.actors.push(actor(
        "extra",
        link("Extra", 6),
        NativeRelationshipRole::Spatial,
    ));
    assert!(CompiledNativeRelationships::compile(extra, &catalog).is_err());
    let mut last = definition(&catalog);
    last.actors[4].role = NativeRelationshipRole::Deployable;
    last.actors[4].foundation = Some(link("UnresolvedFinal", 999));
    assert!(CompiledNativeRelationships::compile(last, &catalog).is_err());
    // A subsequent fresh compile has exactly the declared initial state.
    assert_eq!(
        CompiledNativeRelationships::compile(definition(&catalog), &catalog)
            .unwrap()
            .initial_snapshot()
            .foundations
            .len(),
        2
    );
}

#[test]
fn reassignment_updates_forward_and_reverse_queries_without_rewriting_initial_state() {
    let compiled = compiled();
    let initial = compiled.initial_snapshot();
    let mut state = initial.clone();
    compiled
        .reassign_foundation(&mut state, DomainId(1), Some(DomainId(4)))
        .unwrap();
    assert_eq!(
        compiled.foundation_of(&state, DomainId(1)).unwrap(),
        Some(DomainId(4))
    );
    assert!(
        compiled
            .dependents_of(&state, DomainId(3))
            .unwrap()
            .is_empty()
    );
    compiled
        .reassign_foundation(&mut state, DomainId(2), Some(DomainId(4)))
        .unwrap();
    state.foundations.reverse();
    assert_eq!(
        compiled.dependents_of(&state, DomainId(4)).unwrap(),
        vec![DomainId(1), DomainId(2)]
    );
    compiled
        .reassign_foundation(&mut state, DomainId(1), None)
        .unwrap();
    assert_eq!(compiled.foundation_of(&state, DomainId(1)).unwrap(), None);
    assert_eq!(
        compiled.dependents_of(&state, DomainId(4)).unwrap(),
        vec![DomainId(2)]
    );
    assert_eq!(state.definition, initial.definition);
    assert_eq!(state.relationship_id, initial.relationship_id);
    assert_eq!(compiled.initial_snapshot(), initial);
}

#[test]
fn failed_edits_are_transactional_and_corrupt_state_requires_repair_before_retry() {
    let compiled = compiled();
    let mut state = compiled.initial_snapshot();
    for (owner, target) in [
        (DomainId(3), None),
        (DomainId(5), None),
        (DomainId(0), None),
        (DomainId(99), Some(DomainId(3))),
        (DomainId(1), Some(DomainId(1))),
        (DomainId(1), Some(DomainId(5))),
        (DomainId(1), Some(DomainId(99))),
    ] {
        let before = state.clone();
        assert!(
            compiled
                .reassign_foundation(&mut state, owner, target)
                .is_err()
        );
        assert_eq!(state, before);
    }
    state.foundations[1].foundation = Some(DomainId(99));
    let corrupt = state.clone();
    assert!(
        compiled
            .reassign_foundation(&mut state, DomainId(1), None)
            .is_err()
    );
    assert_eq!(state, corrupt);
    assert!(compiled.foundation_of(&state, DomainId(1)).is_err());
    assert!(compiled.dependents_of(&state, DomainId(3)).is_err());
    state.foundations[1].foundation = None;
    compiled
        .reassign_foundation(&mut state, DomainId(1), None)
        .unwrap();
    assert_eq!(compiled.foundation_of(&state, DomainId(1)).unwrap(), None);
    for owner in [DomainId(3), DomainId(5), DomainId(99)] {
        assert!(compiled.foundation_of(&state, owner).is_err());
    }
    for target in [DomainId(1), DomainId(5), DomainId(99)] {
        assert!(compiled.dependents_of(&state, target).is_err());
    }
}

#[test]
fn edited_edges_survive_serialization_and_snapshot_reload() {
    let catalog = catalog();
    let compiled = CompiledNativeRelationships::compile(definition(&catalog), &catalog).unwrap();
    let mut state = compiled.initial_snapshot();
    compiled
        .reassign_foundation(&mut state, DomainId(1), None)
        .unwrap();
    compiled
        .reassign_foundation(&mut state, DomainId(2), Some(DomainId(4)))
        .unwrap();
    state.foundations.reverse();
    state.definition.actors.reverse();
    let decoded: NativeRelationshipsSnapshot =
        serde_json::from_str(&serde_json::to_string(&state).unwrap()).unwrap();
    compiled.validate_snapshot(&decoded).unwrap();
    let mut world = native(&catalog);
    world.actors.reverse();
    let reloaded = CompiledNativeRelationships::from_snapshot(&decoded, &world).unwrap();
    assert_eq!(reloaded.identity(), compiled.identity());
    assert_eq!(reloaded.foundation_of(&decoded, DomainId(1)).unwrap(), None);
    assert_eq!(
        reloaded.dependents_of(&decoded, DomainId(4)).unwrap(),
        vec![DomainId(2)]
    );
    assert_eq!(reloaded.initial_snapshot(), compiled.initial_snapshot());
}

#[test]
fn snapshot_validation_rejects_changed_definitions_and_every_invalid_edge_set() {
    let catalog = catalog();
    let compiled = CompiledNativeRelationships::compile(definition(&catalog), &catalog).unwrap();
    let state = compiled.initial_snapshot();
    let world = native(&catalog);
    let mut variants = Vec::new();
    let mut changed = state.clone();
    changed.relationship_id = "0".repeat(64);
    variants.push(changed);
    let mut changed = state.clone();
    changed.definition.native_catalog_id = "0".repeat(64);
    variants.push(changed);
    let mut changed = state.clone();
    changed.definition.map = "wrong_map".into();
    variants.push(changed);
    let mut changed = state.clone();
    changed.definition.actors[0].identity.guid = 900;
    variants.push(changed);
    let mut changed = state.clone();
    changed.definition.actors[0].foundation = None;
    variants.push(changed);
    let mut changed = state.clone();
    changed.definition.actors[1].role = NativeRelationshipRole::Spatial;
    variants.push(changed);
    let mut changed = state.clone();
    changed.definition.actors.pop();
    variants.push(changed);
    let mut changed = state.clone();
    changed.foundations.pop();
    variants.push(changed);
    let mut changed = state.clone();
    changed.foundations.push(changed.foundations[0].clone());
    variants.push(changed);
    let mut changed = state.clone();
    changed.foundations[1] = changed.foundations[0].clone();
    variants.push(changed);
    let mut changed = state.clone();
    changed.foundations[0].foundation = Some(DomainId(99));
    variants.push(changed);
    let mut changed = state.clone();
    changed.foundations[0].foundation = Some(DomainId(1));
    variants.push(changed);
    let mut changed = state.clone();
    changed.foundations[0].foundation = Some(DomainId(5));
    variants.push(changed);
    let mut changed = state.clone();
    changed.foundations[0].owner = DomainId(3);
    variants.push(changed);
    let mut changed = state.clone();
    changed.foundations[0].owner = DomainId(99);
    variants.push(changed);
    for changed in variants {
        assert!(compiled.validate_snapshot(&changed).is_err());
        assert!(CompiledNativeRelationships::from_snapshot(&changed, &world).is_err());
    }
    // A changed definition with its own valid hash is a different authored
    // contract, accepted at reload but rejected by a pinned compiled contract.
    let mut changed = definition(&catalog);
    changed.actors[0].identity.guid = 900;
    let changed = CompiledNativeRelationships::compile(changed, &catalog)
        .unwrap()
        .initial_snapshot();
    assert!(compiled.validate_snapshot(&changed).is_err());
    CompiledNativeRelationships::from_snapshot(&changed, &world).unwrap();
}

#[test]
fn reload_checks_complete_native_actor_and_domain_id_bindings() {
    let catalog = catalog();
    let state = compiled().initial_snapshot();
    let world = native(&catalog);
    let mut variants = Vec::new();
    let mut changed = world.clone();
    changed.catalog_id = "0".repeat(64);
    variants.push(changed);
    let mut changed = world.clone();
    changed.map = "wrong_map".into();
    variants.push(changed);
    let mut changed = world.clone();
    changed.actors.pop();
    variants.push(changed);
    let mut changed = world.clone();
    changed.actors[1] = changed.actors[0].clone();
    variants.push(changed);
    let mut changed = world.clone();
    changed.actors[0].id = DomainId(99);
    variants.push(changed);
    let mut changed = world.clone();
    changed.actors[0].placement = "absent".into();
    variants.push(changed);
    let mut changed = world.clone();
    changed.actors[0].class = "absent".into();
    variants.push(changed);
    let mut changed = world.clone();
    changed.actors[0].tile = "absent".into();
    variants.push(changed);
    for changed in variants {
        assert!(CompiledNativeRelationships::from_snapshot(&state, &changed).is_err());
    }
}

#[test]
fn invalid_versions_bindings_guids_and_names_reject_programmatic_inputs() {
    let catalog = catalog();
    let mut variants = Vec::new();
    let mut d = definition(&catalog);
    d.relationship_format = 2;
    variants.push(d);
    let mut d = definition(&catalog);
    d.native_catalog_id = "0".repeat(64);
    variants.push(d);
    let mut d = definition(&catalog);
    d.native_catalog_id = "A".repeat(64);
    variants.push(d);
    let mut d = definition(&catalog);
    d.map = "absent".into();
    variants.push(d);
    let mut d = definition(&catalog);
    d.map.clear();
    variants.push(d);
    let mut d = definition(&catalog);
    d.actors.clear();
    variants.push(d);
    let mut d = definition(&catalog);
    d.actors[0].placement = "bad placement".into();
    variants.push(d);
    for guid in [0, -1] {
        let mut d = definition(&catalog);
        d.actors[0].identity.guid = guid;
        variants.push(d);
        let mut d = definition(&catalog);
        d.actors[0].foundation.as_mut().unwrap().guid = guid;
        variants.push(d);
    }
    for name in [vec![], vec![1; MAX_NATIVE_LINK_NAME_UNITS + 1]] {
        let mut d = definition(&catalog);
        d.actors[0].identity.name = name.clone();
        variants.push(d);
        let mut d = definition(&catalog);
        d.actors[0].foundation.as_mut().unwrap().name = name;
        variants.push(d);
    }
    for d in variants {
        assert!(CompiledNativeRelationships::compile(d, &catalog).is_err());
    }
}

fn large_definition(
    catalog: &CompiledNativeCatalog,
    units: usize,
    unit: u16,
) -> NativeRelationshipsDefinition {
    NativeRelationshipsDefinition {
        relationship_format: 1,
        native_catalog_id: catalog.identity().into(),
        map: "authored_map".into(),
        actors: catalog
            .map("authored_map")
            .unwrap()
            .placements
            .iter()
            .enumerate()
            .map(|(rank, p)| {
                actor(
                    &p.key,
                    NativeActorLinkV1 {
                        name: vec![unit; units],
                        guid: i32::try_from(rank + 1).unwrap(),
                    },
                    NativeRelationshipRole::Spatial,
                )
            })
            .collect(),
    }
}

#[test]
fn bounded_count_aggregate_units_and_serialized_bytes_are_independent() {
    let catalog = catalog();
    let mut too_many = definition(&catalog);
    too_many.actors = vec![too_many.actors[0].clone(); MAX_NATIVE_PLACEMENTS + 1];
    assert!(
        CompiledNativeRelationships::compile(too_many, &catalog)
            .unwrap_err()
            .to_string()
            .contains("count")
    );
    assert!(
        CompiledNativeRelationships::parse(
            &" ".repeat(MAX_NATIVE_RELATIONSHIP_BYTES + 1),
            &catalog
        )
        .is_err()
    );

    let count = MAX_NATIVE_RELATIONSHIP_LINK_UNITS / MAX_NATIVE_LINK_NAME_UNITS;
    let catalog = catalog_with_keys((0..count).map(|rank| format!("p{rank:05}")));
    let d = large_definition(&catalog, MAX_NATIVE_LINK_NAME_UNITS, u16::from(b'a'));
    assert_eq!(
        d.actors
            .iter()
            .map(|a| a.identity.name.len())
            .sum::<usize>(),
        MAX_NATIVE_RELATIONSHIP_LINK_UNITS
    );
    CompiledNativeRelationships::compile(d.clone(), &catalog).unwrap();
    let mut extra_reference = d;
    extra_reference.actors[0].role = NativeRelationshipRole::Deployable;
    extra_reference.actors[1].role = NativeRelationshipRole::Foundation;
    extra_reference.actors[0].foundation = Some(extra_reference.actors[1].identity.clone());
    assert!(
        CompiledNativeRelationships::compile(extra_reference, &catalog)
            .unwrap_err()
            .to_string()
            .contains("aggregate")
    );

    // A legal unit count can still exceed the stable JSON byte budget.
    let catalog = catalog_with_keys((0..700).map(|rank| format!("p{rank:05}")));
    let oversized = large_definition(&catalog, MAX_NATIVE_LINK_NAME_UNITS, u16::MAX);
    assert!(
        oversized.actors.len() * MAX_NATIVE_LINK_NAME_UNITS < MAX_NATIVE_RELATIONSHIP_LINK_UNITS
    );
    assert!(
        CompiledNativeRelationships::compile(oversized, &catalog)
            .unwrap_err()
            .to_string()
            .contains("byte limit")
    );
}

#[test]
fn null_and_omitted_initial_links_are_explicit_no_foundation_states() {
    let catalog = catalog();
    let mut d = definition(&catalog);
    d.actors[0].foundation = None;
    let mut value = serde_json::to_value(&d).unwrap();
    value["actors"][0]
        .as_object_mut()
        .unwrap()
        .remove("foundation");
    let absent = CompiledNativeRelationships::parse(&value.to_string(), &catalog).unwrap();
    let null = CompiledNativeRelationships::compile(d, &catalog).unwrap();
    assert_eq!(absent.identity(), null.identity());
    assert_eq!(absent.initial_snapshot().foundations.len(), 2);
    assert!(
        absent
            .initial_snapshot()
            .foundations
            .iter()
            .all(|row| row.foundation.is_none())
    );
}

#[test]
fn maps_without_deployables_have_no_state_rows() {
    let catalog = catalog();
    let mut d = definition(&catalog);
    for actor in &mut d.actors {
        actor.role = NativeRelationshipRole::Spatial;
        actor.foundation = None;
    }
    let compiled = CompiledNativeRelationships::compile(d, &catalog).unwrap();
    let mut state = compiled.initial_snapshot();
    assert!(state.foundations.is_empty());
    CompiledNativeRelationships::from_snapshot(&state, &native(&catalog)).unwrap();
    state.foundations.push(NativeFoundationRecord {
        owner: DomainId(1),
        foundation: None,
    });
    assert!(compiled.validate_snapshot(&state).is_err());
}

fn replace_pointer(value: &mut Value, pointer: &str, replacement: Value) {
    *value.pointer_mut(pointer).unwrap() = replacement;
}

#[test]
fn all_named_dtos_reject_arrays_unknown_duplicate_and_missing_fields() {
    let state = compiled().initial_snapshot();
    let value = serde_json::to_value(&state).unwrap();
    let paths: &[(&str, &[&str])] = &[
        ("", &["relationship_id", "definition", "foundations"]),
        (
            "/definition",
            &["relationship_format", "native_catalog_id", "map", "actors"],
        ),
        (
            "/definition/actors/0",
            &["placement", "identity", "role", "foundation"],
        ),
        ("/definition/actors/0/identity", &["name", "guid"]),
        ("/definition/actors/0/foundation", &["name", "guid"]),
        ("/foundations/0", &["owner", "foundation"]),
    ];
    for (path, order) in paths {
        let object = value.pointer(path).unwrap().as_object().unwrap();
        let mut positional = value.clone();
        replace_pointer(
            &mut positional,
            path,
            Value::Array(order.iter().map(|key| object[*key].clone()).collect()),
        );
        assert!(serde_json::from_value::<NativeRelationshipsSnapshot>(positional).is_err());
        let mut unknown = value.clone();
        unknown
            .pointer_mut(path)
            .unwrap()
            .as_object_mut()
            .unwrap()
            .insert("secret_unknown_field".into(), json!("secret_payload"));
        let error = serde_json::from_str::<NativeRelationshipsSnapshot>(&unknown.to_string())
            .unwrap_err()
            .to_string();
        assert!(!error.contains("secret_"));
        let mut missing = value.clone();
        missing
            .pointer_mut(path)
            .unwrap()
            .as_object_mut()
            .unwrap()
            .remove(order[0]);
        assert!(serde_json::from_value::<NativeRelationshipsSnapshot>(missing).is_err());
        // Keep duplicate keys in text; Value itself would collapse duplicates.
        let key = order[0];
        let original_object = serde_json::to_string(object).unwrap();
        let duplicate_object = format!("{{{key:?}:{},{}", object[key], &original_object[1..]);
        let original_text = value.to_string();
        let duplicate_text = original_text.replacen(&original_object, &duplicate_object, 1);
        assert_ne!(original_text, duplicate_text);
        assert!(serde_json::from_str::<NativeRelationshipsSnapshot>(&duplicate_text).is_err());
    }
    // Duplicate null Options must also be rejected rather than appearing absent.
    assert!(
        serde_json::from_str::<NativeFoundationRecord>(
            r#"{"owner":1,"foundation":null,"foundation":null}"#
        )
        .is_err()
    );
    assert!(
        serde_json::from_str::<NativeActorLinkV1>(r#"{"name":[65],"guid":1,"guid":2}"#).is_err()
    );
    assert!(
        serde_json::from_str::<NativeRelationshipRole>(r#""secret_role""#)
            .unwrap_err()
            .to_string()
            .contains("role invalid")
    );
    assert!(serde_json::from_value::<NativeRelationshipRole>(json!({"spatial": null})).is_err());
}

fn relationship_toml(catalog: &CompiledNativeCatalog) -> String {
    format!(
        r#"
relationship_format = 1
native_catalog_id = "{}"
map = "authored_map"
[[actors]]
placement = "a_deployable"
role = "deployable"
identity = {{ name = [68], guid = 1 }}
foundation = {{ name = [70], guid = 3 }}
[[actors]]
placement = "b_deployable"
role = "deployable"
identity = {{ name = [68], guid = 2 }}
[[actors]]
placement = "c_foundation"
role = "foundation"
identity = {{ name = [70], guid = 3 }}
[[actors]]
placement = "d_foundation"
role = "foundation"
identity = {{ name = [70], guid = 4 }}
[[actors]]
placement = "e_spatial"
role = "spatial"
identity = {{ name = [83], guid = 5 }}
"#,
        catalog.identity()
    )
}

#[test]
fn strict_json_and_toml_share_the_same_contract_and_sanitized_errors() {
    let catalog = catalog();
    let text = relationship_toml(&catalog);
    let parsed = CompiledNativeRelationships::parse(&text, &catalog).unwrap();
    let json = serde_json::to_string(parsed.definition()).unwrap();
    assert_eq!(
        parsed.initial_snapshot(),
        CompiledNativeRelationships::parse(&json, &catalog)
            .unwrap()
            .initial_snapshot()
    );
    for invalid in [
        format!("secret_key = 'secret_value'\n{text}"),
        text.replacen(
            "relationship_format = 1",
            "relationship_format = 1\nrelationship_format = 1",
            1,
        ),
        text.replacen("role = \"deployable\"", "role = \"secret_role\"", 1),
        text.replacen(
            "identity = { name = [68], guid = 1 }",
            "identity = { name = [68], guid = 1, secret_key = 'secret_value' }",
            1,
        ),
        text.replacen(
            "identity = { name = [68], guid = 1 }",
            "identity = [[68], 1]",
            1,
        ),
        json.replacen("\"guid\":1", "\"guid\":1,\"guid\":2", 1),
    ] {
        let error = CompiledNativeRelationships::parse(&invalid, &catalog).unwrap_err();
        assert!(!format!("{error:#}").contains("secret_"));
    }
}

#[test]
fn regular_files_symlinks_limits_and_diagnostics_preserve_privacy() {
    let catalog = catalog();
    let compiled = compiled();
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("secret_relationship_path.json");
    fs::write(&path, serde_json::to_vec(compiled.definition()).unwrap()).unwrap();
    assert_eq!(
        CompiledNativeRelationships::load(&path, &catalog)
            .unwrap()
            .identity(),
        compiled.identity()
    );
    let symlink = directory.path().join("symlink");
    std::os::unix::fs::symlink(&path, &symlink).unwrap();
    CompiledNativeRelationships::load(&symlink, &catalog).unwrap();
    let fifo = directory.path().join("secret_fifo");
    rustix::fs::mkfifoat(
        rustix::fs::CWD,
        &fifo,
        rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR,
    )
    .unwrap();
    let oversized = directory.path().join("secret_oversized");
    fs::File::create(&oversized)
        .unwrap()
        .set_len(MAX_NATIVE_RELATIONSHIP_BYTES as u64 + 1)
        .unwrap();
    let invalid_utf8 = directory.path().join("secret_utf8");
    fs::write(&invalid_utf8, [0xff, 0xfe]).unwrap();
    let invalid_json = directory.path().join("secret_json");
    fs::write(&invalid_json, r#"{"secret_key":"secret_payload"}"#).unwrap();
    for rejected in [
        directory.path().to_owned(),
        fifo,
        oversized,
        invalid_utf8,
        invalid_json,
        directory.path().join("secret_absent"),
    ] {
        let error = CompiledNativeRelationships::load(&rejected, &catalog).unwrap_err();
        let diagnostic = format!("{error:#?} {error:#}");
        assert!(!diagnostic.contains("secret_"));
        assert!(!diagnostic.contains(&directory.path().display().to_string()));
    }
    let debug = format!(
        "{compiled:?} {:?} {:?} {:?} {:?} {:?}",
        compiled.definition(),
        compiled.definition().actors[0],
        compiled.definition().actors[0].identity,
        compiled.initial_snapshot(),
        compiled.initial_snapshot().foundations[0]
    );
    for secret in [
        "Device",
        "Foundation_A",
        "authored_map",
        "a_deployable",
        compiled.identity(),
    ] {
        assert!(!debug.contains(secret));
    }
}

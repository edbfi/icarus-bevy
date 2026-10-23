//! Authored fixtures only. No original owner discovery or save adapter claim.
use std::{fs, sync::Arc};

use icarus_bevy::{
    native_lifecycle::{
        CompiledNativeLifecycle, ComponentDisposition, LifecycleDefinition, LifecycleSnapshot,
        MAX_LIFECYCLE_DEFINITION_BYTES, MAX_LIFECYCLE_NAME_TOTAL_UNITS, MAX_LIFECYCLE_NAME_UNITS,
        MAX_RESTORE_BATCH_BYTES, OwnerHandle, Presence, RecorderComponentRecord, RecorderKind,
        RecorderSource, ResolvePolicy, RestoreBatch, RestoreDisposition, RestoreRecorder,
        SlotDeclaration, prepare_lifecycle_world, prepare_owner_restore,
        publish_new_lifecycle_world, validate_lifecycle_relationships,
    },
    native_relationships::{
        CompiledNativeRelationships, NativeActorLinkV1, NativeRelationshipActorV1,
        NativeRelationshipRole, NativeRelationshipsDefinition,
    },
    native_world::{
        Aabb3, CompiledNativeCatalog, MAX_NATIVE_PLACEMENTS, NativeCatalogDefinition,
        NativeClassDefinition, NativeClassKind, NativeFrame, NativeMapDefinition, NativePlacement,
        NativeTileDefinition, NativeTransform, NativeValue, VariableDefinition,
        prepare_authored_world,
    },
    persistence::SnapshotStore,
    simulation::{
        DomainId, SNAPSHOT_SCHEMA_LIFECYCLE, SNAPSHOT_SCHEMA_RELATIONSHIPS, WorldSnapshot,
    },
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use icarus_bevy::native_lifecycle::SpawnComponent;

fn catalog() -> CompiledNativeCatalog {
    CompiledNativeCatalog::compile(NativeCatalogDefinition {
        native_format: 1,
        frame: NativeFrame::RightHandedZUpMeters,
        classes: vec![NativeClassDefinition {
            key: "box".into(),
            kind: NativeClassKind::SpatialObjectV1,
            local_bounds: Aabb3 {
                min: [-0.5; 3],
                max: [0.5; 3],
            },
            variables: vec![
                VariableDefinition {
                    key: "charge".into(),
                    default: NativeValue::I32(3),
                },
                VariableDefinition {
                    key: "label".into(),
                    default: NativeValue::String("seed".into()),
                },
            ],
        }],
        maps: vec![NativeMapDefinition {
            key: "map".into(),
            tiles: vec![NativeTileDefinition {
                key: "tile".into(),
                bounds: Aabb3 {
                    min: [-10.0; 3],
                    max: [10.0; 3],
                },
            }],
            placements: ["c", "b", "a"]
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
    .unwrap()
}
fn component(guid: i32) -> RecorderComponentRecord {
    RecorderComponentRecord {
        kind: RecorderKind::SpatialStateV1,
        guid,
    }
}
fn declaration(
    placement: &str,
    name: &str,
    presence: Presence,
    guid: Option<i32>,
) -> SlotDeclaration {
    SlotDeclaration {
        placement: placement.into(),
        owner_name: name.encode_utf16().collect(),
        recorder_kind: RecorderKind::SpatialStateV1,
        initial_presence: presence,
        initial_component: guid.map(component),
        spawn_component: SpawnComponent::DeclaredKind,
    }
}
fn definition(catalog: &CompiledNativeCatalog) -> LifecycleDefinition {
    LifecycleDefinition {
        lifecycle_format: 1,
        native_catalog_id: catalog.identity().into(),
        map: "map".into(),
        slots: vec![
            declaration("c", "Ghost", Presence::Absent, None),
            declaration("b", "Beta", Presence::Live, None),
            declaration("a", "Alpha", Presence::Live, Some(10)),
        ],
    }
}
fn spatial(catalog: &CompiledNativeCatalog) -> WorldSnapshot {
    prepare_authored_world(Arc::new(catalog.clone()), "map")
        .unwrap()
        .initial_snapshot()
}
fn world_with(edit: impl FnOnce(&mut LifecycleDefinition)) -> WorldSnapshot {
    let catalog = catalog();
    let mut definition = definition(&catalog);
    edit(&mut definition);
    let compiled = CompiledNativeLifecycle::compile(definition, &catalog).unwrap();
    prepare_lifecycle_world(spatial(&catalog), &compiled)
        .unwrap()
        .into_snapshot()
}
fn world() -> WorldSnapshot {
    world_with(|_| {})
}
fn lifecycle(world: &WorldSnapshot) -> &LifecycleSnapshot {
    world.native_lifecycle.as_ref().unwrap()
}
fn index(placement: &str) -> usize {
    match placement {
        "a" => 0,
        "b" => 1,
        "c" => 2,
        _ => panic!("fixture placement"),
    }
}
fn command(
    world: &WorldSnapshot,
    placement: &str,
    guid: i32,
    policy: ResolvePolicy,
) -> RestoreRecorder {
    let rank = index(placement);
    let lifecycle = lifecycle(world);
    let native = world.native_world.as_ref().unwrap();
    RestoreRecorder {
        guid,
        kind: RecorderKind::SpatialStateV1,
        target_placement: placement.into(),
        owner_name: lifecycle.definition.slots[rank].owner_name.clone(),
        policy,
        source: RecorderSource::Detached {
            expected_prior_binding: None,
        },
        create_if_missing: false,
        expected_presence: lifecycle.slots[rank].presence,
        expected_incarnation: lifecycle.slots[rank].incarnation,
        transform: native.actors[rank].transform.clone(),
        variables: native.actors[rank].variables.clone(),
    }
}
fn batch(world: &WorldSnapshot, sequence: u64, recorders: Vec<RestoreRecorder>) -> RestoreBatch {
    RestoreBatch {
        lifecycle_id: lifecycle(world).lifecycle_id.clone(),
        sequence,
        recorders,
    }
}
fn apply(world: &WorldSnapshot, sequence: u64, recorder: RestoreRecorder) -> WorldSnapshot {
    prepare_owner_restore(world, &batch(world, sequence, vec![recorder]))
        .unwrap()
        .into_snapshot()
}

fn rejects_non_objects<T>(value: &T)
where
    T: serde::Serialize + serde::de::DeserializeOwned + std::fmt::Debug,
{
    let value = serde_json::to_value(value).unwrap();
    let array = Value::Array(value.as_object().unwrap().values().cloned().collect());
    let mut unknown = value;
    unknown["PRIVATE_UNKNOWN_FIELD"] = json!("PRIVATE_VALUE");
    for invalid in [array, unknown] {
        let error = serde_json::from_value::<T>(invalid)
            .unwrap_err()
            .to_string();
        assert!(!error.contains("PRIVATE"));
    }
}

#[test]
fn absent_slots_keep_complete_dormant_spatial_state_through_storage() {
    let catalog = catalog();
    let compiled = CompiledNativeLifecycle::compile(definition(&catalog), &catalog).unwrap();
    let mut base = spatial(&catalog);
    base.native_world.as_mut().unwrap().actors[2]
        .transform
        .translation[0] = 4.0;
    base.native_world.as_mut().unwrap().actors[2].variables[0].value = NativeValue::I32(999);
    let prepared = prepare_lifecycle_world(base, &compiled).unwrap();
    assert_eq!(
        prepared.snapshot().schema_version,
        SNAPSHOT_SCHEMA_LIFECYCLE
    );
    assert_eq!(
        prepared
            .snapshot()
            .native_world
            .as_ref()
            .unwrap()
            .actors
            .len(),
        3
    );
    assert_eq!(
        lifecycle(prepared.snapshot()).slots[2].presence,
        Presence::Absent
    );
    assert_eq!(lifecycle(prepared.snapshot()).slots[2].incarnation, 0);
    assert!(lifecycle(prepared.snapshot()).slots[2].component.is_none());
    let expected = prepared.snapshot().clone();
    let temp = tempfile::tempdir().unwrap();
    let destination = temp.path().join("world");
    publish_new_lifecycle_world(prepared, &destination).unwrap();
    let loaded = SnapshotStore::open(&destination)
        .unwrap()
        .load()
        .unwrap()
        .unwrap()
        .snapshot;
    assert_eq!(loaded, expected);
    let next = apply(
        &loaded,
        1,
        command(&loaded, "c", 30, ResolvePolicy::FindOrRespawn),
    );
    let restored = &next.native_world.as_ref().unwrap().actors[2];
    assert_eq!(restored.transform.translation[0], 4.0);
    assert_eq!(restored.variables[0].value, NativeValue::I32(999));
    assert_eq!(lifecycle(&next).slots[2].incarnation, 1);
}

#[test]
fn identity_is_canonical_placement_order_and_exact_original_name_units() {
    let catalog = catalog();
    let mut first = definition(&catalog);
    first.slots[0].owner_name = vec![0xd800, u16::from(b'A')];
    let compiled = CompiledNativeLifecycle::compile(first.clone(), &catalog).unwrap();
    first.slots.reverse();
    let reordered = CompiledNativeLifecycle::compile(first.clone(), &catalog).unwrap();
    assert_eq!(compiled.identity(), reordered.identity());
    let bytes = serde_json::to_vec(compiled.definition()).unwrap();
    let expected: String = Sha256::digest(&bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    assert_eq!(compiled.identity(), expected);
    first
        .slots
        .iter_mut()
        .find(|slot| slot.placement == "c")
        .unwrap()
        .owner_name[1] = u16::from(b'a');
    assert_ne!(
        CompiledNativeLifecycle::compile(first, &catalog)
            .unwrap()
            .identity(),
        compiled.identity()
    );
    let parsed =
        CompiledNativeLifecycle::parse(&String::from_utf8(bytes).unwrap(), &catalog).unwrap();
    assert_eq!(parsed.definition().slots[2].owner_name[0], 0xd800);
}

#[test]
fn all_named_dtos_reject_arrays_and_unknown_fields_without_leaking_inputs() {
    let world = world();
    let request = batch(
        &world,
        1,
        vec![command(&world, "a", 20, ResolvePolicy::FindOnly)],
    );
    rejects_non_objects(&lifecycle(&world).definition);
    rejects_non_objects(&lifecycle(&world).definition.slots[0]);
    rejects_non_objects(&lifecycle(&world).slots[0]);
    rejects_non_objects(&lifecycle(&world).bindings[0]);
    let restored = apply(&world, 1, request.recorders[0].clone());
    let completed = lifecycle(&restored).completed.as_ref().unwrap();
    rejects_non_objects(completed);
    rejects_non_objects(&completed.outcomes[0]);
    let values = [
        serde_json::to_value(lifecycle(&world)).unwrap(),
        serde_json::to_value(&request).unwrap(),
        serde_json::to_value(&request.recorders[0]).unwrap(),
        serde_json::to_value(OwnerHandle {
            id: DomainId(1),
            incarnation: 1,
        })
        .unwrap(),
        serde_json::to_value(component(10)).unwrap(),
    ];
    // Exercise each boundary using direct typed decoding, including nested fields.
    for (index, value) in values.iter().enumerate() {
        let array = Value::Array(value.as_object().unwrap().values().cloned().collect());
        let mut unknown = value.clone();
        unknown["PRIVATE_UNKNOWN_FIELD"] = json!("PRIVATE_VALUE");
        let decode = |value: Value| -> String {
            match index {
                0 => serde_json::from_value::<LifecycleSnapshot>(value)
                    .unwrap_err()
                    .to_string(),
                1 => serde_json::from_value::<RestoreBatch>(value)
                    .unwrap_err()
                    .to_string(),
                2 => serde_json::from_value::<RestoreRecorder>(value)
                    .unwrap_err()
                    .to_string(),
                3 => serde_json::from_value::<OwnerHandle>(value)
                    .unwrap_err()
                    .to_string(),
                _ => serde_json::from_value::<RecorderComponentRecord>(value)
                    .unwrap_err()
                    .to_string(),
            }
        };
        for invalid in [array, unknown] {
            assert!(!decode(invalid).contains("PRIVATE"));
        }
    }
    for invalid in [
        json!([]),
        json!({"type":"PRIVATE_KIND"}),
        json!({"type":"detached", "PRIVATE_FIELD":true}),
    ] {
        let error = serde_json::from_value::<RecorderSource>(invalid)
            .unwrap_err()
            .to_string();
        assert!(!error.contains("PRIVATE"));
    }
    let error = serde_json::from_value::<RecorderKind>(json!("PRIVATE_FUTURE_KIND"))
        .unwrap_err()
        .to_string();
    assert!(!error.contains("PRIVATE"));
    let mut nested = serde_json::to_value(&request).unwrap();
    nested["recorders"][0]["transform"] = json!([[0, 0, 0], [0, 0, 0, 1], [1, 1, 1]]);
    assert!(serde_json::from_value::<RestoreBatch>(nested).is_err());
    assert!(
        !format!(
            "{request:?} {:?} {:?}",
            lifecycle(&world),
            request.recorders[0]
        )
        .contains("Alpha")
    );
}

#[test]
fn definitions_and_requests_admit_bounded_regular_json_and_toml_only() {
    let catalog = catalog();
    let definition = definition(&catalog);
    let json = serde_json::to_string(&definition).unwrap();
    assert!(CompiledNativeLifecycle::parse(&json, &catalog).is_ok());
    // TOML has no null representation; these declarations have no components.
    let mut toml_definition = definition.clone();
    for slot in &mut toml_definition.slots {
        slot.initial_component = None;
    }
    let toml = toml::to_string(&toml_definition).unwrap();
    assert!(CompiledNativeLifecycle::parse(&toml, &catalog).is_ok());
    let temp = tempfile::tempdir().unwrap();
    let file = temp.path().join("PRIVATE_FILENAME");
    fs::write(&file, &json).unwrap();
    assert!(CompiledNativeLifecycle::load(&file, &catalog).is_ok());
    let symlink = temp.path().join("link");
    std::os::unix::fs::symlink(&file, &symlink).unwrap();
    assert!(CompiledNativeLifecycle::load(&symlink, &catalog).is_ok());
    assert!(CompiledNativeLifecycle::load(temp.path(), &catalog).is_err());
    fs::write(&file, " ".repeat(MAX_LIFECYCLE_DEFINITION_BYTES + 1)).unwrap();
    assert!(CompiledNativeLifecycle::load(&file, &catalog).is_err());
    fs::write(&file, "{\"PRIVATE_NAME\":\"PRIVATE_VALUE\"}").unwrap();
    let error = CompiledNativeLifecycle::load(&file, &catalog).unwrap_err();
    assert!(!format!("{error:#}").contains("PRIVATE"));
    let world = world();
    let request = batch(
        &world,
        1,
        vec![command(&world, "a", 20, ResolvePolicy::FindOnly)],
    );
    fs::write(&file, serde_json::to_vec(&request).unwrap()).unwrap();
    assert_eq!(RestoreBatch::load(&file).unwrap(), request);
    assert_eq!(
        RestoreBatch::parse(&toml::to_string(&request).unwrap()).unwrap(),
        request
    );
    assert!(RestoreBatch::parse(&" ".repeat(MAX_RESTORE_BATCH_BYTES + 1)).is_err());
    assert!(RestoreBatch::load(temp.path()).is_err());
}

#[test]
fn programmatic_lengths_aggregates_and_spare_capacities_reject_before_cloning() {
    let catalog = catalog();
    let original = definition(&catalog);
    let mut invalid = original.clone();
    invalid.slots[0].owner_name = vec![0; MAX_LIFECYCLE_NAME_UNITS + 1];
    assert!(CompiledNativeLifecycle::compile(invalid, &catalog).is_err());
    let mut invalid = original.clone();
    invalid.slots[0].owner_name.clear();
    assert!(CompiledNativeLifecycle::compile(invalid, &catalog).is_err());
    let mut invalid = original.clone();
    invalid.slots = vec![original.slots[0].clone(); MAX_NATIVE_PLACEMENTS + 1];
    assert!(CompiledNativeLifecycle::compile(invalid, &catalog).is_err());
    let mut invalid = original.clone();
    invalid.slots[0].owner_name = vec![0; MAX_LIFECYCLE_NAME_UNITS];
    invalid.slots = vec![
        invalid.slots[0].clone();
        MAX_LIFECYCLE_NAME_TOTAL_UNITS / MAX_LIFECYCLE_NAME_UNITS + 1
    ];
    assert!(
        CompiledNativeLifecycle::compile(invalid, &catalog)
            .unwrap_err()
            .to_string()
            .contains("aggregate")
    );
    let mut invalid = original.clone();
    invalid.slots[0].owner_name = vec![0xffff; MAX_LIFECYCLE_NAME_UNITS];
    invalid.slots = vec![invalid.slots[0].clone(); 700];
    assert!(
        CompiledNativeLifecycle::compile(invalid, &catalog)
            .unwrap_err()
            .to_string()
            .contains("byte limit")
    );
    let mut invalid = original;
    invalid.slots[0]
        .owner_name
        .reserve(MAX_LIFECYCLE_NAME_UNITS * 2);
    assert!(CompiledNativeLifecycle::compile(invalid, &catalog).is_err());
    let world = world();
    let mut request = batch(
        &world,
        1,
        vec![command(&world, "a", 20, ResolvePolicy::FindOnly)],
    );
    request.recorders[0].variables.reserve(128);
    assert!(prepare_owner_restore(&world, &request).is_err());
    let mut invalid = world.clone();
    invalid
        .native_lifecycle
        .as_mut()
        .unwrap()
        .bindings
        .reserve(MAX_NATIVE_PLACEMENTS + 1);
    assert!(invalid.validate().is_err());
    let mut invalid = world.clone();
    invalid.native_world.as_mut().unwrap().actors[0].variables[1].value = NativeValue::String({
        let mut value = String::with_capacity(8192);
        value.push('x');
        value
    });
    assert!(invalid.validate().is_err());
}

#[test]
fn lifecycle_binding_corruption_fails_closed() {
    let world = world();
    let original = lifecycle(&world).clone();
    let native = world.native_world.as_ref().unwrap();
    let compiled = CompiledNativeLifecycle::from_snapshot(&original, native).unwrap();
    let mut variants = Vec::new();
    let mut invalid = original.clone();
    invalid.bindings.clear();
    variants.push(invalid);
    let mut invalid = original.clone();
    invalid.bindings[0].owner.incarnation += 1;
    variants.push(invalid);
    let mut invalid = original.clone();
    invalid.bindings[0].owner.id = DomainId(3);
    variants.push(invalid);
    let mut invalid = original.clone();
    invalid.slots[2].component = Some(component(30));
    variants.push(invalid);
    let mut invalid = original.clone();
    invalid.slots[2].incarnation = 1;
    variants.push(invalid);
    let mut invalid = original.clone();
    invalid.slots[0].component = Some(component(0));
    variants.push(invalid);
    let mut invalid = original.clone();
    invalid.slots[1].component = Some(component(10));
    variants.push(invalid);
    let mut invalid = original.clone();
    invalid.slots.swap(0, 1);
    variants.push(invalid);
    let mut invalid = original.clone();
    invalid.slots.pop();
    variants.push(invalid);
    let mut invalid = original.clone();
    invalid.definition.slots[0].owner_name.push(1);
    variants.push(invalid);
    for invalid in variants {
        assert!(compiled.validate_snapshot(&invalid, native).is_err());
    }
    // Absent records still obey the full spatial schema and tile bounds.
    let mut invalid = world.clone();
    invalid.native_world.as_mut().unwrap().actors[2]
        .transform
        .translation[0] = 100.0;
    assert!(invalid.validate().is_err());
    let mut invalid = world.clone();
    invalid.native_world.as_mut().unwrap().actors.pop();
    assert!(invalid.validate().is_err());
}

#[test]
fn detached_lookup_searches_absent_declarations_and_preserves_surrogates() {
    let ambiguous = world_with(|definition| {
        definition
            .slots
            .iter_mut()
            .find(|slot| slot.placement == "c")
            .unwrap()
            .owner_name = "ALPHA".encode_utf16().collect();
    });
    let request = batch(
        &ambiguous,
        1,
        vec![command(&ambiguous, "a", 20, ResolvePolicy::FindOnly)],
    );
    assert!(prepare_owner_restore(&ambiguous, &request).is_err());
    let world = world_with(|definition| {
        definition
            .slots
            .iter_mut()
            .find(|slot| slot.placement == "c")
            .unwrap()
            .owner_name = vec![0xd800, u16::from(b'A')];
    });
    let mut recorder = command(&world, "c", 30, ResolvePolicy::FindOrRespawn);
    recorder.owner_name = vec![0xd800, u16::from(b'a')];
    let restored = apply(&world, 1, recorder.clone());
    assert_eq!(lifecycle(&restored).slots[2].presence, Presence::Live);
    recorder.owner_name[0] = 0xfffd;
    assert!(prepare_owner_restore(&world, &batch(&world, 1, vec![recorder])).is_err());
    let mut recorder = command(&world, "c", 30, ResolvePolicy::FindOrRespawn);
    recorder.owner_name = "Beta".encode_utf16().collect();
    assert!(prepare_owner_restore(&world, &batch(&world, 1, vec![recorder])).is_err());
}

#[test]
fn bound_guid_resolution_ignores_name_and_policy_but_rejects_ignored_state() {
    let world = world();
    for policy in [
        ResolvePolicy::FindOnly,
        ResolvePolicy::FindOrRespawn,
        ResolvePolicy::RespawnOnly,
    ] {
        let mut recorder = command(&world, "a", 10, policy);
        recorder.source = RecorderSource::Bound;
        recorder.owner_name = "Ghost".encode_utf16().collect();
        let restored = apply(&world, 1, recorder.clone());
        let outcome = &lifecycle(&restored).completed.as_ref().unwrap().outcomes[0];
        assert_eq!(outcome.disposition, RestoreDisposition::KeptBound);
        assert_eq!(outcome.owner.incarnation, 1);
        assert_eq!(
            outcome.component_disposition,
            ComponentDisposition::Existing
        );
        recorder.transform.translation[0] = 1.0;
        assert!(prepare_owner_restore(&world, &batch(&world, 1, vec![recorder.clone()])).is_err());
        recorder.transform.translation[0] = 0.0;
        recorder.variables[0].value = NativeValue::I32(99);
        assert!(prepare_owner_restore(&world, &batch(&world, 1, vec![recorder])).is_err());
    }
    let mut wrong = command(&world, "b", 10, ResolvePolicy::FindOnly);
    wrong.source = RecorderSource::Bound;
    assert!(prepare_owner_restore(&world, &batch(&world, 1, vec![wrong])).is_err());
}

#[test]
fn detached_find_spawn_and_replace_obey_incarnations_and_retire_incumbents() {
    let world = world();
    for policy in [ResolvePolicy::FindOnly, ResolvePolicy::FindOrRespawn] {
        let restored = apply(&world, 1, command(&world, "a", 20, policy));
        let lifecycle = lifecycle(&restored);
        assert_eq!(lifecycle.slots[0].incarnation, 1);
        assert_eq!(
            lifecycle.completed.as_ref().unwrap().outcomes[0].disposition,
            RestoreDisposition::Attached
        );
        assert_eq!(lifecycle.bindings.len(), 1);
        assert_eq!(lifecycle.bindings[0].guid, 20);
    }
    let restored = apply(
        &world,
        1,
        command(&world, "a", 20, ResolvePolicy::RespawnOnly),
    );
    assert_eq!(lifecycle(&restored).slots[0].incarnation, 2);
    assert_eq!(
        lifecycle(&restored).completed.as_ref().unwrap().outcomes[0].disposition,
        RestoreDisposition::Replaced
    );
    for policy in [ResolvePolicy::FindOrRespawn, ResolvePolicy::RespawnOnly] {
        let mut recorder = command(&world, "c", 30, policy);
        recorder.transform.translation[0] = 2.0;
        recorder.variables[0].value = NativeValue::I32(7);
        let restored = apply(&world, 1, recorder);
        assert_eq!(lifecycle(&restored).slots[2].incarnation, 1);
        assert_eq!(
            lifecycle(&restored).completed.as_ref().unwrap().outcomes[0].disposition,
            RestoreDisposition::Spawned
        );
        assert_eq!(
            restored.native_world.as_ref().unwrap().actors[2].variables[0].value,
            NativeValue::I32(7)
        );
    }
    assert!(
        prepare_owner_restore(
            &world,
            &batch(
                &world,
                1,
                vec![command(&world, "c", 30, ResolvePolicy::FindOnly)]
            )
        )
        .is_err()
    );
}

#[test]
fn missing_components_require_creation_for_found_and_spawned_owners() {
    let world = world();
    for policy in [ResolvePolicy::FindOnly, ResolvePolicy::FindOrRespawn] {
        let mut recorder = command(&world, "b", 20, policy);
        assert!(prepare_owner_restore(&world, &batch(&world, 1, vec![recorder.clone()])).is_err());
        recorder.create_if_missing = true;
        let restored = apply(&world, 1, recorder);
        let outcome = &lifecycle(&restored).completed.as_ref().unwrap().outcomes[0];
        assert_eq!(outcome.disposition, RestoreDisposition::Attached);
        assert_eq!(outcome.component_disposition, ComponentDisposition::Created);
        assert_eq!(outcome.owner.incarnation, 1);
    }
    let world = world_with(|definition| {
        for slot in &mut definition.slots {
            slot.spawn_component = SpawnComponent::Missing;
        }
    });
    for (placement, policy, expected) in [
        (
            "c",
            ResolvePolicy::FindOrRespawn,
            RestoreDisposition::Spawned,
        ),
        (
            "a",
            ResolvePolicy::RespawnOnly,
            RestoreDisposition::Replaced,
        ),
    ] {
        let mut recorder = command(&world, placement, 30, policy);
        assert!(prepare_owner_restore(&world, &batch(&world, 1, vec![recorder.clone()])).is_err());
        recorder.create_if_missing = true;
        let restored = apply(&world, 1, recorder);
        let outcome = &lifecycle(&restored).completed.as_ref().unwrap().outcomes[0];
        assert_eq!(outcome.disposition, expected);
        assert_eq!(outcome.component_disposition, ComponentDisposition::Created);
    }
}

#[test]
fn detached_prior_binding_is_exact_cas_and_cannot_move_a_guid() {
    let world = world();
    let mut recorder = command(&world, "a", 10, ResolvePolicy::RespawnOnly);
    assert!(prepare_owner_restore(&world, &batch(&world, 1, vec![recorder.clone()])).is_err());
    recorder.source = RecorderSource::Detached {
        expected_prior_binding: Some(OwnerHandle {
            id: DomainId(1),
            incarnation: 2,
        }),
    };
    assert!(prepare_owner_restore(&world, &batch(&world, 1, vec![recorder.clone()])).is_err());
    recorder.source = RecorderSource::Detached {
        expected_prior_binding: Some(OwnerHandle {
            id: DomainId(1),
            incarnation: 1,
        }),
    };
    let restored = apply(&world, 1, recorder);
    assert_eq!(lifecycle(&restored).bindings[0].owner.incarnation, 2);
    assert_eq!(lifecycle(&restored).bindings[0].guid, 10);
    let mut move_guid = command(&world, "c", 10, ResolvePolicy::RespawnOnly);
    move_guid.source = RecorderSource::Detached {
        expected_prior_binding: Some(OwnerHandle {
            id: DomainId(1),
            incarnation: 1,
        }),
    };
    assert!(prepare_owner_restore(&world, &batch(&world, 1, vec![move_guid])).is_err());
    let mut unbound = command(&world, "c", 30, ResolvePolicy::RespawnOnly);
    unbound.source = RecorderSource::Detached {
        expected_prior_binding: Some(OwnerHandle {
            id: DomainId(3),
            incarnation: 1,
        }),
    };
    assert!(prepare_owner_restore(&world, &batch(&world, 1, vec![unbound])).is_err());
}

#[test]
fn exact_retry_is_canonical_and_preserves_subsequent_pose_and_variable_edits() {
    let original = world();
    let mut create = command(&original, "b", 20, ResolvePolicy::FindOnly);
    create.create_if_missing = true;
    let mut request = batch(
        &original,
        1,
        vec![
            command(&original, "c", 30, ResolvePolicy::RespawnOnly),
            create,
        ],
    );
    let prepared = prepare_owner_restore(&original, &request).unwrap();
    assert!(!prepared.already_completed());
    let completion = prepared.completion().clone();
    let mut canonical = request.clone();
    canonical
        .recorders
        .sort_by(|a, b| a.target_placement.cmp(&b.target_placement));
    for recorder in &mut canonical.recorders {
        recorder.variables.sort_by(|a, b| a.key.cmp(&b.key));
    }
    let digest: String = Sha256::digest(serde_json::to_vec(&canonical).unwrap())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    assert_eq!(completion.request_digest, digest);
    let mut restored = prepared.into_snapshot();
    restored.native_world.as_mut().unwrap().actors[1]
        .transform
        .translation[0] = 2.0;
    restored.native_world.as_mut().unwrap().actors[1].variables[0].value = NativeValue::I32(123);
    request.recorders.reverse();
    for recorder in &mut request.recorders {
        recorder.variables.reverse();
    }
    let retry = prepare_owner_restore(&restored, &request).unwrap();
    assert!(retry.already_completed());
    assert_eq!(retry.completion(), &completion);
    assert_eq!(retry.snapshot(), &restored);
    // A conflict includes every caller input, even fields unused by Bound.
    let mut conflict = request.clone();
    conflict.recorders[0].create_if_missing ^= true;
    assert!(prepare_owner_restore(&restored, &conflict).is_err());
    let mut conflict = request.clone();
    conflict.recorders[0].source = RecorderSource::Bound;
    assert!(prepare_owner_restore(&restored, &conflict).is_err());
    let mut corrupt = restored.clone();
    corrupt.native_lifecycle.as_mut().unwrap().bindings.clear();
    assert!(prepare_owner_restore(&corrupt, &request).is_err());
}

#[test]
fn review_missing_completion_after_same_guid_attachment_cannot_replay() {
    let initial = world();
    let slot = &lifecycle(&initial).slots[0];
    let mut recorder = command(
        &initial,
        "a",
        slot.component.as_ref().unwrap().guid,
        ResolvePolicy::FindOnly,
    );
    recorder.source = RecorderSource::Detached {
        expected_prior_binding: Some(OwnerHandle {
            id: slot.id,
            incarnation: slot.incarnation,
        }),
    };
    let request = batch(&initial, 1, vec![recorder]);
    let mut restored = prepare_owner_restore(&initial, &request)
        .unwrap()
        .into_snapshot();
    restored.native_world.as_mut().unwrap().actors[0]
        .transform
        .translation[0] = 7.0;
    restored.validate().unwrap();
    assert_eq!(lifecycle(&restored).restore_sequence, 1);
    let mut mismatched = restored.clone();
    mismatched
        .native_lifecycle
        .as_mut()
        .unwrap()
        .restore_sequence = 0;
    assert!(mismatched.validate().is_err());
    restored.native_lifecycle.as_mut().unwrap().completed = None;
    assert!(
        restored.validate().is_err(),
        "missing completion must remain distinguishable from an unstarted world"
    );
    assert!(SnapshotStore::preflight(&restored).is_err());
    assert!(prepare_owner_restore(&restored, &request).is_err());
}

#[test]
fn sequences_stale_expectations_and_checked_overflow_fail_closed() {
    let world = world();
    let request = batch(
        &world,
        1,
        vec![command(&world, "a", 20, ResolvePolicy::RespawnOnly)],
    );
    let restored = prepare_owner_restore(&world, &request)
        .unwrap()
        .into_snapshot();
    let mut stale = request.clone();
    stale.sequence = 2;
    assert!(prepare_owner_restore(&restored, &stale).is_err());
    let next = command(&restored, "a", 40, ResolvePolicy::FindOnly);
    for sequence in [0, 1, 3] {
        assert!(
            prepare_owner_restore(&restored, &batch(&restored, sequence, vec![next.clone()]))
                .is_err()
        );
    }
    assert!(prepare_owner_restore(&world, &batch(&world, 2, vec![next])).is_err());
    let mut exhausted = restored.clone();
    exhausted
        .native_lifecycle
        .as_mut()
        .unwrap()
        .completed
        .as_mut()
        .unwrap()
        .sequence = u64::MAX;
    exhausted
        .native_lifecycle
        .as_mut()
        .unwrap()
        .restore_sequence = u64::MAX;
    exhausted.validate().unwrap();
    assert!(
        prepare_owner_restore(
            &exhausted,
            &batch(
                &exhausted,
                1,
                vec![command(&exhausted, "a", 40, ResolvePolicy::RespawnOnly)]
            )
        )
        .is_err()
    );
    let mut exhausted = restored;
    let lifecycle = exhausted.native_lifecycle.as_mut().unwrap();
    lifecycle.slots[0].incarnation = u64::MAX;
    lifecycle.bindings[0].owner.incarnation = u64::MAX;
    let outcome = &mut lifecycle.completed.as_mut().unwrap().outcomes[0];
    outcome.before_incarnation = u64::MAX - 1;
    outcome.owner.incarnation = u64::MAX;
    exhausted.validate().unwrap();
    assert!(
        prepare_owner_restore(
            &exhausted,
            &batch(
                &exhausted,
                2,
                vec![command(&exhausted, "a", 40, ResolvePolicy::RespawnOnly)]
            )
        )
        .is_err()
    );
}

#[test]
fn completion_receipts_validate_equations_and_current_binding_agreement() {
    let initial = world();
    let restored = apply(
        &initial,
        1,
        command(&initial, "a", 20, ResolvePolicy::RespawnOnly),
    );
    for variant in 0..8 {
        let mut invalid = restored.clone();
        let completed = invalid
            .native_lifecycle
            .as_mut()
            .unwrap()
            .completed
            .as_mut()
            .unwrap();
        match variant {
            0 => completed.sequence = 0,
            1 => completed.request_digest = "A".repeat(64),
            2 => completed.outcomes[0].before_incarnation = 0,
            3 => completed.outcomes[0].owner.incarnation += 1,
            4 => completed.outcomes[0].disposition = RestoreDisposition::Attached,
            5 => completed.outcomes[0].component_disposition = ComponentDisposition::Created,
            6 => completed.outcomes.push(completed.outcomes[0].clone()),
            _ => completed.outcomes[0].guid = 99,
        }
        assert!(invalid.validate().is_err(), "receipt mutation {variant}");
    }
}

#[test]
fn multi_command_failure_and_duplicates_leave_all_original_state_unchanged() {
    let world = world();
    let original = world.clone();
    let first = command(&world, "a", 20, ResolvePolicy::RespawnOnly);
    let mut invalid = command(&world, "c", 30, ResolvePolicy::RespawnOnly);
    invalid.transform.translation[0] = 99.0;
    let request = batch(&world, 1, vec![first.clone(), invalid]);
    assert!(prepare_owner_restore(&world, &request).is_err());
    assert_eq!(world, original);
    assert!(
        prepare_owner_restore(
            &world,
            &batch(&world, 1, vec![first.clone(), first.clone()])
        )
        .is_err()
    );
    let same_guid = command(&world, "c", first.guid, ResolvePolicy::RespawnOnly);
    assert!(
        prepare_owner_restore(&world, &batch(&world, 1, vec![first.clone(), same_guid])).is_err()
    );
    let mut duplicate_variable = first.clone();
    duplicate_variable
        .variables
        .push(duplicate_variable.variables[0].clone());
    assert!(prepare_owner_restore(&world, &batch(&world, 1, vec![duplicate_variable])).is_err());
    let mut missing_variable = first.clone();
    missing_variable.variables.clear();
    assert!(prepare_owner_restore(&world, &batch(&world, 1, vec![missing_variable])).is_err());
    let mut wrong_type = first;
    wrong_type.variables[0].value = NativeValue::Bool(true);
    assert!(prepare_owner_restore(&world, &batch(&world, 1, vec![wrong_type])).is_err());
    assert_eq!(world, original);
}

fn relationship_world() -> WorldSnapshot {
    let catalog = catalog();
    let link = |name: &str, guid| NativeActorLinkV1 {
        name: name.encode_utf16().collect(),
        guid,
    };
    let relationships = CompiledNativeRelationships::compile(
        NativeRelationshipsDefinition {
            relationship_format: 1,
            native_catalog_id: catalog.identity().into(),
            map: "map".into(),
            actors: vec![
                NativeRelationshipActorV1 {
                    placement: "a".into(),
                    identity: link("Device", 100),
                    role: NativeRelationshipRole::Deployable,
                    foundation: Some(link("Base", 200)),
                },
                NativeRelationshipActorV1 {
                    placement: "b".into(),
                    identity: link("Base", 200),
                    role: NativeRelationshipRole::Foundation,
                    foundation: None,
                },
                NativeRelationshipActorV1 {
                    placement: "c".into(),
                    identity: link("Dormant", 300),
                    role: NativeRelationshipRole::Deployable,
                    foundation: None,
                },
            ],
        },
        &catalog,
    )
    .unwrap();
    let mut base = spatial(&catalog);
    base.schema_version = SNAPSHOT_SCHEMA_RELATIONSHIPS;
    base.native_relationships = Some(relationships.initial_snapshot());
    let compiled = CompiledNativeLifecycle::compile(definition(&catalog), &catalog).unwrap();
    prepare_lifecycle_world(base, &compiled)
        .unwrap()
        .into_snapshot()
}

#[test]
fn complete_foundation_edges_survive_restores_and_absent_endpoints_reject() {
    let world = relationship_world();
    let edges = world.native_relationships.as_ref().unwrap().clone();
    let restored = apply(
        &world,
        1,
        command(&world, "a", 20, ResolvePolicy::RespawnOnly),
    );
    assert_eq!(restored.native_relationships.as_ref().unwrap(), &edges);
    let restored = apply(
        &restored,
        2,
        command(&restored, "c", 30, ResolvePolicy::RespawnOnly),
    );
    assert_eq!(restored.native_relationships.as_ref().unwrap(), &edges);
    let mut invalid = world.clone();
    invalid.native_relationships.as_mut().unwrap().foundations[1].foundation = Some(DomainId(2));
    assert!(invalid.validate().is_err());
    // The declared foundation can be absent while its spatial row remains.
    let catalog = catalog();
    let mut definition = definition(&catalog);
    definition
        .slots
        .iter_mut()
        .find(|slot| slot.placement == "b")
        .unwrap()
        .initial_presence = Presence::Absent;
    let compiled = CompiledNativeLifecycle::compile(definition, &catalog).unwrap();
    let absent = compiled.initial_snapshot();
    assert!(
        validate_lifecycle_relationships(
            world.native_world.as_ref().unwrap(),
            &absent,
            Some(&edges)
        )
        .is_err()
    );
    // Removing an edge does not remove its complete deployable state row.
    let mut no_edges = edges;
    no_edges.foundations[0].foundation = None;
    validate_lifecycle_relationships(
        world.native_world.as_ref().unwrap(),
        &absent,
        Some(&no_edges),
    )
    .unwrap();
    no_edges.foundations.pop();
    assert!(
        validate_lifecycle_relationships(
            world.native_world.as_ref().unwrap(),
            &absent,
            Some(&no_edges)
        )
        .is_err()
    );
}

#[test]
fn legacy_base_must_be_schema_four_or_six_and_native_validation_stays_complete() {
    let catalog = catalog();
    let compiled = CompiledNativeLifecycle::compile(definition(&catalog), &catalog).unwrap();
    assert!(prepare_lifecycle_world(WorldSnapshot::default(), &compiled).is_err());
    assert!(prepare_lifecycle_world(world(), &compiled).is_err());
    let mut missing = spatial(&catalog);
    missing.native_world.as_mut().unwrap().actors.pop();
    assert!(prepare_lifecycle_world(missing, &compiled).is_err());
    let mut invalid = spatial(&catalog);
    invalid.native_world.as_mut().unwrap().actors[2]
        .variables
        .clear();
    assert!(prepare_lifecycle_world(invalid, &compiled).is_err());
}

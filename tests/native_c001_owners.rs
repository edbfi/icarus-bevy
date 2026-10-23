//! Independent authored adapter contract; no original or private inputs.
#[path = "common/c001_owners.rs"]
mod c001_owners;
use c001_owners::{Fixture, Tag, fixture, floats, stream, string, units};
use icarus_bevy::{
    import_candidate::DecodedImportCandidate,
    native_c001_owners::{
        C001OwnerAdapter, C001OwnerBindingV1, C001OwnerBindingsV1, C001OwnerImportMetadataV1,
        CompiledC001OwnerBindings, PreparedC001OwnerImport, ResolvedC001OwnerV1,
        prepare_c001_owner_import, publish_new_c001_owner_import,
    },
    native_import::{
        C001InertBindingsV1, C001PlacementBindingV1, CompiledC001InertBindings,
        SemanticImportCategory, SemanticImportError, prepare_c001_inert_import,
    },
    native_lifecycle::{
        CompiledNativeLifecycle, OwnerHandle, Presence, RecorderKind, RecorderSource,
        ResolvePolicy, RestoreBatch, RestoreDisposition, RestoreRecorder, prepare_owner_restore,
    },
    native_relationships::CompiledNativeRelationships,
    native_world::MAX_NATIVE_PLACEMENTS,
    persistence::SnapshotStore,
    preservation::{PreserveLimits, PreservedSection},
    simulation::{DomainId, SNAPSHOT_SCHEMA_C001_OWNERS, WorldSnapshot},
};
use serde_json::{Value, json};
use std::{fs, sync::Arc};

fn prepare(f: &Fixture) -> Result<PreparedC001OwnerImport, SemanticImportError> {
    prepare_c001_owner_import(
        &f.candidate(),
        f.base.clone(),
        &CompiledC001OwnerBindings::compile(f.bindings.clone()).unwrap(),
    )
}
fn policy_numeric(value: u8) -> Tag {
    Tag {
        name: "OwnerResolvePolicy",
        kind: "ByteProperty",
        extras: [string("None"), vec![0]].concat(),
        payload: vec![value],
        index: 0,
    }
}
fn policy_named(kind: &'static str, name: &str) -> Tag {
    Tag {
        name: "OwnerResolvePolicy",
        kind,
        extras: [string("EStateRecorderOwnerResolvePolicy"), vec![0]].concat(),
        payload: string(name),
        index: 0,
    }
}
fn set_policy(f: &mut Fixture, index: usize, tag: Option<Tag>) {
    f.owners[index].retain(|t| t.name != "OwnerResolvePolicy");
    if let Some(tag) = tag {
        f.owners[index].push(tag);
    }
    f.rebuild_source();
}
fn rejected_source(f: &mut Fixture) {
    f.rebuild_source();
    if let Ok(candidate) = DecodedImportCandidate::decode(f.envelope.as_slice()) {
        assert!(
            prepare_c001_owner_import(
                &candidate,
                f.base.clone(),
                &CompiledC001OwnerBindings::compile(f.bindings.clone()).unwrap()
            )
            .is_err()
        );
    }
}
fn replace_lifecycle(
    f: &mut Fixture,
    edit: impl FnOnce(&mut icarus_bevy::native_lifecycle::LifecycleDefinition),
) {
    let mut definition = f.base.native_lifecycle.as_ref().unwrap().definition.clone();
    edit(&mut definition);
    let compiled = CompiledNativeLifecycle::compile(definition, &f.catalog).unwrap();
    f.bindings.native_lifecycle_id = compiled.identity().into();
    f.base.native_lifecycle = Some(compiled.initial_snapshot());
}
fn later_batch(
    world: &WorldSnapshot,
    slot: DomainId,
    guid: i32,
    policy: ResolvePolicy,
) -> RestoreBatch {
    let lifecycle = world.native_lifecycle.as_ref().unwrap();
    let rank = slot.0 as usize - 1;
    let declaration = &lifecycle.definition.slots[rank];
    let state = &lifecycle.slots[rank];
    let actor = world
        .native_world
        .as_ref()
        .unwrap()
        .actors
        .iter()
        .find(|a| a.id == slot)
        .unwrap();
    RestoreBatch {
        lifecycle_id: lifecycle.lifecycle_id.clone(),
        sequence: lifecycle.restore_sequence + 1,
        recorders: vec![RestoreRecorder {
            guid,
            kind: RecorderKind::SpatialStateV1,
            target_placement: declaration.placement.clone(),
            owner_name: declaration.owner_name.clone(),
            policy,
            source: RecorderSource::Detached {
                expected_prior_binding: lifecycle
                    .bindings
                    .iter()
                    .find(|b| b.guid == guid)
                    .map(|b| b.owner),
            },
            create_if_missing: true,
            expected_presence: state.presence,
            expected_incarnation: state.incarnation,
            transform: actor.transform.clone(),
            variables: vec![],
        }],
    }
}
fn inert(f: &Fixture) -> Result<(), SemanticImportError> {
    let definition = C001InertBindingsV1 {
        format: 1,
        expected_payload_sha256: f.bindings.expected_payload_sha256.clone(),
        native_catalog_id: f.bindings.native_catalog_id.clone(),
        source_map: f.bindings.source_map.clone(),
        native_map: f.bindings.native_map.clone(),
        coordinates: f.bindings.coordinates,
        owners: f
            .bindings
            .owners
            .iter()
            .map(|b| C001PlacementBindingV1 {
                source_actor_class: b.source_actor_class.clone(),
                source_actor_path: b.source_actor_path.clone(),
                source_object_name: b.source_object_name.clone(),
                source_icarus_guid: b.source_icarus_guid,
                native_placement: b.native_placement.clone(),
                native_class: b.native_class.clone(),
            })
            .collect(),
    };
    prepare_c001_inert_import(
        &f.candidate(),
        f.catalog.clone(),
        &CompiledC001InertBindings::compile(definition).unwrap(),
    )
    .map(|_| ())
}

#[test]
fn complete_owner_import_preserves_tick_and_independent_relationship_edits() {
    let mut f = fixture();
    f.base.tick = 123;
    let relationships = f.base.native_relationships.as_ref().unwrap();
    let compiled = CompiledNativeRelationships::from_snapshot(
        relationships,
        f.base.native_world.as_ref().unwrap(),
    )
    .unwrap();
    compiled
        .reassign_foundation(
            f.base.native_relationships.as_mut().unwrap(),
            DomainId(3),
            None,
        )
        .unwrap();
    let prepared = prepare(&f).unwrap();
    let counts = prepared.inspection();
    assert_eq!(
        (
            counts.recorders,
            counts.bindings,
            counts.slots,
            counts.live_owners
        ),
        (3, 3, 3, 3)
    );
    assert_eq!(
        (counts.attached, counts.spawned, counts.replaced),
        (1, 1, 1)
    );
    assert_eq!(counts.payload_bytes, f.payload.len());
    assert!(counts.metadata_bytes > 0);
    let snapshot = prepared.into_snapshot();
    assert_eq!(snapshot.schema_version, SNAPSHOT_SCHEMA_C001_OWNERS);
    assert_eq!(snapshot.tick, 123);
    assert_eq!(snapshot.native_relationships, f.base.native_relationships);
    assert!(snapshot.native_import.is_none());
    assert_eq!(
        snapshot
            .preserved_prospect
            .as_ref()
            .unwrap()
            .payload_sha256(),
        f.candidate().preserved().payload_sha256()
    );
    let lifecycle = snapshot.native_lifecycle.as_ref().unwrap();
    assert_eq!(lifecycle.restore_sequence, 1);
    assert!(lifecycle.slots.iter().all(|s| s.presence == Presence::Live));
    assert_eq!(
        lifecycle
            .slots
            .iter()
            .map(|s| s.incarnation)
            .collect::<Vec<_>>(),
        vec![1, 2, 1]
    );
    assert_eq!(
        lifecycle
            .completed
            .as_ref()
            .unwrap()
            .outcomes
            .iter()
            .map(|o| o.disposition)
            .collect::<Vec<_>>(),
        vec![
            RestoreDisposition::Spawned,
            RestoreDisposition::Replaced,
            RestoreDisposition::Attached
        ]
    );
    assert!(!lifecycle.bindings.iter().any(|b| b.guid == 41));
    for (index, binding) in f.bindings.owners.iter().enumerate() {
        let actor = snapshot
            .native_world
            .as_ref()
            .unwrap()
            .actors
            .iter()
            .find(|a| a.placement == binding.native_placement)
            .unwrap();
        assert_eq!(actor.transform, f.transforms[index]);
        assert_ne!(binding.source_object_name, binding.native_owner_name);
        assert_ne!(binding.source_icarus_guid, binding.native_recorder_guid);
    }
    snapshot.validate().unwrap();
}

#[test]
fn omitted_numeric_short_and_qualified_policy_forms_bind_exactly() {
    for kind in ["ByteProperty", "EnumProperty"] {
        for qualified in [false, true] {
            let mut f = fixture();
            for (i, name) in ["FindOnly", "FindOrRespawn", "RespawnOnly"]
                .into_iter()
                .enumerate()
            {
                let name = if qualified {
                    format!("EStateRecorderOwnerResolvePolicy::{name}")
                } else {
                    name.into()
                };
                set_policy(&mut f, i, Some(policy_named(kind, &name)));
            }
            let world = prepare(&f).unwrap().into_snapshot();
            world.validate().unwrap();
            assert_eq!(
                world
                    .native_owner_import
                    .unwrap()
                    .owners
                    .iter()
                    .map(|o| o.source_policy)
                    .collect::<Vec<_>>(),
                vec![
                    ResolvePolicy::FindOnly,
                    ResolvePolicy::FindOrRespawn,
                    ResolvePolicy::RespawnOnly
                ]
            );
        }
    }
    let mut f = fixture();
    for (index, value) in [0, 2, 1].into_iter().enumerate() {
        set_policy(&mut f, index, Some(policy_numeric(value)));
    }
    prepare(&f).unwrap().snapshot().validate().unwrap();
    set_policy(&mut f, 1, None);
    assert_eq!(
        prepare(&f)
            .unwrap()
            .snapshot()
            .native_owner_import
            .as_ref()
            .unwrap()
            .owners[1]
            .source_policy,
        ResolvePolicy::FindOrRespawn
    );
    for index in 0..3 {
        set_policy(&mut f, index, None);
    }
    let world = prepare(&f).unwrap().into_snapshot();
    assert!(
        world
            .native_owner_import
            .unwrap()
            .owners
            .iter()
            .all(|o| o.source_policy == ResolvePolicy::FindOrRespawn)
    );
}

#[test]
fn inert_adapter_retains_find_only_find_or_respawn_and_omission_rules() {
    let mut f = fixture();
    assert!(inert(&f).is_err()); // Owner profile's RespawnOnly never widens inert admission.
    for kind in ["ByteProperty", "EnumProperty"] {
        for name in [
            "RespawnOnly",
            "EStateRecorderOwnerResolvePolicy::RespawnOnly",
        ] {
            set_policy(&mut f, 2, Some(policy_named(kind, name)));
            assert!(inert(&f).is_err());
        }
    }
    for value in [0, 2] {
        for i in 0..3 {
            set_policy(&mut f, i, Some(policy_numeric(value)));
        }
        inert(&f).unwrap();
    }
    for i in 0..3 {
        set_policy(&mut f, i, None);
    }
    inert(&f).unwrap();
}

#[test]
fn unsupported_source_policy_encodings_and_fields_reject_whole_candidate() {
    for value in [3, 4, 255] {
        let mut f = fixture();
        set_policy(&mut f, 2, Some(policy_numeric(value)));
        rejected_source(&mut f);
    }
    for kind in ["ByteProperty", "EnumProperty"] {
        for name in [
            "ManuallyResolved",
            "EStateRecorderOwnerResolvePolicy::ManuallyResolved",
            "respawnonly",
            "Unknown",
        ] {
            let mut f = fixture();
            set_policy(&mut f, 2, Some(policy_named(kind, name)));
            rejected_source(&mut f);
        }
    }
    for name in ["bSkipDefaultOwnerResolution", "bWasMovedToSubLevel"] {
        let mut f = fixture();
        f.owners[2].retain(|t| t.name != name);
        f.owners[2].push(Tag::boolean(name, true));
        rejected_source(&mut f);
    }
    for field in [
        "BoolVariables",
        "IntVariables",
        "FloatVariables",
        "StringVariables",
        "Inventories",
        "Modifiers",
        "SubComponents",
    ] {
        let mut f = fixture();
        f.owners[2].push(Tag::array(
            field,
            "IntProperty",
            0_i32.to_le_bytes().to_vec(),
        ));
        rejected_source(&mut f);
    }
    for field in [
        "Label",
        "ParentCharacterUID",
        "TileName",
        "Respawn",
        "Unexpected",
    ] {
        let mut f = fixture();
        f.owners[2].push(Tag::integer(field, 0));
        rejected_source(&mut f);
    }
    for field in [
        "ActorClassName",
        "ActorPathName",
        "ObjectFName",
        "IcarusActorGUID",
        "RecordedVersion",
        "bWasMovedToSubLevel",
        "ActorTransform",
    ] {
        let mut f = fixture();
        f.owners[2].retain(|t| t.name != field);
        rejected_source(&mut f);
    }
    for child in ["Rotation", "Translation", "Scale3D"] {
        let mut f = fixture();
        let children = [
            Tag::structure("Rotation", "Quat", floats(&[0., 0., 0., 1.])),
            Tag::structure("Translation", "Vector", floats(&[0.; 3])),
            Tag::structure("Scale3D", "Vector", floats(&[1.; 3])),
        ];
        let transform = f.owners[2]
            .iter_mut()
            .find(|t| t.name == "ActorTransform")
            .unwrap();
        transform.payload = stream(
            &children
                .into_iter()
                .filter(|t| t.name != child)
                .collect::<Vec<_>>(),
        );
        rejected_source(&mut f);
    }
    let mut f = fixture();
    f.owners[2]
        .iter_mut()
        .find(|t| t.name == "RecordedVersion")
        .unwrap()
        .payload = 2_i32.to_le_bytes().to_vec();
    rejected_source(&mut f);
    let mut f = fixture();
    f.roots[0].payload = 1_i32.to_le_bytes().to_vec();
    rejected_source(&mut f);
    let mut f = fixture();
    let mut tag = policy_numeric(1);
    tag.extras = [string("OtherPolicy"), vec![0]].concat();
    set_policy(&mut f, 2, Some(tag));
    rejected_source(&mut f);
    let mut f = fixture();
    let mut tag = policy_named("EnumProperty", "RespawnOnly");
    tag.payload = [
        (-12_i32).to_le_bytes().to_vec(),
        "RespawnOnly"
            .encode_utf16()
            .chain([0])
            .flat_map(u16::to_le_bytes)
            .collect(),
    ]
    .concat();
    set_policy(&mut f, 2, Some(tag));
    rejected_source(&mut f);
}

#[test]
fn last_owner_failure_leaves_base_and_destination_unchanged() {
    let mut f = fixture();
    let original = f.base.clone();
    let directory = tempfile::TempDir::new().unwrap();
    let destination = directory.path().join("unopened");
    f.bindings.owners[2].create_if_missing = false;
    assert!(prepare(&f).is_err());
    assert_eq!(f.base, original);
    assert!(!destination.exists());
    for index in [0, 1] {
        let mut f = fixture();
        f.bindings.owners[index].create_if_missing = false;
        assert!(prepare(&f).is_err());
    }
    let mut f = fixture();
    set_policy(&mut f, 0, Some(policy_numeric(2)));
    f.bindings.owners[0].create_if_missing = false;
    assert!(prepare(&f).is_err()); // Live FindOrRespawn attachment failure is terminal.
    assert_eq!(
        f.base.native_lifecycle.as_ref().unwrap().slots[2].incarnation,
        1
    );
}

#[test]
fn explicit_prior_binding_collisions_and_folded_dormant_ambiguity_reject() {
    let mut f = fixture();
    f.bindings.owners[0].expected_prior_binding = Some(OwnerHandle {
        id: DomainId(3),
        incarnation: 1,
    });
    assert!(prepare(&f).is_err());
    let mut f = fixture();
    f.bindings.owners[0].native_recorder_guid = 41;
    f.bindings.owners[0].expected_prior_binding = Some(OwnerHandle {
        id: DomainId(2),
        incarnation: 1,
    });
    assert!(prepare(&f).is_err());
    let mut f = fixture();
    f.bindings.owners[2].native_recorder_guid = 41;
    f.bindings.owners[2].expected_prior_binding = Some(OwnerHandle {
        id: DomainId(2),
        incarnation: 1,
    });
    prepare(&f).unwrap(); // Same-slot initial incumbent can be explicitly named.
    let mut f = fixture();
    replace_lifecycle(&mut f, |d| d.slots[0].owner_name = units("findowner"));
    f.bindings.owners[1].native_owner_name = units("findowner");
    assert!(prepare(&f).is_err());
}

#[test]
fn exact_retry_preserves_pose_and_relationship_edits_and_rejects_old_or_changed_requests() {
    let f = fixture();
    let mut world = f.snapshot();
    world.native_world.as_mut().unwrap().actors[0]
        .transform
        .translation[0] += 2.;
    let relationships = world.native_relationships.as_ref().unwrap();
    let compiled = CompiledNativeRelationships::from_snapshot(
        relationships,
        world.native_world.as_ref().unwrap(),
    )
    .unwrap();
    compiled
        .reassign_foundation(
            world.native_relationships.as_mut().unwrap(),
            DomainId(3),
            Some(DomainId(1)),
        )
        .unwrap();
    let retry = prepare_owner_restore(&world, &f.initial_batch()).unwrap();
    assert!(retry.already_completed());
    assert_eq!(retry.snapshot(), &world);
    let mut changed = f.initial_batch();
    changed.recorders[0].transform.translation[0] += 1.;
    assert!(prepare_owner_restore(&world, &changed).is_err());
    let next = later_batch(&world, DomainId(1), 200, ResolvePolicy::FindOnly);
    world = prepare_owner_restore(&world, &next)
        .unwrap()
        .into_snapshot();
    assert_eq!(world.schema_version, SNAPSHOT_SCHEMA_C001_OWNERS);
    assert!(prepare_owner_restore(&world, &f.initial_batch()).is_err());
    assert_eq!(world.native_owner_import, f.snapshot().native_owner_import);
}

#[test]
fn later_guid_retirement_at_same_or_greater_incarnation_reloads() {
    for policy in [ResolvePolicy::FindOnly, ResolvePolicy::RespawnOnly] {
        let mut world = fixture().snapshot();
        let batch = later_batch(&world, DomainId(2), 999, policy);
        world = prepare_owner_restore(&world, &batch)
            .unwrap()
            .into_snapshot();
        let lifecycle = world.native_lifecycle.as_ref().unwrap();
        assert!(!lifecycle.bindings.iter().any(|b| b.guid == 102));
        assert_eq!(
            lifecycle.slots[1].incarnation,
            if policy == ResolvePolicy::RespawnOnly {
                3
            } else {
                2
            }
        );
        world.native_world.as_mut().unwrap().actors[1]
            .transform
            .translation[0] = 8.;
        world.validate().unwrap();
        let dir = tempfile::TempDir::new().unwrap();
        let mut store = SnapshotStore::open(dir.path()).unwrap();
        store.save_new(&world).unwrap();
        assert_eq!(store.load().unwrap().unwrap().snapshot, world);
    }
}

#[test]
fn provenance_receipts_and_current_incarnation_floor_are_authoritative() {
    let good = fixture().snapshot();
    let reject = |edit: fn(&mut C001OwnerImportMetadataV1)| {
        let mut world = good.clone();
        edit(world.native_owner_import.as_mut().unwrap());
        assert!(world.validate().is_err());
    };
    reject(|m| m.initial_completion.request_digest = "f".repeat(64));
    reject(|m| m.initial_completion.outcomes[0].disposition = RestoreDisposition::KeptBound);
    reject(|m| m.initial_completion.outcomes[0].owner.incarnation += 1);
    reject(|m| m.owners[0].source_policy = ResolvePolicy::FindOrRespawn);
    reject(|m| m.owners[0].binding.source_object_name = units("changed"));
    reject(|m| m.owners[0].binding.source_actor_class = units("changed"));
    reject(|m| m.owners[0].binding.source_actor_path = units("changed"));
    reject(|m| m.owners[0].binding.source_icarus_guid += 1);
    reject(|m| m.owners[0].source_index = 1);
    reject(|m| m.owners[0].id = DomainId(1));
    reject(|m| m.native_catalog_id = "f".repeat(64));
    reject(|m| m.native_lifecycle_id = "f".repeat(64));
    reject(|m| m.source_payload_sha256 = "f".repeat(64));
    reject(|m| m.source_last_saved_date_time = Some(12));
    reject(|m| m.root_presence.lobby_privacy = !m.root_presence.lobby_privacy);
    reject(|m| m.source_map = units("OtherMap"));
    reject(|m| m.source_prospect_id = units("OtherProspect"));
    reject(|m| m.source_lobby_name = units("OtherLobby"));
    reject(|m| m.owners[0].binding.expected_incarnation = 0);
    for missing in 0..3 {
        let mut world = good.clone();
        match missing {
            0 => world.native_lifecycle.as_mut().unwrap().restore_sequence = 0,
            1 => world.native_lifecycle.as_mut().unwrap().completed = None,
            _ => world.native_owner_import = None,
        }
        assert!(world.validate().is_err());
    }
    // Advance another slot, then roll back the initially replaced middle slot.
    // The latest completion remains locally valid, so only the initial floor
    // can reject this otherwise consistent lifecycle state.
    let next = later_batch(&good, DomainId(1), 900, ResolvePolicy::FindOnly);
    let mut rollback = prepare_owner_restore(&good, &next).unwrap().into_snapshot();
    let lifecycle = rollback.native_lifecycle.as_mut().unwrap();
    lifecycle.slots[1].incarnation = 1;
    lifecycle
        .bindings
        .iter_mut()
        .find(|b| b.owner.id == DomainId(2))
        .unwrap()
        .owner
        .incarnation = 1;
    CompiledNativeLifecycle::from_snapshot(lifecycle, rollback.native_world.as_ref().unwrap())
        .unwrap();
    assert!(rollback.validate().is_err());
}

#[test]
fn persisted_source_revalidation_rejects_unsupported_policy_and_state_even_with_matching_digest() {
    for variant in 0..4 {
        let mut f = fixture();
        let mut world = f.snapshot();
        match variant {
            0 => set_policy(&mut f, 2, Some(policy_numeric(3))),
            1 => f.owners[2].push(Tag::array(
                "BoolVariables",
                "BoolProperty",
                0_i32.to_le_bytes().to_vec(),
            )),
            2 => f.owners[2].push(Tag::boolean("bSkipDefaultOwnerResolution", true)),
            _ => f.info[0] = Tag::text("ProspectID", "StrProperty", &units("different")),
        }
        f.rebuild_source();
        let preserved =
            PreservedSection::from_inner_payload(&f.payload, PreserveLimits::default()).unwrap();
        world
            .native_owner_import
            .as_mut()
            .unwrap()
            .source_payload_sha256 = f.bindings.expected_payload_sha256;
        world.preserved_prospect = Some(Arc::new(preserved));
        assert!(world.validate().is_err());
    }
}

fn strict_object<T: serde::Serialize + serde::de::DeserializeOwned + std::fmt::Debug>(value: &T) {
    let Value::Object(mut object) = serde_json::to_value(value).unwrap() else {
        panic!("object fixture")
    };
    let positional = Value::Array(object.values().cloned().collect());
    assert!(serde_json::from_value::<T>(positional).is_err());
    object.insert("DO_NOT_EXPOSE_FIELD".into(), json!("DO_NOT_EXPOSE_VALUE"));
    let error = serde_json::from_value::<T>(Value::Object(object))
        .unwrap_err()
        .to_string();
    assert!(!error.contains("DO_NOT_EXPOSE"));
}
#[test]
fn new_named_dtos_and_adapter_are_strict_objects_and_redact_diagnostics() {
    let f = fixture();
    let world = f.snapshot();
    let metadata = world.native_owner_import.as_ref().unwrap();
    strict_object::<C001OwnerBindingsV1>(&f.bindings);
    strict_object::<C001OwnerBindingV1>(&f.bindings.owners[0]);
    strict_object::<ResolvedC001OwnerV1>(&metadata.owners[0]);
    strict_object::<C001OwnerImportMetadataV1>(metadata);
    for value in [
        json!("DO_NOT_EXPOSE_ADAPTER"),
        json!(7),
        json!(["DO_NOT_EXPOSE_ADAPTER"]),
    ] {
        let error = serde_json::from_value::<C001OwnerAdapter>(value)
            .unwrap_err()
            .to_string();
        assert!(!error.contains("DO_NOT_EXPOSE"));
    }
    let mut value = serde_json::to_value(&f.bindings).unwrap();
    value["owners"][0]["policy"] = json!("respawn_only");
    assert!(serde_json::from_value::<C001OwnerBindingsV1>(value).is_err());
    let debug = format!(
        "{metadata:?} {:?} {:?} {:?}",
        f.bindings, f.bindings.owners[0], metadata.owners[0]
    );
    for secret in [
        "AuthoredMap",
        "/Authored/Owner0",
        "SourceCrate",
        "FindOwner",
        &f.bindings.expected_payload_sha256,
    ] {
        assert!(!debug.contains(secret));
    }
    let mut f = fixture();
    f.bindings.owners[2].create_if_missing = false;
    let error = prepare(&f).unwrap_err();
    assert!(error.to_string().starts_with("C001 authored owner import"));
    let old = SemanticImportError {
        recorder_index: Some(2),
        field: "owner.binding",
        category: SemanticImportCategory::Binding,
    };
    assert_eq!(
        old.to_string(),
        "C001 inert import Binding: field owner.binding, recorder Some(2)"
    );
}

#[test]
fn bindings_json_toml_regular_file_and_capacity_bounds_agree() {
    let f = fixture();
    for text in [
        serde_json::to_string(&f.bindings).unwrap(),
        toml::to_string(&f.bindings).unwrap(),
    ] {
        let parsed = CompiledC001OwnerBindings::parse(&text).unwrap();
        assert_eq!(parsed.definition(), &f.bindings);
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("bindings");
        fs::write(&path, text).unwrap();
        assert_eq!(
            CompiledC001OwnerBindings::load(&path).unwrap().definition(),
            &f.bindings
        );
        std::os::unix::fs::symlink(&path, dir.path().join("link")).unwrap();
        CompiledC001OwnerBindings::load(&dir.path().join("link")).unwrap();
        assert!(CompiledC001OwnerBindings::load(dir.path()).is_err());
    }
    let mut boundary = f.bindings.clone();
    boundary.source_map = vec![0xd800; 1024];
    boundary.owners[0].source_actor_class = vec![0xd800; 1024];
    boundary.owners[0].native_owner_name = vec![0xd800; 1024];
    for text in [
        serde_json::to_string(&boundary).unwrap(),
        toml::to_string(&boundary).unwrap(),
    ] {
        CompiledC001OwnerBindings::parse(&text).unwrap();
    }
    let mut bad = f.bindings.clone();
    bad.source_map.reserve(10000);
    assert!(CompiledC001OwnerBindings::compile(bad).is_err());
    let mut bad = f.bindings.clone();
    bad.owners.reserve(MAX_NATIVE_PLACEMENTS + 1);
    assert!(CompiledC001OwnerBindings::compile(bad).is_err());
    let mut bad = f.bindings.clone();
    bad.owners[0].source_actor_path.reserve(10000);
    assert!(CompiledC001OwnerBindings::compile(bad).is_err());
    let mut bad = f.bindings.clone();
    bad.expected_payload_sha256.reserve(1000);
    assert!(CompiledC001OwnerBindings::compile(bad).is_err());
    let mut bad = f.bindings.clone();
    bad.owners[0].native_placement.reserve(1000);
    assert!(CompiledC001OwnerBindings::compile(bad).is_err());
    let mut bad = f.bindings.clone();
    bad.owners[1].native_recorder_guid = bad.owners[0].native_recorder_guid;
    assert!(CompiledC001OwnerBindings::compile(bad).is_err());
}

#[test]
fn programmatic_base_capacity_is_rejected_before_native_cloning_and_publication_is_exclusive() {
    let f = fixture();
    let mut base = f.base.clone();
    base.native_world.as_mut().unwrap().actors[0]
        .placement
        .reserve(10000);
    let error = prepare_c001_owner_import(
        &f.candidate(),
        base,
        &CompiledC001OwnerBindings::compile(f.bindings.clone()).unwrap(),
    )
    .unwrap_err();
    assert_eq!(error.category, SemanticImportCategory::Limit);
    let mut base = f.base.clone();
    base.native_lifecycle.as_mut().unwrap().definition.slots[0]
        .owner_name
        .reserve(10000);
    assert!(
        prepare_c001_owner_import(
            &f.candidate(),
            base,
            &CompiledC001OwnerBindings::compile(f.bindings.clone()).unwrap()
        )
        .is_err()
    );
    let directory = tempfile::TempDir::new().unwrap();
    let destination = directory.path().join("world");
    publish_new_c001_owner_import(f.prepared(), &destination).unwrap();
    assert!(publish_new_c001_owner_import(f.prepared(), &destination).is_err());
}

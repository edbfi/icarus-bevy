//! Storage fixtures are independently authored; no original or private inputs.
use std::{
    collections::BTreeMap,
    fs,
    path::Path,
    time::{Duration, Instant},
};

use icarus_bevy::{
    native_lifecycle::{
        Presence, RecorderKind, RecorderSource, ResolvePolicy, RestoreBatch, RestoreRecorder,
        prepare_owner_restore,
    },
    native_relationships::CompiledNativeRelationships,
    persistence::{SaveWorker, SnapshotStore},
    preservation::is_sidecar_name,
    simulation::{DomainId, EntityRecord, Position, SNAPSHOT_SCHEMA_C001_OWNERS, WorldSnapshot},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

#[path = "common/c001_owners.rs"]
mod fixture;

const SECRET: &str = "OwnerStorageSecret";

#[test]
fn review_future_source_encoding_refuses_rollback_and_preserves_storage() {
    for encoding in [json!(2), json!(u64::MAX)] {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let initial = state(10, 0, true);
        let current = state(20, 0, true);
        let mut store = SnapshotStore::open(root).unwrap();
        store.save(&initial).unwrap();
        store.save(&current).unwrap();
        let mut wire = read_json(&root.join("snapshot-2.json"));
        wire["preserved_prospect"]["encoding"] = encoding;
        rewrite(root, 2, &wire);
        let unchanged = disk(root);
        for error in [
            store.load().unwrap_err(),
            store.save(&current).unwrap_err(),
            store.save_new(&current).unwrap_err(),
        ] {
            assert!(format!("{error:#}").contains("unsupported native snapshot schema"));
            assert_eq!(disk(root), unchanged);
        }
    }
}

#[test]
fn review_later_receipt_cannot_hide_an_imported_owner_losing_its_recorder() {
    let initial = state(10, 0, true);
    let current = restore(&initial, 0, ResolvePolicy::FindOnly, 20);
    let mut damaged = current.clone();
    let life = damaged.native_lifecycle.as_mut().unwrap();
    let guid = life.slots[1].component.take().unwrap().guid;
    life.bindings.retain(|binding| binding.guid != guid);
    assert!(damaged.validate().is_err());
    assert!(
        icarus_bevy::simulation::build_app(&icarus_bevy::config::ServerConfig::default(), &damaged)
            .is_err()
    );
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let mut store = SnapshotStore::open(root).unwrap();
    store.save(&initial).unwrap();
    store.save(&current).unwrap();
    let unchanged = disk(root);
    assert!(store.save(&damaged).is_err());
    assert_eq!(disk(root), unchanged);
    let mut wire = read_json(&root.join("snapshot-2.json"));
    wire["native_lifecycle"]["slots"][1]["component"] = Value::Null;
    wire["native_lifecycle"]["bindings"]
        .as_array_mut()
        .unwrap()
        .retain(|binding| binding["guid"] != guid);
    rewrite(root, 2, &wire);
    let recovered = store.load().unwrap().unwrap();
    assert!(recovered.recovered_previous);
    assert_eq!(recovered.snapshot, initial);
}

fn state(tick: u64, variant: u8, relationships: bool) -> WorldSnapshot {
    let mut state = fixture::imported_variant(variant);
    state.tick = tick;
    if !relationships {
        state.native_relationships = None;
    }
    state
}

fn read_json(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

fn rewrite_bytes(root: &Path, number: u64, bytes: &[u8]) {
    fs::write(root.join(format!("snapshot-{number}.json")), bytes).unwrap();
    let mut manifest = read_json(&root.join("manifest.json"));
    for slot in ["current", "previous"] {
        if manifest[slot]["number"] == number {
            manifest[slot]["sha256"] = Sha256::digest(bytes)
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
                .into();
        }
    }
    fs::write(
        root.join("manifest.json"),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();
}

fn rewrite(root: &Path, number: u64, value: &Value) {
    rewrite_bytes(root, number, &serde_json::to_vec(value).unwrap());
}

fn disk(root: &Path) -> BTreeMap<String, Vec<u8>> {
    fs::read_dir(root)
        .unwrap()
        .map(|entry| {
            let entry = entry.unwrap();
            (
                entry.file_name().into_string().unwrap(),
                fs::read(entry.path()).unwrap(),
            )
        })
        .collect()
}

fn assert_private(error: &anyhow::Error) {
    assert!(!format!("{error:#?} {error:#}").contains(SECRET));
}

fn later_batch(state: &WorldSnapshot, slot_index: usize, policy: ResolvePolicy) -> RestoreBatch {
    let lifecycle = state.native_lifecycle.as_ref().unwrap();
    let slot = &lifecycle.slots[slot_index];
    let declaration = &lifecycle.definition.slots[slot_index];
    let actor = state
        .native_world
        .as_ref()
        .unwrap()
        .actors
        .iter()
        .find(|actor| actor.id == slot.id)
        .unwrap();
    let sequence = lifecycle.restore_sequence + 1;
    let mut transform = actor.transform.clone();
    transform.translation[0] += 0.25;
    RestoreBatch {
        lifecycle_id: lifecycle.lifecycle_id.clone(),
        sequence,
        recorders: vec![RestoreRecorder {
            guid: 10000 + i32::try_from(sequence).unwrap(),
            kind: RecorderKind::SpatialStateV1,
            target_placement: declaration.placement.clone(),
            owner_name: declaration.owner_name.clone(),
            policy,
            source: RecorderSource::Detached {
                expected_prior_binding: None,
            },
            create_if_missing: true,
            expected_presence: Presence::Live,
            expected_incarnation: slot.incarnation,
            transform,
            variables: vec![],
        }],
    }
}

fn restore(
    state: &WorldSnapshot,
    slot_index: usize,
    policy: ResolvePolicy,
    tick: u64,
) -> WorldSnapshot {
    let mut next = prepare_owner_restore(state, &later_batch(state, slot_index, policy))
        .unwrap()
        .snapshot()
        .clone();
    next.tick = tick;
    next
}

fn edit_relationship(state: &mut WorldSnapshot, target: DomainId) {
    let compiled = CompiledNativeRelationships::from_snapshot(
        state.native_relationships.as_ref().unwrap(),
        state.native_world.as_ref().unwrap(),
    )
    .unwrap();
    compiled
        .reassign_foundation(
            state.native_relationships.as_mut().unwrap(),
            DomainId(3),
            Some(target),
        )
        .unwrap();
}

#[test]
fn schema_eight_roundtrips_source_provenance_complete_owners_and_optional_relationships() {
    for relationships in [false, true] {
        let initial = state(17, 1, relationships);
        assert_eq!(initial.schema_version, SNAPSHOT_SCHEMA_C001_OWNERS);
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        SnapshotStore::preflight(&initial).unwrap();
        assert!(disk(root).is_empty());
        let mut store = SnapshotStore::open(root).unwrap();
        assert_eq!(store.save_new(&initial).unwrap().generation, 1);
        let written = read_json(&root.join("snapshot-1.json"));
        assert_eq!(written["schema_version"], 8);
        assert_eq!(
            written["native_owner_import"]["adapter"],
            "c001_authored_owners_v1"
        );
        assert!(written.get("native_import").is_none());
        assert_eq!(written.get("native_relationships").is_some(), relationships);
        assert!(written["preserved_prospect"]["encoded_sha256"].is_string());
        assert!(written["preserved_prospect"].get("data").is_none());
        assert_eq!(
            disk(root)
                .keys()
                .filter(|name| is_sidecar_name(name))
                .count(),
            1
        );
        drop(store);
        let store = SnapshotStore::open(root).unwrap();
        let loaded = store.load().unwrap().unwrap();
        assert!(!loaded.recovered_previous);
        assert_eq!(loaded.snapshot, initial);
        let lifecycle = loaded.snapshot.native_lifecycle.as_ref().unwrap();
        assert_eq!(lifecycle.restore_sequence, 1);
        assert!(
            lifecycle
                .slots
                .iter()
                .all(|slot| slot.presence == Presence::Live)
        );
        assert_eq!(lifecycle.slots[1].incarnation, 2);
    }
}

#[test]
fn edited_poses_links_and_initial_retry_survive_worker_publication_without_replay() {
    let fixture = fixture::fixture();
    let batch = fixture.initial_batch();
    let mut edited = fixture.snapshot();
    edited.tick = 20;
    edited.native_world.as_mut().unwrap().actors[0]
        .transform
        .translation[0] += 1.0;
    edit_relationship(&mut edited, DomainId(1));
    let retry = prepare_owner_restore(&edited, &batch).unwrap();
    assert!(retry.already_completed());
    assert_eq!(retry.snapshot(), &edited);
    let mut changed = batch.clone();
    changed.recorders[0].transform.translation[0] += 0.5;
    assert!(prepare_owner_restore(&edited, &changed).is_err());
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let mut store = SnapshotStore::open(root).unwrap();
    store.save(&fixture.snapshot()).unwrap();
    let first = disk(root);
    let mut worker = SaveWorker::spawn(store).unwrap();
    worker.submit(edited.clone()).unwrap();
    assert_eq!(
        worker
            .finish_before(Instant::now() + Duration::from_secs(10))
            .unwrap()
            .unwrap()
            .generation,
        2
    );
    drop(worker);
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut reopened = loop {
        match SnapshotStore::open(root) {
            Ok(store) => break store,
            Err(error) => {
                assert!(Instant::now() < deadline, "{error:#}");
                std::thread::sleep(Duration::from_millis(1));
            }
        }
    };
    let loaded = reopened.load().unwrap().unwrap().snapshot;
    assert_eq!(loaded, edited);
    assert_eq!(
        prepare_owner_restore(&loaded, &batch).unwrap().snapshot(),
        &edited
    );
    let after = disk(root);
    for (name, bytes) in first.iter().filter(|(name, _)| is_sidecar_name(name)) {
        assert_eq!(after.get(name), Some(bytes));
    }
    let mut next = loaded;
    next.tick = 30;
    reopened.save(&next).unwrap();
    assert_eq!(reopened.load().unwrap().unwrap().snapshot, next);
}

#[test]
fn later_detached_attachment_and_replacement_retire_initial_guids_and_reload_current_receipts() {
    let initial = state(10, 0, true);
    for policy in [ResolvePolicy::FindOrRespawn, ResolvePolicy::RespawnOnly] {
        let initial_guid = initial.native_lifecycle.as_ref().unwrap().slots[1]
            .component
            .as_ref()
            .unwrap()
            .guid;
        let incarnation = initial.native_lifecycle.as_ref().unwrap().slots[1].incarnation;
        let current = restore(&initial, 1, policy, 20);
        let lifecycle = current.native_lifecycle.as_ref().unwrap();
        assert_eq!(lifecycle.restore_sequence, 2);
        assert_eq!(
            lifecycle.slots[1].incarnation,
            incarnation + u64::from(policy == ResolvePolicy::RespawnOnly)
        );
        assert!(
            lifecycle
                .bindings
                .iter()
                .all(|binding| binding.guid != initial_guid)
        );
        assert_eq!(current.native_owner_import, initial.native_owner_import);
        let directory = tempfile::tempdir().unwrap();
        let mut store = SnapshotStore::open(directory.path()).unwrap();
        store.save(&initial).unwrap();
        store.save(&current).unwrap();
        drop(store);
        let store = SnapshotStore::open(directory.path()).unwrap();
        let loaded = store.load().unwrap().unwrap();
        assert_eq!(loaded.snapshot, current);
        assert!(
            prepare_owner_restore(&loaded.snapshot, &fixture::fixture().initial_batch()).is_err()
        );
    }
}

#[test]
fn every_future_discriminator_wins_over_corruption_and_missing_sidecars() {
    for path in [
        "/schema_version",
        "/preserved_prospect/encoding",
        "/native_world/definitions/native_format",
        "/native_world/definitions/frame",
        "/native_world/definitions/classes/0/kind",
        "/native_world/definitions/classes/0/variables/0/default/type",
        "/native_world/definitions/maps/0/placements/0/initial_values/0/value/type",
        "/native_world/actors/0/variables/0/value/type",
        "/native_relationships/definition/relationship_format",
        "/native_relationships/definition/actors/0/role",
        "/native_lifecycle/definition/lifecycle_format",
        "/native_lifecycle/definition/slots/0/recorder_kind",
        "/native_lifecycle/definition/slots/0/initial_presence",
        "/native_lifecycle/definition/slots/1/initial_component/kind",
        "/native_lifecycle/definition/slots/0/spawn_component",
        "/native_lifecycle/slots/0/presence",
        "/native_lifecycle/slots/0/component/kind",
        "/native_lifecycle/bindings/0/kind",
        "/native_lifecycle/completed/outcomes/0/policy",
        "/native_lifecycle/completed/outcomes/0/source/type",
        "/native_lifecycle/completed/outcomes/0/before_presence",
        "/native_lifecycle/completed/outcomes/0/disposition",
        "/native_lifecycle/completed/outcomes/0/component_disposition",
        "/native_owner_import/format",
        "/native_owner_import/adapter",
        "/native_owner_import/coordinates",
        "/native_owner_import/owners/0/source_policy",
        "/native_owner_import/owners/0/binding/expected_presence",
        "/native_owner_import/initial_completion/outcomes/0/policy",
        "/native_owner_import/initial_completion/outcomes/0/source/type",
        "/native_owner_import/initial_completion/outcomes/0/before_presence",
        "/native_owner_import/initial_completion/outcomes/0/disposition",
        "/native_owner_import/initial_completion/outcomes/0/component_disposition",
    ] {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let before = state(10, 0, true);
        let mut current = before.clone();
        current.tick = 20;
        let mut store = SnapshotStore::open(root).unwrap();
        store.save(&before).unwrap();
        store.save(&current).unwrap();
        let mut wire = read_json(&root.join("snapshot-2.json"));
        wire["native_world"]["definitions"]["classes"][0]["variables"] =
            json!([{"default":{"type":"i32","value":0}}]);
        wire["native_world"]["definitions"]["maps"][0]["placements"][0]["initial_values"] =
            json!([{"value":{"type":"i32","value":0}}]);
        wire["native_world"]["actors"][0]["variables"] =
            json!([{"value":{"type":"i32","value":0}}]);
        *wire.pointer_mut(path).unwrap() = if path == "/schema_version" {
            10.into()
        } else if path.ends_with("_format")
            || path == "/native_owner_import/format"
            || path == "/preserved_prospect/encoding"
        {
            2.into()
        } else {
            format!("{SECRET}FutureEnum").into()
        };
        // A distinct section is corrupt, independently of the upgrade path.
        if path.starts_with("/native_owner_import/") {
            wire["native_world"]["definitions"]["classes"] = Value::Null;
            wire["native_lifecycle"]["definition"] = json!([]);
            wire["native_relationships"] = json!([]);
        } else {
            wire["native_owner_import"]["root_presence"] = json!([]);
        }
        rewrite(root, 2, &wire);
        for name in disk(root).keys().filter(|name| is_sidecar_name(name)) {
            fs::remove_file(root.join(name)).unwrap();
        }
        let unchanged = disk(root);
        current.tick = 30;
        for error in [
            store.load().unwrap_err(),
            store.save(&current).unwrap_err(),
            store.save_new(&current).unwrap_err(),
        ] {
            assert!(
                format!("{error:#}").contains("unsupported native snapshot schema"),
                "{path}: {error:#}"
            );
            assert_private(&error);
            assert_eq!(disk(root), unchanged);
        }
    }
}

#[test]
fn malformed_earlier_array_members_cannot_hide_later_future_discriminators() {
    for (array_path, discriminator) in [
        ("/native_world/definitions/classes", "/kind"),
        (
            "/native_world/definitions/classes/0/variables",
            "/default/type",
        ),
        (
            "/native_world/definitions/maps/0/placements/0/initial_values",
            "/value/type",
        ),
        ("/native_world/actors/0/variables", "/value/type"),
        ("/native_relationships/definition/actors", "/role"),
        ("/native_lifecycle/definition/slots", "/recorder_kind"),
        ("/native_lifecycle/slots", "/presence"),
        ("/native_lifecycle/bindings", "/kind"),
        ("/native_lifecycle/completed/outcomes", "/policy"),
        ("/native_owner_import/owners", "/source_policy"),
        ("/native_owner_import/owners", "/binding/expected_presence"),
        (
            "/native_owner_import/initial_completion/outcomes",
            "/disposition",
        ),
    ] {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let initial = state(10, 0, true);
        let mut store = SnapshotStore::open(root).unwrap();
        store.save(&initial).unwrap();
        let mut current = initial.clone();
        current.tick = 20;
        store.save(&current).unwrap();
        let mut wire = read_json(&root.join("snapshot-2.json"));
        let member = match discriminator {
            "/default/type" => json!({"default":{"type":format!("{SECRET}Future")}}),
            "/value/type" => json!({"value":{"type":format!("{SECRET}Future")}}),
            "/binding/expected_presence" => {
                json!({"binding":{"expected_presence":format!("{SECRET}Future")}})
            }
            _ => json!({(discriminator.trim_start_matches('/')): format!("{SECRET}Future")}),
        };
        *wire.pointer_mut(array_path).unwrap() = json!([null, member]);
        rewrite(root, 2, &wire);
        let unchanged = disk(root);
        let error = store.load().unwrap_err();
        assert_private(&error);
        assert!(
            format!("{error:#}").contains("unsupported native snapshot schema"),
            "{array_path}: {error:#}"
        );
        assert_eq!(disk(root), unchanged);
    }
}

#[test]
fn malformed_known_discriminators_recover_previous_instead_of_requiring_an_upgrade() {
    for (path, malformed) in [
        ("/preserved_prospect/encoding", json!(0)),
        (
            "/preserved_prospect/encoding",
            json!("OwnerStorageSecretEncoding"),
        ),
        ("/preserved_prospect/encoding", json!({})),
        (
            "/native_world/definitions/native_format",
            json!("OwnerStorageSecretFormat"),
        ),
        ("/native_world/definitions/frame", json!(9)),
        ("/native_world/definitions/classes/0/kind", json!([])),
        ("/native_lifecycle/definition/lifecycle_format", json!(null)),
        ("/native_lifecycle/slots/0/presence", json!(5)),
        (
            "/native_relationships/definition/relationship_format",
            json!("OwnerStorageSecretFormat"),
        ),
        (
            "/native_relationships/definition/actors/0/role",
            json!(false),
        ),
        (
            "/native_owner_import/format",
            json!("OwnerStorageSecretFormat"),
        ),
        ("/native_owner_import/adapter", json!({})),
        ("/native_owner_import/coordinates", json!(false)),
        ("/native_owner_import/owners/0/source_policy", json!(1)),
        (
            "/native_owner_import/owners/0/binding/expected_presence",
            json!([]),
        ),
        (
            "/native_owner_import/initial_completion/outcomes/0/source/type",
            json!(false),
        ),
    ] {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let initial = state(10, 0, true);
        let mut current = initial.clone();
        current.tick = 20;
        let mut store = SnapshotStore::open(root).unwrap();
        store.save(&initial).unwrap();
        store.save(&current).unwrap();
        let mut wire = read_json(&root.join("snapshot-2.json"));
        *wire.pointer_mut(path).unwrap() = malformed;
        rewrite(root, 2, &wire);
        let unchanged = disk(root);
        let recovered = store.load().unwrap().unwrap();
        assert!(recovered.recovered_previous, "{path}");
        assert_eq!(recovered.snapshot, initial);
        assert_eq!(disk(root), unchanged);
        assert_private(&store.save_new(&current).unwrap_err());
        assert_eq!(disk(root), unchanged);
    }
}

#[test]
fn malformed_objects_and_missing_required_state_recover_privately() {
    for (path, required) in [
        ("", "tick"),
        ("/native_world", "actors"),
        ("/native_lifecycle", "slots"),
        ("/native_relationships", "foundations"),
        ("/preserved_prospect", "encoding"),
        ("/native_owner_import", "owners"),
        ("/native_owner_import/root_presence", "version"),
        ("/native_owner_import/owners/0", "binding"),
        ("/native_owner_import/owners/0/binding", "source_actor_path"),
        ("/native_owner_import/initial_completion", "request_digest"),
        ("/native_owner_import/initial_completion/outcomes/0", "guid"),
        (
            "/native_owner_import/initial_completion/outcomes/0/source",
            "type",
        ),
        (
            "/native_owner_import/initial_completion/outcomes/0/owner",
            "incarnation",
        ),
    ] {
        for mutation in 0..3 {
            let directory = tempfile::tempdir().unwrap();
            let root = directory.path();
            let initial = state(10, 0, true);
            let mut current = initial.clone();
            current.tick = 20;
            let mut store = SnapshotStore::open(root).unwrap();
            store.save(&initial).unwrap();
            store.save(&current).unwrap();
            let mut wire = read_json(&root.join("snapshot-2.json"));
            let object = wire.pointer_mut(path).unwrap();
            match mutation {
                0 => {
                    *object = Value::Array(object.as_object().unwrap().values().cloned().collect())
                }
                1 => {
                    object[format!("{SECRET}ForeignField")] = format!("{SECRET}ForeignValue").into()
                }
                2 => {
                    object.as_object_mut().unwrap().remove(required);
                }
                _ => unreachable!(),
            }
            rewrite(root, 2, &wire);
            let unchanged = disk(root);
            let recovered = store.load().unwrap().unwrap();
            assert!(recovered.recovered_previous, "{path}, mutation {mutation}");
            assert_eq!(recovered.snapshot, initial);
            assert_eq!(disk(root), unchanged);
            assert_private(&store.save_new(&current).unwrap_err());
            assert_eq!(disk(root), unchanged);
        }
    }
}

#[test]
fn schema_eight_requires_a_sidecar_reference_and_forbids_the_old_metadata_key() {
    for mutation in 0..7 {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let initial = state(10, 0, true);
        let mut current = initial.clone();
        current.tick = 20;
        let mut store = SnapshotStore::open(root).unwrap();
        store.save(&initial).unwrap();
        store.save(&current).unwrap();
        let mut wire = read_json(&root.join("snapshot-2.json"));
        match mutation {
            0 => {
                wire.as_object_mut().unwrap().remove("native_owner_import");
            }
            1 => {
                wire.as_object_mut().unwrap().remove("preserved_prospect");
            }
            2 => wire["native_import"] = Value::Null,
            3 => wire["native_import"] = json!({"format":1}),
            4 => {
                wire["preserved_prospect"] =
                    serde_json::to_value(&current).unwrap()["preserved_prospect"].clone()
            }
            5 => wire["preserved_prospect"] = Value::Null,
            6 => wire["native_owner_import"] = Value::Null,
            _ => unreachable!(),
        }
        rewrite(root, 2, &wire);
        let unchanged = disk(root);
        let recovered = store.load().unwrap().unwrap();
        assert!(recovered.recovered_previous);
        assert_eq!(recovered.snapshot, initial);
        assert_eq!(disk(root), unchanged);
    }
}

#[test]
fn schemas_one_through_seven_reject_even_null_owner_metadata_on_disk() {
    for schema in 1..=7 {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let initial = state(10, 0, false);
        let mut current = initial.clone();
        current.tick = 20;
        let mut store = SnapshotStore::open(root).unwrap();
        store.save(&initial).unwrap();
        store.save(&current).unwrap();
        let mut wire = read_json(&root.join("snapshot-2.json"));
        wire["schema_version"] = schema.into();
        wire["native_owner_import"] = Value::Null;
        // Remove fields introduced after each old schema, leaving only its
        // ordinary representation plus the deliberately forbidden new key.
        if schema <= 6 {
            wire.as_object_mut().unwrap().remove("native_lifecycle");
        }
        if schema <= 3 {
            wire.as_object_mut().unwrap().remove("native_world");
        }
        if schema == 1 {
            wire.as_object_mut().unwrap().remove("preserved_prospect");
        }
        if schema == 2 {
            wire["preserved_prospect"] =
                serde_json::to_value(&current).unwrap()["preserved_prospect"].clone();
        }
        if schema == 5 || schema == 6 {
            // The unit storage DTO test separately isolates the forbidden field
            // against valid generations for every earlier schema.
            wire["native_import"] = Value::Null;
        }
        rewrite(root, 2, &wire);
        let unchanged = disk(root);
        let recovered = store.load().unwrap().unwrap();
        assert!(recovered.recovered_previous, "schema {schema}");
        assert_eq!(recovered.snapshot, initial);
        assert_eq!(disk(root), unchanged);
    }
}

#[test]
fn damaged_source_sidecars_recover_previous_and_later_collection_keeps_exact_retained_sources() {
    for remove in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let initial = state(10, 1, true);
        let current = state(20, 2, true);
        let mut store = SnapshotStore::open(root).unwrap();
        store.save(&initial).unwrap();
        let first_names: Vec<_> = disk(root)
            .into_keys()
            .filter(|name| is_sidecar_name(name))
            .collect();
        store.save(&current).unwrap();
        let current_sidecar = disk(root)
            .into_keys()
            .find(|name| is_sidecar_name(name) && !first_names.contains(name))
            .unwrap();
        if remove {
            fs::remove_file(root.join(&current_sidecar)).unwrap();
        } else {
            fs::write(
                root.join(&current_sidecar),
                b"OwnerStorageSecretDamagedSource",
            )
            .unwrap();
        }
        let unchanged = disk(root);
        let recovered = store.load().unwrap().unwrap();
        assert!(recovered.recovered_previous);
        assert_eq!(recovered.snapshot, initial);
        assert_eq!(disk(root), unchanged);
        assert_private(&store.save_new(&current).unwrap_err());
        assert_eq!(disk(root), unchanged);
        let next = state(30, 3, true);
        store.save(&next).unwrap();
        assert_eq!(store.load().unwrap().unwrap().snapshot, next);
        let last = state(40, 3, true);
        store.save(&last).unwrap();
        assert_eq!(
            disk(root)
                .keys()
                .filter(|name| is_sidecar_name(name))
                .count(),
            1
        );
        for name in first_names {
            assert!(!root.join(name).exists());
        }
        assert!(!root.join(current_sidecar).exists());
    }
}

#[test]
fn save_new_refuses_existing_recovered_and_orphan_stores_without_changing_operator_files() {
    for corruption in 0..3 {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let initial = state(10, 0, true);
        let current = state(20, 1, true);
        let mut store = SnapshotStore::open(root).unwrap();
        store.save(&initial).unwrap();
        store.save(&current).unwrap();
        fs::write(
            root.join("OwnerStorageSecretOperatorFile"),
            b"OwnerStorageSecretContents",
        )
        .unwrap();
        match corruption {
            0 => {}
            1 => fs::write(root.join("snapshot-2.json"), b"damaged").unwrap(),
            2 => fs::remove_file(root.join("manifest.json")).unwrap(),
            _ => unreachable!(),
        }
        let unchanged = disk(root);
        assert_private(&store.save_new(&current).unwrap_err());
        assert_eq!(disk(root), unchanged);
    }
}

#[test]
fn known_provenance_tampering_and_erased_completions_recover_the_exact_previous_world() {
    for (path, replacement) in [
        (
            "/native_owner_import/source_payload_sha256",
            json!("0".repeat(64)),
        ),
        (
            "/native_owner_import/native_catalog_id",
            json!("0".repeat(64)),
        ),
        (
            "/native_owner_import/native_lifecycle_id",
            json!("0".repeat(64)),
        ),
        (
            "/native_owner_import/native_map",
            json!("OwnerStorageSecretOtherMap"),
        ),
        ("/native_owner_import/source_map", json!([43])),
        ("/native_owner_import/source_prospect_id", json!([43])),
        ("/native_owner_import/source_lobby_name", json!([43])),
        (
            "/native_owner_import/source_last_saved_date_time",
            json!(99),
        ),
        ("/native_owner_import/root_presence/version", json!(false)),
        ("/native_owner_import/owners/0/source_index", json!(2)),
        ("/native_owner_import/owners/0/id", json!(1)),
        (
            "/native_owner_import/owners/0/binding/source_actor_class",
            json!([43]),
        ),
        (
            "/native_owner_import/owners/0/binding/source_actor_path",
            json!([43]),
        ),
        (
            "/native_owner_import/owners/0/binding/source_object_name",
            json!([43]),
        ),
        (
            "/native_owner_import/owners/0/binding/source_icarus_guid",
            json!(900),
        ),
        (
            "/native_owner_import/owners/0/binding/native_class",
            json!("OwnerStorageSecretOtherClass"),
        ),
        (
            "/native_owner_import/owners/0/binding/native_owner_name",
            json!([43]),
        ),
        (
            "/native_owner_import/owners/0/binding/native_recorder_guid",
            json!(900),
        ),
        (
            "/native_owner_import/owners/0/source_policy",
            json!("respawn_only"),
        ),
        (
            "/native_owner_import/initial_completion/request_digest",
            json!("0".repeat(64)),
        ),
        (
            "/native_owner_import/initial_completion/outcomes/0/disposition",
            json!("kept_bound"),
        ),
        (
            "/native_owner_import/initial_completion/outcomes/0/guid",
            json!(900),
        ),
        ("/native_lifecycle/restore_sequence", json!(0)),
        ("/native_lifecycle/completed", Value::Null),
    ] {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let initial = state(10, 0, true);
        let mut current = initial.clone();
        current.tick = 20;
        let mut store = SnapshotStore::open(root).unwrap();
        store.save(&initial).unwrap();
        store.save(&current).unwrap();
        let mut wire = read_json(&root.join("snapshot-2.json"));
        assert_ne!(wire.pointer(path).unwrap(), &replacement, "{path}");
        *wire.pointer_mut(path).unwrap() = replacement;
        rewrite(root, 2, &wire);
        let unchanged = disk(root);
        let loaded = store.load().unwrap().unwrap();
        assert!(loaded.recovered_previous, "{path}");
        assert_eq!(loaded.snapshot, initial);
        assert_eq!(disk(root), unchanged);
    }
}

#[test]
fn initial_replacement_incarnation_floor_holds_when_the_latest_receipt_targets_another_slot() {
    let initial = state(10, 0, true);
    let current = restore(&initial, 0, ResolvePolicy::FindOrRespawn, 20);
    assert_eq!(
        current.native_lifecycle.as_ref().unwrap().slots[1].incarnation,
        2
    );
    assert_eq!(
        current
            .native_lifecycle
            .as_ref()
            .unwrap()
            .completed
            .as_ref()
            .unwrap()
            .outcomes[0]
            .target,
        DomainId(1)
    );
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let mut store = SnapshotStore::open(root).unwrap();
    store.save(&initial).unwrap();
    store.save(&current).unwrap();
    let mut wire = read_json(&root.join("snapshot-2.json"));
    wire["native_lifecycle"]["slots"][1]["incarnation"] = 1.into();
    for binding in wire["native_lifecycle"]["bindings"].as_array_mut().unwrap() {
        if binding["owner"]["id"] == 2 {
            binding["owner"]["incarnation"] = 1.into();
        }
    }
    rewrite(root, 2, &wire);
    let unchanged = disk(root);
    let recovered = store.load().unwrap().unwrap();
    assert!(recovered.recovered_previous);
    assert_eq!(recovered.snapshot, initial);
    assert_eq!(disk(root), unchanged);
}

#[test]
fn invalid_schema_eight_preflight_and_publication_leave_every_destination_unchanged() {
    let initial = state(10, 0, true);
    let mut invalid = Vec::new();
    let mut changed = initial.clone();
    changed.native_owner_import = None;
    invalid.push(changed);
    let mut changed = initial.clone();
    changed.preserved_prospect = None;
    invalid.push(changed);
    let mut changed = initial.clone();
    changed.native_world = None;
    invalid.push(changed);
    let mut changed = initial.clone();
    changed.native_lifecycle = None;
    invalid.push(changed);
    let mut changed = initial.clone();
    changed.preserved_prospect = fixture::imported_variant(9).preserved_prospect;
    invalid.push(changed);
    let mut changed = initial.clone();
    changed.native_lifecycle.as_mut().unwrap().restore_sequence = 0;
    invalid.push(changed);
    let mut changed = initial.clone();
    changed.native_lifecycle.as_mut().unwrap().completed = None;
    invalid.push(changed);
    let mut changed = initial.clone();
    changed.native_world.as_mut().unwrap().actors.pop();
    invalid.push(changed);
    let mut changed = initial.clone();
    changed.native_lifecycle.as_mut().unwrap().bindings.pop();
    invalid.push(changed);
    let mut changed = initial.clone();
    changed
        .native_owner_import
        .as_mut()
        .unwrap()
        .initial_completion
        .request_digest = "0".repeat(64);
    invalid.push(changed);
    let mut changed = initial.clone();
    changed.native_owner_import.as_mut().unwrap().owners[0]
        .binding
        .source_actor_path = fixture::units("OwnerStorageSecretOtherPath");
    invalid.push(changed);
    let mut changed = initial.clone();
    changed.entities.push(EntityRecord {
        id: DomainId(99),
        position: Position([0.0; 3]),
    });
    invalid.push(changed);
    for schema in 1..=7 {
        let mut changed = initial.clone();
        changed.schema_version = schema;
        invalid.push(changed);
    }
    for changed in invalid {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        assert_private(&SnapshotStore::preflight(&changed).unwrap_err());
        assert!(disk(root).is_empty());
        let mut store = SnapshotStore::open(root).unwrap();
        let empty = disk(root);
        assert_private(&store.save_new(&changed).unwrap_err());
        assert_eq!(disk(root), empty);
        store.save(&initial).unwrap();
        let committed = disk(root);
        assert_private(&store.save(&changed).unwrap_err());
        assert_eq!(disk(root), committed);
    }
}

#[test]
fn excessive_retained_owner_metadata_allocations_fail_before_store_writes() {
    let initial = state(10, 0, false);
    let mut variants = Vec::new();
    let mut changed = initial.clone();
    changed
        .native_owner_import
        .as_mut()
        .unwrap()
        .source_map
        .reserve_exact(4 * 1024 * 1024);
    variants.push(changed);
    let mut changed = initial.clone();
    changed
        .native_owner_import
        .as_mut()
        .unwrap()
        .owners
        .reserve_exact(icarus_bevy::native_world::MAX_NATIVE_PLACEMENTS + 1);
    variants.push(changed);
    let mut changed = initial.clone();
    changed
        .native_owner_import
        .as_mut()
        .unwrap()
        .initial_completion
        .outcomes
        .reserve_exact(icarus_bevy::native_world::MAX_NATIVE_PLACEMENTS + 1);
    variants.push(changed);
    for changed in variants {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        assert_private(&SnapshotStore::preflight(&changed).unwrap_err());
        assert!(disk(root).is_empty());
        let mut store = SnapshotStore::open(root).unwrap();
        let empty = disk(root);
        assert_private(&store.save_new(&changed).unwrap_err());
        assert_eq!(disk(root), empty);
    }
}

#[test]
fn schema_eight_generation_keeps_the_existing_aggregate_read_cap() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let initial = state(10, 0, false);
    let mut current = initial.clone();
    current.tick = 20;
    let mut store = SnapshotStore::open(root).unwrap();
    store.save(&initial).unwrap();
    store.save(&current).unwrap();
    let mut bytes = fs::read(root.join("snapshot-2.json")).unwrap();
    bytes.resize(64 * 1024 * 1024 + 4 * 1024 + 1, b' ');
    rewrite_bytes(root, 2, &bytes);
    drop(bytes);
    let manifest = fs::read(root.join("manifest.json")).unwrap();
    let loaded = store.load().unwrap().unwrap();
    assert!(loaded.recovered_previous);
    assert_eq!(loaded.snapshot, initial);
    assert_eq!(fs::read(root.join("manifest.json")).unwrap(), manifest);
    assert_eq!(
        fs::metadata(root.join("snapshot-2.json")).unwrap().len(),
        64 * 1024 * 1024 + 4 * 1024 + 1
    );
}

//! Storage fixtures are independently authored; no original or private inputs.
use std::{
    collections::BTreeMap,
    fs,
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};

use icarus_bevy::{
    native_lifecycle::{
        CompiledNativeLifecycle, LifecycleDefinition, Presence, RecorderComponentRecord,
        RecorderKind, RecorderSource, ResolvePolicy, RestoreBatch, RestoreRecorder,
        SlotDeclaration, SpawnComponent, prepare_lifecycle_world, prepare_owner_restore,
    },
    native_relationships::{
        CompiledNativeRelationships, NativeActorLinkV1, NativeRelationshipActorV1,
        NativeRelationshipRole, NativeRelationshipsDefinition, prepare_relationship_world,
    },
    native_world::{CompiledNativeCatalog, prepare_authored_world},
    persistence::{SaveWorker, SnapshotStore},
    preservation::{PreserveLimits, PreservedSection, is_sidecar_name},
    simulation::{DomainId, SNAPSHOT_SCHEMA_LIFECYCLE, SNAPSHOT_SCHEMA_NATIVE, WorldSnapshot},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const CATALOG: &str = r#"
native_format = 1
frame = "right_handed_z_up_meters"
[[classes]]
key = "box"
kind = "spatial_object_v1"
local_bounds = { min = [-0.5, -0.5, -0.5], max = [0.5, 0.5, 0.5] }
variables = [{ key = "charge", default = { type = "i32", value = 1 } }]
[[maps]]
key = "demo"
[[maps.tiles]]
key = "tile"
bounds = { min = [-10.0, -10.0, -10.0], max = [10.0, 10.0, 10.0] }
[[maps.placements]]
key = "a_live"
class = "box"
tile = "tile"
transform = { translation = [0.0, 0.0, 0.0], rotation = [0.0, 0.0, 0.0, 1.0], scale = [1.0, 1.0, 1.0] }
[[maps.placements]]
key = "b_dormant"
class = "box"
tile = "tile"
transform = { translation = [1.0, 0.0, 0.0], rotation = [0.0, 0.0, 0.0, 1.0], scale = [1.0, 1.0, 1.0] }
"#;

#[test]
fn admitted_large_name_aggregate_survives_durable_reload() {
    let catalog = CompiledNativeCatalog::parse(CATALOG).unwrap();
    let mut definition = catalog.definition().clone();
    let placement = definition.maps[0].placements[0].clone();
    definition.maps[0].placements = (0..4097)
        .map(|index| {
            let mut placement = placement.clone();
            placement.key = format!("slot_{index:05}");
            placement
        })
        .collect();
    let catalog = Arc::new(CompiledNativeCatalog::compile(definition).unwrap());
    let native = prepare_authored_world(Arc::clone(&catalog), "demo").unwrap();
    let compiled = CompiledNativeLifecycle::compile(
        LifecycleDefinition {
            lifecycle_format: 1,
            native_catalog_id: catalog.identity().into(),
            map: "demo".into(),
            slots: native
                .snapshot()
                .actors
                .iter()
                .map(|actor| SlotDeclaration {
                    placement: actor.placement.clone(),
                    owner_name: vec![65; 200],
                    recorder_kind: RecorderKind::SpatialStateV1,
                    initial_presence: Presence::Absent,
                    initial_component: None,
                    spawn_component: SpawnComponent::Missing,
                })
                .collect(),
        },
        &catalog,
    )
    .unwrap();
    let world = prepare_lifecycle_world(native.initial_snapshot(), &compiled).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let mut store = SnapshotStore::open(directory.path()).unwrap();
    store.save_new(world.snapshot()).unwrap();
    drop(store);
    let store = SnapshotStore::open(directory.path()).unwrap();
    let loaded = store
        .load()
        .expect("an acknowledged name aggregate must reload")
        .unwrap();
    assert_eq!(&loaded.snapshot, world.snapshot());
    assert!(!loaded.recovered_previous);
}

#[test]
fn admitted_catalog_placements_across_maps_survive_durable_reload() {
    assert_catalog_vectors_reload(false);
}

#[test]
fn admitted_catalog_tiles_across_maps_survive_durable_reload() {
    assert_catalog_vectors_reload(true);
}

fn assert_catalog_vectors_reload(tiles: bool) {
    let original = CompiledNativeCatalog::parse(CATALOG).unwrap();
    let mut definition = original.definition().clone();
    let mut large = definition.maps[0].clone();
    large.key = "large".into();
    let mut selected = definition.maps[0].clone();
    selected.key = "selected".into();
    selected.placements.truncate(1);
    if tiles {
        let tile = large.tiles[0].clone();
        large.tiles = (0..2049)
            .map(|index| {
                let mut tile = tile.clone();
                tile.key = format!("tile_{index:05}");
                tile
            })
            .collect();
        large.placements.truncate(1);
        large.placements[0].tile = large.tiles[0].key.clone();
    } else {
        let placement = large.placements[0].clone();
        large.placements = (0..8193)
            .map(|index| {
                let mut placement = placement.clone();
                placement.key = format!("slot_{index:05}");
                placement
            })
            .collect();
    }
    definition.maps = vec![large, selected];
    let catalog = Arc::new(CompiledNativeCatalog::compile(definition).unwrap());
    let native = prepare_authored_world(Arc::clone(&catalog), "selected").unwrap();
    let compiled = CompiledNativeLifecycle::compile(
        LifecycleDefinition {
            lifecycle_format: 1,
            native_catalog_id: catalog.identity().into(),
            map: "selected".into(),
            slots: vec![SlotDeclaration {
                placement: native.snapshot().actors[0].placement.clone(),
                owner_name: vec![65],
                recorder_kind: RecorderKind::SpatialStateV1,
                initial_presence: Presence::Live,
                initial_component: None,
                spawn_component: SpawnComponent::Missing,
            }],
        },
        &catalog,
    )
    .unwrap();
    let world = prepare_lifecycle_world(native.initial_snapshot(), &compiled).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let mut store = SnapshotStore::open(directory.path()).unwrap();
    store.save_new(world.snapshot()).unwrap();
    drop(store);
    let store = SnapshotStore::open(directory.path()).unwrap();
    let loaded = store
        .load()
        .expect("acknowledged catalog vectors must reload")
        .unwrap();
    assert_eq!(&loaded.snapshot, world.snapshot());
    assert!(!loaded.recovered_previous);
}

fn fixture(tick: u64, preserve: bool, relationships: bool) -> WorldSnapshot {
    let catalog = Arc::new(CompiledNativeCatalog::parse(CATALOG).unwrap());
    let native = prepare_authored_world(Arc::clone(&catalog), "demo")
        .unwrap()
        .snapshot()
        .clone();
    let compiled = CompiledNativeLifecycle::compile(
        LifecycleDefinition {
            lifecycle_format: 1,
            native_catalog_id: catalog.identity().into(),
            map: "demo".into(),
            slots: native
                .actors
                .iter()
                .enumerate()
                .map(|(index, actor)| SlotDeclaration {
                    placement: actor.placement.clone(),
                    owner_name: format!("LifecycleStorageSecretOwner{index}")
                        .encode_utf16()
                        .chain([0xd800])
                        .collect(),
                    recorder_kind: RecorderKind::SpatialStateV1,
                    initial_presence: if index == 0 {
                        Presence::Live
                    } else {
                        Presence::Absent
                    },
                    initial_component: (index == 0).then_some(RecorderComponentRecord {
                        kind: RecorderKind::SpatialStateV1,
                        guid: 11,
                    }),
                    spawn_component: if index == 0 {
                        SpawnComponent::DeclaredKind
                    } else {
                        SpawnComponent::Missing
                    },
                })
                .collect(),
        },
        &catalog,
    )
    .unwrap();
    let mut base = WorldSnapshot {
        schema_version: SNAPSHOT_SCHEMA_NATIVE,
        tick,
        native_world: Some(native),
        preserved_prospect: preserve.then(|| section(1)),
        ..Default::default()
    };
    if relationships {
        let model = CompiledNativeRelationships::compile(
            NativeRelationshipsDefinition {
                relationship_format: 1,
                native_catalog_id: catalog.identity().into(),
                map: "demo".into(),
                actors: base
                    .native_world
                    .as_ref()
                    .unwrap()
                    .actors
                    .iter()
                    .enumerate()
                    .map(|(index, actor)| NativeRelationshipActorV1 {
                        placement: actor.placement.clone(),
                        identity: NativeActorLinkV1 {
                            name: vec![65],
                            guid: index as i32 + 1,
                        },
                        role: if index == 0 {
                            NativeRelationshipRole::Foundation
                        } else {
                            NativeRelationshipRole::Deployable
                        },
                        foundation: None,
                    })
                    .collect(),
            },
            &catalog,
        )
        .unwrap();
        base = prepare_relationship_world(base, &model)
            .unwrap()
            .snapshot()
            .clone();
    }
    prepare_lifecycle_world(base, &compiled)
        .unwrap()
        .snapshot()
        .clone()
}

fn replacement_batch(base: &WorldSnapshot) -> RestoreBatch {
    let lifecycle = base.native_lifecycle.as_ref().unwrap();
    let sequence = lifecycle
        .completed
        .as_ref()
        .map_or(1, |completed| completed.sequence + 1);
    RestoreBatch {
        lifecycle_id: lifecycle.lifecycle_id.clone(),
        sequence,
        recorders: lifecycle
            .slots
            .iter()
            .enumerate()
            .map(|(index, slot)| {
                let actor = &base.native_world.as_ref().unwrap().actors[index];
                let mut transform = actor.transform.clone();
                transform.translation[2] = sequence as f32;
                RestoreRecorder {
                    guid: 100 + i32::try_from(sequence).unwrap() * 2 + index as i32,
                    kind: RecorderKind::SpatialStateV1,
                    target_placement: actor.placement.clone(),
                    owner_name: lifecycle.definition.slots[index].owner_name.clone(),
                    policy: ResolvePolicy::RespawnOnly,
                    source: RecorderSource::Detached {
                        expected_prior_binding: None,
                    },
                    create_if_missing: true,
                    expected_presence: slot.presence,
                    expected_incarnation: slot.incarnation,
                    transform,
                    variables: actor.variables.clone(),
                }
            })
            .collect(),
    }
}

fn replacement(base: &WorldSnapshot, tick: u64) -> WorldSnapshot {
    let mut snapshot = prepare_owner_restore(base, &replacement_batch(base))
        .unwrap()
        .snapshot()
        .clone();
    snapshot.tick = tick;
    snapshot
}

fn string(value: &str) -> Vec<u8> {
    [
        i32::try_from(value.len() + 1)
            .unwrap()
            .to_le_bytes()
            .as_slice(),
        value.as_bytes(),
        &[0],
    ]
    .concat()
}

fn tag(name: &str, kind: &str, extra: &[u8], value: &[u8]) -> Vec<u8> {
    [
        string(name).as_slice(),
        &string(kind),
        &i32::try_from(value.len()).unwrap().to_le_bytes(),
        &[0; 4],
        extra,
        &[0],
        value,
    ]
    .concat()
}

fn section(variant: u8) -> Arc<PreservedSection> {
    let record = [
        tag(
            "ComponentClassName",
            "StrProperty",
            &[],
            &string("LifecycleStorageSecret"),
        ),
        tag(
            "BinaryData",
            "ArrayProperty",
            &string("ByteProperty"),
            &[8_i32.to_le_bytes().as_slice(), &[variant; 8]].concat(),
        ),
        string("None"),
    ]
    .concat();
    let shared = tag(
        "StateRecorderBlobs",
        "StructProperty",
        &[string("StateRecorderBlob"), vec![7; 16]].concat(),
        &record,
    );
    let payload = [
        tag(
            "StateRecorderBlobs",
            "ArrayProperty",
            &string("StructProperty"),
            &[1_i32.to_le_bytes().as_slice(), &shared].concat(),
        ),
        string("None"),
    ]
    .concat();
    Arc::new(PreservedSection::from_inner_payload(&payload, PreserveLimits::default()).unwrap())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn read_json(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

fn rewrite_bytes(root: &Path, number: u64, bytes: &[u8]) {
    fs::write(root.join(format!("snapshot-{number}.json")), bytes).unwrap();
    let mut manifest = read_json(&root.join("manifest.json"));
    for slot in ["current", "previous"] {
        if manifest[slot]["number"] == number {
            manifest[slot]["sha256"] = hex(&Sha256::digest(bytes)).into();
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
    let text = format!("{error:#?} {error:#}");
    assert!(!text.contains("LifecycleStorageSecret"));
}

#[test]
fn schema_seven_roundtrips_complete_live_and_dormant_slots_and_optional_sections() {
    for preserve in [false, true] {
        for relationships in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            let root = directory.path();
            let state = fixture(10, preserve, relationships);
            SnapshotStore::preflight(&state).unwrap();
            assert!(disk(root).is_empty());
            let mut store = SnapshotStore::open(root).unwrap();
            assert_eq!(store.save_new(&state).unwrap().generation, 1);
            let written = read_json(&root.join("snapshot-1.json"));
            assert_eq!(written["schema_version"], SNAPSHOT_SCHEMA_LIFECYCLE);
            assert_eq!(written["entities"], json!([]));
            assert_eq!(
                written["native_world"]["actors"].as_array().unwrap().len(),
                2
            );
            assert_eq!(written["native_lifecycle"]["slots"][0]["presence"], "live");
            assert_eq!(
                written["native_lifecycle"]["slots"][1]["presence"],
                "absent"
            );
            assert_eq!(written.get("native_relationships").is_some(), relationships);
            assert_eq!(written.get("preserved_prospect").is_some(), preserve);
            assert!(written.get("native_import").is_none());
            if preserve {
                assert!(written["preserved_prospect"].get("data").is_none());
                assert_eq!(
                    disk(root)
                        .keys()
                        .filter(|name| is_sidecar_name(name))
                        .count(),
                    1
                );
            }
            assert_eq!(store.load().unwrap().unwrap().snapshot, state);
            drop(store);
            let loaded = SnapshotStore::open(root).unwrap().load().unwrap().unwrap();
            assert!(!loaded.recovered_previous);
            assert_eq!(loaded.snapshot, state);
        }
    }
}

#[test]
fn completed_replacement_and_spawn_survive_worker_save_and_reload_retry_is_idempotent() {
    for preserve in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let initial = fixture(10, preserve, true);
        let batch = replacement_batch(&initial);
        let mut completed = replacement(&initial, 20);
        let state = completed.native_lifecycle.as_ref().unwrap();
        assert_eq!(state.slots[0].incarnation, 2);
        assert_eq!(state.slots[1].incarnation, 1);
        assert!(
            state
                .slots
                .iter()
                .all(|slot| slot.presence == Presence::Live)
        );
        assert!(state.bindings.iter().all(|binding| binding.guid != 11));
        let model = CompiledNativeRelationships::from_snapshot(
            completed.native_relationships.as_ref().unwrap(),
            completed.native_world.as_ref().unwrap(),
        )
        .unwrap();
        model
            .reassign_foundation(
                completed.native_relationships.as_mut().unwrap(),
                DomainId(2),
                Some(DomainId(1)),
            )
            .unwrap();
        let mut store = SnapshotStore::open(root).unwrap();
        store.save(&initial).unwrap();
        let first_disk = disk(root);
        let mut worker = SaveWorker::spawn(store).unwrap();
        worker.submit(completed.clone()).unwrap();
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
        let loaded = reopened.load().unwrap().unwrap();
        assert_eq!(loaded.snapshot, completed);
        let retry = prepare_owner_restore(&loaded.snapshot, &batch).unwrap();
        assert!(retry.already_completed());
        assert_eq!(retry.snapshot(), &completed);
        let unchanged = disk(root);
        for (name, bytes) in first_disk.iter().filter(|(name, _)| is_sidecar_name(name)) {
            assert_eq!(unchanged.get(name), Some(bytes));
        }
        let mut next = retry.snapshot().clone();
        next.tick = 30;
        reopened.save(&next).unwrap();
        assert_eq!(reopened.load().unwrap().unwrap().snapshot, next);
        assert_eq!(
            disk(root)
                .keys()
                .filter(|name| is_sidecar_name(name))
                .count(),
            usize::from(preserve)
        );
    }
}

#[test]
fn every_future_discriminator_is_terminal_beside_corruption_and_missing_sidecars() {
    for path in [
        "/schema_version",
        "/native_world/definitions/native_format",
        "/native_world/definitions/frame",
        "/native_world/definitions/classes/0/kind",
        "/native_world/definitions/classes/0/variables/0/default/type",
        "/native_world/actors/0/variables/0/value/type",
        "/native_relationships/definition/relationship_format",
        "/native_relationships/definition/actors/0/role",
        "/native_lifecycle/definition/lifecycle_format",
        "/native_lifecycle/definition/slots/0/recorder_kind",
        "/native_lifecycle/definition/slots/0/initial_presence",
        "/native_lifecycle/definition/slots/0/initial_component/kind",
        "/native_lifecycle/definition/slots/0/spawn_component",
        "/native_lifecycle/slots/0/presence",
        "/native_lifecycle/slots/0/component/kind",
        "/native_lifecycle/bindings/0/kind",
        "/native_lifecycle/completed/outcomes/0/policy",
        "/native_lifecycle/completed/outcomes/0/source/type",
        "/native_lifecycle/completed/outcomes/0/before_presence",
        "/native_lifecycle/completed/outcomes/0/disposition",
        "/native_lifecycle/completed/outcomes/0/component_disposition",
    ] {
        for preserve in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            let root = directory.path();
            let before = fixture(10, preserve, true);
            let current = replacement(&before, 20);
            let next = replacement(&current, 30);
            let mut store = SnapshotStore::open(root).unwrap();
            store.save(&before).unwrap();
            store.save(&current).unwrap();
            let mut value = read_json(&root.join("snapshot-2.json"));
            *value.pointer_mut(path).unwrap() = if path == "/schema_version" {
                8.into()
            } else if path.ends_with("_format") {
                2.into()
            } else {
                "LifecycleStorageSecretFutureEnum".into()
            };
            if path.starts_with("/native_lifecycle/") {
                value["native_world"]["definitions"]["classes"] = Value::Null;
                value["native_relationships"] = json!([]);
                if path.ends_with("/lifecycle_format") {
                    value["native_lifecycle"]["definition"]["slots"] = json!({"changed": true});
                    value["native_lifecycle"]["slots"] = Value::Null;
                } else {
                    value["native_lifecycle"]["lifecycle_id"] = json!([]);
                }
            } else if path.starts_with("/native_relationships/") {
                value["native_world"]["definitions"]["classes"] = Value::Null;
                value["native_lifecycle"]["definition"] = json!([]);
                if path.ends_with("/relationship_format") {
                    value["native_relationships"]["definition"]["actors"] = Value::Null;
                }
            } else if path.starts_with("/native_world/") {
                value["native_lifecycle"]["definition"] = json!([]);
                value["native_relationships"] = Value::Null;
                if path.ends_with("/native_format") {
                    value["native_world"]["definitions"]["classes"] = Value::Null;
                }
            }
            rewrite(root, 2, &value);
            for name in disk(root).keys().filter(|name| is_sidecar_name(name)) {
                fs::remove_file(root.join(name)).unwrap();
            }
            let unchanged = disk(root);
            for error in [
                store.load().unwrap_err(),
                store.save(&next).unwrap_err(),
                store.save_new(&next).unwrap_err(),
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
}

#[test]
fn review_malformed_relationship_actor_cannot_hide_later_future_role() {
    for schema in [6, 7] {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let mut before = fixture(10, false, true);
        let mut current = replacement(&before, 20);
        if schema == 6 {
            before.schema_version = 6;
            before.native_lifecycle = None;
            current.schema_version = 6;
            current.native_lifecycle = None;
        }
        let mut store = SnapshotStore::open(root).unwrap();
        store.save(&before).unwrap();
        store.save(&current).unwrap();
        let mut value = read_json(&root.join("snapshot-2.json"));
        value["native_relationships"]["definition"]["actors"][0] = Value::Null;
        value["native_relationships"]["definition"]["actors"][1]["role"] = json!("future_role");
        rewrite(root, 2, &value);
        let unchanged = disk(root);
        let mut next = current;
        next.tick = 30;
        for result in [
            store.load().map(|_| ()),
            store.save(&next).map(|_| ()),
            store.save_new(&next).map(|_| ()),
        ] {
            let error = result.unwrap_err();
            assert!(format!("{error:#}").contains("unsupported native snapshot schema"));
            assert_eq!(disk(root), unchanged);
        }
    }
}

#[test]
fn malformed_known_state_recovers_previous_exactly_without_modifying_storage() {
    for path in [
        "/native_world",
        "/native_lifecycle",
        "/entities",
        "/tick",
        "/native_lifecycle/definition",
        "/native_lifecycle/definition/lifecycle_format",
        "/native_lifecycle/definition/slots/0/recorder_kind",
        "/native_lifecycle/definition/slots/0/initial_presence",
        "/native_lifecycle/definition/slots/0/initial_component/kind",
        "/native_lifecycle/definition/slots/0/spawn_component",
        "/native_lifecycle/slots",
        "/native_lifecycle/slots/0/presence",
        "/native_lifecycle/slots/0/incarnation",
        "/native_lifecycle/slots/0/component/kind",
        "/native_lifecycle/bindings",
        "/native_lifecycle/bindings/0/kind",
        "/native_lifecycle/completed",
        "/native_relationships",
        "/preserved_prospect",
    ] {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let before = fixture(10, true, true);
        let mut current = replacement(&before, 20);
        let mut store = SnapshotStore::open(root).unwrap();
        store.save(&before).unwrap();
        store.save(&current).unwrap();
        let mut value = read_json(&root.join("snapshot-2.json"));
        *value.pointer_mut(path).unwrap() = json!({"LifecycleStorageSecretType": true});
        rewrite(root, 2, &value);
        let unchanged = disk(root);
        let recovered = store.load().unwrap().unwrap();
        assert!(recovered.recovered_previous, "{path}");
        assert_eq!(recovered.snapshot, before, "{path}");
        assert_eq!(disk(root), unchanged);
        current.tick = 30;
        store.save(&current).unwrap();
        assert_eq!(store.load().unwrap().unwrap().snapshot, current);
    }
}

#[test]
fn corrupt_incarnations_bindings_and_completion_recover_the_exact_previous_completion() {
    for damage in [
        "lifecycle_id",
        "catalog_id",
        "definition",
        "map",
        "missing_slot",
        "duplicate_slot",
        "incarnation",
        "component_guid",
        "missing_binding",
        "duplicate_binding",
        "binding_handle",
        "missing_completion",
        "missing_restore_sequence",
        "restore_sequence",
        "completion_sequence",
        "completion_digest",
        "completion_handle",
        "completion_transition",
        "sidecar",
    ] {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let before = replacement(&fixture(0, true, true), 10);
        let mut current = replacement(&before, 20);
        current.preserved_prospect = Some(section(2));
        let mut store = SnapshotStore::open(root).unwrap();
        store.save(&before).unwrap();
        store.save(&current).unwrap();
        let mut value = read_json(&root.join("snapshot-2.json"));
        match damage {
            "lifecycle_id" => value["native_lifecycle"]["lifecycle_id"] = "0".repeat(64).into(),
            "catalog_id" => {
                value["native_lifecycle"]["definition"]["native_catalog_id"] = "0".repeat(64).into()
            }
            "definition" => {
                value["native_lifecycle"]["definition"]["slots"][0]["owner_name"] = json!([70])
            }
            "map" => {
                value["native_lifecycle"]["definition"]["map"] = "LifecycleStorageSecretMap".into()
            }
            "missing_slot" => {
                value["native_lifecycle"]["slots"]
                    .as_array_mut()
                    .unwrap()
                    .pop();
            }
            "duplicate_slot" => {
                value["native_lifecycle"]["slots"][1] =
                    value["native_lifecycle"]["slots"][0].clone();
            }
            "incarnation" => value["native_lifecycle"]["slots"][0]["incarnation"] = 0.into(),
            "component_guid" => {
                value["native_lifecycle"]["slots"][0]["component"]["guid"] = 99.into()
            }
            "missing_binding" => {
                value["native_lifecycle"]["bindings"]
                    .as_array_mut()
                    .unwrap()
                    .pop();
            }
            "duplicate_binding" => {
                let binding = value["native_lifecycle"]["bindings"][0].clone();
                value["native_lifecycle"]["bindings"]
                    .as_array_mut()
                    .unwrap()
                    .push(binding);
            }
            "binding_handle" => {
                value["native_lifecycle"]["bindings"][0]["owner"]["incarnation"] = 1.into()
            }
            "missing_completion" => value["native_lifecycle"]["completed"] = Value::Null,
            "missing_restore_sequence" => {
                value["native_lifecycle"]
                    .as_object_mut()
                    .unwrap()
                    .remove("restore_sequence");
            }
            "restore_sequence" => value["native_lifecycle"]["restore_sequence"] = 0.into(),
            "completion_sequence" => value["native_lifecycle"]["completed"]["sequence"] = 0.into(),
            "completion_digest" => {
                value["native_lifecycle"]["completed"]["request_digest"] =
                    "LifecycleStorageSecretDigest".into()
            }
            "completion_handle" => {
                value["native_lifecycle"]["completed"]["outcomes"][0]["owner"]["incarnation"] =
                    1.into()
            }
            "completion_transition" => {
                value["native_lifecycle"]["completed"]["outcomes"][0]["disposition"] =
                    "attached".into()
            }
            "sidecar" => {
                let (name, _) = disk(root)
                    .into_iter()
                    .find(|(name, bytes)| {
                        is_sidecar_name(name)
                            && bytes.as_slice()
                                == current
                                    .preserved_prospect
                                    .as_ref()
                                    .unwrap()
                                    .native_encoding()
                    })
                    .unwrap();
                fs::write(root.join(name), b"damaged").unwrap();
            }
            _ => unreachable!(),
        }
        if damage != "sidecar" {
            rewrite(root, 2, &value);
        }
        let unchanged = disk(root);
        let recovered = store.load().unwrap().unwrap();
        assert!(recovered.recovered_previous, "{damage}");
        assert_eq!(recovered.snapshot, before, "{damage}");
        assert_eq!(disk(root), unchanged);
        let next = replacement(&recovered.snapshot, 30);
        store.save(&next).unwrap();
        assert_eq!(store.load().unwrap().unwrap().snapshot, next);
    }
}

#[test]
fn schema_seven_required_state_and_old_schema_lifecycle_exclusion_preflight_before_writes() {
    let state = fixture(10, true, true);
    let mut variants = Vec::new();
    let mut changed = state.clone();
    changed.native_world = None;
    variants.push(changed);
    let mut changed = state.clone();
    changed.native_lifecycle = None;
    variants.push(changed);
    let mut changed = state.clone();
    changed.native_lifecycle.as_mut().unwrap().lifecycle_id = "0".repeat(64);
    variants.push(changed);
    let mut changed = state.clone();
    changed.native_world.as_mut().unwrap().actors.pop();
    variants.push(changed);
    let mut changed = state.clone();
    changed.native_lifecycle.as_mut().unwrap().slots.pop();
    variants.push(changed);
    let mut changed = state.clone();
    changed.native_lifecycle.as_mut().unwrap().bindings[0]
        .owner
        .incarnation += 1;
    variants.push(changed);
    let mut changed = state.clone();
    changed
        .entities
        .push(icarus_bevy::simulation::EntityRecord {
            id: DomainId(99),
            position: icarus_bevy::simulation::Position([0.0; 3]),
        });
    variants.push(changed);
    let mut changed = state.clone();
    changed.native_import = Some(serde_json::from_value(json!({
        "format": 1, "adapter": "c001_inert_placement_v1",
        "coordinates": "right_handed_z_up_meters_quat_xyzw_world_space",
        "source_version": 2, "source_payload_sha256": "0".repeat(64),
        "native_catalog_id": state.native_world.as_ref().unwrap().catalog_id,
        "native_map": "demo", "source_map": [], "source_prospect_id": [],
        "source_lobby_name": [], "source_last_saved_date_time": null,
        "root_presence": { "version": false, "lobby_privacy": false,
            "prospect_info": false, "prospect_map_name": false, "last_saved_date_time": false,
            "legacy_prospect_id": false, "legacy_prospect_dt_key": false, "state_recorder_blobs": false },
        "owners": []
    })).unwrap());
    variants.push(changed);
    for schema in 1..=6 {
        let mut changed = state.clone();
        changed.schema_version = schema;
        variants.push(changed);
    }
    for invalid in variants {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        assert!(SnapshotStore::preflight(&invalid).is_err());
        assert!(disk(root).is_empty());
        let mut store = SnapshotStore::open(root).unwrap();
        let empty = disk(root);
        assert!(store.save_new(&invalid).is_err());
        assert_eq!(disk(root), empty);
        store.save(&state).unwrap();
        let committed = disk(root);
        assert!(store.save(&invalid).is_err());
        assert_eq!(disk(root), committed);
    }
}

#[test]
fn old_disk_field_sets_refuse_even_null_lifecycle_and_schema_seven_refuses_import_metadata() {
    for schema in 1..=7 {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let before = fixture(10, false, false);
        let mut store = SnapshotStore::open(root).unwrap();
        store.save(&before).unwrap();
        let mut current = before.clone();
        current.tick = 20;
        store.save(&current).unwrap();
        let mut value = read_json(&root.join("snapshot-2.json"));
        value["schema_version"] = schema.into();
        if schema == 7 {
            value["native_import"] = Value::Null;
        } else {
            value["native_lifecycle"] = Value::Null;
        }
        rewrite(root, 2, &value);
        let recovered = store.load().unwrap().unwrap();
        assert!(recovered.recovered_previous);
        assert_eq!(recovered.snapshot, before);
    }
}

#[test]
fn save_new_refuses_existing_and_recovered_destinations_and_keeps_foreign_named_files() {
    for corrupt in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let state = fixture(10, true, false);
        let mut store = SnapshotStore::open(root).unwrap();
        store.save(&state).unwrap();
        let current = replacement(&state, 20);
        store.save(&current).unwrap();
        let foreign = root.join("LifecycleStorageSecretOperatorObject");
        fs::write(&foreign, b"LifecycleStorageSecretBytes").unwrap();
        if corrupt {
            fs::write(root.join("snapshot-2.json"), b"damaged").unwrap();
        }
        let unchanged = disk(root);
        assert_private(&store.save_new(&current).unwrap_err());
        assert_eq!(disk(root), unchanged);
        let mut next = if corrupt { state } else { current };
        next.tick = 30;
        store.save(&next).unwrap();
        assert_eq!(fs::read(foreign).unwrap(), b"LifecycleStorageSecretBytes");
    }
}

#[test]
fn disk_objects_reject_arrays_foreign_fields_and_missing_required_known_state_privately() {
    for path in [
        "",
        "/native_lifecycle",
        "/native_lifecycle/definition",
        "/native_lifecycle/definition/slots/0",
        "/native_lifecycle/definition/slots/0/initial_component",
        "/native_lifecycle/slots/0",
        "/native_lifecycle/slots/0/component",
        "/native_lifecycle/bindings/0",
        "/native_lifecycle/bindings/0/owner",
        "/native_lifecycle/completed",
        "/native_lifecycle/completed/outcomes/0",
        "/native_lifecycle/completed/outcomes/0/source",
        "/native_lifecycle/completed/outcomes/0/owner",
    ] {
        for mutation in 0..3 {
            let directory = tempfile::tempdir().unwrap();
            let root = directory.path();
            let before = replacement(&fixture(0, false, false), 10);
            let mut current = before.clone();
            current.tick = 20;
            let mut store = SnapshotStore::open(root).unwrap();
            store.save(&before).unwrap();
            store.save(&current).unwrap();
            let mut value = read_json(&root.join("snapshot-2.json"));
            let selected = value.pointer_mut(path).unwrap();
            match mutation {
                0 => {
                    *selected =
                        Value::Array(selected.as_object().unwrap().values().cloned().collect())
                }
                1 => {
                    selected["LifecycleStorageSecretForeignKey"] =
                        "LifecycleStorageSecretValue".into()
                }
                2 => {
                    let required = match path {
                        "" => "tick",
                        "/native_lifecycle" => "slots",
                        "/native_lifecycle/definition" => "slots",
                        "/native_lifecycle/definition/slots/0" => "owner_name",
                        "/native_lifecycle/definition/slots/0/initial_component"
                        | "/native_lifecycle/slots/0/component" => "guid",
                        "/native_lifecycle/slots/0" => "incarnation",
                        "/native_lifecycle/bindings/0" => "owner",
                        "/native_lifecycle/bindings/0/owner" => "incarnation",
                        "/native_lifecycle/completed" => "sequence",
                        "/native_lifecycle/completed/outcomes/0" => "before_incarnation",
                        "/native_lifecycle/completed/outcomes/0/source" => "type",
                        "/native_lifecycle/completed/outcomes/0/owner" => "incarnation",
                        _ => unreachable!(),
                    };
                    selected.as_object_mut().unwrap().remove(required);
                }
                _ => unreachable!(),
            }
            rewrite(root, 2, &value);
            let unchanged = disk(root);
            let loaded = store.load().unwrap().unwrap();
            assert!(loaded.recovered_previous, "{path}, mutation {mutation}");
            assert_eq!(loaded.snapshot, before);
            assert_eq!(disk(root), unchanged);
            // Without a usable previous generation the complete diagnostic must
            // still exclude supplied unknown keys, enum strings and values.
            fs::write(root.join("snapshot-1.json"), b"damaged").unwrap();
            assert_private(&store.load().unwrap_err());
        }
    }
}

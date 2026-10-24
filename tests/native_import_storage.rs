//! Storage of independently authored C001 source bytes and native placements.
//! These fixtures make no claim about semantic compatibility with ICARUS.
use std::{
    collections::BTreeMap,
    fs,
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};

use icarus_bevy::{
    native_import::C001InertImportMetadataV1,
    native_world::{CompiledNativeCatalog, prepare_authored_world},
    persistence::{SaveWorker, SnapshotStore},
    preservation::{PreserveLimits, PreservedSection, is_sidecar_name},
    simulation::{DomainId, EntityRecord, Position, SNAPSHOT_SCHEMA_C001_INERT, WorldSnapshot},
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
variables = []
[[maps]]
key = "demo"
[[maps.tiles]]
key = "tile"
bounds = { min = [-10.0, -10.0, -10.0], max = [10.0, 10.0, 10.0] }
[[maps.placements]]
key = "box1"
class = "box"
tile = "tile"
transform = { translation = [0.0, 0.0, 0.0], rotation = [0.0, 0.0, 0.0, 1.0], scale = [1.0, 1.0, 1.0] }
"#;

fn units(value: &str) -> Vec<u16> {
    value.encode_utf16().collect()
}

fn string(value: &str) -> Vec<u8> {
    wide(&units(value))
}

fn wide(value: &[u16]) -> Vec<u8> {
    if value.is_empty() {
        return 0_i32.to_le_bytes().to_vec();
    }
    if value.iter().all(|unit| *unit <= 127) {
        return [
            i32::try_from(value.len() + 1)
                .unwrap()
                .to_le_bytes()
                .to_vec(),
            value.iter().map(|unit| *unit as u8).collect(),
            vec![0],
        ]
        .concat();
    }
    let count = -i32::try_from(value.len() + 1).unwrap();
    [
        count.to_le_bytes().as_slice(),
        &value
            .iter()
            .flat_map(|unit| unit.to_le_bytes())
            .collect::<Vec<_>>(),
        &[0, 0],
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

fn structure(name: &str, kind: &str, value: &[u8]) -> Vec<u8> {
    tag(
        name,
        "StructProperty",
        &[string(kind), vec![0; 16]].concat(),
        value,
    )
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn fixture(tick: u64, variant: u8) -> (WorldSnapshot, Vec<u8>) {
    let catalog = Arc::new(CompiledNativeCatalog::parse(CATALOG).unwrap());
    let native = prepare_authored_world(catalog, "demo")
        .unwrap()
        .snapshot()
        .clone();
    let source_map = units("AuthoredMap_\u{1f30d}");
    let source_prospect = units("AuthoredProspect_\u{03bb}");
    let source_lobby = units("AuthoredLobby_\u{1f680}");
    let source_class = units("AuthoredBoxClass");
    let source_path = units("/Authored/Map.Box1");
    // Preserve code units exactly, including an unpaired surrogate.
    let source_object = vec![0x41, 0xd800, u16::from(variant) + 0x30];
    let timestamp = 638_000_000_000_000_000_i64 + i64::from(variant);
    let floats = |values: &[f32]| {
        values
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect::<Vec<_>>()
    };
    let transform = [
        structure("Translation", "Vector", &floats(&[0.0; 3])),
        structure("Rotation", "Quat", &floats(&[0.0, 0.0, 0.0, 1.0])),
        structure("Scale3D", "Vector", &floats(&[1.0; 3])),
        string("None"),
    ]
    .concat();
    let body = [
        tag("ActorClassName", "NameProperty", &[], &wide(&source_class)),
        tag("ActorPathName", "StrProperty", &[], &wide(&source_path)),
        tag("ObjectFName", "NameProperty", &[], &wide(&source_object)),
        tag(
            "IcarusActorGUID",
            "IntProperty",
            &[],
            &i32::from(variant).to_le_bytes(),
        ),
        tag("RecordedVersion", "IntProperty", &[], &3_i32.to_le_bytes()),
        tag("bWasMovedToSubLevel", "BoolProperty", &[0], &[]),
        structure("ActorTransform", "Transform", &transform),
        string("None"),
        vec![0; 4],
    ]
    .concat();
    let record = [
        tag(
            "ComponentClassName",
            "StrProperty",
            &[],
            &string("/Script/Icarus.ActorStateRecorderComponent"),
        ),
        tag(
            "BinaryData",
            "ArrayProperty",
            &string("ByteProperty"),
            &[
                i32::try_from(body.len()).unwrap().to_le_bytes().as_slice(),
                &body,
            ]
            .concat(),
        ),
        string("None"),
    ]
    .concat();
    let shared = structure("StateRecorderBlobs", "StateRecorderBlob", &record);
    let info = [
        tag("ProspectID", "StrProperty", &[], &wide(&source_prospect)),
        tag("LobbyName", "StrProperty", &[], &wide(&source_lobby)),
        string("None"),
    ]
    .concat();
    let payload = [
        tag("Version", "IntProperty", &[], &2_i32.to_le_bytes()),
        structure("ProspectInfo", "ProspectInfo", &info),
        tag("ProspectMapName", "StrProperty", &[], &wide(&source_map)),
        structure("LastSavedDateTime", "DateTime", &timestamp.to_le_bytes()),
        tag(
            "StateRecorderBlobs",
            "ArrayProperty",
            &string("StructProperty"),
            &[1_i32.to_le_bytes().as_slice(), &shared].concat(),
        ),
        string("None"),
    ]
    .concat();
    let preserved = Arc::new(
        PreservedSection::from_inner_payload(&payload, PreserveLimits::default()).unwrap(),
    );
    let owner = &native.actors[0];
    let metadata: C001InertImportMetadataV1 = serde_json::from_value(json!({
        "format": 1,
        "adapter": "c001_inert_placement_v1",
        "coordinates": "right_handed_z_up_meters_quat_xyzw_world_space",
        "source_version": 2,
        "source_payload_sha256": hex(&preserved.payload_sha256()),
        "native_catalog_id": native.catalog_id,
        "native_map": native.map,
        "source_map": source_map,
        "source_prospect_id": source_prospect,
        "source_lobby_name": source_lobby,
        "source_last_saved_date_time": timestamp,
        "root_presence": {
            "version": true, "lobby_privacy": false, "prospect_info": true,
            "prospect_map_name": true, "last_saved_date_time": true,
            "legacy_prospect_id": false, "legacy_prospect_dt_key": false,
            "state_recorder_blobs": true
        },
        "owners": [{
            "source_index": 0, "id": owner.id,
            "source_actor_class": source_class, "source_actor_path": source_path,
            "source_object_name": source_object, "source_icarus_guid": i32::from(variant),
            "native_placement": owner.placement, "native_class": owner.class
        }]
    }))
    .unwrap();
    (
        WorldSnapshot {
            schema_version: SNAPSHOT_SCHEMA_C001_INERT,
            tick,
            entities: Vec::new(),
            native_world: Some(native),
            preserved_prospect: Some(preserved),
            native_import: Some(metadata),
            native_relationships: None,
            native_lifecycle: None,
            native_owner_import: None,
            native_scalar_contracts: None,
            native_scalar_owner_import: None,
            native_inventory_contracts: None,
            native_inventories: None,
            native_inventory_owner_import: None,
            native_modifiers: None,
        },
        payload,
    )
}

fn read_json(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

fn rewrite(root: &Path, number: u64, value: &Value) {
    let bytes = serde_json::to_vec(value).unwrap();
    fs::write(root.join(format!("snapshot-{number}.json")), &bytes).unwrap();
    let mut manifest = read_json(&root.join("manifest.json"));
    for slot in ["current", "previous"] {
        if manifest[slot]["number"] == number {
            manifest[slot]["sha256"] = hex(&Sha256::digest(&bytes)).into();
        }
    }
    fs::write(
        root.join("manifest.json"),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();
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

#[test]
fn schema_five_preserves_exact_source_identity_bytes_and_pose_edits_through_worker() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let (mut state, payload) = fixture(10, 1);
    SnapshotStore::preflight(&state).unwrap();
    assert!(disk(root).is_empty());
    let mut store = SnapshotStore::open(root).unwrap();
    store.save_new(&state).unwrap();
    let first = disk(root);
    let sidecar = first
        .keys()
        .find(|name| is_sidecar_name(name))
        .unwrap()
        .clone();
    let written = read_json(&root.join("snapshot-1.json"));
    assert_eq!(written["schema_version"], 5);
    assert_eq!(written["native_world"]["definitions"]["native_format"], 1);
    assert!(written["preserved_prospect"].get("data").is_none());
    assert_eq!(
        written["native_import"],
        serde_json::to_value(state.native_import.as_ref().unwrap()).unwrap()
    );
    assert_eq!(store.load().unwrap().unwrap().snapshot, state);
    state.tick = 20;
    state.native_world.as_mut().unwrap().actors[0]
        .transform
        .translation = [2.0, 3.0, 1.0];
    store.save(&state).unwrap();
    state.tick = 30;
    state.native_world.as_mut().unwrap().actors[0]
        .transform
        .rotation = [0.0, 0.0, 1.0, 0.0];
    let mut worker = SaveWorker::spawn(store).unwrap();
    worker.submit(state.clone()).unwrap();
    assert_eq!(
        worker
            .finish_before(Instant::now() + Duration::from_secs(10))
            .unwrap()
            .unwrap()
            .generation,
        3
    );
    drop(worker);
    // Worker teardown releases its store asynchronously, so bound lock retries.
    let deadline = Instant::now() + Duration::from_secs(10);
    let reopened = loop {
        match SnapshotStore::open(root) {
            Ok(store) => break store,
            Err(error) => {
                assert!(Instant::now() < deadline, "{error:#}");
                std::thread::sleep(Duration::from_millis(1));
            }
        }
    };
    let loaded = reopened.load().unwrap().unwrap();
    assert!(!loaded.recovered_previous);
    assert_eq!(loaded.snapshot, state);
    let source = loaded.snapshot.preserved_prospect.unwrap();
    assert_eq!(
        source.decode().unwrap().to_inner_payload().unwrap(),
        payload
    );
    assert_eq!(fs::read(root.join(&sidecar)).unwrap(), first[&sidecar]);
    assert_eq!(
        disk(root)
            .keys()
            .filter(|name| is_sidecar_name(name))
            .count(),
        1
    );
    assert!(!root.join("snapshot-1.json").exists());
}

#[test]
fn semantic_metadata_and_payload_mismatches_fail_preflight_and_leave_store_unchanged() {
    let (original, _) = fixture(10, 1);
    let (other, _) = fixture(20, 2);
    let mut invalid = Vec::new();
    for field in [
        "source_payload_sha256",
        "native_catalog_id",
        "native_map",
        "source_version",
        "source_map",
        "source_last_saved_date_time",
        "owners",
    ] {
        let mut state = original.clone();
        let mut value = serde_json::to_value(state.native_import.as_ref().unwrap()).unwrap();
        value[field] = match field {
            "source_payload_sha256" | "native_catalog_id" => "0".repeat(64).into(),
            "native_map" => "other".into(),
            "source_version" => 1.into(),
            "source_map" => json!(vec![65_u16; 1025]),
            "source_last_saved_date_time" => Value::Null,
            "owners" => json!([]),
            _ => unreachable!(),
        };
        state.native_import = Some(serde_json::from_value(value).unwrap());
        invalid.push(state);
    }
    for field in ["id", "native_placement", "native_class", "source_index"] {
        let mut state = original.clone();
        let mut value = serde_json::to_value(state.native_import.as_ref().unwrap()).unwrap();
        value["owners"][0][field] = match field {
            "id" => 999.into(),
            "source_index" => 1.into(),
            _ => "other".into(),
        };
        state.native_import = Some(serde_json::from_value(value).unwrap());
        invalid.push(state);
    }
    for field in [
        "source_actor_class",
        "source_actor_path",
        "source_object_name",
        "source_icarus_guid",
    ] {
        let mut state = original.clone();
        let mut value = serde_json::to_value(state.native_import.as_ref().unwrap()).unwrap();
        value["owners"][0][field] = if field == "source_icarus_guid" {
            99.into()
        } else {
            json!(units("OtherAuthoredIdentity"))
        };
        state.native_import = Some(serde_json::from_value(value).unwrap());
        invalid.push(state);
    }
    for field in [
        "version",
        "prospect_info",
        "prospect_map_name",
        "state_recorder_blobs",
        "legacy_prospect_id",
        "legacy_prospect_dt_key",
        "lobby_privacy",
    ] {
        let mut state = original.clone();
        let mut value = serde_json::to_value(state.native_import.as_ref().unwrap()).unwrap();
        let old = value["root_presence"][field].as_bool().unwrap();
        value["root_presence"][field] = (!old).into();
        state.native_import = Some(serde_json::from_value(value).unwrap());
        invalid.push(state);
    }
    let mut wrong_payload = original.clone();
    wrong_payload.preserved_prospect = other.preserved_prospect;
    invalid.push(wrong_payload);
    let mut stateful = original.clone();
    let catalog = Arc::new(
        CompiledNativeCatalog::parse(&CATALOG.replace(
            "variables = []",
            "variables = [{ key = \"charge\", default = { type = \"i32\", value = 0 } }]",
        ))
        .unwrap(),
    );
    let native = prepare_authored_world(catalog, "demo")
        .unwrap()
        .snapshot()
        .clone();
    stateful.native_import.as_mut().unwrap().native_catalog_id = native.catalog_id.clone();
    stateful.native_world = Some(native);
    invalid.push(stateful);
    let mut missing_metadata = original.clone();
    missing_metadata.native_import = None;
    invalid.push(missing_metadata);
    let mut missing_source = original.clone();
    missing_source.preserved_prospect = None;
    invalid.push(missing_source);
    let mut missing_native = original.clone();
    missing_native.native_world = None;
    invalid.push(missing_native);
    let mut foundation = original.clone();
    foundation.entities.push(EntityRecord {
        id: DomainId(1),
        position: Position([0.0; 3]),
    });
    invalid.push(foundation);
    for schema in 1..=4 {
        let mut legacy = original.clone();
        legacy.schema_version = schema;
        if schema != 4 {
            legacy.native_world = None;
        }
        if schema == 1 {
            legacy.preserved_prospect = None;
        }
        invalid.push(legacy);
    }
    let directory = tempfile::tempdir().unwrap();
    let mut store = SnapshotStore::open(directory.path()).unwrap();
    store.save(&original).unwrap();
    let before = disk(directory.path());
    for state in invalid {
        assert!(SnapshotStore::preflight(&state).is_err());
        assert!(store.save(&state).is_err());
        assert!(store.save_new(&state).is_err());
        assert_eq!(disk(directory.path()), before);
    }
}

#[test]
fn metadata_byte_bound_is_checked_by_pure_preflight_before_publication() {
    let (original, _) = fixture(10, 1);
    let mut oversized = original.clone();
    let metadata = oversized.native_import.as_mut().unwrap();
    let mut owner = metadata.owners[0].clone();
    owner.source_actor_class = vec![60000; 1024];
    owner.source_actor_path = vec![60001; 1024];
    owner.source_object_name = vec![60002; 1024];
    metadata.owners = vec![owner; 384];
    assert!(serde_json::to_vec(metadata).unwrap().len() > 4 * 1024 * 1024);
    assert!(
        SnapshotStore::preflight(&oversized)
            .unwrap_err()
            .to_string()
            .contains("limit")
    );
    let directory = tempfile::tempdir().unwrap();
    let mut store = SnapshotStore::open(directory.path()).unwrap();
    let empty = disk(directory.path());
    assert!(store.save_new(&oversized).is_err());
    assert_eq!(disk(directory.path()), empty);
    store.save_new(&original).unwrap();
    let before = disk(directory.path());
    assert!(store.save(&oversized).is_err());
    assert_eq!(disk(directory.path()), before);
}

#[test]
fn missing_schema_five_fields_sidecar_and_corrupt_current_recover_previous_exactly() {
    for damage in [
        "native_import",
        "native_world",
        "preserved_prospect",
        "sidecar",
        "checksum",
        "metadata_identity",
    ] {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let (before, _) = fixture(10, 1);
        let (current, _) = fixture(20, 2);
        let mut store = SnapshotStore::open(root).unwrap();
        store.save(&before).unwrap();
        store.save(&current).unwrap();
        drop(store);
        let mut value = read_json(&root.join("snapshot-2.json"));
        match damage {
            "sidecar" => {
                let bytes = current
                    .preserved_prospect
                    .as_ref()
                    .unwrap()
                    .native_encoding();
                fs::remove_file(
                    root.join(format!("preserved-{}.bin", hex(&Sha256::digest(bytes)))),
                )
                .unwrap();
            }
            "checksum" => fs::write(root.join("snapshot-2.json"), b"partial").unwrap(),
            "metadata_identity" => {
                value["native_import"]["native_catalog_id"] = "0".repeat(64).into();
                rewrite(root, 2, &value);
            }
            field => {
                value.as_object_mut().unwrap().remove(field);
                rewrite(root, 2, &value);
            }
        }
        let mut reopened = SnapshotStore::open(root).unwrap();
        let loaded = reopened.load().unwrap().unwrap();
        assert!(loaded.recovered_previous, "{damage}");
        assert_eq!(loaded.snapshot, before);
        let unchanged = disk(root);
        assert!(reopened.save_new(&current).is_err());
        assert_eq!(disk(root), unchanged);
        let mut next = before;
        next.tick = 30;
        assert_eq!(reopened.save(&next).unwrap().generation, 3);
        assert_eq!(reopened.load().unwrap().unwrap().snapshot, next);
    }
}

#[test]
fn missing_required_metadata_and_source_reject_an_only_generation() {
    for field in [
        "native_import",
        "native_world",
        "preserved_prospect",
        "sidecar",
    ] {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let (state, _) = fixture(10, 1);
        let mut store = SnapshotStore::open(root).unwrap();
        store.save(&state).unwrap();
        if field == "sidecar" {
            let name = disk(root)
                .keys()
                .find(|name| is_sidecar_name(name))
                .unwrap()
                .clone();
            fs::remove_file(root.join(name)).unwrap();
        } else {
            let mut value = read_json(&root.join("snapshot-1.json"));
            value.as_object_mut().unwrap().remove(field);
            rewrite(root, 1, &value);
        }
        assert!(store.load().is_err(), "{field}");
        let before = disk(root);
        assert!(store.save_new(&state).is_err());
        assert_eq!(disk(root), before);
    }
}

#[test]
fn future_metadata_and_native_discriminators_are_terminal_before_changed_shapes() {
    for discriminator in ["format", "adapter", "coordinates", "native_format", "kind"] {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let (before, _) = fixture(10, 1);
        let (current, _) = fixture(20, 2);
        let mut store = SnapshotStore::open(root).unwrap();
        store.save(&before).unwrap();
        store.save(&current).unwrap();
        let mut value = read_json(&root.join("snapshot-2.json"));
        match discriminator {
            "format" => {
                value["native_import"]["format"] = 2.into();
                value["native_import"]["owners"] = json!({"future_shape": true});
            }
            "adapter" | "coordinates" => {
                value["native_import"][discriminator] = "future_variant".into();
                value["native_import"]["root_presence"] = json!([1, 2]);
            }
            "native_format" => {
                value["native_world"]["definitions"]["native_format"] = 2.into();
                value["native_world"]["definitions"]["classes"] = json!({"future_shape": true});
            }
            "kind" => {
                value["native_world"]["definitions"]["classes"][0]["kind"] = "future_kind".into();
                value["native_world"]["actors"] = json!({"future_shape": true});
            }
            _ => unreachable!(),
        }
        rewrite(root, 2, &value);
        let unchanged = disk(root);
        // Exercise uncached decoding too, rather than relying on a prior load.
        drop(store);
        let mut store = SnapshotStore::open(root).unwrap();
        assert!(
            store
                .load()
                .unwrap_err()
                .to_string()
                .contains("unsupported"),
            "{discriminator}"
        );
        assert!(
            store
                .save(&current)
                .unwrap_err()
                .to_string()
                .contains("unsupported")
        );
        assert!(
            store
                .save_new(&current)
                .unwrap_err()
                .to_string()
                .contains("unsupported")
        );
        assert_eq!(disk(root), unchanged);
    }
}

#[test]
fn malformed_discriminator_types_and_known_shapes_are_recoverable_corruption() {
    for damage in [
        "format",
        "adapter",
        "coordinates",
        "native_format",
        "kind",
        "owners",
        "root_presence",
        "owner",
        "source_map",
        "unknown_field",
    ] {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let (before, _) = fixture(10, 1);
        let (current, _) = fixture(20, 2);
        let mut store = SnapshotStore::open(root).unwrap();
        store.save(&before).unwrap();
        store.save(&current).unwrap();
        let mut value = read_json(&root.join("snapshot-2.json"));
        match damage {
            "format" | "adapter" | "coordinates" => {
                value["native_import"][damage] = json!({"bad_type": true})
            }
            "native_format" => {
                value["native_world"]["definitions"]["native_format"] = "bad_type".into()
            }
            "kind" => value["native_world"]["definitions"]["classes"][0]["kind"] = 5.into(),
            "owners" => value["native_import"]["owners"] = json!({}),
            "owner" => value["native_import"]["owners"][0] = json!([]),
            "root_presence" => value["native_import"]["root_presence"] = json!(vec![true; 8]),
            "source_map" => value["native_import"]["source_map"] = json!([65536]),
            "unknown_field" => value["native_import"]["future"] = true.into(),
            _ => unreachable!(),
        }
        rewrite(root, 2, &value);
        let loaded = store.load().unwrap().unwrap();
        assert!(loaded.recovered_previous, "{damage}");
        assert_eq!(loaded.snapshot, before);
    }
}

#[test]
fn import_named_objects_and_diagnostics_keep_source_identity_private() {
    let (state, _) = fixture(10, 1);
    let metadata = state.native_import.as_ref().unwrap();
    let original = serde_json::to_value(metadata).unwrap();
    for target in ["metadata", "root_presence", "owner", "root_array"] {
        let mut value = original.clone();
        match target {
            "metadata" => value["extra"] = "AuthoredSecret".into(),
            "root_presence" => value["root_presence"]["extra"] = "AuthoredSecret".into(),
            "owner" => value["owners"][0]["extra"] = "AuthoredSecret".into(),
            "root_array" => {
                value["root_presence"] = json!([true, false, true, true, true, false, false, true])
            }
            _ => unreachable!(),
        }
        assert!(serde_json::from_value::<C001InertImportMetadataV1>(value).is_err());
    }
    assert!(!format!("{metadata:?}").contains("Authored"));
    assert!(serde_json::from_value::<C001InertImportMetadataV1>(json!([])).is_err());
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let mut store = SnapshotStore::open(root).unwrap();
    store.save(&state).unwrap();
    let mut generation = read_json(&root.join("snapshot-1.json"));
    generation["native_import"]["source_version"] = "AuthoredSecret".into();
    rewrite(root, 1, &generation);
    let diagnostic = format!("{:#}", store.load().unwrap_err());
    assert!(!diagnostic.contains("AuthoredSecret"));
    assert!(!diagnostic.contains("AuthoredMap"));
}

#[test]
fn save_new_refuses_an_existing_valid_schema_five_world() {
    let directory = tempfile::tempdir().unwrap();
    let (state, _) = fixture(10, 1);
    let mut store = SnapshotStore::open(directory.path()).unwrap();
    store.save_new(&state).unwrap();
    let unchanged = disk(directory.path());
    assert!(store.save_new(&state).is_err());
    assert_eq!(disk(directory.path()), unchanged);
}

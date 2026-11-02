//! Durable storage of independently authored relationship state. All geometry,
//! identities and preserved C001 source bytes in this file are invented.
use std::{
    collections::BTreeMap,
    fs,
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};

use icarus_bevy::{
    native_import::C001InertImportMetadataV1,
    native_relationships::{
        CompiledNativeRelationships, NativeActorLinkV1, NativeRelationshipActorV1,
        NativeRelationshipRole, NativeRelationshipsDefinition,
    },
    native_world::{CompiledNativeCatalog, NativeWorldSnapshot, prepare_authored_world},
    persistence::{SaveWorker, SnapshotStore},
    preservation::{PreserveLimits, PreservedSection, is_sidecar_name},
    simulation::{DomainId, EntityRecord, Position, SNAPSHOT_SCHEMA_RELATIONSHIPS, WorldSnapshot},
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
key = "a_device"
class = "box"
tile = "tile"
transform = { translation = [0.0, 0.0, 0.0], rotation = [0.0, 0.0, 0.0, 1.0], scale = [1.0, 1.0, 1.0] }
[[maps.placements]]
key = "b_foundation"
class = "box"
tile = "tile"
transform = { translation = [0.0, 0.0, 0.0], rotation = [0.0, 0.0, 0.0, 1.0], scale = [1.0, 1.0, 1.0] }
[[maps.placements]]
key = "c_foundation"
class = "box"
tile = "tile"
transform = { translation = [0.0, 0.0, 0.0], rotation = [0.0, 0.0, 0.0, 1.0], scale = [1.0, 1.0, 1.0] }
"#;

fn units(value: &str) -> Vec<u16> {
    value.encode_utf16().collect()
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
    [
        (-i32::try_from(value.len() + 1).unwrap())
            .to_le_bytes()
            .as_slice(),
        &value
            .iter()
            .flat_map(|unit| unit.to_le_bytes())
            .collect::<Vec<_>>(),
        &[0, 0],
    ]
    .concat()
}

fn string(value: &str) -> Vec<u8> {
    wide(&units(value))
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

// The independently authored valid-source pattern used by native_import_storage,
// expanded to bind every placement. This source does not define the relationship
// roles: those are an independent authored contract.
fn source(
    native: &NativeWorldSnapshot,
    variant: u8,
) -> (Arc<PreservedSection>, C001InertImportMetadataV1) {
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
    let mut records = Vec::new();
    let mut owners = Vec::new();
    for (index, actor) in native.actors.iter().enumerate() {
        let class = units("AuthoredStorageClass");
        let path = units(&format!("/Authored/Storage{index}"));
        let object = vec![
            65,
            0xd800,
            u16::from(variant),
            u16::try_from(index).unwrap() + 49,
        ];
        let guid = i32::try_from(index + 1).unwrap();
        let body = [
            tag("ActorClassName", "NameProperty", &[], &wide(&class)),
            tag("ActorPathName", "StrProperty", &[], &wide(&path)),
            tag("ObjectFName", "NameProperty", &[], &wide(&object)),
            tag("IcarusActorGUID", "IntProperty", &[], &guid.to_le_bytes()),
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
        records.push(record);
        owners.push(json!({
            "source_index": index, "id": actor.id,
            "source_actor_class": class, "source_actor_path": path,
            "source_object_name": object, "source_icarus_guid": guid,
            "native_placement": actor.placement, "native_class": actor.class
        }));
    }
    let info = [
        tag(
            "ProspectID",
            "StrProperty",
            &[],
            &string("AuthoredStorageProspect"),
        ),
        string("None"),
    ]
    .concat();
    let payload = [
        tag("Version", "IntProperty", &[], &2_i32.to_le_bytes()),
        structure("ProspectInfo", "ProspectInfo", &info),
        tag(
            "ProspectMapName",
            "StrProperty",
            &[],
            &string("AuthoredStorageMap"),
        ),
        tag(
            "StateRecorderBlobs",
            "ArrayProperty",
            &string("StructProperty"),
            &[
                i32::try_from(records.len())
                    .unwrap()
                    .to_le_bytes()
                    .as_slice(),
                &structure("StateRecorderBlobs", "StateRecorderBlob", &records.concat()),
            ]
            .concat(),
        ),
        string("None"),
    ]
    .concat();
    let preserved = Arc::new(
        PreservedSection::from_inner_payload(&payload, PreserveLimits::default()).unwrap(),
    );
    let metadata = serde_json::from_value(json!({
        "format": 1, "adapter": "c001_inert_placement_v1",
        "coordinates": "right_handed_z_up_meters_quat_xyzw_world_space",
        "source_version": 2, "source_payload_sha256": hex(&preserved.payload_sha256()),
        "native_catalog_id": native.catalog_id, "native_map": native.map,
        "source_map": units("AuthoredStorageMap"), "source_prospect_id": units("AuthoredStorageProspect"),
        "source_lobby_name": [], "source_last_saved_date_time": null,
        "root_presence": { "version": true, "lobby_privacy": false, "prospect_info": true,
            "prospect_map_name": true, "last_saved_date_time": false, "legacy_prospect_id": false,
            "legacy_prospect_dt_key": false, "state_recorder_blobs": true },
        "owners": owners
    })).unwrap();
    (preserved, metadata)
}

// Modes: pure native, preserved source, preserved source with C001 metadata.
fn fixture(tick: u64, mode: u8, variant: u8) -> WorldSnapshot {
    let catalog = Arc::new(CompiledNativeCatalog::parse(CATALOG).unwrap());
    let native = prepare_authored_world(Arc::clone(&catalog), "demo")
        .unwrap()
        .snapshot()
        .clone();
    let link = |guid| NativeActorLinkV1 {
        name: vec![65, 0xd800],
        guid,
    };
    let model = CompiledNativeRelationships::compile(
        NativeRelationshipsDefinition {
            relationship_format: 1,
            native_catalog_id: catalog.identity().into(),
            map: "demo".into(),
            actors: native
                .actors
                .iter()
                .enumerate()
                .map(|(index, actor)| NativeRelationshipActorV1 {
                    placement: actor.placement.clone(),
                    identity: link(i32::try_from(index + 1).unwrap()),
                    role: if index == 0 {
                        NativeRelationshipRole::Deployable
                    } else {
                        NativeRelationshipRole::Foundation
                    },
                    foundation: (index == 0).then(|| link(2)),
                })
                .collect(),
        },
        &catalog,
    )
    .unwrap();
    let mut state = WorldSnapshot {
        schema_version: SNAPSHOT_SCHEMA_RELATIONSHIPS,
        tick,
        native_world: Some(native),
        native_relationships: Some(model.initial_snapshot()),
        ..Default::default()
    };
    if mode != 0 {
        let (source, metadata) = source(state.native_world.as_ref().unwrap(), variant);
        state.preserved_prospect = Some(source);
        if mode == 2 {
            state.native_import = Some(metadata);
        }
    }
    state
}

fn edit(state: &mut WorldSnapshot, target: Option<DomainId>, x: f32) {
    let model = CompiledNativeRelationships::from_snapshot(
        state.native_relationships.as_ref().unwrap(),
        state.native_world.as_ref().unwrap(),
    )
    .unwrap();
    model
        .reassign_foundation(
            state.native_relationships.as_mut().unwrap(),
            DomainId(1),
            target,
        )
        .unwrap();
    state.native_world.as_mut().unwrap().actors[0]
        .transform
        .translation[0] = x;
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
    for secret in [
        "StorageSecret",
        "AuthoredStorageClass",
        "AuthoredStorageProspect",
        "/Authored/",
    ] {
        assert!(!text.contains(secret));
    }
}

#[test]
fn schema_six_roundtrips_pure_preserved_and_optional_c001_contracts() {
    for mode in 0..3 {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let state = fixture(10, mode, 1);
        SnapshotStore::preflight(&state).unwrap();
        assert!(disk(root).is_empty());
        let mut store = SnapshotStore::open(root).unwrap();
        assert_eq!(store.save_new(&state).unwrap().generation, 1);
        let written = read_json(&root.join("snapshot-1.json"));
        assert_eq!(written["schema_version"], 6);
        assert_eq!(written["entities"], json!([]));
        assert_eq!(
            written["native_relationships"],
            serde_json::to_value(state.native_relationships.as_ref().unwrap()).unwrap()
        );
        assert_eq!(written.get("preserved_prospect").is_some(), mode != 0);
        assert_eq!(written.get("native_import").is_some(), mode == 2);
        if mode != 0 {
            assert!(written["preserved_prospect"].get("data").is_none());
        }
        assert_eq!(store.load().unwrap().unwrap().snapshot, state);
        drop(store);
        let reopened = SnapshotStore::open(root).unwrap();
        let recovered = reopened.load().unwrap().unwrap();
        assert!(!recovered.recovered_previous);
        assert_eq!(recovered.snapshot, state);
    }
}

#[test]
fn worker_saves_changed_edges_and_poses_and_reuses_exact_source() {
    for mode in 0..3 {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let mut state = fixture(10, mode, 1);
        let initial_declarations = state
            .native_relationships
            .as_ref()
            .unwrap()
            .definition
            .clone();
        let mut store = SnapshotStore::open(root).unwrap();
        store.save(&state).unwrap();
        let first = disk(root);
        state.tick = 20;
        edit(&mut state, None, 1.0);
        store.save(&state).unwrap();
        state.tick = 30;
        edit(&mut state, Some(DomainId(3)), 2.0);
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
        assert_eq!(loaded.snapshot, state);
        assert_eq!(
            loaded
                .snapshot
                .native_relationships
                .as_ref()
                .unwrap()
                .definition,
            initial_declarations
        );
        if mode != 0 {
            let sidecar = first.keys().find(|name| is_sidecar_name(name)).unwrap();
            assert_eq!(fs::read(root.join(sidecar)).unwrap(), first[sidecar]);
            assert_eq!(
                disk(root)
                    .keys()
                    .filter(|name| is_sidecar_name(name))
                    .count(),
                1
            );
            assert_eq!(
                loaded
                    .snapshot
                    .preserved_prospect
                    .as_ref()
                    .unwrap()
                    .decode()
                    .unwrap()
                    .to_inner_payload()
                    .unwrap(),
                state
                    .preserved_prospect
                    .as_ref()
                    .unwrap()
                    .decode()
                    .unwrap()
                    .to_inner_payload()
                    .unwrap()
            );
        }
        state.tick = 40;
        edit(&mut state, Some(DomainId(2)), 3.0);
        reopened.save(&state).unwrap();
        assert_eq!(reopened.load().unwrap().unwrap().snapshot, state);
    }
}

#[test]
fn required_state_and_c001_source_are_validated_before_any_publication() {
    let state = fixture(10, 2, 1);
    let mut variants = Vec::new();
    let mut changed = state.clone();
    changed.native_world = None;
    variants.push(changed);
    let mut changed = state.clone();
    changed.native_relationships = None;
    variants.push(changed);
    let mut changed = state.clone();
    changed.preserved_prospect = None;
    variants.push(changed);
    let mut changed = state.clone();
    changed.entities.push(EntityRecord {
        id: DomainId(1),
        position: Position([0.0; 3]),
    });
    variants.push(changed);
    let mut changed = state.clone();
    changed
        .native_import
        .as_mut()
        .unwrap()
        .source_payload_sha256 = "0".repeat(64);
    variants.push(changed);
    let mut changed = state.clone();
    changed.native_import.as_mut().unwrap().owners.pop();
    variants.push(changed);
    let mut changed = state.clone();
    changed
        .native_relationships
        .as_mut()
        .unwrap()
        .relationship_id = "0".repeat(64);
    variants.push(changed);
    let mut changed = state.clone();
    changed.native_relationships.as_mut().unwrap().foundations[0].foundation = Some(DomainId(99));
    variants.push(changed);
    for invalid in variants {
        let directory = tempfile::tempdir().unwrap();
        assert!(SnapshotStore::preflight(&invalid).is_err());
        let mut store = SnapshotStore::open(directory.path()).unwrap();
        let empty = disk(directory.path());
        assert!(store.save_new(&invalid).is_err());
        assert_eq!(disk(directory.path()), empty);
        store.save(&state).unwrap();
        let committed = disk(directory.path());
        assert!(store.save(&invalid).is_err());
        assert_eq!(disk(directory.path()), committed);
    }
    // Retain schema meanings 1 through 5, which cannot carry relationship state.
    for schema in 1..=5 {
        let mut invalid = state.clone();
        invalid.schema_version = schema;
        assert!(SnapshotStore::preflight(&invalid).is_err());
    }
}

#[test]
fn corrupt_current_relationship_digest_metadata_and_edges_recover_previous_exactly() {
    for damage in [
        "digest",
        "definition",
        "map",
        "catalog",
        "state_missing",
        "state_duplicate",
        "state_dangling",
        "state_wrong_role",
        "target_wrong_role",
        "metadata",
        "source_digest",
    ] {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let mut before = fixture(10, 2, 1);
        edit(&mut before, None, 1.0);
        let mut current = fixture(20, 2, 1);
        edit(&mut current, Some(DomainId(3)), 2.0);
        let mut store = SnapshotStore::open(root).unwrap();
        store.save(&before).unwrap();
        store.save(&current).unwrap();
        let mut value = read_json(&root.join("snapshot-2.json"));
        match damage {
            "digest" => value["native_relationships"]["relationship_id"] = "0".repeat(64).into(),
            "definition" => {
                value["native_relationships"]["definition"]["actors"][0]["identity"]["guid"] =
                    901.into()
            }
            "map" => value["native_relationships"]["definition"]["map"] = "StorageSecretMap".into(),
            "catalog" => {
                value["native_relationships"]["definition"]["native_catalog_id"] =
                    "0".repeat(64).into()
            }
            "state_missing" => value["native_relationships"]["foundations"] = json!([]),
            "state_duplicate" => {
                let row = value["native_relationships"]["foundations"][0].clone();
                value["native_relationships"]["foundations"]
                    .as_array_mut()
                    .unwrap()
                    .push(row);
            }
            "state_dangling" => {
                value["native_relationships"]["foundations"][0]["foundation"] = 99.into()
            }
            "state_wrong_role" => {
                value["native_relationships"]["foundations"][0]["owner"] = 2.into()
            }
            "target_wrong_role" => {
                value["native_relationships"]["foundations"][0]["foundation"] = 1.into()
            }
            "metadata" => value["native_import"]["native_catalog_id"] = "0".repeat(64).into(),
            "source_digest" => {
                value["native_import"]["source_payload_sha256"] = "0".repeat(64).into()
            }
            _ => unreachable!(),
        }
        rewrite(root, 2, &value);
        let corrupted = disk(root);
        let recovered = store.load().unwrap().unwrap();
        assert!(recovered.recovered_previous, "{damage}");
        assert_eq!(recovered.snapshot, before, "{damage}");
        assert_eq!(disk(root), corrupted);
        before.tick = 30;
        edit(&mut before, Some(DomainId(2)), 3.0);
        assert_eq!(store.save(&before).unwrap().generation, 3);
        assert_eq!(store.load().unwrap().unwrap().snapshot, before);
        assert!(!root.join("snapshot-2.json").exists());
    }
}

#[test]
fn every_future_discriminator_is_terminal_before_typed_state_or_sidecar_reads() {
    for mode in 0..3 {
        for field in [
            "native_format",
            "kind",
            "relationship_format",
            "role",
            "format",
            "adapter",
            "coordinates",
        ] {
            if mode != 2 && matches!(field, "format" | "adapter" | "coordinates") {
                continue;
            }
            let directory = tempfile::tempdir().unwrap();
            let root = directory.path();
            let before = fixture(10, mode, 1);
            let current = fixture(20, mode, 1);
            let next = fixture(30, mode, 1);
            let mut store = SnapshotStore::open(root).unwrap();
            store.save(&before).unwrap();
            store.save(&current).unwrap();
            let mut value = read_json(&root.join("snapshot-2.json"));
            match field {
                "native_format" => {
                    value["native_world"]["definitions"]["native_format"] = 2.into();
                    value["native_world"]["definitions"]["classes"] =
                        json!({"StorageSecretFuture": true});
                    value["native_relationships"] = json!([]);
                }
                "kind" => {
                    value["native_world"]["definitions"]["classes"][0]["kind"] =
                        "StorageSecretFutureKind".into();
                    value["native_world"]["actors"] = json!({"changed_shape": true});
                    value["native_relationships"] = Value::Null;
                }
                "relationship_format" => {
                    value["native_relationships"]["definition"]["relationship_format"] = 2.into();
                    value["native_relationships"]["definition"]["actors"] =
                        json!({"StorageSecretFuture": true});
                    value["native_world"]["definitions"]["classes"] = Value::Null;
                }
                "role" => {
                    value["native_relationships"]["definition"]["actors"][0]["role"] =
                        "StorageSecretFutureRole".into();
                    value["native_relationships"]["definition"]["actors"][0]["identity"] =
                        json!([]);
                    value["native_relationships"]["foundations"] = json!({"changed_shape": true});
                    value["native_world"]["definitions"]["classes"] = Value::Null;
                }
                "format" => {
                    value["native_import"]["format"] = 2.into();
                    value["native_import"]["owners"] = json!({"changed_shape": true});
                    value["native_relationships"]["definition"]["actors"] = Value::Null;
                }
                "adapter" | "coordinates" => {
                    value["native_import"][field] = "StorageSecretFutureVariant".into();
                    value["native_import"]["root_presence"] = json!([]);
                    value["native_world"]["definitions"]["classes"] = Value::Null;
                }
                _ => unreachable!(),
            }
            rewrite(root, 2, &value);
            // A missing source must not hide an upgrade requirement or cause
            // fallback. Neither generation may be decoded through that source.
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
                    "{field}: {error:#}"
                );
                assert_private(&error);
            }
            assert_eq!(disk(root), unchanged);
        }
    }
}

#[test]
fn known_malformed_types_missing_fields_and_bad_shapes_recover_previous() {
    for damage in [
        "native_world",
        "native_relationships",
        "entities",
        "tick",
        "native_format",
        "kind",
        "relationship_format",
        "relationship_format_zero",
        "role",
        "role_missing",
        "identity",
        "initial_link",
        "edge",
        "format",
        "adapter",
        "coordinates",
        "native_import",
        "preserved_prospect",
        "unknown_field",
        "duplicate_role",
    ] {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let mut before = fixture(10, 2, 1);
        edit(&mut before, None, 1.0);
        let current = fixture(20, 2, 1);
        let mut store = SnapshotStore::open(root).unwrap();
        store.save(&before).unwrap();
        store.save(&current).unwrap();
        let mut value = read_json(&root.join("snapshot-2.json"));
        match damage {
            "native_world" | "native_relationships" | "entities" | "tick" => {
                value.as_object_mut().unwrap().remove(damage);
            }
            "native_format" => {
                value["native_world"]["definitions"]["native_format"] = "StorageSecretType".into()
            }
            "kind" => value["native_world"]["definitions"]["classes"][0]["kind"] = 7.into(),
            "relationship_format" => {
                value["native_relationships"]["definition"]["relationship_format"] =
                    json!({"StorageSecretType": true})
            }
            "relationship_format_zero" => {
                value["native_relationships"]["definition"]["relationship_format"] = 0.into()
            }
            "role" => {
                value["native_relationships"]["definition"]["actors"][0]["role"] =
                    json!(["StorageSecretRole"])
            }
            "role_missing" => {
                value["native_relationships"]["definition"]["actors"][0]
                    .as_object_mut()
                    .unwrap()
                    .remove("role");
            }
            "identity" => {
                value["native_relationships"]["definition"]["actors"][0]["identity"]["guid"] =
                    "StorageSecretGUID".into()
            }
            "initial_link" => {
                value["native_relationships"]["definition"]["actors"][0]["foundation"]["name"] =
                    json!([65536])
            }
            "edge" => {
                value["native_relationships"]["foundations"][0]["foundation"] =
                    "StorageSecretTarget".into()
            }
            "format" | "adapter" | "coordinates" => {
                value["native_import"][damage] = json!({"StorageSecretType": true})
            }
            "native_import" => value[damage] = json!([]),
            "preserved_prospect" => value[damage] = Value::Null,
            "unknown_field" => value["StorageSecretField"] = "StorageSecretValue".into(),
            "duplicate_role" => (),
            _ => unreachable!(),
        }
        if damage == "duplicate_role" {
            let duplicate = value.to_string().replacen(
                "\"role\":\"deployable\"",
                "\"role\":\"deployable\",\"role\":\"foundation\"",
                1,
            );
            rewrite_bytes(root, 2, duplicate.as_bytes());
        } else {
            rewrite(root, 2, &value);
        }
        let unchanged = disk(root);
        let recovered = store.load().unwrap().unwrap();
        assert!(recovered.recovered_previous, "{damage}");
        assert_eq!(recovered.snapshot, before, "{damage}");
        assert_eq!(disk(root), unchanged);
        before.tick = 30;
        edit(&mut before, Some(DomainId(3)), 3.0);
        store.save(&before).unwrap();
        assert_eq!(store.load().unwrap().unwrap().snapshot, before);
    }
}

#[test]
fn schema_six_disk_objects_reject_positional_arrays_without_value_leaks() {
    let paths: &[(&str, &[&str])] = &[
        (
            "",
            &[
                "schema_version",
                "tick",
                "entities",
                "native_world",
                "native_relationships",
                "preserved_prospect",
                "native_import",
            ],
        ),
        (
            "/native_relationships",
            &["relationship_id", "definition", "foundations"],
        ),
        (
            "/native_relationships/definition",
            &["relationship_format", "native_catalog_id", "map", "actors"],
        ),
        (
            "/native_relationships/definition/actors/0",
            &["placement", "identity", "role", "foundation"],
        ),
        (
            "/native_relationships/definition/actors/0/identity",
            &["name", "guid"],
        ),
        (
            "/native_relationships/definition/actors/0/foundation",
            &["name", "guid"],
        ),
        (
            "/native_relationships/foundations/0",
            &["owner", "foundation"],
        ),
        (
            "/native_import",
            &[
                "format",
                "adapter",
                "source_version",
                "source_payload_sha256",
                "native_catalog_id",
                "native_map",
                "coordinates",
                "source_map",
                "source_prospect_id",
                "source_lobby_name",
                "source_last_saved_date_time",
                "root_presence",
                "owners",
            ],
        ),
    ];
    for (path, fields) in paths {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let state = fixture(10, 2, 1);
        let mut store = SnapshotStore::open(root).unwrap();
        store.save(&state).unwrap();
        let mut value = read_json(&root.join("snapshot-1.json"));
        let positional = Value::Array(
            fields
                .iter()
                .map(|field| value.pointer(path).unwrap()[*field].clone())
                .collect(),
        );
        *value.pointer_mut(path).unwrap() = positional;
        rewrite(root, 1, &value);
        let unchanged = disk(root);
        for error in [
            store.load().unwrap_err(),
            store.save_new(&state).unwrap_err(),
        ] {
            assert!(!format!("{error:#}").contains("unsupported native snapshot schema"));
            assert_private(&error);
        }
        assert_eq!(disk(root), unchanged);
    }
    // Check diagnostics from a required state field at a valid schema boundary.
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let state = fixture(10, 0, 1);
    let mut store = SnapshotStore::open(root).unwrap();
    store.save(&state).unwrap();
    let mut value = read_json(&root.join("snapshot-1.json"));
    value["native_relationships"]["definition"]["actors"][0]["identity"]["name"] =
        "StorageSecretPayload".into();
    rewrite(root, 1, &value);
    assert_private(&store.load().unwrap_err());
}

#[test]
fn save_new_refuses_existing_recovered_missing_state_and_orphan_generations() {
    for damage in [
        "none",
        "corrupt",
        "native_world",
        "native_relationships",
        "source",
        "orphan",
    ] {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let state = fixture(10, 2, 1);
        let mut current = fixture(20, 2, 2);
        edit(&mut current, Some(DomainId(3)), 1.0);
        let mut store = SnapshotStore::open(root).unwrap();
        store.save(&state).unwrap();
        store.save(&current).unwrap();
        match damage {
            "none" => (),
            "corrupt" => fs::write(root.join("snapshot-2.json"), b"corrupt").unwrap(),
            "native_world" | "native_relationships" => {
                let mut value = read_json(&root.join("snapshot-2.json"));
                value.as_object_mut().unwrap().remove(damage);
                rewrite(root, 2, &value);
            }
            "source" => {
                // Read the reference by content address rather than depending
                // on a stored filename field in the reference DTO.
                let expected = hex(&Sha256::digest(
                    current
                        .preserved_prospect
                        .as_ref()
                        .unwrap()
                        .decode()
                        .unwrap()
                        .to_native_encoding()
                        .unwrap(),
                ));
                let name = format!("preserved-{expected}.bin");
                fs::remove_file(root.join(name)).unwrap();
            }
            "orphan" => fs::remove_file(root.join("manifest.json")).unwrap(),
            _ => unreachable!(),
        }
        let unchanged = disk(root);
        assert!(store.save_new(&fixture(30, 2, 3)).is_err(), "{damage}");
        assert_eq!(disk(root), unchanged);
        if !matches!(damage, "none" | "orphan") {
            let recovered = store.load().unwrap().unwrap();
            assert!(recovered.recovered_previous);
            assert_eq!(recovered.snapshot, state);
        }
    }
}

#[test]
fn optional_null_fields_and_preservation_without_import_metadata_are_admitted() {
    for mode in [0, 1] {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let state = fixture(10, mode, 1);
        let mut store = SnapshotStore::open(root).unwrap();
        store.save(&state).unwrap();
        let mut value = read_json(&root.join("snapshot-1.json"));
        value["native_import"] = Value::Null;
        if mode == 0 {
            value["preserved_prospect"] = Value::Null;
        }
        rewrite(root, 1, &value);
        assert_eq!(store.load().unwrap().unwrap().snapshot, state);
    }
}

#[test]
fn sidecar_damage_recovers_previous_edges_and_retention_collects_only_obsolete_sources() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let mut before = fixture(10, 2, 1);
    edit(&mut before, None, 1.0);
    let mut current = fixture(20, 2, 2);
    edit(&mut current, Some(DomainId(3)), 2.0);
    let mut store = SnapshotStore::open(root).unwrap();
    store.save(&before).unwrap();
    store.save(&current).unwrap();
    let sidecar = |state: &WorldSnapshot| {
        format!(
            "preserved-{}.bin",
            hex(&Sha256::digest(
                state
                    .preserved_prospect
                    .as_ref()
                    .unwrap()
                    .decode()
                    .unwrap()
                    .to_native_encoding()
                    .unwrap()
            ))
        )
    };
    fs::write(root.join(sidecar(&current)), b"StorageSecretCorruptSource").unwrap();
    let recovered = store.load().unwrap().unwrap();
    assert!(recovered.recovered_previous);
    assert_eq!(recovered.snapshot, before);
    let mut next = fixture(30, 2, 3);
    edit(&mut next, Some(DomainId(2)), 3.0);
    store.save(&next).unwrap();
    assert!(root.join(sidecar(&before)).exists());
    assert!(root.join(sidecar(&next)).exists());
    assert!(!root.join(sidecar(&current)).exists());
    assert_eq!(store.load().unwrap().unwrap().snapshot, next);
    // A pure schema-6 generation retains the previous source until the next
    // pure generation replaces the final reference.
    let pure = fixture(40, 0, 1);
    store.save(&pure).unwrap();
    assert!(root.join(sidecar(&next)).exists());
    let pure = fixture(50, 0, 1);
    store.save(&pure).unwrap();
    assert!(disk(root).keys().all(|name| !is_sidecar_name(name)));
    assert_eq!(store.load().unwrap().unwrap().snapshot, pure);
}

//! Independently authored native definitions and preservation payloads only.
use std::{
    fs,
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};

use icarus_bevy::{
    native_world::{
        CompiledNativeCatalog, NativeValue, NativeWorldSnapshot, prepare_authored_world,
    },
    persistence::{SaveWorker, SnapshotStore},
    preservation::{PreserveLimits, PreservedSection, is_sidecar_name},
    simulation::{DomainId, EntityRecord, Position, SNAPSHOT_SCHEMA_NATIVE, WorldSnapshot},
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
key = "box1"
class = "box"
tile = "tile"
transform = { translation = [0.0, 0.0, 0.0], rotation = [0.0, 0.0, 0.0, 1.0], scale = [1.0, 1.0, 1.0] }
"#;

fn snapshot(tick: u64, preserved: Option<&Arc<PreservedSection>>) -> WorldSnapshot {
    let catalog = Arc::new(CompiledNativeCatalog::parse(CATALOG).unwrap());
    let prepared = prepare_authored_world(catalog, "demo").unwrap();
    let mut native = prepared.snapshot().clone();
    native.actors[0].transform.translation[0] = tick as f32 / 100.0;
    native.actors[0].variables[0].value = NativeValue::I32(i32::try_from(tick).unwrap());
    WorldSnapshot {
        schema_version: SNAPSHOT_SCHEMA_NATIVE,
        tick,
        native_world: Some(native),
        preserved_prospect: preserved.cloned(),
        ..Default::default()
    }
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
            &string("UStorageAuthoredClass"),
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

fn read_json(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

fn sidecars(root: &Path) -> Vec<String> {
    let mut names: Vec<_> = fs::read_dir(root)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .filter(|name| is_sidecar_name(name))
        .collect();
    names.sort();
    names
}

fn rewrite(root: &Path, number: u64, value: &Value) {
    let bytes = serde_json::to_vec(value).unwrap();
    fs::write(root.join(format!("snapshot-{number}.json")), &bytes).unwrap();
    let mut manifest = read_json(&root.join("manifest.json"));
    let digest = Sha256::digest(&bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    for slot in ["current", "previous"] {
        if manifest[slot]["number"] == number {
            manifest[slot]["sha256"] = digest.clone().into();
        }
    }
    fs::write(
        root.join("manifest.json"),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();
}

#[test]
fn schema_four_round_trips_with_or_without_preservation_without_downconversion() {
    let section = section(1);
    for preserved in [None, Some(&section)] {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let original = snapshot(10, preserved);
        let mut store = SnapshotStore::open(root).unwrap();
        let receipt = store.save_new(&original).unwrap();
        assert_eq!(receipt.generation, 1);
        drop(store);
        let written = read_json(&root.join("snapshot-1.json"));
        assert_eq!(written["schema_version"], 4);
        assert_eq!(written["entities"], json!([]));
        assert_eq!(
            serde_json::from_value::<NativeWorldSnapshot>(written["native_world"].clone()).unwrap(),
            *original.native_world.as_ref().unwrap()
        );
        assert_eq!(
            written.get("preserved_prospect").is_some(),
            preserved.is_some()
        );
        let names = sidecars(root);
        assert_eq!(names.len(), usize::from(preserved.is_some()));
        if let Some(section) = preserved {
            assert_eq!(
                fs::read(root.join(&names[0])).unwrap(),
                section.native_encoding()
            );
            assert!(written["preserved_prospect"].get("data").is_none());
            assert_eq!(
                written["preserved_prospect"]["encoded_bytes"],
                section.native_encoding().len()
            );
        }
        let mut reopened = SnapshotStore::open(root).unwrap();
        let loaded = reopened.load().unwrap().unwrap();
        assert!(!loaded.recovered_previous);
        assert_eq!(loaded.snapshot, original);
        let mut next = loaded.snapshot;
        next.tick = 20;
        reopened.save(&next).unwrap();
        assert_eq!(
            read_json(&root.join("snapshot-2.json"))["schema_version"],
            4
        );
        assert_eq!(sidecars(root), names);
        assert_eq!(reopened.load().unwrap().unwrap().snapshot, next);
    }
}

#[test]
fn invalid_schema_world_combinations_never_publish() {
    let section = section(2);
    let original = snapshot(10, None);
    let mut invalid = Vec::new();
    let mut missing = original.clone();
    missing.native_world = None;
    invalid.push(missing);
    let mut mixed = original.clone();
    mixed.entities.push(EntityRecord {
        id: DomainId(1),
        position: Position([0.0; 3]),
    });
    invalid.push(mixed);
    for schema in [1, 2, 3] {
        let mut legacy = original.clone();
        legacy.schema_version = schema;
        legacy.preserved_prospect = (schema != 1).then(|| Arc::clone(&section));
        invalid.push(legacy);
    }
    for invalid in invalid {
        let directory = tempfile::tempdir().unwrap();
        let mut store = SnapshotStore::open(directory.path()).unwrap();
        assert!(store.save_new(&invalid).is_err());
        assert!(store.load().unwrap().is_none());
        assert!(!directory.path().join("snapshot-1.json").exists());
        assert!(sidecars(directory.path()).is_empty());
    }
}

#[test]
fn corrupt_or_tampered_native_generation_recovers_and_retains_valid_previous() {
    for damage in [
        "checksum",
        "identity",
        "definitions",
        "missing_world",
        "foundation_records",
    ] {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let mut store = SnapshotStore::open(root).unwrap();
        store.save(&snapshot(10, None)).unwrap();
        store.save(&snapshot(20, None)).unwrap();
        if damage == "checksum" {
            fs::write(root.join("snapshot-2.json"), b"partial").unwrap();
        } else {
            let mut current = read_json(&root.join("snapshot-2.json"));
            match damage {
                "identity" => current["native_world"]["catalog_id"] = "0".repeat(64).into(),
                "definitions" => {
                    current["native_world"]["definitions"]["classes"][0]["local_bounds"]["max"][0] =
                        0.75.into()
                }
                "missing_world" => {
                    current.as_object_mut().unwrap().remove("native_world");
                }
                "foundation_records" => {
                    current["entities"] = json!([{"id":1,"position":[0.0,0.0,0.0]}])
                }
                _ => unreachable!(),
            }
            // Recompute the outer checksum: schema and embedded identity checks
            // must detect the damaged world independently of file integrity.
            rewrite(root, 2, &current);
        }
        drop(store);
        let mut store = SnapshotStore::open(root).unwrap();
        let recovered = store.load().unwrap().unwrap();
        assert!(recovered.recovered_previous, "{damage}");
        assert_eq!(recovered.snapshot, snapshot(10, None), "{damage}");
        store.save(&snapshot(30, None)).unwrap();
        assert_eq!(
            read_json(&root.join("manifest.json"))["previous"]["number"],
            1
        );
        fs::write(root.join("snapshot-3.json"), b"partial").unwrap();
        assert_eq!(store.load().unwrap().unwrap().snapshot, snapshot(10, None));
    }
}

#[test]
fn native_disk_json_errors_redact_values_and_require_named_objects() {
    for damage in ["field", "world_array", "reference_array"] {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let mut store = SnapshotStore::open(root).unwrap();
        store.save(&snapshot(10, None)).unwrap();
        let mut current = read_json(&root.join("snapshot-1.json"));
        match damage {
            "field" => {
                current["native_world"]["AUTHORED_PRIVATE_FIELD_MARKER"] =
                    "AUTHORED_PRIVATE_VALUE_MARKER".into()
            }
            "world_array" => current["native_world"] = json!(["AUTHORED_PRIVATE_VALUE_MARKER"]),
            "reference_array" => {
                current["preserved_prospect"] = json!(["AUTHORED_PRIVATE_VALUE_MARKER"])
            }
            _ => unreachable!(),
        }
        rewrite(root, 1, &current);
        let error = format!("{:#}", store.load().unwrap_err());
        assert!(
            error.contains("decode native snapshot fields JSON"),
            "{error}"
        );
        assert!(!error.contains("_MARKER"), "{error}");
        assert!(!root.join("snapshot-2.json").exists());
    }
}

#[test]
fn unsupported_embedded_versions_are_terminal_for_loading_and_saving() {
    for future in [
        "format",
        "format_with_new_class_shape",
        "class_kind",
        "class_version",
    ] {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let mut store = SnapshotStore::open(root).unwrap();
        store.save(&snapshot(10, None)).unwrap();
        store.save(&snapshot(20, None)).unwrap();
        let mut current = read_json(&root.join("snapshot-2.json"));
        let definitions = &mut current["native_world"]["definitions"];
        match future {
            "format" => definitions["native_format"] = 2.into(),
            "format_with_new_class_shape" => {
                definitions["native_format"] = 2.into();
                definitions["classes"] = json!({"AUTHORED_FUTURE_MARKER": true});
            }
            "class_kind" => definitions["classes"][0]["kind"] = "AUTHORED_FUTURE_MARKER".into(),
            "class_version" => definitions["classes"][0]["kind"] = "spatial_object_v2".into(),
            _ => unreachable!(),
        }
        rewrite(root, 2, &current);
        let manifest = fs::read(root.join("manifest.json")).unwrap();
        let generation = fs::read(root.join("snapshot-2.json")).unwrap();
        drop(store);
        let mut store = SnapshotStore::open(root).unwrap();
        for error in [
            store.load().unwrap_err(),
            store.save(&snapshot(30, None)).unwrap_err(),
            store.save_new(&snapshot(30, None)).unwrap_err(),
        ] {
            let message = format!("{error:#}");
            assert!(
                message.contains("unsupported native snapshot schema"),
                "{future}: {message}"
            );
            assert!(!message.contains("_MARKER"), "{message}");
        }
        assert_eq!(fs::read(root.join("manifest.json")).unwrap(), manifest);
        assert_eq!(fs::read(root.join("snapshot-2.json")).unwrap(), generation);
        assert!(!root.join("snapshot-3.json").exists());
    }
}

#[test]
fn malformed_supported_discriminators_recover_normally() {
    for damage in ["format_type", "class_type", "known_class_data"] {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let mut store = SnapshotStore::open(root).unwrap();
        store.save(&snapshot(10, None)).unwrap();
        store.save(&snapshot(20, None)).unwrap();
        let mut current = read_json(&root.join("snapshot-2.json"));
        let definitions = &mut current["native_world"]["definitions"];
        match damage {
            "format_type" => definitions["native_format"] = "AUTHORED_MALFORMED_MARKER".into(),
            "class_type" => {
                definitions["classes"][0]["kind"] = json!(["AUTHORED_MALFORMED_MARKER"])
            }
            "known_class_data" => {
                definitions["classes"][0]["local_bounds"] = "AUTHORED_MALFORMED_MARKER".into()
            }
            _ => unreachable!(),
        }
        rewrite(root, 2, &current);
        let recovered = store.load().unwrap().unwrap();
        assert!(recovered.recovered_previous, "{damage}");
        assert_eq!(recovered.snapshot, snapshot(10, None));
    }
}

#[test]
fn optional_native_sidecars_recover_and_survive_until_both_references_retire() {
    let (first, second) = (section(3), section(4));
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let mut store = SnapshotStore::open(root).unwrap();
    store.save(&snapshot(10, Some(&first))).unwrap();
    let first_name = sidecars(root).pop().unwrap();
    store.save(&snapshot(20, Some(&second))).unwrap();
    let second_name = sidecars(root)
        .into_iter()
        .find(|name| name != &first_name)
        .unwrap();
    fs::write(root.join(&second_name), b"damaged").unwrap();
    drop(store);
    let mut store = SnapshotStore::open(root).unwrap();
    let recovered = store.load().unwrap().unwrap();
    assert!(recovered.recovered_previous);
    assert_eq!(recovered.snapshot, snapshot(10, Some(&first)));
    store.save(&snapshot(30, None)).unwrap();
    assert_eq!(sidecars(root), [first_name]);
    store.save(&snapshot(40, None)).unwrap();
    assert!(sidecars(root).is_empty());
    assert_eq!(store.load().unwrap().unwrap().snapshot, snapshot(40, None));
}

#[test]
fn save_new_refuses_native_current_and_recovered_previous_without_overwrite() {
    let preserved = section(5);
    for recover in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let mut store = SnapshotStore::open(root).unwrap();
        store.save_new(&snapshot(10, Some(&preserved))).unwrap();
        store.save(&snapshot(20, Some(&preserved))).unwrap();
        if recover {
            fs::write(root.join("snapshot-2.json"), b"partial").unwrap();
        }
        let manifest = fs::read(root.join("manifest.json")).unwrap();
        let generation = fs::read(root.join("snapshot-2.json")).unwrap();
        let names = sidecars(root);
        drop(store);
        let mut store = SnapshotStore::open(root).unwrap();
        assert_eq!(store.load().unwrap().unwrap().recovered_previous, recover);
        assert!(store.save_new(&snapshot(30, None)).is_err());
        assert_eq!(fs::read(root.join("manifest.json")).unwrap(), manifest);
        assert_eq!(fs::read(root.join("snapshot-2.json")).unwrap(), generation);
        assert_eq!(sidecars(root), names);
        assert!(!root.join("snapshot-3.json").exists());
    }
}

#[test]
fn save_worker_retains_native_world_and_optional_preservation_across_autosaves() {
    let preserved = section(6);
    for preserved in [None, Some(&preserved)] {
        let directory = tempfile::tempdir().unwrap();
        let mut store = SnapshotStore::open(directory.path()).unwrap();
        store.save_new(&snapshot(0, preserved)).unwrap();
        let mut worker = SaveWorker::spawn(store).unwrap();
        for tick in [10, 20, 30] {
            worker.submit(snapshot(tick, preserved)).unwrap();
            let receipt = worker
                .finish_before(Instant::now() + Duration::from_secs(5))
                .unwrap()
                .unwrap();
            assert_eq!(receipt.tick, tick);
        }
        // The manifest is published before the receipt; inspect while the
        // worker still holds its lock, without depending on thread teardown.
        let written = read_json(&directory.path().join("snapshot-4.json"));
        assert_eq!(written["schema_version"], 4);
        assert_eq!(written["tick"], 30);
        assert_eq!(
            serde_json::from_value::<NativeWorldSnapshot>(written["native_world"].clone()).unwrap(),
            snapshot(30, preserved).native_world.unwrap()
        );
        assert_eq!(
            sidecars(directory.path()).len(),
            usize::from(preserved.is_some())
        );
        assert!(!directory.path().join("snapshot-1.json").exists());
        assert!(!directory.path().join("snapshot-2.json").exists());
    }
}

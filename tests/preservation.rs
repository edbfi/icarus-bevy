//! Authored payloads only; every name and value is invented for these tests.
use std::{fs, path::Path, sync::Arc};

use icarus_bevy::{
    config::ServerConfig,
    experimental::component::MAX_BOUNDED_COMPONENT_BYTES,
    experimental::property_tree::NodeValue,
    persistence::SnapshotStore,
    preservation::{
        FieldValue, MAX_PRESERVED_ENCODED_BYTES, OpaqueReason, PreserveLimits, PreservedBody,
        PreservedProspect, PreservedRoot, PreservedSection, is_sidecar_name,
    },
    prospect::MAX_PAYLOAD_BYTES,
    simulation::{DomainId, EntityRecord, Position, WorldSnapshot, build_app, capture},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

/// Distinctive marker shared by every invented name and value below.
const MARKER: &str = "PreserveSentinel";
const CLASS: &str = "UPreserveSentinelClassQQ";
const OTHER_CLASS: &str = "APreserveSentinelOtherQQ";
const FIELD: &str = "PreserveSentinelFieldQQ";
const VALUE: &str = "PreserveSentinelValueQQ";

// Authored framing, independent of the readers.
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

fn struct_extra(name: &str) -> Vec<u8> {
    [string(name), vec![7; 16]].concat()
}

fn stream(fields: &[Vec<u8>]) -> Vec<u8> {
    [fields.concat(), string("None")].concat()
}

/// Tagged fields, terminal name and a four-byte object trailer flag.
fn component(fields: &[Vec<u8>], trailer: &[u8]) -> Vec<u8> {
    [stream(fields), trailer.to_vec()].concat()
}

/// A body that frames: an Int, a nested struct and a struct array.
fn framed_body() -> Vec<u8> {
    let element = stream(&[tag(FIELD, "IntProperty", &[], &9_i32.to_le_bytes())]);
    let elements = [element.clone(), element].concat();
    let array = [
        2_i32.to_le_bytes().as_slice(),
        &tag(FIELD, "StructProperty", &struct_extra(CLASS), &elements),
    ]
    .concat();
    component(
        &[
            tag(FIELD, "IntProperty", &[], &7_i32.to_le_bytes()),
            tag(
                FIELD,
                "StructProperty",
                &struct_extra(CLASS),
                &stream(&[tag(FIELD, "StrProperty", &[], &string(VALUE))]),
            ),
            tag(FIELD, "ArrayProperty", &string("StructProperty"), &array),
        ],
        &[0; 4],
    )
}

fn recorder(class: &str, body: &[u8], unknown: bool) -> Vec<u8> {
    let mut fields = Vec::new();
    if unknown {
        fields.push(tag(FIELD, "IntProperty", &[], &5_i32.to_le_bytes()));
    }
    fields.push(tag(
        "ComponentClassName",
        "StrProperty",
        &[],
        &string(class),
    ));
    fields.push(tag(
        "BinaryData",
        "ArrayProperty",
        &string("ByteProperty"),
        &[
            i32::try_from(body.len()).unwrap().to_le_bytes().as_slice(),
            body,
        ]
        .concat(),
    ));
    stream(&fields)
}

fn payload_with(recorders: &[Vec<u8>]) -> Vec<u8> {
    let shared = tag(
        "StateRecorderBlobs",
        "StructProperty",
        &struct_extra("StateRecorderBlob"),
        &recorders.concat(),
    );
    stream(&[
        tag(FIELD, "IntProperty", &[], &2_i32.to_le_bytes()),
        tag(
            "StateRecorderBlobs",
            "ArrayProperty",
            &string("StructProperty"),
            &[
                i32::try_from(recorders.len())
                    .unwrap()
                    .to_le_bytes()
                    .as_slice(),
                &shared,
            ]
            .concat(),
        ),
    ])
}

/// One framed body plus three opaque bodies, one per reachable reason.
fn mixed_payload() -> Vec<u8> {
    let rejected = component(
        &[tag(FIELD, "StrProperty", &[], &string(VALUE))],
        &[[1, 0, 0, 0].as_slice(), &[3; 16]].concat(),
    );
    let oversized = component(
        &[tag(
            FIELD,
            "StrProperty",
            &[],
            &vec![b'Q'; MAX_BOUNDED_COMPONENT_BYTES],
        )],
        &[0; 4],
    );
    payload_with(&[
        recorder(CLASS, &framed_body(), true),
        recorder(OTHER_CLASS, &rejected, false),
        recorder(CLASS, &oversized, false),
        recorder(OTHER_CLASS, &[], false),
    ])
}

fn preserve(payload: &[u8]) -> PreservedProspect {
    PreservedProspect::from_inner_payload(payload, PreserveLimits::default()).unwrap()
}

/// The compact form a running world holds and the snapshot store persists.
fn section(payload: &[u8]) -> Arc<PreservedSection> {
    Arc::new(PreservedSection::from_inner_payload(payload, PreserveLimits::default()).unwrap())
}

fn digest(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn bodies(preserved: &PreservedProspect) -> Vec<&PreservedBody> {
    let mut bodies = Vec::new();
    for root in preserved.roots() {
        if let PreservedRoot::Recorders(recorders) = root {
            for record in &recorders.records {
                for field in &record.fields {
                    if let FieldValue::BinaryData { body, .. } = &field.value {
                        bodies.push(body);
                    }
                }
            }
        }
    }
    bodies
}

/// The in-memory form capture produces and the store writes: schema 3.
fn preserved_snapshot(tick: u64, preserved: &Arc<PreservedSection>) -> WorldSnapshot {
    WorldSnapshot {
        schema_version: 3,
        tick,
        entities: vec![EntityRecord {
            id: DomainId(7),
            position: Position([1.0, 2.0, 3.0]),
        }],
        preserved_prospect: Some(Arc::clone(preserved)),
    }
}

fn read_json(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

/// Rewrite one generation and its manifest digest, keeping the store's own
/// checksum valid so the preserved section's checks are what is exercised.
fn rewrite_generation(root: &Path, number: u64, value: &Value) {
    let bytes = serde_json::to_vec(value).unwrap();
    fs::write(root.join(format!("snapshot-{number}.json")), &bytes).unwrap();
    let manifest_path = root.join("manifest.json");
    let mut manifest = read_json(&manifest_path);
    for slot in ["current", "previous"] {
        if manifest[slot]["number"] == number {
            manifest[slot]["sha256"] = hex(&digest(&bytes)).into();
        }
    }
    fs::write(manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
}

/// A schema-2 snapshot as an earlier binary loaded it; it serializes to the
/// legacy inline Base64 generation form.
fn legacy_snapshot(tick: u64, preserved: &Arc<PreservedSection>) -> WorldSnapshot {
    WorldSnapshot {
        schema_version: 2,
        ..preserved_snapshot(tick, preserved)
    }
}

/// Replace generation `number` with the inline schema-2 form of `snapshot`.
fn write_legacy(root: &Path, number: u64, snapshot: &WorldSnapshot) {
    rewrite_generation(root, number, &serde_json::to_value(snapshot).unwrap());
}

/// Expected sidecar name, derived independently of the held bytes: the
/// decoded tree is encoded again.
fn sidecar_name(preserved: &PreservedSection) -> String {
    format!(
        "preserved-{}.bin",
        hex(&digest(
            &preserved.decode().unwrap().to_native_encoding().unwrap()
        ))
    )
}

/// Sorted names of every sidecar-named entry in the store directory.
fn sidecars(root: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(root)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .filter(|name| is_sidecar_name(name))
        .collect();
    names.sort();
    names
}

fn assert_private(text: &str) {
    assert!(!text.contains(MARKER), "leaked fixture text: {text}");
}

/// A labeled in-place edit of one persisted preserved section.
type SectionMutation = (&'static str, fn(&mut Value));

#[test]
fn framed_and_opaque_bodies_reconstruct_byte_identically() {
    let payload = mixed_payload();
    let preserved = preserve(&payload);
    assert_eq!(preserved.to_inner_payload().unwrap(), payload);
    assert_eq!(preserved.payload_bytes(), payload.len());
    assert_eq!(preserved.payload_sha256(), digest(&payload));

    let reasons: Vec<_> = bodies(&preserved)
        .into_iter()
        .map(|body| match body {
            PreservedBody::Component(tree) => {
                assert_eq!(tree.to_bytes().unwrap(), framed_body());
                None
            }
            PreservedBody::Opaque { reason, .. } => Some(*reason),
        })
        .collect();
    assert_eq!(
        reasons,
        [
            None,
            Some(OpaqueReason::FramingRejected),
            Some(OpaqueReason::ExceedsComponentLimit),
            Some(OpaqueReason::FramingRejected),
        ]
    );

    let summary = preserved.summary();
    assert_eq!(summary.root_tags, 2);
    assert_eq!(summary.opaque_root_tags, 1);
    assert_eq!(summary.recorder_count, 4);
    assert_eq!(summary.recorder_fields, 9);
    assert_eq!(summary.unknown_recorder_fields, 1);
    assert_eq!(summary.framed_bodies, 1);
    assert_eq!(summary.framed_body_bytes, framed_body().len());
    // Int, Struct (+ Str), Array (+ two elements with one Int each).
    assert_eq!(summary.framed_property_nodes, 6);
    assert_eq!(summary.opaque_bodies, 3);
    assert_eq!(summary.opaque_framing_rejected, 2);
    assert_eq!(summary.opaque_exceeds_component_limit, 1);
    assert_eq!(summary.opaque_round_trip_mismatch, 0);
    assert!(summary.byte_identical);
    assert!(!summary.world_imported);
    assert!(!summary.values_interpreted);

    // Field order, including the unknown field, is retained.
    let PreservedRoot::Recorders(recorders) = &preserved.roots()[1] else {
        panic!("second root should be the recorder array");
    };
    assert!(matches!(
        recorders.records[0].fields[0].value,
        FieldValue::Unknown(_)
    ));
    assert!(matches!(
        recorders.records[0].fields[1].value,
        FieldValue::ClassName(_)
    ));
}

#[test]
fn native_encoding_round_trips_nested_trees_exactly() {
    let payload = mixed_payload();
    let preserved = preserve(&payload);
    let encoded = preserved.to_native_encoding().unwrap();
    let decoded = PreservedProspect::from_native_encoding(
        &encoded,
        preserved.payload_bytes(),
        preserved.payload_sha256(),
    )
    .unwrap();
    assert_eq!(decoded, preserved);
    assert_eq!(decoded.to_inner_payload().unwrap(), payload);
    // The encoding is canonical: re-encoding a decoded value is identical.
    assert_eq!(decoded.to_native_encoding().unwrap(), encoded);
    let PreservedBody::Component(tree) = bodies(&decoded)[0] else {
        panic!("framed body should survive as a tree");
    };
    assert!(matches!(
        tree.tree.properties[1].value,
        NodeValue::Struct(_)
    ));
    assert!(matches!(
        &tree.tree.properties[2].value,
        NodeValue::StructArray { elements, .. } if elements.len() == 2
    ));
}

#[test]
fn caller_limits_select_opaque_storage_and_invalid_limits_fail_first() {
    let payload = payload_with(&[recorder(CLASS, &framed_body(), false)]);
    let small = PreserveLimits {
        component_bytes: framed_body().len() - 1,
        ..PreserveLimits::default()
    };
    let preserved = PreservedProspect::from_inner_payload(&payload, small).unwrap();
    assert_eq!(preserved.summary().opaque_exceeds_component_limit, 1);
    assert_eq!(preserved.to_inner_payload().unwrap(), payload);

    // A one-node budget rejects the multi-node tree, which then stays opaque.
    let mut tight = PreserveLimits::default();
    tight.tree.max_nodes = 1;
    let preserved = PreservedProspect::from_inner_payload(&payload, tight).unwrap();
    assert_eq!(preserved.summary().opaque_framing_rejected, 1);
    assert_eq!(preserved.to_inner_payload().unwrap(), payload);

    for limits in [
        PreserveLimits {
            component_bytes: 0,
            ..PreserveLimits::default()
        },
        PreserveLimits {
            component_bytes: MAX_BOUNDED_COMPONENT_BYTES + 1,
            ..PreserveLimits::default()
        },
        {
            let mut limits = PreserveLimits::default();
            limits.tree.max_depth = 0;
            limits
        },
        {
            let mut limits = PreserveLimits::default();
            limits.tree.max_depth = 17;
            limits
        },
    ] {
        let error = PreservedProspect::from_inner_payload(&payload, limits).unwrap_err();
        assert!(format!("{error:#}").contains("limit outside"), "{error:#}");
    }

    // The payload cap is checked before any framing work.
    let oversized = vec![0; MAX_PAYLOAD_BYTES + 1];
    let error =
        PreservedProspect::from_inner_payload(&oversized, PreserveLimits::default()).unwrap_err();
    assert!(format!("{error:#}").contains("32 MiB"));
}

#[test]
fn incomplete_root_or_recorder_framing_never_yields_a_partial_structure() {
    let valid = payload_with(&[recorder(CLASS, &framed_body(), false)]);
    let no_recorders = stream(&[tag(FIELD, "IntProperty", &[], &2_i32.to_le_bytes())]);
    let trailing = [valid.as_slice(), &[0]].concat();
    let truncated = valid[..valid.len() - 1].to_vec();
    let duplicate = {
        let inner = &valid[..valid.len() - string("None").len()];
        let first_root = tag(FIELD, "IntProperty", &[], &2_i32.to_le_bytes()).len();
        [inner, &inner[first_root..], &string("None")].concat()
    };
    for input in [no_recorders, trailing, truncated, duplicate, Vec::new()] {
        let error =
            PreservedProspect::from_inner_payload(&input, PreserveLimits::default()).unwrap_err();
        assert_private(&format!("{error:#}"));
    }
}

#[test]
fn crafted_native_encodings_fail_closed_before_count_driven_allocation() {
    // Small enough that a count within the root ceiling cannot fit.
    let payload = payload_with(&[
        recorder(CLASS, &framed_body(), true),
        recorder(OTHER_CLASS, &[], false),
    ]);
    let preserved = preserve(&payload);
    let encoded = preserved.to_native_encoding().unwrap();
    let (bytes, sha) = (preserved.payload_bytes(), preserved.payload_sha256());
    let decode = |input: &[u8]| PreservedProspect::from_native_encoding(input, bytes, sha);

    // Root terminator chunk length, then the root count that follows it.
    let terminator = u32::from_le_bytes(encoded[..4].try_into().unwrap()) as usize;
    let count_at = 4 + terminator;
    for (offset, message) in [(0, "truncated"), (count_at, "exceeds encoding limit")] {
        let mut crafted = encoded.clone();
        crafted[offset..offset + 4].copy_from_slice(&u32::MAX.to_le_bytes());
        let error = decode(&crafted).unwrap_err();
        assert!(format!("{error:#}").contains(message), "{error:#}");
    }
    // A count within the ceiling but beyond what the remaining bytes can hold.
    let mut crafted = encoded.clone();
    crafted[count_at..count_at + 4].copy_from_slice(&4096_u32.to_le_bytes());
    let error = decode(&crafted).unwrap_err();
    assert!(format!("{error:#}").contains("remaining encoded bytes"));

    let mut trailing = encoded.clone();
    trailing.push(0);
    assert!(decode(&trailing).is_err());
    assert!(decode(&encoded[..encoded.len() - 1]).is_err());
    // Any changed preserved byte breaks the recorded digest.
    let mut flipped = encoded.clone();
    let middle = flipped.len() / 2;
    flipped[middle] ^= 0x40;
    assert!(decode(&flipped).is_err());
    let mut wrong_sha = sha;
    wrong_sha[0] ^= 1;
    for error in [
        PreservedProspect::from_native_encoding(&encoded, bytes, wrong_sha).unwrap_err(),
        PreservedProspect::from_native_encoding(&encoded, bytes - 1, sha).unwrap_err(),
        PreservedProspect::from_native_encoding(&encoded, MAX_PAYLOAD_BYTES + 1, sha).unwrap_err(),
        PreservedProspect::from_native_encoding(
            &vec![0; MAX_PRESERVED_ENCODED_BYTES + 1],
            bytes,
            sha,
        )
        .unwrap_err(),
    ] {
        assert_private(&format!("{error:#}"));
    }
}

#[test]
fn schema_one_snapshots_still_load_and_stay_schema_one() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = SnapshotStore::open(directory.path()).unwrap();
    let snapshot = WorldSnapshot {
        tick: 5,
        ..Default::default()
    };
    store.save(&snapshot).unwrap();
    let written = read_json(&directory.path().join("snapshot-1.json"));
    assert_eq!(written, json!({"schema_version":1,"tick":5,"entities":[]}));
    // A schema-1 file written by an earlier binary loads unchanged.
    rewrite_generation(
        directory.path(),
        1,
        &json!({"schema_version":1,"tick":5,"entities":[{"id":3,"position":[1.0,2.0,3.0]}]}),
    );
    let loaded = store.load().unwrap().unwrap();
    assert!(!loaded.recovered_previous);
    assert_eq!(loaded.snapshot.schema_version, 1);
    assert!(loaded.snapshot.preserved_prospect.is_none());
    assert_eq!(loaded.snapshot.entities[0].id, DomainId(3));

    // A preserved section is legal only under schemas 2 and 3, which need one.
    let preserved = section(&mixed_payload());
    let mut mismatched = preserved_snapshot(6, &preserved);
    mismatched.schema_version = 1;
    assert!(store.save(&mismatched).is_err());
    for schema_version in [2, 3] {
        let missing = WorldSnapshot {
            schema_version,
            tick: 6,
            ..Default::default()
        };
        assert!(store.save(&missing).is_err());
    }
    assert!(!directory.path().join("snapshot-2.json").exists());
    assert!(sidecars(directory.path()).is_empty());
}

#[test]
fn schema_three_round_trips_through_the_snapshot_store_and_sidecar() {
    let payload = mixed_payload();
    let preserved = section(&payload);
    // Independently encoded from a separately built tree.
    let encoded = preserve(&payload).to_native_encoding().unwrap();
    let directory = tempfile::tempdir().unwrap();
    let mut store = SnapshotStore::open(directory.path()).unwrap();
    // A schema-1 world upgrades to schema 3 when a section is first added.
    store
        .save(&WorldSnapshot {
            tick: 1,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(
        store
            .save(&preserved_snapshot(10, &preserved))
            .unwrap()
            .generation,
        2
    );
    drop(store);

    let written = read_json(&directory.path().join("snapshot-2.json"));
    assert_eq!(written["schema_version"], 3);
    let section = &written["preserved_prospect"];
    assert_eq!(section["encoding"], 1);
    assert_eq!(section["payload_bytes"], payload.len());
    assert_eq!(section["payload_sha256"], hex(&digest(&payload)));
    assert_eq!(section["encoded_bytes"], encoded.len());
    assert_eq!(section["encoded_sha256"], hex(&digest(&encoded)));
    assert_eq!(
        section.as_object().unwrap().keys().collect::<Vec<_>>(),
        [
            "encoded_bytes",
            "encoded_sha256",
            "encoding",
            "payload_bytes",
            "payload_sha256"
        ]
    );
    // The sidecar is the raw native encoding, named by its own digest.
    let name = sidecar_name(&preserved);
    assert_eq!(sidecars(directory.path()), std::slice::from_ref(&name));
    assert_eq!(fs::read(directory.path().join(&name)).unwrap(), encoded);
    assert_eq!(preserved.encoded_bytes(), encoded.len());
    assert_eq!(preserved.encoded_sha256(), digest(&encoded));
    assert_eq!(preserved.native_encoding(), encoded.as_slice());

    let store = SnapshotStore::open(directory.path()).unwrap();
    let loaded = store.load().unwrap().unwrap();
    assert!(!loaded.recovered_previous);
    assert_eq!(loaded.snapshot, preserved_snapshot(10, &preserved));
    let reloaded = loaded.snapshot.preserved_prospect.unwrap();
    assert!(!Arc::ptr_eq(&reloaded, &preserved));
    assert_eq!(
        reloaded.decode().unwrap().to_inner_payload().unwrap(),
        payload
    );
    assert_eq!(reloaded.native_encoding(), encoded.as_slice());
    assert_eq!(reloaded.summary(), preserved.summary());
    assert_eq!(reloaded.encoded_sha256(), preserved.encoded_sha256());
}

#[test]
fn later_saves_reuse_the_sidecar_without_rewriting_it() {
    use std::os::unix::fs::MetadataExt;

    let preserved = section(&mixed_payload());
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let mut store = SnapshotStore::open(root).unwrap();
    store.save(&preserved_snapshot(10, &preserved)).unwrap();
    let path = root.join(sidecar_name(&preserved));
    let identity = |path: &Path| {
        let metadata = fs::metadata(path).unwrap();
        (metadata.ino(), metadata.mtime(), metadata.mtime_nsec())
    };
    let before = identity(&path);
    // Long enough that a rewrite would show a different modification time.
    std::thread::sleep(std::time::Duration::from_millis(20));
    for tick in [20, 30] {
        store.save(&preserved_snapshot(tick, &preserved)).unwrap();
    }
    // A fresh store (new process) verifies and reuses the same file as well.
    drop(store);
    let mut reopened = SnapshotStore::open(root).unwrap();
    reopened.save(&preserved_snapshot(40, &preserved)).unwrap();
    assert_eq!(identity(&path), before);
    assert_eq!(sidecars(root), [sidecar_name(&preserved)]);
    // Generations stay small: the section is never inline.
    let generation = fs::metadata(root.join("snapshot-4.json")).unwrap().len();
    assert!(generation < 1024, "{generation}");
    assert_eq!(reopened.load().unwrap().unwrap().snapshot.tick, 40);
}

#[test]
fn schema_two_inline_worlds_migrate_to_schema_three_on_the_next_save() {
    let payload = mixed_payload();
    let preserved = section(&payload);
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let mut store = SnapshotStore::open(root).unwrap();
    store
        .save(&WorldSnapshot {
            tick: 1,
            ..Default::default()
        })
        .unwrap();
    write_legacy(root, 1, &legacy_snapshot(10, &preserved));
    drop(store);

    let store = SnapshotStore::open(root).unwrap();
    let loaded = store.load().unwrap().unwrap();
    assert_eq!(loaded.snapshot, legacy_snapshot(10, &preserved));
    assert!(sidecars(root).is_empty());
    // The running world captures schema 3, and the save writes the sidecar.
    let mut app = build_app(&ServerConfig::default(), &loaded.snapshot).unwrap();
    let captured = capture(app.world_mut()).unwrap();
    assert_eq!(captured.schema_version, 3);
    let mut store = store;
    store.save(&captured).unwrap();
    assert_eq!(
        read_json(&root.join("snapshot-2.json"))["schema_version"],
        3
    );
    assert_eq!(sidecars(root), [sidecar_name(&preserved)]);
    // The legacy generation stays retained as the previous one.
    assert_eq!(
        read_json(&root.join("snapshot-1.json"))["schema_version"],
        2
    );
    let manifest = read_json(&root.join("manifest.json"));
    assert_eq!(manifest["previous"]["number"], 1);
    drop(store);

    let mut store = SnapshotStore::open(root).unwrap();
    let reloaded = store.load().unwrap().unwrap();
    assert_eq!(reloaded.snapshot, preserved_snapshot(10, &preserved));
    assert_eq!(
        reloaded
            .snapshot
            .preserved_prospect
            .unwrap()
            .decode()
            .unwrap()
            .to_inner_payload()
            .unwrap(),
        payload
    );
    store.save(&preserved_snapshot(20, &preserved)).unwrap();
    assert!(!root.join("snapshot-1.json").exists());
    assert_eq!(sidecars(root), [sidecar_name(&preserved)]);
}

/// Each way a sidecar can be lost or damaged on storage.
fn damage_sidecar(path: &Path, damage: &str) {
    match damage {
        "missing" => fs::remove_file(path).unwrap(),
        "flipped" => {
            let mut bytes = fs::read(path).unwrap();
            let middle = bytes.len() / 2;
            bytes[middle] ^= 0x40;
            fs::write(path, bytes).unwrap();
        }
        "truncated" => {
            let bytes = fs::read(path).unwrap();
            fs::write(path, &bytes[..bytes.len() / 2]).unwrap();
        }
        "extended" => {
            let mut bytes = fs::read(path).unwrap();
            bytes.push(0);
            fs::write(path, bytes).unwrap();
        }
        "directory" => {
            fs::remove_file(path).unwrap();
            fs::create_dir(path).unwrap();
        }
        other => panic!("unknown damage {other}"),
    }
}

const SIDECAR_DAMAGE: [&str; 5] = ["missing", "flipped", "truncated", "extended", "directory"];

#[test]
fn damaged_current_sidecar_recovers_the_previous_generation() {
    let first = section(&payload_with(&[recorder(CLASS, &[1], false)]));
    let second = section(&payload_with(&[recorder(CLASS, &[2], false)]));
    for damage in SIDECAR_DAMAGE {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let mut store = SnapshotStore::open(root).unwrap();
        store.save(&preserved_snapshot(10, &first)).unwrap();
        store.save(&preserved_snapshot(20, &second)).unwrap();
        let damaged = root.join(sidecar_name(&second));
        damage_sidecar(&damaged, damage);
        let recovered = store.load().unwrap().unwrap();
        assert!(recovered.recovered_previous, "{damage}");
        assert_eq!(
            recovered.snapshot,
            preserved_snapshot(10, &first),
            "{damage}"
        );
        // Continuing from the recovered state publishes generation 3 with the
        // previous one retained; the damaged sidecar is then unreferenced.
        let receipt = store.save(&preserved_snapshot(30, &first)).unwrap();
        assert_eq!(receipt.generation, 3, "{damage}");
        assert_eq!(
            read_json(&root.join("manifest.json"))["previous"]["number"],
            1
        );
        assert_eq!(damaged.is_dir(), damage == "directory", "{damage}");
        assert!(!damaged.is_file(), "{damage}");
        assert_eq!(store.load().unwrap().unwrap().snapshot.tick, 30);
    }
}

#[test]
fn an_unverified_file_at_the_sidecar_name_is_replaced_never_trusted() {
    let preserved = section(&mixed_payload());
    let encoded = preserve(&mixed_payload()).to_native_encoding().unwrap();
    for damage in SIDECAR_DAMAGE {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let mut store = SnapshotStore::open(root).unwrap();
        store
            .save(&WorldSnapshot {
                tick: 10,
                ..Default::default()
            })
            .unwrap();
        // A stray file under the name, e.g. left by external tampering.
        let path = root.join(sidecar_name(&preserved));
        fs::write(&path, &encoded).unwrap();
        damage_sidecar(&path, damage);
        let manifest = fs::read(root.join("manifest.json")).unwrap();
        let outcome = store.save(&preserved_snapshot(20, &preserved));
        if damage == "directory" {
            // Nothing can replace a directory; the save fails before publication.
            assert_private(&format!("{:#}", outcome.unwrap_err()));
            assert!(!root.join("snapshot-2.json").exists());
            assert_eq!(fs::read(root.join("manifest.json")).unwrap(), manifest);
            continue;
        }
        outcome.unwrap();
        assert_eq!(fs::read(&path).unwrap(), encoded, "{damage}");
        assert_eq!(
            store.load().unwrap().unwrap().snapshot,
            preserved_snapshot(20, &preserved),
            "{damage}"
        );
    }
}

#[test]
fn damaged_sidecar_recovers_a_legacy_schema_two_previous_generation() {
    let preserved = section(&mixed_payload());
    for damage in SIDECAR_DAMAGE {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let mut store = SnapshotStore::open(root).unwrap();
        store.save(&preserved_snapshot(10, &preserved)).unwrap();
        write_legacy(root, 1, &legacy_snapshot(10, &preserved));
        store.save(&preserved_snapshot(20, &preserved)).unwrap();
        damage_sidecar(&root.join(sidecar_name(&preserved)), damage);
        let recovered = store.load().unwrap().unwrap();
        assert!(recovered.recovered_previous, "{damage}");
        assert_eq!(
            recovered.snapshot,
            legacy_snapshot(10, &preserved),
            "{damage}"
        );
    }
}

#[test]
fn damaged_shared_sidecar_fails_closed_like_two_corrupt_generations() {
    let preserved = section(&mixed_payload());
    for damage in SIDECAR_DAMAGE {
        for two_generations in [true, false] {
            let directory = tempfile::tempdir().unwrap();
            let root = directory.path();
            let mut store = SnapshotStore::open(root).unwrap();
            store.save(&preserved_snapshot(10, &preserved)).unwrap();
            let next = if two_generations {
                store.save(&preserved_snapshot(20, &preserved)).unwrap();
                3
            } else {
                2
            };
            damage_sidecar(&root.join(sidecar_name(&preserved)), damage);
            let manifest = fs::read(root.join("manifest.json")).unwrap();
            let error = store.load().unwrap_err();
            assert_private(&format!("{error:#}"));
            assert_private(&format!("{error:?}"));
            let error = store.save(&preserved_snapshot(30, &preserved)).unwrap_err();
            assert_private(&format!("{error:#}"));
            assert!(!root.join(format!("snapshot-{next}.json")).exists());
            assert_eq!(fs::read(root.join("manifest.json")).unwrap(), manifest);
        }
    }
}

#[test]
fn corrupted_sidecar_references_fail_closed() {
    let preserved = section(&mixed_payload());
    let mutations: [SectionMutation; 10] = [
        ("other sidecar", |section| {
            section["encoded_sha256"] = "0".repeat(64).into();
        }),
        ("uppercase sidecar digest", |section| {
            let upper = section["encoded_sha256"].as_str().unwrap().to_uppercase();
            section["encoded_sha256"] = upper.into();
        }),
        ("encoded length", |section| {
            let length = section["encoded_bytes"].as_u64().unwrap();
            section["encoded_bytes"] = (length - 1).into();
        }),
        ("oversized encoded length", |section| {
            section["encoded_bytes"] = (MAX_PRESERVED_ENCODED_BYTES + 1).into();
        }),
        ("payload digest", |section| {
            section["payload_sha256"] = "0".repeat(64).into();
        }),
        ("payload length", |section| {
            let length = section["payload_bytes"].as_u64().unwrap();
            section["payload_bytes"] = (length + 1).into();
        }),
        ("encoding", |section| section["encoding"] = 2.into()),
        ("inline data", |section| section["data"] = "AAAA".into()),
        ("missing field", |section| {
            section.as_object_mut().unwrap().remove("encoded_bytes");
        }),
        ("positional", |section| {
            *section = json!([
                section["encoding"].clone(),
                section["encoded_sha256"].clone()
            ]);
        }),
    ];
    for (label, mutate) in mutations {
        for two_generations in [true, false] {
            let directory = tempfile::tempdir().unwrap();
            let root = directory.path();
            let mut store = SnapshotStore::open(root).unwrap();
            store.save(&preserved_snapshot(10, &preserved)).unwrap();
            let current = if two_generations {
                store.save(&preserved_snapshot(20, &preserved)).unwrap();
                2
            } else {
                1
            };
            let mut value = read_json(&root.join(format!("snapshot-{current}.json")));
            mutate(&mut value["preserved_prospect"]);
            rewrite_generation(root, current, &value);
            if two_generations {
                let loaded = store.load().unwrap().unwrap();
                assert!(loaded.recovered_previous, "{label}");
                assert_eq!(loaded.snapshot.tick, 10, "{label}");
                // The intact shared sidecar is still referenced and retained.
                store.save(&preserved_snapshot(30, &preserved)).unwrap();
                assert_eq!(sidecars(root), [sidecar_name(&preserved)], "{label}");
            } else {
                let error = store.load().unwrap_err();
                assert_private(&format!("{error:#}"));
                let error = store.save(&preserved_snapshot(30, &preserved)).unwrap_err();
                assert_private(&format!("{error:#}"));
                assert!(!root.join("snapshot-2.json").exists(), "{label}");
            }
        }
    }
}

#[test]
fn unreferenced_sidecars_are_collected_only_after_both_generations_drop_them() {
    let first = section(&payload_with(&[recorder(CLASS, &[1], false)]));
    let second = section(&payload_with(&[recorder(CLASS, &[2], false)]));
    let (one, two) = (sidecar_name(&first), sidecar_name(&second));
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let mut store = SnapshotStore::open(root).unwrap();
    store.save(&preserved_snapshot(10, &first)).unwrap();
    store.save(&preserved_snapshot(20, &first)).unwrap();

    // Unrelated entries, including near-miss and directory sidecar names.
    let hexadecimal = "ab".repeat(32);
    let stray = format!("preserved-{hexadecimal}.bin");
    let keep = [
        "preserved-notes.txt".to_owned(),
        format!("preserved-{}.bin", hexadecimal.to_uppercase()),
        format!("preserved-{}.bin", &hexadecimal[1..]),
        format!("preserved-{hexadecimal}.bin.bak"),
        format!("xpreserved-{hexadecimal}.bin"),
        "operator-note".to_owned(),
    ];
    for name in &keep {
        fs::write(root.join(name), b"keep").unwrap();
    }
    let directory_sidecar = format!("preserved-{}.bin", "cd".repeat(32));
    fs::create_dir(root.join(&directory_sidecar)).unwrap();
    fs::write(root.join(&stray), b"unreferenced").unwrap();

    // Generation 3 uses the second section; generation 2 still holds the first.
    store.save(&preserved_snapshot(30, &second)).unwrap();
    let mut expected = vec![directory_sidecar.clone(), one.clone(), two.clone()];
    expected.sort();
    assert_eq!(sidecars(root), expected);
    // Generation 4 makes generation 3 previous; nothing retains the first.
    store.save(&preserved_snapshot(40, &second)).unwrap();
    let mut expected = vec![directory_sidecar.clone(), two.clone()];
    expected.sort();
    assert_eq!(sidecars(root), expected);
    for name in &keep {
        assert_eq!(fs::read(root.join(name)).unwrap(), b"keep", "{name}");
    }
    assert!(root.join(&directory_sidecar).is_dir());
    assert!(!root.join(&stray).exists());
    // Returning to the first section writes it once more, retaining both.
    store.save(&preserved_snapshot(50, &first)).unwrap();
    let mut expected = vec![directory_sidecar, one, two];
    expected.sort();
    assert_eq!(sidecars(root), expected);
    assert_eq!(
        store.load().unwrap().unwrap().snapshot,
        preserved_snapshot(50, &first)
    );
}

#[test]
fn schema_three_recovery_from_previous_generation_is_unchanged() {
    let payload = mixed_payload();
    let preserved = section(&payload);
    let directory = tempfile::tempdir().unwrap();
    let mut store = SnapshotStore::open(directory.path()).unwrap();
    store.save(&preserved_snapshot(10, &preserved)).unwrap();
    store.save(&preserved_snapshot(20, &preserved)).unwrap();
    fs::write(directory.path().join("snapshot-2.json"), b"partial").unwrap();
    let recovered = store.load().unwrap().unwrap();
    assert!(recovered.recovered_previous);
    assert_eq!(recovered.snapshot, preserved_snapshot(10, &preserved));
    assert_eq!(
        recovered
            .snapshot
            .preserved_prospect
            .unwrap()
            .decode()
            .unwrap()
            .to_inner_payload()
            .unwrap(),
        payload
    );
    assert_eq!(
        store
            .save(&preserved_snapshot(30, &preserved))
            .unwrap()
            .generation,
        3
    );
    fs::write(directory.path().join("snapshot-3.json"), b"partial").unwrap();
    assert_eq!(store.load().unwrap().unwrap().snapshot.tick, 10);
}

#[test]
fn corrupted_legacy_inline_sections_fail_closed() {
    let preserved = section(&mixed_payload());
    let mutations: [SectionMutation; 8] = [
        ("changed data", |section| {
            let data = section["data"].as_str().unwrap();
            let middle = data.len() / 2;
            let replacement = if &data[middle..=middle] == "A" {
                "B"
            } else {
                "A"
            };
            section["data"] =
                format!("{}{replacement}{}", &data[..middle], &data[middle + 1..]).into();
        }),
        ("unpadded data", |section| {
            let data = section["data"].as_str().unwrap().trim_end_matches('=');
            section["data"] = format!("{data}A").into();
        }),
        ("digest", |section| {
            section["payload_sha256"] = "0".repeat(64).into();
        }),
        ("uppercase digest", |section| {
            let upper = section["payload_sha256"].as_str().unwrap().to_uppercase();
            section["payload_sha256"] = upper.into();
        }),
        ("length", |section| {
            let length = section["payload_bytes"].as_u64().unwrap();
            section["payload_bytes"] = (length + 1).into();
        }),
        ("encoding", |section| section["encoding"] = 2.into()),
        ("unknown field", |section| section["extra"] = true.into()),
        ("positional", |section| {
            *section = json!([section["encoding"].clone(), section["data"].clone()]);
        }),
    ];
    for (label, mutate) in mutations {
        for two_generations in [true, false] {
            let directory = tempfile::tempdir().unwrap();
            let root = directory.path();
            let mut store = SnapshotStore::open(root).unwrap();
            store.save(&preserved_snapshot(10, &preserved)).unwrap();
            write_legacy(root, 1, &legacy_snapshot(10, &preserved));
            let current = if two_generations {
                store.save(&preserved_snapshot(20, &preserved)).unwrap();
                2
            } else {
                1
            };
            // The current generation becomes a mutated inline schema-2 one.
            let tick = current * 10;
            let mut value = serde_json::to_value(legacy_snapshot(tick, &preserved)).unwrap();
            mutate(&mut value["preserved_prospect"]);
            rewrite_generation(root, current, &value);
            if two_generations {
                let loaded = store.load().unwrap();
                let loaded = loaded.unwrap();
                assert!(loaded.recovered_previous, "{label}");
                assert_eq!(loaded.snapshot.schema_version, 2, "{label}");
                assert_eq!(loaded.snapshot.tick, 10, "{label}");
            } else {
                let error = store.load().unwrap_err();
                assert_private(&format!("{error:#}"));
                let error = store.save(&preserved_snapshot(30, &preserved)).unwrap_err();
                assert_private(&format!("{error:#}"));
                assert!(!root.join("snapshot-2.json").exists(), "{label}");
            }
        }
    }
}

#[test]
fn capture_carries_the_preserved_section_forward_unchanged() {
    let preserved = section(&mixed_payload());
    let mut app = build_app(&ServerConfig::default(), &preserved_snapshot(3, &preserved)).unwrap();
    let captured = capture(app.world_mut()).unwrap();
    assert_eq!(captured.schema_version, 3);
    assert!(Arc::ptr_eq(
        captured.preserved_prospect.as_ref().unwrap(),
        &preserved
    ));
    assert_eq!(captured, preserved_snapshot(3, &preserved));

    let mut plain = build_app(&ServerConfig::default(), &WorldSnapshot::default()).unwrap();
    let captured = capture(plain.world_mut()).unwrap();
    assert_eq!(captured.schema_version, 1);
    assert!(captured.preserved_prospect.is_none());
}

#[test]
fn debug_summary_and_errors_never_expose_names_or_values() {
    let payload = mixed_payload();
    assert!(String::from_utf8_lossy(&payload).contains(MARKER));
    let preserved = section(&payload);
    let tree = preserved.decode().unwrap();
    assert_private(&format!("{preserved:?}"));
    assert_private(&format!("{preserved:#?}"));
    assert_private(&format!("{tree:?}"));
    assert_private(&format!("{:#?}", tree.roots()));
    assert_private(&serde_json::to_string(preserved.summary()).unwrap());
    assert_private(&format!("{:#?}", preserved_snapshot(1, &preserved)));

    // The generation holds only the sidecar reference: digests and lengths.
    // The sidecar itself is the raw, lossless native encoding by design.
    let directory = tempfile::tempdir().unwrap();
    let mut store = SnapshotStore::open(directory.path()).unwrap();
    store.save(&preserved_snapshot(1, &preserved)).unwrap();
    assert_private(&String::from_utf8_lossy(
        &fs::read(directory.path().join("snapshot-1.json")).unwrap(),
    ));
    assert_private(&format!("{:#?}", store.load().unwrap()));
}

#[test]
fn compact_section_round_trips_and_matches_the_tree() {
    let payload = mixed_payload();
    let tree = preserve(&payload);
    let encoded = tree.to_native_encoding().unwrap();
    let held = PreservedSection::from_inner_payload(&payload, PreserveLimits::default()).unwrap();
    assert_eq!(held.native_encoding(), encoded.as_slice());
    assert_eq!(held.payload_bytes(), payload.len());
    assert_eq!(held.payload_sha256(), digest(&payload));
    assert_eq!(held.encoded_bytes(), encoded.len());
    assert_eq!(held.encoded_sha256(), digest(&encoded));
    assert_eq!(held.summary(), tree.summary());

    // Every construction path yields the same compact value.
    let of_tree = PreservedSection::of(&tree).unwrap();
    let from_encoding =
        PreservedSection::from_native_encoding(encoded.clone(), payload.len(), digest(&payload))
            .unwrap();
    assert_eq!(of_tree, held);
    assert_eq!(from_encoding, held);
    assert_eq!(from_encoding.native_encoding(), encoded.as_slice());

    // On-demand decoding rebuilds the full tree and the exact payload.
    let decoded = held.decode().unwrap();
    assert_eq!(decoded, tree);
    assert_eq!(decoded.to_inner_payload().unwrap(), payload);
    assert_eq!(decoded.to_native_encoding().unwrap(), encoded);
    assert_eq!(bodies(&decoded).len(), 4);
}

#[test]
fn compact_section_from_a_damaged_encoding_fails_closed() {
    let payload = payload_with(&[
        recorder(CLASS, &framed_body(), true),
        recorder(OTHER_CLASS, &[], false),
    ]);
    let encoded = preserve(&payload).to_native_encoding().unwrap();
    let (bytes, sha) = (payload.len(), digest(&payload));
    let build = |input: &[u8]| PreservedSection::from_native_encoding(input.to_vec(), bytes, sha);
    assert!(build(&encoded).is_ok());

    let mut flipped = encoded.clone();
    let middle = flipped.len() / 2;
    flipped[middle] ^= 0x40;
    let mut trailing = encoded.clone();
    trailing.push(0);
    let mut huge_count = encoded.clone();
    huge_count[..4].copy_from_slice(&u32::MAX.to_le_bytes());
    let mut wrong_sha = sha;
    wrong_sha[0] ^= 1;
    let errors = [
        build(&encoded[..encoded.len() - 1]).unwrap_err(),
        build(&encoded[..encoded.len() / 2]).unwrap_err(),
        build(&[]).unwrap_err(),
        build(&flipped).unwrap_err(),
        build(&trailing).unwrap_err(),
        build(&huge_count).unwrap_err(),
        PreservedSection::from_native_encoding(encoded.clone(), bytes, wrong_sha).unwrap_err(),
        PreservedSection::from_native_encoding(encoded.clone(), bytes - 1, sha).unwrap_err(),
        PreservedSection::from_native_encoding(encoded.clone(), MAX_PAYLOAD_BYTES + 1, sha)
            .unwrap_err(),
        // The encoding cap is checked before hashing or decoding.
        PreservedSection::from_native_encoding(
            vec![0; MAX_PRESERVED_ENCODED_BYTES + 1],
            bytes,
            sha,
        )
        .unwrap_err(),
    ];
    for error in errors {
        assert_private(&format!("{error:#}"));
        assert_private(&format!("{error:?}"));
    }
    // Every single-byte change is rejected or still reconstructs the payload.
    for index in 0..encoded.len() {
        let mut altered = encoded.clone();
        altered[index] ^= 0x01;
        if let Ok(section) = build(&altered) {
            assert_eq!(
                section.decode().unwrap().to_inner_payload().unwrap(),
                payload,
                "byte {index}"
            );
        }
    }
}

#[test]
fn compact_section_holds_only_its_encoding_plus_a_small_constant() {
    let fixed = std::mem::size_of::<PreservedSection>();
    assert!(fixed <= 512, "{fixed}");
    let held = section(&mixed_payload());
    assert_eq!(held.held_bytes(), held.encoded_bytes() + fixed);
    // The same constant at a larger size: only the encoding grows.
    let large = section(&payload_with(&[recorder(
        CLASS,
        &vec![b'Q'; 1 << 20],
        false,
    )]));
    assert!(large.encoded_bytes() > 1 << 20);
    assert_eq!(large.held_bytes(), large.encoded_bytes() + fixed);
}

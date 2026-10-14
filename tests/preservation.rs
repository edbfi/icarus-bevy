//! Authored payloads only; every name and value is invented for these tests.
use std::{fs, path::Path, sync::Arc};

use icarus_bevy::{
    config::ServerConfig,
    experimental::component::MAX_BOUNDED_COMPONENT_BYTES,
    experimental::property_tree::NodeValue,
    persistence::SnapshotStore,
    preservation::{
        FieldValue, MAX_PRESERVED_ENCODED_BYTES, OpaqueReason, PreserveLimits, PreservedBody,
        PreservedProspect, PreservedRoot,
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

fn preserved_snapshot(tick: u64, preserved: &Arc<PreservedProspect>) -> WorldSnapshot {
    WorldSnapshot {
        schema_version: 2,
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

    // A preserved section is legal only under schema 2, and schema 2 needs one.
    let preserved = Arc::new(preserve(&mixed_payload()));
    let mut mismatched = preserved_snapshot(6, &preserved);
    mismatched.schema_version = 1;
    assert!(store.save(&mismatched).is_err());
    let missing = WorldSnapshot {
        schema_version: 2,
        tick: 6,
        ..Default::default()
    };
    assert!(store.save(&missing).is_err());
    assert!(!directory.path().join("snapshot-2.json").exists());
}

#[test]
fn schema_two_round_trips_through_the_snapshot_store() {
    let payload = mixed_payload();
    let preserved = Arc::new(preserve(&payload));
    let directory = tempfile::tempdir().unwrap();
    let mut store = SnapshotStore::open(directory.path()).unwrap();
    // A schema-1 world upgrades to schema 2 when a section is first added.
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
    assert_eq!(written["schema_version"], 2);
    let section = &written["preserved_prospect"];
    assert_eq!(section["encoding"], 1);
    assert_eq!(section["payload_bytes"], payload.len());
    assert_eq!(section["payload_sha256"], hex(&digest(&payload)));
    assert_eq!(
        section.as_object().unwrap().keys().collect::<Vec<_>>(),
        ["data", "encoding", "payload_bytes", "payload_sha256"]
    );

    let store = SnapshotStore::open(directory.path()).unwrap();
    let loaded = store.load().unwrap().unwrap();
    assert!(!loaded.recovered_previous);
    assert_eq!(loaded.snapshot, preserved_snapshot(10, &preserved));
    let reloaded = loaded.snapshot.preserved_prospect.unwrap();
    assert!(!Arc::ptr_eq(&reloaded, &preserved));
    assert_eq!(reloaded.to_inner_payload().unwrap(), payload);
    assert_eq!(reloaded.summary(), preserved.summary());
}

#[test]
fn schema_two_recovery_from_previous_generation_is_unchanged() {
    let payload = mixed_payload();
    let preserved = Arc::new(preserve(&payload));
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
fn corrupted_preserved_sections_fail_closed() {
    let preserved = Arc::new(preserve(&mixed_payload()));
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
                let loaded = store.load().unwrap();
                let loaded = loaded.unwrap();
                assert!(loaded.recovered_previous, "{label}");
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
    let preserved = Arc::new(preserve(&mixed_payload()));
    let mut app = build_app(&ServerConfig::default(), &preserved_snapshot(3, &preserved)).unwrap();
    let captured = capture(app.world_mut()).unwrap();
    assert_eq!(captured.schema_version, 2);
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
    let preserved = Arc::new(preserve(&payload));
    assert_private(&format!("{preserved:?}"));
    assert_private(&format!("{:#?}", preserved.roots()));
    assert_private(&serde_json::to_string(preserved.summary()).unwrap());
    assert_private(&format!("{:#?}", preserved_snapshot(1, &preserved)));

    // The persisted section holds only encoded bytes, never plain fixture text.
    let directory = tempfile::tempdir().unwrap();
    let mut store = SnapshotStore::open(directory.path()).unwrap();
    store.save(&preserved_snapshot(1, &preserved)).unwrap();
    assert_private(&String::from_utf8_lossy(
        &fs::read(directory.path().join("snapshot-1.json")).unwrap(),
    ));
}

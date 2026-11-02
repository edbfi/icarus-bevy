//! Authored envelopes only; class and field names are invented for these tests.
use std::io::{Read, Write};

use anyhow::anyhow;
use base64::{Engine, engine::general_purpose::STANDARD};
use flate2::{Compression, write::ZlibEncoder};
use icarus_bevy::experimental::component::MAX_BOUNDED_COMPONENT_BYTES;
use icarus_bevy::experimental::tagged::TagKind;
use icarus_bevy::prospect::{ClassComponentReport, ComponentVisit, visit_class_components};
use serde_json::{Value, json};
use sha1::{Digest, Sha1};

const SELECTED: &str = "UExampleSave";
const OTHER: &str = "AFooActor";
const LIMIT: usize = 64 * 1024;

// Authored framing, independent of the reader and archive codec encoder.
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

fn utf16_string(value: &str) -> Vec<u8> {
    let units: Vec<u16> = value.encode_utf16().chain([0]).collect();
    [
        (-i32::try_from(units.len()).unwrap())
            .to_le_bytes()
            .as_slice(),
        &units
            .iter()
            .flat_map(|unit| unit.to_le_bytes())
            .collect::<Vec<_>>(),
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

/// A base-component candidate: one IntProperty, a terminal name and a false flag.
fn component(field: &str, value: i32) -> Vec<u8> {
    [
        tag(field, "IntProperty", &[], &value.to_le_bytes()),
        string("None"),
        vec![0; 4],
    ]
    .concat()
}

fn payload(recorders: &[(Vec<u8>, Vec<u8>)]) -> Vec<u8> {
    let fields: Vec<u8> = recorders
        .iter()
        .flat_map(|(class, binary)| {
            [
                tag("ComponentClassName", "StrProperty", &[], class),
                tag(
                    "BinaryData",
                    "ArrayProperty",
                    &string("ByteProperty"),
                    &[
                        i32::try_from(binary.len())
                            .unwrap()
                            .to_le_bytes()
                            .as_slice(),
                        binary,
                    ]
                    .concat(),
                ),
                string("None"),
            ]
            .concat()
        })
        .collect();
    let shared = tag(
        "StateRecorderBlobs",
        "StructProperty",
        &[string("StateRecorderBlob"), vec![0; 16]].concat(),
        &fields,
    );
    [
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
        string("None"),
    ]
    .concat()
}

fn envelope_value(payload: &[u8]) -> Value {
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::fast());
    encoder.write_all(payload).unwrap();
    let compressed = encoder.finish().unwrap();
    let hash = Sha1::digest(payload)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    json!({"ProspectBlob": {
        "Hash": hash, "TotalLength": compressed.len(), "DataLength": 0,
        "UncompressedLength": payload.len(), "BinaryBlob": STANDARD.encode(compressed)
    }})
}

fn envelope(recorders: &[(Vec<u8>, Vec<u8>)]) -> Vec<u8> {
    serde_json::to_vec(&envelope_value(&payload(recorders))).unwrap()
}

/// Interleaved classes; selected recorders carry distinct field names and values.
fn interleaved() -> Vec<(Vec<u8>, Vec<u8>)> {
    vec![
        (string(SELECTED), component("ExampleSecretAlpha", 11)),
        (string(OTHER), component("FooHiddenField", 99)),
        (string(SELECTED), component("ExampleSecretBravo", 22)),
        (string(OTHER), component("FooHiddenOther", 98)),
        (string(SELECTED), component("ExampleSecretCharlie", 33)),
    ]
}

#[derive(Debug, PartialEq)]
enum Seen {
    Framed {
        index: usize,
        binary: usize,
        fields: Vec<(Vec<u8>, TagKind, usize)>,
    },
    Failed {
        index: usize,
        binary: usize,
    },
}

fn collect(
    input: &[u8],
    class: &str,
    limit: usize,
) -> (anyhow::Result<ClassComponentReport>, Vec<Seen>) {
    let mut seen = Vec::new();
    let result = visit_class_components(input, class, limit, |visit: ComponentVisit<'_>| {
        seen.push(match visit.component {
            Ok(component) => Seen::Framed {
                index: visit.recorder_index,
                binary: visit.binary_bytes,
                fields: component
                    .fields()
                    .iter()
                    .map(|field| (field.header.to_vec(), field.kind, field.payload.len()))
                    .collect(),
            },
            Err(_) => Seen::Failed {
                index: visit.recorder_index,
                binary: visit.binary_bytes,
            },
        });
        Ok(())
    });
    (result, seen)
}

fn header(field: &str) -> Vec<u8> {
    let full = tag(field, "IntProperty", &[], &[0; 4]);
    full[..full.len() - 4].to_vec()
}

#[test]
fn only_the_selected_class_is_visited_in_array_order() {
    let recorders = interleaved();
    let (report, seen) = collect(&envelope(&recorders), SELECTED, LIMIT);
    let report = report.unwrap();
    let expected: Vec<Seen> = [
        (0, "ExampleSecretAlpha"),
        (2, "ExampleSecretBravo"),
        (4, "ExampleSecretCharlie"),
    ]
    .into_iter()
    .map(|(index, field)| Seen::Framed {
        index,
        binary: recorders[index].1.len(),
        fields: vec![(header(field), TagKind::Int, 4)],
    })
    .collect();
    assert_eq!(seen, expected);
    assert_eq!(
        (
            report.recorder_count,
            report.matched_count,
            report.framed_count,
            report.failed_count
        ),
        (5, 3, 3, 0)
    );
    assert_eq!(
        report.validation,
        "experimental_class_component_framing_only"
    );
    assert!(!report.world_imported);

    let (report, seen) = collect(&envelope(&recorders), OTHER, LIMIT);
    assert_eq!(
        seen.iter()
            .map(|s| match s {
                Seen::Framed { index, .. } | Seen::Failed { index, .. } => *index,
            })
            .collect::<Vec<_>>(),
        [1, 3]
    );
    assert_eq!(report.unwrap().matched_count, 2);
}

#[test]
fn utf16_class_strings_match_by_exact_units() {
    let class = "UExampleSäve";
    let recorders = vec![
        (utf16_string(class), component("ExampleSecretAlpha", 1)),
        (string(SELECTED), component("ExampleSecretBravo", 2)),
    ];
    let (report, seen) = collect(&envelope(&recorders), class, LIMIT);
    assert_eq!(report.unwrap().matched_count, 1);
    assert!(matches!(seen.as_slice(), [Seen::Framed { index: 0, .. }]));
}

#[test]
fn framing_failures_are_reported_and_the_walk_continues() {
    let mut recorders = interleaved();
    recorders[2].1 = b"not-a-component".to_vec();
    let mut guid = component("ExampleSecretDelta", 4);
    let flag = guid.len() - 4;
    guid[flag] = 1;
    recorders.push((string(SELECTED), guid));
    let (report, seen) = collect(&envelope(&recorders), SELECTED, LIMIT);
    let report = report.unwrap();
    assert_eq!(
        (
            report.matched_count,
            report.framed_count,
            report.failed_count
        ),
        (4, 2, 2)
    );
    assert!(matches!(
        seen.as_slice(),
        [
            Seen::Framed { index: 0, .. },
            Seen::Failed {
                index: 2,
                binary: 15
            },
            Seen::Framed { index: 4, .. },
            Seen::Failed { index: 5, .. }
        ]
    ));

    // A selected limit below a body's size is a per-recorder failure, not an abort.
    let small = recorders[0].1.len() - 1;
    let (report, seen) = collect(&envelope(&interleaved()), SELECTED, small);
    let report = report.unwrap();
    assert_eq!((report.framed_count, report.failed_count), (0, 3));
    assert_eq!(seen.len(), 3);
}

#[test]
fn visitor_error_aborts_with_that_error() {
    let input = envelope(&interleaved());
    let mut calls = 0;
    let error = visit_class_components(input.as_slice(), SELECTED, LIMIT, |visit| {
        calls += 1;
        if visit.recorder_index == 2 {
            return Err(anyhow!("visitor-stop-sentinel"));
        }
        Ok(())
    })
    .unwrap_err();
    assert_eq!(error.to_string(), "visitor-stop-sentinel");
    assert_eq!(error.chain().count(), 1);
    assert_eq!(calls, 2);
}

#[test]
fn limits_outside_the_hard_ceiling_are_rejected_before_reading() {
    struct Unread;
    impl Read for Unread {
        fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
            panic!("input must not be read");
        }
    }
    for limit in [0, MAX_BOUNDED_COMPONENT_BYTES + 1, usize::MAX] {
        let error =
            visit_class_components(Unread, SELECTED, limit, |_| panic!("no visit")).unwrap_err();
        assert!(error.to_string().contains("component byte limit"));
    }
    let (report, seen) = collect(
        &envelope(&interleaved()),
        SELECTED,
        MAX_BOUNDED_COMPONENT_BYTES,
    );
    assert_eq!(report.unwrap().framed_count, 3);
    assert_eq!(seen.len(), 3);
}

#[test]
fn non_matching_classes_visit_nothing() {
    let input = envelope(&interleaved());
    for class in [
        "UMissingThing",
        "uexamplesave",
        "UExample",
        "UExampleSaveX",
        "",
        " UExampleSave",
    ] {
        let (report, seen) = collect(&input, class, LIMIT);
        let report = report.unwrap();
        assert!(seen.is_empty(), "{class:?}");
        assert_eq!(
            (
                report.recorder_count,
                report.matched_count,
                report.framed_count,
                report.failed_count
            ),
            (5, 0, 0, 0)
        );
    }
}

#[test]
fn report_serializes_counts_without_class_or_field_strings() {
    let (report, _) = collect(&envelope(&interleaved()), SELECTED, LIMIT);
    let report = report.unwrap();
    let text = serde_json::to_string(&report).unwrap();
    for secret in [
        SELECTED,
        OTHER,
        "ExampleSecret",
        "FooHidden",
        "ComponentClassName",
        "BinaryData",
        "IntProperty",
    ] {
        assert!(!text.contains(secret), "{secret}");
        assert!(!format!("{report:?}").contains(secret), "{secret}");
    }
    assert_eq!(
        serde_json::to_value(&report).unwrap(),
        json!({
            "validation": "experimental_class_component_framing_only", "recorder_count": 5,
            "matched_count": 3, "framed_count": 3, "failed_count": 0, "world_imported": false
        })
    );
}

#[test]
fn invalid_envelope_or_framing_fails_before_any_visit() {
    let mut document = envelope_value(&payload(&interleaved()));
    document["ProspectBlob"]["Hash"] = json!("0".repeat(40));
    let bytes = serde_json::to_vec(&document).unwrap();
    let error = visit_class_components(bytes.as_slice(), SELECTED, LIMIT, |_| panic!("no visit"))
        .unwrap_err();
    assert!(error.to_string().contains("legacy SHA-1 mismatch"));

    let bytes = serde_json::to_vec(&envelope_value(b"not a tagged archive")).unwrap();
    let error = visit_class_components(bytes.as_slice(), SELECTED, LIMIT, |_| panic!("no visit"))
        .unwrap_err();
    assert!(error.to_string().contains("invalid experimental root tags"));

    // A valid first recorder must not be visited when a later recorder is malformed.
    let mut body = payload(&interleaved());
    let end = body.len() - string("None").len();
    body.insert(end - 1, 0xaa);
    let bytes = serde_json::to_vec(&envelope_value(&body)).unwrap();
    let error = visit_class_components(bytes.as_slice(), SELECTED, LIMIT, |_| panic!("no visit"))
        .unwrap_err();
    assert!(
        error.to_string().contains("invalid experimental"),
        "{error:#}"
    );
}

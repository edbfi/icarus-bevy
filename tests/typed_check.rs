use std::{fs, io::Write, process::Command};

use base64::{Engine, engine::general_purpose::STANDARD};
use flate2::{Compression, write::ZlibEncoder};
use icarus_bevy::experimental::recorders::recorder_class_label;
use icarus_bevy::prospect::typed_check;
use serde_json::{Value, json};
use sha1::{Digest, Sha1};
use tempfile::TempDir;

const NO_RECORDER: &str = "/Script/Icarus.NoRecorderComponent";

// Authored framing, independent of the readers and the archive encoder.
fn fstring(value: &str) -> Vec<u8> {
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

fn tag(name: &str, kind: &str, extras: &[u8], payload: &[u8]) -> Vec<u8> {
    [
        fstring(name).as_slice(),
        &fstring(kind),
        &i32::try_from(payload.len()).unwrap().to_le_bytes(),
        &0_i32.to_le_bytes(),
        extras,
        payload,
    ]
    .concat()
}

fn struct_tag(name: &str, type_name: &str, payload: &[u8]) -> Vec<u8> {
    tag(
        name,
        "StructProperty",
        &[fstring(type_name), vec![0; 17]].concat(),
        payload,
    )
}

fn end() -> Vec<u8> {
    fstring("None")
}

fn floats(values: &[f32]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_le_bytes()).collect()
}

/// One base component: a secret string, a transform and a mystery struct
/// when `gap` is set, then `None` and the four-byte false trailer.
fn component(gap: bool) -> Vec<u8> {
    let transform = [
        struct_tag("Rotation", "Quat", &floats(&[0.0, 0.0, 0.0, 1.0])),
        struct_tag("Translation", "Vector", &floats(&[10.0, 20.0, 30.0])),
        struct_tag("Scale3D", "Vector", &floats(&[1.0, 1.0, 1.0])),
        end(),
    ]
    .concat();
    let mut fields = vec![
        tag("SecretField", "StrProperty", &[0], &fstring("secret-value")),
        struct_tag("Where", "Transform", &transform),
    ];
    if gap {
        fields.push(struct_tag("Mystery", "SecretBlobType", &[1, 2, 3]));
    }
    fields.push(end());
    [fields.concat(), vec![0; 4]].concat()
}

fn recorder(class: &str, body: &[u8]) -> Vec<u8> {
    let count = i32::try_from(body.len()).unwrap().to_le_bytes();
    [
        tag("ComponentClassName", "StrProperty", &[0], &fstring(class)),
        tag(
            "BinaryData",
            "ArrayProperty",
            &[fstring("ByteProperty"), vec![0]].concat(),
            &[count.as_slice(), body].concat(),
        ),
        end(),
    ]
    .concat()
}

fn payload(recorders: &[Vec<u8>]) -> Vec<u8> {
    let info = [
        tag(
            "ProspectID",
            "StrProperty",
            &[0],
            &fstring("secret-prospect"),
        ),
        end(),
    ]
    .concat();
    let shared = tag(
        "StateRecorderBlobs",
        "StructProperty",
        &[fstring("StateRecorderBlob"), vec![0; 17]].concat(),
        &recorders.concat(),
    );
    let count = i32::try_from(recorders.len()).unwrap().to_le_bytes();
    [
        tag("Version", "IntProperty", &[0], &2_i32.to_le_bytes()),
        struct_tag("ProspectInfo", "ProspectInfo", &info),
        tag(
            "StateRecorderBlobs",
            "ArrayProperty",
            &[fstring("StructProperty"), vec![0]].concat(),
            &[count.as_slice(), &shared].concat(),
        ),
        end(),
    ]
    .concat()
}

fn envelope(payload: &[u8]) -> Vec<u8> {
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::fast());
    encoder.write_all(payload).unwrap();
    let compressed = encoder.finish().unwrap();
    let hash = Sha1::digest(payload)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    serde_json::to_vec(&json!({"ProspectBlob": {
        "Hash": hash, "TotalLength": compressed.len(), "DataLength": 0,
        "UncompressedLength": payload.len(), "BinaryBlob": STANDARD.encode(compressed)
    }}))
    .unwrap()
}

#[test]
fn class_labels_cover_registered_paths_only() {
    let units = |text: &str| text.encode_utf16().collect::<Vec<_>>();
    let label = recorder_class_label(&units(NO_RECORDER)).unwrap();
    assert!(label.starts_with('C') || label.starts_with('N'));
    assert_eq!(label.len(), 4);
    assert!(recorder_class_label(&units("/Script/Icarus.Unregistered")).is_none());
}

#[test]
fn typed_check_types_every_body_and_counts_gaps_by_class() {
    let source = envelope(&payload(&[
        recorder(NO_RECORDER, &component(false)),
        recorder(NO_RECORDER, &component(true)),
    ]));
    let report = typed_check(source.as_slice()).unwrap();
    assert_eq!(report.validation, "experimental_typed_values_only");
    assert_eq!(report.recorder_count, 2);
    assert_eq!(report.typed_recorders, 2);
    assert_eq!(report.unframed_recorders, 0);
    assert_eq!(report.unregistered_classes, 0);
    assert!(report.round_trip_exact);
    assert!(!report.world_imported);
    // 2 x (string, transform, 3 transform parts) + 1 mystery struct.
    assert_eq!(report.totals.properties, 11);
    assert_eq!(report.totals.fixed_structs, 6);
    assert_eq!(report.totals.uninterpreted, 1);
    let label = recorder_class_label(&NO_RECORDER.encode_utf16().collect::<Vec<_>>()).unwrap();
    let class = &report.classes[&label];
    assert_eq!((class.recorders, class.uninterpreted), (2, 1));
    let serialized = serde_json::to_string(&report).unwrap();
    assert!(!serialized.contains("secret") && !serialized.contains("Secret"));
    assert!(!serialized.contains("NoRecorder"));
}

#[test]
fn typed_check_rejects_unregistered_classes_through_staging() {
    let source = envelope(&payload(&[recorder(
        "/Script/Icarus.Unregistered",
        &component(false),
    )]));
    let error = typed_check(source.as_slice()).unwrap_err();
    assert!(format!("{error:#}").contains("unrecognized recorder ComponentClassName"));
}

#[test]
fn typed_check_cli_reports_counts_only_and_writes_nothing() {
    let root = TempDir::new().unwrap();
    let path = root.path().join("authored.json");
    let source = envelope(&payload(&[recorder(NO_RECORDER, &component(true))]));
    fs::write(&path, &source).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_icarus-server"))
        .current_dir(root.path())
        .args(["inspect-save", "--typed-check", "--source"])
        .arg(&path)
        .output()
        .unwrap();
    assert!(output.status.success(), "{:?}", output.stderr);
    assert!(output.stderr.is_empty());
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["validation"], "experimental_typed_values_only");
    assert_eq!(report["world_imported"], false);
    assert_eq!(report["totals"]["gaps"]["unknown_struct"], 1);
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(!text.contains("secret") && !text.contains("Secret"));
    assert_eq!(fs::read(&path).unwrap(), source);
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);

    let conflicting = Command::new(env!("CARGO_BIN_EXE_icarus-server"))
        .args(["inspect-save", "--typed-check", "--stage-check", "--source"])
        .arg(&path)
        .output()
        .unwrap();
    assert!(!conflicting.status.success());
}

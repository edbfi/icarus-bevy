use std::{
    fs,
    io::{Read, Write},
    process::Command,
};

use base64::{Engine, engine::general_purpose::STANDARD};
use flate2::{Compression, write::ZlibEncoder};
use icarus_bevy::prospect::{
    MAX_ENVELOPE_BYTES, MAX_PAYLOAD_BYTES, inspect, inspect_with_inner_tags,
    inspect_with_recorders, stage_prospect,
};
use serde_json::{Value, json};
use sha1::{Digest, Sha1};
use tempfile::TempDir;

const FIXTURE: &[u8] = include_bytes!("fixtures/prospect-envelope.json");

fn changed(field: &str, value: Value) -> Vec<u8> {
    let mut document: Value = serde_json::from_slice(FIXTURE).unwrap();
    document["ProspectBlob"][field] = value;
    serde_json::to_vec(&document).unwrap()
}

fn utf16le(text: &str) -> Vec<u8> {
    [0xff, 0xfe]
        .into_iter()
        .chain(text.encode_utf16().flat_map(u16::to_le_bytes))
        .collect()
}

fn authored_envelope(payload: &[u8]) -> Value {
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

#[test]
fn independently_authored_zlib_fixture_passes_only_envelope_checks() {
    // Produced with Python zlib.compress and hashlib.sha1, not this implementation.
    let result = inspect(FIXTURE).unwrap();
    assert_eq!(result.payload_bytes, 42);
    assert_eq!(result.validation, "experimental_envelope_only");
    assert!(result.legacy_sha1_matches);
    assert!(!result.world_imported);
    assert_eq!(
        serde_json::to_value(&result).unwrap(),
        json!({
            "validation": "experimental_envelope_only", "compressed_bytes": 50,
            "payload_bytes": 42, "declared_data_length": 0,
            "legacy_sha1_matches": true, "world_imported": false
        })
    );
}

#[test]
fn opt_in_tag_scan_requires_full_envelope_integrity_first() {
    let mut document = authored_envelope(b"not a tagged archive");
    document["ProspectBlob"]["Hash"] = json!("0".repeat(40));
    let bytes = serde_json::to_vec(&document).unwrap();
    let error = inspect_with_inner_tags(bytes.as_slice()).unwrap_err();
    assert!(error.to_string().contains("legacy SHA-1 mismatch"));
    let error = inspect_with_recorders(bytes.as_slice()).unwrap_err();
    assert!(error.to_string().contains("legacy SHA-1 mismatch"));

    let bytes = serde_json::to_vec(&authored_envelope(b"not a tagged archive")).unwrap();
    assert!(inspect(bytes.as_slice()).is_ok());
    assert!(
        inspect_with_inner_tags(bytes.as_slice())
            .unwrap_err()
            .to_string()
            .contains("invalid experimental root tags")
    );
}

fn recorder_fixture(byte_count: i32) -> Vec<u8> {
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
    let fields = [
        tag(
            "ComponentClassName",
            "StrProperty",
            &[],
            &string("secret-class"),
        ),
        tag(
            "BinaryData",
            "ArrayProperty",
            &string("ByteProperty"),
            &[byte_count.to_le_bytes().as_slice(), b"secret-bytes"].concat(),
        ),
        tag("secret-unknown-field", "IntProperty", &[], b"opaque"),
        string("None"),
    ]
    .concat();
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
            &[1_i32.to_le_bytes().as_slice(), &shared].concat(),
        ),
        string("None"),
    ]
    .concat()
}

#[test]
fn recorder_cli_keeps_values_private_and_rejects_inner_failure_without_writes() {
    let root = TempDir::new().unwrap();
    let source = root.path().join("authored.json");
    for (byte_count, valid) in [(12, true), (11, false)] {
        let payload = recorder_fixture(byte_count);
        let envelope = serde_json::to_vec(&authored_envelope(&payload)).unwrap();
        // Both cases have intact envelopes and root tags. Only nested byte-array
        // framing distinguishes the malformed case.
        assert!(inspect_with_inner_tags(envelope.as_slice()).is_ok());
        fs::write(&source, &envelope).unwrap();
        let result = Command::new(env!("CARGO_BIN_EXE_icarus-server"))
            .current_dir(root.path())
            .args(["inspect-save", "--recorders", "--source"])
            .arg(&source)
            .output()
            .unwrap();
        assert_eq!(result.status.success(), valid, "{:?}", result.stderr);
        assert!(!String::from_utf8_lossy(&result.stdout).contains("secret"));
        assert!(!String::from_utf8_lossy(&result.stderr).contains("secret"));
        if valid {
            assert!(result.stderr.is_empty());
            let report: Value = serde_json::from_slice(&result.stdout).unwrap();
            assert_eq!(report["validation"], "experimental_recorder_framing_only");
            assert_eq!(report["world_imported"], false);
            assert_eq!(report["envelope"]["world_imported"], false);
            assert_eq!(report["inner_tags"]["root_property_count"], 1);
            assert!(report["recorders"].is_object());
        } else {
            assert!(result.stdout.is_empty());
            assert!(
                String::from_utf8_lossy(&result.stderr)
                    .contains("invalid experimental recorder framing")
            );
        }
        assert_eq!(fs::read(&source).unwrap(), envelope);
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
    }
}

#[test]
fn corrupt_hash_base64_and_lengths_fail() {
    for (field, value) in [
        ("Hash", json!("0".repeat(40))),
        ("Hash", json!("a".repeat(39))),
        ("BinaryBlob", json!("!invalid!")),
        ("TotalLength", json!(-1)),
        ("TotalLength", json!(0)),
        ("DataLength", json!(-1)),
        ("UncompressedLength", json!(-1)),
        ("UncompressedLength", json!(41)),
        ("UncompressedLength", json!(43)),
        ("UncompressedLength", json!(MAX_PAYLOAD_BYTES + 1)),
        ("UncompressedLength", json!(2147483648_u64)),
    ] {
        assert!(
            inspect(changed(field, value).as_slice()).is_err(),
            "{field}"
        );
    }
}

#[test]
fn truncated_and_trailing_compressed_data_fail_even_when_lengths_match() {
    let original: Value = serde_json::from_slice(FIXTURE).unwrap();
    let compressed = STANDARD
        .decode(original["ProspectBlob"]["BinaryBlob"].as_str().unwrap())
        .unwrap();
    for variant in [
        compressed[..compressed.len() - 1].to_vec(),
        [compressed.as_slice(), b"trailing"].concat(),
    ] {
        let mut document = original.clone();
        document["ProspectBlob"]["BinaryBlob"] = json!(STANDARD.encode(&variant));
        document["ProspectBlob"]["TotalLength"] = json!(variant.len());
        assert!(inspect(serde_json::to_vec(&document).unwrap().as_slice()).is_err());
    }
}

#[test]
fn oversized_stream_stops_after_limit_plus_one() {
    struct Counted {
        consumed: usize,
    }
    impl Read for Counted {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            buffer.fill(b' ');
            self.consumed += buffer.len();
            Ok(buffer.len())
        }
    }
    let mut reader = Counted { consumed: 0 };
    assert!(
        inspect(&mut reader)
            .unwrap_err()
            .to_string()
            .contains("4 MiB")
    );
    assert_eq!(reader.consumed, MAX_ENVELOPE_BYTES + 1);
}

#[test]
fn bom_tagged_utf16le_handles_surrogate_pairs_without_changing_payload_validation() {
    let mut document: Value = serde_json::from_slice(FIXTURE).unwrap();
    document["AuthoredUnicode"] = json!("snowman \u{2603} and rocket \u{1f680}");
    let encoded = utf16le(&serde_json::to_string(&document).unwrap());
    let result = inspect(encoded.as_slice()).unwrap();
    assert_eq!(result.payload_bytes, 42);
    assert!(result.legacy_sha1_matches);
    assert!(!result.world_imported);

    document["ProspectBlob"]["Hash"] = json!("0".repeat(40));
    let bad_hash = utf16le(&serde_json::to_string(&document).unwrap());
    assert!(
        inspect(bad_hash.as_slice())
            .unwrap_err()
            .to_string()
            .contains("legacy SHA-1 mismatch")
    );
}

#[test]
fn utf8_rejects_invalid_bytes_in_ignored_fields() {
    for invalid in [vec![0xff], vec![0xc0, 0xaf], vec![0xed, 0xa0, 0x80]] {
        for (prefix, suffix) in [
            (b"{\"Ignored\":\"".as_slice(), b"\",".as_slice()),
            (b"{\"".as_slice(), b"\":null,".as_slice()),
            (
                b"{\"Ignored\":{\"nested\":[\"".as_slice(),
                b"\"]},".as_slice(),
            ),
            (b"{\"Ignored\":{\"".as_slice(), b"\":null},".as_slice()),
        ] {
            let bytes = [prefix, invalid.as_slice(), suffix, &FIXTURE[1..]].concat();
            assert!(
                inspect(bytes.as_slice()).is_err(),
                "invalid bytes {invalid:x?} after authored prefix {prefix:?}"
            );
        }
    }
}

#[test]
fn utf8_accepts_non_ascii_unknown_keys_and_nested_values() {
    let mut document: Value = serde_json::from_slice(FIXTURE).unwrap();
    document["\u{2603}"] = json!({"\u{1f680}": ["authored caf\u{00e9}"]});
    let bytes = serde_json::to_vec(&document).unwrap();
    let result = inspect(bytes.as_slice()).unwrap();
    assert_eq!(result.payload_bytes, 42);
    assert!(result.legacy_sha1_matches);
    assert!(!result.world_imported);
}

#[test]
fn utf16le_requires_bom_even_byte_count_and_paired_surrogates() {
    let text = std::str::from_utf8(FIXTURE).unwrap();
    let encoded = utf16le(text);
    assert!(inspect(&encoded[2..]).is_err());
    assert!(
        inspect(&encoded[..encoded.len() - 1])
            .unwrap_err()
            .to_string()
            .contains("odd UTF-16LE")
    );
    for invalid in [
        vec![0x00, 0xd8],
        vec![0x00, 0xdc],
        vec![0x00, 0xd8, 0x41, 0x00],
    ] {
        let malformed = [encoded.as_slice(), &invalid].concat();
        assert!(
            inspect(malformed.as_slice())
                .unwrap_err()
                .to_string()
                .contains("invalid UTF-16LE")
        );
    }
    let big_endian: Vec<u8> = [0xfe, 0xff]
        .into_iter()
        .chain(text.encode_utf16().flat_map(u16::to_be_bytes))
        .collect();
    assert!(inspect(big_endian.as_slice()).is_err());
}

#[test]
fn raw_limit_accepts_boundary_in_both_encodings_and_rejects_one_extra_byte() {
    let text = std::str::from_utf8(FIXTURE).unwrap();
    for mut bytes in [FIXTURE.to_vec(), utf16le(text)] {
        if bytes.starts_with(&[0xff, 0xfe]) {
            while bytes.len() < MAX_ENVELOPE_BYTES {
                bytes.extend_from_slice(&[b' ', 0]);
            }
        } else {
            bytes.resize(MAX_ENVELOPE_BYTES, b' ');
        }
        assert_eq!(bytes.len(), MAX_ENVELOPE_BYTES);
        assert!(bytes.len() > 1024 * 1024);
        assert_eq!(inspect(bytes.as_slice()).unwrap().payload_bytes, 42);
        bytes.push(b' ');
        assert!(
            inspect(bytes.as_slice())
                .unwrap_err()
                .to_string()
                .contains("envelope exceeds 4 MiB")
        );
    }
}

#[test]
fn authored_payload_above_previous_limit_remains_bounded_and_hash_verified() {
    let payload = vec![b'A'; 16 * 1024 * 1024 + 1];
    let bytes = serde_json::to_vec(&authored_envelope(&payload)).unwrap();
    assert!(bytes.len() < 256 * 1024);
    let result = inspect(bytes.as_slice()).unwrap();
    assert_eq!(result.payload_bytes, payload.len());
    assert!(result.payload_bytes < MAX_PAYLOAD_BYTES);
    assert!(result.legacy_sha1_matches);
    assert!(!result.world_imported);
}

#[test]
fn declared_output_limit_and_actual_expansion_are_checked_independently() {
    let mut document = authored_envelope(&[b'B'; 64 * 1024]);
    document["ProspectBlob"]["UncompressedLength"] = json!(MAX_PAYLOAD_BYTES + 1);
    assert!(
        inspect(serde_json::to_vec(&document).unwrap().as_slice())
            .unwrap_err()
            .to_string()
            .contains("payload exceeds 32 MiB")
    );
    document["ProspectBlob"]["UncompressedLength"] = json!(128);
    assert!(
        inspect(serde_json::to_vec(&document).unwrap().as_slice())
            .unwrap_err()
            .to_string()
            .contains("output exceeds declared length")
    );
}

#[test]
fn duplicate_required_fields_and_truncated_json_fail() {
    let text = String::from_utf8(FIXTURE.to_vec()).unwrap();
    let duplicate = text.replace("\"TotalLength\":", "\"TotalLength\": 0, \"TotalLength\":");
    assert!(inspect(duplicate.as_bytes()).is_err());
    assert!(inspect(&FIXTURE[..FIXTURE.len() / 2]).is_err());
    assert!(inspect(b"{}".as_slice()).is_err());
}

#[test]
fn required_envelope_objects_reject_positional_arrays() {
    let document: Value = serde_json::from_slice(FIXTURE).unwrap();
    let blob = &document["ProspectBlob"];
    let positional = json!([
        blob["Hash"],
        blob["TotalLength"],
        blob["DataLength"],
        blob["UncompressedLength"],
        blob["BinaryBlob"]
    ]);
    for malformed in [
        json!([positional]),
        json!([blob]),
        json!({"ProspectBlob": positional}),
    ] {
        let text = serde_json::to_string(&malformed).unwrap();
        for bytes in [text.as_bytes().to_vec(), utf16le(&text)] {
            for error in [
                inspect(bytes.as_slice()).unwrap_err(),
                inspect_with_inner_tags(bytes.as_slice()).unwrap_err(),
                inspect_with_recorders(bytes.as_slice()).unwrap_err(),
            ] {
                assert!(
                    error
                        .to_string()
                        .starts_with("invalid prospect envelope JSON")
                );
            }
        }
    }
}

#[test]
fn malformed_envelope_diagnostics_keep_values_private_in_every_cli_mode() {
    const MARKER: &str = "authored-private-marker-keep-out-of-logs";
    let mut document: Value = serde_json::from_slice(FIXTURE).unwrap();
    document["ProspectBlob"]["UncompressedLength"] = json!(MARKER);
    let text = serde_json::to_string(&document).unwrap();
    let root = TempDir::new().unwrap();
    for bytes in [text.as_bytes().to_vec(), utf16le(&text)] {
        let error = inspect(bytes.as_slice()).unwrap_err();
        assert!(!format!("{error:#}").contains(MARKER));
        let source = root.path().join("authored.json");
        fs::write(&source, &bytes).unwrap();
        for flag in [
            None,
            Some("--inner-tags"),
            Some("--recorders"),
            Some("--stage-check"),
        ] {
            let mut command = Command::new(env!("CARGO_BIN_EXE_icarus-server"));
            command
                .current_dir(root.path())
                .args(["inspect-save", "--source"])
                .arg(&source);
            if let Some(flag) = flag {
                command.arg(flag);
            }
            let result = command.output().unwrap();
            assert!(!result.status.success());
            assert!(result.stdout.is_empty());
            let report: Value = serde_json::from_slice(&result.stderr).unwrap();
            let diagnostic = report["error"].as_str().unwrap();
            assert!(diagnostic.contains("invalid prospect envelope JSON: Data at line"));
            assert!(diagnostic.contains("column"));
            assert!(!diagnostic.contains(MARKER));
            assert_eq!(fs::read(&source).unwrap(), bytes);
            assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
        }
    }
}

#[test]
fn inspector_cli_preserves_source_and_creates_no_state() {
    let root = TempDir::new().unwrap();
    let source = root.path().join("authored.json");
    fs::write(&source, FIXTURE).unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_icarus-server"))
        .current_dir(root.path())
        .args(["inspect-save", "--source"])
        .arg(&source)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let report: Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(report["world_imported"], false);
    assert_eq!(fs::read(&source).unwrap(), FIXTURE);
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
    assert!(
        !String::from_utf8(result.stdout)
            .unwrap()
            .contains("authored-fixture")
    );
}

#[test]
fn opt_in_tag_cli_reports_only_counts_and_never_writes_or_exports_values() {
    // Manually packed StrProperty: authored secret name/value, then None.
    // The value deliberately is not an encoded FString: values are not decoded.
    let payload = b"\x0d\0\0\0secret-field\0\x0c\0\0\0StrProperty\0\x06\0\0\0\0\0\0\0\0secret\x05\0\0\0None\0";
    let root = TempDir::new().unwrap();
    let source = root.path().join("authored.json");
    let envelope = serde_json::to_vec(&authored_envelope(payload)).unwrap();
    fs::write(&source, &envelope).unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_icarus-server"))
        .current_dir(root.path())
        .args(["inspect-save", "--inner-tags", "--source"])
        .arg(&source)
        .output()
        .unwrap();
    assert!(result.status.success(), "{:?}", result.stderr);
    assert!(result.stderr.is_empty());
    let report: Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(report["validation"], "experimental_root_tags_only");
    assert_eq!(report["world_imported"], false);
    assert_eq!(report["envelope"]["world_imported"], false);
    assert_eq!(
        report["inner_tags"],
        json!({
            "profile": "experimental_memory_archive_522", "root_property_count": 1,
            "opaque_payload_bytes": 6, "header_bytes": 51
        })
    );
    assert!(!String::from_utf8(result.stdout).unwrap().contains("secret"));
    assert_eq!(fs::read(&source).unwrap(), envelope);
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);

    // A failed framing scan must not emit a successful partial JSON report.
    let invalid = serde_json::to_vec(&authored_envelope(&payload[..payload.len() - 1])).unwrap();
    fs::write(&source, &invalid).unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_icarus-server"))
        .current_dir(root.path())
        .args(["inspect-save", "--inner-tags", "--source"])
        .arg(&source)
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert!(result.stdout.is_empty());
    assert!(!String::from_utf8(result.stderr).unwrap().contains("secret"));
    assert_eq!(fs::read(&source).unwrap(), invalid);
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
}

#[test]
fn stage_prospect_validates_envelope_migration_and_normalized_name() {
    use icarus_bevy::experimental::component::MAX_COMPONENT_BYTES;
    use icarus_bevy::experimental::restore_rules::LobbyPrivacy;

    let fstring = |s: &str| -> Vec<u8> {
        [
            i32::try_from(s.len() + 1).unwrap().to_le_bytes().as_slice(),
            s.as_bytes(),
            &[0],
        ]
        .concat()
    };
    let tag = |name: &str, kind: &str, extras: &[u8], payload: &[u8]| -> Vec<u8> {
        [
            fstring(name).as_slice(),
            fstring(kind).as_slice(),
            &i32::try_from(payload.len()).unwrap().to_le_bytes(),
            &0_i32.to_le_bytes(),
            extras,
            payload,
        ]
        .concat()
    };
    let end = b"\x05\0\0\0None\0";
    let inner_info = [
        tag(
            "ProspectID",
            "StrProperty",
            &[0],
            &fstring("secret-prospect-name"),
        ),
        tag(
            "ProspectDTKey",
            "StrProperty",
            &[0],
            &fstring("Olympus_Terrain"),
        ),
        tag("ExpireTime", "Int64Property", &[0], &(-1_i64).to_le_bytes()),
        end.to_vec(),
    ]
    .concat();
    let payload = [
        tag("Version", "IntProperty", &[0], &2_i32.to_le_bytes()),
        tag(
            "LobbyPrivacy",
            "ByteProperty",
            &[fstring("None"), vec![0]].concat(),
            &[1],
        ),
        tag(
            "ProspectInfo",
            "StructProperty",
            &[fstring("IcarusProspect"), vec![0; 17]].concat(),
            &inner_info,
        ),
        end.to_vec(),
    ]
    .concat();
    let envelope = serde_json::to_vec(&authored_envelope(&payload)).unwrap();
    let staged = stage_prospect(
        envelope.as_slice(),
        MAX_COMPONENT_BYTES,
        Some("SECRET-PROSPECT-NAME.JSON"),
    )
    .unwrap();
    assert_eq!(
        staged.validation,
        "experimental_staged_prospect_save_state_only"
    );
    assert_eq!(staged.staged.loaded_version, 2);
    assert_eq!(staged.staged.migrated_version, 2);
    assert_eq!(staged.staged.lobby_privacy, LobbyPrivacy::FriendsOnly);
    assert!(staged.staged.has_prospect_id);
    assert!(staged.staged.has_prospect_dt_key);
    assert!(staged.staged.has_expire_time);
    assert!(!staged.world_imported);
    let serialized = serde_json::to_string(&staged).unwrap();
    assert!(!serialized.contains("secret"));

    assert!(
        stage_prospect(
            envelope.as_slice(),
            MAX_COMPONENT_BYTES,
            Some("wrong-name.json"),
        )
        .is_err()
    );
    assert!(stage_prospect(envelope.as_slice(), MAX_COMPONENT_BYTES, Some(".json")).is_err());

    let root = TempDir::new().unwrap();
    let source = root.path().join("authored.json");
    fs::write(&source, &envelope).unwrap();
    let cli_ok = Command::new(env!("CARGO_BIN_EXE_icarus-server"))
        .current_dir(root.path())
        .args([
            "inspect-save",
            "--stage-check",
            "--expect-prospect-name",
            "SECRET-PROSPECT-NAME.JSON",
            "--source",
        ])
        .arg(&source)
        .output()
        .unwrap();
    assert!(cli_ok.status.success(), "{:?}", cli_ok.stderr);
    assert!(cli_ok.stderr.is_empty());
    let report: Value = serde_json::from_slice(&cli_ok.stdout).unwrap();
    assert_eq!(
        report["validation"],
        "experimental_staged_prospect_save_state_only"
    );
    assert_eq!(report["world_imported"], false);
    assert_eq!(report["staged"]["migrated_version"], 2);
    assert_eq!(report["staged"]["lobby_privacy"], "FriendsOnly");
    assert!(!String::from_utf8_lossy(&cli_ok.stdout).contains("secret"));
    assert_eq!(fs::read(&source).unwrap(), envelope);
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);

    let cli_mismatch = Command::new(env!("CARGO_BIN_EXE_icarus-server"))
        .current_dir(root.path())
        .args([
            "inspect-save",
            "--stage-check",
            "--expect-prospect-name",
            "wrong-name.json",
            "--source",
        ])
        .arg(&source)
        .output()
        .unwrap();
    assert!(!cli_mismatch.status.success());
    assert!(cli_mismatch.stdout.is_empty());
    assert!(!String::from_utf8_lossy(&cli_mismatch.stderr).contains("secret"));
    assert!(String::from_utf8_lossy(&cli_mismatch.stderr).contains("staged ProspectID mismatch"));
    assert_eq!(fs::read(&source).unwrap(), envelope);
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
}

use std::{
    fs,
    io::{Read, Write},
    process::Command,
};

use base64::{Engine, engine::general_purpose::STANDARD};
use flate2::{Compression, write::ZlibEncoder};
use icarus_bevy::prospect::{MAX_ENVELOPE_BYTES, MAX_PAYLOAD_BYTES, inspect};
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
    document["AuthoredUnicode"] = json!("snowman ☃ and rocket 🚀");
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
    document["☃"] = json!({"🚀": ["authored café"]});
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

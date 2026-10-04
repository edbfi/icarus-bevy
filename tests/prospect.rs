use std::{fs, io::Read, process::Command};

use base64::{Engine, engine::general_purpose::STANDARD};
use icarus_bevy::prospect::{MAX_ENVELOPE_BYTES, inspect};
use serde_json::{Value, json};
use tempfile::TempDir;

const FIXTURE: &[u8] = include_bytes!("fixtures/prospect-envelope.json");

fn changed(field: &str, value: Value) -> Vec<u8> {
    let mut document: Value = serde_json::from_slice(FIXTURE).unwrap();
    document["ProspectBlob"][field] = value;
    serde_json::to_vec(&document).unwrap()
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
        ("UncompressedLength", json!(16 * 1024 * 1024 + 1)),
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
            .contains("1 MiB")
    );
    assert_eq!(reader.consumed, MAX_ENVELOPE_BYTES + 1);
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

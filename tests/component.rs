use icarus_bevy::experimental::component::{
    BaseComponent, MAX_BOUNDED_COMPONENT_BYTES, MAX_COMPONENT_BYTES, MAX_COMPONENT_FIELDS,
    read_base_component, read_base_component_with_limit,
};
use icarus_bevy::experimental::tagged::{TagKind, TagProfile, read_root};

const END: &[u8] = b"\x05\0\0\0None\0";
const NAME: &[u8] = b"\x0d\0\0\0NameProperty\0";

fn read(bytes: &[u8]) -> anyhow::Result<BaseComponent<'_>> {
    read_base_component(bytes, TagProfile::MemoryArchive522)
}

// Independently authored header bytes; values intentionally need not be names.
fn field(kind: &[u8], extensions: &[u8], payload: &[u8]) -> Vec<u8> {
    [
        b"\x11\0\0\0AuthoredSecretZZ\0".as_slice(),
        kind,
        &i32::try_from(payload.len()).unwrap().to_le_bytes(),
        &[0; 4],
        extensions,
        payload,
    ]
    .concat()
}

fn body(fields: &[u8]) -> Vec<u8> {
    [fields, END, &[0; 4]].concat()
}

#[test]
fn source_spans_and_counts_preserve_opaque_values_and_embedded_terminators() {
    let payload = [END, b"private-value-marker", &[0; 4]].concat();
    let first = field(NAME, &[0], &payload);
    let second = field(NAME, &[&[1][..], &[7; 16]].concat(), b"not an FString");
    let bytes = body(&[first.as_slice(), second.as_slice()].concat());
    let parsed = read(&bytes).unwrap();
    assert_eq!(parsed.fields().len(), 2);
    let mut offset = 0;
    for (tag, original) in parsed.fields().iter().zip([&first, &second]) {
        assert_eq!(tag.kind, TagKind::Name);
        assert_eq!(tag.record, original);
        assert_eq!(tag.record.as_ptr(), bytes[offset..].as_ptr());
        assert_eq!(tag.header.as_ptr(), tag.record.as_ptr());
        assert_eq!(tag.header.len() + tag.payload.len(), tag.record.len());
        assert_eq!(
            tag.payload.as_ptr(),
            bytes[offset + tag.header.len()..].as_ptr()
        );
        offset += original.len();
    }
    assert_eq!(parsed.fields()[0].payload, payload);
    assert_eq!(parsed.fields()[1].payload, b"not an FString");
    assert_eq!(parsed.terminator(), END);
    assert_eq!(parsed.terminator().as_ptr(), bytes[offset..].as_ptr());
    assert_eq!(parsed.trailer(), [0; 4]);
    assert_eq!(
        parsed.trailer().as_ptr(),
        bytes[offset + END.len()..].as_ptr()
    );
    let summary = parsed.summary();
    assert_eq!(summary.field_count, 2);
    assert_eq!(
        summary.opaque_payload_bytes,
        payload.len() + b"not an FString".len()
    );
    assert_eq!(summary.trailer_bytes, 4);
    assert_eq!(
        summary.header_bytes + summary.opaque_payload_bytes + summary.trailer_bytes,
        bytes.len()
    );
    assert!(!summary.world_imported);
    assert!(!summary.values_interpreted);
    let json = serde_json::to_string(&summary).unwrap();
    let debug = format!("{summary:?}");
    for text in [json, debug] {
        assert!(!text.contains("AuthoredSecret"));
        assert!(!text.contains("private-value-marker"));
        assert!(!text.contains("not an FString"));
    }
}

#[test]
fn empty_candidate_is_valid_but_root_eof_policy_is_unchanged() {
    let bytes = body(&[]);
    let parsed = read(&bytes).unwrap();
    assert!(parsed.fields().is_empty());
    assert_eq!(parsed.summary().header_bytes, END.len());
    assert!(read_root(&bytes, TagProfile::MemoryArchive522).is_err());
    assert!(read_root(END, TagProfile::MemoryArchive522).is_ok());
    assert!(read(END).is_err());
}

#[test]
fn every_truncation_rejects_without_partial_success() {
    for extensions in [vec![0], [&[1][..], &[3; 16]].concat()] {
        let bytes = body(&field(NAME, &extensions, b"opaque"));
        for length in 0..bytes.len() {
            assert!(read(&bytes[..length]).is_err(), "truncation {length}");
        }
        assert!(read(&bytes).is_ok());
    }
}

#[test]
fn true_invalid_and_trailing_object_trailers_are_rejected() {
    for following in [&[][..], &[7; 16][..], &[7; 17][..]] {
        let bytes = [END, &1_u32.to_le_bytes(), following].concat();
        let error = read(&bytes).err().unwrap().to_string();
        assert_eq!(error, "unsupported component GUID trailer");
    }
    for flag in [2_u32, 256, 0x0100_0000, u32::MAX] {
        let bytes = [END, &flag.to_le_bytes()].concat();
        assert!(
            read(&bytes)
                .err()
                .unwrap()
                .to_string()
                .contains("noncanonical")
        );
    }
    for suffix in [&[0][..], END, &[0; 16][..]] {
        assert!(read(&[body(&[]).as_slice(), suffix].concat()).is_err());
    }
}

#[test]
fn exact_field_and_byte_caps_are_enforced() {
    let record = field(NAME, &[0], &[]);
    let bytes = body(&record.repeat(MAX_COMPONENT_FIELDS));
    assert_eq!(read(&bytes).unwrap().fields().len(), MAX_COMPONENT_FIELDS);
    assert!(read(&body(&record.repeat(MAX_COMPONENT_FIELDS + 1))).is_err());

    let payload_len = MAX_COMPONENT_BYTES - record.len() - END.len() - 4;
    let bytes = body(&field(NAME, &[0], &vec![0; payload_len]));
    assert_eq!(bytes.len(), MAX_COMPONENT_BYTES);
    assert_eq!(
        read(&bytes).unwrap().summary().opaque_payload_bytes,
        payload_len
    );
    let excess = body(&field(NAME, &[0], &vec![0; payload_len + 1]));
    assert_eq!(excess.len(), MAX_COMPONENT_BYTES + 1);
    assert!(read(&excess).err().unwrap().to_string().contains("64 KiB"));
}

#[test]
fn explicit_byte_limit_preserves_spans_and_default_policy() {
    let overhead = field(NAME, &[0], &[]).len() + END.len() + 4;
    for size in [MAX_COMPONENT_BYTES + 1, MAX_BOUNDED_COMPONENT_BYTES] {
        let bytes = body(&field(NAME, &[0], &vec![7; size - overhead]));
        assert_eq!(bytes.len(), size);
        assert!(read(&bytes).is_err());
        let parsed =
            read_base_component_with_limit(&bytes, TagProfile::MemoryArchive522, size).unwrap();
        assert_eq!(parsed.fields()[0].record.as_ptr(), bytes.as_ptr());
        assert_eq!(parsed.fields()[0].payload, vec![7; size - overhead]);
        assert_eq!(parsed.summary().opaque_payload_bytes, size - overhead);
        assert!(
            read_base_component_with_limit(&bytes, TagProfile::MemoryArchive522, size - 1).is_err()
        );
    }
    for invalid in [0, MAX_BOUNDED_COMPONENT_BYTES + 1, usize::MAX] {
        assert!(
            read_base_component_with_limit(&body(&[]), TagProfile::MemoryArchive522, invalid)
                .is_err()
        );
    }
}

#[test]
fn larger_byte_limit_retains_field_and_trailer_checks() {
    let read_bounded = |bytes: &[u8]| {
        read_base_component_with_limit(
            bytes,
            TagProfile::MemoryArchive522,
            MAX_BOUNDED_COMPONENT_BYTES,
        )
        .map(|parsed| parsed.summary())
    };
    let record = field(NAME, &[0], &[]);
    assert!(read_bounded(&body(&record.repeat(MAX_COMPONENT_FIELDS))).is_ok());
    assert!(read_bounded(&body(&record.repeat(MAX_COMPONENT_FIELDS + 1))).is_err());
    for trailer in [vec![0; 3], vec![1, 0, 0, 0], vec![2, 0, 0, 0], vec![0; 5]] {
        assert!(read_bounded(&[END, &trailer].concat()).is_err());
    }
    assert!(read_bounded(&[END, &[1, 0, 0, 0], &[0; 16]].concat()).is_err());
}

#[test]
fn malformed_later_fields_fail_with_sanitized_errors() {
    let good = field(NAME, &[0], b"private-value-marker");
    let bad = field(
        b"\x11\0\0\0AuthoredSecretZZ\0",
        &[0],
        b"private-value-marker",
    );
    let bytes = body(&[good.as_slice(), bad.as_slice()].concat());
    let message = format!("{:#}", read(&bytes).err().unwrap());
    assert!(message.contains("unsupported tag header kind"));
    assert!(!message.contains("AuthoredSecret"));
    assert!(!message.contains("private-value-marker"));
    assert!(read(&body(&field(NAME, &[2], &[]))).is_err());
    // No recognized schema or required field names: duplicate opaque names pass.
    assert_eq!(read(&body(&good.repeat(2))).unwrap().fields().len(), 2);
}

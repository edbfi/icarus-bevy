use icarus_bevy::experimental::tagged::{
    MAX_HEADER_STRING_UNITS, MAX_INPUT_BYTES, MAX_ROOT_TAGS, RootTags, TagKind, TagProfile,
    read_root,
};

const END: &[u8] = b"\x05\0\0\0None\0";
// Manually packed name A, IntProperty, size 4, index 0, absent GUID, value 42.
const INT: &[u8] = b"\x02\0\0\0A\0\x0c\0\0\0IntProperty\0\x04\0\0\0\0\0\0\0\0\x2a\0\0\0";

fn read(bytes: &[u8]) -> anyhow::Result<RootTags<'_>> {
    read_root(bytes, TagProfile::MemoryArchive522)
}

// Fixture assembly uses explicit serialized type names, not the codec encoder.
fn record(kind: &[u8], extensions: &[u8], payload: &[u8]) -> Vec<u8> {
    [
        b"\x02\0\0\0P\0".as_slice(),
        kind,
        &i32::try_from(payload.len()).unwrap().to_le_bytes(),
        &[0; 4],
        extensions,
        payload,
    ]
    .concat()
}

fn fixtures() -> Vec<(TagKind, Vec<u8>, Vec<u8>)> {
    let opaque = b"\x05\0\0\0None\0not another root tag".to_vec();
    let struct_extensions = [
        b"\x05\0\0\0Demo\0".as_slice(),
        &[7; 16], // Struct GUID.
        &[1],
        &[9; 16], // Optional property GUID.
    ]
    .concat();
    [
        (TagKind::Int, INT.to_vec(), vec![42, 0, 0, 0]),
        (
            TagKind::String,
            record(b"\x0c\0\0\0StrProperty\0", &[0], &opaque),
            opaque.clone(),
        ),
        (
            TagKind::Array,
            record(
                b"\x0e\0\0\0ArrayProperty\0",
                b"\x0c\0\0\0IntProperty\0\0",
                &opaque,
            ),
            opaque.clone(),
        ),
        (
            TagKind::Struct,
            record(b"\x0f\0\0\0StructProperty\0", &struct_extensions, &opaque),
            opaque.clone(),
        ),
        (
            TagKind::Byte,
            record(b"\x0d\0\0\0ByteProperty\0", b"\x05\0\0\0None\0\0", &opaque),
            opaque.clone(),
        ),
        (
            TagKind::Enum,
            record(b"\x0d\0\0\0EnumProperty\0", b"\x05\0\0\0Demo\0\0", &opaque),
            opaque,
        ),
        (
            TagKind::Bool,
            record(b"\x0d\0\0\0BoolProperty\0", &[1, 0], &[]),
            Vec::new(),
        ),
    ]
    .into()
}

#[test]
fn manual_headers_preserve_opaque_source_spans_without_parsing_values() {
    let fixtures = fixtures();
    let mut bytes = Vec::new();
    for (_, record, _) in &fixtures {
        bytes.extend_from_slice(record);
    }
    bytes.extend_from_slice(END);
    let parsed = read(&bytes).unwrap();
    assert_eq!(parsed.tags.len(), fixtures.len());
    let mut offset = 0;
    for (tag, (kind, original, payload)) in parsed.tags.iter().zip(&fixtures) {
        assert_eq!(tag.kind, *kind);
        assert_eq!(tag.record, original);
        assert_eq!(tag.record.as_ptr(), bytes[offset..].as_ptr());
        assert_eq!(tag.header.len() + tag.payload.len(), tag.record.len());
        assert_eq!(tag.payload, payload);
        assert_eq!(
            tag.payload.as_ptr(),
            bytes[offset + tag.header.len()..].as_ptr()
        );
        offset += original.len();
    }
    assert_eq!(parsed.terminator, END);
    let summary = parsed.summary();
    assert_eq!(summary.profile, "experimental_memory_archive_522");
    assert_eq!(summary.root_property_count, 7);
    assert_eq!(
        summary.header_bytes + summary.opaque_payload_bytes,
        bytes.len()
    );
}

#[test]
fn every_truncation_and_trailing_byte_fails_without_partial_success() {
    for (_, record, _) in fixtures() {
        let stream = [record.as_slice(), END].concat();
        for length in 0..stream.len() {
            assert!(read(&stream[..length]).is_err(), "truncation {length}");
        }
        assert!(read(&[stream.as_slice(), &[0]].concat()).is_err());
    }
    assert_eq!(read(END).unwrap().summary().root_property_count, 0);
    assert!(read(b"\0\0\0\0").is_err());
    assert!(read(b"\x01\0\0\0\0").is_err());
    assert!(read(b"\x05\0\0\0none\0").is_err());
}

#[test]
fn invalid_sizes_indices_and_noncanonical_flags_fail() {
    for (offset, value) in [(22, -1_i32), (22, i32::MAX), (26, -1)] {
        let mut stream = [INT, END].concat();
        stream[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        assert!(read(&stream).is_err());
    }
    let mut invalid_guid = [INT, END].concat();
    invalid_guid[30] = 2;
    assert!(read(&invalid_guid).is_err());
    let invalid_bool = record(b"\x0d\0\0\0BoolProperty\0", &[2, 0], &[]);
    assert!(read(&[invalid_bool.as_slice(), END].concat()).is_err());
    let zero = record(b"\x0c\0\0\0IntProperty\0", &[0], &[]);
    // No IntProperty value-width validation is implied by root-only framing.
    assert!(read(&[zero.as_slice(), END].concat()).is_ok());
}

#[test]
fn unsupported_type_headers_fail_without_exposing_names() {
    for kind in [
        b"\x0c\0\0\0MapProperty\0".as_slice(),
        b"\x0c\0\0\0SetProperty\0".as_slice(),
        b"\x11\0\0\0AuthoredSecretZZ\0".as_slice(),
    ] {
        let stream = [record(kind, &[0], b"private-marker").as_slice(), END].concat();
        let error = read(&stream).err().unwrap();
        let message = format!("{error:#}");
        assert!(!message.contains("AuthoredSecret"));
        assert!(!message.contains("private-marker"));
    }
}

#[test]
fn root_tag_string_and_input_caps_are_checked() {
    let mut stream = INT.repeat(MAX_ROOT_TAGS);
    stream.extend_from_slice(END);
    assert_eq!(read(&stream).unwrap().tags.len(), MAX_ROOT_TAGS);
    let excess = [INT, stream.as_slice()].concat();
    assert!(read(&excess).is_err());

    let mut maximum_name = i32::try_from(MAX_HEADER_STRING_UNITS)
        .unwrap()
        .to_le_bytes()
        .to_vec();
    maximum_name.extend(std::iter::repeat_n(b'A', MAX_HEADER_STRING_UNITS - 1));
    maximum_name.push(0);
    maximum_name.extend_from_slice(&INT[6..]);
    maximum_name.extend_from_slice(END);
    assert!(read(&maximum_name).is_ok());
    for count in [i32::MIN, i32::MAX, 1025, -1025] {
        let error = read(&count.to_le_bytes()).err().unwrap();
        assert!(error.to_string().contains("unit limit"));
    }
    assert!(read(&vec![0; MAX_INPUT_BYTES + 1]).is_err());
}

#[test]
fn wide_unknown_names_are_preserved_as_raw_header_bytes() {
    // Name is an unpaired UTF-16 surrogate: the reused codec preserves raw units.
    let wide_name = [254, 255, 255, 255, 0, 216, 0, 0];
    let stream = [&wide_name, &INT[6..], END].concat();
    let parsed = read(&stream).unwrap();
    assert_eq!(&parsed.tags[0].header[..wide_name.len()], wide_name);
}

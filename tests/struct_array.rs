use icarus_bevy::experimental::struct_array::{
    MAX_STRUCT_ARRAY_BYTES, MAX_STRUCT_ARRAY_ELEMENTS, MAX_STRUCT_ELEMENT_FIELDS,
    TaggedStructArray, read_tagged_struct_array,
};
use icarus_bevy::experimental::tagged::{TagKind, TagProfile};

const END: &[u8] = b"\x05\0\0\0None\0";
const STRUCT: &[u8] = b"\x0f\0\0\0StructProperty\0";
const UINT32: &[u8] = b"\x0f\0\0\0UInt32Property\0";
const NAME: &[u8] = b"\x0d\0\0\0NameProperty\0";

fn read(bytes: &[u8]) -> anyhow::Result<TaggedStructArray<'_>> {
    read_tagged_struct_array(bytes, TagProfile::MemoryArchive522)
}

// Explicit authored header bytes; no production encoder or supplied data.
fn field(kind: &[u8], index: i32, extensions: &[u8], payload: &[u8]) -> Vec<u8> {
    [
        b"\x11\0\0\0AuthoredSecretZZ\0".as_slice(),
        kind,
        &i32::try_from(payload.len()).unwrap().to_le_bytes(),
        &index.to_le_bytes(),
        extensions,
        payload,
    ]
    .concat()
}

fn array(count: i32, payload: &[u8], guid: bool) -> Vec<u8> {
    let mut extensions = b"\x05\0\0\0Demo\0".to_vec();
    extensions.extend_from_slice(&[7; 16]);
    extensions.push(u8::from(guid));
    if guid {
        extensions.extend_from_slice(&[9; 16]);
    }
    // Nonzero shared index deliberately demonstrates absence of schema policy.
    [
        &count.to_le_bytes()[..],
        &field(STRUCT, 7, &extensions, payload),
    ]
    .concat()
}

fn element(fields: &[u8]) -> Vec<u8> {
    [fields, END].concat()
}

#[test]
fn empty_array_requires_shared_header_and_exact_eof() {
    let bytes = array(0, &[], false);
    let parsed = read(&bytes).unwrap();
    assert!(parsed.elements().is_empty());
    assert!(parsed.shared().payload.is_empty());
    assert_eq!(parsed.prefix(), bytes);
    let summary = parsed.summary();
    assert_eq!(summary.element_count, 0);
    assert_eq!(summary.field_count, 0);
    assert_eq!(summary.array_prefix_bytes, bytes.len());
    assert_eq!(summary.element_header_bytes, 0);
    assert_eq!(summary.opaque_payload_bytes, 0);
    for invalid in [
        vec![0; 4],
        [&[0; 4][..], END].concat(),
        array(0, END, false),
    ] {
        assert!(read(&invalid).is_err());
    }
}

#[test]
fn variable_elements_preserve_borrowed_spans_and_opaque_values() {
    let payload = [END, b"private-value-marker"].concat();
    let first = element(&field(UINT32, 2, &[0], &[255; 4]));
    let second_field = field(NAME, 0, &[&[1][..], &[3; 16]].concat(), &payload);
    let second = element(&second_field);
    let empty = element(&[]);
    let bytes = array(
        3,
        &[first.as_slice(), second.as_slice(), empty.as_slice()].concat(),
        true,
    );
    let parsed = read(&bytes).unwrap();
    assert_eq!(parsed.shared().kind, TagKind::Struct);
    assert_eq!(parsed.shared().record.as_ptr(), bytes[4..].as_ptr());
    assert_eq!(parsed.prefix().as_ptr(), bytes.as_ptr());
    let mut offset = parsed.prefix().len();
    for (actual, expected) in parsed.elements().iter().zip([&first, &second, &empty]) {
        assert_eq!(actual.record(), expected);
        assert_eq!(actual.record().as_ptr(), bytes[offset..].as_ptr());
        assert_eq!(actual.terminator(), END);
        assert_eq!(
            actual.terminator().as_ptr(),
            bytes[offset + expected.len() - END.len()..].as_ptr()
        );
        for f in actual.fields() {
            assert_eq!(f.header.as_ptr(), f.record.as_ptr());
            assert_eq!(f.header.len() + f.payload.len(), f.record.len());
            assert_eq!(f.payload.as_ptr(), f.record[f.header.len()..].as_ptr());
        }
        offset += expected.len();
    }
    assert_eq!(offset, bytes.len());
    assert_eq!(parsed.elements()[0].fields()[0].kind, TagKind::UInt32);
    assert_eq!(parsed.elements()[1].fields()[0].record, second_field);
    assert_eq!(parsed.elements()[1].fields()[0].payload, payload);
    let summary = parsed.summary();
    assert_eq!(summary.element_count, 3);
    assert_eq!(summary.field_count, 2);
    assert_eq!(summary.opaque_payload_bytes, 4 + payload.len());
    assert_eq!(
        summary.array_prefix_bytes + summary.element_header_bytes + summary.opaque_payload_bytes,
        bytes.len()
    );
    assert!(!summary.world_imported);
    assert!(!summary.values_interpreted);
    for text in [
        format!("{summary:?}"),
        serde_json::to_string(&summary).unwrap(),
    ] {
        for secret in ["AuthoredSecret", "private-value-marker", "Demo"] {
            assert!(!text.contains(secret));
        }
    }
}

#[test]
fn every_truncation_and_noncanonical_guid_flag_fails() {
    for guid in [false, true] {
        let extensions = if guid {
            [&[1][..], &[3; 16]].concat()
        } else {
            vec![0]
        };
        let bytes = array(1, &element(&field(UINT32, 0, &extensions, b"opaque")), guid);
        for length in 0..bytes.len() {
            assert!(read(&bytes[..length]).is_err(), "truncation {length}");
        }
        assert!(read(&bytes).is_ok());
    }
    let mut shared_bad = array(0, &[], false);
    *shared_bad.last_mut().unwrap() = 2;
    assert!(read(&shared_bad).is_err());
    assert!(read(&array(1, &element(&field(NAME, 0, &[2], &[])), false)).is_err());
}

#[test]
fn wrong_counts_shared_kind_size_and_extra_bytes_fail() {
    let good_element = element(&field(NAME, 0, &[0], b"opaque"));
    for count in [-1, i32::MIN, 0, 2, i32::MAX] {
        assert!(read(&array(count, &good_element, false)).is_err());
    }
    let wrong_kind = [&1_i32.to_le_bytes()[..], &field(NAME, 0, &[0], END)].concat();
    assert!(read(&wrong_kind).is_err());
    for delta in [-1_i32, 1] {
        let mut bytes = array(1, &good_element, false);
        let size_offset = 4 + b"\x11\0\0\0AuthoredSecretZZ\0".len() + STRUCT.len();
        let size = i32::try_from(good_element.len()).unwrap() + delta;
        bytes[size_offset..size_offset + 4].copy_from_slice(&size.to_le_bytes());
        assert!(read(&bytes).is_err());
    }
    for suffix in [&[0][..], &[0; 4][..], END, &[1, 0, 0, 0, 7, 7, 7, 7][..]] {
        assert!(read(&[array(1, &good_element, false).as_slice(), suffix].concat()).is_err());
        assert!(
            read(&array(
                1,
                &[good_element.as_slice(), suffix].concat(),
                false
            ))
            .is_err()
        );
    }
}

#[test]
fn element_field_count_and_input_limits_include_exact_boundaries() {
    let field = field(NAME, 0, &[0], &[]);
    let exact_fields = array(1, &element(&field.repeat(MAX_STRUCT_ELEMENT_FIELDS)), false);
    assert_eq!(
        read(&exact_fields).unwrap().elements()[0].fields().len(),
        MAX_STRUCT_ELEMENT_FIELDS
    );
    assert!(
        read(&array(
            1,
            &element(&field.repeat(MAX_STRUCT_ELEMENT_FIELDS + 1)),
            false
        ))
        .is_err()
    );
    let exact_count = array(
        MAX_STRUCT_ARRAY_ELEMENTS as i32,
        &END.repeat(MAX_STRUCT_ARRAY_ELEMENTS),
        false,
    );
    assert_eq!(
        read(&exact_count).unwrap().elements().len(),
        MAX_STRUCT_ARRAY_ELEMENTS
    );
    assert!(
        read(&array(
            MAX_STRUCT_ARRAY_ELEMENTS as i32 + 1,
            &END.repeat(MAX_STRUCT_ARRAY_ELEMENTS + 1),
            false
        ))
        .is_err()
    );
    assert!(read(&array(MAX_STRUCT_ARRAY_ELEMENTS as i32, &[], false)).is_err());

    let overhead = array(1, &element(&field), false).len();
    let payload_len = MAX_STRUCT_ARRAY_BYTES - overhead;
    let exact_bytes = array(
        1,
        &element(&self::field(NAME, 0, &[0], &vec![0; payload_len])),
        false,
    );
    assert_eq!(exact_bytes.len(), MAX_STRUCT_ARRAY_BYTES);
    assert_eq!(
        read(&exact_bytes).unwrap().summary().opaque_payload_bytes,
        payload_len
    );
    let excess = array(
        1,
        &element(&self::field(NAME, 0, &[0], &vec![0; payload_len + 1])),
        false,
    );
    assert_eq!(excess.len(), MAX_STRUCT_ARRAY_BYTES + 1);
    assert!(read(&excess).err().unwrap().to_string().contains("64 KiB"));
}

#[test]
fn late_bad_element_fails_without_partial_result_or_private_error_text() {
    let good = element(&field(NAME, 0, &[0], b"private-value-marker"));
    let bad = element(&field(
        b"\x11\0\0\0AuthoredSecretZZ\0",
        0,
        &[0],
        b"private-value-marker",
    ));
    let bytes = array(2, &[good.as_slice(), bad.as_slice()].concat(), false);
    let message = format!("{:#}", read(&bytes).err().unwrap());
    assert!(message.contains("unsupported tag header kind"));
    assert!(!message.contains("AuthoredSecret"));
    assert!(!message.contains("private-value-marker"));
    assert_eq!(
        read(&array(2, &good.repeat(2), false))
            .unwrap()
            .elements()
            .len(),
        2
    );
}

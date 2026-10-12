use icarus_bevy::experimental::recorders::{
    MAX_RECORDER_FIELDS, MAX_RECORDERS, RecorderArray, read_recorders,
};
use icarus_bevy::experimental::tagged::{MAX_INPUT_BYTES, RootTag, TagKind, TagProfile, read_root};

const END: &[u8] = b"\x05\0\0\0None\0";
const PROFILE: TagProfile = TagProfile::MemoryArchive522;
// Authored payloads deliberately contain a terminal-looking byte sequence and
// non-text bytes. They must never be parsed as additional properties.
const BODY: &[u8] = b"\xff\0\x05\0\0\0None\0private-body-marker";
const CLASS: &[u8] = b"\x10\0\0\0private-class-X\0";

// Independent manual fixture assembly, never using the production string encoder.
fn string(text: &str) -> Vec<u8> {
    assert!(text.is_ascii());
    [
        i32::try_from(text.len() + 1)
            .unwrap()
            .to_le_bytes()
            .as_slice(),
        text.as_bytes(),
        &[0],
    ]
    .concat()
}

fn tag(name: &str, kind: &str, index: i32, extras: &[u8], payload: &[u8]) -> Vec<u8> {
    [
        string(name).as_slice(),
        string(kind).as_slice(),
        &i32::try_from(payload.len()).unwrap().to_le_bytes(),
        &index.to_le_bytes(),
        extras,
        payload,
    ]
    .concat()
}

fn class(payload: &[u8]) -> Vec<u8> {
    tag("ComponentClassName", "StrProperty", 0, &[0], payload)
}

fn binary(payload: &[u8]) -> Vec<u8> {
    tag(
        "BinaryData",
        "ArrayProperty",
        0,
        &[string("ByteProperty"), vec![0]].concat(),
        payload,
    )
}

fn binary_value() -> Vec<u8> {
    [
        i32::try_from(BODY.len()).unwrap().to_le_bytes().as_slice(),
        BODY,
    ]
    .concat()
}

fn element() -> Vec<u8> {
    // 15 ASCII bytes plus terminator: the count includes that final unit.
    [
        class(b"\x10\0\0\0private-class-X\0").as_slice(),
        binary(&binary_value()).as_slice(),
        END,
    ]
    .concat()
}

fn root_with_shared(count: i32, shared: &[u8]) -> Vec<u8> {
    let payload = [&count.to_le_bytes(), shared].concat();
    [
        tag(
            "StateRecorderBlobs",
            "ArrayProperty",
            0,
            &[string("StructProperty"), vec![0]].concat(),
            &payload,
        ),
        END.to_vec(),
    ]
    .concat()
}

fn shared(body: &[u8]) -> Vec<u8> {
    tag(
        "StateRecorderBlobs",
        "StructProperty",
        0,
        &[string("StateRecorderBlob"), vec![0; 16], vec![0]].concat(),
        body,
    )
}

fn fixture(count: i32, body: &[u8]) -> Vec<u8> {
    root_with_shared(count, &shared(body))
}

fn read(bytes: &[u8]) -> anyhow::Result<RecorderArray<'_>> {
    let roots = read_root(bytes, PROFILE)?;
    read_recorders(&roots.tags, PROFILE)
}

#[test]
fn known_and_unknown_fields_preserve_exact_borrowed_spans_and_summary_privacy() {
    let unknown = tag(
        "private-unknown-name",
        "IntProperty",
        7,
        &[1; 17],
        b"opaque-no-int-width-check",
    );
    let element = [
        class(CLASS),
        unknown.clone(),
        binary(&binary_value()),
        END.to_vec(),
    ]
    .concat();
    let bytes = fixture(2, &element.repeat(2));
    let parsed = read(&bytes).unwrap();
    assert_eq!(parsed.records().len(), 2);
    assert_eq!(parsed.records()[0].record, element);
    assert_eq!(parsed.records()[0].fields()[1].record, unknown);
    assert_eq!(parsed.records()[0].binary_data, BODY);
    assert_eq!(parsed.records()[0].class_value, CLASS);
    assert_eq!(parsed.records()[0].terminator, END);
    let mut offset =
        parsed.root_header.len() + parsed.count_prefix.len() + parsed.shared_header.len();
    for recorder in parsed.records() {
        assert_eq!(recorder.record.as_ptr(), bytes[offset..].as_ptr());
        let mut field_offset = offset;
        for field in recorder.fields() {
            assert_eq!(field.record.as_ptr(), bytes[field_offset..].as_ptr());
            assert_eq!(field.header.len() + field.payload.len(), field.record.len());
            field_offset += field.record.len();
        }
        offset += recorder.record.len();
    }
    assert_eq!(offset, parsed.root_record.len());
    let summary = parsed.summary();
    assert_eq!(summary.recorder_count, 2);
    assert_eq!(summary.field_tags, 6);
    assert_eq!(summary.unknown_fields, 2);
    assert_eq!(summary.opaque_binary_bytes, BODY.len() * 2);
    assert_eq!(summary.class_value_bytes, CLASS.len() * 2);
    assert!(!summary.world_imported);
    let text = serde_json::to_string(&summary).unwrap();
    assert!(!text.contains("private"));
    assert!(!format!("{summary:?}").contains("private"));
}

#[test]
fn exact_counts_boundaries_and_every_element_truncation_are_required() {
    let element = element();
    assert!(read(&fixture(1, &element)).is_ok());
    assert_eq!(read(&fixture(0, &[])).unwrap().summary().recorder_count, 0);
    for count in [-1, 0, 2, i32::MAX] {
        assert!(read(&fixture(count, &element)).is_err());
    }
    for length in 0..element.len() {
        assert!(
            read(&fixture(1, &element[..length])).is_err(),
            "element truncation {length}"
        );
    }
    assert!(read(&fixture(1, &[element.as_slice(), &[0]].concat())).is_err());
    assert!(read(&root_with_shared(1, &[shared(&element), vec![0]].concat())).is_err());
    assert!(read(&root_with_shared(1, END)).is_err());
    let shared = shared(&element);
    for length in 0..shared.len() {
        assert!(
            read(&root_with_shared(1, &shared[..length])).is_err(),
            "shared truncation {length}"
        );
    }
}

#[test]
fn missing_duplicate_and_wrongly_indexed_known_fields_reject_schema_ambiguity() {
    for fields in [
        vec![],
        vec![class(CLASS)],
        vec![binary(&binary_value())],
        vec![class(CLASS), class(CLASS), binary(&binary_value())],
        vec![
            class(CLASS),
            binary(&binary_value()),
            binary(&binary_value()),
        ],
    ] {
        let body = [fields.concat(), END.to_vec()].concat();
        assert!(read(&fixture(1, &body)).is_err());
    }
    for bad_class in [
        tag("ComponentClassName", "StrProperty", 1, &[0], CLASS),
        tag("ComponentClassName", "IntProperty", 0, &[0], CLASS),
    ] {
        let body = [bad_class, binary(&binary_value()), END.to_vec()].concat();
        assert!(read(&fixture(1, &body)).is_err());
    }
    for bad_binary in [
        tag(
            "BinaryData",
            "ArrayProperty",
            1,
            &[string("ByteProperty"), vec![0]].concat(),
            &binary_value(),
        ),
        tag(
            "BinaryData",
            "ArrayProperty",
            0,
            &[string("IntProperty"), vec![0]].concat(),
            &binary_value(),
        ),
        tag("BinaryData", "StrProperty", 0, &[0], &binary_value()),
    ] {
        let body = [class(CLASS), bad_binary, END.to_vec()].concat();
        assert!(read(&fixture(1, &body)).is_err());
    }
}

#[test]
fn root_and_shared_headers_are_exact_and_duplicates_are_rejected() {
    let one = fixture(1, &element());
    let roots = read_root(&one, PROFILE).unwrap();
    let duplicated = [roots.tags[0].record, roots.tags[0].record, END].concat();
    assert!(read(&duplicated).is_err());
    assert!(read(END).is_err());
    for (name, kind, structure, index) in [
        ("Other", "StructProperty", "StateRecorderBlob", 0),
        ("StateRecorderBlobs", "StructProperty", "Other", 0),
        (
            "StateRecorderBlobs",
            "StructProperty",
            "StateRecorderBlob",
            1,
        ),
    ] {
        let wrong = tag(
            name,
            kind,
            index,
            &[string(structure), vec![0; 17]].concat(),
            &element(),
        );
        assert!(read(&root_with_shared(1, &wrong)).is_err());
    }
    for (kind, inner, index) in [
        ("ArrayProperty", "ByteProperty", 0),
        ("ArrayProperty", "StructProperty", 1),
    ] {
        let root = tag(
            "StateRecorderBlobs",
            kind,
            index,
            &[string(inner), vec![0]].concat(),
            &[],
        );
        assert!(read(&[root, END.to_vec()].concat()).is_err());
    }
}

#[test]
fn class_and_binary_payloads_have_exact_bounded_framing() {
    for payload in [
        vec![],
        vec![0; 3],
        (-1_i32).to_le_bytes().to_vec(),
        1_i32.to_le_bytes().to_vec(),
        [vec![0; 4], vec![1]].concat(),
    ] {
        let body = [class(CLASS), binary(&payload), END.to_vec()].concat();
        assert!(read(&fixture(1, &body)).is_err());
    }
    for payload in [
        vec![],
        vec![0; 3],
        [CLASS, &[0]].concat(),
        1025_i32.to_le_bytes().to_vec(),
        (-1025_i32).to_le_bytes().to_vec(),
        i32::MIN.to_le_bytes().to_vec(),
        b"\x02\0\0\0Xz".to_vec(),
        b"\x03\0\0\0\xffX\0".to_vec(),
    ] {
        let body = [class(&payload), binary(&binary_value()), END.to_vec()].concat();
        let error = read(&fixture(1, &body)).err().unwrap();
        assert!(!format!("{error:#}").contains("private"));
    }
    // FString zero length and raw UTF-16 units remain framing, not class resolution.
    for payload in [vec![0; 4], vec![254, 255, 255, 255, 0, 216, 0, 0]] {
        let body = [class(&payload), binary(&[0; 4]), END.to_vec()].concat();
        let bytes = fixture(1, &body);
        let parsed = read(&bytes).unwrap();
        assert_eq!(parsed.records()[0].class_value, payload);
        assert!(parsed.records()[0].binary_data.is_empty());
    }
}

#[test]
fn flags_unknown_headers_and_negative_sizes_never_expose_values() {
    let ordinary = class(CLASS);
    let name_type_len = string("ComponentClassName").len() + string("StrProperty").len();
    for (offset, value) in [(name_type_len, -1_i32), (name_type_len + 4, -1)] {
        let mut bad = ordinary.clone();
        bad[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        assert!(
            read(&fixture(
                1,
                &[bad, binary(&binary_value()), END.to_vec()].concat()
            ))
            .is_err()
        );
    }
    let mut flag = ordinary;
    flag[name_type_len + 8] = 2;
    for bad in [
        flag,
        tag("private-name", "private-kind", 0, &[0], b"private-value"),
        tag("private-name", "BoolProperty", 0, &[2, 0], b""),
    ] {
        let bytes = fixture(1, &[bad, binary(&binary_value()), END.to_vec()].concat());
        let error = read(&bytes).err().unwrap();
        assert!(!format!("{error:#}").contains("private"));
    }
}

#[test]
fn recorder_field_string_and_input_limits_are_enforced() {
    assert!(read(&fixture(i32::try_from(MAX_RECORDERS + 1).unwrap(), &[])).is_err());
    let empty = [class(&[0; 4]), binary(&[0; 4]), END.to_vec()].concat();
    let bytes = fixture(
        i32::try_from(MAX_RECORDERS).unwrap(),
        &empty.repeat(MAX_RECORDERS),
    );
    assert_eq!(
        read(&bytes).unwrap().summary().recorder_count,
        MAX_RECORDERS
    );

    let unknown = tag("U", "IntProperty", 0, &[0], &[]);
    let body = [
        class(&[0; 4]),
        binary(&[0; 4]),
        unknown.repeat(MAX_RECORDER_FIELDS - 2),
        END.to_vec(),
    ]
    .concat();
    assert_eq!(
        read(&fixture(1, &body)).unwrap().summary().field_tags,
        MAX_RECORDER_FIELDS
    );
    let excess = [unknown, body].concat();
    assert!(read(&fixture(1, &excess)).is_err());

    let oversized = vec![0; MAX_INPUT_BYTES + 1];
    let root = RootTag {
        kind: TagKind::Array,
        record: &oversized,
        header: &[],
        payload: &[],
    };
    let error = read_recorders(&[root], PROFILE).err().unwrap();
    assert!(error.to_string().contains("32 MiB"));
    let long = [1024_i32.to_le_bytes().to_vec(), vec![b'X'; 1023], vec![0]].concat();
    let body = [class(&long), binary(&[0; 4]), END.to_vec()].concat();
    assert!(read(&fixture(1, &body)).is_ok());
}

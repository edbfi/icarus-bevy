use icarus_bevy::experimental::int_array::{
    IntArray, MAX_INT_ARRAY_BYTES, MAX_INT_ARRAY_ELEMENTS, read_int_array,
};
use icarus_bevy::experimental::tagged::TagProfile;

fn read(bytes: &[u8]) -> anyhow::Result<IntArray<'_>> {
    read_int_array(bytes, TagProfile::MemoryArchive522)
}

fn body(elements: &[[u8; 4]]) -> Vec<u8> {
    let mut bytes = (elements.len() as i32).to_le_bytes().to_vec();
    bytes.extend(elements.iter().flatten());
    bytes
}

#[test]
fn empty_and_populated_borrow_exact_opaque_spans() {
    for elements in [
        vec![],
        vec![[0xff, 0xff, 0xff, 0xff]],
        vec![[0; 4], [0, 0, 0, 0x80], *b"ABCD"],
    ] {
        let bytes = body(&elements);
        let parsed = read(&bytes).unwrap();
        assert_eq!(parsed.prefix(), &bytes[..4]);
        assert_eq!(parsed.prefix().as_ptr(), bytes.as_ptr());
        assert_eq!(parsed.elements(), elements);
        for (index, element) in parsed.elements().iter().enumerate() {
            assert_eq!(element.as_ptr(), bytes[4 + index * 4..].as_ptr());
        }
        let summary = parsed.summary();
        assert_eq!(summary.element_count, elements.len());
        assert_eq!(summary.array_prefix_bytes, 4);
        assert_eq!(summary.opaque_payload_bytes, elements.len() * 4);
        assert_eq!(
            summary.array_prefix_bytes + summary.opaque_payload_bytes,
            bytes.len()
        );
        assert!(!summary.values_interpreted);
        assert!(!summary.world_imported);
    }
}

#[test]
fn every_truncation_and_extra_byte_fails() {
    let bytes = body(&[[0x77; 4], [0x88; 4]]);
    for length in 0..bytes.len() {
        assert!(read(&bytes[..length]).is_err(), "length {length}");
    }
    let mut extra = bytes.clone();
    extra.push(0);
    assert!(read(&extra).is_err());
    for count in [0i32, 1, 3] {
        let mut mismatch = bytes.clone();
        mismatch[..4].copy_from_slice(&count.to_le_bytes());
        assert!(read(&mismatch).is_err());
    }
    assert!(read(&[0, 0, 0, 0, 0]).is_err());
}

#[test]
fn signed_and_resource_bounds_fail_before_element_access() {
    for count in [-1i32, i32::MIN, i32::MAX] {
        assert!(read(&count.to_le_bytes()).is_err());
    }
    let exact = body(&vec![[0xa5; 4]; MAX_INT_ARRAY_ELEMENTS]);
    assert_eq!(exact.len(), 16_388);
    assert_eq!(
        read(&exact).unwrap().elements().len(),
        MAX_INT_ARRAY_ELEMENTS
    );
    let excess = body(&vec![[0xa5; 4]; MAX_INT_ARRAY_ELEMENTS + 1]);
    assert!(excess.len() < MAX_INT_ARRAY_BYTES);
    assert!(read(&excess).is_err());
    let oversized = vec![0; MAX_INT_ARRAY_BYTES + 1];
    assert!(
        read(&oversized)
            .err()
            .unwrap()
            .to_string()
            .contains("64 KiB")
    );
}

#[test]
fn summaries_and_errors_do_not_disclose_opaque_values() {
    let bytes = body(&[*b"SECR", *b"ET!!"]);
    let summary = serde_json::to_value(read(&bytes).unwrap().summary()).unwrap();
    assert_eq!(
        summary,
        serde_json::json!({
            "element_count": 2, "array_prefix_bytes": 4, "opaque_payload_bytes": 8,
            "values_interpreted": false, "world_imported": false
        })
    );
    let mut malformed = bytes;
    malformed[..4].copy_from_slice(&3i32.to_le_bytes());
    let error = format!("{:#}", read(&malformed).err().unwrap());
    assert!(!error.contains("SECRET"));
    assert!(!error.contains("SECR"));
}

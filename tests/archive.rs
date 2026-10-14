use icarus_bevy::experimental::archive::{
    ByteOrder, MAX_STRING_UNITS, decode_string, encode_string,
};

#[test]
fn explicit_endian_fixtures_match_ascii_and_wide_encoding() {
    // Independently packed signed counts and units, including one terminator.
    // Wide sample is A plus U+1F600 (two UTF-16 surrogate units).
    for (order, ascii, wide) in [
        (
            ByteOrder::Little,
            vec![4, 0, 0, 0, b'L', b'A', b'B', 0],
            vec![252, 255, 255, 255, 65, 0, 61, 216, 0, 222, 0, 0],
        ),
        (
            ByteOrder::Big,
            vec![0, 0, 0, 4, b'L', b'A', b'B', 0],
            vec![255, 255, 255, 252, 0, 65, 216, 61, 222, 0, 0, 0],
        ),
    ] {
        assert_eq!(encode_string(&[76, 65, 66], order).unwrap(), ascii);
        assert_eq!(encode_string(&[65, 0xd83d, 0xde00], order).unwrap(), wide);
        for (fixture, expected) in [(ascii, vec![76, 65, 66]), (wide, vec![65, 0xd83d, 0xde00])] {
            let with_next_field = [fixture.as_slice(), &[0xff; 8]].concat();
            let decoded = decode_string(&with_next_field, order).unwrap();
            assert_eq!(decoded.units, expected);
            assert_eq!(decoded.consumed, fixture.len());
            for length in 0..fixture.len() {
                assert!(decode_string(&fixture[..length], order).is_err());
            }
        }
    }
}

#[test]
fn zero_single_terminator_and_raw_surrogates_have_explicit_semantics() {
    for fixture in [
        vec![0, 0, 0, 0],
        vec![1, 0, 0, 0, 0],
        vec![255, 255, 255, 255, 0, 0],
    ] {
        let decoded = decode_string(&fixture, ByteOrder::Little).unwrap();
        assert!(decoded.units.is_empty());
        assert_eq!(decoded.consumed, fixture.len());
    }
    assert_eq!(encode_string(&[], ByteOrder::Big).unwrap(), [0; 4]);
    let encoded = encode_string(&[0xd800, 65, 0xdc00], ByteOrder::Little).unwrap();
    assert_eq!(
        decode_string(&encoded, ByteOrder::Little).unwrap().units,
        [0xd800, 65, 0xdc00]
    );
}

#[test]
fn resource_and_normalization_boundaries_fail_without_partial_acceptance() {
    for count in [i32::MIN, i32::MAX, 65_537, -65_537] {
        assert!(decode_string(&count.to_le_bytes(), ByteOrder::Little).is_err());
    }
    for fixture in [
        vec![2, 0, 0, 0, b'A', b'B'],             // Missing terminator.
        vec![3, 0, 0, 0, 0, b'B', 0],             // Embedded NUL.
        vec![2, 0, 0, 0, 0x80, 0],                // Original would substitute '?'.
        vec![254, 255, 255, 255, 255, 255, 0, 0], // Original U+FFFF normalization.
    ] {
        assert!(decode_string(&fixture, ByteOrder::Little).is_err());
    }
    for invalid in [vec![0], vec![0xffff], vec![65; MAX_STRING_UNITS]] {
        assert!(encode_string(&invalid, ByteOrder::Little).is_err());
    }
    let maximum = vec![0x1234; MAX_STRING_UNITS - 1];
    let encoded = encode_string(&maximum, ByteOrder::Little).unwrap();
    assert_eq!(
        decode_string(&encoded, ByteOrder::Little).unwrap().units,
        maximum
    );
}

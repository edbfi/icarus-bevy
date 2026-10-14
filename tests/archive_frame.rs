use anyhow::{Result, ensure};
use icarus_bevy::experimental::archive::{
    ArchiveString, ByteOrder, MAX_STRING_UNITS, StringEncoding, decode_string, read_string_frame,
};

// Independently retained pre-change decoder; intentionally does not invoke the
// production frame validator or encoder. Pins acceptance and error precedence.
fn previous_decoder(input: &[u8], order: ByteOrder) -> Result<ArchiveString> {
    let prefix: [u8; 4] = input
        .get(..4)
        .ok_or_else(|| anyhow::anyhow!("truncated archive string count"))?
        .try_into()?;
    let count = match order {
        ByteOrder::Little => i32::from_le_bytes(prefix),
        ByteOrder::Big => i32::from_be_bytes(prefix),
    };
    let magnitude = count.unsigned_abs() as usize;
    ensure!(
        magnitude <= MAX_STRING_UNITS,
        "archive string exceeds research unit limit"
    );
    if magnitude == 0 {
        return Ok(ArchiveString {
            units: Vec::new(),
            consumed: 4,
        });
    }
    let wide = count < 0;
    let consumed = 4 + magnitude * if wide { 2 } else { 1 };
    let payload = input
        .get(4..consumed)
        .ok_or_else(|| anyhow::anyhow!("truncated archive string payload"))?;
    let mut units: Vec<u16> = if wide {
        payload
            .as_chunks::<2>()
            .0
            .iter()
            .map(|bytes| match order {
                ByteOrder::Little => u16::from_le_bytes(*bytes),
                ByteOrder::Big => u16::from_be_bytes(*bytes),
            })
            .collect()
    } else {
        ensure!(
            payload.is_ascii(),
            "non-ASCII narrow archive string unsupported"
        );
        payload.iter().copied().map(u16::from).collect()
    };
    ensure!(
        units.last() == Some(&0),
        "missing archive string terminator"
    );
    units.pop();
    ensure!(
        units.len() < MAX_STRING_UNITS,
        "archive string exceeds research unit limit"
    );
    ensure!(
        !units.contains(&0),
        "embedded archive string NUL unsupported"
    );
    ensure!(
        !units.contains(&0xffff),
        "archive string U+FFFF normalization unsupported"
    );
    Ok(ArchiveString { units, consumed })
}

fn prefix(count: i32, order: ByteOrder) -> [u8; 4] {
    match order {
        ByteOrder::Little => count.to_le_bytes(),
        ByteOrder::Big => count.to_be_bytes(),
    }
}

// The supplied units include any final terminator; no production encoder used.
fn wide(units: &[u16], order: ByteOrder) -> Vec<u8> {
    let mut bytes = prefix(-i32::try_from(units.len()).unwrap(), order).to_vec();
    for unit in units {
        bytes.extend_from_slice(&match order {
            ByteOrder::Little => unit.to_le_bytes(),
            ByteOrder::Big => unit.to_be_bytes(),
        });
    }
    bytes
}

fn chain(error: anyhow::Error) -> Vec<String> {
    error.chain().map(ToString::to_string).collect()
}

fn compare_previous(bytes: &[u8], order: ByteOrder) {
    let expected = previous_decoder(bytes, order).map_err(chain);
    assert_eq!(decode_string(bytes, order).map_err(chain), expected);
    let framed = read_string_frame(bytes, order)
        .map(|frame| ArchiveString {
            units: frame.units().collect(),
            consumed: frame.consumed(),
        })
        .map_err(chain);
    assert_eq!(framed, expected);
}

#[test]
fn explicit_frames_borrow_unaligned_original_spans_and_leave_following_fields() {
    for (order, ascii, utf16) in [
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
        for (bytes, encoding, units, width, signed_count) in [
            (ascii, StringEncoding::Narrow, vec![76, 65, 66], 1, 4),
            (utf16, StringEncoding::Wide, vec![65, 0xd83d, 0xde00], 2, -4),
        ] {
            let storage = [&[0xab][..], bytes.as_slice(), &[0xff; 8]].concat();
            let source = &storage[1..];
            let frame = read_string_frame(source, order).unwrap();
            assert_eq!(frame.frame(), bytes);
            assert_eq!(frame.frame().as_ptr(), source.as_ptr());
            assert_eq!(frame.prefix(), &bytes[..4]);
            assert_eq!(frame.prefix().as_ptr(), source.as_ptr());
            assert_eq!(frame.payload(), &bytes[4..]);
            assert_eq!(frame.payload().as_ptr(), source[4..].as_ptr());
            assert_eq!(frame.content(), &bytes[4..bytes.len() - width]);
            assert_eq!(frame.content().as_ptr(), source[4..].as_ptr());
            assert_eq!(frame.terminator().unwrap(), &bytes[bytes.len() - width..]);
            assert_eq!(
                frame.terminator().unwrap().as_ptr(),
                source[bytes.len() - width..].as_ptr()
            );
            assert_eq!(frame.consumed(), bytes.len());
            assert_eq!(frame.signed_count(), signed_count);
            assert_eq!(frame.declared_units(), 4);
            assert_eq!(frame.unit_count(), 3);
            assert_eq!(frame.encoding(), encoding);
            assert_eq!(frame.units().len(), 3);
            assert_eq!(frame.units().collect::<Vec<_>>(), units);
            assert_eq!(
                frame.units().rev().collect::<Vec<_>>(),
                units.iter().copied().rev().collect::<Vec<_>>()
            );
            compare_previous(source, order);
            for end in 0..bytes.len() {
                compare_previous(&bytes[..end], order);
                assert!(read_string_frame(&bytes[..end], order).is_err());
            }
        }
    }
}

#[test]
fn empty_forms_and_raw_surrogates_preserve_encoding_and_counts() {
    for order in [ByteOrder::Little, ByteOrder::Big] {
        for (bytes, encoding, count, terminator_bytes) in [
            (prefix(0, order).to_vec(), StringEncoding::Empty, 0, 0),
            (
                [prefix(1, order).as_slice(), &[0]].concat(),
                StringEncoding::Narrow,
                1,
                1,
            ),
            (wide(&[0], order), StringEncoding::Wide, -1, 2),
        ] {
            let frame = read_string_frame(&bytes, order).unwrap();
            assert_eq!(frame.encoding(), encoding);
            assert_eq!(frame.signed_count(), count);
            assert_eq!(frame.declared_units(), usize::from(count != 0));
            assert_eq!(frame.unit_count(), 0);
            assert!(frame.content().is_empty());
            assert_eq!(frame.units().len(), 0);
            assert_eq!(
                frame.terminator().map_or(0, |slice| slice.len()),
                terminator_bytes
            );
            assert_eq!(frame.terminator().is_some(), count != 0);
            compare_previous(&bytes, order);
        }
        for units in [
            vec![0xd800, 65, 0xdc00, 0],
            vec![0xd800, 0],
            vec![0xdc00, 0],
        ] {
            let bytes = wide(&units, order);
            let frame = read_string_frame(&bytes, order).unwrap();
            assert_eq!(frame.units().collect::<Vec<_>>(), &units[..units.len() - 1]);
            compare_previous(&bytes, order);
        }
    }
}

#[test]
fn strict_error_precedence_matches_legacy_even_with_multiple_invalid_units() {
    for order in [ByteOrder::Little, ByteOrder::Big] {
        for (bytes, expected) in [
            (
                wide(&[0xffff, 0, 65], order),
                "missing archive string terminator",
            ),
            (
                wide(&[0, 0xffff, 65], order),
                "missing archive string terminator",
            ),
            (
                wide(&[0xffff, 0, 0], order),
                "embedded archive string NUL unsupported",
            ),
            (
                wide(&[0, 0xffff, 0], order),
                "embedded archive string NUL unsupported",
            ),
            (
                wide(&[0xffff, 65, 0], order),
                "archive string U+FFFF normalization unsupported",
            ),
            (
                [prefix(3, order).as_slice(), &[0, 128, 65]].concat(),
                "non-ASCII narrow archive string unsupported",
            ),
            (
                [prefix(3, order).as_slice(), &[0, 65, 65]].concat(),
                "missing archive string terminator",
            ),
        ] {
            assert_eq!(
                chain(read_string_frame(&bytes, order).err().unwrap()),
                [expected]
            );
            compare_previous(&bytes, order);
        }
    }
}

#[test]
fn differential_authored_corpus_pins_acceptance_consumption_and_error_chains() {
    let alphabet = [0, 65, 127, 128, 0xd800, 0xdc00, 0xffff];
    for order in [ByteOrder::Little, ByteOrder::Big] {
        for count in -8..=8 {
            let bytes = [prefix(count, order).as_slice(), &[0; 20]].concat();
            for length in 0..=bytes.len() {
                compare_previous(&bytes[..length], order);
            }
        }
        for first in 0..=u8::MAX {
            for second in [0, 1, 65, 127, 128, 255] {
                for terminal in [0, 65, 128] {
                    let bytes = [
                        prefix(3, order).as_slice(),
                        &[first, second, terminal],
                        &[0xff; 4],
                    ]
                    .concat();
                    compare_previous(&bytes, order);
                }
            }
        }
        for first in alphabet {
            for second in alphabet {
                for terminal in alphabet {
                    let bytes = [
                        wide(&[first, second, terminal], order).as_slice(),
                        &[0xff; 4],
                    ]
                    .concat();
                    compare_previous(&bytes, order);
                }
            }
        }
    }
}

#[test]
fn count_caps_precede_payload_validation_and_maximum_frames_remain_borrowed() {
    for order in [ByteOrder::Little, ByteOrder::Big] {
        for count in [i32::MIN, i32::MAX, 65_537, -65_537] {
            let bytes = [prefix(count, order).as_slice(), &[0xff; 8]].concat();
            assert_eq!(
                chain(read_string_frame(&bytes, order).err().unwrap()),
                ["archive string exceeds research unit limit"]
            );
            compare_previous(&bytes, order);
        }
        for wide_encoding in [false, true] {
            let count = i32::try_from(MAX_STRING_UNITS).unwrap();
            let bytes = if wide_encoding {
                let mut units = vec![0x1234; MAX_STRING_UNITS];
                *units.last_mut().unwrap() = 0;
                wide(&units, order)
            } else {
                let mut bytes = prefix(count, order).to_vec();
                bytes.extend(std::iter::repeat_n(b'A', MAX_STRING_UNITS - 1));
                bytes.push(0);
                bytes
            };
            let frame = read_string_frame(&bytes, order).unwrap();
            assert_eq!(frame.frame().as_ptr(), bytes.as_ptr());
            assert_eq!(frame.declared_units(), MAX_STRING_UNITS);
            assert_eq!(frame.unit_count(), MAX_STRING_UNITS - 1);
            assert_eq!(frame.units().len(), MAX_STRING_UNITS - 1);
            compare_previous(&bytes, order);
            compare_previous(&bytes[..bytes.len() - 1], order);
        }
    }
}

#[test]
fn errors_report_only_structure_and_never_original_contents() {
    for order in [ByteOrder::Little, ByteOrder::Big] {
        let marker = b"private-frame-marker";
        let bytes = [
            prefix(i32::try_from(marker.len() + 1).unwrap(), order).as_slice(),
            marker,
            b"X",
        ]
        .concat();
        let message = format!("{:#}", read_string_frame(&bytes, order).err().unwrap());
        assert_eq!(message, "missing archive string terminator");
        assert!(!message.contains("private-frame-marker"));
        compare_previous(&bytes, order);
    }
}

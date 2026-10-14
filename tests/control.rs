use icarus_bevy::experimental::archive::{ByteOrder, MAX_STRING_UNITS};
use icarus_bevy::experimental::control::{
    InitialControlBody, MAX_INITIAL_CONTROL_BODY_BYTES, TokenEncoding, read_initial_control_body,
    read_initial_control_body_with_sender_order, sender_byte_order,
};

// Independently authored archive bytes; no production encoder or client data.
fn count(value: i32, order: ByteOrder) -> [u8; 4] {
    match order {
        ByteOrder::Little => value.to_le_bytes(),
        ByteOrder::Big => value.to_be_bytes(),
    }
}

fn body(first: u8, version: u32, token: &[u8], order: ByteOrder) -> Vec<u8> {
    let version = match order {
        ByteOrder::Little => version.to_le_bytes(),
        ByteOrder::Big => version.to_be_bytes(),
    };
    [&[first][..], &version, token].concat()
}

fn wide(units: &[u16], order: ByteOrder) -> Vec<u8> {
    let mut bytes = count(-i32::try_from(units.len() + 1).unwrap(), order).to_vec();
    for unit in units.iter().copied().chain(std::iter::once(0)) {
        bytes.extend_from_slice(&match order {
            ByteOrder::Little => unit.to_le_bytes(),
            ByteOrder::Big => unit.to_be_bytes(),
        });
    }
    bytes
}

fn error(bytes: &[u8], order: ByteOrder) -> String {
    format!(
        "{:#}",
        read_initial_control_body(bytes, order).err().unwrap()
    )
}

#[test]
fn explicit_endian_authored_fields_preserve_exact_source_spans() {
    for (order, version, ascii, utf16) in [
        (
            ByteOrder::Little,
            [4, 3, 2, 1],
            vec![4, 0, 0, 0, b'L', b'A', b'B', 0],
            vec![252, 255, 255, 255, 65, 0, 61, 216, 0, 222, 0, 0],
        ),
        (
            ByteOrder::Big,
            [1, 2, 3, 4],
            vec![0, 0, 0, 4, b'L', b'A', b'B', 0],
            vec![255, 255, 255, 252, 0, 65, 216, 61, 222, 0, 0, 0],
        ),
    ] {
        for (token, encoding) in [(ascii, TokenEncoding::Narrow), (utf16, TokenEncoding::Wide)] {
            let bytes = [&[0x7f][..], &version, token.as_slice()].concat();
            let parsed = read_initial_control_body(&bytes, order).unwrap();
            assert_eq!(parsed.first_field(), 0x7f);
            assert_eq!(parsed.remote_version(), 0x0102_0304);
            assert_eq!(parsed.body(), bytes);
            assert_eq!(parsed.body().as_ptr(), bytes.as_ptr());
            assert_eq!(parsed.first_field_bytes(), [0x7f]);
            assert_eq!(parsed.first_field_bytes().as_ptr(), bytes.as_ptr());
            assert_eq!(parsed.remote_version_bytes(), version);
            assert_eq!(parsed.remote_version_bytes().as_ptr(), bytes[1..].as_ptr());
            assert_eq!(parsed.prefix(), &bytes[..5]);
            assert_eq!(parsed.prefix().as_ptr(), bytes.as_ptr());
            assert_eq!(parsed.token_frame(), token);
            assert_eq!(parsed.token_frame().as_ptr(), bytes[5..].as_ptr());
            let summary = parsed.summary();
            assert_eq!(summary.token_encoding, encoding);
            assert_eq!(summary.token_units, 3);
            assert_eq!(summary.prefix_bytes, 5);
            assert_eq!(summary.token_bytes, token.len());
            assert_eq!(summary.body_bytes, bytes.len());
            assert_eq!(summary.prefix_bytes + summary.token_bytes, bytes.len());
        }
    }
}

#[test]
fn all_first_bytes_empty_forms_and_remote_extremes_are_structural() {
    for order in [ByteOrder::Little, ByteOrder::Big] {
        for (token, encoding) in [
            (count(0, order).to_vec(), TokenEncoding::Empty),
            (
                [count(1, order).as_slice(), &[0]].concat(),
                TokenEncoding::Narrow,
            ),
            (wide(&[], order), TokenEncoding::Wide),
        ] {
            for first in 0..=u8::MAX {
                for version in [0, u32::MAX] {
                    let bytes = body(first, version, &token, order);
                    let parsed = read_initial_control_body(&bytes, order).unwrap();
                    assert_eq!(parsed.first_field(), first);
                    assert_eq!(parsed.remote_version(), version);
                    assert_eq!(parsed.token_frame(), token);
                    let summary = parsed.summary();
                    assert_eq!(summary.token_units, 0);
                    assert_eq!(summary.token_encoding, encoding);
                    assert!(!summary.negotiation_validated);
                    assert!(!summary.identity_verified);
                    assert!(!summary.game_connection_established);
                }
            }
        }
    }
}

#[test]
fn raw_surrogate_frames_are_preserved_without_text_normalization() {
    for order in [ByteOrder::Little, ByteOrder::Big] {
        // Deliberately unpaired surrogates around an ASCII unit.
        let token = wide(&[0xd800, 65, 0xdc00], order);
        let bytes = body(255, 0xface_b00c, &token, order);
        let parsed = read_initial_control_body(&bytes, order).unwrap();
        assert_eq!(parsed.token_frame(), token);
        assert_eq!(parsed.summary().token_units, 3);
        assert_eq!(parsed.summary().token_encoding, TokenEncoding::Wide);
    }
}

#[test]
fn every_prefix_and_extra_body_data_fail_without_partial_success() {
    for order in [ByteOrder::Little, ByteOrder::Big] {
        for token in [
            count(0, order).to_vec(),
            [count(4, order).as_slice(), b"LAB\0"].concat(),
            wide(&[65, 0xd83d, 0xde00], order),
        ] {
            let bytes = body(0x7f, 0x0102_0304, &token, order);
            for length in 0..bytes.len() {
                assert!(
                    read_initial_control_body(&bytes[..length], order).is_err(),
                    "truncated body at {length}"
                );
            }
            for following in [vec![0], vec![255; 8], bytes.clone()] {
                let excess = [bytes.as_slice(), &following].concat();
                assert!(error(&excess, order).contains("trailing data"));
            }
            assert!(read_initial_control_body(&bytes, order).is_ok());
        }
        assert!(error(&[], order).contains("first field"));
        for size in 1..5 {
            assert!(error(&vec![0; size], order).contains("remote version"));
        }
        for size in 5..9 {
            assert!(error(&vec![0; size], order).contains("archive string count"));
        }
    }
}

#[test]
fn huge_signed_counts_fail_before_missing_payload_or_allocation() {
    for order in [ByteOrder::Little, ByteOrder::Big] {
        for value in [i32::MIN, i32::MAX, 65_537, -65_537] {
            let bytes = body(0, 0, &count(value, order), order);
            assert!(error(&bytes, order).contains("research unit limit"));
        }
        let truncated = body(0, 0, &count(2, order), order);
        assert!(error(&truncated, order).contains("truncated archive string payload"));
        let truncated = body(0, 0, &count(-2, order), order);
        assert!(error(&truncated, order).contains("truncated archive string payload"));
    }
}

#[test]
fn exact_string_unit_and_derived_body_limits_are_enforced() {
    assert_eq!(MAX_INITIAL_CONTROL_BODY_BYTES, 5 + 4 + 2 * MAX_STRING_UNITS);
    for order in [ByteOrder::Little, ByteOrder::Big] {
        let mut narrow = count(i32::try_from(MAX_STRING_UNITS).unwrap(), order).to_vec();
        narrow.extend_from_slice(&vec![b'A'; MAX_STRING_UNITS - 1]);
        narrow.push(0);
        let bytes = body(0, 0, &narrow, order);
        let summary = read_initial_control_body(&bytes, order).unwrap().summary();
        assert_eq!(summary.token_units, MAX_STRING_UNITS - 1);
        assert_eq!(summary.token_encoding, TokenEncoding::Narrow);

        let token = wide(&vec![0x1234; MAX_STRING_UNITS - 1], order);
        let bytes = body(0, 0, &token, order);
        assert_eq!(bytes.len(), MAX_INITIAL_CONTROL_BODY_BYTES);
        let parsed = read_initial_control_body(&bytes, order).unwrap();
        assert_eq!(parsed.token_frame().as_ptr(), bytes[5..].as_ptr());
        let summary = parsed.summary();
        assert_eq!(summary.token_units, MAX_STRING_UNITS - 1);
        assert_eq!(summary.token_encoding, TokenEncoding::Wide);
        assert_eq!(summary.prefix_bytes + summary.token_bytes, bytes.len());
        let excess = [bytes.as_slice(), &[0]].concat();
        assert_eq!(excess.len(), MAX_INITIAL_CONTROL_BODY_BYTES + 1);
        assert!(error(&excess, order).contains("research byte limit"));
        // Early whole-body ceiling applies even to malformed input.
        assert!(
            error(&vec![0; MAX_INITIAL_CONTROL_BODY_BYTES + 1], order)
                .contains("research byte limit")
        );
    }
}

#[test]
fn strict_token_normalization_rejections_remain_explicit() {
    for order in [ByteOrder::Little, ByteOrder::Big] {
        for (token, message) in [
            (
                [count(2, order).as_slice(), b"AB"].concat(),
                "missing archive string terminator",
            ),
            (
                [count(3, order).as_slice(), &[0, b'B', 0]].concat(),
                "embedded archive string NUL",
            ),
            (
                [count(2, order).as_slice(), &[0x80, 0]].concat(),
                "non-ASCII narrow",
            ),
            (wide(&[0xffff], order), "U+FFFF normalization"),
            (wide(&[0, 65], order), "embedded archive string NUL"),
        ] {
            let bytes = body(0, 0, &token, order);
            assert!(error(&bytes, order).contains(message));
        }
    }
}

#[test]
fn summaries_and_late_errors_do_not_render_fields_or_token_contents() {
    for order in [ByteOrder::Little, ByteOrder::Big] {
        let marker = b"private-token-marker";
        let token = [
            count(i32::try_from(marker.len() + 1).unwrap(), order).as_slice(),
            marker.as_slice(),
            &[0],
        ]
        .concat();
        let bytes = body(0x7f, 0xface_b00c, &token, order);
        let summary = read_initial_control_body(&bytes, order).unwrap().summary();
        for text in [
            format!("{summary:?}"),
            serde_json::to_string(&summary).unwrap(),
        ] {
            assert!(!text.contains("private-token-marker"));
            assert!(!text.contains("first_field"));
            assert!(!text.contains("remote_version"));
            assert!(!text.contains("4207849484"));
        }
        let mut missing_terminator = bytes.clone();
        *missing_terminator.last_mut().unwrap() = b'X';
        for malformed in [missing_terminator, [bytes.as_slice(), marker].concat()] {
            let message = error(&malformed, order);
            assert!(!message.contains("private-token-marker"));
            assert!(!message.contains("4207849484"));
        }
        assert!(!summary.negotiation_validated);
        assert!(!summary.identity_verified);
        assert!(!summary.game_connection_established);
    }
}

#[test]
fn frame_validation_preserves_complete_context_error_chains_and_precedence() {
    for order in [ByteOrder::Little, ByteOrder::Big] {
        for (token, expected) in [
            // U+FFFF precedes NUL, but NUL validation still takes precedence.
            (
                wide(&[0xffff, 0], order),
                "embedded archive string NUL unsupported",
            ),
            (
                wide(&[0, 0xffff], order),
                "embedded archive string NUL unsupported",
            ),
            (
                wide(&[0xffff], order),
                "archive string U+FFFF normalization unsupported",
            ),
            (
                [count(3, order).as_slice(), &[0, 128, 65]].concat(),
                "non-ASCII narrow archive string unsupported",
            ),
            (
                [count(3, order).as_slice(), &[0, 65, 65]].concat(),
                "missing archive string terminator",
            ),
        ] {
            let bytes = body(0, 0, &token, order);
            assert_eq!(
                error(&bytes, order),
                format!("invalid initial control token frame: {expected}")
            );
        }
        let mut token = wide(&[0xffff, 0], order);
        // Missing terminator dominates both unsupported content conditions.
        *token.last_mut().unwrap() = 1;
        let bytes = body(0, 0, &token, order);
        assert_eq!(
            error(&bytes, order),
            "invalid initial control token frame: missing archive string terminator"
        );
    }
}

// Sender-order entry point: rule inferred from the receive paths (F-519).

fn is_little(order: ByteOrder) -> bool {
    matches!(order, ByteOrder::Little)
}

fn sender_error(bytes: &[u8]) -> String {
    format!(
        "{:#}",
        read_initial_control_body_with_sender_order(bytes)
            .err()
            .unwrap()
    )
}

fn assert_same_fields(left: &InitialControlBody<'_>, right: &InitialControlBody<'_>) {
    assert_eq!(left.body().as_ptr(), right.body().as_ptr());
    assert_eq!(left.body(), right.body());
    assert_eq!(left.first_field(), right.first_field());
    assert_eq!(left.remote_version(), right.remote_version());
    assert_eq!(left.remote_version_bytes(), right.remote_version_bytes());
    assert_eq!(left.token_frame(), right.token_frame());
    let (left, right) = (left.summary(), right.summary());
    assert_eq!(format!("{left:?}"), format!("{right:?}"));
    assert_eq!(
        serde_json::to_string(&left).unwrap(),
        serde_json::to_string(&right).unwrap()
    );
}

#[test]
fn sender_order_maps_only_first_field_one_to_little_endian() {
    let little: Vec<u8> = (0..=u8::MAX)
        .filter(|&first| is_little(sender_byte_order(first)))
        .collect();
    assert_eq!(little, [1]);
}

#[test]
fn sender_order_first_field_one_decodes_little_endian_version() {
    let token = [count(4, ByteOrder::Little).as_slice(), b"LAB\0"].concat();
    let bytes = body(1, 0x0102_0304, &token, ByteOrder::Little);
    assert_eq!(&bytes[1..5], [4, 3, 2, 1]);
    let parsed = read_initial_control_body_with_sender_order(&bytes).unwrap();
    assert_eq!(parsed.first_field(), 1);
    assert_eq!(parsed.remote_version(), 0x0102_0304);
    assert_eq!(parsed.token_frame(), token);
    let summary = parsed.summary();
    assert_eq!(summary.token_encoding, TokenEncoding::Narrow);
    assert_eq!(summary.token_units, 3);
    assert!(!summary.negotiation_validated);
    assert!(!summary.identity_verified);
    assert!(!summary.game_connection_established);
}

#[test]
fn sender_order_other_first_fields_decode_big_endian_version() {
    for first in [0, 2, 0x7f, 0xff] {
        let token = wide(&[65, 0xd83d, 0xde00], ByteOrder::Big);
        let bytes = body(first, 0x0102_0304, &token, ByteOrder::Big);
        assert_eq!(&bytes[1..5], [1, 2, 3, 4]);
        let parsed = read_initial_control_body_with_sender_order(&bytes).unwrap();
        assert_eq!(parsed.first_field(), first);
        assert_eq!(parsed.remote_version(), 0x0102_0304);
        assert_eq!(parsed.summary().token_encoding, TokenEncoding::Wide);
        assert_eq!(parsed.summary().token_units, 3);
        assert!(!parsed.summary().negotiation_validated);
    }
}

#[test]
fn sender_order_agrees_with_matching_explicit_order_and_not_with_the_other() {
    for first in 0..=u8::MAX {
        let derived = sender_byte_order(first);
        let other = if is_little(derived) {
            ByteOrder::Big
        } else {
            ByteOrder::Little
        };
        // Empty tokens are order-independent, so the mismatched order parses too.
        let bytes = body(first, 0x0102_0304, &count(0, derived), derived);
        let sender = read_initial_control_body_with_sender_order(&bytes).unwrap();
        let explicit = read_initial_control_body(&bytes, derived).unwrap();
        assert_same_fields(&sender, &explicit);
        let mismatched = read_initial_control_body(&bytes, other).unwrap();
        assert_eq!(sender.remote_version(), 0x0102_0304);
        assert_eq!(mismatched.remote_version(), 0x0403_0201);
        assert_eq!(mismatched.token_frame(), sender.token_frame());
    }
}

#[test]
fn sender_order_rejections_match_the_explicit_reader_exactly() {
    for first in [0, 1, 2, 0xff] {
        let order = sender_byte_order(first);
        let mut cases = vec![Vec::new(), vec![0; MAX_INITIAL_CONTROL_BODY_BYTES + 1]];
        for token in [
            count(0, order).to_vec(),
            [count(4, order).as_slice(), b"LAB\0"].concat(),
            wide(&[65, 0xd83d, 0xde00], order),
            count(i32::MIN, order).to_vec(),
            count(2, order).to_vec(),
            [count(2, order).as_slice(), b"AB"].concat(),
            wide(&[0xffff], order),
        ] {
            let bytes = body(first, 0x0102_0304, &token, order);
            for length in 0..bytes.len() {
                cases.push(bytes[..length].to_vec());
            }
            cases.push([bytes.as_slice(), &[0]].concat());
            cases.push(bytes);
        }
        let mut limit = wide(&vec![0x1234; MAX_STRING_UNITS - 1], order);
        limit = body(first, 0, &limit, order);
        assert_eq!(limit.len(), MAX_INITIAL_CONTROL_BODY_BYTES);
        cases.push([limit.as_slice(), &[0]].concat());
        cases.push(limit);
        for case in cases {
            let sender = read_initial_control_body_with_sender_order(&case);
            // Empty input fails before any order is used; either explicit order agrees.
            let explicit = read_initial_control_body(&case, order);
            match (sender, explicit) {
                (Ok(sender), Ok(explicit)) => assert_same_fields(&sender, &explicit),
                (Err(sender), Err(explicit)) => {
                    assert_eq!(format!("{sender:#}"), format!("{explicit:#}"));
                }
                _ => panic!("outcomes differ for {} bytes", case.len()),
            }
        }
        assert!(sender_error(&[]).contains("first field"));
        assert!(sender_error(&[first; 4]).contains("remote version"));
        assert!(
            sender_error(&vec![first; MAX_INITIAL_CONTROL_BODY_BYTES + 1])
                .contains("research byte limit")
        );
        let complete = body(first, 0, &count(0, order), order);
        let excess = [complete.as_slice(), &[0]].concat();
        assert!(sender_error(&excess).contains("trailing data"));
    }
}

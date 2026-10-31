use icarus_bevy::experimental::archive::ByteOrder;
use icarus_bevy::experimental::steam_auth::{
    MAX_DATAGRAM_BYTES, MAX_MESSAGE_BYTES, MAX_TICKET_UNITS, SteamAuthMessage, decode_datagram,
    decode_message, encode_datagram, encode_message,
};

const ORDERS: [ByteOrder; 2] = [ByteOrder::Little, ByteOrder::Big];
// Independently authored identity; no Steam identity validity is assumed.
const ID: u64 = 0x0123_4567_89ab_cdef;

fn assert_fixture(message: &SteamAuthMessage, order: ByteOrder, bytes: &[u8]) {
    assert_eq!(encode_message(message, order).unwrap(), bytes);
    assert_eq!(&decode_message(bytes, order).unwrap(), message);
    // Every proper prefix is incomplete, including a missing final ID byte.
    for length in 0..bytes.len() {
        assert!(decode_message(&bytes[..length], order).is_err());
    }
    let mut trailing = bytes.to_vec();
    trailing.push(0);
    assert!(decode_message(&trailing, order).is_err());
}

#[test]
fn independent_narrow_user_data_fixtures_order_count_and_id() {
    let message = SteamAuthMessage::UserData {
        ticket: vec![65, 55, 70],
        steam_id: ID,
    };
    assert_fixture(
        &message,
        ByteOrder::Little,
        &[
            1, 4, 0, 0, 0, b'A', b'7', b'F', 0, 0xef, 0xcd, 0xab, 0x89, 0x67, 0x45, 0x23, 0x01,
        ],
    );
    assert_fixture(
        &message,
        ByteOrder::Big,
        &[
            1, 0, 0, 0, 4, b'A', b'7', b'F', 0, 0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef,
        ],
    );
}

#[test]
fn independent_wide_user_data_fixtures_preserve_utf16_units() {
    let message = SteamAuthMessage::UserData {
        ticket: vec![65, 0xd83d, 0xde00],
        steam_id: ID,
    };
    // Signed count -4 includes the terminator; A and a surrogate pair follow.
    assert_fixture(
        &message,
        ByteOrder::Little,
        &[
            1, 0xfc, 0xff, 0xff, 0xff, 65, 0, 0x3d, 0xd8, 0, 0xde, 0, 0, 0xef, 0xcd, 0xab, 0x89,
            0x67, 0x45, 0x23, 0x01,
        ],
    );
    assert_fixture(
        &message,
        ByteOrder::Big,
        &[
            1, 0xff, 0xff, 0xff, 0xfc, 0, 65, 0xd8, 0x3d, 0xde, 0, 0, 0, 0x01, 0x23, 0x45, 0x67,
            0x89, 0xab, 0xcd, 0xef,
        ],
    );
}

#[test]
fn independent_result_and_resend_fixtures_have_fixed_storage() {
    for order in ORDERS {
        assert_fixture(
            &SteamAuthMessage::Result { accepted: false },
            order,
            &[2, 0, 0, 0, 0],
        );
        // The Boolean stays little-endian even with a big-endian archive.
        assert_fixture(
            &SteamAuthMessage::Result { accepted: true },
            order,
            &[2, 1, 0, 0, 0],
        );
        assert_fixture(&SteamAuthMessage::RequestResendKey, order, &[3]);
        assert_fixture(&SteamAuthMessage::RequestResendResult, order, &[4]);
    }
}

#[test]
fn empty_ticket_zero_count_fixture_and_terminated_forms_are_accepted() {
    let empty = SteamAuthMessage::UserData {
        ticket: vec![],
        steam_id: 0,
    };
    for order in ORDERS {
        assert_fixture(&empty, order, &[1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        let terminated: [&[u8]; 2] = match order {
            ByteOrder::Little => [&[1, 0, 0, 0, 0], &[0xff, 0xff, 0xff, 0xff, 0, 0]],
            ByteOrder::Big => [&[0, 0, 0, 1, 0], &[0xff, 0xff, 0xff, 0xff, 0, 0]],
        };
        for string in terminated {
            let bytes = [&[1][..], string, &[0; 8]].concat();
            assert_eq!(decode_message(&bytes, order).unwrap(), empty);
        }
    }
}

#[test]
fn round_trips_preserve_raw_units_and_do_not_validate_identity_or_hex() {
    for order in ORDERS {
        for ticket in [
            vec![],
            vec![65, 0x7f],
            "not a hexadecimal ticket!".encode_utf16().collect(),
            vec![0xd800, 65, 0xdc00, 0xdfff],
            vec![65; MAX_TICKET_UNITS],
            vec![0x1234; MAX_TICKET_UNITS],
        ] {
            for steam_id in [0, ID, u64::MAX] {
                let message = SteamAuthMessage::UserData {
                    ticket: ticket.clone(),
                    steam_id,
                };
                let encoded = encode_message(&message, order).unwrap();
                assert!(encoded.len() <= MAX_MESSAGE_BYTES);
                assert_eq!(decode_message(&encoded, order).unwrap(), message);
            }
        }
    }
}

#[test]
fn maximum_wide_ticket_fills_exact_authored_byte_bound() {
    assert_eq!(MAX_TICKET_UNITS, 1_024);
    assert_eq!(MAX_MESSAGE_BYTES, 2_063);
    for order in ORDERS {
        let message = SteamAuthMessage::UserData {
            ticket: vec![0x1234; MAX_TICKET_UNITS],
            steam_id: 0,
        };
        // Independent fixture: -1025, 1024 raw units, terminator and u64 zero.
        let (count, unit) = match order {
            ByteOrder::Little => ([0xff, 0xfb, 0xff, 0xff], [0x34, 0x12]),
            ByteOrder::Big => ([0xff, 0xff, 0xfb, 0xff], [0x12, 0x34]),
        };
        let mut fixture = vec![1];
        fixture.extend_from_slice(&count);
        for _ in 0..1_024 {
            fixture.extend_from_slice(&unit);
        }
        fixture.extend_from_slice(&[0; 10]);
        assert_eq!(fixture.len(), MAX_MESSAGE_BYTES);
        assert_fixture(&message, order, &fixture);
    }
}

#[test]
fn oversized_signed_counts_are_rejected_before_payload_or_allocation() {
    for order in ORDERS {
        for count in [1_026_i32, -1_026, 65_536, -65_536, i32::MIN, i32::MAX] {
            let prefix = match order {
                ByteOrder::Little => count.to_le_bytes(),
                ByteOrder::Big => count.to_be_bytes(),
            };
            // No declared payload is supplied. A policy error rather than a
            // truncation error confirms the count is checked first.
            let bytes = [&[1][..], &prefix].concat();
            assert_eq!(
                decode_message(&bytes, order).unwrap_err().to_string(),
                "Steam-auth ticket exceeds authored unit limit"
            );
        }
        for count in [1_025_i32, -1_025] {
            let prefix = match order {
                ByteOrder::Little => count.to_le_bytes(),
                ByteOrder::Big => count.to_be_bytes(),
            };
            let bytes = [&[1][..], &prefix].concat();
            // These counts are within policy, but their payloads are missing.
            assert_eq!(
                decode_message(&bytes, order).unwrap_err().to_string(),
                "invalid strict Steam-auth ticket"
            );
        }
        // A complete oversized narrow string still fits the byte bound. It must
        // be rejected by the unit policy, independently of message byte length.
        let prefix = match order {
            ByteOrder::Little => 1_026_i32.to_le_bytes(),
            ByteOrder::Big => 1_026_i32.to_be_bytes(),
        };
        let mut oversized = vec![1];
        oversized.extend_from_slice(&prefix);
        oversized.extend_from_slice(&[65; MAX_TICKET_UNITS + 1]);
        oversized.extend_from_slice(&[0; 9]);
        assert!(oversized.len() < MAX_MESSAGE_BYTES);
        assert_eq!(
            decode_message(&oversized, order).unwrap_err().to_string(),
            "Steam-auth ticket exceeds authored unit limit"
        );
    }
}

#[test]
fn byte_limit_precedes_dispatch_and_invalid_encoding_never_produces_output() {
    for order in ORDERS {
        for kind in [0, 1, 2, 3, 4, 255] {
            let mut bytes = vec![0; MAX_MESSAGE_BYTES + 1];
            bytes[0] = kind;
            assert_eq!(
                decode_message(&bytes, order).unwrap_err().to_string(),
                "Steam-auth body exceeds authored byte limit"
            );
        }
        for ticket in [
            vec![65; MAX_TICKET_UNITS + 1],
            vec![0x1234; MAX_TICKET_UNITS + 1],
            vec![0],
            vec![0xffff],
            vec![65, 0, 66],
        ] {
            assert!(
                encode_message(
                    &SteamAuthMessage::UserData {
                        ticket,
                        steam_id: ID,
                    },
                    order,
                )
                .is_err()
            );
        }
    }
}

#[test]
fn malformed_strict_strings_are_rejected_in_both_orders() {
    for order in ORDERS {
        let strings: [&[u8]; 6] = match order {
            ByteOrder::Little => [
                &[2, 0, 0, 0, 65, 66],                        // Missing terminator.
                &[3, 0, 0, 0, 0, 66, 0],                      // Embedded NUL.
                &[2, 0, 0, 0, 0x80, 0],                       // Non-ASCII narrow.
                &[0xfe, 0xff, 0xff, 0xff, 0xff, 0xff, 0, 0],  // U+FFFF.
                &[0xfd, 0xff, 0xff, 0xff, 0, 0, 65, 0, 0, 0], // Wide embedded NUL.
                &[0xfe, 0xff, 0xff, 0xff, 65, 0, 1, 0],       // Wide terminator.
            ],
            ByteOrder::Big => [
                &[0, 0, 0, 2, 65, 66],
                &[0, 0, 0, 3, 0, 66, 0],
                &[0, 0, 0, 2, 0x80, 0],
                &[0xff, 0xff, 0xff, 0xfe, 0xff, 0xff, 0, 0],
                &[0xff, 0xff, 0xff, 0xfd, 0, 0, 0, 65, 0, 0],
                &[0xff, 0xff, 0xff, 0xfe, 0, 65, 0, 1],
            ],
        };
        for string in strings {
            let bytes = [&[1][..], string, &[0; 8]].concat();
            assert_eq!(
                decode_message(&bytes, order).unwrap_err().to_string(),
                "invalid strict Steam-auth ticket"
            );
        }
    }
}

#[test]
fn unknown_types_and_noncanonical_booleans_are_rejected() {
    for order in ORDERS {
        for kind in 0_u8..=255 {
            if !(1..=4).contains(&kind) {
                assert!(decode_message(&[kind], order).is_err());
            }
        }
        for storage in [
            [2, 0, 0, 0],
            [0, 1, 0, 0],
            [0, 0, 1, 0],
            [0, 0, 0, 1], // Big-endian true is not Windows raw true.
            [0xff; 4],
        ] {
            let bytes = [&[2][..], &storage].concat();
            assert_eq!(
                decode_message(&bytes, order).unwrap_err().to_string(),
                "noncanonical Steam-auth Boolean"
            );
        }
    }
}

#[test]
fn debug_and_all_error_formats_redact_ticket_and_id() {
    let secret = "private-auth-ticket-DO-NOT-LOG";
    let ticket: Vec<u16> = secret.encode_utf16().collect();
    let message = SteamAuthMessage::UserData {
        ticket: ticket.clone(),
        steam_id: ID,
    };
    let debug = format!("{message:?}");
    assert_eq!(
        debug,
        "UserData { ticket: \"[REDACTED]\", steam_id: \"[REDACTED]\" }"
    );
    let assert_redacted = |rendered: &str| {
        for sensitive in [
            secret.to_owned(),
            format!("{ticket:?}"),
            ID.to_string(),
            format!("{ID:x}"),
            format!("{:?}", ID.to_le_bytes()),
            format!("{:?}", ID.to_be_bytes()),
        ] {
            assert!(!rendered.contains(&sensitive));
        }
    };
    assert_redacted(&debug);
    assert_redacted(&format!("{message:#?}"));
    for order in ORDERS {
        let mut malformed = encode_message(&message, order).unwrap();
        // Replace the narrow terminator, retaining ticket and ID in the input.
        malformed[5 + ticket.len()] = 65;
        let mut trailing = encode_message(&message, order).unwrap();
        trailing.push(0);
        let mut invalid_ticket = ticket.clone();
        invalid_ticket.push(0xffff);
        let invalid = SteamAuthMessage::UserData {
            ticket: invalid_ticket,
            steam_id: ID,
        };
        for error in [
            decode_message(&malformed, order).unwrap_err(),
            decode_message(&trailing, order).unwrap_err(),
            encode_message(&invalid, order).unwrap_err(),
        ] {
            for rendered in [
                error.to_string(),
                format!("{error:#}"),
                format!("{error:?}"),
                format!("{error:#?}"),
            ] {
                assert_redacted(&rendered);
            }
        }
    }
}

// Independent bit placement: no production bit writer, prefix or termination
// helper. The body is byte-serialized but begins at an unaligned wire bit.
fn authored_datagram(body: &[u8], magic_bits: u32, magic: u32, extra: &[bool]) -> Vec<u8> {
    let mut bits: Vec<bool> = (0..magic_bits).map(|bit| magic & (1 << bit) != 0).collect();
    bits.extend([false, true]);
    for byte in body {
        bits.extend((0..8).map(|bit| byte & (1 << bit) != 0));
    }
    bits.extend_from_slice(extra);
    bits.push(true);
    let mut bytes = vec![0; bits.len().div_ceil(8)];
    for (index, bit) in bits.into_iter().enumerate() {
        bytes[index / 8] |= u8::from(bit) << (index % 8);
    }
    bytes
}

#[test]
fn literal_datagrams_have_two_prefix_bits_and_one_handler_termination() {
    for (message, bytes) in [
        (SteamAuthMessage::RequestResendKey, vec![0x0e, 0x04]),
        (SteamAuthMessage::RequestResendResult, vec![0x12, 0x04]),
        (
            SteamAuthMessage::Result { accepted: false },
            vec![0x0a, 0, 0, 0, 0, 4],
        ),
        (
            SteamAuthMessage::Result { accepted: true },
            vec![0x0a, 4, 0, 0, 0, 4],
        ),
    ] {
        for order in ORDERS {
            assert_eq!(encode_datagram(&message, order, 0, 0).unwrap(), bytes);
            assert_eq!(decode_datagram(&bytes, order, 0, 0).unwrap(), message);
        }
    }
}

#[test]
fn authored_datagrams_cover_all_messages_magic_alignment_and_truncation() {
    for order in ORDERS {
        let body = match order {
            ByteOrder::Little => vec![
                1, 2, 0, 0, 0, b'A', 0, 0xef, 0xcd, 0xab, 0x89, 0x67, 0x45, 0x23, 1,
            ],
            ByteOrder::Big => vec![
                1, 0, 0, 0, 2, b'A', 0, 1, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef,
            ],
        };
        let fixtures = [
            (
                SteamAuthMessage::UserData {
                    ticket: vec![65],
                    steam_id: ID,
                },
                body,
            ),
            (
                SteamAuthMessage::Result { accepted: false },
                vec![2, 0, 0, 0, 0],
            ),
            (
                SteamAuthMessage::Result { accepted: true },
                vec![2, 1, 0, 0, 0],
            ),
            (SteamAuthMessage::RequestResendKey, vec![3]),
            (SteamAuthMessage::RequestResendResult, vec![4]),
        ];
        for (magic_bits, magic) in [
            (0, 0),
            (1, 1),
            (6, 0x2a),
            (7, 0x55),
            (8, 0xa5),
            (31, 0x1234_5678),
            (32, u32::MAX),
        ] {
            for (message, body) in &fixtures {
                let bytes = authored_datagram(body, magic_bits, magic, &[]);
                assert_eq!(
                    encode_datagram(message, order, magic_bits, magic).unwrap(),
                    bytes
                );
                assert_eq!(
                    &decode_datagram(&bytes, order, magic_bits, magic).unwrap(),
                    message
                );
                for length in 0..bytes.len() {
                    assert!(decode_datagram(&bytes[..length], order, magic_bits, magic).is_err());
                }
                for extra in [&[false][..], &[true][..], &[false; 8][..], &[true; 8][..]] {
                    let bytes = authored_datagram(body, magic_bits, magic, extra);
                    assert!(decode_datagram(&bytes, order, magic_bits, magic).is_err());
                }
            }
        }
    }
}

#[test]
fn datagrams_reject_bad_prefix_magic_termination_and_oversize() {
    let order = ByteOrder::Little;
    for bytes in [
        &[][..],
        &[0][..],
        &[2][..],
        &[6][..],
        &[0x0e, 0][..],
        &[0x0e, 4, 0][..],
    ] {
        assert!(decode_datagram(bytes, order, 0, 0).is_err());
    }
    // A stateless packet, data packet and both set must not enter this codec.
    for prefix in [0, 1, 3] {
        assert!(decode_datagram(&[0x0c | prefix, 4], order, 0, 0).is_err());
    }
    let bytes = authored_datagram(&[3], 7, 0x55, &[]);
    assert!(decode_datagram(&bytes, order, 7, 0x54).is_err());
    for (bits, magic) in [(33, 0), (0, 1), (7, 0x80)] {
        assert!(encode_datagram(&SteamAuthMessage::RequestResendKey, order, bits, magic).is_err());
        assert!(decode_datagram(&bytes, order, bits, magic).is_err());
    }
    assert_eq!(MAX_DATAGRAM_BYTES, 2_068);
    assert_eq!(
        decode_datagram(&vec![0; MAX_DATAGRAM_BYTES + 1], order, 32, 0)
            .unwrap_err()
            .to_string(),
        "Steam-auth datagram exceeds authored byte limit"
    );
    // The bound is also narrowed for the selected magic width.
    assert_eq!(
        decode_datagram(&vec![0; MAX_MESSAGE_BYTES + 2], order, 0, 0)
            .unwrap_err()
            .to_string(),
        "Steam-auth datagram exceeds authored byte limit"
    );
    let maximum = SteamAuthMessage::UserData {
        ticket: vec![0x1234; MAX_TICKET_UNITS],
        steam_id: ID,
    };
    for order in ORDERS {
        let bytes = encode_datagram(&maximum, order, 32, u32::MAX).unwrap();
        assert_eq!(bytes.len(), MAX_DATAGRAM_BYTES);
        assert_eq!(
            decode_datagram(&bytes, order, 32, u32::MAX).unwrap(),
            maximum
        );
    }
}

#[test]
fn malformed_datagram_errors_do_not_print_ticket_or_identity() {
    let secret = "private-datagram-ticket-DO-NOT-LOG";
    let message = SteamAuthMessage::UserData {
        ticket: secret.encode_utf16().collect(),
        steam_id: ID,
    };
    let mut body = encode_message(&message, ByteOrder::Little).unwrap();
    body.push(0);
    let bytes = authored_datagram(&body, 0, 0, &[]);
    let error = decode_datagram(&bytes, ByteOrder::Little, 0, 0).unwrap_err();
    for rendered in [
        error.to_string(),
        format!("{error:#}"),
        format!("{error:?}"),
        format!("{error:#?}"),
    ] {
        for sensitive in [
            secret.to_owned(),
            ID.to_string(),
            format!("{ID:x}"),
            format!("{bytes:?}"),
        ] {
            assert!(!rendered.contains(&sensitive));
        }
    }
}

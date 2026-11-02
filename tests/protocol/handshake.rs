use icarus_bevy::experimental::handshake::{
    HandshakeFrame, LegacyHandshake, RESTART_REQUEST_FRAME, RestartChallengeResponse, decode,
    decode_frame, encode, encode_restart_response, initial_request, parse_magic_header_bits,
};

fn authored_challenge() -> Vec<u8> {
    // Python oracle: little-endian f64(123.5), cookie bytes 0..19, fields packed
    // into an integer at bit 3, secret bit set and termination at bit 227.
    let text = "050000000000f702020810182028303840485058606870788088909808";
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap())
        .collect()
}

#[test]
fn authored_unaligned_challenge_decodes_exact_fields() {
    let parsed = decode(&authored_challenge()).unwrap();
    assert_eq!(parsed.timestamp, 123.5);
    assert!(parsed.secret_id);
    assert!(!parsed.restart);
    assert_eq!(parsed.cookie, std::array::from_fn(|i| i as u8));
    assert_eq!(
        decode_frame(&authored_challenge()).unwrap(),
        HandshakeFrame::Standard(parsed)
    );
}

#[test]
fn encoded_fields_match_independent_fixture_and_reject_nonfinite_time() {
    let mut packet = LegacyHandshake {
        restart: false,
        secret_id: true,
        timestamp: 123.5,
        cookie: std::array::from_fn(|i| i as u8),
    };
    assert_eq!(encode(&packet).unwrap().as_slice(), authored_challenge());
    for value in [f64::INFINITY, f64::NEG_INFINITY, f64::NAN] {
        packet.timestamp = value;
        assert!(encode(&packet).is_err());
    }
}

#[test]
fn initial_request_has_zero_timestamp_and_cookie() {
    let bytes = initial_request();
    assert_eq!(bytes.iter().map(|b| b.count_ones()).sum::<u32>(), 2);
    let parsed = decode(&bytes).unwrap();
    assert_eq!(parsed.timestamp, 0.0);
    assert_eq!(parsed.cookie, [0; 20]);
    assert!(!parsed.restart && !parsed.secret_id);
}

#[test]
fn truncated_extended_and_malformed_frames_fail() {
    let frame = authored_challenge();
    for length in 0..frame.len() {
        assert!(decode(&frame[..length]).is_err());
    }
    assert!(decode(&[frame.as_slice(), &[0]].concat()).is_err());
    for (offset, mask) in [(0, 1), (28, 8), (28, 16)] {
        let mut altered = frame.clone();
        altered[offset] ^= mask;
        assert!(decode(&altered).is_err());
    }
}

#[test]
fn restart_request_and_extended_49_byte_response_round_trip_and_validate() {
    // 1-byte server-to-client restart request (3 bits: marker=1, restart=1, terminator=1).
    assert_eq!(
        decode_frame(&RESTART_REQUEST_FRAME).unwrap(),
        HandshakeFrame::RestartRequest
    );
    for invalid in [0x00u8, 0x01, 0x03, 0x05, 0x0f, 0x87] {
        assert!(decode_frame(&[invalid]).is_err());
    }

    let response = RestartChallengeResponse {
        secret_id: true,
        timestamp: 123.5,
        cookie: std::array::from_fn(|i| i as u8),
        orig_cookie: std::array::from_fn(|i| (0xa0 + i) as u8),
    };
    let encoded = encode_restart_response(&response).unwrap();
    assert_eq!(encoded.len(), 49);
    assert_eq!(encoded[0] & 0x07, 0b111);
    assert_eq!(encoded[48] & 0xf8, 0x08);
    assert_eq!(
        decode_frame(&encoded).unwrap(),
        HandshakeFrame::RestartResponse(response)
    );

    // Clear restart bit -> rejected for 49-byte frame.
    let mut missing_restart = encoded;
    missing_restart[0] &= !0x02;
    assert!(decode_frame(&missing_restart).is_err());

    // Alter termination or padding bits in byte 48 -> rejected.
    for mask in [0x08u8, 0x10, 0x80] {
        let mut bad_term = encoded;
        bad_term[48] ^= mask;
        assert!(decode_frame(&bad_term).is_err());
    }
}

#[test]
fn magic_header_cvar_parser_enforces_binary_and_32_bit_ceiling() {
    assert!(parse_magic_header_bits("").is_empty());
    assert_eq!(
        parse_magic_header_bits("00010101"),
        vec![false, false, false, true, false, true, false, true]
    );
    assert_eq!(parse_magic_header_bits(&"10".repeat(16)).len(), 32);
    // > 32 bits resets to 0 bits (`0x14372d5d0` warning `0x1453e7d10`).
    assert!(parse_magic_header_bits(&("1".repeat(33))).is_empty());
    // Any non-'0'/'1' character resets to 0 bits (`0x14372d5d0` warning `0x1453e7c30`).
    assert!(parse_magic_header_bits("010201").is_empty());
}

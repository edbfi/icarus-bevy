use icarus_bevy::experimental::handshake::{LegacyHandshake, decode, encode, initial_request};

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

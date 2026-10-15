use icarus_bevy::experimental::archive::{ByteOrder, encode_string};
use icarus_bevy::experimental::control::TokenEncoding;
use icarus_bevy::experimental::control_receive::{
    ControlDisposition, ControlMessageSummary, ControlReceiveConfig, NameKind,
    read_control_datagram,
};
use icarus_bevy::experimental::handshake::initial_request;
use icarus_bevy::experimental::packet::{
    BitWriter, Bunch, BunchHeader, BunchLimits, HandshakePacket, Name, PacketHeader, Payload,
    Receiver, finish_data_packet, write_bunch, write_data_prefix, write_packet_header,
};

// Synthetic datagrams built with the experimental codec writers; no captures.

const LIMITS: BunchLimits = BunchLimits {
    max_channels: 16,
    max_packet_bytes: 1024,
    max_string_units: 64,
};

fn config() -> ControlReceiveConfig {
    ControlReceiveConfig::new(LIMITS)
}

fn initial_body(first: u8, version: u32, token: &[u16]) -> Vec<u8> {
    let order = if first == 1 {
        ByteOrder::Little
    } else {
        ByteOrder::Big
    };
    let version = match order {
        ByteOrder::Little => version.to_le_bytes(),
        ByteOrder::Big => version.to_be_bytes(),
    };
    [
        &[0, first][..],
        &version,
        &encode_string(token, order).unwrap(),
    ]
    .concat()
}

fn bits(bytes: &[u8], extra_bits: u32) -> Payload {
    let mut writer = BitWriter::new();
    writer.write_bytes(bytes);
    writer.write_bits(0, extra_bits);
    Payload::from_writer(writer)
}

fn header() -> BunchHeader {
    BunchHeader {
        control: false,
        open: false,
        close: false,
        close_reason: None,
        paused: false,
        reliable: false,
        channel_index: 0,
        has_package_map_exports: false,
        has_must_be_mapped_guids: false,
        partial: false,
        reliable_sequence_raw: None,
        partial_initial: false,
        partial_final: false,
        channel_name: None,
    }
}

/// Open, reliable bunch named with hard-coded index 255.
fn control_bunch(channel_index: u32, payload: Payload) -> Bunch {
    let mut header = header();
    (header.control, header.open, header.reliable) = (true, true, true);
    header.reliable_sequence_raw = Some(1);
    header.channel_index = channel_index;
    header.channel_name = Some(Name::Hardcoded(255));
    Bunch { header, payload }
}

fn datagram(bunches: &[Bunch]) -> Vec<u8> {
    let mut writer = BitWriter::new();
    write_data_prefix(&mut writer, 0, 0).unwrap();
    let header = PacketHeader {
        sequence: 100,
        acked_sequence: 99,
        history: vec![0],
    };
    write_packet_header(&mut writer, &header, None, Receiver::Server).unwrap();
    for bunch in bunches {
        write_bunch(&mut writer, bunch, &LIMITS).unwrap();
    }
    finish_data_packet(writer)
}

fn messages(disposition: &ControlDisposition) -> &[ControlMessageSummary] {
    match disposition {
        ControlDisposition::Messages { messages } => messages,
        other => panic!("not decoded: {other:?}"),
    }
}

fn error(datagram: &[u8]) -> String {
    format!(
        "{:#}",
        read_control_datagram(datagram, &config()).err().unwrap()
    )
}

#[test]
fn first_packet_open_reliable_control_bunch_decodes_little_endian_initial_message() {
    for (token, encoding, units) in [
        (vec![], TokenEncoding::Empty, 0),
        ("LAB".encode_utf16().collect(), TokenEncoding::Narrow, 3),
    ] {
        let body = initial_body(1, 0x0102_0304, &token);
        assert_eq!(&body[2..6], [4, 3, 2, 1]);
        let received =
            read_control_datagram(&datagram(&[control_bunch(0, bits(&body, 0))]), &config())
                .unwrap();
        let summary = received.summary();
        assert_eq!((summary.sequence, summary.acked_sequence), (100, 99));
        assert_eq!(summary.bunch_count, 1);
        let bunch = &summary.bunches[0];
        assert!(bunch.open && bunch.reliable && !bunch.close && !bunch.partial);
        assert_eq!(bunch.name, NameKind::Hardcoded { index: 255 });
        assert_eq!(bunch.payload_bits, body.len() * 8);
        let [ControlMessageSummary::Initial(initial)] = messages(&bunch.control) else {
            panic!("expected one initial message");
        };
        assert_eq!(initial.token_encoding, encoding);
        assert_eq!(initial.token_units, units);
        assert_eq!(initial.body_bytes, body.len() - 1);
        assert!(!initial.negotiation_validated && !initial.identity_verified);
        assert!(!summary.negotiation_validated);
        assert!(!summary.identity_verified);
        assert!(!summary.game_connection_established);
        let bodies: Vec<_> = received.initial_bodies().collect();
        assert_eq!(bodies.len(), 1);
        assert_eq!(bodies[0].first_field(), 1);
        assert_eq!(bodies[0].remote_version(), 0x0102_0304);
        assert_eq!(bodies[0].body(), &body[1..]);
    }
}

#[test]
fn other_first_fields_decode_big_endian_versions() {
    for first in [0, 2, 0xff] {
        let body = initial_body(first, 0x0102_0304, &[0x41, 0x263a]);
        assert_eq!(&body[2..6], [1, 2, 3, 4]);
        let received =
            read_control_datagram(&datagram(&[control_bunch(0, bits(&body, 0))]), &config())
                .unwrap();
        let body = received.initial_bodies().next().unwrap();
        assert_eq!(body.first_field(), first);
        assert_eq!(body.remote_version(), 0x0102_0304);
        assert_eq!(body.summary().token_encoding, TokenEncoding::Wide);
    }
}

#[test]
fn unknown_second_message_is_reported_without_interpretation() {
    let first = initial_body(1, 7, &"A".encode_utf16().collect::<Vec<_>>());
    let second = initial_body(0, 8, &[]);
    let payload = [first.as_slice(), &[5, 1, 2, 3], &second].concat();
    let received =
        read_control_datagram(&datagram(&[control_bunch(0, bits(&payload, 0))]), &config())
            .unwrap();
    let [
        ControlMessageSummary::Initial(_),
        ControlMessageSummary::Undecoded { id, remaining_bits },
    ] = messages(&received.summary().bunches[0].control)
    else {
        panic!("expected initial then undecoded");
    };
    assert_eq!(*id, 5);
    assert_eq!(*remaining_bits, (3 + second.len()) * 8);
    // A later ID 0 after an unknown ID is not decoded.
    assert_eq!(received.initial_bodies().count(), 1);
    // Two consecutive ID 0 messages both decode.
    let payload = [first.as_slice(), &second].concat();
    let received =
        read_control_datagram(&datagram(&[control_bunch(0, bits(&payload, 0))]), &config())
            .unwrap();
    let versions: Vec<_> = received
        .initial_bodies()
        .map(|body| body.remote_version())
        .collect();
    assert_eq!(versions, [7, 8]);
}

#[test]
fn non_control_bunches_are_listed_but_not_inspected() {
    let body = initial_body(1, 3, &[]);
    let mut actor = header();
    actor.channel_index = 3;
    let actor = Bunch {
        header: actor,
        // Would be an invalid ID 0 body if it were inspected.
        payload: bits(&[0, 1, 2], 5),
    };
    let mut string_named = control_bunch(4, bits(&[0], 0));
    string_named.header.channel_name = Some(Name::string(&[0x43], 2).unwrap());
    // Channel index 0 alone selects the control channel, without open or name.
    let mut unnamed = header();
    unnamed.paused = true;
    let by_index = Bunch {
        header: unnamed,
        payload: bits(&body, 0),
    };
    // The hard-coded name selects a control bunch on another channel index.
    let by_name = control_bunch(6, bits(&body, 0));
    let received = read_control_datagram(
        &datagram(&[actor, string_named, by_index, by_name]),
        &config(),
    )
    .unwrap();
    let summary = received.summary();
    assert_eq!(summary.bunch_count, 4);
    let indices: Vec<_> = summary.bunches.iter().map(|b| b.channel_index).collect();
    assert_eq!(indices, [3, 4, 0, 6]);
    assert!(matches!(
        summary.bunches[0].control,
        ControlDisposition::NotControl
    ));
    assert_eq!(summary.bunches[0].name, NameKind::Absent);
    assert_eq!(summary.bunches[0].payload_bits, 29);
    assert!(matches!(
        summary.bunches[1].control,
        ControlDisposition::NotControl
    ));
    assert_eq!(summary.bunches[1].name, NameKind::String);
    assert_eq!(messages(&summary.bunches[2].control).len(), 1);
    assert_eq!(messages(&summary.bunches[3].control).len(), 1);
    assert_eq!(received.initial_bodies().count(), 2);

    // A caller-supplied control channel index replaces the default.
    let mut custom = config();
    custom.control_channel_index = 3;
    let mut actor = header();
    actor.channel_index = 3;
    let mut zero = header();
    zero.channel_index = 0;
    let datagram = datagram(&[
        Bunch {
            header: actor,
            payload: bits(&body, 0),
        },
        Bunch {
            header: zero,
            payload: bits(&[9], 0),
        },
    ]);
    let received = read_control_datagram(&datagram, &custom).unwrap();
    assert_eq!(messages(&received.summary().bunches[0].control).len(), 1);
    assert!(matches!(
        received.summary().bunches[1].control,
        ControlDisposition::NotControl
    ));
}

#[test]
fn partial_control_bunches_are_reported_not_assembled() {
    let mut partial = control_bunch(0, bits(&[0, 1, 2], 3));
    (partial.header.partial, partial.header.partial_initial) = (true, true);
    let received = read_control_datagram(&datagram(&[partial]), &config()).unwrap();
    let bunch = &received.summary().bunches[0];
    assert!(bunch.partial && bunch.partial_initial && !bunch.partial_final);
    assert!(matches!(
        bunch.control,
        ControlDisposition::PartialNotAssembled
    ));
    assert_eq!(received.initial_bodies().count(), 0);
}

#[test]
fn handshake_datagram_is_rejected_as_a_handshake() {
    let error = read_control_datagram(&initial_request(), &config())
        .err()
        .unwrap();
    assert!(error.is::<HandshakePacket>());
    assert!(read_control_datagram(&[], &config()).is_err());
    assert!(read_control_datagram(&[1, 0], &config()).is_err());
}

#[test]
fn malformed_initial_bodies_fail_the_datagram() {
    let body = initial_body(1, 0x0102_0304, &"LAB".encode_utf16().collect::<Vec<_>>());
    // Every truncated ID 0 body, byte aligned.
    for length in 1..body.len() {
        let message = error(&datagram(&[control_bunch(0, bits(&body[..length], 0))]));
        assert!(message.contains("control bunch 0"), "{length}: {message}");
        assert!(message.contains("truncated"), "{length}: {message}");
    }
    // A payload ending mid-body, not on a byte boundary.
    let message = error(&datagram(&[control_bunch(0, bits(&body[..7], 3))]));
    assert!(message.contains("truncated"), "{message}");
    // Complete messages followed by part of an ID byte.
    let message = error(&datagram(&[control_bunch(0, bits(&body, 3))]));
    assert!(message.contains("inside a message ID"), "{message}");
    // Oversized string counts in both sender orders.
    for (first, count) in [
        (1, 65_537i32.to_le_bytes()),
        (1, (-65_537i32).to_le_bytes()),
        (1, i32::MIN.to_le_bytes()),
        (0, 65_537i32.to_be_bytes()),
    ] {
        let body = [&[0, first][..], &[0; 4], &count, &[0; 16]].concat();
        let message = error(&datagram(&[control_bunch(0, bits(&body, 0))]));
        assert!(message.contains("research unit limit"), "{message}");
    }
    // First field 0 reads the little-endian count 4 as 0x0400_0000: over the cap.
    let mut swapped = body.clone();
    swapped[1] = 0;
    let message = error(&datagram(&[control_bunch(0, bits(&swapped, 0))]));
    assert!(message.contains("research unit limit"), "{message}");
    // Strict token rules are reused unchanged.
    let mut unterminated = body.clone();
    *unterminated.last_mut().unwrap() = b'X';
    let message = error(&datagram(&[control_bunch(0, bits(&unterminated, 0))]));
    assert!(
        message.contains("missing archive string terminator"),
        "{message}"
    );
}

#[test]
fn summaries_render_no_token_contents_or_version_fields() {
    let body = initial_body(
        1,
        0xface_b00c,
        &"private-marker".encode_utf16().collect::<Vec<_>>(),
    );
    let received =
        read_control_datagram(&datagram(&[control_bunch(0, bits(&body, 0))]), &config()).unwrap();
    for text in [
        format!("{:?}", received.summary()),
        serde_json::to_string(received.summary()).unwrap(),
    ] {
        assert!(!text.contains("private-marker"));
        assert!(!text.contains("4207849484"));
        assert!(!text.contains("first_field"));
        assert!(!text.contains("remote_version"));
    }
}

fn sample_datagram_bits() -> BitWriter {
    let body = initial_body(1, 0x0102_0304, &"LAB".encode_utf16().collect::<Vec<_>>());
    let payload = [body.as_slice(), &[3, 1, 2]].concat();
    let mut actor = header();
    actor.channel_index = 2;
    let mut writer = BitWriter::new();
    write_data_prefix(&mut writer, 0, 0).unwrap();
    let header = PacketHeader {
        sequence: 1,
        acked_sequence: 0,
        history: vec![0, 1],
    };
    write_packet_header(&mut writer, &header, None, Receiver::Server).unwrap();
    for bunch in [
        control_bunch(0, bits(&payload, 0)),
        Bunch {
            header: actor,
            payload: bits(&[7; 3], 2),
        },
    ] {
        write_bunch(&mut writer, &bunch, &LIMITS).unwrap();
    }
    writer
}

#[test]
fn truncation_and_bit_flips_of_a_sample_datagram_never_panic() {
    let full = sample_datagram_bits();
    let complete = finish_data_packet(full.clone());
    assert!(read_control_datagram(&complete, &config()).is_ok());
    let mut source =
        icarus_bevy::experimental::packet::BitReader::new(full.as_bytes(), full.bit_len()).unwrap();
    let mut prefix = BitWriter::new();
    let mut accepted = 0;
    for _ in 0..full.bit_len() {
        // Re-terminate every proper bit prefix.
        if read_control_datagram(&finish_data_packet(prefix.clone()), &config()).is_ok() {
            accepted += 1;
        }
        prefix.write_bit(source.read_bit());
    }
    // Only the prefix ending after the header and the first bunch can be complete.
    assert!(accepted <= 2, "{accepted}");
    for length in 0..complete.len() {
        let _ = read_control_datagram(&complete[..length], &config());
    }
    for bit in 0..complete.len() * 8 {
        let mut flipped = complete.clone();
        flipped[bit / 8] ^= 1 << (bit % 8);
        let _ = read_control_datagram(&flipped, &config());
    }
}

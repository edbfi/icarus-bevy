use icarus_bevy::experimental::archive::ByteOrder;
use icarus_bevy::experimental::control::TokenEncoding;
use icarus_bevy::experimental::control_messages::{
    ArchiveText, ControlChannelSendQueue, ControlCodec, ControlFallback, ControlMessage,
    ControlSendDisposition, DESTRUCTION_INFO, Decoded, MAX_CONTROL_OUT_REC,
    MAX_QUEUED_CONTROL_MESSAGES, NetIdForm, NetIdType, NetIdValue, UniqueNetId,
    encode_control_message, read_control_fallback, read_control_message, read_control_messages,
    read_unique_net_id, write_control_fallback, write_control_message, write_unique_net_id,
};
use icarus_bevy::experimental::packet::{BitReader, BitWriter};

// Independently authored vectors from the spec rules; no captured traffic.

fn text(value: &str) -> ArchiveText {
    ArchiveText::text(value).unwrap()
}

fn codec(order: ByteOrder) -> ControlCodec {
    ControlCodec {
        order,
        ..ControlCodec::default()
    }
}

fn decode(bytes: &[u8], codec: &ControlCodec) -> anyhow::Result<Decoded> {
    let mut reader = BitReader::new(bytes, bytes.len() * 8)?;
    let decoded = read_control_message(&mut reader, codec)?;
    assert!(reader.at_end(), "unread bits");
    Ok(decoded)
}

fn message(bytes: &[u8], codec: &ControlCodec) -> ControlMessage {
    match decode(bytes, codec).unwrap() {
        Decoded::Message(message) => message,
        other => panic!("{other:?}"),
    }
}

fn hex_id(kind: NetIdType, hex: &str) -> UniqueNetId {
    UniqueNetId {
        kind,
        value: NetIdValue::from_hex(hex).unwrap(),
    }
}

fn samples() -> Vec<ControlMessage> {
    let wide = ArchiveText::new(&[0x41, 0xd83d, 0xde00]).unwrap();
    vec![
        ControlMessage::Hello {
            first_field: 1,
            version: 0x0102_0304,
            token: text("LAB"),
        },
        ControlMessage::Hello {
            first_field: 0,
            version: 0x0102_0304,
            token: wide.clone(),
        },
        ControlMessage::Welcome {
            map_url: text("/Game/Map"),
            game_name: ArchiveText::new(&[]).unwrap(),
            redirect_url: ArchiveText::with_encoding(&[], TokenEncoding::Narrow).unwrap(),
        },
        ControlMessage::Upgrade { version: 17 },
        ControlMessage::Challenge {
            token: text("0A1B2C3D"),
        },
        ControlMessage::Netspeed { rate: 10_000 },
        ControlMessage::Login {
            response: text("response"),
            url: wide.clone(),
            unique_net_id: hex_id(NetIdType::Default, "ABC"),
            platform: text("WIN64"),
        },
        ControlMessage::Login {
            response: ArchiveText::with_encoding(&[], TokenEncoding::Wide).unwrap(),
            url: text("?Name=A"),
            unique_net_id: hex_id(NetIdType::Named(text("STEAM")), "0123456789ABCDEF"),
            platform: text("LINUX"),
        },
        ControlMessage::Failure {
            message: text("failed"),
        },
        ControlMessage::Join,
        ControlMessage::SplitJoin {
            url: text("?Split"),
            unique_net_id: hex_id(NetIdType::Indexed(2), ""),
        },
        ControlMessage::SplitJoin {
            url: text("?Text"),
            unique_net_id: UniqueNetId {
                kind: NetIdType::Indexed(30),
                value: NetIdValue::Text(wide),
            },
        },
        ControlMessage::Swap { value: u32::MAX },
        ControlMessage::Debug { text: text("dbg") },
        ControlMessage::NetGuidAssign {
            net_guid: 300,
            path: text("A"),
        },
        ControlMessage::EncryptionAck,
    ]
}

#[test]
fn every_message_round_trips_in_both_orders_at_unaligned_positions() {
    for order in [ByteOrder::Little, ByteOrder::Big] {
        let codec = codec(order);
        for sample in samples() {
            let bytes = encode_control_message(&sample, &codec).unwrap();
            assert_eq!(bytes[0], sample.id());
            assert_eq!(message(&bytes, &codec), sample);
            let mut writer = BitWriter::new();
            writer.write_bits(0b101, 3);
            write_control_message(&mut writer, &sample, &codec).unwrap();
            let mut reader = BitReader::new(writer.as_bytes(), writer.bit_len()).unwrap();
            reader.read_bits(3);
            let decoded = read_control_message(&mut reader, &codec).unwrap();
            assert_eq!(decoded, Decoded::Message(sample));
            assert!(reader.at_end());
        }
    }
}

#[test]
fn hand_built_vectors_match_the_spec_rules_in_both_orders() {
    let (little, big) = (codec(ByteOrder::Little), codec(ByteOrder::Big));
    let challenge_le = [&[3, 9, 0, 0, 0][..], b"0A1B2C3D", &[0]].concat();
    let challenge_be = [&[3, 0, 0, 0, 9][..], b"0A1B2C3D", &[0]].concat();
    for (sample, little_bytes, big_bytes) in [
        (
            ControlMessage::Upgrade { version: 17 },
            vec![2, 17, 0, 0, 0],
            vec![2, 0, 0, 0, 17],
        ),
        (
            ControlMessage::Netspeed { rate: 10_000 },
            vec![4, 0x10, 0x27, 0, 0],
            vec![4, 0, 0, 0x27, 0x10],
        ),
        (
            ControlMessage::Challenge {
                token: text("0A1B2C3D"),
            },
            challenge_le,
            challenge_be,
        ),
        (
            ControlMessage::NetGuidAssign {
                net_guid: 300,
                path: text("A"),
            },
            vec![18, 0x59, 0x04, 2, 0, 0, 0, b'A', 0],
            vec![18, 0x59, 0x04, 0, 0, 0, 2, b'A', 0],
        ),
        (ControlMessage::Join, vec![9], vec![9]),
        (ControlMessage::EncryptionAck, vec![21], vec![21]),
    ] {
        assert_eq!(
            encode_control_message(&sample, &little).unwrap(),
            little_bytes
        );
        assert_eq!(encode_control_message(&sample, &big).unwrap(), big_bytes);
        assert_eq!(message(&little_bytes, &little), sample);
        assert_eq!(message(&big_bytes, &big), sample);
    }
    // Hello's own first field selects its order, whatever the codec order.
    let hello_le = [&[0, 1, 4, 3, 2, 1, 4, 0, 0, 0][..], b"LAB", &[0]].concat();
    let hello_be = [
        &[0, 0, 1, 2, 3, 4, 0xff, 0xff, 0xff, 0xfe][..],
        &[0, 0x41, 0, 0],
    ]
    .concat();
    for codec in [little, big] {
        assert_eq!(
            message(&hello_le, &codec),
            ControlMessage::Hello {
                first_field: 1,
                version: 0x0102_0304,
                token: text("LAB"),
            }
        );
        assert_eq!(
            message(&hello_be, &codec),
            ControlMessage::Hello {
                first_field: 0,
                version: 0x0102_0304,
                token: ArchiveText::with_encoding(&[0x41], TokenEncoding::Wide).unwrap(),
            }
        );
    }
}

#[test]
fn unique_net_id_flags_lengths_and_hex_rendering_are_exact() {
    let little = ControlCodec::default();
    for (id, bytes, hex) in [
        // Encoded, odd, default type: 0b101, length 2, zero-padded nibble.
        (
            hex_id(NetIdType::Default, "abc"),
            vec![0x05, 2, 0x0a, 0xbc],
            Some("ABC"),
        ),
        (
            hex_id(NetIdType::Default, "0ABC"),
            vec![0x01, 2, 0x0a, 0xbc],
            Some("0ABC"),
        ),
        // Encoded and empty, type index 2: 2 << 3 | 0b011.
        (hex_id(NetIdType::Indexed(2), ""), vec![0x13], Some("")),
        // Encoded and empty returns after the flags byte: type 31 reads no name.
        (hex_id(NetIdType::Indexed(31), ""), vec![0xfb], Some("")),
        (hex_id(NetIdType::Default, ""), vec![0x03], Some("")),
        // Type 31 is followed by its type-name string, then the value.
        (
            hex_id(NetIdType::Named(text("EOS")), "FF"),
            [&[0xf9, 4, 0, 0, 0][..], b"EOS\0", &[1, 0xff]].concat(),
            Some("FF"),
        ),
        // Inferred text form: no encoded bits, then a string.
        (
            UniqueNetId {
                kind: NetIdType::Indexed(1),
                value: NetIdValue::Text(text("id")),
            },
            [&[0x08, 3, 0, 0, 0][..], b"id\0"].concat(),
            None,
        ),
    ] {
        let mut writer = BitWriter::new();
        write_unique_net_id(&mut writer, &id, &little).unwrap();
        assert_eq!(writer.as_bytes(), bytes);
        let mut reader = BitReader::new(&bytes, bytes.len() * 8).unwrap();
        let read = read_unique_net_id(&mut reader, &little).unwrap();
        assert!(reader.at_end());
        assert_eq!(read, id);
        assert_eq!(read.value.hex().as_deref(), hex);
    }
    let summary = hex_id(NetIdType::Named(text("EOS")), "ABC").summary();
    assert_eq!(
        (summary.type_index, summary.length, summary.odd),
        (31, 2, true)
    );
    assert!(matches!(summary.form, NetIdForm::Encoded));
    assert_eq!(summary.type_name.unwrap().units, 3);

    for invalid in [
        vec![0x07],                   // encoded, empty and odd
        vec![0x05, 0],                // odd with no bytes
        vec![0x05, 1, 0x1a],          // odd with a nonzero dropped nibble
        vec![0x02, 0, 0, 0, 0],       // empty bit without the encoded bit
        vec![0x04, 0, 0, 0, 0],       // odd bit without the encoded bit
        vec![0x01, 3, 0, 0],          // truncated encoded bytes
        vec![0xf9, 9, 0, 0, 0, b'E'], // truncated type name
    ] {
        let mut reader = BitReader::new(&invalid, invalid.len() * 8).unwrap();
        assert!(
            read_unique_net_id(&mut reader, &little).is_err(),
            "{invalid:?}"
        );
    }
    // 0xfb is one byte even when a would-be type-name string follows.
    let bytes = [0xfb, 4, 0, 0, 0, b'E', b'O', b'S', 0];
    let mut reader = BitReader::new(&bytes, bytes.len() * 8).unwrap();
    let read = read_unique_net_id(&mut reader, &little).unwrap();
    assert_eq!(read, hex_id(NetIdType::Indexed(31), ""));
    assert_eq!(reader.position(), 8);
    let named_empty = hex_id(NetIdType::Named(text("EOS")), "");
    assert!(write_unique_net_id(&mut BitWriter::new(), &named_empty, &little).is_err());
    assert!(
        write_unique_net_id(
            &mut BitWriter::new(),
            &hex_id(NetIdType::Indexed(0), ""),
            &little
        )
        .is_err()
    );
    assert!(NetIdValue::from_hex("0G").is_err());
    assert!(NetIdValue::from_hex(&"F".repeat(510)).is_ok());
    assert!(NetIdValue::from_hex(&"F".repeat(511)).is_err());
    for kind in [NetIdType::Indexed(0), NetIdType::Indexed(31)] {
        let id = hex_id(kind, "AB");
        assert!(write_unique_net_id(&mut BitWriter::new(), &id, &little).is_err());
    }
    let mismatched = UniqueNetId {
        kind: NetIdType::Default,
        value: NetIdValue::Encoded {
            odd: true,
            bytes: vec![0x1a],
        },
    };
    assert!(write_unique_net_id(&mut BitWriter::new(), &mismatched, &little).is_err());
}

#[test]
fn a_type_name_naming_none_ends_the_id() {
    let little = codec(ByteOrder::Little);
    for (encoded, name) in [(true, "None"), (false, "none"), (true, "NONE"), (false, "")] {
        let id = UniqueNetId {
            kind: NetIdType::Named(text(name)),
            value: NetIdValue::Absent { encoded },
        };
        let mut writer = BitWriter::new();
        write_unique_net_id(&mut writer, &id, &little).unwrap();
        let written = writer.bit_len();
        // Would-be value bytes after the type name stay unread.
        writer.write_bytes(&[2, 0xab, 0xcd]);
        let bytes = writer.into_bytes();
        assert_eq!(bytes[0], 0xf8 | u8::from(encoded), "{name:?}");
        let mut reader = BitReader::new(&bytes, bytes.len() * 8).unwrap();
        assert_eq!(read_unique_net_id(&mut reader, &little).unwrap(), id);
        assert_eq!(reader.position(), written);
        assert!(matches!(id.summary().form, NetIdForm::Absent));
        assert_eq!(id.summary().length, 0);
    }
    // Other names, a number suffix included, carry a value.
    for name in ["None_0", "Nonee", " None", "STEAM"] {
        let absent = UniqueNetId {
            kind: NetIdType::Named(text(name)),
            value: NetIdValue::Absent { encoded: true },
        };
        assert!(write_unique_net_id(&mut BitWriter::new(), &absent, &little).is_err());
        let id = hex_id(NetIdType::Named(text(name)), "AB");
        let mut writer = BitWriter::new();
        write_unique_net_id(&mut writer, &id, &little).unwrap();
        let bytes = writer.into_bytes();
        let mut reader = BitReader::new(&bytes, bytes.len() * 8).unwrap();
        assert_eq!(read_unique_net_id(&mut reader, &little).unwrap(), id);
        assert!(reader.at_end());
    }
    // A None type name cannot carry a value, nor other types be absent.
    let valued = hex_id(NetIdType::Named(text("None")), "AB");
    assert!(write_unique_net_id(&mut BitWriter::new(), &valued, &little).is_err());
    let unnamed = UniqueNetId {
        kind: NetIdType::Indexed(3),
        value: NetIdValue::Absent { encoded: false },
    };
    assert!(write_unique_net_id(&mut BitWriter::new(), &unnamed, &little).is_err());
    // Form flags on an absent id fail closed, as do names at the Name limit.
    let mut writer = BitWriter::new();
    writer.write_bits(0xfd, 8);
    writer.write_bits(5, 32);
    writer.write_bytes(b"None\0");
    let bytes = writer.into_bytes();
    let mut reader = BitReader::new(&bytes, bytes.len() * 8).unwrap();
    assert!(read_unique_net_id(&mut reader, &little).is_err());
    let long = hex_id(NetIdType::Named(text(&"A".repeat(1024))), "AB");
    assert!(write_unique_net_id(&mut BitWriter::new(), &long, &little).is_err());
    let fits = hex_id(NetIdType::Named(text(&"A".repeat(1023))), "AB");
    assert!(write_unique_net_id(&mut BitWriter::new(), &fits, &little).is_ok());
}

#[test]
fn unknown_ids_stop_decoding_without_reading_fields() {
    let codec = ControlCodec::default();
    for id in [7u8, 8, 11, 14, 16, 19, 20, 22, 255] {
        let bytes = [9, id, 1, 2, 3];
        let mut reader = BitReader::new(&bytes, 40).unwrap();
        let decoded = read_control_messages(&mut reader, &codec).unwrap();
        assert_eq!(
            decoded,
            [
                Decoded::Message(ControlMessage::Join),
                Decoded::Unknown { id }
            ]
        );
        assert_eq!(reader.position(), 16);
    }
    let bytes = [
        &[9u8][..],
        &encode_control_message(&ControlMessage::Upgrade { version: 2 }, &codec).unwrap(),
    ]
    .concat();
    let mut reader = BitReader::new(&bytes, bytes.len() * 8).unwrap();
    assert_eq!(read_control_messages(&mut reader, &codec).unwrap().len(), 2);
    // A partial ID byte is a truncation.
    let mut reader = BitReader::new(&bytes, bytes.len() * 8 - 3).unwrap();
    assert!(read_control_messages(&mut reader, &codec).is_err());
}

#[test]
fn control_channel_fallback_layouts_round_trip_in_both_orders() {
    let fallbacks = [
        ControlFallback::Skip { guid: [1, 2, 3, 4] },
        ControlFallback::Abort { guid: [5, 6, 7, 8] },
        ControlFallback::ActorChannelFailure { channel_index: 42 },
        ControlFallback::GameSpecific {
            message: text("custom"),
        },
        ControlFallback::SecurityViolation {
            kind: 3,
            message: text("violation"),
        },
        ControlFallback::DestructionInfo {
            reason: 1,
            net_guid: 512,
        },
        ControlFallback::BeaconJoin,
        ControlFallback::BeaconLogin {
            text: text("beacon"),
            unique_net_id: hex_id(NetIdType::Default, "AB"),
        },
        ControlFallback::BeaconAck { net_guid: 512 },
        ControlFallback::BeaconWelcome {
            text: text("welcome"),
        },
    ];
    for order in [ByteOrder::Little, ByteOrder::Big] {
        let codec = codec(order);
        for fallback in &fallbacks {
            let mut writer = BitWriter::new();
            write_control_fallback(&mut writer, fallback, &codec).unwrap();
            let mut reader = BitReader::new(writer.as_bytes(), writer.bit_len()).unwrap();
            let id = reader.read_bits(8) as u8;
            assert_eq!(id, fallback.id());
            let decoded = read_control_fallback(id, &mut reader, &codec)
                .unwrap()
                .unwrap();
            assert_eq!(&decoded, fallback);
            assert!(reader.at_end());
        }
    }

    // INTERNAL_LOAD_OBJECT_GUID (1) in DestructionInfo is rejected by both writer and reader.
    let little = codec(ByteOrder::Little);
    let bad_destruction = ControlFallback::DestructionInfo {
        reason: 0,
        net_guid: 1,
    };
    assert!(write_control_fallback(&mut BitWriter::new(), &bad_destruction, &little).is_err());
    let mut raw = BitWriter::new();
    raw.write_bits(1, 8);
    raw.write_int_packed(1);
    let mut reader = BitReader::new(raw.as_bytes(), raw.bit_len()).unwrap();
    assert!(read_control_fallback(DESTRUCTION_INFO, &mut reader, &little).is_err());
}

#[test]
fn string_caps_and_count_limits_are_strict() {
    let capped = ControlCodec {
        order: ByteOrder::Little,
        max_string_units: 4,
    };
    let fits = ControlMessage::Challenge { token: text("ABC") };
    let over = ControlMessage::Challenge {
        token: text("ABCD"),
    };
    let bytes = encode_control_message(&fits, &capped).unwrap();
    assert_eq!(message(&bytes, &capped), fits);
    assert!(encode_control_message(&over, &capped).is_err());
    let over_bytes = encode_control_message(&over, &ControlCodec::default()).unwrap();
    let error = format!("{:#}", decode(&over_bytes, &capped).unwrap_err());
    assert!(error.contains("string unit limit"), "{error}");
    for count in [i32::MIN, 65_537, -65_537] {
        let bytes = [&[3][..], &count.to_le_bytes()].concat();
        assert!(decode(&bytes, &ControlCodec::default()).is_err());
    }
    // Strict archive rules: terminator, embedded NUL, non-ASCII narrow, U+FFFF.
    for payload in [
        [&[3][..], &2i32.to_le_bytes(), b"AB"].concat(),
        [&[3][..], &3i32.to_le_bytes(), b"A\0\0"].concat(),
        [&[3][..], &2i32.to_le_bytes(), &[0x80, 0]].concat(),
        [&[3][..], &(-2i32).to_le_bytes(), &[0xff, 0xff, 0, 0]].concat(),
    ] {
        assert!(decode(&payload, &ControlCodec::default()).is_err());
    }
    assert!(ArchiveText::with_encoding(&[0x263a], TokenEncoding::Narrow).is_err());
    assert!(ArchiveText::with_encoding(&[0x41], TokenEncoding::Empty).is_err());
    assert!(ArchiveText::new(&[0]).is_err());
}

#[test]
fn truncation_at_every_bit_of_each_sample_message_is_an_error() {
    for order in [ByteOrder::Little, ByteOrder::Big] {
        let codec = codec(order);
        for sample in samples() {
            let bytes = encode_control_message(&sample, &codec).unwrap();
            for length in 0..bytes.len() * 8 {
                let mut reader = BitReader::new(&bytes, length).unwrap();
                assert!(
                    read_control_message(&mut reader, &codec).is_err(),
                    "{} truncated at bit {length}",
                    sample.name()
                );
            }
        }
    }
}

#[test]
fn summaries_and_debug_output_carry_kinds_and_lengths_only() {
    let secret = text("private-marker");
    let message = ControlMessage::Login {
        response: secret.clone(),
        url: secret.clone(),
        unique_net_id: hex_id(NetIdType::Named(secret.clone()), "DEADBEEF"),
        platform: secret,
    };
    let summary = message.summary();
    assert_eq!((summary.id, summary.name), (5, "login"));
    assert_eq!(summary.strings.len(), 3);
    assert!(summary.strings.iter().all(|text| text.units == 14));
    for rendered in [
        format!("{message:?}"),
        format!("{summary:?}"),
        serde_json::to_string(&summary).unwrap(),
    ] {
        assert!(!rendered.contains("private-marker"), "{rendered}");
        assert!(!rendered.to_uppercase().contains("DEADBEEF"), "{rendered}");
        assert!(!rendered.contains("222, 173"), "{rendered}");
    }
    let hello = ControlMessage::Hello {
        first_field: 1,
        version: 0xface_b00c,
        token: text("t"),
    }
    .summary();
    let rendered = serde_json::to_string(&hello).unwrap();
    assert!(!rendered.contains("4207849484"));
}

#[test]
fn control_channel_send_queue_enforces_out_rec_and_queued_message_caps() {
    let mut queue = ControlChannelSendQueue::new();
    assert!(queue.is_empty());

    // Below MAX_CONTROL_OUT_REC (255) with empty queue sends immediately.
    assert_eq!(
        queue.send_or_queue(1u32, MAX_CONTROL_OUT_REC - 1, false),
        (ControlSendDisposition::SendImmediate, Some(1))
    );
    // At 255, a non-close bunch queues, whereas a close bunch (`255 + bClose`) still sends immediately.
    assert_eq!(
        queue.send_or_queue(2u32, MAX_CONTROL_OUT_REC, true),
        (ControlSendDisposition::SendImmediate, Some(2))
    );
    assert_eq!(
        queue.send_or_queue(3u32, MAX_CONTROL_OUT_REC, false),
        (ControlSendDisposition::Queued { queued_len: 1 }, None)
    );
    // Once non-empty, subsequent bunches queue even if NumOutRec drops.
    assert_eq!(
        queue.send_or_queue(4u32, 0, false),
        (ControlSendDisposition::Queued { queued_len: 2 }, None)
    );

    // Tick does not drain before bOpenAcked or while closing.
    assert!(queue.drain_ready(false, false, 0).is_empty());
    assert!(queue.drain_ready(true, true, 0).is_empty());
    assert_eq!(
        queue.drain_ready(true, false, MAX_CONTROL_OUT_REC - 1),
        vec![3]
    );
    assert_eq!(queue.len(), 1);
    assert_eq!(queue.drain_ready(true, false, 0), vec![4]);
    assert!(queue.is_empty());

    // Queue up to MAX_QUEUED_CONTROL_MESSAGES (0x7fff); the 0x8000-th message triggers OverflowClose.
    for i in 1..=MAX_QUEUED_CONTROL_MESSAGES {
        assert_eq!(
            queue.send_or_queue(i as u32, MAX_CONTROL_OUT_REC, false),
            (ControlSendDisposition::Queued { queued_len: i }, None)
        );
    }
    assert_eq!(
        queue.send_or_queue(0xffff, MAX_CONTROL_OUT_REC, false),
        (ControlSendDisposition::OverflowClose, None)
    );
}

#[test]
fn control_entries_include_fallback_layouts_without_changing_message_reads() {
    use icarus_bevy::experimental::control_messages::read_control_entry;
    use icarus_bevy::experimental::packet::Payload;
    let codec = ControlCodec::default();
    let failure = ControlFallback::ActorChannelFailure { channel_index: 2 };
    let mut writer = BitWriter::new();
    write_control_fallback(&mut writer, &failure, &codec).unwrap();
    let payload = Payload::from_writer(writer);
    let mut reader = payload.reader();
    assert_eq!(
        read_control_entry(&mut reader, &codec).unwrap(),
        Decoded::Fallback(failure)
    );
    assert!(reader.at_end());
    // The message reader still stops at the ID without reading the body.
    assert_eq!(
        read_control_messages(&mut payload.reader(), &codec).unwrap(),
        [Decoded::Unknown { id: 16 }]
    );
    // An ID outside both tables stays unknown.
    let mut writer = BitWriter::new();
    writer.write_bits(7, 8);
    let payload = Payload::from_writer(writer);
    assert_eq!(
        read_control_entry(&mut payload.reader(), &codec).unwrap(),
        Decoded::Unknown { id: 7 }
    );
}

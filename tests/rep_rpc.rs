use icarus_bevy::experimental::actor_channel::{ActorChannelCodec, ActorChannelLimits};
use icarus_bevy::experimental::archive::ByteOrder;
use icarus_bevy::experimental::packet::{BitWriter, Payload};
use icarus_bevy::experimental::rep_layout::{
    PropertyStream, RepCommand, RepEntry, RepLayoutCodec, RepLayoutLimits, RepValue,
};
use icarus_bevy::experimental::rep_rpc::{
    CustomDeltaHeader, FAST_ARRAY_COUNT_CAP, FastArrayDelta, FastArrayItem, FastArrayItemLayout,
    FieldHeader, RPC_ARRAY_COUNT_MAX, RpcArgument, RpcItem, RpcParameter, encode_rpc_payload,
    parse_rpc_payload, read_custom_delta_header, read_fast_array_delta, read_field_header,
    write_custom_delta_header, write_fast_array_delta, write_field_header,
};

// Independently authored bit fixtures for the client replicator's field
// headers and RPC parameters (F-637), built bit by bit; no captured traffic.

fn codec(order: ByteOrder) -> RepLayoutCodec {
    RepLayoutCodec {
        actor: ActorChannelCodec {
            engine_net_version: 17,
            exports_enabled: false,
            has_level: false,
            order,
            limits: ActorChannelLimits::default(),
        },
        checksums: false,
        limits: RepLayoutLimits::default(),
    }
}

fn payload(build: impl FnOnce(&mut BitWriter)) -> Payload {
    let mut writer = BitWriter::new();
    build(&mut writer);
    Payload::from_writer(writer)
}

fn byte(enum_bits: Option<u8>) -> RepCommand {
    RepCommand::Byte { enum_bits }
}

#[test]
fn field_headers_carry_a_bounded_index_a_packed_bit_count_and_the_payload() {
    let body = payload(|writer| writer.write_bits(0b1_0110, 5));
    // Index 3 below bound 5 takes two bits; index 4 takes three.
    for (index, index_bits) in [(3, vec![true, true]), (4, vec![false, false, true])] {
        let stream = payload(|writer| {
            index_bits.iter().for_each(|bit| writer.write_bit(*bit));
            writer.write_bits(5 << 1, 8);
            writer.write_bits(0b1_0110, 5);
            writer.write_bits(0b101, 3);
        });
        let mut reader = stream.reader();
        let header = read_field_header(&mut reader, 5, 64).unwrap();
        assert_eq!(
            header,
            FieldHeader {
                index,
                payload: body.clone()
            }
        );
        // The next field's bits stay unread.
        assert_eq!(reader.remaining(), 3);
        let mut writer = BitWriter::new();
        write_field_header(&mut writer, &header, 5).unwrap();
        writer.write_bits(0b101, 3);
        assert_eq!(Payload::from_writer(writer), stream);
    }
    // A bound of one reads no index bits.
    let stream = payload(|writer| writer.write_bits(0, 8));
    let header = read_field_header(&mut stream.reader(), 1, 64).unwrap();
    assert_eq!(header.index, 0);
    assert_eq!(header.payload.bit_len(), 0);

    let over = FieldHeader {
        index: 5,
        payload: body.clone(),
    };
    assert!(write_field_header(&mut BitWriter::new(), &over, 5).is_err());
    assert!(read_field_header(&mut stream.reader(), 0, 64).is_err());
    let long = payload(|writer| {
        writer.write_bits(0b11, 2);
        writer.write_bits(5 << 1, 8);
        writer.write_bits(0, 5);
    });
    assert!(read_field_header(&mut long.reader(), 5, 4).is_err());
    let truncated = payload(|writer| {
        writer.write_bits(0b11, 2);
        writer.write_bits(5 << 1, 8);
        writer.write_bits(0, 4);
    });
    assert!(read_field_header(&mut truncated.reader(), 5, 64).is_err());
}

fn parameters() -> Vec<RpcParameter> {
    vec![
        RpcParameter::Bool,
        RpcParameter::Value(vec![byte(None)]),
        RpcParameter::Value(vec![RepCommand::Str]),
        RpcParameter::Value(vec![RepCommand::DynamicArray {
            element: vec![RepCommand::Bool, byte(Some(3))],
        }]),
        RpcParameter::Value(vec![RepCommand::Enum { bits: 4 }, RepCommand::Bool]),
    ]
}

fn arguments() -> Vec<RpcArgument> {
    vec![
        RpcArgument::Bool(true),
        RpcArgument::Sent(vec![RpcItem::Value(RepValue::Byte(0x5a))]),
        RpcArgument::NotSent,
        RpcArgument::Sent(vec![RpcItem::Array(vec![
            vec![
                RpcItem::Value(RepValue::Bool(true)),
                RpcItem::Value(RepValue::Byte(5)),
            ],
            vec![
                RpcItem::Value(RepValue::Bool(false)),
                RpcItem::Value(RepValue::Byte(2)),
            ],
        ])]),
        RpcArgument::Sent(vec![
            RpcItem::Value(RepValue::Enum(9)),
            RpcItem::Value(RepValue::Bool(false)),
        ]),
    ]
}

#[test]
fn rpc_parameters_are_presence_bits_then_commands_and_bools_are_bare_bits() {
    for order in [ByteOrder::Little, ByteOrder::Big] {
        let expected = payload(|writer| {
            writer.write_bit(true); // the bool parameter itself
            writer.write_bit(true);
            writer.write_bits(0x5a, 8);
            writer.write_bit(false); // the string parameter is not sent
            writer.write_bit(true);
            let count = match order {
                ByteOrder::Little => 0x0002,
                ByteOrder::Big => 0x0200,
            };
            writer.write_bits(count, 16);
            writer.write_bit(true);
            writer.write_bits(5, 3);
            writer.write_bit(false);
            writer.write_bits(2, 3);
            writer.write_bit(true);
            writer.write_bits(9, 4);
            writer.write_bit(false);
        });
        let codec = codec(order);
        assert_eq!(
            encode_rpc_payload(&parameters(), &arguments(), &codec).unwrap(),
            expected
        );
        assert_eq!(
            parse_rpc_payload(&expected, &parameters(), &codec).unwrap(),
            arguments()
        );
    }
}

#[test]
fn rpc_payloads_are_consumed_exactly_and_mismatches_fail() {
    let codec = codec(ByteOrder::Little);
    let encoded = encode_rpc_payload(&parameters(), &arguments(), &codec).unwrap();
    let mut longer = BitWriter::new();
    let mut reader = encoded.reader();
    for _ in 0..encoded.bit_len() {
        longer.write_bit(reader.read_bit());
    }
    longer.write_bit(false);
    assert!(parse_rpc_payload(&Payload::from_writer(longer), &parameters(), &codec).is_err());
    let mut shorter = BitWriter::new();
    let mut reader = encoded.reader();
    for _ in 1..encoded.bit_len() {
        shorter.write_bit(reader.read_bit());
    }
    assert!(parse_rpc_payload(&Payload::from_writer(shorter), &parameters(), &codec).is_err());

    let mut wrong = arguments();
    wrong[0] = RpcArgument::NotSent;
    assert!(encode_rpc_payload(&parameters(), &wrong, &codec).is_err());
    let mut wrong = arguments();
    wrong[1] = RpcArgument::Sent(vec![]);
    assert!(encode_rpc_payload(&parameters(), &wrong, &codec).is_err());
    let mut wrong = arguments();
    wrong[3] = RpcArgument::Sent(vec![RpcItem::Value(RepValue::Bool(true))]);
    assert!(encode_rpc_payload(&parameters(), &wrong, &codec).is_err());
    assert!(encode_rpc_payload(&parameters(), &arguments()[..4], &codec).is_err());
    assert!(
        encode_rpc_payload(
            &[RpcParameter::Value(vec![])],
            &[RpcArgument::NotSent],
            &codec
        )
        .is_err()
    );
}

#[test]
fn rpc_array_counts_stop_at_the_client_maximum() {
    let mut codec = codec(ByteOrder::Little);
    codec.limits.max_array_elements = u16::MAX;
    codec.limits.max_entries = 70_000;
    // Zero-width elements keep the largest arrays small.
    let parameters = [RpcParameter::Value(vec![RepCommand::DynamicArray {
        element: vec![byte(Some(0))],
    }])];
    let array = |count: u16| {
        payload(|writer| {
            writer.write_bit(true);
            writer.write_bits(u32::from(count), 16);
        })
    };
    let arguments = parse_rpc_payload(&array(RPC_ARRAY_COUNT_MAX), &parameters, &codec).unwrap();
    let RpcArgument::Sent(items) = &arguments[0] else {
        panic!("the array parameter is sent");
    };
    assert!(matches!(&items[0], RpcItem::Array(elements) if elements.len() == 0xfffe));
    assert_eq!(
        encode_rpc_payload(&parameters, &arguments, &codec).unwrap(),
        array(0xfffe)
    );
    assert!(parse_rpc_payload(&array(0xffff), &parameters, &codec).is_err());
    // The research element limit applies below the client maximum.
    codec.limits.max_array_elements = 3;
    assert!(parse_rpc_payload(&array(4), &parameters, &codec).is_err());
    assert!(parse_rpc_payload(&array(3), &parameters, &codec).is_ok());
}

#[test]
fn custom_delta_payloads_start_with_a_versioned_bit_and_an_array_index() {
    // Net version 17, static array of 4: the bit, then packed index 2.
    let stream = payload(|writer| {
        writer.write_bit(true);
        writer.write_bits(2 << 1, 8);
        writer.write_bits(0b11, 2);
    });
    let mut reader = stream.reader();
    let header = read_custom_delta_header(&mut reader, 17, 4).unwrap();
    assert_eq!(
        header,
        CustomDeltaHeader {
            struct_delta: Some(true),
            array_index: Some(2),
        }
    );
    // The struct's own delta body stays unread.
    assert_eq!(reader.remaining(), 2);
    let mut writer = BitWriter::new();
    write_custom_delta_header(&mut writer, &header, 17, 4).unwrap();
    writer.write_bits(0b11, 2);
    assert_eq!(Payload::from_writer(writer), stream);

    // Before version 11 there is no bit; a single property has no index.
    let bare = payload(|writer| writer.write_bits(0b101, 3));
    let mut reader = bare.reader();
    let header = read_custom_delta_header(&mut reader, 10, 1).unwrap();
    assert_eq!(
        header,
        CustomDeltaHeader {
            struct_delta: None,
            array_index: None,
        }
    );
    assert_eq!(reader.remaining(), 3);
    let flagged = read_custom_delta_header(&mut bare.reader(), 11, 1).unwrap();
    assert_eq!(flagged.struct_delta, Some(true));

    // The index must stay below the dimension; presence follows the inputs.
    let outside = payload(|writer| {
        writer.write_bit(false);
        writer.write_bits(4 << 1, 8);
    });
    assert!(read_custom_delta_header(&mut outside.reader(), 17, 4).is_err());
    for (header, version, dim) in [
        (
            CustomDeltaHeader {
                struct_delta: None,
                array_index: None,
            },
            17,
            1,
        ),
        (
            CustomDeltaHeader {
                struct_delta: Some(false),
                array_index: None,
            },
            10,
            1,
        ),
        (
            CustomDeltaHeader {
                struct_delta: Some(false),
                array_index: None,
            },
            17,
            2,
        ),
        (
            CustomDeltaHeader {
                struct_delta: Some(false),
                array_index: Some(0),
            },
            17,
            1,
        ),
        (
            CustomDeltaHeader {
                struct_delta: Some(false),
                array_index: Some(2),
            },
            17,
            2,
        ),
    ] {
        assert!(write_custom_delta_header(&mut BitWriter::new(), &header, version, dim).is_err());
    }
    let truncated = payload(|writer| writer.write_bit(true));
    assert!(read_custom_delta_header(&mut truncated.reader(), 17, 3).is_err());
}

#[test]
fn mutated_payloads_never_panic_and_accepted_ones_reencode_exactly() {
    let codec = codec(ByteOrder::Little);
    let base = encode_rpc_payload(&parameters(), &arguments(), &codec).unwrap();
    let mut state = 0x9e37_79b9_7f4a_7c15_u64;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let mut accepted = 0;
    for _ in 0..20_000 {
        let mut bytes = base.bytes().to_vec();
        for _ in 0..1 + next() % 3 {
            let at = (next() % bytes.len() as u64) as usize;
            bytes[at] ^= 1 << (next() % 8);
        }
        let bit_len = base.bit_len() - 4 + (next() % 9) as usize;
        let mut writer = BitWriter::new();
        let reader_bits = bit_len.min(bytes.len() * 8);
        let source = icarus_bevy::experimental::packet::BitReader::new(&bytes, bytes.len() * 8);
        let mut source = source.unwrap();
        for _ in 0..reader_bits {
            writer.write_bit(source.read_bit());
        }
        let mutated = Payload::from_writer(writer);
        if let Ok(arguments) = parse_rpc_payload(&mutated, &parameters(), &codec) {
            accepted += 1;
            assert_eq!(
                encode_rpc_payload(&parameters(), &arguments, &codec).unwrap(),
                mutated
            );
        }
        let _ = read_field_header(&mut mutated.reader(), 7, 4096);
        let _ = read_custom_delta_header(&mut mutated.reader(), 17, 3);
    }
    assert!(accepted > 0);
}

fn word(writer: &mut BitWriter, value: i32, order: ByteOrder) {
    let bytes = match order {
        ByteOrder::Little => value.to_le_bytes(),
        ByteOrder::Big => value.to_be_bytes(),
    };
    bytes
        .iter()
        .for_each(|byte| writer.write_bits(u32::from(*byte), 8));
}

fn whole_delta() -> FastArrayDelta {
    FastArrayDelta {
        array_replication_key: 7,
        base_replication_key: 5,
        deleted_ids: vec![3, -1],
        changed: vec![
            (
                10,
                FastArrayItem::Whole(vec![
                    RpcItem::Value(RepValue::Byte(0x22)),
                    RpcItem::Value(RepValue::Bool(true)),
                ]),
            ),
            (
                11,
                FastArrayItem::Whole(vec![
                    RpcItem::Value(RepValue::Byte(0x01)),
                    RpcItem::Value(RepValue::Bool(false)),
                ]),
            ),
        ],
    }
}

#[test]
fn fast_array_whole_items_follow_a_four_word_header_and_deleted_ids() {
    let layout = FastArrayItemLayout::Whole(vec![byte(None), RepCommand::Bool]);
    for order in [ByteOrder::Little, ByteOrder::Big] {
        let expected = payload(|writer| {
            for value in [7, 5, 2, 2, 3, -1] {
                word(writer, value, order);
            }
            word(writer, 10, order);
            writer.write_bits(0x22, 8);
            writer.write_bit(true);
            word(writer, 11, order);
            writer.write_bits(0x01, 8);
            writer.write_bit(false);
        });
        let codec = codec(order);
        let mut writer = BitWriter::new();
        write_fast_array_delta(
            &mut writer,
            &whole_delta(),
            &layout,
            &codec,
            FAST_ARRAY_COUNT_CAP,
        )
        .unwrap();
        assert_eq!(Payload::from_writer(writer), expected);
        let mut reader = expected.reader();
        assert_eq!(
            read_fast_array_delta(&mut reader, &layout, &codec, FAST_ARRAY_COUNT_CAP).unwrap(),
            whole_delta()
        );
        assert!(reader.at_end());
    }
}

#[test]
fn fast_array_delta_items_are_a_set_bit_and_a_property_stream() {
    let codec = codec(ByteOrder::Little);
    let layout = FastArrayItemLayout::Delta(vec![byte(None)]);
    let delta = FastArrayDelta {
        array_replication_key: 1,
        base_replication_key: 0,
        deleted_ids: vec![],
        changed: vec![(
            4,
            FastArrayItem::Delta(PropertyStream {
                entries: vec![RepEntry {
                    handle: 1,
                    checksum: None,
                    value: RepValue::Byte(0xab),
                }],
                terminator_checksum: None,
            }),
        )],
    };
    let expected = payload(|writer| {
        for value in [1, 0, 0, 1, 4] {
            word(writer, value, ByteOrder::Little);
        }
        writer.write_bit(true);
        writer.write_bits(1 << 1, 8);
        writer.write_bits(0xab, 8);
        writer.write_bits(0, 8);
    });
    let mut writer = BitWriter::new();
    write_fast_array_delta(&mut writer, &delta, &layout, &codec, FAST_ARRAY_COUNT_CAP).unwrap();
    assert_eq!(Payload::from_writer(writer), expected);
    let mut reader = expected.reader();
    assert_eq!(
        read_fast_array_delta(&mut reader, &layout, &codec, FAST_ARRAY_COUNT_CAP).unwrap(),
        delta
    );
    assert!(reader.at_end());
    // A clear bit fails the item.
    let clear = payload(|writer| {
        for value in [1, 0, 0, 1, 4] {
            word(writer, value, ByteOrder::Little);
        }
        writer.write_bit(false);
    });
    assert!(
        read_fast_array_delta(&mut clear.reader(), &layout, &codec, FAST_ARRAY_COUNT_CAP).is_err()
    );
    // Whole items do not fit a delta layout.
    let mut writer = BitWriter::new();
    let mismatch = write_fast_array_delta(
        &mut writer,
        &whole_delta(),
        &layout,
        &codec,
        FAST_ARRAY_COUNT_CAP,
    );
    assert!(mismatch.is_err());
}

#[test]
fn fast_array_counts_fail_closed_when_negative_or_over_the_cap() {
    let codec = codec(ByteOrder::Little);
    let layout = FastArrayItemLayout::Whole(vec![RepCommand::Bool]);
    let header = |deletes: i32, changes: i32| {
        payload(|writer| {
            for value in [0, 0, deletes, changes] {
                word(writer, value, ByteOrder::Little);
            }
        })
    };
    let empty = read_fast_array_delta(&mut header(0, 0).reader(), &layout, &codec, 2).unwrap();
    assert!(empty.deleted_ids.is_empty() && empty.changed.is_empty());
    for (deletes, changes) in [(-1, 0), (0, -1), (3, 0), (0, 3)] {
        let bytes = header(deletes, changes);
        let read = read_fast_array_delta(&mut bytes.reader(), &layout, &codec, 2);
        assert!(read.is_err(), "{deletes} {changes}");
    }
    // Two deletes declared but only one present.
    let truncated = payload(|writer| {
        for value in [0, 0, 2, 0, 9] {
            word(writer, value, ByteOrder::Little);
        }
    });
    assert!(read_fast_array_delta(&mut truncated.reader(), &layout, &codec, 2).is_err());
    let mut over = whole_delta();
    over.deleted_ids = vec![1, 2, 3];
    let whole = FastArrayItemLayout::Whole(vec![byte(None), RepCommand::Bool]);
    assert!(write_fast_array_delta(&mut BitWriter::new(), &over, &whole, &codec, 2).is_err());
    assert_eq!(FAST_ARRAY_COUNT_CAP, 2048);
}

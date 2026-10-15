use icarus_bevy::experimental::actor_channel::{
    ActorChannelCodec, ActorChannelLimits, CompressedRotator, ExportPath, GuidExport, NetGuidRef,
};
use icarus_bevy::experimental::archive::ByteOrder;
use icarus_bevy::experimental::control_messages::ArchiveText;
use icarus_bevy::experimental::packet::{BitReader, BitWriter, Payload};
use icarus_bevy::experimental::rep_layout::{
    NetworkName, PropertyStream, QuantizedNormal, QuantizedVector, RepArray, RepCommand, RepEntry,
    RepLayoutCodec, RepLayoutLimits, RepValue, UnsupportedItem, encode_property_stream,
    parse_property_stream, read_network_name, read_property_stream, validate_layout,
    write_network_name, write_property_stream,
};

// Independently authored bit fixtures from the actor-channel spec (F-571 to
// F-575, F-582), built field by field with the packet bit writer and the
// hand-written helpers below; no captured traffic.

fn codec(version: u32, checksums: bool, order: ByteOrder) -> RepLayoutCodec {
    RepLayoutCodec {
        actor: ActorChannelCodec {
            engine_net_version: version,
            exports_enabled: false,
            has_level: false,
            order,
            limits: ActorChannelLimits::default(),
        },
        checksums,
        limits: RepLayoutLimits::default(),
    }
}

fn plain() -> RepLayoutCodec {
    codec(13, false, ByteOrder::Little)
}

fn text(value: &str) -> ArchiveText {
    ArchiveText::text(value).unwrap()
}

fn bits(writer: &BitWriter) -> (Vec<u8>, usize) {
    (writer.as_bytes().to_vec(), writer.bit_len())
}

fn to_payload(fixture: &(Vec<u8>, usize)) -> Payload {
    let mut writer = BitWriter::new();
    let mut reader = BitReader::new(&fixture.0, fixture.1).unwrap();
    for _ in 0..fixture.1 {
        writer.write_bit(reader.read_bit());
    }
    Payload::from_writer(writer)
}

/// Hand-written packed integer: 7-bit groups, bit 0 continues.
fn packed(writer: &mut BitWriter, mut value: u32) {
    loop {
        let group = value & 0x7f;
        value >>= 7;
        writer.write_bits(group << 1 | u32::from(value != 0), 8);
        if value == 0 {
            break;
        }
    }
}

/// Hand-written `SerializeInt(value, max)`: masks 1, 2, 4, ... while the value
/// so far plus the mask is below `max`.
fn capped(writer: &mut BitWriter, value: u32, max: u32) {
    assert!(value < max);
    let mut mask = 1u64;
    let mut sum = 0u64;
    while sum + mask < u64::from(max) {
        let bit = u64::from(value) & mask != 0;
        writer.write_bit(bit);
        if bit {
            sum += mask;
        }
        mask <<= 1;
    }
}

fn le32(writer: &mut BitWriter, value: u32) {
    writer.write_bytes(&value.to_le_bytes());
}

fn be32(writer: &mut BitWriter, value: u32) {
    writer.write_bytes(&value.to_be_bytes());
}

fn u16_le(writer: &mut BitWriter, value: u16) {
    writer.write_bytes(&value.to_le_bytes());
}

/// Hand-written narrow archive string, little-endian count with terminator.
fn narrow(writer: &mut BitWriter, value: &str) {
    le32(writer, value.len() as u32 + 1);
    writer.write_bytes(value.as_bytes());
    writer.write_bits(0, 8);
}

fn entry(handle: u32, value: RepValue) -> RepEntry {
    RepEntry {
        handle,
        checksum: None,
        value,
    }
}

fn stream(entries: Vec<RepEntry>) -> PropertyStream {
    PropertyStream {
        entries,
        terminator_checksum: None,
    }
}

fn opaque(bit_len: usize, seed: u32) -> Payload {
    let mut writer = BitWriter::new();
    for index in 0..bit_len {
        writer.write_bit((index as u32).wrapping_mul(seed) % 3 == 1);
    }
    Payload::from_writer(writer)
}

fn append(writer: &mut BitWriter, payload: &Payload) {
    let mut reader = payload.reader();
    for _ in 0..payload.bit_len() {
        writer.write_bit(reader.read_bit());
    }
}

/// Exact parse, bit-identical re-encode, every proper prefix fails, and a
/// trailing bit fails the exact parse but not the prefix read.
fn assert_stream(
    fixture: &(Vec<u8>, usize),
    layout: &[RepCommand],
    expected: &PropertyStream,
    codec: &RepLayoutCodec,
) {
    let payload = to_payload(fixture);
    assert_eq!(
        &parse_property_stream(&payload, layout, codec).unwrap(),
        expected
    );
    let encoded = encode_property_stream(expected, layout, codec).unwrap();
    assert_eq!(encoded, payload, "re-encoding differs");
    let mut writer = BitWriter::new();
    writer.write_bit(true);
    write_property_stream(&mut writer, expected, layout, codec).unwrap();
    assert_eq!(writer.bit_len(), fixture.1 + 1);
    for len in 0..fixture.1 {
        let mut reader = BitReader::new(&fixture.0, len).unwrap();
        assert!(
            read_property_stream(&mut reader, layout, codec).is_err(),
            "prefix {len} of {} accepted",
            fixture.1
        );
    }
    let mut longer = BitWriter::new();
    append(&mut longer, &payload);
    longer.write_bit(true);
    let longer = Payload::from_writer(longer);
    let error = parse_property_stream(&longer, layout, codec).unwrap_err();
    assert!(format!("{error:#}").contains("trailing"), "{error:#}");
    let mut reader = longer.reader();
    assert_eq!(
        &read_property_stream(&mut reader, layout, codec).unwrap(),
        expected
    );
    assert_eq!(reader.remaining(), 1);
}

fn error_of<T: std::fmt::Debug>(result: anyhow::Result<T>) -> String {
    format!("{:#}", result.unwrap_err())
}

#[test]
fn empty_stream_is_handle_zero() {
    let mut writer = BitWriter::new();
    packed(&mut writer, 0);
    assert_eq!(bits(&writer), (vec![0], 8));
    assert_stream(&bits(&writer), &[], &PropertyStream::default(), &plain());
    assert_stream(
        &bits(&writer),
        &[RepCommand::Bool],
        &PropertyStream::default(),
        &plain(),
    );
}

fn every_item_layout() -> Vec<RepCommand> {
    vec![
        RepCommand::Bool,
        RepCommand::Byte { enum_bits: None },
        RepCommand::Byte { enum_bits: Some(3) },
        RepCommand::Enum { bits: 11 },
        RepCommand::Object,
        RepCommand::NetworkName,
        RepCommand::VectorQuantize,
        RepCommand::VectorQuantizeNormal,
        RepCommand::Rotator,
        RepCommand::Opaque { bits: 13 },
        RepCommand::Unsupported(UnsupportedItem::Float),
        RepCommand::NetworkName,
    ]
}

#[test]
fn every_item_type_round_trips() {
    let layout = every_item_layout();
    let span = opaque(13, 7);
    let mut writer = BitWriter::new();
    packed(&mut writer, 1);
    writer.write_bit(true);
    packed(&mut writer, 2);
    writer.write_bits(0xa5, 8);
    packed(&mut writer, 3);
    writer.write_bits(0b101, 3);
    packed(&mut writer, 4);
    writer.write_bits(1500, 11);
    packed(&mut writer, 5);
    packed(&mut writer, 300);
    packed(&mut writer, 6);
    writer.write_bit(true);
    packed(&mut writer, 255);
    packed(&mut writer, 7);
    // Bit count 5 below 20 takes four bits; components below 1 << 7 take seven.
    let before = writer.bit_len();
    capped(&mut writer, 5, 20);
    assert_eq!(writer.bit_len() - before, 4);
    for component in [3, 64, 127] {
        writer.write_bits(component, 7);
    }
    packed(&mut writer, 8);
    for value in [32_768, 0, 65_535] {
        writer.write_bits(value, 16);
    }
    packed(&mut writer, 9);
    writer.write_bit(true);
    writer.write_bits(0x4000, 16);
    writer.write_bit(false);
    writer.write_bit(true);
    writer.write_bits(1, 16);
    packed(&mut writer, 10);
    append(&mut writer, &span);
    packed(&mut writer, 12);
    writer.write_bit(false);
    narrow(&mut writer, "Door");
    le32(&mut writer, 7);
    packed(&mut writer, 0);
    let expected = stream(vec![
        entry(1, RepValue::Bool(true)),
        entry(2, RepValue::Byte(0xa5)),
        entry(3, RepValue::Byte(5)),
        entry(4, RepValue::Enum(1500)),
        entry(5, RepValue::Object(NetGuidRef::plain(300))),
        entry(6, RepValue::NetworkName(NetworkName::Hardcoded(255))),
        entry(
            7,
            RepValue::VectorQuantize(QuantizedVector {
                bits: 5,
                components: [3, 64, 127],
            }),
        ),
        entry(
            8,
            RepValue::VectorQuantizeNormal(QuantizedNormal([32_768, 0, 65_535])),
        ),
        entry(
            9,
            RepValue::Rotator(CompressedRotator {
                pitch: Some(0x4000),
                yaw: None,
                roll: Some(1),
            }),
        ),
        entry(10, RepValue::Opaque(span)),
        entry(
            12,
            RepValue::NetworkName(NetworkName::String {
                text: text("Door"),
                number: 7,
            }),
        ),
    ]);
    assert_stream(&bits(&writer), &layout, &expected, &plain());
}

#[test]
fn quantized_helpers_unbias_without_scaling() {
    let vector = QuantizedVector {
        bits: 5,
        components: [3, 64, 127],
    };
    assert_eq!(vector.unbiased(), [-61, 0, 63]);
    assert_eq!(
        QuantizedNormal([32_768, 0, 65_535]).unbiased(),
        [0, -32_768, 32_767]
    );
}

#[test]
fn vector_bit_count_is_below_twenty() {
    let layout = [RepCommand::VectorQuantize];
    // 19 is the largest count: five bits, then components below 1 << 21.
    let mut writer = BitWriter::new();
    packed(&mut writer, 1);
    capped(&mut writer, 19, 20);
    assert_eq!(writer.bit_len(), 13);
    for component in [0, (1 << 21) - 1, 1 << 20] {
        writer.write_bits(component, 21);
    }
    packed(&mut writer, 0);
    let expected = stream(vec![entry(
        1,
        RepValue::VectorQuantize(QuantizedVector {
            bits: 19,
            components: [0, (1 << 21) - 1, 1 << 20],
        }),
    )]);
    assert_stream(&bits(&writer), &layout, &expected, &plain());
    // Bit count 0: components below 4 take two bits each.
    let mut writer = BitWriter::new();
    packed(&mut writer, 1);
    capped(&mut writer, 0, 20);
    for component in [0, 1, 3] {
        writer.write_bits(component, 2);
    }
    packed(&mut writer, 0);
    let expected = stream(vec![entry(
        1,
        RepValue::VectorQuantize(QuantizedVector {
            bits: 0,
            components: [0, 1, 3],
        }),
    )]);
    assert_stream(&bits(&writer), &layout, &expected, &plain());
}

#[test]
fn handles_must_strictly_increase() {
    let layout = [RepCommand::Bool, RepCommand::Byte { enum_bits: Some(2) }];
    // Skipping a handle is a sparse stream, not an ordering error.
    let mut writer = BitWriter::new();
    packed(&mut writer, 2);
    writer.write_bits(3, 2);
    packed(&mut writer, 0);
    let expected = stream(vec![entry(2, RepValue::Byte(3))]);
    assert_stream(&bits(&writer), &layout, &expected, &plain());
    // A descending or repeated handle fails on read and on write.
    for (first, second) in [(2, 1), (1, 1)] {
        let mut writer = BitWriter::new();
        packed(&mut writer, first);
        if first == 2 {
            writer.write_bits(3, 2);
        } else {
            writer.write_bit(true);
        }
        packed(&mut writer, second);
        writer.write_bit(false);
        packed(&mut writer, 0);
        let payload = to_payload(&bits(&writer));
        assert!(
            error_of(parse_property_stream(&payload, &layout, &plain()))
                .contains("does not follow"),
            "{first} then {second}"
        );
        let value = |handle| {
            if handle == 2 {
                RepValue::Byte(3)
            } else {
                RepValue::Bool(true)
            }
        };
        let bad = stream(vec![
            entry(first, value(first)),
            entry(second, value(second)),
        ]);
        assert!(
            error_of(encode_property_stream(&bad, &layout, &plain())).contains("does not follow")
        );
    }
}

#[test]
fn array_handles_address_elements_consecutively() {
    // Two commands per element: element e, command c is handle e * 2 + c + 1.
    let layout = [RepCommand::DynamicArray {
        element: vec![RepCommand::Bool, RepCommand::Byte { enum_bits: Some(2) }],
    }];
    let mut writer = BitWriter::new();
    packed(&mut writer, 1);
    u16_le(&mut writer, 3);
    packed(&mut writer, 2);
    writer.write_bits(1, 2);
    packed(&mut writer, 3);
    writer.write_bit(true);
    packed(&mut writer, 6);
    writer.write_bits(2, 2);
    packed(&mut writer, 0);
    packed(&mut writer, 0);
    let expected = stream(vec![entry(
        1,
        RepValue::DynamicArray(RepArray {
            count: 3,
            stream: stream(vec![
                entry(2, RepValue::Byte(1)),
                entry(3, RepValue::Bool(true)),
                entry(6, RepValue::Byte(2)),
            ]),
        }),
    )]);
    assert_stream(&bits(&writer), &layout, &expected, &plain());
    // Handle 7 would be a fourth element of a three-element array.
    let mut writer = BitWriter::new();
    packed(&mut writer, 1);
    u16_le(&mut writer, 3);
    packed(&mut writer, 7);
    writer.write_bit(true);
    packed(&mut writer, 0);
    packed(&mut writer, 0);
    let payload = to_payload(&bits(&writer));
    assert!(error_of(parse_property_stream(&payload, &layout, &plain())).contains("unknown"));
    let bad = stream(vec![entry(
        1,
        RepValue::DynamicArray(RepArray {
            count: 3,
            stream: stream(vec![entry(7, RepValue::Bool(true))]),
        }),
    )]);
    assert!(error_of(encode_property_stream(&bad, &layout, &plain())).contains("unknown"));
    // A zero-count array addresses no element at all.
    let mut writer = BitWriter::new();
    packed(&mut writer, 1);
    u16_le(&mut writer, 0);
    packed(&mut writer, 1);
    writer.write_bit(true);
    packed(&mut writer, 0);
    packed(&mut writer, 0);
    let payload = to_payload(&bits(&writer));
    assert!(error_of(parse_property_stream(&payload, &layout, &plain())).contains("unknown"));
}

#[test]
fn checksums_follow_every_handle_including_the_terminator() {
    let layout = [
        RepCommand::Bool,
        RepCommand::DynamicArray {
            element: vec![RepCommand::Bool],
        },
    ];
    for order in [ByteOrder::Little, ByteOrder::Big] {
        let word = |writer: &mut BitWriter, value| match order {
            ByteOrder::Little => le32(writer, value),
            ByteOrder::Big => be32(writer, value),
        };
        let mut writer = BitWriter::new();
        packed(&mut writer, 1);
        word(&mut writer, 0x1122_3344);
        writer.write_bit(true);
        packed(&mut writer, 2);
        word(&mut writer, 0x5566_7788);
        match order {
            ByteOrder::Little => writer.write_bytes(&1u16.to_le_bytes()),
            ByteOrder::Big => writer.write_bytes(&1u16.to_be_bytes()),
        }
        packed(&mut writer, 1);
        word(&mut writer, 0x99aa_bbcc);
        writer.write_bit(false);
        packed(&mut writer, 0);
        word(&mut writer, 0xddee_ff00);
        packed(&mut writer, 0);
        word(&mut writer, 0x0102_0304);
        let expected = PropertyStream {
            entries: vec![
                RepEntry {
                    handle: 1,
                    checksum: Some(0x1122_3344),
                    value: RepValue::Bool(true),
                },
                RepEntry {
                    handle: 2,
                    checksum: Some(0x5566_7788),
                    value: RepValue::DynamicArray(RepArray {
                        count: 1,
                        stream: PropertyStream {
                            entries: vec![RepEntry {
                                handle: 1,
                                checksum: Some(0x99aa_bbcc),
                                value: RepValue::Bool(false),
                            }],
                            terminator_checksum: Some(0xddee_ff00),
                        },
                    }),
                },
            ],
            terminator_checksum: Some(0x0102_0304),
        };
        assert_stream(&bits(&writer), &layout, &expected, &codec(13, true, order));
    }
    // An empty stream with checksums is the terminator and one word.
    let mut writer = BitWriter::new();
    packed(&mut writer, 0);
    le32(&mut writer, 0xabad_f00d);
    assert_stream(
        &bits(&writer),
        &layout,
        &PropertyStream {
            entries: Vec::new(),
            terminator_checksum: Some(0xabad_f00d),
        },
        &codec(13, true, ByteOrder::Little),
    );
}

#[test]
fn checksum_presence_must_match_the_codec() {
    let layout = [RepCommand::Bool];
    let with = codec(13, true, ByteOrder::Little);
    let missing = stream(vec![entry(1, RepValue::Bool(true))]);
    assert!(error_of(encode_property_stream(&missing, &layout, &with)).contains("checksum"));
    let mut terminator_only = stream(Vec::new());
    terminator_only.terminator_checksum = Some(1);
    assert!(
        error_of(encode_property_stream(&terminator_only, &layout, &plain())).contains("checksum")
    );
    let entry_only = PropertyStream {
        entries: vec![RepEntry {
            handle: 1,
            checksum: Some(2),
            value: RepValue::Bool(true),
        }],
        terminator_checksum: None,
    };
    assert!(error_of(encode_property_stream(&entry_only, &layout, &plain())).contains("checksum"));
}

fn array_layout() -> Vec<RepCommand> {
    vec![
        RepCommand::DynamicArray {
            element: vec![
                RepCommand::Bool,
                RepCommand::DynamicArray {
                    element: vec![RepCommand::Byte { enum_bits: Some(4) }],
                },
            ],
        },
        RepCommand::Bool,
    ]
}

/// An array of two elements, the first holding a bool and an empty inner array
/// (handles 1 and 2), the second only a one-element inner array (handle 4),
/// then a top-level bool: six entries in all.
fn array_fixture() -> ((Vec<u8>, usize), PropertyStream) {
    let mut writer = BitWriter::new();
    packed(&mut writer, 1);
    u16_le(&mut writer, 2);
    packed(&mut writer, 1);
    writer.write_bit(true);
    packed(&mut writer, 2);
    u16_le(&mut writer, 0);
    packed(&mut writer, 0);
    packed(&mut writer, 4);
    u16_le(&mut writer, 1);
    packed(&mut writer, 1);
    writer.write_bits(9, 4);
    packed(&mut writer, 0);
    packed(&mut writer, 0);
    packed(&mut writer, 2);
    writer.write_bit(false);
    packed(&mut writer, 0);
    let inner = |count, entries| {
        RepValue::DynamicArray(RepArray {
            count,
            stream: stream(entries),
        })
    };
    let expected = stream(vec![
        entry(
            1,
            inner(
                2,
                vec![
                    entry(1, RepValue::Bool(true)),
                    entry(2, inner(0, Vec::new())),
                    entry(4, inner(1, vec![entry(1, RepValue::Byte(9))])),
                ],
            ),
        ),
        entry(2, RepValue::Bool(false)),
    ]);
    (bits(&writer), expected)
}

#[test]
fn nested_arrays_round_trip() {
    let (fixture, expected) = array_fixture();
    assert_eq!(expected.total_entries(), 6);
    assert_stream(&fixture, &array_layout(), &expected, &plain());
}

#[test]
fn empty_array_still_carries_its_terminator() {
    let layout = [RepCommand::DynamicArray {
        element: vec![RepCommand::Bool],
    }];
    let mut writer = BitWriter::new();
    packed(&mut writer, 1);
    u16_le(&mut writer, 0);
    packed(&mut writer, 0);
    packed(&mut writer, 0);
    assert_eq!(writer.bit_len(), 40);
    let expected = stream(vec![entry(
        1,
        RepValue::DynamicArray(RepArray {
            count: 0,
            stream: PropertyStream::default(),
        }),
    )]);
    assert_stream(&bits(&writer), &layout, &expected, &plain());
}

#[test]
fn big_endian_array_count() {
    let layout = [RepCommand::DynamicArray {
        element: vec![RepCommand::Bool],
    }];
    let mut writer = BitWriter::new();
    packed(&mut writer, 1);
    writer.write_bytes(&0x0102u16.to_be_bytes());
    packed(&mut writer, 0);
    packed(&mut writer, 0);
    let expected = stream(vec![entry(
        1,
        RepValue::DynamicArray(RepArray {
            count: 0x0102,
            stream: PropertyStream::default(),
        }),
    )]);
    assert_stream(
        &bits(&writer),
        &layout,
        &expected,
        &codec(13, false, ByteOrder::Big),
    );
}

#[test]
fn unknown_handles_fail_closed() {
    let layout = array_layout();
    for handle in [3, 127, 128, u32::MAX] {
        let mut writer = BitWriter::new();
        packed(&mut writer, handle);
        packed(&mut writer, 0);
        let payload = to_payload(&bits(&writer));
        assert!(
            error_of(parse_property_stream(&payload, &layout, &plain())).contains("unknown"),
            "handle {handle}"
        );
        let bad = stream(vec![entry(handle, RepValue::Bool(true))]);
        assert!(error_of(encode_property_stream(&bad, &layout, &plain())).contains("unknown"));
    }
    // A nested handle above the element command count.
    let mut writer = BitWriter::new();
    packed(&mut writer, 1);
    u16_le(&mut writer, 1);
    packed(&mut writer, 3);
    packed(&mut writer, 0);
    packed(&mut writer, 0);
    let payload = to_payload(&bits(&writer));
    assert!(error_of(parse_property_stream(&payload, &layout, &plain())).contains("unknown"));
}

#[test]
fn unsupported_items_fail_closed() {
    let layout = every_item_layout();
    let mut writer = BitWriter::new();
    packed(&mut writer, 11);
    le32(&mut writer, 0);
    packed(&mut writer, 0);
    let payload = to_payload(&bits(&writer));
    assert!(error_of(parse_property_stream(&payload, &layout, &plain())).contains("unsupported"));
    let bad = stream(vec![entry(11, RepValue::Bool(true))]);
    assert!(error_of(encode_property_stream(&bad, &layout, &plain())).contains("unsupported"));
}

#[test]
fn non_canonical_packed_handle_rejected() {
    let layout = [RepCommand::Bool];
    let mut writer = BitWriter::new();
    // Handle 1 with a needless zero continuation group.
    writer.write_bits(0b11, 8);
    writer.write_bits(0, 8);
    writer.write_bit(true);
    packed(&mut writer, 0);
    let payload = to_payload(&bits(&writer));
    assert!(error_of(parse_property_stream(&payload, &layout, &plain())).contains("non-canonical"));
}

fn assert_name(fixture: &(Vec<u8>, usize), name: &NetworkName, codec: &RepLayoutCodec) {
    let mut reader = BitReader::new(&fixture.0, fixture.1).unwrap();
    assert_eq!(&read_network_name(&mut reader, codec).unwrap(), name);
    assert!(reader.at_end());
    let mut writer = BitWriter::new();
    write_network_name(&mut writer, name, codec).unwrap();
    assert_eq!(bits(&writer), *fixture);
    for len in 0..fixture.1 {
        let mut reader = BitReader::new(&fixture.0, len).unwrap();
        assert!(
            read_network_name(&mut reader, codec).is_err(),
            "prefix {len}"
        );
    }
}

#[test]
fn hardcoded_network_names_by_version() {
    // Net version 6 or later: packed index, at most 702.
    let current = plain();
    let mut writer = BitWriter::new();
    writer.write_bit(true);
    packed(&mut writer, 702);
    assert_name(&bits(&writer), &NetworkName::Hardcoded(702), &current);
    let mut writer = BitWriter::new();
    writer.write_bit(true);
    packed(&mut writer, 703);
    let mut reader = BitReader::new(writer.as_bytes(), writer.bit_len()).unwrap();
    assert!(read_network_name(&mut reader, &current).is_err());
    assert!(
        write_network_name(
            &mut BitWriter::new(),
            &NetworkName::Hardcoded(703),
            &current
        )
        .is_err()
    );
    // Before version 6: a capped integer below 411.
    let legacy = codec(5, false, ByteOrder::Little);
    for index in [0, 1, 255, 410] {
        let mut writer = BitWriter::new();
        writer.write_bit(true);
        capped(&mut writer, index, 411);
        assert_name(&bits(&writer), &NetworkName::Hardcoded(index), &legacy);
    }
    assert!(
        write_network_name(&mut BitWriter::new(), &NetworkName::Hardcoded(411), &legacy).is_err()
    );
}

#[test]
fn string_network_names() {
    // Wide string and number, both byte orders.
    let units = [0xc4u16, 0x41];
    let wide = ArchiveText::new(&units).unwrap();
    for order in [ByteOrder::Little, ByteOrder::Big] {
        let mut writer = BitWriter::new();
        writer.write_bit(false);
        let count = -3i32;
        let terminated = units.iter().copied().chain([0u16]);
        match order {
            ByteOrder::Little => {
                writer.write_bytes(&count.to_le_bytes());
                for unit in terminated {
                    writer.write_bytes(&unit.to_le_bytes());
                }
                writer.write_bytes(&0x0a0b_0c0du32.to_le_bytes());
            }
            ByteOrder::Big => {
                writer.write_bytes(&count.to_be_bytes());
                for unit in terminated {
                    writer.write_bytes(&unit.to_be_bytes());
                }
                writer.write_bytes(&0x0a0b_0c0du32.to_be_bytes());
            }
        }
        assert_name(
            &bits(&writer),
            &NetworkName::String {
                text: wide.clone(),
                number: 0x0a0b_0c0d,
            },
            &codec(13, false, order),
        );
    }
    // The empty string has a zero count and no terminator.
    let mut writer = BitWriter::new();
    writer.write_bit(false);
    le32(&mut writer, 0);
    le32(&mut writer, 0);
    assert_name(
        &bits(&writer),
        &NetworkName::String {
            text: ArchiveText::new(&[]).unwrap(),
            number: 0,
        },
        &plain(),
    );
    // A narrow string without its terminator fails.
    let mut writer = BitWriter::new();
    writer.write_bit(false);
    le32(&mut writer, 2);
    writer.write_bytes(b"AB");
    le32(&mut writer, 0);
    let mut reader = BitReader::new(writer.as_bytes(), writer.bit_len()).unwrap();
    assert!(read_network_name(&mut reader, &plain()).is_err());
}

#[test]
fn layout_validation() {
    let limits = RepLayoutLimits::default();
    assert!(validate_layout(&every_item_layout(), &limits).is_ok());
    for bad in [
        RepCommand::Byte { enum_bits: Some(0) },
        RepCommand::Byte { enum_bits: Some(9) },
        RepCommand::Enum { bits: 0 },
        RepCommand::Enum { bits: 33 },
        RepCommand::Opaque { bits: 0 },
        RepCommand::Opaque {
            bits: limits.max_opaque_bits + 1,
        },
        RepCommand::DynamicArray {
            element: vec![RepCommand::Enum { bits: 0 }],
        },
    ] {
        assert!(
            validate_layout(std::slice::from_ref(&bad), &limits).is_err(),
            "{bad:?}"
        );
        assert!(
            encode_property_stream(&stream(Vec::new()), std::slice::from_ref(&bad), &plain())
                .is_err()
        );
        let payload = to_payload(&(vec![0], 8));
        assert!(parse_property_stream(&payload, &[bad], &plain()).is_err());
    }
    let mut nested = vec![RepCommand::Bool];
    for _ in 0..=limits.max_depth {
        nested = vec![RepCommand::DynamicArray { element: nested }];
    }
    assert!(validate_layout(&nested, &limits).is_err());
    let RepCommand::DynamicArray { element } = &nested[0] else {
        unreachable!()
    };
    assert!(validate_layout(element, &limits).is_ok());
}

fn limited(change: impl Fn(&mut RepLayoutLimits)) -> RepLayoutCodec {
    let mut codec = plain();
    change(&mut codec.limits);
    codec
}

#[test]
fn entry_element_and_depth_limits() {
    let (fixture, expected) = array_fixture();
    let payload = to_payload(&fixture);
    let layout = array_layout();
    // Six entries over every level.
    let exact = limited(|limits| limits.max_entries = 6);
    assert!(parse_property_stream(&payload, &layout, &exact).is_ok());
    assert!(encode_property_stream(&expected, &layout, &exact).is_ok());
    let short = limited(|limits| limits.max_entries = 5);
    assert!(error_of(parse_property_stream(&payload, &layout, &short)).contains("entry limit"));
    assert!(error_of(encode_property_stream(&expected, &layout, &short)).contains("entry limit"));
    // Element counts.
    let one = limited(|limits| limits.max_array_elements = 1);
    assert!(error_of(parse_property_stream(&payload, &layout, &one)).contains("element limit"));
    assert!(error_of(encode_property_stream(&expected, &layout, &one)).contains("element limit"));
    let two = limited(|limits| limits.max_array_elements = 2);
    assert!(parse_property_stream(&payload, &layout, &two).is_ok());
    // Depth: the layout nests two arrays.
    let shallow = limited(|limits| limits.max_depth = 1);
    assert!(error_of(parse_property_stream(&payload, &layout, &shallow)).contains("depth"));
    assert!(error_of(encode_property_stream(&expected, &layout, &shallow)).contains("depth"));
    let deep = limited(|limits| limits.max_depth = 2);
    assert!(parse_property_stream(&payload, &layout, &deep).is_ok());
}

#[test]
fn opaque_and_name_limits() {
    let opaque_layout = [RepCommand::Opaque { bits: 13 }];
    let mut writer = BitWriter::new();
    packed(&mut writer, 1);
    append(&mut writer, &opaque(13, 5));
    packed(&mut writer, 0);
    let opaque_payload = to_payload(&bits(&writer));
    assert!(parse_property_stream(&opaque_payload, &opaque_layout, &plain()).is_ok());
    let narrow_opaque = limited(|limits| limits.max_opaque_bits = 12);
    assert!(parse_property_stream(&opaque_payload, &opaque_layout, &narrow_opaque).is_err());
    // Name units include the terminator.
    let name_layout = [RepCommand::NetworkName];
    let mut writer = BitWriter::new();
    packed(&mut writer, 1);
    writer.write_bit(false);
    narrow(&mut writer, "Door");
    le32(&mut writer, 0);
    packed(&mut writer, 0);
    let name_payload = to_payload(&bits(&writer));
    let named = stream(vec![entry(
        1,
        RepValue::NetworkName(NetworkName::String {
            text: text("Door"),
            number: 0,
        }),
    )]);
    let five = limited(|limits| limits.max_name_units = 5);
    assert_eq!(
        parse_property_stream(&name_payload, &name_layout, &five).unwrap(),
        named
    );
    let four = limited(|limits| limits.max_name_units = 4);
    assert!(
        error_of(parse_property_stream(&name_payload, &name_layout, &four)).contains("unit limit")
    );
    assert!(error_of(encode_property_stream(&named, &name_layout, &four)).contains("unit limit"));
}

#[test]
fn oversized_array_count_rejected_before_its_stream() {
    let layout = [RepCommand::DynamicArray {
        element: vec![RepCommand::Bool],
    }];
    let mut writer = BitWriter::new();
    packed(&mut writer, 1);
    u16_le(&mut writer, u16::MAX);
    // No nested stream follows: the count check must fire first.
    let payload = to_payload(&bits(&writer));
    assert!(error_of(parse_property_stream(&payload, &layout, &plain())).contains("element limit"));
}

#[test]
fn writer_validates_values_against_the_layout() {
    let layout = every_item_layout();
    let bad_values = [
        (1, RepValue::Byte(1), "does not match"),
        (2, RepValue::Bool(true), "does not match"),
        (3, RepValue::Byte(8), "width"),
        (4, RepValue::Enum(1 << 11), "width"),
        (
            7,
            RepValue::VectorQuantize(QuantizedVector {
                bits: 20,
                components: [0; 3],
            }),
            "below 20",
        ),
        (
            7,
            RepValue::VectorQuantize(QuantizedVector {
                bits: 5,
                components: [0, 128, 0],
            }),
            "exceeds its bit count",
        ),
        (10, RepValue::Opaque(opaque(12, 3)), "opaque span length"),
        (
            5,
            RepValue::Object(NetGuidRef {
                guid: 2,
                export: Some(GuidExport {
                    no_load: false,
                    path: None,
                    checksum: None,
                }),
            }),
            "export flags",
        ),
        (0, RepValue::Bool(true), "terminator"),
    ];
    for (handle, value, message) in bad_values {
        let bad = stream(vec![entry(handle, value)]);
        let mut writer = BitWriter::new();
        writer.write_bits(0b101, 3);
        let error = error_of(write_property_stream(&mut writer, &bad, &layout, &plain()));
        assert!(error.contains(message), "{handle}: {error}");
        assert_eq!(bits(&writer), (vec![0b101], 3), "partial write");
    }
    // Enum width 32 accepts every value.
    let wide = [RepCommand::Enum { bits: 32 }];
    let full = stream(vec![entry(1, RepValue::Enum(u32::MAX))]);
    let encoded = encode_property_stream(&full, &wide, &plain()).unwrap();
    assert_eq!(encoded.bit_len(), 8 + 32 + 8);
    assert_eq!(
        parse_property_stream(&encoded, &wide, &plain()).unwrap(),
        full
    );
}

#[test]
fn object_items_carry_export_flags_when_enabled() {
    let mut exports = plain();
    exports.actor.exports_enabled = true;
    let layout = [RepCommand::Object];
    let mut writer = BitWriter::new();
    packed(&mut writer, 1);
    packed(&mut writer, 6);
    writer.write_bits(0b011, 8);
    packed(&mut writer, 0);
    narrow(&mut writer, "Pkg");
    packed(&mut writer, 0);
    let expected = stream(vec![entry(
        1,
        RepValue::Object(NetGuidRef {
            guid: 6,
            export: Some(GuidExport {
                no_load: true,
                path: Some(ExportPath {
                    outer: Box::new(NetGuidRef::null()),
                    path: text("Pkg"),
                }),
                checksum: None,
            }),
        }),
    )]);
    assert_stream(&bits(&writer), &layout, &expected, &exports);
}

#[test]
fn debug_and_summaries_carry_no_secrets() {
    let layout = [
        RepCommand::Bool,
        RepCommand::Enum { bits: 20 },
        RepCommand::Object,
        RepCommand::NetworkName,
        RepCommand::VectorQuantize,
        RepCommand::VectorQuantizeNormal,
        RepCommand::Rotator,
        RepCommand::DynamicArray {
            element: vec![RepCommand::NetworkName],
        },
        RepCommand::Opaque { bits: 32 },
    ];
    let checksum = Some(0xdead_beef);
    let secret = |handle, value| RepEntry {
        handle,
        checksum,
        value,
    };
    let full = PropertyStream {
        entries: vec![
            secret(1, RepValue::Bool(true)),
            secret(2, RepValue::Enum(987_654)),
            secret(3, RepValue::Object(NetGuidRef::plain(123_456_788))),
            secret(4, RepValue::NetworkName(NetworkName::Hardcoded(613))),
            secret(
                5,
                RepValue::VectorQuantize(QuantizedVector {
                    bits: 15,
                    components: [98_765, 0, 1],
                }),
            ),
            secret(
                6,
                RepValue::VectorQuantizeNormal(QuantizedNormal([54_321, 0, 1])),
            ),
            secret(
                7,
                RepValue::Rotator(CompressedRotator {
                    pitch: Some(0x4321),
                    yaw: None,
                    roll: None,
                }),
            ),
            secret(
                8,
                RepValue::DynamicArray(RepArray {
                    count: 1,
                    stream: PropertyStream {
                        entries: vec![secret(
                            1,
                            RepValue::NetworkName(NetworkName::String {
                                text: text("SECRET-NAME"),
                                number: 4_242_424,
                            }),
                        )],
                        terminator_checksum: checksum,
                    },
                }),
            ),
            secret(
                9,
                RepValue::Opaque(Payload::from_writer({
                    let mut writer = BitWriter::new();
                    writer.write_u32(0xfeed_f00d);
                    writer
                })),
            ),
        ],
        terminator_checksum: checksum,
    };
    let codec = codec(13, true, ByteOrder::Little);
    let encoded = encode_property_stream(&full, &layout, &codec).unwrap();
    assert_eq!(
        parse_property_stream(&encoded, &layout, &codec).unwrap(),
        full
    );
    let debug = format!("{full:?}");
    let summary = serde_json::to_string(&full.summary()).unwrap();
    for output in [debug, summary] {
        for leak in [
            "SECRET",
            "deadbeef",
            "DEADBEEF",
            "3735928559",
            "987654",
            "F1206",
            "f1206",
            "123456788",
            "613",
            "98765",
            "54321",
            "4321",
            "17185",
            "4242424",
            "feedf00d",
            "4277006349",
        ] {
            assert!(!output.contains(leak), "{leak} leaked: {output}");
        }
        // Lengths remain: the name's 11 units and the 32-bit span.
        assert!(output.contains("11"), "name length missing: {output}");
        assert!(output.contains("32"), "opaque length missing: {output}");
    }
    let summary = full.summary();
    assert_eq!(summary.entries.len(), 9);
    assert!(summary.terminator_checksum);
    assert_eq!(summary.entries[4].vector_bits, Some(15));
    assert_eq!(summary.entries[8].opaque_bits, Some(32));
}

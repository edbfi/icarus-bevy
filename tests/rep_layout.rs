use icarus_bevy::experimental::actor_channel::{
    ActorChannelCodec, ActorChannelLimits, CompressedRotator, ExportPath, GuidExport, NetGuidRef,
};
use icarus_bevy::experimental::archive::ByteOrder;
use icarus_bevy::experimental::control_messages::{
    ArchiveText, NetIdType, NetIdValue, UniqueNetId,
};
use icarus_bevy::experimental::packet::{BitReader, BitWriter, Payload};
use icarus_bevy::experimental::rep_layout::{
    ACCUMULATION_TYPE_MAX_VALUE, AI_AUDIO_STATE_MAX_VALUE, ALIVE_STATE_MAX_VALUE,
    AUDIO_SHELTER_STATE_MAX_VALUE, COMPONENT_MOBILITY_MAX_VALUE, CREATURE_SEX_MAX_VALUE,
    DYNAMIC_ITEM_PROPERTIES_MAX_VALUE, FLAMMABLE_STATE_MAX_VALUE,
    GAMEPLAY_TAG_CONTAINER_COUNT_BITS, GameplayTag, ICARUS_ITEM_CONTEXT_MAX_VALUE,
    ICARUS_ROW_HANDLE_NONE, ICARUS_ROW_HANDLE_TABLE_COUNT, INTERACT_TYPE_MAX_VALUE,
    MISSION_DIFFICULTY_MAX_VALUE, MOUNT_ACTION_MAX_VALUE, MOUNT_COMBAT_BEHAVIOUR_STATE_MAX_VALUE,
    MOUNT_CONSUMPTION_BEHAVIOUR_STATE_MAX_VALUE, MOUNT_GRAZING_BEHAVIOUR_STATE_MAX_VALUE,
    MOUNT_MOVEMENT_BEHAVIOUR_STATE_MAX_VALUE, MOVEMENT_STATE_MAX_VALUE, MovementLevel,
    NET_ROLE_MAX_VALUE, NetworkName, PROSPECT_LOCATION_MAX_VALUE, PROSPECT_STATE_MAX_VALUE,
    PropertyStream, QuantizedNormal, QuantizedVector, REPLICATED_LEFT_PROSPECT_STATE_MAX_VALUE,
    ROCKET_STATE_MAX_VALUE, RepArray, RepCommand, RepEntry, RepLayoutCodec, RepLayoutLimits,
    RepValue, RotationLevel, RowHandle, SHELTERED_ENUM_MAX_VALUE, StatPairRepState, TagEncoding,
    UnsupportedItem, actionable_component_rep_layout, actor_component_rep_layout, actor_rep_layout,
    actor_state_rep_layout, audio_context_component_rep_layout, bp_building_base_rep_layout,
    bp_flammable_actor_rep_layout, bp_icarus_player_character_survival_rep_layout,
    bp_icarus_player_state_rep_layout, bp_interactable_butcher_animal_rep_layout,
    bp_mount_base_rep_layout, bp_mount_horse_rep_layout, bp_sheltered_component_rep_layout,
    bp_swimming_component_rep_layout, bp_tame_base_rep_layout,
    bp_ui_projection_component_mount_status_rep_layout,
    bp_ui_projection_component_player_rep_layout, bp_ui_projection_component_rep_layout,
    bpc_environmental_buildup_rep_layout, character_rep_layout, character_state_rep_layout,
    child_actor_component_rep_layout, controller_rep_layout, decayable_component_rep_layout,
    durable_component_rep_layout, encode_property_stream, encode_rep_properties, enum_bits,
    enum_byte_bits, enum_max_value, fillable_component_rep_layout, flammable_actor_rep_layout,
    flod_actor_component_rep_layout, focusable_component_rep_layout, game_state_base_rep_layout,
    game_state_rep_layout, genetics_component_rep_layout, icarus_actor_rep_layout,
    icarus_character_rep_layout, icarus_game_state_base_rep_layout,
    icarus_game_state_survival_rep_layout, icarus_item_rep_layout,
    icarus_mount_character_rep_layout, icarus_npc_character_rep_layout,
    icarus_npc_goap_character_rep_layout, icarus_player_character_rep_layout,
    icarus_player_character_survival_rep_layout, icarus_player_controller_rep_layout,
    icarus_player_controller_survival_rep_layout, icarus_player_state_rep_layout,
    icarus_row_handle_width_bytes, icarus_stat_replicated_commands,
    icarus_weather_action_rep_layout, interactable_behaviour_rep_layout, inventory_rep_layout,
    inventory_slot_commands, item_data_commands, itemable_component_rep_layout,
    modifier_state_component_rep_layout, mount_character_state_rep_layout,
    networking_status_commands, parse_property_stream, parse_rep_properties, pawn_rep_layout,
    player_character_state_rep_layout, player_controller_rep_layout, player_state_rep_layout,
    prospect_info_commands, read_network_name, read_property_stream, read_rep_properties,
    read_row_handle, read_stat_pair_rep_state, rep_attachment_commands,
    resource_network_component_rep_layout, scene_component_rep_layout, stat_int_len_bytes,
    static_mesh_component_rep_layout, stomach_component_rep_layout,
    survival_character_state_rep_layout, trait_behaviour_rep_layout, trait_component_rep_layout,
    transform_commands, transmutable_component_rep_layout, validate_layout,
    weather_manager_component_rep_layout, write_network_name, write_property_stream,
    write_rep_properties, write_row_handle, write_stat_pair_rep_state,
};

// Independently authored bit fixtures from the actor-channel spec (F-571 to
// F-575, F-593, F-704), built field by field with the packet bit writer and
// the hand-written helpers below, plus offline capture replay tests against
// M-707 / M-708.

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
        RepCommand::Byte { enum_bits: Some(9) },
        RepCommand::Enum { bits: 33 },
        RepCommand::Opaque { bits: 0 },
        RepCommand::Opaque {
            bits: limits.max_opaque_bits + 1,
        },
        RepCommand::DynamicArray {
            element: vec![RepCommand::Enum { bits: 33 }],
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
                    bare_checksum_flag: false,
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
fn enum_widths_follow_the_client_formula() {
    assert_eq!(enum_max_value(&[]), 0);
    assert_eq!(enum_max_value(&[0, 5, 2]), 5);
    assert_eq!(enum_max_value(&[-3, -1]), -1);
    // The maximum's bit length, capped at 8 for enum bytes.
    for (max, bits) in [
        (0, 0),
        (1, 1),
        (3, 2),
        (4, 3),
        (8, 4),
        (255, 8),
        (256, 8),
        (i64::MAX, 8),
        // A maximum of -1 wraps max + 1 to zero; other negatives are wide.
        (-1, 0),
        (-2, 8),
        (i64::MIN, 8),
    ] {
        assert_eq!(enum_byte_bits(max), bits, "{max}");
    }
    // Enum items cap at the underlying integer's bits instead.
    for (max, bytes, bits) in [
        (0, 1, 0),
        (3, 1, 2),
        (300, 1, 8),
        (300, 2, 9),
        (70_000, 2, 16),
        (70_000, 4, 17),
        (-1, 4, 0),
        (-2, 4, 32),
        (-2, 8, 64),
        (i64::MAX, 8, 63),
    ] {
        assert_eq!(enum_bits(max, bytes), bits, "{max} {bytes}");
    }
}

#[test]
fn zero_width_enum_items_send_no_bits() {
    let layout = [
        RepCommand::Byte {
            enum_bits: Some(enum_byte_bits(0)),
        },
        RepCommand::Enum {
            bits: enum_bits(-1, 1),
        },
    ];
    let zero = stream(vec![
        entry(1, RepValue::Byte(0)),
        entry(2, RepValue::Enum(0)),
    ]);
    let encoded = encode_property_stream(&zero, &layout, &plain()).unwrap();
    // Two handles and the terminator, no item bits.
    assert_eq!(encoded.bit_len(), 8 + 8 + 8);
    assert_eq!(
        parse_property_stream(&encoded, &layout, &plain()).unwrap(),
        zero
    );
    for bad in [
        stream(vec![entry(1, RepValue::Byte(1))]),
        stream(vec![entry(2, RepValue::Enum(1))]),
    ] {
        assert!(encode_property_stream(&bad, &layout, &plain()).is_err());
    }
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
                bare_checksum_flag: false,
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
        RepCommand::Int,
        RepCommand::Float,
        RepCommand::Vector,
        RepCommand::UniqueNetId,
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
            secret(10, RepValue::Int(-77_777_777)),
            secret(11, RepValue::Float(0x4378_0000)),
            secret(
                12,
                RepValue::Vector([0x4378_0000, 0x4040_0000, 0x47ca_2580]),
            ),
            secret(
                13,
                RepValue::UniqueNetId(UniqueNetId {
                    kind: NetIdType::Default,
                    value: NetIdValue::from_hex("76561198000000001").unwrap(),
                }),
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
            "77777777",
            "43780000",
            "47ca2580",
            "76561198",
        ] {
            assert!(!output.contains(leak), "{leak} leaked: {output}");
        }
        // Lengths remain: the name's 11 units and the 32-bit span.
        assert!(output.contains("11"), "name length missing: {output}");
        assert!(output.contains("32"), "opaque length missing: {output}");
    }
    let summary = full.summary();
    assert_eq!(summary.entries.len(), 13);
    assert!(summary.terminator_checksum);
    assert_eq!(summary.entries[4].vector_bits, Some(15));
    assert_eq!(summary.entries[8].opaque_bits, Some(32));
    assert!(summary.entries[12].unique_net_id.is_some());
}

#[test]
fn numeric_and_native_struct_items_round_trip_both_byte_orders() {
    let layout = [
        RepCommand::Int8,
        RepCommand::Int16,
        RepCommand::UInt16,
        RepCommand::Int,
        RepCommand::UInt32,
        RepCommand::Int64,
        RepCommand::UInt64,
        RepCommand::Float,
        RepCommand::Double,
        RepCommand::Vector,
        RepCommand::Vector2D,
        RepCommand::UniqueNetId,
    ];
    let expected = stream(vec![
        entry(1, RepValue::Int8(-42)),
        entry(2, RepValue::Int16(-12_345)),
        entry(3, RepValue::UInt16(54_321)),
        entry(4, RepValue::Int(-1_234_567_890)),
        entry(5, RepValue::UInt32(0x89ab_cdef)),
        entry(6, RepValue::Int64(-0x1234_5678_9abc_def0)),
        entry(7, RepValue::UInt64(0xfedc_ba98_7654_3210)),
        entry(8, RepValue::Float(248.0f32.to_bits())),
        entry(9, RepValue::Double(103_499.5f64.to_bits())),
        entry(
            10,
            RepValue::Vector([248.0f32.to_bits(), 3.0f32.to_bits(), 103_499.0f32.to_bits()]),
        ),
        entry(
            11,
            RepValue::Vector2D([(-12.5f32).to_bits(), 64.25f32.to_bits()]),
        ),
        entry(
            12,
            RepValue::UniqueNetId(UniqueNetId {
                kind: NetIdType::Indexed(9),
                value: NetIdValue::from_hex("0110000100000001").unwrap(),
            }),
        ),
    ]);
    for order in [ByteOrder::Little, ByteOrder::Big] {
        let c = codec(17, false, order);
        let encoded = encode_property_stream(&expected, &layout, &c).unwrap();
        assert_eq!(
            parse_property_stream(&encoded, &layout, &c).unwrap(),
            expected
        );
    }
}

#[test]
fn receive_properties_preamble_governs_checksum_bit() {
    let layout = [RepCommand::Int, RepCommand::Bool];
    let unchecksummed = stream(vec![
        entry(1, RepValue::Int(77)),
        entry(2, RepValue::Bool(true)),
    ]);
    let c_plain = codec(17, false, ByteOrder::Little);
    let payload_no_chk = encode_rep_properties(&unchecksummed, &layout, &c_plain).unwrap();
    // Even if caller passes checksums=true in codec, read_rep_properties overrides it from bit 0.
    let c_with_chk = codec(17, true, ByteOrder::Little);
    assert_eq!(
        parse_rep_properties(&payload_no_chk, &layout, &c_with_chk).unwrap(),
        unchecksummed
    );

    let checksummed = PropertyStream {
        entries: vec![
            RepEntry {
                handle: 1,
                checksum: Some(0x1111_2222),
                value: RepValue::Int(77),
            },
            RepEntry {
                handle: 2,
                checksum: Some(0x3333_4444),
                value: RepValue::Bool(true),
            },
        ],
        terminator_checksum: Some(0x5555_6666),
    };
    let payload_chk = encode_rep_properties(&checksummed, &layout, &c_with_chk).unwrap();
    assert_eq!(
        parse_rep_properties(&payload_chk, &layout, &c_plain).unwrap(),
        checksummed
    );
    let mut writer = BitWriter::new();
    write_rep_properties(&mut writer, &checksummed, &layout, &c_with_chk).unwrap();
    let mut reader = BitReader::new(writer.as_bytes(), writer.bit_len()).unwrap();
    assert_eq!(
        read_rep_properties(&mut reader, &layout, &c_plain).unwrap(),
        checksummed
    );
    assert!(reader.at_end());
}

#[test]
fn native_engine_and_icarus_class_layouts_round_trip() {
    assert_eq!(enum_byte_bits(NET_ROLE_MAX_VALUE), 3);
    assert_eq!(enum_bits(REPLICATED_LEFT_PROSPECT_STATE_MAX_VALUE, 1), 2);
    assert_eq!(rep_attachment_commands().len(), 6);

    let comp_layout = actor_component_rep_layout();
    assert_eq!(comp_layout.len(), 2);

    let ctrl_layout = controller_rep_layout(
        MovementLevel::Whole,
        MovementLevel::Whole,
        RotationLevel::Bytes,
    );
    assert_eq!(ctrl_layout.len(), 17);

    let pc_layout = player_controller_rep_layout(
        MovementLevel::Whole,
        MovementLevel::Whole,
        RotationLevel::Bytes,
    );
    assert_eq!(pc_layout.len(), 19);

    let tag_encoding = TagEncoding::NetIndex {
        first_segment: 0,
        total_bits: 12,
    };
    let ipc_layout = icarus_player_controller_rep_layout(
        MovementLevel::Whole,
        MovementLevel::Whole,
        RotationLevel::Bytes,
        tag_encoding,
    );
    assert_eq!(ipc_layout.len(), 25);
    assert_eq!(GAMEPLAY_TAG_CONTAINER_COUNT_BITS, 6);
    assert_eq!(
        ipc_layout[24],
        RepCommand::GameplayTagContainer {
            encoding: tag_encoding,
            count_bits: 6,
        }
    );
    // Handle 25 (`GameplayTags`): packed handle 25 (8 bits) + empty=0 (1 bit)
    // + 6-bit count (2) + two 12-bit net indices + packed terminator 0 (8 bits) = 47 bits.
    let ipc_tags = stream(vec![entry(
        25,
        RepValue::GameplayTagContainer(Some(vec![
            GameplayTag::NetIndex(7),
            GameplayTag::NetIndex(42),
        ])),
    )]);
    let ipc_payload = encode_property_stream(&ipc_tags, &ipc_layout, &plain()).unwrap();
    assert_eq!(ipc_payload.bit_len(), 8 + 1 + 6 + 12 + 12 + 8);
    assert_eq!(
        parse_property_stream(&ipc_payload, &ipc_layout, &plain()).unwrap(),
        ipc_tags
    );

    let ipcs_layout = icarus_player_controller_survival_rep_layout(
        MovementLevel::Whole,
        MovementLevel::Whole,
        RotationLevel::Bytes,
        tag_encoding,
    );
    assert_eq!(ipcs_layout.len(), 30);

    let ps_layout = player_state_rep_layout(
        MovementLevel::Whole,
        MovementLevel::Whole,
        RotationLevel::Bytes,
    );
    assert_eq!(ps_layout.len(), 26);
    assert_eq!(
        &ps_layout[..15],
        actor_rep_layout(
            MovementLevel::Whole,
            MovementLevel::Whole,
            RotationLevel::Bytes
        )
        .as_slice()
    );
    let ps_stream = stream(vec![
        entry(5, RepValue::Byte(1)),
        entry(14, RepValue::Byte(3)),
        entry(16, RepValue::Float(100.0f32.to_bits())),
        entry(17, RepValue::Int(256)),
        entry(18, RepValue::Byte(42)),
        entry(19, RepValue::Bool(false)),
        entry(20, RepValue::Bool(true)),
        entry(24, RepValue::Int(12)),
        entry(
            25,
            RepValue::UniqueNetId(UniqueNetId {
                kind: NetIdType::Default,
                value: NetIdValue::from_hex("0110000100000001").unwrap(),
            }),
        ),
        entry(26, RepValue::Str(text("Pioneer"))),
    ]);
    let ps_payload = encode_property_stream(&ps_stream, &ps_layout, &plain()).unwrap();
    assert_eq!(
        parse_property_stream(&ps_payload, &ps_layout, &plain()).unwrap(),
        ps_stream
    );
}

#[test]
fn captured_channel_2_player_controller_and_component_blocks_round_trip() {
    // Exact 472-bit Channel 2 open bunch from M-707 / M-708 (F-696, F-704).
    let bunch_bytes = [
        0x04, 0x12, 0x06, 0x4f, 0xd8, 0x04, 0xe8, 0x01, 0x00, 0xdd, 0x95, 0x3f, 0x00, 0x7e, 0x14,
        0xa0, 0x20, 0x8e, 0x81, 0x20, 0x98, 0x00, 0x00, 0xe0, 0x0d, 0x01, 0x00, 0x00, 0x01, 0x01,
        0x96, 0x28, 0x1f, 0xb1, 0x30, 0xb8, 0x40, 0xc0, 0x50, 0x00, 0xc4, 0x90, 0x04, 0x41, 0x00,
        0x28, 0x02, 0x80, 0x11, 0x00, 0x8e, 0x00, 0x80, 0x04, 0x88, 0x24, 0x09, 0x82, 0x00,
    ];
    let mut reader = BitReader::new(&bunch_bytes, 472).unwrap();
    // Skip 97-bit spawn record + 8-bit PlayerController net player index hook (F-696)
    // + 2-bit content block header (bHasRepLayout=1, bIsActor=1) + 16-bit packed payload size (199).
    for _ in 0..123 {
        reader.read_bit();
    }
    let block0_payload = {
        let mut w = BitWriter::new();
        for _ in 0..199 {
            w.write_bit(reader.read_bit());
        }
        Payload::from_writer(w)
    };
    let tag_encoding = TagEncoding::NetIndex {
        first_segment: 0,
        total_bits: 12,
    };
    let layout = icarus_player_controller_survival_rep_layout(
        MovementLevel::Whole,
        MovementLevel::Whole,
        RotationLevel::Bytes,
        tag_encoding,
    );
    let c = codec(17, false, ByteOrder::Little);
    let decoded = parse_rep_properties(&block0_payload, &layout, &c).unwrap();
    let expected = stream(vec![
        entry(5, RepValue::Byte(2)),
        entry(14, RepValue::Byte(3)),
        entry(16, RepValue::Object(NetGuidRef::plain(4))),
        entry(
            19,
            RepValue::Vector([248.0f32.to_bits(), 3.0f32.to_bits(), 103_499.0f32.to_bits()]),
        ),
        entry(22, RepValue::Object(NetGuidRef::plain(6))),
        entry(23, RepValue::Object(NetGuidRef::plain(8))),
        entry(24, RepValue::Object(NetGuidRef::plain(10))),
    ]);
    assert_eq!(decoded, expected);
    assert_eq!(
        encode_rep_properties(&decoded, &layout, &c).unwrap(),
        block0_payload
    );

    // Block 1: subobject NetGUID 6 (`UActorComponent`), 18-bit rep-layout payload.
    // Header: bHasRepLayout=1, bIsActor=0, NetGUID=6 (8 bits), bStablyNamed=1 (1 bit), NumBits=18 (8 bits) = 19 bits.
    for _ in 0..19 {
        reader.read_bit();
    }
    let block1_payload = {
        let mut w = BitWriter::new();
        for _ in 0..18 {
            w.write_bit(reader.read_bit());
        }
        Payload::from_writer(w)
    };
    let comp_layout = actor_component_rep_layout();
    let comp_decoded = parse_rep_properties(&block1_payload, &comp_layout, &c).unwrap();
    let comp_expected = stream(vec![entry(2, RepValue::Bool(true))]);
    assert_eq!(comp_decoded, comp_expected);
    assert_eq!(
        encode_rep_properties(&comp_decoded, &comp_layout, &c).unwrap(),
        block1_payload
    );
}

#[test]
fn row_handle_and_stat_pair_rep_state_round_trip_and_reject_invalid() {
    assert_eq!(
        icarus_row_handle_width_bytes(ICARUS_ROW_HANDLE_NONE).unwrap(),
        0
    );
    assert_eq!(icarus_row_handle_width_bytes(62).unwrap(), 0);
    assert_eq!(icarus_row_handle_width_bytes(0).unwrap(), 2);
    assert_eq!(icarus_row_handle_width_bytes(1).unwrap(), 1);
    assert!(icarus_row_handle_width_bytes(ICARUS_ROW_HANDLE_TABLE_COUNT).is_err());

    let layout = [
        RepCommand::RowHandle,
        RepCommand::RowHandle,
        RepCommand::RowHandle,
        RepCommand::StatPairRepState,
        RepCommand::StatPairRepState,
    ];
    let expected = stream(vec![
        entry(
            1,
            RepValue::RowHandle(RowHandle {
                table_index: ICARUS_ROW_HANDLE_NONE,
                row_index: None,
            }),
        ),
        entry(
            2,
            RepValue::RowHandle(RowHandle {
                table_index: 1,
                row_index: Some(42),
            }),
        ),
        entry(
            3,
            RepValue::RowHandle(RowHandle {
                table_index: 0,
                row_index: Some(512),
            }),
        ),
        entry(
            4,
            RepValue::StatPairRepState(StatPairRepState {
                stat: 0,
                value: 500,
            }),
        ),
        entry(
            5,
            RepValue::StatPairRepState(StatPairRepState {
                stat: 0x0102_0304,
                value: -10,
            }),
        ),
    ]);
    for order in [ByteOrder::Little, ByteOrder::Big] {
        let c = codec(17, false, order);
        let encoded = encode_property_stream(&expected, &layout, &c).unwrap();
        assert_eq!(
            parse_property_stream(&encoded, &layout, &c).unwrap(),
            expected
        );
    }

    assert_eq!(stat_int_len_bytes(0), 1);
    assert_eq!(stat_int_len_bytes(0xfe), 1);
    assert_eq!(stat_int_len_bytes(0xff), 2);
    assert_eq!(stat_int_len_bytes(0xfffe), 2);
    assert_eq!(stat_int_len_bytes(0xffff), 3);
    assert_eq!(stat_int_len_bytes(0xff_fffe), 3);
    assert_eq!(stat_int_len_bytes(0xff_ffff), 4);
    assert_eq!(stat_int_len_bytes(-10i32 as u32), 4);

    // Non-canonical stat integer length (2 bytes for value 1) fails closed.
    let mut bad_stat = BitWriter::new();
    bad_stat.write_bits(2, 8);
    bad_stat.write_bits(0, 8);
    bad_stat.write_bits(1, 8);
    bad_stat.write_bits(1, 8);
    bad_stat.write_bits(1, 8);
    let mut reader = BitReader::new(bad_stat.as_bytes(), bad_stat.bit_len()).unwrap();
    assert!(read_stat_pair_rep_state(&mut reader).is_err());

    // Zero-row table carrying a row index fails closed on write.
    let mut w = BitWriter::new();
    assert!(
        write_row_handle(
            &mut w,
            &RowHandle {
                table_index: 62,
                row_index: Some(0),
            },
            ByteOrder::Little,
        )
        .is_err()
    );
    let _ = read_row_handle;
    let _ = write_stat_pair_rep_state;
}

#[test]
fn native_component_and_state_layouts_round_trip() {
    assert_eq!(enum_bits(AUDIO_SHELTER_STATE_MAX_VALUE, 1), 2);
    assert_eq!(enum_bits(ALIVE_STATE_MAX_VALUE, 1), 1);
    assert_eq!(flod_actor_component_rep_layout().len(), 3);
    assert_eq!(audio_context_component_rep_layout().len(), 3);
    assert_eq!(trait_behaviour_rep_layout().len(), 3);
    assert_eq!(resource_network_component_rep_layout().len(), 4);
    assert_eq!(trait_component_rep_layout().len(), 3);
    assert_eq!(durable_component_rep_layout().len(), 4);
    assert_eq!(itemable_component_rep_layout().len(), 4);
    assert_eq!(decayable_component_rep_layout().len(), 5);
    assert_eq!(fillable_component_rep_layout().len(), 5);
    assert_eq!(transmutable_component_rep_layout().len(), 4);
    assert_eq!(focusable_component_rep_layout().len(), 4);
    assert_eq!(actionable_component_rep_layout().len(), 8);
    assert_eq!(modifier_state_component_rep_layout().len(), 8);
    assert_eq!(actor_state_rep_layout().len(), 12);
    assert_eq!(character_state_rep_layout().len(), 17);
    assert_eq!(survival_character_state_rep_layout().len(), 30);
    assert_eq!(player_character_state_rep_layout().len(), 32);

    let c = codec(17, false, ByteOrder::Little);
    let durable = durable_component_rep_layout();
    let d_stream = stream(vec![
        entry(2, RepValue::Bool(true)),
        entry(
            3,
            RepValue::RowHandle(RowHandle {
                table_index: 65,
                row_index: Some(300),
            }),
        ),
        entry(4, RepValue::Int(2500)),
    ]);
    let d_payload = encode_rep_properties(&d_stream, &durable, &c).unwrap();
    assert_eq!(
        parse_rep_properties(&d_payload, &durable, &c).unwrap(),
        d_stream
    );

    let pcs = player_character_state_rep_layout();
    let pcs_stream = stream(vec![
        entry(3, RepValue::Int(100)),
        entry(4, RepValue::Int(100)),
        entry(7, RepValue::Float(1.0f32.to_bits())),
        entry(11, RepValue::Byte(2)),
        entry(12, RepValue::Enum(0)),
        entry(16, RepValue::Byte(0)),
        entry(17, RepValue::Int(60)),
        entry(31, RepValue::Byte(1)),
        entry(32, RepValue::Int(0)),
    ]);
    let pcs_payload = encode_rep_properties(&pcs_stream, &pcs, &c).unwrap();
    assert_eq!(
        parse_rep_properties(&pcs_payload, &pcs, &c).unwrap(),
        pcs_stream
    );
}

#[test]
fn native_and_cooked_blueprint_actor_and_component_layouts_round_trip() {
    assert_eq!(enum_byte_bits(COMPONENT_MOBILITY_MAX_VALUE), 2);
    assert_eq!(enum_bits(ICARUS_ITEM_CONTEXT_MAX_VALUE, 1), 4);
    assert_eq!(enum_bits(DYNAMIC_ITEM_PROPERTIES_MAX_VALUE, 1), 4);
    assert_eq!(enum_bits(PROSPECT_STATE_MAX_VALUE, 1), 3);
    assert_eq!(enum_bits(PROSPECT_LOCATION_MAX_VALUE, 1), 4);
    assert_eq!(enum_bits(MISSION_DIFFICULTY_MAX_VALUE, 1), 3);
    assert_eq!(enum_bits(MOVEMENT_STATE_MAX_VALUE, 1), 4);
    assert_eq!(enum_bits(MOUNT_COMBAT_BEHAVIOUR_STATE_MAX_VALUE, 1), 3);
    assert_eq!(enum_bits(MOUNT_MOVEMENT_BEHAVIOUR_STATE_MAX_VALUE, 1), 3);
    assert_eq!(enum_bits(MOUNT_CONSUMPTION_BEHAVIOUR_STATE_MAX_VALUE, 1), 3);
    assert_eq!(enum_bits(MOUNT_GRAZING_BEHAVIOUR_STATE_MAX_VALUE, 1), 3);
    assert_eq!(enum_bits(AI_AUDIO_STATE_MAX_VALUE, 1), 4);
    assert_eq!(enum_bits(MOUNT_ACTION_MAX_VALUE, 1), 3);
    assert_eq!(enum_bits(INTERACT_TYPE_MAX_VALUE, 1), 3);
    assert_eq!(enum_bits(CREATURE_SEX_MAX_VALUE, 1), 2);
    assert_eq!(enum_bits(FLAMMABLE_STATE_MAX_VALUE, 1), 3);
    assert_eq!(enum_byte_bits(ACCUMULATION_TYPE_MAX_VALUE), 3);
    assert_eq!(enum_byte_bits(SHELTERED_ENUM_MAX_VALUE), 3);
    assert_eq!(enum_bits(ROCKET_STATE_MAX_VALUE, 1), 3);

    let loc = MovementLevel::Whole;
    let char_loc = MovementLevel::Hundredths;
    let vel = MovementLevel::Whole;
    let rot = RotationLevel::Bytes;
    let tag_encoding = TagEncoding::NetIndex {
        first_segment: 0,
        total_bits: 12,
    };

    assert_eq!(transform_commands().len(), 3);
    assert_eq!(icarus_stat_replicated_commands().len(), 2);
    assert_eq!(item_data_commands(tag_encoding).len(), 13);
    assert_eq!(inventory_slot_commands(tag_encoding).len(), 18);
    assert_eq!(inventory_rep_layout().len(), 9);
    assert_eq!(prospect_info_commands().len(), 17);
    assert_eq!(networking_status_commands().len(), 12);

    assert_eq!(icarus_player_state_rep_layout(loc, vel, rot).len(), 80);
    assert_eq!(bp_icarus_player_state_rep_layout(loc, vel, rot).len(), 84);
    assert_eq!(game_state_base_rep_layout(loc, vel, rot).len(), 19);
    assert_eq!(game_state_rep_layout(loc, vel, rot).len(), 21);
    assert_eq!(icarus_game_state_base_rep_layout(loc, vel, rot).len(), 24);
    assert_eq!(
        icarus_game_state_survival_rep_layout(loc, vel, rot).len(),
        82
    );
    assert_eq!(pawn_rep_layout(char_loc, vel, rot).len(), 18);
    assert_eq!(character_rep_layout(char_loc, vel, rot).len(), 45);
    assert_eq!(
        icarus_character_rep_layout(char_loc, vel, rot, tag_encoding).len(),
        55
    );
    assert_eq!(
        icarus_npc_character_rep_layout(char_loc, vel, rot, tag_encoding).len(),
        67
    );
    assert_eq!(
        icarus_npc_goap_character_rep_layout(char_loc, vel, rot, tag_encoding).len(),
        71
    );
    assert_eq!(
        icarus_mount_character_rep_layout(char_loc, vel, rot, tag_encoding).len(),
        81
    );
    assert_eq!(
        bp_mount_base_rep_layout(char_loc, vel, rot, tag_encoding).len(),
        85
    );
    assert_eq!(
        bp_tame_base_rep_layout(char_loc, vel, rot, tag_encoding).len(),
        86
    );
    assert_eq!(
        bp_mount_horse_rep_layout(char_loc, vel, rot, tag_encoding).len(),
        86
    );
    assert_eq!(
        icarus_player_character_rep_layout(char_loc, vel, rot, tag_encoding).len(),
        66
    );
    assert_eq!(
        icarus_player_character_survival_rep_layout(char_loc, vel, rot, tag_encoding).len(),
        73
    );
    assert_eq!(
        bp_icarus_player_character_survival_rep_layout(char_loc, vel, rot, tag_encoding).len(),
        89
    );
    assert_eq!(
        icarus_actor_rep_layout(loc, vel, rot, tag_encoding).len(),
        18
    );
    assert_eq!(
        icarus_item_rep_layout(loc, vel, rot, tag_encoding).len(),
        40
    );
    assert_eq!(
        bp_building_base_rep_layout(loc, vel, rot, tag_encoding).len(),
        53
    );

    assert_eq!(mount_character_state_rep_layout().len(), 31);
    assert_eq!(weather_manager_component_rep_layout().len(), 3);
    assert_eq!(icarus_weather_action_rep_layout().len(), 7);
    assert_eq!(bp_ui_projection_component_rep_layout().len(), 4);
    assert_eq!(
        bp_ui_projection_component_mount_status_rep_layout().len(),
        5
    );
    assert_eq!(bp_ui_projection_component_player_rep_layout().len(), 5);
    assert_eq!(interactable_behaviour_rep_layout().len(), 7);
    assert_eq!(bp_interactable_butcher_animal_rep_layout().len(), 9);
    assert_eq!(scene_component_rep_layout().len(), 15);
    assert_eq!(child_actor_component_rep_layout().len(), 16);
    assert_eq!(static_mesh_component_rep_layout().len(), 16);
    assert_eq!(bpc_environmental_buildup_rep_layout().len(), 18);
    assert_eq!(flammable_actor_rep_layout().len(), 6);
    assert_eq!(bp_flammable_actor_rep_layout().len(), 7);
    assert_eq!(stomach_component_rep_layout().len(), 3);
    assert_eq!(bp_swimming_component_rep_layout().len(), 4);
    assert_eq!(genetics_component_rep_layout().len(), 10);
    assert_eq!(bp_sheltered_component_rep_layout().len(), 5);

    let c = codec(17, false, ByteOrder::Little);
    let tame_layout = bp_tame_base_rep_layout(char_loc, vel, rot, tag_encoding);
    let tame_stream = stream(vec![
        entry(5, RepValue::Byte(1)),
        entry(14, RepValue::Byte(3)),
        entry(46, RepValue::Int(877_331)),
        entry(56, RepValue::Enum(3)),
        entry(65, RepValue::UInt16(104)),
        entry(68, RepValue::UInt16(104)),
        entry(70, RepValue::Int(25)),
        entry(73, RepValue::Str(text("WolfenSteinert"))),
        entry(77, RepValue::Enum(3)),
        entry(78, RepValue::Enum(2)),
        entry(79, RepValue::Enum(1)),
        entry(82, RepValue::Enum(1)),
        entry(86, RepValue::Object(NetGuidRef::plain(0))),
    ]);
    let tame_payload = encode_rep_properties(&tame_stream, &tame_layout, &c).unwrap();
    assert_eq!(
        parse_rep_properties(&tame_payload, &tame_layout, &c).unwrap(),
        tame_stream
    );
}

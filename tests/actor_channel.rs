use icarus_bevy::experimental::actor_channel::{
    ADAPTIVE_NET_UPDATE_DELAY_THRESHOLD_SECONDS, ADAPTIVE_NET_UPDATE_LERP_SCALE, ActorBunchBody,
    ActorChannelCodec, ActorChannelLimits, ActorPriorityInputs, ActorPriorityOutcome,
    ActorRelevancyInputs, ActorRelevancyOutcome, BlockTarget, CompressedRotator,
    ConsiderListActorInputs, ConsiderListActorOutcome, ContentBlock,
    DEFAULT_CONFIGURED_INTERNET_SPEED, DEFAULT_CONFIGURED_LAN_SPEED,
    DEFAULT_CONNECTION_TIMEOUT_SECONDS, DEFAULT_INI_MAX_CLIENT_RATE,
    DEFAULT_INI_MAX_INTERNET_CLIENT_RATE, DEFAULT_INITIAL_CONNECT_TIMEOUT_SECONDS,
    DEFAULT_MIN_NET_UPDATE_FREQUENCY, DEFAULT_NET_SERVER_MAX_TICK_RATE,
    DEFAULT_RESOLUTION_CONNECTION_TIMEOUT_SECONDS, DESTRUCTION_INFO_BEHIND_FAR_DIST_SQ,
    DESTRUCTION_INFO_BEHIND_NEAR_DIST_SQ, DESTRUCTION_INFO_FAR_FACTOR,
    DESTRUCTION_INFO_FRONT_FAR_DIST_SQ, DESTRUCTION_INFO_NEAR_FACTOR, DynamicSpawn, EngineNetMode,
    ExportPath, GuidExport, GuidKind, MAX_EXPORT_GUIDS, MAX_NET_SERVER_MAX_TICK_RATE,
    MIN_NET_SERVER_MAX_TICK_RATE, NetGuidRef, PREP_CONNECTIONS_RECEIVE_TIMEOUT_SECONDS,
    PRIORITY_SCALE_MULTIPLIER, PROCESS_PRIORITIZED_LOW_FREQ_DELAY_SCALE,
    PROCESS_PRIORITIZED_LOW_FREQ_THRESHOLD, PROCESS_PRIORITIZED_RELEVANCY_FAST_PATH_SECONDS,
    PROCESS_PRIORITIZED_RELEVANT_TIME_JITTER_SCALE, PackageMapState, PawnRelevancyInputs,
    PawnRelevancyOutcome, PrioritizeActorInputs, PrioritizeActorOutcome,
    ProcessPrioritizedActorInputs, ProcessPrioritizedActorOutcome, SpawnRecord, SpawnVector,
    SubObjectClass, dequantize_angle, encode_actor_bunch_body,
    encode_player_controller_open_bunch_body, evaluate_actor_net_priority,
    evaluate_actor_net_relevancy, evaluate_consider_list_actor, evaluate_destruction_info_priority,
    evaluate_game_engine_max_tick_rate, evaluate_mark_relevant_actor,
    evaluate_matinee_actor_net_priority, evaluate_online_beacon_net_relevancy,
    evaluate_pawn_net_relevancy, evaluate_player_controller_net_priority,
    evaluate_player_controller_net_relevancy, evaluate_prep_connections_tick_budget,
    evaluate_prioritize_actor, evaluate_process_prioritized_actor, guid_kind,
    is_connection_ready_for_replication, is_within_net_relevancy_distance, parse_actor_bunch,
    parse_player_controller_open_bunch, quantize_angle, read_actor_bunch_body, read_content_block,
    read_export_prefix, read_guid_ref, read_must_be_mapped_guids,
    read_player_controller_open_bunch_body, read_rotator, read_spawn, round_net_priority_to_int,
    write_actor_bunch_body, write_content_block, write_export_prefix, write_guid_ref,
    write_must_be_mapped_guids, write_player_controller_open_bunch_body, write_rotator,
    write_spawn,
};
use icarus_bevy::experimental::archive::ByteOrder;
use icarus_bevy::experimental::control_messages::ArchiveText;
use icarus_bevy::experimental::packet::{BitReader, BitWriter, Bunch, BunchHeader, Payload};

// Independently authored bit fixtures from the actor-channel spec (F-569 to
// F-573), built field by field with the packet bit writer; no captured traffic.

fn codec(version: u32, exports: bool, level: bool) -> ActorChannelCodec {
    ActorChannelCodec {
        engine_net_version: version,
        exports_enabled: exports,
        has_level: level,
        order: ByteOrder::Little,
        limits: ActorChannelLimits::default(),
    }
}

fn text(value: &str) -> ArchiveText {
    ArchiveText::text(value).unwrap()
}

/// Bits of a writer as (bytes, bit length).
fn bits(writer: &BitWriter) -> (Vec<u8>, usize) {
    (writer.as_bytes().to_vec(), writer.bit_len())
}

fn reader(fixture: &(Vec<u8>, usize)) -> BitReader<'_> {
    BitReader::new(&fixture.0, fixture.1).unwrap()
}

fn prefix(fixture: &(Vec<u8>, usize), len: usize) -> BitReader<'_> {
    BitReader::new(&fixture.0, len).unwrap()
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

/// Hand-written narrow archive string, little-endian count with terminator.
fn narrow(writer: &mut BitWriter, value: &str) {
    writer.write_u32(value.len() as u32 + 1);
    writer.write_bytes(value.as_bytes());
    writer.write_bits(0, 8);
}

fn payload(bit_len: usize, seed: u32) -> Payload {
    let mut writer = BitWriter::new();
    for index in 0..bit_len {
        writer.write_bit((index as u32).wrapping_mul(seed) % 3 == 1);
    }
    Payload::from_writer(writer)
}

fn assert_guid(fixture: &(Vec<u8>, usize), expected: &NetGuidRef, codec: &ActorChannelCodec) {
    let mut input = reader(fixture);
    assert_eq!(&read_guid_ref(&mut input, codec).unwrap(), expected);
    assert!(input.at_end(), "unread bits");
    let mut output = BitWriter::new();
    write_guid_ref(&mut output, expected, codec).unwrap();
    assert_eq!(bits(&output), *fixture);
    for len in 0..fixture.1 {
        assert!(
            read_guid_ref(&mut prefix(fixture, len), codec).is_err(),
            "prefix {len} accepted"
        );
    }
}

#[test]
fn guid_kinds() {
    assert_eq!(guid_kind(0), GuidKind::Null);
    assert_eq!(guid_kind(1), GuidKind::Static);
    assert_eq!(guid_kind(7), GuidKind::Static);
    assert_eq!(guid_kind(2), GuidKind::Dynamic);
    assert_eq!(guid_kind(300), GuidKind::Dynamic);
}

#[test]
fn plain_guids_round_trip() {
    let plain = codec(13, false, false);
    // Static 5: one byte 5 << 1.
    assert_guid(&(vec![0x0a], 8), &NetGuidRef::plain(5), &plain);
    // Dynamic 300: groups 44 (continued) and 2.
    assert_guid(&(vec![89, 4], 16), &NetGuidRef::plain(300), &plain);
    // Null reads no export byte even with exports enabled.
    assert_guid(&(vec![0], 8), &NetGuidRef::null(), &codec(13, true, false));
    // Five-group maximum.
    let mut writer = BitWriter::new();
    packed(&mut writer, u32::MAX);
    assert_eq!(writer.bit_len(), 40);
    assert_guid(&bits(&writer), &NetGuidRef::plain(u32::MAX), &plain);
}

#[test]
fn default_guid_always_has_export_flags() {
    let plain = codec(13, false, false);
    let flags_only = |no_load| NetGuidRef {
        guid: 1,
        export: Some(GuidExport {
            no_load,
            path: None,
            checksum: None,
            bare_checksum_flag: false,
        }),
    };
    assert_guid(&(vec![0x02, 0x00], 16), &flags_only(false), &plain);
    assert_guid(&(vec![0x02, 0x02], 16), &flags_only(true), &plain);
}

#[test]
fn exports_with_and_without_path_and_checksum() {
    let exports = codec(13, true, false);
    // GUID 6, flags path, outer 3 (flags 0), path "/A".
    let mut writer = BitWriter::new();
    packed(&mut writer, 6);
    writer.write_bits(0x01, 8);
    packed(&mut writer, 3);
    writer.write_bits(0x00, 8);
    narrow(&mut writer, "/A");
    let outer = NetGuidRef {
        guid: 3,
        export: Some(GuidExport {
            no_load: false,
            path: None,
            checksum: None,
            bare_checksum_flag: false,
        }),
    };
    let with_path = NetGuidRef {
        guid: 6,
        export: Some(GuidExport {
            no_load: false,
            path: Some(ExportPath {
                outer: Box::new(outer.clone()),
                path: text("/A"),
            }),
            checksum: None,
            bare_checksum_flag: false,
        }),
    };
    assert_guid(&bits(&writer), &with_path, &exports);

    // Path, no-load and checksum; a null outer reads no flags.
    let mut writer = BitWriter::new();
    packed(&mut writer, 6);
    writer.write_bits(0x07, 8);
    packed(&mut writer, 0);
    narrow(&mut writer, "/Game/X");
    writer.write_u32(0xdead_beef);
    let full = NetGuidRef {
        guid: 6,
        export: Some(GuidExport {
            no_load: true,
            path: Some(ExportPath {
                outer: Box::new(NetGuidRef::null()),
                path: text("/Game/X"),
            }),
            checksum: Some(0xdead_beef),
            bare_checksum_flag: false,
        }),
    };
    assert_guid(&bits(&writer), &full, &exports);

    // Big-endian archive: count and checksum swapped; wide path.
    let mut big = exports;
    big.order = ByteOrder::Big;
    let mut writer = BitWriter::new();
    packed(&mut writer, 6);
    writer.write_bits(0x05, 8);
    packed(&mut writer, 0);
    writer.write_bytes(&(-2i32).to_be_bytes());
    writer.write_bytes(&[0x01, 0x00, 0x00, 0x00]);
    writer.write_bytes(&0x0102_0304u32.to_be_bytes());
    let wide = NetGuidRef {
        guid: 6,
        export: Some(GuidExport {
            no_load: false,
            path: Some(ExportPath {
                outer: Box::new(NetGuidRef::null()),
                path: ArchiveText::new(&[0x100]).unwrap(),
            }),
            checksum: Some(0x0102_0304),
            bare_checksum_flag: false,
        }),
    };
    assert_guid(&bits(&writer), &wide, &big);
}

#[test]
fn unsupported_export_flags_fail_closed() {
    let exports = codec(13, true, false);
    for flags in [0x08u8, 0x80] {
        let fixture = (vec![0x02, flags], 16);
        assert!(read_guid_ref(&mut reader(&fixture), &exports).is_err());
    }
    let checksum_only = NetGuidRef {
        guid: 1,
        export: Some(GuidExport {
            no_load: false,
            path: None,
            checksum: Some(1),
            bare_checksum_flag: false,
        }),
    };
    assert!(write_guid_ref(&mut BitWriter::new(), &checksum_only, &exports).is_err());
    // Export presence must match the codec.
    assert!(write_guid_ref(&mut BitWriter::new(), &NetGuidRef::plain(6), &exports).is_err());
    assert!(write_guid_ref(&mut BitWriter::new(), &NetGuidRef::plain(1), &exports).is_err());
    let flagged = NetGuidRef {
        guid: 6,
        export: Some(GuidExport {
            no_load: false,
            path: None,
            checksum: None,
            bare_checksum_flag: false,
        }),
    };
    assert!(write_guid_ref(&mut BitWriter::new(), &flagged, &codec(13, false, false)).is_err());
}

/// The original server sends export flag bit 2 without the path bit, and no
/// checksum follows it (F-692).
#[test]
fn bare_checksum_flag_carries_no_checksum() {
    let exports = codec(13, true, false);
    for (flags, no_load) in [(0x04u8, false), (0x06, true)] {
        let fixture = (vec![0x02, flags], 16);
        let mut input = reader(&fixture);
        let reference = read_guid_ref(&mut input, &exports).unwrap();
        assert_eq!(input.remaining(), 0);
        let export = reference.export.as_ref().unwrap();
        assert!(export.bare_checksum_flag && export.checksum.is_none() && export.path.is_none());
        assert_eq!(export.no_load, no_load);
        let mut writer = BitWriter::new();
        write_guid_ref(&mut writer, &reference, &exports).unwrap();
        assert_eq!(bits(&writer), fixture);
    }
    let both = NetGuidRef {
        guid: 1,
        export: Some(GuidExport {
            no_load: false,
            path: Some(ExportPath {
                outer: Box::new(NetGuidRef::null()),
                path: text(""),
            }),
            checksum: Some(1),
            bare_checksum_flag: true,
        }),
    };
    assert!(write_guid_ref(&mut BitWriter::new(), &both, &exports).is_err());
}

#[test]
fn non_canonical_packed_guid_rejected() {
    let plain = codec(13, false, false);
    // 5 with a redundant zero continuation group.
    assert!(read_guid_ref(&mut reader(&(vec![0x0b, 0x00], 16)), &plain).is_err());
    // Fifth group carrying bits above 32.
    let fixture = (vec![0xff, 0xff, 0xff, 0xff, 0xfe], 40);
    assert!(read_guid_ref(&mut reader(&fixture), &plain).is_err());
}

/// GUID 1 chain: each level has flags 0x01, an outer and an empty path.
fn chain(outers: usize) -> ((Vec<u8>, usize), NetGuidRef) {
    fn write(writer: &mut BitWriter, left: usize) {
        packed(writer, 1);
        if left == 0 {
            writer.write_bits(0, 8);
            return;
        }
        writer.write_bits(1, 8);
        write(writer, left - 1);
        writer.write_u32(0);
    }
    let mut reference = NetGuidRef {
        guid: 1,
        export: Some(GuidExport {
            no_load: false,
            path: None,
            checksum: None,
            bare_checksum_flag: false,
        }),
    };
    for _ in 0..outers {
        reference = NetGuidRef {
            guid: 1,
            export: Some(GuidExport {
                no_load: false,
                path: Some(ExportPath {
                    outer: Box::new(reference),
                    path: text(""),
                }),
                checksum: None,
                bare_checksum_flag: false,
            }),
        };
    }
    let mut writer = BitWriter::new();
    write(&mut writer, outers);
    (bits(&writer), reference)
}

#[test]
fn guid_depth_cap() {
    let plain = codec(13, false, false);
    let (fixture, reference) = chain(16);
    assert_guid(&fixture, &reference, &plain);
    assert!(reference.summary().outer.is_some());
    let (fixture, reference) = chain(17);
    assert!(read_guid_ref(&mut reader(&fixture), &plain).is_err());
    assert!(write_guid_ref(&mut BitWriter::new(), &reference, &plain).is_err());
}

/// Version-dependent hand encoding of a spawn vector body.
fn write_vector(writer: &mut BitWriter, version: u32, vector: &SpawnVector) {
    match vector {
        SpawnVector::Raw(raw) => {
            assert!(version >= 13);
            writer.write_bit(false);
            for value in raw {
                writer.write_u32(*value);
            }
        }
        SpawnVector::Quantized { bits, components } => {
            if version >= 13 {
                writer.write_bit(true);
            }
            writer.write_int_max(*bits, 24).unwrap();
            for value in components {
                writer.write_int_max(*value, 1 << (bits + 2)).unwrap();
            }
        }
    }
}

fn assert_spawn(fixture: &(Vec<u8>, usize), expected: &SpawnRecord, codec: &ActorChannelCodec) {
    let mut input = reader(fixture);
    assert_eq!(&read_spawn(&mut input, codec).unwrap(), expected);
    assert!(input.at_end(), "unread bits");
    let mut output = BitWriter::new();
    write_spawn(&mut output, expected, codec).unwrap();
    assert_eq!(bits(&output), *fixture);
    for len in 0..fixture.1 {
        assert!(
            read_spawn(&mut prefix(fixture, len), codec).is_err(),
            "prefix {len} accepted"
        );
    }
}

#[test]
fn static_spawn_has_only_the_actor() {
    let plain = codec(13, false, true);
    assert_spawn(
        &(vec![0x0a], 8),
        &SpawnRecord {
            actor: NetGuidRef::plain(5),
            dynamic: None,
        },
        &plain,
    );
    assert_spawn(
        &(vec![0x00], 8),
        &SpawnRecord {
            actor: NetGuidRef::null(),
            dynamic: None,
        },
        &plain,
    );
}

#[test]
fn every_dynamic_spawn_presence_combination() {
    let location = SpawnVector::Quantized {
        bits: 3,
        components: [1, 2, 31],
    };
    let rotation = CompressedRotator {
        pitch: Some(0x1234),
        yaw: None,
        roll: Some(1),
    };
    let velocity = SpawnVector::Quantized {
        bits: 0,
        components: [0, 3, 2],
    };
    for version in [12u32, 13] {
        let scale = if version >= 13 {
            SpawnVector::Raw([1.0f32.to_bits(), 2.5f32.to_bits(), (-1.0f32).to_bits()])
        } else {
            SpawnVector::Quantized {
                bits: 23,
                components: [0, (1 << 25) - 1, 1 << 24],
            }
        };
        for level in [false, true] {
            for mask in 0u8..16 {
                let codec = codec(version, false, level);
                let mut writer = BitWriter::new();
                packed(&mut writer, 2);
                packed(&mut writer, 9);
                if level {
                    packed(&mut writer, 11);
                }
                let present = |bit: u8| mask & (1 << bit) != 0;
                writer.write_bit(present(0));
                if present(0) {
                    write_vector(&mut writer, version, &location);
                }
                writer.write_bit(present(1));
                if present(1) {
                    writer.write_bit(true);
                    writer.write_bits(0x1234, 16);
                    writer.write_bit(false);
                    writer.write_bit(true);
                    writer.write_bits(1, 16);
                }
                writer.write_bit(present(2));
                if present(2) {
                    write_vector(&mut writer, version, &scale);
                }
                writer.write_bit(present(3));
                if present(3) {
                    write_vector(&mut writer, version, &velocity);
                }
                let expected = SpawnRecord {
                    actor: NetGuidRef::plain(2),
                    dynamic: Some(DynamicSpawn {
                        archetype: NetGuidRef::plain(9),
                        level: level.then(|| NetGuidRef::plain(11)),
                        location: present(0).then_some(location),
                        rotation: present(1).then_some(rotation),
                        scale: present(2).then_some(scale),
                        velocity: present(3).then_some(velocity),
                    }),
                };
                assert_spawn(&bits(&writer), &expected, &codec);
            }
        }
    }
}

#[test]
fn quantized_vector_bit_layout_by_hand() {
    // Version 13: presence, quantized, bits 3 (capped: 1,1,0,0,0), then three
    // five-bit components 1, 2, 31.
    let mut writer = BitWriter::new();
    packed(&mut writer, 2);
    packed(&mut writer, 9);
    for bit in [1, 1, 1, 1, 0, 0, 0] {
        writer.write_bit(bit == 1);
    }
    for value in [1u32, 2, 31] {
        writer.write_bits(value, 5);
    }
    for _ in 0..3 {
        writer.write_bit(false);
    }
    let record = read_spawn(&mut reader(&bits(&writer)), &codec(13, false, false)).unwrap();
    let location = record.dynamic.unwrap().location.unwrap();
    assert_eq!(
        location,
        SpawnVector::Quantized {
            bits: 3,
            components: [1, 2, 31]
        }
    );
}

#[test]
fn vector_and_rotator_decoding() {
    let vector = SpawnVector::Quantized {
        bits: 3,
        components: [26, 16, 0],
    };
    let [x, y, z] = vector.decode();
    assert!((x - 1.0).abs() < 1e-6);
    assert_eq!(y, 0.0);
    assert!((z + 1.6).abs() < 1e-6);
    let raw = SpawnVector::Raw([1.5f32.to_bits(), 0, (-2.0f32).to_bits()]);
    assert_eq!(raw.decode(), [1.5, 0.0, -2.0]);

    assert_eq!(quantize_angle(0.0).unwrap(), 0);
    assert_eq!(quantize_angle(90.0).unwrap(), 16384);
    assert_eq!(quantize_angle(-90.0).unwrap(), 49152);
    assert_eq!(quantize_angle(360.0).unwrap(), 0);
    assert!(quantize_angle(f32::NAN).is_err());
    assert_eq!(dequantize_angle(16384), 90.0);
    let rotator = CompressedRotator::from_degrees(0.0, 90.0, -90.0).unwrap();
    assert_eq!(
        rotator,
        CompressedRotator {
            pitch: None,
            yaw: Some(16384),
            roll: Some(49152),
        }
    );
    assert_eq!(rotator.degrees(), [0.0, 90.0, 270.0]);
}

#[test]
fn rotator_round_trip_preserves_a_set_zero_axis() {
    let rotator = CompressedRotator {
        pitch: Some(0),
        yaw: Some(0xbeef),
        roll: None,
    };
    let mut writer = BitWriter::new();
    writer.write_bit(true);
    writer.write_bits(0, 16);
    writer.write_bit(true);
    writer.write_bytes(&0xbeefu16.to_le_bytes());
    writer.write_bit(false);
    let fixture = bits(&writer);
    let mut input = reader(&fixture);
    assert_eq!(read_rotator(&mut input).unwrap(), rotator);
    assert!(input.at_end());
    let mut output = BitWriter::new();
    write_rotator(&mut output, &rotator);
    assert_eq!(bits(&output), fixture);
    for len in 0..fixture.1 {
        assert!(read_rotator(&mut prefix(&fixture, len)).is_err());
    }
}

#[test]
fn spawn_writer_validation() {
    let v12 = codec(12, false, false);
    let raw = SpawnVector::Raw([0; 3]);
    let dynamic = |location| SpawnRecord {
        actor: NetGuidRef::plain(2),
        dynamic: Some(DynamicSpawn {
            archetype: NetGuidRef::plain(9),
            level: None,
            location: Some(location),
            rotation: None,
            scale: None,
            velocity: None,
        }),
    };
    assert!(write_spawn(&mut BitWriter::new(), &dynamic(raw), &v12).is_err());
    let too_wide = SpawnVector::Quantized {
        bits: 24,
        components: [0; 3],
    };
    assert!(write_spawn(&mut BitWriter::new(), &dynamic(too_wide), &v12).is_err());
    let too_big = SpawnVector::Quantized {
        bits: 1,
        components: [8, 0, 0],
    };
    assert!(write_spawn(&mut BitWriter::new(), &dynamic(too_big), &v12).is_err());
    // Level presence must match the codec.
    assert!(
        write_spawn(
            &mut BitWriter::new(),
            &dynamic(SpawnVector::Raw([0; 3])),
            &codec(13, false, true)
        )
        .is_err()
    );
    // Static actors carry no dynamic fields; dynamic actors need them.
    let mut wrong = dynamic(raw);
    wrong.actor = NetGuidRef::plain(3);
    assert!(write_spawn(&mut BitWriter::new(), &wrong, &codec(13, false, false)).is_err());
    let missing = SpawnRecord {
        actor: NetGuidRef::plain(2),
        dynamic: None,
    };
    assert!(write_spawn(&mut BitWriter::new(), &missing, &v12).is_err());
}

fn assert_block(fixture: &(Vec<u8>, usize), expected: &ContentBlock, codec: &ActorChannelCodec) {
    let mut input = reader(fixture);
    assert_eq!(&read_content_block(&mut input, codec).unwrap(), expected);
    assert!(input.at_end(), "unread bits");
    let mut output = BitWriter::new();
    write_content_block(&mut output, expected, codec).unwrap();
    assert_eq!(bits(&output), *fixture);
    for len in 0..fixture.1 {
        assert!(
            read_content_block(&mut prefix(fixture, len), codec).is_err(),
            "prefix {len} accepted"
        );
    }
}

fn append(writer: &mut BitWriter, payload: &Payload) {
    let mut input = payload.reader();
    for _ in 0..payload.bit_len() {
        writer.write_bit(input.read_bit());
    }
}

#[test]
fn actor_block_payload_spans() {
    let plain = codec(13, false, false);
    for (span, layout) in [(0, false), (1, true), (33, true), (70, false), (300, true)] {
        let data = payload(span, 7);
        let mut writer = BitWriter::new();
        writer.write_bit(layout);
        writer.write_bit(true);
        packed(&mut writer, span as u32);
        append(&mut writer, &data);
        assert_block(
            &bits(&writer),
            &ContentBlock {
                has_rep_layout: layout,
                target: BlockTarget::Actor,
                payload: data,
            },
            &plain,
        );
    }
}

#[test]
fn sub_object_blocks() {
    for exports in [false, true] {
        let codec = codec(13, exports, false);
        let flags = |guid: u32| {
            exports
                .then_some(GuidExport {
                    no_load: false,
                    path: None,
                    checksum: None,
                    bare_checksum_flag: false,
                })
                .filter(|_| guid != 0)
        };
        let guid = |writer: &mut BitWriter, value: u32| {
            packed(writer, value);
            if exports && value != 0 {
                writer.write_bits(0, 8);
            }
        };
        let data = payload(12, 5);
        for case in 0..3 {
            let mut writer = BitWriter::new();
            writer.write_bit(true);
            writer.write_bit(false);
            guid(&mut writer, 8);
            let class = match case {
                0 => {
                    writer.write_bit(true);
                    SubObjectClass::StablyNamed
                }
                1 => {
                    writer.write_bit(false);
                    guid(&mut writer, 9);
                    SubObjectClass::Class(NetGuidRef {
                        guid: 9,
                        export: flags(9),
                    })
                }
                _ => {
                    writer.write_bit(false);
                    guid(&mut writer, 0);
                    SubObjectClass::Delete
                }
            };
            // A deletion block carries no payload count or payload (M-425).
            let deletion = matches!(class, SubObjectClass::Delete);
            let expected = if deletion {
                payload(0, 1)
            } else {
                packed(&mut writer, 12);
                append(&mut writer, &data);
                data.clone()
            };
            assert_block(
                &bits(&writer),
                &ContentBlock {
                    has_rep_layout: true,
                    target: BlockTarget::SubObject {
                        object: NetGuidRef {
                            guid: 8,
                            export: flags(8),
                        },
                        class,
                    },
                    payload: expected,
                },
                &codec,
            );
        }
    }
    let zero_class = ContentBlock {
        has_rep_layout: false,
        target: BlockTarget::SubObject {
            object: NetGuidRef::plain(8),
            class: SubObjectClass::Class(NetGuidRef::null()),
        },
        payload: payload(0, 1),
    };
    assert!(
        write_content_block(&mut BitWriter::new(), &zero_class, &codec(13, false, false)).is_err()
    );
    let deletion_with_payload = ContentBlock {
        has_rep_layout: false,
        target: BlockTarget::SubObject {
            object: NetGuidRef::plain(8),
            class: SubObjectClass::Delete,
        },
        payload: payload(3, 1),
    };
    assert!(
        write_content_block(
            &mut BitWriter::new(),
            &deletion_with_payload,
            &codec(13, false, false)
        )
        .is_err()
    );
}

/// An open bunch body: static spawn, then three blocks. Returns the fixture,
/// the block boundary bit positions and the expected body.
fn body_fixture() -> ((Vec<u8>, usize), Vec<usize>, ActorBunchBody) {
    let mut writer = BitWriter::new();
    packed(&mut writer, 5);
    let mut boundaries = vec![writer.bit_len()];
    let spans = [(true, 9usize), (false, 0), (true, 40)];
    let mut blocks = Vec::new();
    for (index, (layout, span)) in spans.into_iter().enumerate() {
        let data = payload(span, 11 + index as u32);
        writer.write_bit(layout);
        if index == 1 {
            writer.write_bit(false);
            packed(&mut writer, 7);
            writer.write_bit(true);
            blocks.push(ContentBlock {
                has_rep_layout: layout,
                target: BlockTarget::SubObject {
                    object: NetGuidRef::plain(7),
                    class: SubObjectClass::StablyNamed,
                },
                payload: data.clone(),
            });
        } else {
            writer.write_bit(true);
            blocks.push(ContentBlock {
                has_rep_layout: layout,
                target: BlockTarget::Actor,
                payload: data.clone(),
            });
        }
        packed(&mut writer, span as u32);
        append(&mut writer, &data);
        boundaries.push(writer.bit_len());
    }
    let body = ActorBunchBody {
        spawn: Some(SpawnRecord {
            actor: NetGuidRef::plain(5),
            dynamic: None,
        }),
        blocks,
    };
    (bits(&writer), boundaries, body)
}

#[test]
fn whole_body_round_trip_and_truncation() {
    let plain = codec(13, false, false);
    let (fixture, boundaries, body) = body_fixture();
    let mut input = reader(&fixture);
    assert_eq!(
        read_actor_bunch_body(&mut input, true, &plain).unwrap(),
        body
    );
    let encoded = encode_actor_bunch_body(&body, &plain).unwrap();
    assert_eq!(
        (encoded.bytes().to_vec(), encoded.bit_len()),
        fixture.clone()
    );
    for len in 0..fixture.1 {
        let result = read_actor_bunch_body(&mut prefix(&fixture, len), true, &plain);
        match boundaries.iter().position(|boundary| *boundary == len) {
            // Ending exactly between blocks is a shorter valid body.
            Some(count) => assert_eq!(result.unwrap().blocks.len(), count),
            None => assert!(result.is_err(), "prefix {len} accepted"),
        }
    }
    // Without the open flag the same bits are not a block stream.
    assert!(read_actor_bunch_body(&mut reader(&fixture), false, &plain).is_err());
    // A non-open empty body has no blocks.
    let empty = read_actor_bunch_body(&mut reader(&(Vec::new(), 0)), false, &plain).unwrap();
    assert!(empty.spawn.is_none() && empty.blocks.is_empty());
}

#[test]
fn trailing_bits_fail() {
    let plain = codec(13, false, false);
    let (fixture, _, _) = body_fixture();
    for extra in 1..=9usize {
        for pattern in [0u32, 0x3ff, 0x155, 0x003] {
            let mut writer = BitWriter::new();
            let mut input = reader(&fixture);
            for _ in 0..fixture.1 {
                writer.write_bit(input.read_bit());
            }
            writer.write_bits(pattern & ((1 << extra) - 1), extra as u32);
            let padded = bits(&writer);
            assert!(
                read_actor_bunch_body(&mut reader(&padded), true, &plain).is_err(),
                "{extra} trailing bits accepted"
            );
        }
    }
}

#[test]
fn limits_are_enforced() {
    let (fixture, _, body) = body_fixture();
    let mut small = codec(13, false, false);
    small.limits.max_blocks = 2;
    assert!(read_actor_bunch_body(&mut reader(&fixture), true, &small).is_err());
    assert!(write_actor_bunch_body(&mut BitWriter::new(), &body, &small).is_err());

    let mut small = codec(13, false, false);
    small.limits.max_payload_bits = 39;
    assert!(read_actor_bunch_body(&mut reader(&fixture), true, &small).is_err());
    assert!(write_actor_bunch_body(&mut BitWriter::new(), &body, &small).is_err());
    small.limits.max_payload_bits = 40;
    assert!(read_actor_bunch_body(&mut reader(&fixture), true, &small).is_ok());

    // A payload count far beyond the bunch fails before copying.
    let mut writer = BitWriter::new();
    writer.write_bit(false);
    writer.write_bit(true);
    packed(&mut writer, 1 << 30);
    let mut huge = codec(13, false, false);
    huge.limits.max_payload_bits = usize::MAX;
    assert!(read_content_block(&mut reader(&bits(&writer)), &huge).is_err());

    // Path units: "/A" declares three units.
    let exports = codec(13, true, false);
    let mut writer = BitWriter::new();
    packed(&mut writer, 6);
    writer.write_bits(0x01, 8);
    packed(&mut writer, 0);
    narrow(&mut writer, "/A");
    let fixture = bits(&writer);
    let reference = read_guid_ref(&mut reader(&fixture), &exports).unwrap();
    let mut tight = exports;
    tight.limits.max_path_units = 2;
    assert!(read_guid_ref(&mut reader(&fixture), &tight).is_err());
    assert!(write_guid_ref(&mut BitWriter::new(), &reference, &tight).is_err());

    // A huge declared path count fails without its payload.
    let mut writer = BitWriter::new();
    packed(&mut writer, 6);
    writer.write_bits(0x01, 8);
    packed(&mut writer, 0);
    writer.write_u32(0x7fff_ffff);
    let mut loose = exports;
    loose.limits.max_path_units = usize::MAX;
    assert!(read_guid_ref(&mut reader(&bits(&writer)), &loose).is_err());
}

fn bunch(open: bool, payload: Payload) -> Bunch {
    Bunch {
        header: BunchHeader {
            control: open,
            open,
            close: false,
            close_reason: None,
            paused: false,
            reliable: false,
            channel_index: 3,
            has_package_map_exports: false,
            has_must_be_mapped_guids: false,
            partial: false,
            reliable_sequence_raw: None,
            partial_initial: false,
            partial_final: false,
            channel_name: None,
        },
        payload,
    }
}

#[test]
fn parse_bunch_uses_open_flag_and_rejects_unread_prefixes() {
    let plain = codec(13, false, false);
    let (_, _, body) = body_fixture();
    let payload = encode_actor_bunch_body(&body, &plain).unwrap();
    assert_eq!(
        parse_actor_bunch(&bunch(true, payload.clone()), &plain).unwrap(),
        body
    );
    let mut exports = bunch(true, payload.clone());
    exports.header.has_package_map_exports = true;
    assert!(parse_actor_bunch(&exports, &plain).is_err());
    let mut mapped = bunch(true, payload.clone());
    mapped.header.has_must_be_mapped_guids = true;
    assert!(parse_actor_bunch(&mapped, &plain).is_err());
    let mut partial = bunch(true, payload);
    partial.header.partial = true;
    assert!(parse_actor_bunch(&partial, &plain).is_err());
}

#[test]
fn debug_and_summaries_carry_no_secrets() {
    let exports = codec(13, true, true);
    let secret_guid = 123_456_788u32;
    let checksum = 0xc0ff_ee11u32;
    let reference = |guid| NetGuidRef {
        guid,
        export: Some(GuidExport {
            no_load: true,
            path: Some(ExportPath {
                outer: Box::new(NetGuidRef::null()),
                path: text("SECRET-PATH-MARKER"),
            }),
            checksum: Some(checksum),
            bare_checksum_flag: false,
        }),
    };
    let body = ActorBunchBody {
        spawn: Some(SpawnRecord {
            actor: reference(secret_guid),
            dynamic: Some(DynamicSpawn {
                archetype: reference(secret_guid + 2),
                level: Some(reference(secret_guid + 4)),
                location: Some(SpawnVector::Quantized {
                    bits: 23,
                    components: [31_415_926, 0, 1],
                }),
                rotation: Some(CompressedRotator {
                    pitch: Some(0xbeef),
                    yaw: None,
                    roll: Some(48_879),
                }),
                scale: Some(SpawnVector::Raw([0x7e57_ab1e; 3])),
                velocity: None,
            }),
        }),
        blocks: vec![ContentBlock {
            has_rep_layout: true,
            target: BlockTarget::SubObject {
                object: reference(secret_guid + 6),
                class: SubObjectClass::Class(reference(secret_guid + 8)),
            },
            payload: Payload::from_writer({
                let mut writer = BitWriter::new();
                writer.write_u32(0xfeed_f00d);
                writer
            }),
        }],
    };
    // The structure is encodable and parses back.
    let encoded = encode_actor_bunch_body(&body, &exports).unwrap();
    assert_eq!(
        read_actor_bunch_body(&mut encoded.reader(), true, &exports).unwrap(),
        body
    );
    let debug = format!("{body:?}");
    let summary = serde_json::to_string(&body.summary()).unwrap();
    for output in [debug, summary] {
        for secret in [
            "SECRET",
            "123456788",
            "123456790",
            "75BCD14",
            "75bcd14",
            "c0ffee11",
            "C0FFEE11",
            "3237998097",
            "31415926",
            "48879",
            "beef",
            "7e57ab1e",
            "2119674654",
            "feedf00d",
            "4277006349",
        ] {
            assert!(!output.contains(secret), "{secret} leaked: {output}");
        }
        assert!(output.contains("18"), "path length missing: {output}");
    }
}

#[test]
fn export_prefix_round_trips_and_validates_bounds() {
    for order in [ByteOrder::Little, ByteOrder::Big] {
        let mut c = codec(17, false, true);
        c.order = order;
        let refs = vec![
            NetGuidRef {
                guid: 5,
                export: Some(GuidExport {
                    no_load: false,
                    path: Some(ExportPath {
                        outer: Box::new(NetGuidRef::null()),
                        path: text("/Script/Icarus"),
                    }),
                    checksum: Some(0x1234_5678),
                    bare_checksum_flag: false,
                }),
            },
            NetGuidRef {
                guid: 3,
                export: Some(GuidExport {
                    no_load: false,
                    path: None,
                    checksum: None,
                    bare_checksum_flag: true,
                }),
            },
        ];
        let mut writer = BitWriter::new();
        write_export_prefix(&mut writer, &refs, &c).unwrap();
        let fixture = bits(&writer);
        let mut input = reader(&fixture);
        assert_eq!(read_export_prefix(&mut input, &c).unwrap(), refs);
        assert!(input.at_end());
        for len in 0..fixture.1 {
            assert!(
                read_export_prefix(&mut prefix(&fixture, len), &c).is_err(),
                "prefix {len} accepted"
            );
        }
    }

    let c = codec(17, false, true);
    // bHasRepLayoutExport == 1 is rejected.
    let mut rep_export = BitWriter::new();
    rep_export.write_bit(true);
    rep_export.write_u32(0);
    assert!(read_export_prefix(&mut reader(&bits(&rep_export)), &c).is_err());

    // Negative and > MAX_EXPORT_GUIDS counts are rejected.
    for bad_count in [-1i32 as u32, MAX_EXPORT_GUIDS + 1] {
        let mut bad = BitWriter::new();
        bad.write_bit(false);
        bad.write_u32(bad_count);
        assert!(read_export_prefix(&mut reader(&bits(&bad)), &c).is_err());
    }
    let too_many = vec![NetGuidRef::null(); MAX_EXPORT_GUIDS as usize + 1];
    assert!(write_export_prefix(&mut BitWriter::new(), &too_many, &c).is_err());
}

#[test]
fn must_be_mapped_guids_round_trip_and_reject_non_canonical() {
    let guids = vec![2u32, 14, 300, 65_535];
    for order in [ByteOrder::Little, ByteOrder::Big] {
        let mut c = codec(17, false, true);
        c.order = order;
        let mut writer = BitWriter::new();
        write_must_be_mapped_guids(&mut writer, &guids, &c).unwrap();
        let fixture = bits(&writer);
        let mut input = reader(&fixture);
        assert_eq!(read_must_be_mapped_guids(&mut input, &c).unwrap(), guids);
        assert!(input.at_end());
        for len in 0..fixture.1 {
            assert!(
                read_must_be_mapped_guids(&mut prefix(&fixture, len), &c).is_err(),
                "prefix {len} accepted"
            );
        }
    }
    // Non-canonical packed GUID rejected.
    let c = codec(17, false, true);
    let mut bad = BitWriter::new();
    bad.write_bytes(&1u16.to_le_bytes());
    bad.write_bits(0x0b, 8);
    bad.write_bits(0x00, 8);
    assert!(read_must_be_mapped_guids(&mut reader(&bits(&bad)), &c).is_err());
}

#[test]
fn package_map_state_ingests_exports_and_resolves_must_be_mapped() {
    let c = codec(17, false, true);
    let pkg = NetGuidRef {
        guid: 3,
        export: Some(GuidExport {
            no_load: false,
            path: Some(ExportPath {
                outer: Box::new(NetGuidRef::null()),
                path: text("/Script/Icarus"),
            }),
            checksum: None,
            bare_checksum_flag: false,
        }),
    };
    let cls = NetGuidRef {
        guid: 5,
        export: Some(GuidExport {
            no_load: true,
            path: Some(ExportPath {
                outer: Box::new(pkg),
                path: text("Default__AIcarusPlayerControllerSurvival"),
            }),
            checksum: Some(0x9876_5432),
            bare_checksum_flag: false,
        }),
    };
    let mut sender_state = PackageMapState::new();
    let export_payload = sender_state
        .encode_export_bunch_payload(std::slice::from_ref(&cls), &c)
        .unwrap();
    assert_eq!(sender_state.len(), 2);
    let export_fixture = (export_payload.bytes().to_vec(), export_payload.bit_len());

    let mut state = PackageMapState::new();
    assert!(state.is_empty());
    let parsed = state
        .consume_export_prefix(&mut reader(&export_fixture), &c)
        .unwrap();
    assert_eq!(parsed.len(), 1);
    assert_eq!(state.len(), 2);
    assert!(state.contains(3) && state.contains(5) && !state.contains(99));
    assert_eq!(state.resolve_path(3), Some("/Script/Icarus"));
    assert_eq!(
        state.resolve_path(5),
        Some("Default__AIcarusPlayerControllerSurvival")
    );
    assert_eq!(state.resolve_outer_guid(5), Some(3));
    assert_eq!(
        state.resolve_full_path(5).as_deref(),
        Some("/Script/Icarus.Default__AIcarusPlayerControllerSurvival")
    );

    let body = payload(19, 7);
    let prepended = sender_state
        .prepend_must_be_mapped_guids(&[3, 5, 99], &body, &c)
        .unwrap();
    let mut mapped_reader = prepended.reader();
    let (guids, resolved) = state
        .consume_must_be_mapped(&mut mapped_reader, &c)
        .unwrap();
    assert_eq!(guids, vec![3, 5, 99]);
    assert_eq!(resolved, 2);
    assert_eq!(mapped_reader.remaining(), body.bit_len());

    // Debug output does not leak paths or GUIDs.
    let debug = format!("{state:?} {:?}", state.get(5).unwrap());
    assert!(!debug.contains("Icarus") && !debug.contains("98765432"));
}

#[test]
fn player_controller_open_bunch_round_trips_and_validates() {
    // Exact 472-bit Channel 2 open bunch from M-707 / M-708 (F-696, F-709).
    let bunch_bytes = [
        0x04, 0x12, 0x06, 0x4f, 0xd8, 0x04, 0xe8, 0x01, 0x00, 0xdd, 0x95, 0x3f, 0x00, 0x7e, 0x14,
        0xa0, 0x20, 0x8e, 0x81, 0x20, 0x98, 0x00, 0x00, 0xe0, 0x0d, 0x01, 0x00, 0x00, 0x01, 0x01,
        0x96, 0x28, 0x1f, 0xb1, 0x30, 0xb8, 0x40, 0xc0, 0x50, 0x00, 0xc4, 0x90, 0x04, 0x41, 0x00,
        0x28, 0x02, 0x80, 0x11, 0x00, 0x8e, 0x00, 0x80, 0x04, 0x88, 0x24, 0x09, 0x82, 0x00,
    ];
    let fixture = (bunch_bytes.to_vec(), 472usize);
    let c = codec(17, false, true);
    let mut input = reader(&fixture);
    let body = read_player_controller_open_bunch_body(&mut input, &c).unwrap();
    assert!(input.at_end());
    assert_eq!(body.net_player_index, 0);
    assert_eq!(body.blocks.len(), 7);
    assert_eq!(
        body.blocks
            .iter()
            .map(|b| b.payload.bit_len())
            .collect::<Vec<_>>(),
        vec![199, 18, 0, 0, 0, 0, 18]
    );
    let encoded = encode_player_controller_open_bunch_body(&body, &c).unwrap();
    assert_eq!((encoded.bytes().to_vec(), encoded.bit_len()), fixture);
    assert_eq!(
        parse_player_controller_open_bunch(&bunch(true, encoded.clone()), &c).unwrap(),
        body
    );
    assert!(parse_player_controller_open_bunch(&bunch(false, encoded), &c).is_err());

    // Debug and summary do not leak net_player_index.
    let mut secret_body = body.clone();
    secret_body.net_player_index = 251;
    let mut out = BitWriter::new();
    write_player_controller_open_bunch_body(&mut out, &secret_body, &c).unwrap();
    assert_eq!(
        read_player_controller_open_bunch_body(&mut reader(&bits(&out)), &c).unwrap(),
        secret_body
    );
    let debug = format!("{secret_body:?}");
    let summary = serde_json::to_string(&secret_body.summary()).unwrap();
    for text_out in [debug, summary] {
        assert!(
            !text_out.contains("251"),
            "net_player_index leaked: {text_out}"
        );
    }
}

#[test]
fn game_engine_max_tick_rate_and_net_driver_constants() {
    assert_eq!(DEFAULT_NET_SERVER_MAX_TICK_RATE, 30);
    assert_eq!(MIN_NET_SERVER_MAX_TICK_RATE, 1);
    assert_eq!(MAX_NET_SERVER_MAX_TICK_RATE, 1000);
    assert_eq!(DEFAULT_INI_MAX_CLIENT_RATE, 750_000);
    assert_eq!(DEFAULT_INI_MAX_INTERNET_CLIENT_RATE, 750_000);
    assert_eq!(DEFAULT_CONFIGURED_INTERNET_SPEED, 640_000);
    assert_eq!(DEFAULT_CONFIGURED_LAN_SPEED, 912_000);
    assert_eq!(DEFAULT_CONNECTION_TIMEOUT_SECONDS, 360.0);
    assert_eq!(DEFAULT_INITIAL_CONNECT_TIMEOUT_SECONDS, 360.0);
    assert_eq!(DEFAULT_RESOLUTION_CONNECTION_TIMEOUT_SECONDS, 120.0);

    let standalone = EngineNetMode::Standalone;
    let dedicated = EngineNetMode::DedicatedServer;
    let listen = EngineNetMode::ListenServer;
    let client = EngineNetMode::Client;
    assert_eq!(standalone as u8, 0);
    assert_eq!(dedicated as u8, 1);
    assert_eq!(listen as u8, 2);
    assert_eq!(client as u8, 3);

    let eval = evaluate_game_engine_max_tick_rate;

    // Nonzero base_engine_max_tick_rate short-circuits in every net mode.
    for mode in [standalone, dedicated, listen, client] {
        assert_eq!(eval(mode, false, 30, 60.0), 60.0);
    }

    // DedicatedServer clamps NetServerMaxTickRate to 1..=1000 regardless of listen-server flag.
    assert_eq!(eval(dedicated, false, 30, 0.0), 30.0);
    assert_eq!(eval(dedicated, false, 0, 0.0), 1.0);
    assert_eq!(eval(dedicated, false, -50, 0.0), 1.0);
    assert_eq!(eval(dedicated, true, 5000, 0.0), 1000.0);

    // ListenServer requires b_use_client_max_tick_rate_when_listen_server.
    assert_eq!(eval(listen, false, 30, 0.0), 0.0);
    assert_eq!(eval(listen, true, 30, 0.0), 30.0);
    assert_eq!(eval(listen, true, 0, 0.0), 1.0);
    assert_eq!(eval(listen, true, 2000, 0.0), 1000.0);

    // Standalone and Client return 0.0 when base_engine_max_tick_rate == 0.0.
    for flag in [false, true] {
        assert_eq!(eval(standalone, flag, 30, 0.0), 0.0);
        assert_eq!(eval(client, flag, 30, 0.0), 0.0);
    }
}

#[test]
fn net_relevancy_distance_strict_less_than_and_nan() {
    // 3^2 + 4^2 + 0^2 == 25.0.
    assert!(is_within_net_relevancy_distance(
        [3.0, 4.0, 0.0],
        [0.0, 0.0, 0.0],
        25.001
    ));
    // Strict `<` (`comiss xmm2, xmm1; setb al`): exact equality is not relevant.
    assert!(!is_within_net_relevancy_distance(
        [3.0, 4.0, 0.0],
        [0.0, 0.0, 0.0],
        25.0
    ));
    assert!(!is_within_net_relevancy_distance(
        [3.0, 4.0, 0.0],
        [0.0, 0.0, 0.0],
        24.0
    ));
    // NaN returns false.
    assert!(!is_within_net_relevancy_distance(
        [f32::NAN, 0.0, 0.0],
        [0.0, 0.0, 0.0],
        100.0
    ));
    assert!(!is_within_net_relevancy_distance(
        [0.0, 0.0, 0.0],
        [0.0, 0.0, 0.0],
        f32::NAN
    ));
}

#[test]
fn actor_net_relevancy_branch_order() {
    let base = ActorRelevancyInputs {
        b_always_relevant: false,
        b_only_relevant_to_owner: false,
        b_hidden: false,
        b_use_owner_relevancy: false,
        is_owned_by_view_target_or_real_viewer: false,
        view_target_is_instigator: false,
        has_owner: false,
        has_root_component: true,
        root_collision_enabled: true,
        attach_parent_delegates_to_owner: false,
        b_use_distance_based_relevancy: true,
        actor_location: [100.0, 0.0, 0.0],
        src_location: [0.0, 0.0, 0.0],
        net_cull_distance_squared: 1_000.0, // dist_sq == 10_000.0 (out of range by default)
    };
    let check = |inputs: ActorRelevancyInputs, expected: ActorRelevancyOutcome| {
        assert_eq!(evaluate_actor_net_relevancy(&inputs), expected);
    };

    // 1. Always relevant / owned by ViewTarget or RealViewer / ViewTarget is Instigator.
    for mut inputs in [
        ActorRelevancyInputs {
            b_always_relevant: true,
            b_use_owner_relevancy: true,
            has_owner: true,
            b_only_relevant_to_owner: true,
            ..base
        },
        ActorRelevancyInputs {
            is_owned_by_view_target_or_real_viewer: true,
            b_use_owner_relevancy: true,
            has_owner: true,
            b_only_relevant_to_owner: true,
            ..base
        },
        ActorRelevancyInputs {
            view_target_is_instigator: true,
            b_use_owner_relevancy: true,
            has_owner: true,
            b_only_relevant_to_owner: true,
            ..base
        },
    ] {
        inputs.has_root_component = false;
        check(inputs, ActorRelevancyOutcome::Relevant);
    }

    // 2. bUseOwnerRelevancy && Owner != null -> DelegateToOwner (precedes bOnlyRelevantToOwner).
    check(
        ActorRelevancyInputs {
            b_use_owner_relevancy: true,
            has_owner: true,
            b_only_relevant_to_owner: true,
            ..base
        },
        ActorRelevancyOutcome::DelegateToOwner,
    );

    // 3. bOnlyRelevantToOwner -> NotRelevant without missing-root warning.
    check(
        ActorRelevancyInputs {
            b_use_owner_relevancy: true,
            has_owner: false,
            b_only_relevant_to_owner: true,
            has_root_component: false,
            ..base
        },
        ActorRelevancyOutcome::NotRelevant {
            missing_root_component_warning: false,
        },
    );

    // 4. RootComponent && AttachParent delegation -> DelegateToAttachParentOwner.
    check(
        ActorRelevancyInputs {
            has_root_component: true,
            attach_parent_delegates_to_owner: true,
            b_hidden: true,
            root_collision_enabled: false,
            ..base
        },
        ActorRelevancyOutcome::DelegateToAttachParentOwner,
    );

    // 5. bHidden && (!RootComponent || !RootComponent->IsCollisionEnabled()) -> NotRelevant (no warning).
    check(
        ActorRelevancyInputs {
            b_hidden: true,
            has_root_component: false,
            root_collision_enabled: true,
            ..base
        },
        ActorRelevancyOutcome::NotRelevant {
            missing_root_component_warning: false,
        },
    );
    check(
        ActorRelevancyInputs {
            b_hidden: true,
            has_root_component: true,
            root_collision_enabled: false,
            ..base
        },
        ActorRelevancyOutcome::NotRelevant {
            missing_root_component_warning: false,
        },
    );
    // bHidden with RootComponent and collision enabled continues to distance check.
    check(
        ActorRelevancyInputs {
            b_hidden: true,
            has_root_component: true,
            root_collision_enabled: true,
            net_cull_distance_squared: 20_000.0,
            ..base
        },
        ActorRelevancyOutcome::Relevant,
    );

    // 6. !RootComponent (when !bHidden) -> NotRelevant with LogNet missing-root warning.
    check(
        ActorRelevancyInputs {
            b_hidden: false,
            has_root_component: false,
            ..base
        },
        ActorRelevancyOutcome::NotRelevant {
            missing_root_component_warning: true,
        },
    );

    // 7. !bUseDistanceBasedRelevancy -> Relevant regardless of distance.
    check(
        ActorRelevancyInputs {
            b_use_distance_based_relevancy: false,
            net_cull_distance_squared: 10.0,
            ..base
        },
        ActorRelevancyOutcome::Relevant,
    );

    // 8. Distance-based relevancy check.
    check(
        ActorRelevancyInputs {
            net_cull_distance_squared: 10_001.0,
            ..base
        },
        ActorRelevancyOutcome::Relevant,
    );
    check(
        ActorRelevancyInputs {
            net_cull_distance_squared: 10_000.0,
            ..base
        },
        ActorRelevancyOutcome::NotRelevant {
            missing_root_component_warning: false,
        },
    );
}

#[test]
fn pawn_and_player_controller_net_relevancy_branch_order() {
    assert!(evaluate_player_controller_net_relevancy(10_u64, 10_u64));
    assert!(!evaluate_player_controller_net_relevancy(10_u64, 11_u64));

    let base = PawnRelevancyInputs {
        b_always_relevant: false,
        real_viewer_is_controller: false,
        is_owned_by_view_target_or_real_viewer: false,
        view_target_is_instigator: false,
        based_on_view_target_or_vice_versa: false,
        b_hidden: false,
        b_only_relevant_to_owner: false,
        has_root_component: true,
        root_collision_enabled: true,
        movement_base_delegates_to_owner: false,
        b_use_distance_based_relevancy: true,
        actor_location: [100.0, 0.0, 0.0],
        src_location: [0.0, 0.0, 0.0],
        net_cull_distance_squared: 1_000.0, // dist_sq == 10_000.0
    };
    let check = |inputs: PawnRelevancyInputs, expected: PawnRelevancyOutcome| {
        assert_eq!(evaluate_pawn_net_relevancy(&inputs), expected);
    };

    // 1. Any of the 5 fast-path flags returns Relevant.
    for fast_path in [
        PawnRelevancyInputs {
            b_always_relevant: true,
            b_hidden: true,
            has_root_component: false,
            ..base
        },
        PawnRelevancyInputs {
            real_viewer_is_controller: true,
            b_hidden: true,
            has_root_component: false,
            ..base
        },
        PawnRelevancyInputs {
            is_owned_by_view_target_or_real_viewer: true,
            b_only_relevant_to_owner: true,
            has_root_component: false,
            ..base
        },
        PawnRelevancyInputs {
            view_target_is_instigator: true,
            b_hidden: true,
            has_root_component: false,
            ..base
        },
        PawnRelevancyInputs {
            based_on_view_target_or_vice_versa: true,
            b_hidden: true,
            has_root_component: false,
            ..base
        },
    ] {
        check(fast_path, PawnRelevancyOutcome::Relevant);
    }

    // 2. (bHidden || bOnlyRelevantToOwner) && (!RootComponent || !RootCollisionEnabled) -> NotRelevant.
    check(
        PawnRelevancyInputs {
            b_hidden: true,
            has_root_component: true,
            root_collision_enabled: false,
            movement_base_delegates_to_owner: true,
            ..base
        },
        PawnRelevancyOutcome::NotRelevant,
    );
    check(
        PawnRelevancyInputs {
            b_only_relevant_to_owner: true,
            has_root_component: false,
            root_collision_enabled: true,
            movement_base_delegates_to_owner: true,
            ..base
        },
        PawnRelevancyOutcome::NotRelevant,
    );

    // 3. MovementBase delegation -> DelegateToMovementBaseOwner.
    check(
        PawnRelevancyInputs {
            b_hidden: true,
            has_root_component: true,
            root_collision_enabled: true,
            movement_base_delegates_to_owner: true,
            ..base
        },
        PawnRelevancyOutcome::DelegateToMovementBaseOwner,
    );

    // 4. !bUseDistanceBasedRelevancy -> Relevant.
    check(
        PawnRelevancyInputs {
            b_use_distance_based_relevancy: false,
            has_root_component: false,
            ..base
        },
        PawnRelevancyOutcome::Relevant,
    );

    // 5. Distance check requires RootComponent && dist_sq < NetCullDistanceSquared.
    check(
        PawnRelevancyInputs {
            has_root_component: false,
            net_cull_distance_squared: 20_000.0,
            ..base
        },
        PawnRelevancyOutcome::NotRelevant,
    );
    check(
        PawnRelevancyInputs {
            has_root_component: true,
            net_cull_distance_squared: 10_001.0,
            ..base
        },
        PawnRelevancyOutcome::Relevant,
    );
    check(
        PawnRelevancyInputs {
            has_root_component: true,
            net_cull_distance_squared: 10_000.0,
            ..base
        },
        PawnRelevancyOutcome::NotRelevant,
    );
}

#[test]
fn actor_and_player_controller_net_priority_branches() {
    let base = ActorPriorityInputs {
        b_use_owner_relevancy: false,
        has_owner: false,
        is_view_target_or_instigator: false,
        b_hidden: false,
        root_location: Some([0.0, 0.0, 0.0]),
        viewer_location: [0.0, 0.0, 0.0],
        viewer_dir: [1.0, 0.0, 0.0],
        time: 2.0,
        net_priority: 3.0,
    };

    let assert_priority =
        |inputs: ActorPriorityInputs, expected: f32| match evaluate_actor_net_priority(&inputs) {
            ActorPriorityOutcome::Computed(actual) => {
                assert!(
                    (actual - expected).abs() < 1e-5,
                    "expected {expected}, got {actual}"
                );
            }
            other => panic!("expected Computed({expected}), got {other:?}"),
        };

    // Owner delegation when b_use_owner_relevancy && has_owner.
    let delegated = evaluate_actor_net_priority(&ActorPriorityInputs {
        b_use_owner_relevancy: true,
        has_owner: true,
        ..base
    });
    assert_eq!(delegated, ActorPriorityOutcome::DelegateToOwner);

    // Without owner, b_use_owner_relevancy does not delegate.
    assert_priority(
        ActorPriorityInputs {
            b_use_owner_relevancy: true,
            has_owner: false,
            root_location: None,
            ..base
        },
        6.0,
    );

    // Hidden or null RootComponent skips spatial multiplier (1.0x or 4.0x when ViewTarget/Instigator).
    assert_priority(
        ActorPriorityInputs {
            b_hidden: true,
            root_location: Some([1000.0, 0.0, 0.0]),
            ..base
        },
        6.0,
    );
    assert_priority(
        ActorPriorityInputs {
            root_location: None,
            is_view_target_or_instigator: true,
            ..base
        },
        24.0, // 4.0 * 2.0 * 3.0
    );

    // Behind viewer (dot < 0.0):
    // - dist_sq <= 250_000.0 (<= 500 units): multiplier 1.0
    assert_priority(
        ActorPriorityInputs {
            root_location: Some([-400.0, 0.0, 0.0]),
            ..base
        },
        6.0,
    );
    // - 250_000.0 < dist_sq <= 4_000_000.0 (500..=2000 units): multiplier 0.4
    assert_priority(
        ActorPriorityInputs {
            root_location: Some([-1000.0, 0.0, 0.0]),
            ..base
        },
        2.4, // 0.4 * 6.0
    );
    // - dist_sq > 4_000_000.0 (> 2000 units): multiplier 0.2
    assert_priority(
        ActorPriorityInputs {
            root_location: Some([-2500.0, 0.0, 0.0]),
            ..base
        },
        1.2, // 0.2 * 6.0
    );

    // In front of viewer (dot >= 0.0):
    // - dist_sq < 64_000_000.0 (< 8000 units) && dot^2 > 0.5 * dist_sq: multiplier 2.0
    assert_priority(
        ActorPriorityInputs {
            root_location: Some([1000.0, 0.0, 0.0]),
            ..base
        },
        12.0, // 2.0 * 6.0
    );
    // - ViewTarget/Instigator (4.0x at 0x14333b320..0x14333b336: c3 ret) is exclusive with the !b_hidden && RootComponent spatial block:
    assert_priority(
        ActorPriorityInputs {
            is_view_target_or_instigator: true,
            root_location: Some([1000.0, 0.0, 0.0]),
            ..base
        },
        24.0, // 4.0 * 6.0 (does not stack the 2.0x front cone boost)
    );
    // - Outside narrow cone (dot^2 <= 0.5 * dist_sq) and dist_sq <= 9_998_244.0 (<= 3162 units): multiplier 1.0
    assert_priority(
        ActorPriorityInputs {
            root_location: Some([1000.0, 2000.0, 0.0]),
            ..base
        },
        6.0, // 1.0 * 6.0
    );
    // - Outside narrow cone and dist_sq > 9_998_244.0 (> 3162 units): multiplier 0.4
    assert_priority(
        ActorPriorityInputs {
            root_location: Some([1000.0, 4000.0, 0.0]),
            ..base
        },
        2.4, // 0.4 * 6.0
    );
    // - Straight ahead but dist_sq >= 64_000_000.0 (>= 8000 units, > 3162 units): multiplier 0.4
    assert_priority(
        ActorPriorityInputs {
            root_location: Some([9000.0, 0.0, 0.0]),
            ..base
        },
        2.4, // 0.4 * 6.0
    );

    // APlayerController::GetNetPriority: 4.0x when this == InViewer, 1.0x otherwise.
    let pc_low = evaluate_player_controller_net_priority(2.0, 3.0, false);
    let pc_high = evaluate_player_controller_net_priority(2.0, 3.0, true);
    assert_eq!(pc_low, 6.0);
    assert_eq!(pc_high, 24.0);
}

#[test]
fn online_beacon_relevancy_and_matinee_actor_priority() {
    // AOnlineBeacon::IsNetRelevantFor (0x1409a5780 server / 0x1409cc210 client):
    // - When bOnlyRelevantToOwner is true, checks beacon_net_connection == src_actor_net_connection.
    assert!(evaluate_online_beacon_net_relevancy(
        true, "BeaconA", "ViewerB", 7u32, 7u32
    ));
    assert!(!evaluate_online_beacon_net_relevancy(
        true, "BeaconA", "BeaconA", 7u32, 8u32
    ));
    // - When bOnlyRelevantToOwner is false, checks beacon_class == real_viewer_class.
    assert!(evaluate_online_beacon_net_relevancy(
        false, "BeaconA", "BeaconA", 7u32, 8u32
    ));
    assert!(!evaluate_online_beacon_net_relevancy(
        false, "BeaconA", "ViewerB", 7u32, 7u32
    ));

    // AMatineeActor::GetNetPriority (0x143894660 server / 0x143b3f6c0 client):
    let base_inputs = ActorPriorityInputs {
        b_use_owner_relevancy: false,
        has_owner: false,
        is_view_target_or_instigator: false,
        b_hidden: true,
        root_location: None,
        viewer_location: [0.0, 0.0, 0.0],
        viewer_dir: [1.0, 0.0, 0.0],
        time: 2.0,
        net_priority: 3.0, // base priority = 6.0
    };
    // With an open channel (InChannel != nullptr), returns base priority unchanged.
    assert_eq!(
        evaluate_matinee_actor_net_priority(&base_inputs, true, 25.0, 100.0),
        ActorPriorityOutcome::Computed(6.0)
    );
    // Without an open channel (InChannel == nullptr), adds 1.0 - (CreationTime / WorldTimeSeconds).
    assert_eq!(
        evaluate_matinee_actor_net_priority(&base_inputs, false, 25.0, 100.0),
        ActorPriorityOutcome::Computed(6.75)
    );
    // Owner delegation propagates unchanged.
    let delegated = ActorPriorityInputs {
        b_use_owner_relevancy: true,
        has_owner: true,
        ..base_inputs
    };
    assert_eq!(
        evaluate_matinee_actor_net_priority(&delegated, false, 25.0, 100.0),
        ActorPriorityOutcome::DelegateToOwner
    );
}

#[test]
fn server_replicate_actors_prep_connections_and_build_consider_list() {
    assert_eq!(PREP_CONNECTIONS_RECEIVE_TIMEOUT_SECONDS, 1.5);
    assert_eq!(ADAPTIVE_NET_UPDATE_DELAY_THRESHOLD_SECONDS, 2.0);
    assert_eq!(ADAPTIVE_NET_UPDATE_LERP_SCALE, 0.2);
    assert_eq!(DEFAULT_MIN_NET_UPDATE_FREQUENCY, 2.0);

    // PrepConnections tick budget:
    // - Throttling disabled, no per-frame cap -> all clients ticked, carryover preserved.
    assert_eq!(
        evaluate_prep_connections_tick_budget(8, false, 0.01, 0.02, 30.0, false, 0),
        (8, 0.02)
    );
    // - Throttling enabled, budget truncates to 0 -> returns 0 and accumulates carryover.
    let (tick_zero, carry) =
        evaluate_prep_connections_tick_budget(8, true, 0.01, 0.01, 30.0, false, 0);
    assert_eq!(tick_zero, 0);
    assert!((carry - 0.02).abs() < 1e-6);
    // - Throttling enabled with LAN multiplier 2.0: (0.02 + 0.02) * 30.0 * 2.0 = 2.4 -> 2 clients, carryover cleared.
    assert_eq!(
        evaluate_prep_connections_tick_budget(8, true, 0.02, 0.02, 30.0, true, 0),
        (2, 0.0)
    );
    // - Clamped by net.MaxConnectionsToTickPerServerFrame > 0.
    assert_eq!(
        evaluate_prep_connections_tick_budget(8, false, 0.1, 0.0, 30.0, false, 3),
        (3, 0.0)
    );

    // Connection readiness: ViewTarget != null, State == USOCK_Open (3), Elapsed - LastReceive < 1.5.
    assert!(is_connection_ready_for_replication(true, 3, 10.0, 8.6));
    assert!(!is_connection_ready_for_replication(false, 3, 10.0, 9.0));
    assert!(!is_connection_ready_for_replication(true, 2, 10.0, 9.0));
    assert!(!is_connection_ready_for_replication(true, 3, 10.0, 8.5)); // strict `< 1.5`

    // BuildConsiderList actor evaluation:
    let base_actor = ConsiderListActorInputs {
        b_pending_net_update: false,
        next_update_time: 10.0,
        last_net_replicate_time: 9.0,
        optimal_net_update_delta: 0.1,
        world_time_seconds: 12.0,
        driver_elapsed_time: 50.0,
        is_pending_kill: false,
        remote_role: 1,
        net_driver_name_matches: true,
        b_net_startup: false,
        net_dormancy: 0,
        level_initialized: true,
        net_update_frequency: 10.0,
        min_net_update_frequency: 0.0,
        b_use_adaptive_net_update_frequency: true,
        random_unit_float: 0.5,
        server_tick_time: 0.04,
    };

    // 1. Not pending update and world_time <= next_update_time -> skip.
    assert_eq!(
        evaluate_consider_list_actor(&ConsiderListActorInputs {
            world_time_seconds: 10.0,
            ..base_actor
        }),
        ConsiderListActorOutcome::SkipNotReadyForUpdate
    );

    // 2. PendingKill or RemoteRole == ROLE_None (0) -> remove from network object list.
    assert_eq!(
        evaluate_consider_list_actor(&ConsiderListActorInputs {
            is_pending_kill: true,
            ..base_actor
        }),
        ConsiderListActorOutcome::RemoveFromNetworkObjectList
    );
    assert_eq!(
        evaluate_consider_list_actor(&ConsiderListActorInputs {
            remote_role: 0,
            ..base_actor
        }),
        ConsiderListActorOutcome::RemoveFromNetworkObjectList
    );

    // 3. Mismatched NetDriverName -> skip.
    assert_eq!(
        evaluate_consider_list_actor(&ConsiderListActorInputs {
            net_driver_name_matches: false,
            ..base_actor
        }),
        ConsiderListActorOutcome::SkipMismatchedNetDriverName
    );

    // 4. bNetStartup && NetDormancy == DORM_Initial (4) && level_initialized -> remove.
    assert_eq!(
        evaluate_consider_list_actor(&ConsiderListActorInputs {
            b_net_startup: true,
            net_dormancy: 4,
            level_initialized: true,
            ..base_actor
        }),
        ConsiderListActorOutcome::RemoveFromNetworkObjectList
    );

    // 5. Adaptive frequency scaling (world_time = 12.0, last_rep = 9.0 -> delta = 3.0 > 2.0):
    // min_net_update_frequency defaults from 0.0 to 2.0 (max_optimal_delta = 0.5, min_optimal_delta = 0.1).
    // alpha = clamp((3.0 - 2.0) * 0.2, 0.0, 1.0) = 0.2 -> optimal = 0.1 + 0.4 * 0.2 = 0.18.
    // next_update_time = 12.0 + 0.5 * 0.04 + 0.18 = 12.20.
    match evaluate_consider_list_actor(&base_actor) {
        ConsiderListActorOutcome::AddToConsiderListAndPreReplicate {
            next_update_time,
            last_net_replicate_time,
            optimal_net_update_delta,
            updated_min_net_update_frequency,
            updated_last_net_update_time,
        } => {
            assert!((next_update_time - 12.20).abs() < 1e-5);
            assert_eq!(last_net_replicate_time, 9.0);
            assert!((optimal_net_update_delta - 0.18).abs() < 1e-6);
            assert_eq!(updated_min_net_update_frequency, 2.0);
            assert_eq!(updated_last_net_update_time, Some(50.0));
        }
        other => panic!("unexpected outcome: {other:?}"),
    }

    // 6. First-time initialization (last_net_replicate_time == 0.0) with bPendingNetUpdate == true:
    // initializes last_net_replicate_time = world_time and optimal = 1 / 10 = 0.1,
    // and skips rescheduling next_update_time / last_net_update_time because bPendingNetUpdate is true.
    match evaluate_consider_list_actor(&ConsiderListActorInputs {
        b_pending_net_update: true,
        last_net_replicate_time: 0.0,
        optimal_net_update_delta: 0.0,
        ..base_actor
    }) {
        ConsiderListActorOutcome::AddToConsiderListAndPreReplicate {
            next_update_time,
            last_net_replicate_time,
            optimal_net_update_delta,
            updated_min_net_update_frequency,
            updated_last_net_update_time,
        } => {
            assert_eq!(next_update_time, 10.0);
            assert_eq!(last_net_replicate_time, 12.0);
            assert!((optimal_net_update_delta - 0.1).abs() < 1e-6);
            assert_eq!(updated_min_net_update_frequency, 0.0);
            assert_eq!(updated_last_net_update_time, None);
        }
        other => panic!("unexpected outcome: {other:?}"),
    }
}

#[test]
fn relevancy_priority_tick_rate_and_consider_list_never_panic_on_randomized_inputs() {
    let mut state = 0x9e37_79b9_7f4a_7c15_u64;
    let mut next_u64 = || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let sample_f32 = |raw: u64| -> f32 {
        match raw & 0xf {
            0 => 0.0,
            1 => -0.0,
            2 => f32::NAN,
            3 => f32::INFINITY,
            4 => f32::NEG_INFINITY,
            5 => f32::MIN,
            6 => f32::MAX,
            7 => f32::MIN_POSITIVE,
            _ => f32::from_bits(raw as u32),
        }
    };
    let sample_f64 = |raw: u64| -> f64 {
        match raw & 0xf {
            0 => 0.0,
            1 => -0.0,
            2 => f64::NAN,
            3 => f64::INFINITY,
            4 => f64::NEG_INFINITY,
            5 => f64::MIN,
            6 => f64::MAX,
            7 => f64::MIN_POSITIVE,
            _ => f64::from_bits(raw),
        }
    };

    for _ in 0..1024 {
        let bits = next_u64();
        let flag = |shift: u32| ((bits >> shift) & 1) != 0;
        let loc_a = [
            sample_f32(next_u64()),
            sample_f32(next_u64()),
            sample_f32(next_u64()),
        ];
        let loc_b = [
            sample_f32(next_u64()),
            sample_f32(next_u64()),
            sample_f32(next_u64()),
        ];
        let dir = [
            sample_f32(next_u64()),
            sample_f32(next_u64()),
            sample_f32(next_u64()),
        ];
        let cull_sq = sample_f32(next_u64());

        let _ = is_within_net_relevancy_distance(loc_a, loc_b, cull_sq);
        let _ = evaluate_actor_net_relevancy(&ActorRelevancyInputs {
            b_always_relevant: flag(0),
            b_only_relevant_to_owner: flag(1),
            b_hidden: flag(2),
            b_use_owner_relevancy: flag(3),
            is_owned_by_view_target_or_real_viewer: flag(4),
            view_target_is_instigator: flag(5),
            has_owner: flag(6),
            has_root_component: flag(7),
            root_collision_enabled: flag(8),
            attach_parent_delegates_to_owner: flag(9),
            b_use_distance_based_relevancy: flag(10),
            actor_location: loc_a,
            src_location: loc_b,
            net_cull_distance_squared: cull_sq,
        });
        let _ = evaluate_pawn_net_relevancy(&PawnRelevancyInputs {
            b_always_relevant: flag(11),
            real_viewer_is_controller: flag(12),
            is_owned_by_view_target_or_real_viewer: flag(13),
            view_target_is_instigator: flag(14),
            based_on_view_target_or_vice_versa: flag(15),
            b_hidden: flag(16),
            b_only_relevant_to_owner: flag(17),
            has_root_component: flag(18),
            root_collision_enabled: flag(19),
            movement_base_delegates_to_owner: flag(20),
            b_use_distance_based_relevancy: flag(21),
            actor_location: loc_a,
            src_location: loc_b,
            net_cull_distance_squared: cull_sq,
        });
        let _ = evaluate_player_controller_net_relevancy(next_u64(), next_u64());
        let _ = evaluate_online_beacon_net_relevancy(
            flag(22),
            next_u64() & 3,
            next_u64() & 3,
            next_u64() & 3,
            next_u64() & 3,
        );

        let prio_inputs = ActorPriorityInputs {
            b_use_owner_relevancy: flag(23),
            has_owner: flag(24),
            is_view_target_or_instigator: flag(25),
            b_hidden: flag(26),
            root_location: if flag(27) { Some(loc_a) } else { None },
            viewer_location: loc_b,
            viewer_dir: dir,
            time: sample_f32(next_u64()),
            net_priority: sample_f32(next_u64()),
        };
        let _ = evaluate_actor_net_priority(&prio_inputs);
        let _ = evaluate_player_controller_net_priority(
            sample_f32(next_u64()),
            sample_f32(next_u64()),
            flag(28),
        );
        let _ = evaluate_matinee_actor_net_priority(
            &prio_inputs,
            flag(29),
            sample_f32(next_u64()),
            sample_f32(next_u64()),
        );

        let net_mode = match bits & 3 {
            0 => EngineNetMode::Standalone,
            1 => EngineNetMode::DedicatedServer,
            2 => EngineNetMode::ListenServer,
            _ => EngineNetMode::Client,
        };
        let _ = evaluate_game_engine_max_tick_rate(
            net_mode,
            flag(33),
            next_u64() as i32,
            sample_f32(next_u64()),
        );
        let _ = evaluate_prep_connections_tick_budget(
            next_u64() as usize,
            flag(34),
            sample_f32(next_u64()),
            sample_f32(next_u64()),
            sample_f32(next_u64()),
            flag(35),
            next_u64() as i32,
        );
        let _ = is_connection_ready_for_replication(
            flag(36),
            next_u64() as u32,
            sample_f64(next_u64()),
            sample_f64(next_u64()),
        );
        let _ = evaluate_consider_list_actor(&ConsiderListActorInputs {
            b_pending_net_update: flag(37),
            next_update_time: sample_f64(next_u64()),
            last_net_replicate_time: sample_f64(next_u64()),
            optimal_net_update_delta: sample_f32(next_u64()),
            world_time_seconds: sample_f32(next_u64()),
            driver_elapsed_time: sample_f64(next_u64()),
            is_pending_kill: flag(38),
            remote_role: (next_u64() & 0xff) as u8,
            net_driver_name_matches: flag(39),
            b_net_startup: flag(40),
            net_dormancy: (next_u64() & 0xff) as u8,
            level_initialized: flag(41),
            net_update_frequency: sample_f32(next_u64()),
            min_net_update_frequency: sample_f32(next_u64()),
            b_use_adaptive_net_update_frequency: flag(42),
            random_unit_float: sample_f32(next_u64()),
            server_tick_time: sample_f32(next_u64()),
        });
        let _ = round_net_priority_to_int(sample_f32(next_u64()));
        let _ = evaluate_destruction_info_priority(sample_f32(next_u64()), loc_a, loc_b, dir);
        let _ = evaluate_prioritize_actor(&PrioritizeActorInputs {
            has_channel: flag(43),
            b_only_relevant_to_owner: flag(44),
            is_relevant_to_at_least_one_viewer: flag(45),
            driver_elapsed_time: sample_f64(next_u64()),
            channel_relevant_time: sample_f64(next_u64()),
            relevant_timeout: sample_f64(next_u64()),
            actor_net_tag: next_u64() as i32,
            driver_net_tag: next_u64() as i32,
            is_actor_dormant_on_connection: flag(46),
            actor_net_dormancy: (next_u64() & 0x7) as u8,
            channel_b_pending_dormancy: flag(47),
            channel_dormant: flag(48),
            get_net_dormancy_returns_dormant_partial: flag(49),
            level_initialized_for_actor: flag(50),
            channel_last_update_time: sample_f64(next_u64()),
            channel_open_elapsed_time: sample_f64(next_u64()),
            max_net_priority_across_viewers: sample_f32(next_u64()),
        });
        let bits2 = next_u64();
        let flag2 = |shift: u32| ((bits2 >> shift) & 1) != 0;
        let _ = evaluate_process_prioritized_actor(&ProcessPrioritizedActorInputs {
            connection_is_net_ready: flag2(0),
            is_destruction_info: flag2(1),
            destruction_level_initialized: flag2(2),
            has_channel: flag2(3),
            driver_elapsed_time: sample_f64(next_u64()),
            channel_relevant_time: sample_f64(next_u64()),
            relevant_timeout: sample_f64(next_u64()),
            is_relevant_to_connection: flag2(4),
            actor_b_tear_off: flag2(5),
            force_relevant_frame: next_u64() as i32,
            driver_replication_frame: next_u64() as i32,
            channel_b_is_replicating_actor: flag2(6),
            actor_b_always_relevant: flag2(7),
            actor_b_net_temporary: flag2(8),
            channel_open_acked: flag2(9),
            actor_level_initialized: flag2(10),
            package_map_supports_object: flag2(11),
            actor_net_update_frequency: sample_f32(next_u64()),
            world_time_seconds: sample_f32(next_u64()),
            random_unit_float: sample_f32(next_u64()),
            can_create_actor_channel: flag2(12),
            replicate_actor_wrote_anything: flag2(13),
            connection_is_net_ready_after_replicate: flag2(14),
            actor_is_net_startup: flag2(15),
        });
        let _ = evaluate_mark_relevant_actor(
            flag2(16),
            flag2(17),
            next_u64() as i32,
            next_u64() as i32,
            flag2(18),
        );
    }
}

#[test]
fn server_replicate_actors_prioritize_process_and_mark_relevant() {
    assert_eq!(PRIORITY_SCALE_MULTIPLIER, 65536.0);
    assert_eq!(DESTRUCTION_INFO_BEHIND_NEAR_DIST_SQ, 250_000.0);
    assert_eq!(DESTRUCTION_INFO_BEHIND_FAR_DIST_SQ, 4_000_000.0);
    assert_eq!(DESTRUCTION_INFO_FRONT_FAR_DIST_SQ, 9_998_244.0);
    assert_eq!(DESTRUCTION_INFO_NEAR_FACTOR, 0.4);
    assert_eq!(DESTRUCTION_INFO_FAR_FACTOR, 0.2);
    assert_eq!(PROCESS_PRIORITIZED_RELEVANCY_FAST_PATH_SECONDS, 1.0);
    assert_eq!(PROCESS_PRIORITIZED_RELEVANT_TIME_JITTER_SCALE, 0.5);
    assert_eq!(PROCESS_PRIORITIZED_LOW_FREQ_THRESHOLD, 1.0);
    assert_eq!(PROCESS_PRIORITIZED_LOW_FREQ_DELAY_SCALE, 0.2);

    // 1. round_net_priority_to_int (`appRoundToInt(65536.0f * priority)` via `cvtss2si`):
    assert_eq!(round_net_priority_to_int(0.0), 0);
    assert_eq!(round_net_priority_to_int(0.1), 6_554);
    assert_eq!(round_net_priority_to_int(1.0), 65_536);
    assert_eq!(round_net_priority_to_int(1.5), 98_304);
    assert_eq!(round_net_priority_to_int(-1.0), -65_536);
    assert_eq!(round_net_priority_to_int(f32::NAN), i32::MIN >> 1);
    assert_eq!(round_net_priority_to_int(f32::INFINITY), i32::MIN >> 1);

    // 2. evaluate_destruction_info_priority:
    let viewer_loc = [0.0, 0.0, 0.0];
    let viewer_dir = [1.0, 0.0, 0.0];
    // - Behind viewer, dist_sq <= 250_000 (400^2 = 160_000): factor 1.0
    assert_eq!(
        evaluate_destruction_info_priority(2.0, [-400.0, 0.0, 0.0], viewer_loc, viewer_dir),
        131_072
    );
    // - Behind viewer, 250_000 < dist_sq <= 4_000_000 (1000^2 = 1_000_000): factor 0.4
    assert_eq!(
        evaluate_destruction_info_priority(2.0, [-1000.0, 0.0, 0.0], viewer_loc, viewer_dir),
        ((2.0_f32 * 0.4_f32) * 65536.0_f32) as i32
    );
    // - Behind viewer, dist_sq > 4_000_000 (2500^2 = 6_250_000): factor 0.2
    assert_eq!(
        evaluate_destruction_info_priority(2.0, [-2500.0, 0.0, 0.0], viewer_loc, viewer_dir),
        ((2.0_f32 * 0.2_f32) * 65536.0_f32) as i32
    );
    // - In front of viewer, dist_sq <= 9_998_244 (3000^2 = 9_000_000): factor 1.0
    assert_eq!(
        evaluate_destruction_info_priority(2.0, [3000.0, 0.0, 0.0], viewer_loc, viewer_dir),
        131_072
    );
    // - In front of viewer, dist_sq > 9_998_244 (4000^2 = 16_000_000): factor 0.4
    assert_eq!(
        evaluate_destruction_info_priority(2.0, [4000.0, 0.0, 0.0], viewer_loc, viewer_dir),
        ((2.0_f32 * 0.4_f32) * 65536.0_f32) as i32
    );
    // - Non-finite saturates to i32::MIN via cvttss2si
    assert_eq!(
        evaluate_destruction_info_priority(f32::NAN, [0.0, 0.0, 0.0], viewer_loc, viewer_dir),
        i32::MIN
    );

    // 3. evaluate_prioritize_actor:
    let base_prio = PrioritizeActorInputs {
        has_channel: true,
        b_only_relevant_to_owner: false,
        is_relevant_to_at_least_one_viewer: true,
        driver_elapsed_time: 20.0,
        channel_relevant_time: 18.0,
        relevant_timeout: 5.0,
        actor_net_tag: 10,
        driver_net_tag: 11,
        is_actor_dormant_on_connection: false,
        actor_net_dormancy: 1,
        channel_b_pending_dormancy: false,
        channel_dormant: false,
        get_net_dormancy_returns_dormant_partial: false,
        level_initialized_for_actor: true,
        channel_last_update_time: 19.0,
        channel_open_elapsed_time: 4.0,
        max_net_priority_across_viewers: 3.0,
    };

    // Branch 1: !has_channel && b_only_relevant_to_owner && !is_relevant_to_at_least_one_viewer
    assert_eq!(
        evaluate_prioritize_actor(&PrioritizeActorInputs {
            has_channel: false,
            b_only_relevant_to_owner: true,
            is_relevant_to_at_least_one_viewer: false,
            ..base_prio
        }),
        PrioritizeActorOutcome::SkipNotRelevantToOwner
    );

    // Branch 2: actor_net_tag == driver_net_tag
    assert_eq!(
        evaluate_prioritize_actor(&PrioritizeActorInputs {
            actor_net_tag: 11,
            ..base_prio
        }),
        PrioritizeActorOutcome::SkipAlreadyTagged
    );

    // Branch 3a: has_channel && b_only_relevant_to_owner && !is_relevant_to_at_least_one_viewer, timeout exceeded
    assert_eq!(
        evaluate_prioritize_actor(&PrioritizeActorInputs {
            b_only_relevant_to_owner: true,
            is_relevant_to_at_least_one_viewer: false,
            driver_elapsed_time: 25.0,
            channel_relevant_time: 18.0,
            relevant_timeout: 5.0,
            ..base_prio
        }),
        PrioritizeActorOutcome::CloseChannelOwnerIrrelevantTimeout { close_reason: 3 }
    );

    // Branch 3b: has_channel && b_only_relevant_to_owner && !is_relevant_to_at_least_one_viewer, within timeout
    assert_eq!(
        evaluate_prioritize_actor(&PrioritizeActorInputs {
            b_only_relevant_to_owner: true,
            is_relevant_to_at_least_one_viewer: false,
            driver_elapsed_time: 23.0,
            channel_relevant_time: 18.0,
            relevant_timeout: 5.0,
            ..base_prio
        }),
        PrioritizeActorOutcome::SkipOwnerIrrelevantWithinTimeout
    );

    // Branch 4: dormant on connection
    assert_eq!(
        evaluate_prioritize_actor(&PrioritizeActorInputs {
            is_actor_dormant_on_connection: true,
            ..base_prio
        }),
        PrioritizeActorOutcome::SkipDormant
    );

    // Branch 5: DORM_DormantAll (2) or DORM_DormantPartial (3) with GetNetDormancy == 3 -> StartBecomingDormantAndSkip
    assert_eq!(
        evaluate_prioritize_actor(&PrioritizeActorInputs {
            actor_net_dormancy: 2,
            ..base_prio
        }),
        PrioritizeActorOutcome::StartBecomingDormantAndSkip
    );
    assert_eq!(
        evaluate_prioritize_actor(&PrioritizeActorInputs {
            actor_net_dormancy: 3,
            get_net_dormancy_returns_dormant_partial: true,
            ..base_prio
        }),
        PrioritizeActorOutcome::StartBecomingDormantAndSkip
    );

    // Branch 6: level not initialized
    assert_eq!(
        evaluate_prioritize_actor(&PrioritizeActorInputs {
            level_initialized_for_actor: false,
            ..base_prio
        }),
        PrioritizeActorOutcome::SkipLevelNotInitialized
    );

    // Branch 7a: Prioritized with existing channel (including channel_last_update_time == 0.0)
    assert_eq!(
        evaluate_prioritize_actor(&base_prio),
        PrioritizeActorOutcome::Prioritized {
            new_actor_net_tag: 11,
            time_since_last_update: 1.0,
            priority: 196_608,
        }
    );
    assert_eq!(
        evaluate_prioritize_actor(&PrioritizeActorInputs {
            channel_last_update_time: 0.0,
            ..base_prio
        }),
        PrioritizeActorOutcome::Prioritized {
            new_actor_net_tag: 11,
            time_since_last_update: 20.0,
            priority: 196_608,
        }
    );

    // Branch 7b: Prioritized without channel (`!has_channel`) uses channel_open_elapsed_time (`SpawnPrioritySeconds`)
    assert_eq!(
        evaluate_prioritize_actor(&PrioritizeActorInputs {
            has_channel: false,
            ..base_prio
        }),
        PrioritizeActorOutcome::Prioritized {
            new_actor_net_tag: 11,
            time_since_last_update: 4.0,
            priority: 196_608,
        }
    );

    // 4. evaluate_process_prioritized_actor:
    let base_proc = ProcessPrioritizedActorInputs {
        connection_is_net_ready: true,
        is_destruction_info: false,
        destruction_level_initialized: true,
        has_channel: true,
        driver_elapsed_time: 10.0,
        channel_relevant_time: 9.5,
        relevant_timeout: 5.0,
        is_relevant_to_connection: true,
        actor_b_tear_off: false,
        force_relevant_frame: 0,
        driver_replication_frame: 100,
        channel_b_is_replicating_actor: false,
        actor_b_always_relevant: false,
        actor_b_net_temporary: false,
        channel_open_acked: false,
        actor_level_initialized: true,
        package_map_supports_object: true,
        actor_net_update_frequency: 10.0,
        world_time_seconds: 50.0,
        random_unit_float: 0.5,
        can_create_actor_channel: true,
        replicate_actor_wrote_anything: true,
        connection_is_net_ready_after_replicate: true,
        actor_is_net_startup: false,
    };

    // Branch 1: connection saturated at entry
    assert_eq!(
        evaluate_process_prioritized_actor(&ProcessPrioritizedActorInputs {
            connection_is_net_ready: false,
            ..base_proc
        }),
        ProcessPrioritizedActorOutcome::StopConnectionSaturated
    );

    // Branch 2: destruction info (level initialized vs unloaded)
    assert_eq!(
        evaluate_process_prioritized_actor(&ProcessPrioritizedActorInputs {
            is_destruction_info: true,
            destruction_level_initialized: true,
            ..base_proc
        }),
        ProcessPrioritizedActorOutcome::SendDestructionInfo
    );
    assert_eq!(
        evaluate_process_prioritized_actor(&ProcessPrioritizedActorInputs {
            is_destruction_info: true,
            destruction_level_initialized: false,
            ..base_proc
        }),
        ProcessPrioritizedActorOutcome::SkipDestructionInfoLevelUnloaded
    );

    // Branch 4a: SkipAlreadyReplicatingOrAckedNetTemporary (bIsReplicatingActor or acked net temporary)
    assert_eq!(
        evaluate_process_prioritized_actor(&ProcessPrioritizedActorInputs {
            channel_b_is_replicating_actor: true,
            ..base_proc
        }),
        ProcessPrioritizedActorOutcome::SkipAlreadyReplicatingOrAckedNetTemporary {
            updated_relevant_time: Some(10.25),
        }
    );
    assert_eq!(
        evaluate_process_prioritized_actor(&ProcessPrioritizedActorInputs {
            actor_b_always_relevant: true,
            actor_b_net_temporary: true,
            channel_open_acked: true,
            actor_b_tear_off: true,
            driver_elapsed_time: 10.0,
            channel_relevant_time: 8.0,
            relevant_timeout: 5.0,
            ..base_proc
        }),
        ProcessPrioritizedActorOutcome::SkipAlreadyReplicatingOrAckedNetTemporary {
            updated_relevant_time: None,
        }
    );

    // Branch 4b: !has_channel and (!actor_level_initialized || !package_map_supports_object)
    // - Low frequency (< 1.0) schedules WorldTime + 0.2 * random
    assert_eq!(
        evaluate_process_prioritized_actor(&ProcessPrioritizedActorInputs {
            has_channel: false,
            actor_level_initialized: false,
            actor_net_update_frequency: 0.5,
            world_time_seconds: 50.0,
            random_unit_float: 0.5,
            ..base_proc
        }),
        ProcessPrioritizedActorOutcome::DeferCannotOpenChannel {
            updated_next_update_time: Some(50.1),
        }
    );
    // - Normal frequency (>= 1.0) does not touch NextUpdateTime
    assert_eq!(
        evaluate_process_prioritized_actor(&ProcessPrioritizedActorInputs {
            has_channel: false,
            package_map_supports_object: false,
            actor_net_update_frequency: 2.0,
            ..base_proc
        }),
        ProcessPrioritizedActorOutcome::DeferCannotOpenChannel {
            updated_next_update_time: None,
        }
    );

    // Branch 4c: !has_channel and channel creation fails
    assert_eq!(
        evaluate_process_prioritized_actor(&ProcessPrioritizedActorInputs {
            has_channel: false,
            can_create_actor_channel: false,
            ..base_proc
        }),
        ProcessPrioritizedActorOutcome::DeferChannelCreationFailed
    );

    // Branch 4d: ReplicateActorAndStopSaturated vs ReplicateActorContinue
    assert_eq!(
        evaluate_process_prioritized_actor(&ProcessPrioritizedActorInputs {
            has_channel: false,
            connection_is_net_ready_after_replicate: false,
            ..base_proc
        }),
        ProcessPrioritizedActorOutcome::ReplicateActorAndStopSaturated {
            opened_new_channel: true,
            wrote_anything: true,
            updated_relevant_time: Some(10.25),
        }
    );
    assert_eq!(
        evaluate_process_prioritized_actor(&base_proc),
        ProcessPrioritizedActorOutcome::ReplicateActorContinue {
            opened_new_channel: false,
            wrote_anything: true,
            updated_relevant_time: Some(10.25),
        }
    );

    // Branch 5: !is_recently_relevant -> CloseChannel (4 if tear-off else 3) or SkipNotRelevantNoClose
    let not_recent = ProcessPrioritizedActorInputs {
        driver_elapsed_time: 20.0,
        channel_relevant_time: 10.0,
        relevant_timeout: 5.0,
        is_relevant_to_connection: false,
        force_relevant_frame: 50,
        driver_replication_frame: 100,
        ..base_proc
    };
    assert_eq!(
        evaluate_process_prioritized_actor(&not_recent),
        ProcessPrioritizedActorOutcome::CloseChannel { close_reason: 3 }
    );
    assert_eq!(
        evaluate_process_prioritized_actor(&ProcessPrioritizedActorInputs {
            actor_b_tear_off: true,
            ..not_recent
        }),
        ProcessPrioritizedActorOutcome::CloseChannel { close_reason: 4 }
    );
    // NetStartup actor whose level is still initialized does not close its channel here:
    assert_eq!(
        evaluate_process_prioritized_actor(&ProcessPrioritizedActorInputs {
            actor_is_net_startup: true,
            actor_level_initialized: true,
            ..not_recent
        }),
        ProcessPrioritizedActorOutcome::SkipNotRelevantNoClose
    );

    // 5. evaluate_mark_relevant_actor:
    assert_eq!(
        evaluate_mark_relevant_actor(true, false, 41, 10, false),
        (42, true)
    );
    assert_eq!(
        evaluate_mark_relevant_actor(true, false, i32::MAX, 10, false),
        (i32::MIN, true)
    );
    assert_eq!(
        evaluate_mark_relevant_actor(false, false, 41, 10, false),
        (10, false)
    );
    assert_eq!(
        evaluate_mark_relevant_actor(true, true, 41, 10, false),
        (10, false)
    );
}

#[test]
fn server_replicate_actors_frame_is_net_ready_mark_relevant_and_replicate_actor() {
    use icarus_bevy::experimental::actor_channel::{
        CHANNEL_RELIABLE_BUFFER_NET_READY_LIMIT, ConnectionNetReadyInputs,
        MarkRelevantActorStepInputs, MarkRelevantActorStepOutcome, ReplicateActorInputs,
        ReplicateActorOutcome, ReplicateActorRepFlags,
        SERVER_REPLICATE_ACTORS_CPU_SATURATION_FACTOR, ServerReplicateActorsFrameInputs,
        ServerReplicateActorsFrameOutcome, evaluate_actor_channel_replicate_actor,
        evaluate_channel_is_net_ready, evaluate_connection_is_net_ready,
        evaluate_mark_relevant_actor_step, evaluate_server_replicate_actors_frame,
    };

    assert_eq!(SERVER_REPLICATE_ACTORS_CPU_SATURATION_FACTOR, 1.2);
    assert_eq!(CHANNEL_RELIABLE_BUFFER_NET_READY_LIMIT, 255);

    let base_conn = ConnectionNetReadyInputs {
        saturate: false,
        queued_bits: -100,
        send_buffer_num_bits: 100,
        b_use_adaptive_net_frequency: false,
        adaptive_queued_bytes: 500,
        adaptive_max_bytes_per_sec: 10_000.0,
        adaptive_delta_time: 0.1,
        adaptive_out_packet_id: 20,
        adaptive_out_ack_packet_id: 15,
        adaptive_max_in_flight_packets: 10,
    };
    assert_eq!(evaluate_connection_is_net_ready(&base_conn), (true, -100));
    assert_eq!(
        evaluate_connection_is_net_ready(&ConnectionNetReadyInputs {
            queued_bits: -99,
            ..base_conn
        }),
        (false, -99)
    );
    assert_eq!(
        evaluate_connection_is_net_ready(&ConnectionNetReadyInputs {
            saturate: true,
            queued_bits: 500,
            ..base_conn
        }),
        (true, -100)
    );
    assert_eq!(
        evaluate_connection_is_net_ready(&ConnectionNetReadyInputs {
            b_use_adaptive_net_frequency: true,
            ..base_conn
        }),
        (true, -100)
    );
    assert_eq!(
        evaluate_connection_is_net_ready(&ConnectionNetReadyInputs {
            b_use_adaptive_net_frequency: true,
            adaptive_queued_bytes: 1_000,
            ..base_conn
        }),
        (false, -100)
    );
    assert_eq!(evaluate_channel_is_net_ready(254, &base_conn), (true, -100));
    assert_eq!(
        evaluate_channel_is_net_ready(255, &base_conn),
        (false, -100)
    );

    let base_mark = MarkRelevantActorStepInputs {
        actor_info_present: true,
        has_channel: true,
        driver_elapsed_time: 10.0,
        channel_relevant_time: 9.5,
        is_relevant_to_any_viewer: false,
        random_unit_float: 0.5,
        current_b_pending_net_update: false,
        current_force_relevant_frame: 10,
        connection_last_processed_frame: 10,
        driver_replication_frame: 14,
    };
    assert_eq!(
        evaluate_mark_relevant_actor_step(&base_mark),
        MarkRelevantActorStepOutcome {
            b_pending_net_update: true,
            updated_channel_relevant_time: None,
            force_relevant_frame: 15,
        }
    );
    assert_eq!(
        evaluate_mark_relevant_actor_step(&MarkRelevantActorStepInputs {
            channel_relevant_time: 8.5,
            is_relevant_to_any_viewer: true,
            current_force_relevant_frame: 9,
            ..base_mark
        }),
        MarkRelevantActorStepOutcome {
            b_pending_net_update: true,
            updated_channel_relevant_time: Some(10.75),
            force_relevant_frame: 9,
        }
    );

    let base_frame = ServerReplicateActorsFrameInputs {
        num_client_connections: 4,
        has_replication_driver: false,
        replication_driver_updated_count: 9,
        current_replication_frame: 100,
        has_world: true,
        num_clients_to_tick: 2,
        delta_seconds: 0.05,
        max_tick_rate: 30.0,
    };
    assert_eq!(
        evaluate_server_replicate_actors_frame(&ServerReplicateActorsFrameInputs {
            num_client_connections: 0,
            ..base_frame
        }),
        ServerReplicateActorsFrameOutcome::NoClientConnections
    );
    assert_eq!(
        evaluate_server_replicate_actors_frame(&ServerReplicateActorsFrameInputs {
            has_replication_driver: true,
            ..base_frame
        }),
        ServerReplicateActorsFrameOutcome::DelegatedToReplicationDriver { updated: 9 }
    );
    assert_eq!(
        evaluate_server_replicate_actors_frame(&ServerReplicateActorsFrameInputs {
            num_clients_to_tick: 0,
            ..base_frame
        }),
        ServerReplicateActorsFrameOutcome::AbortedAfterFrameIncrement {
            next_replication_frame: 101,
        }
    );
    assert_eq!(
        evaluate_server_replicate_actors_frame(&base_frame),
        ServerReplicateActorsFrameOutcome::RunConnectionReplication {
            next_replication_frame: 101,
            server_tick_time: 1.0 / 30.0,
            clamped_delta_seconds: 1.0 / 30.0,
            b_cpu_saturated: true,
            rotate_connections_left_by: 2,
        }
    );

    let base_rep = ReplicateActorInputs {
        channel_closing: false,
        connection_resend_all_data_state: 0,
        channel_b_is_replicating_actor: false,
        channel_b_actor_is_pending_kill: false,
        actor_uobject_pending_kill_or_unreachable: false,
        open_packet_id_first_is_none: true,
        all_level_components_ready: true,
        channel_clean_up_or_has_bunch_check: false,
        out_bunch_is_error: false,
        channel_b_spawned_for_replay: false,
        channel_spawn_acked: true,
        actor_owned_by_connection: false,
        actor_remote_role: 2,
        actor_b_rep_physics: true,
        connection_is_replay: false,
        actor_properties_wrote: true,
        subobjects_wrote: false,
        deleted_subobjects_wrote: false,
        bunch_bits_written: 128,
        bunch_is_partial_initial: false,
        driver_elapsed_time: 42.5,
    };
    assert_eq!(
        evaluate_actor_channel_replicate_actor(&ReplicateActorInputs {
            channel_closing: true,
            ..base_rep
        }),
        ReplicateActorOutcome::EarlyExitChannelClosing
    );
    assert_eq!(
        evaluate_actor_channel_replicate_actor(&ReplicateActorInputs {
            channel_b_is_replicating_actor: true,
            ..base_rep
        }),
        ReplicateActorOutcome::EarlyExitRecursiveReplication
    );
    assert_eq!(
        evaluate_actor_channel_replicate_actor(&ReplicateActorInputs {
            actor_uobject_pending_kill_or_unreachable: true,
            ..base_rep
        }),
        ReplicateActorOutcome::EarlyExitMarkActorPendingKillAndReleaseReplicator
    );
    assert_eq!(
        evaluate_actor_channel_replicate_actor(&base_rep),
        ReplicateActorOutcome::Completed {
            rep_flags: ReplicateActorRepFlags {
                b_net_owner: false,
                b_net_initial: true,
                b_net_simulated: true,
                b_rep_physics: true,
                b_replay: false,
                raw_bits: 0b01110,
                serialize_new_actor: true,
                downgraded_autonomous_proxy_for_non_owner: true,
            },
            sent_bunch: true,
            bits_written: 128,
            updated_last_update_time: 42.5,
            clear_b_force_compare_properties: true,
            queue_partial_initial_retry: false,
        }
    );
}

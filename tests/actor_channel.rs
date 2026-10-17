use icarus_bevy::experimental::actor_channel::{
    ActorBunchBody, ActorChannelCodec, ActorChannelLimits, BlockTarget, CompressedRotator,
    ContentBlock, DynamicSpawn, ExportPath, GuidExport, GuidKind, MAX_EXPORT_GUIDS, NetGuidRef,
    SpawnRecord, SpawnVector, SubObjectClass, dequantize_angle, encode_actor_bunch_body,
    encode_player_controller_open_bunch_body, guid_kind, parse_actor_bunch,
    parse_player_controller_open_bunch, quantize_angle, read_actor_bunch_body, read_content_block,
    read_export_prefix, read_guid_ref, read_must_be_mapped_guids,
    read_player_controller_open_bunch_body, read_rotator, read_spawn, write_actor_bunch_body,
    write_content_block, write_export_prefix, write_guid_ref, write_must_be_mapped_guids,
    write_player_controller_open_bunch_body, write_rotator, write_spawn,
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

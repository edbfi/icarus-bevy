use icarus_bevy::experimental::actor_channel::{
    ActorChannelCodec, ActorChannelLimits, CompressedRotator,
};
use icarus_bevy::experimental::archive::ByteOrder;
use icarus_bevy::experimental::packet::{BitReader, BitWriter, Payload};
use icarus_bevy::experimental::rep_layout::{
    ByteRotator, MovementLevel, MovementRotation, PropertyStream, QuantizedVector, RepCommand,
    RepEntry, RepLayoutCodec, RepLayoutLimits, RepMovement, RepValue, RotationLevel, VectorPacking,
    dequantize_byte_angle, encode_property_stream, parse_property_stream, quantize_byte_angle,
    read_byte_rotator, read_movement, read_property_stream,
};

// Independently authored bit fixtures for the movement struct and the scaled
// vector packings (F-588, F-589), built field by field; no captured traffic.

fn plain() -> RepLayoutCodec {
    RepLayoutCodec {
        actor: ActorChannelCodec {
            engine_net_version: 13,
            exports_enabled: false,
            has_level: false,
            order: ByteOrder::Little,
            limits: ActorChannelLimits::default(),
        },
        checksums: false,
        limits: RepLayoutLimits::default(),
    }
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

/// Hand-written `SerializeInt(value, max)`.
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

/// A capped vector: bit count below `bits_max`, then three components.
fn vector(writer: &mut BitWriter, bits_max: u32, value: QuantizedVector) {
    capped(writer, value.bits, bits_max);
    for component in value.components {
        capped(writer, component, 1 << (value.bits + 2));
    }
}

fn single(value: RepValue) -> PropertyStream {
    PropertyStream {
        entries: vec![RepEntry {
            handle: 1,
            checksum: None,
            value,
        }],
        terminator_checksum: None,
    }
}

/// Exact parse, bit-identical re-encode and every proper prefix rejected.
fn assert_stream(fixture: &(Vec<u8>, usize), layout: &[RepCommand], expected: &PropertyStream) {
    let codec = plain();
    let payload = to_payload(fixture);
    assert_eq!(
        &parse_property_stream(&payload, layout, &codec).unwrap(),
        expected
    );
    assert_eq!(
        encode_property_stream(expected, layout, &codec).unwrap(),
        payload
    );
    for len in 0..fixture.1 {
        let mut reader = BitReader::new(&fixture.0, len).unwrap();
        assert!(
            read_property_stream(&mut reader, layout, &codec).is_err(),
            "prefix {len}"
        );
    }
}

fn error_of<T: std::fmt::Debug>(result: anyhow::Result<T>) -> String {
    format!("{:#}", result.unwrap_err())
}

const LOCATION: QuantizedVector = QuantizedVector {
    bits: 5,
    components: [3, 64, 127],
};
const LINEAR: QuantizedVector = QuantizedVector {
    bits: 26,
    components: [1, (1 << 28) - 1, 1 << 27],
};
const ANGULAR: QuantizedVector = QuantizedVector {
    bits: 0,
    components: [0, 1, 3],
};

#[test]
fn packings_follow_the_observed_helpers() {
    let table = [
        (VectorPacking::NetQuantize, 20, 1),
        (VectorPacking::NetQuantize10, 24, 10),
        (VectorPacking::NetQuantize100, 30, 100),
        (VectorPacking::MovementWhole, 24, 1),
        (VectorPacking::MovementTenths, 27, 10),
        (VectorPacking::MovementHundredths, 30, 100),
    ];
    for (packing, bits_max, divisor) in table {
        assert_eq!(packing.bits_max(), bits_max, "{packing:?}");
        assert_eq!(packing.divisor(), divisor, "{packing:?}");
    }
    assert_eq!(MovementLevel::Whole.packing(), VectorPacking::MovementWhole);
    assert_eq!(
        MovementLevel::Tenths.packing(),
        VectorPacking::MovementTenths
    );
    assert_eq!(
        MovementLevel::Hundredths.packing(),
        VectorPacking::MovementHundredths
    );
    // The widest valid count still unbiases without overflow.
    let widest = QuantizedVector {
        bits: 29,
        components: [0, 1 << 30, (1u32 << 31) - 1],
    };
    assert_eq!(widest.unbiased(), [-(1 << 30), 0, (1 << 30) - 1]);
}

#[test]
fn scaled_vectors_round_trip_at_their_caps() {
    for (command, bits_max, wrap) in [
        (
            RepCommand::VectorQuantize10,
            24,
            RepValue::VectorQuantize10 as fn(QuantizedVector) -> RepValue,
        ),
        (
            RepCommand::VectorQuantize100,
            30,
            RepValue::VectorQuantize100,
        ),
    ] {
        let value = QuantizedVector {
            bits: bits_max - 1,
            components: [0, 1 << bits_max, (1u32 << (bits_max + 1)) - 1],
        };
        let mut writer = BitWriter::new();
        packed(&mut writer, 1);
        vector(&mut writer, bits_max, value);
        packed(&mut writer, 0);
        let layout = [command.clone()];
        assert_stream(&bits(&writer), &layout, &single(wrap(value)));
        // A count at the cap is rejected on write; the reader cannot express it.
        let wide = QuantizedVector {
            bits: bits_max,
            components: [0; 3],
        };
        assert!(
            error_of(encode_property_stream(
                &single(wrap(wide)),
                &layout,
                &plain()
            ))
            .contains(&format!("below {bits_max}"))
        );
    }
}

fn movement_layout(rotation: RotationLevel) -> [RepCommand; 1] {
    [RepCommand::Movement {
        location: MovementLevel::Whole,
        velocity: MovementLevel::Tenths,
        rotation,
    }]
}

#[test]
fn movement_with_byte_rotation_and_angular_velocity() {
    let mut writer = BitWriter::new();
    packed(&mut writer, 1);
    writer.write_bits(0b11, 2);
    vector(&mut writer, 24, LOCATION);
    writer.write_bit(true);
    writer.write_bits(0x40, 8);
    writer.write_bit(false);
    writer.write_bit(true);
    writer.write_bits(0xc0, 8);
    vector(&mut writer, 27, LINEAR);
    vector(&mut writer, 27, ANGULAR);
    packed(&mut writer, 0);
    let rotator = ByteRotator {
        pitch: Some(0x40),
        yaw: None,
        roll: Some(0xc0),
    };
    let expected = single(RepValue::Movement(RepMovement {
        flags: 3,
        location: LOCATION,
        rotation: Some(MovementRotation::Bytes(rotator)),
        linear_velocity: LINEAR,
        angular_velocity: Some(ANGULAR),
    }));
    assert_stream(
        &bits(&writer),
        &movement_layout(RotationLevel::Bytes),
        &expected,
    );
    assert_eq!(rotator.degrees(), [90.0, 0.0, 270.0]);
    let summary = expected.entries[0].summary();
    let movement = summary.movement.unwrap();
    assert_eq!(movement.rotation_axes_present, Some(2));
    assert!(movement.angular_velocity);
}

#[test]
fn movement_with_short_or_omitted_rotation_and_no_angular_velocity() {
    let shorts = CompressedRotator {
        pitch: None,
        yaw: Some(0x8000),
        roll: None,
    };
    for (level, rotation) in [
        (
            RotationLevel::Shorts,
            Some(MovementRotation::Shorts(shorts)),
        ),
        (RotationLevel::Omitted, None),
    ] {
        let mut writer = BitWriter::new();
        packed(&mut writer, 1);
        // Flag bit 0 alone does not add an angular velocity.
        writer.write_bits(0b01, 2);
        vector(&mut writer, 24, LOCATION);
        if level == RotationLevel::Shorts {
            writer.write_bit(false);
            writer.write_bit(true);
            writer.write_bits(0x8000, 16);
            writer.write_bit(false);
        }
        vector(&mut writer, 27, LINEAR);
        packed(&mut writer, 0);
        let expected = single(RepValue::Movement(RepMovement {
            flags: 1,
            location: LOCATION,
            rotation,
            linear_velocity: LINEAR,
            angular_velocity: None,
        }));
        assert_stream(&bits(&writer), &movement_layout(level), &expected);
    }
}

#[test]
fn movement_levels_select_the_vector_caps() {
    // Hundredths location and whole velocity: caps 30 and 24.
    let location = QuantizedVector {
        bits: 29,
        components: [5, 6, 7],
    };
    let velocity = QuantizedVector {
        bits: 23,
        components: [8, 9, 10],
    };
    let mut writer = BitWriter::new();
    writer.write_bits(0, 2);
    vector(&mut writer, 30, location);
    vector(&mut writer, 24, velocity);
    let fixture = bits(&writer);
    let mut reader = BitReader::new(&fixture.0, fixture.1).unwrap();
    let movement = read_movement(
        &mut reader,
        MovementLevel::Hundredths,
        MovementLevel::Whole,
        RotationLevel::Omitted,
    )
    .unwrap();
    assert!(reader.at_end());
    assert_eq!(movement.location, location);
    assert_eq!(movement.linear_velocity, velocity);
    // The same bits read at the tenths level no longer frame.
    let mut reader = BitReader::new(&fixture.0, fixture.1).unwrap();
    let other = read_movement(
        &mut reader,
        MovementLevel::Tenths,
        MovementLevel::Whole,
        RotationLevel::Omitted,
    );
    assert!(other.is_err() || !reader.at_end());
}

#[test]
fn movement_writer_rejects_inconsistent_parts() {
    let base = RepMovement {
        flags: 0,
        location: LOCATION,
        rotation: None,
        linear_velocity: LINEAR,
        angular_velocity: None,
    };
    let layout = movement_layout(RotationLevel::Omitted);
    let cases = [
        (
            RepMovement {
                angular_velocity: Some(ANGULAR),
                ..base.clone()
            },
            "angular velocity",
        ),
        (
            RepMovement {
                flags: 2,
                ..base.clone()
            },
            "angular velocity",
        ),
        (
            RepMovement {
                flags: 4,
                ..base.clone()
            },
            "two bits",
        ),
        (
            RepMovement {
                rotation: Some(MovementRotation::Bytes(ByteRotator::default())),
                ..base.clone()
            },
            "rotation level",
        ),
        (
            RepMovement {
                linear_velocity: QuantizedVector {
                    bits: 27,
                    components: [0; 3],
                },
                ..base.clone()
            },
            "below 27",
        ),
    ];
    for (movement, message) in cases {
        let error = error_of(encode_property_stream(
            &single(RepValue::Movement(movement)),
            &layout,
            &plain(),
        ));
        assert!(error.contains(message), "{error}");
    }
    let shorts = movement_layout(RotationLevel::Shorts);
    let error = error_of(encode_property_stream(
        &single(RepValue::Movement(RepMovement {
            rotation: Some(MovementRotation::Bytes(ByteRotator::default())),
            ..base.clone()
        })),
        &shorts,
        &plain(),
    ));
    assert!(error.contains("rotation level"), "{error}");
    assert!(
        error_of(encode_property_stream(
            &single(RepValue::Movement(base)),
            &[RepCommand::VectorQuantize10],
            &plain(),
        ))
        .contains("does not match")
    );
}

#[test]
fn byte_angles_round_half_up_and_wrap() {
    assert_eq!(quantize_byte_angle(0.0).unwrap(), 0);
    assert_eq!(quantize_byte_angle(90.0).unwrap(), 64);
    assert_eq!(quantize_byte_angle(-90.0).unwrap(), 192);
    assert_eq!(quantize_byte_angle(360.0).unwrap(), 0);
    // 0.703125 degrees is exactly half a step.
    assert_eq!(quantize_byte_angle(0.703125).unwrap(), 1);
    assert!(quantize_byte_angle(f32::NAN).is_err());
    assert!(quantize_byte_angle(f32::INFINITY).is_err());
    assert_eq!(dequantize_byte_angle(128), 180.0);
    let rotator = ByteRotator::from_degrees(90.0, 0.0, -90.0).unwrap();
    assert_eq!(
        rotator,
        ByteRotator {
            pitch: Some(64),
            yaw: None,
            roll: Some(192),
        }
    );
    // A set bit with a zero byte is preserved as read.
    let mut writer = BitWriter::new();
    writer.write_bit(true);
    writer.write_bits(0, 8);
    writer.write_bit(false);
    writer.write_bit(false);
    let fixture = bits(&writer);
    let mut reader = BitReader::new(&fixture.0, fixture.1).unwrap();
    assert_eq!(
        read_byte_rotator(&mut reader).unwrap(),
        ByteRotator {
            pitch: Some(0),
            yaw: None,
            roll: None,
        }
    );
    assert!(reader.at_end());
    assert!(!format!("{rotator:?}").contains("64"));
}

#[test]
fn quantize_vector_matches_the_bit_exact_spawn_packing_and_bounds() {
    use icarus_bevy::experimental::actor_channel::SpawnVector;
    use icarus_bevy::experimental::rep_layout::quantize_vector;
    use icarus_bevy::experimental::session_open::quantize_spawn_vector;
    // `<10, 24>` is the spawn packing reproduced bit for bit (F-779).
    for value in [
        [248.0, 3.0, 103_485.2],
        [0.0, 0.0, 0.0],
        [-1.25, 7.5, -99_999.9],
    ] {
        let SpawnVector::Quantized { bits, components } = quantize_spawn_vector(value).unwrap()
        else {
            panic!("quantized");
        };
        let quantized = quantize_vector(value, VectorPacking::NetQuantize10).unwrap();
        assert_eq!((quantized.bits, quantized.components), (bits, components));
    }
    // Whole-number movement: unscaled, round half up, biased.
    let whole = quantize_vector([500.4, -2.5, 0.0], VectorPacking::MovementWhole).unwrap();
    assert_eq!(whole.unbiased(), [500, -2, 0]);
    assert_eq!(whole.bits, 8, "CeilLogTwo(1 + 500) - 1");
    let hundredths = quantize_vector([1.234, 0.0, 0.0], VectorPacking::MovementHundredths).unwrap();
    assert_eq!(hundredths.unbiased(), [123, 0, 0]);
    // The bit count must stay below the packing's maximum; no clamping.
    assert!(quantize_vector([8_388_607.0, 0.0, 0.0], VectorPacking::MovementWhole).is_ok());
    assert!(quantize_vector([16_777_216.0, 0.0, 0.0], VectorPacking::MovementWhole).is_err());
    assert!(quantize_vector([f32::NAN, 0.0, 0.0], VectorPacking::MovementWhole).is_err());
}

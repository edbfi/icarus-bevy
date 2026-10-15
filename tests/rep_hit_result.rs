use icarus_bevy::experimental::actor_channel::{ActorChannelCodec, ActorChannelLimits, NetGuidRef};
use icarus_bevy::experimental::archive::ByteOrder;
use icarus_bevy::experimental::packet::{BitReader, BitWriter, Payload};
use icarus_bevy::experimental::rep_layout::{
    HIT_IMPACT_NORMAL_IS_NORMAL, HIT_IMPACT_POINT_IS_LOCATION, HIT_NO_ELEMENT_INDEX,
    HIT_NO_FACE_INDEX, HIT_NO_ITEM, HIT_NO_PENETRATION, HitResult, NetworkName, PropertyStream,
    QuantizedNormal, QuantizedVector, RepCommand, RepEntry, RepLayoutCodec, RepLayoutLimits,
    RepValue, encode_property_stream, parse_property_stream, read_property_stream,
};

// Independently authored bit fixtures for the hit result (F-597), built field
// by field; no captured traffic.

fn codec(order: ByteOrder) -> RepLayoutCodec {
    RepLayoutCodec {
        actor: ActorChannelCodec {
            engine_net_version: 13,
            exports_enabled: false,
            has_level: false,
            order,
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

fn vector(writer: &mut BitWriter, value: QuantizedVector) {
    capped(writer, value.bits, 20);
    for component in value.components {
        capped(writer, component, 1 << (value.bits + 2));
    }
}

fn normal(writer: &mut BitWriter, value: QuantizedNormal) {
    for component in value.0 {
        writer.write_bits(u32::from(component), 16);
    }
}

fn word(writer: &mut BitWriter, value: u32, order: ByteOrder) {
    match order {
        ByteOrder::Little => writer.write_bytes(&value.to_le_bytes()),
        ByteOrder::Big => writer.write_bytes(&value.to_be_bytes()),
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

fn assert_stream(fixture: &(Vec<u8>, usize), expected: &PropertyStream, codec: &RepLayoutCodec) {
    let payload = to_payload(fixture);
    let layout = [RepCommand::HitResult];
    assert_eq!(
        &parse_property_stream(&payload, &layout, codec).unwrap(),
        expected
    );
    assert_eq!(
        encode_property_stream(expected, &layout, codec).unwrap(),
        payload
    );
    for len in 0..fixture.1 {
        let mut reader = BitReader::new(&fixture.0, len).unwrap();
        assert!(
            read_property_stream(&mut reader, &layout, codec).is_err(),
            "prefix {len}"
        );
    }
}

fn error_of<T: std::fmt::Debug>(result: anyhow::Result<T>) -> String {
    format!("{:#}", result.unwrap_err())
}

const V1: QuantizedVector = QuantizedVector {
    bits: 3,
    components: [1, 2, 3],
};
const V2: QuantizedVector = QuantizedVector {
    bits: 0,
    components: [0, 1, 2],
};
const V3: QuantizedVector = QuantizedVector {
    bits: 5,
    components: [64, 65, 127],
};
const V4: QuantizedVector = QuantizedVector {
    bits: 1,
    components: [7, 0, 3],
};
const N1: QuantizedNormal = QuantizedNormal([32_768, 0, 65_535]);
const N2: QuantizedNormal = QuantizedNormal([1, 2, 3]);

fn full(order: ByteOrder) -> ((Vec<u8>, usize), HitResult) {
    // Bits 0 and 1 set (carried struct flags); every field present.
    let mut writer = BitWriter::new();
    packed(&mut writer, 1);
    writer.write_bits(0b11, 8);
    word(&mut writer, 0x3f00_0000, order);
    vector(&mut writer, V1);
    normal(&mut writer, N1);
    vector(&mut writer, V2);
    normal(&mut writer, N2);
    vector(&mut writer, V3);
    vector(&mut writer, V4);
    word(&mut writer, 0x4120_0000, order);
    word(&mut writer, 7, order);
    packed(&mut writer, 10);
    packed(&mut writer, 300);
    packed(&mut writer, 0);
    writer.write_bit(true);
    packed(&mut writer, 255);
    word(&mut writer, 42, order);
    writer.write_bits(9, 8);
    packed(&mut writer, 0);
    let hit = HitResult {
        flags: 0b11,
        time: 0x3f00_0000,
        location: V1,
        normal: N1,
        impact_point: Some(V2),
        impact_normal: Some(N2),
        trace_start: V3,
        trace_end: V4,
        penetration_depth: Some(0x4120_0000),
        item: Some(7),
        phys_material: NetGuidRef::plain(10),
        actor: NetGuidRef::plain(300),
        component: NetGuidRef::plain(0),
        bone_name: NetworkName::Hardcoded(255),
        face_index: Some(42),
        element_index: Some(9),
    };
    (bits(&writer), hit)
}

#[test]
fn a_full_hit_result_round_trips_in_both_byte_orders() {
    for order in [ByteOrder::Little, ByteOrder::Big] {
        let (fixture, hit) = full(order);
        assert_stream(
            &fixture,
            &single(RepValue::HitResult(Box::new(hit))),
            &codec(order),
        );
    }
}

#[test]
fn every_flag_bit_replaces_its_field() {
    let order = ByteOrder::Little;
    let flags = HIT_IMPACT_POINT_IS_LOCATION
        | HIT_IMPACT_NORMAL_IS_NORMAL
        | HIT_NO_ITEM
        | HIT_NO_FACE_INDEX
        | HIT_NO_PENETRATION
        | HIT_NO_ELEMENT_INDEX;
    assert_eq!(flags, 0xfc);
    let mut writer = BitWriter::new();
    packed(&mut writer, 1);
    writer.write_bits(u32::from(flags), 8);
    word(&mut writer, 0, order);
    vector(&mut writer, V1);
    normal(&mut writer, N1);
    vector(&mut writer, V3);
    vector(&mut writer, V4);
    packed(&mut writer, 0);
    packed(&mut writer, 0);
    packed(&mut writer, 0);
    writer.write_bit(true);
    packed(&mut writer, 1);
    packed(&mut writer, 0);
    let hit = HitResult {
        flags,
        time: 0,
        location: V1,
        normal: N1,
        impact_point: None,
        impact_normal: None,
        trace_start: V3,
        trace_end: V4,
        penetration_depth: None,
        item: None,
        phys_material: NetGuidRef::plain(0),
        actor: NetGuidRef::plain(0),
        component: NetGuidRef::plain(0),
        bone_name: NetworkName::Hardcoded(1),
        face_index: None,
        element_index: None,
    };
    assert_stream(
        &bits(&writer),
        &single(RepValue::HitResult(Box::new(hit))),
        &codec(order),
    );
}

#[test]
fn the_writer_rejects_fields_inconsistent_with_flags() {
    let (_, base) = full(ByteOrder::Little);
    let layout = [RepCommand::HitResult];
    type Change = fn(&mut HitResult);
    let cases: [(Change, &str); 6] = [
        (
            |hit| hit.flags |= HIT_IMPACT_POINT_IS_LOCATION,
            "impact point",
        ),
        (|hit| hit.impact_normal = None, "impact normal"),
        (|hit| hit.flags |= HIT_NO_PENETRATION, "penetration depth"),
        (|hit| hit.item = None, "item"),
        (|hit| hit.flags |= HIT_NO_FACE_INDEX, "face index"),
        (|hit| hit.flags |= HIT_NO_ELEMENT_INDEX, "element index"),
    ];
    for (change, message) in cases {
        let mut hit = base.clone();
        change(&mut hit);
        let error = error_of(encode_property_stream(
            &single(RepValue::HitResult(Box::new(hit))),
            &layout,
            &codec(ByteOrder::Little),
        ));
        assert!(error.contains(message), "{error}");
    }
    // A vector over the NetQuantize cap fails.
    let mut hit = base.clone();
    hit.trace_end.bits = 20;
    assert!(
        error_of(encode_property_stream(
            &single(RepValue::HitResult(Box::new(hit))),
            &layout,
            &codec(ByteOrder::Little),
        ))
        .contains("below 20")
    );
    // Debug output carries no values.
    assert!(!format!("{base:?}").contains("42"));
    let summary = single(RepValue::HitResult(Box::new(base))).entries[0].summary();
    assert!(summary.vector_bits.is_none());
}

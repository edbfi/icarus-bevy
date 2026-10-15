use icarus_bevy::experimental::actor_channel::{ActorChannelCodec, ActorChannelLimits};
use icarus_bevy::experimental::archive::ByteOrder;
use icarus_bevy::experimental::control::TokenEncoding;
use icarus_bevy::experimental::control_messages::ArchiveText;
use icarus_bevy::experimental::packet::{BitReader, BitWriter, Payload};
use icarus_bevy::experimental::rep_layout::{
    BaseText, ItemKind, PropertyStream, RepCommand, RepEntry, RepLayoutCodec, RepLayoutLimits,
    RepValue, encode_property_stream, parse_property_stream, read_property_stream,
};

// Independently authored bit fixtures for Text items with a Base history
// (F-613, F-614): flags word, history-type byte, two text keys and the source
// string, built field by field; no captured traffic.

fn codec(order: ByteOrder, change: impl Fn(&mut RepLayoutLimits)) -> RepLayoutCodec {
    let mut limits = RepLayoutLimits::default();
    change(&mut limits);
    RepLayoutCodec {
        actor: ActorChannelCodec {
            engine_net_version: 13,
            exports_enabled: false,
            has_level: false,
            order,
            limits: ActorChannelLimits::default(),
        },
        checksums: false,
        limits,
    }
}

fn plain() -> RepLayoutCodec {
    codec(ByteOrder::Little, |_| {})
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

const LAYOUT: [RepCommand; 1] = [RepCommand::Text];

fn assert_stream(fixture: &(Vec<u8>, usize), expected: &PropertyStream, codec: &RepLayoutCodec) {
    let payload = to_payload(fixture);
    assert_eq!(
        &parse_property_stream(&payload, &LAYOUT, codec).unwrap(),
        expected
    );
    assert_eq!(
        encode_property_stream(expected, &LAYOUT, codec).unwrap(),
        payload
    );
    for len in 0..fixture.1 {
        let mut reader = BitReader::new(&fixture.0, len).unwrap();
        assert!(
            read_property_stream(&mut reader, &LAYOUT, codec).is_err(),
            "prefix {len}"
        );
    }
}

fn error_of<T: std::fmt::Debug>(result: anyhow::Result<T>) -> String {
    format!("{:#}", result.unwrap_err())
}

fn narrow(value: &str) -> ArchiveText {
    let units: Vec<u16> = value.bytes().map(u16::from).collect();
    ArchiveText::with_encoding(&units, TokenEncoding::Narrow).unwrap()
}

/// Handle 1, flags, history type 0, namespace "UI", the empty key in its
/// saved count-1 form, the source as a wide string, then handle 0.
fn base_fixture(order: ByteOrder, history: u32) -> (Vec<u8>, usize) {
    let mut writer = BitWriter::new();
    packed(&mut writer, 1);
    word(&mut writer, 0x0000_0002, order);
    writer.write_bits(history, 8);
    word(&mut writer, 3, order);
    writer.write_bytes(b"UI\0");
    word(&mut writer, 1, order);
    writer.write_bytes(b"\0");
    word(&mut writer, (-3i32) as u32, order);
    for unit in [0x48u16, 0x00e9, 0] {
        match order {
            ByteOrder::Little => writer.write_bytes(&unit.to_le_bytes()),
            ByteOrder::Big => writer.write_bytes(&unit.to_be_bytes()),
        }
    }
    packed(&mut writer, 0);
    bits(&writer)
}

fn base_value() -> BaseText {
    BaseText {
        flags: 2,
        namespace: narrow("UI"),
        key: ArchiveText::with_encoding(&[], TokenEncoding::Narrow).unwrap(),
        source: ArchiveText::with_encoding(&[0x48, 0x00e9], TokenEncoding::Wide).unwrap(),
    }
}

#[test]
fn base_history_text_round_trips_in_both_byte_orders() {
    for order in [ByteOrder::Little, ByteOrder::Big] {
        let expected = single(RepValue::Text(base_value()));
        assert_stream(&base_fixture(order, 0), &expected, &codec(order, |_| {}));
    }
}

#[test]
fn text_summaries_and_debug_carry_lengths_only() {
    let value = RepValue::Text(base_value());
    assert_eq!(value.kind(), ItemKind::Text);
    let summary = single(value.clone()).entries[0].summary();
    assert_eq!(summary.string_units, Some(3));
    let shown = format!("{value:?}");
    assert!(shown.contains("source_units"), "{shown}");
    assert!(!shown.contains("UI") && !shown.contains("72"), "{shown}");
}

#[test]
fn an_empty_source_string_is_a_zero_count() {
    let mut writer = BitWriter::new();
    packed(&mut writer, 1);
    word(&mut writer, 0, ByteOrder::Little);
    writer.write_bits(0, 8);
    for key in [b"ns\0".as_slice(), b"k\0".as_slice()] {
        word(&mut writer, key.len() as u32, ByteOrder::Little);
        writer.write_bytes(key);
    }
    word(&mut writer, 0, ByteOrder::Little);
    packed(&mut writer, 0);
    let expected = single(RepValue::Text(BaseText {
        flags: 0,
        namespace: narrow("ns"),
        key: narrow("k"),
        source: ArchiveText::with_encoding(&[], TokenEncoding::Empty).unwrap(),
    }));
    assert_stream(&bits(&writer), &expected, &plain());
}

#[test]
fn other_history_types_fail_closed() {
    // NamedFormat, StringTableEntry, Transform, TextGenerator and the none form.
    for history in [1, 4, 7, 10, 11, 12, 0xff] {
        let payload = to_payload(&base_fixture(ByteOrder::Little, history));
        let error = error_of(parse_property_stream(&payload, &LAYOUT, &plain()));
        assert!(error.contains("text history type"), "{history}: {error}");
    }
}

#[test]
fn text_strings_obey_the_string_unit_limit() {
    let tight = codec(ByteOrder::Little, |limits| limits.max_string_units = 2);
    let payload = to_payload(&base_fixture(ByteOrder::Little, 0));
    assert!(error_of(parse_property_stream(&payload, &LAYOUT, &tight)).contains("unit limit"));
    let expected = single(RepValue::Text(base_value()));
    assert!(error_of(encode_property_stream(&expected, &LAYOUT, &tight)).contains("unit limit"));
}

#[test]
fn text_values_need_a_text_command() {
    let expected = single(RepValue::Text(base_value()));
    assert!(encode_property_stream(&expected, &[RepCommand::Str], &plain()).is_err());
    let payload = to_payload(&base_fixture(ByteOrder::Little, 0));
    // The same bits under a Str layout are not a Text item.
    assert!(
        parse_property_stream(&payload, &[RepCommand::Str], &plain())
            .map(|stream| stream.entries[0].value.kind() != ItemKind::Text)
            .unwrap_or(true)
    );
}

use icarus_bevy::experimental::actor_channel::{ActorChannelCodec, ActorChannelLimits};
use icarus_bevy::experimental::archive::ByteOrder;
use icarus_bevy::experimental::control::TokenEncoding;
use icarus_bevy::experimental::control_messages::ArchiveText;
use icarus_bevy::experimental::packet::{BitReader, BitWriter, Payload};
use icarus_bevy::experimental::rep_layout::{
    PropertyStream, RepCommand, RepEntry, RepLayoutCodec, RepLayoutLimits, RepValue,
    encode_property_stream, parse_property_stream, read_property_stream,
};

// Independently authored bit fixtures for Str items (F-595): the archive
// string frame, built field by field; no captured traffic.

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
    let layout = [RepCommand::Str];
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

fn narrow(value: &str) -> ArchiveText {
    let units: Vec<u16> = value.bytes().map(u16::from).collect();
    ArchiveText::with_encoding(&units, TokenEncoding::Narrow).unwrap()
}

#[test]
fn narrow_strings_carry_a_count_bytes_and_a_terminator() {
    let mut writer = BitWriter::new();
    packed(&mut writer, 1);
    writer.write_bytes(&5u32.to_le_bytes());
    writer.write_bytes(b"Camp\0");
    packed(&mut writer, 0);
    let expected = single(RepValue::Str(narrow("Camp")));
    assert_stream(&bits(&writer), &expected, &plain());
    assert_eq!(expected.entries[0].summary().string_units, Some(5));
    assert!(!format!("{expected:?}").contains("Camp"));
}

#[test]
fn wide_strings_use_a_negative_count_in_the_caller_byte_order() {
    let units = [0x43u16, 0x00e9, 0];
    for order in [ByteOrder::Little, ByteOrder::Big] {
        let mut writer = BitWriter::new();
        packed(&mut writer, 1);
        let count = (-3i32) as u32;
        match order {
            ByteOrder::Little => writer.write_bytes(&count.to_le_bytes()),
            ByteOrder::Big => writer.write_bytes(&count.to_be_bytes()),
        }
        for unit in units {
            match order {
                ByteOrder::Little => writer.write_bytes(&unit.to_le_bytes()),
                ByteOrder::Big => writer.write_bytes(&unit.to_be_bytes()),
            }
        }
        packed(&mut writer, 0);
        let text = ArchiveText::with_encoding(&units[..2], TokenEncoding::Wide).unwrap();
        assert_stream(
            &bits(&writer),
            &single(RepValue::Str(text)),
            &codec(order, |_| {}),
        );
    }
}

#[test]
fn an_empty_string_is_a_zero_count() {
    let mut writer = BitWriter::new();
    packed(&mut writer, 1);
    writer.write_bytes(&0u32.to_le_bytes());
    packed(&mut writer, 0);
    let empty = ArchiveText::with_encoding(&[], TokenEncoding::Empty).unwrap();
    assert_stream(&bits(&writer), &single(RepValue::Str(empty)), &plain());
}

#[test]
fn string_limits_and_malformed_frames_fail_closed() {
    let mut writer = BitWriter::new();
    packed(&mut writer, 1);
    writer.write_bytes(&5u32.to_le_bytes());
    writer.write_bytes(b"Camp\0");
    packed(&mut writer, 0);
    let payload = to_payload(&bits(&writer));
    let named = single(RepValue::Str(narrow("Camp")));
    let four = codec(ByteOrder::Little, |limits| limits.max_string_units = 4);
    assert!(
        error_of(parse_property_stream(&payload, &[RepCommand::Str], &four)).contains("unit limit")
    );
    assert!(
        error_of(encode_property_stream(&named, &[RepCommand::Str], &four)).contains("unit limit")
    );
    // The name limit does not bound Str items.
    let tight_names = codec(ByteOrder::Little, |limits| limits.max_name_units = 1);
    assert!(parse_property_stream(&payload, &[RepCommand::Str], &tight_names).is_ok());

    // A missing terminator and i32::MIN fail.
    let mut writer = BitWriter::new();
    packed(&mut writer, 1);
    writer.write_bytes(&4u32.to_le_bytes());
    writer.write_bytes(b"Camp");
    packed(&mut writer, 0);
    assert!(
        parse_property_stream(&to_payload(&bits(&writer)), &[RepCommand::Str], &plain()).is_err()
    );
    let mut writer = BitWriter::new();
    packed(&mut writer, 1);
    writer.write_bytes(&0x8000_0000u32.to_le_bytes());
    packed(&mut writer, 0);
    assert!(
        error_of(parse_property_stream(
            &to_payload(&bits(&writer)),
            &[RepCommand::Str],
            &plain()
        ))
        .contains("i32::MIN")
    );
}

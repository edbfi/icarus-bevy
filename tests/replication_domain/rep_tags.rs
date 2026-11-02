use icarus_bevy::experimental::actor_channel::{ActorChannelCodec, ActorChannelLimits};
use icarus_bevy::experimental::archive::ByteOrder;
use icarus_bevy::experimental::packet::{BitReader, BitWriter, Payload};
use icarus_bevy::experimental::rep_layout::{
    GameplayTag, NetworkName, PropertyStream, RepCommand, RepEntry, RepLayoutCodec,
    RepLayoutLimits, RepValue, TagEncoding, encode_property_stream, parse_property_stream,
    read_property_stream, read_tag_net_index, validate_layout, write_tag_net_index,
};

// Independently authored bit fixtures for gameplay tags (F-591, F-592), built
// field by field with invented widths; no tag table or captured traffic.

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

fn index_bits(index: u16, first: u8, total: u8) -> (Vec<u8>, usize) {
    let mut writer = BitWriter::new();
    write_tag_net_index(&mut writer, index, first, total).unwrap();
    bits(&writer)
}

fn read_index(fixture: &(Vec<u8>, usize), first: u8, total: u8) -> anyhow::Result<u16> {
    let mut reader = BitReader::new(&fixture.0, fixture.1).unwrap();
    let index = read_tag_net_index(&mut reader, first, total)?;
    assert!(reader.at_end());
    Ok(index)
}

#[test]
fn net_index_splits_after_the_first_segment() {
    // First segment 4 of 11: small indices take 5 bits, large ones 12.
    let small = index_bits(9, 4, 11);
    assert_eq!(small.1, 5);
    let mut writer = BitWriter::new();
    writer.write_bits(9, 4);
    writer.write_bit(false);
    assert_eq!(small, bits(&writer));
    assert_eq!(read_index(&small, 4, 11).unwrap(), 9);

    let large = index_bits(0x5a3, 4, 11);
    assert_eq!(large.1, 12);
    let mut writer = BitWriter::new();
    writer.write_bits(0x3, 4);
    writer.write_bit(true);
    writer.write_bits(0x5a, 7);
    assert_eq!(large, bits(&writer));
    assert_eq!(read_index(&large, 4, 11).unwrap(), 0x5a3);

    // The boundary: 15 fits the segment, 16 does not.
    assert_eq!(index_bits(15, 4, 11).1, 5);
    assert_eq!(index_bits(16, 4, 11).1, 12);
    for index in [0u16, 1, 15, 16, 17, 0x7ff] {
        assert_eq!(read_index(&index_bits(index, 4, 11), 4, 11).unwrap(), index);
    }
}

#[test]
fn net_index_without_a_usable_segment_is_raw() {
    for first in [0, 11, 12, 16] {
        let fixture = index_bits(0x5a3, first, 11);
        assert_eq!(fixture.1, 11, "first {first}");
        let mut writer = BitWriter::new();
        writer.write_bits(0x5a3, 11);
        assert_eq!(fixture, bits(&writer));
        assert_eq!(read_index(&fixture, first, 11).unwrap(), 0x5a3);
    }
    // A full 16-bit index.
    let fixture = index_bits(u16::MAX, 0, 16);
    assert_eq!(read_index(&fixture, 0, 16).unwrap(), u16::MAX);
}

#[test]
fn net_index_bounds_fail_closed() {
    let mut writer = BitWriter::new();
    assert!(error_of(write_tag_net_index(&mut writer, 0x800, 4, 11).map(|_| ())).contains("width"));
    for total in [0, 17] {
        assert!(
            error_of(write_tag_net_index(&mut writer, 0, 0, total).map(|_| ())).contains("1 to 16")
        );
    }
    // A set continuation bit with zero high bits is not canonical.
    let mut writer = BitWriter::new();
    writer.write_bits(0x3, 4);
    writer.write_bit(true);
    writer.write_bits(0, 7);
    assert!(error_of(read_index(&bits(&writer), 4, 11)).contains("non-canonical"));
    // Truncation inside the high part fails.
    let large = index_bits(0x5a3, 4, 11);
    for len in 0..large.1 {
        let mut reader = BitReader::new(&large.0, len).unwrap();
        assert!(
            read_tag_net_index(&mut reader, 4, 11).is_err(),
            "prefix {len}"
        );
    }
}

const INDEX: TagEncoding = TagEncoding::NetIndex {
    first_segment: 4,
    total_bits: 11,
};

#[test]
fn single_tags_round_trip_in_both_encodings() {
    let mut writer = BitWriter::new();
    packed(&mut writer, 1);
    writer.write_bits(0x3, 4);
    writer.write_bit(true);
    writer.write_bits(0x5a, 7);
    packed(&mut writer, 0);
    assert_stream(
        &bits(&writer),
        &[RepCommand::GameplayTag(INDEX)],
        &single(RepValue::GameplayTag(GameplayTag::NetIndex(0x5a3))),
    );
    // Name mode: a hardcoded network name at net version 13 (F-575).
    let mut writer = BitWriter::new();
    packed(&mut writer, 1);
    writer.write_bit(true);
    packed(&mut writer, 255);
    packed(&mut writer, 0);
    assert_stream(
        &bits(&writer),
        &[RepCommand::GameplayTag(TagEncoding::Name)],
        &single(RepValue::GameplayTag(GameplayTag::Name(
            NetworkName::Hardcoded(255),
        ))),
    );
    // A tag in the other encoding fails on write.
    assert!(
        error_of(encode_property_stream(
            &single(RepValue::GameplayTag(GameplayTag::NetIndex(1))),
            &[RepCommand::GameplayTag(TagEncoding::Name)],
            &plain(),
        ))
        .contains("encoding")
    );
}

#[test]
fn containers_carry_an_empty_bit_and_a_counted_tag_list() {
    let layout = [RepCommand::GameplayTagContainer {
        encoding: INDEX,
        count_bits: 3,
    }];
    // Empty: the bit alone.
    let mut writer = BitWriter::new();
    packed(&mut writer, 1);
    writer.write_bit(true);
    packed(&mut writer, 0);
    let empty = single(RepValue::GameplayTagContainer(None));
    assert_stream(&bits(&writer), &layout, &empty);
    assert_eq!(empty.entries[0].summary().tag_count, None);

    // Two tags: a clear bit, a 3-bit count, then the indices.
    let mut writer = BitWriter::new();
    packed(&mut writer, 1);
    writer.write_bit(false);
    writer.write_bits(2, 3);
    writer.write_bits(9, 4);
    writer.write_bit(false);
    writer.write_bits(0x3, 4);
    writer.write_bit(true);
    writer.write_bits(0x5a, 7);
    packed(&mut writer, 0);
    let two = single(RepValue::GameplayTagContainer(Some(vec![
        GameplayTag::NetIndex(9),
        GameplayTag::NetIndex(0x5a3),
    ])));
    assert_stream(&bits(&writer), &layout, &two);
    assert_eq!(two.entries[0].summary().tag_count, Some(2));

    // A clear bit with a zero count is preserved as read.
    let mut writer = BitWriter::new();
    packed(&mut writer, 1);
    writer.write_bit(false);
    writer.write_bits(0, 3);
    packed(&mut writer, 0);
    assert_stream(
        &bits(&writer),
        &layout,
        &single(RepValue::GameplayTagContainer(Some(Vec::new()))),
    );

    // Eight tags do not fit a 3-bit count.
    let eight = single(RepValue::GameplayTagContainer(Some(vec![
        GameplayTag::NetIndex(1);
        8
    ])));
    assert!(error_of(encode_property_stream(&eight, &layout, &plain())).contains("count"));
}

#[test]
fn tag_layouts_validate_their_widths() {
    let limits = RepLayoutLimits::default();
    for (command, message) in [
        (
            RepCommand::GameplayTag(TagEncoding::NetIndex {
                first_segment: 1,
                total_bits: 0,
            }),
            "1 to 16",
        ),
        (
            RepCommand::GameplayTag(TagEncoding::NetIndex {
                first_segment: 1,
                total_bits: 17,
            }),
            "1 to 16",
        ),
        (
            RepCommand::GameplayTagContainer {
                encoding: INDEX,
                count_bits: 0,
            },
            "1 to 8",
        ),
        (
            RepCommand::GameplayTagContainer {
                encoding: INDEX,
                count_bits: 9,
            },
            "1 to 8",
        ),
    ] {
        assert!(
            error_of(validate_layout(&[command], &limits)).contains(message),
            "{message}"
        );
    }
    assert!(
        validate_layout(
            &[
                RepCommand::GameplayTag(TagEncoding::Name),
                RepCommand::GameplayTagContainer {
                    encoding: INDEX,
                    count_bits: 8,
                },
            ],
            &limits
        )
        .is_ok()
    );
    // Debug output never shows an index.
    let tag = GameplayTag::NetIndex(0x5a3);
    assert!(!format!("{tag:?}").contains("1443"));
}

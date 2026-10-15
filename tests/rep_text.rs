use icarus_bevy::experimental::actor_channel::{ActorChannelCodec, ActorChannelLimits};
use icarus_bevy::experimental::archive::ByteOrder;
use icarus_bevy::experimental::control::TokenEncoding;
use icarus_bevy::experimental::control_messages::ArchiveText;
use icarus_bevy::experimental::packet::{BitReader, BitWriter, Payload};
use icarus_bevy::experimental::rep_layout::{
    ItemKind, NetworkName, PropertyStream, RepCommand, RepEntry, RepLayoutCodec, RepLayoutLimits,
    RepValue, TextValue, encode_property_stream, parse_property_stream, read_property_stream,
};
use icarus_bevy::experimental::rep_text::{
    ArgumentData, ArgumentValue, DateTimeKind, FormatArgument, NumberKind, NumberOptions,
    TextHistory,
};

// Independently authored byte fixtures for Text items (F-624, F-627): each
// field written by hand in archive order; no captured traffic.

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

/// Field-by-field fixture writer in one byte order.
struct Fixture {
    writer: BitWriter,
    order: ByteOrder,
}

impl Fixture {
    fn new(order: ByteOrder) -> Self {
        let mut fixture = Self {
            writer: BitWriter::new(),
            order,
        };
        fixture.packed(1);
        fixture
    }

    /// Hand-written packed integer: 7-bit groups, bit 0 continues.
    fn packed(&mut self, mut value: u32) {
        loop {
            let group = value & 0x7f;
            value >>= 7;
            self.writer
                .write_bits(group << 1 | u32::from(value != 0), 8);
            if value == 0 {
                break;
            }
        }
    }

    fn byte(&mut self, value: u8) -> &mut Self {
        self.writer.write_bytes(&[value]);
        self
    }

    fn word(&mut self, value: u32) -> &mut Self {
        match self.order {
            ByteOrder::Little => self.writer.write_bytes(&value.to_le_bytes()),
            ByteOrder::Big => self.writer.write_bytes(&value.to_be_bytes()),
        }
        self
    }

    fn long(&mut self, value: u64) -> &mut Self {
        match self.order {
            ByteOrder::Little => self.writer.write_bytes(&value.to_le_bytes()),
            ByteOrder::Big => self.writer.write_bytes(&value.to_be_bytes()),
        }
        self
    }

    /// Narrow archive string with its terminator; "" is a zero count.
    fn string(&mut self, value: &str) -> &mut Self {
        if value.is_empty() {
            return self.word(0);
        }
        self.word(value.len() as u32 + 1);
        self.writer.write_bytes(value.as_bytes());
        self.byte(0)
    }

    fn base(&mut self, flags: u32, namespace: &str, key: &str, source: &str) -> &mut Self {
        self.word(flags)
            .byte(0)
            .string(namespace)
            .string(key)
            .string(source)
    }

    fn finish(mut self) -> (Vec<u8>, usize) {
        self.packed(0);
        (self.writer.as_bytes().to_vec(), self.writer.bit_len())
    }
}

fn to_payload(fixture: &(Vec<u8>, usize)) -> Payload {
    let mut writer = BitWriter::new();
    let mut reader = BitReader::new(&fixture.0, fixture.1).unwrap();
    for _ in 0..fixture.1 {
        writer.write_bit(reader.read_bit());
    }
    Payload::from_writer(writer)
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

fn assert_text(fixture: &(Vec<u8>, usize), expected: TextValue, codec: &RepLayoutCodec) {
    let expected = single(RepValue::Text(expected));
    let payload = to_payload(fixture);
    assert_eq!(
        parse_property_stream(&payload, &LAYOUT, codec).unwrap(),
        expected
    );
    assert_eq!(
        encode_property_stream(&expected, &LAYOUT, codec).unwrap(),
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
    if value.is_empty() {
        return ArchiveText::with_encoding(&[], TokenEncoding::Empty).unwrap();
    }
    let units: Vec<u16> = value.bytes().map(u16::from).collect();
    ArchiveText::with_encoding(&units, TokenEncoding::Narrow).unwrap()
}

fn base(flags: u32, namespace: &str, key: &str, source: &str) -> TextValue {
    TextValue {
        flags,
        history: TextHistory::Base {
            namespace: narrow(namespace),
            key: narrow(key),
            source: narrow(source),
        },
    }
}

#[test]
fn base_history_round_trips_with_an_empty_key_in_its_saved_form() {
    for order in [ByteOrder::Little, ByteOrder::Big] {
        let mut fixture = Fixture::new(order);
        fixture.word(2).byte(0).string("UI");
        // An empty key is saved as count 1 and a terminator (F-624).
        fixture.word(1).byte(0);
        fixture.word((-3i32) as u32);
        for unit in [0x48u16, 0x00e9, 0] {
            match order {
                ByteOrder::Little => fixture.writer.write_bytes(&unit.to_le_bytes()),
                ByteOrder::Big => fixture.writer.write_bytes(&unit.to_be_bytes()),
            }
        }
        let expected = TextValue {
            flags: 2,
            history: TextHistory::Base {
                namespace: narrow("UI"),
                key: ArchiveText::with_encoding(&[], TokenEncoding::Narrow).unwrap(),
                source: ArchiveText::with_encoding(&[0x48, 0x00e9], TokenEncoding::Wide).unwrap(),
            },
        };
        assert_text(&fixture.finish(), expected, &codec(order, |_| {}));
    }
}

#[test]
fn none_history_carries_an_optional_culture_invariant_string() {
    let mut with = Fixture::new(ByteOrder::Little);
    with.word(0).byte(0xff).word(1).string("42");
    assert_text(
        &with.finish(),
        TextValue {
            flags: 0,
            history: TextHistory::None {
                culture_invariant: Some(narrow("42")),
            },
        },
        &plain(),
    );
    let mut without = Fixture::new(ByteOrder::Little);
    without.word(8).byte(0xff).word(0);
    assert_text(
        &without.finish(),
        TextValue {
            flags: 8,
            history: TextHistory::None {
                culture_invariant: None,
            },
        },
        &plain(),
    );
}

#[test]
fn format_histories_nest_a_format_text_and_their_arguments() {
    for order in [ByteOrder::Little, ByteOrder::Big] {
        let mut fixture = Fixture::new(order);
        fixture.word(0).byte(2).base(0, "ns", "fmt", "{0} of {1}");
        fixture.word(4);
        fixture.byte(0).long((-5i64) as u64);
        fixture.byte(1).long(7);
        fixture.byte(2).word(1.5f32.to_bits());
        fixture.byte(4).base(0, "ns", "item", "Bow");
        let expected = TextValue {
            flags: 0,
            history: TextHistory::OrderedFormat {
                format: Box::new(base(0, "ns", "fmt", "{0} of {1}")),
                arguments: vec![
                    FormatArgument::Int(-5),
                    FormatArgument::UInt(7),
                    FormatArgument::Float(1.5f32.to_bits()),
                    FormatArgument::Text(Box::new(base(0, "ns", "item", "Bow"))),
                ],
            },
        };
        assert_text(&fixture.finish(), expected, &codec(order, |_| {}));
    }

    let mut named = Fixture::new(ByteOrder::Little);
    named.word(0).byte(1).base(0, "", "", "{Count}");
    named.word(2).string("Count").byte(3).long(2.0f64.to_bits());
    named.string("Who").byte(5);
    let expected = TextValue {
        flags: 0,
        history: TextHistory::NamedFormat {
            format: Box::new(base(0, "", "", "{Count}")),
            arguments: vec![
                (narrow("Count"), FormatArgument::Double(2.0f64.to_bits())),
                (narrow("Who"), FormatArgument::Other(5)),
            ],
        },
    };
    assert_text(&named.finish(), expected, &plain());

    let mut data = Fixture::new(ByteOrder::Little);
    data.word(0).byte(3).base(0, "a", "b", "{X}");
    data.word(4);
    data.string("X").byte(0).word((-1i32) as u32);
    data.string("Y").byte(2).word(0.25f32.to_bits());
    data.string("Z").byte(5).byte(1);
    data.string("W").byte(1);
    let expected = TextValue {
        flags: 0,
        history: TextHistory::ArgumentDataFormat {
            format: Box::new(base(0, "a", "b", "{X}")),
            arguments: vec![
                ArgumentData {
                    name: narrow("X"),
                    value: ArgumentValue::Int(-1),
                },
                ArgumentData {
                    name: narrow("Y"),
                    value: ArgumentValue::Float(0.25f32.to_bits()),
                },
                ArgumentData {
                    name: narrow("Z"),
                    value: ArgumentValue::Gender(1),
                },
                ArgumentData {
                    name: narrow("W"),
                    value: ArgumentValue::Other(1),
                },
            ],
        },
    };
    assert_text(&data.finish(), expected, &plain());
}

#[test]
fn number_histories_carry_options_and_a_culture() {
    let mut fixture = Fixture::new(ByteOrder::Little);
    fixture.word(0).byte(6).string("USD");
    fixture.byte(0).long(1250);
    fixture.word(1).word(0).word(1).byte(2);
    for bound in [1u32, 40, 0, 2] {
        fixture.word(bound);
    }
    fixture.string("en");
    let expected = TextValue {
        flags: 0,
        history: TextHistory::Number {
            kind: NumberKind::Currency {
                code: narrow("USD"),
            },
            value: FormatArgument::Int(1250),
            options: Some(NumberOptions {
                always_sign: false,
                use_grouping: true,
                rounding_mode: 2,
                minimum_integral_digits: 1,
                maximum_integral_digits: 40,
                minimum_fractional_digits: 0,
                maximum_fractional_digits: 2,
            }),
            culture: narrow("en"),
        },
    };
    assert_text(&fixture.finish(), expected, &plain());

    let mut percent = Fixture::new(ByteOrder::Big);
    percent
        .word(0)
        .byte(5)
        .byte(2)
        .word(0.5f32.to_bits())
        .word(0)
        .string("");
    let expected = TextValue {
        flags: 0,
        history: TextHistory::Number {
            kind: NumberKind::Percent,
            value: FormatArgument::Float(0.5f32.to_bits()),
            options: None,
            culture: narrow(""),
        },
    };
    assert_text(&percent.finish(), expected, &codec(ByteOrder::Big, |_| {}));
}

#[test]
fn date_time_histories_carry_ticks_styles_zone_and_culture() {
    let cases = [
        (7u8, DateTimeKind::Date { date_style: 2 }, vec![2u8]),
        (8, DateTimeKind::Time { time_style: -1 }, vec![0xff]),
        (
            9,
            DateTimeKind::DateTime {
                date_style: 1,
                time_style: 3,
            },
            vec![1, 3],
        ),
    ];
    for (history, kind, styles) in cases {
        let mut fixture = Fixture::new(ByteOrder::Little);
        fixture.word(0).byte(history).long(638_000_000_000_000_000);
        for style in &styles {
            fixture.byte(*style);
        }
        fixture.string("UTC").string("en-GB");
        let expected = TextValue {
            flags: 0,
            history: TextHistory::DateTime {
                kind,
                ticks: 638_000_000_000_000_000,
                zone: narrow("UTC"),
                culture: narrow("en-GB"),
            },
        };
        assert_text(&fixture.finish(), expected, &plain());
    }
}

#[test]
fn transform_nests_a_whole_text_and_a_byte() {
    let mut fixture = Fixture::new(ByteOrder::Little);
    fixture.word(0).byte(10).base(4, "n", "k", "camp").byte(1);
    let expected = TextValue {
        flags: 0,
        history: TextHistory::Transform {
            text: Box::new(base(4, "n", "k", "camp")),
            transform: 1,
        },
    };
    assert_text(&fixture.finish(), expected, &plain());
}

#[test]
fn unread_history_types_and_malformed_fields_fail_closed() {
    for history in [13u8, 0x80, 0xfe] {
        let mut fixture = Fixture::new(ByteOrder::Little);
        fixture.word(0).byte(history).string("x").string("y");
        let error = error_of(parse_property_stream(
            &to_payload(&fixture.finish()),
            &LAYOUT,
            &plain(),
        ));
        assert!(error.contains("text history type"), "{history}: {error}");
    }
    // A bool word above 1 is an archive error in the engine.
    let mut bad_bool = Fixture::new(ByteOrder::Little);
    bad_bool.word(0).byte(0xff).word(2);
    assert!(
        error_of(parse_property_stream(
            &to_payload(&bad_bool.finish()),
            &LAYOUT,
            &plain()
        ))
        .contains("bool")
    );
    // A negative argument count.
    let mut negative = Fixture::new(ByteOrder::Little);
    negative.word(0).byte(2).base(0, "", "", "f").word(u32::MAX);
    assert!(
        error_of(parse_property_stream(
            &to_payload(&negative.finish()),
            &LAYOUT,
            &plain()
        ))
        .contains("count")
    );
}

#[test]
fn nesting_and_counts_obey_the_codec_limits() {
    let mut nested = base(0, "", "", "x");
    for _ in 0..3 {
        nested = TextValue {
            flags: 0,
            history: TextHistory::Transform {
                text: Box::new(nested),
                transform: 0,
            },
        };
    }
    let value = single(RepValue::Text(nested));
    let shallow = codec(ByteOrder::Little, |limits| limits.max_depth = 2);
    assert!(error_of(encode_property_stream(&value, &LAYOUT, &shallow)).contains("depth"));
    let payload = encode_property_stream(&value, &LAYOUT, &plain()).unwrap();
    assert!(error_of(parse_property_stream(&payload, &LAYOUT, &shallow)).contains("depth"));

    let many = single(RepValue::Text(TextValue {
        flags: 0,
        history: TextHistory::OrderedFormat {
            format: Box::new(base(0, "", "", "f")),
            arguments: vec![FormatArgument::Int(1); 3],
        },
    }));
    let tight = codec(ByteOrder::Little, |limits| limits.max_array_elements = 2);
    assert!(error_of(encode_property_stream(&many, &LAYOUT, &tight)).contains("count"));
    let payload = encode_property_stream(&many, &LAYOUT, &plain()).unwrap();
    assert!(error_of(parse_property_stream(&payload, &LAYOUT, &tight)).contains("count"));
}

#[test]
fn payloadless_variants_cannot_claim_payload_types() {
    let value = single(RepValue::Text(TextValue {
        flags: 0,
        history: TextHistory::OrderedFormat {
            format: Box::new(base(0, "", "", "f")),
            arguments: vec![FormatArgument::Other(2)],
        },
    }));
    assert!(encode_property_stream(&value, &LAYOUT, &plain()).is_err());
}

#[test]
fn text_summaries_and_debug_carry_no_content() {
    let value = RepValue::Text(base(2, "UI", "Key", "Hello"));
    assert_eq!(value.kind(), ItemKind::Text);
    let summary = single(value.clone()).entries[0].summary();
    assert_eq!(summary.string_units, Some(6));
    let shown = format!("{value:?}");
    assert!(shown.contains("history: 0"), "{shown}");
    assert!(!shown.contains("Hello") && !shown.contains("UI"), "{shown}");
}

#[test]
fn text_values_need_a_text_command() {
    let value = single(RepValue::Text(base(0, "", "", "x")));
    assert!(encode_property_stream(&value, &[RepCommand::Str], &plain()).is_err());
}

#[test]
fn string_table_entries_send_a_network_name_then_a_key() {
    // Hardcoded form: the bit, then the packed index (engine net version 13).
    let mut hardcoded = Fixture::new(ByteOrder::Little);
    hardcoded.word(0).byte(11);
    hardcoded.writer.write_bit(true);
    hardcoded.packed(300);
    hardcoded.string("Greeting");
    assert_text(
        &hardcoded.finish(),
        TextValue {
            flags: 0,
            history: TextHistory::StringTableEntry {
                table: NetworkName::Hardcoded(300),
                key: narrow("Greeting"),
            },
        },
        &plain(),
    );
    // String form: the bit clear, an archive string and a 4-byte number.
    let mut named = Fixture::new(ByteOrder::Big);
    named.word(1).byte(11);
    named.writer.write_bit(false);
    named.string("UI_Table").word(0).string("Key");
    assert_text(
        &named.finish(),
        TextValue {
            flags: 1,
            history: TextHistory::StringTableEntry {
                table: NetworkName::String {
                    text: narrow("UI_Table"),
                    number: 0,
                },
                key: narrow("Key"),
            },
        },
        &codec(ByteOrder::Big, |_| {}),
    );
}

#[test]
fn generator_payloads_follow_unless_the_type_name_is_none() {
    let mut with = Fixture::new(ByteOrder::Little);
    with.word(0).byte(12);
    with.writer.write_bit(true);
    with.packed(300);
    with.word(3).byte(7).byte(8).byte(9);
    assert_text(
        &with.finish(),
        TextValue {
            flags: 0,
            history: TextHistory::Generator {
                generator_type: NetworkName::Hardcoded(300),
                payload: Some(vec![7, 8, 9]),
            },
        },
        &plain(),
    );
    let mut none = Fixture::new(ByteOrder::Little);
    none.word(0).byte(12);
    none.writer.write_bit(true);
    none.packed(0);
    assert_text(
        &none.finish(),
        TextValue {
            flags: 0,
            history: TextHistory::Generator {
                generator_type: NetworkName::Hardcoded(0),
                payload: None,
            },
        },
        &plain(),
    );
    let mut empty = Fixture::new(ByteOrder::Big);
    empty.word(0).byte(12);
    empty.writer.write_bit(false);
    empty.string("Generator").word(0).word(0);
    assert_text(
        &empty.finish(),
        TextValue {
            flags: 0,
            history: TextHistory::Generator {
                generator_type: NetworkName::String {
                    text: narrow("Generator"),
                    number: 0,
                },
                payload: Some(Vec::new()),
            },
        },
        &codec(ByteOrder::Big, |_| {}),
    );
}

#[test]
fn malformed_generators_fail_closed() {
    // A string-form None has unread name-table resolution.
    let mut spelled = Fixture::new(ByteOrder::Little);
    spelled.word(0).byte(12);
    spelled.writer.write_bit(false);
    spelled.string("NONE").word(0);
    assert!(
        error_of(parse_property_stream(
            &to_payload(&spelled.finish()),
            &LAYOUT,
            &plain()
        ))
        .contains("None")
    );
    // Negative and oversized payload counts.
    for count in [u32::MAX, 9] {
        let mut fixture = Fixture::new(ByteOrder::Little);
        fixture.word(0).byte(12);
        fixture.writer.write_bit(true);
        fixture.packed(5);
        fixture.word(count).byte(1);
        let tight = codec(ByteOrder::Little, |limits| limits.max_opaque_bits = 64);
        assert!(
            error_of(parse_property_stream(
                &to_payload(&fixture.finish()),
                &LAYOUT,
                &tight
            ))
            .contains("payload"),
            "{count}"
        );
    }
    // A payload must accompany exactly the non-None type Names.
    for (generator_type, payload) in [
        (NetworkName::Hardcoded(0), Some(vec![1])),
        (NetworkName::Hardcoded(5), None),
    ] {
        let value = single(RepValue::Text(TextValue {
            flags: 0,
            history: TextHistory::Generator {
                generator_type,
                payload,
            },
        }));
        assert!(encode_property_stream(&value, &LAYOUT, &plain()).is_err());
    }
}

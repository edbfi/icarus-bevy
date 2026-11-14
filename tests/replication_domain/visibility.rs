use icarus_bevy::experimental::archive::MAX_STRING_UNITS;
use icarus_bevy::experimental::control::TokenEncoding;
use icarus_bevy::experimental::control_messages::ArchiveText;
use icarus_bevy::experimental::packet::{BitReader, BitWriter, Payload};
use icarus_bevy::experimental::release::ReleaseProfile;
use icarus_bevy::experimental::rep_layout::NetworkName;
use icarus_bevy::experimental::visibility::{
    DecodedVisibilityParameter, VisibilityArchiveProfile, VisibilityDecodeProfile,
    VisibilityRpc82Layout, parse_visibility, parse_visibility_rpc82, read_visibility,
    stage_visibility,
};

// Independently authored custom-struct fixtures only. These have no outer RPC
// presence bit and contain no captured names, traffic or original values.
fn profile() -> VisibilityDecodeProfile {
    VisibilityDecodeProfile::qualify(
        &ReleaseProfile::fixture("3.0.34"),
        VisibilityArchiveProfile::NormalLittleEndianPackageMapCustom,
    )
    .unwrap()
}

fn custom(reuse: bool, visible: bool, names: impl FnOnce(&mut BitWriter)) -> Payload {
    let mut writer = BitWriter::new();
    writer.write_bit(reuse);
    writer.write_bit(visible);
    names(&mut writer);
    Payload::from_writer(writer)
}

fn hardcoded(writer: &mut BitWriter, index: u32) {
    writer.write_bit(true);
    writer.write_int_packed(index);
}

fn string(writer: &mut BitWriter, count: i32, bytes: &[u8], number: u32) {
    writer.write_bit(false);
    writer.write_bytes(&count.to_le_bytes());
    writer.write_bytes(bytes);
    writer.write_bytes(&number.to_le_bytes());
}

fn string_payload(count: i32, bytes: &[u8], number: u32) -> Payload {
    custom(true, false, |writer| string(writer, count, bytes, number))
}

fn prefix(payload: &Payload, bits: usize) -> Payload {
    let mut reader = payload.reader();
    let mut writer = BitWriter::new();
    for _ in 0..bits {
        writer.write_bit(reader.read_bit());
    }
    assert!(!reader.is_error());
    Payload::from_writer(writer)
}

fn with_tail(payload: &Payload, bit: bool) -> Payload {
    let mut writer = BitWriter::new();
    let mut reader = payload.reader();
    for _ in 0..payload.bit_len() {
        writer.write_bit(reader.read_bit());
    }
    writer.write_bit(bit);
    Payload::from_writer(writer)
}

#[test]
fn all_reuse_visibility_combinations_preserve_custom_order_and_length() {
    for reuse in [false, true] {
        for visible in [false, true] {
            let payload = custom(reuse, visible, |writer| {
                hardcoded(writer, 0);
                if !reuse {
                    hardcoded(writer, 702);
                }
            });
            let value = parse_visibility(&payload, &profile()).unwrap();
            assert_eq!(value.reuse_package_name, reuse);
            assert_eq!(value.visible, visible);
            assert_eq!(value.package_name, NetworkName::Hardcoded(0));
            assert_eq!(
                value.file_name,
                NetworkName::Hardcoded(if reuse { 0 } else { 702 })
            );
            assert_eq!(value.payload_bits, if reuse { 11 } else { 28 });
            assert_eq!(value.payload_bits, payload.bit_len());
        }
    }
}

#[test]
fn literal_version17_packed_index_boundaries() {
    // Literal continuation-in-bit-0 groups, independent of the packed writer.
    for (bytes, expected) in [
        (&[0x00][..], Some(0)),
        (&[0x7d, 0x0a][..], Some(702)),
        (&[0x7f, 0x0a][..], None),
    ] {
        let payload = custom(true, false, |writer| {
            writer.write_bit(true);
            writer.write_bytes(bytes);
        });
        match expected {
            Some(index) => assert_eq!(
                parse_visibility(&payload, &profile()).unwrap().package_name,
                NetworkName::Hardcoded(index)
            ),
            None => assert!(parse_visibility(&payload, &profile()).is_err()),
        }
    }
}

#[test]
fn packed_truncation_noncanonical_overflow_and_fifth_continuation_fail_closed() {
    for bytes in [
        &[][..],
        &[0x01][..],
        &[0x01, 0x00][..],       // overlong zero
        &[0x7d, 0x0b, 0x00][..], // overlong 702
        &[0x01, 0x01, 0x01, 0x01][..],
        &[0x01, 0x01, 0x01, 0x01, 0x01][..], // fifth continuation
        &[0x01, 0x01, 0x01, 0x01, 0x20][..], // high bits overflow u32
        &[0x01, 0x01, 0x01, 0x01, 0x02][..], // canonical but index too large
    ] {
        let payload = custom(true, false, |writer| {
            writer.write_bit(true);
            writer.write_bytes(bytes);
        });
        assert!(parse_visibility(&payload, &profile()).is_err());
    }
}

#[test]
fn zero_and_terminator_only_strings_keep_distinct_encodings_and_number_bits() {
    for (count, bytes, encoding) in [
        (0, &[][..], TokenEncoding::Empty),
        (1, &[0][..], TokenEncoding::Narrow),
        (-1, &[0, 0][..], TokenEncoding::Wide),
    ] {
        for number in [0, 1, 0x8000_0000, u32::MAX] {
            let payload = string_payload(count, bytes, number);
            let value = parse_visibility(&payload, &profile()).unwrap();
            let expected = NetworkName::String {
                text: ArchiveText::with_encoding(&[], encoding).unwrap(),
                number,
            };
            assert_eq!(value.package_name, expected);
            assert_eq!(value.file_name, expected);
            assert_eq!(value.payload_bits, 67 + bytes.len() * 8);
        }
    }
}

#[test]
fn ascii_and_raw_utf16_names_preserve_encoding_units_and_independent_numbers() {
    let payload = custom(false, true, |writer| {
        string(writer, 7, b"Unit_7\0", 0x89ab_cdef);
        // Includes a surrogate pair and an unpaired surrogate. Raw u16 units
        // are supported; interpreting them as Unicode scalar text is not done.
        string(
            writer,
            -6,
            &[
                0x51, 0x00, 0xe9, 0x00, 0x3d, 0xd8, 0x00, 0xde, 0x00, 0xd8, 0, 0,
            ],
            0xfedc_ba98,
        );
    });
    let value = parse_visibility(&payload, &profile()).unwrap();
    assert_eq!(
        value.package_name,
        NetworkName::String {
            text: ArchiveText::with_encoding(
                &[0x55, 0x6e, 0x69, 0x74, 0x5f, 0x37],
                TokenEncoding::Narrow
            )
            .unwrap(),
            number: 0x89ab_cdef,
        }
    );
    assert_eq!(
        value.file_name,
        NetworkName::String {
            text: ArchiveText::with_encoding(
                &[0x51, 0xe9, 0xd83d, 0xde00, 0xd800],
                TokenEncoding::Wide
            )
            .unwrap(),
            number: 0xfedc_ba98,
        }
    );
}

#[test]
fn authored_string_unit_limit_includes_terminator_for_both_widths() {
    for wide in [false, true] {
        let mut bytes = if wide {
            [0x41, 0].repeat(MAX_STRING_UNITS)
        } else {
            vec![0x41; MAX_STRING_UNITS]
        };
        let terminator_bytes = if wide { 2 } else { 1 };
        let terminator_start = bytes.len() - terminator_bytes;
        bytes[terminator_start..].fill(0);
        let count = MAX_STRING_UNITS as i32 * if wide { -1 } else { 1 };
        let payload = string_payload(count, &bytes, 5);
        let value = parse_visibility(&payload, &profile()).unwrap();
        let NetworkName::String { text, number } = value.package_name else {
            panic!("expected string representation");
        };
        assert_eq!(text.units().len(), MAX_STRING_UNITS - 1);
        assert_eq!(text.declared_units(), MAX_STRING_UNITS);
        assert_eq!(number, 5);
    }
}

#[test]
fn impossible_or_over_limit_string_counts_are_rejected() {
    for count in [
        i32::MIN,
        i32::MIN + 1,
        i32::MAX,
        MAX_STRING_UNITS as i32 + 1,
        -(MAX_STRING_UNITS as i32 + 1),
    ] {
        assert!(parse_visibility(&string_payload(count, &[], 0), &profile()).is_err());
    }
    // At-limit declarations still need their actual payload, not only a count.
    for count in [MAX_STRING_UNITS as i32, -(MAX_STRING_UNITS as i32)] {
        assert!(parse_visibility(&string_payload(count, &[], 0), &profile()).is_err());
    }
}

#[test]
fn strict_string_subset_rejects_normalizations_and_missing_terminators() {
    for (count, bytes) in [
        (1, &[0x41][..]),
        (-1, &[0x41, 0][..]),
        (2, &[0x80, 0][..]),
        (3, &[0x41, 0, 0][..]),
        (-3, &[0x41, 0, 0, 0, 0, 0][..]),
        (-2, &[0xff, 0xff, 0, 0][..]),
        (-2, &[0x41, 0, 1, 0][..]),
    ] {
        assert!(parse_visibility(&string_payload(count, bytes, 0), &profile()).is_err());
    }
}

#[test]
fn every_truncated_prefix_rejects_including_string_count_payload_and_number() {
    let payload = custom(false, true, |writer| {
        string(writer, 4, b"Lab\0", 0x1234_5678);
        string(writer, -3, &[0x41, 0, 0xe9, 0, 0, 0], 0x8765_4321);
    });
    assert!(parse_visibility(&payload, &profile()).is_ok());
    for bits in 0..payload.bit_len() {
        assert!(
            parse_visibility(&prefix(&payload, bits), &profile()).is_err(),
            "accepted truncated prefix of {bits} bits"
        );
    }
}

#[test]
fn number_requires_all_32_bits_even_for_empty_strings() {
    let payload = string_payload(0, &[], u32::MAX);
    for number_bits in 0..32 {
        assert!(parse_visibility(&prefix(&payload, 35 + number_bits), &profile()).is_err());
    }
}

#[test]
fn scalars_and_strings_load_at_every_unaligned_cursor_without_padding() {
    let payload = custom(false, true, |writer| {
        string(writer, 3, b"Ab\0", 0xa1b2_c3d4);
        hardcoded(writer, 702);
    });
    let expected = parse_visibility(&payload, &profile()).unwrap();
    for start in 0..8 {
        let mut writer = BitWriter::new();
        writer.write_bits(0x55, start);
        let mut source = payload.reader();
        for _ in 0..payload.bit_len() {
            writer.write_bit(source.read_bit());
        }
        let mut reader = BitReader::new(writer.as_bytes(), writer.bit_len()).unwrap();
        reader.read_bits(start);
        assert_eq!(read_visibility(&mut reader, &profile()).unwrap(), expected);
        assert_eq!(reader.position(), writer.bit_len());
        assert!(!reader.is_error());
    }
}

#[test]
fn exact_consumption_rejects_one_trailing_bit_and_byte_rounded_lengths() {
    let payload = custom(true, true, |writer| hardcoded(writer, 0));
    assert_eq!(payload.bit_len(), 11);
    for tail in [false, true] {
        assert!(parse_visibility(&with_tail(&payload, tail), &profile()).is_err());
    }
    let mut reader = BitReader::new(payload.bytes(), payload.bytes().len() * 8).unwrap();
    assert!(read_visibility(&mut reader, &profile()).is_err());
}

#[test]
fn malformed_values_never_replace_empty_or_existing_staging() {
    let valid = string_payload(6, b"Stage\0", 0xf123_4567);
    let previous = parse_visibility(&valid, &profile()).unwrap();
    let invalid_file = custom(false, true, |writer| {
        string(writer, 6, b"First\0", 7);
        hardcoded(writer, 703);
    });
    let malformed = [
        prefix(&valid, 0),
        prefix(&valid, valid.bit_len() - 1),
        with_tail(&valid, false),
        string_payload(i32::MIN, &[], 0),
        string_payload(2, &[0x80, 0], 0),
        invalid_file,
    ];
    for initial in [None, Some(previous)] {
        let mut slot = initial.clone();
        for payload in &malformed {
            assert!(stage_visibility(&mut slot, payload, &profile()).is_err());
            assert_eq!(slot, initial);
        }
        stage_visibility(&mut slot, &valid, &profile()).unwrap();
        assert_eq!(slot, Some(parse_visibility(&valid, &profile()).unwrap()));
    }
}

#[test]
fn explicit_profiles_accept_qualified_routes_and_reject_unknown_or_wrong_metadata() {
    let release = ReleaseProfile::fixture("3.0.34");
    let payload = custom(true, false, |writer| hardcoded(writer, 0));
    for route in [
        VisibilityArchiveProfile::NormalLittleEndianNullMapCustom,
        VisibilityArchiveProfile::NormalLittleEndianPackageMapCustom,
    ] {
        let selected = VisibilityDecodeProfile::qualify(&release, route).unwrap();
        assert!(parse_visibility(&payload, &selected).is_ok());
    }
    assert!(
        VisibilityDecodeProfile::qualify(&release, VisibilityArchiveProfile::Unqualified).is_err()
    );
    let mut wrong_app = release.clone();
    wrong_app.pin.app_id = 0;
    let mut unknown_build = release.clone();
    unknown_build.pin.build_id = 0;
    let mut wrong_engine = release.clone();
    wrong_engine.engine_network_version = 6;
    let mut wrong_game = release.clone();
    wrong_game.game_network_version = 1;
    let mut wrong_bounds = release.clone();
    wrong_bounds.class_net_cache.inventory += 1;
    let mut wrong_version = release.clone();
    wrong_version.pin.project_version.push_str("-altered");
    wrong_version.pin.network_checksum =
        icarus_bevy::experimental::release::network_checksum(&wrong_version.pin.project_version);
    let mut missing = release.clone();
    missing.qualifications.as_mut().unwrap().visibility_decode = None;
    for unsupported in [
        wrong_app,
        unknown_build,
        wrong_engine,
        wrong_game,
        wrong_bounds,
        wrong_version,
        missing,
        ReleaseProfile::fixture("3.0.32"),
        ReleaseProfile::fixture("3.0.36"),
    ] {
        assert!(
            VisibilityDecodeProfile::qualify(
                &unsupported,
                VisibilityArchiveProfile::NormalLittleEndianPackageMapCustom,
            )
            .is_err()
        );
    }
}

#[test]
fn existing_reader_error_cannot_be_cleared_by_decoding() {
    let payload = custom(true, true, |writer| hardcoded(writer, 0));
    let mut reader = payload.reader();
    reader.read_bits(32);
    assert!(reader.is_error());
    assert!(read_visibility(&mut reader, &profile()).is_err());
    assert!(reader.is_error());
}

#[test]
fn diagnostics_do_not_disclose_names_or_number_values() {
    let payload = string_payload(20, b"SyntheticHiddenName\0", 0xdead_beef);
    let decoded = parse_visibility(&payload, &profile()).unwrap();
    let debug = format!("{decoded:?}");
    assert!(!debug.contains("SyntheticHiddenName"));
    assert!(!debug.contains("3735928559"));
    assert!(!debug.contains("deadbeef"));
    let malformed = with_tail(&payload, true);
    let error = format!(
        "{:#}",
        parse_visibility(&malformed, &profile()).unwrap_err()
    );
    assert!(!error.contains("SyntheticHiddenName"));
}

// Independently authored normal RPC82 field vectors; no observed capture bits.
fn literal_outer(bytes: &[u8], bits: usize) -> Payload {
    let mut source = BitReader::new(bytes, bits).unwrap();
    let mut writer = BitWriter::new();
    for _ in 0..bits {
        writer.write_bit(source.read_bit());
    }
    Payload::from_writer(writer)
}

#[test]
fn normal_rpc82_outer_vectors_preserve_presence_and_custom_bit_lengths() {
    for (bytes, bits, reuse, visible) in [
        (&[0x0b, 0x00][..], 12, true, false),
        (&[0x0f, 0x00][..], 12, true, true),
        (&[0x09, 0x10, 0x00][..], 21, false, false),
        (&[0x0d, 0x10, 0x00][..], 21, false, true),
    ] {
        let payload = literal_outer(bytes, bits);
        for route in [
            VisibilityArchiveProfile::NormalLittleEndianNullMapCustom,
            VisibilityArchiveProfile::NormalLittleEndianPackageMapCustom,
        ] {
            let selected =
                VisibilityDecodeProfile::qualify(&ReleaseProfile::fixture("3.0.34"), route)
                    .unwrap();
            let value = parse_visibility_rpc82(
                &payload,
                &selected,
                VisibilityRpc82Layout::NormalSingleCustomStruct,
            )
            .unwrap();
            let DecodedVisibilityParameter::Present(value) = value else {
                panic!("present fixture became absent");
            };
            assert_eq!(value.reuse_package_name, reuse);
            assert_eq!(value.visible, visible);
            assert_eq!(value.package_name, NetworkName::Hardcoded(0));
            assert_eq!(value.file_name, NetworkName::Hardcoded(0));
            assert_eq!(value.payload_bits, bits - 1);
            let mut custom_reader = payload.reader();
            assert!(custom_reader.read_bit());
            assert_eq!(
                read_visibility(&mut custom_reader, &selected).unwrap(),
                value
            );
        }
    }
    let absent = literal_outer(&[0], 1);
    assert_eq!(
        parse_visibility_rpc82(
            &absent,
            &profile(),
            VisibilityRpc82Layout::NormalSingleCustomStruct,
        )
        .unwrap(),
        DecodedVisibilityParameter::Absent
    );
    // The absent representation contains no fabricated custom/default names.
    assert!(parse_visibility(&absent, &profile()).is_err());
}

#[test]
fn normal_rpc82_outer_rejects_truncation_trailing_bits_and_unqualified_layout() {
    for (bytes, bits) in [
        (&[0x0b, 0][..], 12),
        (&[0x0f, 0][..], 12),
        (&[0x09, 0x10, 0][..], 21),
        (&[0x0d, 0x10, 0][..], 21),
        (&[0][..], 1),
    ] {
        let payload = literal_outer(bytes, bits);
        for truncated in 0..bits {
            assert!(
                parse_visibility_rpc82(
                    &prefix(&payload, truncated),
                    &profile(),
                    VisibilityRpc82Layout::NormalSingleCustomStruct,
                )
                .is_err()
            );
        }
        for tail in [false, true] {
            assert!(
                parse_visibility_rpc82(
                    &with_tail(&payload, tail),
                    &profile(),
                    VisibilityRpc82Layout::NormalSingleCustomStruct,
                )
                .is_err()
            );
        }
        assert!(
            parse_visibility_rpc82(&payload, &profile(), VisibilityRpc82Layout::Unqualified,)
                .is_err()
        );
    }
    assert!(
        VisibilityDecodeProfile::qualify(
            &ReleaseProfile::fixture("3.0.34"),
            VisibilityArchiveProfile::Unqualified,
        )
        .is_err()
    );
}

#[test]
fn normal_rpc82_outer_reuses_strict_raw_name_decoder_without_alignment() {
    let custom = custom(false, true, |writer| {
        string(writer, -3, &[0, 0xd8, 0xe9, 0, 0, 0], u32::MAX);
        string(writer, 3, b"Ab\0", 0x8000_0000);
    });
    let expected = parse_visibility(&custom, &profile()).unwrap();
    let mut writer = BitWriter::new();
    writer.write_bit(true);
    let mut reader = custom.reader();
    for _ in 0..custom.bit_len() {
        writer.write_bit(reader.read_bit());
    }
    assert_eq!(
        parse_visibility_rpc82(
            &Payload::from_writer(writer),
            &profile(),
            VisibilityRpc82Layout::NormalSingleCustomStruct,
        )
        .unwrap(),
        DecodedVisibilityParameter::Present(expected)
    );
}

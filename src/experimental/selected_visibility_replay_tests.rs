//! Draft authored schema fixtures, not original capture observations.
use super::*;
use serde_json::{Value, json};

fn profile() -> ReplayProfile {
    ReplayProfile::qualify(
        &ReleaseProfile::fixture("3.0.34"),
        VisibilityArchiveProfile::NormalLittleEndianPackageMapCustom,
        VisibilityRpc82Layout::NormalSingleCustomStruct,
    )
    .unwrap()
}

fn row(bits: usize, hex: &str) -> Value {
    json!({
        "format":1,"field_index":82,"offset_origin":"complete_bunch_payload",
        "ordinal":1,"connection_ordinal":1,"channel":7,"elapsed_ms":10,
        "engine_net_version":17,"archive_byte_order":"little",
        "reliable":true,"partial":false,"reassembled":false,"parts":1,
        "bunch_payload_bits":bits+16,"block_offset_bits":0,"block_length_bits":bits+16,
        "block_payload_offset_bits":8,"block_payload_bits":bits+8,
        "field_offset_bits":8,"field_length_bits":bits+8,
        "payload_offset_bits":16,"payload_bits":bits,"payload_hex":hex
    })
}

fn encode(rows: &[Value]) -> Vec<u8> {
    let mut out = Vec::new();
    for row in rows {
        serde_json::to_writer(&mut out, row).unwrap();
        out.push(b'\n');
    }
    out
}

fn run(rows: &[Value]) -> Result<ReplaySummary, ReplayError> {
    replay(encode(rows).as_slice(), &profile())
}

fn authored_present(reuse: bool) -> Value {
    let mut writer = BitWriter::new();
    writer.write_bit(true); // outer presence
    writer.write_bit(reuse);
    writer.write_bit(true); // visible
    writer.write_bit(true); // hardcoded name
    writer.write_int_packed(0);
    if !reuse {
        writer.write_bit(true);
        writer.write_int_packed(1);
    }
    let payload = Payload::from_writer(writer);
    let hex: String = payload
        .bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    row(payload.bit_len(), &hex)
}

#[test]
fn authored_present_absent_and_unaligned_custom_parse_without_completion_inference() {
    let mut rows = vec![
        row(1, "00"),
        authored_present(true),
        authored_present(false),
    ];
    for (index, row) in rows.iter_mut().enumerate() {
        row["ordinal"] = json!(index + 1);
    }
    let summary = run(&rows).unwrap();
    assert_eq!(summary.records, 3);
    assert_eq!(summary.accepted_absent, 1);
    assert_eq!(summary.accepted_present, 2);
    assert!(!summary.capture_complete_verified);
    assert!(!summary.provenance_verified && !summary.compatibility_proven);
}

#[test]
fn partial_initial_final_single_part_is_writer_valid() {
    let mut row = row(1, "00");
    row["partial"] = json!(true);
    row["reassembled"] = json!(true);
    assert_eq!(run(&[row]).unwrap().accepted_absent, 1);
}

#[test]
fn explicit_profile_and_layout_are_required() {
    assert!(matches!(
        ReplayProfile::qualify(
            &ReleaseProfile::fixture("3.0.34"),
            VisibilityArchiveProfile::Unqualified,
            VisibilityRpc82Layout::NormalSingleCustomStruct
        ),
        Err(ReplayError::Profile)
    ));
    assert!(matches!(
        ReplayProfile::qualify(
            &ReleaseProfile::fixture("3.0.34"),
            VisibilityArchiveProfile::NormalLittleEndianNullMapCustom,
            VisibilityRpc82Layout::Unqualified
        ),
        Err(ReplayError::Layout)
    ));
    let mut release = ReleaseProfile::fixture("3.0.34");
    release.pin.build_id += 1;
    assert!(matches!(
        ReplayProfile::qualify(
            &release,
            VisibilityArchiveProfile::NormalLittleEndianNullMapCustom,
            VisibilityRpc82Layout::NormalSingleCustomStruct
        ),
        Err(ReplayError::Profile)
    ));
}

#[test]
fn schema_and_writer_flag_mismatches_are_fixed_errors() {
    for (key, value, expected) in [
        ("format", json!(2), ReplayError::Metadata),
        ("field_index", json!(81), ReplayError::Metadata),
        (
            "offset_origin",
            json!("SENSITIVE_SECRET"),
            ReplayError::Metadata,
        ),
        ("connection_ordinal", json!(0), ReplayError::Metadata),
        ("engine_net_version", json!(18), ReplayError::Metadata),
        ("archive_byte_order", json!("big"), ReplayError::Metadata),
        ("reliable", json!(false), ReplayError::Metadata),
        ("partial", json!(true), ReplayError::Metadata),
        ("parts", json!(0), ReplayError::Metadata),
        ("parts", json!(2), ReplayError::Metadata),
        ("ordinal", json!(2), ReplayError::Ordinal),
        (
            "unexpected_secret",
            json!("SENSITIVE_SECRET"),
            ReplayError::JsonSchema,
        ),
        ("payload_bits", json!(-1), ReplayError::JsonSchema),
        ("channel", json!(4294967296u64), ReplayError::JsonSchema),
    ] {
        let mut row = row(1, "00");
        row[key] = value;
        assert_eq!(run(&[row]), Err(expected));
    }
}

#[test]
fn checked_interval_equations_reject_overflow_and_false_nesting() {
    for (key, value) in [
        ("block_offset_bits", usize::MAX),
        ("block_payload_offset_bits", 9),
        ("block_length_bits", 16),
        ("bunch_payload_bits", 16),
        ("field_offset_bits", 7),
        ("field_offset_bits", 16),
        ("field_length_bits", usize::MAX),
        ("payload_offset_bits", usize::MAX),
    ] {
        let mut row = row(1, "00");
        row[key] = json!(value);
        assert_eq!(run(&[row]), Err(ReplayError::Offsets));
    }
}

#[test]
fn hex_canonical_length_and_high_padding_are_strict() {
    for hex in ["", "0", "0000", "GG", "0A", "02", " 0"] {
        assert_eq!(run(&[row(1, hex)]), Err(ReplayError::Hex));
    }
}

#[test]
fn no_outer_retry_or_successful_prefix_after_malformed_tail() {
    let summary = run(&[row(2, "00")]).unwrap(); // absent plus trailing bit
    assert_eq!(summary.rejected, 1);
    assert!(!summary.all_decoded());
    let summary = run(&[row(1, "01")]).unwrap(); // present, truncated custom
    assert_eq!(summary.rejected, 1);
    let mut late = row(1, "00");
    late["ordinal"] = json!(2);
    late["payload_hex"] = json!("SECRET");
    assert_eq!(run(&[row(1, "00"), late]), Err(ReplayError::Hex));
}

#[test]
fn bounded_bytes_records_and_newline_framing() {
    assert_eq!(replay(&b"\n"[..], &profile()), Err(ReplayError::BlankLine));
    let mut bytes = encode(&[row(1, "00")]);
    bytes.pop();
    assert_eq!(
        replay(bytes.as_slice(), &profile()),
        Err(ReplayError::FinalNewline)
    );
    let bytes = vec![b'x'; MAX_OUTPUT_BYTES + 1];
    assert_eq!(
        replay(bytes.as_slice(), &profile()),
        Err(ReplayError::InputLimit)
    );
    let mut rows = vec![row(1, "00"); MAX_OBSERVATIONS];
    for (index, row) in rows.iter_mut().enumerate() {
        row["ordinal"] = json!(index + 1);
    }
    assert_eq!(run(&rows).unwrap().records, MAX_OBSERVATIONS as u64);
    rows.push(row(1, "00"));
    assert_eq!(run(&rows), Err(ReplayError::RecordLimit));
    assert_eq!(run(&[]).unwrap().records, 0);
}

#[test]
fn neutral_output_and_fixed_error_codes_never_echo_input() {
    let summary = run(&[row(1, "00")]).unwrap();
    let mut output = Vec::new();
    summary.write_json(&mut output).unwrap();
    let text = String::from_utf8(output).unwrap();
    assert!(
        !text.contains("payload_hex") && !text.contains("connection") && !text.contains("channel")
    );
    let mut bad = row(1, "00");
    bad["offset_origin"] = json!("SENSITIVE_SECRET");
    let error = run(&[bad]).unwrap_err();
    assert_eq!(error.to_string(), "Metadata");
    assert!(!format!("{error:?}").contains("SENSITIVE_SECRET"));
}

#[test]
fn output_failure_is_not_a_success_receipt() {
    struct Failing;
    impl std::io::Write for Failing {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("SECRET_IO_PATH"))
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    assert_eq!(
        run(&[row(1, "00")]).unwrap().write_json(Failing),
        Err(ReplayError::Output)
    );
}

#[test]
fn exact_input_and_field_caps_admit_boundary_but_reject_one_beyond() {
    let mut bytes = encode(&[row(1, "00")]);
    bytes.pop();
    bytes.resize(MAX_OUTPUT_BYTES - 1, b' ');
    bytes.push(b'\n');
    assert_eq!(
        replay(bytes.as_slice(), &profile())
            .unwrap()
            .accepted_absent,
        1
    );
    let at = row(MAX_FIELD_BYTES * 8, &"00".repeat(MAX_FIELD_BYTES));
    assert_eq!(run(&[at]).unwrap().rejected, 1);
    let over = row(MAX_FIELD_BYTES * 8 + 1, &"00".repeat(MAX_FIELD_BYTES + 1));
    assert_eq!(run(&[over]), Err(ReplayError::Metadata));
}

#[test]
fn read_and_flush_failures_become_fixed_errors() {
    struct ReadFailure;
    impl std::io::Read for ReadFailure {
        fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("SECRET_READ_PATH"))
        }
    }
    assert_eq!(replay(ReadFailure, &profile()), Err(ReplayError::Read));
    struct FlushFailure;
    impl std::io::Write for FlushFailure {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Err(std::io::Error::other("SECRET_FLUSH_PATH"))
        }
    }
    assert_eq!(
        run(&[row(1, "00")]).unwrap().write_json(FlushFailure),
        Err(ReplayError::Output)
    );
}

#[test]
fn uppercase_hex_is_admitted_without_weakening_tail_validation() {
    let mut upper = row(8, "AB");
    let mut lower = row(8, "ab");
    upper["payload_bits"] = json!(8);
    lower["payload_bits"] = json!(8);
    assert_eq!(run(&[upper]), run(&[lower]));
    assert!(run(&[row(8, "AB")]).is_ok());
}

#[test]
fn matched_release_reports_consumption_and_redacted_string_lengths() {
    let mut writer = BitWriter::new();
    writer.write_bit(true); // present
    writer.write_bit(true); // reuse
    writer.write_bit(false); // visible
    writer.write_bit(false); // string name
    writer.write_u32(7);
    writer.write_bytes(b"SECRET\0");
    writer.write_u32(0);
    let payload = Payload::from_writer(writer);
    let hex: String = payload
        .bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let rows = encode(&[row(payload.bit_len(), &hex)]);
    for release in [
        ReleaseProfile::fixture("3.0.34"),
        ReleaseProfile::fixture("3.0.35"),
    ] {
        let profile = ReplayProfile::qualify(
            &release,
            VisibilityArchiveProfile::NormalLittleEndianPackageMapCustom,
            VisibilityRpc82Layout::NormalSingleCustomStruct,
        )
        .unwrap();
        let summary = replay(rows.as_slice(), &profile).unwrap();
        assert_eq!(summary.accepted_present, 1);
        let detail = &summary.consumption[0];
        assert_eq!(detail.consumed_bits, payload.bit_len());
        assert_eq!(detail.package_name_units, Some(6));
        assert_eq!(detail.file_name_units, Some(6));
        let mut encoded = Vec::new();
        summary.write_json(&mut encoded).unwrap();
        let output = String::from_utf8(encoded).unwrap();
        assert!(!output.contains("SECRET") && !output.contains(&hex));
    }
}

#[test]
fn rejected_values_report_cursor_without_publishing_partial_name_lengths() {
    let summary = run(&[row(1, "01")]).unwrap();
    assert_eq!(summary.rejected, 1);
    assert_eq!(summary.consumption[0].consumed_bits, 1);
    assert_eq!(summary.consumption[0].package_name_units, None);
    assert_eq!(summary.consumption[0].file_name_units, None);
    let summary = run(&[row(2, "00")]).unwrap();
    assert_eq!(summary.rejected, 1);
    assert_eq!(summary.consumption[0].consumed_bits, 1);
    let summary = run(&[row(1, "00")]).unwrap();
    assert_eq!(summary.accepted_absent, 1);
    assert_eq!(summary.consumption[0].consumed_bits, 1);
    assert_eq!(summary.consumption[0].package_name_units, None);
}

//! Authored bits only: no original or private capture inputs.
use std::io::{Read, Seek, SeekFrom};

use super::*;
use crate::experimental::packet::{BitWriter, BunchHeader, Payload};

const CHANNEL: u32 = 2;

fn payload(bits: usize, seed: u32) -> Payload {
    let mut writer = BitWriter::new();
    for bit in 0..bits {
        writer.write_bit(
            (bit as u32)
                .wrapping_mul(7)
                .wrapping_add(seed)
                .is_multiple_of(3),
        );
    }
    Payload::from_writer(writer)
}

fn bunch(reliable: bool, payload: Payload) -> Bunch {
    Bunch {
        header: BunchHeader {
            control: false,
            open: false,
            close: false,
            close_reason: None,
            paused: false,
            reliable,
            channel_index: CHANNEL,
            has_package_map_exports: false,
            has_must_be_mapped_guids: false,
            partial: false,
            reliable_sequence_raw: reliable.then_some(5),
            partial_initial: false,
            partial_final: false,
            channel_name: None,
        },
        payload,
    }
}

fn tap(sink: &mut PrivatePawnBunchCapture, elapsed_ms: u64) -> PawnBunchTap<'_> {
    PawnBunchTap {
        sink,
        channel: CHANNEL,
        connection_ordinal: 1,
        elapsed_ms,
        unix_ms: 1_700_000_000_000 + elapsed_ms,
    }
}

fn unreliable(sink: &mut PrivatePawnBunchCapture, elapsed_ms: u64, bunch: &Bunch) {
    tap(sink, elapsed_ms).observe(bunch, PawnBunchPhase::UnreliableArrival, 9, None);
}

fn rows(file: &mut File) -> Vec<serde_json::Value> {
    file.seek(SeekFrom::Start(0)).unwrap();
    let mut text = String::new();
    file.read_to_string(&mut text).unwrap();
    text.lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

/// Rebuild the payload from a row exactly as an offline reader would.
fn decode_row(row: &serde_json::Value) -> (usize, Vec<u8>) {
    let bits = row["payload_bits"].as_u64().unwrap() as usize;
    let hex = row["payload_hex"].as_str().unwrap();
    let bytes = (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(bytes.len(), bits.div_ceil(8));
    (bits, bytes)
}

#[test]
fn record_keeps_header_and_bit_exact_payload_with_masked_tail() {
    let mut file = tempfile::tempfile().unwrap();
    let mut sink = PrivatePawnBunchCapture::from_file(file.try_clone().unwrap());
    // 13 bits: the last byte's three high bits are padding.
    let mut raw = bunch(true, payload(13, 1));
    raw.header.partial = true;
    raw.header.partial_initial = true;
    tap(&mut sink, 42).observe(&raw, PawnBunchPhase::ReliableQueueRelease, 77, Some(1029));
    let counts = sink.finish();
    assert_eq!(counts.observations, 1);
    assert_eq!(counts.kept_first, 1);
    assert_eq!(counts.retained_payload_bits, 13);
    assert_eq!(counts.incomplete, 0);
    let rows = rows(&mut file);
    let [row] = &rows[..] else { panic!("one row") };
    assert_eq!(row["format"], 1);
    assert_eq!(row["selection"], "pawn_channel_bunches");
    assert_eq!(row["ordinal"], 1);
    assert_eq!(row["keep_reason"], "first");
    assert_eq!(row["connection_ordinal"], 1);
    assert_eq!(row["connection_seen"], 1);
    assert_eq!(row["channel"], CHANNEL);
    assert_eq!(row["observation_phase"], "reliable_queue_release");
    assert_eq!(row["elapsed_ms"], 42);
    assert_eq!(row["unix_ms"], 1_700_000_000_042_u64);
    assert_eq!(row["packet_sequence"], 77);
    assert_eq!(row["reliable"], true);
    assert_eq!(row["reliable_sequence"], 1029);
    assert_eq!(row["reliable_sequence_raw"], 5);
    assert_eq!(row["partial"], true);
    assert_eq!(row["partial_initial"], true);
    assert_eq!(row["partial_final"], false);
    for flag in [
        "open",
        "close",
        "paused",
        "has_package_map_exports",
        "has_must_be_mapped_guids",
    ] {
        assert_eq!(row[flag], false, "{flag}");
    }
    // Bit-exact: every payload bit matches and the tail is zero.
    let (bits, bytes) = decode_row(row);
    assert_eq!(bits, raw.payload.bit_len());
    let mut reader = raw.payload.reader();
    for bit in 0..bits {
        assert_eq!(bytes[bit / 8] >> (bit % 8) & 1 == 1, reader.read_bit());
    }
    assert_eq!(bytes[1] & 0xe0, 0);
}

#[test]
fn other_channels_are_never_observed() {
    let mut file = tempfile::tempfile().unwrap();
    let mut sink = PrivatePawnBunchCapture::from_file(file.try_clone().unwrap());
    let mut other = bunch(false, payload(8, 0));
    other.header.channel_index = CHANNEL + 1;
    unreliable(&mut sink, 0, &other);
    assert_eq!(sink.finish().seen, 0);
    assert!(rows(&mut file).is_empty());
}

#[test]
fn sampling_keeps_first_then_reliable_changed_and_interval() {
    let mut file = tempfile::tempfile().unwrap();
    let mut sink = PrivatePawnBunchCapture::from_file(file.try_clone().unwrap());
    let steady = bunch(false, payload(70, 0));
    for _ in 0..KEEP_FIRST {
        unreliable(&mut sink, 0, &steady);
    }
    // Identical: sampled out.
    unreliable(&mut sink, 1, &steady);
    unreliable(&mut sink, 2, &steady);
    // Same length, different leading bits: kept; then the steady one again
    // differs from that kept one: kept.
    unreliable(&mut sink, 3, &bunch(false, payload(70, 1)));
    unreliable(&mut sink, 4, &steady);
    // A difference only after the leading bits: sampled out.
    let mut late = payload(70, 0).bytes().to_vec();
    late[6] ^= 0x01;
    let late = Payload::from_writer({
        let mut writer = BitWriter::new();
        for bit in 0..70 {
            writer.write_bit(late[bit / 8] >> (bit % 8) & 1 == 1);
        }
        writer
    });
    unreliable(&mut sink, 5, &bunch(false, late));
    // Different length: kept.
    unreliable(&mut sink, 6, &bunch(false, payload(71, 0)));
    // A reliable bunch is always kept, even if it matches.
    let reliable = bunch(true, payload(71, 0));
    tap(&mut sink, 7).observe(&reliable, PawnBunchPhase::ReliableQueueRelease, 10, Some(2));
    tap(&mut sink, 8).observe(&reliable, PawnBunchPhase::ReliableQueueRelease, 11, Some(3));
    // Steady again: differs from the reliable one (flags), kept; then the
    // interval keeps one only once KEEP_INTERVAL_MS has passed.
    unreliable(&mut sink, 9, &steady);
    unreliable(&mut sink, 9 + KEEP_INTERVAL_MS - 1, &steady);
    unreliable(&mut sink, 9 + KEEP_INTERVAL_MS, &steady);
    unreliable(&mut sink, 9 + KEEP_INTERVAL_MS + 1, &steady);
    let counts = sink.finish();
    assert_eq!(counts.seen, KEEP_FIRST + 12);
    assert_eq!(counts.kept_first, KEEP_FIRST);
    assert_eq!(counts.kept_changed, 4);
    assert_eq!(counts.kept_reliable, 2);
    assert_eq!(counts.kept_interval, 1);
    assert_eq!(counts.sampled_out, 5);
    assert_eq!(counts.observations, KEEP_FIRST + 7);
    // Sampling is by design: the file is still complete.
    assert_eq!(counts.incomplete, 0);
    let rows = rows(&mut file);
    assert_eq!(rows.len() as u64, counts.observations);
    let tail = &rows[KEEP_FIRST as usize..];
    let reasons: Vec<_> = tail.iter().map(|row| row["keep_reason"].clone()).collect();
    assert_eq!(
        reasons,
        [
            "changed", "changed", "changed", "reliable", "reliable", "changed", "interval"
        ]
    );
    // connection_seen exposes the gaps left by sampling.
    let seen: Vec<_> = tail
        .iter()
        .map(|row| row["connection_seen"].as_u64().unwrap() - KEEP_FIRST)
        .collect();
    assert_eq!(seen, [3, 4, 6, 7, 8, 9, 11]);
    for (index, row) in rows.iter().enumerate() {
        assert_eq!(row["ordinal"], index as u64 + 1);
    }
}

#[test]
fn first_rule_and_state_are_per_connection_and_forgotten_on_removal() {
    let mut sink = PrivatePawnBunchCapture::from_file(tempfile::tempfile().unwrap());
    let steady = bunch(false, payload(16, 0));
    for _ in 0..=KEEP_FIRST {
        unreliable(&mut sink, 0, &steady);
    }
    assert_eq!(sink.counts().sampled_out, 1);
    let mut other = PawnBunchTap {
        connection_ordinal: 2,
        ..tap(&mut sink, 0)
    };
    other.observe(&steady, PawnBunchPhase::UnreliableArrival, 1, None);
    assert_eq!(sink.counts().kept_first, KEEP_FIRST + 1);
    sink.forget_connection(1);
    assert!(!sink.connections.contains_key(&1));
    assert!(sink.connections.contains_key(&2));
}

#[test]
fn row_cap_drops_and_marks_incomplete() {
    let mut sink = PrivatePawnBunchCapture::from_file(tempfile::tempfile().unwrap());
    sink.counts.observations = MAX_OBSERVATIONS as u64;
    unreliable(&mut sink, 0, &bunch(false, payload(8, 0)));
    let counts = sink.finish();
    assert_eq!(counts.dropped, 1);
    assert_eq!(counts.observations, MAX_OBSERVATIONS as u64);
    assert_eq!(counts.output_bytes, 0);
    assert_eq!(counts.incomplete, 1);
}

#[test]
fn output_byte_cap_refuses_a_row_that_would_cross_it() {
    let mut file = tempfile::tempfile().unwrap();
    let mut sink = PrivatePawnBunchCapture::from_file(file.try_clone().unwrap());
    sink.counts.output_bytes = MAX_OUTPUT_BYTES as u64 - 10;
    unreliable(&mut sink, 0, &bunch(false, payload(8, 0)));
    let counts = sink.finish();
    assert_eq!(counts.dropped, 1);
    assert_eq!(counts.observations, 0);
    assert_eq!(counts.output_bytes, MAX_OUTPUT_BYTES as u64 - 10);
    assert_eq!(counts.incomplete, 1);
    assert!(rows(&mut file).is_empty());
}

#[test]
fn per_item_cap_counts_oversize_and_keeps_the_rest() {
    let mut file = tempfile::tempfile().unwrap();
    let mut sink = PrivatePawnBunchCapture::from_file(file.try_clone().unwrap());
    unreliable(
        &mut sink,
        0,
        &bunch(false, payload(MAX_PAYLOAD_BYTES * 8 + 1, 0)),
    );
    unreliable(
        &mut sink,
        0,
        &bunch(false, payload(MAX_PAYLOAD_BYTES * 8, 0)),
    );
    let counts = sink.finish();
    assert_eq!(counts.oversize, 1);
    assert_eq!(counts.observations, 1);
    assert_eq!(counts.retained_payload_bits, (MAX_PAYLOAD_BYTES * 8) as u64);
    assert_eq!(counts.incomplete, 1);
    let rows = rows(&mut file);
    assert_eq!(
        rows[0]["payload_hex"].as_str().unwrap().len(),
        MAX_PAYLOAD_BYTES * 2
    );
}

#[test]
fn write_failure_disables_capture_and_finish_reports_it() {
    let readonly = tempfile::NamedTempFile::new().unwrap();
    let mut sink = PrivatePawnBunchCapture::from_file(File::open(readonly.path()).unwrap());
    let steady = bunch(false, payload(8, 0));
    unreliable(&mut sink, 0, &steady);
    unreliable(&mut sink, 0, &steady);
    let counts = sink.finish();
    assert_eq!(counts.write_errors, 1);
    assert_eq!(counts.observations, 0);
    assert_eq!(counts.seen, 1, "disabled after the failure");
    assert_eq!(counts.incomplete, 1);
    // Finished: nothing more is observed.
    unreliable(&mut sink, 0, &steady);
    assert_eq!(sink.counts().seen, 1);
}

#[test]
fn counts_and_debug_expose_no_payload() {
    let mut sink = PrivatePawnBunchCapture::from_file(tempfile::tempfile().unwrap());
    unreliable(&mut sink, 0, &bunch(false, payload(64, 2)));
    assert_eq!(sink.counts().incomplete, 1, "until finished");
    let text = serde_json::to_string(&sink.finish()).unwrap();
    assert!(!text.contains("payload_hex"));
    assert!(!format!("{sink:?}").contains("payload_hex"));
}

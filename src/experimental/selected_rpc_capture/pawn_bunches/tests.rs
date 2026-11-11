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
    assert_eq!(row["format"], 2);
    assert_eq!(row["selection"], "pawn_channel_bunches");
    assert_eq!(row["ordinal"], 1);
    assert_eq!(row["keep_reason"], "first");
    assert_eq!(row["connection_ordinal"], 1);
    assert_eq!(row["connection_seen"], 1);
    assert!(row["shape_seen"].is_null(), "reliable: no shape");
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
fn sampling_keeps_first_reliable_new_shapes_and_per_shape_interval() {
    let mut file = tempfile::tempfile().unwrap();
    let mut sink = PrivatePawnBunchCapture::from_file(file.try_clone().unwrap());
    let steady = bunch(false, payload(70, 0));
    for _ in 0..KEEP_FIRST {
        unreliable(&mut sink, 0, &steady);
    }
    // Same shape as the first ones (already past SHAPE_FIRST): sampled out.
    unreliable(&mut sink, 1, &steady);
    // A new shape (leading bits differ): its first SHAPE_FIRST are kept,
    // interleaved with the steady shape, which stays sampled out.
    let other = bunch(false, payload(70, 1));
    for at in 0..SHAPE_FIRST + 1 {
        unreliable(&mut sink, 2 + at, &other);
        unreliable(&mut sink, 2 + at, &steady);
    }
    // A difference only after the leading bits is the same shape.
    let mut late = payload(70, 0).bytes().to_vec();
    late[6] ^= 0x01;
    let late = Payload::from_writer({
        let mut writer = BitWriter::new();
        for bit in 0..70 {
            writer.write_bit(late[bit / 8] >> (bit % 8) & 1 == 1);
        }
        writer
    });
    unreliable(&mut sink, 20, &bunch(false, late));
    // A reliable bunch is always kept, even if it repeats.
    let reliable = bunch(true, payload(71, 0));
    tap(&mut sink, 21).observe(&reliable, PawnBunchPhase::ReliableQueueRelease, 10, Some(2));
    tap(&mut sink, 22).observe(&reliable, PawnBunchPhase::ReliableQueueRelease, 11, Some(3));
    // Each shape's interval runs from its own last kept row: steady from
    // the first phase (0 ms), other from its last new_shape row (5 ms).
    unreliable(&mut sink, KEEP_INTERVAL_MS - 1, &steady);
    unreliable(&mut sink, KEEP_INTERVAL_MS, &steady);
    unreliable(&mut sink, KEEP_INTERVAL_MS + 1, &steady);
    unreliable(&mut sink, KEEP_INTERVAL_MS + 2, &other);
    unreliable(&mut sink, KEEP_INTERVAL_MS + 5, &other);
    let counts = sink.finish();
    assert_eq!(counts.seen, KEEP_FIRST + 19);
    assert_eq!(counts.kept_first, KEEP_FIRST);
    assert_eq!(counts.kept_new_shape, SHAPE_FIRST);
    assert_eq!(counts.kept_reliable, 2);
    assert_eq!(counts.kept_interval, 2);
    assert_eq!(counts.sampled_out, 11);
    assert_eq!(counts.shapes_tracked_max, 2);
    assert_eq!(counts.observations, KEEP_FIRST + 8);
    // Sampling is by design: nothing lost, the file is complete.
    assert_eq!(counts.lost_to_cap, 0);
    assert_eq!(counts.incomplete, 0);
    let rows = rows(&mut file);
    assert_eq!(rows.len() as u64, counts.observations);
    let tail = &rows[KEEP_FIRST as usize..];
    let reasons: Vec<_> = tail.iter().map(|row| row["keep_reason"].clone()).collect();
    assert_eq!(
        reasons,
        [
            "new_shape",
            "new_shape",
            "new_shape",
            "new_shape",
            "reliable",
            "reliable",
            "interval",
            "interval"
        ]
    );
    let shape_seen: Vec<_> = tail.iter().map(|row| row["shape_seen"].as_u64()).collect();
    assert_eq!(
        shape_seen,
        [
            Some(1),
            Some(2),
            Some(3),
            Some(4),
            None,
            None,
            Some(KEEP_FIRST + 9),
            Some(7)
        ]
    );
    // connection_seen exposes the gaps left by sampling.
    let seen: Vec<_> = tail
        .iter()
        .map(|row| row["connection_seen"].as_u64().unwrap() - KEEP_FIRST)
        .collect();
    assert_eq!(seen, [2, 4, 6, 8, 13, 14, 16, 19]);
    for (index, row) in rows.iter().enumerate() {
        assert_eq!(row["ordinal"], index as u64 + 1);
    }
}

/// Review regression: frequent alternating shapes once filled a shared row
/// cap in about 100 s and a later reliable bunch was dropped. Now neither a
/// reliable bunch nor a never-seen shape can be crowded out.
#[test]
fn frequent_alternating_shapes_cannot_crowd_out_a_late_report() {
    let mut file = tempfile::tempfile().unwrap();
    let mut sink = PrivatePawnBunchCapture::from_file(file.try_clone().unwrap());
    let shapes: Vec<_> = (0..3).map(|seed| bunch(false, payload(70, seed))).collect();
    // 83 per second (12 ms apart) for two hours.
    let frames = 2 * 3600 * 1000 / 12;
    for frame in 0..frames {
        unreliable(&mut sink, frame * 12, &shapes[(frame % 3) as usize]);
    }
    let at = frames * 12;
    tap(&mut sink, at).observe(
        &bunch(true, payload(40, 0)),
        PawnBunchPhase::ReliableQueueRelease,
        1,
        Some(1),
    );
    unreliable(&mut sink, at, &bunch(false, payload(41, 0)));
    let counts = sink.finish();
    assert_eq!(counts.lost_to_cap, 0);
    assert_eq!(counts.incomplete, 0);
    assert_eq!(counts.kept_reliable, 1);
    assert_eq!(counts.kept_new_shape, 1);
    let rows = rows(&mut file);
    let reasons: Vec<_> = rows[rows.len() - 2..]
        .iter()
        .map(|row| row["keep_reason"].clone())
        .collect();
    assert_eq!(reasons, ["reliable", "new_shape"]);
}

/// Feed `shapes` round-robin at 83 per second until the first loss to a
/// cap; returns its time. Then a reliable bunch and a never-seen shape are
/// still kept.
fn first_loss_ms(shapes: &[Bunch]) -> u64 {
    let mut sink = PrivatePawnBunchCapture::from_file(tempfile::tempfile().unwrap());
    let mut frame = 0u64;
    while sink.counts().lost_to_cap == 0 {
        unreliable(
            &mut sink,
            frame * 12,
            &shapes[(frame as usize) % shapes.len()],
        );
        frame += 1;
    }
    let at = frame * 12;
    let counts = sink.counts();
    assert_eq!(counts.lost_interval, 1, "only the interval pool runs out");
    tap(&mut sink, at).observe(
        &bunch(true, payload(40, 0)),
        PawnBunchPhase::ReliableQueueRelease,
        1,
        Some(1),
    );
    unreliable(&mut sink, at, &bunch(false, payload(9, 0)));
    let after = sink.counts();
    assert_eq!(after.kept_reliable, counts.kept_reliable + 1);
    assert_eq!(after.kept_new_shape, counts.kept_new_shape + 1);
    assert_eq!(after.lost_to_cap, 1);
    assert_eq!(after.incomplete, 1);
    at
}

/// The worst-case durations stated in the runbook: how long interval
/// samples of the frequent traffic last before their pool runs out.
#[test]
fn interval_samples_last_hours_for_one_three_or_two_hundred_shapes() {
    const HOUR: u64 = 3600 * 1000;
    let one = [bunch(false, payload(70, 0))];
    let three: Vec<_> = (0..3).map(|seed| bunch(false, payload(70, seed))).collect();
    let many: Vec<_> = (0..200)
        .map(|extra| bunch(false, payload(64 + extra, 0)))
        .collect();
    let one = first_loss_ms(&one);
    let three = first_loss_ms(&three);
    let many = first_loss_ms(&many);
    assert!(one > HOUR * 85 / 10, "one shape: {one} ms");
    assert!(three > HOUR * 28 / 10, "three shapes: {three} ms");
    assert!(many > HOUR * 17 / 10, "200 shapes: {many} ms");
}

#[test]
fn shapes_beyond_the_bound_fall_back_to_one_counted_interval() {
    let mut file = tempfile::tempfile().unwrap();
    let mut sink = PrivatePawnBunchCapture::from_file(file.try_clone().unwrap());
    let steady = bunch(false, payload(8, 0));
    for _ in 0..KEEP_FIRST {
        unreliable(&mut sink, 0, &steady);
    }
    // Fill the shape table with distinct lengths, then three more shapes.
    for extra in 1..MAX_SHAPES + 3 {
        unreliable(&mut sink, 1, &bunch(false, payload(8 + extra, 0)));
    }
    unreliable(
        &mut sink,
        1 + KEEP_INTERVAL_MS,
        &bunch(false, payload(9000, 0)),
    );
    let counts = sink.finish();
    assert_eq!(counts.shapes_tracked_max, MAX_SHAPES as u64);
    assert_eq!(counts.untracked_shape_bunches, 4);
    assert_eq!(counts.kept_new_shape, MAX_SHAPES as u64 - 1);
    // First untracked bunch and the one a full interval later.
    assert_eq!(counts.kept_interval, 2);
    assert_eq!(counts.sampled_out, 2);
    assert_eq!(counts.lost_to_cap, 0);
    let rows = rows(&mut file);
    let last = rows.last().unwrap();
    assert_eq!(last["keep_reason"], "interval");
    assert!(last["shape_seen"].is_null(), "untracked");
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
fn a_full_pool_counts_losses_without_touching_the_other_pools() {
    let mut sink = PrivatePawnBunchCapture::from_file(tempfile::tempfile().unwrap());
    // A never-seen shape finds the first pool full, falls back to the
    // new-shape rule and finds that pool full too: a real loss.
    sink.used[Pool::First as usize].rows = POOLS[Pool::First as usize].rows;
    sink.used[Pool::NewShape as usize].rows = POOLS[Pool::NewShape as usize].rows;
    unreliable(&mut sink, 0, &bunch(false, payload(8, 0)));
    let counts = sink.counts();
    assert_eq!(counts.first_pool_full, 1);
    assert_eq!(counts.lost_new_shape, 1);
    assert_eq!(counts.lost_to_cap, 1, "visible before finish");
    assert_eq!(counts.observations, 0);
    assert_eq!(counts.output_bytes, 0);
    // The reliable pool is untouched by the full pools.
    tap(&mut sink, 0).observe(
        &bunch(true, payload(8, 0)),
        PawnBunchPhase::ReliableQueueRelease,
        1,
        Some(1),
    );
    let counts = sink.finish();
    assert_eq!(counts.kept_first, 1);
    assert_eq!(counts.observations, 1);
    assert_eq!(counts.lost_to_cap, 1);
    assert_eq!(counts.incomplete, 1);
}

#[test]
fn reliable_pool_full_is_a_counted_loss() {
    let mut sink = PrivatePawnBunchCapture::from_file(tempfile::tempfile().unwrap());
    sink.used[Pool::Reliable as usize].rows = POOLS[Pool::Reliable as usize].rows;
    tap(&mut sink, 0).observe(
        &bunch(true, payload(8, 0)),
        PawnBunchPhase::ReliableQueueRelease,
        1,
        Some(1),
    );
    let counts = sink.counts();
    assert_eq!(counts.lost_reliable, 1);
    assert_eq!(counts.lost_to_cap, 1);
    assert_eq!(counts.observations, 0);
}

#[test]
fn pool_byte_budget_refuses_a_row_that_would_cross_it() {
    let mut file = tempfile::tempfile().unwrap();
    let mut sink = PrivatePawnBunchCapture::from_file(file.try_clone().unwrap());
    sink.used[Pool::Reliable as usize].bytes = POOLS[Pool::Reliable as usize].bytes - 10;
    tap(&mut sink, 0).observe(
        &bunch(true, payload(8, 0)),
        PawnBunchPhase::ReliableQueueRelease,
        1,
        Some(1),
    );
    let counts = sink.finish();
    assert_eq!(counts.lost_reliable, 1);
    assert_eq!(counts.lost_to_cap, 1);
    assert_eq!(counts.observations, 0);
    assert_eq!(counts.output_bytes, 0);
    assert_eq!(counts.incomplete, 1);
    assert!(rows(&mut file).is_empty());
}

/// A first-pool refusal, by rows or by bytes, is never a loss: the bunch
/// is kept by the shape rules or sampled out.
#[test]
fn a_full_first_pool_falls_back_to_the_shape_rules() {
    let mut file = tempfile::tempfile().unwrap();
    let mut sink = PrivatePawnBunchCapture::from_file(file.try_clone().unwrap());
    sink.used[Pool::First as usize].bytes = POOLS[Pool::First as usize].bytes - 10;
    let steady = bunch(false, payload(8, 0));
    for _ in 0..=SHAPE_FIRST {
        unreliable(&mut sink, 0, &steady);
    }
    let counts = sink.finish();
    assert_eq!(counts.first_pool_full, SHAPE_FIRST + 1);
    assert_eq!(counts.kept_first, 0);
    assert_eq!(counts.kept_new_shape, SHAPE_FIRST);
    assert_eq!(counts.sampled_out, 1);
    assert_eq!(counts.lost_to_cap, 0);
    assert_eq!(counts.incomplete, 0);
    let rows = rows(&mut file);
    assert_eq!(rows.len() as u64, SHAPE_FIRST);
    for (index, row) in rows.iter().enumerate() {
        assert_eq!(row["keep_reason"], "new_shape");
        assert_eq!(row["ordinal"], index as u64 + 1);
        assert_eq!(row["shape_seen"], index as u64 + 1);
    }
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
    assert_eq!(counts.lost_to_cap, 1);
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

fn on(sink: &mut PrivatePawnBunchCapture, connection: u32, elapsed_ms: u64) -> PawnBunchTap<'_> {
    PawnBunchTap {
        connection_ordinal: connection,
        ..tap(sink, elapsed_ms)
    }
}

/// Review regression: a third connection's first bunches found the first
/// pool (two connections' worth) full and were counted as lost, raising a
/// false `lost_to_cap` alarm. They now fall back to the ordinary rules.
#[test]
fn reconnects_past_the_first_pool_fall_back_to_sampling_without_a_loss() {
    const CONNECTIONS: u32 = 4;
    let mut file = tempfile::tempfile().unwrap();
    let mut sink = PrivatePawnBunchCapture::from_file(file.try_clone().unwrap());
    let steady = bunch(false, payload(70, 0));
    let per_connection = KEEP_FIRST + 100;
    for connection in 1..=CONNECTIONS {
        let start = u64::from(connection) * 1_000_000;
        for n in 0..per_connection {
            on(&mut sink, connection, start + n * 12).observe(
                &steady,
                PawnBunchPhase::UnreliableArrival,
                9,
                None,
            );
        }
        // Each connection's state is fresh, so the pool fills after two.
        assert_eq!(sink.counts().lost_to_cap, 0, "connection {connection}");
    }
    let at = u64::from(CONNECTIONS + 1) * 1_000_000;
    on(&mut sink, CONNECTIONS, at).observe(
        &bunch(true, payload(40, 0)),
        PawnBunchPhase::ReliableQueueRelease,
        1,
        Some(1),
    );
    on(&mut sink, CONNECTIONS, at).observe(
        &bunch(false, payload(41, 0)),
        PawnBunchPhase::UnreliableArrival,
        2,
        None,
    );
    let counts = sink.finish();
    assert_eq!(counts.lost_to_cap, 0);
    assert_eq!(counts.incomplete, 0);
    assert_eq!(counts.kept_first, POOLS[Pool::First as usize].rows);
    let full = u64::from(CONNECTIONS) * KEEP_FIRST - POOLS[Pool::First as usize].rows;
    assert_eq!(counts.first_pool_full, full);
    // Connections 3 and 4 keep their steady shape's first SHAPE_FIRST by
    // the new-shape rule; the late never-seen shape adds one more.
    assert_eq!(counts.kept_new_shape, 2 * SHAPE_FIRST + 1);
    assert_eq!(counts.kept_reliable, 1);
    assert_eq!(counts.kept_interval, 0);
    assert_eq!(
        counts.sampled_out,
        counts.seen - counts.observations,
        "every fall-through is kept or sampled out"
    );
    let rows = rows(&mut file);
    assert_eq!(rows.len() as u64, counts.observations);
    let last: Vec<_> = rows[rows.len() - 2..]
        .iter()
        .map(|row| {
            (
                row["keep_reason"].clone(),
                row["connection_ordinal"].clone(),
            )
        })
        .collect();
    assert_eq!(
        last,
        [
            ("reliable".into(), CONNECTIONS.into()),
            ("new_shape".into(), CONNECTIONS.into())
        ]
    );
}

//! Authored bits only: no original or private capture inputs.
use super::*;
use crate::experimental::{
    actor_channel::{
        ActorBunchBody, ContentBlock, NetGuidRef, SubObjectClass, encode_actor_bunch_body,
    },
    packet::{BitReader, BitWriter, BunchHeader, Payload},
    rep_rpc::{
        BP_ICARUS_PLAYER_CONTROLLER_SURVIVAL_FIELD_INDEX_BOUND, FieldHeader, write_field_header,
    },
};

fn codec() -> ActorChannelCodec {
    ActorChannelCodec {
        engine_net_version: 17,
        exports_enabled: false,
        has_level: true,
        order: ByteOrder::Little,
        limits: ActorChannelLimits::default(),
    }
}

fn bits(count: usize) -> Payload {
    let mut w = BitWriter::new();
    for i in 0..count {
        w.write_bit(i % 7 < 3);
    }
    Payload::from_writer(w)
}

fn block(target: BlockTarget, index: u32, payload: Payload) -> ContentBlock {
    let mut w = BitWriter::new();
    write_field_header(
        &mut w,
        &FieldHeader { index, payload },
        BP_ICARUS_PLAYER_CONTROLLER_SURVIVAL_FIELD_INDEX_BOUND,
    )
    .unwrap();
    ContentBlock {
        has_rep_layout: false,
        target,
        payload: Payload::from_writer(w),
    }
}

fn bunch(blocks: Vec<ContentBlock>) -> Bunch {
    Bunch {
        header: BunchHeader {
            control: false,
            open: false,
            close: false,
            close_reason: None,
            paused: false,
            reliable: true,
            channel_index: 3,
            has_package_map_exports: false,
            has_must_be_mapped_guids: false,
            partial: false,
            reliable_sequence_raw: Some(1),
            partial_initial: false,
            partial_final: false,
            channel_name: None,
        },
        payload: encode_actor_bunch_body(
            &ActorBunchBody {
                spawn: None,
                blocks,
            },
            &codec(),
        )
        .unwrap(),
    }
}

fn tap(sink: &mut PrivateRpc82Capture) -> CaptureTap<'_> {
    CaptureTap {
        sink,
        channel: 3,
        connection_ordinal: 1,
        elapsed_ms: 23,
        engine_net_version: 17,
        order: ByteOrder::Little,
        components: [Some(5), None, None],
    }
}

fn rows(file: &mut File) -> Vec<serde_json::Value> {
    use std::io::{Read, Seek, SeekFrom};
    file.seek(SeekFrom::Start(0)).unwrap();
    let mut text = String::new();
    file.read_to_string(&mut text).unwrap();
    text.lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

#[test]
fn exact_612_bits_roundtrip_and_offsets_include_unaligned_tail() {
    let mut output = tempfile::tempfile().unwrap();
    let mut sink = PrivateRpc82Capture::from_file(output.try_clone().unwrap());
    let payload = bits(612);
    let wire = bunch(vec![block(BlockTarget::Actor, 82, payload.clone())]);
    tap(&mut sink).observe(&wire);
    let counts = sink.finish();
    assert_eq!(counts.observations, 1);
    assert_eq!(counts.retained_payload_bits, 612);
    assert_eq!(counts.incomplete, 0);
    let rows = rows(&mut output);
    let row = &rows[0];
    assert_eq!(row["payload_bits"], 612);
    let hex = row["payload_hex"].as_str().unwrap();
    let bytes: Vec<_> = (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
        .collect();
    assert_eq!(bytes, payload.bytes());
    assert_eq!(bytes.last().unwrap() & 0xf0, 0);
    let start = row["payload_offset_bits"].as_u64().unwrap() as usize;
    assert_ne!(start % 8, 0);
    let mut wire_reader = wire.payload.reader();
    for _ in 0..start {
        wire_reader.read_bit();
    }
    let mut retained = BitReader::new(&bytes, 612).unwrap();
    for _ in 0..612 {
        assert_eq!(wire_reader.read_bit(), retained.read_bit());
    }
    assert_eq!(row["block_offset_bits"], 0);
    assert_eq!(row["block_length_bits"], wire.payload.bit_len());
    assert_eq!(row["field_offset_bits"], row["block_payload_offset_bits"]);
    assert_eq!(
        row["payload_offset_bits"].as_u64().unwrap() + 612,
        wire.payload.bit_len() as u64
    );
    assert_eq!(counts.output_bytes, output.metadata().unwrap().len());
    assert!(
        serde_json::to_value(counts)
            .unwrap()
            .as_object()
            .unwrap()
            .values()
            .all(serde_json::Value::is_u64)
    );
    assert!(!format!("{sink:?}").contains(hex));
}

#[test]
fn first_non_612_payload_is_retained_and_selection_excludes_other_targets() {
    let mut output = tempfile::tempfile().unwrap();
    let mut sink = PrivateRpc82Capture::from_file(output.try_clone().unwrap());
    let mut wrong_channel = bunch(vec![block(BlockTarget::Actor, 82, bits(612))]);
    wrong_channel.header.channel_index = 4;
    tap(&mut sink).observe(&wrong_channel);
    let subobject = BlockTarget::SubObject {
        object: NetGuidRef::plain(999),
        class: SubObjectClass::ClientSent,
    };
    let mut component_fields = BitWriter::new();
    write_field_header(
        &mut component_fields,
        &FieldHeader {
            index: 82,
            payload: bits(612),
        },
        crate::experimental::rep_rpc::BACKEND_PROXY_FIELD_INDEX_BOUND,
    )
    .unwrap();
    let permitted_subobject = ContentBlock {
        has_rep_layout: false,
        target: BlockTarget::SubObject {
            object: NetGuidRef::plain(5),
            class: SubObjectClass::ClientSent,
        },
        payload: Payload::from_writer(component_fields),
    };
    tap(&mut sink).observe(&bunch(vec![
        block(subobject, 82, bits(612)),
        permitted_subobject,
        block(BlockTarget::Actor, 81, bits(612)),
        block(BlockTarget::Actor, 82, bits(13)),
    ]));
    let rows = rows(&mut output);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["payload_bits"], 13);
    assert_eq!(rows[0]["ordinal"], 1);
    assert_eq!(sink.counts().retained_payload_bits, 13);
}

#[test]
fn malformed_trailing_field_or_block_publishes_no_selected_prefix() {
    for malformed_block in [false, true] {
        let mut output = tempfile::tempfile().unwrap();
        let mut sink = PrivateRpc82Capture::from_file(output.try_clone().unwrap());
        let mut wire = bunch(vec![block(BlockTarget::Actor, 82, bits(612))]);
        if malformed_block {
            let mut writer = BitWriter::new();
            let mut reader = wire.payload.reader();
            for _ in 0..wire.payload.bit_len() {
                writer.write_bit(reader.read_bit());
            }
            writer.write_bit(true); // truncated block header
            wire.payload = Payload::from_writer(writer);
        } else {
            wire = bunch(vec![
                block(BlockTarget::Actor, 82, bits(612)),
                block(BlockTarget::Actor, 78, bits(1)),
            ]);
        }
        tap(&mut sink).observe(&wire);
        assert!(rows(&mut output).is_empty());
        assert_eq!(sink.finish().malformed, 1);
        assert_eq!(sink.counts().incomplete, 1);
        let (first, last) = parts(&wire);
        let mut reassembled = PrivateRpc82Capture::from_file(tempfile::tempfile().unwrap());
        tap(&mut reassembled).observe(&first);
        tap(&mut reassembled).observe(&last);
        assert_eq!(reassembled.finish().observations, 0);
        assert_eq!(reassembled.counts().malformed, 1);
    }
}

fn parts(wire: &Bunch) -> (Bunch, Bunch) {
    let mut reader = wire.payload.reader();
    let mut a = BitWriter::new();
    for _ in 0..16 {
        a.write_bit(reader.read_bit());
    }
    let mut b = BitWriter::new();
    let remaining = reader.remaining();
    for _ in 0..remaining {
        b.write_bit(reader.read_bit());
    }
    let mut first = wire.clone();
    first.header.partial = true;
    first.header.partial_initial = true;
    first.payload = Payload::from_writer(a);
    let mut last = first.clone();
    last.header.partial_initial = false;
    last.header.partial_final = true;
    last.header.reliable_sequence_raw = Some(2);
    last.payload = Payload::from_writer(b);
    (first, last)
}

#[test]
fn fragmented_capture_waits_for_completion_and_reports_unfinished_assembly() {
    let mut output = tempfile::tempfile().unwrap();
    let mut sink = PrivateRpc82Capture::from_file(output.try_clone().unwrap());
    let (first, last) = parts(&bunch(vec![block(BlockTarget::Actor, 82, bits(612))]));
    tap(&mut sink).observe(&first);
    assert!(rows(&mut output).is_empty());
    assert_eq!(sink.counts().pending_assemblies, 1);
    tap(&mut sink).observe(&last);
    let rows = rows(&mut output);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["parts"], 2);
    assert_eq!(rows[0]["partial"], true);
    assert_eq!(rows[0]["reassembled"], true);
    assert_eq!(sink.finish().incomplete, 0);
    let mut unfinished = PrivateRpc82Capture::from_file(tempfile::tempfile().unwrap());
    tap(&mut unfinished).observe(&first);
    assert_eq!(unfinished.finish().incomplete, 1);
}

#[test]
fn observations_encoded_output_and_transient_assemblies_are_bounded() {
    let mut output = tempfile::tempfile().unwrap();
    let mut sink = PrivateRpc82Capture::from_file(output.try_clone().unwrap());
    let wire = bunch(vec![block(BlockTarget::Actor, 82, bits(65_000))]);
    for _ in 0..MAX_OBSERVATIONS + 5 {
        tap(&mut sink).observe(&wire);
    }
    let counts = sink.finish();
    assert!(counts.observations <= MAX_OBSERVATIONS as u64);
    assert_eq!(counts.output_bytes, output.metadata().unwrap().len());
    assert!(counts.output_bytes <= MAX_OUTPUT_BYTES as u64);
    assert!(counts.retained_payload_bits <= (MAX_OBSERVATIONS * MAX_FIELD_BYTES * 8) as u64);
    assert!(counts.dropped > 0);
    assert_eq!(counts.incomplete, 1);
    assert_eq!(rows(&mut output).len() as u64, counts.observations);
    let (first, _) = parts(&wire);
    let mut sink = PrivateRpc82Capture::from_file(tempfile::tempfile().unwrap());
    for connection in 1..=MAX_ASSEMBLIES as u32 + 1 {
        let mut tap = tap(&mut sink);
        tap.connection_ordinal = connection;
        tap.observe(&first);
    }
    assert_eq!(sink.counts().pending_assemblies, MAX_ASSEMBLIES as u64);
    assert_eq!(sink.counts().dropped, 1);
    let mut oversized = first.clone();
    oversized.payload = bits(MAX_BUNCH_BYTES * 8 + 1);
    tap(&mut sink).observe(&oversized);
    assert!(sink.counts().pending_assemblies < MAX_ASSEMBLIES as u64);

    let mut output = tempfile::tempfile().unwrap();
    let mut sink = PrivateRpc82Capture::from_file(output.try_clone().unwrap());
    let small = bunch(vec![
        block(BlockTarget::Actor, 82, bits(13));
        MAX_OBSERVATIONS + 1
    ]);
    tap(&mut sink).observe(&small);
    assert_eq!(sink.counts().observations, MAX_OBSERVATIONS as u64);
    assert_eq!(sink.counts().dropped, 1);
    assert_eq!(
        sink.counts().retained_payload_bits,
        MAX_OBSERVATIONS as u64 * 13
    );
    assert_eq!(rows(&mut output).len(), MAX_OBSERVATIONS);
    assert!(sink.counts().output_bytes <= MAX_OUTPUT_BYTES as u64);
}

#[test]
fn write_failure_is_incomplete_and_never_claims_a_record() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("readonly");
    std::fs::write(&path, []).unwrap();
    let mut sink = PrivateRpc82Capture::from_file(File::open(path).unwrap());
    let wire = bunch(vec![block(BlockTarget::Actor, 82, bits(612))]);
    tap(&mut sink).observe(&wire);
    tap(&mut sink).observe(&wire);
    let counts = sink.finish();
    assert_eq!(counts.observations, 0);
    assert_eq!(counts.write_errors, 1);
    assert_eq!(counts.output_bytes, 0);
    assert_eq!(counts.incomplete, 1);
}
#[test]
fn writer_selected_absent_payload_replays_without_source_completion_claim() {
    use crate::experimental::{
        release::ReleaseProfile,
        selected_visibility_replay::{ReplayProfile, replay},
        visibility::{VisibilityArchiveProfile, VisibilityRpc82Layout},
    };
    use std::io::{Seek, SeekFrom};

    let mut file = tempfile::tempfile().unwrap();
    let mut sink = PrivateRpc82Capture::from_file(file.try_clone().unwrap());
    let mut writer = BitWriter::new();
    writer.write_bit(false); // authored one-bit absent outer RPC parameter
    tap(&mut sink).observe(&bunch(vec![block(
        BlockTarget::Actor,
        82,
        Payload::from_writer(writer),
    )]));
    let final_counts = sink.finish();
    assert_eq!(final_counts.observations, 1);
    assert_eq!(final_counts.incomplete, 0);
    file.seek(SeekFrom::Start(0)).unwrap();
    let profile = ReplayProfile::qualify(
        &ReleaseProfile::release_3_0_34(),
        VisibilityArchiveProfile::NormalLittleEndianPackageMapCustom,
        VisibilityRpc82Layout::NormalSingleCustomStruct,
    )
    .unwrap();
    let summary = replay(file, &profile).unwrap();
    assert_eq!(summary.records, 1);
    assert_eq!(summary.accepted_absent, 1);
    assert_eq!(summary.rejected, 0);
    // Even a finalized authored writer does not grant this input API verified
    // capture provenance/completeness. Those receipts remain external.
    assert!(!summary.capture_complete_verified);
    assert!(!summary.provenance_verified);
    assert!(!summary.compatibility_proven);
}

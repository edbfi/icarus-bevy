use icarus_bevy::experimental::packet::{
    BitReader, BitWriter, Bunch, BunchHeader, BunchLimits, ChannelCloseReason, Name, PacketHeader,
    Payload, Receiver, read_bunch, read_packet_header, write_bunch, write_packet_header,
};
use icarus_bevy::experimental::sequencing::{
    MAX_CONSTRUCTED_PARTIAL_BUNCH_BYTES, MAX_QUEUED_RELIABLE, MAX_SINGLE_BUNCH_OVERHEAD_BITS,
    Notification, PacketNotify, PacketSequencer, PartialBunchReassembler, PartialReceive,
    ReliableChannel, ReliableReceive, is_newer, max_partial_bunch_size_bits,
    max_single_bunch_size_bits, next_notification, reconstruct_reliable_sequence,
    sequence_difference, signed_sequence_delta, split_outgoing_bunch, wire_reliable_sequence,
};

// Independently authored state sequences; no captured client or server traffic.

fn transmit(header: &PacketHeader, receiver: Receiver) -> PacketHeader {
    let mut writer = BitWriter::new();
    write_packet_header(&mut writer, header, None, receiver).unwrap();
    let mut reader = BitReader::new(writer.as_bytes(), writer.bit_len()).unwrap();
    let (read, info) = read_packet_header(&mut reader, receiver).unwrap();
    assert!(info.is_none() && reader.at_end());
    read
}

/// Write the next header, commit it and return the header with its packet ID.
fn send(sequencer: &mut PacketSequencer) -> (PacketHeader, u32) {
    let header = sequencer.header().unwrap();
    (header, sequencer.commit().unwrap())
}

/// Receive a header and enter it into the history as delivered.
fn deliver(sequencer: &mut PacketSequencer, header: &PacketHeader) -> Vec<Notification> {
    let received = sequencer.receive(header).unwrap().expect("accepted");
    sequencer.ack_seq(received.sequence, true).unwrap();
    received.notifications
}

fn notification(packet_id: u32, delivered: bool) -> Notification {
    Notification {
        packet_id,
        delivered,
    }
}

#[test]
fn sequence_arithmetic_wraps_at_fourteen_bits() {
    assert_eq!(sequence_difference(0, 0x3fff), 1);
    assert!(is_newer(0, 0x3fff));
    assert!(is_newer(0x1fff, 0));
    assert!(!is_newer(0x2000, 0));
    assert!(!is_newer(5, 5));
    assert_eq!(signed_sequence_delta(0x3fff, 0), -1);
    assert_eq!(signed_sequence_delta(0x2000, 0), -0x2000);
    assert_eq!(signed_sequence_delta(0x1fff, 0), 0x1fff);
}

#[test]
fn init_seeds_packet_ids_notify_and_reliable_sequences() {
    let sequencer = PacketSequencer::new(0x1234, 0x0405, 8).unwrap();
    let notify = sequencer.notify();
    assert_eq!(notify.in_seq(), 0x1233);
    assert_eq!(notify.in_ack_seq(), 0x1233);
    assert_eq!(notify.in_ack_seq_ack(), 0x1233);
    assert_eq!(notify.out_seq(), 0x0405);
    assert_eq!(notify.out_ack_seq(), 0x0404);
    assert_eq!(notify.history(), &[0; 8]);
    assert!(notify.sent_records().is_empty());
    assert_eq!(sequencer.in_packet_id(), 0x1233);
    assert_eq!(sequencer.out_packet_id(), 0x0405);
    assert_eq!(sequencer.last_notified_packet_id(), 0x0404);
    assert_eq!(sequencer.initial_reliable(), (0x234, 0x005));

    let zero = PacketSequencer::new(0, 0, 8).unwrap();
    assert_eq!(zero.notify().in_seq(), 0x3fff);
    assert_eq!(zero.notify().out_ack_seq(), 0x3fff);
    assert_eq!(zero.in_packet_id(), u32::MAX);
    assert_eq!(zero.last_notified_packet_id(), u32::MAX);
    assert!(PacketSequencer::new(0x4000, 0, 8).is_err());
    assert!(PacketNotify::new(0, 0, 0).is_err());
}

#[test]
fn ack_history_shifts_across_word_boundaries() {
    let mut notify = PacketNotify::new(0, 0, 8).unwrap();
    notify.ack_seq(1, true).unwrap();
    assert_eq!(notify.history()[0], 1);
    // 32 more steps carry bit 0 of word 0 into bit 0 of word 1; 33 was delivered.
    notify.ack_seq(33, true).unwrap();
    assert_eq!(notify.in_ack_seq(), 33);
    assert_eq!(notify.history()[..2], [1, 1]);
    notify.ack_seq(34, false).unwrap();
    assert_eq!(notify.history()[..2], [2, 2]);
    // Stale and equal sequences are ignored.
    notify.ack_seq(10, true).unwrap();
    notify.ack_seq(34, true).unwrap();
    assert_eq!(notify.in_ack_seq(), 34);
    assert_eq!(notify.history()[..2], [2, 2]);
    // Bits 1 and 33 reach the tops of words 6 and 7; one more step drops the
    // oldest and carries word 6 bit 31 into word 7 bit 0.
    notify.ack_seq(34 + 222, false).unwrap();
    assert_eq!(notify.history()[6..], [1 << 31, 1 << 31]);
    notify.ack_seq(34 + 223, false).unwrap();
    assert_eq!(notify.history()[6..], [0, 1]);
    assert!(notify.ack_seq(0x4000, true).is_err());
}

#[test]
fn header_word_count_clamps_and_rejects_spans_above_256() {
    let mut notify = PacketNotify::new(0, 7, 8).unwrap();
    assert_eq!(notify.history_word_count().unwrap(), 1);
    for (sequence, words) in [(1, 1), (32, 1), (33, 2), (224, 7), (225, 8), (256, 8)] {
        notify.ack_seq(sequence, true).unwrap();
        assert_eq!(
            notify.history_word_count().unwrap(),
            words,
            "span {sequence}"
        );
    }
    let header = notify.header().unwrap();
    assert_eq!(
        (header.sequence, header.acked_sequence, header.history.len()),
        (7, 256, 8)
    );
    assert_eq!(header.history[0] & 1, 1);
    notify.ack_seq(257, true).unwrap();
    assert!(notify.history_word_count().is_err());
    assert!(notify.header().is_err());
}

#[test]
fn commit_records_sent_packets_until_the_ring_is_full() {
    let mut notify = PacketNotify::new(0x10, 0x3fff, 2).unwrap();
    notify.ack_seq(0x12, true).unwrap();
    assert_eq!(notify.commit().unwrap(), 0x3fff);
    assert_eq!(notify.commit().unwrap(), 0);
    assert_eq!(notify.out_seq(), 1);
    let records: Vec<_> = notify
        .sent_records()
        .iter()
        .map(|record| (record.out_seq, record.in_ack_seq))
        .collect();
    assert_eq!(records, [(0x3fff, 0x12), (0, 0x12)]);
    assert!(notify.commit().is_err());
    assert_eq!(notify.out_seq(), 1);
}

#[test]
fn round_trip_naks_exactly_the_dropped_packet() {
    let mut client = PacketSequencer::new(0x0200, 0x1100, 16).unwrap();
    let mut server = PacketSequencer::new(0x1100, 0x0200, 16).unwrap();
    let mut ids = Vec::new();
    for index in 0..5 {
        let (header, id) = send(&mut client);
        ids.push(id);
        if index == 2 {
            continue;
        }
        let header = transmit(&header, Receiver::Server);
        let received = server.receive(&header).unwrap().expect("accepted");
        assert_eq!(received.skipped, u16::from(index == 3));
        assert_eq!(received.packet_id & 0x3fff, u32::from(received.sequence));
        assert!(received.notifications.is_empty());
        server.ack_seq(received.sequence, true).unwrap();
    }
    assert_eq!(ids, [0x1100, 0x1101, 0x1102, 0x1103, 0x1104]);
    assert_eq!(server.in_packet_id(), 0x1104);

    let (reply, _) = send(&mut server);
    assert_eq!(reply.acked_sequence, 0x1104);
    assert_eq!(reply.history, [0b11011]);
    let notifications = deliver(&mut client, &transmit(&reply, Receiver::Client));
    let expected: Vec<_> = ids
        .iter()
        .map(|id| notification(*id, *id != 0x1102))
        .collect();
    assert_eq!(notifications, expected);
    assert_eq!(client.notify().out_ack_seq(), 0x1104);
    assert_eq!(client.last_notified_packet_id(), 0x1104);
    // The client's record for its fifth packet acked nothing yet.
    assert_eq!(client.notify().in_ack_seq_ack(), 0x01ff);

    // The client's next packet acks the reply, so the server's acked span closes.
    let (header, _) = send(&mut client);
    let notifications = deliver(&mut server, &transmit(&header, Receiver::Server));
    assert_eq!(notifications, [notification(0x0200, true)]);
    assert_eq!(server.notify().in_ack_seq_ack(), 0x1104);
    assert_eq!(server.notify().history_word_count().unwrap(), 1);
}

#[test]
fn sequences_and_packet_ids_continue_across_the_wrap() {
    let mut client = PacketSequencer::new(0x3ffe, 0x3ffd, 8).unwrap();
    let mut server = PacketSequencer::new(0x3ffd, 0x3ffe, 8).unwrap();
    let mut ids = Vec::new();
    for _ in 0..4 {
        let (header, id) = send(&mut client);
        ids.push(id);
        assert!(deliver(&mut server, &transmit(&header, Receiver::Server)).is_empty());
    }
    assert_eq!(ids, [0x3ffd, 0x3ffe, 0x3fff, 0x4000]);
    assert_eq!(server.notify().in_seq(), 0);
    let (reply, _) = send(&mut server);
    assert_eq!((reply.sequence, reply.acked_sequence), (0x3ffe, 0));
    let notifications = deliver(&mut client, &transmit(&reply, Receiver::Client));
    let expected: Vec<_> = ids.iter().map(|id| notification(*id, true)).collect();
    assert_eq!(notifications, expected);
    assert_eq!(client.notify().out_ack_seq(), 0);
}

#[test]
fn stale_and_far_acks_are_ignored_and_stale_sequences_rejected() {
    let mut client = PacketSequencer::new(0x0100, 0x0800, 8).unwrap();
    send(&mut client);
    let out_ack = client.notify().out_ack_seq();
    for (sequence, acked) in [(0x0100, out_ack), (0x0101, (out_ack + 0x2000) & 0x3fff)] {
        let header = PacketHeader {
            sequence,
            acked_sequence: acked,
            history: vec![u32::MAX],
        };
        let received = client.receive(&header).unwrap().expect("accepted");
        assert!(received.notifications.is_empty());
        assert_eq!(client.notify().out_ack_seq(), out_ack);
        assert_eq!(client.notify().sent_records().len(), 1);
    }
    // Equal or older incoming sequences are rejected without any state change.
    for sequence in [0x0101, 0x0050] {
        let header = PacketHeader {
            sequence,
            acked_sequence: 0x0800,
            history: vec![u32::MAX],
        };
        assert_eq!(client.receive(&header).unwrap(), None);
    }
    assert_eq!(client.notify().in_seq(), 0x0101);
    assert_eq!(client.last_notified_packet_id(), 0x07ff);
}

#[test]
fn acks_beyond_the_newest_256_are_lost() {
    let mut client = PacketSequencer::new(0x0010, 0x0020, 400).unwrap();
    let ids: Vec<_> = (0..300).map(|_| send(&mut client).1).collect();
    let header = PacketHeader {
        sequence: 0x0010,
        acked_sequence: (0x0020 + 299) as u16,
        history: vec![u32::MAX; 8],
    };
    let received = client.receive(&header).unwrap().expect("accepted");
    let expected: Vec<_> = ids
        .iter()
        .enumerate()
        .map(|(index, id)| notification(*id, index >= 44))
        .collect();
    assert_eq!(received.notifications, expected);
    assert!(client.notify().sent_records().is_empty());
}

#[test]
fn missing_records_and_history_words_fall_back() {
    let mut client = PacketSequencer::new(0x0010, 0x0020, 8).unwrap();
    for _ in 0..2 {
        send(&mut client);
    }
    // Acking five sequences with two records: in_ack_seq_ack falls back to acked - 256.
    let header = PacketHeader {
        sequence: 0x0010,
        acked_sequence: 0x0024,
        history: vec![0b10101],
    };
    let received = client.receive(&header).unwrap().expect("accepted");
    let delivered: Vec<_> = received.notifications.iter().map(|n| n.delivered).collect();
    assert_eq!(delivered, [true, false, true, false, true]);
    assert_eq!(client.notify().in_ack_seq_ack(), 0x3f24);
    assert_eq!(client.notify().sent_records().len(), 2);

    // Unreceived history words read as zero: everything older than 32 is a nak.
    let mut client = PacketSequencer::new(0x0010, 0x0020, 64).unwrap();
    for _ in 0..40 {
        send(&mut client);
    }
    let header = PacketHeader {
        sequence: 0x0010,
        acked_sequence: 0x0020 + 39,
        history: vec![u32::MAX],
    };
    let received = client.receive(&header).unwrap().expect("accepted");
    let naks = received
        .notifications
        .iter()
        .filter(|n| !n.delivered)
        .count();
    assert_eq!(naks, 8);
    assert!(received.notifications[8..].iter().all(|n| n.delivered));
}

#[test]
fn packet_id_mismatch_closes_the_connection() {
    let mut last = 0x0001_3fff;
    assert_eq!(
        next_notification(&mut last, 0, false).unwrap(),
        notification(0x0001_4000, false)
    );
    assert_eq!(last, 0x0001_4000);
    assert!(next_notification(&mut last, 2, true).is_err());
    assert_eq!(last, 0x0001_4000);

    let mut notify = PacketNotify::new(0, 5, 8).unwrap();
    notify.commit().unwrap();
    let header = PacketHeader {
        sequence: 1,
        acked_sequence: 5,
        history: vec![1],
    };
    let mut wrong_last = 0x10;
    let result = notify.receive_header(&header, |sequence, delivered| {
        next_notification(&mut wrong_last, sequence, delivered).map(|_| ())
    });
    assert!(result.is_err());
}

#[test]
fn reliable_bunches_deliver_in_order_through_the_bunch_header() {
    let limits = BunchLimits {
        max_channels: 4,
        max_packet_bytes: 64,
        max_string_units: 16,
    };
    let sequencer = PacketSequencer::new(0x07fe, 0x0003, 8).unwrap();
    let mut channel = sequencer.new_channel::<&str>();
    assert_eq!(channel.in_reliable(), 0x3fe);
    for (sequence, payload) in [(0x3ff, "a"), (0x400, "b"), (0x401, "c")] {
        let mut writer = BitWriter::new();
        let bunch = Bunch {
            header: BunchHeader {
                control: false,
                open: false,
                close: false,
                close_reason: None,
                paused: false,
                reliable: true,
                channel_index: 1,
                has_package_map_exports: false,
                has_must_be_mapped_guids: false,
                partial: false,
                reliable_sequence_raw: Some(wire_reliable_sequence(sequence)),
                partial_initial: false,
                partial_final: false,
                channel_name: Some(Name::Hardcoded(255)),
            },
            payload: Payload::from_writer(BitWriter::new()),
        };
        write_bunch(&mut writer, &bunch, &limits).unwrap();
        let mut reader = BitReader::new(writer.as_bytes(), writer.bit_len()).unwrap();
        let raw = read_bunch(&mut reader, &limits)
            .unwrap()
            .header
            .reliable_sequence_raw
            .unwrap();
        assert_eq!(
            channel.receive_reliable(raw, payload).unwrap(),
            ReliableReceive::Delivered(vec![payload])
        );
        assert_eq!(channel.in_reliable(), sequence);
    }
    assert_eq!(reconstruct_reliable_sequence(0x3ff, 0x401).unwrap(), 0x3ff);
    assert_eq!(reconstruct_reliable_sequence(0x200, 0).unwrap(), -0x200);
    assert_eq!(reconstruct_reliable_sequence(0x1ff, 0).unwrap(), 0x1ff);
    assert!(reconstruct_reliable_sequence(0x400, 0).is_err());
}

#[test]
fn out_of_order_reliable_bunches_queue_drain_and_drop_duplicates() {
    let mut channel = ReliableChannel::new(0x3fd, 0);
    assert_eq!(
        channel.receive_reliable(0x000, 3).unwrap(),
        ReliableReceive::Queued
    );
    assert_eq!(
        channel.receive_reliable(0x3ff, 2).unwrap(),
        ReliableReceive::Queued
    );
    assert_eq!(
        channel.receive_reliable(0x000, 30).unwrap(),
        ReliableReceive::Duplicate
    );
    assert_eq!(
        channel.receive_reliable(0x002, 5).unwrap(),
        ReliableReceive::Queued
    );
    assert_eq!(channel.queued_len(), 3);
    assert_eq!(
        channel.receive_reliable(0x3fe, 1).unwrap(),
        ReliableReceive::Delivered(vec![1, 2, 3])
    );
    assert_eq!(channel.in_reliable(), 0x400);
    assert_eq!(channel.queued_len(), 1);
    // The current, an older and a half-range-behind value are duplicates.
    for raw in [0x000, 0x3fe, 0x200] {
        assert_eq!(
            channel.receive_reliable(raw, 0).unwrap(),
            ReliableReceive::Duplicate
        );
    }
    assert_eq!(
        channel.receive_reliable(0x001, 4).unwrap(),
        ReliableReceive::Delivered(vec![4, 5])
    );
}

#[test]
fn queuing_a_256th_reliable_bunch_fails() {
    let mut channel = ReliableChannel::new(10, 0);
    for offset in 2..2 + MAX_QUEUED_RELIABLE as u32 {
        assert_eq!(
            channel.receive_reliable(10 + offset, offset).unwrap(),
            ReliableReceive::Queued
        );
    }
    assert_eq!(channel.queued_len(), 255);
    assert!(channel.receive_reliable(10 + 300, 0).is_err());
    let ReliableReceive::Delivered(delivered) = channel.receive_reliable(11, 1).unwrap() else {
        panic!("expected delivery");
    };
    assert_eq!(delivered, (1..=256).collect::<Vec<_>>());
    assert_eq!(channel.in_reliable(), 266);
}

#[test]
fn naks_select_unacked_bunches_and_acks_release_from_the_head() {
    let mut channel = ReliableChannel::new(0, 0x3fe);
    let first = channel.send_reliable(10, "first").unwrap();
    let second = channel.send_reliable(10, "second").unwrap();
    let third = channel.send_reliable(11, "third").unwrap();
    assert_eq!((first, second, third), (0x3ff, 0x400, 0x401));
    assert_eq!(wire_reliable_sequence(second), 0);

    assert_eq!(channel.lost(10), [first, second]);
    for sequence in channel.lost(10) {
        assert_eq!(channel.reassign(sequence, 12).unwrap().packet_id, 12);
    }
    assert!(channel.lost(10).is_empty());
    assert!(channel.reassign(0x999, 13).is_err());

    // Acking the third leaves it queued behind the unacked head.
    assert!(channel.acked(11).is_empty());
    assert!(channel.lost(11).is_empty());
    assert_eq!(channel.out_bunches().len(), 3);
    let released: Vec<_> = channel
        .acked(12)
        .into_iter()
        .map(|bunch| (bunch.reliable_sequence, bunch.payload))
        .collect();
    assert_eq!(
        released,
        [(first, "first"), (second, "second"), (third, "third")]
    );
    assert!(channel.out_bunches().is_empty());
}

fn partial_header(reliable: bool, initial: bool, final_part: bool) -> BunchHeader {
    BunchHeader {
        control: false,
        open: false,
        close: false,
        close_reason: None,
        paused: false,
        reliable,
        channel_index: 3,
        has_package_map_exports: false,
        has_must_be_mapped_guids: false,
        partial: true,
        reliable_sequence_raw: reliable.then_some(1),
        partial_initial: initial,
        partial_final: final_part,
        channel_name: reliable.then_some(Name::Hardcoded(102)),
    }
}

fn byte_payload(bytes: &[u8], extra_bits: Option<(u32, u32)>) -> Payload {
    let mut writer = BitWriter::new();
    writer.write_bytes(bytes);
    if let Some((value, bits)) = extra_bits {
        writer.write_bits(value, bits);
    }
    Payload::from_writer(writer)
}

#[test]
fn channel_close_reasons_classify_and_detect_dormancy() {
    for (raw, expected, dormant) in [
        (0, ChannelCloseReason::Destroyed, false),
        (1, ChannelCloseReason::Dormancy, true),
        (2, ChannelCloseReason::LevelUnloaded, false),
        (3, ChannelCloseReason::Saving, false),
        (4, ChannelCloseReason::TearOff, false),
        (5, ChannelCloseReason::Custom(5), false),
        (14, ChannelCloseReason::Custom(14), false),
    ] {
        let parsed = ChannelCloseReason::from_raw(raw).unwrap();
        assert_eq!(parsed, expected);
        assert_eq!(parsed.as_raw(), raw);
        assert_eq!(parsed.is_dormant(), dormant);
    }
    assert!(ChannelCloseReason::from_raw(15).is_err());

    let mut header = partial_header(true, false, true);
    assert_eq!(header.parsed_close_reason().unwrap(), None);
    header.close = true;
    header.close_reason = Some(1);
    assert_eq!(
        header.parsed_close_reason().unwrap(),
        Some(ChannelCloseReason::Dormancy)
    );
}

#[test]
fn partial_bunch_reassembler_handles_exports_alignment_and_final_flags() {
    let mut reassembler = PartialBunchReassembler::new();
    let empty = byte_payload(&[], None);
    let part1 = byte_payload(&[0xaa, 0xbb], None);
    let part2 = byte_payload(&[0xcc], Some((0b101, 3)));

    // Initial export-only part: 0 leftover bits after exports, not byte-aligned in raw bunch.
    let mut init_export = partial_header(true, true, false);
    init_export.open = true;
    init_export.control = true;
    init_export.has_package_map_exports = true;
    assert_eq!(
        reassembler
            .receive_part(100, &init_export, &empty, None)
            .unwrap(),
        PartialReceive::Incomplete {
            parts: 1,
            replaced_unreliable: false,
        }
    );
    assert!(reassembler.in_progress());
    assert_eq!(reassembler.parts_in_progress(), Some(1));

    // Unreliable initial arriving during a reliable assembly is ignored.
    let unrel_init = partial_header(false, true, false);
    assert_eq!(
        reassembler
            .receive_part(50, &unrel_init, &part1, None)
            .unwrap(),
        PartialReceive::IgnoredUnreliableInitial
    );
    assert_eq!(reassembler.parts_in_progress(), Some(1));

    // Reliable initial arriving during a reliable assembly is an error.
    let rel_init = partial_header(true, true, false);
    assert!(
        reassembler
            .receive_part(101, &rel_init, &part1, None)
            .is_err()
    );

    // Non-final continuation with byte-aligned payload.
    let mid = partial_header(true, false, false);
    assert_eq!(
        reassembler.receive_part(101, &mid, &part1, None).unwrap(),
        PartialReceive::Incomplete {
            parts: 2,
            replaced_unreliable: false,
        }
    );

    // Final part carries unaligned trailing bits, close=true (Dormancy), and must_be_mapped.
    let mut fin = partial_header(true, false, true);
    fin.close = true;
    fin.close_reason = Some(1);
    fin.has_must_be_mapped_guids = true;
    let PartialReceive::Complete {
        bunch,
        parts,
        replaced_unreliable,
    } = reassembler.receive_part(102, &fin, &part2, None).unwrap()
    else {
        panic!("expected complete bunch");
    };
    assert_eq!(parts, 3);
    assert!(!replaced_unreliable);
    assert!(!reassembler.in_progress());
    assert!(bunch.header.open);
    assert!(bunch.header.close);
    assert!(bunch.header.control);
    assert_eq!(
        bunch.header.parsed_close_reason().unwrap(),
        Some(ChannelCloseReason::Dormancy)
    );
    assert!(bunch.header.has_must_be_mapped_guids);
    assert!(!bunch.header.partial);
    assert!(!bunch.header.has_package_map_exports);
    assert_eq!(bunch.payload.bit_len(), 27);
    assert_eq!(bunch.payload.bytes(), &[0xaa, 0xbb, 0xcc, 0b101]);

    // Server cap (MAX_CONSTRUCTED_PARTIAL_BUNCH_BYTES = 65_536) is enforced when supplied.
    let mut capped = PartialBunchReassembler::new();
    let init = partial_header(true, true, false);
    assert!(
        capped
            .receive_part(
                1,
                &init,
                &byte_payload(&[0u8; 32], None),
                Some(MAX_CONSTRUCTED_PARTIAL_BUNCH_BYTES)
            )
            .is_ok()
    );
    assert!(
        capped
            .receive_part(2, &fin, &byte_payload(&[0u8; 32], None), Some(48))
            .is_err()
    );
}

#[test]
fn send_bunch_sizing_and_outgoing_splitter_round_trip() {
    assert_eq!(MAX_SINGLE_BUNCH_OVERHEAD_BITS, 565);
    assert_eq!(max_single_bunch_size_bits(1024, 2).unwrap(), 7_625);
    assert_eq!(max_partial_bunch_size_bits(1024, 2).unwrap(), 7_624);
    assert_eq!(max_partial_bunch_size_bits(1024, 2).unwrap() / 8, 953);
    assert!(max_single_bunch_size_bits(70, 0).is_err());

    let max_single = max_single_bunch_size_bits(1024, 2).unwrap();
    let base_header = BunchHeader {
        control: true,
        open: true,
        close: true,
        close_reason: Some(1),
        paused: false,
        reliable: true,
        channel_index: 3,
        has_package_map_exports: false,
        has_must_be_mapped_guids: true,
        partial: false,
        reliable_sequence_raw: Some(50),
        partial_initial: false,
        partial_final: false,
        channel_name: Some(Name::Hardcoded(102)),
    };

    // Single bunch without exports stays un-partial.
    let small = Bunch {
        header: base_header.clone(),
        payload: byte_payload(&[0x11, 0x22], Some((0b101, 3))),
    };
    let single_out = split_outgoing_bunch(&small, &[], max_single, Some(65_536)).unwrap();
    assert_eq!(single_out.len(), 1);
    assert!(!single_out[0].header.partial);
    assert!(single_out[0].header.open);
    assert!(single_out[0].header.close);

    // Export bunch + oversized payload splits into export initial + 953-byte chunks + final tail,
    // and round-trips through PartialBunchReassembler.
    let export_payload = byte_payload(&[0xde, 0xad], Some((0b11, 2)));
    let body_bytes: Vec<u8> = (0..1_910).map(|i| (i % 251) as u8).collect();
    let large = Bunch {
        header: base_header,
        payload: byte_payload(&body_bytes, Some((0b11011, 5))),
    };
    let parts = split_outgoing_bunch(
        &large,
        std::slice::from_ref(&export_payload),
        max_single,
        Some(MAX_CONSTRUCTED_PARTIAL_BUNCH_BYTES),
    )
    .unwrap();
    assert_eq!(parts.len(), 4);
    assert!(parts[0].header.partial && parts[0].header.partial_initial);
    assert!(parts[0].header.open && !parts[0].header.close);
    assert!(parts[0].header.has_package_map_exports && !parts[0].header.has_must_be_mapped_guids);
    assert!(
        parts[1].header.partial
            && !parts[1].header.partial_initial
            && !parts[1].header.partial_final
    );
    assert!(
        !parts[1].header.open && !parts[1].header.close && parts[1].header.has_must_be_mapped_guids
    );
    assert_eq!(parts[1].payload.bit_len(), 7_624);
    assert_eq!(parts[2].payload.bit_len(), 7_624);
    assert!(parts[3].header.partial && parts[3].header.partial_final);
    assert!(!parts[3].header.open && parts[3].header.close);
    assert_eq!(parts[3].header.close_reason, Some(1));

    let mut reassembler = PartialBunchReassembler::new();
    for (idx, part) in parts.iter().enumerate() {
        let stripped = if part.header.has_package_map_exports {
            byte_payload(&[], None)
        } else {
            part.payload.clone()
        };
        let res = reassembler
            .receive_part(50 + idx as i64, &part.header, &stripped, None)
            .unwrap();
        if idx + 1 < parts.len() {
            assert!(matches!(res, PartialReceive::Incomplete { .. }));
        } else {
            let PartialReceive::Complete {
                bunch,
                parts: count,
                ..
            } = res
            else {
                panic!("expected completion");
            };
            assert_eq!(count, 4);
            assert!(bunch.header.open && bunch.header.close);
            assert_eq!(bunch.header.close_reason, Some(1));
            assert!(bunch.header.has_must_be_mapped_guids);
            assert_eq!(bunch.payload, large.payload);
        }
    }

    // Exceeding max_constructed_bytes fails closed.
    assert!(split_outgoing_bunch(&large, &[], max_single, Some(1024)).is_err());
}

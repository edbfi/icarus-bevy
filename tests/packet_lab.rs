use icarus_bevy::experimental::archive::ByteOrder;
use icarus_bevy::experimental::control_messages::{
    ArchiveText, ControlCodec, ControlMessage, Decoded, HELLO, JOIN, NETSPEED,
    encode_control_message, read_control_messages,
};
use icarus_bevy::experimental::handshake::initial_request;
use icarus_bevy::experimental::packet::{
    BitWriter, Bunch, BunchHeader, BunchLimits, HandshakePacket, Name, Packet, PacketConfig,
    Payload, Receiver, read_packet, write_packet,
};
use icarus_bevy::experimental::packet_lab::{
    DeliveredSummary, LabPeer, MAX_PENDING_RELIABLE, PacketDisposition, PacketLabConfig,
    client_cookie_seeds, server_cookie_seeds,
};
use icarus_bevy::experimental::sequencing::{
    MAX_CONSTRUCTED_PARTIAL_BUNCH_BYTES, Notification, PacketSequencer, PartialBunchReassembler,
    PartialReceive, ReliableChannel, ReliableReceive, SendCapacityError, wire_reliable_sequence,
};

// Independently authored synthetic client built from the experimental codec and
// the client sequencing model; no captured client or server traffic.

const LIMITS: BunchLimits = BunchLimits {
    max_channels: 16,
    max_packet_bytes: 1024,
    max_string_units: 64,
};
const SENTINEL: &str = "SENTINEL-TOKEN-7f3a";
/// Word 0 = 0x1234, word 1 = 0xc405 (masked to 0x0405).
const COOKIE: [u8; 20] = [
    0x34, 0x12, 0x05, 0xc4, 9, 8, 7, 6, 5, 4, 3, 2, 1, 0, 0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff,
];

fn to_server() -> PacketConfig {
    PacketConfig {
        magic_bits: 0,
        receiver: Receiver::Server,
        limits: LIMITS,
    }
}

fn to_client() -> PacketConfig {
    PacketConfig {
        receiver: Receiver::Client,
        ..to_server()
    }
}

fn peer() -> LabPeer {
    LabPeer::from_cookie(&COOKIE, PacketLabConfig::new(LIMITS)).unwrap()
}

fn hello() -> ControlMessage {
    ControlMessage::Hello {
        first_field: 1,
        version: 0x0102_0304,
        token: ArchiveText::text(SENTINEL).unwrap(),
    }
}

fn payload(message: &ControlMessage) -> Payload {
    let mut writer = BitWriter::new();
    writer.write_bytes(&encode_control_message(message, &ControlCodec::default()).unwrap());
    Payload::from_writer(writer)
}

fn control_bunch(reliable_raw: Option<u32>, open: bool, message: &ControlMessage) -> Bunch {
    Bunch {
        header: BunchHeader {
            control: open,
            open,
            close: false,
            close_reason: None,
            paused: false,
            reliable: reliable_raw.is_some(),
            channel_index: 0,
            has_package_map_exports: false,
            has_must_be_mapped_guids: false,
            partial: false,
            reliable_sequence_raw: reliable_raw,
            partial_initial: false,
            partial_final: false,
            channel_name: (open || reliable_raw.is_some()).then_some(Name::Hardcoded(255)),
        },
        payload: payload(message),
    }
}

struct Client {
    sequencer: PacketSequencer,
    control: ReliableChannel<ControlMessage>,
}

impl Client {
    fn new() -> Self {
        let (incoming, outgoing) = client_cookie_seeds(&COOKIE);
        let sequencer = PacketSequencer::new(incoming, outgoing, 64).unwrap();
        let control = sequencer.new_channel();
        Self { sequencer, control }
    }

    /// Commit a datagram carrying `bunches`; returns it and its packet ID.
    fn send(&mut self, bunches: Vec<Bunch>) -> (Vec<u8>, u32) {
        let packet = Packet {
            magic: 0,
            header: self.sequencer.header().unwrap(),
            info: None,
            bunches,
        };
        let bytes = write_packet(&packet, &to_server()).unwrap();
        (bytes, self.sequencer.commit().unwrap())
    }

    /// Send `message` as a new reliable control bunch.
    fn send_reliable(&mut self, message: ControlMessage, open: bool) -> (Vec<u8>, u32) {
        let packet_id = self.sequencer.out_packet_id();
        let sequence = self
            .control
            .send_reliable(packet_id, message.clone())
            .unwrap();
        let bunch = control_bunch(Some(wire_reliable_sequence(sequence)), open, &message);
        self.send(vec![bunch])
    }

    /// Read a lab datagram, deliver it and return the notifications.
    fn receive(&mut self, datagram: &[u8]) -> Vec<Notification> {
        let packet = read_packet(datagram, &to_client()).unwrap();
        assert!(packet.bunches.is_empty());
        let info = packet.info.expect("lab writes packet info");
        assert_eq!((info.jitter_clock, info.has_frame_time), (0, false));
        let received = self.sequencer.receive(&packet.header).unwrap().unwrap();
        self.sequencer.ack_seq(received.sequence, true).unwrap();
        received.notifications
    }
}

fn delivered_ids(summary: &[DeliveredSummary]) -> Vec<u8> {
    summary.iter().map(DeliveredSummary::id).collect()
}

#[test]
fn seeds_mirror_the_client_cookie_words() {
    assert_eq!(client_cookie_seeds(&COOKIE), (0x1234, 0x0405));
    assert_eq!(server_cookie_seeds(&COOKIE), (0x0405, 0x1234));
    let peer = peer();
    let notify = peer.sequencer().notify();
    assert_eq!(notify.in_seq(), 0x0404);
    assert_eq!(notify.out_seq(), 0x1234);
    assert_eq!(notify.out_ack_seq(), 0x1233);
    assert_eq!(peer.sequencer().initial_reliable(), (0x005, 0x234));
    assert_eq!(peer.control_in_reliable(), 0x005);
    // The client's outgoing state matches the peer's incoming state and back.
    let client = Client::new();
    assert_eq!(client.sequencer.notify().out_seq(), notify.in_seq() + 1);
    assert_eq!(client.sequencer.notify().in_seq(), notify.out_seq() - 1);
    assert_eq!(client.control.out_reliable(), peer.control_in_reliable());
}

#[test]
fn config_is_validated() {
    let mut config = PacketLabConfig::new(LIMITS);
    config.packet.receiver = Receiver::Client;
    assert!(LabPeer::from_cookie(&COOKIE, config).is_err());
    let mut config = PacketLabConfig::new(LIMITS);
    config.max_bunches_per_packet = 0;
    assert!(LabPeer::from_cookie(&COOKIE, config).is_err());
    let mut config = PacketLabConfig::new(LIMITS);
    config.control_channel_index = 16;
    assert!(LabPeer::from_cookie(&COOKIE, config).is_err());
    let mut config = PacketLabConfig::new(LIMITS);
    config.sent_capacity = 0;
    assert!(LabPeer::from_cookie(&COOKIE, config).is_err());
}

#[test]
fn reliable_hello_is_decoded_and_acked() {
    let mut peer = peer();
    let mut client = Client::new();
    let (datagram, packet_id) = client.send_reliable(hello(), true);
    let received = peer.receive(&datagram).unwrap();
    let summary = received.summary();
    assert_eq!(summary.sequence, 0x0405);
    assert_eq!(
        summary.disposition,
        PacketDisposition::Accepted {
            packet_id,
            skipped: 0
        }
    );
    assert_eq!(delivered_ids(&summary.delivered), [HELLO]);
    assert_eq!((summary.control_bunches, summary.ignored_bunches), (1, 0));
    assert!(summary.acked.is_empty() && summary.naked.is_empty());
    assert!(!summary.game_connection_established);
    let [Decoded::Message(ControlMessage::Hello { token, .. })] = received.messages() else {
        panic!("expected one Hello");
    };
    assert_eq!(String::from_utf16(token.units()).unwrap(), SENTINEL);
    assert_eq!(peer.control_in_reliable(), 0x006);
    assert_eq!(peer.sequencer().notify().in_ack_seq(), 0x0405);
    assert_eq!(peer.sequencer().notify().history()[0], 1);
}

#[test]
fn hello_first_field_persists_connection_byte_order_across_bunches() {
    let mut peer = peer();
    assert!(peer.header_inspection_pending());
    assert!(matches!(peer.connection_byte_order(), ByteOrder::Little));
    let mut client = Client::new();
    let big_hello = ControlMessage::Hello {
        first_field: 0,
        version: 0x0102_0304,
        token: ArchiveText::text("").unwrap(),
    };
    let (first_datagram, _) = client.send_reliable(big_hello, true);
    peer.receive(&first_datagram).unwrap();
    assert!(!peer.header_inspection_pending());
    assert!(matches!(peer.connection_byte_order(), ByteOrder::Big));

    let big_codec = ControlCodec {
        order: ByteOrder::Big,
        ..ControlCodec::default()
    };
    let packet_id = client.sequencer.out_packet_id();
    let netspeed = ControlMessage::Netspeed { rate: 100_000 };
    let seq = client
        .control
        .send_reliable(packet_id, netspeed.clone())
        .unwrap();
    let mut writer = BitWriter::new();
    writer.write_bytes(&encode_control_message(&netspeed, &big_codec).unwrap());
    let bunch = Bunch {
        header: BunchHeader {
            control: false,
            open: false,
            close: false,
            close_reason: None,
            paused: false,
            reliable: true,
            channel_index: 0,
            has_package_map_exports: false,
            has_must_be_mapped_guids: false,
            partial: false,
            reliable_sequence_raw: Some(wire_reliable_sequence(seq)),
            partial_initial: false,
            partial_final: false,
            channel_name: Some(Name::Hardcoded(255)),
        },
        payload: Payload::from_writer(writer),
    };
    let (second_datagram, _) = client.send(vec![bunch]);
    let received = peer.receive(&second_datagram).unwrap();
    assert_eq!(received.messages(), &[Decoded::Message(netspeed)]);

    let outgoing = peer
        .reliable_control_packet(ControlMessage::Upgrade { version: 7 })
        .unwrap();
    let packet = read_packet(&outgoing, &to_client()).unwrap();
    let decoded =
        read_control_messages(&mut packet.bunches[0].payload.reader(), &big_codec).unwrap();
    assert_eq!(
        decoded,
        [Decoded::Message(ControlMessage::Upgrade { version: 7 })]
    );
}

#[test]
fn ack_datagram_round_trips_and_acks_the_client_packet() {
    let mut peer = peer();
    let mut client = Client::new();
    let (datagram, packet_id) = client.send_reliable(hello(), true);
    peer.receive(&datagram).unwrap();
    let out_id = peer.sequencer().out_packet_id();
    let ack = peer.ack_packet().unwrap();
    assert_eq!(peer.sequencer().out_packet_id(), out_id + 1);
    assert_eq!(peer.sequencer().notify().sent_records().len(), 1);
    let packet = read_packet(&ack, &to_client()).unwrap();
    assert_eq!(packet.header.sequence, 0x1234);
    assert_eq!(packet.header.acked_sequence, 0x0405);
    assert_eq!(packet.header.history, [1]);
    assert_eq!(write_packet(&packet, &to_client()).unwrap(), ack);
    assert_eq!(
        client.receive(&ack),
        [Notification {
            packet_id,
            delivered: true
        }]
    );
    assert_eq!(client.control.acked(packet_id).len(), 1);
    // The client's next packet acknowledges the lab's ack packet.
    let (next, _) = client.send(Vec::new());
    let summary = peer.receive(&next).unwrap().summary().clone();
    assert_eq!(summary.acked, [out_id]);
    assert!(peer.sequencer().notify().sent_records().is_empty());
}

#[test]
fn dropped_client_packet_is_naked_after_the_next_ack() {
    let mut peer = peer();
    let mut client = Client::new();
    let (first, first_id) = client.send(Vec::new());
    peer.receive(&first).unwrap();
    client.receive(&peer.ack_packet().unwrap());
    let (_dropped, dropped_id) = client.send(Vec::new());
    let (third, third_id) = client.send(Vec::new());
    let summary = peer.receive(&third).unwrap().summary().clone();
    assert_eq!(
        summary.disposition,
        PacketDisposition::Accepted {
            packet_id: third_id,
            skipped: 1
        }
    );
    assert_eq!(
        client.receive(&peer.ack_packet().unwrap()),
        [
            Notification {
                packet_id: dropped_id,
                delivered: false
            },
            Notification {
                packet_id: third_id,
                delivered: true
            }
        ]
    );
    assert_eq!(first_id + 1, dropped_id);
}

#[test]
fn duplicate_and_stale_packets_are_rejected_without_state_change() {
    let mut peer = peer();
    let mut client = Client::new();
    let (first, _) = client.send_reliable(hello(), true);
    let (second, _) = client.send(Vec::new());
    peer.receive(&first).unwrap();
    peer.receive(&second).unwrap();
    let before = format!("{peer:?}");
    let ids = (
        peer.sequencer().in_packet_id(),
        *peer.sequencer().notify().history(),
    );
    for datagram in [&second, &first] {
        let received = peer.receive(datagram).unwrap();
        let summary = received.summary();
        assert_eq!(summary.disposition, PacketDisposition::Rejected);
        assert!(summary.delivered.is_empty() && received.messages().is_empty());
    }
    assert_eq!(format!("{peer:?}"), before);
    assert_eq!(
        (
            peer.sequencer().in_packet_id(),
            *peer.sequencer().notify().history()
        ),
        ids
    );
    assert_eq!(peer.control_in_reliable(), 0x006);
}

#[test]
fn out_of_order_reliable_control_is_delivered_in_order() {
    let mut peer = peer();
    let mut client = Client::new();
    let (_lost, lost_id) = client.send_reliable(hello(), true);
    let (second, _) = client.send_reliable(ControlMessage::Netspeed { rate: 100_000 }, false);
    let summary = peer.receive(&second).unwrap().summary().clone();
    assert!(summary.delivered.is_empty());
    assert_eq!(summary.queued_reliable, 1);
    assert_eq!(peer.queued_reliable(), 1);
    // The lab's ack naks the lost packet; the client resends its bunch.
    let notifications = client.receive(&peer.ack_packet().unwrap());
    assert_eq!(notifications[0].packet_id, lost_id);
    assert!(!notifications[0].delivered && notifications[1].delivered);
    let lost = client.control.lost(lost_id);
    assert_eq!(lost.len(), 1);
    let resend_id = client.sequencer.out_packet_id();
    let resent = client.control.reassign(lost[0], resend_id).unwrap();
    let bunch = control_bunch(
        Some(wire_reliable_sequence(resent.reliable_sequence)),
        true,
        &resent.payload.clone(),
    );
    let (third, _) = client.send(vec![bunch]);
    let received = peer.receive(&third).unwrap();
    assert_eq!(
        delivered_ids(&received.summary().delivered),
        [HELLO, NETSPEED]
    );
    assert_eq!(received.summary().queued_reliable, 0);
    assert_eq!(peer.control_in_reliable(), 0x007);
    // A further resend of either bunch is a duplicate.
    let (fourth, _) = client.send(vec![control_bunch(
        Some(wire_reliable_sequence(0x006)),
        true,
        &hello(),
    )]);
    let summary = peer.receive(&fourth).unwrap().summary().clone();
    assert_eq!(summary.duplicate_reliable, 1);
    assert!(summary.delivered.is_empty());
}

#[test]
fn unreliable_and_non_control_bunches() {
    let mut peer = peer();
    let mut client = Client::new();
    let mut other = control_bunch(None, false, &ControlMessage::Join);
    other.header.channel_index = 3;
    let unreliable = control_bunch(None, false, &ControlMessage::Join);
    let (datagram, _) = client.send(vec![other, unreliable]);
    let summary = peer.receive(&datagram).unwrap().summary().clone();
    assert_eq!((summary.control_bunches, summary.ignored_bunches), (1, 1));
    assert_eq!(delivered_ids(&summary.delivered), [JOIN]);
    assert_eq!(peer.control_in_reliable(), 0x005);
}

#[test]
fn malformed_datagrams_fail_closed_without_state_change() {
    let mut peer = peer();
    let mut client = Client::new();
    let before = format!("{peer:?}");
    assert!(peer.receive(&[]).is_err());
    assert!(peer.receive(&[0x10, 0]).is_err());
    assert!(peer.receive(&[0x01]).is_err());
    let error = peer.receive(&initial_request()).unwrap_err();
    assert!(error.downcast_ref::<HandshakePacket>().is_some());
    let (valid, _) = client.send(Vec::new());
    assert!(peer.receive(&valid[..valid.len() - 1]).is_err());
    assert!(peer.receive(&vec![0xff; 1025]).is_err());
    // Too many bunches.
    let mut config = PacketLabConfig::new(LIMITS);
    config.max_bunches_per_packet = 2;
    let mut bounded = LabPeer::from_cookie(&COOKIE, config).unwrap();
    let join = || control_bunch(None, false, &ControlMessage::Join);
    let (three, _) = Client::new().send(vec![join(), join(), join()]);
    assert!(bounded.receive(&three).is_err());
    // Unsupported control forms.
    let mut partial = join();
    partial.header.partial = true;
    let mut exports = join();
    exports.header.has_package_map_exports = true;
    let mut truncated = join();
    truncated.payload = payload(&hello());
    let mut writer = BitWriter::new();
    writer.write_bytes(&truncated.payload.bytes()[..6]);
    truncated.payload = Payload::from_writer(writer);
    for bunch in [partial, exports, truncated] {
        let mut probe = Client::new();
        let (datagram, _) = probe.send(vec![bunch]);
        assert!(peer.receive(&datagram).is_err());
    }
    assert_eq!(format!("{peer:?}"), before);
    assert!(!peer.is_failed());
    assert!(peer.receive(&valid).is_ok());
}

#[test]
fn summaries_and_debug_never_contain_token_text() {
    let mut peer = peer();
    let mut client = Client::new();
    let (_lost, _) = client.send_reliable(hello(), true);
    // Queue a Hello carrying the sentinel behind a missing reliable sequence.
    let packet_id = client.sequencer.out_packet_id();
    let sequence = client.control.send_reliable(packet_id, hello()).unwrap();
    let (queued, _) = client.send(vec![control_bunch(
        Some(wire_reliable_sequence(sequence)),
        false,
        &hello(),
    )]);
    let queued = peer.receive(&queued).unwrap();
    assert_eq!(peer.queued_reliable(), 1);
    let resend = control_bunch(Some(wire_reliable_sequence(0x006)), true, &hello());
    let (released, _) = client.send(vec![resend]);
    let released = peer.receive(&released).unwrap();
    assert_eq!(released.messages().len(), 2);
    for text in [
        format!("{queued:?}"),
        format!("{released:?}"),
        format!("{:?}", released.summary()),
        format!("{peer:?}"),
        serde_json::to_string(released.summary()).unwrap(),
    ] {
        assert!(!text.contains(SENTINEL), "{text}");
        // Numeric message fields stay out of summaries too.
        assert!(!text.contains("16909060"), "{text}");
    }
    // The decoded messages keep their contents but hide strings in Debug.
    assert!(!format!("{:?}", released.messages()).contains(SENTINEL));
}

#[test]
fn reliable_queue_overflow_fails_the_peer_closed() {
    let mut peer = peer();
    let mut client = Client::new();
    let (_lost, _) = client.send_reliable(ControlMessage::Join, true);
    let mut queued = 0;
    let error = loop {
        let packet_id = client.sequencer.out_packet_id();
        let bunches = (0..32)
            .map(|_| {
                let sequence = client
                    .control
                    .send_reliable(packet_id, ControlMessage::Join)
                    .unwrap();
                control_bunch(
                    Some(wire_reliable_sequence(sequence)),
                    false,
                    &ControlMessage::Join,
                )
            })
            .collect();
        let (datagram, _) = client.send(bunches);
        match peer.receive(&datagram) {
            Ok(received) => queued = received.summary().queued_reliable,
            Err(error) => break error,
        }
    };
    assert_eq!(queued, 224);
    assert!(format!("{error:#}").contains("255"));
    assert!(peer.is_failed());
    assert!(peer.ack_packet().is_err());
    let (next, _) = client.send(Vec::new());
    assert!(peer.receive(&next).is_err());
}

/// Read a lab datagram as the client: deliver its header and every reliable
/// control bunch through the client model; returns the notifications and the
/// messages released in order.
fn client_receive(client: &mut Client, datagram: &[u8]) -> (Vec<Notification>, Vec<Decoded>) {
    let packet = read_packet(datagram, &to_client()).unwrap();
    let received = client.sequencer.receive(&packet.header).unwrap().unwrap();
    let mut messages = Vec::new();
    for bunch in &packet.bunches {
        let header = &bunch.header;
        assert!(header.reliable && !header.open && !header.close && !header.partial);
        assert_eq!(header.channel_index, 0);
        assert_eq!(header.channel_name, Some(Name::Hardcoded(255)));
        let decoded =
            read_control_messages(&mut bunch.payload.reader(), &ControlCodec::default()).unwrap();
        let [Decoded::Message(message)] = decoded.as_slice() else {
            panic!("one known message per lab bunch");
        };
        let raw = header.reliable_sequence_raw.unwrap();
        if let ReliableReceive::Delivered(batch) = client
            .control
            .receive_reliable(raw, message.clone())
            .unwrap()
        {
            messages.extend(batch.into_iter().map(Decoded::Message));
        }
    }
    client.sequencer.ack_seq(received.sequence, true).unwrap();
    (received.notifications, messages)
}

fn challenge(text: &str) -> ControlMessage {
    ControlMessage::Challenge {
        token: ArchiveText::text(text).unwrap(),
    }
}

fn raw_sequences(datagram: &[u8]) -> Vec<u32> {
    read_packet(datagram, &to_client())
        .unwrap()
        .bunches
        .iter()
        .map(|bunch| bunch.header.reliable_sequence_raw.unwrap())
        .collect()
}

#[test]
fn reliable_control_packets_advance_sequences_and_carry_acks() {
    let mut peer = peer();
    let mut client = Client::new();
    let (hello_datagram, hello_id) = client.send_reliable(hello(), true);
    peer.receive(&hello_datagram).unwrap();
    let (_, initial_out) = peer.sequencer().initial_reliable();
    assert_eq!(peer.control_out_reliable(), initial_out);
    let first_id = peer.sequencer().out_packet_id();

    let first = peer.reliable_control_packet(challenge(SENTINEL)).unwrap();
    let packet = read_packet(&first, &to_client()).unwrap();
    assert_eq!(packet.header.sequence, 0x1234);
    assert_eq!(packet.header.acked_sequence, 0x0405);
    assert_eq!(packet.header.history, [1]);
    assert_eq!(packet.bunches.len(), 1);
    assert_eq!(
        packet.bunches[0].header.reliable_sequence_raw,
        Some(wire_reliable_sequence(initial_out + 1))
    );
    assert_eq!(write_packet(&packet, &to_client()).unwrap(), first);
    assert_eq!(peer.sequencer().out_packet_id(), first_id + 1);
    assert_eq!(peer.control_out_reliable(), initial_out + 1);
    assert_eq!(peer.pending_reliable(), 1);

    let second = peer
        .reliable_control_packet(ControlMessage::Upgrade { version: 7 })
        .unwrap();
    let header = read_packet(&second, &to_client()).unwrap().header;
    assert_eq!(header.sequence, 0x1235);
    assert_eq!(raw_sequences(&second), [initial_out + 2]);
    assert_eq!(peer.pending_reliable(), 2);

    // The client acks its Hello packet and receives both messages in order.
    let (notifications, messages) = client_receive(&mut client, &first);
    assert_eq!(
        notifications,
        [Notification {
            packet_id: hello_id,
            delivered: true
        }]
    );
    assert_eq!(messages, [Decoded::Message(challenge(SENTINEL))]);
    let (_, messages) = client_receive(&mut client, &second);
    assert_eq!(
        messages,
        [Decoded::Message(ControlMessage::Upgrade { version: 7 })]
    );

    // The client's next packet acknowledges both; the bunches are released.
    let (next, _) = client.send(Vec::new());
    let summary = peer.receive(&next).unwrap().summary().clone();
    assert_eq!(summary.acked, [first_id, first_id + 1]);
    assert_eq!((summary.released_reliable, summary.resend_queued), (2, 0));
    assert_eq!(peer.pending_reliable(), 0);
    // Debug and summaries carry counts only.
    for text in [
        format!("{peer:?}"),
        serde_json::to_string(&summary).unwrap(),
    ] {
        assert!(!text.contains(SENTINEL), "{text}");
    }
}

#[test]
fn naked_reliable_bunches_are_resent_in_the_next_datagram() {
    let mut config = PacketLabConfig::new(LIMITS);
    config.max_bunches_per_packet = 2;
    let mut peer = LabPeer::from_cookie(&COOKIE, config).unwrap();
    let mut client = Client::new();
    let (opening, _) = client.send(Vec::new());
    peer.receive(&opening).unwrap();
    let (_, initial_out) = peer.sequencer().initial_reliable();
    let lost_id = peer.sequencer().out_packet_id();
    for index in 0..3 {
        let _lost = peer
            .reliable_control_packet(challenge(&format!("lost-{index}")))
            .unwrap();
    }
    let ack = peer.ack_packet().unwrap();
    assert!(raw_sequences(&ack).is_empty(), "nothing naked yet");
    let (_, messages) = client_receive(&mut client, &ack);
    assert!(messages.is_empty());

    // The client's ack history naks the three lost packets.
    let (next, _) = client.send(Vec::new());
    let summary = peer.receive(&next).unwrap().summary().clone();
    assert_eq!(summary.naked, [lost_id, lost_id + 1, lost_id + 2]);
    assert_eq!(summary.acked, [lost_id + 3]);
    assert_eq!((summary.resend_queued, summary.released_reliable), (3, 0));
    assert_eq!((peer.pending_resends(), peer.pending_reliable()), (3, 3));

    // At most two bunches per datagram: two resends now, the third next.
    let resent = peer.ack_packet().unwrap();
    assert_eq!(raw_sequences(&resent), [initial_out + 1, initial_out + 2]);
    assert_eq!(peer.pending_resends(), 1);
    let rest = peer.reliable_control_packet(challenge("fresh")).unwrap();
    assert_eq!(raw_sequences(&rest), [initial_out + 3, initial_out + 4]);
    assert_eq!((peer.pending_resends(), peer.pending_reliable()), (0, 4));
    let (_, first) = client_receive(&mut client, &resent);
    let (_, second) = client_receive(&mut client, &rest);
    let delivered: Vec<Decoded> = first.into_iter().chain(second).collect();
    assert_eq!(
        delivered,
        ["lost-0", "lost-1", "lost-2", "fresh"].map(|text| Decoded::Message(challenge(text)))
    );
    let (next, _) = client.send(Vec::new());
    assert_eq!(peer.receive(&next).unwrap().summary().released_reliable, 4);
    assert_eq!(peer.pending_reliable(), 0);
}

#[test]
fn outgoing_limits_fail_without_state_change() {
    let mut peer = peer();
    let mut client = Client::new();
    let (opening, _) = client.send(Vec::new());
    peer.receive(&opening).unwrap();
    let before = format!("{peer:?}");
    let out_id = peer.sequencer().out_packet_id();
    // A payload above the bunch bit limit, then one under it whose datagram
    // still exceeds the maximum packet size.
    for length in [1100, 1015] {
        let message = ControlMessage::Failure {
            message: ArchiveText::text(&"x".repeat(length)).unwrap(),
        };
        assert!(peer.reliable_control_packet(message).is_err(), "{length}");
        assert_eq!(format!("{peer:?}"), before);
        assert_eq!(peer.sequencer().out_packet_id(), out_id);
        assert!(!peer.is_failed());
    }
    // A message that fits is sent within the size limit.
    let fits = ControlMessage::Failure {
        message: ArchiveText::text(&"x".repeat(900)).unwrap(),
    };
    assert!(peer.reliable_control_packet(fits).unwrap().len() <= 1024);

    // The outstanding reliable bound.
    while peer.pending_reliable() < MAX_PENDING_RELIABLE {
        peer.reliable_control_packet(ControlMessage::Upgrade { version: 1 })
            .unwrap();
    }
    let before = format!("{peer:?}");
    let upgrade = ControlMessage::Upgrade { version: 1 };
    assert!(peer.reliable_control_packet(upgrade).is_err());
    assert_eq!(format!("{peer:?}"), before);
    assert!(peer.ack_packet().is_ok());
}

fn actor_open_bunch(channel_index: u32, payload_bytes: &[u8]) -> Bunch {
    let mut writer = BitWriter::new();
    writer.write_bytes(payload_bytes);
    Bunch {
        header: BunchHeader {
            control: true,
            open: true,
            close: false,
            close_reason: None,
            paused: false,
            reliable: true,
            channel_index,
            has_package_map_exports: false,
            has_must_be_mapped_guids: false,
            partial: false,
            reliable_sequence_raw: None,
            partial_initial: false,
            partial_final: false,
            channel_name: Some(Name::Hardcoded(102)),
        },
        payload: Payload::from_writer(writer),
    }
}

fn incoming_actor(channel: u32, sequence: u32, marker: u8) -> Bunch {
    let mut bunch = actor_open_bunch(channel, &[marker]);
    bunch.header.control = false;
    bunch.header.open = false;
    bunch.header.reliable_sequence_raw = Some(wire_reliable_sequence(sequence));
    bunch
}

#[test]
fn actor_receive_orders_duplicates_and_isolates_channels() {
    let mut peer = peer();
    let mut client = Client::new();
    for channel in [2, 3] {
        peer.reliable_actor_open(&actor_open_bunch(channel, &[1]), &[])
            .unwrap();
    }
    let first = peer.sequencer().initial_reliable().0 + 1;
    let later = incoming_actor(2, first + 1, 2);
    let (packet, _) = client.send(vec![
        later.clone(),
        later.clone(),
        incoming_actor(3, first, 9),
    ]);
    let received = peer.receive(&packet).unwrap();
    assert_eq!(received.summary().actor_queued_reliable, 1);
    assert_eq!(received.summary().actor_duplicate_reliable, 1);
    assert_eq!(received.actor_bunches(), &[incoming_actor(3, first, 9)]);
    let (packet, _) = client.send(vec![incoming_actor(2, first, 1)]);
    let received = peer.receive(&packet).unwrap();
    assert_eq!(received.summary().actor_queued_reliable, 0);
    assert_eq!(
        received.actor_bunches(),
        &[incoming_actor(2, first, 1), later.clone()]
    );
    let (packet, _) = client.send(vec![later, incoming_actor(4, first, 0)]);
    let received = peer.receive(&packet).unwrap();
    assert_eq!(received.summary().actor_duplicate_reliable, 1);
    assert_eq!(received.summary().ignored_bunches, 1);
    assert!(received.actor_bunches().is_empty());
    assert!(!peer.is_failed());
}

#[test]
fn actor_receive_drops_unsupported_forms_without_reliable_gaps() {
    let mut peer = peer();
    let mut client = Client::new();
    peer.reliable_actor_open(&actor_open_bunch(2, &[1]), &[])
        .unwrap();
    let first = peer.sequencer().initial_reliable().0 + 1;
    let mut unsupported = Vec::new();
    for offset in 0..6 {
        let mut bunch = incoming_actor(2, first + offset, 1);
        match offset {
            0 => {
                bunch.header.partial = true;
                bunch.header.partial_initial = true;
            }
            1 => bunch.header.has_package_map_exports = true,
            2 => bunch.header.has_must_be_mapped_guids = true,
            3 => {
                bunch.header.open = true;
                bunch.header.control = true;
                bunch.header.channel_name = Some(Name::Hardcoded(255));
            }
            4 => {
                bunch.header.close = true;
                bunch.header.control = true;
                bunch.header.close_reason = Some(0);
            }
            _ => bunch.header.paused = true,
        }
        unsupported.push(bunch);
    }
    let valid = incoming_actor(2, first + 6, 7);
    unsupported.push(valid.clone());
    let (packet, _) = client.send(unsupported);
    let received = peer.receive(&packet).unwrap();
    assert_eq!(received.summary().actor_bunches_dropped, 6);
    assert_eq!(received.actor_bunches(), &[valid]);
    assert!(!peer.is_failed());
}

#[test]
fn actor_receive_queue_overflow_is_bounded_and_does_not_fail_the_peer() {
    let mut peer = peer();
    let mut client = Client::new();
    peer.reliable_actor_open(&actor_open_bunch(2, &[1]), &[])
        .unwrap();
    let first = peer.sequencer().initial_reliable().0 + 1;
    let queued: Vec<_> = (1..=255)
        .map(|offset| incoming_actor(2, first + offset, 1))
        .collect();
    for chunk in queued.chunks(32) {
        let (packet, _) = client.send(chunk.to_vec());
        let received = peer.receive(&packet).unwrap();
        assert!(received.actor_bunches().is_empty());
    }
    let (packet, _) = client.send(vec![incoming_actor(2, first + 256, 2)]);
    let received = peer.receive(&packet).unwrap();
    assert_eq!(received.summary().actor_queued_reliable, 255);
    assert_eq!(received.summary().actor_bunches_dropped, 1);
    assert!(!peer.is_failed());
    let ack = read_packet(&peer.ack_packet().unwrap(), &to_client()).unwrap();
    assert_eq!(
        ack.header.history[0] & 1,
        0,
        "overflow requests retransmission"
    );
    let (packet, _) = client.send(vec![incoming_actor(2, first, 0)]);
    let received = peer.receive(&packet).unwrap();
    assert_eq!(received.actor_bunches().len(), 256);
    assert_eq!(received.summary().actor_queued_reliable, 0);
    // An explicitly resent overflow entry can still follow the drained queue.
    let (packet, _) = client.send(vec![incoming_actor(2, first + 256, 2)]);
    assert_eq!(peer.receive(&packet).unwrap().actor_bunches().len(), 1);
}

#[test]
fn actor_open_splits_exports_and_body_and_resends_them_unchanged() {
    let mut peer = peer();
    let mut client = Client::new();
    let (opening, _) = client.send(Vec::new());
    peer.receive(&opening).unwrap();
    let (_, initial_out) = peer.sequencer().initial_reliable();
    let mut exports = BitWriter::new();
    exports.write_bytes(&[0xa5; 40]);
    let exports = Payload::from_writer(exports);
    let body = actor_open_bunch(2, &[0x5a; 8]);

    // Rejected without a state change: the control channel, a non-open bunch.
    let before = format!("{peer:?}");
    let out_id = peer.sequencer().out_packet_id();
    for bad in [
        actor_open_bunch(0, &[1]),
        Bunch {
            header: BunchHeader {
                open: false,
                ..body.header.clone()
            },
            ..body.clone()
        },
    ] {
        assert!(peer.reliable_actor_open(&bad, &[]).is_err());
        assert_eq!(format!("{peer:?}"), before);
        assert_eq!(peer.sequencer().out_packet_id(), out_id);
    }

    let datagram = peer
        .reliable_actor_open(&body, std::slice::from_ref(&exports))
        .unwrap();
    assert_eq!(peer.pending_actor_reliable(2), Some(2));
    assert!(
        peer.reliable_actor_open(&body, &[]).is_err(),
        "already open"
    );
    let packet = read_packet(&datagram, &to_client()).unwrap();
    let [first, second] = packet.bunches.as_slice() else {
        panic!("exports and body travel as two partial parts");
    };
    let (h0, h1) = (&first.header, &second.header);
    assert!(h0.open && h0.partial && h0.partial_initial && !h0.partial_final);
    assert!(h0.has_package_map_exports && h0.reliable);
    assert!(!h1.open && h1.partial && !h1.partial_initial && h1.partial_final);
    assert!(!h1.has_package_map_exports && h1.reliable);
    assert_eq!(
        (h0.reliable_sequence_raw, h1.reliable_sequence_raw),
        (
            Some(wire_reliable_sequence(initial_out + 1)),
            Some(wire_reliable_sequence(initial_out + 2))
        )
    );
    assert_eq!(h0.channel_name, Some(Name::Hardcoded(102)));
    assert_eq!(first.payload, exports);
    assert_eq!(second.payload, body.payload);

    // The client loses it and receives a later ack packet, so its next
    // packet naks the open and both parts are queued for resend.
    let ack = peer.ack_packet().unwrap();
    assert!(read_packet(&ack, &to_client()).unwrap().bunches.is_empty());
    client.receive(&ack);
    let (next, _) = client.send(Vec::new());
    let summary = peer.receive(&next).unwrap().summary().clone();
    assert_eq!(summary.naked, [out_id]);
    assert_eq!(summary.resend_queued, 2);
    let resent = peer.ack_packet().unwrap();
    let resent_bunches = read_packet(&resent, &to_client()).unwrap().bunches;
    assert_eq!(resent_bunches, packet.bunches, "resent unchanged");
    assert_eq!(peer.pending_resends(), 0);

    // Acknowledging the resend releases both parts.
    let received = client
        .sequencer
        .receive(&read_packet(&resent, &to_client()).unwrap().header)
        .unwrap()
        .unwrap();
    client.sequencer.ack_seq(received.sequence, true).unwrap();
    let (next, _) = client.send(Vec::new());
    let summary = peer.receive(&next).unwrap().summary().clone();
    assert_eq!(summary.released_reliable, 2);
    assert_eq!(peer.pending_actor_reliable(2), Some(0));
}

#[test]
fn control_close_is_a_reliable_close_bunch_resent_unchanged() {
    let mut peer = peer();
    let mut client = Client::new();
    let (opening, _) = client.send(Vec::new());
    peer.receive(&opening).unwrap();
    assert!(!peer.has_received_client_packet(), "no reliable bunch yet");
    let (hello_datagram, _) = client.send_reliable(hello(), true);
    peer.receive(&hello_datagram).unwrap();
    assert!(peer.has_received_client_packet());
    let (_, initial_out) = peer.sequencer().initial_reliable();
    let close_id = peer.sequencer().out_packet_id();

    let datagram = peer.reliable_control_close(0).unwrap();
    assert!(peer.control_closing());
    let packet = read_packet(&datagram, &to_client()).unwrap();
    let [bunch] = packet.bunches.as_slice() else {
        panic!("one close bunch");
    };
    let header = &bunch.header;
    assert!(header.control && header.close && header.reliable && !header.open);
    assert_eq!(header.close_reason, Some(0));
    assert_eq!(header.channel_index, 0);
    assert_eq!(header.channel_name, Some(Name::Hardcoded(255)));
    assert_eq!(
        header.reliable_sequence_raw,
        Some(wire_reliable_sequence(initial_out + 1))
    );
    assert_eq!(bunch.payload.bit_len(), 0);

    // Nothing may follow the close, and a second close fails the same way.
    let before = format!("{peer:?}");
    assert!(peer.reliable_control_packet(hello()).is_err());
    assert!(peer.reliable_control_close(0).is_err());
    assert_eq!(format!("{peer:?}"), before);

    // Lost, then naked: the resend is the same close bunch.
    let ack = peer.ack_packet().unwrap();
    client.receive(&ack);
    let (next, _) = client.send(Vec::new());
    let summary = peer.receive(&next).unwrap().summary().clone();
    assert_eq!(summary.naked, [close_id]);
    let resent = read_packet(&peer.ack_packet().unwrap(), &to_client())
        .unwrap()
        .bunches;
    assert_eq!(resent, packet.bunches);
}

#[test]
fn actor_close_holds_its_channel_until_acknowledged() {
    let mut peer = peer();
    let mut client = Client::new();
    let (opening, _) = client.send(Vec::new());
    peer.receive(&opening).unwrap();
    let open = actor_open_bunch(4, &[0x11; 4]);
    let datagram = peer.reliable_actor_open(&open, &[]).unwrap();
    accept_header(&mut client, &datagram);
    assert!(peer.reliable_actor_close(5, 3).is_err(), "not open");

    let close = peer.reliable_actor_close(4, 3).unwrap();
    assert!(peer.actor_channel_closing(4));
    let packet = read_packet(&close, &to_client()).unwrap();
    let [bunch] = packet.bunches.as_slice() else {
        panic!("one close bunch");
    };
    let header = &bunch.header;
    assert!(header.close && header.reliable && header.control && !header.open);
    assert_eq!((header.channel_index, header.close_reason), (4, Some(3)));
    assert_eq!(header.channel_name, Some(Name::Hardcoded(102)));
    assert_eq!(bunch.payload.bit_len(), 0);
    let before = format!("{peer:?}");
    assert!(peer.reliable_actor_close(4, 3).is_err(), "already closing");
    assert!(
        peer.reliable_actor_open(&open, &[]).is_err(),
        "index still held"
    );
    assert_eq!(peer.free_actor_channel(4), Some(5));
    assert_eq!(format!("{peer:?}"), before);

    // Once the client acknowledges the close, the index is free again.
    accept_header(&mut client, &close);
    let (ack, _) = client.send(Vec::new());
    assert_eq!(
        peer.receive(&ack)
            .unwrap()
            .summary()
            .released_actor_channels,
        [4]
    );
    assert!(!peer.actor_channel_closing(4));
    assert!(!peer.has_actor_channel(4));
    assert_eq!(peer.free_actor_channel(4), Some(4));
    peer.reliable_actor_open(&open, &[]).unwrap();
    let (ack, _) = client.send(Vec::new());
    assert!(
        peer.receive(&ack)
            .unwrap()
            .summary()
            .released_actor_channels
            .is_empty()
    );
}

#[test]
fn lost_actor_close_releases_once_after_retransmit_ack_and_channel_reuse() {
    let mut peer = peer();
    let mut client = Client::new();
    let (opening, _) = client.send(Vec::new());
    peer.receive(&opening).unwrap();
    let open = actor_open_bunch(4, &[0x11; 4]);
    let datagram = peer.reliable_actor_open(&open, &[]).unwrap();
    accept_header(&mut client, &datagram);
    let (ack, _) = client.send(Vec::new());
    assert!(
        peer.receive(&ack)
            .unwrap()
            .summary()
            .released_actor_channels
            .is_empty()
    );

    let close_id = peer.sequencer().out_packet_id();
    let close = peer.reliable_actor_close(4, 3).unwrap();
    // Receive a later packet while deliberately dropping the close.
    accept_header(&mut client, &peer.ack_packet().unwrap());
    let (nak, _) = client.send(Vec::new());
    let received = peer.receive(&nak).unwrap();
    assert_eq!(received.summary().naked, [close_id]);
    assert!(received.summary().released_actor_channels.is_empty());
    assert!(peer.actor_channel_closing(4));
    let resent = peer.ack_packet().unwrap();
    assert_eq!(
        read_packet(&resent, &to_client()).unwrap().bunches,
        read_packet(&close, &to_client()).unwrap().bunches
    );
    accept_header(&mut client, &resent);
    let (ack, _) = client.send(Vec::new());
    assert_eq!(
        peer.receive(&ack)
            .unwrap()
            .summary()
            .released_actor_channels,
        [4]
    );
    assert!(!peer.has_actor_channel(4));

    let reopened = peer.reliable_actor_open(&open, &[]).unwrap();
    accept_header(&mut client, &reopened);
    let (ack, _) = client.send(Vec::new());
    assert!(
        peer.receive(&ack)
            .unwrap()
            .summary()
            .released_actor_channels
            .is_empty()
    );
    assert!(peer.actor_channel_settled(4));
}

/// The ordered free-channel search matches the plain linear definition for
/// every start index, including the control channel, gaps and a full range.
#[test]
fn free_actor_channel_matches_the_linear_search() {
    let mut peer = peer();
    let mut client = Client::new();
    let (opening, _) = client.send(Vec::new());
    peer.receive(&opening).unwrap();
    let limit = peer.config().packet.limits.max_channels;
    let control = peer.config().control_channel_index;
    let linear = |peer: &LabPeer, first: u32| {
        (first..limit).find(|index| *index != control && !peer.has_actor_channel(*index))
    };
    let mut open = Vec::new();
    for channel in [3, 4, 6, 7, 8, 12, limit - 2, limit - 1] {
        if channel == control {
            continue;
        }
        let datagram = peer
            .reliable_actor_open(&actor_open_bunch(channel, &[0x11; 4]), &[])
            .unwrap();
        accept_header(&mut client, &datagram);
        open.push(channel);
        for first in 0..=limit + 1 {
            assert_eq!(
                peer.free_actor_channel(first),
                linear(&peer, first),
                "{first} {open:?}"
            );
        }
    }
    // Fill every index, so the exhausted result is remembered; opening never
    // frees an index, and an acknowledged close frees one again.
    for channel in (0..limit).filter(|index| *index != control && !open.contains(index)) {
        let datagram = peer
            .reliable_actor_open(&actor_open_bunch(channel, &[0x11; 4]), &[])
            .unwrap();
        accept_header(&mut client, &datagram);
    }
    for first in [5, 0, 3, limit] {
        assert_eq!(peer.free_actor_channel(first), None);
        assert_eq!(linear(&peer, first), None);
    }
    let close = peer.reliable_actor_close(7, 3).unwrap();
    assert_eq!(peer.free_actor_channel(0), None, "held until acknowledged");
    accept_header(&mut client, &close);
    let (ack, _) = client.send(Vec::new());
    peer.receive(&ack).unwrap();
    for first in 0..=limit + 1 {
        assert_eq!(
            peer.free_actor_channel(first),
            linear(&peer, first),
            "{first}"
        );
    }
    assert_eq!(peer.free_actor_channel(3), Some(7));
}

/// Accept a lab datagram's header as the client, whatever its bunches.
fn accept_header(client: &mut Client, datagram: &[u8]) {
    let packet = read_packet(datagram, &to_client()).unwrap();
    let received = client.sequencer.receive(&packet.header).unwrap().unwrap();
    client.sequencer.ack_seq(received.sequence, true).unwrap();
}

#[test]
fn actor_updates_need_an_open_channel_and_settle_after_the_open_ack() {
    let mut peer = peer();
    let mut client = Client::new();
    let (opening, _) = client.send(Vec::new());
    peer.receive(&opening).unwrap();
    let update = || {
        let mut writer = BitWriter::new();
        writer.write_bytes(&[0x42]);
        Payload::from_writer(writer)
    };
    assert!(peer.actor_update(4, update()).is_err(), "not open");
    assert!(!peer.actor_channel_settled(4));

    let open = peer
        .reliable_actor_open(&actor_open_bunch(4, &[1]), &[])
        .unwrap();
    assert!(!peer.actor_channel_settled(4), "open not acked yet");
    let sent = peer.actor_update(4, update()).unwrap();
    let packet = read_packet(&sent, &to_client()).unwrap();
    let [bunch] = packet.bunches.as_slice() else {
        panic!("one update bunch");
    };
    let header = &bunch.header;
    assert!(!header.reliable && !header.open && !header.close && !header.control);
    assert_eq!(
        (header.channel_index, header.channel_name.clone()),
        (4, None)
    );
    assert_eq!(bunch.payload, update());
    assert_eq!(
        peer.pending_actor_reliable(4),
        Some(1),
        "updates are not retained"
    );

    // Acknowledging the open settles the channel.
    accept_header(&mut client, &open);
    accept_header(&mut client, &sent);
    let (ack, _) = client.send(Vec::new());
    peer.receive(&ack).unwrap();
    assert!(peer.actor_channel_settled(4));

    // A closing channel takes no more updates and is no longer settled.
    let close = peer.reliable_actor_close(4, 3).unwrap();
    assert!(!peer.actor_channel_settled(4));
    let before = format!("{peer:?}");
    assert!(peer.actor_update(4, update()).is_err(), "closing");
    assert_eq!(format!("{peer:?}"), before);
    accept_header(&mut client, &close);
}

fn bytes_payload(bytes: &[u8]) -> Payload {
    let mut writer = BitWriter::new();
    writer.write_bytes(bytes);
    Payload::from_writer(writer)
}

/// A GameState-sized open on channel 2: 2,400 export bits and an 8,400-bit
/// body, which `SendBunch` splits into 7,624 and 776 bits.
fn large_open() -> (Bunch, Payload) {
    (
        actor_open_bunch(2, &[0x5a; 1050]),
        bytes_payload(&[0xa5; 300]),
    )
}

/// Feed every part of `datagrams`, in order, through the client's partial
/// reassembler; returns the parts and the reassembled bunch.
fn reassemble(datagrams: &[Vec<u8>]) -> (Vec<Bunch>, Bunch) {
    let mut reassembler = PartialBunchReassembler::new();
    let mut parts = Vec::new();
    let mut complete = None;
    for datagram in datagrams {
        for part in read_packet(datagram, &to_client()).unwrap().bunches {
            assert!(complete.is_none(), "no part after the final one");
            let header = &part.header;
            assert!(header.partial && header.reliable && header.channel_index == 2);
            // Export parts carry only exports, stripped before reassembly.
            let after_exports = if header.has_package_map_exports {
                Payload::from_writer(BitWriter::new())
            } else {
                part.payload.clone()
            };
            let sequence = i64::from(header.reliable_sequence_raw.unwrap());
            let received = reassembler
                .receive_part(
                    sequence,
                    header,
                    &after_exports,
                    Some(MAX_CONSTRUCTED_PARTIAL_BUNCH_BYTES),
                )
                .unwrap();
            if let PartialReceive::Complete { bunch, .. } = received {
                complete = Some(bunch);
            }
            parts.push(part);
        }
    }
    (parts, complete.expect("the final part completes the bunch"))
}

#[test]
fn actor_open_spreads_parts_over_datagrams_and_resends_each_unchanged() {
    let mut peer = peer();
    let mut client = Client::new();
    let (opening, _) = client.send(Vec::new());
    peer.receive(&opening).unwrap();
    let (_, initial_out) = peer.sequencer().initial_reliable();
    let first_id = peer.sequencer().out_packet_id();
    let (body, exports) = large_open();

    // The single-datagram form refuses it without a state change.
    let before = format!("{peer:?}");
    assert!(
        peer.reliable_actor_open(&body, std::slice::from_ref(&exports))
            .is_err()
    );
    assert_eq!(format!("{peer:?}"), before);
    assert_eq!(peer.sequencer().out_packet_id(), first_id);

    let sent = peer
        .reliable_actor_open_datagrams(&body, std::slice::from_ref(&exports))
        .unwrap();
    // The exports went out in the first packet (F-796).
    assert_eq!(sent.packet_ids, [first_id, first_id + 1, first_id + 2]);
    assert_eq!(sent.exports_packet_id, Some(first_id));
    let datagrams = sent.datagrams;
    // Exports, then the body's two slices: no two fit one 1,024-byte packet.
    assert_eq!(datagrams.len(), 3);
    assert_eq!(peer.sequencer().out_packet_id(), first_id + 3);
    assert_eq!(peer.pending_actor_reliable(2), Some(3));
    let packets: Vec<Packet> = datagrams
        .iter()
        .map(|datagram| read_packet(datagram, &to_client()).unwrap())
        .collect();
    for (offset, (datagram, packet)) in datagrams.iter().zip(&packets).enumerate() {
        assert!(datagram.len() <= LIMITS.max_packet_bytes as usize);
        assert_eq!(packet.bunches.len(), 1);
        assert_eq!(usize::from(packet.header.sequence), 0x1234 + offset);
    }

    // In order, the parts reassemble the open on consecutive sequences.
    let (parts, bunch) = reassemble(&datagrams);
    let raws: Vec<u32> = parts
        .iter()
        .map(|part| part.header.reliable_sequence_raw.unwrap())
        .collect();
    let expected_raws: Vec<u32> = (1..=3)
        .map(|offset| wire_reliable_sequence(initial_out + offset))
        .collect();
    assert_eq!(raws, expected_raws);
    let flags: Vec<[bool; 4]> = parts
        .iter()
        .map(|part| {
            let header = &part.header;
            [
                header.open,
                header.partial_initial,
                header.partial_final,
                header.has_package_map_exports,
            ]
        })
        .collect();
    assert_eq!(
        flags,
        [
            [true, true, false, true],
            [false, false, false, false],
            [false, false, true, false]
        ]
    );
    assert_eq!(parts[0].payload, exports);
    assert_eq!(
        (parts[1].payload.bit_len(), parts[2].payload.bit_len()),
        (7_624, 776)
    );
    let mut expected = body.clone();
    expected.header.reliable_sequence_raw = Some(wire_reliable_sequence(initial_out + 1));
    assert_eq!(bunch, expected);

    // The client loses the second datagram only: it is naked, the others
    // acked; the first part is released, the third waits behind the second.
    accept_header(&mut client, &datagrams[0]);
    accept_header(&mut client, &datagrams[2]);
    let (next, _) = client.send(Vec::new());
    let summary = peer.receive(&next).unwrap().summary().clone();
    assert_eq!(summary.acked, [first_id, first_id + 2]);
    assert_eq!(summary.naked, [first_id + 1]);
    assert_eq!((summary.resend_queued, summary.released_reliable), (1, 1));
    assert_eq!(peer.pending_actor_reliable(2), Some(2));
    let resent = peer.ack_packet().unwrap();
    assert_eq!(
        read_packet(&resent, &to_client()).unwrap().bunches,
        packets[1].bunches,
        "exactly the naked datagram's part, unchanged"
    );
    assert_eq!(peer.pending_resends(), 0);

    // Acknowledging the resend releases the rest; nothing more is resent.
    accept_header(&mut client, &resent);
    let (next, _) = client.send(Vec::new());
    let summary = peer.receive(&next).unwrap().summary().clone();
    assert_eq!(summary.acked, [first_id + 3]);
    assert!(summary.naked.is_empty());
    assert_eq!(summary.released_reliable, 2);
    assert_eq!(peer.pending_actor_reliable(2), Some(0));
    assert!(peer.actor_channel_settled(2));
    assert_eq!(peer.pending_resends(), 0);
    let ack = peer.ack_packet().unwrap();
    assert!(read_packet(&ack, &to_client()).unwrap().bunches.is_empty());
}

#[test]
fn actor_open_datagrams_carry_pending_resends_in_the_first_only() {
    let mut peer = peer();
    let mut client = Client::new();
    let (opening, _) = client.send(Vec::new());
    peer.receive(&opening).unwrap();
    let lost_id = peer.sequencer().out_packet_id();
    let lost = peer.reliable_control_packet(challenge("lost")).unwrap();
    let ack = peer.ack_packet().unwrap();
    accept_header(&mut client, &ack);
    let (next, _) = client.send(Vec::new());
    assert_eq!(peer.receive(&next).unwrap().summary().naked, [lost_id]);
    assert_eq!(peer.pending_resends(), 1);

    let (body, exports) = large_open();
    let datagrams = peer
        .reliable_actor_open_datagrams(&body, std::slice::from_ref(&exports))
        .unwrap()
        .datagrams;
    assert_eq!(datagrams.len(), 3);
    assert_eq!(peer.pending_resends(), 0);
    let first = read_packet(&datagrams[0], &to_client()).unwrap().bunches;
    let [resend, export] = first.as_slice() else {
        panic!("the resend precedes the export part");
    };
    assert_eq!(
        std::slice::from_ref(resend),
        read_packet(&lost, &to_client()).unwrap().bunches
    );
    assert!(export.header.partial_initial && export.header.has_package_map_exports);
    for datagram in &datagrams[1..] {
        let bunches = read_packet(datagram, &to_client()).unwrap().bunches;
        assert!(bunches.iter().all(|bunch| bunch.header.channel_index == 2));
    }
    // The resent control bunch now rides the first datagram's packet ID.
    for datagram in &datagrams {
        accept_header(&mut client, datagram);
    }
    let (next, _) = client.send(Vec::new());
    assert_eq!(peer.receive(&next).unwrap().summary().released_reliable, 4);
    assert_eq!(
        (peer.pending_reliable(), peer.pending_actor_reliable(2)),
        (0, Some(0))
    );
}

#[test]
fn actor_open_datagram_failures_leave_the_state_unchanged() {
    let open_peer = |sent_capacity: usize| {
        let mut config = PacketLabConfig::new(LIMITS);
        config.sent_capacity = sent_capacity;
        let mut peer = LabPeer::from_cookie(&COOKIE, config).unwrap();
        let (opening, _) = Client::new().send(Vec::new());
        peer.receive(&opening).unwrap();
        peer
    };
    let (body, exports) = large_open();
    let exports = std::slice::from_ref(&exports);
    let small = actor_open_bunch(3, &[0x11; 4]);
    let mut tried = open_peer(16);
    let mut fresh = open_peer(16);
    assert_eq!(
        tried.reliable_actor_open_datagrams(&small, &[]).unwrap(),
        fresh.reliable_actor_open_datagrams(&small, &[]).unwrap()
    );

    // 254 exports and the body's two slices: one part beyond the window.
    let too_many = vec![bytes_payload(&[1]); MAX_PENDING_RELIABLE - 1];
    let oversized = [bytes_payload(&[0xa5; 1100])];
    let control = actor_open_bunch(0, &[1]);
    let attempts: [(&Bunch, &[Payload], &str); 4] = [
        (&control, &[], "control channel"),
        (&small, &[], "already open"),
        (&body, &too_many, "parts beyond the reliable window"),
        (&body, &oversized, "an export part beyond one datagram"),
    ];
    let before = format!("{tried:?}");
    let out_id = tried.sequencer().out_packet_id();
    for (bunch, exports, why) in attempts {
        assert!(
            tried.reliable_actor_open_datagrams(bunch, exports).is_err(),
            "{why}"
        );
        assert_eq!(format!("{tried:?}"), before, "{why}");
        assert_eq!(tried.sequencer().out_packet_id(), out_id, "{why}");
        assert!(!tried.is_failed(), "{why}");
    }
    assert!(!tried.has_actor_channel(2));
    // One export fewer fills the window exactly and is sent.
    let mut window = open_peer(16);
    let fits = &too_many[1..];
    let datagrams = window
        .reliable_actor_open_datagrams(&body, fits)
        .unwrap()
        .datagrams;
    assert_eq!(window.pending_actor_reliable(2), Some(MAX_PENDING_RELIABLE));
    assert!(datagrams.len() >= 2);

    // A later open gets the same sequences, packet IDs and bytes.
    assert_eq!(
        tried.reliable_actor_open_datagrams(&body, exports).unwrap(),
        fresh.reliable_actor_open_datagrams(&body, exports).unwrap()
    );

    // A sent-record ring without room for every datagram: nothing commits.
    let mut tight = open_peer(2);
    let before = format!("{tight:?}");
    assert!(tight.reliable_actor_open_datagrams(&body, exports).is_err());
    assert_eq!(format!("{tight:?}"), before);
    assert!(!tight.has_actor_channel(2) && !tight.is_failed());
    let mut roomy = open_peer(2);
    assert_eq!(
        tight.reliable_actor_open_datagrams(&small, &[]).unwrap(),
        roomy.reliable_actor_open_datagrams(&small, &[]).unwrap()
    );
}

#[test]
fn committed_packets_keep_their_send_instant_for_the_last_256() {
    use icarus_bevy::experimental::packet_lab::SENT_AT_CAPACITY;
    let mut config = PacketLabConfig::new(LIMITS);
    config.sent_capacity = 1_024;
    let mut peer = LabPeer::from_cookie(&COOKIE, config).unwrap();
    let first = peer.sequencer().out_packet_id();
    let before = std::time::Instant::now();
    peer.ack_packet().unwrap();
    let stamped = peer.sent_at(first).expect("the first packet is stamped");
    assert!(stamped >= before);
    assert_eq!(peer.sent_at(first.wrapping_add(1)), None, "not sent yet");
    for _ in 0..SENT_AT_CAPACITY {
        peer.ack_packet().unwrap();
    }
    // The first packet fell out of the window; the latest is kept.
    assert_eq!(peer.sent_at(first), None);
    let last = peer.sequencer().out_packet_id().wrapping_sub(1);
    assert!(peer.sent_at(last).is_some());
}

/// A peer with actor channel 4 open and settled, and one naked control
/// bunch awaiting resend; its sent-record ring is empty.
fn update_peer(sent_capacity: usize) -> LabPeer {
    let mut config = PacketLabConfig::new(LIMITS);
    config.sent_capacity = sent_capacity;
    let mut peer = LabPeer::from_cookie(&COOKIE, config).unwrap();
    let mut client = Client::new();
    let (opening, _) = client.send(Vec::new());
    peer.receive(&opening).unwrap();
    let open = peer
        .reliable_actor_open(&actor_open_bunch(4, &[1]), &[])
        .unwrap();
    let lost_id = peer.sequencer().out_packet_id();
    let _lost = peer.reliable_control_packet(challenge("lost")).unwrap();
    let ack = peer.ack_packet().unwrap();
    accept_header(&mut client, &open);
    accept_header(&mut client, &ack);
    let (next, _) = client.send(Vec::new());
    assert_eq!(peer.receive(&next).unwrap().summary().naked, [lost_id]);
    assert!(peer.actor_channel_settled(4));
    assert_eq!(peer.pending_resends(), 1);
    assert!(peer.sequencer().notify().sent_records().is_empty());
    peer
}

/// The bunch an update of `payload` on channel 4 reassembles to.
fn update_bunch(payload: Payload) -> Bunch {
    Bunch {
        header: BunchHeader {
            control: false,
            open: false,
            close: false,
            close_reason: None,
            paused: false,
            reliable: false,
            channel_index: 4,
            has_package_map_exports: false,
            has_must_be_mapped_guids: false,
            partial: false,
            reliable_sequence_raw: None,
            partial_initial: false,
            partial_final: false,
            channel_name: None,
        },
        payload,
    }
}

/// A payload of `bits` bits.
fn bits_payload(bits: usize) -> Payload {
    let mut writer = BitWriter::new();
    let bytes: Vec<u8> = (0..bits / 8).map(|index| index as u8).collect();
    writer.write_bytes(&bytes);
    for bit in 0..bits % 8 {
        writer.write_bits((bit % 2) as u32, 1);
    }
    Payload::from_writer(writer)
}

/// Feed the channel-4 parts of `sent`, in order, through the client's
/// partial reassembler with each datagram's packet ID as the unreliable
/// sequence; returns the parts and the reassembled bunch, or why not.
fn reassemble_update(sent: &[(Vec<u8>, u32)]) -> (Vec<Bunch>, Result<Bunch, String>) {
    let mut reassembler = PartialBunchReassembler::new();
    let mut parts = Vec::new();
    for (datagram, packet_id) in sent {
        let bunches = read_packet(datagram, &to_client()).unwrap().bunches;
        for part in bunches.into_iter().filter(|b| b.header.channel_index == 4) {
            let received = reassembler.receive_part(
                i64::from(*packet_id),
                &part.header,
                &part.payload,
                Some(MAX_CONSTRUCTED_PARTIAL_BUNCH_BYTES),
            );
            parts.push(part);
            match received {
                Ok(PartialReceive::Complete { bunch, .. }) => return (parts, Ok(bunch)),
                Ok(_) => {}
                Err(error) => return (parts, Err(error.to_string())),
            }
        }
    }
    (parts, Err("no final part".to_owned()))
}

#[test]
fn an_update_within_one_bunch_is_the_single_update_datagram() {
    // Up to MAX_SINGLE_BUNCH_SIZE_BITS (7,625 at MaxPacket 1024) the bunch
    // is not split: the same datagram, packet ID and state as actor_update.
    for bits in [8, 7_200, 7_625] {
        let mut single = update_peer(16);
        let mut split = update_peer(16);
        let id = single.sequencer().out_packet_id();
        let datagram = single.actor_update(4, bits_payload(bits)).unwrap();
        let sent = split.actor_update_datagrams(4, bits_payload(bits)).unwrap();
        assert_eq!(sent, [(datagram.clone(), id)], "{bits}");
        assert_eq!(format!("{split:?}"), format!("{single:?}"), "{bits}");
        assert_eq!(split.sequencer().out_packet_id(), id + 1);
        let bunches = read_packet(&datagram, &to_client()).unwrap().bunches;
        let update = bunches.last().unwrap();
        assert_eq!(*update, update_bunch(bits_payload(bits)), "{bits}");
        if bits < 7_625 {
            // The naked control bunch rides along, as in actor_update.
            assert_eq!(bunches.len(), 2, "{bits}");
            assert!(bunches[0].header.reliable && bunches[0].header.channel_index == 0);
            assert_eq!(split.pending_resends(), 0, "{bits}");
        }
    }
}

#[test]
fn an_oversized_update_goes_out_as_unreliable_partials_on_consecutive_packets() {
    let mut peer = update_peer(16);
    let first_id = peer.sequencer().out_packet_id();
    let first_seq = peer.sequencer().notify().out_seq();
    let payload = bits_payload(16_000);
    let sent = peer.actor_update_datagrams(4, payload.clone()).unwrap();
    // 7,624 + 7,624 + 752 bits: no two parts fit one 1,024-byte packet.
    let ids: Vec<u32> = sent.iter().map(|(_, id)| *id).collect();
    assert_eq!(ids, [first_id, first_id + 1, first_id + 2]);
    assert_eq!(peer.sequencer().out_packet_id(), first_id + 3);
    assert_eq!(peer.pending_actor_reliable(4), Some(0), "nothing retained");
    assert_eq!(peer.pending_resends(), 0);
    for (offset, (datagram, _)) in sent.iter().enumerate() {
        assert!(datagram.len() <= LIMITS.max_packet_bytes as usize);
        let packet = read_packet(datagram, &to_client()).unwrap();
        assert_eq!(packet.header.sequence, first_seq + offset as u16);
    }
    // Only the first datagram carries the naked control bunch, before its part.
    let first = read_packet(&sent[0].0, &to_client()).unwrap().bunches;
    assert_eq!(first.len(), 2);
    assert!(first[0].header.reliable && first[0].header.channel_index == 0);

    let (parts, bunch) = reassemble_update(&sent);
    let flags: Vec<[bool; 4]> = parts
        .iter()
        .map(|part| {
            let header = &part.header;
            [
                header.partial,
                header.partial_initial,
                header.partial_final,
                header.reliable,
            ]
        })
        .collect();
    assert_eq!(
        flags,
        [
            [true, true, false, false],
            [true, false, false, false],
            [true, false, true, false]
        ]
    );
    for part in &parts {
        let header = &part.header;
        assert!(!header.open && !header.close && !header.control);
        assert_eq!(header.reliable_sequence_raw, None);
        assert_eq!(header.channel_name, None);
        assert!(!header.has_package_map_exports && !header.has_must_be_mapped_guids);
    }
    let bits: Vec<usize> = parts.iter().map(|part| part.payload.bit_len()).collect();
    assert_eq!(bits, [7_624, 7_624, 752]);
    assert_eq!(bunch.unwrap(), update_bunch(payload));

    // A lost middle datagram is a gap: the client drops the whole bunch.
    let gap = [sent[0].clone(), sent[2].clone()];
    assert!(reassemble_update(&gap).1.is_err());
}

#[test]
fn update_datagram_failures_leave_the_state_unchanged() {
    // A ring with room for three datagrams.
    let mut peer = update_peer(3);
    let before = format!("{peer:?}");
    let out_id = peer.sequencer().out_packet_id();
    let attempts = [
        (4, bits_payload(24_000), "four datagrams, room for three"),
        (
            4,
            bits_payload((MAX_CONSTRUCTED_PARTIAL_BUNCH_BYTES + 1) * 8),
            "over 64 KiB",
        ),
        (5, bits_payload(8), "channel not open"),
    ];
    for (index, payload, why) in attempts {
        assert!(
            peer.actor_update_datagrams(index, payload).is_err(),
            "{why}"
        );
        assert_eq!(format!("{peer:?}"), before, "{why}");
        assert_eq!(peer.sequencer().out_packet_id(), out_id, "{why}");
        assert!(!peer.is_failed(), "{why}");
    }
    // Three parts fill the ring exactly.
    let sent = peer
        .actor_update_datagrams(4, bits_payload(16_000))
        .unwrap();
    assert_eq!(sent.len(), 3);
    assert_eq!(peer.pending_resends(), 0);

    // The 64 KiB bound itself is sent: 69 consecutive datagrams.
    let mut roomy = update_peer(256);
    let first_id = roomy.sequencer().out_packet_id();
    let payload = bits_payload(MAX_CONSTRUCTED_PARTIAL_BUNCH_BYTES * 8);
    let sent = roomy.actor_update_datagrams(4, payload.clone()).unwrap();
    assert_eq!(sent.len(), 69);
    assert!(
        sent.iter()
            .enumerate()
            .all(|(offset, (_, id))| *id == first_id + offset as u32)
    );
    assert_eq!(reassemble_update(&sent).1.unwrap(), update_bunch(payload));
}

#[test]
fn reliable_profile_sized_updates_match_single_bunch_framing() {
    for bits in [0, 4000, 7625] {
        let mut single = update_peer(16);
        let mut split = update_peer(16);
        let id = single.sequencer().out_packet_id();
        let expected = single.reliable_actor_bunch(4, bits_payload(bits)).unwrap();
        let sent = split
            .reliable_actor_bunch_datagrams(4, bits_payload(bits))
            .unwrap();
        assert_eq!(sent.datagrams, [expected]);
        assert_eq!(sent.packet_ids, [id]);
        assert_eq!(sent.exports_packet_id, None);
        assert!(!format!("{sent:?}").contains(&format!("{:?}", sent.datagrams)));
        assert_eq!(format!("{split:?}"), format!("{single:?}"));
    }
}

#[test]
fn reliable_profile_updates_split_and_repair_lost_parts() {
    let mut peer = peer();
    let mut client = Client::new();
    let opened = peer
        .reliable_actor_open(&actor_open_bunch(4, &[1]), &[])
        .unwrap();
    accept_header(&mut client, &opened);
    let (ack, _) = client.send(Vec::new());
    peer.receive(&ack).unwrap();
    assert!(peer.actor_channel_settled(4));
    let payload = bits_payload(16_000);
    let sent = peer
        .reliable_actor_bunch_datagrams(4, payload.clone())
        .unwrap();
    assert_eq!(sent.datagrams.len(), 3);
    assert_eq!(peer.pending_actor_reliable(4), Some(3));
    let mut reassembler = PartialBunchReassembler::new();
    let mut complete = None;
    let mut originals = Vec::new();
    for (index, datagram) in sent.datagrams.iter().enumerate() {
        let bunches = read_packet(datagram, &to_client()).unwrap().bunches;
        assert_eq!(bunches.len(), 1);
        let part = &bunches[0];
        assert!(part.header.reliable && part.header.partial && !part.header.open);
        let received = reassembler
            .receive_part(
                index as i64 + 1,
                &part.header,
                &part.payload,
                Some(MAX_CONSTRUCTED_PARTIAL_BUNCH_BYTES),
            )
            .unwrap();
        if let PartialReceive::Complete { bunch, .. } = received {
            complete = Some(bunch);
        }
        originals.push(part.clone());
    }
    assert_eq!(complete.unwrap().payload, payload);
    // Losing the middle packet schedules exactly its reliable part for resend.
    accept_header(&mut client, &sent.datagrams[0]);
    accept_header(&mut client, &sent.datagrams[2]);
    let (ack, _) = client.send(Vec::new());
    let received = peer.receive(&ack).unwrap();
    let summary = received.summary();
    assert_eq!(summary.naked, [sent.packet_ids[1]]);
    assert_eq!(peer.pending_resends(), 1);
    let repaired = peer.ack_packet().unwrap();
    let repaired = read_packet(&repaired, &to_client()).unwrap();
    assert_eq!(repaired.bunches, [originals[1].clone()]);
}

#[test]
fn reliable_profile_update_preflight_failures_leave_channel_state_unchanged() {
    let mut peer = update_peer(3);
    for (channel, bits) in [
        (5, 8),
        (4, 24_000),
        (4, (MAX_CONSTRUCTED_PARTIAL_BUNCH_BYTES + 1) * 8),
    ] {
        let before = format!("{peer:?}");
        let next = peer.sequencer().out_packet_id();
        let error = peer
            .reliable_actor_bunch_datagrams(channel, bits_payload(bits))
            .unwrap_err();
        let capacity = error.downcast_ref::<SendCapacityError>();
        if channel == 4 && bits == 24_000 {
            assert_eq!(
                capacity,
                Some(&SendCapacityError::SentRecordRing { capacity: 3 })
            );
            assert_eq!(error.to_string(), "sent-record ring is full (3 records)");
            // This update cannot fit even though there is no sent traffic
            // left to acknowledge. Its typed error is not a retry promise.
            assert!(peer.sequencer().notify().sent_records().is_empty());
        } else {
            assert!(
                capacity.is_none(),
                "semantic/size errors are not queue pressure"
            );
        }
        assert_eq!(format!("{peer:?}"), before);
        assert_eq!(peer.sequencer().out_packet_id(), next);
        assert!(!peer.is_failed());
    }
    assert_eq!(
        peer.reliable_actor_bunch_datagrams(4, bits_payload(16_000))
            .unwrap()
            .datagrams
            .len(),
        3
    );
}

#[test]
fn reliable_actor_sends_classify_live_ring_pressure_without_committing() {
    let mut peer = update_peer(3);
    for _ in 0..3 {
        peer.reliable_actor_bunch(4, bits_payload(8)).unwrap();
    }
    assert_eq!(peer.sequencer().notify().sent_records().len(), 3);
    let before = format!("{peer:?}");
    let next = peer.sequencer().out_packet_id();
    let errors = [
        peer.reliable_actor_bunch(4, bits_payload(8)).unwrap_err(),
        peer.reliable_actor_bunch_datagrams(4, bits_payload(16_000))
            .unwrap_err(),
    ];
    for error in errors {
        assert_eq!(
            error.downcast_ref::<SendCapacityError>(),
            Some(&SendCapacityError::SentRecordRing { capacity: 3 })
        );
        assert_eq!(error.to_string(), "sent-record ring is full (3 records)");
    }
    assert_eq!(format!("{peer:?}"), before);
    assert_eq!(peer.sequencer().out_packet_id(), next);
    assert!(!peer.is_failed());
}

#[test]
fn reliable_control_sends_classify_channel_pressure_without_committing() {
    let mut config = PacketLabConfig::new(LIMITS);
    config.sent_capacity = MAX_PENDING_RELIABLE + 1;
    let mut peer = LabPeer::from_cookie(&COOKIE, config).unwrap();
    for _ in 0..MAX_PENDING_RELIABLE {
        peer.reliable_control_packet(challenge("pending")).unwrap();
    }
    let before = format!("{peer:?}");
    let next = peer.sequencer().out_packet_id();
    for error in [
        peer.reliable_control_packet(challenge("refused"))
            .unwrap_err(),
        peer.reliable_control_close(0).unwrap_err(),
    ] {
        assert_eq!(
            error.downcast_ref::<SendCapacityError>(),
            Some(&SendCapacityError::ReliableControlQueue {
                capacity: MAX_PENDING_RELIABLE
            })
        );
        assert_eq!(
            error.to_string(),
            "255 outgoing reliable control bunches are unacknowledged"
        );
    }
    assert_eq!(format!("{peer:?}"), before);
    assert_eq!(peer.sequencer().out_packet_id(), next);
    assert!(!peer.is_failed());
}

#[test]
fn reliable_actor_sends_classify_channel_pressure_without_committing() {
    let mut peer = update_peer(MAX_PENDING_RELIABLE + 2);
    for _ in 0..MAX_PENDING_RELIABLE {
        peer.reliable_actor_bunch(4, bits_payload(8)).unwrap();
    }
    assert_eq!(peer.pending_actor_reliable(4), Some(MAX_PENDING_RELIABLE));
    let before = format!("{peer:?}");
    let next = peer.sequencer().out_packet_id();
    let single = peer.reliable_actor_bunch(4, bits_payload(8)).unwrap_err();
    assert_eq!(
        single.downcast_ref::<SendCapacityError>(),
        Some(&SendCapacityError::ReliableBunchQueue {
            capacity: MAX_PENDING_RELIABLE
        })
    );
    assert_eq!(
        single.to_string(),
        "255 outgoing reliable bunches are unacknowledged"
    );
    let multipart = peer
        .reliable_actor_bunch_datagrams(4, bits_payload(16_000))
        .unwrap_err();
    assert_eq!(
        multipart.downcast_ref::<SendCapacityError>(),
        Some(&SendCapacityError::ReliableActorWindow {
            pending: MAX_PENDING_RELIABLE,
            additional: 3,
            capacity: MAX_PENDING_RELIABLE,
        })
    );
    assert_eq!(multipart.to_string(), "reliable actor window is full");
    assert_eq!(format!("{peer:?}"), before);
    assert_eq!(peer.sequencer().out_packet_id(), next);
    assert!(!peer.is_failed());
}

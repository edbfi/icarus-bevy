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
    Notification, PacketSequencer, ReliableChannel, ReliableReceive, wire_reliable_sequence,
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

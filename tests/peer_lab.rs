use std::{
    io::ErrorKind,
    net::{SocketAddr, SocketAddrV4, UdpSocket},
    thread,
    time::Duration,
};

use bevy::app::App;
use icarus_bevy::{
    config::ServerConfig,
    experimental::{
        control_messages::{
            ArchiveText, ControlCodec, ControlMessage, HELLO, JOIN, NETSPEED,
            encode_control_message,
        },
        handshake::{LegacyHandshake, decode, encode, initial_request},
        handshake_lab::PACKETS_PER_UPDATE,
        packet::{
            BitWriter, Bunch, BunchHeader, Name, Packet, PacketConfig, Payload, Receiver,
            read_packet, write_packet,
        },
        packet_lab::client_cookie_seeds,
        peer_lab::{PeerLab, PeerLabConfig, PeerLabPlugin, PeerLabStats},
        sequencing::{Notification, PacketSequencer, ReliableChannel, wire_reliable_sequence},
    },
    simulation::{WorldSnapshot, build_app, capture},
};

// Independently authored synthetic client built from the experimental codec and
// the client sequencing model; no captured client or server traffic. Passing
// these tests shows only that the lab agrees with this repository's own model.

const SENTINEL: &str = "SENTINEL-TOKEN-7f3a";

fn lab_app(config: PeerLabConfig) -> (App, SocketAddr) {
    let mut app = build_app(&ServerConfig::default(), &WorldSnapshot::default()).unwrap();
    let lab = PeerLab::bind(0, config).unwrap();
    let address = lab.local_addr().unwrap();
    assert!(address.ip().is_loopback());
    app.insert_resource(lab).add_plugins(PeerLabPlugin);
    (app, address)
}

fn stats(app: &App) -> PeerLabStats {
    app.world().resource::<PeerLab>().stats().clone()
}

fn lab(app: &App) -> &PeerLab {
    app.world().resource::<PeerLab>()
}

fn to_server() -> PacketConfig {
    PacketConfig {
        magic_bits: 0,
        receiver: Receiver::Server,
        limits: PeerLabConfig::default().packet.packet.limits,
    }
}

fn to_client() -> PacketConfig {
    PacketConfig {
        receiver: Receiver::Client,
        ..to_server()
    }
}

fn hello() -> ControlMessage {
    ControlMessage::Hello {
        first_field: 1,
        version: 0x0102_0304,
        token: ArchiveText::text(SENTINEL).unwrap(),
    }
}

fn control_bunch(reliable_raw: Option<u32>, open: bool, message: &ControlMessage) -> Bunch {
    let mut writer = BitWriter::new();
    writer.write_bytes(&encode_control_message(message, &ControlCodec::default()).unwrap());
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
        payload: Payload::from_writer(writer),
    }
}

/// A loopback UDP socket plus the client-side sequencing model.
struct Client {
    socket: UdpSocket,
    cookie: [u8; 20],
    sequencer: PacketSequencer,
    control: ReliableChannel<ControlMessage>,
}

fn socket(lab: SocketAddr) -> UdpSocket {
    let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    socket.connect(lab).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(1)))
        .unwrap();
    socket
}

/// The lab sends synchronously inside `app.update()`, so a reply is already
/// queued on loopback when this runs.
fn try_recv(socket: &UdpSocket) -> Option<Vec<u8>> {
    socket.set_nonblocking(true).unwrap();
    let mut buffer = [0; 2048];
    let result = socket.recv(&mut buffer);
    socket.set_nonblocking(false).unwrap();
    match result {
        Ok(length) => Some(buffer[..length].to_vec()),
        Err(error) if error.kind() == ErrorKind::WouldBlock => None,
        Err(error) => panic!("{error}"),
    }
}

fn recv(socket: &UdpSocket) -> Vec<u8> {
    try_recv(socket).expect("lab reply")
}

/// Request and echo a challenge; returns the echoed challenge datagram and
/// whether the lab acknowledged it.
fn handshake(app: &mut App, socket: &UdpSocket) -> ([u8; 20], Option<Vec<u8>>) {
    socket.send(&initial_request()).unwrap();
    app.update();
    let challenge = recv(socket);
    let cookie = decode(&challenge).unwrap().cookie;
    socket.send(&challenge).unwrap();
    app.update();
    (cookie, try_recv(socket))
}

impl Client {
    fn connect(app: &mut App, lab: SocketAddr) -> Self {
        let socket = socket(lab);
        let (cookie, ack) = handshake(app, &socket);
        let ack = decode(&ack.expect("handshake acknowledgement")).unwrap();
        assert_eq!((ack.timestamp, ack.secret_id), (-1.0, true));
        assert_eq!(ack.cookie, cookie);
        let (incoming, outgoing) = client_cookie_seeds(&cookie);
        let sequencer = PacketSequencer::new(incoming, outgoing, 64).unwrap();
        let control = sequencer.new_channel();
        Self {
            socket,
            cookie,
            sequencer,
            control,
        }
    }

    fn address(&self) -> SocketAddrV4 {
        match self.socket.local_addr().unwrap() {
            SocketAddr::V4(address) => address,
            SocketAddr::V6(_) => unreachable!("IPv4 loopback client"),
        }
    }

    /// Build and commit a datagram; returns it and its packet ID.
    fn packet(&mut self, bunches: Vec<Bunch>) -> (Vec<u8>, u32) {
        let packet = Packet {
            magic: 0,
            header: self.sequencer.header().unwrap(),
            info: None,
            bunches,
        };
        let bytes = write_packet(&packet, &to_server()).unwrap();
        (bytes, self.sequencer.commit().unwrap())
    }

    fn reliable_bunch(&mut self, message: ControlMessage, open: bool) -> Bunch {
        let packet_id = self.sequencer.out_packet_id();
        let sequence = self
            .control
            .send_reliable(packet_id, message.clone())
            .unwrap();
        control_bunch(Some(wire_reliable_sequence(sequence)), open, &message)
    }

    fn send(&self, datagram: &[u8]) {
        self.socket.send(datagram).unwrap();
    }

    /// Parse a lab ack datagram as the client and return its notifications.
    fn receive_ack(&mut self, datagram: &[u8]) -> Vec<Notification> {
        let packet = read_packet(datagram, &to_client()).unwrap();
        assert!(packet.bunches.is_empty());
        let received = self.sequencer.receive(&packet.header).unwrap().unwrap();
        self.sequencer.ack_seq(received.sequence, true).unwrap();
        received.notifications
    }
}

#[test]
fn handshake_creates_peer_and_control_packets_are_acked() {
    let (mut app, address) = lab_app(PeerLabConfig::default());
    let mut client = Client::connect(&mut app, address);
    let after_handshake = stats(&app);
    assert_eq!(after_handshake.challenges_sent, 1);
    assert_eq!(after_handshake.acknowledgements_sent, 1);
    assert_eq!(after_handshake.peers_created, 1);
    assert!(lab(&app).has_peer(client.address()));

    // A reliable Hello opening the control channel.
    let bunch = client.reliable_bunch(hello(), true);
    let (first, first_id) = client.packet(vec![bunch]);
    client.send(&first);
    app.update();
    assert_eq!(
        client.receive_ack(&recv(&client.socket)),
        [Notification {
            packet_id: first_id,
            delivered: true
        }]
    );
    let after_hello = stats(&app);
    assert_eq!((after_hello.data_accepted, after_hello.acks_sent), (1, 1));
    assert_eq!(after_hello.control_delivered, 1);
    assert_eq!(after_hello.control_by_id.get(&HELLO), Some(&1));

    // Two packets in one update are acknowledged by one coalesced ack.
    let netspeed = client.reliable_bunch(ControlMessage::Netspeed { rate: 100_000 }, false);
    let (second, second_id) = client.packet(vec![netspeed]);
    let (third, third_id) = client.packet(vec![control_bunch(None, false, &ControlMessage::Join)]);
    client.send(&second);
    client.send(&third);
    app.update();
    let notifications = client.receive_ack(&recv(&client.socket));
    assert!(try_recv(&client.socket).is_none());
    assert_eq!(
        notifications,
        [
            Notification {
                packet_id: second_id,
                delivered: true
            },
            Notification {
                packet_id: third_id,
                delivered: true
            }
        ]
    );
    let after_more = stats(&app);
    assert_eq!((after_more.data_accepted, after_more.acks_sent), (3, 2));
    assert_eq!(after_more.control_delivered, 3);
    assert_eq!(
        after_more.control_by_id.into_iter().collect::<Vec<_>>(),
        [(HELLO, 1), (NETSPEED, 1), (JOIN, 1)]
    );

    // A duplicate is rejected by the sequence gate and not acknowledged.
    client.send(&first);
    app.update();
    assert!(try_recv(&client.socket).is_none());
    let after_duplicate = stats(&app);
    assert_eq!(after_duplicate.data_rejected, 1);
    assert_eq!(after_duplicate.acks_sent, 2);
    assert_eq!(after_duplicate.control_delivered, 3);
    assert_eq!(after_duplicate.peers_failed, 0);

    // Nothing leaks: no token, numeric field, cookie bytes or address.
    let cookie_hex: String = client.cookie.iter().map(|b| format!("{b:02x}")).collect();
    for text in [
        format!("{:?}", lab(&app)),
        format!("{after_duplicate:?}"),
        serde_json::to_string(&after_duplicate).unwrap(),
    ] {
        assert!(!text.contains(SENTINEL), "{text}");
        assert!(!text.contains("16909060"), "{text}");
        assert!(!text.contains("100000"), "{text}");
        assert!(!text.contains(&cookie_hex), "{text}");
        assert!(!text.contains("127.0.0.1"), "{text}");
    }
    // The lab created no game entities.
    assert!(capture(app.world_mut()).unwrap().entities.is_empty());
}

#[test]
fn data_from_unknown_peer_is_rejected_without_reply() {
    let (mut app, address) = lab_app(PeerLabConfig::default());
    let stranger = socket(address);
    let cookie = [
        0x34, 0x12, 0x05, 0xc4, 9, 8, 7, 6, 5, 4, 3, 2, 1, 0, 1, 2, 3, 4, 5, 6,
    ];
    let (incoming, outgoing) = client_cookie_seeds(&cookie);
    let mut sequencer = PacketSequencer::new(incoming, outgoing, 8).unwrap();
    let datagram = write_packet(
        &Packet {
            magic: 0,
            header: sequencer.header().unwrap(),
            info: None,
            bunches: vec![control_bunch(None, false, &ControlMessage::Join)],
        },
        &to_server(),
    )
    .unwrap();
    sequencer.commit().unwrap();
    stranger.send(&datagram).unwrap();
    stranger.send(&[0x00]).unwrap();
    app.update();
    assert!(try_recv(&stranger).is_none());
    let stats = stats(&app);
    assert_eq!(stats.unknown_peer, 2);
    assert_eq!(
        stats.data_accepted + stats.acks_sent + stats.peers_created,
        0
    );
    assert_eq!(lab(&app).peer_count(), 0);
}

#[test]
fn full_peer_table_rejects_new_peers_and_allows_replacement() {
    let config = PeerLabConfig {
        max_peers: 2,
        ..PeerLabConfig::default()
    };
    let (mut app, address) = lab_app(config);
    let mut first = Client::connect(&mut app, address);
    let second = Client::connect(&mut app, address);
    let third = socket(address);
    let (_, ack) = handshake(&mut app, &third);
    assert!(ack.is_none(), "full table withholds the acknowledgement");
    let full = stats(&app);
    assert_eq!((full.peers_created, full.peers_rejected), (2, 1));
    assert_eq!(full.challenges_sent, 3);
    assert_eq!(full.acknowledgements_sent, 2);
    assert_eq!(lab(&app).peer_count(), 2);
    assert!(lab(&app).has_peer(first.address()) && lab(&app).has_peer(second.address()));
    // The rejected address stays unknown.
    third.send(&[0x00]).unwrap();
    app.update();
    assert_eq!(stats(&app).unknown_peer, 1);

    // A known address that completes the handshake again replaces its peer,
    // even at capacity; the new cookie reseeds its sequences.
    let (cookie, ack) = handshake(&mut app, &first.socket);
    assert!(ack.is_some());
    let (incoming, outgoing) = client_cookie_seeds(&cookie);
    first.sequencer = PacketSequencer::new(incoming, outgoing, 64).unwrap();
    first.control = first.sequencer.new_channel();
    let replaced = stats(&app);
    assert_eq!((replaced.peers_replaced, replaced.peers_created), (1, 2));
    assert_eq!(lab(&app).peer_count(), 2);
    let bunch = first.reliable_bunch(hello(), true);
    let (datagram, packet_id) = first.packet(vec![bunch]);
    first.send(&datagram);
    app.update();
    assert_eq!(
        first.receive_ack(&recv(&first.socket)),
        [Notification {
            packet_id,
            delivered: true
        }]
    );
}

#[test]
fn idle_peers_expire() {
    let config = PeerLabConfig {
        idle_timeout: Duration::from_millis(50),
        ..PeerLabConfig::default()
    };
    let (mut app, address) = lab_app(config);
    let mut client = Client::connect(&mut app, address);
    assert!(lab(&app).has_peer(client.address()));
    thread::sleep(Duration::from_millis(150));
    app.update();
    let expired = stats(&app);
    assert_eq!(expired.peers_expired, 1);
    assert_eq!(lab(&app).peer_count(), 0);
    let (datagram, _) = client.packet(Vec::new());
    client.send(&datagram);
    app.update();
    assert!(try_recv(&client.socket).is_none());
    assert_eq!(stats(&app).unknown_peer, 1);
    // A fresh handshake creates the peer again.
    let (_, ack) = handshake(&mut app, &client.socket);
    assert!(ack.is_some());
    assert_eq!(stats(&app).peers_created, 2);
}

#[test]
fn malformed_and_oversize_datagrams_are_counted_without_failing_the_peer() {
    let (mut app, address) = lab_app(PeerLabConfig::default());
    let mut client = Client::connect(&mut app, address);
    let baseline = stats(&app).received;
    let max = PeerLabConfig::default().max_datagram_bytes();
    let (valid, valid_id) = client.packet(Vec::new());

    // Oversize: one byte over the maximum and a much larger datagram whose
    // valid prefix must not be accepted after kernel truncation.
    let mut long = valid.clone();
    long.resize(max + 1, 0xff);
    client.send(&long);
    client.send(&vec![0xff; 4000]);
    // Malformed data from the known peer.
    client.send(&[]);
    client.send(&[0x00]);
    client.send(&valid[..valid.len() - 1]);
    // An exact handshake form with a forged cookie, and an acknowledgement form.
    for (secret_id, timestamp) in [(false, 1.0), (true, -1.0)] {
        client.send(
            &encode(&LegacyHandshake {
                restart: false,
                secret_id,
                timestamp,
                cookie: [0; 20],
            })
            .unwrap(),
        );
    }
    app.update();
    assert!(try_recv(&client.socket).is_none());
    let counted = stats(&app);
    assert_eq!(counted.received - baseline, 7);
    assert_eq!(counted.oversize, 2);
    assert_eq!(counted.data_malformed, 3);
    assert_eq!(counted.handshake_rejected, 2);
    assert_eq!((counted.peers_failed, counted.data_accepted), (0, 0));
    assert!(lab(&app).has_peer(client.address()));

    // The peer still accepts its packet.
    client.send(&valid);
    app.update();
    assert_eq!(
        client.receive_ack(&recv(&client.socket)),
        [Notification {
            packet_id: valid_id,
            delivered: true
        }]
    );

    // The per-update datagram bound still applies.
    for _ in 0..PACKETS_PER_UPDATE + 1 {
        client.send(&long);
    }
    let before = stats(&app).received;
    app.update();
    assert_eq!(stats(&app).received - before, PACKETS_PER_UPDATE as u64);
    app.update();
    assert_eq!(stats(&app).received - before, PACKETS_PER_UPDATE as u64 + 1);
    assert_eq!(stats(&app).oversize, 2 + PACKETS_PER_UPDATE as u64 + 1);
}

#[test]
fn failed_peer_is_dropped() {
    let (mut app, address) = lab_app(PeerLabConfig::default());
    let mut client = Client::connect(&mut app, address);
    // Lose the first reliable bunch, then overflow the peer's reliable queue.
    let _lost = client.reliable_bunch(ControlMessage::Join, true);
    let _ = client.packet(Vec::new());
    let mut datagrams = 0;
    while stats(&app).peers_failed == 0 {
        assert!(datagrams < 16, "reliable queue never overflowed");
        let bunches = (0..32)
            .map(|_| client.reliable_bunch(ControlMessage::Join, false))
            .collect();
        let (datagram, _) = client.packet(bunches);
        assert!(datagram.len() <= PeerLabConfig::default().max_datagram_bytes());
        client.send(&datagram);
        app.update();
        while try_recv(&client.socket).is_some() {}
        datagrams += 1;
    }
    let failed = stats(&app);
    assert_eq!(failed.peers_failed, 1);
    assert_eq!(failed.data_malformed, 1);
    assert_eq!(failed.data_accepted, datagrams - 1);
    assert_eq!(failed.control_delivered, 0);
    assert_eq!(lab(&app).peer_count(), 0);
    let (next, _) = client.packet(Vec::new());
    client.send(&next);
    app.update();
    assert_eq!(stats(&app).unknown_peer, 1);
}

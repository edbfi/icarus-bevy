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
        admission::{
            AdmissionPolicy, FIND_PLAYER_START_ERROR, INCOMPATIBLE_UNIQUE_NET_ID, SERVER_FULL,
        },
        control_messages::{
            ArchiveText, ControlCodec, ControlMessage, Decoded, HELLO, JOIN, LOGIN, NETSPEED,
            NetIdType, NetIdValue, UniqueNetId, encode_control_message, read_control_messages,
        },
        handshake::{LegacyHandshake, decode, encode, initial_request},
        handshake_lab::PACKETS_PER_UPDATE,
        packet::{
            BitWriter, Bunch, BunchHeader, Name, Packet, PacketConfig, Payload, Receiver,
            read_packet, write_packet,
        },
        packet_lab::client_cookie_seeds,
        peer_lab::{
            ADMISSION_DISABLED_MESSAGE, PeerLab, PeerLabAdmission, PeerLabConfig, PeerLabPlugin,
            PeerLabStats,
        },
        sequencing::{
            Notification, PacketSequencer, ReliableChannel, ReliableReceive, wire_reliable_sequence,
        },
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
        let (notifications, messages) = self.receive_reply(datagram);
        assert!(messages.is_empty());
        notifications
    }

    /// Parse a lab datagram as the client: the header through the client
    /// sequencing model and every bunch as a reliable control bunch decoded
    /// with the client-side readers. Returns notifications and the control
    /// messages released in order.
    fn receive_reply(&mut self, datagram: &[u8]) -> (Vec<Notification>, Vec<ControlMessage>) {
        let packet = read_packet(datagram, &to_client()).unwrap();
        let received = self.sequencer.receive(&packet.header).unwrap().unwrap();
        let mut messages = Vec::new();
        for bunch in &packet.bunches {
            let header = &bunch.header;
            assert!(header.reliable && !header.open && !header.close);
            assert_eq!(header.channel_index, 0);
            let decoded =
                read_control_messages(&mut bunch.payload.reader(), &ControlCodec::default())
                    .unwrap();
            let [Decoded::Message(message)] = decoded.as_slice() else {
                panic!("one known control message per lab bunch");
            };
            let raw = header.reliable_sequence_raw.unwrap();
            if let ReliableReceive::Delivered(batch) =
                self.control.receive_reliable(raw, message.clone()).unwrap()
            {
                messages.extend(batch);
            }
        }
        self.sequencer.ack_seq(received.sequence, true).unwrap();
        (received.notifications, messages)
    }

    /// Send `message` reliably in its own packet and run one lab update.
    fn exchange(&mut self, app: &mut App, message: ControlMessage, open: bool) -> u32 {
        let bunch = self.reliable_bunch(message, open);
        let (datagram, packet_id) = self.packet(vec![bunch]);
        self.send(&datagram);
        app.update();
        packet_id
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

const URL_SENTINEL: &str = "SENTINEL-URL-91c2?Name=Player";
const PLATFORM_SENTINEL: &str = "SENTINEL-PLATFORM-44d0";
const NET_ID_HEX: &str = "0123456789ABCDEF7654";
const LOCAL_VERSION: u32 = 0x0a0b_0c0d;

fn short_hello(token: &str, version: u32) -> ControlMessage {
    ControlMessage::Hello {
        first_field: 1,
        version,
        token: ArchiveText::text(token).unwrap(),
    }
}

fn login() -> ControlMessage {
    ControlMessage::Login {
        response: ArchiveText::text("0").unwrap(),
        url: ArchiveText::text(URL_SENTINEL).unwrap(),
        unique_net_id: UniqueNetId {
            kind: NetIdType::Default,
            value: NetIdValue::from_hex(NET_ID_HEX).unwrap(),
        },
        platform: ArchiveText::text(PLATFORM_SENTINEL).unwrap(),
    }
}

/// Every reportable rendering of the lab: Debug, stats Debug and JSON, config.
fn reports(app: &App) -> Vec<String> {
    let stats = stats(app);
    vec![
        format!("{:?}", lab(app)),
        format!("{stats:?}"),
        serde_json::to_string(&stats).unwrap(),
        format!("{:?}", lab(app).config()),
    ]
}

fn assert_no_secrets(app: &App, secrets: &[&str]) {
    for text in reports(app) {
        for secret in secrets {
            assert!(!text.contains(secret), "{secret} in {text}");
        }
    }
}

#[test]
fn hello_gets_a_challenge_and_login_gets_failure_then_close() {
    let (mut app, address) = lab_app(PeerLabConfig::default());
    let mut client = Client::connect(&mut app, address);

    // A compatible Hello (no local version configured) with an empty token.
    let hello_id = client.exchange(&mut app, short_hello("", 0x0102_0304), true);
    let (notifications, messages) = client.receive_reply(&recv(&client.socket));
    assert!(
        try_recv(&client.socket).is_none(),
        "one datagram per update"
    );
    assert_eq!(
        notifications,
        [Notification {
            packet_id: hello_id,
            delivered: true
        }]
    );
    let [ControlMessage::Challenge { token }] = messages.as_slice() else {
        panic!("expected one Challenge, got {messages:?}");
    };
    let challenge = String::from_utf16(token.units()).unwrap();
    assert_eq!(challenge.len(), 32);
    assert!(challenge.bytes().all(|byte| byte.is_ascii_hexdigit()));
    let after_hello = stats(&app);
    assert_eq!(after_hello.control_challenges_sent, 1);
    assert_eq!(after_hello.acks_sent, 0, "the reply carried the ack");
    assert_eq!(after_hello.control_by_id.get(&HELLO), Some(&1));

    // Each empty-token Hello is answered with a fresh lab string.
    client.exchange(&mut app, short_hello("", 0x0102_0304), false);
    let (_, messages) = client.receive_reply(&recv(&client.socket));
    let [ControlMessage::Challenge { token: second }] = messages.as_slice() else {
        panic!("expected one Challenge");
    };
    assert_ne!(second, token);
    let second = String::from_utf16(second.units()).unwrap();
    assert_eq!(stats(&app).control_challenges_sent, 2);

    // Login: counted, answered with Failure, then the peer is removed.
    client.exchange(&mut app, login(), false);
    let (_, messages) = client.receive_reply(&recv(&client.socket));
    assert_eq!(
        messages,
        [ControlMessage::Failure {
            message: ArchiveText::text(ADMISSION_DISABLED_MESSAGE).unwrap()
        }]
    );
    assert!(try_recv(&client.socket).is_none());
    let closed = stats(&app);
    assert_eq!(closed.logins_received, 1);
    assert_eq!(closed.control_by_id.get(&LOGIN), Some(&1));
    assert_eq!(
        closed.login_net_id_forms.into_iter().collect::<Vec<_>>(),
        [("encoded", 1)]
    );
    assert_eq!((closed.failures_sent, closed.peers_closed), (1, 1));
    assert_eq!((closed.welcomes_sent, closed.upgrades_sent), (0, 0));
    assert_eq!(closed.peers_failed, 0);
    assert!(!lab(&app).has_peer(client.address()));

    // The removed peer's later packets are unknown and unanswered.
    let (next, _) = client.packet(Vec::new());
    client.send(&next);
    app.update();
    assert!(try_recv(&client.socket).is_none());
    assert_eq!(stats(&app).unknown_peer, 1);

    let cookie_hex: String = client.cookie.iter().map(|b| format!("{b:02x}")).collect();
    assert_no_secrets(
        &app,
        &[
            &challenge,
            &second,
            URL_SENTINEL,
            PLATFORM_SENTINEL,
            NET_ID_HEX,
            &NET_ID_HEX.to_lowercase(),
            &cookie_hex,
            "16909060",
            "127.0.0.1",
        ],
    );
    assert!(capture(app.world_mut()).unwrap().entities.is_empty());
}

#[test]
fn mismatched_version_gets_upgrade_then_close() {
    let config = PeerLabConfig {
        local_network_version: Some(LOCAL_VERSION),
        ..PeerLabConfig::default()
    };
    let (mut app, address) = lab_app(config);

    // The configured version is compatible: a short token gets a Challenge.
    let mut matching = Client::connect(&mut app, address);
    matching.exchange(&mut app, short_hello("", LOCAL_VERSION), true);
    let (_, messages) = matching.receive_reply(&recv(&matching.socket));
    assert!(matches!(
        messages.as_slice(),
        [ControlMessage::Challenge { .. }]
    ));

    // A different version gets Upgrade carrying the local version; a Login
    // queued behind it in the same delivery is not processed.
    let mut client = Client::connect(&mut app, address);
    let hello = client.reliable_bunch(short_hello("", 0x0102_0304), true);
    let login = client.reliable_bunch(login(), false);
    let (datagram, _) = client.packet(vec![hello, login]);
    client.send(&datagram);
    app.update();
    let (_, messages) = client.receive_reply(&recv(&client.socket));
    assert_eq!(
        messages,
        [ControlMessage::Upgrade {
            version: LOCAL_VERSION
        }]
    );
    assert!(try_recv(&client.socket).is_none());
    let closed = stats(&app);
    assert_eq!((closed.upgrades_sent, closed.peers_closed), (1, 1));
    assert_eq!(closed.control_challenges_sent, 1);
    assert_eq!((closed.logins_received, closed.failures_sent), (0, 0));
    assert!(!lab(&app).has_peer(client.address()));
    assert!(lab(&app).has_peer(matching.address()));
    assert_no_secrets(&app, &["168496141", "16909060", "a0b0c0d", "A0B0C0D"]);
}

#[test]
fn long_hello_token_is_counted_and_not_answered() {
    let (mut app, address) = lab_app(PeerLabConfig::default());
    let mut client = Client::connect(&mut app, address);
    let packet_id = client.exchange(&mut app, hello(), true);
    // Only the plain ack arrives; no control reply.
    assert_eq!(
        client.receive_ack(&recv(&client.socket)),
        [Notification {
            packet_id,
            delivered: true
        }]
    );
    assert!(try_recv(&client.socket).is_none());
    let counted = stats(&app);
    assert_eq!(counted.hello_unanswered, 1);
    assert_eq!(counted.acks_sent, 1);
    assert_eq!(
        counted.control_challenges_sent + counted.upgrades_sent + counted.failures_sent,
        0
    );
    assert!(lab(&app).has_peer(client.address()));
    assert_no_secrets(&app, &[SENTINEL, "16909060"]);
}

#[test]
fn lost_challenge_is_resent_after_the_client_naks_it() {
    let (mut app, address) = lab_app(PeerLabConfig::default());
    let mut client = Client::connect(&mut app, address);
    client.exchange(&mut app, short_hello("", 1), true);
    let _lost = recv(&client.socket);
    // The client's next packet acknowledges nothing from the lab, so the
    // lab's ack carries no resend yet; its own history then naks the reply.
    let (empty, _) = client.packet(Vec::new());
    client.send(&empty);
    app.update();
    let (_, messages) = client.receive_reply(&recv(&client.socket));
    assert!(messages.is_empty());
    let (empty, _) = client.packet(Vec::new());
    client.send(&empty);
    app.update();
    let (_, messages) = client.receive_reply(&recv(&client.socket));
    assert!(matches!(
        messages.as_slice(),
        [ControlMessage::Challenge { .. }]
    ));
    assert_eq!(stats(&app).control_challenges_sent, 1);
}

#[test]
fn one_unit_hello_token_takes_the_unanswered_encryption_path() {
    // The original compares the token count including its terminator with two,
    // so only an empty token is challenged (F-563).
    let (mut app, address) = lab_app(PeerLabConfig::default());
    let mut client = Client::connect(&mut app, address);
    let packet_id = client.exchange(&mut app, short_hello("x", 0x0102_0304), true);
    assert_eq!(
        client.receive_ack(&recv(&client.socket)),
        [Notification {
            packet_id,
            delivered: true
        }]
    );
    assert!(try_recv(&client.socket).is_none());
    let counted = stats(&app);
    assert_eq!(counted.hello_unanswered, 1);
    assert_eq!(counted.control_challenges_sent, 0);
}

#[test]
fn duplicate_handshake_echo_keeps_live_peer_state() {
    let (mut app, address) = lab_app(PeerLabConfig::default());
    let socket = socket(address);
    socket.send(&initial_request()).unwrap();
    app.update();
    let echo = recv(&socket);
    let cookie = decode(&echo).unwrap().cookie;
    socket.send(&echo).unwrap();
    app.update();
    assert!(try_recv(&socket).is_some(), "handshake acknowledgement");
    let (incoming, outgoing) = client_cookie_seeds(&cookie);
    let sequencer = PacketSequencer::new(incoming, outgoing, 64).unwrap();
    let control = sequencer.new_channel();
    let mut client = Client {
        socket,
        cookie,
        sequencer,
        control,
    };
    client.exchange(&mut app, short_hello("", 0x0102_0304), false);
    let _ = client.receive_reply(&recv(&client.socket));

    // A delayed duplicate of the same echo is re-acknowledged, not a reset.
    client.socket.send(&echo).unwrap();
    app.update();
    let ack = decode(&recv(&client.socket)).unwrap();
    assert_eq!(ack.cookie, cookie);
    let counted = stats(&app);
    assert_eq!(counted.acknowledgements_resent, 1);
    assert_eq!((counted.peers_created, counted.peers_replaced), (1, 0));

    // Reliable state survived: the next Login is delivered and answered.
    client.exchange(&mut app, login(), false);
    let (_, messages) = client.receive_reply(&recv(&client.socket));
    assert!(matches!(
        messages.as_slice(),
        [ControlMessage::Failure { .. }]
    ));
    assert_eq!(stats(&app).failures_sent, 1);
}

#[test]
fn admission_enforces_login_state_and_completes_welcome_and_join() {
    let admission = PeerLabAdmission {
        policy: AdmissionPolicy {
            max_players: 1,
            max_spectators: 1,
            max_splitscreens_per_connection: 1,
        },
        ..PeerLabAdmission::default()
    };
    let config = PeerLabConfig {
        local_network_version: Some(LOCAL_VERSION),
        admission: Some(admission),
        ..PeerLabConfig::default()
    };
    let (mut app, address) = lab_app(config);
    let mut client = Client::connect(&mut app, address);

    // Join and Netspeed before Hello/Welcome are rejected by IsClientMsgTypeValid (only acked).
    let early_join_id = client.exchange(&mut app, ControlMessage::Join, true);
    assert_eq!(
        client.receive_ack(&recv(&client.socket)),
        [Notification {
            packet_id: early_join_id,
            delivered: true
        }]
    );
    assert_eq!(stats(&app).invalid_state_messages, 1);
    assert_eq!(stats(&app).joins_succeeded, 0);

    let early_netspeed_id =
        client.exchange(&mut app, ControlMessage::Netspeed { rate: 10_000 }, false);
    assert_eq!(
        client.receive_ack(&recv(&client.socket)),
        [Notification {
            packet_id: early_netspeed_id,
            delivered: true
        }]
    );
    assert_eq!(stats(&app).invalid_state_messages, 2);
    assert_eq!(stats(&app).netspeeds_updated, 0);
    assert_eq!(lab(&app).current_net_speed(client.address()), None);

    // Valid Hello gets Challenge and advances ExpectedClientLoginMsgType to 5 (Login).
    client.exchange(&mut app, short_hello("", LOCAL_VERSION), false);
    let (_, messages) = client.receive_reply(&recv(&client.socket));
    assert!(matches!(
        messages.as_slice(),
        [ControlMessage::Challenge { .. }]
    ));
    assert_eq!(stats(&app).control_challenges_sent, 1);

    // A second Hello and a Netspeed while expecting Login are rejected by IsClientMsgTypeValid.
    let dup_hello_id = client.exchange(&mut app, short_hello("", LOCAL_VERSION), false);
    assert_eq!(
        client.receive_ack(&recv(&client.socket)),
        [Notification {
            packet_id: dup_hello_id,
            delivered: true
        }]
    );
    assert_eq!(stats(&app).invalid_state_messages, 3);
    assert_eq!(stats(&app).control_challenges_sent, 1);

    let logging_in_netspeed_id =
        client.exchange(&mut app, ControlMessage::Netspeed { rate: 10_000 }, false);
    assert_eq!(
        client.receive_ack(&recv(&client.socket)),
        [Notification {
            packet_id: logging_in_netspeed_id,
            delivered: true
        }]
    );
    assert_eq!(stats(&app).invalid_state_messages, 4);
    assert_eq!(stats(&app).netspeeds_updated, 0);
    assert_eq!(lab(&app).current_net_speed(client.address()), None);

    // Compatible Login gets Welcome (ID 1) and advances ClientLoginState to Welcomed.
    let valid_login = ControlMessage::Login {
        response: ArchiveText::text("0").unwrap(),
        url: ArchiveText::text("?Name=Alice?bIsFromInvite").unwrap(),
        unique_net_id: UniqueNetId {
            kind: NetIdType::Named(ArchiveText::text("STEAM").unwrap()),
            value: NetIdValue::from_hex(NET_ID_HEX).unwrap(),
        },
        platform: ArchiveText::text(PLATFORM_SENTINEL).unwrap(),
    };
    client.exchange(&mut app, valid_login, false);
    let (_, messages) = client.receive_reply(&recv(&client.socket));
    assert_eq!(
        messages,
        [ControlMessage::Welcome {
            map_url: ArchiveText::text(admission.map_url).unwrap(),
            game_name: ArchiveText::text(admission.game_name).unwrap(),
            redirect_url: ArchiveText::text(admission.redirect_url).unwrap(),
        }]
    );
    assert_eq!(stats(&app).welcomes_sent, 1);

    // Netspeed and Join after Welcome succeed; a duplicate Join is ignored;
    // Netspeed in Joined state also succeeds.
    let netspeed_id = client.exchange(&mut app, ControlMessage::Netspeed { rate: 100_000 }, false);
    assert_eq!(
        client.receive_ack(&recv(&client.socket)),
        [Notification {
            packet_id: netspeed_id,
            delivered: true
        }]
    );
    assert_eq!(stats(&app).netspeeds_updated, 1);
    assert_eq!(lab(&app).current_net_speed(client.address()), Some(15_000));

    let join_id = client.exchange(&mut app, ControlMessage::Join, false);
    assert_eq!(
        client.receive_ack(&recv(&client.socket)),
        [Notification {
            packet_id: join_id,
            delivered: true
        }]
    );
    assert_eq!(stats(&app).joins_succeeded, 1);

    let joined_netspeed_id =
        client.exchange(&mut app, ControlMessage::Netspeed { rate: 1_200 }, false);
    assert_eq!(
        client.receive_ack(&recv(&client.socket)),
        [Notification {
            packet_id: joined_netspeed_id,
            delivered: true
        }]
    );
    assert_eq!(stats(&app).netspeeds_updated, 2);
    assert_eq!(lab(&app).current_net_speed(client.address()), Some(1_800));

    let dup_join_id = client.exchange(&mut app, ControlMessage::Join, false);
    assert_eq!(
        client.receive_ack(&recv(&client.socket)),
        [Notification {
            packet_id: dup_join_id,
            delivered: true
        }]
    );
    assert_eq!(stats(&app).duplicate_joins_ignored, 1);
    assert_eq!(stats(&app).joins_succeeded, 1);

    // PCSwap with ChildIndex -1 (parent connection) succeeds; ChildIndex 0 with no children is ignored;
    // DebugText in Joined state is counted without retaining text.
    let parent_swap_id = client.exchange(
        &mut app,
        ControlMessage::Swap {
            value: -1_i32 as u32,
        },
        false,
    );
    assert_eq!(
        client.receive_ack(&recv(&client.socket)),
        [Notification {
            packet_id: parent_swap_id,
            delivered: true
        }]
    );
    assert_eq!(stats(&app).swaps_received, 1);
    assert_eq!(stats(&app).invalid_swaps_ignored, 0);

    let bad_swap_id = client.exchange(&mut app, ControlMessage::Swap { value: 0 }, false);
    assert_eq!(
        client.receive_ack(&recv(&client.socket)),
        [Notification {
            packet_id: bad_swap_id,
            delivered: true
        }]
    );
    assert_eq!(stats(&app).swaps_received, 1);
    assert_eq!(stats(&app).invalid_swaps_ignored, 1);

    // SplitJoin adds one child connection (splitscreen_count == 1), after which
    // ChildIndex 0 resolves to ChildConnection { index: 0 } and ChildIndex 1 is ignored.
    let split_join_id = client.exchange(
        &mut app,
        ControlMessage::SplitJoin {
            url: ArchiveText::text("?SpectatorOnly=1").unwrap(),
            unique_net_id: UniqueNetId {
                kind: NetIdType::Named(ArchiveText::text("STEAM").unwrap()),
                value: NetIdValue::from_hex(NET_ID_HEX).unwrap(),
            },
        },
        false,
    );
    assert_eq!(
        client.receive_ack(&recv(&client.socket)),
        [Notification {
            packet_id: split_join_id,
            delivered: true
        }]
    );
    assert_eq!(stats(&app).joins_succeeded, 2);

    let child_swap_id = client.exchange(&mut app, ControlMessage::Swap { value: 0 }, false);
    assert_eq!(
        client.receive_ack(&recv(&client.socket)),
        [Notification {
            packet_id: child_swap_id,
            delivered: true
        }]
    );
    assert_eq!(stats(&app).swaps_received, 2);
    assert_eq!(stats(&app).invalid_swaps_ignored, 1);

    let out_of_range_child_swap_id =
        client.exchange(&mut app, ControlMessage::Swap { value: 1 }, false);
    assert_eq!(
        client.receive_ack(&recv(&client.socket)),
        [Notification {
            packet_id: out_of_range_child_swap_id,
            delivered: true
        }]
    );
    assert_eq!(stats(&app).swaps_received, 2);
    assert_eq!(stats(&app).invalid_swaps_ignored, 2);

    let debug_id = client.exchange(
        &mut app,
        ControlMessage::Debug {
            text: ArchiveText::text("SENTINEL-DEBUG-99b1").unwrap(),
        },
        false,
    );
    assert_eq!(
        client.receive_ack(&recv(&client.socket)),
        [Notification {
            packet_id: debug_id,
            delivered: true
        }]
    );
    assert_eq!(stats(&app).debug_texts_received, 1);

    // A second player connection is rejected at Login with SERVER_FULL because max_players == 1.
    let mut second = Client::connect(&mut app, address);
    second.exchange(&mut app, short_hello("", LOCAL_VERSION), true);
    let _ = second.receive_reply(&recv(&second.socket));
    let second_login = ControlMessage::Login {
        response: ArchiveText::text("0").unwrap(),
        url: ArchiveText::text("?Name=Bob").unwrap(),
        unique_net_id: UniqueNetId {
            kind: NetIdType::Default,
            value: NetIdValue::from_hex(NET_ID_HEX).unwrap(),
        },
        platform: ArchiveText::text(PLATFORM_SENTINEL).unwrap(),
    };
    second.exchange(&mut app, second_login, false);
    let (_, messages) = second.receive_reply(&recv(&second.socket));
    assert_eq!(
        messages,
        [ControlMessage::Failure {
            message: ArchiveText::text(SERVER_FULL).unwrap()
        }]
    );
    assert!(!lab(&app).has_peer(second.address()));

    // A third connection with an incompatible UniqueNetId subsystem is rejected at PreLogin.
    let mut third = Client::connect(&mut app, address);
    third.exchange(&mut app, short_hello("", LOCAL_VERSION), true);
    let _ = third.receive_reply(&recv(&third.socket));
    let bad_sub_login = ControlMessage::Login {
        response: ArchiveText::text("0").unwrap(),
        url: ArchiveText::text("?SpectatorOnly=1").unwrap(),
        unique_net_id: UniqueNetId {
            kind: NetIdType::Named(ArchiveText::text("EOS").unwrap()),
            value: NetIdValue::from_hex(NET_ID_HEX).unwrap(),
        },
        platform: ArchiveText::text(PLATFORM_SENTINEL).unwrap(),
    };
    third.exchange(&mut app, bad_sub_login, false);
    let (_, messages) = third.receive_reply(&recv(&third.socket));
    assert_eq!(
        messages,
        [ControlMessage::Failure {
            message: ArchiveText::text(INCOMPATIBLE_UNIQUE_NET_ID).unwrap()
        }]
    );
    assert!(!lab(&app).has_peer(third.address()));
    assert_no_secrets(
        &app,
        &[
            "Alice",
            "Bob",
            "SENTINEL-DEBUG-99b1",
            "15000",
            "100000",
            "10000",
            "1800",
            "1200",
            PLATFORM_SENTINEL,
            NET_ID_HEX,
            &NET_ID_HEX.to_lowercase(),
        ],
    );
}

#[test]
fn admission_join_failure_sends_failure_and_closes_peer() {
    let admission = PeerLabAdmission {
        has_player_start: false,
        ..PeerLabAdmission::default()
    };
    let config = PeerLabConfig {
        admission: Some(admission),
        ..PeerLabConfig::default()
    };
    let (mut app, address) = lab_app(config);
    let mut client = Client::connect(&mut app, address);
    client.exchange(&mut app, short_hello("", 0x0102_0304), true);
    let _ = client.receive_reply(&recv(&client.socket));
    client.exchange(&mut app, login(), false);
    let (_, messages) = client.receive_reply(&recv(&client.socket));
    assert!(matches!(
        messages.as_slice(),
        [ControlMessage::Welcome { .. }]
    ));
    client.exchange(&mut app, ControlMessage::Join, false);
    let (_, messages) = client.receive_reply(&recv(&client.socket));
    assert_eq!(
        messages,
        [ControlMessage::Failure {
            message: ArchiveText::text(FIND_PLAYER_START_ERROR).unwrap()
        }]
    );
    assert!(!lab(&app).has_peer(client.address()));
    assert_eq!(stats(&app).joins_succeeded, 0);
    assert_eq!(stats(&app).failures_sent, 1);
}

/// Drain every datagram the lab has queued for `client`.
fn drain(client: &Client) -> Vec<Vec<u8>> {
    std::iter::from_fn(|| try_recv(&client.socket)).collect()
}

/// A reliable control bunch carrying one fallback entry; its sequence comes
/// from the client's control channel (a placeholder holds the slot).
fn fallback_bunch(
    client: &mut Client,
    fallback: &icarus_bevy::experimental::control_messages::ControlFallback,
) -> Bunch {
    let packet_id = client.sequencer.out_packet_id();
    let sequence = client
        .control
        .send_reliable(packet_id, ControlMessage::Join)
        .unwrap();
    let mut writer = BitWriter::new();
    icarus_bevy::experimental::control_messages::write_control_fallback(
        &mut writer,
        fallback,
        &ControlCodec::default(),
    )
    .unwrap();
    Bunch {
        payload: Payload::from_writer(writer),
        ..control_bunch(
            Some(wire_reliable_sequence(sequence)),
            false,
            &ControlMessage::Join,
        )
    }
}

#[test]
fn player_controller_channel_failure_closes_the_connection() {
    use icarus_bevy::experimental::{control_messages::ControlFallback, release::ReleaseProfile};
    let config = PeerLabConfig {
        admission: Some(PeerLabAdmission::default()),
        ..PeerLabConfig::default()
    };
    let mut app = build_app(&ServerConfig::default(), &WorldSnapshot::default()).unwrap();
    let mut bound = PeerLab::bind(0, config).unwrap();
    let profile = ReleaseProfile::release_3_0_32();
    let map_url = PeerLabAdmission::default().map_url;
    bound
        .enable_player_controller_open(
            profile
                .player_controller_open_plan(map_url, [0.0; 3])
                .unwrap()
                .unwrap(),
        )
        .unwrap();
    bound
        .enable_player_state_open(profile.player_state_open_plan(map_url).unwrap().unwrap())
        .unwrap();
    let address = bound.local_addr().unwrap();
    app.insert_resource(bound).add_plugins(PeerLabPlugin);
    let mut client = Client::connect(&mut app, address);

    // Before Login the failure is gated like any non-login message.
    let early = fallback_bunch(
        &mut client,
        &ControlFallback::ActorChannelFailure { channel_index: 2 },
    );
    let (datagram, _) = client.packet(vec![early]);
    client.send(&datagram);
    app.update();
    assert_eq!(stats(&app).invalid_state_messages, 1);
    drain(&client);

    client.exchange(&mut app, short_hello("", 0), false);
    let login = ControlMessage::Login {
        response: ArchiveText::text("0").unwrap(),
        url: ArchiveText::text("?Name=Tester").unwrap(),
        unique_net_id: UniqueNetId {
            kind: NetIdType::Named(ArchiveText::text("STEAM").unwrap()),
            value: NetIdValue::from_hex(NET_ID_HEX).unwrap(),
        },
        platform: ArchiveText::text(PLATFORM_SENTINEL).unwrap(),
    };
    client.exchange(&mut app, login, false);
    client.exchange(&mut app, ControlMessage::Join, false);
    assert_eq!(stats(&app).player_controller_opens_sent, 1);
    assert_eq!(stats(&app).player_state_opens_sent, 1);
    drain(&client);

    // Another actor channel (3) failing only notifies the controller.
    let other = fallback_bunch(
        &mut client,
        &ControlFallback::ActorChannelFailure { channel_index: 3 },
    );
    let (datagram, _) = client.packet(vec![other]);
    client.send(&datagram);
    app.update();
    assert!(lab(&app).has_peer(client.address()));
    assert_eq!(stats(&app).actor_channel_failures, 1);
    drain(&client);

    // The PlayerController channel (2) failing closes the connection.
    let fatal = fallback_bunch(
        &mut client,
        &ControlFallback::ActorChannelFailure { channel_index: 2 },
    );
    let (datagram, _) = client.packet(vec![fatal]);
    client.send(&datagram);
    app.update();
    assert!(!lab(&app).has_peer(client.address()));
    let counted = stats(&app);
    assert_eq!(
        (
            counted.actor_channel_failures,
            counted.peers_closed_by_channel_failure,
            counted.channel_failure_closes_sent,
            counted.reply_errors
        ),
        (2, 1, 1, 0)
    );
    let closes: Vec<_> = drain(&client)
        .iter()
        .flat_map(|datagram| read_packet(datagram, &to_client()).unwrap().bunches)
        .filter(|bunch| bunch.header.close)
        .collect();
    let [close] = closes.as_slice() else {
        panic!("one control close bunch");
    };
    assert_eq!(
        (close.header.channel_index, close.header.close_reason),
        (0, Some(0))
    );
}

#[test]
fn the_game_state_open_follows_the_controller_over_several_datagrams() {
    use icarus_bevy::experimental::{
        game_state_open::{GAME_STATE_CHANNEL_INDEX, handle},
        release::ReleaseProfile,
        rep_layout::RepValue,
        session_open::{ActorKey, PLAYER_STATE_CHANNEL_INDEX},
    };
    let config = PeerLabConfig {
        admission: Some(PeerLabAdmission::default()),
        ..PeerLabConfig::default()
    };
    let mut app = build_app(&ServerConfig::default(), &WorldSnapshot::default()).unwrap();
    let mut bound = PeerLab::bind(0, config).unwrap();
    let profile = ReleaseProfile::release_3_0_32();
    let map_url = PeerLabAdmission::default().map_url;
    let plan = profile.game_state_open_plan(map_url).unwrap().unwrap();
    // A RichPresenceGUID long enough to need a second datagram.
    let state = || icarus_bevy::experimental::game_state_open::GameStateOpenState {
        key: ActorKey::Session(0),
        managers: Default::default(),
        values: std::collections::BTreeMap::from([(
            handle::RICH_PRESENCE_GUID,
            RepValue::Str(ArchiveText::text(&"G".repeat(1_500)).unwrap()),
        )]),
        weather: Vec::new(),
    };
    assert!(
        bound
            .enable_game_state_open(plan.clone(), state(), GAME_STATE_CHANNEL_INDEX)
            .is_err(),
        "the GameState follows the PlayerController"
    );
    bound
        .enable_player_controller_open(
            profile
                .player_controller_open_plan(map_url, [0.0; 3])
                .unwrap()
                .unwrap(),
        )
        .unwrap();
    bound
        .enable_player_state_open(profile.player_state_open_plan(map_url).unwrap().unwrap())
        .unwrap();
    let mut world_key = state();
    world_key.key = ActorKey::World(0);
    let mut timed = state();
    timed
        .values
        .insert(handle::WORLD_TIME_SECONDS, RepValue::Float(0));
    for (state, channel) in [
        (world_key, GAME_STATE_CHANNEL_INDEX),
        (timed, GAME_STATE_CHANNEL_INDEX),
        (state(), 0),
        (state(), 2),
        (state(), PLAYER_STATE_CHANNEL_INDEX),
    ] {
        assert!(
            bound
                .enable_game_state_open(plan.clone(), state, channel)
                .is_err()
        );
    }
    bound
        .enable_game_state_open(plan, state(), GAME_STATE_CHANNEL_INDEX)
        .unwrap();
    let address = bound.local_addr().unwrap();
    app.insert_resource(bound).add_plugins(PeerLabPlugin);
    let mut client = Client::connect(&mut app, address);
    client.exchange(&mut app, short_hello("", 0), false);
    client.exchange(&mut app, login(), false);
    client.exchange(&mut app, ControlMessage::Join, false);
    let counted = stats(&app);
    assert_eq!(
        (
            counted.player_controller_opens_sent,
            counted.game_state_opens_sent,
            counted.game_state_open_errors,
            counted.player_state_opens_sent
        ),
        (1, 1, 0, 1)
    );
    // The GameState's parts arrive in order across consecutive datagrams,
    // after the controller's and before the PlayerState's.
    let channels: Vec<Vec<u32>> = drain(&client)
        .iter()
        .map(|datagram| {
            read_packet(datagram, &to_client())
                .unwrap()
                .bunches
                .iter()
                .map(|bunch| bunch.header.channel_index)
                .collect()
        })
        .filter(|channels: &Vec<u32>| !channels.is_empty())
        .collect();
    let order: Vec<u32> = channels.iter().flatten().copied().collect();
    let game = GAME_STATE_CHANNEL_INDEX;
    let first = order.iter().position(|&channel| channel == game).unwrap();
    let last = order.iter().rposition(|&channel| channel == game).unwrap();
    assert!(order[first..=last].iter().all(|&channel| channel == game));
    assert!(order[..first].contains(&2));
    assert!(order[last + 1..].contains(&PLAYER_STATE_CHANNEL_INDEX));
    let carrying = channels
        .iter()
        .filter(|channels| channels.contains(&game))
        .count();
    assert!(carrying >= 2, "the GameState spans datagrams");
}

#[test]
fn exports_repeat_until_their_packet_is_acked_and_again_after_a_nak() {
    use icarus_bevy::experimental::session_open::{ActorOpenSpec, PathSegment, map_level_chain};
    let config = PeerLabConfig {
        admission: Some(PeerLabAdmission::default()),
        ..PeerLabConfig::default()
    };
    let (mut app, address) = lab_app(config);
    let mut client = Client::connect(&mut app, address);
    client.exchange(&mut app, short_hello("", 0), false);
    client.exchange(&mut app, login(), false);
    client.exchange(&mut app, ControlMessage::Join, false);
    accept_all_headers(&mut client);
    let connection = lab(&app).joined_connections()[0];
    let spec = |name: &str| ActorOpenSpec {
        actor: 0,
        archetype: vec![
            PathSegment::new(&format!("Default__{name}_C"), 0x0cab_0cab, false),
            PathSegment::new(&format!("/Game/Lab/{name}"), 0, false),
        ],
        level: map_level_chain(PeerLabAdmission::default().map_url).unwrap(),
        location: Some([0.0; 3]),
        rotation: None,
        velocity: None,
        blocks: Vec::new(),
    };
    // Whether each queued open datagram starts with an export-only part.
    let exported = |client: &mut Client, accept: bool| -> Vec<bool> {
        let datagrams = drain(client);
        datagrams
            .iter()
            .map(|datagram| {
                let packet = read_packet(datagram, &to_client()).unwrap();
                if accept && let Some(received) = client.sequencer.receive(&packet.header).unwrap()
                {
                    client.sequencer.ack_seq(received.sequence, true).unwrap();
                }
                packet.bunches[0].header.has_package_map_exports
            })
            .collect()
    };
    let open = |app: &mut App, key: u64, name: &str| {
        let mut lab_mut = app.world_mut().resource_mut::<PeerLab>();
        lab_mut
            .open_actor(connection, key, spec(name), 2, 17)
            .unwrap();
    };
    // Two opens before any ack: both export the archetype (F-796).
    open(&mut app, 7, "LabCart");
    open(&mut app, 8, "LabCart");
    assert_eq!(exported(&mut client, true), [true, true]);
    // Once the client acks them, a third open references it plainly.
    let (ack, _) = client.packet(Vec::new());
    client.send(&ack);
    app.update();
    accept_all_headers(&mut client);
    open(&mut app, 9, "LabCart");
    assert_eq!(exported(&mut client, true), [false]);

    // A new archetype whose open is lost: the nak returns it to not acked,
    // so the next open exports it again.
    open(&mut app, 10, "LabBox");
    assert_eq!(exported(&mut client, false), [true], "lost");
    let (keepalive, _) = client.packet(Vec::new());
    client.send(&keepalive);
    app.update();
    accept_all_headers(&mut client);
    let (report, _) = client.packet(Vec::new());
    client.send(&report);
    app.update();
    accept_all_headers(&mut client);
    open(&mut app, 11, "LabBox");
    assert_eq!(exported(&mut client, true), [true]);
}

#[test]
fn the_pawn_opens_after_the_controller_with_its_roles_and_references() {
    use icarus_bevy::experimental::{
        actor_channel::{ActorChannelCodec, ActorChannelLimits, read_actor_bunch_body},
        archive::ByteOrder,
        pawn_open::{PAWN_CHANNEL_INDEX, PawnComponent, PawnContent, PawnOpenPlan},
        peer_lab::pawn_key,
        release::ReleaseProfile,
        rep_layout::{
            MovementLevel, RepLayoutCodec, RepLayoutLimits, RepValue, RotationLevel,
            pawn_rep_layout, read_rep_properties,
        },
        session_open::{PLAYER_STATE_CHANNEL_INDEX, PathSegment},
    };
    let config = PeerLabConfig {
        admission: Some(PeerLabAdmission::default()),
        ..PeerLabConfig::default()
    };
    let mut app = build_app(&ServerConfig::default(), &WorldSnapshot::default()).unwrap();
    let mut bound = PeerLab::bind(0, config).unwrap();
    let profile = ReleaseProfile::release_3_0_32();
    let map_url = PeerLabAdmission::default().map_url;
    let controller = profile
        .player_controller_open_plan(map_url, [10.0, 20.0, 30.0])
        .unwrap()
        .unwrap();
    // Authored identifiers on the `APawn` layout (handles 13..18), padded
    // with object handles up to 56 so the component references (47, 48, 56)
    // encode.
    let mut layout = pawn_rep_layout(
        MovementLevel::Hundredths,
        MovementLevel::Whole,
        RotationLevel::Bytes,
    );
    while layout.len() < 56 {
        layout.push(icarus_bevy::experimental::rep_layout::RepCommand::Object);
    }
    let plan = PawnOpenPlan {
        archetype: vec![
            PathSegment::new("Default__TestPawn_C", 0x5151, false),
            PathSegment::new("/Game/Test/TestPawn", 0, false),
        ],
        level: controller.profile.level.clone(),
        components: ["Stats", "Bag", "State"]
            .iter()
            .zip(1..)
            .map(|(name, checksum)| PawnComponent {
                name: (*name).to_owned(),
                checksum,
                // The bag is active at open.
                rep_bools: if *name == "Bag" {
                    vec![(2, true)]
                } else {
                    Vec::new()
                },
            })
            .collect(),
        stat_container: 0,
        actor_state: 2,
        inventory_component: 1,
        layout: layout.clone(),
        content: profile.pawn_open_plan(map_url).unwrap().unwrap().content,
        engine_net_version: profile.engine_network_version,
    };
    assert!(
        bound
            .enable_pawn_open(plan.clone(), PAWN_CHANNEL_INDEX, PawnContent::default())
            .is_err(),
        "the pawn follows the PlayerController"
    );
    bound.enable_player_controller_open(controller).unwrap();
    bound
        .enable_player_state_open(profile.player_state_open_plan(map_url).unwrap().unwrap())
        .unwrap();
    for channel in [0, 2, PLAYER_STATE_CHANNEL_INDEX] {
        assert!(
            bound
                .enable_pawn_open(plan.clone(), channel, PawnContent::default())
                .is_err()
        );
    }
    bound
        .enable_pawn_open(plan, PAWN_CHANNEL_INDEX, PawnContent::default())
        .unwrap();
    let address = bound.local_addr().unwrap();
    app.insert_resource(bound).add_plugins(PeerLabPlugin);
    let mut client = Client::connect(&mut app, address);
    client.exchange(&mut app, short_hello("", 0), false);
    client.exchange(&mut app, login(), false);
    client.exchange(&mut app, ControlMessage::Join, false);
    let counted = stats(&app);
    assert_eq!(
        (
            counted.player_controller_opens_sent,
            counted.pawn_opens_sent,
            counted.pawn_open_errors,
            counted.player_state_opens_sent
        ),
        (1, 1, 0, 1)
    );
    let bunches: Vec<_> = drain(&client)
        .iter()
        .flat_map(|datagram| read_packet(datagram, &to_client()).unwrap().bunches)
        .filter(|bunch| bunch.header.channel_index >= 2)
        .collect();
    let order: Vec<u32> = bunches
        .iter()
        .map(|bunch| bunch.header.channel_index)
        .collect();
    let first_pawn = order.iter().position(|&channel| channel == 3).unwrap();
    assert!(order[..first_pawn].iter().all(|&channel| channel == 2));
    assert!(order[first_pawn..].contains(&PLAYER_STATE_CHANNEL_INDEX));
    // The pawn body (its last part) decodes to the roles and references.
    let body = bunches
        .iter()
        .rfind(|bunch| bunch.header.channel_index == 3 && !bunch.header.has_package_map_exports)
        .unwrap();
    let codec = ActorChannelCodec {
        engine_net_version: profile.engine_network_version,
        exports_enabled: false,
        has_level: true,
        order: ByteOrder::Little,
        limits: ActorChannelLimits::default(),
    };
    let open = read_actor_bunch_body(&mut body.payload.reader(), true, &codec).unwrap();
    let spawn = open.spawn.unwrap();
    let connection = lab(&app).joined_connections()[0];
    let mut cache_view = lab(&app).guid_cache().clone();
    assert_eq!(
        spawn.actor.guid,
        cache_view.actor_guid(pawn_key(connection))
    );
    assert_eq!(open.blocks.len(), 4, "actor block and three components");
    let rep_codec = RepLayoutCodec {
        actor: codec,
        checksums: false,
        limits: RepLayoutLimits::default(),
    };
    let stream =
        read_rep_properties(&mut open.blocks[0].payload.reader(), &layout, &rep_codec).unwrap();
    let value = |handle: u32| {
        stream
            .entries
            .iter()
            .find(|entry| entry.handle == handle)
            .map(|entry| entry.value.clone())
    };
    assert!(
        matches!(value(5), Some(RepValue::Byte(2))),
        "AutonomousProxy"
    );
    assert!(matches!(value(14), Some(RepValue::Byte(3))));
    let object = |handle: u32| match value(handle) {
        Some(RepValue::Object(reference)) => reference.guid,
        other => panic!("handle {handle}: {other:?}"),
    };
    assert_eq!(object(13), object(18), "Owner and Controller");
    assert_eq!(object(15), spawn.actor.guid, "Instigator");
    assert_eq!(object(17), object(13) + 2, "the controller's PlayerState");
    // Stats and State open empty; the bag carries `bIsActive`.
    let bag = &open.blocks[2];
    assert!(bag.has_rep_layout);
    let bag = read_rep_properties(
        &mut bag.payload.reader(),
        &icarus_bevy::experimental::rep_layout::actor_component_rep_layout(),
        &rep_codec,
    )
    .unwrap();
    assert_eq!(bag.entries.len(), 1);
    assert_eq!(
        (bag.entries[0].handle, &bag.entries[0].value),
        (2, &RepValue::Bool(true))
    );
    for block in [&open.blocks[1], &open.blocks[3]] {
        assert!(!block.has_rep_layout && block.payload.bit_len() == 0);
    }
}

/// Accept every queued lab datagram's header as the client (acks only).
fn accept_all_headers(client: &mut Client) -> usize {
    let datagrams = drain(client);
    for datagram in &datagrams {
        let packet = read_packet(datagram, &to_client()).unwrap();
        if let Some(received) = client.sequencer.receive(&packet.header).unwrap() {
            client.sequencer.ack_seq(received.sequence, true).unwrap();
        }
    }
    datagrams.len()
}

fn acknowledge_lab(app: &mut App, client: &mut Client) {
    accept_all_headers(client);
    let (ack, _) = client.packet(Vec::new());
    client.send(&ack);
    app.update();
    accept_all_headers(client);
}

fn join_motion_client(
    app: &mut App,
    address: SocketAddr,
    order: icarus_bevy::experimental::archive::ByteOrder,
) -> Client {
    use icarus_bevy::experimental::archive::ByteOrder;
    let mut client = Client::connect(app, address);
    let hello = ControlMessage::Hello {
        first_field: u8::from(matches!(order, ByteOrder::Little)),
        version: 0,
        token: ArchiveText::text("").unwrap(),
    };
    for message in [hello, login(), ControlMessage::Join] {
        let mut bunch = client.reliable_bunch(message.clone(), false);
        let mut writer = BitWriter::new();
        writer.write_bytes(
            &encode_control_message(
                &message,
                &ControlCodec {
                    order,
                    ..ControlCodec::default()
                },
            )
            .unwrap(),
        );
        bunch.payload = Payload::from_writer(writer);
        let (datagram, _) = client.packet(vec![bunch]);
        client.send(&datagram);
        app.update();
        accept_all_headers(&mut client);
    }
    acknowledge_lab(app, &mut client);
    client
}

fn motion_spec(
    location: [f32; 3],
    rotation: [f32; 3],
    velocity: [f32; 3],
) -> icarus_bevy::experimental::session_open::ActorOpenSpec {
    use icarus_bevy::experimental::session_open::{
        ActorOpenSpec, PathSegment, authored_movement, map_level_chain, world_actor_block,
    };
    ActorOpenSpec {
        actor: 0,
        archetype: vec![
            PathSegment::new("Default__Motion_C", 0x1234_5678, false),
            PathSegment::new("/Game/Lab/Motion", 0, false),
        ],
        level: map_level_chain(PeerLabAdmission::default().map_url).unwrap(),
        location: Some(location),
        rotation: Some(rotation),
        velocity: Some(velocity),
        blocks: vec![
            world_actor_block(
                17,
                Some(authored_movement(location, rotation, velocity).unwrap()),
            )
            .unwrap(),
        ],
    }
}

fn decoded_motion_updates(
    client: &Client,
    order: icarus_bevy::experimental::archive::ByteOrder,
) -> Vec<icarus_bevy::experimental::rep_layout::RepMovement> {
    use icarus_bevy::experimental::{
        actor_channel::{ActorChannelCodec, ActorChannelLimits, read_actor_bunch_body},
        rep_layout::{
            MovementLevel, RepLayoutCodec, RepLayoutLimits, RepValue, RotationLevel,
            actor_rep_layout, read_rep_properties,
        },
    };
    let codec = RepLayoutCodec {
        actor: ActorChannelCodec {
            engine_net_version: 17,
            exports_enabled: false,
            has_level: true,
            order,
            limits: ActorChannelLimits::default(),
        },
        checksums: false,
        limits: RepLayoutLimits::default(),
    };
    drain(client)
        .into_iter()
        .flat_map(|datagram| read_packet(&datagram, &to_client()).unwrap().bunches)
        .filter(|bunch| !bunch.header.reliable && bunch.header.channel_index != 0)
        .map(|bunch| {
            let body =
                read_actor_bunch_body(&mut bunch.payload.reader(), false, &codec.actor).unwrap();
            assert!(body.spawn.is_none());
            assert_eq!(body.blocks.len(), 1);
            let stream = read_rep_properties(
                &mut body.blocks[0].payload.reader(),
                &actor_rep_layout(
                    MovementLevel::Whole,
                    MovementLevel::Whole,
                    RotationLevel::Bytes,
                ),
                &codec,
            )
            .unwrap();
            assert_eq!(stream.entries.len(), 1, "complete handle 6 only");
            assert_eq!(stream.entries[0].handle, 6);
            let RepValue::Movement(movement) = &stream.entries[0].value else {
                panic!("movement")
            };
            assert_eq!(movement.flags, 0);
            assert!(movement.angular_velocity.is_none());
            movement.clone()
        })
        .collect()
}

#[test]
fn generic_open_movement_baselines_follow_actor_block_transmission_order() {
    use icarus_bevy::experimental::{
        actor_channel::{
            ActorChannelCodec, ActorChannelLimits, BlockTarget, ContentBlock, NetGuidRef,
        },
        archive::ByteOrder,
        rep_layout::{
            MovementLevel, PropertyStream, RepEntry, RepLayoutCodec, RepLayoutLimits, RepValue,
            RotationLevel, actor_rep_layout, controller_rep_layout, encode_rep_properties,
            parse_rep_properties,
        },
        session_open::{authored_movement, world_actor_block},
    };
    for order in [ByteOrder::Little, ByteOrder::Big] {
        for reestablish in [false, true] {
            let (mut app, address) = lab_app(PeerLabConfig {
                admission: Some(PeerLabAdmission::default()),
                ..PeerLabConfig::default()
            });
            let mut client = join_motion_client(&mut app, address, order);
            let connection = lab(&app).joined_connections()[0];
            let mut spec = motion_spec([0.0; 3], [0.0; 3], [0.0; 3]);
            let codec = RepLayoutCodec {
                actor: ActorChannelCodec {
                    engine_net_version: 17,
                    exports_enabled: false,
                    has_level: true,
                    order,
                    limits: ActorChannelLimits::default(),
                },
                checksums: false,
                limits: RepLayoutLimits::default(),
            };
            // Valid subclass properties can supersede handle 6 while making
            // the whole block undecodable under the AActor-default layout.
            let stream = PropertyStream {
                entries: vec![
                    RepEntry {
                        handle: 6,
                        checksum: None,
                        value: RepValue::Movement(
                            authored_movement([10.0, 0.0, 0.0], [90.0; 3], [1.0; 3]).unwrap(),
                        ),
                    },
                    RepEntry {
                        handle: 16,
                        checksum: None,
                        value: RepValue::Object(NetGuidRef::plain(0)),
                    },
                ],
                terminator_checksum: None,
            };
            let layout = controller_rep_layout(
                MovementLevel::Whole,
                MovementLevel::Whole,
                RotationLevel::Bytes,
            );
            let payload = encode_rep_properties(&stream, &layout, &codec).unwrap();
            assert_eq!(
                parse_rep_properties(&payload, &layout, &codec).unwrap(),
                stream
            );
            assert!(
                parse_rep_properties(
                    &payload,
                    &actor_rep_layout(
                        MovementLevel::Whole,
                        MovementLevel::Whole,
                        RotationLevel::Bytes,
                    ),
                    &codec,
                )
                .is_err()
            );
            spec.blocks.push(ContentBlock {
                has_rep_layout: true,
                target: BlockTarget::Actor,
                payload,
            });
            let latest = authored_movement([20.0, 30.0, 40.0], [45.0; 3], [2.0; 3]).unwrap();
            if reestablish {
                spec.blocks
                    .push(world_actor_block(17, Some(latest)).unwrap());
            }
            app.world_mut()
                .resource_mut::<PeerLab>()
                .open_actor(connection, 7, spec, 2, 17)
                .unwrap();
            acknowledge_lab(&mut app, &mut client);
            if reestablish {
                assert!(
                    !app.world_mut()
                        .resource_mut::<PeerLab>()
                        .update_actor_motion(
                            connection,
                            7,
                            [20.0, 30.0, 40.0],
                            [45.0; 3],
                            [2.0; 3],
                            17,
                        ),
                    "later decodable handle 6 reestablishes the baseline"
                );
                assert!(drain(&client).is_empty());
            }
            assert!(
                app.world_mut()
                    .resource_mut::<PeerLab>()
                    .update_actor_motion(connection, 7, [0.0; 3], [0.0; 3], [0.0; 3], 17),
                "the earlier movement must send after the open is acknowledged"
            );
            assert_eq!(
                decoded_motion_updates(&client, order),
                vec![authored_movement([0.0; 3], [0.0; 3], [0.0; 3]).unwrap()]
            );
        }
    }
}

#[test]
fn authored_motion_sends_complete_quantized_changes_and_rejects_invalid_attempts_atomically() {
    use icarus_bevy::experimental::{archive::ByteOrder, session_open::authored_movement};
    for order in [ByteOrder::Little, ByteOrder::Big] {
        let (mut app, address) = lab_app(PeerLabConfig {
            admission: Some(PeerLabAdmission::default()),
            ..PeerLabConfig::default()
        });
        let mut client = join_motion_client(&mut app, address, order);
        let connection = lab(&app).joined_connections()[0];
        let location = [10.0, 20.0, 30.0];
        let mut spec = motion_spec(location, [10.0, 20.0, 30.0], [4.0, 5.0, 6.0]);
        // Baseline is the actual property value, independently of spawn data.
        spec.location = Some([500.0, 0.0, 0.0]);
        app.world_mut()
            .resource_mut::<PeerLab>()
            .open_actor(connection, 7, spec.clone(), 2, 17)
            .unwrap();
        assert!(
            !app.world_mut()
                .resource_mut::<PeerLab>()
                .update_actor_motion(connection, 7, location, [90.0; 3], [9.0; 3], 17),
            "open must be acknowledged"
        );
        acknowledge_lab(&mut app, &mut client);
        assert!(
            !app.world_mut()
                .resource_mut::<PeerLab>()
                .update_actor_motion(
                    connection,
                    7,
                    location,
                    [10.0, 20.0, 30.0],
                    [4.0, 5.0, 6.0],
                    17
                )
        );
        let changes = [
            ([90.0, 20.0, 30.0], [4.0, 5.0, 6.0]),     // rotation only
            ([90.0, 20.0, 30.0], [40.0, 50.0, 60.0]),  // velocity only
            ([45.0, -90.0, 180.0], [-7.0, 8.0, -9.0]), // combined
            ([45.0, -90.0, 180.0], [0.0; 3]),          // stop
            ([0.0; 3], [0.0; 3]),                      // clear all axes
        ];
        for (rotation, velocity) in changes {
            assert!(
                app.world_mut()
                    .resource_mut::<PeerLab>()
                    .update_actor_motion(connection, 7, location, rotation, velocity, 17)
            );
            assert_eq!(
                decoded_motion_updates(&client, order),
                vec![authored_movement(location, rotation, velocity).unwrap()]
            );
        }
        // Full wraps and differences below both vector and byte-rotator
        // quantization must not consume another packet or change the baseline.
        for rotation in [[360.0, -360.0, 720.0], [0.1, -0.1, 0.1]] {
            assert!(
                !app.world_mut()
                    .resource_mut::<PeerLab>()
                    .update_actor_motion(connection, 7, [10.1, 20.1, 30.1], rotation, [0.1; 3], 17)
            );
        }
        assert!(drain(&client).is_empty());
        let before = stats(&app);
        for field in 0..3 {
            for bad in [f32::NAN, f32::INFINITY, -f32::INFINITY, f32::MAX] {
                let mut values = [location, [0.0; 3], [0.0; 3]];
                values[field][1] = bad;
                assert!(
                    !app.world_mut()
                        .resource_mut::<PeerLab>()
                        .update_actor_motion(connection, 7, values[0], values[1], values[2], 17)
                );
            }
        }
        assert_eq!(
            stats(&app).actor_update_errors,
            before.actor_update_errors + 12
        );
        for values in [
            [[20_000_000.0; 3], [0.0; 3], [0.0; 3]],
            [location, [0.0; 3], [20_000_000.0; 3]],
            [location, [1.0e20; 3], [0.0; 3]],
        ] {
            assert!(
                !app.world_mut()
                    .resource_mut::<PeerLab>()
                    .update_actor_motion(connection, 7, values[0], values[1], values[2], 17,)
            );
        }
        assert_eq!(
            stats(&app).actor_update_errors,
            before.actor_update_errors + 15
        );
        assert_eq!(stats(&app).actor_updates_sent, before.actor_updates_sent);
        assert!(
            !app.world_mut()
                .resource_mut::<PeerLab>()
                .update_actor_movement(connection, 7, location, 17),
            "failed attempts preserve zero baseline"
        );
        assert!(
            app.world_mut()
                .resource_mut::<PeerLab>()
                .update_actor_motion(connection, 7, location, [1.0; 3], [1.0; 3], 17),
            "valid retry"
        );

        // No property 6 in a generic open means no inferred movement baseline.
        let mut generic = spec.clone();
        generic.blocks.clear();
        app.world_mut()
            .resource_mut::<PeerLab>()
            .open_actor(connection, 8, generic, 2, 17)
            .unwrap();
        acknowledge_lab(&mut app, &mut client);
        assert!(
            app.world_mut()
                .resource_mut::<PeerLab>()
                .update_actor_motion(
                    connection,
                    8,
                    spec.location.unwrap(),
                    spec.rotation.unwrap(),
                    spec.velocity.unwrap(),
                    17
                )
        );

        // A spawn-only range failure does not consume a GUID or actor slot;
        // the same actor can be opened after correcting its authored velocity.
        let before_guid = format!("{:?}", lab(&app).guid_cache());
        let mut invalid = spec.clone();
        invalid.velocity = Some([2_000_000.0, 0.0, 0.0]);
        assert!(
            app.world_mut()
                .resource_mut::<PeerLab>()
                .open_actor(connection, 9, invalid, 2, 17)
                .is_none()
        );
        assert!(!lab(&app).is_actor_open(connection, 9));
        assert_eq!(format!("{:?}", lab(&app).guid_cache()), before_guid);
        let mut oversized = spec.clone();
        let mut writer = BitWriter::new();
        writer.write_bytes(&vec![0; 8193]); // Above the actor block's 65,536-bit cap.
        oversized
            .blocks
            .push(icarus_bevy::experimental::actor_channel::ContentBlock {
                has_rep_layout: false,
                target: icarus_bevy::experimental::actor_channel::BlockTarget::Actor,
                payload: Payload::from_writer(writer),
            });
        assert!(
            app.world_mut()
                .resource_mut::<PeerLab>()
                .open_actor(connection, 9, oversized, 2, 17)
                .is_none()
        );
        assert!(!lab(&app).is_actor_open(connection, 9));
        assert_eq!(format!("{:?}", lab(&app).guid_cache()), before_guid);
        assert!(
            app.world_mut()
                .resource_mut::<PeerLab>()
                .open_actor(connection, 9, spec, 2, 17)
                .is_some()
        );
    }
}

#[test]
fn movement_packet_pressure_preserves_the_baseline_until_release_and_retry() {
    use icarus_bevy::experimental::archive::ByteOrder;
    let mut config = PeerLabConfig {
        admission: Some(PeerLabAdmission::default()),
        ..PeerLabConfig::default()
    };
    config.packet.sent_capacity = 12;
    let (mut app, address) = lab_app(config);
    let mut client = join_motion_client(&mut app, address, ByteOrder::Little);
    let connection = lab(&app).joined_connections()[0];
    app.world_mut()
        .resource_mut::<PeerLab>()
        .open_actor(
            connection,
            7,
            motion_spec([0.0; 3], [0.0; 3], [0.0; 3]),
            2,
            17,
        )
        .unwrap();
    acknowledge_lab(&mut app, &mut client);
    let mut failed = None;
    for value in 1..64 {
        if !app
            .world_mut()
            .resource_mut::<PeerLab>()
            .update_actor_motion(connection, 7, [0.0; 3], [0.0; 3], [value as f32; 3], 17)
        {
            failed = Some(value);
            break;
        }
    }
    let failed = failed.expect("bounded sent ring fills");
    assert!(failed > 1);
    let retry_open = motion_spec([5.0; 3], [90.0; 3], [3.0; 3]);
    let before_guid = format!("{:?}", lab(&app).guid_cache());
    assert!(
        app.world_mut()
            .resource_mut::<PeerLab>()
            .open_actor(connection, 9, retry_open.clone(), 2, 17)
            .is_none()
    );
    assert!(!lab(&app).is_actor_open(connection, 9));
    assert_eq!(format!("{:?}", lab(&app).guid_cache()), before_guid);
    let before = stats(&app);
    assert!(
        !app.world_mut()
            .resource_mut::<PeerLab>()
            .update_actor_motion(
                connection,
                7,
                [0.0; 3],
                [0.0; 3],
                [(failed - 1) as f32; 3],
                17
            ),
        "last successful baseline remains"
    );
    assert!(
        !app.world_mut()
            .resource_mut::<PeerLab>()
            .update_actor_motion(connection, 7, [0.0; 3], [0.0; 3], [failed as f32; 3], 17),
        "failure is retryable under pressure"
    );
    assert_eq!(stats(&app).actor_updates_sent, before.actor_updates_sent);
    assert_eq!(stats(&app).peers_failed, 0);
    acknowledge_lab(&mut app, &mut client);
    assert!(
        app.world_mut()
            .resource_mut::<PeerLab>()
            .update_actor_motion(connection, 7, [0.0; 3], [0.0; 3], [failed as f32; 3], 17)
    );
    assert!(
        !app.world_mut()
            .resource_mut::<PeerLab>()
            .update_actor_motion(connection, 7, [0.0; 3], [0.0; 3], [failed as f32; 3], 17)
    );
    app.world_mut()
        .resource_mut::<PeerLab>()
        .open_actor(connection, 9, retry_open, 2, 17)
        .unwrap();
    acknowledge_lab(&mut app, &mut client);
    assert!(
        !app.world_mut()
            .resource_mut::<PeerLab>()
            .update_actor_motion(connection, 9, [5.0; 3], [90.0; 3], [3.0; 3], 17),
        "retried open seeds only its successfully composed baseline"
    );
}

#[test]
fn motion_baselines_are_per_peer_and_close_reopen_uses_current_motion() {
    use icarus_bevy::experimental::{archive::ByteOrder, session_open::authored_movement};
    let (mut app, address) = lab_app(PeerLabConfig {
        admission: Some(PeerLabAdmission::default()),
        ..PeerLabConfig::default()
    });
    let mut first = join_motion_client(&mut app, address, ByteOrder::Little);
    let first_id = lab(&app).joined_connections()[0];
    let mut second = join_motion_client(&mut app, address, ByteOrder::Big);
    let second_id = *lab(&app)
        .joined_connections()
        .iter()
        .find(|&&id| id != first_id)
        .unwrap();
    let spec = motion_spec([10.0; 3], [0.0; 3], [0.0; 3]);
    for connection in [first_id, second_id] {
        app.world_mut()
            .resource_mut::<PeerLab>()
            .open_actor(connection, 7, spec.clone(), 2, 17)
            .unwrap();
    }
    acknowledge_lab(&mut app, &mut first);
    acknowledge_lab(&mut app, &mut second);
    for connection in [first_id, second_id] {
        assert!(
            app.world_mut()
                .resource_mut::<PeerLab>()
                .update_actor_motion(connection, 7, [10.0; 3], [90.0; 3], [1.0; 3], 17)
        );
    }
    let expected = authored_movement([10.0; 3], [90.0; 3], [1.0; 3]).unwrap();
    assert_eq!(
        decoded_motion_updates(&first, ByteOrder::Little),
        vec![expected.clone()]
    );
    assert_eq!(
        decoded_motion_updates(&second, ByteOrder::Big),
        vec![expected]
    );
    assert!(
        app.world_mut()
            .resource_mut::<PeerLab>()
            .close_actor(first_id, 7, 3)
    );
    let current = motion_spec([40.0; 3], [180.0; 3], [0.0; 3]);
    app.world_mut()
        .resource_mut::<PeerLab>()
        .open_actor(first_id, 7, current, 2, 17)
        .unwrap();
    assert!(
        !app.world_mut()
            .resource_mut::<PeerLab>()
            .update_actor_motion(first_id, 7, [40.0; 3], [0.0; 3], [0.0; 3], 17)
    );
    acknowledge_lab(&mut app, &mut first);
    assert!(
        !app.world_mut()
            .resource_mut::<PeerLab>()
            .update_actor_motion(first_id, 7, [40.0; 3], [180.0; 3], [0.0; 3], 17),
        "new open seeds current complete movement"
    );
    assert!(
        !app.world_mut()
            .resource_mut::<PeerLab>()
            .update_actor_motion(second_id, 7, [10.0; 3], [90.0; 3], [1.0; 3], 17),
        "other peer unchanged"
    );
    assert!(
        app.world_mut()
            .resource_mut::<PeerLab>()
            .update_actor_motion(first_id, 7, [40.0; 3], [0.0; 3], [0.0; 3], 17),
        "zero clears reopened rotation"
    );
}

#[test]
fn selected_actor_bridge_uses_optional_motion_without_mutating_position() {
    use bevy::prelude::FixedUpdate;
    use icarus_bevy::{
        experimental::{
            archive::ByteOrder,
            session_open::{authored_movement, map_level_chain},
        },
        replication::{
            ClosedActor, ConnectionFrame, NetActor, NetParameters, ReplicationFrame, SelectedActor,
        },
        session_replication::{
            NetArchetype, NetMotion, SessionReplication, SessionReplicationPlugin,
        },
        simulation::{DomainId, Position},
    };
    let (mut app, address) = lab_app(PeerLabConfig {
        admission: Some(PeerLabAdmission::default()),
        ..PeerLabConfig::default()
    });
    let mut client = join_motion_client(&mut app, address, ByteOrder::Little);
    let connection = lab(&app).joined_connections()[0];
    // Supply authored selections directly to isolate the bridge from policy.
    app.insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
        Duration::ZERO,
    ));
    app.init_resource::<ReplicationFrame>()
        .insert_resource(
            SessionReplication::new(
                map_level_chain(PeerLabAdmission::default().map_url).unwrap(),
                17,
                2,
                [0.0; 3],
            )
            .unwrap(),
        )
        .add_plugins(SessionReplicationPlugin);
    let location = [10.0, 20.0, 30.0];
    let spec = motion_spec(location, [0.0; 3], [0.0; 3]);
    let actor = app
        .world_mut()
        .spawn((
            NetActor {
                id: DomainId(7),
                params: NetParameters::default(),
            },
            Position(location),
            NetArchetype {
                archetype: spec.archetype,
                replicate_movement: true,
            },
            NetMotion {
                rotation_degrees: [1.0, 2.0, 3.0],
                linear_velocity: [4.0, 5.0, 6.0],
            },
        ))
        .id();
    app.world_mut()
        .resource_mut::<ReplicationFrame>()
        .decisions
        .connections = vec![ConnectionFrame {
        connection,
        selected: vec![SelectedActor {
            id: DomainId(7),
            priority: 1,
        }],
        ..ConnectionFrame::default()
    }];
    app.world_mut().run_schedule(FixedUpdate);
    assert_eq!(stats(&app).actor_opens_sent, 1);
    app.world_mut().entity_mut(actor).insert(NetMotion {
        rotation_degrees: [90.0; 3],
        linear_velocity: [10.0; 3],
    });
    app.world_mut().run_schedule(FixedUpdate);
    assert_eq!(stats(&app).actor_updates_sent, 0);
    acknowledge_lab(&mut app, &mut client);
    app.world_mut().run_schedule(FixedUpdate);
    assert_eq!(
        decoded_motion_updates(&client, ByteOrder::Little),
        vec![authored_movement(location, [90.0; 3], [10.0; 3]).unwrap()]
    );
    app.world_mut().entity_mut(actor).remove::<NetMotion>();
    app.world_mut().run_schedule(FixedUpdate);
    assert_eq!(
        decoded_motion_updates(&client, ByteOrder::Little),
        vec![authored_movement(location, [0.0; 3], [0.0; 3]).unwrap()]
    );
    app.world_mut().run_schedule(FixedUpdate);
    assert!(decoded_motion_updates(&client, ByteOrder::Little).is_empty());
    assert_eq!(app.world().get::<Position>(actor).unwrap().0, location);

    // A close followed by the same selection seeds a fresh baseline from
    // the current component; ACKs are required again for further changes.
    app.world_mut().entity_mut(actor).insert(NetMotion {
        rotation_degrees: [180.0; 3],
        linear_velocity: [3.0; 3],
    });
    app.world_mut()
        .resource_mut::<ReplicationFrame>()
        .decisions
        .connections[0]
        .closed = vec![ClosedActor {
        id: DomainId(7),
        close_reason: 3,
    }];
    app.world_mut().run_schedule(FixedUpdate);
    app.world_mut()
        .resource_mut::<ReplicationFrame>()
        .decisions
        .connections[0]
        .closed
        .clear();
    assert_eq!(stats(&app).actor_opens_sent, 2);
    assert_eq!(stats(&app).actor_closes_sent, 1);
    acknowledge_lab(&mut app, &mut client);
    let before = stats(&app).actor_updates_sent;
    app.world_mut().run_schedule(FixedUpdate);
    assert_eq!(stats(&app).actor_updates_sent, before);
    assert_eq!(app.world().get::<Position>(actor).unwrap().0, location);
}

#[test]
fn a_close_that_cannot_be_framed_stays_pending_until_retried() {
    use icarus_bevy::experimental::session_open::{ActorOpenSpec, PathSegment, map_level_chain};
    let mut config = PeerLabConfig {
        admission: Some(PeerLabAdmission::default()),
        ..PeerLabConfig::default()
    };
    // A small sent-record ring: unacknowledged lab packets fill it quickly,
    // and a full ring makes framing fail without failing the peer.
    config.packet.sent_capacity = 12;
    let (mut app, address) = lab_app(config);
    let mut client = Client::connect(&mut app, address);
    client.exchange(&mut app, short_hello("", 0), false);
    let login = ControlMessage::Login {
        response: ArchiveText::text("0").unwrap(),
        url: ArchiveText::text("?Name=Tester").unwrap(),
        unique_net_id: UniqueNetId {
            kind: NetIdType::Named(ArchiveText::text("STEAM").unwrap()),
            value: NetIdValue::from_hex(NET_ID_HEX).unwrap(),
        },
        platform: ArchiveText::text(PLATFORM_SENTINEL).unwrap(),
    };
    client.exchange(&mut app, login, false);
    client.exchange(&mut app, ControlMessage::Join, false);
    accept_all_headers(&mut client);
    let (ack, _) = client.packet(Vec::new());
    client.send(&ack);
    app.update();
    accept_all_headers(&mut client);
    let connection = lab(&app).joined_connections()[0];
    let spec = ActorOpenSpec {
        actor: 0,
        archetype: vec![
            PathSegment::new("Default__LabCrate_C", 0x1234_5678, false),
            PathSegment::new("/Game/Lab/LabCrate", 0, false),
        ],
        level: map_level_chain(PeerLabAdmission::default().map_url).unwrap(),
        location: Some([0.0; 3]),
        rotation: None,
        velocity: None,
        blocks: Vec::new(),
    };

    // Open actors without the client acknowledging until framing fails.
    let mut lab_mut = app.world_mut().resource_mut::<PeerLab>();
    let mut opened = 0;
    while lab_mut
        .open_actor(connection, 100 + opened, spec.clone(), 2, 17)
        .is_some()
    {
        opened += 1;
        assert!(opened < 64, "the ring should fill");
    }
    assert!(opened >= 1);
    assert!(!lab_mut.close_actor(connection, 100, 3));
    assert!(lab_mut.is_actor_open(connection, 100), "kept open, pending");
    assert_eq!(lab_mut.stats().actor_close_errors, 1);
    // Still full: a retry fails again and keeps the close pending.
    lab_mut.retry_pending_closes();
    assert_eq!(lab_mut.stats().actor_closes_sent, 0);

    // Once the client acknowledges, the retry frames and sends the close.
    accept_all_headers(&mut client);
    let (ack, _) = client.packet(Vec::new());
    client.send(&ack);
    app.update();
    let mut lab_mut = app.world_mut().resource_mut::<PeerLab>();
    lab_mut.retry_pending_closes();
    assert_eq!(lab_mut.stats().actor_closes_sent, 1);
    assert!(!lab_mut.is_actor_open(connection, 100));
    lab_mut.retry_pending_closes();
    assert_eq!(lab_mut.stats().actor_closes_sent, 1, "sent once");
}

#[test]
fn a_naked_movement_update_is_sent_again() {
    movement_loss_case(false);
}

#[test]
fn a_naked_authored_motion_update_is_sent_again() {
    movement_loss_case(true);
}

fn movement_loss_case(authored_motion: bool) {
    use icarus_bevy::experimental::session_open::{ActorOpenSpec, PathSegment, map_level_chain};
    let config = PeerLabConfig {
        admission: Some(PeerLabAdmission::default()),
        ..PeerLabConfig::default()
    };
    let (mut app, address) = lab_app(config);
    let mut client = Client::connect(&mut app, address);
    client.exchange(&mut app, short_hello("", 0), false);
    let login = ControlMessage::Login {
        response: ArchiveText::text("0").unwrap(),
        url: ArchiveText::text("?Name=Tester").unwrap(),
        unique_net_id: UniqueNetId {
            kind: NetIdType::Named(ArchiveText::text("STEAM").unwrap()),
            value: NetIdValue::from_hex(NET_ID_HEX).unwrap(),
        },
        platform: ArchiveText::text(PLATFORM_SENTINEL).unwrap(),
    };
    client.exchange(&mut app, login, false);
    client.exchange(&mut app, ControlMessage::Join, false);
    accept_all_headers(&mut client);
    let connection = lab(&app).joined_connections()[0];
    let update = |lab: &mut PeerLab, value: f32| {
        if authored_motion {
            lab.update_actor_motion(
                connection,
                7,
                [0.0; 3],
                [value, 0.0, 0.0],
                [0.0, value, 0.0],
                17,
            )
        } else {
            lab.update_actor_movement(connection, 7, [value, 0.0, 0.0], 17)
        }
    };
    let spec = ActorOpenSpec {
        actor: 0,
        archetype: vec![
            PathSegment::new("Default__LabCart_C", 0x0cab_0cab, false),
            PathSegment::new("/Game/Lab/LabCart", 0, false),
        ],
        level: map_level_chain(PeerLabAdmission::default().map_url).unwrap(),
        location: Some([0.0; 3]),
        rotation: None,
        velocity: None,
        blocks: Vec::new(),
    };
    let mut lab_mut = app.world_mut().resource_mut::<PeerLab>();
    lab_mut.open_actor(connection, 7, spec, 2, 17).unwrap();
    // Not settled yet: no update before the client acknowledges the open.
    assert!(!update(&mut lab_mut, 50.0));

    // The client acknowledges the open.
    accept_all_headers(&mut client);
    let (ack, _) = client.packet(Vec::new());
    client.send(&ack);
    app.update();
    accept_all_headers(&mut client);
    let mut lab_mut = app.world_mut().resource_mut::<PeerLab>();
    assert!(update(&mut lab_mut, 50.0));
    assert!(!update(&mut lab_mut, 50.0), "unchanged location");
    // The update datagram is lost.
    assert_eq!(drain(&client).len(), 1);

    // The client's next packet draws an ack it receives, so the packet after
    // that reports the lost one: a nak.
    let (keepalive, _) = client.packet(Vec::new());
    client.send(&keepalive);
    app.update();
    accept_all_headers(&mut client);
    let (report, _) = client.packet(Vec::new());
    client.send(&report);
    app.update();
    accept_all_headers(&mut client);
    let mut lab_mut = app.world_mut().resource_mut::<PeerLab>();
    assert_eq!(lab_mut.stats().actor_update_naks, 1);
    assert_eq!(lab_mut.stats().actor_updates_lost, 1);
    // The same location is due again and goes out once more.
    assert!(update(&mut lab_mut, 50.0));
    assert_eq!(lab_mut.stats().actor_updates_sent, 2);

    // A nak for an older update does not undo a newer location: the resent
    // update is lost too, but the actor moves on before the nak arrives.
    assert_eq!(drain(&client).len(), 1);
    let mut lab_mut = app.world_mut().resource_mut::<PeerLab>();
    assert!(update(&mut lab_mut, 60.0));
    accept_all_headers(&mut client);
    let (report, _) = client.packet(Vec::new());
    client.send(&report);
    app.update();
    let mut lab_mut = app.world_mut().resource_mut::<PeerLab>();
    // The nak arrived (two naks so far) but the newer update supersedes it.
    assert_eq!(lab_mut.stats().actor_update_naks, 2);
    assert_eq!(lab_mut.stats().actor_updates_lost, 1, "superseded");
    assert!(!update(&mut lab_mut, 60.0));

    // A -> B -> A: the nak of the first A finds a newer packet carrying A in
    // flight, so it repairs nothing (the guard compares packets, not places).
    accept_all_headers(&mut client);
    assert!(update(&mut lab_mut, 70.0));
    assert_eq!(drain(&client).len(), 1, "the first A is lost");
    let mut lab_mut = app.world_mut().resource_mut::<PeerLab>();
    assert!(update(&mut lab_mut, 80.0));
    assert!(update(&mut lab_mut, 70.0));
    accept_all_headers(&mut client);
    let (report, _) = client.packet(Vec::new());
    client.send(&report);
    app.update();
    let lab_ref = lab(&app);
    assert_eq!(lab_ref.stats().actor_update_naks, 3);
    assert_eq!(lab_ref.stats().actor_updates_lost, 1);
}

/// GameState update bunches (unreliable, not opens) on its channel among
/// `datagrams` (one connection's, in arrival order), with unreliable partial
/// bunches joined as the client's `PartialBunchReassembler` joins them: the
/// packet ID (the 14-bit sequence, unwrapped here) is the part's sequence, so
/// a gap drops the bunch.
fn game_state_update_bunches(datagrams: &[Vec<u8>]) -> Vec<Bunch> {
    use icarus_bevy::experimental::{
        game_state_open::GAME_STATE_CHANNEL_INDEX,
        sequencing::{
            MAX_CONSTRUCTED_PARTIAL_BUNCH_BYTES, PartialBunchReassembler, PartialReceive,
        },
    };
    let mut reassembler = PartialBunchReassembler::new();
    let mut packet_id: Option<(u16, i64)> = None;
    let mut bunches = Vec::new();
    for datagram in datagrams {
        let packet = read_packet(datagram, &to_client()).unwrap();
        let sequence = packet.header.sequence;
        let id = packet_id.map_or(i64::from(sequence), |(last, id)| {
            id + i64::from(sequence.wrapping_sub(last) & 0x3fff)
        });
        packet_id = Some((sequence, id));
        for bunch in packet.bunches {
            let header = &bunch.header;
            if header.channel_index != GAME_STATE_CHANNEL_INDEX || header.open || header.reliable {
                continue;
            }
            if !header.partial {
                bunches.push(bunch);
                continue;
            }
            if let Ok(PartialReceive::Complete { bunch, .. }) = reassembler.receive_part(
                id,
                header,
                &bunch.payload,
                Some(MAX_CONSTRUCTED_PARTIAL_BUNCH_BYTES),
            ) {
                bunches.push(bunch);
            }
        }
    }
    bunches
}

/// Decoded GameState property updates (not opens) on its channel among
/// `datagrams`, in arrival order.
fn game_state_updates(
    datagrams: &[Vec<u8>],
    layout: &[icarus_bevy::experimental::rep_layout::RepCommand],
) -> Vec<std::collections::BTreeMap<u32, icarus_bevy::experimental::rep_layout::RepValue>> {
    use icarus_bevy::experimental::{
        actor_channel::{ActorChannelCodec, ActorChannelLimits, read_actor_bunch_body},
        archive::ByteOrder,
        rep_layout::{RepLayoutCodec, RepLayoutLimits, read_rep_properties},
    };
    let codec = ActorChannelCodec {
        engine_net_version: 17,
        exports_enabled: false,
        has_level: true,
        order: ByteOrder::Little,
        limits: ActorChannelLimits::default(),
    };
    let rep_codec = RepLayoutCodec {
        actor: codec,
        checksums: false,
        limits: RepLayoutLimits::default(),
    };
    game_state_update_bunches(datagrams)
        .into_iter()
        .map(|bunch| {
            let body = read_actor_bunch_body(&mut bunch.payload.reader(), false, &codec).unwrap();
            assert_eq!(body.blocks.len(), 1, "one actor block");
            read_rep_properties(&mut body.blocks[0].payload.reader(), layout, &rep_codec)
                .unwrap()
                .entries
                .into_iter()
                .map(|entry| (entry.handle, entry.value))
                .collect()
        })
        .collect()
}

fn active_prospect_lab(
    mut config: PeerLabConfig,
    initial: &icarus_bevy::experimental::prospect_info::ProspectInfo,
) -> (
    App,
    SocketAddr,
    Vec<icarus_bevy::experimental::rep_layout::RepCommand>,
) {
    use icarus_bevy::experimental::{
        game_state_open::{GAME_STATE_CHANNEL_INDEX, GameStateOpenState, active_prospect_values},
        release::ReleaseProfile,
        session_open::ActorKey,
    };
    config.admission = Some(PeerLabAdmission::default());
    let profile = ReleaseProfile::release_3_0_32();
    let map = PeerLabAdmission::default().map_url;
    let plan = profile.game_state_open_plan(map).unwrap().unwrap();
    let layout = plan.layout.clone();
    let mut bound = PeerLab::bind(0, config).unwrap();
    bound
        .enable_player_controller_open(
            profile
                .player_controller_open_plan(map, [0.0; 3])
                .unwrap()
                .unwrap(),
        )
        .unwrap();
    bound
        .enable_game_state_open(
            plan,
            GameStateOpenState {
                key: ActorKey::Session(0),
                managers: Default::default(),
                values: active_prospect_values(initial).unwrap(),
                weather: Vec::new(),
            },
            GAME_STATE_CHANNEL_INDEX,
        )
        .unwrap();
    let address = bound.local_addr().unwrap();
    let mut app = build_app(&ServerConfig::default(), &WorldSnapshot::default()).unwrap();
    app.insert_resource(bound).add_plugins(PeerLabPlugin);
    (app, address, layout)
}

fn join_active_prospect_peer(app: &mut App, address: SocketAddr) -> Client {
    let mut client = Client::connect(app, address);
    client.exchange(app, short_hello("", 0), false);
    client.exchange(app, login(), false);
    client.exchange(app, ControlMessage::Join, false);
    client
}

fn acknowledge_datagrams(client: &mut Client, datagrams: &[Vec<u8>]) {
    for datagram in datagrams {
        let packet = read_packet(datagram, &to_client()).unwrap();
        if let Some(received) = client.sequencer.receive(&packet.header).unwrap() {
            client.sequencer.ack_seq(received.sequence, true).unwrap();
        }
    }
}

fn prospect_update_step(app: &mut App, client: &mut Client) -> Vec<Vec<u8>> {
    use icarus_bevy::experimental::peer_lab::GAME_STATE_UPDATE_SECONDS;
    let (ack, _) = client.packet(Vec::new());
    client.send(&ack);
    thread::sleep(Duration::from_secs_f32(GAME_STATE_UPDATE_SECONDS * 1.5));
    app.update();
    let datagrams = drain(client);
    acknowledge_datagrams(client, &datagrams);
    datagrams
}

#[test]
fn explicit_prospect_refresh_waits_for_open_ack_deduplicates_and_clears() {
    use icarus_bevy::experimental::{
        game_state_open::{active_prospect_update_values, handle},
        prospect_info::{AssociatedMember, CustomGameSetting, ProspectInfo},
        rep_layout::RepValue,
    };
    let initial = ProspectInfo::default();
    let (mut app, address, layout) = active_prospect_lab(PeerLabConfig::default(), &initial);
    let mut client = join_active_prospect_peer(&mut app, address);
    let current = ProspectInfo {
        prospect_id: "prospect".encode_utf16().collect(),
        claimed_account_id: "account".encode_utf16().collect(),
        claimed_account_character: 1,
        prospect_dt_key: "row".encode_utf16().collect(),
        faction_mission_dt_key: "mission".encode_utf16().collect(),
        lobby_name: "lobby".encode_utf16().collect(),
        expire_time: -1,
        prospect_state: 2,
        associated_members: vec![AssociatedMember {
            // Matching a lab name must not alter this supplied false flag.
            account_name: "Player256".encode_utf16().collect(),
            is_currently_playing: false,
            ..Default::default()
        }],
        cost: 1,
        reward: 2,
        difficulty: 3,
        insurance: true,
        no_respawns: true,
        elapsed_time: 71,
        selected_drop_point: 1,
        custom_settings: vec![CustomGameSetting {
            setting_row_name: "setting".encode_utf16().collect(),
            setting_value: 2,
        }],
    };
    app.world_mut()
        .resource_mut::<PeerLab>()
        .refresh_active_prospect(&current)
        .unwrap();
    thread::sleep(Duration::from_millis(60));
    app.update();
    assert_eq!(
        stats(&app).game_state_updates_sent,
        0,
        "open is unacknowledged"
    );
    let pending = drain(&client);
    acknowledge_datagrams(&mut client, &pending);
    let updates = game_state_updates(&prospect_update_step(&mut app, &mut client), &layout);
    let expected = active_prospect_update_values(&current).unwrap();
    let seen: std::collections::BTreeMap<_, _> = updates
        .iter()
        .flat_map(|values| values.iter())
        .filter(|(h, _)| handle::ACTIVE_PROSPECT.contains(h))
        .map(|(h, v)| (*h, v.clone()))
        .collect();
    assert_eq!(seen, expected, "all 17 non-default top-level fields change");
    // An identical explicit refresh and ordinary polls carry no prospect.
    app.world_mut()
        .resource_mut::<PeerLab>()
        .refresh_active_prospect(&current)
        .unwrap();
    for _ in 0..3 {
        for update in game_state_updates(&prospect_update_step(&mut app, &mut client), &layout) {
            assert!(!update.keys().any(|h| handle::ACTIVE_PROSPECT.contains(h)));
        }
    }
    app.world_mut()
        .resource_mut::<PeerLab>()
        .refresh_active_prospect(&ProspectInfo::default())
        .unwrap();
    let updates = game_state_updates(&prospect_update_step(&mut app, &mut client), &layout);
    let clear = updates
        .iter()
        .find(|values| values.contains_key(&51))
        .unwrap();
    for h in handle::ACTIVE_PROSPECT {
        assert_eq!(
            clear.get(&h),
            icarus_bevy::experimental::game_state_open::class_default(h).as_ref(),
            "clear {h}"
        );
    }
    assert_eq!(clear.get(&63), Some(&RepValue::Bool(false)));
}

#[test]
fn later_join_uses_refreshed_template_and_never_infers_a_refresh() {
    use icarus_bevy::experimental::{
        actor_channel::{ActorChannelCodec, ActorChannelLimits, read_actor_bunch_body},
        archive::ByteOrder,
        game_state_open::{GAME_STATE_CHANNEL_INDEX, active_prospect_values, handle},
        prospect_info::{AssociatedMember, ProspectInfo},
        rep_layout::{RepLayoutCodec, RepLayoutLimits, read_rep_properties},
    };
    let initial = ProspectInfo {
        elapsed_time: 123,
        insurance: true,
        ..Default::default()
    };
    let (mut app, address, layout) = active_prospect_lab(PeerLabConfig::default(), &initial);
    let mut first = join_active_prospect_peer(&mut app, address);
    accept_all_headers(&mut first);
    // Template initialization alone must not schedule active-prospect updates.
    for _ in 0..3 {
        assert!(
            game_state_updates(&prospect_update_step(&mut app, &mut first), &layout)
                .iter()
                .all(|values| !values.keys().any(|h| handle::ACTIVE_PROSPECT.contains(h)))
        );
    }
    let latest = ProspectInfo {
        lobby_name: "latest".encode_utf16().collect(),
        associated_members: vec![AssociatedMember {
            account_name: "Player257".encode_utf16().collect(),
            is_currently_playing: false,
            ..Default::default()
        }],
        elapsed_time: 77,
        ..Default::default()
    };
    app.world_mut()
        .resource_mut::<PeerLab>()
        .refresh_active_prospect(&latest)
        .unwrap();
    prospect_update_step(&mut app, &mut first);
    let mut second = join_active_prospect_peer(&mut app, address);
    let datagrams = drain(&second);
    let bunch = datagrams
        .iter()
        .flat_map(|bytes| read_packet(bytes, &to_client()).unwrap().bunches)
        .find(|bunch| {
            bunch.header.channel_index == GAME_STATE_CHANNEL_INDEX
                && !bunch.header.has_package_map_exports
        })
        .unwrap();
    let actor = ActorChannelCodec {
        engine_net_version: 17,
        exports_enabled: false,
        has_level: true,
        order: ByteOrder::Little,
        limits: ActorChannelLimits::default(),
    };
    let body = read_actor_bunch_body(&mut bunch.payload.reader(), true, &actor).unwrap();
    let values: std::collections::BTreeMap<_, _> = read_rep_properties(
        &mut body.blocks[0].payload.reader(),
        &layout,
        &RepLayoutCodec {
            actor,
            checksums: false,
            limits: RepLayoutLimits::default(),
        },
    )
    .unwrap()
    .entries
    .into_iter()
    .filter(|entry| handle::ACTIVE_PROSPECT.contains(&entry.handle))
    .map(|entry| (entry.handle, entry.value))
    .collect();
    assert_eq!(values, active_prospect_values(&latest).unwrap());
    acknowledge_datagrams(&mut second, &datagrams);
    for _ in 0..3 {
        for update in game_state_updates(&prospect_update_step(&mut app, &mut second), &layout) {
            assert!(
                !update.keys().any(|h| handle::ACTIVE_PROSPECT.contains(h)),
                "the open established the latest baseline"
            );
        }
        for update in game_state_updates(&prospect_update_step(&mut app, &mut first), &layout) {
            assert!(
                !update.keys().any(|h| handle::ACTIVE_PROSPECT.contains(h)),
                "join does not infer member flags or elapsed time"
            );
        }
    }
}

#[test]
fn a_lost_prospect_partial_repairs_current_state_and_superseded_updates_stay_retired() {
    use icarus_bevy::experimental::{
        game_state_open::{GAME_STATE_CHANNEL_INDEX, handle},
        prospect_info::ProspectInfo,
        rep_layout::RepValue,
    };
    let (mut app, address, layout) =
        active_prospect_lab(PeerLabConfig::default(), &ProspectInfo::default());
    let mut client = join_active_prospect_peer(&mut app, address);
    accept_all_headers(&mut client);
    prospect_update_step(&mut app, &mut client);
    let refresh = |app: &mut App, letter: u16, elapsed: i32| {
        app.world_mut()
            .resource_mut::<PeerLab>()
            .refresh_active_prospect(&ProspectInfo {
                lobby_name: vec![letter; 3_000],
                elapsed_time: elapsed,
                ..Default::default()
            })
            .unwrap();
        thread::sleep(Duration::from_millis(60));
        app.update();
    };
    refresh(&mut app, 65, 10);
    let sent = drain(&client);
    let parts: Vec<usize> = sent
        .iter()
        .enumerate()
        .filter_map(|(index, bytes)| {
            read_packet(bytes, &to_client())
                .unwrap()
                .bunches
                .iter()
                .any(|bunch| {
                    bunch.header.channel_index == GAME_STATE_CHANNEL_INDEX
                        && bunch.header.partial
                        && !bunch.header.reliable
                })
                .then_some(index)
        })
        .collect();
    assert!(parts.len() >= 3, "a real multipart prospect update");
    let lost = parts[1];
    let delivered: Vec<Vec<u8>> = sent
        .into_iter()
        .enumerate()
        .filter(|(index, _)| *index != lost)
        .map(|(_, bytes)| bytes)
        .collect();
    assert!(
        game_state_updates(&delivered, &layout).is_empty(),
        "the missing middle part prevents assembly"
    );
    acknowledge_datagrams(&mut client, &delivered);
    // The next ACK reports the gap; losing any part makes the whole latest
    // prospect update due, and it reassembles from consecutive repair parts.
    let repaired = game_state_updates(&prospect_update_step(&mut app, &mut client), &layout);
    assert!(repaired.iter().any(|values| matches!(values.get(&56), Some(RepValue::Str(text)) if text.units() == vec![65; 3_000])));
    assert!(stats(&app).game_state_updates_lost >= 1);

    refresh(&mut app, 66, 11);
    let lost_old = drain(&client);
    assert!(
        game_state_updates(&lost_old, &layout)
            .iter()
            .any(|values| values.get(&65) == Some(&RepValue::Int(11)))
    );
    // A newer complete state is sent before the old gap is reported.
    refresh(&mut app, 67, 12);
    let latest = drain(&client);
    assert!(
        game_state_updates(&latest, &layout)
            .iter()
            .any(|values| values.get(&65) == Some(&RepValue::Int(12)))
    );
    acknowledge_datagrams(&mut client, &latest);
    let after_nak = game_state_updates(&prospect_update_step(&mut app, &mut client), &layout);
    assert!(
        after_nak
            .iter()
            .all(|values| !values.keys().any(|h| handle::ACTIVE_PROSPECT.contains(h))),
        "the old NAK cannot undo newer prospect values"
    );
    // A later refresh supersedes a lost value before repair is generated:
    // the repair reads current state, rather than retaining the old payload.
    refresh(&mut app, 68, 13);
    drain(&client);
    let (keepalive, _) = client.packet(Vec::new());
    client.send(&keepalive);
    app.update();
    accept_all_headers(&mut client);
    app.world_mut()
        .resource_mut::<PeerLab>()
        .refresh_active_prospect(&ProspectInfo {
            lobby_name: vec![69; 3_000],
            elapsed_time: 14,
            ..Default::default()
        })
        .unwrap();
    let repaired_latest = game_state_updates(&prospect_update_step(&mut app, &mut client), &layout);
    assert!(
        repaired_latest
            .iter()
            .any(|values| values.get(&65) == Some(&RepValue::Int(14)))
    );
    assert!(
        repaired_latest
            .iter()
            .all(|values| values.get(&65) != Some(&RepValue::Int(13)))
    );
}

#[test]
fn game_state_updates_advance_the_clocks_once_the_open_is_acked() {
    use icarus_bevy::experimental::{
        game_state_open::{GAME_STATE_CHANNEL_INDEX, GameStateOpenState, handle},
        peer_lab::GAME_STATE_UPDATE_SECONDS,
        release::ReleaseProfile,
        rep_layout::RepValue,
        server_fps::ServerFpsMeter,
        session_open::ActorKey,
        time_of_day::{RateCurve, TimeOfDayClock},
    };
    let config = PeerLabConfig {
        admission: Some(PeerLabAdmission::default()),
        ..PeerLabConfig::default()
    };
    let mut app = build_app(&ServerConfig::default(), &WorldSnapshot::default()).unwrap();
    let mut bound = PeerLab::bind(0, config).unwrap();
    let profile = ReleaseProfile::release_3_0_32();
    let map_url = PeerLabAdmission::default().map_url;
    let plan = profile.game_state_open_plan(map_url).unwrap().unwrap();
    let layout = plan.layout.clone();
    let clock = TimeOfDayClock::new(RateCurve::constant(60.0).unwrap(), 600.0);
    assert!(
        bound.enable_time_of_day(clock.clone()).is_err(),
        "the clock needs a GameState open"
    );
    bound
        .enable_player_controller_open(
            profile
                .player_controller_open_plan(map_url, [0.0; 3])
                .unwrap()
                .unwrap(),
        )
        .unwrap();
    bound
        .enable_game_state_open(
            plan,
            GameStateOpenState {
                key: ActorKey::Session(0),
                managers: Default::default(),
                values: Default::default(),
                weather: Vec::new(),
            },
            GAME_STATE_CHANNEL_INDEX,
        )
        .unwrap();
    bound.set_game_state_level_time(1_000).unwrap();
    bound.enable_time_of_day(clock).unwrap();
    bound
        .enable_server_fps(ServerFpsMeter::new(64).unwrap())
        .unwrap();
    let address = bound.local_addr().unwrap();
    app.insert_resource(bound).add_plugins(PeerLabPlugin);
    let mut client = Client::connect(&mut app, address);
    client.exchange(&mut app, short_hello("", 0), false);
    client.exchange(&mut app, login(), false);
    client.exchange(&mut app, ControlMessage::Join, false);
    assert_eq!(stats(&app).game_state_opens_sent, 1);
    let pause = || std::thread::sleep(Duration::from_secs_f32(GAME_STATE_UPDATE_SECONDS * 2.0));

    // No update before the client acknowledges the open.
    pause();
    app.update();
    assert_eq!(stats(&app).game_state_updates_sent, 0);
    accept_all_headers(&mut client);
    let (ack, _) = client.packet(Vec::new());
    client.send(&ack);

    // Then each pass sends what changed: the time of day every time (one
    // game minute per world second here), the world time and the level clock
    // when due, and ServerFPS once a 2 s window of polls closed. Passes run
    // until eight updates span 2.3 s (the level clock's whole second and the
    // first ServerFPS window), bounded for slow runners; the 0.1 s
    // world-time rule itself is pinned by the replica unit test.
    let mut updates = Vec::new();
    let started = std::time::Instant::now();
    while (updates.len() < 8 || started.elapsed() < Duration::from_millis(2_300))
        && started.elapsed() < Duration::from_secs(20)
    {
        pause();
        app.update();
        let datagrams = drain(&client);
        for datagram in &datagrams {
            let packet = read_packet(datagram, &to_client()).unwrap();
            if let Some(received) = client.sequencer.receive(&packet.header).unwrap() {
                client.sequencer.ack_seq(received.sequence, true).unwrap();
            }
        }
        updates.extend(game_state_updates(&datagrams, &layout));
        let (ack, _) = client.packet(Vec::new());
        client.send(&ack);
    }
    let counted = stats(&app);
    assert_eq!(counted.game_state_update_errors, 0);
    assert_eq!(counted.game_state_updates_sent, updates.len() as u64);
    assert!(updates.len() >= 8, "{} updates", updates.len());
    let float = |value: &RepValue| match value {
        RepValue::Float(bits) => f32::from_bits(*bits),
        other => panic!("{other:?}"),
    };
    let times: Vec<f32> = updates
        .iter()
        .map(|update| float(&update[&handle::TIME_OF_DAY]))
        .collect();
    assert!(times[0] > 600.0);
    assert!(times.windows(2).all(|pair| pair[0] < pair[1]), "{times:?}");
    let worlds: Vec<f32> = updates
        .iter()
        .filter_map(|update| update.get(&handle::WORLD_TIME_SECONDS).map(float))
        .collect();
    assert!(worlds.len() >= 2);
    assert!(worlds.windows(2).all(|pair| pair[1] - pair[0] >= 0.1));
    // The level clock moved past its restored value after 1.1 s.
    let levels: Vec<&RepValue> = updates
        .iter()
        .filter_map(|update| update.get(&handle::LEVEL_TIME_ELAPSED_SEC))
        .collect();
    assert!(!levels.is_empty());
    assert!(
        levels
            .iter()
            .all(|level| matches!(level, RepValue::Int(seconds) if *seconds > 1_000))
    );
    // The lab's poll rate, about 1 / (2 * GAME_STATE_UPDATE_SECONDS) here.
    let rates: Vec<&RepValue> = updates
        .iter()
        .filter_map(|update| update.get(&handle::SERVER_FPS))
        .collect();
    assert!(!rates.is_empty());
    assert!(
        rates
            .iter()
            .all(|rate| matches!(rate, RepValue::Int(fps) if (1..=1_000).contains(fps))),
        "{rates:?}"
    );
    assert_eq!(
        lab(&app).server_fps().map(ServerFpsMeter::fps),
        Some(match rates.last() {
            Some(RepValue::Int(fps)) => *fps,
            other => panic!("{other:?}"),
        })
    );
    let advanced = lab(&app).time_of_day().unwrap().time_of_day();
    assert!(advanced >= *times.last().unwrap());
}

#[test]
fn the_networking_status_lists_joined_players_in_join_order() {
    use icarus_bevy::experimental::{
        game_state_open::{GAME_STATE_CHANNEL_INDEX, GameStateOpenState, NetworkingStatus, handle},
        peer_lab::GAME_STATE_UPDATE_SECONDS,
        release::ReleaseProfile,
        rep_layout::{RepArray, RepValue},
        session_open::ActorKey,
    };
    let config = PeerLabConfig {
        admission: Some(PeerLabAdmission::default()),
        ..PeerLabConfig::default()
    };
    let mut app = build_app(&ServerConfig::default(), &WorldSnapshot::default()).unwrap();
    let mut bound = PeerLab::bind(0, config).unwrap();
    let profile = ReleaseProfile::release_3_0_32();
    let map_url = PeerLabAdmission::default().map_url;
    let plan = profile.game_state_open_plan(map_url).unwrap().unwrap();
    let layout = plan.layout.clone();
    assert!(
        bound
            .enable_networking_status(NetworkingStatus::local(600.0))
            .is_err(),
        "the status needs a GameState open"
    );
    bound
        .enable_player_controller_open(
            profile
                .player_controller_open_plan(map_url, [0.0; 3])
                .unwrap()
                .unwrap(),
        )
        .unwrap();
    bound
        .enable_game_state_open(
            plan,
            GameStateOpenState {
                key: ActorKey::Session(0),
                managers: Default::default(),
                values: Default::default(),
                weather: Vec::new(),
            },
            GAME_STATE_CHANNEL_INDEX,
        )
        .unwrap();
    let mut listed = NetworkingStatus::local(600.0);
    listed.player_names.push(Vec::new());
    listed.player_ping.push(0);
    assert!(
        bound.enable_networking_status(listed).is_err(),
        "the lab lists players itself"
    );
    bound
        .enable_networking_status(NetworkingStatus::local(600.0))
        .unwrap();
    let address = bound.local_addr().unwrap();
    app.insert_resource(bound).add_plugins(PeerLabPlugin);
    let pause = || std::thread::sleep(Duration::from_secs_f32(GAME_STATE_UPDATE_SECONDS * 2.0));
    // Acknowledge everything queued for `client` and return its GameState
    // updates' player names (handle 70), when any update carried them.
    let names = |client: &mut Client, app: &mut App| -> Option<Vec<String>> {
        let datagrams = drain(client);
        for datagram in &datagrams {
            let packet = read_packet(datagram, &to_client()).unwrap();
            if let Some(received) = client.sequencer.receive(&packet.header).unwrap() {
                client.sequencer.ack_seq(received.sequence, true).unwrap();
            }
        }
        let (ack, _) = client.packet(Vec::new());
        client.send(&ack);
        pause();
        app.update();
        game_state_updates(&datagrams, &layout)
            .into_iter()
            .rev()
            .find_map(
                |update| match update.get(&(handle::NETWORKING_STATUS_FIRST + 2)) {
                    Some(RepValue::DynamicArray(RepArray { stream, .. })) => Some(
                        stream
                            .entries
                            .iter()
                            .map(|entry| match &entry.value {
                                RepValue::Str(text) => String::from_utf16_lossy(text.units()),
                                other => panic!("{other:?}"),
                            })
                            .collect(),
                    ),
                    _ => None,
                },
            )
    };
    let mut first = Client::connect(&mut app, address);
    first.exchange(&mut app, short_hello("", 0), false);
    first.exchange(&mut app, login(), false);
    first.exchange(&mut app, ControlMessage::Join, false);
    let mut seen = None;
    for _ in 0..50 {
        if let Some(listed) = names(&mut first, &mut app) {
            seen = Some(listed);
            break;
        }
    }
    assert_eq!(seen, Some(vec!["Player256".to_owned()]));

    let mut second = Client::connect(&mut app, address);
    second.exchange(&mut app, short_hello("", 0), false);
    second.exchange(&mut app, login(), false);
    second.exchange(&mut app, ControlMessage::Join, false);
    let mut seen = None;
    for _ in 0..50 {
        names(&mut second, &mut app);
        if let Some(listed) = names(&mut first, &mut app)
            && listed.len() == 2
        {
            seen = Some(listed);
            break;
        }
    }
    assert_eq!(
        seen,
        Some(vec!["Player256".to_owned(), "Player257".to_owned()])
    );
    // Acked round trips reach the listed pings: every ack here is processed
    // at least one pause after the lab sent the packet it acknowledges, so
    // once a ping bucket closes each byte is at least a quarter of that.
    let floor = (GAME_STATE_UPDATE_SECONDS * 2.0 * 1000.0 / 4.0) as u8 - 1;
    let mut pings = Vec::new();
    for _ in 0..100 {
        names(&mut first, &mut app);
        names(&mut second, &mut app);
        pings = lab(&app).networking_status().unwrap().player_ping.clone();
        if pings.iter().all(|ping| *ping >= floor) {
            break;
        }
    }
    assert_eq!(pings.len(), 2);
    assert!(
        pings.iter().all(|ping| *ping >= floor),
        "{pings:?} < {floor}"
    );
    assert_eq!(stats(&app).game_state_update_errors, 0);
}

/// A NetworkingStatus too large for one bunch (48 players with 20-unit
/// names) goes out as unreliable partial bunches in consecutive datagrams:
/// every connection reassembles all 48 names, and the clocks keep flowing.
#[test]
fn an_oversized_networking_status_reaches_every_connection_in_partial_bunches() {
    use icarus_bevy::experimental::{
        game_state_open::{GAME_STATE_CHANNEL_INDEX, GameStateOpenState, NetworkingStatus, handle},
        peer_lab::GAME_STATE_UPDATE_SECONDS,
        release::ReleaseProfile,
        rep_layout::{RepArray, RepValue},
        session_open::ActorKey,
    };
    let mut admission = PeerLabAdmission::default();
    admission.policy.max_players = 64;
    let config = PeerLabConfig {
        admission: Some(admission),
        max_peers: 64,
        ..PeerLabConfig::default()
    };
    let mut app = build_app(&ServerConfig::default(), &WorldSnapshot::default()).unwrap();
    let mut bound = PeerLab::bind(0, config).unwrap();
    let profile = ReleaseProfile::release_3_0_32();
    let map_url = PeerLabAdmission::default().map_url;
    let plan = profile.game_state_open_plan(map_url).unwrap().unwrap();
    let layout = plan.layout.clone();
    bound
        .enable_player_controller_open(
            profile
                .player_controller_open_plan(map_url, [0.0; 3])
                .unwrap()
                .unwrap(),
        )
        .unwrap();
    bound
        .enable_game_state_open(
            plan,
            GameStateOpenState {
                key: ActorKey::Session(0),
                managers: Default::default(),
                values: Default::default(),
                weather: Vec::new(),
            },
            GAME_STATE_CHANNEL_INDEX,
        )
        .unwrap();
    bound
        .enable_networking_status(NetworkingStatus::local(600.0))
        .unwrap();
    let address = bound.local_addr().unwrap();
    app.insert_resource(bound).add_plugins(PeerLabPlugin);
    let mut clients = Vec::new();
    for index in 0..48 {
        let mut client = Client::connect(&mut app, address);
        client.exchange(&mut app, short_hello("", 0), false);
        let login = ControlMessage::Login {
            response: ArchiveText::text("0").unwrap(),
            url: ArchiveText::text(&format!("?Name=P{index:019}")).unwrap(),
            unique_net_id: UniqueNetId {
                kind: NetIdType::Default,
                value: NetIdValue::from_hex(NET_ID_HEX).unwrap(),
            },
            platform: ArchiveText::text(PLATFORM_SENTINEL).unwrap(),
        };
        client.exchange(&mut app, login, false);
        client.exchange(&mut app, ControlMessage::Join, false);
        clients.push(client);
    }
    assert_eq!(lab(&app).joined_connections().len(), 48);
    let expected: Vec<String> = (0..48).map(|index| format!("P{index:019}")).collect();
    // Every client acknowledges what it received; each pass's datagrams are
    // drained per client in arrival order, so split updates reassemble.
    let mut listed = vec![false; clients.len()];
    let mut split_parts = 0;
    let mut world_times = 0;
    for _ in 0..40 {
        for (index, client) in clients.iter_mut().enumerate() {
            let datagrams = drain(client);
            for datagram in &datagrams {
                let packet = read_packet(datagram, &to_client()).unwrap();
                split_parts += packet
                    .bunches
                    .iter()
                    .filter(|bunch| {
                        let header = &bunch.header;
                        header.channel_index == GAME_STATE_CHANNEL_INDEX
                            && header.partial
                            && !header.reliable
                    })
                    .count();
                if let Some(received) = client.sequencer.receive(&packet.header).unwrap() {
                    client.sequencer.ack_seq(received.sequence, true).unwrap();
                }
            }
            for update in game_state_updates(&datagrams, &layout) {
                if index == 0 && update.contains_key(&handle::WORLD_TIME_SECONDS) {
                    world_times += 1;
                }
                let Some(RepValue::DynamicArray(RepArray { stream, .. })) =
                    update.get(&(handle::NETWORKING_STATUS_FIRST + 2))
                else {
                    continue;
                };
                let names: Vec<String> = stream
                    .entries
                    .iter()
                    .map(|entry| match &entry.value {
                        RepValue::Str(text) => String::from_utf16_lossy(text.units()),
                        other => panic!("{other:?}"),
                    })
                    .collect();
                assert_eq!(names, expected);
                listed[index] = true;
            }
            let (ack, _) = client.packet(Vec::new());
            client.send(&ack);
        }
        if listed.iter().all(|seen| *seen) && world_times >= 3 {
            break;
        }
        std::thread::sleep(Duration::from_secs_f32(GAME_STATE_UPDATE_SECONDS * 2.0));
        // Two polls take every client's ack (32 datagrams per poll).
        app.update();
        app.update();
    }
    let missing: Vec<usize> = (0..listed.len()).filter(|index| !listed[*index]).collect();
    assert!(
        missing.is_empty(),
        "no NetworkingStatus reached {missing:?}"
    );
    assert!(world_times >= 3, "{world_times} world-time updates");
    assert!(
        split_parts >= 2 * 48,
        "{split_parts} partial update bunches"
    );
    let counted = stats(&app);
    assert_eq!(counted.game_state_update_errors, 0);
    assert!(
        counted.game_state_update_datagrams > counted.game_state_updates_sent,
        "{} datagrams for {} updates",
        counted.game_state_update_datagrams,
        counted.game_state_updates_sent
    );
    assert_eq!(
        lab(&app).networking_status().unwrap().player_names.len(),
        48
    );
}

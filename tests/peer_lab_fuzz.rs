//! Deterministic randomized robustness check for the loopback peer lab: random
//! and mutated datagrams from connected and unknown loopback sockets never
//! panic the lab, its peer table stays within capacity, and its counters stay
//! consistent. Authored synthetic client only; no captured traffic.
use std::{
    io::ErrorKind,
    net::{SocketAddr, UdpSocket},
    time::Duration,
};

use bevy::app::App;
use icarus_bevy::{
    config::ServerConfig,
    experimental::{
        control_messages::{
            ArchiveText, ControlCodec, ControlMessage, NetIdType, NetIdValue, UniqueNetId,
            encode_control_message,
        },
        handshake::{decode, initial_request},
        packet::{
            BitWriter, Bunch, BunchHeader, Name, Packet, PacketConfig, Payload, Receiver,
            write_packet,
        },
        packet_lab::client_cookie_seeds,
        peer_lab::{PeerLab, PeerLabConfig, PeerLabPlugin},
        sequencing::{PacketSequencer, wire_reliable_sequence},
    },
    simulation::{WorldSnapshot, build_app},
};

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    fn below(&mut self, bound: usize) -> usize {
        (self.next() % bound as u64) as usize
    }
}

fn socket(lab: SocketAddr) -> UdpSocket {
    let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    socket.connect(lab).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(1)))
        .unwrap();
    socket
}

/// Read and discard every queued reply so loopback buffers never fill.
fn drain(socket: &UdpSocket) -> usize {
    socket.set_nonblocking(true).unwrap();
    let mut buffer = [0; 2048];
    let mut count = 0;
    loop {
        match socket.recv(&mut buffer) {
            Ok(_) => count += 1,
            Err(error) if error.kind() == ErrorKind::WouldBlock => break,
            Err(error) => panic!("{error}"),
        }
    }
    socket.set_nonblocking(false).unwrap();
    count
}

fn to_server() -> PacketConfig {
    PacketConfig {
        magic_bits: 0,
        receiver: Receiver::Server,
        limits: PeerLabConfig::default().packet.packet.limits,
    }
}

fn text(value: &str) -> ArchiveText {
    ArchiveText::text(value).unwrap()
}

fn control_bunch(raw: u32, open: bool, message: &ControlMessage) -> Bunch {
    let mut writer = BitWriter::new();
    writer.write_bytes(&encode_control_message(message, &ControlCodec::default()).unwrap());
    Bunch {
        header: BunchHeader {
            control: open,
            open,
            close: false,
            close_reason: None,
            paused: false,
            reliable: true,
            channel_index: 0,
            has_package_map_exports: false,
            has_must_be_mapped_guids: false,
            partial: false,
            reliable_sequence_raw: Some(raw),
            partial_initial: false,
            partial_final: false,
            channel_name: Some(Name::Hardcoded(255)),
        },
        payload: Payload::from_writer(writer),
    }
}

/// Handshake one socket, then build a run of in-order data packets with
/// control bunches from that client's sequencing model.
fn connected_corpus(app: &mut App, socket: &UdpSocket) -> Vec<Vec<u8>> {
    socket.send(&initial_request()).unwrap();
    app.update();
    let mut buffer = [0; 2048];
    let length = socket.recv(&mut buffer).unwrap();
    let challenge = buffer[..length].to_vec();
    let cookie = decode(&challenge).unwrap().cookie;
    socket.send(&challenge).unwrap();
    app.update();
    drain(socket);
    let (incoming, outgoing) = client_cookie_seeds(&cookie);
    let mut sequencer = PacketSequencer::new(incoming, outgoing, 64).unwrap();
    let mut control = sequencer.new_channel();
    let messages = [
        ControlMessage::Hello {
            first_field: 1,
            version: 7,
            token: text(""),
        },
        ControlMessage::Netspeed { rate: 10_000 },
        ControlMessage::Login {
            response: text(""),
            url: text("?Name=Fuzz"),
            unique_net_id: UniqueNetId {
                kind: NetIdType::Named(text("EOS")),
                value: NetIdValue::from_hex("FF").unwrap(),
            },
            platform: text("Lab"),
        },
        ControlMessage::Join,
    ];
    let mut corpus = vec![initial_request().to_vec(), challenge];
    for (index, message) in messages.iter().enumerate() {
        let packet_id = sequencer.out_packet_id();
        let sequence = control.send_reliable(packet_id, message.clone()).unwrap();
        let bunch = control_bunch(wire_reliable_sequence(sequence), index == 0, message);
        let packet = Packet {
            magic: 0,
            header: sequencer.header().unwrap(),
            info: None,
            bunches: vec![bunch],
        };
        corpus.push(write_packet(&packet, &to_server()).unwrap());
        sequencer.commit().unwrap();
        let empty = Packet {
            magic: 0,
            header: sequencer.header().unwrap(),
            info: None,
            bunches: Vec::new(),
        };
        corpus.push(write_packet(&empty, &to_server()).unwrap());
        sequencer.commit().unwrap();
    }
    corpus
}

fn mutate(rng: &mut Rng, corpus: &[Vec<u8>]) -> Vec<u8> {
    if rng.below(6) == 0 {
        let len = rng.below(80);
        return (0..len).map(|_| rng.next() as u8).collect();
    }
    let mut bytes = corpus[rng.below(corpus.len())].clone();
    if rng.below(3) == 0 {
        // Unmutated: in-order valid packets reach the deep paths.
        return bytes;
    }
    match rng.below(3) {
        0 => {
            let at = rng.below(bytes.len() + 1);
            bytes.truncate(at);
        }
        1 => {
            for _ in 0..=rng.below(6) {
                bytes.push(rng.next() as u8);
            }
        }
        _ => {}
    }
    for _ in 0..=rng.below(3) {
        if !bytes.is_empty() {
            let at = rng.below(bytes.len());
            bytes[at] ^= 1 << rng.below(8);
        }
    }
    bytes
}

#[test]
fn random_and_mutated_datagrams_never_panic_the_lab() {
    let config = PeerLabConfig {
        max_peers: 2,
        ..PeerLabConfig::default()
    };
    let mut app = build_app(&ServerConfig::default(), &WorldSnapshot::default()).unwrap();
    let lab = PeerLab::bind(0, config).unwrap();
    let address = lab.local_addr().unwrap();
    app.insert_resource(lab).add_plugins(PeerLabPlugin);

    let sockets: Vec<UdpSocket> = (0..4).map(|_| socket(address)).collect();
    let mut corpus = connected_corpus(&mut app, &sockets[0]);
    corpus.extend(connected_corpus(&mut app, &sockets[1]));

    let mut rng = Rng(0x9ee7_1ab0_f022_0001);
    let mut sent = 0u64;
    for round in 0..3000u32 {
        let socket = &sockets[rng.below(sockets.len())];
        socket.send(&mutate(&mut rng, &corpus)).unwrap();
        sent += 1;
        if round % 8 == 7 {
            app.update();
            for socket in &sockets {
                drain(socket);
            }
            assert!(app.world().resource::<PeerLab>().peer_count() <= 2);
        }
    }
    // Let the lab take every queued datagram (it reads a bounded batch per
    // update).
    for _ in 0..8 {
        app.update();
        for socket in &sockets {
            drain(socket);
        }
    }

    let lab = app.world().resource::<PeerLab>();
    let stats = lab.stats();
    assert!(lab.peer_count() <= 2);
    // Two handshakes of two datagrams each precede the loop.
    assert_eq!(stats.received, sent + 4, "{stats:?}");
    assert!(
        stats.data_accepted > 0 && stats.data_malformed > 0,
        "{stats:?}"
    );
    assert!(
        stats.handshake_rejected > 0 || stats.challenges_sent > 2,
        "{stats:?}"
    );
}

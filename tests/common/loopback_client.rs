//! Shared authored loopback peer: the stateless handshake, Hello, Login and
//! Join through this repository's own codec and sequencing, acknowledging every
//! received packet. Panics on any unexpected transport result. Not a stock client.
#![allow(dead_code)]
use std::{
    io::ErrorKind,
    net::{SocketAddr, UdpSocket},
};

use bevy::app::App;
use icarus_bevy::experimental::{
    archive::ByteOrder,
    control_messages::{
        ArchiveText, ControlCodec, ControlMessage, NetIdType, NetIdValue, UniqueNetId,
        encode_control_message,
    },
    handshake::{decode, initial_request},
    packet::{
        BitWriter, Bunch, BunchHeader, Name, Packet, PacketConfig, Payload, Receiver, read_packet,
        write_packet,
    },
    packet_lab::client_cookie_seeds,
    peer_lab::PeerLab,
    sequencing::{PacketSequencer, ReliableChannel, wire_reliable_sequence},
};

/// Authored client sequencing harness, matching the existing peer-lab patterns.
/// Datagrams may be deliberately withheld from accept() to model loss/late ACK.
pub struct Client {
    pub socket: UdpSocket,
    pub sequencer: PacketSequencer,
    pub control: ReliableChannel<ControlMessage>,
    pub packet: PacketConfig,
    pub order: ByteOrder,
}

pub fn drain(socket: &UdpSocket) -> Vec<Vec<u8>> {
    let mut datagrams = Vec::new();
    loop {
        let mut buffer = [0; 2048];
        match socket.recv(&mut buffer) {
            Ok(length) => datagrams.push(buffer[..length].to_vec()),
            Err(error) if error.kind() == ErrorKind::WouldBlock => break,
            Err(error) => panic!("{error}"),
        }
    }
    datagrams
}

impl Client {
    pub fn connect(app: &mut App, address: SocketAddr, order: ByteOrder) -> Self {
        Self::connect_indexed(app, address, order, 0)
    }

    /// Peer `index` logs in with its own name and unique net ID; index 0 is
    /// the identity `connect` uses.
    pub fn connect_indexed(
        app: &mut App,
        address: SocketAddr,
        order: ByteOrder,
        index: u8,
    ) -> Self {
        Self::connect_version(app, address, order, index, 1447163286)
    }

    /// Select the Hello network checksum for tests of a pinned release.
    pub fn connect_version(
        app: &mut App,
        address: SocketAddr,
        order: ByteOrder,
        index: u8,
        version: u32,
    ) -> Self {
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        socket.connect(address).unwrap();
        socket.set_nonblocking(true).unwrap();
        socket.send(&initial_request()).unwrap();
        app.update();
        let challenge = drain(&socket).pop().expect("challenge");
        let cookie = decode(&challenge).unwrap().cookie;
        socket.send(&challenge).unwrap();
        app.update();
        assert_eq!(
            decode(&drain(&socket).pop().expect("handshake ack"))
                .unwrap()
                .cookie,
            cookie
        );
        let (incoming, outgoing) = client_cookie_seeds(&cookie);
        let sequencer = PacketSequencer::new(incoming, outgoing, 64).unwrap();
        let control = sequencer.new_channel();
        let mut client = Self {
            socket,
            sequencer,
            control,
            packet: app.world().resource::<PeerLab>().config().packet.packet,
            order,
        };
        for message in [
            ControlMessage::Hello {
                first_field: u8::from(matches!(order, ByteOrder::Little)),
                version,
                token: ArchiveText::text("").unwrap(),
            },
            ControlMessage::Login {
                response: ArchiveText::text("0").unwrap(),
                url: ArchiveText::text(&if index == 0 {
                    "?Name=A".to_owned()
                } else {
                    format!("?Name=A{index}")
                })
                .unwrap(),
                unique_net_id: UniqueNetId {
                    kind: NetIdType::Named(ArchiveText::text("STEAM").unwrap()),
                    value: NetIdValue::from_hex(&format!("0123456789ABCDEF76{:02X}", 0x54 ^ index))
                        .unwrap(),
                },
                platform: ArchiveText::text("").unwrap(),
            },
        ] {
            client.control(app, message);
            let replies = drain(&client.socket);
            assert!(!replies.is_empty());
            client.accept(&replies);
        }
        client
    }

    pub fn config(&self) -> PacketConfig {
        PacketConfig {
            receiver: Receiver::Client,
            ..self.packet
        }
    }

    pub fn send(&mut self, bunches: Vec<Bunch>) {
        let packet = Packet {
            magic: 0,
            header: self.sequencer.header().unwrap(),
            info: None,
            bunches,
        };
        let bytes = write_packet(&packet, &self.packet).unwrap();
        self.sequencer.commit().unwrap();
        self.socket.send(&bytes).unwrap();
    }

    pub fn control(&mut self, app: &mut App, message: ControlMessage) {
        let sequence = self
            .control
            .send_reliable(self.sequencer.out_packet_id(), message.clone())
            .unwrap();
        let mut payload = BitWriter::new();
        payload.write_bytes(
            &encode_control_message(
                &message,
                &ControlCodec {
                    order: self.order,
                    ..Default::default()
                },
            )
            .unwrap(),
        );
        self.send(vec![Bunch {
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
                reliable_sequence_raw: Some(wire_reliable_sequence(sequence)),
                partial_initial: false,
                partial_final: false,
                channel_name: Some(Name::Hardcoded(255)),
            },
            payload: Payload::from_writer(payload),
        }]);
        app.update();
    }

    pub fn join(&mut self, app: &mut App) -> Vec<Vec<u8>> {
        self.control(app, ControlMessage::Join);
        let mut opened = drain(&self.socket);
        for _ in 0..4 {
            app.update();
            opened.extend(drain(&self.socket));
        }
        assert!(
            !app.world()
                .resource::<PeerLab>()
                .joined_connections()
                .is_empty()
        );
        opened
    }

    pub fn accept(&mut self, datagrams: &[Vec<u8>]) {
        for datagram in datagrams {
            let packet = read_packet(datagram, &self.config()).unwrap();
            if let Some(received) = self.sequencer.receive(&packet.header).unwrap() {
                self.sequencer.ack_seq(received.sequence, true).unwrap();
            }
        }
    }

    pub fn step(&mut self, app: &mut App) -> Vec<Vec<u8>> {
        self.send(Vec::new());
        app.update();
        let mut bytes = drain(&self.socket);
        for _ in 0..4 {
            app.update();
            bytes.extend(drain(&self.socket));
        }
        bytes
    }

    pub fn settle(&mut self, app: &mut App, datagrams: &[Vec<u8>]) -> Vec<Vec<u8>> {
        self.accept(datagrams);
        let bytes = self.step(app);
        self.accept(&bytes);
        bytes
    }

    pub fn disconnect(&mut self, app: &mut App) {
        let sequence = self.control.out_reliable() + 1;
        self.send(vec![Bunch {
            header: BunchHeader {
                control: true,
                open: false,
                close: true,
                close_reason: Some(0),
                paused: false,
                reliable: true,
                channel_index: 0,
                has_package_map_exports: false,
                has_must_be_mapped_guids: false,
                partial: false,
                reliable_sequence_raw: Some(wire_reliable_sequence(sequence)),
                partial_initial: false,
                partial_final: false,
                channel_name: Some(Name::Hardcoded(255)),
            },
            payload: Payload::from_writer(BitWriter::new()),
        }]);
        for _ in 0..4 {
            app.update();
        }
        drain(&self.socket);
    }
}

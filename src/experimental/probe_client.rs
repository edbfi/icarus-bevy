//! EXPERIMENTAL synchronous probe client: the stateless handshake, then one
//! reliable Hello opening the control channel, built from this repository's
//! own codec and client sequencing model (F-519, F-521 to F-527, F-530,
//! F-536 to F-539, F-563).
//!
//! The data-packet model is not wire-validated against an original server, so
//! a reply that fails to decode is kept as data rather than an error: against
//! the original it is the observation. Every received datagram is kept raw.
//! Run against the original only through the approved offline launcher.

use std::{
    io::ErrorKind,
    net::UdpSocket,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail, ensure};

use super::{
    control_messages::{
        ArchiveText, ControlCodec, ControlMessage, Decoded, encode_control_message,
        read_control_messages,
    },
    handshake::{self, LegacyHandshake},
    packet::{
        BitWriter, Bunch, BunchHeader, Name, Packet, PacketConfig, Payload, Receiver, read_packet,
        write_packet,
    },
    packet_lab::client_cookie_seeds,
    sequencing::{PacketSequencer, ReliableChannel, ReliableReceive, wire_reliable_sequence},
};

/// The control channel's hard-coded name index (F-525).
const CONTROL_CHANNEL_NAME: u32 = 255;
/// Sent-record ring capacity of the probe's sequencer.
const SENT_CAPACITY: usize = 64;

/// How the probe frames packets and how long it listens.
#[derive(Clone, Debug)]
pub struct ProbeConfig {
    /// Framing of outgoing packets; `receiver` must be [`Receiver::Server`].
    /// Replies are read with the same framing for [`Receiver::Client`].
    pub packet: PacketConfig,
    pub codec: ControlCodec,
    /// Per-receive timeout of the stateless handshake.
    pub handshake_timeout: Duration,
    /// Initial requests sent before giving up on a challenge.
    pub handshake_attempts: u32,
    /// How long replies to the Hello are collected.
    pub reply_window: Duration,
    /// Reply datagrams kept before listening stops early.
    pub max_datagrams: usize,
}

/// What one received datagram decoded to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReplyDecode {
    /// Control messages released in reliable order by this datagram, plus a
    /// note per bunch that was not a decodable reliable control bunch.
    Decoded {
        messages: Vec<ControlMessage>,
        notes: Vec<String>,
    },
    /// The datagram did not parse under the probe's packet model.
    Undecoded(String),
}

/// One reply datagram, raw and decoded.
#[derive(Clone, Debug)]
pub struct ReplyDatagram {
    pub bytes: Vec<u8>,
    pub decode: ReplyDecode,
}

/// The result of one Hello.
#[derive(Clone, Debug)]
pub struct HelloReport {
    /// The Hello datagram as sent.
    pub hello_datagram: Vec<u8>,
    pub hello_packet_id: u32,
    /// Whether any reply acknowledged the Hello's packet.
    pub hello_acked: bool,
    pub replies: Vec<ReplyDatagram>,
}

impl HelloReport {
    /// Every decoded control message, in arrival order.
    pub fn messages(&self) -> impl Iterator<Item = &ControlMessage> {
        self.replies.iter().flat_map(|reply| match &reply.decode {
            ReplyDecode::Decoded { messages, .. } => messages.as_slice(),
            ReplyDecode::Undecoded(_) => &[],
        })
    }
}

/// The handshake's challenge and acknowledgement, as received.
#[derive(Clone, Debug)]
pub struct HandshakeReport {
    pub challenge: LegacyHandshake,
    pub challenge_datagram: Vec<u8>,
    pub ack_datagram: Vec<u8>,
}

/// A connected probe: the socket, cookie-seeded sequencing and the control
/// channel's reliable layer.
pub struct ProbeClient {
    socket: UdpSocket,
    config: ProbeConfig,
    sequencer: PacketSequencer,
    control: ReliableChannel<ControlMessage>,
    handshake: HandshakeReport,
}

impl ProbeClient {
    /// Run the stateless handshake on a socket already connected to the
    /// server: request, echo the challenge, check the acknowledgement.
    pub fn connect(socket: UdpSocket, config: ProbeConfig) -> Result<Self> {
        ensure!(
            config.packet.receiver == Receiver::Server,
            "outgoing framing must address the server"
        );
        ensure!(config.handshake_attempts > 0, "at least one attempt");
        socket.set_read_timeout(Some(config.handshake_timeout))?;
        let mut buffer = [0_u8; 65_536];
        let mut challenge = None;
        for _ in 0..config.handshake_attempts {
            socket.send(&handshake::initial_request())?;
            if let Some(size) = receive(&socket, &mut buffer)? {
                challenge = Some(buffer[..size].to_vec());
                break;
            }
        }
        let challenge_datagram = challenge.context("no challenge after the bounded requests")?;
        let challenge = handshake::decode(&challenge_datagram)
            .context("challenge framing differs from the model")?;
        ensure!(
            challenge.timestamp > 0.0 && !challenge.restart,
            "reply is not a normal challenge"
        );
        socket.send(&handshake::encode(&challenge)?)?;
        let size = receive(&socket, &mut buffer)?.context("no stateless acknowledgement")?;
        let ack_datagram = buffer[..size].to_vec();
        let ack = handshake::decode(&ack_datagram)?;
        ensure!(
            ack.timestamp == -1.0
                && !ack.restart
                && ack.secret_id
                && ack.cookie == challenge.cookie,
            "reply differs from the stateless acknowledgement model"
        );
        let (incoming, outgoing) = client_cookie_seeds(&challenge.cookie);
        let sequencer = PacketSequencer::new(incoming, outgoing, SENT_CAPACITY)?;
        let control = sequencer.new_channel();
        Ok(Self {
            socket,
            config,
            sequencer,
            control,
            handshake: HandshakeReport {
                challenge,
                challenge_datagram,
                ack_datagram,
            },
        })
    }

    pub fn handshake(&self) -> &HandshakeReport {
        &self.handshake
    }

    /// Send one reliable Hello opening the control channel, then collect
    /// replies for the reply window. An empty `token` asks for a challenge
    /// (F-563).
    pub fn hello(&mut self, version: u32, token: &str) -> Result<HelloReport> {
        let message = ControlMessage::Hello {
            first_field: 1,
            version,
            token: ArchiveText::text(token)?,
        };
        let hello_packet_id = self.sequencer.out_packet_id();
        let sequence = self
            .control
            .send_reliable(hello_packet_id, message.clone())?;
        let mut payload = BitWriter::new();
        payload.write_bytes(&encode_control_message(&message, &self.config.codec)?);
        let bunch = Bunch {
            header: BunchHeader {
                control: true,
                open: true,
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
                channel_name: Some(Name::Hardcoded(CONTROL_CHANNEL_NAME)),
            },
            payload: Payload::from_writer(payload),
        };
        let packet = Packet {
            magic: 0,
            header: self.sequencer.header()?,
            info: None,
            bunches: vec![bunch],
        };
        let hello_datagram = write_packet(&packet, &self.config.packet)?;
        let committed = self.sequencer.commit()?;
        ensure!(committed == hello_packet_id, "packet id moved during send");
        self.socket.send(&hello_datagram)?;

        let mut replies = Vec::new();
        let mut hello_acked = false;
        let deadline = Instant::now() + self.config.reply_window;
        let mut buffer = [0_u8; 65_536];
        while replies.len() < self.config.max_datagrams {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            self.socket.set_read_timeout(Some(left))?;
            let Some(size) = receive(&self.socket, &mut buffer)? else {
                break;
            };
            let bytes = buffer[..size].to_vec();
            let decode = match self.decode_reply(&bytes) {
                Ok((acked, decode)) => {
                    hello_acked |= acked.contains(&hello_packet_id);
                    decode
                }
                Err(error) => ReplyDecode::Undecoded(format!("{error:#}")),
            };
            replies.push(ReplyDatagram { bytes, decode });
        }
        Ok(HelloReport {
            hello_datagram,
            hello_packet_id,
            hello_acked,
            replies,
        })
    }

    /// Parse a reply as the client: the header through the sequencing model,
    /// then every reliable bunch on channel 0 as control messages. Returns the
    /// delivered packet IDs and the decode.
    fn decode_reply(&mut self, bytes: &[u8]) -> Result<(Vec<u32>, ReplyDecode)> {
        let config = PacketConfig {
            receiver: Receiver::Client,
            ..self.config.packet
        };
        let packet = read_packet(bytes, &config)?;
        let received = self
            .sequencer
            .receive(&packet.header)?
            .context("stale or duplicate packet sequence")?;
        let acked = received
            .notifications
            .iter()
            .filter(|notification| notification.delivered)
            .map(|notification| notification.packet_id)
            .collect();
        let mut messages = Vec::new();
        let mut notes = Vec::new();
        for (index, bunch) in packet.bunches.iter().enumerate() {
            let header = &bunch.header;
            let Some(raw) = header.reliable_sequence_raw.filter(|_| header.reliable) else {
                notes.push(format!(
                    "bunch {index}: unreliable on channel {}",
                    header.channel_index
                ));
                continue;
            };
            if header.channel_index != 0 {
                notes.push(format!("bunch {index}: channel {}", header.channel_index));
                continue;
            }
            let decoded =
                match read_control_messages(&mut bunch.payload.reader(), &self.config.codec) {
                    Ok(decoded) => decoded,
                    Err(error) => {
                        notes.push(format!("bunch {index}: {error:#}"));
                        continue;
                    }
                };
            for item in decoded {
                match item {
                    Decoded::Message(message) => {
                        if let ReliableReceive::Delivered(batch) =
                            self.control.receive_reliable(raw, message)?
                        {
                            messages.extend(batch);
                        }
                    }
                    Decoded::Unknown { id } => {
                        notes.push(format!("bunch {index}: unknown id {id}"))
                    }
                }
            }
        }
        self.sequencer.ack_seq(received.sequence, true)?;
        Ok((acked, ReplyDecode::Decoded { messages, notes }))
    }
}

/// One datagram, or `None` on timeout.
fn receive(socket: &UdpSocket, buffer: &mut [u8]) -> Result<Option<usize>> {
    match socket.recv(buffer) {
        Ok(size) => Ok(Some(size)),
        Err(error) if matches!(error.kind(), ErrorKind::TimedOut | ErrorKind::WouldBlock) => {
            Ok(None)
        }
        Err(error) => bail!(error),
    }
}

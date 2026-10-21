//! EXPERIMENTAL disconnected per-peer data-packet state for the loopback lab.
//!
//! The step after the stateless handshake: one peer's packet sequencing, acks
//! and ordered control-channel delivery, composed from the packet codec
//! (`packet`, F-521..F-527), the client-side sequencing model (`sequencing`,
//! F-535..F-539) and the control vocabulary (`control_messages`, F-529,
//! F-530, F-712). Pure state: no sockets, Bevy, timers, reply policy, version
//! selection, identity check, admission or gameplay.
//!
//! Outgoing: [`LabPeer::reliable_control_packet`] writes one caller-chosen
//! control message as a new reliable bunch on the control channel,
//! [`LabPeer::reliable_actor_open`] opens an actor channel with a caller-built
//! open bunch and its exports split as `UChannel::SendBunch` does (F-709), and
//! [`LabPeer::ack_packet`] writes the ack state alone; all also carry any
//! reliable bunches awaiting resend (actor-channel parts unchanged). The client's ack and nak handling is
//! mirrored (F-539, INFERRED for this side): an acked packet releases its
//! out-bunches in order and a naked one queues its unacked bunches for resend
//! in the next written packet. Resend is driven only by naks; there is no
//! timer. The bunch is assembled from codec fields (F-522, F-712): not open,
//! reliable (`FControlChannelOutBunch` sets `bReliable = 1`, `0x1437a83c0`),
//! the configured control channel index, the hard-coded control name 255 and
//! the 10-bit wire reliable sequence; its payload is the message encoded with
//! the connection's active codec byte order (`Connection->bNeedsByteSwapping`
//! at `+0x158`, seeded from `config.codec.order` and updated when the first
//! `Hello` bunch is inspected on `UControlChannel`, `0x1437c4940`,
//! `0x1437a8b60`). The original server's packet info and send cadence were not
//! observed.
//!
//! Every observation behind it is of the client build. The server side here
//! is an INFERRED mirror of the client: the client seeds its incoming and
//! outgoing sequences from the handshake cookie's first and second
//! little-endian 16-bit words masked to 14 bits (F-536), so this peer seeds
//! its incoming sequence from the second word and its outgoing from the first
//! (see [`server_cookie_seeds`]). The sequencer is the client model reused for
//! the other side; the original server's own behaviour was not observed. This
//! does not demonstrate compatibility with any client or server.
//!
//! Control bunches are identified as `control_receive` does: an open bunch
//! named with hard-coded index 255, or one on the configured control channel
//! index (default 0); both identifications are inferred (F-522, F-525).
//! Summaries and `Debug` output carry kinds, counts and lengths only: never
//! payload bytes, token or string contents, numeric message fields or cookies.
use std::collections::{BTreeMap, VecDeque};

use anyhow::{Context, Result, bail, ensure};
use serde::Serialize;

use super::archive::ByteOrder;
use super::control::sender_byte_order;
use super::control_messages::{
    ControlCodec, ControlMessage, Decoded, MessageSummary, read_control_message,
    write_control_message,
};
use super::control_receive::{INFERRED_CONTROL_CHANNEL_INDEX, INFERRED_CONTROL_NAME_INDEX};
use super::packet::{
    BitReader, BitWriter, Bunch, BunchHeader, BunchLimits, Name, Packet, PacketConfig,
    PacketHeader, PacketInfo, Payload, Receiver, read_bunch, read_data_prefix, read_packet_header,
    split_termination, write_packet,
};
use super::sequencing::{
    MAX_QUEUED_RELIABLE, PacketSequencer, ReliableChannel, ReliableReceive, SEQUENCE_MASK,
    max_single_bunch_size_bits, split_outgoing_bunch, wire_reliable_sequence,
};

/// Handshake cookie length of the short framing (`handshake`).
pub const COOKIE_BYTES: usize = 20;
/// Lab choice: the sent-record ring capacity; the client's is not established.
pub const DEFAULT_SENT_CAPACITY: usize = 256;
/// Lab choice: bunches accepted per datagram; one more fails the datagram.
/// Outgoing datagrams carry at most this many bunches as well.
pub const DEFAULT_MAX_BUNCHES_PER_PACKET: usize = 32;
/// Lab choice: unacknowledged outgoing reliable control bunches one peer may
/// hold; sending another fails without a state change. Matches
/// `UControlChannel::SendBunch` (`0x1437ca030`) `NumOutRec < 255` (`0xff`)
/// and the incoming queue bound (F-537, F-712).
pub const MAX_PENDING_RELIABLE: usize = MAX_QUEUED_RELIABLE;
/// `MaxPacketHandlerBits` behind `MAX_SINGLE_BUNCH_SIZE_BITS = 7,625` for
/// `MaxPacket = 1024` (F-709); the lab writes no handler bits itself.
pub const PACKET_HANDLER_BITS: u32 = 2;
/// `NetMaxConstructedPartialBunchSizeBytes` default (`64` KiB, F-709).
pub const MAX_CONSTRUCTED_BUNCH_BYTES: usize = 65_536;

/// Resends carried by one assembled datagram, with their wire bunches.
type Resends = Vec<(Resend, Bunch)>;

/// A naked reliable bunch awaiting resend.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Resend {
    Control(u32),
    /// An actor channel's index and reliable sequence.
    Actor(u32, u32),
}

/// The single place that fixes the cookie word order (F-536): the client's
/// InitSequence takes word 0 as incoming and word 1 as outgoing.
const CLIENT_INCOMING_WORD: usize = 0;
const CLIENT_OUTGOING_WORD: usize = 1;

fn cookie_word(cookie: &[u8; COOKIE_BYTES], word: usize) -> u16 {
    u16::from_le_bytes([cookie[2 * word], cookie[2 * word + 1]]) & SEQUENCE_MASK
}

/// The client's `(incoming, outgoing)` seeds from a cookie (F-536, observed).
pub fn client_cookie_seeds(cookie: &[u8; COOKIE_BYTES]) -> (u16, u16) {
    (
        cookie_word(cookie, CLIENT_INCOMING_WORD),
        cookie_word(cookie, CLIENT_OUTGOING_WORD),
    )
}

/// This peer's `(incoming, outgoing)` seeds: the client's swapped, so the
/// server expects the client's outgoing sequence. The mirror is INFERRED; the
/// original server's seeding was not observed.
pub fn server_cookie_seeds(cookie: &[u8; COOKIE_BYTES]) -> (u16, u16) {
    let (client_incoming, client_outgoing) = client_cookie_seeds(cookie);
    (client_outgoing, client_incoming)
}

/// Caller-supplied parameters; the original negotiated values are unknown.
#[derive(Clone, Copy, Debug)]
pub struct PacketLabConfig {
    /// Framing of incoming client packets; `receiver` must be
    /// [`Receiver::Server`]. Outgoing packets use the same framing written for
    /// [`Receiver::Client`].
    pub packet: PacketConfig,
    /// Initial byte order and string cap for control messages other than Hello.
    /// When the first control bunch starts with `Hello`, its first field
    /// updates the connection's byte order (`Connection + 0x158`, F-712).
    pub codec: ControlCodec,
    /// Bunches on this channel index are control bunches (inferred).
    pub control_channel_index: u32,
    /// Sent-record ring capacity; [`LabPeer::ack_packet`] fails once full.
    pub sent_capacity: usize,
    /// Datagrams with more bunches fail before the excess bunch is read.
    pub max_bunches_per_packet: usize,
}

impl PacketLabConfig {
    /// No magic header, server receiver, little-endian codec, the inferred
    /// control channel 0 and the lab default capacities.
    pub fn new(limits: BunchLimits) -> Self {
        Self {
            packet: PacketConfig {
                magic_bits: 0,
                receiver: Receiver::Server,
                limits,
            },
            codec: ControlCodec::default(),
            control_channel_index: INFERRED_CONTROL_CHANNEL_INDEX,
            sent_capacity: DEFAULT_SENT_CAPACITY,
            max_bunches_per_packet: DEFAULT_MAX_BUNCHES_PER_PACKET,
        }
    }

    /// Check the configuration that [`LabPeer::from_cookie`] enforces.
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.packet.receiver == Receiver::Server,
            "the lab peer reads client packets as the server"
        );
        ensure!(self.packet.magic_bits <= 32, "magic header exceeds 32 bits");
        ensure!(
            self.max_bunches_per_packet >= 1,
            "at least one bunch per packet must be allowed"
        );
        ensure!(
            self.sent_capacity >= 1,
            "sent-record capacity must be at least one"
        );
        ensure!(
            self.control_channel_index < self.packet.limits.max_channels,
            "control channel index exceeds the channel limit"
        );
        Ok(())
    }

    fn outgoing(&self) -> PacketConfig {
        PacketConfig {
            receiver: Receiver::Client,
            ..self.packet
        }
    }
}

/// Whether the incoming packet passed the sequence gate.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PacketDisposition {
    /// Newer than the last accepted packet; processed and acknowledged.
    Accepted {
        /// Full incoming packet ID (inferred advance, see `sequencing`).
        packet_id: u32,
        /// Incoming sequences skipped since the previous accepted packet.
        skipped: u16,
    },
    /// Stale or duplicate: nothing was processed and the state is unchanged.
    Rejected,
}

/// One delivered control message: kinds and lengths only.
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DeliveredSummary {
    Message(MessageSummary),
    /// First ID without a known layout; the rest of its bunch was not read.
    Unknown {
        id: u8,
    },
}

impl DeliveredSummary {
    fn of(decoded: &Decoded) -> Self {
        match decoded {
            Decoded::Message(message) => Self::Message(message.summary()),
            Decoded::Unknown { id } => Self::Unknown { id: *id },
        }
    }

    /// Message ID of either form.
    pub fn id(&self) -> u8 {
        match self {
            Self::Message(summary) => summary.id,
            Self::Unknown { id } => *id,
        }
    }
}

/// Reportable result of one datagram; no contents.
#[derive(Clone, Debug, Serialize)]
pub struct ReceiveSummary {
    /// The packet's 14-bit sequence.
    pub sequence: u16,
    pub disposition: PacketDisposition,
    /// Outgoing packet IDs the peer acknowledged, oldest first.
    pub acked: Vec<u32>,
    /// Outgoing packet IDs the peer reported lost, oldest first.
    pub naked: Vec<u32>,
    /// Outgoing reliable control bunches released by acks in this packet.
    pub released_reliable: usize,
    /// Outgoing reliable control bunches newly queued for resend by naks.
    pub resend_queued: usize,
    pub bunch_count: usize,
    pub control_bunches: usize,
    /// Non-control bunches: not interpreted, yet the packet is acknowledged
    /// as delivered (a lab choice).
    pub ignored_bunches: usize,
    /// Reliable control bunches already delivered or already queued; dropped.
    pub duplicate_reliable: usize,
    /// Control messages released by this packet, in delivery order.
    pub delivered: Vec<DeliveredSummary>,
    /// Out-of-order reliable control bunches now held for ordered delivery.
    pub queued_reliable: usize,
    /// A control bunch carried the close flag; the lab does not act on it.
    pub control_close_seen: bool,
    pub negotiation_validated: bool,
    pub identity_verified: bool,
    pub game_connection_established: bool,
}

/// Owned result of one datagram. `Debug` shows only the summary because the
/// decoded messages retain strings and field values.
pub struct PeerReceive {
    summary: ReceiveSummary,
    messages: Vec<Decoded>,
}

impl std::fmt::Debug for PeerReceive {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PeerReceive")
            .field("summary", &self.summary)
            .finish_non_exhaustive()
    }
}

impl PeerReceive {
    pub fn summary(&self) -> &ReceiveSummary {
        &self.summary
    }

    /// Delivered messages in delivery order, matching `summary().delivered`.
    pub fn messages(&self) -> &[Decoded] {
        &self.messages
    }

    /// The delivered messages, consuming the result.
    pub fn into_messages(self) -> Vec<Decoded> {
        self.messages
    }
}

/// A control bunch decoded before any state changes.
struct StagedControl {
    reliable_raw: Option<u32>,
    close: bool,
    messages: Vec<Decoded>,
}

/// One peer's packet state, seeded from its handshake cookie.
///
/// Errors before the sequence gate (malformed or unsupported datagrams) leave
/// the state unchanged. An error after it leaves the state unspecified, as the
/// client closes the connection then; the peer is marked failed and rejects
/// every later call.
pub struct LabPeer {
    config: PacketLabConfig,
    sequencer: PacketSequencer,
    /// Incoming payloads are delivered message batches; an outgoing payload is
    /// the one sent message, retained for resend until acked.
    control: ReliableChannel<Vec<Decoded>>,
    /// Actor channels this peer opened, by channel index: their reliable
    /// wire bunches, retained unchanged for resend until acked.
    actors: BTreeMap<u32, ReliableChannel<Bunch>>,
    /// Naked out-bunches awaiting resend, oldest first.
    resend: VecDeque<Resend>,
    /// Mirrors `UControlChannel::bNeedsHeaderInspection` (`+0x68`, F-712):
    /// cleared when the first delivered control bunch starting with `Hello`
    /// sets `Connection->bNeedsByteSwapping` (`+0x158`).
    header_inspection_pending: bool,
    failed: bool,
}

impl std::fmt::Debug for LabPeer {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let notify = self.sequencer.notify();
        formatter
            .debug_struct("LabPeer")
            .field("in_seq", &notify.in_seq())
            .field("in_ack_seq", &notify.in_ack_seq())
            .field("out_seq", &notify.out_seq())
            .field("out_ack_seq", &notify.out_ack_seq())
            .field("control_in_reliable", &self.control.in_reliable())
            .field("queued_reliable", &self.control.queued_len())
            .field("control_out_reliable", &self.control.out_reliable())
            .field("pending_reliable", &self.control.out_bunches().len())
            .field("actor_channels", &self.actors.len())
            .field("pending_resends", &self.resend.len())
            .field("failed", &self.failed)
            .finish_non_exhaustive()
    }
}

impl LabPeer {
    /// Seed with [`server_cookie_seeds`] (inferred mirror of F-536) and open
    /// the control channel's reliable state at the initial reliable values.
    /// The cookie itself is not retained.
    pub fn from_cookie(cookie: &[u8; COOKIE_BYTES], config: PacketLabConfig) -> Result<Self> {
        config.validate()?;
        let (incoming, outgoing) = server_cookie_seeds(cookie);
        let sequencer = PacketSequencer::new(incoming, outgoing, config.sent_capacity)?;
        let control = sequencer.new_channel();
        Ok(Self {
            config,
            sequencer,
            control,
            actors: BTreeMap::new(),
            resend: VecDeque::new(),
            header_inspection_pending: true,
            failed: false,
        })
    }

    pub fn config(&self) -> &PacketLabConfig {
        &self.config
    }

    /// Active connection byte order (`Connection->bNeedsByteSwapping` at `+0x158`).
    pub fn connection_byte_order(&self) -> ByteOrder {
        self.config.codec.order
    }

    /// Whether `UControlChannel::bNeedsHeaderInspection` (`+0x68`) is still set.
    pub fn header_inspection_pending(&self) -> bool {
        self.header_inspection_pending
    }

    pub fn sequencer(&self) -> &PacketSequencer {
        &self.sequencer
    }

    /// Last delivered reliable sequence of the control channel (unwrapped).
    pub fn control_in_reliable(&self) -> u32 {
        self.control.in_reliable()
    }

    pub fn queued_reliable(&self) -> usize {
        self.control.queued_len()
    }

    /// Last assigned outgoing reliable sequence of the control channel
    /// (unwrapped).
    pub fn control_out_reliable(&self) -> u32 {
        self.control.out_reliable()
    }

    /// Outgoing reliable control bunches not yet released by an ack.
    pub fn pending_reliable(&self) -> usize {
        self.control.out_bunches().len()
    }

    /// Naked outgoing reliable bunches the next written datagram will resend.
    pub fn pending_resends(&self) -> usize {
        self.resend.len()
    }

    /// Unacknowledged reliable bunches of the actor channel `index`, or
    /// `None` when this peer has not opened it.
    pub fn pending_actor_reliable(&self, index: u32) -> Option<usize> {
        self.actors
            .get(&index)
            .map(|channel| channel.out_bunches().len())
    }

    pub fn is_failed(&self) -> bool {
        self.failed
    }

    fn ensure_usable(&self) -> Result<()> {
        ensure!(
            !self.failed,
            "lab peer failed after an earlier error; the client would close the connection"
        );
        Ok(())
    }

    fn fail_on_error<T>(&mut self, result: Result<T>) -> Result<T> {
        if result.is_err() {
            self.failed = true;
        }
        result
    }

    fn is_control(&self, bunch: &Bunch) -> bool {
        let header = &bunch.header;
        (header.open && header.channel_name == Some(Name::Hardcoded(INFERRED_CONTROL_NAME_INDEX)))
            || header.channel_index == self.config.control_channel_index
    }

    /// Bounded parse: the datagram length, then at most the configured bunch
    /// count. A handshake datagram fails with `packet::HandshakePacket`.
    fn read_bounded(&self, datagram: &[u8]) -> Result<(PacketHeader, Vec<Bunch>)> {
        let limits = &self.config.packet.limits;
        ensure!(
            datagram.len() as u64 <= u64::from(limits.max_packet_bytes),
            "datagram exceeds the maximum packet size"
        );
        let (bytes, bit_len) = split_termination(datagram)?;
        let mut reader = BitReader::new(bytes, bit_len)?;
        read_data_prefix(&mut reader, self.config.packet.magic_bits)?;
        reader.split_connection_termination()?;
        let (header, _info) = read_packet_header(&mut reader, self.config.packet.receiver)?;
        let mut bunches = Vec::new();
        while !reader.at_end() {
            let index = bunches.len();
            ensure!(
                index < self.config.max_bunches_per_packet,
                "datagram exceeds {} bunches",
                self.config.max_bunches_per_packet
            );
            bunches
                .push(read_bunch(&mut reader, limits).with_context(|| format!("bunch {index}"))?);
        }
        Ok((header, bunches))
    }

    /// Decode every control bunch without touching state. Unsupported forms
    /// fail closed: partial bunches (no assembly) and bunches carrying package
    /// map exports or must-be-mapped GUIDs (their payload prefix is not read).
    fn stage(&self, bunches: &[Bunch]) -> Result<Vec<StagedControl>> {
        let mut staged = Vec::new();
        let mut codec = self.config.codec;
        let mut inspect_pending = self.header_inspection_pending;
        for (index, bunch) in bunches.iter().enumerate() {
            if !self.is_control(bunch) {
                continue;
            }
            let header = &bunch.header;
            ensure!(
                !header.partial,
                "control bunch {index} is partial; assembly is not implemented"
            );
            ensure!(
                !header.has_package_map_exports && !header.has_must_be_mapped_guids,
                "control bunch {index} carries exports or must-be-mapped GUIDs; unsupported"
            );
            let mut reader = bunch.payload.reader();
            let mut messages = Vec::new();
            while !reader.at_end() {
                let msg_index = messages.len();
                let next = read_control_message(&mut reader, &codec).with_context(|| {
                    format!("control bunch {index}: control message {msg_index}")
                })?;
                if inspect_pending
                    && msg_index == 0
                    && let Decoded::Message(ControlMessage::Hello { first_field, .. }) = &next
                {
                    codec.order = sender_byte_order(*first_field);
                    inspect_pending = false;
                }
                let stop = matches!(next, Decoded::Unknown { .. });
                messages.push(next);
                if stop {
                    break;
                }
            }
            staged.push(StagedControl {
                reliable_raw: header.reliable_sequence_raw,
                close: header.close,
                messages,
            });
        }
        Ok(staged)
    }

    /// Process one terminated data datagram sent by the client.
    ///
    /// Order: bounded parse and control decode (pure), the sequence gate and
    /// notify update, ordered control delivery through the channel's reliable
    /// queue, then `ack_seq(sequence, delivered = true)`. A stale or duplicate
    /// packet is reported as [`PacketDisposition::Rejected`], not an error.
    pub fn receive(&mut self, datagram: &[u8]) -> Result<PeerReceive> {
        self.ensure_usable()?;
        let (header, bunches) = self.read_bounded(datagram)?;
        let staged = self.stage(&bunches)?;
        let control_bunches = staged.len();
        let mut summary = ReceiveSummary {
            sequence: header.sequence,
            disposition: PacketDisposition::Rejected,
            acked: Vec::new(),
            naked: Vec::new(),
            released_reliable: 0,
            resend_queued: 0,
            bunch_count: bunches.len(),
            control_bunches,
            ignored_bunches: bunches.len() - control_bunches,
            duplicate_reliable: 0,
            delivered: Vec::new(),
            queued_reliable: self.control.queued_len(),
            control_close_seen: false,
            negotiation_validated: false,
            identity_verified: false,
            game_connection_established: false,
        };
        let received = self.sequencer.receive(&header);
        let Some(received) = self.fail_on_error(received)? else {
            return Ok(PeerReceive {
                summary,
                messages: Vec::new(),
            });
        };
        summary.disposition = PacketDisposition::Accepted {
            packet_id: received.packet_id,
            skipped: received.skipped,
        };
        for notification in &received.notifications {
            let packet_id = notification.packet_id;
            if notification.delivered {
                summary.acked.push(packet_id);
                summary.released_reliable += self.control.acked(packet_id).len();
                for channel in self.actors.values_mut() {
                    summary.released_reliable += channel.acked(packet_id).len();
                }
            } else {
                summary.naked.push(packet_id);
                let lost = self
                    .control
                    .lost(packet_id)
                    .into_iter()
                    .map(Resend::Control)
                    .chain(self.actors.iter().flat_map(|(index, channel)| {
                        channel
                            .lost(packet_id)
                            .into_iter()
                            .map(|sequence| Resend::Actor(*index, sequence))
                    }))
                    .collect::<Vec<_>>();
                for entry in lost {
                    if !self.resend.contains(&entry) {
                        self.resend.push_back(entry);
                        summary.resend_queued += 1;
                    }
                }
            }
        }
        let delivered = self.deliver(staged, &mut summary);
        let messages = self.fail_on_error(delivered)?;
        let acked = self.sequencer.ack_seq(received.sequence, true);
        self.fail_on_error(acked)?;
        summary.queued_reliable = self.control.queued_len();
        summary.delivered = messages.iter().map(DeliveredSummary::of).collect();
        Ok(PeerReceive { summary, messages })
    }

    fn deliver(
        &mut self,
        staged: Vec<StagedControl>,
        summary: &mut ReceiveSummary,
    ) -> Result<Vec<Decoded>> {
        let mut messages = Vec::new();
        for bunch in staged {
            summary.control_close_seen |= bunch.close;
            let Some(raw) = bunch.reliable_raw else {
                // Unreliable control bunches are processed on arrival (F-537).
                self.inspect_delivered_batch(&bunch.messages);
                messages.extend(bunch.messages);
                continue;
            };
            match self.control.receive_reliable(raw, bunch.messages)? {
                ReliableReceive::Delivered(batches) => {
                    for batch in batches {
                        self.inspect_delivered_batch(&batch);
                        messages.extend(batch);
                    }
                }
                ReliableReceive::Queued => {}
                ReliableReceive::Duplicate => summary.duplicate_reliable += 1,
            }
        }
        Ok(messages)
    }

    fn inspect_delivered_batch(&mut self, batch: &[Decoded]) {
        if self.header_inspection_pending
            && let Some(Decoded::Message(ControlMessage::Hello { first_field, .. })) = batch.first()
        {
            self.config.codec.order = sender_byte_order(*first_field);
            self.header_inspection_pending = false;
        }
    }

    /// A datagram for the client carrying the ack state: the sequencer's
    /// header (outgoing sequence, acked incoming sequence and history words),
    /// lab-chosen packet info (jitter 0, no frame time) and magic 0, plus any
    /// naked reliable bunches awaiting resend (none unless the lab sent
    /// reliable control messages); then the packet is committed to the
    /// sent-record ring. The original server's packet info and send cadence
    /// were not observed. A failure leaves the state unchanged.
    pub fn ack_packet(&mut self) -> Result<Vec<u8>> {
        self.write_data_packet(None)
    }

    /// Assign `message` the next outgoing reliable sequence of the control
    /// channel and write it as one bunch in a datagram that also carries the
    /// ack state and, as space permits, naked bunches awaiting resend (oldest
    /// first, before it). The bunch is retained until acked; at most
    /// [`MAX_PENDING_RELIABLE`] may be outstanding. Fails without a state
    /// change when the message cannot be encoded, too many bunches are
    /// outstanding or the datagram would exceed the maximum packet size.
    pub fn reliable_control_packet(&mut self, message: ControlMessage) -> Result<Vec<u8>> {
        self.write_data_packet(Some(message))
    }

    /// One reliable control-channel bunch carrying `messages`: codec fields
    /// (F-522); the hard-coded name index 255 is inferred.
    fn reliable_bunch(&self, sequence: u32, messages: &[Decoded]) -> Result<Bunch> {
        let mut writer = BitWriter::new();
        for decoded in messages {
            let Decoded::Message(message) = decoded else {
                bail!("an outgoing control bunch holds an unknown message");
            };
            write_control_message(&mut writer, message, &self.config.codec)?;
        }
        Ok(Bunch {
            header: BunchHeader {
                control: false,
                open: false,
                close: false,
                close_reason: None,
                paused: false,
                reliable: true,
                channel_index: self.config.control_channel_index,
                has_package_map_exports: false,
                has_must_be_mapped_guids: false,
                partial: false,
                reliable_sequence_raw: Some(wire_reliable_sequence(sequence)),
                partial_initial: false,
                partial_final: false,
                channel_name: Some(Name::Hardcoded(INFERRED_CONTROL_NAME_INDEX)),
            },
            payload: Payload::from_writer(writer),
        })
    }

    /// Open actor channel `bunch.header.channel_index` with `bunch` (open and
    /// reliable) preceded by `exports` (export-only payloads), split into
    /// `OutgoingBunches` as `UChannel::SendBunch` does (F-709,
    /// [`split_outgoing_bunch`]): parts take consecutive reliable sequences
    /// from the channel's initial value and travel together in one datagram
    /// that also carries the ack state and, as space permits, naked bunches
    /// awaiting resend. The parts are retained unchanged until acked and are
    /// resent on naks like control bunches. Fails without a state change when
    /// the channel is the control channel or already open, the bunch is not an
    /// open reliable bunch, or the parts do not fit one datagram.
    pub fn reliable_actor_open(&mut self, bunch: &Bunch, exports: &[Payload]) -> Result<Vec<u8>> {
        self.ensure_usable()?;
        let index = bunch.header.channel_index;
        ensure!(
            index != self.config.control_channel_index
                && index < self.config.packet.limits.max_channels,
            "actor channel index {index} is the control channel or out of range"
        );
        ensure!(
            !self.actors.contains_key(&index),
            "actor channel {index} is already open"
        );
        ensure!(
            bunch.header.open && bunch.header.reliable && !bunch.header.partial,
            "an actor open is one whole, open, reliable bunch"
        );
        let channel: ReliableChannel<Bunch> = self.sequencer.new_channel();
        let first = channel
            .out_reliable()
            .checked_add(1)
            .context("reliable sequence counter exhausted")?;
        let mut template = bunch.clone();
        template.header.reliable_sequence_raw = Some(wire_reliable_sequence(first));
        let max_single = max_single_bunch_size_bits(
            self.config.packet.limits.max_packet_bytes,
            PACKET_HANDLER_BITS,
        )?;
        let parts = split_outgoing_bunch(
            &template,
            exports,
            max_single,
            Some(MAX_CONSTRUCTED_BUNCH_BYTES),
        )?;
        ensure!(
            parts.len() <= self.config.max_bunches_per_packet.min(MAX_PENDING_RELIABLE),
            "an actor open of {} parts does not fit one datagram",
            parts.len()
        );
        let (datagram, packet_id, resends) = self.assemble(&parts)?;
        let committed = self.sequencer.commit()?;
        let recorded = self.record_actor_open(committed, packet_id, &resends, channel, parts);
        self.fail_on_error(recorded)?;
        Ok(datagram)
    }

    /// Candidate resends, oldest first, rebuilt (control) or cloned (actor).
    fn resend_candidates(&self, room: usize) -> Result<Resends> {
        let mut resends = Vec::new();
        for entry in &self.resend {
            if resends.len() == room {
                break;
            }
            match *entry {
                Resend::Control(sequence) => {
                    let retained = self
                        .control
                        .out_bunches()
                        .iter()
                        .find(|bunch| bunch.reliable_sequence == sequence);
                    if let Some(bunch) = retained {
                        resends.push((*entry, self.reliable_bunch(sequence, &bunch.payload)?));
                    }
                }
                Resend::Actor(index, sequence) => {
                    let retained = self.actors.get(&index).and_then(|channel| {
                        channel
                            .out_bunches()
                            .iter()
                            .find(|bunch| bunch.reliable_sequence == sequence)
                    });
                    if let Some(bunch) = retained {
                        resends.push((*entry, bunch.payload.clone()));
                    }
                }
            }
        }
        Ok(resends)
    }

    /// Write the datagram with the ack state, resends that fit and `new`
    /// (which must fit); nothing is committed. Resends that do not fit are
    /// dropped from the tail of the candidates and stay queued.
    fn assemble(&self, new: &[Bunch]) -> Result<(Vec<u8>, u32, Resends)> {
        let header = self.sequencer.header()?;
        let packet_id = self.sequencer.out_packet_id();
        let room = self.config.max_bunches_per_packet.saturating_sub(new.len());
        let mut resends = self.resend_candidates(room)?;
        let max = self.config.packet.limits.max_packet_bytes as usize;
        loop {
            let packet = Packet {
                magic: 0,
                header: header.clone(),
                info: Some(PacketInfo {
                    jitter_clock: 0,
                    has_frame_time: false,
                    server_frame_time: None,
                }),
                bunches: resends
                    .iter()
                    .map(|(_, bunch)| bunch.clone())
                    .chain(new.iter().cloned())
                    .collect(),
            };
            let bytes = write_packet(&packet, &self.config.outgoing())?;
            if bytes.len() <= max {
                return Ok((bytes, packet_id, resends));
            }
            ensure!(
                resends.pop().is_some(),
                "outgoing datagram exceeds the maximum packet size"
            );
        }
    }

    /// Build the datagram completely, then commit: errors before the commit
    /// leave the state unchanged.
    fn write_data_packet(&mut self, message: Option<ControlMessage>) -> Result<Vec<u8>> {
        self.ensure_usable()?;
        let new = match message {
            Some(message) => {
                ensure!(
                    self.control.out_bunches().len() < MAX_PENDING_RELIABLE,
                    "{MAX_PENDING_RELIABLE} outgoing reliable control bunches are unacknowledged"
                );
                let sequence = self
                    .control
                    .out_reliable()
                    .checked_add(1)
                    .context("reliable sequence counter exhausted")?;
                let payload = vec![Decoded::Message(message)];
                let bunch = self.reliable_bunch(sequence, &payload)?;
                Some((sequence, bunch, payload))
            }
            None => None,
        };
        let new_bunches: Vec<Bunch> = new.iter().map(|(_, bunch, _)| bunch.clone()).collect();
        let (datagram, packet_id, resends) = self.assemble(&new_bunches)?;
        let committed = self.sequencer.commit()?;
        let recorded = self.record_sent(committed, packet_id, &resends, new);
        self.fail_on_error(recorded)?;
        Ok(datagram)
    }

    /// After the commit: move resent bunches to the new packet ID and drop
    /// resend entries that were sent or already released.
    fn record_resends(&mut self, packet_id: u32, resends: &[(Resend, Bunch)]) -> Result<()> {
        for (entry, _) in resends {
            match *entry {
                Resend::Control(sequence) => {
                    self.control.reassign(sequence, packet_id)?;
                }
                Resend::Actor(index, sequence) => {
                    self.actors
                        .get_mut(&index)
                        .context("resent actor channel vanished")?
                        .reassign(sequence, packet_id)?;
                }
            }
        }
        let control = self.control.out_bunches();
        let actors = &self.actors;
        self.resend.retain(|entry| {
            !resends.iter().any(|(resent, _)| resent == entry)
                && match *entry {
                    Resend::Control(sequence) => control
                        .iter()
                        .any(|bunch| bunch.reliable_sequence == sequence),
                    Resend::Actor(index, sequence) => actors.get(&index).is_some_and(|channel| {
                        channel
                            .out_bunches()
                            .iter()
                            .any(|bunch| bunch.reliable_sequence == sequence)
                    }),
                }
        });
        Ok(())
    }

    /// After the commit: record resends and retain the new control bunch. The
    /// checks above make this infallible in practice; an error still fails
    /// the peer because the state is then partly updated.
    fn record_sent(
        &mut self,
        committed: u32,
        packet_id: u32,
        resends: &[(Resend, Bunch)],
        new: Option<(u32, Bunch, Vec<Decoded>)>,
    ) -> Result<()> {
        ensure!(committed == packet_id, "committed packet ID mismatch");
        self.record_resends(packet_id, resends)?;
        if let Some((sequence, _, payload)) = new {
            let assigned = self.control.send_reliable(packet_id, payload)?;
            ensure!(assigned == sequence, "outgoing reliable sequence mismatch");
        }
        Ok(())
    }

    /// After the commit: record resends and retain the open's parts under
    /// consecutive sequences of the new channel.
    fn record_actor_open(
        &mut self,
        committed: u32,
        packet_id: u32,
        resends: &[(Resend, Bunch)],
        mut channel: ReliableChannel<Bunch>,
        parts: Vec<Bunch>,
    ) -> Result<()> {
        ensure!(committed == packet_id, "committed packet ID mismatch");
        self.record_resends(packet_id, resends)?;
        let index = parts
            .first()
            .context("an actor open has at least one part")?
            .header
            .channel_index;
        for part in parts {
            let raw = part.header.reliable_sequence_raw;
            let sequence = channel.send_reliable(packet_id, part)?;
            ensure!(
                raw == Some(wire_reliable_sequence(sequence)),
                "outgoing reliable sequence mismatch"
            );
        }
        self.actors.insert(index, channel);
        Ok(())
    }
}

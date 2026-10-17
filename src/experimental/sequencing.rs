//! EXPERIMENTAL disconnected model of the client's packet sequencing and ack layer.
//!
//! Static client-build observations only (F-535..F-539, F-710): sequence
//! seeding, the outgoing header's history words, the receive-side ack/nak
//! notification, the per-channel reliable bunch queue and per-channel partial
//! bunch reassembly (`UChannel::ReceivedNextBunch` `0x1437c5500`). Pure state
//! machines: no I/O, timers, channel routing, resend scheduling or admission.
//! Headers are the codec's [`PacketHeader`]; serialization stays in `packet`.
//! Where the client's behaviour was not observed (the sent-record push site,
//! the ring capacity, history words beyond those received, packet-id advance,
//! the reliable comparison window) the choice is marked inferred or assumed.
use std::collections::{BTreeMap, VecDeque};

use anyhow::{Context, Result, bail, ensure};

use super::packet::{
    BitWriter, Bunch, BunchHeader, MAX_HISTORY_WORDS, PacketHeader, Payload,
    RELIABLE_SEQUENCE_LIMIT,
};

/// Packet sequences are 14 bits.
pub const SEQUENCE_MASK: u16 = 0x3fff;
/// A sequence difference below this (and nonzero) is newer.
pub const SEQUENCE_HALF_RANGE: u16 = 0x2000;
/// Ack-history bits: eight 32-bit words (F-535).
pub const HISTORY_BITS: u16 = 256;
/// Reliable sequences are serialized as 10 bits (F-536).
pub const RELIABLE_MASK: u32 = RELIABLE_SEQUENCE_LIMIT - 1;
/// Assumed half range of the reliable comparison (see [`reconstruct_reliable_sequence`]).
pub const RELIABLE_HALF_RANGE: u32 = RELIABLE_SEQUENCE_LIMIT / 2;
/// Out-of-order reliable bunches a channel may queue; one more fails (F-537).
pub const MAX_QUEUED_RELIABLE: usize = 255;
/// Maximum constructed partial bunch size enforced on a non-client receiver
/// (`NetMaxConstructedPartialBunchSizeBytes` at `0x146056454`,
/// `UChannel::ReceivedNextBunch` `0x1437c5500`, F-710).
pub const MAX_CONSTRUCTED_PARTIAL_BUNCH_BYTES: usize = 65_536;
/// Fixed packet + bunch header/trailer bit overhead subtracted from
/// `Connection->MaxPacket * 8 - Connection->MaxPacketHandlerBits` in
/// `UChannel::SendBunch` (`0x1437c9690` line 82) and
/// `UPackageMapClient::ExportNetGUID` (`0x1439aabb0` line 75): `0x235` (`565` bits).
pub const MAX_SINGLE_BUNCH_OVERHEAD_BITS: usize = 0x235;
const WORD_BITS: u16 = 32;

/// `(a - b)` modulo 2^14.
pub fn sequence_difference(a: u16, b: u16) -> u16 {
    a.wrapping_sub(b) & SEQUENCE_MASK
}

/// `a` is 1..0x2000 ahead of `b` modulo 2^14.
pub fn is_newer(a: u16, b: u16) -> bool {
    (1..SEQUENCE_HALF_RANGE).contains(&sequence_difference(a, b))
}

/// The 14-bit difference read as a signed 14-bit value, -0x2000..0x2000.
pub fn signed_sequence_delta(a: u16, b: u16) -> i16 {
    let difference = sequence_difference(a, b) as i16;
    if difference >= SEQUENCE_HALF_RANGE as i16 {
        difference - (SEQUENCE_MASK as i16 + 1)
    } else {
        difference
    }
}

fn next_sequence(sequence: u16) -> u16 {
    sequence.wrapping_add(1) & SEQUENCE_MASK
}

fn ensure_sequence(sequence: u16) -> Result<()> {
    ensure!(sequence <= SEQUENCE_MASK, "packet sequence exceeds 14 bits");
    Ok(())
}

/// What the client records per sent packet: its sequence and the incoming
/// sequence it acknowledged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SentRecord {
    pub out_seq: u16,
    pub in_ack_seq: u16,
}

/// The 14-bit notify state (F-535, F-536, F-538).
///
/// History word 0 is least significant; bit 0 is the newest incoming sequence
/// (`in_ack_seq`), set when it was delivered.
#[derive(Clone, Debug)]
pub struct PacketNotify {
    in_seq: u16,
    in_ack_seq: u16,
    in_ack_seq_ack: u16,
    out_seq: u16,
    out_ack_seq: u16,
    history: [u32; MAX_HISTORY_WORDS],
    sent: VecDeque<SentRecord>,
    sent_capacity: usize,
}

impl PacketNotify {
    /// Seed as the client's notify init (F-536): every incoming field takes
    /// `in_seq`, `out_ack_seq` is `out_seq - 1` and the history is clear.
    ///
    /// `sent_capacity` bounds the sent-record ring. The client's capacity is not
    /// established; [`commit`](Self::commit) fails once it is full.
    pub fn new(in_seq: u16, out_seq: u16, sent_capacity: usize) -> Result<Self> {
        ensure_sequence(in_seq)?;
        ensure_sequence(out_seq)?;
        ensure!(
            sent_capacity >= 1,
            "sent-record capacity must be at least one"
        );
        Ok(Self {
            in_seq,
            in_ack_seq: in_seq,
            in_ack_seq_ack: in_seq,
            out_seq,
            out_ack_seq: out_seq.wrapping_sub(1) & SEQUENCE_MASK,
            history: [0; MAX_HISTORY_WORDS],
            sent: VecDeque::with_capacity(sent_capacity),
            sent_capacity,
        })
    }

    /// Newest accepted incoming sequence.
    pub fn in_seq(&self) -> u16 {
        self.in_seq
    }

    /// Newest incoming sequence entered into the history.
    pub fn in_ack_seq(&self) -> u16 {
        self.in_ack_seq
    }

    /// Incoming sequence the peer is known to have seen acknowledged.
    pub fn in_ack_seq_ack(&self) -> u16 {
        self.in_ack_seq_ack
    }

    /// Sequence of the next outgoing packet.
    pub fn out_seq(&self) -> u16 {
        self.out_seq
    }

    /// Newest outgoing sequence the peer acknowledged.
    pub fn out_ack_seq(&self) -> u16 {
        self.out_ack_seq
    }

    pub fn history(&self) -> &[u32; MAX_HISTORY_WORDS] {
        &self.history
    }

    /// Oldest first.
    pub fn sent_records(&self) -> &VecDeque<SentRecord> {
        &self.sent
    }

    pub fn sent_capacity(&self) -> usize {
        self.sent_capacity
    }

    /// AckSeq: advance `in_ack_seq` one step at a time to `sequence`, shifting
    /// the history left by one per step; only the final step records
    /// `delivered`, skipped sequences record 0. A sequence that is not newer is
    /// ignored.
    pub fn ack_seq(&mut self, sequence: u16, delivered: bool) -> Result<()> {
        ensure_sequence(sequence)?;
        while self.in_ack_seq != sequence && is_newer(sequence, self.in_ack_seq) {
            self.in_ack_seq = next_sequence(self.in_ack_seq);
            self.shift_history(delivered && self.in_ack_seq == sequence);
        }
        Ok(())
    }

    fn shift_history(&mut self, bit: bool) {
        for index in (1..MAX_HISTORY_WORDS).rev() {
            self.history[index] = (self.history[index] << 1) | (self.history[index - 1] >> 31);
        }
        self.history[0] = (self.history[0] << 1) | u32::from(bit);
    }

    /// History words the next header carries (F-535): the unacked incoming span
    /// rounded up to whole words, clamped to 1..=8. A span above 256 bits needs
    /// more words than exist and fails, as the client's refresh does.
    pub fn history_word_count(&self) -> Result<usize> {
        let span = sequence_difference(self.in_ack_seq, self.in_ack_seq_ack);
        ensure!(
            span <= HISTORY_BITS,
            "unacked history span {span} exceeds 256 bits"
        );
        Ok(usize::from(span.div_ceil(WORD_BITS)).clamp(1, MAX_HISTORY_WORDS))
    }

    /// The outgoing header (F-535): `out_seq`, `in_ack_seq` and the first
    /// [`history_word_count`](Self::history_word_count) words. Does not commit.
    pub fn header(&self) -> Result<PacketHeader> {
        let count = self.history_word_count()?;
        Ok(PacketHeader {
            sequence: self.out_seq,
            acked_sequence: self.in_ack_seq,
            history: self.history[..count].to_vec(),
        })
    }

    /// Record `(out_seq, in_ack_seq)` and advance `out_seq`. The ring's role is
    /// observed through its consumer (F-538); the client's push site is not yet
    /// observed, so committing after writing the header is inferred.
    pub fn commit(&mut self) -> Result<u16> {
        ensure!(
            self.sent.len() < self.sent_capacity,
            "sent-record ring is full ({} records)",
            self.sent_capacity
        );
        let sequence = self.out_seq;
        self.sent.push_back(SentRecord {
            out_seq: sequence,
            in_ack_seq: self.in_ack_seq,
        });
        self.out_seq = next_sequence(sequence);
        Ok(sequence)
    }

    /// The receive gate: `sequence` must be newer than `in_seq`; returns the
    /// positive difference, or `None` when the client rejects the packet.
    pub fn incoming_delta(&self, sequence: u16) -> Option<u16> {
        is_newer(sequence, self.in_seq).then(|| sequence_difference(sequence, self.in_seq))
    }

    /// Notify update (F-538) from a received header's ack fields.
    ///
    /// An acked sequence equal to `out_ack_seq` or 0x2000+ beyond it is ignored
    /// (returns 0). Otherwise `in_ack_seq_ack` follows the sent record for the
    /// acked sequence, or falls back to `acked - 256` when that record is absent
    /// (the ring is then left unchanged); each sequence after `out_ack_seq` up to
    /// the acked one is then passed in order to `on_packet` with its delivery:
    /// lost beyond the newest 256, else history bit `acked - sequence`. Words
    /// beyond those received read as 0: the client indexes a fixed 256-bit
    /// header buffer and zero-fill of unreceived words is assumed, not
    /// established. Returns the number of sequences notified. After an error the
    /// state is unspecified; the client closes the connection.
    pub fn update(
        &mut self,
        header: &PacketHeader,
        mut on_packet: impl FnMut(u16, bool) -> Result<()>,
    ) -> Result<u16> {
        ensure_sequence(header.acked_sequence)?;
        ensure!(
            header.history.len() <= MAX_HISTORY_WORDS,
            "packet history exceeds 8 words"
        );
        let acked = header.acked_sequence;
        let count = sequence_difference(acked, self.out_ack_seq);
        if count == 0 || count >= SEQUENCE_HALF_RANGE {
            return Ok(0);
        }
        self.in_ack_seq_ack = self.acked_in_ack_seq(usize::from(count), acked);
        let mut sequence = self.out_ack_seq;
        for _ in 0..count {
            sequence = next_sequence(sequence);
            let index = sequence_difference(acked, sequence);
            let delivered = index < HISTORY_BITS && {
                let word = header
                    .history
                    .get(usize::from(index / WORD_BITS))
                    .copied()
                    .unwrap_or(0);
                (word >> (index % WORD_BITS)) & 1 == 1
            };
            on_packet(sequence, delivered)?;
        }
        self.out_ack_seq = acked;
        Ok(count)
    }

    fn acked_in_ack_seq(&mut self, count: usize, acked: u16) -> u16 {
        let fallback = acked.wrapping_sub(HISTORY_BITS) & SEQUENCE_MASK;
        if count > self.sent.len() {
            return fallback;
        }
        self.sent.drain(..count - 1);
        match self.sent.pop_front() {
            Some(record) if record.out_seq == acked => record.in_ack_seq,
            _ => fallback,
        }
    }

    /// Gate, update, then accept `header.sequence` as `in_seq`. Returns the
    /// incoming difference, or `None` (state unchanged) when rejected.
    pub fn receive_header(
        &mut self,
        header: &PacketHeader,
        on_packet: impl FnMut(u16, bool) -> Result<()>,
    ) -> Result<Option<u16>> {
        ensure_sequence(header.sequence)?;
        let Some(delta) = self.incoming_delta(header.sequence) else {
            return Ok(None);
        };
        self.update(header, on_packet)?;
        self.in_seq = header.sequence;
        Ok(Some(delta))
    }
}

/// One ack or nak for a full outgoing packet ID.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Notification {
    pub packet_id: u32,
    pub delivered: bool,
}

/// Advance the last notified packet ID by one and check that its low 14 bits
/// equal the notified sequence; the client closes the connection otherwise
/// (F-538).
pub fn next_notification(
    last_notified_packet_id: &mut u32,
    sequence: u16,
    delivered: bool,
) -> Result<Notification> {
    let packet_id = last_notified_packet_id.wrapping_add(1);
    ensure!(
        packet_id as u16 & SEQUENCE_MASK == sequence,
        "packet id {packet_id:#x} does not match notified sequence {sequence:#x}; the client closes the connection"
    );
    *last_notified_packet_id = packet_id;
    Ok(Notification {
        packet_id,
        delivered,
    })
}

/// An accepted incoming header.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Received {
    /// The accepted 14-bit sequence; pass it to `ack_seq` after processing.
    pub sequence: u16,
    /// Full incoming packet ID after advancing by the sequence difference
    /// (inferred; only the seed is observed).
    pub packet_id: u32,
    /// Incoming sequences skipped between the previous and this one.
    pub skipped: u16,
    /// Acks and naks for outgoing packets, oldest first.
    pub notifications: Vec<Notification>,
}

/// Connection-level sequencing: packet IDs around a [`PacketNotify`] (F-536).
#[derive(Clone, Debug)]
pub struct PacketSequencer {
    notify: PacketNotify,
    last_notified_packet_id: u32,
    in_packet_id: u32,
    out_packet_id: u32,
    initial_in_reliable: u32,
    initial_out_reliable: u32,
}

impl PacketSequencer {
    /// InitSequence (F-536, M-355) with cookie-derived 14-bit `incoming` and
    /// `outgoing`: `in_packet_id = incoming - 1`, `out_packet_id = outgoing`,
    /// `last_notified_packet_id = outgoing - 1` (32-bit wrapping); the notify
    /// state is seeded from `in_packet_id & 0x3fff` and `outgoing`; channels'
    /// reliable sequences start at `incoming & 0x3ff` and `outgoing & 0x3ff`.
    pub fn new(incoming: u16, outgoing: u16, sent_capacity: usize) -> Result<Self> {
        ensure_sequence(incoming)?;
        ensure_sequence(outgoing)?;
        let in_packet_id = u32::from(incoming).wrapping_sub(1);
        Ok(Self {
            notify: PacketNotify::new(
                in_packet_id as u16 & SEQUENCE_MASK,
                outgoing,
                sent_capacity,
            )?,
            last_notified_packet_id: u32::from(outgoing).wrapping_sub(1),
            in_packet_id,
            out_packet_id: u32::from(outgoing),
            initial_in_reliable: u32::from(incoming) & RELIABLE_MASK,
            initial_out_reliable: u32::from(outgoing) & RELIABLE_MASK,
        })
    }

    pub fn notify(&self) -> &PacketNotify {
        &self.notify
    }

    pub fn last_notified_packet_id(&self) -> u32 {
        self.last_notified_packet_id
    }

    pub fn in_packet_id(&self) -> u32 {
        self.in_packet_id
    }

    /// Full ID of the next outgoing packet.
    pub fn out_packet_id(&self) -> u32 {
        self.out_packet_id
    }

    /// Initial incoming and outgoing reliable sequences of every channel.
    pub fn initial_reliable(&self) -> (u32, u32) {
        (self.initial_in_reliable, self.initial_out_reliable)
    }

    /// A channel seeded with [`initial_reliable`](Self::initial_reliable).
    pub fn new_channel<T>(&self) -> ReliableChannel<T> {
        ReliableChannel::new(self.initial_in_reliable, self.initial_out_reliable)
    }

    pub fn header(&self) -> Result<PacketHeader> {
        self.notify.header()
    }

    /// Commit the packet whose header was written; returns its full packet ID.
    /// Advancing `out_packet_id` together with `out_seq` is inferred.
    pub fn commit(&mut self) -> Result<u32> {
        self.notify.commit()?;
        let packet_id = self.out_packet_id;
        self.out_packet_id = packet_id.wrapping_add(1);
        Ok(packet_id)
    }

    /// Gate and process a received header; `None` when the client rejects it.
    /// An error means the client closes the connection.
    pub fn receive(&mut self, header: &PacketHeader) -> Result<Option<Received>> {
        let mut notifications = Vec::new();
        let last = &mut self.last_notified_packet_id;
        let Some(delta) = self.notify.receive_header(header, |sequence, delivered| {
            notifications.push(next_notification(last, sequence, delivered)?);
            Ok(())
        })?
        else {
            return Ok(None);
        };
        self.in_packet_id = self.in_packet_id.wrapping_add(u32::from(delta));
        Ok(Some(Received {
            sequence: header.sequence,
            packet_id: self.in_packet_id,
            skipped: delta - 1,
            notifications,
        }))
    }

    /// Enter a processed incoming sequence into the history (see
    /// [`PacketNotify::ack_seq`]).
    pub fn ack_seq(&mut self, sequence: u16, delivered: bool) -> Result<()> {
        self.notify.ack_seq(sequence, delivered)
    }
}

/// The 10-bit wire value of a reliable sequence (the bunch header's
/// `reliable_sequence_raw`).
pub fn wire_reliable_sequence(sequence: u32) -> u32 {
    sequence & RELIABLE_MASK
}

/// Reconstruct a 10-bit wire value relative to the channel's last sequence:
/// `last` plus the signed 10-bit difference, -512..=511. The client compares
/// against its own relative reconstruction; this half range is assumed. The
/// result may be negative for stale values.
pub fn reconstruct_reliable_sequence(raw: u32, last: u32) -> Result<i64> {
    ensure!(
        raw < RELIABLE_SEQUENCE_LIMIT,
        "reliable sequence exceeds 10 bits"
    );
    let difference =
        (raw.wrapping_sub(last).wrapping_add(RELIABLE_HALF_RANGE) & RELIABLE_MASK) as i64;
    Ok(i64::from(last) + difference - i64::from(RELIABLE_HALF_RANGE))
}

/// One sent reliable bunch awaiting its ack.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutBunch<T> {
    /// Unwrapped sequence; the wire carries [`wire_reliable_sequence`] of it.
    pub reliable_sequence: u32,
    /// Full ID of the packet that last carried it.
    pub packet_id: u32,
    pub acked: bool,
    pub payload: T,
}

/// Result of one received reliable bunch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReliableReceive<T> {
    /// This payload and any queued consecutive ones, in sequence order.
    Delivered(Vec<T>),
    /// Ahead of the next expected sequence; held in order.
    Queued,
    /// Not newer than the last delivered, or already queued; dropped.
    Duplicate,
}

/// One channel's reliable layer (F-537, F-539).
///
/// Sequences are kept unwrapped from their 10-bit seeds and compared after
/// reconstruction; the client's counter width is inferred. The client's
/// outgoing reliable limit is not modelled.
#[derive(Clone, Debug)]
pub struct ReliableChannel<T> {
    in_reliable: u32,
    out_reliable: u32,
    out_bunches: VecDeque<OutBunch<T>>,
    queued: BTreeMap<u32, T>,
}

impl<T> ReliableChannel<T> {
    /// `in_reliable` and `out_reliable` are the last delivered and last sent
    /// sequences, normally the connection's initial reliable values.
    pub fn new(in_reliable: u32, out_reliable: u32) -> Self {
        Self {
            in_reliable,
            out_reliable,
            out_bunches: VecDeque::new(),
            queued: BTreeMap::new(),
        }
    }

    pub fn in_reliable(&self) -> u32 {
        self.in_reliable
    }

    pub fn out_reliable(&self) -> u32 {
        self.out_reliable
    }

    /// Unreleased out-bunches, oldest first.
    pub fn out_bunches(&self) -> &VecDeque<OutBunch<T>> {
        &self.out_bunches
    }

    pub fn queued_len(&self) -> usize {
        self.queued.len()
    }

    /// Assign the next reliable sequence to `payload`, sent in `packet_id`.
    pub fn send_reliable(&mut self, packet_id: u32, payload: T) -> Result<u32> {
        let sequence = self
            .out_reliable
            .checked_add(1)
            .context("reliable sequence counter exhausted")?;
        self.out_reliable = sequence;
        self.out_bunches.push_back(OutBunch {
            reliable_sequence: sequence,
            packet_id,
            acked: false,
            payload,
        });
        Ok(sequence)
    }

    /// Ack for `packet_id` (F-539): flag its out-bunches acked, then release
    /// acked bunches from the head in order.
    pub fn acked(&mut self, packet_id: u32) -> Vec<OutBunch<T>> {
        for bunch in &mut self.out_bunches {
            if bunch.packet_id == packet_id {
                bunch.acked = true;
            }
        }
        let released = self
            .out_bunches
            .iter()
            .take_while(|bunch| bunch.acked)
            .count();
        self.out_bunches.drain(..released).collect()
    }

    /// Nak for `packet_id` (F-539): sequences of every unacked out-bunch it
    /// carried, oldest first, for the caller to resend and
    /// [`reassign`](Self::reassign).
    pub fn lost(&self, packet_id: u32) -> Vec<u32> {
        self.out_bunches
            .iter()
            .filter(|bunch| !bunch.acked && bunch.packet_id == packet_id)
            .map(|bunch| bunch.reliable_sequence)
            .collect()
    }

    /// Record that a resent bunch now travels in `packet_id`.
    pub fn reassign(&mut self, reliable_sequence: u32, packet_id: u32) -> Result<&OutBunch<T>> {
        let bunch = self
            .out_bunches
            .iter_mut()
            .find(|bunch| bunch.reliable_sequence == reliable_sequence)
            .context("no unreleased out-bunch with that reliable sequence")?;
        bunch.packet_id = packet_id;
        Ok(bunch)
    }

    /// ReceivedRawBunch ordering (F-537) for a 10-bit wire value: the next
    /// expected sequence is delivered and the queue drains in order; a value
    /// further ahead is queued in order; an older or already queued one is
    /// dropped. Queuing more than 255 fails.
    pub fn receive_reliable(&mut self, raw: u32, payload: T) -> Result<ReliableReceive<T>> {
        let reconstructed = reconstruct_reliable_sequence(raw, self.in_reliable)?;
        if reconstructed <= i64::from(self.in_reliable) {
            return Ok(ReliableReceive::Duplicate);
        }
        let Ok(sequence) = u32::try_from(reconstructed) else {
            bail!("reliable sequence counter exhausted");
        };
        if sequence - 1 != self.in_reliable {
            if self.queued.contains_key(&sequence) {
                return Ok(ReliableReceive::Duplicate);
            }
            ensure!(
                self.queued.len() < MAX_QUEUED_RELIABLE,
                "more than 255 queued reliable bunches"
            );
            self.queued.insert(sequence, payload);
            return Ok(ReliableReceive::Queued);
        }
        self.in_reliable = sequence;
        let mut delivered = vec![payload];
        while let Some(entry) = self.queued.first_entry() {
            if *entry.key() - 1 != self.in_reliable {
                break;
            }
            self.in_reliable = *entry.key();
            delivered.push(entry.remove());
        }
        Ok(ReliableReceive::Delivered(delivered))
    }
}

/// Result of feeding one partial bunch part into [`PartialBunchReassembler`]
/// (`UChannel::ReceivedNextBunch` `0x1437c5500`, F-710).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PartialReceive {
    /// An unreliable `bPartialInitial` arrived while a reliable partial bunch
    /// was in progress; the unreliable initial is dropped and the reliable
    /// assembly stays intact (`*bOutSkipAck = 1` at `0x1437c5500` line 139).
    IgnoredUnreliableInitial,
    /// Part accepted; waiting for subsequent continuation/final parts.
    Incomplete {
        parts: usize,
        replaced_unreliable: bool,
    },
    /// Final part accepted; returns the reassembled [`Bunch`] and total part count.
    Complete {
        bunch: Bunch,
        parts: usize,
        replaced_unreliable: bool,
    },
}

#[derive(Clone, Debug)]
struct InProgressPartial {
    reliable: bool,
    sequence: i64,
    header: BunchHeader,
    data: BitWriter,
    parts: usize,
    must_be_mapped: bool,
}

/// Per-channel partial bunch reassembler (`UChannel::ReceivedNextBunch`
/// `0x1437c5500`, F-710).
///
/// Key static client-build rules (`0x1437c5500`):
/// - `UPackageMapClient::ReceiveNetGUIDBunch` (`0x1439b8bb0`) runs in
///   `UChannel::ReceivedRawBunch` (`0x1437c5cc0`) before `ReceivedNextBunch`.
///   When `header.has_package_map_exports` is set on an initial or continuation
///   partial bunch, `ReceivedNextBunch` does not copy payload bits from that
///   part into `InPartialBunch` and does not require 8-bit payload alignment on
///   that part.
/// - Non-final parts without `has_package_map_exports` must be byte-aligned
///   (`(rem_bits & 7) == 0`), and initial parts without exports must also be
///   non-empty (`rem_bits > 0`).
/// - A reliable `bPartialInitial` arriving while an incomplete reliable
///   `InPartialBunch` is already in progress is rejected (`Bunch.SetError()`).
///   An unreliable `bPartialInitial` arriving while a reliable `InPartialBunch`
///   is in progress is dropped (`IgnoredUnreliableInitial`). An incomplete
///   unreliable `InPartialBunch` is replaced by a new `bPartialInitial`.
/// - Continuation parts (`!bPartialInitial`) require `InPartialBunch` to exist
///   with matching `reliable` and consecutive `sequence` (`sequence == prev + 1`
///   when reliable, or `prev..=prev + 1` when unreliable).
/// - Final parts (`bPartialFinal`) reject `has_package_map_exports`, copy
///   `close`, `close_reason` and `has_must_be_mapped_guids` onto the
///   reassembled header, and complete the bunch.
#[derive(Clone, Debug, Default)]
pub struct PartialBunchReassembler {
    in_progress: Option<InProgressPartial>,
}

impl PartialBunchReassembler {
    pub fn new() -> Self {
        Self::default()
    }

    /// True when an incomplete partial bunch is currently held.
    pub fn in_progress(&self) -> bool {
        self.in_progress.is_some()
    }

    /// Number of parts accumulated in the current incomplete assembly, if any.
    pub fn parts_in_progress(&self) -> Option<usize> {
        self.in_progress.as_ref().map(|partial| partial.parts)
    }

    /// Clear any in-progress assembly (for example when a channel opens or closes).
    pub fn clear(&mut self) {
        self.in_progress = None;
    }

    /// Feed one partial bunch part (`header.partial == true`) after any
    /// `has_package_map_exports` prefix has been stripped into
    /// `payload_after_exports`.
    ///
    /// When `max_constructed_bytes` is `Some(limit)` (on a server,
    /// `Some(MAX_CONSTRUCTED_PARTIAL_BUNCH_BYTES)`), rejects assemblies whose
    /// constructed byte size `(bit_len + 7) / 8` exceeds `limit`.
    pub fn receive_part(
        &mut self,
        sequence: i64,
        header: &BunchHeader,
        payload_after_exports: &Payload,
        max_constructed_bytes: Option<usize>,
    ) -> Result<PartialReceive> {
        ensure!(
            header.partial,
            "non-partial bunch passed to PartialBunchReassembler"
        );
        let mut replaced_unreliable = false;
        if header.partial_initial {
            if let Some(existing) = &self.in_progress {
                if existing.reliable && header.reliable {
                    bail!(
                        "reliable partial initial bunch cannot overwrite an incomplete reliable partial bunch"
                    );
                }
                if existing.reliable && !header.reliable {
                    return Ok(PartialReceive::IgnoredUnreliableInitial);
                }
                replaced_unreliable = true;
                self.in_progress = None;
            }
            if header.has_package_map_exports {
                ensure!(
                    !header.partial_final,
                    "final partial bunch cannot carry package-map exports"
                );
                ensure!(
                    payload_after_exports.bit_len() == 0,
                    "partial initial export bunch has {} leftover bits after exports",
                    payload_after_exports.bit_len()
                );
            } else if !header.partial_final {
                ensure!(
                    payload_after_exports.bit_len() > 0
                        && payload_after_exports.bit_len().is_multiple_of(8),
                    "non-final partial initial bunch payload ({} bits) must be non-empty and byte-aligned",
                    payload_after_exports.bit_len()
                );
            }
            let mut writer = BitWriter::new();
            if !header.has_package_map_exports {
                append_payload(&mut writer, payload_after_exports);
            }
            if let Some(limit) = max_constructed_bytes {
                ensure!(
                    writer.bit_len().div_ceil(8) <= limit,
                    "constructed partial bunch exceeds {limit} bytes"
                );
            }
            self.in_progress = Some(InProgressPartial {
                reliable: header.reliable,
                sequence,
                header: header.clone(),
                data: writer,
                parts: 1,
                must_be_mapped: header.has_must_be_mapped_guids,
            });
        } else {
            let Some(partial) = self.in_progress.as_mut() else {
                bail!("partial continuation bunch without an initial partial bunch");
            };
            let next = sequence == partial.sequence + 1;
            let sequence_matches = if partial.reliable {
                next
            } else {
                next || sequence == partial.sequence
            };
            if !sequence_matches || partial.reliable != header.reliable {
                self.in_progress = None;
                bail!("partial continuation sequence or reliability mismatch");
            }
            if header.partial_final && header.has_package_map_exports {
                self.in_progress = None;
                bail!("final partial bunch cannot carry package-map exports");
            }
            if header.has_package_map_exports {
                if payload_after_exports.bit_len() != 0 {
                    self.in_progress = None;
                    bail!(
                        "partial continuation export bunch has {} leftover bits after exports",
                        payload_after_exports.bit_len()
                    );
                }
            } else {
                if !header.partial_final && !payload_after_exports.bit_len().is_multiple_of(8) {
                    let bits = payload_after_exports.bit_len();
                    self.in_progress = None;
                    bail!(
                        "non-final partial continuation payload ({bits} bits) must be byte-aligned"
                    );
                }
                append_payload(&mut partial.data, payload_after_exports);
                if let Some(limit) = max_constructed_bytes
                    && partial.data.bit_len().div_ceil(8) > limit
                {
                    self.in_progress = None;
                    bail!("constructed partial bunch exceeds {limit} bytes");
                }
            }
            partial.sequence = sequence;
            partial.parts += 1;
            partial.must_be_mapped |= header.has_must_be_mapped_guids;
        }

        if !header.partial_final {
            let parts = self.in_progress.as_ref().map_or(0, |p| p.parts);
            return Ok(PartialReceive::Incomplete {
                parts,
                replaced_unreliable,
            });
        }

        let partial = self.in_progress.take().expect("partial in progress");
        let mut merged = partial.header;
        merged.partial = false;
        merged.partial_initial = false;
        merged.partial_final = false;
        merged.has_package_map_exports = false;
        merged.has_must_be_mapped_guids = partial.must_be_mapped;
        merged.close = header.close;
        merged.close_reason = header.close_reason;
        merged.control = merged.open || merged.close;
        Ok(PartialReceive::Complete {
            bunch: Bunch {
                header: merged,
                payload: Payload::from_writer(partial.data),
            },
            parts: partial.parts,
            replaced_unreliable,
        })
    }
}

fn append_payload(writer: &mut BitWriter, payload: &Payload) {
    let mut reader = payload.reader();
    let mut bits = reader.remaining();
    while bits > 0 {
        let chunk = bits.min(32) as u32;
        writer.write_bits(reader.read_bits(chunk), chunk);
        bits -= chunk as usize;
    }
}

/// Maximum payload bits a single non-partial bunch may carry before
/// `UChannel::SendBunch` (`0x1437c9690` line 82) splits it into partial
/// bunches: `(max_packet_bytes * 8) - max_packet_handler_bits - 0x235`.
pub fn max_single_bunch_size_bits(
    max_packet_bytes: u32,
    max_packet_handler_bits: u32,
) -> Result<usize> {
    let total = usize::try_from(max_packet_bytes)
        .context("max_packet_bytes overflow")?
        .checked_mul(8)
        .context("max_packet_bytes * 8 overflow")?;
    let overhead = usize::try_from(max_packet_handler_bits)
        .context("max_packet_handler_bits overflow")?
        .checked_add(MAX_SINGLE_BUNCH_OVERHEAD_BITS)
        .context("bunch overhead overflow")?;
    ensure!(
        total > overhead,
        "max_packet_bytes ({max_packet_bytes}) too small for bunch overhead ({overhead} bits)"
    );
    Ok(total - overhead)
}

/// Byte-aligned slice size (`max_single_bunch_size_bits & !7`) used by
/// `UChannel::SendBunch` (`0x1437c9690` line 84) when splitting an outgoing
/// bunch larger than [`max_single_bunch_size_bits`].
pub fn max_partial_bunch_size_bits(
    max_packet_bytes: u32,
    max_packet_handler_bits: u32,
) -> Result<usize> {
    let single = max_single_bunch_size_bits(max_packet_bytes, max_packet_handler_bits)?;
    let partial = single & !7;
    ensure!(
        partial >= 8,
        "max_partial_bunch_size_bits ({partial}) must be at least 8 bits"
    );
    Ok(partial)
}

/// Segment an outgoing [`Bunch`] and any preceding export-only bunch payloads
/// (`UChannel::AppendExportBunches` `0x1437b03b0` /
/// `UPackageMapClient::AppendExportBunches` `0x1439a5290`) into wire
/// `OutgoingBunches` following `UChannel::SendBunch` (`0x1437c9690`).
///
/// Key static rules (`0x1437c9690`):
/// - When `max_constructed_bytes` is `Some(limit)` (`!IsUnlimitedBunchSizeAllowed`),
///   rejects `bunch.payload` when `(bit_len + 7) / 8 > limit` (`65,536` bytes).
/// - Each payload in `export_payloads` becomes a leading entry in
///   `OutgoingBunches` with `has_package_map_exports = true` and
///   `has_must_be_mapped_guids = false`.
/// - When `bunch.payload.bit_len() > max_single_bits`, `bunch.payload` is
///   sliced into chunks of `max_partial_bits = max_single_bits & !7` bits
///   (whole bytes), with the final chunk carrying the remaining bits;
///   otherwise `bunch.payload` is appended as a single entry.
/// - When `OutgoingBunches.len() > 1`, every entry sets `partial = true`,
///   only index `0` sets `partial_initial = true` and preserves `open`, and
///   only index `OutgoingBunches.len() - 1` sets `partial_final = true` and
///   preserves `close` and `close_reason`.
/// - When `bunch.header.reliable` is set and `reliable_sequence_raw` is
///   `Some(first_raw)`, each entry in `OutgoingBunches` receives the next
///   10-bit wire reliable sequence `(first_raw + i) & 0x3ff`.
pub fn split_outgoing_bunch(
    bunch: &Bunch,
    export_payloads: &[Payload],
    max_single_bits: usize,
    max_constructed_bytes: Option<usize>,
) -> Result<Vec<Bunch>> {
    if let Some(limit) = max_constructed_bytes {
        ensure!(
            bunch.payload.bit_len().div_ceil(8) <= limit,
            "outgoing bunch payload ({} bytes) exceeds limit ({limit} bytes)",
            bunch.payload.bit_len().div_ceil(8)
        );
    }
    let max_partial_bits = max_single_bits & !7;
    ensure!(
        max_partial_bits >= 8,
        "max_single_bits ({max_single_bits}) must be at least 8 bits"
    );

    let mut slices: Vec<(Payload, bool)> = Vec::new();
    for export in export_payloads {
        slices.push((export.clone(), true));
    }
    if bunch.payload.bit_len() > max_single_bits {
        let mut reader = bunch.payload.reader();
        while reader.remaining() > 0 {
            let take = reader.remaining().min(max_partial_bits);
            let mut writer = BitWriter::new();
            let mut rem = take;
            while rem > 0 {
                let chunk = rem.min(32) as u32;
                writer.write_bits(reader.read_bits(chunk), chunk);
                rem -= chunk as usize;
            }
            slices.push((Payload::from_writer(writer), false));
        }
    } else {
        slices.push((bunch.payload.clone(), bunch.header.has_package_map_exports));
    }

    let total = slices.len();
    let mut out = Vec::with_capacity(total);
    let mut next_raw = bunch.header.reliable_sequence_raw;
    for (index, (payload, is_export)) in slices.into_iter().enumerate() {
        let partial = total > 1;
        let partial_initial = partial && index == 0;
        let partial_final = partial && index + 1 == total;
        let open = bunch.header.open && (!partial || index == 0);
        let close = bunch.header.close && (!partial || index + 1 == total);
        let control = if partial {
            open || close
        } else {
            bunch.header.control || open || close
        };
        let close_reason = if close {
            bunch.header.close_reason
        } else {
            None
        };
        let reliable_sequence_raw = if bunch.header.reliable {
            let current = next_raw.map(wire_reliable_sequence);
            next_raw = next_raw.map(|raw| wire_reliable_sequence(raw.wrapping_add(1)));
            current
        } else {
            None
        };
        let channel_name = if open || bunch.header.reliable {
            bunch.header.channel_name.clone()
        } else {
            None
        };
        out.push(Bunch {
            header: BunchHeader {
                control,
                open,
                close,
                close_reason,
                paused: bunch.header.paused,
                reliable: bunch.header.reliable,
                channel_index: bunch.header.channel_index,
                has_package_map_exports: is_export,
                has_must_be_mapped_guids: !is_export && bunch.header.has_must_be_mapped_guids,
                partial,
                reliable_sequence_raw,
                partial_initial,
                partial_final,
                channel_name,
            },
            payload,
        });
    }
    Ok(out)
}

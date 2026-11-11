//! Explicitly selected PRIVATE capture of client-to-server bunches on each
//! connection's own pawn actor channel: a read-only tap for later offline
//! decoding (for example of the client's frozen-movement report, F-1100,
//! whose field index is not bound here).
//!
//! # Tap
//!
//! [`PawnBunchTap::observe`] borrows each wire bunch on the pawn channel at
//! the point where the session would hand it on: a reliable bunch (or part)
//! when its channel's reliable queue releases it, in order and once
//! (duplicates and queue overflow never reach the tap), an unreliable bunch
//! on arrival. This precedes every lab decision about the bunch (reassembly,
//! unsupported forms, dispatch or drop), so those decisions cannot hide it,
//! and the tap returns nothing the session reads: what the server sends and
//! how it treats the bunch are unchanged. I/O errors only disable capture.
//!
//! # Record format (JSONL, `format` 1)
//!
//! One JSON object and `\n` per retained wire bunch; partial bunches are kept
//! part by part (never reassembled), so an offline reader reassembles them
//! from the flags and reliable sequences exactly as the receiver would.
//!
//! - `format` (1), `selection` (`"pawn_channel_bunches"`), `ordinal` (1-based
//!   row number in this file).
//! - `keep_reason`: `first` (one of the connection's first
//!   [`KEEP_FIRST`] pawn bunches), `reliable`, `changed` (header flags, bit
//!   length or the first [`LEADING_BITS`] payload bits differ from the
//!   connection's previous kept bunch) or `interval` (the connection's
//!   previous kept bunch is at least [`KEEP_INTERVAL_MS`] older).
//! - `connection_ordinal` (the session's ordinal, never an address or
//!   identity), `connection_seen` (1-based count of this connection's pawn
//!   bunches seen, kept or not: gaps between rows show how many were
//!   sampled out), `channel`.
//! - `observation_phase`: `reliable_queue_release` or `unreliable_arrival`.
//! - `elapsed_ms` (server monotonic milliseconds since the lab bound, the
//!   clock of `native_join_frozen_movement`) and `unix_ms` (server wall
//!   clock), both read once for the receive call that delivered the bunch.
//! - `packet_sequence`: the packet-header sequence of that received datagram
//!   (for a released reliable bunch, the datagram that released it, not
//!   necessarily the one that carried it).
//! - The bunch header as received: `reliable`, `reliable_sequence` (the
//!   channel's reconstructed in-order sequence, reliable only),
//!   `reliable_sequence_raw` (the wire field), `partial`, `partial_initial`,
//!   `partial_final`, `open`, `close`, `paused`, `has_package_map_exports`,
//!   `has_must_be_mapped_guids`.
//! - `payload_bits`: the exact payload bit length; `payload_hex`: the payload
//!   bytes in wire order, two lowercase hex digits each,
//!   `ceil(payload_bits / 8)` bytes. Bit `i` is bit `i % 8` (least
//!   significant first) of byte `i / 8`, as the engine's bit reader reads
//!   it; bits past `payload_bits` are zero. Together these keep the payload
//!   bit-exact.
//!
//! The file holds raw client traffic: keep it private (mode 0600, under the
//! case's `work/` directory) and never commit it.
use std::{collections::BTreeMap, fs::File, io::Write};

use serde::Serialize;

use crate::experimental::packet::Bunch;

/// Rows kept per file.
pub const MAX_OBSERVATIONS: usize = 8192;
/// Payload bytes kept per row; larger bunches are counted as `oversize`.
pub const MAX_PAYLOAD_BYTES: usize = 8 * 1024;
/// Encoded bytes (metadata, hex and newlines) written per file.
pub const MAX_OUTPUT_BYTES: usize = 16 * 1024 * 1024;
/// Pawn bunches kept unconditionally at the start of each connection.
pub const KEEP_FIRST: u64 = 512;
/// Leading payload bits compared by the change rule.
pub const LEADING_BITS: usize = 32;
/// One bunch is kept at least this often per connection, changed or not.
pub const KEEP_INTERVAL_MS: u64 = 10_000;

/// Public counts; no payload, identity or address.
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq, Serialize)]
pub struct PawnBunchCaptureCounts {
    /// Pawn-channel bunches the tap saw.
    pub seen: u64,
    /// Rows written.
    pub observations: u64,
    pub kept_first: u64,
    pub kept_reliable: u64,
    pub kept_changed: u64,
    pub kept_interval: u64,
    /// Skipped by the sampling rule (by design; not a loss).
    pub sampled_out: u64,
    /// Selected but refused by the row or output-byte caps.
    pub dropped: u64,
    /// Selected but over [`MAX_PAYLOAD_BYTES`].
    pub oversize: u64,
    pub retained_payload_bits: u64,
    pub output_bytes: u64,
    pub write_errors: u64,
    /// 1 until finished, or forever after a drop, oversize or write error.
    pub incomplete: u64,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct SampleKey {
    flags: u16,
    bits: usize,
    leading: u64,
}

#[derive(Default)]
struct ConnectionState {
    seen: u64,
    last: Option<(SampleKey, u64)>,
}

/// Concrete file sink: callers cannot install an observer callback.
pub struct PrivatePawnBunchCapture {
    file: File,
    counts: PawnBunchCaptureCounts,
    connections: BTreeMap<u32, ConnectionState>,
    failed: bool,
    finished: bool,
}

impl std::fmt::Debug for PrivatePawnBunchCapture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PrivatePawnBunchCapture")
            .field("counts", &self.counts())
            .finish()
    }
}

/// When the session handed the bunch on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PawnBunchPhase {
    ReliableQueueRelease,
    UnreliableArrival,
}

/// Borrowed selection, valid for one receive call.
pub(crate) struct PawnBunchTap<'a> {
    pub sink: &'a mut PrivatePawnBunchCapture,
    pub channel: u32,
    pub connection_ordinal: u32,
    pub elapsed_ms: u64,
    pub unix_ms: u64,
}

#[derive(Serialize)]
struct Record {
    format: u8,
    selection: &'static str,
    ordinal: u64,
    keep_reason: &'static str,
    connection_ordinal: u32,
    connection_seen: u64,
    channel: u32,
    observation_phase: &'static str,
    elapsed_ms: u64,
    unix_ms: u64,
    packet_sequence: u16,
    reliable: bool,
    reliable_sequence: Option<i64>,
    reliable_sequence_raw: Option<u32>,
    partial: bool,
    partial_initial: bool,
    partial_final: bool,
    open: bool,
    close: bool,
    paused: bool,
    has_package_map_exports: bool,
    has_must_be_mapped_guids: bool,
    payload_bits: usize,
    payload_hex: String,
}

impl PrivatePawnBunchCapture {
    /// The example must exclusively create a confined mode-0600 file before
    /// startup; kept separate so tests can supply authored files.
    pub fn from_file(file: File) -> Self {
        Self {
            file,
            counts: PawnBunchCaptureCounts::default(),
            connections: BTreeMap::new(),
            failed: false,
            finished: false,
        }
    }

    pub fn counts(&self) -> PawnBunchCaptureCounts {
        PawnBunchCaptureCounts {
            incomplete: u64::from(
                !self.finished
                    || self.failed
                    || self.counts.dropped > 0
                    || self.counts.oversize > 0,
            ),
            ..self.counts
        }
    }

    /// A successful sync is required before reporting completion.
    pub fn finish(&mut self) -> PawnBunchCaptureCounts {
        if !self.finished {
            if !self.failed && self.file.sync_all().is_err() {
                self.write_failed();
            }
            self.finished = true;
        }
        self.counts()
    }

    fn write_failed(&mut self) {
        self.counts.write_errors = self.counts.write_errors.saturating_add(1);
        self.failed = true;
    }

    /// Session removal: its sampling state goes (bounded by live sessions).
    pub(crate) fn forget_connection(&mut self, connection: u32) {
        self.connections.remove(&connection);
    }
}

fn flags(bunch: &Bunch) -> u16 {
    let h = &bunch.header;
    [
        h.reliable,
        h.partial,
        h.partial_initial,
        h.partial_final,
        h.open,
        h.close,
        h.paused,
        h.has_package_map_exports,
        h.has_must_be_mapped_guids,
    ]
    .iter()
    .enumerate()
    .fold(0, |acc, (bit, set)| acc | (u16::from(*set) << bit))
}

/// Masked wire-order hex: bits past `bit_len` are zero.
fn payload_hex(bytes: &[u8], bit_len: usize) -> String {
    use std::fmt::Write;
    let used = bit_len.div_ceil(8);
    let mut hex = String::with_capacity(used * 2);
    for (index, byte) in bytes.iter().take(used).copied().enumerate() {
        let tail = bit_len % 8;
        let byte = if tail != 0 && index + 1 == used {
            byte & ((1 << tail) - 1)
        } else {
            byte
        };
        let _ = write!(&mut hex, "{byte:02x}");
    }
    hex
}

fn leading(bytes: &[u8], bit_len: usize) -> u64 {
    let bits = bit_len.min(LEADING_BITS);
    let mut value = 0u64;
    for bit in 0..bits {
        if bytes
            .get(bit / 8)
            .is_some_and(|byte| byte >> (bit % 8) & 1 == 1)
        {
            value |= 1 << bit;
        }
    }
    value
}

impl PawnBunchTap<'_> {
    /// Observe one wire bunch the session is about to handle. Read-only
    /// with respect to the bunch and the session.
    pub(crate) fn observe(
        &mut self,
        bunch: &Bunch,
        phase: PawnBunchPhase,
        packet_sequence: u16,
        reliable_sequence: Option<i64>,
    ) {
        if bunch.header.channel_index != self.channel || self.sink.failed || self.sink.finished {
            return;
        }
        let sink = &mut *self.sink;
        sink.counts.seen = sink.counts.seen.saturating_add(1);
        let state = sink.connections.entry(self.connection_ordinal).or_default();
        state.seen = state.seen.saturating_add(1);
        let connection_seen = state.seen;
        let bit_len = bunch.payload.bit_len();
        let bytes = bunch.payload.bytes();
        let key = SampleKey {
            flags: flags(bunch),
            bits: bit_len,
            leading: leading(bytes, bit_len),
        };
        let reason = if connection_seen <= KEEP_FIRST {
            "first"
        } else if bunch.header.reliable {
            "reliable"
        } else if state.last.is_none_or(|(last, _)| last != key) {
            "changed"
        } else if state
            .last
            .is_none_or(|(_, at)| self.elapsed_ms.saturating_sub(at) >= KEEP_INTERVAL_MS)
        {
            "interval"
        } else {
            sink.counts.sampled_out = sink.counts.sampled_out.saturating_add(1);
            return;
        };
        if bit_len.div_ceil(8) > MAX_PAYLOAD_BYTES {
            sink.counts.oversize = sink.counts.oversize.saturating_add(1);
            return;
        }
        if sink.counts.observations >= MAX_OBSERVATIONS as u64 {
            sink.counts.dropped = sink.counts.dropped.saturating_add(1);
            return;
        }
        let h = &bunch.header;
        let record = Record {
            format: 1,
            selection: "pawn_channel_bunches",
            ordinal: sink.counts.observations + 1,
            keep_reason: reason,
            connection_ordinal: self.connection_ordinal,
            connection_seen,
            channel: self.channel,
            observation_phase: match phase {
                PawnBunchPhase::ReliableQueueRelease => "reliable_queue_release",
                PawnBunchPhase::UnreliableArrival => "unreliable_arrival",
            },
            elapsed_ms: self.elapsed_ms,
            unix_ms: self.unix_ms,
            packet_sequence,
            reliable: h.reliable,
            reliable_sequence: reliable_sequence.filter(|_| h.reliable),
            reliable_sequence_raw: h.reliable_sequence_raw,
            partial: h.partial,
            partial_initial: h.partial_initial,
            partial_final: h.partial_final,
            open: h.open,
            close: h.close,
            paused: h.paused,
            has_package_map_exports: h.has_package_map_exports,
            has_must_be_mapped_guids: h.has_must_be_mapped_guids,
            payload_bits: bit_len,
            payload_hex: payload_hex(bytes, bit_len),
        };
        let Ok(mut encoded) = serde_json::to_vec(&record) else {
            sink.write_failed();
            return;
        };
        encoded.push(b'\n');
        if sink
            .counts
            .output_bytes
            .saturating_add(encoded.len() as u64)
            > MAX_OUTPUT_BYTES as u64
        {
            sink.counts.dropped = sink.counts.dropped.saturating_add(1);
            return;
        }
        // Charge every byte actually written, including a failed partial row.
        let mut remaining = encoded.as_slice();
        while !remaining.is_empty() {
            match sink.file.write(remaining) {
                Ok(0) => {
                    sink.write_failed();
                    return;
                }
                Ok(n) => {
                    sink.counts.output_bytes += n as u64;
                    remaining = &remaining[n..];
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(_) => {
                    sink.write_failed();
                    return;
                }
            }
        }
        sink.counts.observations += 1;
        sink.counts.retained_payload_bits += bit_len as u64;
        let counter = match reason {
            "first" => &mut sink.counts.kept_first,
            "reliable" => &mut sink.counts.kept_reliable,
            "changed" => &mut sink.counts.kept_changed,
            _ => &mut sink.counts.kept_interval,
        };
        *counter += 1;
        if let Some(state) = sink.connections.get_mut(&self.connection_ordinal) {
            state.last = Some((key, self.elapsed_ms));
        }
    }
}

#[cfg(test)]
mod tests;

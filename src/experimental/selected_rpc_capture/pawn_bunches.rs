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
//! # Sampling (per connection) and budgets (per file)
//!
//! Losing the one report the run exists for to a cap is the failure this is
//! designed against, so frequent traffic cannot crowd out a reliable bunch or
//! a rare unreliable shape:
//!
//! - **Reliable**: every reliable bunch is kept, in its own pool.
//! - **First**: the connection's first [`KEEP_FIRST`] pawn bunches, while
//!   the first pool has room (it holds two connections' worth: a join and
//!   one retry). An unreliable bunch that finds it full falls back to the
//!   shape rules below and is counted in `first_pool_full`; a reliable one
//!   is kept regardless, in the reliable pool.
//! - **Shapes**: an unreliable bunch's shape is its header flags, bit length
//!   and first [`LEADING_BITS`] payload bits. Each connection tracks up to
//!   [`MAX_SHAPES`] shapes and keeps the first [`SHAPE_FIRST`] bunches of each
//!   (`new_shape`), then at most one per shape interval (`interval`), the
//!   longer of [`KEEP_INTERVAL_MS`] and [`SHAPE_INTERVAL_STEP_MS`] per tracked
//!   shape, so many shapes share a bounded interval rate. Bunches of a shape
//!   beyond the bound are counted (`untracked_shape_bunches`) and share one
//!   connection-wide [`KEEP_INTERVAL_MS`] interval; they are never dropped
//!   uncounted.
//! - Everything else is `sampled_out`: by design, not a loss.
//!
//! Each kept row is charged to one pool ([`POOLS`]: reliable, first,
//! new shape, interval; reliable bunches always to the reliable pool). Pools
//! partition [`MAX_OBSERVATIONS`] rows and [`MAX_OUTPUT_BYTES`], so no pool
//! can consume another's share. A row refused by the reliable, new-shape or
//! interval pool, or by [`MAX_PAYLOAD_BYTES`], is a loss: it is counted in
//! `lost_reliable`, `lost_new_shape`, `lost_interval` or `oversize` and in
//! `lost_to_cap`, live, on every counts line. A first-pool refusal is not a
//! loss (it falls back, as above), so reconnects cannot raise a false alarm.
//! A run monitor alarms on `lost_to_cap > 0`; sampling, including
//! `first_pool_full`, never raises it.
//!
//! # Record format (JSONL, `format` 2)
//!
//! One JSON object and `\n` per retained wire bunch; partial bunches are kept
//! part by part (never reassembled), so an offline reader reassembles them
//! from the flags and reliable sequences exactly as the receiver would.
//! Format 1 (never released) had `keep_reason` `changed` and no `shape_seen`.
//!
//! - `format` (2), `selection` (`"pawn_channel_bunches"`), `ordinal` (1-based
//!   row number in this file).
//! - `keep_reason`: `first`, `reliable`, `new_shape` or `interval` (see
//!   Sampling; a reliable bunch within [`KEEP_FIRST`] is `first`).
//! - `connection_ordinal` (the session's ordinal, never an address or
//!   identity), `connection_seen` (1-based count of this connection's pawn
//!   bunches seen, kept or not: gaps between rows show how many were
//!   sampled out), `shape_seen` (1-based count of this connection's bunches
//!   of this unreliable shape; null for reliable bunches and untracked
//!   shapes), `channel`.
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

/// Rows kept per file (the sum of the pools' rows).
pub const MAX_OBSERVATIONS: usize = 8192;
/// Payload bytes kept per row; larger bunches are counted as `oversize`.
pub const MAX_PAYLOAD_BYTES: usize = 8 * 1024;
/// Encoded bytes (metadata, hex and newlines) written per file (the sum of
/// the pools' bytes).
pub const MAX_OUTPUT_BYTES: usize = 16 * 1024 * 1024;
/// Pawn bunches kept at the start of each connection while the first pool
/// has room.
pub const KEEP_FIRST: u64 = 512;
/// Leading payload bits in an unreliable bunch's shape.
pub const LEADING_BITS: usize = 32;
/// Bunches of each tracked unreliable shape kept unconditionally.
pub const SHAPE_FIRST: u64 = 4;
/// Unreliable shapes tracked per connection.
pub const MAX_SHAPES: usize = 256;
/// Shortest interval between kept bunches of one shape.
pub const KEEP_INTERVAL_MS: u64 = 10_000;
/// Per tracked shape, added to each shape's interval (2 s x 200 shapes =
/// 400 s each, so 0.5 interval rows per second in total).
pub const SHAPE_INTERVAL_STEP_MS: u64 = 2_000;

/// A row budget: one pool's share of the file's rows and bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PoolBudget {
    pub rows: u64,
    pub bytes: u64,
}

/// Budgets indexed by [`Pool`]: reliable, first, new shape, interval. The
/// first pool holds two connections' [`KEEP_FIRST`] rows (a join and one
/// retry); later starts are sampled, so it need not grow with reconnects.
pub const POOLS: [PoolBudget; 4] = [
    PoolBudget {
        rows: 2048,
        bytes: 4 * 1024 * 1024,
    },
    PoolBudget {
        rows: 1024,
        bytes: 2 * 1024 * 1024,
    },
    PoolBudget {
        rows: 2048,
        bytes: 4 * 1024 * 1024,
    },
    PoolBudget {
        rows: 3072,
        bytes: 6 * 1024 * 1024,
    },
];

// The pools partition the file's caps exactly.
const _: () = {
    let mut rows = 0;
    let mut bytes = 0;
    let mut index = 0;
    while index < POOLS.len() {
        rows += POOLS[index].rows;
        bytes += POOLS[index].bytes;
        index += 1;
    }
    assert!(rows == MAX_OBSERVATIONS as u64 && bytes == MAX_OUTPUT_BYTES as u64);
};

/// Public counts; no payload, identity or address. Live while the run goes.
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq, Serialize)]
pub struct PawnBunchCaptureCounts {
    /// Pawn-channel bunches the tap saw.
    pub seen: u64,
    /// Rows written.
    pub observations: u64,
    pub kept_first: u64,
    pub kept_reliable: u64,
    pub kept_new_shape: u64,
    pub kept_interval: u64,
    /// Skipped by the sampling rule (by design; not a loss).
    pub sampled_out: u64,
    /// Unreliable bunches within [`KEEP_FIRST`] that found the first pool
    /// full and fell back to the shape rules (then kept as `new_shape` or
    /// `interval`, or `sampled_out`). Not a loss: a later connection's
    /// start (a reconnect) is sampled like any other traffic.
    pub first_pool_full: u64,
    /// Most unreliable shapes tracked by one connection.
    pub shapes_tracked_max: u64,
    /// Unreliable bunches whose shape was beyond [`MAX_SHAPES`]; sampled by
    /// the connection-wide interval instead.
    pub untracked_shape_bunches: u64,
    /// Every selected row lost to a cap: `lost_reliable`, `lost_new_shape`,
    /// `lost_interval` and `oversize`. The field to alarm on.
    pub lost_to_cap: u64,
    pub lost_reliable: u64,
    pub lost_new_shape: u64,
    pub lost_interval: u64,
    /// Selected but over [`MAX_PAYLOAD_BYTES`].
    pub oversize: u64,
    pub retained_payload_bits: u64,
    pub output_bytes: u64,
    pub write_errors: u64,
    /// 1 until finished, or forever after a loss to a cap or a write error.
    pub incomplete: u64,
}

/// Why a row was kept.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum KeepReason {
    First,
    Reliable,
    NewShape,
    Interval,
}

impl KeepReason {
    fn as_str(self) -> &'static str {
        match self {
            Self::First => "first",
            Self::Reliable => "reliable",
            Self::NewShape => "new_shape",
            Self::Interval => "interval",
        }
    }
}

/// Which share of the file a row is charged to; indexes [`POOLS`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Pool {
    Reliable,
    First,
    NewShape,
    Interval,
}

impl Pool {
    fn of(reliable: bool, reason: KeepReason) -> Self {
        match (reliable, reason) {
            (true, _) | (false, KeepReason::Reliable) => Self::Reliable,
            (false, KeepReason::First) => Self::First,
            (false, KeepReason::NewShape) => Self::NewShape,
            (false, KeepReason::Interval) => Self::Interval,
        }
    }

    /// The loss a refusal by this pool counts as; `None` for the first
    /// pool, whose refusals fall back to the shape rules instead.
    fn loss(self) -> Option<Loss> {
        match self {
            Self::Reliable => Some(Loss::Reliable),
            Self::First => None,
            Self::NewShape => Some(Loss::NewShape),
            Self::Interval => Some(Loss::Interval),
        }
    }
}

/// A selected row lost to a cap.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Loss {
    Oversize,
    Reliable,
    NewShape,
    Interval,
}

/// Whose interval clock a selected row restarts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mark {
    None,
    Shape(Shape),
    Untracked,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Shape {
    flags: u16,
    bits: usize,
    leading: u64,
}

#[derive(Default)]
struct ShapeState {
    seen: u64,
    last_selected_ms: Option<u64>,
}

#[derive(Default)]
struct ConnectionState {
    seen: u64,
    shapes: BTreeMap<Shape, ShapeState>,
    untracked_last_selected_ms: Option<u64>,
}

fn due(last: Option<u64>, now: u64, interval: u64) -> bool {
    last.is_none_or(|at| now.saturating_sub(at) >= interval)
}

/// One sampling decision. `first` asks for the first pool (unreliable
/// bunches only); `reason` is the ordinary rule's choice, used when `first`
/// is false or the first pool is full; `None` means sampled out.
struct Selection {
    first: bool,
    reason: Option<KeepReason>,
    mark: Mark,
    shape_seen: Option<u64>,
    untracked: bool,
    shapes_tracked: usize,
}

impl ConnectionState {
    /// Counts the bunch and decides; interval clocks move only in [`mark`].
    ///
    /// [`mark`]: Self::mark
    fn select(&mut self, reliable: bool, shape: Shape, now: u64) -> Selection {
        self.seen = self.seen.saturating_add(1);
        let first = self.seen <= KEEP_FIRST;
        if reliable {
            // Always kept, in the reliable pool; `first` only names it.
            return Selection {
                first: false,
                reason: Some(if first {
                    KeepReason::First
                } else {
                    KeepReason::Reliable
                }),
                mark: Mark::None,
                shape_seen: None,
                untracked: false,
                shapes_tracked: self.shapes.len(),
            };
        }
        if !self.shapes.contains_key(&shape) && self.shapes.len() >= MAX_SHAPES {
            return Selection {
                first,
                reason: due(self.untracked_last_selected_ms, now, KEEP_INTERVAL_MS)
                    .then_some(KeepReason::Interval),
                mark: Mark::Untracked,
                shape_seen: None,
                untracked: true,
                shapes_tracked: self.shapes.len(),
            };
        }
        let interval = KEEP_INTERVAL_MS
            .max(SHAPE_INTERVAL_STEP_MS.saturating_mul(self.shapes.len().max(1) as u64));
        let state = self.shapes.entry(shape).or_default();
        state.seen = state.seen.saturating_add(1);
        let reason = if state.seen <= SHAPE_FIRST {
            Some(KeepReason::NewShape)
        } else if due(state.last_selected_ms, now, interval) {
            Some(KeepReason::Interval)
        } else {
            None
        };
        Selection {
            first,
            reason,
            mark: Mark::Shape(shape),
            shape_seen: Some(state.seen),
            untracked: false,
            shapes_tracked: self.shapes.len(),
        }
    }

    /// A row was selected (kept or lost to a cap): restart its interval, so
    /// a full pool costs one counted loss per interval, not one per bunch.
    fn mark(&mut self, mark: Mark, now: u64) {
        match mark {
            Mark::None => {}
            Mark::Untracked => self.untracked_last_selected_ms = Some(now),
            Mark::Shape(shape) => {
                if let Some(state) = self.shapes.get_mut(&shape) {
                    state.last_selected_ms = Some(now);
                }
            }
        }
    }
}

/// Concrete file sink: callers cannot install an observer callback.
pub struct PrivatePawnBunchCapture {
    file: File,
    counts: PawnBunchCaptureCounts,
    used: [PoolBudget; 4],
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
struct Record<'a> {
    format: u8,
    selection: &'static str,
    ordinal: u64,
    keep_reason: &'static str,
    connection_ordinal: u32,
    connection_seen: u64,
    shape_seen: Option<u64>,
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
    payload_hex: &'a str,
}

/// Outcome of [`PrivatePawnBunchCapture::write_row`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RowWrite {
    Written,
    NoRoom,
    Failed,
}

impl PrivatePawnBunchCapture {
    /// The example must exclusively create a confined mode-0600 file before
    /// startup; kept separate so tests can supply authored files.
    pub fn from_file(file: File) -> Self {
        Self {
            file,
            counts: PawnBunchCaptureCounts::default(),
            used: [PoolBudget { rows: 0, bytes: 0 }; 4],
            connections: BTreeMap::new(),
            failed: false,
            finished: false,
        }
    }

    pub fn counts(&self) -> PawnBunchCaptureCounts {
        PawnBunchCaptureCounts {
            incomplete: u64::from(!self.finished || self.failed || self.counts.lost_to_cap > 0),
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

    fn lost(&mut self, loss: Loss) {
        let counts = &mut self.counts;
        let counter = match loss {
            Loss::Oversize => &mut counts.oversize,
            Loss::Reliable => &mut counts.lost_reliable,
            Loss::NewShape => &mut counts.lost_new_shape,
            Loss::Interval => &mut counts.lost_interval,
        };
        *counter = counter.saturating_add(1);
        counts.lost_to_cap = counts.lost_to_cap.saturating_add(1);
    }

    fn mark(&mut self, connection: u32, mark: Mark, now: u64) {
        if let Some(state) = self.connections.get_mut(&connection) {
            state.mark(mark, now);
        }
    }

    /// Writes one row to `pool` if it has room; I/O failures disable
    /// capture.
    fn write_row(&mut self, pool: Pool, record: &Record<'_>, bit_len: usize) -> RowWrite {
        let budget = POOLS[pool as usize];
        if self.used[pool as usize].rows >= budget.rows {
            return RowWrite::NoRoom;
        }
        let Ok(mut encoded) = serde_json::to_vec(record) else {
            self.write_failed();
            return RowWrite::Failed;
        };
        encoded.push(b'\n');
        if self.used[pool as usize]
            .bytes
            .saturating_add(encoded.len() as u64)
            > budget.bytes
        {
            return RowWrite::NoRoom;
        }
        // Charge every byte actually written, including a failed partial row.
        let mut remaining = encoded.as_slice();
        while !remaining.is_empty() {
            match self.file.write(remaining) {
                Ok(0) => {
                    self.write_failed();
                    return RowWrite::Failed;
                }
                Ok(n) => {
                    self.counts.output_bytes += n as u64;
                    self.used[pool as usize].bytes += n as u64;
                    remaining = &remaining[n..];
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(_) => {
                    self.write_failed();
                    return RowWrite::Failed;
                }
            }
        }
        self.used[pool as usize].rows += 1;
        self.counts.observations += 1;
        self.counts.retained_payload_bits += bit_len as u64;
        RowWrite::Written
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
        let bit_len = bunch.payload.bit_len();
        let bytes = bunch.payload.bytes();
        let shape = Shape {
            flags: flags(bunch),
            bits: bit_len,
            leading: leading(bytes, bit_len),
        };
        let state = sink.connections.entry(self.connection_ordinal).or_default();
        let selection = state.select(bunch.header.reliable, shape, self.elapsed_ms);
        let connection_seen = state.seen;
        let counts = &mut sink.counts;
        counts.shapes_tracked_max = counts
            .shapes_tracked_max
            .max(selection.shapes_tracked as u64);
        if selection.untracked {
            counts.untracked_shape_bunches = counts.untracked_shape_bunches.saturating_add(1);
        }
        if !selection.first && selection.reason.is_none() {
            counts.sampled_out = counts.sampled_out.saturating_add(1);
            return;
        }
        let connection = self.connection_ordinal;
        let reliable = bunch.header.reliable;
        if bit_len.div_ceil(8) > MAX_PAYLOAD_BYTES {
            sink.mark(connection, selection.mark, self.elapsed_ms);
            sink.lost(Loss::Oversize);
            return;
        }
        let hex = payload_hex(bytes, bit_len);
        let mut record = self.record(bunch, phase, packet_sequence, reliable_sequence, &hex);
        record.connection_seen = connection_seen;
        record.shape_seen = selection.shape_seen;
        let sink = &mut *self.sink;
        if selection.first {
            record.keep_reason = KeepReason::First.as_str();
            record.ordinal = sink.counts.observations + 1;
            match sink.write_row(Pool::First, &record, bit_len) {
                RowWrite::Written => {
                    sink.mark(connection, selection.mark, self.elapsed_ms);
                    sink.counts.kept_first += 1;
                    return;
                }
                RowWrite::Failed => return,
                // Not a loss: fall back to the shape rules below.
                RowWrite::NoRoom => {
                    sink.counts.first_pool_full = sink.counts.first_pool_full.saturating_add(1);
                }
            }
        }
        let Some(reason) = selection.reason else {
            sink.counts.sampled_out = sink.counts.sampled_out.saturating_add(1);
            return;
        };
        sink.mark(connection, selection.mark, self.elapsed_ms);
        let pool = Pool::of(reliable, reason);
        record.keep_reason = reason.as_str();
        record.ordinal = sink.counts.observations + 1;
        match sink.write_row(pool, &record, bit_len) {
            RowWrite::Written => {
                let counter = match reason {
                    KeepReason::First => &mut sink.counts.kept_first,
                    KeepReason::Reliable => &mut sink.counts.kept_reliable,
                    KeepReason::NewShape => &mut sink.counts.kept_new_shape,
                    KeepReason::Interval => &mut sink.counts.kept_interval,
                };
                *counter += 1;
            }
            RowWrite::Failed => {}
            RowWrite::NoRoom => {
                if let Some(loss) = pool.loss() {
                    sink.lost(loss);
                }
            }
        }
    }

    /// The row for `bunch`; `keep_reason`, `ordinal` and the seen counts are
    /// filled in by the caller.
    fn record<'h>(
        &self,
        bunch: &Bunch,
        phase: PawnBunchPhase,
        packet_sequence: u16,
        reliable_sequence: Option<i64>,
        payload_hex: &'h str,
    ) -> Record<'h> {
        let h = &bunch.header;
        Record {
            format: 2,
            selection: "pawn_channel_bunches",
            ordinal: 0,
            keep_reason: "",
            connection_ordinal: self.connection_ordinal,
            connection_seen: 0,
            shape_seen: None,
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
            payload_bits: bunch.payload.bit_len(),
            payload_hex,
        }
    }
}

#[cfg(test)]
mod tests;

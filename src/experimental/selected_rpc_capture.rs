//! Explicitly installed PRIVATE actor-field-82 observation. No RPC dispatch.
//!
//! Only the sink serializes raw values. Debug and public status expose counts.
//! Offsets are relative to the complete (possibly reassembled) bunch payload,
//! after transport exports, never offsets into an original datagram.
use std::{collections::BTreeMap, fs::File, io::Write};

use serde::Serialize;

use super::{
    actor_channel::{
        ActorChannelCodec, ActorChannelLimits, BlockTarget, read_client_content_block,
    },
    archive::ByteOrder,
    packet::Bunch,
    peer_lab::validate_controller_capture,
    rep_rpc::read_field_header,
    sequencing::{PartialBunchReassembler, PartialReceive},
};

pub const MAX_OBSERVATIONS: usize = 16;
pub const MAX_FIELD_BYTES: usize = 8 * 1024;
pub const MAX_OUTPUT_BYTES: usize = 256 * 1024;
/// Separate transient framing budget; no whole bunch is persisted.
const MAX_BUNCH_BYTES: usize = 16 * 1024;
const MAX_ASSEMBLIES: usize = 16;

#[derive(Clone, Copy, Default, Debug, Serialize)]
pub struct CaptureCounts {
    pub observations: u64,
    pub retained_payload_bits: u64,
    pub output_bytes: u64,
    pub dropped: u64,
    pub malformed: u64,
    pub write_errors: u64,
    pub pending_assemblies: u64,
    /// Count flag: 1 until finalized, or forever after loss/error.
    pub incomplete: u64,
}

struct Assembly {
    partial: PartialBunchReassembler,
    sequence: i64,
    retained_bits: usize,
}

/// Concrete file sink: callers cannot install an observer callback. I/O errors
/// only disable capture; they never propagate into network delivery.
pub struct PrivateRpc82Capture {
    file: File,
    counts: CaptureCounts,
    assemblies: BTreeMap<(u32, u32), Assembly>,
    failed: bool,
    finished: bool,
}

impl std::fmt::Debug for PrivateRpc82Capture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PrivateRpc82Capture")
            .field("counts", &self.counts())
            .finish()
    }
}

/// Borrowed selection, valid for one receive call. No identities are retained.
pub(crate) struct CaptureTap<'a> {
    pub sink: &'a mut PrivateRpc82Capture,
    pub channel: u32,
    pub connection_ordinal: u32,
    pub elapsed_ms: u64,
    pub engine_net_version: u32,
    pub order: ByteOrder,
    pub components: [Option<u32>; 3],
}

#[derive(Serialize)]
struct Record {
    format: u8,
    field_index: u32,
    offset_origin: &'static str,
    ordinal: u64,
    connection_ordinal: u32,
    channel: u32,
    elapsed_ms: u64,
    engine_net_version: u32,
    archive_byte_order: &'static str,
    reliable: bool,
    partial: bool,
    reassembled: bool,
    parts: usize,
    bunch_payload_bits: usize,
    block_offset_bits: usize,
    block_length_bits: usize,
    block_payload_offset_bits: usize,
    block_payload_bits: usize,
    field_offset_bits: usize,
    field_length_bits: usize,
    payload_offset_bits: usize,
    payload_bits: usize,
    payload_hex: String,
}

struct StagedRecord {
    encoded: Vec<u8>,
    payload_bits: usize,
}

impl PrivateRpc82Capture {
    /// The example must exclusively create a confined mode-0600 file before
    /// startup. Kept separate to allow authored write-failure tests.
    pub fn from_file(file: File) -> Self {
        Self {
            file,
            counts: CaptureCounts::default(),
            assemblies: BTreeMap::new(),
            failed: false,
            finished: false,
        }
    }

    pub fn counts(&self) -> CaptureCounts {
        CaptureCounts {
            pending_assemblies: self
                .assemblies
                .values()
                .filter(|s| s.partial.in_progress())
                .count() as u64,
            incomplete: u64::from(
                !self.finished
                    || self.failed
                    || self.counts.dropped > 0
                    || self.counts.malformed > 0
                    || self.assemblies.values().any(|s| s.partial.in_progress()),
            ),
            ..self.counts
        }
    }

    /// A successful sync is required before reporting capture completion.
    pub fn finish(&mut self) -> CaptureCounts {
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
        self.assemblies.clear();
    }

    fn drop_observation(&mut self) {
        self.counts.dropped = self.counts.dropped.saturating_add(1);
    }

    pub(crate) fn forget_connection(&mut self, connection: u32) {
        let mut lost = 0u64;
        self.assemblies.retain(|(ordinal, _), state| {
            if *ordinal == connection {
                lost += u64::from(state.partial.in_progress());
                false
            } else {
                true
            }
        });
        self.counts.dropped = self.counts.dropped.saturating_add(lost);
    }
}

impl CaptureTap<'_> {
    /// Called only after the reliable queue releases a whole bunch/part in
    /// channel order. Ordinary transport rejection/dispatch stays unchanged.
    pub(crate) fn observe(&mut self, bunch: &Bunch) {
        if bunch.header.channel_index != self.channel || self.sink.failed || self.sink.finished {
            return;
        }
        let h = &bunch.header;
        if !h.reliable
            || h.has_package_map_exports
            || h.has_must_be_mapped_guids
            || h.open
            || h.close
            || h.paused
            || bunch.payload.bit_len() > MAX_BUNCH_BYTES * 8
        {
            self.sink.drop_observation();
            self.sink
                .assemblies
                .remove(&(self.connection_ordinal, self.channel));
            return;
        }
        let key = (self.connection_ordinal, self.channel);
        if h.partial {
            if !self.sink.assemblies.contains_key(&key)
                && self.sink.assemblies.len() >= MAX_ASSEMBLIES
            {
                self.sink.drop_observation();
                return;
            }
            let state = self.sink.assemblies.entry(key).or_insert_with(|| Assembly {
                partial: PartialBunchReassembler::new(),
                sequence: 0,
                retained_bits: 0,
            });
            if state.retained_bits.saturating_add(bunch.payload.bit_len()) > MAX_BUNCH_BYTES * 8 {
                self.sink.assemblies.remove(&key);
                self.sink.drop_observation();
                return;
            }
            state.retained_bits += bunch.payload.bit_len();
            // Ordered delivery already checked raw sequence wrap and duplicates.
            state.sequence += 1;
            let result = state.partial.receive_part(
                state.sequence,
                h,
                &bunch.payload,
                Some(MAX_BUNCH_BYTES),
            );
            match result {
                Ok(PartialReceive::Complete { bunch, parts, .. }) => {
                    self.sink.assemblies.remove(&key);
                    self.complete(&bunch, parts, true);
                }
                Ok(
                    PartialReceive::Incomplete { .. } | PartialReceive::IgnoredUnreliableInitial,
                ) => {}
                Err(_) => {
                    self.sink.assemblies.remove(&key);
                    self.sink.counts.malformed = self.sink.counts.malformed.saturating_add(1);
                }
            }
        } else {
            if self.sink.assemblies.remove(&key).is_some() {
                self.sink.drop_observation();
            }
            self.complete(bunch, 1, false);
        }
    }

    fn complete(&mut self, bunch: &Bunch, parts: usize, partial: bool) {
        // Validate all blocks AND recognized fields before publishing any prefix.
        // The validator stages calls and discards them: capture grants no dispatch.
        if !validate_controller_capture(
            &bunch.payload,
            self.engine_net_version,
            self.order,
            self.components,
        ) {
            self.sink.counts.malformed = self.sink.counts.malformed.saturating_add(1);
            return;
        }
        let Ok(records) = self.stage(bunch, parts, partial) else {
            self.sink.counts.malformed = self.sink.counts.malformed.saturating_add(1);
            return;
        };
        for record in records {
            if self.sink.counts.observations >= MAX_OBSERVATIONS as u64 {
                self.sink.drop_observation();
                continue;
            }
            let bits = record.payload_bits as u64;
            // Charge every byte actually written, including a failed partial row.
            let mut remaining = record.encoded.as_slice();
            while !remaining.is_empty() {
                match self.sink.file.write(remaining) {
                    Ok(0) => {
                        self.sink.write_failed();
                        break;
                    }
                    Ok(n) => {
                        self.sink.counts.output_bytes += n as u64;
                        remaining = &remaining[n..];
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(_) => {
                        self.sink.write_failed();
                        break;
                    }
                }
            }
            if self.sink.failed {
                break;
            }
            self.sink.counts.observations += 1;
            self.sink.counts.retained_payload_bits += bits;
        }
    }

    fn stage(
        &mut self,
        bunch: &Bunch,
        parts: usize,
        partial: bool,
    ) -> anyhow::Result<Vec<StagedRecord>> {
        let codec = ActorChannelCodec {
            engine_net_version: self.engine_net_version,
            exports_enabled: false,
            has_level: true,
            order: self.order,
            limits: ActorChannelLimits::default(),
        };
        let mut reader = bunch.payload.reader();
        let mut records = Vec::new();
        // Exact retained encoding budget, including hex expansion, metadata and
        // newline. No staged prefix is written until the entire scan succeeds.
        let mut remaining_bits = (MAX_OUTPUT_BYTES as u64 - self.sink.counts.output_bytes) * 8;
        while !reader.at_end() {
            let block_start = reader.position();
            let block = read_client_content_block(&mut reader, &codec)?;
            let block_end = reader.position();
            let block_payload_start = block_end - block.payload.bit_len();
            if !matches!(block.target, BlockTarget::Actor) {
                continue;
            }
            let mut fields = block.payload.reader();
            while !fields.at_end() {
                let field_start = fields.position();
                let field = read_field_header(
                    &mut fields,
                    super::rep_rpc::BP_ICARUS_PLAYER_CONTROLLER_SURVIVAL_FIELD_INDEX_BOUND,
                    codec.limits.max_payload_bits,
                )?;
                if field.index != 82 {
                    continue;
                }
                if field.payload.bit_len() > MAX_FIELD_BYTES * 8
                    || records.len()
                        >= MAX_OBSERVATIONS.saturating_sub(self.sink.counts.observations as usize)
                {
                    self.sink.drop_observation();
                    continue;
                }
                let mut hex = String::with_capacity(field.payload.bytes().len() * 2);
                use std::fmt::Write;
                for (index, byte) in field.payload.bytes().iter().copied().enumerate() {
                    let tail = field.payload.bit_len() % 8;
                    let byte = if tail != 0 && index + 1 == field.payload.bytes().len() {
                        byte & ((1 << tail) - 1)
                    } else {
                        byte
                    };
                    write!(&mut hex, "{byte:02x}")?;
                }
                let record = Record {
                    format: 1,
                    field_index: 82,
                    offset_origin: "complete_bunch_payload",
                    ordinal: self.sink.counts.observations + records.len() as u64 + 1,
                    connection_ordinal: self.connection_ordinal,
                    channel: self.channel,
                    elapsed_ms: self.elapsed_ms,
                    engine_net_version: self.engine_net_version,
                    archive_byte_order: match self.order {
                        ByteOrder::Little => "little",
                        ByteOrder::Big => "big",
                    },
                    reliable: bunch.header.reliable,
                    partial,
                    reassembled: partial,
                    parts,
                    bunch_payload_bits: bunch.payload.bit_len(),
                    block_offset_bits: block_start,
                    block_length_bits: block_end - block_start,
                    block_payload_offset_bits: block_payload_start,
                    block_payload_bits: block.payload.bit_len(),
                    field_offset_bits: block_payload_start + field_start,
                    field_length_bits: fields.position() - field_start,
                    payload_offset_bits: block_payload_start + fields.position()
                        - field.payload.bit_len(),
                    payload_bits: field.payload.bit_len(),
                    payload_hex: hex,
                };
                let mut encoded = serde_json::to_vec(&record)?;
                encoded.push(b'\n');
                let retained_bits = encoded.len() as u64 * 8;
                if retained_bits > remaining_bits {
                    self.sink.drop_observation();
                    continue;
                }
                remaining_bits -= retained_bits;
                records.push(StagedRecord {
                    encoded,
                    payload_bits: record.payload_bits,
                });
            }
        }
        Ok(records)
    }
}

#[cfg(test)]
mod tests;

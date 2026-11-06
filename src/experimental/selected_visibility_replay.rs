//! Offline bounded selected-field reader, with no filesystem or dispatch API.
//! Input is private; errors and serialized summaries contain fixed codes/counts
//! only. Caller must confine file descriptors before invoking this module.
//! Capture completeness and provenance always remain unverified.

use std::io::{Read, Write};

use serde::{Deserialize, Serialize};

use crate::experimental::{
    packet::{BitWriter, Payload},
    release::ReleaseProfile,
    selected_rpc_capture::{MAX_FIELD_BYTES, MAX_OBSERVATIONS, MAX_OUTPUT_BYTES},
    visibility::{
        DecodedVisibilityParameter, VisibilityArchiveProfile, VisibilityDecodeProfile,
        VisibilityRpc82Layout, parse_visibility_rpc82,
    },
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReplayError {
    Profile,
    Layout,
    Read,
    InputLimit,
    FinalNewline,
    BlankLine,
    RecordLimit,
    JsonSchema,
    Metadata,
    Ordinal,
    Offsets,
    Hex,
    Output,
}

/// Fixed safe diagnostics. Never retain or format input errors/raw names.
impl std::fmt::Display for ReplayError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for ReplayError {}

/// Requires explicit independent archive/layout assertions; no Default.
pub struct ReplayProfile {
    decode: VisibilityDecodeProfile,
}

impl ReplayProfile {
    pub fn qualify(
        release: &ReleaseProfile,
        archive: VisibilityArchiveProfile,
        layout: VisibilityRpc82Layout,
    ) -> Result<Self, ReplayError> {
        if layout != VisibilityRpc82Layout::NormalSingleCustomStruct {
            return Err(ReplayError::Layout);
        }
        let decode =
            VisibilityDecodeProfile::qualify(release, archive).map_err(|_| ReplayError::Profile)?;
        Ok(Self { decode })
    }
}

/// No Debug: never accidentally print retained payload hex or parser excerpts.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    format: u8,
    field_index: u32,
    offset_origin: String,
    ordinal: u64,
    connection_ordinal: u32,
    channel: u32,
    elapsed_ms: u64,
    engine_net_version: u32,
    archive_byte_order: String,
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

#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct ReplaySummary {
    pub format: u8,
    pub records: u64,
    pub accepted_present: u64,
    pub accepted_absent: u64,
    pub rejected: u64,
    pub capture_complete_verified: bool,
    pub replay_finished: bool,
    pub provenance_verified: bool,
    pub compatibility_proven: bool,
}

impl ReplaySummary {
    /// Zero decoder rejections means conditional subset acceptance only.
    pub fn all_decoded(&self) -> bool {
        self.rejected == 0
    }

    /// Only call after replay returns successfully. A write failure may leave
    /// partial bytes, so callers must never label its output report complete.
    pub fn write_json(&self, mut output: impl Write) -> Result<(), ReplayError> {
        serde_json::to_writer(&mut output, self).map_err(|_| ReplayError::Output)?;
        output.write_all(b"\n").map_err(|_| ReplayError::Output)?;
        output.flush().map_err(|_| ReplayError::Output)
    }
}

/// Whole-input admission before decoding/publishing any result. Read allocation
/// is capped at MAX_OUTPUT_BYTES+1; JSON/hex strings share that finite ceiling.
/// No EOF-derived source-completion claim and no fallback custom-only parser.
pub fn replay(input: impl Read, profile: &ReplayProfile) -> Result<ReplaySummary, ReplayError> {
    let mut encoded = Vec::with_capacity(MAX_OUTPUT_BYTES + 1);
    input
        .take((MAX_OUTPUT_BYTES + 1) as u64)
        .read_to_end(&mut encoded)
        .map_err(|_| ReplayError::Read)?;
    if encoded.len() > MAX_OUTPUT_BYTES {
        return Err(ReplayError::InputLimit);
    }
    if !encoded.is_empty() && !encoded.ends_with(b"\n") {
        return Err(ReplayError::FinalNewline);
    }
    let mut records = Vec::new();
    let mut remaining = encoded.as_slice();
    while !remaining.is_empty() {
        let end = remaining
            .iter()
            .position(|byte| *byte == b'\n')
            .ok_or(ReplayError::FinalNewline)?;
        let line = &remaining[..end];
        remaining = &remaining[end + 1..];
        if line.is_empty() {
            return Err(ReplayError::BlankLine);
        }
        if records.len() >= MAX_OBSERVATIONS {
            return Err(ReplayError::RecordLimit);
        }
        let record: Record = serde_json::from_slice(line).map_err(|_| ReplayError::JsonSchema)?;
        if record.ordinal != records.len() as u64 + 1 {
            return Err(ReplayError::Ordinal);
        }
        validate(&record)?;
        records.push(record);
    }
    let mut summary = ReplaySummary {
        format: 1,
        records: records.len() as u64,
        accepted_present: 0,
        accepted_absent: 0,
        rejected: 0,
        capture_complete_verified: false,
        replay_finished: true,
        provenance_verified: false,
        compatibility_proven: false,
    };
    for record in records {
        let payload = payload(&record);
        match parse_visibility_rpc82(
            &payload,
            &profile.decode,
            VisibilityRpc82Layout::NormalSingleCustomStruct,
        ) {
            Ok(DecodedVisibilityParameter::Absent) => summary.accepted_absent += 1,
            Ok(DecodedVisibilityParameter::Present(_)) => summary.accepted_present += 1,
            Err(_) => summary.rejected += 1,
        }
    }
    Ok(summary)
}

fn validate(record: &Record) -> Result<(), ReplayError> {
    if record.format != 1
        || record.field_index != 82
        || record.offset_origin != "complete_bunch_payload"
        || record.connection_ordinal == 0
        || record.engine_net_version != 17
        || record.archive_byte_order != "little"
        || !record.reliable
        || record.partial != record.reassembled
        || record.parts == 0
        || (!record.partial && record.parts != 1)
        || record.bunch_payload_bits > 16 * 1024 * 8
        || record.payload_bits > MAX_FIELD_BYTES * 8
    {
        return Err(ReplayError::Metadata);
    }
    // Neutral channel/time metadata have no stronger invariant in this writer.
    // Single-part partial reassembly is legal (initial && final in one part).
    let _ = (record.channel, record.elapsed_ms);
    let block_end = record
        .block_offset_bits
        .checked_add(record.block_length_bits)
        .ok_or(ReplayError::Offsets)?;
    let block_payload_end = record
        .block_payload_offset_bits
        .checked_add(record.block_payload_bits)
        .ok_or(ReplayError::Offsets)?;
    let field_end = record
        .field_offset_bits
        .checked_add(record.field_length_bits)
        .ok_or(ReplayError::Offsets)?;
    let payload_end = record
        .payload_offset_bits
        .checked_add(record.payload_bits)
        .ok_or(ReplayError::Offsets)?;
    if record.block_offset_bits > record.block_payload_offset_bits
        || block_end != block_payload_end
        || block_end > record.bunch_payload_bits
        || record.field_offset_bits < record.block_payload_offset_bits
        || record.field_offset_bits >= record.payload_offset_bits
        || field_end != payload_end
        || payload_end > block_payload_end
    {
        return Err(ReplayError::Offsets);
    }
    let bytes = record.payload_bits.div_ceil(8);
    if record.payload_hex.len() != bytes * 2
        || !record.payload_hex.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err(ReplayError::Hex);
    }
    if !record.payload_bits.is_multiple_of(8) {
        let last = record.payload_hex.as_bytes();
        let value = (nibble(last[last.len() - 2]) << 4) | nibble(last[last.len() - 1]);
        if value >> (record.payload_bits % 8) != 0 {
            return Err(ReplayError::Hex);
        }
    }
    Ok(())
}

fn nibble(byte: u8) -> u8 {
    match byte {
        b'0'..=b'9' => byte - b'0',
        b'a'..=b'f' => byte - b'a' + 10,
        b'A'..=b'F' => byte - b'A' + 10,
        _ => unreachable!("validated hex only"),
    }
}

fn payload(record: &Record) -> Payload {
    let mut writer = BitWriter::new();
    for (index, pair) in record
        .payload_hex
        .as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .enumerate()
    {
        let byte = (nibble(pair[0]) << 4) | nibble(pair[1]);
        let bits = (record.payload_bits - index * 8).min(8);
        for bit in 0..bits {
            writer.write_bit(byte & (1 << bit) != 0);
        }
    }
    Payload::from_writer(writer)
}

#[cfg(test)]
#[path = "selected_visibility_replay_tests.rs"]
mod tests;

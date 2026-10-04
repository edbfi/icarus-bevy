//! Experimental read-only envelope inspection, never world import.
//!
//! Recovered field names and algorithm observations are documented in
//! `docs/specs/prospect-envelope.md`. Payload serialization remains unknown.
use std::io::Read;

use anyhow::{Context, Result, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use flate2::{Decompress, FlushDecompress, Status};
use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};

/// Conservative research policy, not a recovered ICARUS format limit.
pub const MAX_ENVELOPE_BYTES: usize = 1024 * 1024;
pub const MAX_PAYLOAD_BYTES: usize = 16 * 1024 * 1024;

#[derive(Deserialize)]
struct Envelope {
    #[serde(rename = "ProspectBlob")]
    blob: Blob,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Blob {
    // Identity and metadata fields are deliberately not returned to the caller.
    hash: String,
    total_length: i32,
    data_length: i32,
    uncompressed_length: i32,
    binary_blob: String,
}

#[derive(Debug, Serialize)]
pub struct EnvelopeInspection {
    pub validation: &'static str,
    pub compressed_bytes: usize,
    pub payload_bytes: usize,
    /// Retained as an observation; its semantics are not established.
    pub declared_data_length: i32,
    pub legacy_sha1_matches: bool,
    pub world_imported: bool,
}

/// Inspect one bounded JSON envelope without extracting payload or mutating state.
///
/// Unknown JSON fields are skipped. Known fields must occur once and use the
/// expected types. Canonical padded Base64 and exactly one zlib stream are required
/// by this inspector's conservative policy; original tolerance is unverified.
pub fn inspect(reader: impl Read) -> Result<EnvelopeInspection> {
    let mut bytes = Vec::new();
    reader
        .take(MAX_ENVELOPE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() <= MAX_ENVELOPE_BYTES,
        "envelope exceeds 1 MiB research limit"
    );
    let envelope: Envelope =
        serde_json::from_slice(&bytes).context("invalid prospect envelope JSON")?;
    let blob = envelope.blob;
    ensure!(
        blob.total_length >= 0 && blob.data_length >= 0 && blob.uncompressed_length >= 0,
        "negative prospect length"
    );
    let expected = blob.uncompressed_length as usize;
    ensure!(
        expected <= MAX_PAYLOAD_BYTES,
        "payload exceeds 16 MiB research limit"
    );
    ensure!(
        blob.hash.len() == 40
            && blob
                .hash
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
        "expected lowercase 40-digit legacy SHA-1"
    );
    let compressed = STANDARD
        .decode(&blob.binary_blob)
        .context("invalid canonical Base64")?;
    ensure!(
        compressed.len() == blob.total_length as usize,
        "TotalLength differs from decoded byte count"
    );

    // One extra byte distinguishes an exact-length stream from excess output.
    // The caller's declared size cannot allocate beyond the fixed research cap.
    let mut output = vec![0_u8; expected + 1];
    let mut decoder = Decompress::new(true);
    let status = decoder
        .decompress(&compressed, &mut output, FlushDecompress::Finish)
        .context("invalid zlib payload")?;
    ensure!(
        status == Status::StreamEnd,
        "incomplete zlib stream or output exceeds declared length"
    );
    ensure!(
        decoder.total_in() == compressed.len() as u64,
        "trailing data after zlib stream"
    );
    ensure!(
        decoder.total_out() == expected as u64,
        "UncompressedLength differs from actual output"
    );
    output.truncate(expected);
    // SHA-1 is used only to inspect the legacy format, never for authentication.
    ensure!(
        format!("{:x}", Sha1::digest(&output)) == blob.hash,
        "legacy SHA-1 mismatch"
    );
    Ok(EnvelopeInspection {
        validation: "experimental_envelope_only",
        compressed_bytes: compressed.len(),
        payload_bytes: output.len(),
        declared_data_length: blob.data_length,
        legacy_sha1_matches: true,
        world_imported: false,
    })
}

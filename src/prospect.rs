//! Experimental read-only envelope inspection, never world import.
//!
//! Recovered field names and algorithm observations are documented in
//! `docs/specs/save-format/prospect-envelope.md`. Payload serialization remains unknown.
use std::io::Read;

use anyhow::{Context, Result, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use flate2::{Decompress, FlushDecompress, Status};
use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};

use crate::experimental::archive::{ByteOrder, decode_string};
use crate::experimental::component::{
    BaseComponent, MAX_BOUNDED_COMPONENT_BYTES, read_base_component_with_limit,
};
use crate::experimental::recorders::{RecorderSummary, read_recorders};
use crate::experimental::tagged::{RootTagSummary, TagProfile, read_root};
use crate::json::JsonObject;
use crate::preservation::{PreserveLimits, PreservedProspect};

/// Bounded research policy, not a recovered ICARUS format limit. Observed inputs
/// reached 2,239,534 raw bytes and 19,131,027 decompressed bytes.
pub const MAX_ENVELOPE_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_PAYLOAD_BYTES: usize = 32 * 1024 * 1024;

#[derive(Deserialize)]
struct Envelope {
    #[serde(rename = "ProspectBlob")]
    blob: JsonObject<Blob>,
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

struct ValidatedEnvelope {
    inspection: EnvelopeInspection,
    payload: Vec<u8>,
}

#[derive(Debug, Serialize)]
pub struct InnerTagInspection {
    pub validation: &'static str,
    pub envelope: EnvelopeInspection,
    pub inner_tags: RootTagSummary,
    pub world_imported: bool,
}

#[derive(Debug, Serialize)]
pub struct RecorderInspection {
    pub validation: &'static str,
    pub envelope: EnvelopeInspection,
    pub inner_tags: RootTagSummary,
    pub recorders: RecorderSummary,
    pub world_imported: bool,
}

/// Inspect one bounded JSON envelope without extracting payload or mutating state.
///
/// Unknown JSON fields are skipped. Known fields must occur once and use the
/// expected types. Canonical padded Base64 and exactly one zlib stream are required
/// by this inspector's conservative policy; original tolerance is unverified.
/// Envelopes may use UTF-8 or explicitly BOM-tagged UTF-16LE. Other encodings are
/// not guessed, and malformed UTF-16 is rejected without replacement characters.
pub fn inspect(reader: impl Read) -> Result<EnvelopeInspection> {
    Ok(decode_envelope(reader)?.inspection)
}

/// Opt-in root framing under an explicit research profile, after full envelope
/// integrity validation. Opaque decoded values are never returned or exported.
pub fn inspect_with_inner_tags(reader: impl Read) -> Result<InnerTagInspection> {
    let validated = decode_envelope(reader)?;
    let tags = read_root(&validated.payload, TagProfile::MemoryArchive522)
        .context("invalid experimental root tags")?;
    Ok(InnerTagInspection {
        validation: "experimental_root_tags_only",
        envelope: validated.inspection,
        inner_tags: tags.summary(),
        world_imported: false,
    })
}

/// Opt-in recorder framing, with component byte contents remaining opaque.
/// Reports aggregates only after all framing and envelope checks succeed.
pub fn inspect_with_recorders(reader: impl Read) -> Result<RecorderInspection> {
    let validated = decode_envelope(reader)?;
    let profile = TagProfile::MemoryArchive522;
    let tags = read_root(&validated.payload, profile).context("invalid experimental root tags")?;
    let recorders =
        read_recorders(&tags.tags, profile).context("invalid experimental recorder framing")?;
    Ok(RecorderInspection {
        validation: "experimental_recorder_framing_only",
        envelope: validated.inspection,
        inner_tags: tags.summary(),
        recorders: recorders.summary(),
        world_imported: false,
    })
}

/// Envelope observations plus a verified lossless preservation of its payload.
pub struct PreservedEnvelope {
    pub envelope: EnvelopeInspection,
    pub preserved: PreservedProspect,
}

/// Fully validate the envelope, then preserve its inner payload losslessly and
/// value-opaquely. The decoded payload itself is never returned; the preserved
/// structure reconstructs it byte-identically or construction fails.
pub fn preserve(reader: impl Read, limits: PreserveLimits) -> Result<PreservedEnvelope> {
    let validated = decode_envelope(reader)?;
    let preserved = PreservedProspect::from_inner_payload(&validated.payload, limits)
        .context("invalid experimental preservation")?;
    Ok(PreservedEnvelope {
        envelope: validated.inspection,
        preserved,
    })
}

/// One caller-selected recorder handed to a [`visit_class_components`] visitor.
///
/// `component` is the framing outcome as a value: `Ok` borrows the framed
/// component's exact source spans, and `Err` borrows the framing error. Both are
/// valid only for the duration of the call, so nothing escapes the visitor.
pub struct ComponentVisit<'a> {
    /// Zero-based position in the recorder array, counting every class.
    pub recorder_index: usize,
    /// Length of the recorder's opaque BinaryData bytes.
    pub binary_bytes: usize,
    pub component: std::result::Result<&'a BaseComponent<'a>, &'a anyhow::Error>,
}

/// Aggregate counts only; never the class string, field names or values.
#[derive(Debug, Serialize)]
pub struct ClassComponentReport {
    pub validation: &'static str,
    pub recorder_count: usize,
    pub matched_count: usize,
    pub framed_count: usize,
    pub failed_count: usize,
    pub world_imported: bool,
}

/// Explicitly experimental: after the same envelope, root-tag and recorder
/// validation as [`inspect_with_recorders`], frame the BinaryData of every
/// recorder whose decoded class string equals `class` exactly (UTF-16 unit
/// equality, array order) as a caller-selected base-component candidate under
/// `limit` bytes (1..=1 MiB, checked before any input is read). A framing
/// failure is passed to `visit` as a value and the walk continues; an `Err`
/// returned by `visit` aborts the walk and is returned unchanged. The visitor
/// receives borrowed spans and owns their privacy; the report holds counts only.
/// Matching a class string does not recognize the class or verify its schema.
pub fn visit_class_components(
    reader: impl Read,
    class: &str,
    limit: usize,
    mut visit: impl FnMut(ComponentVisit<'_>) -> Result<()>,
) -> Result<ClassComponentReport> {
    ensure!(
        (1..=MAX_BOUNDED_COMPONENT_BYTES).contains(&limit),
        "component byte limit must be between 1 byte and 1 MiB"
    );
    let validated = decode_envelope(reader)?;
    let profile = TagProfile::MemoryArchive522;
    let tags = read_root(&validated.payload, profile).context("invalid experimental root tags")?;
    let recorders =
        read_recorders(&tags.tags, profile).context("invalid experimental recorder framing")?;
    let mut report = ClassComponentReport {
        validation: "experimental_class_component_framing_only",
        recorder_count: recorders.records().len(),
        matched_count: 0,
        framed_count: 0,
        failed_count: 0,
        world_imported: false,
    };
    for (recorder_index, recorder) in recorders.records().iter().enumerate() {
        // read_recorders validated this exact FString; decoding cannot leak it.
        let decoded = decode_string(recorder.class_value, ByteOrder::Little)
            .context("invalid experimental recorder class string")?;
        if !decoded.units.iter().copied().eq(class.encode_utf16()) {
            continue;
        }
        report.matched_count += 1;
        let framed = read_base_component_with_limit(recorder.binary_data, profile, limit);
        match &framed {
            Ok(_) => report.framed_count += 1,
            Err(_) => report.failed_count += 1,
        }
        visit(ComponentVisit {
            recorder_index,
            binary_bytes: recorder.binary_data.len(),
            component: framed.as_ref(),
        })?;
    }
    Ok(report)
}

fn decode_envelope(reader: impl Read) -> Result<ValidatedEnvelope> {
    let mut bytes = Vec::new();
    reader
        .take(MAX_ENVELOPE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() <= MAX_ENVELOPE_BYTES,
        "envelope exceeds 4 MiB research limit"
    );
    let envelope: JsonObject<Envelope> = if let Some(encoded) = bytes.strip_prefix(&[0xff, 0xfe]) {
        ensure!(encoded.len().is_multiple_of(2), "odd UTF-16LE byte length");
        let units = encoded
            .as_chunks::<2>()
            .0
            .iter()
            .copied()
            .map(u16::from_le_bytes);
        let text = char::decode_utf16(units)
            .collect::<std::result::Result<String, _>>()
            .context("invalid UTF-16LE envelope")?;
        serde_json::from_str(&text)
    } else {
        // IgnoredAny skips unknown strings without validating their UTF-8 when
        // parsing bytes. Validate the whole document, including ignored metadata.
        let text = std::str::from_utf8(&bytes).context("invalid UTF-8 envelope")?;
        serde_json::from_str(text)
    }
    .map_err(|error| crate::json_parse_error("invalid prospect envelope JSON", error))?;
    let blob = envelope.0.blob.0;
    ensure!(
        blob.total_length >= 0 && blob.data_length >= 0 && blob.uncompressed_length >= 0,
        "negative prospect length"
    );
    let expected = blob.uncompressed_length as usize;
    ensure!(
        expected <= MAX_PAYLOAD_BYTES,
        "payload exceeds 32 MiB research limit"
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
        crate::hex_lower(&Sha1::digest(&output)) == blob.hash,
        "legacy SHA-1 mismatch"
    );
    Ok(ValidatedEnvelope {
        inspection: EnvelopeInspection {
            validation: "experimental_envelope_only",
            compressed_bytes: compressed.len(),
            payload_bytes: output.len(),
            declared_data_length: blob.data_length,
            legacy_sha1_matches: true,
            world_imported: false,
        },
        payload: output,
    })
}

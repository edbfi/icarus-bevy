//! Experimental read-only envelope inspection, never world import.
//!
//! Recovered field names and algorithm observations are documented in
//! `docs/specs/save-format/prospect-envelope.md`. Payload serialization remains unknown.
use std::{fmt, io::Read, marker::PhantomData};

use anyhow::{Context, Result, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use flate2::{Decompress, FlushDecompress, Status};
use serde::{
    Deserialize, Deserializer, Serialize,
    de::{MapAccess, Visitor, value::MapAccessDeserializer},
};
use sha1::{Digest, Sha1};

use crate::experimental::recorders::{RecorderSummary, read_recorders};
use crate::experimental::tagged::{RootTagSummary, TagProfile, read_root};

/// Bounded research policy, not a recovered ICARUS format limit. Observed inputs
/// reached 2,239,534 raw bytes and 19,131,027 decompressed bytes.
pub const MAX_ENVELOPE_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_PAYLOAD_BYTES: usize = 32 * 1024 * 1024;

#[derive(Deserialize)]
struct Envelope {
    #[serde(rename = "ProspectBlob")]
    blob: JsonObject<Blob>,
}

// Derived struct visitors also accept positional sequences. This envelope
// requires named JSON object fields at both the root and blob boundaries.
struct JsonObject<T>(T);

impl<'de, T: Deserialize<'de>> Deserialize<'de> for JsonObject<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        struct ObjectVisitor<T>(PhantomData<T>);

        impl<'de, T: Deserialize<'de>> Visitor<'de> for ObjectVisitor<T> {
            type Value = JsonObject<T>;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a JSON object")
            }

            fn visit_map<A: MapAccess<'de>>(
                self,
                map: A,
            ) -> std::result::Result<Self::Value, A::Error> {
                T::deserialize(MapAccessDeserializer::new(map)).map(JsonObject)
            }
        }

        deserializer.deserialize_map(ObjectVisitor(PhantomData))
    }
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

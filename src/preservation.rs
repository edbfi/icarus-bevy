//! Lossless, value-opaque preservation of a verified prospect inner payload.
//!
//! This is preservation, not import: no name, class string or value is decoded
//! or given meaning, and nothing reaches the simulation as gameplay state. The
//! payload is framed with the strict experimental readers (root tags, the
//! `StateRecorderBlobs` recorder array and, per recorder body, the caller-selected
//! base-component tree). Every span the readers do not frame stays opaque bytes,
//! so every byte of the input survives. Construction fails closed unless the
//! structure re-serializes to the input's exact length and SHA-256 and its
//! native encoding decodes back to the same structure.
//!
//! The native encoding (version 1) belongs to the server's own snapshot schema,
//! not the ICARUS format. All integers are little-endian `u32`; `chunk` is a
//! length followed by that many bytes; `count` is a `u32` element count:
//!
//! ```text
//! section  = chunk(root terminator) count root*
//! root     = 0x00 kind chunk(header) chunk(payload)             ; opaque root tag
//!          | 0x01 chunk(header) chunk(count prefix) chunk(shared header)
//!                 count record*                                 ; recorder array
//! record   = count field* chunk(terminator)
//! field    = role kind chunk(header) value
//!            ; role 0 unknown, 1 class name: chunk(payload)
//!            ; role 2 binary data: chunk(count prefix) body
//! body     = 0x00 reason chunk(bytes) | 0x01 tree chunk(trailer)
//! tree     = count node* chunk(terminator)
//! node     = kind chunk(header) value
//! value    = 0x00 chunk(bytes) | 0x01 tree
//!          | 0x02 chunk(count prefix) chunk(shared header) count tree*
//! ```
//!
//! Decoding limits are owned by this encoding, not by the reader policies, so a
//! later reader change cannot make an already published generation unreadable.
use std::{borrow::Cow, fmt, sync::Arc};

use anyhow::{Context, Result, bail, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de, ser};
use sha2::{Digest, Sha256};

use crate::experimental::component::MAX_BOUNDED_COMPONENT_BYTES;
use crate::experimental::property_tree::{
    BaseComponentTree, ContainerItem, MAX_TREE_DEPTH, MAX_TREE_NODES, NodeValue, PropertyNode,
    PropertyTree, TreeLimits,
};
use crate::experimental::recorders::{RecorderArray, read_recorders};
use crate::experimental::struct_array::MAX_BOUNDED_STRUCT_ARRAY_BYTES;
use crate::experimental::tagged::{TagKind, TagProfile, read_root};
use crate::prospect::MAX_PAYLOAD_BYTES;

/// The only native encoding accepted inside native snapshot schemas 2 and 3.
pub const PRESERVED_ENCODING_VERSION: u32 = 1;
/// Native encoding cap: the 32 MiB payload cap plus 16 MiB of structural
/// framing. Research policy, not an ICARUS limit; checked before decoding.
pub const MAX_PRESERVED_ENCODED_BYTES: usize = 48 * 1024 * 1024;
/// Canonical padded Base64 of the encoding cap: exactly 64 MiB.
pub const MAX_PRESERVED_BASE64_BYTES: usize = MAX_PRESERVED_ENCODED_BYTES.div_ceil(3) * 4;

// Encoding-owned decode ceilings; equal to today's reader ceilings.
const MAX_ROOTS: usize = 4096;
const MAX_RECORDS: usize = 16_384;
const MAX_RECORD_FIELDS: usize = 65_536;
const MAX_DEPTH: usize = 16;
const MAX_BODY_NODES: usize = 1 << 20;

// Smallest encoded sizes, used to reject counts the remaining bytes cannot hold.
const MIN_ROOT: usize = 10;
const MIN_RECORD: usize = 8;
const MIN_FIELD: usize = 10;
const MIN_NODE: usize = 10;
const MIN_TREE: usize = 8;
const MIN_ITEM: usize = 5;

const PROFILE: TagProfile = TagProfile::MemoryArchive522;

/// Append-only code table for encoding version 1. A tag kind missing from this
/// table cannot be preserved and fails construction closed.
const KINDS: [TagKind; 14] = [
    TagKind::Array,
    TagKind::Struct,
    TagKind::String,
    TagKind::Name,
    TagKind::Int,
    TagKind::UInt32,
    TagKind::Float,
    TagKind::Text,
    TagKind::Byte,
    TagKind::Enum,
    TagKind::Bool,
    TagKind::Map,
    TagKind::Set,
    TagKind::Int64,
];

/// Explicit research bounds for framing recorder bodies.
#[derive(Clone, Copy, Debug)]
pub struct PreserveLimits {
    /// Bodies longer than this stay opaque (1 byte up to the component ceiling).
    pub component_bytes: usize,
    pub tree: TreeLimits,
}

impl Default for PreserveLimits {
    fn default() -> Self {
        Self {
            component_bytes: MAX_BOUNDED_COMPONENT_BYTES,
            tree: TreeLimits::default(),
        }
    }
}

impl PreserveLimits {
    /// Reject invalid bounds up front, rather than letting every body fail to
    /// frame and silently stay opaque.
    fn validate(&self) -> Result<()> {
        ensure!(
            (1..=MAX_BOUNDED_COMPONENT_BYTES).contains(&self.component_bytes),
            "preservation component byte limit outside the component ceiling"
        );
        ensure!(
            (1..=MAX_TREE_DEPTH).contains(&self.tree.max_depth),
            "preservation tree depth limit outside the tree ceiling"
        );
        ensure!(
            (1..=MAX_TREE_NODES).contains(&self.tree.max_nodes),
            "preservation tree node limit outside the tree ceiling"
        );
        ensure!(
            (1..=MAX_BOUNDED_STRUCT_ARRAY_BYTES).contains(&self.tree.struct_array_bytes),
            "preservation struct array byte limit outside the struct array ceiling"
        );
        Ok(())
    }
}

/// Why a recorder body is kept as opaque bytes instead of a tree.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpaqueReason {
    /// Longer than the selected component byte limit; not parsed.
    ExceedsComponentLimit,
    /// The strict base-component reader or tree limits rejected it.
    FramingRejected,
    /// It framed but did not re-serialize identically; kept verbatim.
    RoundTripMismatch,
}

const REASONS: [OpaqueReason; 3] = [
    OpaqueReason::ExceedsComponentLimit,
    OpaqueReason::FramingRejected,
    OpaqueReason::RoundTripMismatch,
];

/// One recorder body: a value-opaque tree when it frames, otherwise bytes.
pub enum PreservedBody {
    Component(BaseComponentTree),
    Opaque {
        reason: OpaqueReason,
        bytes: Vec<u8>,
    },
}

/// A recorder field value. Class strings and unknown payloads stay encoded.
pub enum FieldValue {
    /// A supported-header field other than the two known ones.
    Unknown(Vec<u8>),
    /// The complete encoded `ComponentClassName` string, never decoded.
    ClassName(Vec<u8>),
    /// The original four-byte `BinaryData` count and its body.
    BinaryData {
        count_prefix: Vec<u8>,
        body: PreservedBody,
    },
}

pub struct PreservedField {
    pub kind: TagKind,
    /// The original header, including name, type, size, index and extensions.
    pub header: Vec<u8>,
    pub value: FieldValue,
}

/// One recorder element: its fields in original order and its terminal name.
pub struct PreservedRecorder {
    pub fields: Vec<PreservedField>,
    pub terminator: Vec<u8>,
}

pub struct PreservedRecorders {
    /// The original `StateRecorderBlobs` root header.
    pub header: Vec<u8>,
    pub count_prefix: Vec<u8>,
    /// The original shared `StateRecorderBlob` struct header.
    pub shared_header: Vec<u8>,
    pub records: Vec<PreservedRecorder>,
}

/// One root tag in original order.
pub enum PreservedRoot {
    Opaque {
        kind: TagKind,
        header: Vec<u8>,
        payload: Vec<u8>,
    },
    Recorders(PreservedRecorders),
}

/// Counts and byte totals only; never names, class strings or values.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PreservationSummary {
    pub validation: &'static str,
    pub profile: &'static str,
    pub payload_bytes: usize,
    pub root_tags: usize,
    pub opaque_root_tags: usize,
    pub recorder_count: usize,
    pub recorder_fields: usize,
    pub unknown_recorder_fields: usize,
    pub class_value_bytes: usize,
    pub framed_bodies: usize,
    pub framed_body_bytes: usize,
    pub framed_property_nodes: usize,
    pub opaque_bodies: usize,
    pub opaque_body_bytes: usize,
    pub opaque_exceeds_component_limit: usize,
    pub opaque_framing_rejected: usize,
    pub opaque_round_trip_mismatch: usize,
    /// Always true: construction fails closed otherwise.
    pub byte_identical: bool,
    pub world_imported: bool,
    pub values_interpreted: bool,
}

/// Immutable, verified preservation of one inner payload. Fields are private so
/// a constructed value always reconstructs its recorded payload exactly.
pub struct PreservedProspect {
    roots: Vec<PreservedRoot>,
    root_terminator: Vec<u8>,
    payload_bytes: usize,
    payload_sha256: [u8; 32],
    /// Identity of the canonical native encoding, recorded at construction so
    /// a save can reference an existing sidecar without re-encoding.
    encoded_bytes: usize,
    encoded_sha256: [u8; 32],
    summary: PreservationSummary,
}

fn same_span(a: &[u8], b: &[u8]) -> bool {
    std::ptr::eq(a.as_ptr(), b.as_ptr()) && a.len() == b.len()
}

fn sha256(bytes: &[u8]) -> [u8; 32] {
    let mut out = [0; 32];
    out.copy_from_slice(&Sha256::digest(bytes));
    out
}

fn kind_code(kind: TagKind) -> Result<u8> {
    let index = KINDS
        .iter()
        .position(|known| *known == kind)
        .context("tag kind has no native preservation code")?;
    Ok(u8::try_from(index)?)
}

fn kind_from(code: u8) -> Result<TagKind> {
    KINDS
        .get(usize::from(code))
        .copied()
        .context("unknown preserved tag kind code")
}

fn frame_body(bytes: &[u8], limits: &PreserveLimits) -> PreservedBody {
    let opaque = |reason| PreservedBody::Opaque {
        reason,
        bytes: bytes.to_vec(),
    };
    if bytes.len() > limits.component_bytes {
        return opaque(OpaqueReason::ExceedsComponentLimit);
    }
    match BaseComponentTree::parse(bytes, PROFILE, limits.component_bytes, limits.tree) {
        Ok(tree) => match tree.to_bytes() {
            Ok(written) if written == bytes => PreservedBody::Component(tree),
            _ => opaque(OpaqueReason::RoundTripMismatch),
        },
        Err(_) => opaque(OpaqueReason::FramingRejected),
    }
}

fn frame_recorders(array: &RecorderArray<'_>, limits: &PreserveLimits) -> PreservedRecorders {
    let records = array
        .records()
        .iter()
        .map(|record| PreservedRecorder {
            fields: record
                .fields()
                .iter()
                .map(|field| {
                    // read_recorders returns subslices of each field's payload,
                    // so span identity selects the field without decoding names.
                    let value = if same_span(field.payload, record.class_value) {
                        FieldValue::ClassName(field.payload.to_vec())
                    } else if field.payload.len() >= 4
                        && same_span(&field.payload[4..], record.binary_data)
                    {
                        FieldValue::BinaryData {
                            count_prefix: field.payload[..4].to_vec(),
                            body: frame_body(record.binary_data, limits),
                        }
                    } else {
                        FieldValue::Unknown(field.payload.to_vec())
                    };
                    PreservedField {
                        kind: field.kind,
                        header: field.header.to_vec(),
                        value,
                    }
                })
                .collect(),
            terminator: record.terminator.to_vec(),
        })
        .collect();
    PreservedRecorders {
        header: array.root_header.to_vec(),
        count_prefix: array.count_prefix.to_vec(),
        shared_header: array.shared_header.to_vec(),
        records,
    }
}

impl PreservedProspect {
    /// Preserve an inner payload that the caller has already verified through the
    /// envelope integrity chain. Requires exact root and recorder framing; bodies
    /// frame as trees under `limits` or stay opaque with a reason. Succeeds only
    /// after exact-length and SHA-256 reconstruction and a native encoding round
    /// trip. No partial structure is returned on failure.
    pub fn from_inner_payload(payload: &[u8], limits: PreserveLimits) -> Result<Self> {
        ensure!(
            payload.len() <= MAX_PAYLOAD_BYTES,
            "preserved payload exceeds 32 MiB research limit"
        );
        limits.validate()?;
        let root = read_root(payload, PROFILE).context("invalid experimental root tags")?;
        let array =
            read_recorders(&root.tags, PROFILE).context("invalid experimental recorder framing")?;
        let roots = root
            .tags
            .iter()
            .map(|tag| {
                if same_span(tag.record, array.root_record) {
                    PreservedRoot::Recorders(frame_recorders(&array, &limits))
                } else {
                    PreservedRoot::Opaque {
                        kind: tag.kind,
                        header: tag.header.to_vec(),
                        payload: tag.payload.to_vec(),
                    }
                }
            })
            .collect();
        let preserved = Self::assemble(
            roots,
            root.terminator.to_vec(),
            payload.len(),
            sha256(payload),
        )?;
        // Persistability: the native encoding must fit and decode to the same
        // structure, so a later save can never publish an unreadable generation.
        let encoded = preserved.to_native_encoding()?;
        let decoded =
            Self::from_native_encoding(&encoded, preserved.payload_bytes, preserved.payload_sha256)
                .context("preserved native encoding does not round-trip")?;
        ensure!(
            decoded.summary == preserved.summary,
            "preserved native encoding does not round-trip"
        );
        let mut preserved = preserved;
        preserved.encoded_bytes = decoded.encoded_bytes;
        preserved.encoded_sha256 = decoded.encoded_sha256;
        Ok(preserved)
    }

    /// Validate the structural invariants, then require byte-identical
    /// reconstruction before any value exists.
    fn assemble(
        roots: Vec<PreservedRoot>,
        root_terminator: Vec<u8>,
        payload_bytes: usize,
        payload_sha256: [u8; 32],
    ) -> Result<Self> {
        ensure!(
            payload_bytes <= MAX_PAYLOAD_BYTES,
            "preserved payload exceeds 32 MiB research limit"
        );
        let mut preserved = Self {
            roots,
            root_terminator,
            payload_bytes,
            payload_sha256,
            // Set by the constructors once the encoding is known.
            encoded_bytes: 0,
            encoded_sha256: [0; 32],
            summary: PreservationSummary {
                validation: "experimental_lossless_preservation",
                profile: "experimental_memory_archive_522",
                payload_bytes,
                root_tags: 0,
                opaque_root_tags: 0,
                recorder_count: 0,
                recorder_fields: 0,
                unknown_recorder_fields: 0,
                class_value_bytes: 0,
                framed_bodies: 0,
                framed_body_bytes: 0,
                framed_property_nodes: 0,
                opaque_bodies: 0,
                opaque_body_bytes: 0,
                opaque_exceeds_component_limit: 0,
                opaque_framing_rejected: 0,
                opaque_round_trip_mismatch: 0,
                byte_identical: false,
                world_imported: false,
                values_interpreted: false,
            },
        };
        preserved.summarize()?;
        let rebuilt = preserved.to_inner_payload()?;
        ensure!(
            rebuilt.len() == payload_bytes && sha256(&rebuilt) == payload_sha256,
            "preserved payload reconstruction is not byte-identical"
        );
        preserved.summary.byte_identical = true;
        Ok(preserved)
    }

    fn summarize(&mut self) -> Result<()> {
        let summary = &mut self.summary;
        let mut recorder_roots = 0;
        for root in &self.roots {
            summary.root_tags += 1;
            let recorders = match root {
                PreservedRoot::Opaque { .. } => {
                    summary.opaque_root_tags += 1;
                    continue;
                }
                PreservedRoot::Recorders(recorders) => recorders,
            };
            recorder_roots += 1;
            for record in &recorders.records {
                summary.recorder_count += 1;
                let (mut classes, mut binaries) = (0, 0);
                for field in &record.fields {
                    summary.recorder_fields += 1;
                    match &field.value {
                        FieldValue::Unknown(_) => summary.unknown_recorder_fields += 1,
                        FieldValue::ClassName(bytes) => {
                            classes += 1;
                            summary.class_value_bytes += bytes.len();
                        }
                        FieldValue::BinaryData { body, .. } => {
                            binaries += 1;
                            match body {
                                PreservedBody::Component(tree) => {
                                    summary.framed_bodies += 1;
                                    summary.framed_property_nodes += tree.summary().property_count;
                                    summary.framed_body_bytes += tree.to_bytes()?.len();
                                }
                                PreservedBody::Opaque { reason, bytes } => {
                                    summary.opaque_bodies += 1;
                                    summary.opaque_body_bytes += bytes.len();
                                    *match reason {
                                        OpaqueReason::ExceedsComponentLimit => {
                                            &mut summary.opaque_exceeds_component_limit
                                        }
                                        OpaqueReason::FramingRejected => {
                                            &mut summary.opaque_framing_rejected
                                        }
                                        OpaqueReason::RoundTripMismatch => {
                                            &mut summary.opaque_round_trip_mismatch
                                        }
                                    } += 1;
                                }
                            }
                        }
                    }
                }
                ensure!(
                    classes == 1 && binaries == 1,
                    "preserved recorder needs exactly one class and one binary field"
                );
            }
        }
        ensure!(
            recorder_roots == 1,
            "preserved payload needs exactly one recorder root"
        );
        Ok(())
    }

    /// Rebuild the exact inner payload from the preserved structure.
    pub fn to_inner_payload(&self) -> Result<Vec<u8>> {
        let mut out = Vec::with_capacity(self.payload_bytes);
        for root in &self.roots {
            match root {
                PreservedRoot::Opaque {
                    header, payload, ..
                } => {
                    out.extend_from_slice(header);
                    out.extend_from_slice(payload);
                }
                PreservedRoot::Recorders(recorders) => {
                    out.extend_from_slice(&recorders.header);
                    out.extend_from_slice(&recorders.count_prefix);
                    out.extend_from_slice(&recorders.shared_header);
                    for record in &recorders.records {
                        for field in &record.fields {
                            out.extend_from_slice(&field.header);
                            match &field.value {
                                FieldValue::Unknown(bytes) | FieldValue::ClassName(bytes) => {
                                    out.extend_from_slice(bytes);
                                }
                                FieldValue::BinaryData { count_prefix, body } => {
                                    out.extend_from_slice(count_prefix);
                                    match body {
                                        PreservedBody::Component(tree) => {
                                            out.extend_from_slice(&tree.to_bytes()?);
                                        }
                                        PreservedBody::Opaque { bytes, .. } => {
                                            out.extend_from_slice(bytes);
                                        }
                                    }
                                }
                            }
                        }
                        out.extend_from_slice(&record.terminator);
                    }
                }
            }
            // Bounded growth even for structures decoded from crafted input.
            ensure!(
                out.len() <= self.payload_bytes,
                "preserved payload reconstruction exceeds its recorded length"
            );
        }
        out.extend_from_slice(&self.root_terminator);
        Ok(out)
    }

    pub fn roots(&self) -> &[PreservedRoot] {
        &self.roots
    }

    pub fn root_terminator(&self) -> &[u8] {
        &self.root_terminator
    }

    pub fn payload_bytes(&self) -> usize {
        self.payload_bytes
    }

    pub fn payload_sha256(&self) -> [u8; 32] {
        self.payload_sha256
    }

    pub fn summary(&self) -> &PreservationSummary {
        &self.summary
    }

    /// Length of the canonical native encoding.
    pub fn encoded_bytes(&self) -> usize {
        self.encoded_bytes
    }

    /// SHA-256 of the canonical native encoding; names its schema-3 sidecar.
    pub fn encoded_sha256(&self) -> [u8; 32] {
        self.encoded_sha256
    }

    /// The deterministic native encoding version 1, at most 48 MiB.
    pub fn to_native_encoding(&self) -> Result<Vec<u8>> {
        let mut out = Encoder(Vec::new());
        out.chunk(&self.root_terminator)?;
        out.count(self.roots.len())?;
        for root in &self.roots {
            match root {
                PreservedRoot::Opaque {
                    kind,
                    header,
                    payload,
                } => {
                    out.byte(0);
                    out.byte(kind_code(*kind)?);
                    out.chunk(header)?;
                    out.chunk(payload)?;
                }
                PreservedRoot::Recorders(recorders) => {
                    out.byte(1);
                    out.chunk(&recorders.header)?;
                    out.chunk(&recorders.count_prefix)?;
                    out.chunk(&recorders.shared_header)?;
                    out.count(recorders.records.len())?;
                    for record in &recorders.records {
                        out.count(record.fields.len())?;
                        for field in &record.fields {
                            out.field(field)?;
                        }
                        out.chunk(&record.terminator)?;
                    }
                }
            }
        }
        ensure!(
            out.0.len() <= MAX_PRESERVED_ENCODED_BYTES,
            "preserved native encoding exceeds 48 MiB limit"
        );
        Ok(out.0)
    }

    /// Decode native encoding version 1 and require that it reconstructs a
    /// payload of exactly `payload_bytes` with digest `payload_sha256`. Every
    /// count and length is checked against fixed ceilings and the remaining
    /// input before it drives a loop or allocation; trailing bytes fail.
    pub fn from_native_encoding(
        encoded: &[u8],
        payload_bytes: usize,
        payload_sha256: [u8; 32],
    ) -> Result<Self> {
        Self::from_hashed_native_encoding(encoded, sha256(encoded), payload_bytes, payload_sha256)
    }

    /// As `from_native_encoding`, for a caller that has already verified
    /// `encoded_sha256` over exactly `encoded`, avoiding a second digest pass.
    pub(crate) fn from_hashed_native_encoding(
        encoded: &[u8],
        encoded_sha256: [u8; 32],
        payload_bytes: usize,
        payload_sha256: [u8; 32],
    ) -> Result<Self> {
        ensure!(
            encoded.len() <= MAX_PRESERVED_ENCODED_BYTES,
            "preserved native encoding exceeds 48 MiB limit"
        );
        ensure!(
            payload_bytes <= MAX_PAYLOAD_BYTES,
            "preserved payload exceeds 32 MiB research limit"
        );
        let mut input = Decoder {
            input: encoded,
            position: 0,
            fields: 0,
        };
        let root_terminator = input.chunk()?;
        let root_count = input.count(MAX_ROOTS, MIN_ROOT)?;
        let mut roots = Vec::new();
        for _ in 0..root_count {
            roots.push(match input.byte()? {
                0 => PreservedRoot::Opaque {
                    kind: kind_from(input.byte()?)?,
                    header: input.chunk()?,
                    payload: input.chunk()?,
                },
                1 => PreservedRoot::Recorders(input.recorders()?),
                _ => bail!("unknown preserved root code"),
            });
        }
        ensure!(
            input.position == encoded.len(),
            "trailing data after preserved native encoding"
        );
        let mut preserved = Self::assemble(roots, root_terminator, payload_bytes, payload_sha256)?;
        preserved.encoded_bytes = encoded.len();
        preserved.encoded_sha256 = encoded_sha256;
        Ok(preserved)
    }
}

struct Encoder(Vec<u8>);

impl Encoder {
    fn byte(&mut self, value: u8) {
        self.0.push(value);
    }

    fn count(&mut self, value: usize) -> Result<()> {
        let value = u32::try_from(value).context("preserved count exceeds u32")?;
        self.0.extend_from_slice(&value.to_le_bytes());
        Ok(())
    }

    fn chunk(&mut self, bytes: &[u8]) -> Result<()> {
        self.count(bytes.len())?;
        self.0.extend_from_slice(bytes);
        // Fail early instead of growing far past the cap.
        ensure!(
            self.0.len() <= MAX_PRESERVED_ENCODED_BYTES,
            "preserved native encoding exceeds 48 MiB limit"
        );
        Ok(())
    }

    fn field(&mut self, field: &PreservedField) -> Result<()> {
        let role = match &field.value {
            FieldValue::Unknown(_) => 0,
            FieldValue::ClassName(_) => 1,
            FieldValue::BinaryData { .. } => 2,
        };
        self.byte(role);
        self.byte(kind_code(field.kind)?);
        self.chunk(&field.header)?;
        match &field.value {
            FieldValue::Unknown(bytes) | FieldValue::ClassName(bytes) => self.chunk(bytes),
            FieldValue::BinaryData { count_prefix, body } => {
                self.chunk(count_prefix)?;
                match body {
                    PreservedBody::Opaque { reason, bytes } => {
                        self.byte(0);
                        let code = REASONS
                            .iter()
                            .position(|known| known == reason)
                            .context("opaque reason has no native code")?;
                        self.byte(u8::try_from(code)?);
                        self.chunk(bytes)
                    }
                    PreservedBody::Component(tree) => {
                        self.byte(1);
                        self.tree(&tree.tree)?;
                        self.chunk(&tree.trailer)
                    }
                }
            }
        }
    }

    fn tree(&mut self, tree: &PropertyTree) -> Result<()> {
        self.count(tree.properties.len())?;
        for node in &tree.properties {
            self.byte(kind_code(node.kind)?);
            self.chunk(&node.header)?;
            match &node.value {
                NodeValue::Opaque(bytes) => {
                    self.byte(0);
                    self.chunk(bytes)?;
                }
                NodeValue::Struct(nested) => {
                    self.byte(1);
                    self.tree(nested)?;
                }
                NodeValue::StructArray {
                    prefix,
                    shared_header,
                    elements,
                } => {
                    self.byte(2);
                    self.chunk(prefix)?;
                    self.chunk(shared_header)?;
                    self.count(elements.len())?;
                    for element in elements {
                        self.tree(element)?;
                    }
                }
                NodeValue::Map { removed, entries } => {
                    self.byte(3);
                    self.items(removed)?;
                    self.count(entries.len())?;
                    for (key, value) in entries {
                        self.item(key)?;
                        self.item(value)?;
                    }
                }
                NodeValue::Set { removed, elements } => {
                    self.byte(4);
                    self.items(removed)?;
                    self.items(elements)?;
                }
            }
        }
        self.chunk(&tree.terminator)
    }

    fn items(&mut self, items: &[ContainerItem]) -> Result<()> {
        self.count(items.len())?;
        for item in items {
            self.item(item)?;
        }
        Ok(())
    }

    fn item(&mut self, item: &ContainerItem) -> Result<()> {
        match item {
            ContainerItem::Bytes(bytes) => {
                self.byte(0);
                self.chunk(bytes)
            }
            ContainerItem::Struct(tree) => {
                self.byte(1);
                self.tree(tree)
            }
        }
    }
}

struct Decoder<'a> {
    input: &'a [u8],
    position: usize,
    /// Recorder fields across the whole section.
    fields: usize,
}

impl Decoder<'_> {
    fn remaining(&self) -> usize {
        self.input.len() - self.position
    }

    fn take(&mut self, length: usize) -> Result<&[u8]> {
        ensure!(
            length <= self.remaining(),
            "truncated preserved native encoding"
        );
        let bytes = &self.input[self.position..self.position + length];
        self.position += length;
        Ok(bytes)
    }

    fn byte(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    fn u32(&mut self) -> Result<usize> {
        let bytes: [u8; 4] = self.take(4)?.try_into()?;
        Ok(usize::try_from(u32::from_le_bytes(bytes))?)
    }

    /// The length is checked against the remaining input before allocating.
    fn chunk(&mut self) -> Result<Vec<u8>> {
        let length = self.u32()?;
        Ok(self.take(length)?.to_vec())
    }

    fn exact_chunk(&mut self, length: usize) -> Result<Vec<u8>> {
        let bytes = self.chunk()?;
        ensure!(
            bytes.len() == length,
            "preserved fixed-width field has the wrong length"
        );
        Ok(bytes)
    }

    fn count(&mut self, ceiling: usize, minimum_each: usize) -> Result<usize> {
        let count = self.u32()?;
        ensure!(count <= ceiling, "preserved count exceeds encoding limit");
        ensure!(
            count
                .checked_mul(minimum_each)
                .is_some_and(|needed| needed <= self.remaining()),
            "preserved count exceeds remaining encoded bytes"
        );
        Ok(count)
    }

    fn recorders(&mut self) -> Result<PreservedRecorders> {
        let header = self.chunk()?;
        let count_prefix = self.exact_chunk(4)?;
        let shared_header = self.chunk()?;
        let record_count = self.count(MAX_RECORDS, MIN_RECORD)?;
        let mut records = Vec::new();
        for _ in 0..record_count {
            let field_count = self.count(MAX_RECORD_FIELDS, MIN_FIELD)?;
            self.fields = self
                .fields
                .checked_add(field_count)
                .filter(|total| *total <= MAX_RECORD_FIELDS)
                .context("preserved recorder fields exceed encoding limit")?;
            let mut fields = Vec::new();
            for _ in 0..field_count {
                fields.push(self.field()?);
            }
            records.push(PreservedRecorder {
                fields,
                terminator: self.chunk()?,
            });
        }
        Ok(PreservedRecorders {
            header,
            count_prefix,
            shared_header,
            records,
        })
    }

    fn field(&mut self) -> Result<PreservedField> {
        let role = self.byte()?;
        let kind = kind_from(self.byte()?)?;
        let header = self.chunk()?;
        let value = match role {
            0 => FieldValue::Unknown(self.chunk()?),
            1 => FieldValue::ClassName(self.chunk()?),
            2 => {
                let count_prefix = self.exact_chunk(4)?;
                let body = match self.byte()? {
                    0 => {
                        let reason = *REASONS
                            .get(usize::from(self.byte()?))
                            .context("unknown preserved opaque reason code")?;
                        PreservedBody::Opaque {
                            reason,
                            bytes: self.chunk()?,
                        }
                    }
                    1 => {
                        let mut nodes = 0;
                        let tree = self.tree(&mut nodes, 1)?;
                        PreservedBody::Component(BaseComponentTree {
                            tree,
                            trailer: self.exact_chunk(4)?,
                        })
                    }
                    _ => bail!("unknown preserved body code"),
                };
                FieldValue::BinaryData { count_prefix, body }
            }
            _ => bail!("unknown preserved field role"),
        };
        Ok(PreservedField {
            kind,
            header,
            value,
        })
    }

    /// Recursion is bounded by the encoding's depth ceiling.
    fn tree(&mut self, nodes: &mut usize, depth: usize) -> Result<PropertyTree> {
        ensure!(depth <= MAX_DEPTH, "preserved tree exceeds depth limit");
        let count = self.count(MAX_BODY_NODES, MIN_NODE)?;
        let mut properties = Vec::new();
        for _ in 0..count {
            ensure!(*nodes < MAX_BODY_NODES, "preserved tree exceeds node limit");
            *nodes += 1;
            let kind = kind_from(self.byte()?)?;
            let header = self.chunk()?;
            let value = match self.byte()? {
                0 => NodeValue::Opaque(self.chunk()?),
                1 => NodeValue::Struct(self.tree(nodes, depth + 1)?),
                2 => {
                    let prefix = self.exact_chunk(4)?;
                    let shared_header = self.chunk()?;
                    let element_count = self.count(MAX_BODY_NODES, MIN_TREE)?;
                    let mut elements = Vec::new();
                    for _ in 0..element_count {
                        elements.push(self.tree(nodes, depth + 1)?);
                    }
                    NodeValue::StructArray {
                        prefix,
                        shared_header,
                        elements,
                    }
                }
                3 => {
                    let removed = self.items(nodes, depth)?;
                    let count = self.count(MAX_BODY_NODES, 2 * MIN_ITEM)?;
                    let mut entries = Vec::new();
                    for _ in 0..count {
                        let key = self.item(nodes, depth)?;
                        entries.push((key, self.item(nodes, depth)?));
                    }
                    NodeValue::Map { removed, entries }
                }
                4 => NodeValue::Set {
                    removed: self.items(nodes, depth)?,
                    elements: self.items(nodes, depth)?,
                },
                _ => bail!("unknown preserved node value code"),
            };
            properties.push(PropertyNode {
                kind,
                header,
                value,
            });
        }
        Ok(PropertyTree {
            properties,
            terminator: self.chunk()?,
        })
    }

    fn items(&mut self, nodes: &mut usize, depth: usize) -> Result<Vec<ContainerItem>> {
        let count = self.count(MAX_BODY_NODES, MIN_ITEM)?;
        let mut items = Vec::new();
        for _ in 0..count {
            items.push(self.item(nodes, depth)?);
        }
        Ok(items)
    }

    fn item(&mut self, nodes: &mut usize, depth: usize) -> Result<ContainerItem> {
        ensure!(*nodes < MAX_BODY_NODES, "preserved tree exceeds node limit");
        *nodes += 1;
        Ok(match self.byte()? {
            0 => ContainerItem::Bytes(self.chunk()?),
            1 => ContainerItem::Struct(self.tree(nodes, depth + 1)?),
            _ => bail!("unknown preserved container item code"),
        })
    }
}

/// Snapshot equality means the same preserved payload and the same structure
/// counts; the payload is verified against its digest at construction.
impl PartialEq for PreservedProspect {
    fn eq(&self, other: &Self) -> bool {
        self.payload_bytes == other.payload_bytes
            && self.payload_sha256 == other.payload_sha256
            && self.summary == other.summary
    }
}

impl fmt::Debug for PreservedProspect {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PreservedProspect")
            .field("summary", &self.summary)
            .finish_non_exhaustive()
    }
}

impl fmt::Debug for PreservedRoot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Opaque {
                kind,
                header,
                payload,
            } => f
                .debug_struct("Opaque")
                .field("kind", kind)
                .field("header_bytes", &header.len())
                .field("payload_bytes", &payload.len())
                .finish(),
            Self::Recorders(recorders) => f.debug_tuple("Recorders").field(recorders).finish(),
        }
    }
}

impl fmt::Debug for PreservedRecorders {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PreservedRecorders")
            .field("header_bytes", &self.header.len())
            .field("shared_header_bytes", &self.shared_header.len())
            .field("records", &self.records.len())
            .finish_non_exhaustive()
    }
}

impl fmt::Debug for PreservedRecorder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PreservedRecorder")
            .field("fields", &self.fields)
            .field("terminator_bytes", &self.terminator.len())
            .finish()
    }
}

impl fmt::Debug for PreservedField {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PreservedField")
            .field("kind", &self.kind)
            .field("header_bytes", &self.header.len())
            .field("value", &self.value)
            .finish()
    }
}

impl fmt::Debug for FieldValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unknown(bytes) => f
                .debug_struct("Unknown")
                .field("bytes", &bytes.len())
                .finish(),
            Self::ClassName(bytes) => f
                .debug_struct("ClassName")
                .field("bytes", &bytes.len())
                .finish(),
            Self::BinaryData { body, .. } => f.debug_tuple("BinaryData").field(body).finish(),
        }
    }
}

impl fmt::Debug for PreservedBody {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            // The tree's own Debug already prints kinds and lengths only.
            Self::Component(tree) => f.debug_tuple("Component").field(tree).finish(),
            Self::Opaque { reason, bytes } => f
                .debug_struct("Opaque")
                .field("reason", reason)
                .field("bytes", &bytes.len())
                .finish(),
        }
    }
}

/// Persisted form inside native snapshot schema 2, now load-only: the native
/// encoding inline in canonical padded Base64, decoded and verified on load.
/// The serializer remains only so the legacy form stays authorable; native
/// persistence writes schema 3 with a [`SectionReference`].
#[derive(Serialize)]
struct SectionOut<'a> {
    encoding: u32,
    payload_bytes: usize,
    payload_sha256: String,
    data: &'a str,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SectionIn<'a> {
    encoding: u32,
    payload_bytes: u64,
    payload_sha256: Cow<'a, str>,
    #[serde(borrow)]
    data: Cow<'a, str>,
}

fn is_lower_hex_digest(text: &str) -> bool {
    text.len() == 64
        && text
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn parse_digest(text: &str) -> Result<[u8; 32]> {
    ensure!(
        is_lower_hex_digest(text),
        "invalid preserved payload digest"
    );
    let mut digest = [0; 32];
    for (byte, pair) in digest.iter_mut().zip(text.as_bytes().as_chunks::<2>().0) {
        *byte = u8::from_str_radix(std::str::from_utf8(pair)?, 16)?;
    }
    Ok(digest)
}

impl SectionIn<'_> {
    fn decode(&self) -> Result<PreservedProspect> {
        ensure!(
            self.encoding == PRESERVED_ENCODING_VERSION,
            "unsupported preserved native encoding"
        );
        let payload_bytes = usize::try_from(self.payload_bytes)?;
        ensure!(
            payload_bytes <= MAX_PAYLOAD_BYTES,
            "preserved payload exceeds 32 MiB research limit"
        );
        let digest = parse_digest(&self.payload_sha256)?;
        // Checked before Base64 decoding allocates.
        ensure!(
            self.data.len() <= MAX_PRESERVED_BASE64_BYTES,
            "preserved section exceeds 64 MiB Base64 limit"
        );
        let encoded = STANDARD
            .decode(self.data.as_bytes())
            .context("invalid canonical Base64 in preserved section")?;
        PreservedProspect::from_native_encoding(&encoded, payload_bytes, digest)
    }
}

/// File name prefix of a schema-3 sidecar; the full name is the prefix, the
/// encoding's lowercase SHA-256 and [`SIDECAR_SUFFIX`].
pub const SIDECAR_PREFIX: &str = "preserved-";
pub const SIDECAR_SUFFIX: &str = ".bin";

/// Whether `name` is exactly a sidecar name: prefix, 64 lowercase hexadecimal
/// digits, suffix. Retention cleanup removes only names that match.
pub fn is_sidecar_name(name: &str) -> bool {
    name.strip_prefix(SIDECAR_PREFIX)
        .and_then(|rest| rest.strip_suffix(SIDECAR_SUFFIX))
        .is_some_and(is_lower_hex_digest)
}

/// Persisted form inside native snapshot schema 3: a reference to the
/// content-addressed sidecar holding the raw native encoding (no Base64). The
/// sidecar is verified by size and SHA-256 before the bounded decoder runs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SectionReference {
    encoding: u32,
    payload_bytes: u64,
    payload_sha256: String,
    encoded_bytes: u64,
    encoded_sha256: String,
}

impl SectionReference {
    /// Reference a constructed section by its recorded encoding identity.
    pub(crate) fn of(preserved: &PreservedProspect) -> Self {
        Self {
            encoding: PRESERVED_ENCODING_VERSION,
            payload_bytes: preserved.payload_bytes as u64,
            payload_sha256: crate::hex_lower(&preserved.payload_sha256),
            encoded_bytes: preserved.encoded_bytes as u64,
            encoded_sha256: crate::hex_lower(&preserved.encoded_sha256),
        }
    }

    /// Fixed-message checks of every field before any sidecar is opened.
    pub(crate) fn validate(&self) -> Result<()> {
        ensure!(
            self.encoding == PRESERVED_ENCODING_VERSION,
            "unsupported preserved native encoding"
        );
        ensure!(
            usize::try_from(self.payload_bytes).is_ok_and(|bytes| bytes <= MAX_PAYLOAD_BYTES),
            "preserved payload exceeds 32 MiB research limit"
        );
        ensure!(
            usize::try_from(self.encoded_bytes)
                .is_ok_and(|bytes| bytes <= MAX_PRESERVED_ENCODED_BYTES),
            "preserved native encoding exceeds 48 MiB limit"
        );
        parse_digest(&self.payload_sha256)?;
        ensure!(
            is_lower_hex_digest(&self.encoded_sha256),
            "invalid preserved encoding digest"
        );
        Ok(())
    }

    /// The content-addressed sidecar file name; call after `validate`.
    pub(crate) fn sidecar_name(&self) -> String {
        format!("{SIDECAR_PREFIX}{}{SIDECAR_SUFFIX}", self.encoded_sha256)
    }

    pub(crate) fn encoded_bytes(&self) -> u64 {
        self.encoded_bytes
    }

    /// Lowercase hexadecimal SHA-256 of the sidecar contents.
    pub(crate) fn encoded_sha256(&self) -> &str {
        &self.encoded_sha256
    }

    /// Verify the sidecar bytes' size and SHA-256, then decode them with the
    /// bounded decoder against the recorded payload length and digest.
    pub(crate) fn decode(&self, encoded: &[u8]) -> Result<PreservedProspect> {
        self.validate()?;
        ensure!(
            encoded.len() as u64 == self.encoded_bytes,
            "preserved sidecar size mismatch"
        );
        let digest = sha256(encoded);
        ensure!(
            crate::hex_lower(&digest) == self.encoded_sha256,
            "preserved sidecar checksum mismatch"
        );
        PreservedProspect::from_hashed_native_encoding(
            encoded,
            digest,
            usize::try_from(self.payload_bytes)?,
            parse_digest(&self.payload_sha256)?,
        )
    }
}

/// Serde adapter for `WorldSnapshot::preserved_prospect`. Errors carry fixed
/// messages only; snapshot diagnostics keep JSON position and category.
pub(crate) mod snapshot_section {
    use super::*;

    pub(crate) fn serialize<S: Serializer>(
        value: &Option<Arc<PreservedProspect>>,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        let Some(preserved) = value else {
            return serializer.serialize_none();
        };
        let encoded = preserved
            .to_native_encoding()
            .map_err(|_| ser::Error::custom("preserved section cannot be encoded"))?;
        let data = STANDARD.encode(&encoded);
        drop(encoded);
        SectionOut {
            encoding: PRESERVED_ENCODING_VERSION,
            payload_bytes: preserved.payload_bytes,
            payload_sha256: crate::hex_lower(&preserved.payload_sha256),
            data: &data,
        }
        .serialize(serializer)
    }

    pub(crate) fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Option<Arc<PreservedProspect>>, D::Error> {
        let Some(section) =
            Option::<crate::json::JsonObject<SectionIn<'de>>>::deserialize(deserializer)?
        else {
            return Ok(None);
        };
        section
            .0
            .decode()
            .map(|preserved| Some(Arc::new(preserved)))
            .map_err(|_| de::Error::custom("invalid preserved prospect section"))
    }
}

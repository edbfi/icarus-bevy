//! EXPERIMENTAL disconnected codec for statically observed actor-channel bunches.
//!
//! Client-build static observations only (F-569..F-573): NetGUID references,
//! the open-bunch spawn record, the compressed rotator and the content-block
//! headers that frame replicated state. Server writers were not read, nothing
//! here is wire-validated, and no connection state, package map, object
//! resolution, admission or gameplay occurs. Writers mirror the readers for
//! round-trip research only.
//!
//! Caller-selected, because their original sources are unread or outside this
//! codec: the engine net version, whether the archive has exports enabled,
//! whether the level reference is present (its archive-flag gate is unread),
//! the byte order of 4-byte fields and string counts, and every limit.
//!
//! Unsupported and opaque on purpose:
//! - content-block payload bits stay opaque: the property stream and the
//!   per-property net serializers are not decoded here;
//! - NetGUID export bunches and must-be-mapped GUID prefixes are not read, so
//!   [`parse_actor_bunch`] rejects bunches that carry them, and partial bunches;
//! - export flags with reserved bits 3-7, or a checksum bit without the path
//!   bit (whether the checksum read is nested in the path branch is unread);
//! - non-canonical packed integers (the original reader is more lenient), so
//!   every accepted structure re-encodes bit-identically.
//!
//! Interpretations, not observations: below engine net version 13 no quantized
//! bit is read and the vector is taken as quantized (INFERRED from the gate);
//! the component bit count is read as a capped integer with maximum 24, like
//! the components with maximum `1 << (bits + 2)`; "dynamic sub-object" is read
//! as a sub-object whose stably-named bit is clear; recursion depth counts
//! outer references, so a top-level reference plus 16 outers is the deepest
//! accepted chain. Angle rounding (half up) is inferred (F-573). Deletion
//! blocks (zero class reference) carry no payload count (F-576).
//!
//! `Debug` output and summaries carry kinds, counts and lengths only, never
//! GUID values, paths, checksums, vector or angle values, or payload bits.
use anyhow::{Context, Result, ensure};
use serde::Serialize;

use super::archive::{ByteOrder, StringEncoding, read_string_frame};
use super::control::TokenEncoding;
use super::control_messages::ArchiveText;
use super::packet::{BitReader, BitWriter, Bunch, Payload};

/// Deepest outer-reference recursion accepted below a top-level reference.
pub const MAX_GUID_DEPTH: usize = 16;
/// The default GUID, which always carries an export-flags byte (F-572).
pub const DEFAULT_GUID: u32 = 1;
/// Export flag bit 0: a recursive outer reference and a path string follow.
pub const EXPORT_HAS_PATH: u8 = 1;
/// Export flag bit 1: no-load.
pub const EXPORT_NO_LOAD: u8 = 2;
/// Export flag bit 2: a 4-byte network checksum follows.
pub const EXPORT_HAS_CHECKSUM: u8 = 4;
const EXPORT_KNOWN: u8 = EXPORT_HAS_PATH | EXPORT_NO_LOAD | EXPORT_HAS_CHECKSUM;
/// First engine net version whose spawn vectors read a quantized bit (F-571).
pub const QUANTIZED_BIT_NET_VERSION: u32 = 13;
/// Maximum of the capped component-bit-count field (F-571).
pub const VECTOR_BITS_MAX: u32 = 24;
/// Quantized spawn-vector scale (F-573).
pub const VECTOR_SCALE: f32 = 0.1;

/// Research resource policy; the original bounds are unknown.
#[derive(Clone, Copy, Debug)]
pub struct ActorChannelLimits {
    /// Content blocks accepted per bunch body.
    pub max_blocks: usize,
    /// Opaque payload bits accepted per content block.
    pub max_payload_bits: usize,
    /// Declared path-string units, including the terminator.
    pub max_path_units: usize,
}

impl Default for ActorChannelLimits {
    fn default() -> Self {
        Self {
            max_blocks: 1024,
            max_payload_bits: 65_536,
            max_path_units: 1024,
        }
    }
}

/// Caller-selected codec parameters; see the module documentation.
#[derive(Clone, Copy, Debug)]
pub struct ActorChannelCodec {
    pub engine_net_version: u32,
    /// Export-flags bytes follow every nonzero GUID, not only the default one.
    pub exports_enabled: bool,
    /// Dynamic spawn records carry the level reference.
    pub has_level: bool,
    /// Order of raw vector floats, checksums and path-string counts.
    pub order: ByteOrder,
    pub limits: ActorChannelLimits,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GuidKind {
    /// GUID 0: no object.
    Null,
    /// Odd GUID.
    Static,
    /// Nonzero even GUID.
    Dynamic,
}

pub fn guid_kind(guid: u32) -> GuidKind {
    match guid {
        0 => GuidKind::Null,
        guid if guid.is_multiple_of(2) => GuidKind::Dynamic,
        _ => GuidKind::Static,
    }
}

/// One NetGUID reference (F-572). `Debug` shows kinds and lengths only.
#[derive(Clone, PartialEq, Eq)]
pub struct NetGuidRef {
    pub guid: u32,
    /// Present exactly when the GUID is nonzero and is the default GUID or the
    /// codec has exports enabled.
    pub export: Option<GuidExport>,
}

/// The export-flags byte and what it adds.
#[derive(Clone, PartialEq, Eq)]
pub struct GuidExport {
    pub no_load: bool,
    /// Flag bit 0.
    pub path: Option<ExportPath>,
    /// Flag bit 2; requires `path` in this codec.
    pub checksum: Option<u32>,
}

#[derive(Clone, PartialEq, Eq)]
pub struct ExportPath {
    pub outer: Box<NetGuidRef>,
    pub path: ArchiveText,
}

impl NetGuidRef {
    pub fn null() -> Self {
        Self {
            guid: 0,
            export: None,
        }
    }

    /// A reference without an export-flags byte.
    pub fn plain(guid: u32) -> Self {
        Self { guid, export: None }
    }

    pub fn kind(&self) -> GuidKind {
        guid_kind(self.guid)
    }

    pub fn summary(&self) -> GuidSummary {
        let export = self.export.as_ref();
        let path = export.and_then(|export| export.path.as_ref());
        GuidSummary {
            kind: self.kind(),
            export: export.is_some(),
            no_load: export.is_some_and(|export| export.no_load),
            checksum: export.is_some_and(|export| export.checksum.is_some()),
            path_units: path.map(|path| path.path.units().len()),
            outer: path.map(|path| Box::new(path.outer.summary())),
        }
    }
}

impl std::fmt::Debug for NetGuidRef {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("NetGuidRef")
            .field("kind", &self.kind())
            .field("export", &self.export)
            .finish_non_exhaustive()
    }
}

impl std::fmt::Debug for GuidExport {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("GuidExport")
            .field("no_load", &self.no_load)
            .field("path", &self.path)
            .field("checksum", &self.checksum.is_some())
            .finish()
    }
}

impl std::fmt::Debug for ExportPath {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ExportPath")
            .field("outer", &self.outer)
            .field("path", &self.path)
            .finish()
    }
}

/// Spawn vector (F-571). Quantized integers are canonical; no float is applied
/// on read.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SpawnVector {
    /// Three 4-byte float bit patterns, X then Y then Z (version 13+ only).
    Raw([u32; 3]),
    /// The capped bit-count field (below 24) and three components, each below
    /// `1 << (bits + 2)`.
    Quantized { bits: u32, components: [u32; 3] },
}

impl SpawnVector {
    /// Decode: raw bit patterns as floats; quantized as
    /// `(value - (1 << (bits + 1))) * 0.1` in f32 (F-573). Equality with the
    /// original's float arithmetic is not claimed.
    pub fn decode(&self) -> [f32; 3] {
        match *self {
            Self::Raw(bits) => bits.map(f32::from_bits),
            Self::Quantized { bits, components } => {
                let bias = 1i64 << (bits.min(VECTOR_BITS_MAX) + 1);
                components.map(|value| (i64::from(value) - bias) as f32 * VECTOR_SCALE)
            }
        }
    }

    pub fn summary(&self) -> VectorSummary {
        match self {
            Self::Raw(_) => VectorSummary {
                form: VectorForm::Raw,
                bits: None,
            },
            Self::Quantized { bits, .. } => VectorSummary {
                form: VectorForm::Quantized,
                bits: Some(*bits),
            },
        }
    }
}

impl std::fmt::Debug for SpawnVector {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Raw(_) => formatter.write_str("Raw"),
            Self::Quantized { bits, .. } => formatter
                .debug_struct("Quantized")
                .field("bits", bits)
                .finish_non_exhaustive(),
        }
    }
}

/// Compressed rotator (F-573): per axis, `None` when the bit is clear, else the
/// raw 16-bit quantized angle. `Some(0)` is preserved as read, although the
/// observed writer sets the bit only for nonzero angles.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub struct CompressedRotator {
    pub pitch: Option<u16>,
    pub yaw: Option<u16>,
    pub roll: Option<u16>,
}

/// `round(degrees * 65536 / 360)` modulo 65536, rounding half up (inferred).
pub fn quantize_angle(degrees: f32) -> Result<u16> {
    ensure!(degrees.is_finite(), "rotator angle must be finite");
    let scaled = (degrees * 65536.0 / 360.0 + 0.5).floor();
    ensure!(
        scaled.abs() < 2f32.powi(62),
        "rotator angle is out of the quantization range"
    );
    Ok((scaled as i64 & 0xffff) as u16)
}

/// `value * 360 / 65536` degrees.
pub fn dequantize_angle(value: u16) -> f32 {
    f32::from(value) * 360.0 / 65536.0
}

impl CompressedRotator {
    /// Canonical encoding: an axis is present exactly when its quantized value
    /// is nonzero.
    pub fn from_degrees(pitch: f32, yaw: f32, roll: f32) -> Result<Self> {
        let axis = |degrees| quantize_angle(degrees).map(|value| (value != 0).then_some(value));
        Ok(Self {
            pitch: axis(pitch)?,
            yaw: axis(yaw)?,
            roll: axis(roll)?,
        })
    }

    /// Pitch, yaw and roll in degrees; absent axes are zero.
    pub fn degrees(&self) -> [f32; 3] {
        [self.pitch, self.yaw, self.roll].map(|axis| dequantize_angle(axis.unwrap_or(0)))
    }

    fn axes(&self) -> [Option<u16>; 3] {
        [self.pitch, self.yaw, self.roll]
    }

    pub fn summary(&self) -> RotatorSummary {
        RotatorSummary {
            axes_present: self.axes().iter().filter(|axis| axis.is_some()).count(),
        }
    }
}

impl std::fmt::Debug for CompressedRotator {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CompressedRotator")
            .field("pitch", &self.pitch.is_some())
            .field("yaw", &self.yaw.is_some())
            .field("roll", &self.roll.is_some())
            .finish()
    }
}

/// The fields after a dynamic actor GUID (F-569).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DynamicSpawn {
    pub archetype: NetGuidRef,
    /// Present exactly when the codec selects the level reference.
    pub level: Option<NetGuidRef>,
    pub location: Option<SpawnVector>,
    pub rotation: Option<CompressedRotator>,
    pub scale: Option<SpawnVector>,
    pub velocity: Option<SpawnVector>,
}

/// Open-bunch spawn record (F-569).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpawnRecord {
    pub actor: NetGuidRef,
    /// Present exactly when the actor GUID is dynamic.
    pub dynamic: Option<DynamicSpawn>,
}

/// Class of a non-actor content block (F-576).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SubObjectClass {
    /// Stably-named bit set; no class reference follows.
    StablyNamed,
    /// A nonzero class reference.
    Class(NetGuidRef),
    /// A zero class reference: the sub-object is deleted.
    Delete,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BlockTarget {
    Actor,
    SubObject {
        object: NetGuidRef,
        class: SubObjectClass,
    },
}

/// One content block; `payload` bits are opaque replicator input.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContentBlock {
    pub has_rep_layout: bool,
    pub target: BlockTarget,
    pub payload: Payload,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActorBunchBody {
    /// Present exactly when the bunch opens the channel.
    pub spawn: Option<SpawnRecord>,
    pub blocks: Vec<ContentBlock>,
}

#[derive(Clone, Debug, Serialize)]
pub struct GuidSummary {
    pub kind: GuidKind,
    pub export: bool,
    pub no_load: bool,
    pub checksum: bool,
    pub path_units: Option<usize>,
    pub outer: Option<Box<GuidSummary>>,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum VectorForm {
    Raw,
    Quantized,
}

#[derive(Clone, Copy, Debug, Serialize)]
pub struct VectorSummary {
    pub form: VectorForm,
    pub bits: Option<u32>,
}

#[derive(Clone, Copy, Debug, Serialize)]
pub struct RotatorSummary {
    pub axes_present: usize,
}

#[derive(Clone, Debug, Serialize)]
pub struct SpawnSummary {
    pub actor: GuidSummary,
    pub archetype: Option<GuidSummary>,
    pub level: Option<GuidSummary>,
    pub location: Option<VectorSummary>,
    pub rotation: Option<RotatorSummary>,
    pub scale: Option<VectorSummary>,
    pub velocity: Option<VectorSummary>,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BlockKind {
    Actor,
    StablyNamed,
    Class,
    Delete,
}

#[derive(Clone, Debug, Serialize)]
pub struct BlockSummary {
    pub has_rep_layout: bool,
    pub kind: BlockKind,
    pub object: Option<GuidSummary>,
    pub class: Option<GuidSummary>,
    pub payload_bits: usize,
}

/// Kinds, counts and lengths only.
#[derive(Clone, Debug, Serialize)]
pub struct ActorBunchSummary {
    pub spawn: Option<SpawnSummary>,
    pub blocks: Vec<BlockSummary>,
}

impl SpawnRecord {
    pub fn summary(&self) -> SpawnSummary {
        let dynamic = self.dynamic.as_ref();
        SpawnSummary {
            actor: self.actor.summary(),
            archetype: dynamic.map(|spawn| spawn.archetype.summary()),
            level: dynamic
                .and_then(|spawn| spawn.level.as_ref())
                .map(NetGuidRef::summary),
            location: dynamic
                .and_then(|spawn| spawn.location)
                .map(|vector| vector.summary()),
            rotation: dynamic
                .and_then(|spawn| spawn.rotation)
                .map(|rotation| rotation.summary()),
            scale: dynamic
                .and_then(|spawn| spawn.scale)
                .map(|vector| vector.summary()),
            velocity: dynamic
                .and_then(|spawn| spawn.velocity)
                .map(|vector| vector.summary()),
        }
    }
}

impl ContentBlock {
    pub fn summary(&self) -> BlockSummary {
        let (kind, object, class) = match &self.target {
            BlockTarget::Actor => (BlockKind::Actor, None, None),
            BlockTarget::SubObject { object, class } => {
                let (kind, class) = match class {
                    SubObjectClass::StablyNamed => (BlockKind::StablyNamed, None),
                    SubObjectClass::Class(class) => (BlockKind::Class, Some(class.summary())),
                    SubObjectClass::Delete => (BlockKind::Delete, None),
                };
                (kind, Some(object.summary()), class)
            }
        };
        BlockSummary {
            has_rep_layout: self.has_rep_layout,
            kind,
            object,
            class,
            payload_bits: self.payload.bit_len(),
        }
    }
}

impl ActorBunchBody {
    pub fn summary(&self) -> ActorBunchSummary {
        ActorBunchSummary {
            spawn: self.spawn.as_ref().map(SpawnRecord::summary),
            blocks: self.blocks.iter().map(ContentBlock::summary).collect(),
        }
    }
}

fn read_flag(reader: &mut BitReader<'_>, what: &str) -> Result<bool> {
    let bit = reader.read_bit();
    ensure!(!reader.is_error(), "truncated {what}");
    Ok(bit)
}

fn read_capped(reader: &mut BitReader<'_>, max: u32, what: &str) -> Result<u32> {
    let value = reader.read_int_max(max);
    ensure!(!reader.is_error(), "truncated {what}");
    Ok(value)
}

/// Packed integer that must re-encode to exactly the bytes read.
fn read_packed(reader: &mut BitReader<'_>, what: &str) -> Result<u32> {
    let mut raw = [0u8; 5];
    let mut len = 0;
    let mut value = 0u32;
    for group in 0..5u32 {
        let byte = reader.read_bits(8);
        ensure!(!reader.is_error(), "truncated {what}");
        raw[len] = byte as u8;
        len += 1;
        value |= (byte >> 1) << (group * 7);
        if byte & 1 == 0 {
            break;
        }
    }
    let mut canonical = BitWriter::new();
    canonical.write_int_packed(value);
    ensure!(
        canonical.as_bytes() == &raw[..len],
        "non-canonical packed {what} unsupported"
    );
    Ok(value)
}

fn read_u32(reader: &mut BitReader<'_>, order: ByteOrder, what: &str) -> Result<u32> {
    let value = reader.read_u32();
    ensure!(!reader.is_error(), "truncated {what}");
    Ok(match order {
        ByteOrder::Little => value,
        ByteOrder::Big => value.swap_bytes(),
    })
}

fn write_u32(writer: &mut BitWriter, value: u32, order: ByteOrder) {
    writer.write_u32(match order {
        ByteOrder::Little => value,
        ByteOrder::Big => value.swap_bytes(),
    });
}

/// Copy `bits` from the reader; the caller has checked the bound.
fn copy_bits(reader: &mut BitReader<'_>, mut bits: usize) -> Payload {
    let mut writer = BitWriter::new();
    while bits > 0 {
        let chunk = bits.min(32) as u32;
        writer.write_bits(reader.read_bits(chunk), chunk);
        bits -= chunk as usize;
    }
    Payload::from_writer(writer)
}

fn write_payload(writer: &mut BitWriter, payload: &Payload) {
    let mut reader = payload.reader();
    let mut bits = payload.bit_len();
    while bits > 0 {
        let chunk = bits.min(32) as u32;
        writer.write_bits(reader.read_bits(chunk), chunk);
        bits -= chunk as usize;
    }
}

fn read_path(reader: &mut BitReader<'_>, codec: &ActorChannelCodec) -> Result<ArchiveText> {
    let count = read_u32(reader, codec.order, "export path count")? as i32;
    ensure!(count != i32::MIN, "export path count i32::MIN unsupported");
    let units = count.unsigned_abs() as usize;
    ensure!(
        units <= codec.limits.max_path_units,
        "export path exceeds the path unit limit"
    );
    // The cap bounds the allocation; overflowing reads allocate nothing.
    let payload = reader.read_bytes(units * if count < 0 { 2 } else { 1 });
    ensure!(!reader.is_error(), "truncated export path payload");
    let prefix = match codec.order {
        ByteOrder::Little => count.to_le_bytes(),
        ByteOrder::Big => count.to_be_bytes(),
    };
    let frame = [prefix.as_slice(), &payload].concat();
    let parsed = read_string_frame(&frame, codec.order).context("invalid export path")?;
    ensure!(
        parsed.consumed() == frame.len(),
        "export path frame length mismatch"
    );
    let encoding = match parsed.encoding() {
        StringEncoding::Empty => TokenEncoding::Empty,
        StringEncoding::Narrow => TokenEncoding::Narrow,
        StringEncoding::Wide => TokenEncoding::Wide,
    };
    ArchiveText::with_encoding(&parsed.units().collect::<Vec<_>>(), encoding)
}

fn write_path(writer: &mut BitWriter, text: &ArchiveText, codec: &ActorChannelCodec) -> Result<()> {
    let declared = text.declared_units();
    ensure!(
        declared <= codec.limits.max_path_units,
        "export path exceeds the path unit limit"
    );
    let wide = text.encoding() == TokenEncoding::Wide;
    let count = if wide {
        -(declared as i32)
    } else {
        declared as i32
    };
    write_u32(writer, count as u32, codec.order);
    if text.encoding() == TokenEncoding::Empty {
        return Ok(());
    }
    for unit in text.units().iter().copied().chain(std::iter::once(0)) {
        if wide {
            writer.write_bytes(&match codec.order {
                ByteOrder::Little => unit.to_le_bytes(),
                ByteOrder::Big => unit.to_be_bytes(),
            });
        } else {
            writer.write_bits(u32::from(unit), 8);
        }
    }
    Ok(())
}

fn export_expected(guid: u32, codec: &ActorChannelCodec) -> bool {
    guid != 0 && (guid == DEFAULT_GUID || codec.exports_enabled)
}

/// Read one NetGUID reference (F-572).
pub fn read_guid_ref(reader: &mut BitReader<'_>, codec: &ActorChannelCodec) -> Result<NetGuidRef> {
    read_guid_at(reader, codec, 0)
}

fn read_guid_at(
    reader: &mut BitReader<'_>,
    codec: &ActorChannelCodec,
    depth: usize,
) -> Result<NetGuidRef> {
    ensure!(
        depth <= MAX_GUID_DEPTH,
        "NetGUID outer recursion exceeds 16 levels"
    );
    let guid = read_packed(reader, "NetGUID")?;
    if !export_expected(guid, codec) {
        return Ok(NetGuidRef::plain(guid));
    }
    let flags = reader.read_bits(8) as u8;
    ensure!(!reader.is_error(), "truncated NetGUID export flags");
    ensure!(
        flags & !EXPORT_KNOWN == 0,
        "reserved NetGUID export flag bits unsupported"
    );
    ensure!(
        flags & EXPORT_HAS_CHECKSUM == 0 || flags & EXPORT_HAS_PATH != 0,
        "NetGUID checksum without a path unsupported"
    );
    let path = if flags & EXPORT_HAS_PATH != 0 {
        let outer = read_guid_at(reader, codec, depth + 1).context("NetGUID outer")?;
        let path = read_path(reader, codec)?;
        Some(ExportPath {
            outer: Box::new(outer),
            path,
        })
    } else {
        None
    };
    let checksum = if flags & EXPORT_HAS_CHECKSUM != 0 {
        Some(read_u32(reader, codec.order, "NetGUID checksum")?)
    } else {
        None
    };
    Ok(NetGuidRef {
        guid,
        export: Some(GuidExport {
            no_load: flags & EXPORT_NO_LOAD != 0,
            path,
            checksum,
        }),
    })
}

pub fn write_guid_ref(
    writer: &mut BitWriter,
    reference: &NetGuidRef,
    codec: &ActorChannelCodec,
) -> Result<()> {
    write_guid_at(writer, reference, codec, 0)
}

fn write_guid_at(
    writer: &mut BitWriter,
    reference: &NetGuidRef,
    codec: &ActorChannelCodec,
    depth: usize,
) -> Result<()> {
    ensure!(
        depth <= MAX_GUID_DEPTH,
        "NetGUID outer recursion exceeds 16 levels"
    );
    ensure!(
        reference.export.is_some() == export_expected(reference.guid, codec),
        "export flags present exactly for a nonzero default or export-enabled GUID"
    );
    writer.write_int_packed(reference.guid);
    let Some(export) = &reference.export else {
        return Ok(());
    };
    ensure!(
        export.checksum.is_none() || export.path.is_some(),
        "NetGUID checksum without a path unsupported"
    );
    let mut flags = 0u8;
    for (set, bit) in [
        (export.path.is_some(), EXPORT_HAS_PATH),
        (export.no_load, EXPORT_NO_LOAD),
        (export.checksum.is_some(), EXPORT_HAS_CHECKSUM),
    ] {
        if set {
            flags |= bit;
        }
    }
    writer.write_bits(u32::from(flags), 8);
    if let Some(path) = &export.path {
        write_guid_at(writer, &path.outer, codec, depth + 1)?;
        write_path(writer, &path.path, codec)?;
    }
    if let Some(checksum) = export.checksum {
        write_u32(writer, checksum, codec.order);
    }
    Ok(())
}

fn read_vector(
    reader: &mut BitReader<'_>,
    codec: &ActorChannelCodec,
    what: &str,
) -> Result<SpawnVector> {
    let quantized = if codec.engine_net_version >= QUANTIZED_BIT_NET_VERSION {
        read_flag(reader, what)?
    } else {
        true
    };
    if !quantized {
        let mut raw = [0u32; 3];
        for component in &mut raw {
            *component = read_u32(reader, codec.order, what)?;
        }
        return Ok(SpawnVector::Raw(raw));
    }
    let bits = read_capped(reader, VECTOR_BITS_MAX, what)?;
    let max = 1u32 << (bits + 2);
    let mut components = [0u32; 3];
    for component in &mut components {
        *component = read_capped(reader, max, what)?;
    }
    Ok(SpawnVector::Quantized { bits, components })
}

fn write_vector(
    writer: &mut BitWriter,
    vector: &SpawnVector,
    codec: &ActorChannelCodec,
) -> Result<()> {
    let versioned = codec.engine_net_version >= QUANTIZED_BIT_NET_VERSION;
    match vector {
        SpawnVector::Raw(raw) => {
            ensure!(
                versioned,
                "raw spawn vectors need engine net version 13 or later"
            );
            writer.write_bit(false);
            for component in raw {
                write_u32(writer, *component, codec.order);
            }
        }
        SpawnVector::Quantized { bits, components } => {
            ensure!(
                *bits < VECTOR_BITS_MAX,
                "spawn vector bit count must be below 24"
            );
            let max = 1u32 << (bits + 2);
            ensure!(
                components.iter().all(|component| *component < max),
                "spawn vector component exceeds its bit count"
            );
            if versioned {
                writer.write_bit(true);
            }
            writer.write_int_max(*bits, VECTOR_BITS_MAX)?;
            for component in components {
                writer.write_int_max(*component, max)?;
            }
        }
    }
    Ok(())
}

fn read_optional_vector(
    reader: &mut BitReader<'_>,
    codec: &ActorChannelCodec,
    what: &str,
) -> Result<Option<SpawnVector>> {
    if read_flag(reader, what)? {
        Ok(Some(read_vector(reader, codec, what)?))
    } else {
        Ok(None)
    }
}

fn write_optional_vector(
    writer: &mut BitWriter,
    vector: Option<&SpawnVector>,
    codec: &ActorChannelCodec,
) -> Result<()> {
    writer.write_bit(vector.is_some());
    if let Some(vector) = vector {
        write_vector(writer, vector, codec)?;
    }
    Ok(())
}

/// Read a compressed rotator (F-573): per axis a bit, then a 16-bit value.
pub fn read_rotator(reader: &mut BitReader<'_>) -> Result<CompressedRotator> {
    let mut axes = [None; 3];
    for axis in &mut axes {
        if read_flag(reader, "rotator axis")? {
            let value = reader.read_bits(16) as u16;
            ensure!(!reader.is_error(), "truncated rotator axis");
            *axis = Some(value);
        }
    }
    let [pitch, yaw, roll] = axes;
    Ok(CompressedRotator { pitch, yaw, roll })
}

pub fn write_rotator(writer: &mut BitWriter, rotator: &CompressedRotator) {
    for axis in rotator.axes() {
        writer.write_bit(axis.is_some());
        if let Some(value) = axis {
            writer.write_bits(u32::from(value), 16);
        }
    }
}

/// Read the open-bunch spawn record (F-569).
pub fn read_spawn(reader: &mut BitReader<'_>, codec: &ActorChannelCodec) -> Result<SpawnRecord> {
    let actor = read_guid_ref(reader, codec).context("spawn actor")?;
    let dynamic = if actor.kind() == GuidKind::Dynamic {
        let archetype = read_guid_ref(reader, codec).context("spawn archetype")?;
        let level = if codec.has_level {
            Some(read_guid_ref(reader, codec).context("spawn level")?)
        } else {
            None
        };
        let location = read_optional_vector(reader, codec, "spawn location")?;
        let rotation = if read_flag(reader, "spawn rotation")? {
            Some(read_rotator(reader).context("spawn rotation")?)
        } else {
            None
        };
        let scale = read_optional_vector(reader, codec, "spawn scale")?;
        let velocity = read_optional_vector(reader, codec, "spawn velocity")?;
        Some(DynamicSpawn {
            archetype,
            level,
            location,
            rotation,
            scale,
            velocity,
        })
    } else {
        None
    };
    Ok(SpawnRecord { actor, dynamic })
}

pub fn write_spawn(
    writer: &mut BitWriter,
    spawn: &SpawnRecord,
    codec: &ActorChannelCodec,
) -> Result<()> {
    ensure!(
        spawn.dynamic.is_some() == (spawn.actor.kind() == GuidKind::Dynamic),
        "dynamic spawn fields present exactly for a dynamic actor GUID"
    );
    write_guid_ref(writer, &spawn.actor, codec)?;
    let Some(dynamic) = &spawn.dynamic else {
        return Ok(());
    };
    ensure!(
        dynamic.level.is_some() == codec.has_level,
        "spawn level present exactly when the codec selects it"
    );
    write_guid_ref(writer, &dynamic.archetype, codec)?;
    if let Some(level) = &dynamic.level {
        write_guid_ref(writer, level, codec)?;
    }
    write_optional_vector(writer, dynamic.location.as_ref(), codec)?;
    writer.write_bit(dynamic.rotation.is_some());
    if let Some(rotation) = &dynamic.rotation {
        write_rotator(writer, rotation);
    }
    write_optional_vector(writer, dynamic.scale.as_ref(), codec)?;
    write_optional_vector(writer, dynamic.velocity.as_ref(), codec)?;
    Ok(())
}

/// Read one content block header and its opaque payload (F-576). A deletion
/// block (zero class reference) carries no payload count: the client reads the
/// count only when the header did not delete the sub-object (M-424, M-425).
pub fn read_content_block(
    reader: &mut BitReader<'_>,
    codec: &ActorChannelCodec,
) -> Result<ContentBlock> {
    let has_rep_layout = read_flag(reader, "content block rep-layout bit")?;
    let is_actor = read_flag(reader, "content block actor bit")?;
    let target = if is_actor {
        BlockTarget::Actor
    } else {
        let object = read_guid_ref(reader, codec).context("content block sub-object")?;
        let class = if read_flag(reader, "content block stably-named bit")? {
            SubObjectClass::StablyNamed
        } else {
            let class = read_guid_ref(reader, codec).context("content block class")?;
            if class.guid == 0 {
                SubObjectClass::Delete
            } else {
                SubObjectClass::Class(class)
            }
        };
        BlockTarget::SubObject { object, class }
    };
    if matches!(
        target,
        BlockTarget::SubObject {
            class: SubObjectClass::Delete,
            ..
        }
    ) {
        return Ok(ContentBlock {
            has_rep_layout,
            target,
            payload: copy_bits(reader, 0),
        });
    }
    let bits = read_packed(reader, "content block payload bit count")? as usize;
    ensure!(
        bits <= codec.limits.max_payload_bits,
        "content block payload exceeds the payload bit limit"
    );
    ensure!(
        bits <= reader.remaining(),
        "content block payload exceeds the bunch"
    );
    Ok(ContentBlock {
        has_rep_layout,
        target,
        payload: copy_bits(reader, bits),
    })
}

pub fn write_content_block(
    writer: &mut BitWriter,
    block: &ContentBlock,
    codec: &ActorChannelCodec,
) -> Result<()> {
    let bits = block.payload.bit_len();
    ensure!(
        bits <= codec.limits.max_payload_bits,
        "content block payload exceeds the payload bit limit"
    );
    let bits = u32::try_from(bits).context("content block payload exceeds 32 bits")?;
    writer.write_bit(block.has_rep_layout);
    match &block.target {
        BlockTarget::Actor => writer.write_bit(true),
        BlockTarget::SubObject { object, class } => {
            writer.write_bit(false);
            write_guid_ref(writer, object, codec)?;
            match class {
                SubObjectClass::StablyNamed => writer.write_bit(true),
                SubObjectClass::Class(class) => {
                    ensure!(class.guid != 0, "a zero class reference is a deletion");
                    writer.write_bit(false);
                    write_guid_ref(writer, class, codec)?;
                }
                SubObjectClass::Delete => {
                    ensure!(bits == 0, "a deletion block carries no payload");
                    writer.write_bit(false);
                    write_guid_ref(writer, &NetGuidRef::null(), codec)?;
                    return Ok(());
                }
            }
        }
    }
    writer.write_int_packed(bits);
    write_payload(writer, &block.payload);
    Ok(())
}

/// Read a whole actor bunch body: the spawn record when `open`, then content
/// blocks until the reader is exactly at its end.
pub fn read_actor_bunch_body(
    reader: &mut BitReader<'_>,
    open: bool,
    codec: &ActorChannelCodec,
) -> Result<ActorBunchBody> {
    let spawn = if open {
        Some(read_spawn(reader, codec).context("spawn record")?)
    } else {
        None
    };
    let mut blocks = Vec::new();
    while !reader.at_end() {
        let index = blocks.len();
        ensure!(
            index < codec.limits.max_blocks,
            "actor bunch exceeds the content block limit"
        );
        blocks.push(
            read_content_block(reader, codec).with_context(|| format!("content block {index}"))?,
        );
    }
    Ok(ActorBunchBody { spawn, blocks })
}

pub fn write_actor_bunch_body(
    writer: &mut BitWriter,
    body: &ActorBunchBody,
    codec: &ActorChannelCodec,
) -> Result<()> {
    ensure!(
        body.blocks.len() <= codec.limits.max_blocks,
        "actor bunch exceeds the content block limit"
    );
    if let Some(spawn) = &body.spawn {
        write_spawn(writer, spawn, codec).context("spawn record")?;
    }
    for (index, block) in body.blocks.iter().enumerate() {
        write_content_block(writer, block, codec)
            .with_context(|| format!("content block {index}"))?;
    }
    Ok(())
}

/// Encode a body as a bunch payload starting at bit 0.
pub fn encode_actor_bunch_body(
    body: &ActorBunchBody,
    codec: &ActorChannelCodec,
) -> Result<Payload> {
    let mut writer = BitWriter::new();
    write_actor_bunch_body(&mut writer, body, codec)?;
    Ok(Payload::from_writer(writer))
}

/// Parse a complete, unpartitioned bunch's payload as an actor bunch body.
/// Export and must-be-mapped prefixes are unread, so such bunches are rejected.
pub fn parse_actor_bunch(bunch: &Bunch, codec: &ActorChannelCodec) -> Result<ActorBunchBody> {
    let header = &bunch.header;
    ensure!(!header.partial, "partial actor bunches unsupported");
    ensure!(
        !header.has_package_map_exports,
        "NetGUID export bunches unsupported"
    );
    ensure!(
        !header.has_must_be_mapped_guids,
        "must-be-mapped GUID prefixes unsupported"
    );
    let mut reader = bunch.payload.reader();
    read_actor_bunch_body(&mut reader, header.open, codec)
}

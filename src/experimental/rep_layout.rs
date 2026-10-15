//! EXPERIMENTAL disconnected codec for a statically observed rep-layout
//! property stream, against a caller-supplied layout descriptor.
//!
//! Static observations only: the client handle stream (F-571), NetGUID
//! references (F-572), the compressed rotator (F-573), client property net items
//! (F-574), client network names (F-575) and the older server's native struct
//! items (F-582). Nothing here is wire-validated, no original layout is known
//! or shipped, and no property state, object resolution, admission or gameplay
//! occurs. Writers mirror the readers for round-trip research only.
//!
//! Observed grammar: packed handles select layout commands, handle 0 ends a
//! stream, a 4-byte checksum word may follow each handle, and a dynamic array
//! carries a 2-byte element count and a nested zero-terminated handle stream
//! (F-571). Item encodings:
//! - [`RepCommand::Bool`]: one bit (F-574);
//! - [`RepCommand::Byte`]: 8 bits, or for an enum byte a width derived from the
//!   enum maximum, at most 8 bits; the formula is unread and version-split, so
//!   the caller supplies the width (F-574);
//! - [`RepCommand::Enum`]: an enum-derived width, likewise caller-supplied
//!   (F-574);
//! - [`RepCommand::Object`]: a NetGUID reference through
//!   [`super::actor_channel::read_guid_ref`] (F-572, F-574);
//! - [`RepCommand::NetworkName`]: a hardcoded bit, then a hardcoded index or an
//!   archive string and a 4-byte number (F-575);
//! - [`RepCommand::VectorQuantize`]: `FVector_NetQuantize`, a capped component
//!   bit count and three biased integers, unscaled (F-582, server writer);
//! - [`RepCommand::VectorQuantizeNormal`]: three 16-bit values biased by 32768;
//!   the scale constant is unread, so only raw values are kept (F-582, server
//!   writer);
//! - [`RepCommand::Rotator`]: the compressed rotator of
//!   [`super::actor_channel::read_rotator`] (F-573);
//! - [`RepCommand::DynamicArray`]: count and nested stream (F-571);
//! - [`RepCommand::Opaque`]: a caller-sized span of raw bits.
//!
//! Interpretations, not observations:
//! - a checksum word follows every handle read, including the terminating 0
//!   ("each handle"); its value is preserved raw and never checked;
//! - handles follow the client's single sequential walk (F-586): they strictly
//!   increase within a stream, a top-level stream addresses its layout once,
//!   and an array stream numbers `count` copies of the element layout
//!   consecutively, so handle `h` is command `(h - 1) % N` of element
//!   `(h - 1) / N`; a nested array is one handle;
//! - the nested stream is read even for a zero element count (F-586);
//! - the component bit count of `FVector_NetQuantize` ("at most 20") is read as
//!   a capped integer with maximum 20, so it is below 20, like the spawn-vector
//!   field of `actor_channel` (F-573);
//! - the pre-version-6 hardcoded name index ("bounded by 411") is read as a
//!   capped integer with maximum 411; indices above 702 are rejected (F-575);
//!   senders use the hardcoded form only up to 410, but the writer accepts every
//!   readable index so read streams re-encode;
//! - whether Name properties reach the network-name path is unread, so the
//!   caller selects [`RepCommand::NetworkName`] explicitly; whether rotator
//!   properties reach the compressed-rotator helper is likewise inferred;
//! - byte-wide fields follow the bit order of the packet reader; 2-byte counts
//!   and 4-byte fields (checksums, name numbers, string counts) follow the
//!   caller's byte order, as the original byte order is unread; 16-bit item
//!   values are raw 16-bit fields like the rotator's.
//!
//! Unsupported and failing closed: any item the spec does not encode, such as
//! 32-bit integers, floats, strings, text, soft objects, gameplay tag
//! containers, hit results, the movement struct and other native structs
//! ([`UnsupportedItem`]); the optional bit and the preamble before the handle
//! loop (M-426); non-canonical packed integers (the original reader may be more
//! lenient), so every accepted stream re-encodes bit-identically.
//!
//! `Debug` output and summaries carry handles, kinds, counts and lengths only,
//! never values, GUIDs, names, checksums or raw bits.
use anyhow::{Context, Result, bail, ensure};
use serde::Serialize;

use super::actor_channel::{
    ActorChannelCodec, CompressedRotator, GuidSummary, NetGuidRef, RotatorSummary, read_guid_ref,
    read_packed, read_rotator, write_guid_ref, write_rotator,
};
use super::archive::{ByteOrder, StringEncoding, read_string_frame};
use super::control::TokenEncoding;
use super::control_messages::ArchiveText;
use super::packet::{BitReader, BitWriter, MAX_HARDCODED_NAME_INDEX, Payload};

/// Maximum of the capped `FVector_NetQuantize` component bit count (F-582).
pub const NET_QUANTIZE_BITS_MAX: u32 = 20;
/// Bias of `FVector_NetQuantizeNormal` components (F-582).
pub const NET_QUANTIZE_NORMAL_BIAS: u32 = 32_768;
/// First engine net version whose hardcoded name index is packed (F-575).
pub const PACKED_NAME_NET_VERSION: u32 = 6;
/// Bound of the hardcoded name index before net version 6 (F-575).
pub const LEGACY_NAME_INDEX_BOUND: u32 = 411;
/// Highest hardcoded index observed senders use (F-575); informational only.
pub const SENDER_HARDCODED_NAME_MAX: u32 = 410;
/// Widest caller-supplied enum byte (F-574).
pub const ENUM_BYTE_BITS_MAX: u8 = 8;
/// Widest caller-supplied enum item; the original maximum is unread.
pub const ENUM_BITS_MAX: u8 = 32;

/// Research resource policy; the original bounds are unknown.
#[derive(Clone, Copy, Debug)]
pub struct RepLayoutLimits {
    /// Entries accepted per call, counted over every nesting level.
    pub max_entries: usize,
    /// Deepest dynamic-array nesting below the top-level stream.
    pub max_depth: usize,
    /// Highest accepted dynamic-array element count.
    pub max_array_elements: u16,
    /// Bits accepted per opaque span.
    pub max_opaque_bits: usize,
    /// Declared network-name string units, including the terminator.
    pub max_name_units: usize,
}

impl Default for RepLayoutLimits {
    fn default() -> Self {
        Self {
            max_entries: 4096,
            max_depth: 8,
            max_array_elements: 4096,
            max_opaque_bits: 65_536,
            max_name_units: 1024,
        }
    }
}

/// Caller-selected codec parameters; see the module documentation.
#[derive(Clone, Copy, Debug)]
pub struct RepLayoutCodec {
    /// NetGUID export state, engine net version and byte order.
    pub actor: ActorChannelCodec,
    /// A 4-byte checksum word follows every handle (F-571).
    pub checksums: bool,
    pub limits: RepLayoutLimits,
}

/// Item types the spec names without an encoding this codec supports.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UnsupportedItem {
    Int32,
    UInt32,
    Float,
    /// Base item path (F-575); the ordinary serializer is unread.
    Str,
    /// Base item path (F-575); the ordinary serializer is unread.
    Text,
    /// Archive soft-path operator (F-574), unread.
    SoftObject,
    /// Empty bit, count byte and packed tags (F-582); tag packing is unread.
    GameplayTagContainer,
    /// Flags byte and gated fields (F-582); the gated layout is unread.
    HitResult,
    /// Not a public symbol (F-582).
    Movement,
    /// Any other native net-serialize struct.
    OtherNativeStruct,
}

/// One layout command; its handle is its 1-based position in its level.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RepCommand {
    Bool,
    /// `None`: 8 bits. `Some(width)`: a caller-supplied enum width, 1 to 8.
    Byte {
        enum_bits: Option<u8>,
    },
    /// A caller-supplied enum width, 1 to 32.
    Enum {
        bits: u8,
    },
    Object,
    NetworkName,
    VectorQuantize,
    VectorQuantizeNormal,
    Rotator,
    /// The element layout, addressed by the nested stream's handles.
    DynamicArray {
        element: Vec<RepCommand>,
    },
    /// A caller-sized raw span, 1 bit or more.
    Opaque {
        bits: usize,
    },
    /// Any handle selecting this fails closed.
    Unsupported(UnsupportedItem),
}

/// `FVector_NetQuantize` integers as read; no float is applied (F-582).
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct QuantizedVector {
    /// The capped bit-count field, below 20.
    pub bits: u32,
    /// Each below `1 << (bits + 2)`.
    pub components: [u32; 3],
}

impl QuantizedVector {
    /// Components minus the bias `1 << (bits + 1)`; the server writer applies
    /// no scale (F-582). Defined for valid bit counts only.
    pub fn unbiased(&self) -> [i64; 3] {
        let bias = 1i64 << (self.bits.min(NET_QUANTIZE_BITS_MAX) + 1);
        self.components.map(|value| i64::from(value) - bias)
    }
}

impl std::fmt::Debug for QuantizedVector {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("QuantizedVector")
            .field("bits", &self.bits)
            .finish_non_exhaustive()
    }
}

/// Raw `FVector_NetQuantizeNormal` values; the scale constant is unread.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct QuantizedNormal(pub [u16; 3]);

impl QuantizedNormal {
    /// Values minus the 32768 bias (F-582), before the unread scale.
    pub fn unbiased(&self) -> [i32; 3] {
        self.0
            .map(|value| i32::from(value) - NET_QUANTIZE_NORMAL_BIAS as i32)
    }
}

impl std::fmt::Debug for QuantizedNormal {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("QuantizedNormal")
    }
}

/// A network name (F-575).
#[derive(Clone, PartialEq, Eq)]
pub enum NetworkName {
    /// Hardcoded index, at most 702 (below 411 before net version 6).
    Hardcoded(u32),
    String {
        text: ArchiveText,
        number: u32,
    },
}

impl NetworkName {
    pub fn summary(&self) -> NameSummary {
        match self {
            Self::Hardcoded(_) => NameSummary {
                hardcoded: true,
                units: None,
            },
            Self::String { text, .. } => NameSummary {
                hardcoded: false,
                units: Some(text.units().len()),
            },
        }
    }
}

impl std::fmt::Debug for NetworkName {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Hardcoded(_) => formatter.write_str("Hardcoded"),
            Self::String { text, .. } => formatter
                .debug_struct("String")
                .field("text", text)
                .finish_non_exhaustive(),
        }
    }
}

/// A dynamic array: its element count and nested stream (F-571).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RepArray {
    pub count: u16,
    pub stream: PropertyStream,
}

/// One item value; `Debug` shows kinds and lengths only.
#[derive(Clone, PartialEq, Eq)]
pub enum RepValue {
    Bool(bool),
    /// Below `1 << width`.
    Byte(u8),
    /// Below `1 << bits`.
    Enum(u32),
    Object(NetGuidRef),
    NetworkName(NetworkName),
    VectorQuantize(QuantizedVector),
    VectorQuantizeNormal(QuantizedNormal),
    Rotator(CompressedRotator),
    DynamicArray(RepArray),
    Opaque(Payload),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemKind {
    Bool,
    Byte,
    Enum,
    Object,
    NetworkName,
    VectorQuantize,
    VectorQuantizeNormal,
    Rotator,
    DynamicArray,
    Opaque,
}

impl RepValue {
    pub fn kind(&self) -> ItemKind {
        match self {
            Self::Bool(_) => ItemKind::Bool,
            Self::Byte(_) => ItemKind::Byte,
            Self::Enum(_) => ItemKind::Enum,
            Self::Object(_) => ItemKind::Object,
            Self::NetworkName(_) => ItemKind::NetworkName,
            Self::VectorQuantize(_) => ItemKind::VectorQuantize,
            Self::VectorQuantizeNormal(_) => ItemKind::VectorQuantizeNormal,
            Self::Rotator(_) => ItemKind::Rotator,
            Self::DynamicArray(_) => ItemKind::DynamicArray,
            Self::Opaque(_) => ItemKind::Opaque,
        }
    }
}

impl std::fmt::Debug for RepValue {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Bool(_) => formatter.write_str("Bool"),
            Self::Byte(_) => formatter.write_str("Byte"),
            Self::Enum(_) => formatter.write_str("Enum"),
            Self::Object(guid) => formatter.debug_tuple("Object").field(guid).finish(),
            Self::NetworkName(name) => formatter.debug_tuple("NetworkName").field(name).finish(),
            Self::VectorQuantize(vector) => formatter
                .debug_tuple("VectorQuantize")
                .field(vector)
                .finish(),
            Self::VectorQuantizeNormal(normal) => formatter
                .debug_tuple("VectorQuantizeNormal")
                .field(normal)
                .finish(),
            Self::Rotator(rotator) => formatter.debug_tuple("Rotator").field(rotator).finish(),
            Self::DynamicArray(array) => {
                formatter.debug_tuple("DynamicArray").field(array).finish()
            }
            Self::Opaque(payload) => formatter.debug_tuple("Opaque").field(payload).finish(),
        }
    }
}

/// One handle and its value.
#[derive(Clone, PartialEq, Eq)]
pub struct RepEntry {
    /// 1-based position in the stream's sequential walk; in an array stream,
    /// element `(handle - 1) / N` and command `(handle - 1) % N` of an
    /// `N`-command element layout (F-586).
    pub handle: u32,
    /// Present exactly when the codec selects checksums; raw and unchecked.
    pub checksum: Option<u32>,
    pub value: RepValue,
}

impl std::fmt::Debug for RepEntry {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RepEntry")
            .field("handle", &self.handle)
            .field("checksum", &self.checksum.is_some())
            .field("value", &self.value)
            .finish()
    }
}

/// One zero-terminated handle stream.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct PropertyStream {
    pub entries: Vec<RepEntry>,
    /// The checksum after the terminating handle 0, when selected.
    pub terminator_checksum: Option<u32>,
}

impl std::fmt::Debug for PropertyStream {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PropertyStream")
            .field("entries", &self.entries)
            .field("terminator_checksum", &self.terminator_checksum.is_some())
            .finish()
    }
}

#[derive(Clone, Copy, Debug, Serialize)]
pub struct NameSummary {
    pub hardcoded: bool,
    pub units: Option<usize>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ArraySummary {
    pub count: u16,
    pub stream: PropertyStreamSummary,
}

/// Handle, kind and lengths; no value.
#[derive(Clone, Debug, Serialize)]
pub struct EntrySummary {
    pub handle: u32,
    pub kind: ItemKind,
    pub checksum: bool,
    pub object: Option<GuidSummary>,
    pub name: Option<NameSummary>,
    pub vector_bits: Option<u32>,
    pub rotator: Option<RotatorSummary>,
    pub opaque_bits: Option<usize>,
    pub array: Option<Box<ArraySummary>>,
}

#[derive(Clone, Debug, Serialize)]
pub struct PropertyStreamSummary {
    pub entries: Vec<EntrySummary>,
    pub terminator_checksum: bool,
}

impl RepEntry {
    pub fn summary(&self) -> EntrySummary {
        let mut summary = EntrySummary {
            handle: self.handle,
            kind: self.value.kind(),
            checksum: self.checksum.is_some(),
            object: None,
            name: None,
            vector_bits: None,
            rotator: None,
            opaque_bits: None,
            array: None,
        };
        match &self.value {
            RepValue::Object(guid) => summary.object = Some(guid.summary()),
            RepValue::NetworkName(name) => summary.name = Some(name.summary()),
            RepValue::VectorQuantize(vector) => summary.vector_bits = Some(vector.bits),
            RepValue::Rotator(rotator) => summary.rotator = Some(rotator.summary()),
            RepValue::Opaque(payload) => summary.opaque_bits = Some(payload.bit_len()),
            RepValue::DynamicArray(array) => {
                summary.array = Some(Box::new(ArraySummary {
                    count: array.count,
                    stream: array.stream.summary(),
                }));
            }
            RepValue::Bool(_)
            | RepValue::Byte(_)
            | RepValue::Enum(_)
            | RepValue::VectorQuantizeNormal(_) => {}
        }
        summary
    }
}

impl PropertyStream {
    pub fn summary(&self) -> PropertyStreamSummary {
        PropertyStreamSummary {
            entries: self.entries.iter().map(RepEntry::summary).collect(),
            terminator_checksum: self.terminator_checksum.is_some(),
        }
    }

    /// Entries over every nesting level.
    pub fn total_entries(&self) -> usize {
        self.entries
            .iter()
            .map(|entry| {
                1 + match &entry.value {
                    RepValue::DynamicArray(array) => array.stream.total_entries(),
                    _ => 0,
                }
            })
            .sum()
    }
}

/// Validate widths, opaque sizes and nesting depth of a caller layout.
pub fn validate_layout(layout: &[RepCommand], limits: &RepLayoutLimits) -> Result<()> {
    validate_layout_at(layout, limits, 0)
}

fn validate_layout_at(layout: &[RepCommand], limits: &RepLayoutLimits, depth: usize) -> Result<()> {
    ensure!(
        depth <= limits.max_depth,
        "layout nesting exceeds the depth limit"
    );
    ensure!(
        u32::try_from(layout.len()).is_ok(),
        "layout exceeds the handle range"
    );
    for (index, command) in layout.iter().enumerate() {
        let handle = index + 1;
        match command {
            RepCommand::Byte {
                enum_bits: Some(bits),
            } => ensure!(
                (1..=ENUM_BYTE_BITS_MAX).contains(bits),
                "layout handle {handle}: enum byte width must be 1 to 8"
            ),
            RepCommand::Enum { bits } => ensure!(
                (1..=ENUM_BITS_MAX).contains(bits),
                "layout handle {handle}: enum width must be 1 to 32"
            ),
            RepCommand::Opaque { bits } => ensure!(
                (1..=limits.max_opaque_bits).contains(bits),
                "layout handle {handle}: opaque span must be 1 bit to the opaque limit"
            ),
            RepCommand::DynamicArray { element } => {
                validate_layout_at(element, limits, depth + 1)
                    .with_context(|| format!("layout handle {handle}"))?;
            }
            RepCommand::Bool
            | RepCommand::Byte { enum_bits: None }
            | RepCommand::Object
            | RepCommand::NetworkName
            | RepCommand::VectorQuantize
            | RepCommand::VectorQuantizeNormal
            | RepCommand::Rotator
            | RepCommand::Unsupported(_) => {}
        }
    }
    Ok(())
}

/// Resolve a handle the way the client's single sequential walk does (F-586):
/// handles strictly increase within a stream, and a stream over `instances`
/// copies of the layout (1 at top level, the element count in an array)
/// numbers their commands consecutively, so handle `h` is command
/// `(h - 1) % layout.len()` of instance `(h - 1) / layout.len()`.
fn command_at(
    layout: &[RepCommand],
    handle: u32,
    previous: u32,
    instances: usize,
) -> Result<&RepCommand> {
    ensure!(
        handle > previous,
        "property handle {handle} does not follow handle {previous}"
    );
    let index = usize::try_from(handle - 1)?;
    ensure!(
        !layout.is_empty() && index / layout.len() < instances,
        "unknown property handle {handle}"
    );
    Ok(&layout[index % layout.len()])
}

fn read_flag(reader: &mut BitReader<'_>, what: &str) -> Result<bool> {
    let bit = reader.read_bit();
    ensure!(!reader.is_error(), "truncated {what}");
    Ok(bit)
}

fn read_field(reader: &mut BitReader<'_>, bits: u32, what: &str) -> Result<u32> {
    let value = reader.read_bits(bits);
    ensure!(!reader.is_error(), "truncated {what}");
    Ok(value)
}

fn read_capped(reader: &mut BitReader<'_>, max: u32, what: &str) -> Result<u32> {
    let value = reader.read_int_max(max);
    ensure!(!reader.is_error(), "truncated {what}");
    Ok(value)
}

fn read_u16(reader: &mut BitReader<'_>, order: ByteOrder, what: &str) -> Result<u16> {
    let value = read_field(reader, 16, what)? as u16;
    Ok(match order {
        ByteOrder::Little => value,
        ByteOrder::Big => value.swap_bytes(),
    })
}

fn write_u16(writer: &mut BitWriter, value: u16, order: ByteOrder) {
    let value = match order {
        ByteOrder::Little => value,
        ByteOrder::Big => value.swap_bytes(),
    };
    writer.write_bits(u32::from(value), 16);
}

fn read_u32(reader: &mut BitReader<'_>, order: ByteOrder, what: &str) -> Result<u32> {
    let value = read_field(reader, 32, what)?;
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

fn copy_bits(reader: &mut BitReader<'_>, mut bits: usize) -> Payload {
    let mut writer = BitWriter::new();
    while bits > 0 {
        let chunk = bits.min(32) as u32;
        writer.write_bits(reader.read_bits(chunk), chunk);
        bits -= chunk as usize;
    }
    Payload::from_writer(writer)
}

fn append_bits(writer: &mut BitWriter, payload: &Payload) {
    let mut reader = payload.reader();
    let mut bits = payload.bit_len();
    while bits > 0 {
        let chunk = bits.min(32) as u32;
        writer.write_bits(reader.read_bits(chunk), chunk);
        bits -= chunk as usize;
    }
}

fn read_name_string(reader: &mut BitReader<'_>, codec: &RepLayoutCodec) -> Result<ArchiveText> {
    let order = codec.actor.order;
    let count = read_u32(reader, order, "network name string count")? as i32;
    ensure!(
        count != i32::MIN,
        "network name string count i32::MIN unsupported"
    );
    let units = count.unsigned_abs() as usize;
    ensure!(
        units <= codec.limits.max_name_units,
        "network name string exceeds the name unit limit"
    );
    // The cap bounds the allocation; overflowing reads allocate nothing.
    let payload = reader.read_bytes(units * if count < 0 { 2 } else { 1 });
    ensure!(!reader.is_error(), "truncated network name string payload");
    let prefix = match order {
        ByteOrder::Little => count.to_le_bytes(),
        ByteOrder::Big => count.to_be_bytes(),
    };
    let frame = [prefix.as_slice(), &payload].concat();
    let parsed = read_string_frame(&frame, order).context("invalid network name string")?;
    ensure!(
        parsed.consumed() == frame.len(),
        "network name string frame length mismatch"
    );
    let encoding = match parsed.encoding() {
        StringEncoding::Empty => TokenEncoding::Empty,
        StringEncoding::Narrow => TokenEncoding::Narrow,
        StringEncoding::Wide => TokenEncoding::Wide,
    };
    ArchiveText::with_encoding(&parsed.units().collect::<Vec<_>>(), encoding)
}

fn write_name_string(
    writer: &mut BitWriter,
    text: &ArchiveText,
    codec: &RepLayoutCodec,
) -> Result<()> {
    let order = codec.actor.order;
    let declared = text.declared_units();
    ensure!(
        declared <= codec.limits.max_name_units,
        "network name string exceeds the name unit limit"
    );
    let wide = text.encoding() == TokenEncoding::Wide;
    let count = if wide {
        -(declared as i32)
    } else {
        declared as i32
    };
    write_u32(writer, count as u32, order);
    if text.encoding() == TokenEncoding::Empty {
        return Ok(());
    }
    for unit in text.units().iter().copied().chain(std::iter::once(0)) {
        if wide {
            writer.write_bytes(&match order {
                ByteOrder::Little => unit.to_le_bytes(),
                ByteOrder::Big => unit.to_be_bytes(),
            });
        } else {
            writer.write_bits(u32::from(unit), 8);
        }
    }
    Ok(())
}

/// Read one network name (F-575).
pub fn read_network_name(
    reader: &mut BitReader<'_>,
    codec: &RepLayoutCodec,
) -> Result<NetworkName> {
    if !read_flag(reader, "network name hardcoded bit")? {
        let text = read_name_string(reader, codec)?;
        let number = read_u32(reader, codec.actor.order, "network name number")?;
        return Ok(NetworkName::String { text, number });
    }
    let index = if codec.actor.engine_net_version >= PACKED_NAME_NET_VERSION {
        read_packed(reader, "hardcoded name index")?
    } else {
        read_capped(reader, LEGACY_NAME_INDEX_BOUND, "hardcoded name index")?
    };
    ensure!(
        index <= MAX_HARDCODED_NAME_INDEX,
        "hardcoded name index exceeds 702"
    );
    Ok(NetworkName::Hardcoded(index))
}

pub fn write_network_name(
    writer: &mut BitWriter,
    name: &NetworkName,
    codec: &RepLayoutCodec,
) -> Result<()> {
    match name {
        NetworkName::Hardcoded(index) => {
            ensure!(
                *index <= MAX_HARDCODED_NAME_INDEX,
                "hardcoded name index exceeds 702"
            );
            writer.write_bit(true);
            if codec.actor.engine_net_version >= PACKED_NAME_NET_VERSION {
                writer.write_int_packed(*index);
            } else {
                ensure!(
                    *index < LEGACY_NAME_INDEX_BOUND,
                    "hardcoded name index must be below 411 before net version 6"
                );
                writer.write_int_max(*index, LEGACY_NAME_INDEX_BOUND)?;
            }
        }
        NetworkName::String { text, number } => {
            writer.write_bit(false);
            write_name_string(writer, text, codec)?;
            write_u32(writer, *number, codec.actor.order);
        }
    }
    Ok(())
}

fn read_checksum(reader: &mut BitReader<'_>, codec: &RepLayoutCodec) -> Result<Option<u32>> {
    if codec.checksums {
        Ok(Some(read_u32(
            reader,
            codec.actor.order,
            "handle checksum",
        )?))
    } else {
        Ok(None)
    }
}

fn write_checksum(
    writer: &mut BitWriter,
    checksum: Option<u32>,
    codec: &RepLayoutCodec,
) -> Result<()> {
    ensure!(
        checksum.is_some() == codec.checksums,
        "handle checksum present exactly when the codec selects checksums"
    );
    if let Some(checksum) = checksum {
        write_u32(writer, checksum, codec.actor.order);
    }
    Ok(())
}

struct Budget {
    remaining: usize,
}

impl Budget {
    fn take(&mut self) -> Result<()> {
        ensure!(
            self.remaining > 0,
            "property stream exceeds the entry limit"
        );
        self.remaining -= 1;
        Ok(())
    }
}

fn byte_width(enum_bits: Option<u8>) -> u32 {
    u32::from(enum_bits.unwrap_or(ENUM_BYTE_BITS_MAX))
}

fn read_value(
    reader: &mut BitReader<'_>,
    command: &RepCommand,
    codec: &RepLayoutCodec,
    depth: usize,
    budget: &mut Budget,
) -> Result<RepValue> {
    Ok(match command {
        RepCommand::Bool => RepValue::Bool(read_flag(reader, "bool item")?),
        RepCommand::Byte { enum_bits } => {
            RepValue::Byte(read_field(reader, byte_width(*enum_bits), "byte item")? as u8)
        }
        RepCommand::Enum { bits } => {
            RepValue::Enum(read_field(reader, u32::from(*bits), "enum item")?)
        }
        RepCommand::Object => {
            RepValue::Object(read_guid_ref(reader, &codec.actor).context("object item")?)
        }
        RepCommand::NetworkName => RepValue::NetworkName(read_network_name(reader, codec)?),
        RepCommand::VectorQuantize => {
            let bits = read_capped(reader, NET_QUANTIZE_BITS_MAX, "quantized vector bit count")?;
            let max = 1u32 << (bits + 2);
            let mut components = [0u32; 3];
            for component in &mut components {
                *component = read_capped(reader, max, "quantized vector component")?;
            }
            RepValue::VectorQuantize(QuantizedVector { bits, components })
        }
        RepCommand::VectorQuantizeNormal => {
            let mut values = [0u16; 3];
            for value in &mut values {
                *value = read_field(reader, 16, "quantized normal component")? as u16;
            }
            RepValue::VectorQuantizeNormal(QuantizedNormal(values))
        }
        RepCommand::Rotator => RepValue::Rotator(read_rotator(reader).context("rotator item")?),
        RepCommand::DynamicArray { element } => {
            let count = read_u16(reader, codec.actor.order, "dynamic array count")?;
            ensure!(
                count <= codec.limits.max_array_elements,
                "dynamic array count exceeds the element limit"
            );
            let stream = read_stream(
                reader,
                element,
                usize::from(count),
                codec,
                depth + 1,
                budget,
            )
            .context("dynamic array stream")?;
            RepValue::DynamicArray(RepArray { count, stream })
        }
        RepCommand::Opaque { bits } => {
            ensure!(
                *bits <= codec.limits.max_opaque_bits,
                "opaque span exceeds the opaque bit limit"
            );
            ensure!(*bits <= reader.remaining(), "truncated opaque span");
            RepValue::Opaque(copy_bits(reader, *bits))
        }
        RepCommand::Unsupported(item) => bail!("unsupported property item {item:?}"),
    })
}

fn read_stream(
    reader: &mut BitReader<'_>,
    layout: &[RepCommand],
    instances: usize,
    codec: &RepLayoutCodec,
    depth: usize,
    budget: &mut Budget,
) -> Result<PropertyStream> {
    ensure!(
        depth <= codec.limits.max_depth,
        "property stream nesting exceeds the depth limit"
    );
    let mut entries: Vec<RepEntry> = Vec::new();
    loop {
        let handle = read_packed(reader, "property handle")?;
        let checksum = read_checksum(reader, codec)?;
        if handle == 0 {
            return Ok(PropertyStream {
                entries,
                terminator_checksum: checksum,
            });
        }
        let previous = entries.last().map_or(0, |entry| entry.handle);
        let command = command_at(layout, handle, previous, instances)?;
        budget.take()?;
        let value = read_value(reader, command, codec, depth, budget)
            .with_context(|| format!("property handle {handle}"))?;
        entries.push(RepEntry {
            handle,
            checksum,
            value,
        });
    }
}

/// Read one top-level property stream through its terminating handle 0,
/// leaving the reader after it.
pub fn read_property_stream(
    reader: &mut BitReader<'_>,
    layout: &[RepCommand],
    codec: &RepLayoutCodec,
) -> Result<PropertyStream> {
    validate_layout(layout, &codec.limits)?;
    let mut budget = Budget {
        remaining: codec.limits.max_entries,
    };
    read_stream(reader, layout, 1, codec, 0, &mut budget)
}

/// Parse a payload that must hold exactly one property stream.
pub fn parse_property_stream(
    payload: &Payload,
    layout: &[RepCommand],
    codec: &RepLayoutCodec,
) -> Result<PropertyStream> {
    let mut reader = payload.reader();
    let stream = read_property_stream(&mut reader, layout, codec)?;
    ensure!(reader.at_end(), "trailing bits after the property stream");
    Ok(stream)
}

fn write_value(
    writer: &mut BitWriter,
    command: &RepCommand,
    value: &RepValue,
    codec: &RepLayoutCodec,
    depth: usize,
    budget: &mut Budget,
) -> Result<()> {
    match (command, value) {
        (RepCommand::Bool, RepValue::Bool(bit)) => writer.write_bit(*bit),
        (RepCommand::Byte { enum_bits }, RepValue::Byte(byte)) => {
            let width = byte_width(*enum_bits);
            ensure!(
                u32::from(*byte) >> width == 0,
                "byte item exceeds its width"
            );
            writer.write_bits(u32::from(*byte), width);
        }
        (RepCommand::Enum { bits }, RepValue::Enum(item)) => {
            let width = u32::from(*bits);
            ensure!(
                width >= 32 || *item >> width == 0,
                "enum item exceeds its width"
            );
            writer.write_bits(*item, width);
        }
        (RepCommand::Object, RepValue::Object(guid)) => {
            write_guid_ref(writer, guid, &codec.actor).context("object item")?;
        }
        (RepCommand::NetworkName, RepValue::NetworkName(name)) => {
            write_network_name(writer, name, codec)?;
        }
        (RepCommand::VectorQuantize, RepValue::VectorQuantize(vector)) => {
            ensure!(
                vector.bits < NET_QUANTIZE_BITS_MAX,
                "quantized vector bit count must be below 20"
            );
            let max = 1u32 << (vector.bits + 2);
            ensure!(
                vector.components.iter().all(|component| *component < max),
                "quantized vector component exceeds its bit count"
            );
            writer.write_int_max(vector.bits, NET_QUANTIZE_BITS_MAX)?;
            for component in vector.components {
                writer.write_int_max(component, max)?;
            }
        }
        (RepCommand::VectorQuantizeNormal, RepValue::VectorQuantizeNormal(normal)) => {
            for value in normal.0 {
                writer.write_bits(u32::from(value), 16);
            }
        }
        (RepCommand::Rotator, RepValue::Rotator(rotator)) => write_rotator(writer, rotator),
        (RepCommand::DynamicArray { element }, RepValue::DynamicArray(array)) => {
            ensure!(
                array.count <= codec.limits.max_array_elements,
                "dynamic array count exceeds the element limit"
            );
            write_u16(writer, array.count, codec.actor.order);
            write_stream(
                writer,
                &array.stream,
                element,
                usize::from(array.count),
                codec,
                depth + 1,
                budget,
            )
            .context("dynamic array stream")?;
        }
        (RepCommand::Opaque { bits }, RepValue::Opaque(payload)) => {
            ensure!(
                payload.bit_len() == *bits,
                "opaque span length differs from its layout"
            );
            append_bits(writer, payload);
        }
        (RepCommand::Unsupported(item), _) => bail!("unsupported property item {item:?}"),
        (_, value) => bail!("{:?} value does not match its layout command", value.kind()),
    }
    Ok(())
}

fn write_stream(
    writer: &mut BitWriter,
    stream: &PropertyStream,
    layout: &[RepCommand],
    instances: usize,
    codec: &RepLayoutCodec,
    depth: usize,
    budget: &mut Budget,
) -> Result<()> {
    ensure!(
        depth <= codec.limits.max_depth,
        "property stream nesting exceeds the depth limit"
    );
    let mut previous = 0;
    for entry in &stream.entries {
        ensure!(entry.handle != 0, "handle 0 is the stream terminator");
        let command = command_at(layout, entry.handle, previous, instances)?;
        previous = entry.handle;
        budget.take()?;
        writer.write_int_packed(entry.handle);
        write_checksum(writer, entry.checksum, codec)?;
        write_value(writer, command, &entry.value, codec, depth, budget)
            .with_context(|| format!("property handle {}", entry.handle))?;
    }
    writer.write_int_packed(0);
    write_checksum(writer, stream.terminator_checksum, codec)
}

/// Validate a stream against its layout and append its bits. Nothing is
/// appended when validation fails.
pub fn write_property_stream(
    writer: &mut BitWriter,
    stream: &PropertyStream,
    layout: &[RepCommand],
    codec: &RepLayoutCodec,
) -> Result<()> {
    let payload = encode_property_stream(stream, layout, codec)?;
    append_bits(writer, &payload);
    Ok(())
}

/// Encode a stream as a payload starting at bit 0.
pub fn encode_property_stream(
    stream: &PropertyStream,
    layout: &[RepCommand],
    codec: &RepLayoutCodec,
) -> Result<Payload> {
    validate_layout(layout, &codec.limits)?;
    let mut budget = Budget {
        remaining: codec.limits.max_entries,
    };
    let mut writer = BitWriter::new();
    write_stream(&mut writer, stream, layout, 1, codec, 0, &mut budget)?;
    Ok(Payload::from_writer(writer))
}

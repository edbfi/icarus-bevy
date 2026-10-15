//! EXPERIMENTAL disconnected codec for a statically observed rep-layout
//! property stream, against a caller-supplied layout descriptor.
//!
//! Static observations only: the client handle stream (F-571), NetGUID
//! references (F-572), the compressed rotator (F-573), client property net items
//! (F-574), client network names (F-575) and the older server's native struct
//! items (F-593), the movement struct and its quantization levels (F-588,
//! F-589) and gameplay tags (F-591, F-592). Nothing here is wire-validated, no original layout is known
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
//!   bit count and three biased integers, unscaled (F-593, server writer);
//! - [`RepCommand::VectorQuantizeNormal`]: three 16-bit values biased by 32768;
//!   the scale constant is unread, so only raw values are kept (F-593, server
//!   writer);
//! - [`RepCommand::VectorQuantize10`] and [`RepCommand::VectorQuantize100`]:
//!   the same shape with bit counts below 24 and 30, read times 0.1 and 0.01
//!   (F-589);
//! - [`RepCommand::Movement`]: `FRepMovement` (F-588): two flag bits, location,
//!   rotation, linear velocity and, when flag bit 1 is set, angular velocity,
//!   each at a quantization level the layout supplies, because the levels are
//!   struct fields and never cross the wire (F-589);
//! - [`RepCommand::GameplayTag`]: a network name, or a net index packed in a
//!   caller-supplied first segment and total width (F-591);
//! - [`RepCommand::GameplayTagContainer`]: an empty bit, then a count of a
//!   caller-supplied width and that many tags (F-592);
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
//! - the byte rotator's byte, like each movement field, follows the packet
//!   reader's bit order; a level other than 1 or 2 is read as level 0, as the
//!   observed helper does, so the layout names only the three packings;
//! - which property uses which movement level is a property default and stays
//!   caller-supplied;
//! - tag widths and the replication mode come from tag configuration, which
//!   this codec neither reads nor ships (U-065): the caller supplies them, and
//!   an index is never resolved to a tag; the per-connection table path is
//!   not supported; a name-mode tag uses the network-name path (inferred);
//! - whether Name properties reach the network-name path is unread, so the
//!   caller selects [`RepCommand::NetworkName`] explicitly; whether rotator
//!   properties reach the compressed-rotator helper is likewise inferred;
//! - byte-wide fields follow the bit order of the packet reader; 2-byte counts
//!   and 4-byte fields (checksums, name numbers, string counts) follow the
//!   caller's byte order, as the original byte order is unread; 16-bit item
//!   values are raw 16-bit fields like the rotator's.
//!
//! Unsupported and failing closed: any item the spec does not encode, such as
//! 32-bit integers, floats, strings, text, soft objects, hit results and other
//! native structs ([`UnsupportedItem`]); the optional bit and the preamble
//! before the handle loop (M-426); non-canonical packed integers and tag net
//! indices (the original reader may be more lenient), so every accepted stream
//! re-encodes bit-identically.
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

/// Maximum of the capped `FVector_NetQuantize` component bit count (F-593).
pub const NET_QUANTIZE_BITS_MAX: u32 = 20;
/// Maximum of the `FVector_NetQuantize10` bit count (F-589).
pub const NET_QUANTIZE10_BITS_MAX: u32 = 24;
/// Maximum of the `FVector_NetQuantize100` bit count (F-589).
pub const NET_QUANTIZE100_BITS_MAX: u32 = 30;
/// Bias of `FVector_NetQuantizeNormal` components (F-593).
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
    /// Flags byte and gated fields (F-593); the gated layout is unread.
    HitResult,
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
    VectorQuantize10,
    VectorQuantize100,
    VectorQuantizeNormal,
    Rotator,
    /// One gameplay tag in the caller's encoding (F-591, F-592).
    GameplayTag(TagEncoding),
    /// A gameplay tag container (F-592); `count_bits` is 1 to 8.
    GameplayTagContainer {
        encoding: TagEncoding,
        count_bits: u8,
    },
    /// `FRepMovement` at the property's quantization levels (F-588, F-589).
    Movement {
        location: MovementLevel,
        velocity: MovementLevel,
        rotation: RotationLevel,
    },
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

/// A capped-bit-count vector packing (F-593, F-589): the bit-count maximum and
/// the divisor the reader scales the unbiased integers by.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum VectorPacking {
    /// `FVector_NetQuantize`: below 20, unscaled.
    NetQuantize,
    /// `FVector_NetQuantize10`: below 24, divided by 10.
    NetQuantize10,
    /// `FVector_NetQuantize100`: below 30, divided by 100.
    NetQuantize100,
    /// Movement level 0: below 24, unscaled.
    MovementWhole,
    /// Movement level 1: below 27, divided by 10.
    MovementTenths,
    /// Movement level 2: below 30, divided by 100.
    MovementHundredths,
}

impl VectorPacking {
    /// The bit count is below this value.
    pub fn bits_max(self) -> u32 {
        match self {
            Self::NetQuantize => NET_QUANTIZE_BITS_MAX,
            Self::NetQuantize10 | Self::MovementWhole => NET_QUANTIZE10_BITS_MAX,
            Self::MovementTenths => 27,
            Self::NetQuantize100 | Self::MovementHundredths => NET_QUANTIZE100_BITS_MAX,
        }
    }

    /// The reader multiplies by the reciprocal of this divisor (F-589).
    pub fn divisor(self) -> u32 {
        match self {
            Self::NetQuantize | Self::MovementWhole => 1,
            Self::NetQuantize10 | Self::MovementTenths => 10,
            Self::NetQuantize100 | Self::MovementHundredths => 100,
        }
    }
}

/// How a gameplay tag crosses the wire (F-591, F-592); both depend on tag
/// configuration the caller supplies.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TagEncoding {
    /// Fast replication off: a network name.
    Name,
    /// Fast replication on: a net index of `total_bits` (1 to 16), split after
    /// `first_segment` bits when that is 1 to `total_bits - 1`.
    NetIndex { first_segment: u8, total_bits: u8 },
}

/// One gameplay tag as read; an index is never resolved to a tag.
#[derive(Clone, PartialEq, Eq)]
pub enum GameplayTag {
    Name(NetworkName),
    NetIndex(u16),
}

impl std::fmt::Debug for GameplayTag {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Name(name) => formatter.debug_tuple("Name").field(name).finish(),
            Self::NetIndex(_) => formatter.write_str("NetIndex"),
        }
    }
}

/// Tag net index (F-591): `total` raw bits when `first` is 0 or at least
/// `total`; otherwise `first` low bits and a continuation bit, then the
/// `total - first` high bits when the continuation bit is set.
pub fn read_tag_net_index(reader: &mut BitReader<'_>, first: u8, total: u8) -> Result<u16> {
    ensure!((1..=16).contains(&total), "tag index width must be 1 to 16");
    let (first, total) = (u32::from(first), u32::from(total));
    if first == 0 || first >= total {
        return Ok(read_field(reader, total, "tag net index")? as u16);
    }
    let low = read_field(reader, first + 1, "tag net index")?;
    let index = if low >> first & 1 == 0 {
        low
    } else {
        let high = read_field(reader, total - first, "tag net index high bits")?;
        // The original reader accepts a zero high part, but its writer never
        // sends one; rejecting it keeps every accepted stream re-encodable.
        ensure!(high != 0, "non-canonical tag net index");
        low & ((1 << first) - 1) | high << first
    };
    Ok(index as u16)
}

/// Mirror of [`read_tag_net_index`]; the index must fit `total` bits.
pub fn write_tag_net_index(writer: &mut BitWriter, index: u16, first: u8, total: u8) -> Result<()> {
    ensure!((1..=16).contains(&total), "tag index width must be 1 to 16");
    let (first, total, index) = (u32::from(first), u32::from(total), u32::from(index));
    ensure!(index >> total == 0, "tag net index exceeds its width");
    if first == 0 || first >= total {
        writer.write_bits(index, total);
    } else if index < 1 << first {
        writer.write_bits(index, first + 1);
    } else {
        writer.write_bits(index & ((1 << first) - 1) | 1 << first, first + 1);
        writer.write_bits(index >> first, total - first);
    }
    Ok(())
}

/// A movement vector quantization level (F-589); the value is a struct field
/// and never crosses the wire.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MovementLevel {
    /// Level 0, and every level other than 1 and 2.
    Whole,
    Tenths,
    Hundredths,
}

impl MovementLevel {
    pub fn packing(self) -> VectorPacking {
        match self {
            Self::Whole => VectorPacking::MovementWhole,
            Self::Tenths => VectorPacking::MovementTenths,
            Self::Hundredths => VectorPacking::MovementHundredths,
        }
    }
}

/// A movement rotation quantization level (F-588).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RotationLevel {
    /// Level 0: the byte rotator.
    Bytes,
    /// Level 1: the 16-bit compressed rotator (F-573).
    Shorts,
    /// Any other level: no rotation is written.
    Omitted,
}

/// Byte rotator (F-589): per axis, `None` when the bit is clear, else the raw
/// byte. `Some(0)` is preserved as read, although the observed writer sets the
/// bit only for nonzero bytes.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub struct ByteRotator {
    pub pitch: Option<u8>,
    pub yaw: Option<u8>,
    pub roll: Option<u8>,
}

/// `round(degrees * 256 / 360)` modulo 256, rounding half up (inferred, F-589).
pub fn quantize_byte_angle(degrees: f32) -> Result<u8> {
    ensure!(degrees.is_finite(), "rotator angle must be finite");
    let scaled = (degrees * 256.0 / 360.0 + 0.5).floor();
    ensure!(
        scaled.abs() < 2f32.powi(62),
        "rotator angle is out of the quantization range"
    );
    Ok((scaled as i64 & 0xff) as u8)
}

/// `value * 360 / 256` degrees (F-589).
pub fn dequantize_byte_angle(value: u8) -> f32 {
    f32::from(value) * 360.0 / 256.0
}

impl ByteRotator {
    /// Canonical encoding: an axis is present exactly when its byte is nonzero.
    pub fn from_degrees(pitch: f32, yaw: f32, roll: f32) -> Result<Self> {
        let axis =
            |degrees| quantize_byte_angle(degrees).map(|value| (value != 0).then_some(value));
        Ok(Self {
            pitch: axis(pitch)?,
            yaw: axis(yaw)?,
            roll: axis(roll)?,
        })
    }

    /// Pitch, yaw and roll in degrees; absent axes are zero.
    pub fn degrees(&self) -> [f32; 3] {
        self.axes()
            .map(|axis| dequantize_byte_angle(axis.unwrap_or(0)))
    }

    fn axes(&self) -> [Option<u8>; 3] {
        [self.pitch, self.yaw, self.roll]
    }

    pub fn axes_present(&self) -> usize {
        self.axes().iter().filter(|axis| axis.is_some()).count()
    }
}

impl std::fmt::Debug for ByteRotator {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ByteRotator")
            .field("axes_present", &self.axes_present())
            .finish()
    }
}

/// A movement rotation as read at its level.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum MovementRotation {
    Bytes(ByteRotator),
    Shorts(CompressedRotator),
}

impl std::fmt::Debug for MovementRotation {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Bytes(rotator) => formatter.debug_tuple("Bytes").field(rotator).finish(),
            Self::Shorts(rotator) => formatter.debug_tuple("Shorts").field(rotator).finish(),
        }
    }
}

/// `FRepMovement` as read (F-588); integers only, no float is applied.
#[derive(Clone, PartialEq, Eq)]
pub struct RepMovement {
    /// The two flag bits, below 4; bit 1 gates the angular velocity (F-588).
    /// The stock layout names bit 0 the physics-sleep flag and bit 1 the
    /// replicate-physics flag (inferred).
    pub flags: u8,
    pub location: QuantizedVector,
    /// Present exactly when the rotation level is bytes or shorts.
    pub rotation: Option<MovementRotation>,
    pub linear_velocity: QuantizedVector,
    /// Present exactly when flag bit 1 is set.
    pub angular_velocity: Option<QuantizedVector>,
}

/// Flag bit that gates the angular velocity (F-588).
pub const MOVEMENT_ANGULAR_FLAG: u8 = 2;

impl std::fmt::Debug for RepMovement {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RepMovement")
            .field("rotation", &self.rotation)
            .field("angular_velocity", &self.angular_velocity.is_some())
            .finish_non_exhaustive()
    }
}

/// Capped-bit-count vector integers as read; no float is applied (F-593,
/// F-589).
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct QuantizedVector {
    /// The capped bit-count field, below its packing's maximum.
    pub bits: u32,
    /// Each below `1 << (bits + 2)`.
    pub components: [u32; 3],
}

impl QuantizedVector {
    /// Components minus the bias `1 << (bits + 1)`, before the packing's
    /// divisor (F-593, F-589). Defined for valid bit counts only.
    pub fn unbiased(&self) -> [i64; 3] {
        let bias = 1i64 << (self.bits.min(NET_QUANTIZE100_BITS_MAX) + 1);
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
    /// Values minus the 32768 bias (F-593), before the unread scale.
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
    VectorQuantize10(QuantizedVector),
    VectorQuantize100(QuantizedVector),
    VectorQuantizeNormal(QuantizedNormal),
    Rotator(CompressedRotator),
    GameplayTag(GameplayTag),
    /// `None` when the empty bit is set; otherwise the counted tags.
    GameplayTagContainer(Option<Vec<GameplayTag>>),
    Movement(RepMovement),
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
    VectorQuantize10,
    VectorQuantize100,
    VectorQuantizeNormal,
    Rotator,
    GameplayTag,
    GameplayTagContainer,
    Movement,
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
            Self::VectorQuantize10(_) => ItemKind::VectorQuantize10,
            Self::VectorQuantize100(_) => ItemKind::VectorQuantize100,
            Self::VectorQuantizeNormal(_) => ItemKind::VectorQuantizeNormal,
            Self::Rotator(_) => ItemKind::Rotator,
            Self::GameplayTag(_) => ItemKind::GameplayTag,
            Self::GameplayTagContainer(_) => ItemKind::GameplayTagContainer,
            Self::Movement(_) => ItemKind::Movement,
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
            Self::VectorQuantize10(vector) => formatter
                .debug_tuple("VectorQuantize10")
                .field(vector)
                .finish(),
            Self::VectorQuantize100(vector) => formatter
                .debug_tuple("VectorQuantize100")
                .field(vector)
                .finish(),
            Self::VectorQuantizeNormal(normal) => formatter
                .debug_tuple("VectorQuantizeNormal")
                .field(normal)
                .finish(),
            Self::Rotator(rotator) => formatter.debug_tuple("Rotator").field(rotator).finish(),
            Self::GameplayTag(tag) => formatter.debug_tuple("GameplayTag").field(tag).finish(),
            Self::GameplayTagContainer(tags) => formatter
                .debug_tuple("GameplayTagContainer")
                .field(&tags.as_ref().map(Vec::len))
                .finish(),
            Self::Movement(movement) => formatter.debug_tuple("Movement").field(movement).finish(),
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
    pub movement: Option<MovementSummary>,
    /// Container tag count; `None` for an empty-bit container or another kind.
    pub tag_count: Option<usize>,
    pub opaque_bits: Option<usize>,
    pub array: Option<Box<ArraySummary>>,
}

/// Movement shape only: which optional parts are present.
#[derive(Clone, Debug, Serialize)]
pub struct MovementSummary {
    pub rotation_axes_present: Option<usize>,
    pub angular_velocity: bool,
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
            movement: None,
            tag_count: None,
            opaque_bits: None,
            array: None,
        };
        match &self.value {
            RepValue::Object(guid) => summary.object = Some(guid.summary()),
            RepValue::NetworkName(name) => summary.name = Some(name.summary()),
            RepValue::VectorQuantize(vector)
            | RepValue::VectorQuantize10(vector)
            | RepValue::VectorQuantize100(vector) => summary.vector_bits = Some(vector.bits),
            RepValue::Movement(movement) => {
                summary.movement = Some(MovementSummary {
                    rotation_axes_present: movement.rotation.map(|rotation| match rotation {
                        MovementRotation::Bytes(rotator) => rotator.axes_present(),
                        MovementRotation::Shorts(rotator) => rotator.summary().axes_present,
                    }),
                    angular_velocity: movement.angular_velocity.is_some(),
                });
            }
            RepValue::Rotator(rotator) => summary.rotator = Some(rotator.summary()),
            RepValue::Opaque(payload) => summary.opaque_bits = Some(payload.bit_len()),
            RepValue::DynamicArray(array) => {
                summary.array = Some(Box::new(ArraySummary {
                    count: array.count,
                    stream: array.stream.summary(),
                }));
            }
            RepValue::GameplayTag(GameplayTag::Name(name)) => summary.name = Some(name.summary()),
            RepValue::GameplayTagContainer(tags) => {
                summary.tag_count = tags.as_ref().map(Vec::len);
            }
            RepValue::Bool(_)
            | RepValue::Byte(_)
            | RepValue::Enum(_)
            | RepValue::GameplayTag(GameplayTag::NetIndex(_))
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
            RepCommand::GameplayTag(encoding) => validate_tag_encoding(*encoding)
                .with_context(|| format!("layout handle {handle}"))?,
            RepCommand::GameplayTagContainer {
                encoding,
                count_bits,
            } => {
                validate_tag_encoding(*encoding)
                    .with_context(|| format!("layout handle {handle}"))?;
                ensure!(
                    (1..=8).contains(count_bits),
                    "layout handle {handle}: tag count width must be 1 to 8"
                );
            }
            RepCommand::DynamicArray { element } => {
                validate_layout_at(element, limits, depth + 1)
                    .with_context(|| format!("layout handle {handle}"))?;
            }
            RepCommand::Bool
            | RepCommand::Byte { enum_bits: None }
            | RepCommand::Object
            | RepCommand::NetworkName
            | RepCommand::VectorQuantize
            | RepCommand::VectorQuantize10
            | RepCommand::VectorQuantize100
            | RepCommand::VectorQuantizeNormal
            | RepCommand::Rotator
            | RepCommand::Movement { .. }
            | RepCommand::Unsupported(_) => {}
        }
    }
    Ok(())
}

fn validate_tag_encoding(encoding: TagEncoding) -> Result<()> {
    if let TagEncoding::NetIndex { total_bits, .. } = encoding {
        ensure!(
            (1..=16).contains(&total_bits),
            "tag index width must be 1 to 16"
        );
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

fn read_vector(reader: &mut BitReader<'_>, packing: VectorPacking) -> Result<QuantizedVector> {
    let bits = read_capped(reader, packing.bits_max(), "quantized vector bit count")?;
    let max = 1u32 << (bits + 2);
    let mut components = [0u32; 3];
    for component in &mut components {
        *component = read_capped(reader, max, "quantized vector component")?;
    }
    Ok(QuantizedVector { bits, components })
}

fn write_vector(
    writer: &mut BitWriter,
    vector: &QuantizedVector,
    packing: VectorPacking,
) -> Result<()> {
    let bits_max = packing.bits_max();
    ensure!(
        vector.bits < bits_max,
        "quantized vector bit count must be below {bits_max}"
    );
    let max = 1u32 << (vector.bits + 2);
    ensure!(
        vector.components.iter().all(|component| *component < max),
        "quantized vector component exceeds its bit count"
    );
    writer.write_int_max(vector.bits, bits_max)?;
    for component in vector.components {
        writer.write_int_max(component, max)?;
    }
    Ok(())
}

/// Byte rotator (F-589): per axis a presence bit, then 8 bits when set.
pub fn read_byte_rotator(reader: &mut BitReader<'_>) -> Result<ByteRotator> {
    let mut axes = [None; 3];
    for axis in &mut axes {
        if read_flag(reader, "byte rotator axis")? {
            *axis = Some(read_field(reader, 8, "byte rotator axis")? as u8);
        }
    }
    let [pitch, yaw, roll] = axes;
    Ok(ByteRotator { pitch, yaw, roll })
}

pub fn write_byte_rotator(writer: &mut BitWriter, rotator: &ByteRotator) {
    for axis in rotator.axes() {
        writer.write_bit(axis.is_some());
        if let Some(value) = axis {
            writer.write_bits(u32::from(value), 8);
        }
    }
}

/// `FRepMovement` (F-588): flags, location, rotation, linear velocity, then
/// angular velocity at the velocity level when flag bit 1 is set.
pub fn read_movement(
    reader: &mut BitReader<'_>,
    location: MovementLevel,
    velocity: MovementLevel,
    rotation: RotationLevel,
) -> Result<RepMovement> {
    let flags = read_field(reader, 2, "movement flags")? as u8;
    let location = read_vector(reader, location.packing()).context("movement location")?;
    let rotation = match rotation {
        RotationLevel::Bytes => Some(MovementRotation::Bytes(read_byte_rotator(reader)?)),
        RotationLevel::Shorts => Some(MovementRotation::Shorts(
            read_rotator(reader).context("movement rotation")?,
        )),
        RotationLevel::Omitted => None,
    };
    let linear_velocity =
        read_vector(reader, velocity.packing()).context("movement linear velocity")?;
    let angular_velocity = if flags & MOVEMENT_ANGULAR_FLAG != 0 {
        Some(read_vector(reader, velocity.packing()).context("movement angular velocity")?)
    } else {
        None
    };
    Ok(RepMovement {
        flags,
        location,
        rotation,
        linear_velocity,
        angular_velocity,
    })
}

/// Mirror of [`read_movement`]; parts must match the flags and levels.
pub fn write_movement(
    writer: &mut BitWriter,
    movement: &RepMovement,
    location: MovementLevel,
    velocity: MovementLevel,
    rotation: RotationLevel,
) -> Result<()> {
    ensure!(movement.flags < 4, "movement flags exceed two bits");
    ensure!(
        movement.angular_velocity.is_some() == (movement.flags & MOVEMENT_ANGULAR_FLAG != 0),
        "movement angular velocity must be present exactly when flag bit 1 is set"
    );
    writer.write_bits(u32::from(movement.flags), 2);
    write_vector(writer, &movement.location, location.packing()).context("movement location")?;
    match (rotation, &movement.rotation) {
        (RotationLevel::Bytes, Some(MovementRotation::Bytes(rotator))) => {
            write_byte_rotator(writer, rotator);
        }
        (RotationLevel::Shorts, Some(MovementRotation::Shorts(rotator))) => {
            write_rotator(writer, rotator);
        }
        (RotationLevel::Omitted, None) => {}
        _ => bail!("movement rotation does not match its rotation level"),
    }
    write_vector(writer, &movement.linear_velocity, velocity.packing())
        .context("movement linear velocity")?;
    if let Some(angular) = &movement.angular_velocity {
        write_vector(writer, angular, velocity.packing()).context("movement angular velocity")?;
    }
    Ok(())
}

fn read_tag(
    reader: &mut BitReader<'_>,
    encoding: TagEncoding,
    codec: &RepLayoutCodec,
) -> Result<GameplayTag> {
    Ok(match encoding {
        TagEncoding::Name => GameplayTag::Name(read_network_name(reader, codec)?),
        TagEncoding::NetIndex {
            first_segment,
            total_bits,
        } => GameplayTag::NetIndex(read_tag_net_index(reader, first_segment, total_bits)?),
    })
}

fn write_tag(
    writer: &mut BitWriter,
    tag: &GameplayTag,
    encoding: TagEncoding,
    codec: &RepLayoutCodec,
) -> Result<()> {
    match (encoding, tag) {
        (TagEncoding::Name, GameplayTag::Name(name)) => write_network_name(writer, name, codec),
        (
            TagEncoding::NetIndex {
                first_segment,
                total_bits,
            },
            GameplayTag::NetIndex(index),
        ) => write_tag_net_index(writer, *index, first_segment, total_bits),
        _ => bail!("gameplay tag does not match its encoding"),
    }
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
            RepValue::VectorQuantize(read_vector(reader, VectorPacking::NetQuantize)?)
        }
        RepCommand::VectorQuantize10 => {
            RepValue::VectorQuantize10(read_vector(reader, VectorPacking::NetQuantize10)?)
        }
        RepCommand::VectorQuantize100 => {
            RepValue::VectorQuantize100(read_vector(reader, VectorPacking::NetQuantize100)?)
        }
        RepCommand::GameplayTag(encoding) => {
            RepValue::GameplayTag(read_tag(reader, *encoding, codec).context("gameplay tag")?)
        }
        RepCommand::GameplayTagContainer {
            encoding,
            count_bits,
        } => {
            validate_tag_encoding(*encoding)?;
            ensure!(
                (1..=8).contains(count_bits),
                "tag count width must be 1 to 8"
            );
            if read_flag(reader, "tag container empty bit")? {
                RepValue::GameplayTagContainer(None)
            } else {
                let count = read_field(reader, u32::from(*count_bits), "tag container count")?;
                let mut tags = Vec::with_capacity(count as usize);
                for _ in 0..count {
                    budget.take()?;
                    tags.push(read_tag(reader, *encoding, codec).context("container tag")?);
                }
                RepValue::GameplayTagContainer(Some(tags))
            }
        }
        RepCommand::Movement {
            location,
            velocity,
            rotation,
        } => RepValue::Movement(
            read_movement(reader, *location, *velocity, *rotation).context("movement item")?,
        ),
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
            write_vector(writer, vector, VectorPacking::NetQuantize)?;
        }
        (RepCommand::VectorQuantize10, RepValue::VectorQuantize10(vector)) => {
            write_vector(writer, vector, VectorPacking::NetQuantize10)?;
        }
        (RepCommand::VectorQuantize100, RepValue::VectorQuantize100(vector)) => {
            write_vector(writer, vector, VectorPacking::NetQuantize100)?;
        }
        (RepCommand::GameplayTag(encoding), RepValue::GameplayTag(tag)) => {
            write_tag(writer, tag, *encoding, codec).context("gameplay tag")?;
        }
        (
            RepCommand::GameplayTagContainer {
                encoding,
                count_bits,
            },
            RepValue::GameplayTagContainer(tags),
        ) => {
            ensure!(
                (1..=8).contains(count_bits),
                "tag count width must be 1 to 8"
            );
            writer.write_bit(tags.is_none());
            if let Some(tags) = tags {
                ensure!(
                    tags.len() >> count_bits == 0,
                    "tag container count exceeds its width"
                );
                writer.write_bits(tags.len() as u32, u32::from(*count_bits));
                for tag in tags {
                    budget.take()?;
                    write_tag(writer, tag, *encoding, codec).context("container tag")?;
                }
            }
        }
        (
            RepCommand::Movement {
                location,
                velocity,
                rotation,
            },
            RepValue::Movement(movement),
        ) => {
            write_movement(writer, movement, *location, *velocity, *rotation)
                .context("movement item")?;
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

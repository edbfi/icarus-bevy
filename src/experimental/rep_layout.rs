//! EXPERIMENTAL disconnected codec for a statically observed rep-layout
//! property stream, against a caller-supplied layout descriptor.
//!
//! Static observations only: the client handle stream (F-571), NetGUID
//! references (F-572), the compressed rotator (F-573), client property net items
//! (F-574), client network names (F-575) and the older server's native struct
//! items (F-593), the movement struct and its quantization levels (F-588,
//! F-589), gameplay tags (F-591, F-592), native property command expansion,
//! numeric property items and native struct serializers (F-704), and ICARUS
//! `FRowHandle`, typed `F*RowHandle` and `FStatPairRepState` net serializers
//! (F-705). Nothing here is gameplay-integrated, and no property state, object
//! resolution or admission occurs. Writers mirror the readers for round-trip
//! research only.
//!
//! Observed grammar: `FRepLayout::ReceiveProperties` reads a 1-bit `bDoChecksum`
//! flag before the handle stream (M-426, F-704); packed handles select layout
//! commands, handle 0 ends a stream, a 4-byte checksum word follows each handle
//! when `bDoChecksum` is set, and a dynamic array carries a 2-byte element count
//! and a nested zero-terminated handle stream (F-571). Item encodings:
//! - [`RepCommand::Bool`]: one bit (F-574);
//! - [`RepCommand::Byte`]: 8 bits, or for an enum byte the width
//!   [`enum_byte_bits`] derives from the enum's maximum value (F-574, F-632);
//! - [`RepCommand::Enum`]: the width [`enum_bits`] derives from the maximum and
//!   the underlying integer's size (F-574, F-632); a zero width sends no bits;
//! - [`RepCommand::Int8`], [`RepCommand::Int16`], [`RepCommand::UInt16`],
//!   [`RepCommand::Int`], [`RepCommand::UInt32`], [`RepCommand::Int64`],
//!   [`RepCommand::UInt64`], [`RepCommand::Float`] and [`RepCommand::Double`]:
//!   `FNumericProperty::NetSerializeItem` (`0x1421206f0`) delegates to
//!   `TProperty_Numeric::SerializeItem` (`0x142123c50`, `0x142123c90`,
//!   `0x142123ce0`, `0x142123d30`), which reads 1, 2, 4 or 8 archive bytes in
//!   the caller's byte order (F-704); float and double values are kept as raw
//!   bit words (`u32` and `u64`);
//! - [`RepCommand::Object`]: a NetGUID reference through
//!   [`super::actor_channel::read_guid_ref`] (F-572, F-574);
//! - [`RepCommand::NetworkName`]: a hardcoded bit, then a hardcoded index or an
//!   archive string and a 4-byte number (F-575); Name items reach it through
//!   the net archive and the package map (F-595);
//! - [`RepCommand::Str`]: an archive string, a signed 4-byte count then narrow
//!   bytes or 2-byte units with a terminator (F-595);
//! - [`RepCommand::SoftObject`]: the soft-object path as one archive string,
//!   which net archives send in place of the path fields (F-599);
//! - [`RepCommand::Text`]: the shared FText serializer at the archive versions
//!   client bunches carry (F-639), in
//!   [`super::rep_text`]: every history;
//! - [`RepCommand::Vector`]: `FVector::NetSerialize` (`0x142100d00`), three
//!   4-byte IEEE-754 float words in the caller's byte order (F-704);
//! - [`RepCommand::Vector2D`]: `FVector2D::NetSerialize` (`0x142100bf0`), two
//!   4-byte IEEE-754 float words in the caller's byte order (F-704);
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
//! - [`RepCommand::HitResult`]: a flags byte, a 4-byte time, quantized and
//!   normal vectors with flag-gated duplicates, flag-gated 4-byte and 1-byte
//!   fields, three NetGUID references and a network name (F-597);
//! - [`RepCommand::Rotator`]: `FRotator::NetSerialize` (`0x142100890`), the
//!   compressed rotator of [`super::actor_channel::read_rotator`] (F-573,
//!   F-704);
//! - [`RepCommand::UniqueNetId`] — `FUniqueNetIdRepl::NetSerialize`
//!   (`0x143ddced0` -> `0x1439a23f0`), the replicated unique net id of
//!   [`super::control_messages::read_unique_net_id`] (F-531, F-636, F-704);
//! - [`RepCommand::RowHandle`]: `FRowHandle::NetSerialize` (`0x143f7dbb0`,
//!   F-705), a 2-byte data-table index followed by 0, 1, 2 or 4 row-index bytes
//!   determined by [`icarus_row_handle_width_bytes`]; typed derived
//!   `F*RowHandle` structs (`FModifierStatesRowHandle`, `FBiomesRowHandle`,
//!   etc.) have table-specific `NetSerialize` implementations that omit the
//!   2-byte table index and serialize only their 1-byte (`Byte`) or 2-byte
//!   (`UInt16`) row index (F-705);
//! - [`RepCommand::StatPairRepState`] — `FStatPairRepState::NetSerialize`
//!   (`0x141c10c50` -> `0x1417df1a0`, F-705), two length-prefixed big-endian
//!   32-bit integers (`Stat` and `Value`) serialized through
//!   `Detail::ReadInteger` (`0x1417e1b50`) / `Detail::WriteInteger`
//!   (`0x141807bd0`);
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
//! - tag widths and the replication mode come from tag configuration
//!   ([`super::tag_dictionary`], F-702): the caller supplies them, and an index
//!   is never resolved to a tag inside the low-level stream reader; the
//!   per-connection table path is not supported; a name-mode tag uses the
//!   network-name path, as the archive's name operator does (F-595);
//! - byte-wide fields follow the bit order of the packet reader; 2-, 4- and
//!   8-byte archive fields follow the caller's byte order; 16-bit bit-reader
//!   values are raw 16-bit fields like the rotator's.
//!
//! Unsupported and failing closed: any item the spec does not encode, such as
//! unmodeled native net-serialize structs ([`UnsupportedItem`]); non-canonical
//! packed integers, tag net indices and stat variable-length integers (the
//! original reader may be more lenient), so every accepted stream re-encodes
//! bit-identically.
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
use super::control_messages::{
    ArchiveText, ControlCodec, UniqueNetId, UniqueNetIdSummary, read_unique_net_id,
    write_unique_net_id,
};
use super::packet::{BitReader, BitWriter, MAX_HARDCODED_NAME_INDEX, Payload};
pub use super::rep_text::TextValue;
use super::rep_text::{TextHistory, read_text, write_text};

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
/// Widest enum byte (F-574).
pub const ENUM_BYTE_BITS_MAX: u8 = 8;
/// Widest enum item this codec carries; 8-byte underlying integers can need
/// up to 64 bits, which it rejects.
pub const ENUM_BITS_MAX: u8 = 32;

/// The client's enum maximum: the signed maximum of the entry values, 0 for
/// an enum without entries (F-632). Entries are the enum's registered values.
pub fn enum_max_value(values: &[i64]) -> i64 {
    values.iter().copied().max().unwrap_or(0)
}

/// `CeilLogTwo64(max + 1)`: the maximum's unsigned bit length, except 0 when
/// `max + 1` wraps to 0, so a maximum of -1 or 0 takes no bits (F-632).
fn enum_value_bits(max_value: i64) -> u32 {
    let max = max_value as u64;
    if max == u64::MAX {
        0
    } else {
        u64::BITS - max.leading_zeros()
    }
}

/// Width of an enum byte item at the client's engine net version 17 (above
/// 16): the maximum's bit length, at most 8 (F-632).
pub fn enum_byte_bits(max_value: i64) -> u8 {
    enum_value_bits(max_value).min(u32::from(ENUM_BYTE_BITS_MAX)) as u8
}

/// Width of an enum item at engine net version 17 (12 or above): the
/// maximum's bit length, at most the underlying integer's bits (F-632).
pub fn enum_bits(max_value: i64, underlying_bytes: u8) -> u8 {
    enum_value_bits(max_value).min(u32::from(underlying_bytes) * 8) as u8
}

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
    /// Declared Str and soft-object path units, including the terminator; the
    /// original bound is the archive's maximum serialize size, unread.
    pub max_string_units: usize,
}

impl Default for RepLayoutLimits {
    fn default() -> Self {
        Self {
            max_entries: 4096,
            max_depth: 8,
            max_array_elements: 4096,
            max_opaque_bits: 65_536,
            max_name_units: 1024,
            max_string_units: 65_536,
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
    /// Any other native net-serialize struct.
    OtherNativeStruct,
}

/// One layout command; its handle is its 1-based position in its level.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RepCommand {
    Bool,
    /// `None`: 8 bits. `Some(width)`: an enum byte's width, 0 to 8 (see
    /// [`enum_byte_bits`]).
    Byte {
        enum_bits: Option<u8>,
    },
    /// An enum's width, 0 to 32 (see [`enum_bits`]).
    Enum {
        bits: u8,
    },
    /// `FInt8Property`: 1 signed archive byte (F-704).
    Int8,
    /// `FInt16Property`: 2 signed archive bytes (F-704).
    Int16,
    /// `FUInt16Property`: 2 unsigned archive bytes (F-704).
    UInt16,
    /// `FIntProperty`: 4 signed archive bytes (F-704).
    Int,
    /// `FUInt32Property`: 4 unsigned archive bytes (F-704).
    UInt32,
    /// `FInt64Property`: 8 signed archive bytes (F-704).
    Int64,
    /// `FUInt64Property`: 8 unsigned archive bytes (F-704).
    UInt64,
    /// `FFloatProperty`: 4-byte IEEE-754 float word (F-704).
    Float,
    /// `FDoubleProperty`: 8-byte IEEE-754 double word (F-704).
    Double,
    Object,
    NetworkName,
    /// An archive string through the Str item serializer (F-595).
    Str,
    /// A soft-object path as an archive string (F-599).
    SoftObject,
    /// A Text item (F-639).
    Text,
    /// `FVector` (`0x142100d00`): three 4-byte float words (F-704).
    Vector,
    /// `FVector2D` (`0x142100bf0`): two 4-byte float words (F-704).
    Vector2D,
    VectorQuantize,
    VectorQuantize10,
    VectorQuantize100,
    VectorQuantizeNormal,
    Rotator,
    /// `FUniqueNetIdRepl` (`0x143ddced0` -> `0x1439a23f0`, F-531, F-636, F-704).
    UniqueNetId,
    /// `FRowHandle` (`0x143f7dbb0`, F-705), for properties typed directly as
    /// `FRowHandle` (`HasNetSerializer == true`).
    RowHandle,
    /// `FStatPairRepState` (`0x141c10c50` -> `0x1417df1a0`, F-705), the element
    /// struct of `FStatsRepArray` (`UIcarusStatContainer::ReplicatedStatArray`).
    StatPairRepState,
    /// One gameplay tag in the caller's encoding (F-591, F-592).
    GameplayTag(TagEncoding),
    /// A gameplay tag container (F-592); `count_bits` is 1 to 8.
    GameplayTagContainer {
        encoding: TagEncoding,
        count_bits: u8,
    },
    /// `FHitResult` (F-597).
    HitResult,
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

/// Number of static data-table names in the `FRowHandle` global registry
/// (`0x1408a76d0` -> `0x1464e26e0`, `0x125 = 293` entries, F-705).
pub const ICARUS_ROW_HANDLE_TABLE_COUNT: u16 = 293;
/// Unmapped `FRowHandle` data-table index (`0xffff`, F-705).
pub const ICARUS_ROW_HANDLE_NONE: u16 = u16::MAX;

/// Registered `FRowHandle` table indices whose packaged `DataTable` has 0 rows
/// (`D_BlueprintUnlocks = 62`, `D_CharacterPerks = 164`,
/// `D_QuestVocalisationModifiers = 181`, F-705).
pub const ICARUS_ZERO_ROW_TABLES: [u16; 3] = [62, 164, 181];

/// Registered `FRowHandle` table indices whose packaged `DataTable` has 255 to
/// 65534 rows and therefore serializes a 2-byte row index (F-705).
pub const ICARUS_TWO_BYTE_ROW_TABLES: [u16; 28] = [
    0, 29, 54, 64, 65, 73, 80, 84, 92, 112, 134, 135, 136, 148, 152, 177, 178, 201, 207, 209, 215,
    230, 234, 237, 248, 252, 254, 278,
];

/// Row-index byte width (`0`, `1`, `2` or `4`) selected by
/// `FRowHandle::NetSerialize` (`0x143f7dbb0`) for `table_index` (F-705).
pub fn icarus_row_handle_width_bytes(table_index: u16) -> Result<u8> {
    if table_index == ICARUS_ROW_HANDLE_NONE {
        return Ok(0);
    }
    ensure!(
        table_index < ICARUS_ROW_HANDLE_TABLE_COUNT,
        "unknown row handle data table index {table_index}"
    );
    if ICARUS_ZERO_ROW_TABLES.binary_search(&table_index).is_ok() {
        Ok(0)
    } else if ICARUS_TWO_BYTE_ROW_TABLES
        .binary_search(&table_index)
        .is_ok()
    {
        Ok(2)
    } else {
        Ok(1)
    }
}

/// Replicated `FRowHandle` (`0x143f7dbb0`, F-705): a 2-byte data-table index and,
/// when the table has at least 1 row, the row index.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct RowHandle {
    pub table_index: u16,
    pub row_index: Option<u32>,
}

impl std::fmt::Debug for RowHandle {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("RowHandle")
    }
}

/// Replicated `FStatPairRepState` (`0x141c10c50` -> `0x1417df1a0`, F-705):
/// `Stat` (`+0x0c`) and `Value` (`+0x10`), each serialized as a 1-byte length
/// (`1..=4`) followed by that many big-endian bytes.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct StatPairRepState {
    pub stat: u32,
    pub value: i32,
}

impl std::fmt::Debug for StatPairRepState {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("StatPairRepState")
    }
}

/// Canonical byte length (`1..=4`) written by `Detail::WriteInteger`
/// (`0x141807bd0`, F-705) for a 32-bit word: `< 0xff -> 1`, `< 0xffff -> 2`,
/// `< 0xffffff -> 3`, else `4`.
pub fn stat_int_len_bytes(value: u32) -> u8 {
    if value < 0xff {
        1
    } else if value < 0xffff {
        2
    } else if value < 0xff_ffff {
        3
    } else {
        4
    }
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

/// Hit-result flag bits that replace a field with a default (F-597); bits 0
/// and 1 are carried struct flags, and the writer never sets bit 7.
pub const HIT_IMPACT_POINT_IS_LOCATION: u8 = 1 << 2;
pub const HIT_IMPACT_NORMAL_IS_NORMAL: u8 = 1 << 3;
pub const HIT_NO_ITEM: u8 = 1 << 4;
pub const HIT_NO_FACE_INDEX: u8 = 1 << 5;
pub const HIT_NO_PENETRATION: u8 = 1 << 6;
pub const HIT_NO_ELEMENT_INDEX: u8 = 1 << 7;

/// `FHitResult` as read (F-597): integers and raw 4-byte words, never floats.
/// Field roles follow the stock layout (inferred); each `Option` is `None`
/// exactly when its flag bit replaces it.
#[derive(Clone, PartialEq, Eq)]
pub struct HitResult {
    pub flags: u8,
    pub time: u32,
    pub location: QuantizedVector,
    pub normal: QuantizedNormal,
    pub impact_point: Option<QuantizedVector>,
    pub impact_normal: Option<QuantizedNormal>,
    pub trace_start: QuantizedVector,
    pub trace_end: QuantizedVector,
    pub penetration_depth: Option<u32>,
    pub item: Option<u32>,
    pub phys_material: NetGuidRef,
    pub actor: NetGuidRef,
    pub component: NetGuidRef,
    pub bone_name: NetworkName,
    pub face_index: Option<u32>,
    pub element_index: Option<u8>,
}

impl std::fmt::Debug for HitResult {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("HitResult")
            .field("actor", &self.actor)
            .field("bone_name", &self.bone_name)
            .finish_non_exhaustive()
    }
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
    Int8(i8),
    Int16(i16),
    UInt16(u16),
    Int(i32),
    UInt32(u32),
    Int64(i64),
    UInt64(u64),
    /// Raw IEEE-754 `f32` bits.
    Float(u32),
    /// Raw IEEE-754 `f64` bits.
    Double(u64),
    Object(NetGuidRef),
    NetworkName(NetworkName),
    Str(ArchiveText),
    SoftObject(ArchiveText),
    Text(TextValue),
    /// Three raw IEEE-754 `f32` bit words (`[x, y, z]`).
    Vector([u32; 3]),
    /// Two raw IEEE-754 `f32` bit words (`[x, y]`).
    Vector2D([u32; 2]),
    VectorQuantize(QuantizedVector),
    VectorQuantize10(QuantizedVector),
    VectorQuantize100(QuantizedVector),
    VectorQuantizeNormal(QuantizedNormal),
    Rotator(CompressedRotator),
    UniqueNetId(UniqueNetId),
    RowHandle(RowHandle),
    StatPairRepState(StatPairRepState),
    GameplayTag(GameplayTag),
    HitResult(Box<HitResult>),
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
    Int8,
    Int16,
    UInt16,
    Int,
    UInt32,
    Int64,
    UInt64,
    Float,
    Double,
    Object,
    NetworkName,
    Str,
    SoftObject,
    Text,
    Vector,
    Vector2D,
    VectorQuantize,
    VectorQuantize10,
    VectorQuantize100,
    VectorQuantizeNormal,
    Rotator,
    UniqueNetId,
    RowHandle,
    StatPairRepState,
    GameplayTag,
    GameplayTagContainer,
    HitResult,
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
            Self::Int8(_) => ItemKind::Int8,
            Self::Int16(_) => ItemKind::Int16,
            Self::UInt16(_) => ItemKind::UInt16,
            Self::Int(_) => ItemKind::Int,
            Self::UInt32(_) => ItemKind::UInt32,
            Self::Int64(_) => ItemKind::Int64,
            Self::UInt64(_) => ItemKind::UInt64,
            Self::Float(_) => ItemKind::Float,
            Self::Double(_) => ItemKind::Double,
            Self::Object(_) => ItemKind::Object,
            Self::NetworkName(_) => ItemKind::NetworkName,
            Self::Str(_) => ItemKind::Str,
            Self::SoftObject(_) => ItemKind::SoftObject,
            Self::Text(_) => ItemKind::Text,
            Self::Vector(_) => ItemKind::Vector,
            Self::Vector2D(_) => ItemKind::Vector2D,
            Self::VectorQuantize(_) => ItemKind::VectorQuantize,
            Self::VectorQuantize10(_) => ItemKind::VectorQuantize10,
            Self::VectorQuantize100(_) => ItemKind::VectorQuantize100,
            Self::VectorQuantizeNormal(_) => ItemKind::VectorQuantizeNormal,
            Self::Rotator(_) => ItemKind::Rotator,
            Self::UniqueNetId(_) => ItemKind::UniqueNetId,
            Self::RowHandle(_) => ItemKind::RowHandle,
            Self::StatPairRepState(_) => ItemKind::StatPairRepState,
            Self::GameplayTag(_) => ItemKind::GameplayTag,
            Self::GameplayTagContainer(_) => ItemKind::GameplayTagContainer,
            Self::HitResult(_) => ItemKind::HitResult,
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
            Self::Int8(_) => formatter.write_str("Int8"),
            Self::Int16(_) => formatter.write_str("Int16"),
            Self::UInt16(_) => formatter.write_str("UInt16"),
            Self::Int(_) => formatter.write_str("Int"),
            Self::UInt32(_) => formatter.write_str("UInt32"),
            Self::Int64(_) => formatter.write_str("Int64"),
            Self::UInt64(_) => formatter.write_str("UInt64"),
            Self::Float(_) => formatter.write_str("Float"),
            Self::Double(_) => formatter.write_str("Double"),
            Self::Object(guid) => formatter.debug_tuple("Object").field(guid).finish(),
            Self::NetworkName(name) => formatter.debug_tuple("NetworkName").field(name).finish(),
            Self::Str(text) => formatter
                .debug_struct("Str")
                .field("units", &text.declared_units())
                .finish(),
            Self::SoftObject(text) => formatter
                .debug_struct("SoftObject")
                .field("units", &text.declared_units())
                .finish(),
            Self::Text(text) => text.fmt(formatter),
            Self::Vector(_) => formatter.write_str("Vector"),
            Self::Vector2D(_) => formatter.write_str("Vector2D"),
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
            Self::UniqueNetId(id) => formatter.debug_tuple("UniqueNetId").field(id).finish(),
            Self::RowHandle(row) => row.fmt(formatter),
            Self::StatPairRepState(stat) => stat.fmt(formatter),
            Self::GameplayTag(tag) => formatter.debug_tuple("GameplayTag").field(tag).finish(),
            Self::GameplayTagContainer(tags) => formatter
                .debug_tuple("GameplayTagContainer")
                .field(&tags.as_ref().map(Vec::len))
                .finish(),
            Self::HitResult(hit) => formatter.debug_tuple("HitResult").field(hit).finish(),
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
    /// Declared Str or soft-object path units, including the terminator.
    pub string_units: Option<usize>,
    pub vector_bits: Option<u32>,
    pub rotator: Option<RotatorSummary>,
    pub unique_net_id: Option<UniqueNetIdSummary>,
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
            string_units: None,
            vector_bits: None,
            rotator: None,
            unique_net_id: None,
            movement: None,
            tag_count: None,
            opaque_bits: None,
            array: None,
        };
        match &self.value {
            RepValue::Object(guid) => summary.object = Some(guid.summary()),
            RepValue::NetworkName(name) => summary.name = Some(name.summary()),
            RepValue::Str(text)
            | RepValue::SoftObject(text)
            | RepValue::Text(TextValue {
                history: TextHistory::Base { source: text, .. },
                ..
            }) => {
                summary.string_units = Some(text.declared_units());
            }
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
            RepValue::UniqueNetId(id) => summary.unique_net_id = Some(id.summary()),
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
            | RepValue::Int8(_)
            | RepValue::Int16(_)
            | RepValue::UInt16(_)
            | RepValue::Int(_)
            | RepValue::UInt32(_)
            | RepValue::Int64(_)
            | RepValue::UInt64(_)
            | RepValue::Float(_)
            | RepValue::Double(_)
            | RepValue::Vector(_)
            | RepValue::Vector2D(_)
            | RepValue::RowHandle(_)
            | RepValue::StatPairRepState(_)
            | RepValue::GameplayTag(GameplayTag::NetIndex(_))
            | RepValue::HitResult(_)
            | RepValue::Text(_)
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
                *bits <= ENUM_BYTE_BITS_MAX,
                "layout handle {handle}: enum byte width must be 0 to 8"
            ),
            RepCommand::Enum { bits } => ensure!(
                *bits <= ENUM_BITS_MAX,
                "layout handle {handle}: enum width must be 0 to 32"
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
            | RepCommand::Int8
            | RepCommand::Int16
            | RepCommand::UInt16
            | RepCommand::Int
            | RepCommand::UInt32
            | RepCommand::Int64
            | RepCommand::UInt64
            | RepCommand::Float
            | RepCommand::Double
            | RepCommand::Object
            | RepCommand::NetworkName
            | RepCommand::Str
            | RepCommand::SoftObject
            | RepCommand::Text
            | RepCommand::Vector
            | RepCommand::Vector2D
            | RepCommand::VectorQuantize
            | RepCommand::VectorQuantize10
            | RepCommand::VectorQuantize100
            | RepCommand::VectorQuantizeNormal
            | RepCommand::Rotator
            | RepCommand::UniqueNetId
            | RepCommand::RowHandle
            | RepCommand::StatPairRepState
            | RepCommand::Movement { .. }
            | RepCommand::HitResult
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

pub(super) fn read_field(reader: &mut BitReader<'_>, bits: u32, what: &str) -> Result<u32> {
    let value = reader.read_bits(bits);
    ensure!(!reader.is_error(), "truncated {what}");
    Ok(value)
}

fn read_capped(reader: &mut BitReader<'_>, max: u32, what: &str) -> Result<u32> {
    let value = reader.read_int_max(max);
    ensure!(!reader.is_error(), "truncated {what}");
    Ok(value)
}

pub(super) fn read_u16(reader: &mut BitReader<'_>, order: ByteOrder, what: &str) -> Result<u16> {
    let value = read_field(reader, 16, what)? as u16;
    Ok(match order {
        ByteOrder::Little => value,
        ByteOrder::Big => value.swap_bytes(),
    })
}

pub(super) fn write_u16(writer: &mut BitWriter, value: u16, order: ByteOrder) {
    let value = match order {
        ByteOrder::Little => value,
        ByteOrder::Big => value.swap_bytes(),
    };
    writer.write_bits(u32::from(value), 16);
}

pub(super) fn read_u32(reader: &mut BitReader<'_>, order: ByteOrder, what: &str) -> Result<u32> {
    let value = read_field(reader, 32, what)?;
    Ok(match order {
        ByteOrder::Little => value,
        ByteOrder::Big => value.swap_bytes(),
    })
}

pub(super) fn write_u32(writer: &mut BitWriter, value: u32, order: ByteOrder) {
    writer.write_u32(match order {
        ByteOrder::Little => value,
        ByteOrder::Big => value.swap_bytes(),
    });
}

pub(super) fn read_u64(reader: &mut BitReader<'_>, order: ByteOrder, what: &str) -> Result<u64> {
    let low = u64::from(read_field(reader, 32, what)?);
    let high = u64::from(read_field(reader, 32, what)?);
    let raw = low | (high << 32);
    Ok(match order {
        ByteOrder::Little => raw,
        ByteOrder::Big => raw.swap_bytes(),
    })
}

pub(super) fn write_u64(writer: &mut BitWriter, value: u64, order: ByteOrder) {
    let raw = match order {
        ByteOrder::Little => value,
        ByteOrder::Big => value.swap_bytes(),
    };
    writer.write_u32(raw as u32);
    writer.write_u32((raw >> 32) as u32);
}

/// Read `FRowHandle::NetSerialize` (`0x143f7dbb0`, F-705): a 2-byte data-table
/// index in `order`, followed by 0, 1, 2 or 4 row-index bytes according to
/// [`icarus_row_handle_width_bytes`].
pub fn read_row_handle(reader: &mut BitReader<'_>, order: ByteOrder) -> Result<RowHandle> {
    let table_index = read_u16(reader, order, "row handle table index")?;
    let width = icarus_row_handle_width_bytes(table_index)?;
    let row_index = match width {
        0 => None,
        1 => Some(read_field(reader, 8, "row handle 1-byte row index")?),
        2 => Some(u32::from(read_u16(
            reader,
            order,
            "row handle 2-byte row index",
        )?)),
        4 => Some(read_u32(reader, order, "row handle 4-byte row index")?),
        _ => unreachable!(),
    };
    Ok(RowHandle {
        table_index,
        row_index,
    })
}

/// Mirror of [`read_row_handle`]; validates that `row_index` presence and range
/// match [`icarus_row_handle_width_bytes`].
pub fn write_row_handle(
    writer: &mut BitWriter,
    handle: &RowHandle,
    order: ByteOrder,
) -> Result<()> {
    let width = icarus_row_handle_width_bytes(handle.table_index)?;
    match (width, handle.row_index) {
        (0, None) => write_u16(writer, handle.table_index, order),
        (0, Some(_)) => bail!("zero-width row handle cannot carry a row index"),
        (_, None) => bail!("non-empty row handle table requires a row index"),
        (1, Some(row)) => {
            ensure!(
                row <= u32::from(u8::MAX),
                "row handle index exceeds 1-byte table width"
            );
            write_u16(writer, handle.table_index, order);
            writer.write_bits(row, 8);
        }
        (2, Some(row)) => {
            ensure!(
                row <= u32::from(u16::MAX),
                "row handle index exceeds 2-byte table width"
            );
            write_u16(writer, handle.table_index, order);
            write_u16(writer, row as u16, order);
        }
        (4, Some(row)) => {
            write_u16(writer, handle.table_index, order);
            write_u32(writer, row, order);
        }
        _ => unreachable!(),
    }
    Ok(())
}

fn read_stat_var_u32(reader: &mut BitReader<'_>, what: &str) -> Result<u32> {
    let len = read_field(reader, 8, what)? as u8;
    ensure!(
        (1..=4).contains(&len),
        "{what} length must be 1 to 4 bytes, got {len}"
    );
    let mut value = 0u32;
    for _ in 0..len {
        let byte = read_field(reader, 8, what)?;
        value = (value << 8) | byte;
    }
    ensure!(
        stat_int_len_bytes(value) == len,
        "non-canonical {what} length {len}"
    );
    Ok(value)
}

fn write_stat_var_u32(writer: &mut BitWriter, value: u32) {
    let len = stat_int_len_bytes(value);
    writer.write_bits(u32::from(len), 8);
    for shift in (0..len).rev() {
        let byte = (value >> (u32::from(shift) * 8)) & 0xff;
        writer.write_bits(byte, 8);
    }
}

/// Read `FStatPairRepState::NetSerialize` (`0x141c10c50` -> `0x1417df1a0`,
/// F-705): `Stat` (`u32`) and `Value` (`i32`) via `Detail::ReadInteger`
/// (`0x1417e1b50`).
pub fn read_stat_pair_rep_state(reader: &mut BitReader<'_>) -> Result<StatPairRepState> {
    let stat = read_stat_var_u32(reader, "stat pair stat")?;
    let value = read_stat_var_u32(reader, "stat pair value")? as i32;
    Ok(StatPairRepState { stat, value })
}

/// Write `FStatPairRepState::NetSerialize` (`0x141c10c50` -> `0x1417df1a0`,
/// F-705) via `Detail::WriteInteger` (`0x141807bd0`).
pub fn write_stat_pair_rep_state(writer: &mut BitWriter, pair: &StatPairRepState) {
    write_stat_var_u32(writer, pair.stat);
    write_stat_var_u32(writer, pair.value as u32);
}

pub(super) fn copy_bits(reader: &mut BitReader<'_>, mut bits: usize) -> Payload {
    let mut writer = BitWriter::new();
    while bits > 0 {
        let chunk = bits.min(32) as u32;
        writer.write_bits(reader.read_bits(chunk), chunk);
        bits -= chunk as usize;
    }
    Payload::from_writer(writer)
}

pub(super) fn append_bits(writer: &mut BitWriter, payload: &Payload) {
    let mut reader = payload.reader();
    let mut bits = payload.bit_len();
    while bits > 0 {
        let chunk = bits.min(32) as u32;
        writer.write_bits(reader.read_bits(chunk), chunk);
        bits -= chunk as usize;
    }
}

/// An archive string: a signed 4-byte count, narrow bytes for a positive
/// count, 2-byte units for a negative one, each with a terminator; network
/// names and Str items both use it (F-575, F-595).
pub(super) fn read_archive_string(
    reader: &mut BitReader<'_>,
    codec: &RepLayoutCodec,
    max_units: usize,
    what: &str,
) -> Result<ArchiveText> {
    let order = codec.actor.order;
    let count = read_u32(reader, order, what)? as i32;
    ensure!(count != i32::MIN, "{what} count i32::MIN unsupported");
    let units = count.unsigned_abs() as usize;
    ensure!(units <= max_units, "{what} exceeds its unit limit");
    // The cap bounds the allocation; overflowing reads allocate nothing.
    let payload = reader.read_bytes(units * if count < 0 { 2 } else { 1 });
    ensure!(!reader.is_error(), "truncated {what} payload");
    let prefix = match order {
        ByteOrder::Little => count.to_le_bytes(),
        ByteOrder::Big => count.to_be_bytes(),
    };
    let frame = [prefix.as_slice(), &payload].concat();
    let parsed = read_string_frame(&frame, order).with_context(|| format!("invalid {what}"))?;
    ensure!(
        parsed.consumed() == frame.len(),
        "{what} frame length mismatch"
    );
    let encoding = match parsed.encoding() {
        StringEncoding::Empty => TokenEncoding::Empty,
        StringEncoding::Narrow => TokenEncoding::Narrow,
        StringEncoding::Wide => TokenEncoding::Wide,
    };
    ArchiveText::with_encoding(&parsed.units().collect::<Vec<_>>(), encoding)
}

pub(super) fn write_archive_string(
    writer: &mut BitWriter,
    text: &ArchiveText,
    codec: &RepLayoutCodec,
    max_units: usize,
    what: &str,
) -> Result<()> {
    let order = codec.actor.order;
    let declared = text.declared_units();
    ensure!(declared <= max_units, "{what} exceeds its unit limit");
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
        let text = read_archive_string(
            reader,
            codec,
            codec.limits.max_name_units,
            "network name string",
        )?;
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
            write_archive_string(
                writer,
                text,
                codec,
                codec.limits.max_name_units,
                "network name string",
            )?;
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

pub(super) struct Budget {
    pub(super) remaining: usize,
}

impl Budget {
    pub(super) fn take(&mut self) -> Result<()> {
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

fn read_normal(reader: &mut BitReader<'_>) -> Result<QuantizedNormal> {
    let mut values = [0u16; 3];
    for value in &mut values {
        *value = read_field(reader, 16, "quantized normal component")? as u16;
    }
    Ok(QuantizedNormal(values))
}

fn write_normal(writer: &mut BitWriter, normal: &QuantizedNormal) {
    for value in normal.0 {
        writer.write_bits(u32::from(value), 16);
    }
}

/// `FHitResult` (F-597): flags byte, time, location, normal, impact point and
/// normal unless flagged equal, trace start and end, penetration depth and
/// item unless flagged, three object references, the bone name, then face and
/// element index unless flagged.
pub fn read_hit_result(reader: &mut BitReader<'_>, codec: &RepLayoutCodec) -> Result<HitResult> {
    let order = codec.actor.order;
    let flags = read_field(reader, 8, "hit result flags")? as u8;
    let gated_word = |reader: &mut BitReader<'_>, bit: u8, what: &str| -> Result<Option<u32>> {
        Ok(if flags & bit == 0 {
            Some(read_u32(reader, order, what)?)
        } else {
            None
        })
    };
    let time = read_u32(reader, order, "hit result time")?;
    let location = read_vector(reader, VectorPacking::NetQuantize).context("hit location")?;
    let normal = read_normal(reader).context("hit normal")?;
    let impact_point = if flags & HIT_IMPACT_POINT_IS_LOCATION == 0 {
        Some(read_vector(reader, VectorPacking::NetQuantize).context("hit impact point")?)
    } else {
        None
    };
    let impact_normal = if flags & HIT_IMPACT_NORMAL_IS_NORMAL == 0 {
        Some(read_normal(reader).context("hit impact normal")?)
    } else {
        None
    };
    let trace_start = read_vector(reader, VectorPacking::NetQuantize).context("hit trace start")?;
    let trace_end = read_vector(reader, VectorPacking::NetQuantize).context("hit trace end")?;
    let penetration_depth = gated_word(reader, HIT_NO_PENETRATION, "hit penetration depth")?;
    let item = gated_word(reader, HIT_NO_ITEM, "hit item")?;
    let phys_material = read_guid_ref(reader, &codec.actor).context("hit physical material")?;
    let actor = read_guid_ref(reader, &codec.actor).context("hit actor")?;
    let component = read_guid_ref(reader, &codec.actor).context("hit component")?;
    let bone_name = read_network_name(reader, codec).context("hit bone name")?;
    let face_index = gated_word(reader, HIT_NO_FACE_INDEX, "hit face index")?;
    let element_index = if flags & HIT_NO_ELEMENT_INDEX == 0 {
        Some(read_field(reader, 8, "hit element index")? as u8)
    } else {
        None
    };
    Ok(HitResult {
        flags,
        time,
        location,
        normal,
        impact_point,
        impact_normal,
        trace_start,
        trace_end,
        penetration_depth,
        item,
        phys_material,
        actor,
        component,
        bone_name,
        face_index,
        element_index,
    })
}

/// Mirror of [`read_hit_result`]; each optional field must be present exactly
/// when its flag bit is clear.
pub fn write_hit_result(
    writer: &mut BitWriter,
    hit: &HitResult,
    codec: &RepLayoutCodec,
) -> Result<()> {
    let order = codec.actor.order;
    let gated = |present: bool, bit: u8, what: &str| -> Result<()> {
        ensure!(
            present == (hit.flags & bit == 0),
            "hit result {what} must be present exactly when its flag bit is clear"
        );
        Ok(())
    };
    gated(
        hit.impact_point.is_some(),
        HIT_IMPACT_POINT_IS_LOCATION,
        "impact point",
    )?;
    gated(
        hit.impact_normal.is_some(),
        HIT_IMPACT_NORMAL_IS_NORMAL,
        "impact normal",
    )?;
    gated(
        hit.penetration_depth.is_some(),
        HIT_NO_PENETRATION,
        "penetration depth",
    )?;
    gated(hit.item.is_some(), HIT_NO_ITEM, "item")?;
    gated(hit.face_index.is_some(), HIT_NO_FACE_INDEX, "face index")?;
    gated(
        hit.element_index.is_some(),
        HIT_NO_ELEMENT_INDEX,
        "element index",
    )?;
    writer.write_bits(u32::from(hit.flags), 8);
    write_u32(writer, hit.time, order);
    write_vector(writer, &hit.location, VectorPacking::NetQuantize).context("hit location")?;
    write_normal(writer, &hit.normal);
    if let Some(point) = &hit.impact_point {
        write_vector(writer, point, VectorPacking::NetQuantize).context("hit impact point")?;
    }
    if let Some(normal) = &hit.impact_normal {
        write_normal(writer, normal);
    }
    write_vector(writer, &hit.trace_start, VectorPacking::NetQuantize)
        .context("hit trace start")?;
    write_vector(writer, &hit.trace_end, VectorPacking::NetQuantize).context("hit trace end")?;
    for word in [hit.penetration_depth, hit.item].into_iter().flatten() {
        write_u32(writer, word, order);
    }
    for reference in [&hit.phys_material, &hit.actor, &hit.component] {
        write_guid_ref(writer, reference, &codec.actor).context("hit object reference")?;
    }
    write_network_name(writer, &hit.bone_name, codec).context("hit bone name")?;
    if let Some(face) = hit.face_index {
        write_u32(writer, face, order);
    }
    if let Some(element) = hit.element_index {
        writer.write_bits(u32::from(element), 8);
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

fn control_codec(codec: &RepLayoutCodec) -> ControlCodec {
    ControlCodec {
        order: codec.actor.order,
        max_string_units: codec.limits.max_string_units,
    }
}

pub(super) fn read_value(
    reader: &mut BitReader<'_>,
    command: &RepCommand,
    codec: &RepLayoutCodec,
    depth: usize,
    budget: &mut Budget,
) -> Result<RepValue> {
    let order = codec.actor.order;
    Ok(match command {
        RepCommand::Bool => RepValue::Bool(read_flag(reader, "bool item")?),
        RepCommand::Byte { enum_bits } => {
            RepValue::Byte(read_field(reader, byte_width(*enum_bits), "byte item")? as u8)
        }
        RepCommand::Enum { bits } => {
            RepValue::Enum(read_field(reader, u32::from(*bits), "enum item")?)
        }
        RepCommand::Int8 => RepValue::Int8(read_field(reader, 8, "int8 item")? as u8 as i8),
        RepCommand::Int16 => RepValue::Int16(read_u16(reader, order, "int16 item")? as i16),
        RepCommand::UInt16 => RepValue::UInt16(read_u16(reader, order, "uint16 item")?),
        RepCommand::Int => RepValue::Int(read_u32(reader, order, "int item")? as i32),
        RepCommand::UInt32 => RepValue::UInt32(read_u32(reader, order, "uint32 item")?),
        RepCommand::Int64 => RepValue::Int64(read_u64(reader, order, "int64 item")? as i64),
        RepCommand::UInt64 => RepValue::UInt64(read_u64(reader, order, "uint64 item")?),
        RepCommand::Float => RepValue::Float(read_u32(reader, order, "float item")?),
        RepCommand::Double => RepValue::Double(read_u64(reader, order, "double item")?),
        RepCommand::Object => {
            RepValue::Object(read_guid_ref(reader, &codec.actor).context("object item")?)
        }
        RepCommand::NetworkName => RepValue::NetworkName(read_network_name(reader, codec)?),
        RepCommand::Text => RepValue::Text(read_text(reader, codec, 0)?),
        RepCommand::Str => RepValue::Str(read_archive_string(
            reader,
            codec,
            codec.limits.max_string_units,
            "string item",
        )?),
        RepCommand::SoftObject => RepValue::SoftObject(read_archive_string(
            reader,
            codec,
            codec.limits.max_string_units,
            "soft-object path",
        )?),
        RepCommand::Vector => RepValue::Vector([
            read_u32(reader, order, "vector X")?,
            read_u32(reader, order, "vector Y")?,
            read_u32(reader, order, "vector Z")?,
        ]),
        RepCommand::Vector2D => RepValue::Vector2D([
            read_u32(reader, order, "vector2d X")?,
            read_u32(reader, order, "vector2d Y")?,
        ]),
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
        RepCommand::HitResult => RepValue::HitResult(Box::new(
            read_hit_result(reader, codec).context("hit result item")?,
        )),
        RepCommand::Movement {
            location,
            velocity,
            rotation,
        } => RepValue::Movement(
            read_movement(reader, *location, *velocity, *rotation).context("movement item")?,
        ),
        RepCommand::VectorQuantizeNormal => RepValue::VectorQuantizeNormal(read_normal(reader)?),
        RepCommand::Rotator => RepValue::Rotator(read_rotator(reader).context("rotator item")?),
        RepCommand::UniqueNetId => RepValue::UniqueNetId(
            read_unique_net_id(reader, &control_codec(codec)).context("unique net id item")?,
        ),
        RepCommand::RowHandle => {
            RepValue::RowHandle(read_row_handle(reader, order).context("row handle item")?)
        }
        RepCommand::StatPairRepState => RepValue::StatPairRepState(
            read_stat_pair_rep_state(reader).context("stat pair rep state item")?,
        ),
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

pub(super) fn read_stream(
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

/// Read `FRepLayout::ReceiveProperties` (`0x143b20720`, M-426, F-704): a 1-bit
/// `bDoChecksum` flag, then a top-level handle stream with `codec.checksums`
/// set to that bit.
pub fn read_rep_properties(
    reader: &mut BitReader<'_>,
    layout: &[RepCommand],
    codec: &RepLayoutCodec,
) -> Result<PropertyStream> {
    let do_checksum = read_flag(reader, "rep properties checksum bit")?;
    let mut effective = *codec;
    effective.checksums = do_checksum;
    read_property_stream(reader, layout, &effective)
}

/// Parse a payload that must hold an exact `FRepLayout::ReceiveProperties`
/// block (1-bit `bDoChecksum` preamble followed by one handle stream, F-704).
pub fn parse_rep_properties(
    payload: &Payload,
    layout: &[RepCommand],
    codec: &RepLayoutCodec,
) -> Result<PropertyStream> {
    let mut reader = payload.reader();
    let stream = read_rep_properties(&mut reader, layout, codec)?;
    ensure!(reader.at_end(), "trailing bits after the property stream");
    Ok(stream)
}

pub(super) fn write_value(
    writer: &mut BitWriter,
    command: &RepCommand,
    value: &RepValue,
    codec: &RepLayoutCodec,
    depth: usize,
    budget: &mut Budget,
) -> Result<()> {
    let order = codec.actor.order;
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
        (RepCommand::Int8, RepValue::Int8(val)) => {
            writer.write_bits(u32::from(*val as u8), 8);
        }
        (RepCommand::Int16, RepValue::Int16(val)) => {
            write_u16(writer, *val as u16, order);
        }
        (RepCommand::UInt16, RepValue::UInt16(val)) => {
            write_u16(writer, *val, order);
        }
        (RepCommand::Int, RepValue::Int(val)) => {
            write_u32(writer, *val as u32, order);
        }
        (RepCommand::UInt32, RepValue::UInt32(val)) => {
            write_u32(writer, *val, order);
        }
        (RepCommand::Int64, RepValue::Int64(val)) => {
            write_u64(writer, *val as u64, order);
        }
        (RepCommand::UInt64, RepValue::UInt64(val)) => {
            write_u64(writer, *val, order);
        }
        (RepCommand::Float, RepValue::Float(bits)) => {
            write_u32(writer, *bits, order);
        }
        (RepCommand::Double, RepValue::Double(bits)) => {
            write_u64(writer, *bits, order);
        }
        (RepCommand::Object, RepValue::Object(guid)) => {
            write_guid_ref(writer, guid, &codec.actor).context("object item")?;
        }
        (RepCommand::NetworkName, RepValue::NetworkName(name)) => {
            write_network_name(writer, name, codec)?;
        }
        (RepCommand::Text, RepValue::Text(text)) => write_text(writer, text, codec, 0)?,
        (RepCommand::Str, RepValue::Str(text)) => {
            write_archive_string(
                writer,
                text,
                codec,
                codec.limits.max_string_units,
                "string item",
            )?;
        }
        (RepCommand::SoftObject, RepValue::SoftObject(text)) => {
            write_archive_string(
                writer,
                text,
                codec,
                codec.limits.max_string_units,
                "soft-object path",
            )?;
        }
        (RepCommand::Vector, RepValue::Vector(words)) => {
            for word in *words {
                write_u32(writer, word, order);
            }
        }
        (RepCommand::Vector2D, RepValue::Vector2D(words)) => {
            for word in *words {
                write_u32(writer, word, order);
            }
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
            write_normal(writer, normal);
        }
        (RepCommand::HitResult, RepValue::HitResult(hit)) => {
            write_hit_result(writer, hit, codec).context("hit result item")?;
        }
        (RepCommand::Rotator, RepValue::Rotator(rotator)) => write_rotator(writer, rotator),
        (RepCommand::UniqueNetId, RepValue::UniqueNetId(id)) => {
            write_unique_net_id(writer, id, &control_codec(codec)).context("unique net id item")?;
        }
        (RepCommand::RowHandle, RepValue::RowHandle(row)) => {
            write_row_handle(writer, row, order).context("row handle item")?;
        }
        (RepCommand::StatPairRepState, RepValue::StatPairRepState(stat)) => {
            write_stat_pair_rep_state(writer, stat);
        }
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

pub(super) fn write_stream(
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

/// Write `FRepLayout::SendProperties` / `ReceiveProperties` (`0x143b20720`,
/// M-426, F-704): the 1-bit `bDoChecksum` flag (`codec.checksums`), then the
/// top-level handle stream.
pub fn write_rep_properties(
    writer: &mut BitWriter,
    stream: &PropertyStream,
    layout: &[RepCommand],
    codec: &RepLayoutCodec,
) -> Result<()> {
    let payload = encode_rep_properties(stream, layout, codec)?;
    append_bits(writer, &payload);
    Ok(())
}

/// Encode an `FRepLayout::ReceiveProperties` block (1-bit `bDoChecksum`
/// preamble followed by one handle stream, F-704).
pub fn encode_rep_properties(
    stream: &PropertyStream,
    layout: &[RepCommand],
    codec: &RepLayoutCodec,
) -> Result<Payload> {
    validate_layout(layout, &codec.limits)?;
    let mut budget = Budget {
        remaining: codec.limits.max_entries,
    };
    let mut writer = BitWriter::new();
    writer.write_bit(codec.checksums);
    write_stream(&mut writer, stream, layout, 1, codec, 0, &mut budget)?;
    Ok(Payload::from_writer(writer))
}

/// Maximum registered enumerator value of `ENetRole` (`ROLE_MAX = 4`, F-703).
pub const NET_ROLE_MAX_VALUE: i64 = 4;
/// Maximum registered enumerator value of `EReplicatedLeftProspectState` (3, F-703).
pub const REPLICATED_LEFT_PROSPECT_STATE_MAX_VALUE: i64 = 3;
/// Maximum registered enumerator value of `EAudioShelterState` (2, F-705).
pub const AUDIO_SHELTER_STATE_MAX_VALUE: i64 = 2;
/// Maximum registered enumerator value of `EAliveState` (1, F-705).
pub const ALIVE_STATE_MAX_VALUE: i64 = 1;
/// `FGameplayTagContainer::NetSerialize` count width (`NumBitsForContainerSize = 6`, F-592, F-702).
pub const GAMEPLAY_TAG_CONTAINER_COUNT_BITS: u8 = 6;

/// Flattened child commands of `FRepAttachment` (`HasNetSerializer == false`,
/// F-703): `AttachParent`, `LocationOffset`, `RelativeScale3D`,
/// `RotationOffset`, `AttachSocket` and `AttachComponent`.
pub fn rep_attachment_commands() -> [RepCommand; 6] {
    [
        RepCommand::Object,
        RepCommand::VectorQuantize100,
        RepCommand::VectorQuantize100,
        RepCommand::Rotator,
        RepCommand::NetworkName,
        RepCommand::Object,
    ]
}

/// Statically observed 15-handle rep layout of `AActor` (F-609, F-703):
/// handles 1..4 bool flags (`bHidden`, `bReplicateMovement`, `bTearOff`,
/// `bCanBeDamaged`), handle 5 `RemoteRole` (`ENetRole`), handle 6
/// `ReplicatedMovement`, handles 7..12 flattened `AttachmentReplication`,
/// handle 13 `Owner`, handle 14 `Role` (`ENetRole`) and handle 15 `Instigator`.
pub fn actor_rep_layout(
    location: MovementLevel,
    velocity: MovementLevel,
    rotation: RotationLevel,
) -> Vec<RepCommand> {
    let role_bits = Some(enum_byte_bits(NET_ROLE_MAX_VALUE));
    let mut commands = Vec::with_capacity(15);
    commands.extend([
        RepCommand::Bool,
        RepCommand::Bool,
        RepCommand::Bool,
        RepCommand::Bool,
        RepCommand::Byte {
            enum_bits: role_bits,
        },
        RepCommand::Movement {
            location,
            velocity,
            rotation,
        },
    ]);
    commands.extend(rep_attachment_commands());
    commands.extend([
        RepCommand::Object,
        RepCommand::Byte {
            enum_bits: role_bits,
        },
        RepCommand::Object,
    ]);
    commands
}

/// Statically observed 17-handle rep layout of `AController` (F-703):
/// `AActor` handles 1..15, then handle 16 `PlayerState` and handle 17 `Pawn`.
pub fn controller_rep_layout(
    location: MovementLevel,
    velocity: MovementLevel,
    rotation: RotationLevel,
) -> Vec<RepCommand> {
    let mut commands = actor_rep_layout(location, velocity, rotation);
    commands.extend([RepCommand::Object, RepCommand::Object]);
    commands
}

/// Statically observed 19-handle rep layout of `APlayerController` (and
/// `AIcarusController`, which adds no net properties, F-703, F-704):
/// `AController` handles 1..17, then handle 18 `TargetViewRotation`
/// (`FRotator`) and handle 19 `SpawnLocation` (`FVector`).
pub fn player_controller_rep_layout(
    location: MovementLevel,
    velocity: MovementLevel,
    rotation: RotationLevel,
) -> Vec<RepCommand> {
    let mut commands = controller_rep_layout(location, velocity, rotation);
    commands.extend([RepCommand::Rotator, RepCommand::Vector]);
    commands
}

/// Statically observed 25-handle rep layout of `AIcarusPlayerController`
/// (F-703, F-704): `APlayerController` handles 1..19, then handle 20
/// `IcarusPlayerCharacter`, handle 21 `bCaptureViewTraces`, handle 22
/// `BackendProxyComponent`, handle 23 `PlayerDataComponent`, handle 24
/// `BestiaryManagerComponent` and handle 25 `GameplayTags`.
pub fn icarus_player_controller_rep_layout(
    location: MovementLevel,
    velocity: MovementLevel,
    rotation: RotationLevel,
    tag_encoding: TagEncoding,
) -> Vec<RepCommand> {
    let mut commands = player_controller_rep_layout(location, velocity, rotation);
    commands.extend([
        RepCommand::Object,
        RepCommand::Bool,
        RepCommand::Object,
        RepCommand::Object,
        RepCommand::Object,
        RepCommand::GameplayTagContainer {
            encoding: tag_encoding,
            count_bits: GAMEPLAY_TAG_CONTAINER_COUNT_BITS,
        },
    ]);
    commands
}

/// Statically observed 30-handle rep layout of `AIcarusPlayerControllerSurvival`
/// (F-703, F-704): `AIcarusPlayerController` handles 1..25, then handle 26
/// `ReplicatedLeftProspectSession` (`EReplicatedLeftProspectState`), handle 27
/// `AssignedDropshipSpawn`, handle 28 `AssignedDropship`, handle 29
/// `AssignedGravestone` and handle 30 `RemoteUserSettings`.
pub fn icarus_player_controller_survival_rep_layout(
    location: MovementLevel,
    velocity: MovementLevel,
    rotation: RotationLevel,
    tag_encoding: TagEncoding,
) -> Vec<RepCommand> {
    let mut commands =
        icarus_player_controller_rep_layout(location, velocity, rotation, tag_encoding);
    commands.extend([
        RepCommand::Enum {
            bits: enum_bits(REPLICATED_LEFT_PROSPECT_STATE_MAX_VALUE, 1),
        },
        RepCommand::Object,
        RepCommand::Object,
        RepCommand::Object,
        RepCommand::Object,
    ]);
    commands
}

/// Statically observed 26-handle rep layout of `APlayerState` (F-703, F-704):
/// `AActor` handles 1..15 (`AInfo` adds none), then handles 16..26 (`Score`,
/// `PlayerId`, `Ping`, five bitfield bools at `+0x22a` sorted by name,
/// `StartTime`, `UniqueId` and `PlayerNamePrivate`).
pub fn player_state_rep_layout(
    location: MovementLevel,
    velocity: MovementLevel,
    rotation: RotationLevel,
) -> Vec<RepCommand> {
    let mut commands = actor_rep_layout(location, velocity, rotation);
    commands.extend([
        RepCommand::Float,
        RepCommand::Int,
        RepCommand::Byte { enum_bits: None },
        RepCommand::Bool,
        RepCommand::Bool,
        RepCommand::Bool,
        RepCommand::Bool,
        RepCommand::Bool,
        RepCommand::Int,
        RepCommand::UniqueNetId,
        RepCommand::Str,
    ]);
    commands
}

/// Statically observed 2-handle rep layout of `UActorComponent` (and
/// `UPlayerDataComponent`, which adds no net properties, F-703, F-705):
/// handle 1 `bReplicates` and handle 2 `bIsActive`.
pub fn actor_component_rep_layout() -> Vec<RepCommand> {
    vec![RepCommand::Bool, RepCommand::Bool]
}

/// Statically observed 3-handle rep layout of `UFLODActorComponent` (F-705):
/// `UActorComponent` handles 1..2, then handle 3 `bSpawnedFromPool`.
pub fn flod_actor_component_rep_layout() -> Vec<RepCommand> {
    let mut commands = actor_component_rep_layout();
    commands.push(RepCommand::Bool);
    commands
}

/// Statically observed 3-handle rep layout of `UAudioContextComponent` (F-705):
/// `UActorComponent` handles 1..2, then handle 3 `ShelterState`
/// (`EAudioShelterState`, max 2 -> 2 bits).
pub fn audio_context_component_rep_layout() -> Vec<RepCommand> {
    let mut commands = actor_component_rep_layout();
    commands.push(RepCommand::Enum {
        bits: enum_bits(AUDIO_SHELTER_STATE_MAX_VALUE, 1),
    });
    commands
}

/// Statically observed 3-handle rep layout of `UTraitBehaviour` (`0x144f8a150`,
/// F-705): `UActorComponent` handles 1..2, then handle 3 `OwningComponent`
/// (`FObjectProperty`).
pub fn trait_behaviour_rep_layout() -> Vec<RepCommand> {
    let mut commands = actor_component_rep_layout();
    commands.push(RepCommand::Object);
    commands
}

/// Statically observed 4-handle rep layout of `UResourceNetworkComponent`
/// (F-705): `UActorComponent` handles 1..2, then handle 3 `Network` and
/// handle 4 `Spline` (`FObjectProperty`).
pub fn resource_network_component_rep_layout() -> Vec<RepCommand> {
    let mut commands = actor_component_rep_layout();
    commands.extend([RepCommand::Object, RepCommand::Object]);
    commands
}

/// Statically observed 3-handle rep layout of `UTraitComponent` (and subclasses
/// such as `UMeshableComponent`, `UInteractableComponent`,
/// `UHighlightableComponent`, `UUsableComponent`, `UDeployableComponent`,
/// `UTraitBehaviours` and `UInventoryComponent` that add no net properties,
/// F-705): `UActorComponent` handles 1..2, then handle 3 `DataRowHandle`
/// (`FRowHandle`).
pub fn trait_component_rep_layout() -> Vec<RepCommand> {
    let mut commands = actor_component_rep_layout();
    commands.push(RepCommand::RowHandle);
    commands
}

/// Statically observed 4-handle rep layout of `UDurableComponent` (F-705):
/// `UTraitComponent` handles 1..3, then handle 4 `Durability` (`FIntProperty`).
pub fn durable_component_rep_layout() -> Vec<RepCommand> {
    let mut commands = trait_component_rep_layout();
    commands.push(RepCommand::Int);
    commands
}

/// Statically observed 4-handle rep layout of `UItemableComponent` (F-705):
/// `UTraitComponent` handles 1..3, then handle 4 `Stack` (`FIntProperty`).
pub fn itemable_component_rep_layout() -> Vec<RepCommand> {
    let mut commands = trait_component_rep_layout();
    commands.push(RepCommand::Int);
    commands
}

/// Statically observed 5-handle rep layout of `UDecayableComponent` (F-705):
/// `UTraitComponent` handles 1..3, then handle 4 `DecayTime` and handle 5
/// `SpoilTime` (`FIntProperty`).
pub fn decayable_component_rep_layout() -> Vec<RepCommand> {
    let mut commands = trait_component_rep_layout();
    commands.extend([RepCommand::Int, RepCommand::Int]);
    commands
}

/// Statically observed 5-handle rep layout of `UFillableComponent` (F-705):
/// `UTraitComponent` handles 1..3, then handle 4 `FillableType` and handle 5
/// `StoredUnits` (`FIntProperty`).
pub fn fillable_component_rep_layout() -> Vec<RepCommand> {
    let mut commands = trait_component_rep_layout();
    commands.extend([RepCommand::Int, RepCommand::Int]);
    commands
}

/// Statically observed 4-handle rep layout of `UTransmutableComponent` (F-705):
/// `UTraitComponent` handles 1..3, then handle 4 `TransmutableUnits`
/// (`FIntProperty`).
pub fn transmutable_component_rep_layout() -> Vec<RepCommand> {
    let mut commands = trait_component_rep_layout();
    commands.push(RepCommand::Int);
    commands
}

/// Statically observed 4-handle rep layout of `UFocusableComponent` (F-705):
/// `UTraitComponent` handles 1..3, then handle 4 `IsFocused` (`FBoolProperty`).
pub fn focusable_component_rep_layout() -> Vec<RepCommand> {
    let mut commands = trait_component_rep_layout();
    commands.push(RepCommand::Bool);
    commands
}

/// Statically observed 8-handle rep layout of `UActionableComponent` (F-705):
/// `UTraitComponent` handles 1..3, then 5 `FIntProperty` handles 4..8.
pub fn actionable_component_rep_layout() -> Vec<RepCommand> {
    let mut commands = trait_component_rep_layout();
    commands.extend(std::iter::repeat_n(RepCommand::Int, 5));
    commands
}

/// Statically observed 8-handle rep layout of `UModifierStateComponent` (F-705):
/// `UActorComponent` handles 1..2, then handle 3 `DataRowHandleNew`
/// (`FModifierStatesRowHandle::NetSerialize` `0x141c93350` -> `0x141619d80`,
/// 2-byte row index `UInt16`), handle 4 `ReplicatedOwner` (`FObjectProperty`),
/// handle 5 `ModifierUID` (`FIntProperty`), handle 6 `ModifierLifeTime`
/// (`FFloatProperty`), handle 7 `CachedFinalEffectiveness` (`FIntProperty`) and
/// handle 8 `ReplicatedRemainingTime` (`FUInt16Property`).
pub fn modifier_state_component_rep_layout() -> Vec<RepCommand> {
    let mut commands = actor_component_rep_layout();
    commands.extend([
        RepCommand::UInt16,
        RepCommand::Object,
        RepCommand::Int,
        RepCommand::Float,
        RepCommand::Int,
        RepCommand::UInt16,
    ]);
    commands
}

/// Statically observed 12-handle rep layout of `UActorState` (F-705):
/// `UActorComponent` handles 1..2, then handles 3..6 `Health`, `MaxHealth`,
/// `Armor` and `MaxArmor` (`FIntProperty`), handle 7 `Shelter`
/// (`FFloatProperty`), handles 8..10 `ExternalTemperature`,
/// `ModifiedExternalTemperature` and `ModifiedInternalTemperature`
/// (`FIntProperty`), handle 11 `CurrentBiome`
/// (`FBiomesRowHandle::NetSerialize` `0x141a63b90` -> `0x141535390`, 1-byte row
/// index `Byte`) and handle 12 `CurrentAliveState` (`EAliveState`, max 1 -> 1 bit).
pub fn actor_state_rep_layout() -> Vec<RepCommand> {
    let mut commands = actor_component_rep_layout();
    commands.extend([
        RepCommand::Int,
        RepCommand::Int,
        RepCommand::Int,
        RepCommand::Int,
        RepCommand::Float,
        RepCommand::Int,
        RepCommand::Int,
        RepCommand::Int,
        RepCommand::Byte { enum_bits: None },
        RepCommand::Enum {
            bits: enum_bits(ALIVE_STATE_MAX_VALUE, 1),
        },
    ]);
    commands
}

/// Statically observed 17-handle rep layout of `UCharacterState` (F-705):
/// `UActorState` handles 1..12, then handles 13..15 `Stamina`, `MaxStamina` and
/// `TotalExperience` (`FIntProperty`), handle 16 `GrowthRowHandle`
/// (`FCharacterGrowthRowHandle::NetSerialize` `0x141a855e0` -> `0x1415537b0`,
/// 1-byte row index `Byte`) and handle 17 `Level` (`FIntProperty`).
pub fn character_state_rep_layout() -> Vec<RepCommand> {
    let mut commands = actor_state_rep_layout();
    commands.extend([
        RepCommand::Int,
        RepCommand::Int,
        RepCommand::Int,
        RepCommand::Byte { enum_bits: None },
        RepCommand::Int,
    ]);
    commands
}

/// Statically observed 30-handle rep layout of `USurvivalCharacterState` (F-705):
/// `UCharacterState` handles 1..17, then 13 `FIntProperty` handles 18..30
/// (`OxygenLevel`, `WaterLevel`, `FoodLevel`, `MinOxygen`, `MaxOxygen`,
/// `MinWater`, `MaxWater`, `MinFood`, `MaxFood`, `InternalTemperature`,
/// `RadiationLevel`, `MinRadiation` and `MaxRadiation`).
pub fn survival_character_state_rep_layout() -> Vec<RepCommand> {
    let mut commands = character_state_rep_layout();
    commands.extend(std::iter::repeat_n(RepCommand::Int, 13));
    commands
}

/// Statically observed 32-handle rep layout of `UPlayerCharacterState` (F-705):
/// `USurvivalCharacterState` handles 1..30, then handle 31 `LocalWeatherEvent`
/// (`FWeatherEventsRowHandle::NetSerialize` `0x141dec180` -> `0x1416c39b0`,
/// 1-byte row index `Byte`) and handle 32 `AIDetectionPercentage`
/// (`FIntProperty`).
pub fn player_character_state_rep_layout() -> Vec<RepCommand> {
    let mut commands = survival_character_state_rep_layout();
    commands.extend([RepCommand::Byte { enum_bits: None }, RepCommand::Int]);
    commands
}

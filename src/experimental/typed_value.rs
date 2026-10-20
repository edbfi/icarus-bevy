//! Typed, lossless interpretation of a framed property tree.
//!
//! Turns the value-opaque [`PropertyTree`] into typed values: scalars, archive
//! strings, framed text, tagged and fixed-width structs, scalar and struct
//! arrays, and Map/Set items. Every typed leaf is re-encoded and compared with
//! its original bytes; a leaf that cannot be typed, or does not re-encode
//! exactly, stays [`Value::Uninterpreted`] with a [`Gap`] reason, so the tree
//! is always total and reconstructs its input byte for byte.
//!
//! This assigns wire types, not gameplay meanings: property names and values
//! are carried as data, never matched against a schema here. Class meanings,
//! object construction and world import belong to callers (`F-565`, `F-602`,
//! `F-559`, `F-562`). Summaries count kinds and gaps only, never names or values.
use std::collections::BTreeMap;
use std::fmt;

use anyhow::{Context, Result, ensure};
use serde::Serialize;

use super::archive::{ByteOrder, encode_string, read_string_frame, read_text_frame};
use super::property_tree::{
    BaseComponentTree, ContainerItem, NodeValue, PropertyNode, PropertyTree, declared_size,
};
use super::tagged::{ParsedRecord, TagKind, TagProfile, container_kinds, is_name, read_record};

/// Fixed-width engine structs serialized natively (no tags) inside tagged
/// streams. Width and part layout follow the engine serializers (`F-602`).
const FIXED_STRUCTS: [(&str, FixedLayout); 11] = [
    ("Vector", FixedLayout::F32(3)),
    ("Rotator", FixedLayout::F32(3)),
    ("Quat", FixedLayout::F32(4)),
    ("Vector2D", FixedLayout::F32(2)),
    ("Vector4", FixedLayout::F32(4)),
    ("LinearColor", FixedLayout::F32(4)),
    ("IntPoint", FixedLayout::I32(2)),
    ("IntVector", FixedLayout::I32(3)),
    ("Guid", FixedLayout::U32(4)),
    ("DateTime", FixedLayout::I64),
    ("Timespan", FixedLayout::I64),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FixedLayout {
    F32(usize),
    I32(usize),
    U32(usize),
    I64,
}

impl FixedLayout {
    fn width(self) -> usize {
        match self {
            Self::F32(parts) | Self::I32(parts) | Self::U32(parts) => parts * 4,
            Self::I64 => 8,
        }
    }

    fn decode(self, bytes: &[u8]) -> Option<FixedStruct> {
        if bytes.len() != self.width() {
            return None;
        }
        let words = || {
            bytes
                .as_chunks::<4>()
                .0
                .iter()
                .map(|chunk| u32::from_le_bytes(*chunk))
        };
        Some(match self {
            Self::F32(_) => FixedStruct::F32(words().map(f32::from_bits).collect()),
            Self::I32(_) => FixedStruct::I32(words().map(|word| word as i32).collect()),
            Self::U32(_) => FixedStruct::U32(words().collect()),
            Self::I64 => FixedStruct::I64(i64::from_le_bytes(bytes.try_into().ok()?)),
        })
    }
}

fn fixed_layout(type_name: &[u16]) -> Option<FixedLayout> {
    FIXED_STRUCTS
        .iter()
        .find_map(|(name, layout)| is_name(type_name, name).then_some(*layout))
}

/// Why a leaf stayed uninterpreted. Ordered for stable report keys.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Gap {
    /// A Struct payload that neither framed as tags nor names a fixed struct.
    UnknownStruct,
    /// A named fixed struct whose payload width differs from its layout.
    FixedStructWidth,
    /// An Array whose inner kind has no typed element layout here.
    UnsupportedArrayInner,
    /// An Array payload that did not frame exactly under its inner kind.
    ArrayFraming,
    /// A Map or Set the lossless tree left opaque.
    ContainerUnframed,
    /// A binary Struct container item, whose struct name the header omits.
    UnnamedContainerStruct,
    /// A Text payload outside the strict text-frame subset.
    TextFraming,
    /// A scalar payload whose width or encoding differs from its kind.
    ScalarFraming,
    /// The typed value re-encoded to different bytes.
    NonCanonical,
}

/// Fixed-width native struct parts, in serialized order.
#[derive(Clone, Debug, PartialEq)]
pub enum FixedStruct {
    F32(Vec<f32>),
    I32(Vec<i32>),
    U32(Vec<u32>),
    I64(i64),
}

impl FixedStruct {
    fn write(&self, out: &mut Vec<u8>) {
        match self {
            Self::F32(parts) => parts
                .iter()
                .for_each(|part| out.extend_from_slice(&part.to_bits().to_le_bytes())),
            Self::I32(parts) => parts
                .iter()
                .for_each(|part| out.extend_from_slice(&part.to_le_bytes())),
            Self::U32(parts) => parts
                .iter()
                .for_each(|part| out.extend_from_slice(&part.to_le_bytes())),
            Self::I64(value) => out.extend_from_slice(&value.to_le_bytes()),
        }
    }

    /// The parts as `f32` when this is a float struct of exactly `N` parts.
    pub fn floats<const N: usize>(&self) -> Option<[f32; N]> {
        match self {
            Self::F32(parts) => parts.as_slice().try_into().ok(),
            _ => None,
        }
    }
}

/// A scalar property value or container item.
#[derive(Clone, PartialEq)]
pub enum Scalar {
    Bool(bool),
    Int(i32),
    UInt32(u32),
    Int64(i64),
    Float(f32),
    Byte(u8),
    /// An archive string: `StrProperty`.
    Str(Vec<u16>),
    /// An archive string: `NameProperty`.
    Name(Vec<u16>),
    /// An enumerator name: `EnumProperty`, or `ByteProperty` with an enum.
    Enum(Vec<u16>),
}

/// One typed property value.
pub enum Value {
    Scalar(Scalar),
    /// A strict text frame, kept as its exact bytes.
    Text(Vec<u8>),
    /// A struct framed as a terminated tagged stream.
    TaggedStruct {
        type_name: Vec<u16>,
        tree: TypedTree,
    },
    /// A natively serialized fixed-width struct.
    FixedStruct {
        type_name: Vec<u16>,
        value: FixedStruct,
    },
    /// A tagged struct array: count, shared inner header, then elements.
    StructArray {
        shared_header: Vec<u8>,
        type_name: Vec<u16>,
        elements: Vec<TypedTree>,
    },
    /// A fixed-width struct array under one shared inner header.
    FixedStructArray {
        shared_header: Vec<u8>,
        type_name: Vec<u16>,
        elements: Vec<FixedStruct>,
    },
    /// An array of scalars of one inner kind.
    ScalarArray {
        inner: TagKind,
        items: Vec<Scalar>,
    },
    Map {
        removed: Vec<Item>,
        entries: Vec<(Item, Item)>,
    },
    Set {
        removed: Vec<Item>,
        elements: Vec<Item>,
    },
    /// Exact original payload bytes that could not be typed.
    Uninterpreted {
        gap: Gap,
        bytes: Vec<u8>,
    },
}

/// One Map or Set item.
pub enum Item {
    Scalar(Scalar),
    Struct(TypedTree),
    /// A binary struct item; the container header does not name its type.
    UnnamedStruct(Vec<u8>),
}

/// One typed property with its exact original header.
pub struct TypedProperty {
    pub kind: TagKind,
    /// Property name units, as stored in the header.
    pub name: Vec<u16>,
    /// The original header, including name, type, size, index and extensions.
    pub header: Vec<u8>,
    pub value: Value,
}

/// A typed tagged stream: properties and the original terminal name record.
pub struct TypedTree {
    pub properties: Vec<TypedProperty>,
    pub terminator: Vec<u8>,
}

/// A typed base component: its tree and the original four-byte object trailer.
pub struct TypedComponent {
    pub tree: TypedTree,
    pub trailer: Vec<u8>,
}

/// Counts of typed leaves and gaps; never names or values.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct TypedSummary {
    pub properties: usize,
    pub bools: usize,
    pub integers: usize,
    pub floats: usize,
    pub bytes: usize,
    pub strings: usize,
    pub names: usize,
    pub enums: usize,
    pub texts: usize,
    pub tagged_structs: usize,
    pub fixed_structs: usize,
    pub struct_arrays: usize,
    pub fixed_struct_arrays: usize,
    pub scalar_arrays: usize,
    pub array_elements: usize,
    pub maps: usize,
    pub sets: usize,
    pub container_items: usize,
    pub uninterpreted: usize,
    pub uninterpreted_bytes: usize,
    pub gaps: BTreeMap<Gap, usize>,
}

impl TypedSummary {
    pub fn add(&mut self, other: &Self) {
        self.properties += other.properties;
        self.bools += other.bools;
        self.integers += other.integers;
        self.floats += other.floats;
        self.bytes += other.bytes;
        self.strings += other.strings;
        self.names += other.names;
        self.enums += other.enums;
        self.texts += other.texts;
        self.tagged_structs += other.tagged_structs;
        self.fixed_structs += other.fixed_structs;
        self.struct_arrays += other.struct_arrays;
        self.fixed_struct_arrays += other.fixed_struct_arrays;
        self.scalar_arrays += other.scalar_arrays;
        self.array_elements += other.array_elements;
        self.maps += other.maps;
        self.sets += other.sets;
        self.container_items += other.container_items;
        self.uninterpreted += other.uninterpreted;
        self.uninterpreted_bytes += other.uninterpreted_bytes;
        for (gap, count) in &other.gaps {
            *self.gaps.entry(*gap).or_default() += count;
        }
    }

    fn scalar(&mut self, scalar: &Scalar) {
        match scalar {
            Scalar::Bool(_) => self.bools += 1,
            Scalar::Int(_) | Scalar::UInt32(_) | Scalar::Int64(_) => self.integers += 1,
            Scalar::Float(_) => self.floats += 1,
            Scalar::Byte(_) => self.bytes += 1,
            Scalar::Str(_) => self.strings += 1,
            Scalar::Name(_) => self.names += 1,
            Scalar::Enum(_) => self.enums += 1,
        }
    }

    fn gap(&mut self, gap: Gap, bytes: usize) {
        self.uninterpreted += 1;
        self.uninterpreted_bytes += bytes;
        *self.gaps.entry(gap).or_default() += 1;
    }
}

const PROFILE: TagProfile = TagProfile::MemoryArchive522;

/// Header fields the typed layer needs; the rest stays in the exact header.
struct HeaderInfo {
    name: Vec<u16>,
    extra: Option<Vec<u16>>,
    bool_value: Option<bool>,
}

fn header_info(header: &[u8], payload_len: usize) -> Result<HeaderInfo> {
    // The header declares the payload size; a zero-filled payload of that
    // length lets the strict reader parse the header alone.
    let mut record = Vec::with_capacity(header.len() + payload_len);
    record.extend_from_slice(header);
    record.resize(header.len() + payload_len, 0);
    match read_record(&record, PROFILE)? {
        ParsedRecord::Property(property) => {
            ensure!(
                property.tag.header.len() == header.len(),
                "typed header does not match its framed header"
            );
            Ok(HeaderInfo {
                name: property.name,
                extra: property.extra,
                bool_value: property.bool_value,
            })
        }
        ParsedRecord::Terminator(_) => anyhow::bail!("typed header is a terminator"),
    }
}

fn string_exact(bytes: &[u8]) -> Option<Vec<u16>> {
    let frame = read_string_frame(bytes, ByteOrder::Little).ok()?;
    (frame.consumed() == bytes.len()).then(|| frame.units().collect())
}

/// Decode one scalar of `kind` from the front of `input`, returning the value
/// and bytes consumed. `Byte` is one raw byte; `Enum` an archive string.
fn scalar_prefix(kind: TagKind, input: &[u8]) -> Option<(Scalar, usize)> {
    let word = || {
        input
            .get(..4)
            .map(|b| u32::from_le_bytes(b.try_into().unwrap_or([0; 4])))
    };
    Some(match kind {
        TagKind::Bool => match input.first()? {
            0 => (Scalar::Bool(false), 1),
            1 => (Scalar::Bool(true), 1),
            _ => return None,
        },
        TagKind::Byte => (Scalar::Byte(*input.first()?), 1),
        TagKind::Int => (Scalar::Int(word()? as i32), 4),
        TagKind::UInt32 => (Scalar::UInt32(word()?), 4),
        TagKind::Float => (Scalar::Float(f32::from_bits(word()?)), 4),
        TagKind::Int64 => (
            Scalar::Int64(i64::from_le_bytes(input.get(..8)?.try_into().ok()?)),
            8,
        ),
        TagKind::String | TagKind::Name | TagKind::Enum => {
            let frame = read_string_frame(input, ByteOrder::Little).ok()?;
            let units = frame.units().collect();
            let consumed = frame.consumed();
            let scalar = match kind {
                TagKind::String => Scalar::Str(units),
                TagKind::Name => Scalar::Name(units),
                _ => Scalar::Enum(units),
            };
            (scalar, consumed)
        }
        _ => return None,
    })
}

impl Scalar {
    /// The container/array item encoding (Bool is one byte here).
    fn write_item(&self, out: &mut Vec<u8>) -> Result<()> {
        match self {
            Self::Bool(value) => out.push(u8::from(*value)),
            Self::Byte(value) => out.push(*value),
            Self::Int(value) => out.extend_from_slice(&value.to_le_bytes()),
            Self::UInt32(value) => out.extend_from_slice(&value.to_le_bytes()),
            Self::Int64(value) => out.extend_from_slice(&value.to_le_bytes()),
            Self::Float(value) => out.extend_from_slice(&value.to_bits().to_le_bytes()),
            Self::Str(units) | Self::Name(units) | Self::Enum(units) => {
                out.extend_from_slice(&encode_string(units, ByteOrder::Little)?);
            }
        }
        Ok(())
    }

    /// The property payload encoding (Bool lives in the header: no payload).
    fn write_payload(&self, out: &mut Vec<u8>) -> Result<()> {
        match self {
            Self::Bool(_) => Ok(()),
            other => other.write_item(out),
        }
    }
}

impl fmt::Debug for Scalar {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Values may be private player data; print only the variant.
        let label = match self {
            Self::Bool(_) => "Bool",
            Self::Int(_) => "Int",
            Self::UInt32(_) => "UInt32",
            Self::Int64(_) => "Int64",
            Self::Float(_) => "Float",
            Self::Byte(_) => "Byte",
            Self::Str(_) => "Str",
            Self::Name(_) => "Name",
            Self::Enum(_) => "Enum",
        };
        f.write_str(label)
    }
}

struct Typer {
    summary: TypedSummary,
}

impl Typer {
    fn tree(&mut self, tree: &PropertyTree) -> Result<TypedTree> {
        let properties = tree
            .properties
            .iter()
            .map(|node| self.property(node))
            .collect::<Result<_>>()?;
        Ok(TypedTree {
            properties,
            terminator: tree.terminator.clone(),
        })
    }

    fn property(&mut self, node: &PropertyNode) -> Result<TypedProperty> {
        self.summary.properties += 1;
        let payload = original_payload(&node.value)?;
        let info = header_info(&node.header, payload.len())?;
        let value = match &node.value {
            NodeValue::Opaque(bytes) => self.opaque(node.kind, &info, bytes),
            NodeValue::Struct(tree) => {
                self.summary.tagged_structs += 1;
                Value::TaggedStruct {
                    type_name: info.extra.clone().unwrap_or_default(),
                    tree: self.tree(tree)?,
                }
            }
            NodeValue::StructArray {
                prefix: _,
                shared_header,
                elements,
            } => {
                self.summary.struct_arrays += 1;
                self.summary.array_elements += elements.len();
                let shared = declared_size(shared_header)
                    .and_then(|size| header_info(shared_header, size))
                    .ok();
                Value::StructArray {
                    shared_header: shared_header.clone(),
                    type_name: shared.and_then(|s| s.extra).unwrap_or_default(),
                    elements: elements
                        .iter()
                        .map(|element| self.tree(element))
                        .collect::<Result<_>>()?,
                }
            }
            NodeValue::Map { removed, entries } => {
                self.summary.maps += 1;
                let (key, value) = container_kinds(&node.header, PROFILE)?;
                Value::Map {
                    removed: removed
                        .iter()
                        .map(|item| self.item(key, item))
                        .collect::<Result<_>>()?,
                    entries: entries
                        .iter()
                        .map(|(k, v)| Ok((self.item(key, k)?, self.item(value, v)?)))
                        .collect::<Result<_>>()?,
                }
            }
            NodeValue::Set { removed, elements } => {
                self.summary.sets += 1;
                let (element, _) = container_kinds(&node.header, PROFILE)?;
                Value::Set {
                    removed: removed
                        .iter()
                        .map(|item| self.item(element, item))
                        .collect::<Result<_>>()?,
                    elements: elements
                        .iter()
                        .map(|item| self.item(element, item))
                        .collect::<Result<_>>()?,
                }
            }
        };
        let value = self.verify(node.kind, value, &payload)?;
        Ok(TypedProperty {
            kind: node.kind,
            name: info.name,
            header: node.header.clone(),
            value,
        })
    }

    /// Re-encode a typed value; a mismatch demotes it to an exact gap leaf.
    fn verify(&mut self, kind: TagKind, value: Value, original: &[u8]) -> Result<Value> {
        if matches!(value, Value::Uninterpreted { .. }) {
            return Ok(value);
        }
        let mut encoded = Vec::with_capacity(original.len());
        value.write_payload(kind, &mut encoded)?;
        if encoded == original {
            return Ok(value);
        }
        self.summary.gap(Gap::NonCanonical, original.len());
        Ok(Value::Uninterpreted {
            gap: Gap::NonCanonical,
            bytes: original.to_vec(),
        })
    }

    fn uninterpreted(&mut self, gap: Gap, bytes: &[u8]) -> Value {
        self.summary.gap(gap, bytes.len());
        Value::Uninterpreted {
            gap,
            bytes: bytes.to_vec(),
        }
    }

    fn opaque(&mut self, kind: TagKind, info: &HeaderInfo, bytes: &[u8]) -> Value {
        match kind {
            TagKind::Bool => match info.bool_value {
                Some(value) if bytes.is_empty() => self.scalar(Scalar::Bool(value)),
                _ => self.uninterpreted(Gap::ScalarFraming, bytes),
            },
            TagKind::Byte => {
                let has_enum = info
                    .extra
                    .as_deref()
                    .is_some_and(|name| !is_name(name, "None"));
                if has_enum {
                    match string_exact(bytes) {
                        Some(units) => self.scalar(Scalar::Enum(units)),
                        None => self.uninterpreted(Gap::ScalarFraming, bytes),
                    }
                } else {
                    match bytes {
                        [value] => self.scalar(Scalar::Byte(*value)),
                        _ => self.uninterpreted(Gap::ScalarFraming, bytes),
                    }
                }
            }
            TagKind::Int | TagKind::UInt32 | TagKind::Float | TagKind::Int64 => {
                match scalar_prefix(kind, bytes) {
                    Some((scalar, used)) if used == bytes.len() => self.scalar(scalar),
                    _ => self.uninterpreted(Gap::ScalarFraming, bytes),
                }
            }
            TagKind::String | TagKind::Name | TagKind::Enum => match string_exact(bytes) {
                Some(units) => self.scalar(match kind {
                    TagKind::String => Scalar::Str(units),
                    TagKind::Name => Scalar::Name(units),
                    _ => Scalar::Enum(units),
                }),
                None => self.uninterpreted(Gap::ScalarFraming, bytes),
            },
            TagKind::Text => match read_text_frame(bytes, ByteOrder::Little) {
                Ok(frame) if frame.consumed() == bytes.len() => {
                    self.summary.texts += 1;
                    Value::Text(bytes.to_vec())
                }
                _ => self.uninterpreted(Gap::TextFraming, bytes),
            },
            TagKind::Struct => {
                let type_name = info.extra.clone().unwrap_or_default();
                match fixed_layout(&type_name) {
                    Some(layout) => match layout.decode(bytes) {
                        Some(value) => {
                            self.summary.fixed_structs += 1;
                            Value::FixedStruct { type_name, value }
                        }
                        None => self.uninterpreted(Gap::FixedStructWidth, bytes),
                    },
                    None => self.uninterpreted(Gap::UnknownStruct, bytes),
                }
            }
            TagKind::Array => self.array(info.extra.as_deref(), bytes),
            TagKind::Map | TagKind::Set => self.uninterpreted(Gap::ContainerUnframed, bytes),
        }
    }

    fn scalar(&mut self, scalar: Scalar) -> Value {
        self.summary.scalar(&scalar);
        Value::Scalar(scalar)
    }

    fn array(&mut self, inner: Option<&[u16]>, bytes: &[u8]) -> Value {
        let Some(inner) = inner.and_then(kind_named) else {
            return self.uninterpreted(Gap::UnsupportedArrayInner, bytes);
        };
        let Some(count) = bytes
            .get(..4)
            .and_then(|b| usize::try_from(i32::from_le_bytes(b.try_into().ok()?)).ok())
        else {
            return self.uninterpreted(Gap::ArrayFraming, bytes);
        };
        if inner == TagKind::Struct {
            return self.fixed_struct_array(count, bytes);
        }
        if !matches!(
            inner,
            TagKind::Bool
                | TagKind::Byte
                | TagKind::Int
                | TagKind::UInt32
                | TagKind::Float
                | TagKind::Int64
                | TagKind::String
                | TagKind::Name
                | TagKind::Enum
        ) {
            return self.uninterpreted(Gap::UnsupportedArrayInner, bytes);
        }
        // Every element occupies at least one byte.
        if count > bytes.len() - 4 {
            return self.uninterpreted(Gap::ArrayFraming, bytes);
        }
        let mut items = Vec::with_capacity(count);
        let mut position = 4;
        for _ in 0..count {
            match scalar_prefix(inner, &bytes[position..]) {
                Some((scalar, used)) => {
                    items.push(scalar);
                    position += used;
                }
                None => return self.uninterpreted(Gap::ArrayFraming, bytes),
            }
        }
        if position != bytes.len() {
            return self.uninterpreted(Gap::ArrayFraming, bytes);
        }
        self.summary.scalar_arrays += 1;
        self.summary.array_elements += items.len();
        items.iter().for_each(|item| self.summary.scalar(item));
        Value::ScalarArray { inner, items }
    }

    /// A struct array the tagged reader did not frame: count, one shared
    /// StructProperty header, then `count` fixed-width native elements.
    fn fixed_struct_array(&mut self, count: usize, bytes: &[u8]) -> Value {
        let Ok(ParsedRecord::Property(shared)) = read_record(&bytes[4..], PROFILE) else {
            return self.uninterpreted(Gap::ArrayFraming, bytes);
        };
        if shared.tag.kind != TagKind::Struct || 4 + shared.tag.record.len() != bytes.len() {
            return self.uninterpreted(Gap::ArrayFraming, bytes);
        }
        let type_name = shared.extra.clone().unwrap_or_default();
        let Some(layout) = fixed_layout(&type_name) else {
            return self.uninterpreted(Gap::UnknownStruct, bytes);
        };
        let body = shared.tag.payload;
        if count.checked_mul(layout.width()) != Some(body.len()) {
            return self.uninterpreted(Gap::FixedStructWidth, bytes);
        }
        let elements: Option<Vec<_>> = body
            .chunks_exact(layout.width().max(1))
            .map(|chunk| layout.decode(chunk))
            .collect();
        let Some(elements) = elements else {
            return self.uninterpreted(Gap::FixedStructWidth, bytes);
        };
        self.summary.fixed_struct_arrays += 1;
        self.summary.array_elements += elements.len();
        Value::FixedStructArray {
            shared_header: shared.tag.header.to_vec(),
            type_name,
            elements,
        }
    }

    fn item(&mut self, kind: Option<TagKind>, item: &ContainerItem) -> Result<Item> {
        self.summary.container_items += 1;
        Ok(match item {
            ContainerItem::Struct(tree) => {
                self.summary.tagged_structs += 1;
                Item::Struct(self.tree(tree)?)
            }
            ContainerItem::Bytes(bytes) => {
                let kind = kind.context("container item kind outside the supported set")?;
                if kind == TagKind::Struct {
                    self.summary.gap(Gap::UnnamedContainerStruct, bytes.len());
                    Item::UnnamedStruct(bytes.clone())
                } else {
                    let (scalar, used) = scalar_prefix(kind, bytes)
                        .context("container item does not decode under its kind")?;
                    ensure!(used == bytes.len(), "container item has trailing bytes");
                    self.summary.scalar(&scalar);
                    Item::Scalar(scalar)
                }
            }
        })
    }
}

fn kind_named(name: &[u16]) -> Option<TagKind> {
    const KINDS: [(&str, TagKind); 14] = [
        ("ArrayProperty", TagKind::Array),
        ("StructProperty", TagKind::Struct),
        ("StrProperty", TagKind::String),
        ("NameProperty", TagKind::Name),
        ("IntProperty", TagKind::Int),
        ("UInt32Property", TagKind::UInt32),
        ("FloatProperty", TagKind::Float),
        ("TextProperty", TagKind::Text),
        ("ByteProperty", TagKind::Byte),
        ("EnumProperty", TagKind::Enum),
        ("BoolProperty", TagKind::Bool),
        ("MapProperty", TagKind::Map),
        ("SetProperty", TagKind::Set),
        ("Int64Property", TagKind::Int64),
    ];
    KINDS
        .into_iter()
        .find_map(|(label, kind)| is_name(name, label).then_some(kind))
}

/// The exact payload bytes the lossless tree reconstructs for one node.
fn original_payload(value: &NodeValue) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    write_node_value(value, &mut out)?;
    Ok(out)
}

fn write_node_value(value: &NodeValue, out: &mut Vec<u8>) -> Result<()> {
    match value {
        NodeValue::Opaque(bytes) => out.extend_from_slice(bytes),
        NodeValue::Struct(tree) => tree.write(out)?,
        NodeValue::StructArray {
            prefix,
            shared_header,
            elements,
        } => {
            out.extend_from_slice(prefix);
            out.extend_from_slice(shared_header);
            for element in elements {
                element.write(out)?;
            }
        }
        NodeValue::Map { removed, entries } => {
            write_count(out, removed.len())?;
            for item in removed {
                write_container_item(item, out)?;
            }
            write_count(out, entries.len())?;
            for (key, value) in entries {
                write_container_item(key, out)?;
                write_container_item(value, out)?;
            }
        }
        NodeValue::Set { removed, elements } => {
            write_count(out, removed.len())?;
            for item in removed {
                write_container_item(item, out)?;
            }
            write_count(out, elements.len())?;
            for item in elements {
                write_container_item(item, out)?;
            }
        }
    }
    Ok(())
}

fn write_container_item(item: &ContainerItem, out: &mut Vec<u8>) -> Result<()> {
    match item {
        ContainerItem::Bytes(bytes) => out.extend_from_slice(bytes),
        ContainerItem::Struct(tree) => tree.write(out)?,
    }
    Ok(())
}

fn write_count(out: &mut Vec<u8>, count: usize) -> Result<()> {
    out.extend_from_slice(&i32::try_from(count)?.to_le_bytes());
    Ok(())
}

impl Value {
    fn write_payload(&self, kind: TagKind, out: &mut Vec<u8>) -> Result<()> {
        match self {
            Self::Scalar(scalar) => {
                ensure!(
                    !matches!(scalar, Scalar::Bool(_)) || kind == TagKind::Bool,
                    "bool value under a non-bool header"
                );
                scalar.write_payload(out)?;
            }
            Self::Text(bytes) | Self::Uninterpreted { bytes, .. } => out.extend_from_slice(bytes),
            Self::TaggedStruct { tree, .. } => tree.write(out)?,
            Self::FixedStruct { value, .. } => value.write(out),
            Self::StructArray {
                shared_header,
                elements,
                ..
            } => {
                write_count(out, elements.len())?;
                out.extend_from_slice(shared_header);
                for element in elements {
                    element.write(out)?;
                }
            }
            Self::FixedStructArray {
                shared_header,
                elements,
                ..
            } => {
                write_count(out, elements.len())?;
                out.extend_from_slice(shared_header);
                for element in elements {
                    element.write(out);
                }
            }
            Self::ScalarArray { items, .. } => {
                write_count(out, items.len())?;
                for item in items {
                    item.write_item(out)?;
                }
            }
            Self::Map { removed, entries } => {
                write_count(out, removed.len())?;
                for item in removed {
                    item.write(out)?;
                }
                write_count(out, entries.len())?;
                for (key, value) in entries {
                    key.write(out)?;
                    value.write(out)?;
                }
            }
            Self::Set { removed, elements } => {
                write_count(out, removed.len())?;
                for item in removed {
                    item.write(out)?;
                }
                write_count(out, elements.len())?;
                for item in elements {
                    item.write(out)?;
                }
            }
        }
        Ok(())
    }
}

impl Item {
    fn write(&self, out: &mut Vec<u8>) -> Result<()> {
        match self {
            Self::Scalar(scalar) => scalar.write_item(out)?,
            Self::Struct(tree) => tree.write(out)?,
            Self::UnnamedStruct(bytes) => out.extend_from_slice(bytes),
        }
        Ok(())
    }
}

impl TypedTree {
    /// Type every property of a framed tree. Fails only on structural
    /// inconsistency in the input tree; untypeable leaves become gaps.
    pub fn from_tree(tree: &PropertyTree) -> Result<(Self, TypedSummary)> {
        let mut typer = Typer {
            summary: TypedSummary::default(),
        };
        let typed = typer.tree(tree)?;
        Ok((typed, typer.summary))
    }

    pub fn write(&self, out: &mut Vec<u8>) -> Result<()> {
        for property in &self.properties {
            out.extend_from_slice(&property.header);
            property.value.write_payload(property.kind, out)?;
        }
        out.extend_from_slice(&self.terminator);
        Ok(())
    }

    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        let mut out = Vec::new();
        self.write(&mut out)?;
        Ok(out)
    }

    /// The first property whose name equals `name` exactly.
    pub fn get(&self, name: &str) -> Option<&TypedProperty> {
        self.properties
            .iter()
            .find(|property| is_name(&property.name, name))
    }
}

impl TypedComponent {
    /// Type a framed base component; its trailer is carried unchanged.
    pub fn from_component(component: &BaseComponentTree) -> Result<(Self, TypedSummary)> {
        let (tree, summary) = TypedTree::from_tree(&component.tree)?;
        Ok((
            Self {
                tree,
                trailer: component.trailer.clone(),
            },
            summary,
        ))
    }

    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        ensure!(
            self.trailer.len() == 4,
            "component trailer must be four bytes"
        );
        let mut out = self.tree.to_bytes()?;
        out.extend_from_slice(&self.trailer);
        Ok(out)
    }
}

impl fmt::Debug for TypedTree {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TypedTree")
            .field("properties", &self.properties.len())
            .field("terminator_bytes", &self.terminator.len())
            .finish()
    }
}

impl fmt::Debug for TypedProperty {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TypedProperty")
            .field("kind", &self.kind)
            .field("header_bytes", &self.header.len())
            .finish_non_exhaustive()
    }
}

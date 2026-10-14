//! Owned, lossless and value-opaque tree for one tagged property stream.
//!
//! A feasibility primitive for preserving player-created state byte for byte.
//! Names, types and values remain uninterpreted bytes; nested streams are framed
//! only where the existing strict readers accept them exactly, and every other
//! payload is retained verbatim. No schema, class recognition, value decoding,
//! migration or world import occurs, and selecting the profile is not a claim
//! about the producing build. Accepted inputs re-serialize to identical bytes.
use std::fmt;

use anyhow::{Context, Result, ensure};
use serde::Serialize;

use super::archive::{ByteOrder, read_string_frame};
use super::component::read_base_component_with_limit;
use super::struct_array::{MAX_BOUNDED_STRUCT_ARRAY_BYTES, read_tagged_struct_array_with_limit};
use super::tagged::{RootTag, TagKind, TagProfile, read_root};

pub const DEFAULT_MAX_TREE_DEPTH: usize = 8;
/// Hard ceiling for caller-selected nesting, not an original format limit.
pub const MAX_TREE_DEPTH: usize = 16;
pub const DEFAULT_MAX_TREE_NODES: usize = 65_536;
/// Hard ceiling for caller-selected node budgets, not an original format limit.
pub const MAX_TREE_NODES: usize = 1 << 20;

/// Research policy bounds. Root properties are at depth 1; a payload is framed
/// as a nested stream only when its properties would not exceed `max_depth`.
/// Exceeding `max_nodes` fails the whole parse rather than truncating it.
#[derive(Clone, Copy, Debug)]
pub struct TreeLimits {
    pub max_depth: usize,
    pub max_nodes: usize,
    /// Passed to the struct-array reader; larger Array payloads stay opaque.
    pub struct_array_bytes: usize,
}

impl Default for TreeLimits {
    fn default() -> Self {
        Self {
            max_depth: DEFAULT_MAX_TREE_DEPTH,
            max_nodes: DEFAULT_MAX_TREE_NODES,
            struct_array_bytes: MAX_BOUNDED_STRUCT_ARRAY_BYTES,
        }
    }
}

impl TreeLimits {
    fn validate(&self) -> Result<()> {
        ensure!(
            (1..=MAX_TREE_DEPTH).contains(&self.max_depth),
            "property tree depth limit must be between 1 and 16"
        );
        ensure!(
            (1..=MAX_TREE_NODES).contains(&self.max_nodes),
            "property tree node limit must be between 1 and 1,048,576"
        );
        ensure!(
            (1..=MAX_BOUNDED_STRUCT_ARRAY_BYTES).contains(&self.struct_array_bytes),
            "struct array byte limit must be between 1 byte and 128 KiB"
        );
        Ok(())
    }
}

/// One tagged stream: properties followed by the original terminal name record.
/// Byte-holding types print only kinds and lengths through a redacting Debug.
pub struct PropertyTree {
    pub properties: Vec<PropertyNode>,
    pub terminator: Vec<u8>,
}

pub struct PropertyNode {
    pub kind: TagKind,
    /// The original header, including name, type, size, index and extensions.
    pub header: Vec<u8>,
    pub value: NodeValue,
}

pub enum NodeValue {
    /// The exact original payload bytes.
    Opaque(Vec<u8>),
    /// A Struct payload that framed exactly as a terminated tagged stream.
    Struct(PropertyTree),
    /// An Array payload that framed exactly as a tagged struct array.
    StructArray {
        /// The original four-byte count.
        prefix: Vec<u8>,
        /// The original shared StructProperty header.
        shared_header: Vec<u8>,
        elements: Vec<PropertyTree>,
    },
}

/// Counts and lengths only; never names, types or values.
#[derive(Debug, Default, Serialize)]
pub struct KindCounts {
    pub array: usize,
    #[serde(rename = "struct")]
    pub struct_: usize,
    pub string: usize,
    pub name: usize,
    pub int: usize,
    pub uint32: usize,
    pub float: usize,
    pub text: usize,
    pub byte: usize,
    #[serde(rename = "enum")]
    pub enum_: usize,
    pub bool: usize,
    pub map: usize,
    pub set: usize,
    pub int64: usize,
}

#[derive(Debug, Serialize)]
pub struct PropertyTreeSummary {
    /// Every property node at every depth.
    pub property_count: usize,
    pub properties_by_kind: KindCounts,
    pub nested_structs: usize,
    pub struct_arrays: usize,
    pub struct_array_elements: usize,
    pub opaque_leaves: usize,
    pub opaque_payload_bytes: usize,
    /// Deepest property depth; root properties are depth 1, an empty root is 0.
    pub max_depth: usize,
    pub world_imported: bool,
    pub values_interpreted: bool,
}

struct Builder {
    profile: TagProfile,
    limits: TreeLimits,
    nodes: usize,
}

impl Builder {
    fn tree(
        &mut self,
        tags: &[RootTag<'_>],
        terminator: &[u8],
        depth: usize,
    ) -> Result<PropertyTree> {
        let properties = tags
            .iter()
            .map(|tag| self.node(tag, depth))
            .collect::<Result<_>>()?;
        Ok(PropertyTree {
            properties,
            terminator: terminator.to_vec(),
        })
    }

    /// Only the node budget propagates; framing failures select an opaque value.
    fn node(&mut self, tag: &RootTag<'_>, depth: usize) -> Result<PropertyNode> {
        ensure!(
            self.nodes < self.limits.max_nodes,
            "property tree node count exceeds selected limit"
        );
        self.nodes += 1;
        let nested = depth < self.limits.max_depth;
        let value = match tag.kind {
            TagKind::Struct if nested => match read_root(tag.payload, self.profile) {
                Ok(inner) => {
                    NodeValue::Struct(self.tree(&inner.tags, inner.terminator, depth + 1)?)
                }
                Err(_) => NodeValue::Opaque(tag.payload.to_vec()),
            },
            TagKind::Array if nested => match read_tagged_struct_array_with_limit(
                tag.payload,
                self.profile,
                self.limits.struct_array_bytes,
            ) {
                Ok(array) => {
                    let (prefix, _) = array.prefix().split_at(4);
                    let elements = array
                        .elements()
                        .iter()
                        .map(|e| self.tree(e.fields(), e.terminator(), depth + 1))
                        .collect::<Result<_>>()?;
                    NodeValue::StructArray {
                        prefix: prefix.to_vec(),
                        shared_header: array.shared().header.to_vec(),
                        elements,
                    }
                }
                Err(_) => NodeValue::Opaque(tag.payload.to_vec()),
            },
            _ => NodeValue::Opaque(tag.payload.to_vec()),
        };
        Ok(PropertyNode {
            kind: tag.kind,
            header: tag.header.to_vec(),
            value,
        })
    }
}

/// Read the declared payload size from a header without decoding its names.
/// The size follows the name and type strings under the supported profile.
fn declared_size(header: &[u8]) -> Result<usize> {
    let name = read_string_frame(header, ByteOrder::Little)
        .context("invalid property header name")?
        .consumed();
    let kind = read_string_frame(&header[name..], ByteOrder::Little)
        .context("invalid property header type")?
        .consumed();
    let start = name + kind;
    let size = header
        .get(start..start + 4)
        .context("truncated property header size")?;
    usize::try_from(i32::from_le_bytes(size.try_into()?)).context("negative property header size")
}

impl PropertyTree {
    /// Frame a complete root stream with `read_root`; it must end in `None` at
    /// exact EOF within 32 MiB. Nested Struct and Array payloads are framed only
    /// when they parse exactly within the depth limit; others stay opaque.
    pub fn parse(input: &[u8], profile: TagProfile, limits: TreeLimits) -> Result<Self> {
        limits.validate()?;
        let root = read_root(input, profile)?;
        let mut builder = Builder {
            profile,
            limits,
            nodes: 0,
        };
        builder.tree(&root.tags, root.terminator, 1)
    }

    /// Append the stream. Each header is written unchanged; the written payload
    /// length must equal its declared size, as must every struct array's count
    /// and shared size. Names, types and terminators are not re-validated.
    pub fn write(&self, out: &mut Vec<u8>) -> Result<()> {
        for property in &self.properties {
            property.write(out)?;
        }
        out.extend_from_slice(&self.terminator);
        Ok(())
    }

    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        let mut out = Vec::new();
        self.write(&mut out)?;
        Ok(out)
    }

    pub fn summary(&self) -> PropertyTreeSummary {
        let mut summary = PropertyTreeSummary {
            property_count: 0,
            properties_by_kind: KindCounts::default(),
            nested_structs: 0,
            struct_arrays: 0,
            struct_array_elements: 0,
            opaque_leaves: 0,
            opaque_payload_bytes: 0,
            max_depth: 0,
            world_imported: false,
            values_interpreted: false,
        };
        self.accumulate(&mut summary, 1);
        summary
    }

    fn accumulate(&self, summary: &mut PropertyTreeSummary, depth: usize) {
        for property in &self.properties {
            summary.property_count += 1;
            summary.max_depth = summary.max_depth.max(depth);
            let counts = &mut summary.properties_by_kind;
            *match property.kind {
                TagKind::Array => &mut counts.array,
                TagKind::Struct => &mut counts.struct_,
                TagKind::String => &mut counts.string,
                TagKind::Name => &mut counts.name,
                TagKind::Int => &mut counts.int,
                TagKind::UInt32 => &mut counts.uint32,
                TagKind::Float => &mut counts.float,
                TagKind::Text => &mut counts.text,
                TagKind::Byte => &mut counts.byte,
                TagKind::Enum => &mut counts.enum_,
                TagKind::Bool => &mut counts.bool,
                TagKind::Map => &mut counts.map,
                TagKind::Set => &mut counts.set,
                TagKind::Int64 => &mut counts.int64,
            } += 1;
            match &property.value {
                NodeValue::Opaque(bytes) => {
                    summary.opaque_leaves += 1;
                    summary.opaque_payload_bytes += bytes.len();
                }
                NodeValue::Struct(tree) => {
                    summary.nested_structs += 1;
                    tree.accumulate(summary, depth + 1);
                }
                NodeValue::StructArray { elements, .. } => {
                    summary.struct_arrays += 1;
                    summary.struct_array_elements += elements.len();
                    for element in elements {
                        element.accumulate(summary, depth + 1);
                    }
                }
            }
        }
    }
}

impl PropertyNode {
    fn write(&self, out: &mut Vec<u8>) -> Result<()> {
        let declared = declared_size(&self.header)?;
        out.extend_from_slice(&self.header);
        let start = out.len();
        match &self.value {
            NodeValue::Opaque(bytes) => out.extend_from_slice(bytes),
            NodeValue::Struct(tree) => tree.write(out)?,
            NodeValue::StructArray {
                prefix,
                shared_header,
                elements,
            } => {
                let count: [u8; 4] = prefix
                    .as_slice()
                    .try_into()
                    .context("struct array count prefix must be four bytes")?;
                ensure!(
                    usize::try_from(i32::from_le_bytes(count)).ok() == Some(elements.len()),
                    "struct array count does not match element count"
                );
                let shared = declared_size(shared_header)?;
                out.extend_from_slice(prefix);
                out.extend_from_slice(shared_header);
                let elements_start = out.len();
                for element in elements {
                    element.write(out)?;
                }
                ensure!(
                    out.len() - elements_start == shared,
                    "struct array elements do not match shared declared size"
                );
            }
        }
        ensure!(
            out.len() - start == declared,
            "property payload does not match declared size"
        );
        Ok(())
    }
}

/// A tree in the caller-selected base-component layout: properties, terminal
/// `None` and the original four-byte object trailer. No class is recognized.
pub struct BaseComponentTree {
    pub tree: PropertyTree,
    pub trailer: Vec<u8>,
}

impl BaseComponentTree {
    /// Frame with `read_base_component_with_limit` (1 byte to 1 MiB), then
    /// apply the same nested framing and limits as `PropertyTree::parse`.
    pub fn parse(
        input: &[u8],
        profile: TagProfile,
        max_bytes: usize,
        limits: TreeLimits,
    ) -> Result<Self> {
        limits.validate()?;
        let component = read_base_component_with_limit(input, profile, max_bytes)?;
        let mut builder = Builder {
            profile,
            limits,
            nodes: 0,
        };
        Ok(Self {
            tree: builder.tree(component.fields(), component.terminator(), 1)?,
            trailer: component.trailer().to_vec(),
        })
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

    pub fn summary(&self) -> PropertyTreeSummary {
        self.tree.summary()
    }
}

impl fmt::Debug for PropertyTree {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PropertyTree")
            .field("properties", &self.properties)
            .field("terminator_bytes", &self.terminator.len())
            .finish()
    }
}

impl fmt::Debug for PropertyNode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PropertyNode")
            .field("kind", &self.kind)
            .field("header_bytes", &self.header.len())
            .field("value", &self.value)
            .finish()
    }
}

impl fmt::Debug for NodeValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Opaque(bytes) => f
                .debug_struct("Opaque")
                .field("bytes", &bytes.len())
                .finish(),
            Self::Struct(tree) => f.debug_tuple("Struct").field(tree).finish(),
            Self::StructArray {
                prefix,
                shared_header,
                elements,
            } => f
                .debug_struct("StructArray")
                .field("prefix_bytes", &prefix.len())
                .field("shared_header_bytes", &shared_header.len())
                .field("elements", elements)
                .finish(),
        }
    }
}

impl fmt::Debug for BaseComponentTree {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BaseComponentTree")
            .field("tree", &self.tree)
            .field("trailer_bytes", &self.trailer.len())
            .finish()
    }
}

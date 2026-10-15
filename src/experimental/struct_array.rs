//! Framing for an explicitly caller-selected array of tagged struct elements.
//!
//! This candidate layout has a count, one shared StructProperty header and
//! variable-length tagged element streams. Payloads remain opaque. It does not
//! recognize classes, compare schemas, decode nested values or import a world.
use anyhow::{Context, Result, bail, ensure};
use serde::Serialize;

use super::tagged::{ParsedRecord, RootTag, TagKind, TagProfile, read_record};

pub const MAX_STRUCT_ARRAY_BYTES: usize = 64 * 1024;
/// Hard ceiling for caller-selected research bounds, equal to the 1 MiB
/// component ceiling; not an original format limit.
pub const MAX_BOUNDED_STRUCT_ARRAY_BYTES: usize = 1024 * 1024;
pub const MAX_STRUCT_ARRAY_ELEMENTS: usize = 4096;
pub const MAX_STRUCT_ELEMENT_FIELDS: usize = 256;
const MIN_TERMINATOR_BYTES: usize = 9;

/// Borrowed spans and immutable parsed fields; deliberately not Debug/Serialize.
pub struct TaggedStructElement<'a> {
    record: &'a [u8],
    fields: Vec<RootTag<'a>>,
    terminator: &'a [u8],
}

impl<'a> TaggedStructElement<'a> {
    /// The complete element, including its terminal name.
    pub fn record(&self) -> &'a [u8] {
        self.record
    }

    pub fn fields(&self) -> &[RootTag<'a>] {
        &self.fields
    }

    pub fn terminator(&self) -> &'a [u8] {
        self.terminator
    }
}

/// Private storage prevents mutation of the validated count and boundaries.
pub struct TaggedStructArray<'a> {
    prefix: &'a [u8],
    shared: RootTag<'a>,
    elements: Vec<TaggedStructElement<'a>>,
}

#[derive(Debug, Serialize)]
pub struct TaggedStructArraySummary {
    pub element_count: usize,
    pub field_count: usize,
    /// Four-byte count and shared header, excluding the shared payload.
    pub array_prefix_bytes: usize,
    /// Includes every element's terminal name.
    pub element_header_bytes: usize,
    pub opaque_payload_bytes: usize,
    pub world_imported: bool,
    pub values_interpreted: bool,
}

impl<'a> TaggedStructArray<'a> {
    pub fn prefix(&self) -> &'a [u8] {
        self.prefix
    }

    pub fn shared(&self) -> &RootTag<'a> {
        &self.shared
    }

    pub fn elements(&self) -> &[TaggedStructElement<'a>] {
        &self.elements
    }

    pub fn summary(&self) -> TaggedStructArraySummary {
        TaggedStructArraySummary {
            element_count: self.elements.len(),
            field_count: self.elements.iter().map(|e| e.fields.len()).sum(),
            array_prefix_bytes: self.prefix.len(),
            element_header_bytes: self
                .elements
                .iter()
                .map(|e| {
                    e.terminator.len() + e.fields.iter().map(|f| f.header.len()).sum::<usize>()
                })
                .sum(),
            opaque_payload_bytes: self
                .elements
                .iter()
                .flat_map(|e| &e.fields)
                .map(|f| f.payload.len())
                .sum(),
            world_imported: false,
            values_interpreted: false,
        }
    }
}

/// Require a nonnegative LE i32 count and a shared StructProperty header even
/// when the count is zero. Its payload must contain exactly that many tagged
/// element streams, each ending in None, with no object trailers or extra bytes.
/// Requiring a shared StructProperty is an explicit strict research policy;
/// names, nonnegative indices and struct identities are not compared to a schema.
pub fn read_tagged_struct_array(
    input: &[u8],
    profile: TagProfile,
) -> Result<TaggedStructArray<'_>> {
    ensure!(
        input.len() <= MAX_STRUCT_ARRAY_BYTES,
        "struct array input exceeds 64 KiB research limit"
    );
    read_tagged_struct_array_with_limit(input, profile, MAX_STRUCT_ARRAY_BYTES)
}

/// Frame the same opaque layout under an explicit byte limit, up to 1 MiB.
/// The 4,096-element and 256-field caps and all framing/EOF checks remain in
/// force. The ceiling is research policy, not a recovered original format limit.
/// A zero limit is invalid; selecting a larger limit does not recognize schemas.
pub fn read_tagged_struct_array_with_limit(
    input: &[u8],
    profile: TagProfile,
    max_bytes: usize,
) -> Result<TaggedStructArray<'_>> {
    ensure!(
        (1..=MAX_BOUNDED_STRUCT_ARRAY_BYTES).contains(&max_bytes),
        "struct array byte limit must be between 1 byte and 1 MiB"
    );
    ensure!(
        input.len() <= max_bytes,
        "struct array input exceeds selected byte limit"
    );
    let count_bytes = input.get(..4).context("truncated struct array count")?;
    let count = usize::try_from(i32::from_le_bytes(count_bytes.try_into()?))
        .context("negative struct array count")?;
    ensure!(
        count <= MAX_STRUCT_ARRAY_ELEMENTS,
        "struct array element count exceeds research limit"
    );
    let ParsedRecord::Property(property) =
        read_record(&input[4..], profile).context("invalid struct array shared header")?
    else {
        bail!("struct array shared header is required");
    };
    let shared = property.tag;
    ensure!(
        shared.kind == TagKind::Struct,
        "struct array shared header must be StructProperty"
    );
    ensure!(
        shared.record.len() == input.len() - 4,
        "trailing data after struct array shared payload"
    );
    let minimum = count
        .checked_mul(MIN_TERMINATOR_BYTES)
        .context("struct array minimum size overflow")?;
    ensure!(
        minimum <= shared.payload.len(),
        "struct array count exceeds available element bytes"
    );
    // The count and minimum byte budget are checked before allocating elements.
    let mut elements = Vec::with_capacity(count);
    let mut position = 0;
    for _ in 0..count {
        let start = position;
        let mut fields = Vec::new();
        loop {
            let record = read_record(&shared.payload[position..], profile)
                .with_context(|| format!("invalid struct array element at offset {position}"))?;
            match record {
                ParsedRecord::Property(property) => {
                    ensure!(
                        fields.len() < MAX_STRUCT_ELEMENT_FIELDS,
                        "struct array element field count exceeds research limit"
                    );
                    // read_record checked that this entire record fits the suffix.
                    position += property.tag.record.len();
                    fields.push(property.tag);
                }
                ParsedRecord::Terminator(terminator) => {
                    position += terminator.len();
                    elements.push(TaggedStructElement {
                        record: &shared.payload[start..position],
                        fields,
                        terminator,
                    });
                    break;
                }
            }
        }
    }
    ensure!(
        position == shared.payload.len(),
        "trailing data after struct array elements"
    );
    Ok(TaggedStructArray {
        prefix: &input[..4 + shared.header.len()],
        shared,
        elements,
    })
}

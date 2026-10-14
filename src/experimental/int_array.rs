//! Opaque framing for a caller-selected ordinary IntProperty array candidate.
//!
//! The caller must establish the concrete schema and ordinary, non-swapping
//! loading path. Selecting MemoryArchive522 alone does not prove those archive
//! flags. This layout has a little-endian signed count and four opaque bytes per
//! element under the reviewed healthy archive path with archive+0x28 bit 0x20
//! and archive+0x2b bit 0x10 clear, excluding alternate bulk/property-list modes.
//! There is no shared struct header or trailer. Negative-count rejection
//! and exact EOF are strict research policies, not claims about original error
//! handling, successful loading, numeric semantics or world import.
use anyhow::{Context, Result, ensure};
use serde::Serialize;

use super::tagged::TagProfile;

pub const MAX_INT_ARRAY_BYTES: usize = 64 * 1024;
pub const MAX_INT_ARRAY_ELEMENTS: usize = 4096;

/// Validated borrowed spans; deliberately neither Debug nor Serialize.
/// The element limit makes the largest accepted input 16,388 bytes, even though
/// the independent early byte guard permits up to 64 KiB.
pub struct IntArray<'a> {
    prefix: &'a [u8],
    elements: &'a [[u8; 4]],
}

#[derive(Debug, Serialize)]
pub struct IntArraySummary {
    pub element_count: usize,
    pub array_prefix_bytes: usize,
    pub opaque_payload_bytes: usize,
    pub values_interpreted: bool,
    pub world_imported: bool,
}

impl<'a> IntArray<'a> {
    pub fn prefix(&self) -> &'a [u8] {
        self.prefix
    }

    /// Raw four-byte source slices, without decoding their numerical values.
    pub fn elements(&self) -> &'a [[u8; 4]] {
        self.elements
    }

    pub fn summary(&self) -> IntArraySummary {
        IntArraySummary {
            element_count: self.elements.len(),
            array_prefix_bytes: self.prefix.len(),
            opaque_payload_bytes: self.elements.len() * 4,
            values_interpreted: false,
            world_imported: false,
        }
    }
}

/// Require a nonnegative LE i32 count, exactly four bytes per element and EOF.
/// No element allocation or value interpretation occurs. The caller must have
/// established the ordinary non-swapping path; other array modes are unsupported.
pub fn read_int_array(input: &[u8], profile: TagProfile) -> Result<IntArray<'_>> {
    match profile {
        TagProfile::MemoryArchive522 => {}
    }
    ensure!(
        input.len() <= MAX_INT_ARRAY_BYTES,
        "int array input exceeds 64 KiB research limit"
    );
    let prefix = input.get(..4).context("truncated int array count")?;
    let count = usize::try_from(i32::from_le_bytes(prefix.try_into()?))
        .context("negative int array count unsupported")?;
    ensure!(
        count <= MAX_INT_ARRAY_ELEMENTS,
        "int array element count exceeds research limit"
    );
    let payload_bytes = count.checked_mul(4).context("int array length overflow")?;
    let expected = payload_bytes
        .checked_add(4)
        .context("int array length overflow")?;
    ensure!(
        input.len() == expected,
        "int array count does not match exact input extent"
    );
    let (elements, remainder) = input[4..].as_chunks::<4>();
    ensure!(remainder.is_empty(), "incomplete int array element");
    Ok(IntArray { prefix, elements })
}

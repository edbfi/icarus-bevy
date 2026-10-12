//! Opaque framing for an explicitly caller-selected base-component candidate.
//!
//! This layout consists of tagged fields, a terminal name and a four-byte false
//! object-trailer flag. Selecting it does not recognize a component class, verify
//! its schema or establish compatibility with the save's producing build. Values
//! remain uninterpreted; this module does not construct objects or import worlds.
use anyhow::{Context, Result, bail, ensure};
use serde::Serialize;

use super::tagged::{ParsedRecord, RootTag, TagProfile, read_record};

pub const MAX_COMPONENT_BYTES: usize = 64 * 1024;
pub const MAX_COMPONENT_FIELDS: usize = 256;

/// Exact borrowed source spans; deliberately neither Debug nor Serialize.
pub struct BaseComponent<'a> {
    fields: Vec<RootTag<'a>>,
    terminator: &'a [u8],
    trailer: &'a [u8],
    profile: TagProfile,
}

#[derive(Debug, Serialize)]
pub struct BaseComponentSummary {
    pub profile: &'static str,
    pub field_count: usize,
    /// Includes the terminal name, but excludes the object trailer.
    pub header_bytes: usize,
    pub opaque_payload_bytes: usize,
    pub trailer_bytes: usize,
    pub world_imported: bool,
    pub values_interpreted: bool,
}

impl<'a> BaseComponent<'a> {
    pub fn fields(&self) -> &[RootTag<'a>] {
        &self.fields
    }

    pub fn terminator(&self) -> &'a [u8] {
        self.terminator
    }

    pub fn trailer(&self) -> &'a [u8] {
        self.trailer
    }

    pub fn summary(&self) -> BaseComponentSummary {
        BaseComponentSummary {
            profile: match self.profile {
                TagProfile::MemoryArchive522 => "experimental_memory_archive_522",
            },
            field_count: self.fields.len(),
            header_bytes: self.terminator.len()
                + self
                    .fields
                    .iter()
                    .map(|field| field.header.len())
                    .sum::<usize>(),
            opaque_payload_bytes: self.fields.iter().map(|field| field.payload.len()).sum(),
            trailer_bytes: self.trailer.len(),
            world_imported: false,
            values_interpreted: false,
        }
    }
}

/// Frame only this caller-selected candidate layout, with at most 64 KiB and
/// 256 fields. Unknown property names of supported header kinds remain opaque.
/// The terminal name must be followed by exactly four zero bytes and EOF.
/// A true object-trailer flag is explicitly unsupported, including when the
/// following GUID bytes are present. No class or field-schema validation occurs.
pub fn read_base_component(input: &[u8], profile: TagProfile) -> Result<BaseComponent<'_>> {
    ensure!(
        input.len() <= MAX_COMPONENT_BYTES,
        "component input exceeds 64 KiB research limit"
    );
    let mut position = 0;
    let mut fields = Vec::new();
    loop {
        let record = read_record(&input[position..], profile)
            .with_context(|| format!("invalid component field at offset {position}"))?;
        match record {
            ParsedRecord::Property(property) => {
                ensure!(
                    fields.len() < MAX_COMPONENT_FIELDS,
                    "component field count exceeds research limit"
                );
                // read_record checked that the entire record fits this suffix.
                position += property.tag.record.len();
                fields.push(property.tag);
            }
            ParsedRecord::Terminator(terminator) => {
                position += terminator.len();
                let suffix = &input[position..];
                let trailer = suffix.get(..4).context("truncated component trailer")?;
                match u32::from_le_bytes(trailer.try_into()?) {
                    0 => {}
                    1 => bail!("unsupported component GUID trailer"),
                    _ => bail!("noncanonical component trailer flag"),
                }
                ensure!(suffix.len() == 4, "trailing data after component trailer");
                return Ok(BaseComponent {
                    fields,
                    terminator,
                    trailer,
                    profile,
                });
            }
        }
    }
}

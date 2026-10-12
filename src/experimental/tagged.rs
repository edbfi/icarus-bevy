//! Strict root-property framing for one explicitly selected research profile.
//!
//! Values remain opaque borrowed bytes. This is neither schema validation nor
//! save import, and the profile is not a claim about the save's producing build.
use anyhow::{Context, Result, bail, ensure};
use serde::Serialize;

use super::archive::{ByteOrder, decode_string};

pub const MAX_INPUT_BYTES: usize = 32 * 1024 * 1024;
pub const MAX_ROOT_TAGS: usize = 4096;
pub const MAX_HEADER_STRING_UNITS: usize = 1024;

#[derive(Clone, Copy, Debug)]
pub enum TagProfile {
    /// LE FString names and version-522 property-header extensions. Strict policy
    /// requires canonical `None`, terminated strings, and zero/one flag bytes.
    MemoryArchive522,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TagKind {
    Array,
    Struct,
    String,
    Name,
    Int,
    Byte,
    Enum,
    Bool,
}

/// Exact source slices; intentionally not serializable or debug-printable.
pub struct RootTag<'a> {
    pub kind: TagKind,
    pub record: &'a [u8],
    /// Includes the original property name, type and all header extensions.
    pub header: &'a [u8],
    pub payload: &'a [u8],
}

pub struct RootTags<'a> {
    pub tags: Vec<RootTag<'a>>,
    pub terminator: &'a [u8],
    profile: TagProfile,
}

#[derive(Debug, Serialize)]
pub struct RootTagSummary {
    pub profile: &'static str,
    pub root_property_count: usize,
    pub opaque_payload_bytes: usize,
    /// Includes the terminal name record as well as all property headers.
    pub header_bytes: usize,
}

impl RootTags<'_> {
    pub fn summary(&self) -> RootTagSummary {
        RootTagSummary {
            profile: match self.profile {
                TagProfile::MemoryArchive522 => "experimental_memory_archive_522",
            },
            root_property_count: self.tags.len(),
            opaque_payload_bytes: self.tags.iter().map(|tag| tag.payload.len()).sum(),
            header_bytes: self.terminator.len()
                + self.tags.iter().map(|tag| tag.header.len()).sum::<usize>(),
        }
    }
}

struct Cursor<'a> {
    input: &'a [u8],
    position: usize,
}

impl<'a> Cursor<'a> {
    fn take(&mut self, length: usize) -> Result<&'a [u8]> {
        let start = self.position;
        let end = start.checked_add(length).context("tag offset overflow")?;
        let bytes = self
            .input
            .get(start..end)
            .with_context(|| format!("truncated tag data at offset {start}"))?;
        self.position = end;
        Ok(bytes)
    }

    fn i32(&mut self) -> Result<i32> {
        Ok(i32::from_le_bytes(self.take(4)?.try_into()?))
    }

    fn flag(&mut self) -> Result<bool> {
        let offset = self.position;
        match self.take(1)?[0] {
            0 => Ok(false),
            1 => Ok(true),
            _ => bail!("unsupported tag flag at offset {offset}"),
        }
    }

    fn name(&mut self) -> Result<Vec<u16>> {
        let start = self.position;
        let count = self.i32()?;
        ensure!(
            count.unsigned_abs() as usize <= MAX_HEADER_STRING_UNITS,
            "tag string exceeds research unit limit at offset {start}"
        );
        // Check this profile's smaller bound before the generic codec allocates.
        let decoded = decode_string(&self.input[start..], ByteOrder::Little)
            .with_context(|| format!("invalid tag string at offset {start}"))?;
        self.position = start + decoded.consumed; // Codec checked the full range.
        ensure!(
            !decoded.units.is_empty(),
            "empty tag name unsupported at offset {start}"
        );
        Ok(decoded.units)
    }
}

pub(crate) fn is_name(units: &[u16], name: &str) -> bool {
    units.iter().copied().eq(name.encode_utf16())
}

pub(crate) struct PropertyRecord<'a> {
    pub tag: RootTag<'a>,
    pub name: Vec<u16>,
    pub extra: Option<Vec<u16>>,
    pub index: i32,
}

pub(crate) enum ParsedRecord<'a> {
    Property(PropertyRecord<'a>),
    Terminator(&'a [u8]),
}

/// Read one bounded header and its declared opaque payload, or a terminal name.
/// The caller owns sequence/count/EOF policy. No property values are decoded.
pub(crate) fn read_record(input: &[u8], profile: TagProfile) -> Result<ParsedRecord<'_>> {
    ensure!(
        input.len() <= MAX_INPUT_BYTES,
        "tag input exceeds 32 MiB limit"
    );
    let mut cursor = Cursor { input, position: 0 };
    let name = cursor.name()?;
    if is_name(&name, "None") {
        return Ok(ParsedRecord::Terminator(&input[..cursor.position]));
    }
    let type_offset = cursor.position;
    let type_name = cursor.name()?;
    let kind = [
        ("ArrayProperty", TagKind::Array),
        ("StructProperty", TagKind::Struct),
        ("StrProperty", TagKind::String),
        ("NameProperty", TagKind::Name),
        ("IntProperty", TagKind::Int),
        ("ByteProperty", TagKind::Byte),
        ("EnumProperty", TagKind::Enum),
        ("BoolProperty", TagKind::Bool),
    ]
    .into_iter()
    .find_map(|(name, kind)| is_name(&type_name, name).then_some(kind))
    .with_context(|| format!("unsupported tag header kind at offset {type_offset}"))?;
    let size = cursor.i32()?;
    let index = cursor.i32()?;
    ensure!(size >= 0, "negative tag payload size");
    ensure!(index >= 0, "negative tag array index");
    let mut extra = None;
    match profile {
        TagProfile::MemoryArchive522 => {
            match kind {
                TagKind::Struct => {
                    extra = Some(cursor.name()?);
                    cursor.take(16)?;
                }
                TagKind::Array | TagKind::Byte | TagKind::Enum => {
                    extra = Some(cursor.name()?);
                }
                TagKind::Bool => {
                    cursor.flag()?;
                }
                TagKind::String | TagKind::Name | TagKind::Int => {}
            }
            if cursor.flag()? {
                cursor.take(16)?;
            }
        }
    }
    let payload_start = cursor.position;
    let payload = cursor.take(usize::try_from(size)?)?;
    Ok(ParsedRecord::Property(PropertyRecord {
        tag: RootTag {
            kind,
            record: &input[..cursor.position],
            header: &input[..payload_start],
            payload,
        },
        name,
        extra,
        index,
    }))
}

/// Frame root tags only, skipping values by their declared size. Unknown names
/// and payloads are retained verbatim, but unsupported type headers fail closed.
/// Negative sizes/indices, noncanonical flag bytes, absent terminal names and
/// trailing bytes are rejected as explicit research policy.
pub fn read_root(input: &[u8], profile: TagProfile) -> Result<RootTags<'_>> {
    ensure!(
        input.len() <= MAX_INPUT_BYTES,
        "tag input exceeds 32 MiB limit"
    );
    let mut cursor = Cursor { input, position: 0 };
    let mut tags = Vec::new();
    loop {
        let start = cursor.position;
        let name = cursor.name()?;
        if is_name(&name, "None") {
            ensure!(
                cursor.position == input.len(),
                "trailing data after root tag terminator"
            );
            return Ok(RootTags {
                tags,
                terminator: &input[start..cursor.position],
                profile,
            });
        }
        ensure!(
            tags.len() < MAX_ROOT_TAGS,
            "root tag count exceeds research limit"
        );
        let ParsedRecord::Property(property) = read_record(&input[start..], profile)? else {
            unreachable!("nonterminal name was checked above");
        };
        cursor.position = start + property.tag.record.len();
        tags.push(property.tag);
    }
}

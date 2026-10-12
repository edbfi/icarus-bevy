//! Strict experimental framing of the observed StateRecorderBlobs array.
//!
//! This accepts one shared StateRecorderBlob struct tag and exact tagged element
//! boundaries. Class strings and recorder bodies remain borrowed opaque bytes;
//! no class lookup, gameplay interpretation, object construction or import occurs.
use anyhow::{Context, Result, bail, ensure};
use serde::Serialize;

use super::archive::{ByteOrder, decode_string};
use super::tagged::{
    MAX_HEADER_STRING_UNITS, MAX_INPUT_BYTES, MAX_ROOT_TAGS, ParsedRecord, PropertyRecord, RootTag,
    TagKind, TagProfile, is_name, read_record,
};

pub const MAX_RECORDERS: usize = 16_384;
pub const MAX_RECORDER_FIELDS: usize = 65_536;

/// Source spans only; deliberately neither Debug nor Serialize.
pub struct Recorder<'a> {
    /// Complete element, including its terminal name and every unknown field.
    pub record: &'a [u8],
    fields: Vec<RootTag<'a>>,
    pub terminator: &'a [u8],
    /// Complete encoded FString, not a decoded class identifier.
    pub class_value: &'a [u8],
    /// Opaque bytes after the validated BinaryData count.
    pub binary_data: &'a [u8],
}

pub struct RecorderArray<'a> {
    pub root_record: &'a [u8],
    pub root_header: &'a [u8],
    pub count_prefix: &'a [u8],
    pub shared_header: &'a [u8],
    records: Vec<Recorder<'a>>,
    profile: TagProfile,
}

#[derive(Debug, Serialize)]
pub struct RecorderSummary {
    pub profile: &'static str,
    pub recorder_count: usize,
    pub field_tags: usize,
    pub class_fields: usize,
    pub binary_fields: usize,
    pub unknown_fields: usize,
    pub opaque_binary_bytes: usize,
    pub class_value_bytes: usize,
    pub world_imported: bool,
}

impl<'a> Recorder<'a> {
    pub fn fields(&self) -> &[RootTag<'a>] {
        &self.fields
    }
}

impl<'a> RecorderArray<'a> {
    pub fn records(&self) -> &[Recorder<'a>] {
        &self.records
    }

    pub fn summary(&self) -> RecorderSummary {
        let count = self.records.len();
        let fields = self.records.iter().map(|record| record.fields.len()).sum();
        RecorderSummary {
            profile: match self.profile {
                TagProfile::MemoryArchive522 => "experimental_memory_archive_522",
            },
            recorder_count: count,
            field_tags: fields,
            class_fields: count,
            binary_fields: count,
            unknown_fields: fields - count * 2,
            opaque_binary_bytes: self
                .records
                .iter()
                .map(|record| record.binary_data.len())
                .sum(),
            class_value_bytes: self
                .records
                .iter()
                .map(|record| record.class_value.len())
                .sum(),
            world_imported: false,
        }
    }
}

fn count(input: &[u8]) -> Result<usize> {
    let bytes = input.get(..4).context("truncated recorder count")?;
    let value = i32::from_le_bytes(bytes.try_into()?);
    ensure!(value >= 0, "negative recorder count");
    Ok(usize::try_from(value)?)
}

fn extra_is(property: &PropertyRecord<'_>, expected: &str) -> bool {
    property
        .extra
        .as_ref()
        .is_some_and(|name| is_name(name, expected))
}

fn validate_class_value(input: &[u8]) -> Result<()> {
    let prefix = input.get(..4).context("truncated recorder string")?;
    let count = i32::from_le_bytes(prefix.try_into()?);
    ensure!(
        count.unsigned_abs() as usize <= MAX_HEADER_STRING_UNITS,
        "recorder string exceeds research unit limit"
    );
    // Empty and raw UTF-16 strings are framing only, not valid-class assertions.
    let value = decode_string(input, ByteOrder::Little)?;
    ensure!(
        value.consumed == input.len(),
        "trailing recorder string data"
    );
    Ok(())
}

/// Inspect the explicit MemoryArchive522 framing subset after envelope integrity
/// and root framing validation. Exactly one selected root and exactly one of each
/// known field per element are required. Unknown supported-header fields retain
/// their complete original bytes. No unknown header or malformed data is skipped.
pub fn read_recorders<'a>(roots: &[RootTag<'a>], profile: TagProfile) -> Result<RecorderArray<'a>> {
    ensure!(
        roots.len() <= MAX_ROOT_TAGS,
        "recorder root count exceeds research limit"
    );
    let total = roots.iter().try_fold(0usize, |size, root| {
        size.checked_add(root.record.len())
            .context("recorder input length overflow")
    })?;
    ensure!(
        total <= MAX_INPUT_BYTES,
        "recorder input exceeds 32 MiB limit"
    );
    let mut selected = None;
    for root in roots {
        // Reparse bounded metadata without trusting caller-constructed RootTag fields.
        let ParsedRecord::Property(property) = read_record(root.record, profile)? else {
            bail!("unexpected recorder root terminator");
        };
        ensure!(
            property.tag.record.len() == root.record.len(),
            "trailing recorder root data"
        );
        if is_name(&property.name, "StateRecorderBlobs") {
            ensure!(selected.is_none(), "duplicate recorder root");
            ensure!(
                property.tag.kind == TagKind::Array
                    && extra_is(&property, "StructProperty")
                    && property.index == 0,
                "unsupported recorder root header"
            );
            selected = Some(property);
        }
    }
    let selected = selected.context("missing recorder root")?;
    let payload = selected.tag.payload;
    let declared = count(payload)?;
    ensure!(
        declared <= MAX_RECORDERS,
        "recorder count exceeds research limit"
    );
    let ParsedRecord::Property(shared) = read_record(&payload[4..], profile)? else {
        bail!("missing shared recorder struct header");
    };
    ensure!(
        is_name(&shared.name, "StateRecorderBlobs")
            && shared.tag.kind == TagKind::Struct
            && extra_is(&shared, "StateRecorderBlob")
            && shared.index == 0,
        "unsupported shared recorder struct header"
    );
    ensure!(
        shared.tag.record.len() == payload.len() - 4,
        "shared recorder aggregate size mismatch"
    );
    let body = shared.tag.payload;
    let mut position = 0;
    let mut field_count = 0;
    let mut records = Vec::new();
    for _ in 0..declared {
        let start = position;
        let mut fields = Vec::new();
        let mut class_value = None;
        let mut binary_data = None;
        let terminator = loop {
            match read_record(&body[position..], profile)? {
                ParsedRecord::Terminator(bytes) => {
                    position += bytes.len();
                    break bytes;
                }
                ParsedRecord::Property(property) => {
                    ensure!(
                        field_count < MAX_RECORDER_FIELDS,
                        "recorder field count exceeds research limit"
                    );
                    field_count += 1;
                    let tag = property.tag;
                    if is_name(&property.name, "ComponentClassName") {
                        ensure!(class_value.is_none(), "duplicate recorder class field");
                        ensure!(
                            tag.kind == TagKind::String && property.index == 0,
                            "unsupported recorder class header"
                        );
                        validate_class_value(tag.payload)?;
                        class_value = Some(tag.payload);
                    } else if is_name(&property.name, "BinaryData") {
                        ensure!(binary_data.is_none(), "duplicate recorder binary field");
                        ensure!(
                            tag.kind == TagKind::Array
                                && property.index == 0
                                && property
                                    .extra
                                    .as_ref()
                                    .is_some_and(|name| is_name(name, "ByteProperty")),
                            "unsupported recorder binary header"
                        );
                        let bytes = count(tag.payload)?;
                        ensure!(
                            bytes == tag.payload.len() - 4,
                            "recorder byte array count mismatch"
                        );
                        binary_data = Some(&tag.payload[4..]);
                    }
                    position += tag.record.len();
                    fields.push(tag);
                }
            }
        };
        records.push(Recorder {
            record: &body[start..position],
            fields,
            terminator,
            class_value: class_value.context("missing recorder class field")?,
            binary_data: binary_data.context("missing recorder binary field")?,
        });
    }
    ensure!(position == body.len(), "trailing recorder aggregate data");
    Ok(RecorderArray {
        root_record: selected.tag.record,
        root_header: selected.tag.header,
        count_prefix: &payload[..4],
        shared_header: shared.tag.header,
        records,
        profile,
    })
}

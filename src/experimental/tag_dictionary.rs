//! Gameplay-tag net-index dictionary for the experimental net-index codec.
//!
//! Callers may supply an already-ordered runtime sequence with explicit widths
//! (`from_ordered_names`) or build the node sequence from declared tag strings
//! (`from_tag_declarations`, which expands dot-separated parent prefixes,
//! deduplicates and sorts ASCII-case-insensitively, derives `total_bits` and
//! clamps `first_segment`, and computes `net_index_hash`).
//! This module does not load original assets or configuration files directly.

use std::collections::HashMap;
use std::fmt;

use super::rep_layout::TagEncoding;

/// Resource and sentinel limit of the supplied-schema profile.
pub const MAX_TAGS: usize = 65_534;
/// Maximum ASCII bytes per name in this authored profile.
pub const MAX_TAG_NAME_BYTES: usize = 1024;

/// An index's meaning in this supplied schema. No tag names appear in diagnostics.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TagIndexKind {
    Assigned,
    /// The index immediately after the last assigned index.
    Unused,
    /// The next index after the unused slot.
    Invalid,
    /// Outside the assigned indices and the two reserved slots.
    OutOfRange,
}

/// Validation failures deliberately omit supplied names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TagDictionaryError {
    InvalidTotalBits,
    InvalidFirstSegment,
    InvalidContainerBits,
    TooManyTags,
    InvalidName,
    DuplicateName,
    InsufficientIndexWidth,
}

impl fmt::Display for TagDictionaryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidTotalBits => "tag index width must be 1 to 16",
            Self::InvalidFirstSegment => "tag first-segment width must be 0 to 16",
            Self::InvalidContainerBits => "tag container width must be 1 to 8",
            Self::TooManyTags => "tag dictionary exceeds its count limit",
            Self::InvalidName => "tag name is outside the bounded ASCII profile",
            Self::DuplicateName => "tag dictionary contains an ASCII-case-insensitive duplicate",
            Self::InsufficientIndexWidth => "tag index width cannot encode the invalid sentinel",
        })
    }
}

impl std::error::Error for TagDictionaryError {}

/// The supplied runtime order, with zero-based indices and explicit codec widths.
///
/// Names retain the supplied spelling. Lookup folds ASCII case only; this is an
/// authored ASCII profile, with no claim about non-ASCII engine name semantics.
#[derive(Clone)]
pub struct TagDictionary {
    names: Vec<String>,
    indices: HashMap<String, u16>,
    total_bits: u8,
    first_segment: u8,
    container_bits: u8,
}

impl fmt::Debug for TagDictionary {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TagDictionary")
            .field("tag_count", &self.names.len())
            .field("total_bits", &self.total_bits)
            .field("first_segment", &self.first_segment)
            .field("container_bits", &self.container_bits)
            .finish_non_exhaustive()
    }
}

impl TagDictionary {
    /// Accept an explicitly complete runtime sequence in its original order.
    ///
    /// Each ASCII name consists of printable bytes and nonempty dot-separated
    /// segments. This is an authored input profile, not recovered runtime name
    /// validation. Names are neither inferred from configuration nor normalized.
    /// Index widths must be 1 through 16, first-segment widths 0 through 16,
    /// and container widths 1 through 8, matching the property-layout codec.
    /// Zero or `first_segment >= total_bits` selects the codec's unsplit form.
    /// `total_bits` must represent both reserved indices, including `count + 1`.
    pub fn from_ordered_names(
        names: impl IntoIterator<Item = impl AsRef<str>>,
        total_bits: u8,
        first_segment: u8,
        container_bits: u8,
    ) -> Result<Self, TagDictionaryError> {
        if !(1..=16).contains(&total_bits) {
            return Err(TagDictionaryError::InvalidTotalBits);
        }
        if first_segment > 16 {
            return Err(TagDictionaryError::InvalidFirstSegment);
        }
        if !(1..=8).contains(&container_bits) {
            return Err(TagDictionaryError::InvalidContainerBits);
        }
        let mut dictionary = Self {
            names: Vec::new(),
            indices: HashMap::new(),
            total_bits,
            first_segment,
            container_bits,
        };
        for name in names {
            if dictionary.names.len() == MAX_TAGS {
                return Err(TagDictionaryError::TooManyTags);
            }
            let name = name.as_ref();
            if !valid_name(name) {
                return Err(TagDictionaryError::InvalidName);
            }
            let folded = name.to_ascii_lowercase();
            if dictionary.indices.contains_key(&folded) {
                return Err(TagDictionaryError::DuplicateName);
            }
            dictionary
                .indices
                .insert(folded, dictionary.names.len() as u16);
            dictionary.names.push(name.to_owned());
        }
        if u32::from(dictionary.invalid_index()) >= 1_u32 << total_bits {
            return Err(TagDictionaryError::InsufficientIndexWidth);
        }
        Ok(dictionary)
    }

    /// Expand dot-separated parent prefixes from declared tags, sort nodes
    /// ASCII-case-insensitively (`GetAllNodes` + `FName::LexicalLess`), and
    /// derive `total_bits` and clamped `first_segment` (`ConstructNetIndex`).
    pub fn from_tag_declarations(
        declarations: impl IntoIterator<Item = impl AsRef<str>>,
        first_segment: u8,
        container_bits: u8,
    ) -> Result<Self, TagDictionaryError> {
        if first_segment > 16 {
            return Err(TagDictionaryError::InvalidFirstSegment);
        }
        if !(1..=8).contains(&container_bits) {
            return Err(TagDictionaryError::InvalidContainerBits);
        }
        let mut seen = HashMap::<String, String>::new();
        for decl in declarations {
            let decl = decl.as_ref();
            if !valid_name(decl) {
                return Err(TagDictionaryError::InvalidName);
            }
            for (idx, _) in decl.match_indices('.') {
                let prefix = &decl[..idx];
                let folded = prefix.to_ascii_lowercase();
                seen.entry(folded).or_insert_with(|| prefix.to_owned());
            }
            let folded = decl.to_ascii_lowercase();
            seen.entry(folded).or_insert_with(|| decl.to_owned());
            if seen.len() > MAX_TAGS {
                return Err(TagDictionaryError::TooManyTags);
            }
        }
        let mut ordered: Vec<(String, String)> = seen.into_iter().collect();
        ordered.sort_by(|a, b| a.0.cmp(&b.0));
        let invalid_sentinel = (ordered.len() as u32) + 1;
        let total_bits = (invalid_sentinel + 1)
            .next_power_of_two()
            .trailing_zeros()
            .max(1) as u8;
        let clamped_first = first_segment.min(total_bits);
        Self::from_ordered_names(
            ordered.into_iter().map(|(_, original)| original),
            total_bits,
            clamped_first,
            container_bits,
        )
    }

    /// Compute the chained lowercase UTF-16 CRC-32 (`NetworkGameplayTagNodeIndexHash`).
    pub fn net_index_hash(&self) -> u32 {
        self.names
            .iter()
            .fold(0, |crc, name| super::checksum_ascii_lowercase(name, crc))
    }

    pub fn len(&self) -> usize {
        self.names.len()
    }

    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }

    /// Return only an assigned index; unknown and non-ASCII names return None.
    pub fn index_of(&self, name: &str) -> Option<u16> {
        if !name.is_ascii() || name.len() > MAX_TAG_NAME_BYTES {
            return None;
        }
        self.indices.get(&name.to_ascii_lowercase()).copied()
    }

    /// Return the original spelling for an assigned index only.
    pub fn name_of(&self, index: u16) -> Option<&str> {
        self.names.get(usize::from(index)).map(String::as_str)
    }

    pub fn unused_index(&self) -> u16 {
        self.names.len() as u16
    }

    pub fn invalid_index(&self) -> u16 {
        self.unused_index() + 1
    }

    pub fn classify(&self, index: u16) -> TagIndexKind {
        if index < self.unused_index() {
            TagIndexKind::Assigned
        } else if index == self.unused_index() {
            TagIndexKind::Unused
        } else if index == self.invalid_index() {
            TagIndexKind::Invalid
        } else {
            TagIndexKind::OutOfRange
        }
    }

    /// Pass directly to the experimental property-layout codec.
    pub fn encoding(&self) -> TagEncoding {
        TagEncoding::NetIndex {
            first_segment: self.first_segment,
            total_bits: self.total_bits,
        }
    }

    /// Explicit count width for `RepCommand::GameplayTagContainer`.
    pub fn container_bits(&self) -> u8 {
        self.container_bits
    }
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_TAG_NAME_BYTES
        && name.split('.').all(|segment| {
            !segment.is_empty() && segment.bytes().all(|byte| (b' '..=b'~').contains(&byte))
        })
}

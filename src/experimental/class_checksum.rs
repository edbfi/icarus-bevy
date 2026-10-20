//! Class network checksums, as exported with every object NetGUID (`F-776`).
//!
//! A NetGUID export carries `FNetGUIDCache::GetNetworkChecksum(Object)`: the
//! `ClassChecksum` of the object's class (or of the object itself when it is a
//! class), which the receiver compares with its own. `FClassNetCacheMgr`
//! computes it per class as:
//!
//! - start from the superclass's `ClassChecksum` (`0` at the root);
//! - take the class's own replicated properties (one per `ClassReps` entry
//!   group, stepping over `ArrayDim`), sort them by `Offset_Internal` and then
//!   case-insensitively by name, and fold [`property_checksum`] over them.
//!
//! [`property_checksum`] chains `FCrc::StrCrc32` (each UTF-16 unit as four
//! little-endian bytes, see [`crate::experimental::checksum_utf16`]) over the
//! lowercased name, the lowercased `GetCPPType(nullptr, 0)` spelling and the
//! decimal `ArrayDim`; an array then continues with its inner property, and a
//! struct adds its lowercased struct name and folds its fields (super-struct
//! fields included, `CPF_RepSkip` fields excluded) in the same sort order.
//!
//! Type spellings follow the engine, including the quirks the capture confirms:
//! containers spell only `TArray`/`TSet`/`TMap`, wrapped class properties
//! spell `TSubclassOf<AClass> ` with a trailing space, and delegates spell
//! `F` plus their signature name without `__DelegateSignature` (`F-777`).
//! Property schemas are caller data; nothing here reads original files.
use std::cmp::Ordering;

use super::checksum_utf16;

/// `FCrc::StrCrc32` of `text` chained from `seed`.
pub fn str_crc32(text: &str, seed: u32) -> u32 {
    let units: Vec<u16> = text.encode_utf16().collect();
    checksum_utf16(&units, seed)
}

/// What a property adds after its own name, type and dimension.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Children {
    None,
    /// `FArrayProperty::Inner`, hashed next with the same rules.
    ArrayInner(Box<ChecksumProperty>),
    /// A struct: its object name (without the `F` prefix) and its fields.
    Struct {
        name: String,
        fields: Vec<ChecksumProperty>,
    },
}

/// One property as `FClassNetCacheMgr::GetPropertyChecksum` sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChecksumProperty {
    pub name: String,
    /// The engine's `GetCPPType(nullptr, 0)` spelling, e.g. `TEnumAsByte<ENetRole>`.
    pub cpp_type: String,
    pub array_dim: u32,
    /// `Offset_Internal`; with the name, the sort key among siblings.
    pub offset: i32,
    /// `CPF_RepSkip`: excluded when iterating struct fields.
    pub rep_skip: bool,
    pub children: Children,
}

impl ChecksumProperty {
    pub fn leaf(name: &str, cpp_type: &str, offset: i32) -> Self {
        Self {
            name: name.to_owned(),
            cpp_type: cpp_type.to_owned(),
            array_dim: 1,
            offset,
            rep_skip: false,
            children: Children::None,
        }
    }

    pub fn array(name: &str, offset: i32, inner: ChecksumProperty) -> Self {
        Self {
            children: Children::ArrayInner(Box::new(inner)),
            ..Self::leaf(name, "TArray", offset)
        }
    }

    /// A struct property; `struct_name` is the object name (`RepMovement`),
    /// and the type spelling is `F` + that name.
    pub fn structure(
        name: &str,
        struct_name: &str,
        offset: i32,
        fields: Vec<ChecksumProperty>,
    ) -> Self {
        Self {
            children: Children::Struct {
                name: struct_name.to_owned(),
                fields,
            },
            ..Self::leaf(name, &format!("F{struct_name}"), offset)
        }
    }
}

/// Sibling order: `Offset_Internal`, then case-insensitive name (`FString` `<`).
pub fn sort_properties(properties: &mut [&ChecksumProperty]) {
    properties.sort_by(|a, b| match a.offset.cmp(&b.offset) {
        Ordering::Equal => a
            .name
            .to_ascii_lowercase()
            .cmp(&b.name.to_ascii_lowercase()),
        other => other,
    });
}

/// `GetPropertyChecksum(Property, seed, bIncludeChildren = true)`.
pub fn property_checksum(property: &ChecksumProperty, seed: u32) -> u32 {
    let mut checksum = seed;
    let mut current = property;
    loop {
        checksum = str_crc32(&current.name.to_ascii_lowercase(), checksum);
        checksum = str_crc32(&current.cpp_type.to_ascii_lowercase(), checksum);
        checksum = str_crc32(&current.array_dim.to_string(), checksum);
        match &current.children {
            Children::None => return checksum,
            Children::ArrayInner(inner) => current = inner,
            Children::Struct { name, fields } => {
                checksum = str_crc32(&name.to_ascii_lowercase(), checksum);
                let mut kept: Vec<&ChecksumProperty> =
                    fields.iter().filter(|field| !field.rep_skip).collect();
                sort_properties(&mut kept);
                for field in kept {
                    checksum = property_checksum(field, checksum);
                }
                return checksum;
            }
        }
    }
}

/// A class's `ClassChecksum` from its superclass's and its own replicated properties.
pub fn class_checksum(super_checksum: u32, own_replicated: &[ChecksumProperty]) -> u32 {
    let mut sorted: Vec<&ChecksumProperty> = own_replicated.iter().collect();
    sort_properties(&mut sorted);
    sorted
        .into_iter()
        .fold(super_checksum, |checksum, property| {
            property_checksum(property, checksum)
        })
}

/// Engine type spellings that differ from the obvious C++ form.
pub mod cpp {
    /// `FClassProperty` with `CPF_UObjectWrapper`: note the trailing space.
    pub fn subclass_of(prefixed_class: &str) -> String {
        format!("TSubclassOf<{prefixed_class}> ")
    }

    /// `FByteProperty` with a namespaced or regular enum.
    pub fn enum_as_byte(enum_cpp_type: &str) -> String {
        format!("TEnumAsByte<{enum_cpp_type}>")
    }

    /// Delegate properties: `F` + signature name without `__DelegateSignature`.
    pub fn delegate(signature_function: &str) -> String {
        let stem = signature_function
            .strip_suffix("__DelegateSignature")
            .unwrap_or(signature_function);
        format!("F{stem}")
    }

    /// Object pointers: `prefixed_class` already carries its `A` (actor) or
    /// `U` prefix; this only appends `*`.
    pub fn object(prefixed_class: &str) -> String {
        format!("{prefixed_class}*")
    }
}

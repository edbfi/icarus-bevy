//! Candidate-borrowing Bool/I32 restore preparation, independent of use gates.
//!
//! F-735/F-897 establish owner-class SaveGame-property lookup followed by
//! RepNotify; F-728 establishes array coverage, not arbitrary native behavior.
//! F-920 supplies the immutable candidate. Declarations here are user-authored
//! exact identities, never inferred from order, widths, or packaged content.
//! No callback handler is qualified. This module produces assessment only:
//! it cannot activate, modify a world, execute callbacks, or publish a snapshot.
use super::{exact, key_valid};
use crate::{
    experimental::{
        archive::{ArchiveStringFrame, ByteOrder, StringEncoding, read_string_frame},
        tagged::{MAX_HEADER_STRING_UNITS, TagKind},
        typed_value::{Scalar, TypedComponent, TypedProperty, TypedSummary, TypedTree, Value},
    },
    import_candidate::{CandidateLimits, DecodedImportCandidate, RecorderTyping},
    native_world::{
        CompiledNativeCatalog, MAX_NATIVE_PLACEMENTS, MAX_NATIVE_STRING_UNITS,
        MAX_NATIVE_VARIABLE_BYTES, MAX_NATIVE_VARIABLE_VALUES, MAX_NATIVE_VARIABLES_PER_CLASS,
        NativeTransform, NativeValue,
    },
};
use serde::{Serialize, Serializer};
use std::{fmt, mem::size_of};

const C001: &str = "/Script/Icarus.ActorStateRecorderComponent";
const NONE: &[u8] = b"\x05\0\0\0None\0";
const OWNER_FIELDS: [&str; 9] = [
    "ActorClassName",
    "ActorPathName",
    "ObjectFName",
    "IcarusActorGUID",
    "RecordedVersion",
    "bWasMovedToSubLevel",
    "ActorTransform",
    "bSkipDefaultOwnerResolution",
    "OwnerResolvePolicy",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum C001VariableKind {
    Bool,
    I32,
}

/// Field names are independent: neither VariableName nor the value-field name
/// is hardcoded. Even an empty array must carry this exact shared header.
#[derive(Clone, Copy)]
pub struct C001VariableArrayLayoutV1<'a> {
    pub array_name: &'a [u16],
    pub element_tag_name: &'a [u16],
    pub record_type: &'a [u16],
    pub property_name_field: &'a [u16],
    pub value_field: &'a [u16],
}

#[derive(Clone, Copy)]
pub struct C001VariableLayoutV1<'a> {
    pub bools: C001VariableArrayLayoutV1<'a>,
    pub i32s: C001VariableArrayLayoutV1<'a>,
}

/// Exact units, including unpaired surrogates, are compared without normalizing.
#[derive(Clone, Copy)]
pub struct C001VariableOwnerIdentityV1<'a> {
    pub actor_class: &'a [u16],
    pub actor_path: &'a [u16],
    pub object_name: &'a [u16],
    pub guid: i32,
}

#[derive(Clone, Copy)]
pub struct C001VariablePropertyBindingV1<'a> {
    pub source_property: &'a [u16],
    pub native_variable: &'a str,
    pub kind: C001VariableKind,
    /// Authored schema assertion, not discovery or proof of official semantics.
    pub save_game: bool,
}

#[derive(Clone, Copy)]
pub struct C001VariableOwnerBindingV1<'a> {
    pub source_index: usize,
    pub source: C001VariableOwnerIdentityV1<'a>,
    pub native_placement: &'a str,
    pub native_class: &'a str,
    /// Exact set of saved properties; defaults are not silently substituted.
    pub properties: &'a [C001VariablePropertyBindingV1<'a>],
}

#[derive(Clone, Copy)]
pub struct C001VariableBindingsV1<'a> {
    pub expected_payload_sha256: [u8; 32],
    pub native_catalog_id: &'a str,
    pub source_map: &'a [u16],
    pub native_map: &'a str,
    pub owners: &'a [C001VariableOwnerBindingV1<'a>],
}

/// Additional allocations, including temporary lookup capacity, must fit BOTH
/// these allowances and the candidate's remaining configured allowance. No
/// candidate limit is raised. These charges are not a peak-memory/RSS claim.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct C001VariableLimits {
    pub owned_bytes: usize,
    pub assignments: usize,
}
impl Default for C001VariableLimits {
    fn default() -> Self {
        Self {
            owned_bytes: MAX_NATIVE_VARIABLE_BYTES,
            assignments: MAX_NATIVE_VARIABLE_VALUES,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum C001VariableErrorKind {
    Limit,
    Header,
    Layout,
    Binding,
    Type,
    Unsupported,
    DroppedRecorder,
    UnavailableTyping,
    WrongRecorderKind,
}

/// Errors contain only a source ordinal and fixed category. Parser errors and
/// source strings/values never cross this diagnostic boundary.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct C001VariableError {
    pub source_index: Option<usize>,
    pub kind: C001VariableErrorKind,
}
impl fmt::Display for C001VariableError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "C001 variable preparation {:?}, recorder {:?}",
            self.kind, self.source_index
        )
    }
}
impl std::error::Error for C001VariableError {}
type Result<T> = std::result::Result<T, C001VariableError>;
fn check(ok: bool, at: Option<usize>, kind: C001VariableErrorKind) -> Result<()> {
    if ok {
        Ok(())
    } else {
        Err(C001VariableError {
            source_index: at,
            kind,
        })
    }
}

/// Deliberately no Qualified/Ignore/NoOp variant: a caller cannot waive the
/// per-property obligation by choosing an authored use-gate callback.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum C001VariableCallbackBlocker {
    UnqualifiedPropertyRestoreAndRepNotify,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum C001VariableValue {
    Bool(bool),
    I32(i32),
}

pub struct PreparedC001VariableAssignment<'a> {
    source_index: usize,
    source_array: &'a [u16],
    source_element: usize,
    source_property: &'a [u16],
    source_name_header: &'a [u8],
    source_value_header: &'a [u8],
    value: C001VariableValue,
    binding: C001VariablePropertyBindingV1<'a>,
    owner: C001VariableOwnerBindingV1<'a>,
}
impl<'a> PreparedC001VariableAssignment<'a> {
    pub fn source_index(&self) -> usize {
        self.source_index
    }
    pub fn source_array(&self) -> &'a [u16] {
        self.source_array
    }
    pub fn source_element(&self) -> usize {
        self.source_element
    }
    pub fn source_property(&self) -> &'a [u16] {
        self.source_property
    }
    pub fn source_name_header(&self) -> &'a [u8] {
        self.source_name_header
    }
    pub fn source_value_header(&self) -> &'a [u8] {
        self.source_value_header
    }
    pub fn value(&self) -> C001VariableValue {
        self.value
    }
    pub fn binding(&self) -> C001VariablePropertyBindingV1<'a> {
        self.binding
    }
    pub fn owner(&self) -> C001VariableOwnerBindingV1<'a> {
        self.owner
    }
    pub fn callback_blocker(&self) -> C001VariableCallbackBlocker {
        C001VariableCallbackBlocker::UnqualifiedPropertyRestoreAndRepNotify
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct C001VariableInspection {
    pub recorders: usize,
    pub assignments: usize,
    pub bool_assignments: usize,
    pub i32_assignments: usize,
    pub unresolved_callbacks: usize,
    /// Charged output and temporary lookup capacity during preparation.
    pub additional_owned_bytes: usize,
    pub world_imported: bool,
    pub activation_ready: bool,
}

pub struct PreparedC001Variables<'a> {
    candidate: &'a DecodedImportCandidate,
    catalog: &'a CompiledNativeCatalog,
    layout: C001VariableLayoutV1<'a>,
    bindings: C001VariableBindingsV1<'a>,
    assignments: Vec<PreparedC001VariableAssignment<'a>>,
    inspection: C001VariableInspection,
}
impl<'a> PreparedC001Variables<'a> {
    pub fn candidate(&self) -> &'a DecodedImportCandidate {
        self.candidate
    }
    pub fn catalog(&self) -> &'a CompiledNativeCatalog {
        self.catalog
    }
    pub fn layout(&self) -> C001VariableLayoutV1<'a> {
        self.layout
    }
    pub fn bindings(&self) -> C001VariableBindingsV1<'a> {
        self.bindings
    }
    pub fn assignments(&self) -> &[PreparedC001VariableAssignment<'a>] {
        &self.assignments
    }
    pub fn inspection(&self) -> C001VariableInspection {
        self.inspection
    }
    /// Assessment remains closed even with zero saved variables: owner lifecycle,
    /// other state, coordinates and ordinary import are outside this contract.
    pub fn activation_ready(&self) -> bool {
        false
    }
}
impl fmt::Debug for PreparedC001Variables<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.inspection.fmt(f)
    }
}
impl Serialize for PreparedC001Variables<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        self.inspection.serialize(s)
    }
}
macro_rules! redact {
    ($($name:ty),* $(,)?) => { $(
        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("[redacted C001 variables]") }
        }
        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
                s.serialize_str("[redacted C001 variables]")
            }
        }
    )* };
}
redact!(
    C001VariableArrayLayoutV1<'_>,
    C001VariableLayoutV1<'_>,
    C001VariableOwnerIdentityV1<'_>,
    C001VariablePropertyBindingV1<'_>,
    C001VariableOwnerBindingV1<'_>,
    C001VariableBindingsV1<'_>,
    C001VariableValue,
    PreparedC001VariableAssignment<'_>
);

fn units_ok(units: &[u16], max: usize) -> bool {
    !units.is_empty() && units.len() <= max && units.iter().all(|u| *u != 0 && *u != u16::MAX)
}
fn canonical_frame(bytes: &[u8], max: usize, at: Option<usize>) -> Result<ArchiveStringFrame<'_>> {
    let frame = read_string_frame(bytes, ByteOrder::Little).map_err(|_| C001VariableError {
        source_index: at,
        kind: C001VariableErrorKind::Header,
    })?;
    check(
        frame.unit_count() <= max
            && match frame.encoding() {
                StringEncoding::Empty => true,
                StringEncoding::Narrow => frame.unit_count() > 0,
                StringEncoding::Wide => frame.units().any(|u| u > 127),
            },
        at,
        C001VariableErrorKind::Header,
    )?;
    Ok(frame)
}

// No string/vector allocation: inspect the retained exact headers by borrowing
// archive frames. Shared array headers are stored without their payload bytes.
struct Header<'a> {
    name: ArchiveStringFrame<'a>,
    kind: TagKind,
    extra: Option<ArchiveStringFrame<'a>>,
    boolean: Option<bool>,
    size: usize,
    consumed: usize,
}
fn header(bytes: &[u8], at: Option<usize>) -> Result<Header<'_>> {
    use C001VariableErrorKind::{Header as BadHeader, Unsupported};
    let fail = || C001VariableError {
        source_index: at,
        kind: BadHeader,
    };
    let name = canonical_frame(bytes, MAX_HEADER_STRING_UNITS - 1, at)?;
    check(name.unit_count() > 0, at, BadHeader)?;
    let mut pos = name.consumed();
    let ty = canonical_frame(
        bytes.get(pos..).ok_or_else(fail)?,
        MAX_HEADER_STRING_UNITS - 1,
        at,
    )?;
    pos += ty.consumed();
    let kind = [
        ("BoolProperty", TagKind::Bool),
        ("IntProperty", TagKind::Int),
        ("NameProperty", TagKind::Name),
        ("StrProperty", TagKind::String),
        ("StructProperty", TagKind::Struct),
        ("ArrayProperty", TagKind::Array),
        ("ByteProperty", TagKind::Byte),
        ("EnumProperty", TagKind::Enum),
    ]
    .into_iter()
    .find_map(|(name, kind)| ty.units().eq(name.encode_utf16()).then_some(kind))
    .ok_or(C001VariableError {
        source_index: at,
        kind: Unsupported,
    })?;
    let ints = bytes.get(pos..pos + 8).ok_or_else(fail)?;
    let size = usize::try_from(i32::from_le_bytes(
        ints[..4].try_into().map_err(|_| fail())?,
    ))
    .map_err(|_| fail())?;
    check(ints[4..] == [0; 4], at, BadHeader)?;
    pos += 8;
    let mut extra = None;
    let mut boolean = None;
    match kind {
        TagKind::Struct | TagKind::Array | TagKind::Byte | TagKind::Enum => {
            let frame = canonical_frame(
                bytes.get(pos..).ok_or_else(fail)?,
                MAX_HEADER_STRING_UNITS - 1,
                at,
            )?;
            pos += frame.consumed();
            extra = Some(frame);
            if kind == TagKind::Struct {
                check(
                    bytes.get(pos..pos + 16) == Some(&[0; 16][..]),
                    at,
                    BadHeader,
                )?;
                pos += 16;
            }
        }
        TagKind::Bool => {
            boolean = match bytes.get(pos) {
                Some(0) => Some(false),
                Some(1) => Some(true),
                _ => return Err(fail()),
            };
            pos += 1;
            check(size == 0, at, BadHeader)?;
        }
        _ => {}
    }
    check(bytes.get(pos) == Some(&0), at, BadHeader)?;
    pos += 1;
    Ok(Header {
        name,
        kind,
        extra,
        boolean,
        size,
        consumed: pos,
    })
}
fn property_header<'a>(p: &'a TypedProperty, at: Option<usize>) -> Result<Header<'a>> {
    let h = header(&p.header, at)?;
    check(
        h.consumed == p.header.len()
            && h.kind == p.kind
            && h.name.units().eq(p.name.iter().copied()),
        at,
        C001VariableErrorKind::Header,
    )?;
    Ok(h)
}
fn extra_eq(h: &Header<'_>, units: &[u16]) -> bool {
    h.extra
        .as_ref()
        .is_some_and(|f| f.units().eq(units.iter().copied()))
}
fn extra_is(h: &Header<'_>, name: &str) -> bool {
    h.extra
        .as_ref()
        .is_some_and(|f| f.units().eq(name.encode_utf16()))
}
fn unique_tree(tree: &TypedTree, max: usize, at: Option<usize>) -> Result<()> {
    check(
        tree.properties.len() <= max,
        at,
        C001VariableErrorKind::Limit,
    )?;
    check(tree.terminator == NONE, at, C001VariableErrorKind::Header)?;
    for (i, p) in tree.properties.iter().enumerate() {
        property_header(p, at)?;
        check(
            !tree.properties[..i].iter().any(|q| q.name == p.name),
            at,
            C001VariableErrorKind::Header,
        )?;
    }
    Ok(())
}
fn source_name(p: &TypedProperty, kind: TagKind, at: Option<usize>) -> Result<&[u16]> {
    check(p.kind == kind, at, C001VariableErrorKind::Type)?;
    let units = match &p.value {
        Value::Scalar(Scalar::Name(u)) if kind == TagKind::Name => u,
        Value::Scalar(Scalar::Str(u)) if kind == TagKind::String => u,
        _ => {
            return Err(C001VariableError {
                source_index: at,
                kind: C001VariableErrorKind::Type,
            });
        }
    };
    check(
        units_ok(units, MAX_NATIVE_STRING_UNITS),
        at,
        C001VariableErrorKind::Limit,
    )?;
    Ok(units)
}
fn int(p: &TypedProperty, at: Option<usize>) -> Result<i32> {
    match &p.value {
        Value::Scalar(Scalar::Int(v)) if p.kind == TagKind::Int => Ok(*v),
        _ => Err(C001VariableError {
            source_index: at,
            kind: C001VariableErrorKind::Type,
        }),
    }
}
fn boolean(p: &TypedProperty, at: Option<usize>) -> Result<bool> {
    match &p.value {
        Value::Scalar(Scalar::Bool(v)) if p.kind == TagKind::Bool => {
            check(
                property_header(p, at)?.boolean == Some(*v),
                at,
                C001VariableErrorKind::Header,
            )?;
            Ok(*v)
        }
        _ => Err(C001VariableError {
            source_index: at,
            kind: C001VariableErrorKind::Type,
        }),
    }
}

fn validate_layout(layout: C001VariableLayoutV1<'_>) -> Result<()> {
    use C001VariableErrorKind::Layout;
    for a in [layout.bools, layout.i32s] {
        for units in [
            a.array_name,
            a.element_tag_name,
            a.record_type,
            a.property_name_field,
            a.value_field,
        ] {
            check(
                units_ok(units, MAX_HEADER_STRING_UNITS - 1) && !exact(units, "None"),
                None,
                Layout,
            )?;
        }
        check(
            a.property_name_field != a.value_field
                && !OWNER_FIELDS.iter().any(|n| exact(a.array_name, n)),
            None,
            Layout,
        )?;
    }
    check(
        layout.bools.array_name != layout.i32s.array_name,
        None,
        Layout,
    )
}

fn wrapper(candidate: &DecodedImportCandidate, index: usize) -> Result<()> {
    let at = Some(index);
    let fail = || C001VariableError {
        source_index: at,
        kind: C001VariableErrorKind::Header,
    };
    let mut bytes = candidate.recorder_bytes(index).ok_or_else(fail)?;
    let (mut class, mut binary) = (false, false);
    for _ in 0..2 {
        let h = header(bytes, at)?;
        let end = h.consumed.checked_add(h.size).ok_or_else(fail)?;
        let payload = bytes.get(h.consumed..end).ok_or_else(fail)?;
        if h.name.units().eq("ComponentClassName".encode_utf16()) && !class {
            check(h.kind == TagKind::String, at, C001VariableErrorKind::Type)?;
            let f = canonical_frame(payload, MAX_NATIVE_STRING_UNITS, at)?;
            check(
                f.consumed() == payload.len() && f.units().eq(C001.encode_utf16()),
                at,
                C001VariableErrorKind::WrongRecorderKind,
            )?;
            class = true;
        } else if h.name.units().eq("BinaryData".encode_utf16()) && !binary {
            check(
                h.kind == TagKind::Array && extra_is(&h, "ByteProperty"),
                at,
                C001VariableErrorKind::Type,
            )?;
            let count = payload.get(..4).ok_or_else(fail)?;
            let count = usize::try_from(i32::from_le_bytes(count.try_into().map_err(|_| fail())?))
                .map_err(|_| fail())?;
            check(
                count == payload.len() - 4
                    && candidate.recorder_binary_data(index) == payload.get(4..),
                at,
                C001VariableErrorKind::Header,
            )?;
            binary = true;
        } else {
            return Err(fail());
        }
        bytes = bytes.get(end..).ok_or_else(fail)?;
    }
    check(
        class && binary && bytes == NONE,
        at,
        C001VariableErrorKind::Header,
    )
}

fn transform(p: &TypedProperty, at: Option<usize>) -> Result<()> {
    use C001VariableErrorKind::{Type, Unsupported};
    let Value::TaggedStruct { type_name, tree } = &p.value else {
        return Err(C001VariableError {
            source_index: at,
            kind: Type,
        });
    };
    check(
        p.kind == TagKind::Struct
            && exact(type_name, "Transform")
            && extra_is(&property_header(p, at)?, "Transform"),
        at,
        Type,
    )?;
    unique_tree(tree, 3, at)?;
    check(tree.properties.len() == 3, at, Unsupported)?;
    let mut pose = NativeTransform::default();
    for child in &tree.properties {
        let Value::FixedStruct { type_name, value } = &child.value else {
            return Err(C001VariableError {
                source_index: at,
                kind: Type,
            });
        };
        let ty = if exact(&child.name, "Rotation") {
            "Quat"
        } else {
            "Vector"
        };
        check(
            child.kind == TagKind::Struct
                && exact(type_name, ty)
                && extra_is(&property_header(child, at)?, ty),
            at,
            Type,
        )?;
        if exact(&child.name, "Rotation") {
            pose.rotation = value.floats().ok_or(C001VariableError {
                source_index: at,
                kind: Type,
            })?;
        } else if exact(&child.name, "Translation") {
            pose.translation = value.floats().ok_or(C001VariableError {
                source_index: at,
                kind: Type,
            })?;
        } else if exact(&child.name, "Scale3D") {
            pose.scale = value.floats().ok_or(C001VariableError {
                source_index: at,
                kind: Type,
            })?;
        } else {
            return Err(C001VariableError {
                source_index: at,
                kind: Unsupported,
            });
        }
    }
    check(pose.validate().is_ok(), at, Unsupported)
}

fn owner<'a>(
    component: &'a TypedComponent,
    layout: C001VariableLayoutV1<'_>,
    index: usize,
) -> Result<C001VariableOwnerIdentityV1<'a>> {
    use C001VariableErrorKind::{Binding, Unsupported};
    let at = Some(index);
    check(component.trailer == [0; 4], at, Unsupported)?;
    unique_tree(&component.tree, OWNER_FIELDS.len() + 2, at)?;
    let find = |name| {
        component
            .tree
            .properties
            .iter()
            .find(|p| exact(&p.name, name))
            .ok_or(C001VariableError {
                source_index: at,
                kind: Unsupported,
            })
    };
    let result = C001VariableOwnerIdentityV1 {
        actor_class: source_name(find("ActorClassName")?, TagKind::Name, at)?,
        actor_path: source_name(find("ActorPathName")?, TagKind::String, at)?,
        object_name: source_name(find("ObjectFName")?, TagKind::Name, at)?,
        guid: int(find("IcarusActorGUID")?, at)?,
    };
    check(result.guid > 0, at, Binding)?;
    check(
        int(find("RecordedVersion")?, at)? == 3 && !boolean(find("bWasMovedToSubLevel")?, at)?,
        at,
        Unsupported,
    )?;
    transform(find("ActorTransform")?, at)?;
    for p in &component.tree.properties {
        if exact(&p.name, "bSkipDefaultOwnerResolution") {
            check(!boolean(p, at)?, at, Unsupported)?;
        } else if exact(&p.name, "OwnerResolvePolicy") {
            let h = property_header(p, at)?;
            let valid = match &p.value {
                Value::Scalar(Scalar::Byte(v)) => {
                    p.kind == TagKind::Byte && extra_is(&h, "None") && *v <= 2
                }
                Value::Scalar(Scalar::Enum(u)) => {
                    extra_is(&h, "EStateRecorderOwnerResolvePolicy")
                        && [
                            "FindOnly",
                            "RespawnOnly",
                            "FindOrRespawn",
                            "EStateRecorderOwnerResolvePolicy::FindOnly",
                            "EStateRecorderOwnerResolvePolicy::RespawnOnly",
                            "EStateRecorderOwnerResolvePolicy::FindOrRespawn",
                        ]
                        .iter()
                        .any(|n| exact(u, n))
                }
                _ => false,
            };
            check(valid, at, Unsupported)?;
        } else {
            check(
                OWNER_FIELDS.iter().any(|n| exact(&p.name, n))
                    || p.name == layout.bools.array_name
                    || p.name == layout.i32s.array_name,
                at,
                Unsupported,
            )?;
        }
    }
    Ok(result)
}

fn same_owner(a: C001VariableOwnerIdentityV1<'_>, b: C001VariableOwnerIdentityV1<'_>) -> bool {
    a.actor_class == b.actor_class
        && a.actor_path == b.actor_path
        && a.object_name == b.object_name
        && a.guid == b.guid
}

fn lossless(typing: &RecorderTyping, index: usize) -> Result<(&TypedComponent, &TypedSummary)> {
    typing.lossless().ok_or(C001VariableError {
        source_index: Some(index),
        kind: C001VariableErrorKind::UnavailableTyping,
    })
}

struct SourceVariable<'a> {
    name: &'a [u16],
    value: C001VariableValue,
    name_header: &'a [u8],
    value_header: &'a [u8],
}
fn variable<'a>(
    tree: &'a TypedTree,
    shape: C001VariableArrayLayoutV1<'_>,
    kind: C001VariableKind,
    at: Option<usize>,
) -> Result<SourceVariable<'a>> {
    use C001VariableErrorKind::Unsupported;
    unique_tree(tree, 2, at)?;
    check(tree.properties.len() == 2, at, Unsupported)?;
    let name = tree
        .properties
        .iter()
        .find(|p| p.name == shape.property_name_field)
        .ok_or(C001VariableError {
            source_index: at,
            kind: Unsupported,
        })?;
    let value = tree
        .properties
        .iter()
        .find(|p| p.name == shape.value_field)
        .ok_or(C001VariableError {
            source_index: at,
            kind: Unsupported,
        })?;
    Ok(SourceVariable {
        name: source_name(name, TagKind::Name, at)?,
        value: match kind {
            C001VariableKind::Bool => C001VariableValue::Bool(boolean(value, at)?),
            C001VariableKind::I32 => C001VariableValue::I32(int(value, at)?),
        },
        name_header: &name.header,
        value_header: &value.header,
    })
}
fn array<'a>(
    component: &'a TypedComponent,
    shape: C001VariableArrayLayoutV1<'_>,
    at: Option<usize>,
) -> Result<&'a [TypedTree]> {
    use C001VariableErrorKind::{Limit, Type};
    let p = component
        .tree
        .properties
        .iter()
        .find(|p| p.name == shape.array_name)
        .ok_or(C001VariableError {
            source_index: at,
            kind: Type,
        })?;
    check(
        p.kind == TagKind::Array && extra_is(&property_header(p, at)?, "StructProperty"),
        at,
        Type,
    )?;
    let Value::StructArray {
        shared_header,
        type_name,
        elements,
    } = &p.value
    else {
        return Err(C001VariableError {
            source_index: at,
            kind: Type,
        });
    };
    check(elements.len() <= MAX_NATIVE_VARIABLES_PER_CLASS, at, Limit)?;
    let shared = header(shared_header, at)?;
    check(
        shared.consumed == shared_header.len()
            && shared.kind == TagKind::Struct
            && shared
                .name
                .units()
                .eq(shape.element_tag_name.iter().copied())
            && type_name == shape.record_type
            && extra_eq(&shared, shape.record_type),
        at,
        Type,
    )?;
    Ok(elements)
}

fn preparation_bytes(assignments: usize, owners: usize) -> Result<usize> {
    assignments
        .checked_mul(size_of::<PreparedC001VariableAssignment<'_>>())
        .and_then(|n| {
            owners
                .checked_mul(size_of::<&C001VariableOwnerBindingV1<'_>>())
                .and_then(|lookup| n.checked_add(lookup))
        })
        .and_then(|n| n.checked_add(size_of::<PreparedC001Variables<'_>>()))
        .and_then(|n| n.checked_add(size_of::<Vec<&C001VariableOwnerBindingV1<'_>>>()))
        .ok_or(C001VariableError {
            source_index: None,
            kind: C001VariableErrorKind::Limit,
        })
}

fn unique_owner_keys<'a, K: Ord>(
    owners: &mut [&C001VariableOwnerBindingV1<'a>],
    key: impl Fn(&C001VariableOwnerBindingV1<'a>) -> K,
) -> Result<()> {
    // In-place unstable sorting needs no additional allocation. Break ties by
    // source ordinal so a duplicate identifies the later matching recorder.
    owners.sort_unstable_by(|a, b| {
        key(a)
            .cmp(&key(b))
            .then_with(|| a.source_index.cmp(&b.source_index))
    });
    for pair in owners.windows(2) {
        check(
            key(pair[0]) != key(pair[1]),
            Some(pair[1].source_index),
            C001VariableErrorKind::Binding,
        )?;
    }
    Ok(())
}

/// Validate the entire candidate and all declarations before allocating output.
/// One bounded, byte-charged owner index is reused for lookup and uniqueness;
/// all remaining quadratic checks are confined to at most 64 properties/owner.
/// Dropped, unavailable and non-C001 records are terminal errors, including at
/// the last ordinal. No filtering, partial plan, retyping, string collection,
/// envelope decode, filesystem access, callback execution or world mutation.
///
/// Unrelated root state is preserved by borrowing the candidate, not interpreted
/// or approved. Native placement/class/type checks are catalog checks, not owner
/// lifecycle, coordinate or official SaveGame schema qualification.
pub fn prepare_c001_variables<'a>(
    candidate: &'a DecodedImportCandidate,
    catalog: &'a CompiledNativeCatalog,
    layout: C001VariableLayoutV1<'a>,
    bindings: C001VariableBindingsV1<'a>,
    limits: C001VariableLimits,
) -> Result<PreparedC001Variables<'a>> {
    use C001VariableErrorKind::{Binding, Limit, Unsupported};
    validate_layout(layout)?;
    check(
        limits.assignments <= MAX_NATIVE_VARIABLE_VALUES
            && limits.owned_bytes <= MAX_NATIVE_VARIABLE_BYTES,
        None,
        Limit,
    )?;
    check(
        candidate.recorders().len() <= MAX_NATIVE_PLACEMENTS
            && bindings.owners.len() == candidate.recorders().len(),
        None,
        Limit,
    )?;
    check(
        bindings.expected_payload_sha256 == candidate.preserved().payload_sha256()
            && bindings.native_catalog_id == catalog.identity()
            && bindings.source_map == candidate.root_values().prospect_map_name(),
        None,
        Binding,
    )?;
    check(key_valid(bindings.native_map), None, Binding)?;
    let map = catalog.map(bindings.native_map).ok_or(C001VariableError {
        source_index: None,
        kind: Binding,
    })?;
    let CandidateLimits {
        owned_bytes,
        typed_elements,
    } = candidate.inspection().allocation.remaining_headroom;
    let check_bytes = |bytes| {
        check(
            bytes <= limits.owned_bytes && bytes <= owned_bytes,
            None,
            Limit,
        )
    };
    check_bytes(preparation_bytes(0, bindings.owners.len())?)?;
    let mut owners: Vec<&C001VariableOwnerBindingV1<'a>> = Vec::new();
    owners
        .try_reserve_exact(bindings.owners.len())
        .map_err(|_| C001VariableError {
            source_index: None,
            kind: Limit,
        })?;
    check_bytes(preparation_bytes(0, owners.capacity())?)?;
    owners.extend(bindings.owners.iter());
    owners.sort_unstable_by_key(|b| b.source_index);
    for (index, binding) in owners.iter().enumerate() {
        // Equal lengths plus this dense ordinal check reject both missing and
        // duplicate source indices without scanning the declarations per record.
        check(binding.source_index == index, Some(index), Binding)?;
    }
    // Two passes: validation/counting first; exact-capacity output second.
    let (mut total, mut bools) = (0usize, 0usize);
    for (index, recorder) in candidate.recorders().iter().enumerate() {
        let at = Some(index);
        check(
            recorder.source_index() == index && exact(recorder.class_units(), C001),
            at,
            C001VariableErrorKind::WrongRecorderKind,
        )?;
        check(
            recorder.decision().retained,
            at,
            C001VariableErrorKind::DroppedRecorder,
        )?;
        let (component, summary) = lossless(recorder.typing(), index)?;
        check(summary.uninterpreted == 0, at, Unsupported)?;
        let d = recorder.decision();
        check(
            !d.reset_rocket_spawn
                && !d.physics_dynamic_tree
                && !d.skip_default_owner_resolution
                && !d.requires_stitching,
            at,
            Unsupported,
        )?;
        wrapper(candidate, index)?;
        let identity = owner(component, layout, index)?;
        let binding = owners[index];
        check(
            binding.properties.len() <= MAX_NATIVE_VARIABLES_PER_CLASS,
            at,
            Limit,
        )?;
        check(
            same_owner(identity, binding.source)
                && key_valid(binding.native_class)
                && key_valid(binding.native_placement),
            at,
            Binding,
        )?;
        let class = catalog
            .class(binding.native_class)
            .ok_or(C001VariableError {
                source_index: at,
                kind: Binding,
            })?;
        // CompiledNativeCatalog canonicalizes placements by exact key.
        let placement = map
            .placements
            .binary_search_by(|p| p.key.as_str().cmp(binding.native_placement))
            .ok()
            .map(|i| &map.placements[i])
            .ok_or(C001VariableError {
                source_index: at,
                kind: Binding,
            })?;
        check(placement.class == binding.native_class, at, Binding)?;
        for (i, b) in binding.properties.iter().enumerate() {
            check(
                units_ok(b.source_property, MAX_NATIVE_STRING_UNITS)
                    && key_valid(b.native_variable)
                    && b.save_game,
                at,
                Binding,
            )?;
            check(
                !binding.properties[..i].iter().any(|p| {
                    p.source_property == b.source_property || p.native_variable == b.native_variable
                }),
                at,
                Binding,
            )?;
            let native = class
                .variables
                .iter()
                .find(|v| v.key == b.native_variable)
                .ok_or(C001VariableError {
                    source_index: at,
                    kind: Binding,
                })?;
            check(
                matches!(
                    (&native.default, b.kind),
                    (NativeValue::Bool(_), C001VariableKind::Bool)
                        | (NativeValue::I32(_), C001VariableKind::I32)
                ),
                at,
                C001VariableErrorKind::Type,
            )?;
        }
        let arrays = [
            (layout.bools, C001VariableKind::Bool),
            (layout.i32s, C001VariableKind::I32),
        ];
        let mut count = 0usize;
        let mut seen_properties = [false; MAX_NATIVE_VARIABLES_PER_CLASS];
        for (shape, kind) in arrays {
            let elements = array(component, shape, at)?;
            count = count.checked_add(elements.len()).ok_or(C001VariableError {
                source_index: at,
                kind: Limit,
            })?;
            check(count <= MAX_NATIVE_VARIABLES_PER_CLASS, at, Limit)?;
            for tree in elements {
                let v = variable(tree, shape, kind, at)?;
                let property_index = binding
                    .properties
                    .iter()
                    .position(|b| b.source_property == v.name && b.kind == kind)
                    .ok_or(C001VariableError {
                        source_index: at,
                        kind: Binding,
                    })?;
                // Binding names are unique and their count fits this stack
                // array. Mark across both source arrays without reparsing any
                // previously validated record or allocating another lookup.
                check(!seen_properties[property_index], at, Binding)?;
                seen_properties[property_index] = true;
            }
            if kind == C001VariableKind::Bool {
                bools += elements.len();
            }
        }
        check(count == binding.properties.len(), at, Binding)?;
        total = total.checked_add(count).ok_or(C001VariableError {
            source_index: at,
            kind: Limit,
        })?;
        check(
            total <= limits.assignments && total <= typed_elements,
            at,
            Limit,
        )?;
        check_bytes(preparation_bytes(total, owners.capacity())?)?;
    }
    // Validated matching identities above bound string comparisons here. Source
    // paths/GUIDs and destination placements must each be one-to-one, including
    // owners with empty arrays. Reuse the same allocation for all three keys.
    unique_owner_keys(&mut owners, |b| b.source.actor_path)?;
    unique_owner_keys(&mut owners, |b| b.source.guid)?;
    unique_owner_keys(&mut owners, |b| b.native_placement)?;
    owners.sort_unstable_by_key(|b| b.source_index);
    let mut assignments = Vec::new();
    assignments
        .try_reserve_exact(total)
        .map_err(|_| C001VariableError {
            source_index: None,
            kind: Limit,
        })?;
    let bytes = preparation_bytes(assignments.capacity(), owners.capacity())?;
    check_bytes(bytes)?;
    for (index, recorder) in candidate.recorders().iter().enumerate() {
        let at = Some(index);
        let (component, _) = lossless(recorder.typing(), index)?;
        let owner = *owners[index];
        for (shape, kind) in [
            (layout.bools, C001VariableKind::Bool),
            (layout.i32s, C001VariableKind::I32),
        ] {
            for (source_element, tree) in array(component, shape, at)?.iter().enumerate() {
                let v = variable(tree, shape, kind, at)?;
                let binding = *owner
                    .properties
                    .iter()
                    .find(|b| b.source_property == v.name && b.kind == kind)
                    .ok_or(C001VariableError {
                        source_index: at,
                        kind: Binding,
                    })?;
                assignments.push(PreparedC001VariableAssignment {
                    source_index: index,
                    source_array: shape.array_name,
                    source_element,
                    source_property: v.name,
                    source_name_header: v.name_header,
                    source_value_header: v.value_header,
                    value: v.value,
                    binding,
                    owner,
                });
            }
        }
    }
    Ok(PreparedC001Variables {
        candidate,
        catalog,
        layout,
        bindings,
        assignments,
        inspection: C001VariableInspection {
            recorders: candidate.recorders().len(),
            assignments: total,
            bool_assignments: bools,
            i32_assignments: total - bools,
            unresolved_callbacks: total,
            additional_owned_bytes: bytes,
            world_imported: false,
            activation_ready: false,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::import_candidate::RecorderTypingFailure;

    #[test]
    fn every_unavailable_typing_outcome_is_a_terminal_redacted_error() {
        // The candidate deliberately has no public mutation/test constructor.
        // Exercise the exact admission helper used by both preparation passes;
        // do not fabricate an allegedly decoded candidate to reach this branch.
        for reason in [
            RecorderTypingFailure::Framing,
            RecorderTypingFailure::Typing,
            RecorderTypingFailure::Encoding,
            RecorderTypingFailure::RoundTripMismatch,
        ] {
            let err = lossless(&RecorderTyping::Unavailable(reason), 17)
                .err()
                .unwrap();
            assert_eq!(
                err,
                C001VariableError {
                    source_index: Some(17),
                    kind: C001VariableErrorKind::UnavailableTyping,
                }
            );
            assert_eq!(serde_json::to_value(err).unwrap()["source_index"], 17);
        }
    }
}

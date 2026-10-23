//! Shared strict C001 source admission; native binding remains adapter-specific.
use super::{PROFILE, SemanticImportCategory, SemanticResult, error, exact, require, units_valid};
use crate::{
    experimental::{
        archive::{ByteOrder, decode_string, encode_string},
        component::{MAX_BOUNDED_COMPONENT_BYTES, read_base_component_with_limit},
        recorders::{StagedRootMigration, StagedRootValues},
        restore_rules::{LobbyPrivacy, OwnerResolvePolicy},
        tagged::{ParsedRecord, PropertyRecord, TagKind, read_record, read_root},
    },
    native_world::NativeTransform,
};
use std::collections::BTreeSet;

#[derive(Clone, Copy)]
pub(crate) enum C001SourceProfile {
    InertPlacementV1,
    AuthoredOwnersV1,
    AuthoredScalarOwnersV1,
}
impl C001SourceProfile {
    pub(crate) fn admits(self, policy: OwnerResolvePolicy) -> bool {
        matches!(
            policy,
            OwnerResolvePolicy::FindOnly | OwnerResolvePolicy::FindOrRespawn
        ) || (matches!(self, Self::AuthoredOwnersV1 | Self::AuthoredScalarOwnersV1)
            && policy == OwnerResolvePolicy::RespawnOnly)
    }
}
impl SourceOwner {
    pub(crate) fn effective_policy(&self) -> OwnerResolvePolicy {
        self.explicit_policy
            .unwrap_or(OwnerResolvePolicy::FindOrRespawn)
    }
}

pub(crate) fn validate_root(root: &StagedRootValues) -> SemanticResult<()> {
    use SemanticImportCategory::{Missing, Unsupported, Value};
    let p = root.source_presence();
    require(
        p.version
            && root.loaded_version() == Some(2)
            && root.migration() == StagedRootMigration::CurrentVersion,
        None,
        "Version",
        Unsupported,
    )?;
    require(
        !p.legacy_prospect_id && !p.legacy_prospect_dt_key,
        None,
        "legacy_root",
        Unsupported,
    )?;
    require(
        p.prospect_info && p.prospect_map_name && p.state_recorder_blobs,
        None,
        "root_presence",
        Missing,
    )?;
    require(
        root.lobby_privacy() == LobbyPrivacy::Unknown,
        None,
        "LobbyPrivacy",
        Unsupported,
    )?;
    require(
        units_valid(root.prospect_map_name(), true),
        None,
        "ProspectMapName",
        Value,
    )?;
    require(
        root.resolved_prospect_info().is_some(),
        None,
        "ProspectInfo.enum",
        Unsupported,
    )?;
    let info = root.prospect_info();
    require(
        units_valid(&info.prospect_id, true) && units_valid(&info.lobby_name, false),
        None,
        "ProspectInfo.identity",
        Value,
    )?;
    // The two descriptive strings are retained; every other property must be neutral.
    require(
        info.claimed_account_id.is_empty()
            && info.claimed_account_character == 0
            && info.prospect_dt_key.is_empty()
            && info.faction_mission_dt_key.is_empty()
            && info.expire_time == 0
            && info.prospect_state == 0
            && info.associated_members.is_empty()
            && info.cost == 0
            && info.reward == 0
            && info.difficulty == 0
            && !info.insurance
            && !info.no_respawns
            && info.elapsed_time == 0
            && info.selected_drop_point == 0
            && info.custom_settings.is_empty(),
        None,
        "ProspectInfo.state",
        Unsupported,
    )
}

/// Check the original header, including index and both kinds of GUID extension.
/// Values and encoded names are never formatted into an error.
fn header_valid(header: &[u8], kind: TagKind) -> bool {
    fn string_end(bytes: &[u8], offset: &mut usize) -> Option<()> {
        let value = decode_string(bytes.get(*offset..)?, ByteOrder::Little).ok()?;
        *offset = offset.checked_add(value.consumed)?;
        Some(())
    }
    fn check(header: &[u8], kind: TagKind) -> Option<bool> {
        let mut offset = 0;
        string_end(header, &mut offset)?;
        string_end(header, &mut offset)?;
        let ints = header.get(offset..offset.checked_add(8)?)?;
        if ints[4..] != [0; 4] {
            return Some(false);
        }
        offset += 8;
        match kind {
            TagKind::Struct => {
                string_end(header, &mut offset)?;
                if header.get(offset..offset.checked_add(16)?)? != [0; 16] {
                    return Some(false);
                }
                offset += 16;
            }
            TagKind::Array | TagKind::Byte | TagKind::Enum | TagKind::Set => {
                string_end(header, &mut offset)?
            }
            TagKind::Map => {
                string_end(header, &mut offset)?;
                string_end(header, &mut offset)?;
            }
            TagKind::Bool => {
                header.get(offset)?;
                offset += 1;
            }
            _ => {}
        }
        Some(header.get(offset..) == Some(&[0][..]))
    }
    check(header, kind) == Some(true)
}

fn property<'a>(
    bytes: &'a [u8],
    index: Option<usize>,
    field: &'static str,
) -> SemanticResult<PropertyRecord<'a>> {
    let ParsedRecord::Property(p) = read_record(bytes, PROFILE)
        .map_err(|_| error(index, field, SemanticImportCategory::Header))?
    else {
        return Err(error(index, field, SemanticImportCategory::Header));
    };
    require(
        header_valid(p.tag.header, p.tag.kind),
        index,
        field,
        SemanticImportCategory::Header,
    )?;
    Ok(p)
}

/// Reject ignored wrapper fields and header extensions at every admitted root
/// boundary. Staging already supplies exact schema/type/duplicate validation.
pub(crate) fn validate_source_headers(payload: &[u8]) -> SemanticResult<()> {
    let roots = read_root(payload, PROFILE)
        .map_err(|_| error(None, "root", SemanticImportCategory::Header))?;
    for tag in roots.tags {
        let p = property(tag.record, None, "root.header")?;
        if exact(&p.name, "ProspectInfo") {
            let info = read_root(p.tag.payload, PROFILE)
                .map_err(|_| error(None, "ProspectInfo", SemanticImportCategory::Header))?;
            for child in info.tags {
                let p = property(child.record, None, "ProspectInfo.header")?;
                if p.tag.kind == TagKind::Array {
                    let shared = p.tag.payload.get(4..).ok_or_else(|| {
                        error(None, "ProspectInfo.array", SemanticImportCategory::Header)
                    })?;
                    property(shared, None, "ProspectInfo.array.header")?;
                }
            }
        } else if exact(&p.name, "StateRecorderBlobs") {
            let bytes =
                p.tag.payload.get(4..).ok_or_else(|| {
                    error(None, "StateRecorderBlobs", SemanticImportCategory::Header)
                })?;
            property(bytes, None, "StateRecorderBlobs.header")?;
        }
    }
    Ok(())
}

pub(crate) fn validate_wrapper(bytes: &[u8], index: usize) -> SemanticResult<()> {
    let roots = read_root(bytes, PROFILE)
        .map_err(|_| error(Some(index), "recorder", SemanticImportCategory::Header))?;
    require(
        roots.tags.len() == 2,
        Some(index),
        "recorder.fields",
        SemanticImportCategory::Unsupported,
    )?;
    for tag in roots.tags {
        property(tag.record, Some(index), "recorder.header")?;
    }
    Ok(())
}

pub(crate) struct SourceOwner {
    pub(crate) actor_class: Vec<u16>,
    pub(crate) actor_path: Vec<u16>,
    pub(crate) object_name: Vec<u16>,
    pub(crate) guid: i32,
    pub(crate) transform: NativeTransform,
    pub(crate) explicit_policy: Option<OwnerResolvePolicy>,
}

fn source_string(
    p: &PropertyRecord<'_>,
    kind: TagKind,
    index: usize,
    field: &'static str,
) -> SemanticResult<Vec<u16>> {
    require(
        p.tag.kind == kind,
        Some(index),
        field,
        SemanticImportCategory::Type,
    )?;
    let value = decode_string(p.tag.payload, ByteOrder::Little)
        .map_err(|_| error(Some(index), field, SemanticImportCategory::Value))?;
    require(
        value.consumed == p.tag.payload.len()
            && units_valid(&value.units, true)
            && canonical_source_string(p.tag.payload, &value.units),
        Some(index),
        field,
        SemanticImportCategory::Value,
    )?;
    Ok(value.units)
}

// Persisted metadata can be constructed without preparation. Keep its source
// admission aligned with preparation's zero-gap typing contract: forced-wide
// ASCII and other noncanonical scalar encodings are not part of this adapter.
fn canonical_source_string(payload: &[u8], units: &[u16]) -> bool {
    encode_string(units, ByteOrder::Little).is_ok_and(|encoded| encoded == payload)
}
fn source_int(p: &PropertyRecord<'_>, index: usize, field: &'static str) -> SemanticResult<i32> {
    require(
        p.tag.kind == TagKind::Int,
        Some(index),
        field,
        SemanticImportCategory::Type,
    )?;
    let bytes = p
        .tag
        .payload
        .try_into()
        .map_err(|_| error(Some(index), field, SemanticImportCategory::Value))?;
    Ok(i32::from_le_bytes(bytes))
}
fn false_bool(p: &PropertyRecord<'_>, index: usize, field: &'static str) -> SemanticResult<()> {
    require(
        p.tag.kind == TagKind::Bool && p.tag.payload.is_empty() && p.bool_value == Some(false),
        Some(index),
        field,
        SemanticImportCategory::Unsupported,
    )
}
fn struct_name(
    p: &PropertyRecord<'_>,
    name: &str,
    index: usize,
    field: &'static str,
) -> SemanticResult<()> {
    require(
        p.tag.kind == TagKind::Struct && p.extra.as_ref().is_some_and(|units| exact(units, name)),
        Some(index),
        field,
        SemanticImportCategory::Type,
    )
}
fn source_transform(p: &PropertyRecord<'_>, index: usize) -> SemanticResult<NativeTransform> {
    struct_name(p, "Transform", index, "ActorTransform")?;
    let roots = read_root(p.tag.payload, PROFILE)
        .map_err(|_| error(Some(index), "ActorTransform", SemanticImportCategory::Type))?;
    let (mut translation, mut rotation, mut scale) = (None, None, None);
    for tag in roots.tags {
        let p = property(tag.record, Some(index), "ActorTransform.header")?;
        let (slot, width, type_name, field) = if exact(&p.name, "Translation") {
            (&mut translation, 3, "Vector", "ActorTransform.Translation")
        } else if exact(&p.name, "Rotation") {
            (&mut rotation, 4, "Quat", "ActorTransform.Rotation")
        } else if exact(&p.name, "Scale3D") {
            (&mut scale, 3, "Vector", "ActorTransform.Scale3D")
        } else {
            return Err(error(
                Some(index),
                "ActorTransform.field",
                SemanticImportCategory::Unsupported,
            ));
        };
        require(
            slot.is_none(),
            Some(index),
            field,
            SemanticImportCategory::Header,
        )?;
        struct_name(&p, type_name, index, field)?;
        require(
            p.tag.payload.len() == width * 4,
            Some(index),
            field,
            SemanticImportCategory::Type,
        )?;
        *slot = Some(
            p.tag
                .payload
                .as_chunks::<4>()
                .0
                .iter()
                .map(|v| f32::from_le_bytes(*v))
                .collect::<Vec<_>>(),
        );
    }
    let missing = || {
        error(
            Some(index),
            "ActorTransform",
            SemanticImportCategory::Missing,
        )
    };
    let transform = NativeTransform {
        translation: translation
            .ok_or_else(missing)?
            .try_into()
            .map_err(|_| missing())?,
        rotation: rotation
            .ok_or_else(missing)?
            .try_into()
            .map_err(|_| missing())?,
        scale: scale
            .ok_or_else(missing)?
            .try_into()
            .map_err(|_| missing())?,
    };
    transform
        .validate()
        .map_err(|_| error(Some(index), "ActorTransform", SemanticImportCategory::Value))?;
    Ok(transform)
}

pub(crate) fn source_owner(
    bytes: &[u8],
    index: usize,
    profile: C001SourceProfile,
) -> SemanticResult<SourceOwner> {
    parse_owner(bytes, index, profile, |_| Ok(false))
}

fn parse_owner(
    bytes: &[u8],
    index: usize,
    profile: C001SourceProfile,
    mut extra: impl FnMut(&PropertyRecord<'_>) -> SemanticResult<bool>,
) -> SemanticResult<SourceOwner> {
    let component = read_base_component_with_limit(bytes, PROFILE, MAX_BOUNDED_COMPONENT_BYTES)
        .map_err(|_| error(Some(index), "component", SemanticImportCategory::Header))?;
    require(
        component.trailer() == [0; 4],
        Some(index),
        "component.trailer",
        SemanticImportCategory::Unsupported,
    )?;
    let (mut actor_class, mut actor_path, mut object_name, mut guid, mut transform) =
        (None, None, None, None, None);
    let (mut moved, mut version) = (false, false);
    let mut explicit_policy = None;
    let mut seen = BTreeSet::new();
    for tag in component.fields() {
        let p = property(tag.record, Some(index), "component.header")?;
        require(
            seen.insert(p.name.clone()),
            Some(index),
            "component.field",
            SemanticImportCategory::Header,
        )?;
        if exact(&p.name, "ActorClassName") {
            actor_class = Some(source_string(&p, TagKind::Name, index, "ActorClassName")?);
        } else if exact(&p.name, "ActorPathName") {
            actor_path = Some(source_string(&p, TagKind::String, index, "ActorPathName")?);
        } else if exact(&p.name, "ObjectFName") {
            object_name = Some(source_string(&p, TagKind::Name, index, "ObjectFName")?);
        } else if exact(&p.name, "IcarusActorGUID") {
            let value = source_int(&p, index, "IcarusActorGUID")?;
            require(
                value > 0,
                Some(index),
                "IcarusActorGUID",
                SemanticImportCategory::Value,
            )?;
            guid = Some(value);
        } else if exact(&p.name, "RecordedVersion") {
            require(
                source_int(&p, index, "RecordedVersion")? == 3,
                Some(index),
                "RecordedVersion",
                SemanticImportCategory::Unsupported,
            )?;
            version = true;
        } else if exact(&p.name, "bWasMovedToSubLevel") {
            false_bool(&p, index, "bWasMovedToSubLevel")?;
            moved = true;
        } else if exact(&p.name, "ActorTransform") {
            transform = Some(source_transform(&p, index)?);
        } else if exact(&p.name, "bSkipDefaultOwnerResolution") {
            false_bool(&p, index, "bSkipDefaultOwnerResolution")?;
        } else if exact(&p.name, "OwnerResolvePolicy") {
            let policy = match p.tag.kind {
                TagKind::Byte
                    if p.tag.payload.len() == 1
                        && p.extra.as_ref().is_some_and(|u| exact(u, "None")) =>
                {
                    OwnerResolvePolicy::from_saved(p.tag.payload[0])
                }
                TagKind::Byte | TagKind::Enum
                    if p.extra
                        .as_ref()
                        .is_some_and(|u| exact(u, "EStateRecorderOwnerResolvePolicy")) =>
                {
                    decode_string(p.tag.payload, ByteOrder::Little)
                        .ok()
                        .and_then(|v| {
                            if v.consumed != p.tag.payload.len()
                                || !canonical_source_string(p.tag.payload, &v.units)
                            {
                                return None;
                            }
                            [
                                ("FindOnly", OwnerResolvePolicy::FindOnly),
                                ("RespawnOnly", OwnerResolvePolicy::RespawnOnly),
                                ("FindOrRespawn", OwnerResolvePolicy::FindOrRespawn),
                            ]
                            .into_iter()
                            .find_map(|(name, policy)| {
                                (exact(&v.units, name)
                                    || exact(
                                        &v.units,
                                        &format!("EStateRecorderOwnerResolvePolicy::{name}"),
                                    ))
                                .then_some(policy)
                            })
                        })
                }
                _ => None,
            };
            require(
                policy.is_some_and(|p| profile.admits(p)),
                Some(index),
                "OwnerResolvePolicy",
                SemanticImportCategory::Unsupported,
            )?;
            explicit_policy = policy;
        } else if extra(&p)? {
            // Additional fields belong only to the explicitly selected profile.
        } else {
            return Err(error(
                Some(index),
                "component.field",
                SemanticImportCategory::Unsupported,
            ));
        }
    }
    require(
        moved,
        Some(index),
        "bWasMovedToSubLevel",
        SemanticImportCategory::Missing,
    )?;
    require(
        version,
        Some(index),
        "RecordedVersion",
        SemanticImportCategory::Missing,
    )?;
    let missing = |field| error(Some(index), field, SemanticImportCategory::Missing);
    Ok(SourceOwner {
        actor_class: actor_class.ok_or_else(|| missing("ActorClassName"))?,
        actor_path: actor_path.ok_or_else(|| missing("ActorPathName"))?,
        object_name: object_name.ok_or_else(|| missing("ObjectFName"))?,
        guid: guid.ok_or_else(|| missing("IcarusActorGUID"))?,
        transform: transform.ok_or_else(|| missing("ActorTransform"))?,
        explicit_policy,
    })
}

pub(crate) struct SourceScalarOwner {
    pub owner: SourceOwner,
    pub enabled_source_name: Vec<u16>,
    pub enabled: bool,
    pub remaining_uses_source_name: Vec<u16>,
    pub remaining_uses: i32,
}

pub(crate) fn source_scalar_owner(
    bytes: &[u8],
    index: usize,
    layout: &crate::native_c001_scalar_owners::CompiledC001ScalarLayout,
) -> SemanticResult<SourceScalarOwner> {
    // Scalar admission additionally requires canonical encoded header names.
    // Existing source_owner profiles keep their original acceptance behavior.
    let component = read_base_component_with_limit(bytes, PROFILE, MAX_BOUNDED_COMPONENT_BYTES)
        .map_err(|_| {
            error(
                Some(index),
                "scalar.component",
                SemanticImportCategory::Header,
            )
        })?;
    require(
        canonical_source_string(
            component.terminator(),
            &"None".encode_utf16().collect::<Vec<_>>(),
        ),
        Some(index),
        "scalar.terminator",
        SemanticImportCategory::Header,
    )?;
    for tag in component.fields() {
        let p = property(tag.record, Some(index), "scalar.component.header")?;
        require(
            canonical_header(p.tag.header, p.tag.kind),
            Some(index),
            "scalar.component.header",
            SemanticImportCategory::Header,
        )?;
        if exact(&p.name, "ActorTransform") {
            let root = read_root(p.tag.payload, PROFILE).map_err(|_| {
                error(
                    Some(index),
                    "scalar.transform",
                    SemanticImportCategory::Header,
                )
            })?;
            for child in root.tags {
                require(
                    canonical_header(child.header, child.kind),
                    Some(index),
                    "scalar.transform.header",
                    SemanticImportCategory::Header,
                )?;
            }
        }
    }
    let mut enabled = None;
    let mut remaining_uses = None;
    let owner = parse_owner(
        bytes,
        index,
        C001SourceProfile::AuthoredScalarOwnersV1,
        |p| {
            if p.name == layout.definition().enabled.array_name {
                let (name, value) = scalar_record(p, index, &layout.definition().enabled, true)?;
                let crate::native_world::NativeValue::Bool(value) = value else {
                    unreachable!()
                };
                enabled = Some((name, value));
                Ok(true)
            } else if p.name == layout.definition().remaining_uses.array_name {
                let (name, value) =
                    scalar_record(p, index, &layout.definition().remaining_uses, false)?;
                let crate::native_world::NativeValue::I32(value) = value else {
                    unreachable!()
                };
                remaining_uses = Some((name, value));
                Ok(true)
            } else {
                Ok(false)
            }
        },
    )?;
    let (enabled_source_name, enabled) = enabled.ok_or_else(|| {
        error(
            Some(index),
            "scalar.enabled",
            SemanticImportCategory::Missing,
        )
    })?;
    let (remaining_uses_source_name, remaining_uses) = remaining_uses.ok_or_else(|| {
        error(
            Some(index),
            "scalar.remaining_uses",
            SemanticImportCategory::Missing,
        )
    })?;
    Ok(SourceScalarOwner {
        owner,
        enabled_source_name,
        enabled,
        remaining_uses_source_name,
        remaining_uses,
    })
}
fn scalar_record(
    p: &PropertyRecord<'_>,
    index: usize,
    declaration: &crate::native_c001_scalar_owners::C001ScalarArrayV1,
    boolean: bool,
) -> SemanticResult<(Vec<u16>, crate::native_world::NativeValue)> {
    use crate::experimental::struct_array::read_tagged_struct_array_with_limit;
    use crate::native_world::NativeValue;
    let fail = || error(Some(index), "scalar.array", SemanticImportCategory::Header);
    require(
        p.tag.kind == TagKind::Array
            && p.extra.as_ref().is_some_and(|u| exact(u, "StructProperty")),
        Some(index),
        "scalar.array",
        SemanticImportCategory::Type,
    )?;
    // Count before the framing reader allocates its element/field collections.
    require(
        p.tag.payload.get(..4) == Some(&1_i32.to_le_bytes()[..]),
        Some(index),
        "scalar.count",
        SemanticImportCategory::Value,
    )?;
    let array =
        read_tagged_struct_array_with_limit(p.tag.payload, PROFILE, MAX_BOUNDED_COMPONENT_BYTES)
            .map_err(|_| fail())?;
    let shared = property(array.shared().record, Some(index), "scalar.shared")?;
    require(
        canonical_header(shared.tag.header, shared.tag.kind),
        Some(index),
        "scalar.shared.header",
        SemanticImportCategory::Header,
    )?;
    require(
        shared.name == declaration.element_tag_name
            && shared.tag.kind == TagKind::Struct
            && shared.extra.as_ref() == Some(&declaration.record_type),
        Some(index),
        "scalar.shared",
        SemanticImportCategory::Type,
    )?;
    let element = &array.elements()[0];
    require(
        canonical_source_string(
            element.terminator(),
            &"None".encode_utf16().collect::<Vec<_>>(),
        ),
        Some(index),
        "scalar.terminator",
        SemanticImportCategory::Header,
    )?;
    require(
        element.fields().len() == 2,
        Some(index),
        "scalar.fields",
        SemanticImportCategory::Unsupported,
    )?;
    let mut name = None;
    let mut value = None;
    for tag in element.fields() {
        let field = property(tag.record, Some(index), "scalar.header")?;
        require(
            canonical_header(field.tag.header, field.tag.kind),
            Some(index),
            "scalar.header",
            SemanticImportCategory::Header,
        )?;
        if exact(&field.name, "VariableName") && name.is_none() {
            name = Some(source_string(
                &field,
                TagKind::Name,
                index,
                "scalar.VariableName",
            )?);
        } else if field.name == declaration.value_field && value.is_none() {
            value = Some(if boolean {
                require(
                    field.tag.kind == TagKind::Bool
                        && field.tag.payload.is_empty()
                        && field.bool_value.is_some(),
                    Some(index),
                    "scalar.Bool",
                    SemanticImportCategory::Type,
                )?;
                NativeValue::Bool(field.bool_value.ok_or_else(fail)?)
            } else {
                NativeValue::I32(source_int(&field, index, "scalar.Int")?)
            });
        } else {
            return Err(error(
                Some(index),
                "scalar.field",
                SemanticImportCategory::Unsupported,
            ));
        }
    }
    Ok((name.ok_or_else(fail)?, value.ok_or_else(fail)?))
}

// Header string checks are scoped to the new profile so no old profile changes.
fn canonical_header(header: &[u8], kind: TagKind) -> bool {
    fn take_string(header: &[u8], offset: &mut usize) -> Option<()> {
        let bytes = header.get(*offset..)?;
        let decoded = decode_string(bytes, ByteOrder::Little).ok()?;
        if !canonical_source_string(bytes.get(..decoded.consumed)?, &decoded.units) {
            return None;
        }
        *offset = offset.checked_add(decoded.consumed)?;
        Some(())
    }
    let check = || -> Option<()> {
        let mut offset = 0;
        take_string(header, &mut offset)?;
        take_string(header, &mut offset)?;
        offset = offset.checked_add(8)?;
        match kind {
            TagKind::Struct | TagKind::Array | TagKind::Byte | TagKind::Enum | TagKind::Set => {
                take_string(header, &mut offset)?
            }
            TagKind::Map => {
                take_string(header, &mut offset)?;
                take_string(header, &mut offset)?;
            }
            _ => {}
        }
        Some(())
    };
    check().is_some()
}

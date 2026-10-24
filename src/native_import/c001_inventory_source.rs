//! Strict, independently authored inventory-bearing C001 source admission.
//! This parser assigns neither native row meanings nor inventory/stack identities.
use super::c001_source::{
    C001SourceProfile, canonical_header, frame_scalar_component, parse_scalar_component, property,
    source_int,
};
use super::{
    PROFILE, SemanticImportCategory, SemanticImportError, SemanticResult, error, exact, require,
};
use crate::{
    experimental::{
        archive::{ByteOrder, StringEncoding, read_string_frame},
        component::MAX_BOUNDED_COMPONENT_BYTES,
        restore_rules::OwnerResolvePolicy,
        struct_array::{TaggedStructArray, read_tagged_struct_array_with_limit},
        tagged::{MAX_HEADER_STRING_UNITS, PropertyRecord, RootTag, TagKind, read_root},
    },
    native_c001_scalar_owners::{C001ScalarLayoutV1, CompiledC001ScalarLayout},
    native_scalar_owners::encoded_size,
    native_world::NativeTransform,
};
use anyhow::{Context, Result, ensure};
use serde::{
    Deserialize, Deserializer, Serialize,
    de::{SeqAccess, Visitor},
};
use std::{cell::RefCell, fmt};

const MAX_NAME_UNITS: usize = 1024;
// Tagged header counts include the trailing zero unit; payload values use the
// separate content-unit bound above.
const MAX_LAYOUT_NAME_UNITS: usize = MAX_HEADER_STRING_UNITS - 1;
const MAX_NAME_CAPACITY: usize = 2050;
const MAX_NAME_TOTAL: usize = 1_048_576;
const MAX_LAYOUT_BYTES: usize = 4 * 1024 * 1024;
const MAX_INVENTORIES: usize = 8;
const MAX_ITEMS: usize = 512;
const MAX_RECORDER_ITEMS: usize = 4096;
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

fn bounded_units<'de, D: Deserializer<'de>>(d: D) -> std::result::Result<Vec<u16>, D::Error> {
    struct Units;
    impl<'de> Visitor<'de> for Units {
        type Value = Vec<u16>;
        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("bounded UTF16 units")
        }
        fn visit_seq<A: SeqAccess<'de>>(
            self,
            mut seq: A,
        ) -> std::result::Result<Self::Value, A::Error> {
            use serde::de::Error;
            if seq.size_hint().is_some_and(|n| n > MAX_LAYOUT_NAME_UNITS) {
                return Err(A::Error::custom("inventory declaration name limit"));
            }
            let mut units = Vec::new();
            while let Some(unit) = seq.next_element::<u16>()? {
                if units.len() == MAX_LAYOUT_NAME_UNITS {
                    return Err(A::Error::custom("inventory declaration name limit"));
                }
                units.push(unit);
            }
            if units.is_empty() {
                return Err(A::Error::custom("inventory declaration name limit"));
            }
            Ok(units)
        }
    }
    d.deserialize_seq(Units)
        .map_err(|_| <D::Error as serde::de::Error>::custom("inventory declaration name invalid"))
}

macro_rules! object_dto {
    (pub struct $name:ident { $($(#[$attr:meta])* pub $field:ident: $ty:ty),* $(,)? }) => {
        #[derive(Clone, PartialEq, Eq, Serialize)]
        pub struct $name { $($(#[$attr])* pub $field: $ty),* }
        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
                #[derive(Deserialize)]
                #[serde(deny_unknown_fields)]
                struct Fields { $($(#[$attr])* $field: $ty),* }
                let fields: Fields = crate::json::object(d)
                    .map_err(|_| <D::Error as serde::de::Error>::custom("inventory declaration object invalid"))?;
                Ok(Self { $($field: fields.$field),* })
            }
        }
        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.debug_struct(stringify!($name)).finish_non_exhaustive()
            }
        }
    };
}
object_dto! { pub struct StructArrayShapeV1 {
    #[serde(deserialize_with="bounded_units")] pub field_name:Vec<u16>,
    #[serde(deserialize_with="bounded_units")] pub element_tag_name:Vec<u16>,
    #[serde(deserialize_with="bounded_units")] pub record_type:Vec<u16>
} }
object_dto! { pub struct StructShapeV1 {
    #[serde(deserialize_with="bounded_units")] pub field_name:Vec<u16>,
    #[serde(deserialize_with="bounded_units")] pub record_type:Vec<u16>
} }
object_dto! { pub struct C001InventoryLayoutV1 {
    pub inventories:StructArrayShapeV1,
    #[serde(deserialize_with="bounded_units")] pub inventory_key_field:Vec<u16>,
    pub inventory_data:StructShapeV1,
    pub slots:StructArrayShapeV1,
    pub dynamic:StructArrayShapeV1,
    pub additional_stats:StructArrayShapeV1,
    pub alterations:StructArrayShapeV1,
    pub living_item_slots:StructArrayShapeV1
} }

/// Validated identities are immutable; Debug never prints authored names.
#[derive(Clone)]
pub struct CompiledC001InventoryLayout {
    definition: C001InventoryLayoutV1,
    scalar_definition: C001ScalarLayoutV1,
}
impl fmt::Debug for CompiledC001InventoryLayout {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CompiledC001InventoryLayout")
            .finish_non_exhaustive()
    }
}
impl CompiledC001InventoryLayout {
    pub fn compile(
        definition: C001InventoryLayoutV1,
        scalar: &CompiledC001ScalarLayout,
    ) -> Result<Self> {
        let mut total = 0_usize;
        let mut name = |value: &Vec<u16>| -> Result<()> {
            ensure!(
                (1..=MAX_LAYOUT_NAME_UNITS).contains(&value.len())
                    && value.capacity() <= MAX_NAME_CAPACITY
                    && value.iter().all(|u| *u != 0 && *u != u16::MAX),
                "inventory declaration name limit"
            );
            total = total
                .checked_add(value.len())
                .filter(|n| *n <= MAX_NAME_TOTAL)
                .context("inventory declaration aggregate limit")?;
            Ok(())
        };
        for shape in [
            &definition.inventories,
            &definition.slots,
            &definition.dynamic,
            &definition.additional_stats,
            &definition.alterations,
            &definition.living_item_slots,
        ] {
            name(&shape.field_name)?;
            name(&shape.element_tag_name)?;
            name(&shape.record_type)?;
        }
        name(&definition.inventory_key_field)?;
        name(&definition.inventory_data.field_name)?;
        name(&definition.inventory_data.record_type)?;
        encoded_size(&definition, MAX_LAYOUT_BYTES)
            .map_err(|_| anyhow::anyhow!("inventory declaration encoded limit"))?;
        // Component, inventory-record, inventory-data and D01 are independent
        // scopes. Shared tag/type identities may coincide across these scopes.
        ensure!(
            !OWNER_FIELDS
                .iter()
                .any(|n| exact(&definition.inventories.field_name, n))
                && definition.inventories.field_name != scalar.definition().enabled.array_name
                && definition.inventories.field_name
                    != scalar.definition().remaining_uses.array_name,
            "inventory component declaration collision"
        );
        ensure!(
            definition.inventory_key_field != definition.inventory_data.field_name,
            "inventory record declaration collision"
        );
        // The data scope has only slots. In D01 the four array names are fixed,
        // distinct, and disjoint from its four remaining scalar field names.
        for (shape, expected) in [
            (&definition.dynamic, "DynamicData"),
            (&definition.additional_stats, "AdditionalStats"),
            (&definition.alterations, "Alterations"),
            (&definition.living_item_slots, "LivingItemSlots"),
        ] {
            ensure!(
                exact(&shape.field_name, expected),
                "inventory slot declaration topology"
            );
        }
        Ok(Self {
            definition,
            scalar_definition: scalar.definition().clone(),
        })
    }
    pub fn definition(&self) -> &C001InventoryLayoutV1 {
        &self.definition
    }
}

macro_rules! redacted_result {
    (pub struct $name:ident { $(pub $field:ident: $ty:ty),* $(,)? }) => {
        pub struct $name { $(pub $field: $ty),* }
        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.debug_struct(stringify!($name)).finish_non_exhaustive()
            }
        }
    };
}
redacted_result! { pub struct SourceInventoryItemV1 {
    pub source_row:Vec<u16>, pub requested_slot:u16, pub count:u32
} }
redacted_result! { pub struct SourceInventoryV1 {
    pub source_key:Vec<u16>, pub items:Vec<SourceInventoryItemV1>
} }
// Owned source values only; no native binding, row mapping or StackID allocation.
redacted_result! { pub struct SourceInventoryOwner {
    pub actor_class:Vec<u16>, pub actor_path:Vec<u16>, pub object_name:Vec<u16>,
    pub guid:i32, pub transform:NativeTransform, pub explicit_policy:Option<OwnerResolvePolicy>,
    pub enabled_source_name:Vec<u16>, pub enabled:bool,
    pub remaining_uses_source_name:Vec<u16>, pub remaining_uses:i32,
    pub inventories:Vec<SourceInventoryV1>
} }
impl SourceInventoryOwner {
    pub fn effective_policy(&self) -> OwnerResolvePolicy {
        self.explicit_policy
            .unwrap_or(OwnerResolvePolicy::FindOrRespawn)
    }
}

#[derive(Default)]
struct Budget {
    names: usize,
    items: usize,
}
impl Budget {
    fn charge(&mut self, units: usize, index: usize) -> SemanticResult<()> {
        self.names = self
            .names
            .checked_add(units)
            .filter(|n| *n <= MAX_NAME_TOTAL)
            .ok_or_else(|| {
                error(
                    Some(index),
                    "inventory.names",
                    SemanticImportCategory::Limit,
                )
            })?;
        Ok(())
    }
}
fn check(
    ok: bool,
    index: usize,
    field: &'static str,
    category: SemanticImportCategory,
) -> SemanticResult<()> {
    require(ok, Some(index), field, category)
}
fn strict_property<'a>(tag: &RootTag<'a>, index: usize) -> SemanticResult<PropertyRecord<'a>> {
    let p = property(tag.record, Some(index), "inventory.header")?;
    check(
        canonical_header(p.tag.header, p.tag.kind),
        index,
        "inventory.header",
        SemanticImportCategory::Header,
    )?;
    Ok(p)
}
fn terminator(bytes: &[u8], index: usize) -> SemanticResult<()> {
    check(
        bytes == b"\x05\0\0\0None\0",
        index,
        "inventory.terminator",
        SemanticImportCategory::Header,
    )
}
// Borrow and bound before collecting units. Canonical empty strings have count
// zero; canonical nonempty wide strings contain at least one non-ASCII unit.
fn name_units(
    p: &PropertyRecord<'_>,
    kind: TagKind,
    nonempty: bool,
    index: usize,
    budget: &mut Budget,
) -> SemanticResult<Vec<u16>> {
    check(
        p.tag.kind == kind,
        index,
        "inventory.name",
        SemanticImportCategory::Type,
    )?;
    let frame = read_string_frame(p.tag.payload, ByteOrder::Little)
        .map_err(|_| error(Some(index), "inventory.name", SemanticImportCategory::Value))?;
    check(
        frame.consumed() == p.tag.payload.len()
            && (!nonempty || frame.unit_count() > 0)
            && frame.unit_count() <= MAX_NAME_UNITS
            && match frame.encoding() {
                StringEncoding::Empty => true,
                StringEncoding::Narrow => frame.unit_count() > 0,
                StringEncoding::Wide => frame.units().any(|u| u > 127),
            },
        index,
        "inventory.name",
        SemanticImportCategory::Value,
    )?;
    budget.charge(frame.unit_count(), index)?;
    Ok(frame.units().collect())
}
fn struct_array<'a>(
    p: &PropertyRecord<'a>,
    shape: &StructArrayShapeV1,
    min: usize,
    max: usize,
    index: usize,
) -> SemanticResult<TaggedStructArray<'a>> {
    check(
        p.tag.kind == TagKind::Array
            && p.extra.as_ref().is_some_and(|u| exact(u, "StructProperty")),
        index,
        "inventory.array",
        SemanticImportCategory::Type,
    )?;
    let count = p
        .tag
        .payload
        .get(..4)
        .and_then(|v| <[u8; 4]>::try_from(v).ok())
        .and_then(|v| usize::try_from(i32::from_le_bytes(v)).ok());
    // Every caller supplies its semantic count bound before framing can
    // allocate the array's element and field collections (including empty arrays).
    check(
        count.is_some_and(|n| (min..=max).contains(&n)),
        index,
        "inventory.count",
        SemanticImportCategory::Limit,
    )?;
    let array =
        read_tagged_struct_array_with_limit(p.tag.payload, PROFILE, MAX_BOUNDED_COMPONENT_BYTES)
            .map_err(|_| {
                error(
                    Some(index),
                    "inventory.array",
                    SemanticImportCategory::Header,
                )
            })?;
    let shared = strict_property(array.shared(), index)?;
    check(
        shared.name == shape.element_tag_name
            && shared.tag.kind == TagKind::Struct
            && shared.extra.as_ref() == Some(&shape.record_type),
        index,
        "inventory.shared",
        SemanticImportCategory::Type,
    )?;
    for element in array.elements() {
        terminator(element.terminator(), index)?;
    }
    Ok(array)
}
fn empty_array(
    p: &PropertyRecord<'_>,
    shape: &StructArrayShapeV1,
    index: usize,
) -> SemanticResult<()> {
    struct_array(p, shape, 0, 0, index).map(|_| ())
}
fn dynamic_count(
    p: &PropertyRecord<'_>,
    layout: &C001InventoryLayoutV1,
    index: usize,
) -> SemanticResult<u32> {
    let array = struct_array(p, &layout.dynamic, 1, 1, index)?;
    let fields = array.elements()[0].fields();
    check(
        fields.len() == 2,
        index,
        "inventory.dynamic.fields",
        SemanticImportCategory::Unsupported,
    )?;
    let (mut selector, mut value) = (None, None);
    for tag in fields {
        let p = strict_property(tag, index)?;
        if exact(&p.name, "Index") && selector.is_none() {
            selector = Some(source_int(&p, index, "inventory.dynamic.Index")?);
        } else if exact(&p.name, "Value") && value.is_none() {
            value = Some(source_int(&p, index, "inventory.dynamic.Value")?);
        } else {
            return Err(error(
                Some(index),
                "inventory.dynamic.field",
                SemanticImportCategory::Unsupported,
            ));
        }
    }
    check(
        selector == Some(7),
        index,
        "inventory.dynamic.Index",
        SemanticImportCategory::Unsupported,
    )?;
    let value = value
        .filter(|v| (1..=1_000_000).contains(v))
        .ok_or_else(|| {
            error(
                Some(index),
                "inventory.dynamic.Value",
                SemanticImportCategory::Value,
            )
        })?;
    Ok(value as u32)
}
fn item(
    fields: &[RootTag<'_>],
    layout: &C001InventoryLayoutV1,
    index: usize,
    budget: &mut Budget,
) -> SemanticResult<SourceInventoryItemV1> {
    check(
        fields.len() == 8,
        index,
        "inventory.slot.fields",
        SemanticImportCategory::Unsupported,
    )?;
    let names = [
        "Location",
        "ItemStaticData",
        "DynamicData",
        "AdditionalStats",
        "Alterations",
        "LivingItemSlots",
        "ItemGuid",
        "ItemOwnerLookupId",
    ];
    let (mut seen, mut location, mut row, mut count) = (0_u16, None, None, None);
    for tag in fields {
        let p = strict_property(tag, index)?;
        let field = names
            .iter()
            .position(|n| exact(&p.name, n))
            .ok_or_else(|| {
                error(
                    Some(index),
                    "inventory.slot.field",
                    SemanticImportCategory::Unsupported,
                )
            })?;
        check(
            seen & (1 << field) == 0,
            index,
            "inventory.slot.field",
            SemanticImportCategory::Header,
        )?;
        seen |= 1 << field;
        match field {
            0 => {
                let v = source_int(&p, index, "inventory.Location")?;
                check(
                    (0..=255).contains(&v),
                    index,
                    "inventory.Location",
                    SemanticImportCategory::Value,
                )?;
                location = Some(v as u16);
            }
            1 => row = Some(name_units(&p, TagKind::Name, true, index, budget)?),
            2 => count = Some(dynamic_count(&p, layout, index)?),
            3 => empty_array(&p, &layout.additional_stats, index)?,
            4 => empty_array(&p, &layout.alterations, index)?,
            5 => empty_array(&p, &layout.living_item_slots, index)?,
            6 => {
                check(
                    name_units(&p, TagKind::String, false, index, budget)?.is_empty(),
                    index,
                    "inventory.ItemGuid",
                    SemanticImportCategory::Unsupported,
                )?;
            }
            7 => check(
                source_int(&p, index, "inventory.ItemOwnerLookupId")? == -1,
                index,
                "inventory.ItemOwnerLookupId",
                SemanticImportCategory::Unsupported,
            )?,
            _ => unreachable!(),
        }
    }
    let missing = || {
        error(
            Some(index),
            "inventory.slot",
            SemanticImportCategory::Missing,
        )
    };
    Ok(SourceInventoryItemV1 {
        source_row: row.ok_or_else(missing)?,
        requested_slot: location.ok_or_else(missing)?,
        count: count.ok_or_else(missing)?,
    })
}
fn inventory_items(
    p: &PropertyRecord<'_>,
    layout: &C001InventoryLayoutV1,
    index: usize,
    budget: &mut Budget,
) -> SemanticResult<Vec<SourceInventoryItemV1>> {
    check(
        p.tag.kind == TagKind::Struct
            && p.extra.as_ref() == Some(&layout.inventory_data.record_type),
        index,
        "inventory.data",
        SemanticImportCategory::Type,
    )?;
    let root = read_root(p.tag.payload, PROFILE).map_err(|_| {
        error(
            Some(index),
            "inventory.data",
            SemanticImportCategory::Header,
        )
    })?;
    terminator(root.terminator, index)?;
    check(
        root.tags.len() == 1,
        index,
        "inventory.data.fields",
        SemanticImportCategory::Unsupported,
    )?;
    let slots = strict_property(&root.tags[0], index)?;
    check(
        slots.name == layout.slots.field_name,
        index,
        "inventory.data.field",
        SemanticImportCategory::Unsupported,
    )?;
    let remaining_items = MAX_RECORDER_ITEMS
        .checked_sub(budget.items)
        .ok_or_else(|| {
            error(
                Some(index),
                "inventory.items",
                SemanticImportCategory::Limit,
            )
        })?;
    let array = struct_array(
        &slots,
        &layout.slots,
        0,
        MAX_ITEMS.min(remaining_items),
        index,
    )?;
    budget.items = budget
        .items
        .checked_add(array.elements().len())
        .filter(|n| *n <= MAX_RECORDER_ITEMS)
        .ok_or_else(|| {
            error(
                Some(index),
                "inventory.items",
                SemanticImportCategory::Limit,
            )
        })?;
    let mut items = Vec::with_capacity(array.elements().len());
    for element in array.elements() {
        items.push(item(element.fields(), layout, index, budget)?);
    }
    Ok(items)
}
fn inventories(
    p: &PropertyRecord<'_>,
    layout: &C001InventoryLayoutV1,
    index: usize,
    budget: &mut Budget,
) -> SemanticResult<Vec<SourceInventoryV1>> {
    let array = struct_array(p, &layout.inventories, 1, MAX_INVENTORIES, index)?;
    let mut inventories: Vec<SourceInventoryV1> = Vec::with_capacity(array.elements().len());
    for element in array.elements() {
        check(
            element.fields().len() == 2,
            index,
            "inventory.fields",
            SemanticImportCategory::Unsupported,
        )?;
        let (mut key, mut items) = (None, None);
        for tag in element.fields() {
            let p = strict_property(tag, index)?;
            if p.name == layout.inventory_key_field && key.is_none() {
                key = Some(name_units(&p, TagKind::Name, true, index, budget)?);
            } else if p.name == layout.inventory_data.field_name && items.is_none() {
                items = Some(inventory_items(&p, layout, index, budget)?);
            } else {
                return Err(error(
                    Some(index),
                    "inventory.field",
                    SemanticImportCategory::Unsupported,
                ));
            }
        }
        let missing = || {
            error(
                Some(index),
                "inventory.record",
                SemanticImportCategory::Missing,
            )
        };
        let source_key = key.ok_or_else(missing)?;
        check(
            !inventories.iter().any(|v| v.source_key == source_key),
            index,
            "inventory.key",
            SemanticImportCategory::Binding,
        )?;
        inventories.push(SourceInventoryV1 {
            source_key,
            items: items.ok_or_else(missing)?,
        });
    }
    Ok(inventories)
}

pub fn source_inventory_owner(
    bytes: &[u8],
    source_index: usize,
    scalar: &CompiledC001ScalarLayout,
    inventory: &CompiledC001InventoryLayout,
) -> std::result::Result<SourceInventoryOwner, SemanticImportError> {
    let index = source_index;
    check(
        scalar.definition() == &inventory.scalar_definition,
        index,
        "inventory.scalar_layout",
        SemanticImportCategory::Binding,
    )?;
    let component = frame_scalar_component(bytes, index)?;
    let mut budget = RefCell::new(Budget::default());
    // Bound/charge owner names before the shared scalar parser clones them.
    // Scalar and inventory identities use the same budget, charged by their
    // readers before collecting UTF16 values.
    for tag in component.fields() {
        let p = property(tag.record, Some(index), "inventory.component.header")?;
        if ["ActorClassName", "ActorPathName", "ObjectFName"]
            .iter()
            .any(|n| exact(&p.name, n))
        {
            let frame = read_string_frame(p.tag.payload, ByteOrder::Little).map_err(|_| {
                error(
                    Some(index),
                    "inventory.owner.name",
                    SemanticImportCategory::Value,
                )
            })?;
            check(
                frame.unit_count() <= MAX_NAME_UNITS,
                index,
                "inventory.owner.name",
                SemanticImportCategory::Limit,
            )?;
            budget.get_mut().charge(frame.unit_count(), index)?;
        }
        if exact(&p.name, "OwnerResolvePolicy")
            && matches!(p.tag.kind, TagKind::Byte | TagKind::Enum)
            && p.extra
                .as_ref()
                .is_some_and(|u| exact(u, "EStateRecorderOwnerResolvePolicy"))
        {
            let frame = read_string_frame(p.tag.payload, ByteOrder::Little).map_err(|_| {
                error(
                    Some(index),
                    "inventory.policy",
                    SemanticImportCategory::Value,
                )
            })?;
            check(
                frame.unit_count() <= MAX_NAME_UNITS,
                index,
                "inventory.policy",
                SemanticImportCategory::Limit,
            )?;
            budget.get_mut().charge(frame.unit_count(), index)?;
        }
        if exact(&p.name, "ActorTransform") {
            let root = read_root(p.tag.payload, PROFILE).map_err(|_| {
                error(
                    Some(index),
                    "inventory.transform",
                    SemanticImportCategory::Header,
                )
            })?;
            terminator(root.terminator, index)?;
        }
    }
    let mut parsed_inventories = None;
    let source = parse_scalar_component(
        &component,
        index,
        scalar,
        C001SourceProfile::AuthoredInventoryOwnersV1,
        |p, i| name_units(p, TagKind::Name, true, i, &mut budget.borrow_mut()),
        |p| {
            if p.name == inventory.definition.inventories.field_name {
                parsed_inventories = Some(inventories(
                    p,
                    &inventory.definition,
                    index,
                    &mut budget.borrow_mut(),
                )?);
                Ok(true)
            } else {
                Ok(false)
            }
        },
    )?;
    let inventories = parsed_inventories.ok_or_else(|| {
        error(
            Some(index),
            "inventory.array",
            SemanticImportCategory::Missing,
        )
    })?;
    let owner = source.owner;
    Ok(SourceInventoryOwner {
        actor_class: owner.actor_class,
        actor_path: owner.actor_path,
        object_name: owner.object_name,
        guid: owner.guid,
        transform: owner.transform,
        explicit_policy: owner.explicit_policy,
        enabled_source_name: source.enabled_source_name,
        enabled: source.enabled,
        remaining_uses_source_name: source.remaining_uses_source_name,
        remaining_uses: source.remaining_uses,
        inventories,
    })
}

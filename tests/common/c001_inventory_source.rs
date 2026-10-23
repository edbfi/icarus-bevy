//! Independently authored nested inventory fixtures, using the scalar tag writer.
#![allow(dead_code)]
use crate::scalar::{Tag, scalar_array, stream, string, transform, units};
use icarus_bevy::{
    native_c001_scalar_owners::{C001ScalarArrayV1, C001ScalarLayoutV1, CompiledC001ScalarLayout},
    native_import::c001_inventory_source::{
        C001InventoryLayoutV1, CompiledC001InventoryLayout, StructArrayShapeV1, StructShapeV1,
    },
    native_world::NativeTransform,
};

pub fn scalar_definition() -> C001ScalarLayoutV1 {
    C001ScalarLayoutV1 {
        enabled: C001ScalarArrayV1 {
            array_name: units("AuthoredEnabled"),
            element_tag_name: units("EnabledElement"),
            record_type: units("AuthoredBoolRecord"),
            value_field: units("SavedBool"),
        },
        remaining_uses: C001ScalarArrayV1 {
            array_name: units("AuthoredUses"),
            element_tag_name: units("UsesElement"),
            record_type: units("AuthoredIntRecord"),
            value_field: units("SavedInt"),
        },
    }
}
pub fn shape(field: &str, element: &str, record: &str) -> StructArrayShapeV1 {
    StructArrayShapeV1 {
        field_name: units(field),
        element_tag_name: units(element),
        record_type: units(record),
    }
}
pub fn definition() -> C001InventoryLayoutV1 {
    C001InventoryLayoutV1 {
        inventories: shape(
            "AuthoredInventories",
            "InventoryElement",
            "AuthoredInventoryRecord",
        ),
        inventory_key_field: units("SourceInventoryKey"),
        inventory_data: StructShapeV1 {
            field_name: units("AuthoredInventoryData"),
            record_type: units("AuthoredInventoryState"),
        },
        slots: shape("AuthoredSlots", "SlotElement", "AuthoredD01"),
        dynamic: shape("DynamicData", "DynamicElement", "AuthoredDynamic"),
        additional_stats: shape("AdditionalStats", "StatsElement", "AuthoredStats"),
        alterations: shape("Alterations", "AlterElement", "AuthoredAlteration"),
        living_item_slots: shape("LivingItemSlots", "LivingElement", "AuthoredLiving"),
    }
}
pub fn layouts() -> (CompiledC001ScalarLayout, CompiledC001InventoryLayout) {
    let scalar = CompiledC001ScalarLayout::compile(scalar_definition()).unwrap();
    let inventory = CompiledC001InventoryLayout::compile(definition(), &scalar).unwrap();
    (scalar, inventory)
}
pub fn array(
    field: &'static str,
    element: &'static str,
    record: &str,
    records: &[Vec<Tag>],
) -> Tag {
    Tag::array(
        field,
        "StructProperty",
        [
            i32::try_from(records.len()).unwrap().to_le_bytes().to_vec(),
            Tag::structure(
                element,
                record,
                records.iter().flat_map(|r| stream(r)).collect(),
            )
            .bytes(),
        ]
        .concat(),
    )
}
pub fn dynamic(fields: Vec<Tag>) -> Tag {
    array(
        "DynamicData",
        "DynamicElement",
        "AuthoredDynamic",
        &[fields],
    )
}
pub fn item_fields(row: &[u16], slot: i32, count: i32) -> Vec<Tag> {
    // Non-schema order discriminates readers that bind by position.
    vec![
        Tag::integer("ItemOwnerLookupId", -1),
        array("Alterations", "AlterElement", "AuthoredAlteration", &[]),
        Tag::text("ItemStaticData", "NameProperty", row),
        dynamic(vec![Tag::integer("Value", count), Tag::integer("Index", 7)]),
        Tag::integer("Location", slot),
        array("LivingItemSlots", "LivingElement", "AuthoredLiving", &[]),
        Tag::text("ItemGuid", "StrProperty", &[]),
        array("AdditionalStats", "StatsElement", "AuthoredStats", &[]),
    ]
}
pub fn data(items: &[Vec<Tag>]) -> Tag {
    Tag::structure(
        "AuthoredInventoryData",
        "AuthoredInventoryState",
        stream(&[array("AuthoredSlots", "SlotElement", "AuthoredD01", items)]),
    )
}
pub fn inventory_fields(key: &[u16], items: &[Vec<Tag>]) -> Vec<Tag> {
    vec![
        data(items),
        Tag::text("SourceInventoryKey", "NameProperty", key),
    ]
}
pub fn inventory_array(records: &[Vec<Tag>]) -> Tag {
    array(
        "AuthoredInventories",
        "InventoryElement",
        "AuthoredInventoryRecord",
        records,
    )
}
pub fn owner_fields(inventory: Tag) -> Vec<Tag> {
    vec![
        Tag::integer("RecordedVersion", 3),
        Tag::text(
            "ActorPathName",
            "StrProperty",
            &units("/Authored/InventoryOwner"),
        ),
        scalar_array(
            "AuthoredUses",
            "UsesElement",
            "AuthoredIntRecord",
            Tag::integer("SavedInt", 42),
            &[0xd801, 71],
        ),
        Tag::text("ObjectFName", "NameProperty", &[0xd802, 72]),
        inventory,
        Tag::boolean("bWasMovedToSubLevel", false),
        Tag::text("ActorClassName", "NameProperty", &units("AuthoredCrate")),
        scalar_array(
            "AuthoredEnabled",
            "EnabledElement",
            "AuthoredBoolRecord",
            Tag::boolean("SavedBool", true),
            &[0xd800, 70],
        ),
        transform(&NativeTransform {
            translation: [3.0, 5.0, 7.0],
            rotation: [0.0, 0.0, 0.6, 0.8],
            scale: [2.0, 3.0, 4.0],
        }),
        Tag::integer("IcarusActorGUID", 901),
    ]
}
pub fn component(fields: &[Tag]) -> Vec<u8> {
    [stream(fields), vec![0; 4]].concat()
}
pub fn one_item(fields: Vec<Tag>) -> Vec<Tag> {
    owner_fields(inventory_array(&[inventory_fields(
        &units("InventoryZ"),
        &[fields],
    )]))
}
pub fn complete() -> Vec<Tag> {
    owner_fields(inventory_array(&[
        inventory_fields(
            &[0xd803, 73],
            &[
                item_fields(&[0xd804, 74], 201, 3),
                item_fields(&units("AuthoredSecondRow"), 0, 17),
            ],
        ),
        inventory_fields(&units("EmptyInventory"), &[]),
        inventory_fields(&units("InventoryA"), &[item_fields(&[0xd804, 74], 9, 1)]),
    ]))
}
pub fn replace(fields: &mut [Tag], name: &str, value: Tag) {
    *fields.iter_mut().find(|f| f.name == name).unwrap() = value;
}
pub fn field<'a>(fields: &'a mut [Tag], name: &str) -> &'a mut Tag {
    fields.iter_mut().find(|f| f.name == name).unwrap()
}
pub fn wide_ascii(text: &str) -> Vec<u8> {
    [
        (-i32::try_from(text.len() + 1).unwrap())
            .to_le_bytes()
            .to_vec(),
        text.encode_utf16()
            .chain([0])
            .flat_map(u16::to_le_bytes)
            .collect(),
    ]
    .concat()
}
pub fn forged_count(mut tag: Tag, count: i32) -> Tag {
    tag.payload[..4].copy_from_slice(&count.to_le_bytes());
    tag
}
pub fn empty_unknown() -> Tag {
    Tag::array(
        "PrivateSentinelUnknown",
        "IntProperty",
        0_i32.to_le_bytes().to_vec(),
    )
}
pub fn noncanonical_terminator(mut payload: Vec<u8>) -> Vec<u8> {
    payload.truncate(payload.len() - string("None").len());
    payload.extend(wide_ascii("None"));
    payload
}

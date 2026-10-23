#[path = "common/c001_inventory_source.rs"]
mod authored;
#[path = "common/c001_scalar_owners.rs"]
mod scalar;

use authored::*;
use icarus_bevy::{
    experimental::restore_rules::OwnerResolvePolicy,
    native_c001_scalar_owners::CompiledC001ScalarLayout,
    native_import::{
        SemanticImportCategory, SemanticImportError,
        c001_inventory_source::{
            C001InventoryLayoutV1, CompiledC001InventoryLayout, SourceInventoryOwner,
            StructArrayShapeV1, StructShapeV1, source_inventory_owner,
        },
    },
};
use scalar::{Tag, stream, string, units};

fn parse(fields: &[Tag]) -> Result<SourceInventoryOwner, SemanticImportError> {
    let (scalar, inventory) = layouts();
    source_inventory_owner(&component(fields), 37, &scalar, &inventory)
}
fn reject(fields: &[Tag]) -> SemanticImportError {
    let error = parse(fields).unwrap_err();
    assert_eq!(error.recorder_index, Some(37));
    error
}
fn base_item() -> Vec<Tag> {
    item_fields(&units("AuthoredRow"), 23, 11)
}

#[test]
fn complete_owner_scalars_inventory_order_and_exact_utf16_are_retained() {
    let parsed = parse(&complete()).unwrap();
    assert_eq!(parsed.actor_class, units("AuthoredCrate"));
    assert_eq!(parsed.actor_path, units("/Authored/InventoryOwner"));
    assert_eq!(parsed.object_name, [0xd802, 72]);
    assert_eq!(parsed.guid, 901);
    assert_eq!(parsed.transform.translation, [3.0, 5.0, 7.0]);
    assert_eq!(parsed.transform.rotation, [0.0, 0.0, 0.6, 0.8]);
    assert_eq!(parsed.transform.scale, [2.0, 3.0, 4.0]);
    assert_eq!(parsed.explicit_policy, None);
    assert_eq!(parsed.effective_policy(), OwnerResolvePolicy::FindOrRespawn);
    assert!(parsed.enabled);
    assert_eq!(parsed.enabled_source_name, [0xd800, 70]);
    assert_eq!(parsed.remaining_uses, 42);
    assert_eq!(parsed.remaining_uses_source_name, [0xd801, 71]);
    assert_eq!(parsed.inventories.len(), 3);
    assert_eq!(parsed.inventories[0].source_key, [0xd803, 73]);
    assert_eq!(parsed.inventories[0].items[0].source_row, [0xd804, 74]);
    assert_eq!(parsed.inventories[0].items[0].requested_slot, 201);
    assert_eq!(parsed.inventories[0].items[0].count, 3);
    assert_eq!(
        parsed.inventories[0].items[1].source_row,
        units("AuthoredSecondRow")
    );
    assert_eq!(parsed.inventories[0].items[1].requested_slot, 0);
    assert_eq!(parsed.inventories[0].items[1].count, 17);
    assert!(parsed.inventories[1].items.is_empty());
    assert_eq!(parsed.inventories[2].source_key, units("InventoryA"));
    assert_eq!(parsed.inventories[2].items[0].count, 1);
}

#[test]
fn order_is_source_order_and_requested_location_does_no_domain_routing() {
    for location in [0, 1, 7, 255] {
        let parsed = parse(&one_item(item_fields(&units("SameRow"), location, 5))).unwrap();
        assert_eq!(parsed.inventories[0].items.len(), 1);
        assert_eq!(
            parsed.inventories[0].items[0].requested_slot,
            location as u16
        );
    }
    let repeated = base_item();
    let fields = owner_fields(inventory_array(&[inventory_fields(
        &units("K"),
        &[repeated.clone(), repeated],
    )]));
    assert_eq!(parse(&fields).unwrap().inventories[0].items.len(), 2);
    // No slot uniqueness or row-meaning mapping belongs to this source parser.
}

#[test]
fn every_slot_field_is_required_and_duplicate_or_even_empty_extra_is_rejected() {
    let item = base_item();
    for at in 0..8 {
        let mut missing = item.clone();
        missing.remove(at);
        reject(&one_item(missing));
        let mut duplicate = item.clone();
        duplicate[at] = item[(at + 1) % 8].clone();
        reject(&one_item(duplicate));
    }
    let mut extra = item;
    extra.push(empty_unknown());
    reject(&one_item(extra));
}

#[test]
fn inventory_record_data_and_component_are_exhaustively_consumed() {
    let valid = inventory_fields(&units("K"), &[]);
    for at in 0..valid.len() {
        let mut missing = valid.clone();
        missing.remove(at);
        reject(&owner_fields(inventory_array(&[missing])));
        let duplicate = vec![valid[at].clone(), valid[at].clone()];
        reject(&owner_fields(inventory_array(&[duplicate])));
    }
    let mut extra = valid.clone();
    extra.push(empty_unknown());
    reject(&owner_fields(inventory_array(&[extra])));
    for data_fields in [
        vec![],
        vec![empty_unknown()],
        vec![
            array("AuthoredSlots", "SlotElement", "AuthoredD01", &[]),
            empty_unknown(),
        ],
    ] {
        let mut record = valid.clone();
        replace(
            &mut record,
            "AuthoredInventoryData",
            Tag::structure(
                "AuthoredInventoryData",
                "AuthoredInventoryState",
                stream(&data_fields),
            ),
        );
        reject(&owner_fields(inventory_array(&[record])));
    }
    let mut extra = complete();
    extra.push(empty_unknown());
    reject(&extra);
    let mut missing = complete();
    missing.retain(|p| p.name != "AuthoredInventories");
    reject(&missing);
    let mut duplicate = complete();
    duplicate.push(field(&mut complete(), "AuthoredInventories").clone());
    reject(&duplicate);
}

#[test]
fn inventory_keys_are_exact_nonempty_unique_and_not_normalized() {
    let a = inventory_fields(&[0xd800, 90], &[]);
    let b = inventory_fields(&[0xd801, 90], &[]);
    assert_eq!(
        parse(&owner_fields(inventory_array(&[a.clone(), b])))
            .unwrap()
            .inventories
            .len(),
        2
    );
    reject(&owner_fields(inventory_array(&[a.clone(), a])));
    reject(&owner_fields(inventory_array(&[inventory_fields(
        &[],
        &[],
    )])));
    let mut wrong_type = inventory_fields(&units("K"), &[]);
    field(&mut wrong_type, "SourceInventoryKey").kind = "StrProperty";
    reject(&owner_fields(inventory_array(&[wrong_type])));
}

#[test]
fn all_shared_descriptors_are_checked_including_empty_arrays() {
    for (name, element, record) in [
        ("AdditionalStats", "StatsElement", "AuthoredStats"),
        ("Alterations", "AlterElement", "AuthoredAlteration"),
        ("LivingItemSlots", "LivingElement", "AuthoredLiving"),
    ] {
        for wrong in [
            array(name, "WrongElement", record, &[]),
            array(name, element, "WrongType", &[]),
            Tag::array(name, "IntProperty", 0_i32.to_le_bytes().to_vec()),
            Tag::array(name, "StructProperty", 0_i32.to_le_bytes().to_vec()),
            array(name, element, record, &[vec![]]),
        ] {
            let mut item = base_item();
            replace(&mut item, name, wrong);
            reject(&one_item(item));
        }
    }
    for slots in [
        array("AuthoredSlots", "WrongElement", "AuthoredD01", &[]),
        array("AuthoredSlots", "SlotElement", "WrongType", &[]),
    ] {
        let record = vec![
            Tag::text("SourceInventoryKey", "NameProperty", &units("K")),
            Tag::structure(
                "AuthoredInventoryData",
                "AuthoredInventoryState",
                stream(&[slots]),
            ),
        ];
        reject(&owner_fields(inventory_array(&[record])));
    }
    for root in [
        array(
            "AuthoredInventories",
            "WrongElement",
            "AuthoredInventoryRecord",
            &[inventory_fields(&units("K"), &[])],
        ),
        array(
            "AuthoredInventories",
            "InventoryElement",
            "WrongType",
            &[inventory_fields(&units("K"), &[])],
        ),
    ] {
        reject(&owner_fields(root));
    }
}

#[test]
fn data_struct_dynamic_array_and_slot_shared_headers_have_exact_identities() {
    let mut record = inventory_fields(&units("K"), &[]);
    field(&mut record, "AuthoredInventoryData").extras =
        [string("WrongType"), vec![0; 17]].concat();
    reject(&owner_fields(inventory_array(&[record])));
    for dynamic in [
        array(
            "DynamicData",
            "WrongElement",
            "AuthoredDynamic",
            &[vec![Tag::integer("Index", 7), Tag::integer("Value", 1)]],
        ),
        array(
            "DynamicData",
            "DynamicElement",
            "WrongType",
            &[vec![Tag::integer("Index", 7), Tag::integer("Value", 1)]],
        ),
        array("DynamicData", "DynamicElement", "AuthoredDynamic", &[]),
    ] {
        let mut item = base_item();
        replace(&mut item, "DynamicData", dynamic);
        reject(&one_item(item));
    }
    let wrong_slots = array("AuthoredSlots", "SlotElement", "WrongType", &[base_item()]);
    let record = vec![
        Tag::text("SourceInventoryKey", "NameProperty", &units("K")),
        Tag::structure(
            "AuthoredInventoryData",
            "AuthoredInventoryState",
            stream(&[wrong_slots]),
        ),
    ];
    reject(&owner_fields(inventory_array(&[record])));
}

#[test]
fn dynamic_state_is_exact_and_selector_263_never_aliases_seven() {
    for selector in [-1, 0, 6, 8, 263] {
        let mut item = base_item();
        replace(
            &mut item,
            "DynamicData",
            dynamic(vec![
                Tag::integer("Index", selector),
                Tag::integer("Value", 1),
            ]),
        );
        reject(&one_item(item));
    }
    for count in [0, -1, 1_000_001, i32::MAX] {
        reject(&one_item(item_fields(&units("Row"), 4, count)));
    }
    assert_eq!(
        parse(&one_item(item_fields(&units("Row"), 4, 1_000_000)))
            .unwrap()
            .inventories[0]
            .items[0]
            .count,
        1_000_000
    );
    for fields in [
        vec![Tag::integer("Index", 7)],
        vec![Tag::integer("Index", 7), Tag::integer("Index", 7)],
        vec![
            Tag::integer("Index", 7),
            Tag::integer("Value", 1),
            empty_unknown(),
        ],
        vec![
            Tag::integer("Index", 7),
            Tag::integer("PrivateSentinelUnknown", 1),
        ],
        vec![
            Tag::integer("Index", 7),
            Tag::text("Value", "NameProperty", &units("PrivateSentinel")),
        ],
    ] {
        let mut item = base_item();
        replace(&mut item, "DynamicData", dynamic(fields));
        reject(&one_item(item));
    }
    let mut dyn_fields = vec![Tag::integer("Index", 7), Tag::integer("Value", 1)];
    dyn_fields[0].payload.push(0);
    let mut item = base_item();
    replace(&mut item, "DynamicData", dynamic(dyn_fields));
    reject(&one_item(item));
}

#[test]
fn scalar_item_widths_types_and_supported_neutral_state_are_exact() {
    for location in [-1, 256, 263, i32::MAX] {
        reject(&one_item(item_fields(&units("Row"), location, 1)));
    }
    for name in ["Location", "ItemOwnerLookupId"] {
        let mut item = base_item();
        field(&mut item, name).kind = "UInt32Property";
        reject(&one_item(item));
        let mut item = base_item();
        field(&mut item, name).payload.push(0);
        reject(&one_item(item));
    }
    let mut item = base_item();
    field(&mut item, "ItemStaticData").kind = "StrProperty";
    reject(&one_item(item));
    let mut item = base_item();
    replace(
        &mut item,
        "ItemStaticData",
        Tag::text("ItemStaticData", "NameProperty", &[]),
    );
    reject(&one_item(item));
    let mut item = base_item();
    replace(
        &mut item,
        "ItemGuid",
        Tag::text("ItemGuid", "StrProperty", &units("PrivateSentinelGuid")),
    );
    reject(&one_item(item));
    let mut item = base_item();
    field(&mut item, "ItemGuid").kind = "NameProperty";
    reject(&one_item(item));
    for owner in [0, 1, -2] {
        let mut item = base_item();
        replace(
            &mut item,
            "ItemOwnerLookupId",
            Tag::integer("ItemOwnerLookupId", owner),
        );
        reject(&one_item(item));
    }
}

#[test]
fn indices_and_both_guid_extension_kinds_are_rejected_at_nested_boundaries() {
    for at in 0..8 {
        let mut item = base_item();
        item[at].index = 1;
        reject(&one_item(item));
        let mut item = base_item();
        item[at].property_guid();
        reject(&one_item(item));
    }
    let shared = Tag::structure(
        "DynamicElement",
        "AuthoredDynamic",
        stream(&[Tag::integer("Index", 7), Tag::integer("Value", 1)]),
    );
    for mode in 0..3 {
        let mut changed = shared.clone();
        match mode {
            0 => changed.struct_guid(),
            1 => changed.property_guid(),
            _ => changed.index = 263,
        }
        let mut item = base_item();
        field(&mut item, "DynamicData").payload =
            [1_i32.to_le_bytes().to_vec(), changed.bytes()].concat();
        reject(&one_item(item));
    }
    let mut record = inventory_fields(&units("K"), &[]);
    field(&mut record, "AuthoredInventoryData").struct_guid();
    reject(&owner_fields(inventory_array(&[record])));
}

#[test]
fn canonical_strings_terminators_and_trailers_are_required() {
    let mut item = base_item();
    field(&mut item, "ItemStaticData").payload = wide_ascii("Row");
    reject(&one_item(item));
    for empty in [vec![1, 0, 0, 0, 0], vec![255, 255, 255, 255, 0, 0]] {
        let mut item = base_item();
        field(&mut item, "ItemGuid").payload = empty;
        reject(&one_item(item));
    }
    for malformed in [
        vec![2, 0, 0, 0, b'X', 1],
        vec![2, 0, 0, 0, 0, 0],
        vec![0, 0, 0, 0, 0],
        vec![254, 255, 255, 255, 255, 255, 0, 0],
    ] {
        let mut item = base_item();
        field(&mut item, "ItemStaticData").payload = malformed;
        reject(&one_item(item));
    }
    let mut record = inventory_fields(&units("K"), &[]);
    let p = field(&mut record, "AuthoredInventoryData");
    p.payload = noncanonical_terminator(p.payload.clone());
    reject(&owner_fields(inventory_array(&[record])));
    let (scalar, inventory) = layouts();
    let fields = complete();
    for bytes in [
        [noncanonical_terminator(stream(&fields)), vec![0; 4]].concat(),
        [component(&fields), vec![0]].concat(),
        [stream(&fields), vec![1, 0, 0, 0]].concat(),
        [stream(&fields), vec![1, 0, 0, 0], vec![0; 16]].concat(),
    ] {
        assert!(source_inventory_owner(&bytes, 37, &scalar, &inventory).is_err());
    }
    let mut shared = Tag::structure(
        "SlotElement",
        "AuthoredD01",
        noncanonical_terminator(stream(&base_item())),
    );
    let slots = Tag::array(
        "AuthoredSlots",
        "StructProperty",
        [1_i32.to_le_bytes().to_vec(), shared.bytes()].concat(),
    );
    let record = vec![
        Tag::text("SourceInventoryKey", "NameProperty", &units("K")),
        Tag::structure(
            "AuthoredInventoryData",
            "AuthoredInventoryState",
            stream(&[slots]),
        ),
    ];
    reject(&owner_fields(inventory_array(&[record])));
    shared.payload = stream(&base_item());
    shared.payload.push(0);
    let slots = Tag::array(
        "AuthoredSlots",
        "StructProperty",
        [1_i32.to_le_bytes().to_vec(), shared.bytes()].concat(),
    );
    let record = vec![
        Tag::text("SourceInventoryKey", "NameProperty", &units("K")),
        Tag::structure(
            "AuthoredInventoryData",
            "AuthoredInventoryState",
            stream(&[slots]),
        ),
    ];
    reject(&owner_fields(inventory_array(&[record])));
}

#[test]
fn noncanonical_nested_header_strings_are_rejected() {
    let mut fields = base_item();
    let raw = fields.remove(0).bytes();
    let name_end = string("ItemOwnerLookupId").len();
    let noncanonical = [wide_ascii("ItemOwnerLookupId"), raw[name_end..].to_vec()].concat();
    let mut body = noncanonical;
    body.extend(fields.iter().flat_map(Tag::bytes));
    body.extend(string("None"));
    let slots = Tag::array(
        "AuthoredSlots",
        "StructProperty",
        [
            1_i32.to_le_bytes().to_vec(),
            Tag::structure("SlotElement", "AuthoredD01", body).bytes(),
        ]
        .concat(),
    );
    let record = vec![
        Tag::text("SourceInventoryKey", "NameProperty", &units("K")),
        Tag::structure(
            "AuthoredInventoryData",
            "AuthoredInventoryState",
            stream(&[slots]),
        ),
    ];
    reject(&owner_fields(inventory_array(&[record])));
}

#[test]
fn transform_child_names_types_and_struct_identities_require_canonical_headers() {
    assert!(parse(&complete()).is_ok());
    for name in [
        "Translation",
        "Rotation",
        "Scale3D",
        "StructProperty",
        "Vector",
        "Quat",
    ] {
        let mut fields = complete();
        let payload = &mut field(&mut fields, "ActorTransform").payload;
        let narrow = string(name);
        let start = payload
            .windows(narrow.len())
            .position(|bytes| bytes == narrow)
            .unwrap();
        payload.splice(start..start + narrow.len(), wide_ascii(name));
        let error = reject(&fields);
        assert_eq!(error.field, "scalar.transform.header");
        assert_eq!(error.category, SemanticImportCategory::Header);
    }
}

#[test]
fn semantic_counts_fail_before_struct_array_framing() {
    for count in [-1_i32, 0, 9, 4097, i32::MAX] {
        let mut tag = inventory_array(&[]);
        tag.payload = count.to_le_bytes().to_vec();
        let e = reject(&owner_fields(tag));
        assert_eq!(e.field, "inventory.count");
        assert_eq!(e.category, SemanticImportCategory::Limit);
    }
    for count in [-1_i32, 513, 4097, i32::MAX] {
        let slots = Tag::array(
            "AuthoredSlots",
            "StructProperty",
            count.to_le_bytes().to_vec(),
        );
        let record = vec![
            Tag::text("SourceInventoryKey", "NameProperty", &units("K")),
            Tag::structure(
                "AuthoredInventoryData",
                "AuthoredInventoryState",
                stream(&[slots]),
            ),
        ];
        assert_eq!(
            reject(&owner_fields(inventory_array(&[record]))).field,
            "inventory.count"
        );
    }
    for count in [-1_i32, 0, 2, 4097] {
        let mut item = base_item();
        field(&mut item, "DynamicData").payload = count.to_le_bytes().to_vec();
        assert_eq!(reject(&one_item(item)).field, "inventory.count");
    }
}

#[test]
fn inventory_and_item_count_boundaries_are_accepted() {
    let records: Vec<_> = (0..8).map(|i| inventory_fields(&[65 + i], &[])).collect();
    assert_eq!(
        parse(&owner_fields(inventory_array(&records)))
            .unwrap()
            .inventories
            .len(),
        8
    );
    let records = vec![base_item(); 512];
    assert_eq!(
        parse(&owner_fields(inventory_array(&[inventory_fields(
            &units("K"),
            &records
        )])))
        .unwrap()
        .inventories[0]
            .items
            .len(),
        512
    );
    let records = vec![base_item(); 513];
    assert_eq!(
        reject(&owner_fields(inventory_array(&[inventory_fields(
            &units("K"),
            &records
        )])))
        .field,
        "inventory.count"
    );
}

#[test]
fn source_name_bounds_apply_to_nested_values_and_scalars() {
    let row = vec![0xd800; 1024];
    assert!(parse(&one_item(item_fields(&row, 1, 1))).is_ok());
    let row = vec![0xd800; 1025];
    reject(&one_item(item_fields(&row, 1, 1)));
    reject(&owner_fields(inventory_array(&[inventory_fields(
        &row,
        &[],
    )])));
    let mut fields = complete();
    replace(
        &mut fields,
        "AuthoredEnabled",
        scalar::scalar_array(
            "AuthoredEnabled",
            "EnabledElement",
            "AuthoredBoolRecord",
            Tag::boolean("SavedBool", true),
            &row,
        ),
    );
    reject(&fields);
    let mut fields = complete();
    replace(
        &mut fields,
        "ActorClassName",
        Tag::text("ActorClassName", "NameProperty", &row),
    );
    reject(&fields);
}

#[test]
fn layout_header_limits_include_the_terminator_and_admit_matching_source() {
    fn header_array(
        name: &'static str,
        element: &'static str,
        record: &str,
        fields: Vec<Tag>,
    ) -> Tag {
        let mut shared = Tag::structure(element, record, stream(&fields));
        shared.extras = [
            Tag::text("Unused", "NameProperty", &units(record)).payload,
            vec![0; 17],
        ]
        .concat();
        Tag::array(
            name,
            "StructProperty",
            [1_i32.to_le_bytes().to_vec(), shared.bytes()].concat(),
        )
    }
    for character in ['A', 'é'] {
        let record = character.to_string().repeat(1023);
        let scalar = CompiledC001ScalarLayout::compile(scalar_definition()).unwrap();
        let mut declaration = definition();
        declaration.inventories.record_type = units(&record);
        let inventory = CompiledC001InventoryLayout::compile(declaration.clone(), &scalar).unwrap();
        let fields = owner_fields(header_array(
            "AuthoredInventories",
            "InventoryElement",
            &record,
            inventory_fields(&units("K"), &[]),
        ));
        assert!(source_inventory_owner(&component(&fields), 37, &scalar, &inventory).is_ok());
        declaration.inventories.record_type.push(character as u16);
        assert!(CompiledC001InventoryLayout::compile(declaration.clone(), &scalar).is_err());
        let encoded = serde_json::to_string(&declaration).unwrap();
        assert!(serde_json::from_str::<C001InventoryLayoutV1>(&encoded).is_err());

        let mut declaration = scalar_definition();
        declaration.enabled.record_type = units(&record);
        let scalar = CompiledC001ScalarLayout::compile(declaration.clone()).unwrap();
        let inventory = CompiledC001InventoryLayout::compile(definition(), &scalar).unwrap();
        let mut fields = complete();
        replace(
            &mut fields,
            "AuthoredEnabled",
            header_array(
                "AuthoredEnabled",
                "EnabledElement",
                &record,
                vec![
                    Tag::text("VariableName", "NameProperty", &[0xd800, 70]),
                    Tag::boolean("SavedBool", true),
                ],
            ),
        );
        assert!(source_inventory_owner(&component(&fields), 37, &scalar, &inventory).is_ok());
        declaration.enabled.record_type.push(character as u16);
        assert!(CompiledC001ScalarLayout::compile(declaration).is_err());
    }
    for unit in [0, u16::MAX] {
        let (scalar, _) = layouts();
        let mut inventory = definition();
        inventory.inventories.record_type = vec![unit];
        assert!(CompiledC001InventoryLayout::compile(inventory, &scalar).is_err());
        let mut scalar = scalar_definition();
        scalar.enabled.record_type = vec![unit];
        assert!(CompiledC001ScalarLayout::compile(scalar).is_err());
    }
}

#[test]
fn textual_owner_policies_are_bounded_before_shared_value_decoding() {
    for kind in ["ByteProperty", "EnumProperty"] {
        for length in [1025, 65_535] {
            let mut fields = complete();
            fields.push(Tag {
                name: "OwnerResolvePolicy",
                kind,
                extras: [string("EStateRecorderOwnerResolvePolicy"), vec![0]].concat(),
                payload: string(&"x".repeat(length)),
                index: 0,
            });
            let error = reject(&fields);
            assert_eq!(error.field, "inventory.policy");
            assert_eq!(error.category, SemanticImportCategory::Limit);
        }
        let mut fields = complete();
        fields.push(Tag {
            name: "OwnerResolvePolicy",
            kind,
            extras: [string("EStateRecorderOwnerResolvePolicy"), vec![0]].concat(),
            payload: string(&"x".repeat(1024)),
            index: 0,
        });
        assert_eq!(
            reject(&fields).category,
            SemanticImportCategory::Unsupported
        );
    }
}

#[test]
fn existing_owner_restrictions_and_policy_encodings_are_retained() {
    for (saved, expected) in [
        (0, OwnerResolvePolicy::FindOnly),
        (1, OwnerResolvePolicy::RespawnOnly),
        (2, OwnerResolvePolicy::FindOrRespawn),
    ] {
        let mut fields = complete();
        fields.push(Tag {
            name: "OwnerResolvePolicy",
            kind: "ByteProperty",
            extras: [string("None"), vec![0]].concat(),
            payload: vec![saved],
            index: 0,
        });
        assert_eq!(parse(&fields).unwrap().effective_policy(), expected);
    }
    for name in [
        "FindOnly",
        "RespawnOnly",
        "FindOrRespawn",
        "EStateRecorderOwnerResolvePolicy::RespawnOnly",
    ] {
        let mut fields = complete();
        fields.push(Tag {
            name: "OwnerResolvePolicy",
            kind: "EnumProperty",
            extras: [string("EStateRecorderOwnerResolvePolicy"), vec![0]].concat(),
            payload: string(name),
            index: 0,
        });
        assert!(parse(&fields).is_ok());
    }
    for name in ["bWasMovedToSubLevel", "bSkipDefaultOwnerResolution"] {
        let mut fields = complete();
        fields.retain(|p| p.name != name);
        fields.push(Tag::boolean(name, true));
        reject(&fields);
    }
    for version in [0, 2, 4] {
        let mut fields = complete();
        replace(
            &mut fields,
            "RecordedVersion",
            Tag::integer("RecordedVersion", version),
        );
        reject(&fields);
    }
    let mut fields = complete();
    fields.push(Tag {
        name: "OwnerResolvePolicy",
        kind: "ByteProperty",
        extras: [string("None"), vec![0]].concat(),
        payload: vec![3],
        index: 0,
    });
    reject(&fields);
    let mut fields = complete();
    replace(
        &mut fields,
        "ActorTransform",
        scalar::transform(&icarus_bevy::native_world::NativeTransform {
            translation: [f32::NAN, 0.0, 0.0],
            ..Default::default()
        }),
    );
    reject(&fields);
    for name in [
        "AuthoredEnabled",
        "AuthoredUses",
        "ActorTransform",
        "RecordedVersion",
        "bWasMovedToSubLevel",
        "ActorClassName",
        "ActorPathName",
        "ObjectFName",
        "IcarusActorGUID",
    ] {
        let mut fields = complete();
        fields.retain(|p| p.name != name);
        reject(&fields);
    }
}

#[test]
fn layout_topology_collisions_and_bounded_capacity_are_checked() {
    let scalar = CompiledC001ScalarLayout::compile(scalar_definition()).unwrap();
    for root in [
        "ActorClassName",
        "ActorPathName",
        "ObjectFName",
        "IcarusActorGUID",
        "RecordedVersion",
        "bWasMovedToSubLevel",
        "ActorTransform",
        "bSkipDefaultOwnerResolution",
        "OwnerResolvePolicy",
        "AuthoredEnabled",
        "AuthoredUses",
    ] {
        let mut layout = definition();
        layout.inventories.field_name = units(root);
        assert!(CompiledC001InventoryLayout::compile(layout, &scalar).is_err());
    }
    let mut layout = definition();
    layout.inventory_key_field = layout.inventory_data.field_name.clone();
    assert!(CompiledC001InventoryLayout::compile(layout, &scalar).is_err());
    for at in 0..4 {
        let mut layout = definition();
        let shape = match at {
            0 => &mut layout.dynamic,
            1 => &mut layout.additional_stats,
            2 => &mut layout.alterations,
            _ => &mut layout.living_item_slots,
        };
        shape.field_name = units("Wrong");
        assert!(CompiledC001InventoryLayout::compile(layout, &scalar).is_err());
    }
    for name in [vec![], vec![65; 1025]] {
        let mut layout = definition();
        layout.inventories.field_name = name;
        assert!(CompiledC001InventoryLayout::compile(layout, &scalar).is_err());
    }
    let mut layout = definition();
    let mut name = Vec::with_capacity(2051);
    name.push(65);
    layout.inventories.field_name = name;
    assert!(CompiledC001InventoryLayout::compile(layout, &scalar).is_err());
    let mut layout = definition();
    layout.inventories.record_type = vec![0xd800; 1023];
    assert!(CompiledC001InventoryLayout::compile(layout, &scalar).is_ok());
    // Distinct scopes permit the same identity; they are never conflated.
    let mut layout = definition();
    layout.inventory_key_field = layout.slots.field_name.clone();
    assert!(CompiledC001InventoryLayout::compile(layout, &scalar).is_ok());
}

#[test]
fn layout_dtos_require_named_objects_with_no_duplicate_unknown_or_missing_fields() {
    let original = serde_json::to_value(definition()).unwrap();
    assert!(serde_json::from_value::<C001InventoryLayoutV1>(original.clone()).is_ok());
    for key in original.as_object().unwrap().keys() {
        let mut value = original.clone();
        value.as_object_mut().unwrap().remove(key);
        assert!(serde_json::from_value::<C001InventoryLayoutV1>(value).is_err());
    }
    for nested in [
        "inventories",
        "inventory_data",
        "slots",
        "dynamic",
        "additional_stats",
        "alterations",
        "living_item_slots",
    ] {
        let mut value = original.clone();
        value[nested] = serde_json::json!([]);
        assert!(serde_json::from_value::<C001InventoryLayoutV1>(value).is_err());
        let mut value = original.clone();
        value[nested]["PrivateSentinelUnknown"] = serde_json::json!([]);
        let e = serde_json::from_value::<C001InventoryLayoutV1>(value).unwrap_err();
        assert!(!e.to_string().contains("PrivateSentinel"));
    }
    assert!(
        serde_json::from_str::<StructArrayShapeV1>(
            r#"{"field_name":[65],"field_name":[66],"element_tag_name":[67],"record_type":[68]}"#
        )
        .is_err()
    );
    assert!(
        serde_json::from_str::<StructShapeV1>(
            r#"{"field_name":[65],"record_type":[68],"record_type":[69]}"#
        )
        .is_err()
    );
    let text = serde_json::to_string(&definition()).unwrap();
    let duplicated = format!(
        "{{\"inventories\":{},{}",
        serde_json::to_string(&definition().inventories).unwrap(),
        &text[1..]
    );
    assert!(serde_json::from_str::<C001InventoryLayoutV1>(&duplicated).is_err());
    for name in [
        serde_json::json!([]),
        serde_json::json!(vec![65; 1025]),
        serde_json::json!("PrivateSentinel"),
    ] {
        let mut value = original.clone();
        value["inventory_key_field"] = name;
        let e = serde_json::from_value::<C001InventoryLayoutV1>(value).unwrap_err();
        assert!(!e.to_string().contains("PrivateSentinel"));
    }
    let mut value = original;
    value["PrivateSentinelUnknown"] = serde_json::json!([]);
    assert!(serde_json::from_value::<C001InventoryLayoutV1>(value).is_err());
    assert!(serde_json::from_str::<C001InventoryLayoutV1>("[]").is_err());
}

#[test]
fn source_and_definition_debug_and_errors_redact_all_values() {
    let parsed = parse(&complete()).unwrap();
    for debug in [
        format!("{parsed:?}"),
        format!("{:?}", parsed.inventories),
        format!("{:?}", parsed.inventories[0].items),
        format!("{:?}", definition()),
        format!("{:?}", layouts().1),
    ] {
        for sentinel in [
            "AuthoredCrate",
            "InventoryA",
            "AuthoredSecondRow",
            "55296",
            "901",
            "AuthoredInventories",
        ] {
            assert!(!debug.contains(sentinel));
        }
    }
    let mut item = base_item();
    replace(
        &mut item,
        "ItemStaticData",
        Tag::text(
            "ItemStaticData",
            "StrProperty",
            &units("PrivateSentinelSource"),
        ),
    );
    let error = reject(&one_item(item));
    assert!(!format!("{error:?} {error}").contains("PrivateSentinel"));
    let mut layout = definition();
    layout.dynamic.field_name = units("PrivateSentinelDeclaration");
    let e = CompiledC001InventoryLayout::compile(layout, &layouts().0).unwrap_err();
    assert!(!format!("{e:?} {e}").contains("PrivateSentinel"));
}

#[test]
fn changing_scalar_declarations_after_inventory_compilation_fails_closed() {
    let (_, inventory) = layouts();
    let mut definition = scalar_definition();
    definition.enabled.array_name = units("OtherArray");
    let scalar = CompiledC001ScalarLayout::compile(definition).unwrap();
    let e = source_inventory_owner(&component(&complete()), 37, &scalar, &inventory).unwrap_err();
    assert_eq!(e.field, "inventory.scalar_layout");
}

#[test]
fn previous_scalar_and_owner_adapters_reject_inventory_bearing_components() {
    use icarus_bevy::{
        native_c001_owners::{
            C001OwnerAdapter, C001OwnerBindingsV1, CompiledC001OwnerBindings,
            prepare_c001_owner_import,
        },
        native_c001_scalar_owners::prepare_c001_scalar_owner_import,
    };
    let mut fixture = scalar::fixture();
    for owner in &mut fixture.owners {
        owner.push(inventory_array(&[inventory_fields(&units("K"), &[])]));
    }
    fixture.rebuild_source();
    let e = prepare_c001_scalar_owner_import(
        &fixture.candidate(),
        fixture.base.clone(),
        &fixture.bindings,
        &fixture.contracts,
    )
    .unwrap_err();
    assert_eq!(e.field, "c001_scalar_owners.source");
    assert_eq!(e.category, SemanticImportCategory::Unsupported);
    // Remove the scalar arrays to isolate inventory rejection by the older owner profile.
    for owner in &mut fixture.owners {
        owner.retain(|p| p.name != "AuthoredEnabled" && p.name != "AuthoredUses");
    }
    fixture.rebuild_source();
    let b = fixture.bindings.definition();
    let bindings = CompiledC001OwnerBindings::compile(C001OwnerBindingsV1 {
        format: 1,
        adapter: C001OwnerAdapter::C001AuthoredOwnersV1,
        expected_payload_sha256: b.expected_payload_sha256.clone(),
        native_catalog_id: b.native_catalog_id.clone(),
        native_lifecycle_id: b.native_lifecycle_id.clone(),
        source_map: b.source_map.clone(),
        native_map: b.native_map.clone(),
        coordinates: b.coordinates,
        owners: b.owners.iter().map(|b| b.owner.clone()).collect(),
    })
    .unwrap();
    let e = prepare_c001_owner_import(&fixture.candidate(), fixture.base.clone(), &bindings)
        .unwrap_err();
    assert_eq!(e.field, "c001_owners.source");
    assert_eq!(e.category, SemanticImportCategory::Unsupported);
}

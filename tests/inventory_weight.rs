#[path = "pak_writer/mod.rs"]
mod pak_writer;

use icarus_bevy::{
    data_pak::DataTables,
    experimental::{
        inventory_weight::{
            ANY_BAG_QUERY_ROW, AUTHORITY_LOCAL_ROLE, CARRY_WEIGHT_STAT_SCALE_BITS,
            DEFAULT_CDO_CURRENT_WEIGHT, DYNAMIC_PROP_ASSOCIATED_INVENTORY_ID,
            DYNAMIC_PROP_FILLABLE_UNITS, DYNAMIC_PROP_ITEMABLE_STACK, InventoryWeightState,
            ItemWeightInput, RecalculateWeightOutcome, SET_ITEM_SUPPRESSION_FLAGS,
            StatContainerOwnerSource, WEIGHABLE_RESOURCE_CONTAINER_QUERY_ROW, WEIGHT_STAT_INDEX,
            initial_open_current_weight, item_weight, recalculate_inventory_weight,
            recalculate_slot_weight, recalculate_weight_owner_eligible,
            record_container_wants_tick, resolve_stat_container_owner,
            restore_container_manager_inventories, unreferenced_container_indices,
        },
        property_tree::{PropertyTree, TreeLimits},
        prospect_info::{
            SavedContainerManager, SavedInventoryContainer, SavedItem, saved_container_manager,
        },
        tagged::TagProfile,
        typed_value::TypedTree,
    },
    network::container_inventories,
};
use serde_json::json;

#[test]
fn constructor_keeps_the_seed_clean_and_ticks_do_not_calculate() {
    for seed in [0, -17, i32::MIN, i32::MAX] {
        let mut state = InventoryWeightState::new(seed);
        assert_eq!(state.cached_weight(), seed);
        assert!(!state.is_dirty());
        for role in [None, Some(0), Some(3), Some(4), Some(u8::MAX)] {
            assert_eq!(
                state.tick(role, || panic!("clean cache recalculated")),
                None
            );
            assert_eq!(state.cached_weight(), seed);
            assert!(!state.is_dirty());
        }
    }
}

#[test]
fn missing_owner_and_low_roles_preserve_pending_recalculation_and_cache() {
    let mut state = InventoryWeightState::new(-123);
    state.mark_dirty();
    state.mark_dirty();
    for role in [None, Some(0), Some(1), Some(2)] {
        assert_eq!(
            state.tick(role, || panic!("ineligible tick calculated")),
            None
        );
        assert!(state.is_dirty());
        assert_eq!(state.cached_weight(), -123);
    }
    assert_eq!(state.tick(Some(3), || 19), Some(19));
    assert_eq!(state.cached_weight(), 19);
    assert!(!state.is_dirty());
}

#[test]
fn eligible_roles_recalculate_once_per_dirty_period() {
    for role in [3, 4, 128, u8::MAX] {
        let mut state = InventoryWeightState::new(91);
        state.mark_dirty();
        let mut calls = 0;
        // Moving this String into the closure also exercises an FnOnce-only
        // calculation; no repeat-call capability is required.
        let signed = String::from("-47");
        assert_eq!(
            state.tick(Some(role), move || {
                let weight = signed.parse().unwrap();
                drop(signed);
                weight
            }),
            Some(-47)
        );
        assert_eq!(state.cached_weight(), -47);
        assert!(!state.is_dirty());
        for _ in 0..2 {
            assert_eq!(
                state.tick(Some(role), || panic!("clean tick calculated")),
                None
            );
        }
        state.mark_dirty();
        assert_eq!(
            state.tick(Some(role), || {
                calls += 1;
                0
            }),
            Some(0)
        );
        assert_eq!(calls, 1);
        assert_eq!(state.cached_weight(), 0);
        assert!(!state.is_dirty());
    }
}

#[test]
fn stat_container_owner_resolution_and_container_manager_eligibility() {
    assert_eq!(AUTHORITY_LOCAL_ROLE, 3);
    assert_eq!(WEIGHT_STAT_INDEX, 0x5c8);
    assert_eq!(WEIGHABLE_RESOURCE_CONTAINER_QUERY_ROW, 0x16c);
    assert_eq!(ANY_BAG_QUERY_ROW, 0xee);
    assert_eq!(DYNAMIC_PROP_FILLABLE_UNITS, 10);
    assert_eq!(DYNAMIC_PROP_ITEMABLE_STACK, 7);
    assert_eq!(DYNAMIC_PROP_ASSOCIATED_INVENTORY_ID, 13);
    assert_eq!(CARRY_WEIGHT_STAT_SCALE_BITS, 0xbc23_d70a);
    assert_eq!(SET_ITEM_SUPPRESSION_FLAGS, 0x0101);
    assert_eq!(DEFAULT_CDO_CURRENT_WEIGHT, 0);

    assert_eq!(
        resolve_stat_container_owner(true, false),
        Some(StatContainerOwnerSource::DirectActorOwner)
    );
    assert_eq!(
        resolve_stat_container_owner(true, true),
        Some(StatContainerOwnerSource::DirectActorOwner)
    );
    assert_eq!(
        resolve_stat_container_owner(false, true),
        Some(StatContainerOwnerSource::OwningInventoryActorOwner)
    );
    assert_eq!(resolve_stat_container_owner(false, false), None);

    assert!(!recalculate_weight_owner_eligible(false, true, true));
    assert!(recalculate_weight_owner_eligible(true, true, false));
    assert!(recalculate_weight_owner_eligible(true, false, true));
    assert!(!recalculate_weight_owner_eligible(true, false, false));
}

#[test]
fn item_weight_and_recalculate_slot_weight_honor_stack_gate_fillable_and_cvttss2si() {
    let empty = ItemWeightInput {
        has_valid_itemable: false,
        weight_stat: 500,
        matches_weighable_resource_container: true,
        fillable_units: Some(200),
        itemable_stack: Some(4),
        matching_carry_weight_stat_pct: 0,
    };
    assert_eq!(item_weight(&empty), 0);
    assert_eq!(recalculate_slot_weight(&empty), 0);

    // GetItemWeight defaults missing property 7 (ItemableStack) to 1, whereas
    // RecalculateWeight skips slots whose GetItemPropertyValue(7) is invalid.
    let no_stack = ItemWeightInput {
        has_valid_itemable: true,
        weight_stat: 250,
        matches_weighable_resource_container: false,
        fillable_units: None,
        itemable_stack: None,
        matching_carry_weight_stat_pct: 0,
    };
    assert_eq!(item_weight(&no_stack), 250);
    assert_eq!(recalculate_slot_weight(&no_stack), 0);

    // Fillable units apply only when Any_Weighable_Resource_Container matches.
    let non_weighable_zero = ItemWeightInput {
        has_valid_itemable: true,
        weight_stat: 0,
        matches_weighable_resource_container: false,
        fillable_units: Some(500),
        itemable_stack: Some(2),
        matching_carry_weight_stat_pct: 0,
    };
    assert_eq!(item_weight(&non_weighable_zero), 0);

    let weighable = ItemWeightInput {
        matches_weighable_resource_container: true,
        ..non_weighable_zero
    };
    assert_eq!(item_weight(&weighable), 1_000);
    assert_eq!(recalculate_slot_weight(&weighable), 1_000);

    // Carry-weight reduction (-50% -> delta +150, 300 - 150 = 150 * 4 = 600)
    // and >= 1 unit floor (-100% -> 1 * 3 = 3).
    let reduced = ItemWeightInput {
        has_valid_itemable: true,
        weight_stat: 300,
        matches_weighable_resource_container: false,
        fillable_units: None,
        itemable_stack: Some(4),
        matching_carry_weight_stat_pct: -50,
    };
    assert_eq!(item_weight(&reduced), 600);

    let floored = ItemWeightInput {
        itemable_stack: Some(3),
        matching_carry_weight_stat_pct: -100,
        ..reduced
    };
    assert_eq!(item_weight(&floored), 3);

    let increased = ItemWeightInput {
        weight_stat: 100,
        itemable_stack: Some(2),
        matching_carry_weight_stat_pct: 25,
        ..reduced
    };
    assert_eq!(item_weight(&increased), 250);

    // Extreme float product exercises the CVTTSS2SI integer-indefinite path without panicking.
    let overflow = ItemWeightInput {
        weight_stat: i32::MAX,
        itemable_stack: Some(1),
        matching_carry_weight_stat_pct: i32::MIN,
        ..reduced
    };
    let _ = item_weight(&overflow);
}

#[test]
fn recalculate_inventory_weight_sums_slots_child_bags_and_propagates_dirty() {
    let slots = [
        ItemWeightInput {
            has_valid_itemable: true,
            weight_stat: 100,
            matches_weighable_resource_container: false,
            fillable_units: None,
            itemable_stack: Some(3),
            matching_carry_weight_stat_pct: 0,
        },
        ItemWeightInput {
            has_valid_itemable: true,
            weight_stat: 80,
            matches_weighable_resource_container: false,
            fillable_units: None,
            itemable_stack: None,
            matching_carry_weight_stat_pct: 0,
        },
    ];
    assert_eq!(
        recalculate_inventory_weight(0, false, true, false, true, &slots, &[45]),
        None
    );
    assert_eq!(
        recalculate_inventory_weight(0, true, true, false, true, &slots, &[45]),
        Some(RecalculateWeightOutcome {
            current_weight: 345,
            weight_changed: true,
            mark_owning_inventory_dirty: true,
        })
    );
    assert_eq!(
        recalculate_inventory_weight(345, true, true, false, true, &slots, &[45]),
        Some(RecalculateWeightOutcome {
            current_weight: 345,
            weight_changed: false,
            mark_owning_inventory_dirty: false,
        })
    );
}

#[test]
fn c016_restore_enablement_stitching_and_initial_open_omission() {
    assert!(!record_container_wants_tick(false));
    assert!(record_container_wants_tick(true));
    assert_eq!(initial_open_current_weight(0), None);
    assert_eq!(initial_open_current_weight(750), Some(750));

    assert_eq!(
        unreferenced_container_indices(5, &[1, 3, -1, 99]),
        vec![0, 2, 4]
    );

    let mut restored = restore_container_manager_inventories(
        3,
        Some(AUTHORITY_LOCAL_ROLE),
        &[(-1, true), (0, false), (2, true), (3, true)],
    );
    assert_eq!(restored.len(), 3);
    assert!(restored[1].is_none());

    let unticked = restored[0].as_mut().expect("slot 0 restored");
    assert!(unticked.is_registered());
    assert!(!unticked.is_tick_enabled());
    assert!(unticked.weight_state().is_dirty());
    assert_eq!(
        unticked.tick_if_enabled(|| panic!("unticked container recalculated")),
        None
    );
    assert_eq!(unticked.initial_open_weight(), None);

    let ticked = restored[2].as_mut().expect("slot 2 restored");
    assert!(ticked.is_registered());
    assert!(ticked.is_tick_enabled());
    assert!(ticked.weight_state().is_dirty());
    assert_eq!(ticked.tick_if_enabled(|| 1_750), Some(1_750));
    assert!(!ticked.weight_state().is_dirty());
    assert_eq!(ticked.initial_open_weight(), Some(1_750));
    assert_eq!(
        ticked.tick_if_enabled(|| panic!("clean container recalculated")),
        None
    );

    // A ticked container whose recalculated weight equals the CDO (0) still omits handle 4.
    let mut ticked_zero =
        restore_container_manager_inventories(1, Some(AUTHORITY_LOCAL_ROLE), &[(0, true)]);
    let zero = ticked_zero[0].as_mut().unwrap();
    assert_eq!(zero.tick_if_enabled(|| 0), Some(0));
    assert_eq!(zero.initial_open_weight(), None);
}

#[test]
fn ticked_containers_recalculate_nondefault_current_weight_and_fail_closed() {
    let units = |text: &str| text.encode_utf16().collect::<Vec<u16>>();
    let fstring = |value: &str| -> Vec<u8> {
        [
            i32::try_from(value.len() + 1)
                .unwrap()
                .to_le_bytes()
                .as_slice(),
            value.as_bytes(),
            &[0],
        ]
        .concat()
    };
    let tag = |name: &str, kind: &str, extras: &[u8], payload: &[u8]| -> Vec<u8> {
        [
            fstring(name).as_slice(),
            &fstring(kind),
            &i32::try_from(payload.len()).unwrap().to_le_bytes(),
            &0_i32.to_le_bytes(),
            extras,
            payload,
        ]
        .concat()
    };
    let end = || fstring("None");

    // Verify bInventoryWantsTick parses true/false and rejects non-bool tags.
    let element_true = [
        tag("InventoryIndex", "IntProperty", &[0], &0_i32.to_le_bytes()),
        tag("InventoryInfo", "NameProperty", &[0], &fstring("Hands")),
        tag("bInventoryWantsTick", "BoolProperty", &[1, 0], &[]),
        end(),
    ]
    .concat();
    let shared_true = tag(
        "SavedInventoryContainers",
        "StructProperty",
        &[fstring("SavedInventoryContainerData"), vec![0; 17]].concat(),
        &element_true,
    );
    let stream_true = [
        tag("NumInventories", "IntProperty", &[0], &1_i32.to_le_bytes()),
        tag(
            "SavedInventoryContainers",
            "ArrayProperty",
            &[fstring("StructProperty"), vec![0]].concat(),
            &[1_i32.to_le_bytes().as_slice(), &shared_true].concat(),
        ),
        end(),
    ]
    .concat();
    let parsed = PropertyTree::parse(
        &stream_true,
        TagProfile::MemoryArchive522,
        TreeLimits::default(),
    )
    .unwrap();
    let typed = saved_container_manager(&TypedTree::from_tree(&parsed).unwrap().0).unwrap();
    assert!(typed.containers[0].wants_tick);

    let element_bad = [
        tag("InventoryIndex", "IntProperty", &[0], &0_i32.to_le_bytes()),
        tag(
            "bInventoryWantsTick",
            "IntProperty",
            &[0],
            &1_i32.to_le_bytes(),
        ),
        end(),
    ]
    .concat();
    let shared_bad = tag(
        "SavedInventoryContainers",
        "StructProperty",
        &[fstring("SavedInventoryContainerData"), vec![0; 17]].concat(),
        &element_bad,
    );
    let stream_bad = [
        tag("NumInventories", "IntProperty", &[0], &1_i32.to_le_bytes()),
        tag(
            "SavedInventoryContainers",
            "ArrayProperty",
            &[fstring("StructProperty"), vec![0]].concat(),
            &[1_i32.to_le_bytes().as_slice(), &shared_bad].concat(),
        ),
        end(),
    ]
    .concat();
    let parsed_bad = PropertyTree::parse(
        &stream_bad,
        TagProfile::MemoryArchive522,
        TreeLimits::default(),
    )
    .unwrap();
    assert!(saved_container_manager(&TypedTree::from_tree(&parsed_bad).unwrap().0).is_err());

    let root = tempfile::TempDir::new().unwrap();
    let pak = root.path().join("data.pak");
    let tag_queries = json!({"Rows": [
        {"Name": "Q0"},
        {"Name": "Any"},
        {"Name": "Fists"},
        {
            "Name": "Any_Weighable_Resource_Container",
            "Query": {
                "TagDictionary": [{"TagName": "Item.Fillable.Water"}],
                "QueryTokenStream": [0, 1, 1, 1, 0]
            }
        }
    ]});
    let items_static = json!({"Rows": [
        {"Name": "Wood", "Itemable": {"RowName": "Item_Wood"}},
        {
            "Name": "Canteen",
            "Itemable": {"RowName": "Item_Canteen"},
            "Manual_Tags": {"GameplayTags": [{"TagName": "Item.Fillable.Water.NoDrink"}]}
        },
        {"Name": "Feather", "Itemable": {"RowName": "Item_Feather"}},
        {
            "Name": "WeightedStat",
            "Itemable": {"RowName": "Item_Wood"},
            "AdditionalStats": {"(Value=\"BaseWeight_+\")": 10}
        }
    ]});
    let itemable = json!({"Rows": [
        {"Name": "Item_Wood", "Weight": 100},
        {"Name": "Item_Canteen", "Weight": 50},
        {"Name": "Item_Feather"}
    ]});
    std::fs::write(
        &pak,
        pak_writer::official_pak(&[
            (
                "Tags/D_TagQueries.json",
                serde_json::to_vec(&tag_queries).unwrap(),
            ),
            (
                "Items/D_ItemsStatic.json",
                serde_json::to_vec(&items_static).unwrap(),
            ),
            (
                "Traits/D_Itemable.json",
                serde_json::to_vec(&itemable).unwrap(),
            ),
        ]),
    )
    .unwrap();
    let mut tables = DataTables::open(&pak).unwrap();

    let item = |location: i32, name: &str, dynamic: &[(i32, i32)]| SavedItem {
        location,
        row_name: units(name),
        dynamic: dynamic.to_vec(),
        stats: Vec::new(),
        guid: Vec::new(),
        owner_lookup_id: -1,
        alterations: Vec::new(),
        living_slots: 0,
    };

    let saved = SavedContainerManager {
        count: 3,
        containers: vec![
            // Container 0: wants_tick == false -> CurrentWeight stays CDO (None).
            SavedInventoryContainer {
                index: 0,
                info: units("Hands"),
                wants_tick: false,
                items: vec![item(0, "Wood", &[(7, 4)])],
            },
            // Container 1: wants_tick == true -> 100*3 + (50+200)*2 + 0 (missing prop 7) = 800.
            SavedInventoryContainer {
                index: 1,
                info: units("Hands"),
                wants_tick: true,
                items: vec![
                    item(0, "Wood", &[(7, 3)]),
                    item(1, "Canteen", &[(10, 200), (7, 2)]),
                    item(2, "Wood", &[(12, 999)]),
                ],
            },
            // Container 2: wants_tick == true, zero weight -> CurrentWeight == 0 omitted (None).
            SavedInventoryContainer {
                index: 2,
                info: units("Hands"),
                wants_tick: true,
                items: vec![item(0, "Feather", &[(7, 5)])],
            },
        ],
    };
    let mapped = container_inventories(&mut tables, &saved).unwrap();
    assert_eq!(mapped[0].current_weight, None);
    assert_eq!(mapped[1].current_weight, Some(800));
    assert_eq!(mapped[2].current_weight, None);

    // Unsupported ticked container items fail closed instead of guessing.
    let mut custom_stat = item(0, "Wood", &[(7, 1)]);
    custom_stat.stats.push((-1, units("Potency"), 3));
    for unsupported in [
        custom_stat,
        item(0, "Wood", &[(7, 1), (13, 0)]),
        item(0, "WeightedStat", &[(7, 1)]),
    ] {
        let bad = SavedContainerManager {
            count: 1,
            containers: vec![SavedInventoryContainer {
                index: 0,
                info: units("Hands"),
                wants_tick: true,
                items: vec![unsupported],
            }],
        };
        assert!(container_inventories(&mut tables, &bad).is_err());
    }
}

//! A returning pawn's restored stats (F-983 to F-987) on authored tables.
#[path = "pak_writer/mod.rs"]
mod pak_writer;

use std::collections::{BTreeMap, BTreeSet};

use icarus_bevy::experimental::{
    inventory_restore::{InventoryRestoreTables, InventoryRole, ItemInfo},
    pawn_open::{InventorySlot, SlotItem},
    rep_layout::StatPairRepState,
    stat_restore::{
        ItemStatInfo, PawnStatInputs, SetBonus, StatRestoreTables, restore_stats,
        scale_by_effectiveness,
    },
    tag_query::TagQuery,
};
use serde_json::json;

fn grants(pairs: &[(&str, i32)]) -> BTreeMap<String, i32> {
    pairs
        .iter()
        .map(|(stat, value)| ((*stat).to_owned(), *value))
        .collect()
}

#[test]
fn modifier_grants_scale_up_in_magnitude() {
    // 169% effectiveness (final 69): 8.45 and 16.9 round away from zero.
    assert_eq!(scale_by_effectiveness(-5, 69), -9);
    assert_eq!(scale_by_effectiveness(10, 69), 17);
    // Full effectiveness keeps the grant, whatever single precision does.
    for value in [-70, -5, 1, 3, 7, 30, 1000] {
        assert_eq!(scale_by_effectiveness(value, 0), value);
    }
    assert_eq!(scale_by_effectiveness(0, 69), 0);
    // An exact product does not round up.
    assert_eq!(scale_by_effectiveness(10, 50), 15);
    assert_eq!(scale_by_effectiveness(-10, 50), -15);
}

/// Stat rows: every stat the authored sources name, all replicated but the
/// first.
const STATS: [&str; 8] = [
    "BaseWeightCapacity_+",
    "WeightCapacity_+",
    "BaseArmour_+",
    "BaseRegen_+",
    "BaseSpeed_+",
    "IsOverencumbered_?",
    "IsCarryingTool_?",
    "BaseDuration_+",
];

/// Items: 0 a tool, 1 a set helmet linking an inventory, 2 a set chest, 3 a
/// module, 4 an attachment.
fn tables() -> (StatRestoreTables, InventoryRestoreTables) {
    let item = |tags: &[&str], equipped: &[(&str, i32)]| ItemInfo {
        tags: tags
            .iter()
            .map(|tag| (*tag).to_owned())
            .collect::<BTreeSet<_>>(),
        grants: grants(equipped),
        ..ItemInfo::default()
    };
    let inventory = InventoryRestoreTables {
        items: vec![
            item(&["Item", "Item.Tool"], &[]),
            item(&["Item"], &[("BaseArmour_+", 5)]),
            item(&["Item"], &[("BaseArmour_+", 7)]),
            item(&["Item"], &[("BaseSpeed_+", 10)]),
            item(&["Item"], &[]),
        ],
        ..InventoryRestoreTables::default()
    };
    let set = |name: &str| ItemStatInfo {
        armour_set: Some(name.to_owned()),
        ..ItemStatInfo::default()
    };
    let stats = StatRestoreTables {
        names: STATS.iter().map(|name| (*name).to_owned()).collect(),
        replicated: STATS
            .iter()
            .map(|name| !name.starts_with("BaseWeight"))
            .collect(),
        starting: grants(&[("BaseWeightCapacity_+", 10), ("BaseRegen_+", 1)]),
        items: vec![
            ItemStatInfo::default(),
            set("Plate"),
            set("Plate"),
            ItemStatInfo::default(),
            ItemStatInfo {
                attachment: grants(&[("BaseDuration_+", -20)]),
                ..ItemStatInfo::default()
            },
        ],
        set_bonuses: BTreeMap::from([(
            "Plate".to_owned(),
            vec![
                SetBonus {
                    required: 2,
                    grants: grants(&[("BaseRegen_+", 30)]),
                },
                SetBonus {
                    required: 3,
                    grants: grants(&[("BaseRegen_+", 1000)]),
                },
            ],
        )]),
        modifier_grants: vec![
            grants(&[("BaseRegen_+", 10)]),
            grants(&[("IsOverencumbered_?", 1), ("BaseSpeed_+", -70)]),
        ],
        overburdened: 1,
        carrying: vec![(
            TagQuery::from_row(&json!({"Query": {"QueryTokenStream": [0, 1, 1, 1, 0],
                "TagDictionary": [{"TagName": "Item.Tool"}]}}))
            .unwrap(),
            "IsCarryingTool_?".to_owned(),
        )],
    };
    (stats, inventory)
}

fn slots(items: &[(u16, &[(u32, i32)])]) -> Vec<InventorySlot> {
    items
        .iter()
        .zip(0..)
        .map(|((row, dynamic), index)| InventorySlot {
            item: Some(SlotItem {
                dynamic: dynamic.to_vec(),
                ..SlotItem::new(*row).unwrap()
            }),
            ..InventorySlot::empty(0, index)
        })
        .collect()
}

fn value(items: &[StatPairRepState], stat: &str) -> i32 {
    let row = STATS.iter().position(|name| *name == stat).unwrap();
    items
        .iter()
        .find(|pair| pair.stat as usize == row)
        .map_or(0, |pair| pair.value)
}

#[test]
fn restored_stats_sum_gear_sets_attachments_modifiers_and_carrying() {
    let (stats, inventory) = tables();
    // A stack of two tools counts once per inventory; the helmet links C016
    // inventory 4, which holds the attachment twice.
    let quickbar = slots(&[(0, &[(7, 2)]), (4, &[])]);
    let backpack = slots(&[(0, &[(7, 1)])]);
    let equipment = slots(&[(1, &[(13, 4)]), (2, &[(13, -1)])]);
    let upgrade = slots(&[(3, &[])]);
    let linked = BTreeMap::from([(4, vec![4, 4])]);
    let inputs = |weight| PawnStatInputs {
        inventories: vec![
            (InventoryRole::Other, quickbar.as_slice()),
            (InventoryRole::Backpack, backpack.as_slice()),
            (InventoryRole::Equipment, equipment.as_slice()),
            (InventoryRole::Upgrade, upgrade.as_slice()),
        ],
        linked: &linked,
        modifiers: vec![(0, 69)],
        weight,
    };
    let restored = restore_stats(&stats, &inventory, &inputs(10_000)).unwrap();
    assert!(!restored.overburdened);
    let items = &restored.items;
    // The unreplicated base stays out; WeightCapacity_+ passes it through.
    assert_eq!(value(items, "BaseWeightCapacity_+"), 0);
    assert_eq!(value(items, "WeightCapacity_+"), 10);
    assert_eq!(value(items, "BaseArmour_+"), 12);
    // Starting 1, the two-piece set bonus (not the three-piece one), the
    // modifier's 10 at 169%.
    assert_eq!(value(items, "BaseRegen_+"), 1 + 30 + 17);
    // Only Equipment and Upgrade items grant.
    assert_eq!(value(items, "BaseSpeed_+"), 10);
    // Attachments only in the linked inventory, each counted.
    assert_eq!(value(items, "BaseDuration_+"), -40);
    // One per tool item, in every inventory.
    assert_eq!(value(items, "IsCarryingTool_?"), 2);
    assert_eq!(value(items, "IsOverencumbered_?"), 0);
    // Rows in order, zeros left out.
    assert!(items.windows(2).all(|pair| pair[0].stat < pair[1].stat));
    assert!(items.iter().all(|pair| pair.value != 0));

    // Over the capacity times 1000, Overburdened's grants apply.
    let heavy = restore_stats(&stats, &inventory, &inputs(10_001)).unwrap();
    assert!(heavy.overburdened);
    assert_eq!(value(&heavy.items, "IsOverencumbered_?"), 1);
    assert_eq!(value(&heavy.items, "BaseSpeed_+"), -60);
}

#[test]
fn a_grant_the_stat_table_lacks_fails() {
    let (mut stats, inventory) = tables();
    stats.starting.insert("NoSuchStat_+".to_owned(), 1);
    let linked = BTreeMap::new();
    let inputs = PawnStatInputs {
        inventories: Vec::new(),
        linked: &linked,
        modifiers: Vec::new(),
        weight: 0,
    };
    assert!(restore_stats(&stats, &inventory, &inputs).is_err());
}

/// The builder reads the authored data.pak shapes: stat rows and their
/// replication flag, starting grants, armour sets through D_Armour, an
/// attachment's D_Alterations stats, set bonuses, modifier grants and the
/// carrying queries.
#[test]
fn the_tables_build_from_data_pak_rows() {
    use icarus_bevy::{data_pak::DataTables, network::stat_restore_tables};
    let json = |value: serde_json::Value| serde_json::to_vec(&value).unwrap();
    let query = |name: &str| {
        json!({"Name": name, "Query": {"QueryTokenStream": [0, 1, 1, 1, 0],
        "TagDictionary": [{"TagName": "Item.Tool"}]}})
    };
    let tables = [
        (
            "Stats/D_Stats.json",
            json(json!({"Rows": [
                {"Name": "BaseArmour_+", "bIsReplicated": true},
                {"Name": "Hidden_+"},
            ]})),
        ),
        (
            "Stats/D_CharacterStartingStats.json",
            json(json!({"Rows": [
                {"Name": "Base_Stats", "StatsGranted": {"(Value=\"BaseArmour_+\")": 3}},
            ]})),
        ),
        (
            "Items/D_ItemsStatic.json",
            json(json!({"Rows": [
                {"Name": "Helmet", "Armour": {"RowName": "Plate_Head"}},
                {"Name": "Plating", "Attachments": {"RowName": "Tough_1"}},
                {"Name": "Ore"},
            ]})),
        ),
        (
            "Armour/D_Armour.json",
            json(json!({"Rows": [
                {"Name": "Plate_Head", "ArmourSet": {"RowName": "Plate"}},
            ]})),
        ),
        (
            "Armour/D_ArmourSets.json",
            json(json!({"Rows": [
                {"Name": "Plate", "SetBonus": [{"DataTableName": "D_ArmourSetBonus", "RowName": "Plate_2"}]},
            ]})),
        ),
        (
            "Armour/D_ArmourSetBonus.json",
            json(json!({"Rows": [
                {"Name": "Plate_2", "RequiredGear": 2, "StatsGranted": {"(Value=\"BaseArmour_+\")": 30}},
            ]})),
        ),
        (
            "Alterations/D_Alterations.json",
            json(json!({"Rows": [
                {"Name": "Tough_1", "Stats": {"(Value=\"BaseArmour_+\")": 4}},
            ]})),
        ),
        (
            "Modifiers/D_ModifierStates.json",
            json(json!({"Rows": [
                {"Name": "Fed", "GrantedStats": {"(Value=\"BaseArmour_+\")": 1}},
                {"Name": "Overburdened", "GrantedStats": {"(Value=\"BaseArmour_+\")": -1}},
            ]})),
        ),
        (
            "Tags/D_TagQueries.json",
            json(json!({"Rows": [
                query("Any_Tool"), query("Any_AnimalCorpse"), query("Any_Radar"),
                query("Any_Exotic"), query("Any_Extractor"),
            ]})),
        ),
    ];
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("data.pak");
    std::fs::write(&path, pak_writer::official_pak(&tables)).unwrap();
    let mut data = DataTables::open(&path).unwrap();
    let built = stat_restore_tables(&mut data).unwrap();
    assert_eq!(built.names, ["BaseArmour_+", "Hidden_+"]);
    assert_eq!(built.replicated, [true, false]);
    assert_eq!(built.starting, grants(&[("BaseArmour_+", 3)]));
    assert_eq!(built.items[0].armour_set.as_deref(), Some("Plate"));
    assert_eq!(built.items[1].attachment, grants(&[("BaseArmour_+", 4)]));
    assert_eq!(built.items[2], ItemStatInfo::default());
    assert_eq!(
        built.set_bonuses["Plate"],
        [SetBonus {
            required: 2,
            grants: grants(&[("BaseArmour_+", 30)]),
        }]
    );
    assert_eq!(built.modifier_grants[0], grants(&[("BaseArmour_+", 1)]));
    assert_eq!(built.overburdened, 1);
    assert_eq!(
        built
            .carrying
            .iter()
            .map(|(_, stat)| stat.as_str())
            .collect::<Vec<_>>(),
        [
            "IsCarryingTool_?",
            "IsCarryingAnimal_?",
            "IsCarryingRadar_?",
            "IsCarryingExotic_?",
            "IsCarryingExtractor_?"
        ]
    );
}

#[test]
fn the_delta_keeps_open_ids_and_appends_new_stats_in_row_order() {
    use icarus_bevy::experimental::{pawn_open::FastArrayContent, stat_restore::stat_delta};
    let pair = |stat, value| StatPairRepState { stat, value };
    let open = FastArrayContent {
        replication_key: 3,
        items: vec![pair(0, 300), pair(3, 200), pair(9, 5)],
    };
    // Stat 0 changes, 3 stays, 9 drops to 0; 4 and 7 are new.
    let restored = [pair(0, 614), pair(3, 200), pair(4, 1), pair(7, -20)];
    let delta = stat_delta(&open, &restored).unwrap().unwrap();
    assert_eq!(delta.base_key, 3);
    assert_eq!(
        delta.changed,
        [
            (1, pair(0, 614)),
            (3, pair(9, 0)),
            (4, pair(4, 1)),
            (5, pair(7, -20))
        ]
    );
    // One key step per item sent.
    assert_eq!(delta.array_key, 7);
    assert_eq!(stat_delta(&open, &open.items).unwrap(), None);
}

#[test]
fn the_restored_maxima_bound_the_character_state() {
    use icarus_bevy::experimental::{
        pawn_open::character_state_handle as h,
        stat_restore::{RestoredStats, stat_bounds},
    };
    let names: Vec<String> = [
        "MaximumHealth_+",
        "MaximumStamina_+",
        "MaximumOxygen_+",
        "MaximumWater_+",
        "MaximumFood_+",
    ]
    .iter()
    .map(|name| (*name).to_owned())
    .collect();
    let pair = |stat, value| StatPairRepState { stat, value };
    let stats = RestoredStats {
        items: vec![
            pair(0, 614),
            pair(1, 218),
            pair(2, 300),
            pair(3, 310),
            pair(4, 320),
        ],
        overburdened: false,
    };
    assert_eq!(
        stat_bounds(&stats, &names),
        [
            (h::MAX_HEALTH, 614),
            // Stamina refills to its maximum.
            (h::STAMINA, 218),
            (h::MAX_STAMINA, 218),
            (h::MAX_OXYGEN, 300),
            (h::MAX_WATER, 310),
            (h::MAX_FOOD, 320),
        ]
    );
}

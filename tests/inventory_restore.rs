//! A returning pawn's saved inventories restored as the original does
//! (F-969 to F-971, F-976, F-978), on authored tables and saves.
use std::collections::{BTreeMap, BTreeSet};

use icarus_bevy::experimental::{
    inventory_restore::{
        InventoryInfo, InventoryRestoreGap, InventoryRestoreTables, InventoryRole, ItemInfo,
        ItemTraits, SlotChange, restore_inventories,
    },
    pawn_open::{FastArrayContent, InventorySlot, PawnInventory},
    prospect_info::{SavedActorInventory, SavedItem},
    tag_query::TagQuery,
};
use serde_json::json;

fn units(text: &str) -> Vec<u16> {
    text.encode_utf16().collect()
}

/// Query rows: 0 general (no utility), 1 utility, 2 oxygen, 3 food, 4 water.
fn queries() -> Vec<Option<TagQuery>> {
    let query = |tokens: Vec<u8>, tags: &[&str]| {
        Some(
            TagQuery::from_row(&json!({"Query": {
                "QueryTokenStream": tokens,
                "TagDictionary": tags.iter().map(|tag| json!({"TagName": tag})).collect::<Vec<_>>(),
            }}))
            .unwrap(),
        )
    };
    vec![
        query(vec![0, 1, 5, 1, 3, 1, 0], &["Item.Utility"]),
        query(vec![0, 1, 1, 1, 0], &["Item.Utility"]),
        query(vec![0, 1, 1, 1, 0], &["Item.Consumable.Oxygen"]),
        query(vec![0, 1, 1, 1, 0], &["Item.Consumable.Food"]),
        query(vec![0, 1, 1, 1, 0], &["Item.Consumable.Water"]),
    ]
}

fn tags(list: &[&str]) -> BTreeSet<String> {
    let mut all = BTreeSet::new();
    for tag in list {
        for (at, _) in tag.match_indices('.') {
            all.insert(tag[..at].to_owned());
        }
        all.insert((*tag).to_owned());
    }
    all
}

/// Items: 0 ore, 1 backpack (6 backpack slots), 2 suit (1 oxygen, 1 water,
/// 1 upgrade slot), 3 oxygen tank, 4 canteen, 5 sword (durable, actionable),
/// 6 module, 7 torch (utility, actionable, itemable).
fn tables() -> InventoryRestoreTables {
    let item = |list: &[&str], traits: ItemTraits, grants: &[(&str, i32)]| ItemInfo {
        tags: tags(list),
        traits,
        grants: grants
            .iter()
            .map(|(stat, value)| ((*stat).to_owned(), *value))
            .collect(),
    };
    let itemable = ItemTraits {
        itemable: true,
        ..ItemTraits::default()
    };
    let items = vec![
        item(&["Item.Resource.Ore"], itemable, &[]),
        item(
            &["Item.Equip.Backpack"],
            itemable,
            &[("BaseBackpackSlots_+", 2)],
        ),
        item(
            &["Item.Equip.Suit"],
            itemable,
            &[
                ("BaseOxygenSlots_+", 1),
                ("BaseWaterSlots_+", 1),
                ("BaseUpgradeSlots_+", 1),
            ],
        ),
        item(&["Item.Consumable.Oxygen.Tank"], itemable, &[]),
        item(&["Item.Consumable.Water"], itemable, &[]),
        item(
            &["Item.Weapon.Sword"],
            ItemTraits {
                actionable: true,
                durable: true,
                itemable: true,
                ..ItemTraits::default()
            },
            &[],
        ),
        item(&["Item.Upgrade.Passive"], itemable, &[]),
        item(
            &["Item.Utility.Torch"],
            ItemTraits {
                actionable: true,
                itemable: true,
                ..ItemTraits::default()
            },
            &[],
        ),
    ];
    let names = [
        "Ore", "Backpack", "Suit", "Tank", "Canteen", "Sword", "Module", "Torch",
    ];
    let info = |inventory_id, role, starting_slots, template_query| InventoryInfo {
        inventory_id,
        role,
        starting_slots,
        template_query,
    };
    InventoryRestoreTables {
        items,
        item_rows: names
            .iter()
            .zip(0u16..)
            .map(|(name, row)| (name.to_ascii_lowercase(), row))
            .collect(),
        queries: queries(),
        inventories: vec![
            info(2, InventoryRole::Other, 3, 0),
            info(3, InventoryRole::Backpack, 2, 0),
            info(5, InventoryRole::Equipment, 2, 0),
            info(4, InventoryRole::Suit, 0, 0),
            info(11, InventoryRole::Upgrade, 0, 0),
        ],
        suit_queries: [2, 3, 4],
    }
}

/// The inventories as opened: slot queries per inventory, key 2.
fn opened() -> Vec<PawnInventory> {
    let inventory = |info_row, queries: &[u16]| PawnInventory {
        info_row,
        current_weight: None,
        slots: FastArrayContent {
            replication_key: 2,
            items: queries
                .iter()
                .zip(0..)
                .map(|(query, index)| InventorySlot::empty(*query, index))
                .collect(),
        },
    };
    vec![
        inventory(0, &[0, 0, 1]),
        inventory(1, &[0, 0]),
        inventory(3, &[0, 0]),
        inventory(2, &[]),
        inventory(24, &[]),
    ]
}

fn saved_item(location: i32, row: &str, dynamic: &[(i32, i32)]) -> SavedItem {
    SavedItem {
        location,
        row_name: units(row),
        dynamic: dynamic.to_vec(),
        owner_lookup_id: -1,
        ..SavedItem::default()
    }
}

fn saved(id: i32, items: Vec<SavedItem>) -> SavedActorInventory {
    SavedActorInventory { id, items }
}

/// Each restored inventory's slots as (row, stack) per slot.
fn layout(slots: &[InventorySlot]) -> Vec<Option<(u16, i32)>> {
    slots
        .iter()
        .map(|slot| {
            slot.item.as_ref().map(|item| {
                let stack = item
                    .dynamic
                    .iter()
                    .find(|(property, _)| *property == 7)
                    .map_or(0, |(_, value)| *value);
                (item.row, stack)
            })
        })
        .collect()
}

#[test]
fn saved_items_restore_in_place_and_waiting_items_fill_stat_grown_slots_first() {
    let saved = vec![
        // Unknown inventory IDs and item rows are skipped.
        saved(99, vec![saved_item(0, "Ore", &[(7, 1)])]),
        saved(
            2,
            vec![
                saved_item(0, "Ore", &[(7, 3)]),
                saved_item(1, "Missing", &[(7, 1)]),
                // A torch only fits the utility slot.
                saved_item(2, "Torch", &[(7, 1)]),
            ],
        ),
        saved(
            3,
            vec![
                saved_item(1, "Ore", &[(7, 10)]),
                // Beyond the two starting slots: waits for the backpack.
                saved_item(3, "Ore", &[(7, 20)]),
            ],
        ),
        saved(
            5,
            vec![
                saved_item(0, "Backpack", &[(7, 1)]),
                saved_item(1, "Suit", &[(7, 1)]),
            ],
        ),
        saved(
            4,
            vec![
                saved_item(1, "Canteen", &[(7, 1)]),
                saved_item(0, "Tank", &[(7, 1)]),
            ],
        ),
        saved(11, vec![saved_item(0, "Module", &[(7, 1)])]),
    ];
    let restores = restore_inventories(&opened(), &saved, &tables())
        .unwrap()
        .unwrap();
    let by_index: BTreeMap<usize, _> = restores
        .iter()
        .map(|restore| (restore.inventory, restore))
        .collect();
    // Quickbar: in place; the actionable torch gains properties 0..5.
    let quickbar = by_index[&0];
    assert_eq!(
        layout(&quickbar.restored.slots.items),
        [Some((0, 3)), None, Some((7, 1))]
    );
    assert_eq!(
        quickbar.restored.slots.items[2]
            .item
            .as_ref()
            .unwrap()
            .dynamic,
        [(7, 1), (0, 0), (1, 0), (2, 0), (3, 0), (4, 0), (5, 0)]
    );
    // Key: the client's (2 + 3 slots) plus two sets.
    assert_eq!(
        (quickbar.base_key, quickbar.restored.slots.replication_key),
        (5, 7)
    );
    // Backpack: 2 + 2 granted slots; the waiting ore takes slot 0 and the
    // removed ore follows it.
    let backpack = by_index[&1];
    assert_eq!(
        layout(&backpack.restored.slots.items),
        [Some((0, 20)), Some((0, 10)), None, None]
    );
    // 4 + set 1 + remove 1 + AddSlots 1 + waiting 1 + re-place 1.
    assert_eq!(
        (backpack.base_key, backpack.restored.slots.replication_key),
        (4, 9)
    );
    assert!(matches!(backpack.changes[0], (1, SlotChange::Changed(_))));
    assert!(matches!(backpack.changes[2], (3, SlotChange::Added(_))));
    assert_eq!(backpack.changes.len(), 4);
    // Suit: an oxygen slot, then a water slot; each takes its waiting item.
    let suit = by_index[&3];
    assert_eq!(
        suit.restored
            .slots
            .items
            .iter()
            .map(|slot| slot.query_row)
            .collect::<Vec<_>>(),
        [2, 4]
    );
    assert_eq!(
        layout(&suit.restored.slots.items),
        [Some((3, 1)), Some((4, 1))]
    );
    assert_eq!((suit.base_key, suit.restored.slots.replication_key), (2, 6));
    // Upgrade: one granted slot holding the module.
    assert_eq!(layout(&by_index[&4].restored.slots.items), [Some((6, 1))]);
    // Equipment is restored in place and sizes nothing itself.
    assert_eq!(
        layout(&by_index[&2].restored.slots.items),
        [Some((1, 1)), Some((2, 1))]
    );
}

#[test]
fn restore_stops_at_gaps_instead_of_inventing_values() {
    let tables = tables();
    // A durable item without its saved durability: VerifyItem's value is a
    // stat lookup restore does not model.
    let gap = |items| restore_inventories(&opened(), &[saved(2, items)], &tables).unwrap();
    assert_eq!(
        gap(vec![saved_item(0, "Sword", &[(7, 1)])]),
        Err(InventoryRestoreGap::UnreadDefault(6))
    );
    // With it saved, nothing is missing.
    assert!(gap(vec![saved_item(0, "Sword", &[(7, 1), (6, 900)])]).is_ok());
    assert_eq!(
        gap(vec![saved_item(0, "Ore", &[(14, 1)])]),
        Err(InventoryRestoreGap::InvalidDynamicProperty)
    );
    let living = SavedItem {
        living_slots: 1,
        ..saved_item(0, "Ore", &[(7, 1)])
    };
    assert_eq!(gap(vec![living]), Err(InventoryRestoreGap::LivingSlots));
    // A Quickbar item saved beyond its slots never finds one.
    assert_eq!(
        gap(vec![saved_item(5, "Ore", &[(7, 1)])]),
        Err(InventoryRestoreGap::Unplaced)
    );
    // A saved item set in place keeps its GUID, owner lookup and
    // alterations.
    let item = SavedItem {
        guid: units("GUID-1"),
        owner_lookup_id: 0,
        alterations: vec![units("Sharp")],
        ..saved_item(0, "Ore", &[(7, 2)])
    };
    let restores = gap(vec![item]).unwrap();
    let slot = restores[0].restored.slots.items[0].item.as_ref().unwrap();
    assert_eq!(
        (
            slot.guid.units().to_vec(),
            slot.owner_lookup_id,
            slot.alterations.len()
        ),
        (units("GUID-1"), 0, 1)
    );
    // Nothing saved: nothing changes.
    assert!(
        restore_inventories(&opened(), &[], &tables)
            .unwrap()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn tag_queries_match_any_all_none_and_nested_expressions_over_parents() {
    let query = |tokens: Vec<u8>, tags: &[&str]| {
        TagQuery::from_row(&json!({"Query": {
            "QueryTokenStream": tokens,
            "TagDictionary": tags.iter().map(|tag| json!({"TagName": tag})).collect::<Vec<_>>(),
        }}))
        .unwrap()
    };
    let item = tags(&["Item.Consumable.Oxygen.Tank", "Traits.Fillable"]);
    assert!(query(vec![0, 1, 1, 1, 0], &["Item.Consumable.Oxygen"]).matches(&item));
    assert!(!query(vec![0, 1, 1, 1, 0], &["Item.Consumable.Water"]).matches(&item));
    assert!(
        query(
            vec![0, 1, 2, 2, 0, 1],
            &["Item.Consumable", "Traits.Fillable"]
        )
        .matches(&item)
    );
    assert!(!query(vec![0, 1, 3, 1, 0], &["Traits.Fillable"]).matches(&item));
    // ALL(NONE(Item.Utility)): the general slot.
    assert!(query(vec![0, 1, 5, 1, 3, 1, 0], &["Item.Utility"]).matches(&item));
    // ANY(ALL(a), NONE(b)) with nested expressions.
    let nested = query(
        vec![0, 1, 4, 2, 2, 1, 0, 3, 1, 1],
        &["Item.Weapon", "Traits.Fillable"],
    );
    assert!(!nested.matches(&item));
    // The empty query matches nothing; a token past the dictionary rejects.
    assert!(!query(vec![0, 0], &[]).matches(&item));
    assert!(
        TagQuery::from_row(&json!({"Query": {"QueryTokenStream": [0, 1, 1, 1, 3],
            "TagDictionary": [{"TagName": "A"}]}}))
        .is_err()
    );
}

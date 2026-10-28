//! A returning pawn's saved inventories restored as the original's base
//! actor-state restore does (F-969 to F-971, F-976, F-978).
//!
//! Each saved inventory restores onto the pawn inventory with its
//! `InventoryID`; each saved item is built from its saved row, dynamic data,
//! named stats, alterations, GUID and owner lookup ID, `VerifyItem` appends
//! the dynamic properties its traits lack, and `SetItem` places it at its
//! saved location when that slot exists, is empty and its query accepts the
//! item; otherwise it waits. The equipped items then grant the slot stats:
//! when an inventory's slot count changes, its items are removed, slots are
//! added or removed, and the waiting items, then the removed ones, are placed
//! in the first empty slot whose query accepts them.
//!
//! The result is each inventory's final slots and, against the slots it
//! opened with, the changed slots and replication keys of the fast-array
//! delta the client receives. Defaults the original computes from data this
//! module does not model are gaps: they suppress the restore instead of
//! sending a guess.
//!
//! Each changed inventory's `CurrentWeight` follows `RecalculateWeight` on
//! the pawn (F-933, F-982): every stacked item's `Itemable` weight, plus a
//! weighable resource container's stored units, times its stack. Carry-weight
//! stats read 0; a bag with a linked inventory (whose weight adds) or an item
//! with a weight stat of its own leaves the inventory's weight unsent.
use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Result, ensure};

use super::control_messages::ArchiveText;
use super::inventory_weight::{
    DEFAULT_CDO_CURRENT_WEIGHT, ItemWeightInput, recalculate_inventory_weight,
};
use super::pawn_open::{InventorySlot, PawnInventory, SlotItem};
use super::prospect_info::{SavedActorInventory, SavedItem};
use super::tag_query::TagQuery;

/// `EDynamicItemProperties` values `VerifyItem` appends (F-976).
mod property {
    pub const ACTIONABLE: [u32; 6] = [0, 1, 2, 3, 4, 5];
    pub const DURABILITY: u32 = 6;
    pub const ITEMABLE_STACK: u32 = 7;
    pub const MILLIJOULES_REMAINING: u32 = 8;
    pub const TRANSMUTABLE_UNITS: u32 = 9;
    pub const FILLABLE_STORED_UNITS: u32 = 10;
    pub const FILLABLE_TYPE: u32 = 11;
    pub const DECAYABLE_SPOIL_TIME: u32 = 12;
    pub const LINKED_INVENTORY: u32 = 13;
    /// `MaxDynamicItemProperties`: saved indices at or above it are invalid.
    pub const COUNT: u32 = 14;
}

/// The item traits `VerifyItem` reads (F-976), by their `D_ItemsStatic`
/// handles.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ItemTraits {
    pub actionable: bool,
    pub combustible: bool,
    pub transmutable: bool,
    pub itemable: bool,
    pub fillable: bool,
    pub durable: bool,
    /// A `Decayable` row whose `SpoilTime` is positive: only then does
    /// `VerifyItem` append a spoil time.
    pub decaying: bool,
    pub inventory_container: bool,
    pub living: bool,
}

/// What restore needs of one `D_ItemsStatic` row.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ItemInfo {
    /// `Generated_Tags`' tags and parents, which slot queries test.
    pub tags: BTreeSet<String>,
    pub traits: ItemTraits,
    /// Stats the item grants while equipped: its `Equippable` row's
    /// `GrantedStats` and `Armour` row's `ArmourStats`, by stat name.
    pub grants: BTreeMap<String, i32>,
    pub weight: ItemWeight,
}

/// What one item weighs in `RecalculateWeight` (F-933).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ItemWeight {
    /// Not `Itemable`: it weighs nothing.
    #[default]
    Weightless,
    /// Its `Itemable` row's `Weight`, whether it is a weighable resource
    /// container (`Any_Weighable_Resource_Container`), whose stored units
    /// add, and whether it is a bag (`Any_Bag`), whose linked inventory's
    /// weight adds.
    Unit {
        weight: i32,
        resource_container: bool,
        bag: bool,
    },
    /// Its weight depends on stats this module does not model (a `Weight_+`
    /// in its static `AdditionalStats`), or weights are not modelled at all.
    Unmodelled,
}

/// Which of the stat-sized inventories a pawn inventory is (F-971).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InventoryRole {
    Backpack,
    Upgrade,
    Suit,
    Equipment,
    Other,
}

/// One pawn inventory as restore sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InventoryInfo {
    /// Its `InventoryID` as a `D_InventoryID` row index.
    pub inventory_id: i32,
    pub role: InventoryRole,
    /// `StartingSlots`.
    pub starting_slots: i32,
    /// The `SlotTemplate` query row, which added slots use.
    pub template_query: u16,
}

/// The data tables restore consults, precomputed.
#[derive(Clone, Debug, Default)]
pub struct InventoryRestoreTables {
    /// By `D_ItemsStatic` row index.
    pub items: Vec<ItemInfo>,
    /// Lower-case row name to row index (the original resolves names
    /// case-insensitively).
    pub item_rows: BTreeMap<String, u16>,
    /// By `D_TagQueries` row index; `None` for a row that does not compile.
    pub queries: Vec<Option<TagQuery>>,
    /// The pawn's inventories in open order.
    pub inventories: Vec<InventoryInfo>,
    /// `Any_Oxygen`, `Any_Food` and `Any_Water`, the Suit's slot queries.
    pub suit_queries: [u16; 3],
}

/// Why restore stops without sending anything.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InventoryRestoreGap {
    /// `VerifyItem` would append this property, whose default is unread or
    /// needs state restore does not model (linked container inventories).
    UnreadDefault(u32),
    /// A saved dynamic property index at or above `MaxDynamicItemProperties`.
    InvalidDynamicProperty,
    /// A saved item carries living-item slots, whose wire is unread.
    LivingSlots,
    /// A saved stat is indexed, not named (an older recorder version).
    IndexedStat,
    /// An item no slot accepts after slot growth: the original bounces or
    /// drops it, which this module does not model.
    Unplaced,
    /// The pawn's inventories do not match the tables.
    InventoryMismatch,
}

/// One slot's change in the delta, by its fast-array ID.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SlotChange {
    /// A slot the client knows whose contents differ.
    Changed(InventorySlot),
    /// A slot added after the open.
    Added(InventorySlot),
}

/// One inventory's restore: its final state and its delta.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InventoryRestore {
    /// Index into the pawn's inventories.
    pub inventory: usize,
    /// The inventory as the client holds it (its open).
    pub opened: PawnInventory,
    pub restored: PawnInventory,
    /// The key the client holds ([`client_key`]).
    pub base_key: i32,
    /// Changed slots in ID order.
    pub changes: Vec<(i32, SlotChange)>,
    /// The restored `CurrentWeight`; `None` when an item's weight is not
    /// modelled.
    pub weight: Option<i32>,
}

/// `RecalculateWeight` over `slots` on the pawn, an owner with a stat
/// container whose carry-weight stats read 0 (F-933); `None` when an item's
/// weight is not modelled: a bag with a linked inventory (whose weight adds),
/// a `Weight_+` stat of its own, or an unmodelled row. Other items' linked
/// inventories add nothing.
pub fn inventory_weight(tables: &InventoryRestoreTables, slots: &[InventorySlot]) -> Option<i32> {
    let mut inputs = Vec::new();
    for item in slots.iter().filter_map(|slot| slot.item.as_ref()) {
        let value = |property: u32| {
            item.dynamic
                .iter()
                .find(|(saved, _)| *saved == property)
                .map(|(_, value)| *value)
        };
        if item
            .stats
            .iter()
            .any(|(name, _)| String::from_utf16_lossy(name.units()).contains(WEIGHT_STAT_SUFFIX))
        {
            return None;
        }
        let linked = value(property::LINKED_INVENTORY).is_some_and(|linked| linked >= 0);
        let (weight, resource_container) = match tables.items.get(usize::from(item.row))?.weight {
            ItemWeight::Weightless => continue,
            ItemWeight::Unmodelled => return None,
            ItemWeight::Unit { bag: true, .. } if linked => return None,
            ItemWeight::Unit {
                weight,
                resource_container,
                ..
            } => (weight, resource_container),
        };
        inputs.push(ItemWeightInput {
            has_valid_itemable: true,
            weight_stat: weight,
            matches_weighable_resource_container: resource_container,
            fillable_units: value(property::FILLABLE_STORED_UNITS),
            itemable_stack: value(property::ITEMABLE_STACK),
            matching_carry_weight_stat_pct: 0,
        });
    }
    recalculate_inventory_weight(
        DEFAULT_CDO_CURRENT_WEIGHT,
        true,
        false,
        true,
        false,
        &inputs,
        &[],
    )
    .map(|outcome| outcome.current_weight)
}

/// The stat name part that makes an item's own stat a weight stat.
pub const WEIGHT_STAT_SUFFIX: &str = "Weight_+";

/// One inventory's slots while restore runs, with its replication key.
struct Working {
    slots: Vec<InventorySlot>,
    pending: Vec<SlotItem>,
    key: i32,
}

impl Working {
    fn accepts(&self, tables: &InventoryRestoreTables, slot: usize, item: &SlotItem) -> bool {
        let query = self.slots[slot].query_row;
        let tags = &tables.items[usize::from(item.row)].tags;
        tables
            .queries
            .get(usize::from(query))
            .and_then(Option::as_ref)
            .is_some_and(|query| query.matches(tags))
    }

    /// `AutomaticallyPlaceItem` without stacking: the first empty slot that
    /// accepts the item.
    fn place_automatically(&mut self, tables: &InventoryRestoreTables, item: SlotItem) -> bool {
        let found = (0..self.slots.len())
            .find(|slot| self.slots[*slot].item.is_none() && self.accepts(tables, *slot, &item));
        match found {
            Some(slot) => {
                self.slots[slot].item = Some(item);
                self.key += 1;
                true
            }
            None => false,
        }
    }

    /// `AddSlots(count, query)`: append slots, then retry every waiting
    /// item; marks the array dirty once.
    fn add_slots(&mut self, tables: &InventoryRestoreTables, count: usize, query: u16) {
        for _ in 0..count {
            let index = i32::try_from(self.slots.len()).unwrap_or(i32::MAX);
            self.slots.push(InventorySlot::empty(query, index));
        }
        self.key += 1;
        let waiting = std::mem::take(&mut self.pending);
        for item in waiting {
            let unplaced = item.clone();
            if !self.place_automatically(tables, item) {
                self.pending.push(unplaced);
            }
        }
    }

    /// `RemoveAllItems`: every item in slot order, each removal marking its
    /// slot dirty.
    fn remove_all(&mut self) -> Vec<SlotItem> {
        let mut removed = Vec::new();
        for slot in &mut self.slots {
            if let Some(item) = slot.item.take() {
                removed.push(item);
                self.key += 1;
            }
        }
        removed
    }

    /// Bring the slots to `target` (each a query row), as
    /// `UpdateInventorySlots` does when the count differs: remove all items,
    /// add (draining the waiting items) or drop trailing slots, then place
    /// the removed items automatically.
    fn resize(&mut self, tables: &InventoryRestoreTables, target: &[u16]) -> Result<bool> {
        if self.slots.len() == target.len() {
            return Ok(true);
        }
        let removed = self.remove_all();
        if target.len() < self.slots.len() {
            self.slots.truncate(target.len());
            self.key += 1;
        } else {
            let start = self.slots.len();
            // Consecutive slots of one query come from one AddSlots call.
            let mut at = start;
            while at < target.len() {
                let query = target[at];
                let run = target[at..].iter().take_while(|row| **row == query).count();
                self.add_slots(tables, run, query);
                at += run;
            }
        }
        for item in removed {
            if !self.place_automatically(tables, item) {
                return Ok(false);
            }
        }
        Ok(true)
    }
}

/// The key a client holds for an opened inventory's slots: the open's key
/// plus one per slot it opened with, as the original's first delta after
/// the open bases itself (F-978).
fn client_key(inventory: &PawnInventory) -> Result<i32> {
    let slots = i32::try_from(inventory.slots.items.len())?;
    inventory
        .slots
        .replication_key
        .checked_add(slots)
        .ok_or_else(|| anyhow::anyhow!("an inventory's replication key overflows"))
}

/// Build a saved item as `UpdateOwnerInventories` does, then `VerifyItem`
/// (F-969, F-976). `Ok(None)` skips an item whose row has no static data.
fn restore_item(
    tables: &InventoryRestoreTables,
    saved: &SavedItem,
) -> Result<std::result::Result<Option<SlotItem>, InventoryRestoreGap>> {
    let name = String::from_utf16_lossy(&saved.row_name).to_ascii_lowercase();
    let Some(row) = tables.item_rows.get(&name).copied() else {
        return Ok(Ok(None));
    };
    let Some(info) = tables.items.get(usize::from(row)) else {
        return Ok(Ok(None));
    };
    if saved.living_slots != 0 {
        return Ok(Err(InventoryRestoreGap::LivingSlots));
    }
    let mut dynamic = Vec::with_capacity(saved.dynamic.len());
    for (index, value) in &saved.dynamic {
        // UpdateOwnerInventories keeps the index's low byte.
        let index = u32::from(*index as u8);
        if index >= property::COUNT {
            return Ok(Err(InventoryRestoreGap::InvalidDynamicProperty));
        }
        dynamic.push((index, *value));
    }
    let mut stats = Vec::with_capacity(saved.stats.len());
    for (index, name, value) in &saved.stats {
        if *index != -1 {
            return Ok(Err(InventoryRestoreGap::IndexedStat));
        }
        stats.push((ArchiveText::new(name)?, *value));
    }
    let has =
        |dynamic: &[(u32, i32)], property| dynamic.iter().any(|(index, _)| *index == property);
    let traits = info.traits;
    let mut append = |property, value| {
        if !has(&dynamic, property) {
            dynamic.push((property, value));
        }
    };
    if traits.actionable {
        for property in property::ACTIONABLE {
            append(property, 0);
        }
    }
    let unread = [
        (traits.combustible, property::MILLIJOULES_REMAINING),
        (traits.transmutable, property::TRANSMUTABLE_UNITS),
    ];
    for (present, property) in unread {
        if present && !has(&dynamic, property) {
            return Ok(Err(InventoryRestoreGap::UnreadDefault(property)));
        }
    }
    if traits.itemable && !has(&dynamic, property::ITEMABLE_STACK) {
        dynamic.push((property::ITEMABLE_STACK, 1));
    }
    let unread = [
        (traits.fillable, property::FILLABLE_STORED_UNITS),
        (traits.fillable, property::FILLABLE_TYPE),
        (traits.durable, property::DURABILITY),
        (traits.decaying, property::DECAYABLE_SPOIL_TIME),
        (traits.inventory_container, property::LINKED_INVENTORY),
    ];
    for (present, property) in unread {
        if present && !has(&dynamic, property) {
            return Ok(Err(InventoryRestoreGap::UnreadDefault(property)));
        }
    }
    let alterations = saved
        .alterations
        .iter()
        .map(|name| ArchiveText::new(name))
        .collect::<Result<_>>()?;
    Ok(Ok(Some(SlotItem {
        dynamic,
        stats,
        alterations,
        guid: ArchiveText::new(&saved.guid)?,
        owner_lookup_id: saved.owner_lookup_id,
        ..SlotItem::new(row)?
    })))
}

/// The slot queries an inventory holds once its slot stat applies, or
/// `None` when its count does not follow a stat (F-971).
fn target_slots(
    tables: &InventoryRestoreTables,
    info: &InventoryInfo,
    grants: &BTreeMap<String, i32>,
) -> Option<Vec<u16>> {
    let stat = |name: &str| grants.get(name).copied().unwrap_or(0).max(0) as usize;
    let starting = usize::try_from(info.starting_slots).unwrap_or(0);
    let template = |count: usize| vec![info.template_query; starting + count];
    match info.role {
        InventoryRole::Backpack => Some(template(stat("BaseBackpackSlots_+"))),
        InventoryRole::Upgrade => Some(template(stat("BaseUpgradeSlots_+"))),
        InventoryRole::Suit => {
            let [oxygen, food, water] = tables.suit_queries;
            let mut slots = vec![oxygen; stat("BaseOxygenSlots_+")];
            slots.extend(vec![food; stat("BaseFoodSlots_+")]);
            slots.extend(vec![water; stat("BaseWaterSlots_+")]);
            Some(slots)
        }
        InventoryRole::Equipment | InventoryRole::Other => None,
    }
}

/// Restore `saved` onto the pawn's `current` inventories (as opened).
/// Returns the inventories that change, in pawn order, or the gap that
/// stops the restore.
pub fn restore_inventories(
    current: &[PawnInventory],
    saved: &[SavedActorInventory],
    tables: &InventoryRestoreTables,
) -> Result<std::result::Result<Vec<InventoryRestore>, InventoryRestoreGap>> {
    if current.len() != tables.inventories.len() {
        return Ok(Err(InventoryRestoreGap::InventoryMismatch));
    }
    let mut working: Vec<Working> = current
        .iter()
        .map(|inventory| {
            Ok(Working {
                slots: inventory.slots.items.clone(),
                pending: Vec::new(),
                key: client_key(inventory)?,
            })
        })
        .collect::<Result<_>>()?;
    for inventory in saved {
        let Some(index) = tables
            .inventories
            .iter()
            .position(|info| info.inventory_id == inventory.id)
        else {
            continue;
        };
        for saved_item in &inventory.items {
            let item = match restore_item(tables, saved_item)? {
                Ok(Some(item)) => item,
                Ok(None) => continue,
                Err(gap) => return Ok(Err(gap)),
            };
            let target = &mut working[index];
            let slot = usize::try_from(saved_item.location)
                .ok()
                .filter(|slot| *slot < target.slots.len());
            match slot {
                Some(slot)
                    if target.slots[slot].item.is_none() && target.accepts(tables, slot, &item) =>
                {
                    target.slots[slot].item = Some(item);
                    target.key += 1;
                }
                _ => target.pending.push(item),
            }
        }
    }
    // The equipped items' grants size the stat-driven inventories.
    let mut grants: BTreeMap<String, i32> = BTreeMap::new();
    for (inventory, info) in working.iter().zip(&tables.inventories) {
        if info.role != InventoryRole::Equipment {
            continue;
        }
        for item in inventory.slots.iter().filter_map(|slot| slot.item.as_ref()) {
            for (stat, value) in &tables.items[usize::from(item.row)].grants {
                let total = grants.entry(stat.clone()).or_default();
                *total = total.wrapping_add(*value);
            }
        }
    }
    for (inventory, info) in working.iter_mut().zip(&tables.inventories) {
        if let Some(target) = target_slots(tables, info, &grants)
            && !inventory.resize(tables, &target)?
        {
            return Ok(Err(InventoryRestoreGap::Unplaced));
        }
        if !inventory.pending.is_empty() {
            return Ok(Err(InventoryRestoreGap::Unplaced));
        }
    }
    let mut restores = Vec::new();
    for (index, (inventory, opened)) in working.into_iter().zip(current).enumerate() {
        let known = opened.slots.items.len();
        ensure!(inventory.slots.len() < i32::MAX as usize, "too many slots");
        let mut changes = Vec::new();
        for (slot, (final_slot, id)) in inventory.slots.iter().zip(1_i32..).enumerate() {
            if slot >= known {
                changes.push((id, SlotChange::Added(final_slot.clone())));
            } else if *final_slot != opened.slots.items[slot] {
                changes.push((id, SlotChange::Changed(final_slot.clone())));
            }
        }
        // Dropped trailing slots would be deletions; slot counts here only
        // grow from the open's starting slots.
        ensure!(
            inventory.slots.len() >= known,
            "restore removed slots the client knows"
        );
        if changes.is_empty() {
            continue;
        }
        let weight = inventory_weight(tables, &inventory.slots);
        restores.push(InventoryRestore {
            inventory: index,
            opened: opened.clone(),
            base_key: client_key(opened)?,
            restored: PawnInventory {
                slots: super::pawn_open::FastArrayContent {
                    replication_key: inventory.key,
                    items: inventory.slots,
                },
                current_weight: weight.or(opened.current_weight),
                ..opened.clone()
            },
            changes,
            weight,
        });
    }
    Ok(Ok(restores))
}

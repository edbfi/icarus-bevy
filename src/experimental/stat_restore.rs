//! A returning pawn's replicated stats as the original's restore leaves
//! them (F-983 to F-987).
//!
//! The original sums every stat over its sources and derives the virtual
//! stats from their formulas (F-839, F-984). A restored pawn's sources are
//! the starting grants; each item in its Equipment and Upgrade inventories'
//! `Equippable` and `Armour` grants; the `D_ArmourSetBonus` rows whose
//! `RequiredGear` its equipped pieces of one `ArmourSet` reach; the
//! `D_Alterations` stats of the attachments in the inventories its equipped
//! items link (F-985); its modifiers' `GrantedStats` scaled by their final
//! effectiveness, and `Overburdened`'s while its inventories outweigh its
//! `WeightCapacity_+` times 1000 (F-986); and, per inventory, one per item
//! matching each carrying query (F-987). The result is every nonzero
//! `bIsReplicated` stat in row order, as the open sends a fresh pawn's.
use std::collections::BTreeMap;

use anyhow::{Context, Result};

use super::inventory_restore::{InventoryRestoreTables, InventoryRole};
use super::pawn_open::InventorySlot;
use super::rep_layout::StatPairRepState;
use super::tag_query::TagQuery;
use crate::pawn_stats::StatValues;

/// `EDynamicItemProperties` `AssociatedInventoryId`: the linked inventory.
const LINKED_INVENTORY: u32 = 13;

/// The stat that compares against the carried weight (F-986).
const WEIGHT_CAPACITY: &str = "WeightCapacity_+";

/// What stat restore reads of one `D_ItemsStatic` row besides its tags and
/// equipped grants (in [`InventoryRestoreTables`]).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ItemStatInfo {
    /// Its `Armour` row's `ArmourSet`, if any.
    pub armour_set: Option<String>,
    /// The `Stats` of the `D_Alterations` row its `Attachments` names.
    pub attachment: BTreeMap<String, i32>,
}

/// One `D_ArmourSetBonus` row: its `RequiredGear` and `StatsGranted`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SetBonus {
    pub required: i32,
    pub grants: BTreeMap<String, i32>,
}

/// The data tables stat restore consults, precomputed.
#[derive(Clone, Debug, Default)]
pub struct StatRestoreTables {
    /// `D_Stats` row names and whether each row replicates.
    pub names: Vec<String>,
    pub replicated: Vec<bool>,
    /// The starting grants (`D_CharacterStartingStats` `Base_Stats`).
    pub starting: BTreeMap<String, i32>,
    /// By `D_ItemsStatic` row index.
    pub items: Vec<ItemStatInfo>,
    /// `ArmourSet` to its bonuses.
    pub set_bonuses: BTreeMap<String, Vec<SetBonus>>,
    /// By `D_ModifierStates` row index: `GrantedStats`.
    pub modifier_grants: Vec<BTreeMap<String, i32>>,
    /// The `Overburdened` row (F-986).
    pub overburdened: u16,
    /// Each carrying query with the stat it counts into (F-987).
    pub carrying: Vec<(TagQuery, String)>,
}

/// One pawn's state that its restored stats follow.
#[derive(Clone, Debug)]
pub struct PawnStatInputs<'a> {
    /// Each pawn inventory's role and final slots, in open order.
    pub inventories: Vec<(InventoryRole, &'a [InventorySlot])>,
    /// The saved container inventories' items by `InventoryIndex` (`C016`):
    /// the inventories items link.
    pub linked: &'a BTreeMap<i32, Vec<u16>>,
    /// Each restored modifier's row and final effectiveness.
    pub modifiers: Vec<(u16, i32)>,
    /// The inventories' summed `CurrentWeight`.
    pub weight: i32,
}

/// The restored stats and whether `Overburdened` applies.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RestoredStats {
    /// Every nonzero replicated stat in row order.
    pub items: Vec<StatPairRepState>,
    pub overburdened: bool,
}

/// `UModifierStateComponent::ScaleByEffectiveness` (F-986): `v` times
/// `(effectiveness + 100) / 100` in single precision, then `CeilToInt` of
/// its magnitude with its sign.
pub fn scale_by_effectiveness(value: i32, effectiveness: i32) -> i32 {
    let scaled = ((effectiveness as f32 + 100.0) * value as f32) * 0.01_f32;
    let sign = if scaled > 0.0 {
        1.0_f32
    } else if scaled < 0.0 {
        -1.0
    } else {
        0.0
    };
    // UE's `CeilToInt`: `-(round_even(-0.5 - 2x) >> 1)`.
    let doubled = -0.5_f32 - (scaled.abs() + scaled.abs());
    let ceiling = -((doubled.round_ties_even() as i32) >> 1);
    (ceiling as f32 * sign) as i32
}

/// The restored stats for `inputs` (F-983 to F-987).
pub fn restore_stats(
    tables: &StatRestoreTables,
    inventory: &InventoryRestoreTables,
    inputs: &PawnStatInputs<'_>,
) -> Result<RestoredStats> {
    let mut grants = tables.starting.clone();
    let add = |grants: &mut BTreeMap<String, i32>, stat: &str, value: i32| {
        let total = grants.entry(stat.to_owned()).or_default();
        *total = total.wrapping_add(value);
    };
    let mut set_pieces: BTreeMap<&str, i32> = BTreeMap::new();
    for (role, slots) in &inputs.inventories {
        let items = || slots.iter().filter_map(|slot| slot.item.as_ref());
        // Carrying stats count every item, per inventory (F-987).
        for item in items() {
            let info = inventory
                .items
                .get(usize::from(item.row))
                .context("a restored item has no static row")?;
            for (query, stat) in &tables.carrying {
                if query.matches(&info.tags) {
                    add(&mut grants, stat, 1);
                }
            }
        }
        if !matches!(role, InventoryRole::Equipment | InventoryRole::Upgrade) {
            continue;
        }
        for item in items() {
            let row = usize::from(item.row);
            let info = inventory
                .items
                .get(row)
                .context("a restored item has no static row")?;
            for (stat, value) in &info.grants {
                add(&mut grants, stat, *value);
            }
            if *role != InventoryRole::Equipment {
                continue;
            }
            let stats = tables
                .items
                .get(row)
                .context("a restored item has no stat row")?;
            if let Some(set) = &stats.armour_set {
                *set_pieces.entry(set).or_default() += 1;
            }
            // Attachments in the inventory the item links (F-985).
            let linked = item
                .dynamic
                .iter()
                .find(|(property, _)| *property == LINKED_INVENTORY)
                .map(|(_, index)| *index);
            for attachment in linked
                .and_then(|index| inputs.linked.get(&index))
                .into_iter()
                .flatten()
            {
                let attachment = tables
                    .items
                    .get(usize::from(*attachment))
                    .context("a linked item has no stat row")?;
                for (stat, value) in &attachment.attachment {
                    add(&mut grants, stat, *value);
                }
            }
        }
    }
    for (set, pieces) in set_pieces {
        for bonus in tables.set_bonuses.get(set).into_iter().flatten() {
            if pieces >= bonus.required {
                for (stat, value) in &bonus.grants {
                    add(&mut grants, stat, *value);
                }
            }
        }
    }
    for (row, effectiveness) in &inputs.modifiers {
        for (stat, value) in tables
            .modifier_grants
            .get(usize::from(*row))
            .context("a restored modifier has no row")?
        {
            add(
                &mut grants,
                stat,
                scale_by_effectiveness(*value, *effectiveness),
            );
        }
    }
    let capacity = evaluate(tables, &grants)?.value(WEIGHT_CAPACITY);
    let overburdened = capacity.wrapping_mul(1000) < inputs.weight;
    if overburdened {
        for (stat, value) in tables
            .modifier_grants
            .get(usize::from(tables.overburdened))
            .context("no Overburdened row")?
        {
            add(&mut grants, stat, *value);
        }
    }
    let values = evaluate(tables, &grants)?;
    let mut items = Vec::new();
    for (stat, replicated) in tables.replicated.iter().enumerate() {
        if !replicated {
            continue;
        }
        let value = values.at(stat);
        if value != 0 {
            items.push(StatPairRepState {
                stat: u32::try_from(stat)?,
                value,
            });
        }
    }
    Ok(RestoredStats {
        items,
        overburdened,
    })
}

/// `grants` by stat name as stat values over the table's rows; a name the
/// table lacks fails.
fn evaluate<'a>(
    tables: &'a StatRestoreTables,
    grants: &BTreeMap<String, i32>,
) -> Result<StatValues<'a>> {
    let index: BTreeMap<&str, usize> = tables
        .names
        .iter()
        .enumerate()
        .map(|(row, name)| (name.as_str(), row))
        .collect();
    let mut sources = BTreeMap::new();
    for (stat, value) in grants {
        let row = index
            .get(stat.as_str())
            .with_context(|| format!("D_Stats lacks a granted stat {stat}"))?;
        sources.insert(*row, *value);
    }
    Ok(StatValues::new(&tables.names, sources))
}

//! EXPERIMENTAL, session-only projection of authored player bags (F-1200)
//! into the owning pawn's inventory replication
//! (`network.player_inventory_projection`).
//!
//! An operator-authored binding maps every recipe inventory key to a
//! `D_InventoryInfo` row and one `D_TagQueries` row per slot, and every recipe
//! item key to a `D_ItemsStatic` row with its explicitly authored dynamic
//! properties. Rows are checked against the configured data.pak's row names
//! only (protocol identity, F-830, F-993); no packaged value is read, and an
//! unknown, ambiguous, duplicate or missing binding refuses startup.
//!
//! The bound inventories open empty on every pawn (F-799's shape); a bag then
//! reaches its owner as slot deltas with F-978's replication-key arithmetic.
//! Format 2 adds authored `CurrentWeight` changes for the slots each delta
//! leaves in the inventory.
//! Nothing here mutates, retains or persists native state.
use std::{
    collections::{BTreeMap, BTreeSet},
    io::Read,
    os::unix::fs::OpenOptionsExt,
    path::Path,
    sync::Arc,
};

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};

use crate::experimental::inventory_restore::{InventoryRestore, SlotChange};
use crate::experimental::pawn_open::{FastArrayContent, InventorySlot, PawnInventory, SlotItem};
use crate::native_inventory::{InventoryStateV1, StackV1};
use crate::native_player_inventory::CompiledPlayerInventoryRecipe;

/// Largest binding file.
pub const MAX_BINDING_BYTES: usize = 64 * 1024;
/// Most bound inventories (each is one more pawn subobject).
pub const MAX_BOUND_INVENTORIES: usize = 16;
/// Most slots one bound inventory may declare.
pub const MAX_BOUND_SLOTS: usize = 1024;
/// Most changed slots one pawn update carries; the rest follow once the pawn
/// channel settles, in inventory then slot order.
pub const MAX_SLOT_CHANGES_PER_UPDATE: usize = 64;

/// `EDynamicItemProperties::ItemableStack` (F-976, F-1066): the stack count.
pub const ITEMABLE_STACK_PROPERTY: u32 = 7;
/// `EDynamicItemProperties` 13, a container's linked inventory; `-1` links
/// none (F-1066). No linked inventory is modelled.
const LINKED_INVENTORY_PROPERTY: u32 = 13;
/// `MaxDynamicItemProperties` (F-976): properties lie below it.
const DYNAMIC_PROPERTY_COUNT: u32 = 14;
/// A 2-byte row handle naming no row.
const NO_ROW: usize = u16::MAX as usize;

/// The authored binding file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlayerInventoryProjectionBinding {
    pub format: u32,
    pub inventories: Vec<InventoryBinding>,
    pub items: Vec<ItemBinding>,
}

/// One recipe inventory's protocol identities.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InventoryBinding {
    /// Recipe inventory key.
    pub inventory: String,
    /// `D_InventoryInfo` row name.
    pub info_row: String,
    /// One `D_TagQueries` row name per slot, in slot order.
    pub slot_queries: Vec<String>,
}

/// One recipe item's protocol identity and authored item data.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ItemBinding {
    /// Recipe item key.
    pub item: String,
    /// `D_ItemsStatic` row name.
    pub row: String,
    /// Send the stack count as `ItemableStack` (property 7). False requires
    /// the recipe's `stack_limit` to be 1: a count cannot be sent otherwise.
    pub count_property: bool,
    /// Authored `(EDynamicItemProperties, value)` pairs; none are inferred.
    pub dynamic: Vec<DynamicProperty>,
    /// Authored weight per unit. Required in format 2 and absent in format 1.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "unit_weight"
    )]
    pub unit_weight: Option<u32>,
}

fn unit_weight<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Option<u32>, D::Error> {
    u32::deserialize(deserializer).map(Some)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DynamicProperty {
    pub property: u32,
    pub value: i32,
}

/// The installed pak's row names (identities only) a binding resolves.
#[derive(Clone, Copy, Debug)]
pub struct ProjectionCatalog<'a> {
    pub items_static: &'a [String],
    pub inventory_info: &'a [String],
    pub tag_queries: &'a [String],
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct BoundInventory {
    key: String,
    info_row: u8,
    slots: Vec<BoundSlot>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct BoundSlot {
    query_row: u16,
    max_weight: i32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct BoundItem {
    row: u16,
    count_property: bool,
    dynamic: Vec<(u32, i32)>,
}

/// A compiled binding: bound inventories in recipe-key order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlayerInventoryProjection {
    inventories: Vec<BoundInventory>,
    items: BTreeMap<String, BoundItem>,
    unit_weights: Option<Arc<BTreeMap<u16, i32>>>,
}

impl PlayerInventoryProjectionBinding {
    pub fn parse(text: &str) -> Result<Self> {
        ensure!(
            text.len() <= MAX_BINDING_BYTES,
            "network.player_inventory_projection byte limit"
        );
        serde_json::from_str(text)
            .map_err(|error| crate::json_parse_error("network.player_inventory_projection", error))
    }

    /// Read a regular file of at most [`MAX_BINDING_BYTES`], then [`Self::parse`].
    pub fn load(path: &Path) -> Result<Self> {
        let file = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(crate::nonblocking_flag())
            .open(path)
            .map_err(|_| anyhow::anyhow!("open network.player_inventory_projection failed"))?;
        let meta = file.metadata()?;
        ensure!(
            meta.is_file() && meta.len() <= MAX_BINDING_BYTES as u64,
            "network.player_inventory_projection size or type invalid"
        );
        let mut text = String::new();
        file.take(MAX_BINDING_BYTES as u64 + 1)
            .read_to_string(&mut text)
            .map_err(|_| anyhow::anyhow!("read network.player_inventory_projection failed"))?;
        Self::parse(&text)
    }
}

/// The index of the only row named `name`; missing or repeated rows refuse.
fn unique_row(rows: &[String], table: &str, name: &str, limit: usize) -> Result<usize> {
    let mut found = rows
        .iter()
        .enumerate()
        .filter(|(_, row)| row.as_str() == name)
        .map(|(index, _)| index);
    let index = found.next().with_context(|| {
        format!("network.player_inventory_projection: the data.pak has no {table} row {name:?}")
    })?;
    ensure!(
        found.next().is_none(),
        "network.player_inventory_projection: the data.pak lists {table} row {name:?} more than once"
    );
    ensure!(
        index < limit,
        "network.player_inventory_projection: {table} row {name:?} lies past its row handle"
    );
    Ok(index)
}

impl PlayerInventoryProjection {
    /// Check `binding` against the recipe and the pak's row names. `opened`
    /// are the info rows the pawn already opens (the authored profile's
    /// `D_Inventory` Player row): binding one again would be ambiguous, as
    /// the first inventory with an ID wins (F-1064).
    pub fn compile(
        binding: &PlayerInventoryProjectionBinding,
        recipe: &CompiledPlayerInventoryRecipe,
        catalog: ProjectionCatalog<'_>,
        opened: &[u8],
    ) -> Result<Self> {
        const PREFIX: &str = "network.player_inventory_projection";
        ensure!(
            matches!(binding.format, 1 | 2),
            "{PREFIX}: format must be 1 or 2"
        );
        let weighted = binding.format == 2;
        ensure!(
            binding.inventories.len() <= MAX_BOUND_INVENTORIES,
            "{PREFIX}: more than {MAX_BOUND_INVENTORIES} inventories"
        );
        let recipe = recipe.recipe();
        let recipe_inventories: BTreeMap<&str, _> = recipe
            .inventories()
            .iter()
            .map(|inventory| (inventory.key.as_str(), inventory))
            .collect();
        let recipe_items: BTreeMap<&str, _> = recipe
            .items()
            .iter()
            .map(|item| (item.key.as_str(), item))
            .collect();
        ensure!(
            binding.items.len() <= recipe_items.len(),
            "{PREFIX}: more items than the recipe declares"
        );
        let mut inventories = Vec::with_capacity(binding.inventories.len());
        let mut info_rows = BTreeSet::new();
        for bound in &binding.inventories {
            let key = bound.inventory.as_str();
            let definition = recipe_inventories
                .get(key)
                .with_context(|| format!("{PREFIX}: unknown recipe inventory {key:?}"))?;
            ensure!(
                !inventories
                    .iter()
                    .any(|other: &BoundInventory| other.key == key),
                "{PREFIX}: recipe inventory {key:?} is bound twice"
            );
            let slots = definition.slot_rules.len();
            ensure!(
                usize::from(definition.initial_slots) == slots,
                "{PREFIX}: recipe inventory {key:?} needs a fixed slot count (initial_slots equal to its slot rules)"
            );
            ensure!(
                definition.overflow_limit == 0,
                "{PREFIX}: recipe inventory {key:?} needs overflow_limit 0: overflow stacks have no slot"
            );
            ensure!(
                slots <= MAX_BOUND_SLOTS,
                "{PREFIX}: recipe inventory {key:?} has more than {MAX_BOUND_SLOTS} slots"
            );
            ensure!(
                bound.slot_queries.len() == slots,
                "{PREFIX}: recipe inventory {key:?} needs one slot query per slot ({slots})"
            );
            let info = unique_row(
                catalog.inventory_info,
                crate::network::INVENTORY_INFO_TABLE,
                &bound.info_row,
                usize::from(u8::MAX) + 1,
            )?;
            let info = u8::try_from(info)?;
            ensure!(
                !opened.contains(&info),
                "{PREFIX}: {} row {:?} is already opened by the authored D_Inventory Player row",
                crate::network::INVENTORY_INFO_TABLE,
                bound.info_row
            );
            ensure!(
                info_rows.insert(info),
                "{PREFIX}: {} row {:?} is bound twice",
                crate::network::INVENTORY_INFO_TABLE,
                bound.info_row
            );
            let slots = bound
                .slot_queries
                .iter()
                .map(|query| {
                    let row = unique_row(
                        catalog.tag_queries,
                        crate::network::TAG_QUERIES_TABLE,
                        query,
                        NO_ROW,
                    )?;
                    Ok(BoundSlot {
                        query_row: u16::try_from(row)?,
                        max_weight: 0,
                    })
                })
                .collect::<Result<_>>()?;
            inventories.push(BoundInventory {
                key: key.to_owned(),
                info_row: info,
                slots,
            });
        }
        for key in recipe_inventories.keys() {
            ensure!(
                inventories.iter().any(|bound| bound.key == *key),
                "{PREFIX}: recipe inventory {key:?} has no binding"
            );
        }
        inventories.sort_by(|a, b| a.key.cmp(&b.key));
        let mut items = BTreeMap::new();
        let mut rows = BTreeSet::new();
        let mut unit_weights = BTreeMap::new();
        for bound in &binding.items {
            let key = bound.item.as_str();
            let definition = recipe_items
                .get(key)
                .with_context(|| format!("{PREFIX}: unknown recipe item {key:?}"))?;
            ensure!(
                !items.contains_key(key),
                "{PREFIX}: recipe item {key:?} is bound twice"
            );
            let row = u16::try_from(unique_row(
                catalog.items_static,
                crate::network::ITEMS_STATIC_TABLE,
                &bound.row,
                NO_ROW,
            )?)?;
            ensure!(
                rows.insert(row),
                "{PREFIX}: {} row {:?} is bound twice",
                crate::network::ITEMS_STATIC_TABLE,
                bound.row
            );
            ensure!(
                bound.count_property || definition.stack_limit == 1,
                "{PREFIX}: recipe item {key:?} stacks above 1 and needs count_property"
            );
            ensure!(
                bound.unit_weight.is_some() == weighted,
                "{PREFIX}: recipe item {key:?}: unit_weight is required in format 2 and forbidden in format 1"
            );
            if let Some(weight) = bound.unit_weight {
                let weight = i32::try_from(weight).with_context(|| {
                    format!(
                        "{PREFIX}: recipe item {key:?}: unit_weight exceeds signed 32-bit range"
                    )
                })?;
                unit_weights.insert(row, weight);
            }
            let mut properties = BTreeSet::new();
            let mut dynamic = Vec::with_capacity(bound.dynamic.len() + 1);
            for entry in &bound.dynamic {
                ensure!(
                    entry.property < DYNAMIC_PROPERTY_COUNT,
                    "{PREFIX}: recipe item {key:?} names dynamic property {} outside 0..=13",
                    entry.property
                );
                ensure!(
                    entry.property != ITEMABLE_STACK_PROPERTY,
                    "{PREFIX}: recipe item {key:?} sets property 7; use count_property"
                );
                ensure!(
                    entry.property != LINKED_INVENTORY_PROPERTY || entry.value == -1,
                    "{PREFIX}: recipe item {key:?} links an inventory; only -1 is supported"
                );
                ensure!(
                    properties.insert(entry.property),
                    "{PREFIX}: recipe item {key:?} repeats dynamic property {}",
                    entry.property
                );
                dynamic.push((entry.property, entry.value));
            }
            items.insert(
                key.to_owned(),
                BoundItem {
                    row,
                    count_property: bound.count_property,
                    dynamic,
                },
            );
        }
        for key in recipe_items.keys() {
            ensure!(
                items.contains_key(*key),
                "{PREFIX}: recipe item {key:?} has no binding"
            );
        }
        if weighted {
            // Every intermediate page also mixes only valid slots. Bounding
            // them independently covers any combination of held and new slots.
            for inventory in &mut inventories {
                let mut total = 0_i32;
                for (slot, rule) in inventory
                    .slots
                    .iter_mut()
                    .zip(&recipe_inventories[inventory.key.as_str()].slot_rules)
                {
                    let max_weight = rule
                        .allowed_items
                        .iter()
                        .map(|key| {
                            i64::from(unit_weights[&items[key].row])
                                * i64::from(recipe_items[key.as_str()].stack_limit)
                        })
                        .max()
                        .unwrap_or(0);
                    let overflow = || {
                        format!(
                            "{PREFIX}: recipe inventory {:?}: maximum CurrentWeight exceeds signed 32-bit range",
                            inventory.key
                        )
                    };
                    slot.max_weight = i32::try_from(max_weight).with_context(overflow)?;
                    total = total.checked_add(slot.max_weight).with_context(overflow)?;
                }
            }
        }
        Ok(Self {
            inventories,
            items,
            unit_weights: weighted.then(|| Arc::new(unit_weights)),
        })
    }

    /// The bound inventories as every pawn opens them: empty, with the
    /// captured open key (F-799), in recipe-key order.
    pub fn opened_inventories(&self) -> Vec<PawnInventory> {
        self.inventories
            .iter()
            .map(|bound| PawnInventory {
                info_row: bound.info_row,
                current_weight: None,
                slots: FastArrayContent {
                    replication_key: crate::network::INVENTORY_SLOTS_REPLICATION_KEY,
                    items: bound
                        .slots
                        .iter()
                        .zip(0..)
                        .map(|(slot, index)| InventorySlot::empty(slot.query_row, index))
                        .collect(),
                },
            })
            .collect()
    }

    fn item(&self, stack: &StackV1) -> Result<SlotItem> {
        let bound = self
            .items
            .get(&stack.item)
            .context("a projected stack's item is unbound")?;
        let mut dynamic = bound.dynamic.clone();
        if bound.count_property {
            dynamic.push((ITEMABLE_STACK_PROPERTY, i32::try_from(stack.count)?));
        } else {
            ensure!(stack.count == 1, "an uncounted item stacks above 1");
        }
        dynamic.sort_unstable_by_key(|(property, _)| *property);
        Ok(SlotItem {
            dynamic,
            ..SlotItem::new(bound.row)?
        })
    }

    /// The wire slots of each bound inventory (recipe-key order) for a bag's
    /// `inventories`.
    pub fn project(&self, inventories: &[InventoryStateV1]) -> Result<Vec<Vec<InventorySlot>>> {
        self.inventories
            .iter()
            .map(|bound| {
                let state = inventories
                    .iter()
                    .find(|inventory| inventory.key == bound.key)
                    .context("a projected bag lacks a bound inventory")?;
                ensure!(
                    state.overflow.is_empty() && state.slots.len() == bound.slots.len(),
                    "a projected bag inventory differs from its binding"
                );
                state
                    .slots
                    .iter()
                    .zip(&bound.slots)
                    .zip(0..)
                    .map(|((stack, bound), index)| {
                        let slot = InventorySlot {
                            query_row: bound.query_row,
                            index,
                            item: stack.as_ref().map(|stack| self.item(stack)).transpose()?,
                        };
                        if let Some(weights) = &self.unit_weights {
                            ensure!(
                                slot_weight(weights, &slot)? <= bound.max_weight,
                                "a projected slot exceeds its maximum weight"
                            );
                        }
                        Ok(slot)
                    })
                    .collect()
            })
            .collect()
    }
}

fn slot_weight(unit_weights: &BTreeMap<u16, i32>, slot: &InventorySlot) -> Result<i32> {
    let Some(item) = &slot.item else {
        return Ok(0);
    };
    let unit = unit_weights
        .get(&item.row)
        .context("a projected item has no unit weight")?;
    let count = item
        .dynamic
        .iter()
        .find_map(|&(property, value)| (property == ITEMABLE_STACK_PROPERTY).then_some(value))
        .unwrap_or(1);
    ensure!(count > 0, "a projected item has a non-positive count");
    unit.checked_mul(count)
        .context("a projected slot's weight overflows")
}

/// The key a client holds for an opened inventory's slots: the open's key
/// plus one per slot it opened with (F-978).
pub fn client_key(opened: &PawnInventory) -> Result<i32> {
    opened
        .slots
        .replication_key
        .checked_add(i32::try_from(opened.slots.items.len())?)
        .context("an inventory's replication key overflows")
}

/// One peer's projection: what its client holds once every enqueued update
/// arrives, and the latest bag it should hold. Latest only; no queue.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PeerBagProjection {
    /// Pawn subobject index of each bound inventory.
    subobjects: Vec<usize>,
    /// The client's slots and key per bound inventory.
    held: Vec<PawnInventory>,
    /// Shared authored weights for this binding, when enabled.
    unit_weights: Option<Arc<BTreeMap<u16, i32>>>,
    /// The latest bag revision and its wire slots.
    target: Option<(u64, Vec<Vec<InventorySlot>>)>,
    /// The bag revision the held state reached when last enqueued.
    enqueued: Option<u64>,
    /// The player record revision last checked for a bag change.
    seen_player_revision: Option<u64>,
    /// Set once the player's leave cut-off exists; never cleared.
    frozen: bool,
}

impl PeerBagProjection {
    /// After the pawn open carried `projection`'s inventories as subobjects
    /// `first_subobject..`: the client holds them empty at F-978's key.
    pub fn opened(projection: &PlayerInventoryProjection, first_subobject: usize) -> Result<Self> {
        let opened = projection.opened_inventories();
        let held = opened
            .into_iter()
            .map(|mut inventory| {
                inventory.slots.replication_key = client_key(&inventory)?;
                Ok(inventory)
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(Self {
            subobjects: (first_subobject..first_subobject + held.len()).collect(),
            held,
            unit_weights: projection.unit_weights.clone(),
            target: None,
            enqueued: None,
            seen_player_revision: None,
            frozen: false,
        })
    }

    /// Stop all further bag traffic for this connection: a leave cut-off
    /// froze the player (the registry's `is_leaving`). Permanent; nothing
    /// pending is ever enqueued afterwards.
    pub fn freeze(&mut self) {
        self.frozen = true;
    }

    /// Whether [`Self::freeze`] was called.
    pub fn is_frozen(&self) -> bool {
        self.frozen
    }

    /// Test-only crafted state: the key the client is taken to hold for
    /// bound inventory `inventory`, so a test can force a delta whose key
    /// cannot follow (a permanent failure).
    #[cfg(test)]
    pub(crate) fn force_held_key(&mut self, inventory: usize, key: i32) {
        self.held[inventory].slots.replication_key = key;
    }

    /// Whether the bag must be looked up for this player record revision:
    /// every applied bag action also advances the player's record revision.
    pub fn needs_lookup(&self, player_revision: u64) -> bool {
        !self.frozen && self.seen_player_revision != Some(player_revision)
    }

    /// Note the bag at `revision`, projecting it only when its revision is
    /// new. Returns whether the target changed.
    pub fn observe(
        &mut self,
        player_revision: u64,
        revision: u64,
        project: impl FnOnce() -> Result<Vec<Vec<InventorySlot>>>,
    ) -> Result<bool> {
        if self
            .target
            .as_ref()
            .is_some_and(|(current, _)| *current == revision)
        {
            self.seen_player_revision = Some(player_revision);
            return Ok(false);
        }
        let slots = project()?;
        ensure!(
            slots.len() == self.held.len()
                && slots
                    .iter()
                    .zip(&self.held)
                    .all(|(target, held)| target.len() == held.slots.items.len()),
            "a projected bag differs from the opened inventories"
        );
        self.seen_player_revision = Some(player_revision);
        self.target = Some((revision, slots));
        Ok(true)
    }

    /// The bag revision last fully enqueued, if any.
    pub fn enqueued_revision(&self) -> Option<u64> {
        self.enqueued
    }

    /// The latest bag revision known, if any.
    pub fn target_revision(&self) -> Option<u64> {
        self.target.as_ref().map(|(revision, _)| *revision)
    }

    /// Whether the latest bag still has to be enqueued; never once frozen.
    pub fn due(&self) -> bool {
        !self.frozen
            && self
                .target
                .as_ref()
                .is_some_and(|(revision, _)| self.enqueued != Some(*revision))
    }

    /// The slots each bound inventory's client holds.
    pub fn held(&self) -> &[PawnInventory] {
        &self.held
    }

    /// The next update's slot deltas toward the target, at most
    /// `max_changes` changed slots (inventory, then slot order). Each delta's
    /// base is the key the client holds; its key adds one per changed slot
    /// (F-978). Format 2 calculates the resulting weight from the slots in
    /// this page and the held slots it leaves alone. Empty when the client
    /// already holds the target.
    pub fn deltas(&self, max_changes: usize) -> Result<Vec<InventoryRestore>> {
        let Some((_, target)) = &self.target else {
            return Ok(Vec::new());
        };
        let mut budget = max_changes;
        let mut deltas = Vec::new();
        for ((held, target), subobject) in self.held.iter().zip(target).zip(&self.subobjects) {
            if budget == 0 {
                break;
            }
            let mut changes = Vec::new();
            let mut items = held.slots.items.clone();
            for ((index, current), wanted) in held.slots.items.iter().enumerate().zip(target) {
                if budget == 0 {
                    break;
                }
                if current != wanted {
                    changes.push((
                        i32::try_from(index + 1)?,
                        SlotChange::Changed(wanted.clone()),
                    ));
                    items[index] = wanted.clone();
                    budget -= 1;
                }
            }
            if changes.is_empty() {
                continue;
            }
            let base = held.slots.replication_key;
            let key = base
                .checked_add(i32::try_from(changes.len())?)
                .context("an inventory's replication key overflows")?;
            let weight = self
                .unit_weights
                .as_ref()
                .map(|weights| {
                    items.iter().try_fold(0_i32, |total, slot| {
                        total
                            .checked_add(slot_weight(weights, slot)?)
                            .context("a projected inventory's weight overflows")
                    })
                })
                .transpose()?;
            deltas.push(InventoryRestore {
                inventory: *subobject,
                opened: held.clone(),
                restored: PawnInventory {
                    current_weight: weight,
                    slots: FastArrayContent {
                        replication_key: key,
                        items,
                    },
                    ..held.clone()
                },
                base_key: base,
                changes,
                weight,
            });
        }
        Ok(deltas)
    }

    /// Record that `deltas` (from [`Self::deltas`]) were enqueued; once the
    /// client holds the target, its revision counts as enqueued.
    pub fn commit(&mut self, deltas: Vec<InventoryRestore>) {
        for delta in deltas {
            if let Some(position) = self
                .subobjects
                .iter()
                .position(|subobject| *subobject == delta.inventory)
            {
                self.held[position] = delta.restored;
            }
        }
        if let Some((revision, target)) = &self.target
            && self
                .held
                .iter()
                .zip(target)
                .all(|(held, target)| held.slots.items == *target)
        {
            self.enqueued = Some(*revision);
        }
    }
}

#[cfg(test)]
mod tests;

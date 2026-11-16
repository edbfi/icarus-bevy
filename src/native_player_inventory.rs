//! Fresh-store, independently authored player bags. No stock inventory protocol.
//! Only the exclusive-world runtime adapter can commit live inventory changes.
pub use crate::native_inventory::{
    PlayerBagV1, PlayerConsumableV2, PlayerInventoryOwnerV1, PlayerInventoryRecipe,
    PlayerInventoryRecipeV1, PlayerInventoryRecipeV2, PlayerInventorySnapshotV1,
    PlayerOverflowDestinationV1, PlayerSlotEndpointV1, PlayerStackEndpointV1, PlayerStarterItemV1,
};
use crate::{
    native_inventory::*, native_lifecycle::OwnerHandle, native_players::PlayerSnapshot,
    simulation::DomainId,
};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::Read,
    os::unix::fs::OpenOptionsExt,
    path::Path,
};

pub(crate) fn deserialize_optional<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> std::result::Result<Option<PlayerInventorySnapshotV1>, D::Error> {
    PlayerInventorySnapshotV1::deserialize(d).map(Some)
}

#[derive(Clone, PartialEq)]
pub struct CompiledPlayerInventoryRecipe {
    recipe: PlayerInventoryRecipe,
    identity: String,
    baseline: InventoryInitialTotalsV1,
}
impl std::fmt::Debug for CompiledPlayerInventoryRecipe {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CompiledPlayerInventoryRecipe { .. }")
    }
}
impl CompiledPlayerInventoryRecipe {
    pub fn compile(recipe: PlayerInventoryRecipeV1) -> Result<Self> {
        Self::compile_versioned(recipe.into())
    }
    pub fn compile_versioned(mut recipe: PlayerInventoryRecipe) -> Result<Self> {
        player_recipe_charge(&recipe)?;
        let domain = recipe_domain(&recipe)?;
        let (items, inventories) = match &mut recipe {
            PlayerInventoryRecipe::V1(v) => (&mut v.items, &mut v.inventories),
            PlayerInventoryRecipe::V2(v) => {
                v.consumables.sort_by(|a, b| a.item.cmp(&b.item));
                (&mut v.items, &mut v.inventories)
            }
        };
        items.sort_by(|a, b| a.key.cmp(&b.key));
        inventories.sort_by(|a, b| a.key.cmp(&b.key));
        for inventory in inventories {
            for rule in &mut inventory.slot_rules {
                rule.allowed_items.sort();
            }
        }
        let identity = recipe_identity(&recipe, domain)?;
        let contracts =
            CompiledInventoryContractsV1::player_bound(&recipe, [1].into_iter(), &identity)?;
        let live = InventoryLiveOwnersV1 {
            owners: vec![handle(1)],
        };
        let initial =
            prepare_initial_inventory_from(&contracts, &live, &starters(&recipe, 1), 0, 1)?;
        let compiled = Self {
            recipe,
            identity,
            baseline: initial.initial_totals().clone(),
        };
        // Do not overlap the routed starter state/core adapter with the empty
        // retained clone below, including during complete-generation preflight.
        drop(initial);
        drop(contracts);
        // Startup preparation must reject an inadmissible retained recipe
        // before any store or service is opened, including spare capacities.
        player_inventory_charge(&compiled.empty(), 8)?;
        Ok(compiled)
    }
    pub fn parse(text: &str) -> Result<Self> {
        ensure!(
            text.len() <= MAX_INVENTORY_CONTRACT_BYTES,
            "player inventory recipe byte limit"
        );
        let recipe = serde_json::from_str(text)
            .map_err(|_| anyhow::anyhow!("invalid network.player_inventory"))?;
        Self::compile_versioned(recipe)
    }
    pub fn load(path: &Path) -> Result<Self> {
        let file = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(crate::nonblocking_flag())
            .open(path)
            .map_err(|_| anyhow::anyhow!("open network.player_inventory failed"))?;
        let meta = file.metadata()?;
        ensure!(
            meta.is_file() && meta.len() <= MAX_INVENTORY_CONTRACT_BYTES as u64,
            "network.player_inventory size or type invalid"
        );
        let mut text = String::new();
        file.take(MAX_INVENTORY_CONTRACT_BYTES as u64 + 1)
            .read_to_string(&mut text)
            .map_err(|_| anyhow::anyhow!("read network.player_inventory failed"))?;
        Self::parse(&text)
    }
    pub fn identity(&self) -> &str {
        &self.identity
    }
    pub fn empty(&self) -> PlayerInventorySnapshotV1 {
        PlayerInventorySnapshotV1 {
            format: 1,
            recipe_id: self.identity.clone(),
            recipe: self.recipe.clone(),
            baseline: self.baseline.clone(),
            revision: 1,
            next_stack_id: 1,
            bags: vec![],
        }
    }
}
// Semantic validation is shared by compilation and borrowed retained validation.
// Revalidation must not clone the full recipe or repeat startup's empty clone.
fn recipe_domain(recipe: &PlayerInventoryRecipe) -> Result<&'static [u8]> {
    match recipe {
        PlayerInventoryRecipe::V1(v) => {
            ensure!(v.format == 1, "player inventory recipe invalid");
            Ok(b"authored-player-inventory-recipe-v1\0")
        }
        PlayerInventoryRecipe::V2(v) => {
            ensure!(
                v.format == 2 && !v.consumables.is_empty(),
                "player consumables invalid"
            );
            let mut seen = BTreeSet::new();
            for effect in &v.consumables {
                let gains = [effect.oxygen, effect.water, effect.food];
                ensure!(
                    seen.insert(&effect.item)
                        && v.items.iter().any(|i| i.key == effect.item)
                        && gains.iter().all(|g| *g <= 10_000_000)
                        && gains.iter().any(|g| *g > 0),
                    "player consumables invalid"
                );
            }
            Ok(b"authored-player-inventory-recipe-v2\0")
        }
    }
}
fn recipe_identity(recipe: &PlayerInventoryRecipe, domain: &[u8]) -> Result<String> {
    let mut hash = Sha256::new();
    hash.update(domain);
    hash.update(serde_json::to_vec(recipe)?);
    Ok(crate::hex_lower(&hash.finalize()))
}
fn validate_retained_recipe(section: &PlayerInventorySnapshotV1) -> Result<()> {
    let recipe = &section.recipe;
    player_recipe_charge(recipe)?;
    let domain = recipe_domain(recipe)?;
    ensure!(
        recipe.items().is_sorted_by(|a, b| a.key <= b.key)
            && recipe.inventories().is_sorted_by(|a, b| a.key <= b.key)
            && recipe
                .inventories()
                .iter()
                .all(|i| i.slot_rules.iter().all(|r| r.allowed_items.is_sorted()))
            && match recipe {
                PlayerInventoryRecipe::V1(_) => true,
                PlayerInventoryRecipe::V2(v) => v.consumables.is_sorted_by(|a, b| a.item <= b.item),
            },
        "player inventory retained recipe is not canonical"
    );
    ensure!(
        recipe_identity(recipe, domain)? == section.recipe_id,
        "player inventory recipe identity mismatch"
    );
    let contracts =
        CompiledInventoryContractsV1::player_bound(recipe, [1].into_iter(), &section.recipe_id)?;
    let initial = prepare_initial_inventory_from(
        &contracts,
        &InventoryLiveOwnersV1 {
            owners: vec![handle(1)],
        },
        &starters(recipe, 1),
        0,
        1,
    )?;
    ensure!(
        initial.initial_totals() == &section.baseline,
        "player inventory immutable baseline mismatch"
    );
    Ok(())
}
fn handle(id: u64) -> OwnerHandle {
    OwnerHandle {
        id: DomainId(id),
        incarnation: 1,
    }
}
fn starters(recipe: &PlayerInventoryRecipe, id: u64) -> InitialInventoryItemsV1 {
    InitialInventoryItemsV1 {
        items: recipe
            .starter()
            .iter()
            .map(|s| InitialInventoryItemV1 {
                owner: DomainId(id),
                inventory: s.inventory.clone(),
                requested_slot: s.requested_slot,
                item: s.item.clone(),
                count: s.count,
            })
            .collect(),
    }
}
fn zero(baseline: &InventoryInitialTotalsV1) -> Vec<ItemTotalV1> {
    baseline
        .items
        .iter()
        .map(|i| ItemTotalV1 {
            item: i.item.clone(),
            count: 0,
        })
        .collect()
}
fn local(section: &PlayerInventorySnapshotV1, bag: &PlayerBagV1) -> InventoryStateSnapshotV1 {
    InventoryStateSnapshotV1 {
        format: 1,
        contract_id: section.recipe_id.clone(),
        revision: bag.revision,
        next_stack_id: section.next_stack_id,
        consumed: bag.consumed.clone(),
        owners: vec![OwnerInventoriesV1 {
            owner: handle(bag.owner.player_id),
            inventories: bag.inventories.clone(),
        }],
    }
}
impl PlayerInventorySnapshotV1 {
    /// Complete owner-local conservation and global uniqueness, before activation.
    pub fn validate(&self, players: &PlayerSnapshot) -> Result<()> {
        player_inventory_charge(self, 8)?;
        players.validate()?;
        ensure!(
            self.format == 1 && self.revision > 0 && self.next_stack_id > 0,
            "player inventory header invalid"
        );
        validate_retained_recipe(self)?;
        let records: BTreeMap<_, _> = players.records.iter().map(|r| (r.id, r)).collect();
        ensure!(
            self.bags.len() == records.len(),
            "player inventory coverage invalid"
        );
        let issued = self
            .recipe
            .starter()
            .len()
            .checked_mul(self.bags.len())
            .and_then(|n| u64::try_from(n).ok())
            .and_then(|n| n.checked_add(1))
            .context("player inventory allocator exhausted")?;
        ensure!(
            self.next_stack_id >= issued,
            "player inventory allocator precedes issuance"
        );
        let mut seen = BTreeSet::new();
        let mut stacks = BTreeSet::new();
        let mut instances = 0usize;
        let contracts = CompiledInventoryContractsV1::player_bound(
            &self.recipe,
            records.keys().copied(),
            &self.recipe_id,
        )?;
        for bag in &self.bags {
            ensure!(
                bag.owner.incarnation == 1
                    && records
                        .get(&bag.owner.player_id)
                        .is_some_and(|record| record.revision >= bag.revision)
                    && seen.insert(bag.owner.player_id)
                    && bag.revision > 0,
                "player inventory owner invalid"
            );
            let contracts = contracts.for_owner(DomainId(bag.owner.player_id))?;
            validate_inventory_state(
                &contracts,
                &local(self, bag),
                &self.baseline,
                &InventoryLiveOwnersV1 {
                    owners: vec![handle(bag.owner.player_id)],
                },
            )?;
            instances = instances
                .checked_add(bag.inventories.len())
                .context("player inventory instance limit")?;
            ensure!(
                instances <= MAX_INVENTORY_INSTANCES,
                "player inventory instance limit"
            );
            for inventory in &bag.inventories {
                for stack in inventory
                    .slots
                    .iter()
                    .flatten()
                    .chain(inventory.overflow.iter().map(|s| &s.stack))
                {
                    ensure!(
                        stacks.insert(stack.id) && stacks.len() <= MAX_INVENTORY_STACKS,
                        "player inventory stack identity or limit invalid"
                    );
                }
            }
        }
        self.consumed_totals()?;
        Ok(())
    }
    /// Derived only after per-player validation; never an admission baseline.
    fn consumed_totals(&self) -> Result<Vec<ItemTotalV1>> {
        let mut totals: BTreeMap<_, u64> = self
            .baseline
            .items
            .iter()
            .map(|i| (i.item.as_str(), 0))
            .collect();
        for bag in &self.bags {
            for entry in &bag.consumed {
                let count = totals
                    .get_mut(entry.item.as_str())
                    .context("player inventory consumed item invalid")?;
                *count = count
                    .checked_add(entry.count)
                    .context("player inventory quantity exhausted")?;
            }
        }
        Ok(totals
            .into_iter()
            .map(|(item, count)| ItemTotalV1 {
                item: item.to_owned(),
                count,
            })
            .collect())
    }
    pub fn bag(&self, player_id: u64) -> Option<&PlayerBagV1> {
        self.bags.iter().find(|b| b.owner.player_id == player_id)
    }
    pub(crate) fn issue(&self, id: u64) -> Result<Self> {
        player_inventory_charge(self, 8)?;
        ensure!(
            self.bag(id).is_none() && self.bags.len() < 4096,
            "player inventory issuance conflict"
        );
        let contracts = CompiledInventoryContractsV1::player_bound(
            &self.recipe,
            [id].into_iter(),
            &self.recipe_id,
        )?;
        // The shared router admits expansion before allocating slots and stacks.
        let initial = prepare_initial_inventory_from(
            &contracts,
            &InventoryLiveOwnersV1 {
                owners: vec![handle(id)],
            },
            &starters(&self.recipe, id),
            player_inventory_bytes(self)?
                .checked_mul(8)
                .context("player inventory allocation limit")?,
            self.next_stack_id,
        )?;
        let (state, baseline, _) = initial.into_parts();
        ensure!(
            baseline == self.baseline,
            "player inventory issuance baseline invalid"
        );
        let mut next = self.clone();
        next.revision = next
            .revision
            .checked_add(1)
            .context("player inventory revision exhausted")?;
        next.next_stack_id = state.next_stack_id;
        next.bags.push(PlayerBagV1 {
            owner: PlayerInventoryOwnerV1 {
                player_id: id,
                incarnation: 1,
            },
            revision: 1,
            consumed: zero(&self.baseline),
            inventories: state
                .owners
                .into_iter()
                .next()
                .context("player inventory issuance missing")?
                .inventories,
        });
        next.bags.sort_by_key(|b| b.owner);
        player_inventory_charge(&next, 8)?;
        Ok(next)
    }
    fn prepare_local(
        &self,
        request: &PlayerInventoryRequestV1,
    ) -> Result<(InventoryStateSnapshotV1, InventoryActionOutcomeV1)> {
        player_inventory_charge(self, 8)?;
        player_action_bounds(request)?;
        let bag = self
            .bag(request.player_id)
            .context("player inventory missing")?;
        ensure!(
            bag.revision == request.expected_bag_revision,
            "player inventory revision conflict"
        );
        let action = request.action.lower(bag.owner)?;
        let contracts = CompiledInventoryContractsV1::player_bound(
            &self.recipe,
            [request.player_id].into_iter(),
            &self.recipe_id,
        )?;
        let result = prepare_inventory_action(
            &contracts,
            &local(self, bag),
            &self.baseline,
            &InventoryLiveOwnersV1 {
                owners: vec![handle(request.player_id)],
            },
            &InventoryActionRequestV1 {
                expected_inventory_revision: bag.revision,
                action,
            },
        )?;
        let outcome = result.outcome();
        if outcome == InventoryActionOutcomeV1::Applied {
            self.revision
                .checked_add(1)
                .context("player inventory revision exhausted")?;
        }
        Ok((result.into_state(), outcome))
    }
}
#[derive(Clone, PartialEq, Serialize)]
pub struct PlayerInventoryRequestV1 {
    pub player_id: u64,
    pub expected_player_revision: u64,
    pub expected_bag_revision: u64,
    pub action: PlayerInventoryActionV1,
}
impl<'de> Deserialize<'de> for PlayerInventoryRequestV1 {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Fields {
            player_id: u64,
            expected_player_revision: u64,
            expected_bag_revision: u64,
            action: PlayerInventoryActionV1,
        }
        let fields: Fields = crate::json::object(d).map_err(|_| {
            <D::Error as serde::de::Error>::custom("player inventory request invalid")
        })?;
        Ok(Self {
            player_id: fields.player_id,
            expected_player_revision: fields.expected_player_revision,
            expected_bag_revision: fields.expected_bag_revision,
            action: fields.action,
        })
    }
}
impl std::fmt::Debug for PlayerInventoryRequestV1 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PlayerInventoryRequestV1 { .. }")
    }
}
#[derive(Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum PlayerInventoryActionV1 {
    Move {
        source: PlayerStackEndpointV1,
        destination: PlayerSlotEndpointV1,
    },
    Split {
        source: PlayerStackEndpointV1,
        destination: PlayerSlotEndpointV1,
        amount: u32,
    },
    Merge {
        source: PlayerStackEndpointV1,
        destination: PlayerStackEndpointV1,
        amount: u32,
    },
    Consume {
        source: PlayerStackEndpointV1,
        amount: u32,
    },
    Use {
        source: PlayerStackEndpointV1,
    },
    PlaceOverflow {
        source: PlayerStackEndpointV1,
        destination: PlayerOverflowDestinationV1,
    },
}
impl std::fmt::Debug for PlayerInventoryActionV1 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PlayerInventoryActionV1 { .. }")
    }
}
impl PlayerInventoryActionV1 {
    fn lower(&self, owner: PlayerInventoryOwnerV1) -> Result<InventoryActionV1> {
        let stack = |s: &PlayerStackEndpointV1| -> Result<_> {
            ensure!(
                s.owner == owner,
                "player inventory cross-owner endpoint refused"
            );
            Ok(InventoryStackEndpointV1 {
                owner: handle(owner.player_id),
                inventory: s.inventory.clone(),
                stack_id: s.stack_id,
            })
        };
        let slot = |s: &PlayerSlotEndpointV1| -> Result<_> {
            ensure!(
                s.owner == owner,
                "player inventory cross-owner endpoint refused"
            );
            Ok(InventorySlotEndpointV1 {
                owner: handle(owner.player_id),
                inventory: s.inventory.clone(),
                slot: s.slot,
            })
        };
        Ok(match self {
            Self::Move {
                source,
                destination,
            } => InventoryActionV1::Move {
                source: stack(source)?,
                destination: slot(destination)?,
            },
            Self::Split {
                source,
                destination,
                amount,
            } => InventoryActionV1::Split {
                source: stack(source)?,
                destination: slot(destination)?,
                amount: *amount,
            },
            Self::Merge {
                source,
                destination,
                amount,
            } => InventoryActionV1::Merge {
                source: stack(source)?,
                destination: stack(destination)?,
                amount: *amount,
            },
            Self::Consume { source, amount } => InventoryActionV1::Consume {
                source: stack(source)?,
                amount: *amount,
            },
            Self::Use { source } => InventoryActionV1::Consume {
                source: stack(source)?,
                amount: 1,
            },
            Self::PlaceOverflow {
                source,
                destination,
            } => {
                ensure!(
                    destination.owner == owner,
                    "player inventory cross-owner endpoint refused"
                );
                InventoryActionV1::PlaceOverflow {
                    source: stack(source)?,
                    destination: InventoryOverflowDestinationV1 {
                        owner: handle(owner.player_id),
                        inventory: destination.inventory.clone(),
                        slot: destination.slot,
                    },
                }
            }
        })
    }
}
/// Offline operator authority under the caller's held store lock. This edits a
/// prepared snapshot only; durable publication belongs to SnapshotStore::save.
pub fn prepare_offline(
    snapshot: &crate::simulation::WorldSnapshot,
    request: &PlayerInventoryRequestV1,
) -> Result<(crate::simulation::WorldSnapshot, InventoryActionOutcomeV1)> {
    prepare_generation(std::borrow::Cow::Borrowed(snapshot), request)
}

// The exclusive runtime adapter already owns its coherent capture. Reuse it
// rather than retaining a second full aggregate through final preflight.
pub(crate) fn prepare_owned(
    snapshot: crate::simulation::WorldSnapshot,
    request: &PlayerInventoryRequestV1,
) -> Result<(crate::simulation::WorldSnapshot, InventoryActionOutcomeV1)> {
    prepare_generation(std::borrow::Cow::Owned(snapshot), request)
}

fn prepare_generation(
    snapshot: std::borrow::Cow<'_, crate::simulation::WorldSnapshot>,
    request: &PlayerInventoryRequestV1,
) -> Result<(crate::simulation::WorldSnapshot, InventoryActionOutcomeV1)> {
    player_action_bounds(request)?;
    snapshot.validate()?;
    ensure!(
        snapshot.schema_version == crate::simulation::SNAPSHOT_SCHEMA_PLAYER_INVENTORY,
        "player inventory requires schema 15"
    );
    let record = snapshot
        .native_players
        .as_ref()
        .context("players missing")?
        .records
        .iter()
        .find(|r| r.id == request.player_id)
        .context("player missing")?;
    ensure!(
        record.revision == request.expected_player_revision,
        "player revision conflict"
    );
    let checkpoint = if let PlayerInventoryActionV1::Use { source } = &request.action {
        let proposed = prepare_use(&snapshot, request, source, record.checkpoint)?;
        if proposed == record.checkpoint {
            return Ok((snapshot.into_owned(), InventoryActionOutcomeV1::NoChange));
        }
        Some(proposed)
    } else {
        None
    };
    // Fail changed Use before preparing either coupled domain on exhaustion.
    let revision = record.revision.checked_add(1);
    if checkpoint.is_some() {
        ensure!(revision.is_some(), "player revision exhausted");
    }
    let (inventory, outcome) = snapshot
        .player_inventory
        .as_ref()
        .context("player inventory missing")?
        .prepare_local(request)?;
    let mut next = snapshot.into_owned();
    if outcome == InventoryActionOutcomeV1::Applied {
        let record = next
            .native_players
            .as_mut()
            .context("players missing")?
            .records
            .iter_mut()
            .find(|r| r.id == request.player_id)
            .context("player missing")?;
        record.revision = revision.context("player revision exhausted")?;
        if let Some(checkpoint) = checkpoint {
            record.checkpoint = checkpoint;
        }
        let section = next
            .player_inventory
            .as_mut()
            .context("player inventory missing")?;
        section.revision = section
            .revision
            .checked_add(1)
            .context("player inventory revision exhausted")?;
        section.next_stack_id = inventory.next_stack_id;
        let bag = section
            .bags
            .iter_mut()
            .find(|b| b.owner.player_id == request.player_id)
            .context("player inventory missing")?;
        bag.revision = inventory.revision;
        bag.consumed = inventory.consumed;
        bag.inventories = inventory
            .owners
            .into_iter()
            .next()
            .context("player inventory missing")?
            .inventories;
        crate::persistence::SnapshotStore::preflight(&next)?;
    }
    Ok((next, outcome))
}

// Resolve and cap from validated authoritative state before any Consume planner
// can demand a next revision. No full-state clone or unchecked mutation here.
fn prepare_use(
    snapshot: &crate::simulation::WorldSnapshot,
    request: &PlayerInventoryRequestV1,
    source: &PlayerStackEndpointV1,
    mut checkpoint: crate::native_players::PlayerCheckpoint,
) -> Result<crate::native_players::PlayerCheckpoint> {
    let section = snapshot
        .player_inventory
        .as_ref()
        .context("player inventory missing")?;
    let bag = section
        .bag(request.player_id)
        .context("player inventory missing")?;
    ensure!(
        bag.revision == request.expected_bag_revision,
        "player inventory revision conflict"
    );
    ensure!(
        source.owner == bag.owner,
        "player inventory cross-owner endpoint refused"
    );
    let inventory = bag
        .inventories
        .iter()
        .find(|i| i.key == source.inventory)
        .context("player inventory source missing")?;
    let stack = inventory
        .slots
        .iter()
        .flatten()
        .chain(inventory.overflow.iter().map(|s| &s.stack))
        .find(|s| s.id == source.stack_id)
        .context("player inventory source missing")?;
    ensure!(stack.count > 0, "player inventory source empty");
    let effect = section
        .recipe
        .effect(&stack.item)
        .context("player inventory item is not consumable")?;
    let gain = |current: i32, maximum: i32, amount: u32| -> Result<i32> {
        let wide = i64::from(current)
            .checked_add(i64::from(amount))
            .context("player vital overflow")?;
        Ok(i32::try_from(wide.min(i64::from(maximum)))?)
    };
    let v = &mut checkpoint.vitals;
    v.oxygen = gain(v.oxygen, v.max_oxygen, effect.oxygen)?;
    v.water = gain(v.water, v.max_water, effect.water)?;
    v.food = gain(v.food, v.max_food, effect.food)?;
    Ok(checkpoint)
}

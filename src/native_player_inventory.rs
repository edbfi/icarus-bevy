//! Fresh-store, independently authored player bags. No stock inventory protocol.
//! Only the exclusive-world runtime adapter can commit live inventory changes.
pub use crate::native_inventory::{
    PlayerBagV1, PlayerBagV2, PlayerConsumableV2, PlayerInventoryOwnerV1, PlayerInventoryRecipe,
    PlayerInventoryRecipeV1, PlayerInventoryRecipeV2, PlayerInventoryRevisionPairV1,
    PlayerInventorySnapshot, PlayerInventorySnapshotV1, PlayerInventorySnapshotV2,
    PlayerInventoryTransferRequestV1, PlayerOverflowDestinationV1, PlayerSlotEndpointV1,
    PlayerStackEndpointV1, PlayerStarterItemV1,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlayerInventoryMode {
    LocalV1,
    TransfersV2,
}
impl PlayerInventoryMode {
    pub const fn snapshot_schema(self) -> u32 {
        match self {
            Self::LocalV1 => 15,
            Self::TransfersV2 => 16,
        }
    }
}
pub struct PlayerBagRef<'a> {
    pub owner: PlayerInventoryOwnerV1,
    pub revision: u64,
    pub consumed: &'a Vec<ItemTotalV1>,
    pub inventories: &'a Vec<InventoryStateV1>,
}
impl PlayerInventorySnapshot {
    pub fn mode(&self) -> PlayerInventoryMode {
        match self {
            Self::V1(_) => PlayerInventoryMode::LocalV1,
            Self::V2(_) => PlayerInventoryMode::TransfersV2,
        }
    }
    pub fn recipe_id(&self) -> &str {
        match self {
            Self::V1(v) => &v.recipe_id,
            Self::V2(v) => &v.recipe_id,
        }
    }
    pub fn recipe(&self) -> &PlayerInventoryRecipe {
        match self {
            Self::V1(v) => &v.recipe,
            Self::V2(v) => &v.recipe,
        }
    }
    pub fn revision(&self) -> u64 {
        match self {
            Self::V1(v) => v.revision,
            Self::V2(v) => v.revision,
        }
    }
    pub fn next_stack_id(&self) -> u64 {
        match self {
            Self::V1(v) => v.next_stack_id,
            Self::V2(v) => v.next_stack_id,
        }
    }
    pub fn as_v1(&self) -> Option<&PlayerInventorySnapshotV1> {
        match self {
            Self::V1(v) => Some(v),
            _ => None,
        }
    }
    pub fn as_v1_mut(&mut self) -> Option<&mut PlayerInventorySnapshotV1> {
        match self {
            Self::V1(v) => Some(v),
            _ => None,
        }
    }
    pub fn as_v2(&self) -> Option<&PlayerInventorySnapshotV2> {
        match self {
            Self::V2(v) => Some(v),
            _ => None,
        }
    }
    pub fn as_v2_mut(&mut self) -> Option<&mut PlayerInventorySnapshotV2> {
        match self {
            Self::V2(v) => Some(v),
            _ => None,
        }
    }
    pub fn bag(&self, id: u64) -> Option<PlayerBagRef<'_>> {
        match self {
            Self::V1(v) => v.bag(id).map(|b| PlayerBagRef {
                owner: b.owner,
                revision: b.revision,
                consumed: &b.consumed,
                inventories: &b.inventories,
            }),
            Self::V2(v) => v.bag(id).map(|b| PlayerBagRef {
                owner: b.owner,
                revision: b.revision,
                consumed: &b.consumed,
                inventories: &b.inventories,
            }),
        }
    }
    pub fn validate(&self, players: &PlayerSnapshot) -> Result<()> {
        match self {
            Self::V1(v) => v.validate(players),
            Self::V2(v) => v.validate(players),
        }
    }
    pub(crate) fn issue(&self, id: u64) -> Result<Self> {
        match self {
            Self::V1(v) => Ok(v.issue(id)?.into()),
            Self::V2(v) => Ok(v.issue(id)?.into()),
        }
    }
    fn prepare_local(
        &self,
        request: &PlayerInventoryRequestV1,
    ) -> Result<(InventoryStateSnapshotV1, InventoryActionOutcomeV1)> {
        match self {
            Self::V1(v) => v.prepare_local(request),
            Self::V2(v) => v.prepare_local(request),
        }
    }
}

pub(crate) fn deserialize_optional<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> std::result::Result<Option<PlayerInventorySnapshot>, D::Error> {
    PlayerInventorySnapshot::deserialize(d).map(Some)
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
    pub fn empty_v2(&self) -> PlayerInventorySnapshotV2 {
        PlayerInventorySnapshotV2 {
            format: 2,
            recipe_id: self.identity.clone(),
            recipe: self.recipe.clone(),
            baseline: self.baseline.clone(),
            revision: 1,
            next_stack_id: 1,
            bags: vec![],
        }
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
fn validate_retained_recipe_parts(
    recipe: &PlayerInventoryRecipe,
    recipe_id: &str,
    baseline: &InventoryInitialTotalsV1,
) -> Result<()> {
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
        recipe_identity(recipe, domain)? == recipe_id,
        "player inventory recipe identity mismatch"
    );
    let contracts = CompiledInventoryContractsV1::player_bound(recipe, [1].into_iter(), recipe_id)?;
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
        initial.initial_totals() == baseline,
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
        validate_retained_recipe_parts(&self.recipe, &self.recipe_id, &self.baseline)?;
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
        matches!(
            snapshot.schema_version,
            crate::simulation::SNAPSHOT_SCHEMA_PLAYER_INVENTORY
                | crate::simulation::SNAPSHOT_SCHEMA_PLAYER_TRANSFERS
        ),
        "player inventory requires schema 15 or 16"
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
        let inventories = inventory
            .owners
            .into_iter()
            .next()
            .context("player inventory missing")?
            .inventories;
        match section {
            PlayerInventorySnapshot::V1(section) => {
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
                bag.inventories = inventories;
            }
            PlayerInventorySnapshot::V2(section) => {
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
                bag.consumed = inventory
                    .consumed
                    .into_iter()
                    .filter(|v| v.count > 0)
                    .collect();
                bag.inventories = inventories;
            }
        }
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
        .recipe()
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

fn dense_consumed(
    baseline: &InventoryInitialTotalsV1,
    rows: &[ItemTotalV1],
) -> Result<Vec<ItemTotalV1>> {
    validate_player_sparse(baseline, rows)?;
    let mut dense = zero(baseline);
    for row in rows {
        let i = dense
            .binary_search_by(|v| v.item.cmp(&row.item))
            .map_err(|_| anyhow::anyhow!("player inventory item invalid"))?;
        dense[i].count = row.count;
    }
    Ok(dense)
}
fn local_v2(
    section: &PlayerInventorySnapshotV2,
    bag: &PlayerBagV2,
) -> Result<InventoryStateSnapshotV1> {
    Ok(InventoryStateSnapshotV1 {
        format: 1,
        contract_id: section.recipe_id.clone(),
        revision: bag.revision,
        next_stack_id: section.next_stack_id,
        consumed: dense_consumed(&section.baseline, &bag.consumed)?,
        owners: vec![OwnerInventoriesV1 {
            owner: handle(bag.owner.player_id),
            inventories: bag.inventories.clone(),
        }],
    })
}
impl PlayerInventorySnapshotV2 {
    pub fn bag(&self, id: u64) -> Option<&PlayerBagV2> {
        self.bags.iter().find(|b| b.owner.player_id == id)
    }
    pub fn validate(&self, players: &PlayerSnapshot) -> Result<()> {
        player_inventory_v2_charge(self, 8)?;
        players.validate()?;
        ensure!(
            self.format == 2 && self.revision > 0 && self.next_stack_id > 0,
            "player inventory header invalid"
        );
        validate_retained_recipe_parts(&self.recipe, &self.recipe_id, &self.baseline)?;
        ensure!(
            self.bags.len() == players.records.len()
                && self.bags.windows(2).all(|r| r[0].owner < r[1].owner),
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
        let contracts = CompiledInventoryContractsV1::player_bound(
            &self.recipe,
            players.records.iter().map(|r| r.id),
            &self.recipe_id,
        )?;
        let mut received = vec![0u128; self.baseline.items.len()];
        let mut sent = vec![0u128; received.len()];
        let mut stacks = BTreeSet::new();
        let mut instances = 0usize;
        for bag in &self.bags {
            ensure!(
                bag.owner.incarnation == 1
                    && bag.revision > 0
                    && players
                        .records
                        .iter()
                        .any(|r| r.id == bag.owner.player_id && r.revision >= bag.revision),
                "player inventory owner invalid"
            );
            for (rows, totals) in [(&bag.received, &mut received), (&bag.sent, &mut sent)] {
                validate_player_sparse(&self.baseline, rows)?;
                for row in rows {
                    let i = self
                        .baseline
                        .items
                        .binary_search_by(|v| v.item.cmp(&row.item))
                        .map_err(|_| anyhow::anyhow!("player inventory item invalid"))?;
                    totals[i] = totals[i]
                        .checked_add(u128::from(row.count))
                        .context("player inventory quantity exhausted")?;
                }
            }
            let contract = contracts.for_owner(DomainId(bag.owner.player_id))?;
            validate_player_inventory_state_v2(
                &contract,
                &local_v2(self, bag)?,
                &self.baseline,
                &InventoryLiveOwnersV1 {
                    owners: vec![handle(bag.owner.player_id)],
                },
                &bag.received,
                &bag.sent,
            )?;
            instances = instances
                .checked_add(bag.inventories.len())
                .context("player inventory instance limit")?;
            ensure!(
                instances <= MAX_INVENTORY_INSTANCES,
                "player inventory instance limit"
            );
            for inv in &bag.inventories {
                for stack in inv
                    .slots
                    .iter()
                    .flatten()
                    .chain(inv.overflow.iter().map(|v| &v.stack))
                {
                    ensure!(
                        stacks.insert(stack.id) && stacks.len() <= MAX_INVENTORY_STACKS,
                        "player inventory stack identity or limit invalid"
                    );
                }
            }
        }
        ensure!(received == sent, "player inventory global flow mismatch");
        Ok(())
    }
    fn issue(&self, id: u64) -> Result<Self> {
        player_inventory_v2_charge(self, 8)?;
        ensure!(
            self.bag(id).is_none() && self.bags.len() < 4096,
            "player inventory issuance conflict"
        );
        let contracts = CompiledInventoryContractsV1::player_bound(
            &self.recipe,
            [id].into_iter(),
            &self.recipe_id,
        )?;
        let initial = prepare_initial_inventory_from(
            &contracts,
            &InventoryLiveOwnersV1 {
                owners: vec![handle(id)],
            },
            &starters(&self.recipe, id),
            player_inventory_v2_bytes(self)?
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
        next.bags
            .try_reserve(1)
            .context("player inventory allocation limit")?;
        next.bags.push(PlayerBagV2 {
            owner: PlayerInventoryOwnerV1 {
                player_id: id,
                incarnation: 1,
            },
            revision: 1,
            consumed: vec![],
            received: vec![],
            sent: vec![],
            inventories: state
                .owners
                .into_iter()
                .next()
                .context("player inventory issuance missing")?
                .inventories,
        });
        next.bags.sort_by_key(|b| b.owner);
        player_inventory_v2_charge(&next, 8)?;
        Ok(next)
    }
    fn prepare_local(
        &self,
        request: &PlayerInventoryRequestV1,
    ) -> Result<(InventoryStateSnapshotV1, InventoryActionOutcomeV1)> {
        player_inventory_v2_charge(self, 8)?;
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
        let prepared = prepare_player_inventory_action_v2(
            &contracts,
            &local_v2(self, bag)?,
            &self.baseline,
            &InventoryLiveOwnersV1 {
                owners: vec![handle(request.player_id)],
            },
            (&bag.received, &bag.sent),
            &InventoryActionRequestV1 {
                expected_inventory_revision: bag.revision,
                action,
            },
        )?;
        let outcome = prepared.outcome();
        if outcome == InventoryActionOutcomeV1::Applied {
            self.revision
                .checked_add(1)
                .context("player inventory revision exhausted")?;
        }
        Ok((prepared.into_state(), outcome))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct AppliedPlayerInventoryTransfer {
    pub source_player_revision: u64,
    pub source_bag_revision: u64,
    pub destination_player_revision: u64,
    pub destination_bag_revision: u64,
    pub inventory_revision: u64,
}
pub fn prepare_transfer_offline(
    snapshot: &crate::simulation::WorldSnapshot,
    request: &PlayerInventoryTransferRequestV1,
) -> Result<(
    crate::simulation::WorldSnapshot,
    AppliedPlayerInventoryTransfer,
)> {
    prepare_transfer_generation(std::borrow::Cow::Borrowed(snapshot), request)
}
pub(crate) fn prepare_transfer_owned(
    snapshot: crate::simulation::WorldSnapshot,
    request: &PlayerInventoryTransferRequestV1,
) -> Result<(
    crate::simulation::WorldSnapshot,
    AppliedPlayerInventoryTransfer,
)> {
    prepare_transfer_generation(std::borrow::Cow::Owned(snapshot), request)
}
fn flow_next(rows: &[ItemTotalV1], item: &str, amount: u32) -> Result<u64> {
    rows.iter()
        .find(|v| v.item == item)
        .map_or(0, |v| v.count)
        .checked_add(u64::from(amount))
        .context("player inventory flow exhausted")
}
fn set_flow(rows: &mut Vec<ItemTotalV1>, item: String, count: u64) {
    match rows.binary_search_by(|v| v.item.cmp(&item)) {
        Ok(i) => rows[i].count = count,
        Err(i) => rows.insert(i, ItemTotalV1 { item, count }),
    }
}
fn prepare_transfer_generation(
    snapshot: std::borrow::Cow<'_, crate::simulation::WorldSnapshot>,
    request: &PlayerInventoryTransferRequestV1,
) -> Result<(
    crate::simulation::WorldSnapshot,
    AppliedPlayerInventoryTransfer,
)> {
    prepare_transfer_with_reserve(snapshot, request, |rows| {
        rows.try_reserve(1)
            .context("player inventory allocation limit")
    })
}

// Per-call reserve dependency keeps refusal tests local to these two genuinely
// fallible staging operations. The production caller retains Vec::try_reserve.
fn prepare_transfer_with_reserve(
    snapshot: std::borrow::Cow<'_, crate::simulation::WorldSnapshot>,
    request: &PlayerInventoryTransferRequestV1,
    mut reserve: impl FnMut(&mut Vec<ItemTotalV1>) -> Result<()>,
) -> Result<(
    crate::simulation::WorldSnapshot,
    AppliedPlayerInventoryTransfer,
)> {
    player_action_bounds(request)?;
    snapshot.validate()?;
    ensure!(
        snapshot.schema_version == crate::simulation::SNAPSHOT_SCHEMA_PLAYER_TRANSFERS,
        "player inventory transfer requires schema 16"
    );
    let section = snapshot
        .player_inventory
        .as_ref()
        .and_then(PlayerInventorySnapshot::as_v2)
        .context("player inventory transfer requires section 2")?;
    let source = request.source.owner;
    let dest = request.destination.owner;
    ensure!(
        source.player_id > 0
            && dest.player_id > 0
            && source.incarnation == 1
            && dest.incarnation == 1
            && source.player_id != dest.player_id
            && request.amount > 0,
        "player inventory transfer endpoints invalid"
    );
    let players = snapshot
        .native_players
        .as_ref()
        .context("players missing")?;
    let record_index = |id| {
        players
            .records
            .iter()
            .position(|r| r.id == id)
            .context("player inventory transfer player missing")
    };
    let si = record_index(source.player_id)?;
    let di = record_index(dest.player_id)?;
    let sb = section
        .bags
        .iter()
        .position(|b| b.owner == source)
        .context("player inventory transfer bag missing")?;
    let db = section
        .bags
        .iter()
        .position(|b| b.owner == dest)
        .context("player inventory transfer bag missing")?;
    let src = &section.bags[sb];
    let dst = &section.bags[db];
    ensure!(
        players.records[si].revision == request.source_cas.expected_player_revision
            && src.revision == request.source_cas.expected_bag_revision
            && players.records[di].revision == request.destination_cas.expected_player_revision
            && dst.revision == request.destination_cas.expected_bag_revision,
        "player inventory transfer revision conflict"
    );
    let sinv = src
        .inventories
        .iter()
        .position(|i| i.key == request.source.inventory)
        .context("player inventory transfer source missing")?;
    let dinv = dst
        .inventories
        .iter()
        .position(|i| i.key == request.destination.inventory)
        .context("player inventory transfer destination missing")?;
    let inv = &src.inventories[sinv];
    let normal = inv
        .slots
        .iter()
        .position(|v| v.as_ref().is_some_and(|s| s.id == request.source.stack_id));
    let overflow = inv
        .overflow
        .iter()
        .position(|v| v.stack.id == request.source.stack_id);
    let stack = match (normal, overflow) {
        (Some(i), None) => inv.slots[i].as_ref().expect("located stack"),
        (None, Some(i)) => &inv.overflow[i].stack,
        _ => anyhow::bail!("player inventory transfer source missing"),
    };
    let slot = usize::from(request.destination.slot);
    let definition = section
        .recipe
        .inventories()
        .iter()
        .find(|i| i.key == request.destination.inventory)
        .context("player inventory transfer destination missing")?;
    ensure!(
        dst.inventories[dinv]
            .slots
            .get(slot)
            .is_some_and(Option::is_none)
            && definition
                .slot_rules
                .get(slot)
                .is_some_and(|r| r.allowed_items.iter().any(|i| i == &stack.item)),
        "player inventory transfer destination refused"
    );
    let limit = section
        .recipe
        .items()
        .iter()
        .find(|i| i.key == stack.item)
        .context("player inventory transfer item missing")?
        .stack_limit;
    ensure!(
        request.amount <= stack.count && request.amount <= limit,
        "player inventory transfer amount invalid"
    );
    let partial = request.amount < stack.count;
    let next_stack_id = if partial {
        section
            .next_stack_id
            .checked_add(1)
            .context("player inventory allocator exhausted")?
    } else {
        section.next_stack_id
    };
    if partial {
        let count: usize = section
            .bags
            .iter()
            .flat_map(|b| &b.inventories)
            .map(|i| i.slots.iter().flatten().count() + i.overflow.len())
            .sum();
        ensure!(count < MAX_INVENTORY_STACKS, "player inventory stack limit");
    }
    let result = AppliedPlayerInventoryTransfer {
        source_player_revision: players.records[si]
            .revision
            .checked_add(1)
            .context("player revision exhausted")?,
        source_bag_revision: src
            .revision
            .checked_add(1)
            .context("player inventory revision exhausted")?,
        destination_player_revision: players.records[di]
            .revision
            .checked_add(1)
            .context("player revision exhausted")?,
        destination_bag_revision: dst
            .revision
            .checked_add(1)
            .context("player inventory revision exhausted")?,
        inventory_revision: section
            .revision
            .checked_add(1)
            .context("player inventory revision exhausted")?,
    };
    let sent = flow_next(&src.sent, &stack.item, request.amount)?;
    let received = flow_next(&dst.received, &stack.item, request.amount)?;
    let transferred = StackV1 {
        id: if partial {
            StackId::new(section.next_stack_id)?
        } else {
            stack.id
        },
        item: stack.item.clone(),
        count: request.amount,
    };
    let sent_item = stack.item.clone();
    let received_item = stack.item.clone();
    let mut next = snapshot.into_owned();
    let section = next
        .player_inventory
        .as_mut()
        .and_then(PlayerInventorySnapshot::as_v2_mut)
        .expect("validated section");
    if section.bags[sb]
        .sent
        .binary_search_by(|v| v.item.cmp(&sent_item))
        .is_err()
    {
        reserve(&mut section.bags[sb].sent)?;
    }
    if section.bags[db]
        .received
        .binary_search_by(|v| v.item.cmp(&received_item))
        .is_err()
    {
        reserve(&mut section.bags[db].received)?;
    }
    let inv = &mut section.bags[sb].inventories[sinv];
    match (normal, overflow, partial) {
        (Some(i), None, true) => {
            inv.slots[i].as_mut().expect("located stack").count -= request.amount
        }
        (Some(i), None, false) => {
            inv.slots[i].take();
        }
        (None, Some(i), true) => inv.overflow[i].stack.count -= request.amount,
        (None, Some(i), false) => {
            inv.overflow.remove(i);
        }
        _ => unreachable!("validated source"),
    }
    section.bags[db].inventories[dinv].slots[slot] = Some(transferred);
    set_flow(&mut section.bags[sb].sent, sent_item, sent);
    set_flow(&mut section.bags[db].received, received_item, received);
    section.bags[sb].revision = result.source_bag_revision;
    section.bags[db].revision = result.destination_bag_revision;
    section.revision = result.inventory_revision;
    section.next_stack_id = next_stack_id;
    let players = next.native_players.as_mut().expect("validated players");
    players.records[si].revision = result.source_player_revision;
    players.records[di].revision = result.destination_player_revision;
    crate::persistence::SnapshotStore::preflight(&next)?;
    Ok((next, result))
}

#[cfg(test)]
pub(crate) mod transfer_staging_tests {
    use super::*;
    use crate::{
        experimental::pawn_open::CharacterVitals,
        native_player_runtime::PlayerRegistry,
        native_players::{PlayerCheckpoint, PlayerContext, PlayerInputs, PlayerKey},
        simulation::WorldSnapshot,
    };

    pub(crate) fn key(id: u64) -> PlayerKey {
        PlayerKey::new(9000 + id, 0).unwrap()
    }

    pub(crate) fn checkpoint() -> PlayerCheckpoint {
        PlayerCheckpoint {
            location_cm: [10., 20., 30.],
            vitals: CharacterVitals {
                health: 100,
                max_health: 100,
                stamina: 100,
                max_stamina: 100,
                oxygen: 100,
                max_oxygen: 100,
                water: 100,
                max_water: 100,
                food: 100,
                max_food: 100,
                biome_row: 0,
                growth_row: 0,
            },
        }
    }

    // Authored retained owners, issued once each. Larger graph tests reuse the
    // same setup; balanced sparse flows are added explicitly by those tests.
    pub(crate) fn fixture(recipe: PlayerInventoryRecipeV1, owners: u64) -> WorldSnapshot {
        let context = PlayerContext::new(
            "boundary-world".encode_utf16().collect(),
            "/Authored/Boundary".into(),
            PlayerInputs {
                authored_gameplay: [1; 32],
                spawn: [2; 32],
                authored_world: [3; 32],
                declaration: [4; 32],
                initial_values: [5; 32],
            },
            vec!["Player".into()],
        )
        .unwrap();
        let compiled = CompiledPlayerInventoryRecipe::compile(recipe).unwrap();
        let mut section = compiled.empty_v2().issue(1).unwrap();
        let template = section.bags.pop().unwrap();
        let stacks = section.recipe.starter().len() as u64;
        let mut players =
            PlayerRegistry::new(PlayerSnapshot::empty(context.clone()), &context, 64).unwrap();
        for id in 1..=owners {
            players.reserve(key(id), id, checkpoint()).unwrap();
            players.activate(id).unwrap();
            players.detach(id);
            let mut bag = template.clone();
            bag.owner.player_id = id;
            for stack in bag.inventories.iter_mut().flat_map(|i| {
                i.slots
                    .iter_mut()
                    .flatten()
                    .chain(i.overflow.iter_mut().map(|v| &mut v.stack))
            }) {
                stack.id = StackId::new((id - 1) * stacks + stack.id.get()).unwrap();
            }
            section.bags.push(bag);
        }
        section.next_stack_id = stacks * owners + 1;
        section.revision = owners + 1;
        WorldSnapshot {
            schema_version: crate::simulation::SNAPSHOT_SCHEMA_PLAYER_TRANSFERS,
            native_players: Some(players.snapshot()),
            player_inventory: Some(section.into()),
            ..Default::default()
        }
    }

    pub(crate) fn request(snapshot: &WorldSnapshot) -> PlayerInventoryTransferRequestV1 {
        let section = snapshot.player_inventory.as_ref().unwrap().as_v2().unwrap();
        let src = section.bag(1).unwrap();
        let dst = section.bag(2).unwrap();
        PlayerInventoryTransferRequestV1 {
            source: PlayerStackEndpointV1 {
                owner: src.owner,
                inventory: src.inventories[0].key.clone(),
                stack_id: src.inventories[0].slots[0].as_ref().unwrap().id,
            },
            destination: PlayerSlotEndpointV1 {
                owner: dst.owner,
                inventory: dst.inventories[1].key.clone(),
                slot: 0,
            },
            amount: 1,
            source_cas: PlayerInventoryRevisionPairV1 {
                expected_player_revision: snapshot
                    .native_players
                    .as_ref()
                    .unwrap()
                    .projection_by_id(1)
                    .unwrap()
                    .revision,
                expected_bag_revision: src.revision,
            },
            destination_cas: PlayerInventoryRevisionPairV1 {
                expected_player_revision: snapshot
                    .native_players
                    .as_ref()
                    .unwrap()
                    .projection_by_id(2)
                    .unwrap()
                    .revision,
                expected_bag_revision: dst.revision,
            },
        }
    }

    #[test]
    fn boundary_each_transfer_reserve_refusal_preserves_input_then_succeeds() {
        let inventory = |key: &str, slots| InventoryDefinitionV1 {
            key: key.into(),
            initial_slots: slots,
            slot_rules: vec![
                SlotRuleV1 {
                    allowed_items: vec!["item".into()]
                };
                slots as usize
            ],
            force_add_when_zero: false,
            overflow_limit: 0,
        };
        let snapshot = fixture(
            PlayerInventoryRecipeV1 {
                format: 1,
                items: vec![ItemDefinitionV1 {
                    key: "item".into(),
                    stack_limit: 10,
                }],
                inventories: vec![inventory("bag", 1), inventory("spare", 1)],
                starter: vec![PlayerStarterItemV1 {
                    inventory: "bag".into(),
                    requested_slot: 0,
                    item: "item".into(),
                    count: 2,
                }],
            },
            2,
        );
        snapshot.validate().unwrap();
        let before = serde_json::to_vec(&snapshot).unwrap();
        let req = request(&snapshot);
        let expected = prepare_transfer_offline(&snapshot, &req).unwrap();
        for fail_at in 1..=2 {
            let mut calls = 0;
            let err = prepare_transfer_with_reserve(
                std::borrow::Cow::Borrowed(&snapshot),
                &req,
                |rows| {
                    calls += 1;
                    if calls == fail_at {
                        // A real TryReserveError (deterministic capacity overflow),
                        // delivered at the selected private staging call only.
                        Vec::<u8>::new()
                            .try_reserve(usize::MAX)
                            .context("player inventory allocation limit")?;
                        unreachable!("usize::MAX reserve must refuse");
                    }
                    rows.try_reserve(1)
                        .context("player inventory allocation limit")
                },
            )
            .unwrap_err();
            assert_eq!(calls, fail_at);
            assert_eq!(err.to_string(), "player inventory allocation limit");
            assert!(
                err.downcast_ref::<std::collections::TryReserveError>()
                    .is_some()
            );
            assert_eq!(serde_json::to_vec(&snapshot).unwrap(), before);
            let mut calls = 0;
            let retried = prepare_transfer_with_reserve(
                std::borrow::Cow::Borrowed(&snapshot),
                &req,
                |rows| {
                    calls += 1;
                    rows.try_reserve(1)
                        .context("player inventory allocation limit")
                },
            )
            .unwrap();
            assert_eq!(calls, 2);
            assert_eq!(retried, expected);
            assert_eq!(serde_json::to_vec(&snapshot).unwrap(), before);
        }
    }
}

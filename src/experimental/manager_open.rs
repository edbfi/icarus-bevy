//! Opens of world manager actors the GameState references (F-859), on
//! [`super::actor_open`].
//!
//! The original opens each manager as a dynamic actor with its archetype in
//! the persistent level, `RemoteRole = ROLE_SimulatedProxy` and
//! `Role = ROLE_Authority`, and one block per stably named component: the
//! stat container (created only) and, for `AIcarusActor` managers, the actor
//! state, which the actor block references by handle. The plan carries the
//! release's identifiers; nothing here reads original files.
//!
//! The QuestManager also replicates its saved quest state (`C018`): the
//! faction mission's row, difficulty, flags, the initial quest actor and the
//! keyed relevant persistent actors, all referenced by actor key. The
//! PlayerHistoryTracker replicates its saved history (`C024`), the
//! InventoryContainerManager its saved inventories (`C016`) as runtime
//! `UInventory` subobjects its `Inventories` lists, and the FLOD manager
//! references its saved tiles (`C013`).
use std::collections::BTreeMap;

use anyhow::{Context, Result};

use super::actor_channel::NetGuidRef;
use super::actor_open::{
    Block, BlockContent, BlockProperties, ComponentActorOpen, OpenComponent, Reference,
    compose_component_actor_open,
};
use super::archive::ByteOrder;
use super::control_messages::ArchiveText;
use super::game_state_open::GameStateManagers;
use super::pawn_open::{PawnContentPlan, PawnInventory, inventory_subobject};
use super::prospect_info::{PlayerHistoryEntry, RelevantQuestActor, SavedQuestManager};
use super::rep_layout::{PropertyStream, RepArray, RepCommand, RepEntry, RepValue};
use super::session_open::{
    ActorKey, ComposedActorOpen, ConnectionExports, NetGuidCache, PathSegment,
};

/// `AActor::RemoteRole` and `AActor::Role`.
const REMOTE_ROLE: u32 = 5;
const ROLE: u32 = 14;
/// `ROLE_SimulatedProxy` and `ROLE_Authority`, as captured (F-859).
const SIMULATED_PROXY: u8 = 1;
const AUTHORITY: u8 = 3;

/// `AIcarusActor::StatContainer` and `AIcarusActor::ActorState` (handles
/// 16 and 17 of `icarus_actor_rep_layout`).
pub const STAT_CONTAINER_HANDLE: u32 = 16;
pub const ACTOR_STATE_HANDLE: u32 = 17;

/// `UActorState::CurrentBiome` (`actor_state_rep_layout` handle 11); the
/// managers send row 0, as captured.
pub const ACTOR_STATE_CURRENT_BIOME: u32 = 11;

/// `AQuestManager` handles 19..26 (`quest_manager_rep_layout`).
pub mod quest_handle {
    pub const FACTION_MISSION: u32 = 19;
    pub const DYNAMIC_QUEST_DIFFICULTY: u32 = 20;
    pub const INITIAL_QUEST: u32 = 21;
    pub const MISSION_COMPLETE: u32 = 22;
    pub const DYNAMIC_QUEST_DELAY: u32 = 23;
    pub const RUN_QUESTS: u32 = 24;
    pub const RELEVANT_PERSISTENT_ACTORS: u32 = 25;
    pub const RELEVANT_PERSISTENT_CHARACTERS: u32 = 26;
}

/// `APlayerHistoryTracker::History` (`player_history_tracker_rep_layout`).
pub const PLAYER_HISTORY_HANDLE: u32 = 19;

/// `AInventoryContainerManager::Inventories`
/// (`inventory_container_manager_rep_layout`).
pub const CONTAINER_INVENTORIES_HANDLE: u32 = 19;

/// `AFLOD`'s tile actor array (`flod_rep_layout` handle 16).
pub const FLOD_TILES_HANDLE: u32 = 16;

/// Which GameState manager reference a manager open fills (F-794, F-859).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ManagerSlot {
    Quest,
    WorldTalent,
    Radiation,
    InventoryContainer,
    PlayerHistory,
    Flod,
    Disaster,
}

impl ManagerSlot {
    /// The manager's session key: one per world, beside the GameState's
    /// `Session(0)` and below the per-connection pawn keys.
    pub fn key(self) -> ActorKey {
        ActorKey::Session(match self {
            Self::Quest => 1,
            Self::WorldTalent => 2,
            Self::Radiation => 3,
            Self::InventoryContainer => 4,
            Self::PlayerHistory => 5,
            Self::Flod => 6,
            Self::Disaster => 7,
        })
    }

    /// Point the GameState's reference for this slot at its key.
    pub fn reference_from(self, managers: &mut GameStateManagers) {
        let reference = Some(self.key());
        match self {
            Self::Quest => managers.quest_manager = reference,
            Self::WorldTalent => managers.world_talent_manager = reference,
            Self::Radiation => managers.radiation_manager = reference,
            Self::InventoryContainer => managers.inventory_container_manager = reference,
            Self::PlayerHistory => managers.player_history_tracker = reference,
            Self::Flod => managers.flod = reference,
            Self::Disaster => managers.disaster_controller = reference,
        }
    }
}

/// A saved Icarus actor GUID's actor key, apart from domain ids, so a
/// saved actor opened later under it gets the GUID referenced here.
pub fn icarus_actor_key(icarus_uid: i32) -> Option<ActorKey> {
    u64::try_from(icarus_uid)
        .ok()
        .filter(|uid| *uid > 0)
        .map(ActorKey::Saved)
}

/// The stably named component names the managers carry, as captured.
pub const STAT_CONTAINER: &str = "Stat Container";
pub const ACTOR_STATE: &str = "ActorState";

/// One stably named component of a manager.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ManagerComponent {
    pub name: String,
    /// Its class network checksum (F-776).
    pub checksum: u32,
    /// The actor-block handle referencing it, if any.
    pub handle: Option<u32>,
    pub block: Block,
}

impl ManagerComponent {
    /// The stat container, created only, referenced by `handle` if any.
    pub fn stat_container(checksum: u32, handle: Option<u32>) -> Self {
        Self {
            name: STAT_CONTAINER.to_owned(),
            checksum,
            handle,
            block: Block::Encoded(BlockContent::create_only()),
        }
    }

    /// The actor state, referenced by [`ACTOR_STATE_HANDLE`], with the
    /// values of `layout` (`actor_state_rep_layout`).
    pub fn actor_state(
        checksum: u32,
        layout: Vec<RepCommand>,
        values: BTreeMap<u32, RepValue>,
    ) -> Self {
        Self {
            name: ACTOR_STATE.to_owned(),
            checksum,
            handle: Some(ACTOR_STATE_HANDLE),
            block: Block::Composed(BlockProperties {
                layout,
                references: BTreeMap::new(),
                values,
                fields: Vec::new(),
                field_bound: 0,
            }),
        }
    }
}

/// A manager open's identifiers and layouts.
#[derive(Clone, Debug, PartialEq)]
pub struct ManagerOpenPlan {
    /// `Default__<Class>`, then its package (`/Script/Icarus` for native
    /// classes, the Blueprint's package otherwise).
    pub archetype: Vec<PathSegment>,
    /// `PersistentLevel`, the world, then the map package.
    pub level: Vec<PathSegment>,
    /// The spawn location, when the open sends one.
    pub location: Option<[f32; 3]>,
    /// The actor's rep layout.
    pub layout: Vec<RepCommand>,
    pub components: Vec<ManagerComponent>,
    /// Further actor-block values (the roles are written here).
    pub values: BTreeMap<u32, RepValue>,
    /// Actor-block handles referencing keyed actors.
    pub actors: BTreeMap<u32, ActorKey>,
    /// `(Str, Object)` array handles: each element's string and keyed actor.
    pub keyed_actors: BTreeMap<u32, Vec<(ArchiveText, ActorKey)>>,
    /// `Object` array handles: each element's keyed actor.
    pub actor_arrays: BTreeMap<u32, Vec<ActorKey>>,
    /// Runtime inventories opened as subobjects and listed by an
    /// actor-block array.
    pub inventories: Option<ManagerInventories>,
    pub engine_net_version: u32,
}

/// Runtime `UInventory` subobjects a manager opens, in block order, and
/// the actor-block array handle that lists their GUIDs in the same order;
/// none opens no subobject and sends no array.
#[derive(Clone, Debug, PartialEq)]
pub struct ManagerInventories {
    pub handle: u32,
    /// The inventory class, layouts and field bound (the pawn's).
    pub blocks: PawnContentPlan,
    pub inventories: Vec<PawnInventory>,
}

impl ManagerOpenPlan {
    /// Add the saved quest manager state (`C018`) to a QuestManager plan,
    /// with `faction_mission` the saved mission's `D_FactionMissions` row.
    /// An initial open sends only values off the class defaults, so false,
    /// zero and empty values are left out. Links without a valid Icarus
    /// actor GUID are dropped. Fails on a key that is not a string.
    pub fn with_quest_state(
        mut self,
        saved: &SavedQuestManager,
        faction_mission: u8,
    ) -> Result<Self> {
        use quest_handle::*;
        self.values
            .insert(FACTION_MISSION, RepValue::Byte(faction_mission));
        if saved.dynamic_quest_difficulty != 0 {
            self.values.insert(
                DYNAMIC_QUEST_DIFFICULTY,
                RepValue::Enum(u32::from(saved.dynamic_quest_difficulty)),
            );
        }
        if saved.mission_complete {
            self.values.insert(MISSION_COMPLETE, RepValue::Bool(true));
        }
        if saved.dynamic_quest_delay != 0.0 {
            self.values.insert(
                DYNAMIC_QUEST_DELAY,
                RepValue::Float(saved.dynamic_quest_delay.to_bits()),
            );
        }
        if saved.run_quests {
            self.values.insert(RUN_QUESTS, RepValue::Bool(true));
        }
        if let Some(key) = saved.initial_quest.and_then(icarus_actor_key) {
            self.actors.insert(INITIAL_QUEST, key);
        }
        let keyed = |actors: &[RelevantQuestActor]| -> Result<Vec<(ArchiveText, ActorKey)>> {
            actors
                .iter()
                .filter_map(|actor| {
                    icarus_actor_key(actor.icarus_uid)
                        .map(|key| Ok((ArchiveText::new(&actor.key)?, key)))
                })
                .collect()
        };
        for (handle, actors) in [
            (RELEVANT_PERSISTENT_ACTORS, &saved.persistent_actors),
            (RELEVANT_PERSISTENT_CHARACTERS, &saved.persistent_characters),
        ] {
            let actors = keyed(actors)?;
            if !actors.is_empty() {
                self.keyed_actors.insert(handle, actors);
            }
        }
        Ok(self)
    }

    /// Add the saved container inventories (`C016`, F-866) to an
    /// InventoryContainerManager plan, whose release plan carries the
    /// inventory layouts: subobjects listed by `Inventories`. Fails on a
    /// plan without inventory layouts.
    pub fn with_container_inventories(mut self, inventories: Vec<PawnInventory>) -> Result<Self> {
        let owned = self
            .inventories
            .as_mut()
            .context("the manager plan has no inventory layouts")?;
        owned.inventories = inventories;
        Ok(self)
    }

    /// Add the saved FLOD tile count (`C013`, F-914) to a FLOD manager plan:
    /// its tile array references `count` tile actors, keyed by position; the
    /// original assigns their GUIDs at this open, in array order. None is
    /// the class default and adds nothing. A count that is negative or past
    /// a replicated array's bound fails before anything is allocated.
    pub fn with_flod_tiles(mut self, count: i32) -> Result<Self> {
        let count = u16::try_from(count)
            .context("the saved FLOD tile count is negative or exceeds an array's bound")?;
        if count > 0 {
            self.actor_arrays.insert(
                FLOD_TILES_HANDLE,
                (0..u64::from(count)).map(ActorKey::FlodTile).collect(),
            );
        }
        Ok(self)
    }

    /// Add the saved player history (`C024`) to a PlayerHistoryTracker
    /// plan as `History`, every element field, in saved order; an empty
    /// history is the class default and adds nothing. Fails on more
    /// entries than an array counts or on a string that cannot be sent.
    pub fn with_player_history(mut self, history: &[PlayerHistoryEntry]) -> Result<Self> {
        if history.is_empty() {
            return Ok(self);
        }
        let mut entries = Vec::with_capacity(3 * history.len());
        for (entry, element) in history.iter().zip(0u32..) {
            for (field, value) in [
                RepValue::Str(ArchiveText::new(&entry.user_id)?),
                RepValue::Int(entry.chr_slot),
                RepValue::Str(ArchiveText::new(&entry.cached_character_name)?),
            ]
            .into_iter()
            .zip(1u32..)
            .map(|(value, field)| (field, value))
            {
                entries.push(RepEntry {
                    handle: 3 * element + field,
                    checksum: None,
                    value,
                });
            }
        }
        self.values.insert(
            PLAYER_HISTORY_HANDLE,
            RepValue::DynamicArray(RepArray {
                count: u16::try_from(history.len())?,
                stream: PropertyStream {
                    entries,
                    terminator_checksum: None,
                },
            }),
        );
        Ok(self)
    }
}

/// Compose the manager `plan` keyed `key` for one connection on
/// `channel_index` in the connection's byte `order`.
pub fn compose_manager_open(
    cache: &mut NetGuidCache,
    exports: &mut ConnectionExports,
    plan: &ManagerOpenPlan,
    key: ActorKey,
    channel_index: u32,
    order: ByteOrder,
) -> Result<ComposedActorOpen> {
    let mut values = plan.values.clone();
    values.insert(REMOTE_ROLE, RepValue::Byte(SIMULATED_PROXY));
    values.insert(ROLE, RepValue::Byte(AUTHORITY));
    for (handle, actors) in &plan.keyed_actors {
        values.insert(*handle, keyed_actor_array(cache, actors)?);
    }
    for (handle, actors) in &plan.actor_arrays {
        let guids: Vec<u32> = actors.iter().map(|key| cache.actor_guid(*key)).collect();
        values.insert(*handle, object_array(&guids)?);
    }
    // The inventories get their GUIDs where the actor block lists them,
    // before any block, as the subobjects' own blocks then reuse them.
    let mut subobjects = Vec::new();
    if let Some(owned) = plan
        .inventories
        .as_ref()
        .filter(|owned| !owned.inventories.is_empty())
    {
        let guids = (0..u32::try_from(owned.inventories.len())?)
            .map(|index| cache.subobject_guid(key, index))
            .collect::<Vec<_>>();
        values.insert(owned.handle, object_array(&guids)?);
        for inventory in &owned.inventories {
            subobjects.push(inventory_subobject(
                &owned.blocks,
                inventory,
                None,
                plan.engine_net_version,
                order,
            )?);
        }
    }
    let references = plan
        .components
        .iter()
        .enumerate()
        .filter_map(|(index, component)| {
            component
                .handle
                .map(|handle| (handle, Reference::Component(index)))
        })
        .chain(
            plan.actors
                .iter()
                .map(|(handle, key)| (*handle, Reference::Actor(*key))),
        )
        .collect();
    let open = ComponentActorOpen {
        key,
        archetype: plan.archetype.clone(),
        level: plan.level.clone(),
        location: plan.location,
        layout: plan.layout.clone(),
        references,
        values,
        components: plan
            .components
            .iter()
            .map(|component| OpenComponent {
                name: component.name.clone(),
                checksum: component.checksum,
                block: Some(component.block.clone()),
            })
            .collect(),
        subobjects,
        engine_net_version: plan.engine_net_version,
    };
    let (composed, _) = compose_component_actor_open(cache, exports, &open, channel_index, order)?;
    Ok(composed)
}

/// A `(Str, Object)` dynamic array of `actors`, each object the keyed
/// actor's driver-wide GUID, assigned on first use in element order.
fn keyed_actor_array(
    cache: &mut NetGuidCache,
    actors: &[(ArchiveText, ActorKey)],
) -> Result<RepValue> {
    let mut entries = Vec::with_capacity(2 * actors.len());
    for ((text, key), element) in actors.iter().zip(0u32..) {
        entries.push(RepEntry {
            handle: 2 * element + 1,
            checksum: None,
            value: RepValue::Str(text.clone()),
        });
        entries.push(RepEntry {
            handle: 2 * element + 2,
            checksum: None,
            value: RepValue::Object(NetGuidRef {
                guid: cache.actor_guid(*key),
                export: None,
            }),
        });
    }
    Ok(RepValue::DynamicArray(RepArray {
        count: u16::try_from(actors.len())?,
        stream: PropertyStream {
            entries,
            terminator_checksum: None,
        },
    }))
}

/// An `Object` dynamic array of `guids`, in order.
fn object_array(guids: &[u32]) -> Result<RepValue> {
    Ok(RepValue::DynamicArray(RepArray {
        count: u16::try_from(guids.len())?,
        stream: PropertyStream {
            entries: guids
                .iter()
                .zip(1u32..)
                .map(|(guid, handle)| RepEntry {
                    handle,
                    checksum: None,
                    value: RepValue::Object(NetGuidRef {
                        guid: *guid,
                        export: None,
                    }),
                })
                .collect(),
            terminator_checksum: None,
        },
    }))
}

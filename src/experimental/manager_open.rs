//! Opens of world manager actors the GameState references (F-859), on
//! [`super::actor_open`].
//!
//! The original opens each manager as a dynamic actor with its archetype in
//! the persistent level, `RemoteRole = ROLE_SimulatedProxy` and
//! `Role = ROLE_Authority`, and one block per stably named component: the
//! stat container (created only) and, for `AIcarusActor` managers, the actor
//! state, which the actor block references by handle. The plan carries the
//! release's identifiers; nothing here reads original files.
use std::collections::BTreeMap;

use anyhow::Result;

use super::actor_open::{
    Block, BlockContent, BlockProperties, ComponentActorOpen, OpenComponent, Reference,
    compose_component_actor_open,
};
use super::archive::ByteOrder;
use super::rep_layout::{RepCommand, RepValue};
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

/// Which GameState manager reference a manager open fills (F-794, F-859).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ManagerSlot {
    WorldTalent,
    Radiation,
    Disaster,
}

impl ManagerSlot {
    /// The manager's session key: one per world, beside the GameState's
    /// `Session(0)` and below the per-connection pawn keys.
    pub fn key(self) -> ActorKey {
        ActorKey::Session(match self {
            Self::WorldTalent => 2,
            Self::Radiation => 3,
            Self::Disaster => 7,
        })
    }
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
    /// The actor's rep layout.
    pub layout: Vec<RepCommand>,
    pub components: Vec<ManagerComponent>,
    /// Further actor-block values (the roles are written here).
    pub values: BTreeMap<u32, RepValue>,
    pub engine_net_version: u32,
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
    let references = plan
        .components
        .iter()
        .enumerate()
        .filter_map(|(index, component)| {
            component
                .handle
                .map(|handle| (handle, Reference::Component(index)))
        })
        .collect();
    let open = ComponentActorOpen {
        key,
        archetype: plan.archetype.clone(),
        level: plan.level.clone(),
        location: None,
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
        subobjects: Vec::new(),
        engine_net_version: plan.engine_net_version,
    };
    let (composed, _) = compose_component_actor_open(cache, exports, &open, channel_index, order)?;
    Ok(composed)
}

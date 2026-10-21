//! The joining player's pawn open (`F-797`), on [`super::actor_open`].
//!
//! The original opens the player's pawn on its owner's connection with
//! `RemoteRole = ROLE_AutonomousProxy`, `Owner` and `Controller` naming the
//! PlayerController, `Instigator` itself and `PlayerState` the player's
//! PlayerState; three properties reference components (`StatContainer`,
//! `ActorState`, `InventoryComponent`); every component gets a block, and
//! the runtime inventories follow as class-named subobjects.
//!
//! The native server has no stat, character-state or inventory content yet,
//! so [`compose_pawn_open`] opens every component without properties and
//! sends no inventories; identifiers come from the release profile.
use std::collections::BTreeMap;

use anyhow::{Result, ensure};

use super::actor_open::{
    BlockContent, ComponentActorOpen, OpenComponent, Reference, compose_component_actor_open,
};
use super::rep_layout::{RepCommand, RepValue};
use super::session_open::{
    ActorKey, ComposedActorOpen, ConnectionExports, NetGuidCache, PathSegment,
};

/// Handles of the `BP_IcarusPlayerCharacterSurvival_C` layout the pawn open
/// writes (F-797).
pub mod handle {
    /// `AActor::RemoteRole`.
    pub const REMOTE_ROLE: u32 = 5;
    /// `AActor::Owner`.
    pub const OWNER: u32 = 13;
    /// `AActor::Role`.
    pub const ROLE: u32 = 14;
    /// `AActor::Instigator`.
    pub const INSTIGATOR: u32 = 15;
    /// `APawn::PlayerState`.
    pub const PLAYER_STATE: u32 = 17;
    /// `APawn::Controller`.
    pub const CONTROLLER: u32 = 18;
    /// `AIcarusCharacter::StatContainer`.
    pub const STAT_CONTAINER: u32 = 47;
    /// `AIcarusCharacter::ActorState`.
    pub const ACTOR_STATE: u32 = 48;
    /// `AIcarusPlayerCharacter::InventoryComponent`.
    pub const INVENTORY_COMPONENT: u32 = 56;
}

/// `ROLE_AutonomousProxy` for the owning connection and `ROLE_Authority`,
/// as captured (F-797).
const OWNER_REMOTE_ROLE: u8 = 2;
const ROLE: u8 = 3;

/// A pawn component: its name and class network checksum (F-776).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PawnComponent {
    pub name: String,
    pub checksum: u32,
}

/// Everything a session needs to open a player's pawn.
#[derive(Clone, Debug)]
pub struct PawnOpenPlan {
    /// `Default__<Class>_C`, then its package.
    pub archetype: Vec<PathSegment>,
    /// `PersistentLevel`, the world, then the map package.
    pub level: Vec<PathSegment>,
    /// The components, in block order.
    pub components: Vec<PawnComponent>,
    /// Indices into `components` of `StatContainer`, `ActorState` and
    /// `InventoryComponent` (handles 47, 48, 56).
    pub stat_container: usize,
    pub actor_state: usize,
    pub inventory_component: usize,
    /// The `BP_IcarusPlayerCharacterSurvival_C` layout.
    pub layout: Vec<RepCommand>,
    pub engine_net_version: u32,
}

/// One player's pawn values.
#[derive(Clone, Debug, PartialEq)]
pub struct PawnOpenState {
    /// The pawn's key.
    pub key: ActorKey,
    /// The owning PlayerController's and the PlayerState's GUIDs.
    pub controller: u32,
    pub player_state: u32,
    /// The spawn location.
    pub location: [f32; 3],
}

/// Compose the pawn open for its owner's connection (F-797) on
/// `channel_index` in the connection's byte `order`: the captured roles and
/// references, every component opened without properties, no inventories.
pub fn compose_pawn_open(
    cache: &mut NetGuidCache,
    exports: &mut ConnectionExports,
    plan: &PawnOpenPlan,
    state: &PawnOpenState,
    channel_index: u32,
    order: super::archive::ByteOrder,
) -> Result<ComposedActorOpen> {
    for index in [
        plan.stat_container,
        plan.actor_state,
        plan.inventory_component,
    ] {
        ensure!(
            index < plan.components.len(),
            "pawn component index {index} out of range"
        );
    }
    let open = ComponentActorOpen {
        key: state.key,
        archetype: plan.archetype.clone(),
        level: plan.level.clone(),
        location: Some(state.location),
        layout: plan.layout.clone(),
        references: BTreeMap::from([
            (handle::OWNER, Reference::Guid(state.controller)),
            (handle::INSTIGATOR, Reference::Itself),
            (handle::PLAYER_STATE, Reference::Guid(state.player_state)),
            (handle::CONTROLLER, Reference::Guid(state.controller)),
            (
                handle::STAT_CONTAINER,
                Reference::Component(plan.stat_container),
            ),
            (handle::ACTOR_STATE, Reference::Component(plan.actor_state)),
            (
                handle::INVENTORY_COMPONENT,
                Reference::Component(plan.inventory_component),
            ),
        ]),
        values: BTreeMap::from([
            (handle::REMOTE_ROLE, RepValue::Byte(OWNER_REMOTE_ROLE)),
            (handle::ROLE, RepValue::Byte(ROLE)),
        ]),
        components: plan
            .components
            .iter()
            .map(|component| OpenComponent {
                name: component.name.clone(),
                checksum: component.checksum,
                block: Some(BlockContent::create_only()),
            })
            .collect(),
        subobjects: Vec::new(),
        engine_net_version: plan.engine_net_version,
    };
    let (composed, _) = compose_component_actor_open(cache, exports, &open, channel_index, order)?;
    Ok(composed)
}

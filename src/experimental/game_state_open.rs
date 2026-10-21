//! Native composition of the GameState open (`F-794`).
//!
//! The original opens the world's one GameState on every connection soon
//! after Join. Its open carries:
//!
//! - an export prefix with the archetype (`Default__<Class>_C` and its
//!   package), the level when the connection lacks it, the `GameModeClass`
//!   and `SpectatorClass` class chains, and the `ConnectedPlayers`,
//!   `DialogueSystem` and `WeatherComp` components under the actor;
//! - a dynamic spawn without location;
//! - one actor block: the world-actor roles, the class and component
//!   references, the manager actors by dynamic GUID, and the state values;
//! - one block per component: `ConnectedPlayers` and `DialogueSystem`
//!   without properties, `WeatherComp` with its properties.
//!
//! GUIDs follow the [`NetGuidCache`] policy: the GameState and the managers
//! are keyed world actors, so a manager the GameState names keeps that GUID
//! when its own open follows (the capture names 74..86 here and opens them
//! later), and the components keep driver-wide GUIDs that every connection
//! receives with its own export. Identifiers come from the release profile;
//! values come from the caller.
use std::collections::BTreeMap;

use anyhow::{Result, ensure};

use super::actor_channel::{
    BlockTarget, ContentBlock, DynamicSpawn, NetGuidRef, SpawnRecord, SubObjectClass,
};
use super::packet::{BitWriter, Payload};
use super::rep_layout::{
    PropertyStream, RepCommand, RepEntry, RepLayoutCodec, RepLayoutLimits, RepValue,
    encode_rep_properties,
};
use super::session_open::{
    ComponentExport, ComposedActorOpen, ConnectionExports, NetGuidCache, PathSegment, actor_codec,
    actor_open_bunch, entry, exports_of,
};

/// Handles of the `AIcarusGameStateSurvival` rep layout (82 handles; names
/// from the native class descriptors, order from the statically observed
/// layout, F-703).
pub mod handle {
    /// `AActor::RemoteRole`.
    pub const REMOTE_ROLE: u32 = 5;
    /// `AActor::Role`.
    pub const ROLE: u32 = 14;
    /// `AGameStateBase::GameModeClass` (`TSubclassOf<AGameModeBase>`).
    pub const GAME_MODE_CLASS: u32 = 16;
    /// `AGameStateBase::SpectatorClass` (`TSubclassOf<ASpectatorPawn>`).
    pub const SPECTATOR_CLASS: u32 = 17;
    /// `AGameStateBase::bReplicatedHasBegunPlay`.
    pub const HAS_BEGUN_PLAY: u32 = 18;
    /// `AGameStateBase::ReplicatedWorldTimeSeconds`.
    pub const WORLD_TIME_SECONDS: u32 = 19;
    /// `AGameState::MatchState` (`FName`).
    pub const MATCH_STATE: u32 = 20;
    /// `AGameState::ElapsedTime`.
    pub const ELAPSED_TIME: u32 = 21;
    /// `AIcarusGameStateBase::ConnectedPlayers` (component).
    pub const CONNECTED_PLAYERS: u32 = 22;
    /// `AIcarusGameStateBase::DialogueSystem` (component).
    pub const DIALOGUE_SYSTEM: u32 = 23;
    /// `AIcarusGameStateBase::RichPresenceGUID`.
    pub const RICH_PRESENCE_GUID: u32 = 24;
    /// `QuestManager` (actor).
    pub const QUEST_MANAGER: u32 = 25;
    /// `WorldTalentManagerClass`: `RepSkip`, never sent.
    pub const WORLD_TALENT_MANAGER_CLASS: u32 = 26;
    /// `WorldTalentManager` (actor).
    pub const WORLD_TALENT_MANAGER: u32 = 27;
    /// `RadiationManager` (actor).
    pub const RADIATION_MANAGER: u32 = 28;
    /// `InventoryContainerManager` (actor).
    pub const INVENTORY_CONTAINER_MANAGER: u32 = 29;
    /// `PlayerHistoryTracker` (actor).
    pub const PLAYER_HISTORY_TRACKER: u32 = 30;
    pub const DEBUG_LOGGING: u32 = 31;
    pub const GOAP_DEBUGGING: u32 = 32;
    pub const GLOBAL_ENV_TEMP_MODIFIER: u32 = 33;
    pub const SEED: u32 = 34;
    pub const SEED_INITIALISED: u32 = 35;
    /// `FLOD` (actor).
    pub const FLOD: u32 = 36;
    /// `DisasterController` (actor).
    pub const DISASTER_CONTROLLER: u32 = 37;
    pub const TIME_OF_DAY: u32 = 38;
    pub const SECONDS_PER_GAME_DAY: u32 = 39;
    pub const PROSPECT_DURATION_SEC: u32 = 40;
    pub const DYNAMIC_QUEST_SEED: u32 = 41;
    pub const LAST_SESSION_PROSPECT_GAME_TIME: u32 = 42;
    /// `GlobalCheatData`: three flattened bools, 43..=45.
    pub const GLOBAL_CHEAT_DATA_FIRST: u32 = 43;
    pub const LEVEL_TIME_ELAPSED_SEC: u32 = 46;
    pub const SESSION_FLAGS: u32 = 47;
    pub const IS_OPEN_WORLD_PROSPECT: u32 = 48;
    pub const IS_OUTPOST_PROSPECT: u32 = 49;
    pub const LOCKED_MISSIONS: u32 = 50;
    /// `ReplicatedActiveProspect` (`FProspectInfo`, 17 commands), 51..=67.
    pub const ACTIVE_PROSPECT_FIRST: u32 = 51;
    /// `NetworkingStatus` (`FNetworkingStatus`, 12 commands), 68..=79.
    pub const NETWORKING_STATUS_FIRST: u32 = 68;
    pub const TIME_BETWEEN_SAVES: u32 = 80;
    pub const METEOR_DIRECTION: u32 = 81;
    pub const SERVER_FPS: u32 = 82;
    /// The last handle of the layout.
    pub const LAST: u32 = 82;
}

/// `ROLE_SimulatedProxy` and `ROLE_Authority`, as captured (F-791, F-794).
const REMOTE_ROLE: u8 = 1;
const ROLE: u8 = 3;

/// Handles the composer writes itself; [`GameStateOpenState::values`] may
/// not name them.
const COMPOSED_HANDLES: [u32; 13] = [
    handle::REMOTE_ROLE,
    handle::ROLE,
    handle::GAME_MODE_CLASS,
    handle::SPECTATOR_CLASS,
    handle::CONNECTED_PLAYERS,
    handle::DIALOGUE_SYSTEM,
    handle::QUEST_MANAGER,
    handle::WORLD_TALENT_MANAGER,
    handle::RADIATION_MANAGER,
    handle::INVENTORY_CONTAINER_MANAGER,
    handle::PLAYER_HISTORY_TRACKER,
    handle::FLOD,
    handle::DISASTER_CONTROLLER,
];

/// The GameState's replicated components, in serialization order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GameStateComponents {
    /// Referenced by handle 22; opened without properties.
    pub connected_players: ComponentExport,
    /// Referenced by handle 23; opened without properties.
    pub dialogue_system: ComponentExport,
    /// Not referenced by a property; opened with its properties.
    pub weather: ComponentExport,
}

/// Everything a session needs to open the GameState: release identifiers
/// with the map's level chain, and the rep layouts.
#[derive(Clone, Debug)]
pub struct GameStateOpenPlan {
    /// `Default__<Class>_C`, then its package.
    pub archetype: Vec<PathSegment>,
    /// `PersistentLevel`, the world, then the map package.
    pub level: Vec<PathSegment>,
    /// The game mode class, then its package.
    pub game_mode_class: Vec<PathSegment>,
    /// The spectator pawn class, then its package.
    pub spectator_class: Vec<PathSegment>,
    pub components: GameStateComponents,
    /// The `AIcarusGameStateSurvival` layout.
    pub layout: Vec<RepCommand>,
    /// The `UWeatherManagerComponent` layout.
    pub weather_layout: Vec<RepCommand>,
    pub engine_net_version: u32,
}

/// The manager actors the GameState references, by world-actor key; `None`
/// leaves the reference at its default (null) and unsent.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GameStateManagers {
    pub quest_manager: Option<u64>,
    pub world_talent_manager: Option<u64>,
    pub radiation_manager: Option<u64>,
    pub inventory_container_manager: Option<u64>,
    pub player_history_tracker: Option<u64>,
    pub flod: Option<u64>,
    pub disaster_controller: Option<u64>,
}

impl GameStateManagers {
    /// The referencing handle of each manager, in handle order.
    fn by_handle(&self) -> [(u32, Option<u64>); 7] {
        [
            (handle::QUEST_MANAGER, self.quest_manager),
            (handle::WORLD_TALENT_MANAGER, self.world_talent_manager),
            (handle::RADIATION_MANAGER, self.radiation_manager),
            (
                handle::INVENTORY_CONTAINER_MANAGER,
                self.inventory_container_manager,
            ),
            (handle::PLAYER_HISTORY_TRACKER, self.player_history_tracker),
            (handle::FLOD, self.flod),
            (handle::DISASTER_CONTROLLER, self.disaster_controller),
        ]
    }
}

/// Server-side GameState values.
#[derive(Clone, Debug, Default)]
pub struct GameStateOpenState {
    /// The GameState's world-actor key.
    pub key: u64,
    pub managers: GameStateManagers,
    /// Values for the other handles that differ from the class defaults, as
    /// an initial open sends only those. Handles the composer writes, the
    /// `RepSkip` handle 26 and handles outside `18..=82` are rejected.
    pub values: BTreeMap<u32, RepValue>,
    /// `WeatherComp`'s properties (`UWeatherManagerComponent` handles); empty
    /// opens the component without properties.
    pub weather: Vec<RepEntry>,
}

/// Compose the GameState open for one connection (F-794) on `channel_index`
/// in the connection's byte `order`.
pub fn compose_game_state_open(
    cache: &mut NetGuidCache,
    exports: &mut ConnectionExports,
    plan: &GameStateOpenPlan,
    state: &GameStateOpenState,
    channel_index: u32,
    order: super::archive::ByteOrder,
) -> Result<ComposedActorOpen> {
    for handle in state.values.keys() {
        ensure!(
            (handle::HAS_BEGUN_PLAY..=handle::LAST).contains(handle)
                && *handle != handle::WORLD_TALENT_MANAGER_CLASS
                && !COMPOSED_HANDLES.contains(handle),
            "GameState handle {handle} is composed, RepSkip or out of range"
        );
    }
    let codec = actor_codec(plan.engine_net_version, order);
    let actor = cache.actor_guid(state.key);
    let archetype = cache.export_static(exports, &plan.archetype)?;
    let level = cache.export_static(exports, &plan.level)?;
    let game_mode = cache.export_static(exports, &plan.game_mode_class)?;
    let spectator = cache.export_static(exports, &plan.spectator_class)?;
    let components = &plan.components;
    let connected_players =
        cache.export_keyed_subobject(exports, state.key, actor, &components.connected_players)?;
    let dialogue_system =
        cache.export_keyed_subobject(exports, state.key, actor, &components.dialogue_system)?;
    let mut values: BTreeMap<u32, RepValue> = state.values.clone();
    let mut reference = |handle: u32, guid: u32| {
        values.insert(handle, RepValue::Object(NetGuidRef::plain(guid)));
    };
    reference(handle::GAME_MODE_CLASS, game_mode.guid);
    reference(handle::SPECTATOR_CLASS, spectator.guid);
    reference(handle::CONNECTED_PLAYERS, connected_players.guid);
    reference(handle::DIALOGUE_SYSTEM, dialogue_system.guid);
    for (handle, key) in state.managers.by_handle() {
        if let Some(key) = key {
            reference(handle, cache.actor_guid(key));
        }
    }
    values.insert(handle::REMOTE_ROLE, RepValue::Byte(REMOTE_ROLE));
    values.insert(handle::ROLE, RepValue::Byte(ROLE));
    // The weather component gets its GUID after the actor block's references.
    let weather = cache.export_keyed_subobject(exports, state.key, actor, &components.weather)?;
    let rep_codec = RepLayoutCodec {
        actor: codec,
        checksums: false,
        limits: RepLayoutLimits::default(),
    };
    let actor_stream = PropertyStream {
        entries: values
            .into_iter()
            .map(|(handle, value)| entry(handle, value))
            .collect(),
        terminator_checksum: None,
    };
    let component_block = |guid: u32, payload: Option<Payload>| ContentBlock {
        has_rep_layout: payload.is_some(),
        target: BlockTarget::SubObject {
            object: NetGuidRef::plain(guid),
            class: SubObjectClass::StablyNamed,
        },
        payload: payload.unwrap_or_else(|| Payload::from_writer(BitWriter::new())),
    };
    let weather_payload = if state.weather.is_empty() {
        None
    } else {
        let stream = PropertyStream {
            entries: state.weather.clone(),
            terminator_checksum: None,
        };
        Some(encode_rep_properties(
            &stream,
            &plan.weather_layout,
            &rep_codec,
        )?)
    };
    let blocks = vec![
        ContentBlock {
            has_rep_layout: true,
            target: BlockTarget::Actor,
            payload: encode_rep_properties(&actor_stream, &plan.layout, &rep_codec)?,
        },
        component_block(connected_players.guid, None),
        component_block(dialogue_system.guid, None),
        component_block(weather.guid, weather_payload),
    ];
    let export_payload = exports_of(
        &[
            archetype.clone(),
            level.clone(),
            game_mode,
            spectator,
            connected_players,
            dialogue_system,
            weather,
        ],
        &codec,
    )?;
    let spawn = SpawnRecord {
        actor: NetGuidRef::plain(actor),
        dynamic: Some(DynamicSpawn {
            archetype: NetGuidRef::plain(archetype.guid),
            level: Some(NetGuidRef::plain(level.guid)),
            location: None,
            rotation: None,
            scale: None,
            velocity: None,
        }),
    };
    Ok(ComposedActorOpen {
        exports: export_payload,
        bunch: actor_open_bunch(spawn, blocks, channel_index, &codec)?,
    })
}

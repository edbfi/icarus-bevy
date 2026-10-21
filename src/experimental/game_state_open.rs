//! Native composition of the GameState open (`F-794`).
//!
//! The original opens the world's one GameState on every connection soon
//! after Join. Its open carries:
//!
//! - an export prefix with the archetype (`Default__<Class>_C` and its
//!   package), the level unless the connection acked it, the `GameModeClass`
//!   and `SpectatorClass` class chains, and the `ConnectedPlayers`,
//!   `DialogueSystem` and `WeatherComp` components under the actor;
//! - a dynamic spawn without location;
//! - one actor block: the world-actor roles, the class and component
//!   references, the manager actors by dynamic GUID, and the state values;
//! - one block per component: `ConnectedPlayers` and `DialogueSystem`
//!   without properties, `WeatherComp` with its properties.
//!
//! After the open, [`compose_game_state_update`] frames a property update
//! of the GameState actor (no spawn, one actor block), as the original sends
//! its advancing clocks (F-805).
//!
//! GUIDs follow the [`NetGuidCache`] policy: the GameState and the managers
//! are keyed world actors, so a manager the GameState names keeps that GUID
//! when its own open follows (the capture names 74..86 here and opens them
//! later), and the components keep driver-wide GUIDs that every connection
//! receives with its own export. Identifiers come from the release profile;
//! values come from the caller.
use std::collections::BTreeMap;

use anyhow::{Context, Result, ensure};

use super::actor_channel::{ActorBunchBody, BlockTarget, ContentBlock, encode_actor_bunch_body};
use super::actor_open::{
    BlockContent, ComponentActorOpen, OpenComponent, Reference, compose_component_actor_open,
};
use super::control_messages::ArchiveText;
use super::packet::Payload;
use super::prospect_info::{ProspectInfo, SavedGameMode};
use super::rep_layout::{
    PropertyStream, RepArray, RepCommand, RepEntry, RepLayoutCodec, RepLayoutLimits, RepValue,
    encode_rep_properties,
};
use super::session_open::{
    ActorKey, ComponentExport, ComposedActorOpen, ConnectionExports, NetGuidCache, PathSegment,
    actor_codec,
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

/// The GameState's channel: after the PlayerController's and the pawn's, as
/// captured (F-794).
pub const GAME_STATE_CHANNEL_INDEX: u32 = 4;

/// `AGameState::MatchState` once the match has started
/// (`MatchState::InProgress`, as captured, F-794).
pub const MATCH_STATE_IN_PROGRESS: &str = "InProgress";

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

/// `ReplicatedActiveProspect` (handles 51..=67) for a save's `ProspectInfo`
/// (`F-738`): each field in declaration order, sent when it differs from its
/// default (empty, zero, false), arrays with every element field. Members
/// carry 8 commands and custom settings 2 (`prospect_info_commands`).
pub fn active_prospect_values(info: &ProspectInfo) -> Result<BTreeMap<u32, RepValue>> {
    fn text(units: &[u16]) -> Result<Option<RepValue>> {
        Ok((!units.is_empty())
            .then(|| ArchiveText::new(units))
            .transpose()?
            .map(RepValue::Str))
    }
    fn array(elements: Vec<Vec<RepValue>>) -> Result<Option<RepValue>> {
        if elements.is_empty() {
            return Ok(None);
        }
        let count = u16::try_from(elements.len()).context("array exceeds its count width")?;
        let entries = elements
            .into_iter()
            .flatten()
            .zip(1..)
            .map(|(value, handle)| RepEntry {
                handle,
                checksum: None,
                value,
            })
            .collect();
        Ok(Some(RepValue::DynamicArray(RepArray {
            count,
            stream: PropertyStream {
                entries,
                terminator_checksum: None,
            },
        })))
    }
    let int = |value: i32| (value != 0).then_some(RepValue::Int(value));
    let flag = |value: bool| value.then_some(RepValue::Bool(true));
    let members = info
        .associated_members
        .iter()
        .map(|member| {
            Ok(vec![
                RepValue::Str(ArchiveText::new(&member.account_name)?),
                RepValue::Str(ArchiveText::new(&member.character_name)?),
                RepValue::Str(ArchiveText::new(&member.user_id)?),
                RepValue::Int(member.chr_slot),
                RepValue::Int(member.experience),
                RepValue::Enum(u32::from(member.status)),
                RepValue::Bool(member.settled),
                RepValue::Bool(member.is_currently_playing),
            ])
        })
        .collect::<Result<Vec<_>>>()?;
    let settings = info
        .custom_settings
        .iter()
        .map(|setting| {
            Ok(vec![
                RepValue::Str(ArchiveText::new(&setting.setting_row_name)?),
                RepValue::Int(setting.setting_value),
            ])
        })
        .collect::<Result<Vec<_>>>()?;
    let fields = [
        text(&info.prospect_id)?,
        text(&info.claimed_account_id)?,
        int(info.claimed_account_character),
        text(&info.prospect_dt_key)?,
        text(&info.faction_mission_dt_key)?,
        text(&info.lobby_name)?,
        (info.expire_time != 0).then_some(RepValue::Int64(info.expire_time)),
        (info.prospect_state != 0).then_some(RepValue::Enum(u32::from(info.prospect_state))),
        array(members)?,
        int(info.cost),
        int(info.reward),
        (info.difficulty != 0).then_some(RepValue::Enum(u32::from(info.difficulty))),
        flag(info.insurance),
        flag(info.no_respawns),
        int(info.elapsed_time),
        int(info.selected_drop_point),
        array(settings)?,
    ];
    Ok(fields
        .into_iter()
        .zip(handle::ACTIVE_PROSPECT_FIRST..)
        .filter_map(|(value, handle)| value.map(|value| (handle, value)))
        .collect())
}

/// GameState values a loaded save's game mode record restores (`F-803`):
/// `Seed` with `SeedInitialised`, `TimeOfDay` (as saved; the original
/// advances it by a world rate curve, F-806), `SecondsPerGameDay`,
/// `DynamicQuestSeed` and
/// `LastSessionProspectGameTime` (the saved `ProspectGameTime`), each sent
/// when it differs from its default. `LevelTimeElapsedSec` advances with the
/// world clock and is written when the open is sent.
pub fn saved_game_mode_values(mode: &SavedGameMode) -> BTreeMap<u32, RepValue> {
    let int = |handle, value: i32| (value != 0).then_some((handle, RepValue::Int(value)));
    let float =
        |handle, value: f32| (value != 0.0).then_some((handle, RepValue::Float(value.to_bits())));
    [
        int(handle::SEED, mode.game_state_seed),
        Some((handle::SEED_INITIALISED, RepValue::Bool(true))),
        float(handle::TIME_OF_DAY, mode.time_of_day),
        int(handle::SECONDS_PER_GAME_DAY, mode.seconds_per_game_day),
        int(handle::DYNAMIC_QUEST_SEED, mode.dynamic_quest_seed),
        float(
            handle::LAST_SESSION_PROSPECT_GAME_TIME,
            mode.prospect_game_time,
        ),
    ]
    .into_iter()
    .flatten()
    .collect()
}

/// `LevelTimeElapsedSec` at world time `world_seconds`: the restored clock
/// plus the whole seconds since (as captured, F-803).
pub fn level_time_elapsed_sec(restored: i32, world_seconds: f32) -> i32 {
    restored.saturating_add(world_seconds.max(0.0) as i32)
}

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
    pub quest_manager: Option<ActorKey>,
    pub world_talent_manager: Option<ActorKey>,
    pub radiation_manager: Option<ActorKey>,
    pub inventory_container_manager: Option<ActorKey>,
    pub player_history_tracker: Option<ActorKey>,
    pub flod: Option<ActorKey>,
    pub disaster_controller: Option<ActorKey>,
}

impl GameStateManagers {
    /// The referencing handle of each manager, in handle order.
    fn by_handle(&self) -> [(u32, Option<ActorKey>); 7] {
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
#[derive(Clone, Debug)]
pub struct GameStateOpenState {
    /// The GameState's key.
    pub key: ActorKey,
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
    let mut references = BTreeMap::from([
        (
            handle::GAME_MODE_CLASS,
            Reference::Static(plan.game_mode_class.clone()),
        ),
        (
            handle::SPECTATOR_CLASS,
            Reference::Static(plan.spectator_class.clone()),
        ),
        (handle::CONNECTED_PLAYERS, Reference::Component(0)),
        (handle::DIALOGUE_SYSTEM, Reference::Component(1)),
    ]);
    for (handle, key) in state.managers.by_handle() {
        if let Some(key) = key {
            references.insert(handle, Reference::Actor(key));
        }
    }
    let mut values = state.values.clone();
    values.insert(handle::REMOTE_ROLE, RepValue::Byte(REMOTE_ROLE));
    values.insert(handle::ROLE, RepValue::Byte(ROLE));
    let weather = if state.weather.is_empty() {
        BlockContent::create_only()
    } else {
        let rep_codec = RepLayoutCodec {
            actor: actor_codec(plan.engine_net_version, order),
            checksums: false,
            limits: RepLayoutLimits::default(),
        };
        let stream = PropertyStream {
            entries: state.weather.clone(),
            terminator_checksum: None,
        };
        BlockContent {
            has_rep_layout: true,
            payload: encode_rep_properties(&stream, &plan.weather_layout, &rep_codec)?,
        }
    };
    let component = |export: &ComponentExport, block: BlockContent| OpenComponent {
        name: export.name.clone(),
        checksum: export.checksum,
        block: Some(block.into()),
    };
    let components = &plan.components;
    let open = ComponentActorOpen {
        key: state.key,
        archetype: plan.archetype.clone(),
        level: plan.level.clone(),
        location: None,
        layout: plan.layout.clone(),
        references,
        values,
        components: vec![
            component(&components.connected_players, BlockContent::create_only()),
            component(&components.dialogue_system, BlockContent::create_only()),
            component(&components.weather, weather),
        ],
        subobjects: Vec::new(),
        engine_net_version: plan.engine_net_version,
    };
    let (composed, _) = compose_component_actor_open(cache, exports, &open, channel_index, order)?;
    Ok(composed)
}

/// Frame a property update of the GameState actor (rep `layout`, the plan's)
/// for its open channel: an actor bunch body without spawn holding one actor
/// block with `values` in handle order (F-805: the original sends
/// `TimeOfDay`, the world time and `LevelTimeElapsedSec` this way). Handles
/// outside `18..=82`, the `RepSkip` handle 26 and empty updates are
/// rejected.
pub fn compose_game_state_update(
    layout: &[RepCommand],
    engine_net_version: u32,
    values: &BTreeMap<u32, RepValue>,
    order: super::archive::ByteOrder,
) -> Result<Payload> {
    ensure!(!values.is_empty(), "a GameState update carries a value");
    for handle in values.keys() {
        ensure!(
            (handle::HAS_BEGUN_PLAY..=handle::LAST).contains(handle)
                && *handle != handle::WORLD_TALENT_MANAGER_CLASS,
            "GameState handle {handle} is RepSkip or out of range"
        );
    }
    let codec = actor_codec(engine_net_version, order);
    let rep_codec = RepLayoutCodec {
        actor: codec,
        checksums: false,
        limits: RepLayoutLimits::default(),
    };
    let stream = PropertyStream {
        entries: values
            .iter()
            .map(|(handle, value)| RepEntry {
                handle: *handle,
                checksum: None,
                value: value.clone(),
            })
            .collect(),
        terminator_checksum: None,
    };
    let block = ContentBlock {
        has_rep_layout: true,
        target: BlockTarget::Actor,
        payload: encode_rep_properties(&stream, layout, &rep_codec)?,
    };
    encode_actor_bunch_body(
        &ActorBunchBody {
            spawn: None,
            blocks: vec![block],
        },
        &codec,
    )
}

/// `FNetworkingStatus`, the GameState's `NetworkingStatus` (handles
/// `68..=79`; field names from the client PDB's property statics, sources
/// from the 3.0.31 server's `GatherNetworkingStatus`, F-810).
/// `Debug` shows the player count only, never names.
#[derive(Clone, PartialEq)]
pub struct NetworkingStatus {
    pub current_host_player_name: Vec<u16>,
    pub backup_host_player_name: Vec<u16>,
    /// One name per listed player, in player-array order.
    pub player_names: Vec<Vec<u16>>,
    /// The engine's compressed ping per listed player.
    pub player_ping: Vec<u8>,
    pub saved_locally: bool,
    pub time_between_state_saves: f32,
    pub time_since_database_save: f32,
    pub time_between_heartbeats: f32,
    pub time_since_heartbeat: f32,
    pub end_prospect_update_timeout: f32,
    pub update_prospect_state_failed_counter: i32,
    pub update_unreal_session_failed_counter: i32,
}

/// `CurrentHostPlayerName` on a dedicated server (F-810).
pub const DEDICATED_SERVER_HOST_NAME: &str = "Dedicated Server";

impl std::fmt::Debug for NetworkingStatus {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("NetworkingStatus")
            .field("players", &self.player_names.len())
            .field("saved_locally", &self.saved_locally)
            .finish_non_exhaustive()
    }
}

impl NetworkingStatus {
    /// What a server saving locally gathers (F-810): the dedicated-server
    /// host name, no backup host, saved locally, the state-save interval,
    /// no database save or heartbeat yet (-1), a 10 s heartbeat interval,
    /// the prospect update timeout `max(interval + 120, 180)` and zero
    /// failure counters; no players are listed.
    pub fn local(time_between_state_saves: f32) -> Self {
        Self {
            current_host_player_name: DEDICATED_SERVER_HOST_NAME.encode_utf16().collect(),
            backup_host_player_name: Vec::new(),
            player_names: Vec::new(),
            player_ping: Vec::new(),
            saved_locally: true,
            time_between_state_saves,
            time_since_database_save: -1.0,
            time_between_heartbeats: 10.0,
            time_since_heartbeat: -1.0,
            end_prospect_update_timeout: (time_between_state_saves + 120.0).max(180.0),
            update_prospect_state_failed_counter: 0,
            update_unreal_session_failed_counter: 0,
        }
    }

    /// Every field's value by handle (`68..=79`), defaults included. The
    /// two player arrays must have equal lengths.
    pub fn values(&self) -> Result<BTreeMap<u32, RepValue>> {
        ensure!(
            self.player_names.len() == self.player_ping.len(),
            "every listed player has one name and one ping"
        );
        let array = |elements: Vec<RepValue>| -> Result<RepValue> {
            Ok(RepValue::DynamicArray(RepArray {
                count: u16::try_from(elements.len()).context("array exceeds its count width")?,
                stream: PropertyStream {
                    entries: elements
                        .into_iter()
                        .zip(1..)
                        .map(|(value, handle)| RepEntry {
                            handle,
                            checksum: None,
                            value,
                        })
                        .collect(),
                    terminator_checksum: None,
                },
            }))
        };
        let names = self
            .player_names
            .iter()
            .map(|name| Ok(RepValue::Str(ArchiveText::new(name)?)))
            .collect::<Result<Vec<_>>>()?;
        let pings = self
            .player_ping
            .iter()
            .map(|ping| RepValue::Int(i32::from(*ping)))
            .collect();
        let fields = [
            RepValue::Str(ArchiveText::new(&self.current_host_player_name)?),
            RepValue::Str(ArchiveText::new(&self.backup_host_player_name)?),
            array(names)?,
            array(pings)?,
            RepValue::Bool(self.saved_locally),
            RepValue::Float(self.time_between_state_saves.to_bits()),
            RepValue::Float(self.time_since_database_save.to_bits()),
            RepValue::Float(self.time_between_heartbeats.to_bits()),
            RepValue::Float(self.time_since_heartbeat.to_bits()),
            RepValue::Float(self.end_prospect_update_timeout.to_bits()),
            RepValue::Int(self.update_prospect_state_failed_counter),
            RepValue::Int(self.update_unreal_session_failed_counter),
        ];
        Ok(fields
            .into_iter()
            .zip(handle::NETWORKING_STATUS_FIRST..)
            .map(|(value, handle)| (handle, value))
            .collect())
    }
}

/// The class default of an update-carried GameState handle: zero, false,
/// empty strings and empty arrays (`None` for other handles).
pub fn class_default(handle: u32) -> Option<RepValue> {
    use handle::*;
    let empty_array = || {
        RepValue::DynamicArray(RepArray {
            count: 0,
            stream: PropertyStream {
                entries: Vec::new(),
                terminator_checksum: None,
            },
        })
    };
    Some(match handle {
        WORLD_TIME_SECONDS | TIME_OF_DAY | TIME_BETWEEN_SAVES => RepValue::Float(0),
        LEVEL_TIME_ELAPSED_SEC | SERVER_FPS => RepValue::Int(0),
        h if h == NETWORKING_STATUS_FIRST || h == NETWORKING_STATUS_FIRST + 1 => {
            RepValue::Str(ArchiveText::new(&[]).ok()?)
        }
        h if h == NETWORKING_STATUS_FIRST + 2 || h == NETWORKING_STATUS_FIRST + 3 => empty_array(),
        h if h == NETWORKING_STATUS_FIRST + 4 => RepValue::Bool(false),
        h if (NETWORKING_STATUS_FIRST + 5..=NETWORKING_STATUS_FIRST + 9).contains(&h) => {
            RepValue::Float(0)
        }
        h if h == NETWORKING_STATUS_FIRST + 10 || h == NETWORKING_STATUS_FIRST + 11 => {
            RepValue::Int(0)
        }
        _ => return None,
    })
}

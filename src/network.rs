//! EXPERIMENTAL opt-in session layer for `icarus-server run`.
//!
//! When `[network] enabled = true`, the production run binds one IPv4
//! loopback UDP socket before any world state is opened and installs the
//! loopback peer lab ([`PeerLab`] + [`PeerLabPlugin`]) with game admission
//! enabled: handshake, Hello/Challenge, Login/Welcome and Join against the
//! inferred admission model, nothing more. No replication, gameplay or
//! identity verification runs, and nothing here demonstrates compatibility
//! with any ICARUS client. Status records carry counts only.
//!
//! With `[network] release_profile` set, the release TOML is loaded and
//! validated before the socket binds and its network checksum becomes the
//! lab's local Hello version: a differing Hello gets Upgrade and the peer is
//! removed (`upgrades_sent`, `closed_by_reply`). Unset, any version is accepted.
//! When the profile carries `[player_controller]` identifiers (built in for
//! the qualified builds, F-780), every successful Join is answered with the
//! PlayerController open on actor channel 2 (F-778, F-779): exports, spawn at
//! [`SPAWN_LOCATION`], roles, references and component blocks; with `[pawn]`
//! identifiers the player's pawn follows on channel 3 (F-797: captured roles
//! and references, component bools, [`pawn_content`]; no stats or
//! inventories);
//! with `[game_state]` identifiers the world's GameState on channel 4 (F-794:
//! [`game_state_values`], plus a preserved world's prospect and game mode
//! record, [`saved_game_state`], whose `TimeOfDay` then advances at the
//! constant rate of its saved `SecondsPerGameDay`, F-806); with
//! `[player_state]` identifiers the referenced PlayerState on channel 5
//! (F-784: PlayerId from 256, the Login name and unique net id, `bIsHost`
//! for the first player only). Each open takes as many datagrams as it
//! needs. Once a connection acks the GameState open, its world time, level
//! clock, time of day and `ServerFPS` (this server's frame rate over its
//! poll intervals) follow as property updates (F-805); with
//! [`apply_server_settings`], `TimeBetweenSaves` and a locally saving
//! server's `NetworkingStatus` listing the joined players (F-810). Nothing
//! else replicates.
//!
//! Ordering: the lab polls at most `PACKETS_PER_UPDATE` datagrams once per
//! app update in `RunFixedMainLoopSystems::BeforeFixedMainLoop`, so network
//! input is processed before every `SimulationSet` of that update's fixed
//! ticks. Shutdown first stops admission ([`stop_admission`]), then sends
//! every session the original's shutdown notice (Failure "Host closed the
//! connection.", then the control channel close; F-782), removes them all and
//! drops the socket ([`teardown`]); both are idempotent.
use std::net::{SocketAddr, SocketAddrV4};

use std::collections::BTreeMap;

use anyhow::{Context, Result};
use bevy::prelude::{App, Resource, World};

use crate::{
    config::{NetworkConfig, read_configuration},
    experimental::{
        admission::AdmissionPolicy,
        control_messages::ArchiveText,
        game_state_open::{
            GAME_STATE_CHANNEL_INDEX, GameStateManagers, GameStateOpenState,
            MATCH_STATE_IN_PROGRESS, NetworkingStatus, active_prospect_values, handle,
            saved_game_mode_values,
        },
        pawn_open::{CharacterVitals, PAWN_CHANNEL_INDEX, PawnContent},
        peer_lab::{PeerLab, PeerLabAdmission, PeerLabConfig, PeerLabPlugin, SessionCounts},
        release::{ReleasePin, ReleaseProfile},
        rep_layout::{NetworkName, RepValue},
        server_fps::{DEFAULT_SAMPLES, ServerFpsMeter},
        session_open::ActorKey,
        time_of_day::{RateCurve, TimeOfDayClock},
    },
    preservation::PreservedSection,
};

/// Spectator slots granted beside `max_players` (the lab's default policy).
pub const SPECTATOR_SLOTS: u32 = 2;
/// Lab choice: the PlayerController spawn location (no world geometry or
/// PlayerStart is consulted).
pub const SPAWN_LOCATION: [f32; 3] = [0.0, 0.0, 0.0];

/// The session layer's key for the world's one GameState.
pub const GAME_STATE_KEY: ActorKey = ActorKey::Session(0);

/// The GameState values `run` opens every connection with: play has begun
/// and the match is in progress (as captured, F-794); the lab adds the world
/// time. No manager actors exist yet, so their references stay null, and no
/// survival state (prospect, weather, time of day) is modelled or sent.
pub fn game_state_values() -> Result<GameStateOpenState> {
    Ok(GameStateOpenState {
        key: GAME_STATE_KEY,
        managers: GameStateManagers::default(),
        values: BTreeMap::from([
            (handle::HAS_BEGUN_PLAY, RepValue::Bool(true)),
            (
                handle::MATCH_STATE,
                RepValue::NetworkName(NetworkName::String {
                    text: ArchiveText::text(MATCH_STATE_IN_PROGRESS)?,
                    number: 0,
                }),
            ),
        ]),
        weather: Vec::new(),
    })
}

/// The pawn content `run` opens every player's pawn with: movement frozen
/// at open (as captured, F-797) and authored lab vitals, which are not
/// ICARUS balance values. No `IcarusUID`, tags, stats or inventories are
/// sent: they need imported or catalogued content.
pub fn pawn_content() -> PawnContent {
    PawnContent {
        frozen_movement: true,
        character_state: CharacterVitals {
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
        }
        .values(),
        ..PawnContent::default()
    }
}

/// What a loaded world's preserved prospect adds to the GameState open: its
/// `ReplicatedActiveProspect` from the save's `ProspectInfo`, with
/// `ElapsedTime` from the recorded level clock and no member playing (no
/// connection is matched to a member), as the original showed for its
/// loaded save (F-801); the game mode record's values; and the restored
/// level clock (F-803); and a time-of-day clock from the saved `TimeOfDay`
/// (F-806). The original's rate curve is world data and out of scope: the
/// clock runs at the constant rate whose day lasts the saved positive
/// `SecondsPerGameDay`.
pub struct SavedGameState {
    pub values: BTreeMap<u32, RepValue>,
    pub level_time_elapsed_sec: Option<i32>,
    pub time_of_day: Option<TimeOfDayClock>,
}

pub fn saved_game_state(section: &PreservedSection) -> Result<SavedGameState> {
    let payload = section.decode()?.to_inner_payload()?;
    let saved = crate::prospect::saved_prospect_from_payload(&payload)?;
    let level_time = saved.game_mode.map(|mode| mode.level_time_elapsed_sec);
    let mut values = active_prospect_values(
        &saved
            .info
            .for_session(level_time.unwrap_or(saved.info.elapsed_time), |_| false),
    )?;
    if let Some(mode) = &saved.game_mode {
        values.extend(saved_game_mode_values(mode));
    }
    let time_of_day = saved
        .game_mode
        .filter(|mode| mode.seconds_per_game_day > 0)
        .map(|mode| {
            RateCurve::for_seconds_per_game_day(mode.seconds_per_game_day)
                .map(|curve| TimeOfDayClock::new(curve, mode.time_of_day))
        })
        .transpose()?;
    Ok(SavedGameState {
        values,
        level_time_elapsed_sec: level_time,
        time_of_day,
    })
}

/// Add this server's save settings to `lab`'s GameState (F-810): its
/// autosave interval as `TimeBetweenSaves` and as the state-save interval
/// of a locally saving server's `NetworkingStatus`, which then lists the
/// joined players.
pub fn apply_server_settings(lab: &mut PeerLab, autosave_seconds: u64) -> Result<()> {
    let interval = autosave_seconds as f32;
    lab.extend_game_state_values(BTreeMap::from([(
        handle::TIME_BETWEEN_SAVES,
        RepValue::Float(interval.to_bits()),
    )]))?;
    lab.enable_networking_status(NetworkingStatus::local(interval))
}

/// Add a loaded world's [`saved_game_state`] to `lab`'s GameState open.
pub fn apply_saved_game_state(lab: &mut PeerLab, section: &PreservedSection) -> Result<()> {
    let saved = saved_game_state(section)?;
    lab.extend_game_state_values(saved.values)?;
    if let Some(level_time) = saved.level_time_elapsed_sec {
        lab.set_game_state_level_time(level_time)?;
    }
    if let Some(clock) = saved.time_of_day {
        lab.enable_time_of_day(clock)?;
    }
    Ok(())
}

/// Session counts kept after [`teardown`] so a repeated call and the final
/// record report the same values.
#[derive(Resource, Clone, Debug)]
struct ClosedSessions(SessionCounts);

/// Peer-lab parameters for `network`: admission enabled with `max_players`
/// as the occupancy limit. The peer table holds twice the admitted slots so
/// sessions beyond capacity can still complete a handshake and receive the
/// admission Failure instead of silence.
pub fn peer_lab_config(network: &NetworkConfig) -> PeerLabConfig {
    let defaults = PeerLabAdmission::default();
    let max_players = i32::try_from(network.max_players).unwrap_or(i32::MAX);
    let admission = PeerLabAdmission {
        policy: AdmissionPolicy {
            max_players,
            max_spectators: SPECTATOR_SLOTS as i32,
            ..defaults.policy
        },
        ..defaults
    };
    PeerLabConfig {
        max_peers: 2 * (network.max_players + SPECTATOR_SLOTS) as usize,
        idle_timeout: network.session_timeout(),
        admission: Some(admission),
        ..PeerLabConfig::default()
    }
}

/// A bound session socket and the server's own release pin its Hello
/// version check uses, if one is configured.
pub struct BoundNetwork {
    pub lab: PeerLab,
    pub release: Option<ReleasePin>,
}

/// Load and validate the configured release profile (bounded regular-file
/// read), or `None` when `release_profile` is unset.
pub fn release_profile(network: &NetworkConfig) -> Result<Option<ReleaseProfile>> {
    network
        .release_profile
        .as_deref()
        .map(|path| {
            read_configuration(path)
                .and_then(|text| ReleaseProfile::parse(&text))
                .with_context(|| format!("load network.release_profile {}", path.display()))
        })
        .transpose()
}

/// Validate `network`, load its release profile and only then bind its
/// socket, or return `None` when disabled.
pub fn bind(network: &NetworkConfig) -> Result<Option<BoundNetwork>> {
    if !network.enabled {
        return Ok(None);
    }
    network.validate()?;
    let profile = release_profile(network)?;
    let mut config = peer_lab_config(network);
    if let Some(profile) = &profile {
        config = config.with_release_profile(profile);
    }
    let plan = match (&profile, &config.admission) {
        (Some(profile), Some(admission)) => profile
            .player_controller_open_plan(admission.map_url, SPAWN_LOCATION)
            .context("compose the PlayerController open plan")?,
        _ => None,
    };
    let address = SocketAddrV4::new(network.bind, network.port);
    let mut lab = PeerLab::bind_address(address, config)
        .context("bind the experimental network session socket")?;
    if let Some(plan) = plan {
        lab.enable_player_controller_open(plan)?;
        let player_state = match (&profile, &config.admission) {
            (Some(profile), Some(admission)) => profile
                .player_state_open_plan(admission.map_url)
                .context("compose the PlayerState open plan")?,
            _ => None,
        };
        let pawn = match (&profile, &config.admission) {
            (Some(profile), Some(admission)) => profile
                .pawn_open_plan(admission.map_url)
                .context("compose the pawn open plan")?,
            _ => None,
        };
        if let Some(pawn) = pawn {
            lab.enable_pawn_open(pawn, PAWN_CHANNEL_INDEX, pawn_content())?;
        }
        let game_state = match (&profile, &config.admission) {
            (Some(profile), Some(admission)) => profile
                .game_state_open_plan(admission.map_url)
                .context("compose the GameState open plan")?,
            _ => None,
        };
        if let Some(game_state) = game_state {
            lab.enable_game_state_open(game_state, game_state_values()?, GAME_STATE_CHANNEL_INDEX)?;
            // The server's own frame rate, measured as the original does.
            lab.enable_server_fps(ServerFpsMeter::new(DEFAULT_SAMPLES)?)?;
        }
        if let Some(player_state) = player_state {
            lab.enable_player_state_open(player_state)?;
        }
    }
    Ok(Some(BoundNetwork {
        lab,
        release: profile.map(|profile| profile.pin),
    }))
}

/// Install a bound lab into the production app; returns its local address.
pub fn install(app: &mut App, lab: PeerLab) -> Result<SocketAddr> {
    let address = lab.local_addr().context("network session socket address")?;
    app.insert_resource(lab).add_plugins(PeerLabPlugin);
    Ok(address)
}

/// Current counts, the counts kept by [`teardown`], or `None` when the
/// session layer was never installed.
pub fn session_counts(world: &World) -> Option<SessionCounts> {
    world
        .get_resource::<PeerLab>()
        .map(PeerLab::session_counts)
        .or_else(|| {
            world
                .get_resource::<ClosedSessions>()
                .map(|closed| closed.0.clone())
        })
}

/// Stop admitting new sessions; live sessions are kept. Idempotent.
pub fn stop_admission(world: &mut World) {
    if let Some(mut lab) = world.get_resource_mut::<PeerLab>() {
        lab.stop_admission();
    }
}

/// Stop admission, notify and remove every session and drop the socket.
/// Idempotent: a repeated call returns the same final counts.
pub fn teardown(world: &mut World) -> Option<SessionCounts> {
    if let Some(mut lab) = world.remove_resource::<PeerLab>() {
        lab.shutdown();
        let counts = lab.session_counts();
        drop(lab);
        world.insert_resource(ClosedSessions(counts));
    }
    world
        .get_resource::<ClosedSessions>()
        .map(|closed| closed.0.clone())
}

use std::{
    fs::OpenOptions,
    io::Read,
    net::Ipv4Addr,
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{Context, Result, ensure};
use clap::Args;
use serde::{Deserialize, Serialize};

use crate::experimental::admission::JoinPassword;

const MAX_CONFIG_BYTES: u64 = 64 * 1024;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct ServerConfig {
    pub data_dir: PathBuf,
    pub tick_hz: u32,
    pub max_catch_up_ticks: u32,
    pub autosave_seconds: u64,
    pub shutdown_timeout_seconds: u64,
    pub status_interval_seconds: u64,
    /// Experimental session layer; last so TOML writes it as a `[network]` table.
    pub network: NetworkConfig,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            data_dir: PathBuf::from("work/runtime"),
            tick_hz: 30,
            max_catch_up_ticks: 4,
            autosave_seconds: 60,
            shutdown_timeout_seconds: 10,
            status_interval_seconds: 10,
            network: NetworkConfig::default(),
        }
    }
}

/// Largest accepted `network.max_players`.
pub const MAX_NETWORK_PLAYERS: u32 = 64;
/// Largest accepted `network.session_timeout_seconds`.
pub const MAX_SESSION_TIMEOUT_SECONDS: u64 = 600;
/// Largest accepted `network.server_frozen_movement_release_delay_ms` (10 minutes).
pub const MAX_FROZEN_MOVEMENT_RELEASE_DELAY_MS: u64 = 600_000;

/// EXPERIMENTAL opt-in UDP session layer (`[network]`), built from the
/// loopback peer lab. It demonstrates no ICARUS compatibility and runs no
/// gameplay; disabled by default.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct NetworkConfig {
    /// Authored fresh-world checkpoints. The delayed pawn exchange is
    /// unqualified with stock clients; absence retains the ordinary path.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub player_persistence: Option<PlayerPersistenceConfig>,
    /// Independent integer vital units per simulated second; requires checkpoints.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub player_consumption: Option<crate::native_player_runtime::ConsumptionConfig>,
    /// Explicit bounded Steam hosting; absent retains the loopback lab.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hosting: Option<crate::native_hosting::HostingConfig>,
    /// Explicit independently authored seed and FLOD bootstrap, requiring a release profile.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub authored_world: Option<crate::authored_world_bootstrap::AuthoredWorldBootstrap>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub controller_startup: Vec<crate::experimental::peer_lab::ControllerStartupAction>,
    pub enabled: bool,
    /// IPv4 address to bind; nonloopback requires authenticated hosting.
    pub bind: Ipv4Addr,
    /// UDP port; zero selects an ephemeral port. The default matches the game
    /// port the offline reference probe targets; no compatibility is implied.
    pub port: u16,
    /// Admission occupancy limit; the official documented player limit is 8.
    pub max_players: u32,
    /// The operator's join password, as the official server's `JoinPassword`:
    /// absent or empty admits without one; otherwise a Login whose URL
    /// `password` option differs (ASCII case ignored) is refused with
    /// `Invalid Password` before the capacity checks. At most 64 UTF-16
    /// units, no control characters, no `?` (it delimits login options).
    /// Never printed: `check-config` omits it and records show only whether
    /// one is set.
    #[serde(skip_serializing, deserialize_with = "join_password")]
    pub join_password: JoinPassword,
    /// Native timeout for no accepted incoming packet, outstanding sent packets
    /// with no ACK/NAK progress, or an unanswered friends query (N-019).
    /// Removal is local; reconnect may be needed.
    pub session_timeout_seconds: u64,
    /// Optional release TOML (such as `config/release.toml`; relative paths use
    /// the working directory). When set, it is loaded and validated before the
    /// socket binds, and a Hello whose network version differs from its
    /// checksum is answered with Upgrade. Unset accepts any Hello version.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub release_profile: Option<PathBuf>,
    /// Optional `Icarus/Content/Data/data.pak` of the free official dedicated
    /// server (fetched at install time, F-828). When set, it is opened before
    /// the socket binds, and its data tables give replicated row handles
    /// their indices (saved session flags; F-830).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data_pak: Option<PathBuf>,
    /// Independently authored gameplay JSON, required when a data pak supplies
    /// protocol row identities. Never an export of packaged gameplay values.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub authored_gameplay: Option<PathBuf>,
    /// Independently authored, strict JSON declaration for schema-4/7/11 native
    /// spatial replication. Relative paths use the working directory. Ignored
    /// when networking is disabled; absence retains ordinary session behavior.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub authored_replication: Option<PathBuf>,
    /// EXPERIMENTAL: replicate each finalised player's
    /// `UReplicatedConnectedPlayer` record under the GameState's
    /// `ConnectedPlayers` (Mode A). Default on; records still need a release
    /// profile naming the record class and the pawn, GameState and PlayerState
    /// opens, and are otherwise left off. `false` makes a comparison run.
    pub connected_player_records: bool,
    /// EXPERIMENTAL: after each player's pawn open, send one reliable
    /// PlayerController property update with handle 17 `Pawn` and handle 20
    /// `IcarusPlayerCharacter` referencing that pawn: the controller pawn the
    /// connected-player record's finalisation check compares (F-1074). Default
    /// on, as the original sends them; unqualified with a stock client.
    /// Needs a release profile with the controller and pawn opens and is
    /// otherwise left off. `false` makes a comparison run.
    pub controller_possession_properties: bool,
    /// EXPERIMENTAL: release each player's pawn from frozen movement. The
    /// pawn still opens with `bServerFrozenMovement` true, as the original
    /// does (F-797, F-1099); once a positive ACK settles the bunch that
    /// created the player's own connected-player record (its finalisation,
    /// F-1077), and [`Self::server_frozen_movement_release_delay_ms`] later,
    /// one unreliable pawn property update sets it false, as the original
    /// does (F-1099), sent again if NAKed. Default on; unqualified with a
    /// stock client. Needs the pawn open and connected-player records and is
    /// otherwise left off. `false` makes a comparison run.
    pub server_frozen_movement_release: bool,
    /// Milliseconds between the record's ACK and the release, so a client
    /// change can be attributed to it; `0..=`[`MAX_FROZEN_MOVEMENT_RELEASE_DELAY_MS`].
    pub server_frozen_movement_release_delay_ms: u64,
    /// EXPERIMENTAL: decode each player's packed moves (pawn field 39) and,
    /// at most every 100 ms and only after a new move, acknowledge the newest
    /// by arrival with one unreliable good-move response (field 30), as the
    /// original does; keep its location as session state within authored
    /// bounds; never correct (F-1127, F-1128). Default off: the next connected
    /// run sends only the pawn unfreeze (`docs/gates.md`); unqualified with a
    /// stock client. Needs the pawn open and is otherwise left off.
    pub pawn_move_acks: bool,
    /// RESERVED, must stay `false`: level visibility actors (dispatching a
    /// client's field-82 level visibility reports, F-1052, and opening each
    /// newly visible level's actors) are a visibility effect that
    /// `docs/gates.md` ("Before enabling visibility dispatch or effects")
    /// still blocks, so the server binary can never turn them on: `true`
    /// fails validation with [`LEVEL_VISIBILITY_ACTORS_GATED`]. The key is
    /// kept so that it is recognised and refused rather than silently
    /// ignored. Only the bounded join probe enables the feature
    /// (`--level-visibility-actors true`), through the lab API, to qualify
    /// the gate's remaining clauses in a connected run.
    pub level_visibility_actors: bool,
    /// `[network.spawn]`: operator spawn points (centimetres, optional yaw
    /// in degrees), at most 64, each component within ±1,048,576 cm, handed
    /// out round-robin by join to the pawn and controller opens (F-1139).
    /// Empty (the default) keeps the plan's location, the origin. Positions
    /// come from the operator's own knowledge of the map; the server ships
    /// no map data. Needs the release's controller open. Last, so TOML
    /// writes it after the `[network]` values.
    #[serde(skip_serializing_if = "crate::spawn_placement::SpawnConfig::is_empty")]
    pub spawn: crate::spawn_placement::SpawnConfig,
}

impl Default for NetworkConfig {
    fn default() -> Self {
        Self {
            player_persistence: None,
            player_consumption: None,
            hosting: None,
            authored_world: None,
            enabled: false,
            controller_startup: Vec::new(),
            bind: Ipv4Addr::LOCALHOST,
            port: 17_777,
            max_players: 8,
            join_password: JoinPassword::NONE,
            session_timeout_seconds: 30,
            release_profile: None,
            data_pak: None,
            authored_gameplay: None,
            authored_replication: None,
            connected_player_records: true,
            controller_possession_properties: true,
            server_frozen_movement_release: true,
            server_frozen_movement_release_delay_ms: 0,
            pawn_move_acks: false,
            level_visibility_actors: false,
            spawn: crate::spawn_placement::SpawnConfig::default(),
        }
    }
}

/// Explicit context for the authored schema-13 player experiment. Kept
/// independently of hosting so an offline reopen validates the same recipe.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PlayerPersistenceConfig {
    pub prospect_id: String,
    pub map_url: String,
    pub prospect_row: String,
    pub difficulty: u8,
}

/// `network.join_password`, refused without quoting it. Read as any TOML
/// value first: serde's own type error for a non-string (a numeric password,
/// say) would quote the value.
fn join_password<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<JoinPassword, D::Error> {
    let toml::Value::String(text) = toml::Value::deserialize(deserializer)? else {
        return Err(serde::de::Error::custom(
            "network.join_password must be a string",
        ));
    };
    JoinPassword::new(&text)
        .map_err(|rules| serde::de::Error::custom(format_args!("network.join_password {rules}")))
}

/// A TOML error as its message and position. The default rendering quotes
/// the offending source line, which may be the join password's.
fn parse_error(text: &str, error: &toml::de::Error) -> anyhow::Error {
    let position = error
        .span()
        .and_then(|span| text.get(..span.start))
        .map(|before| {
            let line = before.matches('\n').count() + 1;
            let column = before.len() - before.rfind('\n').map_or(0, |at| at + 1) + 1;
            format!(" at line {line}, column {column}")
        })
        .unwrap_or_default();
    anyhow::anyhow!("{}{position}", error.message().trim_end())
}

/// Why `[network] level_visibility_actors = true` is refused.
pub const LEVEL_VISIBILITY_ACTORS_GATED: &str = "network.level_visibility_actors is reserved and must be false: opening a level's actors after a level visibility report is gated (docs/gates.md, \"Before enabling visibility dispatch or effects\"); the server cannot enable it, only the bounded join probe can (--level-visibility-actors true)";

impl NetworkConfig {
    pub fn validate(&self) -> Result<()> {
        if let Some(consumption) = self.player_consumption {
            consumption.validate()?;
            ensure!(
                self.player_persistence.is_some(),
                "player consumption requires player persistence"
            );
        }
        if let Some(players) = &self.player_persistence {
            ensure!(
                self.release_profile.is_some()
                    && self.authored_gameplay.is_some()
                    && self.data_pak.is_some()
                    && self.authored_world.is_some(),
                "player persistence requires release, authored world, gameplay and protocol catalog"
            );
            ensure!(
                self.authored_replication.is_none(),
                "player persistence does not support native world composition"
            );
            ensure!(
                !players.prospect_id.is_empty()
                    && players.prospect_id.len() <= 63
                    && !players.prospect_row.is_empty()
                    && players.prospect_row.len() <= 63
                    && players.difficulty <= 4,
                "invalid player persistence context"
            );
            crate::experimental::session_open::map_level_chain(&players.map_url)?;
            if self.enabled {
                let hosting = self
                    .hosting
                    .as_ref()
                    .context("player persistence networking requires authenticated hosting")?;
                ensure!(
                    hosting.prospect_id == players.prospect_id
                        && hosting.map_url == players.map_url
                        && hosting.prospect_row == players.prospect_row
                        && hosting.difficulty == players.difficulty,
                    "player persistence context disagrees with hosting"
                );
            }
        }
        if let Some(hosting) = &self.hosting {
            hosting.validate(self)?;
        }
        ensure!(!self.level_visibility_actors, LEVEL_VISIBILITY_ACTORS_GATED);
        if let Some(world) = &self.authored_world {
            world.validate()?;
            ensure!(
                world.clock.is_none() || self.player_persistence.is_some(),
                "authored clock requires player persistence schema 13"
            );
            ensure!(
                self.release_profile.is_some(),
                "network.authored_world requires a release profile"
            );
        }
        ensure!(
            self.controller_startup.len() <= 32,
            "network.controller_startup exceeds 32 actions"
        );
        ensure!(
            self.bind.is_loopback() || self.hosting.is_some(),
            "network.bind must be an IPv4 loopback address unless network.hosting is configured"
        );
        ensure!(
            (1..=MAX_NETWORK_PLAYERS).contains(&self.max_players),
            "network.max_players must be 1..={MAX_NETWORK_PLAYERS}"
        );
        ensure!(
            (1..=MAX_SESSION_TIMEOUT_SECONDS).contains(&self.session_timeout_seconds),
            "network.session_timeout_seconds must be 1..={MAX_SESSION_TIMEOUT_SECONDS}"
        );
        ensure!(
            self.server_frozen_movement_release_delay_ms <= MAX_FROZEN_MOVEMENT_RELEASE_DELAY_MS,
            "network.server_frozen_movement_release_delay_ms must be 0..={MAX_FROZEN_MOVEMENT_RELEASE_DELAY_MS}"
        );
        ensure!(
            self.release_profile
                .as_ref()
                .is_none_or(|path| !path.as_os_str().is_empty()),
            "network.release_profile is empty"
        );
        ensure!(
            self.data_pak
                .as_ref()
                .is_none_or(|path| !path.as_os_str().is_empty()),
            "network.data_pak is empty"
        );
        ensure!(
            self.authored_gameplay
                .as_ref()
                .is_none_or(|path| !path.as_os_str().is_empty()),
            "network.authored_gameplay is empty"
        );
        ensure!(
            self.authored_replication
                .as_ref()
                .is_none_or(|path| !path.as_os_str().is_empty()),
            "network.authored_replication is empty"
        );
        self.spawn.validate()?;
        ensure!(
            self.spawn.is_empty() || self.release_profile.is_some(),
            "network.spawn requires a release profile"
        );
        Ok(())
    }

    pub fn session_timeout(&self) -> Duration {
        Duration::from_secs(self.session_timeout_seconds)
    }

    pub fn server_frozen_movement_release_delay(&self) -> Duration {
        Duration::from_millis(self.server_frozen_movement_release_delay_ms)
    }
}

#[derive(Args, Debug, Default)]
pub struct ConfigOptions {
    /// Replace TOML startup actions in flag order; clear them with [] in TOML.
    #[arg(long, value_enum)]
    pub controller_startup: Option<Vec<crate::experimental::peer_lab::ControllerStartupAction>>,
    /// Read up to 64 KiB of native TOML. Relative data paths use the working directory.
    #[arg(long)]
    pub config: Option<PathBuf>,
    #[arg(long)]
    pub data_dir: Option<PathBuf>,
    /// Experimental simulation rate, pending reference-server measurements.
    #[arg(long)]
    pub tick_hz: Option<u32>,
    #[arg(long)]
    pub autosave_seconds: Option<u64>,
    /// UDP port of the experimental `[network]` session layer (0: ephemeral);
    /// does not enable it.
    #[arg(long)]
    pub network_port: Option<u16>,
    /// Authored schema-4/7/11 replication declaration (strict JSON). Ignored when
    /// networking is disabled; otherwise requires a release session plan.
    #[arg(long)]
    pub authored_replication: Option<PathBuf>,
}

impl ConfigOptions {
    pub fn resolve(&self) -> Result<ServerConfig> {
        let mut config = match &self.config {
            Some(path) => {
                let text = read_configuration(path)
                    .with_context(|| format!("read configuration {}", path.display()))?;
                toml::from_str::<ServerConfig>(&text)
                    .map_err(|error| parse_error(&text, &error))
                    .context("parse native configuration")?
            }
            None => ServerConfig::default(),
        };
        if let Some(path) = &self.data_dir {
            config.data_dir.clone_from(path);
        }
        if let Some(hz) = self.tick_hz {
            config.tick_hz = hz;
        }
        if let Some(seconds) = self.autosave_seconds {
            config.autosave_seconds = seconds;
        }
        if let Some(port) = self.network_port {
            config.network.port = port;
        }
        if let Some(path) = &self.authored_replication {
            config.network.authored_replication = Some(path.clone());
        }
        if let Some(actions) = &self.controller_startup {
            config.network.controller_startup.clone_from(actions);
        }
        config.validate()?;
        Ok(config)
    }
}

/// Read a bounded (64 KiB) regular-file text input such as a configuration
/// or release profile.
pub(crate) fn read_configuration(path: &Path) -> Result<String> {
    // Follow normal config symlinks, but check the opened handle so a FIFO
    // cannot block startup or replace a regular file between path checks.
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(crate::nonblocking_flag())
        .open(path)?;
    let metadata = file.metadata()?;
    ensure!(
        metadata.is_file(),
        "configuration input must be a regular file"
    );
    ensure!(
        metadata.len() <= MAX_CONFIG_BYTES,
        "configuration exceeds 64 KiB limit"
    );
    let mut text = String::new();
    file.take(MAX_CONFIG_BYTES + 1).read_to_string(&mut text)?;
    // The metadata bound alone does not cover growth after the handle check.
    ensure!(
        text.len() as u64 <= MAX_CONFIG_BYTES,
        "configuration exceeds 64 KiB limit"
    );
    Ok(text)
}

impl ServerConfig {
    pub fn validate(&self) -> Result<()> {
        ensure!(!self.data_dir.as_os_str().is_empty(), "data_dir is empty");
        ensure!((1..=240).contains(&self.tick_hz), "tick_hz must be 1..=240");
        ensure!(
            (1..=16).contains(&self.max_catch_up_ticks),
            "max_catch_up_ticks must be 1..=16"
        );
        ensure!(
            (1..=3600).contains(&self.autosave_seconds),
            "autosave_seconds must be 1..=3600"
        );
        ensure!(
            (1..=300).contains(&self.shutdown_timeout_seconds),
            "shutdown_timeout_seconds must be 1..=300"
        );
        ensure!(
            (1..=3600).contains(&self.status_interval_seconds),
            "status_interval_seconds must be 1..=3600"
        );
        if let Some(clock) = self
            .network
            .authored_world
            .as_ref()
            .and_then(|world| world.clock.as_ref())
        {
            ensure!(
                clock.tick_hz == self.tick_hz,
                "authored clock tick_hz differs from server tick_hz"
            );
        }
        self.network.validate()
    }

    pub fn tick_interval(&self) -> Duration {
        Duration::from_secs_f64(1.0 / f64::from(self.tick_hz))
    }
}

#[cfg(test)]
mod authored_clock_config_tests {
    use super::*;
    use crate::authored_world_bootstrap::{AuthoredClockConfig, AuthoredWorldBootstrap};
    fn offline() -> ServerConfig {
        ServerConfig {
            network: NetworkConfig {
                release_profile: Some("authored-release.toml".into()),
                data_pak: Some("authored.pak".into()),
                authored_gameplay: Some("authored.json".into()),
                player_persistence: Some(PlayerPersistenceConfig {
                    prospect_id: "authored".into(),
                    map_url: "/Authored/Map".into(),
                    prospect_row: "AuthoredRow".into(),
                    difficulty: 2,
                }),
                authored_world: Some(AuthoredWorldBootstrap {
                    seed: 42,
                    seed_initialised: true,
                    flod_tile_count: 0,
                    clock: Some(AuthoredClockConfig {
                        initial_minute: 480,
                        seconds_per_day: 3600,
                        tick_hz: 30,
                    }),
                }),
                ..NetworkConfig::default()
            },
            ..ServerConfig::default()
        }
    }
    #[test]
    fn offline_clock_requires_player_context_and_matching_actual_tick_rate() {
        let mut config = offline();
        assert!(config.validate().is_ok());
        config.tick_hz = 60;
        assert!(
            config
                .validate()
                .unwrap_err()
                .to_string()
                .contains("tick_hz")
        );
        config.tick_hz = 30;
        config.network.player_persistence = None;
        assert!(
            config
                .network
                .validate()
                .unwrap_err()
                .to_string()
                .contains("requires player persistence")
        );
    }
    #[test]
    fn invalid_authored_clock_is_rejected_even_without_networking() {
        let mut config = offline();
        assert!(!config.network.enabled);
        config
            .network
            .authored_world
            .as_mut()
            .unwrap()
            .clock
            .as_mut()
            .unwrap()
            .seconds_per_day = 0;
        assert!(config.validate().is_err());
    }
    #[test]
    fn omitted_clock_keeps_ordinary_tick_rate_configuration_valid() {
        let mut config = offline();
        config.network.authored_world.as_mut().unwrap().clock = None;
        config.tick_hz = 60;
        assert!(config.validate().is_ok());
        config.network.player_persistence = None;
        assert!(config.validate().is_ok());
    }
}

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

/// EXPERIMENTAL opt-in UDP session layer (`[network]`), built from the
/// loopback peer lab. It demonstrates no ICARUS compatibility and runs no
/// gameplay; disabled by default.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct NetworkConfig {
    pub enabled: bool,
    /// IPv4 address to bind; loopback only while the layer is experimental.
    pub bind: Ipv4Addr,
    /// UDP port; zero selects an ephemeral port. The default matches the game
    /// port the offline reference probe targets; no compatibility is implied.
    pub port: u16,
    /// Admission occupancy limit; the official documented player limit is 8.
    pub max_players: u32,
    /// Native timeout for no accepted incoming packet, or outstanding sent
    /// packets with no ACK/NAK progress. Removal is local; reconnect may be needed.
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
}

impl Default for NetworkConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            bind: Ipv4Addr::LOCALHOST,
            port: 17_777,
            max_players: 8,
            session_timeout_seconds: 30,
            release_profile: None,
            data_pak: None,
            authored_gameplay: None,
            authored_replication: None,
        }
    }
}

impl NetworkConfig {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.bind.is_loopback(),
            "network.bind must be an IPv4 loopback address while the session layer is experimental"
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
        Ok(())
    }

    pub fn session_timeout(&self) -> Duration {
        Duration::from_secs(self.session_timeout_seconds)
    }
}

#[derive(Args, Debug, Default)]
pub struct ConfigOptions {
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
            Some(path) => toml::from_str::<ServerConfig>(
                &read_configuration(path)
                    .with_context(|| format!("read configuration {}", path.display()))?,
            )
            .context("parse native configuration")?,
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
        self.network.validate()
    }

    pub fn tick_interval(&self) -> Duration {
        Duration::from_secs_f64(1.0 / f64::from(self.tick_hz))
    }
}

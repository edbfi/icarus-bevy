use std::{fs, path::PathBuf, time::Duration};

use anyhow::{Context, Result, ensure};
use clap::Args;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct ServerConfig {
    pub data_dir: PathBuf,
    pub tick_hz: u32,
    pub max_catch_up_ticks: u32,
    pub autosave_seconds: u64,
    pub shutdown_timeout_seconds: u64,
    pub status_interval_seconds: u64,
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
        }
    }
}

#[derive(Args, Debug, Default)]
pub struct ConfigOptions {
    /// Read a native TOML configuration. Relative data paths use the working directory.
    #[arg(long)]
    pub config: Option<PathBuf>,
    #[arg(long)]
    pub data_dir: Option<PathBuf>,
    /// Experimental simulation rate, pending reference-server measurements.
    #[arg(long)]
    pub tick_hz: Option<u32>,
    #[arg(long)]
    pub autosave_seconds: Option<u64>,
}

impl ConfigOptions {
    pub fn resolve(&self) -> Result<ServerConfig> {
        let mut config = match &self.config {
            Some(path) => toml::from_str::<ServerConfig>(
                &fs::read_to_string(path)
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
        config.validate()?;
        Ok(config)
    }
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
        Ok(())
    }

    pub fn tick_interval(&self) -> Duration {
        Duration::from_secs_f64(1.0 / f64::from(self.tick_hz))
    }
}

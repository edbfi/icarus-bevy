//! Authored-probe session churn against an already running `icarus-server run`
//! on IPv4 loopback: N cycles of handshake, Hello, Login/Welcome, Join and a
//! client close, over K concurrent workers, then one JSON summary (counts and
//! timings only). Demonstrates no compatibility with any ICARUS client.
use std::{net::SocketAddr, path::PathBuf, time::Duration};

use anyhow::Result;
use clap::Parser;
use icarus_bevy::experimental::{
    release::ReleaseProfile,
    session_churn::{ChurnConfig, run_churn},
};

#[derive(Parser)]
struct Options {
    /// The server's loopback address (its `network_listening` record).
    #[arg(long)]
    server: SocketAddr,
    #[arg(long, default_value_t = 200)]
    cycles: u64,
    /// Concurrent workers; keep within the server's `max_players`.
    #[arg(long, default_value_t = 1)]
    concurrency: usize,
    /// Hello network version; default 0 (accepted only without a version check).
    #[arg(long, conflicts_with = "release_profile")]
    network_version: Option<u32>,
    /// Send the network checksum of this release TOML (e.g. config/release.toml).
    #[arg(long)]
    release_profile: Option<PathBuf>,
    /// Bound on every receive of a cycle, in milliseconds.
    #[arg(long, default_value_t = 2_000)]
    step_timeout_ms: u64,
}

fn main() -> Result<()> {
    let options = Options::parse();
    let network_version = match &options.release_profile {
        Some(path) => ReleaseProfile::load(path)?.pin.network_checksum,
        None => options.network_version.unwrap_or(0),
    };
    let summary = run_churn(&ChurnConfig {
        server: options.server,
        cycles: options.cycles,
        concurrency: options.concurrency,
        network_version,
        step_timeout: Duration::from_millis(options.step_timeout_ms),
    })?;
    println!("{}", serde_json::to_string(&summary)?);
    Ok(())
}

//! Authored-probe session hold against an already running `icarus-server run`
//! on IPv4 loopback: join once, acknowledge everything for the given wall
//! time, and print one JSON summary of the GameState updates received
//! (counts, timings and the server's own clock ranges). Demonstrates no
//! compatibility with any ICARUS client.
use std::{net::SocketAddr, path::PathBuf, time::Duration};

use anyhow::Result;
use clap::Parser;
use icarus_bevy::experimental::{
    release::ReleaseProfile,
    session_hold::{HoldConfig, run_hold},
};

#[derive(Parser)]
struct Options {
    /// The server's loopback address (its `network_listening` record).
    #[arg(long)]
    server: SocketAddr,
    /// The server's release TOML (e.g. config/release.toml): its network
    /// checksum and engine network version.
    #[arg(long)]
    release_profile: PathBuf,
    /// How long to hold the joined session, in seconds.
    #[arg(long, default_value_t = 60)]
    seconds: u64,
    /// Bound on each admission receive, in milliseconds.
    #[arg(long, default_value_t = 2_000)]
    step_timeout_ms: u64,
}

fn main() -> Result<()> {
    let options = Options::parse();
    let profile = ReleaseProfile::load(&options.release_profile)?;
    let summary = run_hold(&HoldConfig {
        server: options.server,
        network_version: profile.pin.network_checksum,
        engine_net_version: profile.engine_network_version,
        duration: Duration::from_secs(options.seconds),
        step_timeout: Duration::from_millis(options.step_timeout_ms),
    })?;
    println!("{}", serde_json::to_string(&summary)?);
    Ok(())
}

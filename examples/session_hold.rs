//! Authored-probe session hold against an already running `icarus-server run`
//! on IPv4 loopback: join once (or `--sessions N` times concurrently),
//! acknowledge everything for the given wall time, and print one JSON
//! summary of the GameState updates received (counts, timings and the
//! server's own clock ranges). Demonstrates no compatibility with any ICARUS
//! client.
use std::{net::SocketAddr, path::PathBuf, time::Duration};

use anyhow::Result;
use clap::Parser;
use icarus_bevy::experimental::{
    release::ReleaseProfile,
    session_hold::{HoldConfig, MultiHoldConfig, run_hold, run_holds},
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
    /// Concurrent sessions; above 1 the multi-session summary is printed.
    #[arg(long, default_value_t = 1)]
    sessions: usize,
}

fn main() -> Result<()> {
    let options = Options::parse();
    let profile = ReleaseProfile::load(&options.release_profile)?;
    let hold = HoldConfig {
        server: options.server,
        network_version: profile.pin.network_checksum,
        engine_net_version: profile.engine_network_version,
        duration: Duration::from_secs(options.seconds),
        step_timeout: Duration::from_millis(options.step_timeout_ms),
    };
    let summary = if options.sessions == 1 {
        serde_json::to_string(&run_hold(&hold)?)?
    } else {
        serde_json::to_string(&run_holds(&MultiHoldConfig {
            hold,
            sessions: options.sessions,
        })?)?
    };
    println!("{summary}");
    Ok(())
}

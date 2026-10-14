//! Bounded loopback peer-lab endpoint: handshake plus inferred per-peer packet
//! state. No game admission, identity check or original assets.
use std::{
    thread,
    time::{Duration, Instant},
};

use anyhow::{Result, ensure};
use clap::Parser;
use icarus_bevy::{
    config::ServerConfig,
    experimental::peer_lab::{PeerLab, PeerLabConfig, PeerLabPlugin},
    simulation::{SimulationClock, WorldSnapshot, build_app},
};
use serde_json::json;

#[derive(Parser)]
struct Options {
    /// IPv4 loopback port; zero requests an ephemeral port.
    #[arg(long, default_value_t = 0)]
    port: u16,
    #[arg(long, default_value_t = 30)]
    seconds: u64,
}

fn main() -> Result<()> {
    let options = Options::parse();
    ensure!(
        (1..=600).contains(&options.seconds),
        "seconds must be 1..=600"
    );
    let mut app = build_app(&ServerConfig::default(), &WorldSnapshot::default())?;
    let lab = PeerLab::bind(options.port, PeerLabConfig::default())?;
    println!(
        "{}",
        json!({"event":"peer_lab_listening", "address":lab.local_addr()?.to_string(), "game_admission":false})
    );
    app.insert_resource(lab).add_plugins(PeerLabPlugin);
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(options.seconds) {
        app.update();
        thread::sleep(Duration::from_millis(2));
    }
    let lab = app.world().resource::<PeerLab>();
    println!(
        "{}",
        json!({
            "event":"peer_lab_stopped", "peers":lab.peer_count(), "stats":lab.stats(),
            "tick":app.world().resource::<SimulationClock>().tick, "game_admission":false
        })
    );
    Ok(())
}

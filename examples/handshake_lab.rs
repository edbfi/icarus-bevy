//! Bounded loopback research endpoint. No game admission or original assets.
use std::{
    thread,
    time::{Duration, Instant},
};

use anyhow::{Result, ensure};
use clap::Parser;
use icarus_bevy::{
    config::ServerConfig,
    experimental::handshake_lab::{HandshakeLab, HandshakeLabPlugin},
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
    let lab = HandshakeLab::bind(options.port)?;
    println!(
        "{}",
        json!({"event":"lab_listening", "address":lab.local_addr()?.to_string(), "game_admission":false})
    );
    app.insert_resource(lab).add_plugins(HandshakeLabPlugin);
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(options.seconds) {
        app.update();
        thread::sleep(Duration::from_millis(2));
    }
    let stats = app.world().resource::<HandshakeLab>().stats();
    println!(
        "{}",
        json!({
            "event":"lab_stopped", "received":stats.received, "rejected":stats.rejected,
            "challenges_sent":stats.challenges_sent, "acknowledgements_sent":stats.acknowledgements_sent,
            "io_errors":stats.io_errors, "tick":app.world().resource::<SimulationClock>().tick,
            "game_admission":false
        })
    );
    Ok(())
}

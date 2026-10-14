//! Repeatable synthetic snapshot measurement, not an ICARUS benchmark.
use std::{path::PathBuf, time::Instant};

use anyhow::{Result, ensure};
use clap::Parser;
use icarus_bevy::{
    persistence::SnapshotStore,
    simulation::{DomainId, EntityRecord, Position, WorldSnapshot},
};
use serde_json::json;

#[derive(Parser)]
struct Options {
    #[arg(long)]
    data_dir: PathBuf,
    #[arg(long, default_value_t = 100_000)]
    entities: u64,
    #[arg(long, default_value_t = 20)]
    iterations: u64,
}

fn main() -> Result<()> {
    let options = Options::parse();
    ensure!(
        (1..=250_000).contains(&options.entities),
        "entities must be 1..250000"
    );
    ensure!(
        (1..=100).contains(&options.iterations),
        "iterations must be 1..100"
    );
    let mut store = SnapshotStore::open(&options.data_dir)?;
    ensure!(store.load()?.is_none(), "benchmark needs a fresh directory");
    let mut snapshot = WorldSnapshot {
        entities: (1..=options.entities)
            .map(|id| EntityRecord {
                id: DomainId(id),
                position: Position([1.25, (id % 100) as f32, -2.5]),
            })
            .collect(),
        ..Default::default()
    };
    // Two warm-up commits establish the same two-generation steady state.
    for tick in 1..=options.iterations + 2 {
        snapshot.tick = tick;
        let start = Instant::now();
        let receipt = store.save(&snapshot)?;
        let save_micros = start.elapsed().as_micros();
        let start = Instant::now();
        let loaded = store.load()?.expect("acknowledged generation must exist");
        let load_micros = start.elapsed().as_micros();
        ensure!(
            !loaded.recovered_previous && loaded.snapshot == snapshot,
            "state mismatch"
        );
        println!(
            "{}",
            json!({"event":"snapshot_measurement", "entities":options.entities,
            "iteration":tick, "warmup":tick <= 2, "save_micros":save_micros,
            "load_micros":load_micros, "generation":receipt.generation,
            "state_verified":true, "icarus_workload":false})
        );
    }
    Ok(())
}

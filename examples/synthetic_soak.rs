//! Authored fixed-population workload; this is not an ICARUS gameplay benchmark.
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use anyhow::{Result, ensure};
use bevy::prelude::*;
use clap::Parser;
use icarus_bevy::{
    config::ServerConfig,
    persistence::{SaveWorker, SnapshotStore},
    simulation::{
        DomainId, EntityRecord, Position, SimulationClock, SimulationSet, TickMetrics,
        WorldSnapshot, build_app, capture,
    },
};
use serde_json::json;

#[derive(Parser)]
struct Options {
    #[arg(long)]
    data_dir: PathBuf,
    #[arg(long, default_value_t = 60)]
    duration_seconds: u64,
    #[arg(long, default_value_t = 100_000)]
    entities: u64,
    #[arg(long, default_value_t = 10)]
    save_seconds: u64,
}

#[derive(Resource, Default)]
struct Samples(Vec<u64>);

fn mutate(clock: Res<SimulationClock>, mut positions: Query<(&DomainId, &mut Position)>) {
    for (id, mut position) in &mut positions {
        position.0[0] = ((clock.tick + id.0) % 10_000) as f32;
    }
}

fn sample_tick(metrics: Res<TickMetrics>, mut samples: ResMut<Samples>) {
    // This bound exceeds two hours at 30 Hz, including permitted catch-up.
    if samples.0.len() < 1_000_000 {
        samples.0.push(metrics.last_tick_micros);
    }
}

fn resident_kib() -> Option<u64> {
    std::fs::read_to_string("/proc/self/status")
        .ok()?
        .lines()
        .find_map(|line| {
            line.strip_prefix("VmRSS:")?
                .split_whitespace()
                .next()?
                .parse()
                .ok()
        })
}

fn quantile(samples: &mut [u64], numerator: usize) -> u64 {
    samples.sort_unstable();
    if samples.is_empty() {
        return 0;
    }
    samples[((samples.len() - 1) * numerator) / 100]
}

fn main() -> Result<()> {
    let options = Options::parse();
    ensure!(
        (1..=7200).contains(&options.duration_seconds),
        "duration must be 1..7200 seconds"
    );
    ensure!(
        (1..=250_000).contains(&options.entities),
        "population must be 1..250000"
    );
    ensure!(
        (1..=600).contains(&options.save_seconds),
        "save interval must be 1..600 seconds"
    );
    let store = SnapshotStore::open(&options.data_dir)?;
    ensure!(
        store.load()?.is_none(),
        "workload requires a fresh data directory"
    );
    let snapshot = WorldSnapshot {
        entities: (1..=options.entities)
            .map(|id| EntityRecord {
                id: DomainId(id),
                position: Position([0.0, (id % 100) as f32, 0.0]),
            })
            .collect(),
        ..Default::default()
    };
    let config = ServerConfig {
        data_dir: options.data_dir.clone(),
        ..Default::default()
    };
    let mut app = build_app(&config, &snapshot)?;
    app.init_resource::<Samples>()
        .add_systems(FixedUpdate, mutate.in_set(SimulationSet::Gameplay))
        .add_systems(FixedUpdate, sample_tick.after(SimulationSet::Finish));
    app.finish();
    app.cleanup();
    drop(snapshot);
    let mut worker = SaveWorker::spawn(store)?;
    let stopping = Arc::new(AtomicBool::new(false));
    let signal = stopping.clone();
    ctrlc::set_handler(move || signal.store(true, Ordering::SeqCst))?;
    let start = Instant::now();
    let mut last_save = start;
    let mut last_status = start;
    let mut pending = None::<Instant>;
    let mut latencies = Vec::new();
    let mut captures = Vec::new();
    let mut commits = 0_u64;
    let mut clamped_frames = 0_u64;
    let mut previous_frame = start;
    println!(
        "{}",
        json!({"event":"soak_started", "entities":options.entities,
        "requested_seconds":options.duration_seconds, "save_seconds":options.save_seconds,
        "tick_hz":config.tick_hz, "icarus_workload":false})
    );
    while start.elapsed() < Duration::from_secs(options.duration_seconds)
        && !stopping.load(Ordering::SeqCst)
    {
        let frame = Instant::now();
        if frame.duration_since(previous_frame) > config.tick_interval() * config.max_catch_up_ticks
        {
            clamped_frames += 1;
        }
        previous_frame = frame;
        app.update();
        if let Some(receipt) = worker.poll()? {
            let elapsed = pending
                .take()
                .expect("receipt must match submitted save")
                .elapsed();
            latencies.push(elapsed.as_micros() as u64);
            commits += 1;
            println!(
                "{}",
                json!({"event":"save_committed", "generation":receipt.generation,
                "tick":receipt.tick, "observed_latency_micros":elapsed.as_micros()})
            );
        }
        if last_save.elapsed() >= Duration::from_secs(options.save_seconds) && !worker.is_busy() {
            let capture_start = Instant::now();
            let snapshot = capture(app.world_mut())?;
            captures.push(capture_start.elapsed().as_micros() as u64);
            worker.submit(snapshot)?;
            pending = Some(Instant::now());
            last_save = Instant::now();
        }
        if last_status.elapsed() >= Duration::from_secs(30) {
            let metrics = app.world().resource::<TickMetrics>();
            println!(
                "{}",
                json!({"event":"soak_status", "elapsed_seconds":start.elapsed().as_secs_f64(),
                "tick":app.world().resource::<SimulationClock>().tick, "rss_kib":resident_kib(),
                "save_commits":commits, "save_in_flight":worker.is_busy(),
                "tick_deadline_misses":metrics.deadline_misses, "clamped_frames":clamped_frames})
            );
            last_status = Instant::now();
        }
        thread::sleep(config.tick_interval().saturating_sub(frame.elapsed()));
    }
    let deadline = Instant::now() + Duration::from_secs(30);
    if worker.finish_before(deadline)?.is_some() {
        commits += 1;
    }
    ensure!(
        Instant::now() < deadline,
        "snapshot shutdown deadline exceeded before final capture"
    );
    let expected = capture(app.world_mut())?;
    worker.submit_before(expected.clone(), deadline)?;
    ensure!(
        worker.finish_before(deadline)?.is_some(),
        "final save receipt missing"
    );
    commits += 1;
    drop(worker);
    // The writer closes its lock asynchronously when its request channel closes.
    let mut reopened = None;
    while Instant::now() < deadline {
        match SnapshotStore::open(&options.data_dir) {
            Ok(store) => {
                reopened = Some(store);
                break;
            }
            Err(_) => thread::sleep(Duration::from_millis(10)),
        }
    }
    let loaded = reopened
        .ok_or_else(|| anyhow::anyhow!("writer did not release directory"))?
        .load()?
        .ok_or_else(|| anyhow::anyhow!("committed state missing"))?;
    ensure!(
        !loaded.recovered_previous && loaded.snapshot == expected,
        "final state failed round trip"
    );
    let metrics = app.world().resource::<TickMetrics>();
    let missed = metrics.deadline_misses;
    let mut ticks = app.world_mut().resource_mut::<Samples>();
    let p50 = quantile(&mut ticks.0, 50);
    let p99 = quantile(&mut ticks.0, 99);
    println!(
        "{}",
        json!({"event":"soak_completed", "elapsed_seconds":start.elapsed().as_secs_f64(),
        "entities":options.entities, "tick":expected.tick, "tick_samples":ticks.0.len(),
        "tick_p50_micros":p50, "tick_p99_micros":p99, "tick_deadline_misses":missed,
        "clamped_frames":clamped_frames, "save_commits":commits,
        "save_observed_p99_micros":quantile(&mut latencies,99),
        "capture_p99_micros":quantile(&mut captures,99), "final_state_verified":true,
        "rss_kib":resident_kib(), "interrupted":stopping.load(Ordering::SeqCst)})
    );
    Ok(())
}

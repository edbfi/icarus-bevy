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
use bevy::time::TimeUpdateStrategy;
use clap::Parser;
use icarus_bevy::{
    config::ServerConfig,
    metrics::{GrowthTrend, LatencyHistogram},
    persistence::{SaveReceipt, SaveWorker, SnapshotStore},
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
    /// RSS growth is measured from the first status sample after this point.
    #[arg(long, default_value_t = 600)]
    warmup_seconds: u64,
    /// Run the duration as simulated time: every update advances exactly one
    /// fixed step without pacing to the wall clock, saves fall due on simulated
    /// time and a save still in flight is awaited, so the run covers the
    /// duration's tick and save counts as fast as the host allows.
    #[arg(long)]
    compressed: bool,
    /// Simulation clock at start, to exercise long-uptime counters.
    #[arg(long, default_value_t = 0)]
    start_tick: u64,
}

/// Up to 72 hours, the length of the plan's stability runs.
const MAX_DURATION_SECONDS: u64 = 72 * 60 * 60;
/// Save and capture latencies: 100 µs resolution up to 10 s (800 KiB each).
const SAVE_RESOLUTION_MICROS: u64 = 100;
const SAVE_BUCKETS: usize = 100_000;

/// Status and RSS samples every 30 seconds of the run's clock.
const STATUS_SECONDS: u64 = 30;

fn mutate(clock: Res<SimulationClock>, mut positions: Query<(&DomainId, &mut Position)>) {
    for (id, mut position) in &mut positions {
        position.0[0] = (clock.tick.wrapping_add(id.0) % 10_000) as f32;
    }
}

/// Submission-to-observation latency of the save in flight, and commit count.
struct SaveObserver {
    pending: Option<Instant>,
    latencies: LatencyHistogram,
    commits: u64,
}

impl SaveObserver {
    fn submitted(&mut self) {
        self.pending = Some(Instant::now());
    }

    fn committed(&mut self, receipt: &SaveReceipt) {
        let elapsed = self
            .pending
            .take()
            .expect("receipt must match submitted save")
            .elapsed();
        self.latencies.record(elapsed.as_micros() as u64);
        self.commits += 1;
        println!(
            "{}",
            json!({"event":"save_committed", "generation":receipt.generation,
            "tick":receipt.tick, "observed_latency_micros":elapsed.as_micros()})
        );
    }
}

/// Elapsed time on the run's clock: wall time, or simulated ticks when compressed.
/// Simulated time is exact: the truncated tick interval would drift.
struct RunClock {
    compressed: bool,
    start: Instant,
    start_tick: u64,
    tick_hz: u32,
}

impl RunClock {
    fn elapsed(&self, tick: u64) -> Duration {
        if self.compressed {
            let nanos =
                u128::from(tick - self.start_tick) * 1_000_000_000 / u128::from(self.tick_hz);
            Duration::from_nanos(u64::try_from(nanos).unwrap_or(u64::MAX))
        } else {
            self.start.elapsed()
        }
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

fn main() -> Result<()> {
    let options = Options::parse();
    let stopping = Arc::new(AtomicBool::new(false));
    let signal = stopping.clone();
    ctrlc::set_handler(move || signal.store(true, Ordering::SeqCst))?;
    println!("{}", run(&options, &stopping)?);
    Ok(())
}

/// Runs the workload and returns its completion record.
fn run(options: &Options, stopping: &AtomicBool) -> Result<serde_json::Value> {
    ensure!(
        (1..=MAX_DURATION_SECONDS).contains(&options.duration_seconds),
        "duration must be 1..{MAX_DURATION_SECONDS} seconds"
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
    let config = ServerConfig {
        data_dir: options.data_dir.clone(),
        ..Default::default()
    };
    let duration = Duration::from_secs(options.duration_seconds);
    let duration_ticks = options.duration_seconds * u64::from(config.tick_hz);
    ensure!(
        options
            .start_tick
            .checked_add(duration_ticks.saturating_mul(2))
            .is_some(),
        "start tick leaves no room for the run"
    );
    let snapshot = WorldSnapshot {
        tick: options.start_tick,
        entities: (1..=options.entities)
            .map(|id| EntityRecord {
                id: DomainId(id),
                position: Position([0.0, (id % 100) as f32, 0.0]),
            })
            .collect(),
        ..Default::default()
    };
    let mut app = build_app(&config, &snapshot)?;
    app.add_systems(FixedUpdate, mutate.in_set(SimulationSet::Gameplay));
    if options.compressed {
        app.insert_resource(TimeUpdateStrategy::FixedTimesteps(1));
    }
    app.finish();
    app.cleanup();
    drop(snapshot);
    let mut worker = SaveWorker::spawn(store)?;
    let start = Instant::now();
    let clock = RunClock {
        compressed: options.compressed,
        start,
        start_tick: options.start_tick,
        tick_hz: config.tick_hz,
    };
    let tick = |app: &App| app.world().resource::<SimulationClock>().tick;
    let mut last_save = Duration::ZERO;
    let mut last_status = Duration::ZERO;
    let mut rss_trend = GrowthTrend::default();
    let mut save_waits = 0_u64;
    let mut save_wait_micros = 0_u64;
    let mut saves = SaveObserver {
        pending: None,
        latencies: LatencyHistogram::new(SAVE_RESOLUTION_MICROS, SAVE_BUCKETS),
        commits: 0,
    };
    let mut captures = LatencyHistogram::new(SAVE_RESOLUTION_MICROS, SAVE_BUCKETS);
    let mut rss_warmup_kib = None::<u64>;
    let mut rss_max_kib = None::<u64>;
    let mut clamped_frames = 0_u64;
    let mut previous_frame = start;
    println!(
        "{}",
        json!({"event":"soak_started", "entities":options.entities,
        "requested_seconds":options.duration_seconds, "save_seconds":options.save_seconds,
        "warmup_seconds":options.warmup_seconds, "tick_hz":config.tick_hz,
        "compressed":options.compressed, "start_tick":options.start_tick,
        "icarus_workload":false})
    );
    while clock.elapsed(tick(&app)) < duration && !stopping.load(Ordering::SeqCst) {
        let frame = Instant::now();
        if !options.compressed
            && frame.duration_since(previous_frame)
                > config.tick_interval() * config.max_catch_up_ticks
        {
            clamped_frames += 1;
        }
        previous_frame = frame;
        let before = tick(&app);
        app.update();
        ensure!(
            !options.compressed || tick(&app) - before <= 1,
            "compressed update ran more than one fixed step"
        );
        let elapsed = clock.elapsed(tick(&app));
        if let Some(receipt) = worker.poll()? {
            saves.committed(&receipt);
        }
        if elapsed.saturating_sub(last_save) >= Duration::from_secs(options.save_seconds) {
            if options.compressed && worker.is_busy() {
                // Await the previous save so the run keeps the duration's save count.
                let waited = Instant::now();
                let receipt = loop {
                    if let Some(receipt) = worker.poll()? {
                        break receipt;
                    }
                    thread::sleep(Duration::from_millis(1));
                };
                save_waits += 1;
                save_wait_micros = save_wait_micros
                    .saturating_add(waited.elapsed().as_micros().min(u128::from(u64::MAX)) as u64);
                saves.committed(&receipt);
            }
            if !worker.is_busy() {
                let capture_start = Instant::now();
                let snapshot = capture(app.world_mut())?;
                captures.record(capture_start.elapsed().as_micros() as u64);
                worker.submit(snapshot)?;
                saves.submitted();
                last_save = elapsed;
            }
        }
        if elapsed.saturating_sub(last_status) >= Duration::from_secs(STATUS_SECONDS) {
            let rss = resident_kib();
            if let Some(rss) = rss {
                rss_max_kib = Some(rss_max_kib.map_or(rss, |max| max.max(rss)));
                if elapsed >= Duration::from_secs(options.warmup_seconds) {
                    rss_warmup_kib.get_or_insert(rss);
                    rss_trend.record(elapsed.as_secs_f64() / 3600.0, rss as f64);
                }
            }
            let metrics = app.world().resource::<TickMetrics>();
            println!(
                "{}",
                json!({"event":"soak_status", "elapsed_seconds":start.elapsed().as_secs_f64(),
                "run_seconds":elapsed.as_secs_f64(), "tick":tick(&app), "rss_kib":rss,
                "save_commits":saves.commits, "save_in_flight":worker.is_busy(),
                "tick_deadline_misses":metrics.deadline_misses,
                "tick_p99_micros":metrics.tick_micros.quantile_micros(99),
                "clamped_frames":(!options.compressed).then_some(clamped_frames)})
            );
            last_status = elapsed;
        }
        if !options.compressed {
            thread::sleep(config.tick_interval().saturating_sub(frame.elapsed()));
        }
    }
    // Recovery verification allocates; growth is judged before it starts.
    let rss_before_verification_kib = resident_kib();
    let run_seconds = clock.elapsed(tick(&app)).as_secs_f64();
    let deadline = Instant::now() + Duration::from_secs(30);
    if worker.finish_before(deadline)?.is_some() {
        saves.commits += 1;
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
    saves.commits += 1;
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
    let ticks = &metrics.tick_micros;
    let growth_percent = match (rss_warmup_kib, rss_before_verification_kib) {
        (Some(warmup), Some(end)) if warmup > 0 => {
            Some((end as f64 - warmup as f64) * 100.0 / warmup as f64)
        }
        _ => None,
    };
    Ok(
        json!({"event":"soak_completed", "elapsed_seconds":start.elapsed().as_secs_f64(),
        "compressed":options.compressed, "run_seconds":run_seconds,
        "start_tick":options.start_tick, "save_waits":save_waits,
        "save_wait_micros":save_wait_micros,
        "entities":options.entities, "tick":expected.tick, "tick_samples":ticks.count(),
        "tick_p50_micros":ticks.quantile_micros(50), "tick_p99_micros":ticks.quantile_micros(99),
        "tick_max_micros":ticks.max_micros(), "tick_histogram_overflow":ticks.overflow(),
        "tick_deadline_misses":metrics.deadline_misses,
        "clamped_frames":(!options.compressed).then_some(clamped_frames),
        "save_commits":saves.commits,
        "save_observed_p99_micros":saves.latencies.quantile_micros(99),
        "save_observed_max_micros":saves.latencies.max_micros(),
        "capture_p99_micros":captures.quantile_micros(99),
        "capture_max_micros":captures.max_micros(),
        "latency_resolution_micros":SAVE_RESOLUTION_MICROS, "final_state_verified":true,
        "rss_warmup_kib":rss_warmup_kib, "rss_max_kib":rss_max_kib,
        "rss_before_verification_kib":rss_before_verification_kib,
        "rss_growth_after_warmup_percent":growth_percent,
        "rss_trend_samples":rss_trend.count(),
        "rss_slope_kib_per_hour":rss_trend.slope(),
        "rss_kib":resident_kib(), "interrupted":stopping.load(Ordering::SeqCst)}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn compressed(data_dir: PathBuf, start_tick: u64) -> Options {
        Options {
            data_dir,
            duration_seconds: 120,
            entities: 50,
            save_seconds: 10,
            warmup_seconds: 30,
            compressed: true,
            start_tick,
        }
    }

    #[test]
    fn compressed_run_executes_the_exact_tick_and_save_counts() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let options = compressed(directory.path().join("world"), 0);
        let completed = run(&options, &AtomicBool::new(false)).expect("compressed run");
        // 120 simulated seconds at 30 Hz, saves due every 10 simulated seconds.
        assert_eq!(completed["tick"], 3_600);
        assert_eq!(completed["tick_samples"], 3_600);
        assert_eq!(completed["run_seconds"], 120.0);
        // Twelve periodic saves, the last due at the final tick, then the final save.
        assert_eq!(completed["save_commits"], 13);
        assert_eq!(completed["final_state_verified"], true);
        assert_eq!(completed["clamped_frames"], serde_json::Value::Null);
        assert!(
            completed["elapsed_seconds"].as_f64().expect("wall seconds") < 120.0,
            "compressed run must not pace to the wall clock"
        );
    }

    #[test]
    fn compressed_run_crosses_long_uptime_clock_boundaries() {
        let directory = tempfile::tempdir().expect("temporary directory");
        for start_tick in [u64::from(u32::MAX) - 1_000, (1 << 53) - 1_000] {
            let options = compressed(directory.path().join(start_tick.to_string()), start_tick);
            let completed = run(&options, &AtomicBool::new(false)).expect("compressed run");
            assert_eq!(completed["tick"], start_tick + 3_600);
            assert_eq!(completed["final_state_verified"], true);
        }
    }

    #[test]
    fn start_tick_without_headroom_is_rejected() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let options = compressed(directory.path().join("world"), u64::MAX - 1_000);
        let error = run(&options, &AtomicBool::new(false)).expect_err("no headroom");
        assert!(error.to_string().contains("start tick"), "{error:#}");
    }
}

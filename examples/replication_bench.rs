//! Replication-scheduling stage cost, naive vs grid relevancy; not an ICARUS
//! benchmark. Net parameters are UE engine defaults. With `--source`, actor
//! positions come from a supplied prospect envelope through
//! `import_plan::planned_actors`; only aggregates are printed.
use std::{os::unix::fs::OpenOptionsExt, path::PathBuf};

use anyhow::{Context, Result, ensure};
use bevy::{app::App, time::TimeUpdateStrategy};
use clap::Parser;
use icarus_bevy::{
    config::ServerConfig,
    import_plan::planned_actors,
    replication::{
        DEFAULT_CONNECTION_BUDGET, NetActor, NetParameters, NetViewer, ReplicationConfig,
        ReplicationFrame, ReplicationPlugin, SpatialMode,
    },
    simulation::{DomainId, Position, WorldSnapshot, build_app},
};
use serde_json::json;

#[derive(Parser)]
struct Options {
    /// Synthetic actor count (ignored with `--source`).
    #[arg(long, default_value_t = 2_700)]
    actors: usize,
    /// Viewer counts to measure, one connection each.
    #[arg(long, value_delimiter = ',', default_value = "1,4,8")]
    viewers: Vec<usize>,
    #[arg(long, default_value_t = 300)]
    frames: usize,
    #[arg(long, default_value_t = 20)]
    warmup_frames: usize,
    #[arg(long, default_value_t = DEFAULT_CONNECTION_BUDGET)]
    budget: usize,
    #[arg(long, default_value_t = 1)]
    seed: u64,
    /// Supplied prospect envelope whose planned actor positions replace the
    /// synthetic layout.
    #[arg(long)]
    source: Option<PathBuf>,
}

/// xorshift64*: seeded synthetic layout and viewer placement.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    fn below(&mut self, bound: usize) -> usize {
        (self.next() % bound as u64) as usize
    }

    /// Uniform in `[-1, 1)`.
    fn signed(&mut self) -> f32 {
        ((self.next() >> 11) as f64 / (1_u64 << 52) as f64 - 1.0) as f32
    }
}

/// Clustered bases plus scattered actors over a 400 km square (authored).
fn synthetic_positions(rng: &mut Rng, count: usize) -> Vec<[f32; 3]> {
    let bases: Vec<[f32; 2]> = (0..24)
        .map(|_| [rng.signed() * 200_000.0, rng.signed() * 200_000.0])
        .collect();
    (0..count)
        .map(|_| {
            let z = rng.signed() * 2_000.0;
            if rng.below(10) < 6 {
                let base = bases[rng.below(bases.len())];
                [
                    base[0] + rng.signed() * 3_000.0,
                    base[1] + rng.signed() * 3_000.0,
                    z,
                ]
            } else {
                [rng.signed() * 200_000.0, rng.signed() * 200_000.0, z]
            }
        })
        .collect()
}

fn supplied_positions(source: &std::path::Path) -> Result<Vec<[f32; 3]>> {
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(icarus_bevy::nonblocking_flag())
        .open(source)
        .context("open prospect envelope")?;
    ensure!(file.metadata()?.is_file(), "source must be a regular file");
    let actors = planned_actors(file)?;
    Ok(actors.into_iter().map(|actor| actor.translation).collect())
}

fn stage(
    positions: &[[f32; 3]],
    viewers: &[([f32; 3], [f32; 3])],
    config: ReplicationConfig,
) -> Result<App> {
    let mut app = build_app(&ServerConfig::default(), &WorldSnapshot::default())?;
    app.insert_resource(TimeUpdateStrategy::FixedTimesteps(1))
        .add_plugins(ReplicationPlugin { config });
    for (index, location) in positions.iter().enumerate() {
        let id = DomainId(index as u64 + 1);
        app.world_mut().spawn((
            id,
            Position(*location),
            NetActor {
                id,
                params: NetParameters::default(),
            },
        ));
    }
    for (connection, (location, direction)) in viewers.iter().enumerate() {
        app.world_mut().spawn(NetViewer {
            connection: connection as u32 + 1,
            location: *location,
            direction: *direction,
        });
    }
    app.finish();
    app.cleanup();
    Ok(app)
}

/// Updates until the next replication frame and returns it.
fn step(app: &mut App) -> Result<ReplicationFrame> {
    let before = app
        .world()
        .resource::<ReplicationFrame>()
        .decisions
        .replication_frame;
    for _ in 0..4 {
        app.update();
        let frame = app.world().resource::<ReplicationFrame>();
        if frame.decisions.replication_frame != before {
            return Ok(frame.clone());
        }
    }
    anyhow::bail!("replication frame did not advance")
}

fn summary(mut nanos: Vec<u64>) -> (f64, f64) {
    nanos.sort_unstable();
    let mean = nanos.iter().sum::<u64>() as f64 / nanos.len() as f64;
    let p99 = nanos[(nanos.len() * 99).div_ceil(100) - 1];
    (mean / 1_000.0, p99 as f64 / 1_000.0)
}

fn round1(value: f64) -> f64 {
    (value * 10.0).round() / 10.0
}

fn measure(
    options: &Options,
    positions: &[[f32; 3]],
    viewer_count: usize,
) -> Result<serde_json::Value> {
    let mut rng = Rng(options.seed ^ (viewer_count as u64).wrapping_mul(0x9e37_79b9));
    let viewers: Vec<([f32; 3], [f32; 3])> = (0..viewer_count)
        .map(|_| {
            let near = positions[rng.below(positions.len())];
            let location = [
                near[0] + rng.signed() * 2_000.0,
                near[1] + rng.signed() * 2_000.0,
                near[2] + 180.0,
            ];
            let angle = rng.signed() * std::f32::consts::PI;
            (location, [angle.cos(), angle.sin(), 0.0])
        })
        .collect();
    let config = ReplicationConfig {
        spatial: SpatialMode::Naive,
        connection_budget: options.budget,
        seed: options.seed,
        ..Default::default()
    };
    let mut naive = stage(positions, &viewers, config)?;
    let mut grid = stage(
        positions,
        &viewers,
        ReplicationConfig {
            spatial: SpatialMode::Grid,
            ..config
        },
    )?;
    let (mut naive_nanos, mut grid_nanos) = (Vec::new(), Vec::new());
    // Stage split sums: consider, index, relevancy, prioritize (naive, grid).
    let mut split = [[0_u64; 4]; 2];
    let (mut considered, mut relevant, mut selected, mut samples) = (0_u64, 0_u64, 0_u64, 0_u64);
    for frame in 0..options.warmup_frames + options.frames {
        let a = step(&mut naive)?;
        let b = step(&mut grid)?;
        ensure!(
            a.decisions == b.decisions,
            "grid and naive decisions differ at frame {frame}"
        );
        if frame < options.warmup_frames {
            continue;
        }
        naive_nanos.push(a.timings.total_nanos);
        grid_nanos.push(b.timings.total_nanos);
        for (sums, timings) in split.iter_mut().zip([a.timings, b.timings]) {
            let parts = [
                timings.consider_nanos,
                timings.index_nanos,
                timings.relevancy_nanos,
                timings.prioritize_nanos,
            ];
            for (sum, part) in sums.iter_mut().zip(parts) {
                *sum += part;
            }
        }
        considered += a.decisions.considered as u64;
        for connection in &a.decisions.connections {
            relevant += connection.relevant as u64;
            selected += connection.selected.len() as u64;
            samples += 1;
        }
    }
    let (naive_mean, naive_p99) = summary(naive_nanos);
    let (grid_mean, grid_p99) = summary(grid_nanos);
    let per_viewer = |total: u64| round1(total as f64 / samples.max(1) as f64);
    let split_micros = |sums: &[u64; 4]| {
        let mean = |sum: u64| round1(sum as f64 / options.frames as f64 / 1_000.0);
        json!({"consider": mean(sums[0]), "index": mean(sums[1]),
            "relevancy": mean(sums[2]), "prioritize": mean(sums[3])})
    };
    Ok(json!({
        "viewers": viewer_count,
        "naive_mean_micros": round1(naive_mean), "naive_p99_micros": round1(naive_p99),
        "grid_mean_micros": round1(grid_mean), "grid_p99_micros": round1(grid_p99),
        "naive_split_mean_micros": split_micros(&split[0]),
        "grid_split_mean_micros": split_micros(&split[1]),
        "mean_considered": round1(considered as f64 / options.frames as f64),
        "mean_relevant_per_viewer": per_viewer(relevant),
        "mean_selected_per_viewer": per_viewer(selected),
        "decisions_identical": true,
    }))
}

fn main() -> Result<()> {
    let options = Options::parse();
    ensure!(
        (1..=100_000).contains(&options.frames),
        "frames must be 1..100000"
    );
    ensure!(options.warmup_frames <= 10_000, "warmup must be 0..10000");
    ensure!(
        !options.viewers.is_empty() && options.viewers.iter().all(|v| (1..=64).contains(v)),
        "viewer counts must be 1..64"
    );
    let (source, positions) = match &options.source {
        Some(path) => ("supplied", supplied_positions(path)?),
        None => {
            ensure!(
                (1..=250_000).contains(&options.actors),
                "actors must be 1..250000"
            );
            let mut rng = Rng(options.seed | 1);
            ("synthetic", synthetic_positions(&mut rng, options.actors))
        }
    };
    ensure!(!positions.is_empty(), "no planned actors to replicate");
    ensure!(positions.len() <= 250_000, "more than 250000 actors");
    let runs = options
        .viewers
        .iter()
        .map(|&viewers| measure(&options, &positions, viewers))
        .collect::<Result<Vec<_>>>()?;
    println!(
        "{}",
        json!({"event":"replication_measurement", "source":source,
        "actors":positions.len(), "frames":options.frames,
        "warmup_frames":options.warmup_frames, "connection_budget":options.budget,
        "seed":options.seed, "net_parameters":"ue_engine_defaults", "runs":runs,
        "icarus_compatible":false})
    );
    Ok(())
}

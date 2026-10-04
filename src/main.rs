use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail, ensure};
use clap::{Parser, Subcommand};
use icarus_bevy::{
    config::ConfigOptions,
    persistence::{SaveReceipt, SaveWorker, SaveWorkerDisconnected, SnapshotStore},
    simulation::{SimulationClock, TickMetrics, build_app, capture},
};
use serde_json::json;

#[derive(Parser)]
#[command(
    version,
    about = "Independent headless Bevy foundation; ICARUS connectivity is not implemented"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run the headless foundation. Opens no gameplay or query sockets.
    Run {
        #[command(flatten)]
        options: ConfigOptions,
        /// Exit after this many additional simulation ticks (smoke testing).
        #[arg(long)]
        max_ticks: Option<u64>,
    },
    /// Validate and print effective native configuration without writing files.
    CheckConfig {
        #[command(flatten)]
        options: ConfigOptions,
    },
    /// Inspect an experimental prospect envelope without importing or writing state.
    InspectSave {
        #[arg(long)]
        source: PathBuf,
    },
    /// Reserved interface: official format is unknown; fails without writing.
    ImportSave {
        #[arg(long)]
        source: PathBuf,
        #[arg(long)]
        destination: PathBuf,
        #[arg(long)]
        dry_run: bool,
    },
}

fn main() -> std::process::ExitCode {
    match execute(Cli::parse()) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!(
                "{}",
                json!({"event": "fatal", "error": format!("{error:#}")})
            );
            std::process::ExitCode::FAILURE
        }
    }
}

fn execute(cli: Cli) -> Result<()> {
    match cli.command {
        Command::CheckConfig { options } => {
            println!("{}", toml::to_string_pretty(&options.resolve()?)?);
            Ok(())
        }
        Command::ImportSave { .. } => bail!(
            "ICARUS save import is unavailable until the format and migration fidelity are validated; no files were written"
        ),
        Command::InspectSave { source } => {
            ensure!(
                std::fs::metadata(&source)?.is_file(),
                "source must be a regular file"
            );
            let file = std::fs::File::open(&source).context("open prospect envelope")?;
            ensure!(file.metadata()?.is_file(), "source must be a regular file");
            let inspection = icarus_bevy::prospect::inspect(file)?;
            println!("{}", serde_json::to_string(&inspection)?);
            Ok(())
        }
        Command::Run { options, max_ticks } => run(options, max_ticks),
    }
}

fn saved(receipt: SaveReceipt) {
    eprintln!(
        "{}",
        json!({"event": "save_committed", "generation": receipt.generation, "tick": receipt.tick})
    );
}

fn run(options: ConfigOptions, max_ticks: Option<u64>) -> Result<()> {
    let config = options.resolve()?;
    ensure!(max_ticks != Some(0), "max_ticks must be positive");
    let store = SnapshotStore::open(&config.data_dir)?;
    let loaded = store.load()?;
    if loaded
        .as_ref()
        .is_some_and(|loaded| loaded.recovered_previous)
    {
        eprintln!(
            "{}",
            json!({"event": "recovered_previous_generation", "warning": "current snapshot was unusable; restored preceding committed state"})
        );
    }
    let snapshot = loaded.map(|loaded| loaded.snapshot).unwrap_or_default();
    let target_tick = max_ticks
        .map(|ticks| {
            snapshot
                .tick
                .checked_add(ticks)
                .context("max_ticks overflows simulation clock")
        })
        .transpose()?;
    let mut app = build_app(&config, &snapshot)?;
    app.finish();
    app.cleanup();
    let mut writer = SaveWorker::spawn(store)?;
    let stopping = Arc::new(AtomicBool::new(false));
    let signal = Arc::clone(&stopping);
    ctrlc::set_handler(move || signal.store(true, Ordering::SeqCst))
        .context("install SIGINT/SIGTERM handler")?;

    let mut last_save_request = Instant::now();
    let mut last_status = Instant::now();
    let mut last_commit = None::<Instant>;
    let mut save_failures = 0_u64;
    let mut clamped_frames = 0_u64;
    let mut previous_frame = Instant::now();
    let interval = config.tick_interval();
    eprintln!(
        "{}",
        json!({"event": "started", "tick": snapshot.tick, "tick_hz": config.tick_hz,
        "icarus_compatible": false, "gameplay_ready": false, "native_schema": 1})
    );

    loop {
        let frame = Instant::now();
        if frame.duration_since(previous_frame) > interval * config.max_catch_up_ticks {
            clamped_frames += 1;
        }
        previous_frame = frame;
        // Manual runner still executes Bevy's complete app schedules and clocks.
        app.update();
        match writer.poll() {
            Ok(Some(receipt)) => {
                last_commit = Some(Instant::now());
                saved(receipt);
            }
            Ok(None) => {}
            Err(error) => {
                if error.is::<SaveWorkerDisconnected>() {
                    return Err(error.context("stop simulation after terminal save-worker failure"));
                }
                save_failures += 1;
                eprintln!(
                    "{}",
                    json!({"event": "save_failed", "error": format!("{error:#}")})
                );
            }
        }
        let tick = app.world().resource::<SimulationClock>().tick;
        if stopping.load(Ordering::SeqCst) || target_tick.is_some_and(|target| tick >= target) {
            break;
        }
        if !writer.is_busy()
            && last_save_request.elapsed() >= Duration::from_secs(config.autosave_seconds)
        {
            writer.submit(capture(app.world_mut())?)?;
            last_save_request = Instant::now();
        }
        if last_status.elapsed() >= Duration::from_secs(config.status_interval_seconds) {
            let metrics = app.world().resource::<TickMetrics>();
            eprintln!(
                "{}",
                json!({"event": "status", "tick": tick,
                "gameplay_ready": false, "save_in_flight": writer.is_busy(),
                "save_failures": save_failures,
                "seconds_since_save": last_commit.map(|time| time.elapsed().as_secs()),
                "clamped_frames": clamped_frames,
                "tick_deadline_misses": metrics.deadline_misses,
                "last_tick_micros": metrics.last_tick_micros,
                "max_tick_micros": metrics.max_tick_micros})
            );
            last_status = Instant::now();
        }
        thread::sleep(interval.saturating_sub(frame.elapsed()));
    }

    let deadline = Instant::now() + Duration::from_secs(config.shutdown_timeout_seconds);
    if let Some(receipt) = writer.finish_before(deadline)? {
        saved(receipt);
    }
    writer.submit(capture(app.world_mut())?)?;
    if let Some(receipt) = writer.finish_before(deadline)? {
        saved(receipt);
    }
    eprintln!(
        "{}",
        json!({"event": "stopped", "tick": app.world().resource::<SimulationClock>().tick})
    );
    Ok(())
}

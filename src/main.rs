use std::{
    os::unix::fs::OpenOptionsExt,
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
        /// Also frame opaque root properties with the experimental memory-522 profile.
        #[arg(long)]
        inner_tags: bool,
        /// Also frame recorder fields, leaving component bytes opaque; implies root framing.
        #[arg(long)]
        recorders: bool,
        /// Preserve the payload losslessly, then save and reload it through a
        /// temporary native schema-2 store; reports counts, sizes and timings.
        #[arg(long, conflicts_with_all = ["inner_tags", "recorders"])]
        preserve_check: bool,
    },
    /// Seed a NEW native world whose only content is the lossless, value-opaque
    /// preservation of a supplied prospect. Nothing is interpreted or imported;
    /// an existing world is never overwritten.
    PreserveSave {
        #[arg(long)]
        source: PathBuf,
        #[arg(long)]
        data_dir: PathBuf,
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
        Command::PreserveSave { source, data_dir } => {
            let file = open_regular_source(&source)?;
            println!("{}", preserve_save(file, &data_dir)?);
            Ok(())
        }
        Command::ImportSave { .. } => bail!(
            "ICARUS save import is unavailable until the format and migration fidelity are validated; no files were written"
        ),
        Command::InspectSave {
            source,
            inner_tags,
            recorders,
            preserve_check,
        } => {
            // Check the opened handle: a path can change to a FIFO after a
            // regular-file precheck. Normal regular-file symlinks still work.
            let file = open_regular_source(&source)?;
            let output = if preserve_check {
                serde_json::to_string(&preserve_check_report(file)?)?
            } else if recorders {
                serde_json::to_string(&icarus_bevy::prospect::inspect_with_recorders(file)?)?
            } else if inner_tags {
                serde_json::to_string(&icarus_bevy::prospect::inspect_with_inner_tags(file)?)?
            } else {
                serde_json::to_string(&icarus_bevy::prospect::inspect(file)?)?
            };
            println!("{output}");
            Ok(())
        }
        Command::Run { options, max_ticks } => run(options, max_ticks),
    }
}

/// Store files after a save: current generation bytes and every sidecar's
/// (name, bytes, inode, modification time in nanoseconds).
type StoreFiles = (u64, Vec<(String, u64, u64, i128)>);

fn store_files(root: &std::path::Path, generation: u64) -> Result<StoreFiles> {
    use std::os::unix::fs::MetadataExt;

    let generation_bytes =
        std::fs::metadata(root.join(format!("snapshot-{generation}.json")))?.len();
    let mut sidecars = Vec::new();
    for entry in std::fs::read_dir(root)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if icarus_bevy::preservation::is_sidecar_name(&name) {
            let metadata = entry.metadata()?;
            let modified =
                i128::from(metadata.mtime()) * 1_000_000_000 + i128::from(metadata.mtime_nsec());
            sidecars.push((name, metadata.len(), metadata.ino(), modified));
        }
    }
    Ok((generation_bytes, sidecars))
}

/// Preserve a supplied envelope and prove the native persistence round trip in
/// a private temporary store that is removed afterwards. Nothing is imported.
/// A second save shows the write-once sidecar is reused rather than rewritten.
/// Open a supplied envelope nonblocking and admit only an opened regular file.
fn open_regular_source(source: &std::path::Path) -> Result<std::fs::File> {
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(icarus_bevy::nonblocking_flag())
        .open(source)
        .context("open prospect envelope")?;
    ensure!(file.metadata()?.is_file(), "source must be a regular file");
    Ok(file)
}

/// Preserve a supplied envelope into a fresh native world at tick 0. The
/// directory may exist but must hold no committed world.
fn preserve_save(file: std::fs::File, data_dir: &std::path::Path) -> Result<serde_json::Value> {
    use icarus_bevy::{
        preservation::PreserveLimits,
        simulation::{SNAPSHOT_SCHEMA_REFERENCED, WorldSnapshot},
    };

    let preserved = icarus_bevy::prospect::preserve(file, PreserveLimits::default())?;
    let summary = serde_json::to_value(preserved.preserved.summary())?;
    let mut store = SnapshotStore::open(data_dir)?;
    ensure!(
        store.load()?.is_none(),
        "data directory already holds a native world; refusing to overwrite it"
    );
    let snapshot = WorldSnapshot {
        schema_version: SNAPSHOT_SCHEMA_REFERENCED,
        preserved_prospect: Some(std::sync::Arc::new(preserved.preserved)),
        ..WorldSnapshot::default()
    };
    let receipt = store.save(&snapshot)?;
    Ok(json!({
        "event": "preserved_world_created",
        "validation": "experimental_lossless_preservation",
        "envelope": preserved.envelope,
        "preservation": summary,
        "native_schema": SNAPSHOT_SCHEMA_REFERENCED,
        "generation": receipt.generation,
        "world_imported": false,
        "gameplay_ready": false,
    }))
}

fn preserve_check_report(file: std::fs::File) -> Result<serde_json::Value> {
    use icarus_bevy::{
        persistence::SnapshotStore,
        preservation::PreserveLimits,
        simulation::{SNAPSHOT_SCHEMA_REFERENCED, WorldSnapshot},
    };
    use std::{sync::Arc, time::Instant};

    let started = Instant::now();
    let preserved = icarus_bevy::prospect::preserve(file, PreserveLimits::default())?;
    let preserve_ms = started.elapsed().as_millis();
    let payload_sha256 = preserved.preserved.payload_sha256();
    let summary = serde_json::to_value(preserved.preserved.summary())?;
    let mut snapshot = WorldSnapshot {
        schema_version: SNAPSHOT_SCHEMA_REFERENCED,
        preserved_prospect: Some(Arc::new(preserved.preserved)),
        ..WorldSnapshot::default()
    };
    let directory = tempfile::tempdir().context("create temporary native store")?;
    let mut store = SnapshotStore::open(directory.path())?;
    let saved = Instant::now();
    let first = store.save(&snapshot)?;
    let save_ms = saved.elapsed().as_millis();
    let (generation_bytes, sidecars) = store_files(directory.path(), first.generation)?;
    ensure!(
        sidecars.len() == 1,
        "expected exactly one preserved sidecar"
    );
    snapshot.tick += 1;
    let saved = Instant::now();
    let second = store.save(&snapshot)?;
    let second_save_ms = saved.elapsed().as_millis();
    let (second_generation_bytes, after) = store_files(directory.path(), second.generation)?;
    ensure!(
        after == sidecars,
        "second save rewrote the preserved sidecar"
    );
    let loaded_at = Instant::now();
    let loaded = store
        .load()?
        .context("saved native generation is missing")?;
    let load_ms = loaded_at.elapsed().as_millis();
    let reloaded = loaded
        .snapshot
        .preserved_prospect
        .context("reloaded generation lost its preserved section")?;
    ensure!(
        reloaded.payload_sha256() == payload_sha256
            && reloaded.to_inner_payload()?.len() == reloaded.payload_bytes(),
        "reloaded preserved section differs"
    );
    drop(store);
    directory.close().context("remove temporary native store")?;
    Ok(json!({
        "validation": "experimental_lossless_preservation",
        "envelope": preserved.envelope,
        "preservation": summary,
        "native_schema": SNAPSHOT_SCHEMA_REFERENCED,
        "native_generation_bytes": generation_bytes,
        "native_second_generation_bytes": second_generation_bytes,
        "native_sidecar_bytes": sidecars[0].1,
        "native_sidecar_rewritten": false,
        "native_round_trip": true,
        "preserve_ms": preserve_ms, "save_ms": save_ms,
        "second_save_ms": second_save_ms, "load_ms": load_ms,
        "world_imported": false,
    }))
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
        "icarus_compatible": false, "gameplay_ready": false,
        "native_schema": snapshot.schema_version})
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
                "max_tick_micros": metrics.max_tick_micros,
                "tick_p50_micros": metrics.tick_micros.quantile_micros(50),
                "tick_p99_micros": metrics.tick_micros.quantile_micros(99)})
            );
            last_status = Instant::now();
        }
        thread::sleep(interval.saturating_sub(frame.elapsed()));
    }

    let deadline = Instant::now() + Duration::from_secs(config.shutdown_timeout_seconds);
    // A failed in-flight autosave is reported as in the loop; the final save still runs.
    match writer.settle_before(deadline)? {
        Some(Ok(receipt)) => saved(receipt),
        Some(Err(error)) => eprintln!(
            "{}",
            json!({"event": "save_failed", "error": format!("{error:#}")})
        ),
        None => {}
    }
    ensure!(
        Instant::now() < deadline,
        "snapshot shutdown deadline exceeded before final capture"
    );
    writer.submit_before(capture(app.world_mut())?, deadline)?;
    if let Some(receipt) = writer.finish_before(deadline)? {
        saved(receipt);
    }
    eprintln!(
        "{}",
        json!({"event": "stopped", "tick": app.world().resource::<SimulationClock>().tick})
    );
    Ok(())
}

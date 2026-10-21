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
use bevy::time::TimeUpdateStrategy;
use clap::{Parser, Subcommand};
use icarus_bevy::{
    config::ConfigOptions,
    network,
    persistence::{SaveReceipt, SaveWorker, SaveWorkerDisconnected, SnapshotStore},
    simulation::{RunClock, SimulationClock, TickMetrics, build_app, capture},
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
    /// Run the headless foundation. Opens no socket unless the experimental,
    /// loopback-only `[network]` session layer is enabled; no gameplay runs.
    Run {
        #[command(flatten)]
        options: ConfigOptions,
        /// Exit after this many additional simulation ticks (smoke testing).
        #[arg(long)]
        max_ticks: Option<u64>,
        /// Qualification only: run the ticks as simulated time, one fixed step
        /// per update without pacing; autosave and status intervals count
        /// simulated seconds and a due autosave awaits one still in flight.
        #[arg(long, requires = "max_ticks")]
        compressed: bool,
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
        /// Stage and validate the `FProspectSaveState` root schema, version
        /// migration (`F-715`), `ProspectInfo` inner tags (`F-726`), and bounded
        /// base-component framing of every `StateRecorderBlob` (`F-403`, `F-733`)
        /// without importing or writing state.
        #[arg(
            long,
            conflicts_with_all = ["inner_tags", "recorders", "preserve_check"]
        )]
        stage_check: bool,
        /// Optional expected prospect name (normalizes an ASCII case-insensitive
        /// `.json` suffix via `F-715`) to compare against `ProspectInfo.ProspectID`
        /// (`F-717`) during `--stage-check`.
        #[arg(long, requires = "stage_check")]
        expect_prospect_name: Option<String>,
        /// Preserve the payload losslessly, then save and reload it through a
        /// temporary native schema-3 store; reports counts, sizes, timings and
        /// the resident set with and without the decoded tree (Linux only).
        #[arg(
            long,
            conflicts_with_all = ["inner_tags", "recorders", "stage_check"]
        )]
        preserve_check: bool,
        /// Stage the prospect, then type every recorder body's values and
        /// require each to re-encode exactly; reports typed and uninterpreted
        /// leaf counts per class pseudonym without importing or writing state.
        #[arg(
            long,
            conflicts_with_all = ["inner_tags", "recorders", "stage_check", "preserve_check"]
        )]
        typed_check: bool,
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
    /// Plan a native import of a supplied prospect. `--dry-run` prints the
    /// plan (counts only); without it the import is refused while the plan has
    /// blockers. Nothing is ever written; the destination is not touched.
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
        Command::ImportSave {
            source, dry_run, ..
        } => {
            let file = open_regular_source(&source)?;
            let plan = icarus_bevy::import_plan::plan_import(
                file,
                &icarus_bevy::import_plan::ContentCatalog::empty(),
            )?;
            if dry_run {
                println!("{}", serde_json::to_string(&plan)?);
                return Ok(());
            }
            ensure!(
                plan.importable,
                "import blocked by {}; run with --dry-run for the plan; no files were written",
                serde_json::to_string(&plan.blockers)?
            );
            bail!("native world import writing is not implemented; no files were written")
        }
        Command::InspectSave {
            source,
            inner_tags,
            recorders,
            stage_check,
            expect_prospect_name,
            preserve_check,
            typed_check,
        } => {
            // Check the opened handle: a path can change to a FIFO after a
            // regular-file precheck. Normal regular-file symlinks still work.
            let file = open_regular_source(&source)?;
            let output = if typed_check {
                serde_json::to_string(&icarus_bevy::prospect::typed_check(file)?)?
            } else if preserve_check {
                serde_json::to_string(&preserve_check_report(file)?)?
            } else if stage_check {
                serde_json::to_string(&icarus_bevy::prospect::stage_prospect(
                    file,
                    icarus_bevy::experimental::component::MAX_BOUNDED_COMPONENT_BYTES,
                    expect_prospect_name.as_deref(),
                )?)?
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
        Command::Run {
            options,
            max_ticks,
            compressed,
        } => run(options, max_ticks, compressed),
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

/// Resident set size of this process from `/proc/self/status` (`VmRSS`), or
/// `None` where that interface is unavailable.
fn resident_bytes() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let line = status
        .lines()
        .find_map(|line| line.strip_prefix("VmRSS:"))?;
    let kibibytes = line.trim().strip_suffix("kB")?.trim().parse::<u64>().ok()?;
    kibibytes.checked_mul(1024)
}

/// Preserve a supplied envelope and prove the native persistence round trip in
/// a private temporary store that is removed afterwards. Nothing is imported.
/// A second save shows the write-once sidecar is reused rather than rewritten.
/// The reloaded compact section is then decoded on demand to its full tree to
/// report the resident set with and without that tree.
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
    let original = snapshot
        .preserved_prospect
        .clone()
        .context("preserved section missing before save")?;
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
    ensure!(*reloaded == *original, "reloaded preserved section differs");
    drop((snapshot, original));
    let section_only = resident_bytes();
    let decoded_at = Instant::now();
    // Verified again: re-hashed, bounded decode and byte-identical reconstruction.
    let tree = reloaded.decode()?;
    let decode_ms = decoded_at.elapsed().as_millis();
    let with_tree = resident_bytes();
    ensure!(
        tree.payload_sha256() == payload_sha256 && tree.payload_bytes() == reloaded.payload_bytes(),
        "decoded preserved section differs"
    );
    drop(tree);
    let after_tree_dropped = resident_bytes();
    let held_section_bytes = reloaded.held_bytes();
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
        "second_save_ms": second_save_ms, "load_ms": load_ms, "decode_ms": decode_ms,
        "held_section_bytes": held_section_bytes,
        // Linux VmRSS; null elsewhere. Freed memory may stay with the allocator.
        "resident_bytes": {
            "section_only": section_only,
            "with_decoded_tree": with_tree,
            "after_tree_dropped": after_tree_dropped,
        },
        "world_imported": false,
    }))
}

/// `submitted` is when the committed snapshot was handed to the save worker;
/// the latency covers queueing, serialization and durable publication.
fn saved(receipt: SaveReceipt, submitted: Option<Instant>) {
    eprintln!(
        "{}",
        json!({"event": "save_committed", "generation": receipt.generation, "tick": receipt.tick,
        "save_latency_micros": submitted.map(|time| time.elapsed().as_micros())})
    );
}

/// Autosave bookkeeping shared by the loop and a compressed run's wait.
struct Autosave {
    submitted: Option<Instant>,
    last_commit: Option<Duration>,
    failures: u64,
}

impl Autosave {
    /// Reports one poll result; only a lost worker is fatal.
    fn observe(&mut self, polled: Result<Option<SaveReceipt>>, now: Duration) -> Result<bool> {
        match polled {
            Ok(Some(receipt)) => {
                self.last_commit = Some(now);
                saved(receipt, self.submitted.take());
                Ok(true)
            }
            Ok(None) => Ok(false),
            Err(error) => {
                if error.is::<SaveWorkerDisconnected>() {
                    return Err(error.context("stop simulation after terminal save-worker failure"));
                }
                self.submitted = None;
                self.failures += 1;
                eprintln!(
                    "{}",
                    json!({"event": "save_failed", "error": format!("{error:#}")})
                );
                Ok(true)
            }
        }
    }
}

fn run(options: ConfigOptions, max_ticks: Option<u64>, compressed: bool) -> Result<()> {
    let config = options.resolve()?;
    ensure!(max_ticks != Some(0), "max_ticks must be positive");
    ensure!(
        !compressed || max_ticks.is_some(),
        "a compressed run needs max_ticks"
    );
    // Load the release profile and bind before any world state opens: either
    // failure exits without creating, locking or loading the data directory.
    let bound = network::bind(&config.network)?;
    let pinned_build = bound
        .as_ref()
        .and_then(|bound| bound.release.as_ref())
        .map(|pin| pin.build_id);
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
    if compressed {
        app.insert_resource(TimeUpdateStrategy::FixedTimesteps(1));
    }
    let mut bound = bound;
    if let (Some(bound), Some(section)) = (&mut bound, &snapshot.preserved_prospect)
        && bound.lab.has_game_state_open()
    {
        // A loaded world's prospect and game mode record replicate in the
        // GameState open; a prospect that cannot supply them leaves the
        // lab's values alone.
        match network::apply_saved_game_state(&mut bound.lab, section) {
            Ok(()) => eprintln!("{}", json!({"event": "active_prospect_replicated"})),
            // Reader errors name fields and kinds, never saved values.
            Err(error) => eprintln!(
                "{}",
                json!({"event": "active_prospect_unavailable", "warning": "the preserved prospect supplies no replicated active prospect", "error": format!("{error:#}")})
            ),
        }
    }
    let network_address = bound
        .map(|bound| network::install(&mut app, bound.lab))
        .transpose()?;
    app.finish();
    app.cleanup();
    let mut writer = SaveWorker::spawn(store)?;
    let stopping = Arc::new(AtomicBool::new(false));
    let signal = Arc::clone(&stopping);
    ctrlc::set_handler(move || signal.store(true, Ordering::SeqCst))
        .context("install SIGINT/SIGTERM handler")?;

    let clock = RunClock {
        compressed,
        start: Instant::now(),
        start_tick: snapshot.tick,
        tick_hz: config.tick_hz,
    };
    let mut last_save_request = Duration::ZERO;
    let mut last_status = Duration::ZERO;
    let mut autosave = Autosave {
        submitted: None,
        last_commit: None,
        failures: 0,
    };
    let mut clamped_frames = 0_u64;
    let mut previous_frame = Instant::now();
    let interval = config.tick_interval();
    eprintln!(
        "{}",
        json!({"event": "started", "tick": snapshot.tick, "tick_hz": config.tick_hz,
        "icarus_compatible": false, "gameplay_ready": false,
        "native_schema": snapshot.schema_version, "compressed": compressed,
        "network_enabled": network_address.is_some()})
    );
    if let Some(address) = network_address {
        // The server's own bound address, configured limits and pinned build;
        // never a client-supplied value.
        eprintln!(
            "{}",
            json!({"event": "network_listening", "experimental": true,
            "address": address.to_string(), "max_players": config.network.max_players,
            "session_timeout_seconds": config.network.session_timeout_seconds,
            "version_check": pinned_build.is_some(), "release_build_id": pinned_build,
            "icarus_compatible": false, "gameplay_ready": false})
        );
    }

    loop {
        let frame = Instant::now();
        if !compressed
            && frame.duration_since(previous_frame) > interval * config.max_catch_up_ticks
        {
            clamped_frames += 1;
        }
        previous_frame = frame;
        // Manual runner still executes Bevy's complete app schedules and clocks.
        app.update();
        let tick = app.world().resource::<SimulationClock>().tick;
        let now = clock.elapsed(tick);
        autosave.observe(writer.poll(), now)?;
        if stopping.load(Ordering::SeqCst) || target_tick.is_some_and(|target| tick >= target) {
            // Admission stops before any drain or final-save work.
            network::stop_admission(app.world_mut());
            break;
        }
        if now.saturating_sub(last_save_request) >= Duration::from_secs(config.autosave_seconds) {
            // A compressed run keeps every autosave by awaiting the one in
            // flight; a stop request leaves the wait for the bounded shutdown.
            while compressed && writer.is_busy() && !autosave.observe(writer.poll(), now)? {
                if stopping.load(Ordering::SeqCst) {
                    break;
                }
                thread::sleep(Duration::from_millis(1));
            }
            if stopping.load(Ordering::SeqCst) {
                network::stop_admission(app.world_mut());
                break;
            }
            if !writer.is_busy() {
                writer.submit(capture(app.world_mut())?)?;
                // The interval runs from submission, after capture.
                last_save_request = clock.elapsed(tick);
                autosave.submitted = Some(Instant::now());
            }
        }
        if now.saturating_sub(last_status) >= Duration::from_secs(config.status_interval_seconds) {
            let metrics = app.world().resource::<TickMetrics>();
            eprintln!(
                "{}",
                json!({"event": "status", "tick": tick,
                "gameplay_ready": false, "save_in_flight": writer.is_busy(),
                "save_failures": autosave.failures,
                "seconds_since_save": autosave.last_commit.map(|time| now.saturating_sub(time).as_secs()),
                "clamped_frames": (!compressed).then_some(clamped_frames),
                "tick_deadline_misses": metrics.deadline_misses,
                "last_tick_micros": metrics.last_tick_micros,
                "max_tick_micros": metrics.max_tick_micros,
                "tick_p50_micros": metrics.tick_micros.quantile_micros(50),
                "tick_p99_micros": metrics.tick_micros.quantile_micros(99),
                "sessions": network::session_counts(app.world())})
            );
            last_status = now;
        }
        if !compressed {
            thread::sleep(interval.saturating_sub(frame.elapsed()));
        }
    }

    // Idempotent: admission is already stopped; close every session and drop
    // the socket before the bounded drain and final save.
    network::stop_admission(app.world_mut());
    let sessions = network::teardown(app.world_mut());
    let deadline = Instant::now() + Duration::from_secs(config.shutdown_timeout_seconds);
    // A failed in-flight autosave is reported as in the loop; the final save still runs.
    match writer.settle_before(deadline)? {
        Some(Ok(receipt)) => saved(receipt, autosave.submitted.take()),
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
    let final_submitted = Instant::now();
    writer.submit_before(capture(app.world_mut())?, deadline)?;
    if let Some(receipt) = writer.finish_before(deadline)? {
        saved(receipt, Some(final_submitted));
    }
    eprintln!(
        "{}",
        json!({"event": "stopped", "tick": app.world().resource::<SimulationClock>().tick,
        "gameplay_ready": false, "sessions": sessions})
    );
    Ok(())
}

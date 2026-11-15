//! Source-authored schema-14 process-recovery workload; no network or original inputs.
use anyhow::{Context, Result, ensure};
use bevy::prelude::{App, FixedUpdate};
use clap::{Parser, Subcommand};
use icarus_bevy::{
    authored_world_bootstrap::{AuthoredClockConfig, AuthoredWorldBootstrap},
    config::{NetworkConfig, PlayerPersistenceConfig, ServerConfig},
    experimental::{
        pawn_open::CharacterVitals,
        release::{ReleasePin, network_checksum},
    },
    native_player_runtime::{ConsumptionConfig, PlayerRegistry},
    native_players::{PlayerCheckpoint, PlayerContext, PlayerInputs, PlayerKey, PlayerSnapshot},
    persistence::{SaveReceipt, SaveWorker, SnapshotStore},
    simulation::{SNAPSHOT_SCHEMA_PLAYERS, SimulationClock, WorldSnapshot, build_app, capture},
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

const HZ: u32 = 7;
const MAX_TICK: u64 = 100_000;
const RATES: [i32; 3] = [2, 3, 5];
const PROTOCOL: &str = "authored_player_recovery_v1";

#[derive(Parser)]
struct Options {
    #[arg(long)]
    data_dir: PathBuf,
    #[arg(long)]
    run_id: String,
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    Init,
    Run {
        #[arg(long, default_value_t = 12)]
        duration_seconds: u64,
        #[arg(long, default_value_t = 1000)]
        save_millis: u64,
    },
    Verify {
        #[arg(long)]
        receipts: PathBuf,
    },
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Acknowledgement {
    generation: u64,
    tick: u64,
    digest: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Generation {
    number: u64,
    sha256: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    schema_version: u32,
    current: Generation,
    previous: Option<Generation>,
}

fn digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
fn fingerprint(value: &impl Serialize) -> Result<[u8; 32]> {
    Ok(Sha256::digest(serde_json::to_vec(value)?).into())
}
fn key(account: u64) -> Result<PlayerKey> {
    Ok(PlayerKey::new(account, 0)?)
}
fn declaration() -> PlayerPersistenceConfig {
    PlayerPersistenceConfig {
        prospect_id: "authored-recovery".into(),
        map_url: "/Authored/Recovery".into(),
        prospect_row: "AuthoredProspect".into(),
        difficulty: 1,
    }
}
fn clock() -> AuthoredClockConfig {
    AuthoredClockConfig {
        initial_minute: 480,
        seconds_per_day: 11,
        tick_hz: HZ,
    }
}
fn rates() -> ConsumptionConfig {
    ConsumptionConfig {
        oxygen: RATES[0],
        water: RATES[1],
        food: RATES[2],
    }
}
fn authored_world() -> AuthoredWorldBootstrap {
    AuthoredWorldBootstrap {
        seed: 42,
        seed_initialised: true,
        flod_tile_count: 0,
        clock: Some(clock()),
    }
}
fn release() -> ReleasePin {
    ReleasePin {
        app_id: 2089300,
        build_id: 1,
        project_version: "authored-recovery".into(),
        network_checksum: network_checksum("authored-recovery"),
    }
}
fn initial() -> PlayerCheckpoint {
    PlayerCheckpoint {
        location_cm: [0., 200.25, -300.5],
        vitals: CharacterVitals {
            health: 100,
            max_health: 100,
            stamina: 90,
            max_stamina: 100,
            oxygen: 1_000_000,
            max_oxygen: 1_000_000,
            water: 1_000_000,
            max_water: 1_000_000,
            food: 1_000_000,
            max_food: 1_000_000,
            biome_row: 0,
            growth_row: 0,
        },
    }
}
fn context() -> Result<PlayerContext> {
    context_for_rules(rates(), HZ)
}
fn context_for_rules(consumption: ConsumptionConfig, tick_hz: u32) -> Result<PlayerContext> {
    let v = initial().vitals;
    let values = [
        v.health,
        v.max_health,
        v.stamina,
        v.max_stamina,
        v.oxygen,
        v.max_oxygen,
        v.water,
        v.max_water,
        v.food,
        v.max_food,
    ];
    Ok(PlayerContext::new(
        "authored-recovery".encode_utf16().collect(),
        "/Authored/Recovery".into(),
        PlayerInputs {
            authored_gameplay: fingerprint(&("authored-recovery-gameplay-v1", values))?,
            spawn: fingerprint(&initial().location_cm)?,
            authored_world: fingerprint(&authored_world())?,
            // Same rule-binding contract as F-1183; fixture-owned inputs, no package.
            declaration: fingerprint(&(
                "authored_player_rules_v1",
                declaration(),
                consumption,
                tick_hz,
            ))?,
            initial_values: fingerprint(&("AuthoredGrowth", values, v.biome_row))?,
        },
        vec!["AuthoredGrowth".into()],
    )?)
}
fn config() -> ServerConfig {
    ServerConfig {
        tick_hz: HZ,
        network: NetworkConfig {
            // Offline build_app validates these labels but never opens them. No network
            // installation is called; the fixture catalog is context().growth_rows.
            release_profile: Some("authored-release-label".into()),
            data_pak: Some("authored-catalog-label".into()),
            authored_gameplay: Some("authored-gameplay-label".into()),
            player_persistence: Some(declaration()),
            authored_world: Some(authored_world()),
            player_consumption: Some(rates()),
            ..Default::default()
        },
        ..Default::default()
    }
}
fn seed() -> Result<WorldSnapshot> {
    let context = context()?;
    let mut registry = PlayerRegistry::new(
        PlayerSnapshot::fresh(context.clone(), &release())?,
        &context,
        2,
    )?;
    for account in 1..=2 {
        registry.reserve(key(account)?, account, initial())?;
        registry.activate(account)?;
    }
    Ok(WorldSnapshot {
        schema_version: SNAPSHOT_SCHEMA_PLAYERS,
        native_players: Some(registry.snapshot()),
        ..Default::default()
    })
}
fn attach(app: &mut App) -> Result<()> {
    let mut players = app.world_mut().resource_mut::<PlayerRegistry>();
    ensure!(
        !players.reserve(key(1)?, 1, initial())?.is_new,
        "active record missing"
    );
    players.activate(1)?;
    Ok(())
}
fn open_app(snapshot: &WorldSnapshot) -> Result<App> {
    let mut opened = snapshot.clone();
    opened
        .native_players
        .as_mut()
        .context("player state missing")?
        .open(&context()?, &release())?;
    let mut app = build_app(&config(), &opened)?;
    app.finish();
    app.cleanup();
    // No attachment and no update at reopen: no downtime debit or phase reset.
    ensure!(
        capture(app.world_mut())? == *snapshot,
        "reopen changed committed state"
    );
    attach(&mut app)?;
    Ok(app)
}
fn step(app: &mut App) -> Result<()> {
    let tick = app.world().resource::<SimulationClock>().tick;
    ensure!(tick < MAX_TICK, "fixture tick bound exceeded");
    let mut players = app.world_mut().resource_mut::<PlayerRegistry>();
    let projection = players.projection(1).context("active player missing")?;
    let mut checkpoint = projection.checkpoint;
    checkpoint.location_cm[0] = (tick + 1) as f64 * 0.25;
    players.edit(key(1)?, projection.revision, checkpoint)?;
    app.world_mut().run_schedule(FixedUpdate);
    ensure!(
        app.world().resource::<SimulationClock>().tick == tick + 1,
        "fixed step failed"
    );
    Ok(())
}
/// Independent integer oracle: no runtime replay and no JSON-number comparison.
fn expected(tick: u64) -> Result<WorldSnapshot> {
    ensure!(tick <= MAX_TICK, "oracle tick bound exceeded");
    let mut state = seed()?;
    let context = context()?;
    let mut registry =
        PlayerRegistry::new(state.native_players.take().context("seed")?, &context, 2)?;
    let original = registry
        .snapshot()
        .projection(key(1)?)
        .context("seed player")?;
    let mut checkpoint = original.checkpoint;
    checkpoint.location_cm[0] = tick as f64 * 0.25;
    checkpoint.vitals.oxygen -= (tick * RATES[0] as u64 / u64::from(HZ)) as i32;
    checkpoint.vitals.water -= (tick * RATES[1] as u64 / u64::from(HZ)) as i32;
    checkpoint.vitals.food -= (tick * RATES[2] as u64 / u64::from(HZ)) as i32;
    // Public CAS edits account for location changes plus one revision per changed
    // survival batch (three nonzero debits in a batch still use one revision).
    let batches = (0..tick)
        .filter(|t| {
            RATES
                .iter()
                .any(|r| (t + 1) * *r as u64 / u64::from(HZ) != t * *r as u64 / u64::from(HZ))
        })
        .count() as u64;
    let revisions = tick + batches;
    for revision in 1..=revisions {
        registry.edit(key(1)?, revision, checkpoint)?;
    }
    state.native_players = Some(registry.snapshot());
    state.tick = tick;
    Ok(state)
}
fn check(snapshot: &WorldSnapshot) -> Result<String> {
    ensure!(
        *snapshot == expected(snapshot.tick)?,
        "typed authored snapshot differs at tick {}",
        snapshot.tick
    );
    Ok(digest(&serde_json::to_vec(snapshot)?))
}
fn load(store: &SnapshotStore, directory: &Path) -> Result<(u64, WorldSnapshot)> {
    let loaded = store.load()?.context("committed state missing")?;
    let manifest: Manifest = serde_json::from_slice(&fs::read(directory.join("manifest.json"))?)?;
    ensure!(manifest.schema_version == 1, "manifest schema");
    let selected = if loaded.recovered_previous {
        manifest.previous.context("previous generation missing")?
    } else {
        manifest.current
    };
    ensure!(
        selected.number > 0 && selected.sha256.len() == 64,
        "invalid selected generation"
    );
    check(&loaded.snapshot)?;
    Ok((selected.number, loaded.snapshot))
}
fn verify(directory: &Path, acknowledgements: &[Acknowledgement]) -> Result<serde_json::Value> {
    ensure!(
        !acknowledgements.is_empty(),
        "acknowledgement history missing"
    );
    let store = SnapshotStore::open_existing(directory)?;
    let (generation, snapshot) = load(&store, directory)?;
    let hash = check(&snapshot)?;
    let mut previous = (0, 0);
    for ack in acknowledgements {
        ensure!(
            ack.generation > previous.0 && ack.tick >= previous.1,
            "acknowledgements out of order"
        );
        ensure!(
            ack.generation <= generation && ack.tick <= snapshot.tick,
            "acknowledged state lost"
        );
        ensure!(
            digest(&serde_json::to_vec(&expected(ack.tick)?)?) == ack.digest,
            "acknowledged digest differs"
        );
        if ack.generation == generation {
            ensure!(
                ack.tick == snapshot.tick && ack.digest == hash,
                "selected receipt differs"
            );
        }
        previous = (ack.generation, ack.tick);
    }
    let mut app = open_app(&snapshot)?;
    // Cross at least one fractional boundary after reopening; this never saves.
    for _ in 0..=HZ {
        step(&mut app)?;
        check(&capture(app.world_mut())?)?;
    }
    let resumed = capture(app.world_mut())?;
    check(&resumed)?;
    Ok(
        json!({"generation":generation, "tick":snapshot.tick, "digest":hash,
        "resumed_tick":resumed.tick, "acknowledgements":acknowledgements.len(), "schema":14,
        "day_sample_bits":clock().prepare()?.sample(snapshot.tick).to_bits()}),
    )
}
fn emit(options: &Options, event: &str, mut fields: serde_json::Value) -> Result<()> {
    fields["event"] = json!(event);
    fields["protocol"] = json!(PROTOCOL);
    fields["run_id"] = json!(options.run_id);
    fields["pid"] = json!(std::process::id());
    let mut out = io::stdout().lock();
    serde_json::to_writer(&mut out, &fields)?;
    writeln!(out)?;
    out.flush()?;
    Ok(())
}
fn acknowledge(
    options: &Options,
    receipt: SaveReceipt,
    pending: &mut Option<Acknowledgement>,
    generation: &mut u64,
) -> Result<()> {
    let ack = pending.take().context("receipt without submission")?;
    ensure!(
        receipt.generation == ack.generation
            && receipt.tick == ack.tick
            && receipt.generation == *generation + 1,
        "writer receipt mismatch"
    );
    *generation = receipt.generation;
    emit(options, "save_committed", serde_json::to_value(ack)?)
}
fn submit(
    app: &mut App,
    worker: &mut SaveWorker,
    generation: u64,
    pending: &mut Option<Acknowledgement>,
) -> Result<()> {
    ensure!(pending.is_none(), "pending receipt overwritten");
    let snapshot = capture(app.world_mut())?;
    let ack = Acknowledgement {
        generation: generation + 1,
        tick: snapshot.tick,
        digest: check(&snapshot)?,
    };
    worker.submit(snapshot)?;
    *pending = Some(ack);
    Ok(())
}
fn run(options: &Options, duration: u64, save_millis: u64, stopping: &AtomicBool) -> Result<()> {
    ensure!(
        (3..=300).contains(&duration) && (100..=5000).contains(&save_millis),
        "invalid workload bounds"
    );
    let store = SnapshotStore::open_existing(&options.data_dir)?;
    let (mut generation, snapshot) = load(&store, &options.data_dir)?;
    let mut app = open_app(&snapshot)?;
    emit(
        options,
        "started",
        json!({"generation":generation,"tick":snapshot.tick,"digest":check(&snapshot)?}),
    )?;
    let mut worker = SaveWorker::spawn(store)?;
    let start = Instant::now();
    let mut next_tick = start + config().tick_interval();
    let mut next_save = start + Duration::from_millis(save_millis);
    let mut pending = None;
    while !stopping.load(Ordering::SeqCst) && start.elapsed() < Duration::from_secs(duration) {
        if let Some(receipt) = worker.poll()? {
            acknowledge(options, receipt, &mut pending, &mut generation)?;
        }
        if Instant::now() >= next_tick {
            step(&mut app)?;
            next_tick = Instant::now() + config().tick_interval();
        }
        if Instant::now() >= next_save && !worker.is_busy() {
            submit(&mut app, &mut worker, generation, &mut pending)?;
            next_save = Instant::now() + Duration::from_millis(save_millis);
        }
        thread::sleep(Duration::from_millis(2));
    }
    ensure!(
        stopping.load(Ordering::SeqCst),
        "workload deadline reached before controller stop"
    );
    emit(options, "stopping", json!({}))?;
    let deadline = Instant::now() + Duration::from_secs(3);
    if let Some(receipt) = worker.finish_before(deadline)? {
        acknowledge(options, receipt, &mut pending, &mut generation)?;
    }
    submit(&mut app, &mut worker, generation, &mut pending)?;
    let receipt = worker
        .finish_before(deadline)?
        .context("final receipt missing")?;
    let tick = receipt.tick;
    acknowledge(options, receipt, &mut pending, &mut generation)?;
    emit(
        options,
        "completed",
        json!({"generation":generation,"tick":tick}),
    )
}
fn main() -> Result<()> {
    let options = Options::parse();
    ensure!(
        !options.run_id.is_empty() && options.run_id.len() <= 128,
        "invalid run identity"
    );
    let stopping = Arc::new(AtomicBool::new(false));
    let signal = stopping.clone();
    ctrlc::set_handler(move || signal.store(true, Ordering::SeqCst))?;
    match &options.command {
        Command::Init => {
            let mut store = SnapshotStore::open(&options.data_dir)?;
            let snapshot = seed()?;
            let receipt = store.save_new(&snapshot)?;
            emit(
                &options,
                "initialized",
                json!({"generation":receipt.generation,"tick":receipt.tick,"digest":check(&snapshot)?}),
            )
        }
        Command::Run {
            duration_seconds,
            save_millis,
        } => run(&options, *duration_seconds, *save_millis, &stopping),
        Command::Verify { receipts } => {
            ensure!(
                fs::metadata(receipts)?.len() <= 1024 * 1024,
                "receipt input exceeds bound"
            );
            let acknowledgements: Vec<Acknowledgement> =
                serde_json::from_slice(&fs::read(receipts)?)?;
            emit(
                &options,
                "verified",
                verify(&options.data_dir, &acknowledgements)?,
            )
        }
    }
}

#[cfg(test)]
#[path = "authored_player_recovery/tests.rs"]
mod tests;

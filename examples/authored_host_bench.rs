//! Whole `App::update()` wall time of the opt-in authored schema-4 replication
//! bridge (F-938) against a matched networking-only baseline; not an ICARUS
//! benchmark. Catalog, release profile and bindings are independently authored
//! in the shapes of the `authored_host_idle_tick_cost` test, served on loopback
//! with no peers or with `--peers` joined authored loopback peers that
//! acknowledge every packet outside timing. Both modes live in one process and
//! alternate every frame, in swapped order each frame. Only aggregates are printed.
#[path = "../tests/common/loopback_client.rs"]
mod loopback;

use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail, ensure};
use bevy::{app::App, time::TimeUpdateStrategy};
use clap::Parser;
use icarus_bevy::{
    config::{NetworkConfig, ServerConfig},
    experimental::{archive::ByteOrder, peer_lab::PeerLab, session_open::authored_movement},
    native_replication::{AuthoredReplication, MAX_BINDINGS, NativeReplicationCounts},
    native_runtime::NativeActorIdentity,
    native_world::{CompiledNativeCatalog, NativeCatalogDefinition, prepare_authored_world},
    network,
    replication::{NetActor, NetActorFlags, NetSchedule, NetViewer, ReplicationFrame},
    session_replication::{NetArchetype, SessionReplication},
    simulation::{DomainId, Position, SimulationClock, WorldSnapshot, build_app, capture},
};
use loopback::{Client, drain};
use serde_json::{Value, json};

#[derive(Parser)]
struct Options {
    /// Authored actor counts (one binding per placement), each 1..=1024.
    #[arg(long, value_delimiter = ',', default_value = "1,64,1024")]
    actors: Vec<usize>,
    /// Timed frames per mode and count.
    #[arg(long, default_value_t = 200)]
    frames: usize,
    /// Untimed one-tick frames per mode before timing.
    #[arg(long, default_value_t = 20)]
    warmup_frames: usize,
    /// Joined authored loopback peers per mode, 0..=8 (the declared connection cap).
    #[arg(long, default_value_t = 0)]
    peers: usize,
    /// Untimed update bound for every peer to open every bound actor.
    #[arg(long, default_value_t = 4096)]
    open_frames: usize,
    /// Cooperative total wall-time limit, checked before every update.
    #[arg(long, default_value_t = 60)]
    budget_seconds: u64,
    /// Also write the aggregate summary as JSON to this new file.
    #[arg(long)]
    json: Option<PathBuf>,
}

// An unknown, independently authored release profile with no shipped content
// identities (the replication tests' profile).
const RELEASE: &str = r#"
[release]
app_id = 2089300
build_id = 1
project_version = "1.0.0"
network_checksum = 1447163286
[tags]
data_table_tag_count = 1
ini_tag_count = 0
unique_node_count = 1
total_bits = 2
configured_first_segment = 0
effective_first_segment = 0
container_bits = 1
net_index_hash = 0
[player_controller]
archetype = [
    { path = "Default__AuthoredController_C", checksum = 7 },
    { path = "/Lab/Controller", checksum = 0 },
]
components = [{ name = "A", checksum = 1 }, { name = "B", checksum = 2 }, { name = "C", checksum = 3 }]
"#;

/// The declared first actor channel.
const FIRST_CHANNEL: u32 = 6;

struct Budget {
    started: Instant,
    limit: Duration,
}

impl Budget {
    fn check(&self) -> Result<()> {
        ensure!(
            self.started.elapsed() < self.limit,
            "cooperative budget of {} s exhausted; no summary produced",
            self.limit.as_secs()
        );
        Ok(())
    }
}

struct Input {
    count: usize,
    /// Bound actor IDs, in placement order.
    ids: Vec<u64>,
    snapshot: WorldSnapshot,
    declaration: Value,
}

fn pose_json(translation: [f32; 3]) -> Value {
    json!({"translation": translation, "rotation": [0.0, 0.0, 0.0, 1.0], "scale": [1.0, 1.0, 1.0]})
}

fn binding_json(placement: &str) -> Value {
    json!({
        "placement": placement, "archetype": [
            {"path": "Default__IndependentBox_C", "checksum": 12345, "no_load": false},
            {"path": "/Lab/IndependentBox", "checksum": 0, "no_load": false}
        ],
        "parameters": {"net_cull_distance_squared": 1e14, "net_update_frequency": 1000.0,
            "min_net_update_frequency": 1000.0, "net_priority": 1.0},
        "replicate_movement": true, "flags": {"hidden": true, "can_be_damaged": true}
    })
}

fn input(count: usize) -> Result<Input> {
    let mut placements = Vec::with_capacity(count);
    let mut bindings = Vec::with_capacity(count);
    for index in 0..count {
        let placement = format!("bench_{index:04}");
        // At most a 32 x 32 grid: 0..=62 m, z = 2 m, inside the tile bounds.
        let x = (index % 32) as f32 * 2.0;
        let y = (index / 32) as f32 * 2.0;
        // The declared frame (axes [2,-1,3], 10 units/m) must quantize.
        authored_movement([y * 10.0, -x * 10.0, 20.0], [0.0; 3], [0.0; 3])?;
        placements.push(json!({
            "key": placement, "class": "spatial", "tile": "tile",
            "transform": pose_json([x, y, 2.0])
        }));
        bindings.push(binding_json(&placement));
    }
    let definition: NativeCatalogDefinition = serde_json::from_value(json!({
        "native_format": 1, "frame": "right_handed_z_up_meters",
        "classes": [{"key": "spatial", "kind": "spatial_object_v1", "variables": [],
            "local_bounds": {"min": [-0.5, -0.5, -0.5], "max": [0.5, 0.5, 0.5]}}],
        "maps": [{"key": "authored", "tiles": [{"key": "tile",
            "bounds": {"min": [-128.0, -128.0, -128.0], "max": [128.0, 128.0, 128.0]}}],
            "placements": placements}]
    }))?;
    let prepared = prepare_authored_world(
        Arc::new(CompiledNativeCatalog::compile(definition)?),
        "authored",
    )?;
    let snapshot = prepared.initial_snapshot();
    snapshot.validate()?;
    let world = snapshot
        .native_world
        .as_ref()
        .context("authored snapshot lacks a native world")?;
    ensure!(snapshot.schema_version == 4, "expected a schema-4 snapshot");
    ensure!(snapshot.tick == 0 && snapshot.preserved_prospect.is_none());
    ensure!(world.actors.len() == count, "authored actor count differs");
    let ids = world.actors.iter().map(|actor| actor.id.0).collect();
    let declaration = json!({
        "format": 1, "catalog_id": world.catalog_id,
        "map": "authored", "frame": {"axes": [2, -1, 3], "units": 10.0},
        "first_channel": FIRST_CHANNEL,
        "schedule": {
            "connection_budget": 32, "max_consider_list": 1024, "max_connections": 8,
            "max_viewers_per_connection": 1, "net_server_max_tick_rate": 30,
            "max_connections_to_tick_per_server_frame": 0,
            "relevant_timeout_seconds": 5.0, "spawn_priority_seconds": 1.0,
            "use_adaptive_net_update_frequency": false, "seed": 19
        },
        "bindings": bindings
    });
    AuthoredReplication::parse(&serde_json::to_string(&declaration)?)?;
    Ok(Input {
        count,
        ids,
        snapshot,
        declaration,
    })
}

/// Matched configurations: identical except for `authored_replication`.
fn configurations(dir: &Path) -> Result<(ServerConfig, ServerConfig)> {
    let host = ServerConfig {
        data_dir: dir.join("state"),
        network: NetworkConfig {
            enabled: true,
            port: 0,
            release_profile: Some(dir.join("release.toml")),
            authored_replication: Some(dir.join("replication.json")),
            ..Default::default()
        },
        ..Default::default()
    };
    let mut baseline = host.clone();
    baseline.network.authored_replication = None;
    host.validate()?;
    baseline.validate()?;
    ensure!(host.network.data_pak.is_none(), "unexpected data pak");
    Ok((baseline, host))
}

fn count_with<T: bevy::ecs::component::Component>(app: &App) -> usize {
    app.world()
        .iter_entities()
        .filter(|entity| entity.contains::<T>())
        .count()
}

/// Cheap invariants, always outside timing. `peers` must be joined and kept;
/// with none, the loopback socket must have seen no traffic at all.
fn verify_idle(app: &App, input: &Input, host: bool, peers: usize) -> Result<()> {
    let lab = app.world().resource::<PeerLab>();
    ensure!(lab.is_admitting(), "lab stopped admitting");
    let stats = lab.stats();
    if peers == 0 {
        ensure!(
            lab.peer_count() == 0
                && lab.peak_peer_count() == 0
                && stats.received == 0
                && stats.peers_created == 0
                && lab.joined_connections().is_empty(),
            "unexpected peer traffic on the loopback socket"
        );
    } else {
        ensure!(
            lab.peer_count() == peers
                && lab.peak_peer_count() == peers
                && stats.peers_created == peers as u64
                && lab.joined_connections().len() == peers,
            "joined peer set changed"
        );
        ensure!(
            stats.peers_replaced == 0
                && stats.peers_rejected == 0
                && stats.peers_expired == 0
                && stats.peers_failed == 0
                && stats.peers_closed == 0
                && stats.data_rejected == 0
                && stats.data_malformed == 0
                && stats.ack_errors == 0
                && stats.reply_errors == 0
                && stats.actor_channel_failures == 0,
            "peer session errors"
        );
    }
    ensure!(stats.io_errors == 0, "lab socket I/O errors");
    ensure!(count_with::<NativeActorIdentity>(app) == input.count);
    ensure!(count_with::<DomainId>(app) == input.count);
    ensure!(
        count_with::<NetViewer>(app) == if host { peers } else { 0 },
        "unexpected viewers"
    );
    let proxies: Vec<_> = app
        .world()
        .iter_entities()
        .filter(|entity| entity.contains::<NetActor>())
        .collect();
    ensure!(
        proxies.len() == if host { input.count } else { 0 },
        "proxy count {} in {} mode",
        proxies.len(),
        if host { "host" } else { "baseline" }
    );
    ensure!(
        proxies.iter().all(|entity| {
            !entity.contains::<DomainId>()
                && !entity.contains::<NativeActorIdentity>()
                && entity.contains::<NetArchetype>()
                && entity.contains::<NetSchedule>()
                && entity.contains::<Position>()
                && entity.contains::<NetActorFlags>()
        }),
        "proxy components differ"
    );
    ensure!(app.world().contains_resource::<SessionReplication>() == host);
    if host {
        let counts = app
            .world()
            .get_resource::<NativeReplicationCounts>()
            .context("host lacks native replication counts")?;
        ensure!(
            counts.active
                && counts.activated == input.count
                && counts.rejected == 0
                && counts.binding_errors == 0,
            "native replication counts differ"
        );
        let frame = app.world().resource::<ReplicationFrame>();
        ensure!(
            peers > 0 || frame.decisions.replication_frame == 0,
            "scheduler advanced without peers"
        );
    } else {
        ensure!(!app.world().contains_resource::<NativeReplicationCounts>());
        ensure!(!app.world().contains_resource::<ReplicationFrame>());
    }
    Ok(())
}

/// Persistent authored state is unchanged; only the clock may advance.
fn verify_unchanged(app: &mut App, input: &Input) -> Result<()> {
    let mut snapshot = capture(app.world_mut())?;
    snapshot.tick = input.snapshot.tick;
    ensure!(
        snapshot == input.snapshot,
        "authored persistent domain state changed"
    );
    Ok(())
}

struct Mode {
    app: App,
    host: bool,
    peers: Vec<Client>,
}

impl Mode {
    /// Untimed: each peer acknowledges everything received and sends one
    /// packet, which the next update polls.
    fn acknowledge(&mut self) {
        for peer in &mut self.peers {
            let datagrams = drain(&peer.socket);
            peer.accept(&datagrams);
            peer.send(Vec::new());
        }
    }

    /// Bound actors open on each joined connection.
    fn open_counts(&self, input: &Input) -> Vec<usize> {
        let lab = self.app.world().resource::<PeerLab>();
        lab.joined_connections()
            .iter()
            .map(|connection| {
                input
                    .ids
                    .iter()
                    .filter(|id| lab.is_actor_open(*connection, **id))
                    .count()
            })
            .collect()
    }

    /// Every peer has as many bound actors open as its actor channels allow.
    fn saturated(&self, input: &Input) -> bool {
        let capacity = actor_channels(&self.app).min(input.count);
        let counts = self.open_counts(input);
        counts.len() == self.peers.len() && counts.iter().all(|count| *count == capacity)
    }

    fn verify(&self, input: &Input) -> Result<()> {
        verify_idle(&self.app, input, self.host, self.peers.len())?;
        if self.host && !self.peers.is_empty() {
            ensure!(self.saturated(input), "a peer's open bound actors changed");
        }
        Ok(())
    }
}

/// Channels from the declared first actor channel to the lab's channel limit.
fn actor_channels(app: &App) -> usize {
    let limit = app
        .world()
        .resource::<PeerLab>()
        .config()
        .packet
        .packet
        .limits
        .max_channels;
    limit.saturating_sub(FIRST_CHANNEL) as usize
}

/// Production `run` order: build, bind, install, finish, cleanup; then join
/// `peers` authored loopback peers.
fn stage(config: &ServerConfig, input: &Input, host: bool, peers: usize) -> Result<Mode> {
    let mut app = build_app(config, &input.snapshot)?;
    let bound = network::bind(&config.network)?.context("networking disabled")?;
    let address = network::install_configured(&mut app, bound, &config.network)?;
    ensure!(address.ip().is_loopback(), "lab is not on loopback");
    app.insert_resource(TimeUpdateStrategy::FixedTimesteps(1));
    app.finish();
    app.cleanup();
    verify_idle(&app, input, host, 0)?;
    // Bevy's first update initializes time with zero delta; not a warmup.
    let before = app.world().resource::<SimulationClock>().tick;
    app.update();
    ensure!(app.world().resource::<SimulationClock>().tick == before);
    verify_idle(&app, input, host, 0)?;
    let mut clients = Vec::with_capacity(peers);
    for index in 0..peers {
        let mut client =
            Client::connect_indexed(&mut app, address, ByteOrder::Little, u8::try_from(index)?);
        let opened = client.join(&mut app);
        client.accept(&opened);
        clients.push(client);
    }
    verify_idle(&app, input, host, peers)?;
    Ok(Mode {
        app,
        host,
        peers: clients,
    })
}

/// One verified one-tick update; returns its wall time in nanoseconds. Peer
/// acknowledgements are sent before, and outside, the timed update.
fn step(mode: &mut Mode, budget: &Budget) -> Result<u64> {
    budget.check()?;
    mode.acknowledge();
    let app = &mut mode.app;
    let before = app.world().resource::<SimulationClock>().tick;
    let started = Instant::now();
    app.update(); // Whole App: every schedule, fixed work and lab poll.
    let elapsed = started.elapsed();
    ensure!(
        app.world().resource::<SimulationClock>().tick == before + 1,
        "update did not advance exactly one tick"
    );
    Ok(u64::try_from(elapsed.as_nanos())?)
}

struct Stats {
    samples: usize,
    min: f64,
    median: f64,
    p95: f64,
    max: f64,
}

fn stats(mut nanos: Vec<u64>) -> Stats {
    nanos.sort_unstable();
    let n = nanos.len();
    let micros = |value: f64| (value / 100.0).round() / 10.0;
    let median = if n % 2 == 1 {
        nanos[n / 2] as f64
    } else {
        (nanos[n / 2 - 1] as f64 + nanos[n / 2] as f64) / 2.0
    };
    Stats {
        samples: n,
        min: micros(nanos[0] as f64),
        median: micros(median),
        p95: micros(nanos[(95 * n).div_ceil(100) - 1] as f64),
        max: micros(nanos[n - 1] as f64),
    }
}

/// Both modes' timings and the bound actors open on every host peer.
fn measure(options: &Options, input: &Input, budget: &Budget) -> Result<([Stats; 2], usize)> {
    let dir = tempfile::tempdir()?;
    fs::write(dir.path().join("release.toml"), RELEASE)?;
    fs::write(
        dir.path().join("replication.json"),
        serde_json::to_vec(&input.declaration)?,
    )?;
    let (baseline_config, host_config) = configurations(dir.path())?;
    budget.check()?;
    let mut modes = [
        stage(&baseline_config, input, false, options.peers)?,
        stage(&host_config, input, true, options.peers)?,
    ];
    // Both modes take the same untimed updates until every peer has opened as
    // many bound actors as its actor channels allow, so timing starts from a
    // fully opened steady state.
    let mut opening = 0;
    while options.peers > 0 && !modes[1].saturated(input) {
        ensure!(
            opening < options.open_frames,
            "peers did not fill their actor channels within {} updates",
            options.open_frames
        );
        for mode in &mut modes {
            step(mode, budget)?;
        }
        opening += 1;
    }
    let started = modes
        .each_ref()
        .map(|mode| mode.app.world().resource::<SimulationClock>().tick);
    let mut nanos = [
        Vec::with_capacity(options.frames),
        Vec::with_capacity(options.frames),
    ];
    for frame in 0..options.warmup_frames + options.frames {
        // Alternate which mode runs first to cancel order effects.
        let order = if frame % 2 == 0 { [0, 1] } else { [1, 0] };
        for index in order {
            let elapsed = step(&mut modes[index], budget)?;
            if frame >= options.warmup_frames {
                nanos[index].push(elapsed);
            }
        }
    }
    let ticks = (options.warmup_frames + options.frames) as u64;
    for (mode, started) in modes.iter_mut().zip(started) {
        ensure!(mode.app.world().resource::<SimulationClock>().tick == started + ticks);
        mode.verify(input)?;
        verify_unchanged(&mut mode.app, input)?;
    }
    let open = modes[1].open_counts(input).into_iter().min().unwrap_or(0);
    drop(modes); // Sockets close before the temporary directory is removed.
    dir.close()?;
    let [baseline, host] = nanos;
    Ok(([stats(baseline), stats(host)], open))
}

fn main() -> Result<()> {
    let options = Options::parse();
    ensure!(
        (1..=100_000).contains(&options.frames),
        "frames must be 1..100000"
    );
    ensure!(options.warmup_frames <= 10_000, "warmup must be 0..10000");
    ensure!(options.peers <= 8, "peers must be 0..8");
    ensure!(
        (1..=100_000).contains(&options.open_frames),
        "open frames must be 1..100000"
    );
    ensure!(
        (1..=3_600).contains(&options.budget_seconds),
        "budget must be 1..3600 seconds"
    );
    ensure!(
        !options.actors.is_empty()
            && options.actors.len() <= 16
            && options
                .actors
                .iter()
                .all(|count| (1..=MAX_BINDINGS).contains(count)),
        "actor counts must be 1..{MAX_BINDINGS}, at most 16 of them"
    );
    if let Some(path) = &options.json
        && path.exists()
    {
        bail!("refusing to replace existing {}", path.display());
    }
    let profile = if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    };
    let budget = Budget {
        started: Instant::now(),
        limit: Duration::from_secs(options.budget_seconds),
    };
    let workload = if options.peers == 0 {
        "idle/no-peer".to_owned()
    } else {
        format!("idle/{}-joined-peer", options.peers)
    };
    println!(
        "authored host bench ({profile}; not an ICARUS benchmark): {workload} whole App::update, \
         {} frames after {} warmups, modes alternate per frame",
        options.frames, options.warmup_frames
    );
    println!(
        "{:>6} {:>8} {:>7} {:>9} {:>9} {:>9} {:>9}",
        "actors", "mode", "samples", "min_us", "median_us", "p95_us", "max_us"
    );
    let mut runs = Vec::with_capacity(options.actors.len());
    for &count in &options.actors {
        budget.check()?;
        let input = input(count)?;
        let ([baseline, host], open) = measure(&options, &input, &budget)?;
        let delta = ((host.median - baseline.median) * 10.0).round() / 10.0;
        let mut modes = Vec::with_capacity(2);
        for (mode, stats) in [("baseline", &baseline), ("host", &host)] {
            println!(
                "{count:>6} {mode:>8} {:>7} {:>9.1} {:>9.1} {:>9.1} {:>9.1}",
                stats.samples, stats.min, stats.median, stats.p95, stats.max
            );
            modes.push(json!({"mode": mode, "samples": stats.samples,
                "min_micros": stats.min, "median_micros": stats.median,
                "p95_micros": stats.p95, "max_micros": stats.max}));
        }
        println!("{count:>6} host-baseline median delta: {delta:+.1} us; {open} open per peer");
        runs.push(json!({"actors": count, "bound_actors": count, "peers": options.peers, "open_actors_per_peer": open,
            "modes": modes, "host_minus_baseline_median_micros": delta}));
    }
    if let Some(path) = &options.json {
        let summary = json!({"event": "authored_host_measurement",
            "measurement": "whole_app_update_elapsed", "profile": profile,
            "workload": workload, "frames": options.frames,
            "warmup_frames": options.warmup_frames, "alternating": true,
            "budget_seconds": options.budget_seconds, "runs": runs,
            "icarus_compatible": false});
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .with_context(|| format!("create {}", path.display()))?;
        serde_json::to_writer_pretty(&mut file, &summary)?;
        writeln!(file)?;
    }
    Ok(())
}

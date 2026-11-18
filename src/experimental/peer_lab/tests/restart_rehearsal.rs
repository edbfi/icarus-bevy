//! Production-path restart rehearsal: the default-off player options together
//! (`checkpoint_pawn_before_identity`, `pawn_move_acks`,
//! `checkpoint_saved_locations`, `checkpoint_location_correction`,
//! `authored_controller_leave`, consumption and, in one scenario,
//! `player_inventory_projection`), driven as `icarus-server run` drives
//! them: an authored configuration file, `ConfigOptions::resolve`,
//! `network::prepare_run_with_tick_hz`, the Bevy app with its installed
//! systems, the run loop's save passes against the durable store, `run`'s
//! shutdown (admission stop, bridge drain, teardown, final save), and a
//! second start that reads everything back from the data directory.
//!
//! [`Server`] mirrors `run` step by step through the same library functions
//! (its docs list what it leaves out). Two substitutions sit at the Steam
//! boundary only: `AuthClient::test_bridge` stands in for the hosting
//! service's client, and an accepted ticket is set at the asynchronous
//! validation boundary, as `pending_auth_peer` does, since an authored
//! ticket cannot pass signature validation. Loopback UDP only.
//!
//! [`StockClient`] speaks real datagrams through the repository's own codecs
//! and readers. Its order is an assumption about a stock client, not a
//! recorded fact: Begin only after a ClientRestart naming its pawn (the
//! order F-1201 infers), moves only after the frozen-movement release,
//! nothing after a completed leave. No game-client behaviour is claimed.
use std::{
    collections::BTreeMap,
    io::ErrorKind,
    net::{SocketAddr, SocketAddrV4, UdpSocket},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use bevy::prelude::App;

use super::super::lossy_join::frozen_release;
use super::super::move_acks::{authored_move, move_bunch_payload, unreliable_bunch};
use super::bag_projection_tests::{BagDelta, bag_deltas_in, slot_properties};
use super::*;
use crate::{
    config::{ConfigOptions, ServerConfig},
    experimental::{
        actor_channel::{
            ActorBunchBody, BlockTarget, ContentBlock, NetGuidRef, SpawnVector, SubObjectClass,
            encode_actor_bunch_body, read_actor_bunch_body, read_spawn,
        },
        archive::ByteOrder,
        character_move::{
            MOVEMENT_MODE_FALLING, MOVEMENT_MODE_WALKING, MoveCorrection, MoveResponse,
            move_response_codec, read_move_response,
        },
        control_messages::{
            ArchiveText, ControlCodec, ControlMessage, Decoded, NetIdType, NetIdValue, UniqueNetId,
            encode_control_message, read_control_messages,
        },
        handshake::{decode, initial_request},
        packet::{
            BitWriter, Bunch, BunchHeader, Name, Packet, PacketConfig, Payload, Receiver,
            read_packet, write_packet,
        },
        packet_lab::client_cookie_seeds,
        pawn_open::{InventorySlot, PAWN_CHANNEL_INDEX, SlotItem},
        probe_initialise::{
            ActorBunch, ActorChannelTracker, authored_character_profile, authored_user_profile,
            begin_player_initialisation_field, bestiary_response_payload,
            friends_list_updated_field, initialise_codec, loadout_response_payload,
        },
        release::{ControllerLeavePlan, ReleaseProfile},
        rep_layout::{RepLayoutCodec, RepValue, parse_rep_properties, read_rep_properties},
        rep_rpc::{
            self, BACKEND_PROXY_FIELD_INDEX_BOUND, BESTIARY_MANAGER_FIELD_INDEX_BOUND,
            BP_ICARUS_PLAYER_CHARACTER_SURVIVAL_FIELD_INDEX_BOUND as PAWN_FIELD_BOUND,
            BP_ICARUS_PLAYER_CONTROLLER_SURVIVAL_FIELD_INDEX_BOUND as CONTROLLER_FIELD_BOUND,
            CLIENT_MOVE_RESPONSE_PACKED_FIELD_INDEX, CLIENT_PREPARE_LOADOUT,
            CLIENT_REQUEST_BESTIARY_DATA, CLIENT_REQUEST_GET_CHARACTER_PROFILE,
            CLIENT_REQUEST_GET_USER_PROFILE, ControllerLeaveRequest, FieldHeader,
            LeaveProspectSessionType, PLAYER_DATA_FIELD_INDEX_BOUND,
            SERVER_BEGIN_PLAYER_INITIALISATION_FIELD_INDEX, SERVER_MOVE_PACKED_FIELD_INDEX,
            encode_controller_leave_request, parse_rpc_payload, player_controller_rpc_parameters,
            read_field_header, write_field_header,
        },
        sequencing::{PacketSequencer, ReliableChannel, ReliableReceive, wire_reliable_sequence},
        session_open::{
            ACTOR_CHANNEL_NAME_INDEX, PLAYER_CONTROLLER_CHANNEL_INDEX, actor_codec,
            quantize_spawn_vector,
        },
        steam_auth::{SteamAuthMessage, decode_datagram},
    },
    native_player_runtime::{CheckpointWriter, PlayerRegistry},
    native_players::{PlayerKey, PlayerProjection},
    network,
    persistence::{SaveWorkerDisconnected, SnapshotStore},
    simulation::{RunClock, SimulationClock, build_app},
    steam_auth::{AuthEvent, AuthOutcome},
    steam_service::{AuthClient, TestBackend},
};

/// The operator's one spawn point (centimetres).
const SPAWN: [f32; 3] = [1_000.0, -2_000.0, 300.0];
const HOSTED_MAP_URL: &str = "/Authored/Maps/Workshop";
/// Character slot every client begins with.
const SLOT: i32 = 3;
/// `network.session_timeout_seconds` of every scenario.
const SESSION_TIMEOUT: Duration = Duration::from_secs(3);
/// `network.begin_timeout_seconds` where a scenario sets it: its smallest
/// accepted value. An authored test value, not a measured client timing.
const BEGIN_TIMEOUT: Duration = Duration::from_secs(5);
/// `autosave_seconds` of every scenario: `run`'s periodic save runs beside
/// the leave saves.
const AUTOSAVE: Duration = Duration::from_secs(3);
/// The saved-location clearance (F-1139) a relocation target adds to z.
const CLEARANCE_CM: f32 = 50.0;
/// Session one's walk: moves of 50 cm per 0.1 s of client time (5 m/s),
/// sent one per run-loop step, 30 m along +x in all.
const WALK_MOVES: u16 = 60;
const WALK_STEP_CM: f32 = 50.0;
const MOVE_SECONDS: f32 = 0.1;

// ------------------------------------------------------------------ inputs

/// One scenario's authored inputs and the configuration file naming them.
struct Inputs {
    _directory: tempfile::TempDir,
    config: PathBuf,
    /// What a client of the pinned build knows: its network checksum and the
    /// pinned controller's leave layout.
    network_version: u32,
    leave: ControllerLeavePlan,
}

impl Inputs {
    /// Scenario 1's configuration; `bag` adds `player_inventory` and
    /// `player_inventory_projection` over an authored recipe and binding.
    fn new(bag: bool) -> Self {
        Self::with_begin_timeout(bag, None)
    }

    /// [`Self::new`], with `network.begin_timeout_seconds` when `begin` is set.
    fn with_begin_timeout(bag: bool, begin: Option<Duration>) -> Self {
        use crate::pak_writer::{fresh_join_pak_with, table_json};
        let directory = tempfile::tempdir().unwrap();
        let path = |name: &str| directory.path().join(name);
        let quoted = |path: &Path| toml::Value::String(path.display().to_string()).to_string();
        let mut extra = vec![(
            "Prospects/D_ProspectList.json",
            table_json(&["AuthoredWorkshop"]),
        )];
        if bag {
            extra.push((
                "Items/D_ItemsStatic.json",
                table_json(&["ItemZero", "AuthoredWater", "AuthoredRation"]),
            ));
        }
        let pak = path("authored.pak");
        std::fs::write(&pak, fresh_join_pak_with(&extra)).unwrap();
        let gameplay = path("authored-gameplay.json");
        let profile = crate::gameplay_profile::fresh_player_profile(
            &mut crate::data_pak::DataPak::open(&pak).unwrap(),
            &crate::gameplay_profile::FreshPlayerValues::default(),
        )
        .unwrap();
        std::fs::write(&gameplay, serde_json::to_vec(&profile).unwrap()).unwrap();
        let mut inventory = String::new();
        if bag {
            let recipe = path("bag-recipe.json");
            std::fs::write(
                &recipe,
                serde_json::to_vec(&serde_json::json!({
                    "format": 2,
                    "items": [{"key": "water", "stack_limit": 10},
                              {"key": "ration", "stack_limit": 1}],
                    "inventories": [{
                        "key": "bag", "initial_slots": 2,
                        "slot_rules": [{"allowed_items": ["water", "ration"]},
                                       {"allowed_items": ["water", "ration"]}],
                        "force_add_when_zero": false, "overflow_limit": 0
                    }],
                    "starter": [
                        {"inventory": "bag", "requested_slot": 0, "item": "water", "count": 3},
                        {"inventory": "bag", "requested_slot": 1, "item": "ration", "count": 1}
                    ],
                    "consumables": [{"item": "water", "oxygen": 0, "water": 25, "food": 0}]
                }))
                .unwrap(),
            )
            .unwrap();
            let binding = path("bag-binding.json");
            std::fs::write(
                &binding,
                serde_json::to_vec(&serde_json::json!({
                    "format": 2,
                    "inventories": [{"inventory": "bag", "info_row": "Unused",
                                     "slot_queries": ["Any", "Any"]}],
                    "items": [
                        {"item": "water", "row": "AuthoredWater", "count_property": true,
                         "dynamic": [], "unit_weight": 5},
                        {"item": "ration", "row": "AuthoredRation", "count_property": false,
                         "dynamic": [], "unit_weight": 7}
                    ]
                }))
                .unwrap(),
            )
            .unwrap();
            inventory = format!(
                "player_inventory = {}\nplayer_inventory_projection = {}\n",
                quoted(&recipe),
                quoted(&binding)
            );
        }
        let release = Path::new(env!("CARGO_MANIFEST_DIR")).join("config/release.toml");
        // An ephemeral loopback game port, released for the server to bind;
        // the hosting query port is validated but never bound here.
        let port = UdpSocket::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let query = if port == 27_015 { 27_016 } else { 27_015 };
        let data_dir = path("state");
        let begin = begin
            .map(|limit| format!("begin_timeout_seconds = {}\n", limit.as_secs()))
            .unwrap_or_default();
        let text = format!(
            r#"# Authored rehearsal configuration: every player option together.
data_dir = {data_dir}
tick_hz = 30
max_catch_up_ticks = 4
autosave_seconds = {autosave}
shutdown_timeout_seconds = 10
status_interval_seconds = 10

[network]
enabled = true
bind = "127.0.0.1"
port = {port}
max_players = 8
session_timeout_seconds = {timeout}
{begin}release_profile = {release}
data_pak = {pak}
authored_gameplay = {gameplay}
controller_startup = ["restart-pawn"]
authored_controller_leave = true
checkpoint_pawn_before_identity = true
pawn_move_acks = true
checkpoint_saved_locations = true
checkpoint_location_correction = true
{inventory}
[network.player_persistence]
prospect_id = "rehearsal-workshop"
map_url = "{HOSTED_MAP_URL}"
prospect_row = "AuthoredWorkshop"
difficulty = 2

[network.player_consumption]
oxygen = 0
water = 2
food = 5

[network.authored_world]
seed = 42
seed_initialised = true
flod_tile_count = 0

[network.spawn]
points = [{{ x = {x:.1}, y = {y:.1}, z = {z:.1}, yaw = 90.0 }}]

[network.hosting]
query_bind = "127.0.0.1"
query_port = {query}
private_ip = "127.0.0.1"
seconds = 600
name = "Rehearsal workshop"
map_url = "{HOSTED_MAP_URL}"
prospect_id = "rehearsal-workshop"
prospect_row = "AuthoredWorkshop"
lobby_name = "Workshop"
difficulty = 2
"#,
            data_dir = quoted(&data_dir),
            autosave = AUTOSAVE.as_secs(),
            timeout = SESSION_TIMEOUT.as_secs(),
            release = quoted(&release),
            pak = quoted(&pak),
            gameplay = quoted(&gameplay),
            x = SPAWN[0],
            y = SPAWN[1],
            z = SPAWN[2],
        );
        let config = path("server.toml");
        std::fs::write(&config, text).unwrap();
        let profile = ReleaseProfile::load(&release).unwrap();
        Self {
            config,
            network_version: profile.pin.network_checksum,
            leave: profile.controller_leave_plan().unwrap().unwrap(),
            _directory: directory,
        }
    }
}

// ------------------------------------------------------------------ server

/// What `run`'s shutdown left: the `stopped` record's network part and the
/// save outcomes.
struct Stopped {
    record: serde_json::Value,
    commits: u64,
    save_failures: u64,
    players: crate::native_players::PlayerSnapshot,
}

/// One server process, as `run` starts, steps and stops it, through the same
/// library calls in the same order. Not covered here: CLI parsing, the
/// private leave proof, official data discovery (the pak is configured),
/// `--max-ticks`/compressed time, `NativeHosting` (Steam advert, start,
/// activation, membership publication and stop), the SIGINT/SIGTERM handler,
/// run-record printing, clamped-frame and tick metrics, and the periodic
/// status cadence (the record's network fields are composed on demand).
struct Server {
    app: App,
    writer: CheckpointWriter,
    config: ServerConfig,
    clock: RunClock,
    last_save_request: Duration,
    auth: AuthClient,
    address: SocketAddrV4,
    reopened: bool,
    initial_vitals: crate::experimental::pawn_open::CharacterVitals,
    save_submissions: u64,
    commits: u64,
    save_failures: u64,
    /// Last: its drop fails the shared queue, which the lab treats as lost
    /// Steam service, so it must outlive the app.
    backend: TestBackend,
}

impl Server {
    /// `run`'s startup, from configuration to the listening app.
    fn start(config: &Path) -> Self {
        let config = ConfigOptions {
            config: Some(config.to_path_buf()),
            ..ConfigOptions::default()
        }
        .resolve()
        .unwrap();
        assert!(config.network.enabled && config.network.data_pak.is_some());
        let (mut prepared, players) = network::prepare_run_with_tick_hz(
            &config.network,
            network::peer_lab_config(&config.network),
            config.tick_hz,
        )
        .unwrap();
        prepared
            .as_mut()
            .unwrap()
            .prepare_runtime(config.autosave_seconds)
            .unwrap();
        let initial_vitals = players.as_ref().unwrap().initial_vitals;
        let existing_state = match std::fs::read_dir(&config.data_dir) {
            Ok(entries) => entries.map(|entry| entry.unwrap().file_name()).any(|name| {
                name == "manifest.json" || name.to_string_lossy().starts_with("snapshot-")
            }),
            Err(error) if error.kind() == ErrorKind::NotFound => false,
            Err(error) => panic!("inspect native state directory: {error}"),
        };
        let mut fresh_app = (!existing_state).then(|| {
            let initial = network::fresh_player_snapshot(players.as_ref());
            let mut app = build_app(&config, &initial).unwrap();
            network::install_player_registry(app.world_mut(), players.as_ref(), &config.network)
                .unwrap();
            prepared.as_mut().unwrap().prepare_app(&mut app).unwrap();
            app
        });
        let store = if config.data_dir.join("server.lock").exists() {
            SnapshotStore::open_existing(&config.data_dir)
        } else {
            SnapshotStore::open(&config.data_dir)
        }
        .unwrap();
        let loaded = store.load().unwrap();
        assert!(fresh_app.is_none() || loaded.is_none());
        assert!(
            !loaded
                .as_ref()
                .is_some_and(|loaded| loaded.recovered_previous),
            "the current generation must load"
        );
        let reopened = loaded.is_some();
        let mut snapshot = loaded
            .map(|loaded| loaded.snapshot)
            .unwrap_or_else(|| network::fresh_player_snapshot(players.as_ref()));
        network::open_player_snapshot(&mut snapshot, players.as_ref()).unwrap();
        // `run`'s hosting and projection guards refuse a preserved world.
        assert!(snapshot.preserved_prospect.is_none());
        config
            .network
            .authored_world
            .as_ref()
            .unwrap()
            .validate_preserved_prospect(false)
            .unwrap();
        let mut app = match fresh_app.take() {
            Some(app) => app,
            None => build_app(&config, &snapshot).unwrap(),
        };
        network::install_player_registry(app.world_mut(), players.as_ref(), &config.network)
            .unwrap();
        {
            // `run`'s preserved-prospect branches need a preserved world.
            let bound = prepared.as_mut().unwrap().session_mut();
            let history = bound
                .saved_managers
                .iter()
                .find(|(slot, _)| *slot == ManagerSlot::PlayerHistory)
                .map(|(_, plan)| plan.clone());
            if let Some(plan) = history
                && !bound.lab.has_player_history_open()
            {
                bound
                    .lab
                    .add_manager_open(ManagerSlot::PlayerHistory, plan)
                    .unwrap();
            }
        }
        prepared.as_mut().unwrap().prepare_app(&mut app).unwrap();
        // The Steam boundary: the hosting service's client is replaced.
        let (auth, backend) = AuthClient::test_bridge(64);
        let bound = prepared
            .unwrap()
            .bind_authenticated(
                SocketAddrV4::new(config.network.bind, config.network.port),
                auth.clone(),
            )
            .unwrap();
        let SocketAddr::V4(address) =
            network::install_configured(&mut app, bound, &config.network).unwrap()
        else {
            unreachable!("IPv4 loopback")
        };
        app.finish();
        app.cleanup();
        let start_tick = snapshot.tick;
        drop(snapshot);
        drop(players);
        let writer = CheckpointWriter::spawn(store).unwrap();
        let clock = RunClock {
            compressed: false,
            start: Instant::now(),
            start_tick,
            tick_hz: config.tick_hz,
        };
        Self {
            app,
            writer,
            config,
            clock,
            last_save_request: Duration::ZERO,
            auth,
            address,
            reopened,
            initial_vitals,
            save_submissions: 0,
            commits: 0,
            save_failures: 0,
            backend,
        }
    }

    /// One iteration of `run`'s real-time loop body.
    fn step(&mut self) {
        let frame = Instant::now();
        self.app.update();
        crate::native_lifecycle_runtime::modifier_runtime_health(self.app.world()).unwrap();
        crate::native_player_runtime::consumption_runtime_health(self.app.world()).unwrap();
        network::authored_clock_runtime_health(self.app.world()).unwrap();
        let tick = self.app.world().resource::<SimulationClock>().tick;
        let now = self.clock.elapsed(tick);
        match self.writer.poll(self.app.world_mut()) {
            Ok(Some(_)) => self.commits += 1,
            Ok(None) => {}
            Err(error) => {
                assert!(!error.is::<SaveWorkerDisconnected>(), "{error:#}");
                self.save_failures += 1;
            }
        }
        // `run` publishes this membership to the Steam service.
        self.app
            .world_mut()
            .resource_mut::<PeerLab>()
            .authenticated_membership()
            .unwrap();
        let leave_due = self
            .app
            .world_mut()
            .get_resource_mut::<PlayerRegistry>()
            .is_some_and(|mut registry| registry.checkpoint_save_needed(Instant::now()));
        let periodic_due = now.saturating_sub(self.last_save_request)
            >= Duration::from_secs(self.config.autosave_seconds);
        if (leave_due || periodic_due) && !self.writer.is_busy() {
            self.writer.submit(self.app.world_mut()).unwrap();
            self.last_save_request = self.clock.elapsed(tick);
            self.save_submissions += 1;
        }
        std::thread::sleep(self.config.tick_interval().saturating_sub(frame.elapsed()));
    }

    fn lab(&self) -> &PeerLab {
        self.app.world().resource::<PeerLab>()
    }

    fn sessions(&self) -> crate::experimental::peer_lab::SessionCounts {
        network::session_counts(self.app.world()).unwrap()
    }

    fn registry(&self) -> &PlayerRegistry {
        self.app.world().resource::<PlayerRegistry>()
    }

    fn stored(&self, key: PlayerKey) -> Option<PlayerProjection> {
        self.registry().snapshot().projection(key)
    }

    /// The Steam boundary: the client's ticket is taken as validated for
    /// `account` and the backend accepts it, as the hosting service would
    /// report a CM acceptance.
    fn accept_ticket(&mut self, address: SocketAddrV4, account: u64) {
        let request = self.auth.allocate_request_id().unwrap();
        {
            let mut lab = self.app.world_mut().resource_mut::<PeerLab>();
            let entry = lab.peers.get_mut(&address).unwrap();
            entry.auth_request = Some(request);
            entry.auth_identity = Some(account);
        }
        self.backend
            .event(AuthEvent {
                request_id: request,
                outcome: AuthOutcome::Accepted,
            })
            .unwrap();
    }

    /// The network fields of `run`'s `status` record.
    fn status(&self) -> serde_json::Value {
        let world = self.app.world();
        let mut record = serde_json::json!({
            "event": "status",
            "save_failures": self.save_failures,
            "sessions": network::session_counts(world),
            "controller_receive": network::controller_receive_diagnostics(world),
            "pawn_receive": network::pawn_receive_diagnostics(world),
            "readiness": world.get_resource::<PeerLab>().map(|lab| lab.readiness_counts()),
        });
        with_player_leave(&mut record, world);
        record
    }

    /// `run`'s stop: admission stops, the bridge drains, the network owner is
    /// torn down, the writer settles and the final save commits before its
    /// deadline. Then the process ends: the app is dropped and the writer's
    /// thread releases the store's lock.
    fn shutdown(mut self) -> Stopped {
        let world = self.app.world_mut();
        // The loop's stop request, then the idempotent post-loop sequence.
        network::stop_admission(world);
        network::stop_admission(world);
        network::drain_player_bridge(world);
        let readiness = world
            .get_resource::<PeerLab>()
            .map(|lab| lab.readiness_counts());
        let sessions = network::teardown(world);
        let deadline = Instant::now() + Duration::from_secs(self.config.shutdown_timeout_seconds);
        match self.writer.settle_before(world, deadline).unwrap() {
            Some(Ok(_)) => self.commits += 1,
            Some(Err(_)) => self.save_failures += 1,
            None => {}
        }
        assert!(Instant::now() < deadline, "final capture deadline");
        self.writer.submit_before(world, deadline).unwrap();
        self.writer
            .finish_before(world, deadline)
            .unwrap()
            .expect("the final save commits");
        self.commits += 1;
        let mut record = serde_json::json!({
            "event": "stopped",
            "tick": world.resource::<SimulationClock>().tick,
            "sessions": &sessions,
            "controller_receive": network::controller_receive_diagnostics(world),
            "pawn_receive": network::pawn_receive_diagnostics(world),
            "readiness_before_shutdown": &readiness,
        });
        with_player_leave(&mut record, world);
        let players = world.resource::<PlayerRegistry>().snapshot();
        let lock = self.config.data_dir.join("server.lock");
        let Self {
            app,
            writer,
            commits,
            save_failures,
            backend,
            ..
        } = self;
        drop(writer);
        drop(app);
        drop(backend);
        // Process exit releases the store's lock; here the writer's thread
        // does, once it sees its queue closed.
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let file = std::fs::File::open(&lock).unwrap();
            if file.try_lock().is_ok() {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "the store lock was never released"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        Stopped {
            record,
            commits,
            save_failures,
            players,
        }
    }
}

/// `run` adds the registry's leave totals to `status` and `stopped` when
/// authored leave diagnostics exist.
fn with_player_leave(record: &mut serde_json::Value, world: &bevy::prelude::World) {
    if network::session_counts(world).is_some_and(|counts| counts.controller_leave.is_some())
        && let Some(registry) = world.get_resource::<PlayerRegistry>()
    {
        record["player_leave"] = serde_json::json!(registry.leave_counts());
    }
}

// ------------------------------------------------------------------ client

/// One reliable bunch the client owes until a positive ACK.
struct OutReliable {
    bunch: Bunch,
    packet: Option<u32>,
    resend: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Component {
    BackendProxy,
    PlayerData,
    Bestiary,
}

/// What the client decoded, in its own delivery order.
#[derive(Default)]
struct Observed {
    log: Vec<String>,
    /// When each logged event was decoded or sent.
    at: Vec<Instant>,
    welcome_map_url: Option<String>,
    /// The controller actor's GUID from its open.
    controller: Option<u32>,
    pawn: Option<u32>,
    pawn_location: Option<SpawnVector>,
    /// The first pawn's spawn level resolved from received exports at open.
    pawn_level_path: Option<String>,
    /// The pawn a controller ClientRestart (field 40) named.
    restart: Option<u32>,
    begin_sent: bool,
    released: bool,
    /// Reliable non-open pawn update bodies, in delivery order.
    pawn_updates: Vec<Payload>,
    responses: Vec<MoveResponse>,
    completions: Vec<u32>,
    /// A control-channel close from the server.
    closed_by_server: bool,
    /// A Failure or Upgrade reply, as debug text.
    refused: Option<String>,
}

impl Observed {
    fn assert_hosted_map(&self) {
        assert_eq!(
            self.welcome_map_url.as_deref(),
            Some(HOSTED_MAP_URL),
            "Welcome names the configured hosted map"
        );
        assert_eq!(
            self.pawn_level_path.as_deref(),
            Some("/Authored/Maps/Workshop.Workshop.PersistentLevel"),
            "the first pawn's level GUID resolves to the hosted map's PersistentLevel"
        );
    }

    fn position(&self, event: &str) -> usize {
        self.log
            .iter()
            .position(|logged| logged == event)
            .unwrap_or_else(|| panic!("no {event} in {:?}", self.log))
    }

    /// The time from the first `from` to the first `to`.
    fn between(&self, from: &str, to: &str) -> Duration {
        self.at[self.position(to)].duration_since(self.at[self.position(from)])
    }

    fn acks(&self) -> Vec<f32> {
        self.responses
            .iter()
            .filter_map(|response| match response {
                MoveResponse::Ack(timestamp) => Some(*timestamp),
                MoveResponse::Correction(_) => None,
            })
            .collect()
    }
}

/// A synthetic client on its own loopback socket.
struct StockClient {
    socket: UdpSocket,
    address: SocketAddrV4,
    sequencer: PacketSequencer,
    write: PacketConfig,
    read: PacketConfig,
    control_codec: ControlCodec,
    control_in: ReliableChannel<Vec<Decoded>>,
    tracker: ActorChannelTracker,
    codec: RepLayoutCodec,
    engine: u32,
    initial_out_reliable: u32,
    next_reliable: BTreeMap<u32, u32>,
    outgoing: Vec<OutReliable>,
    account: u64,
    leave: ControllerLeavePlan,
    /// Whether this client sends Begin at all.
    begins: bool,
    seen: Observed,
}

fn drain(socket: &UdpSocket) -> Vec<Vec<u8>> {
    let mut datagrams = Vec::new();
    loop {
        let mut buffer = [0; 4096];
        match socket.recv(&mut buffer) {
            Ok(length) => datagrams.push(buffer[..length].to_vec()),
            Err(error) if error.kind() == ErrorKind::WouldBlock => return datagrams,
            Err(error) => panic!("{error}"),
        }
    }
}

impl StockClient {
    /// The stateless handshake, the Steam boundary, then a queued Hello.
    fn connect(server: &mut Server, inputs: &Inputs, account: u64, begins: bool) -> Self {
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        socket.connect(server.address).unwrap();
        socket.set_nonblocking(true).unwrap();
        let SocketAddr::V4(address) = socket.local_addr().unwrap() else {
            unreachable!("IPv4 loopback")
        };
        socket.send(&initial_request()).unwrap();
        server.step();
        let challenge = drain(&socket).pop().expect("a challenge");
        let cookie = decode(&challenge).unwrap().cookie;
        socket.send(&challenge).unwrap();
        server.step();
        let acknowledged = drain(&socket).pop().expect("a handshake acknowledgement");
        assert_eq!(decode(&acknowledged).unwrap().cookie, cookie);
        // Game packets are gated until Steam accepts the ticket.
        server.accept_ticket(address, account);
        server.step();
        let results: Vec<_> = drain(&socket)
            .iter()
            .map(|bytes| decode_datagram(bytes, ByteOrder::Little, 0, 0).unwrap())
            .collect();
        assert_eq!(results, [SteamAuthMessage::Result { accepted: true }]);
        let (incoming, outgoing) = client_cookie_seeds(&cookie);
        let sequencer = PacketSequencer::new(incoming, outgoing, 1_024).unwrap();
        let lab = server.lab();
        let engine = lab
            .player_controller_open
            .as_ref()
            .unwrap()
            .engine_net_version;
        let codec = initialise_codec(engine);
        let write = lab.config.packet.packet;
        let read = PacketConfig {
            receiver: Receiver::Client,
            ..write
        };
        let (initial_in, initial_out) = sequencer.initial_reliable();
        let mut client = Self {
            socket,
            address,
            control_in: sequencer.new_channel(),
            tracker: ActorChannelTracker::new(read, codec.actor, initial_in),
            sequencer,
            write,
            read,
            control_codec: ControlCodec::default(),
            codec,
            engine,
            initial_out_reliable: initial_out,
            next_reliable: BTreeMap::new(),
            outgoing: Vec::new(),
            account,
            leave: inputs.leave.clone(),
            begins,
            seen: Observed::default(),
        };
        client.queue_control(
            ControlMessage::Hello {
                first_field: 1,
                version: inputs.network_version,
                token: ArchiveText::text("").unwrap(),
            },
            true,
        );
        client
    }

    fn note(&mut self, event: impl Into<String>) {
        self.seen.log.push(event.into());
        self.seen.at.push(Instant::now());
    }

    fn reliable_header(&mut self, channel: u32, control: bool, open: bool) -> BunchHeader {
        let last = self
            .next_reliable
            .get(&channel)
            .copied()
            .unwrap_or(self.initial_out_reliable);
        let sequence = last + 1;
        self.next_reliable.insert(channel, sequence);
        BunchHeader {
            control,
            open,
            close: false,
            close_reason: None,
            paused: false,
            reliable: true,
            channel_index: channel,
            has_package_map_exports: false,
            has_must_be_mapped_guids: false,
            partial: false,
            reliable_sequence_raw: Some(wire_reliable_sequence(sequence)),
            partial_initial: false,
            partial_final: false,
            channel_name: Some(Name::Hardcoded(if channel == 0 {
                255
            } else {
                ACTOR_CHANNEL_NAME_INDEX
            })),
        }
    }

    fn queue(&mut self, bunch: Bunch) {
        self.outgoing.push(OutReliable {
            bunch,
            packet: None,
            resend: true,
        });
    }

    fn queue_control(&mut self, message: ControlMessage, open: bool) {
        let header = self.reliable_header(0, open, open);
        let mut payload = BitWriter::new();
        payload.write_bytes(&encode_control_message(&message, &self.control_codec).unwrap());
        self.queue(Bunch {
            header,
            payload: Payload::from_writer(payload),
        });
    }

    /// One controller RPC block, reliable on the controller's channel.
    fn queue_rpc(&mut self, target: BlockTarget, field: &FieldHeader, bound: u32) {
        let mut fields = BitWriter::new();
        write_field_header(&mut fields, field, bound).unwrap();
        let payload = encode_actor_bunch_body(
            &ActorBunchBody {
                spawn: None,
                blocks: vec![ContentBlock {
                    has_rep_layout: false,
                    target,
                    payload: Payload::from_writer(fields),
                }],
            },
            &self.codec.actor,
        )
        .unwrap();
        let header = self.reliable_header(PLAYER_CONTROLLER_CHANNEL_INDEX, false, false);
        self.queue(Bunch { header, payload });
    }

    /// A normal leave of `kind` on the pinned controller layout.
    fn queue_leave(&mut self, kind: LeaveProspectSessionType) {
        let codec = RepLayoutCodec {
            actor: actor_codec(self.leave.engine_net_version(), ByteOrder::Little),
            checksums: false,
            limits: Default::default(),
        };
        let field = encode_controller_leave_request(
            ControllerLeaveRequest::Normal(Some(kind)),
            &self.leave,
            &codec,
        )
        .unwrap();
        let bound = self.leave.field_index_bound();
        self.queue_rpc(BlockTarget::Actor, &field, bound);
        self.note("leave");
    }

    /// One client packet: every reliable bunch owed (resends first), then
    /// `moves` as unreliable pawn bunches, and the ACK state.
    fn send(&mut self, moves: &[Payload]) {
        let header = self.sequencer.header().unwrap();
        let mut bunches = Vec::new();
        let mut carried = Vec::new();
        let mut bits = 0;
        for (position, out) in self.outgoing.iter().enumerate() {
            if !out.resend {
                continue;
            }
            let size = out.bunch.payload.bit_len();
            if !bunches.is_empty() && (bits + size > 6_000 || bunches.len() >= 16) {
                break;
            }
            bits += size;
            bunches.push(out.bunch.clone());
            carried.push(position);
        }
        for field in moves {
            bunches.push(unreliable_bunch(
                PAWN_CHANNEL_INDEX,
                move_bunch_payload(field.clone(), self.engine),
            ));
        }
        let bytes = write_packet(
            &Packet {
                magic: 0,
                header,
                info: None,
                bunches,
            },
            &self.write,
        )
        .unwrap();
        let packet = self.sequencer.commit().unwrap();
        for position in carried {
            self.outgoing[position].packet = Some(packet);
            self.outgoing[position].resend = false;
        }
        self.socket.send(&bytes).unwrap();
    }

    /// Send, let the server run one loop step, then read what it sent.
    fn exchange(&mut self, server: &mut Server, moves: &[Payload]) {
        self.send(moves);
        server.step();
        for bytes in drain(&self.socket) {
            self.receive(Some(server.lab()), &bytes);
        }
    }

    /// Keep exchanging (ACKs only) until `done`, within `limit` of real time.
    fn until(
        &mut self,
        server: &mut Server,
        limit: Duration,
        what: &str,
        done: impl Fn(&Observed, &Server) -> bool,
    ) {
        let deadline = Instant::now() + limit;
        while !done(&self.seen, server) {
            assert!(
                self.seen.refused.is_none() && !self.seen.closed_by_server,
                "the server closed the session while waiting for {what}: {:?}",
                self.seen.refused
            );
            assert!(
                Instant::now() < deadline,
                "gave up waiting for {what}; client log {:?}",
                self.seen.log
            );
            self.exchange(server, &[]);
        }
    }

    /// Read one server datagram; actor bunches need the server's plans, so
    /// after teardown (`lab` absent) only the control channel is read.
    fn receive(&mut self, lab: Option<&PeerLab>, bytes: &[u8]) {
        let packet = read_packet(bytes, &self.read).unwrap();
        let Some(received) = self.sequencer.receive(&packet.header).unwrap() else {
            return;
        };
        self.sequencer.ack_seq(received.sequence, true).unwrap();
        for notification in &received.notifications {
            for out in &mut self.outgoing {
                if out.packet == Some(notification.packet_id) && !out.resend {
                    if notification.delivered {
                        out.packet = None;
                    } else {
                        out.resend = true;
                    }
                }
            }
            self.outgoing
                .retain(|out| out.resend || out.packet.is_some());
        }
        for bunch in packet
            .bunches
            .iter()
            .filter(|b| b.header.channel_index == 0)
        {
            if bunch.header.close {
                self.seen.closed_by_server = true;
                self.note("control close");
            }
            let Some(raw) = bunch
                .header
                .reliable_sequence_raw
                .filter(|_| bunch.header.reliable)
            else {
                continue;
            };
            let Ok(decoded) =
                read_control_messages(&mut bunch.payload.reader(), &self.control_codec)
            else {
                continue;
            };
            if let Ok(ReliableReceive::Delivered(batches)) =
                self.control_in.receive_reliable(raw, decoded)
            {
                for message in batches.into_iter().flatten() {
                    if let Decoded::Message(message) = message {
                        self.on_control(&message);
                    }
                }
            }
        }
        let Some(lab) = lab else {
            return;
        };
        let mut notes = Vec::new();
        for bunch in self.tracker.feed_packet(packet, &mut notes) {
            self.on_actor(lab, &bunch);
        }
        assert!(notes.is_empty(), "client tracker: {notes:?}");
    }

    fn on_control(&mut self, message: &ControlMessage) {
        match message {
            ControlMessage::Challenge { .. } => {
                self.note("challenge");
                // A Steam login ID: the accepted ticket's account, in decimal.
                let login = ControlMessage::Login {
                    response: ArchiveText::text("0").unwrap(),
                    url: ArchiveText::text("?Name=Rehearsal").unwrap(),
                    unique_net_id: UniqueNetId {
                        kind: NetIdType::Indexed(3),
                        value: NetIdValue::Text(
                            ArchiveText::text(&self.account.to_string()).unwrap(),
                        ),
                    },
                    platform: ArchiveText::text("").unwrap(),
                };
                self.queue_control(login, false);
            }
            ControlMessage::Welcome { map_url, .. } => {
                self.seen.welcome_map_url = Some(String::from_utf16(map_url.units()).unwrap());
                self.note("welcome");
                self.queue_control(ControlMessage::Netspeed { rate: 100_000 }, false);
                self.queue_control(ControlMessage::Join, false);
            }
            ControlMessage::Failure { .. } | ControlMessage::Upgrade { .. } => {
                self.seen.refused = Some(format!("{message:?}"));
                self.note("failure");
            }
            _ => {}
        }
    }

    fn component(&self, guid: u32) -> Option<Component> {
        let name = self.tracker.package_map().resolve_path(guid)?;
        [
            ("BackendProxyComponent", Component::BackendProxy),
            ("PlayerDataComponent", Component::PlayerData),
            ("BestiaryManagerComponent", Component::Bestiary),
        ]
        .into_iter()
        .find(|(known, _)| known.eq_ignore_ascii_case(name))
        .map(|(_, component)| component)
    }

    fn on_actor(&mut self, lab: &PeerLab, bunch: &ActorBunch) {
        match bunch.channel_index {
            PLAYER_CONTROLLER_CHANNEL_INDEX => self.on_controller(bunch),
            PAWN_CHANNEL_INDEX => self.on_pawn(lab, bunch),
            channel
                if lab
                    .game_state_open
                    .as_ref()
                    .is_some_and(|session| session.channel_index == channel)
                    && self.is_own_record(lab, bunch) =>
            {
                self.note("record");
            }
            _ => {}
        }
    }

    fn on_controller(&mut self, bunch: &ActorBunch) {
        if bunch.open {
            let spawn = read_spawn(&mut bunch.payload.reader(), &self.codec.actor).unwrap();
            self.seen.controller = Some(spawn.actor.guid);
            self.note("controller open");
            return;
        }
        if bunch.close {
            self.note("controller close");
            return;
        }
        let body =
            read_actor_bunch_body(&mut bunch.payload.reader(), false, &self.codec.actor).unwrap();
        for block in &body.blocks {
            self.on_controller_block(block);
        }
    }

    fn on_controller_block(&mut self, block: &ContentBlock) {
        if block.has_rep_layout {
            if block.target != BlockTarget::Actor {
                return;
            }
            // Possession updates and leave completions share the actor layout.
            let codec = RepLayoutCodec {
                actor: actor_codec(self.leave.engine_net_version(), ByteOrder::Little),
                checksums: false,
                limits: Default::default(),
            };
            let Ok(values) = read_rep_properties(
                &mut block.payload.reader(),
                self.leave.actor_layout(),
                &codec,
            ) else {
                return;
            };
            for entry in values.entries {
                match (entry.handle, entry.value) {
                    (26, RepValue::Enum(value)) => {
                        self.seen.completions.push(value);
                        self.note("leave completion");
                    }
                    (17, _) => self.note("possession"),
                    _ => {}
                }
            }
            return;
        }
        let (target, bound) = match &block.target {
            BlockTarget::Actor => (None, CONTROLLER_FIELD_BOUND),
            BlockTarget::SubObject { object, .. } => {
                let Some(component) = self.component(object.guid) else {
                    return;
                };
                let bound = match component {
                    Component::BackendProxy => BACKEND_PROXY_FIELD_INDEX_BOUND,
                    Component::PlayerData => PLAYER_DATA_FIELD_INDEX_BOUND,
                    Component::Bestiary => BESTIARY_MANAGER_FIELD_INDEX_BOUND,
                };
                (Some((component, object.guid)), bound)
            }
        };
        let mut reader = block.payload.reader();
        while !reader.at_end() {
            let field =
                read_field_header(&mut reader, bound, self.codec.actor.limits.max_payload_bits)
                    .unwrap();
            match (target, field.index) {
                (None, 40) => self.on_restart(&field.payload),
                (None, 118) if field.payload.bit_len() == 0 => {
                    self.note("friends query");
                    let answer = friends_list_updated_field(&self.codec).unwrap();
                    self.queue_rpc(BlockTarget::Actor, &answer, CONTROLLER_FIELD_BOUND);
                }
                (Some((Component::BackendProxy, guid)), index)
                    if index == CLIENT_REQUEST_GET_USER_PROFILE
                        || index == CLIENT_REQUEST_GET_CHARACTER_PROFILE =>
                {
                    let character = index == CLIENT_REQUEST_GET_CHARACTER_PROFILE;
                    self.note(if character {
                        "character profile request"
                    } else {
                        "user profile request"
                    });
                    let mut items = vec![RpcItem::Value(RepValue::Bool(true))];
                    items.extend(if character {
                        authored_character_profile(SLOT).unwrap()
                    } else {
                        authored_user_profile().unwrap()
                    });
                    let answer = FieldHeader {
                        index: if character {
                            rep_rpc::RESPONSE_GET_CHARACTER_PROFILE
                        } else {
                            rep_rpc::RESPONSE_GET_USER_PROFILE
                        },
                        payload: rep_rpc::encode_rpc_payload(
                            &rep_rpc::backend_profile_response_parameters(character),
                            &[RpcArgument::Sent(items)],
                            &self.codec,
                        )
                        .unwrap(),
                    };
                    self.queue_rpc(sent_to(guid), &answer, BACKEND_PROXY_FIELD_INDEX_BOUND);
                }
                (Some((Component::PlayerData, guid)), CLIENT_PREPARE_LOADOUT) => {
                    self.note("loadout request");
                    let answer = FieldHeader {
                        index: rep_rpc::SERVER_RECEIVE_LOADOUT,
                        payload: loadout_response_payload(&self.codec).unwrap(),
                    };
                    self.queue_rpc(sent_to(guid), &answer, PLAYER_DATA_FIELD_INDEX_BOUND);
                }
                (Some((Component::Bestiary, guid)), CLIENT_REQUEST_BESTIARY_DATA) => {
                    self.note("bestiary request");
                    let answer = FieldHeader {
                        index: rep_rpc::SERVER_RECEIVE_BESTIARY_DATA,
                        payload: bestiary_response_payload(&self.codec).unwrap(),
                    };
                    self.queue_rpc(sent_to(guid), &answer, BESTIARY_MANAGER_FIELD_INDEX_BOUND);
                }
                _ => {}
            }
        }
    }

    /// A controller ClientRestart (field 40): Begin follows only when it
    /// names the pawn this client opened.
    fn on_restart(&mut self, payload: &Payload) {
        let parameters = player_controller_rpc_parameters(40).unwrap();
        let arguments = parse_rpc_payload(payload, &parameters, &self.codec).unwrap();
        let [RpcArgument::Sent(items)] = arguments.as_slice() else {
            panic!("ClientRestart sends its pawn: {arguments:?}");
        };
        let [RpcItem::Value(RepValue::Object(pawn))] = items.as_slice() else {
            panic!("ClientRestart names one object: {items:?}");
        };
        self.seen.restart = Some(pawn.guid);
        self.note("restart");
        if self.begins && !self.seen.begin_sent && self.seen.pawn == Some(pawn.guid) {
            let field = begin_player_initialisation_field(SLOT, &self.codec).unwrap();
            self.queue_rpc(BlockTarget::Actor, &field, CONTROLLER_FIELD_BOUND);
            self.seen.begin_sent = true;
            self.note("begin");
        }
    }

    fn on_pawn(&mut self, lab: &PeerLab, bunch: &ActorBunch) {
        if bunch.open {
            let spawn = read_spawn(&mut bunch.payload.reader(), &self.codec.actor).unwrap();
            if self.seen.pawn.is_none() {
                self.seen.pawn_level_path = spawn
                    .dynamic
                    .as_ref()
                    .and_then(|dynamic| dynamic.level.as_ref())
                    .and_then(|level| self.tracker.package_map().resolve_full_path(level.guid));
            }
            self.seen.pawn = Some(spawn.actor.guid);
            self.seen.pawn_location = spawn.dynamic.and_then(|dynamic| dynamic.location);
            self.note("pawn open");
            return;
        }
        if bunch.reliable {
            self.seen.pawn_updates.push(bunch.payload.clone());
            self.note("pawn update");
            return;
        }
        if let Some(response) = move_response(self.engine, &bunch.payload) {
            self.note(match response {
                MoveResponse::Ack(_) => "ack",
                MoveResponse::Correction(_) => "correction",
            });
            self.seen.responses.push(response);
        } else if frozen_release(lab, &bunch.payload) {
            self.seen.released = true;
            self.note("frozen release");
        }
    }

    /// Whether a GameState bunch creates this client's own connected-player
    /// record (handle 3 naming its controller).
    fn is_own_record(&self, lab: &PeerLab, bunch: &ActorBunch) -> bool {
        let (Some(plan), Some(controller)) = (&lab.connected_players, self.seen.controller) else {
            return false;
        };
        if !bunch.reliable || bunch.open {
            return false;
        }
        let codec = super::super::record_codec();
        let Ok(body) = read_actor_bunch_body(&mut bunch.payload.reader(), false, &codec.actor)
        else {
            return false;
        };
        body.blocks.iter().any(|block| {
            matches!(
                &block.target,
                BlockTarget::SubObject {
                    class: SubObjectClass::Class(_),
                    ..
                }
            ) && parse_rep_properties(&block.payload, &plan.record_layout, &codec).is_ok_and(
                |values| {
                    values.entries.iter().any(|entry| {
                        entry.handle == 3
                            && matches!(&entry.value, RepValue::Object(reference)
                                if reference.guid == controller)
                    })
                },
            )
        })
    }
}

fn sent_to(guid: u32) -> BlockTarget {
    BlockTarget::SubObject {
        object: NetGuidRef::plain(guid),
        class: SubObjectClass::ClientSent,
    }
}

/// A field-30 move response alone in an unreliable pawn bunch, read strictly.
fn move_response(engine: u32, payload: &Payload) -> Option<MoveResponse> {
    let codec = move_response_codec(engine, ByteOrder::Little);
    let body = read_actor_bunch_body(&mut payload.reader(), false, &codec.actor).ok()?;
    let [block] = body.blocks.as_slice() else {
        return None;
    };
    if block.has_rep_layout || block.target != BlockTarget::Actor {
        return None;
    }
    let mut reader = block.payload.reader();
    let field = read_field_header(&mut reader, PAWN_FIELD_BOUND, usize::MAX).ok()?;
    (reader.at_end() && field.index == CLIENT_MOVE_RESPONSE_PACKED_FIELD_INDEX)
        .then(|| read_move_response(&field.payload, &codec).unwrap())
}

/// The CharacterState values of a reliable pawn update, if it is one.
fn vitals_update(lab: &PeerLab, payload: &Payload) -> Option<BTreeMap<u32, RepValue>> {
    let plan = &lab.pawn_open.as_ref()?.0;
    let codec = RepLayoutCodec {
        actor: actor_codec(plan.engine_net_version, ByteOrder::Little),
        checksums: false,
        limits: Default::default(),
    };
    let body = read_actor_bunch_body(&mut payload.reader(), false, &codec.actor).ok()?;
    let [block] = body.blocks.as_slice() else {
        return None;
    };
    if !block.has_rep_layout {
        return None;
    }
    let stream = read_rep_properties(
        &mut block.payload.reader(),
        &plan.content.character_state_layout,
        &codec,
    )
    .ok()?;
    Some(
        stream
            .entries
            .into_iter()
            .map(|entry| (entry.handle, entry.value))
            .collect(),
    )
}

/// The client's projected slots and weight after the deltas among `payloads`:
/// opened empty at F-978's key (2 + slots), each delta checked against the
/// key it holds. Subobject indices follow the content's own inventories.
fn projected_bag(
    lab: &PeerLab,
    connection: u32,
    payloads: &[Payload],
) -> (Vec<BTreeMap<u32, RepValue>>, i32) {
    let content = &lab.pawn_open.as_ref().unwrap().2;
    let bound = lab
        .player_bag_projection
        .as_ref()
        .unwrap()
        .0
        .opened_inventories()
        .len();
    assert_eq!(bound, 1, "one bound inventory");
    let first = u32::try_from(content.inventories.len() - bound).unwrap();
    // Query row 1 is D_TagQueries `Any` in the authored pak.
    let mut slots: Vec<_> = (0..2)
        .map(|index| slot_properties(&InventorySlot::empty(1, index)))
        .collect();
    let mut key = 2 + 2;
    let mut current_weight = 0;
    for BagDelta {
        inventory,
        weight,
        slots: delta,
    } in bag_deltas_in(lab, connection, first..first + 1, payloads.to_vec())
    {
        assert_eq!(inventory, 0);
        if let Some(weight) = weight {
            assert_ne!(weight, current_weight);
            current_weight = weight;
        }
        assert_eq!(delta.base_replication_key, key, "base is the held key");
        assert_eq!(
            delta.array_replication_key,
            key + delta.changed.len() as i32,
            "one key per changed slot"
        );
        assert!(delta.deleted_ids.is_empty());
        key = delta.array_replication_key;
        for (id, item) in &delta.changed {
            let crate::experimental::rep_rpc::FastArrayItem::Delta(stream) = item else {
                panic!("delta items");
            };
            let slot = &mut slots[usize::try_from(id - 1).unwrap()];
            for entry in &stream.entries {
                slot.insert(entry.handle, entry.value.clone());
            }
        }
    }
    (slots, current_weight)
}

/// The authored starter bag by literal identities: `AuthoredWater` is
/// D_ItemsStatic row 1 with its count 3 as dynamic property 7,
/// `AuthoredRation` row 2 without a count; both slots query row 1 (`Any`).
fn starter_bag_literal() -> Vec<BTreeMap<u32, RepValue>> {
    let slot = |index: i32, row: u16, dynamic: Vec<(u32, i32)>| {
        slot_properties(&InventorySlot {
            query_row: 1,
            index,
            item: Some(SlotItem {
                dynamic,
                ..SlotItem::new(row).unwrap()
            }),
        })
    };
    vec![slot(0, 1, vec![(7, 3)]), slot(1, 2, Vec::new())]
}

// ------------------------------------------------------------------ steps

/// Stock order to the frozen release: Hello, Login, Join, then the opens;
/// Begin only after ClientRestart names the pawn; each profile, loadout,
/// bestiary and friends request answered as it arrives.
fn enter(server: &mut Server, inputs: &Inputs, limit: Duration) -> StockClient {
    let mut client = StockClient::connect(server, inputs, ACCOUNT, true);
    client.until(server, limit, "the frozen release", |seen, _| seen.released);
    let seen = &client.seen;
    assert_eq!(
        seen.restart, seen.pawn,
        "ClientRestart names the opened pawn"
    );
    for (before, after) in [
        ("welcome", "controller open"),
        ("controller open", "pawn open"),
        ("pawn open", "restart"),
        ("restart", "begin"),
        ("begin", "user profile request"),
        ("begin", "character profile request"),
        ("begin", "loadout request"),
        ("begin", "bestiary request"),
        ("begin", "friends query"),
        ("user profile request", "record"),
        ("character profile request", "record"),
        ("loadout request", "record"),
        ("bestiary request", "record"),
        ("friends query", "record"),
        ("record", "frozen release"),
    ] {
        assert!(
            seen.position(before) < seen.position(after),
            "{before} precedes {after}: {:?}",
            seen.log
        );
    }
    client
}

/// A normal leave, its completion, and the server's removal of the session
/// once the completion's prefix is acknowledged.
fn leave(client: &mut StockClient, server: &mut Server) {
    client.queue_leave(LeaveProspectSessionType::ReturnToTitlescreen);
    client.until(
        server,
        Duration::from_secs(5),
        "the leave completion",
        |seen, _| !seen.completions.is_empty(),
    );
    assert_eq!(
        client.seen.completions,
        [LeaveProspectSessionType::ReturnToTitlescreen as u32]
    );
    let address = client.address;
    client.until(
        server,
        Duration::from_secs(3),
        "the session's removal",
        |_, server| !server.lab().peers.contains_key(&address),
    );
}

fn controller_field(record: &serde_json::Value, index: u32) -> &serde_json::Value {
    record["controller_receive"]["fields"]["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["index"] == index && entry["target"] == "Actor")
        .unwrap_or_else(|| panic!("no controller field {index}: {record}"))
}

fn movement_row(record: &serde_json::Value) -> &serde_json::Value {
    let rows = record["pawn_receive"]["fields"]["movement"]
        .as_array()
        .unwrap();
    let [row] = rows.as_slice() else {
        panic!("one movement row: {record}");
    };
    assert_eq!(row["index"], SERVER_MOVE_PACKED_FIELD_INDEX);
    assert_eq!(row["reliable"], false);
    assert_eq!(row["disposition"], "Recognized");
    row
}

/// Read what `run`'s teardown sent this still-connected client.
fn after_shutdown(client: &mut StockClient) {
    for bytes in drain(&client.socket) {
        client.receive(None, &bytes);
    }
}

fn wait_lab(server: &mut Server, limit: Duration, what: &str, done: impl Fn(&PeerLab) -> bool) {
    let deadline = Instant::now() + limit;
    while !done(server.lab()) {
        assert!(Instant::now() < deadline, "gave up waiting for {what}");
        server.step();
    }
}

// ------------------------------------------------------------------ scenarios

/// Scenario 1: entry in stock order, a walk with acknowledgements, a normal
/// leave, `run`'s shutdown, a second start on the same store, and a rejoin
/// whose first move is corrected to the saved location with the depleted
/// vitals of the first session.
#[test]
fn rehearsal_entry_leave_restart_and_rejoin_at_the_saved_location() {
    let inputs = Inputs::new(false);
    let key = PlayerKey::new(ACCOUNT, SLOT).unwrap();

    // ---- first process
    let mut server = Server::start(&inputs.config);
    assert!(!server.reopened);
    let fresh = server.initial_vitals;
    let mut client = enter(&mut server, &inputs, Duration::from_secs(5));
    client.seen.assert_hosted_map();
    assert_eq!(
        client.seen.pawn_location,
        Some(quantize_spawn_vector(SPAWN).unwrap()),
        "the pawn opens at the spawn point"
    );
    assert!(client.seen.between("record", "frozen release") < Duration::from_secs(3));
    let mut at = SPAWN;
    let mut timestamp = 10.0;
    for step in 1..=WALK_MOVES {
        at = [
            SPAWN[0] + WALK_STEP_CM * f32::from(step),
            SPAWN[1],
            SPAWN[2],
        ];
        timestamp += MOVE_SECONDS;
        client.exchange(
            &mut server,
            &[authored_move(timestamp, at, Some(MOVEMENT_MODE_WALKING))],
        );
    }
    assert_eq!(at[0] - SPAWN[0], 3_000.0, "30 m walked");
    let last = timestamp;
    client.until(
        &mut server,
        Duration::from_secs(3),
        "the last move's ack",
        |seen, _| seen.acks().last() == Some(&last),
    );
    assert!(client.seen.acks().len() >= 5, "{:?}", client.seen.acks());
    assert!(
        client
            .seen
            .responses
            .iter()
            .all(|response| matches!(response, MoveResponse::Ack(_))),
        "a new player is never corrected"
    );
    // Consumption debited the live player.
    let depleted = server.stored(key).unwrap().checkpoint.vitals;
    assert!(depleted.food < fresh.food, "{depleted:?} vs {fresh:?}");
    leave(&mut client, &mut server);
    let left = server.stored(key).unwrap().checkpoint;
    assert_eq!(left.location_cm, at.map(f64::from), "saved at the leave");
    assert!(left.vitals.food < fresh.food && left.vitals.water < fresh.water);
    assert!(client.seen.refused.is_none() && !client.seen.closed_by_server);
    let status = server.status();
    let sessions = server.sessions();
    assert_eq!(sessions.initialisation_slots_recorded, 1);
    assert_eq!(sessions.players_finalised, 1);
    assert_eq!(sessions.frozen_movement_releases_sent, 1);
    assert_eq!(sessions.checkpoint_locations_unapplied, Some(0));
    let saved = sessions.checkpoint_saved_locations.unwrap();
    assert_eq!((saved.saved_at_leave, saved.refused), (1, 0));
    assert_eq!(sessions.checkpoint_relocation.unwrap().started, 0);
    let transport = sessions.controller_leave.unwrap();
    assert_eq!(
        (
            transport.intents_accepted,
            transport.completions_admitted,
            transport.completion_prefix_acked
        ),
        (1, 1, 1)
    );
    let begin = controller_field(&status, SERVER_BEGIN_PLAYER_INITIALISATION_FIELD_INDEX);
    assert_eq!(begin["disposition"], "Recognized");
    assert_eq!(begin["count"], 1);
    assert!(movement_row(&status)["count"].as_u64().unwrap() >= u64::from(WALK_MOVES));
    assert_eq!(status["player_leave"]["cutoffs_accepted"], 1);
    assert_eq!(status["player_leave"]["durable"], 1);
    assert_eq!(status["save_failures"], 0);
    let stopped = server.shutdown();
    drop(client);
    assert_eq!(stopped.save_failures, 0);
    assert!(stopped.commits >= 2, "the leave's save and the final save");
    let record = &stopped.record;
    assert_eq!(
        record["sessions"]["closed_by_shutdown"], 0,
        "no session left"
    );
    assert_eq!(record["player_leave"]["detached_settled"], 1);
    assert_eq!(record["player_leave"]["pending"], 0);
    assert_eq!(record["player_leave"]["failed"], 0);
    assert_eq!(
        movement_row(record)["count"],
        movement_row(&status)["count"],
        "pawn_receive survives teardown"
    );
    assert_eq!(
        stopped.players.projection(key).unwrap().checkpoint,
        left,
        "the final save keeps the leave's checkpoint"
    );

    // ---- second process, reading everything from the data directory
    let mut server = Server::start(&inputs.config);
    assert!(server.reopened);
    let stored = server.stored(key).unwrap();
    assert_eq!(stored.checkpoint, left);
    let mut client = enter(&mut server, &inputs, Duration::from_secs(10));
    client.seen.assert_hosted_map();
    assert_eq!(
        client.seen.pawn_location,
        Some(quantize_spawn_vector(SPAWN).unwrap()),
        "the early pawn opens at the spawn point"
    );
    // This client moves only once released, so the pending relocation holds
    // the release for its whole bound and the correction follows it.
    let held = client.seen.between("record", "frozen release");
    assert!(
        held >= RELOCATION_RELEASE_HOLD - Duration::from_millis(250),
        "{held:?}"
    );
    assert_eq!(server.sessions().checkpoint_relocation.unwrap().pending, 1);
    // Stored vitals reach the pawn before its record: the first session's.
    let first_update = client.seen.position("pawn update");
    assert!(first_update < client.seen.position("record"));
    let values = vitals_update(server.lab(), &client.seen.pawn_updates[0]).unwrap();
    assert_eq!(values, with_alive(left.vitals));
    assert_ne!(left.vitals, fresh);
    assert!(client.seen.responses.is_empty(), "no move yet");
    // The first move, from where the pawn opened, is answered by a correction.
    let first = 50.0;
    client.exchange(
        &mut server,
        &[authored_move(first, SPAWN, Some(MOVEMENT_MODE_WALKING))],
    );
    client.until(
        &mut server,
        Duration::from_secs(3),
        "a move response",
        |seen, _| !seen.responses.is_empty(),
    );
    let target = [at[0], at[1], at[2] + CLEARANCE_CM];
    assert_eq!(
        client.seen.responses,
        [MoveResponse::Correction(MoveCorrection {
            timestamp: first,
            location: target,
            velocity: [0.0; 3],
            rotation: None,
            base: None,
            bone: None,
            mode: Some(MOVEMENT_MODE_FALLING),
            base_relative: false,
            root_motion: None,
        })]
    );
    // The client reports a position near the target: applied, then acks.
    let mut timestamp = first;
    for drop_cm in [10.0, 30.0, 50.0] {
        timestamp += MOVE_SECONDS;
        client.exchange(
            &mut server,
            &[authored_move(
                timestamp,
                [target[0], target[1], target[2] - drop_cm],
                Some(MOVEMENT_MODE_FALLING),
            )],
        );
    }
    let newest = timestamp;
    client.until(
        &mut server,
        Duration::from_secs(3),
        "acks after the correction",
        |seen, _| seen.acks().last() == Some(&newest),
    );
    let relocation = server.sessions().checkpoint_relocation.unwrap();
    assert_eq!(
        (relocation.started, relocation.applied, relocation.failed),
        (1, 1, 0)
    );
    assert_eq!(relocation.corrections_sent, 1);
    assert_eq!((relocation.pending, relocation.releases_held), (0, 0));
    // Walking on, the lab's periodic pass (every LOCATION_SAVE_INTERVAL since
    // bind) saves the latest accepted location in the real schedule, before
    // any leave: one the client reported after the correction applied.
    let mut reported: Vec<[f32; 3]> = [10.0, 30.0, 50.0]
        .map(|drop_cm| [target[0], target[1], target[2] - drop_cm])
        .to_vec();
    let mut position = reported[2];
    let deadline = Instant::now() + LOCATION_SAVE_INTERVAL + Duration::from_secs(1);
    while server.sessions().checkpoint_saved_locations.unwrap().saved == 0 {
        assert!(Instant::now() < deadline, "no periodic location save");
        position[0] += 20.0;
        timestamp += MOVE_SECONDS;
        reported.push(position);
        client.exchange(
            &mut server,
            &[authored_move(
                timestamp,
                position,
                Some(MOVEMENT_MODE_WALKING),
            )],
        );
    }
    let periodic = server.stored(key).unwrap().checkpoint;
    assert!(
        reported
            .iter()
            .any(|location| location.map(f64::from) == periodic.location_cm),
        "{periodic:?}"
    );
    assert_eq!(periodic.vitals.max_food, left.vitals.max_food);
    let saved = server.sessions().checkpoint_saved_locations.unwrap();
    assert_eq!(
        (
            saved.saved,
            saved.saved_at_leave,
            saved.refused,
            saved.withheld
        ),
        (1, 0, 0, 0)
    );
    // A few more accepted moves: the teardown flush has a newer one to save.
    for _ in 0..3 {
        position[0] += 20.0;
        timestamp += MOVE_SECONDS;
        client.exchange(
            &mut server,
            &[authored_move(
                timestamp,
                position,
                Some(MOVEMENT_MODE_WALKING),
            )],
        );
    }
    let newest = timestamp;
    client.until(
        &mut server,
        Duration::from_secs(3),
        "the last move's ack",
        |seen, _| seen.acks().last() == Some(&newest),
    );
    let sessions = server.sessions();
    assert_eq!(sessions.checkpoint_locations_unapplied, Some(0));
    assert_eq!(sessions.frozen_movement_releases_sent, 1);
    assert_eq!(sessions.players_finalised, 1);
    // The run loop's periodic autosave ran beside the location saves.
    assert!(server.commits >= 2, "{} commits", server.commits);
    let status = server.status();
    assert_eq!(status["save_failures"], 0);
    movement_row(&status);
    // Shutdown with the player still connected: teardown tells the client
    // and saves its latest location before the final save.
    let stopped = server.shutdown();
    after_shutdown(&mut client);
    assert!(client.seen.closed_by_server, "{:?}", client.seen.log);
    assert!(
        client.seen.refused.is_some(),
        "a host-closed Failure precedes the close"
    );
    drop(client);
    assert_eq!(stopped.save_failures, 0);
    assert_eq!(stopped.record["sessions"]["closed_by_shutdown"], 1);
    assert_eq!(
        stopped.record["sessions"]["checkpoint_saved_locations"]["saved"],
        2
    );
    let last = stopped.players.projection(key).unwrap().checkpoint;
    assert_eq!(last.location_cm, position.map(f64::from));
    // A third start reopens the store once more.
    let server = Server::start(&inputs.config);
    assert!(server.reopened);
    assert_eq!(server.stored(key).unwrap().checkpoint, last);
    server.shutdown();
}

/// Scenario 2: the starter bag reaches the client as slot deltas before its
/// record, decodes to the authored rows and counts, and arrives again after
/// a leave and a restart on the same store.
#[test]
fn rehearsal_bag_projection_survives_leave_and_restart() {
    let inputs = Inputs::new(true);
    let mut server = Server::start(&inputs.config);
    let expected = (starter_bag_literal(), 3 * 5 + 7);
    for process in 0..2 {
        let mut client = enter(&mut server, &inputs, Duration::from_secs(5));
        let connection = server.lab().peers[&client.address].connection_id.unwrap();
        let seen = &client.seen;
        let record = seen.position("record");
        let before: Vec<Payload> = seen
            .log
            .iter()
            .take(record)
            .filter(|event| *event == "pawn update")
            .zip(&seen.pawn_updates)
            .map(|(_, payload)| payload.clone())
            .collect();
        assert_eq!(
            projected_bag(server.lab(), connection, &before),
            expected,
            "process {process}: the bag precedes the record"
        );
        assert_eq!(
            projected_bag(server.lab(), connection, &seen.pawn_updates),
            expected,
            "process {process}: nothing changes it later"
        );
        let projection = server.sessions().player_bag_projection.unwrap();
        assert!(projection.updates_queued >= 1, "{projection:?}");
        leave(&mut client, &mut server);
        let stopped = server.shutdown();
        drop(client);
        assert_eq!(stopped.save_failures, 0);
        assert_eq!(stopped.record["player_leave"]["detached_settled"], 1);
        server = Server::start(&inputs.config);
        assert!(server.reopened);
    }
    server.shutdown();
}

/// Scenario 3: a client that never sends Begin. It gets the early pawn and
/// ClientRestart, then goes silent; the idle bound removes it with no
/// reservation or saved location; shutdown is clean and the store reopens.
#[test]
fn rehearsal_a_client_without_begin_times_out_with_nothing_stored() {
    let inputs = Inputs::new(false);
    let mut server = Server::start(&inputs.config);
    let mut client = StockClient::connect(&mut server, &inputs, ACCOUNT, false);
    client.until(
        &mut server,
        Duration::from_secs(5),
        "ClientRestart",
        |seen, _| seen.restart.is_some(),
    );
    assert_eq!(client.seen.restart, client.seen.pawn);
    assert!(!client.seen.begin_sent);
    let address = client.address;
    assert!(server.lab().peers[&address].checkpoint_epoch.is_none());
    // Silence: nothing more is sent, not even an ACK.
    let silent = Instant::now();
    wait_lab(
        &mut server,
        SESSION_TIMEOUT + Duration::from_secs(2),
        "the idle removal",
        |lab| !lab.peers.contains_key(&address),
    );
    assert!(silent.elapsed() >= SESSION_TIMEOUT - Duration::from_millis(100));
    let sessions = server.sessions();
    assert_eq!(
        sessions.closed_by_timeout + sessions.closed_by_delivery_stall,
        1
    );
    assert_eq!(sessions.initialisation_slots_recorded, 0);
    assert_eq!(sessions.players_finalised, 0);
    assert_eq!(sessions.checkpoint_locations_unapplied, Some(0));
    assert_eq!(
        sessions.checkpoint_saved_locations.unwrap(),
        crate::experimental::peer_lab::SavedLocationCounts::default()
    );
    assert_eq!(server.lab().next_checkpoint_epoch, 1);
    assert!(server.registry().active_epochs().is_empty());
    assert!(server.registry().snapshot().is_empty());
    let status = server.status();
    assert!(
        status["controller_receive"]["fields"]["entries"]
            .as_array()
            .unwrap()
            .iter()
            .all(|entry| entry["index"] != SERVER_BEGIN_PLAYER_INITIALISATION_FIELD_INDEX),
        "no Begin received"
    );
    let stopped = server.shutdown();
    drop(client);
    assert_eq!(stopped.save_failures, 0);
    assert!(stopped.players.is_empty());
    assert_eq!(stopped.record["sessions"]["closed_by_shutdown"], 0);
    let server = Server::start(&inputs.config);
    assert!(server.reopened);
    assert!(server.registry().snapshot().is_empty());
    server.shutdown();
}

/// Scenario 3 with a client that keeps its connection alive: it ACKs every
/// server packet, as a client parked on a loading screen may (F-1199 saw a
/// stalled stock client stay connected until its run's own bound), but never
/// sends Begin. The idle timeout needs a silent client, the delivery bound a
/// lack of ACK progress, and the friends-query deadline starts only after
/// Begin, so only `network.begin_timeout_seconds` bounds this wait for
/// identity (without it the session holds its slot, early pawn and spawn
/// point: [`rehearsal_without_a_begin_timeout_a_keepalive_client_stays_joined`]).
/// It is closed and removed past the limit; nothing is reserved or stored,
/// and shutdown and reopening are clean.
#[test]
fn rehearsal_a_keepalive_client_without_begin_is_timed_out() {
    let inputs = Inputs::with_begin_timeout(false, Some(BEGIN_TIMEOUT));
    let mut server = Server::start(&inputs.config);
    assert_eq!(server.lab().begin_timeout(), Some(BEGIN_TIMEOUT));
    let mut client = keepalive_client(&mut server, &inputs);
    let address = client.address;
    let start = Instant::now();
    while server.lab().peers.contains_key(&address)
        && start.elapsed() < BEGIN_TIMEOUT + SESSION_TIMEOUT
    {
        client.exchange(&mut server, &[]);
    }
    assert!(
        !server.lab().peers.contains_key(&address),
        "still joined after {:?} of ACKs without Begin: {}",
        start.elapsed(),
        serde_json::json!(server.sessions())
    );
    // Measured from Join, which the client sent on Welcome.
    assert!(client.seen.at[client.seen.position("welcome")].elapsed() >= BEGIN_TIMEOUT);
    // The client is told: one more exchange reads the close if it is still
    // in flight.
    client.exchange(&mut server, &[]);
    assert!(client.seen.closed_by_server, "a control close");
    assert!(client.seen.refused.is_none());
    assert!(!client.seen.begin_sent);
    let sessions = server.sessions();
    assert_eq!(sessions.closed_awaiting_begin, Some(1));
    assert_eq!(
        (
            sessions.closed_by_timeout,
            sessions.closed_by_delivery_stall,
            sessions.closed_by_friends_query_timeout,
            sessions.closed_failed,
        ),
        (0, 0, 0, 0)
    );
    assert_eq!(sessions.joined, 0);
    assert_eq!(sessions.initialisation_slots_recorded, 0);
    assert_eq!(server.lab().next_checkpoint_epoch, 1, "nothing reserved");
    assert!(server.registry().active_epochs().is_empty());
    assert!(server.registry().snapshot().is_empty(), "nothing stored");
    let stopped = server.shutdown();
    drop(client);
    assert_eq!(stopped.save_failures, 0);
    assert!(stopped.players.is_empty());
    assert_eq!(stopped.record["sessions"]["closed_by_shutdown"], 0);
    assert_eq!(stopped.record["sessions"]["closed_awaiting_begin"], 1);
    let server = Server::start(&inputs.config);
    assert!(server.reopened);
    assert!(server.registry().snapshot().is_empty());
    server.shutdown();
}

/// The default: with `network.begin_timeout_seconds` absent, the same
/// keepalive client is still joined after the same interval, holding its
/// slot and early pawn, and the session counts carry no
/// `closed_awaiting_begin`. The option is what removes it.
#[test]
fn rehearsal_without_a_begin_timeout_a_keepalive_client_stays_joined() {
    let inputs = Inputs::new(false);
    let mut server = Server::start(&inputs.config);
    assert_eq!(server.lab().begin_timeout(), None);
    let mut client = keepalive_client(&mut server, &inputs);
    let address = client.address;
    let start = Instant::now();
    while start.elapsed() < BEGIN_TIMEOUT + SESSION_TIMEOUT {
        client.exchange(&mut server, &[]);
    }
    let entry = &server.lab().peers[&address];
    assert_eq!(entry.admission.state, ClientLoginState::Joined);
    assert!(entry.peer.has_actor_channel(PAWN_CHANNEL_INDEX));
    assert!(!client.seen.closed_by_server);
    let sessions = server.sessions();
    assert_eq!(sessions.joined, 1);
    assert_eq!(sessions.closed_awaiting_begin, None);
    assert!(
        !serde_json::to_string(&sessions)
            .unwrap()
            .contains("closed_awaiting_begin")
    );
    assert_eq!(server.lab().next_checkpoint_epoch, 1, "nothing reserved");
    assert!(server.registry().snapshot().is_empty(), "nothing stored");
    let stopped = server.shutdown();
    drop(client);
    assert_eq!(stopped.save_failures, 0);
    assert_eq!(stopped.record["sessions"]["closed_by_shutdown"], 1);
    assert!(
        stopped.record["sessions"]
            .get("closed_awaiting_begin")
            .is_none()
    );
}

/// A client that enters as far as ClientRestart (the early pawn's) and never
/// sends Begin.
fn keepalive_client(server: &mut Server, inputs: &Inputs) -> StockClient {
    let mut client = StockClient::connect(server, inputs, ACCOUNT, false);
    client.until(
        server,
        Duration::from_secs(5),
        "ClientRestart",
        |seen, _| seen.restart.is_some(),
    );
    assert_eq!(client.seen.restart, client.seen.pawn);
    client
}

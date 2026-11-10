//! Session churn retention: an authored loopback client drives the full
//! session path the native server supports (handshake, Login, Join, initial
//! opens, Begin, profile, loadout and bestiary answers, the friends query and
//! its answer, finalisation, the connected-player record and its ACK) through
//! many sequential or concurrent sessions, ended cleanly, by idle expiry, by a
//! reconnect from the same address or early. After each checkpoint every
//! driver-wide structure that can outlive a session is compared with its size
//! after warm-up. The client shares this repository's model, so passing shows
//! the native lifecycle releases what a session held, not client
//! compatibility. Idle expiry injects the lab's `now`; nothing sleeps for a
//! timeout.

use std::{
    net::{SocketAddr, SocketAddrV4, UdpSocket},
    sync::{Arc, Barrier, mpsc},
    thread,
    time::{Duration, Instant},
};

use bevy::app::App;

use super::{PeerEntry, PeerLab, PeerLabStats};
use crate::{
    config::{NetworkConfig, ServerConfig},
    experimental::{
        control_messages::{ArchiveText, ControlCodec, ControlMessage},
        packet::{PacketConfig, Receiver},
        probe_client::{ProbeClient, ProbeConfig},
        probe_initialise::{InitialiseConfig, PlayerInitialiser, player_unique_net_id},
        release::ReleaseProfile,
    },
    network,
    simulation::{WorldSnapshot, build_app},
};

/// Authored identity and slot; never derived from a save or a client.
const PLAYER: &str = "churn-retention-player";
const SLOT: i32 = 3;
/// Every bounded wait of one session.
const STEP: Duration = Duration::from_secs(10);
/// One receive window of the pump loop; the loop repeats until its condition.
const WINDOW: Duration = Duration::from_micros(500);

fn release_toml() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("config/release.toml")
}

fn network_config() -> NetworkConfig {
    NetworkConfig {
        enabled: true,
        port: 0,
        release_profile: Some(release_toml()),
        ..NetworkConfig::default()
    }
}

/// Sizes of the `PeerLab` and GUID-cache structures that can outlive a
/// session. Per-session structures must return exactly to their warm-up
/// size; configuration-sized ones (statics, manager and quest opens) stay
/// constant.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Retained {
    peers: usize,
    guid_keyed: usize,
    guid_statics: usize,
    guid_incarnations: usize,
    restored_characters: usize,
    uid_claimed: usize,
    uid_reservations: usize,
    uid_remaps: usize,
    desired_incarnations: usize,
    highest_incarnations: usize,
    manager_opens: usize,
    quest_opens: usize,
    member_refresh_members: usize,
    networking_players: usize,
}

/// Monotonic counters: no stored state per session, bounded by their range.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Counters {
    dynamic_guids: u32,
    next_record: u32,
    next_connection: u32,
    next_player_id: i32,
    next_pawn_object_id: Option<i32>,
}

/// Per-live-peer structure sizes (summed over live peers).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct PeerRetained {
    exports: usize,
    record_receipts: usize,
    records_sent: usize,
    open_receipts: usize,
    property_packets: usize,
    opened_actors: usize,
    pending_closes: usize,
    retiring: usize,
    closing_legacy: usize,
    restored_modifiers: usize,
    sent_records: usize,
}

#[derive(Clone, Debug)]
struct Snapshot {
    retained: Retained,
    counters: Counters,
    live: PeerRetained,
    stats: PeerLabStats,
}

fn peer_retained(entry: &PeerEntry) -> PeerRetained {
    PeerRetained {
        exports: entry.exports.tracked_count(),
        record_receipts: entry.connected_player_receipts.len(),
        records_sent: entry.connected_players_sent.len(),
        open_receipts: entry.initial_open_outcomes.awaiting_ack.len(),
        property_packets: entry.actor_property_packets.len(),
        opened_actors: entry.opened_actors.len(),
        pending_closes: entry.pending_closes.len(),
        retiring: entry.retiring_incarnations.len(),
        closing_legacy: entry.closing_legacy_actors.len(),
        restored_modifiers: entry.restored_modifiers.len(),
        sent_records: entry.peer.sequencer().notify().sent_records().len(),
    }
}

fn snapshot(lab: &PeerLab) -> Snapshot {
    let uids = lab.actor_uids.as_ref();
    let session = lab.game_state_open.as_ref();
    let mut live = PeerRetained::default();
    for entry in lab.peers.values() {
        let one = peer_retained(entry);
        live.exports += one.exports;
        live.record_receipts += one.record_receipts;
        live.records_sent += one.records_sent;
        live.open_receipts += one.open_receipts;
        live.property_packets += one.property_packets;
        live.opened_actors += one.opened_actors;
        live.pending_closes += one.pending_closes;
        live.retiring += one.retiring;
        live.closing_legacy += one.closing_legacy;
        live.restored_modifiers += one.restored_modifiers;
        live.sent_records += one.sent_records;
    }
    Snapshot {
        retained: Retained {
            peers: lab.peers.len(),
            guid_keyed: lab.guid_cache.keyed_count(),
            guid_statics: lab.guid_cache.static_count(),
            guid_incarnations: lab.guid_cache.incarnation_count(),
            restored_characters: lab.restored_characters.len(),
            uid_claimed: uids.map_or(0, |uids| uids.claimed().len()),
            uid_reservations: uids.map_or(0, |uids| uids.reservations().len()),
            uid_remaps: uids.map_or(0, |uids| uids.remaps().len()),
            desired_incarnations: lab.desired_incarnations.len(),
            highest_incarnations: lab.highest_incarnations.len(),
            manager_opens: lab.manager_opens.len(),
            quest_opens: lab.quest_opens.len(),
            member_refresh_members: session
                .and_then(|session| session.member_refresh.as_ref())
                .map_or(0, |prospect| prospect.associated_members.len()),
            networking_players: session
                .and_then(|session| session.networking.as_ref())
                .map_or(0, |status| status.player_names.len()),
        },
        counters: Counters {
            dynamic_guids: lab.guid_cache.dynamic_guids_assigned(),
            next_record: lab.next_connected_player_record,
            next_connection: lab.next_connection_id,
            next_player_id: lab.next_player_id,
            next_pawn_object_id: lab.next_pawn_object_id,
        },
        live,
        stats: lab.stats.clone(),
    }
}

/// What the server loop hosts: a bare lab polled directly, or the
/// production app whose plugin polls it.
trait Host {
    fn step(&mut self);
    fn lab(&self) -> &PeerLab;
    fn lab_mut(&mut self) -> &mut PeerLab;
}

impl Host for PeerLab {
    fn step(&mut self) {
        self.poll();
    }
    fn lab(&self) -> &PeerLab {
        self
    }
    fn lab_mut(&mut self) -> &mut PeerLab {
        self
    }
}

impl Host for App {
    fn step(&mut self) {
        self.update();
    }
    fn lab(&self) -> &PeerLab {
        self.world().resource::<PeerLab>()
    }
    fn lab_mut(&mut self) -> &mut PeerLab {
        self.world_mut().resource_mut::<PeerLab>().into_inner()
    }
}

/// One live peer's progress as the server sees it.
#[derive(Clone, Copy, Debug, Default)]
struct Progress {
    completed: bool,
    finalised: bool,
    record_settled: bool,
}

enum Request {
    Progress(SocketAddrV4, mpsc::Sender<Option<Progress>>),
    /// Reply once the address has no peer.
    Gone(SocketAddrV4, mpsc::Sender<()>),
    /// Inject a lab time past the idle timeout; expires every idle peer.
    Expire(mpsc::Sender<()>),
    /// Reply once exactly `live` peers remain, with a snapshot.
    Snapshot(usize, mpsc::Sender<Snapshot>),
}

/// The client side's handle on the server loop.
#[derive(Clone)]
struct Link(mpsc::Sender<Request>);

impl Link {
    fn ask<T>(&self, request: impl FnOnce(mpsc::Sender<T>) -> Request) -> T {
        let (reply, answer) = mpsc::channel();
        self.0.send(request(reply)).expect("server loop alive");
        answer.recv_timeout(STEP * 6).expect("server loop answered")
    }
    fn progress(&self, address: SocketAddrV4) -> Option<Progress> {
        self.ask(|reply| Request::Progress(address, reply))
    }
    fn gone(&self, address: SocketAddrV4) {
        self.ask(|reply| Request::Gone(address, reply));
    }
    fn expire(&self) {
        self.ask(Request::Expire);
    }
    fn snapshot(&self, live: usize) -> Snapshot {
        self.ask(|reply| Request::Snapshot(live, reply))
    }
}

/// Run `client` on a worker thread while this thread steps `host` and
/// answers its requests.
fn serve<H: Host, T: Send + 'static>(
    host: &mut H,
    client: impl FnOnce(Link) -> T + Send + 'static,
) -> T {
    let (requests, incoming) = mpsc::channel();
    let (done, result) = mpsc::channel();
    let link = Link(requests);
    let worker = thread::spawn(move || {
        let value = client(link);
        let _ = done.send(value);
    });
    let deadline = Instant::now() + Duration::from_secs(1_800);
    let mut gone: Vec<(SocketAddrV4, mpsc::Sender<()>)> = Vec::new();
    let mut snapshots: Vec<(usize, mpsc::Sender<Snapshot>)> = Vec::new();
    loop {
        host.step();
        while let Ok(request) = incoming.try_recv() {
            match request {
                Request::Progress(address, reply) => {
                    let progress = host.lab().peers.get(&address).map(|entry| Progress {
                        completed: entry.profiles.completed,
                        finalised: entry.profiles.finalised,
                        record_settled: entry.connected_player_creations_settled > 0,
                    });
                    let _ = reply.send(progress);
                }
                Request::Gone(address, reply) => gone.push((address, reply)),
                Request::Expire(reply) => {
                    let lab = host.lab_mut();
                    let later = Instant::now() + lab.config.idle_timeout + Duration::from_secs(1);
                    lab.expire_idle(later);
                    let _ = reply.send(());
                }
                Request::Snapshot(live, reply) => snapshots.push((live, reply)),
            }
        }
        gone.retain(|(address, reply)| {
            if host.lab().has_peer(*address) {
                return true;
            }
            let _ = reply.send(());
            false
        });
        snapshots.retain(|(live, reply)| {
            if host.lab().peer_count() != *live {
                return true;
            }
            let _ = reply.send(snapshot(host.lab()));
            false
        });
        if let Ok(value) = result.try_recv() {
            worker.join().expect("client worker");
            return value;
        }
        if worker.is_finished() {
            if let Ok(value) = result.try_recv() {
                worker.join().expect("client worker");
                return value;
            }
            // Panicked before sending: surface the panic.
            if let Err(panic) = worker.join() {
                std::panic::resume_unwind(panic);
            }
            panic!("client worker ended without a result");
        }
        assert!(Instant::now() < deadline, "churn did not finish");
        thread::sleep(Duration::from_micros(50));
    }
}

fn probe_config() -> ProbeConfig {
    ProbeConfig {
        packet: PacketConfig {
            magic_bits: 0,
            receiver: Receiver::Server,
            limits: super::PeerLabConfig::default().packet.packet.limits,
        },
        codec: ControlCodec::default(),
        handshake_timeout: Duration::from_secs(2),
        handshake_attempts: 3,
        reply_window: STEP,
        max_datagrams: 1,
    }
}

/// Discard what the server sent to `socket` until it stays quiet for a few
/// milliseconds, so a late reply to an earlier attempt cannot be read as the
/// next handshake reply (the old session's own traffic is far sparser).
fn drain(socket: &UdpSocket) {
    let mut buffer = [0; 2048];
    socket
        .set_read_timeout(Some(Duration::from_millis(5)))
        .expect("timeout");
    while socket.recv(&mut buffer).is_ok() {}
}

/// The stateless handshake on `socket`, retried when an old session's
/// datagram arrives where a handshake reply was expected.
fn connect(socket: &UdpSocket, reused: bool) -> ProbeClient {
    let mut errors = Vec::new();
    for attempt in 0..8 {
        if reused || attempt > 0 {
            drain(socket);
        }
        let clone = socket.try_clone().expect("socket clone");
        match ProbeClient::connect(clone, probe_config()) {
            Ok(client) => return client,
            Err(error) => errors.push(format!("{error:#}")),
        }
    }
    panic!("no handshake: {errors:?}");
}

/// Where a session stops.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Ending {
    /// Full path, record ACKed, then a client close.
    Clean,
    /// Full path, then silence until the lab's idle expiry.
    Idle,
    /// Full path, then a new handshake from the same address.
    Reconnect,
    /// Close right after the Join's acknowledgement.
    AfterJoin,
    /// Close right after sending Begin.
    AfterBegin,
    /// Profiles, loadout and bestiary complete; the friends query is never
    /// answered, so the player cannot finalise; then close.
    AfterProfiles,
}

struct Session {
    client: ProbeClient,
    initialiser: PlayerInitialiser,
    address: SocketAddrV4,
}

impl Session {
    /// One receive window: answer what arrived, then ACK it.
    fn service(&mut self) {
        let collected = self.client.collect(WINDOW, 64).expect("collect");
        for reply in &collected.replies {
            self.initialiser
                .on_reply(&mut self.client, reply)
                .expect("initialiser");
        }
        self.client.keepalive().expect("keepalive");
    }

    /// Receive, answer and ACK until `done` holds, also servicing `others`
    /// (live sessions that must keep answering requeries).
    fn pump_with(
        &mut self,
        link: &Link,
        others: &mut [Session],
        mut done: impl FnMut(&Self, Option<Progress>) -> bool,
    ) {
        let deadline = Instant::now() + STEP;
        loop {
            self.service();
            for other in others.iter_mut() {
                other.service();
            }
            if done(self, link.progress(self.address)) {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "session stalled: {:?} {:?} {:?}",
                self.initialiser.summary(),
                self.initialiser.notes(),
                link.progress(self.address)
            );
        }
    }

    fn pump(&mut self, link: &Link, done: impl FnMut(&Self, Option<Progress>) -> bool) {
        self.pump_with(link, &mut [], done);
    }
}

/// Handshake, Hello, Login (Welcome), then Netspeed and Join, whose
/// acknowledgement is awaited. Replies after the Join feed the initialiser
/// unless the session ends right after Join.
fn join(socket: &UdpSocket, version: u32, engine: u32, ending: Ending) -> Session {
    let SocketAddr::V4(address) = socket.local_addr().expect("address") else {
        unreachable!("IPv4 loopback")
    };
    let mut client = connect(socket, ending == Ending::Reconnect);
    let mut initialiser = PlayerInitialiser::new(
        &client,
        InitialiseConfig {
            chr_slot: SLOT,
            engine_net_version: engine,
        },
    );
    if ending == Ending::AfterProfiles {
        initialiser.withhold_friends_list();
    }
    let report = client.hello(version, "").expect("hello");
    assert!(matches!(
        report.messages().collect::<Vec<_>>().as_slice(),
        [ControlMessage::Challenge { .. }]
    ));
    client
        .send_control(
            &[ControlMessage::Login {
                response: ArchiveText::text("0").expect("text"),
                url: ArchiveText::text("?Name=ChurnRetention").expect("text"),
                unique_net_id: player_unique_net_id(PLAYER).expect("id"),
                platform: ArchiveText::text("").expect("text"),
            }],
            false,
        )
        .expect("login");
    let welcome = client.collect(STEP, 1).expect("welcome");
    assert!(matches!(
        welcome.messages().collect::<Vec<_>>().as_slice(),
        [ControlMessage::Welcome { .. }]
    ));
    let (_, join) = client
        .send_control(
            &[
                ControlMessage::Netspeed { rate: 100_000 },
                ControlMessage::Join,
            ],
            false,
        )
        .expect("join");
    let deadline = Instant::now() + STEP;
    loop {
        let collected = client.collect(WINDOW, 64).expect("collect");
        if ending != Ending::AfterJoin {
            for reply in &collected.replies {
                initialiser
                    .on_reply(&mut client, reply)
                    .expect("initialiser");
            }
        }
        if collected.acked.contains(&join) {
            break;
        }
        assert!(Instant::now() < deadline, "Join not acknowledged");
    }
    Session {
        client,
        initialiser,
        address,
    }
}

/// One session from a fresh handshake to `ending`. Returns the open client
/// only for [`Ending::Reconnect`], whose replacement ends it.
fn session(
    link: &Link,
    socket: &UdpSocket,
    version: u32,
    engine: u32,
    ending: Ending,
    others: &mut [Session],
) -> Option<Session> {
    let mut session = join(socket, version, engine, ending);
    match ending {
        Ending::AfterJoin => {}
        Ending::AfterBegin => session.pump_with(link, others, |session, _| {
            session.initialiser.summary().begin_sent
        }),
        Ending::AfterProfiles => session.pump_with(link, others, |session, progress| {
            session.initialiser.is_complete() && progress.is_some_and(|p| p.completed)
        }),
        Ending::Clean | Ending::Idle | Ending::Reconnect => {
            session.pump_with(link, others, |_, progress| {
                progress.is_some_and(|p| p.finalised && p.record_settled)
            });
        }
    }
    let address = session.address;
    match ending {
        Ending::Reconnect => return Some(session),
        Ending::Idle => {
            drop(session);
            link.expire();
        }
        _ => {
            session.client.close().expect("close");
        }
    }
    link.gone(address);
    None
}

/// Sessions before the baseline snapshot (per worker when concurrent).
const WARMUP: u64 = 2;

/// Cycles per run; `ICARUS_CHURN_CYCLES` overrides it for measurements.
fn cycles() -> u64 {
    std::env::var("ICARUS_CHURN_CYCLES")
        .ok()
        .and_then(|value| value.parse().ok())
        .filter(|cycles| *cycles >= WARMUP)
        .unwrap_or(1_000)
}

fn release() -> (u32, u32) {
    let profile = ReleaseProfile::load(&release_toml()).expect("release");
    (profile.pin.network_checksum, profile.engine_network_version)
}

#[derive(Debug)]
struct Run {
    label: String,
    /// `(sessions completed, snapshot)`, the first after warm-up.
    checkpoints: Vec<(u64, Snapshot)>,
    wall: Duration,
}

impl Run {
    fn report(&self) {
        for (sessions, snapshot) in &self.checkpoints {
            eprintln!(
                "churn {} k={sessions}: {:?} {:?} live={:?} wall={:?}",
                self.label, snapshot.retained, snapshot.counters, snapshot.live, self.wall
            );
        }
    }

    fn baseline(&self) -> &Snapshot {
        &self.checkpoints.first().expect("warm-up snapshot").1
    }

    fn last(&self) -> &Snapshot {
        &self.checkpoints.last().expect("final snapshot").1
    }

    /// Every driver-wide structure is back at its warm-up size.
    fn assert_retained(&self) {
        self.report();
        let baseline = &self.baseline().retained;
        for (sessions, snapshot) in &self.checkpoints {
            assert_eq!(
                &snapshot.retained, baseline,
                "{} retained after {sessions} sessions",
                self.label
            );
        }
    }
}

fn checkpoints(cycles: u64) -> Vec<u64> {
    let mut points: Vec<u64> = [WARMUP, 10, 100, 1_000]
        .into_iter()
        .filter(|point| *point <= cycles)
        .collect();
    if points.last() != Some(&cycles) {
        points.push(cycles);
    }
    points
}

fn server_socket(server: SocketAddr) -> UdpSocket {
    let socket = UdpSocket::bind("127.0.0.1:0").expect("bind");
    socket.connect(server).expect("connect");
    socket
}

/// `cycles` sequential sessions ending as `ending`. A reconnect reuses one
/// address, each session replacing the previous one; the others each use a
/// fresh address.
fn sequential<H: Host>(host: &mut H, ending: Ending, cycles: u64) -> Run {
    let server = host.lab().local_addr().expect("server address");
    let (version, engine) = release();
    let points = checkpoints(cycles);
    let started = Instant::now();
    let checkpoints = serve(host, move |link| {
        let mut snapshots = Vec::new();
        let shared = server_socket(server);
        let mut open: Option<Session> = None;
        for cycle in 1..=cycles {
            let fresh;
            let socket = if ending == Ending::Reconnect {
                &shared
            } else {
                fresh = server_socket(server);
                &fresh
            };
            let next = session(&link, socket, version, engine, ending, &mut []);
            // The replacement already removed the previous session.
            drop(open.take());
            open = next;
            if points.contains(&cycle) {
                snapshots.push((cycle, link.snapshot(usize::from(open.is_some()))));
            }
        }
        if let Some(last) = open.take() {
            let address = last.address;
            last.client.close().expect("close");
            link.gone(address);
        }
        snapshots
    });
    Run {
        label: format!("{ending:?}"),
        checkpoints,
        wall: started.elapsed(),
    }
}

/// `workers` concurrent clients, each running `rounds` clean full-path
/// sessions; snapshots after warm-up rounds and at checkpoint rounds, when
/// every worker has closed its session.
fn concurrent<H: Host>(host: &mut H, workers: usize, rounds: u64) -> Run {
    let server = host.lab().local_addr().expect("server address");
    let (version, engine) = release();
    let points: Vec<u64> = [WARMUP, 13, rounds]
        .into_iter()
        .filter(|round| *round <= rounds)
        .collect();
    let started = Instant::now();
    let checkpoints = serve(host, move |link| {
        let barrier = Arc::new(Barrier::new(workers));
        let snapshots = Arc::new(std::sync::Mutex::new(Vec::new()));
        thread::scope(|scope| {
            for _ in 0..workers {
                let (link, barrier, snapshots, points) =
                    (link.clone(), barrier.clone(), snapshots.clone(), &points);
                scope.spawn(move || {
                    for round in 1..=rounds {
                        let socket = server_socket(server);
                        session(&link, &socket, version, engine, Ending::Clean, &mut []);
                        if points.contains(&round) {
                            if barrier.wait().is_leader() {
                                let snapshot = link.snapshot(0);
                                snapshots
                                    .lock()
                                    .expect("snapshots")
                                    .push((round * workers as u64, snapshot));
                            }
                            barrier.wait();
                        }
                    }
                });
            }
        });
        Arc::try_unwrap(snapshots)
            .expect("workers joined")
            .into_inner()
            .expect("snapshots")
    });
    Run {
        label: format!("Concurrent{workers}"),
        checkpoints,
        wall: started.elapsed(),
    }
}

/// One finalised observer stays connected and answers every requery while
/// `cycles` clean sessions come and go beside it.
fn observed<H: Host>(host: &mut H, cycles: u64) -> Run {
    let server = host.lab().local_addr().expect("server address");
    let (version, engine) = release();
    let points = checkpoints(cycles);
    let started = Instant::now();
    let checkpoints = serve(host, move |link| {
        let observer_socket = server_socket(server);
        let mut observer = join(&observer_socket, version, engine, Ending::Clean);
        observer.pump(&link, |_, progress| {
            progress.is_some_and(|p| p.finalised && p.record_settled)
        });
        let mut observers = [observer];
        let mut snapshots = Vec::new();
        for cycle in 1..=cycles {
            let socket = server_socket(server);
            session(
                &link,
                &socket,
                version,
                engine,
                Ending::Clean,
                &mut observers,
            );
            if points.contains(&cycle) {
                // Let the observer ACK what the departure sent it.
                for _ in 0..4 {
                    observers[0].service();
                }
                snapshots.push((cycle, link.snapshot(1)));
            }
        }
        let [observer] = observers;
        let address = observer.address;
        observer.client.close().expect("close");
        link.gone(address);
        snapshots
    });
    Run {
        label: "Observed".to_owned(),
        checkpoints,
        wall: started.elapsed(),
    }
}

fn lab() -> PeerLab {
    network::bind(&network_config())
        .expect("bind")
        .expect("enabled")
        .lab
}

fn production_app() -> App {
    let config = ServerConfig {
        network: network_config(),
        ..ServerConfig::default()
    };
    let mut app = build_app(&config, &WorldSnapshot::default()).expect("app");
    let bound = network::bind(&config.network)
        .expect("bind")
        .expect("enabled");
    network::install(&mut app, bound.lab).expect("install");
    app
}

fn lab_ending(ending: Ending) -> Run {
    let cycles = cycles();
    let mut lab = lab();
    let run = sequential(&mut lab, ending, cycles);
    run.assert_retained();
    let stats = &run.last().stats;
    match ending {
        Ending::Clean | Ending::Idle | Ending::Reconnect => {
            assert_eq!(stats.players_finalised, cycles);
            assert_eq!(stats.connected_player_record_creations_acked, cycles);
        }
        Ending::AfterJoin => assert_eq!(stats.players_finalised, 0),
        Ending::AfterBegin | Ending::AfterProfiles => {
            assert_eq!(stats.players_finalised, 0);
            assert!(stats.friends_list_queries_queued >= cycles);
        }
    }
    if ending == Ending::Idle {
        assert_eq!(stats.peers_expired, cycles);
    }
    run
}

#[test]
fn full_path_clean_close_churn_retains_no_session_state() {
    lab_ending(Ending::Clean);
}

#[test]
fn full_path_idle_expiry_churn_retains_no_session_state() {
    lab_ending(Ending::Idle);
}

#[test]
fn full_path_reconnect_churn_retains_no_session_state() {
    lab_ending(Ending::Reconnect);
}

#[test]
fn leave_after_join_churn_retains_no_session_state() {
    lab_ending(Ending::AfterJoin);
}

#[test]
fn leave_after_begin_churn_retains_no_session_state() {
    lab_ending(Ending::AfterBegin);
}

#[test]
fn leave_after_profiles_churn_retains_no_session_state() {
    lab_ending(Ending::AfterProfiles);
}

#[test]
fn eight_concurrent_peers_churn_retains_no_session_state() {
    let workers = 8;
    // A small override still needs a warm-up round to compare against.
    let rounds = cycles().div_ceil(workers as u64).max(WARMUP);
    let mut lab = lab();
    let run = concurrent(&mut lab, workers, rounds);
    run.assert_retained();
    assert_eq!(
        run.last().stats.connected_player_record_creations_acked,
        rounds * workers as u64
    );
    assert_eq!(run.last().stats.peers_rejected, 0);
}

#[test]
fn a_live_observer_retains_nothing_per_departed_session() {
    let cycles = cycles();
    let mut lab = lab();
    let run = observed(&mut lab, cycles);
    run.assert_retained();
    let baseline = run.baseline().live.clone();
    for (sessions, snapshot) in &run.checkpoints {
        let live = PeerRetained {
            sent_records: baseline.sent_records,
            ..snapshot.live.clone()
        };
        assert_eq!(live, baseline, "observer after {sessions} sessions");
    }
}

/// The production app (plugin polling inside `App::update`) at a smaller
/// default count, `ICARUS_CHURN_APP_CYCLES` overriding it; each ending runs
/// on its own app.
#[test]
fn production_app_full_path_churn_retains_no_session_state() {
    let cycles = std::env::var("ICARUS_CHURN_APP_CYCLES")
        .ok()
        .and_then(|value| value.parse().ok())
        .filter(|cycles| *cycles >= WARMUP)
        .unwrap_or(200);
    for ending in [Ending::Clean, Ending::Idle, Ending::Reconnect] {
        let mut app = production_app();
        let run = sequential(&mut app, ending, cycles);
        run.assert_retained();
        let stats = &run.last().stats;
        assert_eq!(stats.connected_player_record_creations_acked, cycles);
        assert_eq!(
            stats.connected_player_records_removed + u64::from(ending == Ending::Reconnect),
            cycles
        );
        let counts = network::session_counts(app.world()).expect("installed");
        assert_eq!(counts.active_peers, 0);
    }
}

//! The opt-in experimental `[network]` session layer of `icarus-server run`,
//! driven only by this repository's authored probe on IPv4 loopback. Both
//! sides share one model, so passing shows agreement with that model and the
//! native lifecycle, not compatibility with any ICARUS client.
use std::{
    fs,
    io::{BufRead, BufReader},
    net::{SocketAddr, SocketAddrV4, UdpSocket},
    process::{Child, Command, Stdio},
    sync::mpsc::{self, Receiver},
    thread,
    time::{Duration, Instant},
};

use bevy::{
    app::App,
    prelude::{FixedUpdate, IntoScheduleConfigs, Res, ResMut, Resource},
    time::TimeUpdateStrategy,
};
use icarus_bevy::{
    config::{NetworkConfig, ServerConfig},
    experimental::{
        admission::SERVER_FULL,
        control_messages::{
            ArchiveText, ControlCodec, ControlMessage, NetIdType, NetIdValue, UniqueNetId,
        },
        handshake::initial_request,
        packet::{PacketConfig, Receiver as PacketReceiver},
        peer_lab::{PeerLab, PeerLabConfig, SHUTTING_DOWN_MESSAGE, SessionCounts},
        probe_client::{ProbeClient, ProbeConfig},
        release::ReleaseProfile,
        session_churn::{ChurnConfig, run_churn, session_cycle},
    },
    network,
    simulation::{SimulationSet, WorldSnapshot, build_app},
};
use serde_json::Value;

mod pak_writer;
#[path = "common/profiles.rs"]
mod profiles;
#[path = "common/c001_scalar_owners.rs"]
mod scalar;

/// Synthetic identity material; asserted never to appear in any record.
const NAME_SENTINEL: &str = "SENTINEL-NAME-5b1e";
const NET_ID_HEX: &str = "0123456789ABCDEF7654";

fn probe_config() -> ProbeConfig {
    ProbeConfig {
        packet: PacketConfig {
            magic_bits: 0,
            receiver: PacketReceiver::Server,
            limits: PeerLabConfig::default().packet.packet.limits,
        },
        codec: ControlCodec::default(),
        handshake_timeout: Duration::from_secs(2),
        handshake_attempts: 3,
        reply_window: Duration::from_secs(3),
        max_datagrams: 1,
    }
}

fn login() -> ControlMessage {
    ControlMessage::Login {
        response: ArchiveText::text("0").unwrap(),
        url: ArchiveText::text(&format!("?Name={NAME_SENTINEL}")).unwrap(),
        unique_net_id: UniqueNetId {
            kind: NetIdType::Named(ArchiveText::text("STEAM").unwrap()),
            value: NetIdValue::from_hex(NET_ID_HEX).unwrap(),
        },
        platform: ArchiveText::text("").unwrap(),
    }
}

fn connect(server: SocketAddr) -> ProbeClient {
    let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    socket.connect(server).unwrap();
    ProbeClient::connect(socket, probe_config()).unwrap()
}

/// Hello (expecting Challenge) then Login; returns the Login reply.
fn hello_and_login(client: &mut ProbeClient) -> ControlMessage {
    let report = client.hello(0, "").unwrap();
    assert!(report.hello_acked);
    assert!(matches!(
        report.messages().collect::<Vec<_>>().as_slice(),
        [ControlMessage::Challenge { .. }]
    ));
    client.send_control(&[login()], false).unwrap();
    let collected = client.collect(Duration::from_secs(3), 1).unwrap();
    let messages: Vec<_> = collected.messages().cloned().collect();
    assert_eq!(messages.len(), 1, "one reply to Login");
    messages.into_iter().next().unwrap()
}

/// Handshake, Hello, Login (expecting Welcome), then Netspeed and Join in
/// one packet whose acknowledgement is awaited.
fn join(server: SocketAddr) -> ProbeClient {
    let mut client = connect(server);
    let reply = hello_and_login(&mut client);
    assert!(
        matches!(reply, ControlMessage::Welcome { .. }),
        "{}",
        reply.name()
    );
    let (_, packet_id) = client
        .send_control(
            &[
                ControlMessage::Netspeed { rate: 100_000 },
                ControlMessage::Join,
            ],
            false,
        )
        .unwrap();
    let collected = client.collect(Duration::from_secs(3), 1).unwrap();
    assert!(collected.acked.contains(&packet_id), "Join acknowledged");
    client
}

fn failure_text(message: &ControlMessage) -> String {
    match message {
        ControlMessage::Failure { message } => String::from_utf16_lossy(message.units()),
        other => panic!("expected Failure, got {}", other.name()),
    }
}

fn enabled(port: u16) -> NetworkConfig {
    NetworkConfig {
        enabled: true,
        port,
        ..NetworkConfig::default()
    }
}

/// The production app with the session layer installed from `network`.
fn server_app(network: &NetworkConfig) -> (App, SocketAddr) {
    let config = ServerConfig {
        network: network.clone(),
        ..ServerConfig::default()
    };
    let mut app = build_app(&config, &WorldSnapshot::default()).unwrap();
    let lab = network::bind(&config.network).unwrap().unwrap().lab;
    let address = network::install(&mut app, lab).unwrap();
    assert!(address.ip().is_loopback());
    (app, address)
}

fn counts(app: &App) -> SessionCounts {
    network::session_counts(app.world()).unwrap()
}

/// Run `probe` on its own thread while the app updates on this one; returns
/// the probe result and the largest peer-table size seen between updates.
fn with_server<T: Send + 'static>(
    app: &mut App,
    probe: impl FnOnce() -> T + Send + 'static,
) -> (T, usize) {
    let (done, result) = mpsc::channel();
    let worker = thread::spawn(move || done.send(probe()).unwrap());
    let deadline = Instant::now() + Duration::from_secs(120);
    let mut peak = 0;
    loop {
        app.update();
        peak = peak.max(app.world().resource::<PeerLab>().peer_count());
        if let Ok(value) = result.try_recv() {
            worker.join().unwrap();
            // Process anything the probe sent last (a close, for instance).
            for _ in 0..3 {
                app.update();
                thread::sleep(Duration::from_millis(2));
            }
            return (value, peak);
        }
        assert!(Instant::now() < deadline, "probe did not finish");
        thread::sleep(Duration::from_micros(200));
    }
}

#[test]
fn network_config_defaults_bounds_and_unknown_fields() {
    let defaults = NetworkConfig::default();
    assert!(!defaults.enabled);
    assert_eq!(defaults.bind, std::net::Ipv4Addr::LOCALHOST);
    assert_eq!(defaults.max_players, 8);
    assert_eq!(defaults.session_timeout_seconds, 30);
    assert!(ServerConfig::default().validate().is_ok());
    let parsed: ServerConfig = toml::from_str("").unwrap();
    assert_eq!(parsed.network, defaults);
    let parsed: ServerConfig = toml::from_str(
        "[network]\nenabled = true\nbind = \"127.0.0.2\"\nport = 0\nmax_players = 64\nsession_timeout_seconds = 600\n",
    )
    .unwrap();
    assert!(parsed.network.enabled);
    assert_eq!(parsed.network.port, 0);
    assert!(parsed.validate().is_ok());
    for (field, value) in [
        ("max_players", "0"),
        ("max_players", "65"),
        ("session_timeout_seconds", "0"),
        ("session_timeout_seconds", "601"),
        ("bind", "\"0.0.0.0\""),
        ("bind", "\"192.0.2.1\""),
    ] {
        let config: ServerConfig =
            toml::from_str(&format!("[network]\n{field} = {value}\n")).unwrap();
        assert!(config.validate().is_err(), "{field} = {value}");
    }
    for invalid in [
        "[network]\nenabeld = true\n",
        "[network]\nport = 70000\n",
        "[network]\nbind = \"::1\"\n",
        "[network]\nbind = \"localhost\"\n",
        "network = true\n",
    ] {
        assert!(
            toml::from_str::<ServerConfig>(invalid).is_err(),
            "{invalid}"
        );
    }
}

#[test]
fn check_config_prints_network_section_and_port_override() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("server.toml");
    fs::write(&path, "[network]\nenabled = true\nmax_players = 4\n").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_icarus-server"))
        .arg("check-config")
        .arg("--config")
        .arg(&path)
        .args(["--network-port", "0", "--data-dir"])
        .arg(directory.path().join("state"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("[network]"), "{text}");
    let effective: ServerConfig = toml::from_str(&text).unwrap();
    assert_eq!(
        effective.network,
        NetworkConfig {
            enabled: true,
            port: 0,
            max_players: 4,
            ..NetworkConfig::default()
        }
    );
    assert!(!directory.path().join("state").exists());
    fs::write(&path, "[network]\nmax_players = 65\n").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_icarus-server"))
        .arg("check-config")
        .arg("--config")
        .arg(&path)
        .output()
        .unwrap();
    assert!(!output.status.success());
}

#[test]
fn network_input_is_processed_before_simulation_ticks() {
    #[derive(Resource, Default)]
    struct SeenAtInput(Vec<u64>);
    fn record(lab: Res<PeerLab>, mut seen: ResMut<SeenAtInput>) {
        seen.0.push(lab.stats().received);
    }
    let (mut app, address) = server_app(&enabled(0));
    app.insert_resource(TimeUpdateStrategy::FixedTimesteps(1))
        .init_resource::<SeenAtInput>()
        .add_systems(FixedUpdate, record.in_set(SimulationSet::Input));
    app.update();
    app.update();
    let before = app.world().resource::<SeenAtInput>().0.len();
    assert!(before > 0, "fixed ticks ran");
    let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    socket.send_to(&initial_request(), address).unwrap();
    // Loopback delivery is asynchronous; wait until the datagram is queued.
    thread::sleep(Duration::from_millis(20));
    app.update();
    let seen = &app.world().resource::<SeenAtInput>().0;
    assert_eq!(seen.len(), before + 1, "one fixed tick per update");
    assert_eq!(
        seen.last(),
        Some(&1),
        "the tick saw the datagram polled this update"
    );
}

#[test]
fn ninth_concurrent_session_is_refused_at_admission_with_default_limit() {
    let network = enabled(0);
    assert_eq!(network.max_players, 8);
    let (mut app, address) = server_app(&network);
    let (refusal, _) = with_server(&mut app, move || {
        let joined: Vec<_> = (0..8).map(|_| join(address)).collect();
        let mut ninth = connect(address);
        let refusal = hello_and_login(&mut ninth);
        drop(joined);
        failure_text(&refusal)
    });
    assert_eq!(refusal, SERVER_FULL);
    let counts = counts(&app);
    assert_eq!(
        (counts.welcomes_sent, counts.joins_succeeded, counts.joined),
        (8, 8, 8)
    );
    assert_eq!((counts.admissions_rejected, counts.closed_by_reply), (1, 1));
    assert_eq!((counts.active_peers, counts.peers_rejected), (8, 0));
    assert_eq!(counts.invalid_state_messages, 0);
}

#[test]
fn thousand_sequential_join_close_cycles_retain_no_sessions() {
    const CYCLES: u64 = 1_000;
    let (mut app, address) = server_app(&enabled(0));
    let started = Instant::now();
    let ((), peak_seen) = with_server(&mut app, move || {
        for _ in 0..CYCLES {
            join(address).close().unwrap();
        }
    });
    let elapsed = started.elapsed();
    let counts = counts(&app);
    let lab = app.world().resource::<PeerLab>();
    eprintln!(
        "{}",
        serde_json::json!({"cycles": CYCLES, "wall_ms": elapsed.as_millis() as u64,
        "final_peers": counts.active_peers, "peak_peers": counts.peak_peers,
        "peak_peers_between_updates": peak_seen, "max_peers": lab.config().max_peers})
    );
    assert_eq!(counts.active_peers, 0);
    assert_eq!((counts.welcomed, counts.joined), (0, 0));
    assert_eq!(
        (
            counts.welcomes_sent,
            counts.joins_succeeded,
            counts.closed_by_client
        ),
        (CYCLES, CYCLES, CYCLES)
    );
    assert_eq!(
        (
            counts.admissions_rejected,
            counts.closed_by_timeout,
            counts.closed_failed
        ),
        (0, 0, 0)
    );
    assert!(counts.peak_peers <= 2, "peak {}", counts.peak_peers);
    assert!(counts.peak_peers <= lab.config().max_peers);
    assert_eq!(lab.stats().peers_created, CYCLES);
}

#[test]
fn joined_session_expires_after_idle_timeout() {
    let network = enabled(0);
    let mut config = network::peer_lab_config(&network);
    config.idle_timeout = Duration::from_millis(100);
    let lab = PeerLab::bind_address(SocketAddrV4::new(network.bind, 0), config).unwrap();
    let mut app = build_app(&ServerConfig::default(), &WorldSnapshot::default()).unwrap();
    let address = network::install(&mut app, lab).unwrap();
    let (client, _) = with_server(&mut app, move || join(address));
    assert_eq!(counts(&app).joined, 1);
    thread::sleep(Duration::from_millis(250));
    app.update();
    let counts = counts(&app);
    assert_eq!((counts.active_peers, counts.closed_by_timeout), (0, 1));
    drop(client);
}

#[test]
fn stopped_admission_refuses_new_sessions_and_teardown_is_idempotent() {
    let (mut app, address) = server_app(&enabled(0));
    let (welcomed, _) = with_server(&mut app, move || {
        let joined = join(address);
        let mut welcomed = connect(address);
        let reply = hello_and_login(&mut welcomed);
        assert!(matches!(reply, ControlMessage::Welcome { .. }));
        (joined, welcomed)
    });
    network::stop_admission(app.world_mut());
    network::stop_admission(app.world_mut());
    assert!(!counts(&app).admitting);
    let (mut joined, mut welcomed) = welcomed;
    let (refusal, _) = with_server(&mut app, move || {
        // A new handshake gets no challenge.
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        socket.connect(address).unwrap();
        let refused = ProbeClient::connect(
            socket,
            ProbeConfig {
                handshake_timeout: Duration::from_millis(100),
                handshake_attempts: 2,
                ..probe_config()
            },
        );
        assert!(refused.is_err());
        // A welcomed session's Join is refused; the joined one stays live.
        welcomed
            .send_control(&[ControlMessage::Join], false)
            .unwrap();
        let collected = welcomed.collect(Duration::from_secs(3), 1).unwrap();
        let reply = collected.messages().next().cloned().unwrap();
        joined.keepalive().unwrap();
        failure_text(&reply)
    });
    assert_eq!(refusal, SHUTTING_DOWN_MESSAGE);
    let live = counts(&app);
    assert_eq!((live.active_peers, live.joined), (1, 1));
    assert_eq!(live.refused_draining, 3);
    let first = network::teardown(app.world_mut()).unwrap();
    let second = network::teardown(app.world_mut()).unwrap();
    assert_eq!(first, second);
    assert_eq!((first.active_peers, first.closed_by_shutdown), (0, 1));
    assert!(app.world().get_resource::<PeerLab>().is_none());
    // Updates after teardown are harmless and the socket is released.
    app.update();
    UdpSocket::bind(address).unwrap();
}

struct RunningServer {
    child: Child,
    logs: Receiver<String>,
    seen: Vec<String>,
}

impl RunningServer {
    fn spawn(command: &mut Command) -> Self {
        let mut child = command
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let stream = child.stderr.take().unwrap();
        let (sender, logs) = mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(stream).lines() {
                if sender.send(line.unwrap()).is_err() {
                    break;
                }
            }
        });
        Self {
            child,
            logs,
            seen: Vec::new(),
        }
    }

    /// The next record matching `accept`, within ten seconds.
    fn wait_for(&mut self, what: &str, accept: impl Fn(&Value) -> bool) -> Value {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let line = self
                .logs
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .unwrap_or_else(|error| {
                    panic!("waiting for {what}: {error}; seen {:?}", self.seen)
                });
            self.seen.push(line.clone());
            let record: Value = serde_json::from_str(&line).unwrap();
            if accept(&record) {
                return record;
            }
        }
    }

    fn event(&mut self, event: &str) -> Value {
        self.wait_for(event, |record| record["event"] == event)
    }

    fn wait_exit(&mut self) -> std::process::ExitStatus {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                return status;
            }
            assert!(Instant::now() < deadline, "server did not exit");
            thread::sleep(Duration::from_millis(5));
        }
    }

    /// Every remaining record after exit.
    fn drain(&mut self) {
        while let Ok(line) = self.logs.recv_timeout(Duration::from_secs(2)) {
            self.seen.push(line);
        }
    }
}

impl Drop for RunningServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn server_config(directory: &std::path::Path) -> std::path::PathBuf {
    let path = directory.join("server.toml");
    fs::write(
        &path,
        "status_interval_seconds = 1\n[network]\nenabled = true\nport = 0\n",
    )
    .unwrap();
    path
}

#[test]
fn production_run_admits_a_probe_session_and_saves_on_sigterm() {
    let directory = tempfile::tempdir().unwrap();
    let config = server_config(directory.path());
    let state = directory.path().join("state");
    let mut server = RunningServer::spawn(
        Command::new(env!("CARGO_BIN_EXE_icarus-server"))
            .arg("run")
            .arg("--config")
            .arg(&config)
            .args(["--tick-hz", "120", "--max-ticks", "100000", "--data-dir"])
            .arg(&state),
    );
    let started = server.event("started");
    assert_eq!(started["network_enabled"], true);
    assert_eq!(started["gameplay_ready"], false);
    let listening = server.event("network_listening");
    assert_eq!(listening["max_players"], 8);
    let address: SocketAddr = listening["address"].as_str().unwrap().parse().unwrap();
    assert!(address.ip().is_loopback() && address.port() != 0);
    assert_eq!(listening["version_check"], false);
    assert_eq!(listening["release_build_id"], Value::Null);

    let client = join(address);
    let status = server.wait_for("status with a joined session", |record| {
        record["event"] == "status" && record["sessions"]["joined"] == 1
    });
    assert_eq!(status["gameplay_ready"], false);
    assert_eq!(status["sessions"]["active_peers"], 1);
    assert_eq!(status["sessions"]["welcomes_sent"], 1);
    assert_eq!(status["sessions"]["admitting"], true);

    assert!(
        Command::new("kill")
            .args(["-TERM", &server.child.id().to_string()])
            .status()
            .unwrap()
            .success()
    );
    assert!(server.wait_exit().success());
    let stopped = server.event("stopped");
    let sessions = &stopped["sessions"];
    assert_eq!(sessions["admitting"], false);
    assert_eq!(sessions["active_peers"], 0);
    assert_eq!(sessions["joins_succeeded"], 1);
    assert_eq!(sessions["closed_by_shutdown"], 1);
    assert_eq!(sessions["invalid_state_messages"], 0);
    server.drain();
    assert!(
        server
            .seen
            .iter()
            .any(|line| line.contains("\"event\":\"save_committed\""))
    );
    for line in &server.seen {
        for secret in [NAME_SENTINEL, NET_ID_HEX, &NET_ID_HEX.to_lowercase()] {
            assert!(!line.contains(secret), "{secret} in {line}");
        }
    }
    let store = icarus_bevy::persistence::SnapshotStore::open(&state).unwrap();
    assert!(store.load().unwrap().is_some());
    drop(client);
}

#[test]
fn production_run_with_max_ticks_reports_sessions_and_exits_cleanly() {
    let directory = tempfile::tempdir().unwrap();
    let config = server_config(directory.path());
    let state = directory.path().join("state");
    let output = Command::new(env!("CARGO_BIN_EXE_icarus-server"))
        .arg("run")
        .arg("--config")
        .arg(&config)
        .args(["--compressed", "--max-ticks", "30", "--data-dir"])
        .arg(&state)
        .output()
        .unwrap();
    let logs = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{logs}");
    assert!(logs.contains("\"event\":\"network_listening\""));
    assert!(logs.contains("save_committed"));
    let stopped: Value = logs
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .find(|record| record["event"] == "stopped")
        .unwrap();
    assert_eq!(stopped["sessions"]["active_peers"], 0);
    assert_eq!(stopped["sessions"]["admitting"], false);
    assert_eq!(stopped["gameplay_ready"], false);
}

#[test]
fn network_bind_failure_exits_before_world_state_is_created() {
    let occupied = UdpSocket::bind("127.0.0.1:0").unwrap();
    let port = occupied.local_addr().unwrap().port().to_string();
    let directory = tempfile::tempdir().unwrap();
    let config = server_config(directory.path());
    let state = directory.path().join("state");
    let output = Command::new(env!("CARGO_BIN_EXE_icarus-server"))
        .arg("run")
        .arg("--config")
        .arg(&config)
        .args(["--network-port", &port, "--max-ticks", "1", "--data-dir"])
        .arg(&state)
        .output()
        .unwrap();
    assert!(!output.status.success());
    let logs = String::from_utf8_lossy(&output.stderr);
    assert!(logs.contains("\"event\":\"fatal\""), "{logs}");
    assert!(logs.contains("bind"), "{logs}");
    assert!(!logs.contains("\"event\":\"started\""), "{logs}");
    assert!(!state.exists());
}

#[test]
fn disabled_network_opens_no_socket_and_reports_no_sessions() {
    let directory = tempfile::tempdir().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_icarus-server"))
        .args(["run", "--compressed", "--max-ticks", "3", "--data-dir"])
        .arg(directory.path())
        .output()
        .unwrap();
    let logs = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{logs}");
    assert!(logs.contains("\"network_enabled\":false"));
    assert!(!logs.contains("network_listening"));
    assert!(logs.contains("\"sessions\":null"));
}

/// The repository's release pin, by absolute path.
fn release_toml() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("config/release.toml")
}

fn pinned_server_config(
    directory: &std::path::Path,
    profile: &std::path::Path,
) -> std::path::PathBuf {
    let path = directory.join("server.toml");
    let profile = toml::Value::String(profile.display().to_string());
    fs::write(
        &path,
        format!(
            "status_interval_seconds = 1\n[network]\nenabled = true\nport = 0\nrelease_profile = {profile}\n"
        ),
    )
    .unwrap();
    path
}

/// Start `run` at 240 Hz (the configured maximum) with `config`.
fn spawn_run(config: &std::path::Path, state: &std::path::Path) -> RunningServer {
    RunningServer::spawn(
        Command::new(env!("CARGO_BIN_EXE_icarus-server"))
            .arg("run")
            .arg("--config")
            .arg(config)
            .args(["--tick-hz", "240", "--max-ticks", "1000000", "--data-dir"])
            .arg(state),
    )
}

fn sigterm(server: &RunningServer) {
    assert!(
        Command::new("kill")
            .args(["-TERM", &server.child.id().to_string()])
            .status()
            .unwrap()
            .success()
    );
}

fn listening_address(server: &mut RunningServer) -> (SocketAddr, Value) {
    let listening = server.event("network_listening");
    let address = listening["address"].as_str().unwrap().parse().unwrap();
    (address, listening)
}

#[test]
fn release_profile_config_parses_and_binds_the_pinned_version() {
    let parsed: ServerConfig =
        toml::from_str("[network]\nrelease_profile = \"config/release.toml\"\n").unwrap();
    assert_eq!(
        parsed.network.release_profile.as_deref(),
        Some(std::path::Path::new("config/release.toml"))
    );
    let empty: ServerConfig = toml::from_str("[network]\nrelease_profile = \"\"\n").unwrap();
    assert!(empty.validate().is_err());
    // Unset stays out of the printed effective configuration.
    let printed = toml::to_string_pretty(&ServerConfig::default()).unwrap();
    assert!(!printed.contains("release_profile"), "{printed}");

    let profile = ReleaseProfile::load(&release_toml()).unwrap();
    let bound = network::bind(&NetworkConfig {
        release_profile: Some(release_toml()),
        ..enabled(0)
    })
    .unwrap()
    .unwrap();
    assert_eq!(
        bound.lab.config().local_network_version,
        Some(profile.pin.network_checksum)
    );
    assert_eq!(bound.release, Some(profile.pin));
    let unpinned = network::bind(&enabled(0)).unwrap().unwrap();
    assert_eq!(unpinned.lab.config().local_network_version, None);
    assert!(unpinned.release.is_none());
}

#[test]
fn pinned_run_upgrades_a_mismatched_hello_and_admits_the_pinned_version() {
    let profile = ReleaseProfile::load(&release_toml()).unwrap();
    let pinned = profile.pin.network_checksum;
    let directory = tempfile::tempdir().unwrap();
    let config = pinned_server_config(directory.path(), &release_toml());
    let mut server = spawn_run(&config, &directory.path().join("state"));
    let (address, listening) = listening_address(&mut server);
    assert_eq!(listening["version_check"], true);
    assert_eq!(listening["release_build_id"], profile.pin.build_id);

    let mut mismatched = connect(address);
    let report = mismatched.hello(pinned.wrapping_add(1), "").unwrap();
    assert!(matches!(
        report.messages().collect::<Vec<_>>().as_slice(),
        [ControlMessage::Upgrade { version }] if *version == pinned
    ));
    if let Err((stage, error)) = session_cycle(address, pinned, Duration::from_secs(3)) {
        panic!("{stage:?}: {error:#}");
    }
    server.wait_for("status after both sessions closed", |record| {
        record["event"] == "status"
            && record["sessions"]["closed_by_client"] == 1
            && record["sessions"]["closed_by_reply"] == 1
    });
    sigterm(&server);
    assert!(server.wait_exit().success());
    let stopped = server.event("stopped");
    let sessions = &stopped["sessions"];
    for (field, value) in [
        ("upgrades_sent", 1),
        ("closed_by_reply", 1),
        ("admissions_rejected", 0),
        ("welcomes_sent", 1),
        ("joins_succeeded", 1),
        ("player_controller_opens_sent", 1),
        ("pawn_opens_sent", 1),
        ("game_state_opens_sent", 1),
        ("player_state_opens_sent", 1),
        // Fresh worlds also open the tracker used by new-player completion.
        ("manager_opens_sent", 4),
        ("open_errors", 0),
        ("io_errors", 0),
        ("closed_by_client", 1),
        ("active_peers", 0),
    ] {
        assert_eq!(sessions[field], value, "{field}");
    }
    drop(mismatched);
}

#[test]
fn bad_release_profile_fails_before_bind_and_world_state() {
    // An occupied port proves the order: a bind attempted first would fail
    // with a bind error instead of the profile error.
    let occupied = UdpSocket::bind("127.0.0.1:0").unwrap();
    let port = occupied.local_addr().unwrap().port().to_string();
    let directory = tempfile::tempdir().unwrap();
    let invalid = directory.path().join("invalid-release.toml");
    let pin = fs::read_to_string(release_toml()).unwrap();
    fs::write(
        &invalid,
        pin.replace("network_checksum = ", "network_checksum = 1"),
    )
    .unwrap();
    for profile in [directory.path().join("missing-release.toml"), invalid] {
        let config = pinned_server_config(directory.path(), &profile);
        let state = directory.path().join("state");
        let output = Command::new(env!("CARGO_BIN_EXE_icarus-server"))
            .arg("run")
            .arg("--config")
            .arg(&config)
            .args(["--network-port", &port, "--max-ticks", "1", "--data-dir"])
            .arg(&state)
            .output()
            .unwrap();
        assert!(!output.status.success());
        let logs = String::from_utf8_lossy(&output.stderr);
        assert!(logs.contains("\"event\":\"fatal\""), "{logs}");
        assert!(logs.contains("network.release_profile"), "{logs}");
        assert!(!logs.contains("bind the experimental"), "{logs}");
        assert!(!logs.contains("\"event\":\"started\""), "{logs}");
        assert!(!state.exists());
    }
}

#[test]
fn production_run_survives_two_hundred_sequential_churn_cycles() {
    const CYCLES: u64 = 200;
    let directory = tempfile::tempdir().unwrap();
    let config = server_config(directory.path());
    let state = directory.path().join("state");
    let mut server = spawn_run(&config, &state);
    let (address, _) = listening_address(&mut server);
    let summary = run_churn(&ChurnConfig {
        server: address,
        cycles: CYCLES,
        concurrency: 1,
        network_version: 0,
        step_timeout: Duration::from_secs(2),
    })
    .unwrap();
    let summary = serde_json::to_value(&summary).unwrap();
    eprintln!("{summary}");
    assert_eq!(summary["cycles_succeeded"], CYCLES, "{summary}");
    assert_eq!(summary["cycles_failed"], 0, "{summary}");
    server.wait_for("status after every client close", |record| {
        record["event"] == "status" && record["sessions"]["closed_by_client"] == CYCLES
    });
    sigterm(&server);
    let signalled = server.seen.len();
    assert!(server.wait_exit().success());
    let stopped = server.event("stopped");
    assert_eq!(stopped["gameplay_ready"], false);
    let sessions = &stopped["sessions"];
    assert_eq!(sessions["active_peers"], 0);
    for field in ["welcomes_sent", "joins_succeeded", "closed_by_client"] {
        assert_eq!(sessions[field], CYCLES, "{field}");
    }
    for field in [
        "admissions_rejected",
        "peers_rejected",
        "closed_by_timeout",
        "closed_by_reply",
        "closed_failed",
        "closed_by_shutdown",
        "invalid_state_messages",
        "upgrades_sent",
    ] {
        assert_eq!(sessions[field], 0, "{field}");
    }
    assert!(
        server.seen[signalled..]
            .iter()
            .any(|line| line.contains("\"event\":\"save_committed\"")),
        "final save after SIGTERM"
    );
    let store = icarus_bevy::persistence::SnapshotStore::open(&state).unwrap();
    assert!(store.load().unwrap().is_some());
}

/// Hello with `version`, Login, then Netspeed and Join; returns the datagram
/// answering the Join.
fn join_pinned(server: SocketAddr, version: u32) -> Vec<u8> {
    join_pinned_replies(server, version, 1).remove(0)
}

/// As [`join_pinned`], collecting up to `max` datagrams after the Join.
fn join_pinned_replies(server: SocketAddr, version: u32, max: usize) -> Vec<Vec<u8>> {
    let (client, collected) = join_pinned_open(server, version, max);
    client.close().unwrap();
    collected
}

/// Join on the pinned version and collect up to `max` datagrams after the
/// Join, leaving the session open.
fn join_pinned_open(server: SocketAddr, version: u32, max: usize) -> (ProbeClient, Vec<Vec<u8>>) {
    let mut client = connect(server);
    let report = client.hello(version, "").unwrap();
    assert!(matches!(
        report.messages().collect::<Vec<_>>().as_slice(),
        [ControlMessage::Challenge { .. }]
    ));
    client.send_control(&[login()], false).unwrap();
    let welcome = client.collect(Duration::from_secs(3), 1).unwrap();
    assert!(matches!(
        welcome.messages().collect::<Vec<_>>().as_slice(),
        [ControlMessage::Welcome { .. }]
    ));
    let (_, packet_id) = client
        .send_control(
            &[
                ControlMessage::Netspeed { rate: 100_000 },
                ControlMessage::Join,
            ],
            false,
        )
        .unwrap();
    let collected = client.collect(Duration::from_secs(3), max).unwrap();
    assert!(collected.acked.contains(&packet_id), "Join acknowledged");
    let replies = collected
        .replies
        .into_iter()
        .map(|reply| reply.bytes)
        .collect();
    (client, replies)
}

/// Compose the pawn open the lab sends after connection `connection`'s
/// PlayerController open (F-797), with `run`'s pawn content.
fn compose_lab_pawn(
    profile: &ReleaseProfile,
    cache: &mut icarus_bevy::experimental::session_open::NetGuidCache,
    exports: &mut icarus_bevy::experimental::session_open::ConnectionExports,
    connection: u32,
    guids: &icarus_bevy::experimental::session_open::PlayerControllerGuids,
) -> icarus_bevy::experimental::session_open::ComposedActorOpen {
    use icarus_bevy::experimental::{
        archive::ByteOrder,
        pawn_open::{PAWN_CHANNEL_INDEX, PawnOpenState, compose_pawn_open},
        peer_lab::{PeerLabAdmission, pawn_key},
    };
    let plan = profile
        .pawn_open_plan(PeerLabAdmission::default().map_url)
        .unwrap()
        .unwrap();
    let mut content = network::pawn_content(None).unwrap();
    content.icarus_uid =
        Some(icarus_bevy::actor_uids::FIRST_NATIVE_OBJECT_ID + connection as i32 - 1);
    let state = PawnOpenState {
        key: pawn_key(connection),
        controller: guids.actor,
        player_state: guids.player_state,
        location: network::SPAWN_LOCATION,
        content,
    };
    compose_pawn_open(
        cache,
        exports,
        &plan,
        &state,
        PAWN_CHANNEL_INDEX,
        ByteOrder::Little,
    )
    .unwrap()
}

/// Decode handle 46 from the pawn's actor block, using the same packet framing
/// as the existing exact-byte pawn-open assertions.
fn sent_pawn_uid(profile: &ReleaseProfile, datagrams: &[Vec<u8>]) -> Option<i32> {
    use icarus_bevy::experimental::{
        actor_channel::{ActorChannelCodec, ActorChannelLimits, read_actor_bunch_body},
        archive::ByteOrder,
        packet::read_packet,
        pawn_open::PAWN_CHANNEL_INDEX,
        peer_lab::PeerLabAdmission,
        rep_layout::{RepLayoutCodec, RepLayoutLimits, RepValue, read_rep_properties},
    };
    let packet_config = PacketConfig {
        receiver: PacketReceiver::Client,
        ..probe_config().packet
    };
    let plan = profile
        .pawn_open_plan(PeerLabAdmission::default().map_url)
        .unwrap()
        .unwrap();
    let actor = ActorChannelCodec {
        engine_net_version: profile.engine_network_version,
        exports_enabled: false,
        has_level: true,
        order: ByteOrder::Little,
        limits: ActorChannelLimits::default(),
    };
    for bytes in datagrams {
        let packet = read_packet(bytes, &packet_config).unwrap();
        for bunch in packet.bunches {
            if bunch.header.channel_index != PAWN_CHANNEL_INDEX
                || bunch.header.has_package_map_exports
            {
                continue;
            }
            let body = read_actor_bunch_body(&mut bunch.payload.reader(), true, &actor).unwrap();
            let codec = RepLayoutCodec {
                actor,
                checksums: false,
                limits: RepLayoutLimits::default(),
            };
            let properties =
                read_rep_properties(&mut body.blocks[0].payload.reader(), &plan.layout, &codec)
                    .unwrap();
            let uid = properties
                .entries
                .into_iter()
                .find(|entry| entry.handle == 46)
                .unwrap();
            let RepValue::Int(uid) = uid.value else {
                panic!("IcarusUID is an i32")
            };
            return Some(uid);
        }
    }
    None
}

/// Authored actor/inventory-owner recorder bodies; no supplied save or original
/// distribution is read. The UID scan must also cover unreplicated classes.
fn uid_save_payload(records: &[Vec<scalar::Tag>]) -> Vec<u8> {
    use scalar::{Tag, stream, units};
    let records_body: Vec<_> = records
        .iter()
        .flat_map(|fields| {
            let body = [stream(fields), vec![0; 4]].concat();
            stream(&[
                Tag::text(
                    "ComponentClassName",
                    "StrProperty",
                    &units("AuthoredInventoryOwner"),
                ),
                Tag::array(
                    "BinaryData",
                    "ByteProperty",
                    [
                        i32::try_from(body.len()).unwrap().to_le_bytes().to_vec(),
                        body,
                    ]
                    .concat(),
                ),
            ])
        })
        .collect();
    stream(&[Tag::array(
        "StateRecorderBlobs",
        "StructProperty",
        [
            i32::try_from(records.len()).unwrap().to_le_bytes().to_vec(),
            Tag::structure("StateRecorderBlobs", "StateRecorderBlob", records_body).bytes(),
        ]
        .concat(),
    )])
}

#[test]
fn saved_actor_uid_scan_covers_all_recorders_and_rejects_partial_namespaces() {
    use icarus_bevy::prospect::saved_actor_uids_from_payload;
    use scalar::Tag;
    let payload = uid_save_payload(&[
        vec![Tag::integer("IcarusActorGUID", 65_536)],
        vec![Tag::integer("IcarusActorGUID", -1)],
        vec![Tag::integer("IcarusActorGUID", 0)],
        vec![Tag::integer("IcarusActorGUID", 65_536)],
        vec![Tag::integer("IcarusActorGUID", 65_537)],
        vec![Tag::integer("RelevantActorIcarusUID", 123)],
    ]);
    assert_eq!(
        saved_actor_uids_from_payload(&payload).unwrap(),
        [65_536, 65_536, 65_537]
    );
    for invalid in [
        vec![
            Tag::integer("IcarusActorGUID", 1),
            Tag::integer("IcarusActorGUID", 2),
        ],
        vec![Tag::text(
            "IcarusActorGUID",
            "StrProperty",
            &scalar::units("invalid"),
        )],
        vec![{
            let mut field = Tag::integer("IcarusActorGUID", 1);
            field.index = 1;
            field
        }],
        vec![{
            let mut field = Tag::integer("IcarusActorGUID", 1);
            field.payload.pop();
            field
        }],
    ] {
        assert!(
            saved_actor_uids_from_payload(&uid_save_payload(&[
                vec![Tag::integer("IcarusActorGUID", 5)],
                invalid,
            ]))
            .is_err()
        );
    }
}

#[test]
fn pinned_run_pawns_have_distinct_positive_uids_avoiding_the_loaded_save() {
    use icarus_bevy::{
        persistence::SnapshotStore,
        preservation::{PreserveLimits, PreservedSection},
    };
    let profile = ReleaseProfile::load(&release_toml()).unwrap();
    let pinned = profile.pin.network_checksum;
    let directory = tempfile::tempdir().unwrap();
    let config = pinned_server_config(directory.path(), &release_toml());
    let state = directory.path().join("state");
    let seeded = [65_536, 65_537, 65_538];
    let payload = uid_save_payload(
        &seeded
            .iter()
            .map(|uid| vec![scalar::Tag::integer("IcarusActorGUID", *uid)])
            .collect::<Vec<_>>(),
    );
    let preserved =
        PreservedSection::from_inner_payload(&payload, PreserveLimits::default()).unwrap();
    {
        let mut store = SnapshotStore::open(&state).unwrap();
        store
            .save(&WorldSnapshot {
                schema_version: icarus_bevy::simulation::SNAPSHOT_SCHEMA_REFERENCED,
                preserved_prospect: Some(preserved.into()),
                ..WorldSnapshot::default()
            })
            .unwrap();
    }
    let mut server = spawn_run(&config, &state);
    let (address, _) = listening_address(&mut server);
    let (first, first_bytes) = join_pinned_open(address, pinned, 7);
    let (second, second_bytes) = join_pinned_open(address, pinned, 7);
    let first_uid = sent_pawn_uid(&profile, &first_bytes).unwrap();
    let second_uid = sent_pawn_uid(&profile, &second_bytes).unwrap();
    assert!(first_uid > 0 && second_uid > 0);
    assert_ne!(first_uid, second_uid);
    assert!(!seeded.contains(&first_uid) && !seeded.contains(&second_uid));
    first.close().unwrap();
    second.close().unwrap();
    sigterm(&server);
    assert!(server.wait_exit().success());
}

#[test]
fn unprofiled_run_preserves_opaque_actor_values_without_enabling_uids() {
    use icarus_bevy::{
        persistence::SnapshotStore,
        preservation::{PreserveLimits, PreservedSection},
    };
    let directory = tempfile::tempdir().unwrap();
    let config = server_config(directory.path());
    let state = directory.path().join("state");
    let payload = uid_save_payload(&[vec![scalar::Tag::text(
        "IcarusActorGUID",
        "StrProperty",
        &scalar::units("opaque authored value"),
    )]]);
    assert!(icarus_bevy::prospect::saved_actor_uids_from_payload(&payload).is_err());
    let preserved =
        PreservedSection::from_inner_payload(&payload, PreserveLimits::default()).unwrap();
    {
        let mut store = SnapshotStore::open(&state).unwrap();
        store
            .save(&WorldSnapshot {
                schema_version: icarus_bevy::simulation::SNAPSHOT_SCHEMA_REFERENCED,
                preserved_prospect: Some(preserved.into()),
                ..WorldSnapshot::default()
            })
            .unwrap();
    }
    let mut server = spawn_run(&config, &state);
    let (address, _) = listening_address(&mut server);
    let client = join(address);
    client.close().unwrap();
    sigterm(&server);
    assert!(server.wait_exit().success());
    let store = SnapshotStore::open(&state).unwrap();
    let restored = store.load().unwrap().unwrap();
    assert_eq!(
        restored
            .snapshot
            .preserved_prospect
            .unwrap()
            .decode()
            .unwrap()
            .to_inner_payload()
            .unwrap(),
        payload
    );
}

fn uid_lab(library: icarus_bevy::actor_uids::ActorUidLibrary) -> (PeerLab, ReleaseProfile) {
    uid_lab_with(library, network::peer_lab_config(&enabled(0)))
}

fn uid_lab_with(
    library: icarus_bevy::actor_uids::ActorUidLibrary,
    config: icarus_bevy::experimental::peer_lab::PeerLabConfig,
) -> (PeerLab, ReleaseProfile) {
    use icarus_bevy::experimental::{pawn_open::PAWN_CHANNEL_INDEX, peer_lab::PeerLabAdmission};
    let profile = ReleaseProfile::load(&release_toml()).unwrap();
    let admission = PeerLabAdmission::default();
    let mut lab = PeerLab::bind(0, config.with_release_profile(&profile)).unwrap();
    lab.enable_player_controller_open(
        profile
            .player_controller_open_plan(admission.map_url, network::SPAWN_LOCATION)
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    lab.enable_pawn_open(
        profile.pawn_open_plan(admission.map_url).unwrap().unwrap(),
        PAWN_CHANNEL_INDEX,
        network::pawn_content(None).unwrap(),
    )
    .unwrap();
    lab.enable_actor_uids(library).unwrap();
    (lab, profile)
}

#[test]
fn loopback_pawn_uids_are_distinct_avoid_saved_uids_and_reuse_after_close() {
    use icarus_bevy::actor_uids::ActorUidLibrary;
    // Force bounded random/sequential fallbacks: first gets 7, second gets 8;
    // after the first closes, a later random draw can grant 7 again.
    let (mut lab, profile) = uid_lab(ActorUidLibrary::with_random(|| 7));
    let pinned = profile.pin.network_checksum;
    let seeded: Vec<_> = (65_536..=65_540).collect();
    let payload = uid_save_payload(
        &seeded
            .iter()
            .map(|uid| vec![scalar::Tag::integer("IcarusActorGUID", *uid)])
            .collect::<Vec<_>>(),
    );
    lab.seed_actor_uids(icarus_bevy::prospect::saved_actor_uids_from_payload(&payload).unwrap())
        .unwrap();
    let mut app = build_app(&ServerConfig::default(), &WorldSnapshot::default()).unwrap();
    let address = network::install(&mut app, lab).unwrap();
    let ((first, first_bytes, second, second_bytes), _) = with_server(&mut app, move || {
        let (first, first_bytes) = join_pinned_open(address, pinned, 2);
        let (second, second_bytes) = join_pinned_open(address, pinned, 2);
        (first, first_bytes, second, second_bytes)
    });
    let first_uid = sent_pawn_uid(&profile, &first_bytes).unwrap();
    let second_uid = sent_pawn_uid(&profile, &second_bytes).unwrap();
    assert_eq!((first_uid, second_uid), (7, 8));
    assert!(!seeded.contains(&first_uid) && !seeded.contains(&second_uid));
    with_server(&mut app, move || first.close().unwrap());
    assert_eq!(counts(&app).closed_by_client, 1);
    let ((third, third_bytes), _) =
        with_server(&mut app, move || join_pinned_open(address, pinned, 2));
    assert_eq!(sent_pawn_uid(&profile, &third_bytes), Some(first_uid));
    with_server(&mut app, move || {
        second.close().unwrap();
        third.close().unwrap();
    });
    assert_eq!(counts(&app).closed_by_client, 3);
    assert_eq!(
        app.world().resource::<PeerLab>().stats().pawn_open_errors,
        0
    );
}

/// Join one pinned peer from a socket bound to `local`, returning the client.
fn join_pinned_from(local: SocketAddr, server: SocketAddr, version: u32) -> ProbeClient {
    let socket = UdpSocket::bind(local).unwrap();
    socket.connect(server).unwrap();
    let mut client = ProbeClient::connect(socket, probe_config()).unwrap();
    client.hello(version, "").unwrap();
    client.send_control(&[login()], false).unwrap();
    client.collect(Duration::from_secs(3), 1).unwrap();
    let (_, packet_id) = client
        .send_control(
            &[
                ControlMessage::Netspeed { rate: 100_000 },
                ControlMessage::Join,
            ],
            false,
        )
        .unwrap();
    let collected = client.collect(Duration::from_secs(3), 2).unwrap();
    assert!(collected.acked.contains(&packet_id), "Join acknowledged");
    client
}

fn claimed(app: &App) -> Vec<i32> {
    app.world()
        .resource::<PeerLab>()
        .claimed_actor_uids()
        .to_vec()
}

#[test]
fn pawn_uids_are_freed_on_idle_expiry_replacement_and_shutdown() {
    use icarus_bevy::actor_uids::{ActorUidLibrary, FIRST_NATIVE_OBJECT_ID};
    let mut config = network::peer_lab_config(&enabled(0));
    config.idle_timeout = Duration::from_millis(150);
    // One claim slot: a leaked UID would refuse every later pawn open.
    let (lab, profile) = uid_lab_with(ActorUidLibrary::with_limit(1, || 7).unwrap(), config);
    let pinned = profile.pin.network_checksum;
    let mut app = build_app(&ServerConfig::default(), &WorldSnapshot::default()).unwrap();
    let server = network::install(&mut app, lab).unwrap();

    // Idle expiry.
    let (client, _) = with_server(&mut app, move || join_pinned_open(server, pinned, 2).0);
    assert_eq!(claimed(&app), [FIRST_NATIVE_OBJECT_ID]);
    thread::sleep(Duration::from_millis(300));
    app.update();
    assert_eq!(counts(&app).closed_by_timeout, 1);
    assert!(claimed(&app).is_empty());
    drop(client);

    // Replacement: the same address completes a new handshake.
    let local = UdpSocket::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap();
    let (client, _) = with_server(&mut app, move || join_pinned_from(local, server, pinned));
    assert_eq!(claimed(&app), [FIRST_NATIVE_OBJECT_ID + 1]);
    drop(client);
    let (client, _) = with_server(&mut app, move || {
        let socket = UdpSocket::bind(local).unwrap();
        socket.connect(server).unwrap();
        ProbeClient::connect(socket, probe_config()).unwrap()
    });
    assert_eq!(app.world().resource::<PeerLab>().stats().peers_replaced, 1);
    assert!(claimed(&app).is_empty());
    drop(client);

    // Shutdown.
    let (client, _) = with_server(&mut app, move || join_pinned_open(server, pinned, 2).0);
    assert_eq!(claimed(&app), [FIRST_NATIVE_OBJECT_ID + 2]);
    // The unjoined replacement connection is still live, too.
    assert_eq!(app.world_mut().resource_mut::<PeerLab>().shutdown(), 2);
    assert!(claimed(&app).is_empty());
    assert_eq!(
        app.world().resource::<PeerLab>().stats().pawn_open_errors,
        0
    );
    drop(client);
}

#[test]
fn loopback_pawn_uid_capacity_and_nonpositive_grants_refuse_the_open() {
    use icarus_bevy::actor_uids::ActorUidLibrary;
    for (index, library) in [
        ActorUidLibrary::with_limit(0, || 0).unwrap(),
        ActorUidLibrary::with_random(|| 0),
    ]
    .into_iter()
    .enumerate()
    {
        let (mut lab, profile) = uid_lab(library);
        if index == 1 {
            lab.seed_actor_uids([65_536, 65_537]).unwrap();
        }
        let pinned = profile.pin.network_checksum;
        let mut app = build_app(&ServerConfig::default(), &WorldSnapshot::default()).unwrap();
        let address = network::install(&mut app, lab).unwrap();
        let ((client, bytes), _) =
            with_server(&mut app, move || join_pinned_open(address, pinned, 1));
        assert_eq!(sent_pawn_uid(&profile, &bytes), None);
        let stats = app.world().resource::<PeerLab>().stats();
        assert_eq!(stats.player_controller_opens_sent, 1);
        assert_eq!(stats.pawn_opens_sent, 0);
        assert_eq!(stats.pawn_open_errors, 1);
        with_server(&mut app, move || client.close().unwrap());
    }
}

/// Compose the GameState open the lab sends after each PlayerController open
/// (F-794), with `world_time` for the lab-written handle 19.
fn compose_lab_game_state(
    profile: &ReleaseProfile,
    cache: &mut icarus_bevy::experimental::session_open::NetGuidCache,
    exports: &mut icarus_bevy::experimental::session_open::ConnectionExports,
    world_time: f32,
    rich_presence: Option<icarus_bevy::experimental::rep_layout::RepValue>,
) -> icarus_bevy::experimental::session_open::ComposedActorOpen {
    use icarus_bevy::experimental::{
        archive::ByteOrder,
        game_state_open::{GAME_STATE_CHANNEL_INDEX, compose_game_state_open, handle},
        peer_lab::PeerLabAdmission,
        rep_layout::RepValue,
    };
    let plan = profile
        .game_state_open_plan(PeerLabAdmission::default().map_url)
        .unwrap()
        .unwrap();
    let mut state = network::game_state_values().unwrap();
    state.values.insert(
        handle::WORLD_TIME_SECONDS,
        RepValue::Float(world_time.to_bits()),
    );
    // Each run draws its own RichPresenceGUID (F-912): compose with the
    // sent one when comparing bits.
    if let Some(value) = rich_presence {
        state.values.insert(handle::RICH_PRESENCE_GUID, value);
    }
    // The lab references its managers from the GameState (F-859).
    for (slot, _) in lab_manager_plans(profile) {
        slot.reference_from(&mut state.managers);
    }
    compose_game_state_open(
        cache,
        exports,
        &plan,
        &state,
        GAME_STATE_CHANNEL_INDEX,
        ByteOrder::Little,
    )
    .unwrap()
}

/// The lab's world manager plans (F-859).
fn lab_manager_plans(
    profile: &ReleaseProfile,
) -> Vec<(
    icarus_bevy::experimental::manager_open::ManagerSlot,
    icarus_bevy::experimental::manager_open::ManagerOpenPlan,
)> {
    profile
        .manager_open_plans(
            icarus_bevy::experimental::peer_lab::PeerLabAdmission::default().map_url,
        )
        .unwrap()
}

/// Compose the lab's manager opens after a connection's PlayerState, on
/// consecutive channels from the first manager channel.
fn compose_lab_managers(
    profile: &ReleaseProfile,
    cache: &mut icarus_bevy::experimental::session_open::NetGuidCache,
    exports: &mut icarus_bevy::experimental::session_open::ConnectionExports,
) -> Vec<icarus_bevy::experimental::session_open::ComposedActorOpen> {
    use icarus_bevy::experimental::{archive::ByteOrder, manager_open::compose_manager_open};
    lab_manager_plans(profile)
        .into_iter()
        .zip(network::MANAGER_FIRST_CHANNEL..)
        .map(|((slot, plan), channel)| {
            compose_manager_open(
                cache,
                exports,
                &plan,
                slot.key(),
                channel,
                ByteOrder::Little,
            )
            .unwrap()
        })
        .collect()
}

#[test]
fn pinned_join_is_answered_with_the_composed_player_controller_open() {
    use icarus_bevy::experimental::{
        actor_channel::{
            ActorChannelCodec, ActorChannelLimits, PackageMapState,
            parse_player_controller_open_bunch,
        },
        archive::ByteOrder,
        packet::{Bunch, read_packet},
        peer_lab::PeerLabAdmission,
        session_open::{
            ConnectionExports, NetGuidCache, PLAYER_CONTROLLER_CHANNEL_INDEX,
            compose_player_controller_open,
        },
    };
    let profile = ReleaseProfile::load(&release_toml()).unwrap();
    let pinned = profile.pin.network_checksum;
    let (mut app, address) = server_app(&NetworkConfig {
        release_profile: Some(release_toml()),
        ..enabled(0)
    });
    let (datagrams, _) = with_server(&mut app, move || {
        [join_pinned(address, pinned), join_pinned(address, pinned)]
    });
    let counts = counts(&app);
    assert_eq!(
        (counts.joins_succeeded, counts.player_controller_opens_sent),
        (2, 2)
    );
    assert_eq!(
        app.world()
            .resource::<PeerLab>()
            .stats()
            .player_controller_open_errors,
        0
    );

    // The same plan composed independently, for both connections in order.
    let plan = profile
        .player_controller_open_plan(PeerLabAdmission::default().map_url, network::SPAWN_LOCATION)
        .unwrap()
        .unwrap();
    let mut cache = NetGuidCache::new();
    cache.register_level(&plan.profile.level);
    let codec = ActorChannelCodec {
        engine_net_version: profile.engine_network_version,
        exports_enabled: false,
        has_level: true,
        order: ByteOrder::Little,
        limits: ActorChannelLimits::default(),
    };
    let to_client = PacketConfig {
        receiver: PacketReceiver::Client,
        ..probe_config().packet
    };
    for (index, datagram) in datagrams.iter().enumerate() {
        let mut exports = ConnectionExports::new();
        let expected =
            compose_player_controller_open(&mut cache, &mut exports, &plan, 0, ByteOrder::Little)
                .unwrap();
        // The lab then opens the pawn (17 GUIDs per connection), the
        // GameState (4 GUIDs and its 3 managers', once) and the managers (5
        // component GUIDs, once).
        compose_lab_pawn(
            &profile,
            &mut cache,
            &mut exports,
            index as u32 + 1,
            &expected.guids,
        );
        compose_lab_game_state(&profile, &mut cache, &mut exports, 0.0, None);
        compose_lab_managers(&profile, &mut cache, &mut exports);
        // Each connection gets its own dynamic GUIDs: actor 2, then 78.
        assert_eq!(expected.guids.actor, 2 + 76 * index as u32);
        let packet = read_packet(datagram, &to_client).unwrap();
        let [exports, body] = packet.bunches.as_slice() else {
            panic!("the open travels as exports plus body");
        };
        for part in [exports, body] {
            assert_eq!(part.header.channel_index, PLAYER_CONTROLLER_CHANNEL_INDEX);
            assert!(part.header.reliable && part.header.partial);
        }
        assert!(exports.header.open && exports.header.has_package_map_exports);
        assert!(body.header.partial_final && !body.header.has_package_map_exports);
        assert_eq!(exports.payload, expected.exports);
        assert_eq!(body.payload, expected.bunch.payload);

        // Each connection receives the static paths in full and resolves them.
        let mut map = PackageMapState::new();
        map.consume_export_prefix(&mut exports.payload.reader(), &codec)
            .unwrap();
        assert_eq!(
            map.resolve_full_path(expected.guids.archetype).as_deref(),
            Some(
                "/Game/BP/Player/BP_IcarusPlayerControllerSurvival.Default__BP_IcarusPlayerControllerSurvival_C"
            )
        );
        let whole = Bunch {
            header: expected.bunch.header.clone(),
            payload: body.payload.clone(),
        };
        let parsed = parse_player_controller_open_bunch(&whole, &codec).unwrap();
        assert_eq!(parsed.net_player_index, 0);
        assert_eq!(parsed.blocks.len(), 7, "actor block plus six components");
    }
}

#[test]
fn pinned_probe_sends_begin_player_initialisation_on_actor_channel() {
    use icarus_bevy::experimental::{
        actor_channel::{ActorChannelCodec, ActorChannelLimits},
        archive::ByteOrder,
        packet::{BitWriter, Payload},
        rep_rpc::{
            BP_ICARUS_PLAYER_CONTROLLER_SURVIVAL_FIELD_INDEX_BOUND, FieldHeader,
            SERVER_BEGIN_PLAYER_INITIALISATION_FIELD_INDEX,
        },
        session_open::PLAYER_CONTROLLER_CHANNEL_INDEX,
    };
    let profile = ReleaseProfile::load(&release_toml()).unwrap();
    let pinned = profile.pin.network_checksum;
    let (mut app, address) = server_app(&NetworkConfig {
        release_profile: Some(release_toml()),
        ..enabled(0)
    });
    let (mut client, _) = with_server(&mut app, move || {
        let (mut client, _) = join_pinned_open(address, pinned, 1);
        let mut parameter = BitWriter::new();
        parameter.write_bit(true);
        parameter.write_bytes(&3_i32.to_le_bytes());
        let (_, packet_id) = client
            .send_actor_rpc(
                PLAYER_CONTROLLER_CHANNEL_INDEX,
                &FieldHeader {
                    index: SERVER_BEGIN_PLAYER_INITIALISATION_FIELD_INDEX,
                    payload: Payload::from_writer(parameter),
                },
                BP_ICARUS_PLAYER_CONTROLLER_SURVIVAL_FIELD_INDEX_BOUND,
                &ActorChannelCodec {
                    engine_net_version: profile.engine_network_version,
                    exports_enabled: false,
                    has_level: true,
                    order: ByteOrder::Little,
                    limits: ActorChannelLimits::default(),
                },
            )
            .unwrap();
        let replies = client.collect(Duration::from_secs(3), 32).unwrap();
        assert!(replies.acked.contains(&packet_id));
        client
    });
    let snapshot = counts(&app);
    assert_eq!(
        (snapshot.joined, snapshot.initialisation_slots_recorded),
        (1, 1)
    );
    assert_eq!(snapshot.initialisation_calls_ignored, 0);
    with_server(&mut app, move || {
        client.keepalive().unwrap();
    });
    assert_eq!(counts(&app).joined, 1);
}

#[test]
fn teardown_sends_host_closed_failure_then_control_close() {
    use icarus_bevy::experimental::{
        packet::read_packet,
        peer_lab::{CLOSE_REASON_DESTROYED, HOST_CLOSED_CONNECTION_MESSAGE},
    };
    let (mut app, address) = server_app(&enabled(0));
    // One joined session and one that only completed the handshake.
    let ((mut joined, mut silent), _) =
        with_server(&mut app, move || (join(address), connect(address)));
    let counts = network::teardown(app.world_mut()).unwrap();
    assert_eq!(counts.active_peers, 0);
    assert_eq!(counts.closed_by_shutdown, 2);
    // Both get the Failure; only the joined one delivered a reliable bunch,
    // so only it gets the control close.
    assert_eq!(
        (counts.shutdown_failures_sent, counts.shutdown_closes_sent),
        (2, 1)
    );

    let to_client = PacketConfig {
        receiver: PacketReceiver::Client,
        ..probe_config().packet
    };
    let collected = joined.collect(Duration::from_secs(2), 2).unwrap();
    assert_eq!(collected.replies.len(), 2);
    let messages: Vec<_> = collected.messages().map(failure_text).collect();
    assert_eq!(messages, [HOST_CLOSED_CONNECTION_MESSAGE]);
    let close = read_packet(&collected.replies[1].bytes, &to_client).unwrap();
    let [bunch] = close.bunches.as_slice() else {
        panic!("one close bunch");
    };
    assert!(bunch.header.close && bunch.header.reliable);
    assert_eq!(bunch.header.channel_index, 0);
    assert_eq!(bunch.header.close_reason, Some(CLOSE_REASON_DESTROYED));

    let collected = silent.collect(Duration::from_millis(500), 2).unwrap();
    let messages: Vec<_> = collected.messages().map(failure_text).collect();
    assert_eq!(messages, [HOST_CLOSED_CONNECTION_MESSAGE]);
    assert_eq!(collected.replies.len(), 1);
    // Idempotent: a repeated teardown sends nothing more.
    assert_eq!(network::teardown(app.world_mut()).unwrap(), counts);
}

/// The world time the lab wrote into a GameState open body (handle 19).
fn sent_world_time(
    profile: &ReleaseProfile,
    body: &icarus_bevy::experimental::packet::Payload,
) -> f32 {
    use icarus_bevy::experimental::{game_state_open::handle, rep_layout::RepValue};
    let RepValue::Float(bits) = sent_game_state_value(profile, body, handle::WORLD_TIME_SECONDS)
    else {
        panic!("world time is a float");
    };
    f32::from_bits(bits)
}

/// The value a sent GameState open body carries for `wanted`.
fn sent_game_state_value(
    profile: &ReleaseProfile,
    body: &icarus_bevy::experimental::packet::Payload,
    wanted: u32,
) -> icarus_bevy::experimental::rep_layout::RepValue {
    use icarus_bevy::experimental::{
        actor_channel::{ActorChannelCodec, ActorChannelLimits, read_actor_bunch_body},
        archive::ByteOrder,
        rep_layout::{RepLayoutCodec, RepLayoutLimits, read_rep_properties},
    };
    let codec = ActorChannelCodec {
        engine_net_version: profile.engine_network_version,
        exports_enabled: false,
        has_level: true,
        order: ByteOrder::Little,
        limits: ActorChannelLimits::default(),
    };
    let open = read_actor_bunch_body(&mut body.reader(), true, &codec).unwrap();
    let plan = profile
        .game_state_open_plan(
            icarus_bevy::experimental::peer_lab::PeerLabAdmission::default().map_url,
        )
        .unwrap()
        .unwrap();
    let rep_codec = RepLayoutCodec {
        actor: codec,
        checksums: false,
        limits: RepLayoutLimits::default(),
    };
    let stream = read_rep_properties(
        &mut open.blocks[0].payload.reader(),
        &plan.layout,
        &rep_codec,
    )
    .unwrap();
    stream
        .entries
        .into_iter()
        .find(|entry| entry.handle == wanted)
        .unwrap()
        .value
}

#[test]
fn pinned_joins_open_the_game_state_then_each_player_state() {
    use icarus_bevy::experimental::{
        actor_channel::{ActorChannelCodec, ActorChannelLimits, read_actor_bunch_body},
        archive::ByteOrder,
        control_messages::{ArchiveText, NetIdType},
        game_state_open::GAME_STATE_CHANNEL_INDEX,
        packet::read_packet,
        pawn_open::PAWN_CHANNEL_INDEX,
        peer_lab::{FIRST_PLAYER_ID, PeerLabAdmission},
        session_open::{
            ConnectionExports, NetGuidCache, PLAYER_STATE_CHANNEL_INDEX, PlayerStateOpenState,
            compose_player_controller_open, compose_player_state_open, replicated_unique_net_id,
        },
    };
    let profile = ReleaseProfile::load(&release_toml()).unwrap();
    let pinned = profile.pin.network_checksum;
    let (mut app, address) = server_app(&NetworkConfig {
        release_profile: Some(release_toml()),
        ..enabled(0)
    });
    let (replies, _) = with_server(&mut app, move || {
        [
            join_pinned_replies(address, pinned, 7),
            join_pinned_replies(address, pinned, 7),
        ]
    });
    let stats = app.world().resource::<PeerLab>().stats().clone();
    assert_eq!(
        (
            stats.player_state_opens_sent,
            stats.player_state_open_errors
        ),
        (2, 0)
    );

    assert_eq!(
        (stats.game_state_opens_sent, stats.game_state_open_errors),
        (2, 0)
    );
    assert_eq!(
        (stats.manager_opens_sent, stats.manager_open_errors),
        (6, 0)
    );

    // Independent composition in the lab's order: each player's
    // PlayerController open, the GameState, then its PlayerState open.
    let admission = PeerLabAdmission::default();
    let controller = profile
        .player_controller_open_plan(admission.map_url, network::SPAWN_LOCATION)
        .unwrap()
        .unwrap();
    let state_plan = profile
        .player_state_open_plan(admission.map_url)
        .unwrap()
        .unwrap();
    let mut cache = NetGuidCache::new();
    cache.register_level(&controller.profile.level);
    let ControlMessage::Login { unique_net_id, .. } = login() else {
        unreachable!()
    };
    let unique_id = replicated_unique_net_id(&unique_net_id, admission.indexed_subsystems);
    assert_eq!(unique_id.kind, NetIdType::Indexed(3), "STEAM by index");
    let to_client = PacketConfig {
        receiver: PacketReceiver::Client,
        ..probe_config().packet
    };
    for (index, datagrams) in replies.iter().enumerate() {
        assert_eq!(
            datagrams.len(),
            7,
            "controller, pawn, game state, player state, three managers"
        );
        let mut exports = ConnectionExports::new();
        let controller_open = compose_player_controller_open(
            &mut cache,
            &mut exports,
            &controller,
            0,
            ByteOrder::Little,
        )
        .unwrap();
        let pawn = compose_lab_pawn(
            &profile,
            &mut cache,
            &mut exports,
            index as u32 + 1,
            &controller_open.guids,
        );
        let packet = read_packet(&datagrams[1], &to_client).unwrap();
        let [pawn_exports, pawn_body] = packet.bunches.as_slice() else {
            panic!("the pawn open travels as exports plus body");
        };
        assert_eq!(pawn_body.header.channel_index, PAWN_CHANNEL_INDEX);
        assert_eq!(Some(&pawn_exports.payload), pawn.exports.as_ref());
        assert_eq!(pawn_body.payload, pawn.bunch.payload);
        // The GameState carries the lab's world time; compose with the sent
        // value and compare everything else bit for bit.
        let packet = read_packet(&datagrams[2], &to_client).unwrap();
        let [game_exports, game_body] = packet.bunches.as_slice() else {
            panic!("the GameState open travels as exports plus body");
        };
        assert_eq!(game_body.header.channel_index, GAME_STATE_CHANNEL_INDEX);
        let world_time = sent_world_time(&profile, &game_body.payload);
        let rich_presence = sent_game_state_value(
            &profile,
            &game_body.payload,
            icarus_bevy::experimental::game_state_open::handle::RICH_PRESENCE_GUID,
        );
        let game_state = compose_lab_game_state(
            &profile,
            &mut cache,
            &mut exports,
            world_time,
            Some(rich_presence),
        );
        assert_eq!(Some(&game_exports.payload), game_state.exports.as_ref());
        assert_eq!(game_body.payload, game_state.bunch.payload);
        // One GameState: the same actor GUID on every connection.
        let codec = ActorChannelCodec {
            engine_net_version: profile.engine_network_version,
            exports_enabled: false,
            has_level: true,
            order: ByteOrder::Little,
            limits: ActorChannelLimits::default(),
        };
        let spawn = read_actor_bunch_body(&mut game_body.payload.reader(), true, &codec)
            .unwrap()
            .spawn
            .unwrap();
        assert_eq!(spawn.actor.guid, 54, "after the first pawn's 20..52");
        let state = PlayerStateOpenState {
            player_state: controller_open.guids.player_state,
            owner: controller_open.guids.actor,
            player_id: FIRST_PLAYER_ID + index as i32,
            unique_id: unique_id.clone(),
            player_name: ArchiveText::text(NAME_SENTINEL).unwrap(),
            // Each session closes before the next joins, so each joins with
            // no other player and is host (F-913).
            is_host: true,
        };
        let expected = compose_player_state_open(
            &mut cache,
            &mut exports,
            &state_plan,
            &state,
            ByteOrder::Little,
        )
        .unwrap();
        let packet = read_packet(&datagrams[3], &to_client).unwrap();
        let [state_exports, body] = packet.bunches.as_slice() else {
            panic!("the PlayerState open travels as exports plus body");
        };
        assert_eq!(body.header.channel_index, PLAYER_STATE_CHANNEL_INDEX);
        assert!(state_exports.header.open && state_exports.header.has_package_map_exports);
        assert_eq!(Some(&state_exports.payload), expected.exports.as_ref());
        assert_eq!(body.payload, expected.bunch.payload);
        // Then the GameState's managers on the next channels (F-859), the
        // same actors on every connection.
        let managers = compose_lab_managers(&profile, &mut cache, &mut exports);
        for ((manager, datagram), channel) in managers
            .iter()
            .zip(&datagrams[4..])
            .zip(network::MANAGER_FIRST_CHANNEL..)
        {
            let packet = read_packet(datagram, &to_client).unwrap();
            let [manager_exports, manager_body] = packet.bunches.as_slice() else {
                panic!("a manager open travels as exports plus body");
            };
            assert_eq!(manager_body.header.channel_index, channel);
            assert_eq!(Some(&manager_exports.payload), manager.exports.as_ref());
            assert_eq!(manager_body.payload, manager.bunch.payload);
        }
    }
}

#[test]
fn replication_selections_open_archetyped_actors_on_free_channels() {
    use icarus_bevy::{
        experimental::{
            actor_channel::{
                ActorChannelCodec, ActorChannelLimits, PackageMapState, read_actor_bunch_body,
            },
            archive::ByteOrder,
            packet::read_packet,
            peer_lab::PeerLabAdmission,
            rep_layout::{
                MovementLevel, RepLayoutCodec, RepLayoutLimits, RepValue, RotationLevel,
                actor_rep_layout, read_rep_properties,
            },
            session_open::{PathSegment, map_level_chain},
        },
        replication::{NetActor, NetParameters, ReplicationPlugin},
        session_replication::{
            NetArchetype, NetMotion, SessionReplication, SessionReplicationPlugin,
        },
        simulation::{DomainId, Position},
    };
    let profile = ReleaseProfile::load(&release_toml()).unwrap();
    let pinned = profile.pin.network_checksum;
    let (mut app, address) = server_app(&NetworkConfig {
        release_profile: Some(release_toml()),
        ..enabled(0)
    });
    let level = map_level_chain(PeerLabAdmission::default().map_url).unwrap();
    app.add_plugins((ReplicationPlugin::default(), SessionReplicationPlugin))
        .insert_resource(
            SessionReplication::new(
                level,
                profile.engine_network_version,
                2,
                network::SPAWN_LOCATION,
            )
            .unwrap(),
        );
    // Authored identifiers only: two actors with an archetype (one beyond the
    // default cull distance, yet force-relevant on the connection's first
    // processed frame, F-772) and one without.
    let archetype = vec![
        PathSegment::new("Default__LabCrate_C", 0x1234_5678, false),
        PathSegment::new("/Game/Lab/LabCrate", 0, false),
    ];
    for (id, location, archetyped) in [
        (42, [100.0, 0.0, 0.0], true),
        (43, [200.0, 0.0, 0.0], false),
        (44, [20_000.0, 0.0, 0.0], true),
    ] {
        let mut entity = app.world_mut().spawn((
            NetActor {
                id: DomainId(id),
                params: NetParameters::default(),
            },
            Position(location),
        ));
        if archetyped {
            entity.insert(NetArchetype {
                archetype: archetype.clone(),
                replicate_movement: false,
            });
        }
        if id == 42 {
            // Spawn motion is independent of bReplicateMovement. This actor
            // still carries only roles and emits no movement updates.
            entity.insert(NetMotion {
                rotation_degrees: [1.0, 90.0, -45.0],
                linear_velocity: [5.25, 0.0, -8.25],
            });
        }
    }
    // Two sessions in turn: each gets both actors, under the same GUIDs.
    let ([replies, later], _) = with_server(&mut app, move || {
        [
            join_pinned_replies(address, pinned, 9),
            join_pinned_replies(address, pinned, 9),
        ]
    });
    let stats = app.world().resource::<PeerLab>().stats().clone();
    assert_eq!((stats.actor_opens_sent, stats.actor_open_errors), (4, 0));
    assert_eq!(stats.actor_updates_sent, 0);
    assert_eq!(
        replies.len(),
        9,
        "controller, pawn, game state, player state, three managers, then two actors"
    );
    assert_eq!(later.len(), 9);

    let to_client = PacketConfig {
        receiver: PacketReceiver::Client,
        ..probe_config().packet
    };
    // Channels 2 to 5 hold the PlayerController, pawn, GameState and
    // PlayerState, 6 to 8 the managers (F-859);
    // the actors take the next free ones. The second open exports the same
    // archetype again unless the client acked the first one's packet in
    // between (F-796).
    let packet = read_packet(&replies[7], &to_client).unwrap();
    let [exports, body] = packet.bunches.as_slice() else {
        panic!("the actor open travels as exports plus body");
    };
    assert_eq!(body.header.channel_index, 9);
    let second = read_packet(&replies[8], &to_client).unwrap();
    let second_body = second.bunches.last().unwrap();
    assert_eq!(second_body.header.channel_index, 10);
    match second.bunches.as_slice() {
        [_] => {}
        [again, _] => assert!(again.header.has_package_map_exports),
        other => panic!("{} bunches in the second open", other.len()),
    }
    let codec = ActorChannelCodec {
        engine_net_version: profile.engine_network_version,
        exports_enabled: false,
        has_level: true,
        order: ByteOrder::Little,
        limits: ActorChannelLimits::default(),
    };
    let mut map = PackageMapState::new();
    let decoded = map
        .consume_export_prefix(&mut exports.payload.reader(), &codec)
        .unwrap();
    assert_eq!(
        map.resolve_full_path(decoded[0].guid).as_deref(),
        Some("/Game/Lab/LabCrate.Default__LabCrate_C")
    );
    let open = read_actor_bunch_body(&mut body.payload.reader(), true, &codec).unwrap();
    let spawn = open.spawn.unwrap();
    // The first connection's controller took dynamic GUIDs 2..18, its pawn
    // 20..52, the GameState 54..60, its managers 62..66 and their
    // components 68..76.
    assert_eq!(spawn.actor.guid, 78);
    let dynamic = spawn.dynamic.unwrap();
    assert!(dynamic.location.is_some());
    let rotation = dynamic.rotation.unwrap().degrees();
    assert!((rotation[0] - 1.0).abs() < 360.0 / 65536.0);
    assert_eq!(rotation[1..], [90.0, 315.0]);
    let velocity = dynamic.velocity.unwrap().decode();
    assert!((velocity[0] - 5.3).abs() < 0.0001);
    assert!((velocity[2] + 8.2).abs() < 0.0001);
    // One actor block with the captured world-actor roles (F-791).
    let [roles] = open.blocks.as_slice() else {
        panic!("one role block");
    };
    let rep_codec = RepLayoutCodec {
        actor: codec,
        checksums: false,
        limits: RepLayoutLimits::default(),
    };
    let layout = actor_rep_layout(
        MovementLevel::Whole,
        MovementLevel::Whole,
        RotationLevel::Bytes,
    );
    let stream = read_rep_properties(&mut roles.payload.reader(), &layout, &rep_codec).unwrap();
    let [remote_role, role] = stream.entries.as_slice() else {
        panic!("two role entries");
    };
    assert_eq!((remote_role.handle, role.handle), (5, 14));
    assert!(matches!(remote_role.value, RepValue::Byte(1)));
    assert!(matches!(role.value, RepValue::Byte(3)));
    let actor_guids = |datagrams: &[Vec<u8>]| -> Vec<u32> {
        let mut guids: Vec<u32> = datagrams[7..]
            .iter()
            .map(|datagram| {
                let packet = read_packet(datagram, &to_client).unwrap();
                let body = packet.bunches.last().unwrap();
                read_actor_bunch_body(&mut body.payload.reader(), true, &codec)
                    .unwrap()
                    .spawn
                    .unwrap()
                    .actor
                    .guid
            })
            .collect();
        guids.sort_unstable();
        guids
    };
    assert_eq!(actor_guids(&replies), [78, 80]);
    assert_eq!(actor_guids(&later), [78, 80], "driver-wide GUIDs");
}

#[test]
fn replication_closes_actors_that_stop_being_relevant() {
    use icarus_bevy::{
        experimental::{
            packet::read_packet,
            peer_lab::PeerLabAdmission,
            session_open::{PathSegment, map_level_chain},
        },
        replication::{NetActor, NetParameters, ReplicationConfig, ReplicationPlugin},
        session_replication::{NetArchetype, SessionReplication, SessionReplicationPlugin},
        simulation::{DomainId, Position},
    };
    let profile = ReleaseProfile::load(&release_toml()).unwrap();
    let pinned = profile.pin.network_checksum;
    let (mut app, address) = server_app(&NetworkConfig {
        release_profile: Some(release_toml()),
        ..enabled(0)
    });
    // A short authored relevancy timeout past the 1 s re-check window: the
    // near actor is re-checked and refreshed, the far one (only forced on the
    // first frame, F-772) closes once the timeout passes (F-749, F-788).
    app.add_plugins((
        ReplicationPlugin {
            config: ReplicationConfig {
                relevant_timeout_seconds: 1.5,
                ..ReplicationConfig::default()
            },
        },
        SessionReplicationPlugin,
    ))
    .insert_resource(
        SessionReplication::new(
            map_level_chain(PeerLabAdmission::default().map_url).unwrap(),
            profile.engine_network_version,
            2,
            network::SPAWN_LOCATION,
        )
        .unwrap(),
    );
    let archetype = vec![
        PathSegment::new("Default__LabCrate_C", 0x1234_5678, false),
        PathSegment::new("/Game/Lab/LabCrate", 0, false),
    ];
    for (id, location) in [(42, [100.0, 0.0, 0.0]), (44, [20_000.0, 0.0, 0.0])] {
        app.world_mut().spawn((
            NetActor {
                id: DomainId(id),
                params: NetParameters::default(),
            },
            Position(location),
            NetArchetype {
                archetype: archetype.clone(),
                replicate_movement: false,
            },
        ));
    }
    let (replies, _) = with_server(&mut app, move || {
        let mut client = connect(address);
        let report = client.hello(pinned, "").unwrap();
        assert!(report.hello_acked);
        client.send_control(&[login()], false).unwrap();
        client.collect(Duration::from_secs(3), 1).unwrap();
        client.send_control(&[ControlMessage::Join], false).unwrap();
        // Keep the session alive and acknowledged for 3 s, past the far
        // actor's relevancy timeout.
        let mut replies = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline {
            let collected = client.collect(Duration::from_millis(200), 64).unwrap();
            replies.extend(collected.replies.into_iter().map(|reply| reply.bytes));
            client.keepalive().unwrap();
        }
        client.close().unwrap();
        replies
    });
    let stats = app.world().resource::<PeerLab>().stats().clone();
    assert_eq!((stats.actor_opens_sent, stats.actor_closes_sent), (2, 1));
    assert_eq!(stats.actor_close_errors, 0);
    let to_client = PacketConfig {
        receiver: PacketReceiver::Client,
        ..probe_config().packet
    };
    let closes: Vec<_> = replies
        .iter()
        .flat_map(|datagram| read_packet(datagram, &to_client).unwrap().bunches)
        .filter(|bunch| bunch.header.close)
        .collect();
    let [close] = closes.as_slice() else {
        panic!("one actor channel close");
    };
    assert!(close.header.reliable && close.header.channel_index >= 4);
    assert_eq!(close.header.close_reason, Some(3), "no longer relevant");
}

#[test]
fn replicated_authored_flags_and_movement_open_and_update_together() {
    use bevy::prelude::{FixedUpdate, Query, Res};
    use icarus_bevy::{
        experimental::{
            actor_channel::{ActorChannelCodec, ActorChannelLimits, read_actor_bunch_body},
            archive::ByteOrder,
            packet::read_packet,
            peer_lab::PeerLabAdmission,
            rep_layout::{
                MovementLevel, RepLayoutCodec, RepLayoutLimits, RepValue, RotationLevel,
                actor_rep_layout, read_rep_properties,
            },
            session_open::{PathSegment, map_level_chain},
        },
        replication::{NetActor, NetActorFlags, NetParameters, ReplicationPlugin, ReplicationSet},
        session_replication::{NetArchetype, SessionReplication, SessionReplicationPlugin},
        simulation::{DomainId, Position},
    };
    /// Authored producer runs before selection; hiding keeps its colliding root.
    fn mover(time: Res<bevy::time::Time>, mut actors: Query<(&mut Position, &mut NetActorFlags)>) {
        if time.elapsed_secs() > 1.0 {
            for (mut position, mut flags) in &mut actors {
                position.0 = [500.0, -20.0, 30.0];
                *flags = NetActorFlags {
                    hidden: false,
                    can_be_damaged: true,
                };
            }
        }
    }
    let profile = ReleaseProfile::load(&release_toml()).unwrap();
    let pinned = profile.pin.network_checksum;
    let (mut app, address) = server_app(&NetworkConfig {
        release_profile: Some(release_toml()),
        ..enabled(0)
    });
    app.add_plugins((ReplicationPlugin::default(), SessionReplicationPlugin))
        .add_systems(FixedUpdate, mover.before(ReplicationSet))
        .insert_resource(
            SessionReplication::new(
                map_level_chain(PeerLabAdmission::default().map_url).unwrap(),
                profile.engine_network_version,
                2,
                network::SPAWN_LOCATION,
            )
            .unwrap(),
        );
    app.world_mut().spawn((
        NetActor {
            id: DomainId(42),
            params: NetParameters::default(),
        },
        Position([100.0, 0.0, 0.0]),
        NetActorFlags {
            hidden: true,
            can_be_damaged: false,
        },
        NetArchetype {
            archetype: vec![
                PathSegment::new("Default__LabCart_C", 0x0cab_0cab, false),
                PathSegment::new("/Game/Lab/LabCart", 0, false),
            ],
            replicate_movement: true,
        },
    ));
    let (replies, _) = with_server(&mut app, move || {
        let mut client = connect(address);
        assert!(client.hello(pinned, "").unwrap().hello_acked);
        client.send_control(&[login()], false).unwrap();
        client.collect(Duration::from_secs(3), 1).unwrap();
        client.send_control(&[ControlMessage::Join], false).unwrap();
        // Keep the session acknowledged for 2.5 s, past the move.
        let mut replies = Vec::new();
        let deadline = Instant::now() + Duration::from_millis(2_500);
        while Instant::now() < deadline {
            let collected = client.collect(Duration::from_millis(200), 64).unwrap();
            replies.extend(collected.replies.into_iter().map(|reply| reply.bytes));
            client.keepalive().unwrap();
        }
        client.close().unwrap();
        replies
    });
    let stats = app.world().resource::<PeerLab>().stats().clone();
    assert_eq!((stats.actor_opens_sent, stats.actor_updates_sent), (1, 1));
    assert_eq!(stats.actor_update_errors, 0);
    assert_eq!(
        app.world().resource::<SessionReplication>().movement_errors,
        0
    );

    let to_client = PacketConfig {
        receiver: PacketReceiver::Client,
        ..probe_config().packet
    };
    let codec = ActorChannelCodec {
        engine_net_version: profile.engine_network_version,
        exports_enabled: false,
        has_level: true,
        order: ByteOrder::Little,
        limits: ActorChannelLimits::default(),
    };
    let rep_codec = RepLayoutCodec {
        actor: codec,
        checksums: false,
        limits: RepLayoutLimits::default(),
    };
    let layout = actor_rep_layout(
        MovementLevel::Whole,
        MovementLevel::Whole,
        RotationLevel::Bytes,
    );
    let bunches: Vec<_> = replies
        .iter()
        .flat_map(|datagram| read_packet(datagram, &to_client).unwrap().bunches)
        // Past the managers' channels 6..8 (F-859).
        .filter(|bunch| bunch.header.channel_index == 9)
        .collect();
    let location_of = |payload: &icarus_bevy::experimental::packet::Payload, open: bool| {
        let body = read_actor_bunch_body(&mut payload.reader(), open, &codec).unwrap();
        let stream =
            read_rep_properties(&mut body.blocks[0].payload.reader(), &layout, &rep_codec).unwrap();
        stream
            .entries
            .iter()
            .find_map(|entry| match &entry.value {
                RepValue::Movement(movement) => Some((entry.handle, movement.location.unbiased())),
                _ => None,
            })
            .unwrap()
    };
    // The open (its body part) carries the resting movement at the start.
    let open = bunches
        .iter()
        .find(|bunch| bunch.header.partial_final)
        .unwrap();
    assert_eq!(location_of(&open.payload, true), (6, [100, 0, 0]));
    // Then one unreliable update with the new location.
    let updates: Vec<_> = bunches
        .iter()
        .filter(|bunch| !bunch.header.reliable)
        .collect();
    let [update] = updates.as_slice() else {
        panic!("one movement update, got {}", updates.len());
    };
    assert!(!update.header.open && update.header.channel_name.is_none());
    assert_eq!(location_of(&update.payload, false), (6, [500, -20, 30]));
    for (bunch, is_open, expected) in [(open, true, [true, false]), (*update, false, [false, true])]
    {
        let body = read_actor_bunch_body(&mut bunch.payload.reader(), is_open, &codec).unwrap();
        let stream =
            read_rep_properties(&mut body.blocks[0].payload.reader(), &layout, &rep_codec).unwrap();
        for (handle, value) in [1, 4].into_iter().zip(expected) {
            assert_eq!(
                stream
                    .entries
                    .iter()
                    .find(|entry| entry.handle == handle)
                    .unwrap()
                    .value,
                RepValue::Bool(value)
            );
        }
    }
    assert_eq!(
        stats.actor_closes_sent, 0,
        "hiding alone keeps spatial relevance"
    );
}

/// A joined probe session held for three seconds keeps receiving GameState
/// updates: the world time every 0.1 s, never decreasing, every block
/// decoding (real binary, authored probe on loopback).
#[test]
fn a_held_session_receives_steady_game_state_updates() {
    use icarus_bevy::experimental::session_hold::{HoldConfig, run_hold};
    let directory = tempfile::tempdir().unwrap();
    let config = pinned_server_config(directory.path(), &release_toml());
    let state = directory.path().join("state");
    let mut server = spawn_run(&config, &state);
    let (address, _) = listening_address(&mut server);
    let profile = ReleaseProfile::load(&release_toml()).unwrap();
    let summary = run_hold(&HoldConfig {
        server: address,
        network_version: profile.pin.network_checksum,
        engine_net_version: profile.engine_network_version,
        duration: Duration::from_secs(3),
        step_timeout: Duration::from_secs(3),
    })
    .unwrap();
    sigterm(&server);
    assert!(server.wait_exit().success());
    assert!(summary.joined);
    assert_eq!(summary.decode_errors, 0);
    assert!(summary.world_time_updates >= 10, "{summary:?}");
    assert_eq!(summary.world_time_regressions, 0);
    assert_eq!(summary.level_time_regressions, 0);
    assert_eq!(summary.time_of_day_regressions, 0);
    // Out-of-range parameters fail before any socket is opened.
    for (duration, step_timeout) in [
        (Duration::ZERO, Duration::from_secs(1)),
        (Duration::from_secs(73 * 3600), Duration::from_secs(1)),
        (Duration::from_secs(3), Duration::ZERO),
    ] {
        assert!(
            run_hold(&HoldConfig {
                server: address,
                network_version: profile.pin.network_checksum,
                engine_net_version: profile.engine_network_version,
                duration,
                step_timeout,
            })
            .is_err()
        );
    }
    // Generous for loaded runners: the world time alone is due every 0.1 s.
    assert!(summary.max_update_gap_ms < 1_000, "{summary:?}");
    assert_eq!(summary.ended_by_error, None);
}

/// A server that disappears mid-hold ends the hold early with the socket
/// error's kind, keeping what arrived before it.
#[test]
fn a_hold_keeps_its_summary_when_the_server_goes_away() {
    use icarus_bevy::experimental::session_hold::{HoldConfig, run_hold};
    let directory = tempfile::tempdir().unwrap();
    let config = pinned_server_config(directory.path(), &release_toml());
    let state = directory.path().join("state");
    let mut server = spawn_run(&config, &state);
    let (address, _) = listening_address(&mut server);
    let profile = ReleaseProfile::load(&release_toml()).unwrap();
    let pid = server.child.id().to_string();
    let killer = thread::spawn(move || {
        thread::sleep(Duration::from_millis(1_500));
        Command::new("kill").args(["-KILL", &pid]).status().unwrap();
    });
    let started = Instant::now();
    let summary = run_hold(&HoldConfig {
        server: address,
        network_version: profile.pin.network_checksum,
        engine_net_version: profile.engine_network_version,
        duration: Duration::from_secs(20),
        step_timeout: Duration::from_secs(3),
    })
    .unwrap();
    killer.join().unwrap();
    assert!(started.elapsed() < Duration::from_secs(15), "{summary:?}");
    assert!(
        summary.joined && summary.game_state_updates > 0,
        "{summary:?}"
    );
    assert_eq!(
        summary.ended_by_error.as_deref(),
        Some("ConnectionRefused"),
        "{summary:?}"
    );
}

/// Without a release profile the server opens no actors after Join: the
/// hold reports no join, no updates and its whole length as the gap.
#[test]
fn a_hold_without_actor_opens_reports_no_join_and_the_whole_gap() {
    use icarus_bevy::experimental::session_hold::{HoldConfig, run_hold};
    let directory = tempfile::tempdir().unwrap();
    let config = server_config(directory.path());
    let state = directory.path().join("state");
    let mut server = spawn_run(&config, &state);
    let (address, _) = listening_address(&mut server);
    let summary = run_hold(&HoldConfig {
        server: address,
        network_version: 0,
        engine_net_version: ReleaseProfile::load(&release_toml())
            .unwrap()
            .engine_network_version,
        duration: Duration::from_secs(2),
        step_timeout: Duration::from_secs(3),
    })
    .unwrap();
    sigterm(&server);
    assert!(server.wait_exit().success());
    assert!(!summary.joined && !summary.refused, "{summary:?}");
    assert_eq!(summary.game_state_updates, 0);
    assert!(summary.max_update_gap_ms >= 1_900, "{summary:?}");
}

/// Four probe sessions held at once each join on their own socket and
/// identity, receive steady GameState updates without regressions and see
/// the `NetworkingStatus` list all four players.
#[test]
fn concurrent_held_sessions_each_receive_updates_and_see_every_player() {
    use icarus_bevy::experimental::session_hold::{HoldConfig, MultiHoldConfig, run_holds};
    let directory = tempfile::tempdir().unwrap();
    let config = pinned_server_config(directory.path(), &release_toml());
    let state = directory.path().join("state");
    let mut server = spawn_run(&config, &state);
    let (address, _) = listening_address(&mut server);
    let profile = ReleaseProfile::load(&release_toml()).unwrap();
    let summary = run_holds(&MultiHoldConfig {
        hold: HoldConfig {
            server: address,
            network_version: profile.pin.network_checksum,
            engine_net_version: profile.engine_network_version,
            duration: Duration::from_secs(3),
            step_timeout: Duration::from_secs(3),
        },
        sessions: 4,
    })
    .unwrap();
    sigterm(&server);
    assert!(server.wait_exit().success());
    assert_eq!(summary.sessions_requested, 4);
    assert_eq!(summary.sessions.len(), 4);
    assert_eq!(
        (summary.joined, summary.refused, summary.failed),
        (4, 0, 0),
        "{summary:?}"
    );
    assert_eq!(summary.decode_errors, 0, "{summary:?}");
    assert_eq!(summary.regressions, 0, "{summary:?}");
    assert!(summary.errors_by_kind.is_empty(), "{summary:?}");
    assert!(summary.min_game_state_updates.unwrap() >= 10, "{summary:?}");
    // Generous for loaded runners: the world time alone is due every 0.1 s.
    assert!(summary.max_update_gap_ms.unwrap() < 1_000, "{summary:?}");
    for session in &summary.sessions {
        assert!(session.hold.world_time_updates >= 10, "{session:?}");
        assert_eq!(session.max_listed_players, Some(4), "{session:?}");
    }
    let total: u64 = summary
        .sessions
        .iter()
        .map(|session| session.hold.game_state_updates)
        .sum();
    assert_eq!(summary.game_state_updates, total);
    // Counts only: no synthetic name reaches the summary.
    let printed = serde_json::to_string(&summary).unwrap();
    assert!(!printed.contains("HoldProbe"), "{printed}");
}

/// Sessions beyond `max_players` are refused at Login and reported as
/// refused; the admitted ones hold and see only themselves listed.
#[test]
fn concurrent_held_sessions_beyond_the_player_limit_are_refused() {
    use icarus_bevy::experimental::session_hold::{HoldConfig, MultiHoldConfig, run_holds};
    let directory = tempfile::tempdir().unwrap();
    let config = directory.path().join("server.toml");
    let profile_path = toml::Value::String(release_toml().display().to_string());
    fs::write(
        &config,
        format!(
            "status_interval_seconds = 1\n[network]\nenabled = true\nport = 0\nmax_players = 2\nrelease_profile = {profile_path}\n"
        ),
    )
    .unwrap();
    let state = directory.path().join("state");
    let mut server = spawn_run(&config, &state);
    let (address, listening) = listening_address(&mut server);
    assert_eq!(listening["max_players"], 2);
    let profile = ReleaseProfile::load(&release_toml()).unwrap();
    let summary = run_holds(&MultiHoldConfig {
        hold: HoldConfig {
            server: address,
            network_version: profile.pin.network_checksum,
            engine_net_version: profile.engine_network_version,
            duration: Duration::from_secs(2),
            step_timeout: Duration::from_secs(3),
        },
        sessions: 4,
    })
    .unwrap();
    sigterm(&server);
    assert!(server.wait_exit().success());
    assert_eq!(
        (summary.joined, summary.refused, summary.failed),
        (2, 2, 0),
        "{summary:?}"
    );
    assert!(summary.errors_by_kind.is_empty(), "{summary:?}");
    assert_eq!(summary.regressions, 0, "{summary:?}");
    for session in &summary.sessions {
        assert_eq!(session.setup_error, None, "{session:?}");
        if session.hold.refused {
            assert!(!session.hold.joined, "{session:?}");
            assert_eq!(session.hold.game_state_updates, 0, "{session:?}");
        } else {
            assert!(session.hold.joined, "{session:?}");
            assert!(session.hold.game_state_updates > 0, "{session:?}");
            assert_eq!(session.max_listed_players, Some(2), "{session:?}");
        }
    }
}

/// A concurrent hold needs 1 to 64 sessions and the single hold's ranges;
/// out-of-range parameters fail before any socket is opened.
#[test]
fn concurrent_hold_session_counts_are_bounded() {
    use icarus_bevy::experimental::session_hold::{
        HoldConfig, MAX_HOLD, MAX_HOLD_SESSIONS, MultiHoldConfig, run_holds,
    };
    let hold = HoldConfig {
        // Nothing listens here; validation must fail first.
        server: "127.0.0.1:9".parse().unwrap(),
        network_version: 0,
        engine_net_version: 0,
        duration: Duration::from_secs(1),
        step_timeout: Duration::from_secs(1),
    };
    for sessions in [0, MAX_HOLD_SESSIONS + 1] {
        let error = run_holds(&MultiHoldConfig {
            hold: hold.clone(),
            sessions,
        })
        .unwrap_err();
        assert!(error.to_string().contains("sessions"), "{error}");
    }
    for duration in [Duration::ZERO, MAX_HOLD + Duration::from_secs(1)] {
        let error = run_holds(&MultiHoldConfig {
            hold: HoldConfig {
                duration,
                ..hold.clone()
            },
            sessions: 4,
        })
        .unwrap_err();
        assert!(error.to_string().contains("hold lasts"), "{error}");
    }
    // 72 hours after full admission fits, with the admission margin.
    assert_eq!(MAX_HOLD, Duration::from_secs(72 * 3600 + 120));
}

#[test]
fn concurrent_hold_progress_arrives_before_completion_and_preserves_counts() {
    use icarus_bevy::experimental::session_hold::{
        HoldConfig, MultiHoldConfig, run_holds_with_progress,
    };
    let directory = tempfile::tempdir().unwrap();
    let config = pinned_server_config(directory.path(), &release_toml());
    let mut server = spawn_run(&config, &directory.path().join("state"));
    let (address, _) = listening_address(&mut server);
    let profile = ReleaseProfile::load(&release_toml()).unwrap();
    let started = Instant::now();
    let mut receipts = Vec::new();
    let summary = run_holds_with_progress(
        &MultiHoldConfig {
            hold: HoldConfig {
                server: address,
                network_version: profile.pin.network_checksum,
                engine_net_version: profile.engine_network_version,
                duration: Duration::from_secs(3),
                step_timeout: Duration::from_secs(3),
            },
            sessions: 4,
        },
        Duration::from_millis(300),
        |receipt| {
            assert!(started.elapsed() < Duration::from_secs(12));
            assert!(receipt.incomplete);
            assert!(receipt.completed < receipt.sessions_requested);
            assert_eq!(receipt.event, "session_hold_progress");
            let printed = serde_json::to_string(receipt).unwrap();
            for private in ["HoldProbe", "127.0.0.1", "sessions\":", "STEAM"] {
                assert!(!printed.contains(private), "{printed}");
            }
            receipts.push(receipt.clone());
            Ok(())
        },
    )
    .unwrap();
    sigterm(&server);
    assert!(server.wait_exit().success());
    assert!(!receipts.is_empty());
    assert!(
        receipts
            .iter()
            .any(|receipt| receipt.joined == 4 && receipt.game_state_updates > 0)
    );
    for pair in receipts.windows(2) {
        assert!(pair[1].wall_ms >= pair[0].wall_ms);
        assert!(pair[1].joined >= pair[0].joined);
        assert!(pair[1].game_state_updates >= pair[0].game_state_updates);
        assert!(pair[1].decode_errors >= pair[0].decode_errors);
        assert!(pair[1].regressions >= pair[0].regressions);
    }
    let last = receipts.last().unwrap();
    assert!(summary.game_state_updates >= last.game_state_updates);
    assert!(summary.decode_errors >= last.decode_errors);
    assert!(summary.regressions >= last.regressions);
    assert_eq!(summary.joined, 4);
}

#[test]
fn hold_progress_includes_the_ongoing_gap_without_updates() {
    use icarus_bevy::experimental::session_hold::{HoldConfig, run_hold_with_progress};
    let directory = tempfile::tempdir().unwrap();
    let config = server_config(directory.path());
    let mut server = spawn_run(&config, &directory.path().join("state"));
    let (address, _) = listening_address(&mut server);
    let mut gaps = Vec::new();
    let summary = run_hold_with_progress(
        &HoldConfig {
            server: address,
            network_version: 0,
            engine_net_version: ReleaseProfile::load(&release_toml())
                .unwrap()
                .engine_network_version,
            duration: Duration::from_secs(3),
            step_timeout: Duration::from_secs(3),
        },
        Duration::from_millis(300),
        |receipt| {
            assert_eq!(receipt.game_state_updates, 0);
            if let Some(gap) = receipt.max_update_gap_ms {
                gaps.push(gap);
            }
            Ok(())
        },
    )
    .unwrap();
    sigterm(&server);
    assert!(server.wait_exit().success());
    assert!(gaps.iter().any(|gap| *gap >= 1_000), "{gaps:?}");
    assert!(gaps.windows(2).all(|pair| pair[1] >= pair[0]));
    assert!(summary.max_update_gap_ms >= *gaps.last().unwrap());
}

#[test]
fn hold_progress_sink_failure_or_panic_cancels_long_holds() {
    use icarus_bevy::experimental::session_hold::{
        HoldConfig, MultiHoldConfig, run_holds_with_progress,
    };
    let directory = tempfile::tempdir().unwrap();
    let config = pinned_server_config(directory.path(), &release_toml());
    let mut server = spawn_run(&config, &directory.path().join("state"));
    let (address, _) = listening_address(&mut server);
    let profile = ReleaseProfile::load(&release_toml()).unwrap();
    let holds = MultiHoldConfig {
        hold: HoldConfig {
            server: address,
            network_version: profile.pin.network_checksum,
            engine_net_version: profile.engine_network_version,
            duration: Duration::from_secs(60),
            step_timeout: Duration::from_secs(2),
        },
        sessions: 2,
    };
    for panic in [false, true] {
        let started = Instant::now();
        let result = std::panic::catch_unwind(|| {
            run_holds_with_progress(&holds, Duration::from_millis(500), |_| {
                if panic {
                    panic!("authored progress sink panic");
                }
                Err(
                    std::io::Error::new(std::io::ErrorKind::BrokenPipe, "authored sink failure")
                        .into(),
                )
            })
        });
        assert!(started.elapsed() < Duration::from_secs(15));
        if panic {
            assert!(result.is_err());
        } else {
            let error = result.unwrap().unwrap_err();
            assert_eq!(
                error.downcast_ref::<std::io::Error>().unwrap().kind(),
                std::io::ErrorKind::BrokenPipe
            );
        }
    }
    sigterm(&server);
    assert!(server.wait_exit().success());
}

#[test]
fn zero_hold_progress_interval_fails_before_opening_a_socket() {
    use icarus_bevy::experimental::session_hold::{HoldConfig, run_hold_with_progress};
    let error = run_hold_with_progress(
        &HoldConfig {
            server: "127.0.0.1:9".parse().unwrap(),
            network_version: 0,
            engine_net_version: 0,
            duration: Duration::from_secs(1),
            step_timeout: Duration::from_secs(1),
        },
        Duration::ZERO,
        |_| panic!("invalid interval must not report progress"),
    )
    .unwrap_err();
    assert!(error.to_string().contains("progress interval"), "{error}");
}

/// Past one bunch of player names the NetworkingStatus update goes out as
/// unreliable partial bunches on consecutive packets, and every one of 48
/// held sessions reassembles it and sees all 48 players listed.
#[test]
fn a_networking_status_over_one_bunch_reaches_every_held_session() {
    use icarus_bevy::experimental::session_hold::{HoldConfig, MultiHoldConfig, run_holds};
    let directory = tempfile::tempdir().unwrap();
    let config = directory.path().join("server.toml");
    let profile_path = toml::Value::String(release_toml().display().to_string());
    fs::write(
        &config,
        format!(
            "status_interval_seconds = 1\n[network]\nenabled = true\nport = 0\nmax_players = 48\nrelease_profile = {profile_path}\n"
        ),
    )
    .unwrap();
    let state = directory.path().join("state");
    let mut server = spawn_run(&config, &state);
    let (address, _) = listening_address(&mut server);
    let profile = ReleaseProfile::load(&release_toml()).unwrap();
    let summary = run_holds(&MultiHoldConfig {
        hold: HoldConfig {
            server: address,
            network_version: profile.pin.network_checksum,
            engine_net_version: profile.engine_network_version,
            // Generous for loaded runners: the joins take well under a second.
            duration: Duration::from_secs(8),
            step_timeout: Duration::from_secs(5),
        },
        sessions: 48,
    })
    .unwrap();
    sigterm(&server);
    assert!(server.wait_exit().success());
    assert_eq!(
        (summary.joined, summary.refused, summary.failed),
        (48, 0, 0),
        "{summary:?}"
    );
    assert_eq!(summary.decode_errors, 0, "{summary:?}");
    assert_eq!(summary.regressions, 0, "{summary:?}");
    for session in &summary.sessions {
        assert_eq!(session.max_listed_players, Some(48), "{session:?}");
    }
    assert!(summary.partial_parts > 0, "no update was split");
    assert_eq!(
        summary.partial_parts,
        summary
            .sessions
            .iter()
            .map(|session| session.hold.partial_parts)
            .sum::<u64>()
    );
}

/// The saved QuestManager (F-861), InventoryContainerManager (F-866) and
/// PlayerHistoryTracker (F-862) join the bound managers on the next
/// channels; a repeated slot fails, and so does a lab without managers.
#[test]
fn the_saved_managers_follow_the_bound_managers() {
    use icarus_bevy::experimental::manager_open::ManagerSlot;
    let network = NetworkConfig {
        release_profile: Some(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("config/release.toml"),
        ),
        ..enabled(0)
    };
    let mut bound = network::bind(&network).unwrap().unwrap();
    let saved = std::mem::take(&mut bound.saved_managers);
    let slots: Vec<ManagerSlot> = saved.iter().map(|(slot, _)| *slot).collect();
    assert_eq!(
        slots,
        [
            ManagerSlot::Quest,
            ManagerSlot::InventoryContainer,
            ManagerSlot::PlayerHistory,
            ManagerSlot::Flod
        ]
    );
    let quest = saved[0].1.clone();
    for ((slot, plan), channel) in saved.into_iter().zip(9..) {
        assert_eq!(bound.lab.add_manager_open(slot, plan).unwrap(), channel);
    }
    assert_eq!(network::MANAGER_FIRST_CHANNEL + 3, 9);
    assert!(
        bound
            .lab
            .add_manager_open(ManagerSlot::Quest, quest.clone())
            .is_err()
    );
    let mut bare = network::bind(&enabled(0)).unwrap().unwrap();
    assert!(bare.saved_managers.is_empty());
    assert!(
        bare.lab
            .add_manager_open(ManagerSlot::Quest, quest)
            .is_err()
    );
}

/// Each run's GameState carries a fresh `RichPresenceGUID` as the original
/// formats it: 32 uppercase hex digits (F-912).
#[test]
fn the_game_state_carries_a_fresh_rich_presence_guid() {
    use icarus_bevy::experimental::game_state_open::handle;
    let guid =
        |values: &icarus_bevy::experimental::game_state_open::GameStateOpenState| match values
            .values
            .get(&handle::RICH_PRESENCE_GUID)
        {
            Some(icarus_bevy::experimental::rep_layout::RepValue::Str(text)) => {
                String::from_utf16(text.units()).unwrap()
            }
            other => panic!("no RichPresenceGUID: {other:?}"),
        };
    let first = guid(&network::game_state_values().unwrap());
    let second = guid(&network::game_state_values().unwrap());
    for value in [&first, &second] {
        assert_eq!(value.len(), 32);
        assert!(
            value
                .chars()
                .all(|c| c.is_ascii_digit() || ('A'..='F').contains(&c))
        );
    }
    assert_ne!(first, second);
}

/// A player joining while another is joined is not host; one joining alone
/// is (F-913): the first PlayerState carries `bIsHost`, the second, joined
/// alongside, does not.
#[test]
fn only_a_player_joining_alone_is_host() {
    use icarus_bevy::experimental::{
        actor_channel::{ActorChannelCodec, ActorChannelLimits, read_actor_bunch_body},
        archive::ByteOrder,
        packet::read_packet,
        rep_layout::{
            MovementLevel, RepLayoutCodec, RepLayoutLimits, RepValue, RotationLevel,
            bp_icarus_player_state_rep_layout, read_rep_properties,
        },
        session_open::PLAYER_STATE_CHANNEL_INDEX,
    };
    let profile = ReleaseProfile::load(&release_toml()).unwrap();
    let pinned = profile.pin.network_checksum;
    let (mut app, address) = server_app(&NetworkConfig {
        release_profile: Some(release_toml()),
        ..enabled(0)
    });
    let (replies, _) = with_server(&mut app, move || {
        let (first, first_replies) = join_pinned_open(address, pinned, 7);
        let (second, second_replies) = join_pinned_open(address, pinned, 7);
        first.close().unwrap();
        second.close().unwrap();
        [first_replies, second_replies]
    });
    let to_client = PacketConfig {
        magic_bits: 0,
        receiver: PacketReceiver::Client,
        limits: PeerLabConfig::default().packet.packet.limits,
    };
    let codec = ActorChannelCodec {
        engine_net_version: profile.engine_network_version,
        exports_enabled: false,
        has_level: true,
        order: ByteOrder::Little,
        limits: ActorChannelLimits::default(),
    };
    let layout = bp_icarus_player_state_rep_layout(
        MovementLevel::Whole,
        MovementLevel::Whole,
        RotationLevel::Bytes,
    );
    let hosts: Vec<bool> = replies
        .iter()
        .map(|datagrams| {
            let body = datagrams
                .iter()
                .flat_map(|datagram| read_packet(datagram, &to_client).unwrap().bunches)
                .find(|bunch| {
                    bunch.header.channel_index == PLAYER_STATE_CHANNEL_INDEX
                        && !bunch.header.has_package_map_exports
                })
                .expect("a PlayerState open");
            let open = read_actor_bunch_body(&mut body.payload.reader(), true, &codec).unwrap();
            let stream = read_rep_properties(
                &mut open.blocks[0].payload.reader(),
                &layout,
                &RepLayoutCodec {
                    actor: codec,
                    checksums: false,
                    limits: RepLayoutLimits::default(),
                },
            )
            .unwrap();
            stream
                .entries
                .iter()
                .any(|entry| entry.handle == 79 && entry.value == RepValue::Bool(true))
        })
        .collect();
    assert_eq!(hosts, [true, false]);
}

/// An authored, stageable prospect: one associated member and its saved
/// player character (`C026`) with an `IcarusActorGUID`. No supplied save.
fn returning_character_payload(user: &str, slot: i32, uid: i32) -> Vec<u8> {
    returning_characters_payload(user, slot, &[uid])
}

/// [`returning_character_payload`] with one saved character per UID, all for
/// the same member and slot.
fn returning_characters_payload(user: &str, slot: i32, uids: &[i32]) -> Vec<u8> {
    returning_characters_payload_with_state(user, slot, uids, &[])
}

fn returning_characters_payload_with_state(
    user: &str,
    slot: i32,
    uids: &[i32],
    state: &[scalar::Tag],
) -> Vec<u8> {
    saved_player_records_payload(user, slot, uids, state, false)
}

fn saved_player_records_payload(
    user: &str,
    slot: i32,
    uids: &[i32],
    state: &[scalar::Tag],
    controller: bool,
) -> Vec<u8> {
    use scalar::{Tag, stream, units};
    let member = stream(&[
        Tag::text("UserID", "StrProperty", &units(user)),
        Tag::integer("ChrSlot", slot),
    ]);
    let members = Tag::array(
        "AssociatedMembers",
        "StructProperty",
        [
            1_i32.to_le_bytes().to_vec(),
            Tag::structure("AssociatedMembers", "AssociatedMemberInfo", member).bytes(),
        ]
        .concat(),
    );
    let info = Tag::structure(
        "ProspectInfo",
        "ProspectInfo",
        stream(&[
            Tag::text("ProspectID", "StrProperty", &units("p-val")),
            members,
        ]),
    );
    let records: Vec<u8> = uids
        .iter()
        .flat_map(|uid| {
            let mut tags = vec![
                Tag::integer(
                    if controller {
                        "PlayerRecorderVersion"
                    } else {
                        "PlayerStateRecorderVersion"
                    },
                    1,
                ),
                Tag::structure(
                    "PlayerCharacterID",
                    "PlayerCharacterID",
                    stream(&[
                        Tag::text("PlayerID", "StrProperty", &units(user)),
                        Tag::integer("ChrSlot", slot),
                    ]),
                ),
                Tag::integer("IcarusActorGUID", *uid),
            ];
            tags.extend_from_slice(state);
            let character = stream(&tags);
            let body = [character, vec![0; 4]].concat();
            stream(&[
                Tag::text(
                    "ComponentClassName",
                    "StrProperty",
                    &units(if controller {
                        "/Script/Icarus.PlayerRecorderComponent"
                    } else {
                        "/Script/Icarus.PlayerStateRecorderComponent"
                    }),
                ),
                Tag::array(
                    "BinaryData",
                    "ByteProperty",
                    [
                        i32::try_from(body.len()).unwrap().to_le_bytes().to_vec(),
                        body,
                    ]
                    .concat(),
                ),
            ])
        })
        .collect();
    let blobs = Tag::array(
        "StateRecorderBlobs",
        "StructProperty",
        [
            i32::try_from(uids.len()).unwrap().to_le_bytes().to_vec(),
            Tag::structure("StateRecorderBlobs", "StateRecorderBlob", records).bytes(),
        ]
        .concat(),
    );
    stream(&[Tag::integer("Version", 2), info, blobs])
}

/// Send `Server_BeginPlayerInitialisation(slot)` and the empty friends list
/// the authority's finalisation waits for (F-973) on the controller channel
/// and return every datagram collected afterwards.
fn send_character_slot(
    client: &mut icarus_bevy::experimental::probe_client::ProbeClient,
    profile_net_version: u32,
    slot: i32,
) -> Vec<Vec<u8>> {
    send_character_slot_with(client, profile_net_version, slot, true)
}

/// `ServerFriendsListUpdated` with no friends on the controller channel.
fn send_friends_list_updated(
    client: &mut icarus_bevy::experimental::probe_client::ProbeClient,
    profile_net_version: u32,
) {
    use icarus_bevy::experimental::{
        probe_initialise::{friends_list_updated_field, initialise_codec},
        rep_rpc::BP_ICARUS_PLAYER_CONTROLLER_SURVIVAL_FIELD_INDEX_BOUND,
        session_open::PLAYER_CONTROLLER_CHANNEL_INDEX,
    };
    let codec = initialise_codec(profile_net_version);
    client
        .send_actor_rpc(
            PLAYER_CONTROLLER_CHANNEL_INDEX,
            &friends_list_updated_field(&codec).unwrap(),
            BP_ICARUS_PLAYER_CONTROLLER_SURVIVAL_FIELD_INDEX_BOUND,
            &codec.actor,
        )
        .unwrap();
}

fn send_character_slot_with(
    client: &mut icarus_bevy::experimental::probe_client::ProbeClient,
    profile_net_version: u32,
    slot: i32,
    send_friends_list: bool,
) -> Vec<Vec<u8>> {
    use icarus_bevy::experimental::{
        actor_channel::{ActorChannelCodec, ActorChannelLimits},
        archive::ByteOrder,
        packet::{BitWriter, Payload},
        rep_rpc::{
            BP_ICARUS_PLAYER_CONTROLLER_SURVIVAL_FIELD_INDEX_BOUND, FieldHeader,
            SERVER_BEGIN_PLAYER_INITIALISATION_FIELD_INDEX,
        },
        session_open::PLAYER_CONTROLLER_CHANNEL_INDEX,
    };
    let mut parameter = BitWriter::new();
    parameter.write_bit(true);
    parameter.write_bytes(&slot.to_le_bytes());
    let (_, packet_id) = client
        .send_actor_rpc(
            PLAYER_CONTROLLER_CHANNEL_INDEX,
            &FieldHeader {
                index: SERVER_BEGIN_PLAYER_INITIALISATION_FIELD_INDEX,
                payload: Payload::from_writer(parameter),
            },
            BP_ICARUS_PLAYER_CONTROLLER_SURVIVAL_FIELD_INDEX_BOUND,
            &ActorChannelCodec {
                engine_net_version: profile_net_version,
                exports_enabled: false,
                has_level: true,
                order: ByteOrder::Little,
                limits: ActorChannelLimits::default(),
            },
        )
        .unwrap();
    if send_friends_list {
        send_friends_list_updated(client, profile_net_version);
    }
    let replies = client.collect(Duration::from_secs(2), 32).unwrap();
    assert!(replies.acked.contains(&packet_id));
    replies
        .replies
        .into_iter()
        .map(|reply| reply.bytes)
        .collect()
}

/// Finish the native exchange using the component GUIDs in its requests.
/// Saved UID restore waits for both loadout and bestiary answers.
fn exchange_profiles(
    client: &mut ProbeClient,
    engine: u32,
    requested_slot: i32,
    profile_slot: i32,
) -> Vec<Vec<u8>> {
    use icarus_bevy::experimental::{
        actor_channel::{ActorChannelCodec, BlockTarget, read_actor_bunch_body},
        archive::ByteOrder,
        packet::{BitWriter, Payload, read_packet},
        probe_client::ProbeProfileResponse,
        rep_layout::RepLayoutCodec,
        rep_rpc::{
            BACKEND_PROXY_FIELD_INDEX_BOUND, BESTIARY_MANAGER_FIELD_INDEX_BOUND,
            CLIENT_REQUEST_GET_CHARACTER_PROFILE, CLIENT_REQUEST_GET_USER_PROFILE,
            PLAYER_DATA_FIELD_INDEX_BOUND, read_field_header,
        },
        session_open::PLAYER_CONTROLLER_CHANNEL_INDEX,
    };
    let codec = RepLayoutCodec {
        actor: ActorChannelCodec {
            engine_net_version: engine,
            exports_enabled: false,
            has_level: true,
            order: ByteOrder::Little,
            limits: Default::default(),
        },
        checksums: false,
        limits: Default::default(),
    };
    let replies = send_character_slot(client, engine, requested_slot);
    let request = |replies: &[Vec<u8>], expected| {
        for bytes in replies {
            for bunch in read_packet(
                bytes,
                &PacketConfig {
                    receiver: PacketReceiver::Client,
                    ..probe_config().packet
                },
            )
            .unwrap()
            .bunches
            {
                if bunch.header.channel_index != PLAYER_CONTROLLER_CHANNEL_INDEX
                    || bunch.header.open
                {
                    continue;
                }
                assert!(bunch.header.reliable);
                let body = read_actor_bunch_body(&mut bunch.payload.reader(), false, &codec.actor)
                    .unwrap();
                for block in body.blocks {
                    if let BlockTarget::SubObject { object, .. } = block.target {
                        let mut reader = block.payload.reader();
                        let field =
                            read_field_header(&mut reader, BACKEND_PROXY_FIELD_INDEX_BOUND, 65_536)
                                .unwrap();
                        assert!(reader.at_end());
                        assert_eq!(field.index, expected);
                        return object.guid;
                    }
                }
            }
        }
        panic!("no profile request")
    };
    // No pawn update may precede both initialisation answers.
    let has_pawn = |replies: &[Vec<u8>]| {
        replies.iter().any(|bytes| {
            read_packet(
                bytes,
                &PacketConfig {
                    receiver: PacketReceiver::Client,
                    ..probe_config().packet
                },
            )
            .unwrap()
            .bunches
            .iter()
            .any(|bunch| {
                bunch.header.channel_index
                    == icarus_bevy::experimental::pawn_open::PAWN_CHANNEL_INDEX
                    && !bunch.header.open
            })
        })
    };
    assert!(!has_pawn(&replies));
    let proxy = request(&replies, CLIENT_REQUEST_GET_USER_PROFILE);
    client
        .send_profile_response(
            PLAYER_CONTROLLER_CHANNEL_INDEX,
            proxy,
            ProbeProfileResponse {
                character: false,
                success: true,
                profile: profiles::user().rpc,
            },
            &codec,
        )
        .unwrap();
    let replies: Vec<_> = client
        .collect(Duration::from_millis(250), 32)
        .unwrap()
        .replies
        .into_iter()
        .map(|reply| reply.bytes)
        .collect();
    assert!(!has_pawn(&replies));
    assert_eq!(
        request(&replies, CLIENT_REQUEST_GET_CHARACTER_PROFILE),
        proxy
    );
    client
        .send_profile_response(
            PLAYER_CONTROLLER_CHANNEL_INDEX,
            proxy,
            ProbeProfileResponse {
                character: true,
                success: true,
                profile: profiles::character(profile_slot).rpc,
            },
            &codec,
        )
        .unwrap();
    let replies: Vec<_> = client
        .collect(Duration::from_millis(250), 32)
        .unwrap()
        .replies
        .into_iter()
        .map(|reply| reply.bytes)
        .collect();
    assert!(!has_pawn(&replies));
    let mut components = None;
    for bytes in &replies {
        for bunch in read_packet(
            bytes,
            &PacketConfig {
                receiver: PacketReceiver::Client,
                ..probe_config().packet
            },
        )
        .unwrap()
        .bunches
        {
            if bunch.header.channel_index != PLAYER_CONTROLLER_CHANNEL_INDEX || bunch.header.open {
                continue;
            }
            let body =
                read_actor_bunch_body(&mut bunch.payload.reader(), false, &codec.actor).unwrap();
            assert_eq!(body.blocks.len(), 3);
            let mut guids = Vec::new();
            for ((block, bound), index) in body
                .blocks
                .iter()
                .zip([
                    PLAYER_DATA_FIELD_INDEX_BOUND,
                    BESTIARY_MANAGER_FIELD_INDEX_BOUND,
                    PLAYER_DATA_FIELD_INDEX_BOUND,
                ])
                .zip([16, 4, 2])
            {
                let BlockTarget::SubObject { object, .. } = &block.target else {
                    panic!("component");
                };
                let mut reader = block.payload.reader();
                assert_eq!(
                    read_field_header(&mut reader, bound, 65_536).unwrap().index,
                    index
                );
                assert!(reader.at_end());
                guids.push(object.guid);
            }
            assert_eq!(guids[0], guids[2]);
            components = Some((guids[0], guids[1]));
        }
    }
    let (player_data, bestiary) = components.unwrap();
    // Exercise both completion orders across the saved-slot matching cases.
    let bestiary_first = requested_slot != 3;
    for bestiary_answer in [bestiary_first, !bestiary_first] {
        let mut writer = BitWriter::new();
        for bit in [true, false, true] {
            writer.write_bit(bit);
        }
        let payload = Payload::from_writer(writer);
        if bestiary_answer {
            client
                .send_bestiary_response(
                    PLAYER_CONTROLLER_CHANNEL_INDEX,
                    bestiary,
                    payload,
                    &codec.actor,
                )
                .unwrap();
        } else {
            client
                .send_loadout_response(
                    PLAYER_CONTROLLER_CHANNEL_INDEX,
                    player_data,
                    payload,
                    &codec.actor,
                )
                .unwrap();
        }
        let replies: Vec<_> = client
            .collect(Duration::from_millis(250), 32)
            .unwrap()
            .replies
            .into_iter()
            .map(|reply| reply.bytes)
            .collect();
        if bestiary_answer == bestiary_first {
            assert!(!has_pawn(&replies));
        } else {
            return replies;
        }
    }
    unreachable!()
}

/// The `IcarusUID` of a reliable, non-open pawn property bunch, if any.
fn pawn_uid_update(profile: &ReleaseProfile, datagrams: &[Vec<u8>]) -> Option<i32> {
    use icarus_bevy::experimental::{
        actor_channel::{ActorChannelCodec, ActorChannelLimits, read_actor_bunch_body},
        archive::ByteOrder,
        packet::read_packet,
        pawn_open::PAWN_CHANNEL_INDEX,
        peer_lab::PeerLabAdmission,
        rep_layout::{RepLayoutCodec, RepLayoutLimits, RepValue, read_rep_properties},
    };
    let packet_config = PacketConfig {
        receiver: PacketReceiver::Client,
        ..probe_config().packet
    };
    let plan = profile
        .pawn_open_plan(PeerLabAdmission::default().map_url)
        .unwrap()
        .unwrap();
    let actor = ActorChannelCodec {
        engine_net_version: profile.engine_network_version,
        exports_enabled: false,
        has_level: true,
        order: ByteOrder::Little,
        limits: ActorChannelLimits::default(),
    };
    let codec = RepLayoutCodec {
        actor,
        checksums: false,
        limits: RepLayoutLimits::default(),
    };
    for bytes in datagrams {
        let Ok(packet) = read_packet(bytes, &packet_config) else {
            continue;
        };
        for bunch in packet.bunches {
            let header = &bunch.header;
            if header.channel_index != PAWN_CHANNEL_INDEX || header.open || !header.reliable {
                continue;
            }
            let body = read_actor_bunch_body(&mut bunch.payload.reader(), false, &actor).unwrap();
            let properties =
                read_rep_properties(&mut body.blocks[0].payload.reader(), &plan.layout, &codec)
                    .unwrap();
            if let Some(property) = properties
                .entries
                .iter()
                .find(|property| property.handle == 46)
            {
                assert_eq!(properties.entries.len(), 1, "only IcarusUID changes");
                let RepValue::Int(uid) = property.value else {
                    panic!("IcarusUID is an i32")
                };
                return Some(uid);
            }
        }
    }
    None
}

type DecodedPawnBlock = (
    icarus_bevy::experimental::actor_channel::BlockTarget,
    std::collections::BTreeMap<u32, icarus_bevy::experimental::rep_layout::RepValue>,
);

/// Decode History from tracker opens or reliable updates. These small opens
/// split only their exports from the complete body; Open is on that export
/// part, and the following body is the final partial (as in sent_pawn_uid).
fn tracker_histories(
    profile: &ReleaseProfile,
    channel: u32,
    datagrams: &[Vec<u8>],
    open: bool,
) -> Vec<icarus_bevy::experimental::rep_layout::RepValue> {
    use icarus_bevy::experimental::{
        actor_channel::{ActorChannelCodec, BlockTarget, read_actor_bunch_body},
        archive::ByteOrder,
        manager_open::{ManagerSlot, PLAYER_HISTORY_HANDLE},
        packet::read_packet,
        peer_lab::PeerLabAdmission,
        rep_layout::{RepLayoutCodec, read_rep_properties},
    };
    let plan = profile
        .manager_open_plan(
            ManagerSlot::PlayerHistory,
            PeerLabAdmission::default().map_url,
        )
        .unwrap()
        .unwrap();
    let codec = RepLayoutCodec {
        actor: ActorChannelCodec {
            engine_net_version: profile.engine_network_version,
            exports_enabled: false,
            has_level: true,
            order: ByteOrder::Little,
            limits: Default::default(),
        },
        checksums: false,
        limits: Default::default(),
    };
    let mut histories = Vec::new();
    let mut opening = false;
    for bytes in datagrams {
        let packet = read_packet(
            bytes,
            &PacketConfig {
                receiver: PacketReceiver::Client,
                ..probe_config().packet
            },
        )
        .unwrap();
        for bunch in packet.bunches {
            if bunch.header.channel_index != channel {
                continue;
            }
            if bunch.header.has_package_map_exports {
                opening |= bunch.header.open;
                continue;
            }
            let is_open = opening || bunch.header.open;
            opening = false;
            if is_open != open {
                continue;
            }
            assert!(bunch.header.reliable);
            if bunch.header.partial {
                assert!(is_open && bunch.header.partial_final && !bunch.header.partial_initial);
            }
            let mut reader = bunch.payload.reader();
            let body = read_actor_bunch_body(&mut reader, open, &codec.actor).unwrap();
            assert!(reader.at_end());
            assert_eq!(body.spawn.is_some(), open);
            assert_eq!(body.blocks[0].target, BlockTarget::Actor);
            let mut reader = body.blocks[0].payload.reader();
            let stream = read_rep_properties(&mut reader, &plan.layout, &codec).unwrap();
            assert!(reader.at_end());
            if !open {
                assert_eq!(body.blocks.len(), 1);
                assert_eq!(stream.entries.len(), 1, "only History changes");
            }
            histories.extend(
                stream
                    .entries
                    .into_iter()
                    .filter(|entry| entry.handle == PLAYER_HISTORY_HANDLE)
                    .map(|entry| entry.value),
            );
        }
    }
    histories
}

#[test]
fn new_players_append_history_for_every_open_tracker_and_later_opens() {
    use icarus_bevy::experimental::{
        manager_open::{ManagerSlot, PLAYER_HISTORY_HANDLE},
        prospect_info::PlayerHistoryEntry,
    };
    let profile = ReleaseProfile::load(&release_toml()).unwrap();
    let pinned = profile.pin.network_checksum;
    let engine = profile.engine_network_version;
    let config = NetworkConfig {
        release_profile: Some(release_toml()),
        ..enabled(0)
    };
    let mut bound = network::bind(&config).unwrap().unwrap();
    let saved_entry = PlayerHistoryEntry {
        user_id: "previous-player".encode_utf16().collect(),
        chr_slot: 2,
        cached_character_name: "previous-name".encode_utf16().collect(),
    };
    let plan = bound
        .saved_managers
        .iter()
        .find(|(slot, _)| *slot == ManagerSlot::PlayerHistory)
        .unwrap()
        .1
        .clone()
        .with_player_history(std::slice::from_ref(&saved_entry))
        .unwrap();
    let channel = bound
        .lab
        .add_manager_open(ManagerSlot::PlayerHistory, plan.clone())
        .unwrap();
    let mut app = build_app(&ServerConfig::default(), &WorldSnapshot::default()).unwrap();
    let address = network::install(&mut app, bound.lab).unwrap();
    let profile_for_gate = profile.clone();
    let ((mut first, mut second), _) = with_server(&mut app, move || {
        let (first, _) = join_pinned_open(address, pinned, 32);
        let (second, _) = join_pinned_open(address, pinned, 32);
        (first, second)
    });
    assert_eq!(
        counts(&app).player_history_appends,
        0,
        "Join alone does not add History"
    );
    let ((first_client, second_client, first_updates, second_updates), _) =
        with_server(&mut app, move || {
            // The first controller has not sent its friends list, so the
            // completed second player does not finalise (F-973).
            let blocked = exchange_profiles(&mut second, engine, 9, 3);
            assert!(tracker_histories(&profile_for_gate, channel, &blocked, false).is_empty());
            send_friends_list_updated(&mut first, engine);
            let second_updates = second
                .collect(Duration::from_millis(250), 32)
                .unwrap()
                .replies
                .into_iter()
                .map(|reply| reply.bytes)
                .collect::<Vec<_>>();
            let first_updates = first
                .collect(Duration::from_millis(250), 32)
                .unwrap()
                .replies
                .into_iter()
                .map(|reply| reply.bytes)
                .collect::<Vec<_>>();
            (first, second, first_updates, second_updates)
        });
    let expected = plan
        .clone()
        .with_player_history(&[
            saved_entry,
            PlayerHistoryEntry {
                user_id: NET_ID_HEX.encode_utf16().collect(),
                chr_slot: 3,
                cached_character_name: "authored-character".encode_utf16().collect(),
            },
        ])
        .unwrap()
        .values[&PLAYER_HISTORY_HANDLE]
        .clone();
    for updates in [&first_updates, &second_updates] {
        assert_eq!(
            tracker_histories(&profile, channel, updates, false),
            std::slice::from_ref(&expected)
        );
    }
    let ((third, opens), _) = with_server(&mut app, move || join_pinned_open(address, pinned, 32));
    assert_eq!(
        tracker_histories(&profile, channel, &opens, true),
        [expected]
    );
    let status = counts(&app);
    assert_eq!(
        (
            status.player_history_appends,
            status.player_history_duplicates,
            status.player_history_errors
        ),
        (1, 0, 0)
    );
    let stats = app.world().resource::<PeerLab>().stats();
    assert_eq!(status.player_history_appends, stats.player_history_appends);
    for text in [
        serde_json::to_string(&status).unwrap(),
        format!("{:?}", app.world().resource::<PeerLab>()),
    ] {
        for private in [
            NET_ID_HEX,
            NAME_SENTINEL,
            "authored-character",
            "authored-profile-user",
        ] {
            assert!(!text.contains(private));
        }
    }
    with_server(&mut app, move || {
        first_client.close().unwrap();
        second_client.close().unwrap();
        third.close().unwrap();
    });
}

#[test]
fn only_saved_controllers_choose_returning_history_and_duplicates_keep_their_name() {
    use icarus_bevy::{
        experimental::{
            manager_open::{ManagerSlot, PLAYER_HISTORY_HANDLE},
            prospect_info::PlayerHistoryEntry,
        },
        prospect,
    };
    #[derive(Clone, Copy, Debug)]
    enum Case {
        Controller,
        OtherSlot,
        CharacterOnly,
        Duplicate,
    }
    for case in [
        Case::Controller,
        Case::OtherSlot,
        Case::CharacterOnly,
        Case::Duplicate,
    ] {
        let profile = ReleaseProfile::load(&release_toml()).unwrap();
        let pinned = profile.pin.network_checksum;
        let engine = profile.engine_network_version;
        let payload = saved_player_records_payload(
            &NET_ID_HEX.to_ascii_lowercase(),
            3,
            &[70_001],
            &[],
            !matches!(case, Case::CharacterOnly),
        );
        let mut bound = network::bind(&NetworkConfig {
            release_profile: Some(release_toml()),
            ..enabled(0)
        })
        .unwrap()
        .unwrap();
        bound
            .lab
            .enable_saved_player_controllers(
                prospect::saved_player_controllers_from_payload(&payload).unwrap(),
            )
            .unwrap();
        bound
            .lab
            .seed_actor_uids(prospect::saved_actor_uids_from_payload(&payload).unwrap())
            .unwrap();
        bound
            .lab
            .enable_returning_characters(
                prospect::saved_player_characters_from_payload(&payload).unwrap(),
            )
            .unwrap();
        if matches!(case, Case::Duplicate) {
            bound
                .lab
                .enable_saved_player_controllers(Vec::new())
                .unwrap();
        }
        let plan = bound
            .saved_managers
            .iter()
            .find(|(slot, _)| *slot == ManagerSlot::PlayerHistory)
            .unwrap()
            .1
            .clone();
        let plan = if matches!(case, Case::Duplicate) {
            plan.with_player_history(&[PlayerHistoryEntry {
                user_id: NET_ID_HEX.to_ascii_lowercase().encode_utf16().collect(),
                chr_slot: 3,
                cached_character_name: "old-cached-name".encode_utf16().collect(),
            }])
            .unwrap()
        } else {
            plan
        };
        let original = plan.values.get(&PLAYER_HISTORY_HANDLE).cloned();
        let channel = bound
            .lab
            .add_manager_open(ManagerSlot::PlayerHistory, plan)
            .unwrap();
        let mut app = build_app(&ServerConfig::default(), &WorldSnapshot::default()).unwrap();
        let address = network::install(&mut app, bound.lab).unwrap();
        let ((client, replies), _) = with_server(&mut app, move || {
            let (mut client, _) = join_pinned_open(address, pinned, 32);
            let replies = exchange_profiles(
                &mut client,
                engine,
                1,
                if matches!(case, Case::OtherSlot) {
                    4
                } else {
                    3
                },
            );
            (client, replies)
        });
        let append = matches!(case, Case::OtherSlot | Case::CharacterOnly);
        assert_eq!(
            tracker_histories(&profile, channel, &replies, false).len(),
            usize::from(append),
            "{case:?}"
        );
        let stats = app.world().resource::<PeerLab>().stats();
        assert_eq!(
            (
                stats.player_history_appends,
                stats.player_history_duplicates,
                stats.player_history_errors
            ),
            (
                u64::from(append),
                u64::from(matches!(case, Case::Duplicate)),
                0
            ),
            "{case:?}"
        );
        let status = counts(&app);
        assert_eq!(
            (
                status.player_history_appends,
                status.player_history_duplicates,
                status.player_history_errors
            ),
            (
                stats.player_history_appends,
                stats.player_history_duplicates,
                stats.player_history_errors
            )
        );
        assert_eq!(status.joined, 1);
        if matches!(case, Case::CharacterOnly) {
            assert_eq!(
                stats.returning_pawns_restored, 1,
                "C026 does not choose the C025 path"
            );
        }
        let ((later, opens), _) =
            with_server(&mut app, move || join_pinned_open(address, pinned, 32));
        if matches!(case, Case::Duplicate) {
            assert_eq!(
                tracker_histories(&profile, channel, &opens, true),
                [original.unwrap()]
            );
        }
        if matches!(case, Case::Controller) {
            assert!(tracker_histories(&profile, channel, &opens, true).is_empty());
        }
        with_server(&mut app, move || {
            client.close().unwrap();
            later.close().unwrap();
        });
    }
}

#[test]
fn history_bounds_count_errors_and_keep_the_completed_session() {
    use icarus_bevy::experimental::{
        manager_open::ManagerSlot, prospect_info::PlayerHistoryEntry, rep_layout::RepLayoutLimits,
    };
    let profile = ReleaseProfile::load(&release_toml()).unwrap();
    let pinned = profile.pin.network_checksum;
    let engine = profile.engine_network_version;
    let mut bound = network::bind(&NetworkConfig {
        release_profile: Some(release_toml()),
        ..enabled(0)
    })
    .unwrap()
    .unwrap();
    let entries: Vec<_> = (0..RepLayoutLimits::default().max_array_elements)
        .map(|index| PlayerHistoryEntry {
            user_id: format!("saved-{index}").encode_utf16().collect(),
            chr_slot: 0,
            cached_character_name: Vec::new(),
        })
        .collect();
    let plan = bound
        .saved_managers
        .iter()
        .find(|(slot, _)| *slot == ManagerSlot::PlayerHistory)
        .unwrap()
        .1
        .clone()
        .with_player_history(&entries)
        .unwrap();
    let channel = bound
        .lab
        .add_manager_open(ManagerSlot::PlayerHistory, plan)
        .unwrap();
    let mut app = build_app(&ServerConfig::default(), &WorldSnapshot::default()).unwrap();
    let address = network::install(&mut app, bound.lab).unwrap();
    let ((client, replies), _) = with_server(&mut app, move || {
        let (mut client, _) = join_pinned_open(address, pinned, 32);
        let replies = exchange_profiles(&mut client, engine, 3, 3);
        (client, replies)
    });
    assert!(tracker_histories(&profile, channel, &replies, false).is_empty());
    let stats = app.world().resource::<PeerLab>().stats();
    assert_eq!(
        (
            stats.player_history_appends,
            stats.player_history_duplicates,
            stats.player_history_errors
        ),
        (0, 0, 1)
    );
    assert_eq!(stats.players_initialised, 1);
    let status = counts(&app);
    assert_eq!(status.player_history_errors, stats.player_history_errors);
    assert_eq!(status.joined, 1);
    with_server(&mut app, move || client.close().unwrap());
}

#[test]
fn production_run_installs_history_and_loads_c025_or_uses_the_new_player_path() {
    use icarus_bevy::{
        persistence::SnapshotStore,
        preservation::{PreserveLimits, PreservedSection},
    };
    for (saved, returning) in [(false, false), (true, true), (true, false)] {
        let profile = ReleaseProfile::load(&release_toml()).unwrap();
        let directory = tempfile::tempdir().unwrap();
        let config = pinned_server_config(directory.path(), &release_toml());
        let state = directory.path().join("state");
        if saved {
            let payload = if returning {
                saved_player_records_payload(
                    &NET_ID_HEX.to_ascii_lowercase(),
                    3,
                    &[70_001],
                    &[],
                    true,
                )
            } else {
                // UID scanning can load this legacy input, but strict staging
                // refuses it. C025 absence must still permit a new player.
                uid_save_payload(&[])
            };
            let preserved =
                PreservedSection::from_inner_payload(&payload, PreserveLimits::default()).unwrap();
            let mut store = SnapshotStore::open(&state).unwrap();
            store
                .save(&WorldSnapshot {
                    schema_version: icarus_bevy::simulation::SNAPSHOT_SCHEMA_REFERENCED,
                    preserved_prospect: Some(preserved.into()),
                    ..WorldSnapshot::default()
                })
                .unwrap();
        }
        let mut server = spawn_run(&config, &state);
        let (address, _) = listening_address(&mut server);
        let (mut client, _) = join_pinned_open(address, profile.pin.network_checksum, 32);
        let replies = exchange_profiles(&mut client, profile.engine_network_version, 9, 3);
        // Three bound managers occupy 6..8; the empty tracker follows at 9.
        assert_eq!(
            tracker_histories(&profile, 9, &replies, false).len(),
            usize::from(!returning)
        );
        client.close().unwrap();
        sigterm(&server);
        assert!(server.wait_exit().success());
        let stopped = server.event("stopped");
        assert_eq!(
            stopped["sessions"]["player_history_appends"],
            u64::from(!returning)
        );
        assert_eq!(stopped["sessions"]["player_history_errors"], 0);
        assert_eq!(stopped["sessions"]["player_history_duplicates"], 0);
    }
}

/// Decode all non-open pawn bunches, including the UID update, in wire order.
fn pawn_property_updates(
    profile: &ReleaseProfile,
    datagrams: &[Vec<u8>],
) -> Vec<Vec<DecodedPawnBlock>> {
    use icarus_bevy::experimental::{
        actor_channel::{ActorChannelCodec, BlockTarget, read_actor_bunch_body},
        archive::ByteOrder,
        packet::read_packet,
        pawn_open::PAWN_CHANNEL_INDEX,
        peer_lab::PeerLabAdmission,
        rep_layout::{RepLayoutCodec, read_rep_properties},
    };
    let plan = profile
        .pawn_open_plan(PeerLabAdmission::default().map_url)
        .unwrap()
        .unwrap();
    let codec = RepLayoutCodec {
        actor: ActorChannelCodec {
            engine_net_version: profile.engine_network_version,
            exports_enabled: false,
            has_level: true,
            order: ByteOrder::Little,
            limits: Default::default(),
        },
        checksums: false,
        limits: Default::default(),
    };
    let mut updates = Vec::new();
    for bytes in datagrams {
        let packet = read_packet(
            bytes,
            &PacketConfig {
                receiver: PacketReceiver::Client,
                ..probe_config().packet
            },
        )
        .unwrap();
        for bunch in packet.bunches {
            if bunch.header.channel_index != PAWN_CHANNEL_INDEX || bunch.header.open {
                continue;
            }
            assert!(bunch.header.reliable);
            assert!(!bunch.header.partial);
            assert!(!bunch.header.has_package_map_exports);
            let mut reader = bunch.payload.reader();
            let body = read_actor_bunch_body(&mut reader, false, &codec.actor).unwrap();
            assert!(reader.at_end());
            assert!(body.spawn.is_none());
            updates.push(
                body.blocks
                    .into_iter()
                    .map(|block| {
                        assert!(block.has_rep_layout);
                        let layout = if block.target == BlockTarget::Actor {
                            &plan.layout
                        } else {
                            &plan.content.character_state_layout
                        };
                        let mut reader = block.payload.reader();
                        let properties = read_rep_properties(&mut reader, layout, &codec).unwrap();
                        assert!(reader.at_end());
                        (
                            block.target,
                            properties
                                .entries
                                .into_iter()
                                .map(|entry| (entry.handle, entry.value))
                                .collect(),
                        )
                    })
                    .collect(),
            );
        }
    }
    updates
}

#[test]
fn returning_pawns_send_supported_saved_content_after_the_uid() {
    use icarus_bevy::{
        experimental::{
            actor_channel::{BlockTarget, CompressedRotator, SubObjectClass},
            pawn_open::{character_state_handle as h, handle},
            recorders::SavedStomachContent,
            rep_layout::{PropertyStream, RepArray, RepEntry, RepValue},
        },
        prospect,
    };
    use scalar::{Tag, units};
    use std::collections::BTreeMap;
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Case {
        Full,
        Dead,
        Unrecorded,
        NoPak,
        Stomach,
        Omitted,
        RestoreError,
        MissingStomach,
        FramingError,
    }
    for case in [
        Case::Full,
        Case::Dead,
        Case::Unrecorded,
        Case::NoPak,
        Case::Stomach,
        Case::Omitted,
        Case::RestoreError,
        Case::MissingStomach,
        Case::FramingError,
    ] {
        let profile = ReleaseProfile::load(&release_toml()).unwrap();
        let pinned = profile.pin.network_checksum;
        let engine = profile.engine_network_version;
        let float3 = |name, kind, values: [f32; 3]| {
            Tag::structure(
                name,
                kind,
                values.into_iter().flat_map(f32::to_le_bytes).collect(),
            )
        };
        let mut state = vec![
            float3("Location", "Vector", [123.0, -456.0, 789.0]),
            float3("Rotation", "Rotator", [0.0, 90.0, 0.0]),
            Tag::integer("RespawnCount", 2),
            Tag::boolean("HasGrantedLoadout", true),
        ];
        if case != Case::Unrecorded {
            state.extend([
                Tag::integer("Health", 125),
                Tag::integer("OxygenLevel", 900),
                Tag::integer("WaterLevel", -4),
                Tag::integer("FoodLevel", 50),
                Tag::integer("RadiationLevel", 900),
                Tag::array(
                    "CosmeticArmourOverrides",
                    "NameProperty",
                    [
                        4_i32.to_le_bytes().to_vec(),
                        scalar::string("Armour_Head_A"),
                        scalar::string("Missing"),
                        scalar::string("Armour_Head_A"),
                        scalar::string("Armour_Chest_B"),
                    ]
                    .concat(),
                ),
            ]);
        }
        if case == Case::Dead {
            state.push(Tag::boolean("bIsAlive", false));
        }
        if case == Case::Omitted {
            state.remove(0);
        }
        let payload = returning_characters_payload_with_state(NET_ID_HEX, 3, &[70_001], &state);
        let directory = tempfile::tempdir().unwrap();
        let pak = directory.path().join("data.pak");
        let rows: Vec<_> = if case == Case::FramingError {
            (0..1_024)
                .map(|row| format!("AuthoredArmour_{row}"))
                .collect()
        } else {
            ["Armour_Chest_B", "Meat_Cooked", "Armour_Head_A"]
                .map(str::to_owned)
                .to_vec()
        };
        fs::write(
            &pak,
            pak_writer::official_pak(&[
                (
                    "Flags/D_SessionFlags.json",
                    pak_writer::table_json(&["Flag_A"]),
                ),
                (
                    "Items/D_ItemsStatic.json",
                    pak_writer::table_json(&rows.iter().map(String::as_str).collect::<Vec<_>>()),
                ),
            ]),
        )
        .unwrap();
        let profile_path = if case == Case::MissingStomach {
            let path = directory.path().join("release.toml");
            let mut text = fs::read_to_string(release_toml()).unwrap();
            text.push_str(r#"
[pawn]
archetype = [{ path = "Default__P_C", checksum = 1 }, { path = "/Game/P", checksum = 0 }]
components = [{ name = "S", checksum = 2 }, { name = "I", checksum = 3 }, { name = "A", checksum = 4 }]
stat_container = "S"
inventory_component = "I"
actor_state = "A"
inventory_class = [{ path = "Bag", checksum = 9 }, { path = "/Script/P", checksum = 0 }]
"#);
            fs::write(&path, text).unwrap();
            path
        } else {
            release_toml()
        };
        let config = NetworkConfig {
            release_profile: Some(profile_path),
            data_pak: (case != Case::NoPak).then_some(pak),
            ..enabled(0)
        };
        let mut lab = network::bind(&config).unwrap().unwrap().lab;
        lab.seed_actor_uids(prospect::saved_actor_uids_from_payload(&payload).unwrap())
            .unwrap();
        let mut characters = prospect::saved_player_characters_from_payload(&payload).unwrap();
        if case == Case::Stomach || case == Case::NoPak {
            characters[0]
                .state
                .stomach_contents
                .push(SavedStomachContent {
                    food_row_name: units("Meat_Cooked"),
                    modifier_name: units("AuthoredModifier"),
                });
        }
        if case == Case::RestoreError {
            characters[0].state.rotation = Some([f32::NAN, 0.0, 0.0]);
        }
        if case == Case::FramingError {
            characters[0].state.cosmetic_armour_overrides =
                rows.iter().map(|row| units(row)).collect();
        }
        lab.enable_returning_characters(characters).unwrap();
        let mut app = build_app(&ServerConfig::default(), &WorldSnapshot::default()).unwrap();
        let address = network::install(&mut app, lab).unwrap();
        let ((client, reply), _) = with_server(&mut app, move || {
            let (mut client, _) = join_pinned_open(address, pinned, 7);
            let reply = exchange_profiles(&mut client, engine, 3, 3);
            (client, reply)
        });
        assert_eq!(pawn_uid_update(&profile, &reply), Some(70_001), "{case:?}");
        let updates = pawn_property_updates(&profile, &reply);
        assert_eq!(updates[0].len(), 1);
        assert_eq!(
            updates[0][0],
            (
                BlockTarget::Actor,
                BTreeMap::from([(handle::ICARUS_UID, RepValue::Int(70_001))])
            )
        );
        // An omitted placement restores as zero (F-963) and Location is never sent.
        // Restored stomach entries are dropped from an update that still goes.
        let gap = case == Case::Dead;
        let error = matches!(
            case,
            Case::RestoreError | Case::MissingStomach | Case::FramingError
        );
        let sent = !gap && !error;
        assert_eq!(updates.len(), 1 + usize::from(sent), "{case:?}");
        if sent {
            let mut pawn = BTreeMap::from([
                (
                    handle::REPLICATED_CONTROL_ROTATION,
                    RepValue::Rotator(CompressedRotator::from_degrees(0.0, 90.0, 0.0).unwrap()),
                ),
                (handle::RESPAWN_COUNT, RepValue::Int(2)),
                (handle::HAS_GRANTED_LOADOUT, RepValue::Bool(true)),
            ]);
            if matches!(case, Case::Full | Case::Omitted | Case::Stomach) {
                pawn.insert(
                    handle::COSMETIC_ARMOUR_OVERRIDES,
                    RepValue::DynamicArray(RepArray {
                        count: 2,
                        stream: PropertyStream {
                            entries: [2, 0]
                                .into_iter()
                                .zip(1..)
                                .map(|(row, handle)| RepEntry {
                                    handle,
                                    value: RepValue::UInt16(row),
                                    checksum: None,
                                })
                                .collect(),
                            terminator_checksum: None,
                        },
                    }),
                );
            }
            assert_eq!(updates[1][0], (BlockTarget::Actor, pawn));
            if case == Case::Unrecorded {
                assert_eq!(
                    updates[1].len(),
                    1,
                    "only pawn handles when levels are unrecorded"
                );
            } else {
                assert_eq!(updates[1].len(), 2);
                let BlockTarget::SubObject { object, class } = &updates[1][1].0 else {
                    panic!("ActorState block");
                };
                assert_ne!(object.guid, 0);
                assert_eq!(*class, SubObjectClass::StablyNamed);
                let mut expected = BTreeMap::from([
                    (h::WATER, RepValue::Int(0)),
                    (h::FOOD, RepValue::Int(50)),
                    (h::RADIATION, RepValue::Int(900)),
                ]);
                if case != Case::NoPak {
                    expected.insert(h::HEALTH, RepValue::Int(125));
                }
                // No pak: health clamps to the lab maximum (already 100).
                expected.insert(h::CURRENT_ALIVE_STATE, RepValue::Enum(0));
                assert_eq!(updates[1][1].1, expected);
            }
        }
        let stats = app.world().resource::<PeerLab>().stats();
        assert_eq!(stats.returning_pawns_restored, 1);
        assert_eq!(stats.returning_pawn_errors, 0);
        assert_eq!(
            (
                stats.returning_pawn_updates_sent,
                stats.returning_pawn_update_gaps,
                stats.returning_pawn_update_errors
            ),
            (u64::from(sent), u64::from(gap), u64::from(error)),
            "{case:?}"
        );
        assert_eq!(
            stats.returning_pawn_stomach_entries_dropped,
            u64::from(case == Case::Stomach),
            "{case:?}"
        );
        let counts = counts(&app);
        assert_eq!(counts.joined, 1, "gap/error keeps the session");
        assert_eq!(
            counts.returning_pawn_stomach_entries_dropped,
            stats.returning_pawn_stomach_entries_dropped
        );
        assert_eq!(
            (
                counts.returning_pawn_updates_sent,
                counts.returning_pawn_update_gaps,
                counts.returning_pawn_update_errors
            ),
            (
                stats.returning_pawn_updates_sent,
                stats.returning_pawn_update_gaps,
                stats.returning_pawn_update_errors
            )
        );
        let status = serde_json::to_string(&counts).unwrap();
        let debug = format!("{:?}", app.world().resource::<PeerLab>());
        for text in [&status, &debug] {
            for private in [NET_ID_HEX, "Armour_Head_A", "AuthoredModifier", "70001"] {
                assert!(!text.contains(private));
            }
        }
        with_server(&mut app, move || client.close().unwrap());
    }
}

#[test]
fn returning_pawns_retake_their_saved_uid_once_per_live_session() {
    use icarus_bevy::{actor_uids::FIRST_NATIVE_OBJECT_ID, prospect};
    let profile = ReleaseProfile::load(&release_toml()).unwrap();
    let pinned = profile.pin.network_checksum;
    let engine = profile.engine_network_version;
    // The probe logs in with NET_ID_HEX; the saved ID differs only in ASCII case.
    let payload = returning_character_payload(&NET_ID_HEX.to_ascii_lowercase(), 3, 70_001);
    let network = NetworkConfig {
        release_profile: Some(release_toml()),
        ..enabled(0)
    };
    let mut lab = network::bind(&network).unwrap().unwrap().lab;
    lab.seed_actor_uids(prospect::saved_actor_uids_from_payload(&payload).unwrap())
        .unwrap();
    let characters = prospect::saved_player_characters_from_payload(&payload).unwrap();
    assert_eq!(characters.len(), 1);
    lab.enable_returning_characters(characters).unwrap();
    let mut app = build_app(&ServerConfig::default(), &WorldSnapshot::default()).unwrap();
    let address = network::install(&mut app, lab).unwrap();
    let claimed = |app: &App| {
        let mut uids = app
            .world()
            .resource::<PeerLab>()
            .claimed_actor_uids()
            .to_vec();
        uids.sort_unstable();
        uids
    };
    assert_eq!(claimed(&app), [70_001]);

    // The profile's slot controls restore, even when Begin requested another slot.
    let ((first, first_open, unmatched), _) = with_server(&mut app, move || {
        let (mut client, bytes) = join_pinned_open(address, pinned, 7);
        let unmatched = exchange_profiles(&mut client, engine, 3, 5);
        (client, bytes, unmatched)
    });
    assert_eq!(
        sent_pawn_uid(&profile, &first_open),
        Some(FIRST_NATIVE_OBJECT_ID)
    );
    assert_eq!(pawn_uid_update(&profile, &unmatched), None);
    assert!(pawn_property_updates(&profile, &unmatched).is_empty());
    assert_eq!(counts(&app).returning_pawn_updates_sent, 0);
    assert_eq!(counts(&app).returning_pawn_update_gaps, 0);
    assert_eq!(counts(&app).returning_pawn_update_errors, 0);
    let ((second, second_open, restored), _) = with_server(&mut app, move || {
        let (mut client, bytes) = join_pinned_open(address, pinned, 7);
        let restored = exchange_profiles(&mut client, engine, 5, 3);
        (client, bytes, restored)
    });
    assert_eq!(
        sent_pawn_uid(&profile, &second_open),
        Some(FIRST_NATIVE_OBJECT_ID + 1)
    );
    assert_eq!(pawn_uid_update(&profile, &restored), Some(70_001));
    assert_eq!(claimed(&app), [FIRST_NATIVE_OBJECT_ID, 70_001]);
    {
        let stats = app.world().resource::<PeerLab>().stats();
        assert_eq!(
            (
                stats.returning_pawns_restored,
                stats.returning_pawns_unmatched
            ),
            (1, 1)
        );
        assert_eq!(stats.returning_pawn_errors, 0);
    }

    // A live session holds the character; another session with the same
    // login cannot take it again.
    let ((third, third_reply), _) = with_server(&mut app, move || {
        let (mut client, _) = join_pinned_open(address, pinned, 7);
        let reply = exchange_profiles(&mut client, engine, 3, 3);
        (client, reply)
    });
    assert_eq!(pawn_uid_update(&profile, &third_reply), None);
    assert_eq!(
        app.world()
            .resource::<PeerLab>()
            .stats()
            .returning_pawns_unmatched,
        2
    );

    // Closing keeps the saved UID claimed for its record and frees the
    // character for the next session.
    with_server(&mut app, move || {
        second.close().unwrap();
        third.close().unwrap();
    });
    assert_eq!(claimed(&app), [FIRST_NATIVE_OBJECT_ID, 70_001]);
    let ((fourth, reply), _) = with_server(&mut app, move || {
        let (mut client, _) = join_pinned_open(address, pinned, 7);
        let reply = exchange_profiles(&mut client, engine, 3, 3);
        (client, reply)
    });
    assert_eq!(pawn_uid_update(&profile, &reply), Some(70_001));
    assert_eq!(
        app.world()
            .resource::<PeerLab>()
            .stats()
            .returning_pawns_restored,
        2
    );
    with_server(&mut app, move || {
        first.close().unwrap();
        fourth.close().unwrap();
    });
    assert_eq!(claimed(&app), [70_001]);
}

#[test]
fn returning_characters_need_a_staged_save_and_actor_uids() {
    use icarus_bevy::prospect;
    // Version 0 drops every recorder; an unregistered class does not stage.
    let legacy = uid_save_payload(&[vec![scalar::Tag::integer("IcarusActorGUID", 5)]]);
    assert!(
        prospect::saved_player_characters_from_payload(&legacy)
            .unwrap()
            .is_empty()
    );
    let known = b"PlayerStateRecorderComponent".to_vec();
    let unknown = b"PlayerStateRecorderComponenX".to_vec();
    let mut unstaged = returning_character_payload("someone", 0, 9);
    let at = unstaged
        .windows(known.len())
        .position(|window| window == known.as_slice())
        .unwrap();
    unstaged[at..at + known.len()].copy_from_slice(&unknown);
    assert!(prospect::saved_player_characters_from_payload(&unstaged).is_err());
    let profile = ReleaseProfile::load(&release_toml()).unwrap();
    let mut bare = PeerLab::bind(
        0,
        network::peer_lab_config(&enabled(0)).with_release_profile(&profile),
    )
    .unwrap();
    assert!(bare.enable_returning_characters(Vec::new()).is_err());
}

#[test]
fn returning_pawns_never_share_a_saved_uid_across_duplicate_records() {
    use icarus_bevy::prospect;
    let profile = ReleaseProfile::load(&release_toml()).unwrap();
    let pinned = profile.pin.network_checksum;
    let engine = profile.engine_network_version;
    // Two saved records for one character share a UID; a third has its own.
    let payload = returning_characters_payload(NET_ID_HEX, 3, &[70_001, 70_001, 70_002]);
    let network = NetworkConfig {
        release_profile: Some(release_toml()),
        ..enabled(0)
    };
    let mut lab = network::bind(&network).unwrap().unwrap().lab;
    lab.seed_actor_uids(prospect::saved_actor_uids_from_payload(&payload).unwrap())
        .unwrap();
    lab.enable_returning_characters(
        prospect::saved_player_characters_from_payload(&payload).unwrap(),
    )
    .unwrap();
    let mut app = build_app(&ServerConfig::default(), &WorldSnapshot::default()).unwrap();
    let address = network::install(&mut app, lab).unwrap();
    let mut updates = Vec::new();
    let mut clients = Vec::new();
    for _ in 0..3 {
        let ((client, reply), _) = with_server(&mut app, move || {
            let (mut client, _) = join_pinned_open(address, pinned, 7);
            let reply = exchange_profiles(&mut client, engine, 3, 3);
            (client, reply)
        });
        updates.push(pawn_uid_update(&profile, &reply));
        clients.push(client);
    }
    // The duplicate of a held UID is skipped; the distinct record is used.
    assert_eq!(updates, [Some(70_001), Some(70_002), None]);
    let mut claimed = app
        .world()
        .resource::<PeerLab>()
        .claimed_actor_uids()
        .to_vec();
    claimed.sort_unstable();
    claimed.dedup();
    assert_eq!(
        claimed.len(),
        app.world().resource::<PeerLab>().claimed_actor_uids().len(),
        "no UID is claimed twice"
    );
    assert!(claimed.contains(&70_001) && claimed.contains(&70_002));
    let stats = app.world().resource::<PeerLab>().stats();
    assert_eq!(
        (
            stats.returning_pawns_restored,
            stats.returning_pawns_unmatched
        ),
        (2, 1)
    );
    with_server(&mut app, move || {
        for client in clients {
            client.close().unwrap();
        }
    });
}

/// The initialising probe's synthetic NULL-type login text; its saved
/// character carries exactly this PlayerID.
const PLAYER_TEXT: &str = "IcarusProbe-Player-7c1F";

#[test]
fn the_initialising_probe_login_id_decodes_to_its_exact_text() {
    use icarus_bevy::experimental::{
        admission::{check_unique_net_id_compatible, unique_net_id_is_valid},
        control_messages::{Decoded, encode_control_message, read_control_messages},
        packet::BitReader,
        peer_lab::PeerLabAdmission,
        probe_initialise::player_unique_net_id,
        session_open::replicated_unique_net_id,
    };
    let admission = PeerLabAdmission::default();
    // A decimal Steam-like text stays text: it is never sent as encoded hex.
    for text in [PLAYER_TEXT, "76561190000000042"] {
        let login = ControlMessage::Login {
            response: ArchiveText::text("0").unwrap(),
            url: ArchiveText::text("?Name=IcarusProbe").unwrap(),
            unique_net_id: player_unique_net_id(text).unwrap(),
            platform: ArchiveText::text("").unwrap(),
        };
        let codec = ControlCodec::default();
        let bytes = encode_control_message(&login, &codec).unwrap();
        let decoded = read_control_messages(
            &mut BitReader::new(&bytes, bytes.len() * 8).unwrap(),
            &codec,
        )
        .unwrap();
        let [Decoded::Message(ControlMessage::Login { unique_net_id, .. })] = decoded.as_slice()
        else {
            panic!("one Login")
        };
        assert_eq!(
            unique_net_id.kind,
            NetIdType::Named(ArchiveText::text("NULL").unwrap())
        );
        let NetIdValue::Text(value) = &unique_net_id.value else {
            panic!("a text id")
        };
        assert_eq!(value.units(), text.encode_utf16().collect::<Vec<_>>());
        assert!(unique_net_id_is_valid(unique_net_id));
        check_unique_net_id_compatible(
            unique_net_id,
            admission.default_subsystem,
            admission.compatible_subsystems,
            admission.indexed_subsystems,
        )
        .unwrap();
        // Replicated by NULL's index with the text unchanged.
        let replicated = replicated_unique_net_id(unique_net_id, admission.indexed_subsystems);
        assert_eq!(replicated.kind, NetIdType::Indexed(1));
        assert_eq!(replicated.value, unique_net_id.value);
    }
    assert!(player_unique_net_id("").is_err());
}

#[test]
fn the_initialiser_finds_a_player_controller_open_on_any_channel() {
    use icarus_bevy::experimental::{
        archive::ByteOrder,
        packet::{Packet, PacketHeader},
        peer_lab::PeerLabAdmission,
        probe_initialise::{ActorChannelTracker, initialise_codec},
        sequencing::{
            MAX_CONSTRUCTED_PARTIAL_BUNCH_BYTES, max_single_bunch_size_bits, split_outgoing_bunch,
        },
        session_open::{ConnectionExports, NetGuidCache, compose_player_controller_open},
    };
    let profile = ReleaseProfile::load(&release_toml()).unwrap();
    let mut plan = profile
        .player_controller_open_plan(PeerLabAdmission::default().map_url, network::SPAWN_LOCATION)
        .unwrap()
        .unwrap();
    plan.channel_index = 37;
    let mut cache = NetGuidCache::new();
    cache.register_level(&plan.profile.level);
    let open = compose_player_controller_open(
        &mut cache,
        &mut ConnectionExports::new(),
        &plan,
        0,
        ByteOrder::Little,
    )
    .unwrap();
    // The incoming reliable seed wraps: the parts carry 0 and 1.
    let seed = 1023;
    let mut bunch = open.bunch.clone();
    bunch.header.reliable_sequence_raw = Some(0);
    let parts = split_outgoing_bunch(
        &bunch,
        std::slice::from_ref(&open.exports),
        max_single_bunch_size_bits(1024, 2).unwrap(),
        Some(MAX_CONSTRUCTED_PARTIAL_BUNCH_BYTES),
    )
    .unwrap();
    assert_eq!(parts.len(), 2, "exports, then the body");
    let packet = |sequence, part: &icarus_bevy::experimental::packet::Bunch| Packet {
        magic: 0,
        header: PacketHeader {
            sequence,
            acked_sequence: 0,
            history: vec![0],
        },
        info: None,
        bunches: vec![part.clone()],
    };
    let mut tracker = ActorChannelTracker::new(
        PacketConfig {
            receiver: PacketReceiver::Client,
            ..probe_config().packet
        },
        initialise_codec(profile.engine_network_version).actor,
        seed,
    );
    let mut notes = Vec::new();
    // The body arrives first and waits for the export part.
    assert!(
        tracker
            .feed_packet(packet(10, &parts[1]), &mut notes)
            .is_empty()
    );
    let complete = tracker.feed_packet(packet(11, &parts[0]), &mut notes);
    assert!(notes.is_empty(), "{notes:?}");
    let [open_bunch] = complete.as_slice() else {
        panic!("one complete open")
    };
    assert_eq!(
        (open_bunch.channel_index, open_bunch.open, open_bunch.parts),
        (37, true, 2)
    );
    assert_eq!(open_bunch.payload, open.bunch.payload);
    assert_eq!(
        tracker.open_archetype(open_bunch).unwrap().as_deref(),
        Some(
            "/Game/BP/Player/BP_IcarusPlayerControllerSurvival.Default__BP_IcarusPlayerControllerSurvival_C"
        )
    );
    // A resent part is dropped by reliable order.
    assert!(
        tracker
            .feed_packet(packet(12, &parts[0]), &mut notes)
            .is_empty()
    );
    assert_eq!(notes, ["channel 37: duplicate reliable bunch"]);
}

/// Join with the initialising probe's NULL-type text id and run its
/// initialisation until it completes, then two more windows. Returns the
/// client, the events without packet IDs, the notes and the datagrams of
/// those two windows.
fn initialise_player(
    server: SocketAddr,
    version: u32,
    engine: u32,
    slot: i32,
) -> (
    ProbeClient,
    Vec<Value>,
    std::collections::BTreeMap<String, u64>,
    Vec<Vec<u8>>,
) {
    use icarus_bevy::experimental::probe_initialise::{
        InitialiseConfig, PlayerInitialiser, player_unique_net_id,
    };
    let mut client = connect(server);
    let mut initialiser = PlayerInitialiser::new(
        &client,
        InitialiseConfig {
            chr_slot: slot,
            engine_net_version: engine,
        },
    );
    let report = client.hello(version, "").unwrap();
    client
        .send_control(
            &[ControlMessage::Login {
                response: ArchiveText::text("0").unwrap(),
                url: ArchiveText::text(&format!("?Name={NAME_SENTINEL}")).unwrap(),
                unique_net_id: player_unique_net_id(PLAYER_TEXT).unwrap(),
                platform: ArchiveText::text("").unwrap(),
            }],
            false,
        )
        .unwrap();
    let welcome = client.collect(Duration::from_secs(3), 1).unwrap();
    assert!(matches!(
        welcome.messages().collect::<Vec<_>>().as_slice(),
        [ControlMessage::Welcome { .. }]
    ));
    let mut events = Vec::new();
    for reply in report.replies.iter().chain(&welcome.replies) {
        events.extend(initialiser.on_reply(&mut client, reply).unwrap());
    }
    client
        .send_control(
            &[
                ControlMessage::Netspeed { rate: 100_000 },
                ControlMessage::Join,
            ],
            false,
        )
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut datagrams = Vec::new();
    let mut after = 0;
    while after < 2 {
        let complete = initialiser.is_complete();
        let collected = client.collect(Duration::from_millis(250), 64).unwrap();
        for reply in &collected.replies {
            if complete {
                datagrams.push(reply.bytes.clone());
            }
            events.extend(initialiser.on_reply(&mut client, reply).unwrap());
        }
        client.keepalive().unwrap();
        after += usize::from(initialiser.is_complete());
        assert!(
            Instant::now() < deadline,
            "incomplete: {:?} {:?}",
            initialiser.summary(),
            initialiser.notes()
        );
    }
    let events = events
        .iter()
        .map(|event| {
            let mut value = serde_json::to_value(event).unwrap();
            value.as_object_mut().unwrap().remove("packet_id");
            value
        })
        .collect();
    (client, events, initialiser.notes().clone(), datagrams)
}

#[test]
fn the_initialising_probe_restores_its_saved_character_through_the_native_exchange() {
    use icarus_bevy::{experimental::session_open::PLAYER_CONTROLLER_CHANNEL_INDEX, prospect};
    use serde_json::json;
    let profile = ReleaseProfile::load(&release_toml()).unwrap();
    let pinned = profile.pin.network_checksum;
    let engine = profile.engine_network_version;
    let payload = returning_character_payload(PLAYER_TEXT, 3, 70_001);
    let mut lab = network::bind(&NetworkConfig {
        release_profile: Some(release_toml()),
        ..enabled(0)
    })
    .unwrap()
    .unwrap()
    .lab;
    lab.seed_actor_uids(prospect::saved_actor_uids_from_payload(&payload).unwrap())
        .unwrap();
    lab.enable_returning_characters(
        prospect::saved_player_characters_from_payload(&payload).unwrap(),
    )
    .unwrap();
    let mut app = build_app(&ServerConfig::default(), &WorldSnapshot::default()).unwrap();
    let address = network::install(&mut app, lab).unwrap();
    let ((client, events, notes, datagrams), _) = with_server(&mut app, move || {
        initialise_player(address, pinned, engine, 3)
    });
    assert_eq!(
        events,
        [
            json!({"event":"controller_channel", "channel":PLAYER_CONTROLLER_CHANNEL_INDEX,
                "archetype":"/Game/BP/Player/BP_IcarusPlayerControllerSurvival.Default__BP_IcarusPlayerControllerSurvival_C"}),
            json!({"event":"begin_sent", "channel":PLAYER_CONTROLLER_CHANNEL_INDEX, "chr_slot":3,
                "slot_sent":true}),
            json!({"event":"friends_list_sent", "channel":PLAYER_CONTROLLER_CHANNEL_INDEX}),
            json!({"event":"profile_request", "profile":"user", "requested_slot":null}),
            json!({"event":"profile_answered", "profile":"user", "chr_slot":null}),
            json!({"event":"profile_request", "profile":"character", "requested_slot":3}),
            json!({"event":"profile_answered", "profile":"character", "chr_slot":3}),
            json!({"event":"loadout_answered", "requested_slot":3}),
            json!({"event":"bestiary_answered"}),
            json!({"event":"associated_prospect", "requested_slot":3}),
        ]
    );
    assert!(notes.is_empty(), "{notes:?}");
    // The native server completed: the pawn took the saved UID.
    assert_eq!(pawn_uid_update(&profile, &datagrams), Some(70_001));
    let stats = app.world().resource::<PeerLab>().stats();
    assert_eq!(
        (stats.returning_pawns_restored, stats.returning_pawn_errors),
        (1, 0)
    );
    let status = counts(&app);
    assert_eq!(
        (status.joined, status.initialisation_slots_recorded),
        (1, 1)
    );
    assert_eq!(status.initialisation_calls_ignored, 0);
    with_server(&mut app, move || client.close().unwrap());
}

#[test]
fn returning_pawns_restore_their_saved_inventories_in_the_update() {
    use icarus_bevy::experimental::{
        actor_channel::{ActorChannelCodec, BlockTarget, SubObjectClass, read_actor_bunch_body},
        archive::ByteOrder,
        packet::read_packet,
        pawn_open::PAWN_CHANNEL_INDEX,
        prospect_info::{SavedActorInventory, SavedItem},
        rep_layout::{RepLayoutCodec, RepValue, read_rep_properties},
    };
    use icarus_bevy::prospect;
    use scalar::units;
    let json = |value: serde_json::Value| serde_json::to_vec(&value).unwrap();
    // Hands (D_InventoryID 1) has three slots: Any, Any, Fists.
    let tables = vec![
        (
            "Inventory/D_InventoryInfo.json",
            json(serde_json::json!({"Rows": [
                {"Name": "Unused", "StartingSlots": 4, "InventoryID": {"Value": "Undefined"}},
                {"Name": "Hands", "SlotTemplate": {"RowName": "Any"}, "StartingSlots": 3,
                 "InventoryID": {"Value": "Hands"},
                 "SlotOverrides": [{"Query": {"RowName": "Fists", "DataTableName": "D_TagQueries"}, "Location": 2}]},
                {"Name": "Bag", "SlotTemplate": {"RowName": "Any"}, "InventoryID": {"Value": "Bag"}},
            ]})),
        ),
        (
            "Inventory/D_InventoryID.json",
            pak_writer::table_json(&["Undefined", "Hands", "Bag"]),
        ),
        (
            "Tags/D_TagQueries.json",
            json(serde_json::json!({"Rows": [
                {"Name": "Q0", "Query": {"QueryTokenStream": [], "TagDictionary": []}},
                // ALL() of no expressions: every item.
                {"Name": "Any", "Query": {"QueryTokenStream": [0, 1, 5, 0], "TagDictionary": []}},
                {"Name": "Fists", "Query": {"QueryTokenStream": [0, 1, 1, 1, 0],
                 "TagDictionary": [{"TagName": "Item.Fist"}]}},
                {"Name": "Any_Oxygen", "Query": {"QueryTokenStream": [], "TagDictionary": []}},
                {"Name": "Any_Food", "Query": {"QueryTokenStream": [], "TagDictionary": []}},
                {"Name": "Any_Water", "Query": {"QueryTokenStream": [], "TagDictionary": []}},
                {"Name": "Any_Weighable_Resource_Container", "Query": {"QueryTokenStream": [0, 1, 1, 1, 0],
                 "TagDictionary": [{"TagName": "Item.Container"}]}},
                {"Name": "Any_Bag", "Query": {"QueryTokenStream": [0, 1, 1, 1, 0],
                 "TagDictionary": [{"TagName": "Item.Bag"}]}},
            ]})),
        ),
        (
            "Items/D_Itemable.json",
            json(serde_json::json!({"Rows": [{"Name": "Item_Ore", "Weight": 10}]})),
        ),
        (
            "Items/D_ItemsStatic.json",
            json(serde_json::json!({"Rows": [
                {"Name": "Ore", "Itemable": {"RowName": "Item_Ore"},
                 "Generated_Tags": {"GameplayTags": [{"TagName": "Item.Resource.Ore"}], "ParentTags": []}},
                {"Name": "Sword", "Durable": {"RowName": "Sword"},
                 "Generated_Tags": {"GameplayTags": [{"TagName": "Item.Weapon"}], "ParentTags": []}},
            ]})),
        ),
        (
            "Traits/D_Decayable.json",
            json(serde_json::json!({"Rows": []})),
        ),
        (
            "Traits/D_Equippable.json",
            json(serde_json::json!({"Rows": []})),
        ),
        (
            "Traits/D_Armour.json",
            json(serde_json::json!({"Rows": []})),
        ),
        (
            "Flags/D_SessionFlags.json",
            pak_writer::table_json(&["Flag_A"]),
        ),
    ];
    for gap in [false, true] {
        let profile = ReleaseProfile::load(&release_toml()).unwrap();
        let pinned = profile.pin.network_checksum;
        let engine = profile.engine_network_version;
        let payload = returning_characters_payload_with_state(NET_ID_HEX, 3, &[70_001], &[]);
        let directory = tempfile::tempdir().unwrap();
        let pak = directory.path().join("data.pak");
        fs::write(&pak, pak_writer::official_pak(&tables)).unwrap();
        let mut lab = network::bind(&NetworkConfig {
            release_profile: Some(release_toml()),
            data_pak: Some(pak),
            ..enabled(0)
        })
        .unwrap()
        .unwrap()
        .lab;
        lab.seed_actor_uids(prospect::saved_actor_uids_from_payload(&payload).unwrap())
            .unwrap();
        let mut characters = prospect::saved_player_characters_from_payload(&payload).unwrap();
        let item = |row: &str, dynamic: &[(i32, i32)]| SavedItem {
            location: 0,
            row_name: units(row),
            dynamic: dynamic.to_vec(),
            owner_lookup_id: -1,
            ..SavedItem::default()
        };
        // A durable item without its saved durability is a gap (F-976).
        characters[0].inventories = vec![SavedActorInventory {
            id: 1,
            items: vec![if gap {
                item("Sword", &[])
            } else {
                item("Ore", &[(7, 3)])
            }],
        }];
        lab.enable_returning_characters(characters).unwrap();
        let mut app = build_app(&ServerConfig::default(), &WorldSnapshot::default()).unwrap();
        let address = network::install(&mut app, lab).unwrap();
        let ((_client, reply), _) = with_server(&mut app, move || {
            let (mut client, _) = join_pinned_open(address, pinned, 7);
            let reply = exchange_profiles(&mut client, engine, 3, 3);
            (client, reply)
        });
        // The restore update carries one class-named inventory block unless
        // the inventories meet a gap.
        let codec = ActorChannelCodec {
            engine_net_version: engine,
            exports_enabled: false,
            has_level: true,
            order: ByteOrder::Little,
            limits: Default::default(),
        };
        let inventory_blocks: Vec<_> = reply
            .iter()
            .flat_map(|bytes| {
                read_packet(
                    bytes,
                    &PacketConfig {
                        receiver: PacketReceiver::Client,
                        ..probe_config().packet
                    },
                )
                .unwrap()
                .bunches
            })
            .filter(|bunch| bunch.header.channel_index == PAWN_CHANNEL_INDEX && !bunch.header.open)
            .flat_map(|bunch| {
                assert!(!bunch.header.partial);
                read_actor_bunch_body(&mut bunch.payload.reader(), false, &codec)
                    .unwrap()
                    .blocks
                    .into_iter()
                    .filter(|block| {
                        matches!(
                            block.target,
                            BlockTarget::SubObject {
                                class: SubObjectClass::Class(_),
                                ..
                            }
                        )
                    })
                    .collect::<Vec<_>>()
            })
            .collect();
        assert_eq!(inventory_blocks.len(), usize::from(!gap));
        // Three ore of weight 10: the restored weight goes ahead of the
        // slots (F-982).
        if let Some(block) = inventory_blocks.first() {
            let plan = profile
                .pawn_open_plan("/Game/Maps/Terrain_016_OLY/Terrain_016")
                .unwrap()
                .unwrap();
            assert!(block.has_rep_layout);
            let properties = read_rep_properties(
                &mut block.payload.reader(),
                &plan.content.inventory_layout,
                &RepLayoutCodec {
                    actor: codec,
                    checksums: false,
                    limits: Default::default(),
                },
            )
            .unwrap();
            assert_eq!(properties.entries.len(), 1);
            assert_eq!(properties.entries[0].handle, 4);
            assert_eq!(properties.entries[0].value, RepValue::Int(30));
        }
        let stats = app.world().resource::<PeerLab>().stats();
        assert_eq!(
            (
                stats.returning_pawn_updates_sent,
                stats.returning_inventories_sent,
                stats.returning_inventory_gaps,
                stats.returning_pawn_update_errors,
            ),
            (1, u64::from(!gap), u64::from(gap), 0),
            "gap {gap}"
        );
        let counts = counts(&app);
        assert_eq!(
            (
                counts.returning_inventories_sent,
                counts.returning_inventory_gaps
            ),
            (u64::from(!gap), u64::from(gap))
        );
    }
}

#[test]
fn returning_pawns_restore_their_saved_modifiers_in_the_update() {
    use icarus_bevy::experimental::{
        modifier_restore::KNOWN_BEHAVIOURS, prospect_info::SavedModifierState,
    };
    use icarus_bevy::prospect;
    use scalar::units;
    let json = |value: serde_json::Value| serde_json::to_vec(&value).unwrap();
    let tables = vec![
        (
            "Items/D_ItemsStatic.json",
            json(serde_json::json!({"Rows": [{"Name": "Ore"}]})),
        ),
        (
            "Modifiers/D_ModifierStates.json",
            json(serde_json::json!({"Rows": [
                {"Name": "Plain"},
                {"Name": "Unsaved", "SaveToDatabase": false},
                {"Name": "Other", "Behaviour": "/Game/Other.Other_C"},
                {"Name": "Drink", "Behaviour": KNOWN_BEHAVIOURS[2]},
            ]})),
        ),
        (
            "Flags/D_SessionFlags.json",
            pak_writer::table_json(&["Flag_A"]),
        ),
    ];
    let profile = ReleaseProfile::load(&release_toml()).unwrap();
    let pinned = profile.pin.network_checksum;
    let engine = profile.engine_network_version;
    let payload = returning_characters_payload_with_state(NET_ID_HEX, 3, &[70_001], &[]);
    let directory = tempfile::tempdir().unwrap();
    let pak = directory.path().join("data.pak");
    fs::write(&pak, pak_writer::official_pak(&tables)).unwrap();
    let mut lab = network::bind(&NetworkConfig {
        release_profile: Some(release_toml()),
        data_pak: Some(pak),
        ..enabled(0)
    })
    .unwrap()
    .unwrap()
    .lab;
    lab.seed_actor_uids(prospect::saved_actor_uids_from_payload(&payload).unwrap())
        .unwrap();
    let mut characters = prospect::saved_player_characters_from_payload(&payload).unwrap();
    let modifier = |row: &str| SavedModifierState {
        row_name: units(row),
        time_remaining: 100.0,
        initial_lifetime: 300.0,
        duration_buff: 1.0,
        effectiveness: -1,
    };
    // A native and a Blueprint one restore; one is not saved and one names
    // an unobserved class.
    characters[0].modifiers = vec![
        modifier("Plain"),
        modifier("Unsaved"),
        modifier("Other"),
        modifier("Drink"),
    ];
    lab.enable_returning_characters(characters).unwrap();
    let mut app = build_app(&ServerConfig::default(), &WorldSnapshot::default()).unwrap();
    let address = network::install(&mut app, lab).unwrap();
    with_server(&mut app, move || {
        let (mut client, _) = join_pinned_open(address, pinned, 7);
        exchange_profiles(&mut client, engine, 3, 3);
    });
    let stats = app.world().resource::<PeerLab>().stats();
    assert_eq!(
        (
            stats.returning_pawn_updates_sent,
            stats.returning_modifiers_sent,
            stats.returning_modifier_gaps,
            stats.returning_pawn_update_errors,
        ),
        (1, 2, 1, 0)
    );
    let counts = counts(&app);
    assert_eq!(
        (
            counts.returning_modifiers_sent,
            counts.returning_modifier_gaps
        ),
        (2, 1)
    );
}

#[test]
fn returning_pawns_restore_their_stats_and_the_bounds_they_set() {
    use icarus_bevy::experimental::{
        actor_channel::{ActorChannelCodec, BlockTarget, SubObjectClass, read_actor_bunch_body},
        archive::ByteOrder,
        packet::read_packet,
        pawn_open::{PAWN_CHANNEL_INDEX, character_state_handle as h},
        prospect_info::{SavedActorInventory, SavedItem},
        rep_layout::{
            RepCommand, RepLayoutCodec, RepValue, StatPairRepState,
            player_character_state_rep_layout, read_rep_properties,
        },
        rep_rpc::{
            FAST_ARRAY_COUNT_CAP, FastArrayItem, FastArrayItemLayout, read_custom_delta_header,
            read_fast_array_delta, read_field_header,
        },
    };
    use icarus_bevy::prospect;
    use scalar::units;
    let json = |value: serde_json::Value| serde_json::to_vec(&value).unwrap();
    let query = |name: &str, tag: &str| {
        serde_json::json!({"Name": name, "Query": {"QueryTokenStream": [0, 1, 1, 1, 0],
            "TagDictionary": [{"TagName": tag}]}})
    };
    let empty = || json(serde_json::json!({"Rows": []}));
    // Bag is the Equipment inventory, with one slot; a helmet adds 50 to
    // BaseMaximumHealth_+ (the starting grants give 250).
    let tables = vec![
        (
            "Inventory/D_InventoryInfo.json",
            json(serde_json::json!({"Rows": [
                {"Name": "Unused", "StartingSlots": 4, "InventoryID": {"Value": "Undefined"}},
                {"Name": "Hands", "SlotTemplate": {"RowName": "Any"}, "StartingSlots": 3,
                 "InventoryID": {"Value": "Hands"}},
                {"Name": "Bag", "SlotTemplate": {"RowName": "Any"}, "StartingSlots": 1,
                 "InventoryID": {"Value": "Equipment"}},
            ]})),
        ),
        (
            "Inventory/D_InventoryID.json",
            pak_writer::table_json(&["Undefined", "Hands", "Equipment"]),
        ),
        (
            "Tags/D_TagQueries.json",
            json(serde_json::json!({"Rows": [
                {"Name": "Q0", "Query": {"QueryTokenStream": [], "TagDictionary": []}},
                {"Name": "Any", "Query": {"QueryTokenStream": [0, 1, 5, 0], "TagDictionary": []}},
                {"Name": "Any_Oxygen", "Query": {"QueryTokenStream": [], "TagDictionary": []}},
                {"Name": "Any_Food", "Query": {"QueryTokenStream": [], "TagDictionary": []}},
                {"Name": "Any_Water", "Query": {"QueryTokenStream": [], "TagDictionary": []}},
                query("Any_Weighable_Resource_Container", "Item.Container"),
                query("Any_Bag", "Item.Bag"),
                query("Any_Tool", "Item.Tool"),
                query("Any_AnimalCorpse", "Harvest.Knife"),
                query("Any_Radar", "Item.Radar"),
                query("Any_Exotic", "Item.Meta.Resource"),
                query("Any_Extractor", "Item.Extractor"),
            ]})),
        ),
        (
            "Items/D_ItemsStatic.json",
            json(serde_json::json!({"Rows": [
                {"Name": "Helmet", "Itemable": {"RowName": "Item_Helmet"},
                 "Armour": {"RowName": "Plate_Head"},
                 "Generated_Tags": {"GameplayTags": [{"TagName": "Item.Armour"}], "ParentTags": []}},
            ]})),
        ),
        (
            "Items/D_Itemable.json",
            json(serde_json::json!({"Rows": [{"Name": "Item_Helmet", "Weight": 500}]})),
        ),
        (
            "Traits/D_Armour.json",
            json(serde_json::json!({"Rows": [{"Name": "Plate_Head",
                "ArmourStats": {"(Value=\"BaseMaximumHealth_+\")": 50}}]})),
        ),
        ("Armour/D_ArmourSets.json", empty()),
        ("Armour/D_ArmourSetBonus.json", empty()),
        ("Alterations/D_Alterations.json", empty()),
        (
            "Modifiers/D_ModifierStates.json",
            json(serde_json::json!({"Rows": [{"Name": "Overburdened"}]})),
        ),
        ("Traits/D_Decayable.json", empty()),
        ("Traits/D_Equippable.json", empty()),
        (
            "Flags/D_SessionFlags.json",
            pak_writer::table_json(&["Flag_A"]),
        ),
    ];
    let profile = ReleaseProfile::load(&release_toml()).unwrap();
    let pinned = profile.pin.network_checksum;
    let engine = profile.engine_network_version;
    let payload = returning_characters_payload_with_state(NET_ID_HEX, 3, &[70_001], &[]);
    let directory = tempfile::tempdir().unwrap();
    let pak = directory.path().join("data.pak");
    fs::write(&pak, pak_writer::official_pak(&tables)).unwrap();
    let mut lab = network::bind(&NetworkConfig {
        release_profile: Some(release_toml()),
        data_pak: Some(pak),
        ..enabled(0)
    })
    .unwrap()
    .unwrap()
    .lab;
    lab.seed_actor_uids(prospect::saved_actor_uids_from_payload(&payload).unwrap())
        .unwrap();
    // No loaded world: no container inventories to link.
    lab.set_linked_inventories(std::collections::BTreeMap::new())
        .unwrap();
    let mut characters = prospect::saved_player_characters_from_payload(&payload).unwrap();
    characters[0].inventories = vec![SavedActorInventory {
        id: 2,
        items: vec![SavedItem {
            location: 0,
            row_name: units("Helmet"),
            dynamic: vec![(7, 1)],
            owner_lookup_id: -1,
            ..SavedItem::default()
        }],
    }];
    lab.enable_returning_characters(characters).unwrap();
    let mut app = build_app(&ServerConfig::default(), &WorldSnapshot::default()).unwrap();
    let address = network::install(&mut app, lab).unwrap();
    let ((_client, reply), _) = with_server(&mut app, move || {
        let (mut client, _) = join_pinned_open(address, pinned, 7);
        let reply = exchange_profiles(&mut client, engine, 3, 3);
        (client, reply)
    });
    let stats = app.world().resource::<PeerLab>().stats();
    assert_eq!(
        (
            stats.returning_pawn_updates_sent,
            stats.returning_stats_sent,
            stats.returning_stat_gaps,
            stats.returning_pawn_update_errors,
        ),
        (1, 1, 0, 0)
    );
    let codec = ActorChannelCodec {
        engine_net_version: engine,
        exports_enabled: false,
        has_level: true,
        order: ByteOrder::Little,
        limits: Default::default(),
    };
    let rep_codec = RepLayoutCodec {
        actor: codec,
        checksums: false,
        limits: Default::default(),
    };
    let blocks: Vec<_> = reply
        .iter()
        .flat_map(|bytes| {
            read_packet(
                bytes,
                &PacketConfig {
                    receiver: PacketReceiver::Client,
                    ..probe_config().packet
                },
            )
            .unwrap()
            .bunches
        })
        .filter(|bunch| bunch.header.channel_index == PAWN_CHANNEL_INDEX && !bunch.header.open)
        .flat_map(|bunch| {
            assert!(!bunch.header.partial);
            read_actor_bunch_body(&mut bunch.payload.reader(), false, &codec)
                .unwrap()
                .blocks
        })
        .collect();
    // The stat container's block comes first of the components, then
    // ActorState's.
    let first = blocks
        .iter()
        .position(|block| {
            matches!(
                block.target,
                BlockTarget::SubObject {
                    class: SubObjectClass::StablyNamed,
                    ..
                }
            )
        })
        .unwrap();
    let stat = &blocks[first];
    assert!(!stat.has_rep_layout);
    let plan = profile
        .pawn_open_plan("/Game/Maps/Terrain_016_OLY/Terrain_016")
        .unwrap()
        .unwrap();
    let mut reader = stat.payload.reader();
    let field = read_field_header(&mut reader, plan.content.stat_container_bound, 1 << 20).unwrap();
    let mut field_reader = field.payload.reader();
    read_custom_delta_header(&mut field_reader, engine, 1).unwrap();
    let delta = read_fast_array_delta(
        &mut field_reader,
        &FastArrayItemLayout::Delta(vec![RepCommand::StatPairRepState]),
        &rep_codec,
        FAST_ARRAY_COUNT_CAP,
    )
    .unwrap();
    // MaximumHealth_+ (row 0, the open's item 1): 250 + 50.
    let FastArrayItem::Delta(item) = &delta.changed[0].1 else {
        panic!("delta item");
    };
    assert_eq!(delta.changed[0].0, 1);
    assert_eq!(
        item.entries[0].value,
        RepValue::StatPairRepState(StatPairRepState {
            stat: 0,
            value: 300
        })
    );
    let state = read_rep_properties(
        &mut blocks[first + 1].payload.reader(),
        &player_character_state_rep_layout(),
        &rep_codec,
    )
    .unwrap();
    assert!(
        state
            .entries
            .iter()
            .any(|entry| entry.handle == h::MAX_HEALTH && entry.value == RepValue::Int(300))
    );
}

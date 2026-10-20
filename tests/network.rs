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
    },
    network,
    simulation::{SimulationSet, WorldSnapshot, build_app},
};
use serde_json::Value;

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
    let lab = network::bind(&config.network).unwrap().unwrap();
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

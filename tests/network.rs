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
    client.close().unwrap();
    collected
        .replies
        .into_iter()
        .map(|reply| reply.bytes)
        .collect()
}

/// Compose the GameState open the lab sends after each PlayerController open
/// (F-794), with `world_time` for the lab-written handle 19.
fn compose_lab_game_state(
    profile: &ReleaseProfile,
    cache: &mut icarus_bevy::experimental::session_open::NetGuidCache,
    exports: &mut icarus_bevy::experimental::session_open::ConnectionExports,
    world_time: f32,
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
        // The lab then opens the GameState, which takes 20..26 once.
        compose_lab_game_state(&profile, &mut cache, &mut exports, 0.0);
        // Each connection gets its own dynamic GUIDs: actor 2, then 28.
        assert_eq!(expected.guids.actor, 2 + 26 * index as u32);
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
    use icarus_bevy::experimental::{
        actor_channel::{ActorChannelCodec, ActorChannelLimits, read_actor_bunch_body},
        archive::ByteOrder,
        game_state_open::handle,
        rep_layout::{RepLayoutCodec, RepLayoutLimits, RepValue, read_rep_properties},
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
    let entry = stream
        .entries
        .iter()
        .find(|entry| entry.handle == handle::WORLD_TIME_SECONDS)
        .unwrap();
    let RepValue::Float(bits) = entry.value else {
        panic!("world time is a float");
    };
    f32::from_bits(bits)
}

#[test]
fn pinned_joins_open_the_game_state_then_each_player_state() {
    use icarus_bevy::experimental::{
        actor_channel::{ActorChannelCodec, ActorChannelLimits, read_actor_bunch_body},
        archive::ByteOrder,
        control_messages::{ArchiveText, NetIdType},
        game_state_open::GAME_STATE_CHANNEL_INDEX,
        packet::read_packet,
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
            join_pinned_replies(address, pinned, 3),
            join_pinned_replies(address, pinned, 3),
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
        assert_eq!(datagrams.len(), 3, "controller, game state, player state");
        let mut exports = ConnectionExports::new();
        let controller_open = compose_player_controller_open(
            &mut cache,
            &mut exports,
            &controller,
            0,
            ByteOrder::Little,
        )
        .unwrap();
        // The GameState carries the lab's world time; compose with the sent
        // value and compare everything else bit for bit.
        let packet = read_packet(&datagrams[1], &to_client).unwrap();
        let [game_exports, game_body] = packet.bunches.as_slice() else {
            panic!("the GameState open travels as exports plus body");
        };
        assert_eq!(game_body.header.channel_index, GAME_STATE_CHANNEL_INDEX);
        let world_time = sent_world_time(&profile, &game_body.payload);
        let game_state = compose_lab_game_state(&profile, &mut cache, &mut exports, world_time);
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
        assert_eq!(spawn.actor.guid, 20);
        let state = PlayerStateOpenState {
            player_state: controller_open.guids.player_state,
            owner: controller_open.guids.actor,
            player_id: FIRST_PLAYER_ID + index as i32,
            unique_id: unique_id.clone(),
            player_name: ArchiveText::text(NAME_SENTINEL).unwrap(),
            is_host: index == 0,
        };
        let expected = compose_player_state_open(
            &mut cache,
            &mut exports,
            &state_plan,
            &state,
            ByteOrder::Little,
        )
        .unwrap();
        let packet = read_packet(&datagrams[2], &to_client).unwrap();
        let [state_exports, body] = packet.bunches.as_slice() else {
            panic!("the PlayerState open travels as exports plus body");
        };
        assert_eq!(body.header.channel_index, PLAYER_STATE_CHANNEL_INDEX);
        assert!(state_exports.header.open && state_exports.header.has_package_map_exports);
        assert_eq!(Some(&state_exports.payload), expected.exports.as_ref());
        assert_eq!(body.payload, expected.bunch.payload);
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
        session_replication::{NetArchetype, SessionReplication, SessionReplicationPlugin},
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
    }
    // Two sessions in turn: each gets both actors, under the same GUIDs.
    let ([replies, later], _) = with_server(&mut app, move || {
        [
            join_pinned_replies(address, pinned, 5),
            join_pinned_replies(address, pinned, 5),
        ]
    });
    let stats = app.world().resource::<PeerLab>().stats().clone();
    assert_eq!((stats.actor_opens_sent, stats.actor_open_errors), (4, 0));
    assert_eq!(
        replies.len(),
        5,
        "controller, game state, player state, then two actors"
    );
    assert_eq!(later.len(), 5);

    let to_client = PacketConfig {
        receiver: PacketReceiver::Client,
        ..probe_config().packet
    };
    // Channels 2 to 4 hold the PlayerController, GameState and PlayerState;
    // the actors take the next free ones. The second open exports the same
    // archetype again unless the client acked the first one's packet in
    // between (F-796).
    let packet = read_packet(&replies[3], &to_client).unwrap();
    let [exports, body] = packet.bunches.as_slice() else {
        panic!("the actor open travels as exports plus body");
    };
    assert_eq!(body.header.channel_index, 5);
    let second = read_packet(&replies[4], &to_client).unwrap();
    let second_body = second.bunches.last().unwrap();
    assert_eq!(second_body.header.channel_index, 6);
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
    // The first connection's controller took dynamic GUIDs 2..18 and the
    // GameState 20..26.
    assert_eq!(spawn.actor.guid, 28);
    assert!(spawn.dynamic.unwrap().location.is_some());
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
        let mut guids: Vec<u32> = datagrams[3..]
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
    assert_eq!(actor_guids(&replies), [28, 30]);
    assert_eq!(actor_guids(&later), [28, 30], "driver-wide GUIDs");
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
fn replicated_movement_opens_at_rest_and_updates_after_a_move() {
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
        replication::{NetActor, NetParameters, ReplicationPlugin},
        session_replication::{NetArchetype, SessionReplication, SessionReplicationPlugin},
        simulation::{DomainId, Position},
    };
    /// Authored motion: the actor moves once, a second after start.
    fn mover(time: Res<bevy::time::Time>, mut actors: Query<&mut Position>) {
        if time.elapsed_secs() > 1.0 {
            for mut position in &mut actors {
                position.0 = [500.0, -20.0, 30.0];
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
        .add_systems(FixedUpdate, mover)
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
        .filter(|bunch| bunch.header.channel_index == 5)
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
}

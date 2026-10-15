use std::{
    net::{SocketAddr, UdpSocket},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

use bevy::app::App;
use icarus_bevy::{
    config::ServerConfig,
    experimental::{
        control_messages::{ControlCodec, ControlMessage},
        default_version_string_checksum,
        handshake::{LegacyHandshake, encode},
        packet::{PacketConfig, Receiver},
        peer_lab::{PeerLab, PeerLabConfig, PeerLabPlugin},
        probe_client::{HelloReport, ProbeClient, ProbeConfig, ReplyDecode},
    },
    simulation::{WorldSnapshot, build_app},
};

// The probe against this repository's own peer lab: both sides share one
// model, so passing shows agreement with that model, not with the original.

fn lab_app(config: PeerLabConfig) -> (App, SocketAddr) {
    let mut app = build_app(&ServerConfig::default(), &WorldSnapshot::default()).unwrap();
    let lab = PeerLab::bind(0, config).unwrap();
    let address = lab.local_addr().unwrap();
    app.insert_resource(lab).add_plugins(PeerLabPlugin);
    (app, address)
}

fn probe_config() -> ProbeConfig {
    ProbeConfig {
        packet: PacketConfig {
            magic_bits: 0,
            receiver: Receiver::Server,
            limits: PeerLabConfig::default().packet.packet.limits,
        },
        codec: ControlCodec::default(),
        handshake_timeout: Duration::from_secs(2),
        handshake_attempts: 3,
        reply_window: Duration::from_millis(500),
        max_datagrams: 8,
    }
}

/// Run `probe` on its own thread while the lab updates on this one.
fn with_lab<T: Send + 'static>(
    app: &mut App,
    address: SocketAddr,
    probe: impl FnOnce(UdpSocket) -> T + Send + 'static,
) -> T {
    let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    socket.connect(address).unwrap();
    let (done, result) = mpsc::channel();
    let worker = thread::spawn(move || done.send(probe(socket)).unwrap());
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        app.update();
        if let Ok(value) = result.try_recv() {
            worker.join().unwrap();
            return value;
        }
        assert!(Instant::now() < deadline, "probe did not finish");
        thread::sleep(Duration::from_millis(2));
    }
}

fn hello(app: &mut App, address: SocketAddr, version: u32) -> HelloReport {
    with_lab(app, address, move |socket| {
        let mut client = ProbeClient::connect(socket, probe_config()).unwrap();
        assert_eq!(client.handshake().ack_datagram.len(), 29);
        client.hello(version, "").unwrap()
    })
}

#[test]
fn matching_version_gets_a_challenge_and_an_ack() {
    let version = default_version_string_checksum(
        "Icarus 3.0.30.158174-rel-DangerousHorizons, NetCL: 0, EngineNetVer: 17, GameNetVer: 0",
    );
    assert_eq!(version, 1_379_612_455);
    let (mut app, address) = lab_app(PeerLabConfig {
        local_network_version: Some(version),
        ..PeerLabConfig::default()
    });
    let report = hello(&mut app, address, version);
    assert!(report.hello_acked);
    assert!(matches!(
        report.messages().collect::<Vec<_>>().as_slice(),
        [ControlMessage::Challenge { .. }]
    ));
    assert!(!report.hello_datagram.is_empty());
}

#[test]
fn mismatched_version_reports_the_servers_version_in_upgrade() {
    let (mut app, address) = lab_app(PeerLabConfig {
        local_network_version: Some(0x0a0b_0c0d),
        ..PeerLabConfig::default()
    });
    let report = hello(&mut app, address, 1);
    assert_eq!(
        report.messages().cloned().collect::<Vec<_>>(),
        [ControlMessage::Upgrade {
            version: 0x0a0b_0c0d
        }]
    );
}

#[test]
fn undecodable_replies_are_kept_raw_not_fatal() {
    // A scripted server: a model-conforming handshake, then a reply to the
    // Hello that no packet model reads.
    let server = UdpSocket::bind("127.0.0.1:0").unwrap();
    let address = server.local_addr().unwrap();
    let script = thread::spawn(move || {
        let mut buffer = [0_u8; 2048];
        let (_, client) = server.recv_from(&mut buffer).unwrap();
        let challenge = LegacyHandshake {
            restart: false,
            secret_id: false,
            timestamp: 12.5,
            cookie: [7; 20],
        };
        server
            .send_to(&encode(&challenge).unwrap(), client)
            .unwrap();
        let _ = server.recv_from(&mut buffer).unwrap();
        let ack = LegacyHandshake {
            timestamp: -1.0,
            secret_id: true,
            ..challenge
        };
        server.send_to(&encode(&ack).unwrap(), client).unwrap();
        let _ = server.recv_from(&mut buffer).unwrap();
        server.send_to(&[0xff, 0x00, 0x13], client).unwrap();
    });
    let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    socket.connect(address).unwrap();
    let mut client = ProbeClient::connect(socket, probe_config()).unwrap();
    let report = client.hello(1, "").unwrap();
    script.join().unwrap();
    assert!(!report.hello_acked);
    assert!(report.messages().next().is_none());
    let [reply] = report.replies.as_slice() else {
        panic!("expected one reply, got {:?}", report.replies);
    };
    assert_eq!(reply.bytes, [0xff, 0x00, 0x13]);
    assert!(matches!(reply.decode, ReplyDecode::Undecoded(_)));
}

#[test]
fn a_silent_server_yields_an_error_not_a_hang() {
    let silent = UdpSocket::bind("127.0.0.1:0").unwrap();
    let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    socket.connect(silent.local_addr().unwrap()).unwrap();
    let config = ProbeConfig {
        handshake_timeout: Duration::from_millis(50),
        ..probe_config()
    };
    let started = Instant::now();
    assert!(ProbeClient::connect(socket, config).is_err());
    assert!(started.elapsed() < Duration::from_secs(2));
}

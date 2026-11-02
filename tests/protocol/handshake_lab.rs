use std::{net::UdpSocket, time::Duration};

use bevy::time::TimeUpdateStrategy;
use icarus_bevy::{
    config::ServerConfig,
    experimental::{
        handshake::{decode, initial_request},
        handshake_lab::{HandshakeLab, HandshakeLabPlugin, PACKETS_PER_UPDATE},
    },
    simulation::{SimulationClock, WorldSnapshot, build_app},
};

fn client(lab: &HandshakeLab) -> UdpSocket {
    let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    socket.connect(lab.local_addr().unwrap()).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(1)))
        .unwrap();
    socket
}

#[test]
fn bevy_lab_completes_handshake_without_creating_game_entities() {
    let mut app = build_app(&ServerConfig::default(), &WorldSnapshot::default()).unwrap();
    let lab = HandshakeLab::bind(0).unwrap();
    assert!(lab.local_addr().unwrap().ip().is_loopback());
    let socket = client(&lab);
    app.insert_resource(lab).add_plugins(HandshakeLabPlugin);
    socket.send(&initial_request()).unwrap();
    app.update();
    let mut buffer = [0; 64];
    let length = socket.recv(&mut buffer).unwrap();
    let challenge = decode(&buffer[..length]).unwrap();
    assert!(challenge.timestamp > 0.0);
    assert!(!challenge.secret_id);
    socket.send(&buffer[..length]).unwrap();
    app.update();
    let length = socket.recv(&mut buffer).unwrap();
    let ack = decode(&buffer[..length]).unwrap();
    assert_eq!(ack.timestamp, -1.0);
    assert_eq!(ack.cookie, challenge.cookie);
    assert!(ack.secret_id);
    let stats = app.world().resource::<HandshakeLab>().stats();
    assert_eq!(stats.challenges_sent, 1);
    assert_eq!(stats.acknowledgements_sent, 1);
    assert_eq!(stats.io_errors, 0);
    assert!(
        icarus_bevy::simulation::capture(app.world_mut())
            .unwrap()
            .entities
            .is_empty()
    );
}

#[test]
fn malformed_datagrams_consume_bounded_budget_while_simulation_advances() {
    let config = ServerConfig::default();
    let mut app = build_app(&config, &WorldSnapshot::default()).unwrap();
    let lab = HandshakeLab::bind(0).unwrap();
    let socket = client(&lab);
    app.insert_resource(lab)
        .insert_resource(TimeUpdateStrategy::ManualDuration(config.tick_interval()))
        .add_plugins(HandshakeLabPlugin);
    // Larger than the 30-byte receive buffer: must not accidentally accept a
    // valid short prefix after kernel truncation of the datagram.
    let mut oversized = [0; 64];
    oversized[..29].copy_from_slice(&initial_request());
    for _ in 0..PACKETS_PER_UPDATE + 1 {
        socket.send(&oversized).unwrap();
    }
    app.update();
    let first = app.world().resource::<HandshakeLab>().stats();
    assert_eq!(first.received, PACKETS_PER_UPDATE as u64);
    assert_eq!(first.rejected, first.received);
    let tick = app.world().resource::<SimulationClock>().tick;
    app.update();
    let second = app.world().resource::<HandshakeLab>().stats();
    assert_eq!(second.received, PACKETS_PER_UPDATE as u64 + 1);
    assert_eq!(second.rejected, second.received);
    assert_eq!(second.challenges_sent + second.acknowledgements_sent, 0);
    assert!(app.world().resource::<SimulationClock>().tick > tick);
}

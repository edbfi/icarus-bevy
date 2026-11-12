//! Operator spawn points through the lab's real open path (F-1139, F-1140):
//! the controller and pawn opens a client receives, decoded with the
//! repository's own readers, the default path's bunches pinned to the parent
//! commit, and the move store's starting point. Every location is authored
//! here; no captured or saved value is used.
use super::super::super::{
    actor_channel::{SpawnVector, read_player_controller_open_bunch_body, read_spawn},
    archive::ByteOrder,
    packet::{BitWriter, Bunch, Payload},
    pawn_open::PAWN_CHANNEL_INDEX,
    rep_layout::{RepLayoutCodec, RepValue, append_bits, read_rep_properties},
    session_open::quantize_spawn_vector,
};
use super::move_acks::{
    LAB, PRODUCTION, move_at, move_session_digests, moving_lab, moving_peer, packed,
    release_toml_config,
};
use super::*;
use crate::spawn_placement::{
    MAX_SPAWN_POINTS, Placement, PlacementSource, SpawnConfig, SpawnPoint,
};

/// The words the server's run log must never carry
/// (`bounded_native_binary_run_uses_the_same_authored_configuration_path`):
/// authored world data uses them, so spawn reporting says "spawn".
pub(super) const RUN_LOG_FORBIDDEN: [&str; 5] = [
    "IndependentBox",
    "authored",
    "catalog_id",
    "rotation",
    "placement",
];

/// Fail if serialized `value` carries any [`RUN_LOG_FORBIDDEN`] word.
fn assert_loggable(label: &str, value: &impl serde::Serialize) {
    let text = serde_json::to_string(value).unwrap();
    for word in RUN_LOG_FORBIDDEN {
        assert!(!text.contains(word), "{label} carries {word:?}");
    }
}

/// The authored points: whole and half centimetres, so `x * 10` is exact.
fn points() -> SpawnConfig {
    SpawnConfig {
        points: vec![
            SpawnPoint {
                x: 12_345.5,
                y: -65_432.0,
                z: 98_765.0,
                yaw: Some(90.0),
            },
            SpawnPoint {
                x: -1_000.0,
                y: 2_000.0,
                z: 300_000.0,
                yaw: None,
            },
        ],
    }
}

/// One joined peer whose initial opens were delivered and ACKed, returning
/// every bunch it received.
fn join_and_receive(
    lab: &mut PeerLab,
    connection: u32,
    user: &str,
) -> (UdpSocket, SocketAddrV4, Vec<Bunch>) {
    use super::super::super::control_messages::{NetIdType, NetIdValue, UniqueNetId};
    let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    socket.connect(lab.local_addr().unwrap()).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(1)))
        .unwrap();
    let (address, mut client) = handshake_peer_on(lab, &socket);
    let entry = lab.peers.get_mut(&address).unwrap();
    entry.connection_id = Some(connection);
    entry.player = Some(RegisteredPlayer {
        player_id: connection as i32,
        name_units: "AUTHORED-SPAWN".encode_utf16().collect(),
    });
    entry.login_unique_id = Some(UniqueNetId {
        kind: NetIdType::Named(ArchiveText::text("NULL").unwrap()),
        value: NetIdValue::Text(ArchiveText::text(user).unwrap()),
    });
    entry.profiles.character_slot = Some(3);
    assert!(lab.send_player_controller_open(address));
    let bunches = deliver_bunches(lab, &socket, &mut client, |_| true);
    (socket, address, bunches)
}

/// The open body on `channel` as the client reassembles it: the first
/// non-export bunch, with its following partial parts up to the final one.
fn open_body(bunches: &[Bunch], channel: u32) -> Payload {
    let mut parts = bunches.iter().filter(|bunch| {
        bunch.header.channel_index == channel
            && bunch.header.reliable
            && !bunch.header.has_package_map_exports
    });
    let first = parts.next().expect("an open body");
    if !first.header.partial {
        return first.payload.clone();
    }
    let mut body = BitWriter::new();
    append_bits(&mut body, &first.payload);
    for part in parts {
        append_bits(&mut body, &part.payload);
        if part.header.partial_final {
            break;
        }
    }
    Payload::from_writer(body)
}

/// The controller open's spawn location and handle 19 `SpawnLocation`, and
/// the pawn open's spawn location, read back from what the client received.
fn received_locations(
    lab: &PeerLab,
    bunches: &[Bunch],
) -> (Option<SpawnVector>, [u32; 3], Option<SpawnVector>) {
    let plan = lab.player_controller_open.as_ref().unwrap();
    let codec = actor_codec(plan.engine_net_version, ByteOrder::Little);
    let controller = read_player_controller_open_bunch_body(
        &mut open_body(bunches, plan.channel_index).reader(),
        &codec,
    )
    .unwrap();
    let rep_codec = RepLayoutCodec {
        actor: codec,
        checksums: false,
        limits: Default::default(),
    };
    let properties = read_rep_properties(
        &mut controller.blocks[0].payload.reader(),
        &plan.actor_layout,
        &rep_codec,
    )
    .unwrap();
    let [handle_19] = properties
        .entries
        .iter()
        .filter(|entry| entry.handle == 19)
        .collect::<Vec<_>>()[..]
    else {
        panic!("one SpawnLocation");
    };
    let RepValue::Vector(spawn_location) = handle_19.value else {
        panic!("SpawnLocation is a vector");
    };
    let pawn_engine = lab.pawn_open.as_ref().unwrap().0.engine_net_version;
    let pawn = read_spawn(
        &mut open_body(bunches, PAWN_CHANNEL_INDEX).reader(),
        &actor_codec(pawn_engine, ByteOrder::Little),
    )
    .unwrap();
    (
        controller.spawn.dynamic.unwrap().location,
        spawn_location,
        pawn.dynamic.unwrap().location,
    )
}

/// Three joins over two points (more joins than points): the first and
/// third open at the first point, the second at the second, in both opens.
/// Golden bits: the first point packs as 19 bits per component, biased by
/// `1 << 20`; handle 19 carries its exact `f32` bits; the pawn's spawn
/// record equals the controller's. The yaw is reported, never sent.
#[test]
fn configured_points_place_both_opens_round_robin() {
    let mut lab = record_lab(true);
    lab.enable_spawn_points(&points()).unwrap();
    assert_eq!(lab.spawn_point_count(), 2);
    let first = [12_345.5_f32, -65_432.0, 98_765.0];
    let second = [-1_000.0_f32, 2_000.0, 300_000.0];
    let golden = SpawnVector::Quantized {
        bits: 19,
        components: [
            123_455 + (1 << 20),
            (1 << 20) - 654_320,
            987_650 + (1 << 20),
        ],
    };
    assert_eq!(quantize_spawn_vector(first).unwrap(), golden);
    let mut sockets = Vec::new();
    for (connection, expected) in [(1_u32, first), (2, second), (3, first)] {
        let (socket, address, bunches) =
            join_and_receive(&mut lab, connection, &format!("AuthoredSpawn{connection}"));
        let (controller, handle_19, pawn) = received_locations(&lab, &bunches);
        let packed = quantize_spawn_vector(expected).unwrap();
        assert_eq!(controller, Some(packed), "join {connection}");
        assert_eq!(pawn, controller, "join {connection}");
        assert_eq!(handle_19, expected.map(f32::to_bits), "join {connection}");
        if expected == first {
            assert_eq!(controller, Some(golden));
        }
        let placement = lab.peers[&address].placement.unwrap();
        assert_eq!(placement.location, expected);
        assert_eq!(placement.source, PlacementSource::Configured);
        assert_eq!(placement.yaw, (expected == first).then_some(90.0));
        sockets.push(socket);
    }
    assert_eq!(lab.stats.pawn_spawns_configured, 3);
    assert_eq!(lab.stats.pawn_spawns_default, 0);
    assert_eq!(lab.stats.pawn_spawns_saved, 0);
    let counts = lab.session_counts();
    assert_eq!(counts.pawn_spawns_configured, 3);
    assert_eq!(counts.pawn_spawn_saved_refused, 0);
    let observations = lab.frozen_movement_observations();
    assert_eq!(observations.len(), 3);
    // What `run` and the probe serialize carries none of the run log's
    // forbidden words, with every spawn counter and field populated.
    assert_loggable("session counts", &counts);
    assert_loggable("readiness", &lab.readiness_counts());
    assert_loggable("stats", lab.stats());
    assert_loggable("observations", &observations);
    assert!(
        observations
            .iter()
            .all(|observation| observation.spawn == Some(PlacementSource::Configured))
    );
}

/// Without points both opens carry the plan's location, as before: the
/// origin, counted as default placements.
#[test]
fn without_points_both_opens_keep_the_plans_location() {
    let mut lab = record_lab(true);
    let (_socket, address, bunches) = join_and_receive(&mut lab, 1, "AuthoredDefault");
    let (controller, handle_19, pawn) = received_locations(&lab, &bunches);
    let origin = quantize_spawn_vector([0.0; 3]).unwrap();
    assert_eq!(controller, Some(origin));
    assert_eq!(pawn, Some(origin));
    assert_eq!(handle_19, [0.0_f32; 3].map(f32::to_bits));
    assert_eq!(
        lab.peers[&address].placement,
        Some(Placement {
            location: [0.0; 3],
            yaw: None,
            source: PlacementSource::Default,
        })
    );
    assert_eq!(lab.stats.pawn_spawns_default, 1);
    assert_eq!(lab.stats.pawn_spawns_configured, 0);
    assert_eq!(lab.peers[&address].pawn_moves.placement, None);
}

/// The default path sends exactly the parent commit's bunches (the digests
/// `switch_off_moves_send_the_parent_commits_bunches` pins): a lab never
/// given points, one given an empty configuration, and the production bind
/// with no `[network.spawn]`. With points every session differs.
#[test]
fn default_placement_sends_the_parent_commits_bunches() {
    let pass = |lab: &mut PeerLab, now: Instant| lab.retry_pawn_move_acks(now);
    let mut untouched = moving_lab(false);
    assert_eq!(move_session_digests(&mut untouched, pass), LAB);
    let mut empty = moving_lab(false);
    empty.enable_spawn_points(&SpawnConfig::default()).unwrap();
    assert_eq!(move_session_digests(&mut empty, pass), LAB);
    let mut production = crate::network::bind(&release_toml_config(false))
        .unwrap()
        .unwrap()
        .lab;
    assert_eq!(production.spawn_point_count(), 0);
    assert_eq!(move_session_digests(&mut production, pass), PRODUCTION);
    let mut placed = moving_lab(false);
    placed.enable_spawn_points(&points()).unwrap();
    let digests = move_session_digests(&mut placed, pass);
    assert!(
        digests
            .iter()
            .zip(LAB)
            .all(|(placed, parent)| placed != parent)
    );
    let config = crate::config::NetworkConfig {
        spawn: points(),
        ..release_toml_config(false)
    };
    let mut production_placed = crate::network::bind(&config).unwrap().unwrap().lab;
    assert_eq!(production_placed.spawn_point_count(), 2);
    let digests = move_session_digests(&mut production_placed, pass);
    assert!(
        digests
            .iter()
            .zip(PRODUCTION)
            .all(|(placed, parent)| placed != parent)
    );
}

/// A one-point lab with move acknowledgements on, its peer joined and
/// released; the point is `spawn`.
fn placed_mover(
    spawn: Option<[f64; 3]>,
) -> (
    PeerLab,
    UdpSocket,
    SocketAddrV4,
    super::super::super::sequencing::PacketSequencer,
) {
    let mut lab = moving_lab(true);
    if let Some([x, y, z]) = spawn {
        lab.enable_spawn_points(&SpawnConfig {
            points: vec![SpawnPoint { x, y, z, yaw: None }],
        })
        .unwrap();
    }
    let (socket, address, client) = moving_peer(&mut lab);
    (lab, socket, address, client)
}

/// A configured placement starts the move store: the first client move near
/// it is stored, one far from it is refused as too far (and a later one
/// near it, still stored); without a placement the store starts unset and a
/// first move is bounded by the world check alone, as before.
#[test]
fn a_configured_placement_starts_the_move_store() {
    let spawn = [50_000.0, -20_000.0, 100_000.0];
    let stored = |lab: &PeerLab, address| lab.peers[&address].pawn_moves.location.map(|(at, _)| at);
    // Near the placement: stored.
    let (mut lab, socket, address, mut client) = placed_mover(Some(spawn));
    let start = Instant::now();
    assert_eq!(
        lab.peers[&address].pawn_moves.placement.map(|(at, _)| at),
        Some(spawn)
    );
    assert_eq!(lab.readiness_counts().pawn_move_acks.positions_held, 0);
    let near = [50_000.0, -20_000.0, 99_900.0];
    move_at(
        &mut lab,
        &socket,
        &mut client,
        Some(packed(near.map(|v| v as f32), None, Some(3))),
        start,
    );
    assert_eq!(stored(&lab, address), Some(near));
    assert_eq!(lab.stats.pawn_move_positions_accepted, 1);
    // Far from the placement: refused, the store stays unset; near it later: stored.
    let (mut lab, socket, address, mut client) = placed_mover(Some(spawn));
    let start = Instant::now();
    let far = [900_000.0_f32, 0.0, 0.0];
    move_at(
        &mut lab,
        &socket,
        &mut client,
        Some(packed(far, None, Some(1))),
        start,
    );
    assert_eq!(stored(&lab, address), None);
    assert_eq!(lab.stats.pawn_move_positions_rejected, 1);
    move_at(
        &mut lab,
        &socket,
        &mut client,
        Some(packed(near.map(|v| v as f32), None, Some(1))),
        start + Duration::from_millis(200),
    );
    assert_eq!(stored(&lab, address), Some(near));
    // No placement: the first move is checked against the world bound only.
    let (mut lab, socket, address, mut client) = placed_mover(None);
    let start = Instant::now();
    assert_eq!(lab.peers[&address].pawn_moves.placement, None);
    move_at(
        &mut lab,
        &socket,
        &mut client,
        Some(packed([2.0e6, 0.0, 0.0], None, Some(1))),
        start,
    );
    assert_eq!(stored(&lab, address), None);
    assert_eq!(lab.stats.pawn_move_positions_rejected, 1);
    move_at(
        &mut lab,
        &socket,
        &mut client,
        Some(packed(far, None, Some(1))),
        start + Duration::from_millis(200),
    );
    assert_eq!(stored(&lab, address), Some(far.map(f64::from)));
}

/// The API: configured before admission, needs the controller open for any
/// point, validates (count, bounds), and an empty configuration needs nothing.
#[test]
fn spawn_points_are_validated_and_configured_before_admission() {
    let mut bare = PeerLab::bind(0, PeerLabConfig::default()).unwrap();
    assert!(bare.enable_spawn_points(&points()).is_err());
    bare.enable_spawn_points(&SpawnConfig::default()).unwrap();
    let mut lab = record_lab(true);
    let too_many = SpawnConfig {
        points: vec![points().points[1]; MAX_SPAWN_POINTS + 1],
    };
    assert!(lab.enable_spawn_points(&too_many).is_err());
    let outside = SpawnConfig {
        points: vec![SpawnPoint {
            x: 0.0,
            y: 0.0,
            z: 1_048_577.0,
            yaw: None,
        }],
    };
    assert!(lab.enable_spawn_points(&outside).is_err());
    assert_eq!(lab.spawn_point_count(), 0);
    lab.enable_spawn_points(&points()).unwrap();
    join_and_receive(&mut lab, 1, "AuthoredEarly");
    assert!(lab.enable_spawn_points(&SpawnConfig::default()).is_err());
    assert_eq!(lab.spawn_point_count(), 2);
}

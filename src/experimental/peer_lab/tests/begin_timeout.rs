//! Native Begin limit (`network.begin_timeout_seconds`, default off): a
//! joined session with no recorded Begin past the limit, measured from Join,
//! is sent a control close and removed however much it ACKs; one that sent
//! Begin is never affected. Authored loopback peers; times are explicit
//! instants. No client behaviour, and no stock client's Begin timing, is
//! shown.
use super::super::private_join_budget::{packet, peer as welcomed_peer};
use super::*;
use crate::experimental::packet::{PacketConfig, Receiver, read_packet};

const LIMIT: Duration = Duration::from_secs(5);
const OTHER: u64 = ACCOUNT + 1;
const JUST_PAST: Duration = Duration::from_millis(1);

fn limited(mut lab: PeerLab) -> PeerLab {
    lab.config.begin_timeout = Some(LIMIT);
    lab
}

fn stamp(lab: &mut PeerLab, address: SocketAddrV4, at: Instant) {
    lab.peers.get_mut(&address).unwrap().joined_at = Some(at);
}

/// Whether `socket` received a control-channel close bunch from the server.
/// As for every close the lab sends (F-782), one is sent only to a peer that
/// has delivered a reliable control bunch; the helper-joined peers here never
/// have (their state is set directly), so only a real Join gets one.
fn received_close(lab: &PeerLab, socket: &UdpSocket) -> bool {
    let config = PacketConfig {
        receiver: Receiver::Client,
        ..lab.config.packet.packet
    };
    socket
        .set_read_timeout(Some(Duration::from_millis(200)))
        .unwrap();
    let mut bytes = [0; 2048];
    while let Ok(length) = socket.recv(&mut bytes) {
        if let Ok(packet) = read_packet(&bytes[..length], &config)
            && packet
                .bunches
                .iter()
                .any(|bunch| bunch.header.channel_index == 0 && bunch.header.close)
        {
            return true;
        }
    }
    false
}

#[test]
fn begin_just_inside_the_limit_keeps_the_peer_with_and_without_the_early_pawn() {
    for early in [false, true] {
        let mut lab = limited(if early { early_lab() } else { lab() });
        let mut registry = registry();
        let (socket, address, mut client, _) = peer(&mut lab, 1, ACCOUNT, true);
        let start = Instant::now();
        stamp(&mut lab, address, start);
        assert_eq!(lab.readiness_counts().players_awaiting_begin, 1);
        lab.close_awaiting_begin(start + LIMIT);
        assert!(lab.has_peer(address), "the limit itself is inside");
        identify(&mut lab, &socket, &mut client, address, 3, 3);
        assert_eq!(lab.readiness_counts().players_awaiting_begin, 0);
        for late in [LIMIT + JUST_PAST, LIMIT * 100] {
            lab.close_awaiting_begin(start + late);
            assert!(lab.has_peer(address), "early {early}: Begin was sent");
        }
        lab.bridge_player_checkpoints(&mut registry);
        assert!(lab.has_peer(address));
        assert_eq!(lab.stats.peers_closed_awaiting_begin, 0);
        assert_eq!(lab.session_counts().closed_awaiting_begin, Some(0));
    }
}

#[test]
fn no_begin_is_closed_once_counted_and_frees_its_slot_and_uid_alone() {
    for early in [false, true] {
        let mut lab = limited(if early { early_lab() } else { lab() });
        lab.enable_actor_uids(crate::actor_uids::ActorUidLibrary::new(1))
            .unwrap();
        let mut registry = registry();
        let (socket, silent, _client, _) = peer(&mut lab, 1, ACCOUNT, true);
        let (other_socket, other, mut other_client, _) = peer(&mut lab, 2, OTHER, true);
        let start = Instant::now();
        stamp(&mut lab, silent, start);
        stamp(&mut lab, other, start);
        identify(&mut lab, &other_socket, &mut other_client, other, 3, 3);
        lab.bridge_player_checkpoints(&mut registry);
        let silent_uid = lab.peers[&silent].pawn_uid;
        assert_eq!(silent_uid.is_some(), early, "only the early pawn has one");
        assert_eq!(lab.readiness_counts().players_awaiting_begin, 1);
        assert_eq!(lab.occupancy(None).players, 2);
        lab.close_awaiting_begin(start + LIMIT);
        assert!(lab.has_peer(silent));
        let told = lab.peers[&silent].peer.has_received_client_packet();
        lab.close_awaiting_begin(start + LIMIT + JUST_PAST);
        assert!(!lab.has_peer(silent));
        assert!(lab.has_peer(other), "early {early}: others are unaffected");
        assert_eq!(received_close(&lab, &socket), told, "early {early}");
        assert_eq!(lab.stats.peers_closed_awaiting_begin, 1);
        let counts = lab.session_counts();
        assert_eq!(counts.closed_awaiting_begin, Some(1));
        assert_eq!(
            (
                counts.closed_by_timeout,
                counts.closed_by_delivery_stall,
                counts.closed_by_friends_query_timeout,
                counts.closed_failed,
            ),
            (0, 0, 0, 0)
        );
        // Exactly once: later passes find nothing more to close.
        lab.close_awaiting_begin(start + LIMIT * 100);
        lab.poll();
        assert_eq!(lab.stats.peers_closed_awaiting_begin, 1);
        // The slot and the early pawn's UID are free.
        assert_eq!(lab.occupancy(None).players, 1);
        if let Some(uid) = silent_uid {
            assert!(!lab.claimed_actor_uids().contains(&uid));
        }
        // Nothing of it was ever reserved or stored.
        lab.bridge_player_checkpoints(&mut registry);
        assert_eq!(registry.active_epochs().len(), 1, "the other player's");
        for slot in [-1, 0, 3] {
            if let Ok(key) = PlayerKey::new(ACCOUNT, slot) {
                assert!(registry.snapshot().projection(key).is_none());
            }
        }
        assert_eq!(lab.next_checkpoint_epoch, 2);
        // The next join takes the freed slot (and, with the early pawn, a UID).
        let (_next_socket, next, _next_client, _) = peer(&mut lab, 3, ACCOUNT, true);
        assert!(lab.has_peer(next));
        assert_eq!(lab.occupancy(None).players, 2);
        assert_eq!(lab.peers[&next].pawn_uid.is_some(), early);
        // Every claimed UID belongs to a live session: none leaked.
        let live = lab.peers.values().filter_map(|entry| entry.pawn_uid);
        assert_eq!(lab.claimed_actor_uids().len(), live.count());
    }
}

/// The gated pawn and pending ClientRestart (without the early pawn) or the
/// early pawn and its sent ClientRestart (with it) all go with the session;
/// nothing for it is left pending or reserved.
#[test]
fn pending_startup_and_pawn_state_go_with_the_closed_session() {
    for early in [false, true] {
        let (mut lab, mut registry) = (limited(stock_lab(early)), registry());
        let (socket, address, mut client) = joined_peer(&mut lab, 1, ACCOUNT, true);
        assert!(lab.send_player_controller_open(address));
        for _ in 0..8 {
            lab.poll();
            lab.bridge_player_checkpoints(&mut registry);
            deliver_bunches(&mut lab, &socket, &mut client, |_| true);
        }
        let before = lab.readiness_counts();
        assert_eq!(before.players_awaiting_begin, 1);
        assert_eq!(
            lab.peers[&address]
                .peer
                .has_actor_channel(PAWN_CHANNEL_INDEX),
            early
        );
        if early {
            assert!(lab.peers[&address].controller_startup_pawn_guid.is_some());
        } else {
            assert_eq!(before.controller_startup_actions_pending, 1);
            assert_eq!(before.initial_open_outcomes.pending.pawn, 1);
        }
        let start = lab.peers[&address].joined_at.unwrap();
        let told = lab.peers[&address].peer.has_received_client_packet();
        lab.close_awaiting_begin(start + LIMIT + JUST_PAST);
        assert!(!lab.has_peer(address));
        assert_eq!(received_close(&lab, &socket), told);
        let after = lab.readiness_counts();
        assert_eq!(after.players_awaiting_begin, 0);
        assert_eq!(after.controller_startup_actions_pending, 0);
        assert_eq!(after.initial_open_outcomes.pending.pawn, 0);
        assert_eq!(after.pending_initial_opens, 0);
        lab.bridge_player_checkpoints(&mut registry);
        assert!(registry.active_epochs().is_empty());
        assert!(registry.snapshot().is_empty());
        assert_eq!(lab.next_checkpoint_epoch, 1);
        assert_eq!(lab.stats.connected_player_records_created, 0);
        assert_eq!(lab.session_counts().closed_awaiting_begin, Some(1));
    }
}

/// With authenticated hosting, the close ends the session's Steam
/// authentication as every other removal does.
#[test]
fn an_authenticated_session_closed_awaiting_begin_ends_its_authentication() {
    let mut lab = limited(early_lab());
    let (_socket, address, _client, _) = peer(&mut lab, 7, ACCOUNT, true);
    let (auth, backend) = AuthClient::test_bridge(8);
    lab.authenticator = Some(auth);
    let start = Instant::now();
    stamp(&mut lab, address, start);
    lab.close_awaiting_begin(start + LIMIT + JUST_PAST);
    assert!(!lab.has_peer(address));
    assert_eq!(backend.ended_requests(), [7]);
    assert_eq!(lab.stats.peers_closed_awaiting_begin, 1);
}

/// Absent (the default), the same keepalive session is never closed and the
/// count is absent from the session counts and their serialized form.
#[test]
fn absent_limit_keeps_a_session_without_begin_and_omits_its_count() {
    for early in [false, true] {
        let mut lab = if early { early_lab() } else { lab() };
        assert_eq!(lab.begin_timeout(), None);
        let (_socket, address, _client, _) = peer(&mut lab, 1, ACCOUNT, true);
        let start = Instant::now();
        stamp(&mut lab, address, start);
        lab.close_awaiting_begin(start + Duration::from_secs(3_600) * 10);
        lab.poll();
        assert!(lab.has_peer(address));
        assert_eq!(lab.readiness_counts().players_awaiting_begin, 1);
        assert_eq!(lab.stats.peers_closed_awaiting_begin, 0);
        let counts = lab.session_counts();
        assert_eq!(counts.closed_awaiting_begin, None);
        assert!(
            !serde_json::to_string(&counts)
                .unwrap()
                .contains("closed_awaiting_begin")
        );
        assert!(
            !serde_json::to_string(&lab.stats)
                .unwrap()
                .contains("closed_awaiting_begin")
        );
    }
}

/// Without a controller open plan Begin can never arrive, so the limit closes
/// no one, however long a joined session waits.
#[test]
fn without_a_controller_open_plan_the_limit_closes_no_one() {
    let mut lab = limited(lab());
    let (_socket, address, _client, _) = peer(&mut lab, 1, ACCOUNT, true);
    let start = Instant::now();
    stamp(&mut lab, address, start);
    lab.player_controller_open = None;
    lab.close_awaiting_begin(start + LIMIT * 100);
    assert!(lab.has_peer(address));
    assert_eq!(lab.stats.peers_closed_awaiting_begin, 0);
}

/// A real Join starts the wait; the close reaches a client whose packets the
/// server has received. A zero limit is refused at bind.
#[test]
fn a_join_starts_the_wait_and_a_zero_limit_is_refused() {
    let config = PeerLabConfig {
        admission: Some(PeerLabAdmission::default()),
        begin_timeout: Some(LIMIT),
        ..Default::default()
    };
    let mut lab = PeerLab::bind(0, config).unwrap();
    assert_eq!(lab.begin_timeout(), Some(LIMIT));
    let (socket, address, mut client, mut raw) = welcomed_peer(&mut lab);
    assert_eq!(lab.peers[&address].joined_at, None);
    let before = Instant::now();
    let bytes = packet(&lab, &mut client, &mut raw, &[ControlMessage::Join]);
    receive_authored_at(&mut lab, &socket, &bytes, Instant::now());
    let entry = &lab.peers[&address];
    assert_eq!(entry.admission.state, ClientLoginState::Joined);
    assert!(entry.peer.has_received_client_packet());
    let joined = entry.joined_at.expect("Join stamps the wait");
    assert!(joined >= before);
    // The limit acts only where a controller open, and so Begin, can exist.
    lab.close_awaiting_begin(joined + LIMIT * 100);
    assert!(lab.has_peer(address));
    lab.player_controller_open = super::lab().player_controller_open.clone();
    assert!(lab.player_controller_open.is_some());
    lab.close_awaiting_begin(joined + LIMIT);
    assert!(lab.has_peer(address));
    lab.close_awaiting_begin(joined + LIMIT + JUST_PAST);
    assert!(!lab.has_peer(address));
    assert!(received_close(&lab, &socket));
    assert_eq!(lab.occupancy(None).players, 0);
    assert_eq!(lab.session_counts().closed_awaiting_begin, Some(1));
    let zero = PeerLabConfig {
        begin_timeout: Some(Duration::ZERO),
        ..Default::default()
    };
    assert!(PeerLab::bind(0, zero).is_err());
}

/// Sessions that send Begin are untouched by the limit, byte for byte: the
/// production bind with it set sends exactly the bunches
/// `switch_off_sessions_send_the_parent_commits_bunches` pins for the same
/// bind without it (its `PRODUCTION` digests, copied here), from handshake
/// and Join through Begin to each connected-player record's ACK.
#[test]
fn begin_sending_sessions_send_the_pinned_bunches_with_the_limit_set() {
    const PRODUCTION: [&str; 2] = [
        "cafb88babd75ea37f94944a890596a7d6d1e8566b9a61278fb471031411bde5c",
        "4d6c55a1fba2f27e51d71df5b9201a4116d2a8ae3322fdba1022e24c97e5b205",
    ];
    let mut lab = production_digest_lab(crate::config::NetworkConfig {
        enabled: true,
        port: 0,
        release_profile: Some(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("config/release.toml"),
        ),
        controller_possession_properties: false,
        begin_timeout_seconds: 60,
        ..crate::config::NetworkConfig::default()
    });
    assert_eq!(lab.begin_timeout(), Some(Duration::from_secs(60)));
    assert_eq!(switch_off_session_digests(&mut lab), PRODUCTION);
    assert_eq!(lab.session_counts().closed_awaiting_begin, Some(0));
}

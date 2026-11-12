//! Native per-query friends deadline (N-019): an unanswered friends query
//! (F-1051) removes its peer after the session timeout, measured from the
//! request and never extended by ACKs, so it cannot hold others' finalisation
//! (F-973). Authored loopback peers only; times are explicit instants.
use super::super::super::{
    packet::{Packet, PacketConfig, Receiver, read_packet, write_packet},
    sequencing::PacketSequencer,
};
use super::*;

/// Positively ACK every outstanding server packet of `socket`'s peer in one
/// client packet accepted at `now`.
fn ack_all_at(lab: &mut PeerLab, socket: &UdpSocket, client: &mut PacketSequencer, now: Instant) {
    let SocketAddr::V4(address) = socket.local_addr().unwrap() else {
        unreachable!()
    };
    let count = lab.peers[&address]
        .peer
        .sequencer()
        .notify()
        .sent_records()
        .len();
    let config = PacketConfig {
        receiver: Receiver::Client,
        ..lab.config.packet.packet
    };
    for _ in 0..count {
        let mut bytes = [0; 2048];
        let length = socket.recv(&mut bytes).unwrap();
        let packet = read_packet(&bytes[..length], &config).unwrap();
        let received = client.receive(&packet.header).unwrap().unwrap();
        client.ack_seq(received.sequence, true).unwrap();
    }
    let bytes = write_packet(
        &Packet {
            magic: 0,
            header: client.header().unwrap(),
            info: None,
            bunches: vec![],
        },
        &lab.config.packet.packet,
    )
    .unwrap();
    client.commit().unwrap();
    receive_authored_at(lab, socket, &bytes, now);
}

/// One registered, completed peer whose friends query was just requested and
/// sent; returns the deadline's start.
fn queried_peer() -> (PeerLab, UdpSocket, SocketAddrV4, PacketSequencer, Instant) {
    let (mut lab, socket, address, client) = initialisation_transport_peer();
    let entry = lab.peers.get_mut(&address).unwrap();
    entry.character_slot = Some(3);
    entry.profiles.completed = true;
    assert_eq!(entry.friends_query_since, None);
    lab.query_connected_friends();
    assert_eq!(lab.stats.friends_list_queries_queued, 1);
    let since = lab.peers[&address].friends_query_since.unwrap();
    (lab, socket, address, client, since)
}

#[test]
fn acking_peer_that_never_answers_is_removed_and_a_new_joiner_finalises() {
    let (mut lab, socket, address, mut client, since) = queried_peer();
    let timeout = lab.config.idle_timeout;
    // A second player joins while the first has not answered (N-019).
    let (second_socket, second_address, mut second_client) = acked_game_state_peer(&mut lab);
    let login = lab.peers[&address].login_unique_id.clone();
    let second = lab.peers.get_mut(&second_address).unwrap();
    second.login_unique_id = login;
    second.connection_id = Some(2);
    second.player = Some(RegisteredPlayer {
        player_id: 2,
        name_units: vec![],
    });
    let state = lab.game_state_open.take();
    assert!(lab.send_player_controller_open(second_address));
    lab.game_state_open = state;
    ack_startup_packets(&mut lab, &second_socket, &mut second_client);
    lab.controller_call(second_address, ControllerCall::Begin(4));
    // Begin re-queries the first peer without moving its deadline.
    assert_eq!(lab.stats.friends_list_queries_queued, 3);
    assert_eq!(lab.peers[&address].friends_query_since, Some(since));
    let second = lab.peers.get_mut(&second_address).unwrap();
    second.profiles.completed = true;
    lab.controller_call(second_address, ControllerCall::FriendsList);
    assert_eq!(lab.peers[&second_address].friends_query_since, None);
    // The first peer keeps acknowledging both queries, up to the deadline.
    ack_all_at(&mut lab, &socket, &mut client, since + timeout);
    assert!(lab.peers[&address].friends_query_receipt.settled);
    assert_eq!(lab.peers[&address].last_seen, since + timeout);
    lab.expire_friends_queries(since + timeout);
    lab.finalise_connected_players();
    assert!(lab.has_peer(address));
    assert_eq!(
        lab.stats.players_finalised, 0,
        "the silent query stalls all"
    );
    lab.expire_friends_queries(since + timeout + Duration::from_millis(1));
    assert!(!lab.has_peer(address));
    assert!(lab.has_peer(second_address));
    assert_eq!(lab.stats.peers_friends_query_expired, 1);
    assert_eq!(lab.session_counts().closed_by_friends_query_timeout, 1);
    assert_eq!(lab.stats.peers_expired, 0);
    assert_eq!(lab.stats.peers_delivery_stalled, 0);
    lab.finalise_connected_players();
    assert_eq!(lab.stats.players_finalised, 1);
    assert!(lab.peers[&second_address].profiles.finalised);
    lab.expire_friends_queries(since + timeout * 100);
    assert!(
        lab.has_peer(second_address),
        "answered peers have no deadline"
    );
    assert_eq!(lab.stats.peers_friends_query_expired, 1);
    lab.shutdown();
}

#[test]
fn answer_just_inside_the_deadline_keeps_the_peer_and_finalises() {
    let (mut lab, _socket, address, _client, since) = queried_peer();
    let timeout = lab.config.idle_timeout;
    lab.expire_friends_queries(since + timeout);
    assert!(lab.has_peer(address));
    lab.controller_call(address, ControllerCall::FriendsList);
    assert_eq!(lab.peers[&address].friends_query_since, None);
    lab.expire_friends_queries(since + timeout + Duration::from_millis(1));
    lab.expire_friends_queries(since + timeout * 100);
    assert!(lab.has_peer(address));
    assert_eq!(lab.stats.peers_friends_query_expired, 0);
    lab.finalise_connected_players();
    assert_eq!(lab.stats.players_finalised, 1);
    // A later query after the answer starts a fresh deadline.
    lab.query_connected_friends();
    let fresh = lab.peers[&address].friends_query_since.unwrap();
    assert!(fresh >= since);
    lab.expire_friends_queries(fresh + timeout);
    assert!(lab.has_peer(address));
}

#[test]
fn an_answer_restarts_the_deadline_for_a_requery_still_to_be_sent() {
    let (mut lab, _socket, address, _client, since) = queried_peer();
    let timeout = lab.config.idle_timeout;
    // A second request that transport has not admitted yet.
    let entry = lab.peers.get_mut(&address).unwrap();
    entry.friends_query_pending = true;
    entry.friends_query_since = Some(since - timeout + Duration::from_secs(1));
    let before = Instant::now();
    lab.controller_call(address, ControllerCall::FriendsList);
    let restarted = lab.peers[&address].friends_query_since.unwrap();
    assert!(restarted >= before, "the answer restarts the clock");
    // The first query's start would have expired here; the restart has not.
    lab.expire_friends_queries(since + Duration::from_secs(2));
    assert!(lab.has_peer(address));
    assert_eq!(lab.stats.peers_friends_query_expired, 0);
    // The re-query still has to be answered within its own deadline.
    lab.expire_friends_queries(restarted + timeout + Duration::from_millis(1));
    assert!(!lab.has_peer(address));
    assert_eq!(lab.stats.peers_friends_query_expired, 1);
}

#[test]
fn acknowledgements_and_requeries_do_not_extend_the_deadline() {
    let (mut lab, socket, address, mut client, since) = queried_peer();
    let timeout = lab.config.idle_timeout;
    let late = since + timeout - Duration::from_millis(1);
    ack_all_at(&mut lab, &socket, &mut client, late);
    let entry = &lab.peers[&address];
    assert!(entry.friends_query_receipt.settled);
    assert_eq!(lab.stats.friends_list_queries_acknowledged, 1);
    assert_eq!(entry.last_seen, late);
    assert_eq!(entry.delivery_progress_at, Some(late));
    lab.query_connected_friends();
    assert_eq!(lab.stats.friends_list_queries_queued, 2);
    assert_eq!(lab.peers[&address].friends_query_since, Some(since));
    ack_all_at(&mut lab, &socket, &mut client, since + timeout);
    let expired = since + timeout + Duration::from_millis(1);
    // Both existing 30 s clocks were just reset by this traffic.
    lab.expire_idle(expired);
    lab.expire_stalled_delivery(expired);
    assert!(lab.has_peer(address));
    lab.expire_friends_queries(expired);
    assert!(!lab.has_peer(address));
    assert_eq!(lab.stats.peers_friends_query_expired, 1);
    assert_eq!(lab.stats.peers_expired, 0);
    assert_eq!(lab.stats.peers_delivery_stalled, 0);
}

#[test]
fn silent_peer_still_closes_by_idle_timeout_through_poll() {
    let (mut lab, _socket, address, _client, _since) = queried_peer();
    let timeout = lab.config.idle_timeout;
    let past = Instant::now() - timeout - Duration::from_secs(1);
    let entry = lab.peers.get_mut(&address).unwrap();
    entry.last_seen = past;
    entry.friends_query_since = Some(past);
    lab.poll();
    assert!(!lab.has_peer(address));
    assert_eq!(lab.stats.peers_expired, 1);
    assert_eq!(lab.stats.peers_friends_query_expired, 0);
    let counts = lab.session_counts();
    assert_eq!(counts.closed_by_timeout, 1);
    assert_eq!(counts.closed_by_friends_query_timeout, 0);
}

#[test]
fn poll_removes_an_expired_query_despite_fresh_traffic() {
    let (mut lab, _socket, address, _client, _since) = queried_peer();
    let timeout = lab.config.idle_timeout;
    let entry = lab.peers.get_mut(&address).unwrap();
    entry.friends_query_since = Some(Instant::now() - timeout - Duration::from_secs(1));
    entry.last_seen = Instant::now();
    lab.poll();
    assert!(!lab.has_peer(address));
    assert_eq!(lab.stats.peers_expired, 0);
    assert_eq!(lab.stats.peers_delivery_stalled, 0);
    assert_eq!(lab.stats.peers_friends_query_expired, 1);
    let status = serde_json::to_value(lab.session_counts()).unwrap();
    assert_eq!(status["closed_by_friends_query_timeout"], 1);
    lab.poll();
    assert_eq!(lab.stats.peers_friends_query_expired, 1);
}

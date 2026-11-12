//! Connected-player record release gated on the client's ACKs (F-1116):
//! with possession properties on, a session's own record waits until the
//! client has ACKed its controller channel through the possession update
//! (the controller open included) and its pawn channel through the pawn
//! open; a session whose update is abandoned never gets its record and is
//! closed. Authored loopback peers only; losses are chosen NAKs.
use super::super::super::{
    packet::{Bunch, Packet, PacketConfig, Receiver, read_packet, write_packet},
    pawn_open::PAWN_CHANNEL_INDEX,
    sequencing::PacketSequencer,
    session_open::PLAYER_CONTROLLER_CHANNEL_INDEX,
};
use super::*;

/// Read every outstanding datagram of `address` on the client side, ACKing
/// each in the client's sequencer, without telling the lab yet.
fn read_outstanding(
    lab: &PeerLab,
    socket: &UdpSocket,
    client: &mut PacketSequencer,
    address: SocketAddrV4,
) -> Vec<Bunch> {
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
    let mut bunches = Vec::new();
    for _ in 0..count {
        let mut bytes = [0; 2048];
        let length = socket.recv(&mut bytes).unwrap();
        let packet = read_packet(&bytes[..length], &config).unwrap();
        let received = client.receive(&packet.header).unwrap().unwrap();
        client.ack_seq(received.sequence, true).unwrap();
        bunches.extend(packet.bunches);
    }
    bunches
}

/// The client's empty packet carrying its ACK state, accepted by the lab.
fn send_client_acks(lab: &mut PeerLab, socket: &UdpSocket, client: &mut PacketSequencer) {
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
    receive_authored_at(lab, socket, &bytes, Instant::now());
}

/// Resend every NAKed reliable bunch of `address` in one datagram.
fn send_repair(lab: &mut PeerLab, address: SocketAddrV4) {
    let repair = lab
        .peers
        .get_mut(&address)
        .unwrap()
        .peer
        .ack_packet()
        .unwrap();
    assert!(lab.send(&repair, address));
}

/// Begin, completion and the friends list, then a finalisation pass and a
/// record pass, as the poll runs them.
fn finalise(lab: &mut PeerLab, address: SocketAddrV4) {
    complete_record_player(lab, address);
    lab.controller_call(address, ControllerCall::FriendsList);
    lab.finalise_connected_players();
    lab.retry_connected_player_records();
    assert_eq!(lab.stats.players_finalised, 1);
}

/// (1) The possession update's packet is lost once and repaired: the
/// finalised record is not sent while the repair is unACKed, even across
/// polls, and is sent exactly once after the client ACKs it.
#[test]
fn a_lost_possession_update_holds_the_record_until_its_repair_is_acked() {
    let mut lab = possession_lab(true);
    let (socket, address, mut client) = possession_identity_peer(&mut lab, 1, "AuthoredLost");
    assert!(lab.send_player_controller_open(address));
    let lost = deliver_bunches_by_content(&mut lab, &socket, &mut client, |bunches| {
        !bunches.iter().any(possession_shaped)
    });
    let [(position, _)] = possession_updates(&lab, &lost).try_into().unwrap();
    let original = lost[position].payload.clone();
    assert_eq!(lab.stats.controller_possession_updates_acked, 0);
    finalise(&mut lab, address);
    assert_eq!(lab.stats.connected_player_record_creations_sent, 0);
    // The repair reaches the client; until its ACK arrives nothing moves.
    send_repair(&mut lab, address);
    for _ in 0..3 {
        lab.poll();
    }
    let repaired = read_outstanding(&lab, &socket, &mut client, address);
    let updates = possession_updates(&lab, &repaired);
    assert!(!updates.is_empty());
    for (position, _) in updates {
        assert_eq!(repaired[position].payload.bytes(), original.bytes());
    }
    assert!(record_bodies(&lab, &repaired).is_empty());
    lab.finalise_connected_players();
    lab.retry_connected_player_records();
    assert_eq!(lab.stats.connected_player_record_creations_sent, 0);
    assert!(!lab.peers[&address].record_prerequisites.settled());
    // The client's ACK settles the prerequisites; the next pass sends it.
    send_client_acks(&mut lab, &socket, &mut client);
    assert_eq!(lab.stats.controller_possession_updates_acked, 1);
    assert!(lab.peers[&address].record_prerequisites.settled());
    lab.poll();
    assert_eq!(lab.stats.connected_player_record_creations_sent, 1);
    let sent = deliver_bunches(&mut lab, &socket, &mut client, |_| true);
    assert_eq!(record_bodies(&lab, &sent).len(), 1);
    assert!(possession_updates(&lab, &sent).is_empty());
    for _ in 0..3 {
        lab.poll();
        deliver_bunches(&mut lab, &socket, &mut client, |_| true);
    }
    assert_eq!(
        (
            lab.stats.connected_player_record_creations_sent,
            lab.stats.connected_player_record_creations_acked,
            lab.stats.connected_player_array_updates_sent,
            lab.stats.controller_possession_updates_sent,
        ),
        (1, 1, 1, 1)
    );
}

/// (2) The controller open, or the pawn open, is unACKed (its packet NAKed,
/// everything else ACKed): the record waits, even though the update's own
/// bunch is ACKed in the controller case, and follows the open's repair.
#[test]
fn an_unacked_controller_or_pawn_open_holds_the_record() {
    for channel in [PLAYER_CONTROLLER_CHANNEL_INDEX, PAWN_CHANNEL_INDEX] {
        let mut lab = possession_lab(true);
        let (socket, address, mut client) = possession_identity_peer(&mut lab, 1, "AuthoredOpen");
        assert!(lab.send_player_controller_open(address));
        let opens = deliver_bunches_by_content(&mut lab, &socket, &mut client, |bunches| {
            !bunches
                .iter()
                .any(|bunch| bunch.header.channel_index == channel && bunch.header.open)
        });
        assert!(
            opens
                .iter()
                .any(|bunch| bunch.header.channel_index == channel && bunch.header.open)
        );
        if channel == PLAYER_CONTROLLER_CHANNEL_INDEX {
            assert_eq!(
                lab.stats.controller_possession_updates_acked, 1,
                "the update's own bunch is ACKed"
            );
        }
        finalise(&mut lab, address);
        // Later passes, their packets ACKed except any copy of the open.
        for _ in 0..3 {
            lab.poll();
        }
        deliver_bunches_by_content(&mut lab, &socket, &mut client, |bunches| {
            !bunches
                .iter()
                .any(|bunch| bunch.header.channel_index == channel && bunch.header.open)
        });
        lab.poll();
        assert_eq!(
            lab.stats.connected_player_record_creations_sent, 0,
            "channel {channel}"
        );
        send_repair(&mut lab, address);
        deliver_bunches(&mut lab, &socket, &mut client, |_| true);
        assert!(lab.peers[&address].record_prerequisites.settled());
        lab.poll();
        assert_eq!(
            lab.stats.connected_player_record_creations_sent, 1,
            "channel {channel}"
        );
        let sent = deliver_bunches(&mut lab, &socket, &mut client, |_| true);
        assert_eq!(record_bodies(&lab, &sent).len(), 1);
        assert_eq!(lab.stats.connected_player_record_creations_acked, 1);
    }
}

/// (3) One session's update is abandoned (its controller channel closes
/// while the update is deferred) while a second player waits for the first
/// one's friends answer: the first record is never sent, its session is
/// closed in the same poll, and the second player finalises and gets its
/// own record in that poll.
#[test]
fn an_abandoned_update_closes_its_session_and_others_finalise() {
    let mut lab = possession_lab(true);
    let (first_socket, first, mut first_client) = held_possession_peer(&mut lab, "AuthoredFirst");
    let (second_socket, second, mut second_client) = record_peer(&mut lab, 2, "AuthoredSecond");
    complete_record_player(&mut lab, second);
    lab.controller_call(second, ControllerCall::FriendsList);
    lab.finalise_connected_players();
    lab.retry_connected_player_records();
    assert!(
        lab.peers[&first].friends_query_pending,
        "the first player owes an answer"
    );
    assert_eq!(lab.stats.players_finalised, 1, "the second one waits");
    assert_eq!(lab.stats.connected_player_record_creations_sent, 0);
    drain_controller_window(&mut lab, &first_socket, &mut first_client, first);
    let entry = lab.peers.get_mut(&first).unwrap();
    let channel = entry.player_controller_channel.unwrap();
    let close = entry.peer.reliable_actor_close(channel, 1).unwrap();
    assert!(lab.send(&close, first));
    lab.poll();
    assert!(!lab.has_peer(first));
    assert_eq!(lab.stats.controller_possession_errors, 1);
    assert_eq!(lab.session_counts().closed_by_possession_abandoned, 1);
    assert_eq!(lab.stats.connected_player_records_removed, 1);
    assert_eq!(lab.stats.players_finalised, 2);
    assert_eq!(lab.stats.connected_player_record_creations_sent, 1);
    let sent = deliver_bunches(&mut lab, &second_socket, &mut second_client, |_| true);
    assert_eq!(record_bodies(&lab, &sent).len(), 1);
    assert_eq!(lab.stats.connected_player_record_creations_acked, 1);
    for _ in 0..3 {
        lab.poll();
        deliver_bunches(&mut lab, &second_socket, &mut second_client, |_| true);
    }
    assert_eq!(
        (
            lab.stats.connected_player_record_creations_sent,
            lab.stats.peers_closed_by_possession_abandoned,
            lab.stats.peers_failed,
        ),
        (1, 1, 0)
    );
}

/// (4) Without loss everything is ACKed before finalisation, so the record
/// goes out in the finalising pass itself, after the update, as before;
/// `disabled_controller_possession_sends_exactly_the_previous_bunches`
/// pins the whole sequence (one extra bunch, the update).
#[test]
fn a_lossless_join_sends_the_record_in_the_finalising_pass() {
    let mut lab = possession_lab(true);
    let (socket, address, mut client, mut bunches) =
        possession_peer(&mut lab, 1, "AuthoredLossless");
    assert!(lab.peers[&address].record_prerequisites.settled());
    finalise(&mut lab, address);
    assert_eq!(lab.stats.connected_player_record_creations_sent, 1);
    let update = possession_updates(&lab, &bunches)[0].0;
    let offset = bunches.len();
    bunches.extend(deliver_bunches(&mut lab, &socket, &mut client, |_| true));
    assert_eq!(record_bodies(&lab, &bunches[offset..]).len(), 1);
    assert!(update < offset);
    assert_eq!(lab.stats.connected_player_record_creations_acked, 1);
}

/// (5) Switches. Possession properties off: nothing is gated, so a record
/// is sent at finalisation while the controller open is still unACKed, as
/// before. Records off: an abandoned update closes nothing.
#[test]
fn the_gate_and_the_close_stay_behind_their_switches() {
    let mut lab = record_lab(true);
    let (socket, address, mut client) = possession_identity_peer(&mut lab, 1, "AuthoredOff");
    assert!(lab.send_player_controller_open(address));
    deliver_bunches_by_content(&mut lab, &socket, &mut client, |bunches| {
        !bunches.iter().any(|bunch| {
            bunch.header.channel_index == PLAYER_CONTROLLER_CHANNEL_INDEX && bunch.header.open
        })
    });
    finalise(&mut lab, address);
    assert_eq!(lab.stats.connected_player_record_creations_sent, 1);
    assert!(
        lab.peers[&address]
            .record_prerequisites
            .controller
            .channel
            .is_none()
    );

    let mut lab = possession_lab(false);
    let (socket, address, mut client) = held_possession_peer(&mut lab, "AuthoredNoRecords");
    drain_controller_window(&mut lab, &socket, &mut client, address);
    let entry = lab.peers.get_mut(&address).unwrap();
    let channel = entry.player_controller_channel.unwrap();
    let close = entry.peer.reliable_actor_close(channel, 1).unwrap();
    assert!(lab.send(&close, address));
    lab.poll();
    assert_eq!(lab.stats.controller_possession_errors, 1);
    assert!(lab.has_peer(address));
    assert_eq!(lab.session_counts().closed_by_possession_abandoned, 0);
}

//! The private pawn-channel bunch capture over the lab's real receive path,
//! driven by the project's own client sequencer. Authored bits only: no
//! captured payload, GUID or account value is used.
use std::io::{Read, Seek, SeekFrom};

use super::super::super::{
    packet::{BitWriter, Bunch, BunchHeader, Name, Packet, write_packet},
    pawn_open::PAWN_CHANNEL_INDEX,
    selected_rpc_capture::pawn_bunches::{PawnBunchCaptureCounts, PrivatePawnBunchCapture},
    sequencing::{PacketSequencer, split_outgoing_bunch, wire_reliable_sequence},
    session_open::ACTOR_CHANNEL_NAME_INDEX,
};
use super::*;

fn bits(count: usize, seed: usize) -> Payload {
    let mut writer = BitWriter::new();
    for bit in 0..count {
        writer.write_bit((bit + seed).is_multiple_of(3));
    }
    Payload::from_writer(writer)
}

fn client_bunch(channel: u32, reliable: Option<u32>, payload: Payload) -> Bunch {
    Bunch {
        header: BunchHeader {
            control: false,
            open: false,
            close: false,
            close_reason: None,
            paused: false,
            reliable: reliable.is_some(),
            channel_index: channel,
            has_package_map_exports: false,
            has_must_be_mapped_guids: false,
            partial: false,
            reliable_sequence_raw: reliable,
            partial_initial: false,
            partial_final: false,
            channel_name: reliable.map(|_| Name::Hardcoded(ACTOR_CHANNEL_NAME_INDEX)),
        },
        payload,
    }
}

fn send(lab: &mut PeerLab, socket: &UdpSocket, client: &mut PacketSequencer, bunches: Vec<Bunch>) {
    let bytes = write_packet(
        &Packet {
            magic: 0,
            header: client.header().unwrap(),
            info: None,
            bunches,
        },
        &lab.config.packet.packet,
    )
    .unwrap();
    client.commit().unwrap();
    receive_authored_at(lab, socket, &bytes, Instant::now());
}

/// [`deliver_bunches`] (all delivered) that also keeps, per server packet,
/// the delivery outcomes of the client's own packets that its header carries.
fn deliver(
    lab: &mut PeerLab,
    socket: &UdpSocket,
    client: &mut PacketSequencer,
) -> (Vec<Bunch>, Vec<Vec<bool>>) {
    use super::super::super::packet::{Receiver, read_packet};
    let SocketAddr::V4(address) = socket.local_addr().unwrap() else {
        unreachable!()
    };
    let count = lab.peers[&address]
        .peer
        .sequencer()
        .notify()
        .sent_records()
        .len();
    let config = super::super::super::packet::PacketConfig {
        receiver: Receiver::Client,
        ..lab.config.packet.packet
    };
    let mut bunches = Vec::new();
    let mut outcomes = Vec::new();
    for _ in 0..count {
        let mut bytes = [0; 2048];
        let length = socket.recv(&mut bytes).unwrap();
        let packet = read_packet(&bytes[..length], &config).unwrap();
        let received = client.receive(&packet.header).unwrap().unwrap();
        outcomes.push(
            received
                .notifications
                .iter()
                .map(|notification| notification.delivered)
                .collect(),
        );
        client.ack_seq(received.sequence, true).unwrap();
        // Reliable sequences start from the random handshake cookie's seed;
        // compare them relative to it.
        let seed = client.initial_reliable().0 % 1024;
        bunches.extend(packet.bunches.into_iter().map(|mut bunch| {
            bunch.header.reliable_sequence_raw = bunch
                .header
                .reliable_sequence_raw
                .map(|raw| (raw + 1024 - seed) % 1024);
            bunch
        }));
    }
    if count > 0 {
        send(lab, socket, client, Vec::new());
    }
    (bunches, outcomes)
}

/// What one scenario run leaves behind: every bunch the server sent the
/// client after each step, in order, the delivery outcome the client read
/// from each server packet header, the lab's counters, and the capture.
struct Run {
    sent: Vec<(Vec<Bunch>, Vec<Vec<bool>>)>,
    stats: serde_json::Value,
    sessions: serde_json::Value,
    game_state: u32,
    pawn_inbound: PawnInboundCounts,
    alive: bool,
    counts: Option<PawnBunchCaptureCounts>,
    rows: Vec<serde_json::Value>,
}

/// One joined player through finalisation and the frozen-movement release,
/// then authored client traffic: pawn unreliable and controller unreliable
/// bunches together, a whole reliable pawn bunch, a two-part reliable pawn
/// bunch, a duplicate of the whole one and the unreliable one again.
fn scenario(capture: bool) -> Run {
    let mut lab = record_lab(true);
    lab.enable_frozen_movement_release(Duration::ZERO).unwrap();
    let mut file = tempfile::tempfile().unwrap();
    if capture {
        lab.install_private_pawn_bunch_capture(PrivatePawnBunchCapture::from_file(
            file.try_clone().unwrap(),
        ))
        .unwrap();
    }
    let (socket, address, mut client) = record_peer(&mut lab, 1, "AuthoredPawnCapture");
    assert!(lab.peers[&address].controller_startup_pawn_guid.is_some());
    assert_eq!(lab.pawn_open.as_ref().unwrap().1, PAWN_CHANNEL_INDEX);
    complete_record_player(&mut lab, address);
    lab.controller_call(address, ControllerCall::FriendsList);
    lab.finalise_connected_players();
    let mut sent = vec![deliver(&mut lab, &socket, &mut client)];
    lab.retry_frozen_movement_release();
    sent.push(deliver(&mut lab, &socket, &mut client));
    assert_eq!(lab.stats.frozen_movement_releases_sent, 1);
    let controller = lab.peers[&address].player_controller_channel.unwrap();
    let first = client.initial_reliable().1 + 1;
    let whole = client_bunch(
        PAWN_CHANNEL_INDEX,
        Some(wire_reliable_sequence(first)),
        bits(40, 1),
    );
    let parts = split_outgoing_bunch(
        &client_bunch(
            PAWN_CHANNEL_INDEX,
            Some(wire_reliable_sequence(first + 1)),
            bits(900, 2),
        ),
        &[],
        512,
        Some(16 * 1024),
    )
    .unwrap();
    assert_eq!(parts.len(), 2);
    let steps = vec![
        vec![
            client_bunch(PAWN_CHANNEL_INDEX, None, bits(70, 0)),
            client_bunch(controller, None, bits(24, 0)),
        ],
        vec![whole.clone()],
        vec![parts[0].clone()],
        vec![parts[1].clone()],
        vec![whole],
        vec![client_bunch(PAWN_CHANNEL_INDEX, None, bits(70, 0))],
    ];
    for bunches in steps {
        send(&mut lab, &socket, &mut client, bunches);
        sent.push(deliver(&mut lab, &socket, &mut client));
    }
    let counts = lab.finish_private_pawn_bunch_capture();
    file.seek(SeekFrom::Start(0)).unwrap();
    let mut text = String::new();
    file.read_to_string(&mut text).unwrap();
    assert!(!text.contains("AuthoredPawnCapture"));
    Run {
        sent,
        stats: serde_json::to_value(lab.stats()).unwrap(),
        sessions: serde_json::to_value(lab.session_counts()).unwrap(),
        game_state: game_state_channel(&lab),
        pawn_inbound: lab.peers[&address].pawn_inbound,
        alive: lab.has_peer(address),
        counts,
        rows: text
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect(),
    }
}

/// With the capture, every client bunch on the player's pawn channel is
/// retained (reliable ones once, in channel order, part by part); controller
/// bunches and the duplicate are not. Without it nothing is retained.
#[test]
fn pawn_channel_bunches_are_retained_only_with_the_capture() {
    let off = scenario(false);
    assert!(off.counts.is_none());
    assert!(off.rows.is_empty());
    let on = scenario(true);
    let counts = on.counts.unwrap();
    assert_eq!(counts.seen, 5);
    assert_eq!(counts.observations, 5);
    assert_eq!(counts.kept_first, 5);
    assert_eq!(counts.retained_payload_bits, 70 + 40 + 900 + 70);
    assert_eq!(counts.incomplete, 0);
    assert_eq!(counts.write_errors, 0);
    let phases: Vec<_> = on
        .rows
        .iter()
        .map(|row| {
            (
                row["observation_phase"].as_str().unwrap().to_owned(),
                row["payload_bits"].as_u64().unwrap(),
                row["partial"].as_bool().unwrap(),
            )
        })
        .collect();
    let release = "reliable_queue_release".to_owned();
    let arrival = "unreliable_arrival".to_owned();
    assert_eq!(phases[0], (arrival.clone(), 70, false));
    assert_eq!(phases[1], (release.clone(), 40, false));
    assert_eq!(phases[2].0, release);
    assert!(phases[2].2 && phases[3].2);
    assert_eq!(phases[2].1 + phases[3].1, 900);
    assert_eq!(phases[4], (arrival, 70, false));
    assert!(
        on.rows
            .iter()
            .all(|row| row["channel"] == PAWN_CHANNEL_INDEX)
    );
    assert!(on.rows.iter().all(|row| row["connection_ordinal"] == 1));
    assert_eq!(on.rows[2]["partial_initial"], true);
    assert_eq!(on.rows[3]["partial_final"], true);
    // Reconstructed reliable sequences follow channel order.
    let first = on.rows[1]["reliable_sequence"].as_i64().unwrap();
    assert_eq!(on.rows[2]["reliable_sequence"], first + 1);
    assert_eq!(on.rows[3]["reliable_sequence"], first + 2);
    assert!(on.rows[0]["reliable_sequence"].is_null());
    // Bit-exact payload: the authored unreliable bunch's bytes.
    let expected: String = bits(70, 0)
        .bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    assert_eq!(on.rows[0]["payload_hex"], expected);
}

/// The capture is a tap: with it on and off, the server sends the same
/// bunches (header and payload bits) after every step, treats the client's
/// bunches the same (counters, pawn inbound counts, session) and the client
/// reads the same delivery outcomes. Packet and reliable sequence numbers
/// start from each lab's random handshake cookie and the GameState record
/// carries server-clock values, so datagrams are compared by their bunches
/// (reliable sequences relative to that seed; the record by header and
/// length) and outcomes; the byte-for-byte comparison is
/// `packet_lab::pawn_tap_tests`, from one fixed cookie.
#[test]
fn the_capture_does_not_change_what_the_server_sends_or_does() {
    let off = scenario(false);
    let on = scenario(true);
    assert_eq!(on.counts.unwrap().observations, 5);
    assert_eq!(off.sent.len(), on.sent.len());
    let game_state = off.game_state;
    assert_eq!(game_state, on.game_state);
    for (step, (off, on)) in off.sent.iter().zip(&on.sent).enumerate() {
        assert_eq!(off.1, on.1, "delivery outcomes after step {step}");
        assert_eq!(off.0.len(), on.0.len(), "server bunches after step {step}");
        for (off, on) in off.0.iter().zip(&on.0) {
            assert_eq!(off.header, on.header, "step {step}");
            assert_eq!(off.payload.bit_len(), on.payload.bit_len(), "step {step}");
            // The connected-player record on the GameState channel carries
            // server-clock values, which differ between the two labs.
            if off.header.channel_index != game_state {
                assert_eq!(off.payload, on.payload, "step {step}");
            }
        }
    }
    assert!(off.sent.iter().any(|(bunches, _)| !bunches.is_empty()));
    assert!(
        off.sent
            .iter()
            .flat_map(|(_, outcomes)| outcomes)
            .any(|packet| !packet.is_empty())
    );
    assert_eq!(off.stats, on.stats);
    assert_eq!(off.sessions, on.sessions);
    assert_eq!(
        off.pawn_inbound.reliable_bunches,
        on.pawn_inbound.reliable_bunches
    );
    assert_eq!(
        off.pawn_inbound.unreliable_bunches,
        on.pawn_inbound.unreliable_bunches
    );
    assert_eq!(off.pawn_inbound.unreliable_bunches, 2);
    assert!(off.alive && on.alive);
}

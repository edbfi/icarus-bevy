//! Authored admission lifetime budget; no original or Steam execution.
use super::super::super::{
    control_messages::encode_control_message,
    packet::{BitWriter, Bunch, BunchHeader, Name, Packet, Payload, write_packet},
    sequencing::{PacketSequencer, wire_reliable_sequence},
};
use super::*;

fn lab() -> PeerLab {
    let mut lab = PeerLab::bind(
        0,
        PeerLabConfig {
            admission: Some(PeerLabAdmission::default()),
            ..Default::default()
        },
    )
    .unwrap();
    lab.enable_private_join_budget(1).unwrap();
    lab
}

pub(super) fn peer(lab: &mut PeerLab) -> (UdpSocket, SocketAddrV4, PacketSequencer, u32) {
    let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    socket.connect(lab.local_addr().unwrap()).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(1)))
        .unwrap();
    let (address, client) = handshake_peer_on(lab, &socket);
    lab.peers.get_mut(&address).unwrap().admission.state = ClientLoginState::Welcomed;
    let raw = client.initial_reliable().1 + 1;
    (socket, address, client, raw)
}

pub(super) fn packet(
    lab: &PeerLab,
    client: &mut PacketSequencer,
    raw: &mut u32,
    messages: &[ControlMessage],
) -> Vec<u8> {
    let mut bits = BitWriter::new();
    for message in messages {
        bits.write_bytes(&encode_control_message(message, &lab.config.packet.codec).unwrap());
    }
    let bytes = write_packet(
        &Packet {
            magic: 0,
            header: client.header().unwrap(),
            info: None,
            bunches: vec![Bunch {
                header: BunchHeader {
                    control: true,
                    open: true,
                    close: false,
                    close_reason: None,
                    paused: false,
                    reliable: true,
                    channel_index: 0,
                    has_package_map_exports: false,
                    has_must_be_mapped_guids: false,
                    partial: false,
                    reliable_sequence_raw: Some(wire_reliable_sequence(*raw)),
                    partial_initial: false,
                    partial_final: false,
                    channel_name: Some(Name::Hardcoded(255)),
                },
                payload: Payload::from_writer(bits),
            }],
        },
        &lab.config.packet.packet,
    )
    .unwrap();
    client.commit().unwrap();
    *raw += 1;
    bytes
}

fn split() -> ControlMessage {
    ControlMessage::SplitJoin {
        url: ArchiveText::text("/Authored/Map?Name=Authored").unwrap(),
        unique_net_id: super::super::super::control_messages::UniqueNetId {
            kind: super::super::super::control_messages::NetIdType::Default,
            value: super::super::super::control_messages::NetIdValue::Text(
                ArchiveText::text("").unwrap(),
            ),
        },
    }
}

#[test]
fn private_join_budget_counts_two_datagrams_before_second_mutation() {
    let mut lab = lab();
    let (one, first, mut client_one, mut raw_one) = peer(&mut lab);
    let (two, second, mut client_two, mut raw_two) = peer(&mut lab);
    let first_packet = packet(&lab, &mut client_one, &mut raw_one, &[ControlMessage::Join]);
    let second_packet = packet(&lab, &mut client_two, &mut raw_two, &[ControlMessage::Join]);
    one.send(&first_packet).unwrap();
    two.send(&second_packet).unwrap();
    lab.poll();
    assert_eq!(lab.stats.joins_succeeded, 1);
    assert!(lab.peers[&first].admission.has_player_controller);
    assert!(!lab.peers[&second].admission.has_player_controller);
    assert!(lab.peers[&second].player.is_none());
    assert_eq!(lab.next_player_id, 257);
    assert_eq!(lab.private_join_remaining, Some(0));
    assert!(lab.private_join_failed);
}

#[test]
fn private_join_budget_shared_with_split_inside_one_packet() {
    for split_first in [false, true] {
        let mut lab = lab();
        let (socket, address, mut client, mut raw) = peer(&mut lab);
        let messages = if split_first {
            vec![split(), ControlMessage::Join]
        } else {
            vec![ControlMessage::Join, split()]
        };
        let bytes = packet(&lab, &mut client, &mut raw, &messages);
        receive_authored_at(&mut lab, &socket, &bytes, Instant::now());
        assert_eq!(lab.stats.joins_succeeded, 1);
        assert_eq!(lab.peers[&address].splitscreen_count, u8::from(split_first));
        assert_eq!(
            lab.peers[&address].admission.has_player_controller,
            !split_first
        );
        assert_eq!(lab.next_player_id, 257);
        assert!(lab.private_join_failed);
    }
}

#[test]
fn private_join_duplicate_does_not_consume_again_and_detach_never_refills() {
    let mut lab = lab();
    let (socket, address, mut client, mut raw) = peer(&mut lab);
    let bytes = packet(
        &lab,
        &mut client,
        &mut raw,
        &[ControlMessage::Join, ControlMessage::Join],
    );
    receive_authored_at(&mut lab, &socket, &bytes, Instant::now());
    assert_eq!(lab.stats.joins_succeeded, 1);
    assert_eq!(lab.stats.duplicate_joins_ignored, 1);
    assert!(!lab.private_join_failed);
    assert!(lab.admitting);
    lab.remove_peer(address);
    let (address, mut client) = handshake_peer_on(&mut lab, &socket);
    lab.peers.get_mut(&address).unwrap().admission.state = ClientLoginState::Welcomed;
    let mut raw = client.initial_reliable().1 + 1;
    let bytes = packet(&lab, &mut client, &mut raw, &[ControlMessage::Join]);
    receive_authored_at(&mut lab, &socket, &bytes, Instant::now());
    assert_eq!(lab.stats.joins_succeeded, 1);
    assert!(!lab.peers[&address].admission.has_player_controller);
    assert!(lab.private_join_failed);
}

#[test]
fn private_join_policy_and_authenticated_split_refusals_preserve_allowance() {
    let mut lab = lab();
    let (_socket, address, _client, _raw) = peer(&mut lab);
    lab.config.admission.as_mut().unwrap().can_spawn_controller = false;
    assert!(matches!(
        lab.reply_for(address, &ControlMessage::Join),
        Some(Reply::Failure(_))
    ));
    assert_eq!(lab.private_join_remaining, Some(1));
    let (auth, _backend) = AuthClient::test_bridge(4);
    lab.authenticator = Some(auth);
    assert!(matches!(
        lab.reply_for(address, &split()),
        Some(Reply::Failure(_))
    ));
    assert_eq!(lab.private_join_remaining, Some(1));
    assert_eq!(lab.stats.joins_succeeded, 0);
}

#[test]
fn private_join_budget_is_single_install_before_peers() {
    let mut lab = PeerLab::bind(0, PeerLabConfig::default()).unwrap();
    assert!(lab.enable_private_join_budget(0).is_err());
    assert!(lab.enable_private_join_budget(2).is_err());
    lab.enable_private_join_budget(1).unwrap();
    assert!(lab.enable_private_join_budget(1).is_err());
}

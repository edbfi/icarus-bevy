use super::*;

#[test]
fn prepared_listener_retains_authentication_and_releases_on_drop() {
    let (auth, backend) = AuthClient::test_bridge(8);
    let mut lab = PeerLab::prepare(PeerLabConfig {
        local_network_version: Some(1),
        admission: Some(PeerLabAdmission::default()),
        ..PeerLabConfig::default()
    })
    .unwrap();
    assert!(lab.local_addr().is_err());
    lab.set_authored_map_url("/Authored/Maps/Workshop".into())
        .unwrap();
    lab.attach_listener(
        SocketAddrV4::new(std::net::Ipv4Addr::UNSPECIFIED, 0),
        Some(auth),
    )
    .unwrap();
    assert!(lab.authenticator.is_some());
    assert_eq!(
        lab.authored_map_url.as_deref(),
        Some("/Authored/Maps/Workshop")
    );
    assert!(lab.buffer.len() > super::super::steam_auth::MAX_DATAGRAM_BYTES);
    let address = lab.local_addr().unwrap();
    assert!(UdpSocket::bind(address).is_err());
    drop(backend);
    lab.poll();
    assert!(!lab.session_counts().admitting);
    drop(lab);
    UdpSocket::bind(address).unwrap();
}

#[test]
fn prepared_unauthenticated_listener_stays_loopback_only() {
    let mut lab = PeerLab::prepare(PeerLabConfig::default()).unwrap();
    assert!(
        lab.attach_listener(SocketAddrV4::new(std::net::Ipv4Addr::UNSPECIFIED, 0), None)
            .is_err()
    );
    assert!(lab.local_addr().is_err());
}

fn pending_auth_peer() -> (
    PeerLab,
    UdpSocket,
    SocketAddrV4,
    crate::steam_service::TestBackend,
) {
    use super::super::handshake::initial_request;
    let (auth, backend) = AuthClient::test_bridge(8);
    let mut lab = PeerLab::bind_authenticated(
        SocketAddrV4::new(std::net::Ipv4Addr::LOCALHOST, 0),
        PeerLabConfig {
            local_network_version: Some(4087436304),
            admission: Some(PeerLabAdmission::default()),
            ..PeerLabConfig::default()
        },
        auth,
    )
    .unwrap();
    let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    socket.connect(lab.local_addr().unwrap()).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(1)))
        .unwrap();
    let SocketAddr::V4(address) = socket.local_addr().unwrap() else {
        unreachable!()
    };
    let mut bytes = [0; 2048];
    socket.send(&initial_request()).unwrap();
    lab.poll();
    let count = socket.recv(&mut bytes).unwrap();
    socket.send(&bytes[..count]).unwrap();
    lab.poll();
    socket.recv(&mut bytes).unwrap();
    let peer = lab.peers.get_mut(&address).unwrap();
    // The signed-ticket validator is tested in steam_auth. This helper
    // starts at the asynchronous validation boundary, without credentials.
    peer.auth_request = Some(1);
    peer.auth_identity = Some(0x0110_0001_0000_002a);
    (lab, socket, address, backend)
}

#[test]
fn cm_membership_requires_join_and_acceptance_and_tracks_revocation() {
    use crate::steam_auth::{AuthEvent, AuthOutcome};
    let (mut lab, _socket, address, backend) = pending_auth_peer();
    assert!(lab.authenticated_membership().unwrap().is_empty());
    backend
        .event(AuthEvent {
            request_id: 1,
            outcome: AuthOutcome::Accepted,
        })
        .unwrap();
    // Acceptance without a committed Join remains ineligible.
    assert!(lab.authenticated_membership().unwrap().is_empty());
    let peer = lab.peers.get_mut(&address).unwrap();
    peer.admission.state = ClientLoginState::Joined;
    peer.connection_id = Some(1);
    let membership = lab.authenticated_membership().unwrap();
    assert_eq!(
        membership,
        crate::steam_presence::MasterMembership::from_authenticated([0x0110_0001_0000_002a])
            .unwrap()
    );
    assert!(!format!("{membership:?}").contains("765611"));
    backend
        .event(AuthEvent {
            request_id: 1,
            outcome: AuthOutcome::Rejected,
        })
        .unwrap();
    assert!(lab.authenticated_membership().unwrap().is_empty());
    assert!(!lab.has_peer(address));
}

#[test]
fn cm_membership_excludes_missing_auth_state_and_removed_peers() {
    let (mut lab, _socket, address, backend) = pending_auth_peer();
    let peer = lab.peers.get_mut(&address).unwrap();
    peer.admission.state = ClientLoginState::Joined;
    peer.connection_id = Some(1);
    // Joined alone cannot make a pending ticket eligible.
    assert!(lab.authenticated_membership().unwrap().is_empty());
    lab.peers.get_mut(&address).unwrap().auth_accepted = true;
    assert_eq!(lab.authenticated_membership().unwrap().len(), 1);
    lab.peers.get_mut(&address).unwrap().auth_request = None;
    assert!(lab.authenticated_membership().unwrap().is_empty());
    lab.peers.get_mut(&address).unwrap().auth_request = Some(1);
    lab.peers.get_mut(&address).unwrap().auth_identity = None;
    assert!(lab.authenticated_membership().unwrap().is_empty());
    lab.peers.get_mut(&address).unwrap().auth_identity = Some(0x0110_0001_0000_002a);
    lab.remove_peer(address);
    assert!(lab.authenticated_membership().unwrap().is_empty());
    drop(backend);
}

#[test]
fn cm_membership_backend_loss_overrides_accepted_live_peer() {
    let (mut lab, _socket, address, backend) = pending_auth_peer();
    let peer = lab.peers.get_mut(&address).unwrap();
    peer.admission.state = ClientLoginState::Joined;
    peer.connection_id = Some(1);
    peer.auth_accepted = true;
    assert_eq!(lab.authenticated_membership().unwrap().len(), 1);
    backend.fail();
    assert!(lab.authenticated_membership().unwrap().is_empty());
    assert!(!lab.is_admitting());
}

#[test]
fn cm_membership_duplicate_accounts_are_unique_until_last_connection_leaves() {
    let (mut lab, _first_socket, first, _backend) = pending_auth_peer();
    let (mut donor, _second_socket, second, _donor_backend) = pending_auth_peer();
    let mut entry = donor.peers.remove(&second).unwrap();
    entry.auth_request = Some(2);
    entry.auth_accepted = true;
    entry.admission.state = ClientLoginState::Joined;
    entry.connection_id = Some(2);
    lab.peers.insert(second, entry);
    let entry = lab.peers.get_mut(&first).unwrap();
    entry.auth_accepted = true;
    entry.admission.state = ClientLoginState::Joined;
    entry.connection_id = Some(1);
    assert_eq!(lab.authenticated_membership().unwrap().len(), 1);
    lab.remove_peer(first);
    assert_eq!(lab.authenticated_membership().unwrap().len(), 1);
    lab.remove_peer(second);
    assert!(lab.authenticated_membership().unwrap().is_empty());
}

#[test]
fn authentication_gates_packets_and_revocation_removes_the_session() {
    use super::super::{
        archive::ByteOrder,
        steam_auth::{SteamAuthMessage, decode_datagram},
    };
    use crate::steam_auth::AuthEvent;
    let (mut lab, socket, address, backend) = pending_auth_peer();
    let header = lab.peers[&address].peer.sequencer().header().unwrap();
    let failures = lab.stats.peers_failed;
    // Even malformed game input cannot advance or fail the packet peer
    // before Steam acceptance.
    lab.buffer[0] = 0;
    lab.data(address, 1, Instant::now());
    assert_eq!(lab.stats.peers_failed, failures);
    assert_eq!(
        lab.peers[&address].peer.sequencer().header().unwrap(),
        header
    );
    backend
        .event(AuthEvent {
            request_id: 1,
            outcome: AuthOutcome::Accepted,
        })
        .unwrap();
    lab.poll();
    assert!(lab.peers[&address].auth_accepted);
    let mut bytes = [0; 128];
    let count = socket.recv(&mut bytes).unwrap();
    assert_eq!(
        decode_datagram(&bytes[..count], ByteOrder::Little, 0, 0).unwrap(),
        SteamAuthMessage::Result { accepted: true }
    );
    assert_eq!(
        lab.peers[&address].peer.sequencer().header().unwrap(),
        header
    );
    backend
        .event(AuthEvent {
            request_id: 1,
            outcome: AuthOutcome::Revoked,
        })
        .unwrap();
    lab.poll();
    assert!(!lab.has_peer(address));
    assert_eq!(backend.ended_requests(), [1]);
    assert_eq!(lab.session_counts().closed_by_authentication, 1);
    backend
        .event(AuthEvent {
            request_id: 1,
            outcome: AuthOutcome::Accepted,
        })
        .unwrap();
    lab.poll();
    assert!(!lab.has_peer(address));
}

#[test]
fn authentication_has_its_own_datagram_bound_without_enlarging_game_packets() {
    use super::super::{
        archive::ByteOrder,
        steam_auth::{MAX_DATAGRAM_BYTES, SteamAuthMessage, decode_datagram, encode_datagram},
    };
    let (mut lab, socket, address, _backend) = pending_auth_peer();
    lab.peers.get_mut(&address).unwrap().auth_request = None;
    // Both input families retain truncation detection and independent caps.
    socket
        .send(&vec![0; lab.config.max_datagram_bytes() + 1])
        .unwrap();
    let mut oversized_auth = vec![0; MAX_DATAGRAM_BYTES + 2];
    oversized_auth[0] = 2;
    socket.send(&oversized_auth).unwrap();
    lab.poll();
    assert_eq!(lab.stats.oversize, 2);
    assert!(lab.has_peer(address));
    let auth = encode_datagram(
        &SteamAuthMessage::UserData {
            ticket: vec![u16::from(b'0'); 1024],
            steam_id: 0x0110_0001_0000_002a,
        },
        ByteOrder::Little,
        0,
        0,
    )
    .unwrap();
    assert!(auth.len() > lab.config.max_datagram_bytes());
    socket.send(&auth).unwrap();
    lab.poll();
    // The whole auth frame reaches verification. Its authored bogus ticket
    // must be rejected explicitly, rather than silently discarded as large.
    assert_eq!(lab.stats.oversize, 2);
    assert!(!lab.has_peer(address));
    assert_eq!(lab.session_counts().closed_by_authentication, 1);
    let mut reply = [0; 128];
    let count = socket.recv(&mut reply).unwrap();
    assert_eq!(
        decode_datagram(&reply[..count], ByteOrder::Little, 0, 0).unwrap(),
        SteamAuthMessage::Result { accepted: false }
    );
}

#[test]
fn authentication_resends_wait_for_acceptance_without_starting_another_request() {
    use super::super::{
        archive::ByteOrder,
        steam_auth::{SteamAuthMessage, encode_datagram},
    };
    for accepted in [false, true] {
        let (mut lab, socket, address, backend) = pending_auth_peer();
        socket.set_nonblocking(true).unwrap();
        let ticket = encode_datagram(
            &SteamAuthMessage::UserData {
                ticket: vec![u16::from(b'0'); 2],
                steam_id: 0x0110_0001_0000_002a,
            },
            ByteOrder::Little,
            0,
            0,
        )
        .unwrap();
        let peer = lab.peers.get_mut(&address).unwrap();
        peer.auth_ticket_digest = Some(Sha256::digest(&ticket).into());
        peer.auth_accepted = accepted;
        let resend = encode_datagram(
            &SteamAuthMessage::RequestResendResult,
            ByteOrder::Little,
            0,
            0,
        )
        .unwrap();
        let expected = encode_datagram(
            &SteamAuthMessage::Result { accepted: true },
            ByteOrder::Little,
            0,
            0,
        )
        .unwrap();
        for datagram in [&resend, &ticket] {
            socket.send(datagram).unwrap();
            lab.poll();
            let mut reply = [0; 128];
            if accepted {
                let count = socket.recv(&mut reply).unwrap();
                assert_eq!(&reply[..count], expected);
            } else {
                assert_eq!(
                    socket.recv(&mut reply).unwrap_err().kind(),
                    io::ErrorKind::WouldBlock
                );
            }
            assert!(lab.has_peer(address));
            assert_eq!(lab.peers[&address].auth_request, Some(1));
            assert_eq!(lab.session_counts().closed_by_authentication, 0);
            // This also rejects an unexpected Begin command.
            assert!(backend.ended_requests().is_empty());
        }
    }
}

#[test]
fn a_second_authentication_ticket_closes_once_and_ignores_late_acceptance() {
    use super::super::{
        archive::ByteOrder,
        steam_auth::{SteamAuthMessage, encode_datagram},
    };
    for accepted in [false, true] {
        let (mut lab, socket, address, backend) = pending_auth_peer();
        let old = encode_datagram(
            &SteamAuthMessage::UserData {
                ticket: vec![u16::from(b'0'); 2],
                steam_id: 0x0110_0001_0000_002a,
            },
            ByteOrder::Little,
            0,
            0,
        )
        .unwrap();
        let replacement = encode_datagram(
            &SteamAuthMessage::UserData {
                ticket: vec![u16::from(b'1'); 2],
                steam_id: 0x0110_0001_0000_002b,
            },
            ByteOrder::Little,
            0,
            0,
        )
        .unwrap();
        let peer = lab.peers.get_mut(&address).unwrap();
        peer.auth_ticket_digest = Some(Sha256::digest(&old).into());
        peer.auth_accepted = accepted;
        socket.send(&replacement).unwrap();
        lab.poll();
        let mut reply = [0; 128];
        let count = socket.recv(&mut reply).unwrap();
        assert_eq!(
            &reply[..count],
            encode_datagram(
                &SteamAuthMessage::Result { accepted: false },
                ByteOrder::Little,
                0,
                0,
            )
            .unwrap()
        );
        assert!(!lab.has_peer(address));
        assert_eq!(lab.session_counts().closed_by_authentication, 1);
        assert_eq!(backend.ended_requests(), [1]);
        backend
            .event(crate::steam_auth::AuthEvent {
                request_id: 1,
                outcome: AuthOutcome::Accepted,
            })
            .unwrap();
        lab.poll();
        assert!(!lab.has_peer(address));
        assert_eq!(lab.session_counts().closed_by_authentication, 1);
        assert!(backend.ended_requests().is_empty());
    }
}

#[test]
fn negative_authentication_outcomes_are_counted_once() {
    for outcome in [
        AuthOutcome::Rejected,
        AuthOutcome::Revoked,
        AuthOutcome::TimedOut,
        AuthOutcome::Expired,
        AuthOutcome::Disconnected,
    ] {
        let (mut lab, _socket, address, backend) = pending_auth_peer();
        let event = crate::steam_auth::AuthEvent {
            request_id: 1,
            outcome,
        };
        backend.event(event).unwrap();
        lab.poll();
        assert!(!lab.has_peer(address));
        assert_eq!(lab.session_counts().closed_by_authentication, 1);
        assert_eq!(backend.ended_requests(), [1]);
        backend.event(event).unwrap();
        lab.poll();
        assert_eq!(lab.session_counts().closed_by_authentication, 1);
        assert!(backend.ended_requests().is_empty());
    }
}

#[test]
fn service_failure_and_drop_release_pending_sessions() {
    let (mut lab, _socket, address, backend) = pending_auth_peer();
    backend.fail();
    lab.poll();
    assert!(!lab.has_peer(address));
    assert!(!lab.is_admitting());
    let (lab, _socket, _address, backend) = pending_auth_peer();
    drop(lab);
    assert_eq!(backend.ended_requests(), [1]);
}

#[test]
fn stale_acceptance_cannot_authenticate_a_replacement_request() {
    let (mut lab, _socket, address, backend) = pending_auth_peer();
    lab.peers.get_mut(&address).unwrap().auth_request = Some(2);
    backend
        .event(crate::steam_auth::AuthEvent {
            request_id: 1,
            outcome: AuthOutcome::Accepted,
        })
        .unwrap();
    lab.poll();
    assert!(!lab.peers[&address].auth_accepted);
}

#[test]
fn login_identity_must_match_the_accepted_ticket() {
    use super::super::control_messages::{NetIdType, NetIdValue, UniqueNetId};
    let (mut lab, _socket, address, _backend) = pending_auth_peer();
    lab.peers.get_mut(&address).unwrap().auth_accepted = true;
    let identity = lab.peers[&address].auth_identity.unwrap();
    let login = |kind, id: u64| ControlMessage::Login {
        response: ArchiveText::text("").unwrap(),
        url: ArchiveText::text("/Game/Maps/Entry?Name=Authored").unwrap(),
        unique_net_id: UniqueNetId {
            kind,
            value: NetIdValue::Text(ArchiveText::text(&id.to_string()).unwrap()),
        },
        platform: ArchiveText::text("Steam").unwrap(),
    };
    for message in [
        login(NetIdType::Indexed(3), identity + 1),
        login(NetIdType::Default, identity),
    ] {
        assert!(matches!(
            lab.reply_for(address, &message),
            Some(Reply::Failure("Steam authentication identity mismatch"))
        ));
    }
    // Matching identity crosses the auth guard; any subsequent admission
    // decision belongs to the separately tested game admission policy.
    let before = lab.stats.logins_received;
    let _ = lab.reply_for(address, &login(NetIdType::Indexed(3), identity));
    assert_eq!(lab.stats.logins_received, before + 1);
}

#[test]
fn incarnation_registration_validates_whole_map_and_retains_high_water_marks() {
    let mut lab = PeerLab::bind(0, PeerLabConfig::default()).unwrap();
    let desired = BTreeMap::from([(10, 3), (20, 2)]);
    lab.reconcile_actor_incarnations(&desired).unwrap();
    for invalid in [
        BTreeMap::from([(10, 4), (20, 0)]),
        BTreeMap::from([(10, 4), (20, 1)]),
    ] {
        assert!(lab.reconcile_actor_incarnations(&invalid).is_err());
        assert_eq!(lab.desired_incarnations, desired);
        assert_eq!(lab.highest_incarnations, desired);
    }
    lab.reconcile_actor_incarnations(&BTreeMap::new()).unwrap();
    assert!(
        lab.reconcile_actor_incarnations(&BTreeMap::from([(10, 2)]))
            .is_err()
    );
    lab.reconcile_actor_incarnations(&BTreeMap::from([(10, 4)]))
        .unwrap();
    assert_eq!(lab.highest_incarnations.get(&20), Some(&2));
}

#[test]
fn incarnation_slots_are_cumulatively_bounded_and_unused_guids_are_pruned() {
    let mut lab = PeerLab::bind(0, PeerLabConfig::default()).unwrap();
    let desired: BTreeMap<_, _> = (0..1024).map(|slot| (slot, 1)).collect();
    lab.reconcile_actor_incarnations(&desired).unwrap();
    let first = lab.guid_cache.incarnation_guid(0, 1).unwrap();
    lab.reconcile_actor_incarnations(&desired).unwrap();
    assert_eq!(lab.guid_cache.incarnation_count(), 1);
    lab.reconcile_actor_incarnations(&BTreeMap::new()).unwrap();
    assert_eq!(lab.guid_cache.incarnation_count(), 0);
    assert!(
        lab.reconcile_actor_incarnations(&BTreeMap::from([(1024, 1)]))
            .is_err()
    );
    assert!(lab.desired_incarnations.is_empty());
    assert_eq!(lab.highest_incarnations.len(), 1024);
    lab.reconcile_actor_incarnations(&BTreeMap::from([(0, 2)]))
        .unwrap();
    let replacement = lab.guid_cache.incarnation_guid(0, 2).unwrap();
    assert_ne!(first, replacement);
    assert_eq!(lab.retiring_incarnation_count(), 0);
}

#[test]
fn controller_configuration_rejects_reserved_channel_before_guid_registration() {
    let mut lab = prospect_refresh_lab();
    // Retain an independently reserved GameState channel while removing
    // only the controller fixture, so the configure-once guard cannot
    // mask channel validation.
    let original = lab.player_controller_open.take().unwrap();
    let reserved = lab.game_state_open.as_ref().unwrap().channel_index;
    let mut colliding = original.clone();
    colliding.channel_index = reserved;
    colliding.profile.level[0].path.push_str("Collision");
    let before = format!("{:?}", lab.guid_cache);
    let game_state = lab.game_state_open.as_ref().unwrap().plan.clone();
    assert!(lab.enable_player_controller_open(colliding).is_err());
    assert!(lab.player_controller_open.is_none());
    assert_eq!(format!("{:?}", lab.guid_cache), before);
    assert_eq!(
        format!("{:?}", lab.game_state_open.as_ref().unwrap().plan),
        format!("{:?}", game_state)
    );
    assert_eq!(
        lab.game_state_open.as_ref().unwrap().channel_index,
        reserved
    );
    lab.enable_player_controller_open(original.clone()).unwrap();
    assert_eq!(
        format!("{:?}", lab.player_controller_open.as_ref().unwrap()),
        format!("{:?}", original)
    );
    assert_eq!(format!("{:?}", lab.guid_cache), before);
}

#[test]
fn controller_configuration_rejects_replacement_without_guid_mutation() {
    let mut lab = prospect_refresh_lab();
    let mut replacement = lab.player_controller_open.clone().unwrap();
    replacement.profile.level[0].path.push_str("Replacement");
    let before = format!("{:?}", lab.guid_cache);
    let original = lab.player_controller_open.clone().unwrap();
    assert!(lab.enable_player_controller_open(replacement).is_err());
    assert_eq!(format!("{:?}", lab.guid_cache), before);
    assert_eq!(
        format!("{:?}", lab.player_controller_open.as_ref().unwrap()),
        format!("{:?}", original)
    );
}

#[test]
fn manager_configuration_rejects_duplicates_and_replacement_atomically() {
    use super::super::release::ReleaseProfile;
    let mut lab = prospect_refresh_lab();
    let plans = ReleaseProfile::fixture("3.0.32")
        .manager_open_plans(PeerLabAdmission::default().map_url)
        .unwrap();
    let before = lab.game_state_open.as_ref().unwrap().state.managers.clone();
    let quest = QuestOpenPlan {
        key: plans[0].0.key(),
        archetype: Vec::new(),
        level: Vec::new(),
        location: None,
        layout: Vec::new(),
        actor_state_layout: Vec::new(),
        stat_container_checksum: 0,
        actor_state_checksum: 0,
        values: BTreeMap::new(),
        references: BTreeMap::new(),
        engine_net_version: 0,
    };
    lab.quest_opens.push((quest, 50));
    assert!(lab.enable_manager_opens(plans.clone(), 20).is_err());
    assert!(lab.manager_opens.is_empty());
    assert_eq!(lab.quest_opens.len(), 1);
    assert_eq!(lab.game_state_open.as_ref().unwrap().state.managers, before);
    lab.quest_opens.clear();
    assert!(
        lab.enable_manager_opens(vec![plans[0].clone(), plans[0].clone()], 20)
            .is_err()
    );
    assert!(lab.manager_opens.is_empty());
    assert_eq!(lab.game_state_open.as_ref().unwrap().state.managers, before);
    lab.enable_manager_opens(plans.clone(), 20).unwrap();
    assert_eq!(lab.manager_opens.len(), plans.len());
    let configured = lab.game_state_open.as_ref().unwrap().state.managers.clone();
    assert!(lab.enable_manager_opens(Vec::new(), 40).is_err());
    assert_eq!(lab.manager_opens.len(), plans.len());
    assert_eq!(
        lab.game_state_open.as_ref().unwrap().state.managers,
        configured
    );
}

fn prospect_refresh_lab() -> PeerLab {
    let mut lab = unbound_prospect_refresh_lab();
    lab.attach_listener(SocketAddrV4::new(std::net::Ipv4Addr::LOCALHOST, 0), None)
        .unwrap();
    lab
}

fn unbound_prospect_refresh_lab() -> PeerLab {
    use super::super::{game_state_open::GAME_STATE_CHANNEL_INDEX, release::ReleaseProfile};
    let mut lab = PeerLab::prepare(PeerLabConfig {
        admission: Some(PeerLabAdmission::default()),
        ..Default::default()
    })
    .unwrap();
    let profile = ReleaseProfile::fixture("3.0.32");
    let map = PeerLabAdmission::default().map_url;
    lab.enable_player_controller_open(
        profile
            .player_controller_open_plan(map, [0.0; 3])
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    lab.enable_game_state_open(
        profile.game_state_open_plan(map).unwrap().unwrap(),
        GameStateOpenState {
            key: ActorKey::Session(0),
            managers: Default::default(),
            values: BTreeMap::from([(
                super::super::game_state_open::handle::SEED,
                RepValue::Int(7),
            )]),
            weather: Vec::new(),
        },
        GAME_STATE_CHANNEL_INDEX,
    )
    .unwrap();
    lab
}

#[test]
fn template_extension_rejects_prospect_handles_atomically_after_refresh() {
    use super::super::{game_state_open::handle, prospect_info::ProspectInfo};
    let mut lab = prospect_refresh_lab();
    // Bootstrap callers may still supply prospect handles before the
    // first typed refresh establishes the authoritative current values.
    lab.extend_game_state_values(BTreeMap::from([
        (handle::SEED, RepValue::Int(8)),
        (65, RepValue::Int(3)),
    ]))
    .unwrap();
    assert_eq!(
        lab.game_state_open.as_ref().unwrap().state.values.get(&65),
        Some(&RepValue::Int(3))
    );
    lab.refresh_active_prospect(&ProspectInfo {
        elapsed_time: 9,
        ..Default::default()
    })
    .unwrap();
    let before = lab.game_state_open.as_ref().unwrap().clone();
    for prospect_handle in handle::ACTIVE_PROSPECT {
        assert!(
            lab.extend_game_state_values(BTreeMap::from([
                (handle::SEED, RepValue::Int(99)),
                (prospect_handle, class_default(prospect_handle).unwrap()),
            ]))
            .is_err()
        );
        let after = lab.game_state_open.as_ref().unwrap();
        assert_eq!(after.state.values, before.state.values);
        assert_eq!(after.active_prospect, before.active_prospect);
    }
    lab.extend_game_state_values(BTreeMap::from([(handle::SEED, RepValue::Int(10))]))
        .unwrap();
    let after = lab.game_state_open.as_ref().unwrap();
    let mut expected = before.state.values.clone();
    expected.insert(handle::SEED, RepValue::Int(10));
    assert_eq!(after.state.values, expected);
    assert_eq!(after.active_prospect, before.active_prospect);
}

/// Establish a lab peer and ACK its GameState open without app clocks
/// or Login/Join behavior. All input is independently authored.
fn acked_game_state_peer(
    lab: &mut PeerLab,
) -> (
    UdpSocket,
    SocketAddrV4,
    super::super::sequencing::PacketSequencer,
) {
    use super::super::{
        handshake::initial_request,
        packet::{Packet, Receiver, read_packet, write_packet},
        packet_lab::client_cookie_seeds,
        sequencing::PacketSequencer,
    };
    let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    socket.connect(lab.local_addr().unwrap()).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(1)))
        .unwrap();
    let SocketAddr::V4(address) = socket.local_addr().unwrap() else {
        unreachable!("IPv4 loopback")
    };
    let mut buffer = [0; 2048];
    socket.send(&initial_request()).unwrap();
    lab.poll();
    let length = socket.recv(&mut buffer).unwrap();
    let cookie = decode(&buffer[..length]).unwrap().cookie;
    socket.send(&buffer[..length]).unwrap();
    lab.poll();
    let length = socket.recv(&mut buffer).unwrap();
    assert_eq!(decode(&buffer[..length]).unwrap().cookie, cookie);
    let (incoming, outgoing) = client_cookie_seeds(&cookie);
    let mut client = PacketSequencer::new(incoming, outgoing, 64).unwrap();
    lab.send_game_state_open(address);
    let entry = &lab.peers[&address];
    assert!(entry.game_state.is_some());
    let count = entry.peer.sequencer().notify().sent_records().len();
    assert!(count > 0);
    let to_client = super::super::packet::PacketConfig {
        receiver: Receiver::Client,
        ..lab.config.packet.packet
    };
    for _ in 0..count {
        let length = socket.recv(&mut buffer).unwrap();
        let packet = read_packet(&buffer[..length], &to_client).unwrap();
        let received = client.receive(&packet.header).unwrap().unwrap();
        client.ack_seq(received.sequence, true).unwrap();
    }
    let ack = write_packet(
        &Packet {
            magic: 0,
            header: client.header().unwrap(),
            info: None,
            bunches: Vec::new(),
        },
        &lab.config.packet.packet,
    )
    .unwrap();
    client.commit().unwrap();
    let channel = lab.game_state_open.as_ref().unwrap().channel_index;
    let entry = lab.peers.get_mut(&address).unwrap();
    entry.peer.receive(&ack).unwrap();
    assert!(entry.peer.actor_channel_settled(channel));
    assert!(entry.peer.sequencer().notify().sent_records().is_empty());
    (socket, address, client)
}

/// Authored wire inputs with a caller-controlled receive clock. The
/// socket and normal data handler are real; only elapsed time is injected.
fn receive_authored_at(lab: &mut PeerLab, socket: &UdpSocket, bytes: &[u8], now: Instant) {
    socket.send(bytes).unwrap();
    let (length, source) = lab.socket.recv_from(&mut lab.buffer).unwrap();
    let SocketAddr::V4(address) = source else {
        unreachable!()
    };
    lab.data(address, length, now);
}

#[test]
fn poll_removes_stalled_delivery_and_reports_session_counter() {
    let mut lab = prospect_refresh_lab();
    let (_socket, address, _) = acked_game_state_peer(&mut lab);
    let timeout = lab.config.idle_timeout;
    let entry = lab.peers.get_mut(&address).unwrap();
    entry.peer.ack_packet().unwrap();
    entry.delivery_progress_at = Some(Instant::now() - timeout - Duration::from_secs(1));
    entry.last_seen = Instant::now();

    lab.poll();

    assert!(!lab.has_peer(address));
    assert_eq!(lab.stats.peers_expired, 0);
    assert_eq!(lab.stats.peers_delivery_stalled, 1);
    let status = serde_json::to_value(lab.session_counts()).unwrap();
    assert_eq!(status["closed_by_delivery_stall"], 1);
    lab.poll();
    assert_eq!(lab.stats.peers_delivery_stalled, 1);
}

#[test]
fn dropped_challenge_expires_delivery_despite_live_reliable_retransmissions() {
    use super::super::{
        control_messages::{ControlCodec, encode_control_message},
        handshake::initial_request,
        packet::{BitWriter, Bunch, BunchHeader, Name, Packet, write_packet},
        packet_lab::client_cookie_seeds,
        sequencing::{PacketSequencer, wire_reliable_sequence},
    };

    let timeout = Duration::from_secs(60);
    let mut config = PeerLabConfig {
        admission: Some(PeerLabAdmission::default()),
        idle_timeout: timeout,
        ..Default::default()
    };
    config.packet.sent_capacity = 1;
    let mut lab = PeerLab::bind(0, config).unwrap();
    let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    socket.connect(lab.local_addr().unwrap()).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(1)))
        .unwrap();
    socket.send(&initial_request()).unwrap();
    lab.poll();
    let mut bytes = [0; 2048];
    let length = socket.recv(&mut bytes).unwrap();
    let cookie = decode(&bytes[..length]).unwrap().cookie;
    socket.send(&bytes[..length]).unwrap();
    lab.poll();
    socket.recv(&mut bytes).unwrap();
    let SocketAddr::V4(address) = socket.local_addr().unwrap() else {
        unreachable!()
    };
    let (incoming, outgoing) = client_cookie_seeds(&cookie);
    let mut client = PacketSequencer::new(incoming, outgoing, 8).unwrap();
    let mut control = client.new_channel();
    let hello = ControlMessage::Hello {
        first_field: 1,
        version: 1,
        token: ArchiveText::text("").unwrap(),
    };
    let sequence = control
        .send_reliable(client.out_packet_id(), hello.clone())
        .unwrap();
    let mut payload = BitWriter::new();
    payload.write_bytes(&encode_control_message(&hello, &ControlCodec::default()).unwrap());
    let hello_bunch = Bunch {
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
            reliable_sequence_raw: Some(wire_reliable_sequence(sequence)),
            partial_initial: false,
            partial_final: false,
            channel_name: Some(Name::Hardcoded(255)),
        },
        payload: Payload::from_writer(payload),
    };
    let start = Instant::now();
    for elapsed in [Duration::ZERO, timeout, timeout + Duration::from_millis(1)] {
        let packet = write_packet(
            &Packet {
                magic: 0,
                header: client.header().unwrap(),
                info: None,
                bunches: vec![hello_bunch.clone()],
            },
            &lab.config.packet.packet,
        )
        .unwrap();
        client.commit().unwrap();
        let now = start + elapsed;
        receive_authored_at(&mut lab, &socket, &packet, now);
        lab.send_acks();
        if elapsed.is_zero() {
            // The Challenge is deliberately lost to the client sequencing
            // model. No later server sequence can fit this one-record ring.
            socket.recv(&mut bytes).unwrap();
        }
        assert_eq!(lab.peers[&address].last_seen, now);
        lab.expire_idle(now);
        assert!(
            lab.has_peer(address),
            "fresh client traffic prevents idle expiry"
        );
        lab.expire_stalled_delivery(now);
        assert_eq!(lab.has_peer(address), elapsed <= timeout);
    }
    assert_eq!(lab.stats.control_challenges_sent, 1);
    assert_eq!(lab.stats.data_accepted, 3);
    assert_eq!(lab.stats.peers_expired, 0);
    assert_eq!(lab.stats.peers_failed, 0);
    assert_eq!(lab.stats.peers_delivery_stalled, 1);
    assert_eq!(lab.session_counts().closed_by_delivery_stall, 1);
    lab.expire_stalled_delivery(start + timeout * 2);
    assert_eq!(lab.stats.peers_delivery_stalled, 1);
}

#[test]
fn delivery_notifications_reset_deadline_and_empty_queues_clear_it() {
    use super::super::packet::{Packet, Receiver, read_packet, write_packet};

    for delivered in [true, false] {
        let mut lab = prospect_refresh_lab();
        let (socket, address, mut client) = acked_game_state_peer(&mut lab);
        let timeout = lab.config.idle_timeout;
        let start = Instant::now();
        // A real reliable control packet and a newer ACK packet allow the
        // client to acknowledge the first, or skip it and report its NAK.
        let first = lab
            .peers
            .get_mut(&address)
            .unwrap()
            .peer
            .reliable_control_packet(ControlMessage::Challenge {
                token: ArchiveText::text("authored").unwrap(),
            })
            .unwrap();
        let newer = lab
            .peers
            .get_mut(&address)
            .unwrap()
            .peer
            .ack_packet()
            .unwrap();
        lab.expire_stalled_delivery(start);
        let mut to_client = lab.config.packet.packet;
        to_client.receiver = Receiver::Client;
        if delivered {
            let packet = read_packet(&first, &to_client).unwrap();
            let received = client.receive(&packet.header).unwrap().unwrap();
            client.ack_seq(received.sequence, true).unwrap();
        }
        let packet = read_packet(&newer, &to_client).unwrap();
        let received = client.receive(&packet.header).unwrap().unwrap();
        client.ack_seq(received.sequence, true).unwrap();
        let ack = write_packet(
            &Packet {
                magic: 0,
                header: client.header().unwrap(),
                info: None,
                bunches: Vec::new(),
            },
            &lab.config.packet.packet,
        )
        .unwrap();
        client.commit().unwrap();
        let progress = start + timeout;
        receive_authored_at(&mut lab, &socket, &ack, progress);
        assert_eq!(lab.peers[&address].delivery_progress_at, Some(progress));
        assert!(
            lab.peers[&address]
                .peer
                .sequencer()
                .notify()
                .sent_records()
                .is_empty()
        );
        assert_eq!(
            lab.peers[&address].peer.pending_resends(),
            usize::from(!delivered)
        );

        // Send a fresh tracked ACK/repair, so the new progress deadline is
        // exercised rather than merely observing an empty ring.
        lab.send_acks();
        lab.expire_stalled_delivery(start + timeout + Duration::from_millis(1));
        assert!(
            lab.has_peer(address),
            "ACK and NAK both extend a live delivery deadline"
        );
        let mut buffer = [0; 2048];
        let length = socket.recv(&mut buffer).unwrap();
        let packet = read_packet(&buffer[..length], &to_client).unwrap();
        let received = client.receive(&packet.header).unwrap().unwrap();
        client.ack_seq(received.sequence, true).unwrap();
        let ack = write_packet(
            &Packet {
                magic: 0,
                header: client.header().unwrap(),
                info: None,
                bunches: Vec::new(),
            },
            &lab.config.packet.packet,
        )
        .unwrap();
        client.commit().unwrap();
        receive_authored_at(&mut lab, &socket, &ack, progress + Duration::from_millis(2));
        lab.expire_stalled_delivery(progress + timeout * 100);
        assert!(
            lab.has_peer(address),
            "no outstanding delivery means no delivery timeout"
        );
        assert_eq!(lab.peers[&address].delivery_progress_at, None);
        assert_eq!(lab.stats.peers_delivery_stalled, 0);
        // A new nonempty epoch does not inherit the old deadline.
        lab.peers
            .get_mut(&address)
            .unwrap()
            .peer
            .ack_packet()
            .unwrap();
        let fresh = progress + timeout * 101;
        lab.expire_stalled_delivery(fresh);
        lab.expire_stalled_delivery(fresh + timeout);
        assert!(lab.has_peer(address), "new traffic gets the full timeout");
        lab.expire_stalled_delivery(fresh + timeout + Duration::from_millis(1));
        assert!(!lab.has_peer(address));
        assert_eq!(lab.stats.peers_delivery_stalled, 1);
    }
}

#[test]
fn readiness_snapshot_counts_real_transport_settlement_and_partitions_player_phases() {
    use super::super::profile_exchange::ProfileExchange;

    let mut lab = prospect_refresh_lab();
    let (_socket, address, _) = acked_game_state_peer(&mut lab);
    let settled = lab.readiness_counts();
    assert_eq!(settled.actor_channels_open, 1);
    assert_eq!(settled.actor_channels_reliable_settled, 1);
    assert_eq!(settled.actor_reliable_pending, 0);
    assert_eq!(settled.sent_packets_pending, 0);
    let entry = lab.peers.get_mut(&address).unwrap();
    entry.player = Some(RegisteredPlayer {
        player_id: 1,
        name_units: "PRIVATE-READINESS-NAME".encode_utf16().collect(),
    });
    assert_eq!(lab.readiness_counts().players_awaiting_begin, 1);

    // A Begin retained behind PlayerState is distinct from no Begin.
    lab.peers.get_mut(&address).unwrap().initial_opens = Some(PendingInitialOpens {
        remaining: VecDeque::from([InitialOpenStage::PlayerState]),
        controller_guids: Some((2, 4)),
        player_state_is_host: false,
        begin_slot: Some(17),
        retry_ready: false,
    });
    let waiting = lab.readiness_counts();
    assert_eq!(waiting.pending_initial_opens, 1);
    assert_eq!(waiting.pending_initial_stages, 1);
    assert_eq!(waiting.players_awaiting_begin, 0);
    assert_eq!(waiting.players_begin_waiting_for_player_state, 1);
    lab.peers.get_mut(&address).unwrap().initial_opens = None;

    let cases = [
        (
            ProfileExchange::default(),
            Some(PendingProfileExchange::Request(ProfileKind::User)),
            "players_awaiting_user_profile",
        ),
        (
            ProfileExchange {
                outstanding: Some(ProfileKind::User),
                ..Default::default()
            },
            None,
            "players_awaiting_user_profile",
        ),
        (
            ProfileExchange::default(),
            Some(PendingProfileExchange::Publish(ProfileKind::User, vec![])),
            "players_awaiting_user_profile",
        ),
        (
            ProfileExchange {
                outstanding: Some(ProfileKind::Character),
                ..Default::default()
            },
            None,
            "players_awaiting_character_profile",
        ),
        (
            ProfileExchange::default(),
            Some(PendingProfileExchange::Publish(
                ProfileKind::Character,
                vec![],
            )),
            "players_awaiting_character_profile",
        ),
        (
            ProfileExchange::default(),
            Some(PendingProfileExchange::Initialisation),
            "players_awaiting_initialisation",
        ),
        (
            ProfileExchange {
                initialisation_requested: true,
                has_loadout: true,
                ..Default::default()
            },
            None,
            "players_awaiting_initialisation",
        ),
        (
            ProfileExchange {
                initialisation_requested: true,
                has_loadout: true,
                has_bestiary: true,
                ..Default::default()
            },
            Some(PendingProfileExchange::Complete),
            "players_awaiting_completion",
        ),
        (
            ProfileExchange {
                completed: true,
                ..Default::default()
            },
            None,
            "players_awaiting_finalisation",
        ),
        (
            ProfileExchange {
                completed: true,
                finalised: true,
                ..Default::default()
            },
            None,
            "players_finalised",
        ),
    ];
    for (profiles, pending, expected_phase) in cases {
        let entry = lab.peers.get_mut(&address).unwrap();
        entry.character_slot = Some(17);
        entry.profiles = profiles;
        let pending_count = usize::from(pending.is_some());
        entry.pending_profile_exchange = pending;
        let snapshot = lab.readiness_counts();
        assert_eq!(
            snapshot,
            lab.readiness_counts(),
            "snapshot must not consume phases"
        );
        assert_eq!(snapshot.pending_profile_exchanges, pending_count);
        let value = serde_json::to_value(&snapshot).unwrap();
        assert_eq!(value[expected_phase], 1, "{expected_phase}");
        let phase_sum: u64 = value
            .as_object()
            .unwrap()
            .iter()
            .filter(|(key, _)| key.starts_with("players_"))
            .map(|(_, count)| count.as_u64().unwrap())
            .sum();
        assert_eq!(phase_sum, 1, "primary player occupies exactly one phase");
        let text = serde_json::to_string(&snapshot).unwrap();
        assert!(!text.contains("PRIVATE-READINESS-NAME"));
        assert!(!text.contains(&address.to_string()));
    }
    assert_eq!(
        lab.stats.profile_user_requests_sent, 0,
        "diagnostics never drives exchange"
    );
    let channel = lab.game_state_open.as_ref().unwrap().channel_index;
    lab.peers
        .get_mut(&address)
        .unwrap()
        .peer
        .reliable_actor_bunch(
            channel,
            Payload::from_writer(super::super::packet::BitWriter::new()),
        )
        .unwrap();
    let outstanding = lab.readiness_counts();
    assert_eq!(outstanding.actor_channels_open, 1);
    assert_eq!(outstanding.actor_channels_reliable_settled, 0);
    assert_eq!(outstanding.actor_reliable_pending, 1);
    assert_eq!(outstanding.sent_packets_pending, 1);
    lab.remove_peer(address).unwrap();
    assert_eq!(lab.readiness_counts(), ReadinessCounts::default());
}

#[test]
fn readiness_initialisation_blockers_follow_admission_and_each_answer() {
    let mut lab = prospect_refresh_lab();
    lab.config.packet.sent_capacity = 8;
    let (socket, address, mut client) = acked_game_state_peer(&mut lab);
    lab.game_state_open = None;
    assert!(lab.send_player_controller_open(address));
    ack_startup_packets(&mut lab, &socket, &mut client);
    let entry = lab.peers.get_mut(&address).unwrap();
    entry.player = Some(RegisteredPlayer {
        player_id: 1,
        name_units: "AUTHORED-BLOCKERS".encode_utf16().collect(),
    });
    entry.profiles.character_slot = Some(17);
    assert_eq!(
        lab.readiness_counts().initialisation_blockers,
        InitialisationBlockers::default()
    );
    // An answer before its request is admitted is ignored and creates no
    // missing-response observation.
    lab.controller_call(
        address,
        ControllerCall::Initialisation(InitialisationAnswer::Loadout),
    );
    assert!(!lab.peers[&address].profiles.has_loadout);
    let capacity = lab.peers[&address].peer.config().sent_capacity;
    for _ in 0..capacity {
        let datagram = lab
            .peers
            .get_mut(&address)
            .unwrap()
            .peer
            .ack_packet()
            .unwrap();
        assert!(lab.send(&datagram, address));
    }
    lab.request_initialisation(address);
    assert!(!lab.peers[&address].profiles.initialisation_requested);
    let deferred = lab.readiness_counts();
    assert_eq!(
        deferred.initialisation_blockers,
        InitialisationBlockers {
            requests_pending_queue: 1,
            ..Default::default()
        }
    );
    for _ in 0..4 {
        lab.retry_pending_profile_exchanges();
        assert_eq!(lab.readiness_counts(), deferred);
    }
    ack_startup_packets(&mut lab, &socket, &mut client);
    lab.retry_pending_profile_exchanges();
    assert!(lab.peers[&address].profiles.initialisation_requested);
    assert_eq!(
        lab.readiness_counts().initialisation_blockers,
        InitialisationBlockers {
            requests_pending_queue: 0,
            awaiting_loadout_response: 1,
            awaiting_bestiary_response: 1,
        }
    );
    lab.controller_call(
        address,
        ControllerCall::Initialisation(InitialisationAnswer::Bestiary),
    );
    assert_eq!(
        lab.readiness_counts().initialisation_blockers,
        InitialisationBlockers {
            awaiting_loadout_response: 1,
            ..Default::default()
        }
    );
    lab.controller_call(
        address,
        ControllerCall::Initialisation(InitialisationAnswer::Bestiary),
    );
    assert_eq!(
        lab.readiness_counts()
            .initialisation_blockers
            .awaiting_loadout_response,
        1
    );
    lab.controller_call(
        address,
        ControllerCall::Initialisation(InitialisationAnswer::Loadout),
    );
    assert_eq!(lab.readiness_counts().players_awaiting_completion, 1);
    assert_eq!(
        lab.readiness_counts().initialisation_blockers,
        InitialisationBlockers::default()
    );
    lab.remove_peer(address).unwrap();
    assert_eq!(lab.readiness_counts(), ReadinessCounts::default());
}

fn initialisation_transport_peer() -> (
    PeerLab,
    UdpSocket,
    SocketAddrV4,
    super::super::sequencing::PacketSequencer,
) {
    use super::super::control_messages::{NetIdType, NetIdValue, UniqueNetId};
    let mut lab = prospect_refresh_lab();
    let profile = ReleaseProfile::fixture("3.0.32");
    let map = PeerLabAdmission::default().map_url;
    lab.enable_player_state_open(profile.player_state_open_plan(map).unwrap().unwrap())
        .unwrap();
    lab.enable_pawn_open(
        profile.pawn_open_plan(map).unwrap().unwrap(),
        super::super::pawn_open::PAWN_CHANNEL_INDEX,
        PawnContent::default(),
    )
    .unwrap();
    let (socket, address, mut client) = acked_game_state_peer(&mut lab);
    let entry = lab.peers.get_mut(&address).unwrap();
    entry.connection_id = Some(1);
    entry.player = Some(RegisteredPlayer {
        player_id: 1,
        name_units: "AUTHORED-INITIALISATION".encode_utf16().collect(),
    });
    entry.login_unique_id = Some(UniqueNetId {
        kind: NetIdType::Named(ArchiveText::text("NULL").unwrap()),
        value: NetIdValue::Text(ArchiveText::text("AuthoredPlayer").unwrap()),
    });
    entry.profiles.character_slot = Some(3);
    let game_state = lab.game_state_open.take();
    assert!(lab.send_player_controller_open(address));
    lab.game_state_open = game_state;
    ack_startup_packets(&mut lab, &socket, &mut client);
    assert_eq!(lab.stats.player_state_opens_sent, 1);
    assert_eq!(lab.stats.pawn_opens_sent, 1);
    (lab, socket, address, client)
}

#[test]
fn friends_query_begin_fanout_is_reliable_empty_and_requires_real_reply() {
    use super::super::{
        actor_channel::read_actor_bunch_body,
        packet::{Receiver, read_packet},
    };
    let (mut lab, socket, address, mut client) = initialisation_transport_peer();
    let engine = lab
        .player_controller_open
        .as_ref()
        .unwrap()
        .engine_net_version;
    let codec = RepLayoutCodec {
        actor: actor_codec(engine, super::super::archive::ByteOrder::Little),
        checksums: false,
        limits: Default::default(),
    };
    let entry = lab.peers.get_mut(&address).unwrap();
    entry.character_slot = Some(3);
    entry.profiles.completed = false;
    entry.profiles.friend_ids_ready = true;
    // A fresh controller's Begin must requery an existing connected player.
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
    let entry = lab.peers.get_mut(&second_address).unwrap();
    entry.player = Some(RegisteredPlayer {
        player_id: 2,
        name_units: vec![],
    });
    assert!(entry.character_slot.is_none());
    assert!(
        entry
            .player_state_channel
            .is_some_and(|c| entry.peer.has_actor_channel(c))
    );
    if !lab.peers[&address]
        .peer
        .sequencer()
        .notify()
        .sent_records()
        .is_empty()
    {
        ack_startup_packets(&mut lab, &socket, &mut client);
    }
    lab.peers.get_mut(&address).unwrap().profiles.completed = true;
    lab.controller_call(second_address, ControllerCall::Begin(4));
    assert_eq!(lab.stats.friends_list_queries_queued, 2);
    let mut bytes = [0; 2048];
    let n = socket.peek(&mut bytes).unwrap();
    let packet = read_packet(
        &bytes[..n],
        &super::super::packet::PacketConfig {
            receiver: Receiver::Client,
            ..lab.config.packet.packet
        },
    )
    .unwrap();
    assert_eq!(packet.bunches.len(), 1);
    let bunch = &packet.bunches[0];
    assert!(bunch.header.reliable);
    assert_eq!(
        Some(bunch.header.channel_index),
        lab.peers[&address].player_controller_channel
    );
    let body = read_actor_bunch_body(&mut bunch.payload.reader(), false, &codec.actor).unwrap();
    assert_eq!(body.blocks.len(), 1);
    assert_eq!(body.blocks[0].target, BlockTarget::Actor);
    let mut reader = body.blocks[0].payload.reader();
    let field = read_field_header(&mut reader, lab.controller_startup_bound, 1024).unwrap();
    assert!(reader.at_end());
    assert_eq!(field.index, 118);
    assert_eq!(field.payload.bit_len(), 0);
    assert!(!lab.peers[&address].profiles.friend_ids_ready);
    // Transport settlement supplies no client friends-list response.
    ack_startup_packets(&mut lab, &socket, &mut client);
    lab.finalise_connected_players();
    assert_eq!(lab.stats.players_finalised, 0);
    lab.controller_call(address, ControllerCall::FriendsList);
    lab.controller_call(second_address, ControllerCall::FriendsList);
    lab.finalise_connected_players();
    assert_eq!(lab.stats.players_finalised, 1);
    lab.controller_call(second_address, ControllerCall::Begin(4));
    lab.retry_friends_queries();
    assert_eq!(lab.stats.friends_list_queries_queued, 2);
    lab.shutdown();
}

#[test]
fn friends_query_pressure_retries_once_and_disconnect_discards_pending() {
    let (mut lab, socket, address, mut client) = initialisation_transport_peer();
    let entry = lab.peers.get_mut(&address).unwrap();
    entry.character_slot = Some(3);
    entry.profiles.completed = true;
    entry.profiles.friend_ids_ready = true;
    for _ in 0..entry.peer.config().sent_capacity {
        let datagram = entry.peer.ack_packet().unwrap();
        lab.socket.send_to(&datagram, address).unwrap();
        let mut bytes = [0; 2048];
        let n = socket.recv(&mut bytes).unwrap();
        let packet = super::super::packet::read_packet(
            &bytes[..n],
            &super::super::packet::PacketConfig {
                receiver: super::super::packet::Receiver::Client,
                ..lab.config.packet.packet
            },
        )
        .unwrap();
        let received = client.receive(&packet.header).unwrap().unwrap();
        client.ack_seq(received.sequence, true).unwrap();
    }
    lab.query_connected_friends();
    assert!(lab.peers[&address].friends_query_pending);
    assert_eq!(lab.stats.friends_list_queries_queued, 0);
    lab.finalise_connected_players();
    assert_eq!(lab.stats.players_finalised, 0);
    for _ in 0..3 {
        lab.retry_friends_queries();
    }
    assert_eq!(lab.stats.friends_list_queries_queued, 0);
    let bytes = super::super::packet::write_packet(
        &super::super::packet::Packet {
            magic: 0,
            header: client.header().unwrap(),
            info: None,
            bunches: vec![],
        },
        &lab.config.packet.packet,
    )
    .unwrap();
    socket.send(&bytes).unwrap();
    lab.poll();
    lab.retry_friends_queries();
    assert_eq!(lab.stats.friends_list_queries_queued, 1);
    assert!(!lab.peers[&address].friends_query_pending);
    assert!(!lab.peers[&address].profiles.friend_ids_ready);
    lab.retry_friends_queries();
    assert_eq!(lab.stats.friends_list_queries_queued, 1);
    lab.remove_peer(address).unwrap();
    lab.retry_friends_queries();
    assert_eq!(lab.peers.len(), 0);
}

#[test]
fn friends_query_nak_repairs_transport_without_requerying_or_finalising() {
    let (mut lab, socket, address, mut client) = initialisation_transport_peer();
    let entry = lab.peers.get_mut(&address).unwrap();
    entry.character_slot = Some(3);
    entry.profiles.completed = true;
    lab.query_connected_friends();
    assert_eq!(lab.stats.friends_list_queries_queued, 1);
    notify_startup_packets(&mut lab, &socket, &mut client, |_| false);
    assert!(lab.readiness_counts().resend_bunches_pending > 0);
    lab.retry_friends_queries();
    lab.finalise_connected_players();
    assert_eq!(lab.stats.friends_list_queries_queued, 1);
    assert_eq!(lab.stats.players_finalised, 0);
    let repair = lab
        .peers
        .get_mut(&address)
        .unwrap()
        .peer
        .ack_packet()
        .unwrap();
    assert!(lab.send(&repair, address));
    ack_startup_packets(&mut lab, &socket, &mut client);
    assert_eq!(lab.readiness_counts().resend_bunches_pending, 0);
    lab.finalise_connected_players();
    assert!(!lab.peers[&address].profiles.friend_ids_ready);
    assert_eq!(lab.stats.friends_lists_received, 0);
    assert_eq!(lab.stats.players_finalised, 0);
    assert_eq!(lab.stats.friends_list_queries_queued, 1);
}

#[test]
fn friends_query_held_back_shows_pending_admission_until_queued() {
    let (mut lab, socket, address, mut client) = initialisation_transport_peer();
    let entry = lab.peers.get_mut(&address).unwrap();
    entry.character_slot = Some(3);
    entry.profiles.completed = true;
    entry.profiles.friend_ids_ready = true;
    for _ in 0..entry.peer.config().sent_capacity {
        let datagram = entry.peer.ack_packet().unwrap();
        lab.socket.send_to(&datagram, address).unwrap();
        let mut bytes = [0; 2048];
        let n = socket.recv(&mut bytes).unwrap();
        let packet = super::super::packet::read_packet(
            &bytes[..n],
            &super::super::packet::PacketConfig {
                receiver: super::super::packet::Receiver::Client,
                ..lab.config.packet.packet
            },
        )
        .unwrap();
        let received = client.receive(&packet.header).unwrap().unwrap();
        client.ack_seq(received.sequence, true).unwrap();
    }
    assert_eq!(lab.readiness_counts().friends_query, Default::default());
    lab.query_connected_friends();
    assert_eq!(
        lab.readiness_counts().friends_query,
        FriendsQueryReadiness {
            pending_admission: 1,
            ..Default::default()
        }
    );
    let bytes = super::super::packet::write_packet(
        &super::super::packet::Packet {
            magic: 0,
            header: client.header().unwrap(),
            info: None,
            bunches: vec![],
        },
        &lab.config.packet.packet,
    )
    .unwrap();
    socket.send(&bytes).unwrap();
    lab.poll();
    lab.retry_friends_queries();
    assert_eq!(lab.stats.friends_list_queries_queued, 1);
    assert_eq!(
        lab.readiness_counts().friends_query,
        FriendsQueryReadiness {
            queued: 1,
            awaiting_ack: 1,
            ..Default::default()
        }
    );
    assert_eq!(lab.stats.friends_list_queries_acknowledged, 0);
    lab.remove_peer(address).unwrap();
    assert_eq!(lab.readiness_counts().friends_query, Default::default());
}

#[test]
fn friends_query_receipt_settles_on_its_own_bunch_ack_then_awaits_response() {
    let (mut lab, socket, address, mut client) = initialisation_transport_peer();
    let entry = lab.peers.get_mut(&address).unwrap();
    entry.character_slot = Some(3);
    entry.profiles.completed = true;
    let channel = entry.player_controller_channel.unwrap();
    lab.query_connected_friends();
    assert_eq!(lab.stats.friends_list_queries_queued, 1);
    let awaiting_ack = FriendsQueryReadiness {
        queued: 1,
        awaiting_ack: 1,
        ..Default::default()
    };
    assert_eq!(lab.readiness_counts().friends_query, awaiting_ack);
    // A later reliable bunch on the same channel whose packet is lost:
    // the channel stays unsettled while the query's own bunch settles.
    let later = lab
        .peers
        .get_mut(&address)
        .unwrap()
        .peer
        .reliable_actor_bunch(
            channel,
            Payload::from_writer(super::super::packet::BitWriter::new()),
        )
        .unwrap();
    assert!(lab.send(&later, address));
    // NAK the query's packet first: nothing settles.
    notify_startup_packets(&mut lab, &socket, &mut client, |_| false);
    assert_eq!(lab.readiness_counts().friends_query, awaiting_ack);
    assert_eq!(lab.stats.friends_list_queries_acknowledged, 0);
    // Repair resends both bunches in one packet; lose a fresh later bunch.
    let repair = lab
        .peers
        .get_mut(&address)
        .unwrap()
        .peer
        .ack_packet()
        .unwrap();
    assert!(lab.send(&repair, address));
    let later = lab
        .peers
        .get_mut(&address)
        .unwrap()
        .peer
        .reliable_actor_bunch(
            channel,
            Payload::from_writer(super::super::packet::BitWriter::new()),
        )
        .unwrap();
    assert!(lab.send(&later, address));
    notify_startup_packets(&mut lab, &socket, &mut client, |index| index == 0);
    assert!(!lab.peers[&address].peer.actor_channel_settled(channel));
    assert_eq!(lab.stats.friends_list_queries_acknowledged, 1);
    assert_eq!(
        lab.readiness_counts().friends_query,
        FriendsQueryReadiness {
            queued: 1,
            settled: 1,
            awaiting_response: 1,
            ..Default::default()
        }
    );
    // The ACK alone neither readies friend IDs nor finalises.
    lab.finalise_connected_players();
    assert_eq!(lab.stats.players_finalised, 0);
    lab.controller_call(address, ControllerCall::FriendsList);
    assert_eq!(lab.stats.friends_lists_received, 1);
    assert_eq!(
        lab.readiness_counts().friends_query,
        FriendsQueryReadiness {
            queued: 1,
            settled: 1,
            ..Default::default()
        }
    );
    // Settlement is counted once per admitted query: repairing the
    // later bunch settles the channel without a second count.
    let repair = lab
        .peers
        .get_mut(&address)
        .unwrap()
        .peer
        .ack_packet()
        .unwrap();
    assert!(lab.send(&repair, address));
    ack_startup_packets(&mut lab, &socket, &mut client);
    assert!(lab.peers[&address].peer.actor_channel_settled(channel));
    assert_eq!(lab.stats.friends_list_queries_acknowledged, 1);
    assert_eq!(lab.session_counts().friends_list_queries_acknowledged, 1);
}

#[test]
fn ignored_friends_lists_are_counted_apart_from_ignored_begin_calls() {
    let (mut lab, _socket, address, _client) = initialisation_transport_peer();
    lab.controller_call(address, ControllerCall::FriendsList);
    assert_eq!(lab.stats.friends_lists_received, 1);
    assert_eq!(lab.stats.initialisation_calls_ignored, 0);
    lab.controller_call(address, ControllerCall::FriendsList);
    assert_eq!(lab.stats.friends_lists_ignored_already_ready, 1);
    assert_eq!(lab.stats.friends_lists_ignored_unregistered, 0);
    assert_eq!(lab.stats.initialisation_calls_ignored, 1);
    lab.controller_call(address, ControllerCall::Begin(-1));
    assert_eq!(lab.stats.initialisation_calls_ignored, 2);
    assert_eq!(lab.stats.friends_lists_ignored_already_ready, 1);
    let entry = lab.peers.get_mut(&address).unwrap();
    entry.player = None;
    entry.profiles.friend_ids_ready = false;
    lab.controller_call(address, ControllerCall::FriendsList);
    assert_eq!(lab.stats.friends_lists_ignored_unregistered, 1);
    assert_eq!(lab.stats.friends_lists_ignored_already_ready, 1);
    assert_eq!(lab.stats.initialisation_calls_ignored, 3);
    assert_eq!(lab.stats.friends_lists_received, 1);
    let counts = lab.session_counts();
    assert_eq!(counts.friends_lists_ignored_unregistered, 1);
    assert_eq!(counts.friends_lists_ignored_already_ready, 1);
    assert_eq!(counts.initialisation_calls_ignored, 3);
}

/// A lab with every open a connected-player record references and Mode A
/// records configured before any peer exists.
fn record_lab(enabled: bool) -> PeerLab {
    let mut lab = prospect_refresh_lab();
    let profile = ReleaseProfile::fixture("3.0.32");
    let map = PeerLabAdmission::default().map_url;
    lab.enable_player_state_open(profile.player_state_open_plan(map).unwrap().unwrap())
        .unwrap();
    lab.enable_pawn_open(
        profile.pawn_open_plan(map).unwrap().unwrap(),
        super::super::pawn_open::PAWN_CHANNEL_INDEX,
        PawnContent::default(),
    )
    .unwrap();
    if enabled {
        lab.enable_connected_player_records(
            ReleaseProfile::fixture("3.0.35")
                .connected_players_plan()
                .unwrap()
                .unwrap(),
        )
        .unwrap();
    }
    lab
}

/// A joined peer with its GameState, controller, pawn and PlayerState open
/// and acked, an authored login ID and profile slot 3; no Begin yet.
fn record_peer(
    lab: &mut PeerLab,
    connection: u32,
    user: &str,
) -> (
    UdpSocket,
    SocketAddrV4,
    super::super::sequencing::PacketSequencer,
) {
    let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    socket.connect(lab.local_addr().unwrap()).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(1)))
        .unwrap();
    let (address, client) = record_peer_on(lab, &socket, connection, user);
    (socket, address, client)
}

/// The stateless handshake on `socket`: a new peer, or a replacement
/// (new cookie) of the peer this address already has.
fn handshake_peer_on(
    lab: &mut PeerLab,
    socket: &UdpSocket,
) -> (SocketAddrV4, super::super::sequencing::PacketSequencer) {
    use super::super::{
        handshake::initial_request, packet_lab::client_cookie_seeds, sequencing::PacketSequencer,
    };
    let SocketAddr::V4(address) = socket.local_addr().unwrap() else {
        unreachable!("IPv4 loopback")
    };
    // A rejoining address may still have its old session's data packets
    // in flight; only the handshake replies matter here.
    let handshake = |buffer: &mut [u8; 2048]| loop {
        let length = socket.recv(buffer).unwrap();
        if let Ok(reply) = decode(&buffer[..length]) {
            return (reply, length);
        }
    };
    let mut buffer = [0; 2048];
    socket.send(&initial_request()).unwrap();
    lab.poll();
    let (challenge, length) = handshake(&mut buffer);
    let cookie = challenge.cookie;
    socket.send(&buffer[..length]).unwrap();
    lab.poll();
    let (acknowledgement, _) = handshake(&mut buffer);
    assert_eq!(acknowledgement.cookie, cookie);
    let (incoming, outgoing) = client_cookie_seeds(&cookie);
    (
        address,
        PacketSequencer::new(incoming, outgoing, 64).unwrap(),
    )
}

/// [`record_peer`] on an existing socket: handshake, then the real
/// initial-open sequence (controller, pawn, GameState, PlayerState), acked.
fn record_peer_on(
    lab: &mut PeerLab,
    socket: &UdpSocket,
    connection: u32,
    user: &str,
) -> (SocketAddrV4, super::super::sequencing::PacketSequencer) {
    use super::super::control_messages::{NetIdType, NetIdValue, UniqueNetId};
    let (address, mut client) = handshake_peer_on(lab, socket);
    let entry = lab.peers.get_mut(&address).unwrap();
    entry.connection_id = Some(connection);
    entry.player = Some(RegisteredPlayer {
        player_id: connection as i32,
        name_units: "AUTHORED-RECORD".encode_utf16().collect(),
    });
    entry.login_unique_id = Some(UniqueNetId {
        kind: NetIdType::Named(ArchiveText::text("NULL").unwrap()),
        value: NetIdValue::Text(ArchiveText::text(user).unwrap()),
    });
    entry.profiles.character_slot = Some(3);
    assert!(lab.send_player_controller_open(address));
    assert!(lab.peers[&address].game_state.is_some());
    ack_startup_packets(lab, socket, &mut client);
    (address, client)
}

/// Begin, both initialisation answers (completion) and the friends list:
/// the authored session reaches finalisation's preconditions.
fn complete_record_player(lab: &mut PeerLab, address: SocketAddrV4) {
    lab.controller_call(address, ControllerCall::Begin(3));
    let entry = lab.peers.get_mut(&address).unwrap();
    entry.profiles.outstanding = None;
    entry.profiles.has_loadout = true;
    entry.profiles.has_bestiary = true;
    lab.complete_player_initialisation(address);
    assert!(lab.peers[&address].profiles.completed);
}

/// Read every outstanding datagram of `address`, notifying the lab as
/// `delivered` says, and return the bunches they carried in order.
fn deliver_bunches(
    lab: &mut PeerLab,
    socket: &UdpSocket,
    client: &mut super::super::sequencing::PacketSequencer,
    delivered: impl Fn(usize) -> bool,
) -> Vec<super::super::packet::Bunch> {
    deliver_bunches_and_reply(lab, socket, client, delivered, Vec::new())
}

fn deliver_bunches_and_reply(
    lab: &mut PeerLab,
    socket: &UdpSocket,
    client: &mut super::super::sequencing::PacketSequencer,
    delivered: impl Fn(usize) -> bool,
    reply: Vec<super::super::packet::Bunch>,
) -> Vec<super::super::packet::Bunch> {
    use super::super::packet::{Packet, Receiver, read_packet, write_packet};
    let SocketAddr::V4(address) = socket.local_addr().unwrap() else {
        unreachable!()
    };
    let count = lab.peers[&address]
        .peer
        .sequencer()
        .notify()
        .sent_records()
        .len();
    let config = super::super::packet::PacketConfig {
        receiver: Receiver::Client,
        ..lab.config.packet.packet
    };
    let mut bunches = Vec::new();
    for index in 0..count {
        let mut bytes = [0; 2048];
        let length = socket.recv(&mut bytes).unwrap();
        let packet = read_packet(&bytes[..length], &config).unwrap();
        let received = client.receive(&packet.header).unwrap().unwrap();
        client.ack_seq(received.sequence, delivered(index)).unwrap();
        bunches.extend(packet.bunches);
    }
    if count > 0 || !reply.is_empty() {
        let bytes = write_packet(
            &Packet {
                magic: 0,
                header: client.header().unwrap(),
                info: None,
                bunches: reply,
            },
            &lab.config.packet.packet,
        )
        .unwrap();
        client.commit().unwrap();
        receive_authored_at(lab, socket, &bytes, Instant::now());
    }
    bunches
}

fn client_close_bunch(peer: &LabPeer, reason: u32) -> super::super::packet::Bunch {
    use super::super::{
        control_receive::INFERRED_CONTROL_NAME_INDEX,
        packet::{BitWriter, Bunch, BunchHeader, Name},
        sequencing::wire_reliable_sequence,
    };
    Bunch {
        header: BunchHeader {
            control: true,
            open: false,
            close: true,
            close_reason: Some(reason),
            paused: false,
            reliable: true,
            channel_index: peer.config().control_channel_index,
            has_package_map_exports: false,
            has_must_be_mapped_guids: false,
            partial: false,
            reliable_sequence_raw: Some(wire_reliable_sequence(peer.control_in_reliable() + 1)),
            partial_initial: false,
            partial_final: false,
            channel_name: Some(Name::Hardcoded(INFERRED_CONTROL_NAME_INDEX)),
        },
        payload: Payload::from_writer(BitWriter::new()),
    }
}

fn game_state_channel(lab: &PeerLab) -> u32 {
    lab.game_state_open.as_ref().unwrap().channel_index
}

/// Reliable, non-opening GameState-channel bunch bodies (export parts
/// excluded), decoded.
fn record_bodies(lab: &PeerLab, bunches: &[super::super::packet::Bunch]) -> Vec<ActorBunchBody> {
    let channel = game_state_channel(lab);
    let codec = actor_codec(17, super::super::archive::ByteOrder::Little);
    bunches
        .iter()
        .filter(|bunch| {
            bunch.header.channel_index == channel
                && bunch.header.reliable
                && !bunch.header.open
                && !bunch.header.has_package_map_exports
        })
        .map(|bunch| {
            super::super::actor_channel::read_actor_bunch_body(
                &mut bunch.payload.reader(),
                false,
                &codec,
            )
            .unwrap()
        })
        .collect()
}

fn record_codec() -> RepLayoutCodec {
    RepLayoutCodec {
        actor: actor_codec(17, super::super::archive::ByteOrder::Little),
        checksums: false,
        limits: Default::default(),
    }
}

/// The record GUIDs a decoded `ConnectedPlayers` block lists.
fn listed_records(lab: &PeerLab, block: &ContentBlock) -> Vec<u32> {
    let plan = lab.connected_players.as_ref().unwrap();
    let stream = super::super::rep_layout::parse_rep_properties(
        &block.payload,
        &plan.component_layout,
        &record_codec(),
    )
    .unwrap();
    let [entry] = stream.entries.as_slice() else {
        panic!("only the array handle");
    };
    assert_eq!(entry.handle, 3);
    let RepValue::DynamicArray(array) = &entry.value else {
        panic!("an array");
    };
    assert_eq!(usize::from(array.count), array.stream.entries.len());
    array
        .stream
        .entries
        .iter()
        .map(|element| match &element.value {
            RepValue::Object(reference) => reference.guid,
            other => panic!("object element expected, got {other:?}"),
        })
        .collect()
}

#[test]
fn connected_player_record_waits_for_finalisation_then_is_created_once() {
    let mut lab = record_lab(true);
    let (socket, address, mut client) = record_peer(&mut lab, 1, "AuthoredRecordUser");
    assert_eq!(
        lab.readiness_counts().connected_player_records,
        ConnectedPlayerRecordReadiness::default()
    );
    complete_record_player(&mut lab, address);
    // Registered at Begin; Mode A sends nothing before finalisation.
    assert_eq!(lab.stats.connected_player_records_created, 1);
    assert_eq!(
        lab.readiness_counts().connected_player_records,
        ConnectedPlayerRecordReadiness {
            registered: 1,
            ..Default::default()
        }
    );
    let bunches = deliver_bunches(&mut lab, &socket, &mut client, |_| true);
    assert!(record_bodies(&lab, &bunches).is_empty());
    lab.finalise_connected_players();
    assert_eq!(lab.stats.players_finalised, 0, "friends list outstanding");
    lab.retry_connected_player_records();
    assert_eq!(lab.stats.connected_player_record_creations_sent, 0);
    lab.controller_call(address, ControllerCall::FriendsList);
    lab.finalise_connected_players();
    assert_eq!(lab.stats.players_finalised, 1);
    assert_eq!(lab.stats.connected_player_record_creations_sent, 1);
    assert_eq!(lab.stats.connected_player_array_updates_sent, 1);
    assert_eq!(
        lab.readiness_counts().connected_player_records,
        ConnectedPlayerRecordReadiness {
            registered: 1,
            finalised: 1,
            creations_queued: 1,
            records_awaiting_ack: 1,
            ..Default::default()
        }
    );
    let bunches = deliver_bunches(&mut lab, &socket, &mut client, |_| true);
    let channel = game_state_channel(&lab);
    let exports: Vec<_> = bunches
        .iter()
        .filter(|bunch| bunch.header.channel_index == channel)
        .map(|bunch| bunch.header.has_package_map_exports)
        .collect();
    // The class export precedes the body on the same reliable channel.
    assert_eq!(exports, [true, false]);
    let [body] = record_bodies(&lab, &bunches).try_into().unwrap();
    let [actor, record, component] = body.blocks.as_slice() else {
        panic!("three blocks");
    };
    assert_eq!(actor.target, BlockTarget::Actor);
    let plan = lab.connected_players.clone().unwrap();
    let entry = &lab.peers[&address];
    let (controller, player_state) = entry.controller_open_guids.unwrap();
    let BlockTarget::SubObject {
        object,
        class: super::super::actor_channel::SubObjectClass::Class(_),
    } = &record.target
    else {
        panic!("a class-named record block");
    };
    let values = super::super::rep_layout::parse_rep_properties(
        &record.payload,
        &plan.record_layout,
        &record_codec(),
    )
    .unwrap();
    let handles: Vec<u32> = values.entries.iter().map(|entry| entry.handle).collect();
    assert_eq!(handles, [1, 2, 3, 4, 5, 6, 7]);
    let value = |handle: usize| values.entries[handle - 1].value.clone();
    assert_eq!(
        value(1),
        RepValue::Str(ArchiveText::text("AuthoredRecordUser").unwrap())
    );
    assert_eq!(value(2), RepValue::Int(3));
    assert_eq!(
        value(3),
        RepValue::Object(super::super::actor_channel::NetGuidRef::plain(controller))
    );
    assert_eq!(
        value(4),
        RepValue::Object(super::super::actor_channel::NetGuidRef::plain(
            entry.controller_startup_pawn_guid.unwrap()
        ))
    );
    assert_eq!(
        value(5),
        RepValue::Object(super::super::actor_channel::NetGuidRef::plain(player_state))
    );
    let (RepValue::Float(start), RepValue::Float(complete)) = (value(6), value(7)) else {
        panic!("float times");
    };
    assert!(f32::from_bits(complete) >= f32::from_bits(start));
    assert_eq!(listed_records(&lab, component), [object.guid]);
    // A positive ACK settles the creation once.
    assert_eq!(lab.stats.connected_player_record_creations_acked, 1);
    assert_eq!(
        lab.readiness_counts().connected_player_records,
        ConnectedPlayerRecordReadiness {
            registered: 1,
            finalised: 1,
            creations_queued: 1,
            creations_settled: 1,
            ..Default::default()
        }
    );
    // Nothing changes on later passes, finalisation or requeries.
    lab.finalise_connected_players();
    lab.mark_connected_players_due(|_| true);
    lab.retry_connected_player_records();
    lab.controller_call(address, ControllerCall::Begin(3));
    assert!(deliver_bunches(&mut lab, &socket, &mut client, |_| true).is_empty());
    assert_eq!(lab.stats.connected_player_record_creations_sent, 1);
    assert_eq!(lab.stats.connected_player_array_updates_sent, 1);
    assert_eq!(lab.stats.connected_player_record_creations_acked, 1);
    assert_eq!(lab.stats.connected_player_records_created, 1);
    assert_eq!(lab.stats.connected_player_record_updates_sent, 0);
    assert_eq!(lab.stats.connected_player_record_errors, 0);
    let counts = lab.session_counts();
    assert_eq!(counts.connected_player_record_creations_sent, 1);
    assert_eq!(counts.connected_player_record_creations_acked, 1);
    let text = serde_json::to_string(&lab.readiness_counts()).unwrap();
    assert!(!text.contains("AuthoredRecordUser"));
    lab.remove_peer(address).unwrap();
    assert_eq!(lab.stats.connected_player_records_removed, 1);
    assert_eq!(lab.readiness_counts(), ReadinessCounts::default());
}

/// The `bServerFrozenMovement` values of the whole unreliable pawn
/// property updates among `bunches`, decoded against the lab's pawn layout.
fn frozen_releases(lab: &PeerLab, bunches: &[super::super::packet::Bunch]) -> Vec<RepValue> {
    let (plan, channel, _) = lab.pawn_open.as_ref().unwrap();
    let codec = record_codec();
    let wire = plan
        .layout_version
        .wire_handle(super::super::pawn_open::handle::SERVER_FROZEN_MOVEMENT)
        .unwrap();
    bunches
        .iter()
        .filter(|bunch| bunch.header.channel_index == *channel && !bunch.header.open)
        .map(|bunch| {
            // Unreliable, as the original sends it (S2).
            assert!(!bunch.header.reliable && !bunch.header.partial);
            let body = super::super::actor_channel::read_actor_bunch_body(
                &mut bunch.payload.reader(),
                false,
                &codec.actor,
            )
            .unwrap();
            let [block] = body.blocks.as_slice() else {
                panic!("one block");
            };
            assert_eq!(block.target, BlockTarget::Actor);
            let stream = super::super::rep_layout::parse_rep_properties(
                &block.payload,
                &plan.layout,
                &codec,
            )
            .unwrap();
            let [entry] = stream.entries.as_slice() else {
                panic!("one property");
            };
            assert_eq!(entry.handle, wire);
            entry.value.clone()
        })
        .collect()
}

/// The release is scheduled by the positive ACK of the player's own
/// record creation, never before; it is sent once after the delay,
/// unreliable; a NAK sends the still-current false again without a second
/// release, and the first ACK of a copy settles it once.
#[test]
fn frozen_movement_release_follows_the_record_ack_after_its_delay_once() {
    let mut lab = record_lab(true);
    let delay = Duration::from_millis(120);
    lab.enable_frozen_movement_release(delay).unwrap();
    assert_eq!(lab.frozen_movement_release_delay(), Some(delay));
    let (socket, address, mut client) = record_peer(&mut lab, 1, "AuthoredFrozenUser");
    assert!(
        lab.enable_frozen_movement_release(delay).is_err(),
        "configured before admission"
    );
    complete_record_player(&mut lab, address);
    deliver_bunches(&mut lab, &socket, &mut client, |_| true);
    lab.controller_call(address, ControllerCall::FriendsList);
    lab.finalise_connected_players();
    assert_eq!(lab.stats.connected_player_record_creations_sent, 1);
    // Finalised and the record sent, but not ACKed: nothing scheduled.
    lab.retry_frozen_movement_release();
    assert_eq!(lab.stats.frozen_movement_releases_scheduled, 0);
    assert_eq!(
        lab.readiness_counts().frozen_movement_release,
        Default::default()
    );
    // A NAK of the record schedules nothing either.
    deliver_bunches(&mut lab, &socket, &mut client, |_| false);
    assert_eq!(lab.stats.frozen_movement_releases_scheduled, 0);
    let repair = lab
        .peers
        .get_mut(&address)
        .unwrap()
        .peer
        .ack_packet()
        .unwrap();
    assert!(lab.send(&repair, address));
    let bunches = deliver_bunches(&mut lab, &socket, &mut client, |_| true);
    assert!(frozen_releases(&lab, &bunches).is_empty());
    assert_eq!(lab.stats.connected_player_record_creations_acked, 1);
    assert_eq!(lab.stats.frozen_movement_releases_scheduled, 1);
    // Waiting out the delay.
    lab.retry_frozen_movement_release();
    assert_eq!(lab.stats.frozen_movement_releases_sent, 0);
    assert_eq!(
        lab.readiness_counts().frozen_movement_release,
        FrozenMovementReleaseReadiness {
            awaiting_delay: 1,
            ..Default::default()
        }
    );
    std::thread::sleep(delay + Duration::from_millis(10));
    lab.retry_frozen_movement_release();
    assert_eq!(lab.stats.frozen_movement_releases_sent, 1);
    assert_eq!(
        lab.readiness_counts().frozen_movement_release,
        FrozenMovementReleaseReadiness {
            awaiting_ack: 1,
            ..Default::default()
        }
    );
    // Nothing is retained for reliable repair.
    assert_eq!(lab.readiness_counts().resend_bunches_pending, 0);
    let [observation] = lab.frozen_movement_observations()[..] else {
        panic!("one session");
    };
    let acked = observation.record_acked_ms.unwrap();
    let delay_ms = delay.as_millis() as u64;
    assert_eq!(observation.release_due_ms, Some(acked + delay_ms));
    assert!(observation.release_sent_ms.unwrap() >= acked + delay_ms);
    // Lost: the current value is due again and sent again, not
    // released a second time.
    let lost = deliver_bunches(&mut lab, &socket, &mut client, |_| false);
    assert_eq!(frozen_releases(&lab, &lost), [RepValue::Bool(false)]);
    assert_eq!(lab.stats.frozen_movement_releases_acked, 0);
    assert_eq!(
        lab.readiness_counts().frozen_movement_release,
        FrozenMovementReleaseReadiness {
            pending_send: 1,
            ..Default::default()
        }
    );
    lab.retry_frozen_movement_release();
    assert_eq!(lab.stats.frozen_movement_release_resends, 1);
    let repaired = deliver_bunches(&mut lab, &socket, &mut client, |_| true);
    assert_eq!(frozen_releases(&lab, &repaired), [RepValue::Bool(false)]);
    assert_eq!(lab.stats.frozen_movement_releases_sent, 1);
    assert_eq!(lab.stats.frozen_movement_releases_acked, 1);
    assert_eq!(
        lab.readiness_counts().frozen_movement_release,
        FrozenMovementReleaseReadiness {
            settled: 1,
            ..Default::default()
        }
    );
    let [observation] = lab.frozen_movement_observations()[..] else {
        panic!("one session");
    };
    assert!(observation.release_acked_ms >= observation.release_sent_ms);
    assert!(!observation.release_failed);
    // Never again: later passes, finalisation and record changes.
    lab.finalise_connected_players();
    lab.mark_connected_players_due(|_| true);
    lab.retry_connected_player_records();
    lab.retry_frozen_movement_release();
    assert!(deliver_bunches(&mut lab, &socket, &mut client, |_| true).is_empty());
    assert_eq!(lab.stats.frozen_movement_releases_scheduled, 1);
    assert_eq!(lab.stats.frozen_movement_releases_sent, 1);
    assert_eq!(lab.stats.frozen_movement_releases_acked, 1);
    assert_eq!(lab.stats.frozen_movement_release_resends, 1);
    assert_eq!(lab.stats.frozen_movement_release_errors, 0);
    let counts = lab.session_counts();
    assert_eq!(counts.frozen_movement_releases_sent, 1);
    assert_eq!(counts.frozen_movement_releases_acked, 1);
    assert_eq!(counts.frozen_movement_release_resends, 1);
    let close = client_close_bunch(&lab.peers[&address].peer, 0);
    deliver_bunches_and_reply(&mut lab, &socket, &mut client, |_| true, vec![close]);
    let close = lab.session_counts().last_client_control_close.unwrap();
    assert_eq!(close.own_record_ack, Some(OwnRecordAck::Earlier));
    let release = close.frozen_movement_release.unwrap();
    assert_eq!(release.state, FrozenReleaseState::Acked);
    assert!(release.sent);
    assert_eq!(release.pawn, None);
    assert!(lab.frozen_movement_observations().is_empty());
}

#[test]
fn client_control_close_retains_record_ack_and_release_guards() {
    use FrozenReleaseState::{AwaitingDelay, Due, PendingSend};
    use OwnRecordAck::{Earlier, InClosePacket};
    for (delay, hold_pawn, record_ack, service_pass_reached, state, pawn) in [
        (
            Duration::ZERO,
            false,
            InClosePacket,
            false,
            Due,
            ReleasePawnState::Settled,
        ),
        (
            Duration::ZERO,
            true,
            Earlier,
            true,
            PendingSend,
            ReleasePawnState::ReliablePending,
        ),
        (
            Duration::from_secs(600),
            false,
            Earlier,
            false,
            AwaitingDelay,
            ReleasePawnState::Settled,
        ),
    ] {
        let mut lab = record_lab(true);
        lab.enable_frozen_movement_release(delay).unwrap();
        let (socket, address, mut client) = record_peer(&mut lab, 1, "AuthoredCloseUser");
        complete_record_player(&mut lab, address);
        deliver_bunches(&mut lab, &socket, &mut client, |_| true);
        lab.controller_call(address, ControllerCall::FriendsList);
        lab.finalise_connected_players();
        assert_eq!(lab.stats.connected_player_record_creations_sent, 1);
        assert_eq!(lab.stats.connected_player_record_creations_acked, 0);
        if record_ack == Earlier {
            deliver_bunches(&mut lab, &socket, &mut client, |_| true);
            assert_eq!(lab.stats.connected_player_record_creations_acked, 1);
        }
        if hold_pawn {
            let (plan, channel, _) = lab.pawn_open.as_ref().unwrap();
            let payload = compose_pawn_frozen_movement_update(
                plan,
                true,
                super::super::archive::ByteOrder::Little,
            )
            .unwrap();
            let channel = *channel;
            let pending = lab
                .peers
                .get_mut(&address)
                .unwrap()
                .peer
                .reliable_actor_bunch_datagrams(channel, payload)
                .unwrap();
            lab.send_all(&pending, address);
            lab.retry_frozen_movement_release();
            assert!(lab.peers[&address].frozen_release.pending);
            assert_eq!(lab.stats.frozen_movement_releases_sent, 0);
        }

        let close = client_close_bunch(&lab.peers[&address].peer, 14);
        // For the reliable-pawn case, the close packet NAKs that update.
        deliver_bunches_and_reply(&mut lab, &socket, &mut client, |_| !hold_pawn, vec![close]);
        lab.retry_frozen_movement_release();

        assert_eq!(lab.readiness_counts(), ReadinessCounts::default());
        let counts = lab.session_counts();
        assert_eq!(counts.active_peers, 0);
        assert_eq!(counts.closed_by_client, 1);
        assert_eq!(counts.connected_player_record_creations_acked, 1);
        assert_eq!(counts.connected_player_records_removed, 1);
        assert_eq!(counts.frozen_movement_releases_scheduled, 1);
        assert_eq!(counts.frozen_movement_releases_sent, 0);
        assert_eq!(counts.frozen_movement_release_errors, 0);
        assert_eq!(counts.frozen_movement_release_sends_deferred, 0);
        assert_eq!(
            counts.last_client_control_close,
            Some(ClientControlClose {
                reason: Some(14),
                phase: PlayerEntryPhase::Finalised,
                checkpoint_pawn_update_pending: false,
                own_record_ack: Some(record_ack),
                frozen_movement_release: Some(FrozenReleaseAtClose {
                    state,
                    sent: false,
                    pawn: Some(pawn),
                    relocation_held: false,
                    deferred: false,
                    service_pass_reached: Some(service_pass_reached),
                }),
            })
        );
    }
}

/// Off (the lab default) sends nothing; the release needs records.
#[test]
fn frozen_movement_release_off_sends_nothing_and_needs_records() {
    assert!(
        record_lab(false)
            .enable_frozen_movement_release(Duration::ZERO)
            .is_err()
    );
    assert!(
        record_lab(true)
            .enable_frozen_movement_release(Duration::from_millis(600_001))
            .is_err()
    );
    let mut lab = record_lab(true);
    assert_eq!(lab.frozen_movement_release_delay(), None);
    let (socket, address, mut client) = record_peer(&mut lab, 1, "AuthoredFrozenUser");
    complete_record_player(&mut lab, address);
    lab.controller_call(address, ControllerCall::FriendsList);
    lab.finalise_connected_players();
    let bunches = deliver_bunches(&mut lab, &socket, &mut client, |_| true);
    assert!(frozen_releases(&lab, &bunches).is_empty());
    assert_eq!(lab.stats.connected_player_record_creations_acked, 1);
    lab.retry_frozen_movement_release();
    assert!(deliver_bunches(&mut lab, &socket, &mut client, |_| true).is_empty());
    assert_eq!(lab.stats.frozen_movement_releases_scheduled, 0);
    assert_eq!(lab.stats.frozen_movement_releases_sent, 0);
    let [observation] = lab.frozen_movement_observations()[..] else {
        panic!("one session");
    };
    assert_eq!(observation.record_acked_ms, None);
}

/// A pawn channel closed before the release falls due abandons it.
#[test]
fn frozen_movement_release_is_abandoned_without_an_open_pawn_channel() {
    let mut lab = record_lab(true);
    lab.enable_frozen_movement_release(Duration::ZERO).unwrap();
    let (socket, address, mut client) = record_peer(&mut lab, 1, "AuthoredFrozenUser");
    complete_record_player(&mut lab, address);
    lab.controller_call(address, ControllerCall::FriendsList);
    lab.finalise_connected_players();
    // Hold the release back by hand, then lose the pawn.
    lab.frozen_movement_release = None;
    deliver_bunches(&mut lab, &socket, &mut client, |_| true);
    lab.frozen_movement_release = Some(Duration::ZERO);
    let entry = lab.peers.get_mut(&address).unwrap();
    entry.frozen_release.record_acked_at = Some(Instant::now());
    entry.frozen_release.due_at = Some(Instant::now());
    entry.controller_startup_pawn_guid = None;
    lab.retry_frozen_movement_release();
    assert_eq!(lab.stats.frozen_movement_releases_sent, 0);
    assert_eq!(lab.stats.frozen_movement_release_errors, 1);
    lab.retry_frozen_movement_release();
    assert_eq!(lab.stats.frozen_movement_release_errors, 1, "once");
    assert!(lab.frozen_movement_observations()[0].release_failed);
}

/// A due release, first copy or repair, waits while a reliable bunch on
/// the pawn channel is unACKed, as GameState updates do, and goes out
/// once that bunch is ACKed.
#[test]
fn frozen_movement_release_waits_for_the_pawn_channels_reliable_bunches() {
    let mut lab = record_lab(true);
    lab.enable_frozen_movement_release(Duration::ZERO).unwrap();
    let (socket, address, mut client) = record_peer(&mut lab, 1, "AuthoredFrozenUser");
    complete_record_player(&mut lab, address);
    deliver_bunches(&mut lab, &socket, &mut client, |_| true);
    lab.controller_call(address, ControllerCall::FriendsList);
    lab.finalise_connected_players();
    // A reliable pawn bunch in flight when the record's ACK arrives.
    let (plan, channel, _) = lab.pawn_open.as_ref().unwrap();
    let (plan, channel) = (plan.clone(), *channel);
    let payload =
        compose_pawn_frozen_movement_update(&plan, true, super::super::archive::ByteOrder::Little)
            .unwrap();
    let entry = lab.peers.get_mut(&address).unwrap();
    let held = entry
        .peer
        .reliable_actor_bunch_datagrams(channel, payload)
        .unwrap();
    lab.send_all(&held, address);
    // Deliver the record and lose the reliable pawn bunch.
    let pawn_packet = lab.peers[&address]
        .peer
        .sequencer()
        .notify()
        .sent_records()
        .len()
        - 1;
    deliver_bunches(&mut lab, &socket, &mut client, |index| index != pawn_packet);
    assert_eq!(lab.stats.frozen_movement_releases_scheduled, 1);
    lab.retry_frozen_movement_release();
    assert_eq!(lab.stats.frozen_movement_releases_sent, 0);
    assert_eq!(
        lab.readiness_counts().frozen_movement_release,
        FrozenMovementReleaseReadiness {
            pending_send: 1,
            ..Default::default()
        }
    );
    // The reliable layer repairs that bunch; once ACKed, the release goes.
    let repair = lab
        .peers
        .get_mut(&address)
        .unwrap()
        .peer
        .ack_packet()
        .unwrap();
    assert!(lab.send(&repair, address));
    deliver_bunches(&mut lab, &socket, &mut client, |_| true);
    lab.retry_frozen_movement_release();
    assert_eq!(lab.stats.frozen_movement_releases_sent, 1);
    let sent = deliver_bunches(&mut lab, &socket, &mut client, |_| true);
    assert_eq!(frozen_releases(&lab, &sent), [RepValue::Bool(false)]);
    assert_eq!(lab.stats.frozen_movement_releases_acked, 1);
    assert_eq!(lab.stats.frozen_movement_release_resends, 0);
}

/// Reachable only by hand: the real sequence opens the controller, pawn
/// and PlayerState before Begin and completion, so a finalised record
/// cannot lack them. Narrow check that opening them later re-triggers the
/// record, which is then sent once.
#[test]
fn connected_player_record_follows_when_its_actors_open() {
    use super::super::control_messages::{NetIdType, NetIdValue, UniqueNetId};
    let mut lab = record_lab(true);
    // GameState open; no controller, pawn or PlayerState yet.
    let (socket, address, mut client) = acked_game_state_peer(&mut lab);
    let entry = lab.peers.get_mut(&address).unwrap();
    entry.connection_id = Some(1);
    entry.player = Some(RegisteredPlayer {
        player_id: 1,
        name_units: vec![],
    });
    entry.login_unique_id = Some(UniqueNetId {
        kind: NetIdType::Named(ArchiveText::text("NULL").unwrap()),
        value: NetIdValue::Text(ArchiveText::text("AuthoredLateActors").unwrap()),
    });
    entry.profiles.completed = true;
    entry.profiles.friend_ids_ready = true;
    entry.profiles.character_slot = Some(3);
    entry.connected_player = Some(ConnectedPlayerRecord {
        index: 0,
        start_time: 1.0,
        complete_time: None,
    });
    lab.next_connected_player_record = 1;
    lab.finalise_connected_players();
    assert_eq!(lab.stats.connected_player_record_creations_sent, 0);
    assert!(!lab.peers[&address].connected_players_pending);
    assert_eq!(
        lab.readiness_counts().connected_player_records.pending_send,
        0
    );
    // The real opens: each commit marks this connection again.
    let game_state = lab.game_state_open.take();
    assert!(lab.send_player_controller_open(address));
    lab.game_state_open = game_state;
    assert!(lab.peers[&address].connected_players_pending);
    lab.retry_connected_player_records();
    assert_eq!(lab.stats.connected_player_record_creations_sent, 1);
    assert_eq!(lab.stats.connected_player_record_errors, 0);
    let bunches = deliver_bunches(&mut lab, &socket, &mut client, |_| true);
    let [body] = record_bodies(&lab, &bunches).try_into().unwrap();
    let plan = lab.connected_players.clone().unwrap();
    let values = super::super::rep_layout::parse_rep_properties(
        &body.blocks[1].payload,
        &plan.record_layout,
        &record_codec(),
    )
    .unwrap();
    let entry = &lab.peers[&address];
    let (controller, player_state) = entry.controller_open_guids.unwrap();
    let object = |guid| RepValue::Object(super::super::actor_channel::NetGuidRef::plain(guid));
    assert_eq!(values.entries[2].value, object(controller));
    assert_eq!(
        values.entries[3].value,
        object(entry.controller_startup_pawn_guid.unwrap())
    );
    assert_eq!(values.entries[4].value, object(player_state));
    lab.mark_connected_players_due(|_| true);
    lab.retry_connected_player_records();
    assert_eq!(lab.stats.connected_player_record_creations_sent, 1);
    assert_eq!(lab.stats.connected_player_record_creations_acked, 1);
}

/// A connection without a GameState is neither marked nor re-armed when
/// another player finalises; its later initial-open commits mark it, and
/// it then lists exactly its own record.
#[test]
fn connections_without_a_game_state_are_not_armed_until_it_commits() {
    let mut lab = record_lab(true);
    let (first_socket, first, mut first_client) = record_peer(&mut lab, 1, "AuthoredReady");
    complete_record_player(&mut lab, first);
    lab.controller_call(first, ControllerCall::FriendsList);
    // A second connection is still joining: handshake only.
    let second_socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    second_socket.connect(lab.local_addr().unwrap()).unwrap();
    second_socket
        .set_read_timeout(Some(Duration::from_secs(1)))
        .unwrap();
    let (second, mut second_client) = handshake_peer_on(&mut lab, &second_socket);
    lab.finalise_connected_players();
    assert_eq!(lab.stats.players_finalised, 1);
    assert_eq!(lab.stats.connected_player_record_creations_sent, 1);
    let joining = &lab.peers[&second];
    assert!(!joining.connected_players_pending);
    assert!(!joining.connected_players_retry_ready);
    assert_eq!(
        lab.readiness_counts().connected_player_records.pending_send,
        0
    );
    // Packet progress on the joining connection arms nothing.
    let probe = lab
        .peers
        .get_mut(&second)
        .unwrap()
        .peer
        .ack_packet()
        .unwrap();
    assert!(lab.send(&probe, second));
    deliver_bunches(&mut lab, &second_socket, &mut second_client, |_| true);
    assert!(!lab.peers[&second].connected_players_retry_ready);
    // It joins through the real initial opens: their commits (GameState
    // included) mark it; the first player's record is not visible there,
    // so nothing is sent and nothing stays pending.
    {
        use super::super::control_messages::{NetIdType, NetIdValue, UniqueNetId};
        let entry = lab.peers.get_mut(&second).unwrap();
        entry.connection_id = Some(2);
        entry.player = Some(RegisteredPlayer {
            player_id: 2,
            name_units: vec![],
        });
        entry.login_unique_id = Some(UniqueNetId {
            kind: NetIdType::Named(ArchiveText::text("NULL").unwrap()),
            value: NetIdValue::Text(ArchiveText::text("AuthoredLate").unwrap()),
        });
        entry.profiles.character_slot = Some(3);
    }
    assert!(lab.send_player_controller_open(second));
    assert!(lab.peers[&second].game_state.is_some());
    assert!(lab.peers[&second].connected_players_pending);
    lab.retry_connected_player_records();
    assert!(!lab.peers[&second].connected_players_pending);
    assert!(lab.peers[&second].connected_players_sent.is_empty());
    assert_eq!(lab.stats.connected_player_array_updates_sent, 1);
    ack_startup_packets(&mut lab, &second_socket, &mut second_client);
    // Once it finalises, it lists exactly its own record.
    complete_record_player(&mut lab, second);
    lab.controller_call(first, ControllerCall::FriendsList);
    lab.controller_call(second, ControllerCall::FriendsList);
    lab.finalise_connected_players();
    assert_eq!(lab.stats.players_finalised, 2);
    let second_index = lab.peers[&second].connected_player.as_ref().unwrap().index;
    let first_index = lab.peers[&first].connected_player.as_ref().unwrap().index;
    assert_eq!(lab.peers[&second].connected_players_sent, [second_index]);
    assert_eq!(lab.peers[&first].connected_players_sent, [first_index]);
    assert_eq!(lab.stats.connected_player_record_creations_sent, 2);
    assert_eq!(lab.stats.connected_player_array_updates_sent, 2);
    assert_eq!(
        lab.readiness_counts().connected_player_records.pending_send,
        0
    );
    deliver_bunches(&mut lab, &first_socket, &mut first_client, |_| true);
}

/// The same address rejoins through the cookie-replacement handshake: the
/// old record is removed, the new session registers one record and, once
/// finalised, sends it once under a new record GUID.
#[test]
fn reconnect_replaces_the_connected_player_record() {
    let mut lab = record_lab(true);
    let (socket, address, mut client) = record_peer(&mut lab, 1, "AuthoredReconnect");
    complete_record_player(&mut lab, address);
    lab.controller_call(address, ControllerCall::FriendsList);
    lab.finalise_connected_players();
    let bunches = deliver_bunches(&mut lab, &socket, &mut client, |_| true);
    let [body] = record_bodies(&lab, &bunches).try_into().unwrap();
    let BlockTarget::SubObject { object: old, .. } = &body.blocks[1].target else {
        panic!("record block");
    };
    let old_guid = old.guid;
    let old_index = lab.peers[&address].connected_player.as_ref().unwrap().index;
    let before = lab.stats.clone();
    assert_eq!(
        (
            before.connected_player_records_created,
            before.connected_player_record_creations_sent,
            before.connected_player_record_creations_acked,
            before.connected_player_records_removed,
        ),
        (1, 1, 1, 0)
    );
    // Rejoin from the same address: a new cookie replaces the peer.
    let (rejoined, mut client) = record_peer_on(&mut lab, &socket, 2, "AuthoredReconnect");
    assert_eq!(rejoined, address);
    assert_eq!(lab.stats.peers_replaced, 1);
    assert_eq!(lab.stats.connected_player_records_removed, 1);
    assert!(lab.peers[&address].connected_player.is_none());
    assert!(lab.peers[&address].connected_players_sent.is_empty());
    assert_eq!(
        lab.readiness_counts().connected_player_records,
        ConnectedPlayerRecordReadiness::default()
    );
    complete_record_player(&mut lab, address);
    assert_eq!(lab.stats.connected_player_records_created, 2);
    lab.controller_call(address, ControllerCall::FriendsList);
    lab.finalise_connected_players();
    assert_eq!(lab.stats.connected_player_record_creations_sent, 2);
    let new_index = lab.peers[&address].connected_player.as_ref().unwrap().index;
    assert_ne!(new_index, old_index);
    let bunches = deliver_bunches(&mut lab, &socket, &mut client, |_| true);
    let [body] = record_bodies(&lab, &bunches).try_into().unwrap();
    let BlockTarget::SubObject { object: new, .. } = &body.blocks[1].target else {
        panic!("record block");
    };
    assert_ne!(new.guid, old_guid);
    assert_eq!(
        listed_records(&lab, body.blocks.last().unwrap()),
        [new.guid]
    );
    assert_eq!(
        (
            lab.stats.connected_player_records_created,
            lab.stats.connected_player_record_creations_sent,
            lab.stats.connected_player_record_creations_acked,
            lab.stats.connected_player_records_removed,
            lab.stats.connected_player_array_updates_sent,
        ),
        (2, 2, 2, 1, 2)
    );
    assert_eq!(
        lab.readiness_counts().connected_player_records,
        ConnectedPlayerRecordReadiness {
            registered: 1,
            finalised: 1,
            creations_queued: 1,
            creations_settled: 1,
            ..Default::default()
        }
    );
}

/// Make the world time due on `address` and lift the update-pass rate
/// limit, so `update_game_state` sends exactly when the channel permits.
fn force_clock_due(lab: &mut PeerLab, address: SocketAddrV4) {
    use super::super::game_state_open::handle::WORLD_TIME_SECONDS;
    lab.game_state_open.as_mut().unwrap().updated_at = None;
    let entry = lab.peers.get_mut(&address).unwrap();
    entry
        .game_state
        .as_mut()
        .unwrap()
        .sent
        .remove(&WORLD_TIME_SECONDS);
}

/// Unreliable GameState updates wait for every reliable bunch on the
/// GameState channel (`actor_channel_settled`): while a record creation is
/// unacknowledged, including after a NAK until its repair is ACKed, no
/// clock update goes out; it resumes on the repair's ACK.
#[test]
fn clock_updates_pause_until_the_record_bunch_settles() {
    let mut lab = record_lab(true);
    let (socket, address, mut client) = record_peer(&mut lab, 1, "AuthoredClock");
    complete_record_player(&mut lab, address);
    lab.controller_call(address, ControllerCall::FriendsList);
    deliver_bunches(&mut lab, &socket, &mut client, |_| true);
    let channel = game_state_channel(&lab);
    force_clock_due(&mut lab, address);
    lab.update_game_state();
    assert_eq!(lab.stats.game_state_updates_sent, 1, "updates flow before");
    deliver_bunches(&mut lab, &socket, &mut client, |_| true);
    lab.finalise_connected_players();
    assert_eq!(lab.stats.connected_player_record_creations_sent, 1);
    assert!(!lab.peers[&address].peer.actor_channel_settled(channel));
    force_clock_due(&mut lab, address);
    lab.update_game_state();
    assert_eq!(lab.stats.game_state_updates_sent, 1, "paused while unacked");
    // Lose it: still paused until the repair is acknowledged.
    deliver_bunches(&mut lab, &socket, &mut client, |_| false);
    force_clock_due(&mut lab, address);
    lab.update_game_state();
    assert_eq!(lab.stats.game_state_updates_sent, 1, "paused after the NAK");
    let repair = lab
        .peers
        .get_mut(&address)
        .unwrap()
        .peer
        .ack_packet()
        .unwrap();
    assert!(lab.send(&repair, address));
    force_clock_due(&mut lab, address);
    lab.update_game_state();
    assert_eq!(lab.stats.game_state_updates_sent, 1, "paused during repair");
    deliver_bunches(&mut lab, &socket, &mut client, |_| true);
    assert!(lab.peers[&address].peer.actor_channel_settled(channel));
    force_clock_due(&mut lab, address);
    lab.update_game_state();
    assert_eq!(lab.stats.game_state_updates_sent, 2, "resumes on the ACK");
    assert_eq!(lab.stats.connected_player_record_creations_acked, 1);
}

#[test]
fn each_connection_lists_only_records_whose_actors_are_open_on_it() {
    let mut lab = record_lab(true);
    let (first_socket, first, mut first_client) = record_peer(&mut lab, 1, "AuthoredFirst");
    let (second_socket, second, mut second_client) = record_peer(&mut lab, 2, "AuthoredSecond");
    complete_record_player(&mut lab, first);
    complete_record_player(&mut lab, second);
    // Both Begins requeried every registered controller (F-1051).
    lab.controller_call(first, ControllerCall::FriendsList);
    lab.finalise_connected_players();
    assert_eq!(lab.stats.players_finalised, 0);
    lab.controller_call(second, ControllerCall::FriendsList);
    lab.finalise_connected_players();
    assert_eq!(lab.stats.players_finalised, 2);
    // One record per peer; each connection gets only its own, because the
    // other player's pawn and PlayerState are not open on it.
    assert_eq!(lab.stats.connected_player_records_created, 2);
    assert_eq!(lab.stats.connected_player_record_creations_sent, 2);
    let first_index = lab.peers[&first].connected_player.as_ref().unwrap().index;
    let second_index = lab.peers[&second].connected_player.as_ref().unwrap().index;
    assert_ne!(first_index, second_index);
    assert_eq!(lab.peers[&first].connected_players_sent, [first_index]);
    assert_eq!(lab.peers[&second].connected_players_sent, [second_index]);
    for (socket, client) in [
        (&first_socket, &mut first_client),
        (&second_socket, &mut second_client),
    ] {
        let bunches = deliver_bunches(&mut lab, socket, client, |_| true);
        let [body] = record_bodies(&lab, &bunches).try_into().unwrap();
        let component = body.blocks.last().unwrap();
        assert_eq!(listed_records(&lab, component).len(), 1);
    }
    // A later joiner registers its own record; existing connections'
    // arrays do not change, so nothing is sent to them.
    let (third_socket, third, mut third_client) = record_peer(&mut lab, 3, "AuthoredThird");
    complete_record_player(&mut lab, third);
    for address in [first, second, third] {
        lab.controller_call(address, ControllerCall::FriendsList);
    }
    lab.finalise_connected_players();
    assert_eq!(lab.stats.players_finalised, 3);
    assert_eq!(lab.stats.connected_player_record_creations_sent, 3);
    assert_eq!(lab.stats.connected_player_array_updates_sent, 3);
    let third_index = lab.peers[&third].connected_player.as_ref().unwrap().index;
    assert_eq!(lab.peers[&third].connected_players_sent, [third_index]);
    for (socket, client) in [
        (&first_socket, &mut first_client),
        (&second_socket, &mut second_client),
    ] {
        let bunches = deliver_bunches(&mut lab, socket, client, |_| true);
        assert!(record_bodies(&lab, &bunches).is_empty());
    }
    let bunches = deliver_bunches(&mut lab, &third_socket, &mut third_client, |_| true);
    assert_eq!(record_bodies(&lab, &bunches).len(), 1);
    // A connection that listed a departing record gets the shrunk array
    // (array only, no record block, no sub-object destroy); the others
    // get nothing. The lab's visibility rule never lists another
    // player's record today, so the listing is authored here.
    lab.peers.get_mut(&first).unwrap().connected_players_sent = vec![first_index, second_index];
    lab.remove_peer(second).unwrap();
    assert_eq!(lab.stats.connected_player_records_removed, 1);
    assert!(lab.peers[&first].connected_players_pending);
    assert!(!lab.peers[&third].connected_players_pending);
    lab.retry_connected_player_records();
    assert_eq!(lab.stats.connected_player_array_updates_sent, 4);
    assert_eq!(lab.stats.connected_player_record_creations_sent, 3);
    let bunches = deliver_bunches(&mut lab, &first_socket, &mut first_client, |_| true);
    let [body] = record_bodies(&lab, &bunches).try_into().unwrap();
    let [actor, component] = body.blocks.as_slice() else {
        panic!("actor and component blocks only");
    };
    assert_eq!(actor.target, BlockTarget::Actor);
    let first_guid = lab
        .guid_cache
        .clone()
        .subobject_guid(ActorKey::Session(0), first_index)
        .unwrap();
    assert_eq!(listed_records(&lab, component), [first_guid]);
    assert_eq!(lab.peers[&first].connected_players_sent, [first_index]);
    assert!(deliver_bunches(&mut lab, &third_socket, &mut third_client, |_| true).is_empty());
    // Removal drops the record from the gauges (the cookie-replacement
    // reconnect is `reconnect_replaces_the_connected_player_record`).
    lab.remove_peer(third).unwrap();
    assert_eq!(lab.stats.connected_player_records_removed, 2);
    assert_eq!(
        lab.readiness_counts().connected_player_records.registered,
        1
    );
}

#[test]
fn connected_player_record_pressure_retries_once_with_the_latest_state() {
    let mut lab = record_lab(true);
    let (socket, address, mut client) = record_peer(&mut lab, 1, "AuthoredPressure");
    complete_record_player(&mut lab, address);
    lab.controller_call(address, ControllerCall::FriendsList);
    deliver_bunches(&mut lab, &socket, &mut client, |_| true);
    let entry = lab.peers.get_mut(&address).unwrap();
    for _ in 0..entry.peer.config().sent_capacity {
        let datagram = entry.peer.ack_packet().unwrap();
        lab.socket.send_to(&datagram, address).unwrap();
        let mut bytes = [0; 2048];
        let n = socket.recv(&mut bytes).unwrap();
        let packet = super::super::packet::read_packet(
            &bytes[..n],
            &super::super::packet::PacketConfig {
                receiver: super::super::packet::Receiver::Client,
                ..lab.config.packet.packet
            },
        )
        .unwrap();
        let received = client.receive(&packet.header).unwrap().unwrap();
        client.ack_seq(received.sequence, true).unwrap();
    }
    lab.finalise_connected_players();
    assert_eq!(lab.stats.players_finalised, 1);
    assert_eq!(lab.stats.connected_player_record_sends_deferred, 1);
    assert_eq!(lab.stats.connected_player_record_creations_sent, 0);
    assert_eq!(
        lab.readiness_counts().connected_player_records.pending_send,
        1
    );
    // No packet progress: retries do not spin.
    for _ in 0..3 {
        lab.retry_connected_player_records();
    }
    assert_eq!(lab.stats.connected_player_record_sends_deferred, 1);
    // State changes while deferred are what the retry carries.
    lab.peers.get_mut(&address).unwrap().profiles.character_slot = Some(4);
    let bytes = super::super::packet::write_packet(
        &super::super::packet::Packet {
            magic: 0,
            header: client.header().unwrap(),
            info: None,
            bunches: vec![],
        },
        &lab.config.packet.packet,
    )
    .unwrap();
    client.commit().unwrap();
    socket.send(&bytes).unwrap();
    lab.poll();
    assert_eq!(lab.stats.connected_player_record_creations_sent, 1);
    assert_eq!(lab.stats.connected_player_record_sends_deferred, 1);
    assert_eq!(lab.stats.connected_player_record_errors, 0);
    assert_eq!(
        lab.readiness_counts().connected_player_records.pending_send,
        0
    );
    let bunches = deliver_bunches(&mut lab, &socket, &mut client, |_| true);
    let [body] = record_bodies(&lab, &bunches).try_into().unwrap();
    let plan = lab.connected_players.clone().unwrap();
    let values = super::super::rep_layout::parse_rep_properties(
        &body.blocks[1].payload,
        &plan.record_layout,
        &record_codec(),
    )
    .unwrap();
    assert_eq!(values.entries[1].value, RepValue::Int(4));
    lab.retry_connected_player_records();
    assert_eq!(lab.stats.connected_player_record_creations_sent, 1);
    assert_eq!(lab.stats.connected_player_record_creations_acked, 1);
}

#[test]
fn connected_player_record_nak_is_repaired_without_recomposition() {
    let mut lab = record_lab(true);
    let (socket, address, mut client) = record_peer(&mut lab, 1, "AuthoredRepair");
    complete_record_player(&mut lab, address);
    lab.controller_call(address, ControllerCall::FriendsList);
    deliver_bunches(&mut lab, &socket, &mut client, |_| true);
    lab.finalise_connected_players();
    assert_eq!(lab.stats.connected_player_record_creations_sent, 1);
    // Lose every datagram of the creation.
    let lost = deliver_bunches(&mut lab, &socket, &mut client, |_| false);
    let [original] = record_bodies(&lab, &lost).try_into().unwrap();
    assert!(lab.readiness_counts().resend_bunches_pending > 0);
    assert_eq!(lab.stats.connected_player_record_creations_acked, 0);
    assert_eq!(
        lab.readiness_counts()
            .connected_player_records
            .records_awaiting_ack,
        1
    );
    for _ in 0..3 {
        lab.mark_connected_players_due(|_| true);
        lab.retry_connected_player_records();
    }
    assert_eq!(lab.stats.connected_player_record_creations_sent, 1);
    assert_eq!(lab.stats.connected_player_array_updates_sent, 1);
    // The reliable layer resends the same bunch unchanged.
    let repair = lab
        .peers
        .get_mut(&address)
        .unwrap()
        .peer
        .ack_packet()
        .unwrap();
    assert!(lab.send(&repair, address));
    let repaired = deliver_bunches(&mut lab, &socket, &mut client, |_| true);
    let [resent] = record_bodies(&lab, &repaired).try_into().unwrap();
    assert_eq!(resent, original);
    assert_eq!(lab.readiness_counts().resend_bunches_pending, 0);
    assert_eq!(lab.stats.connected_player_record_creations_acked, 1);
    // A later ACK does not count the creation again.
    let more = lab
        .peers
        .get_mut(&address)
        .unwrap()
        .peer
        .ack_packet()
        .unwrap();
    assert!(lab.send(&more, address));
    deliver_bunches(&mut lab, &socket, &mut client, |_| true);
    assert_eq!(lab.stats.connected_player_record_creations_acked, 1);
    assert_eq!(lab.stats.connected_player_record_creations_sent, 1);
    assert_eq!(
        lab.readiness_counts().connected_player_records,
        ConnectedPlayerRecordReadiness {
            registered: 1,
            finalised: 1,
            creations_queued: 1,
            creations_settled: 1,
            ..Default::default()
        }
    );
}

/// Every bunch the client receives in one authored join-to-finalisation
/// run, by channel; GameState payloads, which carry the lab clock, by
/// length only.
fn record_scenario(mode: Option<bool>) -> Vec<(u32, bool, bool, bool, usize, Vec<u8>)> {
    let mut lab = record_lab(mode.is_some());
    if mode == Some(false) {
        lab.disable_connected_player_records().unwrap();
    }
    let channel = game_state_channel(&lab);
    let (socket, address, mut client) = record_peer(&mut lab, 1, "AuthoredScenario");
    let mut bunches = Vec::new();
    complete_record_player(&mut lab, address);
    bunches.extend(deliver_bunches(&mut lab, &socket, &mut client, |_| true));
    lab.controller_call(address, ControllerCall::FriendsList);
    lab.finalise_connected_players();
    lab.retry_connected_player_records();
    bunches.extend(deliver_bunches(&mut lab, &socket, &mut client, |_| true));
    bunches
        .into_iter()
        .map(|bunch| {
            let header = &bunch.header;
            let bytes = if header.channel_index == channel {
                Vec::new()
            } else {
                bunch.payload.bytes().to_vec()
            };
            (
                header.channel_index,
                header.reliable,
                header.open,
                header.has_package_map_exports,
                bunch.payload.bit_len(),
                bytes,
            )
        })
        .collect()
}

/// Two sequential record sessions on one lab, the first removed before
/// the second begins: each session's bunches (GameState-channel bunches
/// by shape, block targets and listed records, since their world times
/// vary) and the GUIDs it was given.
fn sequential_record_sessions() -> Vec<(Vec<String>, u32, u32)> {
    let mut lab = record_lab(true);
    let channel = game_state_channel(&lab);
    let mut sessions = Vec::new();
    for (connection, user) in [(1, "AuthoredFirst"), (2, "AuthoredSecond")] {
        let (socket, address, mut client) = record_peer(&mut lab, connection, user);
        let mut bunches = Vec::new();
        complete_record_player(&mut lab, address);
        bunches.extend(deliver_bunches(&mut lab, &socket, &mut client, |_| true));
        lab.controller_call(address, ControllerCall::FriendsList);
        lab.finalise_connected_players();
        lab.retry_connected_player_records();
        bunches.extend(deliver_bunches(&mut lab, &socket, &mut client, |_| true));
        assert_eq!(lab.peers[&address].connected_player_creations_settled, 1);
        let record = record_bodies(&lab, &bunches)
            .into_iter()
            .flat_map(|body| body.blocks)
            .find_map(|block| match block.target {
                BlockTarget::SubObject { object, .. } => Some(object.guid),
                BlockTarget::Actor => None,
            })
            .unwrap();
        let pawn = lab.peers[&address].controller_startup_pawn_guid.unwrap();
        let shapes = bunches
            .iter()
            .map(|bunch| {
                let header = &bunch.header;
                let content = if header.channel_index != channel {
                    format!("{:02x?}", bunch.payload.bytes())
                } else if header.reliable && !header.open && !header.has_package_map_exports {
                    let body = super::super::actor_channel::read_actor_bunch_body(
                        &mut bunch.payload.reader(),
                        false,
                        &actor_codec(17, super::super::archive::ByteOrder::Little),
                    )
                    .unwrap();
                    let listed = body
                        .blocks
                        .last()
                        .map(|block| listed_records(&lab, block))
                        .unwrap_or_default();
                    let targets: Vec<_> = body
                        .blocks
                        .iter()
                        .map(|block| match &block.target {
                            BlockTarget::Actor => None,
                            BlockTarget::SubObject { object, .. } => Some(object.guid),
                        })
                        .collect();
                    format!("{targets:?} {listed:?}")
                } else {
                    String::new()
                };
                format!(
                    "{} {} {} {} {} {content}",
                    header.channel_index,
                    header.reliable,
                    header.open,
                    header.has_package_map_exports,
                    bunch.payload.bit_len()
                )
            })
            .collect();
        sessions.push((shapes, record, pawn));
        lab.remove_peer(address).unwrap();
    }
    sessions
}

/// Releasing a departed session's cached identities changes no bunch
/// of the first session, nor of a session after a departure: the
/// digests were recorded before the release was introduced. GUIDs are
/// never reused across sessions.
#[test]
fn sequential_sessions_send_the_same_bunches_after_identity_release() {
    let sessions = sequential_record_sessions();
    assert_eq!(sessions, sequential_record_sessions(), "deterministic");
    let digests: Vec<String> = sessions
        .iter()
        .map(|(shapes, ..)| {
            let mut hasher = Sha256::new();
            for shape in shapes {
                hasher.update(shape.as_bytes());
                hasher.update(b"\n");
            }
            hasher
                .finalize()
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect()
        })
        .collect();
    // Recorded at c946063, before departed identities were released.
    assert_eq!(
        digests,
        [
            "8ac0fb3576ddeb09435ccbe9d43e66ba15e35ff922dd1735e055acc047ed5c32",
            "9a18acaf7b90592cc21d8efe0f0b5b0d49cfff3e0c038a2e5e82844fa928870d",
        ]
    );
    let [
        (_, first_record, first_pawn),
        (_, second_record, second_pawn),
    ] = sessions.as_slice()
    else {
        panic!("two sessions");
    };
    assert_ne!(first_record, second_record);
    assert_ne!(first_pawn, second_pawn);
    assert!(second_record > first_record && second_pawn > first_pawn);
}

#[test]
fn disabled_connected_player_records_send_exactly_the_previous_bunches() {
    let before = record_scenario(None);
    let disabled = record_scenario(Some(false));
    assert!(!before.is_empty());
    assert_eq!(disabled, before);
    // Enabled, the run differs only by the GameState-channel record
    // bunches: every other bunch is unchanged and in the same order.
    let enabled = record_scenario(Some(true));
    let channel = game_state_channel(&record_lab(false));
    let (records, others): (Vec<_>, Vec<_>) = enabled
        .into_iter()
        .partition(|bunch| bunch.0 == channel && bunch.1 && !bunch.2);
    assert_eq!(others, before);
    assert_eq!(
        records.iter().map(|bunch| bunch.3).collect::<Vec<_>>(),
        [true, false]
    );
    let mut lab = record_lab(true);
    let (_socket, address, _client) = record_peer(&mut lab, 1, "AuthoredLate");
    assert!(lab.disable_connected_player_records().is_err());
    assert!(
        lab.enable_connected_player_records(
            ReleaseProfile::fixture("3.0.35")
                .connected_players_plan()
                .unwrap()
                .unwrap()
        )
        .is_err()
    );
    assert!(lab.has_connected_player_records());
    let _ = address;
}

#[test]
fn connected_player_records_require_the_referenced_opens_and_a_valid_plan() {
    let plan = ReleaseProfile::fixture("3.0.35")
        .connected_players_plan()
        .unwrap()
        .unwrap();
    // The GameState and controller, but no pawn or PlayerState open.
    assert!(
        prospect_refresh_lab()
            .enable_connected_player_records(plan.clone())
            .is_err()
    );
    let mut lab = record_lab(false);
    let mut bad = plan.clone();
    bad.record_class[0].checksum = 0;
    assert!(lab.enable_connected_player_records(bad).is_err());
    let mut bad = plan.clone();
    bad.record_layout.pop();
    assert!(lab.enable_connected_player_records(bad).is_err());
    assert!(!lab.has_connected_player_records());
    lab.enable_connected_player_records(plan).unwrap();
    assert!(lab.has_connected_player_records());
}

/// [`record_lab`] with controller possession properties on.
fn possession_lab(records: bool) -> PeerLab {
    let mut lab = record_lab(records);
    lab.enable_controller_possession_properties().unwrap();
    lab
}

/// A joined peer like [`record_peer`] on a fresh loopback socket, before
/// any opens: the caller drives the initial opens.
fn possession_identity_peer(
    lab: &mut PeerLab,
    connection: u32,
    user: &str,
) -> (
    UdpSocket,
    SocketAddrV4,
    super::super::sequencing::PacketSequencer,
) {
    use super::super::control_messages::{NetIdType, NetIdValue, UniqueNetId};
    let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    socket.connect(lab.local_addr().unwrap()).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(1)))
        .unwrap();
    let (address, client) = handshake_peer_on(lab, &socket);
    let entry = lab.peers.get_mut(&address).unwrap();
    entry.connection_id = Some(connection);
    entry.player = Some(RegisteredPlayer {
        player_id: connection as i32,
        name_units: "AUTHORED-POSSESSION".encode_utf16().collect(),
    });
    entry.login_unique_id = Some(UniqueNetId {
        kind: NetIdType::Named(ArchiveText::text("NULL").unwrap()),
        value: NetIdValue::Text(ArchiveText::text(user).unwrap()),
    });
    entry.profiles.character_slot = Some(3);
    (socket, address, client)
}

/// [`record_peer`], returning every bunch of its real initial-open
/// sequence (all delivered and ACKed).
fn possession_peer(
    lab: &mut PeerLab,
    connection: u32,
    user: &str,
) -> (
    UdpSocket,
    SocketAddrV4,
    super::super::sequencing::PacketSequencer,
    Vec<super::super::packet::Bunch>,
) {
    let (socket, address, mut client) = possession_identity_peer(lab, connection, user);
    assert!(lab.send_player_controller_open(address));
    let bunches = deliver_bunches(lab, &socket, &mut client, |_| true);
    (socket, address, client, bunches)
}

/// Whether `bunch` has the possession update's framing: a whole,
/// reliable, nonopening controller-channel bunch without exports. During
/// the initial opens no other controller bunch has it.
fn possession_shaped(bunch: &super::super::packet::Bunch) -> bool {
    let header = &bunch.header;
    header.channel_index == super::super::session_open::PLAYER_CONTROLLER_CHANNEL_INDEX
        && header.reliable
        && !header.open
        && !header.partial
        && !header.has_package_map_exports
}

/// The possession updates among `bunches`: their positions and their
/// decoded `(handle, value)` entries, read with the repository decoder
/// against the controller open's own layout. Other controller bunches
/// (RPC fields, no property block) are skipped.
fn possession_updates(
    lab: &PeerLab,
    bunches: &[super::super::packet::Bunch],
) -> Vec<(usize, Vec<(u32, RepValue)>)> {
    let plan = lab.player_controller_open.as_ref().unwrap();
    let codec = RepLayoutCodec {
        actor: actor_codec(
            plan.engine_net_version,
            super::super::archive::ByteOrder::Little,
        ),
        checksums: false,
        limits: Default::default(),
    };
    bunches
        .iter()
        .enumerate()
        .filter(|(_, bunch)| possession_shaped(bunch))
        .filter_map(|(index, bunch)| {
            let mut reader = bunch.payload.reader();
            let body = super::super::actor_channel::read_actor_bunch_body(
                &mut reader,
                false,
                &codec.actor,
            )
            .unwrap();
            assert!(reader.at_end());
            let [block] = body.blocks.as_slice() else {
                return None;
            };
            if !block.has_rep_layout {
                return None;
            }
            assert!(body.spawn.is_none());
            assert_eq!(block.target, BlockTarget::Actor);
            let stream = super::super::rep_layout::parse_rep_properties(
                &block.payload,
                &plan.actor_layout,
                &codec,
            )
            .unwrap();
            Some((
                index,
                stream
                    .entries
                    .into_iter()
                    .map(|entry| (entry.handle, entry.value))
                    .collect(),
            ))
        })
        .collect()
}

fn possession_entries(pawn: u32) -> Vec<(u32, RepValue)> {
    let reference = RepValue::Object(super::super::actor_channel::NetGuidRef::plain(pawn));
    vec![(17, reference.clone()), (20, reference)]
}

fn pawn_open_position(bunches: &[super::super::packet::Bunch]) -> usize {
    bunches
        .iter()
        .position(|bunch| {
            bunch.header.channel_index == super::super::pawn_open::PAWN_CHANNEL_INDEX
                && bunch.header.open
        })
        .unwrap()
}

/// The real initial opens send one update right after the pawn open,
/// naming the pawn as handles 17 and 20 of the survival controller's
/// layout; it re-encodes exactly, is ACKed once and is never repeated
/// by finalisation, the record or later passes.
#[test]
fn controller_possession_update_follows_the_pawn_open_once_and_decodes() {
    let mut lab = possession_lab(true);
    let (socket, address, mut client, bunches) = possession_peer(&mut lab, 1, "AuthoredPossession");
    let pawn = lab.peers[&address].controller_startup_pawn_guid.unwrap();
    let [(position, entries)] = possession_updates(&lab, &bunches).try_into().unwrap();
    assert_eq!(entries, possession_entries(pawn));
    assert!(position > pawn_open_position(&bunches));
    // GameState and PlayerState opens follow it.
    let game_state = game_state_channel(&lab);
    assert!(
        bunches[position..]
            .iter()
            .any(|bunch| bunch.header.channel_index == game_state && bunch.header.open)
    );
    // The survival controller's 30-handle layout; exact re-encoding.
    let plan = lab.player_controller_open.clone().unwrap();
    assert_eq!(plan.actor_layout.len(), 30);
    let codec = RepLayoutCodec {
        actor: actor_codec(
            plan.engine_net_version,
            super::super::archive::ByteOrder::Little,
        ),
        checksums: false,
        limits: Default::default(),
    };
    let composed = super::super::session_open::compose_controller_possession_update(
        pawn,
        &plan.actor_layout,
        &codec,
    )
    .unwrap();
    assert_eq!(composed.bytes(), bunches[position].payload.bytes());
    assert_eq!(composed.bit_len(), bunches[position].payload.bit_len());
    let body = super::super::actor_channel::read_actor_bunch_body(
        &mut composed.reader(),
        false,
        &codec.actor,
    )
    .unwrap();
    let reencoded = encode_actor_bunch_body(&body, &codec.actor).unwrap();
    assert_eq!(reencoded.bytes(), composed.bytes());
    assert_eq!(lab.stats.controller_possession_updates_sent, 1);
    assert_eq!(lab.stats.controller_possession_updates_acked, 1);
    assert_eq!(lab.stats.controller_possession_sends_deferred, 0);
    assert_eq!(lab.stats.controller_possession_errors, 0);
    let settled = ControllerPossessionReadiness {
        queued: 1,
        settled: 1,
        ..Default::default()
    };
    assert_eq!(lab.readiness_counts().controller_possession, settled);
    // Begin, completion, finalisation and the record add none.
    complete_record_player(&mut lab, address);
    lab.controller_call(address, ControllerCall::FriendsList);
    lab.finalise_connected_players();
    lab.controller_possession_due(address);
    lab.retry_controller_possession();
    let later = deliver_bunches(&mut lab, &socket, &mut client, |_| true);
    assert!(possession_updates(&lab, &later).is_empty());
    assert_eq!(record_bodies(&lab, &later).len(), 1);
    assert_eq!(lab.stats.connected_player_record_creations_sent, 1);
    assert_eq!(lab.stats.controller_possession_updates_sent, 1);
    assert_eq!(lab.stats.controller_possession_updates_acked, 1);
    let counts = lab.session_counts();
    assert_eq!(counts.controller_possession_updates_sent, 1);
    assert_eq!(counts.controller_possession_updates_acked, 1);
    assert_eq!(counts.controller_possession_sends_deferred, 0);
    assert_eq!(counts.controller_possession_errors, 0);
    lab.remove_peer(address).unwrap();
    assert_eq!(
        lab.readiness_counts().controller_possession,
        ControllerPossessionReadiness::default()
    );
}

/// Nothing is due before the pawn open commits; a pawn that opens later
/// is followed by the update then.
#[test]
fn controller_possession_waits_for_the_pawn_open() {
    let mut lab = possession_lab(false);
    let (socket, address, mut client) = possession_identity_peer(&mut lab, 1, "AuthoredLatePawn");
    assert!(matches!(
        lab.send_initial_controller_open(address),
        InitialOpenOutcome::Committed(_)
    ));
    lab.retry_controller_possession();
    let opened = deliver_bunches(&mut lab, &socket, &mut client, |_| true);
    assert!(possession_updates(&lab, &opened).is_empty());
    assert_eq!(lab.stats.controller_possession_updates_sent, 0);
    assert_eq!(
        lab.readiness_counts().controller_possession,
        ControllerPossessionReadiness::default()
    );
    let (controller, player_state) = lab.peers[&address].controller_open_guids.unwrap();
    assert!(matches!(
        lab.send_pawn_open(address, controller, player_state),
        InitialOpenOutcome::Committed(_)
    ));
    assert_eq!(lab.stats.controller_possession_updates_sent, 1);
    let bunches = deliver_bunches(&mut lab, &socket, &mut client, |_| true);
    let pawn = lab.peers[&address].controller_startup_pawn_guid.unwrap();
    let [(position, entries)] = possession_updates(&lab, &bunches).try_into().unwrap();
    assert_eq!(entries, possession_entries(pawn));
    assert!(position > pawn_open_position(&bunches));
    assert_eq!(lab.stats.controller_possession_updates_acked, 1);
}

/// A finalised player whose controller possession update is held back by
/// genuine transport pressure, so its connected-player record is held
/// too. The real opens run stage by stage (controller, GameState,
/// PlayerState); Begin, both initialisation answers and the friends list
/// arrive before the pawn opens. Then the controller channel's reliable
/// window is filled with authored bunches whose packets the client NAKs:
/// they stay unacknowledged (the window is full) while the sent ring
/// empties. The real pawn open then commits, the update is refused as
/// drainable window pressure and defers, the PlayerState completion goes
/// out on its own channel, and finalisation makes the record due.
///
/// Sent-ring pressure cannot produce this state: completion needs a ring
/// slot as well, and every pass admits the update before completion.
fn held_possession_peer(
    lab: &mut PeerLab,
    user: &str,
) -> (
    UdpSocket,
    SocketAddrV4,
    super::super::sequencing::PacketSequencer,
) {
    use super::super::packet_lab::MAX_PENDING_RELIABLE;
    let (socket, address, mut client) = possession_identity_peer(lab, 1, user);
    assert!(matches!(
        lab.send_initial_controller_open(address),
        InitialOpenOutcome::Committed(_)
    ));
    assert!(matches!(
        lab.send_game_state_open(address),
        InitialOpenOutcome::Committed(_)
    ));
    let (controller, player_state) = lab.peers[&address].controller_open_guids.unwrap();
    assert!(matches!(
        lab.send_player_state_open(address, controller, player_state),
        InitialOpenOutcome::Committed(_)
    ));
    ack_startup_packets(lab, &socket, &mut client);
    lab.controller_call(address, ControllerCall::Begin(3));
    let entry = lab.peers.get_mut(&address).unwrap();
    entry.profiles.outstanding = None;
    entry.profiles.has_loadout = true;
    entry.profiles.has_bestiary = true;
    lab.controller_call(address, ControllerCall::FriendsList);
    deliver_bunches(lab, &socket, &mut client, |_| true);
    assert!(!lab.peers[&address].profiles.completed, "no pawn yet");
    let channel = lab.peers[&address].player_controller_channel.unwrap();
    let ring = |lab: &PeerLab| {
        lab.peers[&address]
            .peer
            .sequencer()
            .notify()
            .sent_records()
            .len()
    };
    for _ in 0..2 * MAX_PENDING_RELIABLE {
        let entry = lab.peers.get_mut(&address).unwrap();
        if entry.peer.pending_actor_reliable(channel) == Some(MAX_PENDING_RELIABLE) {
            break;
        }
        let datagram = entry
            .peer
            .reliable_actor_bunch(
                channel,
                Payload::from_writer(super::super::packet::BitWriter::new()),
            )
            .unwrap();
        assert!(lab.send(&datagram, address));
        if ring(lab) >= 32 {
            notify_startup_packets(lab, &socket, &mut client, |_| false);
        }
    }
    if ring(lab) > 0 {
        notify_startup_packets(lab, &socket, &mut client, |_| false);
    }
    assert_eq!(
        lab.peers[&address].peer.pending_actor_reliable(channel),
        Some(MAX_PENDING_RELIABLE)
    );
    assert_eq!(ring(lab), 0);
    assert!(matches!(
        lab.send_pawn_open(address, controller, player_state),
        InitialOpenOutcome::Committed(_)
    ));
    assert_eq!(lab.stats.controller_possession_sends_deferred, 1);
    assert_eq!(lab.stats.controller_possession_updates_sent, 0);
    assert!(
        lab.peers[&address].profiles.completed,
        "completion went out"
    );
    lab.finalise_connected_players();
    assert_eq!(lab.stats.players_finalised, 1);
    assert_eq!(lab.stats.connected_player_record_creations_sent, 0);
    assert_eq!(
        lab.readiness_counts().controller_possession,
        ControllerPossessionReadiness {
            pending_send: 1,
            ..Default::default()
        }
    );
    (socket, address, client)
}

/// ACK progress for [`held_possession_peer`]: the client acknowledges
/// everything outstanding and the reliable layer repairs the NAKed
/// controller bunches until the controller window has room. Only packet
/// notifications are processed; no retry pass runs.
fn drain_controller_window(
    lab: &mut PeerLab,
    socket: &UdpSocket,
    client: &mut super::super::sequencing::PacketSequencer,
    address: SocketAddrV4,
) {
    use super::super::packet_lab::MAX_PENDING_RELIABLE;
    let channel = lab.peers[&address].player_controller_channel.unwrap();
    deliver_bunches(lab, socket, client, |_| true);
    for _ in 0..MAX_PENDING_RELIABLE {
        if lab.peers[&address].peer.pending_actor_reliable(channel) < Some(MAX_PENDING_RELIABLE) {
            assert!(lab.peers[&address].controller_possession_retry_ready);
            return;
        }
        let repair = lab
            .peers
            .get_mut(&address)
            .unwrap()
            .peer
            .ack_packet()
            .unwrap();
        assert!(lab.send(&repair, address));
        deliver_bunches(lab, socket, client, |_| true);
    }
    panic!("the controller window did not drain");
}

/// The real sequence with records and possession properties on: the
/// update defers at the pawn open under genuine window pressure, the
/// player completes and finalises meanwhile and its record is held;
/// passes without packet progress admit nothing; ACK progress then
/// admits the update, and the record follows once the client has ACKed
/// the update (F-1116), in that order on the wire, each exactly once.
#[test]
fn the_record_follows_a_possession_update_held_by_transport_pressure() {
    let mut lab = possession_lab(true);
    let (socket, address, mut client) = held_possession_peer(&mut lab, "AuthoredHeld");
    for _ in 0..3 {
        lab.retry_controller_possession();
        lab.finalise_connected_players();
        lab.retry_connected_player_records();
    }
    assert_eq!(lab.stats.controller_possession_sends_deferred, 1);
    assert_eq!(lab.stats.controller_possession_updates_sent, 0);
    assert_eq!(lab.stats.connected_player_record_creations_sent, 0);
    drain_controller_window(&mut lab, &socket, &mut client, address);
    assert_eq!(
        lab.stats.controller_possession_updates_sent, 0,
        "no pass yet"
    );
    lab.poll();
    assert_eq!(lab.stats.controller_possession_updates_sent, 1);
    assert_eq!(
        lab.stats.connected_player_record_creations_sent, 0,
        "admitted, not yet ACKed"
    );
    // The client applies the controller channel in order, so the NAKed
    // filler bunches ahead of the update must be repaired and ACKed too.
    let mut bunches = deliver_bunches(&mut lab, &socket, &mut client, |_| true);
    for _ in 0..super::super::packet_lab::MAX_PENDING_RELIABLE {
        lab.poll();
        if lab.stats.connected_player_record_creations_sent > 0 {
            break;
        }
        let repair = lab
            .peers
            .get_mut(&address)
            .unwrap()
            .peer
            .ack_packet()
            .unwrap();
        assert!(lab.send(&repair, address));
        bunches.extend(deliver_bunches(&mut lab, &socket, &mut client, |_| true));
    }
    assert_eq!(lab.stats.connected_player_record_creations_sent, 1);
    assert_eq!(lab.stats.connected_player_array_updates_sent, 1);
    bunches.extend(deliver_bunches(&mut lab, &socket, &mut client, |_| true));
    let pawn = lab.peers[&address].controller_startup_pawn_guid.unwrap();
    let [(update, entries)] = possession_updates(&lab, &bunches).try_into().unwrap();
    assert_eq!(entries, possession_entries(pawn));
    let channel = game_state_channel(&lab);
    let record = bunches
        .iter()
        .position(|bunch| {
            bunch.header.channel_index == channel && bunch.header.reliable && !bunch.header.open
        })
        .unwrap();
    assert!(update < record, "the update leaves before the record");
    assert_eq!(record_bodies(&lab, &bunches).len(), 1);
    // Later passes add nothing; each settled exactly once.
    for _ in 0..3 {
        lab.poll();
    }
    deliver_bunches(&mut lab, &socket, &mut client, |_| true);
    assert_eq!(
        (
            lab.stats.controller_possession_updates_sent,
            lab.stats.controller_possession_updates_acked,
            lab.stats.controller_possession_sends_deferred,
            lab.stats.controller_possession_errors,
        ),
        (1, 1, 1, 0)
    );
    assert_eq!(
        (
            lab.stats.connected_player_record_creations_sent,
            lab.stats.connected_player_record_creations_acked,
            lab.stats.connected_player_array_updates_sent,
            lab.stats.connected_player_record_errors,
        ),
        (1, 1, 1, 0)
    );
    assert_eq!(
        lab.readiness_counts().controller_possession,
        ControllerPossessionReadiness {
            queued: 1,
            settled: 1,
            ..Default::default()
        }
    );
}

/// Abandonment because the controller or pawn channel is gone when the
/// deferred update comes due again. The deferral and the ACK progress are
/// real; the close is driven on the connection's transport
/// (`LabPeer::reliable_actor_close`), because the session never closes
/// its own controller or pawn channel through any public call, so this
/// branch is not reachable through the public sequence. The error counts
/// once and the update is never retried. Its record is never sent: the
/// session is closed and removed in the same poll (F-1116), with its
/// record, and no record error is counted.
#[test]
fn an_update_whose_channel_closed_is_abandoned_once() {
    for close_pawn in [true, false] {
        let mut lab = possession_lab(true);
        let (socket, address, mut client) = held_possession_peer(&mut lab, "AuthoredClosed");
        drain_controller_window(&mut lab, &socket, &mut client, address);
        let entry = lab.peers.get_mut(&address).unwrap();
        let channel = if close_pawn {
            super::super::pawn_open::PAWN_CHANNEL_INDEX
        } else {
            entry.player_controller_channel.unwrap()
        };
        let close = entry.peer.reliable_actor_close(channel, 1).unwrap();
        assert!(lab.send(&close, address));
        lab.poll();
        assert_eq!(
            lab.stats.controller_possession_errors, 1,
            "pawn={close_pawn}"
        );
        assert!(!lab.has_peer(address), "pawn={close_pawn}");
        assert_eq!(lab.stats.peers_closed_by_possession_abandoned, 1);
        assert_eq!(lab.session_counts().closed_by_possession_abandoned, 1);
        assert_eq!(lab.stats.connected_player_records_removed, 1);
        for _ in 0..3 {
            lab.poll();
            lab.controller_possession_due(address);
        }
        assert_eq!(
            (
                lab.stats.controller_possession_updates_sent,
                lab.stats.controller_possession_sends_deferred,
                lab.stats.controller_possession_errors,
                lab.stats.peers_closed_by_possession_abandoned,
            ),
            (0, 1, 1, 1),
            "pawn={close_pawn}"
        );
        assert_eq!(lab.stats.connected_player_record_creations_sent, 0);
        assert_eq!(lab.stats.connected_player_record_errors, 0);
        assert_eq!(lab.readiness_counts(), ReadinessCounts::default());
    }
}

/// Abandonment by a refusal that cannot drain while the peer stays
/// usable, here a composition refusal. A lab's controller plan is fixed
/// before admission and composes for every pawn GUID, so this cannot
/// happen through the public sequence: the plan's layout is truncated by
/// hand after the real deferral. The error counts once, the update is
/// never retried, the held record is never sent, and the session is
/// removed in the same pass (F-1116). As for an actor channel failure,
/// a control close goes out only to a client that has sent a control
/// bunch; this authored client has not, so none does.
#[test]
fn an_update_that_cannot_be_composed_is_abandoned_and_closes_the_session() {
    use super::super::packet::{PacketConfig, Receiver, read_packet};
    let mut lab = possession_lab(true);
    let (socket, address, mut client) = held_possession_peer(&mut lab, "AuthoredRefused");
    lab.player_controller_open
        .as_mut()
        .unwrap()
        .actor_layout
        .truncate(16);
    drain_controller_window(&mut lab, &socket, &mut client, address);
    lab.poll();
    assert_eq!(lab.stats.controller_possession_errors, 1);
    assert_eq!(lab.stats.controller_possession_updates_sent, 0);
    assert_eq!(lab.stats.connected_player_record_creations_sent, 0);
    assert!(!lab.has_peer(address));
    assert_eq!(lab.stats.peers_closed_by_possession_abandoned, 1);
    assert_eq!(lab.stats.connected_player_records_removed, 1);
    // Every datagram was sent before `poll` returned, so a nonblocking
    // read drains them all: neither an update nor a record nor a close.
    let config = PacketConfig {
        receiver: Receiver::Client,
        ..lab.config.packet.packet
    };
    socket.set_nonblocking(true).unwrap();
    let mut bunches = Vec::new();
    let mut bytes = [0; 2048];
    while let Ok(length) = socket.recv(&mut bytes) {
        bunches.extend(read_packet(&bytes[..length], &config).unwrap().bunches);
    }
    assert!(!bunches.iter().any(|bunch| bunch.header.close));
    assert!(possession_updates(&lab, &bunches).is_empty());
    assert!(record_bodies(&lab, &bunches).is_empty());
    for _ in 0..3 {
        lab.poll();
        lab.controller_possession_due(address);
    }
    assert_eq!(
        (
            lab.stats.controller_possession_updates_sent,
            lab.stats.controller_possession_sends_deferred,
            lab.stats.controller_possession_errors,
            lab.stats.peers_failed,
            lab.stats.peers_closed_by_possession_abandoned,
        ),
        (0, 1, 1, 0, 1)
    );
    assert_eq!(lab.stats.connected_player_record_creations_sent, 0);
    assert_eq!(lab.readiness_counts(), ReadinessCounts::default());
}

/// Abandonment on a failed transport, which also removes the peer. Not
/// reachable through the public sequence: every call that fails a
/// connection's transport (a receive error, a send-side invariant)
/// removes that peer at once, so a deferred update never meets a failed
/// transport. The test-only `LabPeer::fail_for_test` fails it between
/// passes after the real deferral and ACK progress. The error counts
/// once, the peer is removed as failed with its record, and nothing is
/// sent.
#[test]
fn an_update_on_a_failed_transport_is_abandoned_and_removes_the_peer() {
    let mut lab = possession_lab(true);
    let (socket, address, mut client) = held_possession_peer(&mut lab, "AuthoredFailed");
    drain_controller_window(&mut lab, &socket, &mut client, address);
    lab.peers.get_mut(&address).unwrap().peer.fail_for_test();
    lab.retry_controller_possession();
    assert_eq!(lab.stats.controller_possession_errors, 1);
    assert_eq!(lab.stats.peers_failed, 1);
    assert!(!lab.has_peer(address));
    assert_eq!(lab.stats.connected_player_records_removed, 1);
    for _ in 0..3 {
        lab.retry_controller_possession();
        lab.poll();
    }
    assert_eq!(
        (
            lab.stats.controller_possession_updates_sent,
            lab.stats.controller_possession_errors,
            lab.stats.connected_player_record_creations_sent,
        ),
        (0, 1, 0)
    );
    assert_eq!(lab.readiness_counts(), ReadinessCounts::default());
}

/// Datagrams the real pawn open takes after an ACKed controller open.
fn pawn_open_datagrams() -> usize {
    let mut lab = record_lab(false);
    let (socket, address, mut client) = possession_identity_peer(&mut lab, 1, "AuthoredPressure");
    assert!(matches!(
        lab.send_initial_controller_open(address),
        InitialOpenOutcome::Committed(_)
    ));
    ack_startup_packets(&mut lab, &socket, &mut client);
    let ring = |lab: &PeerLab| {
        lab.peers[&address]
            .peer
            .sequencer()
            .notify()
            .sent_records()
            .len()
    };
    let before = ring(&lab);
    let (controller, player_state) = lab.peers[&address].controller_open_guids.unwrap();
    assert!(matches!(
        lab.send_pawn_open(address, controller, player_state),
        InitialOpenOutcome::Committed(_)
    ));
    ring(&lab) - before
}

/// The sent ring fills right behind the pawn open: the update defers,
/// does not spin without packet progress, and after an ACK goes out
/// once, composed from the current pawn reference.
#[test]
fn controller_possession_pressure_retries_once_with_the_current_pawn() {
    use super::super::packet::{Packet, PacketConfig, Receiver, read_packet, write_packet};
    let pawn_datagrams = pawn_open_datagrams();
    let mut lab = possession_lab(false);
    let (socket, address, mut client) = possession_identity_peer(&mut lab, 1, "AuthoredPressure");
    assert!(matches!(
        lab.send_initial_controller_open(address),
        InitialOpenOutcome::Committed(_)
    ));
    ack_startup_packets(&mut lab, &socket, &mut client);
    let config = PacketConfig {
        receiver: Receiver::Client,
        ..lab.config.packet.packet
    };
    let entry = lab.peers.get_mut(&address).unwrap();
    let free =
        entry.peer.config().sent_capacity - entry.peer.sequencer().notify().sent_records().len();
    for _ in 0..free - pawn_datagrams {
        let entry = lab.peers.get_mut(&address).unwrap();
        let datagram = entry.peer.ack_packet().unwrap();
        lab.socket.send_to(&datagram, address).unwrap();
        let mut bytes = [0; 2048];
        let length = socket.recv(&mut bytes).unwrap();
        let packet = read_packet(&bytes[..length], &config).unwrap();
        let received = client.receive(&packet.header).unwrap().unwrap();
        client.ack_seq(received.sequence, true).unwrap();
    }
    let (controller, player_state) = lab.peers[&address].controller_open_guids.unwrap();
    assert!(matches!(
        lab.send_pawn_open(address, controller, player_state),
        InitialOpenOutcome::Committed(_)
    ));
    assert_eq!(lab.stats.controller_possession_sends_deferred, 1);
    assert_eq!(lab.stats.controller_possession_updates_sent, 0);
    assert_eq!(
        lab.readiness_counts().controller_possession,
        ControllerPossessionReadiness {
            pending_send: 1,
            ..Default::default()
        }
    );
    for _ in 0..3 {
        lab.retry_controller_possession();
    }
    assert_eq!(lab.stats.controller_possession_sends_deferred, 1);
    // The client's ACK of the filler packets releases the ring.
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
    socket.send(&bytes).unwrap();
    lab.poll();
    assert_eq!(lab.stats.controller_possession_updates_sent, 1);
    assert_eq!(lab.stats.controller_possession_sends_deferred, 1);
    assert_eq!(lab.stats.controller_possession_errors, 0);
    assert_eq!(lab.readiness_counts().controller_possession.pending_send, 0);
    let bunches = deliver_bunches(&mut lab, &socket, &mut client, |_| true);
    let pawn = lab.peers[&address].controller_startup_pawn_guid.unwrap();
    let [(position, entries)] = possession_updates(&lab, &bunches).try_into().unwrap();
    assert_eq!(entries, possession_entries(pawn));
    assert!(position > pawn_open_position(&bunches));
    lab.retry_controller_possession();
    assert_eq!(lab.stats.controller_possession_updates_sent, 1);
    assert_eq!(lab.stats.controller_possession_updates_acked, 1);
}

/// Read every outstanding datagram of `address`, ACKing or NAKing each
/// as `delivered` says of its bunches, and return the bunches in order.
fn deliver_bunches_by_content(
    lab: &mut PeerLab,
    socket: &UdpSocket,
    client: &mut super::super::sequencing::PacketSequencer,
    delivered: impl Fn(&[super::super::packet::Bunch]) -> bool,
) -> Vec<super::super::packet::Bunch> {
    use super::super::packet::{Packet, Receiver, read_packet, write_packet};
    let SocketAddr::V4(address) = socket.local_addr().unwrap() else {
        unreachable!()
    };
    let count = lab.peers[&address]
        .peer
        .sequencer()
        .notify()
        .sent_records()
        .len();
    let config = super::super::packet::PacketConfig {
        receiver: Receiver::Client,
        ..lab.config.packet.packet
    };
    let mut bunches = Vec::new();
    for _ in 0..count {
        let mut bytes = [0; 2048];
        let length = socket.recv(&mut bytes).unwrap();
        let packet = read_packet(&bytes[..length], &config).unwrap();
        let received = client.receive(&packet.header).unwrap().unwrap();
        client
            .ack_seq(received.sequence, delivered(&packet.bunches))
            .unwrap();
        bunches.extend(packet.bunches);
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
    receive_authored_at(lab, socket, &bytes, Instant::now());
    bunches
}

#[test]
fn controller_possession_nak_is_repaired_without_recomposition() {
    let mut lab = possession_lab(false);
    let (socket, address, mut client) = possession_identity_peer(&mut lab, 1, "AuthoredNak");
    assert!(lab.send_player_controller_open(address));
    let lost = deliver_bunches_by_content(&mut lab, &socket, &mut client, |bunches| {
        !bunches.iter().any(possession_shaped)
    });
    let [(position, entries)] = possession_updates(&lab, &lost).try_into().unwrap();
    let original = lost[position].payload.clone();
    let pawn = lab.peers[&address].controller_startup_pawn_guid.unwrap();
    assert_eq!(entries, possession_entries(pawn));
    assert_eq!(lab.stats.controller_possession_updates_acked, 0);
    assert!(lab.readiness_counts().resend_bunches_pending > 0);
    assert_eq!(
        lab.readiness_counts().controller_possession,
        ControllerPossessionReadiness {
            queued: 1,
            awaiting_ack: 1,
            ..Default::default()
        }
    );
    // The reliable layer resends the stored bunch unchanged.
    let repair = lab
        .peers
        .get_mut(&address)
        .unwrap()
        .peer
        .ack_packet()
        .unwrap();
    assert!(lab.send(&repair, address));
    let repaired = deliver_bunches(&mut lab, &socket, &mut client, |_| true);
    let [(position, _)] = possession_updates(&lab, &repaired).try_into().unwrap();
    assert_eq!(repaired[position].payload.bytes(), original.bytes());
    assert_eq!(repaired[position].payload.bit_len(), original.bit_len());
    assert_eq!(lab.stats.controller_possession_updates_sent, 1);
    assert_eq!(lab.stats.controller_possession_updates_acked, 1);
    // A later ACK does not count it again.
    let more = lab
        .peers
        .get_mut(&address)
        .unwrap()
        .peer
        .ack_packet()
        .unwrap();
    assert!(lab.send(&more, address));
    deliver_bunches(&mut lab, &socket, &mut client, |_| true);
    assert_eq!(lab.stats.controller_possession_updates_acked, 1);
    assert_eq!(
        lab.readiness_counts().controller_possession,
        ControllerPossessionReadiness {
            queued: 1,
            settled: 1,
            ..Default::default()
        }
    );
}

#[test]
fn a_reconnect_gets_its_own_possession_update() {
    let mut lab = possession_lab(true);
    let (socket, address, _client) = record_peer(&mut lab, 1, "AuthoredReconnect");
    let first = lab.peers[&address].controller_startup_pawn_guid.unwrap();
    assert_eq!(lab.stats.controller_possession_updates_acked, 1);
    let (rejoined, mut client) = record_peer_on(&mut lab, &socket, 2, "AuthoredReconnect");
    assert_eq!(rejoined, address);
    assert_eq!(lab.stats.peers_replaced, 1);
    let second = lab.peers[&address].controller_startup_pawn_guid.unwrap();
    assert_ne!(first, second);
    assert_eq!(lab.stats.controller_possession_updates_sent, 2);
    assert_eq!(lab.stats.controller_possession_updates_acked, 2);
    assert_eq!(
        lab.readiness_counts().controller_possession,
        ControllerPossessionReadiness {
            queued: 1,
            settled: 1,
            ..Default::default()
        }
    );
    // Nothing more on later passes.
    lab.controller_possession_due(address);
    lab.retry_controller_possession();
    assert!(deliver_bunches(&mut lab, &socket, &mut client, |_| true).is_empty());
    assert_eq!(lab.stats.controller_possession_updates_sent, 2);
}

/// Leaving at each stage (controller only, update held back, admitted
/// and unacknowledged, settled) leaves no possession state or work.
#[test]
fn possession_state_leaves_with_its_session_at_every_stage() {
    for stage in 0..4 {
        let mut lab = possession_lab(true);
        let (socket, address, mut client) = possession_identity_peer(&mut lab, 1, "AuthoredLeave");
        assert!(matches!(
            lab.send_initial_controller_open(address),
            InitialOpenOutcome::Committed(_)
        ));
        if stage > 0 {
            let (controller, player_state) = lab.peers[&address].controller_open_guids.unwrap();
            assert!(matches!(
                lab.send_pawn_open(address, controller, player_state),
                InitialOpenOutcome::Committed(_)
            ));
        }
        match stage {
            1 => {
                let entry = lab.peers.get_mut(&address).unwrap();
                entry.controller_possession_receipt = InitialisationReceipt::default();
                entry.controller_possession_pending = true;
            }
            3 => {
                deliver_bunches(&mut lab, &socket, &mut client, |_| true);
            }
            _ => {}
        }
        let expected_settled = u64::from(stage == 3);
        assert_eq!(
            lab.stats.controller_possession_updates_acked,
            expected_settled
        );
        let sent = lab.stats.controller_possession_updates_sent;
        assert_eq!(sent, u64::from(stage > 0), "stage {stage}");
        lab.remove_peer(address).unwrap();
        assert_eq!(
            lab.readiness_counts().controller_possession,
            ControllerPossessionReadiness::default(),
            "stage {stage}"
        );
        lab.retry_controller_possession();
        lab.controller_possession_due(address);
        assert_eq!(lab.stats.controller_possession_updates_sent, sent);
        assert_eq!(
            lab.stats.controller_possession_updates_acked,
            expected_settled
        );
        assert_eq!(lab.stats.controller_possession_errors, 0);
    }
}

/// Every bunch the client receives from Join to the record's ACK, and
/// the same by channel for comparison: GameState payloads, which carry
/// the lab clock, by length only. `None` never touches the switch;
/// `Some(false)` turns it on, then off.
#[allow(clippy::type_complexity)]
fn possession_scenario(
    mode: Option<bool>,
) -> (
    Vec<super::super::packet::Bunch>,
    Vec<(u32, bool, bool, bool, usize, Vec<u8>)>,
) {
    let mut lab = record_lab(true);
    match mode {
        Some(true) => lab.enable_controller_possession_properties().unwrap(),
        Some(false) => {
            lab.enable_controller_possession_properties().unwrap();
            lab.disable_controller_possession_properties().unwrap();
        }
        None => {}
    }
    assert_eq!(
        lab.has_controller_possession_properties(),
        mode == Some(true)
    );
    let channel = game_state_channel(&lab);
    let (socket, address, mut client, mut bunches) =
        possession_peer(&mut lab, 1, "AuthoredScenario");
    complete_record_player(&mut lab, address);
    bunches.extend(deliver_bunches(&mut lab, &socket, &mut client, |_| true));
    lab.controller_call(address, ControllerCall::FriendsList);
    lab.finalise_connected_players();
    lab.retry_connected_player_records();
    bunches.extend(deliver_bunches(&mut lab, &socket, &mut client, |_| true));
    assert_eq!(lab.stats.connected_player_record_creations_acked, 1);
    let shapes = bunches
        .iter()
        .map(|bunch| {
            let header = &bunch.header;
            let bytes = if header.channel_index == channel {
                Vec::new()
            } else {
                bunch.payload.bytes().to_vec()
            };
            (
                header.channel_index,
                header.reliable,
                header.open,
                header.has_package_map_exports,
                bunch.payload.bit_len(),
                bytes,
            )
        })
        .collect();
    (bunches, shapes)
}

/// Within one build: never touched equals turned on then off, and on adds
/// exactly the one update. It cannot see a change to shared code; the
/// cross-commit pin is `switch_off_sessions_send_the_parent_commits_bunches`.
#[test]
fn disabled_controller_possession_sends_exactly_the_previous_bunches() {
    let (_, before) = possession_scenario(None);
    let (_, disabled) = possession_scenario(Some(false));
    assert!(!before.is_empty());
    assert_eq!(disabled, before);
    // Enabled, the run differs only by the one possession update: every
    // other bunch is unchanged and in the same order.
    let (bunches, mut enabled) = possession_scenario(Some(true));
    assert_eq!(enabled.len(), before.len() + 1);
    let extra = enabled
        .iter()
        .zip(&before)
        .position(|(enabled, before)| enabled != before)
        .unwrap_or(before.len());
    enabled.remove(extra);
    assert_eq!(enabled, before);
    let lab = record_lab(false);
    let updates = possession_updates(&lab, &bunches[extra..=extra]);
    let [(0, entries)] = updates.as_slice() else {
        panic!("the extra bunch is the possession update");
    };
    let handles: Vec<u32> = entries.iter().map(|(handle, _)| *handle).collect();
    assert_eq!(handles, [17, 20]);
    assert!(extra > pawn_open_position(&bunches));
    // Configuration is fixed once a peer exists.
    let mut lab = possession_lab(false);
    let (_socket, _address, _client) = record_peer(&mut lab, 1, "AuthoredLate");
    assert!(lab.disable_controller_possession_properties().is_err());
    assert!(lab.enable_controller_possession_properties().is_err());
    assert!(lab.has_controller_possession_properties());
}

#[test]
fn controller_possession_requires_the_controller_and_pawn_opens() {
    let mut lab = PeerLab::bind(0, PeerLabConfig::default()).unwrap();
    assert!(lab.enable_controller_possession_properties().is_err());
    // The GameState and controller, but no pawn open.
    let mut lab = prospect_refresh_lab();
    assert!(lab.enable_controller_possession_properties().is_err());
    assert!(!lab.has_controller_possession_properties());
    let mut lab = record_lab(false);
    lab.enable_controller_possession_properties().unwrap();
    assert!(lab.has_controller_possession_properties());
}

/// Two sequential sessions on `lab`, each from the stateless handshake
/// and Join to its connected-player record's ACK, the first removed
/// before the second joins: every bunch the client receives, digested
/// per session. Whole bunches off the GameState channel count by their
/// exact bytes; GameState-channel bunches, which carry the lab clock,
/// count by header flags, length and block targets. Uses only behaviour
/// that predates the controller possession switch.
fn switch_off_session_digests(lab: &mut PeerLab) -> Vec<String> {
    use super::super::control_messages::{NetIdType, NetIdValue, UniqueNetId};
    let channel = game_state_channel(lab);
    let codec = actor_codec(17, super::super::archive::ByteOrder::Little);
    let mut digests = Vec::new();
    for (connection, user) in [(1u32, "AuthoredDigestFirst"), (2, "AuthoredDigestSecond")] {
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
            name_units: "AUTHORED-DIGEST".encode_utf16().collect(),
        });
        entry.login_unique_id = Some(UniqueNetId {
            kind: NetIdType::Named(ArchiveText::text("NULL").unwrap()),
            value: NetIdValue::Text(ArchiveText::text(user).unwrap()),
        });
        entry.profiles.character_slot = Some(3);
        assert!(lab.send_player_controller_open(address));
        let mut bunches = deliver_bunches(lab, &socket, &mut client, |_| true);
        complete_record_player(lab, address);
        bunches.extend(deliver_bunches(lab, &socket, &mut client, |_| true));
        lab.controller_call(address, ControllerCall::FriendsList);
        lab.finalise_connected_players();
        lab.retry_connected_player_records();
        bunches.extend(deliver_bunches(lab, &socket, &mut client, |_| true));
        assert_eq!(lab.peers[&address].connected_player_creations_settled, 1);
        let mut hasher = Sha256::new();
        for bunch in &bunches {
            let header = &bunch.header;
            let content = if header.channel_index != channel {
                format!("{:02x?}", bunch.payload.bytes())
            } else if header.reliable
                && !header.open
                && !header.partial
                && !header.has_package_map_exports
            {
                let body = super::super::actor_channel::read_actor_bunch_body(
                    &mut bunch.payload.reader(),
                    false,
                    &codec,
                )
                .unwrap();
                let targets: Vec<_> = body
                    .blocks
                    .iter()
                    .map(|block| match &block.target {
                        BlockTarget::Actor => None,
                        BlockTarget::SubObject { object, .. } => Some(object.guid),
                    })
                    .collect();
                format!("{targets:?}")
            } else {
                String::new()
            };
            hasher.update(
                format!(
                    "{} {} {} {} {} {} {content}\n",
                    header.channel_index,
                    header.reliable,
                    header.open,
                    header.partial,
                    header.has_package_map_exports,
                    bunch.payload.bit_len()
                )
                .as_bytes(),
            );
        }
        digests.push(
            hasher
                .finalize()
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect(),
        );
        lab.remove_peer(address).unwrap();
    }
    digests
}

/// The production bind's lab with records on (the default).
fn production_digest_lab(config: crate::config::NetworkConfig) -> PeerLab {
    crate::network::bind(&config).unwrap().unwrap().lab
}

/// The switch-off path pinned to the parent of the controller possession
/// change. These digests were produced by [`switch_off_session_digests`]
/// on a build of 68b6325's `src/` (the commit before the switch existed,
/// the same helper and fixtures appended to its tests), not by this
/// build. A lab that never touches the switch, one that turns it on and
/// then off, and the production bind with
/// `controller_possession_properties = false` must send exactly those
/// bunches; with the switch on every session differs.
#[test]
fn switch_off_sessions_send_the_parent_commits_bunches() {
    const LAB: [&str; 2] = [
        "da3f7bbda7f9e3c4b8f4183c6ccd55fc6ab259dc1487fbf838d9a29a2862d963",
        "9aed6aef271f5c71155cf76e5693a22f3006b10dc12271c32b95ca7331b272ef",
    ];
    const PRODUCTION: [&str; 2] = [
        "cafb88babd75ea37f94944a890596a7d6d1e8566b9a61278fb471031411bde5c",
        "4d6c55a1fba2f27e51d71df5b9201a4116d2a8ae3322fdba1022e24c97e5b205",
    ];
    let mut untouched = record_lab(true);
    assert!(!untouched.has_controller_possession_properties());
    assert_eq!(switch_off_session_digests(&mut untouched), LAB);
    let mut toggled = record_lab(true);
    toggled.enable_controller_possession_properties().unwrap();
    toggled.disable_controller_possession_properties().unwrap();
    assert_eq!(switch_off_session_digests(&mut toggled), LAB);
    let config = crate::config::NetworkConfig {
        enabled: true,
        port: 0,
        release_profile: Some(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("config/release.toml"),
        ),
        controller_possession_properties: false,
        ..crate::config::NetworkConfig::default()
    };
    let mut production = production_digest_lab(config.clone());
    assert!(!production.has_controller_possession_properties());
    assert!(production.has_connected_player_records());
    assert_eq!(switch_off_session_digests(&mut production), PRODUCTION);
    let on = switch_off_session_digests(&mut possession_lab(true));
    assert!(on.iter().zip(LAB).all(|(on, off)| on != off));
    let mut production_on = production_digest_lab(crate::config::NetworkConfig {
        controller_possession_properties: true,
        ..config
    });
    assert!(production_on.has_controller_possession_properties());
    let on = switch_off_session_digests(&mut production_on);
    assert!(on.iter().zip(PRODUCTION).all(|(on, off)| on != off));
}

fn queue_initialisation_phase(lab: &mut PeerLab, address: SocketAddrV4, completion: bool) {
    if completion {
        let entry = lab.peers.get_mut(&address).unwrap();
        entry.profiles.has_loadout = true;
        entry.profiles.has_bestiary = true;
        lab.complete_player_initialisation(address);
        assert!(lab.peers[&address].profiles.completed);
    } else {
        lab.request_initialisation(address);
        assert!(lab.peers[&address].profiles.initialisation_requested);
    }
}

fn phase_transport_counts(completion: bool, settled: bool) -> InitialisationTransportCounts {
    if completion {
        InitialisationTransportCounts {
            completions_queued: 1,
            completions_settled: usize::from(settled),
            completions_awaiting_ack: usize::from(!settled),
            ..Default::default()
        }
    } else {
        InitialisationTransportCounts {
            requests_queued: 1,
            requests_settled: usize::from(settled),
            requests_awaiting_ack: usize::from(!settled),
            ..Default::default()
        }
    }
}

#[test]
fn initialisation_transport_receipts_require_ack_and_survive_loss_repair() {
    for completion in [false, true] {
        for mixed in [false, true] {
            let (mut lab, socket, address, mut client) = initialisation_transport_peer();
            queue_initialisation_phase(&mut lab, address, completion);
            let queued = phase_transport_counts(completion, false);
            assert_eq!(lab.readiness_counts().initialisation_transport, queued);
            if mixed {
                let packet = lab
                    .peers
                    .get_mut(&address)
                    .unwrap()
                    .peer
                    .ack_packet()
                    .unwrap();
                assert!(lab.send(&packet, address));
            }
            notify_startup_packets(&mut lab, &socket, &mut client, |index| mixed && index > 0);
            assert_eq!(lab.readiness_counts().initialisation_transport, queued);
            assert!(lab.readiness_counts().resend_bunches_pending > 0);
            let updates = lab.stats.player_character_id_updates_sent;
            let requests = lab.stats.loadout_requests_sent;
            for _ in 0..3 {
                lab.retry_pending_profile_exchanges();
                assert_eq!(lab.readiness_counts().initialisation_transport, queued);
            }
            let repair = lab
                .peers
                .get_mut(&address)
                .unwrap()
                .peer
                .ack_packet()
                .unwrap();
            assert!(lab.send(&repair, address));
            ack_startup_packets(&mut lab, &socket, &mut client);
            assert_eq!(
                lab.readiness_counts().initialisation_transport,
                phase_transport_counts(completion, true)
            );
            assert_eq!(lab.stats.player_character_id_updates_sent, updates);
            assert_eq!(lab.stats.loadout_requests_sent, requests);
            lab.remove_peer(address).unwrap();
            assert_eq!(lab.readiness_counts(), ReadinessCounts::default());
            let (_replacement, _, _) = acked_game_state_peer(&mut lab);
            assert_eq!(
                lab.readiness_counts().initialisation_transport,
                InitialisationTransportCounts::default()
            );
        }
    }
}

#[test]
fn initialisation_transport_receipts_ignore_later_same_channel_traffic() {
    for completion in [false, true] {
        let (mut lab, socket, address, mut client) = initialisation_transport_peer();
        queue_initialisation_phase(&mut lab, address, completion);
        let phase_packets = lab.peers[&address]
            .peer
            .sequencer()
            .notify()
            .sent_records()
            .len();
        let entry = lab.peers.get_mut(&address).unwrap();
        let channel = if completion {
            entry.player_state_channel
        } else {
            entry.player_controller_channel
        }
        .unwrap();
        let later = entry
            .peer
            .reliable_actor_bunch_datagrams(
                channel,
                Payload::from_writer(super::super::packet::BitWriter::new()),
            )
            .unwrap();
        assert!(lab.send_all(&later, address));
        notify_startup_packets(&mut lab, &socket, &mut client, |index| {
            index < phase_packets
        });
        assert!(!lab.peers[&address].peer.actor_channel_settled(channel));
        let settled = phase_transport_counts(completion, true);
        assert_eq!(lab.readiness_counts().initialisation_transport, settled);
        let repair = lab
            .peers
            .get_mut(&address)
            .unwrap()
            .peer
            .ack_packet()
            .unwrap();
        assert!(lab.send(&repair, address));
        ack_startup_packets(&mut lab, &socket, &mut client);
        assert_eq!(lab.readiness_counts().initialisation_transport, settled);
    }
}

#[test]
fn initialisation_transport_receipts_recognise_acked_span_behind_an_older_gap() {
    for completion in [false, true] {
        let (mut lab, socket, address, mut client) = initialisation_transport_peer();
        let entry = lab.peers.get_mut(&address).unwrap();
        let channel = if completion {
            entry.player_state_channel
        } else {
            entry.player_controller_channel
        }
        .unwrap();
        let older = entry
            .peer
            .reliable_actor_bunch_datagrams(
                channel,
                Payload::from_writer(super::super::packet::BitWriter::new()),
            )
            .unwrap();
        let older_packets = older.packet_ids.len();
        assert!(lab.send_all(&older, address));
        queue_initialisation_phase(&mut lab, address, completion);
        notify_startup_packets(&mut lab, &socket, &mut client, |index| {
            index >= older_packets
        });
        assert!(!lab.peers[&address].peer.actor_channel_settled(channel));
        assert_eq!(
            lab.readiness_counts().initialisation_transport,
            phase_transport_counts(completion, true)
        );
    }
}

#[test]
fn initialisation_transport_phases_settle_independently_of_finalisation() {
    let (mut lab, socket, address, mut client) = initialisation_transport_peer();
    queue_initialisation_phase(&mut lab, address, false);
    let request_packets = lab.peers[&address]
        .peer
        .sequencer()
        .notify()
        .sent_records()
        .len();
    queue_initialisation_phase(&mut lab, address, true);
    lab.peers
        .get_mut(&address)
        .unwrap()
        .profiles
        .friend_ids_ready = true;
    lab.finalise_connected_players();
    assert!(lab.peers[&address].profiles.finalised);
    assert_eq!(
        lab.readiness_counts().initialisation_transport,
        InitialisationTransportCounts {
            requests_queued: 1,
            requests_awaiting_ack: 1,
            completions_queued: 1,
            completions_awaiting_ack: 1,
            ..Default::default()
        }
    );
    notify_startup_packets(&mut lab, &socket, &mut client, |index| {
        index < request_packets
    });
    assert_eq!(
        lab.readiness_counts().initialisation_transport,
        InitialisationTransportCounts {
            requests_queued: 1,
            requests_settled: 1,
            completions_queued: 1,
            completions_awaiting_ack: 1,
            ..Default::default()
        }
    );
    let repair = lab
        .peers
        .get_mut(&address)
        .unwrap()
        .peer
        .ack_packet()
        .unwrap();
    assert!(lab.send(&repair, address));
    ack_startup_packets(&mut lab, &socket, &mut client);
    assert_eq!(
        lab.readiness_counts().initialisation_transport,
        InitialisationTransportCounts {
            requests_queued: 1,
            requests_settled: 1,
            completions_queued: 1,
            completions_settled: 1,
            ..Default::default()
        }
    );
    assert!(lab.peers[&address].profiles.finalised);
}

#[test]
fn initialisation_transport_close_and_channel_reuse_cannot_settle_old_receipts() {
    for completion in [false, true] {
        let (mut lab, socket, address, mut client) = initialisation_transport_peer();
        queue_initialisation_phase(&mut lab, address, completion);
        let entry = lab.peers.get_mut(&address).unwrap();
        let channel = if completion {
            entry.player_state_channel
        } else {
            entry.player_controller_channel
        }
        .unwrap();
        let close = entry.peer.reliable_actor_close(channel, 0).unwrap();
        assert!(lab.send(&close, address));
        ack_startup_packets(&mut lab, &socket, &mut client);
        let queued = phase_transport_counts(completion, false);
        assert_eq!(lab.readiness_counts().initialisation_transport, queued);
        assert!(!lab.peers[&address].peer.has_actor_channel(channel));
        let mut config = lab.config.packet.packet;
        config.receiver = super::super::packet::Receiver::Client;
        let mut reopened = super::super::packet::read_packet(&close, &config)
            .unwrap()
            .bunches
            .pop()
            .unwrap();
        reopened.header.open = true;
        reopened.header.close = false;
        reopened.header.close_reason = None;
        reopened.header.reliable_sequence_raw = None;
        let sent = lab
            .peers
            .get_mut(&address)
            .unwrap()
            .peer
            .reliable_actor_open_datagrams(&reopened, &[])
            .unwrap();
        assert!(lab.send_all(&sent, address));
        ack_startup_packets(&mut lab, &socket, &mut client);
        assert!(lab.peers[&address].peer.actor_channel_settled(channel));
        assert_eq!(lab.readiness_counts().initialisation_transport, queued);
    }
}

#[test]
fn initialisation_transport_split_span_waits_for_repair_across_wire_sequence_wrap() {
    use super::super::{game_state_open::handle, sequencing::wire_reliable_sequence};
    let (mut lab, socket, address, mut client) = initialisation_transport_peer();
    let channel = lab.peers[&address].player_controller_channel.unwrap();
    // Channels start at the cookie-derived outgoing packet ID & 0x3ff, and
    // the lab draws a random cookie secret, so the start is random: drive
    // to the next sequence whose wire value is 1022, never to absolute
    // 1022, which setup alone may already have passed.
    let start = lab.peers[&address]
        .peer
        .actor_reliable_sequence(channel)
        .unwrap();
    let target = start + (1022 + 1024 - wire_reliable_sequence(start)) % 1024;
    assert_eq!(wire_reliable_sequence(target), 1022);
    while lab.peers[&address]
        .peer
        .actor_reliable_sequence(channel)
        .unwrap()
        < target
    {
        let previous = lab.peers[&address]
            .peer
            .actor_reliable_sequence(channel)
            .unwrap();
        for _ in 0..(target - previous).min(32) {
            let sent = lab
                .peers
                .get_mut(&address)
                .unwrap()
                .peer
                .reliable_actor_bunch_datagrams(
                    channel,
                    Payload::from_writer(super::super::packet::BitWriter::new()),
                )
                .unwrap();
            assert!(lab.send_all(&sent, address));
        }
        ack_startup_packets(&mut lab, &socket, &mut client);
    }
    lab.game_state_open.as_mut().unwrap().state.values.insert(
        handle::ACTIVE_PROSPECT_FIRST,
        RepValue::Str(ArchiveText::text(&"G".repeat(1500)).unwrap()),
    );
    lab.request_initialisation(address);
    let receipt = &lab.peers[&address].initialisation_request_receipt;
    assert_eq!(receipt.first, target + 1);
    assert_eq!(wire_reliable_sequence(receipt.first), 1023);
    assert!(receipt.last > receipt.first);
    assert!(wire_reliable_sequence(receipt.last) < wire_reliable_sequence(receipt.first));
    notify_startup_packets(&mut lab, &socket, &mut client, |index| index > 0);
    assert_eq!(
        lab.readiness_counts().initialisation_transport,
        phase_transport_counts(false, false)
    );
    let repair = lab
        .peers
        .get_mut(&address)
        .unwrap()
        .peer
        .ack_packet()
        .unwrap();
    assert!(lab.send(&repair, address));
    ack_startup_packets(&mut lab, &socket, &mut client);
    assert_eq!(
        lab.readiness_counts().initialisation_transport,
        phase_transport_counts(false, true)
    );
}

#[test]
fn readiness_snapshot_aggregates_multiple_peers_and_clears_on_shutdown() {
    let mut lab = prospect_refresh_lab();
    let (_first, _, _) = acked_game_state_peer(&mut lab);
    let (_second, _, _) = acked_game_state_peer(&mut lab);
    let counts = lab.readiness_counts();
    assert_eq!(counts.actor_channels_open, 2);
    assert_eq!(counts.actor_channels_reliable_settled, 2);
    assert_eq!(counts.actor_reliable_pending, 0);
    assert_eq!(counts.sent_packets_pending, 0);
    assert_eq!(lab.shutdown(), 2);
    assert_eq!(lab.readiness_counts(), ReadinessCounts::default());
}

#[test]
fn initial_open_outcomes_preserve_failures_and_controller_skips_until_teardown() {
    let mut lab = prospect_refresh_lab();
    let (_first, first, _) = acked_game_state_peer(&mut lab);
    let (_second, second, _) = acked_game_state_peer(&mut lab);
    for (address, stages) in [
        (
            first,
            VecDeque::from([
                InitialOpenStage::Pawn,
                InitialOpenStage::Manager(usize::MAX),
                InitialOpenStage::Quest(usize::MAX),
                InitialOpenStage::GameState,
            ]),
        ),
        (
            second,
            VecDeque::from([
                InitialOpenStage::Controller,
                InitialOpenStage::GameState,
                InitialOpenStage::PlayerState,
            ]),
        ),
    ] {
        let entry = lab.peers.get_mut(&address).unwrap();
        entry.peer = LabPeer::from_cookie(&entry.cookie, *entry.peer.config()).unwrap();
        entry.game_state = None;
        for &stage in &stages {
            entry.initial_open_outcomes.configured.record(stage);
        }
        entry.initial_opens = Some(PendingInitialOpens {
            remaining: stages,
            controller_guids: Some((2, 4)),
            player_state_is_host: false,
            begin_slot: None,
            retry_ready: true,
        });
    }
    // Missing configured plans model permanent failures, without changing
    // the existing policy of attempting independent stages after a failure.
    lab.player_controller_open = None;
    lab.retry_initial_opens();
    let snapshot = lab.readiness_counts();
    let outcomes = &snapshot.initial_open_outcomes;
    assert_eq!(outcomes.failed.controller, 1);
    assert_eq!(outcomes.failed.pawn, 1);
    assert_eq!(outcomes.failed.managers, 1);
    assert_eq!(outcomes.failed.quests, 1);
    assert_eq!(outcomes.skipped.game_state, 1);
    assert_eq!(outcomes.skipped.player_state, 1);
    assert!(outcomes.pending.is_empty());
    assert!(outcomes.deferred.is_empty());
    assert_eq!(outcomes.committed.game_state, 1);
    assert_eq!(outcomes.configured.game_state, 2);
    let mut partition = outcomes.committed.clone();
    partition.add(&outcomes.failed);
    partition.add(&outcomes.skipped);
    partition.add(&outcomes.pending);
    assert_eq!(partition, outcomes.configured);
    assert_eq!(outcomes.sequences_failed, 2);
    assert_eq!(outcomes.sequences_succeeded, 0);
    assert_eq!(outcomes.sequences_settled, 0);
    assert!(outcomes.settled.is_empty());
    assert_eq!(outcomes.awaiting_ack, outcomes.committed);
    assert_eq!(snapshot.pending_initial_opens, 0);
    lab.retry_initial_opens();
    assert_eq!(lab.readiness_counts(), snapshot);
    let encoded = serde_json::to_string(outcomes).unwrap();
    assert!(!encoded.contains(&first.to_string()));
    assert!(!encoded.contains(&second.to_string()));
    lab.remove_peer(first).unwrap();
    assert_eq!(
        lab.readiness_counts()
            .initial_open_outcomes
            .sequences_failed,
        1
    );
    lab.shutdown();
    assert_eq!(lab.readiness_counts(), ReadinessCounts::default());
}

#[test]
fn initial_open_success_counts_configured_stages_once_and_reconnect_starts_clean() {
    let mut lab = prospect_refresh_lab();
    let (_socket, address, _) = acked_game_state_peer(&mut lab);
    let entry = lab.peers.get_mut(&address).unwrap();
    entry.peer = LabPeer::from_cookie(&entry.cookie, *entry.peer.config()).unwrap();
    entry.game_state = None;
    assert!(lab.send_player_controller_open(address));
    let snapshot = lab.readiness_counts().initial_open_outcomes;
    assert_eq!(snapshot.configured.controller, 1);
    assert_eq!(snapshot.configured.game_state, 1);
    assert_eq!(snapshot.configured.pawn, 0);
    assert_eq!(snapshot.configured.player_state, 0);
    assert_eq!(snapshot.committed, snapshot.configured);
    assert!(snapshot.failed.is_empty());
    assert!(snapshot.skipped.is_empty());
    assert_eq!(snapshot.sequences_succeeded, 1);
    assert_eq!(snapshot.sequences_settled, 0);
    assert!(snapshot.settled.is_empty());
    assert_eq!(snapshot.awaiting_ack, snapshot.committed);
    assert!(!lab.send_player_controller_open(address));
    lab.retry_initial_opens();
    assert_eq!(lab.readiness_counts().initial_open_outcomes, snapshot);
    lab.remove_peer(address).unwrap();
    let (_replacement, _, _) = acked_game_state_peer(&mut lab);
    assert_eq!(
        lab.readiness_counts().initial_open_outcomes,
        InitialOpenReadiness::default()
    );
}

#[test]
fn initial_open_settlement_distinguishes_stages_and_survives_loss_repair() {
    use super::super::{packet_lab::client_cookie_seeds, sequencing::PacketSequencer};
    let mut lab = prospect_refresh_lab();
    let (socket, address, _) = acked_game_state_peer(&mut lab);
    // Reset both authored transport endpoints so this sequence includes
    // the GameState, rather than reusing the fixture's settled open.
    let entry = lab.peers.get_mut(&address).unwrap();
    entry.peer = LabPeer::from_cookie(&entry.cookie, *entry.peer.config()).unwrap();
    entry.game_state = None;
    let (incoming, outgoing) = client_cookie_seeds(&entry.cookie);
    let mut client = PacketSequencer::new(incoming, outgoing, 64).unwrap();
    assert!(lab.send_player_controller_open(address));
    let queued = lab.readiness_counts().initial_open_outcomes;
    assert_eq!(queued.committed.controller, 1);
    assert_eq!(queued.committed.game_state, 1);
    assert_eq!(queued.awaiting_ack, queued.committed);
    assert!(queued.settled.is_empty());
    assert_eq!(queued.sequences_succeeded, 1);
    assert_eq!(queued.sequences_settled, 0);
    for _ in 0..4 {
        lab.retry_initial_opens();
        assert_eq!(lab.readiness_counts().initial_open_outcomes, queued);
    }
    // The first controller packet is lost, while GameState packets and
    // any remaining controller fragments receive positive ACKs.
    notify_startup_packets(&mut lab, &socket, &mut client, |index| index > 0);
    let partial = lab.readiness_counts().initial_open_outcomes;
    assert_eq!(partial.settled.controller, 0);
    assert_eq!(partial.awaiting_ack.controller, 1);
    assert_eq!(partial.settled.game_state, 1);
    assert_eq!(partial.awaiting_ack.game_state, 0);
    assert_eq!(partial.sequences_settled, 0);
    assert!(lab.readiness_counts().resend_bunches_pending > 0);
    for _ in 0..4 {
        lab.retry_initial_opens();
        assert_eq!(lab.readiness_counts().initial_open_outcomes, partial);
    }
    let datagram = lab
        .peers
        .get_mut(&address)
        .unwrap()
        .peer
        .ack_packet()
        .unwrap();
    assert!(lab.send(&datagram, address));
    ack_startup_packets(&mut lab, &socket, &mut client);
    let settled = lab.readiness_counts().initial_open_outcomes;
    assert_eq!(settled.settled, settled.committed);
    assert!(settled.awaiting_ack.is_empty());
    assert_eq!(settled.sequences_settled, 1);
    assert_eq!(settled.configured, queued.configured);
    assert_eq!(settled.committed, queued.committed);
    lab.remove_peer(address).unwrap();
    assert_eq!(lab.readiness_counts(), ReadinessCounts::default());
    let (_replacement, _, _) = acked_game_state_peer(&mut lab);
    assert_eq!(
        lab.readiness_counts().initial_open_outcomes,
        InitialOpenReadiness::default()
    );
}

#[test]
fn initial_open_split_settlement_waits_for_every_reliable_part() {
    use super::super::{
        game_state_open::handle, packet_lab::client_cookie_seeds, sequencing::PacketSequencer,
    };
    let mut lab = prospect_refresh_lab();
    let (socket, address, _) = acked_game_state_peer(&mut lab);
    lab.game_state_open.as_mut().unwrap().state.values.insert(
        handle::RICH_PRESENCE_GUID,
        RepValue::Str(ArchiveText::text(&"G".repeat(1_500)).unwrap()),
    );
    let entry = lab.peers.get_mut(&address).unwrap();
    entry.peer = LabPeer::from_cookie(&entry.cookie, *entry.peer.config()).unwrap();
    entry.game_state = None;
    let (incoming, outgoing) = client_cookie_seeds(&entry.cookie);
    let mut client = PacketSequencer::new(incoming, outgoing, 64).unwrap();
    entry
        .initial_open_outcomes
        .configured
        .record(InitialOpenStage::GameState);
    entry.initial_opens = Some(PendingInitialOpens {
        remaining: VecDeque::from([InitialOpenStage::GameState]),
        controller_guids: None,
        player_state_is_host: false,
        begin_slot: None,
        retry_ready: true,
    });
    lab.retry_initial_opens();
    assert!(
        lab.peers[&address]
            .peer
            .sequencer()
            .notify()
            .sent_records()
            .len()
            > 1
    );
    assert_eq!(
        lab.readiness_counts()
            .initial_open_outcomes
            .committed
            .game_state,
        1
    );
    notify_startup_packets(&mut lab, &socket, &mut client, |index| index > 0);
    let partial = lab.readiness_counts().initial_open_outcomes;
    assert!(partial.settled.is_empty());
    assert_eq!(partial.awaiting_ack.game_state, 1);
    assert_eq!(partial.sequences_settled, 0);
    let datagram = lab
        .peers
        .get_mut(&address)
        .unwrap()
        .peer
        .ack_packet()
        .unwrap();
    assert!(lab.send(&datagram, address));
    ack_startup_packets(&mut lab, &socket, &mut client);
    let repaired = lab.readiness_counts().initial_open_outcomes;
    assert_eq!(repaired.settled.game_state, 1);
    assert!(repaired.awaiting_ack.is_empty());
    assert_eq!(repaired.sequences_settled, 1);
}

#[test]
fn initial_open_settlement_is_retained_through_later_reliable_traffic() {
    let mut lab = prospect_refresh_lab();
    let (socket, address, mut client) = acked_game_state_peer(&mut lab);
    lab.game_state_open = None;
    assert!(lab.send_player_controller_open(address));
    let queued = lab.readiness_counts().initial_open_outcomes;
    notify_startup_packets(&mut lab, &socket, &mut client, |_| false);
    assert_eq!(lab.readiness_counts().initial_open_outcomes, queued);
    let repair = lab
        .peers
        .get_mut(&address)
        .unwrap()
        .peer
        .ack_packet()
        .unwrap();
    assert!(lab.send(&repair, address));
    ack_startup_packets(&mut lab, &socket, &mut client);
    let settled = lab.readiness_counts().initial_open_outcomes;
    assert_eq!(settled.settled.controller, 1);
    assert_eq!(settled.sequences_settled, 1);
    let entry = lab.peers.get_mut(&address).unwrap();
    let channel = entry.player_controller_channel.unwrap();
    let sent = entry
        .peer
        .reliable_actor_bunch_datagrams(
            channel,
            Payload::from_writer(super::super::packet::BitWriter::new()),
        )
        .unwrap();
    assert!(lab.send_all(&sent, address));
    assert!(!lab.peers[&address].peer.actor_channel_settled(channel));
    assert_eq!(lab.readiness_counts().initial_open_outcomes, settled);
    ack_startup_packets(&mut lab, &socket, &mut client);
    assert_eq!(lab.readiness_counts().initial_open_outcomes, settled);
}

#[test]
fn initial_open_close_ack_does_not_create_a_settlement_receipt() {
    let mut lab = prospect_refresh_lab();
    let (socket, address, mut client) = acked_game_state_peer(&mut lab);
    lab.game_state_open = None;
    assert!(lab.send_player_controller_open(address));
    let entry = lab.peers.get_mut(&address).unwrap();
    let channel = entry.player_controller_channel.unwrap();
    let close = entry.peer.reliable_actor_close(channel, 0).unwrap();
    assert!(lab.send(&close, address));
    ack_startup_packets(&mut lab, &socket, &mut client);
    assert!(!lab.peers[&address].peer.has_actor_channel(channel));
    let closed = lab.readiness_counts().initial_open_outcomes;
    assert_eq!(closed.committed.controller, 1);
    assert_eq!(closed.awaiting_ack.controller, 1);
    assert!(closed.settled.is_empty());
    assert_eq!(closed.sequences_settled, 0);
    lab.retry_initial_opens();
    assert_eq!(lab.readiness_counts().initial_open_outcomes, closed);
    // Reuse the released transport channel with an authored open. Reusing
    // the controller composer would also exercise its unrelated handling
    // of previously exported archetype/level references.
    let mut config = lab.config.packet.packet;
    config.receiver = super::super::packet::Receiver::Client;
    let mut reopened = super::super::packet::read_packet(&close, &config)
        .unwrap()
        .bunches
        .pop()
        .unwrap();
    reopened.header.open = true;
    reopened.header.close = false;
    reopened.header.close_reason = None;
    reopened.header.reliable_sequence_raw = None;
    let sent = lab
        .peers
        .get_mut(&address)
        .unwrap()
        .peer
        .reliable_actor_open_datagrams(&reopened, &[])
        .unwrap();
    assert!(lab.send_all(&sent, address));
    ack_startup_packets(&mut lab, &socket, &mut client);
    assert!(lab.peers[&address].peer.actor_channel_settled(channel));
    assert_eq!(lab.readiness_counts().initial_open_outcomes, closed);
}

#[test]
fn controller_startup_configuration_is_bounded_atomic_and_opt_in() {
    use ControllerStartupAction::{FlushLevelStreaming, RestartPawn};
    let mut lab = prospect_refresh_lab();
    assert!(lab.controller_startup.is_none());
    for actions in [vec![], vec![FlushLevelStreaming; 33], vec![RestartPawn]] {
        assert!(
            lab.enable_controller_startup(&ReleaseProfile::fixture("3.0.32"), actions)
                .is_err()
        );
        assert!(lab.controller_startup.is_none());
    }
    lab.enable_controller_startup(
        &ReleaseProfile::fixture("3.0.32"),
        vec![FlushLevelStreaming],
    )
    .unwrap();
    assert!(
        lab.enable_controller_startup(
            &ReleaseProfile::fixture("3.0.32"),
            vec![FlushLevelStreaming]
        )
        .is_err()
    );
    assert_eq!(lab.controller_startup, Some(vec![FlushLevelStreaming]));
    let (_socket, _, _) = acked_game_state_peer(&mut lab);
    assert!(
        lab.enable_controller_startup(
            &ReleaseProfile::fixture("3.0.32"),
            vec![FlushLevelStreaming]
        )
        .is_err()
    );
    assert_eq!(lab.controller_startup, Some(vec![FlushLevelStreaming]));
}

#[test]
fn controller_startup_payload_targets_actor_and_resolves_each_pawn() {
    use super::super::{
        actor_channel::{NetGuidRef, read_actor_bunch_body},
        archive::ByteOrder,
        rep_rpc::parse_rpc_payload,
    };
    for order in [ByteOrder::Little, ByteOrder::Big] {
        let codec = RepLayoutCodec {
            actor: actor_codec(30, order),
            checksums: false,
            limits: Default::default(),
        };
        for pawn in [2, 42] {
            let payload = controller_startup_payload(
                ControllerStartupAction::RestartPawn,
                BP_ICARUS_PLAYER_CONTROLLER_SURVIVAL_FIELD_INDEX_BOUND,
                Some(pawn),
                &codec,
            )
            .unwrap();
            let body = read_actor_bunch_body(&mut payload.reader(), false, &codec.actor).unwrap();
            assert!(body.spawn.is_none());
            assert_eq!(body.blocks.len(), 1);
            let block = &body.blocks[0];
            assert!(matches!(block.target, BlockTarget::Actor));
            assert!(!block.has_rep_layout);
            let mut reader = block.payload.reader();
            let field = read_field_header(
                &mut reader,
                BP_ICARUS_PLAYER_CONTROLLER_SURVIVAL_FIELD_INDEX_BOUND,
                codec.actor.limits.max_payload_bits,
            )
            .unwrap();
            assert!(reader.at_end());
            assert_eq!(field.index, 40);
            let arguments = parse_rpc_payload(
                &field.payload,
                &player_controller_rpc_parameters(40).unwrap(),
                &codec,
            )
            .unwrap();
            assert_eq!(
                arguments,
                vec![RpcArgument::Sent(vec![RpcItem::Value(RepValue::Object(
                    NetGuidRef::plain(pawn)
                ))])]
            );
        }
        assert!(
            controller_startup_payload(
                ControllerStartupAction::RestartPawn,
                BP_ICARUS_PLAYER_CONTROLLER_SURVIVAL_FIELD_INDEX_BOUND,
                None,
                &codec
            )
            .is_err()
        );
    }
}

#[test]
fn controller_startup_waits_for_open_ack_and_retains_order_without_poll_replays() {
    use ControllerStartupAction::FlushLevelStreaming;
    let mut lab = prospect_refresh_lab();
    lab.config.packet.sent_capacity = 8;
    lab.enable_controller_startup(
        &ReleaseProfile::fixture("3.0.32"),
        vec![FlushLevelStreaming, FlushLevelStreaming],
    )
    .unwrap();
    let (socket, address, mut client) = acked_game_state_peer(&mut lab);
    // The authored fixture supplies an already-settled actor channel. A
    // startup cursor is installed only by the normal open sequence below.
    assert_eq!(lab.readiness_counts().controller_startup_actions_queued, 0);
    lab.retry_controller_startup();
    assert_eq!(lab.readiness_counts().controller_startup_actions_queued, 0);
    lab.game_state_open = None;
    assert!(lab.send_player_controller_open(address));
    assert_eq!(lab.readiness_counts().controller_startup_actions_pending, 2);
    lab.retry_controller_startup();
    assert_eq!(lab.readiness_counts().controller_startup_actions_queued, 0);
    assert!(!lab.peers[&address].controller_startup_retry_ready);
    let snapshot = lab.readiness_counts();
    for _ in 0..4 {
        lab.retry_controller_startup();
    }
    assert_eq!(lab.readiness_counts(), snapshot);
    ack_startup_packets(&mut lab, &socket, &mut client);
    // Occupy the bounded sent-record ring with independently authored
    // ACK-only packets while the actor channel itself remains settled.
    let capacity = lab.peers[&address].peer.config().sent_capacity;
    for _ in 0..capacity {
        let datagram = lab
            .peers
            .get_mut(&address)
            .unwrap()
            .peer
            .ack_packet()
            .unwrap();
        assert!(lab.send(&datagram, address));
    }
    lab.retry_controller_startup();
    assert_eq!(lab.readiness_counts().controller_startup_actions_queued, 0);
    assert_eq!(lab.readiness_counts().controller_startup_actions_pending, 2);
    assert_eq!(
        lab.readiness_counts().controller_startup_sequences_failed,
        0
    );
    let pressure = lab.readiness_counts();
    for _ in 0..4 {
        lab.retry_controller_startup();
    }
    assert_eq!(lab.readiness_counts(), pressure);
    ack_startup_packets(&mut lab, &socket, &mut client);
    lab.retry_controller_startup();
    assert_eq!(lab.readiness_counts().controller_startup_actions_queued, 1);
    assert_eq!(lab.readiness_counts().controller_startup_actions_pending, 1);
    assert_eq!(lab.readiness_counts().controller_startup_actions_settled, 0);
    assert_eq!(
        lab.readiness_counts()
            .controller_startup_actions_awaiting_ack,
        1
    );
    assert_eq!(
        lab.readiness_counts().controller_startup_sequences_settled,
        0
    );
    lab.retry_controller_startup();
    assert_eq!(lab.readiness_counts().controller_startup_actions_queued, 1);
    ack_startup_packets(&mut lab, &socket, &mut client);
    assert_eq!(lab.readiness_counts().controller_startup_actions_settled, 1);
    assert_eq!(
        lab.readiness_counts()
            .controller_startup_actions_awaiting_ack,
        0
    );
    lab.retry_controller_startup();
    assert_eq!(lab.readiness_counts().controller_startup_actions_queued, 2);
    assert_eq!(lab.readiness_counts().controller_startup_actions_pending, 0);
    assert_eq!(lab.readiness_counts().controller_startup_actions_settled, 1);
    assert_eq!(
        lab.readiness_counts()
            .controller_startup_actions_awaiting_ack,
        1
    );
    assert_eq!(
        lab.readiness_counts().controller_startup_sequences_settled,
        0
    );
    assert_eq!(
        lab.readiness_counts().controller_startup_sequences_failed,
        0
    );
    ack_startup_packets(&mut lab, &socket, &mut client);
    assert_eq!(lab.readiness_counts().controller_startup_actions_settled, 2);
    assert_eq!(
        lab.readiness_counts()
            .controller_startup_actions_awaiting_ack,
        0
    );
    assert_eq!(
        lab.readiness_counts().controller_startup_sequences_settled,
        1
    );
    // Later reliable traffic cannot revoke an already retained receipt.
    let channel = lab.peers[&address].player_controller_channel.unwrap();
    lab.peers
        .get_mut(&address)
        .unwrap()
        .peer
        .reliable_actor_bunch(
            channel,
            Payload::from_writer(super::super::packet::BitWriter::new()),
        )
        .unwrap();
    assert!(!lab.peers[&address].peer.actor_channel_settled(channel));
    let settled = lab.readiness_counts();
    assert_eq!(settled.controller_startup_actions_settled, 2);
    assert_eq!(settled.controller_startup_sequences_settled, 1);
    for _ in 0..4 {
        lab.retry_controller_startup();
        assert_eq!(lab.readiness_counts(), settled);
    }
    lab.remove_peer(address).unwrap();
    assert_eq!(lab.readiness_counts(), ReadinessCounts::default());
}

fn ack_startup_packets(
    lab: &mut PeerLab,
    socket: &UdpSocket,
    client: &mut super::super::sequencing::PacketSequencer,
) {
    notify_startup_packets(lab, socket, client, |_| true);
}

fn notify_startup_packets(
    lab: &mut PeerLab,
    socket: &UdpSocket,
    client: &mut super::super::sequencing::PacketSequencer,
    delivered: impl Fn(usize) -> bool,
) {
    use super::super::packet::{Packet, Receiver, read_packet, write_packet};
    let SocketAddr::V4(address) = socket.local_addr().unwrap() else {
        unreachable!()
    };
    let count = lab.peers[&address]
        .peer
        .sequencer()
        .notify()
        .sent_records()
        .len();
    assert!(count > 0);
    let config = super::super::packet::PacketConfig {
        receiver: Receiver::Client,
        ..lab.config.packet.packet
    };
    for index in 0..count {
        let mut bytes = [0; 2048];
        let length = socket.recv(&mut bytes).unwrap();
        let packet = read_packet(&bytes[..length], &config).unwrap();
        let received = client.receive(&packet.header).unwrap().unwrap();
        client.ack_seq(received.sequence, delivered(index)).unwrap();
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
    receive_authored_at(lab, socket, &bytes, Instant::now());
}

#[test]
fn controller_startup_settlement_requires_ack_and_survives_loss_repair() {
    for mixed in [false, true] {
        let mut lab = prospect_refresh_lab();
        lab.enable_controller_startup(
            &ReleaseProfile::fixture("3.0.32"),
            vec![ControllerStartupAction::FlushLevelStreaming],
        )
        .unwrap();
        let (socket, address, mut client) = acked_game_state_peer(&mut lab);
        lab.game_state_open = None;
        assert!(lab.send_player_controller_open(address));
        ack_startup_packets(&mut lab, &socket, &mut client);
        lab.retry_controller_startup();
        assert_eq!(lab.readiness_counts().controller_startup_actions_queued, 1);
        if mixed {
            // A positive ACK for independent traffic must not hide the
            // startup packet's negative ACK in the same notification.
            let datagram = lab
                .peers
                .get_mut(&address)
                .unwrap()
                .peer
                .ack_packet()
                .unwrap();
            assert!(lab.send(&datagram, address));
        }
        notify_startup_packets(&mut lab, &socket, &mut client, |index| mixed && index > 0);
        let lost = lab.readiness_counts();
        assert_eq!(lost.controller_startup_actions_settled, 0);
        assert_eq!(lost.controller_startup_actions_awaiting_ack, 1);
        assert_eq!(lost.controller_startup_sequences_settled, 0);
        assert!(lost.resend_bunches_pending > 0);
        for _ in 0..4 {
            lab.retry_controller_startup();
            assert_eq!(lab.readiness_counts(), lost);
        }
        // The transport resends its retained bunch; startup does not
        // enqueue the RPC a second time.
        let datagram = lab
            .peers
            .get_mut(&address)
            .unwrap()
            .peer
            .ack_packet()
            .unwrap();
        assert!(lab.send(&datagram, address));
        ack_startup_packets(&mut lab, &socket, &mut client);
        let repaired = lab.readiness_counts();
        assert_eq!(repaired.controller_startup_actions_queued, 1);
        assert_eq!(repaired.controller_startup_actions_settled, 1);
        assert_eq!(repaired.controller_startup_actions_awaiting_ack, 0);
        assert_eq!(repaired.controller_startup_sequences_settled, 1);
    }
}

#[test]
fn controller_startup_close_ack_is_not_an_open_channel_settlement_receipt() {
    let mut lab = prospect_refresh_lab();
    lab.enable_controller_startup(
        &ReleaseProfile::fixture("3.0.32"),
        vec![ControllerStartupAction::FlushLevelStreaming],
    )
    .unwrap();
    let (socket, address, mut client) = acked_game_state_peer(&mut lab);
    lab.game_state_open = None;
    assert!(lab.send_player_controller_open(address));
    ack_startup_packets(&mut lab, &socket, &mut client);
    lab.retry_controller_startup();
    let channel = lab.peers[&address].player_controller_channel.unwrap();
    let close = lab
        .peers
        .get_mut(&address)
        .unwrap()
        .peer
        .reliable_actor_close(channel, 0)
        .unwrap();
    assert!(lab.send(&close, address));
    assert!(!lab.peers[&address].peer.actor_channel_settled(channel));
    ack_startup_packets(&mut lab, &socket, &mut client);
    assert!(!lab.peers[&address].peer.has_actor_channel(channel));
    let closed = lab.readiness_counts();
    assert_eq!(closed.controller_startup_actions_settled, 0);
    assert_eq!(closed.controller_startup_actions_awaiting_ack, 1);
    assert_eq!(closed.controller_startup_sequences_settled, 0);
    assert_eq!(lab.readiness_counts(), closed);
}

#[test]
fn controller_startup_aborts_after_failed_initial_open_without_publishing() {
    let mut lab = prospect_refresh_lab();
    lab.enable_controller_startup(
        &ReleaseProfile::fixture("3.0.32"),
        vec![ControllerStartupAction::FlushLevelStreaming],
    )
    .unwrap();
    let (_socket, address, _) = acked_game_state_peer(&mut lab);
    // Authored terminal initial-stage failure: it cannot become a startup
    // success merely because the remaining stage queue is now empty.
    let entry = lab.peers.get_mut(&address).unwrap();
    entry.initial_open_outcomes.failed.pawn = 1;
    entry.initial_opens = Some(PendingInitialOpens {
        remaining: VecDeque::new(),
        controller_guids: None,
        player_state_is_host: true,
        begin_slot: None,
        retry_ready: true,
    });
    lab.retry_initial_opens();
    lab.retry_controller_startup();
    assert_eq!(
        lab.readiness_counts().controller_startup_sequences_failed,
        1
    );
    assert_eq!(lab.readiness_counts().controller_startup_actions_queued, 0);
    assert_eq!(lab.readiness_counts().controller_startup_actions_pending, 0);
}

#[test]
fn controller_startup_restart_uses_each_opened_pawn_and_waits_for_its_ack() {
    use super::super::{
        actor_channel::read_actor_bunch_body,
        packet::{Receiver, read_packet},
        pawn_open::PAWN_CHANNEL_INDEX,
        rep_rpc::parse_rpc_payload,
    };
    let profile = ReleaseProfile::fixture("3.0.32");
    let mut lab = prospect_refresh_lab();
    lab.enable_pawn_open(
        profile
            .pawn_open_plan(PeerLabAdmission::default().map_url)
            .unwrap()
            .unwrap(),
        PAWN_CHANNEL_INDEX,
        PawnContent::default(),
    )
    .unwrap();
    lab.enable_controller_startup(&profile, vec![ControllerStartupAction::RestartPawn])
        .unwrap();
    let mut fixtures = Vec::new();
    for connection in [1, 2] {
        let (socket, address, client) = acked_game_state_peer(&mut lab);
        lab.peers.get_mut(&address).unwrap().connection_id = Some(connection);
        fixtures.push((socket, address, client, connection));
    }
    lab.game_state_open = None;
    let mut pawn_guids = Vec::new();
    for (socket, address, client, connection) in &mut fixtures {
        assert!(lab.send_player_controller_open(*address));
        let expected = lab.guid_cache.actor_guid(pawn_key(*connection)).unwrap();
        assert_eq!(
            lab.peers[address].controller_startup_pawn_guid,
            Some(expected)
        );
        pawn_guids.push(expected);
        lab.retry_controller_startup();
        assert_eq!(lab.peers[address].controller_startup_queued, 0);
        ack_startup_packets(&mut lab, socket, client);
        // Keep only the pawn channel unsettled, independently of the
        // controller. This authored reliable body is transport test data.
        let codec = RepLayoutCodec {
            actor: actor_codec(
                profile.engine_network_version,
                super::super::archive::ByteOrder::Little,
            ),
            checksums: false,
            limits: Default::default(),
        };
        let payload = controller_startup_payload(
            ControllerStartupAction::FlushLevelStreaming,
            lab.controller_startup_bound,
            None,
            &codec,
        )
        .unwrap();
        let sent = lab
            .peers
            .get_mut(address)
            .unwrap()
            .peer
            .reliable_actor_bunch_datagrams(PAWN_CHANNEL_INDEX, payload)
            .unwrap();
        assert!(lab.send_all(&sent, *address));
        assert!(
            lab.peers[address]
                .peer
                .actor_channel_settled(lab.player_controller_open.as_ref().unwrap().channel_index)
        );
        lab.retry_controller_startup();
        assert_eq!(lab.peers[address].controller_startup_queued, 0);
        ack_startup_packets(&mut lab, socket, client);
        lab.retry_controller_startup();
        assert_eq!(lab.peers[address].controller_startup_queued, 1);
        let mut bytes = [0; 2048];
        let length = socket.recv(&mut bytes).unwrap();
        let packet = read_packet(
            &bytes[..length],
            &super::super::packet::PacketConfig {
                receiver: Receiver::Client,
                ..lab.config.packet.packet
            },
        )
        .unwrap();
        assert_eq!(packet.bunches.len(), 1);
        assert_eq!(
            packet.bunches[0].header.channel_index,
            lab.player_controller_open.as_ref().unwrap().channel_index
        );
        let body =
            read_actor_bunch_body(&mut packet.bunches[0].payload.reader(), false, &codec.actor)
                .unwrap();
        let field = read_field_header(
            &mut body.blocks[0].payload.reader(),
            lab.controller_startup_bound,
            codec.actor.limits.max_payload_bits,
        )
        .unwrap();
        assert_eq!(field.index, 40);
        let args = parse_rpc_payload(
            &field.payload,
            &player_controller_rpc_parameters(40).unwrap(),
            &codec,
        )
        .unwrap();
        assert_eq!(
            args,
            vec![RpcArgument::Sent(vec![RpcItem::Value(RepValue::Object(
                super::super::actor_channel::NetGuidRef::plain(expected)
            ))])]
        );
    }
    assert_ne!(pawn_guids[0], pawn_guids[1]);
    lab.shutdown();
    assert_eq!(lab.readiness_counts(), ReadinessCounts::default());
}

#[test]
fn deferred_controller_reports_live_sequence_and_failed_begin_counts_once() {
    let mut lab = prospect_refresh_lab();
    let (_socket, address, _) = acked_game_state_peer(&mut lab);
    let entry = lab.peers.get_mut(&address).unwrap();
    let mut config = *entry.peer.config();
    config.sent_capacity = 1;
    entry.peer = LabPeer::from_cookie(&entry.cookie, config).unwrap();
    entry.peer.ack_packet().unwrap();
    assert!(lab.send_reply(address, &Reply::OpenPlayerController));
    assert!(lab.peers[&address].player_controller_channel.is_none());
    assert!(lab.peers[&address].initial_opens.is_some());
    assert_eq!(lab.readiness_counts().pending_initial_opens, 1);
    assert_eq!(lab.readiness_counts().sent_packets_pending, 1);
    assert_eq!(lab.stats.player_controller_open_errors, 1);
    let outcomes = lab.readiness_counts().initial_open_outcomes;
    assert_eq!(outcomes.configured.controller, 1);
    assert_eq!(outcomes.pending.controller, 1);
    assert_eq!(outcomes.pending.game_state, 1);
    assert_eq!(outcomes.deferred.controller, 1);
    assert_eq!(outcomes.deferred.game_state, 0);
    assert!(outcomes.failed.is_empty());
    assert!(outcomes.settled.is_empty());
    assert!(outcomes.awaiting_ack.is_empty());
    assert_eq!(outcomes.sequences_settled, 0);
    for _ in 0..4 {
        lab.retry_initial_opens();
    }
    assert_eq!(lab.readiness_counts().initial_open_outcomes, outcomes);

    // No PlayerState plan: the pending stage must terminate and count the
    // retained request exactly once, without starting a profile exchange.
    let entry = lab.peers.get_mut(&address).unwrap();
    entry.initial_opens = Some(PendingInitialOpens {
        remaining: VecDeque::from([InitialOpenStage::PlayerState]),
        controller_guids: Some((2, 4)),
        player_state_is_host: true,
        begin_slot: Some(3),
        retry_ready: true,
    });
    lab.retry_initial_opens();
    assert!(lab.peers[&address].initial_opens.is_none());
    assert_eq!(lab.stats.initialisation_calls_ignored, 1);
    assert_eq!(lab.stats.profile_user_requests_sent, 0);
    lab.retry_initial_opens();
    assert_eq!(lab.stats.initialisation_calls_ignored, 1);
}

/// Retry scheduling must not turn one pawn into several UID allocation
/// attempts. The seeded collision also makes leaked historical remaps visible.
#[test]
fn pending_pawn_keeps_one_uid_and_releases_it_on_terminal_paths() {
    use super::super::{
        pawn_open::{PAWN_CHANNEL_INDEX, character_state_handle as h},
        release::ReleaseProfile,
    };
    use crate::actor_uids::{ActorUidLibrary, FIRST_NATIVE_OBJECT_ID};

    for terminal in ["commit", "disconnect", "permanent_failure"] {
        let mut lab = prospect_refresh_lab();
        lab.enable_pawn_open(
            ReleaseProfile::fixture("3.0.32")
                .pawn_open_plan(PeerLabAdmission::default().map_url)
                .unwrap()
                .unwrap(),
            PAWN_CHANNEL_INDEX,
            PawnContent::default(),
        )
        .unwrap();
        lab.enable_actor_uids(ActorUidLibrary::new(1)).unwrap();
        lab.seed_actor_uids([FIRST_NATIVE_OBJECT_ID]).unwrap();
        let (_socket, address, _) = acked_game_state_peer(&mut lab);
        let entry = lab.peers.get_mut(&address).unwrap();
        let cookie = entry.cookie;
        let mut packet_config = *entry.peer.config();
        packet_config.sent_capacity = 1;
        // Replace only the authored transport fixture, leaving one live
        // record so any otherwise valid pawn open encounters drainable pressure.
        entry.peer = LabPeer::from_cookie(&cookie, packet_config).unwrap();
        entry.game_state = None;
        entry.connection_id = Some(1);
        entry.peer.ack_packet().unwrap();
        entry.initial_opens = Some(PendingInitialOpens {
            remaining: VecDeque::from([InitialOpenStage::Pawn]),
            controller_guids: Some((2, 4)),
            player_state_is_host: false,
            begin_slot: None,
            retry_ready: true,
        });
        lab.retry_initial_opens();
        let uid = FIRST_NATIVE_OBJECT_ID + 1;
        assert_eq!(
            lab.readiness_counts().initial_open_outcomes.deferred.pawn,
            1
        );
        assert_eq!(lab.readiness_counts().initial_open_outcomes.failed.pawn, 0);
        assert_eq!(lab.peers[&address].pending_pawn_uid, Some(uid));
        assert_eq!(lab.peers[&address].pawn_uid, None);
        assert_eq!(lab.next_pawn_object_id, Some(FIRST_NATIVE_OBJECT_ID + 1));
        assert_eq!(lab.claimed_actor_uids(), &[FIRST_NATIVE_OBJECT_ID, uid]);
        let remaps = lab.actor_uids.as_ref().unwrap().remaps().to_vec();
        assert_eq!(remaps, vec![(FIRST_NATIVE_OBJECT_ID, uid)]);
        let first_errors = lab.stats.pawn_open_errors;
        assert_eq!(first_errors, 1);

        // Repeated scheduler passes without notifications do no work.
        for _ in 0..4 {
            lab.retry_initial_opens();
        }
        assert_eq!(lab.stats.pawn_open_errors, first_errors);
        // Model bounded notification-driven attempts that still find the
        // ring occupied. Each must reuse the original pending allocation.
        for _ in 0..8 {
            lab.peers
                .get_mut(&address)
                .unwrap()
                .initial_opens
                .as_mut()
                .unwrap()
                .retry_ready = true;
            lab.retry_initial_opens();
            assert_eq!(lab.peers[&address].pending_pawn_uid, Some(uid));
            assert_eq!(lab.peers[&address].pawn_uid, None);
            assert_eq!(lab.next_pawn_object_id, Some(FIRST_NATIVE_OBJECT_ID + 1));
            assert_eq!(lab.actor_uids.as_ref().unwrap().remaps(), remaps);
            assert_eq!(lab.claimed_actor_uids(), &[FIRST_NATIVE_OBJECT_ID, uid]);
        }
        assert_eq!(lab.stats.pawn_open_errors, first_errors + 8);

        match terminal {
            "commit" => {
                // A roomy empty transport isolates the successful ownership
                // transfer from pending to published; no new allocation is needed.
                packet_config.sent_capacity = 256;
                lab.peers.get_mut(&address).unwrap().peer =
                    LabPeer::from_cookie(&cookie, packet_config).unwrap();
                lab.peers
                    .get_mut(&address)
                    .unwrap()
                    .initial_opens
                    .as_mut()
                    .unwrap()
                    .retry_ready = true;
                lab.retry_initial_opens();
                assert_eq!(lab.stats.pawn_opens_sent, 1);
                assert_eq!(lab.peers[&address].pending_pawn_uid, None);
                assert_eq!(lab.peers[&address].pawn_uid, Some(uid));
                assert!(lab.peers[&address].initial_opens.is_none());
                assert_eq!(
                    lab.readiness_counts().initial_open_outcomes.committed.pawn,
                    1
                );
                assert_eq!(
                    lab.readiness_counts().initial_open_outcomes.deferred.pawn,
                    0
                );
                assert_eq!(lab.claimed_actor_uids(), &[FIRST_NATIVE_OBJECT_ID, uid]);
                assert!(lab.remove_peer(address).is_some());
            }
            "disconnect" => {
                assert!(lab.remove_peer(address).is_some());
            }
            "permanent_failure" => {
                // A malformed authored temperature is a permanent composition
                // failure even while outstanding transport records remain.
                lab.pawn_open
                    .as_mut()
                    .unwrap()
                    .2
                    .character_state
                    .insert(h::INTERNAL_TEMPERATURE, RepValue::Bool(true));
                lab.peers
                    .get_mut(&address)
                    .unwrap()
                    .initial_opens
                    .as_mut()
                    .unwrap()
                    .retry_ready = true;
                lab.retry_initial_opens();
                assert_eq!(lab.stats.pawn_opens_sent, 0);
                assert_eq!(lab.peers[&address].pending_pawn_uid, None);
                assert_eq!(lab.peers[&address].pawn_uid, None);
                assert!(lab.peers[&address].initial_opens.is_none());
                assert!(!lab.peers[&address].peer.is_failed());
                assert_eq!(lab.readiness_counts().initial_open_outcomes.failed.pawn, 1);
                assert_eq!(
                    lab.readiness_counts().initial_open_outcomes.deferred.pawn,
                    0
                );
            }
            _ => unreachable!(),
        }
        assert_eq!(lab.claimed_actor_uids(), &[FIRST_NATIVE_OBJECT_ID]);
        assert_eq!(lab.next_pawn_object_id, Some(FIRST_NATIVE_OBJECT_ID + 1));
        assert_eq!(lab.actor_uids.as_ref().unwrap().remaps(), remaps);
    }
}

/// An initial open larger than the entire ring is not retryable once
/// that ring is empty. Existing traffic may defer its first attempt, but
/// unchanged polls cannot repeat it, and the real ACK path clears it.
#[test]
fn initial_open_oversize_stops_after_drain_and_unchanged_polls_do_not_retry() {
    use super::super::{
        game_state_open::handle,
        packet::{Packet, Receiver, read_packet, write_packet},
        packet_lab::client_cookie_seeds,
        sequencing::PacketSequencer,
    };

    for initially_full in [false, true] {
        let mut lab = prospect_refresh_lab();
        let (socket, address, _) = acked_game_state_peer(&mut lab);
        lab.game_state_open.as_mut().unwrap().state.values.insert(
            handle::RICH_PRESENCE_GUID,
            RepValue::Str(ArchiveText::text(&"G".repeat(1_500)).unwrap()),
        );
        // Reset only the authored transport fixture: one valid GameState
        // open now needs more datagrams than this entire one-record ring.
        let entry = lab.peers.get_mut(&address).unwrap();
        let cookie = entry.cookie;
        let mut config = *entry.peer.config();
        config.sent_capacity = 1;
        entry.peer = LabPeer::from_cookie(&cookie, config).unwrap();
        entry.game_state = None;
        let outstanding = initially_full.then(|| entry.peer.ack_packet().unwrap());
        entry.initial_opens = Some(PendingInitialOpens {
            remaining: VecDeque::from([InitialOpenStage::GameState]),
            controller_guids: None,
            player_state_is_host: false,
            begin_slot: None,
            retry_ready: true,
        });
        let before = lab.stats.game_state_open_errors;
        lab.retry_initial_opens();
        assert_eq!(lab.stats.game_state_open_errors, before + 1);
        assert_eq!(lab.peers[&address].initial_opens.is_some(), initially_full);
        let packet_id = lab.peers[&address].peer.sequencer().out_packet_id();
        for _ in 0..8 {
            lab.poll();
        }
        assert_eq!(lab.stats.game_state_open_errors, before + 1);
        assert_eq!(
            lab.peers[&address].peer.sequencer().out_packet_id(),
            packet_id
        );

        if let Some(outstanding) = outstanding {
            assert!(
                !lab.peers[&address]
                    .initial_opens
                    .as_ref()
                    .unwrap()
                    .retry_ready,
                "the deferred stage waits for packet notification progress"
            );
            let (incoming, outgoing) = client_cookie_seeds(&cookie);
            let mut client = PacketSequencer::new(incoming, outgoing, 8).unwrap();
            let mut to_client = lab.config.packet.packet;
            to_client.receiver = Receiver::Client;
            let sent = read_packet(&outstanding, &to_client).unwrap();
            let received = client.receive(&sent.header).unwrap().unwrap();
            client.ack_seq(received.sequence, true).unwrap();
            let ack = write_packet(
                &Packet {
                    magic: 0,
                    header: client.header().unwrap(),
                    info: None,
                    bunches: Vec::new(),
                },
                &lab.config.packet.packet,
            )
            .unwrap();
            client.commit().unwrap();
            socket.send(&ack).unwrap();
            lab.poll();
            assert_eq!(lab.stats.game_state_open_errors, before + 2);
            assert!(lab.peers[&address].initial_opens.is_none());
            assert!(lab.peers[&address].game_state.is_none());
        }
        let errors = lab.stats.game_state_open_errors;
        for _ in 0..8 {
            lab.poll();
        }
        assert_eq!(lab.stats.game_state_open_errors, errors);
        assert!(lab.peers[&address].initial_opens.is_none());
        assert!(!lab.peers[&address].peer.is_failed());
        let outcomes = lab.readiness_counts().initial_open_outcomes;
        assert_eq!(outcomes.failed.game_state, 1);
        assert_eq!(outcomes.committed.game_state, 0);
        assert!(outcomes.pending.is_empty());
        assert!(outcomes.deferred.is_empty());
    }
}

#[test]
fn returning_restore_reexports_unacknowledged_components_once() {
    use super::super::{
        actor_channel::{BlockTarget, read_actor_bunch_body, read_export_prefix},
        archive::ByteOrder,
        control_messages::{NetIdType, NetIdValue, UniqueNetId},
        packet::{PacketConfig, Receiver, read_packet},
        pawn_open::{PAWN_CHANNEL_INDEX, character_state_handle as h},
        recorders::{SavedPlayerCharacter, SavedPlayerState},
        release::ReleaseProfile,
        rep_layout::read_rep_properties,
        sequencing::{PartialBunchReassembler, PartialReceive},
    };

    // Exercise the finalisation path with pending, lost and acknowledged
    // open exports. Only the export delivery state differs; every value
    // is authored. No inventory, modifier or stat delta can select the
    // multi-datagram path on behalf of the component export.
    for export_state in ["pending", "lost", "acked"] {
        let mut lab = prospect_refresh_lab();
        let plan = ReleaseProfile::fixture("3.0.32")
            .pawn_open_plan(PeerLabAdmission::default().map_url)
            .unwrap()
            .unwrap();
        let layout = plan.content.character_state_layout.clone();
        let engine = plan.engine_net_version;
        lab.enable_pawn_open(
            plan,
            PAWN_CHANNEL_INDEX,
            PawnContent {
                character_state: BTreeMap::from([
                    (h::HEALTH, RepValue::Int(100)),
                    (h::MAX_HEALTH, RepValue::Int(100)),
                ]),
                ..Default::default()
            },
        )
        .unwrap();
        lab.enable_actor_uids(crate::actor_uids::ActorUidLibrary::new(1))
            .unwrap();
        lab.seed_actor_uids([70_001]).unwrap();
        lab.enable_returning_characters(vec![SavedPlayerCharacter {
            player_id: "AuthoredPlayer".encode_utf16().collect(),
            chr_slot: 3,
            icarus_uid: 70_001,
            state: SavedPlayerState {
                health: Some(37),
                ..Default::default()
            },
            inventories: Vec::new(),
            modifiers: Vec::new(),
        }])
        .unwrap();
        let (socket, address, _client) = acked_game_state_peer(&mut lab);
        let entry = lab.peers.get_mut(&address).unwrap();
        entry.connection_id = Some(1);
        entry.login_unique_id = Some(UniqueNetId {
            kind: NetIdType::Named(ArchiveText::text("NULL").unwrap()),
            value: NetIdValue::Text(ArchiveText::text("AuthoredPlayer").unwrap()),
        });
        entry.profiles.character_slot = Some(3);
        entry.profiles.completed = true;
        entry.profiles.friend_ids_ready = true;
        let first_packet = entry.peer.sequencer().out_packet_id();
        lab.send_pawn_open(address, 2, 4);
        assert_eq!(lab.stats.pawn_opens_sent, 1);
        let entry = lab.peers.get_mut(&address).unwrap();
        let after_open = entry.peer.sequencer().out_packet_id();
        let mut buffer = [0; 2048];
        for packet in first_packet..after_open {
            socket.recv(&mut buffer).unwrap();
            match export_state {
                "lost" => entry.exports.received_nak(packet),
                "acked" => entry.exports.received_ack(packet),
                _ => {}
            }
        }
        lab.finalise_connected_players();
        assert_eq!(lab.stats.returning_pawns_restored, 1, "{export_state}");
        assert_eq!(lab.stats.returning_pawn_updates_sent, 1, "{export_state}");
        assert_eq!(lab.stats.returning_pawn_update_errors, 0, "{export_state}");
        let count = lab.peers[&address].peer.sequencer().out_packet_id() - after_open;
        let codec = RepLayoutCodec {
            actor: actor_codec(engine, ByteOrder::Little),
            checksums: false,
            limits: Default::default(),
        };
        let mut exported_component = None;
        let mut health = None;
        let mut reassembler = PartialBunchReassembler::new();
        let mut sequence = 0;
        for _ in 0..count {
            let length = socket.recv(&mut buffer).unwrap();
            let packet = read_packet(
                &buffer[..length],
                &PacketConfig {
                    receiver: Receiver::Client,
                    ..lab.config.packet.packet
                },
            )
            .unwrap();
            for bunch in packet.bunches {
                sequence += 1;
                assert_eq!(bunch.header.channel_index, PAWN_CHANNEL_INDEX);
                assert!(bunch.header.reliable);
                assert!(!bunch.header.open);
                let mut reader = bunch.payload.reader();
                let payload = if bunch.header.has_package_map_exports {
                    let refs = read_export_prefix(&mut reader, &codec.actor).unwrap();
                    assert_eq!(refs.len(), 1);
                    assert!(health.is_none(), "exports precede restored state");
                    exported_component = Some(refs[0].guid);
                    assert!(reader.at_end());
                    Payload::from_writer(super::super::packet::BitWriter::new())
                } else {
                    bunch.payload
                };
                let payload = if bunch.header.partial {
                    let PartialReceive::Complete { bunch, .. } = reassembler
                        .receive_part(sequence, &bunch.header, &payload, Some(4096))
                        .unwrap()
                    else {
                        continue;
                    };
                    bunch.payload
                } else {
                    payload
                };
                let body =
                    read_actor_bunch_body(&mut payload.reader(), false, &codec.actor).unwrap();
                for block in body.blocks {
                    let BlockTarget::SubObject { object, .. } = block.target else {
                        continue; // The UID and pawn-property blocks.
                    };
                    if export_state != "acked" {
                        assert_eq!(exported_component, Some(object.guid));
                    }
                    let properties =
                        read_rep_properties(&mut block.payload.reader(), &layout, &codec).unwrap();
                    health = properties.entries.into_iter().find_map(|property| {
                        (property.handle == h::HEALTH).then_some(property.value)
                    });
                }
            }
        }
        assert_eq!(health, Some(RepValue::Int(37)));
        assert_eq!(exported_component.is_some(), export_state != "acked");
        lab.finalise_connected_players();
        assert_eq!(lab.stats.players_finalised, 1);
        assert_eq!(lab.stats.returning_pawn_updates_sent, 1);
        assert_eq!(lab.claimed_actor_uids(), &[70_001]);
    }
}

#[test]
fn parsed_user_response_survives_publication_pressure_without_reapplication() {
    let mut lab = prospect_refresh_lab();
    lab.config.packet.sent_capacity = 8;
    let (socket, address, mut client) = acked_game_state_peer(&mut lab);
    let state_channel = lab.game_state_open.as_ref().unwrap().channel_index;
    let profile = ReleaseProfile::fixture("3.0.32");
    lab.enable_player_state_open(
        profile
            .player_state_open_plan(PeerLabAdmission::default().map_url)
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    // Reuse the fixture's real settled actor transport for publication;
    // this test concerns pressure and phase transitions, not actor spawn.
    lab.game_state_open = None;
    assert!(lab.send_player_controller_open(address));
    ack_startup_packets(&mut lab, &socket, &mut client);
    let entry = lab.peers.get_mut(&address).unwrap();
    entry.player_state_channel = Some(state_channel);
    entry.player = Some(RegisteredPlayer {
        player_id: 1,
        name_units: "AUTHORED-PROFILE-PRESSURE".encode_utf16().collect(),
    });
    entry.character_slot = Some(17);
    entry.profiles.outstanding = Some(ProfileKind::User);
    let capacity = entry.peer.config().sent_capacity;
    for _ in 0..capacity {
        let datagram = lab
            .peers
            .get_mut(&address)
            .unwrap()
            .peer
            .ack_packet()
            .unwrap();
        assert!(lab.send(&datagram, address));
    }
    let empty_array = || {
        RepValue::DynamicArray(super::super::rep_layout::RepArray {
            count: 0,
            stream: super::super::rep_layout::PropertyStream {
                entries: vec![],
                terminator_checksum: None,
            },
        })
    };
    lab.controller_call(
        address,
        ControllerCall::Profile(ProfileResponse {
            kind: ProfileKind::User,
            success: true,
            values: vec![
                RepValue::Str(ArchiveText::text("authored").unwrap()),
                empty_array(),
                empty_array(),
                empty_array(),
                RepValue::Int(0),
                RepValue::Int(0),
            ],
        }),
    );
    assert_eq!(lab.stats.profile_user_responses_recognized, 1);
    assert_eq!(lab.stats.profile_user_publications_deferred, 1);
    assert_eq!(lab.stats.profile_user_responses_received, 0);
    assert_eq!(lab.readiness_counts().players_awaiting_user_profile, 1);
    assert!(matches!(
        lab.peers[&address].pending_profile_exchange,
        Some(PendingProfileExchange::Publish(ProfileKind::User, _))
    ));
    for _ in 0..4 {
        lab.retry_pending_profile_exchanges();
    }
    assert_eq!(lab.stats.profile_user_publications_deferred, 1);
    ack_startup_packets(&mut lab, &socket, &mut client);
    lab.retry_pending_profile_exchanges();
    assert_eq!(lab.stats.profile_user_responses_received, 1);
    assert_eq!(lab.stats.profile_user_responses_recognized, 1);
    assert_eq!(lab.stats.profile_character_requests_sent, 1);
    for _ in 0..4 {
        lab.retry_pending_profile_exchanges();
    }
    assert_eq!(lab.stats.profile_user_responses_received, 1);
    assert_eq!(lab.stats.profile_character_requests_sent, 1);
    // A late duplicate is observed but cannot publish or advance again.
    lab.controller_call(
        address,
        ControllerCall::Profile(ProfileResponse {
            kind: ProfileKind::User,
            success: true,
            values: vec![],
        }),
    );
    assert_eq!(lab.stats.profile_user_responses_recognized, 2);
    assert_eq!(lab.stats.profile_responses_ignored, 1);
    assert_eq!(lab.stats.profile_user_responses_received, 1);
    let counts = lab.session_counts();
    assert_eq!(counts.profile_user_responses_recognized, 2);
    assert_eq!(counts.profile_user_publications_deferred, 1);
}

#[test]
fn an_outstanding_profile_response_fails_a_peer_without_player_state() {
    let mut lab = prospect_refresh_lab();
    let (_socket, address, _client) = acked_game_state_peer(&mut lab);
    lab.peers.get_mut(&address).unwrap().profiles.outstanding = Some(ProfileKind::User);
    lab.controller_call(
        address,
        ControllerCall::Profile(ProfileResponse {
            kind: ProfileKind::User,
            success: true,
            values: Vec::new(),
        }),
    );
    assert!(!lab.has_peer(address));
    assert_eq!(
        (lab.stats.profile_players_failed, lab.stats.peers_failed),
        (1, 1)
    );
    assert_eq!(lab.stats.profile_user_responses_received, 0);
}

fn receive_game_state_update(
    socket: &UdpSocket,
    lab: &PeerLab,
    mut client: Option<&mut super::super::sequencing::PacketSequencer>,
) -> BTreeMap<u32, RepValue> {
    use super::super::{
        actor_channel::read_actor_bunch_body,
        archive::ByteOrder,
        packet::{PacketConfig, Receiver, read_packet},
        rep_layout::{RepLayoutCodec, RepLayoutLimits, read_rep_properties},
        sequencing::{
            MAX_CONSTRUCTED_PARTIAL_BUNCH_BYTES, PartialBunchReassembler, PartialReceive,
        },
    };
    let session = lab.game_state_open.as_ref().unwrap();
    let mut buffer = [0; 2048];
    let mut reassembler = PartialBunchReassembler::new();
    let mut packet_id: Option<(u16, i64)> = None;
    let mut complete = None;
    for _ in 0..32 {
        let length = socket.recv(&mut buffer).unwrap();
        let packet = read_packet(
            &buffer[..length],
            &PacketConfig {
                receiver: Receiver::Client,
                ..lab.config.packet.packet
            },
        )
        .unwrap();
        if let Some(client) = client.as_mut() {
            let received = client.receive(&packet.header).unwrap().unwrap();
            client.ack_seq(received.sequence, true).unwrap();
        }
        let sequence = packet.header.sequence;
        let id = packet_id.map_or(i64::from(sequence), |(last, id)| {
            id + i64::from(sequence.wrapping_sub(last) & 0x3fff)
        });
        packet_id = Some((sequence, id));
        assert_eq!(packet.bunches.len(), 1, "one GameState update part");
        let bunch = packet.bunches.into_iter().next().unwrap();
        assert_eq!(bunch.header.channel_index, session.channel_index);
        assert!(!bunch.header.open && !bunch.header.reliable);
        if !bunch.header.partial {
            complete = Some(bunch);
            break;
        }
        if let PartialReceive::Complete { bunch, .. } = reassembler
            .receive_part(
                id,
                &bunch.header,
                &bunch.payload,
                Some(MAX_CONSTRUCTED_PARTIAL_BUNCH_BYTES),
            )
            .unwrap()
        {
            complete = Some(bunch);
            break;
        }
    }
    let bunch = complete.expect("bounded GameState update completes");
    let codec = RepLayoutCodec {
        actor: ActorChannelCodec {
            engine_net_version: session.plan.engine_net_version,
            exports_enabled: false,
            has_level: true,
            order: ByteOrder::Little,
            limits: ActorChannelLimits::default(),
        },
        checksums: false,
        limits: RepLayoutLimits::default(),
    };
    let body = read_actor_bunch_body(&mut bunch.payload.reader(), false, &codec.actor).unwrap();
    assert_eq!(body.blocks.len(), 1);
    read_rep_properties(
        &mut body.blocks[0].payload.reader(),
        &session.plan.layout,
        &codec,
    )
    .unwrap()
    .entries
    .into_iter()
    .map(|entry| (entry.handle, entry.value))
    .collect()
}

#[test]
fn update_fallback_preserves_each_viable_group_and_retries_omitted_fields() {
    use super::super::{
        game_state_open::handle, prospect_info::ProspectInfo, time_of_day::RateCurve,
    };
    for oversized_prospect in [true, false] {
        let mut lab = prospect_refresh_lab();
        lab.refresh_active_prospect(&ProspectInfo::default())
            .unwrap();
        lab.enable_networking_status(NetworkingStatus::local(600.0))
            .unwrap();
        lab.enable_time_of_day(TimeOfDayClock::new(
            RateCurve::constant(60.0).unwrap(),
            600.0,
        ))
        .unwrap();
        lab.set_game_state_level_time(1_000).unwrap();
        let (socket, address, _) = acked_game_state_peer(&mut lab);
        let before = lab.peers[&address]
            .game_state
            .as_ref()
            .unwrap()
            .sent
            .clone();
        lab.refresh_active_prospect(&ProspectInfo {
            lobby_name: vec![65],
            elapsed_time: 9,
            insurance: true,
            ..Default::default()
        })
        .unwrap();
        let session = lab.game_state_open.as_mut().unwrap();
        let status = session.networking.as_mut().unwrap();
        status.current_host_player_name = vec![66];
        status.saved_locally = false;
        let prospect = session.active_prospect.as_ref().unwrap().clone();
        let status_values = status.values().unwrap();
        let (omitted, retained, repaired) = if oversized_prospect {
            // Model state that has grown past framing limits after
            // validation, without exposing an invalid public refresh.
            session
                .active_prospect
                .as_mut()
                .unwrap()
                .insert(56, RepValue::Str(ArchiveText::new(&[256; 65_535]).unwrap()));
            (handle::ACTIVE_PROSPECT, status_values, prospect)
        } else {
            status.current_host_player_name = vec![256; 65_535];
            (handle::NETWORKING_STATUS, prospect, status_values)
        };
        let earlier = Instant::now() - Duration::from_secs(2);
        lab.started = earlier;
        session.clock_at = earlier;
        session.updated_at = None;
        let errors = lab.stats.game_state_update_errors;
        let packets = lab.peers[&address].peer.sequencer().out_packet_id();
        lab.update_game_state();
        let update = receive_game_state_update(&socket, &lab, None);
        let retained_due: BTreeMap<_, _> = retained
            .into_iter()
            .filter(|(handle, value)| before.get(handle) != Some(value))
            .collect();
        assert!(!retained_due.is_empty());
        let mut expected = retained_due;
        for clock in [
            handle::WORLD_TIME_SECONDS,
            handle::TIME_OF_DAY,
            handle::LEVEL_TIME_ELAPSED_SEC,
        ] {
            expected.insert(
                clock,
                update.get(&clock).expect("clock survives fallback").clone(),
            );
        }
        assert_eq!(update, expected);
        assert!(update.keys().all(|handle| !omitted.contains(handle)));
        assert_eq!(lab.stats.game_state_update_errors, errors + 1);
        let entry = &lab.peers[&address];
        assert!(!entry.peer.is_failed());
        assert_eq!(entry.peer.sequencer().out_packet_id(), packets + 1);
        let replica = entry.game_state.as_ref().unwrap();
        let mut expected_sent = before.clone();
        expected_sent.extend(update.clone());
        assert_eq!(replica.sent, expected_sent);
        assert_eq!(replica.in_flight.len(), 1);
        assert_eq!(
            replica.in_flight.values().next().unwrap().1,
            update.keys().copied().collect::<Vec<_>>()
        );
        // Remove the framing pressure and force the next paced pass.
        // The omitted group must arrive without another typed refresh.
        let session = lab.game_state_open.as_mut().unwrap();
        if oversized_prospect {
            session.active_prospect = Some(repaired.clone());
        } else {
            session
                .networking
                .as_mut()
                .unwrap()
                .current_host_player_name = vec![66];
        }
        session.updated_at = None;
        lab.update_game_state();
        let retry = receive_game_state_update(&socket, &lab, None);
        let repaired_due: BTreeMap<_, _> = repaired
            .into_iter()
            .filter(|(handle, value)| before.get(handle) != Some(value))
            .collect();
        assert!(!repaired_due.is_empty());
        assert_eq!(
            retry
                .iter()
                .filter(|(handle, _)| omitted.contains(handle))
                .map(|(&handle, value)| (handle, value.clone()))
                .collect::<BTreeMap<_, _>>(),
            repaired_due
        );
        assert!(retry.keys().all(|handle| {
            omitted.contains(handle)
                || [
                    handle::WORLD_TIME_SECONDS,
                    handle::TIME_OF_DAY,
                    handle::LEVEL_TIME_ELAPSED_SEC,
                ]
                .contains(handle)
        }));
        assert_eq!(lab.stats.game_state_update_errors, errors + 1);
    }
}

#[test]
fn explicit_member_edits_and_departures_are_atomic() {
    use super::super::prospect_info::{AssociatedMember, ProspectInfo};
    let member = AssociatedMember {
        user_id: "USER".encode_utf16().collect(),
        ..Default::default()
    };
    let other = AssociatedMember {
        chr_slot: 1,
        ..member.clone()
    };
    let initial = ProspectInfo {
        elapsed_time: 123,
        associated_members: vec![member.clone(), other.clone()],
        ..Default::default()
    };
    let mut lab = prospect_refresh_lab();
    lab.enable_member_refresh(initial.clone()).unwrap();
    let mut edited = member.clone();
    edited.user_id = "user".encode_utf16().collect();
    edited.experience = 42;
    edited.settled = true;
    assert!(lab.edit_prospect_member(edited.clone()).unwrap());
    assert!(!lab.edit_prospect_member(edited.clone()).unwrap());
    // Selection folds ASCII case; publication preserves stored spelling.
    edited.user_id.clone_from(&member.user_id);
    let before = lab.game_state_open.as_ref().unwrap().clone();
    let mut invalid = edited.clone();
    invalid.status = u8::MAX;
    assert!(lab.edit_prospect_member(invalid).is_err());
    let after = lab.game_state_open.as_ref().unwrap();
    assert_eq!(after.member_refresh, before.member_refresh);
    assert_eq!(after.active_prospect, before.active_prospect);
    assert_eq!(after.state.values, before.state.values);
    let current = after.member_refresh.as_ref().unwrap();
    assert_eq!(current.elapsed_time, initial.elapsed_time);
    assert_eq!(
        current.associated_members,
        vec![edited.clone(), other.clone()]
    );
    let folded: Vec<u16> = "user".encode_utf16().collect();
    assert!(lab.remove_prospect_member(&folded, 0).unwrap());
    assert!(!lab.remove_prospect_member(&folded, 0).unwrap());
    assert!(!lab.edit_prospect_member(edited).unwrap());
    let current = lab
        .game_state_open
        .as_ref()
        .unwrap()
        .member_refresh
        .as_ref()
        .unwrap();
    assert_eq!(current.elapsed_time, initial.elapsed_time);
    assert_eq!(current.associated_members, vec![other]);
}

#[test]
fn ambiguous_member_edits_reject_and_departure_removes_all_matching_duplicates() {
    use super::super::prospect_info::{AssociatedMember, ProspectInfo};
    let member = AssociatedMember {
        user_id: "USER".encode_utf16().collect(),
        ..Default::default()
    };
    let mut duplicate = member.clone();
    duplicate.user_id = "user".encode_utf16().collect();
    let other_slot = AssociatedMember {
        chr_slot: 1,
        ..member.clone()
    };
    let mut lab = prospect_refresh_lab();
    lab.enable_member_refresh(ProspectInfo {
        elapsed_time: 73,
        associated_members: vec![member.clone(), duplicate, other_slot.clone()],
        ..Default::default()
    })
    .unwrap();
    let before = lab.game_state_open.as_ref().unwrap().clone();
    let mut edit = member.clone();
    edit.experience = 999;
    assert!(!lab.edit_prospect_member(edit).unwrap());
    let after = lab.game_state_open.as_ref().unwrap();
    assert_eq!(after.member_refresh, before.member_refresh);
    assert_eq!(after.active_prospect, before.active_prospect);
    assert_eq!(after.state.values, before.state.values);
    assert!(lab.remove_prospect_member(&member.user_id, 0).unwrap());
    let current = lab
        .game_state_open
        .as_ref()
        .unwrap()
        .member_refresh
        .as_ref()
        .unwrap();
    assert_eq!(current.elapsed_time, 73);
    assert_eq!(current.associated_members, vec![other_slot]);
}

#[test]
fn edited_membership_repairs_latest_array_and_new_peers_open_current_members() {
    use super::super::{
        archive::ByteOrder,
        prospect_info::{AssociatedMember, ProspectInfo},
        rep_layout::{
            PropertyStream, RepEntry, RepLayoutCodec, encode_rep_properties, read_rep_properties,
        },
    };
    let member = AssociatedMember {
        user_id: "USER".encode_utf16().collect(),
        ..Default::default()
    };
    let mut lab = prospect_refresh_lab();
    lab.enable_member_refresh(ProspectInfo {
        associated_members: vec![member.clone()],
        ..Default::default()
    })
    .unwrap();
    let (_old_socket, old_address, _) = acked_game_state_peer(&mut lab);
    let mut edited = member.clone();
    edited.experience = 42;
    assert!(lab.edit_prospect_member(edited).unwrap());
    let edited_values = lab
        .game_state_open
        .as_ref()
        .unwrap()
        .active_prospect
        .clone()
        .unwrap();
    let changed = lab
        .peers
        .get_mut(&old_address)
        .unwrap()
        .game_state
        .as_mut()
        .unwrap();
    let changing = edited_values
        .iter()
        .map(|(&h, v)| (h, Some(v.clone())))
        .collect::<Vec<_>>();
    let due = changed.due(0.0, &changing);
    assert!(due.contains_key(&59));
    changed.sent(&[10, 11], due, 0.0);
    changed.acked(10);
    assert!(changed.naked(11));
    assert_eq!(changed.due(0.0, &changing).get(&59), edited_values.get(&59));
    // A departure supersedes the lost edit; repair must clear the array.
    assert!(lab.remove_prospect_member(&member.user_id, 0).unwrap());
    let session = lab.game_state_open.as_ref().unwrap();
    let current = session.active_prospect.clone().unwrap();
    let layout = session.plan.layout.clone();
    let version = session.plan.engine_net_version;
    for order in [ByteOrder::Little, ByteOrder::Big] {
        let codec = RepLayoutCodec {
            actor: actor_codec(version, order),
            checksums: false,
            limits: Default::default(),
        };
        let stream = PropertyStream {
            entries: current
                .iter()
                .map(|(&handle, value)| RepEntry {
                    handle,
                    checksum: None,
                    value: value.clone(),
                })
                .collect(),
            terminator_checksum: None,
        };
        let wire = encode_rep_properties(&stream, &layout, &codec).unwrap();
        assert_eq!(
            read_rep_properties(&mut wire.reader(), &layout, &codec).unwrap(),
            stream
        );
    }
    let changing = current
        .iter()
        .map(|(&h, v)| (h, Some(v.clone())))
        .collect::<Vec<_>>();
    let replica = lab
        .peers
        .get_mut(&old_address)
        .unwrap()
        .game_state
        .as_mut()
        .unwrap();
    let repair = replica.due(0.0, &changing);
    assert_eq!(repair.get(&59), current.get(&59));
    replica.sent(&[12], repair, 0.0);
    replica.acked(12);
    assert!(replica.due(0.0, &changing).is_empty());
    // A fresh connection's ACKed open reconstructs the current membership,
    // even while the prior connection retains its independent baseline.
    let (_new_socket, new_address, _) = acked_game_state_peer(&mut lab);
    assert_ne!(new_address, old_address);
    assert_eq!(
        lab.peers[&new_address]
            .game_state
            .as_ref()
            .unwrap()
            .sent
            .get(&59),
        current.get(&59)
    );
}

#[test]
fn explicit_spawn_membership_preserves_members_and_latest_prospect_atomically() {
    use super::super::prospect_info::{AssociatedMember, ProspectInfo, SpawnedProspectMember};
    let member = SpawnedProspectMember {
        user_id: "USER".encode_utf16().collect(),
        account_name: "ACCOUNT".encode_utf16().collect(),
        character_name: "CHARACTER".encode_utf16().collect(),
        chr_slot: i32::MIN,
        experience: i32::MAX,
    };
    assert_eq!(format!("{member:?}"), "SpawnedProspectMember");
    let mut lab = prospect_refresh_lab();
    assert!(lab.complete_prospect_member_spawn(member.clone()).is_err());
    let existing = AssociatedMember {
        user_id: "OTHER".encode_utf16().collect(),
        status: 7,
        settled: true,
        is_currently_playing: true,
        experience: -42,
        ..Default::default()
    };
    lab.enable_member_refresh(ProspectInfo {
        associated_members: vec![existing.clone()],
        ..Default::default()
    })
    .unwrap();
    let latest = ProspectInfo {
        lobby_name: "LATEST".encode_utf16().collect(),
        elapsed_time: 123,
        insurance: true,
        associated_members: vec![existing.clone()],
        ..Default::default()
    };
    lab.refresh_active_prospect(&latest).unwrap();
    assert!(lab.complete_prospect_member_spawn(member.clone()).unwrap());
    let current = lab
        .game_state_open
        .as_ref()
        .unwrap()
        .member_refresh
        .as_ref()
        .unwrap()
        .clone();
    assert_eq!(current.associated_members[0], existing);
    let added = &current.associated_members[1];
    assert_eq!(added.status, 2);
    assert!(!added.settled);
    assert!(!added.is_currently_playing);
    assert_eq!(added.user_id, member.user_id);
    assert_eq!(added.account_name, member.account_name);
    assert_eq!(added.character_name, member.character_name);
    assert_eq!(added.chr_slot, i32::MIN);
    assert_eq!(added.experience, i32::MAX);
    let mut without_addition = current.clone();
    without_addition.associated_members.pop();
    assert_eq!(without_addition, latest);
    let mut duplicate = member.clone();
    duplicate.user_id = "user".encode_utf16().collect();
    duplicate.character_name = "CHANGED".encode_utf16().collect();
    duplicate.experience = 0;
    assert!(!lab.complete_prospect_member_spawn(duplicate).unwrap());
    assert_eq!(
        lab.game_state_open
            .as_ref()
            .unwrap()
            .member_refresh
            .as_ref()
            .unwrap(),
        &current
    );
    let mut other_slot = member.clone();
    other_slot.chr_slot = i32::MAX;
    assert!(lab.complete_prospect_member_spawn(other_slot).unwrap());
    let before = lab.game_state_open.as_ref().unwrap().clone();
    let guids = format!("{:?}", lab.guid_cache);
    for bad_name in [vec![0], vec![65; 65_536], vec![256; 65_535]] {
        let mut invalid = member.clone();
        invalid.user_id = "UNIQUE".encode_utf16().collect();
        invalid.account_name = bad_name;
        assert!(lab.complete_prospect_member_spawn(invalid).is_err());
        let after = lab.game_state_open.as_ref().unwrap();
        assert_eq!(after.member_refresh, before.member_refresh);
        assert_eq!(after.active_prospect, before.active_prospect);
        assert_eq!(after.state.values, before.state.values);
        assert_eq!(format!("{:?}", lab.guid_cache), guids);
    }
    assert_eq!(
        lab.game_state_open
            .as_ref()
            .unwrap()
            .member_refresh
            .as_ref()
            .unwrap()
            .associated_members
            .len(),
        3
    );
}

#[test]
fn prospect_send_pressure_leaves_clocks_running_and_same_refresh_due() {
    use super::super::{
        game_state_open::handle,
        packet::{Packet, PacketConfig, Receiver, read_packet, write_packet},
        prospect_info::{ProspectInfo, SpawnedProspectMember},
        time_of_day::RateCurve,
    };
    let mut lab = prospect_refresh_lab();
    lab.config.packet.sent_capacity = 12;
    lab.enable_time_of_day(TimeOfDayClock::new(
        RateCurve::constant(60.0).unwrap(),
        600.0,
    ))
    .unwrap();
    lab.set_game_state_level_time(1_000).unwrap();
    let (socket, address, mut client) = acked_game_state_peer(&mut lab);
    // Fill exactly six of twelve sent records with disposable ACKs.
    // The client remembers them but reports none until pressure clears.
    let to_client = PacketConfig {
        receiver: Receiver::Client,
        ..lab.config.packet.packet
    };
    for _ in 0..6 {
        let datagram = lab
            .peers
            .get_mut(&address)
            .unwrap()
            .peer
            .ack_packet()
            .unwrap();
        let packet = read_packet(&datagram, &to_client).unwrap();
        let received = client.receive(&packet.header).unwrap().unwrap();
        client.ack_seq(received.sequence, true).unwrap();
    }
    let large = ProspectInfo {
        lobby_name: vec![65; 7_000],
        elapsed_time: 10,
        ..Default::default()
    };
    lab.enable_member_refresh(large.clone()).unwrap();
    lab.refresh_active_prospect(&large).unwrap();
    let member = SpawnedProspectMember {
        user_id: vec![85],
        character_name: vec![67],
        ..Default::default()
    };
    assert!(lab.complete_prospect_member_spawn(member.clone()).unwrap());
    let mut second = member.clone();
    second.chr_slot = 1;
    assert!(lab.complete_prospect_member_spawn(second).unwrap());
    let mut edited = lab
        .game_state_open
        .as_ref()
        .unwrap()
        .member_refresh
        .as_ref()
        .unwrap()
        .associated_members[0]
        .clone();
    edited.experience = 42;
    edited.settled = true;
    assert!(lab.edit_prospect_member(edited.clone()).unwrap());
    assert!(lab.remove_prospect_member(&member.user_id, 1).unwrap());
    let mut previous_world = 0.0;
    let mut previous_clock = 600.0;
    let mut previous_level = 1_000;
    let errors = lab.stats.game_state_update_errors;
    for step in 0..4 {
        // Drive the existing private timestamps, so clock progress and
        // pacing are independent of wall-clock scheduling and sleeps.
        let now = Instant::now();
        lab.started = now - Duration::from_secs(2 + step);
        let session = lab.game_state_open.as_mut().unwrap();
        session.clock_at = now - Duration::from_secs(1);
        session.updated_at = None;
        lab.update_game_state();
        let update = receive_game_state_update(&socket, &lab, Some(&mut client));
        assert_eq!(update.len(), 3, "only the three clocks fit");
        let RepValue::Float(world) = update[&handle::WORLD_TIME_SECONDS] else {
            panic!("world time is a float")
        };
        let RepValue::Float(clock) = update[&handle::TIME_OF_DAY] else {
            panic!("time of day is a float")
        };
        let RepValue::Int(level) = update[&handle::LEVEL_TIME_ELAPSED_SEC] else {
            panic!("level time is an integer")
        };
        assert!(f32::from_bits(world) > previous_world);
        assert!(f32::from_bits(clock) > previous_clock);
        assert!(level > previous_level);
        previous_world = f32::from_bits(world);
        previous_clock = f32::from_bits(clock);
        previous_level = level;
        assert_eq!(lab.stats.game_state_update_errors, errors + step + 1);
    }
    assert_eq!(
        lab.peers[&address]
            .peer
            .sequencer()
            .notify()
            .sent_records()
            .len(),
        10
    );
    assert_eq!(lab.stats.peers_failed, 0);
    assert!(!lab.complete_prospect_member_spawn(member).unwrap());
    assert_eq!(
        lab.game_state_open
            .as_ref()
            .unwrap()
            .member_refresh
            .as_ref()
            .unwrap()
            .associated_members
            .len(),
        1
    );
    // Release every outstanding packet through the normal ACK path,
    // then deliver the same refresh with no replacement or new call.
    let ack = write_packet(
        &Packet {
            magic: 0,
            header: client.header().unwrap(),
            info: None,
            bunches: Vec::new(),
        },
        &lab.config.packet.packet,
    )
    .unwrap();
    client.commit().unwrap();
    lab.buffer[..ack.len()].copy_from_slice(&ack);
    lab.data(address, ack.len(), Instant::now());
    assert!(
        lab.peers[&address]
            .peer
            .sequencer()
            .notify()
            .sent_records()
            .is_empty()
    );
    lab.game_state_open.as_mut().unwrap().updated_at = None;
    lab.update_game_state();
    let update = receive_game_state_update(&socket, &lab, Some(&mut client));
    assert_eq!(update.get(&65), Some(&RepValue::Int(10)));
    assert_eq!(
        update.get(&59),
        active_prospect_update_values(
            lab.game_state_open
                .as_ref()
                .unwrap()
                .member_refresh
                .as_ref()
                .unwrap()
        )
        .unwrap()
        .get(&59)
    );
    assert!(matches!(update.get(&59), Some(RepValue::DynamicArray(array)) if array.count == 1));
    assert!(
        matches!(update.get(&56), Some(RepValue::Str(text)) if text.units() == large.lobby_name)
    );
    assert_eq!(lab.stats.game_state_update_errors, errors + 4);
    assert_eq!(lab.stats.peers_failed, 0);
}

#[test]
fn prospect_refresh_validates_before_publishing_template_or_current_values() {
    use super::super::{
        game_state_open::handle,
        prospect_info::{AssociatedMember, ProspectInfo},
        rep_layout::RepCommand,
    };
    let initial = ProspectInfo {
        elapsed_time: 123,
        insurance: true,
        ..Default::default()
    };
    let mut disabled = PeerLab::bind(0, PeerLabConfig::default()).unwrap();
    assert!(disabled.refresh_active_prospect(&initial).is_err());
    let mut lab = prospect_refresh_lab();
    lab.refresh_active_prospect(&initial).unwrap();
    let before = lab.game_state_open.as_ref().unwrap().clone();
    for invalid in [
        ProspectInfo {
            prospect_state: 8,
            ..Default::default()
        },
        ProspectInfo {
            difficulty: 255,
            ..Default::default()
        },
        ProspectInfo {
            associated_members: vec![AssociatedMember {
                status: 13,
                ..Default::default()
            }],
            ..Default::default()
        },
        ProspectInfo {
            lobby_name: vec![0],
            ..Default::default()
        },
        ProspectInfo {
            lobby_name: vec![65; 65_536],
            ..Default::default()
        },
        // Each string fits the actor block; their combined bytes do not.
        ProspectInfo {
            prospect_id: vec![65; 65_535],
            lobby_name: vec![66; 65_535],
            ..Default::default()
        },
        ProspectInfo {
            associated_members: vec![AssociatedMember::default(); 8_193],
            ..Default::default()
        },
        // Wide text fits 128 KiB, but its framing exceeds the actor block.
        ProspectInfo {
            lobby_name: vec![256; 65_535],
            ..Default::default()
        },
    ] {
        assert!(lab.refresh_active_prospect(&invalid).is_err());
        let after = lab.game_state_open.as_ref().unwrap();
        assert_eq!(after.state.values, before.state.values);
        assert_eq!(after.active_prospect, before.active_prospect);
    }
    // A release layout with the wrong property kind also rejects
    // atomically, even though ProspectInfo itself is correctly typed.
    lab.game_state_open.as_mut().unwrap().plan.layout[50] = RepCommand::Bool;
    assert!(
        lab.refresh_active_prospect(&ProspectInfo::default())
            .is_err()
    );
    assert_eq!(
        lab.game_state_open.as_ref().unwrap().state.values,
        before.state.values
    );
    assert_eq!(
        lab.game_state_open.as_ref().unwrap().active_prospect,
        before.active_prospect
    );
    lab.game_state_open.as_mut().unwrap().plan = before.plan.clone();
    lab.game_state_open.as_mut().unwrap().plan.layout[57] = RepCommand::Enum { bits: 1 };
    assert!(
        lab.refresh_active_prospect(&super::super::prospect_info::ProspectInfo {
            prospect_state: 2,
            ..Default::default()
        })
        .is_err()
    );
    assert_eq!(
        lab.game_state_open.as_ref().unwrap().state.values,
        before.state.values
    );
    assert_eq!(
        lab.game_state_open.as_ref().unwrap().active_prospect,
        before.active_prospect
    );
    lab.game_state_open.as_mut().unwrap().plan = before.plan.clone();
    lab.refresh_active_prospect(&ProspectInfo::default())
        .unwrap();
    let session = lab.game_state_open.as_ref().unwrap();
    assert_eq!(
        session.state.values,
        BTreeMap::from([(handle::SEED, RepValue::Int(7))])
    );
    assert_eq!(session.active_prospect.as_ref().unwrap().len(), 17);
    for handle in handle::ACTIVE_PROSPECT {
        assert_eq!(
            session.active_prospect.as_ref().unwrap().get(&handle),
            class_default(handle).as_ref()
        );
    }
}

#[test]
fn prospect_refresh_rejects_combined_dynamic_open_without_publishing() {
    use super::super::{archive::ByteOrder, prospect_info::ProspectInfo};
    let large = ProspectInfo {
        lobby_name: vec![256; 50_000],
        elapsed_time: 9,
        ..Default::default()
    };
    // The prospect alone fits both the update and the open codecs.
    prospect_refresh_lab()
        .refresh_active_prospect(&large)
        .unwrap();
    let mut lab = prospect_refresh_lab();
    let mut status = NetworkingStatus::local(600.0);
    status.current_host_player_name = vec![256; 20_000];
    lab.enable_networking_status(status).unwrap();
    // The dynamic status alone also fits, with a small current prospect.
    lab.refresh_active_prospect(&ProspectInfo {
        elapsed_time: 123,
        insurance: true,
        ..Default::default()
    })
    .unwrap();
    let before = lab.game_state_open.as_ref().unwrap().clone();
    let mut template = before.state.clone();
    template.values.retain(|handle, _| {
        !super::super::game_state_open::handle::ACTIVE_PROSPECT.contains(handle)
    });
    template
        .values
        .extend(super::super::game_state_open::active_prospect_values(&large).unwrap());
    let assembled = before
        .open_state(&template, lab.started.elapsed().as_secs_f32())
        .unwrap();
    for order in [ByteOrder::Little, ByteOrder::Big] {
        // This is the old dry-run: the template still fits by itself.
        compose_game_state_open(
            &mut lab.guid_cache.clone(),
            &mut ConnectionExports::new(),
            &before.plan,
            &template,
            before.channel_index,
            order,
        )
        .unwrap();
        let error = compose_game_state_open(
            &mut lab.guid_cache.clone(),
            &mut ConnectionExports::new(),
            &before.plan,
            &assembled,
            before.channel_index,
            order,
        )
        .expect_err("the combined actor content block exceeds 128 KiB");
        assert!(format!("{error:#}").contains("payload bit limit"));
    }
    assert!(lab.refresh_active_prospect(&large).is_err());
    let after = lab.game_state_open.as_ref().unwrap();
    assert_eq!(after.state.values, before.state.values);
    assert_eq!(after.active_prospect, before.active_prospect);
}

#[test]
fn advancing_clocks_does_not_refresh_prospect_values_or_member_flags() {
    use super::super::{
        prospect_info::{AssociatedMember, ProspectInfo},
        time_of_day::RateCurve,
    };
    let mut lab = prospect_refresh_lab();
    lab.enable_time_of_day(TimeOfDayClock::new(
        RateCurve::constant(60.0).unwrap(),
        600.0,
    ))
    .unwrap();
    lab.set_game_state_level_time(1_000).unwrap();
    let current = ProspectInfo {
        elapsed_time: 7,
        associated_members: vec![AssociatedMember {
            is_currently_playing: true,
            ..Default::default()
        }],
        ..Default::default()
    };
    lab.refresh_active_prospect(&current).unwrap();
    let before = lab.game_state_open.as_ref().unwrap().clone();
    let earlier = Instant::now() - Duration::from_secs(121);
    lab.started = earlier;
    lab.game_state_open.as_mut().unwrap().clock_at = earlier;
    lab.update_game_state();
    let after = lab.game_state_open.as_ref().unwrap();
    assert!(after.clock.as_ref().unwrap().time_of_day() > 600.0);
    assert_eq!(after.state.values, before.state.values);
    assert_eq!(after.active_prospect, before.active_prospect);
    // Legacy template-only initialization also stays template-only.
    lab.game_state_open.as_mut().unwrap().active_prospect = None;
    lab.game_state_open.as_mut().unwrap().updated_at = None;
    lab.update_game_state();
    assert!(
        lab.game_state_open
            .as_ref()
            .unwrap()
            .active_prospect
            .is_none()
    );
}

#[test]
fn prospect_replica_repairs_latest_fields_after_partial_loss_and_supersession() {
    use super::super::prospect_info::ProspectInfo;
    let changing = |info: &ProspectInfo| {
        active_prospect_update_values(info)
            .unwrap()
            .into_iter()
            .map(|(h, v)| (h, Some(v)))
            .collect::<Vec<_>>()
    };
    let mut replica = GameStateReplica::default();
    for handle in update_handles() {
        if let Some(value) = class_default(handle) {
            replica.sent.insert(handle, value);
        }
    }
    let first = ProspectInfo {
        insurance: true,
        elapsed_time: 5,
        ..Default::default()
    };
    let latest = ProspectInfo {
        insurance: true,
        elapsed_time: 6,
        ..Default::default()
    };
    let due = replica.due(0.0, &changing(&first));
    assert_eq!(due.keys().copied().collect::<Vec<_>>(), [63, 65]);
    replica.sent(&[10, 11, 12], due, 0.0);
    replica.acked(10);
    assert!(replica.due(0.0, &changing(&first)).is_empty());
    let due = replica.due(0.0, &changing(&latest));
    assert_eq!(due.keys().copied().collect::<Vec<_>>(), [65]);
    replica.sent(&[13], due, 0.0);
    // Only insurance still depends on the old partial update. Losing
    // any unacked part repairs it, without replacing the latest elapsed.
    assert!(replica.naked(11));
    assert!(!replica.naked(12));
    let due = replica.due(0.0, &changing(&latest));
    assert_eq!(due.keys().copied().collect::<Vec<_>>(), [63]);
    replica.sent(&[14, 15], due, 0.0);
    replica.acked(13);
    replica.acked(14);
    replica.acked(15);
    assert!(!replica.naked(15));
    assert!(replica.due(0.0, &changing(&latest)).is_empty());
    // Clearing is a new value, and its lost split repairs the clear.
    let cleared = changing(&ProspectInfo::default());
    let due = replica.due(0.0, &cleared);
    replica.sent(&[16, 17], due, 0.0);
    assert!(replica.naked(17));
    assert_eq!(
        replica.due(0.0, &cleared),
        BTreeMap::from([(63, RepValue::Bool(false)), (65, RepValue::Int(0))])
    );
    // A -> B -> A is protected by packet identity, not equal values.
    replica.sent(&[18], replica.due(0.0, &changing(&first)), 0.0);
    replica.sent(&[19], replica.due(0.0, &changing(&latest)), 0.0);
    replica.sent(&[20], replica.due(0.0, &changing(&first)), 0.0);
    assert!(!replica.naked(19));
    assert!(replica.due(0.0, &changing(&first)).is_empty());
}

#[test]
fn game_state_replicas_send_what_is_due_and_repair_only_the_latest() {
    use super::super::game_state_open::handle::{
        LEVEL_TIME_ELAPSED_SEC, TIME_OF_DAY, WORLD_TIME_SECONDS,
    };
    let float = |value: f32| RepValue::Float(value.to_bits());
    let mut replica = GameStateReplica {
        world_time_at: 10.0,
        ..GameStateReplica::default()
    };
    replica.sent.insert(WORLD_TIME_SECONDS, float(10.0));
    replica.sent.insert(TIME_OF_DAY, float(600.0));
    replica
        .sent
        .insert(LEVEL_TIME_ELAPSED_SEC, RepValue::Int(5));
    let changing = |tod: f32, level: i32| {
        [
            (TIME_OF_DAY, Some(float(tod))),
            (LEVEL_TIME_ELAPSED_SEC, Some(RepValue::Int(level))),
        ]
    };
    // Nothing changed and the world time is fresh: nothing is due.
    assert!(replica.due(10.05, &changing(600.0, 5)).is_empty());
    // A changed time of day alone, then the world time once 0.1 s old.
    let due = replica.due(10.05, &changing(600.5, 5));
    assert_eq!(due.keys().copied().collect::<Vec<_>>(), [TIME_OF_DAY]);
    replica.sent(&[1], due, 10.05);
    let due = replica.due(10.1, &changing(601.0, 6));
    assert_eq!(
        due.keys().copied().collect::<Vec<_>>(),
        [WORLD_TIME_SECONDS, TIME_OF_DAY, LEVEL_TIME_ELAPSED_SEC]
    );
    replica.sent(&[2], due, 10.1);
    assert_eq!(replica.world_time_at, 10.1);
    // Packet 1's handle is now carried by packet 2: its nak repairs
    // nothing, and only latest packets stay in flight.
    assert_eq!(replica.in_flight.keys().copied().collect::<Vec<_>>(), [2]);
    assert!(!replica.naked(1));
    // Losing packet 2 makes all three due again, even unchanged.
    assert!(replica.naked(2));
    assert_eq!(replica.due(10.15, &changing(601.0, 6)).len(), 3);
    // An acked packet is forgotten; its nak (never sent) changes nothing.
    let due = replica.due(10.15, &changing(601.0, 6));
    replica.sent(&[3], due, 10.15);
    replica.acked(3);
    assert!(!replica.naked(3));
    assert!(replica.due(10.2, &changing(601.0, 6)).is_empty());
}

#[test]
fn a_split_update_is_lost_by_a_nak_of_any_part_and_cleared_by_all_acks() {
    use super::super::game_state_open::handle::{TIME_OF_DAY, WORLD_TIME_SECONDS};
    let float = |value: f32| RepValue::Float(value.to_bits());
    let changing = |tod: f32| [(TIME_OF_DAY, Some(float(tod)))];
    let split = |replica: &mut GameStateReplica, ids: &[u32], tod: f32, at: f32| {
        let due = replica.due(at, &changing(tod));
        assert_eq!(due.len(), 2, "the world time and the time of day");
        replica.sent(ids, due, at);
    };
    // A nak of any one part makes the values due again, once.
    for lost in [10, 11, 12] {
        let mut replica = GameStateReplica::default();
        split(&mut replica, &[10, 11, 12], 600.0, 1.0);
        assert_eq!(
            replica.in_flight.keys().copied().collect::<Vec<_>>(),
            [10, 11, 12]
        );
        assert!(replica.due(1.05, &changing(600.0)).is_empty());
        for acked in [10, 11, 12].into_iter().filter(|id| *id < lost) {
            replica.acked(acked);
        }
        assert!(replica.naked(lost), "part {lost}");
        assert!(replica.in_flight.is_empty(), "part {lost}");
        assert_eq!(
            replica
                .due(1.05, &changing(600.0))
                .keys()
                .copied()
                .collect::<Vec<_>>(),
            [WORLD_TIME_SECONDS, TIME_OF_DAY]
        );
        // The other parts' naks repair nothing more.
        for other in [10, 11, 12] {
            assert!(!replica.naked(other));
        }
    }
    // Acks of every part clear the update; a later nak changes nothing.
    let mut replica = GameStateReplica::default();
    split(&mut replica, &[20, 21], 600.0, 1.0);
    replica.acked(21);
    assert_eq!(replica.in_flight.keys().copied().collect::<Vec<_>>(), [20]);
    replica.acked(20);
    assert!(replica.in_flight.is_empty());
    assert!(!replica.naked(20) && !replica.naked(21));
    assert!(replica.due(1.05, &changing(600.0)).is_empty());
    // A newer update supersedes a split one: the older parts leave the
    // flight and their naks repair nothing.
    split(&mut replica, &[30, 31], 601.0, 2.0);
    split(&mut replica, &[32], 602.0, 3.0);
    assert_eq!(replica.in_flight.keys().copied().collect::<Vec<_>>(), [32]);
    assert!(!replica.naked(30));
    assert!(replica.due(3.05, &changing(602.0)).is_empty());
}

#[test]
fn config_is_validated() {
    assert!(PeerLabConfig::default().validate().is_ok());
    let with_admission = PeerLabConfig {
        admission: Some(PeerLabAdmission::default()),
        ..PeerLabConfig::default()
    };
    assert!(with_admission.validate().is_ok());
    for max_peers in [0, MAX_PEERS_LIMIT + 1] {
        let config = PeerLabConfig {
            max_peers,
            ..PeerLabConfig::default()
        };
        assert!(config.validate().is_err(), "{max_peers}");
    }
    let config = PeerLabConfig {
        idle_timeout: Duration::ZERO,
        ..PeerLabConfig::default()
    };
    assert!(config.validate().is_err());
    for bytes in [28, MAX_UDP_PAYLOAD_BYTES + 1] {
        let mut config = PeerLabConfig::default();
        config.packet.packet.limits.max_packet_bytes = bytes;
        assert!(config.validate().is_err(), "{bytes}");
    }
    let mut config = PeerLabConfig::default();
    config.packet.sent_capacity = 0;
    assert!(config.validate().is_err());
}
mod controller_field_diagnostic_tests {
    use super::super::super::{
        actor_channel::{NetGuidRef, SubObjectClass},
        archive::ByteOrder,
        packet::{BitWriter, Bunch, BunchHeader, Name, Packet, write_packet},
        sequencing::{PacketSequencer, wire_reliable_sequence},
        session_open::ACTOR_CHANNEL_NAME_INDEX,
    };
    use super::*;

    fn controller_peer(lab: &mut PeerLab) -> (UdpSocket, SocketAddrV4, PacketSequencer) {
        let (socket, address, mut client) = acked_game_state_peer(lab);
        // The base fixture has neither Login identity nor a controller channel.
        // Open the actual controller transport and retain its allocated component
        // GUIDs; never repurpose the fixture's GameState channel as a controller.
        assert!(matches!(
            lab.send_initial_controller_open(address),
            InitialOpenOutcome::Committed(_)
        ));
        ack_startup_packets(lab, &socket, &mut client);
        let entry = lab.peers.get_mut(&address).unwrap();
        entry.player = Some(RegisteredPlayer {
            player_id: i32::from(address.port()),
            name_units: "AUTHORED-DIAGNOSTIC-PLAYER".encode_utf16().collect(),
        });
        assert!(
            entry
                .peer
                .actor_channel_settled(entry.player_controller_channel.unwrap())
        );
        assert!(entry.backend_proxy.is_some());
        assert!(entry.player_data.is_some());
        assert!(entry.bestiary.is_some());
        (socket, address, client)
    }

    fn field_block(target: BlockTarget, bound: u32, fields: &[(u32, Payload)]) -> ContentBlock {
        let mut writer = BitWriter::new();
        for (index, payload) in fields {
            write_field_header(
                &mut writer,
                &FieldHeader {
                    index: *index,
                    payload: payload.clone(),
                },
                bound,
            )
            .unwrap();
        }
        ContentBlock {
            has_rep_layout: false,
            target,
            payload: Payload::from_writer(writer),
        }
    }

    fn subobject(guid: u32) -> BlockTarget {
        BlockTarget::SubObject {
            object: NetGuidRef::plain(guid),
            class: SubObjectClass::ClientSent,
        }
    }

    fn empty_payload() -> Payload {
        Payload::from_writer(BitWriter::new())
    }

    fn friends_payload(order: ByteOrder, friends: &[&str]) -> Payload {
        let argument = if friends.is_empty() {
            RpcArgument::NotSent
        } else {
            RpcArgument::Sent(vec![RpcItem::Array(
                friends
                    .iter()
                    .map(|friend| {
                        vec![RpcItem::Value(RepValue::Str(
                            ArchiveText::text(friend).unwrap(),
                        ))]
                    })
                    .collect(),
            )])
        };
        encode_rpc_payload(
            &server_friends_list_updated_parameters(),
            &[argument],
            &RepLayoutCodec {
                actor: actor_codec(0, order),
                checksums: false,
                limits: Default::default(),
            },
        )
        .unwrap()
    }

    #[test]
    fn friends_response_payload_validation_controls_finalisation() {
        for order in [ByteOrder::Little, ByteOrder::Big] {
            let valid = friends_payload(order, &["AUTHORED-FRIEND"]);
            let mut truncated = BitWriter::new();
            let mut reader = valid.reader();
            for _ in 0..valid.bit_len() - 1 {
                truncated.write_bit(reader.read_bit());
            }
            let mut missing_count = BitWriter::new();
            missing_count.write_bit(true);
            let mut trailing = BitWriter::new();
            trailing.write_bit(false);
            trailing.write_bit(true);
            let mut oversized = BitWriter::new();
            oversized.write_bit(true);
            oversized.write_bits(0xffff, 16);
            for (index, payload) in [
                empty_payload(),
                Payload::from_writer(missing_count),
                Payload::from_writer(truncated),
                Payload::from_writer(trailing),
                Payload::from_writer(oversized),
                friends_payload(order, &[]),
                valid,
            ]
            .into_iter()
            .enumerate()
            {
                let expected_valid = index >= 5;
                let mut lab = prospect_refresh_lab();
                lab.config.packet.codec.order = order;
                let (socket, address, mut client) = controller_peer(&mut lab);
                // The friend response is the last blocker for this player.
                lab.peers.get_mut(&address).unwrap().profiles.completed = true;
                lab.finalise_connected_players();
                assert_eq!(lab.stats.players_finalised, 0);
                let payload = client_actor_body(
                    &lab,
                    vec![field_block(
                        BlockTarget::Actor,
                        BP_ICARUS_PLAYER_CONTROLLER_SURVIVAL_FIELD_INDEX_BOUND,
                        &[(SERVER_FRIENDS_LIST_UPDATED_FIELD_INDEX, payload)],
                    )],
                );
                let raw = wire_reliable_sequence(client.initial_reliable().1 + 1);
                receive_controller_payload(&mut lab, &socket, &mut client, raw, payload);
                lab.finalise_connected_players();
                assert_eq!(
                    lab.peers[&address].profiles.friend_ids_ready,
                    expected_valid
                );
                assert_eq!(lab.stats.friends_lists_received, u64::from(expected_valid));
                assert_eq!(lab.stats.players_finalised, u64::from(expected_valid));
                assert_eq!(
                    lab.stats.actor_bunches_malformed,
                    u64::from(!expected_valid)
                );
                let diagnostics = lab.controller_field_diagnostics(address).unwrap();
                assert_eq!(diagnostics.entries.len(), 1);
                assert_eq!(
                    diagnostics.entries[0].disposition,
                    if expected_valid {
                        ControllerFieldDisposition::Recognized
                    } else {
                        ControllerFieldDisposition::Malformed
                    }
                );
            }
        }
    }

    /// Raw archive string frames: declared count, then units.
    type StringFrames = Vec<(i32, Vec<u16>)>;

    /// Authored friends payload bits: a presence bit, then optionally an
    /// array count and raw string frames.
    fn friends_bits(
        order: ByteOrder,
        array: Option<(u16, StringFrames)>,
        trailing: bool,
    ) -> Payload {
        use super::super::super::rep_layout::{write_u16, write_u32};
        let mut writer = BitWriter::new();
        writer.write_bit(array.is_some());
        if let Some((count, strings)) = array {
            write_u16(&mut writer, count, order);
            for (declared, units) in strings {
                write_u32(&mut writer, declared as u32, order);
                for unit in units {
                    if declared < 0 {
                        write_u16(&mut writer, unit, order);
                    } else {
                        writer.write_bits(u32::from(unit), 8);
                    }
                }
            }
        }
        if trailing {
            writer.write_bit(true);
        }
        Payload::from_writer(writer)
    }

    fn friends_codec(order: ByteOrder) -> RepLayoutCodec {
        RepLayoutCodec {
            actor: actor_codec(0, order),
            checksums: false,
            limits: Default::default(),
        }
    }

    fn rejection_cases(order: ByteOrder) -> Vec<(Payload, FriendsListRejection)> {
        use FriendsListRejection as R;
        let a = u16::from(b'A');
        let mut short_count = BitWriter::new();
        short_count.write_bit(true);
        short_count.write_bits(0, 8);
        vec![
            (empty_payload(), R::Truncated),
            (Payload::from_writer(short_count), R::Truncated),
            (
                friends_bits(order, Some((2, vec![(1, vec![0])])), false),
                R::Truncated,
            ),
            (
                friends_bits(order, Some((1, vec![(5, vec![a, a])])), false),
                R::Truncated,
            ),
            (
                friends_bits(order, Some((4097, vec![])), false),
                R::ArrayCountOverLimit,
            ),
            (
                friends_bits(order, Some((1, vec![(65_537, vec![])])), false),
                R::StringLengthOverLimit,
            ),
            (
                friends_bits(order, Some((1, vec![(i32::MIN, vec![])])), false),
                R::StringLengthOverLimit,
            ),
            (
                friends_bits(order, Some((1, vec![(2, vec![a, a])])), false),
                R::StringMissingTerminator,
            ),
            (
                friends_bits(order, Some((1, vec![(-2, vec![a, a])])), false),
                R::StringMissingTerminator,
            ),
            (
                friends_bits(order, Some((1, vec![(2, vec![0x80, 0])])), false),
                R::StringInvalidEncoding,
            ),
            (
                friends_bits(order, Some((1, vec![(3, vec![a, 0, 0])])), false),
                R::StringInvalidEncoding,
            ),
            (
                friends_bits(order, Some((1, vec![(-2, vec![0xffff, 0])])), false),
                R::StringInvalidEncoding,
            ),
            (friends_bits(order, None, true), R::TrailingBits),
            (
                friends_bits(order, Some((1, vec![(2, vec![a, 0])])), true),
                R::TrailingBits,
            ),
        ]
    }

    fn rejection_counts(reason: FriendsListRejection) -> FriendsListRejectionCounts {
        let mut counts = FriendsListRejectionCounts::default();
        counts.record(reason);
        counts
    }

    #[test]
    fn friends_rejection_counts_map_each_reason_to_one_key() {
        use FriendsListRejection as R;
        let reasons = [
            R::Truncated,
            R::ArrayCountOverLimit,
            R::EntryLimitExceeded,
            R::StringLengthOverLimit,
            R::StringMissingTerminator,
            R::StringInvalidEncoding,
            R::TrailingBits,
            R::Unclassified,
        ];
        let mut keys = std::collections::BTreeSet::new();
        for reason in reasons {
            let value = serde_json::to_value(rejection_counts(reason)).unwrap();
            let object = value.as_object().unwrap();
            assert_eq!(object.len(), reasons.len());
            let set: Vec<_> = object.iter().filter(|(_, count)| **count != 0).collect();
            assert_eq!(set.len(), 1);
            assert_eq!(set[0].1, 1);
            assert!(keys.insert(set[0].0.clone()));
        }
        let mut saturated = FriendsListRejectionCounts {
            trailing_bits: u64::MAX,
            ..Default::default()
        };
        saturated.record(R::TrailingBits);
        assert_eq!(saturated.trailing_bits, u64::MAX);
    }

    #[test]
    fn friends_rejection_classifier_agrees_with_the_parser() {
        let parameters = server_friends_list_updated_parameters();
        for order in [ByteOrder::Little, ByteOrder::Big] {
            let codec = friends_codec(order);
            for (payload, reason) in rejection_cases(order) {
                assert!(parse_rpc_payload(&payload, &parameters, &codec).is_err());
                assert_eq!(friends_list_rejection(&payload, &codec), reason);
            }
            for valid in [
                friends_payload(order, &[]),
                friends_payload(order, &["AUTHORED-FRIEND", "B"]),
                friends_bits(order, Some((0, vec![])), false),
                friends_bits(
                    order,
                    Some((2, vec![(0, vec![]), (-2, vec![0xd800, 0])])),
                    false,
                ),
            ] {
                assert!(parse_rpc_payload(&valid, &parameters, &codec).is_ok());
                assert_eq!(
                    friends_list_rejection(&valid, &codec),
                    FriendsListRejection::Unclassified
                );
            }
            // The entry budget: one for the array, one per string.
            let mut tight = friends_codec(order);
            tight.limits.max_entries = 2;
            let two = friends_bits(order, Some((2, vec![(0, vec![]), (0, vec![])])), false);
            assert!(parse_rpc_payload(&two, &parameters, &tight).is_err());
            assert_eq!(
                friends_list_rejection(&two, &tight),
                FriendsListRejection::EntryLimitExceeded
            );
            // Deterministic authored variations: the classifier names a
            // reason exactly when the parser rejects.
            let mut state = 0x9e37_79b9_u32;
            let mut next = |bound: u32| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                state % bound
            };
            let declared = [-3, -2, -1, 0, 1, 2, 3, 65_537, i32::MIN];
            let units = [0, u16::from(b'A'), 0x80, 0xffff];
            for _ in 0..2000 {
                let mut strings = Vec::new();
                for _ in 0..next(4) {
                    let count = declared[next(declared.len() as u32) as usize];
                    let length = (count.unsigned_abs() as usize).min(3);
                    let body: Vec<u16> = (0..length)
                        .map(|_| units[next(units.len() as u32) as usize])
                        .collect();
                    strings.push((count, body));
                }
                let count = if next(16) == 0 {
                    4097
                } else {
                    strings.len() as u16 + u16::from(next(4) == 0)
                };
                let full = friends_bits(
                    order,
                    (next(8) != 0).then_some((count, strings)),
                    next(4) == 0,
                );
                let keep = full.bit_len() - next(4).min(full.bit_len() as u32) as usize;
                let mut cut = BitWriter::new();
                let mut reader = full.reader();
                for _ in 0..keep {
                    cut.write_bit(reader.read_bit());
                }
                let payload = Payload::from_writer(cut);
                assert_eq!(
                    parse_rpc_payload(&payload, &parameters, &codec).is_err(),
                    friends_list_rejection(&payload, &codec) != FriendsListRejection::Unclassified,
                );
            }
        }
    }

    #[test]
    fn rejected_friends_responses_count_exactly_their_reason() {
        for order in [ByteOrder::Little, ByteOrder::Big] {
            for (payload, reason) in rejection_cases(order) {
                let mut lab = prospect_refresh_lab();
                lab.config.packet.codec.order = order;
                let (socket, address, mut client) = controller_peer(&mut lab);
                lab.peers.get_mut(&address).unwrap().profiles.completed = true;
                let payload = client_actor_body(
                    &lab,
                    vec![field_block(
                        BlockTarget::Actor,
                        BP_ICARUS_PLAYER_CONTROLLER_SURVIVAL_FIELD_INDEX_BOUND,
                        &[(SERVER_FRIENDS_LIST_UPDATED_FIELD_INDEX, payload)],
                    )],
                );
                let raw = wire_reliable_sequence(client.initial_reliable().1 + 1);
                receive_controller_payload(&mut lab, &socket, &mut client, raw, payload);
                lab.finalise_connected_players();
                assert_eq!(
                    lab.stats.friends_list_rejections,
                    rejection_counts(reason),
                    "{reason:?}"
                );
                assert_eq!(
                    lab.session_counts().friends_list_rejections,
                    rejection_counts(reason)
                );
                assert_eq!(lab.stats.actor_bunches_malformed, 1);
                assert_eq!(lab.stats.friends_lists_received, 0);
                assert_eq!(lab.stats.friends_lists_ignored_unregistered, 0);
                assert_eq!(lab.stats.friends_lists_ignored_already_ready, 0);
                assert_eq!(lab.stats.players_finalised, 0);
                assert!(lab.has_peer(address));
                assert!(!lab.peers[&address].profiles.friend_ids_ready);
            }
            // A malformed Begin is not a friends rejection.
            let mut lab = prospect_refresh_lab();
            lab.config.packet.codec.order = order;
            let (socket, address, mut client) = controller_peer(&mut lab);
            let mut begin = BitWriter::new();
            begin.write_bit(true);
            let payload = client_actor_body(
                &lab,
                vec![field_block(
                    BlockTarget::Actor,
                    BP_ICARUS_PLAYER_CONTROLLER_SURVIVAL_FIELD_INDEX_BOUND,
                    &[(
                        SERVER_BEGIN_PLAYER_INITIALISATION_FIELD_INDEX,
                        Payload::from_writer(begin),
                    )],
                )],
            );
            let raw = wire_reliable_sequence(client.initial_reliable().1 + 1);
            receive_controller_payload(&mut lab, &socket, &mut client, raw, payload);
            assert_eq!(lab.stats.actor_bunches_malformed, 1);
            assert_eq!(
                lab.stats.friends_list_rejections,
                FriendsListRejectionCounts::default()
            );
            assert!(lab.has_peer(address));
        }
    }

    fn receive_controller_payload(
        lab: &mut PeerLab,
        socket: &UdpSocket,
        client: &mut PacketSequencer,
        reliable_raw: u32,
        payload: Payload,
    ) {
        let SocketAddr::V4(address) = socket.local_addr().unwrap() else {
            unreachable!("IPv4 fixture")
        };
        let bytes = write_packet(
            &Packet {
                magic: 0,
                header: client.header().unwrap(),
                info: None,
                bunches: vec![Bunch {
                    header: BunchHeader {
                        control: false,
                        open: false,
                        close: false,
                        close_reason: None,
                        paused: false,
                        reliable: true,
                        channel_index: lab.peers[&address].player_controller_channel.unwrap(),
                        has_package_map_exports: false,
                        has_must_be_mapped_guids: false,
                        partial: false,
                        reliable_sequence_raw: Some(reliable_raw),
                        partial_initial: false,
                        partial_final: false,
                        channel_name: Some(Name::Hardcoded(ACTOR_CHANNEL_NAME_INDEX)),
                    },
                    payload,
                }],
            },
            &lab.config.packet.packet,
        )
        .unwrap();
        client.commit().unwrap();
        receive_authored_at(lab, socket, &bytes, Instant::now());
    }

    fn client_actor_body(lab: &PeerLab, blocks: Vec<ContentBlock>) -> Payload {
        encode_actor_bunch_body(
            &ActorBunchBody {
                spawn: None,
                blocks,
            },
            &actor_codec(
                lab.player_controller_open
                    .as_ref()
                    .unwrap()
                    .engine_net_version,
                lab.config.packet.codec.order,
            ),
        )
        .unwrap()
    }

    fn assert_diagnostics_eq(
        actual: &ControllerFieldDiagnostics,
        expected: &ControllerFieldDiagnostics,
    ) {
        assert_eq!(actual.skipped_blocks, expected.skipped_blocks);
        assert_eq!(
            actual.skipped_blocks_overflow,
            expected.skipped_blocks_overflow
        );
        assert_eq!(actual.entries, expected.entries);
        assert_eq!(actual.overflow, expected.overflow);
        assert_eq!(actual.malformed_bunches, expected.malformed_bunches);
        assert_eq!(actual.body_parse_failures, expected.body_parse_failures);
        assert_eq!(
            actual.block_payload_limit_rejections,
            expected.block_payload_limit_rejections
        );
        assert_eq!(
            actual.block_payload_length_rejections,
            expected.block_payload_length_rejections
        );
        assert_eq!(
            actual.max_rejected_block_payload_bits,
            expected.max_rejected_block_payload_bits
        );
        assert_eq!(
            actual.unknown_subobject_blocks,
            expected.unknown_subobject_blocks
        );
    }

    #[test]
    fn private_rpc82_opt_in_preserves_dispatch_and_suppresses_reliable_duplicates() {
        use super::super::super::selected_rpc_capture::PrivateRpc82Capture;
        use std::io::{Read, Seek, SeekFrom};
        let mut outcomes = Vec::new();
        for mode in 0..3 {
            let enabled = mode > 0;
            let mut lab = prospect_refresh_lab();
            let mut file = tempfile::tempfile().unwrap();
            let readonly = tempfile::NamedTempFile::new().unwrap();
            if enabled {
                lab.install_private_rpc82_capture(PrivateRpc82Capture::from_file(if mode == 2 {
                    std::fs::File::open(readonly.path()).unwrap()
                } else {
                    file.try_clone().unwrap()
                }))
                .unwrap();
            }
            let (socket, address, mut client) = controller_peer(&mut lab);
            lab.peers.get_mut(&address).unwrap().connection_id = Some(1);
            let mut selected = BitWriter::new();
            for bit in 0..612 {
                selected.write_bit(bit % 3 == 0);
            }
            let payload = client_actor_body(
                &lab,
                vec![field_block(
                    BlockTarget::Actor,
                    BP_ICARUS_PLAYER_CONTROLLER_SURVIVAL_FIELD_INDEX_BOUND,
                    &[
                        (82, Payload::from_writer(selected)),
                        (SERVER_SHORT_TIMEOUT_FIELD_INDEX, empty_payload()),
                    ],
                )],
            );
            let raw = wire_reliable_sequence(client.initial_reliable().1 + 1);
            receive_controller_payload(&mut lab, &socket, &mut client, raw, payload.clone());
            receive_controller_payload(&mut lab, &socket, &mut client, raw, payload);
            assert!(lab.has_peer(address));
            assert_eq!(lab.stats.actor_duplicate_reliable, 1);
            assert_eq!(lab.stats.controller_fields_ignored, 1);
            let diagnostics = lab.controller_field_diagnostics(address).unwrap();
            assert_eq!(
                diagnostics
                    .entries
                    .iter()
                    .find(|field| field.index == 82)
                    .unwrap()
                    .disposition,
                ControllerFieldDisposition::Ignored
            );
            outcomes.push(serde_json::to_value(diagnostics).unwrap());
            if mode == 1 {
                assert_eq!(lab.finish_private_rpc82_capture().unwrap().observations, 1);
                file.seek(SeekFrom::Start(0)).unwrap();
                let mut text = String::new();
                file.read_to_string(&mut text).unwrap();
                assert_eq!(text.lines().count(), 1);
                let row: serde_json::Value = serde_json::from_str(text.trim()).unwrap();
                assert_eq!(row["payload_bits"], 612);
                assert!(!text.contains("AUTHORED-DIAGNOSTIC-PLAYER"));
            } else if mode == 2 {
                let counts = lab.finish_private_rpc82_capture().unwrap();
                assert_eq!(counts.observations, 0);
                assert_eq!(counts.write_errors, 1);
                assert_eq!(counts.incomplete, 1);
                assert!(lab.has_peer(address));
            } else {
                assert!(lab.private_rpc82_capture_counts().is_none());
                assert_eq!(file.metadata().unwrap().len(), 0);
            }
        }
        assert_eq!(outcomes[0], outcomes[1]);
        assert_eq!(outcomes[0], outcomes[2]);
    }

    #[test]
    fn private_capture_and_live_dispatch_reassemble_independently_once() {
        use super::super::super::{
            selected_rpc_capture::PrivateRpc82Capture, sequencing::split_outgoing_bunch,
        };
        let mut lab = prospect_refresh_lab();
        let file = tempfile::tempfile().unwrap();
        lab.install_private_rpc82_capture(PrivateRpc82Capture::from_file(
            file.try_clone().unwrap(),
        ))
        .unwrap();
        let (socket, address, mut client) = controller_peer(&mut lab);
        lab.peers.get_mut(&address).unwrap().connection_id = Some(1);
        let mut selected = BitWriter::new();
        for _ in 0..612 {
            selected.write_bit(true);
        }
        let payload = client_actor_body(
            &lab,
            vec![field_block(
                BlockTarget::Actor,
                BP_ICARUS_PLAYER_CONTROLLER_SURVIVAL_FIELD_INDEX_BOUND,
                &[(82, Payload::from_writer(selected))],
            )],
        );
        let channel = lab.peers[&address].player_controller_channel.unwrap();
        let whole = Bunch {
            header: BunchHeader {
                control: false,
                open: false,
                close: false,
                close_reason: None,
                paused: false,
                reliable: true,
                channel_index: channel,
                has_package_map_exports: false,
                has_must_be_mapped_guids: false,
                partial: false,
                reliable_sequence_raw: Some(wire_reliable_sequence(
                    client.initial_reliable().1 + 1,
                )),
                partial_initial: false,
                partial_final: false,
                channel_name: Some(Name::Hardcoded(ACTOR_CHANNEL_NAME_INDEX)),
            },
            payload,
        };
        let parts = split_outgoing_bunch(&whole, &[], 512, Some(16 * 1024)).unwrap();
        assert_eq!(parts.len(), 2);
        // Queue the final part first. Neither an incomplete nor a queued
        // part may produce a record. Re-send it later as a duplicate.
        for (index, part) in [parts[1].clone(), parts[0].clone(), parts[1].clone()]
            .into_iter()
            .enumerate()
        {
            let bytes = write_packet(
                &Packet {
                    magic: 0,
                    header: client.header().unwrap(),
                    info: None,
                    bunches: vec![part],
                },
                &lab.config.packet.packet,
            )
            .unwrap();
            client.commit().unwrap();
            receive_authored_at(&mut lab, &socket, &bytes, Instant::now());
            assert_eq!(
                lab.private_rpc82_capture_counts().unwrap().observations,
                u64::from(index > 0)
            );
        }
        assert_eq!(lab.finish_private_rpc82_capture().unwrap().incomplete, 0);
        assert_eq!(lab.stats.actor_duplicate_reliable, 1);
        assert_eq!(lab.stats.actor_bunches_dropped, 0);
        assert_eq!(lab.stats.actor_partial_completed, 1);
        assert_eq!(lab.stats.controller_fields_ignored, 1);
        let lifetime = lab.controller_receive_diagnostics();
        assert_eq!(lifetime.actor_partial_completed, 1);
        assert_eq!(lifetime.actor_duplicate_reliable, 1);
        assert_eq!(lifetime.fields, lab.controller_field_totals());
        assert_eq!(
            lab.controller_field_diagnostics(address)
                .unwrap()
                .entries
                .len(),
            1
        );
        assert!(lab.has_peer(address));
    }

    #[test]
    fn fragmented_authored_user_profile_dispatches_once_after_complete_atomic_parse() {
        use super::super::super::{
            probe_initialise::authored_user_profile,
            rep_rpc::backend_profile_response_parameters,
            sequencing::{MAX_CONSTRUCTED_PARTIAL_BUNCH_BYTES, split_outgoing_bunch},
        };
        for failure in 0..2 {
            let malformed_tail = failure != 0;
            let mut lab = prospect_refresh_lab();
            let (socket, address, mut client) = controller_peer(&mut lab);
            let backend = lab.peers[&address].backend_proxy.unwrap();
            let engine = lab
                .player_controller_open
                .as_ref()
                .unwrap()
                .engine_net_version;
            let codec = RepLayoutCodec {
                actor: actor_codec(engine, lab.config.packet.codec.order),
                checksums: false,
                limits: Default::default(),
            };
            let mut values = vec![RpcItem::Value(RepValue::Bool(true))];
            let mut profile = authored_user_profile().unwrap();
            profile[0] = RpcItem::Value(RepValue::Str(
                ArchiveText::text(&"A".repeat(10000)).unwrap(),
            ));
            values.extend(profile);
            let response = encode_rpc_payload(
                &backend_profile_response_parameters(false),
                &[RpcArgument::Sent(values)],
                &codec,
            )
            .unwrap();
            let valid = client_actor_body(
                &lab,
                vec![field_block(
                    subobject(backend),
                    BACKEND_PROXY_FIELD_INDEX_BOUND,
                    &[(RESPONSE_GET_USER_PROFILE, response)],
                )],
            );
            let payload = if malformed_tail {
                let mut writer = BitWriter::new();
                let mut reader = valid.reader();
                while reader.remaining() > 0 {
                    writer.write_bit(reader.read_bit());
                }
                // A stray bit after a response larger than the former
                // 8 KiB cap starts a truncated content header, so the
                // whole body fails without applying the response.
                writer.write_bit(true);
                Payload::from_writer(writer)
            } else {
                valid.clone()
            };
            let first = client.initial_reliable().1 + 1;
            let whole = Bunch {
                header: BunchHeader {
                    control: false,
                    open: false,
                    close: false,
                    close_reason: None,
                    paused: false,
                    reliable: true,
                    channel_index: lab.peers[&address].player_controller_channel.unwrap(),
                    has_package_map_exports: false,
                    has_must_be_mapped_guids: false,
                    partial: false,
                    reliable_sequence_raw: Some(wire_reliable_sequence(first)),
                    partial_initial: false,
                    partial_final: false,
                    channel_name: Some(Name::Hardcoded(ACTOR_CHANNEL_NAME_INDEX)),
                },
                payload,
            };
            let parts =
                split_outgoing_bunch(&whole, &[], 7000, Some(MAX_CONSTRUCTED_PARTIAL_BUNCH_BYTES))
                    .unwrap();
            assert!(parts.len() >= 3);
            // Final-first queues; gapfill completes exactly once. A repeat
            // of the final reliable value must never redispatch the RPC.
            let mut order = vec![parts.last().unwrap().clone()];
            order.extend(parts[..parts.len() - 1].iter().cloned());
            order.push(parts.last().unwrap().clone());
            for (index, part) in order.into_iter().enumerate() {
                let bytes = write_packet(
                    &Packet {
                        magic: 0,
                        header: client.header().unwrap(),
                        info: None,
                        bunches: vec![part],
                    },
                    &lab.config.packet.packet,
                )
                .unwrap();
                client.commit().unwrap();
                receive_authored_at(&mut lab, &socket, &bytes, Instant::now());
                if index < parts.len() - 1 {
                    assert_eq!(lab.stats.profile_user_responses_recognized, 0);
                }
            }
            assert_eq!(lab.stats.actor_partial_completed, 1);
            assert_eq!(lab.stats.actor_duplicate_reliable, 1);
            assert_eq!(
                lab.stats.profile_user_responses_recognized,
                u64::from(!malformed_tail)
            );
            // No request was outstanding: successful parsing reaches the
            // existing phase guard, without profile publication or world claims.
            assert_eq!(
                lab.stats.profile_responses_ignored,
                u64::from(!malformed_tail)
            );
            assert_eq!(lab.stats.profile_user_responses_received, 0);
            if malformed_tail {
                assert_eq!(
                    lab.controller_field_diagnostics(address)
                        .unwrap()
                        .malformed_bunches,
                    1
                );
                let body = lab.controller_field_diagnostics(address).unwrap();
                assert_eq!(body.body_parse_failures, 1);
                assert_eq!(body.block_payload_limit_rejections, 0);
                assert_eq!(body.block_payload_length_rejections, 0);
                assert_eq!(body.max_rejected_block_payload_bits, 0);
                // The failed complete body does not poison the next sequence.
                let recovery = split_outgoing_bunch(
                    &Bunch {
                        header: BunchHeader {
                            reliable_sequence_raw: Some(wire_reliable_sequence(
                                first + parts.len() as u32,
                            )),
                            ..whole.header
                        },
                        payload: valid,
                    },
                    &[],
                    7000,
                    Some(MAX_CONSTRUCTED_PARTIAL_BUNCH_BYTES),
                )
                .unwrap();
                for part in recovery {
                    let bytes = write_packet(
                        &Packet {
                            magic: 0,
                            header: client.header().unwrap(),
                            info: None,
                            bunches: vec![part],
                        },
                        &lab.config.packet.packet,
                    )
                    .unwrap();
                    client.commit().unwrap();
                    receive_authored_at(&mut lab, &socket, &bytes, Instant::now());
                }
                assert_eq!(lab.stats.profile_user_responses_recognized, 1);
                assert_eq!(lab.stats.profile_responses_ignored, 1);
            }
        }
    }

    #[test]
    fn body_rejection_diagnostics_distinguish_available_oversize_from_bad_declaration() {
        use super::super::super::actor_channel::ContentBlockPayloadReadError;
        let limit = actor_codec(17, ByteOrder::Little).limits.max_payload_bits;
        let above = u32::try_from(limit + 1).unwrap();
        let mut omitted = BitWriter::new();
        omitted.write_bit(false);
        let valid = encode_actor_bunch_body(
            &ActorBunchBody {
                spawn: None,
                blocks: vec![field_block(
                    subobject(5),
                    BACKEND_PROXY_FIELD_INDEX_BOUND,
                    &[(RESPONSE_GET_USER_PROFILE, Payload::from_writer(omitted))],
                )],
            },
            &actor_codec(17, ByteOrder::Little),
        )
        .unwrap();
        for (declared, available) in [(above, limit + 1), (above, 0), (16, 0)] {
            let mut writer = BitWriter::new();
            let mut prefix = valid.reader();
            while prefix.remaining() > 0 {
                writer.write_bit(prefix.read_bit());
            }
            writer.write_bit(false); // no rep layout
            writer.write_bit(true); // actor block, no object identity
            writer.write_int_packed(declared);
            for _ in 0..available {
                writer.write_bit(false);
            }
            let mut diagnostics = ControllerFieldDiagnostics::default();
            let error = controller_initialisation_calls(
                &Payload::from_writer(writer),
                17,
                ByteOrder::Little,
                [Some(5), None, None],
                &mut diagnostics,
            )
            .err()
            .unwrap();
            let rejection = error
                .downcast_ref::<ContentBlockPayloadReadError>()
                .unwrap();
            if declared as usize > limit {
                assert_eq!(
                    *rejection,
                    ContentBlockPayloadReadError::Limit {
                        declared_bits: declared as usize,
                        available_bits: available,
                        limit_bits: limit,
                    }
                );
                assert_eq!(
                    rejection.to_string(),
                    "content block payload exceeds the payload bit limit"
                );
            } else {
                assert_eq!(
                    *rejection,
                    ContentBlockPayloadReadError::Truncated {
                        declared_bits: declared as usize,
                        available_bits: available,
                    }
                );
                assert_eq!(
                    rejection.to_string(),
                    "content block payload exceeds the bunch"
                );
            }
            assert_eq!(diagnostics.body_parse_failures, 1);
            assert_eq!(
                diagnostics.block_payload_limit_rejections,
                u64::from(available > 0)
            );
            assert_eq!(
                diagnostics.block_payload_length_rejections,
                u64::from(available == 0)
            );
            assert_eq!(
                diagnostics.max_rejected_block_payload_bits,
                if available > 0 {
                    u64::from(declared)
                } else {
                    0
                }
            );
            // Complete-body staging rejects even the valid UserProfile prefix.
            assert!(diagnostics.entries.is_empty());
        }
    }

    #[test]
    fn malformed_profile_and_unknown_target_do_not_dispatch_profile_calls() {
        let mut lab = prospect_refresh_lab();
        let (socket, address, mut client) = controller_peer(&mut lab);
        let entry = &lab.peers[&address];
        let backend = entry.backend_proxy.unwrap();
        let components = [entry.backend_proxy, entry.player_data, entry.bestiary];
        let unknown = (2..)
            .find(|guid| !components.contains(&Some(*guid)))
            .unwrap();
        // Identical malformed parameter bits on an unknown target must be
        // skipped, while the permitted target must diagnose parse failure.
        let payload = client_actor_body(
            &lab,
            vec![
                field_block(
                    subobject(unknown),
                    BACKEND_PROXY_FIELD_INDEX_BOUND,
                    &[(RESPONSE_GET_USER_PROFILE, empty_payload())],
                ),
                field_block(
                    subobject(backend),
                    BACKEND_PROXY_FIELD_INDEX_BOUND,
                    &[(RESPONSE_GET_USER_PROFILE, empty_payload())],
                ),
            ],
        );
        let raw = wire_reliable_sequence(client.initial_reliable().1 + 1);
        receive_controller_payload(&mut lab, &socket, &mut client, raw, payload);
        let diagnostics = lab.controller_field_diagnostics(address).unwrap();
        assert_eq!(diagnostics.unknown_subobject_blocks, 1);
        assert_eq!(diagnostics.malformed_bunches, 1);
        assert!(
            diagnostics
                .entries
                .iter()
                .any(|entry| entry.index == RESPONSE_GET_USER_PROFILE
                    && entry.disposition == ControllerFieldDisposition::Malformed)
        );
        assert_eq!(lab.stats.profile_user_responses_recognized, 0);
        assert_eq!(lab.stats.profile_user_responses_received, 0);
        assert_eq!(lab.stats.profile_responses_ignored, 0);
        assert!(lab.has_peer(address));
        assert_eq!(lab.controller_field_totals().unknown_subobject_blocks, 1);
        // An omitted response struct is valid and retains Success=false.
        // With no request outstanding it is recognized, then phase-rejected.
        let mut writer = BitWriter::new();
        writer.write_bit(false);
        let payload = client_actor_body(
            &lab,
            vec![field_block(
                subobject(backend),
                BACKEND_PROXY_FIELD_INDEX_BOUND,
                &[(RESPONSE_GET_USER_PROFILE, Payload::from_writer(writer))],
            )],
        );
        receive_controller_payload(
            &mut lab,
            &socket,
            &mut client,
            wire_reliable_sequence(raw + 1),
            payload,
        );
        assert_eq!(lab.stats.profile_user_responses_recognized, 1);
        assert_eq!(lab.stats.profile_responses_ignored, 1);
        assert_eq!(lab.stats.profile_user_responses_received, 0);
    }

    #[test]
    fn short_timeout_layout_requires_exactly_empty_parameters() {
        let parameters =
            player_controller_rpc_parameters(SERVER_SHORT_TIMEOUT_FIELD_INDEX).unwrap();
        assert!(parameters.is_empty());
        assert!(player_controller_rpc_parameters(82).is_none());
        for order in [ByteOrder::Little, ByteOrder::Big] {
            let codec = RepLayoutCodec {
                actor: actor_codec(0, order),
                checksums: false,
                limits: Default::default(),
            };
            assert!(
                parse_rpc_payload(&empty_payload(), &parameters, &codec)
                    .unwrap()
                    .is_empty()
            );
            assert_eq!(
                encode_rpc_payload(&parameters, &[], &codec)
                    .unwrap()
                    .bit_len(),
                0
            );
            for bit in [false, true] {
                let mut writer = BitWriter::new();
                writer.write_bit(bit);
                let nonempty = Payload::from_writer(writer);
                assert!(parse_rpc_payload(&nonempty, &parameters, &codec).is_err());
            }
        }
    }

    #[test]
    fn short_timeout_observation_is_controller_only_and_does_not_change_timeout_policy() {
        let mut lab = prospect_refresh_lab();
        let (socket, address, mut client) = controller_peer(&mut lab);
        let entry = &lab.peers[&address];
        let backend = entry.backend_proxy.unwrap();
        let components = [entry.backend_proxy, entry.player_data, entry.bestiary];
        // Deterministically choose a nondefault reference outside all
        // permitted subobjects; unknown objects are skipped before parsing.
        let unknown = (2..)
            .find(|guid| !components.contains(&Some(*guid)))
            .unwrap();
        let timeout = lab.config.idle_timeout;
        let payload = client_actor_body(
            &lab,
            vec![
                field_block(
                    subobject(backend),
                    BACKEND_PROXY_FIELD_INDEX_BOUND,
                    &[(SERVER_SHORT_TIMEOUT_FIELD_INDEX, empty_payload())],
                ),
                field_block(
                    subobject(unknown),
                    BP_ICARUS_PLAYER_CONTROLLER_SURVIVAL_FIELD_INDEX_BOUND,
                    &[(SERVER_SHORT_TIMEOUT_FIELD_INDEX, empty_payload())],
                ),
                field_block(
                    BlockTarget::Actor,
                    BP_ICARUS_PLAYER_CONTROLLER_SURVIVAL_FIELD_INDEX_BOUND,
                    &[
                        (SERVER_SHORT_TIMEOUT_FIELD_INDEX, empty_payload()),
                        (SERVER_SHORT_TIMEOUT_FIELD_INDEX, empty_payload()),
                        (82, empty_payload()),
                    ],
                ),
            ],
        );
        let raw = wire_reliable_sequence(client.initial_reliable().1 + 1);
        receive_controller_payload(&mut lab, &socket, &mut client, raw, payload);
        let diagnostics = lab.controller_field_diagnostics(address).unwrap();
        assert_eq!(diagnostics.entries.len(), 3);
        assert_eq!(diagnostics.unknown_subobject_blocks, 1);
        assert_eq!(diagnostics.malformed_bunches, 0);
        assert_eq!(diagnostics.overflow, 0);
        let recognized = diagnostics
            .entries
            .iter()
            .filter(|entry| entry.disposition == ControllerFieldDisposition::Recognized)
            .collect::<Vec<_>>();
        assert_eq!(recognized.len(), 1);
        assert_eq!(recognized[0].target, ControllerFieldTarget::Actor);
        assert_eq!(recognized[0].index, SERVER_SHORT_TIMEOUT_FIELD_INDEX);
        assert_eq!(recognized[0].payload_bits, 0);
        assert_eq!(recognized[0].count, 2);
        assert_eq!(lab.stats.controller_fields_ignored, 2);
        assert_eq!(lab.stats.actor_bunches_malformed, 0);
        assert_eq!(lab.stats.initialisation_calls_ignored, 0);
        assert_eq!(lab.character_slot(address), None);
        assert!(!lab.peers[&address].profiles.friend_ids_ready);
        assert_eq!(lab.config.idle_timeout, timeout);
        assert_diagnostics_eq(&lab.controller_field_totals(), diagnostics);
        let last_seen = lab.peers[&address].last_seen;
        lab.expire_idle(last_seen + timeout);
        assert!(lab.has_peer(address));
        lab.expire_idle(last_seen + timeout + Duration::from_millis(1));
        assert!(!lab.has_peer(address));
        assert_eq!(lab.stats.peers_expired, 1);
    }

    #[test]
    fn malformed_short_timeout_tail_discards_earlier_controller_calls() {
        for bit in [false, true] {
            let mut lab = prospect_refresh_lab();
            let (socket, address, mut client) = controller_peer(&mut lab);
            let mut writer = BitWriter::new();
            writer.write_bit(bit);
            let payload = client_actor_body(
                &lab,
                vec![field_block(
                    BlockTarget::Actor,
                    BP_ICARUS_PLAYER_CONTROLLER_SURVIVAL_FIELD_INDEX_BOUND,
                    &[
                        (
                            SERVER_FRIENDS_LIST_UPDATED_FIELD_INDEX,
                            friends_payload(ByteOrder::Little, &[]),
                        ),
                        (SERVER_SHORT_TIMEOUT_FIELD_INDEX, empty_payload()),
                        (
                            SERVER_SHORT_TIMEOUT_FIELD_INDEX,
                            Payload::from_writer(writer),
                        ),
                    ],
                )],
            );
            let raw = wire_reliable_sequence(client.initial_reliable().1 + 1);
            receive_controller_payload(&mut lab, &socket, &mut client, raw, payload);
            let diagnostics = lab.controller_field_diagnostics(address).unwrap();
            assert_eq!(diagnostics.entries.len(), 3);
            assert_eq!(diagnostics.malformed_bunches, 1);
            assert_eq!(
                diagnostics.entries[1].index,
                SERVER_SHORT_TIMEOUT_FIELD_INDEX
            );
            assert_eq!(
                diagnostics.entries[1].disposition,
                ControllerFieldDisposition::Recognized
            );
            let malformed = &diagnostics.entries[2];
            assert_eq!(malformed.index, SERVER_SHORT_TIMEOUT_FIELD_INDEX);
            assert_eq!(malformed.payload_bits, 1);
            assert_eq!(malformed.disposition, ControllerFieldDisposition::Malformed);
            assert_eq!(lab.stats.actor_bunches_malformed, 1);
            assert_eq!(lab.stats.friends_lists_received, 0);
            assert_eq!(lab.stats.controller_fields_ignored, 0);
            assert!(!lab.peers[&address].profiles.friend_ids_ready);
            assert_eq!(lab.character_slot(address), None);
        }
    }

    #[test]
    fn subobject_isolation_preserves_permitted_field_classification() {
        let mut lab = prospect_refresh_lab();
        let (socket, address, mut client) = controller_peer(&mut lab);
        let entry = &lab.peers[&address];
        let backend = entry.backend_proxy.unwrap();
        let player_data = entry.player_data.unwrap();
        let bestiary = entry.bestiary.unwrap();
        let components = [Some(backend), Some(player_data), Some(bestiary)];
        // Avoid the default GUID (1), which requires export flags on wire.
        // This authored reference is distinct from all permitted components.
        let unknown = (2..)
            .find(|guid| !components.contains(&Some(*guid)))
            .unwrap();
        let valid_blocks = vec![
            field_block(
                subobject(player_data),
                PLAYER_DATA_FIELD_INDEX_BOUND,
                &[(SERVER_RECEIVE_LOADOUT, empty_payload())],
            ),
            field_block(
                subobject(bestiary),
                BESTIARY_MANAGER_FIELD_INDEX_BOUND,
                &[(SERVER_RECEIVE_BESTIARY_DATA, empty_payload())],
            ),
        ];
        let valid_payload = client_actor_body(&lab, valid_blocks.clone());
        let mut expected = ControllerFieldDiagnostics::default();
        let (calls, ignored) = controller_initialisation_calls(
            &valid_payload,
            lab.player_controller_open
                .as_ref()
                .unwrap()
                .engine_net_version,
            lab.config.packet.codec.order,
            components,
            &mut expected,
        )
        .unwrap();
        assert_eq!(ignored, 0);
        assert!(matches!(
            calls.as_slice(),
            [
                ControllerCall::Initialisation(InitialisationAnswer::Loadout),
                ControllerCall::Initialisation(InitialisationAnswer::Bestiary),
            ]
        ));
        let mut blocks = vec![
            // An actor RPC index on a known subobject must not dispatch on the actor.
            field_block(
                subobject(backend),
                BACKEND_PROXY_FIELD_INDEX_BOUND,
                &[
                    (
                        SERVER_FRIENDS_LIST_UPDATED_FIELD_INDEX,
                        friends_payload(ByteOrder::Little, &[]),
                    ),
                    (SERVER_RECEIVE_LOADOUT, empty_payload()),
                ],
            ),
            // The Bestiary index also exists in other component caches. Matching
            // the field number alone must not recognize it on PlayerData.
            field_block(
                subobject(player_data),
                PLAYER_DATA_FIELD_INDEX_BOUND,
                &[(SERVER_RECEIVE_BESTIARY_DATA, empty_payload())],
            ),
            // Unknown subobjects are skipped before attempting their fields,
            // including an otherwise recognized actor RPC and an invalid header.
            field_block(
                subobject(unknown),
                BP_ICARUS_PLAYER_CONTROLLER_SURVIVAL_FIELD_INDEX_BOUND,
                &[(
                    SERVER_FRIENDS_LIST_UPDATED_FIELD_INDEX,
                    friends_payload(ByteOrder::Little, &[]),
                )],
            ),
            ContentBlock {
                has_rep_layout: true,
                target: subobject(unknown),
                payload: {
                    let mut writer = BitWriter::new();
                    writer.write_bit(true);
                    Payload::from_writer(writer)
                },
            },
        ];
        blocks.extend(valid_blocks);
        let payload = client_actor_body(&lab, blocks);
        let raw = wire_reliable_sequence(client.initial_reliable().1 + 1);
        receive_controller_payload(&mut lab, &socket, &mut client, raw, payload);
        let diagnostics = lab.controller_field_diagnostics(address).unwrap();
        assert_eq!(diagnostics.malformed_bunches, 0);
        assert_eq!(diagnostics.overflow, 0);
        assert_eq!(diagnostics.entries.len(), 5);
        assert!(
            diagnostics
                .entries
                .iter()
                .all(|entry| entry.target == ControllerFieldTarget::PermittedSubobject)
        );
        let recognized: Vec<_> = diagnostics
            .entries
            .iter()
            .filter(|entry| entry.disposition == ControllerFieldDisposition::Recognized)
            .cloned()
            .collect();
        assert_eq!(recognized, expected.entries);
        assert_eq!(
            diagnostics
                .entries
                .iter()
                .filter(|entry| entry.disposition == ControllerFieldDisposition::Ignored)
                .count(),
            3
        );
        assert_eq!(lab.stats.controller_fields_ignored, 3);
        assert_eq!(lab.stats.friends_lists_received, 0);
        assert!(!lab.peers[&address].profiles.friend_ids_ready);
        assert_eq!(lab.stats.initialisation_calls_ignored, 0);
        assert_eq!(lab.stats.loadout_responses_received, 1);
        assert_eq!(lab.stats.bestiary_responses_received, 1);
        assert_eq!(lab.stats.initialisation_answers_ignored, 2);
        assert_eq!(lab.stats.actor_bunches_malformed, 0);
        assert_diagnostics_eq(&lab.controller_field_totals(), diagnostics);
    }

    #[test]
    fn duplicate_reliable_controller_bunch_advances_diagnostics_and_domain_once() {
        let mut lab = prospect_refresh_lab();
        let (socket, address, mut client) = controller_peer(&mut lab);
        let raw = wire_reliable_sequence(client.initial_reliable().1 + 1);
        let payload = actor_fields(false);
        let accepted_before = lab.stats.data_accepted;
        receive_controller_payload(&mut lab, &socket, &mut client, raw, payload.clone());
        let first = lab.controller_field_diagnostics(address).unwrap().clone();
        assert_eq!(first.entries.len(), 2);
        assert!(first.entries.iter().all(|entry| entry.count == 1));
        assert_eq!(
            first.entries[0].disposition,
            ControllerFieldDisposition::Recognized
        );
        assert_eq!(
            first.entries[1].disposition,
            ControllerFieldDisposition::Ignored
        );
        assert_eq!(lab.stats.friends_lists_received, 1);
        assert!(lab.peers[&address].profiles.friend_ids_ready);
        assert_eq!(lab.stats.controller_fields_ignored, 1);
        assert_eq!(lab.stats.initialisation_calls_ignored, 0);
        assert_eq!(lab.stats.actor_duplicate_reliable, 0);
        // Commit a fresh packet header while reusing the reliable sequence.
        // Replaying identical datagram bytes would only test packet rejection.
        receive_controller_payload(&mut lab, &socket, &mut client, raw, payload);
        assert_eq!(lab.stats.data_accepted, accepted_before + 2);
        assert_eq!(lab.stats.actor_duplicate_reliable, 1);
        assert_eq!(lab.stats.friends_lists_received, 1);
        assert_eq!(lab.stats.controller_fields_ignored, 1);
        assert_eq!(lab.stats.initialisation_calls_ignored, 0);
        assert_eq!(lab.stats.actor_bunches_malformed, 0);
        assert_diagnostics_eq(lab.controller_field_diagnostics(address).unwrap(), &first);
        assert_diagnostics_eq(&lab.controller_field_totals(), &first);
    }

    #[test]
    fn lifetime_begin_diagnostics_preserve_recognition_atomicity_and_duplicate_policy() {
        use super::super::super::probe_initialise::{
            begin_player_initialisation_field, initialise_codec,
        };
        for malformed in [false, true] {
            let mut lab = record_lab(false);
            let (socket, address, mut client) = record_peer(&mut lab, 1, "PRIVATE-DIAGNOSTIC-NAME");
            let codec = initialise_codec(
                lab.player_controller_open
                    .as_ref()
                    .unwrap()
                    .engine_net_version,
            );
            let begin = begin_player_initialisation_field(3, &codec).unwrap();
            let mut fields = vec![(begin.index, begin.payload)];
            if malformed {
                let mut invalid = BitWriter::new();
                invalid.write_bit(true);
                fields.push((
                    SERVER_SHORT_TIMEOUT_FIELD_INDEX,
                    Payload::from_writer(invalid),
                ));
            }
            let payload = client_actor_body(
                &lab,
                vec![field_block(
                    BlockTarget::Actor,
                    BP_ICARUS_PLAYER_CONTROLLER_SURVIVAL_FIELD_INDEX_BOUND,
                    &fields,
                )],
            );
            let raw = wire_reliable_sequence(client.initial_reliable().1 + 1);
            receive_controller_payload(&mut lab, &socket, &mut client, raw, payload.clone());
            let diagnostics = lab.controller_receive_diagnostics();
            assert!(diagnostics.fields.entries.iter().any(|field| field.index
                == SERVER_BEGIN_PLAYER_INITIALISATION_FIELD_INDEX
                && field.disposition == ControllerFieldDisposition::Recognized
                && field.count == 1));
            assert_eq!(lab.character_slot(address), (!malformed).then_some(3));
            assert_eq!(
                lab.stats.initialisation_slots_recorded,
                u64::from(!malformed)
            );
            assert_eq!(diagnostics.actor_bunches_malformed, u64::from(malformed));
            // A fresh packet carrying the same reliable sequence applies and
            // classifies nothing twice, even when the original was malformed.
            receive_controller_payload(&mut lab, &socket, &mut client, raw, payload);
            let duplicate = lab.controller_receive_diagnostics();
            assert_eq!(duplicate.fields, diagnostics.fields);
            assert_eq!(
                duplicate.actor_duplicate_reliable,
                diagnostics.actor_duplicate_reliable + 1
            );
            assert_eq!(
                lab.stats.initialisation_slots_recorded,
                u64::from(!malformed)
            );
            let before = duplicate.fields;
            lab.remove_peer(address).unwrap();
            assert_eq!(lab.controller_receive_diagnostics().fields, before);
            assert!(lab.controller_field_totals().entries.is_empty());
            assert!(lab.remove_peer(address).is_none());
            assert_eq!(lab.controller_receive_diagnostics().fields, before);
        }
    }

    #[test]
    fn cookie_replacement_retires_controller_metadata_once() {
        let mut lab = record_lab(false);
        let (socket, address, mut client) = record_peer(&mut lab, 1, "PRIVATE-OLD-PLAYER");
        let fields = vec![
            (
                SERVER_SHORT_TIMEOUT_FIELD_INDEX,
                Payload::from_writer(BitWriter::new())
            );
            7
        ];
        let payload = client_actor_body(
            &lab,
            vec![field_block(
                BlockTarget::Actor,
                BP_ICARUS_PLAYER_CONTROLLER_SURVIVAL_FIELD_INDEX_BOUND,
                &fields,
            )],
        );
        let raw = wire_reliable_sequence(client.initial_reliable().1 + 1);
        receive_controller_payload(&mut lab, &socket, &mut client, raw, payload);
        let before = lab.controller_receive_diagnostics();
        assert_eq!(before.fields.entries[0].count, 7);
        let (replacement, _client) = record_peer_on(&mut lab, &socket, 2, "PRIVATE-NEW-PLAYER");
        assert_eq!(replacement, address);
        assert_eq!(lab.stats.peers_replaced, 1);
        assert!(lab.controller_field_totals().entries.is_empty());
        assert_eq!(lab.controller_receive_diagnostics().fields, before.fields);
        lab.shutdown();
        assert_eq!(lab.controller_receive_diagnostics().fields, before.fields);
        lab.shutdown();
        assert_eq!(lab.controller_receive_diagnostics().fields, before.fields);
    }

    #[test]
    fn lifetime_table_overflow_is_stable_across_live_peer_removal_and_shutdown() {
        for remove_later_contributor in [false, true] {
            let mut lab = prospect_refresh_lab();
            let (first_socket, first, mut first_client) = controller_peer(&mut lab);
            let (second_socket, second, mut second_client) = controller_peer(&mut lab);
            let first_fields: Vec<_> = (0..64)
                .map(|bits| {
                    let mut writer = BitWriter::new();
                    for _ in 0..bits {
                        writer.write_bit(false);
                    }
                    (0, Payload::from_writer(writer))
                })
                .collect();
            let second_fields = vec![(1, Payload::from_writer(BitWriter::new())); 100];
            for (socket, client, fields) in [
                (&first_socket, &mut first_client, first_fields),
                (&second_socket, &mut second_client, second_fields),
            ] {
                let payload = client_actor_body(
                    &lab,
                    vec![field_block(
                        BlockTarget::Actor,
                        BP_ICARUS_PLAYER_CONTROLLER_SURVIVAL_FIELD_INDEX_BOUND,
                        &fields,
                    )],
                );
                let raw = wire_reliable_sequence(client.initial_reliable().1 + 1);
                receive_controller_payload(&mut lab, socket, client, raw, payload);
            }
            let capped = lab.controller_receive_diagnostics();
            assert_eq!(capped.fields.entries.len(), 64);
            assert_eq!(capped.fields.overflow, 100);
            assert!(
                capped
                    .fields
                    .entries
                    .iter()
                    .all(|entry| entry.index == 0 && entry.count == 1)
            );
            assert_eq!(
                lab.controller_field_diagnostics(second).unwrap().entries[0].count,
                100
            );
            // Both peers are live when the shared cap is exceeded. Departure
            // cannot reorder retained keys or reclassify prior overflow.
            lab.remove_peer(if remove_later_contributor {
                second
            } else {
                first
            })
            .unwrap();
            assert_eq!(lab.controller_receive_diagnostics().fields, capped.fields);
            assert_eq!(lab.shutdown(), 1);
            assert_eq!(lab.controller_receive_diagnostics().fields, capped.fields);
            assert_eq!(lab.shutdown(), 0);
            assert_eq!(lab.controller_receive_diagnostics().fields, capped.fields);
        }
    }

    #[test]
    fn skipped_controller_exports_are_attributed_without_parsing_or_dispatch() {
        let mut lab = prospect_refresh_lab();
        let profile = super::super::super::release::ReleaseProfile::fixture("3.0.36");
        lab.player_controller_open = profile
            .player_controller_open_plan(PeerLabAdmission::default().map_url, [0.0; 3])
            .unwrap();
        let (socket, address, mut client) = controller_peer(&mut lab);
        let mapping = &lab.peers[&address].controller_component_mapping;
        assert!(mapping.complete);
        let configured = mapping.guids[3];
        let unmapped = (2..).find(|guid| !mapping.guids.contains(guid)).unwrap();
        let mut invalid = BitWriter::new();
        invalid.write_bit(true);
        let payload = client_actor_body(
            &lab,
            vec![
                // Invalid field bits and a property flag remain opaque even
                // though this target was really exported by the committed open.
                ContentBlock {
                    has_rep_layout: true,
                    target: subobject(configured),
                    payload: Payload::from_writer(invalid),
                },
                field_block(
                    subobject(unmapped),
                    BP_ICARUS_PLAYER_CONTROLLER_SURVIVAL_FIELD_INDEX_BOUND,
                    &[(
                        SERVER_BEGIN_PLAYER_INITIALISATION_FIELD_INDEX,
                        empty_payload(),
                    )],
                ),
            ],
        );
        let raw = wire_reliable_sequence(client.initial_reliable().1 + 1);
        receive_controller_payload(&mut lab, &socket, &mut client, raw, payload.clone());
        let observed = lab.controller_receive_diagnostics();
        assert_eq!(observed.fields.unknown_subobject_blocks, 2);
        assert_eq!(observed.fields.skipped_blocks.len(), 2);
        assert_eq!(
            observed.fields.skipped_blocks[0],
            ControllerSkippedBlockSummary {
                relation: ControllerSubobjectRelation::ConfiguredControllerComponent,
                component_ordinal: Some(3),
                has_rep_layout: true,
                payload_bits: 1,
                count: 1,
            }
        );
        assert_eq!(
            observed.fields.skipped_blocks[1].relation,
            ControllerSubobjectRelation::Unmapped
        );
        assert_eq!(observed.fields.skipped_blocks[1].component_ordinal, None);
        assert!(observed.fields.entries.is_empty());
        assert_eq!(observed.actor_bunches_malformed, 0);
        assert_eq!(lab.stats.initialisation_slots_recorded, 0);
        assert_eq!(lab.character_slot(address), None);
        assert_eq!(lab.stats.controller_fields_ignored, 0);
        // Releasing a duplicate reliable sequence cannot observe it twice.
        receive_controller_payload(&mut lab, &socket, &mut client, raw, payload);
        assert_eq!(lab.controller_receive_diagnostics().fields, observed.fields);
        assert_eq!(lab.stats.actor_duplicate_reliable, 1);
    }

    #[test]
    fn controller_component_mapping_is_bounded_and_installed_only_on_commit() {
        let guids: Vec<u32> = (100..165).collect();
        for count in [64, 65] {
            let mapping = ControllerComponentMapping::committed(&guids[..count]);
            assert_eq!(mapping.guids.len(), 64);
            assert_eq!(
                mapping.relation(163),
                (
                    ControllerSubobjectRelation::ConfiguredControllerComponent,
                    Some(63)
                )
            );
            assert_eq!(
                mapping.relation(164),
                (
                    if count == 64 {
                        ControllerSubobjectRelation::Unmapped
                    } else {
                        ControllerSubobjectRelation::MappingOverflow
                    },
                    None
                )
            );
            let lab = prospect_refresh_lab();
            let payload = client_actor_body(
                &lab,
                vec![ContentBlock {
                    has_rep_layout: true,
                    target: subobject(164),
                    payload: empty_payload(),
                }],
            );
            let mut diagnostics = ControllerFieldDiagnostics::default();
            let staged = controller_calls(
                &payload,
                lab.player_controller_open
                    .as_ref()
                    .unwrap()
                    .engine_net_version,
                lab.config.packet.codec.order,
                [None; 3],
                Some(&mapping),
                (None, None),
                &mut [&mut diagnostics],
            )
            .unwrap();
            assert!(staged.calls.is_empty());
            assert_eq!(
                diagnostics.skipped_blocks[0].relation,
                mapping.relation(164).0
            );
            assert_eq!(diagnostics.unknown_subobject_blocks, 1);
        }
        assert_eq!(
            ControllerComponentMapping::default().relation(100),
            (ControllerSubobjectRelation::MappingOverflow, None)
        );

        let mut lab = prospect_refresh_lab();
        let (_socket, address, _) = acked_game_state_peer(&mut lab);
        let components = lab
            .player_controller_open
            .as_ref()
            .unwrap()
            .profile
            .components
            .clone();
        lab.player_controller_open
            .as_mut()
            .unwrap()
            .profile
            .components
            .clear();
        assert!(matches!(
            lab.send_initial_controller_open(address),
            InitialOpenOutcome::Failed
        ));
        assert!(
            lab.peers[&address]
                .controller_component_mapping
                .guids
                .is_empty()
        );
        assert!(!lab.peers[&address].controller_component_mapping.complete);
        lab.player_controller_open
            .as_mut()
            .unwrap()
            .profile
            .components = components;
        assert!(matches!(
            lab.send_initial_controller_open(address),
            InitialOpenOutcome::Committed(_)
        ));
        assert_eq!(
            lab.peers[&address].controller_component_mapping.guids[0],
            lab.peers[&address].backend_proxy.unwrap()
        );
        assert!(lab.peers[&address].controller_component_mapping.complete);
    }

    #[test]
    fn skipped_block_lifetime_cap_merge_saturation_and_departure_are_stable() {
        let mut lab = prospect_refresh_lab();
        let (socket, address, mut client) = controller_peer(&mut lab);
        let configured = lab.peers[&address].controller_component_mapping.guids[3];
        for bits in 0..17 {
            let mut writer = BitWriter::new();
            for _ in 0..bits {
                writer.write_bit(true);
            }
            let payload = client_actor_body(
                &lab,
                vec![ContentBlock {
                    has_rep_layout: true,
                    target: subobject(configured),
                    payload: Payload::from_writer(writer),
                }],
            );
            let raw = wire_reliable_sequence(client.initial_reliable().1 + 1 + bits);
            receive_controller_payload(&mut lab, &socket, &mut client, raw, payload);
        }
        let snapshot = lab.controller_receive_diagnostics();
        assert_eq!(snapshot.fields.skipped_blocks.len(), 16);
        assert_eq!(snapshot.fields.skipped_blocks_overflow, 1);
        assert_eq!(snapshot.fields.unknown_subobject_blocks, 17);
        assert_eq!(lab.controller_field_totals(), snapshot.fields);
        lab.remove_peer(address).unwrap();
        assert_eq!(lab.controller_receive_diagnostics(), snapshot);
        assert!(lab.controller_field_totals().skipped_blocks.is_empty());

        // A replacement connection has its own newly committed mapping;
        // shutting it down must retain both historical and new rows.
        let (socket, replacement, mut client) = controller_peer(&mut lab);
        let configured = lab.peers[&replacement].controller_component_mapping.guids[4];
        let payload = client_actor_body(
            &lab,
            vec![ContentBlock {
                has_rep_layout: false,
                target: subobject(configured),
                payload: empty_payload(),
            }],
        );
        let raw = wire_reliable_sequence(client.initial_reliable().1 + 1);
        receive_controller_payload(&mut lab, &socket, &mut client, raw, payload);
        let replacement_snapshot = lab.controller_receive_diagnostics();
        assert_eq!(
            replacement_snapshot.fields.skipped_blocks,
            snapshot.fields.skipped_blocks
        );
        assert_eq!(replacement_snapshot.fields.skipped_blocks_overflow, 2);
        assert_eq!(lab.shutdown(), 1);
        assert_eq!(lab.controller_receive_diagnostics(), replacement_snapshot);
        assert_eq!(lab.shutdown(), 0);
        assert_eq!(lab.controller_receive_diagnostics(), replacement_snapshot);

        let mut saturated = snapshot.fields;
        for entry in &mut saturated.skipped_blocks {
            entry.count = u64::MAX;
        }
        saturated.skipped_blocks_overflow = u64::MAX;
        let mut merged = saturated.clone();
        merged.merge(&saturated);
        assert!(
            merged
                .skipped_blocks
                .iter()
                .all(|entry| entry.count == u64::MAX)
        );
        assert_eq!(merged.skipped_blocks_overflow, u64::MAX);
        assert_eq!(merged.skipped_blocks.len(), 16);
    }

    #[test]
    fn lifetime_receive_metadata_has_fixed_allowlist_and_serialized_bound() {
        let mut lab = prospect_refresh_lab();
        for index in 0..17 {
            lab.lifetime_controller_fields
                .record_skipped_count(ControllerSkippedBlockSummary {
                    relation: ControllerSubobjectRelation::ConfiguredControllerComponent,
                    component_ordinal: Some(index),
                    has_rep_layout: true,
                    payload_bits: usize::MAX,
                    count: u64::MAX,
                });
        }
        for index in 0..65_u32 {
            lab.lifetime_controller_fields.record_count(
                ControllerFieldTarget::PermittedSubobject,
                u32::MAX - index,
                usize::MAX - index as usize,
                ControllerFieldDisposition::Malformed,
                u64::MAX,
            );
        }
        lab.lifetime_controller_fields.malformed_bunches = u64::MAX;
        lab.lifetime_controller_fields.body_parse_failures = u64::MAX;
        lab.lifetime_controller_fields
            .block_payload_limit_rejections = u64::MAX;
        lab.lifetime_controller_fields
            .block_payload_length_rejections = u64::MAX;
        lab.lifetime_controller_fields
            .max_rejected_block_payload_bits = u64::MAX;
        lab.lifetime_controller_fields.unknown_subobject_blocks = u64::MAX;
        lab.stats = PeerLabStats {
            data_accepted: u64::MAX,
            data_rejected: u64::MAX,
            data_malformed: u64::MAX,
            actor_bunches_ignored: u64::MAX,
            actor_bunches_dropped: u64::MAX,
            actor_bunch_drop_reasons: ActorBunchDropReasons {
                unreliable: u64::MAX,
                reliable_queue_refusal: u64::MAX,
                unsupported_header: u64::MAX,
                partial_policy: u64::MAX,
                partial_reassembly: u64::MAX,
            },
            actor_partial_parts: u64::MAX,
            actor_partial_completed: u64::MAX,
            actor_partial_rejected: u64::MAX,
            actor_bunches_malformed: u64::MAX,
            actor_duplicate_reliable: u64::MAX,
            controller_fields_ignored: u64::MAX,
            ..Default::default()
        };
        let diagnostics = lab.controller_receive_diagnostics();
        assert_eq!(diagnostics.fields.entries.len(), 64);
        assert_eq!(diagnostics.fields.overflow, u64::MAX);
        let value = serde_json::to_value(&diagnostics).unwrap();
        let keys: std::collections::BTreeSet<_> = value
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            keys,
            [
                "data_accepted",
                "data_rejected",
                "data_malformed",
                "actor_bunches_ignored",
                "actor_bunches_dropped",
                "actor_bunch_drop_reasons",
                "actor_partial_parts",
                "actor_partial_completed",
                "actor_partial_rejected",
                "actor_bunches_malformed",
                "actor_duplicate_reliable",
                "controller_fields_ignored",
                "fields",
            ]
            .into_iter()
            .collect()
        );
        let field_keys: std::collections::BTreeSet<_> = value["fields"]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            field_keys,
            [
                "entries",
                "overflow",
                "malformed_bunches",
                "body_parse_failures",
                "block_payload_limit_rejections",
                "block_payload_length_rejections",
                "max_rejected_block_payload_bits",
                "unknown_subobject_blocks",
                "skipped_blocks",
                "skipped_blocks_overflow",
            ]
            .into_iter()
            .collect()
        );
        for entry in value["fields"]["entries"].as_array().unwrap() {
            assert_eq!(
                entry
                    .as_object()
                    .unwrap()
                    .keys()
                    .map(String::as_str)
                    .collect::<std::collections::BTreeSet<_>>(),
                ["target", "index", "payload_bits", "disposition", "count"]
                    .into_iter()
                    .collect()
            );
        }
        assert_eq!(diagnostics.fields.skipped_blocks.len(), 16);
        assert_eq!(diagnostics.fields.skipped_blocks_overflow, u64::MAX);
        for entry in value["fields"]["skipped_blocks"].as_array().unwrap() {
            assert_eq!(
                entry
                    .as_object()
                    .unwrap()
                    .keys()
                    .map(String::as_str)
                    .collect::<std::collections::BTreeSet<_>>(),
                [
                    "relation",
                    "component_ordinal",
                    "has_rep_layout",
                    "payload_bits",
                    "count"
                ]
                .into_iter()
                .collect()
            );
        }
        assert_eq!(
            value["actor_bunch_drop_reasons"]
                .as_object()
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect::<std::collections::BTreeSet<_>>(),
            [
                "unreliable",
                "reliable_queue_refusal",
                "unsupported_header",
                "partial_policy",
                "partial_reassembly"
            ]
            .into_iter()
            .collect()
        );
        assert!(serde_json::to_vec(&diagnostics).unwrap().len() <= 16 * 1024);
    }

    #[test]
    fn peer_removal_and_shutdown_drop_only_live_controller_diagnostics() {
        let mut lab = prospect_refresh_lab();
        let (first_socket, first, mut first_client) = controller_peer(&mut lab);
        let (second_socket, second, mut second_client) = controller_peer(&mut lab);
        for (socket, client, payload) in [
            (&first_socket, &mut first_client, actor_fields(true)),
            (&second_socket, &mut second_client, actor_fields(false)),
        ] {
            let raw = wire_reliable_sequence(client.initial_reliable().1 + 1);
            receive_controller_payload(&mut lab, socket, client, raw, payload);
        }
        let first_diagnostics = lab.controller_field_diagnostics(first).unwrap();
        assert_eq!(first_diagnostics.entries.len(), 2);
        assert_eq!(first_diagnostics.malformed_bunches, 1);
        assert!(
            !lab.peers[&first].profiles.friend_ids_ready,
            "malformed trailing input must not apply the preceding recognized call"
        );
        assert_eq!(lab.stats.friends_lists_received, 1);
        assert_eq!(lab.stats.controller_fields_ignored, 1);
        let second_diagnostics = lab.controller_field_diagnostics(second).unwrap().clone();
        assert_eq!(second_diagnostics.entries.len(), 2);
        assert!(
            second_diagnostics
                .entries
                .iter()
                .all(|entry| entry.count == 1)
        );
        let totals = lab.controller_field_totals();
        assert_eq!(lab.controller_receive_diagnostics().fields, totals);
        assert_eq!(totals.entries.len(), 2);
        assert!(totals.entries.iter().all(|entry| entry.count == 2));
        assert_eq!(totals.malformed_bunches, 1);
        assert_eq!(totals.overflow, 0);
        lab.remove_peer(first).unwrap();
        assert!(lab.controller_field_diagnostics(first).is_none());
        assert_diagnostics_eq(
            lab.controller_field_diagnostics(second).unwrap(),
            &second_diagnostics,
        );
        assert_diagnostics_eq(&lab.controller_field_totals(), &second_diagnostics);
        assert_eq!(lab.peer_count(), 1);
        assert_eq!(lab.shutdown(), 1);
        assert!(lab.controller_field_diagnostics(first).is_none());
        assert!(lab.controller_field_diagnostics(second).is_none());
        assert_diagnostics_eq(
            &lab.controller_field_totals(),
            &ControllerFieldDiagnostics::default(),
        );
        assert_eq!(lab.peer_count(), 0);
        assert_eq!(lab.controller_receive_diagnostics().fields, totals);
        assert_eq!(lab.shutdown(), 0);
        assert_eq!(lab.controller_receive_diagnostics().fields, totals);
    }

    #[test]
    fn aggregate_controller_diagnostics_are_bounded_saturating_metadata() {
        let mut lab = prospect_refresh_lab();
        let (_first_socket, first, _) = acked_game_state_peer(&mut lab);
        let (_second_socket, second, _) = acked_game_state_peer(&mut lab);
        for (address, start) in [(first, 0), (second, 64)] {
            let diagnostics = &mut lab
                .peers
                .get_mut(&address)
                .unwrap()
                .controller_field_diagnostics;
            for index in start..start + 65 {
                diagnostics.record_count(
                    ControllerFieldTarget::Actor,
                    index,
                    8,
                    ControllerFieldDisposition::Ignored,
                    u64::MAX,
                );
            }
            diagnostics.record(
                ControllerFieldTarget::Actor,
                start + 65,
                8,
                ControllerFieldDisposition::Ignored,
            );
            diagnostics.malformed_bunches = u64::MAX;
            diagnostics.body_parse_failures = u64::MAX;
            diagnostics.block_payload_limit_rejections = u64::MAX;
            diagnostics.block_payload_length_rejections = u64::MAX;
            diagnostics.max_rejected_block_payload_bits = 65_537 + u64::from(start);
            diagnostics.unknown_subobject_blocks = u64::MAX;
            assert_eq!(diagnostics.entries.len(), 64);
            assert_eq!(diagnostics.overflow, u64::MAX);
        }
        let totals = lab.controller_field_totals();
        assert_eq!(totals.entries.len(), 64);
        assert!(totals.entries.iter().all(|entry| entry.count == u64::MAX));
        assert_eq!(totals.overflow, u64::MAX);
        assert_eq!(totals.malformed_bunches, u64::MAX);
        assert_eq!(totals.body_parse_failures, u64::MAX);
        assert_eq!(totals.block_payload_limit_rejections, u64::MAX);
        assert_eq!(totals.block_payload_length_rejections, u64::MAX);
        assert_eq!(totals.max_rejected_block_payload_bits, 65_601);
        assert_eq!(totals.unknown_subobject_blocks, u64::MAX);
        let value = serde_json::to_value(&totals).unwrap();
        let mut keys: Vec<_> = value
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            [
                "block_payload_length_rejections",
                "block_payload_limit_rejections",
                "body_parse_failures",
                "entries",
                "malformed_bunches",
                "max_rejected_block_payload_bits",
                "overflow",
                "skipped_blocks",
                "skipped_blocks_overflow",
                "unknown_subobject_blocks"
            ]
        );
        for entry in value["entries"].as_array().unwrap() {
            let mut keys: Vec<_> = entry
                .as_object()
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect();
            keys.sort_unstable();
            assert_eq!(
                keys,
                ["count", "disposition", "index", "payload_bits", "target"]
            );
        }
        // With neither peer's own overflow involved, 128 distinct rows must
        // still be bounded to 64 aggregate rows and charge the discarded counts.
        for address in [first, second] {
            let diagnostics = &mut lab
                .peers
                .get_mut(&address)
                .unwrap()
                .controller_field_diagnostics;
            diagnostics.overflow = 0;
            diagnostics.malformed_bunches = 0;
            for entry in &mut diagnostics.entries {
                entry.count = 1;
            }
        }
        let totals = lab.controller_field_totals();
        assert_eq!(totals.entries.len(), 64);
        assert_eq!(totals.overflow, 64);
        assert_eq!(totals.malformed_bunches, 0);
        // Identical rows from distinct peers saturate their aggregate count.
        let shared = lab.peers[&first].controller_field_diagnostics.entries[0].clone();
        for address in [first, second] {
            let diagnostics = &mut lab
                .peers
                .get_mut(&address)
                .unwrap()
                .controller_field_diagnostics;
            diagnostics.entries = vec![shared.clone()];
            diagnostics.entries[0].count = u64::MAX;
        }
        let totals = lab.controller_field_totals();
        assert_eq!(totals.entries.len(), 1);
        assert_eq!(totals.entries[0].count, u64::MAX);
        assert_eq!(totals.overflow, 0);
    }

    fn actor_fields(trailing: bool) -> Payload {
        let mut writer = BitWriter::new();
        for index in [SERVER_FRIENDS_LIST_UPDATED_FIELD_INDEX, 0] {
            write_field_header(
                &mut writer,
                &FieldHeader {
                    index,
                    payload: if index == SERVER_FRIENDS_LIST_UPDATED_FIELD_INDEX {
                        friends_payload(ByteOrder::Little, &[])
                    } else {
                        empty_payload()
                    },
                },
                BP_ICARUS_PLAYER_CONTROLLER_SURVIVAL_FIELD_INDEX_BOUND,
            )
            .unwrap();
        }
        let fields = Payload::from_writer(writer);
        let fields = if trailing {
            // An incomplete header after two complete observations.
            let mut writer = BitWriter::new();
            let mut reader = fields.reader();
            while !reader.at_end() {
                writer.write_bit(reader.read_bit());
            }
            writer.write_bit(true);
            Payload::from_writer(writer)
        } else {
            fields
        };
        encode_actor_bunch_body(
            &ActorBunchBody {
                spawn: None,
                blocks: vec![ContentBlock {
                    has_rep_layout: false,
                    target: BlockTarget::Actor,
                    payload: fields,
                }],
            },
            &actor_codec(0, ByteOrder::Little),
        )
        .unwrap()
    }

    #[test]
    fn mixed_fields_are_observations_and_trailing_failure_returns_no_calls() {
        let mut diagnostics = ControllerFieldDiagnostics::default();
        let result = controller_initialisation_calls(
            &actor_fields(false),
            0,
            ByteOrder::Little,
            [None; 3],
            &mut diagnostics,
        )
        .unwrap();
        assert_eq!(result.0.len(), 1);
        assert_eq!(result.1, 1);
        assert_eq!(diagnostics.entries.len(), 2);
        assert_eq!(
            diagnostics.entries[0].disposition,
            ControllerFieldDisposition::Recognized
        );
        assert_eq!(
            diagnostics.entries[1].disposition,
            ControllerFieldDisposition::Ignored
        );
        let mut diagnostics = ControllerFieldDiagnostics::default();
        assert!(
            controller_initialisation_calls(
                &actor_fields(true),
                0,
                ByteOrder::Little,
                [None; 3],
                &mut diagnostics
            )
            .is_err()
        );
        assert_eq!(diagnostics.entries.len(), 2);
    }

    #[test]
    fn malformed_recognized_payload_records_disposition_without_applying_calls() {
        let mut lab = prospect_refresh_lab();
        let (socket, address, mut client) = controller_peer(&mut lab);
        let payload = client_actor_body(
            &lab,
            vec![field_block(
                BlockTarget::Actor,
                BP_ICARUS_PLAYER_CONTROLLER_SURVIVAL_FIELD_INDEX_BOUND,
                &[
                    (
                        SERVER_FRIENDS_LIST_UPDATED_FIELD_INDEX,
                        friends_payload(ByteOrder::Little, &[]),
                    ),
                    (SERVER_BEGIN_PLAYER_INITIALISATION_FIELD_INDEX, {
                        // Present ChrSlot, missing its required Int payload.
                        let mut writer = BitWriter::new();
                        writer.write_bit(true);
                        Payload::from_writer(writer)
                    }),
                ],
            )],
        );
        let raw = wire_reliable_sequence(client.initial_reliable().1 + 1);
        receive_controller_payload(&mut lab, &socket, &mut client, raw, payload);

        let diagnostics = lab.controller_field_diagnostics(address).unwrap();
        assert_eq!(diagnostics.entries.len(), 2);
        assert_eq!(diagnostics.malformed_bunches, 1);
        assert_eq!(diagnostics.overflow, 0);
        assert_eq!(
            diagnostics.entries[0].disposition,
            ControllerFieldDisposition::Recognized
        );
        let malformed = &diagnostics.entries[1];
        assert_eq!(malformed.target, ControllerFieldTarget::Actor);
        assert_eq!(
            malformed.index,
            SERVER_BEGIN_PLAYER_INITIALISATION_FIELD_INDEX
        );
        assert_eq!(malformed.payload_bits, 1);
        assert_eq!(malformed.disposition, ControllerFieldDisposition::Malformed);
        assert_eq!(malformed.count, 1);
        assert_eq!(lab.stats.actor_bunches_malformed, 1);
        assert_eq!(lab.stats.friends_lists_received, 0);
        assert_eq!(lab.stats.controller_fields_ignored, 0);
        assert!(!lab.peers[&address].profiles.friend_ids_ready);
        assert_eq!(lab.character_slot(address), None);
        assert_diagnostics_eq(&lab.controller_field_totals(), diagnostics);
    }

    #[test]
    fn observations_are_bounded_and_saturate_without_cross_target_coalescing() {
        let mut diagnostics = ControllerFieldDiagnostics::default();
        for index in 0..1000 {
            diagnostics.record(
                ControllerFieldTarget::Actor,
                index,
                8,
                ControllerFieldDisposition::Ignored,
            );
        }
        assert_eq!(diagnostics.entries.len(), 64);
        assert_eq!(diagnostics.overflow, 936);
        diagnostics.entries[0].count = u64::MAX;
        diagnostics.record(
            ControllerFieldTarget::Actor,
            0,
            8,
            ControllerFieldDisposition::Ignored,
        );
        assert_eq!(diagnostics.entries[0].count, u64::MAX);
        diagnostics.record(
            ControllerFieldTarget::PermittedSubobject,
            0,
            8,
            ControllerFieldDisposition::Ignored,
        );
        assert_eq!(diagnostics.overflow, 937);
        assert_eq!(diagnostics.entries[0].target, ControllerFieldTarget::Actor);
    }

    /// Authored client-to-server unreliable bunches on every channel kind:
    /// each still drops exactly as before (counted in
    /// `actor_bunches_dropped`, no dispatch or field diagnostics), and the
    /// header-only breakdown counts it by channel kind, controller field
    /// index and length; the field table caps at its fixed size with an
    /// overflow count, and counts saturate.
    #[test]
    fn unreliable_client_bunches_are_counted_by_channel_and_field_and_stay_dropped() {
        let mut lab = record_lab(false);
        let (socket, address, mut client) = record_peer(&mut lab, 1, "AuthoredUnreliable");
        let entry = &lab.peers[&address];
        let controller = entry.player_controller_channel.unwrap();
        let player_state = entry.player_state_channel.unwrap();
        let pawn = super::super::super::pawn_open::PAWN_CHANNEL_INDEX;
        let game_state = game_state_channel(&lab);
        assert!(entry.peer.has_actor_channel(pawn));
        assert!(entry.peer.has_actor_channel(game_state));
        let unreliable = |channel: u32, partial: bool, payload: Payload| Bunch {
            header: BunchHeader {
                control: false,
                open: false,
                close: false,
                close_reason: None,
                paused: false,
                reliable: false,
                channel_index: channel,
                has_package_map_exports: false,
                has_must_be_mapped_guids: false,
                partial,
                reliable_sequence_raw: None,
                partial_initial: partial,
                partial_final: false,
                channel_name: None,
            },
            payload,
        };
        let bits = |count: usize| {
            let mut writer = BitWriter::new();
            for bit in 0..count {
                writer.write_bit(bit % 3 == 0);
            }
            Payload::from_writer(writer)
        };
        let fields = |lab: &PeerLab, fields: &[(u32, Payload)]| {
            client_actor_body(
                lab,
                vec![field_block(
                    BlockTarget::Actor,
                    BP_ICARUS_PLAYER_CONTROLLER_SURVIVAL_FIELD_INDEX_BOUND,
                    fields,
                )],
            )
        };
        let mut send = |lab: &mut PeerLab, bunches: Vec<Bunch>| {
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
            receive_authored_at(lab, &socket, &bytes, Instant::now());
        };
        let camera = fields(&lab, &[(81, bits(40))]);
        let two = fields(&lab, &[(81, bits(24)), (78, bits(0))]);
        let malformed = Payload::from_writer({
            let mut writer = BitWriter::new();
            writer.write_bit(true);
            writer
        });
        // Off by default: the bunch is dropped as before and not counted.
        let dropped = lab.stats.actor_bunches_dropped;
        send(
            &mut lab,
            vec![unreliable(controller, false, camera.clone())],
        );
        assert_eq!(lab.stats.actor_bunches_dropped, dropped + 1);
        assert_eq!(
            lab.unreliable_actor_bunch_counts(),
            &UnreliableActorBunchCounts::default()
        );
        lab.enable_unreliable_actor_bunch_counts();
        send(
            &mut lab,
            vec![
                unreliable(controller, false, camera.clone()),
                unreliable(controller, false, two.clone()),
                unreliable(pawn, false, bits(70)),
            ],
        );
        send(
            &mut lab,
            vec![
                unreliable(player_state, false, bits(9)),
                unreliable(game_state, false, bits(10)),
                unreliable(controller, true, bits(12)),
                unreliable(controller, false, malformed),
            ],
        );
        let counts = lab.unreliable_actor_bunch_counts().clone();
        assert_eq!(
            counts.controller,
            UnreliableChannelCounts {
                bunches: 4,
                last_payload_bits: 1,
            }
        );
        assert_eq!(
            counts.pawn,
            UnreliableChannelCounts {
                bunches: 1,
                last_payload_bits: 70,
            }
        );
        assert_eq!(counts.player_state.bunches, 1);
        assert_eq!(counts.player_state.last_payload_bits, 9);
        assert_eq!(counts.game_state.bunches, 1);
        assert_eq!(counts.game_state.last_payload_bits, 10);
        assert_eq!(counts.other, UnreliableChannelCounts::default());
        assert_eq!(
            counts.controller_fields,
            [
                UnreliableFieldCounts {
                    index: 81,
                    count: 2,
                    last_payload_bits: 24,
                },
                UnreliableFieldCounts {
                    index: 78,
                    count: 1,
                    last_payload_bits: 0,
                },
            ]
        );
        assert_eq!(counts.controller_unparsed, 1);
        assert_eq!(counts.controller_malformed, 1);
        assert_eq!(counts.controller_field_overflow, 0);
        assert_eq!(counts.controller_other_blocks, 0);
        // Still dropped exactly as before: nothing dispatched or diagnosed.
        // Seven counted bunches plus the one sent before counting began.
        assert_eq!(lab.stats.actor_bunches_dropped, dropped + 8);
        assert_eq!(lab.stats.actor_bunch_drop_reasons.unreliable, dropped + 8);
        assert_eq!(
            lab.controller_receive_diagnostics()
                .actor_bunch_drop_reasons,
            lab.stats.actor_bunch_drop_reasons
        );
        assert_eq!(lab.stats.controller_fields_ignored, 0);
        assert_eq!(lab.stats.actor_bunches_malformed, 0);
        let diagnostics = lab.controller_field_diagnostics(address).unwrap();
        assert!(diagnostics.entries.is_empty());
        assert_eq!(diagnostics.malformed_bunches, 0);
        // A replicated actor the lab opens on a free channel through its
        // public `open_actor` is "other"; a sub-object block is counted,
        // not read.
        let backend = lab.peers[&address].backend_proxy.unwrap();
        let component = client_actor_body(
            &lab,
            vec![field_block(
                subobject(backend),
                BACKEND_PROXY_FIELD_INDEX_BOUND,
                &[(1, bits(3))],
            )],
        );
        let replicated = lab
            .open_actor(
                1,
                0x5eed,
                super::super::super::session_open::ActorOpenSpec {
                    actor: 0,
                    archetype: vec![
                        super::super::super::session_open::PathSegment::new(
                            "Default__AuthoredCrate_C",
                            0x0bad_cafe,
                            false,
                        ),
                        super::super::super::session_open::PathSegment::new(
                            "/Game/Authored/AuthoredCrate",
                            0,
                            false,
                        ),
                    ],
                    level: super::super::super::session_open::map_level_chain(
                        PeerLabAdmission::default().map_url,
                    )
                    .unwrap(),
                    location: Some([0.0; 3]),
                    rotation: None,
                    velocity: None,
                    blocks: Vec::new(),
                },
                game_state + 1,
                17,
            )
            .unwrap();
        assert!(![controller, pawn, player_state, game_state].contains(&replicated));
        send(
            &mut lab,
            vec![
                unreliable(replicated, false, bits(5)),
                unreliable(controller, false, component),
            ],
        );
        let counts = lab.unreliable_actor_bunch_counts().clone();
        assert_eq!(
            counts.other,
            UnreliableChannelCounts {
                bunches: 1,
                last_payload_bits: 5,
            }
        );
        assert_eq!(counts.pawn.bunches, 1);
        assert_eq!(counts.controller_other_blocks, 1);
        // The field table keeps 16 distinct indices; the rest overflow.
        for index in 100..(100 + UNRELIABLE_CONTROLLER_FIELD_ENTRIES as u32) {
            let payload = fields(&lab, &[(index, bits(1))]);
            send(&mut lab, vec![unreliable(controller, false, payload)]);
        }
        let counts = lab.unreliable_actor_bunch_counts().clone();
        assert_eq!(
            counts.controller_fields.len(),
            UNRELIABLE_CONTROLLER_FIELD_ENTRIES
        );
        assert_eq!(counts.controller_field_overflow, 2);
        assert!(
            counts
                .controller_fields
                .iter()
                .all(|field| field.index != 114)
        );
        // Saturation.
        lab.unreliable_actor_bunches.controller.bunches = u64::MAX;
        lab.unreliable_actor_bunches.controller_fields[0].count = u64::MAX;
        lab.unreliable_actor_bunches.controller_field_overflow = u64::MAX;
        let unseen = fields(&lab, &[(150, bits(1))]);
        send(
            &mut lab,
            vec![
                unreliable(controller, false, camera),
                unreliable(controller, false, two),
                unreliable(controller, false, unseen),
            ],
        );
        let counts = lab.unreliable_actor_bunch_counts();
        assert_eq!(counts.controller.bunches, u64::MAX);
        assert_eq!(counts.controller_fields[0].count, u64::MAX);
        assert_eq!(counts.controller_field_overflow, u64::MAX);
        let text = serde_json::to_string(counts).unwrap();
        assert!(!text.contains("AuthoredUnreliable"));
    }
}
mod actor_plan_admission;
mod friends_query_deadline;
mod level_visibility;
mod lossy_join;
mod move_acks;
mod pawn_capture;
mod pawn_receive;
mod player_checkpoints;
mod player_inventory;
mod private_join_budget;
mod record_ack_gating;
mod spawn_placement;
mod authored_clock_integration {
    //! Authored tick-derived wire time; no original daylight or client claim.
    use super::super::super::{
        actor_channel::read_actor_bunch_body,
        archive::ByteOrder,
        game_state_open::handle as h,
        packet::{BitWriter, Bunch, Payload},
        rep_layout::{RepLayoutCodec, append_bits, read_rep_properties},
        sequencing::PacketSequencer,
        time_of_day::RateCurve,
    };
    use super::*;
    use crate::authored_world_bootstrap::AuthoredClockConfig;

    fn plan() -> AuthoredClockPlan {
        AuthoredClockConfig {
            initial_minute: 480,
            seconds_per_day: 3600,
            tick_hz: 30,
        }
        .prepare()
        .unwrap()
    }

    fn enabled(tick: u64) -> PeerLab {
        let mut lab = prospect_refresh_lab();
        lab.enable_authored_time_of_day(plan(), tick).unwrap();
        lab
    }

    fn opened(
        lab: &mut PeerLab,
    ) -> (
        UdpSocket,
        SocketAddrV4,
        PacketSequencer,
        BTreeMap<u32, RepValue>,
    ) {
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        socket.connect(lab.local_addr().unwrap()).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        let (address, mut client) = handshake_peer_on(lab, &socket);
        assert!(matches!(
            lab.send_game_state_open(address),
            InitialOpenOutcome::Committed(_)
        ));
        let bunches = deliver_bunches(lab, &socket, &mut client, |_| true);
        let values = decode_open(lab, &bunches);
        (socket, address, client, values)
    }

    fn decode_open(lab: &PeerLab, bunches: &[Bunch]) -> BTreeMap<u32, RepValue> {
        decode_state(lab, bunches, true)
    }

    fn decode_state(lab: &PeerLab, bunches: &[Bunch], open: bool) -> BTreeMap<u32, RepValue> {
        let session = lab.game_state_open.as_ref().unwrap();
        let mut payload = BitWriter::new();
        for bunch in bunches.iter().filter(|bunch| {
            bunch.header.channel_index == session.channel_index
                && !bunch.header.has_package_map_exports
        }) {
            append_bits(&mut payload, &bunch.payload);
            if !bunch.header.partial || bunch.header.partial_final {
                break;
            }
        }
        let codec = RepLayoutCodec {
            actor: actor_codec(session.plan.engine_net_version, ByteOrder::Little),
            checksums: false,
            limits: Default::default(),
        };
        let body = read_actor_bunch_body(
            &mut Payload::from_writer(payload).reader(),
            open,
            &codec.actor,
        )
        .unwrap();
        read_rep_properties(
            &mut body.blocks[0].payload.reader(),
            &session.plan.layout,
            &codec,
        )
        .unwrap()
        .entries
        .into_iter()
        .map(|entry| (entry.handle, entry.value))
        .collect()
    }

    fn assert_time(values: &BTreeMap<u32, RepValue>, tick: u64) {
        assert_eq!(
            values.get(&h::TIME_OF_DAY),
            Some(&RepValue::Float(plan().sample(tick).to_bits()))
        );
        assert_eq!(
            values.get(&h::SECONDS_PER_GAME_DAY),
            Some(&RepValue::Int(3600))
        );
    }

    #[test]
    fn authored_clock_opens_at_sample_and_midnight_sends_explicit_zero() {
        let mut lab = enabled(27_000);
        let (socket, _address, mut client, values) = opened(&mut lab);
        assert_time(&values, 27_000);
        lab.sample_authored_time_of_day(72_000);
        lab.game_state_open.as_mut().unwrap().updated_at = None;
        lab.update_game_state();
        let changed = receive_game_state_update(&socket, &lab, Some(&mut client));
        assert_time(&changed, 72_000);
        assert_eq!(
            lab.game_state_open
                .as_ref()
                .unwrap()
                .sampled_clock
                .unwrap()
                .minute
                .to_bits(),
            0
        );
    }

    #[test]
    fn declared_large_day_length_reaches_the_wire_without_float_rounding() {
        let mut lab = prospect_refresh_lab();
        let plan = AuthoredClockConfig {
            initial_minute: 480,
            seconds_per_day: i32::MAX,
            tick_hz: 30,
        }
        .prepare()
        .unwrap();
        lab.enable_authored_time_of_day(plan, u64::MAX).unwrap();
        let (_, _, _, values) = opened(&mut lab);
        assert_eq!(values[&h::SECONDS_PER_GAME_DAY], RepValue::Int(i32::MAX));
        assert_eq!(
            values[&h::TIME_OF_DAY],
            RepValue::Float(plan.sample(u64::MAX).to_bits())
        );
    }

    #[test]
    fn unchanged_completed_tick_does_not_advance_with_wall_time_polling() {
        let mut lab = enabled(27_000);
        let (socket, _address, mut client, values) = opened(&mut lab);
        assert_time(&values, 27_000);
        let session = lab.game_state_open.as_mut().unwrap();
        session.clock_at = Instant::now() - Duration::from_secs(120);
        session.updated_at = None;
        lab.started = Instant::now() - Duration::from_secs(120);
        lab.update_game_state();
        let world = receive_game_state_update(&socket, &lab, Some(&mut client));
        assert!(
            world.contains_key(&h::WORLD_TIME_SECONDS),
            "world-seconds ownership is unchanged"
        );
        assert!(!world.contains_key(&h::TIME_OF_DAY));
        assert_eq!(
            lab.game_state_open
                .as_ref()
                .unwrap()
                .sampled_clock
                .unwrap()
                .minute,
            plan().sample(27_000)
        );
        for _ in 0..3 {
            lab.sample_authored_time_of_day(27_000);
            lab.game_state_open.as_mut().unwrap().updated_at = None;
            lab.update_game_state();
        }
        assert_eq!(
            lab.game_state_open
                .as_ref()
                .unwrap()
                .sampled_clock
                .unwrap()
                .minute,
            plan().sample(27_000)
        );
    }

    #[test]
    fn ring_pressure_retries_latest_sample_and_nak_repairs_latest_value() {
        let mut lab = enabled(0);
        lab.config.packet.sent_capacity = 16;
        let (socket, address, mut client, _) = opened(&mut lab);
        let capacity = lab.peers[&address].peer.config().sent_capacity;
        let pending = lab.peers[&address]
            .peer
            .sequencer()
            .notify()
            .sent_records()
            .len();
        for _ in pending..capacity {
            let datagram = lab
                .peers
                .get_mut(&address)
                .unwrap()
                .peer
                .ack_packet()
                .unwrap();
            assert!(lab.send(&datagram, address));
        }
        for tick in [9_000, 18_000, 27_000] {
            lab.sample_authored_time_of_day(tick);
            lab.game_state_open.as_mut().unwrap().updated_at = None;
            lab.update_game_state();
            assert_eq!(
                lab.peers[&address].game_state.as_ref().unwrap().sent[&h::TIME_OF_DAY],
                RepValue::Float(480f32.to_bits())
            );
        }
        deliver_bunches(&mut lab, &socket, &mut client, |_| true);
        lab.game_state_open.as_mut().unwrap().updated_at = None;
        lab.update_game_state();
        let lost = deliver_bunches(&mut lab, &socket, &mut client, |_| false);
        let changed = decode_state(&lab, &lost, false);
        assert_time(&changed, 27_000);
        // A client NAK traversed packet notification and made the latest sample
        // due. The retry may coalesce it to a newer completed tick.
        assert!(
            !lab.peers[&address]
                .game_state
                .as_ref()
                .unwrap()
                .sent
                .contains_key(&h::TIME_OF_DAY)
        );
        lab.sample_authored_time_of_day(36_000);
        lab.game_state_open.as_mut().unwrap().updated_at = None;
        lab.update_game_state();
        let delivered = deliver_bunches(&mut lab, &socket, &mut client, |_| true);
        let repaired = decode_state(&lab, &delivered, false);
        assert_time(&repaired, 36_000);
    }

    #[test]
    fn sampled_and_monotonic_modes_are_exclusive_and_clockless_wire_stays_ordinary() {
        let legacy = || TimeOfDayClock::new(RateCurve::constant(60.0).unwrap(), 600.0);
        let mut sampled = enabled(0);
        assert!(sampled.enable_time_of_day(legacy()).is_err());
        let mut ordinary = prospect_refresh_lab();
        ordinary.enable_time_of_day(legacy()).unwrap();
        assert!(ordinary.enable_authored_time_of_day(plan(), 0).is_err());
        let mut clockless = prospect_refresh_lab();
        let (_, _, _, values) = opened(&mut clockless);
        assert!(!values.contains_key(&h::TIME_OF_DAY));
        assert!(!values.contains_key(&h::SECONDS_PER_GAME_DAY));
    }

    #[test]
    fn restored_owner_is_sampled_before_the_first_wire_open() {
        use crate::{
            config::ServerConfig,
            simulation::{SimulationClock, WorldSnapshot, build_app},
        };
        let mut app = build_app(&ServerConfig::default(), &WorldSnapshot::default()).unwrap();
        let config = AuthoredClockConfig {
            initial_minute: 480,
            seconds_per_day: 3600,
            tick_hz: 30,
        };
        crate::network::install_authored_clock_runtime(&mut app, Some(&config), 30).unwrap();
        app.world_mut().resource_mut::<SimulationClock>().tick = 27_000;
        app.insert_resource(enabled(0));
        crate::network::reconcile_authored_clock(app.world_mut()).unwrap();
        app.world_mut()
            .resource_scope(|_world, mut lab: bevy::prelude::Mut<PeerLab>| {
                let (_, _, _, values) = opened(&mut lab);
                assert_time(&values, 27_000);
            });
    }

    #[test]
    fn absent_phase_owner_or_wrong_fixed_time_stops_before_open_and_latches() {
        use crate::{
            config::ServerConfig,
            simulation::{SimulationClock, WorldSnapshot, build_app, capture},
        };
        use bevy::prelude::{Fixed, Time};
        for missing_clock in [true, false] {
            let mut app = build_app(&ServerConfig::default(), &WorldSnapshot::default()).unwrap();
            let config = AuthoredClockConfig {
                initial_minute: 480,
                seconds_per_day: 3600,
                tick_hz: 30,
            };
            crate::network::install_authored_clock_runtime(&mut app, Some(&config), 30).unwrap();
            app.insert_resource(enabled(0));
            let owner = if missing_clock {
                app.world_mut().remove_resource::<SimulationClock>()
            } else {
                app.insert_resource(Time::<Fixed>::from_hz(7.0));
                None
            };
            // The real pre-poll entry point must fail before receiving/accepting or
            // composing any Join/open; this also leaves no peer for publication.
            poll_peer_lab(app.world_mut());
            assert!(!app.world().resource::<PeerLab>().is_admitting());
            assert_eq!(app.world().resource::<PeerLab>().peer_count(), 0);
            if let Some(owner) = owner {
                app.insert_resource(owner);
            }
            app.insert_resource(Time::<Fixed>::from_hz(30.0));
            assert!(crate::network::authored_clock_runtime_health(app.world()).is_err());
            assert!(capture(app.world_mut()).is_err());
            app.world_mut().run_schedule(bevy::prelude::FixedUpdate);
            assert_eq!(app.world().resource::<SimulationClock>().tick, 0);
        }
    }
}

mod authored_survival_authority_tests {
    use super::*;
    use crate::{
        config::ServerConfig,
        native_player_runtime::{ConsumptionConfig, PlayerRegistry},
        native_players::{
            PlayerCheckpoint, PlayerContext, PlayerInputs, PlayerKey, PlayerSnapshot,
        },
        simulation::{SNAPSHOT_SCHEMA_PLAYERS, SimulationClock, WorldSnapshot, build_app, capture},
    };
    use bevy::prelude::FixedUpdate;

    const ACCOUNT: u64 = 76_561_197_960_265_770;

    fn context() -> PlayerContext {
        PlayerContext::new(
            vec![65],
            "/Authored/Map".into(),
            PlayerInputs {
                authored_gameplay: [7; 32],
                spawn: [7; 32],
                authored_world: [7; 32],
                declaration: [7; 32],
                initial_values: [7; 32],
            },
            vec!["Player".into()],
        )
        .unwrap()
    }

    fn checkpoint() -> PlayerCheckpoint {
        PlayerCheckpoint {
            location_cm: [10.0, 20.0, 30.0],
            vitals: super::super::super::pawn_open::CharacterVitals {
                health: 90,
                max_health: 100,
                stamina: 80,
                max_stamina: 100,
                oxygen: 70,
                max_oxygen: 100,
                water: 60,
                max_water: 100,
                food: 50,
                max_food: 100,
                biome_row: 0,
                growth_row: 0,
            },
        }
    }

    /// Begin at the synthetic post-ticket boundary, using real packet/ring
    /// machinery. Installation/authentication themselves have separate tests.
    fn attached() -> (
        App,
        UdpSocket,
        SocketAddrV4,
        super::super::super::sequencing::PacketSequencer,
        crate::steam_service::TestBackend,
    ) {
        let mut lab = record_lab(false);
        lab.config.packet.sent_capacity = 16;
        let (socket, address, client) = record_peer(&mut lab, 1, "AUTHORED-SURVIVAL");
        let (auth, backend) = AuthClient::test_bridge(32);
        lab.authenticator = Some(auth);
        lab.checkpoint_initial_vitals = Some(checkpoint().vitals);
        let context = context();
        let mut registry =
            PlayerRegistry::new(PlayerSnapshot::empty(context.clone()), &context, 8).unwrap();
        registry
            .reserve(PlayerKey::new(ACCOUNT, 3).unwrap(), 1, checkpoint())
            .unwrap();
        let projection = registry.activate(1).unwrap();
        let peer = lab.peers.get_mut(&address).unwrap();
        peer.auth_accepted = true;
        peer.auth_request = Some(1);
        peer.auth_identity = Some(ACCOUNT);
        peer.admission.state = ClientLoginState::Joined;
        peer.character_slot = Some(3);
        peer.profiles.character_slot = Some(3);
        peer.profiles.completed = true;
        peer.profiles.friend_ids_ready = true;
        peer.checkpoint_epoch = Some(1);
        peer.checkpoint_projection = Some(projection);
        peer.checkpoint_active = true;
        peer.checkpoint_enqueued_revision = Some(projection.revision);
        let snapshot = WorldSnapshot {
            schema_version: SNAPSHOT_SCHEMA_PLAYERS,
            native_players: Some(registry.snapshot()),
            ..Default::default()
        };
        let mut app = build_app(
            &ServerConfig {
                tick_hz: 1,
                ..Default::default()
            },
            &snapshot,
        )
        .unwrap();
        app.insert_resource(registry).insert_resource(lab);
        crate::native_player_runtime::install_consumption(
            &mut app,
            ConsumptionConfig {
                oxygen: 1,
                water: 2,
                food: 3,
            },
            1,
        )
        .unwrap();
        (app, socket, address, client, backend)
    }

    #[test]
    fn queued_revocation_after_poll_wins_before_fixed_debit_without_projection() {
        let (mut app, _socket, address, _client, backend) = attached();
        app.world_mut().resource_mut::<PeerLab>().poll();
        let before = app.world().resource::<PlayerRegistry>().snapshot();
        let queued_revision =
            app.world().resource::<PeerLab>().peers[&address].checkpoint_enqueued_revision;
        backend
            .event(crate::steam_auth::AuthEvent {
                request_id: 1,
                outcome: AuthOutcome::Revoked,
            })
            .unwrap();
        app.world_mut().run_schedule(FixedUpdate);
        assert_eq!(app.world().resource::<PlayerRegistry>().snapshot(), before);
        assert!(
            app.world()
                .resource::<PlayerRegistry>()
                .active_epochs()
                .is_empty()
        );
        assert!(!app.world().resource::<PeerLab>().has_peer(address));
        assert_eq!(queued_revision, Some(1));
        assert_eq!(app.world().resource::<SimulationClock>().tick, 1);
        assert!(crate::native_player_runtime::consumption_runtime_health(app.world()).is_ok());
    }

    #[test]
    fn event_after_explicit_cut_applies_at_next_cut() {
        let (mut app, _socket, address, _client, backend) = attached();
        crate::network::reconcile_player_authority(app.world_mut()).unwrap();
        let before = app.world().resource::<PlayerRegistry>().snapshot();
        backend
            .event(crate::steam_auth::AuthEvent {
                request_id: 1,
                outcome: AuthOutcome::Revoked,
            })
            .unwrap();
        // This models delivery after the cut: no projection/publication is
        // performed by the eligibility operation, and the queued event stays.
        assert_eq!(app.world().resource::<PlayerRegistry>().snapshot(), before);
        assert!(app.world().resource::<PeerLab>().has_peer(address));
        app.world_mut()
            .resource_mut::<PlayerRegistry>()
            .consume_activated(
                crate::native_player_runtime::ConsumptionRates::new(1, 2, 3).unwrap(),
            )
            .unwrap();
        let after_cut = app.world().resource::<PlayerRegistry>().snapshot();
        assert_eq!(
            after_cut
                .projection(PlayerKey::new(ACCOUNT, 3).unwrap())
                .unwrap()
                .checkpoint
                .vitals
                .oxygen,
            69
        );
        crate::network::reconcile_player_authority(app.world_mut()).unwrap();
        assert!(
            app.world()
                .resource::<PlayerRegistry>()
                .active_epochs()
                .is_empty()
        );
        assert!(!app.world().resource::<PeerLab>().has_peer(address));
        assert_eq!(
            app.world().resource::<PlayerRegistry>().snapshot(),
            after_cut
        );
    }

    #[test]
    fn backend_loss_detaches_before_debit_and_preserves_committed_capture() {
        let (mut app, _socket, address, _client, backend) = attached();
        app.world_mut().run_schedule(FixedUpdate);
        let committed = app.world().resource::<PlayerRegistry>().snapshot();
        assert_eq!(app.world().resource::<SimulationClock>().tick, 1);
        drop(backend);
        app.world_mut().run_schedule(FixedUpdate);
        assert_eq!(
            app.world().resource::<PlayerRegistry>().snapshot(),
            committed
        );
        assert!(
            app.world()
                .resource::<PlayerRegistry>()
                .active_epochs()
                .is_empty()
        );
        assert!(!app.world().resource::<PeerLab>().has_peer(address));
        assert!(!app.world().resource::<PeerLab>().is_admitting());
        assert!(crate::native_player_runtime::consumption_runtime_health(app.world()).is_ok());
        let saved = capture(app.world_mut()).unwrap();
        assert_eq!(saved.tick, 2);
        assert_eq!(saved.native_players, Some(committed));
    }

    #[test]
    fn pressure_and_nak_retries_publish_without_repeating_a_debit() {
        let (mut app, socket, address, mut client, _backend) = attached();
        {
            let mut lab = app.world_mut().resource_mut::<PeerLab>();
            let capacity = lab.peers[&address].peer.config().sent_capacity;
            let pending = lab.peers[&address]
                .peer
                .sequencer()
                .notify()
                .sent_records()
                .len();
            for _ in pending..capacity {
                let datagram = lab
                    .peers
                    .get_mut(&address)
                    .unwrap()
                    .peer
                    .ack_packet()
                    .unwrap();
                assert!(lab.send(&datagram, address));
            }
            assert!(lab.peers[&address].peer.full_ring_probe().is_some());
        }
        app.world_mut().run_schedule(FixedUpdate);
        let charged = app.world().resource::<PlayerRegistry>().snapshot();
        let projection = charged
            .projection(PlayerKey::new(ACCOUNT, 3).unwrap())
            .unwrap();
        assert_eq!(projection.checkpoint.vitals.oxygen, 69);
        assert_eq!(projection.revision, 2);
        assert_eq!(
            app.world().resource::<PeerLab>().peers[&address].checkpoint_enqueued_revision,
            Some(1)
        );
        for _ in 0..3 {
            crate::network::drain_player_bridge(app.world_mut());
        }
        assert_eq!(app.world().resource::<PlayerRegistry>().snapshot(), charged);
        app.world_mut()
            .resource_scope(|_world, mut lab: bevy::prelude::Mut<PeerLab>| {
                deliver_bunches(&mut lab, &socket, &mut client, |_| true);
            });
        crate::network::drain_player_bridge(app.world_mut());
        assert_eq!(
            app.world().resource::<PeerLab>().peers[&address].checkpoint_enqueued_revision,
            Some(2)
        );
        // NAK the admitted update using the real sequence/ring path; retry
        // publication is transport work, with no second fixed interval.
        app.world_mut()
            .resource_scope(|_world, mut lab: bevy::prelude::Mut<PeerLab>| {
                deliver_bunches(&mut lab, &socket, &mut client, |_| false);
            });
        crate::network::drain_player_bridge(app.world_mut());
        app.world_mut()
            .resource_scope(|_world, mut lab: bevy::prelude::Mut<PeerLab>| {
                deliver_bunches(&mut lab, &socket, &mut client, |_| true);
            });
        assert_eq!(app.world().resource::<PlayerRegistry>().snapshot(), charged);
        assert_eq!(app.world().resource::<SimulationClock>().tick, 1);
    }
}

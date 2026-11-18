//! Authored registration failures must precede all publication, independently
//! of the connection byte order and initial-open transport pressure.
use super::super::super::{
    actor_open::BlockContent,
    archive::ByteOrder,
    control_messages::{NetIdType, NetIdValue, UniqueNetId},
    manager_open::PLAYER_HISTORY_HANDLE,
    packet::BitWriter,
    packet_lab::MAX_CONSTRUCTED_BUNCH_BYTES,
    prospect_info::{PlayerHistoryEntry, SavedQuestVariable},
    quest_open::{StitchedQuest, quest_actor_handle},
    rep_layout::RepLayoutLimits,
};
use super::*;

const ORDERS: [ByteOrder; 2] = [ByteOrder::Little, ByteOrder::Big];

fn manager(slot: ManagerSlot) -> ManagerOpenPlan {
    ReleaseProfile::fixture("3.0.32")
        .manager_open_plan(slot, PeerLabAdmission::default().map_url)
        .unwrap()
        .unwrap()
}

fn quest(id: u64, variables: Vec<SavedQuestVariable>) -> QuestOpenPlan {
    ReleaseProfile::fixture("3.0.32")
        .quest_open_base_plan(PeerLabAdmission::default().map_url)
        .unwrap()
        .unwrap()
        .quest_plan(&StitchedQuest {
            key: ActorKey::Saved(id),
            archetype: vec![
                PathSegment::new("Default__AuthoredAdmissionQuest", 51, false),
                PathSegment::new("/Lab/AuthoredAdmissionQuest", 0, false),
            ],
            location: None,
            quest_row: 1,
            relevant_actors: Vec::new(),
            relevant_characters: Vec::new(),
            active_quests: Vec::new(),
            variables,
            subquests: Vec::new(),
            parent: None,
        })
        .unwrap()
}

fn oversized_history() -> ManagerOpenPlan {
    manager(ManagerSlot::PlayerHistory)
        .with_player_history(&[PlayerHistoryEntry {
            user_id: vec![u16::from(b'u')],
            chr_slot: 0,
            // A legal wide archive string alone fills the block's byte budget;
            // the other history fields push the complete content over it.
            cached_character_name: vec![0x100; RepLayoutLimits::default().max_string_units - 1],
        }])
        .unwrap()
}

fn oversized_quest() -> QuestOpenPlan {
    quest(
        7002,
        vec![SavedQuestVariable {
            name: vec![0x100; RepLayoutLimits::default().max_string_units - 1],
            bool_value: false,
            float_value: 0.0,
            int_value: 0,
        }],
    )
}

#[derive(Debug, PartialEq)]
struct AdmissionSnapshot {
    managers: Vec<(ActorKey, ManagerOpenPlan, u32)>,
    quests: Vec<(QuestOpenPlan, u32)>,
    references: super::super::super::game_state_open::GameStateManagers,
    values: BTreeMap<u32, RepValue>,
    quest_base: Option<QuestOpenBasePlan>,
    guids: String,
    claims: Vec<i32>,
    reservations: BTreeMap<u64, i32>,
    remaps: Vec<(i32, i32)>,
    next_pawn: Option<i32>,
    configuration: String,
    peers: BTreeMap<SocketAddrV4, String>,
    readiness: ReadinessCounts,
    stats: PeerLabStats,
}

fn snapshot(lab: &PeerLab) -> AdmissionSnapshot {
    let session = lab.game_state_open.as_ref().unwrap();
    AdmissionSnapshot {
        managers: lab.manager_opens.clone(),
        quests: lab.quest_opens.clone(),
        references: session.state.managers.clone(),
        values: session.state.values.clone(),
        quest_base: lab.quest_open_base.clone(),
        // The cache has no PartialEq; Debug includes its allocation counters.
        guids: format!("{:?}", lab.guid_cache),
        claims: lab.claimed_actor_uids().to_vec(),
        reservations: lab.actor_uids.as_ref().unwrap().reservations().clone(),
        remaps: lab.actor_uids.as_ref().unwrap().remaps().to_vec(),
        next_pawn: lab.next_pawn_object_id,
        configuration: format!(
            "{:?}",
            (
                &lab.player_controller_open,
                &lab.pawn_open,
                &lab.player_state_open,
                &session.plan
            )
        ),
        peers: lab
            .peers
            .iter()
            .map(|(address, entry)| {
                let pending = entry.initial_opens.as_ref().map(|pending| {
                    let stages: Vec<_> = pending
                        .remaining
                        .iter()
                        .map(|stage| match stage {
                            InitialOpenStage::Controller => (0, 0),
                            InitialOpenStage::Pawn => (1, 0),
                            InitialOpenStage::GameState => (2, 0),
                            InitialOpenStage::PlayerState => (3, 0),
                            InitialOpenStage::Manager(index) => (4, *index),
                            InitialOpenStage::Quest(index) => (5, *index),
                        })
                        .collect();
                    (
                        stages,
                        pending.controller_guids,
                        pending.player_state_is_host,
                        pending.begin_slot,
                        pending.retry_ready,
                    )
                });
                (
                    *address,
                    format!(
                        "{:?}",
                        (
                            pending,
                            &entry.exports,
                            entry.peer.sequencer(),
                            entry.player_controller_channel,
                            entry.player_state_channel,
                            entry.pawn_uid,
                            entry.pending_pawn_uid,
                        )
                    ),
                )
            })
            .collect(),
        readiness: lab.readiness_counts(),
        stats: lab.stats.clone(),
    }
}

fn admission_lab() -> PeerLab {
    let mut lab = prospect_refresh_lab();
    lab.enable_actor_uids(crate::actor_uids::ActorUidLibrary::new(1))
        .unwrap();
    lab.seed_actor_uids([77]).unwrap();
    lab
}

#[test]
fn manager_content_failures_reject_initial_and_additive_registration_atomically() {
    let mut malformed = manager(ManagerSlot::PlayerHistory);
    malformed
        .values
        .insert(PLAYER_HISTORY_HANDLE, RepValue::Bool(true));
    for (invalid, reason) in [
        (malformed, "does not match"),
        (oversized_history(), "payload bit limit"),
    ] {
        let mut lab = admission_lab();
        // Verify these are content failures in each actual wire composer,
        // rather than level, key or channel rejections in the admission API.
        for order in ORDERS {
            let error = compose_manager_open(
                &mut lab.guid_cache.clone(),
                &mut ConnectionExports::new(),
                &invalid,
                ManagerSlot::PlayerHistory.key(),
                7,
                order,
            )
            .unwrap_err();
            assert!(format!("{error:#}").contains(reason), "{error:#}");
        }
        let before = snapshot(&lab);
        assert!(
            lab.enable_manager_opens(
                vec![
                    (ManagerSlot::Radiation, manager(ManagerSlot::Radiation)),
                    (ManagerSlot::PlayerHistory, invalid.clone()),
                ],
                6
            )
            .is_err()
        );
        assert_eq!(snapshot(&lab), before);
        lab.enable_manager_opens(
            vec![(ManagerSlot::Radiation, manager(ManagerSlot::Radiation))],
            6,
        )
        .unwrap();
        let before = snapshot(&lab);
        assert!(
            lab.add_manager_open(ManagerSlot::PlayerHistory, invalid)
                .is_err()
        );
        assert_eq!(snapshot(&lab), before);
        assert_eq!(
            lab.add_manager_open(
                ManagerSlot::PlayerHistory,
                manager(ManagerSlot::PlayerHistory)
            )
            .unwrap(),
            7
        );
        assert_eq!(format!("{:?}", lab.guid_cache), before.guids);
        assert_eq!(lab.claimed_actor_uids(), before.claims);
    }
}

#[test]
fn quest_batch_content_failures_include_the_optional_manager_in_the_transaction() {
    let mut malformed = quest(7002, Vec::new());
    malformed
        .values
        .insert(quest_actor_handle::QUEST_VARIABLES, RepValue::Bool(true));
    for (invalid, reason) in [
        (malformed, "does not match"),
        (oversized_quest(), "payload bit limit"),
    ] {
        let mut lab = admission_lab();
        lab.enable_manager_opens(
            vec![(ManagerSlot::Radiation, manager(ManagerSlot::Radiation))],
            6,
        )
        .unwrap();
        for order in ORDERS {
            let error = compose_quest_open(
                &mut lab.guid_cache.clone(),
                &mut ConnectionExports::new(),
                &invalid,
                8,
                order,
            )
            .unwrap_err();
            assert!(format!("{error:#}").contains(reason), "{error:#}");
        }
        let first = quest(7001, Vec::new());
        let second = quest(7002, Vec::new());
        let before = snapshot(&lab);
        for trailing in [
            None,
            Some((ManagerSlot::Quest, manager(ManagerSlot::Quest))),
        ] {
            assert!(
                lab.add_quest_opens_with_manager(vec![first.clone(), invalid.clone()], trailing)
                    .is_err()
            );
            assert_eq!(snapshot(&lab), before);
        }
        // All quests are valid; only the trailing manager fails.
        let mut malformed_manager = manager(ManagerSlot::PlayerHistory);
        malformed_manager
            .values
            .insert(PLAYER_HISTORY_HANDLE, RepValue::Bool(true));
        for invalid_manager in [malformed_manager, oversized_history()] {
            assert!(
                lab.add_quest_opens_with_manager(
                    vec![first.clone(), second.clone()],
                    Some((ManagerSlot::PlayerHistory, invalid_manager))
                )
                .is_err()
            );
            assert_eq!(snapshot(&lab), before);
        }
        assert!(lab.add_quest_open(invalid).is_err());
        assert_eq!(snapshot(&lab), before);
        assert_eq!(
            lab.add_quest_opens_with_manager(
                vec![first.clone(), second.clone()],
                Some((ManagerSlot::Quest, manager(ManagerSlot::Quest)))
            )
            .unwrap(),
            (vec![7, 8], Some(9))
        );
        assert_eq!(lab.quest_opens, vec![(first, 7), (second, 8)]);
        assert_eq!(format!("{:?}", lab.guid_cache), before.guids);
    }
}

#[test]
fn manager_admission_bounds_the_whole_bunch_even_when_each_block_fits() {
    let mut lab = admission_lab();
    let mut plan = manager(ManagerSlot::PlayerHistory);
    let mut payload = BitWriter::new();
    payload.write_bytes(&vec![0; MAX_CONSTRUCTED_BUNCH_BYTES / 2]);
    for component in &mut plan.components {
        component.block = Block::Encoded(BlockContent {
            has_rep_layout: false,
            payload: Payload::from_writer(payload.clone()),
        });
    }
    assert!(plan.components.len() >= 2);
    for order in ORDERS {
        let open = compose_manager_open(
            &mut lab.guid_cache.clone(),
            &mut ConnectionExports::new(),
            &plan,
            ManagerSlot::PlayerHistory.key(),
            6,
            order,
        )
        .unwrap();
        assert!(open.bunch.payload.bit_len().div_ceil(8) > MAX_CONSTRUCTED_BUNCH_BYTES);
    }
    let before = snapshot(&lab);
    let error = lab
        .enable_manager_opens(vec![(ManagerSlot::PlayerHistory, plan)], 6)
        .unwrap_err();
    assert!(format!("{error:#}").contains("manager open exceeds the bunch byte limit"));
    assert_eq!(snapshot(&lab), before);
}

#[test]
fn deferred_join_refuses_registration_without_changing_its_plan_then_resumes() {
    for configured in [false, true] {
        let mut lab = admission_lab();
        // Every individual open fits this ring; filling it first models live
        // Welcome/ACK traffic delaying Join's controller, not an impossible plan.
        lab.config.packet.sent_capacity = 16;
        if configured {
            lab.enable_manager_opens(
                vec![(ManagerSlot::Radiation, manager(ManagerSlot::Radiation))],
                6,
            )
            .unwrap();
            lab.add_quest_open(quest(7001, Vec::new())).unwrap();
        }
        let quest_base = ReleaseProfile::fixture("3.0.32")
            .quest_open_base_plan(PeerLabAdmission::default().map_url)
            .unwrap()
            .unwrap();
        lab.enable_quest_opens(quest_base.clone()).unwrap();
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        socket.connect(lab.local_addr().unwrap()).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        let (address, mut client) = handshake_peer_on(&mut lab, &socket);
        // Run actual admission dispatch and reply transitions after the real
        // handshake, ACKing Challenge and Welcome through the normal data path.
        lab.answer(
            address,
            &[Decoded::Message(ControlMessage::Hello {
                first_field: 1,
                version: 0,
                token: ArchiveText::text("").unwrap(),
            })],
        );
        ack_startup_packets(&mut lab, &socket, &mut client);
        lab.answer(
            address,
            &[Decoded::Message(ControlMessage::Login {
                response: ArchiveText::text("0").unwrap(),
                url: ArchiveText::text("?Name=AuthoredAdmission").unwrap(),
                unique_net_id: UniqueNetId {
                    kind: NetIdType::Named(ArchiveText::text("NULL").unwrap()),
                    value: NetIdValue::Text(ArchiveText::text("AuthoredAdmissionUser").unwrap()),
                },
                platform: ArchiveText::text("").unwrap(),
            })],
        );
        ack_startup_packets(&mut lab, &socket, &mut client);
        assert_eq!(
            lab.peers[&address].admission.state,
            ClientLoginState::Welcomed
        );
        // Also exercise a partially committed sequence: let exactly the
        // controller fit, leaving the GameState pending behind its live packets.
        let controller_packets = if configured {
            let entry = &lab.peers[&address];
            let plan = lab.player_controller_open.as_ref().unwrap();
            let open = compose_player_controller_open_at(
                &mut lab.guid_cache.clone(),
                &mut ConnectionExports::new(),
                plan,
                plan.spawn_location,
                0,
                ByteOrder::Little,
            )
            .unwrap();
            LabPeer::from_cookie(&entry.cookie, *entry.peer.config())
                .unwrap()
                .reliable_actor_open_datagrams(&open.bunch, std::slice::from_ref(&open.exports))
                .unwrap()
                .datagrams
                .len()
        } else {
            0
        };
        for _ in controller_packets..lab.config.packet.sent_capacity {
            let packet = lab
                .peers
                .get_mut(&address)
                .unwrap()
                .peer
                .ack_packet()
                .unwrap();
            assert!(lab.send(&packet, address));
        }
        lab.answer(address, &[Decoded::Message(ControlMessage::Join)]);
        assert_eq!(lab.stats.joins_succeeded, 1);
        let deferred = lab.readiness_counts().initial_open_outcomes;
        if configured {
            assert_eq!(deferred.committed.controller, 1);
            assert_eq!(deferred.deferred.game_state, 1);
        } else {
            assert_eq!(deferred.deferred.controller, 1);
        }
        let before = snapshot(&lab);
        let error = if configured {
            lab.add_manager_open(
                ManagerSlot::PlayerHistory,
                manager(ManagerSlot::PlayerHistory),
            )
            .unwrap_err()
        } else {
            lab.enable_manager_opens(
                vec![(ManagerSlot::Radiation, manager(ManagerSlot::Radiation))],
                6,
            )
            .unwrap_err()
        };
        assert!(error.to_string().contains("initial opens are pending"));
        assert_eq!(snapshot(&lab), before);
        if !configured {
            lab.enable_manager_opens(Vec::new(), 6).unwrap();
            assert_eq!(snapshot(&lab), before);
        }
        if configured {
            for (plans, trailing) in [
                (vec![quest(7002, Vec::new())], None),
                (
                    Vec::new(),
                    Some((ManagerSlot::Quest, manager(ManagerSlot::Quest))),
                ),
                (
                    vec![quest(7002, Vec::new())],
                    Some((ManagerSlot::Quest, manager(ManagerSlot::Quest))),
                ),
            ] {
                let error = lab
                    .add_quest_opens_with_manager(plans, trailing)
                    .unwrap_err();
                assert!(error.to_string().contains("initial opens are pending"));
                assert_eq!(snapshot(&lab), before);
            }
        }
        assert_eq!(
            lab.add_quest_opens_with_manager(Vec::new(), None).unwrap(),
            (Vec::new(), None)
        );
        assert_eq!(snapshot(&lab), before);
        // Quest base metadata is not consulted by captured stages: replacing
        // that authoring helper remains safe during initial sends.
        let mut replacement_base = quest_base;
        replacement_base.stat_container_checksum ^= 1;
        lab.enable_quest_opens(replacement_base).unwrap();
        for _ in 0..8 {
            if lab.peers[&address].initial_opens.is_none() {
                break;
            }
            ack_startup_packets(&mut lab, &socket, &mut client);
            lab.retry_initial_opens();
        }
        assert!(lab.peers[&address].initial_opens.is_none());
        let completed = lab.readiness_counts().initial_open_outcomes;
        assert!(completed.failed.is_empty());
        assert_eq!(completed.committed, completed.configured);
        assert_eq!(completed.committed.managers, usize::from(configured));
        assert_eq!(completed.committed.quests, usize::from(configured));
        assert_eq!(lab.stats.joins_succeeded, 1);
        assert_eq!(lab.stats.player_controller_opens_sent, 1);
        // Runtime registration is still supported once the captured sends
        // finish, even while their reliable ACK receipts remain outstanding.
        if !configured {
            lab.enable_manager_opens(
                vec![(ManagerSlot::Radiation, manager(ManagerSlot::Radiation))],
                6,
            )
            .unwrap();
        }
        lab.add_manager_open(
            ManagerSlot::PlayerHistory,
            manager(ManagerSlot::PlayerHistory),
        )
        .unwrap();
        lab.add_quest_opens_with_manager(
            vec![quest(7003, Vec::new())],
            Some((ManagerSlot::Quest, manager(ManagerSlot::Quest))),
        )
        .unwrap();
        assert_eq!(lab.readiness_counts().initial_open_outcomes, completed);
        assert_eq!(lab.peer_count(), 1);
        // A later connection gets the new registrations, while the first
        // connection's captured outcomes stay unchanged.
        let (later_socket, later_address, mut later_client) =
            possession_identity_peer(&mut lab, 2, "AuthoredLaterAdmission");
        assert!(lab.send_player_controller_open(later_address));
        for _ in 0..8 {
            if lab.peers[&later_address].initial_opens.is_none() {
                break;
            }
            ack_startup_packets(&mut lab, &later_socket, &mut later_client);
            lab.retry_initial_opens();
        }
        assert!(lab.peers[&later_address].initial_opens.is_none());
        let later = &lab.peers[&later_address].initial_open_outcomes;
        assert!(later.failed.is_empty());
        assert_eq!(later.committed.managers, 3);
        assert_eq!(later.committed.quests, 1 + usize::from(configured));
        assert_eq!(
            lab.peers[&address].initial_open_outcomes.configured,
            completed.configured
        );
    }
}

//! Authored loopback/writer tests; no original client or menu behavior is claimed.
use super::*;
use crate::{
    config::ServerConfig,
    experimental::{
        actor_channel::{ActorBunchBody, BlockTarget, ContentBlock, encode_actor_bunch_body},
        packet::{BunchHeader, Name, Packet, write_packet},
        release::{
            ControllerLeavePlan, ControllerLeaveQualification, ControllerLeaveRevision,
            ReleaseProfile, ReleaseQualifications,
        },
        rep_rpc::{
            ControllerLeaveRequest as Request, LeaveProspectSessionType as Kind,
            encode_controller_leave_request, write_field_header,
        },
        sequencing::wire_reliable_sequence,
        session_open::ACTOR_CHANNEL_NAME_INDEX,
    },
    native_player_runtime::{CheckpointWriter, ConsumptionRates, LeavePhase},
    simulation::{SimulationClock, build_app},
};
use bevy::{prelude::App, time::TimeUpdateStrategy};

#[path = "private_leave_proof.rs"]
mod private_proof_tests;

fn plan() -> ControllerLeavePlan {
    let mut release = ReleaseProfile::fixture("3.0.36");
    release.qualifications = Some(ReleaseQualifications {
        pin: release.pin.clone(),
        engine_network_version: release.engine_network_version,
        game_network_version: release.game_network_version,
        class_net_cache: release.class_net_cache,
        quest_catalog: None,
        visibility_decode: None,
        inbound_controller: None,
        controller_leave: Some(ControllerLeaveQualification {
            revision: ControllerLeaveRevision::SelectedControllerV1,
            player_controller: release.player_controller.clone().unwrap(),
        }),
    });
    release.controller_leave_plan().unwrap().unwrap()
}

struct Fixture {
    lab: PeerLab,
    app: App,
    socket: UdpSocket,
    address: SocketAddrV4,
    client: PacketSequencer,
    raw: u32,
    epoch: u64,
    plan: ControllerLeavePlan,
}

impl Fixture {
    fn new(enabled: bool) -> Self {
        Self::with_proof(enabled, None)
    }

    fn with_proof(
        enabled: bool,
        proof: Option<crate::leave_runtime_proof::PrivateLeaveProof>,
    ) -> Self {
        Self::build(enabled, proof, false, false)
    }

    /// Leave dispatch with the early checkpoint pawn (no private proof).
    fn early() -> Self {
        Self::build(true, None, true, false)
    }

    /// [`Self::early`] with move reading, saved locations and live
    /// relocation (`network.checkpoint_saved_locations`,
    /// `network.checkpoint_location_correction`).
    fn located() -> Self {
        Self::build(true, None, true, true)
    }

    /// Leave dispatch over a schema-15 bag world with
    /// `network.player_inventory_projection` on, in either pawn order: the
    /// starter bag was projected before the record.
    fn projected(early: bool) -> Self {
        use super::bag_projection_tests as bags;
        let mut lab = lab();
        lab.config.packet.sent_capacity = 16;
        let selected = ReleaseProfile::fixture("3.0.36");
        lab.config.local_network_version = Some(selected.pin.network_checksum);
        lab.player_controller_open = selected
            .player_controller_open_plan(PeerLabAdmission::default().map_url, [0.0; 3])
            .unwrap();
        let plan = plan();
        lab.enable_authored_controller_leave(plan.clone()).unwrap();
        if early {
            lab.enable_checkpoint_early_pawn().unwrap();
        }
        lab.enable_player_inventory_projection(bags::projection())
            .unwrap();
        let mut app = bags::inventory_world(false);
        let session = bags::join(&mut lab, &mut app, 1, ACCOUNT);
        assert_eq!(session.log, ["bag", "finalised"]);
        let epoch = lab.peers[&session.address].checkpoint_epoch.unwrap();
        let raw = session.client.initial_reliable().1 + 1;
        app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::ZERO));
        Self {
            lab,
            app,
            socket: session.socket,
            address: session.address,
            client: session.client,
            raw,
            epoch,
            plan,
        }
    }

    fn build(
        enabled: bool,
        proof: Option<crate::leave_runtime_proof::PrivateLeaveProof>,
        early: bool,
        located: bool,
    ) -> Self {
        let (mut lab, mut registry) = (lab(), registry());
        lab.config.packet.sent_capacity = 16;
        // Retain the checkpoint fixture's authored opens, selecting the
        // controller identity/layout bound by this test's explicit leave plan.
        let selected = ReleaseProfile::fixture("3.0.36");
        lab.config.local_network_version = Some(selected.pin.network_checksum);
        lab.player_controller_open = selected
            .player_controller_open_plan(PeerLabAdmission::default().map_url, [0.0; 3])
            .unwrap();
        let plan = plan();
        if enabled {
            lab.enable_authored_controller_leave(plan.clone()).unwrap();
        }
        if early {
            lab.enable_checkpoint_early_pawn().unwrap();
        }
        if located {
            lab.enable_pawn_move_acks().unwrap();
            lab.enable_checkpoint_saved_locations().unwrap();
            lab.enable_checkpoint_location_correction().unwrap();
        }
        if let Some(proof) = proof {
            registry.enable_private_leave_proof(proof.clone()).unwrap();
            lab.enable_private_leave_proof(proof.clone()).unwrap();
            // Composition fixtures begin at synthetic accepted authentication;
            // the separate admission tests exercise actual wire Join handling.
            proof
                .joined(1, crate::leave_runtime_proof::ProofJoinKind::Join)
                .unwrap();
        }
        let (socket, address, mut client, _) = peer(&mut lab, 1, ACCOUNT, true);
        identify(&mut lab, &socket, &mut client, address, 3, 3);
        lab.bridge_player_checkpoints(&mut registry);
        deliver_bunches(&mut lab, &socket, &mut client, |_| true);
        finish(&mut lab, &mut registry, &socket, &mut client, address);
        if lab.private_leave_proof.is_some() {
            registry
                .consume_activated(ConsumptionRates::new(0, 0, 1).unwrap())
                .unwrap();
        }
        let epoch = lab.peers[&address].checkpoint_epoch.unwrap();
        let raw = client.initial_reliable().1 + 1;
        let snapshot = WorldSnapshot {
            schema_version: SNAPSHOT_SCHEMA_PLAYERS,
            native_players: Some(registry.snapshot()),
            ..Default::default()
        };
        let mut app = build_app(&ServerConfig::default(), &snapshot).unwrap();
        app.insert_resource(registry);
        app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::ZERO));
        Self {
            lab,
            app,
            socket,
            address,
            client,
            raw,
            epoch,
            plan,
        }
    }

    fn registry(&self) -> &PlayerRegistry {
        self.app.world().resource::<PlayerRegistry>()
    }

    /// The leave bridge, after the saved-location pass as in `poll_peer_lab`
    /// (a no-op unless saved locations are on).
    fn bridge(&mut self) {
        self.lab.bridge_saved_locations(self.app.world_mut());
        let mut registry = self.app.world_mut().resource_mut::<PlayerRegistry>();
        self.lab.bridge_controller_leaves(&mut registry);
    }

    fn send(&mut self, request: Request) {
        let bytes = self.datagram(request, false);
        receive_authored_at(&mut self.lab, &self.socket, &bytes, Instant::now());
    }

    fn datagram(&mut self, request: Request, malformed_tail: bool) -> Vec<u8> {
        let codec = RepLayoutCodec {
            actor: actor_codec(self.plan.engine_net_version(), ByteOrder::Little),
            checksums: false,
            limits: Default::default(),
        };
        let field = encode_controller_leave_request(request, &self.plan, &codec).unwrap();
        let mut fields = BitWriter::new();
        write_field_header(&mut fields, &field, self.plan.field_index_bound()).unwrap();
        if malformed_tail {
            // A valid leave followed by a truncated field header must apply
            // none of the bunch's staged calls.
            fields.write_bit(true);
        }
        let payload = encode_actor_bunch_body(
            &ActorBunchBody {
                spawn: None,
                blocks: vec![ContentBlock {
                    has_rep_layout: false,
                    target: BlockTarget::Actor,
                    payload: Payload::from_writer(fields),
                }],
            },
            &codec.actor,
        )
        .unwrap();
        let bytes = write_packet(
            &Packet {
                magic: 0,
                header: self.client.header().unwrap(),
                info: None,
                bunches: vec![Bunch {
                    header: BunchHeader {
                        control: false,
                        open: false,
                        close: false,
                        close_reason: None,
                        paused: false,
                        reliable: true,
                        channel_index: self.lab.peers[&self.address]
                            .player_controller_channel
                            .unwrap(),
                        has_package_map_exports: false,
                        has_must_be_mapped_guids: false,
                        partial: false,
                        reliable_sequence_raw: Some(wire_reliable_sequence(self.raw)),
                        partial_initial: false,
                        partial_final: false,
                        channel_name: Some(Name::Hardcoded(ACTOR_CHANNEL_NAME_INDEX)),
                    },
                    payload,
                }],
            },
            &self.lab.config.packet.packet,
        )
        .unwrap();
        self.raw += 1;
        self.client.commit().unwrap();
        bytes
    }

    fn deliver(&mut self, accepted: bool) -> Vec<Bunch> {
        deliver_bunches(&mut self.lab, &self.socket, &mut self.client, |_| accepted)
    }

    fn completions(&self, bunches: &[Bunch]) -> Vec<u32> {
        let codec = RepLayoutCodec {
            actor: actor_codec(self.plan.engine_net_version(), ByteOrder::Little),
            checksums: false,
            limits: Default::default(),
        };
        let channel = self
            .lab
            .player_controller_open
            .as_ref()
            .unwrap()
            .channel_index;
        bunches
            .iter()
            .filter(|b| {
                b.header.channel_index == channel
                    && b.header.reliable
                    && !b.header.open
                    && !b.header.has_package_map_exports
            })
            .flat_map(|b| {
                read_actor_bunch_body(&mut b.payload.reader(), false, &codec.actor)
                    .unwrap()
                    .blocks
            })
            .filter(|block| block.has_rep_layout)
            .flat_map(|block| {
                read_rep_properties(
                    &mut block.payload.reader(),
                    self.plan.actor_layout(),
                    &codec,
                )
                .unwrap()
                .entries
            })
            .filter(|entry| entry.handle == 26)
            .map(|entry| match entry.value {
                RepValue::Enum(value) => value,
                _ => panic!("leave completion enum"),
            })
            .collect()
    }

    fn phase(&self) -> Option<LeavePhase> {
        self.registry()
            .leave_status(PlayerKey::new(ACCOUNT, 3).unwrap(), self.epoch)
            .map(|s| s.phase)
    }

    fn writer(&self) -> (tempfile::TempDir, CheckpointWriter) {
        let directory = tempfile::tempdir().unwrap();
        if self.lab.private_leave_proof.is_some() {
            use std::os::unix::fs::PermissionsExt;
            // The proof reader deliberately refuses a publicly traversable
            // store; tempfile's directory mode is otherwise umask-dependent.
            std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))
                .unwrap();
        }
        let writer =
            CheckpointWriter::spawn(SnapshotStore::open(directory.path()).unwrap()).unwrap();
        (directory, writer)
    }

    fn save(&mut self, writer: &mut CheckpointWriter) -> crate::persistence::SaveReceipt {
        writer.submit(self.app.world_mut()).unwrap();
        writer
            .finish_before(
                self.app.world_mut(),
                Instant::now() + Duration::from_secs(5),
            )
            .unwrap()
            .unwrap()
    }

    fn pressure(&mut self) {
        let capacity = self.lab.peers[&self.address].peer.config().sent_capacity;
        let pending = self.lab.peers[&self.address]
            .peer
            .sequencer()
            .notify()
            .sent_records()
            .len();
        for _ in pending..capacity {
            let packet = self
                .lab
                .peers
                .get_mut(&self.address)
                .unwrap()
                .peer
                .ack_packet()
                .unwrap();
            assert!(self.lab.send(&packet, self.address));
        }
    }
}

#[test]
fn normal_leave_freezes_without_ticks_and_waits_for_exact_receipt_then_reliable_ack() {
    for kind in [
        Kind::Quit,
        Kind::ReturnToCharacterSelect,
        Kind::ReturnToTitlescreen,
    ] {
        let mut f = Fixture::new(true);
        let (directory, mut writer) = f.writer();
        writer.submit(f.app.world_mut()).unwrap();
        let pinned = f.registry().projection(f.epoch).unwrap();
        f.send(Request::Normal(Some(kind)));
        f.bridge();
        assert_eq!(f.phase(), Some(LeavePhase::Pending));
        assert_eq!(f.registry().leave_counts().cutoffs_accepted, 1);
        assert_eq!(f.registry().leave_counts().durable, 0);
        f.app.update();
        assert_eq!(f.app.world().resource::<SimulationClock>().tick, 0);
        {
            let mut registry = f.app.world_mut().resource_mut::<PlayerRegistry>();
            assert!(registry.activated_epochs().is_empty());
            registry
                .consume_activated(ConsumptionRates::new(1, 1, 1).unwrap())
                .unwrap();
            assert_eq!(registry.projection(f.epoch).unwrap(), pinned);
            assert!(
                registry
                    .edit(
                        PlayerKey::new(ACCOUNT, 3).unwrap(),
                        pinned.revision,
                        pinned.checkpoint
                    )
                    .is_err()
            );
        }
        let older = writer
            .finish_before(f.app.world_mut(), Instant::now() + Duration::from_secs(5))
            .unwrap()
            .unwrap();
        f.bridge();
        assert_eq!(f.phase(), Some(LeavePhase::Pending));
        let before_receipt = f.deliver(true);
        assert!(f.completions(&before_receipt).is_empty());
        // A distinct request cannot replace the first requested destination.
        f.send(Request::Normal(Some(Kind::ReturnToTitlescreen)));
        f.bridge();
        assert_eq!(f.registry().leave_counts().cutoffs_accepted, 1);
        assert_eq!(
            f.lab
                .session_counts()
                .controller_leave
                .unwrap()
                .intents_accepted,
            1
        );
        let receipt = f.save(&mut writer);
        assert_eq!(older.tick, receipt.tick);
        assert!(receipt.generation > older.generation);
        assert_eq!(
            f.phase(),
            Some(LeavePhase::Durable {
                generation: receipt.generation,
                tick: receipt.tick
            })
        );
        let manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(directory.path().join("manifest.json")).unwrap())
                .unwrap();
        assert_eq!(manifest["current"]["number"], receipt.generation);
        f.bridge();
        assert!(
            f.registry().projection(f.epoch).is_some(),
            "enqueue is not ACK settlement"
        );
        let completion = f.deliver(false);
        assert_eq!(f.completions(&completion), vec![kind as u32]);
        f.bridge();
        assert!(f.lab.peers.contains_key(&f.address));
        assert!(f.registry().projection(f.epoch).is_some());
        let transport = f.lab.session_counts().controller_leave.unwrap();
        assert_eq!(transport.completions_admitted, 1);
        assert_eq!(transport.completion_prefix_acked, 0);
        assert!(f.lab.peers[&f.address].peer.pending_resends() > 0);
        // Reliable NAKs queue their unchanged bunch for the next written
        // datagram. Use ordinary ACK traffic to drive that transport step.
        let packet = f
            .lab
            .peers
            .get_mut(&f.address)
            .unwrap()
            .peer
            .ack_packet()
            .unwrap();
        assert!(f.lab.send(&packet, f.address));
        let repair = f.deliver(true);
        assert_eq!(f.completions(&repair), vec![kind as u32]);
        assert_eq!(
            completion
                .iter()
                .find(|b| b.header.reliable)
                .unwrap()
                .payload,
            repair.iter().find(|b| b.header.reliable).unwrap().payload
        );
        f.bridge();
        assert!(!f.lab.peers.contains_key(&f.address));
        assert!(f.registry().projection(f.epoch).is_none());
        assert_eq!(f.phase(), None);
        let counts = f.registry().leave_counts();
        assert_eq!(
            (
                counts.cutoffs_accepted,
                counts.durable,
                counts.detached_settled
            ),
            (1, 1, 1)
        );
        assert_eq!(
            (counts.pending, counts.failed, counts.durable_unsettled),
            (0, 0, 0)
        );
        assert_eq!(counts.last_durable.unwrap().generation, receipt.generation);
        let transport = f.lab.session_counts().controller_leave.unwrap();
        assert_eq!(
            (
                transport.intents_accepted,
                transport.completions_admitted,
                transport.completion_prefix_acked
            ),
            (1, 1, 1)
        );
        assert_eq!(
            f.registry()
                .snapshot()
                .projection(PlayerKey::new(ACCOUNT, 3).unwrap())
                .unwrap()
                .checkpoint,
            pinned.checkpoint
        );
        let expected = f.registry().snapshot();
        drop(writer);
        let deadline = Instant::now() + Duration::from_secs(5);
        let store = loop {
            match SnapshotStore::open_existing(directory.path()) {
                Ok(store) => break store,
                Err(_) if Instant::now() < deadline => std::thread::yield_now(),
                Err(error) => panic!("checkpoint writer did not release store: {error:#}"),
            }
        };
        let restored = store
            .load()
            .unwrap()
            .unwrap()
            .snapshot
            .native_players
            .unwrap();
        assert_eq!(restored, expected);
        let mut restarted = PlayerRegistry::new(restored, &context(), 8).unwrap();
        let projection = restarted
            .reserve(PlayerKey::new(ACCOUNT, 3).unwrap(), 1, pinned.checkpoint)
            .unwrap();
        assert!(!projection.is_new);
        assert_eq!(projection.checkpoint, pinned.checkpoint);
    }
}

#[test]
fn unsupported_requests_and_disabled_dispatch_do_not_cut_off_or_complete() {
    assert!(
        ReleaseProfile::load(std::path::Path::new("config/release.toml"))
            .unwrap()
            .controller_leave_plan()
            .unwrap()
            .is_some()
    );
    for enabled in [false, true] {
        let mut f = Fixture::new(enabled);
        let before = f.registry().snapshot();
        let mut requests = vec![
            Request::Normal(None),
            Request::Normal(Some(Kind::None)),
            Request::Normal(Some(Kind::LeaveByDropship)),
            Request::Normal(Some(Kind::Disconnected)),
            Request::Cancel,
            Request::WithoutSave,
        ];
        if !enabled {
            requests.push(Request::Normal(Some(Kind::Quit)));
        }
        for request in requests {
            f.send(request);
            f.bridge();
            assert_eq!(f.phase(), None);
            assert_eq!(f.registry().snapshot(), before);
            assert_eq!(f.registry().activated_epochs(), vec![f.epoch]);
            let received = f.deliver(true);
            assert!(f.completions(&received).is_empty());
            assert_eq!(f.registry().leave_counts(), Default::default());
            assert_eq!(
                f.lab.session_counts().controller_leave,
                enabled.then_some(Default::default())
            );
        }
    }
}

#[test]
fn cancel_before_admission_suppresses_completion_and_keeps_the_frozen_checkpoint() {
    for stage in ["staged", "pending", "durable"] {
        let mut f = Fixture::new(true);
        let (_directory, mut writer) = f.writer();
        f.send(Request::Normal(Some(Kind::Quit)));
        let before = f.registry().snapshot();
        if stage != "staged" {
            f.bridge();
        }
        if stage == "durable" {
            f.save(&mut writer);
            f.pressure();
            f.bridge();
        }
        f.send(Request::Cancel);
        f.bridge();
        f.send(Request::Cancel);
        f.bridge();
        if stage != "durable" {
            f.save(&mut writer);
        }
        for _ in 0..3 {
            let received = f.deliver(true);
            assert!(f.completions(&received).is_empty());
            f.bridge();
            assert!(matches!(f.phase(), Some(LeavePhase::Durable { .. })));
            assert!(f.registry().activated_epochs().is_empty());
            assert_eq!(f.registry().snapshot(), before);
            assert!(f.lab.peers.contains_key(&f.address));
        }
        f.lab.remove_peer(f.address);
        f.bridge();
        assert_eq!(f.phase(), None);
        assert_eq!(f.registry().snapshot(), before);
        let transport = f.lab.session_counts().controller_leave.unwrap();
        assert_eq!(
            (
                transport.intents_accepted,
                transport.cancel_suppressions,
                transport.completions_admitted
            ),
            (1, 1, 0)
        );
        assert_eq!(f.registry().leave_counts().detached_settled, 1);
    }
}

#[test]
fn completion_waits_under_queue_pressure_and_cancel_after_admission_cannot_retract_it() {
    let mut f = Fixture::new(true);
    let (_directory, mut writer) = f.writer();
    f.send(Request::Normal(Some(Kind::Quit)));
    f.bridge();
    f.save(&mut writer);
    f.pressure();
    for _ in 0..3 {
        f.bridge();
        assert!(matches!(f.phase(), Some(LeavePhase::Durable { .. })));
        assert!(f.registry().projection(f.epoch).is_some());
        assert!(
            !f.app
                .world_mut()
                .resource_mut::<PlayerRegistry>()
                .checkpoint_save_needed(Instant::now())
        );
    }
    let pressure = f.deliver(true);
    assert!(f.completions(&pressure).is_empty());
    f.bridge();
    // The completion has entered the reliable channel but remains unacknowledged.
    f.send(Request::Cancel);
    f.bridge();
    assert!(f.lab.peers.contains_key(&f.address));
    let completion = f.deliver(true);
    assert_eq!(f.completions(&completion), vec![Kind::Quit as u32]);
    f.bridge();
    assert!(!f.lab.peers.contains_key(&f.address));
    assert_eq!(f.phase(), None);
    assert_eq!(
        f.lab
            .session_counts()
            .controller_leave
            .unwrap()
            .cancel_suppressions,
        0
    );
}

#[test]
fn storage_failure_never_emits_completion_or_unfreezes_the_player() {
    let mut f = Fixture::new(true);
    let (directory, mut writer) = f.writer();
    f.send(Request::Normal(Some(Kind::Quit)));
    f.bridge();
    let before = f.registry().snapshot();
    std::fs::create_dir(directory.path().join("manifest.json")).unwrap();
    writer.submit(f.app.world_mut()).unwrap();
    assert!(
        writer
            .finish_before(f.app.world_mut(), Instant::now() + Duration::from_secs(5))
            .is_err()
    );
    assert_eq!(
        f.phase(),
        Some(LeavePhase::Failed(
            crate::native_player_runtime::LeaveFailure::Storage
        ))
    );
    for _ in 0..3 {
        f.bridge();
        let received = f.deliver(true);
        assert!(f.completions(&received).is_empty());
        assert!(f.registry().activated_epochs().is_empty());
        assert_eq!(f.registry().snapshot(), before);
    }
    f.lab.remove_peer(f.address);
    f.bridge();
    assert!(
        f.phase().is_some(),
        "failure keeps the detached checkpoint barrier"
    );
    let counts = f.registry().leave_counts();
    assert_eq!(
        (
            counts.storage_failed,
            counts.failed,
            counts.durable,
            counts.detached_settled
        ),
        (1, 1, 0, 0)
    );
}

#[test]
fn revoked_disconnected_or_reused_destinations_never_receive_late_completion() {
    for destination in 0..3 {
        let mut f = Fixture::new(true);
        let (_directory, mut writer) = f.writer();
        f.send(Request::Normal(Some(Kind::Quit)));
        f.bridge();
        let before = f.registry().snapshot();
        writer.submit(f.app.world_mut()).unwrap();
        let mut replacement_client = None;
        if destination == 0 {
            let (auth, backend) = AuthClient::test_bridge(8);
            f.lab.authenticator = Some(auth);
            backend
                .event(crate::steam_auth::AuthEvent {
                    request_id: 1,
                    outcome: AuthOutcome::Revoked,
                })
                .unwrap();
        } else {
            f.lab.remove_peer(f.address);
        }
        f.bridge();
        assert!(f.registry().projection(f.epoch).is_none());
        assert_eq!(f.phase(), Some(LeavePhase::Pending));
        if destination == 2 {
            let (socket, other, client, _) = peer(&mut f.lab, 2, ACCOUNT + 1, true);
            let replacement = f.lab.peers.remove(&other).unwrap();
            f.lab.peers.insert(f.address, replacement);
            replacement_client = Some((socket, client));
        }
        writer
            .finish_before(f.app.world_mut(), Instant::now() + Duration::from_secs(5))
            .unwrap()
            .unwrap();
        f.bridge();
        assert_eq!(
            f.phase(),
            None,
            "only detached durable obligations can settle without a response"
        );
        assert_eq!(f.registry().snapshot(), before);
        let counts = f.registry().leave_counts();
        assert_eq!(
            (
                counts.cutoffs_accepted,
                counts.durable,
                counts.detached_settled
            ),
            (1, 1, 1)
        );
        let transport = f.lab.session_counts().controller_leave.unwrap();
        assert_eq!(
            (
                transport.completions_admitted,
                transport.completion_prefix_acked
            ),
            (0, 0)
        );
        if let Some((_socket, _client)) = replacement_client {
            assert!(f.lab.peers.contains_key(&f.address));
            assert!(f.lab.peers[&f.address].peer.actor_channel_settled(
                f.lab.player_controller_open.as_ref().unwrap().channel_index
            ));
            let received = f.deliver(true);
            assert!(f.completions(&received).is_empty());
        } else {
            assert!(!f.lab.peers.contains_key(&f.address));
        }
    }
}

#[test]
fn dispatch_cannot_be_enabled_without_checkpoints_or_reconfigured_with_live_peers() {
    let mut unprepared = record_lab(false);
    assert!(unprepared.enable_authored_controller_leave(plan()).is_err());
    let mut mismatched = lab();
    let selected = ReleaseProfile::fixture("3.0.36");
    mismatched.config.local_network_version = Some(selected.pin.network_checksum);
    mismatched.player_controller_open = selected
        .player_controller_open_plan(PeerLabAdmission::default().map_url, [0.0; 3])
        .unwrap();
    mismatched
        .player_controller_open
        .as_mut()
        .unwrap()
        .profile
        .archetype[0]
        .checksum ^= 1;
    assert!(mismatched.enable_authored_controller_leave(plan()).is_err());
    for version in [None, Some(selected.pin.network_checksum ^ 1)] {
        let mut wrong_version = lab();
        wrong_version.player_controller_open = selected
            .player_controller_open_plan(PeerLabAdmission::default().map_url, [0.0; 3])
            .unwrap();
        wrong_version.config.local_network_version = version;
        assert!(
            wrong_version
                .enable_authored_controller_leave(plan())
                .is_err()
        );
    }
    let mut f = Fixture::new(false);
    let before = f.registry().snapshot();
    assert!(f.lab.enable_authored_controller_leave(plan()).is_err());
    f.send(Request::Normal(Some(Kind::Quit)));
    f.bridge();
    assert_eq!(f.phase(), None);
    assert_eq!(f.registry().snapshot(), before);
}

#[test]
fn malformed_tail_cannot_apply_an_earlier_valid_leave() {
    let mut f = Fixture::new(true);
    let before = f.registry().snapshot();
    let bytes = f.datagram(Request::Normal(Some(Kind::Quit)), true);
    receive_authored_at(&mut f.lab, &f.socket, &bytes, Instant::now());
    f.bridge();
    assert_eq!(f.phase(), None);
    assert_eq!(f.registry().snapshot(), before);
    assert_eq!(f.registry().activated_epochs(), vec![f.epoch]);
    let received = f.deliver(true);
    assert!(f.completions(&received).is_empty());
}

#[test]
fn real_poll_plugin_freezes_before_zero_or_multiple_consumption_ticks() {
    use crate::native_player_runtime::{ConsumptionConfig, install_consumption};
    for consuming in [false, true] {
        for duration in [Duration::ZERO, Duration::from_millis(100)] {
            let mut f = Fixture::new(true);
            let pinned = f.registry().projection(f.epoch).unwrap();
            if consuming {
                install_consumption(
                    &mut f.app,
                    ConsumptionConfig {
                        oxygen: 30,
                        water: 30,
                        food: 30,
                    },
                    30,
                )
                .unwrap();
            }
            let bytes = f.datagram(Request::Normal(Some(Kind::Quit)), false);
            let Fixture {
                lab,
                mut app,
                socket,
                address,
                epoch,
                ..
            } = f;
            app.insert_resource(lab).add_plugins(PeerLabPlugin);
            app.update(); // Warm TimePlugin's first zero delta.
            app.insert_resource(TimeUpdateStrategy::ManualDuration(duration));
            socket.send(&bytes).unwrap();
            app.update();
            let registry = app.world().resource::<PlayerRegistry>();
            assert_eq!(
                registry
                    .leave_status(PlayerKey::new(ACCOUNT, 3).unwrap(), epoch)
                    .unwrap()
                    .phase,
                LeavePhase::Pending
            );
            assert_eq!(registry.projection(epoch).unwrap(), pinned);
            assert!(registry.activated_epochs().is_empty());
            let tick = app.world().resource::<SimulationClock>().tick;
            if duration.is_zero() {
                assert_eq!(tick, 0);
            } else {
                assert!(tick >= 2);
            }
            assert!(
                app.world()
                    .resource::<PeerLab>()
                    .peers
                    .contains_key(&address)
            );
        }
    }
}

#[test]
fn completion_ack_waits_for_earlier_reliable_bunch_but_not_later_traffic() {
    check_completion_gap(Fixture::new(true));
}

fn check_completion_gap(mut f: Fixture) -> Fixture {
    let (_directory, mut writer) = f.writer();
    let kind = if f.lab.private_leave_proof.is_some() {
        Kind::ReturnToCharacterSelect
    } else {
        Kind::Quit
    };
    f.send(Request::Normal(Some(kind)));
    f.bridge();
    if let Some(proof) = &f.lab.private_leave_proof {
        proof
            .health()
            .expect("exact registry cutoff before gap test");
    }
    f.save(&mut writer);
    if let Some(proof) = &f.lab.private_leave_proof {
        proof
            .health()
            .expect("exact writer readback before gap test");
    }
    let channel = f.lab.peers[&f.address].player_controller_channel.unwrap();
    let payload = encode_actor_bunch_body(
        &ActorBunchBody {
            spawn: None,
            blocks: vec![],
        },
        &actor_codec(f.plan.engine_net_version(), ByteOrder::Little),
    )
    .unwrap();
    let predecessor = f
        .lab
        .peers
        .get_mut(&f.address)
        .unwrap()
        .peer
        .reliable_actor_bunch(channel, payload.clone())
        .unwrap();
    assert!(f.lab.send(&predecessor, f.address));
    f.bridge();
    let received = deliver_bunches(&mut f.lab, &f.socket, &mut f.client, |index| index != 0);
    assert_eq!(f.completions(&received), vec![kind as u32]);
    f.bridge();
    assert!(
        f.lab.peers.contains_key(&f.address),
        "completion's packet ACK cannot cross an earlier reliable gap"
    );
    assert!(f.registry().projection(f.epoch).is_some());
    assert!(matches!(f.phase(), Some(LeavePhase::Durable { .. })));
    assert_eq!(
        f.lab
            .session_counts()
            .controller_leave
            .unwrap()
            .completion_prefix_acked,
        0
    );
    assert!(f.lab.peers[&f.address].peer.pending_resends() > 0);
    let repair = f
        .lab
        .peers
        .get_mut(&f.address)
        .unwrap()
        .peer
        .ack_packet()
        .unwrap();
    assert!(f.lab.send(&repair, f.address));
    // Later traffic is outside the fixed completion receipt. NAK it while
    // acknowledging the predecessor's repair, so channel-empty is false.
    let later = f
        .lab
        .peers
        .get_mut(&f.address)
        .unwrap()
        .peer
        .reliable_actor_bunch(channel, payload)
        .unwrap();
    assert!(f.lab.send(&later, f.address));
    let received = deliver_bunches(&mut f.lab, &f.socket, &mut f.client, |index| index == 0);
    assert!(
        f.completions(&received).is_empty(),
        "repair retransmits the predecessor, not the already acknowledged completion"
    );
    assert!(!f.lab.peers[&f.address].peer.actor_channel_settled(channel));
    f.bridge();
    assert!(!f.lab.peers.contains_key(&f.address));
    assert!(f.registry().projection(f.epoch).is_none());
    assert_eq!(f.phase(), None);
    f
}

#[test]
fn aggregate_diagnostics_survive_teardown_and_final_writer_observation_without_private_values() {
    let mut f = Fixture::new(true);
    let (_directory, mut writer) = f.writer();
    f.send(Request::Normal(Some(Kind::Quit)));
    f.bridge();
    let transport = f.lab.session_counts().controller_leave;
    let address = f.address.to_string();
    let Fixture { lab, mut app, .. } = f;
    app.insert_resource(lab);
    crate::network::teardown(app.world_mut());
    assert_eq!(
        crate::network::session_counts(app.world())
            .unwrap()
            .controller_leave,
        transport
    );
    assert_eq!(
        app.world()
            .resource::<PlayerRegistry>()
            .leave_counts()
            .pending,
        1
    );
    writer.submit(app.world_mut()).unwrap();
    let receipt = writer
        .finish_before(app.world_mut(), Instant::now() + Duration::from_secs(5))
        .unwrap()
        .unwrap();
    let counts = app.world().resource::<PlayerRegistry>().leave_counts();
    assert_eq!(
        (
            counts.cutoffs_accepted,
            counts.pending,
            counts.durable,
            counts.durable_unsettled
        ),
        (1, 0, 1, 1)
    );
    assert_eq!(counts.last_durable.unwrap().generation, receipt.generation);
    assert_eq!(
        crate::network::session_counts(app.world())
            .unwrap()
            .controller_leave,
        transport
    );
    let encoded = serde_json::to_string(
        &serde_json::json!({"player_leave": counts, "controller_leave": transport}),
    )
    .unwrap();
    for sentinel in [
        ACCOUNT.to_string(),
        address,
        "PRIVATE-NAME-SENTINEL".into(),
        "UNTRUSTED-LOGIN-SENTINEL".into(),
    ] {
        assert!(!encoded.contains(&sentinel));
    }
    for key in [
        "account",
        "slot",
        "epoch",
        "revision",
        "checkpoint",
        "location",
        "vitals",
        "ticket",
        "address",
        "guid",
        "hash",
    ] {
        assert!(!encoded.contains(&format!("\"{key}\"")));
    }
}

#[test]
fn early_pawn_leave_keeps_types_ack_detach_and_cancel_suppression() {
    for kind in [
        Kind::Quit,
        Kind::ReturnToCharacterSelect,
        Kind::ReturnToTitlescreen,
    ] {
        let mut f = Fixture::early();
        assert!(f.lab.has_checkpoint_early_pawn());
        assert_eq!(f.registry().activated_epochs(), vec![f.epoch]);
        let (_directory, mut writer) = f.writer();
        let pinned = f.registry().projection(f.epoch).unwrap();
        f.send(Request::Normal(Some(kind)));
        f.bridge();
        assert_eq!(f.phase(), Some(LeavePhase::Pending));
        assert!(f.registry().activated_epochs().is_empty());
        let before_receipt = f.deliver(true);
        assert!(f.completions(&before_receipt).is_empty());
        let receipt = f.save(&mut writer);
        assert_eq!(
            f.phase(),
            Some(LeavePhase::Durable {
                generation: receipt.generation,
                tick: receipt.tick
            })
        );
        f.bridge();
        let completion = f.deliver(false);
        assert_eq!(f.completions(&completion), vec![kind as u32]);
        f.bridge();
        assert!(f.lab.peers.contains_key(&f.address), "NAK is no settlement");
        let packet = f
            .lab
            .peers
            .get_mut(&f.address)
            .unwrap()
            .peer
            .ack_packet()
            .unwrap();
        assert!(f.lab.send(&packet, f.address));
        let repair = f.deliver(true);
        assert_eq!(f.completions(&repair), vec![kind as u32]);
        f.bridge();
        assert!(!f.lab.peers.contains_key(&f.address));
        assert!(f.registry().projection(f.epoch).is_none());
        assert_eq!(f.phase(), None);
        let counts = f.registry().leave_counts();
        assert_eq!(
            (
                counts.cutoffs_accepted,
                counts.durable,
                counts.detached_settled
            ),
            (1, 1, 1)
        );
        let transport = f.lab.session_counts().controller_leave.unwrap();
        assert_eq!(
            (
                transport.intents_accepted,
                transport.completions_admitted,
                transport.completion_prefix_acked
            ),
            (1, 1, 1)
        );
        assert_eq!(
            f.registry()
                .snapshot()
                .projection(PlayerKey::new(ACCOUNT, 3).unwrap())
                .unwrap()
                .checkpoint,
            pinned.checkpoint
        );
    }
    let mut f = Fixture::early();
    let (_directory, mut writer) = f.writer();
    f.send(Request::Normal(Some(Kind::Quit)));
    f.bridge();
    let before = f.registry().snapshot();
    f.send(Request::Cancel);
    f.bridge();
    f.save(&mut writer);
    for _ in 0..3 {
        let received = f.deliver(true);
        assert!(f.completions(&received).is_empty());
        f.bridge();
        assert!(matches!(f.phase(), Some(LeavePhase::Durable { .. })));
        assert_eq!(f.registry().snapshot(), before);
        assert!(f.lab.peers.contains_key(&f.address));
    }
    f.lab.remove_peer(f.address);
    f.bridge();
    assert_eq!(f.phase(), None);
    let transport = f.lab.session_counts().controller_leave.unwrap();
    assert_eq!(
        (
            transport.intents_accepted,
            transport.cancel_suppressions,
            transport.completions_admitted
        ),
        (1, 1, 0)
    );
    assert_eq!(f.registry().leave_counts().detached_settled, 1);
}

#[test]
fn early_pawn_leave_before_identity_is_ignored() {
    let (mut lab, mut registry) = (lab(), registry());
    let selected = ReleaseProfile::fixture("3.0.36");
    lab.config.local_network_version = Some(selected.pin.network_checksum);
    lab.player_controller_open = selected
        .player_controller_open_plan(PeerLabAdmission::default().map_url, [0.0; 3])
        .unwrap();
    lab.enable_authored_controller_leave(plan()).unwrap();
    lab.enable_checkpoint_early_pawn().unwrap();
    let (socket, address, mut client, _) = peer(&mut lab, 1, ACCOUNT, true);
    assert!(
        lab.peers[&address]
            .peer
            .has_actor_channel(PAWN_CHANNEL_INDEX)
    );
    for request in [
        Request::Normal(Some(Kind::Quit)),
        Request::Cancel,
        Request::Normal(Some(Kind::ReturnToTitlescreen)),
    ] {
        lab.controller_call(address, ControllerCall::Leave(request));
        lab.bridge_controller_leaves(&mut registry);
        assert!(lab.peers[&address].controller_leave.is_none());
    }
    assert_eq!(
        lab.session_counts().controller_leave,
        Some(Default::default())
    );
    identify(&mut lab, &socket, &mut client, address, 3, 3);
    lab.bridge_player_checkpoints(&mut registry);
    deliver_bunches(&mut lab, &socket, &mut client, |_| true);
    finish(&mut lab, &mut registry, &socket, &mut client, address);
    let epoch = lab.peers[&address].checkpoint_epoch.unwrap();
    lab.bridge_controller_leaves(&mut registry);
    assert_eq!(registry.activated_epochs(), vec![epoch]);
    assert_eq!(registry.leave_counts(), Default::default());
    assert_eq!(
        registry.leave_status(PlayerKey::new(ACCOUNT, 3).unwrap(), epoch),
        None
    );
}

#[test]
fn projected_bag_leave_keeps_types_ack_detach_and_sends_no_bag_after_cutoff() {
    use super::bag_projection_tests::bag_deltas;
    for early in [true, false] {
        for kind in [
            Kind::Quit,
            Kind::ReturnToCharacterSelect,
            Kind::ReturnToTitlescreen,
        ] {
            let mut f = Fixture::projected(early);
            assert!(f.lab.has_player_inventory_projection());
            assert_eq!(f.registry().activated_epochs(), vec![f.epoch]);
            let connection = f.lab.peers[&f.address].connection_id.unwrap();
            let projected = f.lab.session_counts().player_bag_projection.unwrap();
            let bags = crate::simulation::capture(f.app.world_mut())
                .unwrap()
                .player_inventory;
            let (_directory, mut writer) = f.writer();
            let pinned = f.registry().projection(f.epoch).unwrap();
            f.send(Request::Normal(Some(kind)));
            f.bridge();
            assert_eq!(f.phase(), Some(LeavePhase::Pending));
            assert!(f.registry().activated_epochs().is_empty());
            let before_receipt = f.deliver(true);
            assert!(f.completions(&before_receipt).is_empty());
            let receipt = f.save(&mut writer);
            assert_eq!(
                f.phase(),
                Some(LeavePhase::Durable {
                    generation: receipt.generation,
                    tick: receipt.tick
                })
            );
            f.bridge();
            let completion = f.deliver(false);
            assert_eq!(f.completions(&completion), vec![kind as u32]);
            f.bridge();
            assert!(f.lab.peers.contains_key(&f.address), "NAK is no settlement");
            let packet = f
                .lab
                .peers
                .get_mut(&f.address)
                .unwrap()
                .peer
                .ack_packet()
                .unwrap();
            assert!(f.lab.send(&packet, f.address));
            let repair = f.deliver(true);
            assert_eq!(f.completions(&repair), vec![kind as u32]);
            for bunches in [&before_receipt, &completion, &repair] {
                assert!(bag_deltas(&f.lab, connection, bunches).is_empty());
            }
            assert_eq!(
                f.lab.session_counts().player_bag_projection.unwrap(),
                projected,
                "the leave sends and observes no bag"
            );
            f.bridge();
            assert!(!f.lab.peers.contains_key(&f.address));
            assert!(f.registry().projection(f.epoch).is_none());
            assert_eq!(f.phase(), None);
            let counts = f.registry().leave_counts();
            assert_eq!(
                (
                    counts.cutoffs_accepted,
                    counts.durable,
                    counts.detached_settled
                ),
                (1, 1, 1)
            );
            assert_eq!(
                f.registry()
                    .snapshot()
                    .projection(PlayerKey::new(ACCOUNT, 3).unwrap())
                    .unwrap()
                    .checkpoint,
                pinned.checkpoint
            );
            assert_eq!(
                crate::simulation::capture(f.app.world_mut())
                    .unwrap()
                    .player_inventory,
                bags,
                "the projection never edits the pinned bag"
            );
        }
    }
}

#[test]
fn a_bag_change_pending_at_the_leave_cutoff_is_never_sent() {
    use super::bag_projection_tests::{act_at, bag_deltas, bridge as full_bridge, endpoint};
    use crate::native_player_inventory::PlayerInventoryActionV1;
    for early in [true, false] {
        let mut f = Fixture::projected(early);
        let connection = f.lab.peers[&f.address].connection_id.unwrap();
        let player = f.lab.peers[&f.address]
            .checkpoint_projection
            .unwrap()
            .player_id;
        let projected = f.lab.session_counts().player_bag_projection.unwrap();
        // A Use commits in the same interval as the leave request: no
        // checkpoint bridge has projected it when the cut-off is taken.
        let source = endpoint(&f.app, player, "bag", 0);
        act_at(
            &mut f.app,
            &f.lab,
            f.address,
            PlayerInventoryActionV1::Use { source },
        );
        let bags = crate::simulation::capture(f.app.world_mut())
            .unwrap()
            .player_inventory;
        let (_directory, mut writer) = f.writer();
        f.send(Request::Normal(Some(Kind::Quit)));
        f.bridge();
        assert_eq!(f.phase(), Some(LeavePhase::Pending), "early {early}");
        let mut after_cutoff = Vec::new();
        // `run` bridges checkpoints after the leave bridge in every interval.
        for _ in 0..3 {
            full_bridge(&mut f.lab, &mut f.app);
            after_cutoff.extend(f.deliver(true));
        }
        assert!(
            bag_deltas(&f.lab, connection, &after_cutoff).is_empty(),
            "the committed Use is not sent after the cut-off (early {early})"
        );
        let bag = f.lab.peers[&f.address].bag_projection.as_ref().unwrap();
        assert!(bag.is_frozen() && !bag.due());
        assert_eq!(bag.target_revision(), Some(1), "the Use is never looked up");
        let receipt = f.save(&mut writer);
        assert_eq!(
            f.phase(),
            Some(LeavePhase::Durable {
                generation: receipt.generation,
                tick: receipt.tick
            })
        );
        f.bridge();
        full_bridge(&mut f.lab, &mut f.app);
        let completion = f.deliver(false);
        after_cutoff.extend(completion.iter().cloned());
        assert_eq!(f.completions(&completion), vec![Kind::Quit as u32]);
        f.bridge();
        let packet = f
            .lab
            .peers
            .get_mut(&f.address)
            .unwrap()
            .peer
            .ack_packet()
            .unwrap();
        assert!(f.lab.send(&packet, f.address));
        let repair = f.deliver(true);
        assert_eq!(f.completions(&repair), vec![Kind::Quit as u32]);
        after_cutoff.extend(repair);
        assert!(
            bag_deltas(&f.lab, connection, &after_cutoff).is_empty(),
            "no bag bytes after the cut-off (early {early})"
        );
        let counts = f.lab.session_counts().player_bag_projection.unwrap();
        assert_eq!(
            (counts.updates_queued, counts.revisions_observed),
            (projected.updates_queued, projected.revisions_observed)
        );
        assert_eq!(counts.sessions_closed, 0);
        f.bridge();
        assert!(!f.lab.peers.contains_key(&f.address), "the leave completes");
        assert!(f.registry().projection(f.epoch).is_none());
        assert_eq!(f.registry().leave_counts().detached_settled, 1);
        assert_eq!(
            crate::simulation::capture(f.app.world_mut())
                .unwrap()
                .player_inventory,
            bags,
            "the store holds the committed bag, Use included"
        );
    }
}

/// Both location options on (with the early pawn and move reading): a normal
/// leave saves the player's last accepted move location in the same pass,
/// just before the cut-off, so the pinned and durable record carries it;
/// refused and based moves never reach it; nothing after the cut-off does;
/// leave types, completion, ACK, detach and Cancel suppression are unchanged.
#[test]
fn located_leave_saves_the_last_accepted_location_before_its_cutoff() {
    let key = PlayerKey::new(ACCOUNT, 3).unwrap();
    for kind in [
        Kind::Quit,
        Kind::ReturnToCharacterSelect,
        Kind::ReturnToTitlescreen,
    ] {
        let mut f = Fixture::located();
        assert!(
            f.lab.has_checkpoint_saved_locations() && f.lab.has_checkpoint_location_correction()
        );
        let (directory, mut writer) = f.writer();
        let before = f.registry().projection(f.epoch).unwrap();
        assert_eq!(before.checkpoint.location_cm, [100.0, 200.0, 300.0]);
        super::located_moves(
            &mut f.lab,
            &f.socket,
            &mut f.client,
            Instant::now(),
            [100.0, 200.0, 300.0],
            4.0,
        );
        assert_eq!(f.lab.stats.pawn_move_positions_rejected, 1);
        assert_eq!(f.lab.stats.pawn_move_positions_skipped, 1);
        f.send(Request::Normal(Some(kind)));
        f.bridge();
        assert_eq!(f.phase(), Some(LeavePhase::Pending));
        let status = f.registry().leave_status(key, f.epoch).unwrap();
        // One revision for the location, then the cut-off pins it.
        assert_eq!(status.revision, before.revision + 1);
        let pinned = f.registry().snapshot().projection(key).unwrap();
        assert_eq!(pinned.revision, status.revision);
        assert_eq!(pinned.checkpoint.location_cm, [130.0, 200.0, 300.0]);
        assert_eq!(pinned.checkpoint.vitals, before.checkpoint.vitals);
        assert_eq!(
            f.lab.session_counts().checkpoint_saved_locations,
            Some(SavedLocationCounts {
                saved: 1,
                saved_at_leave: 1,
                refused: 0
            })
        );
        // An accepted move after the cut-off is refused at every pass.
        super::move_now(
            &mut f.lab,
            &f.socket,
            &mut f.client,
            Some(super::super::move_acks::authored_move(
                9.0,
                [140.0, 200.0, 300.0],
                Some(1),
            )),
            Instant::now(),
        );
        f.lab.flush_saved_locations(f.app.world_mut());
        f.bridge();
        assert_eq!(f.registry().snapshot().projection(key).unwrap(), pinned);
        assert_eq!(f.registry().leave_counts().cutoffs_accepted, 1);
        let refused = f.lab.session_counts().checkpoint_saved_locations.unwrap();
        assert_eq!((refused.saved, refused.refused), (1, 1));
        let receipt = f.save(&mut writer);
        assert_eq!(
            f.phase(),
            Some(LeavePhase::Durable {
                generation: receipt.generation,
                tick: receipt.tick
            })
        );
        f.bridge();
        let completion = f.deliver(false);
        assert_eq!(f.completions(&completion), vec![kind as u32]);
        f.bridge();
        assert!(f.lab.peers.contains_key(&f.address), "NAK is no settlement");
        let packet = f
            .lab
            .peers
            .get_mut(&f.address)
            .unwrap()
            .peer
            .ack_packet()
            .unwrap();
        assert!(f.lab.send(&packet, f.address));
        let repair = f.deliver(true);
        assert_eq!(f.completions(&repair), vec![kind as u32]);
        f.bridge();
        assert!(!f.lab.peers.contains_key(&f.address));
        assert_eq!(f.phase(), None);
        let counts = f.registry().leave_counts();
        assert_eq!(
            (
                counts.cutoffs_accepted,
                counts.durable,
                counts.detached_settled
            ),
            (1, 1, 1)
        );
        let transport = f.lab.session_counts().controller_leave.unwrap();
        assert_eq!(
            (
                transport.intents_accepted,
                transport.completions_admitted,
                transport.completion_prefix_acked
            ),
            (1, 1, 1)
        );
        drop(writer);
        let deadline = Instant::now() + Duration::from_secs(5);
        let store = loop {
            match SnapshotStore::open_existing(directory.path()) {
                Ok(store) => break store,
                Err(_) if Instant::now() < deadline => std::thread::yield_now(),
                Err(error) => panic!("checkpoint writer did not release store: {error:#}"),
            }
        };
        let restored = store
            .load()
            .unwrap()
            .unwrap()
            .snapshot
            .native_players
            .unwrap();
        assert_eq!(
            restored.projection(key).unwrap().checkpoint,
            pinned.checkpoint
        );
    }
    // Cancel before admission suppresses the completion; the pinned
    // location stands.
    let mut f = Fixture::located();
    let (_directory, mut writer) = f.writer();
    super::move_now(
        &mut f.lab,
        &f.socket,
        &mut f.client,
        Some(super::super::move_acks::authored_move(
            4.0,
            [150.0, 200.0, 300.0],
            Some(1),
        )),
        Instant::now(),
    );
    f.send(Request::Normal(Some(Kind::Quit)));
    f.bridge();
    let before = f.registry().snapshot();
    assert_eq!(
        before.projection(key).unwrap().checkpoint.location_cm,
        [150.0, 200.0, 300.0]
    );
    f.send(Request::Cancel);
    f.bridge();
    f.save(&mut writer);
    for _ in 0..3 {
        let received = f.deliver(true);
        assert!(f.completions(&received).is_empty());
        f.bridge();
        assert!(matches!(f.phase(), Some(LeavePhase::Durable { .. })));
        assert_eq!(f.registry().snapshot(), before);
    }
    f.lab.remove_peer(f.address);
    f.bridge();
    assert_eq!(f.phase(), None);
    let transport = f.lab.session_counts().controller_leave.unwrap();
    assert_eq!(
        (
            transport.intents_accepted,
            transport.cancel_suppressions,
            transport.completions_admitted
        ),
        (1, 1, 0)
    );
}

/// The completion-gap check with both location options on: the location
/// saved before the cut-off adds no reliable traffic of its own, so the
/// completion's receipt and its ACK prefix are unchanged.
#[test]
fn located_completion_ack_waits_for_earlier_reliable_bunch_but_not_later_traffic() {
    let mut f = Fixture::located();
    assert!(f.lab.has_checkpoint_saved_locations() && f.lab.has_checkpoint_location_correction());
    let moved = super::move_now(
        &mut f.lab,
        &f.socket,
        &mut f.client,
        Some(super::super::move_acks::authored_move(
            4.0,
            [150.0, 200.0, 300.0],
            Some(1),
        )),
        Instant::now(),
    );
    assert!(moved.iter().all(|bunch| !bunch.header.reliable));
    let f = check_completion_gap(f);
    let key = PlayerKey::new(ACCOUNT, 3).unwrap();
    assert_eq!(
        f.registry()
            .snapshot()
            .projection(key)
            .unwrap()
            .checkpoint
            .location_cm,
        [150.0, 200.0, 300.0]
    );
    let saved = f.lab.session_counts().checkpoint_saved_locations.unwrap();
    assert_eq!((saved.saved, saved.saved_at_leave), (1, 1));
}

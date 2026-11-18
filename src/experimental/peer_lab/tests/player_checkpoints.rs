//! Authored bridge contracts. Synthetic backend acceptance below exercises the
//! post-authentication boundary; it does not verify a Steam ticket or client.
use super::super::super::{
    actor_channel::{read_actor_bunch_body, read_player_controller_open_bunch_body, read_spawn},
    archive::ByteOrder,
    control_messages::{NetIdType, NetIdValue, UniqueNetId},
    packet::{BitWriter, Bunch, Payload},
    pawn_open::{CharacterVitals, PAWN_CHANNEL_INDEX, character_state_handle as h},
    pawn_restore::compose_live_pawn_vitals_update,
    probe_initialise::{authored_character_profile, authored_user_profile},
    rep_layout::{RepLayoutCodec, append_bits, read_rep_properties},
    rep_rpc::{RpcArgument, RpcItem, backend_profile_response_parameters, encode_rpc_payload},
    sequencing::PacketSequencer,
};
use super::*;
use crate::{
    native_player_runtime::PlayerRegistry,
    native_players::{PlayerCheckpoint, PlayerContext, PlayerKey, PlayerSnapshot},
    persistence::SnapshotStore,
    simulation::{SNAPSHOT_SCHEMA_PLAYERS, WorldSnapshot},
    spawn_placement::{SpawnConfig, SpawnPoint},
};

const ACCOUNT: u64 = 76_561_197_960_265_770;

#[path = "bag_projection.rs"]
mod bag_projection_tests;
#[path = "begin_timeout.rs"]
mod begin_timeout;
#[path = "controller_leave.rs"]
mod controller_leave_tests;
#[path = "restart_rehearsal.rs"]
mod restart_rehearsal;

fn vitals() -> CharacterVitals {
    CharacterVitals {
        health: 100,
        max_health: 100,
        stamina: 90,
        max_stamina: 100,
        oxygen: 80,
        max_oxygen: 100,
        water: 70,
        max_water: 100,
        food: 60,
        max_food: 100,
        biome_row: 0,
        growth_row: 0,
    }
}

fn context() -> PlayerContext {
    PlayerContext::new(
        "authored-checkpoint".encode_utf16().collect(),
        PeerLabAdmission::default().map_url.to_owned(),
        crate::native_players::PlayerInputs {
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

fn registry() -> PlayerRegistry {
    PlayerRegistry::new(PlayerSnapshot::empty(context()), &context(), 8).unwrap()
}

fn lab() -> PeerLab {
    let mut lab = record_lab(false);
    lab.enable_spawn_points(&SpawnConfig {
        points: vec![
            SpawnPoint {
                x: 100.0,
                y: 200.0,
                z: 300.0,
                yaw: None,
            },
            SpawnPoint {
                x: 400.0,
                y: 500.0,
                z: 600.0,
                yaw: None,
            },
        ],
    })
    .unwrap();
    lab.enable_player_checkpoints(vitals()).unwrap();
    lab
}

fn peer(
    lab: &mut PeerLab,
    connection: u32,
    account: u64,
    accepted: bool,
) -> (UdpSocket, SocketAddrV4, PacketSequencer, Vec<Bunch>) {
    let (socket, address, mut client) = joined_peer(lab, connection, account, accepted);
    assert!(lab.send_player_controller_open(address));
    let bunches = deliver_bunches(lab, &socket, &mut client, |_| true);
    (socket, address, client, bunches)
}

fn joined_peer(
    lab: &mut PeerLab,
    connection: u32,
    account: u64,
    accepted: bool,
) -> (UdpSocket, SocketAddrV4, PacketSequencer) {
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
        name_units: "PRIVATE-NAME-SENTINEL".encode_utf16().collect(),
    });
    entry.login_unique_id = Some(UniqueNetId {
        kind: NetIdType::Named(ArchiveText::text("NULL").unwrap()),
        value: NetIdValue::Text(ArchiveText::text("UNTRUSTED-LOGIN-SENTINEL").unwrap()),
    });
    entry.admission.state = ClientLoginState::Joined;
    entry.auth_request = Some(u64::from(connection));
    entry.auth_identity = Some(account);
    entry.auth_accepted = accepted;
    (socket, address, client)
}

/// Same initial manager plans and authored-world preparation as native hosting.
/// Interleave an authored Quest before FLOD to exercise both sorted extra kinds.
fn world_bootstrap(lab: &mut PeerLab) -> Vec<u32> {
    use super::super::super::{quest_open::StitchedQuest, session_open::PathSegment};
    use crate::authored_world_bootstrap::AuthoredWorldBootstrap;

    let profile = ReleaseProfile::fixture("3.0.32");
    let map = PeerLabAdmission::default().map_url;
    lab.enable_manager_opens(profile.manager_open_plans(map).unwrap(), 6)
        .unwrap();
    let base = profile.quest_open_base_plan(map).unwrap().unwrap();
    let quest = base
        .quest_plan(&StitchedQuest {
            key: ActorKey::Saved(777),
            archetype: vec![
                PathSegment::new("AuthoredQuest", 40, false),
                PathSegment::new("/Lab/Quest", 0, false),
            ],
            location: None,
            quest_row: 0,
            relevant_actors: Vec::new(),
            relevant_characters: Vec::new(),
            active_quests: Vec::new(),
            variables: Vec::new(),
            subquests: Vec::new(),
            parent: None,
        })
        .unwrap();
    lab.enable_quest_opens(base).unwrap();
    let quest_channel = lab.add_quest_open(quest).unwrap();
    let (values, flod) = AuthoredWorldBootstrap {
        seed: 123,
        seed_initialised: true,
        flod_tile_count: 3,
        clock: None,
    }
    .plan(&profile, map)
    .unwrap();
    lab.extend_game_state_values(values).unwrap();
    let flod_channel = lab.add_manager_open(ManagerSlot::Flod, flod).unwrap();
    assert!(quest_channel < flod_channel);
    (6..=flod_channel).collect()
}

fn open_channels(bunches: &[Bunch]) -> Vec<u32> {
    bunches
        .iter()
        .filter(|bunch| bunch.header.open)
        .map(|bunch| {
            assert!(bunch.header.reliable);
            bunch.header.channel_index
        })
        .collect()
}

fn assert_no_checkpoint_pawn(lab: &PeerLab, registry: &PlayerRegistry, address: SocketAddrV4) {
    let entry = &lab.peers[&address];
    assert_eq!(entry.checkpoint_epoch, None);
    assert_eq!(entry.checkpoint_projection, None);
    assert!(!entry.checkpoint_active);
    assert_eq!(entry.placement, None);
    assert_eq!(entry.controller_startup_pawn_guid, None);
    assert!(!entry.peer.has_actor_channel(PAWN_CHANNEL_INDEX));
    assert_eq!(lab.next_checkpoint_epoch, 1);
    assert!(registry.active_epochs().is_empty());
    assert!(registry.activated_epochs().is_empty());
    assert!(registry.snapshot().is_empty());
    assert_eq!(lab.stats.pawn_opens_sent, 0);
    assert_eq!(lab.stats.pawn_spawns_configured, 0);
    assert_eq!(lab.stats.pawn_spawns_default, 0);
    assert_eq!(lab.stats.pawn_spawns_saved, 0);
    assert_eq!(
        lab.spawn.peek(None, [0.0; 3]).placement.location,
        [100.0, 200.0, 300.0]
    );
}

#[test]
fn independent_world_bootstrap_settles_before_begin_and_legacy_keeps_pawn_first() {
    for checkpoint in [false, true] {
        let mut lab = if checkpoint { lab() } else { record_lab(false) };
        let extras = world_bootstrap(&mut lab);
        let mut registry = registry();
        let (socket, address, mut client, mut opened) = peer(&mut lab, 1, ACCOUNT, true);
        opened.extend(deliver_bunches(&mut lab, &socket, &mut client, |_| true));
        let mut expected = if checkpoint {
            vec![2, 4, 5]
        } else {
            vec![2, 3, 4, 5]
        };
        expected.extend(extras);
        assert_eq!(open_channels(&opened), expected);
        let outcomes = lab.readiness_counts().initial_open_outcomes;
        assert_eq!(outcomes.committed.controller, 1);
        assert_eq!(outcomes.committed.game_state, 1);
        assert_eq!(outcomes.committed.player_state, 1);
        assert_eq!(outcomes.committed.managers, lab.manager_opens.len());
        assert_eq!(outcomes.committed.quests, 1);
        assert_eq!(outcomes.settled, outcomes.committed);
        assert!(outcomes.awaiting_ack.is_empty());
        assert!(outcomes.failed.is_empty());
        assert!(outcomes.skipped.is_empty());
        if checkpoint {
            for _ in 0..3 {
                lab.bridge_player_checkpoints(&mut registry);
                lab.retry_initial_opens();
                assert_no_checkpoint_pawn(&lab, &registry, address);
                assert_eq!(lab.readiness_counts().initial_open_outcomes, outcomes);
            }
            assert_eq!(outcomes.pending.pawn, 1);
        } else {
            assert_eq!(outcomes.committed.pawn, 1);
            assert!(outcomes.pending.is_empty());
        }
    }
}

#[test]
fn pre_begin_pressure_and_loss_retry_independent_opens_without_recomposition() {
    let (mut lab, mut registry) = (lab(), registry());
    let extras = world_bootstrap(&mut lab);
    lab.config.packet.sent_capacity = 6;
    let (socket, address, mut client) = joined_peer(&mut lab, 1, ACCOUNT, true);
    // Fill the actual sent ring before bootstrap; no independent stage fits.
    for _ in 0..6 {
        let datagram = lab
            .peers
            .get_mut(&address)
            .unwrap()
            .peer
            .ack_packet()
            .unwrap();
        assert!(lab.send(&datagram, address));
    }
    assert!(lab.send_player_controller_open(address));
    assert_eq!(
        lab.readiness_counts()
            .initial_open_outcomes
            .deferred
            .controller,
        1
    );
    for _ in 0..3 {
        lab.retry_initial_opens();
        lab.bridge_player_checkpoints(&mut registry);
        assert_no_checkpoint_pawn(&lab, &registry, address);
    }
    deliver_bunches(&mut lab, &socket, &mut client, |_| true);
    lab.retry_initial_opens();
    let deferred = lab.readiness_counts().initial_open_outcomes.deferred;
    let mut independent_deferred = deferred.managers + deferred.quests > 0;
    // Lose the first manager packet after the three identity opens.
    let first = deliver_bunches(&mut lab, &socket, &mut client, |index| index != 3);
    assert!(lab.readiness_counts().resend_bunches_pending > 0);
    let mut transmitted = first;
    for _ in 0..32 {
        lab.retry_initial_opens();
        let deferred = lab.readiness_counts().initial_open_outcomes.deferred;
        independent_deferred |= deferred.managers + deferred.quests > 0;
        lab.bridge_player_checkpoints(&mut registry);
        assert_no_checkpoint_pawn(&lab, &registry, address);
        if lab.readiness_counts().resend_bunches_pending > 0 {
            let datagram = lab
                .peers
                .get_mut(&address)
                .unwrap()
                .peer
                .ack_packet()
                .unwrap();
            assert!(lab.send(&datagram, address));
        }
        transmitted.extend(deliver_bunches(&mut lab, &socket, &mut client, |_| true));
        let outcomes = lab.readiness_counts().initial_open_outcomes;
        if outcomes.pending
            == (InitialOpenStageCounts {
                pawn: 1,
                ..Default::default()
            })
            && outcomes.awaiting_ack.is_empty()
        {
            break;
        }
    }
    // A reliable retry may repeat a transmission, but keeps the same channel,
    // reliable sequence and exact payload. First admissions retain stage order.
    let mut seen = BTreeMap::new();
    let mut unique = Vec::new();
    let mut repaired = false;
    for bunch in transmitted.iter().filter(|bunch| bunch.header.reliable) {
        let key = (
            bunch.header.channel_index,
            bunch.header.reliable_sequence_raw,
        );
        if let Some(previous) = seen.insert(key, bunch) {
            assert_eq!(bunch, previous);
            repaired = true;
        } else {
            unique.push(bunch.clone());
        }
    }
    let mut expected = vec![2, 4, 5];
    expected.extend(extras);
    assert!(independent_deferred);
    assert!(repaired);
    assert_eq!(open_channels(&unique), expected);
    let outcomes = lab.readiness_counts().initial_open_outcomes;
    assert_eq!(outcomes.committed.managers, lab.manager_opens.len());
    assert_eq!(outcomes.committed.quests, 1);
    assert_eq!(outcomes.settled, outcomes.committed);
    assert_eq!(
        outcomes.pending,
        InitialOpenStageCounts {
            pawn: 1,
            ..Default::default()
        }
    );
    assert!(outcomes.failed.is_empty());
    assert!(outcomes.skipped.is_empty());
    assert_eq!(lab.stats.player_controller_opens_sent, 1);
}

fn response(kind: ProfileKind, slot: i32) -> ProfileResponse {
    let mut values = vec![RpcItem::Value(RepValue::Bool(true))];
    values.extend(match kind {
        ProfileKind::User => authored_user_profile().unwrap(),
        ProfileKind::Character => authored_character_profile(slot).unwrap(),
    });
    let codec = RepLayoutCodec {
        actor: actor_codec(17, ByteOrder::Little),
        checksums: false,
        limits: Default::default(),
    };
    let payload = encode_rpc_payload(
        &backend_profile_response_parameters(kind == ProfileKind::Character),
        &[RpcArgument::Sent(values)],
        &codec,
    )
    .unwrap();
    profile_exchange::parse_response(&payload, kind, &codec).unwrap()
}

fn identify(
    lab: &mut PeerLab,
    socket: &UdpSocket,
    client: &mut PacketSequencer,
    address: SocketAddrV4,
    begin: i32,
    slot: i32,
) {
    lab.controller_call(address, ControllerCall::Begin(begin));
    if !lab.peers.contains_key(&address) {
        return;
    }
    deliver_bunches(lab, socket, client, |_| true);
    lab.controller_call(
        address,
        ControllerCall::Profile(response(ProfileKind::User, slot)),
    );
    if !lab.peers.contains_key(&address) {
        return;
    }
    deliver_bunches(lab, socket, client, |_| true);
    lab.controller_call(
        address,
        ControllerCall::Profile(response(ProfileKind::Character, slot)),
    );
    if lab.peers.contains_key(&address) {
        deliver_bunches(lab, socket, client, |_| true);
    }
}

fn finish(
    lab: &mut PeerLab,
    registry: &mut PlayerRegistry,
    socket: &UdpSocket,
    client: &mut PacketSequencer,
    address: SocketAddrV4,
) {
    lab.controller_call(
        address,
        ControllerCall::Initialisation(InitialisationAnswer::Loadout),
    );
    lab.controller_call(
        address,
        ControllerCall::Initialisation(InitialisationAnswer::Bestiary),
    );
    lab.controller_call(address, ControllerCall::FriendsList);
    lab.bridge_player_checkpoints(registry);
    deliver_bunches(lab, socket, client, |_| true);
    assert!(lab.peers[&address].profiles.finalised);
}

fn pawn_payload(bunches: &[Bunch]) -> Payload {
    let mut body = BitWriter::new();
    for bunch in bunches.iter().filter(|bunch| {
        bunch.header.channel_index == PAWN_CHANNEL_INDEX && !bunch.header.has_package_map_exports
    }) {
        append_bits(&mut body, &bunch.payload);
        if !bunch.header.partial || bunch.header.partial_final {
            break;
        }
    }
    Payload::from_writer(body)
}

fn health(lab: &PeerLab, bunches: &[Bunch], open: bool) -> i32 {
    let plan = &lab.pawn_open.as_ref().unwrap().0;
    let codec = RepLayoutCodec {
        actor: actor_codec(plan.engine_net_version, ByteOrder::Little),
        checksums: false,
        limits: Default::default(),
    };
    let body =
        read_actor_bunch_body(&mut pawn_payload(bunches).reader(), open, &codec.actor).unwrap();
    let block = if open {
        &body.blocks[1 + plan.actor_state]
    } else {
        &body.blocks[0]
    };
    let found = read_rep_properties(
        &mut block.payload.reader(),
        &plan.content.character_state_layout,
        &codec,
    )
    .unwrap()
    .entries
    .into_iter()
    .find(|entry| entry.handle == h::HEALTH)
    .map(|entry| entry.value);
    let Some(RepValue::Int(health)) = found else {
        panic!("changed health in CharacterState")
    };
    health
}

fn assert_first_pawn(lab: &PeerLab, bunches: &[Bunch], checkpoint: PlayerCheckpoint) {
    assert_eq!(open_channels(bunches), vec![PAWN_CHANNEL_INDEX]);
    let plan = &lab.pawn_open.as_ref().unwrap().0;
    let codec = RepLayoutCodec {
        actor: actor_codec(plan.engine_net_version, ByteOrder::Little),
        checksums: false,
        limits: Default::default(),
    };
    let body =
        read_actor_bunch_body(&mut pawn_payload(bunches).reader(), true, &codec.actor).unwrap();
    let values: BTreeMap<_, _> = read_rep_properties(
        &mut body.blocks[1 + plan.actor_state].payload.reader(),
        &plan.content.character_state_layout,
        &codec,
    )
    .unwrap()
    .entries
    .into_iter()
    .map(|entry| (entry.handle, entry.value))
    .collect();
    for (handle, expected) in checkpoint.vitals.values() {
        assert_eq!(
            values.get(&handle),
            Some(&expected),
            "vital handle {handle}"
        );
    }
    let spawn = read_spawn(&mut pawn_payload(bunches).reader(), &codec.actor).unwrap();
    assert_eq!(
        spawn.dynamic.unwrap().location,
        Some(
            crate::experimental::session_open::quantize_spawn_vector(
                checkpoint.location_cm.map(|v| v as f32)
            )
            .unwrap()
        )
    );
}

#[test]
fn bootstrap_begin_profiles_precede_reservation_and_pawn() {
    let (mut lab, mut registry) = (lab(), registry());
    world_bootstrap(&mut lab);
    lab.enable_controller_startup(
        &ReleaseProfile::fixture("3.0.32"),
        vec![ControllerStartupAction::RestartPawn],
    )
    .unwrap();
    let (socket, address, mut client, bootstrap) = peer(&mut lab, 1, ACCOUNT, true);
    assert!(bootstrap.iter().any(|b| b.header.channel_index == 2));
    assert!(bootstrap.iter().any(|b| b.header.channel_index == 4));
    assert!(bootstrap.iter().any(|b| b.header.channel_index == 5));
    assert!(
        !bootstrap
            .iter()
            .any(|b| b.header.channel_index == PAWN_CHANNEL_INDEX)
    );
    assert_eq!(
        lab.spawn.peek(None, [0.0; 3]).placement.location,
        [100.0, 200.0, 300.0]
    );
    lab.retry_controller_startup();
    assert_eq!(lab.peers[&address].controller_startup_queued, 0);
    assert_eq!(lab.readiness_counts().controller_startup_actions_pending, 1);
    identify(&mut lab, &socket, &mut client, address, 3, 3);
    assert!(lab.peers[&address].profiles.initialisation_requested);
    assert_eq!(lab.peers[&address].checkpoint_epoch, None);
    lab.bridge_player_checkpoints(&mut registry);
    let epoch = lab.peers[&address].checkpoint_epoch.unwrap();
    let checkpoint = registry.projection(epoch).unwrap().checkpoint;
    lab.retry_controller_startup();
    assert_eq!(lab.peers[&address].controller_startup_queued, 0);
    let opened = deliver_bunches(&mut lab, &socket, &mut client, |_| true);
    assert_first_pawn(&lab, &opened, checkpoint);
    let pawn = read_spawn(
        &mut pawn_payload(&opened).reader(),
        &actor_codec(17, ByteOrder::Little),
    )
    .unwrap()
    .actor
    .guid;
    assert_eq!(lab.peers[&address].controller_startup_pawn_guid, Some(pawn));
    lab.retry_controller_startup();
    assert_eq!(lab.peers[&address].controller_startup_queued, 1);
    let restart = deliver_bunches(&mut lab, &socket, &mut client, |_| true);
    let channel = lab.player_controller_open.as_ref().unwrap().channel_index;
    let codec = RepLayoutCodec {
        actor: actor_codec(17, ByteOrder::Little),
        checksums: false,
        limits: Default::default(),
    };
    let body = read_actor_bunch_body(
        &mut restart
            .iter()
            .find(|b| b.header.channel_index == channel && !b.header.has_package_map_exports)
            .unwrap()
            .payload
            .reader(),
        false,
        &codec.actor,
    )
    .unwrap();
    let field = read_field_header(
        &mut body.blocks[0].payload.reader(),
        lab.controller_startup_bound,
        codec.actor.limits.max_payload_bits,
    )
    .unwrap();
    assert_eq!(field.index, 40);
    assert_eq!(
        super::super::super::rep_rpc::parse_rpc_payload(
            &field.payload,
            &player_controller_rpc_parameters(40).unwrap(),
            &codec
        )
        .unwrap(),
        vec![RpcArgument::Sent(vec![RpcItem::Value(RepValue::Object(
            super::super::super::actor_channel::NetGuidRef::plain(pawn)
        ))])]
    );
    lab.bridge_player_checkpoints(&mut registry);
    assert_eq!(lab.stats.pawn_opens_sent, 1);
    assert!(
        registry.snapshot().is_empty(),
        "reservation grants no committed player"
    );
    finish(&mut lab, &mut registry, &socket, &mut client, address);
    assert_eq!(registry.snapshot().len(), 1);
}

#[test]
fn unaccepted_auth_or_invalid_slots_never_commit_players() {
    for (accepted, begin, slot) in [(false, 3, 3), (true, -2, -2), (true, 3, -1), (true, 3, 4)] {
        let (mut lab, mut registry) = (lab(), registry());
        let (socket, address, mut client, _) = peer(&mut lab, 1, ACCOUNT, accepted);
        identify(&mut lab, &socket, &mut client, address, begin, slot);
        lab.bridge_player_checkpoints(&mut registry);
        assert!(registry.snapshot().is_empty());
        assert!(registry.active_epochs().is_empty());
        assert!(registry.activated_epochs().is_empty());
        assert_eq!(lab.next_checkpoint_epoch, 1);
        assert_eq!(lab.stats.pawn_opens_sent, 0);
        assert_eq!(lab.stats.pawn_spawns_configured, 0);
    }
}

#[test]
fn health_checkpoint_survives_reload_and_reconnect_does_not_rotate_spawn() {
    let (mut lab, mut registry) = (lab(), registry());
    world_bootstrap(&mut lab);
    let (socket, address, mut client, _) = peer(&mut lab, 1, ACCOUNT, true);
    identify(&mut lab, &socket, &mut client, address, 3, 3);
    lab.bridge_player_checkpoints(&mut registry);
    deliver_bunches(&mut lab, &socket, &mut client, |_| true);
    finish(&mut lab, &mut registry, &socket, &mut client, address);
    let key = PlayerKey::new(ACCOUNT, 3).unwrap();
    let before = registry.snapshot().projection(key).unwrap();
    let mut checkpoint = before.checkpoint;
    checkpoint.vitals.health = 37;
    checkpoint.vitals.stamina = 36;
    checkpoint.vitals.oxygen = 35;
    checkpoint.vitals.water = 34;
    checkpoint.vitals.food = 33;
    checkpoint.location_cm = [700.0, 800.0, 900.0];
    registry.edit(key, before.revision, checkpoint).unwrap();
    lab.bridge_player_checkpoints(&mut registry);
    let update = deliver_bunches(&mut lab, &socket, &mut client, |_| true);
    assert_eq!(health(&lab, &update, false), 37);
    assert_eq!(
        lab.peers[&address].placement.unwrap().location,
        [100.0, 200.0, 300.0],
        "checkpoint is not a live teleport"
    );
    lab.remove_peer(address);
    lab.bridge_player_checkpoints(&mut registry);
    let directory = tempfile::tempdir().unwrap();
    let mut store = SnapshotStore::open(directory.path()).unwrap();
    store
        .save(&WorldSnapshot {
            schema_version: SNAPSHOT_SCHEMA_PLAYERS,
            native_players: Some(registry.snapshot()),
            ..Default::default()
        })
        .unwrap();
    let loaded = store
        .load()
        .unwrap()
        .unwrap()
        .snapshot
        .native_players
        .unwrap();
    let mut registry = PlayerRegistry::new(loaded, &context(), 8).unwrap();
    let (socket, address, mut client, bootstrap) = peer(&mut lab, 2, ACCOUNT, true);
    assert!(
        !bootstrap
            .iter()
            .any(|b| b.header.channel_index == PAWN_CHANNEL_INDEX)
    );
    // Bootstrap precedes validated slot lookup: decode both controller locations.
    let plan = lab.player_controller_open.as_ref().unwrap();
    let mut body = BitWriter::new();
    for bunch in bootstrap.iter().filter(|b| {
        b.header.channel_index == plan.channel_index && !b.header.has_package_map_exports
    }) {
        append_bits(&mut body, &bunch.payload);
        if !bunch.header.partial || bunch.header.partial_final {
            break;
        }
    }
    let actor = actor_codec(plan.engine_net_version, ByteOrder::Little);
    let controller =
        read_player_controller_open_bunch_body(&mut Payload::from_writer(body).reader(), &actor)
            .unwrap();
    assert_eq!(
        controller.spawn.dynamic.unwrap().location,
        Some(
            crate::experimental::session_open::quantize_spawn_vector([400.0, 500.0, 600.0])
                .unwrap()
        )
    );
    let properties = read_rep_properties(
        &mut controller.blocks[0].payload.reader(),
        &plan.actor_layout,
        &RepLayoutCodec {
            actor,
            checksums: false,
            limits: Default::default(),
        },
    )
    .unwrap();
    let spawn_location = properties
        .entries
        .iter()
        .find(|entry| entry.handle == 19)
        .unwrap();
    assert_eq!(
        spawn_location.value,
        RepValue::Vector([400.0f32.to_bits(), 500.0f32.to_bits(), 600.0f32.to_bits()])
    );
    identify(&mut lab, &socket, &mut client, address, 3, 3);
    lab.bridge_player_checkpoints(&mut registry);
    let opened = deliver_bunches(&mut lab, &socket, &mut client, |_| true);
    assert_first_pawn(&lab, &opened, checkpoint);
    lab.bridge_player_checkpoints(&mut registry);
    assert_eq!(lab.stats.pawn_opens_sent, 2);
    let spawn = read_spawn(
        &mut pawn_payload(&opened).reader(),
        &actor_codec(17, ByteOrder::Little),
    )
    .unwrap();
    assert_eq!(
        spawn.dynamic.unwrap().location,
        Some(
            crate::experimental::session_open::quantize_spawn_vector([700.0, 800.0, 900.0])
                .unwrap()
        )
    );
    finish(&mut lab, &mut registry, &socket, &mut client, address);
    let (other_socket, other, mut other_client, _) = peer(&mut lab, 3, ACCOUNT + 1, true);
    identify(&mut lab, &other_socket, &mut other_client, other, 3, 3);
    lab.bridge_player_checkpoints(&mut registry);
    assert_eq!(
        lab.peers[&other].placement.unwrap().location,
        [400.0, 500.0, 600.0]
    );
}

#[test]
fn revocation_cancels_pending_projection_and_retains_committed_state() {
    let (mut lab, mut registry) = (lab(), registry());
    let (socket, address, mut client, _) = peer(&mut lab, 1, ACCOUNT, true);
    identify(&mut lab, &socket, &mut client, address, 3, 3);
    lab.bridge_player_checkpoints(&mut registry);
    deliver_bunches(&mut lab, &socket, &mut client, |_| true);
    finish(&mut lab, &mut registry, &socket, &mut client, address);
    let epoch = lab.peers[&address].checkpoint_epoch.unwrap();
    let (auth, backend) = AuthClient::test_bridge(8);
    lab.authenticator = Some(auth);
    backend
        .event(crate::steam_auth::AuthEvent {
            request_id: 1,
            outcome: AuthOutcome::Revoked,
        })
        .unwrap();
    lab.bridge_player_checkpoints(&mut registry);
    assert!(!lab.peers.contains_key(&address));
    assert!(registry.projection(epoch).is_none());
    assert_eq!(registry.snapshot().len(), 1);
    assert!(
        registry
            .reserve(
                PlayerKey::new(ACCOUNT, 3).unwrap(),
                epoch,
                PlayerCheckpoint {
                    location_cm: [0.0; 3],
                    vitals: vitals()
                }
            )
            .is_err()
    );
    let debug = format!("{registry:?} {:?}", registry.snapshot());
    assert!(!debug.contains(&ACCOUNT.to_string()));
    assert!(!format!("{:?}", lab.readiness_counts()).contains("PRIVATE-NAME-SENTINEL"));
}

#[test]
fn queue_pressure_coalesces_latest_vitals_without_advancing_enqueued_revision() {
    let (mut lab, mut registry) = (lab(), registry());
    // Keep the authored pressure fixture below UDP receive-buffer limits.
    // The real bounded ring/queue implementation still handles the refusal.
    lab.config.packet.sent_capacity = 16;
    let (socket, address, mut client, _) = peer(&mut lab, 1, ACCOUNT, true);
    identify(&mut lab, &socket, &mut client, address, 3, 3);
    lab.bridge_player_checkpoints(&mut registry);
    deliver_bunches(&mut lab, &socket, &mut client, |_| true);
    finish(&mut lab, &mut registry, &socket, &mut client, address);
    // Fill the actual packet ring without occupying the pawn's reliable
    // channel. Its default capacity is 256; a fixed 128 controller bunches
    // could not establish this pressure (and the actor window has its own
    // separate bound). ACK-only packets use the existing pressure-fixture seam.
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
    assert_eq!(
        lab.peers[&address]
            .peer
            .sequencer()
            .notify()
            .sent_records()
            .len(),
        capacity
    );
    assert!(lab.peers[&address].peer.full_ring_probe().is_some());
    assert!(
        lab.peers[&address]
            .peer
            .actor_channel_settled(PAWN_CHANNEL_INDEX)
    );
    let key = PlayerKey::new(ACCOUNT, 3).unwrap();
    let old = registry.snapshot().projection(key).unwrap();
    let mut checkpoint = old.checkpoint;
    checkpoint.vitals.health = 55;
    registry.edit(key, old.revision, checkpoint).unwrap();
    lab.bridge_player_checkpoints(&mut registry);
    assert_eq!(
        lab.peers[&address].checkpoint_enqueued_revision,
        Some(old.revision)
    );
    checkpoint.vitals.health = 23;
    let latest = registry.edit(key, old.revision + 1, checkpoint).unwrap();
    lab.bridge_player_checkpoints(&mut registry);
    assert_eq!(
        lab.peers[&address].checkpoint_enqueued_revision,
        Some(old.revision)
    );
    deliver_bunches(&mut lab, &socket, &mut client, |_| true);
    lab.bridge_player_checkpoints(&mut registry);
    assert_eq!(
        lab.peers[&address].checkpoint_enqueued_revision,
        Some(latest.revision)
    );
    let changed = deliver_bunches(&mut lab, &socket, &mut client, |_| true);
    assert_eq!(health(&lab, &changed, false), 23);
}

#[test]
fn out_of_order_profile_readiness_allocates_reservation_epochs_in_ready_order() {
    let (mut lab, mut registry) = (lab(), registry());
    let (first_socket, first, mut first_client, _) = peer(&mut lab, 1, ACCOUNT, true);
    let (second_socket, second, mut second_client, _) = peer(&mut lab, 2, ACCOUNT + 1, true);
    identify(&mut lab, &second_socket, &mut second_client, second, 3, 3);
    lab.bridge_player_checkpoints(&mut registry);
    let second_epoch = lab.peers[&second].checkpoint_epoch.unwrap();
    deliver_bunches(&mut lab, &second_socket, &mut second_client, |_| true);
    identify(&mut lab, &first_socket, &mut first_client, first, 3, 3);
    lab.bridge_player_checkpoints(&mut registry);
    assert!(lab.peers[&first].checkpoint_epoch.unwrap() > second_epoch);
    assert_eq!(registry.active_epochs().len(), 2);
}

#[test]
fn pending_backend_acceptance_and_late_callbacks_cannot_attach_or_revive() {
    let (mut lab, mut registry) = (lab(), registry());
    let (socket, address, mut client, _) = peer(&mut lab, 1, ACCOUNT, false);
    identify(&mut lab, &socket, &mut client, address, 3, 3);
    let (auth, backend) = AuthClient::test_bridge(8);
    lab.authenticator = Some(auth);
    lab.bridge_player_checkpoints(&mut registry);
    assert!(registry.active_epochs().is_empty());
    backend
        .event(crate::steam_auth::AuthEvent {
            request_id: 1,
            outcome: AuthOutcome::Accepted,
        })
        .unwrap();
    lab.bridge_player_checkpoints(&mut registry);
    assert_eq!(registry.active_epochs().len(), 1);
    assert!(registry.snapshot().is_empty());
    backend
        .event(crate::steam_auth::AuthEvent {
            request_id: 1,
            outcome: AuthOutcome::Revoked,
        })
        .unwrap();
    lab.bridge_player_checkpoints(&mut registry);
    assert!(registry.active_epochs().is_empty());
    assert!(registry.snapshot().is_empty());
    backend
        .event(crate::steam_auth::AuthEvent {
            request_id: 1,
            outcome: AuthOutcome::Accepted,
        })
        .unwrap();
    lab.bridge_player_checkpoints(&mut registry);
    assert!(!lab.peers.contains_key(&address));
    assert!(registry.active_epochs().is_empty());
}

#[test]
fn wrong_password_refuses_before_any_checkpoint_reservation() {
    use super::super::super::admission::{INVALID_PASSWORD, JoinPassword};
    let (mut lab, mut registry) = (lab(), registry());
    lab.config.admission.as_mut().unwrap().policy.join_password =
        JoinPassword::new("authored-secret").unwrap();
    let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    socket.connect(lab.local_addr().unwrap()).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(1)))
        .unwrap();
    let (address, _) = handshake_peer_on(&mut lab, &socket);
    let entry = lab.peers.get_mut(&address).unwrap();
    entry.auth_accepted = true;
    entry.auth_request = Some(1);
    entry.auth_identity = Some(ACCOUNT);
    entry.admission.on_challenge_sent();
    let login = ControlMessage::Login {
        response: ArchiveText::text("").unwrap(),
        url: ArchiveText::text("?Name=PRIVATE-NAME-SENTINEL?password=wrong").unwrap(),
        unique_net_id: UniqueNetId {
            kind: NetIdType::Named(ArchiveText::text("NULL").unwrap()),
            value: NetIdValue::Text(ArchiveText::text("UNTRUSTED-LOGIN-SENTINEL").unwrap()),
        },
        platform: ArchiveText::text("").unwrap(),
    };
    assert!(matches!(
        lab.reply_for(address, &login),
        Some(Reply::Failure(INVALID_PASSWORD))
    ));
    lab.answer(address, &[Decoded::Message(login)]);
    lab.bridge_player_checkpoints(&mut registry);
    assert!(!lab.peers.contains_key(&address));
    assert!(registry.snapshot().is_empty());
    assert!(registry.active_epochs().is_empty());
}

#[test]
fn duplicate_live_key_refuses_without_evicting_first_and_slots_are_separate() {
    let (mut lab, mut registry) = (lab(), registry());
    let (socket, first, mut client, _) = peer(&mut lab, 1, ACCOUNT, true);
    identify(&mut lab, &socket, &mut client, first, 3, 3);
    lab.bridge_player_checkpoints(&mut registry);
    deliver_bunches(&mut lab, &socket, &mut client, |_| true);
    let first_epoch = lab.peers[&first].checkpoint_epoch;
    let (duplicate_socket, duplicate, mut duplicate_client, _) = peer(&mut lab, 2, ACCOUNT, true);
    identify(
        &mut lab,
        &duplicate_socket,
        &mut duplicate_client,
        duplicate,
        3,
        3,
    );
    lab.bridge_player_checkpoints(&mut registry);
    assert!(!lab.peers.contains_key(&duplicate));
    assert_eq!(lab.peers[&first].checkpoint_epoch, first_epoch);
    let (other_socket, other, mut other_client, _) = peer(&mut lab, 3, ACCOUNT, true);
    identify(&mut lab, &other_socket, &mut other_client, other, 4, 4);
    lab.bridge_player_checkpoints(&mut registry);
    assert_eq!(registry.active_epochs().len(), 2);
    assert_ne!(
        lab.peers[&other].checkpoint_projection.unwrap().player_id,
        lab.peers[&first].checkpoint_projection.unwrap().player_id
    );
}

#[test]
fn client_profile_xp_and_reported_location_never_mutate_checkpoint() {
    let (mut lab, mut registry) = (lab(), registry());
    let (socket, address, mut client, _) = peer(&mut lab, 1, ACCOUNT, true);
    identify(&mut lab, &socket, &mut client, address, 3, 3);
    lab.bridge_player_checkpoints(&mut registry);
    deliver_bunches(&mut lab, &socket, &mut client, |_| true);
    finish(&mut lab, &mut registry, &socket, &mut client, address);
    let before = registry.snapshot();
    let entry = lab.peers.get_mut(&address).unwrap();
    entry.profiles.character_xp = Some(i32::MAX);
    entry.pawn_moves.location = Some(([999.0; 3], Instant::now()));
    lab.bridge_player_checkpoints(&mut registry);
    assert_eq!(registry.snapshot(), before);
}

#[test]
fn revoked_new_reservation_consumes_one_point_without_refund_or_retry_consumption() {
    let (mut lab, mut registry) = (lab(), registry());
    let (socket, address, mut client, _) = peer(&mut lab, 1, ACCOUNT, true);
    identify(&mut lab, &socket, &mut client, address, 3, 3);
    lab.bridge_player_checkpoints(&mut registry);
    assert_eq!(lab.stats.pawn_spawns_configured, 1);
    assert!(registry.snapshot().is_empty());
    lab.bridge_player_checkpoints(&mut registry);
    assert_eq!(lab.stats.pawn_spawns_configured, 1);
    let (auth, backend) = AuthClient::test_bridge(8);
    lab.authenticator = Some(auth);
    backend
        .event(crate::steam_auth::AuthEvent {
            request_id: 1,
            outcome: AuthOutcome::Revoked,
        })
        .unwrap();
    lab.bridge_player_checkpoints(&mut registry);
    assert!(!lab.peers.contains_key(&address));
    assert!(registry.snapshot().is_empty());
    assert_eq!(
        lab.spawn.peek(None, [0.0; 3]).placement.location,
        [400.0, 500.0, 600.0]
    );
    let (socket, address, mut client, _) = peer(&mut lab, 2, ACCOUNT, true);
    identify(&mut lab, &socket, &mut client, address, 3, 3);
    lab.bridge_player_checkpoints(&mut registry);
    assert_eq!(
        lab.peers[&address].placement.unwrap().location,
        [400.0, 500.0, 600.0]
    );
    assert_eq!(lab.stats.pawn_spawns_configured, 2);
    deliver_bunches(&mut lab, &socket, &mut client, |_| true);
    finish(&mut lab, &mut registry, &socket, &mut client, address);
    lab.bridge_player_checkpoints(&mut registry);
    assert_eq!(lab.stats.pawn_spawns_configured, 2);
    lab.remove_peer(address);
    lab.bridge_player_checkpoints(&mut registry);
    let (socket, address, mut client, _) = peer(&mut lab, 3, ACCOUNT, true);
    identify(&mut lab, &socket, &mut client, address, 3, 3);
    lab.bridge_player_checkpoints(&mut registry);
    assert_eq!(lab.stats.pawn_spawns_configured, 2);
    assert_eq!(
        lab.peers[&address].placement.unwrap().location,
        [400.0, 500.0, 600.0]
    );
    lab.bridge_player_checkpoints(&mut registry);
    assert_eq!(lab.stats.pawn_spawns_configured, 2);
}

#[test]
fn authored_leave_quiesces_across_bridge_retries_and_revocation_retains_obligation() {
    let (mut lab, mut registry) = (lab(), registry());
    let (socket, address, mut client, _) = peer(&mut lab, 1, ACCOUNT, true);
    identify(&mut lab, &socket, &mut client, address, 3, 3);
    lab.bridge_player_checkpoints(&mut registry);
    deliver_bunches(&mut lab, &socket, &mut client, |_| true);
    finish(&mut lab, &mut registry, &socket, &mut client, address);
    let epoch = lab.peers[&address].checkpoint_epoch.unwrap();
    let revision = registry.projection(epoch).unwrap().revision;
    let cutoff = lab
        .request_authored_checkpoint_leave(
            &mut registry,
            CheckpointConnection {
                address,
                connection_id: 1,
                epoch,
            },
            revision,
            Duration::from_secs(60),
        )
        .unwrap();
    let before = registry.snapshot();
    for _ in 0..3 {
        lab.bridge_player_checkpoints(&mut registry);
        assert!(registry.activated_epochs().is_empty());
        assert_eq!(registry.snapshot(), before);
        assert_eq!(
            lab.authored_checkpoint_leave_status(
                &mut registry,
                CheckpointConnection {
                    address,
                    connection_id: 1,
                    epoch
                }
            ),
            Some(cutoff)
        );
    }
    assert_eq!(
        lab.request_authored_checkpoint_leave(
            &mut registry,
            CheckpointConnection {
                address,
                connection_id: 1,
                epoch
            },
            revision,
            Duration::from_secs(60),
        )
        .unwrap(),
        cutoff
    );
    let (auth, backend) = AuthClient::test_bridge(8);
    lab.authenticator = Some(auth);
    backend
        .event(crate::steam_auth::AuthEvent {
            request_id: 1,
            outcome: AuthOutcome::Revoked,
        })
        .unwrap();
    lab.bridge_player_checkpoints(&mut registry);
    assert!(registry.projection(epoch).is_none());
    assert_eq!(
        registry.leave_status(PlayerKey::new(ACCOUNT, 3).unwrap(), epoch),
        Some(cutoff)
    );
    assert_eq!(
        lab.authored_checkpoint_leave_status(
            &mut registry,
            CheckpointConnection {
                address,
                connection_id: 1,
                epoch
            }
        ),
        None
    );
    assert_eq!(registry.snapshot(), before);
    assert!(
        registry
            .reserve(
                PlayerKey::new(ACCOUNT, 3).unwrap(),
                epoch + 1,
                before
                    .projection(PlayerKey::new(ACCOUNT, 3).unwrap())
                    .unwrap()
                    .checkpoint
            )
            .is_err()
    );
}

#[test]
fn authored_leave_address_reuse_cannot_receive_or_create_old_session_completion() {
    let (mut lab, mut registry) = (lab(), registry());
    let (socket, address, mut client, _) = peer(&mut lab, 1, ACCOUNT, true);
    identify(&mut lab, &socket, &mut client, address, 3, 3);
    lab.bridge_player_checkpoints(&mut registry);
    deliver_bunches(&mut lab, &socket, &mut client, |_| true);
    finish(&mut lab, &mut registry, &socket, &mut client, address);
    let epoch = lab.peers[&address].checkpoint_epoch.unwrap();
    let revision = registry.projection(epoch).unwrap().revision;
    assert!(
        lab.request_authored_checkpoint_leave(
            &mut registry,
            CheckpointConnection {
                address,
                connection_id: 2,
                epoch
            },
            revision,
            Duration::from_secs(60)
        )
        .is_err()
    );
    assert_eq!(registry.activated_epochs(), vec![epoch]);
    lab.request_authored_checkpoint_leave(
        &mut registry,
        CheckpointConnection {
            address,
            connection_id: 1,
            epoch,
        },
        revision,
        Duration::from_secs(60),
    )
    .unwrap();
    let (_other_socket, other, _other_client, _) = peer(&mut lab, 2, ACCOUNT + 1, true);
    // Move a genuinely different connection to the original address. No packet
    // or protocol inference: exercise the existing peer table reuse boundary.
    lab.remove_peer(address);
    lab.bridge_player_checkpoints(&mut registry);
    let replacement = lab.peers.remove(&other).unwrap();
    lab.peers.insert(address, replacement);
    assert_eq!(
        lab.authored_checkpoint_leave_status(
            &mut registry,
            CheckpointConnection {
                address,
                connection_id: 1,
                epoch
            }
        ),
        None
    );
    assert_eq!(
        lab.authored_checkpoint_leave_status(
            &mut registry,
            CheckpointConnection {
                address,
                connection_id: 2,
                epoch
            }
        ),
        None
    );
    assert!(
        lab.request_authored_checkpoint_leave(
            &mut registry,
            CheckpointConnection {
                address,
                connection_id: 2,
                epoch
            },
            revision,
            Duration::from_secs(60)
        )
        .is_err()
    );
    assert!(
        registry
            .leave_status(PlayerKey::new(ACCOUNT, 3).unwrap(), epoch)
            .is_some()
    );
}

#[test]
fn persistence_disabled_refuses_authored_leave_control_api() {
    // Prepared only: this refusal requires neither a bound socket nor peers.
    let (mut lab, mut registry) = (
        PeerLab::prepare(PeerLabConfig::default()).unwrap(),
        registry(),
    );
    let address = SocketAddrV4::new(std::net::Ipv4Addr::LOCALHOST, 1);
    let error = lab
        .request_authored_checkpoint_leave(
            &mut registry,
            CheckpointConnection {
                address,
                connection_id: 1,
                epoch: 1,
            },
            1,
            Duration::from_secs(60),
        )
        .unwrap_err();
    assert!(error.to_string().contains("player persistence disabled"));
    assert!(registry.snapshot().is_empty());
    assert!(registry.active_epochs().is_empty());
}

#[test]
fn durable_authored_leave_backpressure_never_repeats_cutoff_and_detach_still_invalidates_delivery()
{
    use crate::{
        config::ServerConfig,
        native_player_runtime::{CheckpointWriter, LeavePhase},
        simulation::build_app,
    };
    let (mut lab, mut registry) = (lab(), registry());
    let (socket, address, mut client, _) = peer(&mut lab, 1, ACCOUNT, true);
    identify(&mut lab, &socket, &mut client, address, 3, 3);
    lab.bridge_player_checkpoints(&mut registry);
    deliver_bunches(&mut lab, &socket, &mut client, |_| true);
    finish(&mut lab, &mut registry, &socket, &mut client, address);
    let epoch = lab.peers[&address].checkpoint_epoch.unwrap();
    let revision = registry.projection(epoch).unwrap().revision;
    lab.request_authored_checkpoint_leave(
        &mut registry,
        CheckpointConnection {
            address,
            connection_id: 1,
            epoch,
        },
        revision,
        Duration::from_secs(60),
    )
    .unwrap();
    let snapshot = WorldSnapshot {
        schema_version: SNAPSHOT_SCHEMA_PLAYERS,
        native_players: Some(registry.snapshot()),
        ..Default::default()
    };
    let mut app = build_app(&ServerConfig::default(), &snapshot).unwrap();
    app.insert_resource(registry);
    let directory = tempfile::tempdir().unwrap();
    let mut writer =
        CheckpointWriter::spawn(SnapshotStore::open(directory.path()).unwrap()).unwrap();
    writer.submit(app.world_mut()).unwrap();
    let receipt = writer
        .finish_before(app.world_mut(), Instant::now() + Duration::from_secs(5))
        .unwrap()
        .unwrap();
    let mut registry = app.world_mut().remove_resource::<PlayerRegistry>().unwrap();
    let expected = registry
        .leave_status(PlayerKey::new(ACCOUNT, 3).unwrap(), epoch)
        .unwrap();
    assert_eq!(
        expected.phase,
        LeavePhase::Durable {
            generation: receipt.generation,
            tick: receipt.tick
        }
    );
    for _ in 0..8 {
        assert_eq!(
            lab.authored_checkpoint_leave_status(
                &mut registry,
                CheckpointConnection {
                    address,
                    connection_id: 1,
                    epoch
                }
            ),
            Some(expected)
        );
        lab.bridge_player_checkpoints(&mut registry);
        assert!(!registry.checkpoint_save_needed(Instant::now()));
        assert!(registry.activated_epochs().is_empty());
        assert_eq!(
            registry.snapshot(),
            snapshot.native_players.clone().unwrap()
        );
    }
    assert!(
        registry
            .settle_leave(PlayerKey::new(ACCOUNT, 3).unwrap(), epoch)
            .is_err()
    );
    lab.remove_peer(address);
    assert_eq!(
        lab.authored_checkpoint_leave_status(
            &mut registry,
            CheckpointConnection {
                address,
                connection_id: 1,
                epoch
            }
        ),
        None
    );
    assert_eq!(
        registry.leave_status(PlayerKey::new(ACCOUNT, 3).unwrap(), epoch),
        Some(expected)
    );
    registry
        .settle_leave(PlayerKey::new(ACCOUNT, 3).unwrap(), epoch)
        .unwrap();
}

// Early checkpoint pawn (`network.checkpoint_pawn_before_identity`). Authored
// loopback peers only: the order a stock client needs is inferred from
// recorded runs (F-1036, F-1181, F-1185, F-1199), not shown here.

fn spawn_points() -> SpawnConfig {
    SpawnConfig {
        points: vec![
            SpawnPoint {
                x: 100.0,
                y: 200.0,
                z: 300.0,
                yaw: None,
            },
            SpawnPoint {
                x: 400.0,
                y: 500.0,
                z: 600.0,
                yaw: None,
            },
        ],
    }
}

fn early_lab() -> PeerLab {
    let mut lab = lab();
    lab.enable_checkpoint_early_pawn().unwrap();
    lab
}

/// As `run` composes them, the pawn's fresh character state and the
/// checkpoint's initial vitals come from the same authored rows
/// (`network::pawn_content`, `network::fresh_player_vitals`).
fn production_content(lab: &mut PeerLab) {
    lab.pawn_open.as_mut().unwrap().2.character_state = vitals().values();
}

/// Checkpoints with connected-player records, possession, the frozen
/// release, ClientRestart startup and the production pawn content.
fn stock_lab(early: bool) -> PeerLab {
    let mut lab = record_lab(true);
    lab.enable_spawn_points(&spawn_points()).unwrap();
    lab.enable_player_checkpoints(vitals()).unwrap();
    production_content(&mut lab);
    lab.enable_controller_possession_properties().unwrap();
    lab.enable_frozen_movement_release(Duration::ZERO).unwrap();
    lab.enable_controller_startup(
        &ReleaseProfile::fixture("3.0.32"),
        vec![ControllerStartupAction::RestartPawn],
    )
    .unwrap();
    if early {
        lab.enable_checkpoint_early_pawn().unwrap();
    }
    lab
}

fn little() -> RepLayoutCodec {
    RepLayoutCodec {
        actor: actor_codec(17, ByteOrder::Little),
        checksums: false,
        limits: Default::default(),
    }
}

/// The arguments of every controller ClientRestart (field 40) among `bunches`.
fn restart_calls(lab: &PeerLab, bunches: &[Bunch]) -> Vec<Vec<RpcArgument>> {
    use super::super::super::{actor_channel::BlockTarget, rep_rpc::parse_rpc_payload};
    let channel = lab.player_controller_open.as_ref().unwrap().channel_index;
    let codec = little();
    let parameters = player_controller_rpc_parameters(40).unwrap();
    bunches
        .iter()
        .filter(|b| {
            b.header.channel_index == channel
                && b.header.reliable
                && !b.header.open
                && !b.header.partial
                && !b.header.has_package_map_exports
        })
        .filter_map(|b| read_actor_bunch_body(&mut b.payload.reader(), false, &codec.actor).ok())
        .flat_map(|body| body.blocks)
        .filter(|block| !block.has_rep_layout && block.target == BlockTarget::Actor)
        .filter_map(|block| {
            let field = read_field_header(
                &mut block.payload.reader(),
                lab.controller_startup_bound,
                codec.actor.limits.max_payload_bits,
            )
            .ok()?;
            (field.index == 40)
                .then(|| parse_rpc_payload(&field.payload, &parameters, &codec).unwrap())
        })
        .collect()
}

fn restart_of(pawn: u32) -> Vec<RpcArgument> {
    vec![RpcArgument::Sent(vec![RpcItem::Value(RepValue::Object(
        super::super::super::actor_channel::NetGuidRef::plain(pawn),
    ))])]
}

/// Reliable pawn property updates: whole bunches that are not opens, open
/// continuations (partial parts) or export parts.
fn pawn_updates(bunches: &[Bunch]) -> Vec<&Bunch> {
    bunches
        .iter()
        .filter(|b| {
            b.header.channel_index == PAWN_CHANNEL_INDEX
                && b.header.reliable
                && !b.header.open
                && !b.header.partial
                && !b.header.has_package_map_exports
        })
        .collect()
}

/// CharacterState values of one non-open pawn update in `order`.
fn update_values(lab: &PeerLab, payload: &Payload, order: ByteOrder) -> BTreeMap<u32, RepValue> {
    let plan = &lab.pawn_open.as_ref().unwrap().0;
    let codec = RepLayoutCodec {
        actor: actor_codec(plan.engine_net_version, order),
        checksums: false,
        limits: Default::default(),
    };
    let body = read_actor_bunch_body(&mut payload.reader(), false, &codec.actor).unwrap();
    let [block] = body.blocks.as_slice() else {
        panic!("one CharacterState block");
    };
    read_rep_properties(
        &mut block.payload.reader(),
        &plan.content.character_state_layout,
        &codec,
    )
    .unwrap()
    .entries
    .into_iter()
    .map(|entry| (entry.handle, entry.value))
    .collect()
}

fn with_alive(vitals: CharacterVitals) -> BTreeMap<u32, RepValue> {
    let mut values = vitals.values();
    values.insert(
        h::CURRENT_ALIVE_STATE,
        RepValue::Enum(u32::from(vitals.health < 1)),
    );
    values
}

/// What a stock-order synthetic join produced.
struct StockJoin {
    /// Kept open for the session's lifetime.
    _socket: UdpSocket,
    address: SocketAddrV4,
    _client: PacketSequencer,
    log: Vec<String>,
    received: Vec<Bunch>,
    pawn: Option<u32>,
}

/// A client that sends nothing until a controller ClientRestart (field 40)
/// names the pawn it was opened; then Begin, then each profile answer, then
/// the initialisation answers and friends list once they are requested. Each
/// round is one app update (`poll`, then the checkpoint bridge) followed by
/// the client's reply. Server transitions are logged after the update, wire
/// arrivals after delivery, client calls as sent.
fn stock_join(
    lab: &mut PeerLab,
    registry: &mut PlayerRegistry,
    connection: u32,
    rounds: usize,
) -> StockJoin {
    let (socket, address, mut client) = joined_peer(lab, connection, ACCOUNT, true);
    assert!(lab.send_player_controller_open(address));
    let records = lab.stats.connected_player_record_creations_sent;
    let releases = lab.stats.frozen_movement_releases_sent;
    let mut log = Vec::new();
    let mut received = Vec::new();
    let mut pawn = None;
    let mut stage = 0;
    let mut state = [false; 5];
    for _ in 0..rounds {
        lab.poll();
        lab.bridge_player_checkpoints(registry);
        let Some(entry) = lab.peers.get(&address) else {
            log.push("closed".to_owned());
            break;
        };
        let now = [
            entry.checkpoint_epoch.is_some(),
            entry.checkpoint_active,
            entry.profiles.finalised,
            lab.stats.connected_player_record_creations_sent > records,
            lab.stats.frozen_movement_releases_sent > releases,
        ];
        for (index, name) in [
            "reservation",
            "activation",
            "finalisation",
            "record",
            "frozen release",
        ]
        .into_iter()
        .enumerate()
        {
            if now[index] && !state[index] {
                state[index] = true;
                log.push(name.to_owned());
            }
        }
        let bunches = deliver_bunches(lab, &socket, &mut client, |_| true);
        for channel in open_channels(&bunches) {
            log.push(format!("open {channel}"));
            if channel == PAWN_CHANNEL_INDEX {
                let pawn_open: Vec<_> = bunches
                    .iter()
                    .filter(|b| b.header.channel_index == PAWN_CHANNEL_INDEX)
                    .cloned()
                    .collect();
                pawn = Some(
                    read_spawn(&mut pawn_payload(&pawn_open).reader(), &little().actor)
                        .unwrap()
                        .actor
                        .guid,
                );
            }
        }
        for arguments in restart_calls(lab, &bunches) {
            assert_eq!(
                Some(arguments),
                pawn.map(restart_of),
                "restart names the pawn"
            );
            log.push("restart".to_owned());
        }
        for _ in pawn_updates(&bunches) {
            log.push("pawn update".to_owned());
        }
        received.extend(bunches);
        match stage {
            0 if log.iter().any(|event| event == "restart") => {
                lab.controller_call(address, ControllerCall::Begin(3));
                log.push("begin".to_owned());
                stage = 1;
            }
            1 => {
                lab.controller_call(
                    address,
                    ControllerCall::Profile(response(ProfileKind::User, 3)),
                );
                received.extend(deliver_bunches(lab, &socket, &mut client, |_| true));
                lab.controller_call(
                    address,
                    ControllerCall::Profile(response(ProfileKind::Character, 3)),
                );
                log.push("profiles".to_owned());
                stage = 2;
            }
            2 if lab.peers[&address].profiles.initialisation_requested => {
                lab.controller_call(
                    address,
                    ControllerCall::Initialisation(InitialisationAnswer::Loadout),
                );
                lab.controller_call(
                    address,
                    ControllerCall::Initialisation(InitialisationAnswer::Bestiary),
                );
                lab.controller_call(address, ControllerCall::FriendsList);
                log.push("initialisation".to_owned());
                stage = 3;
            }
            _ => {}
        }
        if state[4] {
            break;
        }
    }
    StockJoin {
        _socket: socket,
        address,
        _client: client,
        log,
        received,
        pawn,
    }
}

#[test]
fn stock_order_client_waits_forever_without_the_early_pawn_and_enters_with_it() {
    for early in [false, true] {
        let (mut lab, mut registry) = (stock_lab(early), registry());
        let extras = world_bootstrap(&mut lab);
        let join = stock_join(&mut lab, &mut registry, 1, 24);
        let address = join.address;
        let mut opens = vec!["open 2".to_owned()];
        if early {
            opens.push("open 3".to_owned());
        }
        opens.extend(["open 4".to_owned(), "open 5".to_owned()]);
        opens.extend(extras.iter().map(|channel| format!("open {channel}")));
        if !early {
            // The inferred circular wait: the pawn waits for Begin, ClientRestart
            // for the pawn, and this client's Begin for ClientRestart.
            assert_eq!(join.log, opens);
            let entry = &lab.peers[&address];
            assert!(!entry.peer.has_actor_channel(PAWN_CHANNEL_INDEX));
            assert_eq!(entry.controller_startup_queued, 0);
            assert_eq!(entry.controller_startup_pawn_guid, None);
            assert_eq!(entry.checkpoint_epoch, None);
            assert!(restart_calls(&lab, &join.received).is_empty());
            assert!(registry.active_epochs().is_empty());
            assert!(registry.snapshot().is_empty());
            assert_eq!(lab.next_checkpoint_epoch, 1);
            assert_eq!(lab.stats.pawn_opens_sent, 0);
            assert_eq!(lab.stats.initialisation_slots_recorded, 0);
            let readiness = lab.readiness_counts();
            assert_eq!(readiness.initial_open_outcomes.pending.pawn, 1);
            assert_eq!(readiness.controller_startup_actions_pending, 1);
            continue;
        }
        let mut expected = opens;
        expected.extend(
            [
                "restart",
                "begin",
                "profiles",
                "reservation",
                "initialisation",
                "activation",
                "finalisation",
                "record",
                "frozen release",
            ]
            .map(str::to_owned),
        );
        assert_eq!(join.log, expected);
        assert_eq!(
            lab.peers[&address].controller_startup_pawn_guid, join.pawn,
            "ClientRestart named the opened pawn"
        );
        // A fresh player's checkpoint equals the open: no vitals update.
        assert!(pawn_updates(&join.received).is_empty());
        let key = PlayerKey::new(ACCOUNT, 3).unwrap();
        let stored = registry.snapshot().projection(key).unwrap().checkpoint;
        assert_eq!(stored.vitals, vitals());
        assert_eq!(stored.location_cm, [100.0, 200.0, 300.0]);
        assert_eq!(lab.stats.initialisation_slots_recorded, 1);
        assert_eq!(lab.stats.pawn_spawns_configured, 1);
        assert_eq!(lab.session_counts().checkpoint_locations_unapplied, Some(0));
    }
}

#[test]
fn early_fresh_pawn_open_is_the_legacy_open_and_needs_no_vitals_update() {
    let open = |checkpoints: bool| {
        let mut lab = record_lab(false);
        lab.enable_spawn_points(&spawn_points()).unwrap();
        production_content(&mut lab);
        lab.enable_actor_uids(crate::actor_uids::ActorUidLibrary::new(1))
            .unwrap();
        lab.seed_actor_uids([crate::actor_uids::FIRST_NATIVE_OBJECT_ID])
            .unwrap();
        if checkpoints {
            lab.enable_player_checkpoints(vitals()).unwrap();
            lab.enable_checkpoint_early_pawn().unwrap();
        }
        let (socket, address, client, bunches) = peer(&mut lab, 1, ACCOUNT, true);
        (lab, socket, address, client, bunches)
    };
    // Reliable sequence numbers start from each connection's own handshake
    // seed; compare every other header field, the sequence offsets and payloads.
    let pawn_bunches = |bunches: &[Bunch]| -> Vec<Bunch> {
        bunches
            .iter()
            .filter(|b| b.header.channel_index == PAWN_CHANNEL_INDEX)
            .cloned()
            .collect()
    };
    let relative = |bunches: &[Bunch]| -> Vec<Bunch> {
        let first = bunches[0].header.reliable_sequence_raw.unwrap();
        bunches
            .iter()
            .cloned()
            .map(|mut bunch| {
                bunch.header.reliable_sequence_raw = bunch
                    .header
                    .reliable_sequence_raw
                    .map(|raw| raw.wrapping_sub(first));
                bunch
            })
            .collect()
    };
    let (legacy_lab, _legacy_socket, _, _, legacy) = open(false);
    let (mut lab, socket, address, mut client, early) = open(true);
    let legacy_pawn = pawn_bunches(&legacy);
    assert!(legacy_pawn.first().is_some_and(|b| b.header.open));
    assert_eq!(
        relative(&pawn_bunches(&early)),
        relative(&legacy_pawn),
        "same open, byte for byte"
    );
    assert_eq!(open_channels(&early), open_channels(&legacy));
    assert_eq!(
        lab.claimed_actor_uids(),
        legacy_lab.claimed_actor_uids(),
        "same UID claim"
    );
    assert_eq!(lab.claimed_actor_uids().len(), 2);
    let values = {
        let plan = &lab.pawn_open.as_ref().unwrap().0;
        let body = read_actor_bunch_body(
            &mut pawn_payload(&legacy_pawn).reader(),
            true,
            &little().actor,
        )
        .unwrap();
        read_rep_properties(
            &mut body.blocks[1 + plan.actor_state].payload.reader(),
            &plan.content.character_state_layout,
            &little(),
        )
        .unwrap()
        .entries
    };
    assert!(
        values.iter().all(|e| e.handle != h::CURRENT_ALIVE_STATE),
        "legacy open carries no alive state"
    );
    // Before identity: the point is committed, no checkpoint exists.
    let entry = &lab.peers[&address];
    assert_eq!(entry.checkpoint_epoch, None);
    assert_eq!(
        entry.placement.unwrap().location,
        [100.0, 200.0, 300.0],
        "committed at the controller open"
    );
    assert_eq!(lab.stats.pawn_spawns_configured, 1);
    assert_eq!(lab.stats.pawn_opens_sent, 1);
    assert_no_reservation_after_bridges(&mut lab);
    let mut registry = registry();
    identify(&mut lab, &socket, &mut client, address, 3, 3);
    lab.bridge_player_checkpoints(&mut registry);
    let epoch = lab.peers[&address].checkpoint_epoch.unwrap();
    let projection = registry.projection(epoch).unwrap();
    assert!(projection.is_new);
    assert_eq!(projection.checkpoint.location_cm, [100.0, 200.0, 300.0]);
    assert_eq!(projection.checkpoint.vitals, vitals());
    assert_eq!(
        lab.peers[&address].checkpoint_enqueued_revision,
        Some(projection.revision),
        "equal to the open: published without an update"
    );
    let after = deliver_bunches(&mut lab, &socket, &mut client, |_| true);
    finish(&mut lab, &mut registry, &socket, &mut client, address);
    let later = deliver_bunches(&mut lab, &socket, &mut client, |_| true);
    assert!(pawn_updates(&after).is_empty() && pawn_updates(&later).is_empty());
    assert_eq!(lab.stats.pawn_spawns_configured, 1, "no second point");
    assert_eq!(registry.snapshot().len(), 1);
    assert_eq!(lab.session_counts().checkpoint_locations_unapplied, Some(0));
    assert_eq!(
        legacy_lab.session_counts().checkpoint_locations_unapplied,
        None
    );
}

/// No reservation can exist before identity, however often the bridge runs.
fn assert_no_reservation_after_bridges(lab: &mut PeerLab) {
    let mut registry = registry();
    for _ in 0..3 {
        lab.bridge_player_checkpoints(&mut registry);
        lab.retry_initial_opens();
    }
    assert_eq!(lab.next_checkpoint_epoch, 1);
    assert!(registry.active_epochs().is_empty());
}

#[test]
fn early_returning_player_gets_one_vitals_update_before_its_record_and_keeps_its_location() {
    let (mut lab, mut registry) = (stock_lab(true), registry());
    let first = stock_join(&mut lab, &mut registry, 1, 24);
    assert_eq!(first.log.last().map(String::as_str), Some("frozen release"));
    let key = PlayerKey::new(ACCOUNT, 3).unwrap();
    let before = registry.snapshot().projection(key).unwrap();
    let mut checkpoint = before.checkpoint;
    checkpoint.vitals.health = 37;
    checkpoint.vitals.oxygen = 69;
    checkpoint.location_cm = [700.0, 800.0, 900.0];
    registry.edit(key, before.revision, checkpoint).unwrap();
    lab.remove_peer(first.address);
    lab.bridge_player_checkpoints(&mut registry);
    assert!(registry.active_epochs().is_empty());
    let join = stock_join(&mut lab, &mut registry, 2, 24);
    let address = join.address;
    // The open: fresh vitals at the second point, as without checkpoints.
    let pawn_open: Vec<_> = join
        .received
        .iter()
        .filter(|b| b.header.channel_index == PAWN_CHANNEL_INDEX)
        .cloned()
        .collect();
    assert_first_pawn(
        &lab,
        &pawn_open,
        PlayerCheckpoint {
            location_cm: [400.0, 500.0, 600.0],
            vitals: vitals(),
        },
    );
    let at = |event: &str| join.log.iter().position(|e| e == event).unwrap();
    assert_eq!(
        join.log.iter().filter(|e| *e == "pawn update").count(),
        1,
        "exactly one reliable vitals update"
    );
    assert!(at("reservation") < at("pawn update"));
    assert!(at("pawn update") < at("finalisation"));
    assert!(at("finalisation") < at("record"));
    assert!(at("record") < at("frozen release"));
    let projection = registry
        .projection(lab.peers[&address].checkpoint_epoch.unwrap())
        .unwrap();
    assert!(!projection.is_new);
    assert_eq!(projection.checkpoint, checkpoint);
    let [update] = pawn_updates(&join.received).try_into().unwrap();
    assert_eq!(
        update_values(&lab, &update.payload, ByteOrder::Little),
        with_alive(checkpoint.vitals)
    );
    assert_eq!(
        lab.peers[&address].checkpoint_enqueued_revision,
        Some(projection.revision)
    );
    // Neither the live pawn nor the stored location moved; one count.
    assert_eq!(
        lab.peers[&address].placement.unwrap().location,
        [400.0, 500.0, 600.0]
    );
    assert_eq!(
        registry
            .snapshot()
            .projection(key)
            .unwrap()
            .checkpoint
            .location_cm,
        [700.0, 800.0, 900.0]
    );
    assert_eq!(lab.stats.checkpoint_locations_unapplied, 1);
    assert_eq!(lab.session_counts().checkpoint_locations_unapplied, Some(1));
    assert_eq!(lab.stats.pawn_spawns_configured, 2);
    assert_eq!(lab.stats.pawn_spawns_saved, 0);
    let text = serde_json::to_string(&lab.session_counts()).unwrap();
    assert!(text.contains("\"checkpoint_locations_unapplied\":1"));
}

#[test]
fn early_vitals_update_round_trips_in_both_byte_orders_and_marks_death() {
    let (mut lab, mut registry) = (early_lab(), registry());
    let (socket, address, mut client, _) = peer(&mut lab, 1, ACCOUNT, true);
    identify(&mut lab, &socket, &mut client, address, 3, 3);
    lab.bridge_player_checkpoints(&mut registry);
    // The test pawn content carries no vitals, so the projection is sent.
    let first = deliver_bunches(&mut lab, &socket, &mut client, |_| true);
    let [update] = pawn_updates(&first).try_into().unwrap();
    assert_eq!(
        update_values(&lab, &update.payload, ByteOrder::Little),
        with_alive(vitals())
    );
    finish(&mut lab, &mut registry, &socket, &mut client, address);
    let key = PlayerKey::new(ACCOUNT, 3).unwrap();
    let before = registry.snapshot().projection(key).unwrap();
    let mut dead = before.checkpoint;
    dead.vitals.health = 0;
    registry.edit(key, before.revision, dead).unwrap();
    lab.bridge_player_checkpoints(&mut registry);
    let died = deliver_bunches(&mut lab, &socket, &mut client, |_| true);
    let [update] = pawn_updates(&died).try_into().unwrap();
    let values = update_values(&lab, &update.payload, ByteOrder::Little);
    assert_eq!(values[&h::CURRENT_ALIVE_STATE], RepValue::Enum(1));
    assert_eq!(values, with_alive(dead.vitals));
    // The same composer re-read in both byte orders.
    let plan = &lab.pawn_open.as_ref().unwrap().0;
    let connection = lab.peers[&address].connection_id.unwrap();
    for order in [ByteOrder::Little, ByteOrder::Big] {
        for health in [37, 0] {
            let mut vitals = dead.vitals;
            vitals.health = health;
            let mut cache = lab.guid_cache.clone();
            let mut exports = lab.peers[&address].exports.clone();
            let update = compose_live_pawn_vitals_update(
                &mut cache,
                &mut exports,
                plan,
                pawn_key(connection),
                None,
                &[],
                Some(&vitals),
                order,
            )
            .unwrap();
            let values = update_values(&lab, &update.payload, order);
            assert_eq!(values, with_alive(vitals), "{order:?} health {health}");
            assert_eq!(
                values[&h::CURRENT_ALIVE_STATE],
                RepValue::Enum(u32::from(health == 0))
            );
        }
    }
}

fn fill_ring(lab: &mut PeerLab, address: SocketAddrV4) {
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

#[test]
fn early_vitals_coalesce_under_pressure_and_loss_and_debit_only_after_activation() {
    use crate::native_player_runtime::ConsumptionRates;
    let (mut lab, mut registry) = (early_lab(), registry());
    lab.config.packet.sent_capacity = 16;
    let (socket, address, mut client, _) = peer(&mut lab, 1, ACCOUNT, true);
    identify(&mut lab, &socket, &mut client, address, 3, 3);
    lab.bridge_player_checkpoints(&mut registry);
    deliver_bunches(&mut lab, &socket, &mut client, |_| true);
    finish(&mut lab, &mut registry, &socket, &mut client, address);
    let key = PlayerKey::new(ACCOUNT, 3).unwrap();
    let stored = registry.snapshot().projection(key).unwrap();
    let mut checkpoint = stored.checkpoint;
    checkpoint.vitals.health = 37;
    registry.edit(key, stored.revision, checkpoint).unwrap();
    lab.remove_peer(address);
    lab.bridge_player_checkpoints(&mut registry);
    // Returning: its pawn opens fresh, then pressure meets the first publication.
    let (socket, address, mut client, _) = peer(&mut lab, 2, ACCOUNT, true);
    identify(&mut lab, &socket, &mut client, address, 3, 3);
    fill_ring(&mut lab, address);
    let rates = ConsumptionRates::new(1, 1, 1).unwrap();
    for _ in 0..3 {
        lab.bridge_player_checkpoints(&mut registry);
        assert!(lab.peers[&address].checkpoint_epoch.is_some());
        assert_eq!(lab.peers[&address].checkpoint_enqueued_revision, None);
        assert!(!lab.peers[&address].checkpoint_active);
    }
    let reserved = registry.snapshot();
    assert_eq!(registry.consume_activated(rates).unwrap(), 0);
    assert_eq!(registry.snapshot(), reserved, "no debit before activation");
    let current = registry.snapshot().projection(key).unwrap();
    checkpoint.vitals.health = 23;
    let latest = registry.edit(key, current.revision, checkpoint).unwrap();
    lab.bridge_player_checkpoints(&mut registry);
    assert_eq!(lab.peers[&address].checkpoint_enqueued_revision, None);
    // Capacity drains: only the latest values are published, once.
    deliver_bunches(&mut lab, &socket, &mut client, |_| true);
    lab.bridge_player_checkpoints(&mut registry);
    assert_eq!(
        lab.peers[&address].checkpoint_enqueued_revision,
        Some(latest.revision)
    );
    let changed = deliver_bunches(&mut lab, &socket, &mut client, |_| true);
    let [update] = pawn_updates(&changed).try_into().unwrap();
    assert_eq!(
        update_values(&lab, &update.payload, ByteOrder::Little),
        with_alive(checkpoint.vitals)
    );
    finish(&mut lab, &mut registry, &socket, &mut client, address);
    // After activation, one debit; loss repair resends it without another.
    assert_eq!(registry.consume_activated(rates).unwrap(), 1);
    let charged = registry.snapshot();
    let debited = charged.projection(key).unwrap();
    assert_eq!(
        debited.checkpoint.vitals.oxygen,
        checkpoint.vitals.oxygen - 1
    );
    lab.bridge_player_checkpoints(&mut registry);
    assert_eq!(
        lab.peers[&address].checkpoint_enqueued_revision,
        Some(debited.revision)
    );
    let lost = deliver_bunches(&mut lab, &socket, &mut client, |_| false);
    let [lost_update] = pawn_updates(&lost).try_into().unwrap();
    for _ in 0..3 {
        lab.bridge_player_checkpoints(&mut registry);
        assert_eq!(registry.snapshot(), charged);
        assert_eq!(
            lab.peers[&address].checkpoint_enqueued_revision,
            Some(debited.revision)
        );
    }
    let datagram = lab
        .peers
        .get_mut(&address)
        .unwrap()
        .peer
        .ack_packet()
        .unwrap();
    assert!(lab.send(&datagram, address));
    let repaired = deliver_bunches(&mut lab, &socket, &mut client, |_| true);
    let [repair] = pawn_updates(&repaired).try_into().unwrap();
    assert_eq!(repair.payload, lost_update.payload);
    assert_eq!(registry.snapshot(), charged, "never a second debit");
}

#[test]
fn early_slot_disagreement_after_the_pawn_closes_and_frees_its_uid() {
    for (begin, slot) in [(3, 4), (3, -1), (-2, -2)] {
        let (mut lab, mut registry) = (early_lab(), registry());
        lab.enable_actor_uids(crate::actor_uids::ActorUidLibrary::new(1))
            .unwrap();
        let (socket, address, mut client, _) = peer(&mut lab, 1, ACCOUNT, true);
        assert!(
            lab.peers[&address]
                .peer
                .has_actor_channel(PAWN_CHANNEL_INDEX)
        );
        assert_eq!(lab.claimed_actor_uids().len(), 1);
        identify(&mut lab, &socket, &mut client, address, begin, slot);
        lab.bridge_player_checkpoints(&mut registry);
        assert!(!lab.peers.contains_key(&address));
        assert!(lab.claimed_actor_uids().is_empty(), "UID freed on removal");
        assert!(registry.active_epochs().is_empty());
        assert!(registry.snapshot().is_empty());
        assert_eq!(lab.next_checkpoint_epoch, 1);
    }
}

#[test]
fn early_duplicate_live_key_refuses_without_evicting_the_first() {
    let (mut lab, mut registry) = (early_lab(), registry());
    let (socket, first, mut client, _) = peer(&mut lab, 1, ACCOUNT, true);
    identify(&mut lab, &socket, &mut client, first, 3, 3);
    lab.bridge_player_checkpoints(&mut registry);
    deliver_bunches(&mut lab, &socket, &mut client, |_| true);
    let first_epoch = lab.peers[&first].checkpoint_epoch;
    let (duplicate_socket, duplicate, mut duplicate_client, _) = peer(&mut lab, 2, ACCOUNT, true);
    assert!(
        lab.peers[&duplicate]
            .peer
            .has_actor_channel(PAWN_CHANNEL_INDEX)
    );
    identify(
        &mut lab,
        &duplicate_socket,
        &mut duplicate_client,
        duplicate,
        3,
        3,
    );
    lab.bridge_player_checkpoints(&mut registry);
    assert!(!lab.peers.contains_key(&duplicate));
    assert_eq!(lab.peers[&first].checkpoint_epoch, first_epoch);
    assert!(lab.peers[&first].peer.has_actor_channel(PAWN_CHANNEL_INDEX));
    assert_eq!(registry.active_epochs(), vec![first_epoch.unwrap()]);
}

#[test]
fn early_revocation_between_pawn_and_begin_removes_the_peer_without_a_record() {
    let (mut lab, mut registry) = (stock_lab(true), registry());
    let (socket, address, mut client) = joined_peer(&mut lab, 1, ACCOUNT, true);
    assert!(lab.send_player_controller_open(address));
    deliver_bunches(&mut lab, &socket, &mut client, |_| true);
    assert!(
        lab.peers[&address]
            .peer
            .has_actor_channel(PAWN_CHANNEL_INDEX)
    );
    let (auth, backend) = AuthClient::test_bridge(8);
    lab.authenticator = Some(auth);
    backend
        .event(crate::steam_auth::AuthEvent {
            request_id: 1,
            outcome: AuthOutcome::Revoked,
        })
        .unwrap();
    lab.bridge_player_checkpoints(&mut registry);
    assert!(!lab.peers.contains_key(&address));
    // A late Begin from the removed address reaches no session.
    lab.controller_call(address, ControllerCall::Begin(3));
    lab.bridge_player_checkpoints(&mut registry);
    assert!(registry.active_epochs().is_empty());
    assert!(registry.snapshot().is_empty());
    assert_eq!(lab.next_checkpoint_epoch, 1);
    assert_eq!(lab.stats.connected_player_records_created, 0);
    assert_eq!(lab.stats.connected_player_record_creations_sent, 0);
}

#[test]
fn early_client_without_begin_times_out_with_zero_reservations() {
    let (mut lab, mut registry) = (early_lab(), registry());
    lab.enable_actor_uids(crate::actor_uids::ActorUidLibrary::new(1))
        .unwrap();
    let (_socket, address, _client, _) = peer(&mut lab, 1, ACCOUNT, true);
    assert_eq!(lab.claimed_actor_uids().len(), 1);
    for _ in 0..3 {
        lab.bridge_player_checkpoints(&mut registry);
        assert!(registry.active_epochs().is_empty());
    }
    let timeout = lab.config.idle_timeout;
    let entry = lab.peers.get_mut(&address).unwrap();
    entry.last_seen = Instant::now() - timeout - Duration::from_secs(1);
    lab.poll();
    assert!(!lab.peers.contains_key(&address));
    assert_eq!(lab.stats.peers_expired, 1);
    lab.bridge_player_checkpoints(&mut registry);
    assert!(registry.active_epochs().is_empty());
    assert!(registry.snapshot().is_empty());
    assert_eq!(lab.next_checkpoint_epoch, 1);
    assert!(lab.claimed_actor_uids().is_empty());
}

#[test]
fn early_pawn_requires_checkpoints_before_peers() {
    let mut plain = record_lab(false);
    assert!(plain.enable_checkpoint_early_pawn().is_err());
    assert!(!plain.has_checkpoint_early_pawn());
    let mut lab = lab();
    let (_socket, _address, _client, _) = peer(&mut lab, 1, ACCOUNT, true);
    assert!(lab.enable_checkpoint_early_pawn().is_err());
    assert!(!lab.has_checkpoint_early_pawn());
    assert_eq!(lab.session_counts().checkpoint_locations_unapplied, None);
    assert!(
        !serde_json::to_string(&lab.session_counts())
            .unwrap()
            .contains("checkpoint_locations_unapplied")
    );
}

// Saved locations and live relocation (`network.checkpoint_saved_locations`,
// `network.checkpoint_location_correction`). Authored loopback peers and
// moves only; no client behaviour is shown.

/// Pawn-channel bunches as digest lines: header flags, length and payload.
fn pawn_channel_lines(bunches: &[Bunch]) -> Vec<String> {
    bunches
        .iter()
        .filter(|bunch| bunch.header.channel_index == PAWN_CHANNEL_INDEX)
        .map(|bunch| {
            let header = &bunch.header;
            format!(
                "{} {} {} {} {} {:02x?}",
                header.reliable,
                header.open,
                header.partial,
                header.has_package_map_exports,
                bunch.payload.bit_len(),
                bunch.payload.bytes()
            )
        })
        .collect()
}

fn hex_digest(lines: &[String]) -> String {
    let mut hasher = Sha256::new();
    for line in lines {
        hasher.update(line.as_bytes());
        hasher.update(b"\n");
    }
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Four authored moves around `base` at 150 ms steps from `start`, each
/// followed by the acknowledgement pass and delivery: a walk, a 900 m jump
/// (refused by the speed bound), a based move (skipped) and a falling walk.
/// Returns every bunch the client received.
fn located_moves(
    lab: &mut PeerLab,
    socket: &UdpSocket,
    client: &mut PacketSequencer,
    start: Instant,
    base: [f32; 3],
    first_timestamp: f32,
) -> Vec<Bunch> {
    use super::move_acks::{authored_move, move_bunch_payload, packed, send_at, unreliable_bunch};
    let engine = lab.pawn_open.as_ref().unwrap().0.engine_net_version;
    let [x, y, z] = base;
    let fields = [
        authored_move(first_timestamp, [x + 10.0, y, z], Some(1)),
        authored_move(first_timestamp + 0.15, [x + 90_000.0, y, z], Some(1)),
        packed([x + 20.0, y, z], Some(0x7fff_fff1), Some(1)),
        authored_move(first_timestamp + 0.45, [x + 30.0, y, z], Some(3)),
    ];
    let mut bunches = Vec::new();
    for (index, field) in fields.into_iter().enumerate() {
        let now = start + Duration::from_millis(index as u64 * 150);
        send_at(
            lab,
            socket,
            client,
            vec![unreliable_bunch(
                PAWN_CHANNEL_INDEX,
                move_bunch_payload(field, engine),
            )],
            now,
        );
        lab.retry_pawn_move_acks(now);
        bunches.extend(deliver_bunches(lab, socket, client, |_| true));
    }
    bunches
}

/// The early-pawn lab of `stock_lab` with move acknowledgements.
fn located_lab() -> PeerLab {
    let mut lab = stock_lab(true);
    lab.enable_pawn_move_acks().unwrap();
    lab
}

/// One survival debit of every need, then the checkpoint bridge: the pawn
/// lines of the vitals update it sends.
fn debit_update_lines(
    lab: &mut PeerLab,
    registry: &mut PlayerRegistry,
    socket: &UdpSocket,
    client: &mut PacketSequencer,
) -> Vec<String> {
    use crate::native_player_runtime::ConsumptionRates;
    assert_eq!(
        registry
            .consume_activated(ConsumptionRates::new(1, 1, 1).unwrap())
            .unwrap(),
        1
    );
    lab.bridge_player_checkpoints(registry);
    let update = deliver_bunches(lab, socket, client, |_| true);
    assert_eq!(pawn_updates(&update).len(), 1, "one vitals update");
    pawn_channel_lines(&update)
}

/// A fresh early-pawn join that moves ([`located_moves`]) and disconnects;
/// its stored location is then edited to `[700, 800, 900]`; a returning join
/// opens at the second spawn point and moves. Digests of each session's
/// pawn-channel bunches (opens, move responses, the frozen release and the
/// vitals update of one survival debit) and of the player store's serialized
/// snapshot after each session. `between` runs once after each session's
/// moves, before the debit, with the lab and the registry.
fn located_digests(
    lab: &mut PeerLab,
    registry: &mut PlayerRegistry,
    between: impl Fn(&mut PeerLab, &mut PlayerRegistry),
) -> Vec<String> {
    let key = PlayerKey::new(ACCOUNT, 3).unwrap();
    let mut digests = Vec::new();
    let mut first = stock_join(lab, registry, 1, 24);
    assert_eq!(first.log.last().map(String::as_str), Some("frozen release"));
    let mut lines = pawn_channel_lines(&first.received);
    lines.extend(pawn_channel_lines(&located_moves(
        lab,
        &first._socket,
        &mut first._client,
        Instant::now(),
        [100.0, 200.0, 300.0],
        4.0,
    )));
    between(lab, registry);
    lines.extend(debit_update_lines(
        lab,
        registry,
        &first._socket,
        &mut first._client,
    ));
    digests.push(hex_digest(&lines));
    lab.remove_peer(first.address);
    lab.bridge_player_checkpoints(registry);
    digests.push(hex_digest(&[format!(
        "{:02x?}",
        serde_json::to_vec(&registry.snapshot()).unwrap()
    )]));
    let stored = registry.snapshot().projection(key).unwrap();
    let mut checkpoint = stored.checkpoint;
    checkpoint.location_cm = [700.0, 800.0, 900.0];
    registry.edit(key, stored.revision, checkpoint).unwrap();
    let mut second = stock_join(lab, registry, 2, 24);
    let mut lines = pawn_channel_lines(&second.received);
    lines.extend(pawn_channel_lines(&located_moves(
        lab,
        &second._socket,
        &mut second._client,
        Instant::now(),
        [400.0, 500.0, 600.0],
        2.0,
    )));
    between(lab, registry);
    lines.extend(debit_update_lines(
        lab,
        registry,
        &second._socket,
        &mut second._client,
    ));
    digests.push(hex_digest(&lines));
    lab.remove_peer(second.address);
    lab.bridge_player_checkpoints(registry);
    digests.push(hex_digest(&[format!(
        "{:02x?}",
        serde_json::to_vec(&registry.snapshot()).unwrap()
    )]));
    digests
}

use super::super::super::{
    character_move::{
        MOVEMENT_MODE_FALLING, MoveCorrection, MoveResponse, compose_move_correction_block,
        move_response_codec, read_move_response,
    },
    rep_rpc::{
        BP_ICARUS_PLAYER_CHARACTER_SURVIVAL_FIELD_INDEX_BOUND,
        CLIENT_MOVE_RESPONSE_PACKED_FIELD_INDEX,
    },
};

/// [`located_lab`] with saved locations and, when `correct`, live relocation.
fn saving_lab(correct: bool) -> PeerLab {
    let mut lab = located_lab();
    lab.enable_checkpoint_saved_locations().unwrap();
    if correct {
        lab.enable_checkpoint_location_correction().unwrap();
    }
    lab
}

/// A world holding `registry`: the exclusive boundary saved locations need.
fn registry_world(registry: PlayerRegistry) -> bevy::prelude::World {
    let mut world = bevy::prelude::World::new();
    world.insert_resource(registry);
    world
}

/// Run `pass` with `registry` moved into a world and back.
fn in_world(registry: &mut PlayerRegistry, pass: impl FnOnce(&mut bevy::prelude::World)) {
    let mut world = registry_world(std::mem::replace(registry, self::registry()));
    pass(&mut world);
    *registry = world.remove_resource::<PlayerRegistry>().unwrap();
}

/// One authored move bunch carrying `field`, received at `now`, then the
/// move-response pass at `now`; returns the bunches the client received.
fn move_now(
    lab: &mut PeerLab,
    socket: &UdpSocket,
    client: &mut PacketSequencer,
    field: Option<Payload>,
    now: Instant,
) -> Vec<Bunch> {
    use super::move_acks::{move_bunch_payload, send_at, unreliable_bunch};
    let engine = lab.pawn_open.as_ref().unwrap().0.engine_net_version;
    if let Some(field) = field {
        send_at(
            lab,
            socket,
            client,
            vec![unreliable_bunch(
                PAWN_CHANNEL_INDEX,
                move_bunch_payload(field, engine),
            )],
            now,
        );
    }
    lab.retry_pawn_move_acks(now);
    deliver_bunches(lab, socket, client, |_| true)
}

/// Every field-30 response among `bunches`, each the only field of the only
/// block of an unreliable pawn bunch, read strictly.
fn move_responses(lab: &PeerLab, bunches: &[Bunch]) -> Vec<MoveResponse> {
    let engine = lab.pawn_open.as_ref().unwrap().0.engine_net_version;
    let codec = move_response_codec(engine, ByteOrder::Little);
    bunches
        .iter()
        .filter(|b| b.header.channel_index == PAWN_CHANNEL_INDEX && !b.header.reliable)
        .map(|bunch| {
            assert!(!bunch.header.open && !bunch.header.partial);
            let body =
                read_actor_bunch_body(&mut bunch.payload.reader(), false, &codec.actor).unwrap();
            let [block] = body.blocks.as_slice() else {
                panic!("one block");
            };
            assert!(!block.has_rep_layout);
            let mut reader = block.payload.reader();
            let field = read_field_header(
                &mut reader,
                BP_ICARUS_PLAYER_CHARACTER_SURVIVAL_FIELD_INDEX_BOUND,
                usize::MAX,
            )
            .unwrap();
            assert!(reader.at_end());
            assert_eq!(field.index, CLIENT_MOVE_RESPONSE_PACKED_FIELD_INDEX);
            read_move_response(&field.payload, &codec).unwrap()
        })
        .collect()
}

/// The parent commit's [`located_digests`] (2968a6a, before both options
/// existed), produced on that commit's `src/` with only the helpers above
/// added and a no-op `between`: session one's pawn bunches (with its debit's
/// vitals update), the store after it, session two's pawn bunches, the store
/// after it.
const LOCATED_BASELINE: [&str; 4] = [
    "529df6a7b98c58ba799f7ef58c4f72007f91fd4d28c734c4963f0fbe10d40216",
    "6daae0b687f45d8a1c00179e9e6a8ffcd6d0c2523d28ca25d423ad971a245016",
    "af3deb987d9071210679ac5f4a7a3815ea4a455808e516d2ac56443124d15363",
    "ea4d72171549e9f3c57091e17e72821bfdccfb7c25112269682de97f799a4a5f",
];

/// Both options off (the default), the early-pawn session with move
/// acknowledgements sends and stores exactly the parent commit's bytes (the
/// pawn opens, move responses, frozen release and a survival debit's vitals
/// update, and the serialized player store). Both on, saving adds no wire
/// traffic of its own (the debit after a location revision sends the same
/// update bytes) but changes the store, and the returning session is
/// corrected instead of acknowledged.
#[test]
fn both_options_off_send_and_store_the_parent_commits_bytes() {
    assert!(!crate::config::NetworkConfig::default().checkpoint_saved_locations);
    assert!(!crate::config::NetworkConfig::default().checkpoint_location_correction);
    let (mut lab, mut registry) = (located_lab(), registry());
    let flush = |lab: &mut PeerLab, registry: &mut PlayerRegistry| {
        in_world(registry, |world| lab.flush_saved_locations(world));
    };
    assert_eq!(
        located_digests(&mut lab, &mut registry, flush),
        LOCATED_BASELINE
    );
    assert!(!lab.has_checkpoint_saved_locations() && !lab.has_checkpoint_location_correction());
    assert_eq!(lab.stats.checkpoint_locations_unapplied, 1);
    assert_eq!(lab.stats.checkpoint_saved_locations, Default::default());
    assert_eq!(lab.stats.checkpoint_relocation, Default::default());
    let counts = lab.session_counts();
    assert_eq!(counts.checkpoint_saved_locations, None);
    assert_eq!(counts.checkpoint_relocation, None);
    let text = serde_json::to_string(&counts).unwrap();
    assert!(
        !text.contains("checkpoint_saved_locations") && !text.contains("checkpoint_relocation")
    );
    let (mut on, mut registry) = (saving_lab(true), self::registry());
    let digests = located_digests(&mut on, &mut registry, flush);
    assert_eq!(digests[0], LOCATED_BASELINE[0], "saving sends nothing");
    assert!(
        digests[1..]
            .iter()
            .zip(&LOCATED_BASELINE[1..])
            .all(|(on, off)| on != off)
    );
    assert_eq!(on.stats.checkpoint_locations_unapplied, 0);
    assert_eq!(on.stats.checkpoint_relocation.started, 1);
    assert_eq!(on.stats.checkpoint_relocation.corrections_sent, 1);
}

#[test]
fn saved_location_is_the_last_accepted_move_and_never_a_refused_or_based_one() {
    use super::move_acks::{authored_move, packed};
    let mut lab = saving_lab(false);
    let mut world = registry_world(registry());
    let mut join = stock_join(&mut lab, &mut world.resource_mut::<PlayerRegistry>(), 1, 24);
    assert_eq!(join.log.last().map(String::as_str), Some("frozen release"));
    let key = PlayerKey::new(ACCOUNT, 3).unwrap();
    let stored = |world: &bevy::prelude::World| {
        world
            .resource::<PlayerRegistry>()
            .snapshot()
            .projection(key)
            .unwrap()
    };
    let opened = stored(&world);
    assert_eq!(opened.checkpoint.location_cm, [100.0, 200.0, 300.0]);
    let start = Instant::now();
    // No accepted move yet: the first (periodic) pass saves nothing.
    lab.save_locations(&mut world, start, false);
    assert_eq!(stored(&world), opened);
    located_moves(
        &mut lab,
        &join._socket,
        &mut join._client,
        start,
        [100.0, 200.0, 300.0],
        4.0,
    );
    assert_eq!(
        (
            lab.stats.pawn_move_positions_accepted,
            lab.stats.pawn_move_positions_rejected,
            lab.stats.pawn_move_positions_skipped
        ),
        (2, 1, 1)
    );
    // Within the interval and without a leave, nothing is saved.
    lab.save_locations(&mut world, start + Duration::from_secs(9), false);
    assert_eq!(stored(&world), opened);
    lab.save_locations(&mut world, start + LOCATION_SAVE_INTERVAL, false);
    let saved = stored(&world);
    assert_eq!(saved.checkpoint.location_cm, [130.0, 200.0, 300.0]);
    assert_eq!(saved.checkpoint.vitals, opened.checkpoint.vitals);
    assert_eq!(saved.revision, opened.revision + 1);
    assert_eq!(saved.player_id, opened.player_id);
    // The location-only revision sends no pawn update.
    lab.bridge_player_checkpoints(&mut world.resource_mut::<PlayerRegistry>());
    let after = deliver_bunches(&mut lab, &join._socket, &mut join._client, |_| true);
    assert!(pawn_updates(&after).is_empty());
    assert_eq!(
        lab.peers[&join.address].checkpoint_enqueued_revision,
        Some(saved.revision)
    );
    // A refused jump and a based move later change nothing, nor does an
    // unchanged location consume a revision.
    let later = start + Duration::from_secs(11);
    move_now(
        &mut lab,
        &join._socket,
        &mut join._client,
        Some(authored_move(6.0, [500_000.0, 200.0, 300.0], Some(1))),
        later,
    );
    move_now(
        &mut lab,
        &join._socket,
        &mut join._client,
        Some(packed([140.0, 200.0, 300.0], Some(0x7fff_fff1), Some(1))),
        later + Duration::from_millis(150),
    );
    lab.flush_saved_locations(&mut world);
    assert_eq!(stored(&world), saved);
    assert_eq!(
        lab.session_counts().checkpoint_saved_locations,
        Some(SavedLocationCounts {
            saved: 1,
            saved_at_leave: 0,
            refused: 0,
            withheld: 0
        })
    );
    // A disconnect between passes keeps the last saved location; teardown
    // of the network owner flushes the latest first.
    move_now(
        &mut lab,
        &join._socket,
        &mut join._client,
        Some(authored_move(6.5, [150.0, 200.0, 300.0], Some(1))),
        later + Duration::from_secs(1),
    );
    world.insert_resource(lab);
    crate::network::teardown(&mut world);
    let torn = stored(&world);
    assert_eq!(torn.checkpoint.location_cm, [150.0, 200.0, 300.0]);
    assert_eq!(torn.revision, saved.revision + 1);
    assert!(
        world
            .resource::<PlayerRegistry>()
            .active_epochs()
            .is_empty()
    );
}

/// A survival debit between two saves keeps the saved location; a save keeps
/// the debited vitals: each replaces only its own part of the current record.
#[test]
fn survival_debits_and_location_saves_never_overwrite_each_other() {
    use super::move_acks::authored_move;
    use crate::native_player_runtime::ConsumptionRates;
    let mut lab = saving_lab(false);
    let mut world = registry_world(registry());
    let mut join = stock_join(&mut lab, &mut world.resource_mut::<PlayerRegistry>(), 1, 24);
    let key = PlayerKey::new(ACCOUNT, 3).unwrap();
    let start = Instant::now();
    move_now(
        &mut lab,
        &join._socket,
        &mut join._client,
        Some(authored_move(4.0, [120.0, 200.0, 300.0], Some(1))),
        start,
    );
    lab.save_locations(&mut world, start, false);
    let rates = ConsumptionRates::new(1, 2, 3).unwrap();
    let before = world
        .resource::<PlayerRegistry>()
        .snapshot()
        .projection(key)
        .unwrap();
    assert_eq!(
        world
            .resource_mut::<PlayerRegistry>()
            .consume_activated(rates)
            .unwrap(),
        1
    );
    let debited = world
        .resource::<PlayerRegistry>()
        .snapshot()
        .projection(key)
        .unwrap();
    assert_eq!(debited.checkpoint.location_cm, [120.0, 200.0, 300.0]);
    assert_eq!(
        debited.checkpoint.vitals.oxygen,
        before.checkpoint.vitals.oxygen - 1
    );
    assert_eq!(debited.revision, before.revision + 1);
    move_now(
        &mut lab,
        &join._socket,
        &mut join._client,
        Some(authored_move(4.2, [125.0, 200.0, 300.0], Some(1))),
        start + Duration::from_millis(200),
    );
    lab.save_locations(&mut world, start + LOCATION_SAVE_INTERVAL, false);
    let saved = world
        .resource::<PlayerRegistry>()
        .snapshot()
        .projection(key)
        .unwrap();
    assert_eq!(saved.checkpoint.vitals, debited.checkpoint.vitals);
    assert_eq!(saved.checkpoint.location_cm, [125.0, 200.0, 300.0]);
    assert_eq!(saved.revision, debited.revision + 1);
    // The debit's vitals update goes out once; the location revision after it
    // sends none of its own.
    lab.bridge_player_checkpoints(&mut world.resource_mut::<PlayerRegistry>());
    let updates = deliver_bunches(&mut lab, &join._socket, &mut join._client, |_| true);
    let [update] = pawn_updates(&updates).try_into().unwrap();
    assert_eq!(
        update_values(&lab, &update.payload, ByteOrder::Little),
        with_alive(saved.checkpoint.vitals)
    );
}

/// Walk `steps` moves of 40 m along x, one a second (within the speed bound),
/// from `from`; returns the last location.
fn walk(
    lab: &mut PeerLab,
    socket: &UdpSocket,
    client: &mut PacketSequencer,
    start: Instant,
    from: [f32; 3],
    steps: u16,
) -> [f32; 3] {
    use super::move_acks::authored_move;
    let mut at = from;
    for step in 1..=steps {
        at = [from[0] + f32::from(step) * 4000.0, from[1], from[2]];
        move_now(
            lab,
            socket,
            client,
            Some(authored_move(1.0 + f32::from(step), at, Some(1))),
            start + Duration::from_secs(u64::from(step)),
        );
    }
    at
}

/// The full loop: a player walks away from the spawn point and leaves; the
/// store round-trips through a restart; on rejoin the early pawn opens at
/// the spawn point and exactly one correction answers the first move, to the
/// saved location raised by 50 cm, zero velocity, Falling, with that move's
/// timestamp; no acknowledgement until a move near the target applies it;
/// then acknowledgements resume, the frozen release follows the correction
/// and saving continues from the client's reports.
#[test]
fn rejoin_is_corrected_once_in_answer_to_the_first_move_then_applied() {
    use super::move_acks::authored_move;
    let mut lab = saving_lab(true);
    let mut world = registry_world(registry());
    let mut first = stock_join(&mut lab, &mut world.resource_mut::<PlayerRegistry>(), 1, 24);
    let left_at = walk(
        &mut lab,
        &first._socket,
        &mut first._client,
        Instant::now(),
        [100.0, 200.0, 300.0],
        3,
    );
    assert_eq!(left_at, [12_100.0, 200.0, 300.0]);
    assert_eq!(lab.stats.pawn_move_acks_sent, 3);
    assert_eq!(lab.stats.checkpoint_relocation, Default::default());
    world.insert_resource(lab);
    crate::network::teardown(&mut world);
    let key = PlayerKey::new(ACCOUNT, 3).unwrap();
    let saved = world
        .resource::<PlayerRegistry>()
        .snapshot()
        .projection(key)
        .unwrap();
    assert_eq!(saved.checkpoint.location_cm, left_at.map(f64::from));
    // Restart: the store round-trips; a new lab and registry bind it.
    let directory = tempfile::tempdir().unwrap();
    let mut store = SnapshotStore::open(directory.path()).unwrap();
    store
        .save(&WorldSnapshot {
            schema_version: SNAPSHOT_SCHEMA_PLAYERS,
            native_players: Some(world.resource::<PlayerRegistry>().snapshot()),
            ..Default::default()
        })
        .unwrap();
    let loaded = store
        .load()
        .unwrap()
        .unwrap()
        .snapshot
        .native_players
        .unwrap();
    assert_eq!(loaded.projection(key).unwrap().checkpoint, saved.checkpoint);
    let mut lab = saving_lab(true);
    let mut world = registry_world(PlayerRegistry::new(loaded, &context(), 8).unwrap());
    let mut join = stock_join(&mut lab, &mut world.resource_mut::<PlayerRegistry>(), 2, 24);
    let address = join.address;
    // The pawn opened at the first spawn point; the record went out; the
    // frozen release waits for the pending relocation.
    assert!(join.log.iter().any(|event| event == "record"));
    assert!(!join.log.iter().any(|event| event == "frozen release"));
    assert_eq!(lab.stats.frozen_movement_releases_sent, 0);
    assert_eq!(lab.stats.checkpoint_relocation.started, 1);
    let target = [12_100.0, 200.0, 350.0];
    assert_eq!(
        lab.peers[&address].pawn_moves.relocation.map(|r| r.target),
        Some(target)
    );
    assert_eq!(lab.relocation_counts().releases_held, 1);
    let start = Instant::now();
    let first_move = move_now(
        &mut lab,
        &join._socket,
        &mut join._client,
        Some(authored_move(2.5, [105.0, 200.0, 300.0], Some(1))),
        start,
    );
    let responses = move_responses(&lab, &first_move);
    let [MoveResponse::Correction(correction)] = responses.as_slice() else {
        panic!("exactly one correction");
    };
    let expected = MoveCorrection {
        timestamp: 2.5,
        location: target,
        velocity: [0.0; 3],
        rotation: None,
        base: None,
        bone: None,
        mode: Some(MOVEMENT_MODE_FALLING),
        base_relative: false,
        root_motion: None,
    };
    assert_eq!(*correction, expected);
    let engine = lab.pawn_open.as_ref().unwrap().0.engine_net_version;
    let [block] = first_move
        .iter()
        .filter(|b| b.header.channel_index == PAWN_CHANNEL_INDEX)
        .collect::<Vec<_>>()[..]
    else {
        panic!("one pawn bunch");
    };
    assert_eq!(
        block.payload,
        compose_move_correction_block(&expected, &move_response_codec(engine, ByteOrder::Little))
            .unwrap()
    );
    // A move from before the client applied it: neither stored, refused,
    // acknowledged nor answered again within the resend interval.
    let held = move_now(
        &mut lab,
        &join._socket,
        &mut join._client,
        Some(authored_move(2.6, [106.0, 200.0, 300.0], Some(1))),
        start + Duration::from_millis(150),
    );
    assert!(move_responses(&lab, &held).is_empty());
    assert_eq!(lab.stats.pawn_move_positions_rejected, 0);
    assert_eq!(lab.stats.checkpoint_relocation.moves_held, 2);
    // Still frozen while pending.
    lab.retry_frozen_movement_release();
    assert_eq!(lab.stats.frozen_movement_releases_sent, 0);
    // The client now reports the target: applied, then acknowledged.
    let applied = move_now(
        &mut lab,
        &join._socket,
        &mut join._client,
        Some(authored_move(2.7, target, Some(3))),
        start + Duration::from_millis(300),
    );
    assert_eq!(move_responses(&lab, &applied), [MoveResponse::Ack(2.7)]);
    assert_eq!(lab.peers[&address].pawn_moves.relocation, None);
    assert_eq!(lab.stats.checkpoint_relocation.applied, 1);
    assert_eq!(lab.stats.checkpoint_relocation.corrections_sent, 1);
    assert_eq!(lab.stats.checkpoint_relocation.failed, 0);
    assert_eq!(lab.session_counts().checkpoint_locations_unapplied, Some(0));
    // The release follows the applied correction (F-1134's order).
    lab.retry_frozen_movement_release();
    assert_eq!(lab.stats.frozen_movement_releases_sent, 1);
    // Ordinary bounds resume from the client's report; saving continues.
    move_now(
        &mut lab,
        &join._socket,
        &mut join._client,
        Some(authored_move(2.8, [12_100.0, 200.0, 310.0], Some(3))),
        start + Duration::from_millis(450),
    );
    assert_eq!(
        lab.peers[&address].pawn_moves.location.map(|(at, _)| at),
        Some([12_100.0, 200.0, 310.0])
    );
    lab.flush_saved_locations(&mut world);
    assert_eq!(
        world
            .resource::<PlayerRegistry>()
            .snapshot()
            .projection(key)
            .unwrap()
            .checkpoint
            .location_cm,
        [12_100.0, 200.0, 310.0]
    );
    let counts = lab.session_counts().checkpoint_relocation.unwrap();
    assert_eq!((counts.pending, counts.releases_held), (0, 0));
}

/// A returning player at `stored` on a correction lab: the first join
/// stores the player, the stored location is edited, the second join opens
/// at the second spawn point with the relocation pending.
fn relocating_join(
    lab: &mut PeerLab,
    world: &mut bevy::prelude::World,
    stored: [f64; 3],
) -> StockJoin {
    let join = returning_join(lab, world, stored);
    assert!(lab.peers[&join.address].pawn_moves.relocation.is_some());
    join
}

/// A returning player at `stored`: the first join stores the player, the
/// stored location is edited, the second join opens at the second spawn
/// point ([400, 500, 600]).
fn returning_join(
    lab: &mut PeerLab,
    world: &mut bevy::prelude::World,
    stored: [f64; 3],
) -> StockJoin {
    let first = stock_join(lab, &mut world.resource_mut::<PlayerRegistry>(), 1, 24);
    lab.remove_peer(first.address);
    lab.bridge_player_checkpoints(&mut world.resource_mut::<PlayerRegistry>());
    let key = PlayerKey::new(ACCOUNT, 3).unwrap();
    {
        let mut registry = world.resource_mut::<PlayerRegistry>();
        let current = registry.snapshot().projection(key).unwrap();
        let mut checkpoint = current.checkpoint;
        checkpoint.location_cm = stored;
        registry.edit(key, current.revision, checkpoint).unwrap();
    }
    stock_join(lab, &mut world.resource_mut::<PlayerRegistry>(), 2, 24)
}

/// Lost corrections: resent only in answer to later moves, at most one per
/// second; after five a later move still away gives the relocation up,
/// counted in `failed` and `checkpoint_locations_unapplied`; nothing is
/// acknowledged before (the held move that ends it included), acknowledgements
/// resume from the next checked move, and the pawn stays where its client
/// reports it, which is never saved over the stored location. A NAKed
/// correction is never resent by the transport.
#[test]
fn lost_corrections_are_resent_at_the_bounded_rate_then_given_up_and_counted() {
    use super::move_acks::authored_move;
    let mut lab = saving_lab(true);
    let mut world = registry_world(registry());
    let mut join = relocating_join(&mut lab, &mut world, [5000.0, 5000.0, 600.0]);
    let start = Instant::now();
    let mut corrections = Vec::new();
    let mut acks = Vec::new();
    // A move every 250 ms near the spawn point; the client never applies a
    // correction; between 2.2 s and 3.4 s it sends nothing.
    for tick in (0..6_500).step_by(250) {
        let quiet = (2_200..3_400).contains(&tick);
        let field = (!quiet).then(|| {
            authored_move(
                1.0 + tick as f32 / 1000.0,
                [400.0 + tick as f32 / 100.0, 500.0, 600.0],
                Some(1),
            )
        });
        let now = start + Duration::from_millis(tick);
        let received = if tick == 0 {
            // The first correction is lost.
            let engine = lab.pawn_open.as_ref().unwrap().0.engine_net_version;
            use super::move_acks::{move_bunch_payload, send_at, unreliable_bunch};
            send_at(
                &mut lab,
                &join._socket,
                &mut join._client,
                vec![unreliable_bunch(
                    PAWN_CHANNEL_INDEX,
                    move_bunch_payload(field.clone().unwrap(), engine),
                )],
                now,
            );
            lab.retry_pawn_move_acks(now);
            deliver_bunches(&mut lab, &join._socket, &mut join._client, |_| false)
        } else {
            move_now(&mut lab, &join._socket, &mut join._client, field, now)
        };
        for response in move_responses(&lab, &received) {
            match response {
                MoveResponse::Correction(correction) => corrections.push((tick, correction)),
                MoveResponse::Ack(timestamp) => acks.push((tick, timestamp)),
            }
        }
    }
    let at: Vec<u64> = corrections.iter().map(|(tick, _)| *tick).collect();
    // 0 (lost), 1000, 2000, then the first move after the quiet gap, 4500.
    assert_eq!(at, [0, 1000, 2000, 3500, 4500]);
    for pair in at.windows(2) {
        assert!(pair[1] - pair[0] >= 1000);
    }
    for (tick, correction) in &corrections {
        assert_eq!(
            correction.timestamp,
            1.0 + *tick as f32 / 1000.0,
            "answers its move"
        );
        assert_eq!(correction.location, [5000.0, 5000.0, 650.0]);
    }
    // Given up at 5500 ms; the move held then is never acknowledged; the
    // next one, checked as usual, is.
    assert_eq!(acks.first().map(|(tick, _)| *tick), Some(5_750));
    assert!(acks.iter().all(|(tick, _)| *tick >= 5_750));
    assert_eq!(acks.first().map(|(_, timestamp)| *timestamp), Some(6.75));
    let counts = lab.session_counts();
    let relocation = counts.checkpoint_relocation.unwrap();
    assert_eq!(
        (
            relocation.started,
            relocation.corrections_sent,
            relocation.applied,
            relocation.failed,
            relocation.pending
        ),
        (1, 5, 0, 1, 0)
    );
    assert_eq!(counts.checkpoint_locations_unapplied, Some(1));
    assert_eq!(lab.peers[&join.address].pawn_moves.relocation, None);
    // Bounds resume from the spawn point the client reports.
    assert!(lab.peers[&join.address].pawn_moves.location.is_some());
    // That location is never saved: the stored one survives for the next join.
    let key = PlayerKey::new(ACCOUNT, 3).unwrap();
    let before = world.resource::<PlayerRegistry>().snapshot();
    lab.flush_saved_locations(&mut world);
    assert_eq!(world.resource::<PlayerRegistry>().snapshot(), before);
    let stored = before.projection(key).unwrap();
    assert_eq!(stored.checkpoint.location_cm, [5000.0, 5000.0, 600.0]);
    let saved = lab.session_counts().checkpoint_saved_locations.unwrap();
    assert_eq!((saved.saved, saved.withheld), (0, 1));
}

/// The time bound: a relocation whose client stops moving after one
/// correction gives up 30 s after the checkpoint was bound.
#[test]
fn a_relocation_without_an_answer_gives_up_at_its_time_bound() {
    use super::move_acks::authored_move;
    let mut lab = saving_lab(true);
    let mut world = registry_world(registry());
    let mut join = relocating_join(&mut lab, &mut world, [5000.0, 5000.0, 600.0]);
    let since = lab.peers[&join.address]
        .pawn_moves
        .relocation
        .unwrap()
        .since;
    let first = move_now(
        &mut lab,
        &join._socket,
        &mut join._client,
        Some(authored_move(3.0, [400.0, 500.0, 600.0], Some(1))),
        since + Duration::from_millis(10),
    );
    assert_eq!(move_responses(&lab, &first).len(), 1);
    lab.retry_pawn_move_acks(since + RELOCATION_TIMEOUT - Duration::from_millis(1));
    assert!(lab.peers[&join.address].pawn_moves.relocation.is_some());
    lab.retry_pawn_move_acks(since + RELOCATION_TIMEOUT);
    assert_eq!(lab.peers[&join.address].pawn_moves.relocation, None);
    assert_eq!(lab.stats.checkpoint_relocation.failed, 1);
    assert_eq!(lab.stats.checkpoint_locations_unapplied, 1);
    // That move was answered by the correction; the next one is acknowledged.
    let acked = move_now(
        &mut lab,
        &join._socket,
        &mut join._client,
        Some(authored_move(40.0, [401.0, 500.0, 600.0], Some(1))),
        since + RELOCATION_TIMEOUT + Duration::from_millis(200),
    );
    assert_eq!(move_responses(&lab, &acked), [MoveResponse::Ack(40.0)]);
}

/// A client that sends no move: nothing can be corrected (a correction needs
/// a move's timestamp). The record and finalisation go ahead; the frozen
/// release waits at most its hold past its due time, then goes out with the
/// relocation still pending; the first later move is corrected.
#[test]
fn a_silent_client_blocks_neither_its_record_nor_its_release() {
    use super::move_acks::authored_move;
    let mut lab = saving_lab(true);
    let mut world = registry_world(registry());
    let mut join = relocating_join(&mut lab, &mut world, [5000.0, 5000.0, 600.0]);
    let address = join.address;
    assert!(join.log.iter().any(|event| event == "record"));
    assert!(lab.peers[&address].profiles.finalised);
    assert_eq!(
        lab.stats.frozen_movement_releases_sent, 1,
        "the first join's"
    );
    let due = lab.peers[&address].frozen_release.due_at.unwrap();
    assert_eq!(lab.relocation_counts().releases_held, 1);
    // Due, within the hold: still held.
    let entry = lab.peers.get_mut(&address).unwrap();
    entry.frozen_release.due_at = Some(due.min(Instant::now()));
    lab.retry_frozen_movement_release();
    assert_eq!(lab.stats.frozen_movement_releases_sent, 1);
    // Past the hold: released, the relocation still pending.
    let entry = lab.peers.get_mut(&address).unwrap();
    entry.frozen_release.due_at =
        Some(Instant::now() - RELOCATION_RELEASE_HOLD - Duration::from_millis(1));
    lab.retry_frozen_movement_release();
    assert_eq!(lab.stats.frozen_movement_releases_sent, 2);
    // The release: one unreliable pawn property update, no move response.
    let released = deliver_bunches(&mut lab, &join._socket, &mut join._client, |_| true);
    let [release] = released
        .iter()
        .filter(|b| b.header.channel_index == PAWN_CHANNEL_INDEX)
        .collect::<Vec<_>>()[..]
    else {
        panic!("one pawn bunch");
    };
    assert!(!release.header.reliable);
    assert!(lab.peers[&address].pawn_moves.relocation.is_some());
    assert_eq!(lab.stats.checkpoint_relocation.corrections_sent, 0);
    assert_eq!(lab.stats.checkpoint_locations_unapplied, 0);
    let counts = lab.relocation_counts();
    assert_eq!((counts.pending, counts.releases_held), (1, 0));
    // Nothing is saved while pending, however often the pass runs, even with
    // a session location held (here one from before the binding).
    let key = PlayerKey::new(ACCOUNT, 3).unwrap();
    let before = world.resource::<PlayerRegistry>().snapshot();
    lab.peers.get_mut(&address).unwrap().pawn_moves.location =
        Some(([401.0, 500.0, 600.0], Instant::now()));
    lab.flush_saved_locations(&mut world);
    assert_eq!(world.resource::<PlayerRegistry>().snapshot(), before);
    assert_eq!(
        before.projection(key).unwrap().checkpoint.location_cm,
        [5000.0, 5000.0, 600.0]
    );
    // The first move after the release is corrected.
    let received = move_now(
        &mut lab,
        &join._socket,
        &mut join._client,
        Some(authored_move(9.0, [400.0, 500.0, 600.0], Some(1))),
        Instant::now(),
    );
    let responses = move_responses(&lab, &received);
    let [MoveResponse::Correction(correction)] = responses.as_slice() else {
        panic!("one correction");
    };
    assert_eq!(correction.timestamp, 9.0);
    // Its move is not saved while pending either.
    lab.flush_saved_locations(&mut world);
    assert_eq!(world.resource::<PlayerRegistry>().snapshot(), before);
}

/// The switch APIs: before peers, saving needs checkpoints and move reading;
/// correction needs the early pawn and move reading; move reading cannot be
/// turned off under either. Correction off counts a differing returning
/// location unapplied, as before; a target outside the placement bound is
/// refused and counted the same way.
#[test]
fn saved_locations_and_correction_need_their_prerequisites_before_peers() {
    let mut plain = record_lab(false);
    plain.enable_pawn_move_acks().unwrap();
    assert!(
        plain
            .enable_checkpoint_saved_locations()
            .unwrap_err()
            .to_string()
            .contains("require player checkpoints")
    );
    let mut lab = lab();
    assert!(
        lab.enable_checkpoint_saved_locations()
            .unwrap_err()
            .to_string()
            .contains("require pawn move acknowledgements")
    );
    lab.enable_pawn_move_acks().unwrap();
    assert!(
        lab.enable_checkpoint_location_correction()
            .unwrap_err()
            .to_string()
            .contains("requires the early checkpoint pawn")
    );
    let mut early = early_lab();
    assert!(
        early
            .enable_checkpoint_location_correction()
            .unwrap_err()
            .to_string()
            .contains("requires pawn move acknowledgements")
    );
    let mut on = saving_lab(true);
    assert!(on.has_checkpoint_saved_locations() && on.has_checkpoint_location_correction());
    assert!(on.disable_pawn_move_acks().is_err());
    assert!(on.has_pawn_move_acks());
    let mut late = located_lab();
    let (_socket, _address, _client, _) = peer(&mut late, 1, ACCOUNT, true);
    assert!(
        late.enable_checkpoint_saved_locations()
            .unwrap_err()
            .to_string()
            .contains("must precede peers")
    );
    assert!(
        late.enable_checkpoint_location_correction()
            .unwrap_err()
            .to_string()
            .contains("must precede peers")
    );
    // Off, and refused: both count the location unapplied and relocate nothing.
    for (correct, stored) in [
        (false, [5000.0, 5000.0, 600.0]),
        (true, [0.0, 0.0, 1_048_576.0]),
    ] {
        let mut lab = saving_lab(correct);
        let mut world = registry_world(registry());
        let first = stock_join(&mut lab, &mut world.resource_mut::<PlayerRegistry>(), 1, 24);
        lab.remove_peer(first.address);
        lab.bridge_player_checkpoints(&mut world.resource_mut::<PlayerRegistry>());
        let key = PlayerKey::new(ACCOUNT, 3).unwrap();
        {
            let mut registry = world.resource_mut::<PlayerRegistry>();
            let current = registry.snapshot().projection(key).unwrap();
            let mut checkpoint = current.checkpoint;
            checkpoint.location_cm = stored;
            registry.edit(key, current.revision, checkpoint).unwrap();
        }
        let join = stock_join(&mut lab, &mut world.resource_mut::<PlayerRegistry>(), 2, 24);
        assert_eq!(lab.peers[&join.address].pawn_moves.relocation, None);
        assert_eq!(lab.stats.checkpoint_locations_unapplied, 1);
        assert_eq!(lab.stats.checkpoint_relocation.started, 0);
        assert_eq!(join.log.last().map(String::as_str), Some("frozen release"));
    }
}

/// The player's stored checkpoint.
fn stored_checkpoint(world: &bevy::prelude::World) -> crate::native_players::PlayerProjection {
    world
        .resource::<PlayerRegistry>()
        .snapshot()
        .projection(PlayerKey::new(ACCOUNT, 3).unwrap())
        .unwrap()
}

/// Accepted moves near the second spawn point, 150 ms apart from `start`.
fn moves_near_spawn(lab: &mut PeerLab, join: &mut StockJoin, start: Instant, count: u16) {
    use super::move_acks::authored_move;
    for step in 0..count {
        move_now(
            lab,
            &join._socket,
            &mut join._client,
            Some(authored_move(
                20.0 + f32::from(step) * 0.15,
                [410.0 + f32::from(step) * 10.0, 500.0, 600.0],
                Some(1),
            )),
            start + Duration::from_millis(u64::from(step) * 150),
        );
    }
}

/// Rule: a session saves only when its pawn's position descends from the
/// stored location. Correction off, a returning player whose stored location
/// differs from where the early pawn opened walks near the spawn point; no
/// periodic pass and no teardown saves over the stored location (each is
/// counted `withheld`), and the next join still sees it.
#[test]
fn an_unapplied_stored_location_is_never_saved_over() {
    let stored = [5000.0, 5000.0, 600.0];
    let mut lab = saving_lab(false);
    let mut world = registry_world(registry());
    let mut join = returning_join(&mut lab, &mut world, stored);
    assert_eq!(lab.peers[&join.address].pawn_moves.relocation, None);
    assert!(!lab.peers[&join.address].pawn_moves.saves_location);
    assert_eq!(lab.stats.checkpoint_locations_unapplied, 1);
    let start = Instant::now();
    moves_near_spawn(&mut lab, &mut join, start, 3);
    assert_eq!(
        lab.peers[&join.address]
            .pawn_moves
            .location
            .map(|(at, _)| at),
        Some([430.0, 500.0, 600.0])
    );
    let before = world.resource::<PlayerRegistry>().snapshot();
    lab.save_locations(&mut world, start, false);
    lab.save_locations(&mut world, start + LOCATION_SAVE_INTERVAL, false);
    assert_eq!(world.resource::<PlayerRegistry>().snapshot(), before);
    world.insert_resource(lab);
    let counts = crate::network::teardown(&mut world).unwrap();
    assert_eq!(world.resource::<PlayerRegistry>().snapshot(), before);
    assert_eq!(stored_checkpoint(&world).checkpoint.location_cm, stored);
    assert_eq!(
        counts.checkpoint_saved_locations,
        Some(SavedLocationCounts {
            saved: 0,
            saved_at_leave: 0,
            refused: 0,
            withheld: 3
        })
    );
    // The next join still finds it: a correction lab relocates to it. The
    // registry's epochs continue above the last lab's.
    let mut next = saving_lab(true);
    next.next_checkpoint_epoch = 3;
    let join = stock_join(
        &mut next,
        &mut world.resource_mut::<PlayerRegistry>(),
        1,
        24,
    );
    assert_eq!(
        next.peers[&join.address]
            .pawn_moves
            .relocation
            .map(|relocation| relocation.target),
        Some([5000.0, 5000.0, 650.0])
    );
}

/// A relocation that fails by time, and one refused because its target is
/// outside the placement bound, both leave the session withheld for good:
/// the stored location is unchanged after periodic passes and teardown.
#[test]
fn a_failed_or_refused_relocation_never_saves_over_the_stored_location() {
    use super::move_acks::authored_move;
    // Failed by time (failure by attempts:
    // `lost_corrections_are_resent_at_the_bounded_rate_then_given_up_and_counted`).
    let stored = [5000.0, 5000.0, 600.0];
    let mut lab = saving_lab(true);
    let mut world = registry_world(registry());
    let mut join = relocating_join(&mut lab, &mut world, stored);
    let since = lab.peers[&join.address]
        .pawn_moves
        .relocation
        .unwrap()
        .since;
    move_now(
        &mut lab,
        &join._socket,
        &mut join._client,
        Some(authored_move(3.0, [400.0, 500.0, 600.0], Some(1))),
        since + Duration::from_millis(10),
    );
    lab.retry_pawn_move_acks(since + RELOCATION_TIMEOUT);
    assert_eq!(lab.stats.checkpoint_relocation.failed, 1);
    moves_near_spawn(
        &mut lab,
        &mut join,
        since + RELOCATION_TIMEOUT + Duration::from_millis(200),
        2,
    );
    assert!(lab.peers[&join.address].pawn_moves.location.is_some());
    let before = world.resource::<PlayerRegistry>().snapshot();
    lab.save_locations(&mut world, since + RELOCATION_TIMEOUT, false);
    world.insert_resource(lab);
    let counts = crate::network::teardown(&mut world).unwrap();
    assert_eq!(world.resource::<PlayerRegistry>().snapshot(), before);
    assert_eq!(stored_checkpoint(&world).checkpoint.location_cm, stored);
    let saved = counts.checkpoint_saved_locations.unwrap();
    assert_eq!((saved.saved, saved.withheld), (0, 2));
    // Refused: the raised target exceeds the bound; counted unapplied.
    let stored = [0.0, 0.0, 1_048_576.0];
    let mut lab = saving_lab(true);
    let mut world = registry_world(registry());
    let mut join = returning_join(&mut lab, &mut world, stored);
    assert_eq!(lab.peers[&join.address].pawn_moves.relocation, None);
    assert_eq!(lab.stats.checkpoint_locations_unapplied, 1);
    assert_eq!(lab.stats.checkpoint_relocation.started, 0);
    moves_near_spawn(&mut lab, &mut join, Instant::now(), 2);
    assert!(lab.peers[&join.address].pawn_moves.location.is_some());
    let before = world.resource::<PlayerRegistry>().snapshot();
    lab.flush_saved_locations(&mut world);
    world.insert_resource(lab);
    let counts = crate::network::teardown(&mut world).unwrap();
    assert_eq!(world.resource::<PlayerRegistry>().snapshot(), before);
    assert_eq!(stored_checkpoint(&world).checkpoint.location_cm, stored);
    let saved = counts.checkpoint_saved_locations.unwrap();
    assert_eq!((saved.saved, saved.withheld), (0, 2));
}

/// The positive cases of the rule: a new checkpoint saves, a returning
/// player whose stored location is where the early pawn opened saves, and
/// an applied relocation saves (the last two also with correction on).
#[test]
fn new_matched_and_applied_locations_are_saved() {
    use super::move_acks::authored_move;
    // New: the first join's checkpoint.
    let mut lab = saving_lab(true);
    let mut world = registry_world(registry());
    let mut join = stock_join(&mut lab, &mut world.resource_mut::<PlayerRegistry>(), 1, 24);
    assert!(lab.peers[&join.address].pawn_moves.saves_location);
    move_now(
        &mut lab,
        &join._socket,
        &mut join._client,
        Some(authored_move(4.0, [110.0, 200.0, 300.0], Some(1))),
        Instant::now(),
    );
    lab.flush_saved_locations(&mut world);
    assert_eq!(
        stored_checkpoint(&world).checkpoint.location_cm,
        [110.0, 200.0, 300.0]
    );
    // Matched: stored exactly where the returning early pawn opens.
    for correct in [false, true] {
        let mut lab = saving_lab(correct);
        let mut world = registry_world(registry());
        let mut join = returning_join(&mut lab, &mut world, [400.0, 500.0, 600.0]);
        let entry = &lab.peers[&join.address];
        assert!(entry.pawn_moves.relocation.is_none() && entry.pawn_moves.saves_location);
        assert_eq!(lab.stats.checkpoint_locations_unapplied, 0);
        moves_near_spawn(&mut lab, &mut join, Instant::now(), 2);
        lab.flush_saved_locations(&mut world);
        assert_eq!(
            stored_checkpoint(&world).checkpoint.location_cm,
            [420.0, 500.0, 600.0],
            "correct {correct}"
        );
        let saved = lab.session_counts().checkpoint_saved_locations.unwrap();
        assert_eq!((saved.saved, saved.withheld), (1, 0));
    }
    // Applied: the relocation applies, then the client's reports are saved.
    let mut lab = saving_lab(true);
    let mut world = registry_world(registry());
    let mut join = relocating_join(&mut lab, &mut world, [5000.0, 5000.0, 600.0]);
    let start = Instant::now();
    move_now(
        &mut lab,
        &join._socket,
        &mut join._client,
        Some(authored_move(2.5, [405.0, 500.0, 600.0], Some(1))),
        start,
    );
    // Pending: the held move is not stored, so nothing is offered.
    lab.flush_saved_locations(&mut world);
    assert_eq!(
        stored_checkpoint(&world).checkpoint.location_cm,
        [5000.0, 5000.0, 600.0]
    );
    move_now(
        &mut lab,
        &join._socket,
        &mut join._client,
        Some(authored_move(2.7, [5000.0, 5000.0, 640.0], Some(3))),
        start + Duration::from_millis(300),
    );
    assert_eq!(lab.stats.checkpoint_relocation.applied, 1);
    assert!(lab.peers[&join.address].pawn_moves.saves_location);
    lab.flush_saved_locations(&mut world);
    assert_eq!(
        stored_checkpoint(&world).checkpoint.location_cm,
        [5000.0, 5000.0, 640.0]
    );
    let saved = lab.session_counts().checkpoint_saved_locations.unwrap();
    assert_eq!((saved.saved, saved.withheld), (1, 0));
}

/// A move held while a relocation is pending was never checked or stored:
/// when the relocation fails, by time here (by attempts in
/// `lost_corrections_are_resent_at_the_bounded_rate_then_given_up_and_counted`),
/// that pass acknowledges nothing; the next move, checked as usual, is
/// acknowledged.
#[test]
fn a_move_held_when_its_relocation_fails_is_never_acknowledged() {
    use super::move_acks::authored_move;
    let mut lab = saving_lab(true);
    let mut world = registry_world(registry());
    let mut join = relocating_join(&mut lab, &mut world, [5000.0, 5000.0, 600.0]);
    let since = lab.peers[&join.address]
        .pawn_moves
        .relocation
        .unwrap()
        .since;
    let first = move_now(
        &mut lab,
        &join._socket,
        &mut join._client,
        Some(authored_move(3.0, [400.0, 500.0, 600.0], Some(1))),
        since + Duration::from_millis(10),
    );
    assert!(matches!(
        move_responses(&lab, &first).as_slice(),
        [MoveResponse::Correction(_)]
    ));
    // Held, then the same pass gives the relocation up.
    let held = lab.stats.checkpoint_relocation.moves_held;
    let failing = move_now(
        &mut lab,
        &join._socket,
        &mut join._client,
        Some(authored_move(33.0, [90_000.0, 500.0, 600.0], Some(1))),
        since + RELOCATION_TIMEOUT,
    );
    assert_eq!(lab.stats.checkpoint_relocation.moves_held, held + 1);
    assert_eq!(lab.stats.checkpoint_relocation.failed, 1);
    assert!(move_responses(&lab, &failing).is_empty());
    assert!(!lab.peers[&join.address].pawn_moves.unacked);
    // Nothing is due later either, until a new move arrives.
    let idle = move_now(
        &mut lab,
        &join._socket,
        &mut join._client,
        None,
        since + RELOCATION_TIMEOUT + Duration::from_millis(500),
    );
    assert!(move_responses(&lab, &idle).is_empty());
    let next = move_now(
        &mut lab,
        &join._socket,
        &mut join._client,
        Some(authored_move(34.0, [401.0, 500.0, 600.0], Some(1))),
        since + RELOCATION_TIMEOUT + Duration::from_secs(1),
    );
    assert_eq!(move_responses(&lab, &next), [MoveResponse::Ack(34.0)]);
    assert_eq!(lab.stats.pawn_move_positions_accepted, 1);
}

/// Teardown, in `run`'s shutdown order (stop admission, drain the bridge,
/// tear the network owner down, then the final writer submission), commits
/// the last accepted move before the final store submission captures it.
#[test]
fn teardown_saves_the_last_accepted_move_before_the_final_store_submission() {
    use super::move_acks::authored_move;
    use crate::{
        config::ServerConfig, native_player_runtime::CheckpointWriter, simulation::build_app,
    };
    let mut lab = saving_lab(false);
    let snapshot = WorldSnapshot {
        schema_version: SNAPSHOT_SCHEMA_PLAYERS,
        native_players: Some(registry().snapshot()),
        ..Default::default()
    };
    let mut app = build_app(&ServerConfig::default(), &snapshot).unwrap();
    app.insert_resource(registry());
    let mut join = stock_join(
        &mut lab,
        &mut app.world_mut().resource_mut::<PlayerRegistry>(),
        1,
        24,
    );
    let start = Instant::now();
    for (step, x) in [110.0, 120.0, 135.0].into_iter().enumerate() {
        move_now(
            &mut lab,
            &join._socket,
            &mut join._client,
            Some(authored_move(
                4.0 + step as f32 * 0.15,
                [x, 200.0, 300.0],
                Some(1),
            )),
            start + Duration::from_millis(step as u64 * 150),
        );
    }
    let last = lab.peers[&join.address].pawn_moves.location.unwrap().0;
    assert_eq!(last, [135.0, 200.0, 300.0]);
    app.insert_resource(lab);
    let directory = tempfile::tempdir().unwrap();
    let mut writer =
        CheckpointWriter::spawn(SnapshotStore::open(directory.path()).unwrap()).unwrap();
    crate::network::stop_admission(app.world_mut());
    crate::network::drain_player_bridge(app.world_mut());
    let counts = crate::network::teardown(app.world_mut()).unwrap();
    assert_eq!(counts.checkpoint_saved_locations.unwrap().saved, 1);
    writer.submit(app.world_mut()).unwrap();
    writer
        .finish_before(app.world_mut(), Instant::now() + Duration::from_secs(30))
        .unwrap()
        .unwrap();
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
        restored
            .projection(PlayerKey::new(ACCOUNT, 3).unwrap())
            .unwrap()
            .checkpoint
            .location_cm,
        last
    );
}

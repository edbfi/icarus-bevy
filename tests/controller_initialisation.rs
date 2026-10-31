//! Authored loopback sessions only; agreement with our codecs is not stock-client proof.
use std::net::SocketAddrV4;

use bevy::app::App;
use icarus_bevy::{
    config::ServerConfig,
    experimental::{
        actor_channel::{
            ActorBunchBody, ActorChannelCodec, ActorChannelLimits, BlockTarget, ContentBlock,
            NetGuidRef, SubObjectClass, encode_actor_bunch_body,
        },
        archive::ByteOrder,
        packet::{BitWriter, Bunch, BunchHeader, Name, Payload},
        peer_lab::{PeerLab, PeerLabAdmission, PeerLabConfig, PeerLabPlugin},
        release::ReleaseProfile,
        rep_layout::{RepCommand, RepLayoutCodec, RepValue},
        rep_rpc::{
            BP_ICARUS_PLAYER_CONTROLLER_SURVIVAL_FIELD_INDEX_BOUND as BOUND, FieldHeader,
            RpcArgument, RpcItem, RpcParameter,
            SERVER_BEGIN_PLAYER_INITIALISATION_FIELD_INDEX as BEGIN, encode_rpc_payload,
            parse_begin_player_initialisation, write_field_header,
        },
        sequencing::wire_reliable_sequence,
        session_open::{ACTOR_CHANNEL_NAME_INDEX, PLAYER_CONTROLLER_CHANNEL_INDEX as CONTROLLER},
    },
    simulation::{WorldSnapshot, build_app},
};

#[path = "common/loopback_client.rs"]
mod loopback_client;
use loopback_client::{Client, drain};
#[path = "common/profiles.rs"]
mod profiles;

fn codec(order: ByteOrder) -> ActorChannelCodec {
    ActorChannelCodec {
        engine_net_version: 17,
        exports_enabled: false,
        has_level: true,
        order,
        limits: ActorChannelLimits::default(),
    }
}

fn rep_codec(order: ByteOrder) -> RepLayoutCodec {
    RepLayoutCodec {
        actor: codec(order),
        checksums: false,
        limits: Default::default(),
    }
}

fn field(slot: i32, order: ByteOrder) -> FieldHeader {
    FieldHeader {
        index: BEGIN,
        payload: encode_rpc_payload(
            &[RpcParameter::Value(vec![RepCommand::Int])],
            &[RpcArgument::Sent(vec![RpcItem::Value(RepValue::Int(slot))])],
            &rep_codec(order),
        )
        .unwrap(),
    }
}

fn fields_payload(fields: &[FieldHeader]) -> Payload {
    let mut writer = BitWriter::new();
    for field in fields {
        write_field_header(&mut writer, field, BOUND).unwrap();
    }
    Payload::from_writer(writer)
}

fn body(fields: &[FieldHeader], order: ByteOrder) -> Payload {
    encode_actor_bunch_body(
        &ActorBunchBody {
            spawn: None,
            blocks: vec![ContentBlock {
                has_rep_layout: false,
                target: BlockTarget::Actor,
                payload: fields_payload(fields),
            }],
        },
        &codec(order),
    )
    .unwrap()
}

fn actor(channel: u32, sequence: u32, payload: Payload) -> Bunch {
    Bunch {
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
            reliable_sequence_raw: Some(wire_reliable_sequence(sequence)),
            partial_initial: false,
            partial_final: false,
            channel_name: Some(Name::Hardcoded(ACTOR_CHANNEL_NAME_INDEX)),
        },
        payload,
    }
}

fn setup(controller: bool, player_state: bool, order: ByteOrder) -> (App, Client) {
    setup_with_capacity(controller, player_state, order, None)
}

fn setup_with_capacity(
    controller: bool,
    player_state: bool,
    order: ByteOrder,
    sent_capacity: Option<usize>,
) -> (App, Client) {
    let profile = ReleaseProfile::load(
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("config/release.toml"),
    )
    .unwrap();
    let admission = PeerLabAdmission::default();
    let map = admission.map_url;
    let mut config = PeerLabConfig {
        admission: Some(admission),
        ..PeerLabConfig::default().with_release_profile(&profile)
    };
    if let Some(capacity) = sent_capacity {
        config.packet.sent_capacity = capacity;
    }
    let mut lab = PeerLab::bind(0, config).unwrap();
    if controller {
        lab.enable_player_controller_open(
            profile
                .player_controller_open_plan(map, [0.0; 3])
                .unwrap()
                .unwrap(),
        )
        .unwrap();
        if player_state {
            lab.enable_player_state_open(profile.player_state_open_plan(map).unwrap().unwrap())
                .unwrap();
        }
    }
    let address = lab.local_addr().unwrap();
    let mut app = build_app(&ServerConfig::default(), &WorldSnapshot::default()).unwrap();
    app.insert_resource(lab).add_plugins(PeerLabPlugin);
    let client = Client::connect_version(&mut app, address, order, 0, profile.pin.network_checksum);
    (app, client)
}

fn join(app: &mut App, client: &mut Client) {
    let opened = client.join(app);
    client.settle(app, &opened);
}

fn enable_pawn(app: &mut App) {
    use icarus_bevy::experimental::pawn_open::{PAWN_CHANNEL_INDEX, PawnContent};
    let profile = ReleaseProfile::load(
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("config/release.toml"),
    )
    .unwrap();
    app.world_mut()
        .resource_mut::<PeerLab>()
        .enable_pawn_open(
            profile
                .pawn_open_plan(PeerLabAdmission::default().map_url)
                .unwrap()
                .unwrap(),
            PAWN_CHANNEL_INDEX,
            PawnContent::default(),
        )
        .unwrap();
}

fn address(client: &Client) -> SocketAddrV4 {
    let std::net::SocketAddr::V4(address) = client.socket.local_addr().unwrap() else {
        panic!("IPv4")
    };
    address
}

fn slot(app: &App, client: &Client) -> Option<i32> {
    app.world()
        .resource::<PeerLab>()
        .character_slot(address(client))
}

fn send(app: &mut App, client: &mut Client, bunch: Bunch) -> Vec<Vec<u8>> {
    client.send(vec![bunch]);
    app.update();
    let replies = drain(&client.socket);
    assert!(!replies.is_empty(), "input remains acknowledged");
    client.accept(&replies);
    let lab = app.world().resource::<PeerLab>();
    assert!(lab.has_peer(address(client)));
    assert_eq!(lab.stats().peers_failed, 0);
    assert_eq!(lab.stats().failures_sent, 0);
    replies
}

#[test]
fn joined_pinned_session_records_character_slot_once() {
    let (mut app, mut client) = setup(true, true, ByteOrder::Little);
    join(&mut app, &mut client);
    let first = client.sequencer.initial_reliable().1 + 1;
    send(
        &mut app,
        &mut client,
        actor(
            CONTROLLER,
            first,
            body(&[field(3, ByteOrder::Little)], ByteOrder::Little),
        ),
    );
    assert_eq!(slot(&app, &client), Some(3));
    send(
        &mut app,
        &mut client,
        actor(
            CONTROLLER,
            first + 1,
            body(&[field(7, ByteOrder::Little)], ByteOrder::Little),
        ),
    );
    assert_eq!(slot(&app, &client), Some(3));
    let counts = app.world().resource::<PeerLab>().session_counts();
    assert_eq!(
        (
            counts.initialisation_slots_recorded,
            counts.initialisation_calls_ignored
        ),
        (1, 1)
    );
    assert_eq!(counts.joined, 1);
}

/// The controller can deliver Begin before its PlayerState open fits. In the
/// ACK-bearing case that very RPC frees the capacity needed by the open.
#[test]
fn begin_survives_deferred_player_state_and_preserves_the_first_slot() {
    use icarus_bevy::experimental::rep_rpc::CLIENT_REQUEST_GET_USER_PROFILE;

    for ack_with_begin in [false, true] {
        let (mut app, mut client) = setup_with_capacity(true, true, ByteOrder::Little, Some(1));
        let opened = client.join(&mut app);
        let stats = app.world().resource::<PeerLab>().stats();
        assert_eq!(stats.player_controller_opens_sent, 1);
        assert_eq!(stats.player_state_opens_sent, 0);
        assert!(stats.player_state_open_errors > 0);
        if ack_with_begin {
            client.accept(&opened);
        }
        let first = client.sequencer.initial_reliable().1 + 1;
        client.send(vec![actor(
            CONTROLLER,
            first,
            body(
                &[field(3, ByteOrder::Little), field(7, ByteOrder::Little)],
                ByteOrder::Little,
            ),
        )]);
        app.update();
        let replies = drain(&client.socket);
        let mut observed_requests = requests(&client, &replies);
        client.accept(&replies);
        if !ack_with_begin {
            assert_eq!(
                app.world()
                    .resource::<PeerLab>()
                    .stats()
                    .player_state_opens_sent,
                0,
                "Begin alone cannot release the occupied sent ring"
            );
            client.accept(&opened);
        }
        for _ in 0..8 {
            let replies = client.step(&mut app);
            observed_requests.extend(requests(&client, &replies));
            client.accept(&replies);
        }
        assert_eq!(slot(&app, &client), Some(3));
        assert_eq!(observed_requests.len(), 1);
        assert_eq!(
            observed_requests[0].1.index,
            CLIENT_REQUEST_GET_USER_PROFILE
        );
        assert_eq!(observed_requests[0].1.payload.bit_len(), 0);
        let stats = app.world().resource::<PeerLab>().stats();
        assert_eq!(stats.player_state_opens_sent, 1);
        assert_eq!(stats.initialisation_slots_recorded, 1);
        assert_eq!(stats.initialisation_calls_ignored, 1);
        assert_eq!(stats.profile_user_requests_sent, 1);
        assert_eq!(stats.peers_failed, 0);

        // A later reliable duplicate neither changes the retained slot nor
        // starts another profile exchange after the pending Begin is consumed.
        let replies = send(
            &mut app,
            &mut client,
            actor(
                CONTROLLER,
                first + 1,
                body(&[field(9, ByteOrder::Little)], ByteOrder::Little),
            ),
        );
        assert!(requests(&client, &replies).is_empty());
        for _ in 0..4 {
            let replies = client.step(&mut app);
            assert!(requests(&client, &replies).is_empty());
            client.accept(&replies);
        }
        let stats = app.world().resource::<PeerLab>().stats();
        assert_eq!(stats.initialisation_slots_recorded, 1);
        assert_eq!(stats.initialisation_calls_ignored, 2);
        assert_eq!(stats.profile_user_requests_sent, 1);
        assert_eq!(slot(&app, &client), Some(3));
    }
}

#[test]
fn disconnect_discards_begin_waiting_for_player_state() {
    let (mut app, mut client) = setup_with_capacity(true, true, ByteOrder::Little, Some(1));
    let opened = client.join(&mut app);
    assert!(!opened.is_empty());
    assert_eq!(
        app.world()
            .resource::<PeerLab>()
            .stats()
            .player_state_opens_sent,
        0
    );
    // Withhold the controller-open ACK so Begin remains pending when the
    // reliable close arrives. No initialization may resume after removal.
    let first = client.sequencer.initial_reliable().1 + 1;
    client.send(vec![actor(
        CONTROLLER,
        first,
        body(&[field(3, ByteOrder::Little)], ByteOrder::Little),
    )]);
    app.update();
    let replies = drain(&client.socket);
    assert!(requests(&client, &replies).is_empty());
    assert_eq!(
        app.world()
            .resource::<PeerLab>()
            .stats()
            .profile_user_requests_sent,
        0
    );
    client.disconnect(&mut app);
    for _ in 0..4 {
        app.update();
    }
    let lab = app.world().resource::<PeerLab>();
    assert!(!lab.has_peer(address(&client)));
    assert_eq!(lab.stats().player_state_opens_sent, 0);
    assert_eq!(lab.stats().profile_user_requests_sent, 0);
    assert_eq!(lab.stats().peers_failed, 0);
    assert_eq!(slot(&app, &client), None);
}

/// Both PlayerStates wait for transport capacity while the second player joins.
/// The host flag belongs to the Join decision, not the eventual retry order.
#[test]
fn deferred_player_state_preserves_join_time_host_for_two_peers() {
    use icarus_bevy::experimental::{
        actor_channel::read_actor_bunch_body, packet::read_packet, rep_layout::read_rep_properties,
        session_open::PLAYER_STATE_CHANNEL_INDEX,
    };

    let release = ReleaseProfile::load(
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("config/release.toml"),
    )
    .unwrap();
    let layout = release
        .player_state_open_plan(PeerLabAdmission::default().map_url)
        .unwrap()
        .unwrap()
        .layout;
    let (mut app, mut first) = setup_with_capacity(true, true, ByteOrder::Little, Some(1));
    let first_opened = first.join(&mut app);
    let stats = app.world().resource::<PeerLab>().stats();
    assert_eq!(stats.player_controller_opens_sent, 1);
    assert_eq!(stats.player_state_opens_sent, 0);
    assert!(stats.player_state_open_errors > 0);

    // Withhold the first controller ACK until both players have joined.
    let server = app.world().resource::<PeerLab>().local_addr().unwrap();
    let mut second = Client::connect_version(
        &mut app,
        server,
        ByteOrder::Little,
        1,
        release.pin.network_checksum,
    );
    let second_opened = second.join(&mut app);
    let stats = app.world().resource::<PeerLab>().stats();
    assert_eq!(stats.joins_succeeded, 2);
    assert_eq!(stats.player_controller_opens_sent, 2);
    assert_eq!(stats.player_state_opens_sent, 0);

    for (client, opened, expected_host) in [
        (&mut first, &first_opened, true),
        (&mut second, &second_opened, false),
    ] {
        client.accept(opened);
        let replies = client.step(&mut app);
        let mut flags = Vec::new();
        for bytes in &replies {
            for bunch in read_packet(bytes, &client.config()).unwrap().bunches {
                if bunch.header.channel_index != PLAYER_STATE_CHANNEL_INDEX
                    || bunch.header.has_package_map_exports
                {
                    continue;
                }
                let body =
                    read_actor_bunch_body(&mut bunch.payload.reader(), true, &codec(client.order))
                        .unwrap();
                assert_eq!(body.blocks.len(), 1);
                assert_eq!(body.blocks[0].target, BlockTarget::Actor);
                assert!(body.blocks[0].has_rep_layout);
                let stream = read_rep_properties(
                    &mut body.blocks[0].payload.reader(),
                    &layout,
                    &rep_codec(client.order),
                )
                .unwrap();
                let host = stream
                    .entries
                    .iter()
                    .find(|entry| entry.handle == 79)
                    .expect("PlayerState open carries bIsHost");
                flags.push(host.value.clone());
            }
        }
        assert_eq!(flags, [RepValue::Bool(expected_host)]);
        client.accept(&replies);
    }

    let lab = app.world().resource::<PeerLab>();
    assert_eq!(lab.stats().player_state_opens_sent, 2);
    assert!(lab.has_peer(address(&first)));
    assert!(lab.has_peer(address(&second)));
    assert_eq!(lab.stats().peers_failed, 0);
    assert_eq!(lab.stats().io_errors, 0);
}

#[test]
fn controller_rpcs_are_delivered_reliably_in_order() {
    let (mut app, mut client) = setup(true, true, ByteOrder::Little);
    join(&mut app, &mut client);
    let first = client.sequencer.initial_reliable().1 + 1;
    let later = actor(
        CONTROLLER,
        first + 1,
        body(&[field(9, ByteOrder::Little)], ByteOrder::Little),
    );
    send(&mut app, &mut client, later.clone());
    send(&mut app, &mut client, later.clone());
    assert_eq!(slot(&app, &client), None);
    send(
        &mut app,
        &mut client,
        actor(
            CONTROLLER,
            first,
            body(&[field(3, ByteOrder::Little)], ByteOrder::Little),
        ),
    );
    send(&mut app, &mut client, later);
    assert_eq!(slot(&app, &client), Some(3));
    let stats = app.world().resource::<PeerLab>().stats();
    assert_eq!(stats.actor_duplicate_reliable, 2);
    assert_eq!(
        (
            stats.initialisation_slots_recorded,
            stats.initialisation_calls_ignored
        ),
        (1, 1)
    );
}

#[test]
fn invalid_controller_calls_and_payloads_leave_session_live() {
    let (mut app, mut client) = setup(true, true, ByteOrder::Little);
    join(&mut app, &mut client);
    let mut sequence = client.sequencer.initial_reliable().1;
    let mut deliver = |app: &mut App, client: &mut Client, payload| {
        sequence += 1;
        send(app, client, actor(CONTROLLER, sequence, payload));
    };
    deliver(
        &mut app,
        &mut client,
        body(&[field(-1, ByteOrder::Little)], ByteOrder::Little),
    );
    assert_eq!(slot(&app, &client), None);
    let mut opaque = BitWriter::new();
    opaque.write_bits(0b10101, 5);
    let unknown = FieldHeader {
        index: 182,
        payload: Payload::from_writer(opaque),
    };
    deliver(
        &mut app,
        &mut client,
        body(std::slice::from_ref(&unknown), ByteOrder::Little),
    );
    assert_eq!(slot(&app, &client), None);
    // A truncated value, trailing RPC bits, malformed block and truncated
    // field length each drop the entire bunch, even after a valid call.
    let mut truncated = BitWriter::new();
    truncated.write_bit(true);
    let bad_field = FieldHeader {
        index: BEGIN,
        payload: Payload::from_writer(truncated),
    };
    deliver(
        &mut app,
        &mut client,
        body(&[field(3, ByteOrder::Little), bad_field], ByteOrder::Little),
    );
    let mut trailing = BitWriter::new();
    trailing.write_bit(false);
    trailing.write_bit(false);
    deliver(
        &mut app,
        &mut client,
        body(
            &[FieldHeader {
                index: BEGIN,
                payload: Payload::from_writer(trailing),
            }],
            ByteOrder::Little,
        ),
    );
    let mut bad_block = BitWriter::new();
    bad_block.write_bit(false);
    deliver(&mut app, &mut client, Payload::from_writer(bad_block));
    let mut bad_header = BitWriter::new();
    bad_header.write_int_max(BEGIN, BOUND).unwrap();
    let malformed = encode_actor_bunch_body(
        &ActorBunchBody {
            spawn: None,
            blocks: vec![ContentBlock {
                has_rep_layout: false,
                target: BlockTarget::Actor,
                payload: Payload::from_writer(bad_header),
            }],
        },
        &codec(ByteOrder::Little),
    )
    .unwrap();
    deliver(&mut app, &mut client, malformed);
    assert_eq!(slot(&app, &client), None);
    // The unknown field's non-byte-aligned payload must not hide the next RPC.
    deliver(
        &mut app,
        &mut client,
        body(&[unknown, field(3, ByteOrder::Little)], ByteOrder::Little),
    );
    assert_eq!(slot(&app, &client), Some(3));
    let stats = app.world().resource::<PeerLab>().stats();
    assert_eq!(stats.actor_bunches_malformed, 4);
    assert_eq!(stats.controller_fields_ignored, 2);
    assert_eq!(stats.initialisation_calls_ignored, 1);
    assert_eq!(stats.initialisation_slots_recorded, 1);
}

#[test]
fn controller_initialisation_requires_open_controller_and_player_state() {
    for (controller, player_state) in [(false, false), (true, false), (true, true)] {
        let (mut app, mut client) = setup(controller, player_state, ByteOrder::Little);
        let first = client.sequencer.initial_reliable().1 + 1;
        // Before Join, even the configured controller channel is unopened.
        send(
            &mut app,
            &mut client,
            actor(
                CONTROLLER,
                first,
                body(&[field(3, ByteOrder::Little)], ByteOrder::Little),
            ),
        );
        assert_eq!(slot(&app, &client), None);
        join(&mut app, &mut client);
        // Unknown channels never acquire reliability state from client input.
        send(
            &mut app,
            &mut client,
            actor(
                31,
                first,
                body(&[field(3, ByteOrder::Little)], ByteOrder::Little),
            ),
        );
        assert_eq!(slot(&app, &client), None);
        if player_state {
            // An opened PlayerState channel still cannot receive this controller RPC.
            send(
                &mut app,
                &mut client,
                actor(
                    icarus_bevy::experimental::session_open::PLAYER_STATE_CHANNEL_INDEX,
                    first,
                    body(&[field(3, ByteOrder::Little)], ByteOrder::Little),
                ),
            );
            assert_eq!(slot(&app, &client), None);
        }
        send(
            &mut app,
            &mut client,
            actor(
                CONTROLLER,
                first,
                body(&[field(3, ByteOrder::Little)], ByteOrder::Little),
            ),
        );
        assert_eq!(slot(&app, &client), player_state.then_some(3));
        let stats = app.world().resource::<PeerLab>().stats();
        assert_eq!(
            stats.actor_bunches_ignored,
            if controller {
                2 + u64::from(player_state)
            } else {
                3
            }
        );
        assert_eq!(
            stats.initialisation_calls_ignored,
            u64::from(controller && !player_state)
        );
    }
}

#[test]
fn subobject_rpc_does_not_initialise_the_controller() {
    let (mut app, mut client) = setup(true, true, ByteOrder::Little);
    join(&mut app, &mut client);
    let payload = encode_actor_bunch_body(
        &ActorBunchBody {
            spawn: None,
            blocks: vec![ContentBlock {
                has_rep_layout: false,
                target: BlockTarget::SubObject {
                    object: NetGuidRef::plain(44),
                    class: SubObjectClass::ClientSent,
                },
                payload: fields_payload(&[field(3, ByteOrder::Little)]),
            }],
        },
        &codec(ByteOrder::Little),
    )
    .unwrap();
    let first = client.sequencer.initial_reliable().1 + 1;
    send(&mut app, &mut client, actor(CONTROLLER, first, payload));
    assert_eq!(slot(&app, &client), None);
    send(
        &mut app,
        &mut client,
        actor(
            CONTROLLER,
            first + 1,
            body(&[field(3, ByteOrder::Little)], ByteOrder::Little),
        ),
    );
    assert_eq!(slot(&app, &client), Some(3));
}

#[test]
fn character_slots_are_isolated_between_peers_and_replacement_sessions() {
    let (mut app, mut first_client) = setup(true, true, ByteOrder::Big);
    join(&mut app, &mut first_client);
    let first = first_client.sequencer.initial_reliable().1 + 1;
    send(
        &mut app,
        &mut first_client,
        actor(
            CONTROLLER,
            first,
            body(&[field(3, ByteOrder::Big)], ByteOrder::Big),
        ),
    );
    let lab_address = app.world().resource::<PeerLab>().local_addr().unwrap();
    let version = app
        .world()
        .resource::<PeerLab>()
        .config()
        .local_network_version
        .unwrap();
    let mut second = Client::connect_version(&mut app, lab_address, ByteOrder::Little, 1, version);
    join(&mut app, &mut second);
    assert_eq!(slot(&app, &second), None);
    let first = second.sequencer.initial_reliable().1 + 1;
    send(
        &mut app,
        &mut second,
        actor(
            CONTROLLER,
            first,
            body(&[field(8, ByteOrder::Little)], ByteOrder::Little),
        ),
    );
    assert_eq!(slot(&app, &first_client), Some(3));
    assert_eq!(slot(&app, &second), Some(8));
    first_client.disconnect(&mut app);
    assert_eq!(slot(&app, &first_client), None);
    let mut replacement =
        Client::connect_version(&mut app, lab_address, ByteOrder::Little, 0, version);
    join(&mut app, &mut replacement);
    assert_eq!(slot(&app, &replacement), None);
    let first = replacement.sequencer.initial_reliable().1 + 1;
    send(
        &mut app,
        &mut replacement,
        actor(
            CONTROLLER,
            first,
            body(&[field(4, ByteOrder::Little)], ByteOrder::Little),
        ),
    );
    assert_eq!(slot(&app, &replacement), Some(4));
    // A new handshake on the very same address replaces the session, clearing
    // its slot and login state rather than inheriting either from the old peer.
    replacement
        .socket
        .send(&icarus_bevy::experimental::handshake::initial_request())
        .unwrap();
    app.update();
    let challenge = drain(&replacement.socket).pop().expect("fresh challenge");
    replacement.socket.send(&challenge).unwrap();
    app.update();
    assert!(!drain(&replacement.socket).is_empty());
    assert_eq!(slot(&app, &replacement), None);
    assert_eq!(slot(&app, &second), Some(8));
    assert_eq!(app.world().resource::<PeerLab>().stats().peers_replaced, 1);
}

#[test]
fn initialisation_rpc_absent_parameter_uses_zero_and_consumes_exactly() {
    assert_eq!(
        icarus_bevy::experimental::rep_rpc::player_controller_rpc_parameters(BEGIN),
        Some(vec![RpcParameter::Value(vec![RepCommand::Int])])
    );
    let mut absent = BitWriter::new();
    absent.write_bit(false);
    let payload = Payload::from_writer(absent);
    assert_eq!(
        parse_begin_player_initialisation(&payload, &rep_codec(ByteOrder::Little)).unwrap(),
        0
    );
    for order in [ByteOrder::Little, ByteOrder::Big] {
        let field = field(i32::MIN, order);
        assert_eq!(field.payload.bit_len(), 33);
        assert_eq!(
            parse_begin_player_initialisation(&field.payload, &rep_codec(order)).unwrap(),
            i32::MIN
        );
    }
}

fn requests(client: &Client, replies: &[Vec<u8>]) -> Vec<(u32, FieldHeader)> {
    use icarus_bevy::experimental::{
        actor_channel::read_actor_bunch_body,
        packet::read_packet,
        rep_rpc::{BACKEND_PROXY_FIELD_INDEX_BOUND, read_field_header},
    };
    let mut requests = Vec::new();
    for bytes in replies {
        for bunch in read_packet(bytes, &client.config()).unwrap().bunches {
            if bunch.header.channel_index != CONTROLLER || bunch.header.open {
                continue;
            }
            assert!(bunch.header.reliable);
            let body =
                read_actor_bunch_body(&mut bunch.payload.reader(), false, &codec(client.order))
                    .unwrap();
            for block in body.blocks {
                let BlockTarget::SubObject { object, .. } = block.target else {
                    continue;
                };
                assert!(!block.has_rep_layout);
                let mut reader = block.payload.reader();
                while !reader.at_end() {
                    requests.push((
                        object.guid,
                        read_field_header(&mut reader, BACKEND_PROXY_FIELD_INDEX_BOUND, 65_536)
                            .unwrap(),
                    ));
                }
            }
        }
    }
    requests
}

fn begin_profile(app: &mut App, client: &mut Client, sequence: u32) -> u32 {
    use icarus_bevy::experimental::rep_rpc::CLIENT_REQUEST_GET_USER_PROFILE;
    let replies = send(
        app,
        client,
        actor(
            CONTROLLER,
            sequence,
            body(&[field(3, client.order)], client.order),
        ),
    );
    let requests = requests(client, &replies);
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].1.index, CLIENT_REQUEST_GET_USER_PROFILE);
    assert_eq!(requests[0].1.payload.bit_len(), 0);
    requests[0].0
}

fn response_field(character: bool, success: bool, slot: i32, order: ByteOrder) -> FieldHeader {
    use icarus_bevy::experimental::rep_rpc::{
        RESPONSE_GET_CHARACTER_PROFILE, RESPONSE_GET_USER_PROFILE,
        backend_profile_response_parameters,
    };
    let mut items = vec![RpcItem::Value(RepValue::Bool(success))];
    items.extend(if character {
        profiles::character(slot).rpc
    } else {
        profiles::user().rpc
    });
    FieldHeader {
        index: if character {
            RESPONSE_GET_CHARACTER_PROFILE
        } else {
            RESPONSE_GET_USER_PROFILE
        },
        payload: encode_rpc_payload(
            &backend_profile_response_parameters(character),
            &[RpcArgument::Sent(items)],
            &rep_codec(order),
        )
        .unwrap(),
    }
}

fn proxy_body(proxy: u32, fields: &[FieldHeader], order: ByteOrder) -> Payload {
    use icarus_bevy::experimental::rep_rpc::BACKEND_PROXY_FIELD_INDEX_BOUND;
    subobject_body(proxy, fields, BACKEND_PROXY_FIELD_INDEX_BOUND, order)
}

fn subobject_body(object: u32, fields: &[FieldHeader], bound: u32, order: ByteOrder) -> Payload {
    let mut writer = BitWriter::new();
    for field in fields {
        write_field_header(&mut writer, field, bound).unwrap();
    }
    encode_actor_bunch_body(
        &ActorBunchBody {
            spawn: None,
            blocks: vec![ContentBlock {
                has_rep_layout: false,
                target: BlockTarget::SubObject {
                    object: NetGuidRef::plain(object),
                    class: SubObjectClass::ClientSent,
                },
                payload: Payload::from_writer(writer),
            }],
        },
        &codec(order),
    )
    .unwrap()
}

/// Decode the three follow-up calls, asserting they occupy one reliable step
/// and retain their order even though PlayerData appears in two blocks.
fn completion_requests(client: &Client, replies: &[Vec<u8>]) -> [(u32, FieldHeader); 3] {
    use icarus_bevy::experimental::{
        actor_channel::read_actor_bunch_body,
        packet::read_packet,
        rep_rpc::{
            BESTIARY_MANAGER_FIELD_INDEX_BOUND, PLAYER_DATA_FIELD_INDEX_BOUND, read_field_header,
        },
    };
    let mut steps = Vec::new();
    for bytes in replies {
        for bunch in read_packet(bytes, &client.config()).unwrap().bunches {
            if bunch.header.channel_index != CONTROLLER || bunch.header.open {
                continue;
            }
            assert!(bunch.header.reliable && !bunch.header.partial);
            let body =
                read_actor_bunch_body(&mut bunch.payload.reader(), false, &codec(client.order))
                    .unwrap();
            assert_eq!(body.blocks.len(), 3);
            for (block, bound) in body.blocks.iter().zip([
                PLAYER_DATA_FIELD_INDEX_BOUND,
                BESTIARY_MANAGER_FIELD_INDEX_BOUND,
                PLAYER_DATA_FIELD_INDEX_BOUND,
            ]) {
                let BlockTarget::SubObject { object, .. } = &block.target else {
                    panic!("subobject");
                };
                assert!(!block.has_rep_layout);
                let mut reader = block.payload.reader();
                steps.push((
                    object.guid,
                    read_field_header(&mut reader, bound, 65_536).unwrap(),
                ));
                assert!(reader.at_end());
            }
        }
    }
    let steps: [_; 3] = steps.try_into().unwrap();
    assert_eq!(steps[0].0, steps[2].0);
    assert_ne!(steps[0].0, steps[1].0);
    assert_eq!(
        [steps[0].1.index, steps[1].1.index, steps[2].1.index],
        [16, 4, 2]
    );
    steps
}

fn answer_initialisation(
    app: &mut App,
    client: &mut Client,
    sequence: u32,
    object: u32,
    bestiary: bool,
) -> Vec<Vec<u8>> {
    use icarus_bevy::experimental::rep_rpc::{
        BESTIARY_MANAGER_FIELD_INDEX_BOUND, PLAYER_DATA_FIELD_INDEX_BOUND,
        SERVER_RECEIVE_BESTIARY_DATA, SERVER_RECEIVE_LOADOUT,
    };
    // Deliberately opaque and not byte-aligned: the native receiver must skip it.
    let mut writer = BitWriter::new();
    for bit in [true, false, true, true, false] {
        writer.write_bit(bit);
    }
    let field = FieldHeader {
        index: if bestiary {
            SERVER_RECEIVE_BESTIARY_DATA
        } else {
            SERVER_RECEIVE_LOADOUT
        },
        payload: Payload::from_writer(writer),
    };
    let payload = subobject_body(
        object,
        &[field],
        if bestiary {
            BESTIARY_MANAGER_FIELD_INDEX_BOUND
        } else {
            PLAYER_DATA_FIELD_INDEX_BOUND
        },
        client.order,
    );
    send(app, client, actor(CONTROLLER, sequence, payload))
}

fn respond(
    app: &mut App,
    client: &mut Client,
    sequence: u32,
    proxy: u32,
    character: bool,
    success: bool,
) -> Vec<Vec<u8>> {
    send(
        app,
        client,
        actor(
            CONTROLLER,
            sequence,
            proxy_body(
                proxy,
                &[response_field(character, success, 7, client.order)],
                client.order,
            ),
        ),
    )
}

fn assert_character_request(client: &Client, replies: &[Vec<u8>], proxy: u32) {
    use icarus_bevy::experimental::rep_rpc::{
        CLIENT_REQUEST_GET_CHARACTER_PROFILE, parse_rpc_payload,
    };
    let requests = requests(client, replies);
    assert_eq!(requests.len(), 1);
    assert_eq!(
        (requests[0].0, requests[0].1.index),
        (proxy, CLIENT_REQUEST_GET_CHARACTER_PROFILE)
    );
    assert_eq!(
        parse_rpc_payload(
            &requests[0].1.payload,
            &[RpcParameter::Value(vec![RepCommand::Int])],
            &rep_codec(client.order)
        )
        .unwrap(),
        [RpcArgument::Sent(vec![RpcItem::Value(RepValue::Int(3))])]
    );
}

fn profile_updates(
    client: &Client,
    replies: &[Vec<u8>],
) -> Vec<icarus_bevy::experimental::rep_layout::PropertyStream> {
    use icarus_bevy::experimental::{
        actor_channel::read_actor_bunch_body, packet::read_packet, rep_layout::read_rep_properties,
        session_open::PLAYER_STATE_CHANNEL_INDEX,
    };
    let release = ReleaseProfile::load(
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("config/release.toml"),
    )
    .unwrap();
    let layout = release
        .player_state_open_plan(PeerLabAdmission::default().map_url)
        .unwrap()
        .unwrap()
        .layout;
    let mut updates = Vec::new();
    for bytes in replies {
        for bunch in read_packet(bytes, &client.config()).unwrap().bunches {
            if bunch.header.channel_index != PLAYER_STATE_CHANNEL_INDEX || bunch.header.open {
                continue;
            }
            assert!(bunch.header.reliable && !bunch.header.partial);
            let body =
                read_actor_bunch_body(&mut bunch.payload.reader(), false, &codec(client.order))
                    .unwrap();
            assert_eq!(body.blocks.len(), 1);
            assert_eq!(body.blocks[0].target, BlockTarget::Actor);
            assert!(body.blocks[0].has_rep_layout);
            let mut reader = body.blocks[0].payload.reader();
            updates
                .push(read_rep_properties(&mut reader, &layout, &rep_codec(client.order)).unwrap());
            assert!(reader.at_end());
        }
    }
    updates
}

#[test]
fn profile_exchange_replicates_all_handles_verbatim_in_both_byte_orders() {
    use icarus_bevy::experimental::control_messages::ArchiveText;
    for order in [ByteOrder::Little, ByteOrder::Big] {
        let (mut app, mut client) = setup(true, true, order);
        enable_pawn(&mut app);
        join(&mut app, &mut client);
        let first = client.sequencer.initial_reliable().1 + 1;
        let proxy = begin_profile(&mut app, &mut client, first);
        let replies = respond(&mut app, &mut client, first + 1, proxy, false, true);
        assert_character_request(&client, &replies, proxy);
        let mut updates = profile_updates(&client, &replies);
        assert_eq!(
            updates[0]
                .entries
                .iter()
                .map(|entry| entry.handle)
                .collect::<Vec<_>>(),
            (27..=32).collect::<Vec<_>>()
        );
        let replies = respond(&mut app, &mut client, first + 2, proxy, true, true);
        updates.extend(profile_updates(&client, &replies));
        assert_eq!(
            updates[1]
                .entries
                .iter()
                .map(|entry| entry.handle)
                .collect::<Vec<_>>(),
            (33..=76).collect::<Vec<_>>()
        );
        let followups = completion_requests(&client, &replies);
        // The two answer orders both complete, only on the second answer.
        let answers = if matches!(order, ByteOrder::Little) {
            [(followups[0].0, false), (followups[1].0, true)]
        } else {
            [(followups[1].0, true), (followups[0].0, false)]
        };
        let replies =
            answer_initialisation(&mut app, &mut client, first + 3, answers[0].0, answers[0].1);
        assert!(profile_updates(&client, &replies).is_empty());
        let replies =
            answer_initialisation(&mut app, &mut client, first + 4, answers[1].0, answers[1].1);
        updates.extend(profile_updates(&client, &replies));
        assert_eq!(updates.len(), 3);
        assert_eq!(
            updates[2]
                .entries
                .iter()
                .map(|entry| entry.handle)
                .collect::<Vec<_>>(),
            [77, 78]
        );
        assert_eq!(
            updates
                .iter()
                .flat_map(|update| &update.entries)
                .map(|entry| entry.handle)
                .collect::<Vec<_>>(),
            (27..=78).collect::<Vec<_>>()
        );
        let mut expected = profiles::user().properties;
        expected.extend(profiles::character(7).properties);
        expected.extend([
            RepValue::Str(ArchiveText::text("0123456789ABCDEF7654").unwrap()),
            RepValue::Int(7),
        ]);
        assert_eq!(
            updates
                .iter()
                .flat_map(|update| &update.entries)
                .map(|entry| entry.value.clone())
                .collect::<Vec<_>>(),
            expected
        );
        // Requested slot is retained independently of the profile's ChrSlot.
        assert_eq!(slot(&app, &client), Some(3));
        let counts = app.world().resource::<PeerLab>().session_counts();
        assert_eq!(
            (
                counts.profile_user_requests_sent,
                counts.profile_character_requests_sent,
                counts.profile_user_responses_received,
                counts.profile_character_responses_received,
                counts.profile_updates_sent
            ),
            (1, 1, 1, 1, 2)
        );
        assert_eq!(
            (
                counts.loadout_requests_sent,
                counts.bestiary_requests_sent,
                counts.associated_prospects_sent,
                counts.loadout_responses_received,
                counts.bestiary_responses_received,
                counts.players_initialised,
                counts.player_character_id_updates_sent
            ),
            (1, 1, 1, 1, 1, 1, 1)
        );
        assert_eq!(counts.initialisation_answers_ignored, 0);
        assert_eq!(counts.profile_exchange_errors, 0);
        let status = serde_json::to_string(&counts).unwrap();
        assert!(!status.contains("authored") && !status.contains("012345"));
        let debug = format!("{updates:?}");
        assert!(
            !debug.contains("authored")
                && !debug.contains("012345")
                && !debug.contains("9876543210")
        );
    }
}

#[test]
fn followup_rpcs_use_profile_slot_dedicated_host_and_current_prospect() {
    use icarus_bevy::experimental::{
        control_messages::ArchiveText,
        game_state_open::{
            GAME_STATE_CHANNEL_INDEX, GameStateManagers, GameStateOpenState, active_prospect_values,
        },
        rep_rpc::{
            add_associated_prospect_parameters, client_prepare_loadout_parameters,
            parse_rpc_payload,
        },
        session_open::ActorKey,
    };
    for order in [ByteOrder::Little, ByteOrder::Big] {
        // No GameState, template-only state, and a newer explicit refresh.
        for mode in 0..3 {
            let (mut app, mut client) = setup(true, true, order);
            let (info, fixture) = profiles::prospect();
            if mode != 0 {
                let release = ReleaseProfile::load(
                    &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("config/release.toml"),
                )
                .unwrap();
                let mut lab = app.world_mut().resource_mut::<PeerLab>();
                lab.enable_game_state_open(
                    release
                        .game_state_open_plan(PeerLabAdmission::default().map_url)
                        .unwrap()
                        .unwrap(),
                    GameStateOpenState {
                        key: ActorKey::Session(8_001),
                        managers: GameStateManagers::default(),
                        values: active_prospect_values(&info).unwrap(),
                        weather: Vec::new(),
                    },
                    GAME_STATE_CHANNEL_INDEX,
                )
                .unwrap();
            }
            join(&mut app, &mut client);
            let first = client.sequencer.initial_reliable().1 + 1;
            let proxy = begin_profile(&mut app, &mut client, first);
            respond(&mut app, &mut client, first + 1, proxy, false, true);
            let mut prospect =
                if mode == 0 {
                    (51..=67)
                        .map(|handle| {
                            match icarus_bevy::experimental::game_state_open::class_default(handle)
                                .unwrap()
                            {
                                RepValue::DynamicArray(_) => RpcItem::Array(Vec::new()),
                                value => RpcItem::Value(value),
                            }
                        })
                        .collect::<Vec<_>>()
                } else {
                    fixture.rpc
                };
            if mode == 2 {
                let mut latest = info;
                latest.lobby_name = "newest".encode_utf16().collect();
                app.world_mut()
                    .resource_mut::<PeerLab>()
                    .refresh_active_prospect(&latest)
                    .unwrap();
                prospect[5] = RpcItem::Value(RepValue::Str(ArchiveText::text("newest").unwrap()));
            }
            let replies = respond(&mut app, &mut client, first + 2, proxy, true, true);
            let calls = completion_requests(&client, &replies);
            let host = vec![
                RpcItem::Value(RepValue::Byte(2)),
                RpcItem::Value(RepValue::Str(ArchiveText::new(&[]).unwrap())),
                RpcItem::Value(RepValue::Str(ArchiveText::new(&[]).unwrap())),
                RpcItem::Value(RepValue::Str(ArchiveText::new(&[]).unwrap())),
            ];
            let slot = RpcArgument::Sent(vec![RpcItem::Value(RepValue::Int(7))]);
            // 2 presence bits + 32-bit slot + 2-bit enum byte + 3 empty strings.
            assert_eq!(calls[0].1.payload.bit_len(), 132);
            assert_eq!(
                parse_rpc_payload(
                    &calls[0].1.payload,
                    &client_prepare_loadout_parameters(),
                    &rep_codec(order)
                )
                .unwrap(),
                [slot.clone(), RpcArgument::Sent(host.clone())]
            );
            assert_eq!(calls[1].1.payload.bit_len(), 0);
            prospect.extend(host);
            assert_eq!(
                parse_rpc_payload(
                    &calls[2].1.payload,
                    &add_associated_prospect_parameters(),
                    &rep_codec(order)
                )
                .unwrap(),
                [slot, RpcArgument::Sent(prospect)]
            );
        }
    }
}

#[test]
fn initialisation_answers_before_requests_and_repeats_are_counted_and_ignored() {
    let (mut app, mut client) = setup(true, true, ByteOrder::Little);
    enable_pawn(&mut app);
    let opened = client.join(&mut app);
    client.settle(&mut app, &opened);
    let [proxy, player_data, bestiary] = controller_components(&client, &opened);
    let first = client.sequencer.initial_reliable().1 + 1;
    for (offset, object, bestiary_answer) in [(0, player_data, false), (1, bestiary, true)] {
        let replies = answer_initialisation(
            &mut app,
            &mut client,
            first + offset,
            object,
            bestiary_answer,
        );
        assert!(profile_updates(&client, &replies).is_empty());
    }
    assert_eq!(begin_profile(&mut app, &mut client, first + 2), proxy);
    respond(&mut app, &mut client, first + 3, proxy, false, true);
    let replies = respond(&mut app, &mut client, first + 4, proxy, true, true);
    let requests = completion_requests(&client, &replies);
    assert_eq!((requests[0].0, requests[1].0), (player_data, bestiary));
    for offset in [5, 6] {
        let replies =
            answer_initialisation(&mut app, &mut client, first + offset, player_data, false);
        assert!(profile_updates(&client, &replies).is_empty());
    }
    let replies = answer_initialisation(&mut app, &mut client, first + 7, bestiary, true);
    assert_eq!(profile_updates(&client, &replies).len(), 1);
    for (offset, object, bestiary_answer) in [(8, player_data, false), (9, bestiary, true)] {
        let replies = answer_initialisation(
            &mut app,
            &mut client,
            first + offset,
            object,
            bestiary_answer,
        );
        assert!(profile_updates(&client, &replies).is_empty());
    }
    let counts = app.world().resource::<PeerLab>().session_counts();
    assert_eq!(
        (
            counts.loadout_responses_received,
            counts.bestiary_responses_received,
            counts.initialisation_answers_ignored
        ),
        (4, 3, 5)
    );
    assert_eq!(
        (
            counts.players_initialised,
            counts.player_character_id_updates_sent
        ),
        (1, 1)
    );
}

#[test]
fn initialisation_waits_for_both_answers_and_an_open_pawn() {
    for (pawn, answers) in [
        (true, vec![false]),
        (true, vec![true]),
        (false, vec![false, true]),
    ] {
        let (mut app, mut client) = setup(true, true, ByteOrder::Little);
        if pawn {
            enable_pawn(&mut app);
        }
        join(&mut app, &mut client);
        let first = client.sequencer.initial_reliable().1 + 1;
        let proxy = begin_profile(&mut app, &mut client, first);
        respond(&mut app, &mut client, first + 1, proxy, false, true);
        let replies = respond(&mut app, &mut client, first + 2, proxy, true, true);
        let requests = completion_requests(&client, &replies);
        for (offset, bestiary) in answers.into_iter().enumerate() {
            let replies = answer_initialisation(
                &mut app,
                &mut client,
                first + 3 + offset as u32,
                requests[usize::from(bestiary)].0,
                bestiary,
            );
            assert!(profile_updates(&client, &replies).is_empty());
        }
        for _ in 0..8 {
            app.update();
        }
        let lab = app.world().resource::<PeerLab>();
        assert!(lab.has_peer(address(&client)));
        assert_eq!(
            (
                lab.stats().players_initialised,
                lab.stats().player_character_id_updates_sent,
                lab.stats().returning_pawns_restored,
                lab.stats().profile_players_failed
            ),
            (0, 0, 0, 0)
        );
        assert_eq!(
            (
                lab.stats().loadout_requests_sent,
                lab.stats().bestiary_requests_sent
            ),
            (1, 1)
        );
    }
}

#[test]
fn initialisation_payloads_skip_exactly_to_following_fields_on_each_component() {
    use icarus_bevy::experimental::rep_rpc::{
        BESTIARY_MANAGER_FIELD_INDEX_BOUND, PLAYER_DATA_FIELD_INDEX_BOUND,
    };
    for order in [ByteOrder::Little, ByteOrder::Big] {
        let (mut app, mut client) = setup(true, true, order);
        enable_pawn(&mut app);
        join(&mut app, &mut client);
        let first = client.sequencer.initial_reliable().1 + 1;
        let proxy = begin_profile(&mut app, &mut client, first);
        respond(&mut app, &mut client, first + 1, proxy, false, true);
        let replies = respond(&mut app, &mut client, first + 2, proxy, true, true);
        let requests = completion_requests(&client, &replies);
        for (offset, request, bound, answer, unknown) in [
            (3, &requests[0], PLAYER_DATA_FIELD_INDEX_BOUND, 24, 25),
            (4, &requests[1], BESTIARY_MANAGER_FIELD_INDEX_BOUND, 6, 5),
        ] {
            let mut bits = BitWriter::new();
            for bit in [true, false, false, true, false, true, true] {
                bits.write_bit(bit);
            }
            let fields = [
                FieldHeader {
                    index: answer,
                    payload: Payload::from_writer(bits),
                },
                FieldHeader {
                    index: unknown,
                    payload: Payload::from_writer(BitWriter::new()),
                },
                FieldHeader {
                    index: answer,
                    payload: Payload::from_writer(BitWriter::new()),
                },
            ];
            let replies = send(
                &mut app,
                &mut client,
                actor(
                    CONTROLLER,
                    first + offset,
                    subobject_body(request.0, &fields, bound, order),
                ),
            );
            assert_eq!(
                profile_updates(&client, &replies).len(),
                usize::from(offset == 4)
            );
        }
        let stats = app.world().resource::<PeerLab>().stats();
        assert_eq!(
            (
                stats.loadout_responses_received,
                stats.bestiary_responses_received,
                stats.controller_fields_ignored,
                stats.initialisation_answers_ignored
            ),
            (2, 2, 2, 2)
        );
        assert_eq!(
            (stats.players_initialised, stats.actor_bunches_malformed),
            (1, 0)
        );
    }
}

#[test]
fn malformed_trailing_block_prevents_both_initialisation_answers_from_applying() {
    use icarus_bevy::experimental::{
        actor_channel::read_client_actor_bunch_body,
        rep_rpc::{BESTIARY_MANAGER_FIELD_INDEX_BOUND, PLAYER_DATA_FIELD_INDEX_BOUND},
    };
    let (mut app, mut client) = setup(true, true, ByteOrder::Little);
    enable_pawn(&mut app);
    join(&mut app, &mut client);
    let first = client.sequencer.initial_reliable().1 + 1;
    let proxy = begin_profile(&mut app, &mut client, first);
    respond(&mut app, &mut client, first + 1, proxy, false, true);
    let replies = respond(&mut app, &mut client, first + 2, proxy, true, true);
    let requests = completion_requests(&client, &replies);
    let mut blocks = Vec::new();
    for (request, bound, index) in [
        (&requests[0], PLAYER_DATA_FIELD_INDEX_BOUND, 24),
        (&requests[1], BESTIARY_MANAGER_FIELD_INDEX_BOUND, 6),
    ] {
        let payload = subobject_body(
            request.0,
            &[FieldHeader {
                index,
                payload: Payload::from_writer(BitWriter::new()),
            }],
            bound,
            client.order,
        );
        blocks.extend(
            read_client_actor_bunch_body(&mut payload.reader(), &codec(client.order))
                .unwrap()
                .blocks,
        );
    }
    let valid = encode_actor_bunch_body(
        &ActorBunchBody {
            spawn: None,
            blocks: blocks.clone(),
        },
        &codec(client.order),
    )
    .unwrap();
    let mut bits = BitWriter::new();
    bits.write_bit(true);
    blocks.push(ContentBlock {
        has_rep_layout: false,
        target: BlockTarget::Actor,
        payload: Payload::from_writer(bits),
    });
    let malformed = encode_actor_bunch_body(
        &ActorBunchBody {
            spawn: None,
            blocks,
        },
        &codec(client.order),
    )
    .unwrap();
    let replies = send(
        &mut app,
        &mut client,
        actor(CONTROLLER, first + 3, malformed),
    );
    assert!(profile_updates(&client, &replies).is_empty());
    let stats = app.world().resource::<PeerLab>().stats();
    assert_eq!(
        (
            stats.actor_bunches_malformed,
            stats.loadout_responses_received,
            stats.bestiary_responses_received,
            stats.players_initialised
        ),
        (1, 0, 0, 0)
    );
    // Malformed input consumed this sequence: a corrected duplicate cannot apply.
    let replies = send(
        &mut app,
        &mut client,
        actor(CONTROLLER, first + 3, valid.clone()),
    );
    assert!(profile_updates(&client, &replies).is_empty());
    let replies = send(&mut app, &mut client, actor(CONTROLLER, first + 4, valid));
    assert_eq!(profile_updates(&client, &replies).len(), 1);
    let stats = app.world().resource::<PeerLab>().stats();
    assert_eq!(
        (
            stats.actor_duplicate_reliable,
            stats.loadout_responses_received,
            stats.bestiary_responses_received,
            stats.players_initialised
        ),
        (1, 1, 1, 1)
    );
}

#[test]
fn profile_failure_retries_are_separate_and_user_retry_can_succeed() {
    let (mut app, mut client) = setup(true, true, ByteOrder::Little);
    join(&mut app, &mut client);
    let first = client.sequencer.initial_reliable().1 + 1;
    let proxy = begin_profile(&mut app, &mut client, first);
    let replies = respond(&mut app, &mut client, first + 1, proxy, false, false);
    assert_eq!(requests(&client, &replies)[0].1.index, 25);
    let replies = respond(&mut app, &mut client, first + 2, proxy, false, true);
    assert_character_request(&client, &replies, proxy);
    for offset in 3..5 {
        let replies = respond(&mut app, &mut client, first + offset, proxy, true, false);
        assert_character_request(&client, &replies, proxy);
    }
    let replies = respond(&mut app, &mut client, first + 5, proxy, true, true);
    assert_eq!(profile_updates(&client, &replies).len(), 1);
    let stats = app.world().resource::<PeerLab>().stats();
    assert_eq!(
        (
            stats.profile_user_failures,
            stats.profile_character_failures
        ),
        (1, 2)
    );
    assert_eq!(
        (
            stats.profile_user_requests_sent,
            stats.profile_character_requests_sent
        ),
        (2, 3)
    );
    assert_eq!(stats.profile_players_failed, 0);
}

#[test]
fn three_profile_failures_remove_the_session_for_each_kind() {
    for character in [false, true] {
        let (mut app, mut client) = setup(true, true, ByteOrder::Little);
        join(&mut app, &mut client);
        let mut sequence = client.sequencer.initial_reliable().1 + 1;
        let proxy = begin_profile(&mut app, &mut client, sequence);
        sequence += 1;
        if character {
            respond(&mut app, &mut client, sequence, proxy, false, true);
            sequence += 1;
        }
        for _ in 0..2 {
            respond(&mut app, &mut client, sequence, proxy, character, false);
            sequence += 1;
        }
        client.send(vec![actor(
            CONTROLLER,
            sequence,
            proxy_body(
                proxy,
                &[response_field(character, false, 7, client.order)],
                client.order,
            ),
        )]);
        app.update();
        let lab = app.world().resource::<PeerLab>();
        assert!(!lab.has_peer(address(&client)));
        assert_eq!(
            (lab.stats().profile_players_failed, lab.stats().peers_failed),
            (1, 1)
        );
        assert_eq!(lab.stats().profile_updates_sent, u64::from(character));
        assert_eq!(
            if character {
                lab.stats().profile_character_failures
            } else {
                lab.stats().profile_user_failures
            },
            3
        );
    }
}

fn controller_components(client: &Client, opened: &[Vec<u8>]) -> [u32; 3] {
    // Actual component references from controller handles 22..24.
    use icarus_bevy::experimental::{
        actor_channel::read_player_controller_open_bunch_body, packet::read_packet,
        rep_layout::read_rep_properties,
    };
    let release = ReleaseProfile::load(
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("config/release.toml"),
    )
    .unwrap();
    let layout = release
        .player_controller_open_plan(PeerLabAdmission::default().map_url, [0.0; 3])
        .unwrap()
        .unwrap()
        .actor_layout;
    let mut components = [None; 3];
    for bytes in opened {
        for bunch in read_packet(bytes, &client.config()).unwrap().bunches {
            if bunch.header.channel_index != CONTROLLER
                || bunch.header.has_package_map_exports
                || (!bunch.header.open && !bunch.header.partial_final)
            {
                continue;
            }
            let body = read_player_controller_open_bunch_body(
                &mut bunch.payload.reader(),
                &codec(client.order),
            )
            .unwrap();
            let values = read_rep_properties(
                &mut body.blocks[0].payload.reader(),
                &layout,
                &rep_codec(client.order),
            )
            .unwrap();
            for entry in values.entries {
                if let RepValue::Object(id) = entry.value
                    && (22..=24).contains(&entry.handle)
                {
                    components[(entry.handle - 22) as usize] = Some(id.guid);
                }
            }
        }
    }
    components.map(Option::unwrap)
}

#[test]
fn profile_responses_without_matching_requests_are_ignored() {
    let (mut app, mut client) = setup(true, true, ByteOrder::Little);
    let opened = client.join(&mut app);
    client.settle(&mut app, &opened);
    let [proxy, _, _] = controller_components(&client, &opened);
    let first = client.sequencer.initial_reliable().1 + 1;
    respond(&mut app, &mut client, first, proxy, false, true);
    respond(&mut app, &mut client, first + 1, proxy, true, true);
    assert_eq!(begin_profile(&mut app, &mut client, first + 2), proxy);
    respond(&mut app, &mut client, first + 3, proxy, true, true);
    respond(&mut app, &mut client, first + 4, proxy, false, true);
    respond(&mut app, &mut client, first + 5, proxy, false, true);
    respond(&mut app, &mut client, first + 6, proxy, true, true);
    for (offset, character) in [(7, false), (8, true)] {
        let replies = respond(
            &mut app,
            &mut client,
            first + offset,
            proxy,
            character,
            false,
        );
        assert!(requests(&client, &replies).is_empty());
        assert!(profile_updates(&client, &replies).is_empty());
    }
    let stats = app.world().resource::<PeerLab>().stats();
    assert_eq!(stats.profile_responses_ignored, 6);
    assert_eq!(
        (
            stats.profile_updates_sent,
            stats.profile_user_failures,
            stats.profile_character_failures
        ),
        (2, 0, 0)
    );
}

#[test]
fn malformed_profile_bunch_is_atomic_and_consumes_its_reliable_sequence() {
    use icarus_bevy::experimental::rep_rpc::RESPONSE_GET_USER_PROFILE;
    let (mut app, mut client) = setup(true, true, ByteOrder::Little);
    join(&mut app, &mut client);
    let first = client.sequencer.initial_reliable().1 + 1;
    let proxy = begin_profile(&mut app, &mut client, first);
    // A valid success followed by a truncated struct must not advance phase.
    let mut truncated = BitWriter::new();
    truncated.write_bit(true);
    let malformed = FieldHeader {
        index: RESPONSE_GET_USER_PROFILE,
        payload: Payload::from_writer(truncated),
    };
    let replies = send(
        &mut app,
        &mut client,
        actor(
            CONTROLLER,
            first + 1,
            proxy_body(
                proxy,
                &[response_field(false, true, 7, ByteOrder::Little), malformed],
                ByteOrder::Little,
            ),
        ),
    );
    assert!(requests(&client, &replies).is_empty());
    let stats = app.world().resource::<PeerLab>().stats();
    assert_eq!(stats.actor_bunches_malformed, 1);
    assert_eq!(stats.profile_user_responses_received, 0);
    assert_eq!(stats.profile_character_requests_sent, 0);
    let replies = respond(&mut app, &mut client, first + 2, proxy, false, true);
    assert_character_request(&client, &replies, proxy);
    // Likewise a successful character reply cannot partly apply on trailing junk.
    let mut malformed = response_field(true, true, 7, ByteOrder::Little);
    let mut bits = BitWriter::new();
    let mut reader = malformed.payload.reader();
    while !reader.at_end() {
        bits.write_bit(reader.read_bit());
    }
    bits.write_bit(false);
    malformed.payload = Payload::from_writer(bits);
    let replies = send(
        &mut app,
        &mut client,
        actor(
            CONTROLLER,
            first + 3,
            proxy_body(proxy, &[malformed], ByteOrder::Little),
        ),
    );
    assert!(profile_updates(&client, &replies).is_empty());
    assert_eq!(
        app.world()
            .resource::<PeerLab>()
            .stats()
            .profile_character_responses_received,
        0
    );
    let replies = respond(&mut app, &mut client, first + 4, proxy, true, true);
    assert_eq!(profile_updates(&client, &replies).len(), 1);
    assert_eq!(
        app.world()
            .resource::<PeerLab>()
            .stats()
            .actor_bunches_malformed,
        2
    );
}

#[test]
fn larger_profiles_replicate_as_separate_reliable_player_state_updates_in_parts() {
    use icarus_bevy::experimental::{
        actor_channel::read_actor_bunch_body,
        packet::read_packet,
        rep_layout::{PropertyStream, RepArray, RepEntry, read_rep_properties},
        rep_rpc::backend_profile_response_parameters,
        sequencing::{PartialBunchReassembler, PartialReceive},
        session_open::PLAYER_STATE_CHANNEL_INDEX,
    };
    let (mut app, mut client) = setup(true, true, ByteOrder::Little);
    join(&mut app, &mut client);
    let first = client.sequencer.initial_reliable().1 + 1;
    let proxy = begin_profile(&mut app, &mut client, first);
    for (sequence, character) in (first + 1..).zip([false, true]) {
        let mut fixture = if character {
            profiles::character(7)
        } else {
            profiles::user()
        };
        // A property array carries per-element handles that RPC arrays omit.
        // Keep the incoming RPC within one packet; the outgoing update splits.
        let (array, count) = if character {
            (8, 150_u16)
        } else {
            (2, 180_u16)
        };
        fixture.rpc[array] = RpcItem::Array(
            (0..count)
                .map(|value| vec![RpcItem::Value(RepValue::Int(i32::from(value)))])
                .collect(),
        );
        fixture.properties[array] = RepValue::DynamicArray(RepArray {
            count,
            stream: PropertyStream {
                entries: (0..count)
                    .map(|value| RepEntry {
                        handle: u32::from(value) + 1,
                        checksum: None,
                        value: RepValue::Int(i32::from(value)),
                    })
                    .collect(),
                terminator_checksum: None,
            },
        });
        let expected = fixture.properties;
        let mut items = vec![RpcItem::Value(RepValue::Bool(true))];
        items.extend(fixture.rpc);
        let field = FieldHeader {
            index: if character { 108 } else { 110 },
            payload: encode_rpc_payload(
                &backend_profile_response_parameters(character),
                &[RpcArgument::Sent(items)],
                &rep_codec(client.order),
            )
            .unwrap(),
        };
        let payload = proxy_body(proxy, &[field], client.order);
        let reply = send(&mut app, &mut client, actor(CONTROLLER, sequence, payload));
        let mut reassembler = PartialBunchReassembler::new();
        let mut complete = None;
        let mut parts = 0;
        for bytes in reply {
            for bunch in read_packet(&bytes, &client.config()).unwrap().bunches {
                if bunch.header.channel_index != PLAYER_STATE_CHANNEL_INDEX {
                    continue;
                }
                assert!(bunch.header.partial && bunch.header.reliable && !bunch.header.open);
                parts += 1;
                if let PartialReceive::Complete { bunch, .. } = reassembler
                    .receive_part(parts, &bunch.header, &bunch.payload, Some(65_536))
                    .unwrap()
                {
                    complete = Some(bunch);
                }
            }
        }
        assert!(parts > 1);
        let body = read_actor_bunch_body(
            &mut complete.unwrap().payload.reader(),
            false,
            &codec(client.order),
        )
        .unwrap();
        let release = ReleaseProfile::load(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("config/release.toml"),
        )
        .unwrap();
        let layout = release
            .player_state_open_plan(PeerLabAdmission::default().map_url)
            .unwrap()
            .unwrap()
            .layout;
        let stream = read_rep_properties(
            &mut body.blocks[0].payload.reader(),
            &layout,
            &rep_codec(client.order),
        )
        .unwrap();
        assert_eq!(
            stream
                .entries
                .iter()
                .map(|entry| entry.handle)
                .collect::<Vec<_>>(),
            if character {
                (33..=76).collect::<Vec<_>>()
            } else {
                (27..=32).collect::<Vec<_>>()
            }
        );
        assert_eq!(
            stream
                .entries
                .iter()
                .map(|entry| entry.value.clone())
                .collect::<Vec<_>>(),
            expected
        );
    }
    let stats = app.world().resource::<PeerLab>().stats();
    assert_eq!(
        (stats.profile_updates_sent, stats.profile_exchange_errors),
        (2, 0)
    );
}

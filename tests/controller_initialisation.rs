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
    let profile = ReleaseProfile::load(
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("config/release.toml"),
    )
    .unwrap();
    let admission = PeerLabAdmission::default();
    let map = admission.map_url;
    let mut lab = PeerLab::bind(
        0,
        PeerLabConfig {
            admission: Some(admission),
            ..PeerLabConfig::default().with_release_profile(&profile)
        },
    )
    .unwrap();
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

fn send(app: &mut App, client: &mut Client, bunch: Bunch) {
    client.send(vec![bunch]);
    app.update();
    let replies = drain(&client.socket);
    assert!(!replies.is_empty(), "input remains acknowledged");
    client.accept(&replies);
    let lab = app.world().resource::<PeerLab>();
    assert!(lab.has_peer(address(client)));
    assert_eq!(lab.stats().peers_failed, 0);
    assert_eq!(lab.stats().failures_sent, 0);
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
                    class: SubObjectClass::StablyNamed,
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

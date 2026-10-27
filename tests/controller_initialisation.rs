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
    let mut writer = BitWriter::new();
    for field in fields {
        write_field_header(&mut writer, field, BACKEND_PROXY_FIELD_INDEX_BOUND).unwrap();
    }
    encode_actor_bunch_body(
        &ActorBunchBody {
            spawn: None,
            blocks: vec![ContentBlock {
                has_rep_layout: false,
                target: BlockTarget::SubObject {
                    object: NetGuidRef::plain(proxy),
                    class: SubObjectClass::StablyNamed,
                },
                payload: Payload::from_writer(writer),
            }],
        },
        &codec(order),
    )
    .unwrap()
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
        join(&mut app, &mut client);
        let first = client.sequencer.initial_reliable().1 + 1;
        let proxy = begin_profile(&mut app, &mut client, first);
        let replies = respond(&mut app, &mut client, first + 1, proxy, false, true);
        assert_character_request(&client, &replies, proxy);
        assert!(profile_updates(&client, &replies).is_empty());
        let replies = respond(&mut app, &mut client, first + 2, proxy, true, true);
        let updates = profile_updates(&client, &replies);
        assert_eq!(updates.len(), 1);
        assert_eq!(
            updates[0]
                .entries
                .iter()
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
            updates[0]
                .entries
                .iter()
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
            (1, 1, 1, 1, 1)
        );
        assert_eq!(counts.profile_exchange_errors, 0);
        let status = serde_json::to_string(&counts).unwrap();
        assert!(!status.contains("authored") && !status.contains("012345"));
        let debug = format!("{:?}", updates[0]);
        assert!(
            !debug.contains("authored")
                && !debug.contains("012345")
                && !debug.contains("9876543210")
        );
    }
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
        assert_eq!(lab.stats().profile_updates_sent, 0);
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

#[test]
fn profile_responses_without_matching_requests_are_ignored() {
    let (mut app, mut client) = setup(true, true, ByteOrder::Little);
    let opened = client.join(&mut app);
    client.settle(&mut app, &opened);
    // Obtain the actual proxy reference from controller handle 22.
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
    let mut proxy = None;
    for bytes in opened {
        for bunch in read_packet(&bytes, &client.config()).unwrap().bunches {
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
            proxy = values.entries.iter().find_map(|entry| match &entry.value {
                RepValue::Object(id) if entry.handle == 22 => Some(id.guid),
                _ => None,
            });
        }
    }
    let proxy = proxy.unwrap();
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
        (1, 0, 0)
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
fn larger_profiles_replicate_as_one_reliable_player_state_update_in_parts() {
    use icarus_bevy::experimental::{
        actor_channel::read_actor_bunch_body,
        control_messages::ArchiveText,
        packet::read_packet,
        rep_layout::read_rep_properties,
        rep_rpc::backend_profile_response_parameters,
        sequencing::{PartialBunchReassembler, PartialReceive},
        session_open::PLAYER_STATE_CHANNEL_INDEX,
    };
    let (mut app, mut client) = setup(true, true, ByteOrder::Little);
    join(&mut app, &mut client);
    let first = client.sequencer.initial_reliable().1 + 1;
    let proxy = begin_profile(&mut app, &mut client, first);
    let mut expected = Vec::new();
    let mut reply = Vec::new();
    for (offset, character) in [(1, false), (2, true)] {
        let mut fixture = if character {
            profiles::character(7)
        } else {
            profiles::user()
        };
        let name = RepValue::Str(ArchiveText::text(&"X".repeat(450)).unwrap());
        fixture.properties[0] = name.clone();
        fixture.rpc[0] = RpcItem::Value(name);
        expected.extend(fixture.properties);
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
        reply = send(
            &mut app,
            &mut client,
            actor(CONTROLLER, first + offset, payload),
        );
    }
    expected.extend([
        RepValue::Str(ArchiveText::text("0123456789ABCDEF7654").unwrap()),
        RepValue::Int(7),
    ]);
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
            .map(|entry| entry.value.clone())
            .collect::<Vec<_>>(),
        expected
    );
    let stats = app.world().resource::<PeerLab>().stats();
    assert_eq!(
        (stats.profile_updates_sent, stats.profile_exchange_errors),
        (1, 0)
    );
}

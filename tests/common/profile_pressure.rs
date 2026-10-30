//! Authored profile exchanges under transport pressure, through public APIs.
use super::*;
use icarus_bevy::{
    actor_uids::ActorUidLibrary,
    experimental::{
        actor_channel::{BlockTarget, read_actor_bunch_body},
        packet::{BitWriter, Payload, read_packet},
        packet_lab::DEFAULT_SENT_CAPACITY,
        peer_lab::PeerLabAdmission,
        probe_client::ProbeProfileResponse,
        probe_initialise::{begin_player_initialisation_field, initialise_codec},
        rep_rpc::{
            BACKEND_PROXY_FIELD_INDEX_BOUND, BESTIARY_MANAGER_FIELD_INDEX_BOUND,
            BP_ICARUS_PLAYER_CONTROLLER_SURVIVAL_FIELD_INDEX_BOUND,
            CLIENT_REQUEST_GET_CHARACTER_PROFILE, CLIENT_REQUEST_GET_USER_PROFILE,
            PLAYER_DATA_FIELD_INDEX_BOUND, read_field_header,
        },
        session_open::{
            ActorOpenSpec, PLAYER_CONTROLLER_CHANNEL_INDEX, PathSegment, map_level_chain,
        },
    },
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Step {
    Begin,
    User,
    Character,
    Loadout,
    Bestiary,
}

const STEPS: [Step; 5] = [
    Step::Begin,
    Step::User,
    Step::Character,
    Step::Loadout,
    Step::Bestiary,
];
const PRESSURE_ACTOR: u64 = 42;

struct Fixture {
    app: App,
    client: ProbeClient,
    address: SocketAddr,
    profile: ReleaseProfile,
    connection: u32,
    proxy: Option<u32>,
    player_data: Option<u32>,
    bestiary: Option<u32>,
}

impl Fixture {
    fn new() -> Self {
        let (mut lab, profile) = uid_lab(ActorUidLibrary::with_random(|| 7));
        let map = PeerLabAdmission::default().map_url;
        lab.enable_player_state_open(profile.player_state_open_plan(map).unwrap().unwrap())
            .unwrap();
        // No GameState: its world-time heartbeat would add unrelated traffic.
        let mut app = build_app(&ServerConfig::default(), &WorldSnapshot::default()).unwrap();
        let address = network::install(&mut app, lab).unwrap();
        let version = profile.pin.network_checksum;
        let ((client, _), _) =
            with_server(&mut app, move || join_pinned_open(address, version, 32));
        let connection = app.world().resource::<PeerLab>().joined_connections()[0];
        let mut value = Self {
            app,
            client,
            address,
            profile,
            connection,
            proxy: None,
            player_data: None,
            bestiary: None,
        };
        assert!(
            value
                .app
                .world_mut()
                .resource_mut::<PeerLab>()
                .open_actor(
                    connection,
                    PRESSURE_ACTOR,
                    ActorOpenSpec {
                        actor: 0,
                        archetype: vec![
                            PathSegment::new("Default__ProfilePressure_C", 0x1234_5678, false),
                            PathSegment::new("/Game/Lab/ProfilePressure", 0, false),
                        ],
                        level: map_level_chain(map).unwrap(),
                        location: Some([0.0; 3]),
                        rotation: None,
                        velocity: None,
                        blocks: Vec::new(),
                    },
                    16,
                    value.profile.engine_network_version,
                )
                .is_some()
        );
        value.collect();
        send_friends_list_updated(&mut value.client, value.profile.engine_network_version);
        value.poll_collect();
        value.ack();
        value
    }

    fn collect(&mut self) {
        let replies = self.client.collect(Duration::from_millis(40), 32).unwrap();
        let codec = initialise_codec(self.profile.engine_network_version);
        for reply in replies.replies {
            let packet = read_packet(
                &reply.bytes,
                &PacketConfig {
                    receiver: PacketReceiver::Client,
                    ..probe_config().packet
                },
            )
            .unwrap();
            for bunch in packet.bunches {
                if bunch.header.channel_index != PLAYER_CONTROLLER_CHANNEL_INDEX
                    || bunch.header.open
                    || bunch.header.has_package_map_exports
                {
                    continue;
                }
                let body = read_actor_bunch_body(&mut bunch.payload.reader(), false, &codec.actor)
                    .unwrap();
                let initialise = body.blocks.len() == 3;
                for (index, block) in body.blocks.into_iter().enumerate() {
                    let BlockTarget::SubObject { object, .. } = block.target else {
                        continue;
                    };
                    let bound = if initialise {
                        if index == 1 {
                            BESTIARY_MANAGER_FIELD_INDEX_BOUND
                        } else {
                            PLAYER_DATA_FIELD_INDEX_BOUND
                        }
                    } else {
                        BACKEND_PROXY_FIELD_INDEX_BOUND
                    };
                    let field =
                        read_field_header(&mut block.payload.reader(), bound, 65_536).unwrap();
                    if initialise {
                        match index {
                            0 => {
                                assert_eq!(field.index, 16);
                                self.player_data = Some(object.guid);
                            }
                            1 => {
                                assert_eq!(field.index, 4);
                                self.bestiary = Some(object.guid);
                            }
                            _ => assert_eq!(field.index, 2),
                        }
                    } else {
                        assert!(matches!(
                            field.index,
                            CLIENT_REQUEST_GET_USER_PROFILE | CLIENT_REQUEST_GET_CHARACTER_PROFILE
                        ));
                        self.proxy = Some(object.guid);
                    }
                }
            }
        }
    }

    fn poll_collect(&mut self) {
        self.app.world_mut().resource_mut::<PeerLab>().poll();
        self.collect();
    }

    fn ack(&mut self) {
        self.client.keepalive().unwrap();
        self.poll_collect();
    }

    fn send(&mut self, step: Step) {
        let codec = initialise_codec(self.profile.engine_network_version);
        match step {
            Step::Begin => {
                self.client
                    .send_actor_rpc(
                        PLAYER_CONTROLLER_CHANNEL_INDEX,
                        &begin_player_initialisation_field(3, &codec).unwrap(),
                        BP_ICARUS_PLAYER_CONTROLLER_SURVIVAL_FIELD_INDEX_BOUND,
                        &codec.actor,
                    )
                    .unwrap();
            }
            Step::User | Step::Character => {
                self.client
                    .send_profile_response(
                        PLAYER_CONTROLLER_CHANNEL_INDEX,
                        self.proxy.unwrap(),
                        ProbeProfileResponse {
                            character: step == Step::Character,
                            success: true,
                            profile: if step == Step::User {
                                profiles::user().rpc
                            } else {
                                profiles::character(3).rpc
                            },
                        },
                        &codec,
                    )
                    .unwrap();
            }
            Step::Loadout | Step::Bestiary => {
                let mut bits = BitWriter::new();
                for bit in [true, false, true] {
                    bits.write_bit(bit);
                }
                let payload = Payload::from_writer(bits);
                if step == Step::Loadout {
                    self.client
                        .send_loadout_response(
                            PLAYER_CONTROLLER_CHANNEL_INDEX,
                            self.player_data.unwrap(),
                            payload,
                            &codec.actor,
                        )
                        .unwrap();
                } else {
                    self.client
                        .send_bestiary_response(
                            PLAYER_CONTROLLER_CHANNEL_INDEX,
                            self.bestiary.unwrap(),
                            payload,
                            &codec.actor,
                        )
                        .unwrap();
                }
            }
        }
    }

    fn through_before(&mut self, stop: Step) {
        for step in STEPS {
            if step == stop {
                break;
            }
            self.send(step);
            self.poll_collect();
            self.ack();
        }
    }

    /// Send the trigger before pressure traffic exists. Its stale ACK fields
    /// therefore cannot free the ring when the server finally processes it.
    fn pressure(&mut self, step: Step, spare: usize) {
        self.client.keepalive().unwrap();
        self.app.world_mut().resource_mut::<PeerLab>().poll();
        // Do not receive the resulting ACK before sending the trigger.
        self.send(step);
        // A separately sequenced duplicate must not overwrite the retained
        // response or start another phase while the first one is blocked.
        self.send(step);
        self.collect();
        // The ACK is the ring's sole record after the preceding keepalive.
        for index in 0..DEFAULT_SENT_CAPACITY - 1 - spare {
            assert!(
                self.app
                    .world_mut()
                    .resource_mut::<PeerLab>()
                    .update_actor_movement(
                        self.connection,
                        PRESSURE_ACTOR,
                        [index as f32 + 1.0, 0.0, 0.0],
                        self.profile.engine_network_version,
                    )
            );
            assert_eq!(
                self.client
                    .collect(Duration::from_secs(1), 1)
                    .unwrap()
                    .replies
                    .len(),
                1
            );
        }
        self.poll_collect();
        let stats = self.app.world().resource::<PeerLab>().stats();
        assert_eq!(self.app.world().resource::<PeerLab>().peer_count(), 1);
        assert_eq!(stats.profile_players_failed, 0);
        assert_eq!(stats.peers_failed, 0);
        assert!(stats.profile_exchange_errors > 0, "{step:?}, spare={spare}");
        match (step, spare) {
            (Step::Begin, 0) => assert_eq!(stats.profile_user_requests_sent, 0),
            (Step::User, 0) => assert_eq!(stats.profile_user_responses_received, 0),
            (Step::Character, 0) => assert_eq!(stats.profile_character_responses_received, 0),
            (Step::User, 1) => {
                assert_eq!(stats.profile_user_responses_received, 1);
                assert_eq!(stats.profile_character_requests_sent, 0);
            }
            (Step::Character, 1) => {
                assert_eq!(stats.profile_character_responses_received, 1);
                assert_eq!(stats.loadout_requests_sent, 0);
            }
            (Step::Bestiary, 0) => assert_eq!(stats.players_initialised, 0),
            _ => panic!("unsupported pressure stage"),
        }
        let before = (stats.profile_exchange_errors, stats.ack_errors);
        for _ in 0..8 {
            self.app.world_mut().resource_mut::<PeerLab>().poll();
        }
        let stats = self.app.world().resource::<PeerLab>().stats();
        assert_eq!(
            (stats.profile_exchange_errors, stats.ack_errors),
            before,
            "no busy retry"
        );
    }

    fn finish_after(&mut self, completed: Step) {
        let mut remaining = false;
        for step in STEPS {
            if remaining {
                self.send(step);
                self.poll_collect();
                self.ack();
            }
            remaining |= step == completed;
        }
    }

    fn assert_complete(&self) {
        let stats = self.app.world().resource::<PeerLab>().stats();
        assert_eq!(stats.profile_user_responses_received, 1);
        assert_eq!(stats.profile_character_responses_received, 1);
        assert_eq!(stats.loadout_requests_sent, 1);
        assert_eq!(stats.bestiary_requests_sent, 1);
        assert_eq!(stats.players_initialised, 1);
        assert_eq!(stats.players_finalised, 1);
        assert_eq!(stats.player_character_id_updates_sent, 1);
        assert_eq!(stats.profile_players_failed, 0);
        assert_eq!(stats.peers_failed, 0);
    }
}

#[test]
fn profile_transitions_resume_once_after_packet_pressure() {
    // One spare slot allows the small authored profile publication, but not
    // the following Character request or loadout/bestiary request batch.
    for (step, spare) in [
        (Step::Begin, 0),
        (Step::User, 0),
        (Step::Character, 0),
        (Step::User, 1),
        (Step::Character, 1),
        (Step::Bestiary, 0),
    ] {
        let mut fixture = Fixture::new();
        fixture.through_before(step);
        fixture.pressure(step, spare);
        fixture.ack();
        // A new reliable RPC carrying the same application response must not
        // repeat the accepted transition, despite advancing packet history.
        fixture.send(step);
        fixture.poll_collect();
        fixture.ack();
        fixture.finish_after(step);
        fixture.assert_complete();
        fixture.ack();
        fixture.assert_complete();
        fixture.client.close().unwrap();
        fixture.app.world_mut().resource_mut::<PeerLab>().poll();
        assert_eq!(fixture.app.world().resource::<PeerLab>().peer_count(), 0);
    }
}

#[test]
fn pending_profile_does_not_block_another_peer_and_is_discarded_on_close() {
    let mut fixture = Fixture::new();
    fixture.through_before(Step::Character);
    fixture.pressure(Step::Character, 0);
    let address = fixture.address;
    let version = fixture.profile.pin.network_checksum;
    let engine = fixture.profile.engine_network_version;
    let (healthy, _) = with_server(&mut fixture.app, move || {
        let (mut client, _) = join_pinned_open(address, version, 32);
        exchange_profiles(&mut client, engine, 4, 4);
        client
    });
    let stats = fixture.app.world().resource::<PeerLab>().stats();
    assert_eq!(stats.players_initialised, 1);
    assert_eq!(stats.players_finalised, 1);
    assert_eq!(stats.profile_character_responses_received, 1);
    fixture.client.close().unwrap();
    fixture.app.world_mut().resource_mut::<PeerLab>().poll();
    assert_eq!(fixture.app.world().resource::<PeerLab>().peer_count(), 1);
    let (reconnected, _) = with_server(&mut fixture.app, move || {
        let (mut client, _) = join_pinned_open(address, version, 32);
        exchange_profiles(&mut client, engine, 3, 3);
        client
    });
    let stats = fixture.app.world().resource::<PeerLab>().stats();
    assert_eq!(stats.players_initialised, 2);
    assert_eq!(stats.players_finalised, 2);
    assert_eq!(stats.profile_character_responses_received, 2);
    assert_eq!(stats.profile_players_failed, 0);
    healthy.close().unwrap();
    reconnected.close().unwrap();
    fixture.app.world_mut().resource_mut::<PeerLab>().poll();
    assert_eq!(fixture.app.world().resource::<PeerLab>().peer_count(), 0);
}

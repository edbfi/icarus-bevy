//! Independent authored native worlds and loopback peers only. These tests
//! exercise the production installer; they register no replication plugins or
//! actor components, and establish no ICARUS/stock-client compatibility.
#[path = "common/c001_inventory_owners.rs"]
mod inventory_fixture;
#[path = "common/c001_owners.rs"]
mod owner_fixture;
#[path = "common/c001_scalar_owners.rs"]
mod scalar;
#[path = "common/c001_inventory_source.rs"]
mod source;
use std::{
    collections::BTreeMap,
    fs,
    io::ErrorKind,
    net::{SocketAddr, UdpSocket},
    sync::Arc,
    time::{Duration, Instant},
};

use bevy::{prelude::*, time::TimeUpdateStrategy};
use icarus_bevy::{
    config::{ConfigOptions, NetworkConfig, ServerConfig},
    experimental::{
        actor_channel::{ActorChannelCodec, ActorChannelLimits, read_actor_bunch_body},
        archive::ByteOrder,
        control_messages::{
            ArchiveText, ControlCodec, ControlMessage, NetIdType, NetIdValue, UniqueNetId,
            encode_control_message,
        },
        handshake::{decode, initial_request},
        packet::{
            BitWriter, Bunch, BunchHeader, Name, Packet, PacketConfig, Payload, Receiver,
            read_packet, write_packet,
        },
        packet_lab::client_cookie_seeds,
        peer_lab::{PeerLab, PeerLabAdmission, PeerLabConfig},
        rep_layout::{
            MovementLevel, RepLayoutCodec, RepLayoutLimits, RepValue, RotationLevel,
            actor_rep_layout, parse_rep_properties,
        },
        sequencing::{
            MAX_CONSTRUCTED_PARTIAL_BUNCH_BYTES, PacketSequencer, PartialBunchReassembler,
            PartialReceive, ReliableChannel, reconstruct_reliable_sequence, wire_reliable_sequence,
        },
        session_open::{authored_movement, map_level_chain},
    },
    native_replication::{
        self, AuthoredFlags, AuthoredReplication, MAX_BINDINGS, MAX_DECLARATION_BYTES,
        MAX_PATH_DEPTH, MAX_STRING_BYTES, NativeReplicationCounts, NativeReplicationSet,
    },
    native_runtime::{
        NativeActorIdentity, NativeRotation, NativeWorldContext, update_native_actor,
    },
    native_world::{
        CompiledNativeCatalog, NativeCatalogDefinition, NativeTransform, prepare_authored_world,
    },
    network,
    persistence::SnapshotStore,
    replication::{
        NetActor, NetActorFlags, NetSchedule, NetViewer, ReplicationConfig, ReplicationFrame,
        ReplicationPlugin,
    },
    session_replication::{NetArchetype, SessionReplication},
    simulation::{
        DomainId, Position, SimulationClock, SimulationSet, WorldSnapshot, build_app, capture,
    },
};
use serde_json::{Value, json};

// An unknown, independently authored release/profile with no shipped content
// identities. The real release parser, version pin and session planner are used.
const RELEASE: &str = r#"
[release]
app_id = 2089300
build_id = 1
project_version = "1.0.0"
network_checksum = 1447163286
[tags]
data_table_tag_count = 1
ini_tag_count = 0
unique_node_count = 1
total_bits = 2
configured_first_segment = 0
effective_first_segment = 0
container_bits = 1
net_index_hash = 0
[player_controller]
archetype = [
    { path = "Default__AuthoredController_C", checksum = 7 },
    { path = "/Lab/Controller", checksum = 0 },
]
components = [{ name = "A", checksum = 1 }, { name = "B", checksum = 2 }, { name = "C", checksum = 3 }]
"#;

const MANAGER_PROFILE: &str = r#"
[game_state]
archetype = [{ path = "State", checksum = 21 }, { path = "/Lab/State", checksum = 0 }]
game_mode_class = [{ path = "Mode", checksum = 22 }, { path = "/Lab/Mode", checksum = 0 }]
spectator_class = [{ path = "Spectator", checksum = 23 }, { path = "/Lab/Spectator", checksum = 0 }]
connected_players = { name = "Connections", checksum = 24 }
dialogue_system = { name = "Dialogue", checksum = 25 }
weather = { name = "Weather", checksum = 26 }
[managers]
quest_manager = [{ path = "QuestManager", checksum = 31 }, { path = "/Lab/Managers", checksum = 0 }]
inventory_container_manager = [{ path = "Containers", checksum = 32 }, { path = "/Lab/Managers", checksum = 0 }]
player_history_tracker = [{ path = "History", checksum = 33 }, { path = "/Lab/Managers", checksum = 0 }]
flod = [{ path = "Flod", checksum = 34 }, { path = "/Lab/Managers", checksum = 0 }]
radiation_manager = [{ path = "Radiation", checksum = 35 }, { path = "/Lab/Managers", checksum = 0 }]
world_talent_manager = [{ path = "Talent", checksum = 36 }, { path = "/Lab/Managers", checksum = 0 }]
disaster_controller = [{ path = "Disaster", checksum = 37 }, { path = "/Lab/Managers", checksum = 0 }]
stat_container_checksum = 38
actor_state_checksum = 39
"#;

struct Fixture {
    dir: tempfile::TempDir,
    snapshot: WorldSnapshot,
    declaration: Value,
    config: ServerConfig,
}

impl Fixture {
    fn new() -> Self {
        let definition: NativeCatalogDefinition = serde_json::from_value(json!({
            "native_format": 1, "frame": "right_handed_z_up_meters",
            "classes": [{"key": "spatial", "kind": "spatial_object_v1", "variables": [],
                "local_bounds": {"min": [-0.5, -0.5, -0.5], "max": [0.5, 0.5, 0.5]}}],
            "maps": [{"key": "authored", "tiles": [{"key": "tile",
                "bounds": {"min": [-20000000.0, -20000000.0, -20000000.0], "max": [20000000.0, 20000000.0, 20000000.0]}}],
                "placements": [
                    {"key": "a", "class": "spatial", "tile": "tile", "transform": pose_json([1.0, 2.0, 3.0])},
                    {"key": "b", "class": "spatial", "tile": "tile", "transform": pose_json([4.0, 5.0, 6.0])},
                    {"key": "unbound", "class": "spatial", "tile": "tile", "transform": pose_json([7.0, 8.0, 9.0])}
                ]}]
        })).unwrap();
        let prepared = prepare_authored_world(
            Arc::new(CompiledNativeCatalog::compile(definition).unwrap()),
            "authored",
        )
        .unwrap();
        let snapshot = prepared.initial_snapshot();
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("release.toml"), RELEASE).unwrap();
        let declaration = json!({
            "format": 1, "catalog_id": snapshot.native_world.as_ref().unwrap().catalog_id,
            "map": "authored", "frame": {"axes": [2, -1, 3], "units": 10.0},
            "first_channel": 6,
            "schedule": {
                "connection_budget": 32, "max_consider_list": 32, "max_connections": 8,
                "max_viewers_per_connection": 1, "net_server_max_tick_rate": 30,
                "max_connections_to_tick_per_server_frame": 0,
                "relevant_timeout_seconds": 5.0, "spawn_priority_seconds": 1.0,
                "use_adaptive_net_update_frequency": false, "seed": 19
            },
            "bindings": [binding_json("a")]
        });
        let config = ServerConfig {
            data_dir: dir.path().join("state"),
            network: NetworkConfig {
                enabled: true,
                port: 0,
                release_profile: Some(dir.path().join("release.toml")),
                authored_replication: Some(dir.path().join("replication.json")),
                ..Default::default()
            },
            ..Default::default()
        };
        Self {
            dir,
            snapshot,
            declaration,
            config,
        }
    }

    fn write_declaration(&self) {
        fs::write(
            self.config.network.authored_replication.as_ref().unwrap(),
            serde_json::to_vec(&self.declaration).unwrap(),
        )
        .unwrap();
    }

    fn app(&self) -> (App, SocketAddr) {
        self.app_tuned(None)
    }

    fn app_tuned(&self, tune: Option<fn(&mut PeerLabConfig)>) -> (App, SocketAddr) {
        self.write_declaration();
        let mut app = build_app(&self.config, &self.snapshot).unwrap();
        let bound = if let Some(tune) = tune {
            // The installed runtime still uses the production helper/plan.
            // Only the existing transport's authored resource limits change.
            let mut config = network::peer_lab_config(&self.config.network);
            tune(&mut config);
            network::bind_with_parameters(&self.config.network, config)
                .unwrap()
                .unwrap()
        } else {
            network::bind(&self.config.network).unwrap().unwrap()
        };
        let address = network::install_configured(&mut app, bound, &self.config.network).unwrap();
        app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_millis(
            34,
        )));
        (app, address)
    }

    fn id(&self, placement: &str) -> DomainId {
        self.snapshot
            .native_world
            .as_ref()
            .unwrap()
            .actors
            .iter()
            .find(|actor| actor.placement == placement)
            .unwrap()
            .id
    }
}

fn pose_json(translation: [f32; 3]) -> Value {
    json!({"translation": translation, "rotation": [0.0, 0.0, 0.0, 1.0], "scale": [1.0, 1.0, 1.0]})
}

fn binding_json(placement: &str) -> Value {
    json!({
        "placement": placement, "archetype": [
            {"path": "Default__IndependentBox_C", "checksum": 12345, "no_load": false},
            {"path": "/Lab/IndependentBox", "checksum": 0, "no_load": false}
        ],
        "parameters": {"net_cull_distance_squared": 1e14, "net_update_frequency": 1000.0,
            "min_net_update_frequency": 1000.0, "net_priority": 1.0},
        "replicate_movement": true, "flags": {"hidden": true, "can_be_damaged": true}
    })
}

fn native_entity(app: &App, id: DomainId) -> Entity {
    app.world()
        .iter_entities()
        .find(|entity| entity.get::<DomainId>() == Some(&id))
        .unwrap()
        .id()
}

fn edit(app: &mut App, id: DomainId, translation: [f32; 3]) {
    update_native_actor(
        app.world_mut(),
        id,
        NativeTransform {
            translation,
            ..Default::default()
        },
        Vec::new(),
    )
    .unwrap();
}

fn session() -> SessionReplication {
    SessionReplication::new(
        map_level_chain(PeerLabAdmission::default().map_url).unwrap(),
        17,
        1,
        [0.0; 3],
    )
    .unwrap()
}

fn prepare(app: &mut App, value: Value) -> anyhow::Result<native_replication::PreparedReplication> {
    native_replication::prepare(
        app.world_mut(),
        AuthoredReplication::parse(&value.to_string())?,
        session(),
        1024,
    )
}

/// Authored client sequencing harness, matching the existing peer-lab patterns.
/// Datagrams may be deliberately withheld from accept() to model loss/late ACK.
struct Client {
    socket: UdpSocket,
    sequencer: PacketSequencer,
    control: ReliableChannel<ControlMessage>,
    packet: PacketConfig,
    order: ByteOrder,
}

fn drain(socket: &UdpSocket) -> Vec<Vec<u8>> {
    let mut datagrams = Vec::new();
    loop {
        let mut buffer = [0; 2048];
        match socket.recv(&mut buffer) {
            Ok(length) => datagrams.push(buffer[..length].to_vec()),
            Err(error) if error.kind() == ErrorKind::WouldBlock => break,
            Err(error) => panic!("{error}"),
        }
    }
    datagrams
}

impl Client {
    fn connect(app: &mut App, address: SocketAddr, order: ByteOrder) -> Self {
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        socket.connect(address).unwrap();
        socket.set_nonblocking(true).unwrap();
        socket.send(&initial_request()).unwrap();
        app.update();
        let challenge = drain(&socket).pop().expect("challenge");
        let cookie = decode(&challenge).unwrap().cookie;
        socket.send(&challenge).unwrap();
        app.update();
        assert_eq!(
            decode(&drain(&socket).pop().expect("handshake ack"))
                .unwrap()
                .cookie,
            cookie
        );
        let (incoming, outgoing) = client_cookie_seeds(&cookie);
        let sequencer = PacketSequencer::new(incoming, outgoing, 64).unwrap();
        let control = sequencer.new_channel();
        let mut client = Self {
            socket,
            sequencer,
            control,
            packet: app.world().resource::<PeerLab>().config().packet.packet,
            order,
        };
        for message in [
            ControlMessage::Hello {
                first_field: u8::from(matches!(order, ByteOrder::Little)),
                version: 1447163286,
                token: ArchiveText::text("").unwrap(),
            },
            ControlMessage::Login {
                response: ArchiveText::text("0").unwrap(),
                url: ArchiveText::text("?Name=A").unwrap(),
                unique_net_id: UniqueNetId {
                    kind: NetIdType::Named(ArchiveText::text("STEAM").unwrap()),
                    value: NetIdValue::from_hex("0123456789ABCDEF7654").unwrap(),
                },
                platform: ArchiveText::text("").unwrap(),
            },
        ] {
            client.control(app, message);
            let replies = drain(&client.socket);
            assert!(!replies.is_empty());
            client.accept(&replies);
        }
        client
    }

    fn config(&self) -> PacketConfig {
        PacketConfig {
            receiver: Receiver::Client,
            ..self.packet
        }
    }

    fn send(&mut self, bunches: Vec<Bunch>) {
        let packet = Packet {
            magic: 0,
            header: self.sequencer.header().unwrap(),
            info: None,
            bunches,
        };
        let bytes = write_packet(&packet, &self.packet).unwrap();
        self.sequencer.commit().unwrap();
        self.socket.send(&bytes).unwrap();
    }

    fn control(&mut self, app: &mut App, message: ControlMessage) {
        let sequence = self
            .control
            .send_reliable(self.sequencer.out_packet_id(), message.clone())
            .unwrap();
        let mut payload = BitWriter::new();
        payload.write_bytes(
            &encode_control_message(
                &message,
                &ControlCodec {
                    order: self.order,
                    ..Default::default()
                },
            )
            .unwrap(),
        );
        self.send(vec![Bunch {
            header: BunchHeader {
                control: false,
                open: false,
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
        }]);
        app.update();
    }

    fn join(&mut self, app: &mut App) -> Vec<Vec<u8>> {
        self.control(app, ControlMessage::Join);
        let mut opened = drain(&self.socket);
        for _ in 0..4 {
            app.update();
            opened.extend(drain(&self.socket));
        }
        assert!(
            !app.world()
                .resource::<PeerLab>()
                .joined_connections()
                .is_empty()
        );
        opened
    }

    fn accept(&mut self, datagrams: &[Vec<u8>]) {
        for datagram in datagrams {
            let packet = read_packet(datagram, &self.config()).unwrap();
            if let Some(received) = self.sequencer.receive(&packet.header).unwrap() {
                self.sequencer.ack_seq(received.sequence, true).unwrap();
            }
        }
    }

    fn step(&mut self, app: &mut App) -> Vec<Vec<u8>> {
        self.send(Vec::new());
        app.update();
        let mut bytes = drain(&self.socket);
        for _ in 0..4 {
            app.update();
            bytes.extend(drain(&self.socket));
        }
        bytes
    }

    fn settle(&mut self, app: &mut App, datagrams: &[Vec<u8>]) -> Vec<Vec<u8>> {
        self.accept(datagrams);
        let bytes = self.step(app);
        self.accept(&bytes);
        bytes
    }

    fn disconnect(&mut self, app: &mut App) {
        let sequence = self.control.out_reliable() + 1;
        self.send(vec![Bunch {
            header: BunchHeader {
                control: true,
                open: false,
                close: true,
                close_reason: Some(0),
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
            payload: Payload::from_writer(BitWriter::new()),
        }]);
        for _ in 0..4 {
            app.update();
        }
        drain(&self.socket);
    }
}

type Fields = BTreeMap<u32, RepValue>;

fn codec(order: ByteOrder) -> RepLayoutCodec {
    RepLayoutCodec {
        actor: ActorChannelCodec {
            engine_net_version: 17,
            exports_enabled: false,
            has_level: true,
            order,
            limits: ActorChannelLimits::default(),
        },
        checksums: false,
        limits: RepLayoutLimits::default(),
    }
}

/// Reassemble actual wire bodies, including partial property groups. Decode
/// only dynamic world channels (the controller owns 2 in this fixture).
fn actor_bodies(
    bytes: &[Vec<u8>],
    client: &Client,
) -> Vec<(
    bool,
    icarus_bevy::experimental::actor_channel::ActorBunchBody,
)> {
    let mut reassemblers = BTreeMap::new();
    let mut reliable_sequences = BTreeMap::new();
    let mut previous: Option<(u16, i64)> = None;
    let mut bodies = Vec::new();
    for datagram in bytes {
        let packet = read_packet(datagram, &client.config()).unwrap();
        let sequence = packet.header.sequence;
        let id = previous.map_or(i64::from(sequence), |(last, id)| {
            id + i64::from(sequence.wrapping_sub(last) & 0x3fff)
        });
        previous = Some((sequence, id));
        for bunch in packet.bunches {
            if bunch.header.channel_index < 6 || bunch.header.close {
                continue;
            }
            let bunch = if bunch.header.partial {
                let empty = Payload::from_writer(BitWriter::new());
                let payload = if bunch.header.has_package_map_exports {
                    &empty
                } else {
                    &bunch.payload
                };
                let part_sequence = if bunch.header.reliable {
                    let raw = bunch.header.reliable_sequence_raw.unwrap();
                    let last = reliable_sequences
                        .entry(bunch.header.channel_index)
                        .or_insert(raw);
                    let sequence = reconstruct_reliable_sequence(raw, *last).unwrap();
                    *last = u32::try_from(sequence).unwrap();
                    sequence
                } else {
                    id
                };
                match reassemblers
                    .entry(bunch.header.channel_index)
                    .or_insert_with(PartialBunchReassembler::new)
                    .receive_part(
                        part_sequence,
                        &bunch.header,
                        payload,
                        Some(MAX_CONSTRUCTED_PARTIAL_BUNCH_BYTES),
                    ) {
                    Ok(PartialReceive::Complete { bunch, .. }) => bunch,
                    _ => continue,
                }
            } else {
                if bunch.header.has_package_map_exports {
                    continue;
                }
                bunch
            };
            let open = bunch.header.open;
            let body = read_actor_bunch_body(
                &mut bunch.payload.reader(),
                open,
                &codec(client.order).actor,
            )
            .unwrap();
            bodies.push((open, body));
        }
    }
    bodies
}

fn fields(bytes: &[Vec<u8>], client: &Client, opens: bool) -> Vec<Fields> {
    let layout = actor_rep_layout(
        MovementLevel::Whole,
        MovementLevel::Whole,
        RotationLevel::Bytes,
    );
    actor_bodies(bytes, client)
        .into_iter()
        .filter(|(open, _)| *open == opens)
        .map(|(_, body)| {
            body.blocks
                .into_iter()
                .flat_map(|block| {
                    parse_rep_properties(&block.payload, &layout, &codec(client.order))
                        .unwrap()
                        .entries
                })
                .map(|entry| (entry.handle, entry.value))
                .collect()
        })
        .collect()
}

fn movement(position: [f32; 3]) -> RepValue {
    RepValue::Movement(authored_movement(position, [0.0; 3], [0.0; 3]).unwrap())
}

#[test]
fn production_installer_opens_native_actors_and_edits_through_explicit_frame_in_both_orders() {
    for order in [ByteOrder::Little, ByteOrder::Big] {
        let fixture = Fixture::new();
        let (mut app, address) = fixture.app();
        let native = native_entity(&app, fixture.id("a"));
        assert!(app.world().get::<NetActor>(native).is_none());
        assert_eq!(
            app.world().get::<Position>(native),
            Some(&Position([1.0, 2.0, 3.0]))
        );
        let mut client = Client::connect(&mut app, address, order);
        let opened = client.join(&mut app);
        let initial = fields(&opened, &client, true);
        assert_eq!(initial.len(), 1);
        assert_eq!(initial[0].get(&6), Some(&movement([20.0, -10.0, 30.0])));
        assert_eq!(
            initial[0].keys().copied().collect::<Vec<_>>(),
            [1, 4, 5, 6, 14]
        );
        let spawns = actor_bodies(&opened, &client)
            .into_iter()
            .filter_map(|(_, body)| body.spawn)
            .collect::<Vec<_>>();
        assert_eq!(spawns.len(), 1, "unbound native placements never open");
        assert_eq!(
            app.world().resource::<PeerLab>().stats().actor_opens_sent,
            1
        );
        edit(&mut app, fixture.id("a"), [7.0, 8.0, 9.0]);
        for _ in 0..4 {
            app.update();
        }
        assert_eq!(
            app.world().resource::<PeerLab>().stats().actor_updates_sent,
            0,
            "open ACK gates the late native edit"
        );
        assert!(fields(&drain(&client.socket), &client, false).is_empty());
        let changed = client.settle(&mut app, &opened);
        assert_eq!(
            fields(&changed, &client, false),
            [Fields::from([(6, movement([80.0, -70.0, 90.0]))])]
        );
        assert_eq!(
            app.world().get::<Position>(native),
            Some(&Position([7.0, 8.0, 9.0]))
        );
        assert_eq!(
            app.world().get::<NativeRotation>(native).unwrap().0,
            [0.0, 0.0, 0.0, 1.0]
        );
        let captured = capture(app.world_mut()).unwrap();
        assert_eq!(captured.native_world.as_ref().unwrap().actors.len(), 3);
        assert_eq!(
            captured.native_world.as_ref().unwrap().actors[0]
                .transform
                .translation,
            [7.0, 8.0, 9.0]
        );
        network::teardown(app.world_mut());
        let mut reloaded_fixture = Fixture::new();
        reloaded_fixture.snapshot = captured.clone();
        let (mut reloaded, second_address) = reloaded_fixture.app();
        assert!(capture(reloaded.world_mut()).unwrap() == captured);
        let mut rejoined = Client::connect(&mut reloaded, second_address, order);
        let reopened = rejoined.join(&mut reloaded);
        assert_eq!(
            fields(&reopened, &rejoined, true)[0].get(&6),
            Some(&movement([80.0, -70.0, 90.0]))
        );
        assert_eq!(
            reloaded
                .world()
                .resource::<NativeReplicationCounts>()
                .activated,
            1
        );
        network::teardown(reloaded.world_mut());
    }
}

#[test]
fn invalid_last_binding_is_atomic_and_diagnostics_are_structural() {
    let fixture = Fixture::new();
    let mut bad = fixture.declaration.clone();
    bad["bindings"]
        .as_array_mut()
        .unwrap()
        .push(binding_json("SENTINEL_UNKNOWN_PLACEMENT"));
    fixture.write_declaration();
    fs::write(
        fixture
            .config
            .network
            .authored_replication
            .as_ref()
            .unwrap(),
        bad.to_string(),
    )
    .unwrap();
    let mut app = build_app(&fixture.config, &fixture.snapshot).unwrap();
    let before = capture(app.world_mut()).unwrap();
    let entities = app.world().iter_entities().count();
    let bound = network::bind(&fixture.config.network).unwrap().unwrap();
    let next_guid = bound.lab.guid_cache().clone().assign_dynamic();
    let error = network::install_configured(&mut app, bound, &fixture.config.network)
        .expect_err("reject invalid last binding");
    assert!(!format!("{error:#}").contains("SENTINEL_UNKNOWN_PLACEMENT"));
    assert!(format!("{error:#}").contains("placement missing"));
    assert_eq!(app.world().iter_entities().count(), entities);
    assert!(capture(app.world_mut()).unwrap() == before);
    assert!(!app.is_plugin_added::<ReplicationPlugin>());
    assert!(!app.world().contains_resource::<PeerLab>());
    assert!(!app.world().contains_resource::<NativeReplicationCounts>());
    // A successful retry starts with exactly a fresh lab's GUID allocation.
    fixture.write_declaration();
    let bound = network::bind(&fixture.config.network).unwrap().unwrap();
    assert_eq!(bound.lab.guid_cache().clone().assign_dynamic(), next_guid);
    network::install_configured(&mut app, bound, &fixture.config.network).unwrap();
    assert_eq!(
        app.world()
            .resource::<PeerLab>()
            .guid_cache()
            .clone()
            .assign_dynamic(),
        next_guid
    );
    network::teardown(app.world_mut());
}

#[test]
fn configured_static_path_identities_reject_atomically_and_accept_agreement() {
    use icarus_bevy::experimental::{
        quest_open::QuestOpenPlan,
        release::ReleaseProfile,
        rep_layout::{actor_state_rep_layout, quest_rep_layout},
        session_open::{ActorKey, PathSegment},
    };
    let mut fixture = Fixture::new();
    let profile_text = format!(
        "{RELEASE}{MANAGER_PROFILE}{}",
        r#"
[player_state]
archetype = [{ path = "PlayerState", checksum = 51 }, { path = "/Lab/PlayerState", checksum = 0 }]
[pawn]
archetype = [{ path = "Pawn", checksum = 52 }, { path = "/Lab/Pawn", checksum = 0 }]
components = [{ name = "Stats", checksum = 53 }, { name = "State", checksum = 54 }, { name = "Inventory", checksum = 55 }]
stat_container = "Stats"
actor_state = "State"
inventory_component = "Inventory"
inventory_class = [{ path = "InventoryClass", checksum = 56 }, { path = "/Lab/Inventory", checksum = 0 }]
"#
    );
    fs::write(
        fixture.config.network.release_profile.as_ref().unwrap(),
        &profile_text,
    )
    .unwrap();
    let profile = ReleaseProfile::parse(&profile_text).unwrap();
    let map = PeerLabAdmission::default().map_url;
    let controller = profile
        .player_controller_open_plan(map, [0.0; 3])
        .unwrap()
        .unwrap();
    let pawn = profile.pawn_open_plan(map).unwrap().unwrap();
    let state = profile.game_state_open_plan(map).unwrap().unwrap();
    let player = profile.player_state_open_plan(map).unwrap().unwrap();
    let quest = QuestOpenPlan {
        key: ActorKey::Saved(777),
        archetype: vec![
            PathSegment::new("AuthoredQuest", 40, false),
            PathSegment::new("/Lab/Quest", 0, false),
        ],
        level: map_level_chain(map).unwrap(),
        location: None,
        layout: quest_rep_layout(
            MovementLevel::Whole,
            MovementLevel::Whole,
            RotationLevel::Bytes,
            profile.tags.tag_encoding(),
        ),
        actor_state_layout: actor_state_rep_layout(),
        stat_container_checksum: 41,
        actor_state_checksum: 42,
        values: BTreeMap::new(),
        references: BTreeMap::new(),
        engine_net_version: profile.engine_network_version,
    };
    // Source all expected identities from the same configured plans used by
    // this host, including managers deferred until saved state is available.
    let mut paths = vec![
        controller.profile.archetype,
        controller.profile.level,
        pawn.archetype,
        pawn.content.inventory_class,
        state.archetype,
        state.game_mode_class,
        state.spectator_class,
        player.archetype,
        quest.archetype.clone(),
    ];
    paths.extend(
        profile
            .manager_open_plans(map)
            .unwrap()
            .into_iter()
            .map(|(_, plan)| plan.archetype),
    );
    paths.extend(
        profile
            .saved_manager_open_plans(map)
            .unwrap()
            .into_iter()
            .map(|(_, plan)| plan.archetype),
    );
    let bind = |network: &NetworkConfig| {
        let mut bound = network::bind(network).unwrap().unwrap();
        bound.lab.add_quest_open(quest.clone()).unwrap();
        bound
    };
    fixture.declaration["first_channel"] = json!(10);
    fixture.declaration["bindings"]
        .as_array_mut()
        .unwrap()
        .push(binding_json("b"));
    for path in paths {
        // Reuse archetypes directly; nest the level (whose checksum is zero)
        // beneath an authored leaf to exercise a shared inner suffix as well.
        let depth = usize::from(path[0].checksum == 0);
        let shared = if depth == 1 {
            std::iter::once(PathSegment::new("Nested", 12345, false))
                .chain(path.clone())
                .collect::<Vec<_>>()
        } else {
            path.clone()
        };
        for change in 0..4 {
            let mut candidate = shared.clone();
            match change {
                0 => candidate[depth].checksum += 1,
                1 => candidate[depth].no_load = !candidate[depth].no_load,
                2 => {
                    let package = candidate.last_mut().unwrap();
                    package.no_load = !package.no_load;
                }
                _ => {}
            }
            fixture.declaration["bindings"][1]["archetype"] = Value::Array(candidate.iter().map(|segment| json!({
                "path": segment.path, "checksum": segment.checksum, "no_load": segment.no_load,
            })).collect());
            fixture.write_declaration();
            let mut app = build_app(&fixture.config, &fixture.snapshot).unwrap();
            let before = capture(app.world_mut()).unwrap();
            let entities = app.world().iter_entities().count();
            let bound = bind(&fixture.config.network);
            let next_dynamic = bound.lab.guid_cache().clone().assign_dynamic();
            let next_static = bound
                .lab
                .guid_cache()
                .clone()
                .assign_static(&[PathSegment::new("/Lab/Unassigned", 0, false)]);
            let result = network::install_configured(&mut app, bound, &fixture.config.network);
            assert!(capture(app.world_mut()).unwrap() == before);
            if change < 3 {
                let error = result.expect_err("reject conflicting metadata in last binding");
                assert!(format!("{error:#}").contains("static path identity conflict"));
                assert!(!format!("{error:#}").contains(&path[0].path));
                assert_eq!(app.world().iter_entities().count(), entities);
                assert!(!app.is_plugin_added::<ReplicationPlugin>());
                assert!(!app.is_plugin_added::<icarus_bevy::session_replication::SessionReplicationPlugin>());
                assert!(!app.world().contains_resource::<PeerLab>());
                assert!(!app.world().contains_resource::<NativeReplicationCounts>());
                assert!(!app.world().contains_resource::<SessionReplication>());
                assert!(!app.world().contains_resource::<ReplicationConfig>());
                assert!(!app.world().contains_resource::<ReplicationFrame>());
            } else {
                result.unwrap();
                assert_eq!(
                    app.world().resource::<NativeReplicationCounts>().activated,
                    2
                );
                let lab = app.world().resource::<PeerLab>();
                assert_eq!(lab.guid_cache().clone().assign_dynamic(), next_dynamic);
                assert_eq!(
                    lab.guid_cache().clone().assign_static(&[PathSegment::new(
                        "/Lab/Unassigned",
                        0,
                        false
                    ),]),
                    next_static
                );
                network::teardown(app.world_mut());
            }
        }
    }
}

#[test]
fn declarations_reject_formats_unknown_fields_sequences_identity_paths_and_all_caps() {
    let fixture = Fixture::new();
    let mutations: Vec<fn(&mut Value)> = vec![
        |v| v["format"] = json!(2),
        |v| v["catalog_id"] = json!("0".repeat(64)),
        |v| v["catalog_id"] = json!("F".repeat(64)),
        |v| v["map"] = json!("wrong_map"),
        |v| v["first_channel"] = json!(0),
        |v| v["first_channel"] = json!(1024),
        |v| v["frame"]["axes"] = json!([1, 1, 3]),
        |v| v["frame"]["axes"] = json!([0, 2, 3]),
        |v| v["frame"]["axes"] = json!([-128, 2, 3]),
        |v| v["frame"]["units"] = json!(0),
        |v| v["frame"]["units"] = json!(-1),
        |v| v["frame"]["units"] = json!(1e30),
        |v| v["frame"]["units"] = json!(1e6),
        |v| v["frame"]["SENTINEL_UNKNOWN_FIELD"] = json!("SENTINEL_VALUE"),
        |v| v["bindings"][0]["placement"] = json!("missing"),
        |v| v["bindings"][0]["placement"] = json!("a".repeat(MAX_STRING_BYTES + 1)),
        |v| v["bindings"][0]["parameters"]["net_priority"] = json!(0),
        |v| v["bindings"][0]["parameters"]["net_update_frequency"] = json!(-1),
        |v| v["bindings"][0]["parameters"]["min_net_update_frequency"] = json!(1001),
        |v| v["bindings"][0]["parameters"]["net_cull_distance_squared"] = json!(-1),
        |v| {
            v["bindings"][0]["parameters"]
                .as_object_mut()
                .unwrap()
                .remove("net_priority")
                .map(|_| ())
                .unwrap()
        },
        |v| v["bindings"][0]["archetype"][0]["checksum"] = json!(0),
        |v| v["bindings"][0]["archetype"][1]["checksum"] = json!(1),
        |v| v["bindings"][0]["archetype"][1]["path"] = json!("relative_package"),
        |v| v["bindings"][0]["archetype"][1]["path"] = json!("/Lab/../Bad"),
        |v| v["bindings"][0]["archetype"][0]["path"] = json!(""),
        |v| v["bindings"][0]["archetype"][0]["no_load"] = Value::Null,
        |v| v["bindings"][0]["archetype"][0]["path"] = json!("P".repeat(MAX_STRING_BYTES + 1)),
        |v| v["bindings"][0]["archetype"] = json!([]),
        |v| v["bindings"][0]["parameters"] = json!([1, 2, 3, 4]),
        |v| v["bindings"][0]["flags"] = json!([true, false]),
        |v| v["bindings"][0]["flags"]["extra"] = json!(7),
        |v| v["schedule"]["connection_budget"] = json!(0),
        |v| v["schedule"]["connection_budget"] = json!(MAX_BINDINGS + 1),
        |v| v["schedule"]["max_consider_list"] = json!(MAX_BINDINGS + 1),
        |v| v["schedule"]["max_connections"] = json!(65),
        |v| v["schedule"]["max_viewers_per_connection"] = json!(5),
        |v| v["schedule"]["net_server_max_tick_rate"] = json!(0),
        |v| v["schedule"]["max_connections_to_tick_per_server_frame"] = json!(-1),
        |v| v["schedule"]["relevant_timeout_seconds"] = json!(3601),
        |v| v["schedule"]["spawn_priority_seconds"] = json!(-1),
        |v| v["bindings"] = json!([]),
        |v| {
            let binding = v["bindings"][0].clone();
            v["bindings"].as_array_mut().unwrap().push(binding);
        },
        |v| {
            v["bindings"] = Value::Array(
                (0..=MAX_BINDINGS)
                    .map(|index| binding_json(&format!("placement_{index}")))
                    .collect(),
            );
        },
        |v| {
            v["bindings"][0]["archetype"] = Value::Array((0..=MAX_PATH_DEPTH).map(|index| json!({"path": format!("Part{index}"), "checksum": 1, "no_load": false})).collect());
        },
        |v| {
            let mut binding = binding_json("b");
            binding["archetype"][0]["checksum"] = json!(987);
            v["bindings"].as_array_mut().unwrap().push(binding);
        },
    ];
    for (index, mutate) in mutations.into_iter().enumerate() {
        let mut bad = fixture.declaration.clone();
        mutate(&mut bad);
        let mut app = build_app(&fixture.config, &fixture.snapshot).unwrap();
        let before = capture(app.world_mut()).unwrap();
        let error = prepare(&mut app, bad)
            .err()
            .unwrap_or_else(|| panic!("case {index} accepted"));
        let text = format!("{error:#}");
        for secret in [
            "SENTINEL_UNKNOWN_FIELD",
            "SENTINEL_VALUE",
            "IndependentBox",
            "wrong_map",
        ] {
            assert!(!text.contains(secret));
        }
        assert!(capture(app.world_mut()).unwrap() == before);
        assert!(!app.world().contains_resource::<ReplicationConfig>());
        assert!(!app.is_plugin_added::<ReplicationPlugin>());
    }
    let valid = fixture.declaration.to_string();
    for text in [
        format!("{}{}", " ".repeat(MAX_DECLARATION_BYTES), valid),
        valid.replacen("\"format\":1", "\"format\":1,\"format\":1", 1),
        "[1, \"SENTINEL_VALUE\"]".into(),
        "format = 1".into(),
    ] {
        assert!(AuthoredReplication::parse(&text).is_err());
    }
    let path = fixture.dir.path().join("large.json");
    fs::write(&path, vec![b' '; MAX_DECLARATION_BYTES + 1]).unwrap();
    assert!(AuthoredReplication::load(&path).is_err());
    assert!(AuthoredReplication::load(fixture.dir.path()).is_err());
    // Also reject nonfinite programmatic values: JSON cannot encode them.
    for units in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        let mut declaration = AuthoredReplication::parse(&valid).unwrap();
        declaration.frame.units = units;
        let mut app = build_app(&fixture.config, &fixture.snapshot).unwrap();
        assert!(
            native_replication::prepare(app.world_mut(), declaration, session(), 1024).is_err()
        );
    }
}

#[test]
fn native_pose_bounds_activation_and_component_conflicts_reject_before_publication() {
    let fixture = Fixture::new();
    for pose in [
        NativeTransform {
            rotation: [0.0, 0.0, 1.0, 0.0],
            ..Default::default()
        },
        NativeTransform {
            rotation: [0.0, 0.0, 0.0, -1.0],
            ..Default::default()
        },
        NativeTransform {
            scale: [2.0, 1.0, 1.0],
            ..Default::default()
        },
        NativeTransform {
            translation: [f32::INFINITY; 3],
            ..Default::default()
        },
        NativeTransform {
            translation: [30_000_000.0; 3],
            ..Default::default()
        },
    ] {
        let mut app = build_app(&fixture.config, &fixture.snapshot).unwrap();
        let entity = native_entity(&app, fixture.id("a"));
        app.world_mut().entity_mut(entity).insert((
            Position(pose.translation),
            NativeRotation(pose.rotation),
            icarus_bevy::native_runtime::NativeScale(pose.scale),
        ));
        assert!(prepare(&mut app, fixture.declaration.clone()).is_err());
        assert!(!app.is_plugin_added::<ReplicationPlugin>());
    }
    let mut app = build_app(&fixture.config, &fixture.snapshot).unwrap();
    let plan = prepare(&mut app, fixture.declaration.clone()).unwrap();
    edit(&mut app, fixture.id("a"), [9.0; 3]);
    assert!(
        native_replication::install(&mut app, plan).is_err(),
        "prepared pose is stale"
    );
    let plan = prepare(&mut app, fixture.declaration.clone()).unwrap();
    let mut other = build_app(&fixture.config, &fixture.snapshot).unwrap();
    assert!(
        native_replication::install(&mut other, plan).is_err(),
        "identical data is not the same activation"
    );
    let mut altered = build_app(&fixture.config, &fixture.snapshot).unwrap();
    altered
        .world_mut()
        .insert_resource(app.world().resource::<NativeWorldContext>().clone());
    assert!(prepare(&mut altered, fixture.declaration.clone()).is_err());
    for component in 0..4 {
        let mut app = build_app(&fixture.config, &fixture.snapshot).unwrap();
        let entity = native_entity(&app, fixture.id("a"));
        match component {
            0 => {
                app.world_mut().entity_mut(entity).insert(NetActorFlags {
                    hidden: false,
                    can_be_damaged: false,
                });
            }
            1 => {
                app.world_mut().entity_mut(entity).insert(NetArchetype {
                    archetype: Vec::new(),
                    replicate_movement: false,
                });
            }
            2 => {
                app.world_mut().remove_resource::<NativeWorldContext>();
            }
            _ => {
                app.world_mut().entity_mut(entity).remove::<Position>();
            }
        }
        assert!(prepare(&mut app, fixture.declaration.clone()).is_err());
        assert!(!app.is_plugin_added::<ReplicationPlugin>());
    }
    let mut inert = build_app(&ServerConfig::default(), &WorldSnapshot::default()).unwrap();
    assert!(prepare(&mut inert, fixture.declaration.clone()).is_err());
    for schema in 5..=11 {
        let mut unsupported = fixture.snapshot.clone();
        unsupported.schema_version = schema;
        assert!(
            build_app(&fixture.config, &unsupported).is_err(),
            "a schema tag cannot promote spatial state into another activation"
        );
    }
}

#[test]
fn valid_import_inert_relationship_lifecycle_and_preserved_modes_cannot_activate_v1() {
    use icarus_bevy::{
        native_import::{
            C001InertBindingsV1, C001PlacementBindingV1, CompiledC001InertBindings,
            prepare_c001_inert_import,
        },
        native_relationships::{
            CompiledNativeRelationships, NativeActorLinkV1, NativeRelationshipActorV1,
            NativeRelationshipRole, NativeRelationshipsDefinition, prepare_relationship_world,
        },
    };
    let fixture = Fixture::new();
    let mut source = owner_fixture::fixture();
    for fields in &mut source.owners {
        for field in fields
            .iter_mut()
            .filter(|field| field.name == "OwnerResolvePolicy")
        {
            field.payload = vec![0];
        }
    }
    source.rebuild_source();
    let bindings = CompiledC001InertBindings::compile(C001InertBindingsV1 {
        format: 1,
        expected_payload_sha256: source.bindings.expected_payload_sha256.clone(),
        native_catalog_id: source.catalog.identity().into(),
        source_map: source.bindings.source_map.clone(),
        native_map: source.bindings.native_map.clone(),
        coordinates: source.bindings.coordinates,
        owners: source
            .bindings
            .owners
            .iter()
            .map(|owner| C001PlacementBindingV1 {
                source_actor_class: owner.source_actor_class.clone(),
                source_actor_path: owner.source_actor_path.clone(),
                source_object_name: owner.source_object_name.clone(),
                source_icarus_guid: owner.source_icarus_guid,
                native_placement: owner.native_placement.clone(),
                native_class: owner.native_class.clone(),
            })
            .collect(),
    })
    .unwrap();
    let inert = prepare_c001_inert_import(&source.candidate(), source.catalog.clone(), &bindings)
        .unwrap()
        .snapshot()
        .clone();
    let native = fixture.snapshot.native_world.as_ref().unwrap();
    let catalog = CompiledNativeCatalog::compile(native.definitions.clone()).unwrap();
    let relationships = CompiledNativeRelationships::compile(
        NativeRelationshipsDefinition {
            relationship_format: 1,
            native_catalog_id: native.catalog_id.clone(),
            map: native.map.clone(),
            actors: native
                .actors
                .iter()
                .enumerate()
                .map(|(index, actor)| NativeRelationshipActorV1 {
                    placement: actor.placement.clone(),
                    identity: NativeActorLinkV1 {
                        name: vec![65 + index as u16],
                        guid: 100 + index as i32,
                    },
                    role: NativeRelationshipRole::Spatial,
                    foundation: None,
                })
                .collect(),
        },
        &catalog,
    )
    .unwrap();
    let related = prepare_relationship_world(fixture.snapshot.clone(), &relationships)
        .unwrap()
        .snapshot()
        .clone();
    let lifecycle = owner_fixture::fixture().base;
    let modifiers =
        icarus_bevy::native_modifiers::enable_modifier_owners(lifecycle.clone()).unwrap();
    let mut preserved = fixture.snapshot.clone();
    preserved.preserved_prospect = inert.preserved_prospect.clone();
    let legacy_preserved = WorldSnapshot {
        schema_version: 3,
        preserved_prospect: inert.preserved_prospect.clone(),
        ..Default::default()
    };
    let unsupported = [
        inert,
        related,
        lifecycle,
        owner_fixture::imported(),
        scalar::imported(),
        inventory_fixture::imported(),
        modifiers,
        preserved,
        legacy_preserved,
    ];
    assert_eq!(
        unsupported[..7]
            .iter()
            .map(|snapshot| snapshot.schema_version)
            .collect::<Vec<_>>(),
        [5, 6, 7, 8, 9, 10, 11]
    );
    for snapshot in unsupported {
        snapshot.validate().unwrap();
        let mut app = build_app(&fixture.config, &snapshot).unwrap();
        let before = capture(app.world_mut()).unwrap();
        let error = prepare(&mut app, fixture.declaration.clone())
            .err()
            .expect("mode must reject");
        assert!(
            format!("{error:#}").contains("requires authored schema 4"),
            "reject the valid mode before resolving catalog or placement bindings"
        );
        assert!(capture(app.world_mut()).unwrap() == before);
        assert!(!app.is_plugin_added::<ReplicationPlugin>());
        assert!(!app.world().contains_resource::<NativeReplicationCounts>());
    }
}

#[test]
fn actor_and_declaration_insertion_order_do_not_change_identity_or_selection() {
    let mut fixture = Fixture::new();
    fixture.declaration["bindings"]
        .as_array_mut()
        .unwrap()
        .push(binding_json("b"));
    let (mut first, first_address) = fixture.app();
    fixture.declaration["bindings"]
        .as_array_mut()
        .unwrap()
        .reverse();
    fixture
        .snapshot
        .native_world
        .as_mut()
        .unwrap()
        .actors
        .reverse();
    let (mut second, second_address) = fixture.app();
    let mut left = Client::connect(&mut first, first_address, ByteOrder::Little);
    let mut right = Client::connect(&mut second, second_address, ByteOrder::Little);
    let left_bytes = left.join(&mut first);
    let right_bytes = right.join(&mut second);
    assert_eq!(
        fields(&left_bytes, &left, true),
        fields(&right_bytes, &right, true)
    );
    let guids = |bytes: &[Vec<u8>], client: &Client| {
        actor_bodies(bytes, client)
            .into_iter()
            .filter_map(|(_, body)| body.spawn.map(|spawn| spawn.actor.guid))
            .collect::<Vec<_>>()
    };
    assert_eq!(guids(&left_bytes, &left), guids(&right_bytes, &right));
    assert_eq!(
        first.world().resource::<ReplicationFrame>().decisions,
        second.world().resource::<ReplicationFrame>().decisions
    );
    assert!(capture(first.world_mut()).unwrap() == capture(second.world_mut()).unwrap());
    network::teardown(first.world_mut());
    network::teardown(second.world_mut());
}

#[derive(Resource, Default)]
struct EditAfterInput {
    id: Option<DomainId>,
    saw_join_in_input: bool,
}

fn observe_input(lab: Res<PeerLab>, mut edit: ResMut<EditAfterInput>) {
    edit.saw_join_in_input |= !lab.joined_connections().is_empty();
}

fn deferred_edit(mut commands: Commands, mut edit: ResMut<EditAfterInput>) {
    if edit.saw_join_in_input
        && let Some(id) = edit.id.take()
    {
        commands.queue(move |world: &mut World| {
            update_native_actor(
                world,
                id,
                NativeTransform {
                    translation: [8.0, 9.0, 10.0],
                    ..Default::default()
                },
                Vec::new(),
            )
            .unwrap();
        });
    }
}

#[test]
fn network_poll_precedes_input_and_deferred_gameplay_edits_precede_extraction_and_finish() {
    let fixture = Fixture::new();
    let (mut app, address) = fixture.app();
    app.insert_resource(EditAfterInput {
        id: Some(fixture.id("a")),
        ..Default::default()
    })
    .add_systems(FixedUpdate, observe_input.in_set(SimulationSet::Input))
    .add_systems(
        FixedUpdate,
        deferred_edit
            .in_set(SimulationSet::Gameplay)
            .before(NativeReplicationSet),
    );
    let mut client = Client::connect(&mut app, address, ByteOrder::Little);
    let opened = client.join(&mut app);
    assert!(app.world().resource::<EditAfterInput>().saw_join_in_input);
    assert!(app.world().resource::<EditAfterInput>().id.is_none());
    assert_eq!(
        fields(&opened, &client, true)[0].get(&6),
        Some(&movement([90.0, -80.0, 100.0]))
    );
    assert!(app.world().resource::<SimulationClock>().tick > 0);
    network::teardown(app.world_mut());
}

fn tiny_parameters(config: &mut PeerLabConfig) {
    config.admission.as_mut().unwrap().map_url = "/L";
    config.admission.as_mut().unwrap().game_name = "G";
    // Keep room for full, short authored export chains while forcing the
    // native translation/flags body across 120-bit partial payloads.
    config.packet.packet.limits.max_packet_bytes = 86;
    config.packet.max_bunches_per_packet = 1;
    config.packet.sent_capacity = 64;
}

fn tiny_fixture() -> Fixture {
    let mut fixture = Fixture::new();
    fixture.declaration["frame"]["units"] = json!(1.0);
    fixture.declaration["bindings"][0]["archetype"] = json!([
        {"path": "D", "checksum": 1, "no_load": false},
        {"path": "/L", "checksum": 0, "no_load": true}
    ]);
    fixture
}

fn tick_bytes(app: &mut App, client: &Client) -> Vec<Vec<u8>> {
    let mut bytes = Vec::new();
    for _ in 0..4 {
        app.update();
        bytes.extend(drain(&client.socket));
    }
    bytes
}

#[test]
fn each_lost_native_property_partial_repairs_current_fields_after_a_late_superseding_edit() {
    for order in [ByteOrder::Little, ByteOrder::Big] {
        let mut exercised = 0;
        for lost in 0..4 {
            let fixture = tiny_fixture();
            let (mut app, address) = fixture.app_tuned(Some(tiny_parameters));
            let mut client = Client::connect(&mut app, address, order);
            let opened = client.join(&mut app);
            assert_eq!(
                app.world().resource::<PeerLab>().stats().actor_opens_sent,
                1,
                "bounded full authored paths must open under these transport limits"
            );
            client.settle(&mut app, &opened);
            edit(
                &mut app,
                fixture.id("a"),
                [500_000.0, 1_000_000.0, -1_000_000.0],
            );
            native_replication::update_flags(
                app.world_mut(),
                fixture.id("a"),
                Some(AuthoredFlags {
                    hidden: false,
                    can_be_damaged: false,
                }),
            )
            .unwrap();
            let sent = tick_bytes(&mut app, &client);
            let parts: Vec<_> = sent
                .iter()
                .filter(|bytes| {
                    read_packet(bytes, &client.config())
                        .unwrap()
                        .bunches
                        .iter()
                        .any(|bunch| {
                            bunch.header.channel_index >= 6
                                && !bunch.header.reliable
                                && bunch.header.partial
                        })
                })
                .cloned()
                .collect();
            assert!(
                parts.len() >= 2 && parts.len() <= 4,
                "real native translation and flag update must split into bounded parts"
            );
            if lost >= parts.len() {
                network::teardown(app.world_mut());
                continue;
            }
            exercised += 1;
            assert_eq!(fields(&parts, &client, false).len(), 1);
            let delivered: Vec<_> = parts
                .into_iter()
                .enumerate()
                .filter_map(|(index, bytes)| (index != lost).then_some(bytes))
                .collect();
            assert!(
                fields(&delivered, &client, false).is_empty(),
                "incomplete group cannot decode"
            );
            client.accept(&delivered);
            // Supersede only handle 1 before reporting the old group's gap.
            // Motion and damage must still repair from their current native/
            // explicit authored values; no codec or loss policy is replaced.
            native_replication::update_flags(
                app.world_mut(),
                fixture.id("a"),
                Some(AuthoredFlags {
                    hidden: true,
                    can_be_damaged: false,
                }),
            )
            .unwrap();
            let newer = tick_bytes(&mut app, &client);
            client.accept(&newer);
            let mut repaired = Vec::new();
            for _ in 0..3 {
                let bytes = client.step(&mut app);
                client.accept(&bytes);
                repaired.extend(bytes);
            }
            let updates = fields(&repaired, &client, false);
            assert!(
                updates.iter().any(|fields| fields.get(&6)
                    == Some(&movement([1_000_000.0, -500_000.0, -1_000_000.0])))
            );
            assert!(
                updates
                    .iter()
                    .any(|fields| fields.get(&4) == Some(&RepValue::Bool(false)))
            );
            assert!(updates.iter().all(|fields| {
                fields
                    .get(&1)
                    .is_none_or(|value| value == &RepValue::Bool(true))
            }));
            assert!(app.world().resource::<PeerLab>().stats().actor_updates_lost >= 1);
            let sent = app.world().resource::<PeerLab>().stats().actor_updates_sent;
            let acked = client.step(&mut app);
            client.accept(&acked);
            assert!(fields(&client.step(&mut app), &client, false).is_empty());
            assert_eq!(
                app.world().resource::<PeerLab>().stats().actor_updates_sent,
                sent
            );
            network::teardown(app.world_mut());
        }
        assert!(exercised >= 2, "every actual partial index was exercised");
    }
}

#[test]
fn lost_open_part_keeps_native_edits_gated_until_complete_open_ack() {
    for order in [ByteOrder::Little, ByteOrder::Big] {
        for lost_part in [0, 1] {
            let fixture = Fixture::new();
            let (mut app, address) =
                fixture.app_tuned(Some(|config| config.packet.max_bunches_per_packet = 1));
            let mut client = Client::connect(&mut app, address, order);
            let opened = client.join(&mut app);
            let actor_parts: Vec<_> = opened
                .iter()
                .filter(|bytes| {
                    read_packet(bytes, &client.config())
                        .unwrap()
                        .bunches
                        .iter()
                        .any(|bunch| bunch.header.channel_index >= 6)
                })
                .cloned()
                .collect();
            assert!(actor_parts.len() >= 2, "separate export and body packets");
            // Drop either the export or final open body, retaining the other
            // native part and all controller parts.
            let lost = &actor_parts[if lost_part == 0 {
                0
            } else {
                actor_parts.len() - 1
            }];
            let delivered: Vec<_> = opened.into_iter().filter(|bytes| bytes != lost).collect();
            client.accept(&delivered);
            edit(&mut app, fixture.id("a"), [11.0, 12.0, 13.0]);
            assert!(fields(&tick_bytes(&mut app, &client), &client, false).is_empty());
            assert_eq!(
                app.world().resource::<PeerLab>().stats().actor_updates_sent,
                0
            );
            // Report the gap, then ACK the lab's existing reliable resend.
            let mut repaired_open = Vec::new();
            for _ in 0..3 {
                let bytes = client.step(&mut app);
                if bytes
                    .iter()
                    .flat_map(|bytes| read_packet(bytes, &client.config()).unwrap().bunches)
                    .any(|bunch| bunch.header.channel_index >= 6 && bunch.header.reliable)
                {
                    repaired_open = bytes;
                    break;
                }
                client.accept(&bytes);
            }
            assert!(
                repaired_open
                    .iter()
                    .flat_map(|bytes| read_packet(bytes, &client.config()).unwrap().bunches)
                    .any(|bunch| bunch.header.channel_index >= 6 && bunch.header.reliable)
            );
            assert!(fields(&repaired_open, &client, false).is_empty());
            let delta = client.settle(&mut app, &repaired_open);
            assert_eq!(
                fields(&delta, &client, false),
                [Fields::from([(6, movement([120.0, -110.0, 130.0]))])]
            );
            assert_eq!(
                app.world().resource::<PeerLab>().stats().actor_opens_sent,
                1
            );
            network::teardown(app.world_mut());
        }
    }
}

#[test]
fn open_pressure_retries_without_consuming_a_second_actor_or_native_state() {
    for order in [ByteOrder::Little, ByteOrder::Big] {
        let fixture = Fixture::new();
        let (mut app, address) = fixture.app_tuned(Some(|config| {
            // The open needs two records after the ACK-only reply uses one.
            // Admission initially fills two records; acknowledging it releases room.
            config.packet.sent_capacity = 3;
            config.packet.max_bunches_per_packet = 1;
        }));
        let mut client = Client::connect(&mut app, address, order);
        let controller = client.join(&mut app);
        assert_eq!(
            app.world().resource::<PeerLab>().stats().actor_opens_sent,
            0
        );
        assert!(app.world().resource::<PeerLab>().stats().actor_open_errors > 0);
        let native_before = capture(app.world_mut()).unwrap().native_world;
        edit(&mut app, fixture.id("a"), [21.0, 22.0, 23.0]);
        let opened = client.settle(&mut app, &controller);
        assert_eq!(
            fields(&opened, &client, true)[0].get(&6),
            Some(&movement([220.0, -210.0, 230.0]))
        );
        assert_eq!(
            app.world().resource::<PeerLab>().stats().actor_opens_sent,
            1
        );
        client.settle(&mut app, &opened);
        assert_eq!(
            app.world().resource::<PeerLab>().stats().actor_opens_sent,
            1
        );
        let after = capture(app.world_mut()).unwrap().native_world.unwrap();
        assert_eq!(after.actors.len(), native_before.unwrap().actors.len());
        assert_eq!(after.actors[0].transform.translation, [21.0, 22.0, 23.0]);
        network::teardown(app.world_mut());
    }
}

#[test]
fn production_host_reserves_all_authored_session_manager_and_quest_channels() {
    use icarus_bevy::experimental::{
        quest_open::QuestOpenPlan,
        release::ReleaseProfile,
        rep_layout::{actor_state_rep_layout, quest_rep_layout},
        session_open::{ActorKey, PathSegment},
    };
    for order in [ByteOrder::Little, ByteOrder::Big] {
        let mut fixture = Fixture::new();
        let release_path = fixture.config.network.release_profile.as_ref().unwrap();
        fs::write(release_path, format!("{RELEASE}{MANAGER_PROFILE}")).unwrap();
        let profile = ReleaseProfile::load(release_path).unwrap();
        let quest = QuestOpenPlan {
            key: ActorKey::Saved(777),
            archetype: vec![
                PathSegment::new("AuthoredQuest", 40, false),
                PathSegment::new("/Lab/Quest", 0, false),
            ],
            level: map_level_chain(PeerLabAdmission::default().map_url).unwrap(),
            location: None,
            layout: quest_rep_layout(
                MovementLevel::Whole,
                MovementLevel::Whole,
                RotationLevel::Bytes,
                profile.tags.tag_encoding(),
            ),
            actor_state_layout: actor_state_rep_layout(),
            stat_container_checksum: 41,
            actor_state_checksum: 42,
            values: BTreeMap::new(),
            references: BTreeMap::new(),
            engine_net_version: profile.engine_network_version,
        };
        let bind = |network: &NetworkConfig| {
            let mut bound = network::bind(network).unwrap().unwrap();
            assert_eq!(bound.lab.manager_open_count(), 3);
            assert_eq!(bound.lab.add_quest_open(quest.clone()).unwrap(), 9);
            bound
        };
        fixture.write_declaration();
        let mut app = build_app(&fixture.config, &fixture.snapshot).unwrap();
        let error = network::install_configured(
            &mut app,
            bind(&fixture.config.network),
            &fixture.config.network,
        )
        .expect_err("floor 6 overlaps reserved plans");
        assert!(format!("{error:#}").contains("reserved"));
        assert!(!app.is_plugin_added::<ReplicationPlugin>());
        assert!(capture(app.world_mut()).unwrap() == fixture.snapshot);
        fixture.declaration["first_channel"] = json!(10);
        fixture.write_declaration();
        let address = network::install_configured(
            &mut app,
            bind(&fixture.config.network),
            &fixture.config.network,
        )
        .unwrap();
        app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_millis(
            34,
        )));
        let mut client = Client::connect(&mut app, address, order);
        let opened = client.join(&mut app);
        let channels: std::collections::BTreeSet<_> = opened
            .iter()
            .flat_map(|bytes| read_packet(bytes, &client.config()).unwrap().bunches)
            .filter(|bunch| bunch.header.open)
            .map(|bunch| bunch.header.channel_index)
            .collect();
        for channel in [2, 4, 6, 7, 8, 9, 10] {
            assert!(channels.contains(&channel));
        }
        let lab = app.world().resource::<PeerLab>();
        assert_eq!(lab.stats().actor_opens_sent, 1);
        assert_eq!(lab.stats().manager_opens_sent, 3);
        assert_eq!(lab.stats().quest_opens_sent, 1);
        assert!(lab.is_actor_open(lab.joined_connections()[0], fixture.id("a").0));
        network::teardown(app.world_mut());
    }
}

fn fill_update_ring(app: &mut App, client: &Client, id: DomainId) -> Vec<Vec<u8>> {
    let before_errors = app
        .world()
        .resource::<PeerLab>()
        .stats()
        .actor_update_errors;
    let mut bytes = Vec::new();
    for step in 1..=128 {
        edit(app, id, [step as f32, 8.0, 9.0]);
        bytes.extend(tick_bytes(app, client));
        if app
            .world()
            .resource::<PeerLab>()
            .stats()
            .actor_update_errors
            > before_errors
        {
            return bytes;
        }
    }
    panic!("authored sent-record bound did not apply");
}

#[test]
fn native_translation_under_pressure_stays_due_and_binding_loss_retries_reliable_close() {
    for order in [ByteOrder::Little, ByteOrder::Big] {
        let fixture = Fixture::new();
        let (mut app, address) = fixture.app_tuned(Some(|config| config.packet.sent_capacity = 12));
        let mut client = Client::connect(&mut app, address, order);
        let opened = client.join(&mut app);
        client.settle(&mut app, &opened);
        let queued = fill_update_ring(&mut app, &client, fixture.id("a"));
        let last_native = app
            .world()
            .get::<Position>(native_entity(&app, fixture.id("a")))
            .unwrap()
            .0;
        let sent_before = app.world().resource::<PeerLab>().stats().actor_updates_sent;
        let changed = client.settle(&mut app, &queued);
        assert_eq!(
            fields(&changed, &client, false).last().unwrap().get(&6),
            Some(&movement([80.0, -last_native[0] * 10.0, 90.0]))
        );
        assert!(app.world().resource::<PeerLab>().stats().actor_updates_sent > sent_before);
        client.settle(&mut app, &changed);
        let queued = fill_update_ring(&mut app, &client, fixture.id("a"));
        let native_before = capture(app.world_mut()).unwrap().native_world;
        let proxy = app
            .world()
            .iter_entities()
            .find(|entity| {
                entity
                    .get::<NetActor>()
                    .is_some_and(|actor| actor.id == fixture.id("a"))
            })
            .unwrap()
            .id();
        app.world_mut().despawn(proxy);
        let blocked_close = tick_bytes(&mut app, &client);
        let connection = app.world().resource::<PeerLab>().joined_connections()[0];
        assert!(
            app.world()
                .resource::<PeerLab>()
                .is_actor_open(connection, fixture.id("a").0),
            "close remains pending under pressure"
        );
        assert!(app.world().resource::<PeerLab>().stats().actor_close_errors > 0);
        assert_eq!(
            app.world()
                .resource::<NativeReplicationCounts>()
                .binding_errors,
            1
        );
        assert!(!app.world().resource::<NativeReplicationCounts>().active);
        assert_eq!(
            app.world()
                .iter_entities()
                .filter(|entity| entity.contains::<NetActor>() || entity.contains::<NetViewer>())
                .count(),
            0
        );
        assert_eq!(
            capture(app.world_mut()).unwrap().native_world,
            native_before
        );
        client.accept(&blocked_close);
        let closed = client.settle(&mut app, &queued);
        assert!(
            !app.world()
                .resource::<PeerLab>()
                .is_actor_open(connection, fixture.id("a").0)
        );
        let close_count = closed
            .iter()
            .flat_map(|bytes| read_packet(bytes, &client.config()).unwrap().bunches)
            .filter(|bunch| {
                bunch.header.channel_index >= 6 && bunch.header.close && bunch.header.reliable
            })
            .count();
        assert_eq!(close_count, 1);
        assert_eq!(
            app.world().resource::<PeerLab>().stats().actor_closes_sent,
            1
        );
        client.settle(&mut app, &closed);
        assert_eq!(
            app.world().resource::<PeerLab>().stats().actor_closes_sent,
            1
        );
        assert_eq!(
            app.world().resource::<PeerLab>().stats().actor_opens_sent,
            1,
            "failed binding never reopens"
        );
        network::teardown(app.world_mut());
    }
}

#[test]
fn late_join_disconnect_reconnect_and_flag_removal_keep_only_activation_owned_state() {
    let fixture = Fixture::new();
    let (mut app, address) = fixture.app();
    edit(&mut app, fixture.id("a"), [31.0, 32.0, 33.0]);
    for _ in 0..10 {
        app.update();
    }
    assert_eq!(
        app.world()
            .iter_entities()
            .filter(|entity| entity.contains::<NetViewer>())
            .count(),
        0
    );
    let mut first = Client::connect(&mut app, address, ByteOrder::Little);
    let opened = first.join(&mut app);
    assert_eq!(
        fields(&opened, &first, true)[0].get(&6),
        Some(&movement([320.0, -310.0, 330.0]))
    );
    first.settle(&mut app, &opened);
    native_replication::update_flags(app.world_mut(), fixture.id("a"), None).unwrap();
    let reset = tick_bytes(&mut app, &first);
    assert_eq!(
        fields(&reset, &first, false),
        [Fields::from([
            (1, RepValue::Bool(false)),
            (4, RepValue::Bool(false))
        ])]
    );
    // Leave that unreliable baseline unacknowledged across disconnect.
    let old_connection = app.world().resource::<PeerLab>().joined_connections()[0];
    first.disconnect(&mut app);
    assert_eq!(app.world().resource::<PeerLab>().peer_count(), 0);
    assert!(
        !app.world()
            .resource::<PeerLab>()
            .is_actor_open(old_connection, fixture.id("a").0)
    );
    assert_eq!(
        app.world()
            .iter_entities()
            .filter(|entity| entity.contains::<NetViewer>())
            .count(),
        0
    );
    assert_eq!(
        app.world()
            .iter_entities()
            .filter(|entity| entity.contains::<NetActor>())
            .count(),
        1,
        "one driver proxy per binding, never one per old peer"
    );
    edit(&mut app, fixture.id("a"), [41.0, 42.0, 43.0]);
    let mut second = Client::connect(&mut app, address, ByteOrder::Big);
    let reopened = second.join(&mut app);
    let initial = fields(&reopened, &second, true);
    assert_eq!(initial[0].get(&6), Some(&movement([420.0, -410.0, 430.0])));
    assert!(
        !initial[0].contains_key(&1) && !initial[0].contains_key(&4),
        "a reconnect starts from its actual flag-free open"
    );
    second.settle(&mut app, &reopened);
    assert_eq!(
        app.world()
            .iter_entities()
            .filter(|entity| entity.contains::<NetViewer>())
            .count(),
        1
    );
    assert_eq!(
        app.world()
            .iter_entities()
            .filter(|entity| entity.contains::<NetActor>())
            .count(),
        1
    );
    assert_eq!(
        app.world().resource::<PeerLab>().stats().actor_opens_sent,
        2
    );
    let native = capture(app.world_mut()).unwrap().native_world;
    let closed = network::teardown(app.world_mut());
    assert_eq!(network::teardown(app.world_mut()), closed);
    native_replication::teardown(app.world_mut());
    for _ in 0..4 {
        app.update();
    }
    assert_eq!(
        app.world()
            .iter_entities()
            .filter(|entity| entity.contains::<NetViewer>()
                || entity.contains::<NetActor>()
                || entity.contains::<NetArchetype>())
            .count(),
        0
    );
    assert!(!app.world().contains_resource::<PeerLab>());
    assert!(!app.world().contains_resource::<SessionReplication>());
    assert!(!app.world().resource::<NativeReplicationCounts>().active);
    assert_eq!(capture(app.world_mut()).unwrap().native_world, native);
}

#[test]
fn idle_expiry_lab_removal_and_direct_shutdown_quiesce_viewers_and_proxies() {
    for shutdown in [false, true] {
        let fixture = Fixture::new();
        let (mut app, address) =
            fixture.app_tuned(Some(|config| config.idle_timeout = Duration::from_secs(1)));
        let mut first = Client::connect(&mut app, address, ByteOrder::Little);
        let opened = first.join(&mut app);
        first.settle(&mut app, &opened);
        // An owned, bounded idle window; no original binaries or services.
        std::thread::sleep(Duration::from_millis(1100));
        app.update();
        assert_eq!(app.world().resource::<PeerLab>().peer_count(), 0);
        assert_eq!(
            app.world()
                .iter_entities()
                .filter(|entity| entity.contains::<NetViewer>())
                .count(),
            0
        );
        let mut second = Client::connect(&mut app, address, ByteOrder::Big);
        let reopened = second.join(&mut app);
        second.settle(&mut app, &reopened);
        let native = capture(app.world_mut()).unwrap().native_world;
        if shutdown {
            assert_eq!(app.world_mut().resource_mut::<PeerLab>().shutdown(), 1);
            assert_eq!(app.world_mut().resource_mut::<PeerLab>().shutdown(), 0);
        } else {
            app.world_mut().remove_resource::<PeerLab>();
        }
        for _ in 0..4 {
            app.update();
        }
        assert_eq!(
            app.world()
                .iter_entities()
                .filter(|entity| entity.contains::<NetViewer>()
                    || entity.contains::<NetActor>()
                    || entity.contains::<NetArchetype>())
                .count(),
            0
        );
        assert!(!app.world().contains_resource::<SessionReplication>());
        assert!(!app.world().resource::<NativeReplicationCounts>().active);
        assert_eq!(capture(app.world_mut()).unwrap().native_world, native);
        network::teardown(app.world_mut());
        native_replication::teardown(app.world_mut());
    }
}

#[test]
fn unexpected_native_binding_or_proxy_component_changes_fail_closed_without_native_edits() {
    for change in 0..6 {
        let fixture = Fixture::new();
        let (mut app, address) = fixture.app();
        let mut client = Client::connect(&mut app, address, ByteOrder::Little);
        let opened = client.join(&mut app);
        client.settle(&mut app, &opened);
        let proxy = app
            .world()
            .iter_entities()
            .find(|entity| entity.contains::<NetActor>())
            .unwrap()
            .id();
        let source = native_entity(&app, fixture.id("a"));
        match change {
            0 => {
                app.world_mut().entity_mut(proxy).remove::<NetArchetype>();
            }
            1 => {
                app.world_mut().entity_mut(proxy).remove::<NetActor>();
            }
            2 => {
                app.world_mut()
                    .entity_mut(source)
                    .remove::<NativeActorIdentity>();
            }
            3 => {
                app.world_mut()
                    .entity_mut(source)
                    .insert(NativeRotation([0.0, 0.0, 1.0, 0.0]));
            }
            4 => {
                app.world_mut().entity_mut(proxy).remove::<NetSchedule>();
            }
            _ => {
                app.world_mut().remove_resource::<SessionReplication>();
            }
        }
        let retired = tick_bytes(&mut app, &client);
        assert_eq!(
            app.world().resource::<PeerLab>().stats().actor_closes_sent,
            1
        );
        assert_eq!(
            app.world()
                .resource::<NativeReplicationCounts>()
                .binding_errors,
            1
        );
        assert!(!app.world().resource::<NativeReplicationCounts>().active);
        assert!(
            retired
                .iter()
                .flat_map(|bytes| read_packet(bytes, &client.config()).unwrap().bunches)
                .any(|bunch| bunch.header.channel_index >= 6
                    && bunch.header.reliable
                    && bunch.header.close)
        );
        assert_eq!(
            app.world().get::<Position>(source),
            Some(&Position([1.0, 2.0, 3.0]))
        );
        client.settle(&mut app, &retired);
        assert_eq!(
            app.world().resource::<PeerLab>().stats().actor_closes_sent,
            1
        );
        assert_eq!(
            app.world().resource::<PeerLab>().stats().actor_opens_sent,
            1
        );
        network::teardown(app.world_mut());
    }
}

#[test]
fn late_lifecycle_components_retire_the_authored_host_and_remain_latched_after_repair() {
    use bevy::ecs::entity_disabling::{DefaultQueryFilters, Disabled};
    use icarus_bevy::native_lifecycle_runtime::NativeOwnerIncarnation;

    #[derive(Component)]
    struct Filtered;

    #[derive(Resource)]
    struct UnrelatedResource;

    fn assert_retired(app: &App) {
        assert_eq!(
            *app.world().resource::<NativeReplicationCounts>(),
            NativeReplicationCounts {
                activated: 1,
                rejected: 1,
                binding_errors: 1,
                active: false,
            }
        );
        assert!(!app.world().resource::<PeerLab>().is_admitting());
        assert!(!app.world().contains_resource::<SessionReplication>());
        assert_eq!(
            app.world()
                .iter_entities()
                .filter(|entity| entity.contains::<NetActor>()
                    || entity.contains::<NetArchetype>()
                    || entity.contains::<NetViewer>())
                .count(),
            0
        );
    }

    for order in [ByteOrder::Little, ByteOrder::Big] {
        for variant in ["ordinary", "disabled", "custom_filtered", "resource"] {
            for pressure in [false, true] {
                let fixture = Fixture::new();
                let (mut app, address) =
                    fixture.app_tuned(Some(|config| config.packet.sent_capacity = 12));
                let offender = match variant {
                    "ordinary" => app.world_mut().spawn_empty().id(),
                    "disabled" => app.world_mut().spawn(Disabled).id(),
                    "custom_filtered" => {
                        let disabling = app.world_mut().register_component::<Filtered>();
                        app.world_mut()
                            .resource_mut::<DefaultQueryFilters>()
                            .register_disabling_component(disabling);
                        app.world_mut().spawn(Filtered).id()
                    }
                    "resource" => {
                        app.insert_resource(UnrelatedResource);
                        app.world().resource_entity::<UnrelatedResource>().unwrap()
                    }
                    _ => unreachable!(),
                };
                let mut client = Client::connect(&mut app, address, order);
                let opened = client.join(&mut app);
                client.settle(&mut app, &opened);
                let connection = app.world().resource::<PeerLab>().joined_connections()[0];
                assert!(app.world().resource::<NativeReplicationCounts>().active);
                assert!(app.world().contains_resource::<SessionReplication>());
                assert!(app.world().resource::<PeerLab>().is_admitting());
                assert!(
                    app.world()
                        .resource::<PeerLab>()
                        .is_actor_open(connection, fixture.id("a").0)
                );
                assert_eq!(
                    app.world()
                        .iter_entities()
                        .filter(|entity| entity.contains::<NetViewer>())
                        .count(),
                    1
                );
                // Several clean host ticks precede first registration: a cached
                // absent ComponentId must not hide a later lifecycle component.
                assert!(
                    app.world()
                        .component_id::<NativeOwnerIncarnation>()
                        .is_none()
                );
                let queued = if pressure {
                    fill_update_ring(&mut app, &client, fixture.id("a"))
                } else {
                    Vec::new()
                };
                let native_before = capture(app.world_mut()).unwrap().native_world;
                app.world_mut()
                    .entity_mut(offender)
                    .insert(NativeOwnerIncarnation(1));

                // Observe retirement on the very next host tick, even when the
                // reliable close cannot yet be framed under transport pressure.
                app.update();
                let retired = drain(&client.socket);
                assert_retired(&app);
                let lab = app.world().resource::<PeerLab>();
                if pressure {
                    assert!(lab.is_actor_open(connection, fixture.id("a").0));
                    assert!(lab.stats().actor_close_errors > 0);
                    assert_eq!(lab.stats().actor_closes_sent, 0);
                } else {
                    assert!(!lab.is_actor_open(connection, fixture.id("a").0));
                    assert_eq!(lab.stats().actor_closes_sent, 1);
                }
                assert_eq!(
                    app.world().get::<NativeOwnerIncarnation>(offender),
                    Some(&NativeOwnerIncarnation(1))
                );

                // Repair empties the offending archetype but must not revive
                // the retired plan or cancel its still-pending channel close.
                let bad_archetype = app.world().entity(offender).archetype().id();
                app.world_mut()
                    .entity_mut(offender)
                    .remove::<NativeOwnerIncarnation>();
                assert!(app.world().archetypes()[bad_archetype].is_empty());
                assert_eq!(
                    capture(app.world_mut()).unwrap().native_world,
                    native_before
                );
                if variant == "resource" {
                    assert!(app.world().contains_resource::<UnrelatedResource>());
                }
                client.accept(&retired);
                let closed = client.settle(&mut app, &queued);
                assert_retired(&app);
                assert!(
                    !app.world()
                        .resource::<PeerLab>()
                        .is_actor_open(connection, fixture.id("a").0)
                );
                assert_eq!(
                    retired
                        .iter()
                        .chain(&closed)
                        .flat_map(|bytes| read_packet(bytes, &client.config()).unwrap().bunches)
                        .filter(|bunch| bunch.header.channel_index == 6
                            && bunch.header.reliable
                            && bunch.header.close)
                        .count(),
                    1
                );
                client.settle(&mut app, &closed);
                for _ in 0..4 {
                    let bytes = client.step(&mut app);
                    client.accept(&bytes);
                    assert_retired(&app);
                    let lab = app.world().resource::<PeerLab>();
                    assert_eq!(lab.stats().actor_opens_sent, 1);
                    assert_eq!(lab.stats().actor_closes_sent, 1);
                    assert!(!lab.is_actor_open(connection, fixture.id("a").0));
                }
                assert_eq!(
                    capture(app.world_mut()).unwrap().native_world,
                    native_before
                );
                network::teardown(app.world_mut());
            }
        }
    }
}

#[test]
fn absent_option_disabled_network_and_cli_configuration_preserve_legacy_behavior() {
    let fixture = Fixture::new();
    let mut config = fixture.config.clone();
    config.network.authored_replication = None;
    let mut app = build_app(&config, &fixture.snapshot).unwrap();
    let bound = network::bind(&config.network).unwrap().unwrap();
    let address = network::install_configured(&mut app, bound, &config.network).unwrap();
    app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_millis(
        34,
    )));
    let mut client = Client::connect(&mut app, address, ByteOrder::Little);
    let opened = client.join(&mut app);
    assert!(actor_bodies(&opened, &client).is_empty());
    assert_eq!(
        app.world().resource::<PeerLab>().stats().actor_opens_sent,
        0
    );
    assert!(!app.is_plugin_added::<ReplicationPlugin>());
    assert!(!app.world().contains_resource::<NativeReplicationCounts>());
    assert_eq!(
        app.world()
            .iter_entities()
            .filter(|entity| entity.contains::<NetActor>())
            .count(),
        0
    );
    assert_eq!(
        capture(app.world_mut()).unwrap().native_world,
        fixture.snapshot.native_world
    );
    network::teardown(app.world_mut());
    config.network.enabled = false;
    config.network.authored_replication = Some(fixture.dir.path().join("does-not-exist.json"));
    assert!(
        network::bind(&config.network).unwrap().is_none(),
        "disabled mode never opens the declaration"
    );
    let options = ConfigOptions {
        authored_replication: Some(fixture.dir.path().join("chosen.json")),
        ..Default::default()
    };
    assert_eq!(
        options.resolve().unwrap().network.authored_replication,
        options.authored_replication
    );
}

/// The test owns this exact child, always reaps it, and kills only that PID on
/// timeout/panic. Root runs this source-authored smoke under its execution scope.
struct OwnedChild(std::process::Child);

impl Drop for OwnedChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn bounded_native_binary_run_uses_the_same_authored_configuration_path() {
    use std::process::{Command, Stdio};
    let fixture = Fixture::new();
    fixture.write_declaration();
    let mut store = SnapshotStore::open(&fixture.config.data_dir).unwrap();
    store.save_new(&fixture.snapshot).unwrap();
    drop(store);
    let config_path = fixture.dir.path().join("server.toml");
    fs::write(&config_path, toml::to_string(&fixture.config).unwrap()).unwrap();
    let log_path = fixture.dir.path().join("run.log");
    let mut child = OwnedChild(
        Command::new(env!("CARGO_BIN_EXE_icarus-server"))
            .arg("run")
            .arg("--config")
            .arg(&config_path)
            .args(["--compressed", "--max-ticks", "3"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(fs::File::create(&log_path).unwrap())
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    let status = loop {
        if let Some(status) = child.0.try_wait().unwrap() {
            break status;
        }
        assert!(
            Instant::now() < deadline,
            "bounded authored native run exceeded ten seconds"
        );
        std::thread::sleep(Duration::from_millis(10));
    };
    let log = fs::read_to_string(&log_path).unwrap();
    assert!(status.success(), "{log}");
    let rows: Vec<Value> = log
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let listening = rows
        .iter()
        .find(|row| row["event"] == "network_listening")
        .unwrap();
    assert_eq!(listening["native_replication"]["activated"], 1);
    assert_eq!(listening["native_replication"]["active"], true);
    assert_eq!(
        rows.iter().find(|row| row["event"] == "stopped").unwrap()["native_replication"]["active"],
        false
    );
    for secret in [
        "IndependentBox",
        "authored",
        "catalog_id",
        "rotation",
        "placement",
    ] {
        assert!(
            !log.contains(secret),
            "native declaration/state data must not enter run logs"
        );
    }
    let store = SnapshotStore::open(&fixture.config.data_dir).unwrap();
    let saved = store.load().unwrap().unwrap().snapshot;
    assert_eq!(saved.native_world, fixture.snapshot.native_world);
}

#[test]
#[ignore = "bounded authored idle-cost measurement; explicit run only"]
fn authored_host_idle_tick_cost() -> anyhow::Result<()> {
    use std::path::{Path, PathBuf};

    const COUNTS: [usize; 3] = [1, 64, 1024];
    const WARMUPS: usize = 8;
    const SAMPLES: usize = 32;
    const TOTAL_BUDGET: Duration = Duration::from_secs(10);

    fn validate_parameters(counts: &[usize], warmups: usize, samples: usize, budget: Duration) {
        assert_eq!(counts, [1, 64, 1024]);
        assert_eq!(warmups, 8);
        assert_eq!(samples, 32);
        assert_eq!(budget, Duration::from_secs(10));
        assert!(
            counts
                .iter()
                .all(|count| (1..=MAX_BINDINGS).contains(count))
        );
        if !cfg!(debug_assertions) {
            panic!("authored idle-cost selection requires the debug test profile");
        }
    }

    fn check_budget(started: Instant, budget: Duration) {
        assert!(
            started.elapsed() < budget,
            "authored idle-cost cooperative total budget exhausted; retain all artifacts"
        );
    }

    // Reuse the existing authored pose/binding/release helpers, but not Fixture's
    // TempDir ownership: its implicit Drop removes artifacts even on unwind.
    struct IdleCostInput {
        count: usize,
        catalog: Value,
        snapshot: WorldSnapshot,
        declaration: Value,
    }

    fn input(count: usize) -> anyhow::Result<IdleCostInput> {
        let mut placements = Vec::with_capacity(count);
        let mut bindings = Vec::with_capacity(count);
        for index in 0..count {
            let placement = format!("idle_{index:04}");
            // At most a 32 x 32 grid: coordinates 0..=62 m, z=2 m.
            // Unit-scale half-meter bounds fit wholly inside [-128,128]^3.
            let x = (index % 32) as f32 * 2.0;
            let y = (index / 32) as f32 * 2.0;
            authored_movement([y * 10.0, -x * 10.0, 20.0], [0.0; 3], [0.0; 3])?;
            placements.push(json!({
                "key": placement, "class": "spatial", "tile": "tile",
                "transform": pose_json([x, y, 2.0])
            }));
            bindings.push(binding_json(&placement));
        }
        let catalog = json!({
            "native_format": 1, "frame": "right_handed_z_up_meters",
            "classes": [{"key": "spatial", "kind": "spatial_object_v1", "variables": [],
                "local_bounds": {"min": [-0.5, -0.5, -0.5], "max": [0.5, 0.5, 0.5]}}],
            "maps": [{"key": "authored", "tiles": [{"key": "tile",
                "bounds": {"min": [-128.0, -128.0, -128.0], "max": [128.0, 128.0, 128.0]}}],
                "placements": placements}]
        });
        let definition: NativeCatalogDefinition = serde_json::from_value(catalog.clone())?;
        let prepared = prepare_authored_world(
            Arc::new(CompiledNativeCatalog::compile(definition)?),
            "authored",
        )?;
        let snapshot = prepared.initial_snapshot();
        snapshot.validate()?;
        assert_eq!(snapshot.schema_version, 4);
        assert_eq!(snapshot.tick, 0);
        assert!(snapshot.preserved_prospect.is_none());
        assert_eq!(snapshot.native_world.as_ref().unwrap().actors.len(), count);
        let declaration = json!({
            "format": 1, "catalog_id": snapshot.native_world.as_ref().unwrap().catalog_id,
            "map": "authored", "frame": {"axes": [2, -1, 3], "units": 10.0},
            "first_channel": 6,
            "schedule": {
                "connection_budget": 32, "max_consider_list": 1024, "max_connections": 8,
                "max_viewers_per_connection": 1, "net_server_max_tick_rate": 30,
                "max_connections_to_tick_per_server_frame": 0,
                "relevant_timeout_seconds": 5.0, "spawn_priority_seconds": 1.0,
                "use_adaptive_net_update_frequency": false, "seed": 19
            },
            "bindings": bindings
        });
        AuthoredReplication::parse(&serde_json::to_string(&declaration)?)?;
        Ok(IdleCostInput {
            count,
            catalog,
            snapshot,
            declaration,
        })
    }

    fn write_json(path: impl AsRef<Path>, value: &impl serde::Serialize) -> anyhow::Result<()> {
        fs::write(path, serde_json::to_vec(value)?)?;
        Ok(())
    }

    fn host_configuration(dir: &Path) -> ServerConfig {
        ServerConfig {
            data_dir: dir.join("state"),
            network: NetworkConfig {
                enabled: true,
                port: 0,
                release_profile: Some(dir.join("release.toml")),
                authored_replication: Some(dir.join("replication.json")),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    // Read only this freshly created authored store. Bound both entry count and
    // bytes; no recursion, symlinks, original/private inputs, or hashes.
    fn store_bytes(path: &Path) -> anyhow::Result<BTreeMap<PathBuf, Vec<u8>>> {
        let mut files = BTreeMap::new();
        let mut total = 0usize;
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            assert!(files.len() < 4, "unexpected authored store entry count");
            assert!(
                entry.file_type()?.is_file(),
                "unexpected authored store entry type"
            );
            assert!(entry.metadata()?.len() <= 4 * 1024 * 1024);
            let bytes = fs::read(entry.path())?;
            total = total.checked_add(bytes.len()).unwrap();
            assert!(total <= 8 * 1024 * 1024);
            files.insert(PathBuf::from(entry.file_name()), bytes);
        }
        assert_eq!(files.len(), 3); // server.lock, manifest.json, snapshot-1.json
        Ok(files)
    }

    fn verify_idle(app: &mut App, input: &IdleCostInput, host: bool) -> anyhow::Result<()> {
        let lab = app.world().resource::<PeerLab>();
        assert!(lab.is_admitting());
        assert_eq!(lab.peer_count(), 0);
        assert_eq!(lab.peak_peer_count(), 0);
        assert_eq!(lab.stats().received, 0);
        assert_eq!(lab.stats().peers_created, 0);
        assert_eq!(lab.stats().io_errors, 0);
        assert!(lab.joined_connections().is_empty());
        let entities = app.world().iter_entities();
        assert_eq!(
            entities
                .filter(|entity| entity.contains::<NativeActorIdentity>())
                .count(),
            input.count
        );
        assert_eq!(
            app.world()
                .iter_entities()
                .filter(|entity| entity.contains::<DomainId>())
                .count(),
            input.count
        );
        assert_eq!(
            app.world()
                .iter_entities()
                .filter(|entity| entity.contains::<NetViewer>())
                .count(),
            0
        );
        let proxies: Vec<_> = app
            .world()
            .iter_entities()
            .filter(|entity| entity.contains::<NetActor>())
            .collect();
        assert_eq!(proxies.len(), if host { input.count } else { 0 });
        assert!(proxies.iter().all(|entity| {
            !entity.contains::<DomainId>()
                && !entity.contains::<NativeActorIdentity>()
                && entity.contains::<NetArchetype>()
                && entity.contains::<NetSchedule>()
                && entity.contains::<Position>()
                && entity.contains::<NetActorFlags>()
        }));
        assert_eq!(app.is_plugin_added::<ReplicationPlugin>(), host);
        assert_eq!(app.world().contains_resource::<SessionReplication>(), host);
        if host {
            let counts = app.world().resource::<NativeReplicationCounts>();
            assert!(counts.active);
            assert_eq!(counts.activated, input.count);
            assert_eq!(counts.rejected, 0);
            assert_eq!(counts.binding_errors, 0);
            // The scheduler does not increment its connection frame without peers.
            let decisions = &app.world().resource::<ReplicationFrame>().decisions;
            assert_eq!(decisions.replication_frame, 0);
        } else {
            assert!(!app.world().contains_resource::<NativeReplicationCounts>());
            assert!(!app.world().contains_resource::<ReplicationFrame>());
        }
        let mut snapshot = capture(app.world_mut())?;
        assert_eq!(
            snapshot.tick,
            app.world().resource::<SimulationClock>().tick
        );
        snapshot.tick = input.snapshot.tick;
        // All persistent fields must match; only the verified clock may advance.
        assert!(
            snapshot == input.snapshot,
            "authored persistent domain state changed"
        );
        Ok(())
    }

    fn step(
        app: &mut App,
        input: &IdleCostInput,
        host: bool,
        measured: bool,
        started: Instant,
        budget: Duration,
    ) -> anyhow::Result<Option<u64>> {
        let before = app.world().resource::<SimulationClock>().tick;
        check_budget(started, budget); // BEFORE every timed update (and warmup).
        let elapsed = if measured {
            let sample_started = Instant::now();
            app.update(); // Entire App update: every schedule, fixed work and lab poll.
            Some(sample_started.elapsed())
        } else {
            app.update();
            None
        };
        check_budget(started, budget); // Immediately after update/timer; cooperative.
        assert_eq!(
            app.world()
                .resource::<SimulationClock>()
                .tick
                .checked_sub(before),
            Some(1)
        );
        verify_idle(app, input, host)?; // All correctness checks OUTSIDE timing.
        check_budget(started, budget);
        Ok(elapsed.map(|duration| u64::try_from(duration.as_nanos()).unwrap()))
    }

    validate_parameters(&COUNTS, WARMUPS, SAMPLES, TOTAL_BUDGET);
    let started = Instant::now(); // Total includes preparation and correctness work.
    // Preflight the complete finite selection before fixtures, files or sockets.
    let preflight_host = host_configuration(Path::new("work/authored-idle-cost-preflight"));
    preflight_host.validate()?; // Validation only: does not open these paths.
    let mut preflight_baseline = preflight_host.clone();
    preflight_baseline.network.authored_replication = None;
    preflight_baseline.validate()?;
    let inputs = COUNTS
        .into_iter()
        .map(input)
        .collect::<anyhow::Result<Vec<_>>>()?;
    check_budget(started, TOTAL_BUDGET);
    let run_dir = PathBuf::from(
        std::env::var_os("ICARUS_AUTHORED_IDLE_COST_DIR").ok_or_else(|| {
            anyhow::anyhow!("set an absent idle-cost child inside an owned work case")
        })?,
    );
    anyhow::ensure!(
        run_dir.is_absolute() && run_dir.file_name().is_some(),
        "idle-cost child must be absolute"
    );
    let parent = run_dir
        .parent()
        .ok_or_else(|| anyhow::anyhow!("idle-cost parent missing"))?;
    let canonical_parent = fs::canonicalize(parent)?;
    let work = fs::canonicalize(Path::new(env!("CARGO_MANIFEST_DIR")).join("work"))?;
    anyhow::ensure!(
        canonical_parent.starts_with(&work)
            && canonical_parent != work
            && parent == canonical_parent,
        "idle-cost parent must be a canonical owned work case"
    );
    // Caller prepares the parent case/scope. Refuse an existing child; never replace.
    fs::create_dir(&run_dir)?;
    write_json(
        run_dir.join("case.json"),
        &json!({
            "state": "open", "owner": "authored_host_idle_tick_cost",
            "purpose": "Bounded debug whole-App-update authored idle/no-peer measurement",
            "counts": COUNTS, "warmups": WARMUPS, "samples": SAMPLES,
            "cooperative_seconds": 10, "required_outer_timeout_seconds": 60,
            "retention": "all artifacts on success and error; Root closes interrupted case"
        }),
    )?;
    let mut rows = Vec::with_capacity(COUNTS.len() * 2);
    for input in &inputs {
        check_budget(started, TOTAL_BUDGET);
        let dir = run_dir.join(format!("actors-{}", input.count));
        fs::create_dir(&dir)?;
        fs::write(dir.join("release.toml"), RELEASE)?;
        write_json(dir.join("catalog.json"), &input.catalog)?;
        write_json(dir.join("initial-snapshot.json"), &input.snapshot)?;
        write_json(dir.join("replication.json"), &input.declaration)?;
        let host_config = host_configuration(&dir);
        let mut baseline_config = host_config.clone();
        baseline_config.network.authored_replication = None;
        host_config.validate()?;
        baseline_config.validate()?;
        assert!(host_config.network.data_pak.is_none());
        let mut matched = host_config.clone();
        matched.network.authored_replication = None;
        assert!(toml::to_string(&matched)? == toml::to_string(&baseline_config)?);
        fs::write(dir.join("host.toml"), toml::to_string(&host_config)?)?;
        fs::write(
            dir.join("baseline.toml"),
            toml::to_string(&baseline_config)?,
        )?;
        let mut store = SnapshotStore::open(&host_config.data_dir)?;
        store.save_new(&input.snapshot)?; // Setup only; no worker/autosave installed.
        assert!(store.load()?.unwrap().snapshot == input.snapshot);
        let unchanged_store = store_bytes(&host_config.data_dir)?;
        for (mode, config, host) in [
            ("baseline_lab_option_absent", &baseline_config, false),
            ("explicit_host_bridge", &host_config, true),
        ] {
            check_budget(started, TOTAL_BUDGET);
            let mut app = build_app(config, &input.snapshot)?;
            let bound = network::bind(&config.network)?.unwrap();
            let address = network::install_configured(&mut app, bound, &config.network)?;
            assert!(address.ip().is_loopback());
            app.insert_resource(TimeUpdateStrategy::FixedTimesteps(1));
            verify_idle(&mut app, input, host)?;
            // Bevy's first update initializes time with zero delta. This priming
            // call is separate from the eight actual one-tick warmups.
            let before = app.world().resource::<SimulationClock>().tick;
            check_budget(started, TOTAL_BUDGET);
            app.update();
            check_budget(started, TOTAL_BUDGET);
            assert_eq!(app.world().resource::<SimulationClock>().tick, before);
            verify_idle(&mut app, input, host)?;
            for _ in 0..WARMUPS {
                step(&mut app, input, host, false, started, TOTAL_BUDGET)?;
            }
            let mut nanos = Vec::with_capacity(SAMPLES);
            for _ in 0..SAMPLES {
                nanos.push(step(&mut app, input, host, true, started, TOTAL_BUDGET)?.unwrap());
            }
            assert_eq!(nanos.len(), SAMPLES);
            assert_eq!(
                app.world().resource::<SimulationClock>().tick,
                (WARMUPS + SAMPLES) as u64
            );
            assert!(store.load()?.unwrap().snapshot == input.snapshot);
            assert!(
                store_bytes(&config.data_dir)? == unchanged_store,
                "authored store bytes changed"
            );
            let raw_samples_ns = nanos.clone();
            nanos.sort_unstable();
            // Even-count median: integer floor of the mean of samples 16 and 17.
            let median = nanos[SAMPLES / 2 - 1] / 2
                + nanos[SAMPLES / 2] / 2
                + (nanos[SAMPLES / 2 - 1] % 2 + nanos[SAMPLES / 2] % 2) / 2;
            rows.push(json!({
                "mode": mode, "profile": "debug", "workload": "idle/no-peer",
                "actors": input.count, "declared_bindings": input.count,
                "bound_actors": if host { input.count } else { 0 },
                "active_native_actors": input.count,
                "active_proxies": if host { input.count } else { 0 },
                "peers": 0, "warmups": WARMUPS, "count": nanos.len(),
                "verified_tick_delta_per_update": 1,
                "median_ns": median, "p95_ns": nanos[(95 * SAMPLES).div_ceil(100) - 1],
                "max_ns": nanos[SAMPLES - 1], "raw_samples_ns": raw_samples_ns
            }));
            // Ordinary App/socket Drop is retained, including during unwinding.
            // No teardown/drop saves state, launches a child, or removes files.
            drop(app);
            check_budget(started, TOTAL_BUDGET);
        }
        assert!(store_bytes(&host_config.data_dir)? == unchanged_store);
        drop(store); // Releases advisory lock; retains every file.
    }
    assert_eq!(rows.len(), 6);
    let summary = json!({
        "measurement": "whole_app_update_elapsed_ns", "profile": "debug",
        "workload": "idle/no-peer", "complete": true,
        "cooperative_total_limit_seconds": 10, "required_outer_timeout_seconds": 60,
        "rows": rows
    });
    check_budget(started, TOTAL_BUDGET);
    write_json(run_dir.join("summary.json"), &summary)?;
    check_budget(started, TOTAL_BUDGET);
    write_json(
        run_dir.join("case.json"),
        &json!({
            "state": "closed", "owner": "authored_host_idle_tick_cost",
            "purpose": "Bounded debug whole-App-update authored idle/no-peer measurement",
            "retention": "all authored fixtures and stores retained", "aggregate_rows": 6
        }),
    )?;
    check_budget(started, TOTAL_BUDGET);
    println!("{}", serde_json::to_string(&summary)?);
    check_budget(started, TOTAL_BUDGET);
    Ok(())
}

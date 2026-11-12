//! Seeded fault campaign over the whole native join: authored loopback
//! clients run Hello, Login, Join, initialisation, the friends answer and
//! synthetic moves against the production bind (records, possession
//! properties and the frozen-movement release on, move acknowledgements and
//! spawn points per scenario, level visibility off) through a deterministic
//! fault model that drops, duplicates and delays datagrams each way.
//!
//! Time: the lab polls with its own clock, as in production. A run lasts far
//! less than the 30 s session timeout, so no timer can fire by itself; the
//! designed timer closes are reached only by explicit instants derived from
//! the peer's own stamps ([`expire_silent`]). Fault decisions are keyed on the
//! client step (client to server) and on the reliable bunch and its
//! transmission, the release copy or the move acknowledgement (server to
//! client); only clock-driven background datagrams (ACKs, GameState updates)
//! take a separate keyed stream, so their real-time cadence moves no other
//! decision. Authored client, loopback; not a stock client.
use std::collections::{BTreeMap, BTreeSet, VecDeque};

use super::super::super::{
    actor_channel::{
        ActorBunchBody, BlockTarget, ContentBlock, NetGuidRef, SubObjectClass,
        encode_actor_bunch_body, read_actor_bunch_body,
    },
    archive::ByteOrder,
    character_move::read_good_move_ack,
    control_messages::{
        ControlCodec, ControlMessage, Decoded, NetIdType, NetIdValue, UniqueNetId,
        encode_control_message, read_control_messages,
    },
    handshake::initial_request,
    packet::{
        BitWriter, Bunch, BunchHeader, Name, Packet, PacketConfig, Payload, Receiver, read_packet,
        write_packet,
    },
    packet_lab::{MAX_PENDING_RELIABLE, client_cookie_seeds},
    pawn_open::PAWN_CHANNEL_INDEX,
    probe_initialise::{
        ActorBunch, ActorChannelTracker, authored_character_profile, authored_user_profile,
        begin_player_initialisation_field, bestiary_response_payload, friends_list_updated_field,
        initialise_codec, loadout_response_payload,
    },
    rep_layout::RepLayoutCodec,
    rep_rpc::{
        self, BACKEND_PROXY_FIELD_INDEX_BOUND, BESTIARY_MANAGER_FIELD_INDEX_BOUND,
        BP_ICARUS_PLAYER_CHARACTER_SURVIVAL_FIELD_INDEX_BOUND as PAWN_FIELD_BOUND,
        BP_ICARUS_PLAYER_CONTROLLER_SURVIVAL_FIELD_INDEX_BOUND as CONTROLLER_FIELD_BOUND,
        CLIENT_MOVE_RESPONSE_PACKED_FIELD_INDEX, CLIENT_PREPARE_LOADOUT,
        CLIENT_REQUEST_BESTIARY_DATA, CLIENT_REQUEST_GET_CHARACTER_PROFILE,
        CLIENT_REQUEST_GET_USER_PROFILE, FieldHeader, PLAYER_DATA_FIELD_INDEX_BOUND, RpcArgument,
        RpcItem, read_field_header, write_field_header,
    },
    sequencing::{PacketSequencer, ReliableChannel, ReliableReceive, wire_reliable_sequence},
    session_open::{ACTOR_CHANNEL_NAME_INDEX, PLAYER_CONTROLLER_CHANNEL_INDEX},
};
use super::move_acks::{authored_move, move_bunch_payload, unreliable_bunch};
use super::*;

/// The control channel's hard-coded name index (F-525).
const CONTROL_CHANNEL_NAME: u32 = 255;
/// The authored client's sent-record ring: large, so the client keeps sending
/// through long outages (the stock client's window is not established) and
/// only the server's own bounds decide what a long outage does.
const CLIENT_SENT_CAPACITY: usize = 8_192;
/// Payload bits of new and resent reliable bunches packed into one client
/// packet (one bunch always fits; the single-bunch limit is 7,625 bits).
const CLIENT_PACKET_BITS: usize = 6_000;
/// The authored character slot every client begins with.
const CHR_SLOT: i32 = 3;
/// Client steps between synthetic moves.
const MOVE_EVERY: u64 = 3;
/// Steps a healed link gets to finish what faults left open.
const HEALED_STEPS: u64 = 1_500;
/// Steps every run spends on a healed link before timers are run out.
const MIN_HEALED_STEPS: u64 = 100;
/// Steps after completion in which nothing may be sent twice.
const QUIET_STEPS: u64 = 60;
/// Events kept for a failure report.
const TRACE: usize = 60;
/// Injected time per step for the move-acknowledgement pass, so the 100 ms
/// interval admits an acknowledgement every ten steps however fast steps run.
const MOVE_ACK_STEP: Duration = Duration::from_millis(10);

// ---------------------------------------------------------------- faults

/// One direction-independent link: per-mille probabilities and the longest
/// delay in client steps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Link {
    server_to_client_drop: u64,
    client_to_server_drop: u64,
    duplicate: u64,
    reorder: u64,
    max_delay: u64,
}

impl Link {
    const CLEAN: Self = Self {
        server_to_client_drop: 0,
        client_to_server_drop: 0,
        duplicate: 0,
        reorder: 0,
        max_delay: 0,
    };
    /// Up to what ordinary links see: 20% loss each way.
    const REAL: Self = Self {
        server_to_client_drop: 200,
        client_to_server_drop: 200,
        duplicate: 50,
        reorder: 100,
        max_delay: 3,
    };
    const MILD: Self = Self {
        server_to_client_drop: 100,
        client_to_server_drop: 100,
        duplicate: 30,
        reorder: 80,
        max_delay: 2,
    };
    /// The harsher tier: 40% to 45% loss each way, frequent reordering.
    const HARSH: Self = Self {
        server_to_client_drop: 400,
        client_to_server_drop: 400,
        duplicate: 100,
        reorder: 250,
        max_delay: 6,
    };
    const HEAVY: Self = Self {
        server_to_client_drop: 450,
        client_to_server_drop: 450,
        duplicate: 100,
        reorder: 300,
        max_delay: 8,
    };
}

/// SplitMix64's finaliser over the seed and the decision key.
fn keyed(seed: u64, key: &[u64]) -> u64 {
    let mut state = seed ^ 0x9e37_79b9_7f4a_7c15;
    for part in key {
        state = state.wrapping_add(0x9e37_79b9_7f4a_7c15) ^ part;
        let mut z = state;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        state = z ^ (z >> 31);
    }
    state
}

/// Delivery delays for one datagram (empty: dropped; two: duplicated).
fn decide(seed: u64, link: Link, drop: u64, key: &[u64]) -> Vec<u64> {
    let draw = |salt: u64| {
        let mut salted = key.to_vec();
        salted.push(salt);
        keyed(seed, &salted)
    };
    if draw(1) % 1000 < drop {
        return Vec::new();
    }
    let delay = |salt: u64| {
        if link.max_delay > 0 && draw(salt) % 1000 < link.reorder {
            1 + draw(salt + 1) % link.max_delay
        } else {
            0
        }
    };
    let mut delays = vec![delay(2)];
    if draw(4) % 1000 < link.duplicate {
        delays.push(delay(5));
    }
    delays
}

const SERVER_TO_CLIENT: u64 = 1;
const CLIENT_TO_SERVER: u64 = 2;
const KIND_RELIABLE: u64 = 1;
const KIND_RELEASE: u64 = 2;
const KIND_MOVE_ACK: u64 = 3;
const KIND_BACKGROUND: u64 = 4;

/// A datagram waiting in the fault model.
struct InFlight {
    due: u64,
    order: u64,
    bytes: Vec<u8>,
    /// Client to server: the move timestamp it carries.
    moved: Option<f32>,
    /// Server to client: it carries a release copy.
    release: bool,
}

// ---------------------------------------------------------------- scenarios

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tier {
    /// Every client must complete while the faults last.
    Real,
    /// Complete, possibly after the link heals, or be closed by a designed,
    /// counted path.
    Harsh,
    /// An outage then a healed link: every client completes, none is closed.
    Recover,
}

/// A seeded stretch in which one or both directions lose everything, as a
/// link outage or handover does.
#[derive(Clone, Copy, Debug)]
struct Outage {
    server_to_client: bool,
    client_to_server: bool,
    /// The outage starts at a seeded step below this bound.
    start_before: u64,
    steps: u64,
}

#[derive(Clone, Copy, Debug)]
struct Scenario {
    name: &'static str,
    tier: Tier,
    clients: usize,
    links: [Link; 2],
    release_delay_ms: u64,
    move_acks: bool,
    spawn_points: usize,
    /// One client goes silent both ways from a seeded step below this bound.
    blackout_before: Option<u64>,
    outage: Option<Outage>,
    steps: u64,
}

const SCENARIOS: [Scenario; 9] = [
    Scenario {
        name: "real-one",
        tier: Tier::Real,
        clients: 1,
        links: [Link::REAL, Link::REAL],
        release_delay_ms: 0,
        move_acks: true,
        spawn_points: 0,
        blackout_before: None,
        outage: None,
        steps: 1_500,
    },
    Scenario {
        name: "real-two-delay-spawn",
        tier: Tier::Real,
        clients: 2,
        links: [Link::REAL, Link::MILD],
        release_delay_ms: 20,
        move_acks: true,
        spawn_points: 3,
        blackout_before: None,
        outage: None,
        steps: 2_000,
    },
    Scenario {
        name: "real-two-no-move-acks",
        tier: Tier::Real,
        clients: 2,
        links: [Link::REAL, Link::REAL],
        release_delay_ms: 0,
        move_acks: false,
        spawn_points: 1,
        blackout_before: None,
        outage: None,
        steps: 2_000,
    },
    Scenario {
        name: "harsh-one",
        tier: Tier::Harsh,
        clients: 1,
        links: [Link::HARSH, Link::HARSH],
        release_delay_ms: 0,
        move_acks: true,
        spawn_points: 2,
        blackout_before: None,
        outage: None,
        steps: 2_000,
    },
    Scenario {
        name: "harsh-two-one-heavy",
        tier: Tier::Harsh,
        clients: 2,
        links: [Link::MILD, Link::HEAVY],
        release_delay_ms: 10,
        move_acks: true,
        spawn_points: 2,
        blackout_before: None,
        outage: None,
        steps: 2_500,
    },
    Scenario {
        name: "blackout-two",
        tier: Tier::Harsh,
        clients: 2,
        links: [Link::REAL, Link::REAL],
        release_delay_ms: 0,
        move_acks: true,
        spawn_points: 0,
        blackout_before: Some(40),
        outage: None,
        steps: 1_500,
    },
    Scenario {
        name: "outage-down-short",
        tier: Tier::Recover,
        clients: 2,
        links: [Link::MILD, Link::MILD],
        release_delay_ms: 0,
        move_acks: true,
        spawn_points: 0,
        blackout_before: None,
        outage: Some(Outage {
            server_to_client: true,
            client_to_server: false,
            start_before: 40,
            steps: 150,
        }),
        steps: 2_000,
    },
    Scenario {
        name: "outage-up-short",
        tier: Tier::Recover,
        clients: 2,
        links: [Link::MILD, Link::MILD],
        release_delay_ms: 0,
        move_acks: true,
        spawn_points: 0,
        blackout_before: None,
        outage: Some(Outage {
            server_to_client: false,
            client_to_server: true,
            start_before: 40,
            steps: 200,
        }),
        steps: 2_000,
    },
    Scenario {
        name: "outage-both",
        tier: Tier::Recover,
        clients: 2,
        links: [Link::MILD, Link::MILD],
        release_delay_ms: 0,
        move_acks: true,
        spawn_points: 0,
        blackout_before: None,
        outage: Some(Outage {
            server_to_client: true,
            client_to_server: true,
            start_before: 40,
            steps: 120,
        }),
        steps: 2_000,
    },
];

/// Outages longer than 256 packets one way, which once closed (N-020: more
/// than 256 lost or unanswered client packets) or hung (N-021: a full sent
/// ring) the session. Run by [`long_outages_recover_after_the_link_heals`].
const LONG_OUTAGES: [Scenario; 2] = [
    Scenario {
        name: "outage-down-long",
        tier: Tier::Recover,
        clients: 2,
        links: [Link::MILD, Link::MILD],
        release_delay_ms: 0,
        move_acks: true,
        spawn_points: 0,
        blackout_before: None,
        outage: Some(Outage {
            server_to_client: true,
            client_to_server: false,
            start_before: 40,
            steps: 300,
        }),
        steps: 2_000,
    },
    Scenario {
        name: "outage-up-long",
        tier: Tier::Recover,
        clients: 2,
        links: [Link::MILD, Link::MILD],
        release_delay_ms: 0,
        move_acks: true,
        spawn_points: 0,
        blackout_before: None,
        outage: Some(Outage {
            server_to_client: false,
            client_to_server: true,
            start_before: 40,
            steps: 300,
        }),
        steps: 2_000,
    },
];

fn scenario(name: &str) -> Scenario {
    *SCENARIOS
        .iter()
        .chain(&LONG_OUTAGES)
        .find(|scenario| scenario.name == name)
        .unwrap_or_else(|| panic!("unknown scenario {name}"))
}

fn spawn_point(index: usize) -> [f32; 3] {
    let index = index as f32;
    [
        1_000.0 + 250.0 * index,
        -2_000.0 - 125.0 * index,
        300.0 + 10.0 * index,
    ]
}

fn campaign_lab(scenario: &Scenario) -> PeerLab {
    let points = (0..scenario.spawn_points)
        .map(|index| {
            let [x, y, z] = spawn_point(index);
            crate::spawn_placement::SpawnPoint {
                x: f64::from(x),
                y: f64::from(y),
                z: f64::from(z),
                yaw: Some(15.0 * index as f64),
            }
        })
        .collect();
    let config = crate::config::NetworkConfig {
        server_frozen_movement_release_delay_ms: scenario.release_delay_ms,
        spawn: crate::spawn_placement::SpawnConfig { points },
        ..super::move_acks::release_toml_config(scenario.move_acks)
    };
    assert!(config.connected_player_records);
    assert!(config.controller_possession_properties);
    assert!(config.server_frozen_movement_release);
    assert!(!config.level_visibility_actors);
    crate::network::bind(&config).unwrap().unwrap().lab
}

// ---------------------------------------------------------------- client

/// One reliable bunch the client owes the server until a positive ACK.
struct OutReliable {
    bunch: Bunch,
    /// The packet carrying the latest copy.
    packet: Option<u32>,
    resend: bool,
}

/// Where a controller subobject request goes.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Component {
    BackendProxy,
    PlayerData,
    Bestiary,
}

#[derive(Default, Debug)]
struct ClientProgress {
    controller_open: bool,
    pawn_open: bool,
    possession: bool,
    begin_queued: bool,
    friends_answers: u32,
    /// Creations delivered per record sub-object GUID (reliable duplicates
    /// already removed by the client's channel).
    record_creations: BTreeMap<u32, u32>,
    own_records: u32,
    /// Release copies the server sent, the copies the client never
    /// accepted (dropped, or stale on arrival) and those it accepted.
    release_copies_sent: u32,
    release_copies_lost: u32,
    release_copies_accepted: u32,
    move_acks: Vec<f32>,
    moves_sent: u32,
    stale: u64,
    closed_by_server: bool,
    /// The own record arrived before the controller open, the possession
    /// update or the pawn open.
    order_violation: bool,
    completed_at: Option<u64>,
}

struct Client {
    index: usize,
    socket: UdpSocket,
    address: SocketAddrV4,
    sequencer: PacketSequencer,
    write: PacketConfig,
    read: PacketConfig,
    control_codec: ControlCodec,
    initial_out_reliable: u32,
    next_reliable: BTreeMap<u32, u32>,
    outgoing: Vec<OutReliable>,
    control_in: ReliableChannel<Vec<Decoded>>,
    tracker: ActorChannelTracker,
    codec: RepLayoutCodec,
    /// Per (channel, raw reliable sequence) transmissions seen.
    transmissions: BTreeMap<(u32, u32), u64>,
    release_copies: u64,
    move_ack_copies: u64,
    background: u64,
    to_client: Vec<InFlight>,
    to_server: Vec<InFlight>,
    order: u64,
    /// Timestamps of moves the fault model delivered to the lab.
    moves_delivered: BTreeSet<u32>,
    next_move: u64,
    silent: bool,
    /// The client could not go on (its own header or decode failed).
    failed: Option<String>,
    progress: ClientProgress,
}

impl Client {
    fn connect(lab: &mut PeerLab, index: usize) -> Self {
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        socket.connect(lab.local_addr().unwrap()).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        let SocketAddr::V4(address) = socket.local_addr().unwrap() else {
            unreachable!("IPv4 loopback")
        };
        // The stateless handshake, lossless: out of the campaign's scope.
        let mut buffer = [0; 2048];
        socket.send(&initial_request()).unwrap();
        lab.poll();
        let length = socket.recv(&mut buffer).unwrap();
        let challenge = decode(&buffer[..length]).unwrap();
        socket.send(&buffer[..length]).unwrap();
        lab.poll();
        let length = socket.recv(&mut buffer).unwrap();
        assert_eq!(decode(&buffer[..length]).unwrap().cookie, challenge.cookie);
        socket.set_nonblocking(true).unwrap();
        let (incoming, outgoing) = client_cookie_seeds(&challenge.cookie);
        let sequencer = PacketSequencer::new(incoming, outgoing, CLIENT_SENT_CAPACITY).unwrap();
        let engine = lab
            .player_controller_open
            .as_ref()
            .unwrap()
            .engine_net_version;
        let codec = initialise_codec(engine);
        let write = lab.config.packet.packet;
        let read = PacketConfig {
            receiver: Receiver::Client,
            ..write
        };
        let (initial_in, initial_out) = sequencer.initial_reliable();
        let mut client = Self {
            index,
            socket,
            address,
            control_in: sequencer.new_channel(),
            tracker: ActorChannelTracker::new(read, codec.actor, initial_in),
            sequencer,
            write,
            read,
            control_codec: ControlCodec::default(),
            initial_out_reliable: initial_out,
            next_reliable: BTreeMap::new(),
            outgoing: Vec::new(),
            codec,
            transmissions: BTreeMap::new(),
            release_copies: 0,
            move_ack_copies: 0,
            background: 0,
            to_client: Vec::new(),
            to_server: Vec::new(),
            order: 0,
            moves_delivered: BTreeSet::new(),
            next_move: 0,
            silent: false,
            failed: None,
            progress: ClientProgress::default(),
        };
        let version = lab.config.local_network_version.unwrap();
        client.queue_control(
            ControlMessage::Hello {
                first_field: 1,
                version,
                token: ArchiveText::text("").unwrap(),
            },
            true,
        );
        client
    }

    fn active(&self) -> bool {
        !self.silent && self.failed.is_none() && !self.progress.closed_by_server
    }

    fn reliable_header(&mut self, channel: u32, control: bool, open: bool) -> BunchHeader {
        let last = self
            .next_reliable
            .get(&channel)
            .copied()
            .unwrap_or(self.initial_out_reliable);
        let sequence = last + 1;
        self.next_reliable.insert(channel, sequence);
        BunchHeader {
            control,
            open,
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
            channel_name: Some(Name::Hardcoded(if channel == 0 {
                CONTROL_CHANNEL_NAME
            } else {
                ACTOR_CHANNEL_NAME_INDEX
            })),
        }
    }

    fn queue_control(&mut self, message: ControlMessage, open: bool) {
        let header = self.reliable_header(0, open, open);
        let mut payload = BitWriter::new();
        payload.write_bytes(&encode_control_message(&message, &self.control_codec).unwrap());
        self.outgoing.push(OutReliable {
            bunch: Bunch {
                header,
                payload: Payload::from_writer(payload),
            },
            packet: None,
            resend: true,
        });
    }

    fn queue_rpc(&mut self, target: BlockTarget, field: &FieldHeader, bound: u32) {
        let mut fields = BitWriter::new();
        write_field_header(&mut fields, field, bound).unwrap();
        let payload = encode_actor_bunch_body(
            &ActorBunchBody {
                spawn: None,
                blocks: vec![ContentBlock {
                    has_rep_layout: false,
                    target,
                    payload: Payload::from_writer(fields),
                }],
            },
            &self.codec.actor,
        )
        .unwrap();
        let header = self.reliable_header(PLAYER_CONTROLLER_CHANNEL_INDEX, false, false);
        self.outgoing.push(OutReliable {
            bunch: Bunch { header, payload },
            packet: None,
            resend: true,
        });
    }

    /// This step's packet: every reliable bunch owed (resends first, in
    /// creation order) within the budget, a synthetic move when due, and the
    /// ACK state. `None` when the client's own sent ring is full.
    fn build(
        &mut self,
        step: u64,
        lab: &PeerLab,
        trace: &mut Trace,
    ) -> Option<(Vec<u8>, Option<f32>)> {
        if self.sequencer.notify().sent_records().len() >= CLIENT_SENT_CAPACITY {
            return None;
        }
        let header = match self.sequencer.header() {
            Ok(header) => header,
            Err(error) => {
                self.failed = Some(format!("client header: {error:#}"));
                trace.push(format!("s{step} c{} client header failed", self.index));
                return None;
            }
        };
        let mut bunches = Vec::new();
        let mut carried = Vec::new();
        let mut bits = 0;
        for (position, out) in self.outgoing.iter().enumerate() {
            if !out.resend {
                continue;
            }
            let size = out.bunch.payload.bit_len();
            if !bunches.is_empty() && (bits + size > CLIENT_PACKET_BITS || bunches.len() >= 16) {
                break;
            }
            bits += size;
            bunches.push(out.bunch.clone());
            carried.push(position);
        }
        let mut moved = None;
        if self.progress.pawn_open && step >= self.next_move && bunches.len() < 16 {
            let count = self.progress.moves_sent;
            let timestamp = 1.0 + 0.03 * count as f32;
            let base = lab
                .peers
                .get(&self.address)
                .and_then(|entry| entry.placement);
            let [x, y, z] = base.map_or([0.0; 3], |placement| placement.location);
            let field = authored_move(timestamp, [x + 4.0 * count as f32, y, z], Some(1));
            let engine = lab.pawn_open.as_ref().unwrap().0.engine_net_version;
            bunches.push(unreliable_bunch(
                PAWN_CHANNEL_INDEX,
                move_bunch_payload(field, engine),
            ));
            self.progress.moves_sent += 1;
            self.next_move = step + MOVE_EVERY;
            moved = Some(timestamp);
        }
        let bytes = write_packet(
            &Packet {
                magic: 0,
                header,
                info: None,
                bunches,
            },
            &self.write,
        )
        .unwrap();
        let packet_id = self.sequencer.commit().unwrap();
        for position in carried {
            let out = &mut self.outgoing[position];
            out.packet = Some(packet_id);
            out.resend = false;
        }
        Some((bytes, moved))
    }

    /// Classify a datagram the server sent, before any fault: its decision
    /// key and whether it carries a release copy (which may share a datagram
    /// with reliable resends).
    fn classify(&mut self, lab: &PeerLab, bytes: &[u8]) -> (Vec<u64>, bool) {
        let Ok(packet) = read_packet(bytes, &self.read) else {
            self.background += 1;
            return (vec![KIND_BACKGROUND, self.background], false);
        };
        let release = packet.bunches.iter().any(|bunch| is_release(lab, bunch));
        if release {
            self.release_copies += 1;
            self.progress.release_copies_sent += 1;
        }
        if let Some(bunch) = packet
            .bunches
            .iter()
            .find(|bunch| bunch.header.reliable && bunch.header.reliable_sequence_raw.is_some())
        {
            let key = (
                bunch.header.channel_index,
                bunch.header.reliable_sequence_raw.unwrap_or_default(),
            );
            let count = self.transmissions.entry(key).or_default();
            *count += 1;
            return (
                vec![KIND_RELIABLE, u64::from(key.0), u64::from(key.1), *count],
                release,
            );
        }
        if release {
            return (vec![KIND_RELEASE, self.release_copies], true);
        }
        if packet
            .bunches
            .iter()
            .any(|bunch| bunch.header.channel_index == PAWN_CHANNEL_INDEX && !bunch.header.open)
        {
            self.move_ack_copies += 1;
            return (vec![KIND_MOVE_ACK, self.move_ack_copies], false);
        }
        self.background += 1;
        (vec![KIND_BACKGROUND, self.background], false)
    }

    /// Process one delivered server datagram as the client.
    fn receive(
        &mut self,
        lab: &PeerLab,
        bytes: &[u8],
        release: bool,
        step: u64,
        trace: &mut Trace,
    ) {
        let packet = match read_packet(bytes, &self.read) {
            Ok(packet) => packet,
            Err(error) => {
                self.failed = Some(format!("server packet did not read: {error:#}"));
                return;
            }
        };
        let received = match self.sequencer.receive(&packet.header) {
            Ok(Some(received)) => received,
            Ok(None) => {
                self.progress.stale += 1;
                if release {
                    self.progress.release_copies_lost += 1;
                }
                return;
            }
            Err(error) => {
                self.failed = Some(format!("client sequencing: {error:#}"));
                return;
            }
        };
        self.sequencer.ack_seq(received.sequence, true).unwrap();
        for notification in &received.notifications {
            for out in &mut self.outgoing {
                if out.packet == Some(notification.packet_id) && !out.resend {
                    if notification.delivered {
                        out.packet = None;
                    } else {
                        out.resend = true;
                    }
                }
            }
            self.outgoing
                .retain(|out| out.resend || out.packet.is_some());
        }
        if release {
            self.progress.release_copies_accepted += 1;
        }
        for bunch in packet
            .bunches
            .iter()
            .filter(|bunch| bunch.header.channel_index == 0)
        {
            if bunch.header.close {
                self.progress.closed_by_server = true;
                trace.push(format!("s{step} c{} control close received", self.index));
            }
            let Some(raw) = bunch
                .header
                .reliable_sequence_raw
                .filter(|_| bunch.header.reliable)
            else {
                continue;
            };
            let Ok(decoded) =
                read_control_messages(&mut bunch.payload.reader(), &self.control_codec)
            else {
                continue;
            };
            if let Ok(ReliableReceive::Delivered(batches)) =
                self.control_in.receive_reliable(raw, decoded)
            {
                for message in batches.into_iter().flatten() {
                    if let Decoded::Message(message) = message {
                        self.on_control(&message, step, trace);
                    }
                }
            }
        }
        let mut notes = Vec::new();
        for bunch in self.tracker.feed_packet(packet, &mut notes) {
            self.on_actor(lab, &bunch, step, trace);
        }
    }

    fn on_control(&mut self, message: &ControlMessage, step: u64, trace: &mut Trace) {
        match message {
            ControlMessage::Challenge { .. } => {
                trace.push(format!("s{step} c{} challenge", self.index));
                let login = ControlMessage::Login {
                    response: ArchiveText::text("0").unwrap(),
                    url: ArchiveText::text(&format!("?Name=AuthoredLossy{}", self.index)).unwrap(),
                    unique_net_id: UniqueNetId {
                        kind: NetIdType::Named(ArchiveText::text("STEAM").unwrap()),
                        value: NetIdValue::from_hex(&format!("0A0B0C0D0E0F10{:02X}", self.index))
                            .unwrap(),
                    },
                    platform: ArchiveText::text("").unwrap(),
                };
                self.queue_control(login, false);
            }
            ControlMessage::Welcome { .. } => {
                trace.push(format!("s{step} c{} welcome", self.index));
                self.queue_control(ControlMessage::Netspeed { rate: 100_000 }, false);
                self.queue_control(ControlMessage::Join, false);
            }
            ControlMessage::Failure { .. } | ControlMessage::Upgrade { .. } => {
                self.failed = Some(format!("server refused: {message:?}"));
            }
            _ => {}
        }
    }

    fn component(&self, guid: u32) -> Option<Component> {
        let name = self.tracker.package_map().resolve_path(guid)?;
        [
            ("BackendProxyComponent", Component::BackendProxy),
            ("PlayerDataComponent", Component::PlayerData),
            ("BestiaryManagerComponent", Component::Bestiary),
        ]
        .into_iter()
        .find(|(known, _)| known.eq_ignore_ascii_case(name))
        .map(|(_, component)| component)
    }

    fn on_actor(&mut self, lab: &PeerLab, bunch: &ActorBunch, step: u64, trace: &mut Trace) {
        let channel = bunch.channel_index;
        if channel == PLAYER_CONTROLLER_CHANNEL_INDEX {
            if bunch.open {
                self.progress.controller_open = true;
                trace.push(format!("s{step} c{} controller open", self.index));
                if !self.progress.begin_queued {
                    self.progress.begin_queued = true;
                    let field = begin_player_initialisation_field(CHR_SLOT, &self.codec).unwrap();
                    self.queue_rpc(BlockTarget::Actor, &field, CONTROLLER_FIELD_BOUND);
                }
                return;
            }
            if bunch.close {
                trace.push(format!("s{step} c{} controller channel closed", self.index));
                return;
            }
            let Ok(body) =
                read_actor_bunch_body(&mut bunch.payload.reader(), false, &self.codec.actor)
            else {
                return;
            };
            for block in &body.blocks {
                self.on_controller_block(block, step, trace);
            }
            return;
        }
        if channel == PAWN_CHANNEL_INDEX {
            if bunch.open {
                self.progress.pawn_open = true;
                trace.push(format!("s{step} c{} pawn open", self.index));
                return;
            }
            if let Some(timestamp) = move_ack(bunch) {
                self.progress.move_acks.push(timestamp);
                trace.push(format!("s{step} c{} move ack {timestamp}", self.index));
            } else if frozen_release(lab, &bunch.payload) {
                trace.push(format!("s{step} c{} release", self.index));
            }
            return;
        }
        let Some(session) = &lab.game_state_open else {
            return;
        };
        if channel != session.channel_index || !bunch.reliable || bunch.open {
            return;
        }
        let Ok(body) = read_actor_bunch_body(
            &mut bunch.payload.reader(),
            false,
            &actor_codec(17, ByteOrder::Little),
        ) else {
            return;
        };
        let own = lab
            .peers
            .get(&self.address)
            .and_then(|entry| entry.controller_open_guids)
            .map(|(controller, _)| controller);
        let Some(plan) = &lab.connected_players else {
            return;
        };
        for block in &body.blocks {
            let BlockTarget::SubObject {
                object,
                class: SubObjectClass::Class(_),
            } = &block.target
            else {
                continue;
            };
            let Ok(values) = super::super::super::rep_layout::parse_rep_properties(
                &block.payload,
                &plan.record_layout,
                &record_codec(),
            ) else {
                continue;
            };
            *self
                .progress
                .record_creations
                .entry(object.guid)
                .or_default() += 1;
            let controller = values.entries.iter().find(|entry| entry.handle == 3);
            let is_own = matches!(
                (controller.map(|entry| &entry.value), own),
                (Some(RepValue::Object(reference)), Some(own)) if reference.guid == own
            );
            if is_own {
                self.progress.own_records += 1;
                trace.push(format!(
                    "s{step} c{} own record (controller {}, possession {}, pawn {})",
                    self.index,
                    self.progress.controller_open,
                    self.progress.possession,
                    self.progress.pawn_open
                ));
                if !(self.progress.controller_open
                    && self.progress.possession
                    && self.progress.pawn_open)
                {
                    self.progress.order_violation = true;
                }
            }
        }
    }

    fn on_controller_block(&mut self, block: &ContentBlock, step: u64, trace: &mut Trace) {
        if block.has_rep_layout {
            if block.target == BlockTarget::Actor {
                self.progress.possession = true;
                trace.push(format!("s{step} c{} possession update", self.index));
            }
            return;
        }
        let (target, bound) = match &block.target {
            BlockTarget::Actor => (None, CONTROLLER_FIELD_BOUND),
            BlockTarget::SubObject { object, .. } => {
                let Some(component) = self.component(object.guid) else {
                    return;
                };
                let bound = match component {
                    Component::BackendProxy => BACKEND_PROXY_FIELD_INDEX_BOUND,
                    Component::PlayerData => PLAYER_DATA_FIELD_INDEX_BOUND,
                    Component::Bestiary => BESTIARY_MANAGER_FIELD_INDEX_BOUND,
                };
                (Some((component, object.guid)), bound)
            }
        };
        let mut reader = block.payload.reader();
        while !reader.at_end() {
            let Ok(field) =
                read_field_header(&mut reader, bound, self.codec.actor.limits.max_payload_bits)
            else {
                return;
            };
            match (target, field.index) {
                (None, 118) if field.payload.bit_len() == 0 => {
                    self.progress.friends_answers += 1;
                    trace.push(format!("s{step} c{} friends query", self.index));
                    let answer = friends_list_updated_field(&self.codec).unwrap();
                    self.queue_rpc(BlockTarget::Actor, &answer, CONTROLLER_FIELD_BOUND);
                }
                (Some((Component::BackendProxy, guid)), index)
                    if index == CLIENT_REQUEST_GET_USER_PROFILE
                        || index == CLIENT_REQUEST_GET_CHARACTER_PROFILE =>
                {
                    let character = index == CLIENT_REQUEST_GET_CHARACTER_PROFILE;
                    trace.push(format!(
                        "s{step} c{} profile request (character {character})",
                        self.index
                    ));
                    let mut items = vec![RpcItem::Value(RepValue::Bool(true))];
                    items.extend(if character {
                        authored_character_profile(CHR_SLOT).unwrap()
                    } else {
                        authored_user_profile().unwrap()
                    });
                    let answer = FieldHeader {
                        index: if character {
                            rep_rpc::RESPONSE_GET_CHARACTER_PROFILE
                        } else {
                            rep_rpc::RESPONSE_GET_USER_PROFILE
                        },
                        payload: rep_rpc::encode_rpc_payload(
                            &rep_rpc::backend_profile_response_parameters(character),
                            &[RpcArgument::Sent(items)],
                            &self.codec,
                        )
                        .unwrap(),
                    };
                    self.queue_rpc(sent_to(guid), &answer, BACKEND_PROXY_FIELD_INDEX_BOUND);
                }
                (Some((Component::PlayerData, guid)), CLIENT_PREPARE_LOADOUT) => {
                    trace.push(format!("s{step} c{} loadout request", self.index));
                    let answer = FieldHeader {
                        index: rep_rpc::SERVER_RECEIVE_LOADOUT,
                        payload: loadout_response_payload(&self.codec).unwrap(),
                    };
                    self.queue_rpc(sent_to(guid), &answer, PLAYER_DATA_FIELD_INDEX_BOUND);
                }
                (Some((Component::Bestiary, guid)), CLIENT_REQUEST_BESTIARY_DATA) => {
                    trace.push(format!("s{step} c{} bestiary request", self.index));
                    let answer = FieldHeader {
                        index: rep_rpc::SERVER_RECEIVE_BESTIARY_DATA,
                        payload: bestiary_response_payload(&self.codec).unwrap(),
                    };
                    self.queue_rpc(sent_to(guid), &answer, BESTIARY_MANAGER_FIELD_INDEX_BOUND);
                }
                _ => {}
            }
        }
    }
}

fn sent_to(guid: u32) -> BlockTarget {
    BlockTarget::SubObject {
        object: NetGuidRef::plain(guid),
        class: SubObjectClass::ClientSent,
    }
}

/// A whole unreliable pawn bunch setting `bServerFrozenMovement` false.
fn is_release(lab: &PeerLab, bunch: &Bunch) -> bool {
    let header = &bunch.header;
    header.channel_index == PAWN_CHANNEL_INDEX
        && !header.reliable
        && !header.open
        && !header.partial
        && frozen_release(lab, &bunch.payload)
}

fn frozen_release(lab: &PeerLab, payload: &Payload) -> bool {
    let Some((plan, _, _)) = &lab.pawn_open else {
        return false;
    };
    let Ok(body) = read_actor_bunch_body(
        &mut payload.reader(),
        false,
        &actor_codec(plan.engine_net_version, ByteOrder::Little),
    ) else {
        return false;
    };
    let Ok(wire) = plan
        .layout_version
        .wire_handle(super::super::super::pawn_open::handle::SERVER_FROZEN_MOVEMENT)
    else {
        return false;
    };
    body.blocks.iter().any(|block| {
        block.has_rep_layout
            && super::super::super::rep_layout::parse_rep_properties(
                &block.payload,
                &plan.layout,
                &record_codec(),
            )
            .is_ok_and(|stream| {
                stream
                    .entries
                    .iter()
                    .any(|entry| entry.handle == wire && entry.value == RepValue::Bool(false))
            })
    })
}

/// The acknowledged timestamp of a good-move response on the pawn channel.
fn move_ack(bunch: &ActorBunch) -> Option<f32> {
    if bunch.reliable {
        return None;
    }
    let body = read_actor_bunch_body(
        &mut bunch.payload.reader(),
        false,
        &actor_codec(17, ByteOrder::Little),
    )
    .ok()?;
    let [block] = body.blocks.as_slice() else {
        return None;
    };
    if block.has_rep_layout {
        return None;
    }
    let mut reader = block.payload.reader();
    let field = read_field_header(&mut reader, PAWN_FIELD_BOUND, usize::MAX).ok()?;
    (field.index == CLIENT_MOVE_RESPONSE_PACKED_FIELD_INDEX)
        .then(|| read_good_move_ack(&field.payload, ByteOrder::Little).ok())
        .flatten()
}

// ---------------------------------------------------------------- driver

/// The last events, for a failure report: authored values only.
#[derive(Default)]
struct Trace(VecDeque<String>);

impl Trace {
    fn push(&mut self, event: String) {
        if self.0.len() == TRACE {
            self.0.pop_front();
        }
        self.0.push_back(event);
    }
}

/// How a session ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Outcome {
    Completed,
    CompletedAfterHealing,
    ClosedIdle,
    ClosedDeliveryStall,
    ClosedFriendsDeadline,
    ClosedPossessionAbandoned,
    ClosedChannelFailure,
    ClosedFailed,
    ClosedOther,
}

/// One seed's result.
#[derive(Debug)]
struct RunReport {
    outcomes: Vec<Outcome>,
    /// Steps each completed client needed.
    steps: Vec<u64>,
    move_acks: u64,
    release_resends: u64,
    stale: u64,
    errors: Vec<String>,
}

/// The lab's nonzero error, failure and stall counters, by name.
fn error_counts(lab: &PeerLab) -> Vec<String> {
    serde_json::to_value(&lab.stats)
        .ok()
        .and_then(|value| value.as_object().cloned())
        .into_iter()
        .flatten()
        .filter(|(name, value)| {
            (name.contains("error") || name.contains("fail") || name.contains("stall"))
                && value.as_u64().is_some_and(|count| count > 0)
        })
        .map(|(name, value)| format!("{name}={value}"))
        .collect()
}

/// The lab counters by which a session may leave.
fn close_counts(lab: &PeerLab) -> [u64; 7] {
    let stats = &lab.stats;
    [
        stats.peers_expired,
        stats.peers_delivery_stalled,
        stats.peers_friends_query_expired,
        stats.peers_closed_by_possession_abandoned,
        stats.peers_closed_by_channel_failure,
        stats.peers_failed,
        stats.peers_closed_by_client
            + stats.peers_closed
            + stats.peers_auth_closed
            + stats.peers_shutdown
            + stats.peers_replaced,
    ]
}

fn close_outcome(before: [u64; 7], after: [u64; 7]) -> Outcome {
    const PATHS: [Outcome; 7] = [
        Outcome::ClosedIdle,
        Outcome::ClosedDeliveryStall,
        Outcome::ClosedFriendsDeadline,
        Outcome::ClosedPossessionAbandoned,
        Outcome::ClosedChannelFailure,
        Outcome::ClosedFailed,
        Outcome::ClosedOther,
    ];
    PATHS
        .into_iter()
        .zip(before.into_iter().zip(after))
        .find(|(_, (before, after))| after > before)
        .map_or(Outcome::ClosedOther, |(outcome, _)| outcome)
}

struct Run {
    scenario: Scenario,
    seed: u64,
    lab: PeerLab,
    clients: Vec<Client>,
    trace: Trace,
    step: u64,
    healed: bool,
    blackout: Option<(usize, u64)>,
    /// The client whose link has the outage, and its steps.
    outage: Option<(usize, std::ops::Range<u64>)>,
    outcomes: Vec<Option<Outcome>>,
    /// Sessions in the order their placement was committed.
    placements: Vec<(usize, [f32; 3])>,
    started: Instant,
}

impl Run {
    fn new(scenario: Scenario, seed: u64) -> Self {
        let mut lab = campaign_lab(&scenario);
        let clients = (0..scenario.clients)
            .map(|index| Client::connect(&mut lab, index))
            .collect();
        let blackout = scenario.blackout_before.map(|bound| {
            let client = (keyed(seed, &[9, 1]) % scenario.clients as u64) as usize;
            (client, keyed(seed, &[9, 2]) % bound)
        });
        let outage = scenario.outage.map(|outage| {
            let client = (keyed(seed, &[9, 3]) % scenario.clients as u64) as usize;
            let start = keyed(seed, &[9, 4]) % outage.start_before;
            (client, start..start + outage.steps)
        });
        Self {
            scenario,
            seed,
            lab,
            clients,
            trace: Trace::default(),
            step: 0,
            healed: false,
            blackout,
            outage,
            outcomes: vec![None; scenario.clients],
            placements: Vec::new(),
            started: Instant::now(),
        }
    }

    fn link(&self, client: usize) -> Link {
        if self.healed {
            return Link::CLEAN;
        }
        let mut link = self.scenario.links[client];
        if let (Some(outage), Some((affected, steps))) = (self.scenario.outage, &self.outage)
            && *affected == client
            && steps.contains(&self.step)
        {
            if outage.server_to_client {
                link.server_to_client_drop = 1000;
            }
            if outage.client_to_server {
                link.client_to_server_drop = 1000;
            }
        }
        link
    }

    fn fail(&self, message: &str) -> ! {
        let events: Vec<_> = self.trace.0.iter().cloned().collect();
        let progress: Vec<_> = self
            .clients
            .iter()
            .map(|client| {
                let lab = self.lab.peers.get(&client.address).map(|entry| {
                    let release = &entry.frozen_release;
                    format!(
                        "finalised {} creations settled {} receipts {} prerequisites {} \
                         possession pending {} failed {} | release record_acked {} due {} \
                         pending {} deferred {} in_flight {:?} sent {} acked {} failed {}",
                        entry.profiles.finalised,
                        entry.connected_player_creations_settled,
                        entry.connected_player_receipts.len(),
                        entry.record_prerequisites.settled(),
                        entry.controller_possession_pending,
                        entry.controller_possession_failed,
                        release.record_acked_at.is_some(),
                        release.due_at.is_some(),
                        release.pending,
                        release.deferred,
                        release.in_flight,
                        release.sent_at.is_some(),
                        release.acked_at.is_some(),
                        release.failed,
                    )
                });
                let transport = self.lab.peers.get(&client.address).map(|entry| {
                    let peer = &entry.peer;
                    format!(
                        "sent ring {} ack blocked {} control pending {} resends {} client packet \
                         received {} reply pending {} retry ready {} connection {:?} controller \
                         channel {:?} initial opens {}",
                        peer.sequencer().notify().sent_records().len(),
                        entry.ack_capacity_blocked,
                        peer.pending_reliable(),
                        peer.pending_resends(),
                        peer.has_received_client_packet(),
                        entry.pending_control_reply.is_some(),
                        entry.control_reply_retry_ready,
                        entry.connection_id,
                        entry.player_controller_channel,
                        entry.initial_opens.is_some(),
                    )
                });
                let owed = client.outgoing.len();
                let resend = client.outgoing.iter().filter(|out| out.resend).count();
                format!(
                    "c{}: {:?} failed={:?}\n    client: owes {owed} reliable ({resend} to send), \
                     sent ring {}, in {} out {}\n    lab: {}\n    transport: {}",
                    client.index,
                    client.progress,
                    client.failed,
                    client.sequencer.notify().sent_records().len(),
                    client.sequencer.notify().in_seq(),
                    client.sequencer.notify().out_seq(),
                    lab.unwrap_or_else(|| "removed".to_owned()),
                    transport.unwrap_or_default(),
                )
            })
            .collect();
        let errors = error_counts(&self.lab);
        let stats = &self.lab.stats;
        let counters = format!(
            "record creations sent {} acked {}; possession sent {} acked {}; releases scheduled {} \
             sent {} acked {} resends {} deferred {} errors {}; move acks {} moves {}",
            stats.connected_player_record_creations_sent,
            stats.connected_player_record_creations_acked,
            stats.controller_possession_updates_sent,
            stats.controller_possession_updates_acked,
            stats.frozen_movement_releases_scheduled,
            stats.frozen_movement_releases_sent,
            stats.frozen_movement_releases_acked,
            stats.frozen_movement_release_resends,
            stats.frozen_movement_release_sends_deferred,
            stats.frozen_movement_release_errors,
            stats.pawn_move_acks_sent,
            stats.pawn_moves_received,
        );
        panic!(
            "lossy join campaign: {message}\nscenario {} seed {} step {} healed {} blackout {:?} \
             outage {:?}\n\
             reproduce: ICARUS_LOSSY_JOIN_SCENARIO={} ICARUS_LOSSY_JOIN_SEED={} \
             python3 tools/cargo_guard.py test --locked --lib lossy_join::reproduce_one -- --ignored --nocapture\n\
             counters: {counters}\nerrors: {}\nclients:\n{}\nlast events:\n{}",
            self.scenario.name,
            self.seed,
            self.step,
            self.healed,
            self.blackout,
            self.outage,
            self.scenario.name,
            self.seed,
            errors.join(" "),
            progress.join("\n"),
            events.join("\n"),
        );
    }

    /// One client step, the lab's poll and delivery back.
    fn step(&mut self) {
        let step = self.step;
        let seed = self.seed;
        if let Some((client, at)) = self.blackout
            && at == step
        {
            self.clients[client].silent = true;
            self.trace.push(format!("s{step} c{client} blackout"));
        }
        // Client to server.
        for index in 0..self.clients.len() {
            let link = self.link(index);
            let client = &mut self.clients[index];
            if !client.active() {
                continue;
            }
            let Some((bytes, moved)) = client.build(step, &self.lab, &mut self.trace) else {
                continue;
            };
            let delays = decide(
                seed,
                link,
                link.client_to_server_drop,
                &[CLIENT_TO_SERVER, index as u64, step],
            );
            if delays.is_empty()
                && client
                    .outgoing
                    .iter()
                    .any(|out| out.packet.is_some() && !out.resend)
            {
                self.trace
                    .push(format!("s{step} c{index} up dropped (reliable owed)"));
            }
            for delay in delays {
                client.order += 1;
                client.to_server.push(InFlight {
                    due: step + delay,
                    order: client.order,
                    bytes: bytes.clone(),
                    moved,
                    release: false,
                });
            }
        }
        for client in &mut self.clients {
            client
                .to_server
                .sort_by_key(|flight| (flight.due, flight.order));
            let due = client
                .to_server
                .partition_point(|flight| flight.due <= step);
            for flight in client.to_server.drain(..due).collect::<Vec<_>>() {
                if client.silent {
                    continue;
                }
                if let Some(timestamp) = flight.moved {
                    client.moves_delivered.insert(timestamp.to_bits());
                }
                client.socket.send(&flight.bytes).unwrap();
            }
        }
        let closes = close_counts(&self.lab);
        let present: Vec<bool> = self
            .clients
            .iter()
            .map(|client| self.lab.peers.contains_key(&client.address))
            .collect();
        self.lab.poll();
        // The move-acknowledgement pass again at an injected instant: the
        // lab takes its time here, unlike the poll's other passes.
        let virtual_now = self.started + MOVE_ACK_STEP * u32::try_from(step).unwrap();
        self.lab.retry_pawn_move_acks(virtual_now);
        self.note_removals(&present, closes);
        // Server to client.
        let mut buffer = [0; 4096];
        for index in 0..self.clients.len() {
            let link = self.link(index);
            loop {
                let length = match self.clients[index].socket.recv(&mut buffer) {
                    Ok(length) => length,
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                    Err(error) => panic!("loopback receive: {error}"),
                };
                let bytes = buffer[..length].to_vec();
                let client = &mut self.clients[index];
                let (key, release) = client.classify(&self.lab, &bytes);
                let mut full = vec![SERVER_TO_CLIENT, index as u64];
                full.extend(&key);
                let delays = if client.silent {
                    Vec::new()
                } else {
                    decide(seed, link, link.server_to_client_drop, &full)
                };
                if delays.is_empty() {
                    if release {
                        client.progress.release_copies_lost += 1;
                    }
                    if key[0] != KIND_BACKGROUND {
                        self.trace
                            .push(format!("s{step} c{index} down dropped {key:?}"));
                    }
                }
                for delay in delays {
                    client.order += 1;
                    client.to_client.push(InFlight {
                        due: step + delay,
                        order: client.order,
                        bytes: bytes.clone(),
                        moved: None,
                        release,
                    });
                }
            }
        }
        for index in 0..self.clients.len() {
            let client = &mut self.clients[index];
            client
                .to_client
                .sort_by_key(|flight| (flight.due, flight.order));
            let due = client
                .to_client
                .partition_point(|flight| flight.due <= step);
            for flight in client.to_client.drain(..due).collect::<Vec<_>>() {
                if !client.active() {
                    // Never accepted: a copy in transit when its client went
                    // silent is lost, not a resend without a NAK.
                    if flight.release {
                        client.progress.release_copies_lost += 1;
                    }
                    continue;
                }
                client.receive(
                    &self.lab,
                    &flight.bytes,
                    flight.release,
                    step,
                    &mut self.trace,
                );
            }
        }
        self.check_step();
        self.step += 1;
    }

    /// Attribute every session the poll removed to the counter that moved.
    fn note_removals(&mut self, present: &[bool], before: [u64; 7]) {
        let after = close_counts(&self.lab);
        let removed: Vec<usize> = (0..self.clients.len())
            .filter(|&index| {
                present[index] && !self.lab.peers.contains_key(&self.clients[index].address)
            })
            .collect();
        if removed.is_empty() {
            return;
        }
        let moved: u64 = before
            .iter()
            .zip(after)
            .map(|(before, after)| after - before)
            .sum();
        if moved < removed.len() as u64 {
            self.fail(&format!(
                "SILENT REMOVAL: {} sessions left with {moved} close counts",
                removed.len()
            ));
        }
        for index in removed {
            let outcome = close_outcome(before, after);
            self.trace
                .push(format!("s{} c{index} removed: {outcome:?}", self.step));
            if self.outcomes[index].is_none() {
                self.outcomes[index] = Some(outcome);
            }
        }
    }

    fn complete(&self, index: usize) -> bool {
        let client = &self.clients[index];
        let Some(entry) = self.lab.peers.get(&client.address) else {
            return false;
        };
        entry.profiles.finalised
            && entry.frozen_release.record_acked_at.is_some()
            && entry.frozen_release.acked_at.is_some()
            && client.progress.own_records == 1
            && client.progress.release_copies_accepted >= 1
    }

    /// Bounds and exactly-once effects, after every step.
    fn check_step(&mut self) {
        // Placements: committed once per session, round-robin in commit order.
        for index in 0..self.clients.len() {
            if self.placements.iter().any(|(placed, _)| *placed == index) {
                continue;
            }
            if let Some(placement) = self
                .lab
                .peers
                .get(&self.clients[index].address)
                .and_then(|entry| entry.placement)
            {
                self.placements.push((index, placement.location));
            }
        }
        let lab = &self.lab;
        let limit = lab.config.max_peers;
        if lab.peers.len() > limit {
            self.fail(&format!("BOUND: {} peers over {limit}", lab.peers.len()));
        }
        let capacity = lab.config.packet.sent_capacity;
        for client in &self.clients {
            let Some(entry) = lab.peers.get(&client.address) else {
                continue;
            };
            let peer = &entry.peer;
            let sent = peer.sequencer().notify().sent_records().len();
            let control = peer.pending_reliable();
            let actor: usize = (1..lab.config.packet.packet.limits.max_channels.min(64))
                .filter_map(|channel| peer.pending_actor_reliable(channel))
                .inspect(|pending| {
                    if *pending > MAX_PENDING_RELIABLE {
                        self.fail(&format!("BOUND: {pending} reliable bunches on a channel"));
                    }
                })
                .sum();
            if sent > capacity || control > MAX_PENDING_RELIABLE {
                self.fail(&format!("BOUND: sent ring {sent}, control queue {control}"));
            }
            if peer.pending_resends() > control + actor {
                self.fail(&format!(
                    "BOUND: {} resends queued for {} outstanding reliable bunches",
                    peer.pending_resends(),
                    control + actor
                ));
            }
            if entry.frozen_release.in_flight.len() > 4 {
                self.fail("BOUND: release copy spans more than four packets");
            }
            if entry.actor_property_packets.len() > capacity {
                self.fail("BOUND: actor property packets exceed the sent ring");
            }
        }
        for client in &self.clients {
            let progress = &client.progress;
            if progress.order_violation {
                self.fail(&format!(
                    "ORDER: client {} received its own record before the controller open, the \
                     possession update and the pawn open",
                    client.index
                ));
            }
            if progress.own_records > 1 {
                self.fail(&format!(
                    "ONCE: client {} got its own record twice",
                    client.index
                ));
            }
            if let Some((guid, count)) = progress
                .record_creations
                .iter()
                .find(|(_, count)| **count > 1)
            {
                self.fail(&format!(
                    "ONCE: client {} got record {guid} created {count} times",
                    client.index
                ));
            }
            if progress.release_copies_accepted > 1 {
                self.fail(&format!(
                    "ONCE: client {} accepted {} release copies",
                    client.index, progress.release_copies_accepted
                ));
            }
            // A copy still in transit may yet arrive stale (overtaken, so
            // already NAKed); one accepted copy is never resent.
            let in_transit = client
                .to_client
                .iter()
                .filter(|flight| flight.release)
                .count() as u32;
            if progress.release_copies_sent > 1 + progress.release_copies_lost + in_transit {
                self.fail(&format!(
                    "ONCE: {} release copies for {} lost and {in_transit} in transit (a resend \
                     without a NAK)",
                    progress.release_copies_sent, progress.release_copies_lost
                ));
            }
            if let Some(ack) = progress
                .move_acks
                .iter()
                .find(|ack| !client.moves_delivered.contains(&ack.to_bits()))
            {
                self.fail(&format!(
                    "ONCE: client {} got an ACK for move {ack} that never reached the lab",
                    client.index
                ));
            }
        }
        let stats = &lab.stats;
        if self.scenario.spawn_points > 0 {
            for (order, (index, location)) in self.placements.iter().enumerate() {
                if *location != spawn_point(order % self.scenario.spawn_points) {
                    self.fail(&format!(
                        "SPAWN: client {index} placed out of round-robin order"
                    ));
                }
            }
            if stats.pawn_spawns_configured != self.placements.len() as u64 {
                self.fail(&format!(
                    "SPAWN: {} points consumed for {} sessions",
                    stats.pawn_spawns_configured,
                    self.placements.len()
                ));
            }
        }
        let pairs = [
            (
                "record creations",
                stats.connected_player_record_creations_sent,
                stats.connected_player_record_creations_acked,
            ),
            (
                "possession updates",
                stats.controller_possession_updates_sent,
                stats.controller_possession_updates_acked,
            ),
            (
                "releases",
                stats.frozen_movement_releases_sent,
                stats.frozen_movement_releases_acked,
            ),
            (
                "releases scheduled",
                stats.frozen_movement_releases_scheduled,
                stats.frozen_movement_releases_sent,
            ),
            (
                "moves",
                stats.pawn_moves_received,
                stats.pawn_move_acks_sent,
            ),
            (
                "opens",
                stats.player_controller_opens_sent,
                stats.frozen_movement_releases_scheduled,
            ),
        ];
        if let Some((name, sent, acked)) = pairs.iter().find(|(_, sent, acked)| sent < acked) {
            self.fail(&format!("COUNTERS: {name} {sent} below {acked}"));
        }
        let closes: u64 = close_counts(lab).iter().sum::<u64>() - stats.peers_replaced;
        if stats.peers_created != lab.peers.len() as u64 + closes {
            self.fail(&format!(
                "COUNTERS: {} sessions created, {} live, {closes} closed",
                stats.peers_created,
                lab.peers.len()
            ));
        }
    }

    /// Step until every live client completes, all are gone, or `bound`.
    fn run_until(&mut self, bound: u64) {
        while self.step < bound {
            self.step();
            for index in 0..self.clients.len() {
                if self.clients[index].progress.completed_at.is_none() && self.complete(index) {
                    self.clients[index].progress.completed_at = Some(self.step);
                    self.trace.push(format!("s{} c{index} complete", self.step));
                }
            }
            let settled = (0..self.clients.len()).all(|index| {
                self.clients[index].progress.completed_at.is_some()
                    || self.outcomes[index].is_some()
                    || !self.clients[index].active()
            });
            if settled && !self.awaiting_delay() {
                return;
            }
        }
    }

    /// A release legitimately waiting out its delay in real time.
    fn awaiting_delay(&self) -> bool {
        let now = Instant::now();
        self.lab
            .peers
            .values()
            .any(|entry| entry.frozen_release.awaiting_delay(now))
    }

    /// Remove sessions whose client went silent the way the lab would once
    /// its timers run out: each timer pass at the first instant past the
    /// session's own stamps, which every other live session must not reach.
    fn expire_silent(&mut self, index: usize) {
        let address = self.clients[index].address;
        let Some(entry) = self.lab.peers.get(&address) else {
            return;
        };
        let timeout = self.lab.config.idle_timeout;
        let at = entry.last_seen + timeout + Duration::from_micros(1);
        for (other, client) in self.clients.iter().enumerate() {
            if other == index {
                continue;
            }
            if let Some(entry) = self.lab.peers.get(&client.address) {
                let stamps = [
                    Some(entry.last_seen),
                    entry.delivery_progress_at,
                    entry.friends_query_since,
                ];
                if stamps
                    .into_iter()
                    .flatten()
                    .any(|stamp| at.saturating_duration_since(stamp) > timeout)
                {
                    self.fail(&format!(
                        "TIMERS: expiring client {index} would also expire client {other}"
                    ));
                }
            }
        }
        let present: Vec<bool> = self
            .clients
            .iter()
            .map(|client| self.lab.peers.contains_key(&client.address))
            .collect();
        let before = close_counts(&self.lab);
        self.lab.expire_idle(at);
        self.lab.expire_stalled_delivery(at);
        self.lab.expire_friends_queries(at);
        self.note_removals(&present, before);
        if self.lab.peers.contains_key(&address) {
            self.fail(&format!(
                "STUCK: silent client {index} outlives every timer"
            ));
        }
    }

    fn execute(mut self) -> RunReport {
        let scenario = self.scenario;
        self.run_until(scenario.steps);
        let incomplete: Vec<usize> = (0..self.clients.len())
            .filter(|&index| {
                self.clients[index].progress.completed_at.is_none()
                    && self.outcomes[index].is_none()
            })
            .collect();
        if scenario.tier == Tier::Real
            && !incomplete.is_empty()
            && scenario.blackout_before.is_none()
        {
            self.fail(&format!(
                "LIVENESS: clients {incomplete:?} incomplete after {} faulty steps",
                scenario.steps
            ));
        }
        // Heal the link; silent or failed clients stay silent.
        self.healed = true;
        let healed_from = self.step;
        // At least a short healed stretch, so live clients' stamps are fresh
        // before any silent one's timers are run out.
        let fresh = self.step + MIN_HEALED_STEPS;
        while self.step < fresh {
            self.step();
        }
        self.run_until(healed_from + HEALED_STEPS);
        // A silent client, completed or not, is removed once its timers run
        // out; until then it may hold others' finalisation (N-019).
        for index in 0..self.clients.len() {
            if !self.clients[index].active() {
                self.expire_silent(index);
            }
        }
        // Whoever waited on a silent player now finishes.
        self.run_until(self.step + HEALED_STEPS);
        for index in 0..self.clients.len() {
            let alive = self.lab.peers.contains_key(&self.clients[index].address);
            if alive && self.clients[index].progress.completed_at.is_none() {
                let entry = &self.lab.peers[&self.clients[index].address];
                self.fail(&format!(
                    "STUCK: client {index} live but incomplete after healing (finalised {}, \
                     record acked {}, release sent {}, release acked {}, possession failed {}, \
                     friends pending {})",
                    entry.profiles.finalised,
                    entry.frozen_release.record_acked_at.is_some(),
                    entry.frozen_release.sent_at.is_some(),
                    entry.frozen_release.acked_at.is_some(),
                    entry.controller_possession_failed,
                    entry.friends_query_pending,
                ));
            }
        }
        // Quiet: nothing more may be sent twice.
        let quiet = self.step + QUIET_STEPS;
        while self.step < quiet {
            self.step();
        }
        let outcomes: Vec<Outcome> = (0..self.clients.len())
            .map(|index| {
                match (
                    self.clients[index].progress.completed_at,
                    self.outcomes[index],
                ) {
                    (Some(at), _) if at <= healed_from => Outcome::Completed,
                    (Some(_), _) => Outcome::CompletedAfterHealing,
                    (None, Some(outcome)) => outcome,
                    (None, None) => self.fail(&format!("client {index} has no outcome")),
                }
            })
            .collect();
        if scenario.tier == Tier::Real {
            for (index, outcome) in outcomes.iter().enumerate() {
                let silent = self.blackout.is_some_and(|(client, _)| client == index);
                if !silent && *outcome != Outcome::Completed {
                    self.fail(&format!("LIVENESS: client {index} ended {outcome:?}"));
                }
            }
        }
        if scenario.tier == Tier::Recover {
            for (index, outcome) in outcomes.iter().enumerate() {
                if !matches!(outcome, Outcome::Completed | Outcome::CompletedAfterHealing) {
                    self.fail(&format!(
                        "RECOVER: client {index} ended {outcome:?} after the link healed (errors {:?})",
                        error_counts(&self.lab)
                    ));
                }
            }
        }
        // Second player: whoever was neither silenced, cut off nor on a
        // harsh link completes, whatever happened to the other one.
        for (index, outcome) in outcomes.iter().enumerate() {
            let silent = self.blackout.is_some_and(|(client, _)| client == index)
                || self
                    .outage
                    .as_ref()
                    .is_some_and(|(client, _)| *client == index);
            let harsh = matches!(scenario.links[index], Link::HARSH | Link::HEAVY);
            if silent || harsh || self.clients[index].failed.is_some() {
                continue;
            }
            if !matches!(outcome, Outcome::Completed | Outcome::CompletedAfterHealing) {
                self.fail(&format!(
                    "SECOND PLAYER: client {index} on an ordinary link ended {outcome:?}"
                ));
            }
        }
        // Once per session, lab side.
        let stats = &self.lab.stats;
        let sessions = stats.peers_created;
        if stats.connected_player_records_created > sessions
            || stats.frozen_movement_releases_scheduled > sessions
            || stats.pawn_spawns_configured > sessions
        {
            self.fail(&format!(
                "ONCE: {} records, {} releases scheduled, {} spawn points for {sessions} sessions",
                stats.connected_player_records_created,
                stats.frozen_movement_releases_scheduled,
                stats.pawn_spawns_configured
            ));
        }
        RunReport {
            steps: self
                .clients
                .iter()
                .filter_map(|client| client.progress.completed_at)
                .collect(),
            move_acks: self
                .clients
                .iter()
                .map(|client| client.progress.move_acks.len() as u64)
                .sum(),
            release_resends: self.lab.stats.frozen_movement_release_resends,
            stale: self
                .clients
                .iter()
                .map(|client| client.progress.stale)
                .sum(),
            errors: error_counts(&self.lab),
            outcomes,
        }
    }
}

/// Run `seeds` of every scenario in `names`; returns per-scenario totals.
/// A failing seed is reported (its message names the scenario, seed, step and
/// last events) and the campaign goes on; it fails at the end if any did.
fn campaign(names: &[&str], seeds: impl Iterator<Item = u64> + Clone) -> Vec<String> {
    let mut lines = Vec::new();
    let mut failures = Vec::new();
    for name in names {
        let started = Instant::now();
        let scenario = scenario(name);
        let mut outcomes: BTreeMap<Outcome, u64> = BTreeMap::new();
        let (mut runs, mut steps, mut max_steps, mut acks, mut resends, mut stale) =
            (0, 0, 0, 0, 0, 0);
        let mut errors: BTreeMap<String, u64> = BTreeMap::new();
        let mut failed = 0;
        for seed in seeds.clone() {
            runs += 1;
            let report = match std::panic::catch_unwind(|| Run::new(scenario, seed).execute()) {
                Ok(report) => report,
                Err(panic) => {
                    failed += 1;
                    let message = panic
                        .downcast_ref::<String>()
                        .cloned()
                        .or_else(|| panic.downcast_ref::<&str>().map(|text| (*text).to_owned()))
                        .unwrap_or_default();
                    let first = message.lines().next().unwrap_or_default().to_owned();
                    failures.push(format!("{name} seed {seed}: {first}"));
                    continue;
                }
            };
            for outcome in report.outcomes {
                *outcomes.entry(outcome).or_default() += 1;
            }
            steps += report.steps.iter().sum::<u64>();
            max_steps = max_steps.max(report.steps.iter().copied().max().unwrap_or(0));
            acks += report.move_acks;
            resends += report.release_resends;
            stale += report.stale;
            for error in report.errors {
                *errors.entry(error).or_default() += 1;
            }
        }
        let line = format!(
            "{name}: {runs} seeds, {failed} failed, outcomes {outcomes:?}, steps to complete \
             total {steps} max {max_steps}, move acks {acks}, release resends {resends}, stale \
             datagrams {stale}, runs with error counters {errors:?}"
        );
        // As each scenario ends, so a bounded run that is cut off keeps them.
        println!("{line} ({:?})", started.elapsed());
        lines.push(line);
    }
    assert!(
        failures.is_empty(),
        "{} failing seeds:\n{}",
        failures.len(),
        failures.join("\n")
    );
    lines
}

/// Default tier: a fixed seed list over every scenario, in the normal suite.
#[test]
fn seeded_lossy_joins_complete_or_close_by_a_designed_path() {
    const SEEDS: [u64; 4] = [1, 0x5eed, 0xdead_beef, 20_261_112];
    let names: Vec<&str> = SCENARIOS.iter().map(|scenario| scenario.name).collect();
    campaign(&names, SEEDS.into_iter());
}

/// The fault model itself: keyed, so reproducible, and near its rates.
#[test]
fn fault_decisions_are_keyed_and_near_their_rates() {
    let link = Link::REAL;
    let draws: Vec<Vec<u64>> = (0..20_000)
        .map(|step| {
            decide(
                7,
                link,
                link.client_to_server_drop,
                &[CLIENT_TO_SERVER, 0, step],
            )
        })
        .collect();
    let again: Vec<Vec<u64>> = (0..20_000)
        .map(|step| {
            decide(
                7,
                link,
                link.client_to_server_drop,
                &[CLIENT_TO_SERVER, 0, step],
            )
        })
        .collect();
    assert_eq!(draws, again);
    let dropped = draws.iter().filter(|delays| delays.is_empty()).count();
    let duplicated = draws.iter().filter(|delays| delays.len() == 2).count();
    let delayed = draws.iter().flatten().filter(|delay| **delay > 0).count();
    assert!((3_600..4_400).contains(&dropped), "{dropped}");
    assert!((600..1_000).contains(&duplicated), "{duplicated}");
    assert!((1_400..2_200).contains(&delayed), "{delayed}");
    assert!(draws.iter().flatten().all(|delay| *delay <= link.max_delay));
    let other = decide(
        8,
        link,
        link.client_to_server_drop,
        &[CLIENT_TO_SERVER, 0, 0],
    );
    let differs = (0..64).any(|step| {
        decide(
            8,
            link,
            link.client_to_server_drop,
            &[CLIENT_TO_SERVER, 0, step],
        ) != draws[step as usize]
    });
    assert!(differs, "{other:?}");
}

fn env_u64(name: &str) -> Option<u64> {
    std::env::var(name).ok().map(|value| {
        value
            .parse()
            .unwrap_or_else(|_| panic!("{name} must be an unsigned integer"))
    })
}

/// Extended tier: `ICARUS_LOSSY_JOIN_SEEDS` seeds (default 200) per scenario,
/// long outages included, from `ICARUS_LOSSY_JOIN_FIRST_SEED` (default 1000).
#[test]
#[ignore = "seeded lossy-join campaign, extended tier; explicit run only"]
fn extended_lossy_join_campaign() {
    let count = env_u64("ICARUS_LOSSY_JOIN_SEEDS").unwrap_or(200);
    let first = env_u64("ICARUS_LOSSY_JOIN_FIRST_SEED").unwrap_or(1_000);
    let names: Vec<&str> = match std::env::var("ICARUS_LOSSY_JOIN_SCENARIO") {
        Ok(name) => vec![scenario(&name).name],
        Err(_) => SCENARIOS
            .iter()
            .chain(&LONG_OUTAGES)
            .map(|scenario| scenario.name)
            .collect(),
    };
    let started = Instant::now();
    campaign(&names, first..first + count);
    println!("elapsed {:?}", started.elapsed());
}

/// One scenario and seed from a failure report. The lab's own clock still
/// paces GameState updates and the release delay, so a seed is retried up to
/// `ICARUS_LOSSY_JOIN_REPEAT` times (default 20) until it fails.
#[test]
#[ignore = "reproduces one reported seed; explicit run only"]
fn reproduce_one() {
    let name = std::env::var("ICARUS_LOSSY_JOIN_SCENARIO").expect("ICARUS_LOSSY_JOIN_SCENARIO");
    let seed = env_u64("ICARUS_LOSSY_JOIN_SEED").expect("ICARUS_LOSSY_JOIN_SEED");
    let repeat = env_u64("ICARUS_LOSSY_JOIN_REPEAT").unwrap_or(20);
    for attempt in 1..=repeat {
        let report = std::panic::catch_unwind(|| Run::new(scenario(&name), seed).execute());
        match report {
            Ok(report) => println!("attempt {attempt}: {report:?}"),
            Err(panic) => {
                println!("attempt {attempt} failed");
                std::panic::resume_unwind(panic);
            }
        }
    }
}

/// Outages longer than 256 packets one way, then a healed link: every
/// session recovers (N-020, N-021). Four seeds here; the extended tier runs
/// these scenarios with the rest.
#[test]
fn long_outages_recover_after_the_link_heals() {
    let names: Vec<&str> = LONG_OUTAGES.iter().map(|scenario| scenario.name).collect();
    campaign(&names, 1_000..1_004);
}

/// One client joined over a clean link, its release ACKed.
fn clean_joined_run() -> Run {
    let mut run = Run::new(
        Scenario {
            name: "clean-one",
            tier: Tier::Real,
            clients: 1,
            links: [Link::CLEAN, Link::CLEAN],
            release_delay_ms: 0,
            move_acks: false,
            spawn_points: 0,
            blackout_before: None,
            outage: None,
            steps: 200,
        },
        1,
    );
    run.run_until(200);
    assert!(run.clients[0].progress.completed_at.is_some());
    run
}

/// N-020: 300 client packets lost in a row, then the link heals. The ACK
/// history clamps at eight words (F-535, F-759) and older sequences read as
/// lost; the session goes on. Before the fix the ACK header failed
/// (`history_word_count`, span above 256) and `send_acks` removed the session
/// (`ack_errors`, `peers_failed`).
#[test]
fn a_long_client_to_server_outage_does_not_close_the_session() {
    let mut run = clean_joined_run();
    let address = run.clients[0].address;
    let client = &mut run.clients[0].sequencer;
    for _ in 0..300 {
        client.header().unwrap();
        client.commit().unwrap();
    }
    for _ in 0..20 {
        run.step();
    }
    assert_eq!(run.lab.stats.ack_errors, 0);
    assert!(run.lab.peers.contains_key(&address), "the session survives");
}

/// N-021: every server packet is lost for a few steps, and the ring fills
/// with more lost packets (fewer than 256 client packets meanwhile, so N-020
/// cannot fire); then the link heals and the client keeps sending. The
/// server speaks again: each client packet gets the ring's newest datagram
/// once more, whose ACK drains the ring. Before the fix `send_acks` only set
/// `ack_capacity_blocked`, which only ACK or NAK progress clears, and the
/// session hung until the 30 s delivery stall removed it.
#[test]
fn a_full_sent_ring_does_not_hang_a_session_whose_client_keeps_sending() {
    let mut run = clean_joined_run();
    let address = run.clients[0].address;
    run.scenario.outage = Some(Outage {
        server_to_client: true,
        client_to_server: false,
        start_before: 1,
        steps: 3,
    });
    run.outage = Some((0, run.step..run.step + 3));
    for _ in 0..3 {
        run.step();
    }
    let entry = run.lab.peers.get_mut(&address).unwrap();
    // Committed and never sent: lost on the way to the client.
    while entry.peer.ack_packet().is_ok() {}
    assert_eq!(
        entry.peer.sequencer().notify().sent_records().len(),
        run.lab.config.packet.sent_capacity
    );
    let heard = run.clients[0].sequencer.notify().in_seq();
    for _ in 0..50 {
        run.step();
    }
    assert!(run.lab.peers.contains_key(&address));
    assert_ne!(
        run.clients[0].sequencer.notify().in_seq(),
        heard,
        "the server spoke again after the link healed"
    );
    let entry = &run.lab.peers[&address];
    assert!(!entry.ack_capacity_blocked);
    assert!(
        entry.peer.sequencer().notify().sent_records().len() < run.lab.config.packet.sent_capacity
    );
    assert!(run.lab.stats.ack_ring_probes_sent >= 1);
    assert_eq!(run.lab.stats.ack_errors, 0);
    assert_eq!(run.lab.stats.peers_delivery_stalled, 0);
}

/// N-021 at the delivery-stall deadline: a ring probe is no delivery
/// progress, so a client that never hears the server is still removed once
/// the deadline passes, however much it sends; a client whose ACK of a probe
/// arrives exactly at the deadline drains the ring and stays.
#[test]
fn ring_probes_never_extend_the_delivery_stall_and_a_heal_at_the_deadline_counts() {
    for heal in [false, true] {
        let mut lab = prospect_refresh_lab();
        let (socket, address, mut client) = acked_game_state_peer(&mut lab);
        let timeout = lab.config.idle_timeout;
        let capacity = lab.config.packet.sent_capacity;
        let start = Instant::now();
        let entry = lab.peers.get_mut(&address).unwrap();
        // Committed and never sent: every packet of the ring is lost.
        while entry.peer.ack_packet().is_ok() {}
        assert_eq!(
            entry.peer.sequencer().notify().sent_records().len(),
            capacity
        );
        let probe = entry.peer.full_ring_probe().unwrap().to_vec();
        entry.delivery_progress_at = Some(start);
        let to_server = lab.config.packet.packet;
        let to_client = PacketConfig {
            receiver: Receiver::Client,
            ..to_server
        };
        let client_packet = |client: &mut PacketSequencer| {
            let bytes = write_packet(
                &Packet {
                    magic: 0,
                    header: client.header().unwrap(),
                    info: None,
                    bunches: Vec::new(),
                },
                &to_server,
            )
            .unwrap();
            client.commit().unwrap();
            bytes
        };
        let mut buffer = [0; 2048];
        let sends = [Duration::from_secs(1), timeout / 2, timeout];
        for elapsed in sends {
            // Stale ACKs only: the client has heard nothing of the ring.
            let bytes = client_packet(&mut client);
            receive_authored_at(&mut lab, &socket, &bytes, start + elapsed);
            lab.send_acks();
            let length = socket.recv(&mut buffer).unwrap();
            assert_eq!(
                &buffer[..length],
                probe.as_slice(),
                "the newest datagram again"
            );
            let entry = &lab.peers[&address];
            assert!(entry.ack_capacity_blocked);
            assert_eq!(
                entry.delivery_progress_at,
                Some(start),
                "a probe is no progress"
            );
        }
        assert_eq!(lab.stats.ack_ring_probes_sent, sends.len() as u64);
        assert_eq!(lab.stats.acks_sent, 0);
        if heal {
            let packet = read_packet(&probe, &to_client).unwrap();
            let received = client.receive(&packet.header).unwrap().unwrap();
            client.ack_seq(received.sequence, true).unwrap();
            // A second copy is a duplicate at the client's sequence gate.
            assert_eq!(client.receive(&packet.header).unwrap(), None);
            let bytes = client_packet(&mut client);
            receive_authored_at(&mut lab, &socket, &bytes, start + timeout);
            let entry = &lab.peers[&address];
            assert!(!entry.ack_capacity_blocked);
            assert!(entry.peer.sequencer().notify().sent_records().is_empty());
            assert_eq!(entry.delivery_progress_at, Some(start + timeout));
        }
        lab.expire_stalled_delivery(start + timeout);
        assert!(lab.has_peer(address), "not past the deadline yet");
        lab.expire_stalled_delivery(start + timeout + Duration::from_micros(1));
        assert_eq!(lab.has_peer(address), heal);
        assert_eq!(lab.stats.peers_delivery_stalled, u64::from(!heal));
        if heal {
            lab.send_acks();
            assert_eq!(lab.stats.acks_sent, 1, "an ordinary ACK again");
        }
    }
}

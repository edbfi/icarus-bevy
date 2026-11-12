//! Pawn move acknowledgements over the lab's real receive path, driven by the
//! project's own client sequencer with explicit receive and pass instants.
//! Every move is authored here by hand from the packed layout; no captured
//! payload, timestamp, location, GUID or account value is used.
use super::super::super::{
    actor_channel::{ActorBunchBody, BlockTarget, ContentBlock, encode_actor_bunch_body},
    archive::ByteOrder,
    character_move::{
        MoveFields, NewMove, PackedMove, read_good_move_ack, write_server_move_packed,
    },
    packet::{BitWriter, Bunch, BunchHeader, Packet, write_packet},
    rep_layout::{VectorPacking, append_bits, quantize_vector},
    rep_rpc::{
        BP_ICARUS_PLAYER_CHARACTER_SURVIVAL_FIELD_INDEX_BOUND as PAWN_FIELD_BOUND,
        CLIENT_MOVE_RESPONSE_PACKED_FIELD_INDEX, FieldHeader, SERVER_MOVE_PACKED_FIELD_INDEX,
        read_field_header, write_field_header,
    },
    sequencing::PacketSequencer,
};
use super::*;

/// Write `value` as the packed quantized vector `packing` names.
fn write_quantized(writer: &mut BitWriter, value: [f32; 3], packing: VectorPacking) {
    let vector = quantize_vector(value, packing).unwrap();
    writer
        .write_int_max(vector.bits, packing.bits_max())
        .unwrap();
    for component in vector.components {
        writer
            .write_int_max(component, 1 << (vector.bits + 2))
            .unwrap();
    }
}

/// One authored field-39 payload written by hand from the packed layout: the
/// presence bit, the packed container bit count, then a new move (timestamp,
/// zero acceleration, `location`, a yaw-only rotation, no flags, base or
/// bone, `mode` when given), no pending or old move and the final bit.
pub(super) fn authored_move(timestamp: f32, location: [f32; 3], mode: Option<u8>) -> Payload {
    let mut data = BitWriter::new();
    data.write_u32(timestamp.to_bits());
    write_quantized(&mut data, [0.0; 3], VectorPacking::NetQuantize10);
    write_quantized(&mut data, location, VectorPacking::NetQuantize100);
    for axis in [None, Some(0x4000_u32), None] {
        data.write_bit(axis.is_some());
        if let Some(value) = axis {
            data.write_bits(value, 16);
        }
    }
    data.write_bit(false);
    data.write_bit(false);
    data.write_bit(false);
    data.write_bit(mode.is_some());
    if let Some(mode) = mode {
        data.write_bits(u32::from(mode), 8);
    }
    for _ in 0..3 {
        data.write_bit(false);
    }
    let data = Payload::from_writer(data);
    let mut payload = BitWriter::new();
    payload.write_bit(true);
    payload.write_int_packed(u32::try_from(data.bit_len()).unwrap());
    append_bits(&mut payload, &data);
    Payload::from_writer(payload)
}

/// A pawn bunch body: one actor block holding field 39 (bound 129) with
/// `field` as its payload, as a client sends its packed move.
pub(super) fn move_bunch_payload(field: Payload, engine: u32) -> Payload {
    let mut fields = BitWriter::new();
    write_field_header(
        &mut fields,
        &FieldHeader {
            index: SERVER_MOVE_PACKED_FIELD_INDEX,
            payload: field,
        },
        PAWN_FIELD_BOUND,
    )
    .unwrap();
    encode_actor_bunch_body(
        &ActorBunchBody {
            spawn: None,
            blocks: vec![ContentBlock {
                has_rep_layout: false,
                target: BlockTarget::Actor,
                payload: Payload::from_writer(fields),
            }],
        },
        &actor_codec(engine, ByteOrder::Little),
    )
    .unwrap()
}

/// One unreliable client bunch on `channel`.
pub(super) fn unreliable_bunch(channel: u32, payload: Payload) -> Bunch {
    Bunch {
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
            partial: false,
            reliable_sequence_raw: None,
            partial_initial: false,
            partial_final: false,
            channel_name: None,
        },
        payload,
    }
}

/// Send `bunches` in one client packet received at `now`.
pub(super) fn send_at(
    lab: &mut PeerLab,
    socket: &UdpSocket,
    client: &mut PacketSequencer,
    bunches: Vec<Bunch>,
    now: Instant,
) {
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
    receive_authored_at(lab, socket, &bytes, now);
}

/// The authored move sequence of the digest sessions, `(milliseconds after
/// the first move, field-39 payload)`: 24 moves 24 ms apart walking along x,
/// a timestamp reset after the twelfth, and one malformed payload.
fn digest_moves() -> Vec<(u64, Payload)> {
    (0..24_u16)
        .map(|index| {
            let at = u64::from(index) * 24;
            let timestamp = if index < 12 {
                10.0 + f32::from(index) * 0.024
            } else {
                0.5 + f32::from(index - 12) * 0.024
            };
            let payload = if index == 6 {
                let mut junk = BitWriter::new();
                junk.write_bits(0x2a5, 10);
                Payload::from_writer(junk)
            } else {
                authored_move(
                    timestamp,
                    [1000.0 + f32::from(index) * 10.0, -500.0, 200.0],
                    Some(1),
                )
            };
            (at, payload)
        })
        .collect()
}

/// Two sequential sessions on `lab`, each from the stateless handshake and
/// Join through its connected-player record's ACK and the frozen-movement
/// release pass, then [`digest_moves`] as unreliable pawn bunches at explicit
/// instants, each followed by `ack_pass` at that instant and delivery: every
/// bunch the client receives, digested per session as
/// `switch_off_session_digests` digests them (GameState-channel bunches,
/// which carry the lab clock, by header, length and block targets).
pub(super) fn move_session_digests(
    lab: &mut PeerLab,
    ack_pass: impl Fn(&mut PeerLab, Instant),
) -> Vec<String> {
    use super::super::super::control_messages::{NetIdType, NetIdValue, UniqueNetId};
    let channel = game_state_channel(lab);
    let codec = actor_codec(17, ByteOrder::Little);
    let (pawn_channel, engine) = {
        let (plan, channel, _) = lab.pawn_open.as_ref().unwrap();
        (*channel, plan.engine_net_version)
    };
    let mut digests = Vec::new();
    for (connection, user) in [(1u32, "AuthoredMoveFirst"), (2, "AuthoredMoveSecond")] {
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
            name_units: "AUTHORED-MOVE".encode_utf16().collect(),
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
        lab.retry_frozen_movement_release();
        bunches.extend(deliver_bunches(lab, &socket, &mut client, |_| true));
        let start = Instant::now();
        for (at, field) in digest_moves() {
            let now = start + Duration::from_millis(at);
            send_at(
                lab,
                &socket,
                &mut client,
                vec![unreliable_bunch(
                    pawn_channel,
                    move_bunch_payload(field, engine),
                )],
                now,
            );
            ack_pass(lab, now);
            bunches.extend(deliver_bunches(lab, &socket, &mut client, |_| true));
        }
        assert_eq!(lab.peers[&address].pawn_inbound.unreliable_bunches, 24);
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
                let body = super::super::super::actor_channel::read_actor_bunch_body(
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

/// [`record_lab`] with the frozen-movement release (no delay), as
/// production defaults, and move acknowledgements when `acks`.
pub(super) fn moving_lab(acks: bool) -> PeerLab {
    let mut lab = record_lab(true);
    lab.enable_frozen_movement_release(Duration::ZERO).unwrap();
    if acks {
        lab.enable_pawn_move_acks().unwrap();
    }
    lab
}

/// One joined player through its record's ACK and the frozen-movement
/// release, every server packet ACKed: the pawn channel is open and settled.
pub(super) fn moving_peer(lab: &mut PeerLab) -> (UdpSocket, SocketAddrV4, PacketSequencer) {
    let (socket, address, mut client) = record_peer(lab, 1, "AuthoredMover");
    complete_record_player(lab, address);
    lab.controller_call(address, ControllerCall::FriendsList);
    lab.finalise_connected_players();
    lab.retry_connected_player_records();
    deliver_bunches(lab, &socket, &mut client, |_| true);
    lab.retry_frozen_movement_release();
    deliver_bunches(lab, &socket, &mut client, |_| true);
    assert_eq!(lab.stats.frozen_movement_releases_sent, 1);
    (socket, address, client)
}

fn pawn(lab: &PeerLab) -> (u32, u32) {
    let (plan, channel, _) = lab.pawn_open.as_ref().unwrap();
    (*channel, plan.engine_net_version)
}

/// The acknowledged timestamps among `bunches`: whole unreliable pawn
/// bunches of one actor block without a rep layout holding field 30 alone,
/// read with the repository's readers. Any other pawn bunch fails.
fn acks_in(lab: &PeerLab, bunches: &[Bunch]) -> Vec<f32> {
    let (channel, engine) = pawn(lab);
    let codec = actor_codec(engine, ByteOrder::Little);
    bunches
        .iter()
        .filter(|bunch| bunch.header.channel_index == channel)
        .map(|bunch| {
            assert!(!bunch.header.reliable && !bunch.header.open && !bunch.header.partial);
            let body = super::super::super::actor_channel::read_actor_bunch_body(
                &mut bunch.payload.reader(),
                false,
                &codec,
            )
            .unwrap();
            let [block] = body.blocks.as_slice() else {
                panic!("one block");
            };
            assert!(!block.has_rep_layout);
            assert_eq!(block.target, BlockTarget::Actor);
            assert_eq!(block.payload.bit_len(), 57);
            let mut reader = block.payload.reader();
            let field = read_field_header(&mut reader, PAWN_FIELD_BOUND, usize::MAX).unwrap();
            assert!(reader.at_end());
            assert_eq!(field.index, CLIENT_MOVE_RESPONSE_PACKED_FIELD_INDEX);
            read_good_move_ack(&field.payload, ByteOrder::Little).unwrap()
        })
        .collect()
}

/// Send one move bunch carrying `field` at `now`, run the ack pass at `now`
/// and return the acknowledged timestamps the client received.
pub(super) fn move_at(
    lab: &mut PeerLab,
    socket: &UdpSocket,
    client: &mut PacketSequencer,
    field: Option<Payload>,
    now: Instant,
) -> Vec<f32> {
    let (channel, engine) = pawn(lab);
    if let Some(field) = field {
        send_at(
            lab,
            socket,
            client,
            vec![unreliable_bunch(channel, move_bunch_payload(field, engine))],
            now,
        );
    }
    lab.retry_pawn_move_acks(now);
    let bunches = deliver_bunches(lab, socket, client, |_| true);
    acks_in(lab, &bunches)
}

/// What a synthetic movement run saw: each move's arrival (milliseconds,
/// timestamp) and each acknowledgement's (milliseconds, timestamp).
struct RateRun {
    arrivals: Vec<(u64, f32)>,
    acks: Vec<(u64, f32)>,
}

/// Moves every `period_ms` for `duration_ms`, the client clock starting at 5 s
/// and reset to 0.25 s at `reset_ms`, walking 1 m/s along x; the poll's ack
/// pass runs every 4 ms of lab time in between.
fn rate_run(acks: bool, period_ms: u64, duration_ms: u64, reset_ms: u64) -> (PeerLab, RateRun) {
    let mut lab = moving_lab(acks);
    let (socket, _, mut client) = moving_peer(&mut lab);
    let start = Instant::now();
    let mut run = RateRun {
        arrivals: Vec::new(),
        acks: Vec::new(),
    };
    for tick in (0..duration_ms).step_by(4) {
        let field = tick.is_multiple_of(period_ms).then(|| {
            let timestamp = if tick < reset_ms {
                5.0 + tick as f32 / 1000.0
            } else {
                0.25 + (tick - reset_ms) as f32 / 1000.0
            };
            run.arrivals.push((tick, timestamp));
            authored_move(timestamp, [tick as f32 / 10.0, 0.0, 100.0], Some(1))
        });
        let acked = move_at(
            &mut lab,
            &socket,
            &mut client,
            field,
            start + Duration::from_millis(tick),
        );
        run.acks
            .extend(acked.into_iter().map(|timestamp| (tick, timestamp)));
    }
    (lab, run)
}

/// At about 42 and 83 moves a second the newest move is acknowledged at
/// most every 100 ms (at most ten in any second), never without a new
/// move, each time with the timestamp of the move that arrived last, also
/// across a client timestamp reset; counters agree, nothing waits for
/// reliable repair.
#[test]
fn moves_are_acknowledged_newest_by_arrival_at_most_ten_a_second() {
    for (period_ms, minimum) in [(24_u64, 16_usize), (12, 18)] {
        let (lab, run) = rate_run(true, period_ms, 2000, 1000);
        let label = format!("period {period_ms} ms");
        assert!(run.acks.len() >= minimum, "{label}: {}", run.acks.len());
        for pair in run.acks.windows(2) {
            assert!(pair[1].0 - pair[0].0 >= 100, "{label}: {pair:?}");
        }
        for (at, _) in &run.acks {
            let window = run
                .acks
                .iter()
                .filter(|(other, _)| (*at..at + 1000).contains(other))
                .count();
            assert!(window <= 10, "{label}: {window} in a second");
        }
        for (at, timestamp) in &run.acks {
            let newest = run
                .arrivals
                .iter()
                .rev()
                .find(|(arrived, _)| arrived <= at)
                .unwrap();
            assert_eq!(timestamp.to_bits(), newest.1.to_bits(), "{label} at {at}");
        }
        // The reset: later acknowledgements carry the smaller timestamps.
        assert!(
            run.acks
                .iter()
                .any(|(at, timestamp)| *at >= 1000 && *timestamp < 1.0),
            "{label}"
        );
        let moves = run.arrivals.len() as u64;
        assert_eq!(lab.stats.pawn_moves_received, moves);
        assert_eq!(lab.stats.pawn_moves_malformed, 0);
        assert_eq!(lab.stats.pawn_move_acks_sent, run.acks.len() as u64);
        assert_eq!(lab.stats.pawn_move_ack_errors, 0);
        assert_eq!(lab.stats.pawn_move_ack_sends_deferred, 0);
        assert_eq!(lab.stats.pawn_move_positions_accepted, moves);
        assert_eq!(lab.stats.pawn_move_positions_rejected, 0);
        let counts = lab.session_counts();
        assert_eq!(counts.pawn_moves_received, moves);
        assert_eq!(counts.pawn_move_acks_sent, run.acks.len() as u64);
        let readiness = lab.readiness_counts();
        assert_eq!(readiness.resend_bunches_pending, 0);
        assert_eq!(readiness.pawn_move_acks.sessions_moving, 1);
        assert_eq!(readiness.pawn_move_acks.positions_held, 1);
        let [observation] = lab.frozen_movement_observations()[..] else {
            panic!("one session");
        };
        assert_eq!(observation.pawn_moves.moves, moves);
        assert_eq!(observation.pawn_moves.acks_sent, run.acks.len() as u64);
        assert_eq!(observation.pawn_moves.mode, Some(1));
        assert!(observation.last_move_ack_ms.is_some());
        assert_eq!(observation.pawn_inbound.unreliable_bunches, moves);
    }
}

/// Switched off (the default) the same traffic gets no acknowledgement and
/// moves no move counter; the bunches are still counted as before.
#[test]
fn switched_off_moves_get_no_acknowledgement() {
    assert!(!crate::config::NetworkConfig::default().pawn_move_acks);
    let (lab, run) = rate_run(false, 12, 600, 300);
    assert!(!lab.has_pawn_move_acks());
    assert!(run.acks.is_empty());
    assert_eq!(lab.stats.pawn_moves_received, 0);
    assert_eq!(lab.stats.pawn_move_acks_sent, 0);
    assert_eq!(lab.stats.pawn_move_positions_accepted, 0);
    assert_eq!(
        lab.stats.pawn_bunches_received_unreliable,
        run.arrivals.len() as u64
    );
    assert_eq!(
        lab.readiness_counts().pawn_move_acks,
        PawnMoveAckReadiness::default()
    );
    let [observation] = lab.frozen_movement_observations()[..] else {
        panic!("one session");
    };
    assert_eq!(observation.pawn_moves, PawnMoveCounts::default());
    assert_eq!(observation.last_move_ack_ms, None);
}

/// Malformed moves (a refused payload, an unreadable bunch, a non-finite or
/// negative timestamp) are counted and skipped: no acknowledgement, no
/// close; a well-formed move afterwards is acknowledged, and a later
/// malformed one does not make it due again.
#[test]
fn malformed_moves_are_counted_and_never_acknowledged() {
    let mut lab = moving_lab(true);
    let (socket, address, mut client) = moving_peer(&mut lab);
    let (channel, _) = pawn(&lab);
    let start = Instant::now();
    let mut junk = BitWriter::new();
    junk.write_bits(0x155, 9);
    let junk = Payload::from_writer(junk);
    let fields = [
        Some(junk.clone()),
        Some(authored_move(f32::NAN, [0.0; 3], None)),
        Some(authored_move(f32::INFINITY, [0.0; 3], None)),
        Some(authored_move(-1.0, [0.0; 3], None)),
    ];
    for (index, field) in fields.into_iter().enumerate() {
        let at = start + Duration::from_millis(index as u64 * 150);
        assert!(move_at(&mut lab, &socket, &mut client, field, at).is_empty());
    }
    // A bunch whose body cannot be read at all.
    send_at(
        &mut lab,
        &socket,
        &mut client,
        vec![unreliable_bunch(channel, junk.clone())],
        start + Duration::from_millis(700),
    );
    lab.retry_pawn_move_acks(start + Duration::from_millis(700));
    assert!(deliver_bunches(&mut lab, &socket, &mut client, |_| true).is_empty());
    assert_eq!(lab.stats.pawn_moves_malformed, 5);
    assert_eq!(lab.stats.pawn_moves_received, 0);
    assert_eq!(lab.stats.pawn_move_acks_sent, 0);
    assert!(lab.has_peer(address));
    let good = authored_move(7.5, [0.0, 0.0, 100.0], None);
    let acked = move_at(
        &mut lab,
        &socket,
        &mut client,
        Some(good),
        start + Duration::from_millis(800),
    );
    assert_eq!(acked, [7.5]);
    let acked = move_at(
        &mut lab,
        &socket,
        &mut client,
        Some(junk),
        start + Duration::from_millis(1000),
    );
    assert!(acked.is_empty());
    assert_eq!(lab.stats.pawn_moves_malformed, 6);
    assert_eq!(lab.stats.pawn_move_acks_sent, 1);
    assert_eq!(lab.peers[&address].pawn_moves.counts.malformed, 6);
    assert!(lab.has_peer(address));
}

/// A move that arrives while the pawn open is still unACKed is decoded but
/// not acknowledged; the acknowledgement follows once the open settles.
#[test]
fn no_acknowledgement_before_the_pawn_channel_settles() {
    use super::super::super::control_messages::{NetIdType, NetIdValue, UniqueNetId};
    let mut lab = moving_lab(true);
    let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    socket.connect(lab.local_addr().unwrap()).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(1)))
        .unwrap();
    let (address, mut client) = handshake_peer_on(&mut lab, &socket);
    let entry = lab.peers.get_mut(&address).unwrap();
    entry.connection_id = Some(1);
    entry.player = Some(RegisteredPlayer {
        player_id: 1,
        name_units: "AUTHORED-MOVE".encode_utf16().collect(),
    });
    entry.login_unique_id = Some(UniqueNetId {
        kind: NetIdType::Named(ArchiveText::text("NULL").unwrap()),
        value: NetIdValue::Text(ArchiveText::text("AuthoredEarlyMover").unwrap()),
    });
    entry.profiles.character_slot = Some(3);
    assert!(lab.send_player_controller_open(address));
    let (channel, engine) = pawn(&lab);
    assert!(!lab.peers[&address].peer.actor_channel_settled(channel));
    let start = Instant::now();
    send_at(
        &mut lab,
        &socket,
        &mut client,
        vec![unreliable_bunch(
            channel,
            move_bunch_payload(authored_move(2.0, [0.0, 0.0, 100.0], Some(1)), engine),
        )],
        start,
    );
    lab.retry_pawn_move_acks(start + Duration::from_millis(200));
    assert_eq!(lab.stats.pawn_moves_received, 1);
    assert_eq!(lab.stats.pawn_move_acks_sent, 0);
    assert_eq!(lab.readiness_counts().pawn_move_acks.awaiting_ack, 1);
    let opens = deliver_bunches(&mut lab, &socket, &mut client, |_| true);
    // Only the reliable opens went out on the pawn channel: no acknowledgement.
    assert!(
        opens
            .iter()
            .all(|bunch| bunch.header.channel_index != channel || bunch.header.reliable)
    );
    assert!(lab.peers[&address].peer.actor_channel_settled(channel));
    let acked = move_at(
        &mut lab,
        &socket,
        &mut client,
        None,
        start + Duration::from_millis(300),
    );
    assert_eq!(acked, [2.0]);
    assert_eq!(lab.readiness_counts().pawn_move_acks.awaiting_ack, 0);
}

pub(super) fn packed(location: [f32; 3], base: Option<u32>, mode: Option<u8>) -> Payload {
    use super::super::super::actor_channel::CompressedRotator;
    let quantized = |value, packing| quantize_vector(value, packing).unwrap();
    let codec = RepLayoutCodec {
        actor: actor_codec(17, ByteOrder::Little),
        checksums: false,
        limits: Default::default(),
    };
    write_server_move_packed(
        &PackedMove {
            new: NewMove {
                fields: MoveFields {
                    timestamp: 3.0,
                    acceleration: quantized([0.0; 3], VectorPacking::NetQuantize10),
                    location: quantized(location, VectorPacking::NetQuantize100),
                    rotation: CompressedRotator::default(),
                    flags: None,
                },
                base,
                bone: None,
                mode,
            },
            pending: None,
            old: None,
            disable_combined: false,
        },
        &codec,
    )
    .unwrap()
}

/// The authored location rule (F-1128): a world move is stored; one with a
/// base (an unknown NetGUID) or no movement mode is skipped; one outside the
/// world bound or farther than the speed bound allows is refused, logged at
/// most once per interval and leaves the stored location; time restores
/// the allowance. Nothing is ever sent back but acknowledgements.
#[test]
fn locations_are_stored_within_authored_bounds_and_never_corrected() {
    let mut lab = moving_lab(true);
    let (socket, address, mut client) = moving_peer(&mut lab);
    let start = Instant::now();
    let at = |ms: u64| start + Duration::from_millis(ms);
    let stored = |lab: &PeerLab| lab.peers[&address].pawn_moves.location.map(|(at, _)| at);
    let mut send = |lab: &mut PeerLab, field: Payload, ms: u64| {
        move_at(lab, &socket, &mut client, Some(field), at(ms))
    };
    send(&mut lab, packed([1000.0, 2000.0, 300.0], None, None), 0);
    assert_eq!(stored(&lab), Some([1000.0, 2000.0, 300.0]));
    // Based (an unknown base GUID) and mode None: skipped.
    send(
        &mut lab,
        packed([9.0e5, 0.0, 0.0], Some(0x7fff_fff1), Some(1)),
        30,
    );
    send(&mut lab, packed([9.0e5, 0.0, 0.0], None, Some(0)), 60);
    assert_eq!(stored(&lab), Some([1000.0, 2000.0, 300.0]));
    assert_eq!(lab.stats.pawn_move_positions_skipped, 2);
    // A teleport 1 km away 90 ms later: refused, logged once.
    send(
        &mut lab,
        packed([101_000.0, 2000.0, 300.0], None, Some(1)),
        90,
    );
    let logged = lab.peers[&address].pawn_moves.last_refusal_logged_at;
    assert_eq!(logged, Some(at(90)));
    // Outside the world bound: refused, not logged again within 10 s.
    send(&mut lab, packed([2.0e6, 0.0, 0.0], None, Some(3)), 120);
    assert_eq!(
        lab.peers[&address].pawn_moves.last_refusal_logged_at,
        logged
    );
    assert_eq!(stored(&lab), Some([1000.0, 2000.0, 300.0]));
    assert_eq!(lab.stats.pawn_move_positions_rejected, 2);
    // A walk within the bound is stored, relative to the stored location.
    send(
        &mut lab,
        packed([1100.0, 2000.0, 300.0], None, Some(1)),
        150,
    );
    assert_eq!(stored(&lab), Some([1100.0, 2000.0, 300.0]));
    // 20 s later the teleport is within 5,000 cm/s plus slack.
    send(
        &mut lab,
        packed([101_000.0, 2000.0, 300.0], None, Some(3)),
        20_150,
    );
    assert_eq!(stored(&lab), Some([101_000.0, 2000.0, 300.0]));
    assert_eq!(lab.stats.pawn_move_positions_accepted, 3);
    assert_eq!(lab.stats.pawn_moves_received, 7);
    // Only acknowledgements went out (acks_in panics on anything else): at
    // 0, 120 and 20,150 ms, each at least 100 ms after the last.
    assert_eq!(lab.stats.pawn_move_acks_sent, 3);
    assert_eq!(lab.peers[&address].pawn_moves.counts.mode, Some(3));
}

#[test]
fn location_checks_refuse_non_finite_outside_and_too_far() {
    let now = Instant::now();
    assert_eq!(check_move_location(None, [0.0, 1.0, 2.0], now), Ok(()));
    for location in [
        [f64::NAN, 0.0, 0.0],
        [0.0, f64::INFINITY, 0.0],
        [0.0, 0.0, f64::NEG_INFINITY],
    ] {
        assert_eq!(
            check_move_location(None, location, now),
            Err(MoveLocationRefusal::NotFinite)
        );
    }
    assert_eq!(
        check_move_location(None, [0.0, 0.0, -1_048_577.0], now),
        Err(MoveLocationRefusal::OutsideWorld)
    );
    assert_eq!(
        check_move_location(None, [1_048_576.0, -1_048_576.0, 0.0], now),
        Ok(())
    );
    let previous = Some(([0.0; 3], now));
    let later = now + Duration::from_secs(1);
    // 5,000 cm/s for one second plus 500 cm slack.
    assert_eq!(
        check_move_location(previous, [5_500.0, 0.0, 0.0], later),
        Ok(())
    );
    assert_eq!(
        check_move_location(previous, [5_501.0, 0.0, 0.0], later),
        Err(MoveLocationRefusal::TooFar)
    );
    assert_eq!(
        check_move_location(previous, [501.0, 0.0, 0.0], now),
        Err(MoveLocationRefusal::TooFar)
    );
}

/// The switch API: configured before admission, needs the pawn open.
#[test]
fn move_acknowledgements_need_the_pawn_and_are_configured_before_admission() {
    let mut lab = PeerLab::bind(0, PeerLabConfig::default()).unwrap();
    assert!(lab.enable_pawn_move_acks().is_err());
    assert!(!lab.has_pawn_move_acks());
    let mut lab = moving_lab(false);
    lab.enable_pawn_move_acks().unwrap();
    assert!(lab.has_pawn_move_acks());
    lab.disable_pawn_move_acks().unwrap();
    assert!(!lab.has_pawn_move_acks());
    let mut lab = moving_lab(true);
    moving_peer(&mut lab);
    assert!(lab.disable_pawn_move_acks().is_err());
    assert!(lab.enable_pawn_move_acks().is_err());
    assert!(lab.has_pawn_move_acks());
}

pub(super) fn release_toml_config(pawn_move_acks: bool) -> crate::config::NetworkConfig {
    crate::config::NetworkConfig {
        enabled: true,
        port: 0,
        release_profile: Some(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("config/release.toml"),
        ),
        pawn_move_acks,
        ..crate::config::NetworkConfig::default()
    }
}

/// The parent commit's [`move_session_digests`] for the lab (`LAB`) and the
/// production bind (`PRODUCTION`); how they were produced is below.
pub(super) const LAB: [&str; 2] = [
    "51b48993f8d7a34c0c655a16b169c0f5cd1097d89c5d208a3b821f1f2078ae9b",
    "ac4c2ab72c060778a5d7938ba0c0affab2d4593fe730d6b719b23d3fb0c2cf28",
];
pub(super) const PRODUCTION: [&str; 2] = [
    "5b189e9ba5356ef236e0299e6884fc8cb288223b43979ce215b400deced107a1",
    "d45e777826d5334fda7cf6ea57992bac2c8220cca3cc41d9d5f91c1ac631a042",
];

/// The switch-off path pinned to the parent of the move acknowledgement
/// change. These digests were produced by [`move_session_digests`] with a
/// no-op `ack_pass` on 66462ba's `src/` with only this module's helpers
/// added (before the switch and its pass existed), not by this build: a lab
/// that never touches the switch, one that turns it on and then off, and the
/// production bind with `pawn_move_acks = false` (the default) must send
/// exactly those bunches, the pass running after every move; with the
/// switch on every session differs.
#[test]
fn switch_off_moves_send_the_parent_commits_bunches() {
    let pass = |lab: &mut PeerLab, now: Instant| lab.retry_pawn_move_acks(now);
    let mut untouched = moving_lab(false);
    assert_eq!(move_session_digests(&mut untouched, pass), LAB);
    assert_eq!(untouched.stats.pawn_moves_received, 0);
    let mut toggled = moving_lab(true);
    toggled.disable_pawn_move_acks().unwrap();
    assert_eq!(move_session_digests(&mut toggled, pass), LAB);
    let mut production = crate::network::bind(&release_toml_config(false))
        .unwrap()
        .unwrap()
        .lab;
    assert!(!production.has_pawn_move_acks());
    assert_eq!(move_session_digests(&mut production, pass), PRODUCTION);
    assert_eq!(production.stats.pawn_move_acks_sent, 0);
    let mut on = moving_lab(true);
    let digests = move_session_digests(&mut on, pass);
    assert!(digests.iter().zip(LAB).all(|(on, off)| on != off));
    assert!(on.stats.pawn_move_acks_sent >= 4);
    // Two sessions of 23 well-formed moves and one malformed each.
    assert_eq!(on.stats.pawn_moves_received, 46);
    assert_eq!(on.stats.pawn_moves_malformed, 2);
    let mut production_on = crate::network::bind(&release_toml_config(true))
        .unwrap()
        .unwrap()
        .lab;
    assert!(production_on.has_pawn_move_acks());
    let digests = move_session_digests(&mut production_on, pass);
    assert!(digests.iter().zip(PRODUCTION).all(|(on, off)| on != off));
}

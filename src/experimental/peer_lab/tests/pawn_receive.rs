//! Pawn-channel receive metadata over the lab's real receive path. Every
//! bunch, field index, payload and sub-object block is authored here; no
//! captured payload, GUID, call identity or account value is used, and no
//! row establishes what a client call means.
use super::super::super::{
    actor_channel::{
        ActorBunchBody, BlockTarget, ContentBlock, NetGuidRef, SubObjectClass,
        encode_actor_bunch_body,
    },
    packet::{BitWriter, Bunch, Name, Payload},
    pawn_open::{FastArrayContent, PawnInventory},
    rep_rpc::{
        BP_ICARUS_PLAYER_CHARACTER_SURVIVAL_FIELD_INDEX_BOUND as PAWN_FIELD_BOUND, FieldHeader,
        SERVER_MOVE_PACKED_FIELD_INDEX, write_field_header,
    },
    sequencing::{PacketSequencer, wire_reliable_sequence},
    session_open::ACTOR_CHANNEL_NAME_INDEX,
};
use super::super::pawn_receive::{
    MoveOutcomes, PawnSubobjectMapping, observe_pawn_bunch, observe_pawn_transport_drop,
};
use super::move_acks::{
    authored_move, move_bunch_payload, moving_lab, moving_peer, send_at, unreliable_bunch,
};
use super::*;

/// A joined peer whose pawn opened with two empty runtime inventories.
struct Session {
    lab: PeerLab,
    socket: UdpSocket,
    address: SocketAddrV4,
    client: PacketSequencer,
    channel: u32,
    engine: u32,
    next_reliable: u32,
}

impl Session {
    fn new(acks: bool) -> Self {
        let mut lab = moving_lab(acks);
        let inventory = || PawnInventory {
            info_row: 0,
            current_weight: None,
            slots: FastArrayContent::default(),
        };
        lab.pawn_open.as_mut().unwrap().2.inventories = vec![inventory(), inventory()];
        let (socket, address, client) = moving_peer(&mut lab);
        let (plan, channel, _) = lab.pawn_open.as_ref().unwrap();
        let (channel, engine) = (*channel, plan.engine_net_version);
        let next_reliable = client.initial_reliable().1 + 1;
        Self {
            lab,
            socket,
            address,
            client,
            channel,
            engine,
            next_reliable,
        }
    }

    fn body(&self, blocks: Vec<ContentBlock>) -> Payload {
        encode_actor_bunch_body(
            &ActorBunchBody {
                spawn: None,
                blocks,
            },
            &actor_codec(self.engine, self.lab.config.packet.codec.order),
        )
        .unwrap()
    }

    fn reliable(&mut self, payload: Payload) -> Bunch {
        let mut bunch = unreliable_bunch(self.channel, payload);
        bunch.header.reliable = true;
        bunch.header.reliable_sequence_raw = Some(wire_reliable_sequence(self.next_reliable));
        bunch.header.channel_name = Some(Name::Hardcoded(ACTOR_CHANNEL_NAME_INDEX));
        self.next_reliable += 1;
        bunch
    }

    fn send(&mut self, bunches: Vec<Bunch>) {
        send_at(
            &mut self.lab,
            &self.socket,
            &mut self.client,
            bunches,
            Instant::now(),
        );
    }

    fn mapped(&self, relation: PawnSubobjectRelation, ordinal: u8) -> u32 {
        self.lab.peers[&self.address]
            .pawn_subobject_mapping
            .guid_of(relation, ordinal)
            .unwrap()
    }

    fn lifetime(&self) -> PawnFieldDiagnostics {
        self.lab.pawn_receive_diagnostics().fields
    }

    fn live(&self) -> PawnFieldDiagnostics {
        self.lab
            .pawn_field_diagnostics(self.address)
            .unwrap()
            .clone()
    }
}

fn bits(count: usize) -> Payload {
    let mut writer = BitWriter::new();
    for index in 0..count {
        writer.write_bit(index % 3 == 0);
    }
    Payload::from_writer(writer)
}

fn block(target: BlockTarget, bound: u32, fields: &[(u32, usize)]) -> ContentBlock {
    let mut writer = BitWriter::new();
    for (index, width) in fields {
        write_field_header(
            &mut writer,
            &FieldHeader {
                index: *index,
                payload: bits(*width),
            },
            bound,
        )
        .unwrap();
    }
    ContentBlock {
        has_rep_layout: false,
        target,
        payload: Payload::from_writer(writer),
    }
}

fn subobject(guid: u32) -> BlockTarget {
    BlockTarget::SubObject {
        object: NetGuidRef::plain(guid),
        class: SubObjectClass::ClientSent,
    }
}

fn actor_row(index: u32, payload_bits: u64, reliable: bool, count: u64) -> PawnFieldSummary {
    PawnFieldSummary {
        target: PawnFieldTarget::Actor,
        relation: None,
        ordinal: None,
        index,
        payload_bits,
        reliable,
        disposition: PawnFieldDisposition::Ignored,
        count,
    }
}

fn no_unframed(fields: &PawnFieldDiagnostics) -> bool {
    fields.partial_bunches == 0
        && fields.export_bunches == 0
        && fields.must_be_mapped_bunches == 0
        && fields.body_parse_failures == 0
        && fields.malformed_blocks == 0
        && fields.malformed_bunches == 0
        && fields.property_blocks == 0
        && fields.unbounded_subobject_blocks == 0
        && fields.unknown_subobject_blocks == 0
        && fields.transport_drops == PawnTransportDrops::default()
}

#[test]
fn reliable_actor_fields_are_ignored_rows_by_index_bits_and_count() {
    let mut session = Session::new(true);
    let payload = session.body(vec![block(
        BlockTarget::Actor,
        PAWN_FIELD_BOUND,
        &[(12, 8), (77, 0)],
    )]);
    let first = session.reliable(payload.clone());
    session.send(vec![first]);
    let expected = vec![actor_row(12, 8, true, 1), actor_row(77, 0, true, 1)];
    assert_eq!(session.lifetime().entries, expected);
    let second = session.reliable(payload.clone());
    let third = session.reliable(payload);
    session.send(vec![second, third]);
    let fields = session.lifetime();
    assert_eq!(
        fields.entries,
        vec![actor_row(12, 8, true, 3), actor_row(77, 0, true, 3)]
    );
    assert!(fields.movement.is_empty() && fields.skipped_blocks.is_empty());
    assert_eq!(fields.overflow, 0);
    assert!(no_unframed(&fields));
    assert_eq!(session.live(), fields);
    assert_eq!(session.lab.pawn_field_totals(), fields);
    let receive = session.lab.pawn_receive_diagnostics();
    assert_eq!(receive.reliable_bunches, 3);
    assert_eq!(receive.unreliable_bunches, 0);
    // The bunches' existing handling is unchanged: still counted, never applied.
    assert_eq!(
        session.lab.peers[&session.address]
            .pawn_inbound
            .reliable_bunches,
        3
    );
    assert_eq!(session.lab.stats.pawn_moves_received, 0);
}

#[test]
fn unreliable_moves_occupy_one_recognized_movement_row_and_are_applied_as_before() {
    let mut session = Session::new(true);
    for step in 0..20u16 {
        let field = authored_move(
            1.0 + f32::from(step) * 0.05,
            [
                1000.0 + f32::from(step),
                -500.0,
                200.0 + f32::from(step % 7) * 113.0,
            ],
            Some(1),
        );
        let bunch = unreliable_bunch(session.channel, move_bunch_payload(field, session.engine));
        session.send(vec![bunch]);
    }
    let fields = session.lifetime();
    assert!(fields.entries.is_empty());
    let [row] = fields.movement.as_slice() else {
        panic!("one movement row: {:?}", fields.movement);
    };
    assert_eq!(row.index, SERVER_MOVE_PACKED_FIELD_INDEX);
    assert!(!row.reliable);
    assert_eq!(row.disposition, PawnFieldDisposition::Recognized);
    assert_eq!(row.count, 20);
    assert!(row.max_payload_bits > 0);
    assert!(no_unframed(&fields));
    let moves = &session.lab.peers[&session.address].pawn_moves.counts;
    assert_eq!(moves.moves, 20);
    assert_eq!(moves.malformed, 0);
    assert_eq!(session.lab.stats.pawn_moves_received, 20);

    // A payload the move reader refuses is Malformed in its own row and is
    // counted malformed by move handling exactly as before.
    let mut junk = BitWriter::new();
    junk.write_bits(0x2a5, 10);
    let bunch = unreliable_bunch(
        session.channel,
        move_bunch_payload(Payload::from_writer(junk), session.engine),
    );
    session.send(vec![bunch]);
    let fields = session.lifetime();
    assert_eq!(fields.movement.len(), 2);
    assert_eq!(
        fields.movement[1].disposition,
        PawnFieldDisposition::Malformed
    );
    assert_eq!(fields.movement[1].count, 1);
    assert_eq!(
        session.lab.peers[&session.address]
            .pawn_moves
            .counts
            .malformed,
        1
    );
    assert!(fields.entries.is_empty());
}

#[test]
fn unread_moves_are_one_ignored_movement_row_without_move_handling() {
    let mut session = Session::new(false);
    for step in 0..5u16 {
        let field = authored_move(2.0 + f32::from(step), [0.0, 0.0, 100.0], Some(1));
        let bunch = unreliable_bunch(session.channel, move_bunch_payload(field, session.engine));
        session.send(vec![bunch]);
    }
    let fields = session.lifetime();
    let [row] = fields.movement.as_slice() else {
        panic!("one movement row");
    };
    assert_eq!(row.disposition, PawnFieldDisposition::Ignored);
    assert_eq!(row.count, 5);
    assert_eq!(session.lab.stats.pawn_moves_received, 0);
    assert_eq!(session.lab.pawn_receive_diagnostics().unreliable_bunches, 5);
}

#[test]
fn subobject_blocks_report_bounded_relations_and_no_identifier() {
    let mut session = Session::new(true);
    let (stat_container, inventory_component, stat_bound, inventory_bound) = {
        let (plan, _, _) = session.lab.pawn_open.as_ref().unwrap();
        (
            plan.stat_container,
            plan.inventory_component,
            plan.content.stat_container_bound,
            plan.content.inventory_bound,
        )
    };
    let stat_ordinal = u8::try_from(stat_container).unwrap();
    let component_ordinal = u8::try_from(inventory_component).unwrap();
    let inventory = session.mapped(PawnSubobjectRelation::PawnInventory, 1);
    let stats = session.mapped(PawnSubobjectRelation::PawnComponent, stat_ordinal);
    let component = session.mapped(PawnSubobjectRelation::PawnComponent, component_ordinal);
    let unknown = 0x00ab_cdef_u32;
    assert!(![inventory, stats, component].contains(&unknown));
    let payload = session.body(vec![
        block(subobject(inventory), inventory_bound, &[(5, 24)]),
        block(subobject(stats), stat_bound, &[(3, 4)]),
        block(subobject(component), 16, &[(9, 6)]),
        block(subobject(unknown), 16, &[(1, 2)]),
    ]);
    let bunch = session.reliable(payload);
    session.send(vec![bunch]);
    let fields = session.lifetime();
    assert_eq!(
        fields.entries,
        vec![
            PawnFieldSummary {
                target: PawnFieldTarget::Subobject,
                relation: Some(PawnSubobjectRelation::PawnInventory),
                ordinal: Some(1),
                index: 5,
                payload_bits: 24,
                reliable: true,
                disposition: PawnFieldDisposition::Ignored,
                count: 1,
            },
            PawnFieldSummary {
                target: PawnFieldTarget::Subobject,
                relation: Some(PawnSubobjectRelation::PawnComponent),
                ordinal: Some(stat_ordinal),
                index: 3,
                payload_bits: 4,
                reliable: true,
                disposition: PawnFieldDisposition::Ignored,
                count: 1,
            },
        ]
    );
    // Neither a bound for the inventory component's class nor the unknown
    // object's class exists: both stay unframed, with their size only.
    assert_eq!(fields.unbounded_subobject_blocks, 1);
    assert_eq!(fields.unknown_subobject_blocks, 1);
    assert_eq!(fields.skipped_blocks.len(), 2);
    assert_eq!(
        fields.skipped_blocks[0].relation,
        Some(PawnSubobjectRelation::PawnComponent)
    );
    assert_eq!(fields.skipped_blocks[0].ordinal, Some(component_ordinal));
    assert_eq!(
        fields.skipped_blocks[0].reason,
        PawnSkipReason::NoFieldIndexBound
    );
    assert_eq!(
        fields.skipped_blocks[1].relation,
        Some(PawnSubobjectRelation::Unmapped)
    );
    assert_eq!(fields.skipped_blocks[1].ordinal, None);
    // No key can carry an identity: only sizes, indices, ordinals and counts.
    fn keys(value: &serde_json::Value, found: &mut Vec<String>) {
        match value {
            serde_json::Value::Object(map) => {
                for (key, item) in map {
                    found.push(key.clone());
                    keys(item, found);
                }
            }
            serde_json::Value::Array(items) => items.iter().for_each(|item| keys(item, found)),
            _ => {}
        }
    }
    let mut found = Vec::new();
    keys(
        &serde_json::to_value(session.lab.pawn_receive_diagnostics()).unwrap(),
        &mut found,
    );
    const ALLOWED: [&str; 35] = [
        "reliable_bunches",
        "unreliable_bunches",
        "fields",
        "entries",
        "overflow",
        "movement",
        "skipped_blocks",
        "skipped_blocks_overflow",
        "partial_bunches",
        "export_bunches",
        "must_be_mapped_bunches",
        "body_parse_failures",
        "malformed_blocks",
        "malformed_bunches",
        "property_blocks",
        "unbounded_subobject_blocks",
        "unknown_subobject_blocks",
        "transport_drops",
        "queue_refusal",
        "package_map_exports",
        "must_be_mapped_guids",
        "other_unsupported_header",
        "partial_policy",
        "partial_reassembly",
        "target",
        "relation",
        "ordinal",
        "index",
        "payload_bits",
        "max_payload_bits",
        "reliable",
        "disposition",
        "reason",
        "has_rep_layout",
        "count",
    ];
    for key in found {
        assert!(ALLOWED.contains(&key.as_str()), "{key}");
    }
}

#[test]
fn the_sixty_fifth_key_overflows_and_changes_no_row() {
    let mut session = Session::new(true);
    // Indices 40..=103: actor field 39 is the move field, kept apart.
    let first: Vec<(u32, usize)> = (40..=103).map(|index| (index, 0)).collect();
    let payload = session.body(vec![block(BlockTarget::Actor, PAWN_FIELD_BOUND, &first)]);
    let bunch = session.reliable(payload);
    session.send(vec![bunch]);
    let full = session.lifetime();
    assert_eq!(full.entries.len(), PAWN_FIELD_ENTRIES);
    assert_eq!(full.overflow, 0);
    let payload = session.body(vec![block(
        BlockTarget::Actor,
        PAWN_FIELD_BOUND,
        &[(104, 0), (104, 0)],
    )]);
    let bunch = session.reliable(payload);
    session.send(vec![bunch]);
    // Movement never competes for field rows.
    let field = authored_move(3.0, [0.0, 0.0, 100.0], Some(1));
    let bunch = unreliable_bunch(session.channel, move_bunch_payload(field, session.engine));
    session.send(vec![bunch]);
    let after = session.lifetime();
    assert_eq!(after.entries, full.entries);
    assert_eq!(after.overflow, 2);
    assert_eq!(after.movement.len(), 1);
    // A retained key still counts.
    let payload = session.body(vec![block(
        BlockTarget::Actor,
        PAWN_FIELD_BOUND,
        &[(40, 0)],
    )]);
    let bunch = session.reliable(payload);
    session.send(vec![bunch]);
    let after = session.lifetime();
    assert_eq!(after.entries[0].index, 40);
    assert_eq!(after.entries[0].count, 2);
    assert_eq!(after.overflow, 2);
}

#[test]
fn malformed_and_unframed_bunches_count_without_field_rows() {
    let mut session = Session::new(true);
    // A body that cannot be read at all.
    let mut writer = BitWriter::new();
    writer.write_bits(0b101, 3);
    let bunch = session.reliable(Payload::from_writer(writer));
    session.send(vec![bunch]);
    assert_eq!(session.lifetime().body_parse_failures, 1);
    // A field stream truncated inside its index.
    let mut writer = BitWriter::new();
    writer.write_bits(0b11, 2);
    let truncated = ContentBlock {
        has_rep_layout: false,
        target: BlockTarget::Actor,
        payload: Payload::from_writer(writer),
    };
    let payload = session.body(vec![truncated]);
    let bunch = session.reliable(payload);
    session.send(vec![bunch]);
    // A client property block.
    let mut property = block(BlockTarget::Actor, PAWN_FIELD_BOUND, &[(4, 4)]);
    property.has_rep_layout = true;
    let payload = session.body(vec![property]);
    let bunch = session.reliable(payload);
    session.send(vec![bunch]);
    // Unreliable forms the session cannot frame.
    let framed = session.body(vec![block(BlockTarget::Actor, PAWN_FIELD_BOUND, &[(2, 1)])]);
    let mut partial = unreliable_bunch(session.channel, framed.clone());
    partial.header.partial = true;
    partial.header.partial_initial = true;
    let mut exports = unreliable_bunch(session.channel, framed.clone());
    exports.header.has_package_map_exports = true;
    let mut mapped = unreliable_bunch(session.channel, framed.clone());
    mapped.header.has_must_be_mapped_guids = true;
    session.send(vec![partial, exports, mapped]);
    // A reliable bunch the transport drops before the session.
    let mut dropped = session.reliable(framed);
    dropped.header.has_package_map_exports = true;
    session.send(vec![dropped]);
    let fields = session.lifetime();
    assert!(fields.entries.is_empty() && fields.movement.is_empty());
    assert_eq!(fields.overflow, 0);
    assert_eq!(fields.body_parse_failures, 1);
    assert_eq!(fields.malformed_blocks, 1);
    assert_eq!(fields.malformed_bunches, 1);
    assert_eq!(fields.property_blocks, 1);
    assert_eq!(fields.partial_bunches, 1);
    assert_eq!(fields.export_bunches, 1);
    assert_eq!(fields.must_be_mapped_bunches, 1);
    assert_eq!(fields.transport_drops.package_map_exports, 1);
    let [skipped] = fields.skipped_blocks.as_slice() else {
        panic!("one skipped property block");
    };
    assert_eq!(skipped.target, PawnFieldTarget::Actor);
    assert_eq!(skipped.reason, PawnSkipReason::PropertyBlock);
    assert!(skipped.has_rep_layout);
    assert_eq!(session.live(), fields);
}

#[test]
fn malformed_block_discards_only_its_own_rows() {
    let mut session = Session::new(true);
    let mut writer = BitWriter::new();
    writer.write_bits(0b1, 1);
    let truncated = ContentBlock {
        has_rep_layout: false,
        target: BlockTarget::Actor,
        payload: Payload::from_writer(writer),
    };
    let inventory = session.mapped(PawnSubobjectRelation::PawnInventory, 0);
    let inventory_bound = session
        .lab
        .pawn_open
        .as_ref()
        .unwrap()
        .0
        .content
        .inventory_bound;
    let payload = session.body(vec![
        block(subobject(inventory), inventory_bound, &[(6, 3)]),
        truncated,
    ]);
    let bunch = session.reliable(payload);
    session.send(vec![bunch]);
    let fields = session.lifetime();
    assert_eq!(fields.entries.len(), 1);
    assert_eq!(fields.entries[0].index, 6);
    assert_eq!(fields.malformed_blocks, 1);
    assert_eq!(fields.malformed_bunches, 1);
}

#[test]
fn lifetime_totals_survive_peer_removal() {
    let mut session = Session::new(true);
    let payload = session.body(vec![block(
        BlockTarget::Actor,
        PAWN_FIELD_BOUND,
        &[(20, 5)],
    )]);
    let bunch = session.reliable(payload);
    session.send(vec![bunch]);
    let field = authored_move(4.0, [0.0, 0.0, 100.0], Some(1));
    let bunch = unreliable_bunch(session.channel, move_bunch_payload(field, session.engine));
    session.send(vec![bunch]);
    let before = session.lab.pawn_receive_diagnostics();
    assert_eq!(before.fields, session.lab.pawn_field_totals());
    assert_eq!(before.fields.entries.len(), 1);
    assert_eq!(before.fields.movement.len(), 1);
    session.lab.remove_peer(session.address).unwrap();
    assert!(
        session
            .lab
            .pawn_field_diagnostics(session.address)
            .is_none()
    );
    assert_eq!(
        session.lab.pawn_field_totals(),
        PawnFieldDiagnostics::default()
    );
    assert_eq!(session.lab.pawn_receive_diagnostics(), before);
    session.lab.shutdown();
    assert_eq!(session.lab.pawn_receive_diagnostics(), before);
}

/// Every leaf is a number, a boolean or one of the fixed enum strings; no
/// null, free text or identifier can appear.
fn assert_numeric_or_enum(value: &serde_json::Value, path: &str) {
    const ENUMS: [&str; 11] = [
        "Actor",
        "Subobject",
        "PawnComponent",
        "PawnInventory",
        "Unmapped",
        "MappingOverflow",
        "Recognized",
        "Ignored",
        "Malformed",
        "PropertyBlock",
        "NoFieldIndexBound",
    ];
    match value {
        serde_json::Value::Number(_) | serde_json::Value::Bool(_) => {}
        serde_json::Value::String(text) => {
            assert!(ENUMS.contains(&text.as_str()), "{path}: {text}");
        }
        serde_json::Value::Array(items) => {
            for (index, item) in items.iter().enumerate() {
                assert_numeric_or_enum(item, &format!("{path}[{index}]"));
            }
        }
        serde_json::Value::Object(map) => {
            for (key, item) in map {
                assert_numeric_or_enum(item, &format!("{path}.{key}"));
            }
        }
        serde_json::Value::Null => panic!("{path}: null"),
    }
}

#[test]
fn serialized_pawn_receive_holds_only_numbers_booleans_and_fixed_enums() {
    let mut session = Session::new(true);
    let inventory = session.mapped(PawnSubobjectRelation::PawnInventory, 0);
    let inventory_bound = session
        .lab
        .pawn_open
        .as_ref()
        .unwrap()
        .0
        .content
        .inventory_bound;
    let mut property = block(BlockTarget::Actor, PAWN_FIELD_BOUND, &[(4, 4)]);
    property.has_rep_layout = true;
    let payload = session.body(vec![
        block(BlockTarget::Actor, PAWN_FIELD_BOUND, &[(30, 7)]),
        block(subobject(inventory), inventory_bound, &[(2, 9)]),
        block(subobject(0x0012_3457), 16, &[(1, 1)]),
        property,
    ]);
    let bunch = session.reliable(payload);
    session.send(vec![bunch]);
    let field = authored_move(5.0, [0.0, 0.0, 100.0], Some(1));
    let bunch = unreliable_bunch(session.channel, move_bunch_payload(field, session.engine));
    session.send(vec![bunch]);
    let value = serde_json::to_value(session.lab.pawn_receive_diagnostics()).unwrap();
    let fields = &value["fields"];
    assert!(!fields["entries"].as_array().unwrap().is_empty());
    assert!(!fields["movement"].as_array().unwrap().is_empty());
    assert!(!fields["skipped_blocks"].as_array().unwrap().is_empty());
    assert_numeric_or_enum(&value, "pawn_receive");
    let mut keys: Vec<_> = value.as_object().unwrap().keys().cloned().collect();
    keys.sort();
    assert_eq!(
        keys,
        ["fields", "reliable_bunches", "unreliable_bunches"].map(String::from)
    );
}

/// Deterministic xorshift; no captured or random-seeded input.
struct Noise(u64);

impl Noise {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn payload(&mut self, max_bits: usize) -> Payload {
        let length = (self.next() as usize) % (max_bits + 1);
        let mut writer = BitWriter::new();
        for _ in 0..length {
            writer.write_bit(self.next() & 1 == 1);
        }
        Payload::from_writer(writer)
    }
}

#[test]
fn random_payload_bits_never_panic_and_stay_bounded() {
    let session = Session::new(true);
    let actor = actor_codec(session.engine, session.lab.config.packet.codec.order);
    let mapping = &session.lab.peers[&session.address].pawn_subobject_mapping;
    let unmapped = PawnSubobjectMapping::default();
    let inventory = session.mapped(PawnSubobjectRelation::PawnInventory, 0);
    let inventory_bound = session
        .lab
        .pawn_open
        .as_ref()
        .unwrap()
        .0
        .content
        .inventory_bound;
    let mut noise = Noise(0x9e37_79b9_7f4a_7c15);
    let mut fields = PawnFieldDiagnostics::default();
    let mut lifetime = PawnFieldDiagnostics::default();
    for round in 0..4000 {
        let structured = round % 2 == 0;
        let payload = if structured {
            // Valid framing around random field payloads and indices.
            let mut blocks = Vec::new();
            for _ in 0..(noise.next() % 4) {
                let target = match noise.next() % 3 {
                    0 => BlockTarget::Actor,
                    1 => subobject(inventory),
                    _ => subobject(noise.next() as u32),
                };
                let bound = match &target {
                    BlockTarget::Actor => PAWN_FIELD_BOUND,
                    _ => inventory_bound,
                };
                let mut writer = BitWriter::new();
                for _ in 0..(noise.next() % 5) {
                    write_field_header(
                        &mut writer,
                        &FieldHeader {
                            index: (noise.next() % u64::from(bound)) as u32,
                            payload: noise.payload(96),
                        },
                        bound,
                    )
                    .unwrap();
                }
                // Occasionally trail random bits after the fields.
                if noise.next().is_multiple_of(4) {
                    for _ in 0..(noise.next() % 12) {
                        writer.write_bit(noise.next() & 1 == 1);
                    }
                }
                blocks.push(ContentBlock {
                    has_rep_layout: noise.next().is_multiple_of(8),
                    target,
                    payload: Payload::from_writer(writer),
                });
            }
            session.body(blocks)
        } else {
            noise.payload(512)
        };
        let mut bunch = unreliable_bunch(session.channel, payload);
        bunch.header.reliable = noise.next().is_multiple_of(2);
        bunch.header.partial = noise.next().is_multiple_of(16);
        bunch.header.has_package_map_exports = noise.next().is_multiple_of(16);
        bunch.header.has_must_be_mapped_guids = noise.next().is_multiple_of(16);
        let moves = match noise.next() % 3 {
            0 => MoveOutcomes::NotRead,
            1 => MoveOutcomes::Refused,
            _ => MoveOutcomes::Read(
                (0..(noise.next() % 3))
                    .map(|_| noise.next().is_multiple_of(2))
                    .collect(),
            ),
        };
        let mapping = if noise.next().is_multiple_of(4) {
            &unmapped
        } else {
            mapping
        };
        observe_pawn_bunch(
            &bunch,
            &actor,
            mapping,
            &moves,
            &mut [&mut fields, &mut lifetime],
        );
        if noise.next().is_multiple_of(64) {
            observe_pawn_transport_drop(
                &super::super::super::packet_lab::ReliableActorDrop {
                    channel_index: session.channel,
                    reason: super::super::super::packet_lab::ReliableActorDropReason::PartialPolicy,
                },
                &mut [&mut fields, &mut lifetime],
            );
        }
        assert!(fields.entries.len() <= PAWN_FIELD_ENTRIES);
        assert!(fields.movement.len() <= 6);
        assert!(fields.skipped_blocks.len() <= PAWN_SKIPPED_BLOCK_ENTRIES);
    }
    assert_eq!(fields, lifetime);
    assert!(fields.body_parse_failures > 0);
    assert!(fields.malformed_blocks > 0 || !fields.entries.is_empty());
    let value = serde_json::to_value(&fields).unwrap();
    assert_numeric_or_enum(&value, "fields");

    // The same noise through the real receive path: the session survives
    // and move handling sees exactly what it saw before.
    let mut session = Session::new(true);
    let mut noise = Noise(0x2545_f491_4f6c_dd1d);
    for round in 0..300 {
        if round % 16 == 0 {
            session.lab.send_acks();
            deliver_bunches(
                &mut session.lab,
                &session.socket,
                &mut session.client,
                |_| true,
            );
        }
        let payload = noise.payload(400);
        let bunch = if noise.next().is_multiple_of(2) {
            session.reliable(payload)
        } else {
            unreliable_bunch(session.channel, payload)
        };
        session.send(vec![bunch]);
    }
    assert!(session.lab.peers.contains_key(&session.address));
    let fields = session.lifetime();
    assert!(fields.entries.len() <= PAWN_FIELD_ENTRIES);
    assert_eq!(session.live(), fields);
}

#[test]
fn saturating_counts_never_wrap() {
    let mut fields = PawnFieldDiagnostics {
        overflow: u64::MAX,
        body_parse_failures: u64::MAX,
        ..PawnFieldDiagnostics::default()
    };
    fields.entries.push(actor_row(3, 1, true, u64::MAX));
    let other = fields.clone();
    fields.merge(&other);
    assert_eq!(fields.overflow, u64::MAX);
    assert_eq!(fields.body_parse_failures, u64::MAX);
    assert_eq!(fields.entries[0].count, u64::MAX);
}

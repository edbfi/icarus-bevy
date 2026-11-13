//! Level visibility actors: authored field-82 reports and their effects.
//! Every level name here is an authored placeholder of the shape a client
//! reports; no captured payload, GUID or account value is used.
use super::super::super::{
    packet::{BitWriter, Bunch, BunchHeader, Name, Packet, write_packet},
    sequencing::{PacketSequencer, wire_reliable_sequence},
    session_open::ACTOR_CHANNEL_NAME_INDEX,
};
use super::*;

/// Authored placeholder levels, shaped like the streamed levels a client
/// reports (a `/Game/` package path whose last segment names the world).
pub(super) const LEVEL_A: &str = "/Game/Authored/Sublevels/Tile_x0_y0";
pub(super) const LEVEL_B: &str = "/Game/Authored/Sublevels/Tile_x1_y0";

/// One authored field-82 payload in the normal single-custom-struct layout
/// (F-1020, F-1021): the outer presence bit, reuse, visible, then the package
/// name as a narrow string with number 0; `trailing` appends one stray bit.
pub(super) fn rpc82(visible: bool, package: &str, trailing: bool) -> Payload {
    let mut writer = BitWriter::new();
    writer.write_bit(true);
    writer.write_bit(true);
    writer.write_bit(visible);
    writer.write_bit(false);
    let units = i32::try_from(package.len() + 1).unwrap();
    writer.write_bytes(&units.to_le_bytes());
    writer.write_bytes(package.as_bytes());
    writer.write_bytes(&[0]);
    writer.write_bytes(&0u32.to_le_bytes());
    if trailing {
        writer.write_bit(false);
    }
    Payload::from_writer(writer)
}

/// A field-82 payload naming a hardcoded name index instead of a string.
pub(super) fn rpc82_hardcoded(visible: bool) -> Payload {
    let mut writer = BitWriter::new();
    writer.write_bit(true);
    writer.write_bit(true);
    writer.write_bit(visible);
    writer.write_bit(true);
    writer.write_int_packed(0);
    Payload::from_writer(writer)
}

/// The client's next reliable sequence on its controller channel.
pub(super) struct ControllerSender {
    pub(super) next: u32,
}

impl ControllerSender {
    pub(super) fn new(client: &PacketSequencer) -> Self {
        Self {
            next: client.initial_reliable().1 + 1,
        }
    }

    /// Send one reliable controller bunch carrying `fields` on the actor
    /// block, through the lab's real receive path.
    pub(super) fn send(
        &mut self,
        lab: &mut PeerLab,
        socket: &UdpSocket,
        client: &mut PacketSequencer,
        fields: &[(u32, Payload)],
    ) {
        let block = field_block(
            BlockTarget::Actor,
            BP_ICARUS_PLAYER_CONTROLLER_SURVIVAL_FIELD_INDEX_BOUND,
            fields,
        );
        self.send_blocks(lab, socket, client, vec![block]);
    }

    /// Send one reliable controller bunch carrying `blocks`.
    pub(super) fn send_blocks(
        &mut self,
        lab: &mut PeerLab,
        socket: &UdpSocket,
        client: &mut PacketSequencer,
        blocks: Vec<ContentBlock>,
    ) {
        let SocketAddr::V4(address) = socket.local_addr().unwrap() else {
            unreachable!("IPv4 fixture")
        };
        let plan = lab.player_controller_open.as_ref().unwrap();
        let payload = encode_actor_bunch_body(
            &ActorBunchBody {
                spawn: None,
                blocks,
            },
            &actor_codec(plan.engine_net_version, lab.config.packet.codec.order),
        )
        .unwrap();
        let bytes = write_packet(
            &Packet {
                magic: 0,
                header: client.header().unwrap(),
                info: None,
                bunches: vec![Bunch {
                    header: BunchHeader {
                        control: false,
                        open: false,
                        close: false,
                        close_reason: None,
                        paused: false,
                        reliable: true,
                        channel_index: lab.peers[&address].player_controller_channel.unwrap(),
                        has_package_map_exports: false,
                        has_must_be_mapped_guids: false,
                        partial: false,
                        reliable_sequence_raw: Some(wire_reliable_sequence(self.next)),
                        partial_initial: false,
                        partial_final: false,
                        channel_name: Some(Name::Hardcoded(ACTOR_CHANNEL_NAME_INDEX)),
                    },
                    payload,
                }],
            },
            &lab.config.packet.packet,
        )
        .unwrap();
        self.next += 1;
        client.commit().unwrap();
        receive_authored_at(lab, socket, &bytes, Instant::now());
    }
}

/// A content block of length-framed fields under `bound`.
pub(super) fn field_block(
    target: BlockTarget,
    bound: u32,
    fields: &[(u32, Payload)],
) -> ContentBlock {
    let mut writer = BitWriter::new();
    for (index, payload) in fields {
        write_field_header(
            &mut writer,
            &FieldHeader {
                index: *index,
                payload: payload.clone(),
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

/// The authored report sequence of the switch-off pin: an add, a repeated
/// add with a second level, a hide, a malformed add (a stray trailing bit)
/// and an add naming a hardcoded name index, one controller bunch each.
pub(super) fn report_sequence() -> Vec<Vec<(u32, Payload)>> {
    vec![
        vec![(82, rpc82(true, LEVEL_A, false))],
        vec![
            (82, rpc82(true, LEVEL_A, false)),
            (82, rpc82(true, LEVEL_B, false)),
        ],
        vec![(82, rpc82(false, LEVEL_A, false))],
        vec![(82, rpc82(true, LEVEL_B, true))],
        vec![(82, rpc82_hardcoded(true))],
    ]
}

/// Two sequential sessions on `lab`, each from the stateless handshake and
/// Join through the connected-player record's ACK, then the authored report
/// sequence, every server bunch delivered and ACKed after each step: every
/// bunch the client receives, digested per session with the controller
/// field accounting. Whole bunches off the GameState channel count by their
/// exact bytes; GameState-channel bunches, which carry the lab clock, by
/// header flags, length and block targets. Uses only behaviour that
/// predates the level visibility actors switch.
pub(super) fn level_report_session_digests(lab: &mut PeerLab) -> Vec<String> {
    use super::super::super::control_messages::{NetIdType, NetIdValue, UniqueNetId};
    let channel = game_state_channel(lab);
    let codec = actor_codec(17, super::super::super::archive::ByteOrder::Little);
    let mut digests = Vec::new();
    for (connection, user) in [(1u32, "AuthoredLevelFirst"), (2, "AuthoredLevelSecond")] {
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
            name_units: "AUTHORED-LEVEL".encode_utf16().collect(),
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
        let ignored = lab.stats.controller_fields_ignored;
        let malformed = lab.stats.actor_bunches_malformed;
        let mut sender = ControllerSender::new(&client);
        for fields in report_sequence() {
            sender.send(lab, &socket, &mut client, &fields);
            bunches.extend(deliver_bunches(lab, &socket, &mut client, |_| true));
        }
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
        let diagnostics = lab.controller_field_diagnostics(address).unwrap();
        hasher.update(
            format!(
                "ignored {} malformed {} fields {}\n",
                lab.stats.controller_fields_ignored - ignored,
                lab.stats.actor_bunches_malformed - malformed,
                serde_json::to_string(&diagnostics.entries).unwrap()
            )
            .as_bytes(),
        );
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

fn release_toml_config(level_visibility_actors: bool) -> crate::config::NetworkConfig {
    crate::config::NetworkConfig {
        enabled: true,
        port: 0,
        release_profile: Some(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("config/release.toml"),
        ),
        level_visibility_actors,
        ..crate::config::NetworkConfig::default()
    }
}

/// The release profile `config/release.toml` pins.
fn release_toml_profile() -> ReleaseProfile {
    ReleaseProfile::load(
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("config/release.toml"),
    )
    .unwrap()
}

/// [`possession_lab`] (records and possession on, as production defaults)
/// with level visibility actors on.
pub(super) fn level_lab() -> PeerLab {
    let mut lab = possession_lab(true);
    lab.enable_level_visibility_actors(&ReleaseProfile::release_3_0_35())
        .unwrap();
    lab
}

/// The switch-off path pinned to the parent of the level visibility change.
/// These digests were produced by [`level_report_session_digests`] on a
/// build of 73f0ad4's `src/` with only this test module added (before the
/// switch existed, by the same helper and fixtures), not by this build: a
/// lab that never touches the switch, one that turns it on and then off,
/// and the production bind with `level_visibility_actors = false` (the
/// default, and the only value production accepts) must send exactly those
/// bunches and keep field 82's accounting (ignored); with the switch on
/// (through the lab API, as the bounded probe enables it) every session
/// differs.
#[test]
fn switch_off_level_reports_send_the_parent_commits_bunches() {
    const LAB: [&str; 2] = [
        "a0d6f8565ebf7e18502346b37598dda54596e93504a1105ef217302ca6b6e453",
        "99b5ae9af69005447db67547a75c3cac72c04d0a322f227dc432b92abdf1e01d",
    ];
    const PRODUCTION: [&str; 2] = [
        "6f7372c27b879f8f4af6cf0b18d4938c15ed05ff400a756ed85ae42d6f822052",
        "1c92ffea56387cbe16024f6cda73a5296ed0740fe238d39c224314503205e6f7",
    ];
    let mut untouched = possession_lab(true);
    assert!(!untouched.has_level_visibility_actors());
    assert_eq!(level_report_session_digests(&mut untouched), LAB);
    let mut toggled = level_lab();
    toggled.disable_level_visibility_actors().unwrap();
    assert!(!toggled.has_level_visibility_actors());
    assert_eq!(level_report_session_digests(&mut toggled), LAB);
    assert!(!crate::config::NetworkConfig::default().level_visibility_actors);
    let mut production = production_digest_lab(release_toml_config(false));
    assert!(!production.has_level_visibility_actors());
    assert_eq!(level_report_session_digests(&mut production), PRODUCTION);
    // Off, every report stays an ignored field and no counter moves.
    assert_eq!(production.stats.level_visibility_reports_added, 0);
    assert_eq!(production.stats.level_actor_opens_sent, 0);
    let on = level_report_session_digests(&mut level_lab());
    assert!(on.iter().zip(LAB).all(|(on, off)| on != off));
    // The probe's path: the production bind, then the lab API.
    let mut production_on = production_digest_lab(release_toml_config(false));
    production_on
        .enable_level_visibility_actors(&ReleaseProfile::release_3_0_35())
        .unwrap();
    assert!(production_on.has_level_visibility_actors());
    let on = level_report_session_digests(&mut production_on);
    assert!(on.iter().zip(PRODUCTION).all(|(on, off)| on != off));
}

/// A field-82 payload with an explicit name number and string encoding:
/// `wide` writes UTF-16 units with a negative count.
fn rpc82_named(visible: bool, package: &str, number: u32, wide: bool) -> Payload {
    let mut writer = BitWriter::new();
    writer.write_bit(true);
    writer.write_bit(true);
    writer.write_bit(visible);
    writer.write_bit(false);
    if wide {
        let units: Vec<u16> = package.encode_utf16().collect();
        let count = -i32::try_from(units.len() + 1).unwrap();
        writer.write_bytes(&count.to_le_bytes());
        for unit in units.iter().chain([&0]) {
            writer.write_bytes(&unit.to_le_bytes());
        }
    } else {
        let count = i32::try_from(package.len() + 1).unwrap();
        writer.write_bytes(&count.to_le_bytes());
        writer.write_bytes(package.as_bytes());
        writer.write_bytes(&[0]);
    }
    writer.write_bytes(&number.to_le_bytes());
    Payload::from_writer(writer)
}

/// A field-82 payload the decoder refuses: a name whose string is cut off.
fn rpc82_truncated() -> Payload {
    let mut writer = BitWriter::new();
    for bit in [true, true, true, false] {
        writer.write_bit(bit);
    }
    writer.write_bytes(&40i32.to_le_bytes());
    Payload::from_writer(writer)
}

/// An authored package path of `units` units whose world (last segment) has
/// `world` units: the shapes of the observed streamed levels without their
/// names.
fn shaped_package(units: usize, world: usize) -> String {
    let leaf = format!("W{}", "x".repeat(world - 1));
    let package = format!("/Game/{}/{leaf}", "a".repeat(units - 7 - world));
    assert_eq!(package.len(), units);
    package
}

/// Authored tile `index`, distinct from [`LEVEL_A`] and [`LEVEL_B`].
fn tile(index: usize) -> String {
    format!("/Game/Authored/Sublevels/Tile_n{index}")
}

/// One level actor open as the client receives it: its channel, the export
/// part's references and length, and the decoded open body.
#[derive(Debug)]
struct ReceivedLevelOpen {
    channel: u32,
    exports: Vec<super::super::super::actor_channel::NetGuidRef>,
    export_bits: usize,
    body: ActorBunchBody,
    body_bits: usize,
    body_bytes: Vec<u8>,
}

fn is_level_open_part(lab: &PeerLab, bunch: &Bunch) -> bool {
    bunch.header.open
        && !lab
            .reserved_session_channels()
            .contains(&bunch.header.channel_index)
}

/// The level actor opens among `bunches`: open bunches on channels outside
/// the lab's session channels, an export part then one content part each
/// (only the first part carries the open flag). Replication opens on those
/// channels are told apart by their dynamic spawn and skipped.
fn level_opens(lab: &PeerLab, bunches: &[Bunch]) -> Vec<ReceivedLevelOpen> {
    use super::super::super::actor_channel::{read_actor_bunch_body, read_export_prefix};
    let reserved = lab.reserved_session_channels();
    let codec = actor_codec(17, super::super::super::archive::ByteOrder::Little);
    let mut opens: Vec<ReceivedLevelOpen> = Vec::new();
    for bunch in bunches {
        let header = &bunch.header;
        let continues = !header.open
            && opens
                .iter()
                .any(|open| open.channel == header.channel_index && open.body.spawn.is_none());
        if reserved.contains(&header.channel_index) || !(header.open || continues) {
            continue;
        }
        if header.has_package_map_exports {
            assert!(header.open && header.reliable && header.partial);
            let mut reader = bunch.payload.reader();
            let exports = read_export_prefix(&mut reader, &codec).unwrap();
            assert!(reader.at_end());
            opens.push(ReceivedLevelOpen {
                channel: header.channel_index,
                exports,
                export_bits: bunch.payload.bit_len(),
                body: ActorBunchBody {
                    spawn: None,
                    blocks: vec![],
                },
                body_bits: 0,
                body_bytes: vec![],
            });
        } else if let Some(open) = opens
            .iter_mut()
            .rev()
            .find(|open| open.channel == header.channel_index && open.body.spawn.is_none())
        {
            let mut reader = bunch.payload.reader();
            open.body = read_actor_bunch_body(&mut reader, true, &codec).unwrap();
            open.body_bits = bunch.payload.bit_len();
            open.body_bytes = bunch.payload.bytes().to_vec();
        }
    }
    opens.retain(|open| {
        open.body
            .spawn
            .as_ref()
            .is_some_and(|spawn| spawn.dynamic.is_none())
    });
    opens
}

/// The leaf-first path of an exported chain and its checksums, and the GUID
/// where it stops: null after the package, or an outer the connection has
/// already ACKed, referenced without its path (F-796).
fn chain_of(
    reference: &super::super::super::actor_channel::NetGuidRef,
) -> (Vec<(String, Option<u32>, bool)>, u32) {
    let mut chain = Vec::new();
    let mut current = reference;
    while let Some(export) = &current.export {
        let Some(path) = &export.path else {
            break;
        };
        chain.push((
            String::from_utf16(path.path.units()).unwrap(),
            export.checksum,
            export.no_load,
        ));
        current = &path.outer;
    }
    (chain, current.guid)
}

/// Assert `open` names actor `index` of `package` (in that spelling) the way
/// the original's opens do: a static GUID exported with the 4-segment chain
/// (flags 7, the class checksum on the actor only), a static spawn and one
/// actor block with handles 5 `RemoteRole` and 14 `Role` only, in 31 bits.
fn assert_level_open(open: &ReceivedLevelOpen, index: usize, package: &str) -> u32 {
    use super::super::super::rep_layout::{
        MovementLevel, RotationLevel, parse_rep_properties, world_settings_rep_layout,
    };
    let actors = super::super::super::release::ReleaseLevelActorsProfile::icarus_world_settings();
    let world = package.rsplit('/').next().unwrap();
    let [reference] = open.exports.as_slice() else {
        panic!("one exported reference");
    };
    let expected = [
        (actors.names[index].clone(), Some(actors.checksum), true),
        ("PersistentLevel".to_owned(), Some(0), true),
        (world.to_owned(), Some(0), true),
        (package.to_owned(), Some(0), true),
    ];
    let (chain, end) = chain_of(reference);
    assert!(!chain.is_empty());
    assert_eq!(chain, expected[..chain.len()]);
    // The whole chain ends at the null GUID; a shorter one at an ACKed outer.
    assert_eq!(end == 0, chain.len() == expected.len(), "{chain:?} {end}");
    let spawn = open.body.spawn.as_ref().unwrap();
    assert_eq!(spawn.actor.guid, reference.guid);
    assert_eq!(spawn.actor.guid % 2, 1, "a static GUID");
    assert!(spawn.actor.export.is_none() && spawn.dynamic.is_none());
    let [block] = open.body.blocks.as_slice() else {
        panic!("one actor block");
    };
    assert!(block.has_rep_layout);
    assert_eq!(block.target, BlockTarget::Actor);
    assert_eq!(block.payload.bit_len(), 31);
    let codec = RepLayoutCodec {
        actor: actor_codec(17, super::super::super::archive::ByteOrder::Little),
        checksums: false,
        limits: Default::default(),
    };
    let layout = world_settings_rep_layout(
        MovementLevel::Whole,
        MovementLevel::Whole,
        RotationLevel::Bytes,
    );
    assert_eq!(layout.len(), 20);
    let stream = parse_rep_properties(&block.payload, &layout, &codec).unwrap();
    let handles: Vec<u32> = stream.entries.iter().map(|entry| entry.handle).collect();
    assert_eq!(handles, [5, 14]);
    reference.guid
}

/// The composed static open, through the repository decoder: one export of
/// the 4-segment chain, a static spawn and the two role properties. With the
/// actor GUIDs in the two-byte packed range, export parts are 1,409/1,433
/// bits and bodies 57 bits for levels shaped like the report's Generated
/// tiles (package 67 units, world 20), 1,249/1,273 for its HeightMap tiles
/// (52, 15): the observed structure. Both re-encode exactly.
#[test]
fn composed_static_open_decodes_reencodes_and_matches_the_observed_lengths() {
    use super::super::super::{
        actor_channel::{encode_actor_bunch_body, read_actor_bunch_body, read_export_prefix},
        actor_open::{StaticActorOpen, compose_static_actor_open},
        archive::ByteOrder,
        release::ReleaseLevelActorsProfile,
        session_open::{ConnectionExports, NetGuidCache, encode_exports},
    };
    let actors = ReleaseLevelActorsProfile::icarus_world_settings();
    assert_eq!(actors.names[0].len(), 21);
    assert_eq!(actors.names[1].len(), 24);
    let codec = actor_codec(17, ByteOrder::Little);
    for (units, world, expected) in [(67, 20, [1_409, 1_433]), (52, 15, [1_249, 1_273])] {
        let package = shaped_package(units, world);
        let mut cache = NetGuidCache::new();
        // Push static GUIDs past 127, into the two-byte packed range.
        for filler in 0..64 {
            cache.assign_static(&[PathSegment::new(
                &format!("/Game/Authored/Filler{filler}"),
                0,
                false,
            )]);
        }
        let mut exports = ConnectionExports::new();
        for (index, bits) in expected.into_iter().enumerate() {
            let open = StaticActorOpen {
                chain: actors.actor_chain(index, &package).unwrap(),
                blocks: vec![actor_role_block(17).unwrap()],
                engine_net_version: 17,
            };
            let (composed, guid) =
                compose_static_actor_open(&mut cache, &mut exports, &open, 9, ByteOrder::Little)
                    .unwrap();
            assert!(guid > 127 && guid < 16_384);
            // Unacked, each open re-exports the whole chain, as S2's do.
            let export_payload = composed.exports.unwrap();
            assert_eq!(export_payload.bit_len(), bits, "{units}/{world}/{index}");
            let mut reader = export_payload.reader();
            let references = read_export_prefix(&mut reader, &codec).unwrap();
            assert!(reader.at_end());
            assert_eq!(
                encode_exports(&references, &codec).unwrap().bytes(),
                export_payload.bytes()
            );
            let received = ReceivedLevelOpen {
                channel: 9,
                exports: references,
                export_bits: bits,
                body: read_actor_bunch_body(&mut composed.bunch.payload.reader(), true, &codec)
                    .unwrap(),
                body_bits: composed.bunch.payload.bit_len(),
                body_bytes: composed.bunch.payload.bytes().to_vec(),
            };
            assert_eq!(assert_level_open(&received, index, &package), guid);
            assert_eq!(received.body_bits, 57);
            let reencoded = encode_actor_bunch_body(&received.body, &codec).unwrap();
            assert_eq!(reencoded.bytes(), received.body_bytes.as_slice());
            assert_eq!(reencoded.bit_len(), received.body_bits);
            let header = &composed.bunch.header;
            assert!(header.open && header.reliable && !header.partial);
            assert_eq!(header.channel_index, 9);
            assert_eq!(
                header.channel_name,
                Some(Name::Hardcoded(ACTOR_CHANNEL_NAME_INDEX))
            );
        }
    }
}

/// A connection fixture of [`level_lab`] whose initial opens are all
/// committed and ACKed, with a controller sender.
fn level_peer(
    lab: &mut PeerLab,
    connection: u32,
    user: &str,
) -> (UdpSocket, SocketAddrV4, PacketSequencer, ControllerSender) {
    let (socket, address, client) = record_peer(lab, connection, user);
    let sender = ControllerSender::new(&client);
    (socket, address, client, sender)
}

/// Run level admission passes (the poll's last pass) with every bunch
/// delivered and ACKed in between, until nothing is pending; returns the
/// bunches.
fn admit(lab: &mut PeerLab, socket: &UdpSocket, client: &mut PacketSequencer) -> Vec<Bunch> {
    let mut bunches = deliver_bunches(lab, socket, client, |_| true);
    // At least one pass: a released channel is checked there.
    for _ in 0..256 {
        lab.retry_level_actor_opens();
        bunches.extend(deliver_bunches(lab, socket, client, |_| true));
        if level_slots(lab).pending == 0 {
            break;
        }
    }
    bunches
}

/// Report, then admit and deliver, returning the bunches the report produced.
fn report(
    lab: &mut PeerLab,
    socket: &UdpSocket,
    client: &mut PacketSequencer,
    sender: &mut ControllerSender,
    fields: &[(u32, Payload)],
) -> Vec<Bunch> {
    sender.send(lab, socket, client, fields);
    admit(lab, socket, client)
}

/// Slot gauges of the readiness snapshot.
#[derive(Debug, Default, PartialEq, Eq)]
struct Slots {
    visible: usize,
    with_actors: usize,
    pending: usize,
    awaiting: usize,
    settled: usize,
    refused: usize,
}

fn level_slots(lab: &PeerLab) -> Slots {
    let levels = lab.readiness_counts().level_visibility;
    Slots {
        visible: levels.levels_visible,
        with_actors: levels.levels_with_actors,
        pending: levels.opens_pending,
        awaiting: levels.opens_awaiting_ack,
        settled: levels.opens_settled,
        refused: levels.opens_refused_waiting,
    }
}

/// The first channel above the session channels.
fn first_level_channel(lab: &PeerLab) -> u32 {
    lab.reserved_session_channels().into_iter().max().unwrap() + 1
}

/// An authored replicated actor spec on the lab's map level.
fn replication_spec(lab: &PeerLab, package: &str) -> ActorOpenSpec {
    ActorOpenSpec {
        actor: 0,
        archetype: vec![
            PathSegment::new("Default__AuthoredReplicated_C", 7, false),
            PathSegment::new(package, 0, false),
        ],
        level: lab
            .player_controller_open
            .as_ref()
            .unwrap()
            .profile
            .level
            .clone(),
        location: None,
        rotation: None,
        velocity: None,
        blocks: vec![actor_role_block(17).unwrap()],
    }
}

/// An add opens the level's two actors once, each on a fresh channel above
/// the session channels, ACKed once each; the report itself sends nothing
/// (the poll's last pass does); a repeat, a hide and a re-add of the same
/// level open nothing more and close nothing; a second level gets its own
/// pair. The connection stays up throughout.
#[test]
fn a_reported_level_opens_its_actors_once_and_repeats_or_hides_open_nothing() {
    let mut lab = level_lab();
    let (socket, address, mut client, mut sender) = level_peer(&mut lab, 1, "AuthoredLevels");
    let first = first_level_channel(&lab);
    let statics = lab.guid_cache.static_count();
    sender.send(
        &mut lab,
        &socket,
        &mut client,
        &[(82, rpc82(true, LEVEL_A, false))],
    );
    assert_eq!(lab.stats.level_actor_opens_sent, 0, "queued, not sent");
    let bunches = admit(&mut lab, &socket, &mut client);
    let opens = level_opens(&lab, &bunches);
    assert_eq!(opens.len(), 2);
    assert_eq!(opens[0].channel, first);
    assert_eq!(opens[1].channel, first + 1);
    let guids = [
        assert_level_open(&opens[0], 0, LEVEL_A),
        assert_level_open(&opens[1], 1, LEVEL_A),
    ];
    assert_ne!(guids[0], guids[1]);
    // Two actor chains plus the level's three.
    assert_eq!(lab.guid_cache.static_count(), statics + 5);
    assert_eq!(lab.stats.level_visibility_reports_added, 1);
    assert_eq!(lab.stats.level_actor_opens_sent, 2);
    assert_eq!(lab.stats.level_actor_opens_acked, 2);
    assert_eq!(
        level_slots(&lab),
        Slots {
            visible: 1,
            with_actors: 1,
            settled: 2,
            ..Default::default()
        }
    );
    // A field-82 report is recognized, not ignored, with the switch on.
    let ignored = lab.stats.controller_fields_ignored;
    let entry = lab
        .controller_field_diagnostics(address)
        .unwrap()
        .entries
        .iter()
        .find(|entry| entry.index == 82)
        .cloned()
        .unwrap();
    assert_eq!(entry.disposition, ControllerFieldDisposition::Recognized);
    assert_eq!(entry.payload_bits, rpc82(true, LEVEL_A, false).bit_len());
    for fields in [
        vec![(82, rpc82(true, LEVEL_A, false))],
        vec![(82, rpc82(false, LEVEL_A, false))],
        vec![(82, rpc82(true, LEVEL_A, false))],
    ] {
        let bunches = report(&mut lab, &socket, &mut client, &mut sender, &fields);
        assert!(level_opens(&lab, &bunches).is_empty());
        assert!(
            bunches.iter().all(|bunch| !bunch.header.close),
            "nothing is closed"
        );
    }
    assert_eq!(lab.stats.level_visibility_reports_duplicate, 1);
    assert_eq!(lab.stats.level_visibility_reports_hidden, 1);
    assert_eq!(lab.stats.level_hides_actors_kept, 1);
    assert_eq!(lab.stats.level_visibility_reports_added, 2);
    assert_eq!(lab.stats.level_actor_opens_sent, 2);
    let bunches = report(
        &mut lab,
        &socket,
        &mut client,
        &mut sender,
        &[(82, rpc82(true, LEVEL_B, false))],
    );
    let opens = level_opens(&lab, &bunches);
    assert_eq!(
        opens.iter().map(|open| open.channel).collect::<Vec<_>>(),
        [first + 2, first + 3]
    );
    assert_level_open(&opens[0], 0, LEVEL_B);
    assert_level_open(&opens[1], 1, LEVEL_B);
    assert_eq!(lab.stats.level_actor_opens_sent, 4);
    assert_eq!(lab.stats.level_actor_opens_acked, 4);
    assert_eq!(lab.stats.controller_fields_ignored, ignored);
    assert_eq!(lab.stats.level_actor_open_errors, 0);
    let levels = lab.readiness_counts().level_visibility;
    assert_eq!((levels.levels_visible, levels.opens_settled), (2, 4));
    assert_eq!(levels.channels_held, 4);
    assert_eq!(
        levels.channel_budget_remaining,
        MAX_LEVEL_ACTOR_CHANNELS - 4
    );
    let counts = lab.session_counts();
    assert_eq!(counts.level_visibility_reports_added, 3);
    assert_eq!(counts.level_actor_opens_sent, 4);
    assert_eq!(counts.level_actor_opens_acked, 4);
    assert!(lab.has_peer(address));
    // Counts only: no level name reaches the exported counters.
    let text = serde_json::to_string(&(lab.stats(), counts, lab.readiness_counts())).unwrap();
    assert!(!text.contains("Authored") && !text.contains("Tile_"));
}

/// Inadmissible names (in any letter case for the reserved packages) and
/// absent parameters are counted and never close; a refused field 82 is
/// skipped alone: a valid report beside it still applies, and no bunch is
/// counted malformed.
#[test]
fn refused_absent_and_malformed_reports_never_close_or_drop_their_bunch() {
    let mut lab = level_lab();
    let (socket, address, mut client, mut sender) = level_peer(&mut lab, 1, "AuthoredRefused");
    // The length bound, which no single datagram of this lab can carry.
    let config = lab.level_visibility.as_ref().unwrap();
    let name = |units: usize| NetworkName::String {
        text: ArchiveText::text(&format!("/Game/{}", "a".repeat(units - 6))).unwrap(),
        number: 0,
    };
    assert!(config.admitted(&name(MAX_LEVEL_NAME_UNITS)).is_some());
    assert!(config.admitted(&name(MAX_LEVEL_NAME_UNITS + 1)).is_none());
    let release = ReleaseProfile::release_3_0_35();
    let archetype_package = release
        .player_controller
        .as_ref()
        .unwrap()
        .archetype
        .last()
        .unwrap()
        .path
        .clone();
    let manager_package = release.managers.as_ref().unwrap().flod[1].path.clone();
    let persistent = PeerLabAdmission::default().map_url.to_owned();
    let refused = [
        rpc82_hardcoded(true),
        rpc82_named(true, LEVEL_A, 1, false),
        rpc82_named(true, "/Game/Authored/Sublevels/Tile_é", 0, true),
        rpc82(true, "/Other/Authored/Tile_x0_y0", false),
        rpc82(true, "/Game/Authored/Tile?x0", false),
        rpc82(true, "/Game/Authored//Tile_x0_y0", false),
        rpc82(true, "/Game/Authored/Tile_x0_y0/", false),
        rpc82(true, &persistent, false),
        rpc82(true, &persistent.to_ascii_uppercase(), false),
        rpc82(true, &archetype_package, false),
        rpc82(true, &archetype_package.to_ascii_lowercase(), false),
        rpc82(true, &manager_package.to_ascii_uppercase(), false),
    ];
    let count = refused.len() as u64;
    let statics = lab.guid_cache.static_count();
    for payload in refused {
        let bunches = report(
            &mut lab,
            &socket,
            &mut client,
            &mut sender,
            &[(82, payload)],
        );
        assert!(level_opens(&lab, &bunches).is_empty());
    }
    assert_eq!(lab.stats.level_visibility_reports_refused_name, count);
    assert_eq!(lab.guid_cache.static_count(), statics);
    let mut absent = BitWriter::new();
    absent.write_bit(false);
    report(
        &mut lab,
        &socket,
        &mut client,
        &mut sender,
        &[(82, Payload::from_writer(absent))],
    );
    assert_eq!(lab.stats.level_visibility_reports_absent, 1);
    let malformed = lab.stats.actor_bunches_malformed;
    let ignored = lab.stats.controller_fields_ignored;
    for fields in [
        vec![(82, rpc82(true, LEVEL_A, true))],
        vec![(82, rpc82_truncated())],
    ] {
        let bunches = report(&mut lab, &socket, &mut client, &mut sender, &fields);
        assert!(level_opens(&lab, &bunches).is_empty());
    }
    // A valid report beside a refused one applies.
    let bunches = report(
        &mut lab,
        &socket,
        &mut client,
        &mut sender,
        &[
            (82, rpc82(true, LEVEL_A, false)),
            (82, rpc82(true, LEVEL_B, true)),
        ],
    );
    assert_eq!(level_opens(&lab, &bunches).len(), 2);
    assert_eq!(lab.stats.level_visibility_reports_malformed, 3);
    assert_eq!(lab.stats.actor_bunches_malformed, malformed);
    assert_eq!(lab.stats.controller_fields_ignored, ignored);
    assert_eq!(lab.stats.level_visibility_reports_added, 1);
    let diagnostics = lab.controller_field_diagnostics(address).unwrap();
    assert!(diagnostics.entries.iter().any(
        |entry| entry.index == 82 && entry.disposition == ControllerFieldDisposition::Malformed
    ));
    assert_eq!(diagnostics.malformed_bunches, 0);
    assert!(lab.has_peer(address));
    assert_eq!(lab.stats.peers_failed, 0);
}

/// The visible-level bound and the per-connection level record bound, both
/// reached by real reports under a small configured bound: a full set
/// ignores further adds; hides free set room but hidden levels keep their
/// records, so a new level then gets its opens refused while it is visible.
#[test]
fn the_level_bounds_are_reached_by_real_reports() {
    let mut lab = level_lab();
    lab.level_visibility.as_mut().unwrap().max_visible_levels = 2;
    let (socket, address, mut client, mut sender) = level_peer(&mut lab, 1, "AuthoredBound");
    for level in [LEVEL_A, LEVEL_B] {
        report(
            &mut lab,
            &socket,
            &mut client,
            &mut sender,
            &[(82, rpc82(true, level, false))],
        );
    }
    let third = tile(3);
    let bunches = report(
        &mut lab,
        &socket,
        &mut client,
        &mut sender,
        &[(82, rpc82(true, &third, false))],
    );
    assert!(level_opens(&lab, &bunches).is_empty());
    assert_eq!(lab.stats.level_visibility_reports_over_bound, 1);
    report(
        &mut lab,
        &socket,
        &mut client,
        &mut sender,
        &[(82, rpc82(false, LEVEL_A, false))],
    );
    let bunches = report(
        &mut lab,
        &socket,
        &mut client,
        &mut sender,
        &[(82, rpc82(true, &third, false))],
    );
    assert!(level_opens(&lab, &bunches).is_empty());
    assert_eq!(lab.stats.level_visibility_reports_added, 3);
    assert_eq!(lab.stats.level_actor_opens_refused, 2);
    assert_eq!(lab.peers[&address].visible_levels.len(), 2);
    assert_eq!(lab.peers[&address].level_actors.len(), 2);
    assert_eq!(lab.stats.level_actor_opens_sent, 4);
}

/// Enabling level visibility never costs a call the bunch would otherwise
/// deliver: a refused field 82 batched with Begin, with the friends response
/// or with a profile response leaves that call applied, exactly as with the
/// switch off, and the connection up.
#[test]
fn a_refused_level_report_never_drops_the_calls_beside_it() {
    use super::super::super::{
        probe_initialise::{
            authored_user_profile, begin_player_initialisation_field, friends_list_updated_field,
            initialise_codec,
        },
        rep_rpc::{BACKEND_PROXY_FIELD_INDEX_BOUND, backend_profile_response_parameters},
    };
    let codec = initialise_codec(17);
    let begin = begin_player_initialisation_field(3, &codec).unwrap();
    let friends = friends_list_updated_field(&codec).unwrap();
    let mut items = vec![RpcItem::Value(RepValue::Bool(true))];
    items.extend(authored_user_profile().unwrap());
    let profile = encode_rpc_payload(
        &backend_profile_response_parameters(false),
        &[RpcArgument::Sent(items)],
        &codec,
    )
    .unwrap();
    let mut outcomes = Vec::new();
    for enabled in [false, true] {
        let mut lab = if enabled {
            level_lab()
        } else {
            possession_lab(true)
        };
        let (socket, address, mut client, mut sender) = level_peer(&mut lab, 1, "AuthoredBatch");
        // Begin after a refused report, in one bunch.
        sender.send(
            &mut lab,
            &socket,
            &mut client,
            &[
                (82, rpc82_truncated()),
                (begin.index, begin.payload.clone()),
            ],
        );
        assert_eq!(lab.peers[&address].character_slot, Some(3));
        deliver_bunches(&mut lab, &socket, &mut client, |_| true);
        // The friends response before one.
        sender.send(
            &mut lab,
            &socket,
            &mut client,
            &[
                (friends.index, friends.payload.clone()),
                (82, rpc82(true, LEVEL_A, true)),
            ],
        );
        assert!(lab.peers[&address].profiles.friend_ids_ready);
        deliver_bunches(&mut lab, &socket, &mut client, |_| true);
        // A profile response on the BackendProxy beside one on the actor.
        let proxy = lab.peers[&address].backend_proxy.unwrap();
        lab.peers.get_mut(&address).unwrap().profiles.outstanding = Some(ProfileKind::User);
        sender.send_blocks(
            &mut lab,
            &socket,
            &mut client,
            vec![
                field_block(
                    BlockTarget::Actor,
                    BP_ICARUS_PLAYER_CONTROLLER_SURVIVAL_FIELD_INDEX_BOUND,
                    &[(82, rpc82_truncated())],
                ),
                field_block(
                    BlockTarget::SubObject {
                        object: super::super::super::actor_channel::NetGuidRef::plain(proxy),
                        class: super::super::super::actor_channel::SubObjectClass::ClientSent,
                    },
                    BACKEND_PROXY_FIELD_INDEX_BOUND,
                    &[(RESPONSE_GET_USER_PROFILE, profile.clone())],
                ),
            ],
        );
        // Applied: the user profile is taken and the character profile
        // requested next.
        assert!(lab.peers[&address].profiles.outstanding == Some(ProfileKind::Character));
        assert!(lab.has_peer(address));
        let stats = &lab.stats;
        assert_eq!(stats.actor_bunches_malformed, 0);
        assert_eq!(
            stats.level_visibility_reports_malformed,
            if enabled { 3 } else { 0 }
        );
        assert_eq!(stats.controller_fields_ignored, if enabled { 0 } else { 3 });
        let expected = if enabled {
            ControllerFieldDisposition::Malformed
        } else {
            ControllerFieldDisposition::Ignored
        };
        assert!(
            lab.controller_field_diagnostics(address)
                .unwrap()
                .entries
                .iter()
                .filter(|entry| entry.index == 82)
                .all(|entry| entry.disposition == expected)
        );
        outcomes.push((
            stats.initialisation_slots_recorded,
            stats.friends_lists_received,
            stats.profile_user_responses_recognized,
            stats.profile_user_responses_received,
            stats.profile_responses_ignored,
        ));
    }
    assert_eq!(outcomes[0], outcomes[1]);
    assert_eq!(outcomes[1], (1, 1, 1, 1, 0));
}

/// Level actors stop at the channel budget and always leave the reserve:
/// a client reporting more levels than fit gets the rest refused (counted,
/// shown in readiness), and the server's own replication opens on that
/// connection still succeed, every reserved channel of them.
#[test]
fn level_actors_leave_the_channel_reserve_to_replication() {
    let mut lab = level_lab();
    let first = first_level_channel(&lab);
    lab.config.packet.packet.limits.max_channels = first + LEVEL_ACTOR_CHANNEL_RESERVE as u32 + 6;
    let (socket, address, mut client, mut sender) = level_peer(&mut lab, 1, "AuthoredBudget");
    assert_eq!(
        lab.readiness_counts()
            .level_visibility
            .channel_budget_remaining,
        6
    );
    let fields: Vec<_> = (0..5)
        .map(|index| (82, rpc82(true, &tile(index), false)))
        .collect();
    let bunches = report(&mut lab, &socket, &mut client, &mut sender, &fields);
    let opens = level_opens(&lab, &bunches);
    assert_eq!(opens.len(), 6);
    assert!(opens.iter().all(|open| open.channel >= first));
    assert_eq!(lab.stats.level_actor_opens_sent, 6);
    assert_eq!(lab.stats.level_actor_opens_refused, 4);
    let levels = lab.readiness_counts().level_visibility;
    assert_eq!(levels.channels_held, 6);
    assert_eq!(levels.channel_budget_remaining, 0);
    assert_eq!(levels.opens_refused_waiting, 4);
    // Every reserved channel is still open to the server's own opens.
    for key in 0..LEVEL_ACTOR_CHANNEL_RESERVE as u64 {
        let spec = replication_spec(&lab, "/Game/Authored/Replicated/AuthoredReplicated");
        assert!(
            lab.open_actor(1, 100 + key, spec, first, 17).is_some(),
            "replication open {key}"
        );
        deliver_bunches(&mut lab, &socket, &mut client, |_| true);
    }
    assert_eq!(lab.stats.actor_open_errors, 0);
    assert!(lab.has_peer(address));
}

/// A released actor channel (a replication close's ACK) makes refused level
/// opens of visible levels eligible again, one per freed channel beyond the
/// reserve; a refusal again is counted again.
#[test]
fn a_released_channel_makes_a_refused_level_open_eligible() {
    let mut lab = level_lab();
    let first = first_level_channel(&lab);
    lab.config.packet.packet.limits.max_channels = first + LEVEL_ACTOR_CHANNEL_RESERVE as u32 + 6;
    let (socket, address, mut client, mut sender) = level_peer(&mut lab, 1, "AuthoredRelease");
    for key in [7, 8] {
        let spec = replication_spec(&lab, "/Game/Authored/Replicated/AuthoredReplicated");
        assert!(lab.open_actor(1, key, spec, first, 17).is_some());
    }
    deliver_bunches(&mut lab, &socket, &mut client, |_| true);
    let fields: Vec<_> = (0..3)
        .map(|index| (82, rpc82(true, &tile(index), false)))
        .collect();
    let bunches = report(&mut lab, &socket, &mut client, &mut sender, &fields);
    assert_eq!(level_opens(&lab, &bunches).len(), 4);
    assert_eq!(lab.stats.level_actor_opens_refused, 2);
    assert_eq!(level_slots(&lab).refused, 2);
    // Free one channel: exactly one refused open is retried, and goes out;
    // the other waits, untried and not counted again.
    assert!(lab.close_actor(1, 7, 0));
    let bunches = admit(&mut lab, &socket, &mut client);
    let opens = level_opens(&lab, &bunches);
    assert_eq!(opens.len(), 1);
    assert_level_open(&opens[0], 0, &tile(2));
    assert_eq!(lab.stats.level_actor_opens_requeued, 1, "exactly one retry");
    assert_eq!(lab.stats.level_actor_opens_refused, 2);
    assert_eq!(level_slots(&lab).refused, 1);
    assert!(lab.close_actor(1, 8, 0));
    let bunches = admit(&mut lab, &socket, &mut client);
    let opens = level_opens(&lab, &bunches);
    assert_eq!(opens.len(), 1);
    assert_level_open(&opens[0], 1, &tile(2));
    assert_eq!(lab.stats.level_actor_opens_sent, 6);
    assert_eq!(lab.stats.level_actor_opens_requeued, 2);
    assert_eq!(lab.stats.level_actor_opens_refused, 2);
    assert_eq!(level_slots(&lab).refused, 0);
    // Without a release nothing is retried.
    lab.retry_level_actor_opens();
    assert!(deliver_bunches(&mut lab, &socket, &mut client, |_| true).is_empty());
    assert!(lab.has_peer(address));
}

/// The pass in which each kind of the session's own traffic first left, and
/// the level opens per pass, over polls with everything ACKed between them:
/// the possession update and the connected-player record held by a full
/// sent ring, then GameState clock updates; with `burst`, four level reports
/// arrive while they are held.
fn priority_passes(burst: bool) -> ([Option<usize>; 3], Vec<usize>, Vec<Vec<char>>) {
    let mut lab = level_lab();
    let (socket, address, mut client, mut sender) = level_peer(&mut lab, 1, "AuthoredPriority");
    complete_record_player(&mut lab, address);
    deliver_bunches(&mut lab, &socket, &mut client, |_| true);
    lab.controller_call(address, ControllerCall::FriendsList);
    // The possession update due again, as when its pawn open commits.
    let entry = lab.peers.get_mut(&address).unwrap();
    entry.controller_possession_receipt = InitialisationReceipt::default();
    entry.controller_possession_pending = true;
    entry.controller_possession_retry_ready = false;
    let held = fill_sent_ring(&mut lab, &socket, address, None);
    lab.finalise_connected_players();
    assert_eq!(lab.stats.connected_player_record_creations_sent, 0);
    if burst {
        let fields: Vec<_> = (0..4)
            .map(|index| (82, rpc82(true, &tile(index), false)))
            .collect();
        sender.send(&mut lab, &socket, &mut client, &fields);
        assert_eq!(lab.stats.level_actor_opens_sent, 0);
        assert_eq!(level_slots(&lab).pending, 8);
    }
    let channel = game_state_channel(&lab);
    let mut first = [None; 3];
    let mut opens = Vec::new();
    let mut order = Vec::new();
    for pass in 0..8 {
        force_clock_due(&mut lab, address);
        if pass == 0 {
            ack_held_and_poll(&mut lab, &socket, &mut client, &held);
        } else {
            lab.poll();
        }
        let bunches = deliver_bunches(&mut lab, &socket, &mut client, |_| true);
        let mut kinds = Vec::new();
        for bunch in &bunches {
            let header = &bunch.header;
            let kind = if possession_shaped(bunch) {
                'p'
            } else if header.channel_index == channel && header.reliable {
                'r'
            } else if header.channel_index == channel {
                'g'
            } else if is_level_open_part(&lab, bunch) {
                'l'
            } else {
                continue;
            };
            if kinds.last() != Some(&kind) {
                kinds.push(kind);
            }
            if let Some(slot) = "prg".find(kind)
                && first[slot].is_none()
            {
                first[slot] = Some(pass);
            }
        }
        opens.push(level_opens(&lab, &bunches).len());
        order.push(kinds);
    }
    assert_eq!(lab.stats.connected_player_record_creations_acked, 1);
    (first, opens, order)
}

/// Level opens are the lowest priority: a burst of reports, arriving while
/// the session's possession update and connected-player record are held by
/// transport pressure, delays neither of them nor the GameState updates;
/// the opens follow, at most two per poll, behind that pass's own traffic.
#[test]
fn a_report_burst_never_delays_the_sessions_own_traffic() {
    let (quiet, quiet_opens, _) = priority_passes(false);
    let (burst, burst_opens, order) = priority_passes(true);
    assert!(quiet.iter().all(Option::is_some), "{quiet:?}");
    assert_eq!(burst, quiet);
    assert!(quiet_opens.iter().all(|opens| *opens == 0));
    assert!(
        burst_opens
            .iter()
            .all(|opens| *opens <= LEVEL_ACTOR_OPENS_PER_PASS)
    );
    assert_eq!(burst_opens.iter().sum::<usize>(), 8);
    // Within each pass the level opens come last.
    for kinds in &order {
        if let Some(level) = kinds.iter().position(|kind| *kind == 'l') {
            assert_eq!(level, kinds.len() - 1, "{order:?}");
        }
    }
    assert_eq!(order[0], ['p', 'r', 'l']);
}

/// Fill `address`'s sent ring with ack-only packets, taken off `socket` but
/// not yet given to the client (so its next packet ACKs none of them),
/// leaving `leave` records free (none by default).
fn fill_sent_ring(
    lab: &mut PeerLab,
    socket: &UdpSocket,
    address: SocketAddrV4,
    leave: Option<usize>,
) -> Vec<Vec<u8>> {
    let entry = lab.peers.get_mut(&address).unwrap();
    let free =
        entry.peer.config().sent_capacity - entry.peer.sequencer().notify().sent_records().len();
    let count = free - leave.unwrap_or(0);
    let mut held = Vec::with_capacity(count);
    for _ in 0..count {
        let entry = lab.peers.get_mut(&address).unwrap();
        let datagram = entry.peer.ack_packet().unwrap();
        lab.socket.send_to(&datagram, address).unwrap();
        let mut bytes = [0; 2048];
        let length = socket.recv(&mut bytes).unwrap();
        held.push(bytes[..length].to_vec());
    }
    held
}

/// Give the client the `held` datagrams and ACK them, then send the ACK
/// through [`PeerLab::poll`].
fn ack_held_and_poll(
    lab: &mut PeerLab,
    socket: &UdpSocket,
    client: &mut PacketSequencer,
    held: &[Vec<u8>],
) {
    use super::super::super::packet::{PacketConfig, Receiver, read_packet};
    let config = PacketConfig {
        receiver: Receiver::Client,
        ..lab.config.packet.packet
    };
    for bytes in held {
        let packet = read_packet(bytes, &config).unwrap();
        let received = client.receive(&packet.header).unwrap().unwrap();
        client.ack_seq(received.sequence, true).unwrap();
    }
    let bytes = write_packet(
        &Packet {
            magic: 0,
            header: client.header().unwrap(),
            info: None,
            bunches: vec![],
        },
        &lab.config.packet.packet,
    )
    .unwrap();
    client.commit().unwrap();
    socket.send(&bytes).unwrap();
    lab.poll();
}

/// Under transport pressure no level open goes out and the waiting passes
/// do not spin; after ACK progress all go out once, in report order.
#[test]
fn pressure_defers_level_opens_then_admits_them_in_report_order() {
    let mut lab = level_lab();
    let (socket, address, mut client, mut sender) = level_peer(&mut lab, 1, "AuthoredPressure");
    let first = first_level_channel(&lab);
    let held = fill_sent_ring(&mut lab, &socket, address, None);
    sender.send(
        &mut lab,
        &socket,
        &mut client,
        &[
            (82, rpc82(true, LEVEL_A, false)),
            (82, rpc82(true, LEVEL_B, false)),
        ],
    );
    for _ in 0..3 {
        lab.retry_level_actor_opens();
    }
    assert_eq!(lab.stats.level_actor_opens_sent, 0);
    assert_eq!(lab.stats.level_actor_opens_deferred, 1);
    assert_eq!(level_slots(&lab).pending, 4);
    ack_held_and_poll(&mut lab, &socket, &mut client, &held);
    let bunches = admit(&mut lab, &socket, &mut client);
    let opens = level_opens(&lab, &bunches);
    assert_eq!(
        opens.iter().map(|open| open.channel).collect::<Vec<_>>(),
        [first, first + 1, first + 2, first + 3]
    );
    assert_level_open(&opens[0], 0, LEVEL_A);
    assert_level_open(&opens[1], 1, LEVEL_A);
    assert_level_open(&opens[2], 0, LEVEL_B);
    assert_level_open(&opens[3], 1, LEVEL_B);
    assert_eq!(lab.stats.level_actor_opens_sent, 4);
    assert_eq!(lab.stats.level_actor_opens_acked, 4);
    assert_eq!(lab.stats.level_actor_open_errors, 0);
}

/// Two connections reporting different levels each receive only their own
/// level's actors; a level both report shares its driver-wide static GUIDs.
#[test]
fn connections_receive_only_the_levels_they_report() {
    let mut lab = level_lab();
    let (socket_one, _one, mut client_one, mut sender_one) =
        level_peer(&mut lab, 1, "AuthoredFirst");
    let (socket_two, _two, mut client_two, mut sender_two) =
        level_peer(&mut lab, 2, "AuthoredSecond");
    let a = report(
        &mut lab,
        &socket_one,
        &mut client_one,
        &mut sender_one,
        &[(82, rpc82(true, LEVEL_A, false))],
    );
    let b = report(
        &mut lab,
        &socket_two,
        &mut client_two,
        &mut sender_two,
        &[(82, rpc82(true, LEVEL_B, false))],
    );
    let a = level_opens(&lab, &a);
    let b = level_opens(&lab, &b);
    assert_eq!((a.len(), b.len()), (2, 2));
    for (index, open) in a.iter().enumerate() {
        assert_level_open(open, index, LEVEL_A);
    }
    for (index, open) in b.iter().enumerate() {
        assert_level_open(open, index, LEVEL_B);
    }
    assert!(deliver_bunches(&mut lab, &socket_one, &mut client_one, |_| true).is_empty());
    assert!(deliver_bunches(&mut lab, &socket_two, &mut client_two, |_| true).is_empty());
}

/// The same level held by two connections survives either leaving first;
/// a third connection then reporting it reuses the survivor's GUIDs, and
/// they are forgotten only with the last holder.
#[test]
fn a_shared_level_survives_either_holder_leaving_first() {
    for one_leaves_first in [true, false] {
        let mut lab = level_lab();
        let statics = lab.guid_cache.static_count();
        let (socket_one, one, mut client_one, mut sender_one) =
            level_peer(&mut lab, 1, "AuthoredFirst");
        let (socket_two, two, mut client_two, mut sender_two) =
            level_peer(&mut lab, 2, "AuthoredSecond");
        let base = lab.guid_cache.static_count();
        let first = report(
            &mut lab,
            &socket_one,
            &mut client_one,
            &mut sender_one,
            &[(82, rpc82(true, LEVEL_A, false))],
        );
        let guids: Vec<u32> = level_opens(&lab, &first)
            .iter()
            .enumerate()
            .map(|(index, open)| assert_level_open(open, index, LEVEL_A))
            .collect();
        let second = report(
            &mut lab,
            &socket_two,
            &mut client_two,
            &mut sender_two,
            &[(82, rpc82(true, LEVEL_A, false))],
        );
        let second: Vec<u32> = level_opens(&lab, &second)
            .iter()
            .enumerate()
            .map(|(index, open)| assert_level_open(open, index, LEVEL_A))
            .collect();
        assert_eq!(second, guids);
        assert_eq!(lab.guid_cache.static_count(), base + 5);
        let (leaver, stayer) = if one_leaves_first {
            (one, two)
        } else {
            (two, one)
        };
        lab.remove_peer(leaver).unwrap();
        assert_eq!(lab.guid_cache.static_count(), base + 5);
        let (socket_three, three, mut client_three, mut sender_three) =
            level_peer(&mut lab, 3, "AuthoredThird");
        let third = report(
            &mut lab,
            &socket_three,
            &mut client_three,
            &mut sender_three,
            &[(82, rpc82(true, LEVEL_A, false))],
        );
        let third: Vec<u32> = level_opens(&lab, &third)
            .iter()
            .enumerate()
            .map(|(index, open)| assert_level_open(open, index, LEVEL_A))
            .collect();
        assert_eq!(third, guids);
        lab.remove_peer(stayer).unwrap();
        assert_eq!(lab.guid_cache.static_count(), base + 5);
        lab.remove_peer(three).unwrap();
        assert_eq!(lab.guid_cache.static_count(), base);
        assert!(lab.level_actor_holders.is_empty());
        let _ = (socket_one, socket_two, statics);
    }
}

/// Names fold by ASCII case: a variant spelling of a visible level is a
/// duplicate and hides it; another connection's variant shares its GUIDs
/// and receives the first holder's spelling; once nobody holds it a new
/// report's spelling is used.
#[test]
fn level_names_fold_by_case_and_keep_the_first_holders_spelling() {
    let mut lab = level_lab();
    let variant = LEVEL_A.replace("Tile", "TILE").replace("/Game/", "/GAME/");
    let (socket_one, one, mut client_one, mut sender_one) = level_peer(&mut lab, 1, "AuthoredOne");
    let statics = lab.guid_cache.static_count();
    let first = report(
        &mut lab,
        &socket_one,
        &mut client_one,
        &mut sender_one,
        &[(82, rpc82(true, LEVEL_A, false))],
    );
    let guids: Vec<u32> = level_opens(&lab, &first)
        .iter()
        .enumerate()
        .map(|(index, open)| assert_level_open(open, index, LEVEL_A))
        .collect();
    for visible in [true, false, true] {
        let bunches = report(
            &mut lab,
            &socket_one,
            &mut client_one,
            &mut sender_one,
            &[(82, rpc82(visible, &variant, false))],
        );
        assert!(level_opens(&lab, &bunches).is_empty());
    }
    assert_eq!(lab.stats.level_visibility_reports_duplicate, 1);
    assert_eq!(lab.stats.level_hides_actors_kept, 1);
    assert_eq!(lab.peers[&one].visible_levels.len(), 1);
    assert_eq!(lab.peers[&one].level_actors.len(), 1);
    let (socket_two, two, mut client_two, mut sender_two) = level_peer(&mut lab, 2, "AuthoredTwo");
    let second = report(
        &mut lab,
        &socket_two,
        &mut client_two,
        &mut sender_two,
        &[(82, rpc82(true, &variant, false))],
    );
    let second: Vec<u32> = level_opens(&lab, &second)
        .iter()
        .enumerate()
        .map(|(index, open)| assert_level_open(open, index, LEVEL_A))
        .collect();
    assert_eq!(second, guids);
    lab.remove_peer(one).unwrap();
    lab.remove_peer(two).unwrap();
    assert_eq!(lab.guid_cache.static_count(), statics);
    let (socket_three, _three, mut client_three, mut sender_three) =
        level_peer(&mut lab, 3, "AuthoredThree");
    let third = report(
        &mut lab,
        &socket_three,
        &mut client_three,
        &mut sender_three,
        &[(82, rpc82(true, &variant, false))],
    );
    let third = level_opens(&lab, &third);
    assert_eq!(third.len(), 2);
    assert_level_open(&third[0], 0, &variant);
    assert_level_open(&third[1], 1, &variant);
}

/// A static identity a level created and a later server open came to share
/// (here an archetype exported from the level's own package after the
/// report) survives the reporter leaving; later opens keep its GUID, while
/// the level's other identities are forgotten.
#[test]
fn a_shared_identity_survives_the_reporters_departure() {
    let mut lab = level_lab();
    let (socket_one, one, mut client_one, mut sender_one) = level_peer(&mut lab, 1, "AuthoredOne");
    let (socket_two, _two, mut client_two, _sender_two) = level_peer(&mut lab, 2, "AuthoredTwo");
    report(
        &mut lab,
        &socket_one,
        &mut client_one,
        &mut sender_one,
        &[(82, rpc82(true, LEVEL_A, false))],
    );
    let package = [PathSegment::new(LEVEL_A, 0, true)];
    let shared = lab.guid_cache.static_guid(&package).unwrap();
    let first = first_level_channel(&lab);
    let spec = replication_spec(&lab, LEVEL_A);
    assert!(lab.open_actor(2, 7, spec, first, 17).is_some());
    deliver_bunches(&mut lab, &socket_two, &mut client_two, |_| true);
    let actor_chain = ReleaseProfile::release_3_0_35()
        .level_actors
        .unwrap()
        .actor_chain(0, LEVEL_A)
        .unwrap();
    assert!(lab.guid_cache.static_guid(&actor_chain).is_some());
    lab.remove_peer(one).unwrap();
    assert_eq!(lab.guid_cache.static_guid(&package), Some(shared));
    assert!(lab.guid_cache.static_guid(&actor_chain).is_none());
    // A later open on a fresh connection exports the same package GUID.
    let (socket_three, _three, mut client_three, _sender_three) =
        level_peer(&mut lab, 3, "AuthoredThree");
    let spec = replication_spec(&lab, LEVEL_A);
    assert!(lab.open_actor(3, 7, spec, first, 17).is_some());
    let bunches = deliver_bunches(&mut lab, &socket_three, &mut client_three, |_| true);
    let codec = actor_codec(17, super::super::super::archive::ByteOrder::Little);
    let mut package_guids = Vec::new();
    for bunch in bunches
        .iter()
        .filter(|bunch| bunch.header.has_package_map_exports)
    {
        let references = super::super::super::actor_channel::read_export_prefix(
            &mut bunch.payload.reader(),
            &codec,
        )
        .unwrap();
        for reference in &references {
            let mut current = reference;
            while let Some(export) = &current.export {
                let Some(path) = &export.path else {
                    break;
                };
                if String::from_utf16(path.path.units()).unwrap() == LEVEL_A {
                    package_guids.push(current.guid);
                }
                current = &path.outer;
            }
        }
    }
    assert_eq!(package_guids, [shared]);
    assert_eq!(lab.guid_cache.static_guid(&package), Some(shared));
}

/// The static GUID space refuses exhaustion as the dynamic one does, never
/// wrapping: at the boundary a chain needing more GUIDs than remain is
/// refused with the cache unchanged; the level-open path counts it as an
/// error and keeps the connection.
#[test]
fn static_guid_exhaustion_is_refused_and_counted() {
    use super::super::super::session_open::{ConnectionExports, NetGuidCache};
    let mut cache = NetGuidCache::new();
    cache.set_static_index((u32::MAX >> 1) - 1);
    let mut exports = ConnectionExports::new();
    let two = [
        PathSegment::new("Leaf", 0, true),
        PathSegment::new("/Game/Authored/Outer", 0, true),
    ];
    assert!(cache.export_static(&mut exports, &two).is_err());
    assert_eq!(cache.static_count(), 0);
    let one = [PathSegment::new("/Game/Authored/Last", 0, true)];
    assert_eq!(
        cache.export_static(&mut exports, &one).unwrap().guid,
        u32::MAX
    );
    assert_eq!(cache.static_count(), 1);
    assert_eq!(cache.assign_static(&two), 0, "refused, never wrapped");
    assert_eq!(
        cache.assign_static(&one),
        u32::MAX,
        "held identities remain"
    );
    assert!(
        cache
            .export_static(
                &mut exports,
                &[PathSegment::new("/Game/Authored/Next", 0, true)]
            )
            .is_err()
    );
    // The level-open path: both opens fail as counted errors.
    let mut lab = level_lab();
    let (socket, address, mut client, mut sender) = level_peer(&mut lab, 1, "AuthoredExhausted");
    lab.guid_cache.set_static_index((u32::MAX >> 1) - 2);
    let bunches = report(
        &mut lab,
        &socket,
        &mut client,
        &mut sender,
        &[(82, rpc82(true, LEVEL_A, false))],
    );
    assert!(level_opens(&lab, &bunches).is_empty());
    assert_eq!(lab.stats.level_actor_open_errors, 2);
    assert_eq!(lab.stats.level_actor_opens_sent, 0);
    assert!(lab.has_peer(address));
    assert_eq!(lab.stats.peers_failed, 0);
}

/// A level never ends with half a pair: an open deferred after its pair's
/// first was admitted, then a hide, then a re-add, opens the second; a hide
/// while both are pending then a re-add opens both; a hide while they await
/// ACK, or after they settled, then a re-add, opens nothing more.
#[test]
fn a_re_added_level_completes_its_pair_and_never_reopens() {
    // Deferred second open, hidden, re-added.
    let mut lab = level_lab();
    let (socket, address, mut client, mut sender) = level_peer(&mut lab, 1, "AuthoredHalf");
    let capacity = lab.peers[&address].peer.config().sent_capacity;
    let held = fill_sent_ring(&mut lab, &socket, address, Some(capacity / 2));
    sender.send(
        &mut lab,
        &socket,
        &mut client,
        &[(82, rpc82(true, LEVEL_A, false))],
    );
    lab.retry_level_actor_opens();
    assert_eq!(lab.stats.level_actor_opens_sent, 1);
    assert_eq!(lab.stats.level_actor_opens_deferred, 1);
    sender.send(
        &mut lab,
        &socket,
        &mut client,
        &[(82, rpc82(false, LEVEL_A, false))],
    );
    assert_eq!(lab.stats.level_actor_opens_withdrawn, 1);
    assert_eq!(lab.stats.level_hides_actors_kept, 1);
    ack_held_and_poll(&mut lab, &socket, &mut client, &held);
    let mut bunches = deliver_bunches(&mut lab, &socket, &mut client, |_| true);
    bunches.extend(report(
        &mut lab,
        &socket,
        &mut client,
        &mut sender,
        &[(82, rpc82(true, LEVEL_A, false))],
    ));
    let opens = level_opens(&lab, &bunches);
    assert_eq!(opens.len(), 2);
    assert_level_open(&opens[0], 0, LEVEL_A);
    assert_level_open(&opens[1], 1, LEVEL_A);
    assert_ne!(opens[0].channel, opens[1].channel);
    assert_eq!(lab.stats.level_actor_opens_sent, 2);
    assert_eq!(level_slots(&lab).settled, 2);
    // Hidden while both pending, then re-added.
    let mut lab = level_lab();
    let (socket, _address, mut client, mut sender) = level_peer(&mut lab, 1, "AuthoredPending");
    for visible in [true, false] {
        sender.send(
            &mut lab,
            &socket,
            &mut client,
            &[(82, rpc82(visible, LEVEL_A, false))],
        );
    }
    assert_eq!(lab.stats.level_actor_opens_withdrawn, 2);
    assert_eq!(level_slots(&lab), Slots::default());
    let bunches = report(
        &mut lab,
        &socket,
        &mut client,
        &mut sender,
        &[(82, rpc82(true, LEVEL_A, false))],
    );
    assert_eq!(level_opens(&lab, &bunches).len(), 2);
    assert_eq!(lab.stats.level_actor_opens_sent, 2);
    assert_eq!(level_slots(&lab).settled, 2);
    // Hidden while awaiting ACK, and again after settling; re-added each time.
    for settle_first in [false, true] {
        let mut lab = level_lab();
        let (socket, _address, mut client, mut sender) =
            level_peer(&mut lab, 1, "AuthoredAwaiting");
        sender.send(
            &mut lab,
            &socket,
            &mut client,
            &[(82, rpc82(true, LEVEL_A, false))],
        );
        lab.retry_level_actor_opens();
        assert_eq!(level_slots(&lab).awaiting, 2);
        if settle_first {
            deliver_bunches(&mut lab, &socket, &mut client, |_| true);
        }
        for visible in [false, true] {
            sender.send(
                &mut lab,
                &socket,
                &mut client,
                &[(82, rpc82(visible, LEVEL_A, false))],
            );
            lab.retry_level_actor_opens();
        }
        let bunches = admit(&mut lab, &socket, &mut client);
        let reopened = bunches
            .iter()
            .filter(|bunch| is_level_open_part(&lab, bunch) && bunch.header.has_package_map_exports)
            .count();
        assert_eq!(
            reopened,
            if settle_first { 0 } else { 2 },
            "repairs or nothing"
        );
        assert_eq!(lab.stats.level_actor_opens_sent, 2);
        assert_eq!(lab.stats.level_actor_opens_acked, 2);
        assert_eq!(level_slots(&lab).settled, 2);
    }
}

/// A report that arrives while the session's other opens are still
/// unacknowledged (here before the pawn, GameState and PlayerState opens
/// even commit) is honoured on channels the session opens never use; the
/// session's own opens then still take their channels.
#[test]
fn a_report_before_the_session_opens_settle_is_honoured() {
    let mut lab = level_lab();
    let (socket, address, mut client) = possession_identity_peer(&mut lab, 1, "AuthoredEarly");
    assert!(matches!(
        lab.send_initial_controller_open(address),
        InitialOpenOutcome::Committed(_)
    ));
    let reserved = lab.reserved_session_channels();
    let mut sender = ControllerSender::new(&client);
    sender.send(
        &mut lab,
        &socket,
        &mut client,
        &[(82, rpc82(true, LEVEL_A, false))],
    );
    lab.retry_level_actor_opens();
    assert_eq!(lab.stats.level_actor_opens_sent, 2);
    let channels: Vec<u32> = lab.peers[&address].level_actors[&LEVEL_A.to_ascii_lowercase()]
        .iter()
        .map(|slot| match slot {
            LevelActorSlot::Opened { channel, .. } => *channel,
            _ => panic!("opened"),
        })
        .collect();
    assert!(channels.iter().all(|channel| !reserved.contains(channel)));
    let bunches = deliver_bunches(&mut lab, &socket, &mut client, |_| true);
    assert_eq!(level_opens(&lab, &bunches).len(), 2);
    assert_eq!(lab.stats.level_actor_opens_acked, 2);
    let (controller, player_state) = lab.peers[&address].controller_open_guids.unwrap();
    assert!(matches!(
        lab.send_pawn_open(address, controller, player_state),
        InitialOpenOutcome::Committed(_)
    ));
    assert!(lab.has_peer(address));
}

/// A NAK of the datagram carrying an open is repaired by the reliable layer
/// from the stored parts, byte for byte; the open is ACKed and counted once.
#[test]
fn a_naked_level_open_is_repaired_without_recomposition() {
    let mut lab = level_lab();
    let (socket, address, mut client, mut sender) = level_peer(&mut lab, 1, "AuthoredNak");
    sender.send(
        &mut lab,
        &socket,
        &mut client,
        &[(82, rpc82(true, LEVEL_A, false))],
    );
    lab.retry_level_actor_opens();
    let reserved = lab.reserved_session_channels();
    let lost = deliver_bunches_by_content(&mut lab, &socket, &mut client, |bunches| {
        !bunches
            .iter()
            .any(|bunch| bunch.header.open && !reserved.contains(&bunch.header.channel_index))
    });
    let original = level_opens(&lab, &lost);
    assert_eq!(original.len(), 2);
    assert_eq!(lab.stats.level_actor_opens_acked, 0);
    assert_eq!(level_slots(&lab).awaiting, 2);
    let repair = lab
        .peers
        .get_mut(&address)
        .unwrap()
        .peer
        .ack_packet()
        .unwrap();
    assert!(lab.send(&repair, address));
    let repaired = deliver_bunches(&mut lab, &socket, &mut client, |_| true);
    let repaired = level_opens(&lab, &repaired);
    assert_eq!(repaired.len(), 2);
    for (repaired, original) in repaired.iter().zip(&original) {
        assert_eq!(repaired.channel, original.channel);
        assert_eq!(repaired.export_bits, original.export_bits);
        assert_eq!(repaired.body_bytes, original.body_bytes);
        assert_eq!(repaired.body_bits, original.body_bits);
    }
    assert_eq!(lab.stats.level_actor_opens_sent, 2);
    assert_eq!(lab.stats.level_actor_opens_acked, 2);
    let more = lab
        .peers
        .get_mut(&address)
        .unwrap()
        .peer
        .ack_packet()
        .unwrap();
    assert!(lab.send(&more, address));
    deliver_bunches(&mut lab, &socket, &mut client, |_| true);
    assert_eq!(lab.stats.level_actor_opens_acked, 2);
    assert_eq!(level_slots(&lab).settled, 2);
}

/// Leaving at each stage (opens queued, admitted and unacknowledged,
/// settled with a hidden level kept open) releases every level structure
/// and the level's static GUIDs; a reconnect from the same address starts
/// clean and gets the level's actors again under fresh GUIDs.
#[test]
fn level_state_leaves_with_its_session_and_a_reconnect_starts_clean() {
    for stage in 0..3 {
        let mut lab = level_lab();
        let (socket, address, mut client, mut sender) = level_peer(&mut lab, 1, "AuthoredLeave");
        let statics = lab.guid_cache.static_count();
        sender.send(
            &mut lab,
            &socket,
            &mut client,
            &[
                (82, rpc82(true, LEVEL_A, false)),
                (82, rpc82(true, LEVEL_B, false)),
            ],
        );
        if stage == 1 {
            lab.retry_level_actor_opens();
            lab.retry_level_actor_opens();
        }
        if stage == 2 {
            admit(&mut lab, &socket, &mut client);
            sender.send(
                &mut lab,
                &socket,
                &mut client,
                &[(82, rpc82(false, LEVEL_A, false))],
            );
        }
        assert_eq!(level_slots(&lab).with_actors, 2, "stage {stage}");
        lab.remove_peer(address).unwrap();
        assert_eq!(level_slots(&lab), Slots::default(), "stage {stage}");
        assert_eq!(
            lab.readiness_counts().level_visibility,
            LevelVisibilityReadiness::default()
        );
        assert!(lab.level_actor_holders.is_empty());
        assert_eq!(lab.guid_cache.static_count(), statics, "stage {stage}");
        lab.retry_level_actor_opens();
        let sent = lab.stats.level_actor_opens_sent;
        assert_eq!(sent, if stage == 0 { 0 } else { 4 }, "stage {stage}");
    }
    let mut lab = level_lab();
    let (socket, address, mut client, mut sender) = level_peer(&mut lab, 1, "AuthoredReconnect");
    let first = report(
        &mut lab,
        &socket,
        &mut client,
        &mut sender,
        &[(82, rpc82(true, LEVEL_A, false))],
    );
    let first = level_opens(&lab, &first);
    let (rejoined, mut client) = record_peer_on(&mut lab, &socket, 2, "AuthoredReconnect");
    assert_eq!(rejoined, address);
    assert_eq!(level_slots(&lab), Slots::default());
    let mut sender = ControllerSender::new(&client);
    let second = report(
        &mut lab,
        &socket,
        &mut client,
        &mut sender,
        &[(82, rpc82(true, LEVEL_A, false))],
    );
    let second = level_opens(&lab, &second);
    assert_eq!(second.len(), 2);
    for (index, (first, second)) in first.iter().zip(&second).enumerate() {
        assert_level_open(second, index, LEVEL_A);
        assert_ne!(first.exports[0].guid, second.exports[0].guid);
    }
    assert_eq!(lab.stats.level_actor_opens_sent, 4);
    assert_eq!(lab.stats.level_actor_opens_acked, 4);
}

/// The private field-82 capture (F-1022) records the same 612-bit payload
/// whether level visibility actors are on or off; on, the report is also
/// dispatched once.
#[test]
fn the_private_capture_is_unchanged_in_both_switch_states() {
    use super::super::super::selected_rpc_capture::PrivateRpc82Capture;
    use std::io::{Read, Seek, SeekFrom};
    let package = shaped_package(67, 20);
    let payload = rpc82(true, &package, false);
    assert_eq!(payload.bit_len(), 612);
    let mut rows = Vec::new();
    for enabled in [false, true] {
        let mut lab = if enabled {
            level_lab()
        } else {
            possession_lab(true)
        };
        let mut file = tempfile::tempfile().unwrap();
        lab.install_private_rpc82_capture(PrivateRpc82Capture::from_file(
            file.try_clone().unwrap(),
        ))
        .unwrap();
        let (socket, address, mut client, mut sender) = level_peer(&mut lab, 1, "AuthoredCapture");
        report(
            &mut lab,
            &socket,
            &mut client,
            &mut sender,
            &[(82, payload.clone())],
        );
        let counts = lab.finish_private_rpc82_capture().unwrap();
        assert_eq!(counts.observations, 1);
        assert_eq!(counts.retained_payload_bits, 612);
        assert_eq!(counts.incomplete, 0);
        file.seek(SeekFrom::Start(0)).unwrap();
        let mut text = String::new();
        file.read_to_string(&mut text).unwrap();
        let row: serde_json::Value = serde_json::from_str(text.trim()).unwrap();
        rows.push((row["payload_bits"].clone(), row["payload_hex"].clone()));
        assert_eq!(lab.stats.level_visibility_reports_added, u64::from(enabled));
        assert_eq!(
            lab.stats.level_actor_opens_sent,
            if enabled { 2 } else { 0 }
        );
        assert!(lab.has_peer(address));
    }
    assert_eq!(rows[0], rows[1]);
}

/// A big-endian connection keeps field 82 ignored with the switch on: the
/// qualified decoder reads little-endian archives only.
#[test]
fn a_big_endian_connection_keeps_field_82_ignored() {
    let mut lab = level_lab();
    lab.config.packet.codec.order = super::super::super::archive::ByteOrder::Big;
    let (socket, address, mut client, mut sender) = level_peer(&mut lab, 1, "AuthoredBig");
    assert!(matches!(
        lab.peers[&address].peer.connection_byte_order(),
        super::super::super::archive::ByteOrder::Big
    ));
    let ignored = lab.stats.controller_fields_ignored;
    let bunches = report(
        &mut lab,
        &socket,
        &mut client,
        &mut sender,
        &[(82, rpc82(true, LEVEL_A, false))],
    );
    assert!(level_opens(&lab, &bunches).is_empty());
    assert_eq!(lab.stats.controller_fields_ignored, ignored + 1);
    assert_eq!(lab.stats.level_visibility_reports_added, 0);
    assert_eq!(lab.stats.level_visibility_reports_malformed, 0);
    let entry = lab
        .controller_field_diagnostics(address)
        .unwrap()
        .entries
        .iter()
        .find(|entry| entry.index == 82)
        .cloned()
        .unwrap();
    assert_eq!(entry.disposition, ControllerFieldDisposition::Ignored);
    assert!(lab.has_peer(address));
}

/// Production can never enable the gated effect: `level_visibility_actors =
/// true` fails validation with the gated message, at configuration load and
/// at every bind, whatever the release says; `false` and absent bind with
/// the feature off, and a release's `[level_actors]` identities enable
/// nothing by themselves. The probe's path, the production bind followed by
/// the lab API, still enables it, and still refuses identities on a build
/// the visibility decoder is not qualified for.
#[test]
fn production_refuses_the_gated_switch_and_only_the_lab_api_enables_it() {
    use super::super::super::release::network_checksum;
    use crate::config::{LEVEL_VISIBILITY_ACTORS_GATED, ServerConfig};
    // Configuration load.
    let refused = toml::from_str::<ServerConfig>("[network]\nlevel_visibility_actors = true\n")
        .unwrap()
        .validate()
        .unwrap_err();
    assert_eq!(refused.to_string(), LEVEL_VISIBILITY_ACTORS_GATED);
    assert!(LEVEL_VISIBILITY_ACTORS_GATED.contains("gated"));
    assert!(LEVEL_VISIBILITY_ACTORS_GATED.contains("--level-visibility-actors true"));
    for text in [
        "[network]\nlevel_visibility_actors = false\n",
        "[network]\n",
        "",
    ] {
        let config = toml::from_str::<ServerConfig>(text).unwrap();
        assert!(config.validate().is_ok(), "{text}");
        assert!(!config.network.level_visibility_actors);
    }
    // Every bind.
    let directory = tempfile::tempdir().unwrap();
    let pin = |version: &str, build: u64, actors: bool| {
        let mut text = format!(
            "[release]\napp_id = 2089300\nbuild_id = {build}\nproject_version = \"{version}\"\nnetwork_checksum = {}\n",
            network_checksum(version)
        );
        if actors {
            text.push_str("[level_actors]\nnames = [\"AuthoredWorldSettings_0\"]\nchecksum = 1\n");
        }
        let path = directory.path().join(format!("{build}-{actors}.toml"));
        std::fs::write(&path, text).unwrap();
        path
    };
    let older = "3.0.32.158743-rel-DangerousHorizons";
    let current = "3.0.35.159586-rel-DangerousHorizons";
    let pins = [
        None,
        Some(pin(current, 26_425_113, true)),
        Some(pin(older, 25_956_137, true)),
        Some(pin(older, 25_956_137, false)),
    ];
    for release in &pins {
        let config = |level_visibility_actors| crate::config::NetworkConfig {
            release_profile: release
                .clone()
                .or_else(|| release_toml_config(false).release_profile),
            ..release_toml_config(level_visibility_actors)
        };
        let error = crate::network::bind(&config(true)).err().expect("refused");
        assert_eq!(error.to_string(), LEVEL_VISIBILITY_ACTORS_GATED);
        let lab = production_digest_lab(config(false));
        assert!(
            !lab.has_level_visibility_actors(),
            "identities enable nothing"
        );
    }
    let absent = crate::config::NetworkConfig {
        enabled: true,
        port: 0,
        release_profile: release_toml_config(false).release_profile,
        ..crate::config::NetworkConfig::default()
    };
    assert!(!production_digest_lab(absent).has_level_visibility_actors());
    // The tracked 3.0.36 pin has actor identities but no qualified decoder.
    let mut lab = production_digest_lab(release_toml_config(false));
    assert!(
        lab.enable_level_visibility_actors(&release_toml_profile())
            .is_err()
    );
    assert!(!lab.has_level_visibility_actors());
    // The probe's path still works with an explicitly qualified 3.0.35 pin.
    let mut lab = production_digest_lab(crate::config::NetworkConfig {
        release_profile: pins[1].clone(),
        ..release_toml_config(false)
    });
    lab.enable_level_visibility_actors(&ReleaseProfile::load(pins[1].as_ref().unwrap()).unwrap())
        .unwrap();
    assert_eq!(
        lab.level_visibility.as_ref().unwrap().actors.names,
        ["AuthoredWorldSettings_0"]
    );
    let mut older_lab = production_digest_lab(crate::config::NetworkConfig {
        release_profile: pins[2].clone(),
        ..release_toml_config(false)
    });
    assert!(
        older_lab
            .enable_level_visibility_actors(
                &ReleaseProfile::load(pins[2].as_ref().unwrap()).unwrap()
            )
            .is_err()
    );
    // The release's own packages are reserved, in folded form.
    let reserved = &lab.level_visibility.as_ref().unwrap().reserved_packages;
    let release = ReleaseProfile::release_3_0_35();
    for package in release.server_package_paths() {
        assert!(
            reserved.contains(&package.to_ascii_lowercase()),
            "{package}"
        );
    }
    // Configuration is fixed once a peer exists.
    let mut lab = level_lab();
    let _peer = level_peer(&mut lab, 1, "AuthoredLate");
    assert!(lab.disable_level_visibility_actors().is_err());
    assert!(
        lab.enable_level_visibility_actors(&ReleaseProfile::release_3_0_35())
            .is_err()
    );
    // Without the controller open, or with a release without identities.
    let mut bare = PeerLab::bind(0, PeerLabConfig::default()).unwrap();
    assert!(
        bare.enable_level_visibility_actors(&ReleaseProfile::release_3_0_35())
            .is_err()
    );
    let mut lab = possession_lab(true);
    assert!(
        lab.enable_level_visibility_actors(&ReleaseProfile::release_3_0_32())
            .is_err()
    );
    assert!(!lab.has_level_visibility_actors());
}

/// A connection at its level channel cap gets later opens refused for the
/// cap, which no released channel lifts: replication closes re-queue
/// nothing and count nothing; a level shown again after a hide (its record
/// kept by an admitted actor) is refused for the same cause without a
/// further count.
#[test]
fn cap_refusals_are_never_retried_by_released_channels() {
    let mut lab = level_lab();
    lab.level_visibility.as_mut().unwrap().max_level_channels = 3;
    let first = first_level_channel(&lab);
    let (socket, address, mut client, mut sender) = level_peer(&mut lab, 1, "AuthoredCap");
    let fields: Vec<_> = (0..3)
        .map(|index| (82, rpc82(true, &tile(index), false)))
        .collect();
    let bunches = report(&mut lab, &socket, &mut client, &mut sender, &fields);
    // Tile 0's pair and tile 1's first actor; the rest refused for the cap.
    assert_eq!(level_opens(&lab, &bunches).len(), 3);
    assert_eq!(lab.stats.level_actor_opens_refused, 3);
    let levels = lab.readiness_counts().level_visibility;
    assert_eq!(
        (levels.channels_held, levels.channel_budget_remaining),
        (3, 0)
    );
    assert_eq!(levels.opens_refused_waiting, 3);
    assert!(!lab.peers[&address].level_actors_refused);
    for key in 7..10 {
        let spec = replication_spec(&lab, "/Game/Authored/Replicated/AuthoredReplicated");
        assert!(lab.open_actor(1, key, spec, first, 17).is_some());
        deliver_bunches(&mut lab, &socket, &mut client, |_| true);
        assert!(lab.close_actor(1, key, 0));
        let bunches = admit(&mut lab, &socket, &mut client);
        lab.retry_level_actor_opens();
        assert!(level_opens(&lab, &bunches).is_empty());
        assert_eq!(level_slots(&lab).pending, 0, "nothing re-queued");
    }
    assert_eq!(lab.stats.level_actor_opens_requeued, 0);
    assert_eq!(lab.stats.level_actor_opens_refused, 3);
    assert_eq!(lab.stats.level_actor_opens_sent, 3);
    // Hidden and shown again: tried again, refused again for the cap, not
    // counted again.
    for visible in [false, true] {
        report(
            &mut lab,
            &socket,
            &mut client,
            &mut sender,
            &[(82, rpc82(visible, &tile(1), false))],
        );
    }
    assert_eq!(lab.stats.level_hides_actors_kept, 1);
    assert_eq!(lab.stats.level_actor_opens_refused, 3);
    assert_eq!(lab.stats.level_actor_opens_sent, 3);
    assert_eq!(level_slots(&lab).refused, 3);
    assert!(lab.has_peer(address));
}

/// The case-folded index of the static cache stays equal to the statics
/// through assignment, plain and owner-tagged export, forgetting (refused for
/// shared identities, done for owned ones, including case-variant keys that
/// fold together) and an exhaustion refusal, and answers case-insensitively.
#[test]
fn the_case_folded_static_index_stays_consistent() {
    use super::super::super::session_open::{ConnectionExports, NetGuidCache};
    let mut cache = NetGuidCache::new();
    let mut exports = ConnectionExports::new();
    let chain = |leaf: &str, package: &str| {
        vec![
            PathSegment::new(leaf, 1, true),
            PathSegment::new(package, 0, true),
        ]
    };
    let upper = chain("Leaf", "/Game/Authored/Folded");
    let lower = chain("leaf", "/game/authored/folded");
    cache.assign_static(&upper);
    assert!(cache.folded_index_consistent());
    assert!(cache.has_static_ignoring_case(&lower));
    cache.export_static_owned(&mut exports, &lower).unwrap();
    assert!(cache.folded_index_consistent());
    cache
        .export_static(&mut exports, &chain("Other", "/Game/Authored/Plain"))
        .unwrap();
    assert!(cache.folded_index_consistent());
    // Owned and untouched: forgotten; the other spelling stays found.
    assert!(cache.forget_owned_static(&lower));
    assert!(cache.folded_index_consistent());
    assert!(cache.has_static_ignoring_case(&lower));
    // The lower spelling's package key is owned until another user shares
    // it; then it is never forgotten.
    let lower_package = [PathSegment::new("/game/authored/folded", 0, true)];
    cache.export_static(&mut exports, &lower_package).unwrap();
    assert!(!cache.forget_owned_static(&lower_package), "shared since");
    assert!(cache.folded_index_consistent());
    assert!(!cache.forget_owned_static(&upper), "never owned");
    assert!(cache.folded_index_consistent());
    // An exhaustion refusal changes nothing.
    let before = cache.static_count();
    cache.set_static_index(u32::MAX >> 1);
    assert!(
        cache
            .export_static_owned(&mut exports, &chain("New", "/Game/Authored/New"))
            .is_err()
    );
    assert_eq!(cache.static_count(), before);
    assert!(cache.folded_index_consistent());
    assert!(!cache.has_static_ignoring_case(&chain("NEW", "/GAME/AUTHORED/NEW")));
}

/// One bunch carrying show, hide, show, ... of a level whose opens never get
/// admitted re-registers it at every show, each with one probe per level
/// chain of the case-folded index (never a scan), however many static
/// identities the cache holds.
#[test]
fn a_show_hide_storm_in_one_bunch_makes_bounded_lookups() {
    for fillers in [0usize, 400] {
        let mut lab = level_lab();
        for filler in 0..fillers {
            lab.guid_cache.assign_static(&[PathSegment::new(
                &format!("/Game/Authored/Filler/F{filler}"),
                0,
                false,
            )]);
        }
        let (socket, address, mut client, mut sender) = level_peer(&mut lab, 1, "AuthoredStorm");
        let shows = 8;
        let fields: Vec<_> = (0..shows)
            .flat_map(|_| {
                [
                    (82, rpc82(true, LEVEL_A, false)),
                    (82, rpc82(false, LEVEL_A, false)),
                ]
            })
            .collect();
        let before = lab.guid_cache.folded_probes();
        sender.send(&mut lab, &socket, &mut client, &fields);
        let probes = lab.guid_cache.folded_probes() - before;
        // Three level chains per registering show.
        assert_eq!(probes, 3 * shows as u64, "fillers {fillers}");
        assert_eq!(lab.stats.level_visibility_reports_added, shows as u64);
        assert_eq!(lab.stats.level_visibility_reports_hidden, shows as u64);
        assert_eq!(lab.stats.level_actor_opens_withdrawn, 2 * shows as u64);
        assert_eq!(lab.stats.level_actor_opens_sent, 0);
        assert!(lab.level_actor_holders.is_empty());
        assert!(lab.guid_cache.folded_index_consistent());
        assert!(lab.has_peer(address));
    }
}

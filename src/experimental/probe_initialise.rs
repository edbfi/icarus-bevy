//! EXPERIMENTAL synthetic player initialisation for the probe client.
//!
//! After a Join a stock client initialises its player: it calls
//! `Server_BeginPlayerInitialisation(ChrSlot)` on its PlayerController, then
//! answers the controller subobjects' requests: the BackendProxy's user and
//! character profile requests (F-951), PlayerData's `ClientPrepareLoadout` and
//! the BestiaryManager's `Client_RequestBestiaryData` (F-958).
//! [`PlayerInitialiser`] does this for the probe with authored profiles and
//! empty loadout and bestiary answers, using this repository's own decoders.
//!
//! The original assigns actor channel indices at runtime, so the controller's
//! channel is found from the server's own opens: the first complete
//! actor-channel open whose spawn archetype (for a static actor, the actor
//! itself) resolves through the connection's package-map exports to a path
//! containing [`PLAYER_CONTROLLER_ARCHETYPE`]. [`ActorChannelTracker`] reads
//! every bunch's export prefix and the spawn record's inline exports, delivers
//! reliable bunches in per-channel order from the cookie seed, and reassembles
//! partial bunches as the client does (F-537, F-709, F-710). Request
//! subobjects are recognised by their exported names, the pinned component
//! names (F-779, F-780).
//!
//! Against the native server both sides share one model, so a completed
//! initialisation shows agreement with that model only. Run against the
//! original only through the approved offline launcher.

use std::collections::BTreeMap;

use anyhow::{Context, Result, ensure};
use serde::Serialize;

use super::{
    actor_channel::{
        ActorChannelCodec, ActorChannelLimits, BlockTarget, ContentBlock, PackageMapState,
        read_actor_bunch_body, read_spawn,
    },
    archive::ByteOrder,
    control_messages::{ArchiveText, NetIdType, NetIdValue, UniqueNetId},
    packet::{Bunch, Packet, PacketConfig, Payload, Receiver, read_packet},
    probe_client::{ProbeClient, ProbeProfileResponse, ReplyDatagram, ReplyDecode},
    rep_layout::{RepCommand, RepLayoutCodec, RepLayoutLimits, RepValue, copy_bits},
    rep_rpc::{
        ADD_ASSOCIATED_PROSPECT, BACKEND_PROXY_FIELD_INDEX_BOUND,
        BESTIARY_MANAGER_FIELD_INDEX_BOUND, BP_ICARUS_PLAYER_CONTROLLER_SURVIVAL_FIELD_INDEX_BOUND,
        CLIENT_PREPARE_LOADOUT, CLIENT_REQUEST_BESTIARY_DATA, CLIENT_REQUEST_GET_CHARACTER_PROFILE,
        CLIENT_REQUEST_GET_USER_PROFILE, FieldHeader, PLAYER_DATA_FIELD_INDEX_BOUND, RpcArgument,
        RpcItem, RpcParameter, SERVER_BEGIN_PLAYER_INITIALISATION_FIELD_INDEX,
        SERVER_FRIENDS_LIST_UPDATED_FIELD_INDEX, add_associated_prospect_parameters,
        client_prepare_loadout_parameters, encode_rpc_payload, parse_rpc_payload,
        player_controller_rpc_parameters, read_field_header,
        server_friends_list_updated_parameters, server_receive_bestiary_data_parameters,
        server_receive_loadout_parameters,
    },
    sequencing::{
        MAX_CONSTRUCTED_PARTIAL_BUNCH_BYTES, PartialBunchReassembler, PartialReceive,
        ReliableChannel, ReliableReceive, SEQUENCE_MASK, reconstruct_reliable_sequence,
    },
};

/// The PlayerController class name its archetype path contains.
pub const PLAYER_CONTROLLER_ARCHETYPE: &str = "BP_IcarusPlayerControllerSurvival";
/// Payload bits accepted per content block when observing the server.
const OBSERVED_PAYLOAD_BITS: usize = 1 << 22;
/// Note keys are cut to this many characters before counting.
const NOTE_CHARS: usize = 64;

/// The probe's one login identity: a `NULL`-typed unique net id carrying
/// `player_id` as text. A text id is an archive string, not encoded hex, so
/// the server's id string (`ToString`, F-945) is `player_id` exactly; the
/// original welcomes this form (F-858).
pub fn player_unique_net_id(player_id: &str) -> Result<UniqueNetId> {
    ensure!(!player_id.is_empty(), "the player id must not be empty");
    Ok(UniqueNetId {
        kind: NetIdType::Named(ArchiveText::text("NULL")?),
        value: NetIdValue::Text(ArchiveText::text(player_id)?),
    })
}

/// Authored `FResGetUserProfile` items after Success, deliberately unrelated
/// to login or save identities.
pub fn authored_user_profile() -> Result<Vec<RpcItem>> {
    let text = |value: &str| -> Result<RpcItem> {
        Ok(RpcItem::Value(RepValue::Str(ArchiveText::text(value)?)))
    };
    let int = |value| RpcItem::Value(RepValue::Int(value));
    Ok(vec![
        text("authored-profile-user")?,
        RpcItem::Array(vec![vec![text("Ore")?, int(17)], vec![text("")?, int(-8)]]),
        RpcItem::Array(vec![vec![int(42)], vec![int(-1)]]),
        RpcItem::Array(vec![vec![text("Talent")?, int(3)]]),
        int(8),
        int(19),
    ])
}

/// Authored `FResGetCharacterProfile` items after Success; the second, the
/// profile's `ChrSlot`, is `slot`.
pub fn authored_character_profile(slot: i32) -> Result<Vec<RpcItem>> {
    let text = |value: &str| -> Result<RpcItem> {
        Ok(RpcItem::Value(RepValue::Str(ArchiveText::text(value)?)))
    };
    let int = |value| RpcItem::Value(RepValue::Int(value));
    let float = |value: f32| RpcItem::Value(RepValue::Float(value.to_bits()));
    let boolean = |value| RpcItem::Value(RepValue::Bool(value));
    let mut items = vec![
        text("authored-character")?,
        int(slot),
        int(41),
        int(-99),
        boolean(true),
        boolean(false),
        text("authored-prospect")?,
        RpcItem::Value(RepValue::Enum(1)),
        RpcItem::Array(vec![vec![int(9)], vec![int(-7)]]),
        RpcItem::Array(vec![vec![text("Skill")?, int(13)]]),
    ];
    items.extend((-10..0).map(int));
    items.push(boolean(true));
    items.extend((21..26).map(int));
    items.extend([boolean(false), int(27)]);
    items.extend([1.5_f32, -3.25, 0.0].map(float));
    items.extend((31..38).map(int));
    items.extend([0.25_f32, -0.0, 100.5, 1.0].map(float));
    items.extend([
        RpcItem::Array(vec![
            vec![text("Effect")?, int(-12)],
            vec![text("Other")?, int(6)],
        ]),
        RpcItem::Value(RepValue::Int64(-9_876_543_210)),
    ]);
    Ok(items)
}

/// One non-bool parameter sent as the engine's RPC sender would: a value equal
/// to its zero default goes as a clear presence bit (`ChrSlot` 0 is not sent).
fn int_argument(value: i32) -> RpcArgument {
    if value == 0 {
        RpcArgument::NotSent
    } else {
        RpcArgument::Sent(vec![RpcItem::Value(RepValue::Int(value))])
    }
}

/// `Server_BeginPlayerInitialisation(ChrSlot)` as one controller field.
pub fn begin_player_initialisation_field(
    chr_slot: i32,
    codec: &RepLayoutCodec,
) -> Result<FieldHeader> {
    let parameters =
        player_controller_rpc_parameters(SERVER_BEGIN_PLAYER_INITIALISATION_FIELD_INDEX)
            .context("Server_BeginPlayerInitialisation layout")?;
    Ok(FieldHeader {
        index: SERVER_BEGIN_PLAYER_INITIALISATION_FIELD_INDEX,
        payload: encode_rpc_payload(&parameters, &[int_argument(chr_slot)], codec)?,
    })
}

/// `ServerFriendsListUpdated` with no friends: the empty array equals its
/// zero default, so it goes as a clear presence bit.
pub fn friends_list_updated_field(codec: &RepLayoutCodec) -> Result<FieldHeader> {
    Ok(FieldHeader {
        index: SERVER_FRIENDS_LIST_UPDATED_FIELD_INDEX,
        payload: encode_rpc_payload(
            &server_friends_list_updated_parameters(),
            &[RpcArgument::NotSent],
            codec,
        )?,
    })
}

/// `ServerReceiveLoadout` with its one struct parameter not sent: the server
/// keeps a default-constructed `FPlayerLoadoutData`. Its members are not
/// modelled ([`server_receive_loadout_parameters`]).
pub fn loadout_response_payload(codec: &RepLayoutCodec) -> Result<Payload> {
    encode_rpc_payload(
        &server_receive_loadout_parameters(),
        &[RpcArgument::NotSent],
        codec,
    )
}

/// `Server_ReceiveBestiaryData` with both arrays not sent, as the engine's RPC
/// sender writes an empty array (identical to its zero default); the server's
/// parameters hold two empty arrays.
pub fn bestiary_response_payload(codec: &RepLayoutCodec) -> Result<Payload> {
    encode_rpc_payload(
        &server_receive_bestiary_data_parameters(),
        &[RpcArgument::NotSent, RpcArgument::NotSent],
        codec,
    )
}

/// The actor codec for reading and writing this exchange's bunches.
pub fn initialise_codec(engine_net_version: u32) -> RepLayoutCodec {
    RepLayoutCodec {
        actor: ActorChannelCodec {
            engine_net_version,
            exports_enabled: false,
            has_level: true,
            order: ByteOrder::Little,
            limits: ActorChannelLimits {
                max_payload_bits: OBSERVED_PAYLOAD_BITS,
                ..ActorChannelLimits::default()
            },
        },
        checksums: false,
        limits: RepLayoutLimits::default(),
    }
}

/// One complete server actor-channel bunch: parts joined, export and
/// must-be-mapped prefixes consumed.
#[derive(Clone, Debug)]
pub struct ActorBunch {
    pub channel_index: u32,
    pub open: bool,
    pub close: bool,
    pub reliable: bool,
    pub parts: usize,
    pub payload: Payload,
}

struct TrackedChannel {
    /// Unwrapped reliable sequence and bunch, in delivery order.
    reliable: ReliableChannel<(i64, Bunch)>,
    partial: PartialBunchReassembler,
}

/// One connection's server actor bunches, in the client's delivery order.
pub struct ActorChannelTracker {
    packet: PacketConfig,
    codec: ActorChannelCodec,
    initial_in_reliable: u32,
    /// Last 14-bit packet sequence and its unwrapped ID.
    last_packet: Option<(u16, i64)>,
    channels: BTreeMap<u32, TrackedChannel>,
    package_map: PackageMapState,
}

impl ActorChannelTracker {
    /// `packet` frames server datagrams (its receiver is the client);
    /// `initial_in_reliable` is the connection's incoming reliable seed.
    pub fn new(packet: PacketConfig, codec: ActorChannelCodec, initial_in_reliable: u32) -> Self {
        Self {
            packet,
            codec,
            initial_in_reliable,
            last_packet: None,
            channels: BTreeMap::new(),
            package_map: PackageMapState::new(),
        }
    }

    pub fn package_map(&self) -> &PackageMapState {
        &self.package_map
    }

    /// Read one datagram the probe accepted; a stale or duplicate packet must
    /// not be fed. Bunch failures become `notes`.
    pub fn feed(&mut self, bytes: &[u8], notes: &mut Vec<String>) -> Result<Vec<ActorBunch>> {
        let packet = read_packet(bytes, &self.packet)?;
        Ok(self.feed_packet(packet, notes))
    }

    /// As [`feed`](Self::feed), for a packet already read. Control-channel
    /// bunches are left to the probe client.
    pub fn feed_packet(&mut self, packet: Packet, notes: &mut Vec<String>) -> Vec<ActorBunch> {
        // Unreliable partial parts join on the same or the next packet ID.
        let sequence = packet.header.sequence;
        let packet_id = self.last_packet.map_or(i64::from(sequence), |(last, id)| {
            id + i64::from(sequence.wrapping_sub(last) & SEQUENCE_MASK)
        });
        self.last_packet = Some((sequence, packet_id));
        let mut complete = Vec::new();
        for bunch in packet.bunches {
            let index = bunch.header.channel_index;
            if index == 0 {
                continue;
            }
            let initial = self.initial_in_reliable;
            let channel = self
                .channels
                .entry(index)
                .or_insert_with(|| TrackedChannel {
                    reliable: ReliableChannel::new(initial, 0),
                    partial: PartialBunchReassembler::new(),
                });
            let arrivals = match bunch.header.reliable_sequence_raw {
                Some(raw) if bunch.header.reliable => {
                    let ordered =
                        reconstruct_reliable_sequence(raw, channel.reliable.in_reliable())
                            .and_then(|sequence| {
                                channel.reliable.receive_reliable(raw, (sequence, bunch))
                            });
                    match ordered {
                        Ok(ReliableReceive::Delivered(batch)) => batch,
                        Ok(ReliableReceive::Queued) => Vec::new(),
                        Ok(ReliableReceive::Duplicate) => {
                            notes.push(format!("channel {index}: duplicate reliable bunch"));
                            Vec::new()
                        }
                        Err(error) => {
                            notes.push(format!("channel {index}: {error:#}"));
                            Vec::new()
                        }
                    }
                }
                _ => vec![(packet_id, bunch)],
            };
            for (sequence, bunch) in arrivals {
                match self.next_bunch(sequence, bunch) {
                    Ok(Some(bunch)) => complete.push(bunch),
                    Ok(None) => {}
                    Err(error) => notes.push(format!("channel {index}: {error:#}")),
                }
            }
        }
        complete
    }

    /// The export prefix, the partial merge, then the must-be-mapped prefix,
    /// in the client's `ReceivedRawBunch`, `ReceivedNextBunch` and
    /// `UActorChannel::ReceivedBunch` order (F-709, F-710).
    fn next_bunch(&mut self, sequence: i64, bunch: Bunch) -> Result<Option<ActorBunch>> {
        let index = bunch.header.channel_index;
        let mut reader = bunch.payload.reader();
        if bunch.header.has_package_map_exports {
            self.package_map
                .consume_export_prefix(&mut reader, &self.codec)
                .context("export prefix")?;
        }
        let bits = reader.remaining();
        let rest = copy_bits(&mut reader, bits);
        let channel = self.channels.get_mut(&index).context("untracked channel")?;
        let (header, payload, parts) = if bunch.header.partial {
            match channel.partial.receive_part(
                sequence,
                &bunch.header,
                &rest,
                Some(MAX_CONSTRUCTED_PARTIAL_BUNCH_BYTES),
            )? {
                PartialReceive::Complete { bunch, parts, .. } => {
                    (bunch.header, bunch.payload, parts)
                }
                _ => return Ok(None),
            }
        } else {
            (bunch.header, rest, 1)
        };
        if header.close {
            channel.partial.clear();
        }
        let payload = if header.has_must_be_mapped_guids {
            let mut reader = payload.reader();
            self.package_map
                .consume_must_be_mapped(&mut reader, &self.codec)
                .context("must-be-mapped prefix")?;
            let bits = reader.remaining();
            copy_bits(&mut reader, bits)
        } else {
            payload
        };
        Ok(Some(ActorBunch {
            channel_index: index,
            open: header.open,
            close: header.close,
            reliable: header.reliable,
            parts,
            payload,
        }))
    }

    /// The archetype path an open's spawn record resolves to: its archetype,
    /// or for a static actor the actor itself, through the exports seen so far
    /// and the record's own inline exports. Only the spawn record is read.
    pub fn open_archetype(&mut self, bunch: &ActorBunch) -> Result<Option<String>> {
        ensure!(bunch.open, "not an open bunch");
        let spawn = read_spawn(&mut bunch.payload.reader(), &self.codec)?;
        self.package_map.ingest_guid_ref(&spawn.actor);
        let guid = match &spawn.dynamic {
            Some(dynamic) => {
                self.package_map.ingest_guid_ref(&dynamic.archetype);
                dynamic.archetype.guid
            }
            None => spawn.actor.guid,
        };
        Ok(self.package_map.resolve_full_path(guid))
    }
}

/// What the probe begins with and answers.
#[derive(Clone, Copy, Debug)]
pub struct InitialiseConfig {
    /// `Server_BeginPlayerInitialisation`'s slot and the character profile's.
    pub chr_slot: i32,
    pub engine_net_version: u32,
}

/// One initialisation step. Carries slots, channel, paths and packet IDs only.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum InitialiseEvent {
    ControllerChannel {
        channel: u32,
        archetype: String,
    },
    /// `slot_sent` is false when ChrSlot 0 went as a clear presence bit.
    BeginSent {
        channel: u32,
        chr_slot: i32,
        slot_sent: bool,
        packet_id: u32,
    },
    /// An empty `ServerFriendsListUpdated`, sent right after Begin.
    FriendsListSent {
        channel: u32,
        packet_id: u32,
    },
    ProfileRequest {
        profile: &'static str,
        requested_slot: Option<i32>,
    },
    ProfileAnswered {
        profile: &'static str,
        chr_slot: Option<i32>,
        packet_id: u32,
    },
    LoadoutAnswered {
        requested_slot: Option<i32>,
        packet_id: u32,
    },
    BestiaryAnswered {
        packet_id: u32,
    },
    AssociatedProspect {
        requested_slot: Option<i32>,
    },
}

/// Counts of the steps taken so far.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct InitialiseSummary {
    pub controller_channel: Option<u32>,
    pub begin_sent: bool,
    pub friends_list_sent: bool,
    pub user_profiles_answered: u32,
    pub character_profiles_answered: u32,
    pub loadouts_answered: u32,
    pub bestiaries_answered: u32,
    pub associated_prospects: u32,
}

/// The controller subobjects the exchange addresses, by exported name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Component {
    BackendProxy,
    PlayerData,
    Bestiary,
}

impl Component {
    const ALL: [Self; 3] = [Self::BackendProxy, Self::PlayerData, Self::Bestiary];

    fn name(self) -> &'static str {
        match self {
            Self::BackendProxy => "BackendProxyComponent",
            Self::PlayerData => "PlayerDataComponent",
            Self::Bestiary => "BestiaryManagerComponent",
        }
    }

    fn index_bound(self) -> u32 {
        match self {
            Self::BackendProxy => BACKEND_PROXY_FIELD_INDEX_BOUND,
            Self::PlayerData => PLAYER_DATA_FIELD_INDEX_BOUND,
            Self::Bestiary => BESTIARY_MANAGER_FIELD_INDEX_BOUND,
        }
    }

    fn from_name(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|component| component.name().eq_ignore_ascii_case(name))
    }
}

/// The probe's side of player initialisation over one joined connection.
pub struct PlayerInitialiser {
    config: InitialiseConfig,
    codec: RepLayoutCodec,
    tracker: ActorChannelTracker,
    summary: InitialiseSummary,
    notes: BTreeMap<String, u64>,
}

impl PlayerInitialiser {
    /// Track `client`'s connection from its cookie seed; feed it every reply
    /// from the Join onwards.
    pub fn new(client: &ProbeClient, config: InitialiseConfig) -> Self {
        let codec = initialise_codec(config.engine_net_version);
        let packet = PacketConfig {
            receiver: Receiver::Client,
            ..client.config().packet
        };
        Self {
            config,
            tracker: ActorChannelTracker::new(packet, codec.actor, client.initial_reliable().0),
            codec,
            summary: InitialiseSummary::default(),
            notes: BTreeMap::new(),
        }
    }

    pub fn summary(&self) -> &InitialiseSummary {
        &self.summary
    }

    /// Unexpected or undecodable input, counted by kind.
    pub fn notes(&self) -> &BTreeMap<String, u64> {
        &self.notes
    }

    /// Begun, and every request the native exchange needs answered once.
    pub fn is_complete(&self) -> bool {
        let summary = &self.summary;
        summary.begin_sent
            && summary.user_profiles_answered > 0
            && summary.character_profiles_answered > 0
            && summary.loadouts_answered > 0
            && summary.bestiaries_answered > 0
    }

    /// Process one reply and send what it calls for. Only datagrams the probe
    /// accepted are read; decoding failures become notes, send failures
    /// errors.
    pub fn on_reply(
        &mut self,
        client: &mut ProbeClient,
        reply: &ReplyDatagram,
    ) -> Result<Vec<InitialiseEvent>> {
        let mut events = Vec::new();
        if !matches!(reply.decode, ReplyDecode::Decoded { .. }) {
            return Ok(events);
        }
        let mut notes = Vec::new();
        let bunches = self
            .tracker
            .feed(&reply.bytes, &mut notes)
            .unwrap_or_else(|error| {
                notes.push(format!("packet: {error:#}"));
                Vec::new()
            });
        for bunch in &bunches {
            self.on_bunch(client, bunch, &mut events, &mut notes)?;
        }
        for note in notes {
            *self
                .notes
                .entry(note.chars().take(NOTE_CHARS).collect())
                .or_default() += 1;
        }
        Ok(events)
    }

    fn on_bunch(
        &mut self,
        client: &mut ProbeClient,
        bunch: &ActorBunch,
        events: &mut Vec<InitialiseEvent>,
        notes: &mut Vec<String>,
    ) -> Result<()> {
        let channel = bunch.channel_index;
        if bunch.open && self.summary.controller_channel.is_none() {
            match self.tracker.open_archetype(bunch) {
                Ok(Some(archetype)) if archetype.contains(PLAYER_CONTROLLER_ARCHETYPE) => {
                    self.summary.controller_channel = Some(channel);
                    events.push(InitialiseEvent::ControllerChannel { channel, archetype });
                    let field =
                        begin_player_initialisation_field(self.config.chr_slot, &self.codec)?;
                    let (_, packet_id) = client.send_actor_rpc(
                        channel,
                        &field,
                        BP_ICARUS_PLAYER_CONTROLLER_SURVIVAL_FIELD_INDEX_BOUND,
                        &self.codec.actor,
                    )?;
                    self.summary.begin_sent = true;
                    events.push(InitialiseEvent::BeginSent {
                        channel,
                        chr_slot: self.config.chr_slot,
                        slot_sent: self.config.chr_slot != 0,
                        packet_id,
                    });
                    let field = friends_list_updated_field(&self.codec)?;
                    let (_, packet_id) = client.send_actor_rpc(
                        channel,
                        &field,
                        BP_ICARUS_PLAYER_CONTROLLER_SURVIVAL_FIELD_INDEX_BOUND,
                        &self.codec.actor,
                    )?;
                    self.summary.friends_list_sent = true;
                    events.push(InitialiseEvent::FriendsListSent { channel, packet_id });
                }
                Ok(Some(_)) => {}
                Ok(None) => notes.push("open: unresolved archetype".to_owned()),
                Err(error) => notes.push(format!("open: {error:#}")),
            }
            return Ok(());
        }
        if bunch.open || self.summary.controller_channel != Some(channel) {
            return Ok(());
        }
        if bunch.close {
            notes.push("controller channel closed".to_owned());
        }
        let body =
            match read_actor_bunch_body(&mut bunch.payload.reader(), false, &self.codec.actor) {
                Ok(body) => body,
                Err(error) => {
                    notes.push(format!("controller bunch: {error:#}"));
                    return Ok(());
                }
            };
        for block in &body.blocks {
            self.on_block(client, channel, block, events, notes)?;
        }
        Ok(())
    }

    fn on_block(
        &mut self,
        client: &mut ProbeClient,
        channel: u32,
        block: &ContentBlock,
        events: &mut Vec<InitialiseEvent>,
        notes: &mut Vec<String>,
    ) -> Result<()> {
        let (target, bound) = match &block.target {
            BlockTarget::Actor => (None, BP_ICARUS_PLAYER_CONTROLLER_SURVIVAL_FIELD_INDEX_BOUND),
            BlockTarget::SubObject { object, .. } => {
                let name = self.tracker.package_map().resolve_path(object.guid);
                let Some(component) = name.and_then(Component::from_name) else {
                    notes.push(format!("subobject {}", name.unwrap_or("unresolved")));
                    return Ok(());
                };
                (Some((component, object.guid)), component.index_bound())
            }
        };
        let label = target.map_or("controller", |(component, _)| component.name());
        if block.has_rep_layout {
            notes.push(format!("{label} properties"));
            return Ok(());
        }
        let mut reader = block.payload.reader();
        while !reader.at_end() {
            let field = match read_field_header(
                &mut reader,
                bound,
                self.codec.actor.limits.max_payload_bits,
            ) {
                Ok(field) => field,
                Err(error) => {
                    notes.push(format!("{label} field: {error:#}"));
                    return Ok(());
                }
            };
            let Some((component, guid)) = target else {
                notes.push(format!("{label} field {}", field.index));
                continue;
            };
            match (component, field.index) {
                (Component::BackendProxy, CLIENT_REQUEST_GET_USER_PROFILE) => {
                    events.push(InitialiseEvent::ProfileRequest {
                        profile: "user",
                        requested_slot: None,
                    });
                    let (_, packet_id) = client.send_profile_response(
                        channel,
                        guid,
                        ProbeProfileResponse {
                            character: false,
                            success: true,
                            profile: authored_user_profile()?,
                        },
                        &self.codec,
                    )?;
                    self.summary.user_profiles_answered += 1;
                    events.push(InitialiseEvent::ProfileAnswered {
                        profile: "user",
                        chr_slot: None,
                        packet_id,
                    });
                }
                (Component::BackendProxy, CLIENT_REQUEST_GET_CHARACTER_PROFILE) => {
                    let requested_slot =
                        self.slot_parameter(&field.payload, &[int_parameter()], notes);
                    events.push(InitialiseEvent::ProfileRequest {
                        profile: "character",
                        requested_slot,
                    });
                    let (_, packet_id) = client.send_profile_response(
                        channel,
                        guid,
                        ProbeProfileResponse {
                            character: true,
                            success: true,
                            profile: authored_character_profile(self.config.chr_slot)?,
                        },
                        &self.codec,
                    )?;
                    self.summary.character_profiles_answered += 1;
                    events.push(InitialiseEvent::ProfileAnswered {
                        profile: "character",
                        chr_slot: Some(self.config.chr_slot),
                        packet_id,
                    });
                }
                (Component::PlayerData, CLIENT_PREPARE_LOADOUT) => {
                    let requested_slot = self.slot_parameter(
                        &field.payload,
                        &client_prepare_loadout_parameters(),
                        notes,
                    );
                    let (_, packet_id) = client.send_loadout_response(
                        channel,
                        guid,
                        loadout_response_payload(&self.codec)?,
                        &self.codec.actor,
                    )?;
                    self.summary.loadouts_answered += 1;
                    events.push(InitialiseEvent::LoadoutAnswered {
                        requested_slot,
                        packet_id,
                    });
                }
                (Component::PlayerData, ADD_ASSOCIATED_PROSPECT) => {
                    let requested_slot = self.slot_parameter(
                        &field.payload,
                        &add_associated_prospect_parameters(),
                        notes,
                    );
                    self.summary.associated_prospects += 1;
                    events.push(InitialiseEvent::AssociatedProspect { requested_slot });
                }
                (Component::Bestiary, CLIENT_REQUEST_BESTIARY_DATA) => {
                    let (_, packet_id) = client.send_bestiary_response(
                        channel,
                        guid,
                        bestiary_response_payload(&self.codec)?,
                        &self.codec.actor,
                    )?;
                    self.summary.bestiaries_answered += 1;
                    events.push(InitialiseEvent::BestiaryAnswered { packet_id });
                }
                (component, index) => notes.push(format!("{} field {index}", component.name())),
            }
        }
        Ok(())
    }

    /// The leading `ChrSlot` of a request whose parameters are `parameters`;
    /// a clear presence bit is slot 0. Undecodable parameters become a note.
    fn slot_parameter(
        &self,
        payload: &Payload,
        parameters: &[RpcParameter],
        notes: &mut Vec<String>,
    ) -> Option<i32> {
        match parse_rpc_payload(payload, parameters, &self.codec)
            .as_deref()
            .map(<[RpcArgument]>::first)
        {
            Ok(Some(RpcArgument::NotSent)) => Some(0),
            Ok(Some(RpcArgument::Sent(items))) => match items.as_slice() {
                [RpcItem::Value(RepValue::Int(slot))] => Some(*slot),
                _ => None,
            },
            Ok(_) => None,
            Err(error) => {
                notes.push(format!("request parameters: {error:#}"));
                None
            }
        }
    }
}

fn int_parameter() -> RpcParameter {
    RpcParameter::Value(vec![RepCommand::Int])
}

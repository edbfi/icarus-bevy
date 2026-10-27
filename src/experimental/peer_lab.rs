//! EXPERIMENTAL loopback Bevy lab with a bounded per-peer packet table.
//!
//! Extends the stateless handshake lab (`handshake_lab`, same lab cookie policy,
//! not the original algorithm) with one [`LabPeer`] per IPv4 loopback address:
//! a peer is created (or replaced) when the lab sends it a handshake
//! acknowledgement, its later data datagrams pass through [`LabPeer::receive`]
//! and every accepted packet is answered with [`LabPeer::ack_packet`]. When
//! [`PeerLabConfig::admission`] is `None` (default), no admission, identity
//! check, Welcome, channel other than control, or gameplay runs; delivered
//! control messages are counted by kind and then discarded, except that Hello
//! and Login get the mapped control replies below. When [`PeerLabConfig::admission`]
//! is `Some`, each peer also enforces [`ConnectionLoginAdmission`]
//! (`IsClientMsgTypeValid`), `PreLogin`, `Welcome`, `Netspeed`, `Join` /
//! `SplitJoin` (`SpawnPlayActor` / `InitNewPlayer`), `PCSwap`, and `DebugText`
//! checks from `experimental::admission` (`F-712`, `F-713`, `F-740`).
//!
//! The server side is an INFERRED mirror of client-build observations (cookie
//! seeding F-536, sequencing F-535..F-539, framing F-521..F-527); the original
//! server's peer table, timeouts, ack cadence and limits were not observed.
//! Nothing here demonstrates compatibility with any client.
//!
//! Control replies follow the client and server builds' `NotifyControlMessage`
//! (`F-530`, `F-531`, `F-579`, `F-712`, `F-713`, `F-740`) and are sent as
//! reliable control bunches through [`LabPeer::reliable_control_packet`] (each
//! also carries the ack state):
//! - When [`PeerLabConfig::admission`] is `Some`, any delivered control message
//!   rejected by [`ConnectionLoginAdmission::is_client_msg_type_valid`] is
//!   counted in [`PeerLabStats::invalid_state_messages`] and ignored without
//!   advancing state (`F-713`).
//! - Hello, when [`PeerLabConfig::local_network_version`] is set and differs
//!   from the Hello version: Upgrade (ID 2) carrying the configured version,
//!   then the peer is removed (`peers_closed`). The comparison is plain
//!   equality (`F-527`, `F-713`; `U-037` resolved). `None` skips the comparison.
//! - Otherwise a Hello whose token is empty (declared count, terminator
//!   included, below two, as the server and client compare it, F-563): Challenge
//!   (ID 3) carrying 32 lowercase hex digits of fresh OS randomness, advancing
//!   [`ConnectionLoginAdmission::on_challenge_sent`]. The original challenge is
//!   counter-derived (F-531); this string is a lab choice, is not retained and
//!   is never claimed to match.
//! - A Hello with a nonempty token (the encryption path) is counted in
//!   `hello_unanswered` and not answered: unsupported.
//! - Login (ID 5): counted by unique-net-id form only. When
//!   [`PeerLabConfig::admission`] is `None`, Failure (ID 6) carrying
//!   [`ADMISSION_DISABLED_MESSAGE`] is sent and the peer is removed. When
//!   [`PeerLabConfig::admission`] is `Some`, [`AdmissionPolicy::pre_login`] runs
//!   against the current welcomed/joined occupancy: on `Ok(())`, Welcome (ID 1)
//!   carrying the configured map/game/redirect URLs is sent and the peer
//!   advances to `Welcomed` (`welcomes_sent`); on `Err(reason)`, Failure (ID 6)
//!   carrying `reason` is sent and the peer is removed.
//! - Netspeed (ID 4), when [`PeerLabConfig::admission`] is `Some`: clamps the
//!   requested rate with [`clamp_client_netspeed`] against
//!   [`PeerLabAdmission::max_client_rate`], stores `CurrentNetSpeed`, and
//!   increments [`PeerLabStats::netspeeds_updated`] (`F-740`).
//! - Join (ID 9) and SplitJoin (ID 10), when [`PeerLabConfig::admission`] is
//!   `Some`: `Join` checks [`ConnectionLoginAdmission::on_join_spawned`]
//!   (ignoring duplicate `Join` in `duplicate_joins_ignored`) and runs
//!   [`AdmissionPolicy::login_and_init_new_player`], counting `joins_succeeded`
//!   or sending Failure (ID 6) and closing the peer. With
//!   [`PeerLab::enable_player_controller_open`], a successful `Join` (not
//!   `SplitJoin`) is answered with the composed PlayerController open
//!   ([`compose_player_controller_open`], F-778, F-779) through
//!   [`LabPeer::reliable_actor_open_datagrams`]: GUIDs come from one
//!   driver-wide cache, and each connection's exports are committed to the
//!   packet carrying them and repeated until it is acked, its acks and naks
//!   applied in packet order (F-796; `player_controller_opens_sent`,
//!   `player_controller_open_errors`). With
//!   [`PeerLab::enable_game_state_open`] the world's GameState follows
//!   ([`compose_game_state_open`], F-794) through
//!   [`LabPeer::reliable_actor_open_datagrams`], one actor and driver-wide
//!   component GUIDs for every connection (`game_state_opens_sent`,
//!   `game_state_open_errors`); with [`PeerLab::enable_player_state_open`]
//!   the player's PlayerState comes last (F-784). Once a connection's
//!   GameState open is acked, [`PeerLab::poll`] sends it unreliable
//!   GameState property updates at most every [`GAME_STATE_UPDATE_SECONDS`]
//!   (the capture's cadence, F-805): `TimeOfDay` when it changed (with
//!   [`PeerLab::enable_time_of_day`], advanced by [`TimeOfDayClock`],
//!   F-806), the world time every [`WORLD_TIME_UPDATE_SECONDS`] and
//!   `LevelTimeElapsedSec` when its whole second changed, `ServerFPS`
//!   when it changed (with [`PeerLab::enable_server_fps`], measured over
//!   the lab's own poll intervals by [`ServerFpsMeter`]), and
//!   `LockedMissions` when a minute's refresh changed it (with
//!   [`PeerLab::enable_locked_missions`], F-834). Explicit
//!   [`PeerLab::refresh_active_prospect`] calls supply complete prospect
//!   values; polling and Login never infer a prospect refresh. With
//!   [`PeerLab::enable_member_refresh`], a Join whose login ID names an
//!   associated member refreshes it as the original's spawn completion does
//!   (F-822, F-945). An
//!   update over one bunch (a long `NetworkingStatus`) goes out through
//!   [`LabPeer::actor_update_datagrams`] as unreliable partial bunches in
//!   consecutive datagrams (`game_state_update_datagrams`). A naked update's
//!   values (a nak of any of its datagrams) that were still the latest are
//!   sent again (`game_state_updates_lost`).
//! - Swap (ID 15, `NMT_PCSwap`) and Debug (ID 17, `NMT_DebugText`), when
//!   [`PeerLabConfig::admission`] is `Some`: `Swap` evaluates
//!   [`evaluate_pc_swap_target`] against the peer's splitscreen child count
//!   (`swaps_received` vs `invalid_swaps_ignored`), and `Debug` counts
//!   [`PeerLabStats::debug_texts_received`] without retaining text (`F-713`, `F-740`).
//! - `NMT_ActorChannelFailure` (ID 16, a fallback entry), gated like other
//!   messages: on the PlayerController's channel the peer gets the control
//!   close (reason 0) and is removed (`peers_closed_by_channel_failure`);
//!   other channels only count (`actor_channel_failures`), as the base
//!   `NotifyActorChannelFailure` is a no-op (F-740). Other fallback entries
//!   count in `fallbacks_ignored`.
//! - Messages after a closing reply in the same delivery are not processed.
//! - Whole reliable actor bunches on this peer's opened PlayerController are
//!   read as content blocks and fields. F-951 / controller net fields: field
//!   124 (bound 183) receives `Server_BeginPlayerInitialisation(ChrSlot)` once
//!   per session. Missing PlayerState, slot -1 and repeats count and do nothing;
//!   the original failure wire is unknown. Other fields/subobjects are skipped.
//!   Malformed actor payloads are dropped after consuming their reliable sequence.
//!
//! - Login, Join and SplitJoin after [`PeerLab::stop_admission`]: Failure
//!   (ID 6) carrying [`SHUTTING_DOWN_MESSAGE`], then the peer is removed
//!   (`refused_draining`, `peers_closed`).
//!
//! Removing a peer is the lab's close: outside [`PeerLab::shutdown`] no close
//! bunch is written, so a lost Upgrade or Failure is never resent. An accepted
//! data packet carrying a control bunch with the close flag removes its peer
//! after that packet's delivered messages are answered
//! (`peers_closed_by_client`); this is a lab choice, applied even when the
//! close bunch itself waits in the reliable queue. Naked reliable replies of
//! live peers are resent only inside later ack or reply datagrams (see
//! `packet_lab`), which the lab writes only after accepting a client packet;
//! there is no timer.
//!
//! Policy (lab choices):
//! - At most [`PeerLabConfig::max_peers`] peers. When the table is full, a
//!   valid challenge echo from a new address is rejected: no acknowledgement is
//!   sent and `peers_rejected` counts it. Nothing is ever evicted to make room;
//!   a known address that completes a new handshake replaces its own peer; a
//!   repeated echo of its current cookie is only re-acknowledged.
//! - Peers idle longer than [`PeerLabConfig::idle_timeout`] (no accepted data
//!   packet since creation or the last accepted one) are removed at the start of
//!   an update; deadlines use monotonic real time ([`Instant`]), never
//!   simulation time. Failed peers ([`LabPeer::is_failed`]) and peers whose
//!   ack packet cannot be built are removed at once.
//! - After [`PeerLab::stop_admission`] no exact handshake form is answered
//!   (`refused_draining`), so no peer is created or replaced; live peers keep
//!   their packet state until [`PeerLab::shutdown`] notifies and removes them
//!   all (`peers_shutdown`). Both calls are idempotent.
//! - [`PeerLab::shutdown`] mirrors `UNetDriver::Shutdown` (F-782): every peer
//!   whose control channel is not closing gets a reliable Failure (ID 6)
//!   carrying [`HOST_CLOSED_CONNECTION_MESSAGE`] in its own datagram; then each
//!   peer that has delivered a reliable control bunch gets the control
//!   channel's reliable close bunch (reason 0, `Destroyed`) in another. Nothing
//!   is resent afterwards: the peers are removed at once.
//! - At most [`PACKETS_PER_UPDATE`] datagrams per update. The receive buffer is
//!   one byte longer than the configured maximum datagram size
//!   (`limits.max_packet_bytes`), so a longer, kernel-truncated datagram is
//!   detected and rejected as oversize.
//! - At most one ack packet per peer per update, sent after the receive loop
//!   when that peer had at least one accepted data packet. Stale or duplicate
//!   packets are not acknowledged.
//!
//! Stats and `Debug` carry counts and message kinds only: never payload bytes,
//! cookies, tokens, challenge strings, URLs, unique net ids, versions, sequence
//! values or numeric message fields.
use std::{
    collections::{BTreeMap, HashMap},
    fmt, io,
    net::{SocketAddr, SocketAddrV4, UdpSocket},
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail, ensure};
use bevy::{
    app::{RunFixedMainLoop, RunFixedMainLoopSystems},
    prelude::{App, IntoScheduleConfigs, Plugin, ResMut, Resource, resource_exists},
};
use serde::Serialize;

use super::actor_channel::{
    ActorBunchBody, ActorChannelCodec, ActorChannelLimits, BlockTarget, encode_actor_bunch_body,
    read_actor_bunch_body,
};
use super::actor_open::{Block, Reference};
use super::admission::{
    ActorChannelFailureAction, AdmissionPolicy, ClientLoginState, ConnectionLoginAdmission,
    DEFAULT_MAX_CLIENT_RATE, FIND_PLAYER_START_ERROR, JoinSpawnContext, Occupancy,
    clamp_client_netspeed, evaluate_actor_channel_failure, evaluate_pc_swap_target, int_option,
};
use super::control_messages::{
    ArchiveText, ControlFallback, ControlMessage, Decoded, NetIdValue, UniqueNetId,
};
use super::game_state_open::{
    GameStateOpenPlan, GameStateOpenState, LockedMission, NetworkingStatus,
    active_prospect_update_values, class_default, compose_game_state_open,
    compose_game_state_update,
};
use super::handshake::decode;
use super::handshake_lab::{
    CookieAuthority, HANDSHAKE_BYTES, HandshakeReply, PACKETS_PER_UPDATE, bind_loopback,
    lab_seconds,
};
use super::manager_open::{ManagerOpenPlan, ManagerSlot, compose_manager_open};
use super::packet::BunchLimits;
use super::packet::Payload;
use super::packet_lab::{
    ActorOpenSent, DeliveredSummary, LabPeer, PacketDisposition, PacketLabConfig,
};
use super::pawn_open::{
    PawnContent, PawnOpenPlan, PawnOpenState, compose_pawn_icarus_uid_update, compose_pawn_open,
};
use super::ping::PingEstimator;
use super::quest_open::{QuestOpenBasePlan, QuestOpenPlan, compose_quest_open};
use super::release::ReleaseProfile;
use super::rep_layout::{RepLayoutCodec, RepValue};
use super::rep_rpc::{
    BP_ICARUS_PLAYER_CONTROLLER_SURVIVAL_FIELD_INDEX_BOUND,
    SERVER_BEGIN_PLAYER_INITIALISATION_FIELD_INDEX, parse_begin_player_initialisation,
    read_field_header,
};
use super::server_fps::ServerFpsMeter;
use super::session_open::{
    ActorKey, ActorOpenSpec, ConnectionExports, NetGuidCache, PathSegment,
    PlayerControllerOpenPlan, PlayerStateOpenPlan, PlayerStateOpenState, actor_codec,
    actor_properties_update_block, authored_movement, compose_actor_open,
    compose_player_controller_open, compose_player_state_open, initial_actor_properties,
    replicated_unique_net_id,
};
use super::time_of_day::TimeOfDayClock;

/// Lab text of the Failure reply to Login; not an original message.
pub const ADMISSION_DISABLED_MESSAGE: &str = "icarus-bevy peer lab: game admission is disabled";
/// Source text of `NSLOCTEXT("NetworkErrors", "HostClosedConnection", ...)`,
/// the Failure every client receives when the server shuts down (F-782).
pub const HOST_CLOSED_CONNECTION_MESSAGE: &str = "Host closed the connection.";
/// `EChannelCloseReason::Destroyed`, the reason of the shutdown close bunch.
pub const CLOSE_REASON_DESTROYED: u32 = 0;
/// `AGameSession::GetNextPlayerID`'s first value (`NextPlayerID` in `.data`,
/// post-incremented by `RegisterPlayer`; F-784).
pub const FIRST_PLAYER_ID: i32 = 256;
/// A login unique net ID's string form, as a member's `UserID` names it
/// (F-945): encoded IDs render as uppercase hexadecimal (a Steam ID's
/// decimal digits unchanged), text IDs as their text, absent IDs as empty.
fn login_user_id(id: &super::control_messages::UniqueNetId) -> Vec<u16> {
    use super::control_messages::NetIdValue;
    match &id.value {
        NetIdValue::Text(text) => text.units().to_vec(),
        value => value.hex().unwrap_or_default().encode_utf16().collect(),
    }
}

/// UTF-16 IDs equal ignoring ASCII case.
fn same_user_id(left: &[u16], right: &[u16]) -> bool {
    let fold = |unit: u16| match u8::try_from(unit) {
        Ok(byte) => u16::from(byte.to_ascii_lowercase()),
        Err(_) => unit,
    };
    left.len() == right.len()
        && left
            .iter()
            .zip(right)
            .all(|(&left, &right)| fold(left) == fold(right))
}

/// Lab text of the Failure reply to Login or Join after admission stopped;
/// not an original message.
pub const SHUTTING_DOWN_MESSAGE: &str = "icarus-bevy peer lab: server is shutting down";
/// Hello tokens whose declared count, including the terminator, is below this
/// get a Challenge: only an empty token does (server and client, F-563).
pub const CHALLENGE_TOKEN_THRESHOLD: usize = 2;
/// Random bytes behind the lab challenge string (twice as many hex digits).
pub const LAB_CHALLENGE_BYTES: usize = 16;

/// Lab choice: default peer-table capacity.
pub const DEFAULT_MAX_PEERS: usize = 16;
/// Upper bound accepted for [`PeerLabConfig::max_peers`].
pub const MAX_PEERS_LIMIT: usize = 1024;
/// Lab choice: default idle timeout; the original timeout was not observed.
pub const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_secs(30);
/// Lab choice: default maximum datagram size; the negotiated value is unknown.
pub const DEFAULT_MAX_DATAGRAM_BYTES: u32 = 1024;
/// Largest UDP payload over IPv4.
pub const MAX_UDP_PAYLOAD_BYTES: u32 = 65_507;
/// Lab choice: default channel-index limit for bunch parsing and actor-channel
/// failure checks (UE4's `UNetConnection::MAX_CHANNELS` is `10240`, while
/// `M-707` opens quest and manager channels up to `126`, `F-858`, `F-934`;
/// `256` leaves room above channels `0..=8` for multi-stage `C019` quest DAGs
/// and the saved managers without allocating `10240`-entry tables).
pub const DEFAULT_MAX_CHANNELS: u32 = 256;
/// Lab choice: default archive-string unit cap for channel names.
pub const DEFAULT_MAX_STRING_UNITS: usize = 256;

/// Opt-in login-state gating (`IsClientMsgTypeValid`), `PreLogin`, `Welcome`,
/// `Netspeed`, and `Join` / `SplitJoin` (`SpawnPlayActor`) policy for [`PeerLab`].
#[derive(Clone, Copy)]
pub struct PeerLabAdmission {
    pub policy: AdmissionPolicy,
    pub max_client_rate: i32,
    pub default_subsystem: Option<&'static str>,
    pub compatible_subsystems: &'static [&'static str],
    pub indexed_subsystems: &'static [(u8, &'static str)],
    pub map_url: &'static str,
    pub game_name: &'static str,
    pub redirect_url: &'static str,
    pub default_player_name: &'static str,
    pub has_game_session: bool,
    pub can_spawn_controller: bool,
    pub has_player_state: bool,
    pub has_player_start: bool,
    pub must_spectate: bool,
}

impl fmt::Debug for PeerLabAdmission {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PeerLabAdmission")
            .field("policy", &self.policy)
            .field("default_subsystem_set", &self.default_subsystem.is_some())
            .field(
                "compatible_subsystems_len",
                &self.compatible_subsystems.len(),
            )
            .field("indexed_subsystems_len", &self.indexed_subsystems.len())
            .field("has_game_session", &self.has_game_session)
            .field("can_spawn_controller", &self.can_spawn_controller)
            .field("has_player_state", &self.has_player_state)
            .field("has_player_start", &self.has_player_start)
            .field("must_spectate", &self.must_spectate)
            .finish()
    }
}

impl Default for PeerLabAdmission {
    fn default() -> Self {
        Self {
            policy: AdmissionPolicy {
                max_players: 8,
                max_spectators: 2,
                max_splitscreens_per_connection: 4,
            },
            max_client_rate: DEFAULT_MAX_CLIENT_RATE,
            default_subsystem: Some("NULL"),
            compatible_subsystems: &["STEAM"],
            indexed_subsystems: &[(1, "NULL"), (3, "STEAM")],
            map_url: "/Game/Maps/Conifer_Terrain_004",
            game_name: "/Game/BP/Framework/BP_IcarusGameModeSurvival.BP_IcarusGameModeSurvival_C",
            redirect_url: "",
            default_player_name: "Player",
            has_game_session: true,
            can_spawn_controller: true,
            has_player_state: true,
            has_player_start: true,
            must_spectate: false,
        }
    }
}

/// Caller-supplied lab parameters.
#[derive(Clone, Copy)]
pub struct PeerLabConfig {
    /// Per-peer packet state; `packet.limits.max_packet_bytes` is also the
    /// maximum accepted datagram size and sizes the receive buffer.
    pub packet: PacketLabConfig,
    /// Peer-table capacity, `1..=MAX_PEERS_LIMIT`.
    pub max_peers: usize,
    /// Nonzero idle interval after which a peer is removed.
    pub idle_timeout: Duration,
    /// Caller-supplied local network version compared with each Hello version
    /// (plain equality); a mismatch is answered with Upgrade. `None` skips the
    /// comparison. `Debug` shows only whether it is set.
    pub local_network_version: Option<u32>,
    /// Optional login-state gating (`IsClientMsgTypeValid`), `PreLogin`,
    /// `Welcome`, and `Join` / `SplitJoin` admission policy (`F-712`, `F-713`).
    /// `None` disables game admission and answers `Login` with [`ADMISSION_DISABLED_MESSAGE`].
    pub admission: Option<PeerLabAdmission>,
}

impl fmt::Debug for PeerLabConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PeerLabConfig")
            .field("packet", &self.packet)
            .field("max_peers", &self.max_peers)
            .field("idle_timeout", &self.idle_timeout)
            .field(
                "local_network_version_set",
                &self.local_network_version.is_some(),
            )
            .field("admission_enabled", &self.admission.is_some())
            .finish()
    }
}

impl Default for PeerLabConfig {
    fn default() -> Self {
        Self {
            packet: PacketLabConfig::new(BunchLimits {
                max_channels: DEFAULT_MAX_CHANNELS,
                max_packet_bytes: DEFAULT_MAX_DATAGRAM_BYTES,
                max_string_units: DEFAULT_MAX_STRING_UNITS,
            }),
            max_peers: DEFAULT_MAX_PEERS,
            idle_timeout: DEFAULT_IDLE_TIMEOUT,
            local_network_version: None,
            admission: None,
        }
    }
}

impl PeerLabConfig {
    /// Bind the peer lab's Hello network-version comparison to `profile`.
    pub fn with_release_profile(mut self, profile: &ReleaseProfile) -> Self {
        self.local_network_version = Some(profile.pin.network_checksum);
        self
    }

    /// Maximum accepted datagram size in bytes.
    pub fn max_datagram_bytes(&self) -> usize {
        self.packet.packet.limits.max_packet_bytes as usize
    }

    fn validate(&self) -> Result<()> {
        ensure!(
            (1..=MAX_PEERS_LIMIT).contains(&self.max_peers),
            "max_peers must be 1..={MAX_PEERS_LIMIT}"
        );
        ensure!(!self.idle_timeout.is_zero(), "idle timeout must be nonzero");
        let max = self.packet.packet.limits.max_packet_bytes;
        ensure!(
            (HANDSHAKE_BYTES as u32..=MAX_UDP_PAYLOAD_BYTES).contains(&max),
            "maximum datagram size must be {HANDSHAKE_BYTES}..={MAX_UDP_PAYLOAD_BYTES} bytes"
        );
        if let Some(admission) = &self.admission {
            ArchiveText::text(admission.map_url).context("invalid admission map_url")?;
            ArchiveText::text(admission.game_name).context("invalid admission game_name")?;
            ArchiveText::text(admission.redirect_url).context("invalid admission redirect_url")?;
        }
        self.packet
            .validate()
            .context("invalid packet-lab configuration")
    }
}

/// Counters only; no addresses, payloads, cookies or field values.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct PeerLabStats {
    /// Datagrams taken from the socket, including rejected ones.
    pub received: u64,
    /// Longer than the maximum datagram size (possibly kernel-truncated).
    pub oversize: u64,
    /// Non-IPv4 sources.
    pub non_ipv4: u64,
    pub io_errors: u64,
    pub challenges_sent: u64,
    /// Handshake acknowledgements sent; each created or replaced a peer.
    pub acknowledgements_sent: u64,
    /// Exact 29-byte handshake forms that were not answered (forged, expired,
    /// restart or acknowledgement forms), from known or unknown addresses.
    pub handshake_rejected: u64,
    pub peers_created: u64,
    pub peers_replaced: u64,
    /// Repeated echoes of a live peer's own cookie, re-acknowledged without
    /// resetting its packet state.
    pub acknowledgements_resent: u64,
    /// Valid challenge echoes refused because the table was full.
    pub peers_rejected: u64,
    pub peers_expired: u64,
    /// Peers removed after a failed receive or ack.
    pub peers_failed: u64,
    /// Non-handshake datagrams from addresses without a peer.
    pub unknown_peer: u64,
    /// Data packets that passed the sequence gate.
    pub data_accepted: u64,
    /// Stale or duplicate data packets (state unchanged).
    pub data_rejected: u64,
    /// Datagrams from a known peer that failed to parse or decode (including
    /// non-short handshake forms), or that failed the peer.
    pub data_malformed: u64,
    pub acks_sent: u64,
    /// Ack packets that could not be built; the peer is removed.
    pub ack_errors: u64,
    /// Control messages delivered in order, of any kind.
    pub control_delivered: u64,
    /// Delivered entries whose ID has no known layout.
    pub control_unknown: u64,
    /// Delivered control messages per message ID (kind), known or not.
    pub control_by_id: BTreeMap<u8, u64>,
    /// Accepted data packets carrying a control bunch with the close flag;
    /// each removes its peer (`peers_closed_by_client`).
    pub control_close_seen: u64,
    /// Delivered control messages rejected by `IsClientMsgTypeValid` (`F-713`).
    pub invalid_state_messages: u64,
    /// Challenge (ID 3) replies sent to Hello.
    pub control_challenges_sent: u64,
    /// Upgrade (ID 2) replies sent to a mismatched Hello version.
    pub upgrades_sent: u64,
    /// Hello messages with a token of two or more units (encryption path):
    /// counted, not answered.
    pub hello_unanswered: u64,
    /// Login messages delivered and parsed.
    pub logins_received: u64,
    /// Delivered Login messages per unique-net-id form (`encoded_empty`,
    /// `encoded`, `text` or `absent`); kinds only.
    pub login_net_id_forms: BTreeMap<&'static str, u64>,
    /// Failure (ID 6) replies sent to Login or Join.
    pub failures_sent: u64,
    /// Welcome (ID 1) replies sent to accepted Logins.
    pub welcomes_sent: u64,
    /// Netspeed (ID 4) requests applied to `CurrentNetSpeed` (`F-740`).
    pub netspeeds_updated: u64,
    /// Join / SplitJoin requests that passed `SpawnPlayActor` / `InitNewPlayer`.
    pub joins_succeeded: u64,
    /// Duplicate Join requests ignored because a PlayerController already exists (`F-713`).
    pub duplicate_joins_ignored: u64,
    /// PCSwap (ID 15) requests whose child index resolved to parent or a live child (`F-713`, `F-740`).
    pub swaps_received: u64,
    /// PCSwap (ID 15) requests ignored because `ChildIndex >= Children.Num()` (`F-713`, `F-740`).
    pub invalid_swaps_ignored: u64,
    /// DebugText (ID 17) messages received in valid login state (`F-713`, `F-740`).
    pub debug_texts_received: u64,
    /// Peers removed after an Upgrade or Failure reply.
    pub peers_closed: u64,
    /// Peers removed after an accepted packet carrying a control close bunch.
    pub peers_closed_by_client: u64,
    /// Peers removed by [`PeerLab::shutdown`].
    pub peers_shutdown: u64,
    /// Handshake forms ignored and Login / Join / SplitJoin refused after
    /// [`PeerLab::stop_admission`].
    pub refused_draining: u64,
    /// Replies that could not be built (the peer is kept unless it failed).
    pub reply_errors: u64,
    /// PlayerController opens sent after Join (opt-in, module docs).
    pub player_controller_opens_sent: u64,
    /// PlayerController opens that could not be composed or framed.
    pub player_controller_open_errors: u64,
    /// Actor bunches on unopened/closing channels or outside this peer's controller.
    pub actor_bunches_ignored: u64,
    /// Unsupported actor bunch forms or reliable queue overflow.
    pub actor_bunches_dropped: u64,
    /// Malformed controller content blocks, field headers or RPC parameters.
    pub actor_bunches_malformed: u64,
    pub actor_duplicate_reliable: u64,
    /// Unrecognized fields skipped by their declared payload length.
    pub controller_fields_ignored: u64,
    pub initialisation_slots_recorded: u64,
    /// Slot -1, missing PlayerState or a repeated call; no failure wire is sent.
    pub initialisation_calls_ignored: u64,
    /// Returning pawns given their saved UID (F-950).
    pub returning_pawns_restored: u64,
    /// Recorded slots with no free matching saved character.
    pub returning_pawns_unmatched: u64,
    /// Matches whose saved UID was unusable or whose update failed.
    pub returning_pawn_errors: u64,
    /// Pawn opens sent after the PlayerController open (F-797).
    pub pawn_opens_sent: u64,
    /// Pawn opens that could not be composed or framed.
    pub pawn_open_errors: u64,
    /// GameState opens sent after the PlayerController open (F-794).
    pub game_state_opens_sent: u64,
    /// GameState opens that could not be composed or framed.
    pub game_state_open_errors: u64,
    /// GameState property updates sent after its open was acked (F-805).
    pub game_state_updates_sent: u64,
    /// Datagrams sent carrying GameState updates: one per update unless it
    /// was split into partial bunches over several.
    pub game_state_update_datagrams: u64,
    /// Active prospect refreshes after a member's Join (F-822, F-945).
    pub member_refreshes: u64,
    /// Joins whose login ID names no associated member: the original adds
    /// such a player as a member and saves, which is not modelled, so no
    /// refresh follows.
    pub member_refreshes_unassociated: u64,
    /// Member refreshes that could not be validated or composed.
    pub member_refresh_errors: u64,
    /// GameState updates that could not be composed or framed.
    pub game_state_update_errors: u64,
    /// Naked GameState updates whose values were still the latest and are
    /// due again.
    pub game_state_updates_lost: u64,
    /// PlayerState opens sent after the PlayerController open (F-784).
    pub player_state_opens_sent: u64,
    /// PlayerState opens that could not be composed or framed.
    pub player_state_open_errors: u64,
    /// World manager opens sent after the PlayerState open (F-859).
    pub manager_opens_sent: u64,
    /// World manager opens that could not be composed or framed.
    pub manager_open_errors: u64,
    /// Saved quest actor opens sent with the world managers (`F-934`).
    pub quest_opens_sent: u64,
    /// Saved quest actor opens that could not be composed or framed.
    pub quest_open_errors: u64,
    /// `NMT_ActorChannelFailure` entries delivered in a valid login state.
    pub actor_channel_failures: u64,
    /// Peers closed because their PlayerController channel failed.
    pub peers_closed_by_channel_failure: u64,
    /// Control close bunches sent to those peers.
    pub channel_failure_closes_sent: u64,
    /// Other fallback entries (no action in the lab).
    pub fallbacks_ignored: u64,
    /// Replicated actor opens sent through [`PeerLab::open_actor`].
    pub actor_opens_sent: u64,
    /// Replicated actor open attempts that could not be composed or framed;
    /// a persistent failure is retried, and counted, every time the actor is
    /// selected again.
    pub actor_open_errors: u64,
    /// Replicated actor channel closes sent through [`PeerLab::close_actor`].
    pub actor_closes_sent: u64,
    /// Replicated actor channel closes that could not be framed.
    pub actor_close_errors: u64,
    /// Actor property updates sent through [`PeerLab::update_actor_movement`],
    /// [`PeerLab::update_actor_motion`] or [`PeerLab::update_actor_properties`].
    pub actor_updates_sent: u64,
    /// Actor property updates that could not be composed or framed.
    pub actor_update_errors: u64,
    /// Naks of in-flight actor property updates, superseded ones included.
    pub actor_update_naks: u64,
    /// Naked actor property updates with still-current fields that are due again.
    pub actor_updates_lost: u64,
    /// Failure notices sent by [`PeerLab::shutdown`] (F-782).
    pub shutdown_failures_sent: u64,
    /// Control-channel close bunches sent by [`PeerLab::shutdown`] (F-782).
    pub shutdown_closes_sent: u64,
}

impl PeerLabStats {
    fn count_delivered(&mut self, delivered: &[DeliveredSummary]) {
        for entry in delivered {
            self.control_delivered = self.control_delivered.saturating_add(1);
            if matches!(entry, DeliveredSummary::Unknown { .. }) {
                self.control_unknown = self.control_unknown.saturating_add(1);
            }
            let count = self.control_by_id.entry(entry.id()).or_default();
            *count = count.saturating_add(1);
        }
    }
}

/// Session gauges and counters for status records; counts only, never
/// addresses, identifiers or message fields.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct SessionCounts {
    /// False after [`PeerLab::stop_admission`].
    pub admitting: bool,
    /// Peers currently in the table, in any login state.
    pub active_peers: usize,
    /// Largest peer-table size observed since bind.
    pub peak_peers: usize,
    /// Peers currently welcomed and not yet joined.
    pub welcomed: usize,
    /// Peers currently joined.
    pub joined: usize,
    /// Welcome replies sent since bind.
    pub welcomes_sent: u64,
    /// Join / SplitJoin requests that passed admission since bind.
    pub joins_succeeded: u64,
    /// Failure replies to Login or Join (capacity, identity, spawn or
    /// shutdown refusals).
    pub admissions_rejected: u64,
    /// Valid handshakes refused because the peer table was full.
    pub peers_rejected: u64,
    /// Handshakes ignored and logins/joins refused after admission stopped.
    pub refused_draining: u64,
    /// Sessions removed by the idle timeout.
    pub closed_by_timeout: u64,
    /// Sessions closed by a client control close bunch.
    pub closed_by_client: u64,
    /// Sessions closed after an Upgrade or Failure reply.
    pub closed_by_reply: u64,
    /// Upgrade replies to a Hello whose network version differed from the
    /// configured one (each also counts in `closed_by_reply`).
    pub upgrades_sent: u64,
    /// Sessions removed after a receive, reply or ack failure.
    pub closed_failed: u64,
    /// Sessions removed by shutdown.
    pub closed_by_shutdown: u64,
    /// PlayerController opens sent after Join.
    pub player_controller_opens_sent: u64,
    pub initialisation_slots_recorded: u64,
    pub initialisation_calls_ignored: u64,
    pub returning_pawns_restored: u64,
    /// Pawn, GameState, PlayerState, world manager and saved quest opens sent
    /// after Join.
    pub pawn_opens_sent: u64,
    pub game_state_opens_sent: u64,
    pub player_state_opens_sent: u64,
    pub manager_opens_sent: u64,
    pub quest_opens_sent: u64,
    /// Actor opens (session and replicated) that failed to compose or
    /// frame; a datagram the socket failed to send counts in `io_errors`.
    pub open_errors: u64,
    /// Socket send or receive failures.
    pub io_errors: u64,
    /// Shutdown Failure notices sent (F-782).
    pub shutdown_failures_sent: u64,
    /// Shutdown control-channel close bunches sent (F-782).
    pub shutdown_closes_sent: u64,
    /// Control messages rejected by `IsClientMsgTypeValid` (`F-713`).
    pub invalid_state_messages: u64,
}

/// The session key of connection `connection`'s pawn: session keys from
/// `1 << 32` up (below that, the caller's, such as the GameState's).
pub fn pawn_key(connection: u32) -> ActorKey {
    ActorKey::Session((1 << 32) | u64::from(connection))
}

/// `NotifyBunchCommit` for a sent open: its exports went out in the packet
/// carrying the last export part (F-796).
fn commit_exports(exports: &mut ConnectionExports, sent: &ActorOpenSent) {
    match sent.exports_packet_id {
        Some(packet_id) => exports.commit_bunch(packet_id),
        None => exports.begin_bunch(),
    }
}

fn bump(counter: &mut u64) {
    *counter = counter.saturating_add(1);
}

/// A control reply chosen for one delivered message.
enum Reply {
    Challenge,
    Upgrade(u32),
    Welcome {
        spectator: bool,
    },
    Failure(&'static str),
    /// Not a control message: the PlayerController open on its actor channel.
    OpenPlayerController,
}

impl Reply {
    /// Upgrade and Failure end the peer (the client closes after them, F-530).
    fn closes(&self) -> bool {
        matches!(self, Self::Upgrade(_) | Self::Failure(_))
    }
}

/// 32 lowercase hex digits of fresh OS randomness; a lab choice, not the
/// original counter-derived challenge (F-531).
fn lab_challenge() -> Result<ArchiveText> {
    let mut bytes = [0; LAB_CHALLENGE_BYTES];
    getrandom::fill(&mut bytes)
        .map_err(|error| anyhow::anyhow!("challenge randomness: {error}"))?;
    let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    ArchiveText::text(&hex)
}

fn net_id_form(value: &NetIdValue) -> &'static str {
    match value {
        NetIdValue::EncodedEmpty => "encoded_empty",
        NetIdValue::Encoded { .. } => "encoded",
        NetIdValue::Text(_) => "text",
        NetIdValue::Absent { .. } => "absent",
    }
}

/// The shortest interval between a connection's GameState updates: the
/// M-707 server's 30 updates per second (F-805).
pub const GAME_STATE_UPDATE_SECONDS: f32 = 1.0 / 30.0;

/// How often the world time (`ReplicatedWorldTimeSeconds`) is sent: every
/// third GameState update in the capture, UE's 0.1 s default (F-805).
pub const WORLD_TIME_UPDATE_SECONDS: f32 = 0.1;

/// `LockedMissions` as last refreshed at `world_seconds` (`Some(None)`
/// when every lock has ended), or `None` when it is not replicated.
fn locked_missions(session: &GameStateSession, world_seconds: f32) -> Option<Option<RepValue>> {
    let restored = session.level_time?;
    if session.locked_missions.is_empty() {
        return None;
    }
    let now = super::game_state_open::locked_missions_refreshed_at(restored, world_seconds);
    Some(super::game_state_open::locked_missions_value(
        &session.locked_missions,
        now,
    ))
}

/// The GameState handles updates carry: the world time, `TimeOfDay`,
/// `LevelTimeElapsedSec`, `ServerFPS`, `LockedMissions` and the
/// `NetworkingStatus` fields and explicitly refreshed active prospect.
fn update_handles() -> impl Iterator<Item = u32> {
    use super::game_state_open::handle;
    [
        handle::WORLD_TIME_SECONDS,
        handle::TIME_OF_DAY,
        handle::LEVEL_TIME_ELAPSED_SEC,
        handle::SERVER_FPS,
        handle::LOCKED_MISSIONS,
    ]
    .into_iter()
    .chain(handle::ACTIVE_PROSPECT)
    .chain(handle::NETWORKING_STATUS)
}

/// What one connection last received of the GameState's changing values.
#[derive(Default)]
struct GameStateReplica {
    /// Each update handle's last sent value; a missing handle is due.
    sent: BTreeMap<u32, RepValue>,
    /// World time of the last sent world time.
    world_time_at: f32,
    /// Updates awaiting the client's ack or nak: each packet that carried a
    /// part -> the update (its last packet ID) and its handles. A split
    /// update arrives only whole (`ReceivedNextBunch` drops a partial bunch
    /// with a gap), so a nak of any part loses it and acks of all clear it.
    in_flight: BTreeMap<u32, (u32, Vec<u32>)>,
    /// Each handle's latest update (its last packet ID); only its nak
    /// repairs.
    last_packet: BTreeMap<u32, u32>,
}

impl GameStateReplica {
    /// What is due at `world_seconds`: the world time when it is missing or
    /// [`WORLD_TIME_UPDATE_SECONDS`] old, and every `changing` value that
    /// differs from the one sent.
    fn due(
        &self,
        world_seconds: f32,
        changing: &[(u32, Option<RepValue>)],
    ) -> BTreeMap<u32, RepValue> {
        use super::game_state_open::handle::WORLD_TIME_SECONDS;
        let mut values = BTreeMap::new();
        if !self.sent.contains_key(&WORLD_TIME_SECONDS)
            || world_seconds - self.world_time_at >= WORLD_TIME_UPDATE_SECONDS
        {
            values.insert(WORLD_TIME_SECONDS, RepValue::Float(world_seconds.to_bits()));
        }
        for (handle, value) in changing {
            if let Some(value) = value
                && self.sent.get(handle) != Some(value)
            {
                values.insert(*handle, value.clone());
            }
        }
        values
    }

    /// Record `values` as sent in the packets `packet_ids` (one update, in
    /// send order) at `world_seconds`.
    fn sent(&mut self, packet_ids: &[u32], values: BTreeMap<u32, RepValue>, world_seconds: f32) {
        use super::game_state_open::handle::WORLD_TIME_SECONDS;
        let Some(&update) = packet_ids.last() else {
            return;
        };
        if values.contains_key(&WORLD_TIME_SECONDS) {
            self.world_time_at = world_seconds;
        }
        let handles: Vec<u32> = values.keys().copied().collect();
        for (handle, value) in values {
            self.sent.insert(handle, value);
            self.last_packet.insert(handle, update);
        }
        for packet_id in packet_ids {
            self.in_flight.insert(*packet_id, (update, handles.clone()));
        }
        // Only a handle's latest update repairs: at most one per handle stays.
        let latest = &self.last_packet;
        self.in_flight
            .retain(|_, (update, _)| latest.values().any(|packet| packet == update));
    }

    fn acked(&mut self, packet_id: u32) {
        self.in_flight.remove(&packet_id);
    }

    /// The client lost `packet_id`, so the whole update that it carried a
    /// part of: its values that no newer update carried are due again.
    /// Returns whether any was.
    fn naked(&mut self, packet_id: u32) -> bool {
        let Some((update, handles)) = self.in_flight.remove(&packet_id) else {
            return false;
        };
        self.in_flight.retain(|_, (other, _)| *other != update);
        let mut lost = false;
        for handle in handles {
            if self.last_packet.get(&handle) == Some(&update) {
                self.last_packet.remove(&handle);
                self.sent.remove(&handle);
                lost = true;
            }
        }
        lost
    }
}

/// Small, fixed-prefix actor state. Missing values are unknown/due. Packet
/// records live only while a sent-ring notification can still arrive. Each
/// field has its own latest logical update (the group's final packet ID).
#[derive(Default)]
struct ActorReplica {
    sent: BTreeMap<u32, RepValue>,
    flags_managed: bool,
    last_packet: BTreeMap<u32, u32>,
}

struct ActorPropertyPacket {
    key: u64,
    update: u32,
    handles: Vec<u32>,
}

struct PeerEntry {
    peer: LabPeer,
    /// Owned until this connection is removed, including idle/failure paths.
    pawn_uid: Option<i32>,
    /// The saved character this pawn restored (F-950); its UID stays claimed
    /// for the saved record when the session ends.
    restored_character: Option<usize>,
    /// The cookie that created this peer; compared only, never reported.
    cookie: [u8; 20],
    last_seen: Instant,
    ack_pending: bool,
    admission: ConnectionLoginAdmission,
    login_url_units: Vec<u16>,
    spectator: bool,
    splitscreen_count: u8,
    current_net_speed: Option<i32>,
    /// Static GUIDs this connection has received with their paths.
    exports: ConnectionExports,
    /// The Login's unique net id; used only to replicate the PlayerState,
    /// never reported.
    login_unique_id: Option<UniqueNetId>,
    /// The primary player registered at Join (PlayerId and name).
    player: Option<RegisteredPlayer>,
    /// Lab connection ID assigned at Join (replication viewers use it).
    connection_id: Option<u32>,
    /// Session actor opens actually committed for this peer, never inferred
    /// from a channel index another actor might occupy.
    player_controller_channel: Option<u32>,
    player_state_channel: Option<u32>,
    /// F-951 / controller net fields: the first accepted ChrSlot this session.
    character_slot: Option<i32>,
    /// Replicated actors this connection opened, by caller key.
    opened_actors: BTreeMap<u64, u32>,
    /// Closes that could not be framed yet (key -> reason), retried by
    /// [`PeerLab::retry_pending_closes`]; their actors stay open meanwhile.
    pending_closes: BTreeMap<u64, u32>,
    actor_properties: BTreeMap<u64, ActorReplica>,
    /// All parts of one logical update share its handles and identity. ACKs
    /// retire individual parts; any NAK repairs still-current fields only.
    /// Superseded records remain for NAK counting, bounded by the sent ring:
    /// every removed ring entry notifies ACK/NAK; actor close and peer teardown
    /// also discard their records.
    actor_property_packets: BTreeMap<u32, ActorPropertyPacket>,
    /// The GameState's updated values, once its open was sent.
    game_state: Option<GameStateReplica>,
    /// Ping from the round trips of acked packets (F-815).
    ping: PingEstimator,
}

/// A player registered by `AGameSession::RegisterPlayer` (F-784): its
/// `PlayerId` and initial name.
struct RegisteredPlayer {
    player_id: i32,
    name_units: Vec<u16>,
}

/// Stage a complete controller bunch before applying any calls: malformed
/// trailing input must not partially mutate session state. Subobject payloads
/// are never interpreted as calls on the controller itself.
fn controller_initialisation_calls(
    payload: &Payload,
    engine_net_version: u32,
    order: super::archive::ByteOrder,
) -> Result<(Vec<i32>, usize)> {
    let actor = actor_codec(engine_net_version, order);
    let body = read_actor_bunch_body(&mut payload.reader(), false, &actor)?;
    let codec = RepLayoutCodec {
        actor,
        checksums: false,
        limits: Default::default(),
    };
    let mut slots = Vec::new();
    let mut ignored = 0;
    for block in body.blocks {
        if block.target != BlockTarget::Actor {
            continue;
        }
        ensure!(
            !block.has_rep_layout,
            "client controller properties unsupported"
        );
        let mut reader = block.payload.reader();
        while !reader.at_end() {
            // F-951 / controller net fields: the pinned class uses bound 183.
            let field = read_field_header(
                &mut reader,
                BP_ICARUS_PLAYER_CONTROLLER_SURVIVAL_FIELD_INDEX_BOUND,
                actor.limits.max_payload_bits,
            )?;
            if field.index == SERVER_BEGIN_PLAYER_INITIALISATION_FIELD_INDEX {
                slots.push(parse_begin_player_initialisation(&field.payload, &codec)?);
            } else {
                ignored += 1;
            }
        }
    }
    Ok((slots, ignored))
}

/// The GameState every joined connection is opened with (F-794).
#[derive(Clone, Debug)]
struct GameStateSession {
    plan: GameStateOpenPlan,
    state: GameStateOpenState,
    /// Complete values supplied by the last explicit refresh, including
    /// defaults. None leaves legacy template-only initialization unchanged.
    active_prospect: Option<BTreeMap<u32, RepValue>>,
    /// The prospect that a member's Join refreshes, with its
    /// `IsCurrentlyPlaying` flags recomputed ([`PeerLab::enable_member_refresh`]).
    member_refresh: Option<super::prospect_info::ProspectInfo>,
    channel_index: u32,
    /// The restored `LevelTimeElapsedSec`, advanced with the world time at
    /// each send.
    level_time: Option<i32>,
    /// The missions `LockedMissions` lists while their locks last, at the
    /// level time of its last refresh (F-834); empty when none.
    locked_missions: Vec<LockedMission>,
    /// The world's time of day, advanced to the instant `clock_at`.
    clock: Option<TimeOfDayClock>,
    clock_at: Instant,
    /// `ServerFPS` over the poll intervals, the last poll at `polled_at`
    /// (an instant, so intervals keep their resolution on long runs).
    fps: Option<ServerFpsMeter>,
    polled_at: Instant,
    /// The last GameState update pass.
    updated_at: Option<Instant>,
    /// `NetworkingStatus` as last gathered: the enabled base with the joined
    /// players listed at each update pass (F-810).
    networking: Option<NetworkingStatus>,
}

impl GameStateSession {
    /// Assemble the template with the dynamic values an open would send now.
    fn open_state(
        &self,
        template: &GameStateOpenState,
        world_seconds: f32,
    ) -> Result<GameStateOpenState> {
        use super::game_state_open::handle;
        let mut state = template.clone();
        if let Some(status) = &self.networking {
            // As gathered at the last update pass; defaults stay unsent.
            for (handle, value) in status.values()? {
                if class_default(handle).as_ref() == Some(&value) {
                    state.values.remove(&handle);
                } else {
                    state.values.insert(handle, value);
                }
            }
        }
        if let Some(meter) = &self.fps {
            match meter.fps() {
                0 => state.values.remove(&handle::SERVER_FPS),
                fps => state.values.insert(handle::SERVER_FPS, RepValue::Int(fps)),
            };
        }
        if let Some(clock) = &self.clock {
            let time_of_day = clock.time_of_day();
            if time_of_day == 0.0 {
                state.values.remove(&handle::TIME_OF_DAY);
            } else {
                state
                    .values
                    .insert(handle::TIME_OF_DAY, RepValue::Float(time_of_day.to_bits()));
            }
        }
        state.values.insert(
            handle::WORLD_TIME_SECONDS,
            RepValue::Float(world_seconds.to_bits()),
        );
        if let Some(restored) = self.level_time {
            state.values.insert(
                handle::LEVEL_TIME_ELAPSED_SEC,
                RepValue::Int(super::game_state_open::level_time_elapsed_sec(
                    restored,
                    world_seconds,
                )),
            );
        }
        if let Some(value) = locked_missions(self, world_seconds) {
            // An empty list is the default and unsent.
            match value {
                Some(value) => state.values.insert(handle::LOCKED_MISSIONS, value),
                None => state.values.remove(&handle::LOCKED_MISSIONS),
            };
        }
        Ok(state)
    }
}

/// The loopback peer lab. `Debug` reports only the peer count and counters.
#[derive(Resource)]
pub struct PeerLab {
    socket: UdpSocket,
    authority: CookieAuthority,
    started: Instant,
    config: PeerLabConfig,
    peers: HashMap<SocketAddrV4, PeerEntry>,
    buffer: Vec<u8>,
    stats: PeerLabStats,
    admitting: bool,
    peak_peers: usize,
    /// Opt-in PlayerController open after Join, and the driver-wide GUIDs.
    player_controller_open: Option<PlayerControllerOpenPlan>,
    /// Opt-in pawn open after the PlayerController open: the plan, its
    /// channel and every player's content.
    pawn_open: Option<(PawnOpenPlan, u32, PawnContent)>,
    /// Native run's opt-in UID owner; bare lab experiments retain templates.
    actor_uids: Option<crate::actor_uids::ActorUidLibrary>,
    /// Saved player characters returning pawns restore from (F-950), and the
    /// ones live sessions hold.
    returning_characters: Vec<super::recorders::SavedPlayerCharacter>,
    restored_characters: Vec<usize>,
    next_pawn_object_id: Option<i32>,
    /// Opt-in GameState open after the PlayerController open.
    game_state_open: Option<GameStateSession>,
    /// Opt-in PlayerState open after the PlayerController open.
    player_state_open: Option<PlayerStateOpenPlan>,
    /// Opt-in world manager opens after the PlayerState: each manager's
    /// key, plan and channel (F-859).
    manager_opens: Vec<(ActorKey, ManagerOpenPlan, u32)>,
    /// Base layout and checksums for `C019` quest actor opens (`F-934`).
    quest_open_base: Option<QuestOpenBasePlan>,
    /// Saved `C019` quest actor opens (`F-934`), each with its channel.
    quest_opens: Vec<(QuestOpenPlan, u32)>,
    /// Driver-wide NetGUIDs, replicated actors keyed by the caller's key.
    guid_cache: NetGuidCache,
    /// `AGameSession::GetNextPlayerID`'s counter (F-784): 256 first.
    next_player_id: i32,
    /// Next lab connection ID (from 1) for joined peers.
    next_connection_id: u32,
}

impl fmt::Debug for PeerLab {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PeerLab")
            .field("peers", &self.peers.len())
            .field("peak_peers", &self.peak_peers)
            .field("max_peers", &self.config.max_peers)
            .field("admitting", &self.admitting)
            .field(
                "player_controller_open",
                &self.player_controller_open.is_some(),
            )
            .field("stats", &self.stats)
            .finish_non_exhaustive()
    }
}

impl PeerLab {
    /// Validate `config` and bind IPv4 loopback only; zero selects an
    /// ephemeral port. A fresh random cookie secret is drawn per lab.
    pub fn bind(port: u16, config: PeerLabConfig) -> Result<Self> {
        config.validate()?;
        let authority = CookieAuthority::random().context("cookie secret")?;
        let socket = bind_loopback(port).context("bind IPv4 loopback")?;
        Ok(Self::with_socket(socket, authority, config))
    }

    /// As [`bind`](Self::bind), on a caller-chosen IPv4 address that must be
    /// loopback (`127.0.0.0/8`): the lab is not exposed beyond the host.
    pub fn bind_address(address: SocketAddrV4, config: PeerLabConfig) -> Result<Self> {
        ensure!(
            address.ip().is_loopback(),
            "the peer lab binds IPv4 loopback addresses only"
        );
        config.validate()?;
        let authority = CookieAuthority::random().context("cookie secret")?;
        let socket = UdpSocket::bind(address).context("bind IPv4 loopback")?;
        socket
            .set_nonblocking(true)
            .context("set the socket nonblocking")?;
        Ok(Self::with_socket(socket, authority, config))
    }

    fn with_socket(socket: UdpSocket, authority: CookieAuthority, config: PeerLabConfig) -> Self {
        Self {
            socket,
            authority,
            started: Instant::now(),
            peers: HashMap::with_capacity(config.max_peers),
            // One excess byte detects datagrams longer than the maximum.
            buffer: vec![0; config.max_datagram_bytes() + 1],
            config,
            stats: PeerLabStats::default(),
            admitting: true,
            peak_peers: 0,
            player_controller_open: None,
            pawn_open: None,
            actor_uids: None,
            returning_characters: Vec::new(),
            restored_characters: Vec::new(),
            next_pawn_object_id: Some(crate::actor_uids::FIRST_NATIVE_OBJECT_ID),
            game_state_open: None,
            player_state_open: None,
            manager_opens: Vec::new(),
            quest_open_base: None,
            quest_opens: Vec::new(),
            guid_cache: NetGuidCache::new(),
            next_player_id: FIRST_PLAYER_ID,
            next_connection_id: 1,
        }
    }

    /// Joined peers with their lab connection IDs, in ID order: the
    /// replication viewers a session bridge maintains.
    pub fn joined_connections(&self) -> Vec<u32> {
        let mut ids: Vec<u32> = self
            .peers
            .values()
            .filter_map(|entry| entry.connection_id)
            .collect();
        ids.sort_unstable();
        ids
    }

    /// Retry every pending close (see [`close_actor`](Self::close_actor));
    /// cheap when none is pending.
    pub fn retry_pending_closes(&mut self) {
        let pending: Vec<(u32, u64, u32)> = self
            .peers
            .values()
            .filter_map(|entry| entry.connection_id.map(|id| (id, &entry.pending_closes)))
            .flat_map(|(id, closes)| closes.iter().map(move |(key, reason)| (id, *key, *reason)))
            .collect();
        for (connection, key, reason) in pending {
            self.close_actor(connection, key, reason);
        }
    }

    /// Send the actor `key`'s resting `ReplicatedMovement` at `location` to the
    /// joined connection `connection` as an unreliable property update, when
    /// the quantized movement differs from the one last sent and the channel's
    /// open is acked (UE drops unreliable bunches for a channel the client has not
    /// opened yet). Returns whether an update was sent; failures count
    /// `actor_update_errors` and change nothing. A lost update is repaired as
    /// UE re-sends a naked packet's properties: when the client naks its
    /// packet and no newer update has gone out since, the movement counts as
    /// unsent (`actor_updates_lost`) and the next selection sends it again.
    pub fn update_actor_movement(
        &mut self,
        connection: u32,
        key: u64,
        location: [f32; 3],
        engine_net_version: u32,
    ) -> bool {
        self.update_actor_motion(
            connection,
            key,
            location,
            [0.0; 3],
            [0.0; 3],
            engine_net_version,
        )
    }

    /// Send complete authored property 6 at the AActor defaults. Rotation is
    /// pitch/yaw/roll degrees; velocity is linear only. Invalid or unframeable
    /// attempts leave the baseline and packet repair identity unchanged.
    pub fn update_actor_motion(
        &mut self,
        connection: u32,
        key: u64,
        location: [f32; 3],
        rotation: [f32; 3],
        velocity: [f32; 3],
        engine_net_version: u32,
    ) -> bool {
        self.send_actor_properties(
            connection,
            key,
            authored_movement(location, rotation, velocity).map(Some),
            None,
            engine_net_version,
        )
    }

    /// Replicate selected authored state atomically. `motion` is
    /// `[location, pitch/yaw/roll degrees, linear velocity]`, or absent for an
    /// actor without movement replication. `flags` supplies both explicit
    /// Booleans; removing previously managed flags requests the authored reset
    /// `(false, false)` until sent (not official class defaults). Never-managed
    /// actors emit neither flag. Pending closes and unacked opens block sends.
    pub fn update_actor_properties(
        &mut self,
        connection: u32,
        key: u64,
        motion: Option<[[f32; 3]; 3]>,
        flags: Option<crate::replication::NetActorFlags>,
        engine_net_version: u32,
    ) -> bool {
        let movement = motion
            .map(|[location, rotation, velocity]| authored_movement(location, rotation, velocity))
            .transpose();
        self.send_actor_properties(connection, key, movement, Some(flags), engine_net_version)
    }

    fn send_actor_properties(
        &mut self,
        connection: u32,
        key: u64,
        movement: anyhow::Result<Option<super::rep_layout::RepMovement>>,
        flags: Option<Option<crate::replication::NetActorFlags>>,
        engine_net_version: u32,
    ) -> bool {
        let Some((address, entry)) = self
            .peers
            .iter_mut()
            .find(|(_, entry)| entry.connection_id == Some(connection))
        else {
            return false;
        };
        let address = *address;
        let Some(channel) = entry.opened_actors.get(&key).copied() else {
            return false;
        };
        if entry.pending_closes.contains_key(&key) || !entry.peer.actor_channel_settled(channel) {
            return false;
        }
        let movement = match movement {
            Ok(movement) => movement,
            Err(_) => {
                bump(&mut self.stats.actor_update_errors);
                return false;
            }
        };
        let Some(replica) = entry.actor_properties.get(&key) else {
            return false;
        };
        let mut values = BTreeMap::new();
        if let Some(movement) = movement {
            values.insert(6, RepValue::Movement(movement));
        }
        let requested_flags = flags.flatten();
        if let Some(flags) = flags {
            let flags = flags.or_else(|| {
                replica
                    .flags_managed
                    .then_some(crate::replication::NetActorFlags {
                        hidden: false,
                        can_be_damaged: false,
                    })
            });
            if let Some(flags) = flags {
                values.insert(1, RepValue::Bool(flags.hidden));
                values.insert(4, RepValue::Bool(flags.can_be_damaged));
            }
        }
        values.retain(|handle, value| replica.sent.get(handle) != Some(value));
        if values.is_empty() {
            return false;
        }
        let movement = match values.get(&6) {
            Some(RepValue::Movement(value)) => Some(value.clone()),
            _ => None,
        };
        let boolean = |handle| match values.get(&handle) {
            Some(RepValue::Bool(value)) => Some(*value),
            _ => None,
        };
        let codec = ActorChannelCodec {
            engine_net_version,
            exports_enabled: false,
            has_level: true,
            order: entry.peer.connection_byte_order(),
            limits: ActorChannelLimits::default(),
        };
        let sent =
            actor_properties_update_block(engine_net_version, movement, boolean(1), boolean(4))
                .and_then(|block| {
                    encode_actor_bunch_body(
                        &ActorBunchBody {
                            spawn: None,
                            blocks: vec![block],
                        },
                        &codec,
                    )
                })
                .and_then(|payload| entry.peer.actor_update_datagrams(channel, payload));
        let datagrams = match sent {
            Ok(datagrams) => datagrams,
            Err(_) => {
                bump(&mut self.stats.actor_update_errors);
                if entry.peer.is_failed() {
                    self.remove_peer(address);
                    bump(&mut self.stats.peers_failed);
                }
                return false;
            }
        };
        entry.ack_pending = false;
        let mut delivered = true;
        for (datagram, _) in &datagrams {
            delivered &= self.send(datagram, address);
        }
        if !delivered {
            // No baseline or identity commits until every local send succeeds.
            return false;
        }
        if let Some(entry) = self.peers.get_mut(&address) {
            let replica = entry.actor_properties.get_mut(&key).unwrap();
            replica.flags_managed |= requested_flags.is_some();
            let update = datagrams.last().unwrap().1;
            let handles: Vec<_> = values.keys().copied().collect();
            for (handle, value) in values {
                replica.sent.insert(handle, value);
                replica.last_packet.insert(handle, update);
            }
            for (_, packet_id) in datagrams {
                entry.actor_property_packets.insert(
                    packet_id,
                    ActorPropertyPacket {
                        key,
                        update,
                        handles: handles.clone(),
                    },
                );
            }
        }
        bump(&mut self.stats.actor_updates_sent);
        true
    }

    /// Whether the actor `key` is open on the joined connection `connection`;
    /// a cheap check before building an [`ActorOpenSpec`].
    pub fn is_actor_open(&self, connection: u32, key: u64) -> bool {
        self.peers.values().any(|entry| {
            entry.connection_id == Some(connection) && entry.opened_actors.contains_key(&key)
        })
    }

    /// Close the actor `key`'s channel on the joined connection `connection`
    /// with `reason` ([`LabPeer::reliable_actor_close`]); the actor may be
    /// opened again later on a free channel. Returns whether a close was
    /// sent. A close that cannot be framed counts `actor_close_errors` and is
    /// kept pending: the actor stays open (so it is not reopened twice) and
    /// [`retry_pending_closes`](Self::retry_pending_closes) retries it, since
    /// the replication stage reports a close only once.
    pub fn close_actor(&mut self, connection: u32, key: u64, reason: u32) -> bool {
        let Some(address) = self
            .peers
            .iter()
            .find(|(_, entry)| entry.connection_id == Some(connection))
            .map(|(address, _)| *address)
        else {
            return false;
        };
        let Some(entry) = self.peers.get_mut(&address) else {
            return false;
        };
        let Some(channel) = entry.opened_actors.get(&key).copied() else {
            return false;
        };
        let datagram = match entry.peer.reliable_actor_close(channel, reason) {
            Ok(datagram) => datagram,
            Err(_) => {
                bump(&mut self.stats.actor_close_errors);
                if entry.peer.is_failed() {
                    self.remove_peer(address);
                    bump(&mut self.stats.peers_failed);
                } else {
                    entry.pending_closes.insert(key, reason);
                }
                return false;
            }
        };
        entry.pending_closes.remove(&key);
        entry.opened_actors.remove(&key);
        entry.actor_properties.remove(&key);
        entry
            .actor_property_packets
            .retain(|_, packet| packet.key != key);
        entry.ack_pending = false;
        if self.send(&datagram, address) {
            bump(&mut self.stats.actor_closes_sent);
        }
        true
    }

    /// Open the actor `key` on the joined connection `connection` with
    /// `open` on the lowest free channel from `first_channel`. The lab owns
    /// actor GUIDs: `open.actor` is ignored and replaced by the driver-wide
    /// GUID for `key`. Returns the channel, or `None` when the actor is
    /// already open there or the connection is unknown; the already-open
    /// check runs first and allocates nothing. Composition and framing
    /// failures count `actor_open_errors` and change nothing.
    pub fn open_actor(
        &mut self,
        connection: u32,
        key: u64,
        mut open: ActorOpenSpec,
        first_channel: u32,
        engine_net_version: u32,
    ) -> Option<u32> {
        let address = self
            .peers
            .iter()
            .find(|(_, entry)| entry.connection_id == Some(connection))
            .map(|(address, _)| *address)?;
        let entry = self.peers.get_mut(&address)?;
        if entry.opened_actors.contains_key(&key) {
            return None;
        }
        // Without a free channel nothing can change: fail before cloning the
        // driver GUID cache and the peer's exports.
        let Some(channel) = entry.peer.free_actor_channel(first_channel) else {
            bump(&mut self.stats.actor_open_errors);
            if entry.peer.is_failed() {
                self.remove_peer(address);
                bump(&mut self.stats.peers_failed);
            }
            return None;
        };
        let mut cache = self.guid_cache.clone();
        let guid = cache.actor_guid(ActorKey::World(key));
        open.actor = guid;
        let mut exports = entry.exports.clone();
        let (values, flags_managed) = initial_actor_properties(
            &open.blocks,
            engine_net_version,
            entry.peer.connection_byte_order(),
        );
        let sent = compose_actor_open(
            &mut cache,
            &mut exports,
            &open,
            channel,
            engine_net_version,
            entry.peer.connection_byte_order(),
        )
        .and_then(|composed| {
            let export_payloads: Vec<Payload> = composed.exports.into_iter().collect();
            entry
                .peer
                .reliable_actor_open_datagrams(&composed.bunch, &export_payloads)
        });
        let sent = match sent {
            Ok(sent) => sent,
            Err(_) => {
                bump(&mut self.stats.actor_open_errors);
                if entry.peer.is_failed() {
                    self.remove_peer(address);
                    bump(&mut self.stats.peers_failed);
                }
                return None;
            }
        };
        commit_exports(&mut exports, &sent);
        entry.exports = exports;
        entry.ack_pending = false;
        entry.opened_actors.insert(key, channel);
        entry.actor_properties.insert(
            key,
            ActorReplica {
                flags_managed,
                ..ActorReplica::default()
            },
        );
        self.guid_cache = cache;
        if self.send_all(&sent, address) {
            if let Some(replica) = self
                .peers
                .get_mut(&address)
                .and_then(|entry| entry.actor_properties.get_mut(&key))
            {
                replica.sent = values;
            }
            bump(&mut self.stats.actor_opens_sent);
        }
        Some(channel)
    }

    /// Whether `channel` is in range, not the control channel and not taken
    /// by another enabled session open.
    fn free_session_channel(&self, channel: u32) -> bool {
        let taken = [
            Some(self.config.packet.control_channel_index),
            self.player_controller_open
                .as_ref()
                .map(|plan| plan.channel_index),
            self.pawn_open.as_ref().map(|(_, channel, _)| *channel),
            self.game_state_open
                .as_ref()
                .map(|session| session.channel_index),
            self.player_state_open
                .as_ref()
                .map(|plan| plan.channel_index),
        ];
        channel < self.config.packet.packet.limits.max_channels
            && !taken.contains(&Some(channel))
            && !self
                .manager_opens
                .iter()
                .any(|(_, _, taken)| *taken == channel)
            && !self.quest_opens.iter().any(|(_, taken)| *taken == channel)
    }

    /// After [`enable_player_controller_open`](Self::enable_player_controller_open),
    /// also open each joined player's pawn (F-797) on `channel_index`, right
    /// after its PlayerController, at the controller's spawn location, under
    /// [`pawn_key`] (a session key per connection), with `content`.
    pub fn enable_pawn_open(
        &mut self,
        plan: PawnOpenPlan,
        channel_index: u32,
        content: PawnContent,
    ) -> Result<()> {
        let Some(controller) = &self.player_controller_open else {
            bail!("the pawn open follows the PlayerController open");
        };
        ensure!(
            plan.level == controller.profile.level,
            "the pawn and PlayerController opens name different levels"
        );
        ensure!(
            self.free_session_channel(channel_index),
            "the pawn channel collides with another channel or is out of range"
        );
        self.pawn_open = Some((plan, channel_index, content));
        Ok(())
    }

    /// Assign each pawn a UID in a world-wide namespace. Configure and seed
    /// before admitting peers. Only handle 46 varies from the shared content.
    /// The authored object counter starts at 65,536, advances once per attempt,
    /// and fails closed at i32 exhaustion. It is not an engine object index.
    pub fn enable_actor_uids(&mut self, library: crate::actor_uids::ActorUidLibrary) -> Result<()> {
        ensure!(
            self.peers.is_empty(),
            "configure actor UIDs before admitting peers"
        );
        ensure!(
            self.actor_uids.is_none(),
            "actor UIDs are already configured"
        );
        self.actor_uids = Some(library);
        Ok(())
    }

    /// Whether native per-pawn UID allocation is configured.
    pub fn has_actor_uids(&self) -> bool {
        self.actor_uids.is_some()
    }

    /// UIDs currently claimed, in claim order: saved seeds, then live pawns.
    /// Empty when native UID allocation is not configured.
    pub fn claimed_actor_uids(&self) -> &[i32] {
        self.actor_uids
            .as_ref()
            .map_or(&[], crate::actor_uids::ActorUidLibrary::claimed)
    }

    /// Occupy every positive saved actor UID before joins. This protects even
    /// actors whose replication is unsupported; it does not import them.
    pub fn seed_actor_uids(&mut self, uids: impl IntoIterator<Item = i32>) -> Result<()> {
        ensure!(
            self.peers.is_empty(),
            "seed actor UIDs before admitting peers"
        );
        if let Some(library) = &mut self.actor_uids {
            library.seed_saved(uids)?;
        }
        Ok(())
    }

    /// Let returning players' pawns retake their saved UIDs (F-950): when a
    /// joined session sends its character slot (F-951), the first saved
    /// character matching its login ID and slot (F-949) that no live session
    /// holds gives the pawn its seeded UID. Configure after
    /// [`seed_actor_uids`](Self::seed_actor_uids) and before admitting peers.
    pub fn enable_returning_characters(
        &mut self,
        characters: Vec<super::recorders::SavedPlayerCharacter>,
    ) -> Result<()> {
        ensure!(
            self.peers.is_empty(),
            "configure returning characters before admitting peers"
        );
        ensure!(
            self.actor_uids.is_some(),
            "returning characters need actor UIDs"
        );
        self.returning_characters = characters;
        Ok(())
    }

    /// F-950: the original's character restore frees the pawn's BeginPlay UID
    /// and claims its saved one; the client sees the changed `IcarusUID`.
    /// Natively the saved UID is already claimed as a seed, so ownership moves
    /// to the pawn once the update is sent. The update goes out as a reliable
    /// bunch on the pawn channel (native policy: no loss repair needed).
    fn restore_returning_pawn_uid(&mut self, address: SocketAddrV4) {
        let (Some((plan, channel, _)), Some(library)) = (&self.pawn_open, &mut self.actor_uids)
        else {
            return;
        };
        let Some(entry) = self.peers.get_mut(&address) else {
            return;
        };
        let (Some(fresh), Some(slot), Some(id), None) = (
            entry.pawn_uid,
            entry.character_slot,
            entry.login_unique_id.as_ref(),
            entry.restored_character,
        ) else {
            return;
        };
        let user = login_user_id(id);
        // A save may hold several records with one UID; a UID a live session
        // already holds must not be handed out again, so every record that
        // shares it is skipped along with the held records themselves.
        let held: Vec<usize> = self
            .returning_characters
            .iter()
            .enumerate()
            .filter(|(_, character)| {
                self.restored_characters
                    .iter()
                    .any(|held| self.returning_characters[*held].icarus_uid == character.icarus_uid)
            })
            .map(|(index, _)| index)
            .collect();
        let Some((index, character)) = super::recorders::find_saved_player_character(
            &self.returning_characters,
            &user,
            slot,
            &held,
        ) else {
            bump(&mut self.stats.returning_pawns_unmatched);
            return;
        };
        let saved = character.icarus_uid;
        if saved <= 0 || saved == fresh || !library.is_claimed(saved) {
            bump(&mut self.stats.returning_pawn_errors);
            return;
        }
        let sent = compose_pawn_icarus_uid_update(
            &plan.layout,
            plan.engine_net_version,
            saved,
            entry.peer.connection_byte_order(),
        )
        .and_then(|payload| entry.peer.reliable_actor_bunch(*channel, payload));
        let datagram = match sent {
            Ok(datagram) => datagram,
            Err(_) => {
                bump(&mut self.stats.returning_pawn_errors);
                if entry.peer.is_failed() {
                    self.remove_peer(address);
                    bump(&mut self.stats.peers_failed);
                }
                return;
            }
        };
        // The saved UID's seed claim becomes the pawn's: it stays claimed
        // throughout (no re-claim a reservation could remap) and only the
        // fresh claim is released.
        library.free(fresh);
        entry.pawn_uid = Some(saved);
        entry.restored_character = Some(index);
        entry.ack_pending = false;
        self.restored_characters.push(index);
        bump(&mut self.stats.returning_pawns_restored);
        self.send(&datagram, address);
    }

    // Native policy releases a pawn's UID at every session removal. The
    // original's FreeUniqueID call timing has not been established.
    fn remove_peer(&mut self, address: SocketAddrV4) -> Option<PeerEntry> {
        let entry = self.peers.remove(&address)?;
        if let Some(index) = entry.restored_character {
            // The saved record keeps its UID, as its seed did before the join.
            self.restored_characters.retain(|held| *held != index);
        } else if let (Some(uid), Some(library)) = (entry.pawn_uid, &mut self.actor_uids) {
            library.free(uid);
        }
        Some(entry)
    }

    /// After [`enable_player_controller_open`](Self::enable_player_controller_open),
    /// also open the world's GameState (F-794) on each joined connection, on
    /// `channel_index`, after its PlayerController and before its PlayerState,
    /// spread over as many datagrams as it needs. `state.key` must be an
    /// [`ActorKey::Session`] key. The lab writes `ReplicatedWorldTimeSeconds`
    /// (handle 19) itself: seconds since bind, a lab choice for the world's
    /// time (`state.values` may not carry it).
    pub fn enable_game_state_open(
        &mut self,
        plan: GameStateOpenPlan,
        state: GameStateOpenState,
        channel_index: u32,
    ) -> Result<()> {
        let Some(controller) = &self.player_controller_open else {
            bail!("the GameState open follows the PlayerController open");
        };
        ensure!(
            matches!(state.key, ActorKey::Session(_)),
            "the GameState is a session actor"
        );
        ensure!(
            !state
                .values
                .contains_key(&super::game_state_open::handle::WORLD_TIME_SECONDS),
            "the lab writes the GameState's world time"
        );
        ensure!(
            self.free_session_channel(channel_index),
            "the GameState channel collides with another channel or is out of range"
        );
        ensure!(
            plan.level == controller.profile.level,
            "the GameState and PlayerController opens name different levels"
        );
        self.game_state_open = Some(GameStateSession {
            plan,
            state,
            active_prospect: None,
            member_refresh: None,
            channel_index,
            level_time: None,
            locked_missions: Vec::new(),
            clock: None,
            clock_at: Instant::now(),
            fps: None,
            polled_at: Instant::now(),
            updated_at: None,
            networking: None,
        });
        Ok(())
    }

    /// Whether joined players are sent the GameState open.
    pub fn has_game_state_open(&self) -> bool {
        self.game_state_open.is_some()
    }

    /// Add `values` to the GameState open's values (replacing equal
    /// handles), for state that is known only after bind, such as a loaded
    /// world's active prospect. Fails without a GameState open, or for the
    /// lab-written world time. After an explicit prospect refresh, any
    /// active-prospect handle rejects the entire map; use
    /// [`refresh_active_prospect`](Self::refresh_active_prospect) instead.
    /// Unrelated handles still change the future-open template only.
    pub fn extend_game_state_values(&mut self, values: BTreeMap<u32, RepValue>) -> Result<()> {
        let Some(session) = &mut self.game_state_open else {
            bail!("no GameState open is enabled");
        };
        ensure!(
            !values.contains_key(&super::game_state_open::handle::WORLD_TIME_SECONDS),
            "the lab writes the GameState's world time"
        );
        ensure!(
            session.active_prospect.is_none()
                || !values
                    .keys()
                    .any(|handle| super::game_state_open::handle::ACTIVE_PROSPECT.contains(handle)),
            "use refresh_active_prospect after an explicit prospect refresh"
        );
        session.state.values.extend(values);
        Ok(())
    }

    /// Replace the active prospect for future opens and joined peers.
    ///
    /// The caller supplies current `ElapsedTime` and each member's
    /// `IsCurrentlyPlaying`; [`super::prospect_info::ProspectInfo::for_session`]
    /// can compose those from explicitly supplied state. This operation
    /// establishes no original trigger timing (F-822; U-070). Login,
    /// disconnect and clock ticks never call it automatically; Join calls it
    /// only under [`Self::enable_member_refresh`], whose stored prospect each
    /// successful call here replaces.
    ///
    /// Validate enums, codec kinds/widths, resource bounds and both byte
    /// orders against the configured release before changing either state,
    /// including the open's dynamic fields as they stand at this call.
    /// Defaults disappear from the future-open template but remain explicit
    /// current values, so zero/false/empty clear what a peer previously saw.
    /// Updates wait for each peer's open ACK and use the existing pacing and
    /// ACK/NAK/partial repair path. No packet is sent by this call itself.
    pub fn refresh_active_prospect(
        &mut self,
        info: &super::prospect_info::ProspectInfo,
    ) -> Result<()> {
        use super::archive::ByteOrder;
        use super::game_state_open::handle;
        let Some(session) = &mut self.game_state_open else {
            bail!("no GameState open is enabled");
        };
        let values = active_prospect_update_values(info)?;
        let mut state = session.state.clone();
        state
            .values
            .retain(|handle, _| !handle::ACTIVE_PROSPECT.contains(handle));
        for (&handle, value) in &values {
            if class_default(handle).as_ref() != Some(value) {
                state.values.insert(handle, value.clone());
            }
        }
        let open_state = session.open_state(&state, self.started.elapsed().as_secs_f32())?;
        for order in [ByteOrder::Little, ByteOrder::Big] {
            let update = compose_game_state_update(
                &session.plan.layout,
                session.plan.engine_net_version,
                &values,
                order,
            )?;
            ensure!(
                update.bit_len().div_ceil(8) <= super::packet_lab::MAX_CONSTRUCTED_BUNCH_BYTES,
                "active prospect update exceeds the bunch byte limit"
            );
            // Compose with disposable GUID/export state: an invalid future
            // open must not publish either the template or current values.
            let open = compose_game_state_open(
                &mut self.guid_cache.clone(),
                &mut ConnectionExports::new(),
                &session.plan,
                &open_state,
                session.channel_index,
                order,
            )?;
            ensure!(
                open.bunch.payload.bit_len().div_ceil(8)
                    <= super::packet_lab::MAX_CONSTRUCTED_BUNCH_BYTES,
                "active prospect open exceeds the bunch byte limit"
            );
        }
        session.state = state;
        session.active_prospect = Some(values);
        // A later member Join starts from the latest prospect, not the one
        // member refresh was enabled with.
        if let Some(base) = &mut session.member_refresh {
            base.clone_from(info);
        }
        Ok(())
    }

    /// Refresh `info` as the active prospect whenever a Join's login ID
    /// names one of its associated members, as the original's spawn
    /// completion refreshes an associated player's prospect (F-822).
    /// Each member is currently playing iff a joined peer's login ID equals
    /// its `UserID` (F-945): the ID's string form, uppercase hexadecimal for
    /// encoded IDs, compared ignoring ASCII case (the original's compare
    /// rule is unread). `ElapsedTime` and every other field stay as last
    /// supplied: `info`, or the latest [`Self::refresh_active_prospect`].
    /// Joins of unassociated players, which the original adds as members
    /// and saves, and disconnects (no direct original caller refreshes
    /// then) change nothing. Validated as [`Self::refresh_active_prospect`]
    /// would be before anything changes.
    pub fn enable_member_refresh(
        &mut self,
        info: super::prospect_info::ProspectInfo,
    ) -> Result<()> {
        let Some(session) = &self.game_state_open else {
            bail!("no GameState open is enabled");
        };
        info.validate_enumerators()?;
        active_prospect_update_values(&info)?;
        ensure!(
            session.member_refresh.is_none(),
            "member refresh is already enabled"
        );
        if let Some(session) = &mut self.game_state_open {
            session.member_refresh = Some(info);
        }
        Ok(())
    }

    /// The refresh a Join of `address` causes under
    /// [`Self::enable_member_refresh`], counted in the stats.
    fn refresh_members_after_join(&mut self, address: SocketAddrV4) {
        let Some(base) = self
            .game_state_open
            .as_ref()
            .and_then(|session| session.member_refresh.as_ref())
        else {
            return;
        };
        let joined: Vec<Vec<u16>> = self
            .peers
            .iter()
            .filter(|(other, peer)| {
                **other == address || peer.admission.state == ClientLoginState::Joined
            })
            .filter_map(|(_, peer)| peer.login_unique_id.as_ref().map(login_user_id))
            .collect();
        let Some(joiner) = self
            .peers
            .get(&address)
            .and_then(|peer| peer.login_unique_id.as_ref())
            .map(login_user_id)
        else {
            return;
        };
        if !base
            .associated_members
            .iter()
            .any(|member| same_user_id(&member.user_id, &joiner))
        {
            bump(&mut self.stats.member_refreshes_unassociated);
            return;
        }
        let mut info = base.clone();
        for member in &mut info.associated_members {
            member.is_currently_playing = joined.iter().any(|id| same_user_id(&member.user_id, id));
        }
        match self.refresh_active_prospect(&info) {
            Ok(()) => bump(&mut self.stats.member_refreshes),
            Err(_) => bump(&mut self.stats.member_refresh_errors),
        }
    }

    /// After the GameState open, also open the world's managers (F-859) on
    /// each joined connection, after its PlayerState, on consecutive
    /// channels from `first_channel`, and reference them from the GameState
    /// open by their slots' keys. Fails without a GameState open, on a level
    /// other than the GameState's, or on a taken channel.
    pub fn enable_manager_opens(
        &mut self,
        plans: Vec<(ManagerSlot, ManagerOpenPlan)>,
        first_channel: u32,
    ) -> Result<()> {
        let Some(session) = &self.game_state_open else {
            bail!("the manager opens follow the GameState open");
        };
        let level = session.plan.level.clone();
        let mut managers = session.state.managers.clone();
        let mut opens = Vec::new();
        for (channel, (slot, plan)) in (first_channel..).zip(plans) {
            ensure!(
                plan.level == level,
                "a manager and the GameState name different levels"
            );
            ensure!(
                self.free_session_channel(channel),
                "a manager channel collides with another channel or is out of range"
            );
            slot.reference_from(&mut managers);
            opens.push((slot.key(), plan, channel));
        }
        if let Some(session) = &mut self.game_state_open {
            session.state.managers = managers;
        }
        self.manager_opens = opens;
        Ok(())
    }

    /// Also open `plan` in `slot` on each joined connection after the
    /// enabled managers and quests, on the channel after theirs, and reference
    /// it from the GameState open; returns the channel. Fails without enabled
    /// managers, on a slot already opened, on a level other than the
    /// GameState's or on a taken channel.
    pub fn add_manager_open(&mut self, slot: ManagerSlot, plan: ManagerOpenPlan) -> Result<u32> {
        let Some(session) = &self.game_state_open else {
            bail!("the manager opens follow the GameState open");
        };
        ensure!(
            plan.level == session.plan.level,
            "a manager and the GameState name different levels"
        );
        ensure!(
            self.manager_opens
                .iter()
                .all(|(key, ..)| *key != slot.key())
                && self
                    .quest_opens
                    .iter()
                    .all(|(quest, _)| quest.key != slot.key()),
            "the manager slot is already opened"
        );
        let channel = self
            .manager_opens
            .iter()
            .map(|(.., channel)| *channel)
            .chain(self.quest_opens.iter().map(|(_, channel)| *channel))
            .max()
            .context("a manager added after the enabled managers")?
            .checked_add(1)
            .context("manager channel overflow")?;
        ensure!(
            self.free_session_channel(channel),
            "a manager channel collides with another channel or is out of range"
        );
        if let Some(session) = &mut self.game_state_open {
            slot.reference_from(&mut session.state.managers);
        }
        self.manager_opens.push((slot.key(), plan, channel));
        Ok(channel)
    }

    /// Record the release's base layout and component checksums for saved
    /// `C019` quest actor opens (`F-934`). Fails without a GameState open or
    /// on a level other than the GameState's.
    pub fn enable_quest_opens(&mut self, base: QuestOpenBasePlan) -> Result<()> {
        let Some(session) = &self.game_state_open else {
            bail!("the quest opens follow the GameState open");
        };
        ensure!(
            base.level == session.plan.level,
            "the quest open base and the GameState name different levels"
        );
        self.quest_open_base = Some(base);
        Ok(())
    }

    /// The release's base layout and checksums for `C019` quest actor opens,
    /// if enabled.
    pub fn quest_open_base(&self) -> Option<&QuestOpenBasePlan> {
        self.quest_open_base.as_ref()
    }

    /// Number of enabled world manager opens (`F-859`).
    pub fn manager_open_count(&self) -> usize {
        self.manager_opens.len()
    }

    /// Number of enabled `C019` quest actor opens (`F-934`).
    pub fn quest_open_count(&self) -> usize {
        self.quest_opens.len()
    }

    /// Validate shared static identities without assigning GUIDs or changing
    /// plans/export state. Include deferred manager plans as well as enabled
    /// opens: the host may attach those managers after binding. Dynamic actors
    /// and their named components use separate GUID keys, not static paths.
    pub(crate) fn validate_static_paths<'a>(
        &'a self,
        paths: impl IntoIterator<Item = &'a [PathSegment]>,
        additional_managers: impl IntoIterator<Item = &'a ManagerOpenPlan>,
    ) -> Result<()> {
        let mut paths: Vec<_> = paths.into_iter().collect();
        if let Some(plan) = &self.player_controller_open {
            paths.extend([
                plan.profile.archetype.as_slice(),
                plan.profile.level.as_slice(),
            ]);
        }
        if let Some((plan, _, _)) = &self.pawn_open {
            paths.extend([
                plan.archetype.as_slice(),
                plan.level.as_slice(),
                plan.content.inventory_class.as_slice(),
            ]);
        }
        if let Some(session) = &self.game_state_open {
            let plan = &session.plan;
            paths.extend([
                plan.archetype.as_slice(),
                plan.level.as_slice(),
                plan.game_mode_class.as_slice(),
                plan.spectator_class.as_slice(),
            ]);
        }
        if let Some(plan) = &self.player_state_open {
            paths.extend([plan.archetype.as_slice(), plan.level.as_slice()]);
        }
        for plan in self
            .manager_opens
            .iter()
            .map(|(_, plan, _)| plan)
            .chain(additional_managers)
        {
            paths.extend([plan.archetype.as_slice(), plan.level.as_slice()]);
            if let Some(owned) = &plan.inventories {
                paths.push(owned.blocks.inventory_class.as_slice());
            }
            for component in &plan.components {
                if let Block::Composed(block) = &component.block {
                    for reference in block.references.values() {
                        if let Reference::Static(chain) = reference {
                            paths.push(chain.as_slice());
                        }
                    }
                }
            }
        }
        if let Some(base) = &self.quest_open_base {
            paths.push(base.level.as_slice());
        }
        for (plan, _) in &self.quest_opens {
            paths.extend([plan.archetype.as_slice(), plan.level.as_slice()]);
            for reference in plan.references.values() {
                if let Reference::Static(chain) = reference {
                    paths.push(chain.as_slice());
                }
            }
        }
        let mut identities = BTreeMap::new();
        for chain in paths {
            for (index, segment) in chain.iter().enumerate() {
                // Match NetGuidCache::assign_static: every suffix has its own
                // key, containing path strings only. Metadata must agree even
                // when only an outer/package is shared with another chain.
                let key: Vec<_> = chain[index..]
                    .iter()
                    .map(|part| part.path.as_str())
                    .collect();
                let identity = (segment.checksum, segment.no_load);
                ensure!(
                    identities
                        .insert(key, identity)
                        .is_none_or(|previous| previous == identity),
                    "configured static path identity conflict"
                );
            }
        }
        Ok(())
    }

    /// Also open the saved quest actor `plan` (`C019`, `F-934`) on each joined
    /// connection after the enabled managers and quests, on the next channel;
    /// returns the channel. Fails without enabled managers, on a duplicate
    /// actor key, on a level other than the GameState's or on a taken channel.
    pub fn add_quest_open(&mut self, plan: QuestOpenPlan) -> Result<u32> {
        let (channels, _) = self.add_quest_opens_with_manager(vec![plan], None)?;
        Ok(channels[0])
    }

    /// Atomically validate and add a batch of saved `C019` quest actor opens
    /// (`F-934`), plus an optional trailing world manager (such as
    /// `ManagerSlot::Quest`), on consecutive channels after the enabled
    /// managers and quests. If any quest or the trailing manager fails level,
    /// key-uniqueness or channel-capacity validation, neither `quest_opens`
    /// nor `manager_opens` is modified.
    pub fn add_quest_opens_with_manager(
        &mut self,
        plans: Vec<QuestOpenPlan>,
        manager: Option<(ManagerSlot, ManagerOpenPlan)>,
    ) -> Result<(Vec<u32>, Option<u32>)> {
        if plans.is_empty() && manager.is_none() {
            return Ok((Vec::new(), None));
        }
        let Some(session) = &self.game_state_open else {
            bail!("the quest opens follow the GameState open");
        };
        let first_channel = self
            .manager_opens
            .iter()
            .map(|(.., channel)| *channel)
            .chain(self.quest_opens.iter().map(|(_, channel)| *channel))
            .max()
            .context(if plans.is_empty() {
                "a manager added after the enabled managers"
            } else {
                "a quest actor added after the enabled managers"
            })?
            .checked_add(1)
            .context("quest channel overflow")?;
        let mut quest_channels = Vec::with_capacity(plans.len());
        for (index, plan) in plans.iter().enumerate() {
            ensure!(
                plan.level == session.plan.level,
                "a quest actor and the GameState name different levels"
            );
            ensure!(
                self.quest_opens
                    .iter()
                    .all(|(existing, _)| existing.key != plan.key)
                    && self.manager_opens.iter().all(|(key, ..)| *key != plan.key)
                    && plans[..index].iter().all(|earlier| earlier.key != plan.key),
                "the quest actor key is already opened"
            );
            let offset = u32::try_from(index).context("quest channel overflow")?;
            let channel = first_channel
                .checked_add(offset)
                .context("quest channel overflow")?;
            ensure!(
                self.free_session_channel(channel),
                "a quest channel collides with another channel or is out of range"
            );
            quest_channels.push(channel);
        }
        let manager_channel = match &manager {
            Some((slot, plan)) => {
                ensure!(
                    plan.level == session.plan.level,
                    "a manager and the GameState name different levels"
                );
                ensure!(
                    self.manager_opens
                        .iter()
                        .all(|(key, ..)| *key != slot.key())
                        && self
                            .quest_opens
                            .iter()
                            .all(|(quest, _)| quest.key != slot.key())
                        && plans.iter().all(|quest| quest.key != slot.key()),
                    "the manager slot is already opened"
                );
                let offset = u32::try_from(plans.len()).context("manager channel overflow")?;
                let channel = first_channel
                    .checked_add(offset)
                    .context("manager channel overflow")?;
                ensure!(
                    self.free_session_channel(channel),
                    "a manager channel collides with another channel or is out of range"
                );
                Some(channel)
            }
            None => None,
        };
        for (plan, &channel) in plans.into_iter().zip(&quest_channels) {
            self.quest_opens.push((plan, channel));
        }
        if let (Some((slot, plan)), Some(channel)) = (manager, manager_channel) {
            if let Some(session) = &mut self.game_state_open {
                slot.reference_from(&mut session.state.managers);
            }
            self.manager_opens.push((slot.key(), plan, channel));
        }
        Ok((quest_channels, manager_channel))
    }

    /// Send the GameState's `LevelTimeElapsedSec` as `restored` plus the
    /// whole world seconds at each send. Fails without a GameState open.
    pub fn set_game_state_level_time(&mut self, restored: i32) -> Result<()> {
        let Some(session) = &mut self.game_state_open else {
            bail!("no GameState open is enabled");
        };
        session.level_time = Some(restored);
        Ok(())
    }

    /// Replicate `missions` as the GameState's `LockedMissions` (F-834),
    /// refreshed at the restored level clock and every
    /// `LOCKED_MISSIONS_REFRESH_SECONDS` of world time after it. Fails
    /// without a GameState open or a restored level clock.
    pub fn enable_locked_missions(&mut self, missions: Vec<LockedMission>) -> Result<()> {
        let Some(session) = &mut self.game_state_open else {
            bail!("no GameState open is enabled");
        };
        ensure!(
            session.level_time.is_some(),
            "locked missions need the restored level clock"
        );
        session.locked_missions = missions;
        Ok(())
    }

    /// Advance the GameState's `TimeOfDay` with `clock` from now on (F-806):
    /// opens carry its current value and updates its changes. Fails without
    /// a GameState open.
    pub fn enable_time_of_day(&mut self, clock: TimeOfDayClock) -> Result<()> {
        let Some(session) = &mut self.game_state_open else {
            bail!("no GameState open is enabled");
        };
        session.clock = Some(clock);
        session.clock_at = Instant::now();
        Ok(())
    }

    /// Measure `ServerFPS` with `meter` over the intervals between polls
    /// from now on: opens carry its last value and updates its changes.
    /// Fails without a GameState open.
    pub fn enable_server_fps(&mut self, meter: ServerFpsMeter) -> Result<()> {
        let Some(session) = &mut self.game_state_open else {
            bail!("no GameState open is enabled");
        };
        session.fps = Some(meter);
        session.polled_at = Instant::now();
        Ok(())
    }

    /// Replicate `status` as the GameState's `NetworkingStatus` (F-810):
    /// opens carry it as last gathered and each update pass lists the
    /// joined players (names; pings from acked round trips, F-815) in join
    /// order, sending the fields that changed. Fails without a GameState
    /// open or with players already listed.
    pub fn enable_networking_status(&mut self, status: NetworkingStatus) -> Result<()> {
        let Some(session) = &mut self.game_state_open else {
            bail!("no GameState open is enabled");
        };
        ensure!(
            status.player_names.is_empty() && status.player_ping.is_empty(),
            "the lab lists the joined players itself"
        );
        session.networking = Some(status);
        Ok(())
    }

    /// The GameState's `NetworkingStatus` as last gathered, if enabled.
    pub fn networking_status(&self) -> Option<&NetworkingStatus> {
        self.game_state_open.as_ref()?.networking.as_ref()
    }

    /// The GameState's `ServerFPS` meter, if enabled.
    pub fn server_fps(&self) -> Option<&ServerFpsMeter> {
        self.game_state_open.as_ref()?.fps.as_ref()
    }

    /// The GameState's current `TimeOfDay` clock, if enabled.
    pub fn time_of_day(&self) -> Option<&TimeOfDayClock> {
        self.game_state_open.as_ref()?.clock.as_ref()
    }

    /// After [`enable_player_controller_open`](Self::enable_player_controller_open),
    /// also open each joined player's PlayerState (F-784) on `plan.channel_index`.
    pub fn enable_player_state_open(&mut self, plan: PlayerStateOpenPlan) -> Result<()> {
        let Some(controller) = &self.player_controller_open else {
            bail!("the PlayerState open follows the PlayerController open");
        };
        ensure!(
            self.free_session_channel(plan.channel_index),
            "the PlayerState channel collides with another channel or is out of range"
        );
        ensure!(
            plan.level == controller.profile.level,
            "the PlayerState and PlayerController opens name different levels"
        );
        self.player_state_open = Some(plan);
        Ok(())
    }

    /// Open each joined peer's PlayerController with `plan` (module docs):
    /// registers the plan's level chain in the driver-wide GUID cache first,
    /// as the original does at world start. Requires admission.
    pub fn enable_player_controller_open(&mut self, plan: PlayerControllerOpenPlan) -> Result<()> {
        ensure!(
            self.config.admission.is_some(),
            "the PlayerController open needs game admission"
        );
        ensure!(
            plan.channel_index != self.config.packet.control_channel_index
                && plan.channel_index < self.config.packet.packet.limits.max_channels,
            "the PlayerController channel is the control channel or out of range"
        );
        self.guid_cache.register_level(&plan.profile.level);
        self.player_controller_open = Some(plan);
        Ok(())
    }

    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.socket.local_addr()
    }

    pub fn config(&self) -> &PeerLabConfig {
        &self.config
    }

    /// The driver-wide GUID cache (read-only).
    pub fn guid_cache(&self) -> &NetGuidCache {
        &self.guid_cache
    }

    pub fn stats(&self) -> &PeerLabStats {
        &self.stats
    }

    pub fn peer_count(&self) -> usize {
        self.peers.len()
    }

    pub fn has_peer(&self, address: SocketAddrV4) -> bool {
        self.peers.contains_key(&address)
    }

    /// Largest peer-table size observed since bind.
    pub fn peak_peer_count(&self) -> usize {
        self.peak_peers
    }

    pub fn is_admitting(&self) -> bool {
        self.admitting
    }

    /// Stop admitting new sessions: no handshake is answered and Login, Join
    /// and SplitJoin get Failure. Live peers are kept. Idempotent.
    pub fn stop_admission(&mut self) {
        self.admitting = false;
    }

    /// Stop admission, notify every peer as `UNetDriver::Shutdown` does
    /// (module docs, F-782) and remove them all. Returns the number removed;
    /// a repeated call removes none.
    pub fn shutdown(&mut self) -> usize {
        self.stop_admission();
        let mut addresses: Vec<SocketAddrV4> = self.peers.keys().copied().collect();
        addresses.sort_unstable();
        for address in &addresses {
            self.send_shutdown_notice(*address, false);
        }
        for address in addresses.iter().rev() {
            self.send_shutdown_notice(*address, true);
        }
        let removed = self.peers.len();
        for address in addresses {
            self.remove_peer(address);
        }
        self.stats.peers_shutdown = self.stats.peers_shutdown.saturating_add(removed as u64);
        removed
    }

    /// One shutdown datagram for `address`: the Failure notice, or (`close`)
    /// the control channel's close bunch. Errors only count; the peer is
    /// removed by the caller either way.
    fn send_shutdown_notice(&mut self, address: SocketAddrV4, close: bool) {
        let Some(entry) = self.peers.get_mut(&address) else {
            return;
        };
        if entry.peer.is_failed() || entry.peer.control_closing() {
            return;
        }
        let datagram = if close {
            if !entry.peer.has_received_client_packet() {
                return;
            }
            entry.peer.reliable_control_close(CLOSE_REASON_DESTROYED)
        } else {
            ArchiveText::text(HOST_CLOSED_CONNECTION_MESSAGE).and_then(|message| {
                entry
                    .peer
                    .reliable_control_packet(ControlMessage::Failure { message })
            })
        };
        let Ok(datagram) = datagram else {
            bump(&mut self.stats.reply_errors);
            return;
        };
        if self.send(&datagram, address) {
            bump(if close {
                &mut self.stats.shutdown_closes_sent
            } else {
                &mut self.stats.shutdown_failures_sent
            });
        }
    }

    /// Current session gauges and counters (counts only).
    pub fn session_counts(&self) -> SessionCounts {
        let mut counts = SessionCounts {
            admitting: self.admitting,
            active_peers: self.peers.len(),
            peak_peers: self.peak_peers,
            welcomes_sent: self.stats.welcomes_sent,
            joins_succeeded: self.stats.joins_succeeded,
            admissions_rejected: self.stats.failures_sent,
            peers_rejected: self.stats.peers_rejected,
            refused_draining: self.stats.refused_draining,
            closed_by_timeout: self.stats.peers_expired,
            closed_by_client: self.stats.peers_closed_by_client,
            closed_by_reply: self.stats.peers_closed,
            upgrades_sent: self.stats.upgrades_sent,
            closed_failed: self.stats.peers_failed,
            closed_by_shutdown: self.stats.peers_shutdown,
            player_controller_opens_sent: self.stats.player_controller_opens_sent,
            initialisation_slots_recorded: self.stats.initialisation_slots_recorded,
            initialisation_calls_ignored: self.stats.initialisation_calls_ignored,
            returning_pawns_restored: self.stats.returning_pawns_restored,
            pawn_opens_sent: self.stats.pawn_opens_sent,
            game_state_opens_sent: self.stats.game_state_opens_sent,
            player_state_opens_sent: self.stats.player_state_opens_sent,
            manager_opens_sent: self.stats.manager_opens_sent,
            quest_opens_sent: self.stats.quest_opens_sent,
            open_errors: [
                self.stats.player_controller_open_errors,
                self.stats.pawn_open_errors,
                self.stats.game_state_open_errors,
                self.stats.player_state_open_errors,
                self.stats.manager_open_errors,
                self.stats.quest_open_errors,
                self.stats.actor_open_errors,
            ]
            .into_iter()
            .fold(0, u64::saturating_add),
            io_errors: self.stats.io_errors,
            shutdown_failures_sent: self.stats.shutdown_failures_sent,
            shutdown_closes_sent: self.stats.shutdown_closes_sent,
            invalid_state_messages: self.stats.invalid_state_messages,
            ..SessionCounts::default()
        };
        for entry in self.peers.values() {
            match entry.admission.state {
                ClientLoginState::Welcomed => counts.welcomed += 1,
                ClientLoginState::Joined => counts.joined += 1,
                _ => {}
            }
        }
        counts
    }

    /// Clamped `CurrentNetSpeed` (`UNetConnection + 0x38`, `F-740`) recorded
    /// for `address`, or `None` if the peer does not exist or has not applied a
    /// valid `Netspeed` control message yet.
    pub fn current_net_speed(&self, address: SocketAddrV4) -> Option<i32> {
        self.peers
            .get(&address)
            .and_then(|entry| entry.current_net_speed)
    }

    /// First accepted character slot for this live session; never included in
    /// status or Debug output. A replacement handshake starts with no slot.
    pub fn character_slot(&self, address: SocketAddrV4) -> Option<i32> {
        self.peers
            .get(&address)
            .and_then(|entry| entry.character_slot)
    }

    fn occupancy(&self, excluding: Option<SocketAddrV4>) -> Occupancy {
        let mut occupancy = Occupancy::default();
        for (address, entry) in &self.peers {
            if excluding == Some(*address) {
                continue;
            }
            if matches!(
                entry.admission.state,
                ClientLoginState::Welcomed | ClientLoginState::Joined
            ) {
                if entry.spectator {
                    occupancy.spectators = occupancy.spectators.saturating_add(1);
                } else {
                    occupancy.players = occupancy
                        .players
                        .saturating_add(1 + i32::from(entry.splitscreen_count));
                }
            }
        }
        occupancy
    }

    fn expire_idle(&mut self, now: Instant) {
        let timeout = self.config.idle_timeout;
        let before = self.peers.len();
        let expired: Vec<_> = self
            .peers
            .iter()
            .filter_map(|(address, entry)| {
                (now.saturating_duration_since(entry.last_seen) > timeout).then_some(*address)
            })
            .collect();
        for address in expired {
            self.remove_peer(address);
        }
        let expired = (before - self.peers.len()) as u64;
        self.stats.peers_expired = self.stats.peers_expired.saturating_add(expired);
    }

    /// Answer a handshake form; returns `false` when `bytes` is not one the
    /// authority answers.
    fn handshake(&mut self, address: SocketAddrV4, length: usize, now: Instant) -> bool {
        let seconds = lab_seconds(self.started);
        let Some(reply) = self
            .authority
            .respond(address, &self.buffer[..length], seconds)
        else {
            return false;
        };
        if !self.admitting {
            bump(&mut self.stats.refused_draining);
            return true;
        }
        match reply {
            HandshakeReply::Challenge(datagram) => {
                if self.send(&datagram, address) {
                    bump(&mut self.stats.challenges_sent);
                }
            }
            HandshakeReply::Acknowledgement { datagram, cookie } => {
                // A repeated echo of the live peer's own cookie (a retransmission
                // or a delayed duplicate) is answered again without resetting its
                // packet state; only a different cookie starts a new peer.
                if self
                    .peers
                    .get(&address)
                    .is_some_and(|entry| entry.cookie == cookie)
                {
                    if self.send(&datagram, address) {
                        bump(&mut self.stats.acknowledgements_resent);
                    }
                    return true;
                }
                let known = self.peers.contains_key(&address);
                if !known && self.peers.len() >= self.config.max_peers {
                    bump(&mut self.stats.peers_rejected);
                    return true;
                }
                // The configuration was validated at bind, so this cannot fail.
                let Ok(peer) = LabPeer::from_cookie(&cookie, self.config.packet) else {
                    bump(&mut self.stats.peers_failed);
                    return true;
                };
                if !self.send(&datagram, address) {
                    return true;
                }
                bump(&mut self.stats.acknowledgements_sent);
                bump(if known {
                    &mut self.stats.peers_replaced
                } else {
                    &mut self.stats.peers_created
                });
                self.remove_peer(address);
                self.peers.insert(
                    address,
                    PeerEntry {
                        peer,
                        pawn_uid: None,
                        restored_character: None,
                        cookie,
                        last_seen: now,
                        ack_pending: false,
                        admission: ConnectionLoginAdmission::new(),
                        login_url_units: Vec::new(),
                        spectator: false,
                        splitscreen_count: 0,
                        current_net_speed: None,
                        exports: ConnectionExports::new(),
                        login_unique_id: None,
                        player: None,
                        connection_id: None,
                        player_controller_channel: None,
                        player_state_channel: None,
                        character_slot: None,
                        opened_actors: BTreeMap::new(),
                        pending_closes: BTreeMap::new(),
                        actor_properties: BTreeMap::new(),
                        actor_property_packets: BTreeMap::new(),
                        game_state: None,
                        ping: PingEstimator::new(),
                    },
                );
                self.peak_peers = self.peak_peers.max(self.peers.len());
            }
        }
        true
    }

    fn data(&mut self, address: SocketAddrV4, length: usize, now: Instant) {
        let Some(entry) = self.peers.get_mut(&address) else {
            bump(&mut self.stats.unknown_peer);
            return;
        };
        let mut close = false;
        let mut slot_recorded = false;
        let messages = match entry.peer.receive(&self.buffer[..length]) {
            Ok(received) => {
                let summary = received.summary();
                match summary.disposition {
                    PacketDisposition::Accepted { .. } => {
                        entry.last_seen = now;
                        entry.ack_pending = true;
                        bump(&mut self.stats.data_accepted);
                        let world_seconds = now.duration_since(self.started).as_secs_f64();
                        for packet_id in &summary.acked {
                            if let Some(packet) = entry.actor_property_packets.remove(packet_id)
                                && !entry.actor_property_packets.values().any(|other| {
                                    other.key == packet.key && other.update == packet.update
                                })
                                && let Some(replica) = entry.actor_properties.get_mut(&packet.key)
                            {
                                // Only the final part's ACK retires the logical
                                // update identity; sent values remain known.
                                replica
                                    .last_packet
                                    .retain(|_, update| *update != packet.update);
                            }
                            // Each acked packet's round trip is a ping sample.
                            if let Some(sent) = entry.peer.sent_at(*packet_id) {
                                entry.ping.update(
                                    now.saturating_duration_since(sent).as_secs_f32(),
                                    world_seconds,
                                );
                            }
                        }
                        // `ReceivedAck` / `ReceivedNak` in packet order (F-796).
                        let mut outcomes: Vec<(u32, bool)> = summary
                            .acked
                            .iter()
                            .map(|id| (*id, true))
                            .chain(summary.naked.iter().map(|id| (*id, false)))
                            .collect();
                        outcomes.sort_unstable();
                        for (packet_id, acked) in outcomes {
                            if acked {
                                entry.exports.received_ack(packet_id);
                            } else {
                                entry.exports.received_nak(packet_id);
                            }
                        }
                        // Losing any part loses the logical update. Only fields
                        // it still owns become due; unrelated newer flags or
                        // movement cannot supersede its repair identity.
                        for packet_id in &summary.naked {
                            let Some(packet) = entry.actor_property_packets.remove(packet_id)
                            else {
                                continue;
                            };
                            bump(&mut self.stats.actor_update_naks);
                            entry.actor_property_packets.retain(|_, other| {
                                other.key != packet.key || other.update != packet.update
                            });
                            let mut lost = false;
                            if let Some(replica) = entry.actor_properties.get_mut(&packet.key) {
                                for handle in packet.handles {
                                    if replica.last_packet.get(&handle) == Some(&packet.update) {
                                        replica.last_packet.remove(&handle);
                                        replica.sent.remove(&handle);
                                        lost = true;
                                    }
                                }
                            }
                            if lost {
                                bump(&mut self.stats.actor_updates_lost);
                            }
                        }
                        // Likewise a naked GameState update's values that
                        // no newer update carried.
                        if let Some(replica) = &mut entry.game_state {
                            for packet_id in &summary.acked {
                                replica.acked(*packet_id);
                            }
                            for packet_id in &summary.naked {
                                if replica.naked(*packet_id) {
                                    bump(&mut self.stats.game_state_updates_lost);
                                }
                            }
                        }
                        if summary.control_close_seen {
                            bump(&mut self.stats.control_close_seen);
                            close = true;
                        }
                        self.stats.count_delivered(&summary.delivered);
                        self.stats.actor_bunches_ignored = self
                            .stats
                            .actor_bunches_ignored
                            .saturating_add(summary.ignored_bunches as u64);
                        self.stats.actor_bunches_dropped = self
                            .stats
                            .actor_bunches_dropped
                            .saturating_add(summary.actor_bunches_dropped as u64);
                        self.stats.actor_duplicate_reliable = self
                            .stats
                            .actor_duplicate_reliable
                            .saturating_add(summary.actor_duplicate_reliable as u64);
                        for bunch in received.actor_bunches() {
                            let Some(plan) = &self.player_controller_open else {
                                bump(&mut self.stats.actor_bunches_ignored);
                                continue;
                            };
                            if entry.player_controller_channel != Some(bunch.header.channel_index) {
                                bump(&mut self.stats.actor_bunches_ignored);
                                continue;
                            }
                            let Ok((slots, ignored)) = controller_initialisation_calls(
                                &bunch.payload,
                                plan.engine_net_version,
                                entry.peer.connection_byte_order(),
                            ) else {
                                bump(&mut self.stats.actor_bunches_malformed);
                                continue;
                            };
                            self.stats.controller_fields_ignored = self
                                .stats
                                .controller_fields_ignored
                                .saturating_add(ignored as u64);
                            for slot in slots {
                                // F-951 / controller net fields: the original fails
                                // missing PlayerState (0x29) and ChrSlot -1 (0x28).
                                // Its failure wire is unknown; keep the native session live.
                                if slot == -1
                                    || entry.character_slot.is_some()
                                    || entry.player.is_none()
                                    || entry.player_state_channel.is_none_or(|channel| {
                                        !entry.peer.has_actor_channel(channel)
                                    })
                                {
                                    bump(&mut self.stats.initialisation_calls_ignored);
                                } else {
                                    entry.character_slot = Some(slot);
                                    slot_recorded = true;
                                    bump(&mut self.stats.initialisation_slots_recorded);
                                }
                            }
                        }
                    }
                    PacketDisposition::Rejected => bump(&mut self.stats.data_rejected),
                }
                received.into_messages()
            }
            Err(_) => {
                bump(&mut self.stats.data_malformed);
                if entry.peer.is_failed() {
                    self.remove_peer(address);
                    bump(&mut self.stats.peers_failed);
                }
                return;
            }
        };
        self.answer(address, &messages);
        if slot_recorded {
            self.restore_returning_pawn_uid(address);
        }
        // Decoded messages are dropped here; nothing of them is retained.
        if close && self.remove_peer(address).is_some() {
            bump(&mut self.stats.peers_closed_by_client);
        }
    }

    /// Apply the reply policy (module docs) to delivered messages in order,
    /// stopping after a reply that closes the peer.
    fn answer(&mut self, address: SocketAddrV4, messages: &[Decoded]) {
        for decoded in messages {
            let message = match decoded {
                Decoded::Message(message) => message,
                Decoded::Fallback(fallback) => {
                    if self.fallback(address, fallback) {
                        return;
                    }
                    continue;
                }
                Decoded::Unknown { .. } => continue,
            };
            if self.config.admission.is_some() {
                let Some(entry) = self.peers.get(&address) else {
                    return;
                };
                if !entry.admission.is_client_msg_type_valid(message.id()) {
                    bump(&mut self.stats.invalid_state_messages);
                    continue;
                }
            }
            let Some(reply) = self.reply_for(address, message) else {
                continue;
            };
            if !self.send_reply(address, &reply) || reply.closes() {
                return;
            }
        }
    }

    /// A fallback entry from `UControlChannel::ReceivedBunch` (F-740, F-757),
    /// gated like other messages. Only `NMT_ActorChannelFailure` acts: on the
    /// PlayerController's channel the server closes the connection
    /// (`UNetConnection::Close`: the control close, reason 0; F-782), and the
    /// peer is removed. Returns whether the peer is gone.
    fn fallback(&mut self, address: SocketAddrV4, fallback: &ControlFallback) -> bool {
        let Some(entry) = self.peers.get(&address) else {
            return true;
        };
        if self.config.admission.is_some()
            && !entry.admission.is_client_msg_type_valid(fallback.id())
        {
            bump(&mut self.stats.invalid_state_messages);
            return false;
        }
        let ControlFallback::ActorChannelFailure { channel_index } = fallback else {
            bump(&mut self.stats.fallbacks_ignored);
            return false;
        };
        bump(&mut self.stats.actor_channel_failures);
        let controller_channel = self
            .player_controller_open
            .as_ref()
            .map(|plan| plan.channel_index);
        let has_controller =
            controller_channel.is_some_and(|channel| entry.peer.has_actor_channel(channel));
        let index = u32::try_from(*channel_index).ok();
        let is_actor_channel = index.is_some_and(|index| entry.peer.has_actor_channel(index));
        let action = evaluate_actor_channel_failure(
            true,
            *channel_index,
            self.config.packet.packet.limits.max_channels as usize,
            is_actor_channel,
            is_actor_channel,
            is_actor_channel && index == controller_channel,
            has_controller,
        );
        if action != ActorChannelFailureAction::CloseConnection {
            return false;
        }
        let Some(entry) = self.peers.get_mut(&address) else {
            return true;
        };
        if entry.peer.has_received_client_packet() {
            match entry.peer.reliable_control_close(CLOSE_REASON_DESTROYED) {
                Ok(datagram) => {
                    if self.send(&datagram, address) {
                        bump(&mut self.stats.channel_failure_closes_sent);
                    }
                }
                Err(_) => bump(&mut self.stats.reply_errors),
            }
        }
        self.remove_peer(address);
        bump(&mut self.stats.peers_closed_by_channel_failure);
        true
    }

    fn reply_for(&mut self, address: SocketAddrV4, message: &ControlMessage) -> Option<Reply> {
        match message {
            ControlMessage::Hello { version, token, .. } => {
                match self.config.local_network_version {
                    Some(local) if local != *version => Some(Reply::Upgrade(local)),
                    _ if token.declared_units() < CHALLENGE_TOKEN_THRESHOLD => {
                        Some(Reply::Challenge)
                    }
                    _ => {
                        bump(&mut self.stats.hello_unanswered);
                        None
                    }
                }
            }
            ControlMessage::Login {
                url, unique_net_id, ..
            } => {
                bump(&mut self.stats.logins_received);
                let count = self
                    .stats
                    .login_net_id_forms
                    .entry(net_id_form(&unique_net_id.value))
                    .or_default();
                *count = count.saturating_add(1);
                if !self.admitting {
                    bump(&mut self.stats.refused_draining);
                    return Some(Reply::Failure(SHUTTING_DOWN_MESSAGE));
                }
                let Some(admission) = &self.config.admission else {
                    return Some(Reply::Failure(ADMISSION_DISABLED_MESSAGE));
                };
                let occupancy = self.occupancy(Some(address));
                let options = url.units();
                match admission.policy.pre_login(
                    options,
                    unique_net_id,
                    occupancy,
                    admission.default_subsystem,
                    admission.compatible_subsystems,
                    admission.indexed_subsystems,
                ) {
                    Ok(()) => {
                        let spectator = int_option(options, "SpectatorOnly") == 1;
                        if let Some(entry) = self.peers.get_mut(&address) {
                            entry.login_url_units = options.to_vec();
                            entry.login_unique_id = Some(unique_net_id.clone());
                        }
                        Some(Reply::Welcome { spectator })
                    }
                    Err(reason) => Some(Reply::Failure(reason)),
                }
            }
            ControlMessage::Netspeed { rate } => {
                let admission = self.config.admission?;
                let entry = self.peers.get_mut(&address)?;
                entry.current_net_speed = Some(clamp_client_netspeed(
                    *rate as i32,
                    admission.max_client_rate,
                ));
                bump(&mut self.stats.netspeeds_updated);
                None
            }
            ControlMessage::Join | ControlMessage::SplitJoin { .. }
                if self.config.admission.is_some() && !self.admitting =>
            {
                bump(&mut self.stats.refused_draining);
                Some(Reply::Failure(SHUTTING_DOWN_MESSAGE))
            }
            ControlMessage::Join => {
                let admission = self.config.admission?;
                let occupancy = self.occupancy(Some(address));
                let entry = self.peers.get_mut(&address)?;
                if !entry.admission.on_join_spawned() {
                    bump(&mut self.stats.duplicate_joins_ignored);
                    return None;
                }
                let options = entry.login_url_units.clone();
                let ctx = JoinSpawnContext {
                    has_game_session: admission.has_game_session,
                    can_spawn_controller: admission.can_spawn_controller,
                    has_player_state: admission.has_player_state,
                    has_player_start: admission.has_player_start,
                    must_spectate: admission.must_spectate,
                    default_player_name: admission.default_player_name,
                    player_id: self.next_player_id,
                    net_player_index: 0,
                };
                let result = admission
                    .policy
                    .login_and_init_new_player(&options, occupancy, ctx);
                self.consume_player_id(&result);
                match result {
                    Ok(outcome) => {
                        let connection_id = self.next_connection_id;
                        self.next_connection_id = self.next_connection_id.wrapping_add(1).max(1);
                        if let Some(entry) = self.peers.get_mut(&address) {
                            entry.player = Some(RegisteredPlayer {
                                player_id: ctx.player_id,
                                name_units: outcome.player_name_units,
                            });
                            entry.connection_id = Some(connection_id);
                        }
                        bump(&mut self.stats.joins_succeeded);
                        self.player_controller_open
                            .is_some()
                            .then_some(Reply::OpenPlayerController)
                    }
                    Err(reason) => Some(Reply::Failure(reason)),
                }
            }
            ControlMessage::SplitJoin { url, .. } => {
                let admission = self.config.admission?;
                let occupancy = self.occupancy(None);
                let entry = self.peers.get(&address)?;
                let next_split = entry.splitscreen_count.saturating_add(1);
                let ctx = JoinSpawnContext {
                    has_game_session: admission.has_game_session,
                    can_spawn_controller: admission.can_spawn_controller,
                    has_player_state: admission.has_player_state,
                    has_player_start: admission.has_player_start,
                    must_spectate: admission.must_spectate,
                    default_player_name: admission.default_player_name,
                    player_id: self.next_player_id,
                    net_player_index: next_split,
                };
                let result =
                    admission
                        .policy
                        .login_and_init_new_player(url.units(), occupancy, ctx);
                self.consume_player_id(&result);
                match result {
                    Ok(_) => {
                        if let Some(entry) = self.peers.get_mut(&address) {
                            entry.splitscreen_count = next_split;
                        }
                        bump(&mut self.stats.joins_succeeded);
                        None
                    }
                    Err(reason) => Some(Reply::Failure(reason)),
                }
            }
            ControlMessage::Swap { value } => {
                self.config.admission?;
                let entry = self.peers.get(&address)?;
                if evaluate_pc_swap_target(*value as i32, usize::from(entry.splitscreen_count))
                    .is_some()
                {
                    bump(&mut self.stats.swaps_received);
                } else {
                    bump(&mut self.stats.invalid_swaps_ignored);
                }
                None
            }
            ControlMessage::Debug { .. } => {
                self.config.admission?;
                bump(&mut self.stats.debug_texts_received);
                None
            }
            _ => None,
        }
    }

    /// `AGameModeBase::InitNewPlayer` registers the player (taking the next
    /// PlayerId) before its player-start check, so an admitted Join and one
    /// refused only for a missing player start both consume an ID (F-784).
    fn consume_player_id<T>(&mut self, result: &Result<T, &'static str>) {
        if matches!(result, Ok(_) | Err(FIND_PLAYER_START_ERROR)) {
            self.next_player_id = self.next_player_id.wrapping_add(1);
        }
    }

    /// Build and send one reply; a closing reply then removes the peer.
    /// Returns `false` when the peer is gone or the reply could not be built.
    fn send_reply(&mut self, address: SocketAddrV4, reply: &Reply) -> bool {
        let message = match reply {
            Reply::OpenPlayerController => return self.send_player_controller_open(address),
            Reply::Challenge => lab_challenge().map(|token| ControlMessage::Challenge { token }),
            Reply::Upgrade(version) => Ok(ControlMessage::Upgrade { version: *version }),
            Reply::Welcome { .. } => {
                let Some(admission) = &self.config.admission else {
                    return false;
                };
                ArchiveText::text(admission.map_url).and_then(|map_url| {
                    ArchiveText::text(admission.game_name).and_then(|game_name| {
                        ArchiveText::text(admission.redirect_url).map(|redirect_url| {
                            ControlMessage::Welcome {
                                map_url,
                                game_name,
                                redirect_url,
                            }
                        })
                    })
                })
            }
            Reply::Failure(reason) => {
                ArchiveText::text(reason).map(|message| ControlMessage::Failure { message })
            }
        };
        let Some(entry) = self.peers.get_mut(&address) else {
            return false;
        };
        let datagram = match message.and_then(|message| entry.peer.reliable_control_packet(message))
        {
            Ok(datagram) => datagram,
            Err(_) => {
                bump(&mut self.stats.reply_errors);
                if entry.peer.is_failed() {
                    self.remove_peer(address);
                    bump(&mut self.stats.peers_failed);
                }
                return false;
            }
        };
        // The reply datagram carries the ack state.
        entry.ack_pending = false;
        match reply {
            Reply::Challenge => entry.admission.on_challenge_sent(),
            Reply::Welcome { spectator } => {
                entry.spectator = *spectator;
                entry.admission.on_welcome_sent();
            }
            Reply::Upgrade(_) | Reply::Failure(_) => {
                self.remove_peer(address);
                bump(&mut self.stats.peers_closed);
            }
            Reply::OpenPlayerController => {}
        }
        if self.send(&datagram, address) {
            bump(match reply {
                Reply::Challenge => &mut self.stats.control_challenges_sent,
                Reply::Upgrade(_) => &mut self.stats.upgrades_sent,
                Reply::Welcome { .. } => &mut self.stats.welcomes_sent,
                Reply::Failure(_) => &mut self.stats.failures_sent,
                Reply::OpenPlayerController => &mut self.stats.player_controller_opens_sent,
            });
        }
        true
    }

    /// Compose and send the joined peer's PlayerController open. GUIDs and
    /// export state are committed only once the datagram is built; a failure
    /// counts `player_controller_open_errors` and removes a failed peer.
    fn send_player_controller_open(&mut self, address: SocketAddrV4) -> bool {
        let Some(plan) = &self.player_controller_open else {
            return false;
        };
        let Some(entry) = self.peers.get_mut(&address) else {
            return false;
        };
        let mut cache = self.guid_cache.clone();
        let mut exports = entry.exports.clone();
        let sent = compose_player_controller_open(
            &mut cache,
            &mut exports,
            plan,
            0,
            entry.peer.connection_byte_order(),
        )
        .and_then(|open| {
            entry
                .peer
                .reliable_actor_open_datagrams(&open.bunch, std::slice::from_ref(&open.exports))
                .map(|sent| (open.guids, sent))
        });
        let (guids, sent) = match sent {
            Ok(sent) => sent,
            Err(_) => {
                bump(&mut self.stats.player_controller_open_errors);
                if entry.peer.is_failed() {
                    self.remove_peer(address);
                    bump(&mut self.stats.peers_failed);
                }
                return false;
            }
        };
        entry.player_controller_channel = Some(plan.channel_index);
        commit_exports(&mut exports, &sent);
        entry.exports = exports;
        entry.ack_pending = false;
        self.guid_cache = cache;
        if self.send_all(&sent, address) {
            bump(&mut self.stats.player_controller_opens_sent);
        }
        if self.pawn_open.is_some() {
            self.send_pawn_open(address, guids.actor, guids.player_state);
        }
        if self.game_state_open.is_some() {
            self.send_game_state_open(address);
        }
        if self.player_state_open.is_some() {
            self.send_player_state_open(address, guids.actor, guids.player_state);
        }
        let mut extra_opens: Vec<(u32, bool, usize)> = self
            .manager_opens
            .iter()
            .enumerate()
            .map(|(index, (.., channel))| (*channel, false, index))
            .chain(
                self.quest_opens
                    .iter()
                    .enumerate()
                    .map(|(index, (_, channel))| (*channel, true, index)),
            )
            .collect();
        extra_opens.sort_unstable_by_key(|&(channel, ..)| channel);
        for (_, is_quest, index) in extra_opens {
            if is_quest {
                self.send_quest_open(address, index);
            } else {
                self.send_manager_open(address, index);
            }
        }
        self.refresh_members_after_join(address);
        true
    }

    /// Compose and send the player's pawn open (F-797) on its owner's
    /// connection, committing GUID and export state only once every datagram
    /// is built; a failure counts `pawn_open_errors`.
    fn send_pawn_open(&mut self, address: SocketAddrV4, controller: u32, player_state: u32) {
        let (Some((plan, channel, content)), Some(controller_plan)) =
            (&self.pawn_open, &self.player_controller_open)
        else {
            return;
        };
        let Some(entry) = self.peers.get_mut(&address) else {
            return;
        };
        let Some(connection) = entry.connection_id else {
            return;
        };
        let mut content = content.clone();
        let mut claimed = None;
        if let Some(library) = &mut self.actor_uids {
            let grant = self.next_pawn_object_id.and_then(|desired| {
                self.next_pawn_object_id = desired.checked_add(1);
                library.try_claim(desired, None).ok()
            });
            match grant {
                Some(uid) if uid > 0 => {
                    content.icarus_uid = Some(uid);
                    claimed = Some(uid);
                }
                other => {
                    if let Some(uid) = other {
                        library.free(uid);
                    }
                    bump(&mut self.stats.pawn_open_errors);
                    return;
                }
            }
        }
        let mut cache = self.guid_cache.clone();
        let mut exports = entry.exports.clone();
        let state = PawnOpenState {
            key: pawn_key(connection),
            controller,
            player_state,
            location: controller_plan.spawn_location,
            content,
        };
        let sent = compose_pawn_open(
            &mut cache,
            &mut exports,
            plan,
            &state,
            *channel,
            entry.peer.connection_byte_order(),
        )
        .and_then(|open| {
            let export_payloads: Vec<Payload> = open.exports.into_iter().collect();
            entry
                .peer
                .reliable_actor_open_datagrams(&open.bunch, &export_payloads)
        });
        let sent = match sent {
            Ok(sent) => sent,
            Err(_) => {
                bump(&mut self.stats.pawn_open_errors);
                if let (Some(uid), Some(library)) = (claimed, &mut self.actor_uids) {
                    library.free(uid);
                }
                if entry.peer.is_failed() {
                    self.remove_peer(address);
                    bump(&mut self.stats.peers_failed);
                }
                return;
            }
        };
        entry.pawn_uid = claimed;
        commit_exports(&mut exports, &sent);
        entry.exports = exports;
        entry.ack_pending = false;
        self.guid_cache = cache;
        if self.send_all(&sent, address) {
            bump(&mut self.stats.pawn_opens_sent);
        }
    }

    /// Compose and send the GameState open (F-794) over as many datagrams as
    /// it needs, committing GUID and export state only once every datagram
    /// is built; a failure counts `game_state_open_errors`.
    fn send_game_state_open(&mut self, address: SocketAddrV4) {
        let Some(session) = &self.game_state_open else {
            return;
        };
        let Some(entry) = self.peers.get_mut(&address) else {
            return;
        };
        let mut cache = self.guid_cache.clone();
        let mut exports = entry.exports.clone();
        let world_seconds = self.started.elapsed().as_secs_f32();
        let state = match session.open_state(&session.state, world_seconds) {
            Ok(state) => state,
            Err(_) => {
                bump(&mut self.stats.game_state_open_errors);
                return;
            }
        };
        let sent = compose_game_state_open(
            &mut cache,
            &mut exports,
            &session.plan,
            &state,
            session.channel_index,
            entry.peer.connection_byte_order(),
        )
        .and_then(|open| {
            let export_payloads: Vec<Payload> = open.exports.into_iter().collect();
            entry
                .peer
                .reliable_actor_open_datagrams(&open.bunch, &export_payloads)
        });
        let sent = match sent {
            Ok(sent) => sent,
            Err(_) => {
                bump(&mut self.stats.game_state_open_errors);
                if entry.peer.is_failed() {
                    self.remove_peer(address);
                    bump(&mut self.stats.peers_failed);
                }
                return;
            }
        };
        commit_exports(&mut exports, &sent);
        entry.exports = exports;
        entry.ack_pending = false;
        // The open's values are what the connection has; a lost open is
        // resent whole, as it is reliable.
        let mut replica = GameStateReplica {
            world_time_at: world_seconds,
            ..GameStateReplica::default()
        };
        for handle in update_handles() {
            // Absent from an open: the class default.
            if let Some(value) = state
                .values
                .get(&handle)
                .cloned()
                .or_else(|| class_default(handle))
            {
                replica.sent.insert(handle, value);
            }
        }
        entry.game_state = Some(replica);
        self.guid_cache = cache;
        if self.send_all(&sent, address) {
            bump(&mut self.stats.game_state_opens_sent);
        }
    }

    /// Compose and send the PlayerState open the PlayerController references
    /// (F-784), committing GUID, export and host state only once the datagram
    /// is built; a failure counts `player_state_open_errors`.
    fn send_player_state_open(&mut self, address: SocketAddrV4, owner: u32, player_state: u32) {
        let (Some(plan), Some(admission)) = (&self.player_state_open, &self.config.admission)
        else {
            return;
        };
        // The original sets bIsHost at the PlayerState's BeginPlay when the
        // first player controller's character is this PlayerState's pawn,
        // which does not exist yet (F-913). On the lab every joined player
        // has a pawn, so a player is host when no other player is joined.
        let is_host = !self.peers.iter().any(|(other, peer)| {
            *other != address && peer.admission.state == ClientLoginState::Joined
        });
        let Some(entry) = self.peers.get_mut(&address) else {
            return;
        };
        let mut cache = self.guid_cache.clone();
        let mut exports = entry.exports.clone();
        let sent = (|| {
            let player = entry
                .player
                .as_ref()
                .context("no admitted player for the PlayerState")?;
            let login_id = entry
                .login_unique_id
                .as_ref()
                .context("no login unique net id")?;
            let state = PlayerStateOpenState {
                player_state,
                owner,
                player_id: player.player_id,
                unique_id: replicated_unique_net_id(login_id, admission.indexed_subsystems),
                player_name: ArchiveText::new(&player.name_units)?,
                is_host,
            };
            let open = compose_player_state_open(
                &mut cache,
                &mut exports,
                plan,
                &state,
                entry.peer.connection_byte_order(),
            )?;
            let export_payloads: Vec<Payload> = open.exports.into_iter().collect();
            entry
                .peer
                .reliable_actor_open_datagrams(&open.bunch, &export_payloads)
        })();
        let sent = match sent {
            Ok(sent) => sent,
            Err(_) => {
                bump(&mut self.stats.player_state_open_errors);
                if entry.peer.is_failed() {
                    self.remove_peer(address);
                    bump(&mut self.stats.peers_failed);
                }
                return;
            }
        };
        entry.player_state_channel = Some(plan.channel_index);
        commit_exports(&mut exports, &sent);
        entry.exports = exports;
        entry.ack_pending = false;
        self.guid_cache = cache;
        if self.send_all(&sent, address) {
            bump(&mut self.stats.player_state_opens_sent);
        }
    }

    /// Compose and send world manager `index` (F-859), committing GUID and
    /// export state only once every datagram is built; a failure counts
    /// `manager_open_errors`.
    fn send_manager_open(&mut self, address: SocketAddrV4, index: usize) {
        let Some((key, plan, channel)) = self.manager_opens.get(index) else {
            return;
        };
        let Some(entry) = self.peers.get_mut(&address) else {
            return;
        };
        let mut cache = self.guid_cache.clone();
        let mut exports = entry.exports.clone();
        let sent = compose_manager_open(
            &mut cache,
            &mut exports,
            plan,
            *key,
            *channel,
            entry.peer.connection_byte_order(),
        )
        .and_then(|open| {
            let export_payloads: Vec<Payload> = open.exports.into_iter().collect();
            entry
                .peer
                .reliable_actor_open_datagrams(&open.bunch, &export_payloads)
        });
        let sent = match sent {
            Ok(sent) => sent,
            Err(_) => {
                bump(&mut self.stats.manager_open_errors);
                if entry.peer.is_failed() {
                    self.remove_peer(address);
                    bump(&mut self.stats.peers_failed);
                }
                return;
            }
        };
        commit_exports(&mut exports, &sent);
        entry.exports = exports;
        entry.ack_pending = false;
        self.guid_cache = cache;
        if self.send_all(&sent, address) {
            bump(&mut self.stats.manager_opens_sent);
        }
    }

    /// Compose and send saved quest actor `index` (`C019`, `F-934`),
    /// committing GUID and export state only once every datagram is built; a
    /// failure counts `quest_open_errors`.
    fn send_quest_open(&mut self, address: SocketAddrV4, index: usize) {
        let Some((plan, channel)) = self.quest_opens.get(index) else {
            return;
        };
        let Some(entry) = self.peers.get_mut(&address) else {
            return;
        };
        let mut cache = self.guid_cache.clone();
        let mut exports = entry.exports.clone();
        let sent = compose_quest_open(
            &mut cache,
            &mut exports,
            plan,
            *channel,
            entry.peer.connection_byte_order(),
        )
        .and_then(|open| {
            let export_payloads: Vec<Payload> = open.exports.into_iter().collect();
            entry
                .peer
                .reliable_actor_open_datagrams(&open.bunch, &export_payloads)
        });
        let sent = match sent {
            Ok(sent) => sent,
            Err(_) => {
                bump(&mut self.stats.quest_open_errors);
                if entry.peer.is_failed() {
                    self.remove_peer(address);
                    bump(&mut self.stats.peers_failed);
                }
                return;
            }
        };
        commit_exports(&mut exports, &sent);
        entry.exports = exports;
        entry.ack_pending = false;
        self.guid_cache = cache;
        if self.send_all(&sent, address) {
            bump(&mut self.stats.quest_opens_sent);
        }
    }

    fn send_acks(&mut self) {
        let mut failed = Vec::new();
        let mut acks = Vec::new();
        for (address, entry) in &mut self.peers {
            if !std::mem::take(&mut entry.ack_pending) {
                continue;
            }
            match entry.peer.ack_packet() {
                Ok(datagram) => acks.push((*address, datagram)),
                Err(_) => failed.push(*address),
            }
        }
        for address in failed {
            self.remove_peer(address);
            bump(&mut self.stats.ack_errors);
            bump(&mut self.stats.peers_failed);
        }
        for (address, datagram) in acks {
            if self.send(&datagram, address) {
                bump(&mut self.stats.acks_sent);
            }
        }
    }

    /// Send every datagram of an open; true when all went out whole.
    fn send_all(&mut self, sent: &ActorOpenSent, address: SocketAddrV4) -> bool {
        let mut delivered = true;
        for datagram in &sent.datagrams {
            delivered &= self.send(datagram, address);
        }
        delivered
    }

    fn send(&mut self, datagram: &[u8], address: SocketAddrV4) -> bool {
        match self.socket.send_to(datagram, address) {
            Ok(sent) if sent == datagram.len() => true,
            _ => {
                bump(&mut self.stats.io_errors);
                false
            }
        }
    }

    /// One bounded update: expire idle peers, process at most
    /// [`PACKETS_PER_UPDATE`] datagrams, then send pending acks. The
    /// [`PeerLabPlugin`] runs this once per app update.
    pub fn poll(&mut self) {
        self.expire_idle(Instant::now());
        let max = self.config.max_datagram_bytes();
        for _ in 0..PACKETS_PER_UPDATE {
            let (length, source) = match self.socket.recv_from(&mut self.buffer) {
                Ok(received) => received,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                Err(_) => {
                    bump(&mut self.stats.io_errors);
                    break;
                }
            };
            bump(&mut self.stats.received);
            let SocketAddr::V4(address) = source else {
                bump(&mut self.stats.non_ipv4);
                continue;
            };
            if length > max {
                bump(&mut self.stats.oversize);
                continue;
            };
            let now = Instant::now();
            if self.handshake(address, length, now) {
                continue;
            }
            if decode(&self.buffer[..length]).is_ok() {
                // An exact short handshake form the authority did not answer.
                bump(&mut self.stats.handshake_rejected);
                continue;
            }
            self.data(address, length, now);
        }
        self.send_acks();
        self.update_game_state();
    }

    /// One GameState update pass (module docs): advance the clock to the
    /// world time, then, at most every [`GAME_STATE_UPDATE_SECONDS`], send
    /// each connection whose GameState open is acked the values it lacks.
    fn update_game_state(&mut self) {
        use super::game_state_open::handle;
        let now = Instant::now();
        let world_seconds = now.duration_since(self.started).as_secs_f32();
        let Some(session) = &mut self.game_state_open else {
            return;
        };
        if let Some(clock) = &mut session.clock {
            clock.advance(now.duration_since(session.clock_at).as_secs_f32());
            session.clock_at = now;
        }
        if let Some(meter) = &mut session.fps {
            meter.observe(now.duration_since(session.polled_at).as_secs_f32());
            session.polled_at = now;
        }
        if session.updated_at.is_some_and(|updated| {
            now.duration_since(updated).as_secs_f32() < GAME_STATE_UPDATE_SECONDS
        }) {
            return;
        }
        session.updated_at = Some(now);
        if let Some(status) = &mut session.networking {
            // GatherNetworkingStatus lists the player array's players.
            let mut joined: Vec<(u32, Vec<u16>, u8)> = self
                .peers
                .values()
                .filter_map(|entry| {
                    Some((
                        entry.connection_id?,
                        entry.player.as_ref()?.name_units.clone(),
                        entry.ping.ping(),
                    ))
                })
                .collect();
            joined.sort_unstable_by_key(|(connection, ..)| *connection);
            status.player_ping = joined.iter().map(|(.., ping)| *ping).collect();
            status.player_names = joined.into_iter().map(|(_, name, _)| name).collect();
        }
        let mut changing = vec![
            (
                handle::TIME_OF_DAY,
                session
                    .clock
                    .as_ref()
                    .map(|clock| RepValue::Float(clock.time_of_day().to_bits())),
            ),
            (
                handle::SERVER_FPS,
                session.fps.as_ref().map(|meter| RepValue::Int(meter.fps())),
            ),
            (
                handle::LEVEL_TIME_ELAPSED_SEC,
                session.level_time.map(|restored| {
                    RepValue::Int(super::game_state_open::level_time_elapsed_sec(
                        restored,
                        world_seconds,
                    ))
                }),
            ),
            (
                handle::LOCKED_MISSIONS,
                locked_missions(session, world_seconds)
                    .and_then(|value| value.or_else(|| class_default(handle::LOCKED_MISSIONS))),
            ),
        ];
        match session.networking.as_ref().map(NetworkingStatus::values) {
            Some(Ok(values)) => changing.extend(values.into_iter().map(|(h, v)| (h, Some(v)))),
            Some(Err(_)) => bump(&mut self.stats.game_state_update_errors),
            None => {}
        }
        if let Some(values) = &session.active_prospect {
            changing.extend(
                values
                    .iter()
                    .map(|(handle, value)| (*handle, Some(value.clone()))),
            );
        }
        let (channel, layout, engine) = (
            session.channel_index,
            &session.plan.layout,
            session.plan.engine_net_version,
        );
        let mut outgoing = Vec::new();
        let mut failed = Vec::new();
        for (address, entry) in &mut self.peers {
            let Some(replica) = &entry.game_state else {
                continue;
            };
            if !entry.peer.actor_channel_settled(channel) {
                continue;
            }
            let values = replica.due(world_seconds, &changing);
            if values.is_empty() {
                continue;
            }
            let order = entry.peer.connection_byte_order();
            // An update over one bunch (a long NetworkingStatus) goes out as
            // unreliable partial bunches in consecutive datagrams.
            let mut sent = compose_game_state_update(layout, engine, &values, order)
                .and_then(|payload| entry.peer.actor_update_datagrams(channel, payload));
            // Large status/prospect fields must not hold back clocks under
            // bunch-size or sent-ring pressure. Try each group independently
            // from the original due map, then clocks alone; omitted fields
            // stay due. Failed framing attempts are transactional.
            let mut sent_values = None;
            let mut counted = false;
            for (omit_status, omit_prospect) in [(true, false), (false, true), (true, true)] {
                if sent.is_ok() || entry.peer.is_failed() {
                    break;
                }
                if !counted {
                    bump(&mut self.stats.game_state_update_errors);
                    counted = true;
                }
                let candidate: BTreeMap<_, _> = values
                    .iter()
                    .filter(|(handle, _)| {
                        !(omit_status && handle::NETWORKING_STATUS.contains(handle)
                            || omit_prospect && handle::ACTIVE_PROSPECT.contains(handle))
                    })
                    .map(|(&handle, value)| (handle, value.clone()))
                    .collect();
                if candidate.is_empty() || candidate == values {
                    continue;
                }
                sent = compose_game_state_update(layout, engine, &candidate, order)
                    .and_then(|payload| entry.peer.actor_update_datagrams(channel, payload));
                sent_values = Some(candidate);
            }
            match sent {
                Ok(datagrams) => {
                    entry.ack_pending = false;
                    outgoing.push((*address, datagrams, sent_values.unwrap_or(values)));
                }
                Err(_) => {
                    if !counted {
                        bump(&mut self.stats.game_state_update_errors);
                    }
                    if entry.peer.is_failed() {
                        failed.push(*address);
                    }
                }
            }
        }
        for address in failed {
            self.remove_peer(address);
            bump(&mut self.stats.peers_failed);
        }
        for (address, datagrams, values) in outgoing {
            // In order and back to back: the client joins the parts only on
            // the same or the next packet ID.
            let mut delivered = true;
            for (datagram, _) in &datagrams {
                if self.send(datagram, address) {
                    bump(&mut self.stats.game_state_update_datagrams);
                } else {
                    delivered = false;
                }
            }
            // Not recorded unless every part went out: the next pass builds
            // it again.
            if !delivered {
                continue;
            }
            if let Some(replica) = self
                .peers
                .get_mut(&address)
                .and_then(|entry| entry.game_state.as_mut())
            {
                let packet_ids: Vec<u32> = datagrams.iter().map(|(_, id)| *id).collect();
                replica.sent(&packet_ids, values, world_seconds);
                bump(&mut self.stats.game_state_updates_sent);
            }
        }
    }
}

/// Insert a bound [`PeerLab`] before adding this optional research plugin.
///
/// The lab polls once per app update in [`RunFixedMainLoop`] before the fixed
/// main loop, so every datagram taken in an update is processed before that
/// update's `FixedUpdate` simulation ticks. Removing the [`PeerLab`] resource
/// (teardown) stops polling.
pub struct PeerLabPlugin;

impl Plugin for PeerLabPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            RunFixedMainLoop,
            poll_peer_lab
                .run_if(resource_exists::<PeerLab>)
                .in_set(RunFixedMainLoopSystems::BeforeFixedMainLoop),
        );
    }
}

fn poll_peer_lab(mut lab: ResMut<PeerLab>) {
    lab.poll();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prospect_refresh_lab() -> PeerLab {
        use super::super::{game_state_open::GAME_STATE_CHANNEL_INDEX, release::ReleaseProfile};
        let mut lab = PeerLab::bind(
            0,
            PeerLabConfig {
                admission: Some(PeerLabAdmission::default()),
                ..Default::default()
            },
        )
        .unwrap();
        let profile = ReleaseProfile::release_3_0_32();
        let map = PeerLabAdmission::default().map_url;
        lab.enable_player_controller_open(
            profile
                .player_controller_open_plan(map, [0.0; 3])
                .unwrap()
                .unwrap(),
        )
        .unwrap();
        lab.enable_game_state_open(
            profile.game_state_open_plan(map).unwrap().unwrap(),
            GameStateOpenState {
                key: ActorKey::Session(0),
                managers: Default::default(),
                values: BTreeMap::from([(
                    super::super::game_state_open::handle::SEED,
                    RepValue::Int(7),
                )]),
                weather: Vec::new(),
            },
            GAME_STATE_CHANNEL_INDEX,
        )
        .unwrap();
        lab
    }

    #[test]
    fn template_extension_rejects_prospect_handles_atomically_after_refresh() {
        use super::super::{game_state_open::handle, prospect_info::ProspectInfo};
        let mut lab = prospect_refresh_lab();
        // Bootstrap callers may still supply prospect handles before the
        // first typed refresh establishes the authoritative current values.
        lab.extend_game_state_values(BTreeMap::from([
            (handle::SEED, RepValue::Int(8)),
            (65, RepValue::Int(3)),
        ]))
        .unwrap();
        assert_eq!(
            lab.game_state_open.as_ref().unwrap().state.values.get(&65),
            Some(&RepValue::Int(3))
        );
        lab.refresh_active_prospect(&ProspectInfo {
            elapsed_time: 9,
            ..Default::default()
        })
        .unwrap();
        let before = lab.game_state_open.as_ref().unwrap().clone();
        for prospect_handle in handle::ACTIVE_PROSPECT {
            assert!(
                lab.extend_game_state_values(BTreeMap::from([
                    (handle::SEED, RepValue::Int(99)),
                    (prospect_handle, class_default(prospect_handle).unwrap()),
                ]))
                .is_err()
            );
            let after = lab.game_state_open.as_ref().unwrap();
            assert_eq!(after.state.values, before.state.values);
            assert_eq!(after.active_prospect, before.active_prospect);
        }
        lab.extend_game_state_values(BTreeMap::from([(handle::SEED, RepValue::Int(10))]))
            .unwrap();
        let after = lab.game_state_open.as_ref().unwrap();
        let mut expected = before.state.values.clone();
        expected.insert(handle::SEED, RepValue::Int(10));
        assert_eq!(after.state.values, expected);
        assert_eq!(after.active_prospect, before.active_prospect);
    }

    /// Establish a lab peer and ACK its GameState open without app clocks
    /// or Login/Join behavior. All input is independently authored.
    fn acked_game_state_peer(
        lab: &mut PeerLab,
    ) -> (
        UdpSocket,
        SocketAddrV4,
        super::super::sequencing::PacketSequencer,
    ) {
        use super::super::{
            handshake::initial_request,
            packet::{Packet, Receiver, read_packet, write_packet},
            packet_lab::client_cookie_seeds,
            sequencing::PacketSequencer,
        };
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        socket.connect(lab.local_addr().unwrap()).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        let SocketAddr::V4(address) = socket.local_addr().unwrap() else {
            unreachable!("IPv4 loopback")
        };
        let mut buffer = [0; 2048];
        socket.send(&initial_request()).unwrap();
        lab.poll();
        let length = socket.recv(&mut buffer).unwrap();
        let cookie = decode(&buffer[..length]).unwrap().cookie;
        socket.send(&buffer[..length]).unwrap();
        lab.poll();
        let length = socket.recv(&mut buffer).unwrap();
        assert_eq!(decode(&buffer[..length]).unwrap().cookie, cookie);
        let (incoming, outgoing) = client_cookie_seeds(&cookie);
        let mut client = PacketSequencer::new(incoming, outgoing, 64).unwrap();
        lab.send_game_state_open(address);
        let entry = &lab.peers[&address];
        assert!(entry.game_state.is_some());
        let count = entry.peer.sequencer().notify().sent_records().len();
        assert!(count > 0);
        let to_client = super::super::packet::PacketConfig {
            receiver: Receiver::Client,
            ..lab.config.packet.packet
        };
        for _ in 0..count {
            let length = socket.recv(&mut buffer).unwrap();
            let packet = read_packet(&buffer[..length], &to_client).unwrap();
            let received = client.receive(&packet.header).unwrap().unwrap();
            client.ack_seq(received.sequence, true).unwrap();
        }
        let ack = write_packet(
            &Packet {
                magic: 0,
                header: client.header().unwrap(),
                info: None,
                bunches: Vec::new(),
            },
            &lab.config.packet.packet,
        )
        .unwrap();
        client.commit().unwrap();
        let channel = lab.game_state_open.as_ref().unwrap().channel_index;
        let entry = lab.peers.get_mut(&address).unwrap();
        entry.peer.receive(&ack).unwrap();
        assert!(entry.peer.actor_channel_settled(channel));
        assert!(entry.peer.sequencer().notify().sent_records().is_empty());
        (socket, address, client)
    }

    fn receive_game_state_update(
        socket: &UdpSocket,
        lab: &PeerLab,
        mut client: Option<&mut super::super::sequencing::PacketSequencer>,
    ) -> BTreeMap<u32, RepValue> {
        use super::super::{
            actor_channel::read_actor_bunch_body,
            archive::ByteOrder,
            packet::{PacketConfig, Receiver, read_packet},
            rep_layout::{RepLayoutCodec, RepLayoutLimits, read_rep_properties},
            sequencing::{
                MAX_CONSTRUCTED_PARTIAL_BUNCH_BYTES, PartialBunchReassembler, PartialReceive,
            },
        };
        let session = lab.game_state_open.as_ref().unwrap();
        let mut buffer = [0; 2048];
        let mut reassembler = PartialBunchReassembler::new();
        let mut packet_id: Option<(u16, i64)> = None;
        let mut complete = None;
        for _ in 0..32 {
            let length = socket.recv(&mut buffer).unwrap();
            let packet = read_packet(
                &buffer[..length],
                &PacketConfig {
                    receiver: Receiver::Client,
                    ..lab.config.packet.packet
                },
            )
            .unwrap();
            if let Some(client) = client.as_mut() {
                let received = client.receive(&packet.header).unwrap().unwrap();
                client.ack_seq(received.sequence, true).unwrap();
            }
            let sequence = packet.header.sequence;
            let id = packet_id.map_or(i64::from(sequence), |(last, id)| {
                id + i64::from(sequence.wrapping_sub(last) & 0x3fff)
            });
            packet_id = Some((sequence, id));
            assert_eq!(packet.bunches.len(), 1, "one GameState update part");
            let bunch = packet.bunches.into_iter().next().unwrap();
            assert_eq!(bunch.header.channel_index, session.channel_index);
            assert!(!bunch.header.open && !bunch.header.reliable);
            if !bunch.header.partial {
                complete = Some(bunch);
                break;
            }
            if let PartialReceive::Complete { bunch, .. } = reassembler
                .receive_part(
                    id,
                    &bunch.header,
                    &bunch.payload,
                    Some(MAX_CONSTRUCTED_PARTIAL_BUNCH_BYTES),
                )
                .unwrap()
            {
                complete = Some(bunch);
                break;
            }
        }
        let bunch = complete.expect("bounded GameState update completes");
        let codec = RepLayoutCodec {
            actor: ActorChannelCodec {
                engine_net_version: session.plan.engine_net_version,
                exports_enabled: false,
                has_level: true,
                order: ByteOrder::Little,
                limits: ActorChannelLimits::default(),
            },
            checksums: false,
            limits: RepLayoutLimits::default(),
        };
        let body = read_actor_bunch_body(&mut bunch.payload.reader(), false, &codec.actor).unwrap();
        assert_eq!(body.blocks.len(), 1);
        read_rep_properties(
            &mut body.blocks[0].payload.reader(),
            &session.plan.layout,
            &codec,
        )
        .unwrap()
        .entries
        .into_iter()
        .map(|entry| (entry.handle, entry.value))
        .collect()
    }

    #[test]
    fn update_fallback_preserves_each_viable_group_and_retries_omitted_fields() {
        use super::super::{
            game_state_open::handle, prospect_info::ProspectInfo, time_of_day::RateCurve,
        };
        for oversized_prospect in [true, false] {
            let mut lab = prospect_refresh_lab();
            lab.refresh_active_prospect(&ProspectInfo::default())
                .unwrap();
            lab.enable_networking_status(NetworkingStatus::local(600.0))
                .unwrap();
            lab.enable_time_of_day(TimeOfDayClock::new(
                RateCurve::constant(60.0).unwrap(),
                600.0,
            ))
            .unwrap();
            lab.set_game_state_level_time(1_000).unwrap();
            let (socket, address, _) = acked_game_state_peer(&mut lab);
            let before = lab.peers[&address]
                .game_state
                .as_ref()
                .unwrap()
                .sent
                .clone();
            lab.refresh_active_prospect(&ProspectInfo {
                lobby_name: vec![65],
                elapsed_time: 9,
                insurance: true,
                ..Default::default()
            })
            .unwrap();
            let session = lab.game_state_open.as_mut().unwrap();
            let status = session.networking.as_mut().unwrap();
            status.current_host_player_name = vec![66];
            status.saved_locally = false;
            let prospect = session.active_prospect.as_ref().unwrap().clone();
            let status_values = status.values().unwrap();
            let (omitted, retained, repaired) = if oversized_prospect {
                // Model state that has grown past framing limits after
                // validation, without exposing an invalid public refresh.
                session
                    .active_prospect
                    .as_mut()
                    .unwrap()
                    .insert(56, RepValue::Str(ArchiveText::new(&[65; 9_000]).unwrap()));
                (handle::ACTIVE_PROSPECT, status_values, prospect)
            } else {
                status.current_host_player_name = vec![66; 9_000];
                (handle::NETWORKING_STATUS, prospect, status_values)
            };
            let earlier = Instant::now() - Duration::from_secs(2);
            lab.started = earlier;
            session.clock_at = earlier;
            session.updated_at = None;
            let errors = lab.stats.game_state_update_errors;
            let packets = lab.peers[&address].peer.sequencer().out_packet_id();
            lab.update_game_state();
            let update = receive_game_state_update(&socket, &lab, None);
            let retained_due: BTreeMap<_, _> = retained
                .into_iter()
                .filter(|(handle, value)| before.get(handle) != Some(value))
                .collect();
            assert!(!retained_due.is_empty());
            let mut expected = retained_due;
            for clock in [
                handle::WORLD_TIME_SECONDS,
                handle::TIME_OF_DAY,
                handle::LEVEL_TIME_ELAPSED_SEC,
            ] {
                expected.insert(
                    clock,
                    update.get(&clock).expect("clock survives fallback").clone(),
                );
            }
            assert_eq!(update, expected);
            assert!(update.keys().all(|handle| !omitted.contains(handle)));
            assert_eq!(lab.stats.game_state_update_errors, errors + 1);
            let entry = &lab.peers[&address];
            assert!(!entry.peer.is_failed());
            assert_eq!(entry.peer.sequencer().out_packet_id(), packets + 1);
            let replica = entry.game_state.as_ref().unwrap();
            let mut expected_sent = before.clone();
            expected_sent.extend(update.clone());
            assert_eq!(replica.sent, expected_sent);
            assert_eq!(replica.in_flight.len(), 1);
            assert_eq!(
                replica.in_flight.values().next().unwrap().1,
                update.keys().copied().collect::<Vec<_>>()
            );
            // Remove the framing pressure and force the next paced pass.
            // The omitted group must arrive without another typed refresh.
            let session = lab.game_state_open.as_mut().unwrap();
            if oversized_prospect {
                session.active_prospect = Some(repaired.clone());
            } else {
                session
                    .networking
                    .as_mut()
                    .unwrap()
                    .current_host_player_name = vec![66];
            }
            session.updated_at = None;
            lab.update_game_state();
            let retry = receive_game_state_update(&socket, &lab, None);
            let repaired_due: BTreeMap<_, _> = repaired
                .into_iter()
                .filter(|(handle, value)| before.get(handle) != Some(value))
                .collect();
            assert!(!repaired_due.is_empty());
            assert_eq!(
                retry
                    .iter()
                    .filter(|(handle, _)| omitted.contains(handle))
                    .map(|(&handle, value)| (handle, value.clone()))
                    .collect::<BTreeMap<_, _>>(),
                repaired_due
            );
            assert!(retry.keys().all(|handle| {
                omitted.contains(handle)
                    || [
                        handle::WORLD_TIME_SECONDS,
                        handle::TIME_OF_DAY,
                        handle::LEVEL_TIME_ELAPSED_SEC,
                    ]
                    .contains(handle)
            }));
            assert_eq!(lab.stats.game_state_update_errors, errors + 1);
        }
    }

    #[test]
    fn prospect_send_pressure_leaves_clocks_running_and_same_refresh_due() {
        use super::super::{
            game_state_open::handle,
            packet::{Packet, PacketConfig, Receiver, read_packet, write_packet},
            prospect_info::ProspectInfo,
            time_of_day::RateCurve,
        };
        let mut lab = prospect_refresh_lab();
        lab.config.packet.sent_capacity = 12;
        lab.enable_time_of_day(TimeOfDayClock::new(
            RateCurve::constant(60.0).unwrap(),
            600.0,
        ))
        .unwrap();
        lab.set_game_state_level_time(1_000).unwrap();
        let (socket, address, mut client) = acked_game_state_peer(&mut lab);
        // Fill exactly six of twelve sent records with disposable ACKs.
        // The client remembers them but reports none until pressure clears.
        let to_client = PacketConfig {
            receiver: Receiver::Client,
            ..lab.config.packet.packet
        };
        for _ in 0..6 {
            let datagram = lab
                .peers
                .get_mut(&address)
                .unwrap()
                .peer
                .ack_packet()
                .unwrap();
            let packet = read_packet(&datagram, &to_client).unwrap();
            let received = client.receive(&packet.header).unwrap().unwrap();
            client.ack_seq(received.sequence, true).unwrap();
        }
        let large = ProspectInfo {
            lobby_name: vec![65; 7_000],
            elapsed_time: 10,
            ..Default::default()
        };
        lab.refresh_active_prospect(&large).unwrap();
        let mut previous_world = 0.0;
        let mut previous_clock = 600.0;
        let mut previous_level = 1_000;
        let errors = lab.stats.game_state_update_errors;
        for step in 0..4 {
            // Drive the existing private timestamps, so clock progress and
            // pacing are independent of wall-clock scheduling and sleeps.
            let now = Instant::now();
            lab.started = now - Duration::from_secs(2 + step);
            let session = lab.game_state_open.as_mut().unwrap();
            session.clock_at = now - Duration::from_secs(1);
            session.updated_at = None;
            lab.update_game_state();
            let update = receive_game_state_update(&socket, &lab, Some(&mut client));
            assert_eq!(update.len(), 3, "only the three clocks fit");
            let RepValue::Float(world) = update[&handle::WORLD_TIME_SECONDS] else {
                panic!("world time is a float")
            };
            let RepValue::Float(clock) = update[&handle::TIME_OF_DAY] else {
                panic!("time of day is a float")
            };
            let RepValue::Int(level) = update[&handle::LEVEL_TIME_ELAPSED_SEC] else {
                panic!("level time is an integer")
            };
            assert!(f32::from_bits(world) > previous_world);
            assert!(f32::from_bits(clock) > previous_clock);
            assert!(level > previous_level);
            previous_world = f32::from_bits(world);
            previous_clock = f32::from_bits(clock);
            previous_level = level;
            assert_eq!(lab.stats.game_state_update_errors, errors + step + 1);
        }
        assert_eq!(
            lab.peers[&address]
                .peer
                .sequencer()
                .notify()
                .sent_records()
                .len(),
            10
        );
        assert_eq!(lab.stats.peers_failed, 0);
        // Release every outstanding packet through the normal ACK path,
        // then deliver the same refresh with no replacement or new call.
        let ack = write_packet(
            &Packet {
                magic: 0,
                header: client.header().unwrap(),
                info: None,
                bunches: Vec::new(),
            },
            &lab.config.packet.packet,
        )
        .unwrap();
        client.commit().unwrap();
        lab.buffer[..ack.len()].copy_from_slice(&ack);
        lab.data(address, ack.len(), Instant::now());
        assert!(
            lab.peers[&address]
                .peer
                .sequencer()
                .notify()
                .sent_records()
                .is_empty()
        );
        lab.game_state_open.as_mut().unwrap().updated_at = None;
        lab.update_game_state();
        let update = receive_game_state_update(&socket, &lab, Some(&mut client));
        assert_eq!(update.get(&65), Some(&RepValue::Int(10)));
        assert!(
            matches!(update.get(&56), Some(RepValue::Str(text)) if text.units() == large.lobby_name)
        );
        assert_eq!(lab.stats.game_state_update_errors, errors + 4);
        assert_eq!(lab.stats.peers_failed, 0);
    }

    #[test]
    fn prospect_refresh_validates_before_publishing_template_or_current_values() {
        use super::super::{
            game_state_open::handle,
            prospect_info::{AssociatedMember, ProspectInfo},
            rep_layout::RepCommand,
        };
        let initial = ProspectInfo {
            elapsed_time: 123,
            insurance: true,
            ..Default::default()
        };
        let mut disabled = PeerLab::bind(0, PeerLabConfig::default()).unwrap();
        assert!(disabled.refresh_active_prospect(&initial).is_err());
        let mut lab = prospect_refresh_lab();
        lab.refresh_active_prospect(&initial).unwrap();
        let before = lab.game_state_open.as_ref().unwrap().clone();
        for invalid in [
            ProspectInfo {
                prospect_state: 8,
                ..Default::default()
            },
            ProspectInfo {
                difficulty: 255,
                ..Default::default()
            },
            ProspectInfo {
                associated_members: vec![AssociatedMember {
                    status: 13,
                    ..Default::default()
                }],
                ..Default::default()
            },
            ProspectInfo {
                lobby_name: vec![0],
                ..Default::default()
            },
            ProspectInfo {
                lobby_name: vec![65; 65_536],
                ..Default::default()
            },
            // Each string fits the actor block; their combined bytes do not.
            ProspectInfo {
                prospect_id: vec![65; 4_500],
                lobby_name: vec![66; 4_500],
                ..Default::default()
            },
            ProspectInfo {
                associated_members: vec![AssociatedMember::default(); 512],
                ..Default::default()
            },
            // ASCII text fits 8 KiB, but its framing exceeds the actor block.
            ProspectInfo {
                lobby_name: vec![65; 8_190],
                ..Default::default()
            },
        ] {
            assert!(lab.refresh_active_prospect(&invalid).is_err());
            let after = lab.game_state_open.as_ref().unwrap();
            assert_eq!(after.state.values, before.state.values);
            assert_eq!(after.active_prospect, before.active_prospect);
        }
        // A release layout with the wrong property kind also rejects
        // atomically, even though ProspectInfo itself is correctly typed.
        lab.game_state_open.as_mut().unwrap().plan.layout[50] = RepCommand::Bool;
        assert!(
            lab.refresh_active_prospect(&ProspectInfo::default())
                .is_err()
        );
        assert_eq!(
            lab.game_state_open.as_ref().unwrap().state.values,
            before.state.values
        );
        assert_eq!(
            lab.game_state_open.as_ref().unwrap().active_prospect,
            before.active_prospect
        );
        lab.game_state_open.as_mut().unwrap().plan = before.plan.clone();
        lab.game_state_open.as_mut().unwrap().plan.layout[57] = RepCommand::Enum { bits: 1 };
        assert!(
            lab.refresh_active_prospect(&super::super::prospect_info::ProspectInfo {
                prospect_state: 2,
                ..Default::default()
            })
            .is_err()
        );
        assert_eq!(
            lab.game_state_open.as_ref().unwrap().state.values,
            before.state.values
        );
        assert_eq!(
            lab.game_state_open.as_ref().unwrap().active_prospect,
            before.active_prospect
        );
        lab.game_state_open.as_mut().unwrap().plan = before.plan.clone();
        lab.refresh_active_prospect(&ProspectInfo::default())
            .unwrap();
        let session = lab.game_state_open.as_ref().unwrap();
        assert_eq!(
            session.state.values,
            BTreeMap::from([(handle::SEED, RepValue::Int(7))])
        );
        assert_eq!(session.active_prospect.as_ref().unwrap().len(), 17);
        for handle in handle::ACTIVE_PROSPECT {
            assert_eq!(
                session.active_prospect.as_ref().unwrap().get(&handle),
                class_default(handle).as_ref()
            );
        }
    }

    #[test]
    fn prospect_refresh_rejects_combined_dynamic_open_without_publishing() {
        use super::super::{archive::ByteOrder, prospect_info::ProspectInfo};
        let large = ProspectInfo {
            lobby_name: vec![65; 7_000],
            elapsed_time: 9,
            ..Default::default()
        };
        // The prospect alone fits both the update and the open codecs.
        prospect_refresh_lab()
            .refresh_active_prospect(&large)
            .unwrap();
        let mut lab = prospect_refresh_lab();
        let mut status = NetworkingStatus::local(600.0);
        status.current_host_player_name = vec![66; 2_000];
        lab.enable_networking_status(status).unwrap();
        // The dynamic status alone also fits, with a small current prospect.
        lab.refresh_active_prospect(&ProspectInfo {
            elapsed_time: 123,
            insurance: true,
            ..Default::default()
        })
        .unwrap();
        let before = lab.game_state_open.as_ref().unwrap().clone();
        let mut template = before.state.clone();
        template.values.retain(|handle, _| {
            !super::super::game_state_open::handle::ACTIVE_PROSPECT.contains(handle)
        });
        template
            .values
            .extend(super::super::game_state_open::active_prospect_values(&large).unwrap());
        let assembled = before
            .open_state(&template, lab.started.elapsed().as_secs_f32())
            .unwrap();
        for order in [ByteOrder::Little, ByteOrder::Big] {
            // This is the old dry-run: the template still fits by itself.
            compose_game_state_open(
                &mut lab.guid_cache.clone(),
                &mut ConnectionExports::new(),
                &before.plan,
                &template,
                before.channel_index,
                order,
            )
            .unwrap();
            let error = compose_game_state_open(
                &mut lab.guid_cache.clone(),
                &mut ConnectionExports::new(),
                &before.plan,
                &assembled,
                before.channel_index,
                order,
            )
            .expect_err("the combined actor content block exceeds 65,536 bits");
            assert!(format!("{error:#}").contains("payload bit limit"));
        }
        assert!(lab.refresh_active_prospect(&large).is_err());
        let after = lab.game_state_open.as_ref().unwrap();
        assert_eq!(after.state.values, before.state.values);
        assert_eq!(after.active_prospect, before.active_prospect);
    }

    #[test]
    fn advancing_clocks_does_not_refresh_prospect_values_or_member_flags() {
        use super::super::{
            prospect_info::{AssociatedMember, ProspectInfo},
            time_of_day::RateCurve,
        };
        let mut lab = prospect_refresh_lab();
        lab.enable_time_of_day(TimeOfDayClock::new(
            RateCurve::constant(60.0).unwrap(),
            600.0,
        ))
        .unwrap();
        lab.set_game_state_level_time(1_000).unwrap();
        let current = ProspectInfo {
            elapsed_time: 7,
            associated_members: vec![AssociatedMember {
                is_currently_playing: true,
                ..Default::default()
            }],
            ..Default::default()
        };
        lab.refresh_active_prospect(&current).unwrap();
        let before = lab.game_state_open.as_ref().unwrap().clone();
        let earlier = Instant::now() - Duration::from_secs(121);
        lab.started = earlier;
        lab.game_state_open.as_mut().unwrap().clock_at = earlier;
        lab.update_game_state();
        let after = lab.game_state_open.as_ref().unwrap();
        assert!(after.clock.as_ref().unwrap().time_of_day() > 600.0);
        assert_eq!(after.state.values, before.state.values);
        assert_eq!(after.active_prospect, before.active_prospect);
        // Legacy template-only initialization also stays template-only.
        lab.game_state_open.as_mut().unwrap().active_prospect = None;
        lab.game_state_open.as_mut().unwrap().updated_at = None;
        lab.update_game_state();
        assert!(
            lab.game_state_open
                .as_ref()
                .unwrap()
                .active_prospect
                .is_none()
        );
    }

    #[test]
    fn prospect_replica_repairs_latest_fields_after_partial_loss_and_supersession() {
        use super::super::prospect_info::ProspectInfo;
        let changing = |info: &ProspectInfo| {
            active_prospect_update_values(info)
                .unwrap()
                .into_iter()
                .map(|(h, v)| (h, Some(v)))
                .collect::<Vec<_>>()
        };
        let mut replica = GameStateReplica::default();
        for handle in update_handles() {
            if let Some(value) = class_default(handle) {
                replica.sent.insert(handle, value);
            }
        }
        let first = ProspectInfo {
            insurance: true,
            elapsed_time: 5,
            ..Default::default()
        };
        let latest = ProspectInfo {
            insurance: true,
            elapsed_time: 6,
            ..Default::default()
        };
        let due = replica.due(0.0, &changing(&first));
        assert_eq!(due.keys().copied().collect::<Vec<_>>(), [63, 65]);
        replica.sent(&[10, 11, 12], due, 0.0);
        replica.acked(10);
        assert!(replica.due(0.0, &changing(&first)).is_empty());
        let due = replica.due(0.0, &changing(&latest));
        assert_eq!(due.keys().copied().collect::<Vec<_>>(), [65]);
        replica.sent(&[13], due, 0.0);
        // Only insurance still depends on the old partial update. Losing
        // any unacked part repairs it, without replacing the latest elapsed.
        assert!(replica.naked(11));
        assert!(!replica.naked(12));
        let due = replica.due(0.0, &changing(&latest));
        assert_eq!(due.keys().copied().collect::<Vec<_>>(), [63]);
        replica.sent(&[14, 15], due, 0.0);
        replica.acked(13);
        replica.acked(14);
        replica.acked(15);
        assert!(!replica.naked(15));
        assert!(replica.due(0.0, &changing(&latest)).is_empty());
        // Clearing is a new value, and its lost split repairs the clear.
        let cleared = changing(&ProspectInfo::default());
        let due = replica.due(0.0, &cleared);
        replica.sent(&[16, 17], due, 0.0);
        assert!(replica.naked(17));
        assert_eq!(
            replica.due(0.0, &cleared),
            BTreeMap::from([(63, RepValue::Bool(false)), (65, RepValue::Int(0))])
        );
        // A -> B -> A is protected by packet identity, not equal values.
        replica.sent(&[18], replica.due(0.0, &changing(&first)), 0.0);
        replica.sent(&[19], replica.due(0.0, &changing(&latest)), 0.0);
        replica.sent(&[20], replica.due(0.0, &changing(&first)), 0.0);
        assert!(!replica.naked(19));
        assert!(replica.due(0.0, &changing(&first)).is_empty());
    }

    #[test]
    fn game_state_replicas_send_what_is_due_and_repair_only_the_latest() {
        use super::super::game_state_open::handle::{
            LEVEL_TIME_ELAPSED_SEC, TIME_OF_DAY, WORLD_TIME_SECONDS,
        };
        let float = |value: f32| RepValue::Float(value.to_bits());
        let mut replica = GameStateReplica {
            world_time_at: 10.0,
            ..GameStateReplica::default()
        };
        replica.sent.insert(WORLD_TIME_SECONDS, float(10.0));
        replica.sent.insert(TIME_OF_DAY, float(600.0));
        replica
            .sent
            .insert(LEVEL_TIME_ELAPSED_SEC, RepValue::Int(5));
        let changing = |tod: f32, level: i32| {
            [
                (TIME_OF_DAY, Some(float(tod))),
                (LEVEL_TIME_ELAPSED_SEC, Some(RepValue::Int(level))),
            ]
        };
        // Nothing changed and the world time is fresh: nothing is due.
        assert!(replica.due(10.05, &changing(600.0, 5)).is_empty());
        // A changed time of day alone, then the world time once 0.1 s old.
        let due = replica.due(10.05, &changing(600.5, 5));
        assert_eq!(due.keys().copied().collect::<Vec<_>>(), [TIME_OF_DAY]);
        replica.sent(&[1], due, 10.05);
        let due = replica.due(10.1, &changing(601.0, 6));
        assert_eq!(
            due.keys().copied().collect::<Vec<_>>(),
            [WORLD_TIME_SECONDS, TIME_OF_DAY, LEVEL_TIME_ELAPSED_SEC]
        );
        replica.sent(&[2], due, 10.1);
        assert_eq!(replica.world_time_at, 10.1);
        // Packet 1's handle is now carried by packet 2: its nak repairs
        // nothing, and only latest packets stay in flight.
        assert_eq!(replica.in_flight.keys().copied().collect::<Vec<_>>(), [2]);
        assert!(!replica.naked(1));
        // Losing packet 2 makes all three due again, even unchanged.
        assert!(replica.naked(2));
        assert_eq!(replica.due(10.15, &changing(601.0, 6)).len(), 3);
        // An acked packet is forgotten; its nak (never sent) changes nothing.
        let due = replica.due(10.15, &changing(601.0, 6));
        replica.sent(&[3], due, 10.15);
        replica.acked(3);
        assert!(!replica.naked(3));
        assert!(replica.due(10.2, &changing(601.0, 6)).is_empty());
    }

    #[test]
    fn a_split_update_is_lost_by_a_nak_of_any_part_and_cleared_by_all_acks() {
        use super::super::game_state_open::handle::{TIME_OF_DAY, WORLD_TIME_SECONDS};
        let float = |value: f32| RepValue::Float(value.to_bits());
        let changing = |tod: f32| [(TIME_OF_DAY, Some(float(tod)))];
        let split = |replica: &mut GameStateReplica, ids: &[u32], tod: f32, at: f32| {
            let due = replica.due(at, &changing(tod));
            assert_eq!(due.len(), 2, "the world time and the time of day");
            replica.sent(ids, due, at);
        };
        // A nak of any one part makes the values due again, once.
        for lost in [10, 11, 12] {
            let mut replica = GameStateReplica::default();
            split(&mut replica, &[10, 11, 12], 600.0, 1.0);
            assert_eq!(
                replica.in_flight.keys().copied().collect::<Vec<_>>(),
                [10, 11, 12]
            );
            assert!(replica.due(1.05, &changing(600.0)).is_empty());
            for acked in [10, 11, 12].into_iter().filter(|id| *id < lost) {
                replica.acked(acked);
            }
            assert!(replica.naked(lost), "part {lost}");
            assert!(replica.in_flight.is_empty(), "part {lost}");
            assert_eq!(
                replica
                    .due(1.05, &changing(600.0))
                    .keys()
                    .copied()
                    .collect::<Vec<_>>(),
                [WORLD_TIME_SECONDS, TIME_OF_DAY]
            );
            // The other parts' naks repair nothing more.
            for other in [10, 11, 12] {
                assert!(!replica.naked(other));
            }
        }
        // Acks of every part clear the update; a later nak changes nothing.
        let mut replica = GameStateReplica::default();
        split(&mut replica, &[20, 21], 600.0, 1.0);
        replica.acked(21);
        assert_eq!(replica.in_flight.keys().copied().collect::<Vec<_>>(), [20]);
        replica.acked(20);
        assert!(replica.in_flight.is_empty());
        assert!(!replica.naked(20) && !replica.naked(21));
        assert!(replica.due(1.05, &changing(600.0)).is_empty());
        // A newer update supersedes a split one: the older parts leave the
        // flight and their naks repair nothing.
        split(&mut replica, &[30, 31], 601.0, 2.0);
        split(&mut replica, &[32], 602.0, 3.0);
        assert_eq!(replica.in_flight.keys().copied().collect::<Vec<_>>(), [32]);
        assert!(!replica.naked(30));
        assert!(replica.due(3.05, &changing(602.0)).is_empty());
    }

    #[test]
    fn config_is_validated() {
        assert!(PeerLabConfig::default().validate().is_ok());
        let with_admission = PeerLabConfig {
            admission: Some(PeerLabAdmission::default()),
            ..PeerLabConfig::default()
        };
        assert!(with_admission.validate().is_ok());
        for max_peers in [0, MAX_PEERS_LIMIT + 1] {
            let config = PeerLabConfig {
                max_peers,
                ..PeerLabConfig::default()
            };
            assert!(config.validate().is_err(), "{max_peers}");
        }
        let config = PeerLabConfig {
            idle_timeout: Duration::ZERO,
            ..PeerLabConfig::default()
        };
        assert!(config.validate().is_err());
        for bytes in [28, MAX_UDP_PAYLOAD_BYTES + 1] {
            let mut config = PeerLabConfig::default();
            config.packet.packet.limits.max_packet_bytes = bytes;
            assert!(config.validate().is_err(), "{bytes}");
        }
        let mut config = PeerLabConfig::default();
        config.packet.sent_capacity = 0;
        assert!(config.validate().is_err());
    }
}

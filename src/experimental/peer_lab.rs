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
//!   [`PeerLab::enable_locked_missions`], F-834). An update over
//!   one bunch (a long `NetworkingStatus`) goes out through
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
    ActorBunchBody, ActorChannelCodec, ActorChannelLimits, encode_actor_bunch_body,
};
use super::admission::{
    ActorChannelFailureAction, AdmissionPolicy, ClientLoginState, ConnectionLoginAdmission,
    DEFAULT_MAX_CLIENT_RATE, FIND_PLAYER_START_ERROR, JoinSpawnContext, Occupancy,
    clamp_client_netspeed, evaluate_actor_channel_failure, evaluate_pc_swap_target, int_option,
};
use super::control_messages::{
    ArchiveText, ControlFallback, ControlMessage, Decoded, NetIdValue, UniqueNetId,
};
use super::game_state_open::{
    GameStateOpenPlan, GameStateOpenState, LockedMission, NetworkingStatus, class_default,
    compose_game_state_open, compose_game_state_update,
};
use super::handshake::decode;
use super::handshake_lab::{
    CookieAuthority, HANDSHAKE_BYTES, HandshakeReply, PACKETS_PER_UPDATE, bind_loopback,
    lab_seconds,
};
use super::packet::BunchLimits;
use super::packet::Payload;
use super::packet_lab::{
    ActorOpenSent, DeliveredSummary, LabPeer, PacketDisposition, PacketLabConfig,
};
use super::pawn_open::{PawnContent, PawnOpenPlan, PawnOpenState, compose_pawn_open};
use super::ping::PingEstimator;
use super::release::ReleaseProfile;
use super::rep_layout::RepValue;
use super::server_fps::ServerFpsMeter;
use super::session_open::{
    ActorKey, ActorOpenSpec, ConnectionExports, NetGuidCache, PlayerControllerOpenPlan,
    PlayerStateOpenPlan, PlayerStateOpenState, compose_actor_open, compose_player_controller_open,
    compose_player_state_open, movement_update_block, replicated_unique_net_id, resting_movement,
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
/// Lab choice: default channel-index limit for bunch parsing.
pub const DEFAULT_MAX_CHANNELS: u32 = 32;
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
    /// GameState updates that could not be composed or framed.
    pub game_state_update_errors: u64,
    /// Naked GameState updates whose values were still the latest and are
    /// due again.
    pub game_state_updates_lost: u64,
    /// PlayerState opens sent after the PlayerController open (F-784).
    pub player_state_opens_sent: u64,
    /// PlayerState opens that could not be composed or framed.
    pub player_state_open_errors: u64,
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
    /// Movement updates sent through [`PeerLab::update_actor_movement`].
    pub actor_updates_sent: u64,
    /// Movement updates that could not be composed or framed.
    pub actor_update_errors: u64,
    /// Naks of in-flight movement updates, superseded ones included.
    pub actor_update_naks: u64,
    /// Naked movement updates that were still the latest and are due again.
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

/// The GameState handles updates carry: the world time, `TimeOfDay`,
/// `LevelTimeElapsedSec`, `ServerFPS` and the `NetworkingStatus` fields.
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

struct PeerEntry {
    peer: LabPeer,
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
    /// Replicated actors this connection opened, by caller key.
    opened_actors: BTreeMap<u64, u32>,
    /// Closes that could not be framed yet (key -> reason), retried by
    /// [`PeerLab::retry_pending_closes`]; their actors stay open meanwhile.
    pending_closes: BTreeMap<u64, u32>,
    /// The location each open actor was last sent at (its spawn, then
    /// movement updates).
    actor_locations: BTreeMap<u64, [f32; 3]>,
    /// Movement updates awaiting the client's ack or nak: packet ID -> actor.
    /// A record the client never reports stays until the actor closes or the
    /// peer is removed, so this is bounded by the peer's lifetime.
    movement_in_flight: BTreeMap<u32, u64>,
    /// Each actor's latest movement update packet; only its nak repairs.
    last_movement_packet: BTreeMap<u64, u32>,
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

/// The GameState every joined connection is opened with (F-794).
#[derive(Clone, Debug)]
struct GameStateSession {
    plan: GameStateOpenPlan,
    state: GameStateOpenState,
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
    /// Opt-in GameState open after the PlayerController open.
    game_state_open: Option<GameStateSession>,
    /// Opt-in PlayerState open after the PlayerController open.
    player_state_open: Option<PlayerStateOpenPlan>,
    /// Driver-wide NetGUIDs, replicated actors keyed by the caller's key.
    guid_cache: NetGuidCache,
    /// `AGameSession::GetNextPlayerID`'s counter (F-784): 256 first.
    next_player_id: i32,
    /// Lab choice (U-068): only the first PlayerState opened since bind
    /// replicates `bIsHost = true`.
    host_assigned: bool,
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
            game_state_open: None,
            player_state_open: None,
            guid_cache: NetGuidCache::new(),
            next_player_id: FIRST_PLAYER_ID,
            host_assigned: false,
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
    /// the location differs from the one last sent and the channel's open is
    /// acked (UE drops unreliable bunches for a channel the client has not
    /// opened yet). Returns whether an update was sent; failures count
    /// `actor_update_errors` and change nothing. A lost update is repaired as
    /// UE re-sends a naked packet's properties: when the client naks its
    /// packet and no newer update was sent since, the location counts as
    /// unsent (`actor_updates_lost`) and the next selection sends it again.
    pub fn update_actor_movement(
        &mut self,
        connection: u32,
        key: u64,
        location: [f32; 3],
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
        if entry.actor_locations.get(&key) == Some(&location)
            || !entry.peer.actor_channel_settled(channel)
        {
            return false;
        }
        let codec = ActorChannelCodec {
            engine_net_version,
            exports_enabled: false,
            has_level: true,
            order: entry.peer.connection_byte_order(),
            limits: ActorChannelLimits::default(),
        };
        let packet_id = entry.peer.sequencer().out_packet_id();
        let sent = resting_movement(location)
            .and_then(|movement| movement_update_block(engine_net_version, movement))
            .and_then(|block| {
                encode_actor_bunch_body(
                    &ActorBunchBody {
                        spawn: None,
                        blocks: vec![block],
                    },
                    &codec,
                )
            })
            .and_then(|payload| entry.peer.actor_update(channel, payload));
        let datagram = match sent {
            Ok(datagram) => datagram,
            Err(_) => {
                bump(&mut self.stats.actor_update_errors);
                if entry.peer.is_failed() {
                    self.peers.remove(&address);
                    bump(&mut self.stats.peers_failed);
                }
                return false;
            }
        };
        entry.ack_pending = false;
        if !self.send(&datagram, address) {
            // Not recorded: the next selection builds the update again.
            return false;
        }
        if let Some(entry) = self.peers.get_mut(&address) {
            entry.actor_locations.insert(key, location);
            entry.movement_in_flight.insert(packet_id, key);
            entry.last_movement_packet.insert(key, packet_id);
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
                    self.peers.remove(&address);
                    bump(&mut self.stats.peers_failed);
                } else {
                    entry.pending_closes.insert(key, reason);
                }
                return false;
            }
        };
        entry.pending_closes.remove(&key);
        entry.opened_actors.remove(&key);
        entry.actor_locations.remove(&key);
        entry.movement_in_flight.retain(|_, actor| *actor != key);
        entry.last_movement_packet.remove(&key);
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
        let mut cache = self.guid_cache.clone();
        let guid = cache.actor_guid(ActorKey::World(key));
        open.actor = guid;
        let mut exports = entry.exports.clone();
        let sent = entry
            .peer
            .free_actor_channel(first_channel)
            .context("no free actor channel")
            .and_then(|channel| {
                let composed = compose_actor_open(
                    &mut cache,
                    &mut exports,
                    &open,
                    channel,
                    engine_net_version,
                    entry.peer.connection_byte_order(),
                )?;
                let export_payloads: Vec<Payload> = composed.exports.into_iter().collect();
                let sent = entry
                    .peer
                    .reliable_actor_open_datagrams(&composed.bunch, &export_payloads)?;
                Ok((channel, sent))
            });
        let (channel, sent) = match sent {
            Ok(sent) => sent,
            Err(_) => {
                bump(&mut self.stats.actor_open_errors);
                if entry.peer.is_failed() {
                    self.peers.remove(&address);
                    bump(&mut self.stats.peers_failed);
                }
                return None;
            }
        };
        commit_exports(&mut exports, &sent);
        entry.exports = exports;
        entry.ack_pending = false;
        entry.opened_actors.insert(key, channel);
        if let Some(location) = open.location {
            entry.actor_locations.insert(key, location);
        }
        self.guid_cache = cache;
        if self.send_all(&sent, address) {
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
        channel < self.config.packet.packet.limits.max_channels && !taken.contains(&Some(channel))
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
    /// lab-written world time.
    pub fn extend_game_state_values(&mut self, values: BTreeMap<u32, RepValue>) -> Result<()> {
        let Some(session) = &mut self.game_state_open else {
            bail!("no GameState open is enabled");
        };
        ensure!(
            !values.contains_key(&super::game_state_open::handle::WORLD_TIME_SECONDS),
            "the lab writes the GameState's world time"
        );
        session.state.values.extend(values);
        Ok(())
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
        self.peers.clear();
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
        self.peers
            .retain(|_, entry| now.saturating_duration_since(entry.last_seen) <= timeout);
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
                self.peers.insert(
                    address,
                    PeerEntry {
                        peer,
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
                        opened_actors: BTreeMap::new(),
                        pending_closes: BTreeMap::new(),
                        actor_locations: BTreeMap::new(),
                        movement_in_flight: BTreeMap::new(),
                        last_movement_packet: BTreeMap::new(),
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
                            entry.movement_in_flight.remove(packet_id);
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
                        // A lost movement update is sent again, as UE re-sends
                        // the properties of a naked packet: forget the sent
                        // location unless a newer update has gone out since.
                        for packet_id in &summary.naked {
                            let Some(key) = entry.movement_in_flight.remove(packet_id) else {
                                continue;
                            };
                            bump(&mut self.stats.actor_update_naks);
                            if entry.last_movement_packet.get(&key) == Some(packet_id) {
                                entry.last_movement_packet.remove(&key);
                                entry.actor_locations.remove(&key);
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
                    }
                    PacketDisposition::Rejected => bump(&mut self.stats.data_rejected),
                }
                received.into_messages()
            }
            Err(_) => {
                bump(&mut self.stats.data_malformed);
                if entry.peer.is_failed() {
                    self.peers.remove(&address);
                    bump(&mut self.stats.peers_failed);
                }
                return;
            }
        };
        self.answer(address, &messages);
        // Decoded messages are dropped here; nothing of them is retained.
        if close && self.peers.remove(&address).is_some() {
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
        self.peers.remove(&address);
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
                    self.peers.remove(&address);
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
                self.peers.remove(&address);
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
                    self.peers.remove(&address);
                    bump(&mut self.stats.peers_failed);
                }
                return false;
            }
        };
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
        let mut cache = self.guid_cache.clone();
        let mut exports = entry.exports.clone();
        let state = PawnOpenState {
            key: pawn_key(connection),
            controller,
            player_state,
            location: controller_plan.spawn_location,
            content: content.clone(),
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
                if entry.peer.is_failed() {
                    self.peers.remove(&address);
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
        let mut state = session.state.clone();
        let world_seconds = self.started.elapsed().as_secs_f32();
        if let Some(status) = &session.networking {
            // As gathered at the last update pass; defaults stay unsent.
            match status.values() {
                Ok(values) => {
                    for (handle, value) in values {
                        if class_default(handle).as_ref() == Some(&value) {
                            state.values.remove(&handle);
                        } else {
                            state.values.insert(handle, value);
                        }
                    }
                }
                Err(_) => {
                    bump(&mut self.stats.game_state_open_errors);
                    return;
                }
            }
        }
        if let Some(meter) = &session.fps {
            // As for the time of day below: 0 is the default and unsent.
            match meter.fps() {
                0 => state
                    .values
                    .remove(&super::game_state_open::handle::SERVER_FPS),
                fps => state.values.insert(
                    super::game_state_open::handle::SERVER_FPS,
                    RepValue::Int(fps),
                ),
            };
        }
        if let Some(clock) = &session.clock {
            // An open sends only values that differ from the default (0).
            let time_of_day = clock.time_of_day();
            if time_of_day == 0.0 {
                state
                    .values
                    .remove(&super::game_state_open::handle::TIME_OF_DAY);
            } else {
                state.values.insert(
                    super::game_state_open::handle::TIME_OF_DAY,
                    RepValue::Float(time_of_day.to_bits()),
                );
            }
        }
        state.values.insert(
            super::game_state_open::handle::WORLD_TIME_SECONDS,
            RepValue::Float(world_seconds.to_bits()),
        );
        if let Some(restored) = session.level_time {
            state.values.insert(
                super::game_state_open::handle::LEVEL_TIME_ELAPSED_SEC,
                RepValue::Int(super::game_state_open::level_time_elapsed_sec(
                    restored,
                    world_seconds,
                )),
            );
        }
        if let Some(value) = locked_missions(session, world_seconds) {
            // An empty list is the default and unsent.
            match value {
                Some(value) => state
                    .values
                    .insert(super::game_state_open::handle::LOCKED_MISSIONS, value),
                None => state
                    .values
                    .remove(&super::game_state_open::handle::LOCKED_MISSIONS),
            };
        }
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
                    self.peers.remove(&address);
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
                is_host: !self.host_assigned,
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
                    self.peers.remove(&address);
                    bump(&mut self.stats.peers_failed);
                }
                return;
            }
        };
        commit_exports(&mut exports, &sent);
        entry.exports = exports;
        entry.ack_pending = false;
        self.guid_cache = cache;
        self.host_assigned = true;
        if self.send_all(&sent, address) {
            bump(&mut self.stats.player_state_opens_sent);
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
            self.peers.remove(&address);
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
            let mut values = replica.due(world_seconds, &changing);
            if values.is_empty() {
                continue;
            }
            let order = entry.peer.connection_byte_order();
            // An update over one bunch (a long NetworkingStatus) goes out as
            // unreliable partial bunches in consecutive datagrams.
            let mut sent = compose_game_state_update(layout, engine, &values, order)
                .and_then(|payload| entry.peer.actor_update_datagrams(channel, payload));
            // A NetworkingStatus that still cannot be sent (over the 64 KiB
            // bunch bound or the sent-record ring's room) must not hold back
            // the clocks: count the failure and send the other values; the
            // status stays due. Composition and framing failures change no
            // state, so the retry is clean.
            let status = handle::NETWORKING_STATUS;
            let mut counted = false;
            if sent.is_err()
                && !entry.peer.is_failed()
                && values.keys().any(|handle| status.contains(handle))
            {
                // One failed attempt counts once, whether or not the retry
                // goes out.
                bump(&mut self.stats.game_state_update_errors);
                counted = true;
                values.retain(|handle, _| !status.contains(handle));
                if values.is_empty() {
                    continue;
                }
                sent = compose_game_state_update(layout, engine, &values, order)
                    .and_then(|payload| entry.peer.actor_update_datagrams(channel, payload));
            }
            match sent {
                Ok(datagrams) => {
                    entry.ack_pending = false;
                    outgoing.push((*address, datagrams, values));
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
            self.peers.remove(&address);
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

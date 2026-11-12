//! EXPERIMENTAL loopback Bevy lab with a bounded per-peer packet table.
//!
//! Extends the stateless handshake lab (`handshake_lab`, same lab cookie policy,
//! not the original algorithm) with one [`LabPeer`] per IPv4 loopback address:
//! a peer is created (or replaced) when the lab sends it a handshake
//! acknowledgement, its later data datagrams pass through [`LabPeer::receive`]
//! and accepted packets are acknowledged when send capacity permits. When
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
//! A drainable queue refusal retains one chosen reply until packet
//! notifications permit a retry. Until it commits, later control dispatch is
//! suppressed; admission checks and challenge generation are not replayed.
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
//!   counter-derived (F-531); this string is a lab choice, is never reported and
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
//!   ([`compose_player_controller_open_at`] at the connection's placement,
//!   F-778, F-779, F-1140) through
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
//!   per session. A pending PlayerState open retains the first valid Begin.
//!   Absent/failed PlayerState, slot -1 and repeats count and do nothing;
//!   the original failure wire is unknown. The controller's BackendProxy then
//!   requests user and character profiles with independent three-failure limits.
//!   Successful profiles replicate PlayerState handles 27..78; only then does
//!   the profile's ChrSlot trigger saved-UID restore. Other fields/subobjects are skipped.
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
//! - Native bounded-delivery policy: outstanding sent packets without ACK/NAK
//!   progress for [`PeerLabConfig::idle_timeout`] also remove the peer, even
//!   when incoming packets keep it active. The clock starts when outstanding
//!   traffic is first observed and clears with an empty sent ring. This is
//!   not an inference about the original server's timeout or retransmit timing.
//!   Removal is local: a full ring cannot carry a close, so the client may
//!   need to reconnect after its own connection timeout.
//! - Native per-query deadline (N-019): a peer whose friends query (F-1051)
//!   stays unanswered for longer than [`PeerLabConfig::idle_timeout`] is
//!   removed the same way (`peers_friends_query_expired`). The clock starts
//!   when the query is first requested and only an accepted answer stops it (or
//!   restarts it, when a re-query is still to be sent);
//!   ACKs and re-queries never extend it, so no peer holds finalisation
//!   (F-973) for others beyond that timeout. No such deadline was observed
//!   in the original; F-858 records only a silent client's close.
//! - Native policy (F-1116): with connected-player records and controller
//!   possession properties on, a session whose possession update is
//!   abandoned never gets its record (its client could not finalise it) and
//!   is closed and removed in the same poll
//!   (`peers_closed_by_possession_abandoned`). Not observed in the original.
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
//!   one byte longer than the larger enabled game/auth datagram allowance,
//!   detecting kernel truncation. Each family keeps its own size limit.
//! - At most one ack packet per peer per update, sent after the receive loop
//!   when that peer had at least one accepted data packet. Stale or duplicate
//!   packets are not acknowledged.
//!
//! Stats and `Debug` carry counts and message kinds only: never payload bytes,
//! cookies, tokens, challenge strings, URLs, unique net ids, versions, sequence
//! values or numeric message fields.

use std::{
    collections::{BTreeMap, BTreeSet, HashMap, VecDeque},
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
use sha2::{Digest, Sha256};

use crate::{
    spawn_placement::{Placement, PlacementSource, SpawnConfig, SpawnResolver},
    steam_auth::{AuthOutcome, VerifiedTicket, decode_ticket_hex},
    steam_presence::ICARUS_APP_ID,
    steam_service::AuthClient,
};

use super::actor_channel::{
    ActorBunchBody, ActorChannelCodec, ActorChannelLimits, BlockTarget, ContentBlock,
    encode_actor_bunch_body, read_client_actor_bunch_body,
};
use super::actor_open::{Block, Reference, StaticActorOpen, compose_static_actor_open};
use super::admission::{
    ActorChannelFailureAction, AdmissionPolicy, ClientLoginState, ConnectionLoginAdmission,
    DEFAULT_MAX_CLIENT_RATE, FIND_PLAYER_START_ERROR, JoinSpawnContext, Occupancy,
    clamp_client_netspeed, evaluate_actor_channel_failure, evaluate_pc_swap_target, int_option,
};
use super::control_messages::{
    ArchiveText, ControlFallback, ControlMessage, Decoded, NetIdValue, UniqueNetId,
};
use super::game_state_open::{
    ConnectedPlayerState, ConnectedPlayersPlan, GameStateOpenPlan, GameStateOpenState,
    LockedMission, NetworkingStatus, active_prospect_update_values, class_default,
    compose_connected_players_update, compose_game_state_open, compose_game_state_update,
};
use super::handshake::decode;
use super::handshake_lab::{
    CookieAuthority, HANDSHAKE_BYTES, HandshakeReply, PACKETS_PER_UPDATE, bind_loopback,
    lab_seconds,
};
use super::inventory_restore::{InventoryRestore, InventoryRestoreTables, restore_inventories};
use super::inventory_weight::DEFAULT_CDO_CURRENT_WEIGHT;
use super::manager_open::{
    ManagerOpenPlan, ManagerSlot, compose_manager_open, compose_player_history_update,
};
use super::modifier_restore::{ModifierRestoreTables, RestoredModifier, restore_modifiers};
use super::modifier_timing::{
    RemainingTime, RemainingTimeChange, on_rep_remaining_time, set_remaining_time,
};
use super::packet::BunchLimits;
use super::packet::Payload;
use super::packet_lab::{
    ActorOpenSent, DeliveredSummary, LabPeer, PacketDisposition, PacketLabConfig,
};
use super::pawn_open::{
    InventorySlot, PawnContent, PawnInventory, PawnOpenPlan, PawnOpenState, PawnTemperatureState,
    compose_pawn_frozen_movement_update, compose_pawn_icarus_uid_update, compose_pawn_move_ack,
    compose_pawn_open,
};
use super::pawn_restore::{
    CurrentPawnState, compose_live_pawn_update, compose_restored_pawn_update, restore_saved_pawn,
};
use super::ping::PingEstimator;
use super::profile_exchange::{
    self, InitialisationAnswer, ProfileExchange, ProfileKind, ProfileResponse,
};
use super::quest_open::{QuestOpenBasePlan, QuestOpenPlan, compose_quest_open};
use super::release::{ReleaseLevelActorsProfile, ReleaseProfile};
use super::rep_layout::{NetworkName, RepLayoutCodec, RepValue};
use super::rep_rpc::{
    BACKEND_PROXY_FIELD_INDEX_BOUND, BESTIARY_MANAGER_FIELD_INDEX_BOUND,
    BP_ICARUS_PLAYER_CONTROLLER_SURVIVAL_FIELD_INDEX_BOUND, FriendsListRejection,
    PLAYER_DATA_FIELD_INDEX_BOUND, RESPONSE_GET_CHARACTER_PROFILE, RESPONSE_GET_USER_PROFILE,
    SERVER_BEGIN_PLAYER_INITIALISATION_FIELD_INDEX, SERVER_FRIENDS_LIST_UPDATED_FIELD_INDEX,
    SERVER_RECEIVE_BESTIARY_DATA, SERVER_RECEIVE_LOADOUT, SERVER_SHORT_TIMEOUT_FIELD_INDEX,
    SERVER_UPDATE_LEVEL_VISIBILITY_FIELD_INDEX, friends_list_rejection,
    parse_begin_player_initialisation, parse_rpc_payload, read_field_header,
    server_friends_list_updated_parameters,
};
use super::rep_rpc::{
    FieldHeader, RpcArgument, RpcItem, encode_rpc_payload, player_controller_rpc_parameters,
    write_field_header,
};
use super::session_open::{actor_role_block, compose_controller_possession_update};
use super::visibility::{
    DecodedVisibilityParameter, VisibilityArchiveProfile, VisibilityDecodeProfile,
    VisibilityRpc82Layout, parse_visibility_rpc82,
};

/// Explicit caller-authored startup actions. Their order and necessity are not
/// qualified against a stock client. No actions are enabled by default.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize, clap::ValueEnum,
)]
#[serde(rename_all = "kebab-case")]
pub enum ControllerStartupAction {
    FlushLevelStreaming,
    RestartPawn,
}

/// Validate an explicit plan without binding sockets or allocating live GUIDs.
pub fn validate_controller_startup(
    profile: &ReleaseProfile,
    actions: &[ControllerStartupAction],
    controller: &PlayerControllerOpenPlan,
    has_pawn: bool,
) -> Result<()> {
    ensure!(
        (1..=32).contains(&actions.len()),
        "controller startup needs 1..=32 actions"
    );
    profile.validate()?;
    ensure!(
        profile.engine_network_version == controller.engine_net_version,
        "startup release differs from controller plan"
    );
    ensure!(
        !actions.contains(&ControllerStartupAction::RestartPawn) || has_pawn,
        "restart requires a pawn plan"
    );
    for order in [
        super::archive::ByteOrder::Little,
        super::archive::ByteOrder::Big,
    ] {
        let codec = RepLayoutCodec {
            actor: actor_codec(controller.engine_net_version, order),
            checksums: false,
            limits: Default::default(),
        };
        for &action in actions {
            controller_startup_payload(
                action,
                profile.class_net_cache.bp_icarus_player_controller_survival,
                Some(2),
                &codec,
            )?;
        }
    }
    Ok(())
}

fn controller_startup_payload(
    action: ControllerStartupAction,
    field_index_bound: u32,
    pawn: Option<u32>,
    codec: &RepLayoutCodec,
) -> Result<Payload> {
    let (index, arguments) = match action {
        ControllerStartupAction::FlushLevelStreaming => (23, vec![]),
        ControllerStartupAction::RestartPawn => (
            40,
            vec![RpcArgument::Sent(vec![RpcItem::Value(RepValue::Object(
                super::actor_channel::NetGuidRef::plain(pawn.context("startup pawn unavailable")?),
            ))])],
        ),
    };
    let parameters =
        player_controller_rpc_parameters(index).context("startup RPC layout missing")?;
    let payload = encode_rpc_payload(&parameters, &arguments, codec)?;
    let mut writer = super::packet::BitWriter::new();
    write_field_header(
        &mut writer,
        &FieldHeader { index, payload },
        field_index_bound,
    )?;
    encode_actor_bunch_body(
        &ActorBunchBody {
            spawn: None,
            blocks: vec![ContentBlock {
                has_rep_layout: false,
                target: BlockTarget::Actor,
                payload: Payload::from_writer(writer),
            }],
        },
        &codec.actor,
    )
}
use super::server_fps::ServerFpsMeter;
use super::session_open::{
    ActorKey, ActorOpenSpec, ConnectionExports, NetGuidCache, PathSegment,
    PlayerControllerOpenPlan, PlayerStateOpenPlan, PlayerStateOpenState, actor_codec,
    actor_properties_update_block, authored_movement, compose_actor_open,
    compose_player_controller_open_at, compose_player_state_open, initial_actor_properties,
    replicated_unique_net_id,
};
use super::stat_restore::{
    PawnStatInputs, RestoredStats, StatRestoreTables, restore_stats, stat_bounds, stat_delta,
};
use super::time_of_day::TimeOfDayClock;

/// Explicit per-modifier inputs for [`PeerLab::tick_restored_modifier`].
/// No constructor, row or scheduling defaults are inferred. Remaining time
/// comes from the modifier's latest retained raw and wire values.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RestoredModifierTickContext {
    /// The setter's direct owner-role lookup, distinct from the callback's
    /// GetOwnerRole result. Neither is inferred from the other's value.
    pub owner_role_byte: Option<u8>,
    /// Whether the modifier's interval path is enabled.
    pub interval_enabled: bool,
    /// Accumulated interval time in seconds.
    pub accumulated_seconds: f32,
    /// The threshold and period subtracted by an admitted interval path.
    pub interval_seconds: f32,
}

/// Explicit per-pawn timing inputs for [`PeerLab::tick_pawn_temperature`].
/// No defaults or original constructor/clock semantics are implied.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PawnTemperatureTickContext {
    /// Whether the reference tested at receiver +0x218 is non-null.
    pub required_reference_present: bool,
    /// Whether the temperature singleton is cached; false requests acquisition.
    pub singleton_cached: bool,
    /// Sampling threshold in seconds; a tick samples only after exceeding it.
    pub sample_interval_seconds: f32,
    /// Accumulated sampling time in seconds; one interval is removed per sample.
    pub sample_elapsed_seconds: f32,
    /// Retained fractional internal-temperature adjustment in temperature units.
    pub internal_remainder: f32,
}

impl PawnTemperatureTickContext {
    fn state(
        self,
        temperatures: Option<PawnTemperatureState>,
    ) -> Result<super::temperature::TemperatureTickState> {
        let t = temperatures.context("pawn open has not been queued")?;
        Ok(super::temperature::TemperatureTickState {
            required_reference_present: self.required_reference_present,
            singleton_cached: self.singleton_cached,
            sample_interval_seconds: self.sample_interval_seconds,
            sample_elapsed_seconds: self.sample_elapsed_seconds,
            internal_remainder: self.internal_remainder,
            external: t.external.context("unknown external temperature")?,
            modified_external: t
                .modified_external
                .context("unknown modified external temperature")?,
            modified_internal: t
                .modified_internal
                .context("unknown modified internal temperature")?,
            internal: t.internal.context("unknown internal temperature")?,
        })
    }
}

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
/// Native bound on one connection's visible-level set and on the levels it
/// keeps actor records for (a memory bound, not a channel bound: see
/// [`MAX_LEVEL_ACTOR_CHANNELS`]). The original server's join-time level
/// status call lists 134 streaming levels for the observed map (F-1078), so
/// 256 names every one of them. With [`MAX_LEVEL_NAME_UNITS`] a connection
/// retains at most three copies of each name (the set, the per-level map and
/// the admission queue: 3 x 256 x 1,023 bytes, about 0.75 MiB), plus the
/// driver-wide holder entry of each distinct level held (two copies).
pub const MAX_VISIBLE_LEVELS: usize = 256;
/// Per-connection cap on the actor channels level actors may hold: 64
/// levels of two actors. Hides close nothing in this version, so the budget
/// is consumed by every level ever shown in a session, never returned; a
/// session that shows more levels than fit gets their opens refused
/// (counted). Readiness exports the budget remaining.
pub const MAX_LEVEL_ACTOR_CHANNELS: usize = 128;
/// Free actor channels (outside the configured session channels) that level
/// actors always leave to the server's own later opens, such as native
/// replication's relevancy set and any future session open. A native
/// policy: the lab cannot know how many actors replication will open, so a
/// fixed quarter of the default 256-channel table is kept back; with that
/// table level actors stop at [`MAX_LEVEL_ACTOR_CHANNELS`] first.
pub const LEVEL_ACTOR_CHANNEL_RESERVE: usize = 64;
/// Level actor opens admitted per connection per poll pass, after every
/// other retry pass of that poll (lowest priority): one level's pair.
pub const LEVEL_ACTOR_OPENS_PER_PASS: usize = 2;
/// Native admission cap on a reported level package name, in UTF-16 units:
/// the engine's `NAME_SIZE` (1024) less its terminator. The decoder's own
/// cap (65,536 units, F-1021) is far above any package name.
pub const MAX_LEVEL_NAME_UNITS: usize = 1023;

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
    /// Nonzero interval for inbound-idle, stalled-delivery and unanswered
    /// friends-query (N-019) local removal. Delivery progress uses ACK/NAK
    /// notifications, not incoming traffic alone.
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
    /// Outstanding delivery made no ACK/NAK progress within the native timeout.
    pub peers_delivery_stalled: u64,
    /// Friends query (F-1051) unanswered within the native timeout (N-019).
    pub peers_friends_query_expired: u64,
    /// With records and possession properties on, sessions closed because
    /// their possession update was abandoned: their client could never
    /// finalise the record, so it is never sent (F-1116).
    pub peers_closed_by_possession_abandoned: u64,
    /// Peers removed after a failed receive or ack.
    pub peers_failed: u64,
    /// Peers removed by ticket verification or a negative Steam auth outcome.
    pub peers_auth_closed: u64,
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
    /// PlayerController open attempts refused by composition, framing or temporary send pressure.
    pub player_controller_open_errors: u64,
    /// Actor bunches on unopened/closing channels or outside this peer's controller.
    pub actor_bunches_ignored: u64,
    /// Unsupported forms, reliable queue overflow, or refused partial parts.
    pub actor_bunches_dropped: u64,
    /// Reliable partial parts released in order, excluding duplicate arrivals.
    pub actor_partial_parts: u64,
    /// Complete bodies constructed, before domain validation or application.
    pub actor_partial_completed: u64,
    /// Partial refusals and interruptions of pending assemblies.
    pub actor_partial_rejected: u64,
    /// Malformed controller content blocks, field headers or RPC parameters.
    pub actor_bunches_malformed: u64,
    pub actor_duplicate_reliable: u64,
    /// Unrecognized fields skipped by their declared payload length.
    pub controller_fields_ignored: u64,
    pub initialisation_slots_recorded: u64,
    /// Slot -1, absent/failed PlayerState or a repeated call; pending opens retain Begin.
    pub initialisation_calls_ignored: u64,
    /// Parsed profile calls dispatched after complete controller-bunch validation.
    /// Includes unsuccessful and wrong-phase responses; excludes parse rejection.
    pub profile_user_responses_recognized: u64,
    pub profile_character_responses_recognized: u64,
    /// Publication attempts retained behind drainable transport pressure.
    pub profile_user_publications_deferred: u64,
    pub profile_character_publications_deferred: u64,
    pub profile_user_requests_sent: u64,
    pub profile_character_requests_sent: u64,
    /// Successful response publications admitted to reliable PlayerState transport.
    pub profile_user_responses_received: u64,
    pub profile_character_responses_received: u64,
    /// Unsuccessful outstanding responses (separate retry budgets, limit 3).
    pub profile_user_failures: u64,
    pub profile_character_failures: u64,
    /// Unsolicited, wrong-phase or repeated profile responses.
    pub profile_responses_ignored: u64,
    /// Sessions removed on retry exhaustion, vanished PlayerState or exchange
    /// error, including a friends-list query (F-1051) that cannot reach transport.
    pub profile_players_failed: u64,
    /// Composition/framing refusals; drainable capacity waits for ACK/NAK progress.
    pub profile_exchange_errors: u64,
    /// Separate reliable ActiveUserProfile and ActiveCharacter updates sent.
    pub profile_updates_sent: u64,
    pub loadout_requests_sent: u64,
    pub bestiary_requests_sent: u64,
    pub associated_prospects_sent: u64,
    /// Every recognized answer, including ignored answers; payloads stay opaque.
    pub loadout_responses_received: u64,
    pub bestiary_responses_received: u64,
    /// Answers before their requests, duplicates and answers after completion.
    pub initialisation_answers_ignored: u64,
    /// Both answers accepted with an open pawn and PlayerState.
    pub players_initialised: u64,
    /// Controllers' `ServerFriendsListUpdated` calls accepted (F-973).
    pub friends_lists_received: u64,
    /// Friends-list requests admitted to reliable controller transport (F-1051).
    pub friends_list_queries_queued: u64,
    /// Admitted friends-list requests whose own reliable bunch a positive
    /// packet ACK settled; a later query supersedes an unsettled receipt.
    /// No client execution or response is implied.
    pub friends_list_queries_acknowledged: u64,
    /// Parsed `ServerFriendsListUpdated` calls ignored because the peer has
    /// no registered player (also counted in `initialisation_calls_ignored`).
    pub friends_lists_ignored_unregistered: u64,
    /// Parsed calls ignored because friend IDs were already ready (also
    /// counted in `initialisation_calls_ignored`).
    pub friends_lists_ignored_already_ready: u64,
    /// `ServerFriendsListUpdated` fields refused, by reason; each refusal
    /// also counts its bunch in `actor_bunches_malformed`.
    pub friends_list_rejections: FriendsListRejectionCounts,
    /// Completed players finalised once every joined controller sent its
    /// friends list (F-973).
    pub players_finalised: u64,
    /// `UReplicatedConnectedPlayer` records registered at Begin acceptance
    /// (`AddConnectedPlayer`), one per session, with
    /// [`PeerLab::enable_connected_player_records`].
    pub connected_player_records_created: u64,
    /// Record creations (all seven handles) admitted to reliable GameState
    /// transport, one per record per connection; Mode A sends a record once,
    /// after its player is finalised.
    pub connected_player_record_creations_sent: u64,
    /// Admitted record creations whose reliable bunches a positive packet ACK
    /// settled, counted once each. No client execution is implied.
    pub connected_player_record_creations_acked: u64,
    /// Record property updates (staged handle sets) admitted to transport.
    /// Mode A sends none; reserved for the staged mode.
    pub connected_player_record_updates_sent: u64,
    /// `ConnectedPlayers` array (handle 3) updates admitted to transport,
    /// including those created with a record and removal updates.
    pub connected_player_array_updates_sent: u64,
    /// Record or array sends held back by drainable transport pressure; the
    /// retry sends the latest state after ACK/NAK progress.
    pub connected_player_record_sends_deferred: u64,
    /// Records removed with their session (`RemoveConnectedPlayer`).
    pub connected_player_records_removed: u64,
    /// Composition or framing refusals that cannot drain; nothing is retried
    /// until the next record change.
    pub connected_player_record_errors: u64,
    /// Controller possession property updates (handle 17 `Pawn` and handle
    /// 20 `IcarusPlayerCharacter`, the controller pawn F-1074's check reads)
    /// admitted to reliable controller transport: at most one per session,
    /// after its pawn open, with
    /// [`PeerLab::enable_controller_possession_properties`].
    pub controller_possession_updates_sent: u64,
    /// Admitted possession updates whose own reliable bunch a positive packet
    /// ACK settled, counted once each. No client application is implied.
    pub controller_possession_updates_acked: u64,
    /// Possession updates held back by drainable transport pressure; the
    /// retry composes from the current state after ACK/NAK progress.
    pub controller_possession_sends_deferred: u64,
    /// Composition or framing refusals that cannot drain, or a controller or
    /// pawn channel no longer open when due; that session's update is
    /// abandoned. With records on, its record is never sent and the session
    /// is closed (`peers_closed_by_possession_abandoned`).
    pub controller_possession_errors: u64,
    /// Frozen-movement releases scheduled: one per session, when a positive
    /// ACK settles the creation of its own connected-player record, with
    /// [`PeerLab::enable_frozen_movement_release`].
    pub frozen_movement_releases_scheduled: u64,
    /// Unreliable pawn property updates setting `bServerFrozenMovement` false
    /// first sent, at most one per session.
    pub frozen_movement_releases_sent: u64,
    /// Releases whose latest copy a positive packet ACK covered, counted once
    /// each. No client application is implied.
    pub frozen_movement_releases_acked: u64,
    /// Copies of a release sent again after a NAK lost the previous one.
    pub frozen_movement_release_resends: u64,
    /// Releases held back by drainable transport pressure.
    pub frozen_movement_release_sends_deferred: u64,
    /// Composition or framing refusals that cannot drain, or a pawn channel
    /// no longer open when due; that session's release is abandoned.
    pub frozen_movement_release_errors: u64,
    /// Client bunches on a connection's open pawn channel, reliable and
    /// unreliable, counted by header only on every connection.
    pub pawn_bunches_received_reliable: u64,
    pub pawn_bunches_received_unreliable: u64,
    /// Client packed moves (pawn field 39) decoded with
    /// [`PeerLab::enable_pawn_move_acks`] (F-1127).
    pub pawn_moves_received: u64,
    /// Unreliable pawn bunches the lab could not read for moves (partial,
    /// with exports or must-be-mapped GUIDs, an unreadable body or field
    /// header) and field-39 payloads or timestamps refused: counted and
    /// skipped, the bunch otherwise treated as before, never a close.
    pub pawn_moves_malformed: u64,
    /// Good-move acknowledgements (pawn field 30) sent unreliable, at most
    /// one per connection per [`MOVE_ACK_INTERVAL`]; never resent.
    pub pawn_move_acks_sent: u64,
    /// Acknowledgements held back by drainable transport pressure.
    pub pawn_move_ack_sends_deferred: u64,
    /// Composition or framing refusals that cannot drain; that newest move
    /// goes unacknowledged.
    pub pawn_move_ack_errors: u64,
    /// Client-reported locations stored, refused by the authored checks
    /// (F-1128; the stored one kept), or skipped because the move has a
    /// movement base or no movement mode.
    pub pawn_move_positions_accepted: u64,
    pub pawn_move_positions_rejected: u64,
    pub pawn_move_positions_skipped: u64,
    /// Joining connections placed, by source (F-1139): a restored saved
    /// location, a configured spawn point ([`PeerLab::enable_spawn_points`])
    /// or the plan's location; one per connection, at its controller open.
    pub pawn_spawns_saved: u64,
    pub pawn_spawns_configured: u64,
    pub pawn_spawns_default: u64,
    /// Saved locations offered but refused (not finite or out of bound once
    /// raised); the next source was used.
    pub pawn_spawn_saved_refused: u64,
    /// Level visibility reports (controller field 82, F-1052) dispatched with
    /// [`PeerLab::enable_level_visibility_actors`], by disposition; counts
    /// only, never names. A newly visible level added to the connection's set.
    pub level_visibility_reports_added: u64,
    /// A visible report of a level already in the set: no effect.
    pub level_visibility_reports_duplicate: u64,
    /// A hidden report; the name leaves the set if it was there.
    pub level_visibility_reports_hidden: u64,
    /// A visible report while the set holds [`MAX_VISIBLE_LEVELS`] names:
    /// ignored.
    pub level_visibility_reports_over_bound: u64,
    /// Field 82 payloads the decoder refused: that field alone is skipped by
    /// its declared length (diagnosed Malformed) and the rest of its bunch
    /// is applied as when field 82 was ignored.
    pub level_visibility_reports_malformed: u64,
    /// A visible report naming no admissible streamed level (an unresolved
    /// hardcoded name, a nonzero name number, a name that is not a plain
    /// `/Game/` package path, the persistent map or a release package in any
    /// letter case, or one naming an identity the server already holds):
    /// counted and ignored, never a close.
    pub level_visibility_reports_refused_name: u64,
    /// A report whose parameter was absent (presence bit clear).
    pub level_visibility_reports_absent: u64,
    /// Level actor opens (one per actor of a newly visible level) admitted to
    /// reliable transport on fresh actor channels.
    pub level_actor_opens_sent: u64,
    /// Admitted level actor opens a positive packet ACK settled, once each.
    /// No client spawn is implied.
    pub level_actor_opens_acked: u64,
    /// Level actor admission passes held back by transport pressure (a
    /// drainable refusal, or a sent ring less than half free); retried in
    /// report order after ACK/NAK progress.
    pub level_actor_opens_deferred: u64,
    /// Level actor refusals. One increment is one actor slot of one
    /// connection entering the refused state for a cause it was not refused
    /// for at its previous attempt: the per-connection cap
    /// ([`MAX_LEVEL_ACTOR_CHANNELS`]), which only the session's end lifts
    /// since hides close nothing, or the channels (no free channel, or the
    /// [`LEVEL_ACTOR_CHANNEL_RESERVE`] would be touched), which a released
    /// actor channel can lift. A retry refused again for the same cause adds
    /// nothing; a slot refused for channels is retried after a release only
    /// while the connection is below its cap and the budget can admit it,
    /// and on a later report of its level. A level hidden while none of its
    /// actors was admitted is forgotten, so a later report of it starts
    /// afresh. Also one per actor name for each visible report of a level
    /// that gets no record because the connection's level record bound is
    /// full.
    pub level_actor_opens_refused: u64,
    /// Channel-refused slots re-armed for one retry after an actor channel
    /// was released: at most as many as the budget can then admit, never a
    /// cap refusal.
    pub level_actor_opens_requeued: u64,
    /// Composition or framing failures that cannot drain, including an
    /// exhausted static GUID space; retried only on a later report.
    pub level_actor_open_errors: u64,
    /// Opens still awaiting admission when their level was reported hidden:
    /// dropped unsent.
    pub level_actor_opens_withdrawn: u64,
    /// Hidden reports of a level whose actors this connection opened: the
    /// actors stay open (the original's hide-side wire is unobserved).
    pub level_hides_actors_kept: u64,
    pub player_character_id_updates_sent: u64,
    /// Returning pawns given their saved UID (F-950).
    pub returning_pawns_restored: u64,
    /// Completed players with no free matching saved character.
    pub returning_pawns_unmatched: u64,
    /// Matches whose saved UID was unusable or whose update failed.
    pub returning_pawn_errors: u64,
    /// Reliable saved pawn content updates sent after UID restoration (F-960).
    pub returning_pawn_updates_sent: u64,
    /// Dead-on-save records; their update is withheld.
    pub returning_pawn_update_gaps: u64,
    /// Restored stomach entries left out of a sent update: their modifier IDs
    /// need the unimplemented modifier restore, so the pawn keeps the empty
    /// stomach its open sent (F-963).
    pub returning_pawn_stomach_entries_dropped: u64,
    /// Restore, composition or framing failures; the UID restore is retained.
    pub returning_pawn_update_errors: u64,
    /// Restored inventories sent in returning-pawn updates (F-978).
    pub returning_inventories_sent: u64,
    /// Saved inventories left out of a sent update because their restore
    /// meets a gap (an unread default, an unplaced item).
    pub returning_inventory_gaps: u64,
    /// Restored modifiers sent in returning-pawn updates (F-980).
    pub returning_modifiers_sent: u64,
    /// Explicit modifier time batches accepted by the reliable transport.
    pub modifier_time_updates_queued: u64,
    /// Modifier time composition or transport framing failures.
    pub modifier_time_update_errors: u64,
    /// Temperature batches accepted by the pawn's reliable transport.
    pub pawn_temperature_updates_queued: u64,
    /// Temperature composition or transport framing failures.
    pub pawn_temperature_update_errors: u64,
    /// Saved modifiers left out because restore coverage is unavailable.
    pub returning_modifier_gaps: u64,
    /// Returning-pawn updates that carried restored stats (F-988).
    pub returning_stats_sent: u64,
    /// Updates whose stats or dependent levels lack complete restore inputs.
    pub returning_stat_gaps: u64,
    /// New-player AddPlayer outcomes (F-865); returning C025 players skip it.
    pub player_history_appends: u64,
    pub player_history_duplicates: u64,
    /// Validation, composition or reliable framing failures, without identities.
    pub player_history_errors: u64,
    /// Pawn opens sent after the PlayerController open (F-797).
    pub pawn_opens_sent: u64,
    /// Pawn open attempts refused by composition, framing or temporary send pressure.
    pub pawn_open_errors: u64,
    /// GameState opens sent after the PlayerController open (F-794).
    pub game_state_opens_sent: u64,
    /// GameState open attempts refused by composition, framing or temporary send pressure.
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
    /// PlayerState open attempts refused by composition, framing or temporary send pressure.
    pub player_state_open_errors: u64,
    /// World manager opens sent after the PlayerState open (F-859).
    pub manager_opens_sent: u64,
    /// World manager open attempts refused by composition, framing or temporary send pressure.
    pub manager_open_errors: u64,
    /// Saved quest actor opens sent with the world managers (`F-934`).
    pub quest_opens_sent: u64,
    /// Saved quest actor open attempts refused by composition, framing or temporary send pressure.
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
    /// Native delivery timeout, independent of incoming-packet activity.
    pub closed_by_delivery_stall: u64,
    /// Native per-query deadline: friends query unanswered despite traffic.
    pub closed_by_friends_query_timeout: u64,
    /// Native policy: possession update abandoned, so no record (F-1116).
    pub closed_by_possession_abandoned: u64,
    /// Sessions closed by a client control close bunch.
    pub closed_by_client: u64,
    /// Sessions closed after an Upgrade or Failure reply.
    pub closed_by_reply: u64,
    /// Upgrade replies to a Hello whose network version differed from the
    /// configured one (each also counts in `closed_by_reply`).
    pub upgrades_sent: u64,
    /// Sessions removed after a receive, reply or ack failure.
    pub closed_failed: u64,
    /// Sessions removed by local ticket rejection or a negative Steam outcome.
    pub closed_by_authentication: u64,
    /// Sessions removed by shutdown.
    pub closed_by_shutdown: u64,
    /// PlayerController opens sent after Join.
    pub player_controller_opens_sent: u64,
    pub initialisation_slots_recorded: u64,
    pub initialisation_calls_ignored: u64,
    /// Parsed profile calls dispatched after complete controller-bunch validation.
    /// Includes unsuccessful and wrong-phase responses; excludes parse rejection.
    pub profile_user_responses_recognized: u64,
    pub profile_character_responses_recognized: u64,
    /// Publication attempts retained behind drainable transport pressure.
    pub profile_user_publications_deferred: u64,
    pub profile_character_publications_deferred: u64,
    pub profile_user_requests_sent: u64,
    pub profile_character_requests_sent: u64,
    pub profile_user_responses_received: u64,
    pub profile_character_responses_received: u64,
    pub profile_user_failures: u64,
    pub profile_character_failures: u64,
    pub profile_responses_ignored: u64,
    pub profile_players_failed: u64,
    pub profile_exchange_errors: u64,
    pub profile_updates_sent: u64,
    pub loadout_requests_sent: u64,
    pub bestiary_requests_sent: u64,
    pub associated_prospects_sent: u64,
    pub loadout_responses_received: u64,
    pub bestiary_responses_received: u64,
    pub initialisation_answers_ignored: u64,
    pub players_initialised: u64,
    pub friends_lists_received: u64,
    /// Friends-list requests admitted to reliable controller transport (F-1051).
    pub friends_list_queries_queued: u64,
    pub friends_list_queries_acknowledged: u64,
    pub friends_lists_ignored_unregistered: u64,
    pub friends_lists_ignored_already_ready: u64,
    pub friends_list_rejections: FriendsListRejectionCounts,
    pub players_finalised: u64,
    /// `UReplicatedConnectedPlayer` record counters, as in [`PeerLabStats`].
    pub connected_player_records_created: u64,
    pub connected_player_record_creations_sent: u64,
    pub connected_player_record_creations_acked: u64,
    pub connected_player_record_updates_sent: u64,
    pub connected_player_array_updates_sent: u64,
    pub connected_player_record_sends_deferred: u64,
    pub connected_player_records_removed: u64,
    pub connected_player_record_errors: u64,
    /// Controller possession property counters, as in [`PeerLabStats`].
    pub controller_possession_updates_sent: u64,
    pub controller_possession_updates_acked: u64,
    pub controller_possession_sends_deferred: u64,
    pub controller_possession_errors: u64,
    /// Frozen-movement release and pawn inbound counters, as in [`PeerLabStats`].
    pub frozen_movement_releases_scheduled: u64,
    pub frozen_movement_releases_sent: u64,
    pub frozen_movement_releases_acked: u64,
    pub frozen_movement_release_resends: u64,
    pub frozen_movement_release_sends_deferred: u64,
    pub frozen_movement_release_errors: u64,
    pub pawn_bunches_received_reliable: u64,
    pub pawn_bunches_received_unreliable: u64,
    /// Pawn move and acknowledgement counters, as in [`PeerLabStats`].
    pub pawn_moves_received: u64,
    pub pawn_moves_malformed: u64,
    pub pawn_move_acks_sent: u64,
    pub pawn_move_ack_sends_deferred: u64,
    pub pawn_move_ack_errors: u64,
    pub pawn_move_positions_accepted: u64,
    pub pawn_move_positions_rejected: u64,
    pub pawn_move_positions_skipped: u64,
    /// Placements by source, as in [`PeerLabStats`].
    pub pawn_spawns_saved: u64,
    pub pawn_spawns_configured: u64,
    pub pawn_spawns_default: u64,
    pub pawn_spawn_saved_refused: u64,
    /// Level visibility report and level actor counters, as in [`PeerLabStats`].
    pub level_visibility_reports_added: u64,
    pub level_visibility_reports_duplicate: u64,
    pub level_visibility_reports_hidden: u64,
    pub level_visibility_reports_over_bound: u64,
    pub level_visibility_reports_malformed: u64,
    pub level_visibility_reports_refused_name: u64,
    pub level_visibility_reports_absent: u64,
    pub level_actor_opens_sent: u64,
    pub level_actor_opens_acked: u64,
    pub level_actor_opens_deferred: u64,
    pub level_actor_opens_refused: u64,
    pub level_actor_opens_requeued: u64,
    pub level_actor_open_errors: u64,
    pub level_actor_opens_withdrawn: u64,
    pub level_hides_actors_kept: u64,
    pub player_character_id_updates_sent: u64,
    pub returning_pawns_restored: u64,
    pub returning_pawn_updates_sent: u64,
    pub returning_pawn_update_gaps: u64,
    pub returning_pawn_stomach_entries_dropped: u64,
    pub returning_pawn_update_errors: u64,
    pub returning_inventories_sent: u64,
    pub returning_inventory_gaps: u64,
    pub returning_modifiers_sent: u64,
    pub returning_modifier_gaps: u64,
    pub modifier_time_updates_queued: u64,
    pub modifier_time_update_errors: u64,
    pub pawn_temperature_updates_queued: u64,
    pub pawn_temperature_update_errors: u64,
    pub returning_stats_sent: u64,
    pub returning_stat_gaps: u64,
    pub player_history_appends: u64,
    pub player_history_duplicates: u64,
    pub player_history_errors: u64,
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

/// Bounded aggregate gauges for native join diagnostics. No payloads,
/// identities, GUIDs, addresses or profile values are exposed. Player phases
/// partition the primary joined players; queue/channel gauges are independent.
/// Transport settlement is not client actor-spawn or geometry readiness proof.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct ReadinessCounts {
    pub initialisation_transport: InitialisationTransportCounts,
    pub friends_query: FriendsQueryReadiness,
    pub connected_player_records: ConnectedPlayerRecordReadiness,
    pub controller_possession: ControllerPossessionReadiness,
    pub frozen_movement_release: FrozenMovementReleaseReadiness,
    pub pawn_move_acks: PawnMoveAckReadiness,
    pub level_visibility: LevelVisibilityReadiness,
    /// Queued means accepted by reliable transport, not ACKed or spawned.
    pub controller_startup_actions_queued: usize,
    /// Retained receipts after a positive ACK leaves the controller channel
    /// open and reliably settled. No client execution is implied.
    pub controller_startup_actions_settled: usize,
    pub controller_startup_actions_awaiting_ack: usize,
    pub controller_startup_sequences_settled: usize,
    pub controller_startup_actions_pending: usize,
    pub controller_startup_sequences_failed: usize,
    pub initial_open_outcomes: InitialOpenReadiness,
    pub pending_control_replies: usize,
    pub pending_initial_opens: usize,
    pub pending_initial_stages: usize,
    pub pending_profile_exchanges: usize,
    pub sent_packets_pending: usize,
    pub control_reliable_pending: usize,
    pub actor_reliable_pending: usize,
    pub resend_bunches_pending: usize,
    pub actor_channels_open: usize,
    /// Open, non-closing channels with no unacknowledged reliable bunches.
    pub actor_channels_reliable_settled: usize,
    pub players_awaiting_begin: usize,
    pub players_begin_waiting_for_player_state: usize,
    pub players_awaiting_user_profile: usize,
    pub players_awaiting_character_profile: usize,
    /// Includes an unsent initialisation request and missing loadout/bestiary answers.
    pub players_awaiting_initialisation: usize,
    pub initialisation_blockers: InitialisationBlockers,
    /// Both answers accepted, but completion has not yet committed.
    pub players_awaiting_completion: usize,
    pub players_awaiting_finalisation: usize,
    pub players_finalised: usize,
}

/// Retained transport receipts for the request batch on the controller and
/// character-ID publication on PlayerState. Queue acceptance and positive ACK
/// settlement are independent of response/application and finalisation phases.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct InitialisationTransportCounts {
    pub requests_queued: usize,
    pub requests_settled: usize,
    pub requests_awaiting_ack: usize,
    pub completions_queued: usize,
    pub completions_settled: usize,
    pub completions_awaiting_ack: usize,
}

/// Live friends-list query (F-1051) gauges, one receipt per peer for its
/// latest admitted query, tracked like [`InitialisationTransportCounts`].
/// Settled means a positive packet ACK covered that query's reliable bunch;
/// it proves no client execution.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct FriendsQueryReadiness {
    /// Queries held back from reliable transport, e.g. by send pressure.
    pub pending_admission: usize,
    pub queued: usize,
    pub settled: usize,
    pub awaiting_ack: usize,
    /// Settled latest query, no newer query pending, and no accepted
    /// `ServerFriendsListUpdated` since it was admitted.
    pub awaiting_response: usize,
}

/// Live `UReplicatedConnectedPlayer` record gauges (Mode A). Queued and
/// settled count record creations per connection, settled by a positive
/// packet ACK of their own reliable bunches; they prove no client execution.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct ConnectedPlayerRecordReadiness {
    /// Live sessions with a registered record.
    pub registered: usize,
    /// Registered records whose player is finalised.
    pub finalised: usize,
    /// Connections with a record or array change not yet admitted to
    /// transport (pressure, or a GameState channel not open yet).
    pub pending_send: usize,
    pub creations_queued: usize,
    pub creations_settled: usize,
    /// Queued record creations not yet ACKed.
    pub records_awaiting_ack: usize,
}

/// Live controller possession property gauges (F-1074), one receipt per
/// session for its single update; settled means a positive packet ACK
/// covered that update's reliable bunch, not that a client applied it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct ControllerPossessionReadiness {
    /// Sessions whose pawn open committed but whose update is not admitted
    /// yet (transport pressure).
    pub pending_send: usize,
    /// `queued` = `settled` + `awaiting_ack`.
    pub queued: usize,
    pub settled: usize,
    pub awaiting_ack: usize,
}

/// Live frozen-movement release gauges
/// ([`PeerLab::enable_frozen_movement_release`]), one release per session;
/// settled means a positive packet ACK covered the update's latest copy, not
/// that a client applied it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct FrozenMovementReleaseReadiness {
    /// Scheduled by the record's ACK, waiting out the configured delay.
    pub awaiting_delay: usize,
    /// Due, or lost to a NAK, and not sent yet: the pawn channel still has
    /// reliable bunches unACKed, or transport pressure.
    pub pending_send: usize,
    /// A copy sent, neither ACKed nor NAKed yet.
    pub awaiting_ack: usize,
    pub settled: usize,
}

/// Live pawn move gauges ([`PeerLab::enable_pawn_move_acks`]), one per
/// session at most each; counts only, never timestamps or locations.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct PawnMoveAckReadiness {
    /// Sessions that sent at least one decoded move.
    pub sessions_moving: usize,
    /// A move arrived since the session's last acknowledgement: due once
    /// [`MOVE_ACK_INTERVAL`] has passed and the pawn channel is settled.
    pub awaiting_ack: usize,
    /// Held back by transport pressure until ACK/NAK progress.
    pub deferred: usize,
    /// Sessions with a stored client-reported location.
    pub positions_held: usize,
}

/// Live level visibility gauges ([`PeerLab::enable_level_visibility_actors`]),
/// summed over connections; counts only, never level names.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct LevelVisibilityReadiness {
    /// Levels currently in the connections' visible sets.
    pub levels_visible: usize,
    /// Levels whose actors a connection holds (opened or still pending),
    /// visible or since hidden.
    pub levels_with_actors: usize,
    /// Level actor opens awaiting admission (transport pressure).
    pub opens_pending: usize,
    /// Admitted opens not yet ACKed.
    pub opens_awaiting_ack: usize,
    /// Admitted opens a positive ACK settled.
    pub opens_settled: usize,
    /// Opens refused by the channel budget, waiting for a released channel.
    pub opens_refused_waiting: usize,
    /// Actor channels level actors hold (opened, kept across hides).
    pub channels_held: usize,
    /// Further level actor channels the connections' budgets allow now:
    /// per connection the lesser of the cap left and the free channels
    /// beyond the reserve. Read per connection with one client connected.
    pub channel_budget_remaining: usize,
}

/// Fixed channel kinds of client-to-server unreliable actor bunches, from the
/// opens this connection committed; never an actor identity.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UnreliableChannelKind {
    Controller,
    Pawn,
    PlayerState,
    GameState,
    Other,
}

/// Count and last whole-bunch payload length of one channel kind, summed
/// over every connection since bind: `last_payload_bits` is whichever
/// connection's bunch of this kind arrived last.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct UnreliableChannelCounts {
    /// Lab-wide count since bind, all connections.
    pub bunches: u64,
    /// From the most recent such bunch on any connection.
    pub last_payload_bits: u64,
}

/// One controller actor-block field index seen in unreliable bunches, on any
/// connection since bind.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct UnreliableFieldCounts {
    pub index: u32,
    /// Lab-wide count since bind, all connections.
    pub count: u64,
    /// The field's own payload length (after its header) when last seen on
    /// any connection.
    pub last_payload_bits: u64,
}

/// Distinct controller field indices [`UnreliableActorBunchCounts`] keeps.
pub const UNRELIABLE_CONTROLLER_FIELD_ENTRIES: usize = 16;

/// Header-only breakdown of client-to-server unreliable actor bunches, which
/// stay dropped exactly as before (each is also in `actor_bunches_dropped`).
/// Counts and lengths only: no payloads, values or identities. Field indices
/// are read only on the controller channel, the one channel whose class field
/// bound the lab holds; a pawn or GameState field index needs a class bound
/// the lab does not have, so those channels are counted whole.
///
/// Lab-wide, not per session: counts accumulate over every connection since
/// bind and are not dropped when a session ends, one connection can fill the
/// field table for all, and each `last_payload_bits` comes from whichever
/// connection sent last. It reads cleanly only with one client connected.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct UnreliableActorBunchCounts {
    pub controller: UnreliableChannelCounts,
    pub pawn: UnreliableChannelCounts,
    pub player_state: UnreliableChannelCounts,
    pub game_state: UnreliableChannelCounts,
    /// Open actor channels that are none of the kinds above for their
    /// connection (managers, quests, replicated actors).
    pub other: UnreliableChannelCounts,
    /// At most [`UNRELIABLE_CONTROLLER_FIELD_ENTRIES`] distinct indices, in
    /// first-seen order, shared by every connection (the first connections
    /// to send can fill it); further indices count in
    /// `controller_field_overflow`.
    pub controller_fields: Vec<UnreliableFieldCounts>,
    pub controller_field_overflow: u64,
    /// Controller bunches not read to fields: partial parts, or open, close,
    /// paused, export or must-be-mapped forms.
    pub controller_unparsed: u64,
    /// Controller bunches whose body or field headers did not parse; fields
    /// read before the failure still count.
    pub controller_malformed: u64,
    /// Controller blocks addressed to a sub-object or carrying properties:
    /// counted, not read.
    pub controller_other_blocks: u64,
}

impl UnreliableActorBunchCounts {
    fn channel(&mut self, kind: UnreliableChannelKind) -> &mut UnreliableChannelCounts {
        match kind {
            UnreliableChannelKind::Controller => &mut self.controller,
            UnreliableChannelKind::Pawn => &mut self.pawn,
            UnreliableChannelKind::PlayerState => &mut self.player_state,
            UnreliableChannelKind::GameState => &mut self.game_state,
            UnreliableChannelKind::Other => &mut self.other,
        }
    }

    /// Count one dropped unreliable bunch of `kind`; `controller` is the
    /// controller's actor codec, given only for the controller channel.
    fn record(
        &mut self,
        kind: UnreliableChannelKind,
        bunch: &super::packet::Bunch,
        controller: Option<&super::actor_channel::ActorChannelCodec>,
    ) {
        let channel = self.channel(kind);
        bump(&mut channel.bunches);
        channel.last_payload_bits = bunch.payload.bit_len() as u64;
        let Some(codec) = controller else {
            return;
        };
        let h = &bunch.header;
        if h.partial
            || h.open
            || h.close
            || h.paused
            || h.has_package_map_exports
            || h.has_must_be_mapped_guids
        {
            bump(&mut self.controller_unparsed);
            return;
        }
        let Ok(body) = read_client_actor_bunch_body(&mut bunch.payload.reader(), codec) else {
            bump(&mut self.controller_malformed);
            return;
        };
        for block in &body.blocks {
            if block.has_rep_layout || !matches!(block.target, BlockTarget::Actor) {
                bump(&mut self.controller_other_blocks);
                continue;
            }
            let mut reader = block.payload.reader();
            while !reader.at_end() {
                let Ok(field) = read_field_header(
                    &mut reader,
                    BP_ICARUS_PLAYER_CONTROLLER_SURVIVAL_FIELD_INDEX_BOUND,
                    codec.limits.max_payload_bits,
                ) else {
                    bump(&mut self.controller_malformed);
                    return;
                };
                self.record_field(field.index, field.payload.bit_len());
            }
        }
    }

    fn record_field(&mut self, index: u32, payload_bits: usize) {
        if let Some(entry) = self
            .controller_fields
            .iter_mut()
            .find(|entry| entry.index == index)
        {
            bump(&mut entry.count);
            entry.last_payload_bits = payload_bits as u64;
        } else if self.controller_fields.len() < UNRELIABLE_CONTROLLER_FIELD_ENTRIES {
            self.controller_fields.push(UnreliableFieldCounts {
                index,
                count: 1,
                last_payload_bits: payload_bits as u64,
            });
        } else {
            bump(&mut self.controller_field_overflow);
        }
    }
}

/// Refused `ServerFriendsListUpdated` fields by [`FriendsListRejection`];
/// fixed keys, saturating counts, no payload values.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct FriendsListRejectionCounts {
    pub truncated: u64,
    pub array_count_over_limit: u64,
    pub entry_limit_exceeded: u64,
    pub string_length_over_limit: u64,
    pub string_missing_terminator: u64,
    pub string_invalid_encoding: u64,
    pub trailing_bits: u64,
    pub unclassified: u64,
}

impl FriendsListRejectionCounts {
    fn record(&mut self, reason: FriendsListRejection) {
        bump(match reason {
            FriendsListRejection::Truncated => &mut self.truncated,
            FriendsListRejection::ArrayCountOverLimit => &mut self.array_count_over_limit,
            FriendsListRejection::EntryLimitExceeded => &mut self.entry_limit_exceeded,
            FriendsListRejection::StringLengthOverLimit => &mut self.string_length_over_limit,
            FriendsListRejection::StringMissingTerminator => &mut self.string_missing_terminator,
            FriendsListRejection::StringInvalidEncoding => &mut self.string_invalid_encoding,
            FriendsListRejection::TrailingBits => &mut self.trailing_bits,
            FriendsListRejection::Unclassified => &mut self.unclassified,
        });
    }
}

#[derive(Default)]
struct InitialisationReceipt {
    channel: Option<u32>,
    first: u32,
    last: u32,
    settled: bool,
    abandoned: bool,
}

impl InitialisationReceipt {
    fn observe_positive_ack(&mut self, peer: &LabPeer, released: &[u32]) {
        if let Some(channel) = self.channel {
            self.abandoned |= released.contains(&channel);
            self.settled |=
                !self.abandoned && peer.actor_reliable_span_settled(channel, self.first, self.last);
        }
    }

    fn committed(peer: &LabPeer, channel: u32, previous: u32) -> Self {
        Self {
            channel: Some(channel),
            first: previous + 1,
            last: peer
                .actor_reliable_sequence(channel)
                .expect("committed actor channel"),
            ..Default::default()
        }
    }

    /// Every reliable bunch queued so far on open actor channel `channel`,
    /// its open included; `None` without that channel.
    fn through(peer: &LabPeer, channel: u32) -> Option<Self> {
        Some(Self {
            channel: Some(channel),
            first: 0,
            last: peer.actor_reliable_sequence(channel)?,
            ..Default::default()
        })
    }
}

/// What the client must have acknowledged before its own connected-player
/// record is sent with possession properties on (F-1116), committed when
/// the possession update is admitted. The client applies a channel's
/// reliable bunches in order, so with every one through the update ACKed it
/// holds the controller and the pawn the update names; the record's
/// controller reference then resolves to its local controller, which is
/// what finalisation needs to raise `OnConnectedPlayerInitialised`
/// (F-1103). A null controller would finalise without it, for good.
#[derive(Default)]
struct RecordPrerequisites {
    /// The controller channel through the possession update.
    controller: InitialisationReceipt,
    /// The pawn channel through its latest bunch at that admission.
    pawn: InitialisationReceipt,
}

impl RecordPrerequisites {
    fn settled(&self) -> bool {
        self.controller.settled && self.pawn.settled
    }
}

/// This session's `UReplicatedConnectedPlayer` record
/// (F-1073, F-1077): registered at
/// Begin acceptance, completed with the player, finalised with the player.
/// Its values are read from the session when composed, so a send always
/// carries the latest state.
struct ConnectedPlayerRecord {
    /// Runtime sub-object index under the GameState key: one driver-wide
    /// dynamic GUID per record, never reused.
    index: u32,
    /// `ConnectionStartTime`: lab world seconds at registration (the exact
    /// original clock is not bound, [I]).
    start_time: f32,
    /// `ConnectionCompleteTime`: lab world seconds at finalisation.
    complete_time: Option<f32>,
}

/// One queued record-creation bunch on a connection's GameState channel and
/// the number of records it creates.
struct ConnectedPlayerReceipt {
    receipt: InitialisationReceipt,
    records: usize,
    /// The bunch creates the connection's own player's record.
    own: bool,
}

/// One session's frozen-movement release
/// ([`PeerLab::enable_frozen_movement_release`]): scheduled once when the
/// creation of its own connected-player record settles, then sent once due
/// as an unreliable pawn property update, as the original sends it (S2,
/// M-1161). As UE re-sends a NAKed packet's still-current properties (and
/// as [`GameStateReplica`] does), a NAK of the latest copy makes the
/// property due again and its current value, still false, goes out again;
/// the first positive ACK of a copy settles the release.
#[derive(Default)]
struct FrozenMovementRelease {
    /// When the record creation settled (the trigger); set once.
    record_acked_at: Option<Instant>,
    /// When the release falls due; set with `record_acked_at`.
    due_at: Option<Instant>,
    /// Due, or its latest copy NAKed, and not sent yet.
    pending: bool,
    /// Held back by drainable transport pressure until ACK/NAK progress.
    deferred: bool,
    /// Packet IDs carrying the latest copy, while neither ACKed nor NAKed.
    in_flight: Vec<u32>,
    /// The first copy; set once, so a session releases at most once.
    sent_at: Option<Instant>,
    acked_at: Option<Instant>,
    /// The pawn was gone when a copy fell due; nothing more is sent.
    failed: bool,
}

impl FrozenMovementRelease {
    /// Scheduled, not yet due, sent or abandoned.
    fn awaiting_delay(&self, now: Instant) -> bool {
        self.due_at.is_some_and(|due| due > now) && self.idle()
    }

    /// Neither pending, sent nor abandoned.
    fn idle(&self) -> bool {
        !self.pending && self.sent_at.is_none() && !self.failed
    }

    /// Apply one packet outcome. A NAK of any part of the latest copy loses
    /// it whole, so the release is due again; an ACK of its last outstanding
    /// part settles it. Returns `Some(true)` when this settled the release
    /// and `Some(false)` when it lost the copy.
    fn observe(&mut self, packet_id: u32, acked: bool) -> Option<bool> {
        let position = self.in_flight.iter().position(|id| *id == packet_id)?;
        if !acked {
            self.in_flight.clear();
            self.pending = !self.failed;
            return Some(false);
        }
        self.in_flight.swap_remove(position);
        (self.in_flight.is_empty() && self.acked_at.is_none()).then_some(true)
    }
}

/// Shortest interval between two move acknowledgements on one connection
/// (F-1127): at most ten a second, as the original's cadence allows.
pub const MOVE_ACK_INTERVAL: Duration = Duration::from_millis(100);
/// Authored world bound for a client-reported location: each component
/// within this many centimetres of the origin (the engine's default half
/// world extent), F-1128.
pub const MOVE_WORLD_BOUND_CM: f64 = 1_048_576.0;
/// Authored speed bound between two stored locations, centimetres per
/// second of server arrival time, with [`MOVE_DISTANCE_SLACK_CM`] added for
/// arrival jitter (F-1128).
pub const MOVE_MAX_SPEED_CM_PER_S: f64 = 5_000.0;
pub const MOVE_DISTANCE_SLACK_CM: f64 = 500.0;
/// Shortest interval between two logged location refusals per connection.
pub const MOVE_VIOLATION_LOG_INTERVAL: Duration = Duration::from_secs(10);

/// Why a client-reported location was refused (F-1128).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MoveLocationRefusal {
    NotFinite,
    OutsideWorld,
    TooFar,
}

/// The authored location checks (F-1128): every component finite and within
/// [`MOVE_WORLD_BOUND_CM`], and, after a stored location `previous` stored
/// at its arrival instant, no farther from it than
/// [`MOVE_MAX_SPEED_CM_PER_S`] times the elapsed arrival time plus
/// [`MOVE_DISTANCE_SLACK_CM`].
pub fn check_move_location(
    previous: Option<([f64; 3], Instant)>,
    location: [f64; 3],
    now: Instant,
) -> std::result::Result<(), MoveLocationRefusal> {
    if !location.iter().all(|component| component.is_finite()) {
        return Err(MoveLocationRefusal::NotFinite);
    }
    if location
        .iter()
        .any(|component| component.abs() > MOVE_WORLD_BOUND_CM)
    {
        return Err(MoveLocationRefusal::OutsideWorld);
    }
    if let Some((stored, at)) = previous {
        let elapsed = now.saturating_duration_since(at).as_secs_f64();
        let distance = stored
            .iter()
            .zip(location)
            .map(|(from, to)| (to - from) * (to - from))
            .sum::<f64>()
            .sqrt();
        if distance > MOVE_MAX_SPEED_CM_PER_S * elapsed + MOVE_DISTANCE_SLACK_CM {
            return Err(MoveLocationRefusal::TooFar);
        }
    }
    Ok(())
}

/// One session's pawn moves ([`PeerLab::enable_pawn_move_acks`]): the
/// newest move by arrival (the client resets its timestamps, so never by
/// value) and whether it is acknowledged, and the last stored location.
/// Fixed size; dropped with the session.
#[derive(Default)]
struct PawnMoves {
    latest_timestamp: Option<f32>,
    /// A move arrived since the last acknowledgement.
    unacked: bool,
    last_ack_at: Option<Instant>,
    /// Held back by drainable transport pressure until ACK/NAK progress.
    deferred: bool,
    /// The last stored location (centimetres) and its arrival instant.
    location: Option<([f64; 3], Instant)>,
    /// The server's own placement (a saved or configured source, F-1140)
    /// and when the pawn opened there: the starting point the first stored
    /// location is checked against. Unset for the default placement, which
    /// keeps the world check alone, as before.
    placement: Option<([f64; 3], Instant)>,
    last_refusal_logged_at: Option<Instant>,
    counts: PawnMoveCounts,
}

/// One session's pawn move counters; counts and the movement mode only.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct PawnMoveCounts {
    pub moves: u64,
    pub malformed: u64,
    pub acks_sent: u64,
    pub positions_accepted: u64,
    pub positions_rejected: u64,
    pub positions_skipped: u64,
    /// The raw movement-mode byte of the newest decoded move (absent = 1).
    pub mode: Option<u8>,
}

/// Client-to-server bunches on one connection's pawn channel, counted by
/// header only (their content is not decoded: the pawn's RPC field indices,
/// such as `ServerUpdateClientFrozenMovement`'s, are not bound).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct PawnInboundCounts {
    /// Whole reliable bunches released to the session (otherwise ignored).
    pub reliable_bunches: u64,
    /// Unreliable bunches (dropped as before).
    pub unreliable_bunches: u64,
    /// Lab milliseconds of the latest such bunch.
    pub last_bunch_ms: Option<u64>,
}

/// One live session's frozen-movement release and pawn-channel traffic, in
/// lab milliseconds since bind; counts and times only. `connection` is the
/// session's ordinal, never an address or identity.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct FrozenMovementObservation {
    pub connection: u32,
    /// The positive ACK that settled the creation of this player's own
    /// connected-player record (the release's trigger).
    pub record_acked_ms: Option<u64>,
    pub release_due_ms: Option<u64>,
    /// The first copy of the `bServerFrozenMovement` = false update.
    pub release_sent_ms: Option<u64>,
    /// The positive ACK of a copy that settled it; not proof that the client
    /// applied it.
    pub release_acked_ms: Option<u64>,
    pub release_failed: bool,
    pub pawn_inbound: PawnInboundCounts,
    /// Pawn moves ([`PeerLab::enable_pawn_move_acks`]); zero when off.
    pub pawn_moves: PawnMoveCounts,
    pub last_move_ack_ms: Option<u64>,
    /// Where this session's pawn and controller opened from (F-1139); `None`
    /// before its controller open. Serialized as `spawn`: run logs never carry
    /// the word "placement" (authored world data uses it).
    pub spawn: Option<PlacementSource>,
}

/// Level visibility actors ([`PeerLab::enable_level_visibility_actors`]):
/// the qualified field-82 decoder and the actor identities a level's report
/// opens.
struct LevelVisibilityActors {
    /// Normal little-endian, package-map custom route (F-1021, F-1034),
    /// qualified against the release at configuration.
    profile: VisibilityDecodeProfile,
    actors: ReleaseLevelActorsProfile,
    engine_net_version: u32,
    /// Case-folded packages a report may never name: the persistent map's
    /// (its actors open with the session) and every package the release's
    /// own opens may export ([`ReleaseProfile::server_package_paths`]).
    reserved_packages: BTreeSet<String>,
    /// [`MAX_VISIBLE_LEVELS`]; smaller only in tests.
    max_visible_levels: usize,
    /// [`MAX_LEVEL_ACTOR_CHANNELS`]; smaller only in tests.
    max_level_channels: usize,
}

/// An admitted level: its case-folded key (set membership, the per-level
/// map, the queue and the driver-wide holders) and the client's spelling.
struct AdmittedLevel {
    key: String,
    spelling: String,
}

impl LevelVisibilityActors {
    /// The level a report names, if it is admissible as a streamed level:
    /// a string name with number 0 whose text is a plain `/Game/` package
    /// path (ASCII letters, digits, `_` and `-` in nonempty `/` segments, at
    /// most [`MAX_LEVEL_NAME_UNITS`]) that is not a reserved package in any
    /// letter case. Names compare ignoring ASCII case, as the engine
    /// compares names. This is an authored native rule. The original instead
    /// rejects a name whose package its own loader cannot find (and then
    /// closes the connection, F-1023), which depends on server packages a
    /// native server does not have; a refused name is counted, never closed
    /// on. Hardcoded indices stay unresolved (F-1021) and a nonzero number
    /// would need an FName suffix rendering that is not qualified, so both
    /// are refused.
    fn admitted(&self, name: &NetworkName) -> Option<AdmittedLevel> {
        let NetworkName::String { text, number: 0 } = name else {
            return None;
        };
        let units = text.units();
        if units.len() > MAX_LEVEL_NAME_UNITS {
            return None;
        }
        let spelling = String::from_utf16(units).ok()?;
        let root = spelling.get(..6)?;
        if !root.eq_ignore_ascii_case("/Game/") {
            return None;
        }
        let rest = &spelling[6..];
        let plain = rest.split('/').all(|segment| {
            !segment.is_empty()
                && segment
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
        });
        let key = spelling.to_ascii_lowercase();
        (plain && !self.reserved_packages.contains(&key)).then_some(AdmittedLevel { key, spelling })
    }

    /// Every static chain a level's actors name, leaf first, in `spelling`:
    /// one per actor, then the level's `PersistentLevel`, world and package
    /// chains.
    fn level_chains(&self, spelling: &str) -> Result<Vec<Vec<PathSegment>>> {
        let mut chains = (0..self.actors.names.len())
            .map(|index| self.actors.actor_chain(index, spelling))
            .collect::<Result<Vec<_>>>()?;
        let level = super::session_open::map_level_chain(spelling)?;
        for depth in 0..level.len() {
            chains.push(level[depth..].to_vec());
        }
        Ok(chains)
    }
}

/// A level whose actors at least one live connection holds: how many, and
/// the spelling every connection's exports carry. One object has one path
/// and one GUID in the driver's cache, so the first holder's spelling is
/// kept while any connection holds it, as the engine's name table keeps the
/// spelling a name was first created with; names resolve ignoring case.
struct LevelHolding {
    holders: usize,
    spelling: String,
}

/// Why a level actor open was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LevelRefusal {
    /// The connection's level actors hold its cap; hides close nothing, so
    /// no released channel lifts it.
    Cap,
    /// No free channel, or taking one would touch the reserve: a released
    /// actor channel can lift it.
    Channels,
}

/// One actor of a level a connection reported visible.
enum LevelActorSlot {
    /// Awaiting admission to reliable transport (its level is queued), with
    /// the cause it was refused for at its previous attempt, if any.
    Pending { refused: Option<LevelRefusal> },
    /// Admitted on `channel`; settled once a positive ACK left the channel
    /// reliably empty. Nothing else is ever sent on it.
    Opened {
        channel: u32,
        settled: bool,
        abandoned: bool,
    },
    /// Refused for this cause: a [`LevelRefusal::Channels`] refusal of a
    /// visible level is eligible again after an actor channel is released
    /// while the budget can admit it; either is eligible again on a later
    /// report of the level.
    Refused(LevelRefusal),
    /// Withdrawn by a hide or failed for good: eligible again only on a
    /// later report of the level.
    Unsent,
}

/// Independent live-player blockers, separate from the exclusive player phases.
/// Missing responses count only after their request batch enters transport.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct InitialisationBlockers {
    pub requests_pending_queue: usize,
    pub awaiting_loadout_response: usize,
    pub awaiting_bestiary_response: usize,
}

/// Fixed categories of configured initial opens, without actor identities.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct InitialOpenStageCounts {
    pub controller: usize,
    pub pawn: usize,
    pub game_state: usize,
    pub player_state: usize,
    pub managers: usize,
    pub quests: usize,
}

impl InitialOpenStageCounts {
    fn record(&mut self, stage: InitialOpenStage) {
        match stage {
            InitialOpenStage::Controller => self.controller += 1,
            InitialOpenStage::Pawn => self.pawn += 1,
            InitialOpenStage::GameState => self.game_state += 1,
            InitialOpenStage::PlayerState => self.player_state += 1,
            InitialOpenStage::Manager(_) => self.managers += 1,
            InitialOpenStage::Quest(_) => self.quests += 1,
        }
    }

    fn add(&mut self, other: &Self) {
        self.controller += other.controller;
        self.pawn += other.pawn;
        self.game_state += other.game_state;
        self.player_state += other.player_state;
        self.managers += other.managers;
        self.quests += other.quests;
    }

    fn is_empty(&self) -> bool {
        self == &Self::default()
    }
}

/// Live-peer initial-open outcomes. Committed means reliably queued, not
/// acknowledged or spawned by the client. Deferred is a subset of pending;
/// skipped stages were never attempted after a controller failure.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct InitialOpenReadiness {
    pub configured: InitialOpenStageCounts,
    pub committed: InitialOpenStageCounts,
    /// Retained positive-ACK receipts observed with the channel open,
    /// nonclosing and reliable-empty. These prove no client execution.
    pub settled: InitialOpenStageCounts,
    pub awaiting_ack: InitialOpenStageCounts,
    pub failed: InitialOpenStageCounts,
    pub skipped: InitialOpenStageCounts,
    pub pending: InitialOpenStageCounts,
    pub deferred: InitialOpenStageCounts,
    pub sequences_succeeded: usize,
    pub sequences_failed: usize,
    pub sequences_settled: usize,
}

#[derive(Default)]
struct InitialOpenOutcomes {
    configured: InitialOpenStageCounts,
    committed: InitialOpenStageCounts,
    settled: InitialOpenStageCounts,
    /// Only committed stages enter this bounded list. Later reliable traffic
    /// cannot retract a receipt once it has moved to `settled`.
    awaiting_ack: Vec<InitialOpenReceipt>,
    failed: InitialOpenStageCounts,
    skipped: InitialOpenStageCounts,
    deferred: bool,
}

struct InitialOpenReceipt {
    stage: InitialOpenStage,
    channel: u32,
    /// A later actor on a reused channel cannot satisfy the original open.
    abandoned: bool,
}

impl InitialOpenOutcomes {
    fn observe_positive_ack(&mut self, peer: &LabPeer, released: &[u32]) {
        self.awaiting_ack.retain_mut(|receipt| {
            receipt.abandoned |= released.contains(&receipt.channel);
            if !receipt.abandoned && peer.actor_channel_settled(receipt.channel) {
                self.settled.record(receipt.stage);
                false
            } else {
                true
            }
        });
    }
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
#[derive(Clone, Copy)]
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

#[derive(Clone, Copy)]
enum PendingPawnRestore {
    Uid,
    State(usize),
}

/// At most one unsent phase is retained; queued phases belong to the reliable
/// transport and are never replayed by this state machine.
enum PendingProfileExchange {
    Request(ProfileKind),
    Publish(ProfileKind, Vec<RepValue>),
    Initialisation,
    Complete,
}

/// Only uncommitted initial opens remain here. The number of stages is bounded
/// by the configured actor channels; committed opens stay in reliable transport.
#[derive(Clone, Copy)]
enum InitialOpenStage {
    Controller,
    Pawn,
    GameState,
    PlayerState,
    Manager(usize),
    Quest(usize),
}

struct PendingInitialOpens {
    remaining: VecDeque<InitialOpenStage>,
    /// Assigned once by the committed controller open, reused across deferrals.
    controller_guids: Option<(u32, u32)>,
    /// Preserve the Join-time host decision if a later peer joins while deferred.
    player_state_is_host: bool,
    /// First valid Begin received while the PlayerState stage is pending.
    begin_slot: Option<i32>,
    retry_ready: bool,
}

enum InitialOpenOutcome {
    Committed(u32),
    Deferred,
    Failed,
}

/// Privacy-preserving observations; recognition does not imply application.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
pub enum ControllerFieldTarget {
    Actor,
    PermittedSubobject,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
pub enum ControllerFieldDisposition {
    Recognized,
    Ignored,
    Malformed,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
pub struct ControllerFieldSummary {
    pub target: ControllerFieldTarget,
    pub index: u32,
    pub payload_bits: usize,
    pub disposition: ControllerFieldDisposition,
    pub count: u64,
}

/// At most 64 distinct observations per live peer; no payloads or identities.
#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct ControllerFieldDiagnostics {
    pub entries: Vec<ControllerFieldSummary>,
    pub overflow: u64,
    pub malformed_bunches: u64,
    /// Whole actor-body failures before any field dispatch, including native limits.
    pub body_parse_failures: u64,
    /// Declared block length exceeds the native cap but fits available payload.
    /// This establishes a limit rejection, not a legitimate RPC or profile.
    pub block_payload_limit_rejections: u64,
    /// Declared length exceeds available bits, whether or not also above the cap.
    pub block_payload_length_rejections: u64,
    /// Maximum declared length among cap rejections with that many bits available.
    pub max_rejected_block_payload_bits: u64,
    /// Content blocks skipped because their subobject GUID is not permitted.
    pub unknown_subobject_blocks: u64,
}

impl ControllerFieldDiagnostics {
    fn record(
        &mut self,
        target: ControllerFieldTarget,
        index: u32,
        payload_bits: usize,
        disposition: ControllerFieldDisposition,
    ) {
        self.record_count(target, index, payload_bits, disposition, 1);
    }

    fn record_count(
        &mut self,
        target: ControllerFieldTarget,
        index: u32,
        payload_bits: usize,
        disposition: ControllerFieldDisposition,
        count: u64,
    ) {
        if let Some(entry) = self.entries.iter_mut().find(|entry| {
            entry.target == target
                && entry.index == index
                && entry.payload_bits == payload_bits
                && entry.disposition == disposition
        }) {
            entry.count = entry.count.saturating_add(count);
        } else if self.entries.len() < 64 {
            self.entries.push(ControllerFieldSummary {
                target,
                index,
                payload_bits,
                disposition,
                count,
            });
        } else {
            self.overflow = self.overflow.saturating_add(count);
        }
    }
}

struct PeerEntry {
    /// This session's own connected-player record.
    connected_player: Option<ConnectedPlayerRecord>,
    /// Record indices this connection was last sent in its array, in order.
    connected_players_sent: Vec<u32>,
    /// A record or array change for this connection awaits admission.
    connected_players_pending: bool,
    connected_players_retry_ready: bool,
    /// Unsettled creation bunches; settled ones are counted and dropped.
    connected_player_receipts: Vec<ConnectedPlayerReceipt>,
    /// Records this connection created whose creation bunches settled.
    connected_player_creations_settled: usize,
    /// The committed PlayerController open's actor and PlayerState GUIDs.
    controller_open_guids: Option<(u32, u32)>,
    /// Controller possession properties (F-1074): due once this session's
    /// pawn open committed, until admitted to transport or abandoned.
    controller_possession_pending: bool,
    controller_possession_retry_ready: bool,
    /// The admitted update's own reliable bunch; its channel is set once
    /// admitted, so a session sends at most one.
    controller_possession_receipt: InitialisationReceipt,
    /// The update could not be admitted for good. With records on, the
    /// record is never sent and the session is closed (F-1116).
    controller_possession_failed: bool,
    /// ACKs the own record waits for with possession properties on.
    record_prerequisites: RecordPrerequisites,
    /// Frozen-movement release ([`PeerLab::enable_frozen_movement_release`]).
    frozen_release: FrozenMovementRelease,
    /// Client-to-server bunches on this connection's pawn channel.
    pawn_inbound: PawnInboundCounts,
    /// Pawn moves and their acknowledgement ([`PeerLab::enable_pawn_move_acks`]).
    pawn_moves: PawnMoves,
    /// This connection's placement, resolved once at its first open and
    /// shared by its controller and pawn opens (F-1140).
    placement: Option<Placement>,
    /// Case-folded packages this connection reported visible and that are
    /// still visible (at most [`MAX_VISIBLE_LEVELS`]).
    visible_levels: BTreeSet<String>,
    /// Per case-folded level whose actors this connection was due, one slot
    /// per actor name: an actor opens at most once per connection, and an
    /// opened actor is kept when its level is hidden (at most
    /// [`MAX_VISIBLE_LEVELS`] levels).
    level_actors: BTreeMap<String, Vec<LevelActorSlot>>,
    /// Levels with a pending slot, in report order, each once.
    level_actor_queue: VecDeque<String>,
    level_actors_retry_ready: bool,
    /// Some visible level's slot is refused for channels: a released actor
    /// channel arms a budget check ([`Self::level_channels_released`]).
    level_actors_refused: bool,
    /// An actor channel was released since: the next admission pass checks
    /// whether channel-refused slots can now be admitted.
    level_channels_released: bool,
    friends_query_pending: bool,
    friends_query_retry_ready: bool,
    /// The last admitted friends query's reliable bunch (F-1051).
    friends_query_receipt: InitialisationReceipt,
    /// Native policy (N-019): when this peer's oldest unanswered friends query
    /// was requested. Only an accepted `ServerFriendsListUpdated` clears it;
    /// ACKs, NAKs, re-queries and other inbound traffic never move it.
    friends_query_since: Option<Instant>,
    initialisation_request_receipt: InitialisationReceipt,
    initialisation_completion_receipt: InitialisationReceipt,
    controller_field_diagnostics: ControllerFieldDiagnostics,
    controller_startup_pawn_guid: Option<u32>,
    controller_startup_next: Option<usize>,
    controller_startup_queued: usize,
    controller_startup_settled: usize,
    controller_startup_failed: bool,
    controller_startup_retry_ready: bool,
    peer: LabPeer,
    auth_request: Option<u64>,
    auth_identity: Option<u64>,
    auth_accepted: bool,
    auth_ticket_digest: Option<[u8; 32]>,
    /// Character-state values actually composed into this session's pawn open.
    pawn_character_state: Option<BTreeMap<u32, RepValue>>,
    pawn_temperature: Option<PawnTemperatureState>,
    pawn_temperature_tick: Option<PawnTemperatureTickContext>,
    queued_pawn_temperature: Option<PawnTemperatureState>,
    /// Owned until this connection is removed, including idle/failure paths.
    pawn_uid: Option<i32>,
    /// One UID claim retained while the initial pawn open waits for capacity.
    pending_pawn_uid: Option<i32>,
    /// The saved character this pawn restored (F-950); its UID stays claimed
    /// for the saved record when the session ends.
    restored_character: Option<usize>,
    /// Successfully queued initial restore, retained only for this connection.
    restored_modifiers: Vec<RestoredModifier>,
    /// Explicit contexts exist only for members of restored_modifiers.
    restored_modifier_ticks: BTreeMap<i32, RestoredModifierTickContext>,
    /// Baselines owned by the reliable queue; NAK retransmission stays there.
    queued_modifier_times: BTreeMap<i32, u16>,
    pawn_property_retry_ready: bool,
    pending_pawn_restore: Option<PendingPawnRestore>,
    pawn_restore_retry_ready: bool,
    /// The cookie that created this peer; compared only, never reported.
    cookie: [u8; 20],
    last_seen: Instant,
    /// Native policy only: accepted inbound traffic does not reset delivery progress.
    delivery_progress_at: Option<Instant>,
    ack_pending: bool,
    ack_capacity_blocked: bool,
    /// One chosen control reply, without replaying admission or challenge entropy.
    pending_control_reply: Option<(Reply, ControlMessage)>,
    control_reply_retry_ready: bool,
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
    initial_opens: Option<PendingInitialOpens>,
    initial_open_outcomes: InitialOpenOutcomes,
    /// Set only after this connection's tracker open was reliably queued.
    player_history_channel: Option<u32>,
    /// The latest published History has not yet been queued for this peer.
    player_history_pending: bool,
    player_history_retry_ready: bool,
    /// F-951 / controller net fields: the first accepted ChrSlot this session.
    character_slot: Option<i32>,
    backend_proxy: Option<u32>,
    player_data: Option<u32>,
    bestiary: Option<u32>,
    profiles: ProfileExchange,
    pending_profile_exchange: Option<PendingProfileExchange>,
    profile_exchange_retry_ready: bool,
    /// Replicated actors this connection opened, by caller key.
    opened_actors: BTreeMap<u64, u32>,
    opened_incarnations: BTreeMap<u64, u64>,
    /// Slot to (incarnation, channel), retained until the reliable close drains.
    retiring_incarnations: BTreeMap<u64, (u64, u32)>,
    /// Legacy closes also reserve their slot against incarnation registration.
    closing_legacy_actors: BTreeMap<u32, u64>,
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

/// The packed moves one client unreliable pawn bunch carries: each actor
/// block's field-39 payloads, in order. Sub-object blocks and other fields
/// are skipped; a bunch the lab cannot read is an error.
fn pawn_bunch_moves(
    bunch: &super::packet::Bunch,
    codec: &RepLayoutCodec,
) -> Result<Vec<Result<super::character_move::PackedMove>>> {
    use super::actor_channel::{BlockTarget, read_client_actor_bunch_body};
    use super::character_move::read_server_move_packed;
    use super::rep_rpc::{
        BP_ICARUS_PLAYER_CHARACTER_SURVIVAL_FIELD_INDEX_BOUND, SERVER_MOVE_PACKED_FIELD_INDEX,
        read_field_header,
    };
    let header = &bunch.header;
    ensure!(
        !header.partial && !header.has_package_map_exports && !header.has_must_be_mapped_guids,
        "pawn bunch form not read for moves"
    );
    let body = read_client_actor_bunch_body(&mut bunch.payload.reader(), &codec.actor)?;
    let mut moves = Vec::new();
    for block in &body.blocks {
        if block.target != BlockTarget::Actor {
            continue;
        }
        ensure!(!block.has_rep_layout, "client pawn properties not read");
        let mut reader = block.payload.reader();
        while !reader.at_end() {
            // Bounded by the block; the move decoder caps its own container.
            let field = read_field_header(
                &mut reader,
                BP_ICARUS_PLAYER_CHARACTER_SURVIVAL_FIELD_INDEX_BOUND,
                usize::MAX,
            )?;
            if field.index == SERVER_MOVE_PACKED_FIELD_INDEX {
                moves.push(read_server_move_packed(&field.payload, codec));
            }
        }
    }
    Ok(moves)
}

/// One connection's placement (F-1139): resolved once, at its first open,
/// and counted by source; later opens reuse it. The restored saved location
/// is not available here: the save's character is matched at finalisation
/// (`restore_returning_pawn_uid`, which needs the character profile's slot),
/// after the controller and pawn open at Join, so `saved` is `None` until
/// the restore pipeline can match a player before its opens (F-1140).
fn connection_placement(
    slot: &mut Option<Placement>,
    resolver: &mut SpawnResolver,
    stats: &mut PeerLabStats,
    default: [f32; 3],
) -> Placement {
    if let Some(placement) = slot {
        return *placement;
    }
    let resolution = resolver.resolve(None, default);
    if resolution.saved_refused {
        bump(&mut stats.pawn_spawn_saved_refused);
    }
    bump(match resolution.placement.source {
        PlacementSource::Saved => &mut stats.pawn_spawns_saved,
        PlacementSource::Configured => &mut stats.pawn_spawns_configured,
        PlacementSource::Default => &mut stats.pawn_spawns_default,
    });
    *slot = Some(resolution.placement);
    resolution.placement
}

/// Apply one client unreliable pawn bunch's moves to its session
/// ([`PeerLab::enable_pawn_move_acks`]): each well-formed move with a
/// finite, nonnegative timestamp becomes the newest (by arrival) and due for
/// acknowledgement, and its location is stored after the authored checks;
/// anything else is counted malformed and skipped.
fn receive_pawn_moves(
    state: &mut PawnMoves,
    stats: &mut PeerLabStats,
    bunch: &super::packet::Bunch,
    codec: &RepLayoutCodec,
    now: Instant,
    connection: Option<u32>,
) {
    use super::character_move::MOVEMENT_MODE_NONE;
    let malformed = |state: &mut PawnMoves, stats: &mut PeerLabStats| {
        state.counts.malformed = state.counts.malformed.saturating_add(1);
        bump(&mut stats.pawn_moves_malformed);
    };
    let Ok(moves) = pawn_bunch_moves(bunch, codec) else {
        malformed(state, stats);
        return;
    };
    for packed in moves {
        let Some(packed) = packed.ok().filter(|packed| {
            let timestamp = packed.new.fields.timestamp;
            timestamp.is_finite() && timestamp >= 0.0
        }) else {
            malformed(state, stats);
            continue;
        };
        let mode = packed.new.movement_mode();
        state.latest_timestamp = Some(packed.new.fields.timestamp);
        state.unacked = true;
        state.counts.moves = state.counts.moves.saturating_add(1);
        state.counts.mode = packed.new.mode;
        bump(&mut stats.pawn_moves_received);
        // Only a world location counts: no base, and a movement mode.
        if packed.new.base.is_some() || mode == MOVEMENT_MODE_NONE {
            state.counts.positions_skipped = state.counts.positions_skipped.saturating_add(1);
            bump(&mut stats.pawn_move_positions_skipped);
            continue;
        }
        let location = packed.new.fields.location_cm();
        match check_move_location(state.location.or(state.placement), location, now) {
            Ok(()) => {
                state.location = Some((location, now));
                state.counts.positions_accepted = state.counts.positions_accepted.saturating_add(1);
                bump(&mut stats.pawn_move_positions_accepted);
            }
            Err(refusal) => {
                state.counts.positions_rejected = state.counts.positions_rejected.saturating_add(1);
                bump(&mut stats.pawn_move_positions_rejected);
                if state.last_refusal_logged_at.is_none_or(|at| {
                    now.saturating_duration_since(at) >= MOVE_VIOLATION_LOG_INTERVAL
                }) {
                    state.last_refusal_logged_at = Some(now);
                    bevy::log::warn!(
                        "connection {connection:?}: client pawn location refused ({refusal:?}); \
                         the stored location is kept ({} refused so far)",
                        state.counts.positions_rejected
                    );
                }
            }
        }
    }
}

/// Capacity planning may fill a cloned queue even when the live queue is
/// empty. Only outstanding traffic that can drain makes a refusal retryable.
fn transport_capacity_can_drain(entry: &PeerEntry, error: &anyhow::Error, channel: u32) -> bool {
    use super::sequencing::SendCapacityError;
    let temporary = match error.downcast_ref::<SendCapacityError>() {
        Some(SendCapacityError::SentRecordRing { .. }) => {
            !entry.peer.sequencer().notify().sent_records().is_empty()
        }
        Some(SendCapacityError::ReliableBunchQueue { .. }) => entry
            .peer
            .pending_actor_reliable(channel)
            .is_some_and(|pending| pending > 0),
        Some(SendCapacityError::ReliableControlQueue { .. }) => entry.peer.pending_reliable() > 0,
        Some(SendCapacityError::ReliableActorWindow {
            pending,
            additional,
            capacity,
        }) => *pending > 0 && additional <= capacity,
        None => false,
    };
    temporary && !entry.peer.is_failed()
}

/// One connection's level actor channel budget at one moment.
struct LevelChannelBudget {
    /// The per-connection cap ([`MAX_LEVEL_ACTOR_CHANNELS`]).
    cap: usize,
    /// The first channel above the session channels.
    first: u32,
    reserved: Vec<u32>,
    /// Channels its level actors hold.
    held: usize,
    /// Free actor channels from `first`, session channels excluded.
    free: usize,
}

impl LevelChannelBudget {
    /// Level actor channels it may still take: the cap left, and the free
    /// channels beyond the reserve, whichever is less.
    fn remaining(&self) -> usize {
        self.cap
            .saturating_sub(self.held)
            .min(self.free.saturating_sub(LEVEL_ACTOR_CHANNEL_RESERVE))
    }
}

/// Queue `key` once, arming the connection's next admission pass.
fn enqueue_level(entry: &mut PeerEntry, key: &str) {
    if !entry.level_actor_queue.iter().any(|queued| queued == key) {
        entry.level_actor_queue.push_back(key.to_owned());
    }
    entry.level_actors_retry_ready = true;
}

/// Make every never-admitted slot of `key` (refused or withdrawn) pending
/// again, remembering a refusal's cause, and queue the level.
fn requeue_level(entry: &mut PeerEntry, key: &str) {
    let Some(slots) = entry.level_actors.get_mut(key) else {
        return;
    };
    let mut any = false;
    for slot in slots.iter_mut() {
        let refused = match slot {
            LevelActorSlot::Refused(cause) => Some(*cause),
            LevelActorSlot::Unsent => None,
            LevelActorSlot::Pending { .. } | LevelActorSlot::Opened { .. } => continue,
        };
        *slot = LevelActorSlot::Pending { refused };
        any = true;
    }
    if any {
        enqueue_level(entry, key);
    }
}

/// Make at most `limit` channel-refused slots of visible levels pending
/// again, in level order, and note whether any stay refused for channels.
/// Returns how many it re-armed.
fn requeue_channel_refusals(entry: &mut PeerEntry, limit: usize) -> usize {
    let mut left = limit;
    let mut remaining = false;
    let visible: Vec<String> = entry.visible_levels.iter().cloned().collect();
    for key in visible {
        let mut queued = false;
        if let Some(slots) = entry.level_actors.get_mut(&key) {
            for slot in slots.iter_mut() {
                if !matches!(slot, LevelActorSlot::Refused(LevelRefusal::Channels)) {
                    continue;
                }
                if left == 0 {
                    remaining = true;
                    continue;
                }
                *slot = LevelActorSlot::Pending {
                    refused: Some(LevelRefusal::Channels),
                };
                left -= 1;
                queued = true;
            }
        }
        if queued {
            enqueue_level(entry, &key);
        }
    }
    entry.level_actors_refused = remaining;
    limit - left
}

/// Whether a connection can carry connected-player records: its GameState
/// open is committed (replica present) and its channel is open, not closing.
fn connected_players_receivable(entry: &PeerEntry, channel: Option<u32>) -> bool {
    entry.game_state.is_some()
        && channel.is_some_and(|channel| {
            entry.peer.has_actor_channel(channel) && !entry.peer.actor_channel_closing(channel)
        })
}

fn defer_pawn_restore(
    entry: &mut PeerEntry,
    error: &anyhow::Error,
    stage: PendingPawnRestore,
    channel: u32,
) {
    if transport_capacity_can_drain(entry, error, channel) {
        entry.pending_pawn_restore = Some(stage);
        entry.pawn_restore_retry_ready = false;
    }
}

/// A player registered by `AGameSession::RegisterPlayer` (F-784): its
/// `PlayerId` and initial name.
struct RegisteredPlayer {
    player_id: i32,
    name_units: Vec<u16>,
}

enum ControllerCall {
    Begin(i32),
    ShortTimeout,
    /// A decoded field-82 report (F-1021), dispatched only with
    /// [`PeerLab::enable_level_visibility_actors`].
    LevelVisibility(DecodedVisibilityParameter),
    FriendsList,
    Profile(ProfileResponse),
    Initialisation(InitialisationAnswer),
}

pub(crate) fn validate_controller_capture(
    payload: &Payload,
    engine_net_version: u32,
    order: super::archive::ByteOrder,
    components: [Option<u32>; 3],
) -> bool {
    // The private capture's validation never decodes field 82: it stays
    // identical whether or not level visibility actors are enabled.
    controller_initialisation_calls(
        payload,
        engine_net_version,
        order,
        components,
        &mut ControllerFieldDiagnostics::default(),
    )
    .is_ok()
}

/// Stage a complete controller bunch before applying any calls: malformed
/// trailing input must not partially mutate session state. Subobject payloads
/// are never interpreted as calls on the controller itself. Field 82 stays
/// ignored ([`controller_calls`] without a visibility profile).
fn controller_initialisation_calls(
    payload: &Payload,
    engine_net_version: u32,
    order: super::archive::ByteOrder,
    components: [Option<u32>; 3],
    diagnostics: &mut ControllerFieldDiagnostics,
) -> Result<(Vec<ControllerCall>, usize)> {
    controller_calls(
        payload,
        engine_net_version,
        order,
        components,
        None,
        diagnostics,
    )
    .map(|staged| (staged.calls, staged.ignored))
}

/// One controller bunch's staged calls and field accounting.
struct StagedControllerCalls {
    calls: Vec<ControllerCall>,
    /// Fields skipped as unrecognized.
    ignored: usize,
    /// Field-82 payloads the visibility decoder refused, each skipped alone.
    malformed_level_reports: usize,
}

/// [`controller_initialisation_calls`], and with `level_visibility` an
/// actor-block field 82 is decoded (exact consumption, absence distinct,
/// F-1021) and staged with the bunch's other calls; without it the field
/// stays ignored, as before. A field 82 the decoder refuses is skipped by
/// its declared length and diagnosed Malformed; the bunch's other fields
/// are applied exactly as when field 82 is ignored, so enabling level
/// visibility never costs a call the bunch would otherwise deliver.
fn controller_calls(
    payload: &Payload,
    engine_net_version: u32,
    order: super::archive::ByteOrder,
    components: [Option<u32>; 3],
    level_visibility: Option<&VisibilityDecodeProfile>,
    diagnostics: &mut ControllerFieldDiagnostics,
) -> Result<StagedControllerCalls> {
    let actor = actor_codec(engine_net_version, order);
    let body = match read_client_actor_bunch_body(&mut payload.reader(), &actor) {
        Ok(body) => body,
        Err(error) => {
            diagnostics.body_parse_failures = diagnostics.body_parse_failures.saturating_add(1);
            if let Some(rejection) =
                error.downcast_ref::<super::actor_channel::ContentBlockPayloadReadError>()
            {
                match *rejection {
                    super::actor_channel::ContentBlockPayloadReadError::Limit {
                        declared_bits,
                        available_bits,
                        limit_bits,
                    } if declared_bits <= available_bits => {
                        debug_assert!(declared_bits > limit_bits);
                        diagnostics.block_payload_limit_rejections =
                            diagnostics.block_payload_limit_rejections.saturating_add(1);
                        diagnostics.max_rejected_block_payload_bits = diagnostics
                            .max_rejected_block_payload_bits
                            .max(declared_bits as u64);
                    }
                    super::actor_channel::ContentBlockPayloadReadError::Limit {
                        declared_bits,
                        available_bits,
                        ..
                    }
                    | super::actor_channel::ContentBlockPayloadReadError::Truncated {
                        declared_bits,
                        available_bits,
                    } => {
                        debug_assert!(declared_bits > available_bits);
                        diagnostics.block_payload_length_rejections = diagnostics
                            .block_payload_length_rejections
                            .saturating_add(1);
                    }
                }
            }
            return Err(error);
        }
    };
    let codec = RepLayoutCodec {
        actor,
        checksums: false,
        limits: Default::default(),
    };
    let mut calls = Vec::new();
    let mut ignored = 0;
    let mut malformed_level_reports = 0;
    for block in body.blocks {
        let (component, bound) = match &block.target {
            BlockTarget::Actor => (None, BP_ICARUS_PLAYER_CONTROLLER_SURVIVAL_FIELD_INDEX_BOUND),
            BlockTarget::SubObject { object, .. } => {
                let Some(index) = components
                    .iter()
                    .position(|guid| *guid == Some(object.guid))
                else {
                    diagnostics.unknown_subobject_blocks =
                        diagnostics.unknown_subobject_blocks.saturating_add(1);
                    continue;
                };
                (
                    Some(index),
                    [
                        BACKEND_PROXY_FIELD_INDEX_BOUND,
                        PLAYER_DATA_FIELD_INDEX_BOUND,
                        BESTIARY_MANAGER_FIELD_INDEX_BOUND,
                    ][index],
                )
            }
        };
        ensure!(
            !block.has_rep_layout,
            "client controller properties unsupported"
        );
        let mut reader = block.payload.reader();
        while !reader.at_end() {
            let field = read_field_header(&mut reader, bound, actor.limits.max_payload_bits)?;
            let target = if component.is_none() {
                ControllerFieldTarget::Actor
            } else {
                ControllerFieldTarget::PermittedSubobject
            };
            let before = calls.len();
            if component.is_none()
                && field.index == SERVER_UPDATE_LEVEL_VISIBILITY_FIELD_INDEX
                && let Some(profile) = level_visibility
            {
                // F-1052 field identity; F-1021 decoder. The field is framed
                // by its own length, so a refusal skips it alone.
                let disposition = match parse_visibility_rpc82(
                    &field.payload,
                    profile,
                    VisibilityRpc82Layout::NormalSingleCustomStruct,
                ) {
                    Ok(parameter) => {
                        calls.push(ControllerCall::LevelVisibility(parameter));
                        ControllerFieldDisposition::Recognized
                    }
                    Err(_) => {
                        malformed_level_reports += 1;
                        ControllerFieldDisposition::Malformed
                    }
                };
                diagnostics.record(target, field.index, field.payload.bit_len(), disposition);
                continue;
            }
            let parsed = (|| -> Result<()> {
                if component.is_none()
                    && field.index == SERVER_BEGIN_PLAYER_INITIALISATION_FIELD_INDEX
                {
                    calls.push(ControllerCall::Begin(parse_begin_player_initialisation(
                        &field.payload,
                        &codec,
                    )?));
                } else if component.is_none() && field.index == SERVER_SHORT_TIMEOUT_FIELD_INDEX {
                    parse_rpc_payload(&field.payload, &[], &codec)?;
                    calls.push(ControllerCall::ShortTimeout);
                } else if component.is_none()
                    && field.index == SERVER_FRIENDS_LIST_UPDATED_FIELD_INDEX
                {
                    // Validate the complete bounded array before allowing its
                    // arrival to release connected-player finalisation.
                    // The classified reason rides on the unchanged error.
                    parse_rpc_payload(
                        &field.payload,
                        &server_friends_list_updated_parameters(),
                        &codec,
                    )
                    .map_err(|error| {
                        error.context(friends_list_rejection(&field.payload, &codec))
                    })?;
                    // Its friend IDs are not interpreted.
                    calls.push(ControllerCall::FriendsList);
                } else if component == Some(0)
                    && matches!(
                        field.index,
                        RESPONSE_GET_USER_PROFILE | RESPONSE_GET_CHARACTER_PROFILE
                    )
                {
                    let kind = if field.index == RESPONSE_GET_USER_PROFILE {
                        ProfileKind::User
                    } else {
                        ProfileKind::Character
                    };
                    calls.push(ControllerCall::Profile(profile_exchange::parse_response(
                        &field.payload,
                        kind,
                        &codec,
                    )?));
                } else if component == Some(1) && field.index == SERVER_RECEIVE_LOADOUT {
                    calls.push(ControllerCall::Initialisation(
                        InitialisationAnswer::Loadout,
                    ));
                } else if component == Some(2) && field.index == SERVER_RECEIVE_BESTIARY_DATA {
                    calls.push(ControllerCall::Initialisation(
                        InitialisationAnswer::Bestiary,
                    ));
                } else {
                    ignored += 1;
                }
                Ok(())
            })();
            let disposition = if parsed.is_err() {
                ControllerFieldDisposition::Malformed
            } else if calls.len() > before {
                ControllerFieldDisposition::Recognized
            } else {
                ControllerFieldDisposition::Ignored
            };
            diagnostics.record(target, field.index, field.payload.bit_len(), disposition);
            parsed?;
        }
    }
    Ok(StagedControllerCalls {
        calls,
        ignored,
        malformed_level_reports,
    })
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
    private_rpc82_capture: Option<super::selected_rpc_capture::PrivateRpc82Capture>,
    private_pawn_bunch_capture:
        Option<super::selected_rpc_capture::pawn_bunches::PrivatePawnBunchCapture>,
    controller_startup: Option<Vec<ControllerStartupAction>>,
    controller_startup_bound: u32,
    socket: UdpSocket,
    authenticator: Option<AuthClient>,
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
    /// D_ItemsStatic names in the same row order used by run's row handles.
    pawn_restore_item_rows: Vec<String>,
    /// The data tables a returning pawn's inventory restore reads (F-978).
    inventory_restore: Option<InventoryRestoreTables>,
    /// `D_ModifierStates` for a returning pawn's modifier restore (F-980).
    modifier_restore: Option<ModifierRestoreTables>,
    /// The data tables a returning pawn's stat restore reads (F-988).
    stat_restore: Option<StatRestoreTables>,
    /// The saved container inventories' item rows by `InventoryIndex`
    /// (`C016`): the inventories a returning pawn's items link; `None`
    /// until a loaded world supplies them, which leaves stats unrestored.
    linked_inventories: Option<BTreeMap<i32, Vec<u16>>>,
    /// Native run's opt-in UID owner; bare lab experiments retain templates.
    actor_uids: Option<crate::actor_uids::ActorUidLibrary>,
    /// Saved player characters returning pawns restore from (F-950), and the
    /// ones live sessions hold.
    returning_characters: Vec<super::recorders::SavedPlayerCharacter>,
    saved_player_controllers: Vec<super::recorders::SavedPlayerController>,
    restored_characters: Vec<usize>,
    next_pawn_object_id: Option<i32>,
    /// Opt-in GameState open after the PlayerController open.
    game_state_open: Option<GameStateSession>,
    /// Opt-in PlayerState open after the PlayerController open.
    player_state_open: Option<PlayerStateOpenPlan>,
    /// Opt-in `UReplicatedConnectedPlayer` records under the GameState's
    /// `ConnectedPlayers` ([`PeerLab::enable_connected_player_records`]).
    connected_players: Option<ConnectedPlayersPlan>,
    /// The next record's sub-object index under the GameState key.
    next_connected_player_record: u32,
    /// Opt-in controller possession properties after each pawn open
    /// ([`PeerLab::enable_controller_possession_properties`]).
    controller_possession: bool,
    /// A possession update was abandoned since the last close pass
    /// ([`PeerLab::close_abandoned_possessions`]); the cheap trigger.
    possession_abandoned: bool,
    /// Opt-in frozen-movement release with its delay after the record ACK
    /// ([`PeerLab::enable_frozen_movement_release`]).
    frozen_movement_release: Option<Duration>,
    /// Opt-in pawn move acknowledgements ([`PeerLab::enable_pawn_move_acks`]).
    pawn_move_acks: bool,
    /// Per-connection placement ([`PeerLab::enable_spawn_points`]); without
    /// points every connection keeps the controller plan's location.
    spawn: SpawnResolver,
    /// Opt-in level visibility dispatch and actor opens
    /// ([`PeerLab::enable_level_visibility_actors`]); `None` leaves field 82
    /// ignored.
    level_visibility: Option<LevelVisibilityActors>,
    /// Per case-folded level, the live connections holding its actors
    /// (opened or not) and its wire spelling. The static GUIDs this feature
    /// created for it, and nothing else touched, are forgotten when the last
    /// one leaves.
    level_actor_holders: BTreeMap<String, LevelHolding>,
    /// Header-only counts of dropped client unreliable actor bunches, kept
    /// only while `count_unreliable_actor_bunches` is set.
    unreliable_actor_bunches: UnreliableActorBunchCounts,
    /// Opt-in ([`PeerLab::enable_unreliable_actor_bunch_counts`]): off by
    /// default, so the receive path parses no dropped bunch for diagnostics.
    count_unreliable_actor_bunches: bool,
    /// Opt-in world manager opens after the PlayerState: each manager's
    /// key, plan and channel (F-859).
    manager_opens: Vec<(ActorKey, ManagerOpenPlan, u32)>,
    /// Base layout and checksums for `C019` quest actor opens (`F-934`).
    quest_open_base: Option<QuestOpenBasePlan>,
    /// Saved `C019` quest actor opens (`F-934`), each with its channel.
    quest_opens: Vec<(QuestOpenPlan, u32)>,
    /// Driver-wide NetGUIDs, replicated actors keyed by the caller's key.
    guid_cache: NetGuidCache,
    desired_incarnations: BTreeMap<u64, u64>,
    highest_incarnations: BTreeMap<u64, u64>,
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
    /// Explicit private file sink. Installation is restricted to pre-admission
    /// setup so selection and budgets cannot be reset during a run.
    pub fn install_private_rpc82_capture(
        &mut self,
        capture: super::selected_rpc_capture::PrivateRpc82Capture,
    ) -> Result<()> {
        ensure!(
            self.peers.is_empty() && self.peak_peers == 0 && self.private_rpc82_capture.is_none(),
            "capture installation requires fresh lab"
        );
        self.private_rpc82_capture = Some(capture);
        Ok(())
    }

    pub fn private_rpc82_capture_counts(
        &self,
    ) -> Option<super::selected_rpc_capture::CaptureCounts> {
        self.private_rpc82_capture
            .as_ref()
            .map(|capture| capture.counts())
    }

    pub fn finish_private_rpc82_capture(
        &mut self,
    ) -> Option<super::selected_rpc_capture::CaptureCounts> {
        self.private_rpc82_capture
            .as_mut()
            .map(|capture| capture.finish())
    }

    /// Explicit private pawn-channel bunch sink, also restricted to
    /// pre-admission setup. Nothing in the server configuration installs it.
    pub fn install_private_pawn_bunch_capture(
        &mut self,
        capture: super::selected_rpc_capture::pawn_bunches::PrivatePawnBunchCapture,
    ) -> Result<()> {
        ensure!(
            self.peers.is_empty()
                && self.peak_peers == 0
                && self.private_pawn_bunch_capture.is_none(),
            "capture installation requires fresh lab"
        );
        self.private_pawn_bunch_capture = Some(capture);
        Ok(())
    }

    pub fn private_pawn_bunch_capture_counts(
        &self,
    ) -> Option<super::selected_rpc_capture::pawn_bunches::PawnBunchCaptureCounts> {
        self.private_pawn_bunch_capture
            .as_ref()
            .map(|capture| capture.counts())
    }

    pub fn finish_private_pawn_bunch_capture(
        &mut self,
    ) -> Option<super::selected_rpc_capture::pawn_bunches::PawnBunchCaptureCounts> {
        self.private_pawn_bunch_capture
            .as_mut()
            .map(|capture| capture.finish())
    }
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

    /// Bind a native game listener whose peers must complete Steam ticket
    /// verification before any connection packet is processed. The CM worker
    /// owns validation and revocation; failure of that worker closes all peers.
    /// This enables connected qualification, not a compatibility claim.
    pub fn bind_authenticated(
        address: SocketAddrV4,
        config: PeerLabConfig,
        authenticator: AuthClient,
    ) -> Result<Self> {
        config.validate()?;
        ensure!(
            config.local_network_version.is_some(),
            "authenticated listener requires a release pin"
        );
        ensure!(
            config.admission.is_some(),
            "authenticated listener requires admission policy"
        );
        ensure!(
            !address.ip().is_multicast() && !address.ip().is_broadcast(),
            "invalid game bind address"
        );
        let authority = CookieAuthority::random().context("cookie secret")?;
        let socket = UdpSocket::bind(address).context("bind authenticated game listener")?;
        socket.set_nonblocking(true)?;
        let mut lab = Self::with_socket(socket, authority, config);
        lab.authenticator = Some(authenticator);
        // Authentication FString bodies can exceed the connection packet cap.
        // Keep one extra byte to detect kernel truncation at either limit.
        lab.buffer.resize(
            lab.buffer
                .len()
                .max(super::steam_auth::MAX_DATAGRAM_BYTES + 1),
            0,
        );
        Ok(lab)
    }

    fn with_socket(socket: UdpSocket, authority: CookieAuthority, config: PeerLabConfig) -> Self {
        Self {
            private_rpc82_capture: None,
            private_pawn_bunch_capture: None,
            socket,
            authenticator: None,
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
            controller_startup: None,
            controller_startup_bound: BP_ICARUS_PLAYER_CONTROLLER_SURVIVAL_FIELD_INDEX_BOUND,
            pawn_open: None,
            pawn_restore_item_rows: Vec::new(),
            inventory_restore: None,
            modifier_restore: None,
            stat_restore: None,
            linked_inventories: None,
            actor_uids: None,
            returning_characters: Vec::new(),
            saved_player_controllers: Vec::new(),
            restored_characters: Vec::new(),
            next_pawn_object_id: Some(crate::actor_uids::FIRST_NATIVE_OBJECT_ID),
            game_state_open: None,
            player_state_open: None,
            connected_players: None,
            next_connected_player_record: 0,
            controller_possession: false,
            possession_abandoned: false,
            frozen_movement_release: None,
            pawn_move_acks: false,
            spawn: SpawnResolver::default(),
            level_visibility: None,
            level_actor_holders: BTreeMap::new(),
            unreliable_actor_bunches: UnreliableActorBunchCounts::default(),
            count_unreliable_actor_bunches: false,
            manager_opens: Vec::new(),
            quest_open_base: None,
            quest_opens: Vec::new(),
            guid_cache: NetGuidCache::new(),
            desired_incarnations: BTreeMap::new(),
            highest_incarnations: BTreeMap::new(),
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

    /// Reconcile independently authored slot identities before actor selection.
    /// Registration is cumulative and bounded; removed slots retain their high-water mark.
    pub(crate) fn reconcile_actor_incarnations(
        &mut self,
        desired: &BTreeMap<u64, u64>,
    ) -> Result<()> {
        // The legacy bridge has no versioned actors to reconcile.
        if desired.is_empty() && self.highest_incarnations.is_empty() {
            return Ok(());
        }
        ensure!(
            desired.len() <= 1024,
            "native actor incarnation slot limit exceeded"
        );
        let additional = desired
            .keys()
            .filter(|key| !self.highest_incarnations.contains_key(*key))
            .count();
        ensure!(
            self.highest_incarnations.len() + additional <= 1024,
            "native actor incarnation slot limit exceeded"
        );
        for (&key, &incarnation) in desired {
            ensure!(
                incarnation > 0
                    && self
                        .highest_incarnations
                        .get(&key)
                        .is_none_or(|highest| incarnation >= *highest),
                "native actor incarnation regressed or is zero"
            );
            ensure!(
                self.peers.values().all(|entry| {
                    (!entry.opened_actors.contains_key(&key)
                        || entry.opened_incarnations.contains_key(&key))
                        && !entry
                            .closing_legacy_actors
                            .values()
                            .any(|closed| *closed == key)
                }),
                "native actor incarnation conflicts with a legacy actor"
            );
        }
        self.highest_incarnations
            .extend(desired.iter().map(|(&key, &inc)| (key, inc)));
        self.desired_incarnations.clone_from(desired);
        let stale: Vec<_> = self
            .peers
            .values()
            .filter_map(|entry| entry.connection_id.map(|id| (id, entry)))
            .flat_map(|(connection, entry)| {
                entry
                    .opened_incarnations
                    .iter()
                    .filter_map(move |(&key, &inc)| {
                        (desired.get(&key) != Some(&inc)).then_some((connection, key))
                    })
            })
            .collect();
        for (connection, key) in stale {
            self.close_actor(connection, key, 3);
        }
        self.retry_pending_closes();
        self.prune_incarnation_guids();
        Ok(())
    }

    fn prune_incarnation_guids(&mut self) {
        let mut retained: BTreeSet<_> = self
            .desired_incarnations
            .iter()
            .map(|(&key, &inc)| (key, inc))
            .collect();
        for entry in self.peers.values() {
            retained.extend(
                entry
                    .opened_incarnations
                    .iter()
                    .map(|(&key, &inc)| (key, inc)),
            );
            retained.extend(
                entry
                    .retiring_incarnations
                    .iter()
                    .map(|(&key, &(inc, _))| (key, inc)),
            );
        }
        self.guid_cache.retain_incarnations(&retained);
    }

    /// Number of per-peer incarnation closes awaiting acknowledgement.
    pub fn retiring_incarnation_count(&self) -> usize {
        self.peers
            .values()
            .map(|entry| entry.retiring_incarnations.len())
            .sum()
    }

    /// Native incarnation GUIDs retained by current desired or per-peer identities.
    pub fn incarnation_guid_count(&self) -> usize {
        self.guid_cache.incarnation_count()
    }

    /// Whether the desired incarnation is open and has no pending retirement.
    pub(crate) fn is_actor_incarnation_open(
        &self,
        connection: u32,
        key: u64,
        incarnation: u64,
    ) -> bool {
        self.desired_incarnations.get(&key) == Some(&incarnation)
            && self.peers.values().any(|entry| {
                entry.connection_id == Some(connection)
                    && entry.opened_incarnations.get(&key) == Some(&incarnation)
                    && entry.opened_actors.contains_key(&key)
                    && !entry.pending_closes.contains_key(&key)
                    && !entry.retiring_incarnations.contains_key(&key)
            })
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
        if let Some(incarnation) = entry.opened_incarnations.get(&key)
            && self.desired_incarnations.get(&key) != Some(incarnation)
        {
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
        if let Some(incarnation) = entry.opened_incarnations.remove(&key) {
            entry
                .retiring_incarnations
                .insert(key, (incarnation, channel));
        } else {
            entry.closing_legacy_actors.insert(channel, key);
        }
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
        open: ActorOpenSpec,
        first_channel: u32,
        engine_net_version: u32,
    ) -> Option<u32> {
        if self.highest_incarnations.contains_key(&key) {
            return None;
        }
        self.open_actor_version(
            connection,
            key,
            None,
            open,
            first_channel,
            engine_net_version,
        )
    }

    /// Open the current native incarnation once this peer's old close is acknowledged.
    pub(crate) fn open_actor_incarnation(
        &mut self,
        connection: u32,
        key: u64,
        incarnation: u64,
        open: ActorOpenSpec,
        first_channel: u32,
        engine_net_version: u32,
    ) -> Option<u32> {
        if self.desired_incarnations.get(&key) != Some(&incarnation) {
            return None;
        }
        self.open_actor_version(
            connection,
            key,
            Some(incarnation),
            open,
            first_channel,
            engine_net_version,
        )
    }

    fn open_actor_version(
        &mut self,
        connection: u32,
        key: u64,
        incarnation: Option<u64>,
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
        if entry.opened_actors.contains_key(&key) || entry.retiring_incarnations.contains_key(&key)
        {
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
        let guid = match incarnation {
            Some(incarnation) => cache.incarnation_guid(key, incarnation),
            None => cache.actor_guid(ActorKey::World(key)),
        };
        let guid = match guid {
            Ok(guid) => guid,
            Err(_) => {
                bump(&mut self.stats.actor_open_errors);
                return None;
            }
        };
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
        if let Some(incarnation) = incarnation {
            entry.opened_incarnations.insert(key, incarnation);
        }
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
        ensure!(
            self.peers
                .values()
                .all(|entry| entry.pawn_character_state.is_none()),
            "configure pawn opens before a pawn open is queued"
        );
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
    /// The authored object counter starts at 65,536 and advances per allocation;
    /// deferred opens retain their claim. Exhaustion fails closed. This is not
    /// an engine object index.
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

    /// Install strictly staged C025 identities before any connections. These
    /// choose the returning-player path independently of C026 pawn restore.
    pub fn enable_saved_player_controllers(
        &mut self,
        controllers: Vec<super::recorders::SavedPlayerController>,
    ) -> Result<()> {
        ensure!(
            self.peers.is_empty(),
            "saved controllers must precede connections"
        );
        self.saved_player_controllers = controllers;
        Ok(())
    }

    /// Whether future joins have a PlayerHistoryTracker open plan.
    pub fn has_player_history_open(&self) -> bool {
        self.manager_opens
            .iter()
            .any(|(key, ..)| *key == ManagerSlot::PlayerHistory.key())
    }

    /// Configure saved cosmetic/food row resolution from run's data.pak.
    /// Without a table no saved item-static names resolve.
    pub fn enable_pawn_restore_item_rows(&mut self, rows: &[String]) -> Result<()> {
        ensure!(
            self.peers.is_empty(),
            "configure pawn rows before admitting peers"
        );
        self.pawn_restore_item_rows = rows.to_vec();
        Ok(())
    }

    /// Also restore a returning pawn's saved inventories (F-969 to F-971,
    /// F-978) from these tables, built for the pawn content's inventories.
    pub fn enable_inventory_restore(&mut self, tables: InventoryRestoreTables) -> Result<()> {
        ensure!(
            self.peers.is_empty(),
            "configure inventory restore before admitting peers"
        );
        self.inventory_restore = Some(tables);
        Ok(())
    }

    /// Also restore a returning pawn's stats (F-988) from `tables`, once a
    /// loaded world supplies its container inventories
    /// ([`Self::set_linked_inventories`]); they need the inventory restore
    /// too.
    pub fn enable_stat_restore(&mut self, tables: StatRestoreTables) -> Result<()> {
        ensure!(
            self.peers.is_empty(),
            "configure stat restore before admitting peers"
        );
        self.stat_restore = Some(tables);
        Ok(())
    }

    /// The loaded world's container inventories' item rows by
    /// `InventoryIndex` (`C016`), which a returning pawn's items link.
    pub fn set_linked_inventories(&mut self, linked: BTreeMap<i32, Vec<u16>>) -> Result<()> {
        ensure!(
            self.peers.is_empty(),
            "set the linked inventories before admitting peers"
        );
        self.linked_inventories = Some(linked);
        Ok(())
    }

    /// Also restore a returning pawn's saved modifiers (F-980).
    pub fn enable_modifier_restore(&mut self, tables: ModifierRestoreTables) -> Result<()> {
        ensure!(
            self.peers.is_empty(),
            "configure modifier restore before admitting peers"
        );
        self.modifier_restore = Some(tables);
        Ok(())
    }

    /// A copy of a successfully restored modifier's current caller-owned time.
    /// Raw seconds and wire tenths are independent; neither polling nor reading
    /// this state advances it. State lasts only for this connection.
    pub fn restored_modifier_time(&self, connection_id: u32, uid: i32) -> Option<RemainingTime> {
        let entry = self
            .peers
            .values()
            .find(|entry| entry.connection_id == Some(connection_id))?;
        let modifier = entry
            .restored_modifiers
            .iter()
            .find(|modifier| modifier.uid == uid)?;
        Some(RemainingTime {
            seconds: modifier.remaining_seconds,
            replicated_tenths: modifier.remaining_tenths,
        })
    }

    /// Apply the pinned setter and OnRep ordering for this native-authority
    /// modifier, with caller-owned lifetime-notification effects. The connection
    /// and modifier must already exist before the handler can run. An accepted
    /// setter invokes it exactly once, including raw-only changes and overflow
    /// that retains the previous wire value; rejected setters do not invoke it.
    /// Authority preserves the accepted raw value at the OnRep boundary.
    ///
    /// The handler's final raw/wire state is retained for the same reliable,
    /// coalescing publication as [`Self::refresh_restored_modifier_times`].
    /// Polling and transport retries never invoke the handler again. The return
    /// value describes the setter before notification: `wire_changed` does not
    /// include handler effects. No ticking, expiry or persistence is inferred.
    pub fn set_restored_modifier_time(
        &mut self,
        connection_id: u32,
        uid: i32,
        requested_seconds: f32,
        notify_lifetime_updated: impl FnOnce(&mut RemainingTime),
    ) -> Result<RemainingTimeChange> {
        let mut state = self
            .restored_modifier_time(connection_id, uid)
            .context("no restored modifier for connection")?;
        let change = set_remaining_time(&mut state, Some(3), requested_seconds);
        if change.call_on_rep {
            on_rep_remaining_time(&mut state, Some(3), notify_lifetime_updated);
            self.refresh_restored_modifier_times(connection_id, &[(uid, state)])?;
        }
        Ok(change)
    }

    /// Publish explicitly resolved modifier times, atomically for one connection.
    /// The caller owns setter authority, quantization and callback effects. In
    /// particular, raw floating-point bits need not quantize to the supplied wire
    /// value (F-991 retains the old wire value on overflow). This API performs no
    /// ticking, expiry, stat recalculation or persistence. It cannot create a
    /// modifier or move one between peers. Unknown or duplicate IDs reject the
    /// whole batch. Only wire changes send; raw-only changes remain local.
    ///
    /// Polling coalesces the newest values while the pawn's reliable channel is
    /// unsettled. Successfully queued updates use reliable delivery; capacity
    /// refusals retry after ACK/NAK progress. This is native delivery policy,
    /// not a claim about the original's timer scheduling or reliability policy.
    pub fn refresh_restored_modifier_times(
        &mut self,
        connection_id: u32,
        updates: &[(i32, RemainingTime)],
    ) -> Result<()> {
        let entry = self
            .peers
            .values_mut()
            .find(|entry| entry.connection_id == Some(connection_id))
            .context("no restored modifier connection")?;
        ensure!(
            updates.len() <= entry.restored_modifiers.len(),
            "modifier update count exceeds restored state"
        );
        let mut seen = std::collections::BTreeSet::new();
        for (uid, _) in updates {
            ensure!(seen.insert(*uid), "duplicate modifier update");
            ensure!(
                entry
                    .restored_modifiers
                    .iter()
                    .any(|modifier| modifier.uid == *uid),
                "unknown restored modifier"
            );
        }
        for (uid, value) in updates {
            let modifier = entry
                .restored_modifiers
                .iter_mut()
                .find(|modifier| modifier.uid == *uid)
                .expect("validated modifier");
            modifier.remaining_seconds = value.seconds;
            modifier.remaining_tenths = value.replicated_tenths;
        }
        if !updates.is_empty() {
            entry.pawn_property_retry_ready = true;
        }
        Ok(())
    }

    /// Configure every caller-owned tick field for an existing restored
    /// modifier. Repeating this call replaces its context without changing
    /// remaining time. Unknown connections and UIDs cannot allocate contexts.
    pub fn configure_restored_modifier_tick(
        &mut self,
        connection_id: u32,
        uid: i32,
        context: RestoredModifierTickContext,
    ) -> Result<()> {
        let entry = self
            .peers
            .values_mut()
            .find(|entry| entry.connection_id == Some(connection_id))
            .context("no restored modifier connection")?;
        ensure!(
            entry
                .restored_modifiers
                .iter()
                .any(|modifier| modifier.uid == uid),
            "unknown restored modifier"
        );
        entry.restored_modifier_ticks.insert(uid, context);
        Ok(())
    }

    /// Current explicit context for this connection's restored modifier.
    /// Disconnect discards all contexts; reconnect requires configuration again.
    pub fn restored_modifier_tick_context(
        &self,
        connection_id: u32,
        uid: i32,
    ) -> Option<RestoredModifierTickContext> {
        self.peers
            .values()
            .find(|entry| entry.connection_id == Some(connection_id))?
            .restored_modifier_ticks
            .get(&uid)
            .copied()
    }

    /// Run one qualified modifier tick with caller-supplied elapsed time and
    /// effects. Validation precedes every callback. Reads the latest remaining
    /// time, including patches/setters since the previous tick, then retains
    /// the final raw/wire values and callback-mutated context.
    ///
    /// Publication coalesces reliably with pawn temperatures; polling, queue
    /// pressure and loss repair never replay callbacks. A destruction request
    /// remains a caller effect: it does not remove this modifier or context.
    /// This API infers no automatic clock, expiry, row effects or persistence.
    pub fn tick_restored_modifier(
        &mut self,
        connection_id: u32,
        uid: i32,
        delta_seconds: f32,
        calls: &mut impl super::modifier_timing::ModifierTickCalls,
    ) -> Result<()> {
        let entry = self
            .peers
            .values_mut()
            .find(|entry| entry.connection_id == Some(connection_id))
            .context("no restored modifier connection")?;
        let context = entry
            .restored_modifier_ticks
            .get(&uid)
            .copied()
            .context("restored modifier tick is not configured")?;
        let modifier = entry
            .restored_modifiers
            .iter_mut()
            .find(|modifier| modifier.uid == uid)
            .context("unknown restored modifier")?;
        let mut state = super::modifier_timing::ModifierTickState {
            remaining: RemainingTime {
                seconds: modifier.remaining_seconds,
                replicated_tenths: modifier.remaining_tenths,
            },
            owner_role_byte: context.owner_role_byte,
            interval_enabled: context.interval_enabled,
            accumulated_seconds: context.accumulated_seconds,
            interval_seconds: context.interval_seconds,
        };
        super::modifier_timing::tick_modifier(&mut state, delta_seconds, calls);
        modifier.remaining_seconds = state.remaining.seconds;
        modifier.remaining_tenths = state.remaining.replicated_tenths;
        entry.restored_modifier_ticks.insert(
            uid,
            RestoredModifierTickContext {
                owner_role_byte: state.owner_role_byte,
                interval_enabled: state.interval_enabled,
                accumulated_seconds: state.accumulated_seconds,
                interval_seconds: state.interval_seconds,
            },
        );
        entry.pawn_property_retry_ready = true;
        Ok(())
    }

    /// Latest explicitly supplied temperatures for a successfully queued pawn
    /// open. Unknown fields remain `None`; zero is a known value.
    pub fn pawn_temperature(&self, connection_id: u32) -> Option<PawnTemperatureState> {
        self.peers
            .values()
            .find(|entry| entry.connection_id == Some(connection_id))?
            .pawn_temperature
    }

    /// Apply the qualified internal-temperature setter to a known pawn value.
    /// The caller supplies both stat reads and notification effects; no bounds
    /// or initial temperature are inferred. Validation precedes every caller
    /// effect. The final value, including callback mutations, is retained for
    /// reliable publication even when the setter did not request notification.
    /// Polling and delivery retries never replay these calls. The return value
    /// reports the setter's notification decision, not whether a packet queued.
    pub fn set_pawn_internal_temperature(
        &mut self,
        connection_id: u32,
        requested: i32,
        calls: &mut impl super::temperature::InternalTemperatureCalls,
    ) -> Result<bool> {
        let mut value = self
            .pawn_temperature(connection_id)
            .and_then(|temperature| temperature.internal)
            .context("no known internal temperature for pawn connection")?;
        let notify = super::temperature::set_internal_temperature(&mut value, requested, calls);
        self.refresh_pawn_temperature(
            connection_id,
            PawnTemperatureState {
                internal: Some(value),
                ..Default::default()
            },
        )?;
        Ok(notify)
    }

    /// Atomically merge known temperature fields for this connection's pawn.
    /// `None` leaves a field unchanged. The caller owns calculation and timing;
    /// this does not infer climate, callbacks, ticking or persistence. Reliable
    /// publication coalesces with modifier times after the pawn channel settles.
    pub fn refresh_pawn_temperature(
        &mut self,
        connection_id: u32,
        patch: PawnTemperatureState,
    ) -> Result<()> {
        let entry = self
            .peers
            .values_mut()
            .find(|entry| entry.connection_id == Some(connection_id))
            .context("no pawn temperature connection")?;
        let current = entry
            .pawn_temperature
            .as_mut()
            .context("pawn open has not been queued")?;
        current.merge(patch);
        if !patch.is_empty() {
            entry.pawn_property_retry_ready = true;
        }
        Ok(())
    }

    /// Set every caller-owned timing field for explicit temperature ticks.
    /// Requires a queued pawn open and four known temperature values. Repeating
    /// this call intentionally replaces the timing context; it starts no clock.
    pub fn configure_pawn_temperature_tick(
        &mut self,
        connection_id: u32,
        context: PawnTemperatureTickContext,
    ) -> Result<()> {
        let entry = self
            .peers
            .values_mut()
            .find(|entry| entry.connection_id == Some(connection_id))
            .context("no pawn temperature connection")?;
        context.state(entry.pawn_temperature)?;
        entry.pawn_temperature_tick = Some(context);
        Ok(())
    }

    /// A copy of the current per-connection timing context. Removing the peer
    /// discards it; reconnect requires explicit configuration again.
    pub fn pawn_temperature_tick_context(
        &self,
        connection_id: u32,
    ) -> Option<PawnTemperatureTickContext> {
        self.peers
            .values()
            .find(|entry| entry.connection_id == Some(connection_id))?
            .pawn_temperature_tick
    }

    /// Run one temperature tick with caller-supplied elapsed time and effects.
    /// Validation precedes all effects. Reads the latest pawn temperatures,
    /// including explicit patches/setters since the previous tick, then retains
    /// the final values and timing context. Polling only delivers the coalesced
    /// result; ACK/pressure/loss retries never replay calls. This is not an
    /// automatic scheduler, climate profile or persistence adapter.
    pub fn tick_pawn_temperature(
        &mut self,
        connection_id: u32,
        delta_seconds: f32,
        calls: &mut impl super::temperature::TemperatureTickCalls,
    ) -> Result<()> {
        let entry = self
            .peers
            .values_mut()
            .find(|entry| entry.connection_id == Some(connection_id))
            .context("no pawn temperature connection")?;
        let context = entry
            .pawn_temperature_tick
            .context("pawn temperature tick is not configured")?;
        let mut state = context.state(entry.pawn_temperature)?;
        super::temperature::tick_temperature(&mut state, delta_seconds, calls);
        entry.pawn_temperature_tick = Some(PawnTemperatureTickContext {
            required_reference_present: state.required_reference_present,
            singleton_cached: state.singleton_cached,
            sample_interval_seconds: state.sample_interval_seconds,
            sample_elapsed_seconds: state.sample_elapsed_seconds,
            internal_remainder: state.internal_remainder,
        });
        entry.pawn_temperature = Some(PawnTemperatureState {
            external: Some(state.external),
            modified_external: Some(state.modified_external),
            modified_internal: Some(state.modified_internal),
            internal: Some(state.internal),
        });
        entry.pawn_property_retry_ready = true;
        Ok(())
    }

    fn update_pawn_properties(&mut self) {
        let Some((plan, channel, _)) = &self.pawn_open else {
            return;
        };
        let mut outgoing = Vec::new();
        let mut failed = Vec::new();
        for (address, entry) in &mut self.peers {
            if !entry.pawn_property_retry_ready || !entry.peer.actor_channel_settled(*channel) {
                continue;
            }
            entry.pawn_property_retry_ready = false;
            let due: Vec<_> = entry
                .restored_modifiers
                .iter()
                .filter(|modifier| {
                    entry.queued_modifier_times.get(&modifier.uid)
                        != Some(&modifier.remaining_tenths)
                })
                .cloned()
                .collect();
            let temperature_due = match (entry.pawn_temperature, entry.queued_pawn_temperature) {
                (Some(current), Some(queued)) => PawnTemperatureState {
                    external: current
                        .external
                        .filter(|_| current.external != queued.external),
                    modified_external: current
                        .modified_external
                        .filter(|_| current.modified_external != queued.modified_external),
                    modified_internal: current
                        .modified_internal
                        .filter(|_| current.modified_internal != queued.modified_internal),
                    internal: current
                        .internal
                        .filter(|_| current.internal != queued.internal),
                },
                _ => PawnTemperatureState::default(),
            };
            if due.is_empty() && temperature_due.is_empty() {
                continue;
            }
            let sent = (|| {
                let key = pawn_key(entry.connection_id.context("no pawn connection")?);
                let mut cache = self.guid_cache.clone();
                let mut exports = entry.exports.clone();
                let update = compose_live_pawn_update(
                    &mut cache,
                    &mut exports,
                    plan,
                    key,
                    (!temperature_due.is_empty()).then_some(&temperature_due),
                    &due,
                    entry.peer.connection_byte_order(),
                )?;
                let sent = entry.peer.reliable_actor_update_datagrams(
                    *channel,
                    update.payload,
                    update.exports.as_slice(),
                )?;
                commit_exports(&mut exports, &sent);
                self.guid_cache = cache;
                entry.exports = exports;
                Ok::<_, anyhow::Error>(sent)
            })();
            match sent {
                Ok(sent) => {
                    if !due.is_empty() {
                        bump(&mut self.stats.modifier_time_updates_queued);
                    }
                    for modifier in due {
                        entry
                            .queued_modifier_times
                            .insert(modifier.uid, modifier.remaining_tenths);
                    }
                    entry.ack_pending = false;
                    if !temperature_due.is_empty() {
                        entry
                            .queued_pawn_temperature
                            .as_mut()
                            .expect("temperature baseline exists")
                            .merge(temperature_due);
                        bump(&mut self.stats.pawn_temperature_updates_queued);
                    }
                    outgoing.push((*address, sent));
                }
                Err(_) => {
                    // No partial queue or baseline publication. ACK/NAK progress
                    // or an explicit refresh re-enables this connection's retry.
                    if !due.is_empty() {
                        bump(&mut self.stats.modifier_time_update_errors);
                    }
                    if !temperature_due.is_empty() {
                        bump(&mut self.stats.pawn_temperature_update_errors);
                    }
                    if entry.peer.is_failed() {
                        failed.push(*address);
                    }
                }
            }
        }
        for (address, sent) in outgoing {
            self.send_all(&sent, address);
        }
        for address in failed {
            self.remove_peer(address);
            bump(&mut self.stats.peers_failed);
        }
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
            entry.profiles.character_slot,
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
            Err(error) => {
                bump(&mut self.stats.returning_pawn_errors);
                defer_pawn_restore(entry, &error, PendingPawnRestore::Uid, *channel);
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
        self.restore_returning_pawn_state(address, index);
    }

    /// F-960: content follows the UID restore. A gap or error keeps both the
    /// session and its saved UID; Location is never passed to a wire encoder.
    fn restore_returning_pawn_state(&mut self, address: SocketAddrV4, index: usize) {
        let sent = (|| {
            let (plan, channel, _) = self.pawn_open.as_ref().context("no pawn plan")?;
            let entry = self.peers.get_mut(&address).context("no returning peer")?;
            let (_, _, content) = self.pawn_open.as_ref().context("no pawn content")?;
            let saved = &self.returning_characters[index];
            // Without a restore model, saved inputs are unknown, not evidence
            // that their contribution or the inventory resize pass is a no-op.
            let mut inventory_gap =
                self.inventory_restore.is_none() && !saved.inventories.is_empty();
            let mut inventories = Vec::new();
            if let Some(tables) = &self.inventory_restore {
                // Any failure, gap or error, leaves the inventories out of an
                // update that still restores the rest of the pawn.
                match restore_inventories(
                    &content.inventories,
                    &self.returning_characters[index].inventories,
                    tables,
                ) {
                    Ok(Ok(restored)) => inventories = restored,
                    Ok(Err(_)) | Err(_) => inventory_gap = true,
                }
            }
            let mut modifiers = Vec::new();
            let mut modifier_gaps = if self.modifier_restore.is_none() {
                saved
                    .modifiers
                    .iter()
                    .filter(|modifier| {
                        !modifier.time_remaining.is_nan() && modifier.time_remaining >= 0.0
                    })
                    .count()
            } else {
                0
            };
            if let Some(tables) = &self.modifier_restore {
                // A fresh pawn's modifier UIDs start at 1; its runtime
                // sub-objects after the open's inventories take the next
                // indices. Effectiveness affectors read as a fresh pawn's
                // 0: the stats they read follow the modifiers.
                let (restored, gaps) = restore_modifiers(
                    &self.returning_characters[index].modifiers,
                    tables,
                    1,
                    u32::try_from(content.inventories.len())?,
                    |_| 0,
                );
                modifiers = restored;
                modifier_gaps = gaps.len();
            }
            let open_state = entry
                .pawn_character_state
                .as_ref()
                .context("no pawn state")?;
            // Derived stats and the levels clamped by them require every
            // source. A partial modifier or inventory restore cannot establish
            // the final bounds; opening defaults are not a substitute.
            let source_gap = inventory_gap || modifier_gaps != 0;
            // Even unchanged opening items contribute equipment/attachment and
            // carrying grants; an empty slot delta does not prove unchanged stats.
            let stats_required = !inventories.is_empty()
                || !modifiers.is_empty()
                || content.inventories.iter().any(|inventory| {
                    inventory.slots.items.iter().any(|slot| slot.item.is_some())
                        || inventory
                            .current_weight
                            .unwrap_or(DEFAULT_CDO_CURRENT_WEIGHT)
                            != DEFAULT_CDO_CURRENT_WEIGHT
                });
            let mut stat_gap = source_gap || stats_required;
            let mut stats = None;
            if let (Some(tables), Some(inventory_tables), Some(open_stats)) =
                (&self.stat_restore, &self.inventory_restore, &content.stats)
            {
                let restored_stats = self
                    .linked_inventories
                    .as_ref()
                    .filter(|_| !source_gap)
                    .and_then(|linked| {
                        restored_pawn_stats(
                            tables,
                            inventory_tables,
                            &content.inventories,
                            &inventories,
                            linked,
                            &modifiers,
                        )
                        .ok()
                    })
                    .flatten();
                match restored_stats {
                    Some(restored) => {
                        stats = Some((stat_delta(open_stats, &restored.items)?, restored));
                        stat_gap = false;
                    }
                    None => stat_gap = true,
                }
            }
            let mut bounded = open_state.clone();
            if let (Some((_, restored)), Some(tables)) = (&stats, &self.stat_restore) {
                for (handle, value) in stat_bounds(restored, &tables.names) {
                    bounded.insert(handle, RepValue::Int(value));
                }
            }
            let current = CurrentPawnState {
                character_state: &bounded,
                // PawnContent opens Stomach without any entries.
                stomach_contents: 0,
            };
            let mut state = saved.state.clone();
            if stat_gap {
                // Omit only inputs whose restored bounds are unqualified.
                // Preserve the retained save and independently known fields.
                state.health = None;
                state.oxygen_level = None;
                state.food_level = None;
                state.water_level = None;
            }
            let restored = restore_saved_pawn(&state, &current, |name| {
                let name = String::from_utf16_lossy(name);
                Ok(self
                    .pawn_restore_item_rows
                    .iter()
                    .position(|row| row.eq_ignore_ascii_case(&name)))
            })?;
            let Ok(mut restored) = restored else {
                return Ok(None);
            };
            // The original restores TotalExperience from the character
            // profile's XP, the debt not subtracted (F-990).
            if let Some(xp) = entry.profiles.character_xp {
                bounded.insert(
                    super::pawn_open::character_state_handle::TOTAL_EXPERIENCE,
                    RepValue::Int(xp),
                );
            }
            // The bounds the stats moved go with the levels clamped to them.
            for (handle, value) in &bounded {
                if open_state.get(handle) != Some(value) {
                    restored
                        .character_state
                        .entry(*handle)
                        .or_insert_with(|| value.clone());
                }
            }
            restored.inventories = inventories;
            restored.modifiers = modifiers;
            let stats_sent = match stats {
                Some((delta, _)) => {
                    let sent = delta.is_some();
                    restored.stats = delta;
                    sent
                }
                None => false,
            };
            // The composer refuses AddSaveDataToStomach's unread ModifierID, so
            // restored stomach entries are left out and the rest still goes.
            let dropped = match restored.stomach.take() {
                Some(entries) if !entries.is_empty() => entries.len(),
                kept => {
                    restored.stomach = kept;
                    0
                }
            };
            let stomach = plan
                .components
                .iter()
                .position(|component| component.name == "Stomach")
                .context("no pawn stomach component")?;
            let key = pawn_key(entry.connection_id.context("no pawn connection")?);
            let mut cache = self.guid_cache.clone();
            let mut exports = entry.exports.clone();
            let update = compose_restored_pawn_update(
                &mut cache,
                &mut exports,
                plan,
                key,
                stomach,
                &restored,
                entry.peer.connection_byte_order(),
            )?;
            // Unacknowledged component paths need exports even for a
            // state-only restore. Keep those exports ahead of the body on
            // the same reliable channel, as for inventory/modifier classes.
            // Otherwise state-only updates retain their single-bunch limit.
            let datagrams = if restored.inventories.is_empty()
                && restored.modifiers.is_empty()
                && restored.stats.is_none()
                && update.exports.is_none()
            {
                exports.begin_bunch();
                vec![entry.peer.reliable_actor_bunch(*channel, update.payload)?]
            } else {
                let sent = entry.peer.reliable_actor_update_datagrams(
                    *channel,
                    update.payload,
                    update.exports.as_slice(),
                )?;
                commit_exports(&mut exports, &sent);
                sent.datagrams
            };
            self.guid_cache = cache;
            entry.exports = exports;
            entry.ack_pending = false;
            let restored_counts = (restored.inventories.len(), restored.modifiers.len());
            entry.queued_modifier_times = restored
                .modifiers
                .iter()
                .map(|modifier| (modifier.uid, modifier.remaining_tenths))
                .collect();
            entry.restored_modifiers = restored.modifiers;
            entry.restored_modifier_ticks.clear();
            Ok::<_, anyhow::Error>(Some((
                datagrams,
                dropped,
                restored_counts,
                (inventory_gap, stat_gap),
                modifier_gaps,
                stats_sent,
            )))
        })();
        match sent {
            Ok(Some((
                datagrams,
                dropped,
                (inventories, modifiers),
                (inventory_gap, stat_gap),
                modifier_gaps,
                stats_sent,
            ))) => {
                if stat_gap {
                    bump(&mut self.stats.returning_stat_gaps);
                }
                self.stats.returning_modifier_gaps = self
                    .stats
                    .returning_modifier_gaps
                    .saturating_add(modifier_gaps as u64);
                self.stats.returning_pawn_stomach_entries_dropped = self
                    .stats
                    .returning_pawn_stomach_entries_dropped
                    .saturating_add(dropped as u64);
                if inventory_gap {
                    bump(&mut self.stats.returning_inventory_gaps);
                }
                let mut delivered = true;
                for datagram in &datagrams {
                    delivered &= self.send(datagram, address);
                }
                if delivered {
                    bump(&mut self.stats.returning_pawn_updates_sent);
                    self.stats.returning_inventories_sent = self
                        .stats
                        .returning_inventories_sent
                        .saturating_add(inventories as u64);
                    self.stats.returning_modifiers_sent = self
                        .stats
                        .returning_modifiers_sent
                        .saturating_add(modifiers as u64);
                    if stats_sent {
                        bump(&mut self.stats.returning_stats_sent);
                    }
                }
            }
            Ok(None) => bump(&mut self.stats.returning_pawn_update_gaps),
            Err(error) => {
                bump(&mut self.stats.returning_pawn_update_errors);
                if let (Some(entry), Some((_, channel, _))) =
                    (self.peers.get_mut(&address), &self.pawn_open)
                {
                    defer_pawn_restore(entry, &error, PendingPawnRestore::State(index), *channel);
                }
            }
        }
    }

    // Native policy releases a pawn's UID at every session removal. The
    // original's FreeUniqueID call timing has not been established.
    fn remove_peer(&mut self, address: SocketAddrV4) -> Option<PeerEntry> {
        let entry = self.peers.remove(&address)?;
        if let Some(record) = &entry.connected_player {
            // `RemoveConnectedPlayer` drops the record from the array; the
            // remaining connections that listed it get the shrunk array at the
            // next retry pass. No sub-object destroy is sent ([I]: none was
            // observed and S2 had one player).
            bump(&mut self.stats.connected_player_records_removed);
            let index = record.index;
            self.mark_connected_players_due(|other| other.connected_players_sent.contains(&index));
            // Arrays are composed from live records only, and a NAK repair
            // resends stored bytes, so nothing names this record again. Its
            // numeric GUID stays consumed; indices are never reused.
            if let Some(session) = &self.game_state_open {
                self.guid_cache.forget_subobject(session.state.key, index);
            }
        }
        if let Some(connection) = entry.connection_id {
            // The pawn (actor, components, inventory and modifier subobjects)
            // opens only on its owner's connection, and connection IDs are
            // never reissued to a later Join (until the u32 counter wraps), so
            // no live connection can name these identities again. Releasing
            // them bounds the cache by live sessions; GUIDs are not reused
            // (F-1083).
            self.guid_cache.forget_actor(pawn_key(connection));
        }
        for package in entry.level_actors.keys() {
            self.release_level_actors(package);
        }
        if let (Some(capture), Some(connection)) =
            (&mut self.private_rpc82_capture, entry.connection_id)
        {
            capture.forget_connection(connection);
        }
        if let (Some(capture), Some(connection)) =
            (&mut self.private_pawn_bunch_capture, entry.connection_id)
        {
            capture.forget_connection(connection);
        }
        if let (Some(auth), Some(request)) = (&self.authenticator, entry.auth_request) {
            // Queue failure marks the service unusable; the next poll closes
            // every peer. Never silently drop an EndAuthSession operation.
            let _ = auth.end(request);
        }
        if let Some(index) = entry.restored_character {
            // The saved record keeps its UID, as its seed did before the join.
            self.restored_characters.retain(|held| *held != index);
        } else if let (Some(uid), Some(library)) = (entry.pawn_uid, &mut self.actor_uids) {
            library.free(uid);
        }
        if let (Some(uid), Some(library)) = (entry.pending_pawn_uid, &mut self.actor_uids) {
            library.free(uid);
        }
        self.prune_incarnation_guids();
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

    /// Explicitly add a member after a caller-qualified completed spawn
    /// (F-946), then publish through the bounded reliable prospect-refresh path.
    /// Requires enabled member refresh as the retained authoritative prospect.
    /// Names, XP and identity are caller-owned; no spawn timing, automatic
    /// finalisation or persistence is inferred. Existing user/slot pairs are
    /// unchanged and return false. Native matching ignores ASCII case as Join
    /// refresh does; the original's case rule is not qualified.
    ///
    /// A new member starts at status 2, not settled and not currently playing.
    /// All other fields remain unchanged. Validation of the complete update
    /// and future open precedes publication; failed calls change no state.
    /// Pressure/loss repair coalesces the latest prospect without adding again.
    pub fn complete_prospect_member_spawn(
        &mut self,
        member: super::prospect_info::SpawnedProspectMember,
    ) -> Result<bool> {
        let base = self
            .game_state_open
            .as_ref()
            .and_then(|session| session.member_refresh.as_ref())
            .context("spawn membership requires enabled member refresh")?;
        let (candidate, added) = base.with_spawned_member(member);
        if added {
            self.refresh_active_prospect(&candidate)?;
        }
        Ok(added)
    }

    /// Explicit caller-driven departure; no disconnect or dropship timing is inferred.
    /// Identity is ASCII-insensitive user ID plus exact slot, as spawn addition.
    /// Missing identities return false. All matching entries are removed, leaving
    /// other members and time unchanged. Whole-prospect preflight is atomic.
    pub fn remove_prospect_member(&mut self, user_id: &[u16], chr_slot: i32) -> Result<bool> {
        let mut candidate = self
            .game_state_open
            .as_ref()
            .and_then(|session| session.member_refresh.as_ref())
            .context("departure requires enabled member refresh")?
            .clone();
        let before = candidate.associated_members.len();
        candidate.associated_members.retain(|member| {
            member.chr_slot != chr_slot
                || !super::restore_rules::names_match(&member.user_id, user_id)
        });
        if candidate.associated_members.len() == before {
            return Ok(false);
        }
        self.refresh_active_prospect(&candidate)?;
        Ok(true)
    }

    /// Replace caller-owned member values, retaining the stored identity spelling.
    /// User ID (ASCII-insensitive) and exact slot select an existing member;
    /// identity changes require departure and addition. Missing, ambiguous
    /// duplicate identities and unchanged values return false. No other member
    /// or clock is changed; publication uses the whole-prospect atomic preflight
    /// and existing ACK, coalescing, pressure and loss-repair mechanisms.
    pub fn edit_prospect_member(
        &mut self,
        mut member: super::prospect_info::AssociatedMember,
    ) -> Result<bool> {
        let mut candidate = self
            .game_state_open
            .as_ref()
            .and_then(|session| session.member_refresh.as_ref())
            .context("member edit requires enabled member refresh")?
            .clone();
        let mut matches =
            candidate
                .associated_members
                .iter()
                .enumerate()
                .filter(|(_, existing)| {
                    existing.chr_slot == member.chr_slot
                        && super::restore_rules::names_match(&existing.user_id, &member.user_id)
                });
        let Some((index, existing)) = matches.next() else {
            return Ok(false);
        };
        if matches.next().is_some() {
            return Ok(false);
        }
        member.user_id.clone_from(&existing.user_id);
        if *existing == member {
            return Ok(false);
        }
        candidate.associated_members[index] = member;
        self.refresh_active_prospect(&candidate)?;
        Ok(true)
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
    /// other than the GameState's, on a taken channel, or on duplicate manager
    /// or quest keys. Once any manager opens are configured, this method rejects
    /// replacement; use the additive
    /// APIs to register further managers and quests.
    pub fn enable_manager_opens(
        &mut self,
        plans: Vec<(ManagerSlot, ManagerOpenPlan)>,
        first_channel: u32,
    ) -> Result<()> {
        ensure!(
            self.manager_opens.is_empty(),
            "manager opens are already configured"
        );
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
            ensure!(
                opens.iter().all(|(key, _, _)| *key != slot.key())
                    && self
                        .quest_opens
                        .iter()
                        .all(|(quest, _)| quest.key != slot.key()),
                "the manager key is already opened"
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

    /// Replicate each joined player's `UReplicatedConnectedPlayer` record
    /// under the GameState's `ConnectedPlayers` (Mode A,
    /// F-1077): a record registers
    /// at Begin acceptance and, once its player is finalised (F-973), one
    /// reliable GameState-channel bunch creates it with all seven handles and
    /// sends the whole array. Requires the PlayerController, pawn, GameState
    /// and PlayerState opens. A connection receives a record only once the
    /// controller, pawn and PlayerState it references are open on it; the lab
    /// opens those only on their owner's connection, so today each connection
    /// lists its own record. Must be configured before any peer exists.
    pub fn enable_connected_player_records(&mut self, plan: ConnectedPlayersPlan) -> Result<()> {
        ensure!(
            self.peers.is_empty() && self.peak_peers == 0,
            "connected-player records are configured before admission"
        );
        ensure!(
            self.player_controller_open.is_some()
                && self.pawn_open.is_some()
                && self.game_state_open.is_some()
                && self.player_state_open.is_some(),
            "connected-player records need the controller, pawn, GameState and PlayerState opens"
        );
        ensure!(
            !plan.record_class.is_empty()
                && plan.record_class[0].checksum != 0
                && plan.record_layout.len() == 7
                && plan.component_layout.len() == 3,
            "invalid connected-player record plan"
        );
        self.connected_players = Some(plan);
        Ok(())
    }

    /// Turn connected-player records off for a comparison run; the lab then
    /// behaves as without [`enable_connected_player_records`](Self::enable_connected_player_records).
    /// Must be called before any peer exists.
    pub fn disable_connected_player_records(&mut self) -> Result<()> {
        ensure!(
            self.peers.is_empty() && self.peak_peers == 0,
            "connected-player records are configured before admission"
        );
        self.connected_players = None;
        Ok(())
    }

    /// Whether joined players' connected-player records are replicated.
    pub fn has_connected_player_records(&self) -> bool {
        self.connected_players.is_some()
    }

    /// Send each session's controller possession properties: once its pawn
    /// open is committed, one reliable actor property update on its
    /// PlayerController channel with handle 17 `Pawn` and handle 20
    /// `IcarusPlayerCharacter`, both referencing that pawn. The connected-
    /// player record's finalisation check compares handle 20 with the
    /// record's pawn, and only replication sets it on a client (F-1074).
    /// With records on, a session's record waits until the client has ACKed
    /// the controller open, this update and the pawn open; a session whose
    /// update is abandoned is closed instead (F-1116).
    /// Unqualified with a stock client. Requires the PlayerController and
    /// pawn opens; must be configured before any peer exists.
    pub fn enable_controller_possession_properties(&mut self) -> Result<()> {
        ensure!(
            self.peers.is_empty() && self.peak_peers == 0,
            "controller possession properties are configured before admission"
        );
        ensure!(
            self.player_controller_open.is_some() && self.pawn_open.is_some(),
            "controller possession properties need the controller and pawn opens"
        );
        self.controller_possession = true;
        Ok(())
    }

    /// Turn controller possession properties off for a comparison run; the
    /// lab then sends exactly what it sends without them. Must be called
    /// before any peer exists.
    pub fn disable_controller_possession_properties(&mut self) -> Result<()> {
        ensure!(
            self.peers.is_empty() && self.peak_peers == 0,
            "controller possession properties are configured before admission"
        );
        self.controller_possession = false;
        Ok(())
    }

    /// Whether each session's controller possession properties are sent.
    pub fn has_controller_possession_properties(&self) -> bool {
        self.controller_possession
    }

    /// Release each session's pawn from frozen movement. The pawn still opens
    /// with `bServerFrozenMovement` true (F-797); when a positive ACK settles
    /// the bunch that created the session's own connected-player record (the
    /// lab's finalisation acknowledged, F-1077), and `delay` later, one
    /// unreliable pawn property update sets it false, as the original does
    /// once (F-1099). At most one release per session; a NAK of its latest
    /// copy sends the still-current false again, and the first ACK of a copy
    /// settles it. Unqualified with a stock client.
    /// Needs the pawn open and connected-player records; must be called
    /// before any peer exists.
    pub fn enable_frozen_movement_release(&mut self, delay: Duration) -> Result<()> {
        ensure!(
            self.peers.is_empty() && self.peak_peers == 0,
            "the frozen-movement release is configured before admission"
        );
        ensure!(
            self.pawn_open.is_some() && self.connected_players.is_some(),
            "the frozen-movement release needs the pawn open and connected-player records"
        );
        ensure!(
            delay <= Duration::from_millis(crate::config::MAX_FROZEN_MOVEMENT_RELEASE_DELAY_MS),
            "the frozen-movement release delay exceeds its bound"
        );
        self.frozen_movement_release = Some(delay);
        Ok(())
    }

    /// Turn the frozen-movement release off for a comparison run; the lab
    /// then sends exactly what it sends without it. Must be called before any
    /// peer exists.
    pub fn disable_frozen_movement_release(&mut self) -> Result<()> {
        ensure!(
            self.peers.is_empty() && self.peak_peers == 0,
            "the frozen-movement release is configured before admission"
        );
        self.frozen_movement_release = None;
        Ok(())
    }

    /// The configured delay when each session's pawn is released from
    /// frozen movement; `None` when it is not.
    pub fn frozen_movement_release_delay(&self) -> Option<Duration> {
        self.frozen_movement_release
    }

    /// Each live session's frozen-movement release and pawn-channel
    /// traffic, by connection ordinal.
    pub fn frozen_movement_observations(&self) -> Vec<FrozenMovementObservation> {
        let ms = |at: Option<Instant>| {
            at.map(|at| {
                at.saturating_duration_since(self.started)
                    .as_millis()
                    .min(u128::from(u64::MAX)) as u64
            })
        };
        let mut observations: Vec<_> = self
            .peers
            .values()
            .filter_map(|entry| {
                let release = &entry.frozen_release;
                Some(FrozenMovementObservation {
                    connection: entry.connection_id?,
                    record_acked_ms: ms(release.record_acked_at),
                    release_due_ms: ms(release.due_at),
                    release_sent_ms: ms(release.sent_at),
                    release_acked_ms: ms(release.acked_at),
                    release_failed: release.failed,
                    pawn_inbound: entry.pawn_inbound,
                    pawn_moves: entry.pawn_moves.counts,
                    last_move_ack_ms: ms(entry.pawn_moves.last_ack_at),
                    spawn: entry.placement.map(|placement| placement.source),
                })
            })
            .collect();
        observations.sort_unstable_by_key(|observation| observation.connection);
        observations
    }

    /// Decode each session's packed moves (pawn field 39, F-1127) and
    /// acknowledge the newest by arrival with one unreliable good-move
    /// response (field 30), at most every [`MOVE_ACK_INTERVAL`] and only when
    /// a move arrived since the last; nothing is resent on loss. Like the
    /// frozen-movement release, an acknowledgement waits until every reliable
    /// bunch on the pawn channel, its open included, is ACKed (the client
    /// drops unreliable bunches on a channel it has not opened); it does not
    /// wait for the release itself, which the original's own acknowledgements
    /// precede. A move without base and with a movement mode stores its
    /// location as session state after [`check_move_location`] (F-1128);
    /// nothing is ever corrected. Malformed moves are counted and skipped and
    /// never close a session. Unqualified with a stock client. Needs the pawn
    /// open; must be called before any peer exists.
    pub fn enable_pawn_move_acks(&mut self) -> Result<()> {
        ensure!(
            self.peers.is_empty() && self.peak_peers == 0,
            "pawn move acknowledgements are configured before admission"
        );
        ensure!(
            self.pawn_open.is_some(),
            "pawn move acknowledgements need the pawn open"
        );
        self.pawn_move_acks = true;
        Ok(())
    }

    /// Turn pawn move acknowledgements off for a comparison run; the lab
    /// then sends exactly what it sends without them. Must be called before
    /// any peer exists.
    pub fn disable_pawn_move_acks(&mut self) -> Result<()> {
        ensure!(
            self.peers.is_empty() && self.peak_peers == 0,
            "pawn move acknowledgements are configured before admission"
        );
        self.pawn_move_acks = false;
        Ok(())
    }

    /// Whether client moves are decoded and acknowledged.
    pub fn has_pawn_move_acks(&self) -> bool {
        self.pawn_move_acks
    }

    /// Place joining players at `config`'s validated spawn points,
    /// round-robin by join (F-1139): each connection's controller open (its
    /// spawn record and handle 19) and pawn open (its spawn record) carry
    /// the point's location; a yaw is kept but not sent (F-1140). A
    /// configured placement also starts that session's move store. Empty
    /// points restore the plan's location. Needs the controller open; must
    /// be called before any peer exists.
    pub fn enable_spawn_points(&mut self, config: &SpawnConfig) -> Result<()> {
        ensure!(
            self.peers.is_empty() && self.peak_peers == 0,
            "spawn points are configured before admission"
        );
        ensure!(
            config.is_empty() || self.player_controller_open.is_some(),
            "spawn points need the PlayerController open"
        );
        self.spawn = SpawnResolver::new(config)?;
        Ok(())
    }

    /// How many spawn points are configured.
    pub fn spawn_point_count(&self) -> usize {
        self.spawn.point_count()
    }

    /// Dispatch level visibility reports and open each newly visible level's
    /// actors (level visibility actors). A client reports a streamed level it
    /// made visible or hid with controller field 82 (F-1052; it streams tiles
    /// once its connected-player record is finalised, F-1080, and reports
    /// each, F-1081). Each actor-block field 82 on a connection's own
    /// controller channel is decoded with the qualified conditional decoder
    /// (exact consumption, absence distinct, staged with its bunch; F-1021,
    /// F-1024) and applied to that connection's visible-level set; a field
    /// the decoder refuses is skipped alone. A level newly added there gets
    /// one reliable static open per actor `profile` names, on fresh actor
    /// channels, as the original answers each report: admitted at the end of
    /// each poll, at most [`LEVEL_ACTOR_OPENS_PER_PASS`] per connection and
    /// within its channel budget ([`MAX_LEVEL_ACTOR_CHANNELS`], leaving
    /// [`LEVEL_ACTOR_CHANNEL_RESERVE`] free). An actor opens at most once per
    /// connection; a hide closes nothing, so its channel stays spent.
    /// Unqualified with a stock client; the visibility gate is not cleared
    /// by this code. Requires the PlayerController open and a release with
    /// level actor identities on the qualified visibility profile; must be
    /// configured before any peer exists.
    pub fn enable_level_visibility_actors(&mut self, profile: &ReleaseProfile) -> Result<()> {
        ensure!(
            self.peers.is_empty() && self.peak_peers == 0,
            "level visibility actors are configured before admission"
        );
        let controller = self
            .player_controller_open
            .as_ref()
            .context("level visibility actors need the controller open")?;
        let actors = profile
            .level_actors
            .clone()
            .context("the release names no level actors")?;
        actors.validate()?;
        ensure!(
            profile.engine_network_version == controller.engine_net_version,
            "level visibility release differs from the controller plan"
        );
        let decoder = VisibilityDecodeProfile::qualify(
            profile,
            VisibilityArchiveProfile::NormalLittleEndianPackageMapCustom,
        )?;
        let persistent_package = controller
            .profile
            .level
            .last()
            .context("controller level chain is empty")?
            .path
            .clone();
        let reserved_packages = profile
            .server_package_paths()
            .into_iter()
            .chain([persistent_package])
            .map(|package| package.to_ascii_lowercase())
            .collect();
        self.level_visibility = Some(LevelVisibilityActors {
            profile: decoder,
            actors,
            engine_net_version: controller.engine_net_version,
            reserved_packages,
            max_visible_levels: MAX_VISIBLE_LEVELS,
            max_level_channels: MAX_LEVEL_ACTOR_CHANNELS,
        });
        Ok(())
    }

    /// Turn level visibility actors off for a comparison run; the lab then
    /// sends exactly what it sends without them. Must be called before any
    /// peer exists.
    pub fn disable_level_visibility_actors(&mut self) -> Result<()> {
        ensure!(
            self.peers.is_empty() && self.peak_peers == 0,
            "level visibility actors are configured before admission"
        );
        self.level_visibility = None;
        Ok(())
    }

    /// Whether level visibility reports are dispatched and answered.
    pub fn has_level_visibility_actors(&self) -> bool {
        self.level_visibility.is_some()
    }

    /// Header-only counts of the client unreliable actor bunches dropped
    /// since bind, by channel kind and controller field index: lab-wide over
    /// every connection, kept after sessions end, so they read cleanly only
    /// with one client connected (the probe's `unreliable_actor_bunches`).
    /// All zero unless [`Self::enable_unreliable_actor_bunch_counts`] was
    /// called.
    pub fn unreliable_actor_bunch_counts(&self) -> &UnreliableActorBunchCounts {
        &self.unreliable_actor_bunches
    }

    /// Count dropped client unreliable actor bunches by channel kind and
    /// controller field from now on. A diagnostic for probes: it parses each
    /// such controller bunch's headers on the receive path, so the server
    /// leaves it off. The bunches stay dropped either way.
    pub fn enable_unreliable_actor_bunch_counts(&mut self) {
        self.count_unreliable_actor_bunches = true;
    }

    /// Configure at most 32 explicit actions once, before admitting any peers.
    /// Actions retain caller order and start only after all initial opens queue
    /// successfully and the controller and referenced pawn channels settle.
    /// This conservative ACK policy does not establish geometry or possession.
    pub fn enable_controller_startup(
        &mut self,
        profile: &ReleaseProfile,
        actions: Vec<ControllerStartupAction>,
    ) -> Result<()> {
        ensure!(
            self.peers.is_empty(),
            "configure controller startup before peers"
        );
        ensure!(
            self.controller_startup.is_none(),
            "controller startup already configured"
        );
        let controller = self
            .player_controller_open
            .as_ref()
            .context("controller startup requires a controller plan")?;
        validate_controller_startup(profile, &actions, controller, self.pawn_open.is_some())?;
        self.controller_startup_bound =
            profile.class_net_cache.bp_icarus_player_controller_survival;
        self.controller_startup = Some(actions);
        Ok(())
    }

    /// Open each joined peer's PlayerController with `plan` (module docs):
    /// registers the plan's level chain in the driver-wide GUID cache first,
    /// as the original does at world start. Requires admission. Configuration
    /// is accepted once; replacement is rejected without modifying GUIDs.
    pub fn enable_player_controller_open(&mut self, plan: PlayerControllerOpenPlan) -> Result<()> {
        ensure!(
            self.config.admission.is_some(),
            "the PlayerController open needs game admission"
        );
        ensure!(
            self.player_controller_open.is_none(),
            "the PlayerController open is already configured"
        );
        ensure!(
            self.free_session_channel(plan.channel_index),
            "the PlayerController channel collides with another channel or is out of range"
        );
        ensure!(
            self.guid_cache.register_level(&plan.profile.level) != 0,
            "static NetGUID space exhausted"
        );
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
            closed_by_delivery_stall: self.stats.peers_delivery_stalled,
            closed_by_friends_query_timeout: self.stats.peers_friends_query_expired,
            closed_by_possession_abandoned: self.stats.peers_closed_by_possession_abandoned,
            closed_by_client: self.stats.peers_closed_by_client,
            closed_by_reply: self.stats.peers_closed,
            upgrades_sent: self.stats.upgrades_sent,
            closed_failed: self.stats.peers_failed,
            closed_by_authentication: self.stats.peers_auth_closed,
            closed_by_shutdown: self.stats.peers_shutdown,
            player_controller_opens_sent: self.stats.player_controller_opens_sent,
            initialisation_slots_recorded: self.stats.initialisation_slots_recorded,
            initialisation_calls_ignored: self.stats.initialisation_calls_ignored,
            profile_user_responses_recognized: self.stats.profile_user_responses_recognized,
            profile_character_responses_recognized: self
                .stats
                .profile_character_responses_recognized,
            profile_user_publications_deferred: self.stats.profile_user_publications_deferred,
            profile_character_publications_deferred: self
                .stats
                .profile_character_publications_deferred,
            profile_user_requests_sent: self.stats.profile_user_requests_sent,
            profile_character_requests_sent: self.stats.profile_character_requests_sent,
            profile_user_responses_received: self.stats.profile_user_responses_received,
            profile_character_responses_received: self.stats.profile_character_responses_received,
            profile_user_failures: self.stats.profile_user_failures,
            profile_character_failures: self.stats.profile_character_failures,
            profile_responses_ignored: self.stats.profile_responses_ignored,
            profile_players_failed: self.stats.profile_players_failed,
            profile_exchange_errors: self.stats.profile_exchange_errors,
            profile_updates_sent: self.stats.profile_updates_sent,
            loadout_requests_sent: self.stats.loadout_requests_sent,
            bestiary_requests_sent: self.stats.bestiary_requests_sent,
            associated_prospects_sent: self.stats.associated_prospects_sent,
            loadout_responses_received: self.stats.loadout_responses_received,
            bestiary_responses_received: self.stats.bestiary_responses_received,
            initialisation_answers_ignored: self.stats.initialisation_answers_ignored,
            players_initialised: self.stats.players_initialised,
            friends_lists_received: self.stats.friends_lists_received,
            friends_list_queries_queued: self.stats.friends_list_queries_queued,
            friends_list_queries_acknowledged: self.stats.friends_list_queries_acknowledged,
            friends_lists_ignored_unregistered: self.stats.friends_lists_ignored_unregistered,
            friends_lists_ignored_already_ready: self.stats.friends_lists_ignored_already_ready,
            friends_list_rejections: self.stats.friends_list_rejections,
            players_finalised: self.stats.players_finalised,
            connected_player_records_created: self.stats.connected_player_records_created,
            connected_player_record_creations_sent: self
                .stats
                .connected_player_record_creations_sent,
            connected_player_record_creations_acked: self
                .stats
                .connected_player_record_creations_acked,
            connected_player_record_updates_sent: self.stats.connected_player_record_updates_sent,
            connected_player_array_updates_sent: self.stats.connected_player_array_updates_sent,
            connected_player_record_sends_deferred: self
                .stats
                .connected_player_record_sends_deferred,
            connected_player_records_removed: self.stats.connected_player_records_removed,
            connected_player_record_errors: self.stats.connected_player_record_errors,
            controller_possession_updates_sent: self.stats.controller_possession_updates_sent,
            controller_possession_updates_acked: self.stats.controller_possession_updates_acked,
            controller_possession_sends_deferred: self.stats.controller_possession_sends_deferred,
            controller_possession_errors: self.stats.controller_possession_errors,
            frozen_movement_releases_scheduled: self.stats.frozen_movement_releases_scheduled,
            frozen_movement_releases_sent: self.stats.frozen_movement_releases_sent,
            frozen_movement_releases_acked: self.stats.frozen_movement_releases_acked,
            frozen_movement_release_resends: self.stats.frozen_movement_release_resends,
            frozen_movement_release_sends_deferred: self
                .stats
                .frozen_movement_release_sends_deferred,
            frozen_movement_release_errors: self.stats.frozen_movement_release_errors,
            pawn_bunches_received_reliable: self.stats.pawn_bunches_received_reliable,
            pawn_bunches_received_unreliable: self.stats.pawn_bunches_received_unreliable,
            pawn_moves_received: self.stats.pawn_moves_received,
            pawn_moves_malformed: self.stats.pawn_moves_malformed,
            pawn_move_acks_sent: self.stats.pawn_move_acks_sent,
            pawn_move_ack_sends_deferred: self.stats.pawn_move_ack_sends_deferred,
            pawn_move_ack_errors: self.stats.pawn_move_ack_errors,
            pawn_move_positions_accepted: self.stats.pawn_move_positions_accepted,
            pawn_move_positions_rejected: self.stats.pawn_move_positions_rejected,
            pawn_move_positions_skipped: self.stats.pawn_move_positions_skipped,
            pawn_spawns_saved: self.stats.pawn_spawns_saved,
            pawn_spawns_configured: self.stats.pawn_spawns_configured,
            pawn_spawns_default: self.stats.pawn_spawns_default,
            pawn_spawn_saved_refused: self.stats.pawn_spawn_saved_refused,
            level_visibility_reports_added: self.stats.level_visibility_reports_added,
            level_visibility_reports_duplicate: self.stats.level_visibility_reports_duplicate,
            level_visibility_reports_hidden: self.stats.level_visibility_reports_hidden,
            level_visibility_reports_over_bound: self.stats.level_visibility_reports_over_bound,
            level_visibility_reports_malformed: self.stats.level_visibility_reports_malformed,
            level_visibility_reports_refused_name: self.stats.level_visibility_reports_refused_name,
            level_visibility_reports_absent: self.stats.level_visibility_reports_absent,
            level_actor_opens_sent: self.stats.level_actor_opens_sent,
            level_actor_opens_acked: self.stats.level_actor_opens_acked,
            level_actor_opens_deferred: self.stats.level_actor_opens_deferred,
            level_actor_opens_refused: self.stats.level_actor_opens_refused,
            level_actor_opens_requeued: self.stats.level_actor_opens_requeued,
            level_actor_open_errors: self.stats.level_actor_open_errors,
            level_actor_opens_withdrawn: self.stats.level_actor_opens_withdrawn,
            level_hides_actors_kept: self.stats.level_hides_actors_kept,
            player_character_id_updates_sent: self.stats.player_character_id_updates_sent,
            returning_pawns_restored: self.stats.returning_pawns_restored,
            returning_pawn_updates_sent: self.stats.returning_pawn_updates_sent,
            returning_pawn_update_gaps: self.stats.returning_pawn_update_gaps,
            returning_pawn_stomach_entries_dropped: self
                .stats
                .returning_pawn_stomach_entries_dropped,
            returning_pawn_update_errors: self.stats.returning_pawn_update_errors,
            returning_inventories_sent: self.stats.returning_inventories_sent,
            returning_inventory_gaps: self.stats.returning_inventory_gaps,
            returning_modifiers_sent: self.stats.returning_modifiers_sent,
            returning_modifier_gaps: self.stats.returning_modifier_gaps,
            modifier_time_updates_queued: self.stats.modifier_time_updates_queued,
            modifier_time_update_errors: self.stats.modifier_time_update_errors,
            pawn_temperature_updates_queued: self.stats.pawn_temperature_updates_queued,
            pawn_temperature_update_errors: self.stats.pawn_temperature_update_errors,
            returning_stats_sent: self.stats.returning_stats_sent,
            returning_stat_gaps: self.stats.returning_stat_gaps,
            player_history_appends: self.stats.player_history_appends,
            player_history_duplicates: self.stats.player_history_duplicates,
            player_history_errors: self.stats.player_history_errors,
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

    /// Bounded field observations for a live peer; recognition is not application.
    pub fn controller_field_diagnostics(
        &self,
        address: SocketAddrV4,
    ) -> Option<&ControllerFieldDiagnostics> {
        self.peers
            .get(&address)
            .map(|entry| &entry.controller_field_diagnostics)
    }

    /// Aggregate live-peer observations into at most 64 rows, without identities.
    pub fn controller_field_totals(&self) -> ControllerFieldDiagnostics {
        let mut totals = ControllerFieldDiagnostics::default();
        for peer in self.peers.values() {
            let diagnostics = &peer.controller_field_diagnostics;
            totals.overflow = totals.overflow.saturating_add(diagnostics.overflow);
            totals.unknown_subobject_blocks = totals
                .unknown_subobject_blocks
                .saturating_add(diagnostics.unknown_subobject_blocks);
            totals.malformed_bunches = totals
                .malformed_bunches
                .saturating_add(diagnostics.malformed_bunches);
            totals.body_parse_failures = totals
                .body_parse_failures
                .saturating_add(diagnostics.body_parse_failures);
            totals.block_payload_limit_rejections = totals
                .block_payload_limit_rejections
                .saturating_add(diagnostics.block_payload_limit_rejections);
            totals.block_payload_length_rejections = totals
                .block_payload_length_rejections
                .saturating_add(diagnostics.block_payload_length_rejections);
            totals.max_rejected_block_payload_bits = totals
                .max_rejected_block_payload_bits
                .max(diagnostics.max_rejected_block_payload_bits);
            for entry in &diagnostics.entries {
                totals.record_count(
                    entry.target,
                    entry.index,
                    entry.payload_bits,
                    entry.disposition,
                    entry.count,
                );
            }
        }
        totals
    }

    /// Count current transport queues and primary joined-player phases without
    /// retaining per-peer detail or allocating output proportional to traffic.
    pub fn readiness_counts(&self) -> ReadinessCounts {
        let mut counts = ReadinessCounts::default();
        let now = Instant::now();
        for entry in self.peers.values() {
            let transport = &mut counts.initialisation_transport;
            let request = &entry.initialisation_request_receipt;
            transport.requests_queued += usize::from(request.channel.is_some());
            transport.requests_settled += usize::from(request.settled);
            transport.requests_awaiting_ack +=
                usize::from(request.channel.is_some() && !request.settled);
            let completion = &entry.initialisation_completion_receipt;
            transport.completions_queued += usize::from(completion.channel.is_some());
            transport.completions_settled += usize::from(completion.settled);
            transport.completions_awaiting_ack +=
                usize::from(completion.channel.is_some() && !completion.settled);
            let friends = &mut counts.friends_query;
            let query = &entry.friends_query_receipt;
            friends.pending_admission += usize::from(entry.friends_query_pending);
            friends.queued += usize::from(query.channel.is_some());
            friends.settled += usize::from(query.settled);
            friends.awaiting_ack += usize::from(query.channel.is_some() && !query.settled);
            friends.awaiting_response += usize::from(
                query.settled && !entry.friends_query_pending && !entry.profiles.friend_ids_ready,
            );
            let records = &mut counts.connected_player_records;
            if let Some(record) = &entry.connected_player {
                records.registered += 1;
                records.finalised += usize::from(record.complete_time.is_some());
            }
            records.pending_send += usize::from(entry.connected_players_pending);
            let awaiting: usize = entry
                .connected_player_receipts
                .iter()
                .map(|pending| pending.records)
                .sum();
            records.creations_queued += entry.connected_player_creations_settled + awaiting;
            records.creations_settled += entry.connected_player_creations_settled;
            records.records_awaiting_ack += awaiting;
            let possession = &mut counts.controller_possession;
            let receipt = &entry.controller_possession_receipt;
            possession.pending_send += usize::from(entry.controller_possession_pending);
            possession.queued += usize::from(receipt.channel.is_some());
            possession.settled += usize::from(receipt.settled);
            possession.awaiting_ack += usize::from(receipt.channel.is_some() && !receipt.settled);
            let release = &mut counts.frozen_movement_release;
            let frozen = &entry.frozen_release;
            release.awaiting_delay += usize::from(frozen.awaiting_delay(now));
            release.pending_send += usize::from(frozen.pending);
            release.awaiting_ack += usize::from(!frozen.in_flight.is_empty());
            release.settled += usize::from(frozen.acked_at.is_some());
            let moves = &mut counts.pawn_move_acks;
            let state = &entry.pawn_moves;
            moves.sessions_moving += usize::from(state.counts.moves > 0);
            moves.awaiting_ack += usize::from(state.unacked);
            moves.deferred += usize::from(state.deferred);
            moves.positions_held += usize::from(state.location.is_some());
            let levels = &mut counts.level_visibility;
            levels.levels_visible += entry.visible_levels.len();
            levels.levels_with_actors += entry.level_actors.len();
            for slot in entry.level_actors.values().flatten() {
                match slot {
                    LevelActorSlot::Pending { .. } => levels.opens_pending += 1,
                    LevelActorSlot::Opened { settled: true, .. } => levels.opens_settled += 1,
                    LevelActorSlot::Opened { .. } => levels.opens_awaiting_ack += 1,
                    LevelActorSlot::Refused(_) => levels.opens_refused_waiting += 1,
                    LevelActorSlot::Unsent => {}
                }
            }
            if self.level_visibility.is_some() {
                let budget = self.level_channel_budget(entry);
                levels.channels_held += budget.held;
                levels.channel_budget_remaining += budget.remaining();
            }
            counts.controller_startup_actions_queued += entry.controller_startup_queued;
            counts.controller_startup_actions_settled += entry.controller_startup_settled;
            counts.controller_startup_actions_awaiting_ack +=
                entry.controller_startup_queued - entry.controller_startup_settled;
            if let Some(actions) = &self.controller_startup
                && !actions.is_empty()
                && !entry.controller_startup_failed
                && entry.controller_startup_queued == actions.len()
                && entry.controller_startup_settled == actions.len()
            {
                counts.controller_startup_sequences_settled += 1;
            }
            counts.controller_startup_sequences_failed +=
                usize::from(entry.controller_startup_failed);
            if let (Some(actions), Some(next)) =
                (&self.controller_startup, entry.controller_startup_next)
            {
                counts.controller_startup_actions_pending += actions.len() - next;
            } else if let Some(actions) = &self.controller_startup
                && entry.initial_opens.is_some()
                && !entry.controller_startup_failed
            {
                counts.controller_startup_actions_pending += actions.len();
            }
            let outcomes = &entry.initial_open_outcomes;
            let initial = &mut counts.initial_open_outcomes;
            initial.configured.add(&outcomes.configured);
            initial.committed.add(&outcomes.committed);
            initial.settled.add(&outcomes.settled);
            for receipt in &outcomes.awaiting_ack {
                initial.awaiting_ack.record(receipt.stage);
            }
            initial.failed.add(&outcomes.failed);
            initial.skipped.add(&outcomes.skipped);
            if let Some(pending) = &entry.initial_opens {
                for &stage in &pending.remaining {
                    initial.pending.record(stage);
                }
                if outcomes.deferred
                    && let Some(&stage) = pending.remaining.front()
                {
                    initial.deferred.record(stage);
                }
            } else if !outcomes.configured.is_empty() {
                if outcomes.failed.is_empty() {
                    initial.sequences_succeeded += 1;
                    initial.sequences_settled += usize::from(outcomes.awaiting_ack.is_empty());
                } else {
                    initial.sequences_failed += 1;
                }
            }
            counts.pending_control_replies += usize::from(entry.pending_control_reply.is_some());
            counts.pending_profile_exchanges +=
                usize::from(entry.pending_profile_exchange.is_some());
            if let Some(pending) = &entry.initial_opens {
                counts.pending_initial_opens += 1;
                counts.pending_initial_stages += pending.remaining.len();
            }
            counts.sent_packets_pending += entry.peer.sequencer().notify().sent_records().len();
            counts.control_reliable_pending += entry.peer.pending_reliable();
            counts.resend_bunches_pending += entry.peer.pending_resends();
            let (open, settled, pending) = entry.peer.actor_queue_counts();
            counts.actor_channels_open += open;
            counts.actor_channels_reliable_settled += settled;
            counts.actor_reliable_pending += pending;
            if entry.player.is_none() {
                continue;
            }
            if !entry.profiles.completed && !entry.profiles.finalised {
                counts.initialisation_blockers.requests_pending_queue += usize::from(
                    !entry.profiles.initialisation_requested
                        && matches!(
                            entry.pending_profile_exchange,
                            Some(PendingProfileExchange::Initialisation)
                        ),
                );
                if entry.profiles.initialisation_requested {
                    counts.initialisation_blockers.awaiting_loadout_response +=
                        usize::from(!entry.profiles.has_loadout);
                    counts.initialisation_blockers.awaiting_bestiary_response +=
                        usize::from(!entry.profiles.has_bestiary);
                }
            }
            if entry.profiles.finalised {
                counts.players_finalised += 1;
            } else if entry.profiles.completed {
                counts.players_awaiting_finalisation += 1;
            } else if entry.profiles.has_loadout && entry.profiles.has_bestiary {
                counts.players_awaiting_completion += 1;
            } else if entry.profiles.initialisation_requested
                || matches!(
                    entry.pending_profile_exchange,
                    Some(PendingProfileExchange::Initialisation)
                )
            {
                counts.players_awaiting_initialisation += 1;
            } else if entry.profiles.outstanding == Some(ProfileKind::Character)
                || matches!(
                    entry.pending_profile_exchange,
                    Some(
                        PendingProfileExchange::Request(ProfileKind::Character)
                            | PendingProfileExchange::Publish(ProfileKind::Character, _)
                    )
                )
            {
                counts.players_awaiting_character_profile += 1;
            } else if entry.character_slot.is_some() {
                counts.players_awaiting_user_profile += 1;
            } else if entry
                .initial_opens
                .as_ref()
                .is_some_and(|pending| pending.begin_slot.is_some())
            {
                counts.players_begin_waiting_for_player_state += 1;
            } else {
                counts.players_awaiting_begin += 1;
            }
        }
        counts
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

    fn expire_stalled_delivery(&mut self, now: Instant) {
        let timeout = self.config.idle_timeout;
        let stalled: Vec<_> = self
            .peers
            .iter_mut()
            .filter_map(|(address, entry)| {
                if entry.peer.sequencer().notify().sent_records().is_empty() {
                    entry.delivery_progress_at = None;
                    return None;
                }
                let progress = entry.delivery_progress_at.get_or_insert(now);
                (now.saturating_duration_since(*progress) > timeout).then_some(*address)
            })
            .collect();
        for address in stalled {
            if self.remove_peer(address).is_some() {
                bump(&mut self.stats.peers_delivery_stalled);
            }
        }
    }

    /// Native policy (N-019): a peer whose friends query stays unanswered past
    /// [`PeerLabConfig::idle_timeout`] is removed, however much it ACKs, so its
    /// missing response cannot hold every player's finalisation (F-973).
    fn expire_friends_queries(&mut self, now: Instant) {
        let timeout = self.config.idle_timeout;
        let expired: Vec<_> = self
            .peers
            .iter()
            .filter_map(|(address, entry)| {
                entry
                    .friends_query_since
                    .is_some_and(|since| now.saturating_duration_since(since) > timeout)
                    .then_some(*address)
            })
            .collect();
        for address in expired {
            if self.remove_peer(address).is_some() {
                bump(&mut self.stats.peers_friends_query_expired);
            }
        }
    }

    /// Native policy (F-1116): with records and possession properties on, a
    /// session whose possession update was abandoned can never finalise its
    /// record on the client (F-1087), and the record is never sent to it.
    /// Abandonment is final (the update is never retried), so waiting cannot
    /// help: the session is closed (a best-effort control close, as for an
    /// actor channel failure) and removed in the same poll rather than hold
    /// its slot and loading screen while it keeps ACKing. Removal also stops
    /// friends queries to it, so it cannot hold others' finalisation (F-973).
    /// A failed transport is removed where it fails (`peers_failed`).
    fn close_abandoned_possessions(&mut self) {
        if !std::mem::take(&mut self.possession_abandoned) || self.connected_players.is_none() {
            return;
        }
        let abandoned: Vec<_> = self
            .peers
            .iter()
            .filter_map(|(address, entry)| entry.controller_possession_failed.then_some(*address))
            .collect();
        for address in abandoned {
            let Some(entry) = self.peers.get_mut(&address) else {
                continue;
            };
            if entry.peer.has_received_client_packet() {
                match entry.peer.reliable_control_close(CLOSE_REASON_DESTROYED) {
                    Ok(datagram) => {
                        self.send(&datagram, address);
                    }
                    Err(_) => bump(&mut self.stats.reply_errors),
                }
            }
            if self.remove_peer(address).is_some() {
                bump(&mut self.stats.peers_closed_by_possession_abandoned);
            }
        }
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
                        connected_player: None,
                        connected_players_sent: Vec::new(),
                        connected_players_pending: false,
                        connected_players_retry_ready: false,
                        connected_player_receipts: Vec::new(),
                        connected_player_creations_settled: 0,
                        controller_open_guids: None,
                        controller_possession_pending: false,
                        controller_possession_retry_ready: false,
                        controller_possession_receipt: InitialisationReceipt::default(),
                        controller_possession_failed: false,
                        record_prerequisites: RecordPrerequisites::default(),
                        frozen_release: FrozenMovementRelease::default(),
                        pawn_inbound: PawnInboundCounts::default(),
                        pawn_moves: PawnMoves::default(),
                        placement: None,
                        visible_levels: BTreeSet::new(),
                        level_actors: BTreeMap::new(),
                        level_actor_queue: VecDeque::new(),
                        level_actors_retry_ready: false,
                        level_actors_refused: false,
                        level_channels_released: false,
                        friends_query_pending: false,
                        friends_query_retry_ready: false,
                        friends_query_receipt: InitialisationReceipt::default(),
                        friends_query_since: None,
                        controller_field_diagnostics: ControllerFieldDiagnostics::default(),
                        auth_request: None,
                        auth_identity: None,
                        auth_accepted: false,
                        auth_ticket_digest: None,
                        pawn_character_state: None,
                        pawn_temperature: None,
                        pawn_temperature_tick: None,
                        queued_pawn_temperature: None,
                        peer,
                        pawn_uid: None,
                        controller_startup_pawn_guid: None,
                        controller_startup_next: None,
                        controller_startup_queued: 0,
                        controller_startup_settled: 0,
                        initialisation_request_receipt: InitialisationReceipt::default(),
                        initialisation_completion_receipt: InitialisationReceipt::default(),
                        controller_startup_failed: false,
                        controller_startup_retry_ready: false,
                        pending_pawn_uid: None,
                        restored_character: None,
                        restored_modifiers: Vec::new(),
                        restored_modifier_ticks: BTreeMap::new(),
                        queued_modifier_times: BTreeMap::new(),
                        pawn_property_retry_ready: false,
                        pending_pawn_restore: None,
                        pawn_restore_retry_ready: false,
                        cookie,
                        last_seen: now,
                        delivery_progress_at: None,
                        ack_pending: false,
                        ack_capacity_blocked: false,
                        pending_control_reply: None,
                        control_reply_retry_ready: false,
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
                        initial_opens: None,
                        initial_open_outcomes: InitialOpenOutcomes::default(),
                        player_history_channel: None,
                        player_history_pending: false,
                        player_history_retry_ready: false,
                        character_slot: None,
                        backend_proxy: None,
                        player_data: None,
                        bestiary: None,
                        profiles: ProfileExchange::default(),
                        pending_profile_exchange: None,
                        profile_exchange_retry_ready: false,
                        opened_actors: BTreeMap::new(),
                        opened_incarnations: BTreeMap::new(),
                        retiring_incarnations: BTreeMap::new(),
                        closing_legacy_actors: BTreeMap::new(),
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
        if self.authenticator.is_some()
            && !self
                .peers
                .get(&address)
                .is_some_and(|peer| peer.auth_accepted)
        {
            return;
        }
        let Some(entry) = self.peers.get_mut(&address) else {
            bump(&mut self.stats.unknown_peer);
            return;
        };
        let mut close = false;
        let mut incarnation_released = false;
        let mut staged_calls = Vec::new();
        let mut capture = self.private_rpc82_capture.as_mut().and_then(|sink| {
            Some(super::selected_rpc_capture::CaptureTap {
                sink,
                channel: entry.player_controller_channel?,
                connection_ordinal: entry.connection_id?,
                elapsed_ms: now
                    .saturating_duration_since(self.started)
                    .as_millis()
                    .min(u128::from(u64::MAX)) as u64,
                engine_net_version: self.player_controller_open.as_ref()?.engine_net_version,
                order: entry.peer.connection_byte_order(),
                components: [entry.backend_proxy, entry.player_data, entry.bestiary],
            })
        });
        // Only the joining player's own open pawn channel, as counted below.
        let mut pawn_capture = self.private_pawn_bunch_capture.as_mut().and_then(|sink| {
            entry.controller_startup_pawn_guid?;
            Some(super::selected_rpc_capture::pawn_bunches::PawnBunchTap {
                sink,
                channel: self.pawn_open.as_ref()?.1,
                connection_ordinal: entry.connection_id?,
                elapsed_ms: now
                    .saturating_duration_since(self.started)
                    .as_millis()
                    .min(u128::from(u64::MAX)) as u64,
                unix_ms: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |time| time.as_millis().min(u128::from(u64::MAX)) as u64),
            })
        });
        let messages = match entry.peer.receive_with_capture(
            &self.buffer[..length],
            capture.as_mut(),
            pawn_capture.as_mut(),
        ) {
            Ok(received) => {
                let summary = received.summary();
                match summary.disposition {
                    PacketDisposition::Accepted { .. } => {
                        // Observe before inbound calls or retries can enqueue
                        // more traffic. NAKs and close ACKs cannot settle opens.
                        if !summary.acked.is_empty() {
                            entry.initialisation_request_receipt.observe_positive_ack(
                                &entry.peer,
                                &summary.released_actor_channels,
                            );
                            entry
                                .initialisation_completion_receipt
                                .observe_positive_ack(
                                    &entry.peer,
                                    &summary.released_actor_channels,
                                );
                            entry.initial_open_outcomes.observe_positive_ack(
                                &entry.peer,
                                &summary.released_actor_channels,
                            );
                            let receipt = &mut entry.friends_query_receipt;
                            let settled = receipt.settled;
                            receipt.observe_positive_ack(
                                &entry.peer,
                                &summary.released_actor_channels,
                            );
                            if receipt.settled && !settled {
                                bump(&mut self.stats.friends_list_queries_acknowledged);
                            }
                            let receipt = &mut entry.controller_possession_receipt;
                            let settled = receipt.settled;
                            receipt.observe_positive_ack(
                                &entry.peer,
                                &summary.released_actor_channels,
                            );
                            if receipt.settled && !settled {
                                bump(&mut self.stats.controller_possession_updates_acked);
                            }
                            // The own record's prerequisites (F-1116) just
                            // settled: a finalised record may go out now.
                            let prerequisites = &mut entry.record_prerequisites;
                            let held = !prerequisites.settled();
                            prerequisites.controller.observe_positive_ack(
                                &entry.peer,
                                &summary.released_actor_channels,
                            );
                            prerequisites.pawn.observe_positive_ack(
                                &entry.peer,
                                &summary.released_actor_channels,
                            );
                            if held
                                && prerequisites.settled()
                                && self.connected_players.is_some()
                                && entry
                                    .connected_player
                                    .as_ref()
                                    .is_some_and(|record| record.complete_time.is_some())
                            {
                                entry.connected_players_pending = true;
                                entry.connected_players_retry_ready = true;
                            }
                            // Each record creation settles once, on a positive
                            // ACK of its own bunch; NAKs leave repair to the
                            // reliable layer and settle nothing.
                            let mut settled_records = 0;
                            let mut own_settled = false;
                            entry.connected_player_receipts.retain_mut(|pending| {
                                pending.receipt.observe_positive_ack(
                                    &entry.peer,
                                    &summary.released_actor_channels,
                                );
                                if pending.receipt.settled {
                                    settled_records += pending.records;
                                    own_settled |= pending.own;
                                }
                                !pending.receipt.settled
                            });
                            // The player's own record creation settled: its
                            // frozen-movement release is scheduled, once.
                            if own_settled
                                && let Some(delay) = self.frozen_movement_release
                                && entry.frozen_release.record_acked_at.is_none()
                            {
                                entry.frozen_release.record_acked_at = Some(now);
                                entry.frozen_release.due_at = Some(now + delay);
                                bump(&mut self.stats.frozen_movement_releases_scheduled);
                            }
                            entry.connected_player_creations_settled += settled_records;
                            self.stats.connected_player_record_creations_acked = self
                                .stats
                                .connected_player_record_creations_acked
                                .saturating_add(settled_records as u64);
                            // Each level actor open settles once, when a
                            // positive ACK leaves its own channel reliably
                            // empty; NAKs are repaired by the reliable layer.
                            for slot in entry.level_actors.values_mut().flatten() {
                                if let LevelActorSlot::Opened {
                                    channel,
                                    settled,
                                    abandoned,
                                } = slot
                                    && !*settled
                                {
                                    *abandoned |= summary.released_actor_channels.contains(channel);
                                    if !*abandoned && entry.peer.actor_channel_settled(*channel) {
                                        *settled = true;
                                        bump(&mut self.stats.level_actor_opens_acked);
                                    }
                                }
                            }
                        }
                        // Retain the receipt before subsequent calls can queue
                        // unrelated controller traffic. NAKs retain reliable
                        // out-bunches; closed/missing channels never settle.
                        if !summary.acked.is_empty()
                            && entry.controller_startup_queued > entry.controller_startup_settled
                            && entry
                                .player_controller_channel
                                .is_some_and(|channel| entry.peer.actor_channel_settled(channel))
                        {
                            entry.controller_startup_settled = entry.controller_startup_queued;
                        }
                        let retiring = entry.retiring_incarnations.len();
                        entry.retiring_incarnations.retain(|_, (_, channel)| {
                            !summary.released_actor_channels.contains(channel)
                        });
                        incarnation_released = entry.retiring_incarnations.len() != retiring;
                        for channel in &summary.released_actor_channels {
                            entry.closing_legacy_actors.remove(channel);
                        }
                        // A released actor channel (a replication close's
                        // ACK) may make room for channel-refused level opens:
                        // the next admission pass checks the budget once.
                        // The cheap trigger, never a per-poll scan.
                        if !summary.released_actor_channels.is_empty() && entry.level_actors_refused
                        {
                            entry.level_channels_released = true;
                            entry.level_actors_retry_ready = true;
                        }
                        entry.last_seen = now;
                        entry.ack_pending = true;
                        if !summary.acked.is_empty() || !summary.naked.is_empty() {
                            entry.delivery_progress_at = Some(now);
                            if entry.controller_startup_next.is_some() {
                                entry.controller_startup_retry_ready = true;
                            }
                            entry.ack_capacity_blocked = false;
                            if entry.pending_control_reply.is_some() {
                                entry.control_reply_retry_ready = true;
                            }
                            if let Some(pending) = &mut entry.initial_opens {
                                pending.retry_ready = true;
                            }
                            if entry.friends_query_pending {
                                entry.friends_query_retry_ready = true;
                            }
                            if entry.connected_players_pending {
                                entry.connected_players_retry_ready = true;
                            }
                            if entry.controller_possession_pending {
                                entry.controller_possession_retry_ready = true;
                            }
                            entry.frozen_release.deferred = false;
                            entry.pawn_moves.deferred = false;
                            if !entry.level_actor_queue.is_empty() {
                                entry.level_actors_retry_ready = true;
                            }
                            if entry.pending_profile_exchange.is_some() {
                                entry.profile_exchange_retry_ready = true;
                            }
                        }
                        if entry.pending_pawn_restore.is_some()
                            && (!summary.acked.is_empty() || !summary.naked.is_empty())
                        {
                            entry.pawn_restore_retry_ready = true;
                        }
                        if (entry.pawn_temperature.is_some()
                            || !entry.restored_modifiers.is_empty())
                            && (!summary.acked.is_empty() || !summary.naked.is_empty())
                        {
                            entry.pawn_property_retry_ready = true;
                        }
                        if entry.player_history_pending
                            && (!summary.acked.is_empty() || !summary.naked.is_empty())
                        {
                            entry.player_history_retry_ready = true;
                        }
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
                            // The unreliable frozen-movement release: a NAK
                            // makes it due again, an ACK of its copy settles.
                            match entry.frozen_release.observe(packet_id, acked) {
                                Some(true) => {
                                    entry.frozen_release.acked_at = Some(now);
                                    bump(&mut self.stats.frozen_movement_releases_acked);
                                }
                                Some(false) => entry.frozen_release.deferred = false,
                                None => {}
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
                        self.stats.actor_partial_parts = self
                            .stats
                            .actor_partial_parts
                            .saturating_add(summary.actor_partial_parts as u64);
                        self.stats.actor_partial_completed = self
                            .stats
                            .actor_partial_completed
                            .saturating_add(summary.actor_partial_completed as u64);
                        self.stats.actor_partial_rejected = self
                            .stats
                            .actor_partial_rejected
                            .saturating_add(summary.actor_partial_rejected as u64);
                        self.stats.actor_duplicate_reliable = self
                            .stats
                            .actor_duplicate_reliable
                            .saturating_add(summary.actor_duplicate_reliable as u64);
                        // Pawn-channel traffic per connection, by header
                        // only; the bunches' handling is unchanged.
                        if let Some((plan, pawn_channel, _)) = &self.pawn_open
                            && entry.controller_startup_pawn_guid.is_some()
                        {
                            let on_pawn = |bunch: &&super::packet::Bunch| {
                                bunch.header.channel_index == *pawn_channel
                            };
                            let reliable =
                                received.actor_bunches().iter().filter(on_pawn).count() as u64;
                            let unreliable = received
                                .unreliable_actor_bunches()
                                .iter()
                                .filter(on_pawn)
                                .count() as u64;
                            if reliable + unreliable > 0 {
                                let counts = &mut entry.pawn_inbound;
                                counts.reliable_bunches =
                                    counts.reliable_bunches.saturating_add(reliable);
                                counts.unreliable_bunches =
                                    counts.unreliable_bunches.saturating_add(unreliable);
                                counts.last_bunch_ms = Some(
                                    now.saturating_duration_since(self.started)
                                        .as_millis()
                                        .min(u128::from(u64::MAX))
                                        as u64,
                                );
                                self.stats.pawn_bunches_received_reliable = self
                                    .stats
                                    .pawn_bunches_received_reliable
                                    .saturating_add(reliable);
                                self.stats.pawn_bunches_received_unreliable = self
                                    .stats
                                    .pawn_bunches_received_unreliable
                                    .saturating_add(unreliable);
                            }
                            // The client's packed moves (F-1127): decoded on
                            // arrival, acknowledged from the poll; the bunches
                            // stay dropped otherwise.
                            if self.pawn_move_acks && unreliable > 0 {
                                let codec = RepLayoutCodec {
                                    actor: actor_codec(
                                        plan.engine_net_version,
                                        entry.peer.connection_byte_order(),
                                    ),
                                    checksums: false,
                                    limits: Default::default(),
                                };
                                for bunch in
                                    received.unreliable_actor_bunches().iter().filter(on_pawn)
                                {
                                    receive_pawn_moves(
                                        &mut entry.pawn_moves,
                                        &mut self.stats,
                                        bunch,
                                        &codec,
                                        now,
                                        entry.connection_id,
                                    );
                                }
                            }
                        }
                        // Header-only counts; these bunches stay dropped.
                        if self.count_unreliable_actor_bunches
                            && !received.unreliable_actor_bunches().is_empty()
                        {
                            let controller_codec =
                                self.player_controller_open.as_ref().map(|plan| {
                                    actor_codec(
                                        plan.engine_net_version,
                                        entry.peer.connection_byte_order(),
                                    )
                                });
                            let pawn_channel =
                                self.pawn_open.as_ref().map(|(_, channel, _)| *channel);
                            let game_state_channel = self
                                .game_state_open
                                .as_ref()
                                .map(|session| session.channel_index);
                            for bunch in received.unreliable_actor_bunches() {
                                let channel = Some(bunch.header.channel_index);
                                let kind = if channel == entry.player_controller_channel {
                                    UnreliableChannelKind::Controller
                                } else if channel == pawn_channel
                                    && entry.controller_startup_pawn_guid.is_some()
                                {
                                    UnreliableChannelKind::Pawn
                                } else if channel == entry.player_state_channel {
                                    UnreliableChannelKind::PlayerState
                                } else if channel == game_state_channel
                                    && entry.game_state.is_some()
                                {
                                    UnreliableChannelKind::GameState
                                } else {
                                    UnreliableChannelKind::Other
                                };
                                let controller = controller_codec
                                    .as_ref()
                                    .filter(|_| kind == UnreliableChannelKind::Controller);
                                self.unreliable_actor_bunches
                                    .record(kind, bunch, controller);
                            }
                        }
                        for bunch in received.actor_bunches() {
                            let Some(plan) = &self.player_controller_open else {
                                bump(&mut self.stats.actor_bunches_ignored);
                                continue;
                            };
                            if entry.player_controller_channel != Some(bunch.header.channel_index) {
                                bump(&mut self.stats.actor_bunches_ignored);
                                continue;
                            }
                            // The qualified decoder reads little-endian
                            // archives only (F-1021); a big-endian connection
                            // keeps field 82 ignored.
                            let order = entry.peer.connection_byte_order();
                            let level_visibility = self
                                .level_visibility
                                .as_ref()
                                .filter(|_| matches!(order, super::archive::ByteOrder::Little))
                                .map(|config| &config.profile);
                            let parsed = controller_calls(
                                &bunch.payload,
                                plan.engine_net_version,
                                order,
                                [entry.backend_proxy, entry.player_data, entry.bestiary],
                                level_visibility,
                                &mut entry.controller_field_diagnostics,
                            );
                            let StagedControllerCalls {
                                calls,
                                ignored,
                                malformed_level_reports,
                            } = match parsed {
                                Ok(parsed) => parsed,
                                Err(error) => {
                                    if let Some(reason) =
                                        error.downcast_ref::<FriendsListRejection>()
                                    {
                                        self.stats.friends_list_rejections.record(*reason);
                                    }
                                    entry.controller_field_diagnostics.malformed_bunches = entry
                                        .controller_field_diagnostics
                                        .malformed_bunches
                                        .saturating_add(1);
                                    bump(&mut self.stats.actor_bunches_malformed);
                                    continue;
                                }
                            };
                            self.stats.controller_fields_ignored = self
                                .stats
                                .controller_fields_ignored
                                .saturating_add(ignored as u64);
                            self.stats.level_visibility_reports_malformed = self
                                .stats
                                .level_visibility_reports_malformed
                                .saturating_add(malformed_level_reports as u64);
                            staged_calls.extend(calls);
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
        if incarnation_released {
            self.prune_incarnation_guids();
        }
        for call in staged_calls {
            self.controller_call(address, call);
        }
        self.answer(address, &messages);
        // Decoded messages are dropped here; nothing of them is retained.
        if close && self.remove_peer(address).is_some() {
            bump(&mut self.stats.peers_closed_by_client);
        }
    }

    fn fail_profile_exchange(&mut self, address: SocketAddrV4) {
        if self.remove_peer(address).is_some() {
            bump(&mut self.stats.profile_players_failed);
            bump(&mut self.stats.peers_failed);
        }
    }

    fn defer_or_fail_profile_exchange(
        &mut self,
        address: SocketAddrV4,
        error: &anyhow::Error,
        pending: PendingProfileExchange,
    ) {
        bump(&mut self.stats.profile_exchange_errors);
        if let Some(entry) = self.peers.get_mut(&address) {
            let channel = match &pending {
                PendingProfileExchange::Request(_) | PendingProfileExchange::Initialisation => {
                    entry.player_controller_channel
                }
                PendingProfileExchange::Publish(..) | PendingProfileExchange::Complete => {
                    entry.player_state_channel
                }
            };
            if channel.is_some_and(|channel| transport_capacity_can_drain(entry, error, channel)) {
                if let PendingProfileExchange::Publish(kind, _) = &pending {
                    bump(match kind {
                        ProfileKind::User => &mut self.stats.profile_user_publications_deferred,
                        ProfileKind::Character => {
                            &mut self.stats.profile_character_publications_deferred
                        }
                    });
                }
                entry.pending_profile_exchange = Some(pending);
                entry.profile_exchange_retry_ready = false;
                return;
            }
        }
        self.fail_profile_exchange(address);
    }

    fn retry_pending_profile_exchanges(&mut self) {
        let ready: Vec<_> = self
            .peers
            .iter_mut()
            .filter_map(|(address, entry)| {
                if !std::mem::take(&mut entry.profile_exchange_retry_ready) {
                    return None;
                }
                entry
                    .pending_profile_exchange
                    .take()
                    .map(|pending| (*address, pending))
            })
            .collect();
        for (address, pending) in ready {
            match pending {
                PendingProfileExchange::Request(kind) => self.request_profile(address, kind),
                PendingProfileExchange::Publish(kind, values) => {
                    self.accept_profile(address, kind, values);
                }
                PendingProfileExchange::Initialisation => self.request_initialisation(address),
                PendingProfileExchange::Complete => self.complete_player_initialisation(address),
            }
        }
    }

    /// Matched AddConnectedPlayer adds a player, then queries every registered
    /// player's controller (F-1051). Registration follows Begin, so the native
    /// registered set is the peers with a recorded character slot. Coalesce
    /// unsent queries; reliable transport repairs admitted requests. A
    /// re-query keeps an unanswered query's deadline (N-019).
    fn query_connected_friends(&mut self) {
        let now = Instant::now();
        for entry in self.peers.values_mut() {
            if entry.character_slot.is_some() {
                entry.friends_query_pending = true;
                entry.friends_query_retry_ready = true;
                entry.friends_query_since.get_or_insert(now);
            }
        }
        self.retry_friends_queries();
    }

    fn retry_friends_queries(&mut self) {
        let ready: Vec<_> = self
            .peers
            .iter_mut()
            .filter_map(|(address, entry)| {
                (entry.friends_query_pending
                    && std::mem::take(&mut entry.friends_query_retry_ready))
                .then_some(*address)
            })
            .collect();
        for address in ready {
            let sent = (|| {
                let plan = self
                    .player_controller_open
                    .as_ref()
                    .context("no controller plan")?;
                let entry = self
                    .peers
                    .get_mut(&address)
                    .context("no friends-query peer")?;
                let channel = entry
                    .player_controller_channel
                    .context("no controller channel")?;
                let codec = RepLayoutCodec {
                    actor: actor_codec(plan.engine_net_version, entry.peer.connection_byte_order()),
                    checksums: false,
                    limits: Default::default(),
                };
                let mut writer = super::packet::BitWriter::new();
                write_field_header(
                    &mut writer,
                    &FieldHeader {
                        index: 118,
                        payload: encode_rpc_payload(&[], &[], &codec)?,
                    },
                    self.controller_startup_bound,
                )?;
                let payload = encode_actor_bunch_body(
                    &ActorBunchBody {
                        spawn: None,
                        blocks: vec![ContentBlock {
                            has_rep_layout: false,
                            target: BlockTarget::Actor,
                            payload: Payload::from_writer(writer),
                        }],
                    },
                    &codec.actor,
                )?;
                let previous = entry
                    .peer
                    .actor_reliable_sequence(channel)
                    .context("controller channel missing")?;
                let datagram = entry.peer.reliable_actor_bunch(channel, payload)?;
                entry.friends_query_receipt =
                    InitialisationReceipt::committed(&entry.peer, channel, previous);
                entry.friends_query_pending = false;
                entry.profiles.friend_ids_ready = false;
                bump(&mut self.stats.friends_list_queries_queued);
                Ok::<_, anyhow::Error>(datagram)
            })();
            match sent {
                Ok(datagram) => {
                    self.send(&datagram, address);
                }
                Err(error) => {
                    let drainable = self.peers.get(&address).is_some_and(|entry| {
                        entry.player_controller_channel.is_some_and(|channel| {
                            transport_capacity_can_drain(entry, &error, channel)
                        })
                    });
                    if !drainable {
                        self.fail_profile_exchange(address);
                    }
                }
            }
        }
    }

    fn request_profile(&mut self, address: SocketAddrV4, kind: ProfileKind) {
        let sent = (|| {
            let plan = self
                .player_controller_open
                .as_ref()
                .context("no controller plan")?;
            let entry = self.peers.get_mut(&address).context("no profile peer")?;
            let codec = RepLayoutCodec {
                actor: actor_codec(plan.engine_net_version, entry.peer.connection_byte_order()),
                checksums: false,
                limits: Default::default(),
            };
            let payload = profile_exchange::request(
                entry.backend_proxy.context("no BackendProxy")?,
                kind,
                entry.character_slot.context("no character slot")?,
                &codec,
            )?;
            let datagram = entry.peer.reliable_actor_bunch(
                entry
                    .player_controller_channel
                    .context("no controller channel")?,
                payload,
            )?;
            entry.profiles.outstanding = Some(kind);
            entry.ack_pending = false;
            Ok::<_, anyhow::Error>(datagram)
        })();
        match sent {
            Ok(datagram) => {
                if self.send(&datagram, address) {
                    bump(match kind {
                        ProfileKind::User => &mut self.stats.profile_user_requests_sent,
                        ProfileKind::Character => &mut self.stats.profile_character_requests_sent,
                    });
                }
            }
            Err(error) => {
                self.defer_or_fail_profile_exchange(
                    address,
                    &error,
                    PendingProfileExchange::Request(kind),
                );
            }
        }
    }

    fn controller_call(&mut self, address: SocketAddrV4, call: ControllerCall) {
        let Some(entry) = self.peers.get_mut(&address) else {
            return;
        };
        match call {
            // The bounded field diagnostics retain recognition. Timeout policy
            // and the original callback's effects are not qualified here.
            ControllerCall::ShortTimeout => {}
            ControllerCall::LevelVisibility(parameter) => {
                self.level_visibility_report(address, parameter);
            }
            ControllerCall::Begin(slot) => {
                if slot == -1
                    || entry.character_slot.is_some()
                    || entry.player.is_none()
                    || entry
                        .initial_opens
                        .as_ref()
                        .is_some_and(|pending| pending.begin_slot.is_some())
                {
                    bump(&mut self.stats.initialisation_calls_ignored);
                    return;
                }
                if entry
                    .player_state_channel
                    .is_none_or(|channel| !entry.peer.has_actor_channel(channel))
                {
                    if let Some(pending) = &mut entry.initial_opens
                        && pending
                            .remaining
                            .iter()
                            .any(|stage| matches!(stage, InitialOpenStage::PlayerState))
                    {
                        pending.begin_slot = Some(slot);
                    } else {
                        bump(&mut self.stats.initialisation_calls_ignored);
                    }
                    return;
                }
                entry.character_slot = Some(slot);
                bump(&mut self.stats.initialisation_slots_recorded);
                self.register_connected_player(address);
                self.query_connected_friends();
                self.request_profile(address, ProfileKind::User);
            }
            ControllerCall::FriendsList => {
                if entry.player.is_none() || entry.profiles.friend_ids_ready {
                    bump(&mut self.stats.initialisation_calls_ignored);
                    bump(if entry.player.is_none() {
                        &mut self.stats.friends_lists_ignored_unregistered
                    } else {
                        &mut self.stats.friends_lists_ignored_already_ready
                    });
                    return;
                }
                entry.profiles.friend_ids_ready = true;
                // An unsent pending query will clear readiness again, so
                // only an answer with no query left to send ends the deadline.
                // With one still to send, this answer restarts the clock: the
                // next query must not inherit time its predecessor used up.
                entry.friends_query_since = entry.friends_query_pending.then(Instant::now);
                bump(&mut self.stats.friends_lists_received);
            }
            ControllerCall::Profile(response) => {
                bump(match response.kind {
                    ProfileKind::User => &mut self.stats.profile_user_responses_recognized,
                    ProfileKind::Character => {
                        &mut self.stats.profile_character_responses_recognized
                    }
                });
                if entry.profiles.outstanding != Some(response.kind) {
                    bump(&mut self.stats.profile_responses_ignored);
                    return;
                }
                if entry
                    .player_state_channel
                    .is_none_or(|channel| !entry.peer.has_actor_channel(channel))
                {
                    // Original reason 0x27; its error-text wire is unknown.
                    self.fail_profile_exchange(address);
                    return;
                }
                entry.profiles.outstanding = None;
                if !response.success {
                    let (failures, counter) = match response.kind {
                        ProfileKind::User => (
                            &mut entry.profiles.user_failures,
                            &mut self.stats.profile_user_failures,
                        ),
                        ProfileKind::Character => (
                            &mut entry.profiles.character_failures,
                            &mut self.stats.profile_character_failures,
                        ),
                    };
                    *failures += 1;
                    bump(counter);
                    if *failures >= 3 {
                        // Original reasons 0x30 (user) and 0x25 (character).
                        self.fail_profile_exchange(address);
                    } else {
                        self.request_profile(address, response.kind);
                    }
                    return;
                }
                self.accept_profile(address, response.kind, response.values);
            }
            ControllerCall::Initialisation(answer) => {
                let (received, answered) = match answer {
                    InitialisationAnswer::Loadout => (
                        &mut self.stats.loadout_responses_received,
                        &mut entry.profiles.has_loadout,
                    ),
                    InitialisationAnswer::Bestiary => (
                        &mut self.stats.bestiary_responses_received,
                        &mut entry.profiles.has_bestiary,
                    ),
                };
                bump(received);
                if !entry.profiles.initialisation_requested || *answered || entry.profiles.completed
                {
                    bump(&mut self.stats.initialisation_answers_ignored);
                    return;
                }
                *answered = true;
                self.complete_player_initialisation(address);
            }
        }
    }

    fn accept_profile(&mut self, address: SocketAddrV4, kind: ProfileKind, values: Vec<RepValue>) {
        let sent = (|| {
            let plan = self
                .player_state_open
                .as_ref()
                .context("no PlayerState plan")?;
            let entry = self.peers.get_mut(&address).context("no profile peer")?;
            let codec = RepLayoutCodec {
                actor: actor_codec(plan.engine_net_version, entry.peer.connection_byte_order()),
                checksums: false,
                limits: Default::default(),
            };
            let (first, count) = match kind {
                ProfileKind::User => (27, 6),
                ProfileKind::Character => (33, 44),
            };
            ensure!(values.len() == count, "profile command count");
            // Validate caller-visible state before committing reliable traffic.
            let character = if kind == ProfileKind::Character {
                let slot = profile_exchange::character_slot(&values)?;
                let xp = profile_exchange::character_xp(&values)?;
                let Some(RepValue::Str(name)) = values.first() else {
                    bail!("character profile needs CharacterName");
                };
                Some((slot, xp, name.clone()))
            } else {
                None
            };
            let payload =
                profile_exchange::player_state_update(first, &values, &plan.layout, &codec)?;
            let sent = entry.peer.reliable_actor_bunch_datagrams(
                entry
                    .player_state_channel
                    .context("no PlayerState channel")?,
                payload,
            )?;
            if let Some((slot, xp, name)) = character {
                entry.profiles.character_slot = Some(slot);
                entry.profiles.character_xp = Some(xp);
                entry.profiles.character_name = Some(name);
            }
            entry.ack_pending = false;
            Ok::<_, anyhow::Error>(sent)
        })();
        match sent {
            Ok(sent) => {
                bump(match kind {
                    ProfileKind::User => &mut self.stats.profile_user_responses_received,
                    ProfileKind::Character => &mut self.stats.profile_character_responses_received,
                });
                // The reliable parts are queued for ACK/NAK repair, so a failed
                // send only affects the counter, never the exchange's progress.
                if self.send_all(&sent, address) {
                    bump(&mut self.stats.profile_updates_sent);
                }
                match kind {
                    ProfileKind::User => self.request_profile(address, ProfileKind::Character),
                    ProfileKind::Character => self.request_initialisation(address),
                }
            }
            Err(error) => {
                self.defer_or_fail_profile_exchange(
                    address,
                    &error,
                    PendingProfileExchange::Publish(kind, values),
                );
            }
        }
    }

    fn request_initialisation(&mut self, address: SocketAddrV4) {
        let sent = (|| {
            let plan = self
                .player_controller_open
                .as_ref()
                .context("no controller plan")?;
            let session = self.game_state_open.as_ref();
            // Use the same complete current values as the GameState, filling
            // unsent defaults. Template-only sessions may also carry a prospect.
            let prospect = (51..=67)
                .map(|handle| {
                    session
                        .and_then(|session| {
                            session
                                .active_prospect
                                .as_ref()
                                .unwrap_or(&session.state.values)
                                .get(&handle)
                        })
                        .cloned()
                        .or_else(|| class_default(handle))
                        .context("missing prospect field")
                })
                .collect::<Result<Vec<_>>>()?;
            let entry = self
                .peers
                .get_mut(&address)
                .context("no initialisation peer")?;
            let codec = RepLayoutCodec {
                actor: actor_codec(plan.engine_net_version, entry.peer.connection_byte_order()),
                checksums: false,
                limits: Default::default(),
            };
            let payload = profile_exchange::initialisation_requests(
                entry.player_data.context("no PlayerDataComponent")?,
                entry.bestiary.context("no BestiaryManagerComponent")?,
                entry.profiles.character_slot.context("no profile slot")?,
                &prospect,
                &codec,
            )?;
            let channel = entry
                .player_controller_channel
                .context("no controller channel")?;
            let previous = entry
                .peer
                .actor_reliable_sequence(channel)
                .context("controller channel missing")?;
            let sent = entry
                .peer
                .reliable_actor_bunch_datagrams(channel, payload)?;
            entry.profiles.initialisation_requested = true;
            entry.initialisation_request_receipt =
                InitialisationReceipt::committed(&entry.peer, channel, previous);
            entry.ack_pending = false;
            Ok::<_, anyhow::Error>(sent)
        })();
        match sent {
            Ok(sent) => {
                if self.send_all(&sent, address) {
                    bump(&mut self.stats.loadout_requests_sent);
                    bump(&mut self.stats.bestiary_requests_sent);
                    bump(&mut self.stats.associated_prospects_sent);
                }
            }
            Err(error) => {
                self.defer_or_fail_profile_exchange(
                    address,
                    &error,
                    PendingProfileExchange::Initialisation,
                );
            }
        }
    }

    fn complete_player_initialisation(&mut self, address: SocketAddrV4) {
        let Some(entry) = self.peers.get(&address) else {
            return;
        };
        if entry.profiles.completed
            || entry.pending_profile_exchange.is_some()
            || !entry.profiles.has_loadout
            || !entry.profiles.has_bestiary
            || self
                .pawn_open
                .as_ref()
                .is_none_or(|(_, channel, _)| !entry.peer.has_actor_channel(*channel))
        {
            return;
        }
        let sent = (|| {
            let plan = self
                .player_state_open
                .as_ref()
                .context("no PlayerState plan")?;
            let entry = self
                .peers
                .get_mut(&address)
                .context("no initialisation peer")?;
            let codec = RepLayoutCodec {
                actor: actor_codec(plan.engine_net_version, entry.peer.connection_byte_order()),
                checksums: false,
                limits: Default::default(),
            };
            let user = login_user_id(entry.login_unique_id.as_ref().context("no login user ID")?);
            let values = profile_exchange::player_character_id(
                &user,
                entry.profiles.character_slot.context("no profile slot")?,
            )?;
            let payload = profile_exchange::player_state_update(77, &values, &plan.layout, &codec)?;
            let channel = entry
                .player_state_channel
                .context("no PlayerState channel")?;
            let previous = entry
                .peer
                .actor_reliable_sequence(channel)
                .context("PlayerState channel missing")?;
            let sent = entry
                .peer
                .reliable_actor_bunch_datagrams(channel, payload)?;
            entry.profiles.completed = true;
            entry.initialisation_completion_receipt =
                InitialisationReceipt::committed(&entry.peer, channel, previous);
            entry.ack_pending = false;
            Ok::<_, anyhow::Error>(sent)
        })();
        match sent {
            Ok(sent) => {
                bump(&mut self.stats.players_initialised);
                // Queued for repair as above: only the counter depends on the send.
                if self.send_all(&sent, address) {
                    bump(&mut self.stats.player_character_id_updates_sent);
                }
            }
            Err(error) => {
                self.defer_or_fail_profile_exchange(
                    address,
                    &error,
                    PendingProfileExchange::Complete,
                );
            }
        }
    }

    /// `UConnectedPlayers::TryResolveConnectedPlayersState` (F-973): while
    /// any joined controller has not sent its friends list, no player
    /// finalises; then each completed player finalises once, which restores
    /// a returning pawn and adds a new player to History.
    fn finalise_connected_players(&mut self) {
        if self.peers.values().any(|entry| {
            entry.player.is_some()
                && (entry.friends_query_pending || !entry.profiles.friend_ids_ready)
        }) {
            return;
        }
        let ready: Vec<SocketAddrV4> = self
            .peers
            .iter()
            .filter(|(_, entry)| entry.profiles.completed && !entry.profiles.finalised)
            .map(|(address, _)| *address)
            .collect();
        let world_seconds = self.started.elapsed().as_secs_f32();
        let mut records_finalised = false;
        for address in ready {
            let Some(entry) = self.peers.get_mut(&address) else {
                continue;
            };
            entry.profiles.finalised = true;
            // `FinaliseConnectedPlayerInitialisation` writes the completion time.
            if let Some(record) = &mut entry.connected_player {
                record.complete_time = Some(world_seconds);
                records_finalised = true;
            }
            bump(&mut self.stats.players_finalised);
            self.restore_returning_pawn_uid(address);
            self.add_new_player_to_history(address);
        }
        if records_finalised {
            // Only connections that can carry records now; one whose GameState
            // commits later is marked then (`connected_player_actor_opened`).
            let channel = self
                .game_state_open
                .as_ref()
                .map(|session| session.channel_index);
            self.mark_connected_players_due(|entry| connected_players_receivable(entry, channel));
            self.retry_connected_player_records();
        }
    }

    /// An actor a record can reference or carry (controller, pawn, GameState,
    /// PlayerState) was committed on `address`'s connection: once any record
    /// is finalised, that connection's array may now change, so it alone is
    /// marked for the next pass. No scan runs per poll.
    fn connected_player_actor_opened(&mut self, address: SocketAddrV4) {
        if self.connected_players.is_none()
            || !self.peers.values().any(|entry| {
                entry
                    .connected_player
                    .as_ref()
                    .is_some_and(|record| record.complete_time.is_some())
            })
        {
            return;
        }
        if let Some(entry) = self.peers.get_mut(&address) {
            entry.connected_players_pending = true;
            entry.connected_players_retry_ready = true;
        }
    }

    /// `AddConnectedPlayer` on Begin acceptance: one record per session, with
    /// its start time. Mode A sends nothing yet.
    fn register_connected_player(&mut self, address: SocketAddrV4) {
        if self.connected_players.is_none() {
            return;
        }
        let start_time = self.started.elapsed().as_secs_f32();
        let Some(entry) = self.peers.get_mut(&address) else {
            return;
        };
        if entry.connected_player.is_some() {
            return;
        }
        let Some(next) = self.next_connected_player_record.checked_add(1) else {
            bump(&mut self.stats.connected_player_record_errors);
            return;
        };
        entry.connected_player = Some(ConnectedPlayerRecord {
            index: self.next_connected_player_record,
            start_time,
            complete_time: None,
        });
        self.next_connected_player_record = next;
        bump(&mut self.stats.connected_player_records_created);
    }

    /// Mark the connections selected by `which` for a record/array pass.
    fn mark_connected_players_due(&mut self, which: impl Fn(&PeerEntry) -> bool) {
        if self.connected_players.is_none() {
            return;
        }
        for entry in self.peers.values_mut() {
            if which(entry) {
                entry.connected_players_pending = true;
                entry.connected_players_retry_ready = true;
            }
        }
    }

    /// The record of `owner` as `viewer` may receive it, or `None` while it
    /// must not be sent there: Mode A sends only finalised records, and only
    /// once the controller, pawn and PlayerState the record references are
    /// open on the viewer's connection (S2 order; checked as
    /// `complete_player_initialisation` checks its opens). The lab opens those
    /// actors only on their owner's connection, so cross-connection records
    /// are deferred until those actors are opened there; until then each
    /// connection lists only its own record. A later open re-marks the
    /// connection (`connected_player_actor_opened`). Were another player's
    /// record to qualify, its controller reference would be null: [I]
    /// controllers are relevant only to their owner, and the client gate
    /// skips a null controller.
    fn connected_player_for(
        &self,
        owner: &PeerEntry,
        viewer: &PeerEntry,
        same: bool,
    ) -> Result<Option<(u32, ConnectedPlayerState)>> {
        let Some(record) = &owner.connected_player else {
            return Ok(None);
        };
        let Some(complete_time) = record.complete_time else {
            return Ok(None);
        };
        let open = |channel: Option<u32>| {
            channel.is_some_and(|channel| {
                viewer.peer.has_actor_channel(channel)
                    && !viewer.peer.actor_channel_closing(channel)
            })
        };
        let pawn_channel = self.pawn_open.as_ref().map(|(_, channel, _)| *channel);
        // With possession properties on, the record waits until the client
        // has ACKed its controller channel through the update (the open
        // included) and its pawn channel through the pawn open (F-1116):
        // admitted or merely open is not enough under loss. An abandoned
        // update never settles, so its record is never sent; its session is
        // closed instead (`close_abandoned_possessions`).
        if self.controller_possession && !owner.record_prerequisites.settled() {
            return Ok(None);
        }
        if !same
            || !owner.profiles.completed
            || !open(owner.player_controller_channel)
            || !open(owner.player_state_channel)
            || !open(pawn_channel)
        {
            return Ok(None);
        }
        let (controller, player_state) = owner
            .controller_open_guids
            .context("no committed controller open")?;
        let character = owner
            .controller_startup_pawn_guid
            .context("no committed pawn open")?;
        let user = login_user_id(owner.login_unique_id.as_ref().context("no login user ID")?);
        Ok(Some((
            record.index,
            ConnectedPlayerState {
                player_id: ArchiveText::new(&user).context("login user ID exceeds codec limits")?,
                chr_slot: owner.profiles.character_slot.context("no profile slot")?,
                controller: same.then_some(controller),
                character,
                player_state,
                connection_start_time: record.start_time,
                connection_complete_time: complete_time,
            },
        )))
    }

    /// Admit each due connection's record/array change to reliable GameState
    /// transport, composed from the current state, so a retry never sends a
    /// stale intermediate. Capacity refusal keeps the change pending until
    /// ACK/NAK progress (as [`retry_friends_queries`](Self::retry_friends_queries));
    /// a NAK of an admitted bunch is repaired by the reliable layer.
    fn retry_connected_player_records(&mut self) {
        if self.connected_players.is_none() {
            return;
        }
        let ready: Vec<SocketAddrV4> = self
            .peers
            .iter_mut()
            .filter_map(|(address, entry)| {
                (entry.connected_players_pending
                    && std::mem::take(&mut entry.connected_players_retry_ready))
                .then_some(*address)
            })
            .collect();
        for address in ready {
            self.send_connected_players(address);
        }
    }

    fn send_connected_players(&mut self, address: SocketAddrV4) {
        let (Some(plan), Some(session)) = (&self.connected_players, &self.game_state_open) else {
            return;
        };
        let Some(viewer) = self.peers.get(&address) else {
            return;
        };
        let channel = session.channel_index;
        if !connected_players_receivable(viewer, Some(channel)) {
            // No GameState here yet (or it is closing): nothing is pending
            // for this connection until its GameState open commits, which
            // marks it again.
            if let Some(entry) = self.peers.get_mut(&address) {
                entry.connected_players_pending = false;
            }
            return;
        }
        let mut desired = Vec::new();
        let mut failed = false;
        for (owner_address, owner) in &self.peers {
            match self.connected_player_for(owner, viewer, *owner_address == address) {
                Ok(Some(record)) => desired.push(record),
                Ok(None) => {}
                Err(_) => failed = true,
            }
        }
        // Registration order is array order, as `AddConnectedPlayer` appends.
        desired.sort_unstable_by_key(|(index, _)| *index);
        let array: Vec<u32> = desired.iter().map(|(index, _)| *index).collect();
        let created: Vec<(u32, ConnectedPlayerState)> = desired
            .into_iter()
            .filter(|(index, _)| !viewer.connected_players_sent.contains(index))
            .collect();
        let world_seconds = self.started.elapsed().as_secs_f32();
        let (key, engine_order) = (session.state.key, viewer.peer.connection_byte_order());
        let unchanged = created.is_empty() && array == viewer.connected_players_sent;
        let own_index = viewer.connected_player.as_ref().map(|record| record.index);
        let own = created.iter().any(|(index, _)| Some(*index) == own_index);
        let Some(entry) = self.peers.get_mut(&address) else {
            return;
        };
        if failed {
            bump(&mut self.stats.connected_player_record_errors);
        }
        if unchanged {
            entry.connected_players_pending = false;
            return;
        }
        let mut cache = self.guid_cache.clone();
        let mut exports = entry.exports.clone();
        // The bunch keeps S2's shape: a GameState actor block first, carrying
        // the world time the unreliable update path also sends. It does not
        // change what that path tracks as sent.
        let actor_values = BTreeMap::from([(
            super::game_state_open::handle::WORLD_TIME_SECONDS,
            RepValue::Float(world_seconds.to_bits()),
        )]);
        let sent = compose_connected_players_update(
            &mut cache,
            &mut exports,
            &session.plan,
            plan,
            key,
            &actor_values,
            &created,
            &array,
            engine_order,
        )
        .and_then(|update| {
            let previous = entry
                .peer
                .actor_reliable_sequence(channel)
                .context("GameState channel missing")?;
            let sent = entry.peer.reliable_actor_update_datagrams(
                channel,
                update.payload,
                update.exports.as_slice(),
            )?;
            Ok((sent, previous))
        });
        match sent {
            Ok((sent, previous)) => {
                commit_exports(&mut exports, &sent);
                entry.exports = exports;
                entry.ack_pending = false;
                if !created.is_empty() {
                    entry
                        .connected_player_receipts
                        .push(ConnectedPlayerReceipt {
                            receipt: InitialisationReceipt::committed(
                                &entry.peer,
                                channel,
                                previous,
                            ),
                            records: created.len(),
                            own,
                        });
                }
                entry.connected_players_sent = array;
                entry.connected_players_pending = false;
                self.guid_cache = cache;
                self.stats.connected_player_record_creations_sent = self
                    .stats
                    .connected_player_record_creations_sent
                    .saturating_add(created.len() as u64);
                bump(&mut self.stats.connected_player_array_updates_sent);
                // Queued for reliable repair: a failed socket send only counts.
                self.send_all(&sent, address);
            }
            Err(error) => {
                if transport_capacity_can_drain(entry, &error, channel) {
                    entry.connected_players_retry_ready = false;
                    bump(&mut self.stats.connected_player_record_sends_deferred);
                } else {
                    entry.connected_players_pending = false;
                    bump(&mut self.stats.connected_player_record_errors);
                    if entry.peer.is_failed() {
                        self.remove_peer(address);
                        bump(&mut self.stats.peers_failed);
                    }
                }
            }
        }
    }

    /// This session's pawn open just committed: its controller possession
    /// properties are due now, once per session.
    fn controller_possession_due(&mut self, address: SocketAddrV4) {
        if !self.controller_possession {
            return;
        }
        let Some(entry) = self.peers.get_mut(&address) else {
            return;
        };
        if entry.controller_possession_receipt.channel.is_some()
            || entry.controller_possession_failed
        {
            return;
        }
        entry.controller_possession_pending = true;
        entry.controller_possession_retry_ready = true;
        self.retry_controller_possession();
    }

    /// Admit each due session's possession update, composed from the current
    /// pawn reference. Capacity refusal keeps it pending until ACK/NAK
    /// progress (as [`retry_friends_queries`](Self::retry_friends_queries));
    /// a NAK of an admitted bunch is repaired by the reliable layer from the
    /// stored bytes, never recomposed.
    fn retry_controller_possession(&mut self) {
        if !self.controller_possession {
            return;
        }
        let ready: Vec<SocketAddrV4> = self
            .peers
            .iter_mut()
            .filter_map(|(address, entry)| {
                (entry.controller_possession_pending
                    && std::mem::take(&mut entry.controller_possession_retry_ready))
                .then_some(*address)
            })
            .collect();
        for address in ready {
            self.send_controller_possession(address);
        }
    }

    fn send_controller_possession(&mut self, address: SocketAddrV4) {
        let (Some(plan), Some((_, pawn_channel, _))) =
            (&self.player_controller_open, &self.pawn_open)
        else {
            return;
        };
        let Some(entry) = self.peers.get_mut(&address) else {
            return;
        };
        let open = |channel: u32| {
            entry.peer.has_actor_channel(channel) && !entry.peer.actor_channel_closing(channel)
        };
        let target = entry
            .player_controller_channel
            .filter(|channel| open(*channel))
            .zip(
                entry
                    .controller_startup_pawn_guid
                    .filter(|_| open(*pawn_channel)),
            );
        let pawn_span = InitialisationReceipt::through(&entry.peer, *pawn_channel);
        let Some(((channel, pawn), pawn_span)) = target.zip(pawn_span) else {
            // Nowhere to send it or nothing to name: abandoned. Its record is
            // never sent; the session is closed by the next close pass.
            entry.controller_possession_pending = false;
            entry.controller_possession_failed = true;
            bump(&mut self.stats.controller_possession_errors);
            self.possession_abandoned = true;
            return;
        };
        let codec = RepLayoutCodec {
            actor: actor_codec(plan.engine_net_version, entry.peer.connection_byte_order()),
            checksums: false,
            limits: Default::default(),
        };
        let sent = compose_controller_possession_update(pawn, &plan.actor_layout, &codec).and_then(
            |payload| {
                let previous = entry
                    .peer
                    .actor_reliable_sequence(channel)
                    .context("controller channel missing")?;
                let sent = entry
                    .peer
                    .reliable_actor_bunch_datagrams(channel, payload)?;
                Ok((sent, previous))
            },
        );
        match sent {
            Ok((sent, previous)) => {
                let receipt = InitialisationReceipt::committed(&entry.peer, channel, previous);
                // The record waits for these, not for this admission; the
                // ACK that settles them marks it due (F-1116). A session
                // admits one update (`controller_possession_due`), so they
                // are committed once.
                if entry.record_prerequisites.controller.channel.is_none() {
                    entry.record_prerequisites = RecordPrerequisites {
                        controller: InitialisationReceipt {
                            channel: receipt.channel,
                            first: 0,
                            last: receipt.last,
                            ..Default::default()
                        },
                        pawn: pawn_span,
                    };
                }
                entry.controller_possession_receipt = receipt;
                entry.controller_possession_pending = false;
                entry.ack_pending = false;
                bump(&mut self.stats.controller_possession_updates_sent);
                // Queued for reliable repair: a failed socket send only counts.
                self.send_all(&sent, address);
            }
            Err(error) => {
                if transport_capacity_can_drain(entry, &error, channel) {
                    entry.controller_possession_retry_ready = false;
                    bump(&mut self.stats.controller_possession_sends_deferred);
                } else {
                    entry.controller_possession_pending = false;
                    entry.controller_possession_failed = true;
                    bump(&mut self.stats.controller_possession_errors);
                    if entry.peer.is_failed() {
                        self.remove_peer(address);
                        bump(&mut self.stats.peers_failed);
                    } else {
                        // Its record is never sent; the next close pass
                        // closes the session.
                        self.possession_abandoned = true;
                    }
                }
            }
        }
    }

    /// Send each session's due frozen-movement release: once its delay after
    /// the record's ACK has passed, and again when a NAK lost its latest
    /// copy. Like GameState updates, a copy waits until every reliable bunch
    /// on the pawn channel, its open included, is ACKed
    /// (`actor_channel_settled`): the client drops unreliable bunches on a
    /// channel it has not opened, and applies them at once, ahead of any
    /// reliable bunch still being repaired. Transport pressure holds it back
    /// until ACK/NAK progress.
    fn retry_frozen_movement_release(&mut self) {
        if self.frozen_movement_release.is_none() {
            return;
        }
        let Some((_, channel, _)) = &self.pawn_open else {
            return;
        };
        let channel = *channel;
        let now = Instant::now();
        let ready: Vec<SocketAddrV4> = self
            .peers
            .iter_mut()
            .filter_map(|(address, entry)| {
                let release = &mut entry.frozen_release;
                if release.idle() && release.due_at.is_some_and(|due| due <= now) {
                    release.pending = true;
                }
                let open = entry.controller_startup_pawn_guid.is_some()
                    && entry.peer.has_actor_channel(channel)
                    && !entry.peer.actor_channel_closing(channel);
                let release = &entry.frozen_release;
                (release.pending
                    && !release.deferred
                    && (!open || entry.peer.actor_channel_settled(channel)))
                .then_some(*address)
            })
            .collect();
        for address in ready {
            self.send_frozen_movement_release(address);
        }
    }

    fn send_frozen_movement_release(&mut self, address: SocketAddrV4) {
        let Some((plan, channel, _)) = &self.pawn_open else {
            return;
        };
        let channel = *channel;
        let Some(entry) = self.peers.get_mut(&address) else {
            return;
        };
        if entry.controller_startup_pawn_guid.is_none()
            || !entry.peer.has_actor_channel(channel)
            || entry.peer.actor_channel_closing(channel)
        {
            // No pawn left to release: abandoned.
            entry.frozen_release.pending = false;
            entry.frozen_release.failed = true;
            bump(&mut self.stats.frozen_movement_release_errors);
            return;
        }
        // Unreliable, as the original sends it (S2, M-1161): the current
        // value, false, composed afresh for each copy.
        let sent =
            compose_pawn_frozen_movement_update(plan, false, entry.peer.connection_byte_order())
                .and_then(|payload| entry.peer.actor_update_datagrams(channel, payload));
        match sent {
            Ok(datagrams) => {
                let now = Instant::now();
                let release = &mut entry.frozen_release;
                release.pending = false;
                // A copy that fails to leave the socket is reported missing
                // by the client's next ACK, which repairs it like a NAK.
                release.in_flight = datagrams.iter().map(|(_, id)| *id).collect();
                if release.sent_at.is_none() {
                    release.sent_at = Some(now);
                    bump(&mut self.stats.frozen_movement_releases_sent);
                } else {
                    bump(&mut self.stats.frozen_movement_release_resends);
                }
                entry.ack_pending = false;
                for (datagram, _) in &datagrams {
                    self.send(datagram, address);
                }
            }
            Err(error) => {
                if transport_capacity_can_drain(entry, &error, channel) {
                    entry.frozen_release.deferred = true;
                    bump(&mut self.stats.frozen_movement_release_sends_deferred);
                } else {
                    entry.frozen_release.pending = false;
                    entry.frozen_release.failed = true;
                    bump(&mut self.stats.frozen_movement_release_errors);
                    if entry.peer.is_failed() {
                        self.remove_peer(address);
                        bump(&mut self.stats.peers_failed);
                    }
                }
            }
        }
    }

    /// Acknowledge each session's newest move, as of `now` (the poll's
    /// clock): when a move arrived since the last acknowledgement, at least
    /// [`MOVE_ACK_INTERVAL`] after it, and with the pawn channel open and its
    /// reliable bunches all ACKed. Transport pressure holds it back until
    /// ACK/NAK progress.
    fn retry_pawn_move_acks(&mut self, now: Instant) {
        if !self.pawn_move_acks {
            return;
        }
        let Some((_, channel, _)) = &self.pawn_open else {
            return;
        };
        let channel = *channel;
        let ready: Vec<SocketAddrV4> = self
            .peers
            .iter()
            .filter_map(|(address, entry)| {
                let moves = &entry.pawn_moves;
                (moves.unacked
                    && !moves.deferred
                    && moves
                        .last_ack_at
                        .is_none_or(|at| now.saturating_duration_since(at) >= MOVE_ACK_INTERVAL)
                    && entry.controller_startup_pawn_guid.is_some()
                    && entry.peer.has_actor_channel(channel)
                    && !entry.peer.actor_channel_closing(channel)
                    && entry.peer.actor_channel_settled(channel))
                .then_some(*address)
            })
            .collect();
        for address in ready {
            self.send_pawn_move_ack(address, now);
        }
    }

    fn send_pawn_move_ack(&mut self, address: SocketAddrV4, now: Instant) {
        let Some((plan, channel, _)) = &self.pawn_open else {
            return;
        };
        let channel = *channel;
        let Some(entry) = self.peers.get_mut(&address) else {
            return;
        };
        let Some(timestamp) = entry.pawn_moves.latest_timestamp else {
            return;
        };
        // Unreliable and alone in its datagram, as the original sends it.
        let sent = compose_pawn_move_ack(plan, timestamp, entry.peer.connection_byte_order())
            .and_then(|payload| entry.peer.actor_update_datagrams(channel, payload));
        match sent {
            Ok(datagrams) => {
                let moves = &mut entry.pawn_moves;
                moves.unacked = false;
                moves.last_ack_at = Some(now);
                moves.counts.acks_sent = moves.counts.acks_sent.saturating_add(1);
                bump(&mut self.stats.pawn_move_acks_sent);
                entry.ack_pending = false;
                for (datagram, _) in &datagrams {
                    self.send(datagram, address);
                }
            }
            Err(error) => {
                if transport_capacity_can_drain(entry, &error, channel) {
                    entry.pawn_moves.deferred = true;
                    bump(&mut self.stats.pawn_move_ack_sends_deferred);
                } else {
                    entry.pawn_moves.unacked = false;
                    bump(&mut self.stats.pawn_move_ack_errors);
                    if entry.peer.is_failed() {
                        self.remove_peer(address);
                        bump(&mut self.stats.peers_failed);
                    }
                }
            }
        }
    }

    /// Apply one decoded level visibility report (field 82) to the
    /// connection's visible-level set and queue a newly visible level's
    /// actors; the opens themselves are admitted only at the end of a poll
    /// ([`retry_level_actor_opens`](Self::retry_level_actor_opens)), after
    /// the session's own traffic. The report is honoured whatever the
    /// session's phase: it can arrive only on this connection's own open
    /// controller channel, and the original applies it without a
    /// finalisation or readiness check (F-1023). The opens depend on nothing
    /// else the connection holds: each names its own static chain on a fresh
    /// channel, so they need not wait for any other open's ACK.
    fn level_visibility_report(
        &mut self,
        address: SocketAddrV4,
        parameter: DecodedVisibilityParameter,
    ) {
        let Some(config) = &self.level_visibility else {
            return;
        };
        let DecodedVisibilityParameter::Present(report) = parameter else {
            // The original zeroes its parameter before reading (bit1 and
            // both names cleared), so an absent value would be a hide of no
            // name; nothing here holds such a name.
            bump(&mut self.stats.level_visibility_reports_absent);
            return;
        };
        let level = config.admitted(&report.package_name);
        if !report.visible {
            bump(&mut self.stats.level_visibility_reports_hidden);
            if let Some(level) = level {
                self.hide_level(address, &level.key);
            }
            return;
        }
        // A rejected add makes the original close the connection (F-1023),
        // but its rejection tests packages a native server does not have, so
        // an inadmissible name is counted and ignored, never closed on.
        let Some(level) = level else {
            bump(&mut self.stats.level_visibility_reports_refused_name);
            return;
        };
        let Some(entry) = self.peers.get(&address) else {
            return;
        };
        if entry.visible_levels.contains(&level.key) {
            bump(&mut self.stats.level_visibility_reports_duplicate);
            return;
        }
        if entry.visible_levels.len() >= config.max_visible_levels {
            bump(&mut self.stats.level_visibility_reports_over_bound);
            return;
        }
        let registering = !entry.level_actors.contains_key(&level.key)
            && !self.level_actor_holders.contains_key(&level.key);
        if registering {
            // A level chain the server already names, in any letter case
            // (another map, a package exported for its own opens that was
            // not known in advance), is not a streamed level here.
            let collides = match config.level_chains(&level.spelling) {
                Ok(chains) => chains[config.actors.names.len()..]
                    .iter()
                    .any(|chain| self.guid_cache.has_static_ignoring_case(chain)),
                Err(_) => true,
            };
            if collides {
                bump(&mut self.stats.level_visibility_reports_refused_name);
                return;
            }
        }
        let names = config.actors.names.len();
        let max_levels = config.max_visible_levels;
        let Some(entry) = self.peers.get_mut(&address) else {
            return;
        };
        entry.visible_levels.insert(level.key.clone());
        bump(&mut self.stats.level_visibility_reports_added);
        if entry.level_actors.contains_key(&level.key) {
            // Shown again: every actor never admitted on this connection is
            // due again; an opened one is never opened twice.
            requeue_level(entry, &level.key);
            return;
        }
        if entry.level_actors.len() >= max_levels {
            // Hidden levels keep their records: the record bound is reached
            // by levels shown over the session, not only visible ones.
            self.stats.level_actor_opens_refused = self
                .stats
                .level_actor_opens_refused
                .saturating_add(names as u64);
            return;
        }
        entry.level_actors.insert(
            level.key.clone(),
            (0..names)
                .map(|_| LevelActorSlot::Pending { refused: None })
                .collect(),
        );
        enqueue_level(entry, &level.key);
        self.level_actor_holders
            .entry(level.key)
            .or_insert(LevelHolding {
                holders: 0,
                spelling: level.spelling,
            })
            .holders += 1;
    }

    /// A hidden report: the level leaves the set. Opens still awaiting
    /// admission are withdrawn (the original replicates a level's actors only
    /// while it is visible, F-1023). Admitted opens stay open: the original
    /// closes a hidden level's channels (reason 2, F-1023), but no hide has
    /// been observed on the wire, so this first version closes nothing, and
    /// the channels stay counted against the connection's budget.
    fn hide_level(&mut self, address: SocketAddrV4, key: &str) {
        let Some(entry) = self.peers.get_mut(&address) else {
            return;
        };
        entry.visible_levels.remove(key);
        let Some(slots) = entry.level_actors.get_mut(key) else {
            return;
        };
        let mut withdrawn = 0;
        for slot in slots.iter_mut() {
            if matches!(slot, LevelActorSlot::Pending { .. }) {
                *slot = LevelActorSlot::Unsent;
                withdrawn += 1;
            }
        }
        entry.level_actor_queue.retain(|queued| queued != key);
        self.stats.level_actor_opens_withdrawn = self
            .stats
            .level_actor_opens_withdrawn
            .saturating_add(withdrawn);
        let opened = slots
            .iter()
            .any(|slot| matches!(slot, LevelActorSlot::Opened { .. }));
        if opened {
            bump(&mut self.stats.level_hides_actors_kept);
        } else {
            // Nothing of it was sent: a later report may open it afresh.
            entry.level_actors.remove(key);
            self.release_level_actors(key);
        }
    }

    /// One fewer live connection holds `key`'s actors; with none left, no
    /// connection of this feature names its static identities again, so the
    /// ones it created and nothing else has touched since are forgotten
    /// (numbers stay consumed), bounding the cache by live sessions. A shared
    /// identity (a later server open that exported the same package, say)
    /// lost its owner tag when shared and survives.
    fn release_level_actors(&mut self, key: &str) {
        let Some(holding) = self.level_actor_holders.get_mut(key) else {
            return;
        };
        holding.holders = holding.holders.saturating_sub(1);
        if holding.holders > 0 {
            return;
        }
        let Some(holding) = self.level_actor_holders.remove(key) else {
            return;
        };
        if let Some(config) = &self.level_visibility
            && let Ok(chains) = config.level_chains(&holding.spelling)
        {
            for chain in &chains {
                self.guid_cache.forget_owned_static(chain);
            }
        }
    }

    /// The lowest-priority pass of a poll: for each connection with queued
    /// level actor opens, admit at most [`LEVEL_ACTOR_OPENS_PER_PASS`], in
    /// report order, and only while its sent ring is at least half free, so
    /// a burst of reports cannot fill the ring ahead of the session's own
    /// records and updates. A connection held back by transport waits for
    /// ACK/NAK progress; one stopped by the per-pass bound continues next
    /// poll. A NAK of an admitted open is repaired by the reliable layer from
    /// the stored parts, never recomposed.
    fn retry_level_actor_opens(&mut self) {
        if self.level_visibility.is_none() {
            return;
        }
        let ready: Vec<SocketAddrV4> = self
            .peers
            .iter_mut()
            .filter_map(|(address, entry)| {
                ((!entry.level_actor_queue.is_empty() || entry.level_channels_released)
                    && std::mem::take(&mut entry.level_actors_retry_ready))
                .then_some(*address)
            })
            .collect();
        for address in ready {
            self.send_level_actor_opens(address);
        }
    }

    /// Session channels the lab configures (controller, pawn, GameState,
    /// PlayerState, managers, quests): a level actor never takes one, even
    /// before its own open commits.
    fn reserved_session_channels(&self) -> Vec<u32> {
        [
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
        ]
        .into_iter()
        .flatten()
        .chain(self.manager_opens.iter().map(|(_, _, channel)| *channel))
        .chain(self.quest_opens.iter().map(|(_, channel)| *channel))
        .collect()
    }

    /// `entry`'s level actor channel budget now: the channels its level
    /// actors hold, and the free actor channels above the session channels
    /// (excluding them) that anything may still take.
    fn level_channel_budget(&self, entry: &PeerEntry) -> LevelChannelBudget {
        let reserved = self.reserved_session_channels();
        let first = reserved.iter().max().map_or(2, |channel| channel + 1);
        let end = entry.peer.config().packet.limits.max_channels;
        let control = entry.peer.config().control_channel_index;
        let free = (first..end)
            .filter(|channel| {
                *channel != control
                    && !reserved.contains(channel)
                    && !entry.peer.has_actor_channel(*channel)
            })
            .count();
        let held = entry
            .level_actors
            .values()
            .flatten()
            .filter(|slot| matches!(slot, LevelActorSlot::Opened { .. }))
            .count();
        LevelChannelBudget {
            cap: self
                .level_visibility
                .as_ref()
                .map_or(MAX_LEVEL_ACTOR_CHANNELS, |config| config.max_level_channels),
            first,
            reserved,
            held,
            free,
        }
    }

    fn send_level_actor_opens(&mut self, address: SocketAddrV4) {
        let mut admitted = 0;
        let Some(entry) = self.peers.get(&address) else {
            return;
        };
        // Computed once per pass, then kept current as opens are admitted.
        let mut budget = self.level_channel_budget(entry);
        if let Some(entry) = self.peers.get_mut(&address)
            && std::mem::take(&mut entry.level_channels_released)
            && budget.held < budget.cap
        {
            // Only as many channel refusals as the budget can now admit; a
            // cap refusal is never re-armed by a released channel.
            let requeued = requeue_channel_refusals(entry, budget.remaining());
            self.stats.level_actor_opens_requeued = self
                .stats
                .level_actor_opens_requeued
                .saturating_add(requeued as u64);
        }
        loop {
            let Some(config) = &self.level_visibility else {
                return;
            };
            let Some(entry) = self.peers.get_mut(&address) else {
                return;
            };
            let Some(key) = entry.level_actor_queue.front().cloned() else {
                return;
            };
            let Some(index) = entry.level_actors.get(&key).and_then(|slots| {
                slots
                    .iter()
                    .position(|slot| matches!(slot, LevelActorSlot::Pending { .. }))
            }) else {
                entry.level_actor_queue.pop_front();
                continue;
            };
            if admitted >= LEVEL_ACTOR_OPENS_PER_PASS {
                // The rest go out on later polls, after their traffic.
                entry.level_actors_retry_ready = true;
                return;
            }
            let ring = entry.peer.sequencer().notify().sent_records().len();
            let capacity = entry.peer.config().sent_capacity;
            if (capacity - ring.min(capacity)) * 2 < capacity {
                // Leave half the ring to the session's own traffic; ACK/NAK
                // progress (which this outstanding traffic brings) re-arms.
                bump(&mut self.stats.level_actor_opens_deferred);
                return;
            }
            let set_slot = |entry: &mut PeerEntry, slot: LevelActorSlot| {
                if let Some(current) = entry
                    .level_actors
                    .get_mut(&key)
                    .and_then(|slots| slots.get_mut(index))
                {
                    *current = slot;
                }
            };
            let previous = entry
                .level_actors
                .get(&key)
                .and_then(|slots| slots.get(index))
                .and_then(|slot| match slot {
                    LevelActorSlot::Pending { refused } => *refused,
                    _ => None,
                });
            let mut refuse = |entry: &mut PeerEntry, cause: LevelRefusal| {
                set_slot(entry, LevelActorSlot::Refused(cause));
                if cause == LevelRefusal::Channels && entry.visible_levels.contains(&key) {
                    entry.level_actors_refused = true;
                }
                // Once per cause transition, never per retry.
                if previous != Some(cause) {
                    bump(&mut self.stats.level_actor_opens_refused);
                }
            };
            if budget.remaining() == 0 {
                // At the cap, or the reserve would be touched.
                let cause = if budget.held >= budget.cap {
                    LevelRefusal::Cap
                } else {
                    LevelRefusal::Channels
                };
                refuse(entry, cause);
                continue;
            }
            let mut start = budget.first;
            let channel = loop {
                match entry.peer.free_actor_channel(start) {
                    Some(channel) if budget.reserved.contains(&channel) => start = channel + 1,
                    other => break other,
                }
            };
            let Some(channel) = channel else {
                refuse(entry, LevelRefusal::Channels);
                continue;
            };
            let Some(spelling) = self
                .level_actor_holders
                .get(&key)
                .map(|holding| holding.spelling.clone())
            else {
                set_slot(entry, LevelActorSlot::Unsent);
                bump(&mut self.stats.level_actor_open_errors);
                continue;
            };
            let order = entry.peer.connection_byte_order();
            let mut cache = self.guid_cache.clone();
            let mut exports = entry.exports.clone();
            let sent = config
                .actors
                .actor_chain(index, &spelling)
                .and_then(|chain| {
                    Ok(StaticActorOpen {
                        chain,
                        blocks: vec![actor_role_block(config.engine_net_version)?],
                        engine_net_version: config.engine_net_version,
                    })
                })
                .and_then(|open| {
                    compose_static_actor_open(&mut cache, &mut exports, &open, channel, order)
                })
                .and_then(|(composed, _)| {
                    entry
                        .peer
                        .reliable_actor_open_datagrams(&composed.bunch, composed.exports.as_slice())
                });
            match sent {
                Ok(sent) => {
                    commit_exports(&mut exports, &sent);
                    entry.exports = exports;
                    entry.ack_pending = false;
                    set_slot(
                        entry,
                        LevelActorSlot::Opened {
                            channel,
                            settled: false,
                            abandoned: false,
                        },
                    );
                    self.guid_cache = cache;
                    admitted += 1;
                    budget.held += 1;
                    budget.free = budget.free.saturating_sub(1);
                    bump(&mut self.stats.level_actor_opens_sent);
                    // Queued for reliable repair: a failed socket send only counts.
                    self.send_all(&sent, address);
                }
                Err(error) => {
                    if transport_capacity_can_drain(entry, &error, channel) {
                        // Keep it and everything after it, in order, until
                        // ACK/NAK progress.
                        bump(&mut self.stats.level_actor_opens_deferred);
                        return;
                    }
                    // Composition or framing that cannot drain, including an
                    // exhausted static GUID space: counted, the connection
                    // kept unless its transport failed.
                    set_slot(entry, LevelActorSlot::Unsent);
                    bump(&mut self.stats.level_actor_open_errors);
                    if entry.peer.is_failed() {
                        self.remove_peer(address);
                        bump(&mut self.stats.peers_failed);
                        return;
                    }
                }
            }
        }
    }

    /// Resume only a transport-capacity refusal after packet notifications
    /// released space. The UID stage and the state stage never repeat once
    /// queued; semantic gaps and empty-queue capacity refusals remain terminal.
    fn retry_pending_pawn_restores(&mut self) {
        let ready: Vec<_> = self
            .peers
            .iter_mut()
            .filter_map(|(address, entry)| {
                if !std::mem::take(&mut entry.pawn_restore_retry_ready) {
                    return None;
                }
                entry
                    .pending_pawn_restore
                    .take()
                    .map(|stage| (*address, stage))
            })
            .collect();
        for (address, stage) in ready {
            match stage {
                PendingPawnRestore::Uid => self.restore_returning_pawn_uid(address),
                PendingPawnRestore::State(index) => {
                    self.restore_returning_pawn_state(address, index)
                }
            }
        }
    }

    /// Completion's new-player path. Validate the candidate before publication,
    /// then queue a reliable whole-array update for every opened tracker.
    fn add_new_player_to_history(&mut self, address: SocketAddrV4) {
        let Some(index) = self
            .manager_opens
            .iter()
            .position(|(key, ..)| *key == ManagerSlot::PlayerHistory.key())
        else {
            return; // Bare lab configurations can omit the tracker.
        };
        let candidate = (|| -> Result<Option<ManagerOpenPlan>> {
            let entry = self.peers.get(&address).context("no completion peer")?;
            let user = login_user_id(entry.login_unique_id.as_ref().context("no login user ID")?);
            let slot = entry.profiles.character_slot.context("no profile slot")?;
            if self.saved_player_controllers.iter().any(|saved| {
                saved.chr_slot == slot && super::restore_rules::names_match(&saved.player_id, &user)
            }) {
                return Ok(None);
            }
            let mut plan = self.manager_opens[index].1.clone();
            if !plan.add_player_to_history(
                ArchiveText::new(&user)?,
                slot,
                entry
                    .profiles
                    .character_name
                    .clone()
                    .context("no CharacterName")?,
            )? {
                bump(&mut self.stats.player_history_duplicates);
                return Ok(None);
            }
            // Also validate the future open, whose roles/components consume
            // part of the same property budget. Disposable GUID state only.
            compose_manager_open(
                &mut self.guid_cache.clone(),
                &mut ConnectionExports::new(),
                &plan,
                ManagerSlot::PlayerHistory.key(),
                self.manager_opens[index].2,
                super::archive::ByteOrder::Little,
            )?;
            compose_player_history_update(&plan, super::archive::ByteOrder::Little)?;
            Ok(Some(plan))
        })();
        let plan = match candidate {
            Ok(Some(plan)) => plan,
            Ok(None) => return,
            Err(_) => {
                bump(&mut self.stats.player_history_errors);
                return;
            }
        };
        self.manager_opens[index].1 = plan;
        bump(&mut self.stats.player_history_appends);
        for entry in self.peers.values_mut() {
            if entry
                .player_history_channel
                .is_some_and(|channel| entry.peer.has_actor_channel(channel))
            {
                entry.player_history_pending = true;
                entry.player_history_retry_ready = true;
            }
        }
    }

    /// Coalesce unsent publications into the latest whole array. Successful
    /// queueing transfers delivery to the reliable transport; capacity refusal
    /// waits for ACK/NAK progress, while semantic failures remain terminal.
    fn retry_pending_player_history(&mut self) {
        let Some(index) = self
            .manager_opens
            .iter()
            .position(|(key, ..)| *key == ManagerSlot::PlayerHistory.key())
        else {
            return;
        };
        let addresses: Vec<_> = self
            .peers
            .iter_mut()
            .filter_map(|(address, entry)| {
                (std::mem::take(&mut entry.player_history_retry_ready)
                    && entry.player_history_pending)
                    .then_some(*address)
            })
            .collect();
        for address in addresses {
            let entry = self
                .peers
                .get_mut(&address)
                .expect("collected history peer");
            let Some(channel) = entry
                .player_history_channel
                .filter(|channel| entry.peer.has_actor_channel(*channel))
            else {
                entry.player_history_pending = false;
                continue;
            };
            let sent = compose_player_history_update(
                &self.manager_opens[index].1,
                entry.peer.connection_byte_order(),
            )
            .and_then(|payload| entry.peer.reliable_actor_bunch_datagrams(channel, payload));
            match sent {
                Ok(sent) => {
                    entry.player_history_pending = false;
                    entry.ack_pending = false;
                    self.send_all(&sent, address);
                }
                Err(error) => {
                    entry.player_history_pending =
                        transport_capacity_can_drain(entry, &error, channel);
                    bump(&mut self.stats.player_history_errors);
                }
            }
        }
    }

    /// Apply the reply policy (module docs) to delivered messages in order,
    /// stopping after a reply that closes the peer.
    fn answer(&mut self, address: SocketAddrV4, messages: &[Decoded]) {
        // The admission transition has not committed while its reply waits.
        // Suppress additional dispatch rather than retaining arbitrary client
        // payloads or repeating PreLogin. Packet notifications still run above.
        if self
            .peers
            .get(&address)
            .is_some_and(|entry| entry.pending_control_reply.is_some())
        {
            return;
        }
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
                if self.authenticator.is_some() {
                    use super::control_messages::NetIdType;
                    let steam_type = match &unique_net_id.kind {
                        NetIdType::Indexed(3) => true,
                        NetIdType::Named(name) => {
                            name.units() == "STEAM".encode_utf16().collect::<Vec<_>>()
                        }
                        _ => false,
                    };
                    let expected = self
                        .peers
                        .get(&address)
                        .filter(|peer| peer.auth_accepted)
                        .and_then(|peer| peer.auth_identity)
                        .map(|id| id.to_string().encode_utf16().collect::<Vec<_>>());
                    if !steam_type || expected.as_ref() != Some(&login_user_id(unique_net_id)) {
                        return Some(Reply::Failure("Steam authentication identity mismatch"));
                    }
                }
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
            ControlMessage::SplitJoin { .. } if self.authenticator.is_some() => Some(
                Reply::Failure("Steam authenticated split joins are unsupported"),
            ),
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
        let message = match message {
            Ok(message) => message,
            Err(_) => {
                bump(&mut self.stats.reply_errors);
                return false;
            }
        };
        self.send_control_reply(address, *reply, message)
    }

    fn retry_control_replies(&mut self) {
        let ready: Vec<_> = self
            .peers
            .iter_mut()
            .filter_map(|(address, entry)| {
                if !std::mem::take(&mut entry.control_reply_retry_ready) {
                    return None;
                }
                entry
                    .pending_control_reply
                    .take()
                    .map(|(reply, message)| (*address, reply, message))
            })
            .collect();
        for (address, reply, message) in ready {
            self.send_control_reply(address, reply, message);
        }
    }

    fn send_control_reply(
        &mut self,
        address: SocketAddrV4,
        reply: Reply,
        message: ControlMessage,
    ) -> bool {
        let Some(entry) = self.peers.get_mut(&address) else {
            return false;
        };
        let datagram = match entry.peer.reliable_control_packet(message.clone()) {
            Ok(datagram) => datagram,
            Err(error) => {
                bump(&mut self.stats.reply_errors);
                if transport_capacity_can_drain(entry, &error, 0) {
                    entry.pending_control_reply = Some((reply, message));
                    entry.control_reply_retry_ready = false;
                    return false;
                }
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
                entry.spectator = spectator;
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

    /// Start the bounded initial-open sequence once for this Join. Duplicate
    /// Join is rejected by admission; a committed controller is never reopened.
    fn send_player_controller_open(&mut self, address: SocketAddrV4) -> bool {
        if self.player_controller_open.is_none() {
            return false;
        }
        let player_state_is_host = !self.peers.iter().any(|(other, peer)| {
            *other != address && peer.admission.state == ClientLoginState::Joined
        });
        let Some(entry) = self.peers.get_mut(&address) else {
            return false;
        };
        if entry.initial_opens.is_some() || entry.player_controller_channel.is_some() {
            return false;
        }
        let mut remaining = VecDeque::from([InitialOpenStage::Controller]);
        if self.pawn_open.is_some() {
            remaining.push_back(InitialOpenStage::Pawn);
        }
        if self.game_state_open.is_some() {
            remaining.push_back(InitialOpenStage::GameState);
        }
        if self.player_state_open.is_some() {
            remaining.push_back(InitialOpenStage::PlayerState);
        }
        let mut extras: Vec<_> = self
            .manager_opens
            .iter()
            .enumerate()
            .map(|(index, (.., channel))| (*channel, InitialOpenStage::Manager(index)))
            .chain(
                self.quest_opens
                    .iter()
                    .enumerate()
                    .map(|(index, (_, channel))| (*channel, InitialOpenStage::Quest(index))),
            )
            .collect();
        extras.sort_unstable_by_key(|&(channel, _)| channel);
        remaining.extend(extras.into_iter().map(|(_, stage)| stage));
        entry.initial_open_outcomes = InitialOpenOutcomes::default();
        for &stage in &remaining {
            entry.initial_open_outcomes.configured.record(stage);
        }
        entry.initial_opens = Some(PendingInitialOpens {
            remaining,
            controller_guids: None,
            player_state_is_host,
            begin_slot: None,
            retry_ready: false,
        });
        self.continue_initial_opens(address);
        self.peers.get(&address).is_some_and(|entry| {
            entry.initial_opens.is_some() || entry.player_controller_channel.is_some()
        })
    }

    /// Retry only after packet notifications release capacity. In particular,
    /// merely polling an unchanged full queue cannot spin or consume more IDs.
    fn retry_controller_startup(&mut self) {
        let Some(actions) = &self.controller_startup else {
            return;
        };
        let Some(plan) = &self.player_controller_open else {
            return;
        };
        let needs_pawn = actions.contains(&ControllerStartupAction::RestartPawn);
        let addresses: Vec<_> = self.peers.keys().copied().collect();
        for address in addresses {
            while let Some(entry) = self.peers.get_mut(&address) {
                let Some(next) = entry.controller_startup_next else {
                    break;
                };
                if !entry.controller_startup_retry_ready {
                    break;
                }
                if next == actions.len() {
                    entry.controller_startup_next = None;
                    entry.controller_startup_retry_ready = false;
                    break;
                }
                let Some(channel) = entry.player_controller_channel else {
                    entry.controller_startup_failed = true;
                    entry.controller_startup_next = None;
                    break;
                };
                // A reliable ACK is a conservative prerequisite for bare GUID
                // references across channels. It proves no gameplay readiness.
                if !entry.peer.actor_channel_settled(channel)
                    || (needs_pawn
                        && self.pawn_open.as_ref().is_none_or(|(_, channel, _)| {
                            !entry.peer.actor_channel_settled(*channel)
                        }))
                {
                    entry.controller_startup_retry_ready = false;
                    break;
                }
                let codec = RepLayoutCodec {
                    actor: actor_codec(plan.engine_net_version, entry.peer.connection_byte_order()),
                    checksums: false,
                    limits: Default::default(),
                };
                let pawn = if needs_pawn {
                    entry
                        .controller_startup_pawn_guid
                        .context("startup pawn GUID unavailable")
                        .map(Some)
                } else {
                    Ok(None)
                };
                let sent = pawn
                    .and_then(|pawn| {
                        controller_startup_payload(
                            actions[next],
                            self.controller_startup_bound,
                            pawn,
                            &codec,
                        )
                    })
                    .and_then(|payload| {
                        entry.peer.reliable_actor_bunch_datagrams(channel, payload)
                    });
                match sent {
                    Ok(sent) => {
                        entry.controller_startup_next = Some(next + 1);
                        entry.controller_startup_queued += 1;
                        entry.ack_pending = false;
                        // Transport owns loss repair after queue acceptance.
                        // Socket failure must not enqueue the same call twice.
                        for datagram in &sent.datagrams {
                            if !matches!(self.socket.send_to(datagram, address), Ok(length) if length == datagram.len())
                            {
                                bump(&mut self.stats.io_errors);
                            }
                        }
                    }
                    Err(error) => {
                        if !transport_capacity_can_drain(entry, &error, channel) {
                            entry.controller_startup_failed = true;
                            entry.controller_startup_next = None;
                        }
                        entry.controller_startup_retry_ready = false;
                        break;
                    }
                }
            }
        }
    }

    fn retry_initial_opens(&mut self) {
        let ready: Vec<_> = self
            .peers
            .iter_mut()
            .filter_map(|(address, entry)| {
                entry.initial_opens.as_mut().and_then(|pending| {
                    std::mem::take(&mut pending.retry_ready).then_some(*address)
                })
            })
            .collect();
        for address in ready {
            self.continue_initial_opens(address);
        }
    }

    fn continue_initial_opens(&mut self, address: SocketAddrV4) {
        loop {
            let Some(pending) = self
                .peers
                .get(&address)
                .and_then(|entry| entry.initial_opens.as_ref())
            else {
                return;
            };
            let Some(stage) = pending.remaining.front().copied() else {
                if let Some(entry) = self.peers.get_mut(&address) {
                    entry.initial_opens = None;
                    if self.controller_startup.is_some() {
                        if entry.initial_open_outcomes.failed.is_empty() {
                            entry.controller_startup_next = Some(0);
                            entry.controller_startup_retry_ready = true;
                        } else {
                            entry.controller_startup_failed = true;
                        }
                    }
                }
                // Exhausted means no initial work remains, not that every open
                // succeeded: permanent stage errors retain the existing policy.
                self.refresh_members_after_join(address);
                return;
            };
            let guids = pending.controller_guids;
            let outcome = match stage {
                InitialOpenStage::Controller => self.send_initial_controller_open(address),
                InitialOpenStage::Pawn => match guids {
                    Some((controller, player_state)) => {
                        self.send_pawn_open(address, controller, player_state)
                    }
                    None => InitialOpenOutcome::Failed,
                },
                InitialOpenStage::GameState => self.send_game_state_open(address),
                InitialOpenStage::PlayerState => match guids {
                    Some((controller, player_state)) => {
                        self.send_player_state_open(address, controller, player_state)
                    }
                    None => InitialOpenOutcome::Failed,
                },
                InitialOpenStage::Manager(index) => self.send_manager_open(address, index),
                InitialOpenStage::Quest(index) => self.send_quest_open(address, index),
            };
            let Some(entry) = self.peers.get_mut(&address) else {
                return;
            };
            let Some(pending) = &mut entry.initial_opens else {
                return;
            };
            entry.initial_open_outcomes.deferred = matches!(outcome, InitialOpenOutcome::Deferred);
            match outcome {
                InitialOpenOutcome::Committed(channel) => {
                    entry.initial_open_outcomes.committed.record(stage);
                    entry
                        .initial_open_outcomes
                        .awaiting_ack
                        .push(InitialOpenReceipt {
                            stage,
                            channel,
                            abandoned: false,
                        });
                }
                InitialOpenOutcome::Failed => entry.initial_open_outcomes.failed.record(stage),
                InitialOpenOutcome::Deferred => {}
            }
            if matches!(outcome, InitialOpenOutcome::Failed)
                && matches!(stage, InitialOpenStage::PlayerState)
                && pending.begin_slot.take().is_some()
            {
                bump(&mut self.stats.initialisation_calls_ignored);
            }
            match outcome {
                InitialOpenOutcome::Deferred => {
                    pending.retry_ready = false;
                    return;
                }
                InitialOpenOutcome::Failed if matches!(stage, InitialOpenStage::Controller) => {
                    entry.controller_startup_failed = self.controller_startup.is_some();
                    for &stage in pending.remaining.iter().skip(1) {
                        entry.initial_open_outcomes.skipped.record(stage);
                    }
                    entry.initial_opens = None;
                    return;
                }
                InitialOpenOutcome::Committed(_) | InitialOpenOutcome::Failed => {
                    // Preserve permanent-error behavior: later independent
                    // stages are still attempted; only pressure retains a stage.
                    let _ = pending.remaining.pop_front();
                }
            }
        }
    }

    /// Queue pressure is recoverable only while existing traffic can drain.
    /// A repeated refusal with an empty live queue consumes the failed stage
    /// instead of retrying an impossible single-open size indefinitely.
    fn initial_open_failure(
        &mut self,
        address: SocketAddrV4,
        error: &anyhow::Error,
        channel: u32,
    ) -> InitialOpenOutcome {
        let Some(entry) = self.peers.get(&address) else {
            return InitialOpenOutcome::Failed;
        };
        if transport_capacity_can_drain(entry, error, channel) {
            return InitialOpenOutcome::Deferred;
        }
        if entry.peer.is_failed() {
            self.remove_peer(address);
            bump(&mut self.stats.peers_failed);
        }
        InitialOpenOutcome::Failed
    }

    /// Compose and send the joined peer's PlayerController open. GUIDs and
    /// export state are committed only once the datagram is built; a failure
    /// counts `player_controller_open_errors` and removes a failed peer.
    fn send_initial_controller_open(&mut self, address: SocketAddrV4) -> InitialOpenOutcome {
        let Some(plan) = &self.player_controller_open else {
            return InitialOpenOutcome::Failed;
        };
        let Some(entry) = self.peers.get_mut(&address) else {
            return InitialOpenOutcome::Failed;
        };
        let placement = connection_placement(
            &mut entry.placement,
            &mut self.spawn,
            &mut self.stats,
            plan.spawn_location,
        );
        let mut cache = self.guid_cache.clone();
        let mut exports = entry.exports.clone();
        let sent = compose_player_controller_open_at(
            &mut cache,
            &mut exports,
            plan,
            placement.location,
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
            Err(error) => {
                bump(&mut self.stats.player_controller_open_errors);
                let channel = plan.channel_index;
                return self.initial_open_failure(address, &error, channel);
            }
        };
        entry.player_controller_channel = Some(plan.channel_index);
        entry.controller_open_guids = Some((guids.actor, guids.player_state));
        entry.backend_proxy = guids.components.first().copied();
        entry.player_data = guids.components.get(1).copied();
        entry.bestiary = guids.components.get(2).copied();
        if let Some(pending) = &mut entry.initial_opens {
            pending.controller_guids = Some((guids.actor, guids.player_state));
        }
        commit_exports(&mut exports, &sent);
        entry.exports = exports;
        entry.ack_pending = false;
        let channel = plan.channel_index;
        self.guid_cache = cache;
        if self.send_all(&sent, address) {
            bump(&mut self.stats.player_controller_opens_sent);
        }
        self.connected_player_actor_opened(address);
        InitialOpenOutcome::Committed(channel)
    }

    /// Compose and send the player's pawn open (F-797) on its owner's
    /// connection, committing GUID and export state only once every datagram
    /// is built; a failure counts `pawn_open_errors`.
    fn send_pawn_open(
        &mut self,
        address: SocketAddrV4,
        controller: u32,
        player_state: u32,
    ) -> InitialOpenOutcome {
        let (Some((plan, channel, content)), Some(controller_plan)) =
            (&self.pawn_open, &self.player_controller_open)
        else {
            return InitialOpenOutcome::Failed;
        };
        let Some(entry) = self.peers.get_mut(&address) else {
            return InitialOpenOutcome::Failed;
        };
        let Some(connection) = entry.connection_id else {
            return InitialOpenOutcome::Failed;
        };
        let mut content = content.clone();
        let mut claimed = entry.pending_pawn_uid;
        if let Some(library) = &mut self.actor_uids {
            let grant = claimed.or_else(|| {
                self.next_pawn_object_id.and_then(|desired| {
                    self.next_pawn_object_id = desired.checked_add(1);
                    library.try_claim(desired, None).ok()
                })
            });
            match grant {
                Some(uid) if uid > 0 => {
                    content.icarus_uid = Some(uid);
                    claimed = Some(uid);
                    entry.pending_pawn_uid = Some(uid);
                }
                other => {
                    if let Some(uid) = other {
                        library.free(uid);
                    }
                    bump(&mut self.stats.pawn_open_errors);
                    return InitialOpenOutcome::Failed;
                }
            }
        }
        // The controller open resolved this connection's placement; the pawn
        // opens at the same location (F-1140).
        let placement = connection_placement(
            &mut entry.placement,
            &mut self.spawn,
            &mut self.stats,
            controller_plan.spawn_location,
        );
        let mut cache = self.guid_cache.clone();
        let mut exports = entry.exports.clone();
        let state = PawnOpenState {
            key: pawn_key(connection),
            controller,
            player_state,
            location: placement.location,
            content,
        };
        let sent = PawnTemperatureState::from_values(&state.content.character_state).and_then(
            |temperature| {
                let open = compose_pawn_open(
                    &mut cache,
                    &mut exports,
                    plan,
                    &state,
                    *channel,
                    entry.peer.connection_byte_order(),
                )?;
                let export_payloads: Vec<Payload> = open.exports.into_iter().collect();
                let sent = entry
                    .peer
                    .reliable_actor_open_datagrams(&open.bunch, &export_payloads)?;
                Ok((sent, temperature))
            },
        );
        let (sent, temperature) = match sent {
            Ok(sent) => sent,
            Err(error) => {
                bump(&mut self.stats.pawn_open_errors);
                if !transport_capacity_can_drain(entry, &error, *channel)
                    && let Some(uid) = entry.pending_pawn_uid.take()
                    && let Some(library) = &mut self.actor_uids
                {
                    library.free(uid);
                }
                let channel = *channel;
                return self.initial_open_failure(address, &error, channel);
            }
        };
        entry.pending_pawn_uid = None;
        entry.pawn_uid = claimed;
        // A server placement is the move store's starting point (F-1140): the
        // first client move is checked against it, not against nothing.
        if placement.source != PlacementSource::Default {
            entry.pawn_moves.location = None;
            entry.pawn_moves.placement = Some((placement.location.map(f64::from), Instant::now()));
        }
        entry.controller_startup_pawn_guid = cache.actor_guid(state.key).ok();
        entry.pawn_temperature = Some(temperature);
        entry.queued_pawn_temperature = Some(temperature);
        entry.pawn_character_state = Some(state.content.character_state);
        commit_exports(&mut exports, &sent);
        entry.exports = exports;
        entry.ack_pending = false;
        let channel = *channel;
        self.guid_cache = cache;
        let delivered = self.send_all(&sent, address);
        // The possession properties name this pawn, so they follow its open.
        self.controller_possession_due(address);
        if delivered {
            bump(&mut self.stats.pawn_opens_sent);
            self.complete_player_initialisation(address);
        }
        self.connected_player_actor_opened(address);
        InitialOpenOutcome::Committed(channel)
    }

    /// Compose and send the GameState open (F-794) over as many datagrams as
    /// it needs, committing GUID and export state only once every datagram
    /// is built; a failure counts `game_state_open_errors`.
    fn send_game_state_open(&mut self, address: SocketAddrV4) -> InitialOpenOutcome {
        let Some(session) = &self.game_state_open else {
            return InitialOpenOutcome::Failed;
        };
        let Some(entry) = self.peers.get_mut(&address) else {
            return InitialOpenOutcome::Failed;
        };
        let mut cache = self.guid_cache.clone();
        let mut exports = entry.exports.clone();
        let world_seconds = self.started.elapsed().as_secs_f32();
        let state = match session.open_state(&session.state, world_seconds) {
            Ok(state) => state,
            Err(_) => {
                bump(&mut self.stats.game_state_open_errors);
                return InitialOpenOutcome::Failed;
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
            Err(error) => {
                bump(&mut self.stats.game_state_open_errors);
                let channel = session.channel_index;
                return self.initial_open_failure(address, &error, channel);
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
        let channel = session.channel_index;
        self.guid_cache = cache;
        if self.send_all(&sent, address) {
            bump(&mut self.stats.game_state_opens_sent);
        }
        self.connected_player_actor_opened(address);
        InitialOpenOutcome::Committed(channel)
    }

    /// Compose and send the PlayerState open the PlayerController references
    /// (F-784), committing GUID, export and host state only once the datagram
    /// is built; a failure counts `player_state_open_errors`.
    fn send_player_state_open(
        &mut self,
        address: SocketAddrV4,
        owner: u32,
        player_state: u32,
    ) -> InitialOpenOutcome {
        let (Some(plan), Some(admission)) = (&self.player_state_open, &self.config.admission)
        else {
            return InitialOpenOutcome::Failed;
        };
        // The original sets bIsHost at the PlayerState's BeginPlay when the
        // first player controller's character is this PlayerState's pawn,
        // which does not exist yet (F-913). On the lab every joined player
        // has a pawn, so a player is host when no other player is joined.
        let is_host = self
            .peers
            .get(&address)
            .and_then(|entry| entry.initial_opens.as_ref())
            .map(|pending| pending.player_state_is_host)
            .unwrap_or_else(|| {
                !self.peers.iter().any(|(other, peer)| {
                    *other != address && peer.admission.state == ClientLoginState::Joined
                })
            });
        let Some(entry) = self.peers.get_mut(&address) else {
            return InitialOpenOutcome::Failed;
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
            Err(error) => {
                bump(&mut self.stats.player_state_open_errors);
                let channel = plan.channel_index;
                return self.initial_open_failure(address, &error, channel);
            }
        };
        entry.player_state_channel = Some(plan.channel_index);
        commit_exports(&mut exports, &sent);
        entry.exports = exports;
        entry.ack_pending = false;
        let channel = plan.channel_index;
        self.guid_cache = cache;
        if self.send_all(&sent, address) {
            bump(&mut self.stats.player_state_opens_sent);
        }
        self.connected_player_actor_opened(address);
        // The RPC carrying this open's capacity-releasing ACK was processed
        // before retries. Begin exactly once now that its dependency exists.
        let begin = self
            .peers
            .get_mut(&address)
            .and_then(|entry| entry.initial_opens.as_mut())
            .and_then(|pending| pending.begin_slot.take());
        if let Some(slot) = begin {
            self.controller_call(address, ControllerCall::Begin(slot));
        }
        InitialOpenOutcome::Committed(channel)
    }

    /// Compose and send world manager `index` (F-859), committing GUID and
    /// export state only once every datagram is built; a failure counts
    /// `manager_open_errors`.
    fn send_manager_open(&mut self, address: SocketAddrV4, index: usize) -> InitialOpenOutcome {
        let Some((key, plan, channel)) = self.manager_opens.get(index) else {
            return InitialOpenOutcome::Failed;
        };
        let Some(entry) = self.peers.get_mut(&address) else {
            return InitialOpenOutcome::Failed;
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
            Err(error) => {
                bump(&mut self.stats.manager_open_errors);
                let channel = *channel;
                return self.initial_open_failure(address, &error, channel);
            }
        };
        commit_exports(&mut exports, &sent);
        entry.exports = exports;
        entry.ack_pending = false;
        self.guid_cache = cache;
        if *key == ManagerSlot::PlayerHistory.key() {
            entry.player_history_channel = Some(*channel);
            entry.player_history_pending = false;
            entry.player_history_retry_ready = false;
        }
        let channel = *channel;
        if self.send_all(&sent, address) {
            bump(&mut self.stats.manager_opens_sent);
        }
        InitialOpenOutcome::Committed(channel)
    }

    /// Compose and send saved quest actor `index` (`C019`, `F-934`),
    /// committing GUID and export state only once every datagram is built; a
    /// failure counts `quest_open_errors`.
    fn send_quest_open(&mut self, address: SocketAddrV4, index: usize) -> InitialOpenOutcome {
        let Some((plan, channel)) = self.quest_opens.get(index) else {
            return InitialOpenOutcome::Failed;
        };
        let Some(entry) = self.peers.get_mut(&address) else {
            return InitialOpenOutcome::Failed;
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
            Err(error) => {
                bump(&mut self.stats.quest_open_errors);
                let channel = *channel;
                return self.initial_open_failure(address, &error, channel);
            }
        };
        commit_exports(&mut exports, &sent);
        entry.exports = exports;
        entry.ack_pending = false;
        let channel = *channel;
        self.guid_cache = cache;
        if self.send_all(&sent, address) {
            bump(&mut self.stats.quest_opens_sent);
        }
        InitialOpenOutcome::Committed(channel)
    }

    fn send_acks(&mut self) {
        let mut failed = Vec::new();
        let mut acks = Vec::new();
        for (address, entry) in &mut self.peers {
            if !entry.ack_pending || entry.ack_capacity_blocked {
                continue;
            }
            match entry.peer.ack_packet() {
                Ok(datagram) => {
                    entry.ack_pending = false;
                    acks.push((*address, datagram));
                }
                Err(error) => {
                    if matches!(
                        error.downcast_ref::<super::sequencing::SendCapacityError>(),
                        Some(super::sequencing::SendCapacityError::SentRecordRing { .. })
                    ) && transport_capacity_can_drain(entry, &error, 0)
                    {
                        // Keep the ACK intent, but wait for packet notification
                        // progress before attempting another allocation.
                        entry.ack_capacity_blocked = true;
                    } else {
                        failed.push(*address);
                    }
                }
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

    /// Send every datagram of an open or reliable update; true when all went out whole.
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

    fn auth_result(&mut self, address: SocketAddrV4, accepted: bool) {
        use super::{
            archive::ByteOrder,
            steam_auth::{SteamAuthMessage, encode_datagram},
        };
        if let Ok(datagram) = encode_datagram(
            &SteamAuthMessage::Result { accepted },
            ByteOrder::Little,
            0,
            0,
        ) {
            self.send(&datagram, address);
        }
    }

    fn poll_authentication(&mut self) {
        let Some(auth) = self.authenticator.clone() else {
            return;
        };
        let Ok(events) = auth.drain_events() else {
            self.shutdown();
            return;
        };
        for event in events {
            let address = self.peers.iter().find_map(|(address, peer)| {
                (peer.auth_request == Some(event.request_id)).then_some(*address)
            });
            let Some(address) = address else { continue };
            if event.outcome == AuthOutcome::Accepted {
                if let Some(peer) = self.peers.get_mut(&address) {
                    peer.auth_accepted = true;
                    peer.last_seen = Instant::now();
                }
                self.auth_result(address, true);
            } else {
                self.auth_result(address, false);
                self.remove_peer(address);
                bump(&mut self.stats.peers_auth_closed);
            }
        }
    }

    fn receive_authentication(&mut self, address: SocketAddrV4, length: usize) {
        use super::{
            archive::ByteOrder,
            steam_auth::{SteamAuthMessage, decode_datagram},
        };
        let Some(peer) = self.peers.get(&address) else {
            return;
        };
        let message = decode_datagram(&self.buffer[..length], ByteOrder::Little, 0, 0);
        if matches!(message, Ok(SteamAuthMessage::RequestResendResult)) {
            if peer.auth_accepted {
                self.auth_result(address, true);
            }
            return;
        }
        let digest: [u8; 32] = Sha256::digest(&self.buffer[..length]).into();
        if peer.auth_ticket_digest == Some(digest) {
            if peer.auth_accepted {
                self.auth_result(address, true);
            }
            return;
        }
        // A connection gets one ticket. Replacing it needs a new cookie/peer;
        // this prevents late validation from applying to a different identity.
        if peer.auth_request.is_some() {
            self.auth_result(address, false);
            self.remove_peer(address);
            bump(&mut self.stats.peers_auth_closed);
            return;
        }
        let attempt = (|| -> Result<(u64, u64)> {
            let SteamAuthMessage::UserData { ticket, steam_id } = message? else {
                bail!("unexpected client authentication message");
            };
            let bytes = decode_ticket_hex(&ticket)?;
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_secs();
            let verified = VerifiedTicket::verify(&bytes, steam_id, ICARUS_APP_ID, now)?;
            let auth = self
                .authenticator
                .as_ref()
                .context("authentication unavailable")?;
            let request = auth.allocate_request_id()?;
            auth.begin(request, verified)?;
            Ok((request, steam_id))
        })();
        match attempt {
            Ok((request, identity)) => {
                if let Some(peer) = self.peers.get_mut(&address) {
                    peer.auth_request = Some(request);
                    peer.auth_identity = Some(identity);
                    peer.auth_ticket_digest = Some(digest);
                }
            }
            Err(_) => {
                self.auth_result(address, false);
                self.remove_peer(address);
                bump(&mut self.stats.peers_auth_closed);
            }
        }
    }

    /// One bounded update: expire idle peers, process at most
    /// [`PACKETS_PER_UPDATE`] datagrams, then send pending acks. The
    /// [`PeerLabPlugin`] runs this once per app update.
    pub fn poll(&mut self) {
        self.poll_authentication();
        let now = Instant::now();
        self.expire_idle(now);
        self.expire_stalled_delivery(now);
        self.expire_friends_queries(now);
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
            let authentication =
                self.authenticator.is_some() && length > 0 && self.buffer[0] & 3 == 2;
            let limit = if authentication {
                super::steam_auth::MAX_DATAGRAM_BYTES
            } else {
                max
            };
            if length > limit {
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
            if authentication {
                self.receive_authentication(address, length);
            } else {
                self.data(address, length, now);
            }
        }
        self.retry_control_replies();
        self.retry_initial_opens();
        self.retry_controller_possession();
        self.close_abandoned_possessions();
        self.retry_controller_startup();
        self.retry_friends_queries();
        self.retry_pending_profile_exchanges();
        self.retry_pending_pawn_restores();
        self.finalise_connected_players();
        self.retry_connected_player_records();
        self.retry_frozen_movement_release();
        self.retry_pawn_move_acks(Instant::now());
        self.retry_pending_player_history();
        self.update_pawn_properties();
        self.send_acks();
        self.update_game_state();
        // Lowest priority: after every other pass of this poll, bounded.
        self.retry_level_actor_opens();
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

/// A returning pawn's restored stats (F-988) from its opened inventories,
/// their restores and its restored modifiers; `None` when an inventory's
/// weight is not modelled.
fn restored_pawn_stats(
    tables: &StatRestoreTables,
    inventory_tables: &InventoryRestoreTables,
    opened: &[PawnInventory],
    restores: &[InventoryRestore],
    linked: &BTreeMap<i32, Vec<u16>>,
    modifiers: &[RestoredModifier],
) -> Result<Option<RestoredStats>> {
    let mut slots: Vec<&[InventorySlot]> = opened
        .iter()
        .map(|inventory| inventory.slots.items.as_slice())
        .collect();
    let mut weight: i32 = 0;
    let mut weights: Vec<Option<i32>> = opened
        .iter()
        .map(|inventory| {
            Some(
                inventory
                    .current_weight
                    .unwrap_or(DEFAULT_CDO_CURRENT_WEIGHT),
            )
        })
        .collect();
    for restore in restores {
        let index = restore.inventory;
        *slots
            .get_mut(index)
            .context("a restore names no inventory")? = &restore.restored.slots.items;
        weights[index] = restore.weight;
    }
    for inventory_weight in weights {
        let Some(inventory_weight) = inventory_weight else {
            return Ok(None);
        };
        weight = weight.wrapping_add(inventory_weight);
    }
    let inputs = PawnStatInputs {
        inventories: inventory_tables
            .inventories
            .iter()
            .zip(slots)
            .map(|(info, slots)| (info.role, slots))
            .collect(),
        linked,
        modifiers: modifiers
            .iter()
            .map(|modifier| (modifier.row, modifier.final_effectiveness))
            .collect(),
        weight,
    };
    Ok(Some(restore_stats(tables, inventory_tables, &inputs)?))
}

impl Drop for PeerLab {
    fn drop(&mut self) {
        if let Some(auth) = &self.authenticator {
            for peer in self.peers.values() {
                if let Some(request) = peer.auth_request {
                    let _ = auth.end(request);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pending_auth_peer() -> (
        PeerLab,
        UdpSocket,
        SocketAddrV4,
        crate::steam_service::TestBackend,
    ) {
        use super::super::handshake::initial_request;
        let (auth, backend) = AuthClient::test_bridge(8);
        let mut lab = PeerLab::bind_authenticated(
            SocketAddrV4::new(std::net::Ipv4Addr::LOCALHOST, 0),
            PeerLabConfig {
                local_network_version: Some(4087436304),
                admission: Some(PeerLabAdmission::default()),
                ..PeerLabConfig::default()
            },
            auth,
        )
        .unwrap();
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        socket.connect(lab.local_addr().unwrap()).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        let SocketAddr::V4(address) = socket.local_addr().unwrap() else {
            unreachable!()
        };
        let mut bytes = [0; 2048];
        socket.send(&initial_request()).unwrap();
        lab.poll();
        let count = socket.recv(&mut bytes).unwrap();
        socket.send(&bytes[..count]).unwrap();
        lab.poll();
        socket.recv(&mut bytes).unwrap();
        let peer = lab.peers.get_mut(&address).unwrap();
        // The signed-ticket validator is tested in steam_auth. This helper
        // starts at the asynchronous validation boundary, without credentials.
        peer.auth_request = Some(1);
        peer.auth_identity = Some(0x0110_0001_0000_002a);
        (lab, socket, address, backend)
    }

    #[test]
    fn authentication_gates_packets_and_revocation_removes_the_session() {
        use super::super::{
            archive::ByteOrder,
            steam_auth::{SteamAuthMessage, decode_datagram},
        };
        use crate::steam_auth::AuthEvent;
        let (mut lab, socket, address, backend) = pending_auth_peer();
        let header = lab.peers[&address].peer.sequencer().header().unwrap();
        let failures = lab.stats.peers_failed;
        // Even malformed game input cannot advance or fail the packet peer
        // before Steam acceptance.
        lab.buffer[0] = 0;
        lab.data(address, 1, Instant::now());
        assert_eq!(lab.stats.peers_failed, failures);
        assert_eq!(
            lab.peers[&address].peer.sequencer().header().unwrap(),
            header
        );
        backend
            .event(AuthEvent {
                request_id: 1,
                outcome: AuthOutcome::Accepted,
            })
            .unwrap();
        lab.poll();
        assert!(lab.peers[&address].auth_accepted);
        let mut bytes = [0; 128];
        let count = socket.recv(&mut bytes).unwrap();
        assert_eq!(
            decode_datagram(&bytes[..count], ByteOrder::Little, 0, 0).unwrap(),
            SteamAuthMessage::Result { accepted: true }
        );
        assert_eq!(
            lab.peers[&address].peer.sequencer().header().unwrap(),
            header
        );
        backend
            .event(AuthEvent {
                request_id: 1,
                outcome: AuthOutcome::Revoked,
            })
            .unwrap();
        lab.poll();
        assert!(!lab.has_peer(address));
        assert_eq!(backend.ended_requests(), [1]);
        assert_eq!(lab.session_counts().closed_by_authentication, 1);
        backend
            .event(AuthEvent {
                request_id: 1,
                outcome: AuthOutcome::Accepted,
            })
            .unwrap();
        lab.poll();
        assert!(!lab.has_peer(address));
    }

    #[test]
    fn authentication_has_its_own_datagram_bound_without_enlarging_game_packets() {
        use super::super::{
            archive::ByteOrder,
            steam_auth::{MAX_DATAGRAM_BYTES, SteamAuthMessage, decode_datagram, encode_datagram},
        };
        let (mut lab, socket, address, _backend) = pending_auth_peer();
        lab.peers.get_mut(&address).unwrap().auth_request = None;
        // Both input families retain truncation detection and independent caps.
        socket
            .send(&vec![0; lab.config.max_datagram_bytes() + 1])
            .unwrap();
        let mut oversized_auth = vec![0; MAX_DATAGRAM_BYTES + 2];
        oversized_auth[0] = 2;
        socket.send(&oversized_auth).unwrap();
        lab.poll();
        assert_eq!(lab.stats.oversize, 2);
        assert!(lab.has_peer(address));
        let auth = encode_datagram(
            &SteamAuthMessage::UserData {
                ticket: vec![u16::from(b'0'); 1024],
                steam_id: 0x0110_0001_0000_002a,
            },
            ByteOrder::Little,
            0,
            0,
        )
        .unwrap();
        assert!(auth.len() > lab.config.max_datagram_bytes());
        socket.send(&auth).unwrap();
        lab.poll();
        // The whole auth frame reaches verification. Its authored bogus ticket
        // must be rejected explicitly, rather than silently discarded as large.
        assert_eq!(lab.stats.oversize, 2);
        assert!(!lab.has_peer(address));
        assert_eq!(lab.session_counts().closed_by_authentication, 1);
        let mut reply = [0; 128];
        let count = socket.recv(&mut reply).unwrap();
        assert_eq!(
            decode_datagram(&reply[..count], ByteOrder::Little, 0, 0).unwrap(),
            SteamAuthMessage::Result { accepted: false }
        );
    }

    #[test]
    fn authentication_resends_wait_for_acceptance_without_starting_another_request() {
        use super::super::{
            archive::ByteOrder,
            steam_auth::{SteamAuthMessage, encode_datagram},
        };
        for accepted in [false, true] {
            let (mut lab, socket, address, backend) = pending_auth_peer();
            socket.set_nonblocking(true).unwrap();
            let ticket = encode_datagram(
                &SteamAuthMessage::UserData {
                    ticket: vec![u16::from(b'0'); 2],
                    steam_id: 0x0110_0001_0000_002a,
                },
                ByteOrder::Little,
                0,
                0,
            )
            .unwrap();
            let peer = lab.peers.get_mut(&address).unwrap();
            peer.auth_ticket_digest = Some(Sha256::digest(&ticket).into());
            peer.auth_accepted = accepted;
            let resend = encode_datagram(
                &SteamAuthMessage::RequestResendResult,
                ByteOrder::Little,
                0,
                0,
            )
            .unwrap();
            let expected = encode_datagram(
                &SteamAuthMessage::Result { accepted: true },
                ByteOrder::Little,
                0,
                0,
            )
            .unwrap();
            for datagram in [&resend, &ticket] {
                socket.send(datagram).unwrap();
                lab.poll();
                let mut reply = [0; 128];
                if accepted {
                    let count = socket.recv(&mut reply).unwrap();
                    assert_eq!(&reply[..count], expected);
                } else {
                    assert_eq!(
                        socket.recv(&mut reply).unwrap_err().kind(),
                        io::ErrorKind::WouldBlock
                    );
                }
                assert!(lab.has_peer(address));
                assert_eq!(lab.peers[&address].auth_request, Some(1));
                assert_eq!(lab.session_counts().closed_by_authentication, 0);
                // This also rejects an unexpected Begin command.
                assert!(backend.ended_requests().is_empty());
            }
        }
    }

    #[test]
    fn a_second_authentication_ticket_closes_once_and_ignores_late_acceptance() {
        use super::super::{
            archive::ByteOrder,
            steam_auth::{SteamAuthMessage, encode_datagram},
        };
        for accepted in [false, true] {
            let (mut lab, socket, address, backend) = pending_auth_peer();
            let old = encode_datagram(
                &SteamAuthMessage::UserData {
                    ticket: vec![u16::from(b'0'); 2],
                    steam_id: 0x0110_0001_0000_002a,
                },
                ByteOrder::Little,
                0,
                0,
            )
            .unwrap();
            let replacement = encode_datagram(
                &SteamAuthMessage::UserData {
                    ticket: vec![u16::from(b'1'); 2],
                    steam_id: 0x0110_0001_0000_002b,
                },
                ByteOrder::Little,
                0,
                0,
            )
            .unwrap();
            let peer = lab.peers.get_mut(&address).unwrap();
            peer.auth_ticket_digest = Some(Sha256::digest(&old).into());
            peer.auth_accepted = accepted;
            socket.send(&replacement).unwrap();
            lab.poll();
            let mut reply = [0; 128];
            let count = socket.recv(&mut reply).unwrap();
            assert_eq!(
                &reply[..count],
                encode_datagram(
                    &SteamAuthMessage::Result { accepted: false },
                    ByteOrder::Little,
                    0,
                    0,
                )
                .unwrap()
            );
            assert!(!lab.has_peer(address));
            assert_eq!(lab.session_counts().closed_by_authentication, 1);
            assert_eq!(backend.ended_requests(), [1]);
            backend
                .event(crate::steam_auth::AuthEvent {
                    request_id: 1,
                    outcome: AuthOutcome::Accepted,
                })
                .unwrap();
            lab.poll();
            assert!(!lab.has_peer(address));
            assert_eq!(lab.session_counts().closed_by_authentication, 1);
            assert!(backend.ended_requests().is_empty());
        }
    }

    #[test]
    fn negative_authentication_outcomes_are_counted_once() {
        for outcome in [
            AuthOutcome::Rejected,
            AuthOutcome::Revoked,
            AuthOutcome::TimedOut,
            AuthOutcome::Expired,
            AuthOutcome::Disconnected,
        ] {
            let (mut lab, _socket, address, backend) = pending_auth_peer();
            let event = crate::steam_auth::AuthEvent {
                request_id: 1,
                outcome,
            };
            backend.event(event).unwrap();
            lab.poll();
            assert!(!lab.has_peer(address));
            assert_eq!(lab.session_counts().closed_by_authentication, 1);
            assert_eq!(backend.ended_requests(), [1]);
            backend.event(event).unwrap();
            lab.poll();
            assert_eq!(lab.session_counts().closed_by_authentication, 1);
            assert!(backend.ended_requests().is_empty());
        }
    }

    #[test]
    fn service_failure_and_drop_release_pending_sessions() {
        let (mut lab, _socket, address, backend) = pending_auth_peer();
        backend.fail();
        lab.poll();
        assert!(!lab.has_peer(address));
        assert!(!lab.is_admitting());
        let (lab, _socket, _address, backend) = pending_auth_peer();
        drop(lab);
        assert_eq!(backend.ended_requests(), [1]);
    }

    #[test]
    fn stale_acceptance_cannot_authenticate_a_replacement_request() {
        let (mut lab, _socket, address, backend) = pending_auth_peer();
        lab.peers.get_mut(&address).unwrap().auth_request = Some(2);
        backend
            .event(crate::steam_auth::AuthEvent {
                request_id: 1,
                outcome: AuthOutcome::Accepted,
            })
            .unwrap();
        lab.poll();
        assert!(!lab.peers[&address].auth_accepted);
    }

    #[test]
    fn login_identity_must_match_the_accepted_ticket() {
        use super::super::control_messages::{NetIdType, NetIdValue, UniqueNetId};
        let (mut lab, _socket, address, _backend) = pending_auth_peer();
        lab.peers.get_mut(&address).unwrap().auth_accepted = true;
        let identity = lab.peers[&address].auth_identity.unwrap();
        let login = |kind, id: u64| ControlMessage::Login {
            response: ArchiveText::text("").unwrap(),
            url: ArchiveText::text("/Game/Maps/Entry?Name=Authored").unwrap(),
            unique_net_id: UniqueNetId {
                kind,
                value: NetIdValue::Text(ArchiveText::text(&id.to_string()).unwrap()),
            },
            platform: ArchiveText::text("Steam").unwrap(),
        };
        for message in [
            login(NetIdType::Indexed(3), identity + 1),
            login(NetIdType::Default, identity),
        ] {
            assert!(matches!(
                lab.reply_for(address, &message),
                Some(Reply::Failure("Steam authentication identity mismatch"))
            ));
        }
        // Matching identity crosses the auth guard; any subsequent admission
        // decision belongs to the separately tested game admission policy.
        let before = lab.stats.logins_received;
        let _ = lab.reply_for(address, &login(NetIdType::Indexed(3), identity));
        assert_eq!(lab.stats.logins_received, before + 1);
    }

    #[test]
    fn incarnation_registration_validates_whole_map_and_retains_high_water_marks() {
        let mut lab = PeerLab::bind(0, PeerLabConfig::default()).unwrap();
        let desired = BTreeMap::from([(10, 3), (20, 2)]);
        lab.reconcile_actor_incarnations(&desired).unwrap();
        for invalid in [
            BTreeMap::from([(10, 4), (20, 0)]),
            BTreeMap::from([(10, 4), (20, 1)]),
        ] {
            assert!(lab.reconcile_actor_incarnations(&invalid).is_err());
            assert_eq!(lab.desired_incarnations, desired);
            assert_eq!(lab.highest_incarnations, desired);
        }
        lab.reconcile_actor_incarnations(&BTreeMap::new()).unwrap();
        assert!(
            lab.reconcile_actor_incarnations(&BTreeMap::from([(10, 2)]))
                .is_err()
        );
        lab.reconcile_actor_incarnations(&BTreeMap::from([(10, 4)]))
            .unwrap();
        assert_eq!(lab.highest_incarnations.get(&20), Some(&2));
    }

    #[test]
    fn incarnation_slots_are_cumulatively_bounded_and_unused_guids_are_pruned() {
        let mut lab = PeerLab::bind(0, PeerLabConfig::default()).unwrap();
        let desired: BTreeMap<_, _> = (0..1024).map(|slot| (slot, 1)).collect();
        lab.reconcile_actor_incarnations(&desired).unwrap();
        let first = lab.guid_cache.incarnation_guid(0, 1).unwrap();
        lab.reconcile_actor_incarnations(&desired).unwrap();
        assert_eq!(lab.guid_cache.incarnation_count(), 1);
        lab.reconcile_actor_incarnations(&BTreeMap::new()).unwrap();
        assert_eq!(lab.guid_cache.incarnation_count(), 0);
        assert!(
            lab.reconcile_actor_incarnations(&BTreeMap::from([(1024, 1)]))
                .is_err()
        );
        assert!(lab.desired_incarnations.is_empty());
        assert_eq!(lab.highest_incarnations.len(), 1024);
        lab.reconcile_actor_incarnations(&BTreeMap::from([(0, 2)]))
            .unwrap();
        let replacement = lab.guid_cache.incarnation_guid(0, 2).unwrap();
        assert_ne!(first, replacement);
        assert_eq!(lab.retiring_incarnation_count(), 0);
    }

    #[test]
    fn controller_configuration_rejects_reserved_channel_before_guid_registration() {
        let mut lab = prospect_refresh_lab();
        // Retain an independently reserved GameState channel while removing
        // only the controller fixture, so the configure-once guard cannot
        // mask channel validation.
        let original = lab.player_controller_open.take().unwrap();
        let reserved = lab.game_state_open.as_ref().unwrap().channel_index;
        let mut colliding = original.clone();
        colliding.channel_index = reserved;
        colliding.profile.level[0].path.push_str("Collision");
        let before = format!("{:?}", lab.guid_cache);
        let game_state = lab.game_state_open.as_ref().unwrap().plan.clone();
        assert!(lab.enable_player_controller_open(colliding).is_err());
        assert!(lab.player_controller_open.is_none());
        assert_eq!(format!("{:?}", lab.guid_cache), before);
        assert_eq!(
            format!("{:?}", lab.game_state_open.as_ref().unwrap().plan),
            format!("{:?}", game_state)
        );
        assert_eq!(
            lab.game_state_open.as_ref().unwrap().channel_index,
            reserved
        );
        lab.enable_player_controller_open(original.clone()).unwrap();
        assert_eq!(
            format!("{:?}", lab.player_controller_open.as_ref().unwrap()),
            format!("{:?}", original)
        );
        assert_eq!(format!("{:?}", lab.guid_cache), before);
    }

    #[test]
    fn controller_configuration_rejects_replacement_without_guid_mutation() {
        let mut lab = prospect_refresh_lab();
        let mut replacement = lab.player_controller_open.clone().unwrap();
        replacement.profile.level[0].path.push_str("Replacement");
        let before = format!("{:?}", lab.guid_cache);
        let original = lab.player_controller_open.clone().unwrap();
        assert!(lab.enable_player_controller_open(replacement).is_err());
        assert_eq!(format!("{:?}", lab.guid_cache), before);
        assert_eq!(
            format!("{:?}", lab.player_controller_open.as_ref().unwrap()),
            format!("{:?}", original)
        );
    }

    #[test]
    fn manager_configuration_rejects_duplicates_and_replacement_atomically() {
        use super::super::release::ReleaseProfile;
        let mut lab = prospect_refresh_lab();
        let plans = ReleaseProfile::release_3_0_32()
            .manager_open_plans(PeerLabAdmission::default().map_url)
            .unwrap();
        let before = lab.game_state_open.as_ref().unwrap().state.managers.clone();
        let quest = QuestOpenPlan {
            key: plans[0].0.key(),
            archetype: Vec::new(),
            level: Vec::new(),
            location: None,
            layout: Vec::new(),
            actor_state_layout: Vec::new(),
            stat_container_checksum: 0,
            actor_state_checksum: 0,
            values: BTreeMap::new(),
            references: BTreeMap::new(),
            engine_net_version: 0,
        };
        lab.quest_opens.push((quest, 50));
        assert!(lab.enable_manager_opens(plans.clone(), 20).is_err());
        assert!(lab.manager_opens.is_empty());
        assert_eq!(lab.quest_opens.len(), 1);
        assert_eq!(lab.game_state_open.as_ref().unwrap().state.managers, before);
        lab.quest_opens.clear();
        assert!(
            lab.enable_manager_opens(vec![plans[0].clone(), plans[0].clone()], 20)
                .is_err()
        );
        assert!(lab.manager_opens.is_empty());
        assert_eq!(lab.game_state_open.as_ref().unwrap().state.managers, before);
        lab.enable_manager_opens(plans.clone(), 20).unwrap();
        assert_eq!(lab.manager_opens.len(), plans.len());
        let configured = lab.game_state_open.as_ref().unwrap().state.managers.clone();
        assert!(lab.enable_manager_opens(Vec::new(), 40).is_err());
        assert_eq!(lab.manager_opens.len(), plans.len());
        assert_eq!(
            lab.game_state_open.as_ref().unwrap().state.managers,
            configured
        );
    }

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

    /// Authored wire inputs with a caller-controlled receive clock. The
    /// socket and normal data handler are real; only elapsed time is injected.
    fn receive_authored_at(lab: &mut PeerLab, socket: &UdpSocket, bytes: &[u8], now: Instant) {
        socket.send(bytes).unwrap();
        let (length, source) = lab.socket.recv_from(&mut lab.buffer).unwrap();
        let SocketAddr::V4(address) = source else {
            unreachable!()
        };
        lab.data(address, length, now);
    }

    #[test]
    fn poll_removes_stalled_delivery_and_reports_session_counter() {
        let mut lab = prospect_refresh_lab();
        let (_socket, address, _) = acked_game_state_peer(&mut lab);
        let timeout = lab.config.idle_timeout;
        let entry = lab.peers.get_mut(&address).unwrap();
        entry.peer.ack_packet().unwrap();
        entry.delivery_progress_at = Some(Instant::now() - timeout - Duration::from_secs(1));
        entry.last_seen = Instant::now();

        lab.poll();

        assert!(!lab.has_peer(address));
        assert_eq!(lab.stats.peers_expired, 0);
        assert_eq!(lab.stats.peers_delivery_stalled, 1);
        let status = serde_json::to_value(lab.session_counts()).unwrap();
        assert_eq!(status["closed_by_delivery_stall"], 1);
        lab.poll();
        assert_eq!(lab.stats.peers_delivery_stalled, 1);
    }

    #[test]
    fn dropped_challenge_expires_delivery_despite_live_reliable_retransmissions() {
        use super::super::{
            control_messages::{ControlCodec, encode_control_message},
            handshake::initial_request,
            packet::{BitWriter, Bunch, BunchHeader, Name, Packet, write_packet},
            packet_lab::client_cookie_seeds,
            sequencing::{PacketSequencer, wire_reliable_sequence},
        };

        let timeout = Duration::from_secs(60);
        let mut config = PeerLabConfig {
            admission: Some(PeerLabAdmission::default()),
            idle_timeout: timeout,
            ..Default::default()
        };
        config.packet.sent_capacity = 1;
        let mut lab = PeerLab::bind(0, config).unwrap();
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        socket.connect(lab.local_addr().unwrap()).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        socket.send(&initial_request()).unwrap();
        lab.poll();
        let mut bytes = [0; 2048];
        let length = socket.recv(&mut bytes).unwrap();
        let cookie = decode(&bytes[..length]).unwrap().cookie;
        socket.send(&bytes[..length]).unwrap();
        lab.poll();
        socket.recv(&mut bytes).unwrap();
        let SocketAddr::V4(address) = socket.local_addr().unwrap() else {
            unreachable!()
        };
        let (incoming, outgoing) = client_cookie_seeds(&cookie);
        let mut client = PacketSequencer::new(incoming, outgoing, 8).unwrap();
        let mut control = client.new_channel();
        let hello = ControlMessage::Hello {
            first_field: 1,
            version: 1,
            token: ArchiveText::text("").unwrap(),
        };
        let sequence = control
            .send_reliable(client.out_packet_id(), hello.clone())
            .unwrap();
        let mut payload = BitWriter::new();
        payload.write_bytes(&encode_control_message(&hello, &ControlCodec::default()).unwrap());
        let hello_bunch = Bunch {
            header: BunchHeader {
                control: true,
                open: true,
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
        };
        let start = Instant::now();
        for elapsed in [Duration::ZERO, timeout, timeout + Duration::from_millis(1)] {
            let packet = write_packet(
                &Packet {
                    magic: 0,
                    header: client.header().unwrap(),
                    info: None,
                    bunches: vec![hello_bunch.clone()],
                },
                &lab.config.packet.packet,
            )
            .unwrap();
            client.commit().unwrap();
            let now = start + elapsed;
            receive_authored_at(&mut lab, &socket, &packet, now);
            lab.send_acks();
            if elapsed.is_zero() {
                // The Challenge is deliberately lost to the client sequencing
                // model. No later server sequence can fit this one-record ring.
                socket.recv(&mut bytes).unwrap();
            }
            assert_eq!(lab.peers[&address].last_seen, now);
            lab.expire_idle(now);
            assert!(
                lab.has_peer(address),
                "fresh client traffic prevents idle expiry"
            );
            lab.expire_stalled_delivery(now);
            assert_eq!(lab.has_peer(address), elapsed <= timeout);
        }
        assert_eq!(lab.stats.control_challenges_sent, 1);
        assert_eq!(lab.stats.data_accepted, 3);
        assert_eq!(lab.stats.peers_expired, 0);
        assert_eq!(lab.stats.peers_failed, 0);
        assert_eq!(lab.stats.peers_delivery_stalled, 1);
        assert_eq!(lab.session_counts().closed_by_delivery_stall, 1);
        lab.expire_stalled_delivery(start + timeout * 2);
        assert_eq!(lab.stats.peers_delivery_stalled, 1);
    }

    #[test]
    fn delivery_notifications_reset_deadline_and_empty_queues_clear_it() {
        use super::super::packet::{Packet, Receiver, read_packet, write_packet};

        for delivered in [true, false] {
            let mut lab = prospect_refresh_lab();
            let (socket, address, mut client) = acked_game_state_peer(&mut lab);
            let timeout = lab.config.idle_timeout;
            let start = Instant::now();
            // A real reliable control packet and a newer ACK packet allow the
            // client to acknowledge the first, or skip it and report its NAK.
            let first = lab
                .peers
                .get_mut(&address)
                .unwrap()
                .peer
                .reliable_control_packet(ControlMessage::Challenge {
                    token: ArchiveText::text("authored").unwrap(),
                })
                .unwrap();
            let newer = lab
                .peers
                .get_mut(&address)
                .unwrap()
                .peer
                .ack_packet()
                .unwrap();
            lab.expire_stalled_delivery(start);
            let mut to_client = lab.config.packet.packet;
            to_client.receiver = Receiver::Client;
            if delivered {
                let packet = read_packet(&first, &to_client).unwrap();
                let received = client.receive(&packet.header).unwrap().unwrap();
                client.ack_seq(received.sequence, true).unwrap();
            }
            let packet = read_packet(&newer, &to_client).unwrap();
            let received = client.receive(&packet.header).unwrap().unwrap();
            client.ack_seq(received.sequence, true).unwrap();
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
            let progress = start + timeout;
            receive_authored_at(&mut lab, &socket, &ack, progress);
            assert_eq!(lab.peers[&address].delivery_progress_at, Some(progress));
            assert!(
                lab.peers[&address]
                    .peer
                    .sequencer()
                    .notify()
                    .sent_records()
                    .is_empty()
            );
            assert_eq!(
                lab.peers[&address].peer.pending_resends(),
                usize::from(!delivered)
            );

            // Send a fresh tracked ACK/repair, so the new progress deadline is
            // exercised rather than merely observing an empty ring.
            lab.send_acks();
            lab.expire_stalled_delivery(start + timeout + Duration::from_millis(1));
            assert!(
                lab.has_peer(address),
                "ACK and NAK both extend a live delivery deadline"
            );
            let mut buffer = [0; 2048];
            let length = socket.recv(&mut buffer).unwrap();
            let packet = read_packet(&buffer[..length], &to_client).unwrap();
            let received = client.receive(&packet.header).unwrap().unwrap();
            client.ack_seq(received.sequence, true).unwrap();
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
            receive_authored_at(&mut lab, &socket, &ack, progress + Duration::from_millis(2));
            lab.expire_stalled_delivery(progress + timeout * 100);
            assert!(
                lab.has_peer(address),
                "no outstanding delivery means no delivery timeout"
            );
            assert_eq!(lab.peers[&address].delivery_progress_at, None);
            assert_eq!(lab.stats.peers_delivery_stalled, 0);
            // A new nonempty epoch does not inherit the old deadline.
            lab.peers
                .get_mut(&address)
                .unwrap()
                .peer
                .ack_packet()
                .unwrap();
            let fresh = progress + timeout * 101;
            lab.expire_stalled_delivery(fresh);
            lab.expire_stalled_delivery(fresh + timeout);
            assert!(lab.has_peer(address), "new traffic gets the full timeout");
            lab.expire_stalled_delivery(fresh + timeout + Duration::from_millis(1));
            assert!(!lab.has_peer(address));
            assert_eq!(lab.stats.peers_delivery_stalled, 1);
        }
    }

    #[test]
    fn readiness_snapshot_counts_real_transport_settlement_and_partitions_player_phases() {
        use super::super::profile_exchange::ProfileExchange;

        let mut lab = prospect_refresh_lab();
        let (_socket, address, _) = acked_game_state_peer(&mut lab);
        let settled = lab.readiness_counts();
        assert_eq!(settled.actor_channels_open, 1);
        assert_eq!(settled.actor_channels_reliable_settled, 1);
        assert_eq!(settled.actor_reliable_pending, 0);
        assert_eq!(settled.sent_packets_pending, 0);
        let entry = lab.peers.get_mut(&address).unwrap();
        entry.player = Some(RegisteredPlayer {
            player_id: 1,
            name_units: "PRIVATE-READINESS-NAME".encode_utf16().collect(),
        });
        assert_eq!(lab.readiness_counts().players_awaiting_begin, 1);

        // A Begin retained behind PlayerState is distinct from no Begin.
        lab.peers.get_mut(&address).unwrap().initial_opens = Some(PendingInitialOpens {
            remaining: VecDeque::from([InitialOpenStage::PlayerState]),
            controller_guids: Some((2, 4)),
            player_state_is_host: false,
            begin_slot: Some(17),
            retry_ready: false,
        });
        let waiting = lab.readiness_counts();
        assert_eq!(waiting.pending_initial_opens, 1);
        assert_eq!(waiting.pending_initial_stages, 1);
        assert_eq!(waiting.players_awaiting_begin, 0);
        assert_eq!(waiting.players_begin_waiting_for_player_state, 1);
        lab.peers.get_mut(&address).unwrap().initial_opens = None;

        let cases = [
            (
                ProfileExchange::default(),
                Some(PendingProfileExchange::Request(ProfileKind::User)),
                "players_awaiting_user_profile",
            ),
            (
                ProfileExchange {
                    outstanding: Some(ProfileKind::User),
                    ..Default::default()
                },
                None,
                "players_awaiting_user_profile",
            ),
            (
                ProfileExchange::default(),
                Some(PendingProfileExchange::Publish(ProfileKind::User, vec![])),
                "players_awaiting_user_profile",
            ),
            (
                ProfileExchange {
                    outstanding: Some(ProfileKind::Character),
                    ..Default::default()
                },
                None,
                "players_awaiting_character_profile",
            ),
            (
                ProfileExchange::default(),
                Some(PendingProfileExchange::Publish(
                    ProfileKind::Character,
                    vec![],
                )),
                "players_awaiting_character_profile",
            ),
            (
                ProfileExchange::default(),
                Some(PendingProfileExchange::Initialisation),
                "players_awaiting_initialisation",
            ),
            (
                ProfileExchange {
                    initialisation_requested: true,
                    has_loadout: true,
                    ..Default::default()
                },
                None,
                "players_awaiting_initialisation",
            ),
            (
                ProfileExchange {
                    initialisation_requested: true,
                    has_loadout: true,
                    has_bestiary: true,
                    ..Default::default()
                },
                Some(PendingProfileExchange::Complete),
                "players_awaiting_completion",
            ),
            (
                ProfileExchange {
                    completed: true,
                    ..Default::default()
                },
                None,
                "players_awaiting_finalisation",
            ),
            (
                ProfileExchange {
                    completed: true,
                    finalised: true,
                    ..Default::default()
                },
                None,
                "players_finalised",
            ),
        ];
        for (profiles, pending, expected_phase) in cases {
            let entry = lab.peers.get_mut(&address).unwrap();
            entry.character_slot = Some(17);
            entry.profiles = profiles;
            let pending_count = usize::from(pending.is_some());
            entry.pending_profile_exchange = pending;
            let snapshot = lab.readiness_counts();
            assert_eq!(
                snapshot,
                lab.readiness_counts(),
                "snapshot must not consume phases"
            );
            assert_eq!(snapshot.pending_profile_exchanges, pending_count);
            let value = serde_json::to_value(&snapshot).unwrap();
            assert_eq!(value[expected_phase], 1, "{expected_phase}");
            let phase_sum: u64 = value
                .as_object()
                .unwrap()
                .iter()
                .filter(|(key, _)| key.starts_with("players_"))
                .map(|(_, count)| count.as_u64().unwrap())
                .sum();
            assert_eq!(phase_sum, 1, "primary player occupies exactly one phase");
            let text = serde_json::to_string(&snapshot).unwrap();
            assert!(!text.contains("PRIVATE-READINESS-NAME"));
            assert!(!text.contains(&address.to_string()));
        }
        assert_eq!(
            lab.stats.profile_user_requests_sent, 0,
            "diagnostics never drives exchange"
        );
        let channel = lab.game_state_open.as_ref().unwrap().channel_index;
        lab.peers
            .get_mut(&address)
            .unwrap()
            .peer
            .reliable_actor_bunch(
                channel,
                Payload::from_writer(super::super::packet::BitWriter::new()),
            )
            .unwrap();
        let outstanding = lab.readiness_counts();
        assert_eq!(outstanding.actor_channels_open, 1);
        assert_eq!(outstanding.actor_channels_reliable_settled, 0);
        assert_eq!(outstanding.actor_reliable_pending, 1);
        assert_eq!(outstanding.sent_packets_pending, 1);
        lab.remove_peer(address).unwrap();
        assert_eq!(lab.readiness_counts(), ReadinessCounts::default());
    }

    #[test]
    fn readiness_initialisation_blockers_follow_admission_and_each_answer() {
        let mut lab = prospect_refresh_lab();
        lab.config.packet.sent_capacity = 8;
        let (socket, address, mut client) = acked_game_state_peer(&mut lab);
        lab.game_state_open = None;
        assert!(lab.send_player_controller_open(address));
        ack_startup_packets(&mut lab, &socket, &mut client);
        let entry = lab.peers.get_mut(&address).unwrap();
        entry.player = Some(RegisteredPlayer {
            player_id: 1,
            name_units: "AUTHORED-BLOCKERS".encode_utf16().collect(),
        });
        entry.profiles.character_slot = Some(17);
        assert_eq!(
            lab.readiness_counts().initialisation_blockers,
            InitialisationBlockers::default()
        );
        // An answer before its request is admitted is ignored and creates no
        // missing-response observation.
        lab.controller_call(
            address,
            ControllerCall::Initialisation(InitialisationAnswer::Loadout),
        );
        assert!(!lab.peers[&address].profiles.has_loadout);
        let capacity = lab.peers[&address].peer.config().sent_capacity;
        for _ in 0..capacity {
            let datagram = lab
                .peers
                .get_mut(&address)
                .unwrap()
                .peer
                .ack_packet()
                .unwrap();
            assert!(lab.send(&datagram, address));
        }
        lab.request_initialisation(address);
        assert!(!lab.peers[&address].profiles.initialisation_requested);
        let deferred = lab.readiness_counts();
        assert_eq!(
            deferred.initialisation_blockers,
            InitialisationBlockers {
                requests_pending_queue: 1,
                ..Default::default()
            }
        );
        for _ in 0..4 {
            lab.retry_pending_profile_exchanges();
            assert_eq!(lab.readiness_counts(), deferred);
        }
        ack_startup_packets(&mut lab, &socket, &mut client);
        lab.retry_pending_profile_exchanges();
        assert!(lab.peers[&address].profiles.initialisation_requested);
        assert_eq!(
            lab.readiness_counts().initialisation_blockers,
            InitialisationBlockers {
                requests_pending_queue: 0,
                awaiting_loadout_response: 1,
                awaiting_bestiary_response: 1,
            }
        );
        lab.controller_call(
            address,
            ControllerCall::Initialisation(InitialisationAnswer::Bestiary),
        );
        assert_eq!(
            lab.readiness_counts().initialisation_blockers,
            InitialisationBlockers {
                awaiting_loadout_response: 1,
                ..Default::default()
            }
        );
        lab.controller_call(
            address,
            ControllerCall::Initialisation(InitialisationAnswer::Bestiary),
        );
        assert_eq!(
            lab.readiness_counts()
                .initialisation_blockers
                .awaiting_loadout_response,
            1
        );
        lab.controller_call(
            address,
            ControllerCall::Initialisation(InitialisationAnswer::Loadout),
        );
        assert_eq!(lab.readiness_counts().players_awaiting_completion, 1);
        assert_eq!(
            lab.readiness_counts().initialisation_blockers,
            InitialisationBlockers::default()
        );
        lab.remove_peer(address).unwrap();
        assert_eq!(lab.readiness_counts(), ReadinessCounts::default());
    }

    fn initialisation_transport_peer() -> (
        PeerLab,
        UdpSocket,
        SocketAddrV4,
        super::super::sequencing::PacketSequencer,
    ) {
        use super::super::control_messages::{NetIdType, NetIdValue, UniqueNetId};
        let mut lab = prospect_refresh_lab();
        let profile = ReleaseProfile::release_3_0_32();
        let map = PeerLabAdmission::default().map_url;
        lab.enable_player_state_open(profile.player_state_open_plan(map).unwrap().unwrap())
            .unwrap();
        lab.enable_pawn_open(
            profile.pawn_open_plan(map).unwrap().unwrap(),
            super::super::pawn_open::PAWN_CHANNEL_INDEX,
            PawnContent::default(),
        )
        .unwrap();
        let (socket, address, mut client) = acked_game_state_peer(&mut lab);
        let entry = lab.peers.get_mut(&address).unwrap();
        entry.connection_id = Some(1);
        entry.player = Some(RegisteredPlayer {
            player_id: 1,
            name_units: "AUTHORED-INITIALISATION".encode_utf16().collect(),
        });
        entry.login_unique_id = Some(UniqueNetId {
            kind: NetIdType::Named(ArchiveText::text("NULL").unwrap()),
            value: NetIdValue::Text(ArchiveText::text("AuthoredPlayer").unwrap()),
        });
        entry.profiles.character_slot = Some(3);
        let game_state = lab.game_state_open.take();
        assert!(lab.send_player_controller_open(address));
        lab.game_state_open = game_state;
        ack_startup_packets(&mut lab, &socket, &mut client);
        assert_eq!(lab.stats.player_state_opens_sent, 1);
        assert_eq!(lab.stats.pawn_opens_sent, 1);
        (lab, socket, address, client)
    }

    #[test]
    fn friends_query_begin_fanout_is_reliable_empty_and_requires_real_reply() {
        use super::super::{
            actor_channel::read_actor_bunch_body,
            packet::{Receiver, read_packet},
        };
        let (mut lab, socket, address, mut client) = initialisation_transport_peer();
        let engine = lab
            .player_controller_open
            .as_ref()
            .unwrap()
            .engine_net_version;
        let codec = RepLayoutCodec {
            actor: actor_codec(engine, super::super::archive::ByteOrder::Little),
            checksums: false,
            limits: Default::default(),
        };
        let entry = lab.peers.get_mut(&address).unwrap();
        entry.character_slot = Some(3);
        entry.profiles.completed = false;
        entry.profiles.friend_ids_ready = true;
        // A fresh controller's Begin must requery an existing connected player.
        let (second_socket, second_address, mut second_client) = acked_game_state_peer(&mut lab);
        let login = lab.peers[&address].login_unique_id.clone();
        let second = lab.peers.get_mut(&second_address).unwrap();
        second.login_unique_id = login;
        second.connection_id = Some(2);
        second.player = Some(RegisteredPlayer {
            player_id: 2,
            name_units: vec![],
        });
        let state = lab.game_state_open.take();
        assert!(lab.send_player_controller_open(second_address));
        lab.game_state_open = state;
        ack_startup_packets(&mut lab, &second_socket, &mut second_client);
        let entry = lab.peers.get_mut(&second_address).unwrap();
        entry.player = Some(RegisteredPlayer {
            player_id: 2,
            name_units: vec![],
        });
        assert!(entry.character_slot.is_none());
        assert!(
            entry
                .player_state_channel
                .is_some_and(|c| entry.peer.has_actor_channel(c))
        );
        if !lab.peers[&address]
            .peer
            .sequencer()
            .notify()
            .sent_records()
            .is_empty()
        {
            ack_startup_packets(&mut lab, &socket, &mut client);
        }
        lab.peers.get_mut(&address).unwrap().profiles.completed = true;
        lab.controller_call(second_address, ControllerCall::Begin(4));
        assert_eq!(lab.stats.friends_list_queries_queued, 2);
        let mut bytes = [0; 2048];
        let n = socket.peek(&mut bytes).unwrap();
        let packet = read_packet(
            &bytes[..n],
            &super::super::packet::PacketConfig {
                receiver: Receiver::Client,
                ..lab.config.packet.packet
            },
        )
        .unwrap();
        assert_eq!(packet.bunches.len(), 1);
        let bunch = &packet.bunches[0];
        assert!(bunch.header.reliable);
        assert_eq!(
            Some(bunch.header.channel_index),
            lab.peers[&address].player_controller_channel
        );
        let body = read_actor_bunch_body(&mut bunch.payload.reader(), false, &codec.actor).unwrap();
        assert_eq!(body.blocks.len(), 1);
        assert_eq!(body.blocks[0].target, BlockTarget::Actor);
        let mut reader = body.blocks[0].payload.reader();
        let field = read_field_header(&mut reader, lab.controller_startup_bound, 1024).unwrap();
        assert!(reader.at_end());
        assert_eq!(field.index, 118);
        assert_eq!(field.payload.bit_len(), 0);
        assert!(!lab.peers[&address].profiles.friend_ids_ready);
        // Transport settlement supplies no client friends-list response.
        ack_startup_packets(&mut lab, &socket, &mut client);
        lab.finalise_connected_players();
        assert_eq!(lab.stats.players_finalised, 0);
        lab.controller_call(address, ControllerCall::FriendsList);
        lab.controller_call(second_address, ControllerCall::FriendsList);
        lab.finalise_connected_players();
        assert_eq!(lab.stats.players_finalised, 1);
        lab.controller_call(second_address, ControllerCall::Begin(4));
        lab.retry_friends_queries();
        assert_eq!(lab.stats.friends_list_queries_queued, 2);
        lab.shutdown();
    }

    #[test]
    fn friends_query_pressure_retries_once_and_disconnect_discards_pending() {
        let (mut lab, socket, address, mut client) = initialisation_transport_peer();
        let entry = lab.peers.get_mut(&address).unwrap();
        entry.character_slot = Some(3);
        entry.profiles.completed = true;
        entry.profiles.friend_ids_ready = true;
        for _ in 0..entry.peer.config().sent_capacity {
            let datagram = entry.peer.ack_packet().unwrap();
            lab.socket.send_to(&datagram, address).unwrap();
            let mut bytes = [0; 2048];
            let n = socket.recv(&mut bytes).unwrap();
            let packet = super::super::packet::read_packet(
                &bytes[..n],
                &super::super::packet::PacketConfig {
                    receiver: super::super::packet::Receiver::Client,
                    ..lab.config.packet.packet
                },
            )
            .unwrap();
            let received = client.receive(&packet.header).unwrap().unwrap();
            client.ack_seq(received.sequence, true).unwrap();
        }
        lab.query_connected_friends();
        assert!(lab.peers[&address].friends_query_pending);
        assert_eq!(lab.stats.friends_list_queries_queued, 0);
        lab.finalise_connected_players();
        assert_eq!(lab.stats.players_finalised, 0);
        for _ in 0..3 {
            lab.retry_friends_queries();
        }
        assert_eq!(lab.stats.friends_list_queries_queued, 0);
        let bytes = super::super::packet::write_packet(
            &super::super::packet::Packet {
                magic: 0,
                header: client.header().unwrap(),
                info: None,
                bunches: vec![],
            },
            &lab.config.packet.packet,
        )
        .unwrap();
        socket.send(&bytes).unwrap();
        lab.poll();
        lab.retry_friends_queries();
        assert_eq!(lab.stats.friends_list_queries_queued, 1);
        assert!(!lab.peers[&address].friends_query_pending);
        assert!(!lab.peers[&address].profiles.friend_ids_ready);
        lab.retry_friends_queries();
        assert_eq!(lab.stats.friends_list_queries_queued, 1);
        lab.remove_peer(address).unwrap();
        lab.retry_friends_queries();
        assert_eq!(lab.peers.len(), 0);
    }

    #[test]
    fn friends_query_nak_repairs_transport_without_requerying_or_finalising() {
        let (mut lab, socket, address, mut client) = initialisation_transport_peer();
        let entry = lab.peers.get_mut(&address).unwrap();
        entry.character_slot = Some(3);
        entry.profiles.completed = true;
        lab.query_connected_friends();
        assert_eq!(lab.stats.friends_list_queries_queued, 1);
        notify_startup_packets(&mut lab, &socket, &mut client, |_| false);
        assert!(lab.readiness_counts().resend_bunches_pending > 0);
        lab.retry_friends_queries();
        lab.finalise_connected_players();
        assert_eq!(lab.stats.friends_list_queries_queued, 1);
        assert_eq!(lab.stats.players_finalised, 0);
        let repair = lab
            .peers
            .get_mut(&address)
            .unwrap()
            .peer
            .ack_packet()
            .unwrap();
        assert!(lab.send(&repair, address));
        ack_startup_packets(&mut lab, &socket, &mut client);
        assert_eq!(lab.readiness_counts().resend_bunches_pending, 0);
        lab.finalise_connected_players();
        assert!(!lab.peers[&address].profiles.friend_ids_ready);
        assert_eq!(lab.stats.friends_lists_received, 0);
        assert_eq!(lab.stats.players_finalised, 0);
        assert_eq!(lab.stats.friends_list_queries_queued, 1);
    }

    #[test]
    fn friends_query_held_back_shows_pending_admission_until_queued() {
        let (mut lab, socket, address, mut client) = initialisation_transport_peer();
        let entry = lab.peers.get_mut(&address).unwrap();
        entry.character_slot = Some(3);
        entry.profiles.completed = true;
        entry.profiles.friend_ids_ready = true;
        for _ in 0..entry.peer.config().sent_capacity {
            let datagram = entry.peer.ack_packet().unwrap();
            lab.socket.send_to(&datagram, address).unwrap();
            let mut bytes = [0; 2048];
            let n = socket.recv(&mut bytes).unwrap();
            let packet = super::super::packet::read_packet(
                &bytes[..n],
                &super::super::packet::PacketConfig {
                    receiver: super::super::packet::Receiver::Client,
                    ..lab.config.packet.packet
                },
            )
            .unwrap();
            let received = client.receive(&packet.header).unwrap().unwrap();
            client.ack_seq(received.sequence, true).unwrap();
        }
        assert_eq!(lab.readiness_counts().friends_query, Default::default());
        lab.query_connected_friends();
        assert_eq!(
            lab.readiness_counts().friends_query,
            FriendsQueryReadiness {
                pending_admission: 1,
                ..Default::default()
            }
        );
        let bytes = super::super::packet::write_packet(
            &super::super::packet::Packet {
                magic: 0,
                header: client.header().unwrap(),
                info: None,
                bunches: vec![],
            },
            &lab.config.packet.packet,
        )
        .unwrap();
        socket.send(&bytes).unwrap();
        lab.poll();
        lab.retry_friends_queries();
        assert_eq!(lab.stats.friends_list_queries_queued, 1);
        assert_eq!(
            lab.readiness_counts().friends_query,
            FriendsQueryReadiness {
                queued: 1,
                awaiting_ack: 1,
                ..Default::default()
            }
        );
        assert_eq!(lab.stats.friends_list_queries_acknowledged, 0);
        lab.remove_peer(address).unwrap();
        assert_eq!(lab.readiness_counts().friends_query, Default::default());
    }

    #[test]
    fn friends_query_receipt_settles_on_its_own_bunch_ack_then_awaits_response() {
        let (mut lab, socket, address, mut client) = initialisation_transport_peer();
        let entry = lab.peers.get_mut(&address).unwrap();
        entry.character_slot = Some(3);
        entry.profiles.completed = true;
        let channel = entry.player_controller_channel.unwrap();
        lab.query_connected_friends();
        assert_eq!(lab.stats.friends_list_queries_queued, 1);
        let awaiting_ack = FriendsQueryReadiness {
            queued: 1,
            awaiting_ack: 1,
            ..Default::default()
        };
        assert_eq!(lab.readiness_counts().friends_query, awaiting_ack);
        // A later reliable bunch on the same channel whose packet is lost:
        // the channel stays unsettled while the query's own bunch settles.
        let later = lab
            .peers
            .get_mut(&address)
            .unwrap()
            .peer
            .reliable_actor_bunch(
                channel,
                Payload::from_writer(super::super::packet::BitWriter::new()),
            )
            .unwrap();
        assert!(lab.send(&later, address));
        // NAK the query's packet first: nothing settles.
        notify_startup_packets(&mut lab, &socket, &mut client, |_| false);
        assert_eq!(lab.readiness_counts().friends_query, awaiting_ack);
        assert_eq!(lab.stats.friends_list_queries_acknowledged, 0);
        // Repair resends both bunches in one packet; lose a fresh later bunch.
        let repair = lab
            .peers
            .get_mut(&address)
            .unwrap()
            .peer
            .ack_packet()
            .unwrap();
        assert!(lab.send(&repair, address));
        let later = lab
            .peers
            .get_mut(&address)
            .unwrap()
            .peer
            .reliable_actor_bunch(
                channel,
                Payload::from_writer(super::super::packet::BitWriter::new()),
            )
            .unwrap();
        assert!(lab.send(&later, address));
        notify_startup_packets(&mut lab, &socket, &mut client, |index| index == 0);
        assert!(!lab.peers[&address].peer.actor_channel_settled(channel));
        assert_eq!(lab.stats.friends_list_queries_acknowledged, 1);
        assert_eq!(
            lab.readiness_counts().friends_query,
            FriendsQueryReadiness {
                queued: 1,
                settled: 1,
                awaiting_response: 1,
                ..Default::default()
            }
        );
        // The ACK alone neither readies friend IDs nor finalises.
        lab.finalise_connected_players();
        assert_eq!(lab.stats.players_finalised, 0);
        lab.controller_call(address, ControllerCall::FriendsList);
        assert_eq!(lab.stats.friends_lists_received, 1);
        assert_eq!(
            lab.readiness_counts().friends_query,
            FriendsQueryReadiness {
                queued: 1,
                settled: 1,
                ..Default::default()
            }
        );
        // Settlement is counted once per admitted query: repairing the
        // later bunch settles the channel without a second count.
        let repair = lab
            .peers
            .get_mut(&address)
            .unwrap()
            .peer
            .ack_packet()
            .unwrap();
        assert!(lab.send(&repair, address));
        ack_startup_packets(&mut lab, &socket, &mut client);
        assert!(lab.peers[&address].peer.actor_channel_settled(channel));
        assert_eq!(lab.stats.friends_list_queries_acknowledged, 1);
        assert_eq!(lab.session_counts().friends_list_queries_acknowledged, 1);
    }

    #[test]
    fn ignored_friends_lists_are_counted_apart_from_ignored_begin_calls() {
        let (mut lab, _socket, address, _client) = initialisation_transport_peer();
        lab.controller_call(address, ControllerCall::FriendsList);
        assert_eq!(lab.stats.friends_lists_received, 1);
        assert_eq!(lab.stats.initialisation_calls_ignored, 0);
        lab.controller_call(address, ControllerCall::FriendsList);
        assert_eq!(lab.stats.friends_lists_ignored_already_ready, 1);
        assert_eq!(lab.stats.friends_lists_ignored_unregistered, 0);
        assert_eq!(lab.stats.initialisation_calls_ignored, 1);
        lab.controller_call(address, ControllerCall::Begin(-1));
        assert_eq!(lab.stats.initialisation_calls_ignored, 2);
        assert_eq!(lab.stats.friends_lists_ignored_already_ready, 1);
        let entry = lab.peers.get_mut(&address).unwrap();
        entry.player = None;
        entry.profiles.friend_ids_ready = false;
        lab.controller_call(address, ControllerCall::FriendsList);
        assert_eq!(lab.stats.friends_lists_ignored_unregistered, 1);
        assert_eq!(lab.stats.friends_lists_ignored_already_ready, 1);
        assert_eq!(lab.stats.initialisation_calls_ignored, 3);
        assert_eq!(lab.stats.friends_lists_received, 1);
        let counts = lab.session_counts();
        assert_eq!(counts.friends_lists_ignored_unregistered, 1);
        assert_eq!(counts.friends_lists_ignored_already_ready, 1);
        assert_eq!(counts.initialisation_calls_ignored, 3);
    }

    /// A lab with every open a connected-player record references and Mode A
    /// records configured before any peer exists.
    fn record_lab(enabled: bool) -> PeerLab {
        let mut lab = prospect_refresh_lab();
        let profile = ReleaseProfile::release_3_0_32();
        let map = PeerLabAdmission::default().map_url;
        lab.enable_player_state_open(profile.player_state_open_plan(map).unwrap().unwrap())
            .unwrap();
        lab.enable_pawn_open(
            profile.pawn_open_plan(map).unwrap().unwrap(),
            super::super::pawn_open::PAWN_CHANNEL_INDEX,
            PawnContent::default(),
        )
        .unwrap();
        if enabled {
            lab.enable_connected_player_records(
                ReleaseProfile::release_3_0_35()
                    .connected_players_plan()
                    .unwrap()
                    .unwrap(),
            )
            .unwrap();
        }
        lab
    }

    /// A joined peer with its GameState, controller, pawn and PlayerState open
    /// and acked, an authored login ID and profile slot 3; no Begin yet.
    fn record_peer(
        lab: &mut PeerLab,
        connection: u32,
        user: &str,
    ) -> (
        UdpSocket,
        SocketAddrV4,
        super::super::sequencing::PacketSequencer,
    ) {
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        socket.connect(lab.local_addr().unwrap()).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        let (address, client) = record_peer_on(lab, &socket, connection, user);
        (socket, address, client)
    }

    /// The stateless handshake on `socket`: a new peer, or a replacement
    /// (new cookie) of the peer this address already has.
    fn handshake_peer_on(
        lab: &mut PeerLab,
        socket: &UdpSocket,
    ) -> (SocketAddrV4, super::super::sequencing::PacketSequencer) {
        use super::super::{
            handshake::initial_request, packet_lab::client_cookie_seeds,
            sequencing::PacketSequencer,
        };
        let SocketAddr::V4(address) = socket.local_addr().unwrap() else {
            unreachable!("IPv4 loopback")
        };
        // A rejoining address may still have its old session's data packets
        // in flight; only the handshake replies matter here.
        let handshake = |buffer: &mut [u8; 2048]| loop {
            let length = socket.recv(buffer).unwrap();
            if let Ok(reply) = decode(&buffer[..length]) {
                return (reply, length);
            }
        };
        let mut buffer = [0; 2048];
        socket.send(&initial_request()).unwrap();
        lab.poll();
        let (challenge, length) = handshake(&mut buffer);
        let cookie = challenge.cookie;
        socket.send(&buffer[..length]).unwrap();
        lab.poll();
        let (acknowledgement, _) = handshake(&mut buffer);
        assert_eq!(acknowledgement.cookie, cookie);
        let (incoming, outgoing) = client_cookie_seeds(&cookie);
        (
            address,
            PacketSequencer::new(incoming, outgoing, 64).unwrap(),
        )
    }

    /// [`record_peer`] on an existing socket: handshake, then the real
    /// initial-open sequence (controller, pawn, GameState, PlayerState), acked.
    fn record_peer_on(
        lab: &mut PeerLab,
        socket: &UdpSocket,
        connection: u32,
        user: &str,
    ) -> (SocketAddrV4, super::super::sequencing::PacketSequencer) {
        use super::super::control_messages::{NetIdType, NetIdValue, UniqueNetId};
        let (address, mut client) = handshake_peer_on(lab, socket);
        let entry = lab.peers.get_mut(&address).unwrap();
        entry.connection_id = Some(connection);
        entry.player = Some(RegisteredPlayer {
            player_id: connection as i32,
            name_units: "AUTHORED-RECORD".encode_utf16().collect(),
        });
        entry.login_unique_id = Some(UniqueNetId {
            kind: NetIdType::Named(ArchiveText::text("NULL").unwrap()),
            value: NetIdValue::Text(ArchiveText::text(user).unwrap()),
        });
        entry.profiles.character_slot = Some(3);
        assert!(lab.send_player_controller_open(address));
        assert!(lab.peers[&address].game_state.is_some());
        ack_startup_packets(lab, socket, &mut client);
        (address, client)
    }

    /// Begin, both initialisation answers (completion) and the friends list:
    /// the authored session reaches finalisation's preconditions.
    fn complete_record_player(lab: &mut PeerLab, address: SocketAddrV4) {
        lab.controller_call(address, ControllerCall::Begin(3));
        let entry = lab.peers.get_mut(&address).unwrap();
        entry.profiles.outstanding = None;
        entry.profiles.has_loadout = true;
        entry.profiles.has_bestiary = true;
        lab.complete_player_initialisation(address);
        assert!(lab.peers[&address].profiles.completed);
    }

    /// Read every outstanding datagram of `address`, notifying the lab as
    /// `delivered` says, and return the bunches they carried in order.
    fn deliver_bunches(
        lab: &mut PeerLab,
        socket: &UdpSocket,
        client: &mut super::super::sequencing::PacketSequencer,
        delivered: impl Fn(usize) -> bool,
    ) -> Vec<super::super::packet::Bunch> {
        use super::super::packet::{Packet, Receiver, read_packet, write_packet};
        let SocketAddr::V4(address) = socket.local_addr().unwrap() else {
            unreachable!()
        };
        let count = lab.peers[&address]
            .peer
            .sequencer()
            .notify()
            .sent_records()
            .len();
        let config = super::super::packet::PacketConfig {
            receiver: Receiver::Client,
            ..lab.config.packet.packet
        };
        let mut bunches = Vec::new();
        for index in 0..count {
            let mut bytes = [0; 2048];
            let length = socket.recv(&mut bytes).unwrap();
            let packet = read_packet(&bytes[..length], &config).unwrap();
            let received = client.receive(&packet.header).unwrap().unwrap();
            client.ack_seq(received.sequence, delivered(index)).unwrap();
            bunches.extend(packet.bunches);
        }
        if count > 0 {
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
            receive_authored_at(lab, socket, &bytes, Instant::now());
        }
        bunches
    }

    fn game_state_channel(lab: &PeerLab) -> u32 {
        lab.game_state_open.as_ref().unwrap().channel_index
    }

    /// Reliable, non-opening GameState-channel bunch bodies (export parts
    /// excluded), decoded.
    fn record_bodies(
        lab: &PeerLab,
        bunches: &[super::super::packet::Bunch],
    ) -> Vec<ActorBunchBody> {
        let channel = game_state_channel(lab);
        let codec = actor_codec(17, super::super::archive::ByteOrder::Little);
        bunches
            .iter()
            .filter(|bunch| {
                bunch.header.channel_index == channel
                    && bunch.header.reliable
                    && !bunch.header.open
                    && !bunch.header.has_package_map_exports
            })
            .map(|bunch| {
                super::super::actor_channel::read_actor_bunch_body(
                    &mut bunch.payload.reader(),
                    false,
                    &codec,
                )
                .unwrap()
            })
            .collect()
    }

    fn record_codec() -> RepLayoutCodec {
        RepLayoutCodec {
            actor: actor_codec(17, super::super::archive::ByteOrder::Little),
            checksums: false,
            limits: Default::default(),
        }
    }

    /// The record GUIDs a decoded `ConnectedPlayers` block lists.
    fn listed_records(lab: &PeerLab, block: &ContentBlock) -> Vec<u32> {
        let plan = lab.connected_players.as_ref().unwrap();
        let stream = super::super::rep_layout::parse_rep_properties(
            &block.payload,
            &plan.component_layout,
            &record_codec(),
        )
        .unwrap();
        let [entry] = stream.entries.as_slice() else {
            panic!("only the array handle");
        };
        assert_eq!(entry.handle, 3);
        let RepValue::DynamicArray(array) = &entry.value else {
            panic!("an array");
        };
        assert_eq!(usize::from(array.count), array.stream.entries.len());
        array
            .stream
            .entries
            .iter()
            .map(|element| match &element.value {
                RepValue::Object(reference) => reference.guid,
                other => panic!("object element expected, got {other:?}"),
            })
            .collect()
    }

    #[test]
    fn connected_player_record_waits_for_finalisation_then_is_created_once() {
        let mut lab = record_lab(true);
        let (socket, address, mut client) = record_peer(&mut lab, 1, "AuthoredRecordUser");
        assert_eq!(
            lab.readiness_counts().connected_player_records,
            ConnectedPlayerRecordReadiness::default()
        );
        complete_record_player(&mut lab, address);
        // Registered at Begin; Mode A sends nothing before finalisation.
        assert_eq!(lab.stats.connected_player_records_created, 1);
        assert_eq!(
            lab.readiness_counts().connected_player_records,
            ConnectedPlayerRecordReadiness {
                registered: 1,
                ..Default::default()
            }
        );
        let bunches = deliver_bunches(&mut lab, &socket, &mut client, |_| true);
        assert!(record_bodies(&lab, &bunches).is_empty());
        lab.finalise_connected_players();
        assert_eq!(lab.stats.players_finalised, 0, "friends list outstanding");
        lab.retry_connected_player_records();
        assert_eq!(lab.stats.connected_player_record_creations_sent, 0);
        lab.controller_call(address, ControllerCall::FriendsList);
        lab.finalise_connected_players();
        assert_eq!(lab.stats.players_finalised, 1);
        assert_eq!(lab.stats.connected_player_record_creations_sent, 1);
        assert_eq!(lab.stats.connected_player_array_updates_sent, 1);
        assert_eq!(
            lab.readiness_counts().connected_player_records,
            ConnectedPlayerRecordReadiness {
                registered: 1,
                finalised: 1,
                creations_queued: 1,
                records_awaiting_ack: 1,
                ..Default::default()
            }
        );
        let bunches = deliver_bunches(&mut lab, &socket, &mut client, |_| true);
        let channel = game_state_channel(&lab);
        let exports: Vec<_> = bunches
            .iter()
            .filter(|bunch| bunch.header.channel_index == channel)
            .map(|bunch| bunch.header.has_package_map_exports)
            .collect();
        // The class export precedes the body on the same reliable channel.
        assert_eq!(exports, [true, false]);
        let [body] = record_bodies(&lab, &bunches).try_into().unwrap();
        let [actor, record, component] = body.blocks.as_slice() else {
            panic!("three blocks");
        };
        assert_eq!(actor.target, BlockTarget::Actor);
        let plan = lab.connected_players.clone().unwrap();
        let entry = &lab.peers[&address];
        let (controller, player_state) = entry.controller_open_guids.unwrap();
        let BlockTarget::SubObject {
            object,
            class: super::super::actor_channel::SubObjectClass::Class(_),
        } = &record.target
        else {
            panic!("a class-named record block");
        };
        let values = super::super::rep_layout::parse_rep_properties(
            &record.payload,
            &plan.record_layout,
            &record_codec(),
        )
        .unwrap();
        let handles: Vec<u32> = values.entries.iter().map(|entry| entry.handle).collect();
        assert_eq!(handles, [1, 2, 3, 4, 5, 6, 7]);
        let value = |handle: usize| values.entries[handle - 1].value.clone();
        assert_eq!(
            value(1),
            RepValue::Str(ArchiveText::text("AuthoredRecordUser").unwrap())
        );
        assert_eq!(value(2), RepValue::Int(3));
        assert_eq!(
            value(3),
            RepValue::Object(super::super::actor_channel::NetGuidRef::plain(controller))
        );
        assert_eq!(
            value(4),
            RepValue::Object(super::super::actor_channel::NetGuidRef::plain(
                entry.controller_startup_pawn_guid.unwrap()
            ))
        );
        assert_eq!(
            value(5),
            RepValue::Object(super::super::actor_channel::NetGuidRef::plain(player_state))
        );
        let (RepValue::Float(start), RepValue::Float(complete)) = (value(6), value(7)) else {
            panic!("float times");
        };
        assert!(f32::from_bits(complete) >= f32::from_bits(start));
        assert_eq!(listed_records(&lab, component), [object.guid]);
        // A positive ACK settles the creation once.
        assert_eq!(lab.stats.connected_player_record_creations_acked, 1);
        assert_eq!(
            lab.readiness_counts().connected_player_records,
            ConnectedPlayerRecordReadiness {
                registered: 1,
                finalised: 1,
                creations_queued: 1,
                creations_settled: 1,
                ..Default::default()
            }
        );
        // Nothing changes on later passes, finalisation or requeries.
        lab.finalise_connected_players();
        lab.mark_connected_players_due(|_| true);
        lab.retry_connected_player_records();
        lab.controller_call(address, ControllerCall::Begin(3));
        assert!(deliver_bunches(&mut lab, &socket, &mut client, |_| true).is_empty());
        assert_eq!(lab.stats.connected_player_record_creations_sent, 1);
        assert_eq!(lab.stats.connected_player_array_updates_sent, 1);
        assert_eq!(lab.stats.connected_player_record_creations_acked, 1);
        assert_eq!(lab.stats.connected_player_records_created, 1);
        assert_eq!(lab.stats.connected_player_record_updates_sent, 0);
        assert_eq!(lab.stats.connected_player_record_errors, 0);
        let counts = lab.session_counts();
        assert_eq!(counts.connected_player_record_creations_sent, 1);
        assert_eq!(counts.connected_player_record_creations_acked, 1);
        let text = serde_json::to_string(&lab.readiness_counts()).unwrap();
        assert!(!text.contains("AuthoredRecordUser"));
        lab.remove_peer(address).unwrap();
        assert_eq!(lab.stats.connected_player_records_removed, 1);
        assert_eq!(lab.readiness_counts(), ReadinessCounts::default());
    }

    /// The `bServerFrozenMovement` values of the whole unreliable pawn
    /// property updates among `bunches`, decoded against the lab's pawn layout.
    fn frozen_releases(lab: &PeerLab, bunches: &[super::super::packet::Bunch]) -> Vec<RepValue> {
        let (plan, channel, _) = lab.pawn_open.as_ref().unwrap();
        let codec = record_codec();
        let wire = plan
            .layout_version
            .wire_handle(super::super::pawn_open::handle::SERVER_FROZEN_MOVEMENT)
            .unwrap();
        bunches
            .iter()
            .filter(|bunch| bunch.header.channel_index == *channel && !bunch.header.open)
            .map(|bunch| {
                // Unreliable, as the original sends it (S2).
                assert!(!bunch.header.reliable && !bunch.header.partial);
                let body = super::super::actor_channel::read_actor_bunch_body(
                    &mut bunch.payload.reader(),
                    false,
                    &codec.actor,
                )
                .unwrap();
                let [block] = body.blocks.as_slice() else {
                    panic!("one block");
                };
                assert_eq!(block.target, BlockTarget::Actor);
                let stream = super::super::rep_layout::parse_rep_properties(
                    &block.payload,
                    &plan.layout,
                    &codec,
                )
                .unwrap();
                let [entry] = stream.entries.as_slice() else {
                    panic!("one property");
                };
                assert_eq!(entry.handle, wire);
                entry.value.clone()
            })
            .collect()
    }

    /// The release is scheduled by the positive ACK of the player's own
    /// record creation, never before; it is sent once after the delay,
    /// unreliable; a NAK sends the still-current false again without a second
    /// release, and the first ACK of a copy settles it once.
    #[test]
    fn frozen_movement_release_follows_the_record_ack_after_its_delay_once() {
        let mut lab = record_lab(true);
        let delay = Duration::from_millis(120);
        lab.enable_frozen_movement_release(delay).unwrap();
        assert_eq!(lab.frozen_movement_release_delay(), Some(delay));
        let (socket, address, mut client) = record_peer(&mut lab, 1, "AuthoredFrozenUser");
        assert!(
            lab.enable_frozen_movement_release(delay).is_err(),
            "configured before admission"
        );
        complete_record_player(&mut lab, address);
        deliver_bunches(&mut lab, &socket, &mut client, |_| true);
        lab.controller_call(address, ControllerCall::FriendsList);
        lab.finalise_connected_players();
        assert_eq!(lab.stats.connected_player_record_creations_sent, 1);
        // Finalised and the record sent, but not ACKed: nothing scheduled.
        lab.retry_frozen_movement_release();
        assert_eq!(lab.stats.frozen_movement_releases_scheduled, 0);
        assert_eq!(
            lab.readiness_counts().frozen_movement_release,
            Default::default()
        );
        // A NAK of the record schedules nothing either.
        deliver_bunches(&mut lab, &socket, &mut client, |_| false);
        assert_eq!(lab.stats.frozen_movement_releases_scheduled, 0);
        let repair = lab
            .peers
            .get_mut(&address)
            .unwrap()
            .peer
            .ack_packet()
            .unwrap();
        assert!(lab.send(&repair, address));
        let bunches = deliver_bunches(&mut lab, &socket, &mut client, |_| true);
        assert!(frozen_releases(&lab, &bunches).is_empty());
        assert_eq!(lab.stats.connected_player_record_creations_acked, 1);
        assert_eq!(lab.stats.frozen_movement_releases_scheduled, 1);
        // Waiting out the delay.
        lab.retry_frozen_movement_release();
        assert_eq!(lab.stats.frozen_movement_releases_sent, 0);
        assert_eq!(
            lab.readiness_counts().frozen_movement_release,
            FrozenMovementReleaseReadiness {
                awaiting_delay: 1,
                ..Default::default()
            }
        );
        std::thread::sleep(delay + Duration::from_millis(10));
        lab.retry_frozen_movement_release();
        assert_eq!(lab.stats.frozen_movement_releases_sent, 1);
        assert_eq!(
            lab.readiness_counts().frozen_movement_release,
            FrozenMovementReleaseReadiness {
                awaiting_ack: 1,
                ..Default::default()
            }
        );
        // Nothing is retained for reliable repair.
        assert_eq!(lab.readiness_counts().resend_bunches_pending, 0);
        let [observation] = lab.frozen_movement_observations()[..] else {
            panic!("one session");
        };
        let acked = observation.record_acked_ms.unwrap();
        let delay_ms = delay.as_millis() as u64;
        assert_eq!(observation.release_due_ms, Some(acked + delay_ms));
        assert!(observation.release_sent_ms.unwrap() >= acked + delay_ms);
        // Lost: the current value is due again and sent again, not
        // released a second time.
        let lost = deliver_bunches(&mut lab, &socket, &mut client, |_| false);
        assert_eq!(frozen_releases(&lab, &lost), [RepValue::Bool(false)]);
        assert_eq!(lab.stats.frozen_movement_releases_acked, 0);
        assert_eq!(
            lab.readiness_counts().frozen_movement_release,
            FrozenMovementReleaseReadiness {
                pending_send: 1,
                ..Default::default()
            }
        );
        lab.retry_frozen_movement_release();
        assert_eq!(lab.stats.frozen_movement_release_resends, 1);
        let repaired = deliver_bunches(&mut lab, &socket, &mut client, |_| true);
        assert_eq!(frozen_releases(&lab, &repaired), [RepValue::Bool(false)]);
        assert_eq!(lab.stats.frozen_movement_releases_sent, 1);
        assert_eq!(lab.stats.frozen_movement_releases_acked, 1);
        assert_eq!(
            lab.readiness_counts().frozen_movement_release,
            FrozenMovementReleaseReadiness {
                settled: 1,
                ..Default::default()
            }
        );
        let [observation] = lab.frozen_movement_observations()[..] else {
            panic!("one session");
        };
        assert!(observation.release_acked_ms >= observation.release_sent_ms);
        assert!(!observation.release_failed);
        // Never again: later passes, finalisation and record changes.
        lab.finalise_connected_players();
        lab.mark_connected_players_due(|_| true);
        lab.retry_connected_player_records();
        lab.retry_frozen_movement_release();
        assert!(deliver_bunches(&mut lab, &socket, &mut client, |_| true).is_empty());
        assert_eq!(lab.stats.frozen_movement_releases_scheduled, 1);
        assert_eq!(lab.stats.frozen_movement_releases_sent, 1);
        assert_eq!(lab.stats.frozen_movement_releases_acked, 1);
        assert_eq!(lab.stats.frozen_movement_release_resends, 1);
        assert_eq!(lab.stats.frozen_movement_release_errors, 0);
        let counts = lab.session_counts();
        assert_eq!(counts.frozen_movement_releases_sent, 1);
        assert_eq!(counts.frozen_movement_releases_acked, 1);
        assert_eq!(counts.frozen_movement_release_resends, 1);
        lab.remove_peer(address).unwrap();
        assert!(lab.frozen_movement_observations().is_empty());
    }

    /// Off (the lab default) sends nothing; the release needs records.
    #[test]
    fn frozen_movement_release_off_sends_nothing_and_needs_records() {
        assert!(
            record_lab(false)
                .enable_frozen_movement_release(Duration::ZERO)
                .is_err()
        );
        assert!(
            record_lab(true)
                .enable_frozen_movement_release(Duration::from_millis(600_001))
                .is_err()
        );
        let mut lab = record_lab(true);
        assert_eq!(lab.frozen_movement_release_delay(), None);
        let (socket, address, mut client) = record_peer(&mut lab, 1, "AuthoredFrozenUser");
        complete_record_player(&mut lab, address);
        lab.controller_call(address, ControllerCall::FriendsList);
        lab.finalise_connected_players();
        let bunches = deliver_bunches(&mut lab, &socket, &mut client, |_| true);
        assert!(frozen_releases(&lab, &bunches).is_empty());
        assert_eq!(lab.stats.connected_player_record_creations_acked, 1);
        lab.retry_frozen_movement_release();
        assert!(deliver_bunches(&mut lab, &socket, &mut client, |_| true).is_empty());
        assert_eq!(lab.stats.frozen_movement_releases_scheduled, 0);
        assert_eq!(lab.stats.frozen_movement_releases_sent, 0);
        let [observation] = lab.frozen_movement_observations()[..] else {
            panic!("one session");
        };
        assert_eq!(observation.record_acked_ms, None);
    }

    /// A pawn channel closed before the release falls due abandons it.
    #[test]
    fn frozen_movement_release_is_abandoned_without_an_open_pawn_channel() {
        let mut lab = record_lab(true);
        lab.enable_frozen_movement_release(Duration::ZERO).unwrap();
        let (socket, address, mut client) = record_peer(&mut lab, 1, "AuthoredFrozenUser");
        complete_record_player(&mut lab, address);
        lab.controller_call(address, ControllerCall::FriendsList);
        lab.finalise_connected_players();
        // Hold the release back by hand, then lose the pawn.
        lab.frozen_movement_release = None;
        deliver_bunches(&mut lab, &socket, &mut client, |_| true);
        lab.frozen_movement_release = Some(Duration::ZERO);
        let entry = lab.peers.get_mut(&address).unwrap();
        entry.frozen_release.record_acked_at = Some(Instant::now());
        entry.frozen_release.due_at = Some(Instant::now());
        entry.controller_startup_pawn_guid = None;
        lab.retry_frozen_movement_release();
        assert_eq!(lab.stats.frozen_movement_releases_sent, 0);
        assert_eq!(lab.stats.frozen_movement_release_errors, 1);
        lab.retry_frozen_movement_release();
        assert_eq!(lab.stats.frozen_movement_release_errors, 1, "once");
        assert!(lab.frozen_movement_observations()[0].release_failed);
    }

    /// A due release, first copy or repair, waits while a reliable bunch on
    /// the pawn channel is unACKed, as GameState updates do, and goes out
    /// once that bunch is ACKed.
    #[test]
    fn frozen_movement_release_waits_for_the_pawn_channels_reliable_bunches() {
        let mut lab = record_lab(true);
        lab.enable_frozen_movement_release(Duration::ZERO).unwrap();
        let (socket, address, mut client) = record_peer(&mut lab, 1, "AuthoredFrozenUser");
        complete_record_player(&mut lab, address);
        deliver_bunches(&mut lab, &socket, &mut client, |_| true);
        lab.controller_call(address, ControllerCall::FriendsList);
        lab.finalise_connected_players();
        // A reliable pawn bunch in flight when the record's ACK arrives.
        let (plan, channel, _) = lab.pawn_open.as_ref().unwrap();
        let (plan, channel) = (plan.clone(), *channel);
        let payload = compose_pawn_frozen_movement_update(
            &plan,
            true,
            super::super::archive::ByteOrder::Little,
        )
        .unwrap();
        let entry = lab.peers.get_mut(&address).unwrap();
        let held = entry
            .peer
            .reliable_actor_bunch_datagrams(channel, payload)
            .unwrap();
        lab.send_all(&held, address);
        // Deliver the record and lose the reliable pawn bunch.
        let pawn_packet = lab.peers[&address]
            .peer
            .sequencer()
            .notify()
            .sent_records()
            .len()
            - 1;
        deliver_bunches(&mut lab, &socket, &mut client, |index| index != pawn_packet);
        assert_eq!(lab.stats.frozen_movement_releases_scheduled, 1);
        lab.retry_frozen_movement_release();
        assert_eq!(lab.stats.frozen_movement_releases_sent, 0);
        assert_eq!(
            lab.readiness_counts().frozen_movement_release,
            FrozenMovementReleaseReadiness {
                pending_send: 1,
                ..Default::default()
            }
        );
        // The reliable layer repairs that bunch; once ACKed, the release goes.
        let repair = lab
            .peers
            .get_mut(&address)
            .unwrap()
            .peer
            .ack_packet()
            .unwrap();
        assert!(lab.send(&repair, address));
        deliver_bunches(&mut lab, &socket, &mut client, |_| true);
        lab.retry_frozen_movement_release();
        assert_eq!(lab.stats.frozen_movement_releases_sent, 1);
        let sent = deliver_bunches(&mut lab, &socket, &mut client, |_| true);
        assert_eq!(frozen_releases(&lab, &sent), [RepValue::Bool(false)]);
        assert_eq!(lab.stats.frozen_movement_releases_acked, 1);
        assert_eq!(lab.stats.frozen_movement_release_resends, 0);
    }

    /// Reachable only by hand: the real sequence opens the controller, pawn
    /// and PlayerState before Begin and completion, so a finalised record
    /// cannot lack them. Narrow check that opening them later re-triggers the
    /// record, which is then sent once.
    #[test]
    fn connected_player_record_follows_when_its_actors_open() {
        use super::super::control_messages::{NetIdType, NetIdValue, UniqueNetId};
        let mut lab = record_lab(true);
        // GameState open; no controller, pawn or PlayerState yet.
        let (socket, address, mut client) = acked_game_state_peer(&mut lab);
        let entry = lab.peers.get_mut(&address).unwrap();
        entry.connection_id = Some(1);
        entry.player = Some(RegisteredPlayer {
            player_id: 1,
            name_units: vec![],
        });
        entry.login_unique_id = Some(UniqueNetId {
            kind: NetIdType::Named(ArchiveText::text("NULL").unwrap()),
            value: NetIdValue::Text(ArchiveText::text("AuthoredLateActors").unwrap()),
        });
        entry.profiles.completed = true;
        entry.profiles.friend_ids_ready = true;
        entry.profiles.character_slot = Some(3);
        entry.connected_player = Some(ConnectedPlayerRecord {
            index: 0,
            start_time: 1.0,
            complete_time: None,
        });
        lab.next_connected_player_record = 1;
        lab.finalise_connected_players();
        assert_eq!(lab.stats.connected_player_record_creations_sent, 0);
        assert!(!lab.peers[&address].connected_players_pending);
        assert_eq!(
            lab.readiness_counts().connected_player_records.pending_send,
            0
        );
        // The real opens: each commit marks this connection again.
        let game_state = lab.game_state_open.take();
        assert!(lab.send_player_controller_open(address));
        lab.game_state_open = game_state;
        assert!(lab.peers[&address].connected_players_pending);
        lab.retry_connected_player_records();
        assert_eq!(lab.stats.connected_player_record_creations_sent, 1);
        assert_eq!(lab.stats.connected_player_record_errors, 0);
        let bunches = deliver_bunches(&mut lab, &socket, &mut client, |_| true);
        let [body] = record_bodies(&lab, &bunches).try_into().unwrap();
        let plan = lab.connected_players.clone().unwrap();
        let values = super::super::rep_layout::parse_rep_properties(
            &body.blocks[1].payload,
            &plan.record_layout,
            &record_codec(),
        )
        .unwrap();
        let entry = &lab.peers[&address];
        let (controller, player_state) = entry.controller_open_guids.unwrap();
        let object = |guid| RepValue::Object(super::super::actor_channel::NetGuidRef::plain(guid));
        assert_eq!(values.entries[2].value, object(controller));
        assert_eq!(
            values.entries[3].value,
            object(entry.controller_startup_pawn_guid.unwrap())
        );
        assert_eq!(values.entries[4].value, object(player_state));
        lab.mark_connected_players_due(|_| true);
        lab.retry_connected_player_records();
        assert_eq!(lab.stats.connected_player_record_creations_sent, 1);
        assert_eq!(lab.stats.connected_player_record_creations_acked, 1);
    }

    /// A connection without a GameState is neither marked nor re-armed when
    /// another player finalises; its later initial-open commits mark it, and
    /// it then lists exactly its own record.
    #[test]
    fn connections_without_a_game_state_are_not_armed_until_it_commits() {
        let mut lab = record_lab(true);
        let (first_socket, first, mut first_client) = record_peer(&mut lab, 1, "AuthoredReady");
        complete_record_player(&mut lab, first);
        lab.controller_call(first, ControllerCall::FriendsList);
        // A second connection is still joining: handshake only.
        let second_socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        second_socket.connect(lab.local_addr().unwrap()).unwrap();
        second_socket
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        let (second, mut second_client) = handshake_peer_on(&mut lab, &second_socket);
        lab.finalise_connected_players();
        assert_eq!(lab.stats.players_finalised, 1);
        assert_eq!(lab.stats.connected_player_record_creations_sent, 1);
        let joining = &lab.peers[&second];
        assert!(!joining.connected_players_pending);
        assert!(!joining.connected_players_retry_ready);
        assert_eq!(
            lab.readiness_counts().connected_player_records.pending_send,
            0
        );
        // Packet progress on the joining connection arms nothing.
        let probe = lab
            .peers
            .get_mut(&second)
            .unwrap()
            .peer
            .ack_packet()
            .unwrap();
        assert!(lab.send(&probe, second));
        deliver_bunches(&mut lab, &second_socket, &mut second_client, |_| true);
        assert!(!lab.peers[&second].connected_players_retry_ready);
        // It joins through the real initial opens: their commits (GameState
        // included) mark it; the first player's record is not visible there,
        // so nothing is sent and nothing stays pending.
        {
            use super::super::control_messages::{NetIdType, NetIdValue, UniqueNetId};
            let entry = lab.peers.get_mut(&second).unwrap();
            entry.connection_id = Some(2);
            entry.player = Some(RegisteredPlayer {
                player_id: 2,
                name_units: vec![],
            });
            entry.login_unique_id = Some(UniqueNetId {
                kind: NetIdType::Named(ArchiveText::text("NULL").unwrap()),
                value: NetIdValue::Text(ArchiveText::text("AuthoredLate").unwrap()),
            });
            entry.profiles.character_slot = Some(3);
        }
        assert!(lab.send_player_controller_open(second));
        assert!(lab.peers[&second].game_state.is_some());
        assert!(lab.peers[&second].connected_players_pending);
        lab.retry_connected_player_records();
        assert!(!lab.peers[&second].connected_players_pending);
        assert!(lab.peers[&second].connected_players_sent.is_empty());
        assert_eq!(lab.stats.connected_player_array_updates_sent, 1);
        ack_startup_packets(&mut lab, &second_socket, &mut second_client);
        // Once it finalises, it lists exactly its own record.
        complete_record_player(&mut lab, second);
        lab.controller_call(first, ControllerCall::FriendsList);
        lab.controller_call(second, ControllerCall::FriendsList);
        lab.finalise_connected_players();
        assert_eq!(lab.stats.players_finalised, 2);
        let second_index = lab.peers[&second].connected_player.as_ref().unwrap().index;
        let first_index = lab.peers[&first].connected_player.as_ref().unwrap().index;
        assert_eq!(lab.peers[&second].connected_players_sent, [second_index]);
        assert_eq!(lab.peers[&first].connected_players_sent, [first_index]);
        assert_eq!(lab.stats.connected_player_record_creations_sent, 2);
        assert_eq!(lab.stats.connected_player_array_updates_sent, 2);
        assert_eq!(
            lab.readiness_counts().connected_player_records.pending_send,
            0
        );
        deliver_bunches(&mut lab, &first_socket, &mut first_client, |_| true);
    }

    /// The same address rejoins through the cookie-replacement handshake: the
    /// old record is removed, the new session registers one record and, once
    /// finalised, sends it once under a new record GUID.
    #[test]
    fn reconnect_replaces_the_connected_player_record() {
        let mut lab = record_lab(true);
        let (socket, address, mut client) = record_peer(&mut lab, 1, "AuthoredReconnect");
        complete_record_player(&mut lab, address);
        lab.controller_call(address, ControllerCall::FriendsList);
        lab.finalise_connected_players();
        let bunches = deliver_bunches(&mut lab, &socket, &mut client, |_| true);
        let [body] = record_bodies(&lab, &bunches).try_into().unwrap();
        let BlockTarget::SubObject { object: old, .. } = &body.blocks[1].target else {
            panic!("record block");
        };
        let old_guid = old.guid;
        let old_index = lab.peers[&address].connected_player.as_ref().unwrap().index;
        let before = lab.stats.clone();
        assert_eq!(
            (
                before.connected_player_records_created,
                before.connected_player_record_creations_sent,
                before.connected_player_record_creations_acked,
                before.connected_player_records_removed,
            ),
            (1, 1, 1, 0)
        );
        // Rejoin from the same address: a new cookie replaces the peer.
        let (rejoined, mut client) = record_peer_on(&mut lab, &socket, 2, "AuthoredReconnect");
        assert_eq!(rejoined, address);
        assert_eq!(lab.stats.peers_replaced, 1);
        assert_eq!(lab.stats.connected_player_records_removed, 1);
        assert!(lab.peers[&address].connected_player.is_none());
        assert!(lab.peers[&address].connected_players_sent.is_empty());
        assert_eq!(
            lab.readiness_counts().connected_player_records,
            ConnectedPlayerRecordReadiness::default()
        );
        complete_record_player(&mut lab, address);
        assert_eq!(lab.stats.connected_player_records_created, 2);
        lab.controller_call(address, ControllerCall::FriendsList);
        lab.finalise_connected_players();
        assert_eq!(lab.stats.connected_player_record_creations_sent, 2);
        let new_index = lab.peers[&address].connected_player.as_ref().unwrap().index;
        assert_ne!(new_index, old_index);
        let bunches = deliver_bunches(&mut lab, &socket, &mut client, |_| true);
        let [body] = record_bodies(&lab, &bunches).try_into().unwrap();
        let BlockTarget::SubObject { object: new, .. } = &body.blocks[1].target else {
            panic!("record block");
        };
        assert_ne!(new.guid, old_guid);
        assert_eq!(
            listed_records(&lab, body.blocks.last().unwrap()),
            [new.guid]
        );
        assert_eq!(
            (
                lab.stats.connected_player_records_created,
                lab.stats.connected_player_record_creations_sent,
                lab.stats.connected_player_record_creations_acked,
                lab.stats.connected_player_records_removed,
                lab.stats.connected_player_array_updates_sent,
            ),
            (2, 2, 2, 1, 2)
        );
        assert_eq!(
            lab.readiness_counts().connected_player_records,
            ConnectedPlayerRecordReadiness {
                registered: 1,
                finalised: 1,
                creations_queued: 1,
                creations_settled: 1,
                ..Default::default()
            }
        );
    }

    /// Make the world time due on `address` and lift the update-pass rate
    /// limit, so `update_game_state` sends exactly when the channel permits.
    fn force_clock_due(lab: &mut PeerLab, address: SocketAddrV4) {
        use super::super::game_state_open::handle::WORLD_TIME_SECONDS;
        lab.game_state_open.as_mut().unwrap().updated_at = None;
        let entry = lab.peers.get_mut(&address).unwrap();
        entry
            .game_state
            .as_mut()
            .unwrap()
            .sent
            .remove(&WORLD_TIME_SECONDS);
    }

    /// Unreliable GameState updates wait for every reliable bunch on the
    /// GameState channel (`actor_channel_settled`): while a record creation is
    /// unacknowledged, including after a NAK until its repair is ACKed, no
    /// clock update goes out; it resumes on the repair's ACK.
    #[test]
    fn clock_updates_pause_until_the_record_bunch_settles() {
        let mut lab = record_lab(true);
        let (socket, address, mut client) = record_peer(&mut lab, 1, "AuthoredClock");
        complete_record_player(&mut lab, address);
        lab.controller_call(address, ControllerCall::FriendsList);
        deliver_bunches(&mut lab, &socket, &mut client, |_| true);
        let channel = game_state_channel(&lab);
        force_clock_due(&mut lab, address);
        lab.update_game_state();
        assert_eq!(lab.stats.game_state_updates_sent, 1, "updates flow before");
        deliver_bunches(&mut lab, &socket, &mut client, |_| true);
        lab.finalise_connected_players();
        assert_eq!(lab.stats.connected_player_record_creations_sent, 1);
        assert!(!lab.peers[&address].peer.actor_channel_settled(channel));
        force_clock_due(&mut lab, address);
        lab.update_game_state();
        assert_eq!(lab.stats.game_state_updates_sent, 1, "paused while unacked");
        // Lose it: still paused until the repair is acknowledged.
        deliver_bunches(&mut lab, &socket, &mut client, |_| false);
        force_clock_due(&mut lab, address);
        lab.update_game_state();
        assert_eq!(lab.stats.game_state_updates_sent, 1, "paused after the NAK");
        let repair = lab
            .peers
            .get_mut(&address)
            .unwrap()
            .peer
            .ack_packet()
            .unwrap();
        assert!(lab.send(&repair, address));
        force_clock_due(&mut lab, address);
        lab.update_game_state();
        assert_eq!(lab.stats.game_state_updates_sent, 1, "paused during repair");
        deliver_bunches(&mut lab, &socket, &mut client, |_| true);
        assert!(lab.peers[&address].peer.actor_channel_settled(channel));
        force_clock_due(&mut lab, address);
        lab.update_game_state();
        assert_eq!(lab.stats.game_state_updates_sent, 2, "resumes on the ACK");
        assert_eq!(lab.stats.connected_player_record_creations_acked, 1);
    }

    #[test]
    fn each_connection_lists_only_records_whose_actors_are_open_on_it() {
        let mut lab = record_lab(true);
        let (first_socket, first, mut first_client) = record_peer(&mut lab, 1, "AuthoredFirst");
        let (second_socket, second, mut second_client) = record_peer(&mut lab, 2, "AuthoredSecond");
        complete_record_player(&mut lab, first);
        complete_record_player(&mut lab, second);
        // Both Begins requeried every registered controller (F-1051).
        lab.controller_call(first, ControllerCall::FriendsList);
        lab.finalise_connected_players();
        assert_eq!(lab.stats.players_finalised, 0);
        lab.controller_call(second, ControllerCall::FriendsList);
        lab.finalise_connected_players();
        assert_eq!(lab.stats.players_finalised, 2);
        // One record per peer; each connection gets only its own, because the
        // other player's pawn and PlayerState are not open on it.
        assert_eq!(lab.stats.connected_player_records_created, 2);
        assert_eq!(lab.stats.connected_player_record_creations_sent, 2);
        let first_index = lab.peers[&first].connected_player.as_ref().unwrap().index;
        let second_index = lab.peers[&second].connected_player.as_ref().unwrap().index;
        assert_ne!(first_index, second_index);
        assert_eq!(lab.peers[&first].connected_players_sent, [first_index]);
        assert_eq!(lab.peers[&second].connected_players_sent, [second_index]);
        for (socket, client) in [
            (&first_socket, &mut first_client),
            (&second_socket, &mut second_client),
        ] {
            let bunches = deliver_bunches(&mut lab, socket, client, |_| true);
            let [body] = record_bodies(&lab, &bunches).try_into().unwrap();
            let component = body.blocks.last().unwrap();
            assert_eq!(listed_records(&lab, component).len(), 1);
        }
        // A later joiner registers its own record; existing connections'
        // arrays do not change, so nothing is sent to them.
        let (third_socket, third, mut third_client) = record_peer(&mut lab, 3, "AuthoredThird");
        complete_record_player(&mut lab, third);
        for address in [first, second, third] {
            lab.controller_call(address, ControllerCall::FriendsList);
        }
        lab.finalise_connected_players();
        assert_eq!(lab.stats.players_finalised, 3);
        assert_eq!(lab.stats.connected_player_record_creations_sent, 3);
        assert_eq!(lab.stats.connected_player_array_updates_sent, 3);
        let third_index = lab.peers[&third].connected_player.as_ref().unwrap().index;
        assert_eq!(lab.peers[&third].connected_players_sent, [third_index]);
        for (socket, client) in [
            (&first_socket, &mut first_client),
            (&second_socket, &mut second_client),
        ] {
            let bunches = deliver_bunches(&mut lab, socket, client, |_| true);
            assert!(record_bodies(&lab, &bunches).is_empty());
        }
        let bunches = deliver_bunches(&mut lab, &third_socket, &mut third_client, |_| true);
        assert_eq!(record_bodies(&lab, &bunches).len(), 1);
        // A connection that listed a departing record gets the shrunk array
        // (array only, no record block, no sub-object destroy); the others
        // get nothing. The lab's visibility rule never lists another
        // player's record today, so the listing is authored here.
        lab.peers.get_mut(&first).unwrap().connected_players_sent = vec![first_index, second_index];
        lab.remove_peer(second).unwrap();
        assert_eq!(lab.stats.connected_player_records_removed, 1);
        assert!(lab.peers[&first].connected_players_pending);
        assert!(!lab.peers[&third].connected_players_pending);
        lab.retry_connected_player_records();
        assert_eq!(lab.stats.connected_player_array_updates_sent, 4);
        assert_eq!(lab.stats.connected_player_record_creations_sent, 3);
        let bunches = deliver_bunches(&mut lab, &first_socket, &mut first_client, |_| true);
        let [body] = record_bodies(&lab, &bunches).try_into().unwrap();
        let [actor, component] = body.blocks.as_slice() else {
            panic!("actor and component blocks only");
        };
        assert_eq!(actor.target, BlockTarget::Actor);
        let first_guid = lab
            .guid_cache
            .clone()
            .subobject_guid(ActorKey::Session(0), first_index)
            .unwrap();
        assert_eq!(listed_records(&lab, component), [first_guid]);
        assert_eq!(lab.peers[&first].connected_players_sent, [first_index]);
        assert!(deliver_bunches(&mut lab, &third_socket, &mut third_client, |_| true).is_empty());
        // Removal drops the record from the gauges (the cookie-replacement
        // reconnect is `reconnect_replaces_the_connected_player_record`).
        lab.remove_peer(third).unwrap();
        assert_eq!(lab.stats.connected_player_records_removed, 2);
        assert_eq!(
            lab.readiness_counts().connected_player_records.registered,
            1
        );
    }

    #[test]
    fn connected_player_record_pressure_retries_once_with_the_latest_state() {
        let mut lab = record_lab(true);
        let (socket, address, mut client) = record_peer(&mut lab, 1, "AuthoredPressure");
        complete_record_player(&mut lab, address);
        lab.controller_call(address, ControllerCall::FriendsList);
        deliver_bunches(&mut lab, &socket, &mut client, |_| true);
        let entry = lab.peers.get_mut(&address).unwrap();
        for _ in 0..entry.peer.config().sent_capacity {
            let datagram = entry.peer.ack_packet().unwrap();
            lab.socket.send_to(&datagram, address).unwrap();
            let mut bytes = [0; 2048];
            let n = socket.recv(&mut bytes).unwrap();
            let packet = super::super::packet::read_packet(
                &bytes[..n],
                &super::super::packet::PacketConfig {
                    receiver: super::super::packet::Receiver::Client,
                    ..lab.config.packet.packet
                },
            )
            .unwrap();
            let received = client.receive(&packet.header).unwrap().unwrap();
            client.ack_seq(received.sequence, true).unwrap();
        }
        lab.finalise_connected_players();
        assert_eq!(lab.stats.players_finalised, 1);
        assert_eq!(lab.stats.connected_player_record_sends_deferred, 1);
        assert_eq!(lab.stats.connected_player_record_creations_sent, 0);
        assert_eq!(
            lab.readiness_counts().connected_player_records.pending_send,
            1
        );
        // No packet progress: retries do not spin.
        for _ in 0..3 {
            lab.retry_connected_player_records();
        }
        assert_eq!(lab.stats.connected_player_record_sends_deferred, 1);
        // State changes while deferred are what the retry carries.
        lab.peers.get_mut(&address).unwrap().profiles.character_slot = Some(4);
        let bytes = super::super::packet::write_packet(
            &super::super::packet::Packet {
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
        assert_eq!(lab.stats.connected_player_record_creations_sent, 1);
        assert_eq!(lab.stats.connected_player_record_sends_deferred, 1);
        assert_eq!(lab.stats.connected_player_record_errors, 0);
        assert_eq!(
            lab.readiness_counts().connected_player_records.pending_send,
            0
        );
        let bunches = deliver_bunches(&mut lab, &socket, &mut client, |_| true);
        let [body] = record_bodies(&lab, &bunches).try_into().unwrap();
        let plan = lab.connected_players.clone().unwrap();
        let values = super::super::rep_layout::parse_rep_properties(
            &body.blocks[1].payload,
            &plan.record_layout,
            &record_codec(),
        )
        .unwrap();
        assert_eq!(values.entries[1].value, RepValue::Int(4));
        lab.retry_connected_player_records();
        assert_eq!(lab.stats.connected_player_record_creations_sent, 1);
        assert_eq!(lab.stats.connected_player_record_creations_acked, 1);
    }

    #[test]
    fn connected_player_record_nak_is_repaired_without_recomposition() {
        let mut lab = record_lab(true);
        let (socket, address, mut client) = record_peer(&mut lab, 1, "AuthoredRepair");
        complete_record_player(&mut lab, address);
        lab.controller_call(address, ControllerCall::FriendsList);
        deliver_bunches(&mut lab, &socket, &mut client, |_| true);
        lab.finalise_connected_players();
        assert_eq!(lab.stats.connected_player_record_creations_sent, 1);
        // Lose every datagram of the creation.
        let lost = deliver_bunches(&mut lab, &socket, &mut client, |_| false);
        let [original] = record_bodies(&lab, &lost).try_into().unwrap();
        assert!(lab.readiness_counts().resend_bunches_pending > 0);
        assert_eq!(lab.stats.connected_player_record_creations_acked, 0);
        assert_eq!(
            lab.readiness_counts()
                .connected_player_records
                .records_awaiting_ack,
            1
        );
        for _ in 0..3 {
            lab.mark_connected_players_due(|_| true);
            lab.retry_connected_player_records();
        }
        assert_eq!(lab.stats.connected_player_record_creations_sent, 1);
        assert_eq!(lab.stats.connected_player_array_updates_sent, 1);
        // The reliable layer resends the same bunch unchanged.
        let repair = lab
            .peers
            .get_mut(&address)
            .unwrap()
            .peer
            .ack_packet()
            .unwrap();
        assert!(lab.send(&repair, address));
        let repaired = deliver_bunches(&mut lab, &socket, &mut client, |_| true);
        let [resent] = record_bodies(&lab, &repaired).try_into().unwrap();
        assert_eq!(resent, original);
        assert_eq!(lab.readiness_counts().resend_bunches_pending, 0);
        assert_eq!(lab.stats.connected_player_record_creations_acked, 1);
        // A later ACK does not count the creation again.
        let more = lab
            .peers
            .get_mut(&address)
            .unwrap()
            .peer
            .ack_packet()
            .unwrap();
        assert!(lab.send(&more, address));
        deliver_bunches(&mut lab, &socket, &mut client, |_| true);
        assert_eq!(lab.stats.connected_player_record_creations_acked, 1);
        assert_eq!(lab.stats.connected_player_record_creations_sent, 1);
        assert_eq!(
            lab.readiness_counts().connected_player_records,
            ConnectedPlayerRecordReadiness {
                registered: 1,
                finalised: 1,
                creations_queued: 1,
                creations_settled: 1,
                ..Default::default()
            }
        );
    }

    /// Every bunch the client receives in one authored join-to-finalisation
    /// run, by channel; GameState payloads, which carry the lab clock, by
    /// length only.
    fn record_scenario(mode: Option<bool>) -> Vec<(u32, bool, bool, bool, usize, Vec<u8>)> {
        let mut lab = record_lab(mode.is_some());
        if mode == Some(false) {
            lab.disable_connected_player_records().unwrap();
        }
        let channel = game_state_channel(&lab);
        let (socket, address, mut client) = record_peer(&mut lab, 1, "AuthoredScenario");
        let mut bunches = Vec::new();
        complete_record_player(&mut lab, address);
        bunches.extend(deliver_bunches(&mut lab, &socket, &mut client, |_| true));
        lab.controller_call(address, ControllerCall::FriendsList);
        lab.finalise_connected_players();
        lab.retry_connected_player_records();
        bunches.extend(deliver_bunches(&mut lab, &socket, &mut client, |_| true));
        bunches
            .into_iter()
            .map(|bunch| {
                let header = &bunch.header;
                let bytes = if header.channel_index == channel {
                    Vec::new()
                } else {
                    bunch.payload.bytes().to_vec()
                };
                (
                    header.channel_index,
                    header.reliable,
                    header.open,
                    header.has_package_map_exports,
                    bunch.payload.bit_len(),
                    bytes,
                )
            })
            .collect()
    }

    /// Two sequential record sessions on one lab, the first removed before
    /// the second begins: each session's bunches (GameState-channel bunches
    /// by shape, block targets and listed records, since their world times
    /// vary) and the GUIDs it was given.
    fn sequential_record_sessions() -> Vec<(Vec<String>, u32, u32)> {
        let mut lab = record_lab(true);
        let channel = game_state_channel(&lab);
        let mut sessions = Vec::new();
        for (connection, user) in [(1, "AuthoredFirst"), (2, "AuthoredSecond")] {
            let (socket, address, mut client) = record_peer(&mut lab, connection, user);
            let mut bunches = Vec::new();
            complete_record_player(&mut lab, address);
            bunches.extend(deliver_bunches(&mut lab, &socket, &mut client, |_| true));
            lab.controller_call(address, ControllerCall::FriendsList);
            lab.finalise_connected_players();
            lab.retry_connected_player_records();
            bunches.extend(deliver_bunches(&mut lab, &socket, &mut client, |_| true));
            assert_eq!(lab.peers[&address].connected_player_creations_settled, 1);
            let record = record_bodies(&lab, &bunches)
                .into_iter()
                .flat_map(|body| body.blocks)
                .find_map(|block| match block.target {
                    BlockTarget::SubObject { object, .. } => Some(object.guid),
                    BlockTarget::Actor => None,
                })
                .unwrap();
            let pawn = lab.peers[&address].controller_startup_pawn_guid.unwrap();
            let shapes = bunches
                .iter()
                .map(|bunch| {
                    let header = &bunch.header;
                    let content = if header.channel_index != channel {
                        format!("{:02x?}", bunch.payload.bytes())
                    } else if header.reliable && !header.open && !header.has_package_map_exports {
                        let body = super::super::actor_channel::read_actor_bunch_body(
                            &mut bunch.payload.reader(),
                            false,
                            &actor_codec(17, super::super::archive::ByteOrder::Little),
                        )
                        .unwrap();
                        let listed = body
                            .blocks
                            .last()
                            .map(|block| listed_records(&lab, block))
                            .unwrap_or_default();
                        let targets: Vec<_> = body
                            .blocks
                            .iter()
                            .map(|block| match &block.target {
                                BlockTarget::Actor => None,
                                BlockTarget::SubObject { object, .. } => Some(object.guid),
                            })
                            .collect();
                        format!("{targets:?} {listed:?}")
                    } else {
                        String::new()
                    };
                    format!(
                        "{} {} {} {} {} {content}",
                        header.channel_index,
                        header.reliable,
                        header.open,
                        header.has_package_map_exports,
                        bunch.payload.bit_len()
                    )
                })
                .collect();
            sessions.push((shapes, record, pawn));
            lab.remove_peer(address).unwrap();
        }
        sessions
    }

    /// Releasing a departed session's cached identities changes no bunch
    /// of the first session, nor of a session after a departure: the
    /// digests were recorded before the release was introduced. GUIDs are
    /// never reused across sessions.
    #[test]
    fn sequential_sessions_send_the_same_bunches_after_identity_release() {
        let sessions = sequential_record_sessions();
        assert_eq!(sessions, sequential_record_sessions(), "deterministic");
        let digests: Vec<String> = sessions
            .iter()
            .map(|(shapes, ..)| {
                let mut hasher = Sha256::new();
                for shape in shapes {
                    hasher.update(shape.as_bytes());
                    hasher.update(b"\n");
                }
                hasher
                    .finalize()
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect()
            })
            .collect();
        // Recorded at c946063, before departed identities were released.
        assert_eq!(
            digests,
            [
                "8ac0fb3576ddeb09435ccbe9d43e66ba15e35ff922dd1735e055acc047ed5c32",
                "9a18acaf7b90592cc21d8efe0f0b5b0d49cfff3e0c038a2e5e82844fa928870d",
            ]
        );
        let [
            (_, first_record, first_pawn),
            (_, second_record, second_pawn),
        ] = sessions.as_slice()
        else {
            panic!("two sessions");
        };
        assert_ne!(first_record, second_record);
        assert_ne!(first_pawn, second_pawn);
        assert!(second_record > first_record && second_pawn > first_pawn);
    }

    #[test]
    fn disabled_connected_player_records_send_exactly_the_previous_bunches() {
        let before = record_scenario(None);
        let disabled = record_scenario(Some(false));
        assert!(!before.is_empty());
        assert_eq!(disabled, before);
        // Enabled, the run differs only by the GameState-channel record
        // bunches: every other bunch is unchanged and in the same order.
        let enabled = record_scenario(Some(true));
        let channel = game_state_channel(&record_lab(false));
        let (records, others): (Vec<_>, Vec<_>) = enabled
            .into_iter()
            .partition(|bunch| bunch.0 == channel && bunch.1 && !bunch.2);
        assert_eq!(others, before);
        assert_eq!(
            records.iter().map(|bunch| bunch.3).collect::<Vec<_>>(),
            [true, false]
        );
        let mut lab = record_lab(true);
        let (_socket, address, _client) = record_peer(&mut lab, 1, "AuthoredLate");
        assert!(lab.disable_connected_player_records().is_err());
        assert!(
            lab.enable_connected_player_records(
                ReleaseProfile::release_3_0_35()
                    .connected_players_plan()
                    .unwrap()
                    .unwrap()
            )
            .is_err()
        );
        assert!(lab.has_connected_player_records());
        let _ = address;
    }

    #[test]
    fn connected_player_records_require_the_referenced_opens_and_a_valid_plan() {
        let plan = ReleaseProfile::release_3_0_35()
            .connected_players_plan()
            .unwrap()
            .unwrap();
        // The GameState and controller, but no pawn or PlayerState open.
        assert!(
            prospect_refresh_lab()
                .enable_connected_player_records(plan.clone())
                .is_err()
        );
        let mut lab = record_lab(false);
        let mut bad = plan.clone();
        bad.record_class[0].checksum = 0;
        assert!(lab.enable_connected_player_records(bad).is_err());
        let mut bad = plan.clone();
        bad.record_layout.pop();
        assert!(lab.enable_connected_player_records(bad).is_err());
        assert!(!lab.has_connected_player_records());
        lab.enable_connected_player_records(plan).unwrap();
        assert!(lab.has_connected_player_records());
    }

    /// [`record_lab`] with controller possession properties on.
    fn possession_lab(records: bool) -> PeerLab {
        let mut lab = record_lab(records);
        lab.enable_controller_possession_properties().unwrap();
        lab
    }

    /// A joined peer like [`record_peer`] on a fresh loopback socket, before
    /// any opens: the caller drives the initial opens.
    fn possession_identity_peer(
        lab: &mut PeerLab,
        connection: u32,
        user: &str,
    ) -> (
        UdpSocket,
        SocketAddrV4,
        super::super::sequencing::PacketSequencer,
    ) {
        use super::super::control_messages::{NetIdType, NetIdValue, UniqueNetId};
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        socket.connect(lab.local_addr().unwrap()).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        let (address, client) = handshake_peer_on(lab, &socket);
        let entry = lab.peers.get_mut(&address).unwrap();
        entry.connection_id = Some(connection);
        entry.player = Some(RegisteredPlayer {
            player_id: connection as i32,
            name_units: "AUTHORED-POSSESSION".encode_utf16().collect(),
        });
        entry.login_unique_id = Some(UniqueNetId {
            kind: NetIdType::Named(ArchiveText::text("NULL").unwrap()),
            value: NetIdValue::Text(ArchiveText::text(user).unwrap()),
        });
        entry.profiles.character_slot = Some(3);
        (socket, address, client)
    }

    /// [`record_peer`], returning every bunch of its real initial-open
    /// sequence (all delivered and ACKed).
    fn possession_peer(
        lab: &mut PeerLab,
        connection: u32,
        user: &str,
    ) -> (
        UdpSocket,
        SocketAddrV4,
        super::super::sequencing::PacketSequencer,
        Vec<super::super::packet::Bunch>,
    ) {
        let (socket, address, mut client) = possession_identity_peer(lab, connection, user);
        assert!(lab.send_player_controller_open(address));
        let bunches = deliver_bunches(lab, &socket, &mut client, |_| true);
        (socket, address, client, bunches)
    }

    /// Whether `bunch` has the possession update's framing: a whole,
    /// reliable, nonopening controller-channel bunch without exports. During
    /// the initial opens no other controller bunch has it.
    fn possession_shaped(bunch: &super::super::packet::Bunch) -> bool {
        let header = &bunch.header;
        header.channel_index == super::super::session_open::PLAYER_CONTROLLER_CHANNEL_INDEX
            && header.reliable
            && !header.open
            && !header.partial
            && !header.has_package_map_exports
    }

    /// The possession updates among `bunches`: their positions and their
    /// decoded `(handle, value)` entries, read with the repository decoder
    /// against the controller open's own layout. Other controller bunches
    /// (RPC fields, no property block) are skipped.
    fn possession_updates(
        lab: &PeerLab,
        bunches: &[super::super::packet::Bunch],
    ) -> Vec<(usize, Vec<(u32, RepValue)>)> {
        let plan = lab.player_controller_open.as_ref().unwrap();
        let codec = RepLayoutCodec {
            actor: actor_codec(
                plan.engine_net_version,
                super::super::archive::ByteOrder::Little,
            ),
            checksums: false,
            limits: Default::default(),
        };
        bunches
            .iter()
            .enumerate()
            .filter(|(_, bunch)| possession_shaped(bunch))
            .filter_map(|(index, bunch)| {
                let mut reader = bunch.payload.reader();
                let body = super::super::actor_channel::read_actor_bunch_body(
                    &mut reader,
                    false,
                    &codec.actor,
                )
                .unwrap();
                assert!(reader.at_end());
                let [block] = body.blocks.as_slice() else {
                    return None;
                };
                if !block.has_rep_layout {
                    return None;
                }
                assert!(body.spawn.is_none());
                assert_eq!(block.target, BlockTarget::Actor);
                let stream = super::super::rep_layout::parse_rep_properties(
                    &block.payload,
                    &plan.actor_layout,
                    &codec,
                )
                .unwrap();
                Some((
                    index,
                    stream
                        .entries
                        .into_iter()
                        .map(|entry| (entry.handle, entry.value))
                        .collect(),
                ))
            })
            .collect()
    }

    fn possession_entries(pawn: u32) -> Vec<(u32, RepValue)> {
        let reference = RepValue::Object(super::super::actor_channel::NetGuidRef::plain(pawn));
        vec![(17, reference.clone()), (20, reference)]
    }

    fn pawn_open_position(bunches: &[super::super::packet::Bunch]) -> usize {
        bunches
            .iter()
            .position(|bunch| {
                bunch.header.channel_index == super::super::pawn_open::PAWN_CHANNEL_INDEX
                    && bunch.header.open
            })
            .unwrap()
    }

    /// The real initial opens send one update right after the pawn open,
    /// naming the pawn as handles 17 and 20 of the survival controller's
    /// layout; it re-encodes exactly, is ACKed once and is never repeated
    /// by finalisation, the record or later passes.
    #[test]
    fn controller_possession_update_follows_the_pawn_open_once_and_decodes() {
        let mut lab = possession_lab(true);
        let (socket, address, mut client, bunches) =
            possession_peer(&mut lab, 1, "AuthoredPossession");
        let pawn = lab.peers[&address].controller_startup_pawn_guid.unwrap();
        let [(position, entries)] = possession_updates(&lab, &bunches).try_into().unwrap();
        assert_eq!(entries, possession_entries(pawn));
        assert!(position > pawn_open_position(&bunches));
        // GameState and PlayerState opens follow it.
        let game_state = game_state_channel(&lab);
        assert!(
            bunches[position..]
                .iter()
                .any(|bunch| bunch.header.channel_index == game_state && bunch.header.open)
        );
        // The survival controller's 30-handle layout; exact re-encoding.
        let plan = lab.player_controller_open.clone().unwrap();
        assert_eq!(plan.actor_layout.len(), 30);
        let codec = RepLayoutCodec {
            actor: actor_codec(
                plan.engine_net_version,
                super::super::archive::ByteOrder::Little,
            ),
            checksums: false,
            limits: Default::default(),
        };
        let composed = super::super::session_open::compose_controller_possession_update(
            pawn,
            &plan.actor_layout,
            &codec,
        )
        .unwrap();
        assert_eq!(composed.bytes(), bunches[position].payload.bytes());
        assert_eq!(composed.bit_len(), bunches[position].payload.bit_len());
        let body = super::super::actor_channel::read_actor_bunch_body(
            &mut composed.reader(),
            false,
            &codec.actor,
        )
        .unwrap();
        let reencoded = encode_actor_bunch_body(&body, &codec.actor).unwrap();
        assert_eq!(reencoded.bytes(), composed.bytes());
        assert_eq!(lab.stats.controller_possession_updates_sent, 1);
        assert_eq!(lab.stats.controller_possession_updates_acked, 1);
        assert_eq!(lab.stats.controller_possession_sends_deferred, 0);
        assert_eq!(lab.stats.controller_possession_errors, 0);
        let settled = ControllerPossessionReadiness {
            queued: 1,
            settled: 1,
            ..Default::default()
        };
        assert_eq!(lab.readiness_counts().controller_possession, settled);
        // Begin, completion, finalisation and the record add none.
        complete_record_player(&mut lab, address);
        lab.controller_call(address, ControllerCall::FriendsList);
        lab.finalise_connected_players();
        lab.controller_possession_due(address);
        lab.retry_controller_possession();
        let later = deliver_bunches(&mut lab, &socket, &mut client, |_| true);
        assert!(possession_updates(&lab, &later).is_empty());
        assert_eq!(record_bodies(&lab, &later).len(), 1);
        assert_eq!(lab.stats.connected_player_record_creations_sent, 1);
        assert_eq!(lab.stats.controller_possession_updates_sent, 1);
        assert_eq!(lab.stats.controller_possession_updates_acked, 1);
        let counts = lab.session_counts();
        assert_eq!(counts.controller_possession_updates_sent, 1);
        assert_eq!(counts.controller_possession_updates_acked, 1);
        assert_eq!(counts.controller_possession_sends_deferred, 0);
        assert_eq!(counts.controller_possession_errors, 0);
        lab.remove_peer(address).unwrap();
        assert_eq!(
            lab.readiness_counts().controller_possession,
            ControllerPossessionReadiness::default()
        );
    }

    /// Nothing is due before the pawn open commits; a pawn that opens later
    /// is followed by the update then.
    #[test]
    fn controller_possession_waits_for_the_pawn_open() {
        let mut lab = possession_lab(false);
        let (socket, address, mut client) =
            possession_identity_peer(&mut lab, 1, "AuthoredLatePawn");
        assert!(matches!(
            lab.send_initial_controller_open(address),
            InitialOpenOutcome::Committed(_)
        ));
        lab.retry_controller_possession();
        let opened = deliver_bunches(&mut lab, &socket, &mut client, |_| true);
        assert!(possession_updates(&lab, &opened).is_empty());
        assert_eq!(lab.stats.controller_possession_updates_sent, 0);
        assert_eq!(
            lab.readiness_counts().controller_possession,
            ControllerPossessionReadiness::default()
        );
        let (controller, player_state) = lab.peers[&address].controller_open_guids.unwrap();
        assert!(matches!(
            lab.send_pawn_open(address, controller, player_state),
            InitialOpenOutcome::Committed(_)
        ));
        assert_eq!(lab.stats.controller_possession_updates_sent, 1);
        let bunches = deliver_bunches(&mut lab, &socket, &mut client, |_| true);
        let pawn = lab.peers[&address].controller_startup_pawn_guid.unwrap();
        let [(position, entries)] = possession_updates(&lab, &bunches).try_into().unwrap();
        assert_eq!(entries, possession_entries(pawn));
        assert!(position > pawn_open_position(&bunches));
        assert_eq!(lab.stats.controller_possession_updates_acked, 1);
    }

    /// A finalised player whose controller possession update is held back by
    /// genuine transport pressure, so its connected-player record is held
    /// too. The real opens run stage by stage (controller, GameState,
    /// PlayerState); Begin, both initialisation answers and the friends list
    /// arrive before the pawn opens. Then the controller channel's reliable
    /// window is filled with authored bunches whose packets the client NAKs:
    /// they stay unacknowledged (the window is full) while the sent ring
    /// empties. The real pawn open then commits, the update is refused as
    /// drainable window pressure and defers, the PlayerState completion goes
    /// out on its own channel, and finalisation makes the record due.
    ///
    /// Sent-ring pressure cannot produce this state: completion needs a ring
    /// slot as well, and every pass admits the update before completion.
    fn held_possession_peer(
        lab: &mut PeerLab,
        user: &str,
    ) -> (
        UdpSocket,
        SocketAddrV4,
        super::super::sequencing::PacketSequencer,
    ) {
        use super::super::packet_lab::MAX_PENDING_RELIABLE;
        let (socket, address, mut client) = possession_identity_peer(lab, 1, user);
        assert!(matches!(
            lab.send_initial_controller_open(address),
            InitialOpenOutcome::Committed(_)
        ));
        assert!(matches!(
            lab.send_game_state_open(address),
            InitialOpenOutcome::Committed(_)
        ));
        let (controller, player_state) = lab.peers[&address].controller_open_guids.unwrap();
        assert!(matches!(
            lab.send_player_state_open(address, controller, player_state),
            InitialOpenOutcome::Committed(_)
        ));
        ack_startup_packets(lab, &socket, &mut client);
        lab.controller_call(address, ControllerCall::Begin(3));
        let entry = lab.peers.get_mut(&address).unwrap();
        entry.profiles.outstanding = None;
        entry.profiles.has_loadout = true;
        entry.profiles.has_bestiary = true;
        lab.controller_call(address, ControllerCall::FriendsList);
        deliver_bunches(lab, &socket, &mut client, |_| true);
        assert!(!lab.peers[&address].profiles.completed, "no pawn yet");
        let channel = lab.peers[&address].player_controller_channel.unwrap();
        let ring = |lab: &PeerLab| {
            lab.peers[&address]
                .peer
                .sequencer()
                .notify()
                .sent_records()
                .len()
        };
        for _ in 0..2 * MAX_PENDING_RELIABLE {
            let entry = lab.peers.get_mut(&address).unwrap();
            if entry.peer.pending_actor_reliable(channel) == Some(MAX_PENDING_RELIABLE) {
                break;
            }
            let datagram = entry
                .peer
                .reliable_actor_bunch(
                    channel,
                    Payload::from_writer(super::super::packet::BitWriter::new()),
                )
                .unwrap();
            assert!(lab.send(&datagram, address));
            if ring(lab) >= 32 {
                notify_startup_packets(lab, &socket, &mut client, |_| false);
            }
        }
        if ring(lab) > 0 {
            notify_startup_packets(lab, &socket, &mut client, |_| false);
        }
        assert_eq!(
            lab.peers[&address].peer.pending_actor_reliable(channel),
            Some(MAX_PENDING_RELIABLE)
        );
        assert_eq!(ring(lab), 0);
        assert!(matches!(
            lab.send_pawn_open(address, controller, player_state),
            InitialOpenOutcome::Committed(_)
        ));
        assert_eq!(lab.stats.controller_possession_sends_deferred, 1);
        assert_eq!(lab.stats.controller_possession_updates_sent, 0);
        assert!(
            lab.peers[&address].profiles.completed,
            "completion went out"
        );
        lab.finalise_connected_players();
        assert_eq!(lab.stats.players_finalised, 1);
        assert_eq!(lab.stats.connected_player_record_creations_sent, 0);
        assert_eq!(
            lab.readiness_counts().controller_possession,
            ControllerPossessionReadiness {
                pending_send: 1,
                ..Default::default()
            }
        );
        (socket, address, client)
    }

    /// ACK progress for [`held_possession_peer`]: the client acknowledges
    /// everything outstanding and the reliable layer repairs the NAKed
    /// controller bunches until the controller window has room. Only packet
    /// notifications are processed; no retry pass runs.
    fn drain_controller_window(
        lab: &mut PeerLab,
        socket: &UdpSocket,
        client: &mut super::super::sequencing::PacketSequencer,
        address: SocketAddrV4,
    ) {
        use super::super::packet_lab::MAX_PENDING_RELIABLE;
        let channel = lab.peers[&address].player_controller_channel.unwrap();
        deliver_bunches(lab, socket, client, |_| true);
        for _ in 0..MAX_PENDING_RELIABLE {
            if lab.peers[&address].peer.pending_actor_reliable(channel) < Some(MAX_PENDING_RELIABLE)
            {
                assert!(lab.peers[&address].controller_possession_retry_ready);
                return;
            }
            let repair = lab
                .peers
                .get_mut(&address)
                .unwrap()
                .peer
                .ack_packet()
                .unwrap();
            assert!(lab.send(&repair, address));
            deliver_bunches(lab, socket, client, |_| true);
        }
        panic!("the controller window did not drain");
    }

    /// The real sequence with records and possession properties on: the
    /// update defers at the pawn open under genuine window pressure, the
    /// player completes and finalises meanwhile and its record is held;
    /// passes without packet progress admit nothing; ACK progress then
    /// admits the update, and the record follows once the client has ACKed
    /// the update (F-1116), in that order on the wire, each exactly once.
    #[test]
    fn the_record_follows_a_possession_update_held_by_transport_pressure() {
        let mut lab = possession_lab(true);
        let (socket, address, mut client) = held_possession_peer(&mut lab, "AuthoredHeld");
        for _ in 0..3 {
            lab.retry_controller_possession();
            lab.finalise_connected_players();
            lab.retry_connected_player_records();
        }
        assert_eq!(lab.stats.controller_possession_sends_deferred, 1);
        assert_eq!(lab.stats.controller_possession_updates_sent, 0);
        assert_eq!(lab.stats.connected_player_record_creations_sent, 0);
        drain_controller_window(&mut lab, &socket, &mut client, address);
        assert_eq!(
            lab.stats.controller_possession_updates_sent, 0,
            "no pass yet"
        );
        lab.poll();
        assert_eq!(lab.stats.controller_possession_updates_sent, 1);
        assert_eq!(
            lab.stats.connected_player_record_creations_sent, 0,
            "admitted, not yet ACKed"
        );
        // The client applies the controller channel in order, so the NAKed
        // filler bunches ahead of the update must be repaired and ACKed too.
        let mut bunches = deliver_bunches(&mut lab, &socket, &mut client, |_| true);
        for _ in 0..super::super::packet_lab::MAX_PENDING_RELIABLE {
            lab.poll();
            if lab.stats.connected_player_record_creations_sent > 0 {
                break;
            }
            let repair = lab
                .peers
                .get_mut(&address)
                .unwrap()
                .peer
                .ack_packet()
                .unwrap();
            assert!(lab.send(&repair, address));
            bunches.extend(deliver_bunches(&mut lab, &socket, &mut client, |_| true));
        }
        assert_eq!(lab.stats.connected_player_record_creations_sent, 1);
        assert_eq!(lab.stats.connected_player_array_updates_sent, 1);
        bunches.extend(deliver_bunches(&mut lab, &socket, &mut client, |_| true));
        let pawn = lab.peers[&address].controller_startup_pawn_guid.unwrap();
        let [(update, entries)] = possession_updates(&lab, &bunches).try_into().unwrap();
        assert_eq!(entries, possession_entries(pawn));
        let channel = game_state_channel(&lab);
        let record = bunches
            .iter()
            .position(|bunch| {
                bunch.header.channel_index == channel && bunch.header.reliable && !bunch.header.open
            })
            .unwrap();
        assert!(update < record, "the update leaves before the record");
        assert_eq!(record_bodies(&lab, &bunches).len(), 1);
        // Later passes add nothing; each settled exactly once.
        for _ in 0..3 {
            lab.poll();
        }
        deliver_bunches(&mut lab, &socket, &mut client, |_| true);
        assert_eq!(
            (
                lab.stats.controller_possession_updates_sent,
                lab.stats.controller_possession_updates_acked,
                lab.stats.controller_possession_sends_deferred,
                lab.stats.controller_possession_errors,
            ),
            (1, 1, 1, 0)
        );
        assert_eq!(
            (
                lab.stats.connected_player_record_creations_sent,
                lab.stats.connected_player_record_creations_acked,
                lab.stats.connected_player_array_updates_sent,
                lab.stats.connected_player_record_errors,
            ),
            (1, 1, 1, 0)
        );
        assert_eq!(
            lab.readiness_counts().controller_possession,
            ControllerPossessionReadiness {
                queued: 1,
                settled: 1,
                ..Default::default()
            }
        );
    }

    /// Abandonment because the controller or pawn channel is gone when the
    /// deferred update comes due again. The deferral and the ACK progress are
    /// real; the close is driven on the connection's transport
    /// (`LabPeer::reliable_actor_close`), because the session never closes
    /// its own controller or pawn channel through any public call, so this
    /// branch is not reachable through the public sequence. The error counts
    /// once and the update is never retried. Its record is never sent: the
    /// session is closed and removed in the same poll (F-1116), with its
    /// record, and no record error is counted.
    #[test]
    fn an_update_whose_channel_closed_is_abandoned_once() {
        for close_pawn in [true, false] {
            let mut lab = possession_lab(true);
            let (socket, address, mut client) = held_possession_peer(&mut lab, "AuthoredClosed");
            drain_controller_window(&mut lab, &socket, &mut client, address);
            let entry = lab.peers.get_mut(&address).unwrap();
            let channel = if close_pawn {
                super::super::pawn_open::PAWN_CHANNEL_INDEX
            } else {
                entry.player_controller_channel.unwrap()
            };
            let close = entry.peer.reliable_actor_close(channel, 1).unwrap();
            assert!(lab.send(&close, address));
            lab.poll();
            assert_eq!(
                lab.stats.controller_possession_errors, 1,
                "pawn={close_pawn}"
            );
            assert!(!lab.has_peer(address), "pawn={close_pawn}");
            assert_eq!(lab.stats.peers_closed_by_possession_abandoned, 1);
            assert_eq!(lab.session_counts().closed_by_possession_abandoned, 1);
            assert_eq!(lab.stats.connected_player_records_removed, 1);
            for _ in 0..3 {
                lab.poll();
                lab.controller_possession_due(address);
            }
            assert_eq!(
                (
                    lab.stats.controller_possession_updates_sent,
                    lab.stats.controller_possession_sends_deferred,
                    lab.stats.controller_possession_errors,
                    lab.stats.peers_closed_by_possession_abandoned,
                ),
                (0, 1, 1, 1),
                "pawn={close_pawn}"
            );
            assert_eq!(lab.stats.connected_player_record_creations_sent, 0);
            assert_eq!(lab.stats.connected_player_record_errors, 0);
            assert_eq!(lab.readiness_counts(), ReadinessCounts::default());
        }
    }

    /// Abandonment by a refusal that cannot drain while the peer stays
    /// usable, here a composition refusal. A lab's controller plan is fixed
    /// before admission and composes for every pawn GUID, so this cannot
    /// happen through the public sequence: the plan's layout is truncated by
    /// hand after the real deferral. The error counts once, the update is
    /// never retried, the held record is never sent, and the session is
    /// removed in the same pass (F-1116). As for an actor channel failure,
    /// a control close goes out only to a client that has sent a control
    /// bunch; this authored client has not, so none does.
    #[test]
    fn an_update_that_cannot_be_composed_is_abandoned_and_closes_the_session() {
        use super::super::packet::{PacketConfig, Receiver, read_packet};
        let mut lab = possession_lab(true);
        let (socket, address, mut client) = held_possession_peer(&mut lab, "AuthoredRefused");
        lab.player_controller_open
            .as_mut()
            .unwrap()
            .actor_layout
            .truncate(16);
        drain_controller_window(&mut lab, &socket, &mut client, address);
        lab.poll();
        assert_eq!(lab.stats.controller_possession_errors, 1);
        assert_eq!(lab.stats.controller_possession_updates_sent, 0);
        assert_eq!(lab.stats.connected_player_record_creations_sent, 0);
        assert!(!lab.has_peer(address));
        assert_eq!(lab.stats.peers_closed_by_possession_abandoned, 1);
        assert_eq!(lab.stats.connected_player_records_removed, 1);
        // Every datagram was sent before `poll` returned, so a nonblocking
        // read drains them all: neither an update nor a record nor a close.
        let config = PacketConfig {
            receiver: Receiver::Client,
            ..lab.config.packet.packet
        };
        socket.set_nonblocking(true).unwrap();
        let mut bunches = Vec::new();
        let mut bytes = [0; 2048];
        while let Ok(length) = socket.recv(&mut bytes) {
            bunches.extend(read_packet(&bytes[..length], &config).unwrap().bunches);
        }
        assert!(!bunches.iter().any(|bunch| bunch.header.close));
        assert!(possession_updates(&lab, &bunches).is_empty());
        assert!(record_bodies(&lab, &bunches).is_empty());
        for _ in 0..3 {
            lab.poll();
            lab.controller_possession_due(address);
        }
        assert_eq!(
            (
                lab.stats.controller_possession_updates_sent,
                lab.stats.controller_possession_sends_deferred,
                lab.stats.controller_possession_errors,
                lab.stats.peers_failed,
                lab.stats.peers_closed_by_possession_abandoned,
            ),
            (0, 1, 1, 0, 1)
        );
        assert_eq!(lab.stats.connected_player_record_creations_sent, 0);
        assert_eq!(lab.readiness_counts(), ReadinessCounts::default());
    }

    /// Abandonment on a failed transport, which also removes the peer. Not
    /// reachable through the public sequence: every call that fails a
    /// connection's transport (a receive error, a send-side invariant)
    /// removes that peer at once, so a deferred update never meets a failed
    /// transport. The test-only `LabPeer::fail_for_test` fails it between
    /// passes after the real deferral and ACK progress. The error counts
    /// once, the peer is removed as failed with its record, and nothing is
    /// sent.
    #[test]
    fn an_update_on_a_failed_transport_is_abandoned_and_removes_the_peer() {
        let mut lab = possession_lab(true);
        let (socket, address, mut client) = held_possession_peer(&mut lab, "AuthoredFailed");
        drain_controller_window(&mut lab, &socket, &mut client, address);
        lab.peers.get_mut(&address).unwrap().peer.fail_for_test();
        lab.retry_controller_possession();
        assert_eq!(lab.stats.controller_possession_errors, 1);
        assert_eq!(lab.stats.peers_failed, 1);
        assert!(!lab.has_peer(address));
        assert_eq!(lab.stats.connected_player_records_removed, 1);
        for _ in 0..3 {
            lab.retry_controller_possession();
            lab.poll();
        }
        assert_eq!(
            (
                lab.stats.controller_possession_updates_sent,
                lab.stats.controller_possession_errors,
                lab.stats.connected_player_record_creations_sent,
            ),
            (0, 1, 0)
        );
        assert_eq!(lab.readiness_counts(), ReadinessCounts::default());
    }

    /// Datagrams the real pawn open takes after an ACKed controller open.
    fn pawn_open_datagrams() -> usize {
        let mut lab = record_lab(false);
        let (socket, address, mut client) =
            possession_identity_peer(&mut lab, 1, "AuthoredPressure");
        assert!(matches!(
            lab.send_initial_controller_open(address),
            InitialOpenOutcome::Committed(_)
        ));
        ack_startup_packets(&mut lab, &socket, &mut client);
        let ring = |lab: &PeerLab| {
            lab.peers[&address]
                .peer
                .sequencer()
                .notify()
                .sent_records()
                .len()
        };
        let before = ring(&lab);
        let (controller, player_state) = lab.peers[&address].controller_open_guids.unwrap();
        assert!(matches!(
            lab.send_pawn_open(address, controller, player_state),
            InitialOpenOutcome::Committed(_)
        ));
        ring(&lab) - before
    }

    /// The sent ring fills right behind the pawn open: the update defers,
    /// does not spin without packet progress, and after an ACK goes out
    /// once, composed from the current pawn reference.
    #[test]
    fn controller_possession_pressure_retries_once_with_the_current_pawn() {
        use super::super::packet::{Packet, PacketConfig, Receiver, read_packet, write_packet};
        let pawn_datagrams = pawn_open_datagrams();
        let mut lab = possession_lab(false);
        let (socket, address, mut client) =
            possession_identity_peer(&mut lab, 1, "AuthoredPressure");
        assert!(matches!(
            lab.send_initial_controller_open(address),
            InitialOpenOutcome::Committed(_)
        ));
        ack_startup_packets(&mut lab, &socket, &mut client);
        let config = PacketConfig {
            receiver: Receiver::Client,
            ..lab.config.packet.packet
        };
        let entry = lab.peers.get_mut(&address).unwrap();
        let free = entry.peer.config().sent_capacity
            - entry.peer.sequencer().notify().sent_records().len();
        for _ in 0..free - pawn_datagrams {
            let entry = lab.peers.get_mut(&address).unwrap();
            let datagram = entry.peer.ack_packet().unwrap();
            lab.socket.send_to(&datagram, address).unwrap();
            let mut bytes = [0; 2048];
            let length = socket.recv(&mut bytes).unwrap();
            let packet = read_packet(&bytes[..length], &config).unwrap();
            let received = client.receive(&packet.header).unwrap().unwrap();
            client.ack_seq(received.sequence, true).unwrap();
        }
        let (controller, player_state) = lab.peers[&address].controller_open_guids.unwrap();
        assert!(matches!(
            lab.send_pawn_open(address, controller, player_state),
            InitialOpenOutcome::Committed(_)
        ));
        assert_eq!(lab.stats.controller_possession_sends_deferred, 1);
        assert_eq!(lab.stats.controller_possession_updates_sent, 0);
        assert_eq!(
            lab.readiness_counts().controller_possession,
            ControllerPossessionReadiness {
                pending_send: 1,
                ..Default::default()
            }
        );
        for _ in 0..3 {
            lab.retry_controller_possession();
        }
        assert_eq!(lab.stats.controller_possession_sends_deferred, 1);
        // The client's ACK of the filler packets releases the ring.
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
        assert_eq!(lab.stats.controller_possession_updates_sent, 1);
        assert_eq!(lab.stats.controller_possession_sends_deferred, 1);
        assert_eq!(lab.stats.controller_possession_errors, 0);
        assert_eq!(lab.readiness_counts().controller_possession.pending_send, 0);
        let bunches = deliver_bunches(&mut lab, &socket, &mut client, |_| true);
        let pawn = lab.peers[&address].controller_startup_pawn_guid.unwrap();
        let [(position, entries)] = possession_updates(&lab, &bunches).try_into().unwrap();
        assert_eq!(entries, possession_entries(pawn));
        assert!(position > pawn_open_position(&bunches));
        lab.retry_controller_possession();
        assert_eq!(lab.stats.controller_possession_updates_sent, 1);
        assert_eq!(lab.stats.controller_possession_updates_acked, 1);
    }

    /// Read every outstanding datagram of `address`, ACKing or NAKing each
    /// as `delivered` says of its bunches, and return the bunches in order.
    fn deliver_bunches_by_content(
        lab: &mut PeerLab,
        socket: &UdpSocket,
        client: &mut super::super::sequencing::PacketSequencer,
        delivered: impl Fn(&[super::super::packet::Bunch]) -> bool,
    ) -> Vec<super::super::packet::Bunch> {
        use super::super::packet::{Packet, Receiver, read_packet, write_packet};
        let SocketAddr::V4(address) = socket.local_addr().unwrap() else {
            unreachable!()
        };
        let count = lab.peers[&address]
            .peer
            .sequencer()
            .notify()
            .sent_records()
            .len();
        let config = super::super::packet::PacketConfig {
            receiver: Receiver::Client,
            ..lab.config.packet.packet
        };
        let mut bunches = Vec::new();
        for _ in 0..count {
            let mut bytes = [0; 2048];
            let length = socket.recv(&mut bytes).unwrap();
            let packet = read_packet(&bytes[..length], &config).unwrap();
            let received = client.receive(&packet.header).unwrap().unwrap();
            client
                .ack_seq(received.sequence, delivered(&packet.bunches))
                .unwrap();
            bunches.extend(packet.bunches);
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
        receive_authored_at(lab, socket, &bytes, Instant::now());
        bunches
    }

    #[test]
    fn controller_possession_nak_is_repaired_without_recomposition() {
        let mut lab = possession_lab(false);
        let (socket, address, mut client) = possession_identity_peer(&mut lab, 1, "AuthoredNak");
        assert!(lab.send_player_controller_open(address));
        let lost = deliver_bunches_by_content(&mut lab, &socket, &mut client, |bunches| {
            !bunches.iter().any(possession_shaped)
        });
        let [(position, entries)] = possession_updates(&lab, &lost).try_into().unwrap();
        let original = lost[position].payload.clone();
        let pawn = lab.peers[&address].controller_startup_pawn_guid.unwrap();
        assert_eq!(entries, possession_entries(pawn));
        assert_eq!(lab.stats.controller_possession_updates_acked, 0);
        assert!(lab.readiness_counts().resend_bunches_pending > 0);
        assert_eq!(
            lab.readiness_counts().controller_possession,
            ControllerPossessionReadiness {
                queued: 1,
                awaiting_ack: 1,
                ..Default::default()
            }
        );
        // The reliable layer resends the stored bunch unchanged.
        let repair = lab
            .peers
            .get_mut(&address)
            .unwrap()
            .peer
            .ack_packet()
            .unwrap();
        assert!(lab.send(&repair, address));
        let repaired = deliver_bunches(&mut lab, &socket, &mut client, |_| true);
        let [(position, _)] = possession_updates(&lab, &repaired).try_into().unwrap();
        assert_eq!(repaired[position].payload.bytes(), original.bytes());
        assert_eq!(repaired[position].payload.bit_len(), original.bit_len());
        assert_eq!(lab.stats.controller_possession_updates_sent, 1);
        assert_eq!(lab.stats.controller_possession_updates_acked, 1);
        // A later ACK does not count it again.
        let more = lab
            .peers
            .get_mut(&address)
            .unwrap()
            .peer
            .ack_packet()
            .unwrap();
        assert!(lab.send(&more, address));
        deliver_bunches(&mut lab, &socket, &mut client, |_| true);
        assert_eq!(lab.stats.controller_possession_updates_acked, 1);
        assert_eq!(
            lab.readiness_counts().controller_possession,
            ControllerPossessionReadiness {
                queued: 1,
                settled: 1,
                ..Default::default()
            }
        );
    }

    #[test]
    fn a_reconnect_gets_its_own_possession_update() {
        let mut lab = possession_lab(true);
        let (socket, address, _client) = record_peer(&mut lab, 1, "AuthoredReconnect");
        let first = lab.peers[&address].controller_startup_pawn_guid.unwrap();
        assert_eq!(lab.stats.controller_possession_updates_acked, 1);
        let (rejoined, mut client) = record_peer_on(&mut lab, &socket, 2, "AuthoredReconnect");
        assert_eq!(rejoined, address);
        assert_eq!(lab.stats.peers_replaced, 1);
        let second = lab.peers[&address].controller_startup_pawn_guid.unwrap();
        assert_ne!(first, second);
        assert_eq!(lab.stats.controller_possession_updates_sent, 2);
        assert_eq!(lab.stats.controller_possession_updates_acked, 2);
        assert_eq!(
            lab.readiness_counts().controller_possession,
            ControllerPossessionReadiness {
                queued: 1,
                settled: 1,
                ..Default::default()
            }
        );
        // Nothing more on later passes.
        lab.controller_possession_due(address);
        lab.retry_controller_possession();
        assert!(deliver_bunches(&mut lab, &socket, &mut client, |_| true).is_empty());
        assert_eq!(lab.stats.controller_possession_updates_sent, 2);
    }

    /// Leaving at each stage (controller only, update held back, admitted
    /// and unacknowledged, settled) leaves no possession state or work.
    #[test]
    fn possession_state_leaves_with_its_session_at_every_stage() {
        for stage in 0..4 {
            let mut lab = possession_lab(true);
            let (socket, address, mut client) =
                possession_identity_peer(&mut lab, 1, "AuthoredLeave");
            assert!(matches!(
                lab.send_initial_controller_open(address),
                InitialOpenOutcome::Committed(_)
            ));
            if stage > 0 {
                let (controller, player_state) = lab.peers[&address].controller_open_guids.unwrap();
                assert!(matches!(
                    lab.send_pawn_open(address, controller, player_state),
                    InitialOpenOutcome::Committed(_)
                ));
            }
            match stage {
                1 => {
                    let entry = lab.peers.get_mut(&address).unwrap();
                    entry.controller_possession_receipt = InitialisationReceipt::default();
                    entry.controller_possession_pending = true;
                }
                3 => {
                    deliver_bunches(&mut lab, &socket, &mut client, |_| true);
                }
                _ => {}
            }
            let expected_settled = u64::from(stage == 3);
            assert_eq!(
                lab.stats.controller_possession_updates_acked,
                expected_settled
            );
            let sent = lab.stats.controller_possession_updates_sent;
            assert_eq!(sent, u64::from(stage > 0), "stage {stage}");
            lab.remove_peer(address).unwrap();
            assert_eq!(
                lab.readiness_counts().controller_possession,
                ControllerPossessionReadiness::default(),
                "stage {stage}"
            );
            lab.retry_controller_possession();
            lab.controller_possession_due(address);
            assert_eq!(lab.stats.controller_possession_updates_sent, sent);
            assert_eq!(
                lab.stats.controller_possession_updates_acked,
                expected_settled
            );
            assert_eq!(lab.stats.controller_possession_errors, 0);
        }
    }

    /// Every bunch the client receives from Join to the record's ACK, and
    /// the same by channel for comparison: GameState payloads, which carry
    /// the lab clock, by length only. `None` never touches the switch;
    /// `Some(false)` turns it on, then off.
    #[allow(clippy::type_complexity)]
    fn possession_scenario(
        mode: Option<bool>,
    ) -> (
        Vec<super::super::packet::Bunch>,
        Vec<(u32, bool, bool, bool, usize, Vec<u8>)>,
    ) {
        let mut lab = record_lab(true);
        match mode {
            Some(true) => lab.enable_controller_possession_properties().unwrap(),
            Some(false) => {
                lab.enable_controller_possession_properties().unwrap();
                lab.disable_controller_possession_properties().unwrap();
            }
            None => {}
        }
        assert_eq!(
            lab.has_controller_possession_properties(),
            mode == Some(true)
        );
        let channel = game_state_channel(&lab);
        let (socket, address, mut client, mut bunches) =
            possession_peer(&mut lab, 1, "AuthoredScenario");
        complete_record_player(&mut lab, address);
        bunches.extend(deliver_bunches(&mut lab, &socket, &mut client, |_| true));
        lab.controller_call(address, ControllerCall::FriendsList);
        lab.finalise_connected_players();
        lab.retry_connected_player_records();
        bunches.extend(deliver_bunches(&mut lab, &socket, &mut client, |_| true));
        assert_eq!(lab.stats.connected_player_record_creations_acked, 1);
        let shapes = bunches
            .iter()
            .map(|bunch| {
                let header = &bunch.header;
                let bytes = if header.channel_index == channel {
                    Vec::new()
                } else {
                    bunch.payload.bytes().to_vec()
                };
                (
                    header.channel_index,
                    header.reliable,
                    header.open,
                    header.has_package_map_exports,
                    bunch.payload.bit_len(),
                    bytes,
                )
            })
            .collect();
        (bunches, shapes)
    }

    /// Within one build: never touched equals turned on then off, and on adds
    /// exactly the one update. It cannot see a change to shared code; the
    /// cross-commit pin is `switch_off_sessions_send_the_parent_commits_bunches`.
    #[test]
    fn disabled_controller_possession_sends_exactly_the_previous_bunches() {
        let (_, before) = possession_scenario(None);
        let (_, disabled) = possession_scenario(Some(false));
        assert!(!before.is_empty());
        assert_eq!(disabled, before);
        // Enabled, the run differs only by the one possession update: every
        // other bunch is unchanged and in the same order.
        let (bunches, mut enabled) = possession_scenario(Some(true));
        assert_eq!(enabled.len(), before.len() + 1);
        let extra = enabled
            .iter()
            .zip(&before)
            .position(|(enabled, before)| enabled != before)
            .unwrap_or(before.len());
        enabled.remove(extra);
        assert_eq!(enabled, before);
        let lab = record_lab(false);
        let updates = possession_updates(&lab, &bunches[extra..=extra]);
        let [(0, entries)] = updates.as_slice() else {
            panic!("the extra bunch is the possession update");
        };
        let handles: Vec<u32> = entries.iter().map(|(handle, _)| *handle).collect();
        assert_eq!(handles, [17, 20]);
        assert!(extra > pawn_open_position(&bunches));
        // Configuration is fixed once a peer exists.
        let mut lab = possession_lab(false);
        let (_socket, _address, _client) = record_peer(&mut lab, 1, "AuthoredLate");
        assert!(lab.disable_controller_possession_properties().is_err());
        assert!(lab.enable_controller_possession_properties().is_err());
        assert!(lab.has_controller_possession_properties());
    }

    #[test]
    fn controller_possession_requires_the_controller_and_pawn_opens() {
        let mut lab = PeerLab::bind(0, PeerLabConfig::default()).unwrap();
        assert!(lab.enable_controller_possession_properties().is_err());
        // The GameState and controller, but no pawn open.
        let mut lab = prospect_refresh_lab();
        assert!(lab.enable_controller_possession_properties().is_err());
        assert!(!lab.has_controller_possession_properties());
        let mut lab = record_lab(false);
        lab.enable_controller_possession_properties().unwrap();
        assert!(lab.has_controller_possession_properties());
    }

    /// Two sequential sessions on `lab`, each from the stateless handshake
    /// and Join to its connected-player record's ACK, the first removed
    /// before the second joins: every bunch the client receives, digested
    /// per session. Whole bunches off the GameState channel count by their
    /// exact bytes; GameState-channel bunches, which carry the lab clock,
    /// count by header flags, length and block targets. Uses only behaviour
    /// that predates the controller possession switch.
    fn switch_off_session_digests(lab: &mut PeerLab) -> Vec<String> {
        use super::super::control_messages::{NetIdType, NetIdValue, UniqueNetId};
        let channel = game_state_channel(lab);
        let codec = actor_codec(17, super::super::archive::ByteOrder::Little);
        let mut digests = Vec::new();
        for (connection, user) in [(1u32, "AuthoredDigestFirst"), (2, "AuthoredDigestSecond")] {
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
                name_units: "AUTHORED-DIGEST".encode_utf16().collect(),
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
                    let body = super::super::actor_channel::read_actor_bunch_body(
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

    /// The production bind's lab with records on (the default).
    fn production_digest_lab(config: crate::config::NetworkConfig) -> PeerLab {
        crate::network::bind(&config).unwrap().unwrap().lab
    }

    /// The switch-off path pinned to the parent of the controller possession
    /// change. These digests were produced by [`switch_off_session_digests`]
    /// on a build of 68b6325's `src/` (the commit before the switch existed,
    /// the same helper and fixtures appended to its tests), not by this
    /// build. A lab that never touches the switch, one that turns it on and
    /// then off, and the production bind with
    /// `controller_possession_properties = false` must send exactly those
    /// bunches; with the switch on every session differs.
    #[test]
    fn switch_off_sessions_send_the_parent_commits_bunches() {
        const LAB: [&str; 2] = [
            "da3f7bbda7f9e3c4b8f4183c6ccd55fc6ab259dc1487fbf838d9a29a2862d963",
            "9aed6aef271f5c71155cf76e5693a22f3006b10dc12271c32b95ca7331b272ef",
        ];
        const PRODUCTION: [&str; 2] = [
            "cafb88babd75ea37f94944a890596a7d6d1e8566b9a61278fb471031411bde5c",
            "4d6c55a1fba2f27e51d71df5b9201a4116d2a8ae3322fdba1022e24c97e5b205",
        ];
        let mut untouched = record_lab(true);
        assert!(!untouched.has_controller_possession_properties());
        assert_eq!(switch_off_session_digests(&mut untouched), LAB);
        let mut toggled = record_lab(true);
        toggled.enable_controller_possession_properties().unwrap();
        toggled.disable_controller_possession_properties().unwrap();
        assert_eq!(switch_off_session_digests(&mut toggled), LAB);
        let config = crate::config::NetworkConfig {
            enabled: true,
            port: 0,
            release_profile: Some(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("config/release.toml"),
            ),
            controller_possession_properties: false,
            ..crate::config::NetworkConfig::default()
        };
        let mut production = production_digest_lab(config.clone());
        assert!(!production.has_controller_possession_properties());
        assert!(production.has_connected_player_records());
        assert_eq!(switch_off_session_digests(&mut production), PRODUCTION);
        let on = switch_off_session_digests(&mut possession_lab(true));
        assert!(on.iter().zip(LAB).all(|(on, off)| on != off));
        let mut production_on = production_digest_lab(crate::config::NetworkConfig {
            controller_possession_properties: true,
            ..config
        });
        assert!(production_on.has_controller_possession_properties());
        let on = switch_off_session_digests(&mut production_on);
        assert!(on.iter().zip(PRODUCTION).all(|(on, off)| on != off));
    }

    fn queue_initialisation_phase(lab: &mut PeerLab, address: SocketAddrV4, completion: bool) {
        if completion {
            let entry = lab.peers.get_mut(&address).unwrap();
            entry.profiles.has_loadout = true;
            entry.profiles.has_bestiary = true;
            lab.complete_player_initialisation(address);
            assert!(lab.peers[&address].profiles.completed);
        } else {
            lab.request_initialisation(address);
            assert!(lab.peers[&address].profiles.initialisation_requested);
        }
    }

    fn phase_transport_counts(completion: bool, settled: bool) -> InitialisationTransportCounts {
        if completion {
            InitialisationTransportCounts {
                completions_queued: 1,
                completions_settled: usize::from(settled),
                completions_awaiting_ack: usize::from(!settled),
                ..Default::default()
            }
        } else {
            InitialisationTransportCounts {
                requests_queued: 1,
                requests_settled: usize::from(settled),
                requests_awaiting_ack: usize::from(!settled),
                ..Default::default()
            }
        }
    }

    #[test]
    fn initialisation_transport_receipts_require_ack_and_survive_loss_repair() {
        for completion in [false, true] {
            for mixed in [false, true] {
                let (mut lab, socket, address, mut client) = initialisation_transport_peer();
                queue_initialisation_phase(&mut lab, address, completion);
                let queued = phase_transport_counts(completion, false);
                assert_eq!(lab.readiness_counts().initialisation_transport, queued);
                if mixed {
                    let packet = lab
                        .peers
                        .get_mut(&address)
                        .unwrap()
                        .peer
                        .ack_packet()
                        .unwrap();
                    assert!(lab.send(&packet, address));
                }
                notify_startup_packets(&mut lab, &socket, &mut client, |index| mixed && index > 0);
                assert_eq!(lab.readiness_counts().initialisation_transport, queued);
                assert!(lab.readiness_counts().resend_bunches_pending > 0);
                let updates = lab.stats.player_character_id_updates_sent;
                let requests = lab.stats.loadout_requests_sent;
                for _ in 0..3 {
                    lab.retry_pending_profile_exchanges();
                    assert_eq!(lab.readiness_counts().initialisation_transport, queued);
                }
                let repair = lab
                    .peers
                    .get_mut(&address)
                    .unwrap()
                    .peer
                    .ack_packet()
                    .unwrap();
                assert!(lab.send(&repair, address));
                ack_startup_packets(&mut lab, &socket, &mut client);
                assert_eq!(
                    lab.readiness_counts().initialisation_transport,
                    phase_transport_counts(completion, true)
                );
                assert_eq!(lab.stats.player_character_id_updates_sent, updates);
                assert_eq!(lab.stats.loadout_requests_sent, requests);
                lab.remove_peer(address).unwrap();
                assert_eq!(lab.readiness_counts(), ReadinessCounts::default());
                let (_replacement, _, _) = acked_game_state_peer(&mut lab);
                assert_eq!(
                    lab.readiness_counts().initialisation_transport,
                    InitialisationTransportCounts::default()
                );
            }
        }
    }

    #[test]
    fn initialisation_transport_receipts_ignore_later_same_channel_traffic() {
        for completion in [false, true] {
            let (mut lab, socket, address, mut client) = initialisation_transport_peer();
            queue_initialisation_phase(&mut lab, address, completion);
            let phase_packets = lab.peers[&address]
                .peer
                .sequencer()
                .notify()
                .sent_records()
                .len();
            let entry = lab.peers.get_mut(&address).unwrap();
            let channel = if completion {
                entry.player_state_channel
            } else {
                entry.player_controller_channel
            }
            .unwrap();
            let later = entry
                .peer
                .reliable_actor_bunch_datagrams(
                    channel,
                    Payload::from_writer(super::super::packet::BitWriter::new()),
                )
                .unwrap();
            assert!(lab.send_all(&later, address));
            notify_startup_packets(&mut lab, &socket, &mut client, |index| {
                index < phase_packets
            });
            assert!(!lab.peers[&address].peer.actor_channel_settled(channel));
            let settled = phase_transport_counts(completion, true);
            assert_eq!(lab.readiness_counts().initialisation_transport, settled);
            let repair = lab
                .peers
                .get_mut(&address)
                .unwrap()
                .peer
                .ack_packet()
                .unwrap();
            assert!(lab.send(&repair, address));
            ack_startup_packets(&mut lab, &socket, &mut client);
            assert_eq!(lab.readiness_counts().initialisation_transport, settled);
        }
    }

    #[test]
    fn initialisation_transport_receipts_recognise_acked_span_behind_an_older_gap() {
        for completion in [false, true] {
            let (mut lab, socket, address, mut client) = initialisation_transport_peer();
            let entry = lab.peers.get_mut(&address).unwrap();
            let channel = if completion {
                entry.player_state_channel
            } else {
                entry.player_controller_channel
            }
            .unwrap();
            let older = entry
                .peer
                .reliable_actor_bunch_datagrams(
                    channel,
                    Payload::from_writer(super::super::packet::BitWriter::new()),
                )
                .unwrap();
            let older_packets = older.packet_ids.len();
            assert!(lab.send_all(&older, address));
            queue_initialisation_phase(&mut lab, address, completion);
            notify_startup_packets(&mut lab, &socket, &mut client, |index| {
                index >= older_packets
            });
            assert!(!lab.peers[&address].peer.actor_channel_settled(channel));
            assert_eq!(
                lab.readiness_counts().initialisation_transport,
                phase_transport_counts(completion, true)
            );
        }
    }

    #[test]
    fn initialisation_transport_phases_settle_independently_of_finalisation() {
        let (mut lab, socket, address, mut client) = initialisation_transport_peer();
        queue_initialisation_phase(&mut lab, address, false);
        let request_packets = lab.peers[&address]
            .peer
            .sequencer()
            .notify()
            .sent_records()
            .len();
        queue_initialisation_phase(&mut lab, address, true);
        lab.peers
            .get_mut(&address)
            .unwrap()
            .profiles
            .friend_ids_ready = true;
        lab.finalise_connected_players();
        assert!(lab.peers[&address].profiles.finalised);
        assert_eq!(
            lab.readiness_counts().initialisation_transport,
            InitialisationTransportCounts {
                requests_queued: 1,
                requests_awaiting_ack: 1,
                completions_queued: 1,
                completions_awaiting_ack: 1,
                ..Default::default()
            }
        );
        notify_startup_packets(&mut lab, &socket, &mut client, |index| {
            index < request_packets
        });
        assert_eq!(
            lab.readiness_counts().initialisation_transport,
            InitialisationTransportCounts {
                requests_queued: 1,
                requests_settled: 1,
                completions_queued: 1,
                completions_awaiting_ack: 1,
                ..Default::default()
            }
        );
        let repair = lab
            .peers
            .get_mut(&address)
            .unwrap()
            .peer
            .ack_packet()
            .unwrap();
        assert!(lab.send(&repair, address));
        ack_startup_packets(&mut lab, &socket, &mut client);
        assert_eq!(
            lab.readiness_counts().initialisation_transport,
            InitialisationTransportCounts {
                requests_queued: 1,
                requests_settled: 1,
                completions_queued: 1,
                completions_settled: 1,
                ..Default::default()
            }
        );
        assert!(lab.peers[&address].profiles.finalised);
    }

    #[test]
    fn initialisation_transport_close_and_channel_reuse_cannot_settle_old_receipts() {
        for completion in [false, true] {
            let (mut lab, socket, address, mut client) = initialisation_transport_peer();
            queue_initialisation_phase(&mut lab, address, completion);
            let entry = lab.peers.get_mut(&address).unwrap();
            let channel = if completion {
                entry.player_state_channel
            } else {
                entry.player_controller_channel
            }
            .unwrap();
            let close = entry.peer.reliable_actor_close(channel, 0).unwrap();
            assert!(lab.send(&close, address));
            ack_startup_packets(&mut lab, &socket, &mut client);
            let queued = phase_transport_counts(completion, false);
            assert_eq!(lab.readiness_counts().initialisation_transport, queued);
            assert!(!lab.peers[&address].peer.has_actor_channel(channel));
            let mut config = lab.config.packet.packet;
            config.receiver = super::super::packet::Receiver::Client;
            let mut reopened = super::super::packet::read_packet(&close, &config)
                .unwrap()
                .bunches
                .pop()
                .unwrap();
            reopened.header.open = true;
            reopened.header.close = false;
            reopened.header.close_reason = None;
            reopened.header.reliable_sequence_raw = None;
            let sent = lab
                .peers
                .get_mut(&address)
                .unwrap()
                .peer
                .reliable_actor_open_datagrams(&reopened, &[])
                .unwrap();
            assert!(lab.send_all(&sent, address));
            ack_startup_packets(&mut lab, &socket, &mut client);
            assert!(lab.peers[&address].peer.actor_channel_settled(channel));
            assert_eq!(lab.readiness_counts().initialisation_transport, queued);
        }
    }

    #[test]
    fn initialisation_transport_split_span_waits_for_repair_across_wire_sequence_wrap() {
        use super::super::{game_state_open::handle, sequencing::wire_reliable_sequence};
        let (mut lab, socket, address, mut client) = initialisation_transport_peer();
        let channel = lab.peers[&address].player_controller_channel.unwrap();
        // Channels start at the cookie-derived outgoing packet ID & 0x3ff, and
        // the lab draws a random cookie secret, so the start is random: drive
        // to the next sequence whose wire value is 1022, never to absolute
        // 1022, which setup alone may already have passed.
        let start = lab.peers[&address]
            .peer
            .actor_reliable_sequence(channel)
            .unwrap();
        let target = start + (1022 + 1024 - wire_reliable_sequence(start)) % 1024;
        assert_eq!(wire_reliable_sequence(target), 1022);
        while lab.peers[&address]
            .peer
            .actor_reliable_sequence(channel)
            .unwrap()
            < target
        {
            let previous = lab.peers[&address]
                .peer
                .actor_reliable_sequence(channel)
                .unwrap();
            for _ in 0..(target - previous).min(32) {
                let sent = lab
                    .peers
                    .get_mut(&address)
                    .unwrap()
                    .peer
                    .reliable_actor_bunch_datagrams(
                        channel,
                        Payload::from_writer(super::super::packet::BitWriter::new()),
                    )
                    .unwrap();
                assert!(lab.send_all(&sent, address));
            }
            ack_startup_packets(&mut lab, &socket, &mut client);
        }
        lab.game_state_open.as_mut().unwrap().state.values.insert(
            handle::ACTIVE_PROSPECT_FIRST,
            RepValue::Str(ArchiveText::text(&"G".repeat(1500)).unwrap()),
        );
        lab.request_initialisation(address);
        let receipt = &lab.peers[&address].initialisation_request_receipt;
        assert_eq!(receipt.first, target + 1);
        assert_eq!(wire_reliable_sequence(receipt.first), 1023);
        assert!(receipt.last > receipt.first);
        assert!(wire_reliable_sequence(receipt.last) < wire_reliable_sequence(receipt.first));
        notify_startup_packets(&mut lab, &socket, &mut client, |index| index > 0);
        assert_eq!(
            lab.readiness_counts().initialisation_transport,
            phase_transport_counts(false, false)
        );
        let repair = lab
            .peers
            .get_mut(&address)
            .unwrap()
            .peer
            .ack_packet()
            .unwrap();
        assert!(lab.send(&repair, address));
        ack_startup_packets(&mut lab, &socket, &mut client);
        assert_eq!(
            lab.readiness_counts().initialisation_transport,
            phase_transport_counts(false, true)
        );
    }

    #[test]
    fn readiness_snapshot_aggregates_multiple_peers_and_clears_on_shutdown() {
        let mut lab = prospect_refresh_lab();
        let (_first, _, _) = acked_game_state_peer(&mut lab);
        let (_second, _, _) = acked_game_state_peer(&mut lab);
        let counts = lab.readiness_counts();
        assert_eq!(counts.actor_channels_open, 2);
        assert_eq!(counts.actor_channels_reliable_settled, 2);
        assert_eq!(counts.actor_reliable_pending, 0);
        assert_eq!(counts.sent_packets_pending, 0);
        assert_eq!(lab.shutdown(), 2);
        assert_eq!(lab.readiness_counts(), ReadinessCounts::default());
    }

    #[test]
    fn initial_open_outcomes_preserve_failures_and_controller_skips_until_teardown() {
        let mut lab = prospect_refresh_lab();
        let (_first, first, _) = acked_game_state_peer(&mut lab);
        let (_second, second, _) = acked_game_state_peer(&mut lab);
        for (address, stages) in [
            (
                first,
                VecDeque::from([
                    InitialOpenStage::Pawn,
                    InitialOpenStage::Manager(usize::MAX),
                    InitialOpenStage::Quest(usize::MAX),
                    InitialOpenStage::GameState,
                ]),
            ),
            (
                second,
                VecDeque::from([
                    InitialOpenStage::Controller,
                    InitialOpenStage::GameState,
                    InitialOpenStage::PlayerState,
                ]),
            ),
        ] {
            let entry = lab.peers.get_mut(&address).unwrap();
            entry.peer = LabPeer::from_cookie(&entry.cookie, *entry.peer.config()).unwrap();
            entry.game_state = None;
            for &stage in &stages {
                entry.initial_open_outcomes.configured.record(stage);
            }
            entry.initial_opens = Some(PendingInitialOpens {
                remaining: stages,
                controller_guids: Some((2, 4)),
                player_state_is_host: false,
                begin_slot: None,
                retry_ready: true,
            });
        }
        // Missing configured plans model permanent failures, without changing
        // the existing policy of attempting independent stages after a failure.
        lab.player_controller_open = None;
        lab.retry_initial_opens();
        let snapshot = lab.readiness_counts();
        let outcomes = &snapshot.initial_open_outcomes;
        assert_eq!(outcomes.failed.controller, 1);
        assert_eq!(outcomes.failed.pawn, 1);
        assert_eq!(outcomes.failed.managers, 1);
        assert_eq!(outcomes.failed.quests, 1);
        assert_eq!(outcomes.skipped.game_state, 1);
        assert_eq!(outcomes.skipped.player_state, 1);
        assert!(outcomes.pending.is_empty());
        assert!(outcomes.deferred.is_empty());
        assert_eq!(outcomes.committed.game_state, 1);
        assert_eq!(outcomes.configured.game_state, 2);
        let mut partition = outcomes.committed.clone();
        partition.add(&outcomes.failed);
        partition.add(&outcomes.skipped);
        partition.add(&outcomes.pending);
        assert_eq!(partition, outcomes.configured);
        assert_eq!(outcomes.sequences_failed, 2);
        assert_eq!(outcomes.sequences_succeeded, 0);
        assert_eq!(outcomes.sequences_settled, 0);
        assert!(outcomes.settled.is_empty());
        assert_eq!(outcomes.awaiting_ack, outcomes.committed);
        assert_eq!(snapshot.pending_initial_opens, 0);
        lab.retry_initial_opens();
        assert_eq!(lab.readiness_counts(), snapshot);
        let encoded = serde_json::to_string(outcomes).unwrap();
        assert!(!encoded.contains(&first.to_string()));
        assert!(!encoded.contains(&second.to_string()));
        lab.remove_peer(first).unwrap();
        assert_eq!(
            lab.readiness_counts()
                .initial_open_outcomes
                .sequences_failed,
            1
        );
        lab.shutdown();
        assert_eq!(lab.readiness_counts(), ReadinessCounts::default());
    }

    #[test]
    fn initial_open_success_counts_configured_stages_once_and_reconnect_starts_clean() {
        let mut lab = prospect_refresh_lab();
        let (_socket, address, _) = acked_game_state_peer(&mut lab);
        let entry = lab.peers.get_mut(&address).unwrap();
        entry.peer = LabPeer::from_cookie(&entry.cookie, *entry.peer.config()).unwrap();
        entry.game_state = None;
        assert!(lab.send_player_controller_open(address));
        let snapshot = lab.readiness_counts().initial_open_outcomes;
        assert_eq!(snapshot.configured.controller, 1);
        assert_eq!(snapshot.configured.game_state, 1);
        assert_eq!(snapshot.configured.pawn, 0);
        assert_eq!(snapshot.configured.player_state, 0);
        assert_eq!(snapshot.committed, snapshot.configured);
        assert!(snapshot.failed.is_empty());
        assert!(snapshot.skipped.is_empty());
        assert_eq!(snapshot.sequences_succeeded, 1);
        assert_eq!(snapshot.sequences_settled, 0);
        assert!(snapshot.settled.is_empty());
        assert_eq!(snapshot.awaiting_ack, snapshot.committed);
        assert!(!lab.send_player_controller_open(address));
        lab.retry_initial_opens();
        assert_eq!(lab.readiness_counts().initial_open_outcomes, snapshot);
        lab.remove_peer(address).unwrap();
        let (_replacement, _, _) = acked_game_state_peer(&mut lab);
        assert_eq!(
            lab.readiness_counts().initial_open_outcomes,
            InitialOpenReadiness::default()
        );
    }

    #[test]
    fn initial_open_settlement_distinguishes_stages_and_survives_loss_repair() {
        use super::super::{packet_lab::client_cookie_seeds, sequencing::PacketSequencer};
        let mut lab = prospect_refresh_lab();
        let (socket, address, _) = acked_game_state_peer(&mut lab);
        // Reset both authored transport endpoints so this sequence includes
        // the GameState, rather than reusing the fixture's settled open.
        let entry = lab.peers.get_mut(&address).unwrap();
        entry.peer = LabPeer::from_cookie(&entry.cookie, *entry.peer.config()).unwrap();
        entry.game_state = None;
        let (incoming, outgoing) = client_cookie_seeds(&entry.cookie);
        let mut client = PacketSequencer::new(incoming, outgoing, 64).unwrap();
        assert!(lab.send_player_controller_open(address));
        let queued = lab.readiness_counts().initial_open_outcomes;
        assert_eq!(queued.committed.controller, 1);
        assert_eq!(queued.committed.game_state, 1);
        assert_eq!(queued.awaiting_ack, queued.committed);
        assert!(queued.settled.is_empty());
        assert_eq!(queued.sequences_succeeded, 1);
        assert_eq!(queued.sequences_settled, 0);
        for _ in 0..4 {
            lab.retry_initial_opens();
            assert_eq!(lab.readiness_counts().initial_open_outcomes, queued);
        }
        // The first controller packet is lost, while GameState packets and
        // any remaining controller fragments receive positive ACKs.
        notify_startup_packets(&mut lab, &socket, &mut client, |index| index > 0);
        let partial = lab.readiness_counts().initial_open_outcomes;
        assert_eq!(partial.settled.controller, 0);
        assert_eq!(partial.awaiting_ack.controller, 1);
        assert_eq!(partial.settled.game_state, 1);
        assert_eq!(partial.awaiting_ack.game_state, 0);
        assert_eq!(partial.sequences_settled, 0);
        assert!(lab.readiness_counts().resend_bunches_pending > 0);
        for _ in 0..4 {
            lab.retry_initial_opens();
            assert_eq!(lab.readiness_counts().initial_open_outcomes, partial);
        }
        let datagram = lab
            .peers
            .get_mut(&address)
            .unwrap()
            .peer
            .ack_packet()
            .unwrap();
        assert!(lab.send(&datagram, address));
        ack_startup_packets(&mut lab, &socket, &mut client);
        let settled = lab.readiness_counts().initial_open_outcomes;
        assert_eq!(settled.settled, settled.committed);
        assert!(settled.awaiting_ack.is_empty());
        assert_eq!(settled.sequences_settled, 1);
        assert_eq!(settled.configured, queued.configured);
        assert_eq!(settled.committed, queued.committed);
        lab.remove_peer(address).unwrap();
        assert_eq!(lab.readiness_counts(), ReadinessCounts::default());
        let (_replacement, _, _) = acked_game_state_peer(&mut lab);
        assert_eq!(
            lab.readiness_counts().initial_open_outcomes,
            InitialOpenReadiness::default()
        );
    }

    #[test]
    fn initial_open_split_settlement_waits_for_every_reliable_part() {
        use super::super::{
            game_state_open::handle, packet_lab::client_cookie_seeds, sequencing::PacketSequencer,
        };
        let mut lab = prospect_refresh_lab();
        let (socket, address, _) = acked_game_state_peer(&mut lab);
        lab.game_state_open.as_mut().unwrap().state.values.insert(
            handle::RICH_PRESENCE_GUID,
            RepValue::Str(ArchiveText::text(&"G".repeat(1_500)).unwrap()),
        );
        let entry = lab.peers.get_mut(&address).unwrap();
        entry.peer = LabPeer::from_cookie(&entry.cookie, *entry.peer.config()).unwrap();
        entry.game_state = None;
        let (incoming, outgoing) = client_cookie_seeds(&entry.cookie);
        let mut client = PacketSequencer::new(incoming, outgoing, 64).unwrap();
        entry
            .initial_open_outcomes
            .configured
            .record(InitialOpenStage::GameState);
        entry.initial_opens = Some(PendingInitialOpens {
            remaining: VecDeque::from([InitialOpenStage::GameState]),
            controller_guids: None,
            player_state_is_host: false,
            begin_slot: None,
            retry_ready: true,
        });
        lab.retry_initial_opens();
        assert!(
            lab.peers[&address]
                .peer
                .sequencer()
                .notify()
                .sent_records()
                .len()
                > 1
        );
        assert_eq!(
            lab.readiness_counts()
                .initial_open_outcomes
                .committed
                .game_state,
            1
        );
        notify_startup_packets(&mut lab, &socket, &mut client, |index| index > 0);
        let partial = lab.readiness_counts().initial_open_outcomes;
        assert!(partial.settled.is_empty());
        assert_eq!(partial.awaiting_ack.game_state, 1);
        assert_eq!(partial.sequences_settled, 0);
        let datagram = lab
            .peers
            .get_mut(&address)
            .unwrap()
            .peer
            .ack_packet()
            .unwrap();
        assert!(lab.send(&datagram, address));
        ack_startup_packets(&mut lab, &socket, &mut client);
        let repaired = lab.readiness_counts().initial_open_outcomes;
        assert_eq!(repaired.settled.game_state, 1);
        assert!(repaired.awaiting_ack.is_empty());
        assert_eq!(repaired.sequences_settled, 1);
    }

    #[test]
    fn initial_open_settlement_is_retained_through_later_reliable_traffic() {
        let mut lab = prospect_refresh_lab();
        let (socket, address, mut client) = acked_game_state_peer(&mut lab);
        lab.game_state_open = None;
        assert!(lab.send_player_controller_open(address));
        let queued = lab.readiness_counts().initial_open_outcomes;
        notify_startup_packets(&mut lab, &socket, &mut client, |_| false);
        assert_eq!(lab.readiness_counts().initial_open_outcomes, queued);
        let repair = lab
            .peers
            .get_mut(&address)
            .unwrap()
            .peer
            .ack_packet()
            .unwrap();
        assert!(lab.send(&repair, address));
        ack_startup_packets(&mut lab, &socket, &mut client);
        let settled = lab.readiness_counts().initial_open_outcomes;
        assert_eq!(settled.settled.controller, 1);
        assert_eq!(settled.sequences_settled, 1);
        let entry = lab.peers.get_mut(&address).unwrap();
        let channel = entry.player_controller_channel.unwrap();
        let sent = entry
            .peer
            .reliable_actor_bunch_datagrams(
                channel,
                Payload::from_writer(super::super::packet::BitWriter::new()),
            )
            .unwrap();
        assert!(lab.send_all(&sent, address));
        assert!(!lab.peers[&address].peer.actor_channel_settled(channel));
        assert_eq!(lab.readiness_counts().initial_open_outcomes, settled);
        ack_startup_packets(&mut lab, &socket, &mut client);
        assert_eq!(lab.readiness_counts().initial_open_outcomes, settled);
    }

    #[test]
    fn initial_open_close_ack_does_not_create_a_settlement_receipt() {
        let mut lab = prospect_refresh_lab();
        let (socket, address, mut client) = acked_game_state_peer(&mut lab);
        lab.game_state_open = None;
        assert!(lab.send_player_controller_open(address));
        let entry = lab.peers.get_mut(&address).unwrap();
        let channel = entry.player_controller_channel.unwrap();
        let close = entry.peer.reliable_actor_close(channel, 0).unwrap();
        assert!(lab.send(&close, address));
        ack_startup_packets(&mut lab, &socket, &mut client);
        assert!(!lab.peers[&address].peer.has_actor_channel(channel));
        let closed = lab.readiness_counts().initial_open_outcomes;
        assert_eq!(closed.committed.controller, 1);
        assert_eq!(closed.awaiting_ack.controller, 1);
        assert!(closed.settled.is_empty());
        assert_eq!(closed.sequences_settled, 0);
        lab.retry_initial_opens();
        assert_eq!(lab.readiness_counts().initial_open_outcomes, closed);
        // Reuse the released transport channel with an authored open. Reusing
        // the controller composer would also exercise its unrelated handling
        // of previously exported archetype/level references.
        let mut config = lab.config.packet.packet;
        config.receiver = super::super::packet::Receiver::Client;
        let mut reopened = super::super::packet::read_packet(&close, &config)
            .unwrap()
            .bunches
            .pop()
            .unwrap();
        reopened.header.open = true;
        reopened.header.close = false;
        reopened.header.close_reason = None;
        reopened.header.reliable_sequence_raw = None;
        let sent = lab
            .peers
            .get_mut(&address)
            .unwrap()
            .peer
            .reliable_actor_open_datagrams(&reopened, &[])
            .unwrap();
        assert!(lab.send_all(&sent, address));
        ack_startup_packets(&mut lab, &socket, &mut client);
        assert!(lab.peers[&address].peer.actor_channel_settled(channel));
        assert_eq!(lab.readiness_counts().initial_open_outcomes, closed);
    }

    #[test]
    fn controller_startup_configuration_is_bounded_atomic_and_opt_in() {
        use ControllerStartupAction::{FlushLevelStreaming, RestartPawn};
        let mut lab = prospect_refresh_lab();
        assert!(lab.controller_startup.is_none());
        for actions in [vec![], vec![FlushLevelStreaming; 33], vec![RestartPawn]] {
            assert!(
                lab.enable_controller_startup(&ReleaseProfile::release_3_0_32(), actions)
                    .is_err()
            );
            assert!(lab.controller_startup.is_none());
        }
        lab.enable_controller_startup(&ReleaseProfile::release_3_0_32(), vec![FlushLevelStreaming])
            .unwrap();
        assert!(
            lab.enable_controller_startup(
                &ReleaseProfile::release_3_0_32(),
                vec![FlushLevelStreaming]
            )
            .is_err()
        );
        assert_eq!(lab.controller_startup, Some(vec![FlushLevelStreaming]));
        let (_socket, _, _) = acked_game_state_peer(&mut lab);
        assert!(
            lab.enable_controller_startup(
                &ReleaseProfile::release_3_0_32(),
                vec![FlushLevelStreaming]
            )
            .is_err()
        );
        assert_eq!(lab.controller_startup, Some(vec![FlushLevelStreaming]));
    }

    #[test]
    fn controller_startup_payload_targets_actor_and_resolves_each_pawn() {
        use super::super::{
            actor_channel::{NetGuidRef, read_actor_bunch_body},
            archive::ByteOrder,
            rep_rpc::parse_rpc_payload,
        };
        for order in [ByteOrder::Little, ByteOrder::Big] {
            let codec = RepLayoutCodec {
                actor: actor_codec(30, order),
                checksums: false,
                limits: Default::default(),
            };
            for pawn in [2, 42] {
                let payload = controller_startup_payload(
                    ControllerStartupAction::RestartPawn,
                    BP_ICARUS_PLAYER_CONTROLLER_SURVIVAL_FIELD_INDEX_BOUND,
                    Some(pawn),
                    &codec,
                )
                .unwrap();
                let body =
                    read_actor_bunch_body(&mut payload.reader(), false, &codec.actor).unwrap();
                assert!(body.spawn.is_none());
                assert_eq!(body.blocks.len(), 1);
                let block = &body.blocks[0];
                assert!(matches!(block.target, BlockTarget::Actor));
                assert!(!block.has_rep_layout);
                let mut reader = block.payload.reader();
                let field = read_field_header(
                    &mut reader,
                    BP_ICARUS_PLAYER_CONTROLLER_SURVIVAL_FIELD_INDEX_BOUND,
                    codec.actor.limits.max_payload_bits,
                )
                .unwrap();
                assert!(reader.at_end());
                assert_eq!(field.index, 40);
                let arguments = parse_rpc_payload(
                    &field.payload,
                    &player_controller_rpc_parameters(40).unwrap(),
                    &codec,
                )
                .unwrap();
                assert_eq!(
                    arguments,
                    vec![RpcArgument::Sent(vec![RpcItem::Value(RepValue::Object(
                        NetGuidRef::plain(pawn)
                    ))])]
                );
            }
            assert!(
                controller_startup_payload(
                    ControllerStartupAction::RestartPawn,
                    BP_ICARUS_PLAYER_CONTROLLER_SURVIVAL_FIELD_INDEX_BOUND,
                    None,
                    &codec
                )
                .is_err()
            );
        }
    }

    #[test]
    fn controller_startup_waits_for_open_ack_and_retains_order_without_poll_replays() {
        use ControllerStartupAction::FlushLevelStreaming;
        let mut lab = prospect_refresh_lab();
        lab.config.packet.sent_capacity = 8;
        lab.enable_controller_startup(
            &ReleaseProfile::release_3_0_32(),
            vec![FlushLevelStreaming, FlushLevelStreaming],
        )
        .unwrap();
        let (socket, address, mut client) = acked_game_state_peer(&mut lab);
        // The authored fixture supplies an already-settled actor channel. A
        // startup cursor is installed only by the normal open sequence below.
        assert_eq!(lab.readiness_counts().controller_startup_actions_queued, 0);
        lab.retry_controller_startup();
        assert_eq!(lab.readiness_counts().controller_startup_actions_queued, 0);
        lab.game_state_open = None;
        assert!(lab.send_player_controller_open(address));
        assert_eq!(lab.readiness_counts().controller_startup_actions_pending, 2);
        lab.retry_controller_startup();
        assert_eq!(lab.readiness_counts().controller_startup_actions_queued, 0);
        assert!(!lab.peers[&address].controller_startup_retry_ready);
        let snapshot = lab.readiness_counts();
        for _ in 0..4 {
            lab.retry_controller_startup();
        }
        assert_eq!(lab.readiness_counts(), snapshot);
        ack_startup_packets(&mut lab, &socket, &mut client);
        // Occupy the bounded sent-record ring with independently authored
        // ACK-only packets while the actor channel itself remains settled.
        let capacity = lab.peers[&address].peer.config().sent_capacity;
        for _ in 0..capacity {
            let datagram = lab
                .peers
                .get_mut(&address)
                .unwrap()
                .peer
                .ack_packet()
                .unwrap();
            assert!(lab.send(&datagram, address));
        }
        lab.retry_controller_startup();
        assert_eq!(lab.readiness_counts().controller_startup_actions_queued, 0);
        assert_eq!(lab.readiness_counts().controller_startup_actions_pending, 2);
        assert_eq!(
            lab.readiness_counts().controller_startup_sequences_failed,
            0
        );
        let pressure = lab.readiness_counts();
        for _ in 0..4 {
            lab.retry_controller_startup();
        }
        assert_eq!(lab.readiness_counts(), pressure);
        ack_startup_packets(&mut lab, &socket, &mut client);
        lab.retry_controller_startup();
        assert_eq!(lab.readiness_counts().controller_startup_actions_queued, 1);
        assert_eq!(lab.readiness_counts().controller_startup_actions_pending, 1);
        assert_eq!(lab.readiness_counts().controller_startup_actions_settled, 0);
        assert_eq!(
            lab.readiness_counts()
                .controller_startup_actions_awaiting_ack,
            1
        );
        assert_eq!(
            lab.readiness_counts().controller_startup_sequences_settled,
            0
        );
        lab.retry_controller_startup();
        assert_eq!(lab.readiness_counts().controller_startup_actions_queued, 1);
        ack_startup_packets(&mut lab, &socket, &mut client);
        assert_eq!(lab.readiness_counts().controller_startup_actions_settled, 1);
        assert_eq!(
            lab.readiness_counts()
                .controller_startup_actions_awaiting_ack,
            0
        );
        lab.retry_controller_startup();
        assert_eq!(lab.readiness_counts().controller_startup_actions_queued, 2);
        assert_eq!(lab.readiness_counts().controller_startup_actions_pending, 0);
        assert_eq!(lab.readiness_counts().controller_startup_actions_settled, 1);
        assert_eq!(
            lab.readiness_counts()
                .controller_startup_actions_awaiting_ack,
            1
        );
        assert_eq!(
            lab.readiness_counts().controller_startup_sequences_settled,
            0
        );
        assert_eq!(
            lab.readiness_counts().controller_startup_sequences_failed,
            0
        );
        ack_startup_packets(&mut lab, &socket, &mut client);
        assert_eq!(lab.readiness_counts().controller_startup_actions_settled, 2);
        assert_eq!(
            lab.readiness_counts()
                .controller_startup_actions_awaiting_ack,
            0
        );
        assert_eq!(
            lab.readiness_counts().controller_startup_sequences_settled,
            1
        );
        // Later reliable traffic cannot revoke an already retained receipt.
        let channel = lab.peers[&address].player_controller_channel.unwrap();
        lab.peers
            .get_mut(&address)
            .unwrap()
            .peer
            .reliable_actor_bunch(
                channel,
                Payload::from_writer(super::super::packet::BitWriter::new()),
            )
            .unwrap();
        assert!(!lab.peers[&address].peer.actor_channel_settled(channel));
        let settled = lab.readiness_counts();
        assert_eq!(settled.controller_startup_actions_settled, 2);
        assert_eq!(settled.controller_startup_sequences_settled, 1);
        for _ in 0..4 {
            lab.retry_controller_startup();
            assert_eq!(lab.readiness_counts(), settled);
        }
        lab.remove_peer(address).unwrap();
        assert_eq!(lab.readiness_counts(), ReadinessCounts::default());
    }

    fn ack_startup_packets(
        lab: &mut PeerLab,
        socket: &UdpSocket,
        client: &mut super::super::sequencing::PacketSequencer,
    ) {
        notify_startup_packets(lab, socket, client, |_| true);
    }

    fn notify_startup_packets(
        lab: &mut PeerLab,
        socket: &UdpSocket,
        client: &mut super::super::sequencing::PacketSequencer,
        delivered: impl Fn(usize) -> bool,
    ) {
        use super::super::packet::{Packet, Receiver, read_packet, write_packet};
        let SocketAddr::V4(address) = socket.local_addr().unwrap() else {
            unreachable!()
        };
        let count = lab.peers[&address]
            .peer
            .sequencer()
            .notify()
            .sent_records()
            .len();
        assert!(count > 0);
        let config = super::super::packet::PacketConfig {
            receiver: Receiver::Client,
            ..lab.config.packet.packet
        };
        for index in 0..count {
            let mut bytes = [0; 2048];
            let length = socket.recv(&mut bytes).unwrap();
            let packet = read_packet(&bytes[..length], &config).unwrap();
            let received = client.receive(&packet.header).unwrap().unwrap();
            client.ack_seq(received.sequence, delivered(index)).unwrap();
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
        receive_authored_at(lab, socket, &bytes, Instant::now());
    }

    #[test]
    fn controller_startup_settlement_requires_ack_and_survives_loss_repair() {
        for mixed in [false, true] {
            let mut lab = prospect_refresh_lab();
            lab.enable_controller_startup(
                &ReleaseProfile::release_3_0_32(),
                vec![ControllerStartupAction::FlushLevelStreaming],
            )
            .unwrap();
            let (socket, address, mut client) = acked_game_state_peer(&mut lab);
            lab.game_state_open = None;
            assert!(lab.send_player_controller_open(address));
            ack_startup_packets(&mut lab, &socket, &mut client);
            lab.retry_controller_startup();
            assert_eq!(lab.readiness_counts().controller_startup_actions_queued, 1);
            if mixed {
                // A positive ACK for independent traffic must not hide the
                // startup packet's negative ACK in the same notification.
                let datagram = lab
                    .peers
                    .get_mut(&address)
                    .unwrap()
                    .peer
                    .ack_packet()
                    .unwrap();
                assert!(lab.send(&datagram, address));
            }
            notify_startup_packets(&mut lab, &socket, &mut client, |index| mixed && index > 0);
            let lost = lab.readiness_counts();
            assert_eq!(lost.controller_startup_actions_settled, 0);
            assert_eq!(lost.controller_startup_actions_awaiting_ack, 1);
            assert_eq!(lost.controller_startup_sequences_settled, 0);
            assert!(lost.resend_bunches_pending > 0);
            for _ in 0..4 {
                lab.retry_controller_startup();
                assert_eq!(lab.readiness_counts(), lost);
            }
            // The transport resends its retained bunch; startup does not
            // enqueue the RPC a second time.
            let datagram = lab
                .peers
                .get_mut(&address)
                .unwrap()
                .peer
                .ack_packet()
                .unwrap();
            assert!(lab.send(&datagram, address));
            ack_startup_packets(&mut lab, &socket, &mut client);
            let repaired = lab.readiness_counts();
            assert_eq!(repaired.controller_startup_actions_queued, 1);
            assert_eq!(repaired.controller_startup_actions_settled, 1);
            assert_eq!(repaired.controller_startup_actions_awaiting_ack, 0);
            assert_eq!(repaired.controller_startup_sequences_settled, 1);
        }
    }

    #[test]
    fn controller_startup_close_ack_is_not_an_open_channel_settlement_receipt() {
        let mut lab = prospect_refresh_lab();
        lab.enable_controller_startup(
            &ReleaseProfile::release_3_0_32(),
            vec![ControllerStartupAction::FlushLevelStreaming],
        )
        .unwrap();
        let (socket, address, mut client) = acked_game_state_peer(&mut lab);
        lab.game_state_open = None;
        assert!(lab.send_player_controller_open(address));
        ack_startup_packets(&mut lab, &socket, &mut client);
        lab.retry_controller_startup();
        let channel = lab.peers[&address].player_controller_channel.unwrap();
        let close = lab
            .peers
            .get_mut(&address)
            .unwrap()
            .peer
            .reliable_actor_close(channel, 0)
            .unwrap();
        assert!(lab.send(&close, address));
        assert!(!lab.peers[&address].peer.actor_channel_settled(channel));
        ack_startup_packets(&mut lab, &socket, &mut client);
        assert!(!lab.peers[&address].peer.has_actor_channel(channel));
        let closed = lab.readiness_counts();
        assert_eq!(closed.controller_startup_actions_settled, 0);
        assert_eq!(closed.controller_startup_actions_awaiting_ack, 1);
        assert_eq!(closed.controller_startup_sequences_settled, 0);
        assert_eq!(lab.readiness_counts(), closed);
    }

    #[test]
    fn controller_startup_aborts_after_failed_initial_open_without_publishing() {
        let mut lab = prospect_refresh_lab();
        lab.enable_controller_startup(
            &ReleaseProfile::release_3_0_32(),
            vec![ControllerStartupAction::FlushLevelStreaming],
        )
        .unwrap();
        let (_socket, address, _) = acked_game_state_peer(&mut lab);
        // Authored terminal initial-stage failure: it cannot become a startup
        // success merely because the remaining stage queue is now empty.
        let entry = lab.peers.get_mut(&address).unwrap();
        entry.initial_open_outcomes.failed.pawn = 1;
        entry.initial_opens = Some(PendingInitialOpens {
            remaining: VecDeque::new(),
            controller_guids: None,
            player_state_is_host: true,
            begin_slot: None,
            retry_ready: true,
        });
        lab.retry_initial_opens();
        lab.retry_controller_startup();
        assert_eq!(
            lab.readiness_counts().controller_startup_sequences_failed,
            1
        );
        assert_eq!(lab.readiness_counts().controller_startup_actions_queued, 0);
        assert_eq!(lab.readiness_counts().controller_startup_actions_pending, 0);
    }

    #[test]
    fn controller_startup_restart_uses_each_opened_pawn_and_waits_for_its_ack() {
        use super::super::{
            actor_channel::read_actor_bunch_body,
            packet::{Receiver, read_packet},
            pawn_open::PAWN_CHANNEL_INDEX,
            rep_rpc::parse_rpc_payload,
        };
        let profile = ReleaseProfile::release_3_0_32();
        let mut lab = prospect_refresh_lab();
        lab.enable_pawn_open(
            profile
                .pawn_open_plan(PeerLabAdmission::default().map_url)
                .unwrap()
                .unwrap(),
            PAWN_CHANNEL_INDEX,
            PawnContent::default(),
        )
        .unwrap();
        lab.enable_controller_startup(&profile, vec![ControllerStartupAction::RestartPawn])
            .unwrap();
        let mut fixtures = Vec::new();
        for connection in [1, 2] {
            let (socket, address, client) = acked_game_state_peer(&mut lab);
            lab.peers.get_mut(&address).unwrap().connection_id = Some(connection);
            fixtures.push((socket, address, client, connection));
        }
        lab.game_state_open = None;
        let mut pawn_guids = Vec::new();
        for (socket, address, client, connection) in &mut fixtures {
            assert!(lab.send_player_controller_open(*address));
            let expected = lab.guid_cache.actor_guid(pawn_key(*connection)).unwrap();
            assert_eq!(
                lab.peers[address].controller_startup_pawn_guid,
                Some(expected)
            );
            pawn_guids.push(expected);
            lab.retry_controller_startup();
            assert_eq!(lab.peers[address].controller_startup_queued, 0);
            ack_startup_packets(&mut lab, socket, client);
            // Keep only the pawn channel unsettled, independently of the
            // controller. This authored reliable body is transport test data.
            let codec = RepLayoutCodec {
                actor: actor_codec(
                    profile.engine_network_version,
                    super::super::archive::ByteOrder::Little,
                ),
                checksums: false,
                limits: Default::default(),
            };
            let payload = controller_startup_payload(
                ControllerStartupAction::FlushLevelStreaming,
                lab.controller_startup_bound,
                None,
                &codec,
            )
            .unwrap();
            let sent = lab
                .peers
                .get_mut(address)
                .unwrap()
                .peer
                .reliable_actor_bunch_datagrams(PAWN_CHANNEL_INDEX, payload)
                .unwrap();
            assert!(lab.send_all(&sent, *address));
            assert!(
                lab.peers[address].peer.actor_channel_settled(
                    lab.player_controller_open.as_ref().unwrap().channel_index
                )
            );
            lab.retry_controller_startup();
            assert_eq!(lab.peers[address].controller_startup_queued, 0);
            ack_startup_packets(&mut lab, socket, client);
            lab.retry_controller_startup();
            assert_eq!(lab.peers[address].controller_startup_queued, 1);
            let mut bytes = [0; 2048];
            let length = socket.recv(&mut bytes).unwrap();
            let packet = read_packet(
                &bytes[..length],
                &super::super::packet::PacketConfig {
                    receiver: Receiver::Client,
                    ..lab.config.packet.packet
                },
            )
            .unwrap();
            assert_eq!(packet.bunches.len(), 1);
            assert_eq!(
                packet.bunches[0].header.channel_index,
                lab.player_controller_open.as_ref().unwrap().channel_index
            );
            let body =
                read_actor_bunch_body(&mut packet.bunches[0].payload.reader(), false, &codec.actor)
                    .unwrap();
            let field = read_field_header(
                &mut body.blocks[0].payload.reader(),
                lab.controller_startup_bound,
                codec.actor.limits.max_payload_bits,
            )
            .unwrap();
            assert_eq!(field.index, 40);
            let args = parse_rpc_payload(
                &field.payload,
                &player_controller_rpc_parameters(40).unwrap(),
                &codec,
            )
            .unwrap();
            assert_eq!(
                args,
                vec![RpcArgument::Sent(vec![RpcItem::Value(RepValue::Object(
                    super::super::actor_channel::NetGuidRef::plain(expected)
                ))])]
            );
        }
        assert_ne!(pawn_guids[0], pawn_guids[1]);
        lab.shutdown();
        assert_eq!(lab.readiness_counts(), ReadinessCounts::default());
    }

    #[test]
    fn deferred_controller_reports_live_sequence_and_failed_begin_counts_once() {
        let mut lab = prospect_refresh_lab();
        let (_socket, address, _) = acked_game_state_peer(&mut lab);
        let entry = lab.peers.get_mut(&address).unwrap();
        let mut config = *entry.peer.config();
        config.sent_capacity = 1;
        entry.peer = LabPeer::from_cookie(&entry.cookie, config).unwrap();
        entry.peer.ack_packet().unwrap();
        assert!(lab.send_reply(address, &Reply::OpenPlayerController));
        assert!(lab.peers[&address].player_controller_channel.is_none());
        assert!(lab.peers[&address].initial_opens.is_some());
        assert_eq!(lab.readiness_counts().pending_initial_opens, 1);
        assert_eq!(lab.readiness_counts().sent_packets_pending, 1);
        assert_eq!(lab.stats.player_controller_open_errors, 1);
        let outcomes = lab.readiness_counts().initial_open_outcomes;
        assert_eq!(outcomes.configured.controller, 1);
        assert_eq!(outcomes.pending.controller, 1);
        assert_eq!(outcomes.pending.game_state, 1);
        assert_eq!(outcomes.deferred.controller, 1);
        assert_eq!(outcomes.deferred.game_state, 0);
        assert!(outcomes.failed.is_empty());
        assert!(outcomes.settled.is_empty());
        assert!(outcomes.awaiting_ack.is_empty());
        assert_eq!(outcomes.sequences_settled, 0);
        for _ in 0..4 {
            lab.retry_initial_opens();
        }
        assert_eq!(lab.readiness_counts().initial_open_outcomes, outcomes);

        // No PlayerState plan: the pending stage must terminate and count the
        // retained request exactly once, without starting a profile exchange.
        let entry = lab.peers.get_mut(&address).unwrap();
        entry.initial_opens = Some(PendingInitialOpens {
            remaining: VecDeque::from([InitialOpenStage::PlayerState]),
            controller_guids: Some((2, 4)),
            player_state_is_host: true,
            begin_slot: Some(3),
            retry_ready: true,
        });
        lab.retry_initial_opens();
        assert!(lab.peers[&address].initial_opens.is_none());
        assert_eq!(lab.stats.initialisation_calls_ignored, 1);
        assert_eq!(lab.stats.profile_user_requests_sent, 0);
        lab.retry_initial_opens();
        assert_eq!(lab.stats.initialisation_calls_ignored, 1);
    }

    /// Retry scheduling must not turn one pawn into several UID allocation
    /// attempts. The seeded collision also makes leaked historical remaps visible.
    #[test]
    fn pending_pawn_keeps_one_uid_and_releases_it_on_terminal_paths() {
        use super::super::{
            pawn_open::{PAWN_CHANNEL_INDEX, character_state_handle as h},
            release::ReleaseProfile,
        };
        use crate::actor_uids::{ActorUidLibrary, FIRST_NATIVE_OBJECT_ID};

        for terminal in ["commit", "disconnect", "permanent_failure"] {
            let mut lab = prospect_refresh_lab();
            lab.enable_pawn_open(
                ReleaseProfile::release_3_0_32()
                    .pawn_open_plan(PeerLabAdmission::default().map_url)
                    .unwrap()
                    .unwrap(),
                PAWN_CHANNEL_INDEX,
                PawnContent::default(),
            )
            .unwrap();
            lab.enable_actor_uids(ActorUidLibrary::new(1)).unwrap();
            lab.seed_actor_uids([FIRST_NATIVE_OBJECT_ID]).unwrap();
            let (_socket, address, _) = acked_game_state_peer(&mut lab);
            let entry = lab.peers.get_mut(&address).unwrap();
            let cookie = entry.cookie;
            let mut packet_config = *entry.peer.config();
            packet_config.sent_capacity = 1;
            // Replace only the authored transport fixture, leaving one live
            // record so any otherwise valid pawn open encounters drainable pressure.
            entry.peer = LabPeer::from_cookie(&cookie, packet_config).unwrap();
            entry.game_state = None;
            entry.connection_id = Some(1);
            entry.peer.ack_packet().unwrap();
            entry.initial_opens = Some(PendingInitialOpens {
                remaining: VecDeque::from([InitialOpenStage::Pawn]),
                controller_guids: Some((2, 4)),
                player_state_is_host: false,
                begin_slot: None,
                retry_ready: true,
            });
            lab.retry_initial_opens();
            let uid = FIRST_NATIVE_OBJECT_ID + 1;
            assert_eq!(
                lab.readiness_counts().initial_open_outcomes.deferred.pawn,
                1
            );
            assert_eq!(lab.readiness_counts().initial_open_outcomes.failed.pawn, 0);
            assert_eq!(lab.peers[&address].pending_pawn_uid, Some(uid));
            assert_eq!(lab.peers[&address].pawn_uid, None);
            assert_eq!(lab.next_pawn_object_id, Some(FIRST_NATIVE_OBJECT_ID + 1));
            assert_eq!(lab.claimed_actor_uids(), &[FIRST_NATIVE_OBJECT_ID, uid]);
            let remaps = lab.actor_uids.as_ref().unwrap().remaps().to_vec();
            assert_eq!(remaps, vec![(FIRST_NATIVE_OBJECT_ID, uid)]);
            let first_errors = lab.stats.pawn_open_errors;
            assert_eq!(first_errors, 1);

            // Repeated scheduler passes without notifications do no work.
            for _ in 0..4 {
                lab.retry_initial_opens();
            }
            assert_eq!(lab.stats.pawn_open_errors, first_errors);
            // Model bounded notification-driven attempts that still find the
            // ring occupied. Each must reuse the original pending allocation.
            for _ in 0..8 {
                lab.peers
                    .get_mut(&address)
                    .unwrap()
                    .initial_opens
                    .as_mut()
                    .unwrap()
                    .retry_ready = true;
                lab.retry_initial_opens();
                assert_eq!(lab.peers[&address].pending_pawn_uid, Some(uid));
                assert_eq!(lab.peers[&address].pawn_uid, None);
                assert_eq!(lab.next_pawn_object_id, Some(FIRST_NATIVE_OBJECT_ID + 1));
                assert_eq!(lab.actor_uids.as_ref().unwrap().remaps(), remaps);
                assert_eq!(lab.claimed_actor_uids(), &[FIRST_NATIVE_OBJECT_ID, uid]);
            }
            assert_eq!(lab.stats.pawn_open_errors, first_errors + 8);

            match terminal {
                "commit" => {
                    // A roomy empty transport isolates the successful ownership
                    // transfer from pending to published; no new allocation is needed.
                    packet_config.sent_capacity = 256;
                    lab.peers.get_mut(&address).unwrap().peer =
                        LabPeer::from_cookie(&cookie, packet_config).unwrap();
                    lab.peers
                        .get_mut(&address)
                        .unwrap()
                        .initial_opens
                        .as_mut()
                        .unwrap()
                        .retry_ready = true;
                    lab.retry_initial_opens();
                    assert_eq!(lab.stats.pawn_opens_sent, 1);
                    assert_eq!(lab.peers[&address].pending_pawn_uid, None);
                    assert_eq!(lab.peers[&address].pawn_uid, Some(uid));
                    assert!(lab.peers[&address].initial_opens.is_none());
                    assert_eq!(
                        lab.readiness_counts().initial_open_outcomes.committed.pawn,
                        1
                    );
                    assert_eq!(
                        lab.readiness_counts().initial_open_outcomes.deferred.pawn,
                        0
                    );
                    assert_eq!(lab.claimed_actor_uids(), &[FIRST_NATIVE_OBJECT_ID, uid]);
                    assert!(lab.remove_peer(address).is_some());
                }
                "disconnect" => {
                    assert!(lab.remove_peer(address).is_some());
                }
                "permanent_failure" => {
                    // A malformed authored temperature is a permanent composition
                    // failure even while outstanding transport records remain.
                    lab.pawn_open
                        .as_mut()
                        .unwrap()
                        .2
                        .character_state
                        .insert(h::INTERNAL_TEMPERATURE, RepValue::Bool(true));
                    lab.peers
                        .get_mut(&address)
                        .unwrap()
                        .initial_opens
                        .as_mut()
                        .unwrap()
                        .retry_ready = true;
                    lab.retry_initial_opens();
                    assert_eq!(lab.stats.pawn_opens_sent, 0);
                    assert_eq!(lab.peers[&address].pending_pawn_uid, None);
                    assert_eq!(lab.peers[&address].pawn_uid, None);
                    assert!(lab.peers[&address].initial_opens.is_none());
                    assert!(!lab.peers[&address].peer.is_failed());
                    assert_eq!(lab.readiness_counts().initial_open_outcomes.failed.pawn, 1);
                    assert_eq!(
                        lab.readiness_counts().initial_open_outcomes.deferred.pawn,
                        0
                    );
                }
                _ => unreachable!(),
            }
            assert_eq!(lab.claimed_actor_uids(), &[FIRST_NATIVE_OBJECT_ID]);
            assert_eq!(lab.next_pawn_object_id, Some(FIRST_NATIVE_OBJECT_ID + 1));
            assert_eq!(lab.actor_uids.as_ref().unwrap().remaps(), remaps);
        }
    }

    /// An initial open larger than the entire ring is not retryable once
    /// that ring is empty. Existing traffic may defer its first attempt, but
    /// unchanged polls cannot repeat it, and the real ACK path clears it.
    #[test]
    fn initial_open_oversize_stops_after_drain_and_unchanged_polls_do_not_retry() {
        use super::super::{
            game_state_open::handle,
            packet::{Packet, Receiver, read_packet, write_packet},
            packet_lab::client_cookie_seeds,
            sequencing::PacketSequencer,
        };

        for initially_full in [false, true] {
            let mut lab = prospect_refresh_lab();
            let (socket, address, _) = acked_game_state_peer(&mut lab);
            lab.game_state_open.as_mut().unwrap().state.values.insert(
                handle::RICH_PRESENCE_GUID,
                RepValue::Str(ArchiveText::text(&"G".repeat(1_500)).unwrap()),
            );
            // Reset only the authored transport fixture: one valid GameState
            // open now needs more datagrams than this entire one-record ring.
            let entry = lab.peers.get_mut(&address).unwrap();
            let cookie = entry.cookie;
            let mut config = *entry.peer.config();
            config.sent_capacity = 1;
            entry.peer = LabPeer::from_cookie(&cookie, config).unwrap();
            entry.game_state = None;
            let outstanding = initially_full.then(|| entry.peer.ack_packet().unwrap());
            entry.initial_opens = Some(PendingInitialOpens {
                remaining: VecDeque::from([InitialOpenStage::GameState]),
                controller_guids: None,
                player_state_is_host: false,
                begin_slot: None,
                retry_ready: true,
            });
            let before = lab.stats.game_state_open_errors;
            lab.retry_initial_opens();
            assert_eq!(lab.stats.game_state_open_errors, before + 1);
            assert_eq!(lab.peers[&address].initial_opens.is_some(), initially_full);
            let packet_id = lab.peers[&address].peer.sequencer().out_packet_id();
            for _ in 0..8 {
                lab.poll();
            }
            assert_eq!(lab.stats.game_state_open_errors, before + 1);
            assert_eq!(
                lab.peers[&address].peer.sequencer().out_packet_id(),
                packet_id
            );

            if let Some(outstanding) = outstanding {
                assert!(
                    !lab.peers[&address]
                        .initial_opens
                        .as_ref()
                        .unwrap()
                        .retry_ready,
                    "the deferred stage waits for packet notification progress"
                );
                let (incoming, outgoing) = client_cookie_seeds(&cookie);
                let mut client = PacketSequencer::new(incoming, outgoing, 8).unwrap();
                let mut to_client = lab.config.packet.packet;
                to_client.receiver = Receiver::Client;
                let sent = read_packet(&outstanding, &to_client).unwrap();
                let received = client.receive(&sent.header).unwrap().unwrap();
                client.ack_seq(received.sequence, true).unwrap();
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
                socket.send(&ack).unwrap();
                lab.poll();
                assert_eq!(lab.stats.game_state_open_errors, before + 2);
                assert!(lab.peers[&address].initial_opens.is_none());
                assert!(lab.peers[&address].game_state.is_none());
            }
            let errors = lab.stats.game_state_open_errors;
            for _ in 0..8 {
                lab.poll();
            }
            assert_eq!(lab.stats.game_state_open_errors, errors);
            assert!(lab.peers[&address].initial_opens.is_none());
            assert!(!lab.peers[&address].peer.is_failed());
            let outcomes = lab.readiness_counts().initial_open_outcomes;
            assert_eq!(outcomes.failed.game_state, 1);
            assert_eq!(outcomes.committed.game_state, 0);
            assert!(outcomes.pending.is_empty());
            assert!(outcomes.deferred.is_empty());
        }
    }

    #[test]
    fn returning_restore_reexports_unacknowledged_components_once() {
        use super::super::{
            actor_channel::{BlockTarget, read_actor_bunch_body, read_export_prefix},
            archive::ByteOrder,
            control_messages::{NetIdType, NetIdValue, UniqueNetId},
            packet::{PacketConfig, Receiver, read_packet},
            pawn_open::{PAWN_CHANNEL_INDEX, character_state_handle as h},
            recorders::{SavedPlayerCharacter, SavedPlayerState},
            release::ReleaseProfile,
            rep_layout::read_rep_properties,
            sequencing::{PartialBunchReassembler, PartialReceive},
        };

        // Exercise the finalisation path with pending, lost and acknowledged
        // open exports. Only the export delivery state differs; every value
        // is authored. No inventory, modifier or stat delta can select the
        // multi-datagram path on behalf of the component export.
        for export_state in ["pending", "lost", "acked"] {
            let mut lab = prospect_refresh_lab();
            let plan = ReleaseProfile::release_3_0_32()
                .pawn_open_plan(PeerLabAdmission::default().map_url)
                .unwrap()
                .unwrap();
            let layout = plan.content.character_state_layout.clone();
            let engine = plan.engine_net_version;
            lab.enable_pawn_open(
                plan,
                PAWN_CHANNEL_INDEX,
                PawnContent {
                    character_state: BTreeMap::from([
                        (h::HEALTH, RepValue::Int(100)),
                        (h::MAX_HEALTH, RepValue::Int(100)),
                    ]),
                    ..Default::default()
                },
            )
            .unwrap();
            lab.enable_actor_uids(crate::actor_uids::ActorUidLibrary::new(1))
                .unwrap();
            lab.seed_actor_uids([70_001]).unwrap();
            lab.enable_returning_characters(vec![SavedPlayerCharacter {
                player_id: "AuthoredPlayer".encode_utf16().collect(),
                chr_slot: 3,
                icarus_uid: 70_001,
                state: SavedPlayerState {
                    health: Some(37),
                    ..Default::default()
                },
                inventories: Vec::new(),
                modifiers: Vec::new(),
            }])
            .unwrap();
            let (socket, address, _client) = acked_game_state_peer(&mut lab);
            let entry = lab.peers.get_mut(&address).unwrap();
            entry.connection_id = Some(1);
            entry.login_unique_id = Some(UniqueNetId {
                kind: NetIdType::Named(ArchiveText::text("NULL").unwrap()),
                value: NetIdValue::Text(ArchiveText::text("AuthoredPlayer").unwrap()),
            });
            entry.profiles.character_slot = Some(3);
            entry.profiles.completed = true;
            entry.profiles.friend_ids_ready = true;
            let first_packet = entry.peer.sequencer().out_packet_id();
            lab.send_pawn_open(address, 2, 4);
            assert_eq!(lab.stats.pawn_opens_sent, 1);
            let entry = lab.peers.get_mut(&address).unwrap();
            let after_open = entry.peer.sequencer().out_packet_id();
            let mut buffer = [0; 2048];
            for packet in first_packet..after_open {
                socket.recv(&mut buffer).unwrap();
                match export_state {
                    "lost" => entry.exports.received_nak(packet),
                    "acked" => entry.exports.received_ack(packet),
                    _ => {}
                }
            }
            lab.finalise_connected_players();
            assert_eq!(lab.stats.returning_pawns_restored, 1, "{export_state}");
            assert_eq!(lab.stats.returning_pawn_updates_sent, 1, "{export_state}");
            assert_eq!(lab.stats.returning_pawn_update_errors, 0, "{export_state}");
            let count = lab.peers[&address].peer.sequencer().out_packet_id() - after_open;
            let codec = RepLayoutCodec {
                actor: actor_codec(engine, ByteOrder::Little),
                checksums: false,
                limits: Default::default(),
            };
            let mut exported_component = None;
            let mut health = None;
            let mut reassembler = PartialBunchReassembler::new();
            let mut sequence = 0;
            for _ in 0..count {
                let length = socket.recv(&mut buffer).unwrap();
                let packet = read_packet(
                    &buffer[..length],
                    &PacketConfig {
                        receiver: Receiver::Client,
                        ..lab.config.packet.packet
                    },
                )
                .unwrap();
                for bunch in packet.bunches {
                    sequence += 1;
                    assert_eq!(bunch.header.channel_index, PAWN_CHANNEL_INDEX);
                    assert!(bunch.header.reliable);
                    assert!(!bunch.header.open);
                    let mut reader = bunch.payload.reader();
                    let payload = if bunch.header.has_package_map_exports {
                        let refs = read_export_prefix(&mut reader, &codec.actor).unwrap();
                        assert_eq!(refs.len(), 1);
                        assert!(health.is_none(), "exports precede restored state");
                        exported_component = Some(refs[0].guid);
                        assert!(reader.at_end());
                        Payload::from_writer(super::super::packet::BitWriter::new())
                    } else {
                        bunch.payload
                    };
                    let payload = if bunch.header.partial {
                        let PartialReceive::Complete { bunch, .. } = reassembler
                            .receive_part(sequence, &bunch.header, &payload, Some(4096))
                            .unwrap()
                        else {
                            continue;
                        };
                        bunch.payload
                    } else {
                        payload
                    };
                    let body =
                        read_actor_bunch_body(&mut payload.reader(), false, &codec.actor).unwrap();
                    for block in body.blocks {
                        let BlockTarget::SubObject { object, .. } = block.target else {
                            continue; // The UID and pawn-property blocks.
                        };
                        if export_state != "acked" {
                            assert_eq!(exported_component, Some(object.guid));
                        }
                        let properties =
                            read_rep_properties(&mut block.payload.reader(), &layout, &codec)
                                .unwrap();
                        health = properties.entries.into_iter().find_map(|property| {
                            (property.handle == h::HEALTH).then_some(property.value)
                        });
                    }
                }
            }
            assert_eq!(health, Some(RepValue::Int(37)));
            assert_eq!(exported_component.is_some(), export_state != "acked");
            lab.finalise_connected_players();
            assert_eq!(lab.stats.players_finalised, 1);
            assert_eq!(lab.stats.returning_pawn_updates_sent, 1);
            assert_eq!(lab.claimed_actor_uids(), &[70_001]);
        }
    }

    #[test]
    fn parsed_user_response_survives_publication_pressure_without_reapplication() {
        let mut lab = prospect_refresh_lab();
        lab.config.packet.sent_capacity = 8;
        let (socket, address, mut client) = acked_game_state_peer(&mut lab);
        let state_channel = lab.game_state_open.as_ref().unwrap().channel_index;
        let profile = ReleaseProfile::release_3_0_32();
        lab.enable_player_state_open(
            profile
                .player_state_open_plan(PeerLabAdmission::default().map_url)
                .unwrap()
                .unwrap(),
        )
        .unwrap();
        // Reuse the fixture's real settled actor transport for publication;
        // this test concerns pressure and phase transitions, not actor spawn.
        lab.game_state_open = None;
        assert!(lab.send_player_controller_open(address));
        ack_startup_packets(&mut lab, &socket, &mut client);
        let entry = lab.peers.get_mut(&address).unwrap();
        entry.player_state_channel = Some(state_channel);
        entry.player = Some(RegisteredPlayer {
            player_id: 1,
            name_units: "AUTHORED-PROFILE-PRESSURE".encode_utf16().collect(),
        });
        entry.character_slot = Some(17);
        entry.profiles.outstanding = Some(ProfileKind::User);
        let capacity = entry.peer.config().sent_capacity;
        for _ in 0..capacity {
            let datagram = lab
                .peers
                .get_mut(&address)
                .unwrap()
                .peer
                .ack_packet()
                .unwrap();
            assert!(lab.send(&datagram, address));
        }
        let empty_array = || {
            RepValue::DynamicArray(super::super::rep_layout::RepArray {
                count: 0,
                stream: super::super::rep_layout::PropertyStream {
                    entries: vec![],
                    terminator_checksum: None,
                },
            })
        };
        lab.controller_call(
            address,
            ControllerCall::Profile(ProfileResponse {
                kind: ProfileKind::User,
                success: true,
                values: vec![
                    RepValue::Str(ArchiveText::text("authored").unwrap()),
                    empty_array(),
                    empty_array(),
                    empty_array(),
                    RepValue::Int(0),
                    RepValue::Int(0),
                ],
            }),
        );
        assert_eq!(lab.stats.profile_user_responses_recognized, 1);
        assert_eq!(lab.stats.profile_user_publications_deferred, 1);
        assert_eq!(lab.stats.profile_user_responses_received, 0);
        assert_eq!(lab.readiness_counts().players_awaiting_user_profile, 1);
        assert!(matches!(
            lab.peers[&address].pending_profile_exchange,
            Some(PendingProfileExchange::Publish(ProfileKind::User, _))
        ));
        for _ in 0..4 {
            lab.retry_pending_profile_exchanges();
        }
        assert_eq!(lab.stats.profile_user_publications_deferred, 1);
        ack_startup_packets(&mut lab, &socket, &mut client);
        lab.retry_pending_profile_exchanges();
        assert_eq!(lab.stats.profile_user_responses_received, 1);
        assert_eq!(lab.stats.profile_user_responses_recognized, 1);
        assert_eq!(lab.stats.profile_character_requests_sent, 1);
        for _ in 0..4 {
            lab.retry_pending_profile_exchanges();
        }
        assert_eq!(lab.stats.profile_user_responses_received, 1);
        assert_eq!(lab.stats.profile_character_requests_sent, 1);
        // A late duplicate is observed but cannot publish or advance again.
        lab.controller_call(
            address,
            ControllerCall::Profile(ProfileResponse {
                kind: ProfileKind::User,
                success: true,
                values: vec![],
            }),
        );
        assert_eq!(lab.stats.profile_user_responses_recognized, 2);
        assert_eq!(lab.stats.profile_responses_ignored, 1);
        assert_eq!(lab.stats.profile_user_responses_received, 1);
        let counts = lab.session_counts();
        assert_eq!(counts.profile_user_responses_recognized, 2);
        assert_eq!(counts.profile_user_publications_deferred, 1);
    }

    #[test]
    fn an_outstanding_profile_response_fails_a_peer_without_player_state() {
        let mut lab = prospect_refresh_lab();
        let (_socket, address, _client) = acked_game_state_peer(&mut lab);
        lab.peers.get_mut(&address).unwrap().profiles.outstanding = Some(ProfileKind::User);
        lab.controller_call(
            address,
            ControllerCall::Profile(ProfileResponse {
                kind: ProfileKind::User,
                success: true,
                values: Vec::new(),
            }),
        );
        assert!(!lab.has_peer(address));
        assert_eq!(
            (lab.stats.profile_players_failed, lab.stats.peers_failed),
            (1, 1)
        );
        assert_eq!(lab.stats.profile_user_responses_received, 0);
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
                    .insert(56, RepValue::Str(ArchiveText::new(&[256; 65_535]).unwrap()));
                (handle::ACTIVE_PROSPECT, status_values, prospect)
            } else {
                status.current_host_player_name = vec![256; 65_535];
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
    fn explicit_member_edits_and_departures_are_atomic() {
        use super::super::prospect_info::{AssociatedMember, ProspectInfo};
        let member = AssociatedMember {
            user_id: "USER".encode_utf16().collect(),
            ..Default::default()
        };
        let other = AssociatedMember {
            chr_slot: 1,
            ..member.clone()
        };
        let initial = ProspectInfo {
            elapsed_time: 123,
            associated_members: vec![member.clone(), other.clone()],
            ..Default::default()
        };
        let mut lab = prospect_refresh_lab();
        lab.enable_member_refresh(initial.clone()).unwrap();
        let mut edited = member.clone();
        edited.user_id = "user".encode_utf16().collect();
        edited.experience = 42;
        edited.settled = true;
        assert!(lab.edit_prospect_member(edited.clone()).unwrap());
        assert!(!lab.edit_prospect_member(edited.clone()).unwrap());
        // Selection folds ASCII case; publication preserves stored spelling.
        edited.user_id.clone_from(&member.user_id);
        let before = lab.game_state_open.as_ref().unwrap().clone();
        let mut invalid = edited.clone();
        invalid.status = u8::MAX;
        assert!(lab.edit_prospect_member(invalid).is_err());
        let after = lab.game_state_open.as_ref().unwrap();
        assert_eq!(after.member_refresh, before.member_refresh);
        assert_eq!(after.active_prospect, before.active_prospect);
        assert_eq!(after.state.values, before.state.values);
        let current = after.member_refresh.as_ref().unwrap();
        assert_eq!(current.elapsed_time, initial.elapsed_time);
        assert_eq!(
            current.associated_members,
            vec![edited.clone(), other.clone()]
        );
        let folded: Vec<u16> = "user".encode_utf16().collect();
        assert!(lab.remove_prospect_member(&folded, 0).unwrap());
        assert!(!lab.remove_prospect_member(&folded, 0).unwrap());
        assert!(!lab.edit_prospect_member(edited).unwrap());
        let current = lab
            .game_state_open
            .as_ref()
            .unwrap()
            .member_refresh
            .as_ref()
            .unwrap();
        assert_eq!(current.elapsed_time, initial.elapsed_time);
        assert_eq!(current.associated_members, vec![other]);
    }

    #[test]
    fn ambiguous_member_edits_reject_and_departure_removes_all_matching_duplicates() {
        use super::super::prospect_info::{AssociatedMember, ProspectInfo};
        let member = AssociatedMember {
            user_id: "USER".encode_utf16().collect(),
            ..Default::default()
        };
        let mut duplicate = member.clone();
        duplicate.user_id = "user".encode_utf16().collect();
        let other_slot = AssociatedMember {
            chr_slot: 1,
            ..member.clone()
        };
        let mut lab = prospect_refresh_lab();
        lab.enable_member_refresh(ProspectInfo {
            elapsed_time: 73,
            associated_members: vec![member.clone(), duplicate, other_slot.clone()],
            ..Default::default()
        })
        .unwrap();
        let before = lab.game_state_open.as_ref().unwrap().clone();
        let mut edit = member.clone();
        edit.experience = 999;
        assert!(!lab.edit_prospect_member(edit).unwrap());
        let after = lab.game_state_open.as_ref().unwrap();
        assert_eq!(after.member_refresh, before.member_refresh);
        assert_eq!(after.active_prospect, before.active_prospect);
        assert_eq!(after.state.values, before.state.values);
        assert!(lab.remove_prospect_member(&member.user_id, 0).unwrap());
        let current = lab
            .game_state_open
            .as_ref()
            .unwrap()
            .member_refresh
            .as_ref()
            .unwrap();
        assert_eq!(current.elapsed_time, 73);
        assert_eq!(current.associated_members, vec![other_slot]);
    }

    #[test]
    fn edited_membership_repairs_latest_array_and_new_peers_open_current_members() {
        use super::super::{
            archive::ByteOrder,
            prospect_info::{AssociatedMember, ProspectInfo},
            rep_layout::{
                PropertyStream, RepEntry, RepLayoutCodec, encode_rep_properties,
                read_rep_properties,
            },
        };
        let member = AssociatedMember {
            user_id: "USER".encode_utf16().collect(),
            ..Default::default()
        };
        let mut lab = prospect_refresh_lab();
        lab.enable_member_refresh(ProspectInfo {
            associated_members: vec![member.clone()],
            ..Default::default()
        })
        .unwrap();
        let (_old_socket, old_address, _) = acked_game_state_peer(&mut lab);
        let mut edited = member.clone();
        edited.experience = 42;
        assert!(lab.edit_prospect_member(edited).unwrap());
        let edited_values = lab
            .game_state_open
            .as_ref()
            .unwrap()
            .active_prospect
            .clone()
            .unwrap();
        let changed = lab
            .peers
            .get_mut(&old_address)
            .unwrap()
            .game_state
            .as_mut()
            .unwrap();
        let changing = edited_values
            .iter()
            .map(|(&h, v)| (h, Some(v.clone())))
            .collect::<Vec<_>>();
        let due = changed.due(0.0, &changing);
        assert!(due.contains_key(&59));
        changed.sent(&[10, 11], due, 0.0);
        changed.acked(10);
        assert!(changed.naked(11));
        assert_eq!(changed.due(0.0, &changing).get(&59), edited_values.get(&59));
        // A departure supersedes the lost edit; repair must clear the array.
        assert!(lab.remove_prospect_member(&member.user_id, 0).unwrap());
        let session = lab.game_state_open.as_ref().unwrap();
        let current = session.active_prospect.clone().unwrap();
        let layout = session.plan.layout.clone();
        let version = session.plan.engine_net_version;
        for order in [ByteOrder::Little, ByteOrder::Big] {
            let codec = RepLayoutCodec {
                actor: actor_codec(version, order),
                checksums: false,
                limits: Default::default(),
            };
            let stream = PropertyStream {
                entries: current
                    .iter()
                    .map(|(&handle, value)| RepEntry {
                        handle,
                        checksum: None,
                        value: value.clone(),
                    })
                    .collect(),
                terminator_checksum: None,
            };
            let wire = encode_rep_properties(&stream, &layout, &codec).unwrap();
            assert_eq!(
                read_rep_properties(&mut wire.reader(), &layout, &codec).unwrap(),
                stream
            );
        }
        let changing = current
            .iter()
            .map(|(&h, v)| (h, Some(v.clone())))
            .collect::<Vec<_>>();
        let replica = lab
            .peers
            .get_mut(&old_address)
            .unwrap()
            .game_state
            .as_mut()
            .unwrap();
        let repair = replica.due(0.0, &changing);
        assert_eq!(repair.get(&59), current.get(&59));
        replica.sent(&[12], repair, 0.0);
        replica.acked(12);
        assert!(replica.due(0.0, &changing).is_empty());
        // A fresh connection's ACKed open reconstructs the current membership,
        // even while the prior connection retains its independent baseline.
        let (_new_socket, new_address, _) = acked_game_state_peer(&mut lab);
        assert_ne!(new_address, old_address);
        assert_eq!(
            lab.peers[&new_address]
                .game_state
                .as_ref()
                .unwrap()
                .sent
                .get(&59),
            current.get(&59)
        );
    }

    #[test]
    fn explicit_spawn_membership_preserves_members_and_latest_prospect_atomically() {
        use super::super::prospect_info::{AssociatedMember, ProspectInfo, SpawnedProspectMember};
        let member = SpawnedProspectMember {
            user_id: "USER".encode_utf16().collect(),
            account_name: "ACCOUNT".encode_utf16().collect(),
            character_name: "CHARACTER".encode_utf16().collect(),
            chr_slot: i32::MIN,
            experience: i32::MAX,
        };
        assert_eq!(format!("{member:?}"), "SpawnedProspectMember");
        let mut lab = prospect_refresh_lab();
        assert!(lab.complete_prospect_member_spawn(member.clone()).is_err());
        let existing = AssociatedMember {
            user_id: "OTHER".encode_utf16().collect(),
            status: 7,
            settled: true,
            is_currently_playing: true,
            experience: -42,
            ..Default::default()
        };
        lab.enable_member_refresh(ProspectInfo {
            associated_members: vec![existing.clone()],
            ..Default::default()
        })
        .unwrap();
        let latest = ProspectInfo {
            lobby_name: "LATEST".encode_utf16().collect(),
            elapsed_time: 123,
            insurance: true,
            associated_members: vec![existing.clone()],
            ..Default::default()
        };
        lab.refresh_active_prospect(&latest).unwrap();
        assert!(lab.complete_prospect_member_spawn(member.clone()).unwrap());
        let current = lab
            .game_state_open
            .as_ref()
            .unwrap()
            .member_refresh
            .as_ref()
            .unwrap()
            .clone();
        assert_eq!(current.associated_members[0], existing);
        let added = &current.associated_members[1];
        assert_eq!(added.status, 2);
        assert!(!added.settled);
        assert!(!added.is_currently_playing);
        assert_eq!(added.user_id, member.user_id);
        assert_eq!(added.account_name, member.account_name);
        assert_eq!(added.character_name, member.character_name);
        assert_eq!(added.chr_slot, i32::MIN);
        assert_eq!(added.experience, i32::MAX);
        let mut without_addition = current.clone();
        without_addition.associated_members.pop();
        assert_eq!(without_addition, latest);
        let mut duplicate = member.clone();
        duplicate.user_id = "user".encode_utf16().collect();
        duplicate.character_name = "CHANGED".encode_utf16().collect();
        duplicate.experience = 0;
        assert!(!lab.complete_prospect_member_spawn(duplicate).unwrap());
        assert_eq!(
            lab.game_state_open
                .as_ref()
                .unwrap()
                .member_refresh
                .as_ref()
                .unwrap(),
            &current
        );
        let mut other_slot = member.clone();
        other_slot.chr_slot = i32::MAX;
        assert!(lab.complete_prospect_member_spawn(other_slot).unwrap());
        let before = lab.game_state_open.as_ref().unwrap().clone();
        let guids = format!("{:?}", lab.guid_cache);
        for bad_name in [vec![0], vec![65; 65_536], vec![256; 65_535]] {
            let mut invalid = member.clone();
            invalid.user_id = "UNIQUE".encode_utf16().collect();
            invalid.account_name = bad_name;
            assert!(lab.complete_prospect_member_spawn(invalid).is_err());
            let after = lab.game_state_open.as_ref().unwrap();
            assert_eq!(after.member_refresh, before.member_refresh);
            assert_eq!(after.active_prospect, before.active_prospect);
            assert_eq!(after.state.values, before.state.values);
            assert_eq!(format!("{:?}", lab.guid_cache), guids);
        }
        assert_eq!(
            lab.game_state_open
                .as_ref()
                .unwrap()
                .member_refresh
                .as_ref()
                .unwrap()
                .associated_members
                .len(),
            3
        );
    }

    #[test]
    fn prospect_send_pressure_leaves_clocks_running_and_same_refresh_due() {
        use super::super::{
            game_state_open::handle,
            packet::{Packet, PacketConfig, Receiver, read_packet, write_packet},
            prospect_info::{ProspectInfo, SpawnedProspectMember},
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
        lab.enable_member_refresh(large.clone()).unwrap();
        lab.refresh_active_prospect(&large).unwrap();
        let member = SpawnedProspectMember {
            user_id: vec![85],
            character_name: vec![67],
            ..Default::default()
        };
        assert!(lab.complete_prospect_member_spawn(member.clone()).unwrap());
        let mut second = member.clone();
        second.chr_slot = 1;
        assert!(lab.complete_prospect_member_spawn(second).unwrap());
        let mut edited = lab
            .game_state_open
            .as_ref()
            .unwrap()
            .member_refresh
            .as_ref()
            .unwrap()
            .associated_members[0]
            .clone();
        edited.experience = 42;
        edited.settled = true;
        assert!(lab.edit_prospect_member(edited.clone()).unwrap());
        assert!(lab.remove_prospect_member(&member.user_id, 1).unwrap());
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
        assert!(!lab.complete_prospect_member_spawn(member).unwrap());
        assert_eq!(
            lab.game_state_open
                .as_ref()
                .unwrap()
                .member_refresh
                .as_ref()
                .unwrap()
                .associated_members
                .len(),
            1
        );
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
        assert_eq!(
            update.get(&59),
            active_prospect_update_values(
                lab.game_state_open
                    .as_ref()
                    .unwrap()
                    .member_refresh
                    .as_ref()
                    .unwrap()
            )
            .unwrap()
            .get(&59)
        );
        assert!(matches!(update.get(&59), Some(RepValue::DynamicArray(array)) if array.count == 1));
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
                prospect_id: vec![65; 65_535],
                lobby_name: vec![66; 65_535],
                ..Default::default()
            },
            ProspectInfo {
                associated_members: vec![AssociatedMember::default(); 8_193],
                ..Default::default()
            },
            // Wide text fits 128 KiB, but its framing exceeds the actor block.
            ProspectInfo {
                lobby_name: vec![256; 65_535],
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
            lobby_name: vec![256; 50_000],
            elapsed_time: 9,
            ..Default::default()
        };
        // The prospect alone fits both the update and the open codecs.
        prospect_refresh_lab()
            .refresh_active_prospect(&large)
            .unwrap();
        let mut lab = prospect_refresh_lab();
        let mut status = NetworkingStatus::local(600.0);
        status.current_host_player_name = vec![256; 20_000];
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
            .expect_err("the combined actor content block exceeds 128 KiB");
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
    mod controller_field_diagnostic_tests {
        use super::super::super::{
            actor_channel::{NetGuidRef, SubObjectClass},
            archive::ByteOrder,
            packet::{BitWriter, Bunch, BunchHeader, Name, Packet, write_packet},
            sequencing::{PacketSequencer, wire_reliable_sequence},
            session_open::ACTOR_CHANNEL_NAME_INDEX,
        };
        use super::*;

        fn controller_peer(lab: &mut PeerLab) -> (UdpSocket, SocketAddrV4, PacketSequencer) {
            let (socket, address, mut client) = acked_game_state_peer(lab);
            // The base fixture has neither Login identity nor a controller channel.
            // Open the actual controller transport and retain its allocated component
            // GUIDs; never repurpose the fixture's GameState channel as a controller.
            assert!(matches!(
                lab.send_initial_controller_open(address),
                InitialOpenOutcome::Committed(_)
            ));
            ack_startup_packets(lab, &socket, &mut client);
            let entry = lab.peers.get_mut(&address).unwrap();
            entry.player = Some(RegisteredPlayer {
                player_id: i32::from(address.port()),
                name_units: "AUTHORED-DIAGNOSTIC-PLAYER".encode_utf16().collect(),
            });
            assert!(
                entry
                    .peer
                    .actor_channel_settled(entry.player_controller_channel.unwrap())
            );
            assert!(entry.backend_proxy.is_some());
            assert!(entry.player_data.is_some());
            assert!(entry.bestiary.is_some());
            (socket, address, client)
        }

        fn field_block(target: BlockTarget, bound: u32, fields: &[(u32, Payload)]) -> ContentBlock {
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

        fn subobject(guid: u32) -> BlockTarget {
            BlockTarget::SubObject {
                object: NetGuidRef::plain(guid),
                class: SubObjectClass::ClientSent,
            }
        }

        fn empty_payload() -> Payload {
            Payload::from_writer(BitWriter::new())
        }

        fn friends_payload(order: ByteOrder, friends: &[&str]) -> Payload {
            let argument = if friends.is_empty() {
                RpcArgument::NotSent
            } else {
                RpcArgument::Sent(vec![RpcItem::Array(
                    friends
                        .iter()
                        .map(|friend| {
                            vec![RpcItem::Value(RepValue::Str(
                                ArchiveText::text(friend).unwrap(),
                            ))]
                        })
                        .collect(),
                )])
            };
            encode_rpc_payload(
                &server_friends_list_updated_parameters(),
                &[argument],
                &RepLayoutCodec {
                    actor: actor_codec(0, order),
                    checksums: false,
                    limits: Default::default(),
                },
            )
            .unwrap()
        }

        #[test]
        fn friends_response_payload_validation_controls_finalisation() {
            for order in [ByteOrder::Little, ByteOrder::Big] {
                let valid = friends_payload(order, &["AUTHORED-FRIEND"]);
                let mut truncated = BitWriter::new();
                let mut reader = valid.reader();
                for _ in 0..valid.bit_len() - 1 {
                    truncated.write_bit(reader.read_bit());
                }
                let mut missing_count = BitWriter::new();
                missing_count.write_bit(true);
                let mut trailing = BitWriter::new();
                trailing.write_bit(false);
                trailing.write_bit(true);
                let mut oversized = BitWriter::new();
                oversized.write_bit(true);
                oversized.write_bits(0xffff, 16);
                for (index, payload) in [
                    empty_payload(),
                    Payload::from_writer(missing_count),
                    Payload::from_writer(truncated),
                    Payload::from_writer(trailing),
                    Payload::from_writer(oversized),
                    friends_payload(order, &[]),
                    valid,
                ]
                .into_iter()
                .enumerate()
                {
                    let expected_valid = index >= 5;
                    let mut lab = prospect_refresh_lab();
                    lab.config.packet.codec.order = order;
                    let (socket, address, mut client) = controller_peer(&mut lab);
                    // The friend response is the last blocker for this player.
                    lab.peers.get_mut(&address).unwrap().profiles.completed = true;
                    lab.finalise_connected_players();
                    assert_eq!(lab.stats.players_finalised, 0);
                    let payload = client_actor_body(
                        &lab,
                        vec![field_block(
                            BlockTarget::Actor,
                            BP_ICARUS_PLAYER_CONTROLLER_SURVIVAL_FIELD_INDEX_BOUND,
                            &[(SERVER_FRIENDS_LIST_UPDATED_FIELD_INDEX, payload)],
                        )],
                    );
                    let raw = wire_reliable_sequence(client.initial_reliable().1 + 1);
                    receive_controller_payload(&mut lab, &socket, &mut client, raw, payload);
                    lab.finalise_connected_players();
                    assert_eq!(
                        lab.peers[&address].profiles.friend_ids_ready,
                        expected_valid
                    );
                    assert_eq!(lab.stats.friends_lists_received, u64::from(expected_valid));
                    assert_eq!(lab.stats.players_finalised, u64::from(expected_valid));
                    assert_eq!(
                        lab.stats.actor_bunches_malformed,
                        u64::from(!expected_valid)
                    );
                    let diagnostics = lab.controller_field_diagnostics(address).unwrap();
                    assert_eq!(diagnostics.entries.len(), 1);
                    assert_eq!(
                        diagnostics.entries[0].disposition,
                        if expected_valid {
                            ControllerFieldDisposition::Recognized
                        } else {
                            ControllerFieldDisposition::Malformed
                        }
                    );
                }
            }
        }

        /// Raw archive string frames: declared count, then units.
        type StringFrames = Vec<(i32, Vec<u16>)>;

        /// Authored friends payload bits: a presence bit, then optionally an
        /// array count and raw string frames.
        fn friends_bits(
            order: ByteOrder,
            array: Option<(u16, StringFrames)>,
            trailing: bool,
        ) -> Payload {
            use super::super::super::rep_layout::{write_u16, write_u32};
            let mut writer = BitWriter::new();
            writer.write_bit(array.is_some());
            if let Some((count, strings)) = array {
                write_u16(&mut writer, count, order);
                for (declared, units) in strings {
                    write_u32(&mut writer, declared as u32, order);
                    for unit in units {
                        if declared < 0 {
                            write_u16(&mut writer, unit, order);
                        } else {
                            writer.write_bits(u32::from(unit), 8);
                        }
                    }
                }
            }
            if trailing {
                writer.write_bit(true);
            }
            Payload::from_writer(writer)
        }

        fn friends_codec(order: ByteOrder) -> RepLayoutCodec {
            RepLayoutCodec {
                actor: actor_codec(0, order),
                checksums: false,
                limits: Default::default(),
            }
        }

        fn rejection_cases(order: ByteOrder) -> Vec<(Payload, FriendsListRejection)> {
            use FriendsListRejection as R;
            let a = u16::from(b'A');
            let mut short_count = BitWriter::new();
            short_count.write_bit(true);
            short_count.write_bits(0, 8);
            vec![
                (empty_payload(), R::Truncated),
                (Payload::from_writer(short_count), R::Truncated),
                (
                    friends_bits(order, Some((2, vec![(1, vec![0])])), false),
                    R::Truncated,
                ),
                (
                    friends_bits(order, Some((1, vec![(5, vec![a, a])])), false),
                    R::Truncated,
                ),
                (
                    friends_bits(order, Some((4097, vec![])), false),
                    R::ArrayCountOverLimit,
                ),
                (
                    friends_bits(order, Some((1, vec![(65_537, vec![])])), false),
                    R::StringLengthOverLimit,
                ),
                (
                    friends_bits(order, Some((1, vec![(i32::MIN, vec![])])), false),
                    R::StringLengthOverLimit,
                ),
                (
                    friends_bits(order, Some((1, vec![(2, vec![a, a])])), false),
                    R::StringMissingTerminator,
                ),
                (
                    friends_bits(order, Some((1, vec![(-2, vec![a, a])])), false),
                    R::StringMissingTerminator,
                ),
                (
                    friends_bits(order, Some((1, vec![(2, vec![0x80, 0])])), false),
                    R::StringInvalidEncoding,
                ),
                (
                    friends_bits(order, Some((1, vec![(3, vec![a, 0, 0])])), false),
                    R::StringInvalidEncoding,
                ),
                (
                    friends_bits(order, Some((1, vec![(-2, vec![0xffff, 0])])), false),
                    R::StringInvalidEncoding,
                ),
                (friends_bits(order, None, true), R::TrailingBits),
                (
                    friends_bits(order, Some((1, vec![(2, vec![a, 0])])), true),
                    R::TrailingBits,
                ),
            ]
        }

        fn rejection_counts(reason: FriendsListRejection) -> FriendsListRejectionCounts {
            let mut counts = FriendsListRejectionCounts::default();
            counts.record(reason);
            counts
        }

        #[test]
        fn friends_rejection_counts_map_each_reason_to_one_key() {
            use FriendsListRejection as R;
            let reasons = [
                R::Truncated,
                R::ArrayCountOverLimit,
                R::EntryLimitExceeded,
                R::StringLengthOverLimit,
                R::StringMissingTerminator,
                R::StringInvalidEncoding,
                R::TrailingBits,
                R::Unclassified,
            ];
            let mut keys = std::collections::BTreeSet::new();
            for reason in reasons {
                let value = serde_json::to_value(rejection_counts(reason)).unwrap();
                let object = value.as_object().unwrap();
                assert_eq!(object.len(), reasons.len());
                let set: Vec<_> = object.iter().filter(|(_, count)| **count != 0).collect();
                assert_eq!(set.len(), 1);
                assert_eq!(set[0].1, 1);
                assert!(keys.insert(set[0].0.clone()));
            }
            let mut saturated = FriendsListRejectionCounts {
                trailing_bits: u64::MAX,
                ..Default::default()
            };
            saturated.record(R::TrailingBits);
            assert_eq!(saturated.trailing_bits, u64::MAX);
        }

        #[test]
        fn friends_rejection_classifier_agrees_with_the_parser() {
            let parameters = server_friends_list_updated_parameters();
            for order in [ByteOrder::Little, ByteOrder::Big] {
                let codec = friends_codec(order);
                for (payload, reason) in rejection_cases(order) {
                    assert!(parse_rpc_payload(&payload, &parameters, &codec).is_err());
                    assert_eq!(friends_list_rejection(&payload, &codec), reason);
                }
                for valid in [
                    friends_payload(order, &[]),
                    friends_payload(order, &["AUTHORED-FRIEND", "B"]),
                    friends_bits(order, Some((0, vec![])), false),
                    friends_bits(
                        order,
                        Some((2, vec![(0, vec![]), (-2, vec![0xd800, 0])])),
                        false,
                    ),
                ] {
                    assert!(parse_rpc_payload(&valid, &parameters, &codec).is_ok());
                    assert_eq!(
                        friends_list_rejection(&valid, &codec),
                        FriendsListRejection::Unclassified
                    );
                }
                // The entry budget: one for the array, one per string.
                let mut tight = friends_codec(order);
                tight.limits.max_entries = 2;
                let two = friends_bits(order, Some((2, vec![(0, vec![]), (0, vec![])])), false);
                assert!(parse_rpc_payload(&two, &parameters, &tight).is_err());
                assert_eq!(
                    friends_list_rejection(&two, &tight),
                    FriendsListRejection::EntryLimitExceeded
                );
                // Deterministic authored variations: the classifier names a
                // reason exactly when the parser rejects.
                let mut state = 0x9e37_79b9_u32;
                let mut next = |bound: u32| {
                    state ^= state << 13;
                    state ^= state >> 17;
                    state ^= state << 5;
                    state % bound
                };
                let declared = [-3, -2, -1, 0, 1, 2, 3, 65_537, i32::MIN];
                let units = [0, u16::from(b'A'), 0x80, 0xffff];
                for _ in 0..2000 {
                    let mut strings = Vec::new();
                    for _ in 0..next(4) {
                        let count = declared[next(declared.len() as u32) as usize];
                        let length = (count.unsigned_abs() as usize).min(3);
                        let body: Vec<u16> = (0..length)
                            .map(|_| units[next(units.len() as u32) as usize])
                            .collect();
                        strings.push((count, body));
                    }
                    let count = if next(16) == 0 {
                        4097
                    } else {
                        strings.len() as u16 + u16::from(next(4) == 0)
                    };
                    let full = friends_bits(
                        order,
                        (next(8) != 0).then_some((count, strings)),
                        next(4) == 0,
                    );
                    let keep = full.bit_len() - next(4).min(full.bit_len() as u32) as usize;
                    let mut cut = BitWriter::new();
                    let mut reader = full.reader();
                    for _ in 0..keep {
                        cut.write_bit(reader.read_bit());
                    }
                    let payload = Payload::from_writer(cut);
                    assert_eq!(
                        parse_rpc_payload(&payload, &parameters, &codec).is_err(),
                        friends_list_rejection(&payload, &codec)
                            != FriendsListRejection::Unclassified,
                    );
                }
            }
        }

        #[test]
        fn rejected_friends_responses_count_exactly_their_reason() {
            for order in [ByteOrder::Little, ByteOrder::Big] {
                for (payload, reason) in rejection_cases(order) {
                    let mut lab = prospect_refresh_lab();
                    lab.config.packet.codec.order = order;
                    let (socket, address, mut client) = controller_peer(&mut lab);
                    lab.peers.get_mut(&address).unwrap().profiles.completed = true;
                    let payload = client_actor_body(
                        &lab,
                        vec![field_block(
                            BlockTarget::Actor,
                            BP_ICARUS_PLAYER_CONTROLLER_SURVIVAL_FIELD_INDEX_BOUND,
                            &[(SERVER_FRIENDS_LIST_UPDATED_FIELD_INDEX, payload)],
                        )],
                    );
                    let raw = wire_reliable_sequence(client.initial_reliable().1 + 1);
                    receive_controller_payload(&mut lab, &socket, &mut client, raw, payload);
                    lab.finalise_connected_players();
                    assert_eq!(
                        lab.stats.friends_list_rejections,
                        rejection_counts(reason),
                        "{reason:?}"
                    );
                    assert_eq!(
                        lab.session_counts().friends_list_rejections,
                        rejection_counts(reason)
                    );
                    assert_eq!(lab.stats.actor_bunches_malformed, 1);
                    assert_eq!(lab.stats.friends_lists_received, 0);
                    assert_eq!(lab.stats.friends_lists_ignored_unregistered, 0);
                    assert_eq!(lab.stats.friends_lists_ignored_already_ready, 0);
                    assert_eq!(lab.stats.players_finalised, 0);
                    assert!(lab.has_peer(address));
                    assert!(!lab.peers[&address].profiles.friend_ids_ready);
                }
                // A malformed Begin is not a friends rejection.
                let mut lab = prospect_refresh_lab();
                lab.config.packet.codec.order = order;
                let (socket, address, mut client) = controller_peer(&mut lab);
                let mut begin = BitWriter::new();
                begin.write_bit(true);
                let payload = client_actor_body(
                    &lab,
                    vec![field_block(
                        BlockTarget::Actor,
                        BP_ICARUS_PLAYER_CONTROLLER_SURVIVAL_FIELD_INDEX_BOUND,
                        &[(
                            SERVER_BEGIN_PLAYER_INITIALISATION_FIELD_INDEX,
                            Payload::from_writer(begin),
                        )],
                    )],
                );
                let raw = wire_reliable_sequence(client.initial_reliable().1 + 1);
                receive_controller_payload(&mut lab, &socket, &mut client, raw, payload);
                assert_eq!(lab.stats.actor_bunches_malformed, 1);
                assert_eq!(
                    lab.stats.friends_list_rejections,
                    FriendsListRejectionCounts::default()
                );
                assert!(lab.has_peer(address));
            }
        }

        fn receive_controller_payload(
            lab: &mut PeerLab,
            socket: &UdpSocket,
            client: &mut PacketSequencer,
            reliable_raw: u32,
            payload: Payload,
        ) {
            let SocketAddr::V4(address) = socket.local_addr().unwrap() else {
                unreachable!("IPv4 fixture")
            };
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
                            reliable_sequence_raw: Some(reliable_raw),
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
            client.commit().unwrap();
            receive_authored_at(lab, socket, &bytes, Instant::now());
        }

        fn client_actor_body(lab: &PeerLab, blocks: Vec<ContentBlock>) -> Payload {
            encode_actor_bunch_body(
                &ActorBunchBody {
                    spawn: None,
                    blocks,
                },
                &actor_codec(
                    lab.player_controller_open
                        .as_ref()
                        .unwrap()
                        .engine_net_version,
                    lab.config.packet.codec.order,
                ),
            )
            .unwrap()
        }

        fn assert_diagnostics_eq(
            actual: &ControllerFieldDiagnostics,
            expected: &ControllerFieldDiagnostics,
        ) {
            assert_eq!(actual.entries, expected.entries);
            assert_eq!(actual.overflow, expected.overflow);
            assert_eq!(actual.malformed_bunches, expected.malformed_bunches);
            assert_eq!(actual.body_parse_failures, expected.body_parse_failures);
            assert_eq!(
                actual.block_payload_limit_rejections,
                expected.block_payload_limit_rejections
            );
            assert_eq!(
                actual.block_payload_length_rejections,
                expected.block_payload_length_rejections
            );
            assert_eq!(
                actual.max_rejected_block_payload_bits,
                expected.max_rejected_block_payload_bits
            );
            assert_eq!(
                actual.unknown_subobject_blocks,
                expected.unknown_subobject_blocks
            );
        }

        #[test]
        fn private_rpc82_opt_in_preserves_dispatch_and_suppresses_reliable_duplicates() {
            use super::super::super::selected_rpc_capture::PrivateRpc82Capture;
            use std::io::{Read, Seek, SeekFrom};
            let mut outcomes = Vec::new();
            for mode in 0..3 {
                let enabled = mode > 0;
                let mut lab = prospect_refresh_lab();
                let mut file = tempfile::tempfile().unwrap();
                let readonly = tempfile::NamedTempFile::new().unwrap();
                if enabled {
                    lab.install_private_rpc82_capture(PrivateRpc82Capture::from_file(
                        if mode == 2 {
                            std::fs::File::open(readonly.path()).unwrap()
                        } else {
                            file.try_clone().unwrap()
                        },
                    ))
                    .unwrap();
                }
                let (socket, address, mut client) = controller_peer(&mut lab);
                lab.peers.get_mut(&address).unwrap().connection_id = Some(1);
                let mut selected = BitWriter::new();
                for bit in 0..612 {
                    selected.write_bit(bit % 3 == 0);
                }
                let payload = client_actor_body(
                    &lab,
                    vec![field_block(
                        BlockTarget::Actor,
                        BP_ICARUS_PLAYER_CONTROLLER_SURVIVAL_FIELD_INDEX_BOUND,
                        &[
                            (82, Payload::from_writer(selected)),
                            (SERVER_SHORT_TIMEOUT_FIELD_INDEX, empty_payload()),
                        ],
                    )],
                );
                let raw = wire_reliable_sequence(client.initial_reliable().1 + 1);
                receive_controller_payload(&mut lab, &socket, &mut client, raw, payload.clone());
                receive_controller_payload(&mut lab, &socket, &mut client, raw, payload);
                assert!(lab.has_peer(address));
                assert_eq!(lab.stats.actor_duplicate_reliable, 1);
                assert_eq!(lab.stats.controller_fields_ignored, 1);
                let diagnostics = lab.controller_field_diagnostics(address).unwrap();
                assert_eq!(
                    diagnostics
                        .entries
                        .iter()
                        .find(|field| field.index == 82)
                        .unwrap()
                        .disposition,
                    ControllerFieldDisposition::Ignored
                );
                outcomes.push(serde_json::to_value(diagnostics).unwrap());
                if mode == 1 {
                    assert_eq!(lab.finish_private_rpc82_capture().unwrap().observations, 1);
                    file.seek(SeekFrom::Start(0)).unwrap();
                    let mut text = String::new();
                    file.read_to_string(&mut text).unwrap();
                    assert_eq!(text.lines().count(), 1);
                    let row: serde_json::Value = serde_json::from_str(text.trim()).unwrap();
                    assert_eq!(row["payload_bits"], 612);
                    assert!(!text.contains("AUTHORED-DIAGNOSTIC-PLAYER"));
                } else if mode == 2 {
                    let counts = lab.finish_private_rpc82_capture().unwrap();
                    assert_eq!(counts.observations, 0);
                    assert_eq!(counts.write_errors, 1);
                    assert_eq!(counts.incomplete, 1);
                    assert!(lab.has_peer(address));
                } else {
                    assert!(lab.private_rpc82_capture_counts().is_none());
                    assert_eq!(file.metadata().unwrap().len(), 0);
                }
            }
            assert_eq!(outcomes[0], outcomes[1]);
            assert_eq!(outcomes[0], outcomes[2]);
        }

        #[test]
        fn private_capture_and_live_dispatch_reassemble_independently_once() {
            use super::super::super::{
                selected_rpc_capture::PrivateRpc82Capture, sequencing::split_outgoing_bunch,
            };
            let mut lab = prospect_refresh_lab();
            let file = tempfile::tempfile().unwrap();
            lab.install_private_rpc82_capture(PrivateRpc82Capture::from_file(
                file.try_clone().unwrap(),
            ))
            .unwrap();
            let (socket, address, mut client) = controller_peer(&mut lab);
            lab.peers.get_mut(&address).unwrap().connection_id = Some(1);
            let mut selected = BitWriter::new();
            for _ in 0..612 {
                selected.write_bit(true);
            }
            let payload = client_actor_body(
                &lab,
                vec![field_block(
                    BlockTarget::Actor,
                    BP_ICARUS_PLAYER_CONTROLLER_SURVIVAL_FIELD_INDEX_BOUND,
                    &[(82, Payload::from_writer(selected))],
                )],
            );
            let channel = lab.peers[&address].player_controller_channel.unwrap();
            let whole = Bunch {
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
                    reliable_sequence_raw: Some(wire_reliable_sequence(
                        client.initial_reliable().1 + 1,
                    )),
                    partial_initial: false,
                    partial_final: false,
                    channel_name: Some(Name::Hardcoded(ACTOR_CHANNEL_NAME_INDEX)),
                },
                payload,
            };
            let parts = split_outgoing_bunch(&whole, &[], 512, Some(16 * 1024)).unwrap();
            assert_eq!(parts.len(), 2);
            // Queue the final part first. Neither an incomplete nor a queued
            // part may produce a record. Re-send it later as a duplicate.
            for (index, part) in [parts[1].clone(), parts[0].clone(), parts[1].clone()]
                .into_iter()
                .enumerate()
            {
                let bytes = write_packet(
                    &Packet {
                        magic: 0,
                        header: client.header().unwrap(),
                        info: None,
                        bunches: vec![part],
                    },
                    &lab.config.packet.packet,
                )
                .unwrap();
                client.commit().unwrap();
                receive_authored_at(&mut lab, &socket, &bytes, Instant::now());
                assert_eq!(
                    lab.private_rpc82_capture_counts().unwrap().observations,
                    u64::from(index > 0)
                );
            }
            assert_eq!(lab.finish_private_rpc82_capture().unwrap().incomplete, 0);
            assert_eq!(lab.stats.actor_duplicate_reliable, 1);
            assert_eq!(lab.stats.actor_bunches_dropped, 0);
            assert_eq!(lab.stats.actor_partial_completed, 1);
            assert_eq!(lab.stats.controller_fields_ignored, 1);
            assert_eq!(
                lab.controller_field_diagnostics(address)
                    .unwrap()
                    .entries
                    .len(),
                1
            );
            assert!(lab.has_peer(address));
        }

        #[test]
        fn fragmented_authored_user_profile_dispatches_once_after_complete_atomic_parse() {
            use super::super::super::{
                probe_initialise::authored_user_profile,
                rep_rpc::backend_profile_response_parameters,
                sequencing::{MAX_CONSTRUCTED_PARTIAL_BUNCH_BYTES, split_outgoing_bunch},
            };
            for failure in 0..2 {
                let malformed_tail = failure != 0;
                let mut lab = prospect_refresh_lab();
                let (socket, address, mut client) = controller_peer(&mut lab);
                let backend = lab.peers[&address].backend_proxy.unwrap();
                let engine = lab
                    .player_controller_open
                    .as_ref()
                    .unwrap()
                    .engine_net_version;
                let codec = RepLayoutCodec {
                    actor: actor_codec(engine, lab.config.packet.codec.order),
                    checksums: false,
                    limits: Default::default(),
                };
                let mut values = vec![RpcItem::Value(RepValue::Bool(true))];
                let mut profile = authored_user_profile().unwrap();
                profile[0] = RpcItem::Value(RepValue::Str(
                    ArchiveText::text(&"A".repeat(10000)).unwrap(),
                ));
                values.extend(profile);
                let response = encode_rpc_payload(
                    &backend_profile_response_parameters(false),
                    &[RpcArgument::Sent(values)],
                    &codec,
                )
                .unwrap();
                let valid = client_actor_body(
                    &lab,
                    vec![field_block(
                        subobject(backend),
                        BACKEND_PROXY_FIELD_INDEX_BOUND,
                        &[(RESPONSE_GET_USER_PROFILE, response)],
                    )],
                );
                let payload = if malformed_tail {
                    let mut writer = BitWriter::new();
                    let mut reader = valid.reader();
                    while reader.remaining() > 0 {
                        writer.write_bit(reader.read_bit());
                    }
                    // A stray bit after a response larger than the former
                    // 8 KiB cap starts a truncated content header, so the
                    // whole body fails without applying the response.
                    writer.write_bit(true);
                    Payload::from_writer(writer)
                } else {
                    valid.clone()
                };
                let first = client.initial_reliable().1 + 1;
                let whole = Bunch {
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
                        reliable_sequence_raw: Some(wire_reliable_sequence(first)),
                        partial_initial: false,
                        partial_final: false,
                        channel_name: Some(Name::Hardcoded(ACTOR_CHANNEL_NAME_INDEX)),
                    },
                    payload,
                };
                let parts = split_outgoing_bunch(
                    &whole,
                    &[],
                    7000,
                    Some(MAX_CONSTRUCTED_PARTIAL_BUNCH_BYTES),
                )
                .unwrap();
                assert!(parts.len() >= 3);
                // Final-first queues; gapfill completes exactly once. A repeat
                // of the final reliable value must never redispatch the RPC.
                let mut order = vec![parts.last().unwrap().clone()];
                order.extend(parts[..parts.len() - 1].iter().cloned());
                order.push(parts.last().unwrap().clone());
                for (index, part) in order.into_iter().enumerate() {
                    let bytes = write_packet(
                        &Packet {
                            magic: 0,
                            header: client.header().unwrap(),
                            info: None,
                            bunches: vec![part],
                        },
                        &lab.config.packet.packet,
                    )
                    .unwrap();
                    client.commit().unwrap();
                    receive_authored_at(&mut lab, &socket, &bytes, Instant::now());
                    if index < parts.len() - 1 {
                        assert_eq!(lab.stats.profile_user_responses_recognized, 0);
                    }
                }
                assert_eq!(lab.stats.actor_partial_completed, 1);
                assert_eq!(lab.stats.actor_duplicate_reliable, 1);
                assert_eq!(
                    lab.stats.profile_user_responses_recognized,
                    u64::from(!malformed_tail)
                );
                // No request was outstanding: successful parsing reaches the
                // existing phase guard, without profile publication or world claims.
                assert_eq!(
                    lab.stats.profile_responses_ignored,
                    u64::from(!malformed_tail)
                );
                assert_eq!(lab.stats.profile_user_responses_received, 0);
                if malformed_tail {
                    assert_eq!(
                        lab.controller_field_diagnostics(address)
                            .unwrap()
                            .malformed_bunches,
                        1
                    );
                    let body = lab.controller_field_diagnostics(address).unwrap();
                    assert_eq!(body.body_parse_failures, 1);
                    assert_eq!(body.block_payload_limit_rejections, 0);
                    assert_eq!(body.block_payload_length_rejections, 0);
                    assert_eq!(body.max_rejected_block_payload_bits, 0);
                    // The failed complete body does not poison the next sequence.
                    let recovery = split_outgoing_bunch(
                        &Bunch {
                            header: BunchHeader {
                                reliable_sequence_raw: Some(wire_reliable_sequence(
                                    first + parts.len() as u32,
                                )),
                                ..whole.header
                            },
                            payload: valid,
                        },
                        &[],
                        7000,
                        Some(MAX_CONSTRUCTED_PARTIAL_BUNCH_BYTES),
                    )
                    .unwrap();
                    for part in recovery {
                        let bytes = write_packet(
                            &Packet {
                                magic: 0,
                                header: client.header().unwrap(),
                                info: None,
                                bunches: vec![part],
                            },
                            &lab.config.packet.packet,
                        )
                        .unwrap();
                        client.commit().unwrap();
                        receive_authored_at(&mut lab, &socket, &bytes, Instant::now());
                    }
                    assert_eq!(lab.stats.profile_user_responses_recognized, 1);
                    assert_eq!(lab.stats.profile_responses_ignored, 1);
                }
            }
        }

        #[test]
        fn body_rejection_diagnostics_distinguish_available_oversize_from_bad_declaration() {
            use super::super::super::actor_channel::ContentBlockPayloadReadError;
            let limit = actor_codec(17, ByteOrder::Little).limits.max_payload_bits;
            let above = u32::try_from(limit + 1).unwrap();
            let mut omitted = BitWriter::new();
            omitted.write_bit(false);
            let valid = encode_actor_bunch_body(
                &ActorBunchBody {
                    spawn: None,
                    blocks: vec![field_block(
                        subobject(5),
                        BACKEND_PROXY_FIELD_INDEX_BOUND,
                        &[(RESPONSE_GET_USER_PROFILE, Payload::from_writer(omitted))],
                    )],
                },
                &actor_codec(17, ByteOrder::Little),
            )
            .unwrap();
            for (declared, available) in [(above, limit + 1), (above, 0), (16, 0)] {
                let mut writer = BitWriter::new();
                let mut prefix = valid.reader();
                while prefix.remaining() > 0 {
                    writer.write_bit(prefix.read_bit());
                }
                writer.write_bit(false); // no rep layout
                writer.write_bit(true); // actor block, no object identity
                writer.write_int_packed(declared);
                for _ in 0..available {
                    writer.write_bit(false);
                }
                let mut diagnostics = ControllerFieldDiagnostics::default();
                let error = controller_initialisation_calls(
                    &Payload::from_writer(writer),
                    17,
                    ByteOrder::Little,
                    [Some(5), None, None],
                    &mut diagnostics,
                )
                .err()
                .unwrap();
                let rejection = error
                    .downcast_ref::<ContentBlockPayloadReadError>()
                    .unwrap();
                if declared as usize > limit {
                    assert_eq!(
                        *rejection,
                        ContentBlockPayloadReadError::Limit {
                            declared_bits: declared as usize,
                            available_bits: available,
                            limit_bits: limit,
                        }
                    );
                    assert_eq!(
                        rejection.to_string(),
                        "content block payload exceeds the payload bit limit"
                    );
                } else {
                    assert_eq!(
                        *rejection,
                        ContentBlockPayloadReadError::Truncated {
                            declared_bits: declared as usize,
                            available_bits: available,
                        }
                    );
                    assert_eq!(
                        rejection.to_string(),
                        "content block payload exceeds the bunch"
                    );
                }
                assert_eq!(diagnostics.body_parse_failures, 1);
                assert_eq!(
                    diagnostics.block_payload_limit_rejections,
                    u64::from(available > 0)
                );
                assert_eq!(
                    diagnostics.block_payload_length_rejections,
                    u64::from(available == 0)
                );
                assert_eq!(
                    diagnostics.max_rejected_block_payload_bits,
                    if available > 0 {
                        u64::from(declared)
                    } else {
                        0
                    }
                );
                // Complete-body staging rejects even the valid UserProfile prefix.
                assert!(diagnostics.entries.is_empty());
            }
        }

        #[test]
        fn malformed_profile_and_unknown_target_do_not_dispatch_profile_calls() {
            let mut lab = prospect_refresh_lab();
            let (socket, address, mut client) = controller_peer(&mut lab);
            let entry = &lab.peers[&address];
            let backend = entry.backend_proxy.unwrap();
            let components = [entry.backend_proxy, entry.player_data, entry.bestiary];
            let unknown = (2..)
                .find(|guid| !components.contains(&Some(*guid)))
                .unwrap();
            // Identical malformed parameter bits on an unknown target must be
            // skipped, while the permitted target must diagnose parse failure.
            let payload = client_actor_body(
                &lab,
                vec![
                    field_block(
                        subobject(unknown),
                        BACKEND_PROXY_FIELD_INDEX_BOUND,
                        &[(RESPONSE_GET_USER_PROFILE, empty_payload())],
                    ),
                    field_block(
                        subobject(backend),
                        BACKEND_PROXY_FIELD_INDEX_BOUND,
                        &[(RESPONSE_GET_USER_PROFILE, empty_payload())],
                    ),
                ],
            );
            let raw = wire_reliable_sequence(client.initial_reliable().1 + 1);
            receive_controller_payload(&mut lab, &socket, &mut client, raw, payload);
            let diagnostics = lab.controller_field_diagnostics(address).unwrap();
            assert_eq!(diagnostics.unknown_subobject_blocks, 1);
            assert_eq!(diagnostics.malformed_bunches, 1);
            assert!(
                diagnostics
                    .entries
                    .iter()
                    .any(|entry| entry.index == RESPONSE_GET_USER_PROFILE
                        && entry.disposition == ControllerFieldDisposition::Malformed)
            );
            assert_eq!(lab.stats.profile_user_responses_recognized, 0);
            assert_eq!(lab.stats.profile_user_responses_received, 0);
            assert_eq!(lab.stats.profile_responses_ignored, 0);
            assert!(lab.has_peer(address));
            assert_eq!(lab.controller_field_totals().unknown_subobject_blocks, 1);
            // An omitted response struct is valid and retains Success=false.
            // With no request outstanding it is recognized, then phase-rejected.
            let mut writer = BitWriter::new();
            writer.write_bit(false);
            let payload = client_actor_body(
                &lab,
                vec![field_block(
                    subobject(backend),
                    BACKEND_PROXY_FIELD_INDEX_BOUND,
                    &[(RESPONSE_GET_USER_PROFILE, Payload::from_writer(writer))],
                )],
            );
            receive_controller_payload(
                &mut lab,
                &socket,
                &mut client,
                wire_reliable_sequence(raw + 1),
                payload,
            );
            assert_eq!(lab.stats.profile_user_responses_recognized, 1);
            assert_eq!(lab.stats.profile_responses_ignored, 1);
            assert_eq!(lab.stats.profile_user_responses_received, 0);
        }

        #[test]
        fn short_timeout_layout_requires_exactly_empty_parameters() {
            let parameters =
                player_controller_rpc_parameters(SERVER_SHORT_TIMEOUT_FIELD_INDEX).unwrap();
            assert!(parameters.is_empty());
            assert!(player_controller_rpc_parameters(82).is_none());
            for order in [ByteOrder::Little, ByteOrder::Big] {
                let codec = RepLayoutCodec {
                    actor: actor_codec(0, order),
                    checksums: false,
                    limits: Default::default(),
                };
                assert!(
                    parse_rpc_payload(&empty_payload(), &parameters, &codec)
                        .unwrap()
                        .is_empty()
                );
                assert_eq!(
                    encode_rpc_payload(&parameters, &[], &codec)
                        .unwrap()
                        .bit_len(),
                    0
                );
                for bit in [false, true] {
                    let mut writer = BitWriter::new();
                    writer.write_bit(bit);
                    let nonempty = Payload::from_writer(writer);
                    assert!(parse_rpc_payload(&nonempty, &parameters, &codec).is_err());
                }
            }
        }

        #[test]
        fn short_timeout_observation_is_controller_only_and_does_not_change_timeout_policy() {
            let mut lab = prospect_refresh_lab();
            let (socket, address, mut client) = controller_peer(&mut lab);
            let entry = &lab.peers[&address];
            let backend = entry.backend_proxy.unwrap();
            let components = [entry.backend_proxy, entry.player_data, entry.bestiary];
            // Deterministically choose a nondefault reference outside all
            // permitted subobjects; unknown objects are skipped before parsing.
            let unknown = (2..)
                .find(|guid| !components.contains(&Some(*guid)))
                .unwrap();
            let timeout = lab.config.idle_timeout;
            let payload = client_actor_body(
                &lab,
                vec![
                    field_block(
                        subobject(backend),
                        BACKEND_PROXY_FIELD_INDEX_BOUND,
                        &[(SERVER_SHORT_TIMEOUT_FIELD_INDEX, empty_payload())],
                    ),
                    field_block(
                        subobject(unknown),
                        BP_ICARUS_PLAYER_CONTROLLER_SURVIVAL_FIELD_INDEX_BOUND,
                        &[(SERVER_SHORT_TIMEOUT_FIELD_INDEX, empty_payload())],
                    ),
                    field_block(
                        BlockTarget::Actor,
                        BP_ICARUS_PLAYER_CONTROLLER_SURVIVAL_FIELD_INDEX_BOUND,
                        &[
                            (SERVER_SHORT_TIMEOUT_FIELD_INDEX, empty_payload()),
                            (SERVER_SHORT_TIMEOUT_FIELD_INDEX, empty_payload()),
                            (82, empty_payload()),
                        ],
                    ),
                ],
            );
            let raw = wire_reliable_sequence(client.initial_reliable().1 + 1);
            receive_controller_payload(&mut lab, &socket, &mut client, raw, payload);
            let diagnostics = lab.controller_field_diagnostics(address).unwrap();
            assert_eq!(diagnostics.entries.len(), 3);
            assert_eq!(diagnostics.unknown_subobject_blocks, 1);
            assert_eq!(diagnostics.malformed_bunches, 0);
            assert_eq!(diagnostics.overflow, 0);
            let recognized = diagnostics
                .entries
                .iter()
                .filter(|entry| entry.disposition == ControllerFieldDisposition::Recognized)
                .collect::<Vec<_>>();
            assert_eq!(recognized.len(), 1);
            assert_eq!(recognized[0].target, ControllerFieldTarget::Actor);
            assert_eq!(recognized[0].index, SERVER_SHORT_TIMEOUT_FIELD_INDEX);
            assert_eq!(recognized[0].payload_bits, 0);
            assert_eq!(recognized[0].count, 2);
            assert_eq!(lab.stats.controller_fields_ignored, 2);
            assert_eq!(lab.stats.actor_bunches_malformed, 0);
            assert_eq!(lab.stats.initialisation_calls_ignored, 0);
            assert_eq!(lab.character_slot(address), None);
            assert!(!lab.peers[&address].profiles.friend_ids_ready);
            assert_eq!(lab.config.idle_timeout, timeout);
            assert_diagnostics_eq(&lab.controller_field_totals(), diagnostics);
            let last_seen = lab.peers[&address].last_seen;
            lab.expire_idle(last_seen + timeout);
            assert!(lab.has_peer(address));
            lab.expire_idle(last_seen + timeout + Duration::from_millis(1));
            assert!(!lab.has_peer(address));
            assert_eq!(lab.stats.peers_expired, 1);
        }

        #[test]
        fn malformed_short_timeout_tail_discards_earlier_controller_calls() {
            for bit in [false, true] {
                let mut lab = prospect_refresh_lab();
                let (socket, address, mut client) = controller_peer(&mut lab);
                let mut writer = BitWriter::new();
                writer.write_bit(bit);
                let payload = client_actor_body(
                    &lab,
                    vec![field_block(
                        BlockTarget::Actor,
                        BP_ICARUS_PLAYER_CONTROLLER_SURVIVAL_FIELD_INDEX_BOUND,
                        &[
                            (
                                SERVER_FRIENDS_LIST_UPDATED_FIELD_INDEX,
                                friends_payload(ByteOrder::Little, &[]),
                            ),
                            (SERVER_SHORT_TIMEOUT_FIELD_INDEX, empty_payload()),
                            (
                                SERVER_SHORT_TIMEOUT_FIELD_INDEX,
                                Payload::from_writer(writer),
                            ),
                        ],
                    )],
                );
                let raw = wire_reliable_sequence(client.initial_reliable().1 + 1);
                receive_controller_payload(&mut lab, &socket, &mut client, raw, payload);
                let diagnostics = lab.controller_field_diagnostics(address).unwrap();
                assert_eq!(diagnostics.entries.len(), 3);
                assert_eq!(diagnostics.malformed_bunches, 1);
                assert_eq!(
                    diagnostics.entries[1].index,
                    SERVER_SHORT_TIMEOUT_FIELD_INDEX
                );
                assert_eq!(
                    diagnostics.entries[1].disposition,
                    ControllerFieldDisposition::Recognized
                );
                let malformed = &diagnostics.entries[2];
                assert_eq!(malformed.index, SERVER_SHORT_TIMEOUT_FIELD_INDEX);
                assert_eq!(malformed.payload_bits, 1);
                assert_eq!(malformed.disposition, ControllerFieldDisposition::Malformed);
                assert_eq!(lab.stats.actor_bunches_malformed, 1);
                assert_eq!(lab.stats.friends_lists_received, 0);
                assert_eq!(lab.stats.controller_fields_ignored, 0);
                assert!(!lab.peers[&address].profiles.friend_ids_ready);
                assert_eq!(lab.character_slot(address), None);
            }
        }

        #[test]
        fn subobject_isolation_preserves_permitted_field_classification() {
            let mut lab = prospect_refresh_lab();
            let (socket, address, mut client) = controller_peer(&mut lab);
            let entry = &lab.peers[&address];
            let backend = entry.backend_proxy.unwrap();
            let player_data = entry.player_data.unwrap();
            let bestiary = entry.bestiary.unwrap();
            let components = [Some(backend), Some(player_data), Some(bestiary)];
            // Avoid the default GUID (1), which requires export flags on wire.
            // This authored reference is distinct from all permitted components.
            let unknown = (2..)
                .find(|guid| !components.contains(&Some(*guid)))
                .unwrap();
            let valid_blocks = vec![
                field_block(
                    subobject(player_data),
                    PLAYER_DATA_FIELD_INDEX_BOUND,
                    &[(SERVER_RECEIVE_LOADOUT, empty_payload())],
                ),
                field_block(
                    subobject(bestiary),
                    BESTIARY_MANAGER_FIELD_INDEX_BOUND,
                    &[(SERVER_RECEIVE_BESTIARY_DATA, empty_payload())],
                ),
            ];
            let valid_payload = client_actor_body(&lab, valid_blocks.clone());
            let mut expected = ControllerFieldDiagnostics::default();
            let (calls, ignored) = controller_initialisation_calls(
                &valid_payload,
                lab.player_controller_open
                    .as_ref()
                    .unwrap()
                    .engine_net_version,
                lab.config.packet.codec.order,
                components,
                &mut expected,
            )
            .unwrap();
            assert_eq!(ignored, 0);
            assert!(matches!(
                calls.as_slice(),
                [
                    ControllerCall::Initialisation(InitialisationAnswer::Loadout),
                    ControllerCall::Initialisation(InitialisationAnswer::Bestiary),
                ]
            ));
            let mut blocks = vec![
                // An actor RPC index on a known subobject must not dispatch on the actor.
                field_block(
                    subobject(backend),
                    BACKEND_PROXY_FIELD_INDEX_BOUND,
                    &[
                        (
                            SERVER_FRIENDS_LIST_UPDATED_FIELD_INDEX,
                            friends_payload(ByteOrder::Little, &[]),
                        ),
                        (SERVER_RECEIVE_LOADOUT, empty_payload()),
                    ],
                ),
                // The Bestiary index also exists in other component caches. Matching
                // the field number alone must not recognize it on PlayerData.
                field_block(
                    subobject(player_data),
                    PLAYER_DATA_FIELD_INDEX_BOUND,
                    &[(SERVER_RECEIVE_BESTIARY_DATA, empty_payload())],
                ),
                // Unknown subobjects are skipped before attempting their fields,
                // including an otherwise recognized actor RPC and an invalid header.
                field_block(
                    subobject(unknown),
                    BP_ICARUS_PLAYER_CONTROLLER_SURVIVAL_FIELD_INDEX_BOUND,
                    &[(
                        SERVER_FRIENDS_LIST_UPDATED_FIELD_INDEX,
                        friends_payload(ByteOrder::Little, &[]),
                    )],
                ),
                ContentBlock {
                    has_rep_layout: true,
                    target: subobject(unknown),
                    payload: {
                        let mut writer = BitWriter::new();
                        writer.write_bit(true);
                        Payload::from_writer(writer)
                    },
                },
            ];
            blocks.extend(valid_blocks);
            let payload = client_actor_body(&lab, blocks);
            let raw = wire_reliable_sequence(client.initial_reliable().1 + 1);
            receive_controller_payload(&mut lab, &socket, &mut client, raw, payload);
            let diagnostics = lab.controller_field_diagnostics(address).unwrap();
            assert_eq!(diagnostics.malformed_bunches, 0);
            assert_eq!(diagnostics.overflow, 0);
            assert_eq!(diagnostics.entries.len(), 5);
            assert!(
                diagnostics
                    .entries
                    .iter()
                    .all(|entry| entry.target == ControllerFieldTarget::PermittedSubobject)
            );
            let recognized: Vec<_> = diagnostics
                .entries
                .iter()
                .filter(|entry| entry.disposition == ControllerFieldDisposition::Recognized)
                .cloned()
                .collect();
            assert_eq!(recognized, expected.entries);
            assert_eq!(
                diagnostics
                    .entries
                    .iter()
                    .filter(|entry| entry.disposition == ControllerFieldDisposition::Ignored)
                    .count(),
                3
            );
            assert_eq!(lab.stats.controller_fields_ignored, 3);
            assert_eq!(lab.stats.friends_lists_received, 0);
            assert!(!lab.peers[&address].profiles.friend_ids_ready);
            assert_eq!(lab.stats.initialisation_calls_ignored, 0);
            assert_eq!(lab.stats.loadout_responses_received, 1);
            assert_eq!(lab.stats.bestiary_responses_received, 1);
            assert_eq!(lab.stats.initialisation_answers_ignored, 2);
            assert_eq!(lab.stats.actor_bunches_malformed, 0);
            assert_diagnostics_eq(&lab.controller_field_totals(), diagnostics);
        }

        #[test]
        fn duplicate_reliable_controller_bunch_advances_diagnostics_and_domain_once() {
            let mut lab = prospect_refresh_lab();
            let (socket, address, mut client) = controller_peer(&mut lab);
            let raw = wire_reliable_sequence(client.initial_reliable().1 + 1);
            let payload = actor_fields(false);
            let accepted_before = lab.stats.data_accepted;
            receive_controller_payload(&mut lab, &socket, &mut client, raw, payload.clone());
            let first = lab.controller_field_diagnostics(address).unwrap().clone();
            assert_eq!(first.entries.len(), 2);
            assert!(first.entries.iter().all(|entry| entry.count == 1));
            assert_eq!(
                first.entries[0].disposition,
                ControllerFieldDisposition::Recognized
            );
            assert_eq!(
                first.entries[1].disposition,
                ControllerFieldDisposition::Ignored
            );
            assert_eq!(lab.stats.friends_lists_received, 1);
            assert!(lab.peers[&address].profiles.friend_ids_ready);
            assert_eq!(lab.stats.controller_fields_ignored, 1);
            assert_eq!(lab.stats.initialisation_calls_ignored, 0);
            assert_eq!(lab.stats.actor_duplicate_reliable, 0);
            // Commit a fresh packet header while reusing the reliable sequence.
            // Replaying identical datagram bytes would only test packet rejection.
            receive_controller_payload(&mut lab, &socket, &mut client, raw, payload);
            assert_eq!(lab.stats.data_accepted, accepted_before + 2);
            assert_eq!(lab.stats.actor_duplicate_reliable, 1);
            assert_eq!(lab.stats.friends_lists_received, 1);
            assert_eq!(lab.stats.controller_fields_ignored, 1);
            assert_eq!(lab.stats.initialisation_calls_ignored, 0);
            assert_eq!(lab.stats.actor_bunches_malformed, 0);
            assert_diagnostics_eq(lab.controller_field_diagnostics(address).unwrap(), &first);
            assert_diagnostics_eq(&lab.controller_field_totals(), &first);
        }

        #[test]
        fn peer_removal_and_shutdown_drop_only_live_controller_diagnostics() {
            let mut lab = prospect_refresh_lab();
            let (first_socket, first, mut first_client) = controller_peer(&mut lab);
            let (second_socket, second, mut second_client) = controller_peer(&mut lab);
            for (socket, client, payload) in [
                (&first_socket, &mut first_client, actor_fields(true)),
                (&second_socket, &mut second_client, actor_fields(false)),
            ] {
                let raw = wire_reliable_sequence(client.initial_reliable().1 + 1);
                receive_controller_payload(&mut lab, socket, client, raw, payload);
            }
            let first_diagnostics = lab.controller_field_diagnostics(first).unwrap();
            assert_eq!(first_diagnostics.entries.len(), 2);
            assert_eq!(first_diagnostics.malformed_bunches, 1);
            assert!(
                !lab.peers[&first].profiles.friend_ids_ready,
                "malformed trailing input must not apply the preceding recognized call"
            );
            assert_eq!(lab.stats.friends_lists_received, 1);
            assert_eq!(lab.stats.controller_fields_ignored, 1);
            let second_diagnostics = lab.controller_field_diagnostics(second).unwrap().clone();
            assert_eq!(second_diagnostics.entries.len(), 2);
            assert!(
                second_diagnostics
                    .entries
                    .iter()
                    .all(|entry| entry.count == 1)
            );
            let totals = lab.controller_field_totals();
            assert_eq!(totals.entries.len(), 2);
            assert!(totals.entries.iter().all(|entry| entry.count == 2));
            assert_eq!(totals.malformed_bunches, 1);
            assert_eq!(totals.overflow, 0);
            lab.remove_peer(first).unwrap();
            assert!(lab.controller_field_diagnostics(first).is_none());
            assert_diagnostics_eq(
                lab.controller_field_diagnostics(second).unwrap(),
                &second_diagnostics,
            );
            assert_diagnostics_eq(&lab.controller_field_totals(), &second_diagnostics);
            assert_eq!(lab.peer_count(), 1);
            assert_eq!(lab.shutdown(), 1);
            assert!(lab.controller_field_diagnostics(first).is_none());
            assert!(lab.controller_field_diagnostics(second).is_none());
            assert_diagnostics_eq(
                &lab.controller_field_totals(),
                &ControllerFieldDiagnostics::default(),
            );
            assert_eq!(lab.peer_count(), 0);
            assert_eq!(lab.shutdown(), 0);
        }

        #[test]
        fn aggregate_controller_diagnostics_are_bounded_saturating_metadata() {
            let mut lab = prospect_refresh_lab();
            let (_first_socket, first, _) = acked_game_state_peer(&mut lab);
            let (_second_socket, second, _) = acked_game_state_peer(&mut lab);
            for (address, start) in [(first, 0), (second, 64)] {
                let diagnostics = &mut lab
                    .peers
                    .get_mut(&address)
                    .unwrap()
                    .controller_field_diagnostics;
                for index in start..start + 65 {
                    diagnostics.record_count(
                        ControllerFieldTarget::Actor,
                        index,
                        8,
                        ControllerFieldDisposition::Ignored,
                        u64::MAX,
                    );
                }
                diagnostics.record(
                    ControllerFieldTarget::Actor,
                    start + 65,
                    8,
                    ControllerFieldDisposition::Ignored,
                );
                diagnostics.malformed_bunches = u64::MAX;
                diagnostics.body_parse_failures = u64::MAX;
                diagnostics.block_payload_limit_rejections = u64::MAX;
                diagnostics.block_payload_length_rejections = u64::MAX;
                diagnostics.max_rejected_block_payload_bits = 65_537 + u64::from(start);
                diagnostics.unknown_subobject_blocks = u64::MAX;
                assert_eq!(diagnostics.entries.len(), 64);
                assert_eq!(diagnostics.overflow, u64::MAX);
            }
            let totals = lab.controller_field_totals();
            assert_eq!(totals.entries.len(), 64);
            assert!(totals.entries.iter().all(|entry| entry.count == u64::MAX));
            assert_eq!(totals.overflow, u64::MAX);
            assert_eq!(totals.malformed_bunches, u64::MAX);
            assert_eq!(totals.body_parse_failures, u64::MAX);
            assert_eq!(totals.block_payload_limit_rejections, u64::MAX);
            assert_eq!(totals.block_payload_length_rejections, u64::MAX);
            assert_eq!(totals.max_rejected_block_payload_bits, 65_601);
            assert_eq!(totals.unknown_subobject_blocks, u64::MAX);
            let value = serde_json::to_value(&totals).unwrap();
            let mut keys: Vec<_> = value
                .as_object()
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect();
            keys.sort_unstable();
            assert_eq!(
                keys,
                [
                    "block_payload_length_rejections",
                    "block_payload_limit_rejections",
                    "body_parse_failures",
                    "entries",
                    "malformed_bunches",
                    "max_rejected_block_payload_bits",
                    "overflow",
                    "unknown_subobject_blocks"
                ]
            );
            for entry in value["entries"].as_array().unwrap() {
                let mut keys: Vec<_> = entry
                    .as_object()
                    .unwrap()
                    .keys()
                    .map(String::as_str)
                    .collect();
                keys.sort_unstable();
                assert_eq!(
                    keys,
                    ["count", "disposition", "index", "payload_bits", "target"]
                );
            }
            // With neither peer's own overflow involved, 128 distinct rows must
            // still be bounded to 64 aggregate rows and charge the discarded counts.
            for address in [first, second] {
                let diagnostics = &mut lab
                    .peers
                    .get_mut(&address)
                    .unwrap()
                    .controller_field_diagnostics;
                diagnostics.overflow = 0;
                diagnostics.malformed_bunches = 0;
                for entry in &mut diagnostics.entries {
                    entry.count = 1;
                }
            }
            let totals = lab.controller_field_totals();
            assert_eq!(totals.entries.len(), 64);
            assert_eq!(totals.overflow, 64);
            assert_eq!(totals.malformed_bunches, 0);
            // Identical rows from distinct peers saturate their aggregate count.
            let shared = lab.peers[&first].controller_field_diagnostics.entries[0].clone();
            for address in [first, second] {
                let diagnostics = &mut lab
                    .peers
                    .get_mut(&address)
                    .unwrap()
                    .controller_field_diagnostics;
                diagnostics.entries = vec![shared.clone()];
                diagnostics.entries[0].count = u64::MAX;
            }
            let totals = lab.controller_field_totals();
            assert_eq!(totals.entries.len(), 1);
            assert_eq!(totals.entries[0].count, u64::MAX);
            assert_eq!(totals.overflow, 0);
        }

        fn actor_fields(trailing: bool) -> Payload {
            let mut writer = BitWriter::new();
            for index in [SERVER_FRIENDS_LIST_UPDATED_FIELD_INDEX, 0] {
                write_field_header(
                    &mut writer,
                    &FieldHeader {
                        index,
                        payload: if index == SERVER_FRIENDS_LIST_UPDATED_FIELD_INDEX {
                            friends_payload(ByteOrder::Little, &[])
                        } else {
                            empty_payload()
                        },
                    },
                    BP_ICARUS_PLAYER_CONTROLLER_SURVIVAL_FIELD_INDEX_BOUND,
                )
                .unwrap();
            }
            let fields = Payload::from_writer(writer);
            let fields = if trailing {
                // An incomplete header after two complete observations.
                let mut writer = BitWriter::new();
                let mut reader = fields.reader();
                while !reader.at_end() {
                    writer.write_bit(reader.read_bit());
                }
                writer.write_bit(true);
                Payload::from_writer(writer)
            } else {
                fields
            };
            encode_actor_bunch_body(
                &ActorBunchBody {
                    spawn: None,
                    blocks: vec![ContentBlock {
                        has_rep_layout: false,
                        target: BlockTarget::Actor,
                        payload: fields,
                    }],
                },
                &actor_codec(0, ByteOrder::Little),
            )
            .unwrap()
        }

        #[test]
        fn mixed_fields_are_observations_and_trailing_failure_returns_no_calls() {
            let mut diagnostics = ControllerFieldDiagnostics::default();
            let result = controller_initialisation_calls(
                &actor_fields(false),
                0,
                ByteOrder::Little,
                [None; 3],
                &mut diagnostics,
            )
            .unwrap();
            assert_eq!(result.0.len(), 1);
            assert_eq!(result.1, 1);
            assert_eq!(diagnostics.entries.len(), 2);
            assert_eq!(
                diagnostics.entries[0].disposition,
                ControllerFieldDisposition::Recognized
            );
            assert_eq!(
                diagnostics.entries[1].disposition,
                ControllerFieldDisposition::Ignored
            );
            let mut diagnostics = ControllerFieldDiagnostics::default();
            assert!(
                controller_initialisation_calls(
                    &actor_fields(true),
                    0,
                    ByteOrder::Little,
                    [None; 3],
                    &mut diagnostics
                )
                .is_err()
            );
            assert_eq!(diagnostics.entries.len(), 2);
        }

        #[test]
        fn malformed_recognized_payload_records_disposition_without_applying_calls() {
            let mut lab = prospect_refresh_lab();
            let (socket, address, mut client) = controller_peer(&mut lab);
            let payload = client_actor_body(
                &lab,
                vec![field_block(
                    BlockTarget::Actor,
                    BP_ICARUS_PLAYER_CONTROLLER_SURVIVAL_FIELD_INDEX_BOUND,
                    &[
                        (
                            SERVER_FRIENDS_LIST_UPDATED_FIELD_INDEX,
                            friends_payload(ByteOrder::Little, &[]),
                        ),
                        (SERVER_BEGIN_PLAYER_INITIALISATION_FIELD_INDEX, {
                            // Present ChrSlot, missing its required Int payload.
                            let mut writer = BitWriter::new();
                            writer.write_bit(true);
                            Payload::from_writer(writer)
                        }),
                    ],
                )],
            );
            let raw = wire_reliable_sequence(client.initial_reliable().1 + 1);
            receive_controller_payload(&mut lab, &socket, &mut client, raw, payload);

            let diagnostics = lab.controller_field_diagnostics(address).unwrap();
            assert_eq!(diagnostics.entries.len(), 2);
            assert_eq!(diagnostics.malformed_bunches, 1);
            assert_eq!(diagnostics.overflow, 0);
            assert_eq!(
                diagnostics.entries[0].disposition,
                ControllerFieldDisposition::Recognized
            );
            let malformed = &diagnostics.entries[1];
            assert_eq!(malformed.target, ControllerFieldTarget::Actor);
            assert_eq!(
                malformed.index,
                SERVER_BEGIN_PLAYER_INITIALISATION_FIELD_INDEX
            );
            assert_eq!(malformed.payload_bits, 1);
            assert_eq!(malformed.disposition, ControllerFieldDisposition::Malformed);
            assert_eq!(malformed.count, 1);
            assert_eq!(lab.stats.actor_bunches_malformed, 1);
            assert_eq!(lab.stats.friends_lists_received, 0);
            assert_eq!(lab.stats.controller_fields_ignored, 0);
            assert!(!lab.peers[&address].profiles.friend_ids_ready);
            assert_eq!(lab.character_slot(address), None);
            assert_diagnostics_eq(&lab.controller_field_totals(), diagnostics);
        }

        #[test]
        fn observations_are_bounded_and_saturate_without_cross_target_coalescing() {
            let mut diagnostics = ControllerFieldDiagnostics::default();
            for index in 0..1000 {
                diagnostics.record(
                    ControllerFieldTarget::Actor,
                    index,
                    8,
                    ControllerFieldDisposition::Ignored,
                );
            }
            assert_eq!(diagnostics.entries.len(), 64);
            assert_eq!(diagnostics.overflow, 936);
            diagnostics.entries[0].count = u64::MAX;
            diagnostics.record(
                ControllerFieldTarget::Actor,
                0,
                8,
                ControllerFieldDisposition::Ignored,
            );
            assert_eq!(diagnostics.entries[0].count, u64::MAX);
            diagnostics.record(
                ControllerFieldTarget::PermittedSubobject,
                0,
                8,
                ControllerFieldDisposition::Ignored,
            );
            assert_eq!(diagnostics.overflow, 937);
            assert_eq!(diagnostics.entries[0].target, ControllerFieldTarget::Actor);
        }

        /// Authored client-to-server unreliable bunches on every channel kind:
        /// each still drops exactly as before (counted in
        /// `actor_bunches_dropped`, no dispatch or field diagnostics), and the
        /// header-only breakdown counts it by channel kind, controller field
        /// index and length; the field table caps at its fixed size with an
        /// overflow count, and counts saturate.
        #[test]
        fn unreliable_client_bunches_are_counted_by_channel_and_field_and_stay_dropped() {
            let mut lab = record_lab(false);
            let (socket, address, mut client) = record_peer(&mut lab, 1, "AuthoredUnreliable");
            let entry = &lab.peers[&address];
            let controller = entry.player_controller_channel.unwrap();
            let player_state = entry.player_state_channel.unwrap();
            let pawn = super::super::super::pawn_open::PAWN_CHANNEL_INDEX;
            let game_state = game_state_channel(&lab);
            assert!(entry.peer.has_actor_channel(pawn));
            assert!(entry.peer.has_actor_channel(game_state));
            let unreliable = |channel: u32, partial: bool, payload: Payload| Bunch {
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
                    partial,
                    reliable_sequence_raw: None,
                    partial_initial: partial,
                    partial_final: false,
                    channel_name: None,
                },
                payload,
            };
            let bits = |count: usize| {
                let mut writer = BitWriter::new();
                for bit in 0..count {
                    writer.write_bit(bit % 3 == 0);
                }
                Payload::from_writer(writer)
            };
            let fields = |lab: &PeerLab, fields: &[(u32, Payload)]| {
                client_actor_body(
                    lab,
                    vec![field_block(
                        BlockTarget::Actor,
                        BP_ICARUS_PLAYER_CONTROLLER_SURVIVAL_FIELD_INDEX_BOUND,
                        fields,
                    )],
                )
            };
            let mut send = |lab: &mut PeerLab, bunches: Vec<Bunch>| {
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
                receive_authored_at(lab, &socket, &bytes, Instant::now());
            };
            let camera = fields(&lab, &[(81, bits(40))]);
            let two = fields(&lab, &[(81, bits(24)), (78, bits(0))]);
            let malformed = Payload::from_writer({
                let mut writer = BitWriter::new();
                writer.write_bit(true);
                writer
            });
            // Off by default: the bunch is dropped as before and not counted.
            let dropped = lab.stats.actor_bunches_dropped;
            send(
                &mut lab,
                vec![unreliable(controller, false, camera.clone())],
            );
            assert_eq!(lab.stats.actor_bunches_dropped, dropped + 1);
            assert_eq!(
                lab.unreliable_actor_bunch_counts(),
                &UnreliableActorBunchCounts::default()
            );
            lab.enable_unreliable_actor_bunch_counts();
            send(
                &mut lab,
                vec![
                    unreliable(controller, false, camera.clone()),
                    unreliable(controller, false, two.clone()),
                    unreliable(pawn, false, bits(70)),
                ],
            );
            send(
                &mut lab,
                vec![
                    unreliable(player_state, false, bits(9)),
                    unreliable(game_state, false, bits(10)),
                    unreliable(controller, true, bits(12)),
                    unreliable(controller, false, malformed),
                ],
            );
            let counts = lab.unreliable_actor_bunch_counts().clone();
            assert_eq!(
                counts.controller,
                UnreliableChannelCounts {
                    bunches: 4,
                    last_payload_bits: 1,
                }
            );
            assert_eq!(
                counts.pawn,
                UnreliableChannelCounts {
                    bunches: 1,
                    last_payload_bits: 70,
                }
            );
            assert_eq!(counts.player_state.bunches, 1);
            assert_eq!(counts.player_state.last_payload_bits, 9);
            assert_eq!(counts.game_state.bunches, 1);
            assert_eq!(counts.game_state.last_payload_bits, 10);
            assert_eq!(counts.other, UnreliableChannelCounts::default());
            assert_eq!(
                counts.controller_fields,
                [
                    UnreliableFieldCounts {
                        index: 81,
                        count: 2,
                        last_payload_bits: 24,
                    },
                    UnreliableFieldCounts {
                        index: 78,
                        count: 1,
                        last_payload_bits: 0,
                    },
                ]
            );
            assert_eq!(counts.controller_unparsed, 1);
            assert_eq!(counts.controller_malformed, 1);
            assert_eq!(counts.controller_field_overflow, 0);
            assert_eq!(counts.controller_other_blocks, 0);
            // Still dropped exactly as before: nothing dispatched or diagnosed.
            // Seven counted bunches plus the one sent before counting began.
            assert_eq!(lab.stats.actor_bunches_dropped, dropped + 8);
            assert_eq!(lab.stats.controller_fields_ignored, 0);
            assert_eq!(lab.stats.actor_bunches_malformed, 0);
            let diagnostics = lab.controller_field_diagnostics(address).unwrap();
            assert!(diagnostics.entries.is_empty());
            assert_eq!(diagnostics.malformed_bunches, 0);
            // A replicated actor the lab opens on a free channel through its
            // public `open_actor` is "other"; a sub-object block is counted,
            // not read.
            let backend = lab.peers[&address].backend_proxy.unwrap();
            let component = client_actor_body(
                &lab,
                vec![field_block(
                    subobject(backend),
                    BACKEND_PROXY_FIELD_INDEX_BOUND,
                    &[(1, bits(3))],
                )],
            );
            let replicated = lab
                .open_actor(
                    1,
                    0x5eed,
                    super::super::super::session_open::ActorOpenSpec {
                        actor: 0,
                        archetype: vec![
                            super::super::super::session_open::PathSegment::new(
                                "Default__AuthoredCrate_C",
                                0x0bad_cafe,
                                false,
                            ),
                            super::super::super::session_open::PathSegment::new(
                                "/Game/Authored/AuthoredCrate",
                                0,
                                false,
                            ),
                        ],
                        level: super::super::super::session_open::map_level_chain(
                            PeerLabAdmission::default().map_url,
                        )
                        .unwrap(),
                        location: Some([0.0; 3]),
                        rotation: None,
                        velocity: None,
                        blocks: Vec::new(),
                    },
                    game_state + 1,
                    17,
                )
                .unwrap();
            assert!(![controller, pawn, player_state, game_state].contains(&replicated));
            send(
                &mut lab,
                vec![
                    unreliable(replicated, false, bits(5)),
                    unreliable(controller, false, component),
                ],
            );
            let counts = lab.unreliable_actor_bunch_counts().clone();
            assert_eq!(
                counts.other,
                UnreliableChannelCounts {
                    bunches: 1,
                    last_payload_bits: 5,
                }
            );
            assert_eq!(counts.pawn.bunches, 1);
            assert_eq!(counts.controller_other_blocks, 1);
            // The field table keeps 16 distinct indices; the rest overflow.
            for index in 100..(100 + UNRELIABLE_CONTROLLER_FIELD_ENTRIES as u32) {
                let payload = fields(&lab, &[(index, bits(1))]);
                send(&mut lab, vec![unreliable(controller, false, payload)]);
            }
            let counts = lab.unreliable_actor_bunch_counts().clone();
            assert_eq!(
                counts.controller_fields.len(),
                UNRELIABLE_CONTROLLER_FIELD_ENTRIES
            );
            assert_eq!(counts.controller_field_overflow, 2);
            assert!(
                counts
                    .controller_fields
                    .iter()
                    .all(|field| field.index != 114)
            );
            // Saturation.
            lab.unreliable_actor_bunches.controller.bunches = u64::MAX;
            lab.unreliable_actor_bunches.controller_fields[0].count = u64::MAX;
            lab.unreliable_actor_bunches.controller_field_overflow = u64::MAX;
            let unseen = fields(&lab, &[(150, bits(1))]);
            send(
                &mut lab,
                vec![
                    unreliable(controller, false, camera),
                    unreliable(controller, false, two),
                    unreliable(controller, false, unseen),
                ],
            );
            let counts = lab.unreliable_actor_bunch_counts();
            assert_eq!(counts.controller.bunches, u64::MAX);
            assert_eq!(counts.controller_fields[0].count, u64::MAX);
            assert_eq!(counts.controller_field_overflow, u64::MAX);
            let text = serde_json::to_string(counts).unwrap();
            assert!(!text.contains("AuthoredUnreliable"));
        }
    }
    mod friends_query_deadline;
    mod level_visibility;
    mod move_acks;
    mod pawn_capture;
    mod record_ack_gating;
    mod spawn_placement;
}

#[cfg(test)]
mod churn_tests;

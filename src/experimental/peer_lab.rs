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
//! (`IsClientMsgTypeValid`), `PreLogin`, `Welcome`, `Netspeed`, and `Join` /
//! `SplitJoin` (`SpawnPlayActor` / `InitNewPlayer`) checks from
//! `experimental::admission` (`F-712`, `F-713`, `F-740`).
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
//!   or sending Failure (ID 6) and closing the peer.
//! - Messages after a closing reply in the same delivery are not processed.
//!
//! Removing a peer is the lab's close: no close bunch is written, so a lost
//! Upgrade or Failure is never resent. Naked reliable replies of live peers are
//! resent only inside later ack or reply datagrams (see `packet_lab`), which
//! the lab writes only after accepting a client packet; there is no timer.
//!
//! Policy (lab choices):
//! - At most [`PeerLabConfig::max_peers`] peers. When the table is full, a
//!   valid challenge echo from a new address is rejected: no acknowledgement is
//!   sent and `peers_rejected` counts it. Nothing is ever evicted to make room;
//!   a known address that completes a new handshake replaces its own peer; a
//!   repeated echo of its current cookie is only re-acknowledged.
//! - Peers idle longer than [`PeerLabConfig::idle_timeout`] (no accepted data
//!   packet since creation or the last accepted one) are removed at the start of
//!   an update. Failed peers ([`LabPeer::is_failed`]) and peers whose ack packet
//!   cannot be built are removed at once.
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

use anyhow::{Context, Result, ensure};
use bevy::prelude::{App, Plugin, ResMut, Resource, Update};
use serde::Serialize;

use super::admission::{
    AdmissionPolicy, ClientLoginState, ConnectionLoginAdmission, DEFAULT_MAX_CLIENT_RATE,
    JoinSpawnContext, Occupancy, clamp_client_netspeed, int_option,
};
use super::control_messages::{ArchiveText, ControlMessage, Decoded, NetIdValue};
use super::handshake::decode;
use super::handshake_lab::{
    CookieAuthority, HANDSHAKE_BYTES, HandshakeReply, PACKETS_PER_UPDATE, bind_loopback,
    lab_seconds,
};
use super::packet::BunchLimits;
use super::packet_lab::{DeliveredSummary, LabPeer, PacketDisposition, PacketLabConfig};
use super::release::ReleaseProfile;

/// Lab text of the Failure reply to Login; not an original message.
pub const ADMISSION_DISABLED_MESSAGE: &str = "icarus-bevy peer lab: game admission is disabled";
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
    /// Control bunches carrying the close flag; the lab does not act on them.
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
    /// Peers removed after an Upgrade or Failure reply.
    pub peers_closed: u64,
    /// Replies that could not be built (the peer is kept unless it failed).
    pub reply_errors: u64,
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

fn bump(counter: &mut u64) {
    *counter = counter.saturating_add(1);
}

/// A control reply chosen for one delivered message.
enum Reply {
    Challenge,
    Upgrade(u32),
    Welcome { spectator: bool },
    Failure(&'static str),
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
}

impl fmt::Debug for PeerLab {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PeerLab")
            .field("peers", &self.peers.len())
            .field("max_peers", &self.config.max_peers)
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
        Ok(Self {
            socket,
            authority,
            started: Instant::now(),
            peers: HashMap::with_capacity(config.max_peers),
            // One excess byte detects datagrams longer than the maximum.
            buffer: vec![0; config.max_datagram_bytes() + 1],
            config,
            stats: PeerLabStats::default(),
        })
    }

    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.socket.local_addr()
    }

    pub fn config(&self) -> &PeerLabConfig {
        &self.config
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
                    },
                );
            }
        }
        true
    }

    fn data(&mut self, address: SocketAddrV4, length: usize, now: Instant) {
        let Some(entry) = self.peers.get_mut(&address) else {
            bump(&mut self.stats.unknown_peer);
            return;
        };
        let messages = match entry.peer.receive(&self.buffer[..length]) {
            Ok(received) => {
                let summary = received.summary();
                match summary.disposition {
                    PacketDisposition::Accepted { .. } => {
                        entry.last_seen = now;
                        entry.ack_pending = true;
                        bump(&mut self.stats.data_accepted);
                        if summary.control_close_seen {
                            bump(&mut self.stats.control_close_seen);
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
    }

    /// Apply the reply policy (module docs) to delivered messages in order,
    /// stopping after a reply that closes the peer.
    fn answer(&mut self, address: SocketAddrV4, messages: &[Decoded]) {
        for decoded in messages {
            let Decoded::Message(message) = decoded else {
                continue;
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
                    player_id: 1,
                    net_player_index: 0,
                };
                match admission
                    .policy
                    .login_and_init_new_player(&options, occupancy, ctx)
                {
                    Ok(_) => {
                        bump(&mut self.stats.joins_succeeded);
                        None
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
                    player_id: 1 + i32::from(next_split),
                    net_player_index: next_split,
                };
                match admission
                    .policy
                    .login_and_init_new_player(url.units(), occupancy, ctx)
                {
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
            _ => None,
        }
    }

    /// Build and send one reply; a closing reply then removes the peer.
    /// Returns `false` when the peer is gone or the reply could not be built.
    fn send_reply(&mut self, address: SocketAddrV4, reply: &Reply) -> bool {
        let message = match reply {
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
        }
        if self.send(&datagram, address) {
            bump(match reply {
                Reply::Challenge => &mut self.stats.control_challenges_sent,
                Reply::Upgrade(_) => &mut self.stats.upgrades_sent,
                Reply::Welcome { .. } => &mut self.stats.welcomes_sent,
                Reply::Failure(_) => &mut self.stats.failures_sent,
            });
        }
        true
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
    /// [`PACKETS_PER_UPDATE`] datagrams, then send pending acks.
    fn poll(&mut self) {
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
    }
}

/// Insert a bound [`PeerLab`] before adding this optional research plugin.
pub struct PeerLabPlugin;

impl Plugin for PeerLabPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, poll_peer_lab);
    }
}

fn poll_peer_lab(mut lab: ResMut<PeerLab>) {
    lab.poll();
}

#[cfg(test)]
mod tests {
    use super::*;

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

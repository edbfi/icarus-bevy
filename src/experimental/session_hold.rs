//! EXPERIMENTAL session hold: one authored-probe session joined to an already
//! running `icarus-server run` on IPv4 loopback and kept for a bounded wall
//! time, acknowledging every packet and checking the GameState updates it
//! receives (world time, `TimeOfDay`, `LevelTimeElapsedSec`, `ServerFPS`;
//! F-807, F-809).
//!
//! The probe and the server share this repository's model, so a clean hold
//! shows the native session layer staying consistent over time, not
//! compatibility with any client. The summary carries counts, timings and
//! the server's own clock and frame-rate ranges only.
use std::{
    net::{SocketAddr, UdpSocket},
    time::{Duration, Instant},
};

use anyhow::{Context, Result, ensure};
use serde::Serialize;

use super::{
    actor_channel::{ActorChannelCodec, ActorChannelLimits, BlockTarget, read_actor_bunch_body},
    archive::ByteOrder,
    control_messages::{
        ArchiveText, ControlCodec, ControlMessage, NetIdType, NetIdValue, UniqueNetId,
    },
    game_state_open::{GAME_STATE_CHANNEL_INDEX, handle},
    packet::{PacketConfig, Receiver, read_packet},
    peer_lab::PeerLabConfig,
    probe_client::{ProbeClient, ProbeConfig, ReplyDecode},
    rep_layout::{
        MovementLevel, RepLayoutCodec, RepLayoutLimits, RepValue, RotationLevel,
        icarus_game_state_survival_rep_layout, read_rep_properties,
    },
};

/// Longest accepted [`HoldConfig::duration`]: 72 hours.
pub const MAX_HOLD: Duration = Duration::from_secs(72 * 3600);
/// Synthetic Login identity; never logged by the server.
const SYNTHETIC_NAME: &str = "HoldProbe";
const SYNTHETIC_NET_ID_HEX: &str = "00000000000000000002";
/// How long each receive window lasts before the probe acknowledges.
const WINDOW: Duration = Duration::from_millis(250);
/// A `TimeOfDay` drop larger than half a day counts as a new day.
const DAY_WRAP_MINUTES: f32 = 720.0;

/// Hold parameters.
#[derive(Clone, Debug)]
pub struct HoldConfig {
    /// IPv4 loopback address of the running server.
    pub server: SocketAddr,
    /// Hello network version (the pinned release's checksum when the server
    /// checks versions; any value otherwise).
    pub network_version: u32,
    /// The release's engine network version (GameState block decoding).
    pub engine_net_version: u32,
    /// How long the joined session is held, `1 s..=MAX_HOLD`.
    pub duration: Duration,
    /// Bound on each admission receive.
    pub step_timeout: Duration,
}

/// One JSON-serializable hold result.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct HoldSummary {
    /// The server opened an actor channel after Join (its PlayerController).
    pub joined: bool,
    /// The server answered with a Failure control message (the join or
    /// session refused); the hold ends there.
    pub refused: bool,
    pub held_ms: u64,
    pub datagrams: u64,
    pub keepalives: u64,
    pub game_state_updates: u64,
    /// GameState update blocks that did not decode with the layout.
    pub decode_errors: u64,
    /// Longest wait between two GameState updates by datagram arrival,
    /// including the wait from the last update to the end of the hold.
    pub max_update_gap_ms: u64,
    /// The socket error that ended the hold early, if any (kind only); the
    /// summary covers what arrived before it.
    pub ended_by_error: Option<String>,
    pub world_time_updates: u64,
    /// World times that did not increase.
    pub world_time_regressions: u64,
    pub last_world_time: Option<f32>,
    pub time_of_day_updates: u64,
    /// `TimeOfDay` drops other than a new day.
    pub time_of_day_regressions: u64,
    pub day_wraps: u64,
    pub level_time_updates: u64,
    /// `LevelTimeElapsedSec` values that did not increase.
    pub level_time_regressions: u64,
    pub server_fps_updates: u64,
    pub server_fps_min: Option<i32>,
    pub server_fps_max: Option<i32>,
}

/// Join one probe session and hold it for `config.duration`.
pub fn run_hold(config: &HoldConfig) -> Result<HoldSummary> {
    ensure!(
        config.server.ip().is_loopback(),
        "the hold probe targets loopback only"
    );
    ensure!(
        (Duration::from_secs(1)..=MAX_HOLD).contains(&config.duration),
        "a hold lasts 1 s to 72 h"
    );
    // A zero socket timeout is rejected by the OS deep in the handshake.
    ensure!(
        !config.step_timeout.is_zero(),
        "admission receives need a positive bound"
    );
    let socket = UdpSocket::bind("127.0.0.1:0").context("bind the hold probe socket")?;
    socket.connect(config.server)?;
    let to_client = PacketConfig {
        magic_bits: 0,
        receiver: Receiver::Client,
        limits: PeerLabConfig::default().packet.packet.limits,
    };
    let mut client = ProbeClient::connect(
        socket,
        ProbeConfig {
            packet: PacketConfig {
                magic_bits: 0,
                receiver: Receiver::Server,
                limits: to_client.limits,
            },
            codec: ControlCodec::default(),
            handshake_timeout: config.step_timeout,
            handshake_attempts: 3,
            reply_window: config.step_timeout,
            max_datagrams: 1,
        },
    )?;
    client.hello(config.network_version, "")?;
    client.send_control(
        &[ControlMessage::Login {
            response: ArchiveText::text("0")?,
            url: ArchiveText::text(&format!("?Name={SYNTHETIC_NAME}"))?,
            unique_net_id: UniqueNetId {
                kind: NetIdType::Named(ArchiveText::text("STEAM")?),
                value: NetIdValue::from_hex(SYNTHETIC_NET_ID_HEX)?,
            },
            platform: ArchiveText::text("")?,
        }],
        false,
    )?;
    let welcome = client.collect(config.step_timeout, 1)?;
    ensure!(
        welcome
            .messages()
            .any(|message| matches!(message, ControlMessage::Welcome { .. })),
        "no Welcome"
    );
    client.send_control(
        &[
            ControlMessage::Netspeed { rate: 100_000 },
            ControlMessage::Join,
        ],
        false,
    )?;
    let mut summary = HoldSummary::default();
    let mut tracker = Tracker::new(config.engine_net_version);
    let started = Instant::now();
    // The first gap runs from the start, so a hold without updates reports
    // its whole length.
    tracker.last_update = Some(started);
    while started.elapsed() < config.duration {
        // A socket error (the server gone, say) ends the hold but keeps
        // what was gathered.
        let collected = match client.collect(WINDOW, 4_096) {
            Ok(collected) => collected,
            Err(error) => {
                summary.ended_by_error = Some(error_kind(&error));
                break;
            }
        };
        for reply in &collected.replies {
            summary.datagrams += 1;
            // Only packets the probe accepted count: stale or duplicate
            // sequences (and undecodable datagrams) are skipped.
            let ReplyDecode::Decoded { messages, .. } = &reply.decode else {
                continue;
            };
            if messages
                .iter()
                .any(|message| matches!(message, ControlMessage::Failure { .. }))
            {
                summary.refused = true;
            }
            let Ok(packet) = read_packet(&reply.bytes, &to_client) else {
                continue;
            };
            for bunch in packet.bunches {
                if bunch.header.open && bunch.header.channel_index != 0 {
                    summary.joined = true;
                }
                if bunch.header.channel_index == GAME_STATE_CHANNEL_INDEX
                    && !bunch.header.open
                    && !bunch.header.reliable
                {
                    tracker.update(&bunch.payload, reply.received_at, &mut summary);
                }
            }
        }
        if summary.refused {
            break;
        }
        if let Err(error) = client.keepalive() {
            summary.ended_by_error = Some(error_kind(&error));
            break;
        }
        summary.keepalives += 1;
    }
    let ended = Instant::now();
    tracker.gap_to(ended, &mut summary);
    summary.held_ms = u64::try_from(ended.duration_since(started).as_millis()).unwrap_or(u64::MAX);
    if summary.ended_by_error.is_none() && !summary.refused {
        client.close()?;
    }
    Ok(summary)
}

/// The I/O error kind behind `error`, or a fixed label; never addresses or
/// payload bytes.
fn error_kind(error: &anyhow::Error) -> String {
    error
        .chain()
        .find_map(|cause| cause.downcast_ref::<std::io::Error>())
        .map_or_else(|| "probe error".to_owned(), |io| format!("{:?}", io.kind()))
}

/// Checks each GameState update against the previous values.
struct Tracker {
    actor: ActorChannelCodec,
    layout: Vec<super::rep_layout::RepCommand>,
    last_update: Option<Instant>,
    world_time: Option<f32>,
    time_of_day: Option<f32>,
    level_time: Option<i32>,
}

impl Tracker {
    fn new(engine_net_version: u32) -> Self {
        Self {
            actor: ActorChannelCodec {
                engine_net_version,
                exports_enabled: false,
                has_level: true,
                order: ByteOrder::Little,
                limits: ActorChannelLimits::default(),
            },
            layout: icarus_game_state_survival_rep_layout(
                MovementLevel::Whole,
                MovementLevel::Whole,
                RotationLevel::Bytes,
            ),
            last_update: None,
            world_time: None,
            time_of_day: None,
            level_time: None,
        }
    }

    /// Widen `max_update_gap_ms` with the wait from the last update to `at`.
    fn gap_to(&mut self, at: Instant, summary: &mut HoldSummary) {
        if let Some(last) = self.last_update {
            let gap =
                u64::try_from(at.saturating_duration_since(last).as_millis()).unwrap_or(u64::MAX);
            summary.max_update_gap_ms = summary.max_update_gap_ms.max(gap);
        }
    }

    fn update(
        &mut self,
        payload: &super::packet::Payload,
        received_at: Instant,
        summary: &mut HoldSummary,
    ) {
        let rep = RepLayoutCodec {
            actor: self.actor,
            checksums: false,
            limits: RepLayoutLimits::default(),
        };
        let entries = read_actor_bunch_body(&mut payload.reader(), false, &self.actor)
            .ok()
            .and_then(|body| {
                let block = body
                    .blocks
                    .into_iter()
                    .find(|block| matches!(block.target, BlockTarget::Actor))?;
                read_rep_properties(&mut block.payload.reader(), &self.layout, &rep).ok()
            });
        let Some(stream) = entries else {
            summary.decode_errors += 1;
            return;
        };
        summary.game_state_updates += 1;
        self.gap_to(received_at, summary);
        self.last_update = Some(received_at);
        for entry in stream.entries {
            match (entry.handle, entry.value) {
                (handle::WORLD_TIME_SECONDS, RepValue::Float(bits)) => {
                    let value = f32::from_bits(bits);
                    summary.world_time_updates += 1;
                    if self.world_time.is_some_and(|last| value <= last) {
                        summary.world_time_regressions += 1;
                    }
                    self.world_time = Some(value);
                    summary.last_world_time = Some(value);
                }
                (handle::TIME_OF_DAY, RepValue::Float(bits)) => {
                    let value = f32::from_bits(bits);
                    summary.time_of_day_updates += 1;
                    if let Some(last) = self.time_of_day
                        && value < last
                    {
                        if last - value > DAY_WRAP_MINUTES {
                            summary.day_wraps += 1;
                        } else {
                            summary.time_of_day_regressions += 1;
                        }
                    }
                    self.time_of_day = Some(value);
                }
                (handle::LEVEL_TIME_ELAPSED_SEC, RepValue::Int(value)) => {
                    summary.level_time_updates += 1;
                    if self.level_time.is_some_and(|last| value <= last) {
                        summary.level_time_regressions += 1;
                    }
                    self.level_time = Some(value);
                }
                (handle::SERVER_FPS, RepValue::Int(value)) => {
                    summary.server_fps_updates += 1;
                    summary.server_fps_min =
                        Some(summary.server_fps_min.map_or(value, |min| min.min(value)));
                    summary.server_fps_max =
                        Some(summary.server_fps_max.map_or(value, |max| max.max(value)));
                }
                _ => {}
            }
        }
    }
}

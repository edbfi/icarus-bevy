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
    probe_client::{ProbeClient, ProbeConfig},
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
    pub joined: bool,
    pub held_ms: u64,
    pub datagrams: u64,
    pub keepalives: u64,
    pub game_state_updates: u64,
    /// GameState update blocks that did not decode with the layout.
    pub decode_errors: u64,
    /// Longest wait between two GameState updates.
    pub max_update_gap_ms: u64,
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
    let mut summary = HoldSummary {
        joined: true,
        ..HoldSummary::default()
    };
    let mut tracker = Tracker::new(config.engine_net_version);
    let started = Instant::now();
    while started.elapsed() < config.duration {
        let collected = client.collect(WINDOW, 4_096)?;
        for reply in &collected.replies {
            summary.datagrams += 1;
            // Undecodable datagrams are kept by the probe; skip them here.
            let Ok(packet) = read_packet(&reply.bytes, &to_client) else {
                continue;
            };
            for bunch in packet.bunches {
                if bunch.header.channel_index == GAME_STATE_CHANNEL_INDEX
                    && !bunch.header.open
                    && !bunch.header.reliable
                {
                    tracker.update(&bunch.payload, &mut summary);
                }
            }
        }
        client.keepalive()?;
        summary.keepalives += 1;
    }
    summary.held_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    client.close()?;
    Ok(summary)
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

    fn update(&mut self, payload: &super::packet::Payload, summary: &mut HoldSummary) {
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
        let now = Instant::now();
        if let Some(last) = self.last_update {
            let gap = u64::try_from(now.duration_since(last).as_millis()).unwrap_or(u64::MAX);
            summary.max_update_gap_ms = summary.max_update_gap_ms.max(gap);
        }
        self.last_update = Some(now);
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

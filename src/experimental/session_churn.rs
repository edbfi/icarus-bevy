//! EXPERIMENTAL session churn: repeated authored-probe session cycles against
//! an already running `icarus-server run` on IPv4 loopback.
//!
//! One cycle is the stateless handshake, Hello (expecting Challenge), Login
//! with synthetic identity material (expecting Welcome), Netspeed and Join in
//! one packet (expecting its acknowledgement), then a client control close.
//! Every receive is bounded by [`ChurnConfig::step_timeout`]. The probe and the
//! server share this repository's model, so a clean run shows agreement with
//! that model and the native lifecycle, not compatibility with any client.
//! The summary carries counts and timings only.

use std::{
    collections::BTreeMap,
    net::{SocketAddr, UdpSocket},
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, ensure};
use serde::Serialize;

use super::{
    control_messages::{
        ArchiveText, ControlCodec, ControlMessage, NetIdType, NetIdValue, UniqueNetId,
    },
    packet::{PacketConfig, Receiver},
    peer_lab::PeerLabConfig,
    probe_client::{ProbeClient, ProbeConfig},
};

/// Largest accepted [`ChurnConfig::cycles`].
pub const MAX_CHURN_CYCLES: u64 = 1_000_000;
/// Largest accepted [`ChurnConfig::concurrency`].
pub const MAX_CHURN_CONCURRENCY: usize = 64;
/// Synthetic Login identity; never logged by the server.
const SYNTHETIC_NAME: &str = "ChurnProbe";
const SYNTHETIC_NET_ID_HEX: &str = "00000000000000000001";

/// Churn parameters.
#[derive(Clone, Debug)]
pub struct ChurnConfig {
    /// IPv4 loopback address of the running server.
    pub server: SocketAddr,
    /// Total cycles, `1..=MAX_CHURN_CYCLES`, split across the workers.
    pub cycles: u64,
    /// Worker threads each running cycles sequentially, `1..=MAX_CHURN_CONCURRENCY`.
    pub concurrency: usize,
    /// Hello network version (the pinned release's checksum when the server
    /// checks versions; any value otherwise).
    pub network_version: u32,
    /// Bound on every receive of a cycle.
    pub step_timeout: Duration,
}

/// The cycle step at which a failure occurred.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ChurnStage {
    Handshake,
    Hello,
    Login,
    Join,
    Close,
}

/// One JSON-serializable churn result; counts and timings only.
#[derive(Clone, Debug, Default, Serialize)]
pub struct ChurnSummary {
    pub cycles_attempted: u64,
    pub cycles_succeeded: u64,
    pub cycles_failed: u64,
    pub concurrency: usize,
    pub wall_ms: u64,
    /// Per-cycle latency of succeeded cycles (nearest rank).
    pub cycle_p50_micros: Option<u64>,
    pub cycle_p99_micros: Option<u64>,
    pub cycle_max_micros: Option<u64>,
    pub failures_by_stage: BTreeMap<ChurnStage, u64>,
}

fn probe_config(step_timeout: Duration) -> ProbeConfig {
    ProbeConfig {
        packet: PacketConfig {
            magic_bits: 0,
            receiver: Receiver::Server,
            limits: PeerLabConfig::default().packet.packet.limits,
        },
        codec: ControlCodec::default(),
        handshake_timeout: step_timeout,
        handshake_attempts: 3,
        reply_window: step_timeout,
        max_datagrams: 1,
    }
}

fn login() -> Result<ControlMessage> {
    Ok(ControlMessage::Login {
        response: ArchiveText::text("0")?,
        url: ArchiveText::text(&format!("?Name={SYNTHETIC_NAME}"))?,
        unique_net_id: UniqueNetId {
            kind: NetIdType::Named(ArchiveText::text("STEAM")?),
            value: NetIdValue::from_hex(SYNTHETIC_NET_ID_HEX)?,
        },
        platform: ArchiveText::text("")?,
    })
}

/// The single control reply collected within the step timeout.
fn single_reply(client: &mut ProbeClient, timeout: Duration) -> Result<ControlMessage> {
    let collected = client.collect(timeout, 1)?;
    collected
        .messages()
        .next()
        .cloned()
        .context("no control reply")
}

/// Run one handshake → Hello → Login → Join → close cycle.
pub fn session_cycle(
    server: SocketAddr,
    network_version: u32,
    step_timeout: Duration,
) -> std::result::Result<(), (ChurnStage, anyhow::Error)> {
    let at = |stage| move |error| (stage, error);
    let socket = UdpSocket::bind("127.0.0.1:0")
        .and_then(|socket| socket.connect(server).map(|()| socket))
        .map_err(anyhow::Error::from)
        .map_err(at(ChurnStage::Handshake))?;
    let mut client = ProbeClient::connect(socket, probe_config(step_timeout))
        .map_err(at(ChurnStage::Handshake))?;
    let report = client
        .hello(network_version, "")
        .map_err(at(ChurnStage::Hello))?;
    match report.messages().next() {
        Some(ControlMessage::Challenge { .. }) => {}
        other => {
            return Err((
                ChurnStage::Hello,
                anyhow::anyhow!("expected Challenge, got {:?}", other.map(|m| m.name())),
            ));
        }
    }
    let login = login().map_err(at(ChurnStage::Login))?;
    client
        .send_control(&[login], false)
        .map_err(at(ChurnStage::Login))?;
    let reply = single_reply(&mut client, step_timeout).map_err(at(ChurnStage::Login))?;
    if !matches!(reply, ControlMessage::Welcome { .. }) {
        return Err((
            ChurnStage::Login,
            anyhow::anyhow!("expected Welcome, got {}", reply.name()),
        ));
    }
    let (_, packet_id) = client
        .send_control(
            &[
                ControlMessage::Netspeed { rate: 100_000 },
                ControlMessage::Join,
            ],
            false,
        )
        .map_err(at(ChurnStage::Join))?;
    let collected = client
        .collect(step_timeout, 1)
        .map_err(at(ChurnStage::Join))?;
    if !collected.acked.contains(&packet_id) {
        return Err((ChurnStage::Join, anyhow::anyhow!("Join not acknowledged")));
    }
    client.close().map_err(at(ChurnStage::Close))?;
    Ok(())
}

/// Nearest-rank percentile of ascending `sorted`.
fn percentile(sorted: &[u64], percent: usize) -> Option<u64> {
    let last = sorted.len().checked_sub(1)?;
    sorted.get(last * percent / 100).copied()
}

/// Run `config.cycles` cycles across `config.concurrency` workers.
pub fn run_churn(config: &ChurnConfig) -> Result<ChurnSummary> {
    ensure!(
        matches!(config.server, SocketAddr::V4(address) if address.ip().is_loopback()),
        "churn targets an IPv4 loopback server only"
    );
    ensure!(
        (1..=MAX_CHURN_CYCLES).contains(&config.cycles),
        "cycles must be 1..={MAX_CHURN_CYCLES}"
    );
    ensure!(
        (1..=MAX_CHURN_CONCURRENCY).contains(&config.concurrency),
        "concurrency must be 1..={MAX_CHURN_CONCURRENCY}"
    );
    ensure!(
        !config.step_timeout.is_zero(),
        "step timeout must be nonzero"
    );
    let workers = config.concurrency as u64;
    let started = Instant::now();
    let results = thread::scope(|scope| {
        let handles: Vec<_> = (0..workers)
            .map(|worker| {
                let share = config.cycles / workers + u64::from(worker < config.cycles % workers);
                scope.spawn(move || {
                    let mut latencies = Vec::with_capacity(share as usize);
                    let mut failures = Vec::new();
                    for _ in 0..share {
                        let cycle = Instant::now();
                        match session_cycle(
                            config.server,
                            config.network_version,
                            config.step_timeout,
                        ) {
                            Ok(()) => latencies.push(
                                u64::try_from(cycle.elapsed().as_micros()).unwrap_or(u64::MAX),
                            ),
                            Err((stage, _)) => failures.push(stage),
                        }
                    }
                    (latencies, failures)
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|handle| handle.join())
            .collect::<Vec<_>>()
    });
    let wall_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    let mut latencies = Vec::with_capacity(config.cycles as usize);
    let mut summary = ChurnSummary {
        cycles_attempted: config.cycles,
        concurrency: config.concurrency,
        wall_ms,
        ..ChurnSummary::default()
    };
    for result in results {
        let (worker_latencies, failures) =
            result.map_err(|_| anyhow::anyhow!("churn worker panicked"))?;
        latencies.extend(worker_latencies);
        for stage in failures {
            *summary.failures_by_stage.entry(stage).or_default() += 1;
        }
    }
    latencies.sort_unstable();
    summary.cycles_succeeded = latencies.len() as u64;
    summary.cycles_failed = config.cycles - summary.cycles_succeeded;
    summary.cycle_p50_micros = percentile(&latencies, 50);
    summary.cycle_p99_micros = percentile(&latencies, 99);
    summary.cycle_max_micros = latencies.last().copied();
    Ok(summary)
}

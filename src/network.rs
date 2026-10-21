//! EXPERIMENTAL opt-in session layer for `icarus-server run`.
//!
//! When `[network] enabled = true`, the production run binds one IPv4
//! loopback UDP socket before any world state is opened and installs the
//! loopback peer lab ([`PeerLab`] + [`PeerLabPlugin`]) with game admission
//! enabled: handshake, Hello/Challenge, Login/Welcome and Join against the
//! inferred admission model, nothing more. No replication, gameplay or
//! identity verification runs, and nothing here demonstrates compatibility
//! with any ICARUS client. Status records carry counts only.
//!
//! With `[network] release_profile` set, the release TOML is loaded and
//! validated before the socket binds and its network checksum becomes the
//! lab's local Hello version: a differing Hello gets Upgrade and the peer is
//! removed (`upgrades_sent`, `closed_by_reply`). Unset, any version is accepted.
//! When the profile carries `[player_controller]` identifiers (built in for
//! the qualified builds, F-780), every successful Join is answered with the
//! PlayerController open on actor channel 2 (F-778, F-779): exports, spawn at
//! [`SPAWN_LOCATION`], roles, references and component blocks. Nothing else
//! replicates, and the PlayerState it references is never opened.
//!
//! Ordering: the lab polls at most `PACKETS_PER_UPDATE` datagrams once per
//! app update in `RunFixedMainLoopSystems::BeforeFixedMainLoop`, so network
//! input is processed before every `SimulationSet` of that update's fixed
//! ticks. Shutdown first stops admission ([`stop_admission`]), then removes
//! every session and drops the socket ([`teardown`]); both are idempotent.
use std::net::{SocketAddr, SocketAddrV4};

use anyhow::{Context, Result};
use bevy::prelude::{App, Resource, World};

use crate::{
    config::{NetworkConfig, read_configuration},
    experimental::{
        admission::AdmissionPolicy,
        peer_lab::{PeerLab, PeerLabAdmission, PeerLabConfig, PeerLabPlugin, SessionCounts},
        release::{ReleasePin, ReleaseProfile},
    },
};

/// Spectator slots granted beside `max_players` (the lab's default policy).
pub const SPECTATOR_SLOTS: u32 = 2;
/// Lab choice: the PlayerController spawn location (no world geometry or
/// PlayerStart is consulted).
pub const SPAWN_LOCATION: [f32; 3] = [0.0, 0.0, 0.0];

/// Session counts kept after [`teardown`] so a repeated call and the final
/// record report the same values.
#[derive(Resource, Clone, Debug)]
struct ClosedSessions(SessionCounts);

/// Peer-lab parameters for `network`: admission enabled with `max_players`
/// as the occupancy limit. The peer table holds twice the admitted slots so
/// sessions beyond capacity can still complete a handshake and receive the
/// admission Failure instead of silence.
pub fn peer_lab_config(network: &NetworkConfig) -> PeerLabConfig {
    let defaults = PeerLabAdmission::default();
    let max_players = i32::try_from(network.max_players).unwrap_or(i32::MAX);
    let admission = PeerLabAdmission {
        policy: AdmissionPolicy {
            max_players,
            max_spectators: SPECTATOR_SLOTS as i32,
            ..defaults.policy
        },
        ..defaults
    };
    PeerLabConfig {
        max_peers: 2 * (network.max_players + SPECTATOR_SLOTS) as usize,
        idle_timeout: network.session_timeout(),
        admission: Some(admission),
        ..PeerLabConfig::default()
    }
}

/// A bound session socket and the server's own release pin its Hello
/// version check uses, if one is configured.
pub struct BoundNetwork {
    pub lab: PeerLab,
    pub release: Option<ReleasePin>,
}

/// Load and validate the configured release profile (bounded regular-file
/// read), or `None` when `release_profile` is unset.
pub fn release_profile(network: &NetworkConfig) -> Result<Option<ReleaseProfile>> {
    network
        .release_profile
        .as_deref()
        .map(|path| {
            read_configuration(path)
                .and_then(|text| ReleaseProfile::parse(&text))
                .with_context(|| format!("load network.release_profile {}", path.display()))
        })
        .transpose()
}

/// Validate `network`, load its release profile and only then bind its
/// socket, or return `None` when disabled.
pub fn bind(network: &NetworkConfig) -> Result<Option<BoundNetwork>> {
    if !network.enabled {
        return Ok(None);
    }
    network.validate()?;
    let profile = release_profile(network)?;
    let mut config = peer_lab_config(network);
    if let Some(profile) = &profile {
        config = config.with_release_profile(profile);
    }
    let plan = match (&profile, &config.admission) {
        (Some(profile), Some(admission)) => profile
            .player_controller_open_plan(admission.map_url, SPAWN_LOCATION)
            .context("compose the PlayerController open plan")?,
        _ => None,
    };
    let address = SocketAddrV4::new(network.bind, network.port);
    let mut lab = PeerLab::bind_address(address, config)
        .context("bind the experimental network session socket")?;
    if let Some(plan) = plan {
        lab.enable_player_controller_open(plan)?;
    }
    Ok(Some(BoundNetwork {
        lab,
        release: profile.map(|profile| profile.pin),
    }))
}

/// Install a bound lab into the production app; returns its local address.
pub fn install(app: &mut App, lab: PeerLab) -> Result<SocketAddr> {
    let address = lab.local_addr().context("network session socket address")?;
    app.insert_resource(lab).add_plugins(PeerLabPlugin);
    Ok(address)
}

/// Current counts, the counts kept by [`teardown`], or `None` when the
/// session layer was never installed.
pub fn session_counts(world: &World) -> Option<SessionCounts> {
    world
        .get_resource::<PeerLab>()
        .map(PeerLab::session_counts)
        .or_else(|| {
            world
                .get_resource::<ClosedSessions>()
                .map(|closed| closed.0.clone())
        })
}

/// Stop admitting new sessions; live sessions are kept. Idempotent.
pub fn stop_admission(world: &mut World) {
    if let Some(mut lab) = world.get_resource_mut::<PeerLab>() {
        lab.stop_admission();
    }
}

/// Stop admission, remove every session and drop the socket. Idempotent: a
/// repeated call returns the same final counts.
pub fn teardown(world: &mut World) -> Option<SessionCounts> {
    if let Some(mut lab) = world.remove_resource::<PeerLab>() {
        lab.shutdown();
        let counts = lab.session_counts();
        drop(lab);
        world.insert_resource(ClosedSessions(counts));
    }
    world
        .get_resource::<ClosedSessions>()
        .map(|closed| closed.0.clone())
}

//! Bounded connected native qualification with Steam ticket verification.
//! Uses authored empty-world defaults and release-specific initialization.
//! No compatibility, gameplay, or official-save import claim.
use std::{
    collections::BTreeMap,
    io::{self, Write},
    net::{Ipv4Addr, SocketAddrV4, UdpSocket},
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, ensure};
use bevy::prelude::{App, MinimalPlugins};
use clap::Parser;
use icarus_bevy::{
    config::NetworkConfig,
    experimental::{
        archive::ByteOrder,
        game_state_open::handle,
        manager_open::{ManagerOpenPlan, ManagerSlot, compose_manager_open},
        peer_lab::{ControllerStartupAction, PeerLab},
        prospect_info::ProspectInfo,
        release::ReleaseProfile,
        rep_layout::RepValue,
        session_open::{ConnectionExports, NetGuidCache},
    },
    network,
    steam_fetch::net::{self, HttpTimeouts, Tls},
    steam_presence::{
        Advert, PresenceHandles, RegistrationTiming, encode_prospect_info, logon_game_server,
        serve_queries, version_tag,
    },
    steam_service::{ServiceLimits, SteamService},
};
use serde_json::json;

// Protocol identities observed in S2; the native world remains independently
// authored and empty. These names do not import terrain or gameplay tables.
const PROBE_MAP: &str = "/Game/Maps/Terrain_016_OLY/Terrain_016";
const PROBE_PROSPECT_KEY: &str = "Outpost006_Olympus";
const PROBE_PROSPECT_ID: &str = "native-auth-probe";

#[derive(Clone, Copy, Debug, clap::ValueEnum)]
enum StartupAction {
    FlushLevelStreaming,
    RestartPawn,
}

impl StartupAction {
    fn action(self) -> ControllerStartupAction {
        match self {
            Self::FlushLevelStreaming => ControllerStartupAction::FlushLevelStreaming,
            Self::RestartPawn => ControllerStartupAction::RestartPawn,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::FlushLevelStreaming => "flush-level-streaming",
            Self::RestartPawn => "restart-pawn",
        }
    }
}
#[derive(clap::Args)]
struct AuthoredWorld {
    #[arg(long, default_value_t = 1, allow_hyphen_values = true)]
    seed: i32,
    #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
    seed_initialised: bool,
    /// Authored tile references; startup also checks the actual wire codec limits.
    #[arg(long, default_value_t = 0, value_parser = clap::value_parser!(i32).range(0..=65535))]
    flod_tile_count: i32,
}

impl AuthoredWorld {
    fn plan(&self, profile: &ReleaseProfile) -> Result<(BTreeMap<u32, RepValue>, ManagerOpenPlan)> {
        let flod = profile
            .manager_open_plan(ManagerSlot::Flod, PROBE_MAP)?
            .context("probe release has no FLOD manager plan")?
            .with_flod_tiles(self.flod_tile_count)?;
        // The array's u16 count alone does not establish that the manager open
        // fits the entry and payload budgets. This early rejection uses fresh
        // GUID/export state; it is not a guarantee for every live-cache encoding.
        // Runtime composition remains authoritative for the assigned GUIDs/channel.
        for order in [ByteOrder::Little, ByteOrder::Big] {
            compose_manager_open(
                &mut NetGuidCache::new(),
                &mut ConnectionExports::new(),
                &flod,
                ManagerSlot::Flod.key(),
                6,
                order,
            )
            .context("authored FLOD manager exceeds the wire codec limits")?;
        }
        Ok((
            BTreeMap::from([
                (handle::SEED, RepValue::Int(self.seed)),
                (
                    handle::SEED_INITIALISED,
                    RepValue::Bool(self.seed_initialised),
                ),
            ]),
            flod,
        ))
    }

    fn report(&self) -> serde_json::Value {
        json!({"seed": self.seed, "seed_initialised": self.seed_initialised,
            "flod_tile_count": self.flod_tile_count})
    }
}

#[derive(Parser)]
#[command(about = "Bounded Steam-authenticated native game listener; compatibility unproven")]
struct Options {
    /// Explicit experimental controller calls, in flag order; at most 32.
    #[arg(long, value_enum)]
    controller_startup: Vec<StartupAction>,
    #[command(flatten)]
    authored_world: AuthoredWorld,
    #[arg(long)]
    name: String,
    #[arg(long, default_value = "0.0.0.0:17777")]
    game_bind: SocketAddrV4,
    #[arg(long, default_value = "0.0.0.0:27015")]
    query_bind: SocketAddrV4,
    #[arg(long, default_value = "127.0.0.1")]
    private_ip: Ipv4Addr,
    #[arg(long, default_value = "config/release.toml")]
    release: PathBuf,
    #[arg(long, default_value_t = 300, value_parser = clap::value_parser!(u64).range(30..=1800))]
    seconds: u64,
    /// Flush aggregate progress to stderr at this cadence; never includes player values.
    #[arg(long, default_value_t = 5, value_parser = clap::value_parser!(u64).range(1..=60))]
    progress_interval_seconds: u64,
}

impl Options {
    /// Pure preflight before sockets, TLS or Steam startup. Live configuration
    /// still validates against the actual controller and pawn plans.
    fn startup_plan(&self, profile: &ReleaseProfile) -> Result<Vec<ControllerStartupAction>> {
        ensure!(
            self.controller_startup.len() <= 32,
            "at most 32 controller startup actions"
        );
        if self.controller_startup.is_empty() {
            return Ok(vec![]);
        }
        let controller = profile
            .player_controller_open_plan(PROBE_MAP, [0.0; 3])?
            .context("startup requires a controller profile")?;
        let pawn = profile.pawn_open_plan(PROBE_MAP)?;
        let actions: Vec<_> = self
            .controller_startup
            .iter()
            .map(|action| action.action())
            .collect();
        icarus_bevy::experimental::peer_lab::validate_controller_startup(
            profile,
            &actions,
            &controller,
            pawn.is_some(),
        )?;
        Ok(actions)
    }
}

fn emit_progress(value: &serde_json::Value) -> Result<()> {
    let mut stderr = io::stderr().lock();
    serde_json::to_writer(&mut stderr, value)?;
    writeln!(&mut stderr)?;
    stderr.flush()?;
    Ok(())
}

fn main() -> Result<()> {
    let options = Options::parse();
    ensure!(
        options.game_bind.port() != 0 && options.query_bind.port() != 0,
        "probe ports must be nonzero"
    );
    ensure!(
        options.game_bind.port() != options.query_bind.port(),
        "game and query ports must differ"
    );
    let profile = ReleaseProfile::load(&options.release)?;
    let startup = options.startup_plan(&profile)?;
    let (game_state, flod) = options.authored_world.plan(&profile)?;
    let prospect = ProspectInfo {
        prospect_id: PROBE_PROSPECT_ID.encode_utf16().collect(),
        prospect_dt_key: PROBE_PROSPECT_KEY.encode_utf16().collect(),
        lobby_name: options.name.encode_utf16().collect(),
        expire_time: -1,
        prospect_state: 2,
        difficulty: 2,
        ..ProspectInfo::default()
    };
    let mut advert = Advert {
        server_steam_id: 0,
        name: options.name,
        map: String::new(),
        game_port: options.game_bind.port(),
        query_port: options.query_bind.port(),
        max_players: 1,
        players: Vec::new(),
        version_tag: version_tag(&profile.pin.project_version)?,
        prospect: Some(PROBE_PROSPECT_ID.to_owned()),
        prospect_info: Some(encode_prospect_info(&prospect)?),
    };
    advert.validate()?;
    let query = UdpSocket::bind(options.query_bind).context("bind query listener")?;
    let tls = Tls::from_native_roots()?;
    let http = net::http_agent(&tls, &HttpTimeouts::default());
    let (session, logon, _) = logon_game_server(
        &tls,
        &http,
        options.private_ip,
        RegistrationTiming::default(),
    )?;
    advert.server_steam_id = logon.steam_id;
    let presence = PresenceHandles {
        advert: Arc::new(Mutex::new(advert)),
        stop: Arc::new(AtomicBool::new(false)),
    };
    let stop = presence.stop.clone();
    ctrlc::set_handler(move || stop.store(true, Ordering::Relaxed))?;
    let deadline = Instant::now() + Duration::from_secs(options.seconds);
    let service = SteamService::spawn(
        session,
        presence.clone(),
        deadline,
        ServiceLimits::default(),
    )?;
    let config = NetworkConfig {
        enabled: true,
        max_players: 1,
        session_timeout_seconds: 30,
        release_profile: Some(options.release),
        ..NetworkConfig::default()
    };
    let mut peer_config = network::peer_lab_config(&config);
    peer_config
        .admission
        .as_mut()
        .context("probe admission disabled")?
        .map_url = PROBE_MAP;
    let mut bound = network::bind_authenticated_with_parameters(
        &config,
        peer_config,
        options.game_bind,
        service.client(),
    )?
    .context("network disabled")?;
    bound.lab.refresh_active_prospect(&prospect)?;
    // Independently authored empty-world initialization, supplied before any
    // connection is polled. Connected world entry remains unqualified.
    bound.lab.extend_game_state_values(game_state)?;
    bound.lab.add_manager_open(ManagerSlot::Flod, flod)?;
    if !startup.is_empty() {
        bound.lab.enable_controller_startup(&profile, startup)?;
    }
    network::apply_server_settings(&mut bound.lab, 60)?;
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    network::install_configured(&mut app, bound, &config)?;
    let query_presence = presence.clone();
    let responder =
        thread::spawn(move || serve_queries(&query, &query_presence.advert, &query_presence.stop));
    let progress_interval = Duration::from_secs(options.progress_interval_seconds);
    let mut next_progress = Instant::now() + progress_interval;
    let mut output_error = emit_progress(&json!({
        "event": "native_join_ready",
        "game_port": options.game_bind.port(),
        "query_port": options.query_bind.port(),
        "build": profile.pin.build_id,
        "seconds": options.seconds,
        "progress_interval_seconds": options.progress_interval_seconds,
        "authored_world": options.authored_world.report(),
        "controller_startup": options.controller_startup.iter().map(|action| action.name()).collect::<Vec<_>>(),
        "compatibility_proven": false,
    }))
    .err();
    while Instant::now() < deadline
        && !presence.stop.load(Ordering::Relaxed)
        && app.world().resource::<PeerLab>().is_admitting()
        && output_error.is_none()
    {
        app.update();
        let now = Instant::now();
        if now >= next_progress {
            let lab = app.world().resource::<PeerLab>();
            output_error = emit_progress(&json!({"event":"native_join_progress","incomplete":true,"sessions":lab.session_counts(),"readiness":lab.readiness_counts(),"stats":lab.stats(),"compatibility_proven":false})).err();
            // Schedule from now: slow updates never create a catch-up burst.
            next_progress = now + progress_interval;
        }
        thread::sleep(Duration::from_millis(5));
    }
    let reason = if output_error.is_some() {
        "output_error"
    } else if Instant::now() >= deadline {
        "deadline"
    } else if !app.world().resource::<PeerLab>().is_admitting() {
        "admission_closed"
    } else {
        "stop_requested"
    };
    let mut lab = app.world_mut().resource_mut::<PeerLab>();
    let counts = lab.session_counts();
    let readiness = lab.readiness_counts();
    let stats = lab.stats().clone();
    let peak = lab.peak_peer_count();
    lab.shutdown();
    let service_result = service.stop_and_join();
    let query_result = responder
        .join()
        .map_err(|_| anyhow::anyhow!("query worker panicked"))
        .and_then(|result| result);
    let reason = if service_result.is_err() {
        "service_error"
    } else if query_result.is_err() {
        "query_error"
    } else {
        reason
    };
    let summary = json!({"event":"native_join_stopped","reason":reason,"sessions_before_shutdown":counts,"readiness_before_shutdown":readiness,"stats_before_shutdown":stats,"peak_peers":peak,"queries":query_result.as_ref().ok(),"service_ok":service_result.is_ok(),"query_ok":query_result.is_ok(),"compatibility_proven":false});
    let mut stdout = io::stdout().lock();
    serde_json::to_writer(&mut stdout, &summary)?;
    writeln!(&mut stdout)?;
    stdout.flush()?;
    service_result?;
    query_result?;
    if let Some(error) = output_error {
        return Err(error.context("write native join progress"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn startup_flags_preserve_order_and_reject_invalid_plans_before_networking() {
        let profile = ReleaseProfile::release_3_0_32();
        let options = Options::try_parse_from([
            "probe",
            "--name",
            "Authored",
            "--controller-startup",
            "restart-pawn",
            "--controller-startup",
            "flush-level-streaming",
        ])
        .unwrap();
        assert_eq!(
            options.startup_plan(&profile).unwrap(),
            vec![
                ControllerStartupAction::RestartPawn,
                ControllerStartupAction::FlushLevelStreaming
            ]
        );
        let default = Options::try_parse_from(["probe", "--name", "Authored"]).unwrap();
        assert!(default.startup_plan(&profile).unwrap().is_empty());
        assert!(
            Options::try_parse_from([
                "probe",
                "--name",
                "Authored",
                "--controller-startup",
                "unknown"
            ])
            .is_err()
        );
        let mut too_many = default;
        too_many.controller_startup = vec![StartupAction::FlushLevelStreaming; 33];
        assert!(too_many.startup_plan(&profile).is_err());
        let mut no_pawn = profile.clone();
        no_pawn.pawn = None;
        assert!(options.startup_plan(&no_pawn).is_err());
        let mut small_bound = profile;
        small_bound
            .class_net_cache
            .bp_icarus_player_controller_survival = 40;
        assert!(options.startup_plan(&small_bound).is_err());
    }

    #[test]
    fn authored_world_defaults_and_wire_budget_are_checked_before_startup() {
        let profile = ReleaseProfile::load(std::path::Path::new("config/release.toml")).unwrap();
        let options = Options::try_parse_from(["probe", "--name", "Authored"]).unwrap();
        assert_eq!(
            options.authored_world.report(),
            json!({"seed": 1, "seed_initialised": true, "flod_tile_count": 0})
        );
        // Nonempty experiments remain usable; the u16 boundary and the codec's
        // array boundary must reject before an external service is contacted.
        for (count, accepted) in [
            (0, true),
            (1, true),
            (3, true),
            (4096, false),
            (65535, false),
        ] {
            let world = AuthoredWorld {
                seed: i32::MIN,
                seed_initialised: false,
                flod_tile_count: count,
            };
            assert_eq!(world.plan(&profile).is_ok(), accepted, "count={count}");
        }
    }

    #[test]
    fn authored_options_plan_and_report_agree() {
        use icarus_bevy::experimental::manager_open::FLOD_TILES_HANDLE;
        let profile = ReleaseProfile::load(std::path::Path::new("config/release.toml")).unwrap();
        for args in [
            vec![],
            vec![
                "--seed",
                "-7",
                "--seed-initialised",
                "false",
                "--flod-tile-count",
                "3",
            ],
        ] {
            let options = Options::try_parse_from(
                ["native_join_probe", "--name", "Authored"]
                    .into_iter()
                    .chain(args),
            )
            .unwrap();
            let world = &options.authored_world;
            let (values, flod) = world.plan(&profile).unwrap();
            let report = world.report();
            assert_eq!(
                values[&handle::SEED],
                RepValue::Int(report["seed"].as_i64().unwrap() as i32)
            );
            assert_eq!(
                values[&handle::SEED_INITIALISED],
                RepValue::Bool(report["seed_initialised"].as_bool().unwrap())
            );
            assert_eq!(
                flod.actor_arrays
                    .get(&FLOD_TILES_HANDLE)
                    .map_or(0, Vec::len),
                report["flod_tile_count"].as_u64().unwrap() as usize
            );
        }
    }

    #[test]
    fn invalid_flod_counts_fail_argument_parsing() {
        for count in ["-1", "65536", "2147483648"] {
            assert!(
                Options::try_parse_from([
                    "probe",
                    "--name",
                    "Authored",
                    "--flod-tile-count",
                    count
                ])
                .is_err()
            );
        }
    }

    #[test]
    fn progress_cadence_is_bounded_and_has_a_useful_default() {
        let defaults =
            Options::try_parse_from(["native_join_probe", "--name", "Authored"]).unwrap();
        assert_eq!(defaults.progress_interval_seconds, 5);
        for seconds in ["0", "61"] {
            assert!(
                Options::try_parse_from([
                    "native_join_probe",
                    "--name",
                    "Authored",
                    "--progress-interval-seconds",
                    seconds
                ])
                .is_err()
            );
        }
        for seconds in ["1", "60"] {
            assert!(
                Options::try_parse_from([
                    "native_join_probe",
                    "--name",
                    "Authored",
                    "--progress-interval-seconds",
                    seconds
                ])
                .is_ok()
            );
        }
    }
}

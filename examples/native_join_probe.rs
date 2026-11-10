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
        manager_open::{ManagerOpenPlan, ManagerSlot},
        peer_lab::{ControllerStartupAction, PeerLab},
        prospect_info::ProspectInfo,
        release::ReleaseProfile,
        rep_layout::RepValue,
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

#[path = "native_join_probe/capture.rs"]
mod native_join_capture;

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
        icarus_bevy::authored_world_bootstrap::AuthoredWorldBootstrap {
            seed: self.seed,
            seed_initialised: self.seed_initialised,
            flod_tile_count: self.flod_tile_count,
        }
        .plan(profile, PROBE_MAP)
    }
    fn report(&self) -> serde_json::Value {
        json!({"seed": self.seed, "seed_initialised": self.seed_initialised,
            "flod_tile_count": self.flod_tile_count})
    }
}

#[derive(Parser)]
#[command(about = "Bounded Steam-authenticated native game listener; compatibility unproven")]
struct Options {
    /// PRIVATE selected actor RPC82 JSONL, exclusively created under checkout work/.
    #[arg(long, conflicts_with = "private_controller_headers_output")]
    private_rpc82_output: Option<PathBuf>,
    /// PRIVATE controller subobject headers only; no payloads or field identities.
    #[arg(long, conflicts_with = "private_rpc82_output")]
    private_controller_headers_output: Option<PathBuf>,
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
    /// Replicate each finalised player's ReplicatedConnectedPlayer record under the
    /// GameState's ConnectedPlayers; `false` makes a comparison run without it.
    #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
    connected_player_records: bool,
    /// After each pawn open, send the PlayerController's Pawn and IcarusPlayerCharacter
    /// properties naming that pawn (unqualified); `false` makes a comparison run without them.
    #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
    controller_possession_properties: bool,
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

    /// Pure preflight of `--connected-player-records`: enabled records need a
    /// release that names the record class and the opens a record references.
    fn connected_player_records(&self, profile: &ReleaseProfile) -> Result<bool> {
        if !self.connected_player_records {
            return Ok(false);
        }
        ensure!(
            profile.connected_players_plan()?.is_some()
                && profile.pawn_open_plan(PROBE_MAP)?.is_some()
                && profile.player_state_open_plan(PROBE_MAP)?.is_some()
                && profile.game_state_open_plan(PROBE_MAP)?.is_some(),
            "connected-player records need the release's record class and pawn, GameState and PlayerState profiles; pass --connected-player-records false"
        );
        Ok(true)
    }

    /// Pure preflight of `--controller-possession-properties`: enabled
    /// properties need the release's controller and pawn opens.
    fn controller_possession_properties(&self, profile: &ReleaseProfile) -> Result<bool> {
        if !self.controller_possession_properties {
            return Ok(false);
        }
        ensure!(
            profile
                .player_controller_open_plan(PROBE_MAP, [0.0; 3])?
                .is_some()
                && profile.pawn_open_plan(PROBE_MAP)?.is_some(),
            "controller possession properties need the release's controller and pawn profiles; pass --controller-possession-properties false"
        );
        Ok(true)
    }
}

/// Aggregate counts read before [`PeerLab::shutdown`], as the summary has
/// always reported them.
fn counts_before_shutdown(lab: &PeerLab) -> serde_json::Map<String, serde_json::Value> {
    let mut counts = serde_json::Map::new();
    counts.insert(
        "sessions_before_shutdown".into(),
        json!(lab.session_counts()),
    );
    counts.insert(
        "readiness_before_shutdown".into(),
        json!(lab.readiness_counts()),
    );
    counts.insert(
        "controller_fields_before_shutdown".into(),
        json!(lab.controller_field_totals()),
    );
    // Lab-wide over every connection since bind, not per session: it reads
    // cleanly only with one client connected (the probe admits one).
    counts.insert(
        "unreliable_actor_bunches_before_shutdown".into(),
        json!(lab.unreliable_actor_bunch_counts()),
    );
    counts.insert("stats_before_shutdown".into(), json!(lab.stats()));
    counts.insert("peak_peers".into(), json!(lab.peak_peer_count()));
    counts
}

/// Shut the lab down and return the session counts that only shutdown
/// moves (`shutdown_failures_sent`, `shutdown_closes_sent`,
/// `closed_by_shutdown`), read afterwards.
fn shut_down(lab: &mut PeerLab) -> serde_json::Map<String, serde_json::Value> {
    lab.shutdown();
    let mut counts = serde_json::Map::new();
    counts.insert(
        "sessions_after_shutdown".into(),
        json!(lab.session_counts()),
    );
    counts
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
    let connected_player_records = options.connected_player_records(&profile)?;
    let controller_possession_properties = options.controller_possession_properties(&profile)?;
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
    // Must fail before sockets, TLS, Steam initialization, or authentication.
    let header_capture = options.private_controller_headers_output.is_some();
    let work_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("work");
    let private_capture = if header_capture {
        native_join_capture::prepare_headers(
            options.private_controller_headers_output.as_deref(),
            &work_root,
        )?
    } else {
        native_join_capture::prepare(options.private_rpc82_output.as_deref(), &work_root)?
    };
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
        connected_player_records,
        controller_possession_properties,
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
    if let Some(capture) = private_capture {
        bound.lab.install_private_rpc82_capture(capture)?;
    }
    // Independently authored empty-world initialization, supplied before any
    // connection is polled. Connected world entry remains unqualified.
    bound.lab.extend_game_state_values(game_state)?;
    bound.lab.add_manager_open(ManagerSlot::Flod, flod)?;
    ensure!(
        bound.lab.has_connected_player_records() == connected_player_records,
        "connected-player record configuration did not apply"
    );
    ensure!(
        bound.lab.has_controller_possession_properties() == controller_possession_properties,
        "controller possession configuration did not apply"
    );
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
        "connected_player_records": connected_player_records,
        "controller_possession_properties": controller_possession_properties,
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
            let mut progress = json!({"event":"native_join_progress","incomplete":true,"sessions":lab.session_counts(),"readiness":lab.readiness_counts(),"controller_fields":lab.controller_field_totals(),"unreliable_actor_bunches":lab.unreliable_actor_bunch_counts(),"stats":lab.stats(),"compatibility_proven":false});
            native_join_capture::add_counts(
                &mut progress,
                lab.private_rpc82_capture_counts(),
                header_capture,
            );
            output_error = emit_progress(&progress).err();
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
    let before = counts_before_shutdown(&lab);
    let private_capture = lab.finish_private_rpc82_capture();
    let after = shut_down(&mut lab);
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
    let mut summary = json!({"event":"native_join_stopped","reason":reason,"queries":query_result.as_ref().ok(),"service_ok":service_result.is_ok(),"query_ok":query_result.is_ok(),"compatibility_proven":false});
    if let Some(object) = summary.as_object_mut() {
        object.extend(before);
        object.extend(after);
    }
    native_join_capture::add_counts(&mut summary, private_capture, header_capture);
    let mut stdout = io::stdout().lock();
    serde_json::to_writer(&mut stdout, &summary)?;
    writeln!(&mut stdout)?;
    stdout.flush()?;
    ensure!(
        private_capture.is_none_or(|counts| counts.write_errors == 0),
        "private capture incomplete: write failure (see counts)"
    );
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
    fn private_header_selection_is_opt_in_and_excludes_payload_capture() {
        let default = Options::try_parse_from(["probe", "--name", "Authored"]).unwrap();
        assert!(default.private_controller_headers_output.is_none());
        assert!(default.private_rpc82_output.is_none());
        let selected = Options::try_parse_from([
            "probe",
            "--name",
            "Authored",
            "--private-controller-headers-output",
            "work/authored/headers.jsonl",
        ])
        .unwrap();
        assert!(selected.private_controller_headers_output.is_some());
        assert!(
            Options::try_parse_from([
                "probe",
                "--name",
                "Authored",
                "--private-controller-headers-output",
                "work/authored/headers.jsonl",
                "--private-rpc82-output",
                "work/authored/payload.jsonl",
            ])
            .is_err()
        );
    }

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
        use icarus_bevy::experimental::game_state_open::handle;
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
    fn connected_player_records_default_on_and_fail_closed_before_networking() {
        let profile = ReleaseProfile::load(std::path::Path::new("config/release.toml")).unwrap();
        let default = Options::try_parse_from(["probe", "--name", "Authored"]).unwrap();
        assert!(default.connected_player_records);
        assert!(default.connected_player_records(&profile).unwrap());
        let off = Options::try_parse_from([
            "probe",
            "--name",
            "Authored",
            "--connected-player-records",
            "false",
        ])
        .unwrap();
        assert!(!off.connected_player_records(&profile).unwrap());
        for value in ["maybe", ""] {
            assert!(
                Options::try_parse_from([
                    "probe",
                    "--name",
                    "Authored",
                    "--connected-player-records",
                    value,
                ])
                .is_err()
            );
        }
        // A release without the record class cannot run the default.
        let mut unnamed = profile.clone();
        unnamed.game_state.as_mut().unwrap().connected_player_record = None;
        assert!(default.connected_player_records(&unnamed).is_err());
        assert!(!off.connected_player_records(&unnamed).unwrap());
        let mut no_pawn = profile;
        no_pawn.pawn = None;
        assert!(default.connected_player_records(&no_pawn).is_err());
    }

    #[test]
    fn controller_possession_properties_default_on_and_fail_closed_before_networking() {
        let profile = ReleaseProfile::load(std::path::Path::new("config/release.toml")).unwrap();
        let default = Options::try_parse_from(["probe", "--name", "Authored"]).unwrap();
        assert!(default.controller_possession_properties);
        assert!(default.controller_possession_properties(&profile).unwrap());
        let off = Options::try_parse_from([
            "probe",
            "--name",
            "Authored",
            "--controller-possession-properties",
            "false",
        ])
        .unwrap();
        assert!(!off.controller_possession_properties(&profile).unwrap());
        for value in ["maybe", ""] {
            assert!(
                Options::try_parse_from([
                    "probe",
                    "--name",
                    "Authored",
                    "--controller-possession-properties",
                    value,
                ])
                .is_err()
            );
        }
        let mut no_pawn = profile.clone();
        no_pawn.pawn = None;
        assert!(default.controller_possession_properties(&no_pawn).is_err());
        assert!(!off.controller_possession_properties(&no_pawn).unwrap());
        let mut no_controller = profile;
        no_controller.player_controller = None;
        assert!(
            default
                .controller_possession_properties(&no_controller)
                .is_err()
        );
    }

    #[test]
    fn stopped_summary_reports_counts_that_only_shutdown_moves() {
        use icarus_bevy::experimental::{handshake::initial_request, peer_lab::PeerLabConfig};
        let mut lab = PeerLab::bind(0, PeerLabConfig::default()).unwrap();
        let client = UdpSocket::bind("127.0.0.1:0").unwrap();
        client.connect(lab.local_addr().unwrap()).unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        // Authored stateless handshake: challenge, echo, acknowledgement.
        let mut buffer = [0; 2048];
        client.send(&initial_request()).unwrap();
        lab.poll();
        let length = client.recv(&mut buffer).unwrap();
        client.send(&buffer[..length]).unwrap();
        lab.poll();
        client.recv(&mut buffer).unwrap();
        assert_eq!(lab.peer_count(), 1);
        let before = counts_before_shutdown(&lab);
        let after = shut_down(&mut lab);
        assert_eq!(lab.peer_count(), 0);
        let mut summary = json!({"event": "native_join_stopped"});
        let object = summary.as_object_mut().unwrap();
        object.extend(before);
        object.extend(after);
        let failures = "/sessions_before_shutdown/shutdown_failures_sent";
        assert_eq!(summary.pointer(failures), Some(&json!(0)));
        assert_eq!(
            summary.pointer("/stats_before_shutdown/shutdown_failures_sent"),
            Some(&json!(0))
        );
        let after = "/sessions_after_shutdown";
        assert_eq!(
            summary.pointer(&format!("{after}/shutdown_failures_sent")),
            Some(&json!(1))
        );
        // The peer never delivered a reliable control bunch: no close.
        assert_eq!(
            summary.pointer(&format!("{after}/shutdown_closes_sent")),
            Some(&json!(0))
        );
        assert_eq!(
            summary.pointer(&format!("{after}/closed_by_shutdown")),
            Some(&json!(1))
        );
        assert_eq!(
            summary.pointer(&format!("{after}/active_peers")),
            Some(&json!(0))
        );
        for key in [
            "sessions_before_shutdown",
            "readiness_before_shutdown",
            "controller_fields_before_shutdown",
            "unreliable_actor_bunches_before_shutdown",
            "stats_before_shutdown",
            "peak_peers",
        ] {
            assert!(summary.get(key).is_some(), "{key}");
        }
        assert_eq!(summary["peak_peers"], 1);
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

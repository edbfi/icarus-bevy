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
        game_state_open::handle, manager_open::ManagerSlot, peer_lab::PeerLab,
        prospect_info::ProspectInfo, release::ReleaseProfile, rep_layout::RepValue,
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

#[derive(Parser)]
#[command(about = "Bounded Steam-authenticated native game listener; compatibility unproven")]
struct Options {
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
    bound.lab.extend_game_state_values(BTreeMap::from([
        (handle::SEED, RepValue::Int(1)),
        (handle::SEED_INITIALISED, RepValue::Bool(true)),
    ]))?;
    let flod = profile
        .manager_open_plan(ManagerSlot::Flod, PROBE_MAP)?
        .context("probe release has no FLOD manager plan")?
        .with_flod_tiles(0)?;
    bound.lab.add_manager_open(ManagerSlot::Flod, flod)?;
    network::apply_server_settings(&mut bound.lab, 60)?;
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    network::install_configured(&mut app, bound, &config)?;
    let query_presence = presence.clone();
    let responder =
        thread::spawn(move || serve_queries(&query, &query_presence.advert, &query_presence.stop));
    let progress_interval = Duration::from_secs(options.progress_interval_seconds);
    let mut next_progress = Instant::now() + progress_interval;
    let mut output_error = emit_progress(&json!({"event":"native_join_ready","game_port":options.game_bind.port(),"query_port":options.query_bind.port(),"build":profile.pin.build_id,"seconds":options.seconds,"progress_interval_seconds":options.progress_interval_seconds,"compatibility_proven":false})).err();
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

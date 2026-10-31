//! Bounded native Steam presence probe: logs on to Steam as an anonymous
//! ICARUS game server, lists one authored advert for at most `--seconds`,
//! answers A2S on `--query-bind`, then logs off. It opens no game port and
//! admits no player; it shows whether Steam lists the native server
//! (`GetServersAtAddress`) and what its query port answers.
//!
//! Stdout carries one JSON summary: the assigned Steam ID, CM endpoint,
//! status replies and counts.
use std::{
    net::{Ipv4Addr, SocketAddr, UdpSocket},
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, ensure};
use clap::Parser;
use icarus_bevy::{
    experimental::release::ReleasePin,
    steam_fetch::net::{self, HttpTimeouts, Tls},
    steam_presence::{
        Advert, PresenceHandles, RegistrationReport, RegistrationTiming, keep_listed,
        logon_game_server, serve_queries, version_tag,
    },
};
use serde_json::json;

#[derive(Parser)]
#[command(about = "List an authored native ICARUS advert on Steam for a bounded time")]
struct Cli {
    /// Advertised server name (no commas or colons).
    #[arg(long)]
    name: String,
    /// Advertised game port; nothing listens on it.
    #[arg(long, default_value_t = 17_777)]
    game_port: u16,
    /// Address the A2S responder binds; its port is the advertised query port.
    #[arg(long, default_value = "0.0.0.0:27015")]
    query_bind: SocketAddr,
    /// Private IPv4 address reported at logon.
    #[arg(long, default_value = "127.0.0.1")]
    private_ip: Ipv4Addr,
    /// Release pin whose project version gives the `G_s` tag.
    #[arg(long, default_value = "config/release.toml")]
    release: PathBuf,
    /// Override the pinned project version (for a newer public build).
    #[arg(long)]
    project_version: Option<String>,
    /// Advertised prospect identifier.
    #[arg(long)]
    prospect: Option<String>,
    #[arg(long, default_value_t = 1)]
    max_players: u8,
    /// Listing lifetime.
    #[arg(long, default_value_t = 120, value_parser = clap::value_parser!(u64).range(30..=1_800))]
    seconds: u64,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let project_version = match cli.project_version {
        Some(version) => version,
        None => ReleasePin::load(&cli.release)?.project_version,
    };
    ensure!(cli.query_bind.is_ipv4(), "the query responder binds IPv4");
    // Every local check precedes the Steam logon, so a bad argument costs no
    // session.
    let version_tag = version_tag(&project_version)?;
    let query = UdpSocket::bind(cli.query_bind).context("bind the query port")?;
    let mut advert = Advert {
        server_steam_id: 0,
        name: cli.name,
        map: String::new(),
        game_port: cli.game_port,
        query_port: cli.query_bind.port(),
        max_players: cli.max_players,
        players: Vec::new(),
        version_tag,
        prospect: cli.prospect,
        prospect_info: None,
    };
    advert.validate()?;
    let tls = Tls::from_native_roots()?;
    let agent = net::http_agent(&tls, &HttpTimeouts::default());
    let timing = RegistrationTiming::default();
    let (mut session, logon, endpoint) = logon_game_server(&tls, &agent, cli.private_ip, timing)?;
    advert.server_steam_id = logon.steam_id;
    let handles = PresenceHandles {
        advert: Arc::new(Mutex::new(advert)),
        stop: Arc::new(AtomicBool::new(false)),
    };
    let responder = {
        let handles = handles.clone();
        thread::spawn(move || serve_queries(&query, &handles.advert, &handles.stop))
    };
    let stop = handles.stop.clone();
    ctrlc::set_handler(move || stop.store(true, Ordering::Relaxed)).ok();
    let mut report = RegistrationReport {
        steam_id: logon.steam_id,
        endpoint,
        ..RegistrationReport::default()
    };
    let listed = keep_listed(
        &mut session,
        &handles.advert,
        &handles.stop,
        Instant::now() + Duration::from_secs(cli.seconds),
        timing,
        &mut report,
    );
    handles.stop.store(true, Ordering::Relaxed);
    session.close();
    let answered = responder
        .join()
        .map_err(|_| anyhow::anyhow!("query responder panicked"))??;
    listed?;
    println!(
        "{}",
        json!({
            "steam_id": report.steam_id.to_string(),
            "endpoint": report.endpoint,
            "status_replies": report.status_replies,
            "secure": report.secure,
            "updates_sent": report.updates_sent,
            "query_datagrams_answered": answered,
        })
    );
    Ok(())
}

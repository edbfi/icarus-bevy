//! Native Steam presence: what a server browser lists for this server and
//! how it gets there.
//!
//! One [`Advert`] feeds both the Steam master listing (`CMsgGameServerData`
//! sent by an anonymous game-server logon over the CM WebSocket) and the A2S
//! answers on the query port, so the two cannot disagree. Its ICARUS session
//! keys follow the original 3.0.34 server's observed advertisement (F-1002)
//! with `CONMETHOD=Direct`, the only method with which a stock client
//! connects to the advertised address instead of Steam P2P (F-1003). Values
//! whose meaning is unqualified (`SESSIONFLAGS`, `D_s`, `L_s`, the `-x-x`
//! version suffix) are carried as observed, not interpreted.
//!
//! The query responder answers every request with a challenge first, as the
//! original does, so an unverified source only ever receives a 9-byte reply.

mod prospect;
pub use prospect::encode_prospect_info;

use std::{
    net::{Ipv4Addr, SocketAddr, UdpSocket},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail, ensure};
use prost::Message as _;
use sha2::{Digest, Sha256};

use crate::{
    experimental::server_query::{
        Info, MAX_DATAGRAM, Player, Request, Response, Rule, encode_response, parse_request,
        split_response,
    },
    steam_fetch::{
        cm::{CmLimits, CmSession, CmTimeouts, Logon},
        frame::emsg,
        net::{self, Tls},
        proto::{GameServerData, GameServerPlayer, GsServerType, GsStatusReply},
    },
};

/// The ICARUS game application clients run and browse for (F-1002).
pub const ICARUS_APP_ID: u32 = 1_149_460;
/// The session flags the original advertised (F-1002); meaning unqualified.
pub const OBSERVED_SESSION_FLAGS: u32 = 683;
/// `EServerFlags`.
pub const SERVER_FLAG_ACTIVE: u32 = 1;
pub const SERVER_FLAG_SECURE: u32 = 2;
pub const SERVER_FLAG_DEDICATED: u32 = 4;
pub const SERVER_FLAG_LINUX: u32 = 8;
/// Longest server, owner or prospect name accepted.
pub const MAX_NAME_BYTES: usize = 63;

/// A player the advert lists.
#[derive(Clone, Debug, PartialEq)]
pub struct AdvertPlayer {
    pub steam_id: u64,
    pub name: String,
    pub connected_seconds: f32,
}

/// Everything a server browser learns about this server.
#[derive(Clone, Debug, PartialEq)]
pub struct Advert {
    pub server_steam_id: u64,
    pub name: String,
    pub map: String,
    pub game_port: u16,
    pub query_port: u16,
    pub max_players: u8,
    pub players: Vec<AdvertPlayer>,
    pub version_tag: String,
    /// Prospect identifier of the loaded world, if any.
    pub prospect: Option<String>,
    /// Base64 `ProspectInfo_s` rule, when one has been composed.
    pub prospect_info: Option<String>,
}

/// The `G_s` tag for a project version: its numeric prefix plus `-x-x`, as
/// the original advertised `3.0.34.159385-rel-DangerousHorizons` (F-1002).
pub fn version_tag(project_version: &str) -> Result<String> {
    let numeric = project_version
        .split('-')
        .next()
        .filter(|prefix| {
            !prefix.is_empty() && prefix.bytes().all(|b| b.is_ascii_digit() || b == b'.')
        })
        .context("project version lacks a numeric prefix")?;
    Ok(format!("{numeric}-x-x"))
}

fn check_name(what: &str, value: &str) -> Result<()> {
    ensure!(
        !value.is_empty() && value.len() <= MAX_NAME_BYTES,
        "{what} must be 1..={MAX_NAME_BYTES} bytes"
    );
    ensure!(
        value
            .chars()
            .all(|c| !c.is_control() && c != ',' && c != ':'),
        "{what} must not hold control characters, commas or colons"
    );
    Ok(())
}

impl Advert {
    /// Reject values that would corrupt the comma/colon key lists.
    pub fn validate(&self) -> Result<()> {
        check_name("server name", &self.name)?;
        check_name("version tag", &self.version_tag)?;
        if let Some(prospect) = &self.prospect {
            check_name("prospect", prospect)?;
        }
        ensure!(
            self.map.chars().all(|c| !c.is_control()),
            "map must not hold control characters"
        );
        ensure!(
            self.players.len() <= usize::from(self.max_players),
            "more players than the advertised maximum"
        );
        ensure!(
            self.players.iter().all(|player| {
                player.name.len() <= MAX_NAME_BYTES
                    && !player.name.chars().any(char::is_control)
                    && player.connected_seconds.is_finite()
            }),
            "player names must be at most {MAX_NAME_BYTES} bytes without control characters"
        );
        if let Some(info) = &self.prospect_info {
            ensure!(
                info.len() <= 4_096
                    && info
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"+/=".contains(&b)),
                "prospect info must be base64 of at most 4096 bytes"
            );
        }
        Ok(())
    }

    /// Session keys shared by the keywords and the master game data.
    fn session_keys(&self) -> Vec<(String, String)> {
        let mut keys = vec![
            ("OWNINGID".to_owned(), self.server_steam_id.to_string()),
            ("OWNINGNAME".to_owned(), self.name.clone()),
            (
                "SESSIONFLAGS".to_owned(),
                OBSERVED_SESSION_FLAGS.to_string(),
            ),
        ];
        if let Some(prospect) = &self.prospect {
            keys.push(("P_s".to_owned(), format!("_{prospect}")));
        }
        keys.push(("G_s".to_owned(), self.version_tag.clone()));
        keys
    }

    fn connection_keys(&self) -> Vec<(String, String)> {
        vec![
            ("CONMETHOD".to_owned(), "Direct".to_owned()),
            ("P2PADDR".to_owned(), "0.0.0.0".to_owned()),
            ("P2PPORT".to_owned(), self.game_port.to_string()),
        ]
    }

    /// The A2S keywords and master `game_type`.
    pub fn keywords(&self) -> String {
        std::iter::once(("BUILDID".to_owned(), "0".to_owned()))
            .chain(self.session_keys())
            .map(|(key, value)| format!("{key}:{value}"))
            .collect::<Vec<_>>()
            .join(",")
    }

    /// The master `game_data`: session then connection keys.
    pub fn game_data(&self) -> String {
        self.session_keys()
            .into_iter()
            .chain(self.connection_keys())
            .map(|(key, value)| format!("{key}:{value}"))
            .collect::<Vec<_>>()
            .join(",")
    }

    /// The A2S rules in the original's observed order (F-1002).
    pub fn rules(&self) -> Vec<Rule> {
        let mut rules = vec![
            ("CONMETHOD", "Direct".to_owned()),
            ("D_s", "1".to_owned()),
            ("G_s", self.version_tag.clone()),
            ("L_s", "1".to_owned()),
            ("OWNINGID", self.server_steam_id.to_string()),
            ("OWNINGNAME", self.name.clone()),
            ("P2PADDR", "0.0.0.0".to_owned()),
            ("P2PPORT", self.game_port.to_string()),
        ];
        if let Some(info) = &self.prospect_info {
            rules.push(("ProspectInfo_s", info.clone()));
        }
        if let Some(prospect) = &self.prospect {
            rules.push(("P_s", format!("_{prospect}")));
        }
        rules.push(("SESSIONFLAGS", OBSERVED_SESSION_FLAGS.to_string()));
        rules
            .into_iter()
            .map(|(key, value)| Rule {
                key: key.as_bytes().to_vec(),
                value: value.into_bytes(),
            })
            .collect()
    }

    /// The A2S_INFO answer.
    pub fn info(&self) -> Info {
        Info {
            protocol: 17,
            name: self.name.as_bytes().to_vec(),
            map: self.map.as_bytes().to_vec(),
            folder: b"Icarus".to_vec(),
            game: b"Icarus".to_vec(),
            app_id: 0,
            players: u8::try_from(self.players.len()).unwrap_or(u8::MAX),
            max_players: self.max_players,
            bots: 0,
            server_type: b'd',
            environment: b'l',
            visibility: 0,
            vac: 1,
            version: b"0.0.0.1".to_vec(),
            port: Some(self.game_port),
            steam_id: Some(self.server_steam_id),
            spectator: None,
            keywords: Some(self.keywords().into_bytes()),
            game_id: Some(u64::from(ICARUS_APP_ID)),
        }
    }

    /// The A2S_PLAYER answer.
    pub fn players(&self) -> Vec<Player> {
        self.players
            .iter()
            .zip(0u8..)
            .map(|(player, index)| Player {
                index,
                name: player.name.as_bytes().to_vec(),
                score: 0,
                duration: player.connected_seconds,
            })
            .collect()
    }

    /// The `GSServerType` this server sends after logon.
    pub fn server_type(&self) -> GsServerType {
        GsServerType {
            app_id_served: Some(ICARUS_APP_ID),
            flags: Some(
                SERVER_FLAG_ACTIVE | SERVER_FLAG_SECURE | SERVER_FLAG_DEDICATED | SERVER_FLAG_LINUX,
            ),
            game_port: Some(u32::from(self.game_port)),
            game_dir: Some("Icarus".to_owned()),
            game_version: Some("0.0.0.1".to_owned()),
            game_query_port: Some(u32::from(self.query_port)),
        }
    }

    /// The master listing details at `revision`.
    pub fn game_server_data(&self, revision: u32) -> GameServerData {
        GameServerData {
            revision: Some(revision),
            query_port: Some(u32::from(self.query_port)),
            game_port: Some(u32::from(self.game_port)),
            spectator_port: Some(0),
            server_name: Some(self.name.clone()),
            game_description: Some("Icarus".to_owned()),
            app_id: Some(ICARUS_APP_ID),
            gamedir: Some("Icarus".to_owned()),
            version: Some("0.0.0.1".to_owned()),
            product: Some("Icarus".to_owned()),
            region: Some(String::new()),
            players: self
                .players
                .iter()
                .map(|player| GameServerPlayer {
                    steam_id: Some(player.steam_id),
                })
                .collect(),
            max_players: Some(u32::from(self.max_players)),
            bot_count: Some(0),
            password: Some(false),
            secure: Some(true),
            dedicated: Some(true),
            os: Some("l".to_owned()),
            game_data: Some(self.game_data()),
            game_type: Some(self.keywords()),
            map: Some(self.map.clone()),
        }
    }
}

/// Seconds one challenge stays valid (current or previous window).
const CHALLENGE_WINDOW_SECONDS: u64 = 30;

/// Answers A2S requests from one shared advert; challenges bind the source
/// address to a time window under a per-process secret.
pub struct QueryResponder {
    secret: [u8; 32],
    started: Instant,
    split_id: i32,
}

impl QueryResponder {
    pub fn new() -> Result<Self> {
        let mut secret = [0u8; 32];
        getrandom::fill(&mut secret)
            .map_err(|error| anyhow::anyhow!("challenge secret: {error}"))?;
        Ok(Self::with_secret(secret, Instant::now()))
    }

    pub fn with_secret(secret: [u8; 32], started: Instant) -> Self {
        Self {
            secret,
            started,
            split_id: 0,
        }
    }

    fn challenge(&self, from: SocketAddr, window: u64) -> i32 {
        let mut hash = Sha256::new();
        hash.update(self.secret);
        hash.update(from.ip().to_string().as_bytes());
        hash.update(window.to_le_bytes());
        let digest = hash.finalize();
        // Uniform over non-negative values, so never "request a challenge".
        i32::from_le_bytes([digest[0], digest[1], digest[2], digest[3] & 0x7f])
    }

    fn window(&self, now: Instant) -> u64 {
        now.saturating_duration_since(self.started).as_secs() / CHALLENGE_WINDOW_SECONDS
    }

    fn valid(&self, from: SocketAddr, challenge: i32, now: Instant) -> bool {
        let window = self.window(now);
        challenge == self.challenge(from, window)
            || (window > 0 && challenge == self.challenge(from, window - 1))
    }

    /// Decide what `datagram` from `from` earns without touching the advert:
    /// nothing (malformed), a fresh challenge, or a verified request.
    pub fn admit(&self, from: SocketAddr, datagram: &[u8], now: Instant) -> Result<Admission> {
        let Ok(request) = parse_request(datagram) else {
            return Ok(Admission::Ignore);
        };
        let challenge = match request {
            Request::Info { challenge } => challenge,
            Request::Players { challenge } | Request::Rules { challenge } => Some(challenge),
        };
        if challenge.is_some_and(|value| self.valid(from, value, now)) {
            return Ok(Admission::Answer(request));
        }
        let fresh = self.challenge(from, self.window(now));
        Ok(Admission::Reply(vec![encode_response(
            &Response::Challenge(fresh),
        )?]))
    }

    /// The datagrams answering a verified request.
    pub fn answer_request(&mut self, advert: &Advert, request: Request) -> Result<Vec<Vec<u8>>> {
        let response = match request {
            Request::Info { .. } => Response::Info(advert.info()),
            Request::Players { .. } => Response::Players(advert.players()),
            Request::Rules { .. } => Response::Rules(advert.rules()),
        };
        let message = encode_response(&response)?;
        self.split_id = self.split_id.wrapping_add(1) & i32::MAX;
        split_response(&message, self.split_id, MAX_DATAGRAM - 12)
    }

    /// The datagrams answering `datagram` from `from`; malformed requests get
    /// none.
    pub fn answer(
        &mut self,
        advert: &Advert,
        from: SocketAddr,
        datagram: &[u8],
        now: Instant,
    ) -> Result<Vec<Vec<u8>>> {
        match self.admit(from, datagram, now)? {
            Admission::Ignore => Ok(Vec::new()),
            Admission::Reply(replies) => Ok(replies),
            Admission::Answer(request) => self.answer_request(advert, request),
        }
    }
}

/// What a query datagram earns.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Admission {
    Ignore,
    Reply(Vec<Vec<u8>>),
    Answer(Request),
}

/// Serve A2S on `socket` until `stop` is set; returns the datagrams answered.
/// A request that cannot be answered or sent is dropped; only a failing
/// socket read or a poisoned advert lock ends the service. The advert is copied only for verified
/// requests.
pub fn serve_queries(socket: &UdpSocket, advert: &Mutex<Advert>, stop: &AtomicBool) -> Result<u64> {
    socket.set_read_timeout(Some(Duration::from_millis(250)))?;
    let mut responder = QueryResponder::new()?;
    let mut buffer = [0u8; MAX_DATAGRAM + 1];
    let mut answered = 0;
    while !stop.load(Ordering::Relaxed) {
        let (size, from) = match socket.recv_from(&mut buffer) {
            Ok(received) => received,
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                continue;
            }
            Err(error) => return Err(error).context("query socket"),
        };
        if size > MAX_DATAGRAM {
            continue;
        }
        let replies = match responder.admit(from, &buffer[..size], Instant::now()) {
            Ok(Admission::Ignore) | Err(_) => continue,
            Ok(Admission::Reply(replies)) => replies,
            Ok(Admission::Answer(request)) => {
                let snapshot = advert
                    .lock()
                    .map_err(|_| anyhow::anyhow!("advert lock poisoned"))?
                    .clone();
                match responder.answer_request(&snapshot, request) {
                    Ok(replies) => replies,
                    Err(_) => continue,
                }
            }
        };
        for reply in replies {
            if socket.send_to(&reply, from).is_ok() {
                answered += 1;
            }
        }
    }
    Ok(answered)
}

/// How a registration ended.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RegistrationReport {
    pub steam_id: u64,
    pub endpoint: String,
    pub status_replies: u32,
    pub secure: Option<bool>,
    pub updates_sent: u32,
}

/// Bounds of one registration.
#[derive(Clone, Copy, Debug)]
pub struct RegistrationTiming {
    /// Interval between listing refreshes.
    pub refresh: Duration,
    /// Most CM endpoints tried for the logon.
    pub endpoints: usize,
}

impl Default for RegistrationTiming {
    fn default() -> Self {
        Self {
            refresh: Duration::from_secs(60),
            endpoints: 3,
        }
    }
}

/// Log on as an anonymous game server and return the session with the
/// Steam ID the CM assigned.
pub fn logon_game_server(
    tls: &Tls,
    agent: &ureq::Agent,
    private_ipv4: Ipv4Addr,
    timing: RegistrationTiming,
) -> Result<(CmSession, Logon, String)> {
    let endpoints = net::cm_endpoints(agent).context("CM directory")?;
    let mut failures = Vec::new();
    for endpoint in endpoints.iter().take(timing.endpoints) {
        let attempt = CmSession::connect(
            endpoint,
            tls.client.clone(),
            CmTimeouts::default(),
            CmLimits {
                max_message: 1024 * 1024,
                max_unzipped: 4 * 1024 * 1024,
            },
        )
        .and_then(|mut session| {
            let logon = session.logon_anonymous_game_server(ICARUS_APP_ID, private_ipv4)?;
            Ok((session, logon))
        });
        match attempt {
            Ok((session, logon)) => return Ok((session, logon, endpoint.clone())),
            Err(error) => failures.push(format!("{endpoint}: {error:#}")),
        }
    }
    bail!(
        "no CM accepted an anonymous game-server logon: {}",
        failures.join("; ")
    )
}

/// Keep `advert` listed until `stop` is set or `deadline` passes, then log
/// off. The advert's Steam ID must already be the session's.
pub fn keep_listed(
    session: &mut CmSession,
    advert: &Mutex<Advert>,
    stop: &AtomicBool,
    deadline: Instant,
    timing: RegistrationTiming,
    report: &mut RegistrationReport,
) -> Result<()> {
    let snapshot = |advert: &Mutex<Advert>| -> Result<Advert> {
        Ok(advert
            .lock()
            .map_err(|_| anyhow::anyhow!("advert lock poisoned"))?
            .clone())
    };
    let first = snapshot(advert)?;
    first.validate()?;
    session.send(emsg::GS_SERVER_TYPE, &first.server_type().encode_to_vec())?;
    let mut revision = 1;
    // Only what the master sees decides a refresh; the revision is excluded.
    let mut listed = first.game_server_data(0);
    session.send(
        emsg::AM_GAME_SERVER_UPDATE,
        &first.game_server_data(revision).encode_to_vec(),
    )?;
    report.updates_sent = 1;
    let mut last_update = Instant::now();
    while !stop.load(Ordering::Relaxed) && Instant::now() < deadline {
        for packet in session.poll(Duration::from_secs(1))? {
            if packet.emsg == emsg::GS_STATUS_REPLY {
                report.status_replies += 1;
                if let Some(secure) = GsStatusReply::decode(packet.body.as_slice())
                    .ok()
                    .and_then(|reply| reply.is_secure)
                {
                    report.secure = Some(secure);
                }
            }
        }
        let current = snapshot(advert)?;
        let data = current.game_server_data(0);
        if data != listed || last_update.elapsed() >= timing.refresh {
            current.validate()?;
            revision += 1;
            session.send(
                emsg::AM_GAME_SERVER_UPDATE,
                &current.game_server_data(revision).encode_to_vec(),
            )?;
            report.updates_sent += 1;
            listed = data;
            last_update = Instant::now();
        }
    }
    Ok(())
}

/// Shared handles for a presence that runs beside the server.
#[derive(Clone)]
pub struct PresenceHandles {
    pub advert: Arc<Mutex<Advert>>,
    pub stop: Arc<AtomicBool>,
}

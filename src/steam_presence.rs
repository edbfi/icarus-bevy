//! Native Steam presence: what a server browser lists for this server and
//! how it gets there.
//!
//! One [`Advert`] feeds both the Steam master listing (`CMsgGameServerData`
//! sent by an anonymous game-server logon over the CM WebSocket) and the A2S
//! answers on the query port, so the two cannot disagree. Its ICARUS session
//! keys follow the original 3.0.34 server's observed advertisement (F-1002)
//! with `CONMETHOD=Direct`, the only method with which a stock client
//! connects to the advertised address instead of Steam P2P (F-1003). Values
//! whose meaning is unqualified (`SESSIONFLAGS`, the `-x-x` version suffix)
//! are carried as observed, not interpreted.
//!
//! `D_s` ("dedicated") and `L_s` ("locked") are presence flags: the client
//! sets its session's `DedicatedServer` and `Locked` flags when the key is
//! present, whatever its value, and a locked listing asks for a password.
//! The 26425113 server's `MakeSessionExtraSettingsFromProspect` adds `D` =
//! `"1"` always and `L` = `"1"` only when its `JoinPassword` is non-empty,
//! so `L_s` is sent only for a password-protected advert and never as
//! `"0"` (F-1161, F-1162). The Steam `password` field, the server flags and
//! the A2S visibility byte do not drive that flag and stay unchanged.
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

/// Authenticated hosting membership for the CM only. Diagnostics expose its
/// size, never its contents; it has no serialization implementation.
#[derive(Clone, PartialEq, Eq, Default)]
pub struct MasterMembership(Vec<u64>);

impl std::fmt::Debug for MasterMembership {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MasterMembership")
            .field("count", &self.len())
            .finish()
    }
}

impl MasterMembership {
    pub(crate) fn from_authenticated(ids: impl IntoIterator<Item = u64>) -> Result<Self> {
        let mut ids: Vec<_> = ids
            .into_iter()
            .take(crate::config::MAX_NETWORK_PLAYERS as usize + 1)
            .collect();
        ensure!(
            ids.len() <= crate::config::MAX_NETWORK_PLAYERS as usize,
            "hosting membership exceeds peer bound"
        );
        ensure!(
            ids.iter().all(|id| *id != 0),
            "invalid hosting membership identity"
        );
        ids.sort_unstable();
        ids.dedup();
        Ok(Self(ids))
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
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
    /// Counts-only A2S occupancy; never publishes names or Steam IDs.
    pub occupancy: Option<u8>,
    /// Hosting-only CM identities, separate from named A2S player records.
    /// `None` preserves the existing named-player conversion for legacy users.
    pub master_membership: Option<MasterMembership>,
    pub version_tag: String,
    /// Prospect identifier of the loaded world, if any.
    pub prospect: Option<String>,
    /// Base64 `ProspectInfo_s` rule, when one has been composed.
    pub prospect_info: Option<String>,
    /// Whether joining needs a password: adds the `L_s` rule, which makes a
    /// stock client ask for one. The password itself is never advertised.
    pub password_protected: bool,
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
        if let Some(membership) = &self.master_membership {
            ensure!(
                membership.len() <= usize::from(self.max_players)
                    && self.occupancy.map(usize::from) == Some(membership.len())
                    && self.players.is_empty(),
                "hosting membership and occupancy disagree"
            );
        }
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
            self.players.len() <= usize::from(self.max_players)
                && self.occupancy.is_none_or(|count| count <= self.max_players),
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

    /// The A2S rules in the original's observed order (F-1002); `L_s` only
    /// when [`Self::password_protected`] (module docs).
    pub fn rules(&self) -> Vec<Rule> {
        let mut rules = vec![
            ("CONMETHOD", "Direct".to_owned()),
            ("D_s", "1".to_owned()),
            ("G_s", self.version_tag.clone()),
        ];
        if self.password_protected {
            rules.push(("L_s", "1".to_owned()));
        }
        rules.extend([
            ("OWNINGID", self.server_steam_id.to_string()),
            ("OWNINGNAME", self.name.clone()),
            ("P2PADDR", "0.0.0.0".to_owned()),
            ("P2PPORT", self.game_port.to_string()),
        ]);
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
            players: self
                .occupancy
                .unwrap_or_else(|| u8::try_from(self.players.len()).unwrap_or(u8::MAX)),
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
            players: if let Some(membership) = &self.master_membership {
                membership
                    .0
                    .iter()
                    .map(|id| GameServerPlayer {
                        steam_id: Some(*id),
                    })
                    .collect()
            } else {
                self.players
                    .iter()
                    .map(|player| GameServerPlayer {
                        steam_id: Some(player.steam_id),
                    })
                    .collect()
            },
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
    serve_prepared_queries(socket, advert, stop, QueryResponder::new()?)
}

pub(crate) fn serve_prepared_queries(
    socket: &UdpSocket,
    advert: &Mutex<Advert>,
    stop: &AtomicBool,
    mut responder: QueryResponder,
) -> Result<u64> {
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
            match session.logon_anonymous_game_server(ICARUS_APP_ID, private_ipv4) {
                Ok(logon) => Ok((session, logon)),
                Err(error) => {
                    session.close_bounded(Duration::from_secs(1));
                    Err(error)
                }
            }
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

#[cfg(test)]
mod membership_tests {
    use super::*;
    const A: u64 = 0x0110_0001_1234_5678;
    const B: u64 = 0x0110_0001_8765_4321;

    fn advert(ids: &[u64]) -> Advert {
        let membership = MasterMembership::from_authenticated(ids.iter().copied()).unwrap();
        Advert {
            server_steam_id: 0,
            name: "Authored host".into(),
            map: "/Authored/Maps/Workshop".into(),
            game_port: 17777,
            query_port: 27015,
            max_players: 8,
            players: Vec::new(),
            occupancy: Some(membership.len() as u8),
            master_membership: Some(membership),
            version_tag: "1.0-x-x".into(),
            prospect: None,
            prospect_info: None,
            password_protected: true,
        }
    }

    #[test]
    fn synthetic_membership_uses_existing_fixed64_player_records_only() {
        let advert = advert(&[B, A, B]);
        advert.validate().unwrap();
        let data = advert.game_server_data(7);
        assert_eq!(data.revision, Some(7));
        assert_eq!(
            data.players
                .iter()
                .map(|player| player.steam_id)
                .collect::<Vec<_>>(),
            [Some(A), Some(B)]
        );
        let encoded = data.encode_to_vec();
        let decoded = GameServerData::decode(encoded.as_slice()).unwrap();
        assert_eq!(decoded, data);
        for id in [A, B] {
            let record = GameServerPlayer { steam_id: Some(id) }.encode_to_vec();
            let mut expected = vec![0x09]; // player field 1, fixed64
            expected.extend(id.to_le_bytes());
            assert_eq!(record, expected);
            let mut outer = vec![0x5a, 9]; // existing field 11, length-delimited
            outer.extend(record);
            assert!(encoded.windows(outer.len()).any(|part| part == outer));
        }
        assert_eq!(advert.info().players, 2);
        assert!(advert.players().is_empty());
    }

    #[test]
    fn membership_changes_drive_the_existing_revision_refresh_predicate() {
        let empty = advert(&[]).game_server_data(0);
        let joined = advert(&[B, A]).game_server_data(0);
        let reordered = advert(&[A, B, A]).game_server_data(0);
        let departed = advert(&[A]).game_server_data(0);
        let due = |current: &GameServerData, previous: &GameServerData| {
            crate::steam_service::presence_refresh_due(
                current,
                previous,
                Duration::ZERO,
                Duration::from_secs(60),
            )
        };
        assert!(due(&joined, &empty));
        assert!(!due(&reordered, &joined));
        assert!(due(&departed, &joined));
        assert!(due(&advert(&[B]).game_server_data(0), &departed));
        assert!(due(&empty, &departed));
        assert!(crate::steam_service::presence_refresh_due(
            &empty,
            &empty,
            Duration::from_secs(60),
            Duration::from_secs(60)
        ));
    }

    #[test]
    fn membership_diagnostics_and_public_query_payloads_never_contain_ids() {
        let advert = advert(&[A, B]);
        let debug = format!("{advert:?} {:?}", advert.master_membership);
        let public = [
            encode_response(&Response::Info(advert.info())).unwrap(),
            encode_response(&Response::Players(advert.players())).unwrap(),
            encode_response(&Response::Rules(advert.rules())).unwrap(),
        ];
        for id in [A, B] {
            assert!(!debug.contains(&id.to_string()));
            assert!(!debug.contains(&format!("{id:x}")));
            for payload in &public {
                assert!(!payload.windows(8).any(|bytes| bytes == id.to_le_bytes()));
                assert!(!String::from_utf8_lossy(payload).contains(&id.to_string()));
            }
        }
    }

    #[test]
    fn membership_is_bounded_and_validation_rejects_split_counts() {
        assert!(MasterMembership::from_authenticated(1..=65).is_err());
        assert!(MasterMembership::from_authenticated([0]).is_err());
        let mut advert = advert(&[A]);
        advert.occupancy = Some(0);
        assert!(advert.validate().is_err());
        advert.occupancy = Some(1);
        advert.players.push(AdvertPlayer {
            steam_id: B,
            name: "Authored legacy".into(),
            connected_seconds: 0.0,
        });
        assert!(advert.validate().is_err());
    }
}

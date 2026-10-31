//! Bounded A2S query of a server on this host only: sends `A2S_INFO`,
//! `A2S_PLAYER` and `A2S_RULES` to a loopback query port, answers challenges,
//! reassembles split responses, and records every datagram. Used to observe
//! what a recorded case's server advertises; never pointed at another host.
//!
//! Stdout carries counts only. Every datagram (hex, direction, milliseconds
//! since start) and the decoded answers go to the detail file, which must lie
//! under this checkout's `work/` directory because advertised values can
//! identify a server or its players.
use std::{
    fs::File,
    io::{BufWriter, Write},
    net::{SocketAddr, UdpSocket},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail, ensure};
use clap::Parser;
use icarus_bevy::experimental::server_query::{
    Info, MAX_DATAGRAM, REQUEST_CHALLENGE, Request, Response, SIMPLE_HEADER, SPLIT_HEADER,
    SplitAssembler, datagram_header, encode_request, parse_response, parse_split,
};
use serde_json::{Value, json};

#[derive(Parser)]
#[command(about = "Query a loopback server's A2S answers into a work/ detail file")]
struct Cli {
    /// Loopback query address, for example 127.0.0.1:27015.
    #[arg(long)]
    target: SocketAddr,
    /// Detail JSONL; must lie under this checkout's `work/`.
    #[arg(long)]
    out: PathBuf,
    /// Receive timeout per datagram, in milliseconds.
    #[arg(long, default_value_t = 1_500, value_parser = clap::value_parser!(u64).range(100..=5_000))]
    timeout_ms: u64,
    /// Attempts per query kind when nothing arrives.
    #[arg(long, default_value_t = 3, value_parser = clap::value_parser!(u8).range(1..=5))]
    attempts: u8,
}

/// Challenge rounds per query before giving up.
const MAX_CHALLENGES: usize = 2;
/// Split slices read for one answer before giving up.
const MAX_SLICES: usize = 64;

struct Session {
    socket: UdpSocket,
    target: SocketAddr,
    start: Instant,
    detail: BufWriter<File>,
    datagrams: usize,
}

impl Session {
    fn record(&mut self, direction: &str, kind: &str, bytes: &[u8]) -> Result<()> {
        self.datagrams += 1;
        let line = json!({
            "event": "datagram",
            "direction": direction,
            "query": kind,
            "ms": self.start.elapsed().as_millis(),
            "hex": hex(bytes),
        });
        writeln!(self.detail, "{line}")?;
        Ok(())
    }

    fn send(&mut self, kind: &str, request: &Request) -> Result<()> {
        let bytes = encode_request(request);
        self.record("sent", kind, &bytes)?;
        self.socket.send_to(&bytes, self.target)?;
        Ok(())
    }

    /// One whole message from the target, or `None` on timeout.
    fn receive(&mut self, kind: &str) -> Result<Option<Vec<u8>>> {
        let mut assembler = SplitAssembler::default();
        let mut buffer = [0u8; MAX_DATAGRAM + 1];
        for _ in 0..MAX_SLICES {
            let (size, from) = match self.socket.recv_from(&mut buffer) {
                Ok(received) => received,
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) =>
                {
                    return Ok(None);
                }
                Err(error) => return Err(error.into()),
            };
            if from != self.target {
                continue;
            }
            let datagram = &buffer[..size];
            self.record("received", kind, datagram)?;
            ensure!(
                size <= MAX_DATAGRAM,
                "datagram exceeds {MAX_DATAGRAM} bytes"
            );
            match datagram_header(datagram)? {
                SIMPLE_HEADER => return Ok(Some(datagram.to_vec())),
                SPLIT_HEADER => {
                    if let Some(message) = assembler.push(parse_split(datagram)?)? {
                        return Ok(Some(message));
                    }
                }
                other => bail!("unexpected datagram header {other}"),
            }
        }
        bail!("split answer exceeds {MAX_SLICES} slices")
    }

    /// Ask, answering up to [`MAX_CHALLENGES`] challenges; `None` if silent.
    fn query(&mut self, kind: &str, attempts: u8) -> Result<Option<Response>> {
        let mut challenge = None;
        let mut challenges = 0;
        let mut silent = 0;
        loop {
            let request = match kind {
                "info" => Request::Info { challenge },
                "players" => Request::Players {
                    challenge: challenge.unwrap_or(REQUEST_CHALLENGE),
                },
                _ => Request::Rules {
                    challenge: challenge.unwrap_or(REQUEST_CHALLENGE),
                },
            };
            self.send(kind, &request)?;
            let Some(message) = self.receive(kind)? else {
                silent += 1;
                if silent >= attempts {
                    return Ok(None);
                }
                continue;
            };
            match parse_response(&message)? {
                Response::Challenge(value) if challenges < MAX_CHALLENGES => {
                    challenges += 1;
                    challenge = Some(value);
                }
                Response::Challenge(_) => bail!("{kind} kept answering with challenges"),
                response => return Ok(Some(response)),
            }
        }
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn text(bytes: &[u8]) -> Value {
    json!({"text": String::from_utf8_lossy(bytes), "hex": hex(bytes)})
}

fn info_json(info: &Info) -> Value {
    json!({
        "protocol": info.protocol,
        "name": text(&info.name),
        "map": text(&info.map),
        "folder": text(&info.folder),
        "game": text(&info.game),
        "app_id": info.app_id,
        "players": info.players,
        "max_players": info.max_players,
        "bots": info.bots,
        "server_type": char::from(info.server_type).to_string(),
        "environment": char::from(info.environment).to_string(),
        "visibility": info.visibility,
        "vac": info.vac,
        "version": text(&info.version),
        "extra_data_flags": info.extra_data_flags(),
        "port": info.port,
        "steam_id": info.steam_id.map(|id| id.to_string()),
        "spectator": info.spectator.as_ref().map(|s| json!({"port": s.port, "name": text(&s.name)})),
        "keywords": info.keywords.as_deref().map(text),
        "game_id": info.game_id.map(|id| id.to_string()),
    })
}

fn response_json(response: &Response) -> Value {
    match response {
        Response::Challenge(value) => json!({"challenge": value}),
        Response::Info(info) => json!({"info": info_json(info)}),
        Response::Players(players) => json!({"players": players
            .iter()
            .map(|p| json!({"index": p.index, "name": text(&p.name), "score": p.score, "duration": p.duration}))
            .collect::<Vec<_>>()}),
        Response::Rules(rules) => json!({"rules": rules
            .iter()
            .map(|r| json!({"key": text(&r.key), "value": text(&r.value)}))
            .collect::<Vec<_>>()}),
    }
}

/// The detail file must stay under this checkout's `work/` directory.
fn check_detail_path(path: &Path) -> Result<()> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let canonical = parent
        .canonicalize()
        .context("detail directory must exist")?;
    let work_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("work")
        .canonicalize()
        .context("work directory must exist")?;
    ensure!(
        canonical.starts_with(&work_root),
        "detail output must lie under {}",
        work_root.display()
    );
    Ok(())
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    ensure!(
        cli.target.ip().is_loopback(),
        "a2s_query only queries loopback addresses"
    );
    check_detail_path(&cli.out)?;
    let bind: SocketAddr = if cli.target.is_ipv4() {
        "127.0.0.1:0".parse()?
    } else {
        "[::1]:0".parse()?
    };
    let socket = UdpSocket::bind(bind)?;
    socket.set_read_timeout(Some(Duration::from_millis(cli.timeout_ms)))?;
    let mut session = Session {
        socket,
        target: cli.target,
        start: Instant::now(),
        detail: BufWriter::new(
            File::create_new(&cli.out).with_context(|| format!("create {}", cli.out.display()))?,
        ),
        datagrams: 0,
    };
    let mut summary = serde_json::Map::new();
    for kind in ["info", "players", "rules"] {
        let outcome = session.query(kind, cli.attempts);
        let (line, count) = match &outcome {
            Ok(Some(response)) => {
                let count = match response {
                    Response::Players(players) => players.len(),
                    Response::Rules(rules) => rules.len(),
                    _ => 1,
                };
                (
                    json!({"event": "answer", "query": kind, "answer": response_json(response)}),
                    json!(count),
                )
            }
            Ok(None) => (json!({"event": "silent", "query": kind}), Value::Null),
            Err(error) => (
                json!({"event": "error", "query": kind, "error": format!("{error:#}")}),
                json!("error"),
            ),
        };
        writeln!(session.detail, "{line}")?;
        summary.insert(kind.to_owned(), count);
    }
    session.detail.flush()?;
    summary.insert("datagrams".to_owned(), json!(session.datagrams));
    println!("{}", Value::Object(summary));
    Ok(())
}

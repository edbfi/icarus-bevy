//! Run only via the approved offline Docker launcher, inside its network namespace.
use std::{
    collections::BTreeMap,
    net::UdpSocket,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, ensure};
use clap::Parser;
use icarus_bevy::experimental::{
    control_messages::{
        ArchiveText, ControlCodec, ControlMessage, NetIdType, NetIdValue, UniqueNetId,
    },
    handshake,
    packet::{BunchLimits, PacketConfig, Receiver},
    peer_lab::PeerLabConfig,
    probe_client::{ProbeClient, ProbeConfig, ReplyDecode},
};
use serde_json::json;

#[derive(Parser)]
struct Options {
    /// Echo the issued cookie and observe the stateless acknowledgement only.
    #[arg(long)]
    complete_stateless: bool,
    /// After the stateless exchange, send one reliable Hello carrying this
    /// network version with an empty token and record every reply.
    #[arg(long, conflicts_with = "complete_stateless")]
    hello: Option<u32>,
    /// After a Challenge, log in with synthetic ids until one is welcomed,
    /// then send Netspeed and Join and hold the connection this many seconds.
    #[arg(long, requires = "hello")]
    join: Option<u64>,
}

/// Channel-index ceiling for decoding replies; observation only.
const PROBE_MAX_CHANNELS: u32 = 32_767;

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn main() -> Result<()> {
    let options = Options::parse();
    ensure!(
        std::env::var("ICARUS_OFFLINE_PROBE").as_deref() == Ok("1"),
        "invoke only through the approved isolated reference launcher"
    );
    let socket = UdpSocket::bind("127.0.0.1:0")?;
    socket.connect("127.0.0.1:17777")?;
    if let (Some(version), Some(seconds)) = (options.hello, options.join) {
        drop(socket);
        return join(version, seconds);
    }
    if let Some(version) = options.hello {
        return hello(socket, version);
    }
    socket.set_read_timeout(Some(Duration::from_secs(2)))?;
    socket.set_write_timeout(Some(Duration::from_secs(2)))?;
    let request = handshake::initial_request();
    let mut response = [0_u8; 65_536];
    for attempt in 1..=3 {
        socket.send(&request)?;
        println!(
            "{}",
            json!({"event":"probe_send", "attempt":attempt, "peer":"127.0.0.1:17777", "local":socket.local_addr()?.to_string(),
            "bytes":request.len(), "hex":hex(&request)})
        );
        match socket.recv(&mut response) {
            Ok(size) => {
                println!(
                    "{}",
                    json!({"event":"probe_receive", "bytes":size, "hex":hex(&response[..size])})
                );
                let decoded = handshake::decode(&response[..size])
                    .context("response framing differs from candidate")?;
                ensure!(
                    decoded.timestamp > 0.0 && !decoded.restart,
                    "response is not a normal challenge candidate"
                );
                println!(
                    "{}",
                    json!({"event":"challenge_candidate", "timestamp":decoded.timestamp,
                    "secret_id":decoded.secret_id, "cookie_hex":hex(&decoded.cookie),
                    "cookie_echo_sent":false, "game_connection_established":false})
                );
                if options.complete_stateless {
                    let echo = handshake::encode(&decoded)?;
                    socket.send(&echo)?;
                    println!(
                        "{}",
                        json!({"event":"probe_cookie_response", "bytes":echo.len(), "hex":hex(&echo)})
                    );
                    let size = socket
                        .recv(&mut response)
                        .context("stateless acknowledgement timed out or failed")?;
                    println!(
                        "{}",
                        json!({"event":"probe_ack_receive", "bytes":size, "hex":hex(&response[..size])})
                    );
                    let acknowledgement = handshake::decode(&response[..size])?;
                    ensure!(
                        acknowledgement.timestamp == -1.0
                            && !acknowledgement.restart
                            && acknowledgement.secret_id
                            && acknowledgement.cookie == decoded.cookie,
                        "response differs from inspected stateless acknowledgement"
                    );
                    println!(
                        "{}",
                        json!({"event":"stateless_exchange_completed", "cookie_echo_sent":true,
                        "identity_material_sent":false, "control_messages_sent":false,
                        "game_connection_established":false})
                    );
                }
                return Ok(());
            }
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
                ) =>
            {
                println!("{}", json!({"event":"probe_timeout", "attempt":attempt}));
            }
            Err(error) => return Err(error.into()),
        }
    }
    anyhow::bail!("no response after three bounded initial requests")
}

/// Handshake, one Hello, and every reply, raw and decoded. The packet model is
/// this repository's own (no magic header, peer-lab limits); a reply it cannot
/// read is recorded, not an error.
fn probe_config() -> ProbeConfig {
    ProbeConfig {
        packet: PacketConfig {
            magic_bits: 0,
            receiver: Receiver::Server,
            // The original's channel limit is unknown; a joined server opens
            // channels past the lab's 32, so the probe decodes up to the
            // largest packed index the engine allocates by default.
            limits: BunchLimits {
                max_channels: PROBE_MAX_CHANNELS,
                ..PeerLabConfig::default().packet.packet.limits
            },
        },
        codec: ControlCodec::default(),
        handshake_timeout: Duration::from_secs(2),
        handshake_attempts: 3,
        reply_window: Duration::from_secs(5),
        max_datagrams: 16,
    }
}

fn hello(socket: UdpSocket, version: u32) -> Result<()> {
    let mut client = ProbeClient::connect(socket, probe_config())?;
    let handshake = client.handshake();
    println!(
        "{}",
        json!({"event":"stateless_exchange_completed", "challenge_hex":hex(&handshake.challenge_datagram),
        "ack_hex":hex(&handshake.ack_datagram), "identity_material_sent":false})
    );
    let report = client.hello(version, "")?;
    println!(
        "{}",
        json!({"event":"hello_sent", "version":version, "token_units":0,
        "packet_id":report.hello_packet_id, "hex":hex(&report.hello_datagram)})
    );
    for reply in &report.replies {
        let decode = match &reply.decode {
            ReplyDecode::Decoded { messages, notes } => json!({
                "messages": messages.iter().map(|message| {
                    let upgrade = match message {
                        ControlMessage::Upgrade { version } => Some(*version),
                        _ => None,
                    };
                    json!({"summary": message.summary(), "upgrade_version": upgrade})
                }).collect::<Vec<_>>(),
                "notes": notes,
            }),
            ReplyDecode::Undecoded(error) => json!({"undecoded": error}),
        };
        println!(
            "{}",
            json!({"event":"reply", "bytes":reply.bytes.len(), "hex":hex(&reply.bytes), "decode":decode})
        );
    }
    println!(
        "{}",
        json!({"event":"hello_done", "hello_acked":report.hello_acked, "replies":report.replies.len(),
        "login_sent":false, "game_connection_established":false})
    );
    Ok(())
}

fn text(value: &ArchiveText) -> String {
    String::from_utf16_lossy(value.units())
}

/// Synthetic login identities, tried in order: no real account data.
fn identities() -> Result<Vec<(&'static str, UniqueNetId)>> {
    let named = |kind: &str, value: NetIdValue| -> Result<UniqueNetId> {
        Ok(UniqueNetId {
            kind: NetIdType::Named(ArchiveText::text(kind)?),
            value,
        })
    };
    Ok(vec![
        (
            "null-text",
            named(
                "NULL",
                NetIdValue::Text(ArchiveText::text("IcarusProbe-0001")?),
            )?,
        ),
        (
            "default-text",
            UniqueNetId {
                kind: NetIdType::Default,
                value: NetIdValue::Text(ArchiveText::text("IcarusProbe-0001")?),
            },
        ),
        (
            "steam-encoded",
            named(
                "Steam",
                NetIdValue::Encoded {
                    odd: false,
                    bytes: 0x0110_0001_0000_0001_u64.to_le_bytes().to_vec(),
                },
            )?,
        ),
    ])
}

/// Hello, Login with each synthetic id until a Welcome, then Netspeed and
/// Join, then acknowledge everything and send keepalives for `seconds`.
fn join(version: u32, seconds: u64) -> Result<()> {
    for (label, unique_net_id) in identities()? {
        let socket = UdpSocket::bind("127.0.0.1:0")?;
        socket.connect("127.0.0.1:17777")?;
        let mut client = ProbeClient::connect(socket, probe_config())?;
        let report = client.hello(version, "")?;
        if !report
            .messages()
            .any(|message| matches!(message, ControlMessage::Challenge { .. }))
        {
            println!(
                "{}",
                json!({"event":"no_challenge", "identity":label, "replies":report.replies.len()})
            );
            return Ok(());
        }
        let login = ControlMessage::Login {
            response: ArchiveText::text("0")?,
            url: ArchiveText::text("?Name=IcarusProbe")?,
            unique_net_id,
            platform: ArchiveText::text("")?,
        };
        client.send_control(&[login], false)?;
        let collected = client.collect(Duration::from_secs(5), 64)?;
        let mut welcomed = false;
        for message in collected.messages() {
            let detail = match message {
                ControlMessage::Failure { message } => json!({"failure": text(message)}),
                ControlMessage::Welcome {
                    map_url,
                    game_name,
                    redirect_url,
                } => {
                    welcomed = true;
                    json!({"map_url": text(map_url), "game_name": text(game_name), "redirect_url": text(redirect_url)})
                }
                other => json!({"summary": other.summary()}),
            };
            println!(
                "{}",
                json!({"event":"login_reply", "identity":label, "message":message.name(), "detail":detail})
            );
        }
        if !welcomed {
            println!(
                "{}",
                json!({"event":"login_refused", "identity":label, "datagrams":collected.replies.len()})
            );
            continue;
        }
        client.send_control(
            &[
                ControlMessage::Netspeed { rate: 100_000 },
                ControlMessage::Join,
            ],
            false,
        )?;
        println!("{}", json!({"event":"join_sent", "identity":label}));
        return hold(&mut client, seconds);
    }
    println!("{}", json!({"event":"no_identity_welcomed"}));
    Ok(())
}

fn hold(client: &mut ProbeClient, seconds: u64) -> Result<()> {
    let start = Instant::now();
    let mut last_datagram = Instant::now();
    let mut last_report = Instant::now();
    let (mut datagrams, mut bytes, mut undecoded) = (0_u64, 0_u64, 0_u64);
    let mut control: BTreeMap<&'static str, u64> = BTreeMap::new();
    let mut notes: BTreeMap<String, u64> = BTreeMap::new();
    let mut first_undecoded = None;
    while start.elapsed() < Duration::from_secs(seconds) {
        let collected = client.collect(Duration::from_millis(200), 512)?;
        if !collected.replies.is_empty() {
            last_datagram = Instant::now();
        }
        for reply in &collected.replies {
            datagrams += 1;
            bytes += reply.bytes.len() as u64;
            match &reply.decode {
                ReplyDecode::Decoded {
                    messages,
                    notes: bunch_notes,
                } => {
                    for message in messages {
                        *control.entry(message.name()).or_default() += 1;
                    }
                    for note in bunch_notes {
                        // Keep only the note kind, e.g. "channel 3" or an error class.
                        let kind = note
                            .split_once(": ")
                            .map_or(note.as_str(), |(_, kind)| kind);
                        *notes.entry(kind.chars().take(48).collect()).or_default() += 1;
                    }
                }
                ReplyDecode::Undecoded(error) => {
                    undecoded += 1;
                    first_undecoded.get_or_insert_with(|| (error.clone(), hex(&reply.bytes)));
                }
            }
        }
        client.keepalive()?;
        if last_report.elapsed() >= Duration::from_secs(15) {
            last_report = Instant::now();
            println!(
                "{}",
                json!({"event":"holding", "seconds":start.elapsed().as_secs(), "datagrams":datagrams,
                "bytes":bytes, "undecoded":undecoded, "control":&control, "distinct_notes":notes.len()})
            );
        }
        if last_datagram.elapsed() >= Duration::from_secs(30) {
            println!(
                "{}",
                json!({"event":"server_silent", "seconds":start.elapsed().as_secs()})
            );
            break;
        }
    }
    let busiest: Vec<_> = {
        let mut entries: Vec<_> = notes.iter().collect();
        entries.sort_by(|a, b| b.1.cmp(a.1));
        entries.into_iter().take(24).collect()
    };
    println!(
        "{}",
        json!({"event":"hold_done", "seconds":start.elapsed().as_secs(), "datagrams":datagrams,
        "bytes":bytes, "undecoded":undecoded, "control":control, "notes":busiest,
        "first_undecoded":first_undecoded.map(|(error, hex)| json!({"error":error, "hex":hex}))})
    );
    Ok(())
}

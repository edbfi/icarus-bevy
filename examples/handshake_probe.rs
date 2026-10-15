//! Run only via the approved offline Docker launcher, inside its network namespace.
use std::{net::UdpSocket, time::Duration};

use anyhow::{Context, Result, ensure};
use clap::Parser;
use icarus_bevy::experimental::{
    control_messages::{ControlCodec, ControlMessage},
    handshake,
    packet::{PacketConfig, Receiver},
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
}

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
fn hello(socket: UdpSocket, version: u32) -> Result<()> {
    let config = ProbeConfig {
        packet: PacketConfig {
            magic_bits: 0,
            receiver: Receiver::Server,
            limits: PeerLabConfig::default().packet.packet.limits,
        },
        codec: ControlCodec::default(),
        handshake_timeout: Duration::from_secs(2),
        handshake_attempts: 3,
        reply_window: Duration::from_secs(5),
        max_datagrams: 16,
    };
    let mut client = ProbeClient::connect(socket, config)?;
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

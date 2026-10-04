//! Run only via the approved offline Docker launcher, inside its network namespace.
use std::{net::UdpSocket, time::Duration};

use anyhow::{Context, Result, ensure};
use icarus_bevy::experimental::handshake;
use serde_json::json;

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn main() -> Result<()> {
    ensure!(
        std::env::var("ICARUS_OFFLINE_PROBE").as_deref() == Ok("1"),
        "invoke only through the approved isolated reference launcher"
    );
    let socket = UdpSocket::bind("127.0.0.1:0")?;
    socket.connect("127.0.0.1:17777")?;
    socket.set_read_timeout(Some(Duration::from_secs(2)))?;
    socket.set_write_timeout(Some(Duration::from_secs(2)))?;
    let request = handshake::initial_request();
    let mut response = [0_u8; 65_536];
    for attempt in 1..=3 {
        socket.send(&request)?;
        println!(
            "{}",
            json!({"event":"probe_send", "attempt":attempt, "peer":"127.0.0.1:17777",
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

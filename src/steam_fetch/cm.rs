//! Anonymous CM session over a TLS WebSocket (`wss://<endpoint>/cmsocket/`).
//!
//! ClientHello, then ClientLogon as an anonymous user; heartbeats follow the
//! interval in ClientLogOnResponse. Requests carry a source job id and match
//! the response's target job id; unified service methods go through
//! ServiceMethodCallFromClient with `target_job_name`. Steam's
//! documented/observed protocol (SteamKit2 as reference, independently
//! implemented).

use std::{
    collections::VecDeque,
    io,
    net::{TcpStream, ToSocketAddrs},
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail, ensure};
use prost::Message as _;
use rustls::{ClientConfig, ClientConnection, StreamOwned, pki_types::ServerName};
use tungstenite::{Message, WebSocket, protocol::WebSocketConfig};

use super::{
    frame::{self, Packet, emsg},
    proto::{
        ClientHeartBeat, ClientHello, ClientLoggedOff, ClientLogon, ClientLogonResponse,
        ProtoHeader,
    },
};

/// `MsgClientLogon.CurrentProtocol` (major 1, minor 45).
pub const PROTOCOL_VERSION: u32 = 65_581;
/// `EOSType.LinuxUnknown` (-203) as sent on the wire.
const OS_TYPE_LINUX_UNKNOWN: u32 = (-203i32) as u32;
/// Public universe, anonymous-user account type, account and instance 0.
pub const ANONYMOUS_STEAM_ID: u64 = (1u64 << 56) | (10u64 << 52);
/// EResult values the fetcher distinguishes.
pub const ERESULT_OK: i32 = 1;

/// Human name for an EResult, for diagnostics.
pub fn eresult_name(value: i32) -> &'static str {
    match value {
        1 => "OK",
        2 => "Fail",
        3 => "NoConnection",
        5 => "InvalidPassword",
        7 => "InvalidProtocolVer",
        8 => "InvalidParam",
        9 => "FileNotFound",
        10 => "Busy",
        15 => "AccessDenied",
        16 => "Timeout",
        20 => "ServiceUnavailable",
        48 => "TryAnotherCM",
        84 => "RateLimitExceeded",
        _ => "other",
    }
}

/// Timeouts for one CM session.
#[derive(Debug, Clone, Copy)]
pub struct CmTimeouts {
    pub connect: Duration,
    pub handshake: Duration,
    pub request: Duration,
    /// Socket read tick; heartbeats are checked between reads.
    pub tick: Duration,
}

impl Default for CmTimeouts {
    fn default() -> Self {
        Self {
            connect: Duration::from_secs(15),
            handshake: Duration::from_secs(20),
            request: Duration::from_secs(30),
            tick: Duration::from_millis(500),
        }
    }
}

/// Size bounds for CM traffic.
#[derive(Debug, Clone, Copy)]
pub struct CmLimits {
    pub max_message: usize,
    pub max_unzipped: usize,
}

impl Default for CmLimits {
    fn default() -> Self {
        Self {
            max_message: 32 * 1024 * 1024,
            max_unzipped: 64 * 1024 * 1024,
        }
    }
}

type Stream = StreamOwned<ClientConnection, TcpStream>;

/// Result of a successful anonymous logon.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Logon {
    pub steam_id: u64,
    pub session_id: i32,
    pub cell_id: u32,
    pub heartbeat: Duration,
}

/// A connected CM session.
pub struct CmSession {
    ws: WebSocket<Stream>,
    pending: VecDeque<Packet>,
    steam_id: u64,
    session_id: i32,
    next_job: u64,
    heartbeat: Option<Duration>,
    last_send: Instant,
    timeouts: CmTimeouts,
    limits: CmLimits,
}

fn split_endpoint(endpoint: &str) -> Result<(&str, u16)> {
    let (host, port) = endpoint
        .rsplit_once(':')
        .context("CM endpoint has no port")?;
    ensure!(
        !host.is_empty()
            && host
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-')),
        "CM endpoint host {host:?} is not a plain host name"
    );
    Ok((host, port.parse().context("CM endpoint port")?))
}

impl CmSession {
    /// Open TCP, TLS and the WebSocket to `host:port`.
    pub fn connect(
        endpoint: &str,
        tls: Arc<ClientConfig>,
        timeouts: CmTimeouts,
        limits: CmLimits,
    ) -> Result<Self> {
        let (host, port) = split_endpoint(endpoint)?;
        let mut last_error = None;
        let mut tcp = None;
        for addr in (host, port)
            .to_socket_addrs()
            .with_context(|| format!("resolve {host}"))?
            .take(4)
        {
            match TcpStream::connect_timeout(&addr, timeouts.connect) {
                Ok(stream) => {
                    tcp = Some(stream);
                    break;
                }
                Err(error) => last_error = Some(error),
            }
        }
        let tcp = match (tcp, last_error) {
            (Some(tcp), _) => tcp,
            (None, Some(error)) => {
                return Err(error).with_context(|| format!("connect {endpoint}"));
            }
            (None, None) => bail!("{host} resolved to no addresses"),
        };
        tcp.set_nodelay(true).ok();
        tcp.set_read_timeout(Some(timeouts.handshake))?;
        tcp.set_write_timeout(Some(timeouts.handshake))?;
        let name = ServerName::try_from(host.to_owned()).context("CM TLS server name")?;
        let conn = ClientConnection::new(tls, name).context("start CM TLS")?;
        let stream = StreamOwned::new(conn, tcp);
        let config = WebSocketConfig::default()
            .max_message_size(Some(limits.max_message))
            .max_frame_size(Some(limits.max_message));
        let url = format!("wss://{host}:{port}/cmsocket/");
        let (ws, _response) =
            tungstenite::client::client_with_config(url.as_str(), stream, Some(config)).map_err(
                |error| anyhow::anyhow!("CM WebSocket handshake with {endpoint}: {error}"),
            )?;
        ws.get_ref().sock.set_read_timeout(Some(timeouts.tick))?;
        Ok(Self {
            ws,
            pending: VecDeque::new(),
            steam_id: ANONYMOUS_STEAM_ID,
            session_id: 0,
            next_job: 1,
            heartbeat: None,
            last_send: Instant::now(),
            timeouts,
            limits,
        })
    }

    fn header(&self) -> ProtoHeader {
        ProtoHeader {
            steamid: Some(self.steam_id),
            client_sessionid: Some(self.session_id),
            ..ProtoHeader::default()
        }
    }

    fn send_raw(&mut self, emsg: u32, header: &ProtoHeader, body: &[u8]) -> Result<()> {
        let bytes = frame::encode(emsg, header, body);
        self.ws
            .send(Message::binary(bytes))
            .with_context(|| format!("send EMsg {emsg}"))?;
        self.last_send = Instant::now();
        Ok(())
    }

    fn take_job(&mut self) -> u64 {
        let job = self.next_job;
        self.next_job += 1;
        job
    }

    fn heartbeat_if_due(&mut self) -> Result<()> {
        if let Some(interval) = self.heartbeat
            && self.last_send.elapsed() >= interval
        {
            let header = self.header();
            self.send_raw(
                emsg::CLIENT_HEART_BEAT,
                &header,
                &ClientHeartBeat::default().encode_to_vec(),
            )?;
        }
        Ok(())
    }

    /// Wait until a packet satisfies `matches`, servicing heartbeats.
    fn recv_matching(&mut self, mut matches: impl FnMut(&Packet) -> bool) -> Result<Packet> {
        let deadline = Instant::now() + self.timeouts.request;
        loop {
            while let Some(packet) = self.pending.pop_front() {
                if packet.emsg == emsg::CLIENT_LOGGED_OFF {
                    let eresult = ClientLoggedOff::decode(packet.body.as_slice())
                        .ok()
                        .and_then(|body| body.eresult)
                        .unwrap_or(2);
                    bail!(
                        "CM logged the session off: eresult {eresult} ({})",
                        eresult_name(eresult)
                    );
                }
                if matches(&packet) {
                    return Ok(packet);
                }
            }
            ensure!(Instant::now() < deadline, "CM request timed out");
            self.heartbeat_if_due()?;
            match self.ws.read() {
                Ok(Message::Binary(bytes)) => {
                    let packets = frame::decode_expanded(&bytes, self.limits.max_unzipped)?;
                    self.pending.extend(packets);
                }
                Ok(Message::Close(frame)) => bail!("CM closed the connection: {frame:?}"),
                Ok(_) => {}
                Err(tungstenite::Error::Io(error))
                    if matches!(
                        error.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                    ) =>
                {
                    // Flush any queued pong; a tick timeout is not an error.
                    match self.ws.flush() {
                        Ok(()) => {}
                        Err(tungstenite::Error::Io(error))
                            if matches!(
                                error.kind(),
                                io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                            ) => {}
                        Err(error) => return Err(error).context("flush CM socket"),
                    }
                }
                Err(error) => return Err(error).context("read CM socket"),
            }
        }
    }

    /// Hello, anonymous logon and heartbeat setup.
    pub fn logon_anonymous(&mut self) -> Result<Logon> {
        let hello = ClientHello {
            protocol_version: Some(PROTOCOL_VERSION),
        };
        self.send_raw(
            emsg::CLIENT_HELLO,
            &ProtoHeader::default(),
            &hello.encode_to_vec(),
        )?;
        let logon = ClientLogon {
            protocol_version: Some(PROTOCOL_VERSION),
            cell_id: Some(0),
            client_language: Some("english".to_owned()),
            client_os_type: Some(OS_TYPE_LINUX_UNKNOWN),
        };
        let header = ProtoHeader {
            steamid: Some(ANONYMOUS_STEAM_ID),
            client_sessionid: Some(0),
            ..ProtoHeader::default()
        };
        self.send_raw(emsg::CLIENT_LOGON, &header, &logon.encode_to_vec())?;
        let packet = self.recv_matching(|p| p.emsg == emsg::CLIENT_LOG_ON_RESPONSE)?;
        let response =
            ClientLogonResponse::decode(packet.body.as_slice()).context("ClientLogOnResponse")?;
        let eresult = response.eresult.unwrap_or(2);
        ensure!(
            eresult == ERESULT_OK,
            "anonymous logon failed: eresult {eresult} ({}), extended {:?}",
            eresult_name(eresult),
            response.eresult_extended
        );
        if let Some(steam_id) = packet.header.steamid {
            self.steam_id = steam_id;
        }
        if let Some(session_id) = packet.header.client_sessionid {
            self.session_id = session_id;
        }
        let seconds = response
            .heartbeat_seconds
            .filter(|&s| s > 0)
            .or(response
                .legacy_out_of_game_heartbeat_seconds
                .filter(|&s| s > 0))
            .unwrap_or(9)
            .clamp(2, 120);
        let heartbeat = Duration::from_secs(seconds as u64);
        self.heartbeat = Some(heartbeat);
        Ok(Logon {
            steam_id: self.steam_id,
            session_id: self.session_id,
            cell_id: response.cell_id.unwrap_or(0),
            heartbeat,
        })
    }

    /// Send a job request and return the first response of `response_emsg`
    /// addressed to the job.
    pub fn request(
        &mut self,
        request_emsg: u32,
        body: &[u8],
        response_emsg: u32,
    ) -> Result<Packet> {
        let job = self.send_job(request_emsg, body, None)?;
        self.recv_job(job, response_emsg)
    }

    /// Send a job-tagged message and return the job id.
    pub fn send_job(
        &mut self,
        emsg: u32,
        body: &[u8],
        target_job_name: Option<&str>,
    ) -> Result<u64> {
        let job = self.take_job();
        let header = ProtoHeader {
            jobid_source: Some(job),
            target_job_name: target_job_name.map(str::to_owned),
            ..self.header()
        };
        self.send_raw(emsg, &header, body)?;
        Ok(job)
    }

    /// Wait for the next `response_emsg` addressed to `job`. Responses
    /// without a target job are accepted by EMsg alone.
    pub fn recv_job(&mut self, job: u64, response_emsg: u32) -> Result<Packet> {
        self.recv_matching(|p| {
            p.emsg == response_emsg
                && p.header
                    .jobid_target
                    .is_none_or(|target| target == job || target == u64::MAX)
        })
        .with_context(|| format!("waiting for EMsg {response_emsg}"))
    }

    /// Call a unified service method such as
    /// `ContentServerDirectory.GetServersForSteamPipe#1`.
    pub fn service_call<Resp: prost::Message + Default>(
        &mut self,
        method: &str,
        body: &impl prost::Message,
    ) -> Result<Resp> {
        let job = self.send_job(
            emsg::SERVICE_METHOD_CALL_FROM_CLIENT,
            &body.encode_to_vec(),
            Some(method),
        )?;
        let packet = self
            .recv_job(job, emsg::SERVICE_METHOD_RESPONSE)
            .with_context(|| format!("service call {method}"))?;
        let eresult = packet.eresult();
        ensure!(
            eresult == ERESULT_OK,
            "service call {method} failed: eresult {eresult} ({}) {}",
            eresult_name(eresult),
            packet.header.error_message.as_deref().unwrap_or_default()
        );
        Resp::decode(packet.body.as_slice()).with_context(|| format!("{method} response"))
    }

    /// Log off and close; errors are ignored.
    pub fn close(mut self) {
        let header = self.header();
        let _ = self.send_raw(emsg::CLIENT_LOG_OFF, &header, &[]);
        let _ = self.ws.close(None);
        let _ = self.ws.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anonymous_identity_and_endpoints() {
        // Universe 1 (Public), account type 10 (AnonUser), account 0.
        assert_eq!(ANONYMOUS_STEAM_ID, 0x01A0_0000_0000_0000);
        assert_eq!(OS_TYPE_LINUX_UNKNOWN, 0xFFFF_FF35);
        assert_eq!(PROTOCOL_VERSION >> 16, 1);
        assert_eq!(
            split_endpoint("cmp1-fra2.steamserver.net:27023").unwrap(),
            ("cmp1-fra2.steamserver.net", 27023)
        );
        assert!(split_endpoint("no-port.example").is_err());
        assert!(split_endpoint("bad/host:443").is_err());
        assert!(split_endpoint("host:99999").is_err());
    }
}

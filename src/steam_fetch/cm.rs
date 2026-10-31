//! Anonymous CM session over a TLS WebSocket (`wss://<endpoint>/cmsocket/`).
//!
//! ClientHello, then ClientLogon as an anonymous user or an anonymous game
//! server (which names the application it serves); heartbeats follow the
//! interval in ClientLogOnResponse. Requests carry a source job id and match
//! the response's target job id; unified service methods go through
//! ServiceMethodCallFromClient with `target_job_name`. Steam's
//! documented/observed protocol (SteamKit2 as reference, independently
//! implemented).

use std::{
    cell::Cell,
    collections::VecDeque,
    io::{self, Read, Write},
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
        ClientHeartBeat, ClientHello, ClientLoggedOff, ClientLogon, ClientLogonResponse, IpAddress,
        Multi, PRIVATE_IP_MASK, ProtoHeader,
    },
};

/// `MsgClientLogon.CurrentProtocol` (major 1, minor 45).
pub const PROTOCOL_VERSION: u32 = 65_581;
/// `EOSType.LinuxUnknown` (-203) as sent on the wire.
const OS_TYPE_LINUX_UNKNOWN: u32 = (-203i32) as u32;
/// Public universe, anonymous-user account type, account and instance 0.
pub const ANONYMOUS_STEAM_ID: u64 = (1u64 << 56) | (10u64 << 52);
/// Public universe, anonymous-game-server account type; Steam assigns the
/// account in the logon response header.
pub const ANONYMOUS_GAME_SERVER_STEAM_ID: u64 = (1u64 << 56) | (4u64 << 52);
/// EResult values the fetcher distinguishes.
pub const ERESULT_OK: i32 = 1;

/// A private IPv4 address as Steam clients report it at logon.
pub fn obfuscate_private_ip(ip: std::net::Ipv4Addr) -> IpAddress {
    IpAddress {
        v4: Some(u32::from(ip) ^ PRIVATE_IP_MASK),
    }
}

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

/// A socket timeout applies to each syscall. This wrapper additionally applies
/// one absolute deadline to every nested TLS/WebSocket read and write, so a
/// slowly arriving record or frame cannot reset the operation's budget.
struct DeadlineTcp {
    socket: TcpStream,
    deadline: Cell<Option<Instant>>,
    read_timeout: Cell<Option<Duration>>,
    write_timeout: Cell<Option<Duration>>,
}

impl DeadlineTcp {
    fn new(socket: TcpStream) -> io::Result<Self> {
        let read_timeout = Cell::new(socket.read_timeout()?);
        let write_timeout = Cell::new(socket.write_timeout()?);
        Ok(Self {
            socket,
            deadline: Cell::new(None),
            read_timeout,
            write_timeout,
        })
    }

    fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        self.socket.set_read_timeout(timeout)?;
        self.read_timeout.set(timeout);
        Ok(())
    }

    fn set_deadline(&self, deadline: Option<Instant>) -> io::Result<()> {
        self.deadline.set(deadline);
        if deadline.is_none() {
            self.socket.set_read_timeout(self.read_timeout.get())?;
            self.socket.set_write_timeout(self.write_timeout.get())?;
        }
        Ok(())
    }

    fn timeout(&self, configured: Option<Duration>) -> io::Result<Option<Duration>> {
        let Some(deadline) = self.deadline.get() else {
            return Ok(configured);
        };
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "CM operation deadline reached",
            ));
        }
        Ok(Some(
            configured.map_or(remaining, |timeout| timeout.min(remaining)),
        ))
    }
}

impl Read for DeadlineTcp {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        self.socket
            .set_read_timeout(self.timeout(self.read_timeout.get())?)?;
        self.socket.read(bytes)
    }
}

impl Write for DeadlineTcp {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.socket
            .set_write_timeout(self.timeout(self.write_timeout.get())?)?;
        self.socket.write(bytes)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.socket
            .set_write_timeout(self.timeout(self.write_timeout.get())?)?;
        self.socket.flush()
    }
}

type Stream = StreamOwned<ClientConnection, DeadlineTcp>;

const MAX_WORKER_DECODE_BYTES: usize = 256 * 1024;
const MAX_WORKER_FRAME_PACKETS: usize = 1024;

/// The service needs small control messages. Bound aggregate nested expansion,
/// rather than allowing every nested gzip to spend the full frame budget.
fn decode_worker_frame(
    bytes: &[u8],
    max_unzipped: usize,
    deadline: Instant,
) -> Result<Vec<Packet>> {
    fn expand(
        bytes: &[u8],
        depth: usize,
        budget: &mut usize,
        deadline: Instant,
        out: &mut Vec<Packet>,
    ) -> Result<()> {
        ensure!(Instant::now() < deadline, "CM decoding deadline reached");
        *budget = budget
            .checked_sub(bytes.len())
            .context("CM worker decoding budget exceeded")?;
        let packet = frame::decode(bytes)?;
        if packet.emsg != emsg::MULTI {
            ensure!(
                out.len() < MAX_WORKER_FRAME_PACKETS,
                "CM worker packet budget exceeded"
            );
            out.push(packet);
            return Ok(());
        }
        ensure!(packet.proto && depth < 2, "invalid CM worker Multi nesting");
        let multi = Multi::decode(packet.body.as_slice()).context("CM worker Multi")?;
        let body = multi.message_body.unwrap_or_default();
        let payload = match multi.size_unzipped.unwrap_or(0) as usize {
            0 => body,
            size => {
                *budget = budget
                    .checked_sub(size)
                    .context("CM worker expansion budget exceeded")?;
                let mut payload = Vec::with_capacity(size);
                flate2::read::GzDecoder::new(body.as_slice())
                    .take(size as u64 + 1)
                    .read_to_end(&mut payload)
                    .context("CM worker gzip")?;
                ensure!(payload.len() == size, "CM worker gzip length mismatch");
                payload
            }
        };
        let mut rest = payload.as_slice();
        while !rest.is_empty() {
            ensure!(Instant::now() < deadline, "CM decoding deadline reached");
            ensure!(rest.len() >= 4, "truncated CM worker Multi length");
            let length = u32::from_le_bytes(rest[..4].try_into().expect("checked length")) as usize;
            ensure!(
                length <= rest.len() - 4,
                "CM worker Multi length exceeds body"
            );
            expand(&rest[4..4 + length], depth + 1, budget, deadline, out)?;
            rest = &rest[4 + length..];
        }
        Ok(())
    }
    let mut packets = Vec::new();
    let mut budget = MAX_WORKER_DECODE_BYTES.min(max_unzipped);
    expand(bytes, 0, &mut budget, deadline, &mut packets)?;
    ensure!(Instant::now() < deadline, "CM decoding deadline reached");
    Ok(packets)
}

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
        let stream = StreamOwned::new(conn, DeadlineTcp::new(tcp)?);
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
    fn recv_matching(&mut self, matches: impl FnMut(&Packet) -> bool) -> Result<Packet> {
        let deadline = Instant::now() + self.timeouts.request;
        self.recv_matching_until(matches, deadline)?
            .context("CM request timed out")
    }

    /// As [`Self::recv_matching`], returning `None` at `deadline`.
    fn recv_matching_until(
        &mut self,
        matches: impl FnMut(&Packet) -> bool,
        deadline: Instant,
    ) -> Result<Option<Packet>> {
        self.recv_matching_until_inner(matches, deadline, false)
    }

    fn recv_matching_until_inner(
        &mut self,
        mut matches: impl FnMut(&Packet) -> bool,
        deadline: Instant,
        bounded: bool,
    ) -> Result<Option<Packet>> {
        loop {
            if bounded && Instant::now() >= deadline {
                return Ok(None);
            }
            while let Some(packet) = self.pending.pop_front() {
                if bounded {
                    ensure!(
                        packet.body.len() <= MAX_WORKER_DECODE_BYTES,
                        "CM worker pending packet exceeds byte budget"
                    );
                }
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
                    return Ok(Some(packet));
                }
            }
            if Instant::now() >= deadline {
                return Ok(None);
            }
            self.heartbeat_if_due()?;
            match self.ws.read() {
                Ok(Message::Binary(bytes)) => {
                    let packets = if bounded {
                        decode_worker_frame(&bytes, self.limits.max_unzipped, deadline)?
                    } else {
                        frame::decode_expanded(&bytes, self.limits.max_unzipped)?
                    };
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
        let logon = ClientLogon {
            protocol_version: Some(PROTOCOL_VERSION),
            cell_id: Some(0),
            client_language: Some("english".to_owned()),
            client_os_type: Some(OS_TYPE_LINUX_UNKNOWN),
            ..ClientLogon::default()
        };
        self.logon(ANONYMOUS_STEAM_ID, &logon, "anonymous logon")
    }

    /// Hello and logon as an anonymous game server serving `app_id`, which
    /// reports `private_ipv4` masked as Steam clients do.
    pub fn logon_anonymous_game_server(
        &mut self,
        app_id: u32,
        private_ipv4: std::net::Ipv4Addr,
    ) -> Result<Logon> {
        let logon = ClientLogon {
            protocol_version: Some(PROTOCOL_VERSION),
            client_os_type: Some(OS_TYPE_LINUX_UNKNOWN),
            obfuscated_private_ip: Some(obfuscate_private_ip(private_ipv4)),
            game_server_app_id: Some(i32::try_from(app_id).context("game server app id")?),
            ..ClientLogon::default()
        };
        self.logon(
            ANONYMOUS_GAME_SERVER_STEAM_ID,
            &logon,
            "anonymous game-server logon",
        )
    }

    fn logon(&mut self, steam_id: u64, logon: &ClientLogon, what: &str) -> Result<Logon> {
        let hello = ClientHello {
            protocol_version: Some(PROTOCOL_VERSION),
        };
        self.send_raw(
            emsg::CLIENT_HELLO,
            &ProtoHeader::default(),
            &hello.encode_to_vec(),
        )?;
        let header = ProtoHeader {
            steamid: Some(steam_id),
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
            "{what} failed: eresult {eresult} ({}), extended {:?}",
            eresult_name(eresult),
            response.eresult_extended
        );
        self.steam_id = packet.header.steamid.unwrap_or(steam_id);
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

    /// Send a message without a job.
    pub fn send(&mut self, emsg: u32, body: &[u8]) -> Result<()> {
        let header = self.header();
        self.send_raw(emsg, &header, body)
    }

    /// Send under a worker-selected socket write bound. A partial write error
    /// is fatal to the caller's service; it must not replay this message blindly.
    pub fn send_bounded(&mut self, emsg: u32, body: &[u8], timeout: Duration) -> Result<()> {
        ensure!(
            !timeout.is_zero() && timeout <= Duration::from_secs(1),
            "invalid CM write window"
        );
        self.ws
            .get_ref()
            .sock
            .set_deadline(Some(Instant::now() + timeout))?;
        let result = self.send(emsg, body);
        self.ws.get_ref().sock.set_deadline(None)?;
        result
    }

    /// Service heartbeats and return the messages that arrive within `wait`
    /// (at least one socket tick). A logoff from the CM is an error.
    pub fn poll(&mut self, wait: Duration) -> Result<Vec<Packet>> {
        let deadline = Instant::now() + wait;
        let mut packets = Vec::new();
        loop {
            match self.recv_matching_until(|_| true, deadline)? {
                Some(packet) => packets.push(packet),
                None => return Ok(packets),
            }
        }
    }

    /// Drain at most `max_packets` with a bounded read window, retaining later
    /// packets for the next call. The existing WebSocket and Multi byte limits
    /// bound each decoded frame. No unmatched message is discarded.
    pub fn poll_bounded(&mut self, wait: Duration, max_packets: usize) -> Result<Vec<Packet>> {
        ensure!(
            !wait.is_zero() && wait <= Duration::from_secs(1),
            "invalid CM poll window"
        );
        ensure!(
            (1..=1024).contains(&max_packets),
            "invalid CM poll packet bound"
        );
        let deadline = Instant::now() + wait;
        self.ws.get_ref().sock.set_deadline(Some(deadline))?;
        let result = (|| {
            self.heartbeat_if_due()?;
            let mut packets = Vec::with_capacity(max_packets);
            let mut remaining_bytes = MAX_WORKER_DECODE_BYTES;
            while packets.len() < max_packets && Instant::now() < deadline {
                match self.recv_matching_until_inner(|_| true, deadline, true)? {
                    Some(packet) => {
                        if packet.body.len() > remaining_bytes {
                            self.pending.push_front(packet);
                            break;
                        }
                        remaining_bytes -= packet.body.len();
                        packets.push(packet);
                    }
                    None => break,
                }
            }
            Ok(packets)
        })();
        self.ws.get_ref().sock.set_deadline(None)?;
        result
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

    /// Best-effort logoff with a short bounded write timeout for owned workers.
    pub fn close_bounded(self, timeout: Duration) {
        let timeout = timeout.clamp(Duration::from_millis(1), Duration::from_secs(1));
        let _ = self
            .ws
            .get_ref()
            .sock
            .set_deadline(Some(Instant::now() + timeout));
        self.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn socket_pair() -> (TcpStream, TcpStream) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (server, _) = listener.accept().unwrap();
        (client, server)
    }

    #[test]
    fn a_trickling_frame_cannot_reset_the_absolute_read_deadline() {
        let (client, mut server) = socket_pair();
        client
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        server
            .set_write_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        let sender = std::thread::spawn(move || {
            for _ in 0..40 {
                if server.write_all(&[0x5a]).is_err() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        });
        let mut socket = DeadlineTcp::new(client).unwrap();
        socket
            .set_deadline(Some(Instant::now() + Duration::from_millis(80)))
            .unwrap();
        let error = socket.read_exact(&mut [0; 40]).unwrap_err();
        assert!(matches!(
            error.kind(),
            io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
        ));
        drop(socket);
        sender.join().unwrap();
    }

    #[test]
    fn expired_deadline_blocks_writes_and_clear_restores_configured_timeouts() {
        let (client, mut server) = socket_pair();
        client
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        client
            .set_write_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let mut socket = DeadlineTcp::new(client).unwrap();
        socket.set_deadline(Some(Instant::now())).unwrap();
        assert_eq!(
            socket.write(&[1]).unwrap_err().kind(),
            io::ErrorKind::TimedOut
        );
        assert_eq!(
            socket.read(&mut [0]).unwrap_err().kind(),
            io::ErrorKind::TimedOut
        );
        socket.set_deadline(None).unwrap();
        assert_eq!(
            socket.socket.read_timeout().unwrap(),
            Some(Duration::from_secs(2))
        );
        assert_eq!(
            socket.socket.write_timeout().unwrap(),
            Some(Duration::from_secs(3))
        );
        socket.write_all(&[7]).unwrap();
        server
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        let mut read = [0];
        server.read_exact(&mut read).unwrap();
        assert_eq!(read, [7]);
    }

    #[test]
    fn worker_decode_limits_total_nested_expansion_packets_and_time() {
        let deadline = Instant::now() + Duration::from_secs(10);
        let small = frame::encode(emsg::GS_STATUS_REPLY, &ProtoHeader::default(), &[8, 0]);
        let nested = frame::encode_multi(
            &[frame::encode_multi(std::slice::from_ref(&small), true)],
            true,
        );
        let result = decode_worker_frame(&nested, MAX_WORKER_DECODE_BYTES, deadline).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].body, [8, 0]);
        assert!(decode_worker_frame(&small, MAX_WORKER_DECODE_BYTES, Instant::now()).is_err());
        let many = frame::encode_multi(&vec![small; MAX_WORKER_FRAME_PACKETS + 1], true);
        assert!(decode_worker_frame(&many, MAX_WORKER_DECODE_BYTES, deadline).is_err());
        // Each child expansion fits, but their aggregate work exceeds the cap.
        let large = frame::encode(
            emsg::GS_STATUS_REPLY,
            &ProtoHeader::default(),
            &vec![0; 70 * 1024],
        );
        let child = frame::encode_multi(&[large], true);
        let nested = frame::encode_multi(&[child.clone(), child], true);
        assert!(decode_worker_frame(&nested, MAX_WORKER_DECODE_BYTES, deadline).is_err());
    }

    #[test]
    fn anonymous_identity_and_endpoints() {
        // Universe 1 (Public), account type 10 (AnonUser), account 0.
        assert_eq!(ANONYMOUS_STEAM_ID, 0x01A0_0000_0000_0000);
        // Account type 4 (AnonGameServer).
        assert_eq!(ANONYMOUS_GAME_SERVER_STEAM_ID, 0x0140_0000_0000_0000);
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

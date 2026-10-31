//! Strict codec for Steam's public A2S server queries (`A2S_INFO`,
//! `A2S_PLAYER`, `A2S_RULES` and their challenge), independently authored
//! from Valve's public "Server queries" protocol documentation.
//!
//! These are the UDP queries a server browser sends to a game server's query
//! port. Which fields and rules the original ICARUS server answers with is an
//! observation for a recorded case, not something this module asserts. Strings
//! are kept as raw bytes (NUL-terminated on the wire, no interior NUL), so a
//! decode followed by an encode reproduces the datagram. Split responses use
//! the Source layout (`-2`, 32-bit ID, total, number, 16-bit size); compressed
//! splits are rejected. Every read is bounded by the datagram.

use anyhow::{Result, bail, ensure};

/// Header of an unsplit datagram.
pub const SIMPLE_HEADER: i32 = -1;
/// Header of one slice of a split response.
pub const SPLIT_HEADER: i32 = -2;
/// The payload `A2S_INFO` requests carry before any challenge.
pub const INFO_PAYLOAD: &[u8] = b"Source Engine Query\0";
/// The challenge value that asks a server to issue one.
pub const REQUEST_CHALLENGE: i32 = -1;
/// The largest datagram this codec reads or writes.
pub const MAX_DATAGRAM: usize = 1_400;
/// The largest reassembled response accepted from split slices.
pub const MAX_REASSEMBLED: usize = 64 * 1_024;
/// The most slices of one split response.
pub const MAX_SPLIT_SLICES: u8 = 64;

const INFO_REQUEST: u8 = b'T';
const PLAYER_REQUEST: u8 = b'U';
const RULES_REQUEST: u8 = b'V';
const CHALLENGE_RESPONSE: u8 = b'A';
const INFO_RESPONSE: u8 = b'I';
const PLAYER_RESPONSE: u8 = b'D';
const RULES_RESPONSE: u8 = b'E';

const EDF_GAME_ID: u8 = 0x01;
const EDF_STEAM_ID: u8 = 0x10;
const EDF_KEYWORDS: u8 = 0x20;
const EDF_SPECTATOR: u8 = 0x40;
const EDF_PORT: u8 = 0x80;
const EDF_KNOWN: u8 = EDF_GAME_ID | EDF_STEAM_ID | EDF_KEYWORDS | EDF_SPECTATOR | EDF_PORT;

/// A query a client sends to the query port.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Request {
    /// `A2S_INFO`; the challenge is absent on a first attempt.
    Info { challenge: Option<i32> },
    /// `A2S_PLAYER`; [`REQUEST_CHALLENGE`] asks for a challenge.
    Players { challenge: i32 },
    /// `A2S_RULES`; [`REQUEST_CHALLENGE`] asks for a challenge.
    Rules { challenge: i32 },
}

/// Spectator port and name, present when the extra-data flag says so.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Spectator {
    pub port: u16,
    pub name: Vec<u8>,
}

/// The `A2S_INFO` answer. Optional fields follow the extra-data flag byte,
/// which [`Info::extra_data_flags`] derives, so unknown flag bits cannot be
/// encoded and are rejected on decode.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Info {
    pub protocol: u8,
    pub name: Vec<u8>,
    pub map: Vec<u8>,
    pub folder: Vec<u8>,
    pub game: Vec<u8>,
    /// Low 16 bits of the Steam application ID.
    pub app_id: u16,
    pub players: u8,
    pub max_players: u8,
    pub bots: u8,
    /// `d` dedicated, `l` listen, `p` proxy.
    pub server_type: u8,
    /// `l` Linux, `w` Windows, `m` or `o` macOS.
    pub environment: u8,
    /// Zero public, one password protected.
    pub visibility: u8,
    pub vac: u8,
    pub version: Vec<u8>,
    pub port: Option<u16>,
    pub steam_id: Option<u64>,
    pub spectator: Option<Spectator>,
    pub keywords: Option<Vec<u8>>,
    pub game_id: Option<u64>,
}

impl Info {
    /// The extra-data flag byte for the present optional fields.
    pub fn extra_data_flags(&self) -> u8 {
        let mut flags = 0;
        if self.port.is_some() {
            flags |= EDF_PORT;
        }
        if self.steam_id.is_some() {
            flags |= EDF_STEAM_ID;
        }
        if self.spectator.is_some() {
            flags |= EDF_SPECTATOR;
        }
        if self.keywords.is_some() {
            flags |= EDF_KEYWORDS;
        }
        if self.game_id.is_some() {
            flags |= EDF_GAME_ID;
        }
        flags
    }
}

/// One `A2S_PLAYER` entry.
#[derive(Clone, Debug, PartialEq)]
pub struct Player {
    pub index: u8,
    pub name: Vec<u8>,
    pub score: i32,
    /// Seconds connected.
    pub duration: f32,
}

/// One `A2S_RULES` key and value, in wire order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rule {
    pub key: Vec<u8>,
    pub value: Vec<u8>,
}

/// A server's answer, after any split reassembly.
#[derive(Clone, Debug, PartialEq)]
pub enum Response {
    Challenge(i32),
    Info(Info),
    Players(Vec<Player>),
    Rules(Vec<Rule>),
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, at: 0 }
    }

    fn take(&mut self, size: usize) -> Result<&'a [u8]> {
        let end = self.at.checked_add(size);
        let Some(slice) = end.and_then(|end| self.bytes.get(self.at..end)) else {
            bail!("server query truncated at byte {}", self.at);
        };
        self.at += size;
        Ok(slice)
    }

    fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into()?))
    }

    fn i32(&mut self) -> Result<i32> {
        Ok(i32::from_le_bytes(self.take(4)?.try_into()?))
    }

    fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into()?))
    }

    fn f32(&mut self) -> Result<f32> {
        Ok(f32::from_le_bytes(self.take(4)?.try_into()?))
    }

    fn string(&mut self) -> Result<Vec<u8>> {
        let rest = &self.bytes[self.at..];
        let Some(end) = rest.iter().position(|&byte| byte == 0) else {
            bail!(
                "server query string at byte {} lacks its terminator",
                self.at
            );
        };
        let value = rest[..end].to_vec();
        self.at += end + 1;
        Ok(value)
    }

    fn finish(&self) -> Result<()> {
        ensure!(
            self.at == self.bytes.len(),
            "server query has {} trailing bytes",
            self.bytes.len() - self.at
        );
        Ok(())
    }
}

fn put_string(out: &mut Vec<u8>, value: &[u8]) -> Result<()> {
    ensure!(
        !value.contains(&0),
        "server query string holds an interior NUL"
    );
    out.extend_from_slice(value);
    out.push(0);
    Ok(())
}

fn simple(kind: u8) -> Vec<u8> {
    let mut out = SIMPLE_HEADER.to_le_bytes().to_vec();
    out.push(kind);
    out
}

fn checked(out: Vec<u8>) -> Result<Vec<u8>> {
    ensure!(
        out.len() <= MAX_REASSEMBLED,
        "server query message of {} bytes exceeds {MAX_REASSEMBLED}",
        out.len()
    );
    Ok(out)
}

/// Encode a client query as one datagram.
pub fn encode_request(request: &Request) -> Vec<u8> {
    match *request {
        Request::Info { challenge } => {
            let mut out = simple(INFO_REQUEST);
            out.extend_from_slice(INFO_PAYLOAD);
            if let Some(challenge) = challenge {
                out.extend_from_slice(&challenge.to_le_bytes());
            }
            out
        }
        Request::Players { challenge } => {
            let mut out = simple(PLAYER_REQUEST);
            out.extend_from_slice(&challenge.to_le_bytes());
            out
        }
        Request::Rules { challenge } => {
            let mut out = simple(RULES_REQUEST);
            out.extend_from_slice(&challenge.to_le_bytes());
            out
        }
    }
}

/// Decode a client query datagram; anything else is an error.
pub fn parse_request(datagram: &[u8]) -> Result<Request> {
    ensure!(
        datagram.len() <= MAX_DATAGRAM,
        "server query datagram exceeds {MAX_DATAGRAM} bytes"
    );
    let mut reader = Reader::new(datagram);
    ensure!(
        reader.i32()? == SIMPLE_HEADER,
        "server query request is not an unsplit datagram"
    );
    let request = match reader.u8()? {
        INFO_REQUEST => {
            ensure!(
                reader.take(INFO_PAYLOAD.len())? == INFO_PAYLOAD,
                "A2S_INFO payload differs"
            );
            let challenge = if reader.at == datagram.len() {
                None
            } else {
                Some(reader.i32()?)
            };
            Request::Info { challenge }
        }
        PLAYER_REQUEST => Request::Players {
            challenge: reader.i32()?,
        },
        RULES_REQUEST => Request::Rules {
            challenge: reader.i32()?,
        },
        other => bail!("unsupported server query request 0x{other:02x}"),
    };
    reader.finish()?;
    Ok(request)
}

/// Encode a server answer as one unsplit message; [`split_response`] slices
/// it when it exceeds a datagram.
pub fn encode_response(response: &Response) -> Result<Vec<u8>> {
    let out = match response {
        Response::Challenge(challenge) => {
            let mut out = simple(CHALLENGE_RESPONSE);
            out.extend_from_slice(&challenge.to_le_bytes());
            out
        }
        Response::Info(info) => {
            let mut out = simple(INFO_RESPONSE);
            out.push(info.protocol);
            put_string(&mut out, &info.name)?;
            put_string(&mut out, &info.map)?;
            put_string(&mut out, &info.folder)?;
            put_string(&mut out, &info.game)?;
            out.extend_from_slice(&info.app_id.to_le_bytes());
            out.extend_from_slice(&[
                info.players,
                info.max_players,
                info.bots,
                info.server_type,
                info.environment,
                info.visibility,
                info.vac,
            ]);
            put_string(&mut out, &info.version)?;
            out.push(info.extra_data_flags());
            if let Some(port) = info.port {
                out.extend_from_slice(&port.to_le_bytes());
            }
            if let Some(steam_id) = info.steam_id {
                out.extend_from_slice(&steam_id.to_le_bytes());
            }
            if let Some(spectator) = &info.spectator {
                out.extend_from_slice(&spectator.port.to_le_bytes());
                put_string(&mut out, &spectator.name)?;
            }
            if let Some(keywords) = &info.keywords {
                put_string(&mut out, keywords)?;
            }
            if let Some(game_id) = info.game_id {
                out.extend_from_slice(&game_id.to_le_bytes());
            }
            out
        }
        Response::Players(players) => {
            let count = u8::try_from(players.len())?;
            let mut out = simple(PLAYER_RESPONSE);
            out.push(count);
            for player in players {
                out.push(player.index);
                put_string(&mut out, &player.name)?;
                out.extend_from_slice(&player.score.to_le_bytes());
                out.extend_from_slice(&player.duration.to_le_bytes());
            }
            out
        }
        Response::Rules(rules) => {
            let count = u16::try_from(rules.len())?;
            let mut out = simple(RULES_RESPONSE);
            out.extend_from_slice(&count.to_le_bytes());
            for rule in rules {
                put_string(&mut out, &rule.key)?;
                put_string(&mut out, &rule.value)?;
            }
            out
        }
    };
    checked(out)
}

/// Decode one unsplit server message (a datagram with header `-1`, or a
/// reassembled split payload).
pub fn parse_response(message: &[u8]) -> Result<Response> {
    ensure!(
        message.len() <= MAX_REASSEMBLED,
        "server query message exceeds {MAX_REASSEMBLED} bytes"
    );
    let mut reader = Reader::new(message);
    ensure!(
        reader.i32()? == SIMPLE_HEADER,
        "server query response is not an unsplit message"
    );
    let response = match reader.u8()? {
        CHALLENGE_RESPONSE => Response::Challenge(reader.i32()?),
        INFO_RESPONSE => {
            let protocol = reader.u8()?;
            let name = reader.string()?;
            let map = reader.string()?;
            let folder = reader.string()?;
            let game = reader.string()?;
            let app_id = reader.u16()?;
            let [
                players,
                max_players,
                bots,
                server_type,
                environment,
                visibility,
                vac,
            ] = reader.take(7)?.try_into()?;
            let version = reader.string()?;
            let flags = if reader.at == message.len() {
                0
            } else {
                reader.u8()?
            };
            ensure!(
                flags & !EDF_KNOWN == 0,
                "A2S_INFO extra-data flags 0x{flags:02x} hold unknown bits"
            );
            let port = (flags & EDF_PORT != 0).then(|| reader.u16()).transpose()?;
            let steam_id = (flags & EDF_STEAM_ID != 0)
                .then(|| reader.u64())
                .transpose()?;
            let spectator = if flags & EDF_SPECTATOR != 0 {
                Some(Spectator {
                    port: reader.u16()?,
                    name: reader.string()?,
                })
            } else {
                None
            };
            let keywords = (flags & EDF_KEYWORDS != 0)
                .then(|| reader.string())
                .transpose()?;
            let game_id = (flags & EDF_GAME_ID != 0)
                .then(|| reader.u64())
                .transpose()?;
            Response::Info(Info {
                protocol,
                name,
                map,
                folder,
                game,
                app_id,
                players,
                max_players,
                bots,
                server_type,
                environment,
                visibility,
                vac,
                version,
                port,
                steam_id,
                spectator,
                keywords,
                game_id,
            })
        }
        PLAYER_RESPONSE => {
            let count = reader.u8()?;
            let mut players = Vec::with_capacity(usize::from(count));
            for _ in 0..count {
                players.push(Player {
                    index: reader.u8()?,
                    name: reader.string()?,
                    score: reader.i32()?,
                    duration: reader.f32()?,
                });
            }
            Response::Players(players)
        }
        RULES_RESPONSE => {
            let count = reader.u16()?;
            // Each rule takes at least two terminators, so a count the message
            // cannot hold fails before allocation.
            ensure!(
                usize::from(count) * 2 <= message.len() - reader.at,
                "A2S_RULES count {count} exceeds its message"
            );
            let mut rules = Vec::with_capacity(usize::from(count));
            for _ in 0..count {
                rules.push(Rule {
                    key: reader.string()?,
                    value: reader.string()?,
                });
            }
            Response::Rules(rules)
        }
        other => bail!("unsupported server query response 0x{other:02x}"),
    };
    reader.finish()?;
    Ok(response)
}

/// One slice of a split response.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SplitSlice {
    pub id: i32,
    pub total: u8,
    pub number: u8,
    pub max_size: u16,
    pub payload: Vec<u8>,
}

/// Decode a split slice; compressed responses (ID high bit) are rejected.
pub fn parse_split(datagram: &[u8]) -> Result<SplitSlice> {
    ensure!(
        datagram.len() <= MAX_DATAGRAM,
        "server query datagram exceeds {MAX_DATAGRAM} bytes"
    );
    let mut reader = Reader::new(datagram);
    ensure!(
        reader.i32()? == SPLIT_HEADER,
        "server query datagram is not a split slice"
    );
    let id = reader.i32()?;
    ensure!(id >= 0, "compressed split responses are not supported");
    let total = reader.u8()?;
    let number = reader.u8()?;
    let max_size = reader.u16()?;
    ensure!(
        (1..=MAX_SPLIT_SLICES).contains(&total) && number < total,
        "split slice {number} of {total} is out of range"
    );
    Ok(SplitSlice {
        id,
        total,
        number,
        max_size,
        payload: datagram[reader.at..].to_vec(),
    })
}

/// Slice `message` into split datagrams whose payloads hold at most
/// `max_payload` bytes; a message that fits is returned unsplit.
pub fn split_response(message: &[u8], id: i32, max_payload: usize) -> Result<Vec<Vec<u8>>> {
    const SPLIT_PREFIX: usize = 12;
    ensure!(id >= 0, "split ID must leave the compression bit clear");
    ensure!(
        (1..=MAX_DATAGRAM - SPLIT_PREFIX).contains(&max_payload),
        "split payload size {max_payload} is out of range"
    );
    if message.len() <= MAX_DATAGRAM {
        return Ok(vec![message.to_vec()]);
    }
    let total = message.len().div_ceil(max_payload);
    let total = u8::try_from(total)
        .ok()
        .filter(|&total| total <= MAX_SPLIT_SLICES)
        .ok_or_else(|| anyhow::anyhow!("message needs more than {MAX_SPLIT_SLICES} slices"))?;
    let max_size = u16::try_from(max_payload + SPLIT_PREFIX)?;
    Ok(message
        .chunks(max_payload)
        .enumerate()
        .map(|(number, chunk)| {
            let mut out = SPLIT_HEADER.to_le_bytes().to_vec();
            out.extend_from_slice(&id.to_le_bytes());
            // `number < total <= MAX_SPLIT_SLICES` was checked above.
            out.extend_from_slice(&[total, number as u8]);
            out.extend_from_slice(&max_size.to_le_bytes());
            out.extend_from_slice(chunk);
            out
        })
        .collect())
}

/// Reassembles one split response from slices in any order.
#[derive(Debug, Default)]
pub struct SplitAssembler {
    id: Option<i32>,
    slices: Vec<Option<Vec<u8>>>,
    bytes: usize,
}

impl SplitAssembler {
    /// Add a slice; returns the reassembled message once every slice arrived.
    /// Slices of another response, a changed total and duplicates are errors.
    pub fn push(&mut self, slice: SplitSlice) -> Result<Option<Vec<u8>>> {
        match self.id {
            None => {
                self.id = Some(slice.id);
                self.slices = vec![None; usize::from(slice.total)];
            }
            Some(id) => {
                ensure!(id == slice.id, "split slice belongs to another response");
                ensure!(
                    self.slices.len() == usize::from(slice.total),
                    "split slice total changed"
                );
            }
        }
        let entry = &mut self.slices[usize::from(slice.number)];
        ensure!(entry.is_none(), "duplicate split slice {}", slice.number);
        self.bytes += slice.payload.len();
        ensure!(
            self.bytes <= MAX_REASSEMBLED,
            "split response exceeds {MAX_REASSEMBLED} bytes"
        );
        *entry = Some(slice.payload);
        if self.slices.iter().any(Option::is_none) {
            return Ok(None);
        }
        let message = self.slices.drain(..).flatten().flatten().collect();
        self.id = None;
        self.bytes = 0;
        Ok(Some(message))
    }
}

/// Read the header of any datagram: `-1` unsplit, `-2` split.
pub fn datagram_header(datagram: &[u8]) -> Result<i32> {
    Reader::new(datagram).i32()
}

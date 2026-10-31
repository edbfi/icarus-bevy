//! CM message framing over the WebSocket transport.
//!
//! Each binary WebSocket message carries one CM message: little-endian
//! `u32` EMsg with the protobuf bit, little-endian `u32` header length, the
//! `CMsgProtoBufHeader`, then the body. `Multi` bundles length-prefixed
//! messages, gzip-compressed when `size_unzipped` is non-zero. Steam's
//! documented/observed protocol (SteamKit2 as reference, independently
//! implemented).

use std::io::Read;

use anyhow::{Context, Result, bail, ensure};
use prost::Message as _;

use super::proto::{Multi, ProtoHeader};

/// Bit set on the EMsg of every protobuf-headed message.
pub const PROTO_MASK: u32 = 0x8000_0000;

/// EMsg values used by the fetcher and the game-server presence.
pub mod emsg {
    pub const MULTI: u32 = 1;
    pub const SERVICE_METHOD_RESPONSE: u32 = 147;
    pub const SERVICE_METHOD_CALL_FROM_CLIENT: u32 = 151;
    pub const CLIENT_HEART_BEAT: u32 = 703;
    pub const CLIENT_LOG_OFF: u32 = 706;
    pub const CLIENT_LOG_ON_RESPONSE: u32 = 751;
    pub const CLIENT_LOGGED_OFF: u32 = 757;
    pub const GS_STATUS_REPLY: u32 = 774;
    pub const GS_SERVER_TYPE: u32 = 908;
    pub const AM_GAME_SERVER_UPDATE: u32 = 4331;
    pub const CLIENT_TICKET_AUTH_COMPLETE: u32 = 5429;
    pub const CLIENT_REQUEST_AUTH_LIST: u32 = 5431;
    pub const CLIENT_AUTH_LIST: u32 = 5432;
    pub const CLIENT_GET_DEPOT_DECRYPTION_KEY: u32 = 5438;
    pub const CLIENT_GET_DEPOT_DECRYPTION_KEY_RESPONSE: u32 = 5439;
    pub const CLIENT_LOGON: u32 = 5514;
    pub const CLIENT_AUTH_LIST_ACK: u32 = 5575;
    pub const CLIENT_PICS_PRODUCT_INFO_REQUEST: u32 = 8903;
    pub const CLIENT_PICS_PRODUCT_INFO_RESPONSE: u32 = 8904;
    pub const CLIENT_PICS_ACCESS_TOKEN_REQUEST: u32 = 8905;
    pub const CLIENT_PICS_ACCESS_TOKEN_RESPONSE: u32 = 8906;
    pub const CLIENT_HELLO: u32 = 9805;
}

/// Largest header accepted; real headers are well under 1 KiB.
const MAX_HEADER_BYTES: usize = 64 * 1024;
/// Nesting bound for `Multi` inside `Multi`.
const MAX_MULTI_DEPTH: usize = 2;
/// Bound on messages unpacked from one `Multi`.
const MAX_MULTI_MESSAGES: usize = 4096;

/// One decoded CM message. `proto` is false for legacy non-protobuf
/// messages, whose header and body are left undecoded in `body`.
#[derive(Debug, Clone, PartialEq)]
pub struct Packet {
    pub emsg: u32,
    pub proto: bool,
    pub header: ProtoHeader,
    pub body: Vec<u8>,
}

impl Packet {
    /// Header eresult with the protocol default (2, `Fail`) when absent.
    pub fn eresult(&self) -> i32 {
        self.header.eresult.unwrap_or(2)
    }
}

/// Encode a protobuf-headed CM message.
pub fn encode(emsg: u32, header: &ProtoHeader, body: &[u8]) -> Vec<u8> {
    let header_bytes = header.encode_to_vec();
    let mut out = Vec::with_capacity(8 + header_bytes.len() + body.len());
    out.extend_from_slice(&(emsg | PROTO_MASK).to_le_bytes());
    out.extend_from_slice(&(header_bytes.len() as u32).to_le_bytes());
    out.extend_from_slice(&header_bytes);
    out.extend_from_slice(body);
    out
}

/// Decode one CM message.
pub fn decode(bytes: &[u8]) -> Result<Packet> {
    ensure!(bytes.len() >= 4, "CM message shorter than its EMsg");
    let raw = u32::from_le_bytes(bytes[..4].try_into().expect("four bytes"));
    let emsg = raw & !PROTO_MASK;
    if raw & PROTO_MASK == 0 {
        return Ok(Packet {
            emsg,
            proto: false,
            header: ProtoHeader::default(),
            body: bytes[4..].to_vec(),
        });
    }
    ensure!(
        bytes.len() >= 8,
        "CM message shorter than its header length"
    );
    let header_len = u32::from_le_bytes(bytes[4..8].try_into().expect("four bytes")) as usize;
    ensure!(
        header_len <= MAX_HEADER_BYTES && header_len <= bytes.len() - 8,
        "CM header length {header_len} exceeds the message"
    );
    let header = ProtoHeader::decode(&bytes[8..8 + header_len]).context("CM protobuf header")?;
    Ok(Packet {
        emsg,
        proto: true,
        header,
        body: bytes[8 + header_len..].to_vec(),
    })
}

/// Decode a message and expand any `Multi`, returning the inner messages in
/// order. `max_unzipped` bounds every gzip expansion.
pub fn decode_expanded(bytes: &[u8], max_unzipped: usize) -> Result<Vec<Packet>> {
    let mut out = Vec::new();
    expand(decode(bytes)?, max_unzipped, 0, &mut out)?;
    Ok(out)
}

fn expand(packet: Packet, max_unzipped: usize, depth: usize, out: &mut Vec<Packet>) -> Result<()> {
    if packet.emsg != emsg::MULTI {
        out.push(packet);
        return Ok(());
    }
    ensure!(packet.proto, "non-protobuf Multi message");
    ensure!(depth < MAX_MULTI_DEPTH, "Multi nested too deeply");
    let multi = Multi::decode(packet.body.as_slice()).context("CMsgMulti")?;
    let body = multi.message_body.unwrap_or_default();
    let payload = match multi.size_unzipped.unwrap_or(0) {
        0 => body,
        size => {
            let size = size as usize;
            ensure!(
                size <= max_unzipped,
                "Multi expands to {size} bytes, above {max_unzipped}"
            );
            let mut unzipped = Vec::with_capacity(size);
            flate2::read::GzDecoder::new(body.as_slice())
                .take(size as u64 + 1)
                .read_to_end(&mut unzipped)
                .context("gunzip Multi body")?;
            ensure!(
                unzipped.len() == size,
                "Multi unzipped to {} bytes, header says {size}",
                unzipped.len()
            );
            unzipped
        }
    };
    let mut rest = payload.as_slice();
    let mut count = 0usize;
    while !rest.is_empty() {
        count += 1;
        ensure!(count <= MAX_MULTI_MESSAGES, "Multi holds too many messages");
        if rest.len() < 4 {
            bail!("truncated Multi sub-message length");
        }
        let len = u32::from_le_bytes(rest[..4].try_into().expect("four bytes")) as usize;
        ensure!(len <= rest.len() - 4, "Multi sub-message overruns the body");
        expand(decode(&rest[4..4 + len])?, max_unzipped, depth + 1, out)?;
        rest = &rest[4 + len..];
    }
    Ok(())
}

/// Build a `Multi` body around already-framed messages (used by tests and
/// offline tooling); gzip when `gzip` is set.
pub fn encode_multi(messages: &[Vec<u8>], gzip: bool) -> Vec<u8> {
    use std::io::Write;

    let mut joined = Vec::new();
    for message in messages {
        joined.extend_from_slice(&(message.len() as u32).to_le_bytes());
        joined.extend_from_slice(message);
    }
    let multi = if gzip {
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder
            .write_all(&joined)
            .expect("writing to a Vec cannot fail");
        Multi {
            size_unzipped: Some(joined.len() as u32),
            message_body: Some(encoder.finish().expect("writing to a Vec cannot fail")),
        }
    } else {
        Multi {
            size_unzipped: None,
            message_body: Some(joined),
        }
    };
    encode(emsg::MULTI, &ProtoHeader::default(), &multi.encode_to_vec())
}

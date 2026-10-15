//! EXPERIMENTAL typed codec for the statically observed control vocabulary.
//!
//! Static client-build observations only (F-529..F-531): message layouts from
//! the client's pending-game and listen-server parsers, not wire-validated and
//! not observed from the original server. Each message is one ID byte followed
//! by consecutive archive fields read from a bit reader (bunch payloads start
//! at bit 0). Strings use the strict archive rules of `archive`; `u32` values
//! and string counts follow a caller-selected byte order (the bunch swap rule:
//! little-endian unless the sender's Hello first field is not 1), except Hello,
//! whose own first field selects the order of its version and token. Unknown
//! IDs stop decoding because their fields are unknown.
//!
//! Unique-net-ids follow the client reader (F-531, F-636): a non-encoded id
//! is one archive string after the type part, and a type-name string naming
//! None ends the id with no value. The client ignores the empty and odd bits
//! of a non-encoded id and drops an odd id's first digit unchecked; this codec
//! rejects both to keep one canonical form. Whether a default-type id is read
//! depends on the runtime's default type and is assumed. The type table is
//! unread, so type indices stay raw. No connection state, reply,
//! version selection, identity check or admission occurs. Summaries and `Debug`
//! output carry kinds and lengths only, never string or id contents.
use anyhow::{Context, Result, ensure};
use serde::Serialize;

use super::archive::{ByteOrder, MAX_STRING_UNITS, StringEncoding, read_string_frame};
use super::control::{TokenEncoding, sender_byte_order};
use super::packet::{BitReader, BitWriter};

pub const HELLO: u8 = 0;
pub const WELCOME: u8 = 1;
pub const UPGRADE: u8 = 2;
pub const CHALLENGE: u8 = 3;
pub const NETSPEED: u8 = 4;
pub const LOGIN: u8 = 5;
pub const FAILURE: u8 = 6;
pub const JOIN: u8 = 9;
pub const SPLIT_JOIN: u8 = 10;
pub const SWAP: u8 = 15;
pub const DEBUG: u8 = 17;
pub const NET_GUID_ASSIGN: u8 = 18;
pub const ENCRYPTION_ACK: u8 = 21;

/// Type index announcing a following type-name string (F-531).
pub const NAMED_NET_ID_TYPE: u8 = 31;
/// Encoded unique-net-id bytes follow a one-byte length.
pub const MAX_ENCODED_NET_ID_BYTES: usize = 255;
/// The engine's Name length limit; longer type names are left undetermined.
pub const NAME_SIZE: usize = 1024;

/// Per-call codec parameters.
#[derive(Clone, Copy, Debug)]
pub struct ControlCodec {
    /// Order of `u32` fields and string counts, except inside Hello.
    pub order: ByteOrder,
    /// Strict string cap, in declared units including the terminator.
    pub max_string_units: usize,
}

impl Default for ControlCodec {
    /// Little-endian with the archive research cap.
    fn default() -> Self {
        Self {
            order: ByteOrder::Little,
            max_string_units: MAX_STRING_UNITS,
        }
    }
}

/// A strict archive string: raw UTF-16 units plus its original encoding, so
/// frames round-trip exactly. `Debug` shows only the length and encoding.
#[derive(Clone, PartialEq, Eq)]
pub struct ArchiveText {
    units: Vec<u16>,
    encoding: TokenEncoding,
}

impl std::fmt::Debug for ArchiveText {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ArchiveText")
            .field("units", &self.units.len())
            .field("encoding", &self.encoding)
            .finish()
    }
}

impl ArchiveText {
    /// Empty for no units, narrow when every unit is ASCII, wide otherwise.
    pub fn new(units: &[u16]) -> Result<Self> {
        let encoding = if units.is_empty() {
            TokenEncoding::Empty
        } else if units.iter().all(|unit| *unit <= 0x7f) {
            TokenEncoding::Narrow
        } else {
            TokenEncoding::Wide
        };
        Self::with_encoding(units, encoding)
    }

    pub fn text(text: &str) -> Result<Self> {
        Self::new(&text.encode_utf16().collect::<Vec<_>>())
    }

    /// Explicit encoding, e.g. a terminator-only narrow or wide empty string.
    pub fn with_encoding(units: &[u16], encoding: TokenEncoding) -> Result<Self> {
        ensure!(
            units.len() < MAX_STRING_UNITS,
            "archive string exceeds research unit limit"
        );
        ensure!(
            !units.contains(&0),
            "embedded archive string NUL unsupported"
        );
        ensure!(
            !units.contains(&0xffff),
            "archive string U+FFFF normalization unsupported"
        );
        match encoding {
            TokenEncoding::Empty => ensure!(units.is_empty(), "empty encoding with units"),
            TokenEncoding::Narrow => ensure!(
                units.iter().all(|unit| *unit <= 0x7f),
                "non-ASCII narrow archive string unsupported"
            ),
            TokenEncoding::Wide => {}
        }
        Ok(Self {
            units: units.to_vec(),
            encoding,
        })
    }

    pub fn units(&self) -> &[u16] {
        &self.units
    }

    pub fn encoding(&self) -> TokenEncoding {
        self.encoding
    }

    /// Declared count magnitude, including the terminator when nonempty.
    pub fn declared_units(&self) -> usize {
        match self.encoding {
            TokenEncoding::Empty => 0,
            _ => self.units.len() + 1,
        }
    }

    fn summary(&self) -> TextSummary {
        TextSummary {
            units: self.units.len(),
            encoding: self.encoding,
        }
    }
}

/// Unique-net-id type from flag bits 3-7 (F-531); the index table is unread.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NetIdType {
    /// Index 0: the default platform type.
    Default,
    /// Raw index 1..=30 (1..=31 for `EncodedEmpty`, which reads no type part);
    /// its type name is unknown.
    Indexed(u8),
    /// Index 31: followed by a type-name archive string (never `EncodedEmpty`).
    Named(ArchiveText),
}

#[derive(Clone, PartialEq, Eq)]
pub enum NetIdValue {
    /// Encoded and empty flags: nothing follows the flags byte, not even the
    /// type part.
    EncodedEmpty,
    /// Encoded: a length byte and these bytes, rendered as uppercase hex with
    /// the leading (zero) digit dropped when `odd`.
    Encoded { odd: bool, bytes: Vec<u8> },
    /// Non-encoded: one archive string after the type part (F-636).
    Text(ArchiveText),
    /// After a type-name string naming None the client reads nothing more,
    /// in either form (F-636); `encoded` keeps the flag bit.
    Absent { encoded: bool },
}

impl std::fmt::Debug for NetIdValue {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EncodedEmpty => formatter.write_str("EncodedEmpty"),
            Self::Encoded { odd, bytes } => formatter
                .debug_struct("Encoded")
                .field("odd", odd)
                .field("bytes", &bytes.len())
                .finish(),
            Self::Text(text) => formatter.debug_tuple("Text").field(text).finish(),
            Self::Absent { encoded } => formatter
                .debug_struct("Absent")
                .field("encoded", encoded)
                .finish(),
        }
    }
}

impl NetIdValue {
    /// Encoded form from hex digits (either case). Empty input is EncodedEmpty;
    /// an odd digit count is left-padded with a zero nibble and marked odd.
    pub fn from_hex(hex: &str) -> Result<Self> {
        if hex.is_empty() {
            return Ok(Self::EncodedEmpty);
        }
        let mut nibbles = hex
            .chars()
            .map(|digit| digit.to_digit(16).map(|value| value as u8))
            .collect::<Option<Vec<u8>>>()
            .context("unique net id is not hexadecimal")?;
        let odd = !nibbles.len().is_multiple_of(2);
        if odd {
            nibbles.insert(0, 0);
        }
        let bytes: Vec<u8> = nibbles
            .chunks(2)
            .map(|pair| pair[0] << 4 | pair[1])
            .collect();
        ensure!(
            bytes.len() <= MAX_ENCODED_NET_ID_BYTES,
            "encoded unique net id exceeds 255 bytes"
        );
        Ok(Self::Encoded { odd, bytes })
    }

    /// Uppercase hex rendering of the encoded forms; `None` for text or absent.
    pub fn hex(&self) -> Option<String> {
        match self {
            Self::EncodedEmpty => Some(String::new()),
            Self::Encoded { odd, bytes } => {
                let hex: String = bytes.iter().map(|byte| format!("{byte:02X}")).collect();
                Some(if *odd { hex[1..].to_owned() } else { hex })
            }
            Self::Text(_) | Self::Absent { .. } => None,
        }
    }
}

/// Whether a type-name string makes the Name None: empty, or `None` in any
/// ASCII case with no number suffix. Names at the engine's length limit and
/// beyond are not modelled and fail closed.
fn names_none(name: &ArchiveText) -> Result<bool> {
    ensure!(
        name.units.len() < NAME_SIZE,
        "unique net id type name at or beyond the Name length limit"
    );
    Ok(name.units.is_empty()
        || (name.units.len() == 4
            && name.units.iter().zip(b"none").all(|(unit, letter)| {
                u8::try_from(*unit).is_ok_and(|unit| unit.eq_ignore_ascii_case(letter))
            })))
}

/// Replicated unique net id (F-531); `Debug` shows kinds and lengths only.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UniqueNetId {
    pub kind: NetIdType,
    pub value: NetIdValue,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ControlMessage {
    /// ID 0; `version` and `token` use `sender_byte_order(first_field)`.
    Hello {
        first_field: u8,
        version: u32,
        token: ArchiveText,
    },
    Welcome {
        map_url: ArchiveText,
        game_name: ArchiveText,
        redirect_url: ArchiveText,
    },
    Upgrade {
        version: u32,
    },
    Challenge {
        token: ArchiveText,
    },
    Netspeed {
        rate: u32,
    },
    Login {
        response: ArchiveText,
        url: ArchiveText,
        unique_net_id: UniqueNetId,
        platform: ArchiveText,
    },
    Failure {
        message: ArchiveText,
    },
    Join,
    SplitJoin {
        url: ArchiveText,
        unique_net_id: UniqueNetId,
    },
    Swap {
        value: u32,
    },
    Debug {
        text: ArchiveText,
    },
    NetGuidAssign {
        net_guid: u32,
        path: ArchiveText,
    },
    EncryptionAck,
}

/// One decoded message, or the first ID without a known layout.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Decoded {
    Message(ControlMessage),
    /// Decoding stops here; nothing after the ID byte was read.
    Unknown {
        id: u8,
    },
}

#[derive(Clone, Copy, Debug, Serialize)]
pub struct TextSummary {
    pub units: usize,
    pub encoding: TokenEncoding,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NetIdForm {
    EncodedEmpty,
    Encoded,
    Text,
    Absent,
}

#[derive(Clone, Copy, Debug, Serialize)]
pub struct UniqueNetIdSummary {
    /// Raw flag-bit type index (0 default, 31 named).
    pub type_index: u8,
    pub type_name: Option<TextSummary>,
    pub form: NetIdForm,
    pub odd: bool,
    /// Encoded byte count, text units, or 0 when absent.
    pub length: usize,
}

/// Kinds and lengths only: no string contents, ids or numeric field values.
#[derive(Clone, Debug, Serialize)]
pub struct MessageSummary {
    pub id: u8,
    pub name: &'static str,
    pub strings: Vec<TextSummary>,
    pub unique_net_id: Option<UniqueNetIdSummary>,
}

impl UniqueNetId {
    fn type_index(&self) -> u8 {
        match &self.kind {
            NetIdType::Default => 0,
            NetIdType::Indexed(index) => *index,
            NetIdType::Named(_) => NAMED_NET_ID_TYPE,
        }
    }

    pub fn summary(&self) -> UniqueNetIdSummary {
        let (form, odd, length) = match &self.value {
            NetIdValue::EncodedEmpty => (NetIdForm::EncodedEmpty, false, 0),
            NetIdValue::Encoded { odd, bytes } => (NetIdForm::Encoded, *odd, bytes.len()),
            NetIdValue::Text(text) => (NetIdForm::Text, false, text.units.len()),
            NetIdValue::Absent { .. } => (NetIdForm::Absent, false, 0),
        };
        UniqueNetIdSummary {
            type_index: self.type_index(),
            type_name: match &self.kind {
                NetIdType::Named(name) => Some(name.summary()),
                _ => None,
            },
            form,
            odd,
            length,
        }
    }
}

impl ControlMessage {
    pub fn id(&self) -> u8 {
        match self {
            Self::Hello { .. } => HELLO,
            Self::Welcome { .. } => WELCOME,
            Self::Upgrade { .. } => UPGRADE,
            Self::Challenge { .. } => CHALLENGE,
            Self::Netspeed { .. } => NETSPEED,
            Self::Login { .. } => LOGIN,
            Self::Failure { .. } => FAILURE,
            Self::Join => JOIN,
            Self::SplitJoin { .. } => SPLIT_JOIN,
            Self::Swap { .. } => SWAP,
            Self::Debug { .. } => DEBUG,
            Self::NetGuidAssign { .. } => NET_GUID_ASSIGN,
            Self::EncryptionAck => ENCRYPTION_ACK,
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Self::Hello { .. } => "hello",
            Self::Welcome { .. } => "welcome",
            Self::Upgrade { .. } => "upgrade",
            Self::Challenge { .. } => "challenge",
            Self::Netspeed { .. } => "netspeed",
            Self::Login { .. } => "login",
            Self::Failure { .. } => "failure",
            Self::Join => "join",
            Self::SplitJoin { .. } => "split_join",
            Self::Swap { .. } => "swap",
            Self::Debug { .. } => "debug",
            Self::NetGuidAssign { .. } => "net_guid_assign",
            Self::EncryptionAck => "encryption_ack",
        }
    }

    pub fn summary(&self) -> MessageSummary {
        let (strings, unique_net_id): (Vec<&ArchiveText>, Option<&UniqueNetId>) = match self {
            Self::Hello { token, .. } | Self::Challenge { token } => (vec![token], None),
            Self::Welcome {
                map_url,
                game_name,
                redirect_url,
            } => (vec![map_url, game_name, redirect_url], None),
            Self::Login {
                response,
                url,
                unique_net_id,
                platform,
            } => (vec![response, url, platform], Some(unique_net_id)),
            Self::Failure { message } => (vec![message], None),
            Self::SplitJoin { url, unique_net_id } => (vec![url], Some(unique_net_id)),
            Self::Debug { text } => (vec![text], None),
            Self::NetGuidAssign { path, .. } => (vec![path], None),
            Self::Upgrade { .. }
            | Self::Netspeed { .. }
            | Self::Swap { .. }
            | Self::Join
            | Self::EncryptionAck => (Vec::new(), None),
        };
        MessageSummary {
            id: self.id(),
            name: self.name(),
            strings: strings.into_iter().map(ArchiveText::summary).collect(),
            unique_net_id: unique_net_id.map(UniqueNetId::summary),
        }
    }
}

fn read_u32(reader: &mut BitReader<'_>, order: ByteOrder, what: &str) -> Result<u32> {
    let value = reader.read_u32();
    ensure!(!reader.is_error(), "truncated {what}");
    Ok(match order {
        ByteOrder::Little => value,
        ByteOrder::Big => value.swap_bytes(),
    })
}

fn write_u32(writer: &mut BitWriter, value: u32, order: ByteOrder) {
    writer.write_u32(match order {
        ByteOrder::Little => value,
        ByteOrder::Big => value.swap_bytes(),
    });
}

fn read_byte(reader: &mut BitReader<'_>, what: &str) -> Result<u8> {
    let value = reader.read_bits(8) as u8;
    ensure!(!reader.is_error(), "truncated {what}");
    Ok(value)
}

fn read_text(
    reader: &mut BitReader<'_>,
    order: ByteOrder,
    max_units: usize,
    what: &str,
) -> Result<ArchiveText> {
    let count = read_u32(reader, order, &format!("{what} count"))? as i32;
    ensure!(count != i32::MIN, "{what} count i32::MIN unsupported");
    let units = count.unsigned_abs() as usize;
    ensure!(units <= max_units, "{what} exceeds the string unit limit");
    // The cap bounds the allocation; overflowing reads allocate nothing.
    let payload = reader.read_bytes(units * if count < 0 { 2 } else { 1 });
    ensure!(!reader.is_error(), "truncated {what} payload");
    let prefix = match order {
        ByteOrder::Little => count.to_le_bytes(),
        ByteOrder::Big => count.to_be_bytes(),
    };
    let frame = [prefix.as_slice(), &payload].concat();
    let parsed = read_string_frame(&frame, order).with_context(|| format!("invalid {what}"))?;
    ensure!(
        parsed.consumed() == frame.len(),
        "{what} frame length mismatch"
    );
    Ok(ArchiveText {
        units: parsed.units().collect(),
        encoding: match parsed.encoding() {
            StringEncoding::Empty => TokenEncoding::Empty,
            StringEncoding::Narrow => TokenEncoding::Narrow,
            StringEncoding::Wide => TokenEncoding::Wide,
        },
    })
}

fn write_text(
    writer: &mut BitWriter,
    text: &ArchiveText,
    order: ByteOrder,
    max_units: usize,
) -> Result<()> {
    let declared = text.declared_units();
    ensure!(
        declared <= max_units,
        "archive string exceeds the string unit limit"
    );
    let count = declared as i32;
    let count = if text.encoding == TokenEncoding::Wide {
        -count
    } else {
        count
    };
    write_u32(writer, count as u32, order);
    if text.encoding == TokenEncoding::Empty {
        return Ok(());
    }
    for unit in text.units.iter().copied().chain(std::iter::once(0)) {
        if text.encoding == TokenEncoding::Wide {
            writer.write_bytes(&match order {
                ByteOrder::Little => unit.to_le_bytes(),
                ByteOrder::Big => unit.to_be_bytes(),
            });
        } else {
            writer.write_bits(u32::from(unit), 8);
        }
    }
    Ok(())
}

/// Replicated unique-net-id read (F-531). With the encoded and empty bits
/// both set the original returns right after the flags byte, so no type part
/// is read and the type bits stay raw (`Indexed(31)` is possible only there).
/// Otherwise the type part (31: a type-name string) precedes the value.
/// Strict research choices, not original checks: encoded-empty ids must not be
/// odd (the original returns before looking); odd encoded ids need at least one
/// byte with a zero leading nibble; the inferred text form rejects the empty
/// and odd bits.
pub fn read_unique_net_id(reader: &mut BitReader<'_>, codec: &ControlCodec) -> Result<UniqueNetId> {
    let flags = read_byte(reader, "unique net id flags")?;
    let (encoded, empty, odd) = (flags & 1 != 0, flags & 2 != 0, flags & 4 != 0);
    let index = flags >> 3;
    if encoded && empty {
        ensure!(!odd, "empty unique net id marked odd");
        return Ok(UniqueNetId {
            kind: match index {
                0 => NetIdType::Default,
                index => NetIdType::Indexed(index),
            },
            value: NetIdValue::EncodedEmpty,
        });
    }
    let kind = match index {
        0 => NetIdType::Default,
        NAMED_NET_ID_TYPE => NetIdType::Named(read_text(
            reader,
            codec.order,
            codec.max_string_units,
            "unique net id type name",
        )?),
        index => NetIdType::Indexed(index),
    };
    if let NetIdType::Named(name) = &kind
        && names_none(name)?
    {
        ensure!(!empty && !odd, "absent unique net id with form flags");
        return Ok(UniqueNetId {
            kind,
            value: NetIdValue::Absent { encoded },
        });
    }
    let value = if encoded {
        let length = read_byte(reader, "unique net id length")?;
        let bytes = reader.read_bytes(usize::from(length));
        ensure!(!reader.is_error(), "truncated unique net id bytes");
        if odd {
            ensure!(
                bytes.first().is_some_and(|byte| byte >> 4 == 0),
                "odd unique net id needs a zero leading nibble"
            );
        }
        NetIdValue::Encoded { odd, bytes }
    } else {
        ensure!(
            !empty && !odd,
            "non-encoded unique net id with encoded-form flags"
        );
        NetIdValue::Text(read_text(
            reader,
            codec.order,
            codec.max_string_units,
            "unique net id text",
        )?)
    };
    Ok(UniqueNetId { kind, value })
}

pub fn write_unique_net_id(
    writer: &mut BitWriter,
    id: &UniqueNetId,
    codec: &ControlCodec,
) -> Result<()> {
    let empty = id.value == NetIdValue::EncodedEmpty;
    match &id.kind {
        NetIdType::Indexed(index) if empty => ensure!(
            (1..=NAMED_NET_ID_TYPE).contains(index),
            "encoded-empty unique net id type must be 1..=31"
        ),
        NetIdType::Indexed(index) => ensure!(
            (1..NAMED_NET_ID_TYPE).contains(index),
            "indexed unique net id type must be 1..=30"
        ),
        NetIdType::Named(_) => ensure!(!empty, "encoded-empty unique net id carries no type name"),
        NetIdType::Default => {}
    }
    let named_none = match &id.kind {
        NetIdType::Named(name) => names_none(name)?,
        _ => false,
    };
    ensure!(
        named_none == matches!(id.value, NetIdValue::Absent { .. }),
        "a unique net id has no value exactly when its type name is None"
    );
    let flags = match &id.value {
        NetIdValue::EncodedEmpty => 0b011,
        NetIdValue::Encoded { odd, bytes } => {
            ensure!(
                bytes.len() <= MAX_ENCODED_NET_ID_BYTES,
                "encoded unique net id exceeds 255 bytes"
            );
            if *odd {
                ensure!(
                    bytes.first().is_some_and(|byte| byte >> 4 == 0),
                    "odd unique net id needs a zero leading nibble"
                );
            }
            0b001 | u8::from(*odd) << 2
        }
        NetIdValue::Text(_) => 0,
        NetIdValue::Absent { encoded } => u8::from(*encoded),
    };
    writer.write_bits(u32::from(flags | id.type_index() << 3), 8);
    if let NetIdType::Named(name) = &id.kind {
        write_text(writer, name, codec.order, codec.max_string_units)?;
    }
    match &id.value {
        NetIdValue::EncodedEmpty | NetIdValue::Absent { .. } => {}
        NetIdValue::Encoded { bytes, .. } => {
            writer.write_bits(bytes.len() as u32, 8);
            writer.write_bytes(bytes);
        }
        NetIdValue::Text(text) => {
            write_text(writer, text, codec.order, codec.max_string_units)?;
        }
    }
    Ok(())
}

/// Read one ID byte and that message's fields. Unknown IDs read nothing more.
pub fn read_control_message(reader: &mut BitReader<'_>, codec: &ControlCodec) -> Result<Decoded> {
    let id = read_byte(reader, "control message ID")?;
    let (order, cap) = (codec.order, codec.max_string_units);
    let text = |reader: &mut BitReader<'_>, what: &str| read_text(reader, order, cap, what);
    let message = match id {
        HELLO => {
            let first_field = read_byte(reader, "hello first field")?;
            let order = sender_byte_order(first_field);
            ControlMessage::Hello {
                first_field,
                version: read_u32(reader, order, "hello version")?,
                token: read_text(reader, order, cap, "hello token")?,
            }
        }
        WELCOME => ControlMessage::Welcome {
            map_url: text(reader, "welcome map URL")?,
            game_name: text(reader, "welcome game name")?,
            redirect_url: text(reader, "welcome redirect URL")?,
        },
        UPGRADE => ControlMessage::Upgrade {
            version: read_u32(reader, order, "upgrade version")?,
        },
        CHALLENGE => ControlMessage::Challenge {
            token: text(reader, "challenge")?,
        },
        NETSPEED => ControlMessage::Netspeed {
            rate: read_u32(reader, order, "netspeed")?,
        },
        LOGIN => ControlMessage::Login {
            response: text(reader, "login response")?,
            url: text(reader, "login URL")?,
            unique_net_id: read_unique_net_id(reader, codec).context("login unique net id")?,
            platform: text(reader, "login platform")?,
        },
        FAILURE => ControlMessage::Failure {
            message: text(reader, "failure")?,
        },
        JOIN => ControlMessage::Join,
        SPLIT_JOIN => ControlMessage::SplitJoin {
            url: text(reader, "split join URL")?,
            unique_net_id: read_unique_net_id(reader, codec).context("split join unique net id")?,
        },
        SWAP => ControlMessage::Swap {
            value: read_u32(reader, order, "swap")?,
        },
        DEBUG => ControlMessage::Debug {
            text: text(reader, "debug")?,
        },
        NET_GUID_ASSIGN => {
            let net_guid = reader.read_int_packed();
            ensure!(!reader.is_error(), "truncated net GUID");
            ControlMessage::NetGuidAssign {
                net_guid,
                path: text(reader, "net GUID path")?,
            }
        }
        ENCRYPTION_ACK => ControlMessage::EncryptionAck,
        id => return Ok(Decoded::Unknown { id }),
    };
    Ok(Decoded::Message(message))
}

/// Read messages until the reader ends or an unknown ID stops decoding.
pub fn read_control_messages(
    reader: &mut BitReader<'_>,
    codec: &ControlCodec,
) -> Result<Vec<Decoded>> {
    let mut decoded = Vec::new();
    while !reader.at_end() {
        let index = decoded.len();
        let next = read_control_message(reader, codec)
            .with_context(|| format!("control message {index}"))?;
        let stop = matches!(next, Decoded::Unknown { .. });
        decoded.push(next);
        if stop {
            break;
        }
    }
    Ok(decoded)
}

pub fn write_control_message(
    writer: &mut BitWriter,
    message: &ControlMessage,
    codec: &ControlCodec,
) -> Result<()> {
    let (order, cap) = (codec.order, codec.max_string_units);
    writer.write_bits(u32::from(message.id()), 8);
    let text = |writer: &mut BitWriter, text: &ArchiveText| write_text(writer, text, order, cap);
    match message {
        ControlMessage::Hello {
            first_field,
            version,
            token,
        } => {
            let order = sender_byte_order(*first_field);
            writer.write_bits(u32::from(*first_field), 8);
            write_u32(writer, *version, order);
            write_text(writer, token, order, cap)?;
        }
        ControlMessage::Welcome {
            map_url,
            game_name,
            redirect_url,
        } => {
            text(writer, map_url)?;
            text(writer, game_name)?;
            text(writer, redirect_url)?;
        }
        ControlMessage::Upgrade { version: value }
        | ControlMessage::Netspeed { rate: value }
        | ControlMessage::Swap { value } => write_u32(writer, *value, order),
        ControlMessage::Challenge { token: value }
        | ControlMessage::Failure { message: value }
        | ControlMessage::Debug { text: value } => text(writer, value)?,
        ControlMessage::Login {
            response,
            url,
            unique_net_id,
            platform,
        } => {
            text(writer, response)?;
            text(writer, url)?;
            write_unique_net_id(writer, unique_net_id, codec)?;
            text(writer, platform)?;
        }
        ControlMessage::Join | ControlMessage::EncryptionAck => {}
        ControlMessage::SplitJoin { url, unique_net_id } => {
            text(writer, url)?;
            write_unique_net_id(writer, unique_net_id, codec)?;
        }
        ControlMessage::NetGuidAssign { net_guid, path } => {
            writer.write_int_packed(*net_guid);
            text(writer, path)?;
        }
    }
    Ok(())
}

/// Encode one message to bytes from bit 0 (all fields are whole bytes).
pub fn encode_control_message(message: &ControlMessage, codec: &ControlCodec) -> Result<Vec<u8>> {
    let mut writer = BitWriter::new();
    write_control_message(&mut writer, message, codec)?;
    ensure!(
        writer.bit_len().is_multiple_of(8),
        "control message is not byte aligned"
    );
    Ok(writer.into_bytes())
}

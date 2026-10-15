//! EXPERIMENTAL Text items for the replication codec (F-639).
//!
//! The shared FText serializer at the archive versions client bunches carry
//! (UE4 522, editor-object custom version 40): a 4-byte flags word, a
//! history-type byte, the history body and, for Base only, the display
//! strings. Every version gate on that path passes at those versions, so the
//! layouts below are unconditional. Names take the network-name form, the
//! net archives' name operator (F-595, F-629). A TextGenerator payload is
//! an opaque counted byte array that follows unless the generator-type Name
//! is None (F-639); the hardcoded None index marks that case, and a
//! string-form None fails closed because its name-table resolution is unread.
//!
//! Bools are 4-byte words of 0 or 1; larger values fail closed, as the engine
//! flags them as an archive error. Multi-byte numbers follow the archive byte
//! order; floats and doubles keep their exact bits. Nested Text counts toward
//! the codec's depth limit and argument counts toward its array-element limit.
//! `Debug` shows history kinds and counts only.
use anyhow::{Result, bail, ensure};

use super::control_messages::ArchiveText;
use super::packet::{BitReader, BitWriter};
use super::rep_layout::{
    NetworkName, RepLayoutCodec, read_archive_string, read_field, read_network_name, read_u32,
    write_archive_string, write_network_name, write_u32,
};

/// One Text item.
#[derive(Clone, PartialEq, Eq)]
pub struct TextValue {
    pub flags: u32,
    pub history: TextHistory,
}

/// A history body; the type byte is implied by the variant.
#[derive(Clone, PartialEq, Eq)]
pub enum TextHistory {
    /// Type 0xFF: the culture-invariant string when present.
    None {
        culture_invariant: Option<ArchiveText>,
    },
    /// Type 0: namespace and key as text keys, then the source string.
    Base {
        namespace: ArchiveText,
        key: ArchiveText,
        source: ArchiveText,
    },
    /// Type 1.
    NamedFormat {
        format: Box<TextValue>,
        arguments: Vec<(ArchiveText, FormatArgument)>,
    },
    /// Type 2.
    OrderedFormat {
        format: Box<TextValue>,
        arguments: Vec<FormatArgument>,
    },
    /// Type 3.
    ArgumentDataFormat {
        format: Box<TextValue>,
        arguments: Vec<ArgumentData>,
    },
    /// Types 4 (number), 5 (percent) and 6 (currency, with its code).
    Number {
        kind: NumberKind,
        value: FormatArgument,
        options: Option<NumberOptions>,
        culture: ArchiveText,
    },
    /// Types 7 (date), 8 (time) and 9 (date and time).
    DateTime {
        kind: DateTimeKind,
        ticks: i64,
        zone: ArchiveText,
        culture: ArchiveText,
    },
    /// Type 10.
    Transform { text: Box<TextValue>, transform: u8 },
    /// Type 11: the table's Name, then the entry key.
    StringTableEntry {
        table: NetworkName,
        key: ArchiveText,
    },
    /// Type 12: the generator type's Name, then its opaque payload unless that
    /// Name is the hardcoded None.
    Generator {
        generator_type: NetworkName,
        payload: Option<Vec<u8>>,
    },
}

/// The hardcoded network-name index of None.
const NAME_NONE: u32 = 0;

#[derive(Clone, PartialEq, Eq)]
pub enum NumberKind {
    Number,
    Percent,
    Currency { code: ArchiveText },
}

/// Style bytes, signed as the engine reads them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DateTimeKind {
    Date { date_style: i8 },
    Time { time_style: i8 },
    DateTime { date_style: i8, time_style: i8 },
}

/// A format-argument value: a type byte, then its payload.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FormatArgument {
    Int(i64),
    UInt(u64),
    /// IEEE single-precision bits.
    Float(u32),
    /// IEEE double-precision bits.
    Double(u64),
    Text(Box<TextValue>),
    /// Any other type byte, which carries no payload.
    Other(i8),
}

/// An ArgumentDataFormat element: a name string, a type byte, a payload.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArgumentData {
    pub name: ArchiveText,
    pub value: ArgumentValue,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ArgumentValue {
    Int(i32),
    /// IEEE single-precision bits.
    Float(u32),
    Text(Box<TextValue>),
    Gender(u8),
    /// Any other type byte, which carries no payload.
    Other(i8),
}

/// Number formatting options, in serialized order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NumberOptions {
    pub always_sign: bool,
    pub use_grouping: bool,
    pub rounding_mode: i8,
    pub minimum_integral_digits: i32,
    pub maximum_integral_digits: i32,
    pub minimum_fractional_digits: i32,
    pub maximum_fractional_digits: i32,
}

impl TextHistory {
    pub fn type_byte(&self) -> u8 {
        match self {
            Self::None { .. } => 0xff,
            Self::Base { .. } => 0,
            Self::NamedFormat { .. } => 1,
            Self::OrderedFormat { .. } => 2,
            Self::ArgumentDataFormat { .. } => 3,
            Self::Number { kind, .. } => match kind {
                NumberKind::Number => 4,
                NumberKind::Percent => 5,
                NumberKind::Currency { .. } => 6,
            },
            Self::DateTime { kind, .. } => match kind {
                DateTimeKind::Date { .. } => 7,
                DateTimeKind::Time { .. } => 8,
                DateTimeKind::DateTime { .. } => 9,
            },
            Self::Transform { .. } => 10,
            Self::StringTableEntry { .. } => 11,
            Self::Generator { .. } => 12,
        }
    }
}

impl std::fmt::Debug for TextValue {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Text")
            .field("history", &self.history.type_byte())
            .finish_non_exhaustive()
    }
}

impl std::fmt::Debug for TextHistory {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "TextHistory({})", self.type_byte())
    }
}

impl std::fmt::Debug for NumberKind {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Number => "Number",
            Self::Percent => "Percent",
            Self::Currency { .. } => "Currency",
        })
    }
}

struct Io<'c> {
    codec: &'c RepLayoutCodec,
}

impl Io<'_> {
    fn units(&self) -> usize {
        self.codec.limits.max_string_units
    }

    fn read_string(&self, reader: &mut BitReader<'_>, what: &str) -> Result<ArchiveText> {
        read_archive_string(reader, self.codec, self.units(), what)
    }

    fn write_string(&self, writer: &mut BitWriter, text: &ArchiveText, what: &str) -> Result<()> {
        write_archive_string(writer, text, self.codec, self.units(), what)
    }

    fn read_u8(&self, reader: &mut BitReader<'_>, what: &str) -> Result<u8> {
        Ok(read_field(reader, 8, what)? as u8)
    }

    fn read_u32(&self, reader: &mut BitReader<'_>, what: &str) -> Result<u32> {
        read_u32(reader, self.codec.actor.order, what)
    }

    fn write_u32(&self, writer: &mut BitWriter, value: u32) {
        write_u32(writer, value, self.codec.actor.order);
    }

    fn read_u64(&self, reader: &mut BitReader<'_>, what: &str) -> Result<u64> {
        let first = u64::from(self.read_u32(reader, what)?);
        let second = u64::from(self.read_u32(reader, what)?);
        // Each half was read in archive order; the halves follow it too.
        Ok(match self.codec.actor.order {
            super::archive::ByteOrder::Little => second << 32 | first,
            super::archive::ByteOrder::Big => first << 32 | second,
        })
    }

    fn write_u64(&self, writer: &mut BitWriter, value: u64) {
        let (low, high) = (value as u32, (value >> 32) as u32);
        match self.codec.actor.order {
            super::archive::ByteOrder::Little => {
                self.write_u32(writer, low);
                self.write_u32(writer, high);
            }
            super::archive::ByteOrder::Big => {
                self.write_u32(writer, high);
                self.write_u32(writer, low);
            }
        }
    }

    /// The opaque-span bound, capped at the bit archives' 16 MiB array limit.
    fn max_payload_bytes(&self) -> usize {
        (self.codec.limits.max_opaque_bits / 8).min(0x0100_0000)
    }

    fn read_bool(&self, reader: &mut BitReader<'_>, what: &str) -> Result<bool> {
        match self.read_u32(reader, what)? {
            0 => Ok(false),
            1 => Ok(true),
            other => bail!("{what} bool word {other} is not 0 or 1"),
        }
    }

    fn write_bool(&self, writer: &mut BitWriter, value: bool) {
        self.write_u32(writer, u32::from(value));
    }

    fn read_count(&self, reader: &mut BitReader<'_>, what: &str) -> Result<usize> {
        let count = self.read_u32(reader, what)? as i32;
        ensure!(
            (0..=i32::from(self.codec.limits.max_array_elements)).contains(&count),
            "{what} count {count} outside 0..={}",
            self.codec.limits.max_array_elements
        );
        Ok(count as usize)
    }

    fn write_count(&self, writer: &mut BitWriter, count: usize, what: &str) -> Result<()> {
        ensure!(
            count <= usize::from(self.codec.limits.max_array_elements),
            "{what} count exceeds its limit"
        );
        self.write_u32(writer, count as u32);
        Ok(())
    }
}

/// Whether a generator-type Name is None; a string-form None fails closed.
fn generator_is_none(name: &NetworkName) -> Result<bool> {
    match name {
        NetworkName::Hardcoded(index) => Ok(*index == NAME_NONE),
        NetworkName::String { text, .. } => {
            let spelled_none = text.units().len() == 4
                && text.units().iter().zip(b"none").all(|(unit, lower)| {
                    *unit < 0x80 && (*unit as u8).to_ascii_lowercase() == *lower
                });
            ensure!(
                !spelled_none,
                "string-form None generator Name is unsupported"
            );
            Ok(false)
        }
    }
}

/// Read one Text item; `depth` counts enclosing Text items.
pub(super) fn read_text(
    reader: &mut BitReader<'_>,
    codec: &RepLayoutCodec,
    depth: usize,
) -> Result<TextValue> {
    ensure!(
        depth <= codec.limits.max_depth,
        "text nesting exceeds its depth limit"
    );
    let io = Io { codec };
    let flags = io.read_u32(reader, "text flags")?;
    let history = match io.read_u8(reader, "text history type")? {
        0xff => TextHistory::None {
            culture_invariant: io
                .read_bool(reader, "text culture-invariant flag")?
                .then(|| io.read_string(reader, "text culture-invariant string"))
                .transpose()?,
        },
        0 => TextHistory::Base {
            namespace: io.read_string(reader, "text namespace")?,
            key: io.read_string(reader, "text key")?,
            source: io.read_string(reader, "text source string")?,
        },
        1 => {
            let format = Box::new(read_text(reader, codec, depth + 1)?);
            let count = io.read_count(reader, "named arguments")?;
            let mut arguments = Vec::with_capacity(count);
            for _ in 0..count {
                let name = io.read_string(reader, "argument name")?;
                arguments.push((name, read_argument(reader, codec, depth)?));
            }
            TextHistory::NamedFormat { format, arguments }
        }
        2 => {
            let format = Box::new(read_text(reader, codec, depth + 1)?);
            let count = io.read_count(reader, "ordered arguments")?;
            let arguments = (0..count)
                .map(|_| read_argument(reader, codec, depth))
                .collect::<Result<_>>()?;
            TextHistory::OrderedFormat { format, arguments }
        }
        3 => {
            let format = Box::new(read_text(reader, codec, depth + 1)?);
            let count = io.read_count(reader, "argument data")?;
            let arguments = (0..count)
                .map(|_| read_argument_data(reader, codec, depth))
                .collect::<Result<_>>()?;
            TextHistory::ArgumentDataFormat { format, arguments }
        }
        history @ 4..=6 => {
            let kind = match history {
                4 => NumberKind::Number,
                5 => NumberKind::Percent,
                _ => NumberKind::Currency {
                    code: io.read_string(reader, "currency code")?,
                },
            };
            let value = read_argument(reader, codec, depth)?;
            let options = io
                .read_bool(reader, "number options flag")?
                .then(|| read_options(reader, &io))
                .transpose()?;
            let culture = io.read_string(reader, "target culture")?;
            TextHistory::Number {
                kind,
                value,
                options,
                culture,
            }
        }
        history @ 7..=9 => {
            let ticks = io.read_u64(reader, "date-time ticks")? as i64;
            let style =
                |reader: &mut BitReader<'_>| io.read_u8(reader, "date-time style").map(|v| v as i8);
            let kind = match history {
                7 => DateTimeKind::Date {
                    date_style: style(reader)?,
                },
                8 => DateTimeKind::Time {
                    time_style: style(reader)?,
                },
                _ => DateTimeKind::DateTime {
                    date_style: style(reader)?,
                    time_style: style(reader)?,
                },
            };
            TextHistory::DateTime {
                kind,
                ticks,
                zone: io.read_string(reader, "time zone")?,
                culture: io.read_string(reader, "target culture")?,
            }
        }
        10 => TextHistory::Transform {
            text: Box::new(read_text(reader, codec, depth + 1)?),
            transform: io.read_u8(reader, "transform type")?,
        },
        11 => TextHistory::StringTableEntry {
            table: read_network_name(reader, codec)?,
            key: io.read_string(reader, "string table key")?,
        },
        12 => {
            let generator_type = read_network_name(reader, codec)?;
            let payload = if generator_is_none(&generator_type)? {
                None
            } else {
                let count = io.read_u32(reader, "generator payload count")? as i32;
                ensure!(
                    count >= 0 && count as usize <= io.max_payload_bytes(),
                    "generator payload count {count} outside its limit"
                );
                let bytes = reader.read_bytes(count as usize);
                ensure!(!reader.is_error(), "truncated generator payload");
                Some(bytes)
            };
            TextHistory::Generator {
                generator_type,
                payload,
            }
        }
        other => bail!("text history type {other} unsupported"),
    };
    Ok(TextValue { flags, history })
}

fn read_options(reader: &mut BitReader<'_>, io: &Io<'_>) -> Result<NumberOptions> {
    Ok(NumberOptions {
        always_sign: io.read_bool(reader, "always-sign option")?,
        use_grouping: io.read_bool(reader, "grouping option")?,
        rounding_mode: io.read_u8(reader, "rounding mode")? as i8,
        minimum_integral_digits: io.read_u32(reader, "digit bound")? as i32,
        maximum_integral_digits: io.read_u32(reader, "digit bound")? as i32,
        minimum_fractional_digits: io.read_u32(reader, "digit bound")? as i32,
        maximum_fractional_digits: io.read_u32(reader, "digit bound")? as i32,
    })
}

fn read_argument(
    reader: &mut BitReader<'_>,
    codec: &RepLayoutCodec,
    depth: usize,
) -> Result<FormatArgument> {
    let io = Io { codec };
    Ok(match io.read_u8(reader, "argument type")? as i8 {
        0 => FormatArgument::Int(io.read_u64(reader, "int argument")? as i64),
        1 => FormatArgument::UInt(io.read_u64(reader, "uint argument")?),
        2 => FormatArgument::Float(io.read_u32(reader, "float argument")?),
        3 => FormatArgument::Double(io.read_u64(reader, "double argument")?),
        4 => FormatArgument::Text(Box::new(read_text(reader, codec, depth + 1)?)),
        other => FormatArgument::Other(other),
    })
}

fn read_argument_data(
    reader: &mut BitReader<'_>,
    codec: &RepLayoutCodec,
    depth: usize,
) -> Result<ArgumentData> {
    let io = Io { codec };
    let name = io.read_string(reader, "argument data name")?;
    let value = match io.read_u8(reader, "argument data type")? as i8 {
        0 => ArgumentValue::Int(io.read_u32(reader, "int argument data")? as i32),
        2 => ArgumentValue::Float(io.read_u32(reader, "float argument data")?),
        4 => ArgumentValue::Text(Box::new(read_text(reader, codec, depth + 1)?)),
        5 => ArgumentValue::Gender(io.read_u8(reader, "gender argument data")?),
        other => ArgumentValue::Other(other),
    };
    Ok(ArgumentData { name, value })
}

/// Write one Text item; `depth` counts enclosing Text items.
pub(super) fn write_text(
    writer: &mut BitWriter,
    text: &TextValue,
    codec: &RepLayoutCodec,
    depth: usize,
) -> Result<()> {
    ensure!(
        depth <= codec.limits.max_depth,
        "text nesting exceeds its depth limit"
    );
    let io = Io { codec };
    io.write_u32(writer, text.flags);
    writer.write_bits(u32::from(text.history.type_byte()), 8);
    match &text.history {
        TextHistory::None { culture_invariant } => {
            io.write_bool(writer, culture_invariant.is_some());
            if let Some(string) = culture_invariant {
                io.write_string(writer, string, "text culture-invariant string")?;
            }
        }
        TextHistory::Base {
            namespace,
            key,
            source,
        } => {
            io.write_string(writer, namespace, "text namespace")?;
            io.write_string(writer, key, "text key")?;
            io.write_string(writer, source, "text source string")?;
        }
        TextHistory::NamedFormat { format, arguments } => {
            write_text(writer, format, codec, depth + 1)?;
            io.write_count(writer, arguments.len(), "named arguments")?;
            for (name, argument) in arguments {
                io.write_string(writer, name, "argument name")?;
                write_argument(writer, argument, codec, depth)?;
            }
        }
        TextHistory::OrderedFormat { format, arguments } => {
            write_text(writer, format, codec, depth + 1)?;
            io.write_count(writer, arguments.len(), "ordered arguments")?;
            for argument in arguments {
                write_argument(writer, argument, codec, depth)?;
            }
        }
        TextHistory::ArgumentDataFormat { format, arguments } => {
            write_text(writer, format, codec, depth + 1)?;
            io.write_count(writer, arguments.len(), "argument data")?;
            for argument in arguments {
                io.write_string(writer, &argument.name, "argument data name")?;
                match &argument.value {
                    ArgumentValue::Int(value) => {
                        writer.write_bits(0, 8);
                        io.write_u32(writer, *value as u32);
                    }
                    ArgumentValue::Float(bits) => {
                        writer.write_bits(2, 8);
                        io.write_u32(writer, *bits);
                    }
                    ArgumentValue::Text(text) => {
                        writer.write_bits(4, 8);
                        write_text(writer, text, codec, depth + 1)?;
                    }
                    ArgumentValue::Gender(gender) => {
                        writer.write_bits(5, 8);
                        writer.write_bits(u32::from(*gender), 8);
                    }
                    ArgumentValue::Other(kind) => {
                        ensure!(
                            !matches!(kind, 0 | 2 | 4 | 5),
                            "argument data type {kind} needs its payload"
                        );
                        writer.write_bits(u32::from(*kind as u8), 8);
                    }
                }
            }
        }
        TextHistory::Number {
            kind,
            value,
            options,
            culture,
        } => {
            if let NumberKind::Currency { code } = kind {
                io.write_string(writer, code, "currency code")?;
            }
            write_argument(writer, value, codec, depth)?;
            io.write_bool(writer, options.is_some());
            if let Some(options) = options {
                io.write_bool(writer, options.always_sign);
                io.write_bool(writer, options.use_grouping);
                writer.write_bits(u32::from(options.rounding_mode as u8), 8);
                for bound in [
                    options.minimum_integral_digits,
                    options.maximum_integral_digits,
                    options.minimum_fractional_digits,
                    options.maximum_fractional_digits,
                ] {
                    io.write_u32(writer, bound as u32);
                }
            }
            io.write_string(writer, culture, "target culture")?;
        }
        TextHistory::DateTime {
            kind,
            ticks,
            zone,
            culture,
        } => {
            io.write_u64(writer, *ticks as u64);
            let styles: &[i8] = match kind {
                DateTimeKind::Date { date_style } => &[*date_style],
                DateTimeKind::Time { time_style } => &[*time_style],
                DateTimeKind::DateTime {
                    date_style,
                    time_style,
                } => &[*date_style, *time_style],
            };
            for style in styles {
                writer.write_bits(u32::from(*style as u8), 8);
            }
            io.write_string(writer, zone, "time zone")?;
            io.write_string(writer, culture, "target culture")?;
        }
        TextHistory::Transform { text, transform } => {
            write_text(writer, text, codec, depth + 1)?;
            writer.write_bits(u32::from(*transform), 8);
        }
        TextHistory::StringTableEntry { table, key } => {
            write_network_name(writer, table, codec)?;
            io.write_string(writer, key, "string table key")?;
        }
        TextHistory::Generator {
            generator_type,
            payload,
        } => {
            ensure!(
                generator_is_none(generator_type)? == payload.is_none(),
                "a generator payload follows exactly when its type Name is not None"
            );
            write_network_name(writer, generator_type, codec)?;
            if let Some(payload) = payload {
                ensure!(
                    payload.len() <= io.max_payload_bytes(),
                    "generator payload exceeds its limit"
                );
                io.write_u32(writer, payload.len() as u32);
                writer.write_bytes(payload);
            }
        }
    }
    Ok(())
}

fn write_argument(
    writer: &mut BitWriter,
    argument: &FormatArgument,
    codec: &RepLayoutCodec,
    depth: usize,
) -> Result<()> {
    let io = Io { codec };
    match argument {
        FormatArgument::Int(value) => {
            writer.write_bits(0, 8);
            io.write_u64(writer, *value as u64);
        }
        FormatArgument::UInt(value) => {
            writer.write_bits(1, 8);
            io.write_u64(writer, *value);
        }
        FormatArgument::Float(bits) => {
            writer.write_bits(2, 8);
            io.write_u32(writer, *bits);
        }
        FormatArgument::Double(bits) => {
            writer.write_bits(3, 8);
            io.write_u64(writer, *bits);
        }
        FormatArgument::Text(text) => {
            writer.write_bits(4, 8);
            write_text(writer, text, codec, depth + 1)?;
        }
        FormatArgument::Other(kind) => {
            ensure!(
                !(0..=4).contains(kind),
                "argument type {kind} needs its payload"
            );
            writer.write_bits(u32::from(*kind as u8), 8);
        }
    }
    Ok(())
}

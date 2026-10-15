//! EXPERIMENTAL replicator field headers and RPC parameters, against a
//! caller-supplied parameter layout.
//!
//! Static client observations only (F-637, F-638); nothing is wire-validated, no
//! original function or field list is known or shipped, and no RPC is
//! dispatched. Writers mirror the readers for round-trip research only.
//!
//! After a content block's rep-layout properties, the client replicator reads
//! field headers until its payload ends: a field index bounded like the
//! engine's `SerializeInt` by the class cache's maximum index plus one, a
//! packed payload-bit count, then exactly that payload. An unknown index is
//! skipped by its payload. Function fields carry RPC parameters; property
//! fields carry custom-delta payloads: from engine net version 11 a
//! struct-delta bit, a packed static-array index when the property is a
//! static array, then the struct's own delta serializer, left opaque here.
//!
//! RPC parameters: every parameter (each static-array element separately)
//! that is a bool is just its one bit; any other parameter has a presence bit
//! and, when set, its commands in order through the rep-layout item encodings,
//! without handles or checksums. A dynamic array is a 2-byte count, at most
//! [`RPC_ARRAY_COUNT_MAX`], then each element's commands in order. The client
//! requires a parameter payload to be consumed exactly. Which values a sender
//! marks present is not read here.
use anyhow::{Context, Result, ensure};

use super::actor_channel::read_packed;
use super::packet::{BitReader, BitWriter, Payload};
use super::rep_layout::{
    Budget, RepCommand, RepLayoutCodec, RepValue, append_bits, copy_bits, read_u16, read_value,
    validate_layout, write_u16, write_value,
};

/// Largest dynamic-array count the client accepts in RPC parameters.
pub const RPC_ARRAY_COUNT_MAX: u16 = 0xfffe;

/// One replicator field: its index and its exact payload.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FieldHeader {
    pub index: u32,
    pub payload: Payload,
}

/// Read one field header and its payload. `index_bound` is the class cache's
/// maximum index plus one; the index is below it.
pub fn read_field_header(
    reader: &mut BitReader<'_>,
    index_bound: u32,
    max_payload_bits: usize,
) -> Result<FieldHeader> {
    ensure!(index_bound >= 1, "field index bound must be at least one");
    let index = reader.read_int_max(index_bound);
    ensure!(!reader.is_error(), "truncated field index");
    let bits = read_packed(reader, "field payload bit count")? as usize;
    ensure!(
        bits <= max_payload_bits,
        "field payload exceeds the payload bit limit"
    );
    ensure!(bits <= reader.remaining(), "truncated field payload");
    Ok(FieldHeader {
        index,
        payload: copy_bits(reader, bits),
    })
}

pub fn write_field_header(
    writer: &mut BitWriter,
    header: &FieldHeader,
    index_bound: u32,
) -> Result<()> {
    ensure!(
        header.index < index_bound,
        "field index must be below its bound"
    );
    let bits = u32::try_from(header.payload.bit_len()).context("field payload too long")?;
    writer.write_int_max(header.index, index_bound)?;
    writer.write_int_packed(bits);
    append_bits(writer, &header.payload);
    Ok(())
}

/// First engine net version whose custom-delta payloads start with the
/// struct-delta bit.
pub const STRUCT_DELTA_NET_VERSION: u32 = 11;

/// The client's custom-delta payload prefix; the struct's own delta
/// serializer reads the rest and must consume it exactly.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CustomDeltaHeader {
    /// The struct-delta bit, present from [`STRUCT_DELTA_NET_VERSION`].
    pub struct_delta: Option<bool>,
    /// A packed static-array index, present when the property's array
    /// dimension is not 1; below that dimension.
    pub array_index: Option<u32>,
}

pub fn read_custom_delta_header(
    reader: &mut BitReader<'_>,
    engine_net_version: u32,
    array_dim: u32,
) -> Result<CustomDeltaHeader> {
    let struct_delta = (engine_net_version >= STRUCT_DELTA_NET_VERSION).then(|| reader.read_bit());
    ensure!(!reader.is_error(), "truncated struct-delta bit");
    let array_index = if array_dim == 1 {
        None
    } else {
        let index = read_packed(reader, "custom-delta array index")?;
        ensure!(index < array_dim, "custom-delta array index out of range");
        Some(index)
    };
    Ok(CustomDeltaHeader {
        struct_delta,
        array_index,
    })
}

pub fn write_custom_delta_header(
    writer: &mut BitWriter,
    header: &CustomDeltaHeader,
    engine_net_version: u32,
    array_dim: u32,
) -> Result<()> {
    ensure!(
        header.struct_delta.is_some() == (engine_net_version >= STRUCT_DELTA_NET_VERSION),
        "struct-delta bit presence must follow the engine net version"
    );
    match header.array_index {
        None => ensure!(array_dim == 1, "a static array needs its index"),
        Some(index) => ensure!(
            array_dim != 1 && index < array_dim,
            "custom-delta array index out of range"
        ),
    }
    if let Some(bit) = header.struct_delta {
        writer.write_bit(bit);
    }
    if let Some(index) = header.array_index {
        writer.write_int_packed(index);
    }
    Ok(())
}

/// One RPC parameter as the function's layout declares it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RpcParameter {
    /// A bool parameter: one bit, no presence bit.
    Bool,
    /// Any other parameter: its commands in order.
    Value(Vec<RepCommand>),
}

/// One command's value inside a sent parameter.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RpcItem {
    Value(RepValue),
    /// A dynamic array's elements, each its element commands' items.
    Array(Vec<Vec<RpcItem>>),
}

/// One received parameter.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RpcArgument {
    Bool(bool),
    /// The presence bit was clear; the receiver keeps the initialized value.
    NotSent,
    Sent(Vec<RpcItem>),
}

fn validate_parameters(parameters: &[RpcParameter], codec: &RepLayoutCodec) -> Result<()> {
    for (index, parameter) in parameters.iter().enumerate() {
        if let RpcParameter::Value(commands) = parameter {
            ensure!(
                !commands.is_empty(),
                "RPC parameter {index} has no commands"
            );
            validate_layout(commands, &codec.limits)
                .with_context(|| format!("RPC parameter {index}"))?;
        }
    }
    Ok(())
}

fn array_limit(codec: &RepLayoutCodec) -> u16 {
    codec.limits.max_array_elements.min(RPC_ARRAY_COUNT_MAX)
}

fn read_items(
    reader: &mut BitReader<'_>,
    commands: &[RepCommand],
    codec: &RepLayoutCodec,
    depth: usize,
    budget: &mut Budget,
) -> Result<Vec<RpcItem>> {
    ensure!(
        depth <= codec.limits.max_depth,
        "RPC parameter nesting exceeds the depth limit"
    );
    let mut items = Vec::with_capacity(commands.len());
    for command in commands {
        budget.take()?;
        items.push(match command {
            RepCommand::DynamicArray { element } => {
                let count = read_u16(reader, codec.actor.order, "RPC array count")?;
                ensure!(
                    count <= array_limit(codec),
                    "RPC array count exceeds the element limit"
                );
                let mut elements = Vec::with_capacity(usize::from(count));
                for _ in 0..count {
                    elements.push(read_items(reader, element, codec, depth + 1, budget)?);
                }
                RpcItem::Array(elements)
            }
            command => RpcItem::Value(read_value(reader, command, codec, depth, budget)?),
        });
    }
    Ok(items)
}

/// Read every parameter in order, leaving the reader after the last.
pub fn read_rpc_arguments(
    reader: &mut BitReader<'_>,
    parameters: &[RpcParameter],
    codec: &RepLayoutCodec,
) -> Result<Vec<RpcArgument>> {
    validate_parameters(parameters, codec)?;
    let mut budget = Budget {
        remaining: codec.limits.max_entries,
    };
    let mut arguments = Vec::with_capacity(parameters.len());
    for (index, parameter) in parameters.iter().enumerate() {
        let bit = reader.read_bit();
        ensure!(!reader.is_error(), "truncated RPC parameter {index}");
        arguments.push(match parameter {
            RpcParameter::Bool => RpcArgument::Bool(bit),
            RpcParameter::Value(_) if !bit => RpcArgument::NotSent,
            RpcParameter::Value(commands) => RpcArgument::Sent(
                read_items(reader, commands, codec, 0, &mut budget)
                    .with_context(|| format!("RPC parameter {index}"))?,
            ),
        });
    }
    Ok(arguments)
}

/// Parse a field payload that must hold exactly the parameters.
pub fn parse_rpc_payload(
    payload: &Payload,
    parameters: &[RpcParameter],
    codec: &RepLayoutCodec,
) -> Result<Vec<RpcArgument>> {
    let mut reader = payload.reader();
    let arguments = read_rpc_arguments(&mut reader, parameters, codec)?;
    ensure!(reader.at_end(), "trailing bits after the RPC parameters");
    Ok(arguments)
}

fn write_items(
    writer: &mut BitWriter,
    commands: &[RepCommand],
    items: &[RpcItem],
    codec: &RepLayoutCodec,
    depth: usize,
    budget: &mut Budget,
) -> Result<()> {
    ensure!(
        depth <= codec.limits.max_depth,
        "RPC parameter nesting exceeds the depth limit"
    );
    ensure!(
        commands.len() == items.len(),
        "RPC items must match their commands"
    );
    for (command, item) in commands.iter().zip(items) {
        budget.take()?;
        match (command, item) {
            (RepCommand::DynamicArray { element }, RpcItem::Array(elements)) => {
                let count = u16::try_from(elements.len())
                    .ok()
                    .filter(|count| *count <= array_limit(codec))
                    .context("RPC array count exceeds the element limit")?;
                write_u16(writer, count, codec.actor.order);
                for items in elements {
                    write_items(writer, element, items, codec, depth + 1, budget)?;
                }
            }
            (RepCommand::DynamicArray { .. }, RpcItem::Value(_)) | (_, RpcItem::Array(_)) => {
                anyhow::bail!("RPC item kind does not match its command")
            }
            (command, RpcItem::Value(value)) => {
                write_value(writer, command, value, codec, depth, budget)?;
            }
        }
    }
    Ok(())
}

pub fn write_rpc_arguments(
    writer: &mut BitWriter,
    parameters: &[RpcParameter],
    arguments: &[RpcArgument],
    codec: &RepLayoutCodec,
) -> Result<()> {
    validate_parameters(parameters, codec)?;
    ensure!(
        parameters.len() == arguments.len(),
        "RPC arguments must match their parameters"
    );
    let mut budget = Budget {
        remaining: codec.limits.max_entries,
    };
    for (index, (parameter, argument)) in parameters.iter().zip(arguments).enumerate() {
        match (parameter, argument) {
            (RpcParameter::Bool, RpcArgument::Bool(bit)) => writer.write_bit(*bit),
            (RpcParameter::Value(_), RpcArgument::NotSent) => writer.write_bit(false),
            (RpcParameter::Value(commands), RpcArgument::Sent(items)) => {
                writer.write_bit(true);
                write_items(writer, commands, items, codec, 0, &mut budget)
                    .with_context(|| format!("RPC parameter {index}"))?;
            }
            _ => anyhow::bail!("RPC parameter {index} does not match its argument"),
        }
    }
    Ok(())
}

/// Encode the parameters as one field payload.
pub fn encode_rpc_payload(
    parameters: &[RpcParameter],
    arguments: &[RpcArgument],
    codec: &RepLayoutCodec,
) -> Result<Payload> {
    let mut writer = BitWriter::new();
    write_rpc_arguments(&mut writer, parameters, arguments, codec)?;
    Ok(Payload::from_writer(writer))
}

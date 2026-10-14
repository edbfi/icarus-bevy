//! EXPERIMENTAL stateless pipeline from one data datagram to control messages.
//!
//! Composes the packet codec (`packet`) and the strict initial-body reader
//! (`control`): termination, data prefix, a client-sent packet header as the
//! server reads it, bunches, then control-channel messages. A control bunch is
//! an open bunch named with hard-coded index 255, or one on the caller's
//! control channel index (default 0); both identifications are inferred
//! (F-522, F-525). Each message is one ID byte followed by its fields as
//! consecutive archive reads from payload bit 0 (F-519, inferred). Only ID 0 is
//! decoded; the first other ID stops decoding of its bunch and is reported
//! uninterpreted, because no other field layouts are known. Partial bunches are
//! reported, not assembled.
//!
//! Static client-build observations only; not wire-validated. No connection
//! state, ack or reliable-sequence tracking, reply, version selection, identity
//! check or admission. Summaries never contain payload bytes, token contents,
//! the first field or the remote version.
use std::ops::Range;

use anyhow::{Context, Result, ensure};
use serde::Serialize;

use super::archive::{ByteOrder, MAX_STRING_UNITS};
use super::control::{
    InitialControlBody, InitialControlBodySummary, read_initial_control_body_with_sender_order,
    sender_byte_order,
};
use super::packet::{
    BitReader, Bunch, BunchLimits, Name, Receiver, read_bunches, read_data_prefix,
    read_packet_header, split_termination,
};

/// Inferred hard-coded name index of the control channel (F-522, F-525).
pub const INFERRED_CONTROL_NAME_INDEX: u32 = 255;
/// Inferred default control channel index.
pub const INFERRED_CONTROL_CHANNEL_INDEX: u32 = 0;
/// First field, remote version and string count bytes of an ID 0 body.
const INITIAL_FIXED_BYTES: usize = 1 + 4 + 4;

/// Caller-supplied parameters; the original negotiated values are unknown.
#[derive(Clone, Copy, Debug)]
pub struct ControlReceiveConfig {
    /// Optional magic-header width, 0..=32.
    pub magic_bits: u32,
    pub limits: BunchLimits,
    /// Bunches on this channel index are treated as control bunches (inferred).
    pub control_channel_index: u32,
}

impl ControlReceiveConfig {
    /// No magic header and the inferred control channel index 0.
    pub fn new(limits: BunchLimits) -> Self {
        Self {
            magic_bits: 0,
            limits,
            control_channel_index: INFERRED_CONTROL_CHANNEL_INDEX,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum NameKind {
    Absent,
    Hardcoded {
        index: u32,
    },
    /// Archive-string name; its text and number are not rendered.
    String,
}

#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ControlMessageSummary {
    /// Decoded ID 0 body through the strict reader.
    Initial(InitialControlBodySummary),
    /// First nonzero ID; `remaining_bits` follow its ID byte, uninterpreted.
    Undecoded { id: u8, remaining_bits: usize },
}

#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ControlDisposition {
    /// Not identified as a control bunch; payload not inspected.
    NotControl,
    /// A partial control bunch; reassembly is not implemented.
    PartialNotAssembled,
    Messages {
        messages: Vec<ControlMessageSummary>,
    },
}

#[derive(Debug, Serialize)]
pub struct BunchSummary {
    pub channel_index: u32,
    pub open: bool,
    pub close: bool,
    pub reliable: bool,
    pub partial: bool,
    pub partial_initial: bool,
    pub partial_final: bool,
    pub name: NameKind,
    pub payload_bits: usize,
    pub control: ControlDisposition,
}

#[derive(Debug, Serialize)]
pub struct ControlReceiveSummary {
    pub sequence: u16,
    pub acked_sequence: u16,
    pub bunch_count: usize,
    pub bunches: Vec<BunchSummary>,
    pub negotiation_validated: bool,
    pub identity_verified: bool,
    pub game_connection_established: bool,
}

/// Owned decode result; deliberately neither Debug nor Serialize because it
/// retains payloads. Use [`ControlReceive::summary`] for reporting.
pub struct ControlReceive {
    bunches: Vec<Bunch>,
    initial_spans: Vec<(usize, Range<usize>)>,
    summary: ControlReceiveSummary,
}

impl ControlReceive {
    pub fn summary(&self) -> &ControlReceiveSummary {
        &self.summary
    }

    /// Decoded ID 0 bodies in packet order, borrowing their bunch payloads.
    /// Each was already accepted by the strict reader.
    pub fn initial_bodies(&self) -> impl Iterator<Item = InitialControlBody<'_>> {
        self.initial_spans.iter().filter_map(|(bunch, span)| {
            read_initial_control_body_with_sender_order(
                &self.bunches[*bunch].payload.bytes()[span.clone()],
            )
            .ok()
        })
    }
}

fn is_control(bunch: &Bunch, config: &ControlReceiveConfig) -> bool {
    let header = &bunch.header;
    (header.open && header.channel_name == Some(Name::Hardcoded(INFERRED_CONTROL_NAME_INDEX)))
        || header.channel_index == config.control_channel_index
}

/// The ID 0 body is not length-prefixed: its extent follows from the fixed
/// fields and the string count. When those are incomplete or out of range, the
/// returned span lets the strict reader report the precise rejection.
fn initial_body_span(rest: &[u8]) -> usize {
    let Some(count) = rest.get(5..INITIAL_FIXED_BYTES) else {
        return rest.len();
    };
    // The count uses the order the strict reader will derive from the first field.
    let count = [count[0], count[1], count[2], count[3]];
    let count = match sender_byte_order(rest[0]) {
        ByteOrder::Little => i32::from_le_bytes(count),
        ByteOrder::Big => i32::from_be_bytes(count),
    };
    let units = count.unsigned_abs() as usize;
    if units > MAX_STRING_UNITS {
        return INITIAL_FIXED_BYTES;
    }
    let extent = INITIAL_FIXED_BYTES + units * if count < 0 { 2 } else { 1 };
    extent.min(rest.len())
}

/// Messages of one control bunch; returns summaries and decoded ID 0 spans.
fn read_messages(bunch: &Bunch) -> Result<(Vec<ControlMessageSummary>, Vec<Range<usize>>)> {
    let bit_len = bunch.payload.bit_len();
    // The payload starts at bit 0, so archive bytes are byte aligned.
    let bytes = &bunch.payload.bytes()[..bit_len / 8];
    let mut messages = Vec::new();
    let mut spans = Vec::new();
    let mut position = 0;
    while position < bytes.len() {
        let id = bytes[position];
        position += 1;
        if id != 0 {
            messages.push(ControlMessageSummary::Undecoded {
                id,
                remaining_bits: bit_len - position * 8,
            });
            return Ok((messages, spans));
        }
        let index = messages.len();
        let span = position..position + initial_body_span(&bytes[position..]);
        let body = read_initial_control_body_with_sender_order(&bytes[span.clone()])
            .with_context(|| format!("control message {index} (ID 0)"))?;
        messages.push(ControlMessageSummary::Initial(body.summary()));
        position = span.end;
        spans.push(span);
    }
    ensure!(
        bit_len.is_multiple_of(8),
        "control payload ends inside a message ID byte"
    );
    Ok((messages, spans))
}

/// Decode one terminated data datagram sent by a client, as a server reads it.
///
/// A handshake datagram fails with a downcastable `packet::HandshakePacket`.
/// Any malformed packet, bunch or ID 0 body fails the whole datagram.
pub fn read_control_datagram(
    datagram: &[u8],
    config: &ControlReceiveConfig,
) -> Result<ControlReceive> {
    let (bytes, bit_len) = split_termination(datagram)?;
    let mut reader = BitReader::new(bytes, bit_len)?;
    read_data_prefix(&mut reader, config.magic_bits)?;
    let (header, _info) = read_packet_header(&mut reader, Receiver::Server)?;
    let bunches = read_bunches(&mut reader, &config.limits)?;
    let mut summaries = Vec::with_capacity(bunches.len());
    let mut initial_spans = Vec::new();
    for (index, bunch) in bunches.iter().enumerate() {
        let control = if !is_control(bunch, config) {
            ControlDisposition::NotControl
        } else if bunch.header.partial {
            ControlDisposition::PartialNotAssembled
        } else {
            let (messages, spans) =
                read_messages(bunch).with_context(|| format!("control bunch {index}"))?;
            initial_spans.extend(spans.into_iter().map(|span| (index, span)));
            ControlDisposition::Messages { messages }
        };
        let header = &bunch.header;
        summaries.push(BunchSummary {
            channel_index: header.channel_index,
            open: header.open,
            close: header.close,
            reliable: header.reliable,
            partial: header.partial,
            partial_initial: header.partial_initial,
            partial_final: header.partial_final,
            name: match &header.channel_name {
                None => NameKind::Absent,
                Some(Name::Hardcoded(index)) => NameKind::Hardcoded { index: *index },
                Some(Name::String { .. }) => NameKind::String,
            },
            payload_bits: bunch.payload.bit_len(),
            control,
        });
    }
    Ok(ControlReceive {
        summary: ControlReceiveSummary {
            sequence: header.sequence,
            acked_sequence: header.acked_sequence,
            bunch_count: bunches.len(),
            bunches: summaries,
            negotiation_validated: false,
            identity_verified: false,
            game_connection_established: false,
        },
        bunches,
        initial_spans,
    })
}

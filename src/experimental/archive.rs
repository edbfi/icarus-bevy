//! Bounded, strict archive-string subset; not outer UDP/control-channel framing.
//!
//! Archive byte order must be supplied by the caller. Unsupported normalizations
//! are rejected rather than silently changing a name or identity-bearing value.
use anyhow::{Result, ensure};

/// Research resource policy, including the terminating unit when present.
pub const MAX_STRING_UNITS: usize = 65_536;

#[derive(Clone, Copy, Debug)]
pub enum ByteOrder {
    Little,
    Big,
}

#[derive(Debug, PartialEq, Eq)]
pub struct ArchiveString {
    /// Raw UTF-16 units without the terminator. Surrogates are preserved.
    pub units: Vec<u16>,
    /// Bytes consumed, including the signed count. Following fields are untouched.
    pub consumed: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StringEncoding {
    /// Zero signed count, without a payload or terminator.
    Empty,
    Narrow,
    Wide,
}

/// Validated original spans, deliberately neither Debug nor Serialize.
/// Validation and unit iteration do not allocate or normalize text.
pub struct ArchiveStringFrame<'a> {
    frame: &'a [u8],
    count: i32,
    order: ByteOrder,
}

impl<'a> ArchiveStringFrame<'a> {
    pub fn frame(&self) -> &'a [u8] {
        self.frame
    }

    pub fn prefix(&self) -> &'a [u8] {
        &self.frame[..4]
    }

    /// Original signed count, including the terminator for nonzero counts.
    pub fn signed_count(&self) -> i32 {
        self.count
    }

    pub fn declared_units(&self) -> usize {
        self.count.unsigned_abs() as usize
    }

    /// Number of content units, excluding the terminator.
    pub fn unit_count(&self) -> usize {
        self.declared_units().saturating_sub(1)
    }

    pub fn encoding(&self) -> StringEncoding {
        match self.count {
            0 => StringEncoding::Empty,
            1.. => StringEncoding::Narrow,
            _ => StringEncoding::Wide,
        }
    }

    /// Original payload, including the terminator when present.
    pub fn payload(&self) -> &'a [u8] {
        &self.frame[4..]
    }

    pub fn content(&self) -> &'a [u8] {
        let payload = self.payload();
        let terminator_bytes = match self.encoding() {
            StringEncoding::Empty => 0,
            StringEncoding::Narrow => 1,
            StringEncoding::Wide => 2,
        };
        &payload[..payload.len() - terminator_bytes]
    }

    pub fn terminator(&self) -> Option<&'a [u8]> {
        match self.encoding() {
            StringEncoding::Empty => None,
            StringEncoding::Narrow => Some(&self.frame[self.frame.len() - 1..]),
            StringEncoding::Wide => Some(&self.frame[self.frame.len() - 2..]),
        }
    }

    pub fn consumed(&self) -> usize {
        self.frame.len()
    }

    /// Iterate copied UTF-16 content units without allocating. Unpaired
    /// surrogates are preserved; this is not Unicode scalar/text conversion.
    pub fn units(&self) -> impl DoubleEndedIterator<Item = u16> + ExactSizeIterator + '_ {
        let wide = self.count < 0;
        self.content()
            .chunks(if wide { 2 } else { 1 })
            .map(move |bytes| {
                if wide {
                    let unit = [bytes[0], bytes[1]];
                    match self.order {
                        ByteOrder::Little => u16::from_le_bytes(unit),
                        ByteOrder::Big => u16::from_be_bytes(unit),
                    }
                } else {
                    u16::from(bytes[0])
                }
            })
    }
}

/// Borrow a validated zero-length, positive ASCII or negative UTF-16 frame.
///
/// Nonzero counts include the terminator. Unlike the observed original reader,
/// this strict subset rejects missing/embedded NUL, U+FFFF and non-ASCII narrow
/// bytes. No success-path heap allocation or normalization occurs. Following
/// fields are untouched; exact input EOF is the caller's policy.
pub fn read_string_frame(input: &[u8], order: ByteOrder) -> Result<ArchiveStringFrame<'_>> {
    let prefix: [u8; 4] = input
        .get(..4)
        .ok_or_else(|| anyhow::anyhow!("truncated archive string count"))?
        .try_into()?;
    let count = match order {
        ByteOrder::Little => i32::from_le_bytes(prefix),
        ByteOrder::Big => i32::from_be_bytes(prefix),
    };
    let magnitude = count.unsigned_abs() as usize;
    ensure!(
        magnitude <= MAX_STRING_UNITS,
        "archive string exceeds research unit limit"
    );
    if magnitude == 0 {
        return Ok(ArchiveStringFrame {
            frame: &input[..4],
            count,
            order,
        });
    }
    let wide = count < 0;
    // The unit limit bounds this multiplication and every later slice.
    let consumed = 4 + magnitude * if wide { 2 } else { 1 };
    let payload = input
        .get(4..consumed)
        .ok_or_else(|| anyhow::anyhow!("truncated archive string payload"))?;
    if !wide {
        ensure!(
            payload.is_ascii(),
            "non-ASCII narrow archive string unsupported"
        );
    }
    let frame = ArchiveStringFrame {
        frame: &input[..consumed],
        count,
        order,
    };
    ensure!(
        frame
            .terminator()
            .is_some_and(|bytes| bytes.iter().all(|byte| *byte == 0)),
        "missing archive string terminator"
    );
    // Preserve legacy precedence: missing terminator, then any embedded NUL,
    // then U+FFFF, even when U+FFFF occurs before an embedded NUL in the source.
    ensure!(
        frame.units().all(|unit| unit != 0),
        "embedded archive string NUL unsupported"
    );
    ensure!(
        frame.units().all(|unit| unit != 0xffff),
        "archive string U+FFFF normalization unsupported"
    );
    Ok(frame)
}

/// Materialize the strict frame's UTF-16 content units when the caller needs
/// owned units. Acceptance, error precedence and consumed bytes match framing.
pub fn decode_string(input: &[u8], order: ByteOrder) -> Result<ArchiveString> {
    let frame = read_string_frame(input, order)?;
    Ok(ArchiveString {
        units: frame.units().collect(),
        consumed: frame.consumed(),
    })
}

/// Encode the strict subset with ASCII preferred and UTF-16 for other units.
/// Forced-wide archive flags are not implemented. Empty input uses count zero.
pub fn encode_string(units: &[u16], order: ByteOrder) -> Result<Vec<u8>> {
    validate_units(units)?;
    if units.is_empty() {
        return Ok(vec![0; 4]);
    }
    let wide = units.iter().any(|unit| *unit > 127);
    let magnitude = (units.len() + 1) as i32;
    let count = if wide { -magnitude } else { magnitude };
    let prefix = match order {
        ByteOrder::Little => count.to_le_bytes(),
        ByteOrder::Big => count.to_be_bytes(),
    };
    let mut output = Vec::with_capacity(4 + magnitude as usize * if wide { 2 } else { 1 });
    output.extend_from_slice(&prefix);
    for unit in units.iter().copied().chain(std::iter::once(0)) {
        if wide {
            output.extend_from_slice(&match order {
                ByteOrder::Little => unit.to_le_bytes(),
                ByteOrder::Big => unit.to_be_bytes(),
            });
        } else {
            output.push(unit as u8);
        }
    }
    Ok(output)
}

fn validate_units(units: &[u16]) -> Result<()> {
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
    Ok(())
}

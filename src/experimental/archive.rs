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

/// Decode zero length, positive ASCII count or negative UTF-16 count.
///
/// Nonzero counts include the terminator. Unlike the observed original reader,
/// this strict subset rejects missing/embedded NUL, U+FFFF and non-ASCII narrow
/// bytes. It does not attempt identity-changing normalization.
pub fn decode_string(input: &[u8], order: ByteOrder) -> Result<ArchiveString> {
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
        return Ok(ArchiveString {
            units: Vec::new(),
            consumed: 4,
        });
    }
    let wide = count < 0;
    // The unit limit bounds this multiplication and every later allocation.
    let consumed = 4 + magnitude * if wide { 2 } else { 1 };
    let payload = input
        .get(4..consumed)
        .ok_or_else(|| anyhow::anyhow!("truncated archive string payload"))?;
    let units: Vec<u16> = if wide {
        payload
            .as_chunks::<2>()
            .0
            .iter()
            .map(|bytes| match order {
                ByteOrder::Little => u16::from_le_bytes(*bytes),
                ByteOrder::Big => u16::from_be_bytes(*bytes),
            })
            .collect()
    } else {
        ensure!(
            payload.is_ascii(),
            "non-ASCII narrow archive string unsupported"
        );
        payload.iter().copied().map(u16::from).collect()
    };
    ensure!(
        units.last() == Some(&0),
        "missing archive string terminator"
    );
    let mut units = units;
    units.pop();
    validate_units(&units)?;
    Ok(ArchiveString { units, consumed })
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

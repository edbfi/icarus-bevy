//! Offline check of pawn field-30 `ClientMoveResponsePacked` bits against the
//! strict move-response codec (`experimental::character_move`): each item is
//! decoded, re-encoded and compared bit for bit. No networking; it reads one
//! local file and writes only to stdout.
//!
//! ```text
//! move_response_check --byte-order little|big --engine-net-version N [--input payload|field] INPUT
//! ```
//!
//! Each line of `INPUT` holds one item as `BITS HEX` (decimal bit length,
//! then the bytes as hex) or as a JSON object with `payload_bits` and
//! `payload_hex`, the form `capture_decode` writes. Bits run LSB-first from
//! bit 0 of the first byte, as `Payload::bytes` stores them; the hex must have
//! exactly `ceil(BITS / 8)` bytes with zero unused high bits. Blank lines and
//! lines starting with `#` are skipped. `--input payload` (the default) means
//! the field-30 parameter payload from its presence bit; `--input field`
//! means a content block payload holding field 30 alone (index, count,
//! payload). `--byte-order` is the archive byte order the capture tooling
//! uses; `--engine-net-version` selects the base bone's name encoding.
//!
//! Stdout: one JSON line per item with `line`, `bits` and either `kind`
//! (`ack` or `correction`), `present` (the optional parts present) and
//! `reencode` (`identical` or `differs`), or `refused` with a static reason;
//! then a `summary` line of counts. It prints no positions, timestamps,
//! identifiers or other decoded values.
use std::{
    fs::File,
    io::{BufRead, BufReader, Read, Write},
    path::PathBuf,
};

use anyhow::{Context, Result, ensure};
use clap::{Parser, ValueEnum};
use icarus_bevy::experimental::{
    archive::ByteOrder,
    character_move::{
        MAX_MOVE_RESPONSE_DATA_BITS, MoveResponseInput, MoveResponseKind, check_move_response,
        move_response_codec,
    },
    packet::{BitReader, BitWriter, Payload},
    rep_layout::RepLayoutCodec,
};
use serde_json::{Value, json};

/// Local resource policy, not a property of the original.
const MAX_FILE_BYTES: u64 = 16 * 1024 * 1024;
const MAX_LINES: usize = 100_000;
/// Field index (7), two count fields (at most 40 bits each), the cap.
const MAX_ITEM_BITS: usize = 7 + 40 + 1 + 40 + MAX_MOVE_RESPONSE_DATA_BITS as usize;

#[derive(Clone, Copy, Debug, ValueEnum)]
enum Order {
    Little,
    Big,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum Input {
    Payload,
    Field,
}

#[derive(Parser)]
#[command(about = "Check field-30 move responses against the strict codec")]
struct Options {
    #[arg(long, value_enum)]
    byte_order: Order,
    #[arg(long)]
    engine_net_version: u32,
    #[arg(long, value_enum, default_value = "payload")]
    input: Input,
    path: PathBuf,
}

fn decode_hex(text: &str) -> Result<Vec<u8>> {
    ensure!(
        text.len().is_multiple_of(2) && text.is_ascii(),
        "input: invalid hex"
    );
    (0..text.len())
        .step_by(2)
        .map(|start| u8::from_str_radix(&text[start..start + 2], 16).context("input: invalid hex"))
        .collect()
}

/// One line's bits, or `None` for a blank or comment line.
fn parse_line(line: &str) -> Result<Option<Payload>> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return Ok(None);
    }
    let (bits, hex) = if line.starts_with('{') {
        let value: Value = serde_json::from_str(line).context("input: invalid JSON")?;
        let bits = value
            .get("payload_bits")
            .and_then(Value::as_u64)
            .context("input: payload_bits missing")?;
        let hex = value
            .get("payload_hex")
            .and_then(Value::as_str)
            .context("input: payload_hex missing")?;
        (
            usize::try_from(bits).context("input: bit length too large")?,
            hex.to_owned(),
        )
    } else {
        let mut parts = line.split_whitespace();
        let (Some(bits), Some(hex), None) = (parts.next(), parts.next(), parts.next()) else {
            anyhow::bail!("input: expected BITS HEX");
        };
        (
            bits.parse().context("input: invalid bit length")?,
            hex.to_owned(),
        )
    };
    ensure!(
        bits <= MAX_ITEM_BITS,
        "input: bit length above the item cap"
    );
    let bytes = decode_hex(&hex)?;
    ensure!(
        bytes.len() == bits.div_ceil(8),
        "input: bit length does not match the hex"
    );
    if !bits.is_multiple_of(8) {
        let last = bytes[bytes.len() - 1];
        ensure!(last >> (bits % 8) == 0, "input: nonzero padding bits");
    }
    let mut reader = BitReader::new(&bytes, bits)?;
    let mut writer = BitWriter::new();
    for _ in 0..bits {
        writer.write_bit(reader.read_bit());
    }
    Ok(Some(Payload::from_writer(writer)))
}

#[derive(Default)]
struct Summary {
    items: usize,
    skipped: usize,
    acks: usize,
    corrections: usize,
    refused: usize,
    reencode_differs: usize,
}

/// Check every line, returning the per-item rows and the summary row.
fn process(
    input: impl BufRead,
    kind: MoveResponseInput,
    codec: &RepLayoutCodec,
) -> Result<Vec<Value>> {
    let mut rows = Vec::new();
    let mut summary = Summary::default();
    for (index, line) in input.lines().enumerate() {
        ensure!(index < MAX_LINES, "input exceeds the line cap");
        let line = line.context("input is not UTF-8 text")?;
        let number = index + 1;
        let payload = match parse_line(&line) {
            Ok(Some(payload)) => payload,
            Ok(None) => {
                summary.skipped += 1;
                continue;
            }
            Err(error) => {
                summary.items += 1;
                summary.refused += 1;
                rows.push(json!({"line": number, "bits": null, "refused": error.to_string()}));
                continue;
            }
        };
        summary.items += 1;
        match check_move_response(&payload, kind, codec) {
            Ok(check) => {
                match check.kind {
                    MoveResponseKind::Ack => summary.acks += 1,
                    MoveResponseKind::Correction => summary.corrections += 1,
                }
                if !check.reencode_identical {
                    summary.reencode_differs += 1;
                }
                rows.push(
                    json!({"line": number, "bits": check.bits, "kind": check.kind,
                    "present": check.present,
                    "reencode": if check.reencode_identical { "identical" } else { "differs" }}),
                );
            }
            Err(error) => {
                summary.refused += 1;
                rows.push(json!({"line": number, "bits": payload.bit_len(),
                    "refused": error.to_string()}));
            }
        }
    }
    rows.push(
        json!({"summary": {"items": summary.items, "skipped": summary.skipped,
        "acks": summary.acks, "corrections": summary.corrections, "refused": summary.refused,
        "reencode_differs": summary.reencode_differs}}),
    );
    Ok(rows)
}

fn main() -> Result<()> {
    let options = Options::parse();
    let order = match options.byte_order {
        Order::Little => ByteOrder::Little,
        Order::Big => ByteOrder::Big,
    };
    let kind = match options.input {
        Input::Payload => MoveResponseInput::Payload,
        Input::Field => MoveResponseInput::Field,
    };
    let file = File::open(&options.path).context("cannot open the input")?;
    ensure!(
        file.metadata().context("cannot stat the input")?.len() <= MAX_FILE_BYTES,
        "input exceeds the file cap"
    );
    let input = BufReader::new(file.take(MAX_FILE_BYTES));
    let codec = move_response_codec(options.engine_net_version, order);
    let rows = process(input, kind, &codec)?;
    let mut stdout = std::io::stdout().lock();
    for row in rows {
        writeln!(stdout, "{row}")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use icarus_bevy::experimental::character_move::{
        MOVEMENT_MODE_FALLING, MoveCorrection, write_good_move_ack, write_move_correction,
        write_move_response_field,
    };

    use super::*;

    fn line(payload: &Payload) -> String {
        let hex: String = payload
            .bytes()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        format!("{} {hex}", payload.bit_len())
    }

    fn correction() -> MoveCorrection {
        MoveCorrection {
            timestamp: 47.023,
            location: [-123_456.78, 98_765.4, 31_000.5],
            velocity: [0.0, 0.0, -981.0],
            rotation: None,
            base: Some(0x2a6),
            bone: None,
            mode: Some(MOVEMENT_MODE_FALLING),
            base_relative: false,
            root_motion: None,
        }
    }

    /// Authored acknowledgements and corrections in both input forms and
    /// both byte orders are classified and re-encode identically; malformed
    /// lines are refused; rows carry only the documented keys.
    #[test]
    fn authored_lines_are_checked_without_values() {
        for order in [ByteOrder::Little, ByteOrder::Big] {
            let codec = move_response_codec(17, order);
            let ack = write_good_move_ack(1.5, order);
            let fix = write_move_correction(&correction(), &codec).unwrap();
            let json_line = format!(
                r#"{{"payload_bits":{},"payload_hex":"{}"}}"#,
                fix.bit_len(),
                line(&fix).split(' ').nth(1).unwrap()
            );
            let text = [
                "# authored".to_owned(),
                line(&ack),
                String::new(),
                line(&fix),
                json_line,
                "8 zz".to_owned(),
                "3 0f".to_owned(),
                "16 00".to_owned(),
                "1 00".to_owned(),
            ]
            .join("\n");
            let rows = process(text.as_bytes(), MoveResponseInput::Payload, &codec).unwrap();
            assert_eq!(rows.len(), 8);
            assert_eq!(rows[0]["kind"], "ack");
            assert_eq!(rows[0]["bits"], 42);
            for row in &rows[1..3] {
                assert_eq!(row["kind"], "correction");
                assert_eq!(row["bits"], 258 + 16);
                assert_eq!(row["present"], json!(["base", "mode"]));
                assert_eq!(row["reencode"], "identical");
            }
            assert_eq!(rows[3]["refused"], "input: invalid hex");
            assert_eq!(rows[4]["refused"], "input: nonzero padding bits");
            assert_eq!(
                rows[5]["refused"],
                "input: bit length does not match the hex"
            );
            assert_eq!(rows[6]["refused"], "move response parameter absent");
            assert_eq!(
                rows[7]["summary"],
                json!({"items": 7, "skipped": 2, "acks": 1, "corrections": 2,
                    "refused": 4, "reencode_differs": 0})
            );
            for row in &rows[..7] {
                let keys: Vec<&str> = row
                    .as_object()
                    .unwrap()
                    .keys()
                    .map(String::as_str)
                    .collect();
                assert!(
                    keys.iter().all(|key| [
                        "line", "bits", "kind", "present", "reencode", "refused"
                    ]
                    .contains(key)),
                    "{keys:?}"
                );
            }
            // Field form: field 30 alone.
            let field = write_move_response_field(&fix).unwrap();
            let rows = process(line(&field).as_bytes(), MoveResponseInput::Field, &codec).unwrap();
            assert_eq!(rows[0]["kind"], "correction");
            assert_eq!(rows[0]["reencode"], "identical");
            // A payload offered as a field is refused.
            let rows = process(line(&fix).as_bytes(), MoveResponseInput::Field, &codec).unwrap();
            assert!(rows[0]["refused"].is_string());
        }
    }
}

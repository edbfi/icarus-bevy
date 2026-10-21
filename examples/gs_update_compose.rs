//! Re-compose GameState property updates from `capture_decode` detail rows
//! (`--values-channel`, `update_actor_values`) with
//! `experimental::game_state_open::compose_game_state_update` and compare
//! each with the recorded bunch body. Rows whose values are not all `int` or
//! `float` scalars are counted and skipped. Prints counts only; never reads
//! original files.
use std::{
    collections::BTreeMap,
    fs::File,
    io::{BufRead, BufReader},
    path::PathBuf,
};

use anyhow::{Context, Result};
use clap::Parser;
use icarus_bevy::experimental::{
    archive::ByteOrder,
    game_state_open::compose_game_state_update,
    rep_layout::{MovementLevel, RepValue, RotationLevel, icarus_game_state_survival_rep_layout},
};
use serde_json::{Value, json};

#[derive(Parser)]
struct Options {
    /// `capture_decode` detail JSONL.
    detail: PathBuf,
    /// The decode pass whose rows are compared.
    #[arg(long, default_value = "export_hypothesis")]
    pass: String,
}

/// The capture's engine network version (as `capture_decode`).
const ENGINE_NET_VERSION: u32 = 17;

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Whether the hex body `recorded` starts with the first `bits` bits of
/// `ours` (bits fill each byte from its least significant end).
fn bit_prefix(recorded: &str, ours: &[u8], bits: usize) -> bool {
    let Some(recorded) = (0..recorded.len() / 2)
        .map(|index| u8::from_str_radix(recorded.get(index * 2..index * 2 + 2)?, 16).ok())
        .collect::<Option<Vec<u8>>>()
    else {
        return false;
    };
    (0..bits).all(|bit| {
        let (byte, shift) = (bit / 8, bit % 8);
        recorded.get(byte).map(|value| (value >> shift) & 1)
            == ours.get(byte).map(|value| (value >> shift) & 1)
    })
}

/// A scalar entry's value, or `None` for any other kind.
fn scalar(value: &Value) -> Option<RepValue> {
    if let Some(number) = value.get("int").and_then(Value::as_i64) {
        return i32::try_from(number).ok().map(RepValue::Int);
    }
    value
        .get("float")
        .and_then(Value::as_f64)
        .map(|number| RepValue::Float((number as f32).to_bits()))
}

fn main() -> Result<()> {
    let options = Options::parse();
    let layout = icarus_game_state_survival_rep_layout(
        MovementLevel::Whole,
        MovementLevel::Whole,
        RotationLevel::Bytes,
    );
    let reader = BufReader::new(File::open(&options.detail).context("open the detail file")?);
    let (mut compared, mut exact, mut leading, mut skipped) = (0u64, 0u64, 0u64, 0u64);
    let mut handle_sets: BTreeMap<String, u64> = BTreeMap::new();
    for line in reader.lines() {
        let row: Value = serde_json::from_str(&line?)?;
        if row["kind"] != "update_actor_values"
            || row["schema"] != "AIcarusGameStateSurvival"
            || row["pass"] != options.pass.as_str()
        {
            continue;
        }
        let Some(entries) = row["entries"]["Ok"].as_array() else {
            skipped += 1;
            continue;
        };
        let values: Option<BTreeMap<u32, RepValue>> = entries
            .iter()
            .map(|entry| {
                let handle = u32::try_from(entry[0].as_u64()?).ok()?;
                Some((handle, scalar(&entry[1])?))
            })
            .collect();
        let Some(values) = values else {
            skipped += 1;
            continue;
        };
        compared += 1;
        let key = format!("{:?}", values.keys().collect::<Vec<_>>());
        *handle_sets.entry(key.clone()).or_default() += 1;
        let payload =
            compose_game_state_update(&layout, ENGINE_NET_VERSION, &values, ByteOrder::Little)?;
        let same = row["body_bits"].as_u64() == Some(payload.bit_len() as u64)
            && row["body_hex"].as_str() == Some(hex(payload.bytes()).as_str());
        exact += u64::from(same);
        // A body with more blocks (components) starts with the actor block.
        let leads = !same
            && row["body_bits"]
                .as_u64()
                .is_some_and(|bits| bits > payload.bit_len() as u64)
            && row["body_hex"]
                .as_str()
                .is_some_and(|recorded| bit_prefix(recorded, payload.bytes(), payload.bit_len()));
        leading += u64::from(leads);
    }
    println!(
        "{}",
        json!({"compared": compared, "exact": exact,
        "actor_block_leads_longer_body": leading, "skipped": skipped,
        "handle_sets": handle_sets})
    );
    Ok(())
}

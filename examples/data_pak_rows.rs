//! List row names of the official dedicated server's `data.pak`, so an
//! operator can author a binding such as `network.player_inventory_projection`.
//! Input parity (F-828, F-993) allows row names and their order only: this
//! tool calls only the reader's name functions (`DataPak::members`, `has`,
//! `member_rows`, `table_rows`) and never reads, prints or derives any other
//! field or value of a row. Read-only, no network; the reader's own limits
//! bound the pak and each table, and output stops after [`MAX_LINES`] lines.
//!
//! A unique name is not enough for a projection binding: startup also refuses
//! a row past its row handle, that is a `D_InventoryInfo` row at index 256 or
//! above, or a `D_TagQueries` or `D_ItemsStatic` row at index 65535 or above.
//! The printed index shows which rows those are.
//!
//! ```text
//! data_pak_rows --data-pak PATH                               # tables and row counts
//! data_pak_rows --data-pak PATH --table NAME                  # row names in pak order
//! data_pak_rows --data-pak PATH --table NAME --contains TEXT  # case-insensitive filter
//! ```
use std::{
    collections::BTreeMap,
    io::{BufWriter, Write},
    path::PathBuf,
};

use anyhow::{Context, Result, ensure};
use clap::Parser;
use icarus_bevy::data_pak::DataPak;
use serde_json::json;

#[cfg(test)]
#[allow(dead_code)]
#[path = "../tests/pak_writer/mod.rs"]
mod pak_writer;

/// Most listing lines printed before the final omission line.
const MAX_LINES: usize = 20_000;

#[derive(Parser)]
#[command(about = "List data.pak table names or one table's row names (names only)")]
struct Options {
    /// The official dedicated server's `Icarus/Content/Data/data.pak`.
    #[arg(long)]
    data_pak: PathBuf,
    /// A table name as bindings name it (for example `D_ItemsStatic`).
    #[arg(long)]
    table: Option<String>,
    /// Only rows whose name contains this text, case-insensitively.
    #[arg(long, requires = "table")]
    contains: Option<String>,
}

fn main() -> Result<()> {
    let options = Options::parse();
    let mut pak = DataPak::open(&options.data_pak).context("open --data-pak")?;
    let lines = match &options.table {
        None => table_lines(&mut pak)?,
        Some(table) => row_lines(&mut pak, table, options.contains.as_deref())?,
    };
    let mut out = BufWriter::new(std::io::stdout().lock());
    write_capped(&mut out, &lines, MAX_LINES)?;
    out.flush()?;
    Ok(())
}

/// One line per data table: its name as bindings and `network` resolve it
/// (the member's file stem; `DataPak::table_rows` reads `{table}.json`) and
/// its row count. A stem shared by several members cannot be resolved by
/// name and is marked `"ambiguous": true`. A member whose path is the tail of
/// another member's path cannot even be read by name, so it is listed
/// ambiguous without a row count. JSON members without `Rows` are not tables
/// and are skipped.
fn table_lines(pak: &mut DataPak) -> Result<Vec<serde_json::Value>> {
    let members: Vec<String> = pak.members().map(str::to_owned).collect();
    let mut stems: BTreeMap<String, usize> = BTreeMap::new();
    let mut tables = Vec::new();
    for member in &members {
        let file = member.rsplit('/').next().unwrap_or(member);
        let Some(stem) = file.strip_suffix(".json") else {
            continue;
        };
        *stems.entry(stem.to_owned()).or_default() += 1;
        // The reader matches members by path suffix, so this one is unreadable.
        let shadowed = members
            .iter()
            .any(|other| other.ends_with(&format!("/{member}")));
        if shadowed {
            tables.push((stem.to_owned(), None));
            continue;
        }
        let rows = pak
            .member_rows(member)
            .with_context(|| format!("data table member {member}"))?;
        if let Some(rows) = rows {
            tables.push((stem.to_owned(), Some(rows.len())));
        }
    }
    tables.sort();
    Ok(tables
        .into_iter()
        .map(|(table, rows)| {
            let mut line = json!({"table": table});
            if let Some(rows) = rows {
                line["rows"] = json!(rows);
            }
            if stems[&table] > 1 {
                line["ambiguous"] = json!(true);
            }
            line
        })
        .collect())
}

/// One line per row of `table` in pak order: its zero-based index (the row
/// handle) and name, `"duplicate": true` when the name occurs more than once
/// in the whole table. `contains` filters case-insensitively and keeps the
/// pak index.
fn row_lines(
    pak: &mut DataPak,
    table: &str,
    contains: Option<&str>,
) -> Result<Vec<serde_json::Value>> {
    ensure!(
        pak.has(&format!("{table}.json")),
        "data pak has no table {table}"
    );
    let rows = pak
        .table_rows(table)
        .with_context(|| format!("data table {table}"))?;
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for name in &rows {
        *counts.entry(name.as_str()).or_default() += 1;
    }
    let needle = contains.map(str::to_lowercase);
    Ok(rows
        .iter()
        .enumerate()
        .filter(|(_, name)| {
            needle
                .as_deref()
                .is_none_or(|needle| name.to_lowercase().contains(needle))
        })
        .map(|(index, name)| {
            let mut line = json!({"index": index, "name": name});
            if counts[name.as_str()] > 1 {
                line["duplicate"] = json!(true);
            }
            line
        })
        .collect())
}

/// Write at most `cap` lines, then one line counting any omitted.
fn write_capped(out: &mut impl Write, lines: &[serde_json::Value], cap: usize) -> Result<()> {
    for line in lines.iter().take(cap) {
        writeln!(out, "{line}")?;
    }
    let omitted = lines.len().saturating_sub(cap);
    if omitted > 0 {
        writeln!(
            out,
            "{}",
            json!({"event": "lines_omitted", "omitted": omitted, "limit": cap})
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pak(members: &[(&str, Vec<u8>)]) -> (tempfile::TempDir, DataPak) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("data.pak");
        let borrowed: Vec<(&str, &[u8], bool)> = members
            .iter()
            .map(|(path, bytes)| (*path, bytes.as_slice(), true))
            .collect();
        std::fs::write(&path, pak_writer::write_pak(&borrowed)).unwrap();
        let pak = DataPak::open(&path).unwrap();
        (dir, pak)
    }

    fn authored() -> (tempfile::TempDir, DataPak) {
        pak(&[
            (
                "Items/D_ItemsStatic.json",
                pak_writer::table_json(&[
                    "Item_Water",
                    "Item_Stone",
                    "item_WATERSKIN",
                    "Item_Stone",
                ]),
            ),
            (
                "Inventory/D_InventoryInfo.json",
                pak_writer::table_json(&["Bag"]),
            ),
            (
                "Tags/D_TagQueries.json",
                pak_writer::table_json(&["Any", "Fists"]),
            ),
            ("Meta/Notes.json", br#"{"Note": "not a table"}"#.to_vec()),
            ("Meta/Readme.txt", b"not json".to_vec()),
        ])
    }

    fn text(lines: &[serde_json::Value], cap: usize) -> Vec<String> {
        let mut out = Vec::new();
        write_capped(&mut out, lines, cap).unwrap();
        String::from_utf8(out)
            .unwrap()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    #[test]
    fn lists_tables_with_row_counts() {
        let (_dir, mut pak) = authored();
        assert_eq!(
            text(&table_lines(&mut pak).unwrap(), MAX_LINES),
            [
                r#"{"rows":1,"table":"D_InventoryInfo"}"#,
                r#"{"rows":4,"table":"D_ItemsStatic"}"#,
                r#"{"rows":2,"table":"D_TagQueries"}"#,
            ]
        );
    }

    #[test]
    fn marks_a_table_name_shared_by_two_members_ambiguous() {
        let (_dir, mut pak) = pak(&[
            ("A/D_Same.json", pak_writer::table_json(&["One"])),
            ("B/D_Same.json", pak_writer::table_json(&["Two"])),
        ]);
        let lines = table_lines(&mut pak).unwrap();
        assert_eq!(lines.len(), 2);
        assert!(lines.iter().all(|line| line["ambiguous"] == json!(true)));
    }

    #[test]
    fn a_member_shadowed_by_a_longer_path_is_listed_ambiguous_without_rows() {
        let (_dir, mut pak) = pak(&[
            ("D_Same.json", pak_writer::table_json(&["One"])),
            ("Sub/D_Same.json", pak_writer::table_json(&["Two", "Three"])),
        ]);
        assert_eq!(
            text(&table_lines(&mut pak).unwrap(), MAX_LINES),
            [
                r#"{"ambiguous":true,"table":"D_Same"}"#,
                r#"{"ambiguous":true,"rows":2,"table":"D_Same"}"#,
            ]
        );
    }

    #[test]
    fn lists_rows_in_pak_order_with_indices_and_duplicates() {
        let (_dir, mut pak) = authored();
        assert_eq!(
            text(
                &row_lines(&mut pak, "D_ItemsStatic", None).unwrap(),
                MAX_LINES
            ),
            [
                r#"{"index":0,"name":"Item_Water"}"#,
                r#"{"duplicate":true,"index":1,"name":"Item_Stone"}"#,
                r#"{"index":2,"name":"item_WATERSKIN"}"#,
                r#"{"duplicate":true,"index":3,"name":"Item_Stone"}"#,
            ]
        );
    }

    #[test]
    fn contains_is_case_insensitive_and_keeps_pak_indices() {
        let (_dir, mut pak) = authored();
        assert_eq!(
            text(
                &row_lines(&mut pak, "D_ItemsStatic", Some("wAtEr")).unwrap(),
                MAX_LINES
            ),
            [
                r#"{"index":0,"name":"Item_Water"}"#,
                r#"{"index":2,"name":"item_WATERSKIN"}"#,
            ]
        );
        let stone = row_lines(&mut pak, "D_ItemsStatic", Some("STONE")).unwrap();
        assert_eq!(stone.len(), 2);
        assert!(stone.iter().all(|line| line["duplicate"] == json!(true)));
    }

    #[test]
    fn unknown_table_is_an_error() {
        let (_dir, mut pak) = authored();
        let error = row_lines(&mut pak, "D_Missing", None).unwrap_err();
        assert!(error.to_string().contains("no table D_Missing"));
        // A JSON member without `Rows` is not a table either.
        assert!(row_lines(&mut pak, "Notes", None).is_err());
    }

    #[test]
    fn line_cap_reports_omitted_lines() {
        let (_dir, mut pak) = authored();
        let lines = row_lines(&mut pak, "D_ItemsStatic", None).unwrap();
        let out = text(&lines, 3);
        assert_eq!(out.len(), 4);
        assert_eq!(out[2], r#"{"index":2,"name":"item_WATERSKIN"}"#);
        assert_eq!(out[3], r#"{"event":"lines_omitted","limit":3,"omitted":1}"#);
        // Exactly at the cap: no omission line.
        assert_eq!(text(&lines, 4).len(), 4);
    }
}

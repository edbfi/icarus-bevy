//! Write an independently authored gameplay profile (`network.authored_gameplay`,
//! F-993) for a fresh player over the official server's `data.pak`: row
//! names and order from the pak's catalog, every value from the flags below
//! (`gameplay_profile::fresh_player_profile`), or with `--skeleton TABLE`
//! names-only rows of the listed tables for the operator to author.
//! The written file is then loaded the way `run` and the join probe load it,
//! and the stat count a fresh pawn would carry is printed. Prints counts and
//! stat row positions (protocol identity) only, never a packaged value.
use std::{io::Write, path::PathBuf};

use anyhow::{Context, Result, ensure};
use clap::Parser;
use icarus_bevy::{
    config::NetworkConfig,
    data_pak::DataPak,
    gameplay_profile::{FreshPlayerValues, catalog_skeleton, fresh_player_profile},
    network,
    pawn_stats::STATS_TABLE,
};
use serde_json::json;

#[derive(Parser)]
#[command(about = "Write an authored gameplay profile over a data.pak catalog")]
struct Options {
    /// The official dedicated server's `Icarus/Content/Data/data.pak`.
    #[arg(long)]
    data_pak: PathBuf,
    /// New profile path; an existing file is never replaced.
    #[arg(long)]
    output: PathBuf,
    /// Instead of a fresh-player profile, names-only rows of these tables.
    #[arg(long)]
    skeleton: Vec<String>,
    #[arg(long, default_value_t = FreshPlayerValues::default().walk_speed)]
    walk_speed: i32,
    #[arg(long, default_value_t = FreshPlayerValues::default().sprint_speed)]
    sprint_speed: i32,
    #[arg(long, default_value_t = FreshPlayerValues::default().crouch_speed)]
    crouch_speed: i32,
    #[arg(long, default_value_t = FreshPlayerValues::default().swim_speed)]
    swim_speed: i32,
    #[arg(long, default_value_t = FreshPlayerValues::default().swim_sprint_speed)]
    swim_sprint_speed: i32,
    #[arg(long, default_value_t = FreshPlayerValues::default().max_health)]
    max_health: i32,
    #[arg(long, default_value_t = FreshPlayerValues::default().max_stamina)]
    max_stamina: i32,
    #[arg(long, default_value_t = FreshPlayerValues::default().max_oxygen)]
    max_oxygen: i32,
    #[arg(long, default_value_t = FreshPlayerValues::default().max_water)]
    max_water: i32,
    #[arg(long, default_value_t = FreshPlayerValues::default().max_food)]
    max_food: i32,
    #[arg(long, default_value_t = FreshPlayerValues::default().weight_capacity)]
    weight_capacity: i32,
}

impl Options {
    fn values(&self) -> FreshPlayerValues {
        FreshPlayerValues {
            walk_speed: self.walk_speed,
            sprint_speed: self.sprint_speed,
            crouch_speed: self.crouch_speed,
            swim_speed: self.swim_speed,
            swim_sprint_speed: self.swim_sprint_speed,
            max_health: self.max_health,
            max_stamina: self.max_stamina,
            max_oxygen: self.max_oxygen,
            max_water: self.max_water,
            max_food: self.max_food,
            weight_capacity: self.weight_capacity,
        }
    }
}

fn main() -> Result<()> {
    let options = Options::parse();
    let values = options.values();
    // Refused before the pak is opened or anything is written.
    values.validate()?;
    let mut pak = DataPak::open(&options.data_pak).context("open --data-pak")?;
    let skeleton = !options.skeleton.is_empty();
    let profile = if skeleton {
        let tables: Vec<&str> = options.skeleton.iter().map(String::as_str).collect();
        catalog_skeleton(&mut pak, &tables)?
    } else {
        fresh_player_profile(&mut pak, &values)?
    };
    let rows: serde_json::Map<String, serde_json::Value> = profile["tables"]
        .as_object()
        .context("profile tables")?
        .iter()
        .map(|(table, rows)| (table.clone(), json!(rows.as_array().map_or(0, Vec::len))))
        .collect();
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&options.output)
        .context("create --output (an existing file is never replaced)")?;
    file.write_all(&serde_json::to_vec_pretty(&profile)?)?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    drop(file);
    let mut summary =
        json!({"event": "gameplay_profile_written", "skeleton": skeleton, "table_rows": rows});
    if !skeleton {
        // Load it as `run` and the join probe do; a profile they would
        // refuse is removed and reported as a failure.
        let verified = verify(&options, &values);
        if verified.is_err() {
            std::fs::remove_file(&options.output).context("remove the refused profile")?;
        }
        let (count, ids) = verified?;
        summary["fresh_pawn_stats"] = json!(count);
        summary["movement_stat_ids"] = json!(ids);
    }
    println!("{summary}");
    Ok(())
}

/// The written profile's fresh pawn stat count and the five speed stats'
/// IDs (their `D_Stats` row positions, F-838), loaded as `run` loads it.
fn verify(
    options: &Options,
    values: &FreshPlayerValues,
) -> Result<(usize, serde_json::Map<String, serde_json::Value>)> {
    let mut tables = network::data_tables(&NetworkConfig {
        data_pak: Some(options.data_pak.clone()),
        authored_gameplay: Some(options.output.clone()),
        ..NetworkConfig::default()
    })?
    .context("tables configured")?;
    let count = network::fresh_pawn_stat_count(Some(&mut tables))?;
    ensure!(count > 0, "the profile gives a fresh pawn no stats");
    let names = tables.rows(STATS_TABLE)?;
    let ids = values
        .grants()
        .iter()
        .take(5)
        .filter_map(|(_, derived, _)| {
            let id = names.iter().position(|name| name == derived)?;
            Some(((*derived).to_owned(), json!(id)))
        })
        .collect();
    Ok((count, ids))
}

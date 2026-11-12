//! An independently authored gameplay profile (`network.authored_gameplay`,
//! F-993) that lets a fresh player's pawn open carry replicated stats
//! (F-839) over the official server's `data.pak`. Row names and their order
//! come from the pak's catalog (protocol identity, F-830); every value comes
//! from [`FreshPlayerValues`]. Nothing of a packaged row is read but its name.
use std::collections::BTreeMap;

use anyhow::{Context, Result, ensure};
use serde_json::{Map, Value, json};

use crate::data_pak::DataPak;
use crate::network::{
    FACTION_MISSIONS_TABLE, INVENTORY_TABLE, MODIFIER_STATES_TABLE, PLAYER_INVENTORY_ROW,
    QUESTS_TABLE, STARTING_STATS_ROW, STARTING_STATS_TABLE,
};
use crate::pawn_stats::{REPLICATED_FIELD, STATS_TABLE};

/// The authored gameplay schema `DataTables::runtime` reads.
pub const SCHEMA_VERSION: u32 = 1;

/// The tables startup and a fresh join (with no starting inventories) read
/// as objects, so a profile must supply them: the first four always, the
/// quests and modifier states whenever the pak holds them (startup reads
/// them only then). A loaded world's or a returning player's restore reads
/// more.
pub const FRESH_JOIN_TABLES: [&str; 6] = [
    FACTION_MISSIONS_TABLE,
    STATS_TABLE,
    STARTING_STATS_TABLE,
    INVENTORY_TABLE,
    QUESTS_TABLE,
    MODIFIER_STATES_TABLE,
];

/// The tables a fresh join cannot do without.
const REQUIRED_TABLES: [&str; 4] = [
    FACTION_MISSIONS_TABLE,
    STATS_TABLE,
    STARTING_STATS_TABLE,
    INVENTORY_TABLE,
];

/// A fresh player's starting values, chosen by the operator (lab defaults
/// are round numbers, not ICARUS balance values). Speeds are cm/s.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FreshPlayerValues {
    pub walk_speed: i32,
    pub sprint_speed: i32,
    pub crouch_speed: i32,
    pub swim_speed: i32,
    pub swim_sprint_speed: i32,
    pub max_health: i32,
    pub max_stamina: i32,
    pub max_oxygen: i32,
    pub max_water: i32,
    pub max_food: i32,
    pub weight_capacity: i32,
}

impl Default for FreshPlayerValues {
    fn default() -> Self {
        Self {
            walk_speed: 600,
            sprint_speed: 900,
            crouch_speed: 300,
            swim_speed: 300,
            swim_sprint_speed: 450,
            max_health: 100,
            max_stamina: 100,
            max_oxygen: 100,
            max_water: 100,
            max_food: 100,
            weight_capacity: 1_000_000,
        }
    }
}

/// The most any one value may be: well inside the formulas' 32-bit
/// arithmetic.
pub const MAX_VALUE: i32 = 10_000_000;

impl FreshPlayerValues {
    /// Each value as the starting grant of a base stat, with the stat the
    /// formulas derive from it (F-838): with no percentage or coefficient
    /// granted, the derived stat equals its base.
    pub fn grants(&self) -> [(&'static str, &'static str, i32); 11] {
        [
            ("BaseMovementSpeed_+", "MovementSpeed_+", self.walk_speed),
            ("BaseSprintSpeed_+", "SprintSpeed_+", self.sprint_speed),
            ("BaseCrouchSpeed_+", "CrouchSpeed_+", self.crouch_speed),
            ("BaseSwimSpeed_+", "SwimSpeed_+", self.swim_speed),
            (
                "BaseSwimSprintSpeed_+",
                "SwimSprintSpeed_+",
                self.swim_sprint_speed,
            ),
            ("BaseMaximumHealth_+", "MaximumHealth_+", self.max_health),
            ("BaseMaximumStamina_+", "MaximumStamina_+", self.max_stamina),
            ("BaseMaximumOxygen_+", "MaximumOxygen_+", self.max_oxygen),
            ("BaseMaximumWater_+", "MaximumWater_+", self.max_water),
            ("BaseMaximumFood_+", "MaximumFood_+", self.max_food),
            (
                "BaseWeightCapacity_+",
                "WeightCapacity_+",
                self.weight_capacity,
            ),
        ]
    }

    /// Every value positive and at most [`MAX_VALUE`]: a zero speed or
    /// maximum is what leaves a character unable to move (or dead).
    pub fn validate(&self) -> Result<()> {
        for (base, _, value) in self.grants() {
            ensure!(
                (1..=MAX_VALUE).contains(&value),
                "{base} must be 1..={MAX_VALUE}"
            );
        }
        Ok(())
    }
}

/// `table`'s catalog rows as names-only objects, in catalog order.
fn names_only(pak: &mut DataPak, table: &str) -> Result<Vec<Map<String, Value>>> {
    Ok(pak
        .table_rows(table)
        .with_context(|| format!("read the {table} catalog"))?
        .into_iter()
        .map(|name| Map::from_iter([("Name".to_owned(), Value::String(name))]))
        .collect())
}

/// The row named `name` of `rows`, which the catalog must list.
fn row_mut<'a>(
    rows: &'a mut [Map<String, Value>],
    table: &str,
    name: &str,
) -> Result<&'a mut Map<String, Value>> {
    rows.iter_mut()
        .find(|row| row.get("Name").and_then(Value::as_str) == Some(name))
        .with_context(|| format!("the {table} catalog has no {name} row"))
}

/// A names-only skeleton of `tables` over `pak`'s catalog, as
/// `{"schema_version": 1, "tables": {...}}`: each table's rows in catalog
/// order with only their `Name`, for the operator to author values into.
/// Every listed table must be in the pak.
pub fn catalog_skeleton(pak: &mut DataPak, tables: &[&str]) -> Result<Value> {
    let mut out = BTreeMap::new();
    for table in tables {
        out.insert((*table).to_owned(), names_only(pak, table)?);
    }
    Ok(json!({"schema_version": SCHEMA_VERSION, "tables": out}))
}

/// A complete profile for a fresh join over `pak`'s catalog:
/// [`FRESH_JOIN_TABLES`] the pak holds as names-only rows, then
/// `values` authored in. `D_CharacterStartingStats`'s `Base_Stats` grants
/// each base stat of [`FreshPlayerValues::grants`]; `D_Stats` replicates
/// those bases and the stats derived from them (so a fresh pawn carries 22
/// stats, the five speeds among them), and no other; `D_Inventory`'s `Player`
/// lists no inventories. Every other row and field is omitted, so the
/// native parser's defaults apply.
pub fn fresh_player_profile(pak: &mut DataPak, values: &FreshPlayerValues) -> Result<Value> {
    values.validate()?;
    for table in REQUIRED_TABLES {
        ensure!(
            pak.has(&format!("{table}.json")),
            "the data pak has no {table} table"
        );
    }
    let present: Vec<&str> = FRESH_JOIN_TABLES
        .into_iter()
        .filter(|table| pak.has(&format!("{table}.json")))
        .collect();
    let mut tables = BTreeMap::new();
    for table in &present {
        tables.insert(*table, names_only(pak, table)?);
    }
    let stats = tables.get_mut(STATS_TABLE).context("stats table")?;
    for (base, derived, _) in values.grants() {
        for name in [base, derived] {
            row_mut(stats, STATS_TABLE, name)?.insert(REPLICATED_FIELD.to_owned(), true.into());
        }
    }
    let granted: Map<String, Value> = values
        .grants()
        .iter()
        .map(|(base, _, value)| (format!("(Value=\"{base}\")"), Value::from(*value)))
        .collect();
    let starting = tables
        .get_mut(STARTING_STATS_TABLE)
        .context("starting stats table")?;
    row_mut(starting, STARTING_STATS_TABLE, STARTING_STATS_ROW)?
        .insert("StatsGranted".to_owned(), Value::Object(granted));
    let inventory = tables.get_mut(INVENTORY_TABLE).context("inventory table")?;
    row_mut(inventory, INVENTORY_TABLE, PLAYER_INVENTORY_ROW)?
        .insert("Inventories".to_owned(), Value::Array(Vec::new()));
    Ok(json!({"schema_version": SCHEMA_VERSION, "tables": tables}))
}

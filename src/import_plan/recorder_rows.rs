//! Row identities and order for the two recorder fields with a qualified table.

use std::collections::BTreeMap;

use anyhow::{Result, ensure};
use serde::Deserialize;

use crate::data_pak::MAX_TABLE_ROWS;
use crate::experimental::rep_layout::ICARUS_ROW_HANDLE_NONE;
use crate::network::{FACTION_MISSIONS_TABLE, QUESTS_TABLE};

use super::{MAX_CATALOG_REFERENCE_UNITS, ascii_lower_unit, fold_case};

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RecorderRowTablesFile {
    #[serde(rename = "D_Quests")]
    quests: Option<Vec<String>>,
    #[serde(rename = "D_FactionMissions")]
    faction_missions: Option<Vec<String>>,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum RecorderRowTable {
    Quests,
    FactionMissions,
}

impl RecorderRowTable {
    pub(super) fn for_recorder(class: &str) -> Option<Self> {
        match class {
            "C019" => Some(Self::Quests),
            "C018" => Some(Self::FactionMissions),
            _ => None,
        }
    }

    pub(super) fn field(self) -> &'static str {
        match self {
            Self::Quests => "QuestName",
            Self::FactionMissions => "FactionMissionName",
        }
    }

    pub(super) fn for_pak_member(path: &str) -> Option<Self> {
        let name = path.rsplit('/').next()?.strip_suffix(".json")?;
        match name {
            QUESTS_TABLE => Some(Self::Quests),
            FACTION_MISSIONS_TABLE => Some(Self::FactionMissions),
            _ => None,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Quests => QUESTS_TABLE,
            Self::FactionMissions => FACTION_MISSIONS_TABLE,
        }
    }

    fn row_key(self, name: &[u16]) -> Vec<u16> {
        match self {
            Self::Quests => fold_case(name),
            Self::FactionMissions => name.to_vec(),
        }
    }

    fn same_row_name(self, left: &[u16], right: &[u16]) -> bool {
        match self {
            Self::Quests => left
                .iter()
                .copied()
                .map(ascii_lower_unit)
                .eq(right.iter().copied().map(ascii_lower_unit)),
            Self::FactionMissions => left == right,
        }
    }

    fn index_fits(self, index: usize) -> bool {
        match self {
            Self::Quests => index < usize::from(ICARUS_ROW_HANDLE_NONE),
            Self::FactionMissions => u8::try_from(index).is_ok(),
        }
    }
}

#[derive(Clone, Copy)]
pub(super) enum ScopedTableIssue {
    Unusable,
    Conflicting,
}

#[derive(Clone, Default)]
pub(super) struct RecorderRows {
    tables: BTreeMap<RecorderRowTable, OrderedRows>,
    issues: BTreeMap<RecorderRowTable, ScopedTableIssue>,
}

#[derive(Clone)]
struct OrderedRows {
    authored: bool,
    names: Vec<Vec<u16>>,
    first_indices: BTreeMap<Vec<u16>, usize>,
}

impl OrderedRows {
    fn new(table: RecorderRowTable, names: Vec<Vec<u16>>, authored: bool) -> Self {
        let mut first_indices = BTreeMap::new();
        for (index, name) in names.iter().enumerate() {
            first_indices.entry(table.row_key(name)).or_insert(index);
        }
        Self {
            authored,
            names,
            first_indices,
        }
    }
}

impl RecorderRows {
    pub(super) fn compile(tables: RecorderRowTablesFile) -> Result<Self> {
        let mut compiled = Self::default();
        for (table, names) in [
            (RecorderRowTable::Quests, tables.quests),
            (RecorderRowTable::FactionMissions, tables.faction_missions),
        ] {
            let Some(names) = names else { continue };
            let table_name = table.name();
            ensure!(
                names.len() <= MAX_TABLE_ROWS,
                "content catalog {table_name} has too many rows"
            );
            let names: Vec<Vec<u16>> = names
                .into_iter()
                .map(|name| name.encode_utf16().collect())
                .collect();
            for (index, name) in names.iter().enumerate() {
                ensure!(
                    !name.is_empty() && name.len() <= MAX_CATALOG_REFERENCE_UNITS,
                    "content catalog {table_name} row {index} must be 1..={MAX_CATALOG_REFERENCE_UNITS} UTF-16 units"
                );
            }
            compiled
                .tables
                .insert(table, OrderedRows::new(table, names, true));
        }
        Ok(compiled)
    }

    /// Present members without one usable ordered row list block only their
    /// scoped references. Absence leaves the authored table available.
    pub(super) fn attach_pak(&mut self, tables: BTreeMap<RecorderRowTable, Option<Vec<String>>>) {
        for (table, names) in tables {
            let Some(names) = names else {
                self.issues
                    .entry(table)
                    .or_insert(ScopedTableIssue::Unusable);
                continue;
            };
            let names: Vec<Vec<u16>> = names
                .into_iter()
                .map(|name| name.encode_utf16().collect())
                .collect();
            if let Some(existing) = self.tables.get(&table) {
                if existing.names.len() != names.len()
                    || !existing
                        .names
                        .iter()
                        .zip(&names)
                        .all(|(left, right)| table.same_row_name(left, right))
                {
                    self.issues.insert(table, ScopedTableIssue::Conflicting);
                }
            } else {
                self.tables
                    .insert(table, OrderedRows::new(table, names, false));
            }
        }
    }

    pub(super) fn contains(
        &self,
        table: RecorderRowTable,
        name: &[u16],
    ) -> Result<bool, ScopedTableIssue> {
        if let Some(&issue) = self.issues.get(&table) {
            return Err(issue);
        }
        let Some(rows) = self.tables.get(&table) else {
            return Ok(false);
        };
        let key = match table {
            // Valid UTF-16 table names cannot match an invalid quest name.
            RecorderRowTable::Quests => fold_case(name),
            // DataTables::row_index compares the lossy conversion exactly.
            RecorderRowTable::FactionMissions => {
                String::from_utf16_lossy(name).encode_utf16().collect()
            }
        };
        Ok(rows
            .first_indices
            .get(&key)
            .is_some_and(|&index| table.index_fits(index)))
    }

    pub(super) fn authored_references(&self) -> impl Iterator<Item = &[u16]> {
        self.tables
            .values()
            .filter(|rows| rows.authored)
            .flat_map(|rows| rows.names.iter().map(Vec::as_slice))
    }
}

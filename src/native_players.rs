//! Independently authored checkpoints. Keys are private; construction is not authentication.
//!
//! A store is bound to its world (prospect and map) and to one digest per
//! operator-authored input that defines gameplay, so a refusal names the input
//! that changed. The game release is provenance only: it is recorded when a
//! store is opened or created, written with it and never compared, so a new
//! game release alone never refuses a store. A checkpoint's growth row is
//! stored as its table and row name, never as an index, and is resolved
//! against the installed rows when the store is opened ([`PlayerSnapshot::open`]).
use crate::experimental::{
    pawn_open::CharacterVitals,
    release::{DEDICATED_SERVER_APP_ID, ReleasePin, network_checksum},
};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::{collections::BTreeSet, sync::Arc};

pub const MAX_PLAYERS: usize = 4096;
pub const MAX_PLAYERS_PER_ACCOUNT: usize = 64;
/// Conservative validated-state/encoding allowance, not a peak JSON-parser/RSS claim.
pub const MAX_PLAYER_BYTES: usize = 4 * 1024 * 1024;
/// The game-data table a checkpoint's `growth_row` indexes.
pub const GROWTH_TABLE: &str = crate::network::CHARACTER_GROWTH_TABLE;
/// The only `CurrentBiome` value a checkpoint holds. The server loads no biome
/// table and opens every pawn with this value, so the biome is not player
/// state: it is never stored, and a checkpoint with another value refuses
/// rather than being stored with a meaning the installed data could change.
pub const FRESH_BIOME_ROW: u8 = 0;
/// Bounds of the installed growth-row names a context retains.
const MAX_GROWTH_ROWS: usize = 65_536;
const MAX_ROW_NAME_BYTES: usize = 1024;
const MAX_GROWTH_ROW_BYTES: usize = 1024 * 1024;
/// A 1-byte row handle reaches rows 0..=255; more distinct stored names are invalid.
const MAX_STORED_GROWTH_ROWS: usize = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlayerError {
    InvalidIdentity,
    InvalidContext,
    InvalidState,
    Limit,
    Duplicate,
    Missing,
    Conflict,
    StaleRevision,
    Exhausted,
}
impl std::fmt::Display for PlayerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "authored player checkpoint refused ({self:?})")
    }
}
impl std::error::Error for PlayerError {}

/// Why [`PlayerSnapshot::open`] refused a stored player store. Every variant
/// leaves the store unchanged; none resets or guesses a field.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StoreRefusal {
    /// `network.player_persistence.prospect_id` differs from the store's.
    Prospect,
    /// `network.player_persistence.map_url` differs from the store's.
    Map,
    /// These authored inputs differ from the ones the store was created with.
    Inputs(Vec<&'static str>),
    /// A stored row reference names a row the installed data does not have.
    MissingRow {
        table: &'static str,
        row: String,
    },
    /// A stored row reference resolves past the 1-byte row handle range.
    RowOutOfRange {
        table: &'static str,
        row: String,
        index: usize,
    },
    Invalid(PlayerError),
}
impl std::fmt::Display for StoreRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        const ASIDE: &str = "restore it or move the state directory aside to start a fresh store";
        match self {
            Self::Prospect => write!(
                f,
                "player store belongs to another world: network.player_persistence.prospect_id differs from the store's; {ASIDE}"
            ),
            Self::Map => write!(
                f,
                "player store belongs to another world: network.player_persistence.map_url differs from the store's; {ASIDE}"
            ),
            Self::Inputs(names) => write!(
                f,
                "player store input changed: {} differs from the input the store was created with; {ASIDE}",
                names.join(", ")
            ),
            Self::MissingRow { table, row } => write!(
                f,
                "player store references {table} row {row:?}, which the installed game data no longer has; restore data with that row or move the state directory aside to start a fresh store"
            ),
            Self::RowOutOfRange { table, row, index } => write!(
                f,
                "player store references {table} row {row:?}, now at index {index}, past the 1-byte row handle range; move the state directory aside to start a fresh store"
            ),
            Self::Invalid(error) => error.fmt(f),
        }
    }
}
impl std::error::Error for StoreRefusal {}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct PlayerKey {
    pub(crate) account: u64,
    pub(crate) slot: i32,
}
impl PlayerKey {
    /// Validates representation only. Caller must establish authentication and slot agreement.
    pub fn new(account: u64, slot: i32) -> Result<Self, PlayerError> {
        if account == 0 || slot < 0 {
            return Err(PlayerError::InvalidIdentity);
        }
        Ok(Self { account, slot })
    }
}
impl std::fmt::Debug for PlayerKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PlayerKey(private)")
    }
}

/// SHA-256 digests of the operator-authored inputs a store is bound to. The
/// release file is deliberately absent: it is provenance, never a binding.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct PlayerInputs {
    /// The exact bytes of `network.authored_gameplay`.
    pub authored_gameplay: [u8; 32],
    /// `network.spawn`.
    pub spawn: [u8; 32],
    /// `network.authored_world`.
    pub authored_world: [u8; 32],
    /// `network.player_persistence`.
    pub declaration: [u8; 32],
    /// The resolved fresh-player values: starting maxima and the growth row by name.
    pub initial_values: [u8; 32],
}
impl PlayerInputs {
    fn named(&self) -> [(&'static str, [u8; 32]); 5] {
        [
            ("network.authored_gameplay", self.authored_gameplay),
            ("network.spawn", self.spawn),
            ("network.authored_world", self.authored_world),
            ("network.player_persistence", self.declaration),
            ("resolved initial player values", self.initial_values),
        ]
    }
    fn changed(&self, current: &Self) -> Vec<&'static str> {
        self.named()
            .into_iter()
            .zip(current.named())
            .filter(|((_, stored), (_, current))| stored != current)
            .map(|((name, _), _)| name)
            .collect()
    }
}
impl std::fmt::Debug for PlayerInputs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PlayerInputs(private)")
    }
}

/// What a store is bound to, plus the growth-row names its checkpoint indices
/// refer to. A context prepared at startup holds the installed row order; one
/// read from storage holds the stored names until [`PlayerSnapshot::open`]
/// resolves them, so it differs from the prepared context until then.
#[derive(Clone, PartialEq, Eq)]
pub struct PlayerContext {
    prospect_id: Vec<u16>,
    map_url: String,
    inputs: PlayerInputs,
    growth_rows: Arc<[String]>,
}
impl PlayerContext {
    /// `growth_rows` lists the installed [`GROWTH_TABLE`] rows in order; a
    /// checkpoint's `growth_row` is an index into it.
    pub fn new(
        prospect_id: Vec<u16>,
        map_url: String,
        inputs: PlayerInputs,
        growth_rows: Vec<String>,
    ) -> Result<Self, PlayerError> {
        if prospect_id.len() > 1024 || map_url.len() > 512 {
            return Err(PlayerError::Limit);
        }
        retained_context_bytes(prospect_id.capacity(), map_url.capacity())?;
        check_growth_rows(&growth_rows).map_err(|defect| defect.error())?;
        let value = Self {
            prospect_id,
            map_url,
            inputs,
            growth_rows: growth_rows.into(),
        };
        value.validate()?;
        Ok(value)
    }
    /// Row names are immutable after construction and checked there.
    fn validate(&self) -> Result<(), PlayerError> {
        retained_context_bytes(self.prospect_id.capacity(), self.map_url.capacity())?;
        if self.prospect_id.is_empty()
            || self.prospect_id.len() > 1024
            || char::decode_utf16(self.prospect_id.iter().copied()).any(|c| c.is_err())
            || self.map_url.len() > 512
            || !self.map_url.starts_with('/')
            || !self.map_url.is_ascii()
            || self.map_url.contains('?')
            || self.map_url.chars().any(char::is_control)
            || self
                .map_url
                .split('/')
                .skip(1)
                .any(|s| s.is_empty() || s == "." || s == "..")
            || self
                .inputs
                .named()
                .iter()
                .any(|(_, digest)| *digest == [0; 32])
        {
            return Err(PlayerError::InvalidContext);
        }
        Ok(())
    }
    /// A checkpoint's row references must name rows this context lists.
    pub(crate) fn check_rows(&self, checkpoint: &PlayerCheckpoint) -> Result<(), PlayerError> {
        if usize::from(checkpoint.vitals.growth_row) >= self.growth_rows.len() {
            return Err(PlayerError::InvalidState);
        }
        Ok(())
    }
}
impl std::fmt::Debug for PlayerContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PlayerContext(private)")
    }
}

/// Why a table's rows cannot back stored row names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RowsDefect {
    /// More rows than a context retains.
    TooMany(usize),
    /// The row names together exceed the retained-bytes bound.
    TooLarge,
    /// The row at this index has a name over the per-name size limit.
    LongName(usize),
    /// The row at this index has an empty name.
    EmptyName(usize),
    /// This name appears more than once, so it could not be resolved back to one row.
    Duplicate(String),
}
impl RowsDefect {
    fn error(&self) -> PlayerError {
        match self {
            Self::TooMany(_) | Self::TooLarge | Self::LongName(_) => PlayerError::Limit,
            Self::EmptyName(_) | Self::Duplicate(_) => PlayerError::InvalidContext,
        }
    }
}

/// The installed rows of `table` cannot back player checkpoints, which store
/// rows by name. Player persistence refuses to start, whether or not a store
/// exists.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstalledRowsRefusal {
    pub table: &'static str,
    pub defect: RowsDefect,
}
impl std::fmt::Display for InstalledRowsRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let table = self.table;
        write!(
            f,
            "installed game data table {table} cannot back player checkpoints: "
        )?;
        match &self.defect {
            RowsDefect::TooMany(count) => {
                write!(
                    f,
                    "it has {count} rows, over the {MAX_GROWTH_ROWS}-row limit"
                )?;
            }
            RowsDefect::TooLarge => {
                write!(
                    f,
                    "its row names exceed {MAX_GROWTH_ROW_BYTES} bytes in total"
                )?;
            }
            RowsDefect::LongName(index) => write!(
                f,
                "the name of row {index} exceeds {MAX_ROW_NAME_BYTES} bytes"
            )?,
            RowsDefect::EmptyName(index) => write!(f, "row {index} has an empty name")?,
            RowsDefect::Duplicate(row) => write!(f, "row name {row:?} appears more than once")?,
        }
        f.write_str(
            "; checkpoints store rows by name, so names must be non-empty, unique and bounded",
        )
    }
}
impl std::error::Error for InstalledRowsRefusal {}

/// Check the installed [`GROWTH_TABLE`] rows before building a
/// [`PlayerContext`] from them, so a refusal names the table and the defect.
pub fn check_installed_growth_rows(rows: &[String]) -> Result<(), InstalledRowsRefusal> {
    check_growth_rows(rows).map_err(|defect| InstalledRowsRefusal {
        table: GROWTH_TABLE,
        defect,
    })
}

fn check_growth_rows(rows: &[String]) -> Result<(), RowsDefect> {
    let mut bytes = 0usize;
    let mut seen = BTreeSet::new();
    if rows.len() > MAX_GROWTH_ROWS {
        return Err(RowsDefect::TooMany(rows.len()));
    }
    for (index, row) in rows.iter().enumerate() {
        if row.len() > MAX_ROW_NAME_BYTES {
            return Err(RowsDefect::LongName(index));
        }
        bytes = bytes.checked_add(row.len()).ok_or(RowsDefect::TooLarge)?;
        if bytes > MAX_GROWTH_ROW_BYTES {
            return Err(RowsDefect::TooLarge);
        }
        if row.is_empty() {
            return Err(RowsDefect::EmptyName(index));
        }
        // A duplicate name could not be resolved back to the same row.
        if !seen.insert(row.as_str()) {
            return Err(RowsDefect::Duplicate(row.clone()));
        }
    }
    Ok(())
}

fn validate_release(release: &ReleasePin) -> Result<(), PlayerError> {
    if release.app_id != DEDICATED_SERVER_APP_ID
        || release.build_id == 0
        || release.project_version.is_empty()
        || release.project_version.len() > 512
        || !release.project_version.is_ascii()
        || release.project_version.chars().any(char::is_control)
        || release.network_checksum != network_checksum(&release.project_version)
    {
        return Err(PlayerError::InvalidContext);
    }
    Ok(())
}

/// Stored provenance is informational: only what protects the decoder is
/// checked (a bounded, control-free version string). It is never checked for
/// internal consistency or against current constants, so a changed checksum
/// formula or app id cannot make a store unreadable.
fn validate_provenance(release: &ReleasePin) -> Result<(), PlayerError> {
    if release.project_version.len() > 512 {
        return Err(PlayerError::Limit);
    }
    if release.project_version.chars().any(char::is_control) {
        return Err(PlayerError::InvalidContext);
    }
    Ok(())
}

#[derive(Clone, Copy, PartialEq)]
pub struct PlayerCheckpoint {
    pub location_cm: [f64; 3],
    pub vitals: CharacterVitals,
}
impl PlayerCheckpoint {
    pub fn validate(&self) -> Result<(), PlayerError> {
        let v = self.vitals;
        if self
            .location_cm
            .iter()
            .any(|v| !v.is_finite() || v.abs() > 1_048_576.0)
            || v.biome_row != FRESH_BIOME_ROW
            || [
                (v.health, v.max_health),
                (v.stamina, v.max_stamina),
                (v.oxygen, v.max_oxygen),
                (v.water, v.max_water),
                (v.food, v.max_food),
            ]
            .iter()
            .any(|(current, max)| *max < 1 || *max > 10_000_000 || *current < 0 || current > max)
        {
            return Err(PlayerError::InvalidState);
        }
        Ok(())
    }
    pub(crate) fn same_recipe(&self, other: &Self) -> bool {
        let a = self.vitals;
        let b = other.vitals;
        (
            a.max_health,
            a.max_stamina,
            a.max_oxygen,
            a.max_water,
            a.max_food,
            a.biome_row,
            a.growth_row,
        ) == (
            b.max_health,
            b.max_stamina,
            b.max_oxygen,
            b.max_water,
            b.max_food,
            b.biome_row,
            b.growth_row,
        )
    }
}
impl std::fmt::Debug for PlayerCheckpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PlayerCheckpoint(private)")
    }
}
#[derive(Clone, Copy, PartialEq)]
pub struct PlayerProjection {
    pub player_id: u64,
    pub revision: u64,
    pub checkpoint: PlayerCheckpoint,
    pub is_new: bool,
}
impl std::fmt::Debug for PlayerProjection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PlayerProjection")
            .field("revision", &self.revision)
            .finish_non_exhaustive()
    }
}
#[derive(Clone, PartialEq)]
pub(crate) struct PlayerRecord {
    pub key: PlayerKey,
    pub id: u64,
    pub revision: u64,
    pub checkpoint: PlayerCheckpoint,
}
impl PlayerRecord {
    pub(crate) fn projection(&self, is_new: bool) -> PlayerProjection {
        PlayerProjection {
            player_id: self.id,
            revision: self.revision,
            checkpoint: self.checkpoint,
            is_new,
        }
    }
}

#[derive(Clone, PartialEq)]
pub struct PlayerSnapshot {
    pub(crate) context: PlayerContext,
    written_under: Option<ReleasePin>,
    pub(crate) high_water: u64,
    pub(crate) records: Vec<PlayerRecord>,
}
impl std::fmt::Debug for PlayerSnapshot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PlayerSnapshot")
            .field("records", &self.len())
            .finish()
    }
}
impl PlayerSnapshot {
    pub fn empty(context: PlayerContext) -> Self {
        Self {
            context,
            written_under: None,
            high_water: 0,
            records: Vec::new(),
        }
    }
    /// An empty store created under `release`, recorded as provenance only.
    pub fn fresh(context: PlayerContext, release: &ReleasePin) -> Result<Self, PlayerError> {
        validate_release(release)?;
        let mut snapshot = Self::empty(context);
        snapshot.written_under = Some(release.clone());
        Ok(snapshot)
    }
    pub fn context(&self) -> &PlayerContext {
        &self.context
    }
    /// The release the store was last opened or created under. Provenance for
    /// logs and diagnostics; no check compares it.
    pub fn written_under(&self) -> Option<&ReleasePin> {
        self.written_under.as_ref()
    }
    pub fn len(&self) -> usize {
        self.records.len()
    }
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }
    pub fn projection(&self, key: PlayerKey) -> Option<PlayerProjection> {
        self.records
            .iter()
            .find(|r| r.key == key)
            .map(|r| r.projection(false))
    }
    /// Bind this store to the current startup context. The world and every
    /// authored input must match; each stored growth row is resolved by name
    /// against `expected`'s installed rows, so rows added, removed or reordered
    /// around it change nothing. `release` becomes the store's provenance. On
    /// refusal the snapshot is unchanged. Returns the previous provenance.
    pub fn open(
        &mut self,
        expected: &PlayerContext,
        release: &ReleasePin,
    ) -> Result<Option<ReleasePin>, StoreRefusal> {
        self.validate().map_err(StoreRefusal::Invalid)?;
        expected.validate().map_err(StoreRefusal::Invalid)?;
        validate_release(release).map_err(StoreRefusal::Invalid)?;
        let stored = &self.context;
        if stored.prospect_id != expected.prospect_id {
            return Err(StoreRefusal::Prospect);
        }
        if stored.map_url != expected.map_url {
            return Err(StoreRefusal::Map);
        }
        let changed = stored.inputs.changed(&expected.inputs);
        if !changed.is_empty() {
            return Err(StoreRefusal::Inputs(changed));
        }
        // Resolve each referenced stored name once, before changing anything.
        let mut resolved = std::collections::BTreeMap::new();
        for r in &self.records {
            let stored_index = r.checkpoint.vitals.growth_row;
            if resolved.contains_key(&stored_index) {
                continue;
            }
            let row = stored
                .growth_rows
                .get(usize::from(stored_index))
                .ok_or(StoreRefusal::Invalid(PlayerError::InvalidState))?;
            let index = expected
                .growth_rows
                .iter()
                .position(|name| name == row)
                .ok_or_else(|| StoreRefusal::MissingRow {
                    table: GROWTH_TABLE,
                    row: row.clone(),
                })?;
            let index = u8::try_from(index).map_err(|_| StoreRefusal::RowOutOfRange {
                table: GROWTH_TABLE,
                row: row.clone(),
                index,
            })?;
            resolved.insert(stored_index, index);
        }
        let mut records = self.records.clone();
        for record in &mut records {
            let row = &mut record.checkpoint.vitals.growth_row;
            *row = *resolved
                .get(row)
                .ok_or(StoreRefusal::Invalid(PlayerError::InvalidState))?;
        }
        self.records = records;
        self.context = expected.clone();
        Ok(self.written_under.replace(release.clone()))
    }
    pub fn validate(&self) -> Result<(), PlayerError> {
        self.context.validate()?;
        if let Some(release) = &self.written_under {
            validate_provenance(release)?;
        }
        Self::check_capacity(self.records.len())?;
        let retained = self
            .records
            .capacity()
            .checked_mul(std::mem::size_of::<PlayerRecord>())
            .and_then(|n| n.checked_add(8192))
            .ok_or(PlayerError::Limit)?;
        if retained > MAX_PLAYER_BYTES {
            return Err(PlayerError::Limit);
        }
        let mut ids = BTreeSet::new();
        let mut keys = BTreeSet::new();
        let mut accounts = std::collections::BTreeMap::<u64, usize>::new();
        for r in &self.records {
            PlayerKey::new(r.key.account, r.key.slot)?;
            r.checkpoint.validate()?;
            self.context.check_rows(&r.checkpoint)?;
            if r.id == 0 || r.id > self.high_water || r.revision == 0 {
                return Err(PlayerError::InvalidState);
            }
            if !ids.insert(r.id) || !keys.insert(r.key) {
                return Err(PlayerError::Duplicate);
            }
            let count = accounts.entry(r.key.account).or_default();
            *count += 1;
            if *count > MAX_PLAYERS_PER_ACCOUNT {
                return Err(PlayerError::Limit);
            }
        }
        Ok(())
    }
    pub(crate) fn check_capacity(count: usize) -> Result<(), PlayerError> {
        let bytes = count
            .checked_mul(512usize.max(std::mem::size_of::<PlayerRecord>()))
            .and_then(|n| n.checked_add(8192))
            .ok_or(PlayerError::Limit)?;
        if count > MAX_PLAYERS || bytes > MAX_PLAYER_BYTES {
            return Err(PlayerError::Limit);
        }
        Ok(())
    }
}

// Only these private storage DTOs serialize account identifiers. Never forward serde diagnostics.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct InputsDto {
    authored_gameplay: [u8; 32],
    spawn: [u8; 32],
    authored_world: [u8; 32],
    declaration: [u8; 32],
    initial_values: [u8; 32],
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ContextDto {
    #[serde(deserialize_with = "bounded_prospect")]
    prospect_id: Vec<u16>,
    #[serde(deserialize_with = "bounded_text")]
    map_url: String,
    #[serde(deserialize_with = "crate::json::object")]
    inputs: InputsDto,
}
/// Provenance only: the release the store was last opened or created under.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReleaseDto {
    app_id: u32,
    build_id: u64,
    #[serde(deserialize_with = "bounded_text")]
    project_version: String,
    network_checksum: u32,
}
/// A stored game-data row reference: the row's identity, never its index.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RowDto {
    #[serde(deserialize_with = "bounded_text")]
    table: String,
    #[serde(deserialize_with = "bounded_row")]
    row: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RecordDto {
    account: u64,
    slot: i32,
    id: u64,
    revision: u64,
    location_cm: [f64; 3],
    vitals: [i32; 10],
    #[serde(deserialize_with = "crate::json::object")]
    growth_row: RowDto,
}
#[derive(Serialize)]
struct SnapshotOut<'a> {
    context: ContextDto,
    written_under: Option<ReleaseDto>,
    high_water: u64,
    records: Vec<RecordOut<'a>>,
}
#[derive(Serialize)]
struct RecordOut<'a> {
    account: u64,
    slot: i32,
    id: u64,
    revision: u64,
    location_cm: [f64; 3],
    vitals: [i32; 10],
    growth_row: RowOut<'a>,
}
#[derive(Serialize)]
struct RowOut<'a> {
    table: &'static str,
    row: &'a str,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SnapshotDto {
    #[serde(deserialize_with = "crate::json::object")]
    context: ContextDto,
    #[serde(deserialize_with = "crate::json::optional_object")]
    written_under: Option<ReleaseDto>,
    high_water: u64,
    #[serde(deserialize_with = "bounded_records")]
    records: StoredRecords,
}
/// Records whose growth index refers to `growth_rows`, the distinct stored
/// row names in order of first use. Interning bounds retained names.
struct StoredRecords {
    records: Vec<PlayerRecord>,
    growth_rows: Vec<String>,
}
fn bounded_records<'de, D: Deserializer<'de>>(d: D) -> Result<StoredRecords, D::Error> {
    struct V;
    impl<'de> serde::de::Visitor<'de> for V {
        type Value = StoredRecords;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("bounded private player records")
        }
        fn visit_seq<A: serde::de::SeqAccess<'de>>(
            self,
            mut seq: A,
        ) -> Result<Self::Value, A::Error> {
            let mut out = StoredRecords {
                records: Vec::new(),
                growth_rows: Vec::new(),
            };
            while let Some(value) = seq.next_element::<crate::json::JsonObject<RecordDto>>()? {
                if out.records.len() >= MAX_PLAYERS {
                    return Err(serde::de::Error::custom("player record limit"));
                }
                let r = value.0;
                if r.growth_row.table != GROWTH_TABLE {
                    return Err(serde::de::Error::custom("player record row table"));
                }
                let growth = match out
                    .growth_rows
                    .iter()
                    .position(|row| *row == r.growth_row.row)
                {
                    Some(index) => index,
                    None if out.growth_rows.len() < MAX_STORED_GROWTH_ROWS => {
                        out.growth_rows.push(r.growth_row.row);
                        out.growth_rows.len() - 1
                    }
                    None => return Err(serde::de::Error::custom("player record row limit")),
                };
                let v = r.vitals;
                out.records.push(PlayerRecord {
                    key: PlayerKey {
                        account: r.account,
                        slot: r.slot,
                    },
                    id: r.id,
                    revision: r.revision,
                    checkpoint: PlayerCheckpoint {
                        location_cm: r.location_cm,
                        vitals: CharacterVitals {
                            health: v[0],
                            max_health: v[1],
                            stamina: v[2],
                            max_stamina: v[3],
                            oxygen: v[4],
                            max_oxygen: v[5],
                            water: v[6],
                            max_water: v[7],
                            food: v[8],
                            max_food: v[9],
                            biome_row: FRESH_BIOME_ROW,
                            growth_row: u8::try_from(growth).map_err(serde::de::Error::custom)?,
                        },
                    },
                });
            }
            Ok(out)
        }
    }
    d.deserialize_seq(V)
}
impl Serialize for PlayerSnapshot {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.validate().map_err(serde::ser::Error::custom)?;
        let c = &self.context;
        let mut records = self.records.iter().collect::<Vec<_>>();
        records.sort_by_key(|r| r.key);
        let dto = SnapshotOut {
            context: ContextDto {
                prospect_id: c.prospect_id.clone(),
                map_url: c.map_url.clone(),
                inputs: InputsDto {
                    authored_gameplay: c.inputs.authored_gameplay,
                    spawn: c.inputs.spawn,
                    authored_world: c.inputs.authored_world,
                    declaration: c.inputs.declaration,
                    initial_values: c.inputs.initial_values,
                },
            },
            written_under: self.written_under.as_ref().map(|r| ReleaseDto {
                app_id: r.app_id,
                build_id: r.build_id,
                project_version: r.project_version.clone(),
                network_checksum: r.network_checksum,
            }),
            high_water: self.high_water,
            records: records
                .into_iter()
                .map(|r| {
                    let v = r.checkpoint.vitals;
                    // validate() checked every index against the context's rows.
                    let row = c
                        .growth_rows
                        .get(usize::from(v.growth_row))
                        .ok_or_else(|| serde::ser::Error::custom(PlayerError::InvalidState))?;
                    Ok(RecordOut {
                        account: r.key.account,
                        slot: r.key.slot,
                        id: r.id,
                        revision: r.revision,
                        location_cm: r.checkpoint.location_cm,
                        vitals: [
                            v.health,
                            v.max_health,
                            v.stamina,
                            v.max_stamina,
                            v.oxygen,
                            v.max_oxygen,
                            v.water,
                            v.max_water,
                            v.food,
                            v.max_food,
                        ],
                        growth_row: RowOut {
                            table: GROWTH_TABLE,
                            row,
                        },
                    })
                })
                .collect::<Result<_, S::Error>>()?,
        };
        dto.serialize(s)
    }
}
impl<'de> Deserialize<'de> for PlayerSnapshot {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let dto = crate::json::JsonObject::<SnapshotDto>::deserialize(d)
            .map_err(|_| serde::de::Error::custom("invalid private player checkpoint"))?
            .0;
        let c = dto.context;
        let i = c.inputs;
        let context = PlayerContext::new(
            c.prospect_id,
            c.map_url,
            PlayerInputs {
                authored_gameplay: i.authored_gameplay,
                spawn: i.spawn,
                authored_world: i.authored_world,
                declaration: i.declaration,
                initial_values: i.initial_values,
            },
            dto.records.growth_rows,
        )
        .map_err(serde::de::Error::custom)?;
        let value = Self {
            context,
            written_under: dto.written_under.map(|r| ReleasePin {
                app_id: r.app_id,
                build_id: r.build_id,
                project_version: r.project_version,
                network_checksum: r.network_checksum,
            }),
            high_water: dto.high_water,
            records: dto.records.records,
        };
        value.validate().map_err(serde::de::Error::custom)?;
        Ok(value)
    }
}
/// A present field must be a non-null checkpoint; absence is handled by serde default.
pub(crate) fn deserialize_optional<'de, D: Deserializer<'de>>(
    d: D,
) -> Result<Option<PlayerSnapshot>, D::Error> {
    PlayerSnapshot::deserialize(d).map(Some)
}

fn bounded_prospect<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u16>, D::Error> {
    struct Units;
    impl<'de> serde::de::Visitor<'de> for Units {
        type Value = Vec<u16>;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("bounded private context")
        }
        fn visit_seq<A: serde::de::SeqAccess<'de>>(
            self,
            mut sequence: A,
        ) -> Result<Self::Value, A::Error> {
            let mut units = Vec::new();
            while let Some(unit) = sequence.next_element::<u16>()? {
                if units.len() >= 1024 {
                    return Err(serde::de::Error::custom("player context limit"));
                }
                units.push(unit);
            }
            Ok(units)
        }
    }
    d.deserialize_seq(Units)
}
fn bounded_text<'de, D: Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    bounded_string::<D, 512>(d)
}
fn bounded_row<'de, D: Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    bounded_string::<D, MAX_ROW_NAME_BYTES>(d)
}
fn bounded_string<'de, D: Deserializer<'de>, const LIMIT: usize>(d: D) -> Result<String, D::Error> {
    struct Text<const LIMIT: usize>;
    impl<'de, const LIMIT: usize> serde::de::Visitor<'de> for Text<LIMIT> {
        type Value = String;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("bounded private context")
        }
        fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<String, E> {
            if value.len() > LIMIT {
                return Err(E::custom("player context limit"));
            }
            Ok(value.to_owned())
        }
        fn visit_string<E: serde::de::Error>(self, value: String) -> Result<String, E> {
            if value.len() > LIMIT {
                return Err(E::custom("player context limit"));
            }
            Ok(value)
        }
    }
    d.deserialize_string(Text::<LIMIT>)
}

fn retained_context_bytes(units: usize, map: usize) -> Result<usize, PlayerError> {
    let bytes = units
        .checked_mul(std::mem::size_of::<u16>())
        .and_then(|n| n.checked_add(map))
        .ok_or(PlayerError::Limit)?;
    if bytes > 8192 {
        return Err(PlayerError::Limit);
    }
    Ok(bytes)
}

#[cfg(test)]
mod allocation_tests {
    use super::*;
    #[test]
    fn private_records_cannot_retain_unbounded_spare_capacity() {
        let inputs = PlayerInputs {
            authored_gameplay: [1; 32],
            spawn: [1; 32],
            authored_world: [1; 32],
            declaration: [1; 32],
            initial_values: [1; 32],
        };
        let context = PlayerContext::new(
            vec![65],
            "/Authored/Map".into(),
            inputs,
            vec!["Player".into()],
        )
        .unwrap();
        let mut snapshot = PlayerSnapshot::empty(context);
        snapshot.records =
            Vec::with_capacity(MAX_PLAYER_BYTES / std::mem::size_of::<PlayerRecord>() + 1);
        assert_eq!(snapshot.validate(), Err(PlayerError::Limit));
    }
}

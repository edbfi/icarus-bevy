//! Independently authored checkpoints. Keys are private; construction is not authentication.
use crate::experimental::{
    pawn_open::CharacterVitals,
    release::{DEDICATED_SERVER_APP_ID, ReleasePin, network_checksum},
};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::BTreeSet;

pub const MAX_PLAYERS: usize = 4096;
pub const MAX_PLAYERS_PER_ACCOUNT: usize = 64;
/// Conservative validated-state/encoding allowance, not a peak JSON-parser/RSS claim.
pub const MAX_PLAYER_BYTES: usize = 4 * 1024 * 1024;

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

#[derive(Clone, PartialEq, Eq)]
pub struct PlayerContext {
    prospect_id: Vec<u16>,
    map_url: String,
    app_id: u32,
    build_id: u64,
    project_version: String,
    network_checksum: u32,
    recipe_sha256: [u8; 32],
}
impl PlayerContext {
    pub fn new(
        prospect_id: Vec<u16>,
        map_url: String,
        release: &ReleasePin,
        recipe_sha256: [u8; 32],
    ) -> Result<Self, PlayerError> {
        // Check borrowed lengths and moved allocations before copying release text.
        if release.project_version.len() > 512 || prospect_id.len() > 1024 || map_url.len() > 512 {
            return Err(PlayerError::Limit);
        }
        retained_context_bytes(
            prospect_id.capacity(),
            map_url.capacity(),
            release.project_version.len(),
        )?;
        let value = Self {
            prospect_id,
            map_url,
            app_id: release.app_id,
            build_id: release.build_id,
            project_version: release.project_version.clone(),
            network_checksum: release.network_checksum,
            recipe_sha256,
        };
        value.validate()?;
        Ok(value)
    }
    fn validate(&self) -> Result<(), PlayerError> {
        retained_context_bytes(
            self.prospect_id.capacity(),
            self.map_url.capacity(),
            self.project_version.capacity(),
        )?;
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
            || self.app_id != DEDICATED_SERVER_APP_ID
            || self.build_id == 0
            || self.project_version.is_empty()
            || self.project_version.len() > 512
            || !self.project_version.is_ascii()
            || self.project_version.chars().any(char::is_control)
            || self.network_checksum != network_checksum(&self.project_version)
            || self.recipe_sha256 == [0; 32]
        {
            return Err(PlayerError::InvalidContext);
        }
        Ok(())
    }
}
impl std::fmt::Debug for PlayerContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PlayerContext(private)")
    }
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
            high_water: 0,
            records: Vec::new(),
        }
    }
    pub fn context(&self) -> &PlayerContext {
        &self.context
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
    pub fn validate(&self) -> Result<(), PlayerError> {
        self.context.validate()?;
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
struct ContextDto {
    #[serde(deserialize_with = "bounded_prospect")]
    prospect_id: Vec<u16>,
    #[serde(deserialize_with = "bounded_text")]
    map_url: String,
    app_id: u32,
    build_id: u64,
    #[serde(deserialize_with = "bounded_text")]
    project_version: String,
    network_checksum: u32,
    recipe_sha256: [u8; 32],
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
    biome_row: u8,
    growth_row: u8,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SnapshotDto {
    #[serde(deserialize_with = "crate::json::object")]
    context: ContextDto,
    high_water: u64,
    #[serde(deserialize_with = "bounded_records")]
    records: Vec<RecordDto>,
}
fn bounded_records<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<RecordDto>, D::Error> {
    struct V;
    impl<'de> serde::de::Visitor<'de> for V {
        type Value = Vec<RecordDto>;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("bounded private player records")
        }
        fn visit_seq<A: serde::de::SeqAccess<'de>>(
            self,
            mut seq: A,
        ) -> Result<Self::Value, A::Error> {
            let mut out = Vec::new();
            while let Some(value) = seq.next_element::<crate::json::JsonObject<RecordDto>>()? {
                if out.len() >= MAX_PLAYERS {
                    return Err(serde::de::Error::custom("player record limit"));
                }
                out.push(value.0);
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
        let dto = SnapshotDto {
            context: ContextDto {
                prospect_id: c.prospect_id.clone(),
                map_url: c.map_url.clone(),
                app_id: c.app_id,
                build_id: c.build_id,
                project_version: c.project_version.clone(),
                network_checksum: c.network_checksum,
                recipe_sha256: c.recipe_sha256,
            },
            high_water: self.high_water,
            records: records
                .into_iter()
                .map(|r| {
                    let v = r.checkpoint.vitals;
                    RecordDto {
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
                        biome_row: v.biome_row,
                        growth_row: v.growth_row,
                    }
                })
                .collect(),
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
        let value = Self {
            context: PlayerContext {
                prospect_id: c.prospect_id,
                map_url: c.map_url,
                app_id: c.app_id,
                build_id: c.build_id,
                project_version: c.project_version,
                network_checksum: c.network_checksum,
                recipe_sha256: c.recipe_sha256,
            },
            high_water: dto.high_water,
            records: dto
                .records
                .into_iter()
                .map(|r| {
                    let v = r.vitals;
                    PlayerRecord {
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
                                biome_row: r.biome_row,
                                growth_row: r.growth_row,
                            },
                        },
                    }
                })
                .collect(),
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
    struct Text;
    impl<'de> serde::de::Visitor<'de> for Text {
        type Value = String;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("bounded private context")
        }
        fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<String, E> {
            if value.len() > 512 {
                return Err(E::custom("player context limit"));
            }
            Ok(value.to_owned())
        }
        fn visit_string<E: serde::de::Error>(self, value: String) -> Result<String, E> {
            if value.len() > 512 {
                return Err(E::custom("player context limit"));
            }
            Ok(value)
        }
    }
    d.deserialize_string(Text)
}

fn retained_context_bytes(units: usize, map: usize, project: usize) -> Result<usize, PlayerError> {
    let bytes = units
        .checked_mul(std::mem::size_of::<u16>())
        .and_then(|n| n.checked_add(map))
        .and_then(|n| n.checked_add(project))
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
        let release = ReleasePin {
            app_id: DEDICATED_SERVER_APP_ID,
            build_id: 1,
            project_version: "authored".into(),
            network_checksum: network_checksum("authored"),
        };
        let context =
            PlayerContext::new(vec![65], "/Authored/Map".into(), &release, [1; 32]).unwrap();
        let mut snapshot = PlayerSnapshot::empty(context);
        snapshot.records =
            Vec::with_capacity(MAX_PLAYER_BYTES / std::mem::size_of::<PlayerRecord>() + 1);
        assert_eq!(snapshot.validate(), Err(PlayerError::Limit));
    }
}

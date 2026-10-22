//! A save's `ProspectInfo`, typed (`F-726`, `F-738`).
//!
//! `FProspectInfo` has 17 properties, `FAssociatedMemberInfo` 8 and
//! `FCustomGameSetting` 2, in the declaration order both the tagged save and
//! the replicated layout follow (`F-738`). A tagged stream omits properties
//! at their defaults, so a missing property reads as its default; a present
//! one must have its declared kind. Enumerators are stored by name and map to
//! their values (client descriptors, `M-780`). Values are the caller's data;
//! nothing here reports them.
use anyhow::{Context, Result, bail, ensure};

use super::tagged::TagKind;
use super::typed_value::{Scalar, TypedTree, Value};

/// `EProspectState` enumerators, by value.
pub const PROSPECT_STATES: [&str; 5] = [
    "Unclaimed",
    "Claimed",
    "Active",
    "Ended",
    "MaxProspectStates",
];
/// `EProspectLocation` enumerators, by value.
pub const PROSPECT_LOCATIONS: [&str; 13] = [
    "Unknown",
    "Hab",
    "Prospect_Conifer",
    "Prospect_Arctic",
    "Prospect_Cave",
    "Prospect_Desert",
    "Prospect_Grasslands",
    "Prospect_Volcanic",
    "Prospect_Swamp",
    "Prospect_Geothermal",
    "Prospect_Tundra",
    "Prospect_Coastal",
    "Prospect_Jungle",
];
/// `EMissionDifficulty` enumerators, by value.
pub const MISSION_DIFFICULTIES: [&str; 5] = ["None", "Easy", "Medium", "Hard", "Extreme"];

/// One `FAssociatedMemberInfo`; `Debug` shows no values.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct AssociatedMember {
    pub account_name: Vec<u16>,
    pub character_name: Vec<u16>,
    pub user_id: Vec<u16>,
    pub chr_slot: i32,
    pub experience: i32,
    /// `EProspectLocation` value.
    pub status: u8,
    pub settled: bool,
    pub is_currently_playing: bool,
}

/// One `FCustomGameSetting`; `Debug` shows no values.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct CustomGameSetting {
    pub setting_row_name: Vec<u16>,
    pub setting_value: i32,
}

/// One `FProspectInfo`; strings are UTF-16 units without the terminator.
/// `Debug` shows only the member and setting counts.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct ProspectInfo {
    pub prospect_id: Vec<u16>,
    pub claimed_account_id: Vec<u16>,
    pub claimed_account_character: i32,
    pub prospect_dt_key: Vec<u16>,
    pub faction_mission_dt_key: Vec<u16>,
    pub lobby_name: Vec<u16>,
    pub expire_time: i64,
    /// `EProspectState` value.
    pub prospect_state: u8,
    pub associated_members: Vec<AssociatedMember>,
    pub cost: i32,
    pub reward: i32,
    /// `EMissionDifficulty` value.
    pub difficulty: u8,
    pub insurance: bool,
    pub no_respawns: bool,
    pub elapsed_time: i32,
    pub selected_drop_point: i32,
    pub custom_settings: Vec<CustomGameSetting>,
}

impl std::fmt::Debug for AssociatedMember {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("AssociatedMember")
    }
}

impl std::fmt::Debug for CustomGameSetting {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("CustomGameSetting")
    }
}

impl std::fmt::Debug for ProspectInfo {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ProspectInfo")
            .field("associated_members", &self.associated_members.len())
            .field("custom_settings", &self.custom_settings.len())
            .finish_non_exhaustive()
    }
}

/// Typed access to one tagged struct's properties by name.
struct Fields<'a> {
    tree: &'a TypedTree,
    what: &'static str,
}

impl<'a> Fields<'a> {
    fn scalar(&self, name: &str) -> Option<&'a Scalar> {
        match self.tree.get(name).map(|property| &property.value) {
            Some(Value::Scalar(scalar)) => Some(scalar),
            _ => None,
        }
    }

    fn present(&self, name: &str) -> bool {
        self.tree.get(name).is_some()
    }

    fn string(&self, name: &str) -> Result<Vec<u16>> {
        match self.scalar(name) {
            Some(Scalar::Str(units)) => Ok(units.clone()),
            None if !self.present(name) => Ok(Vec::new()),
            _ => bail!("{}.{name} is not a StrProperty", self.what),
        }
    }

    fn int(&self, name: &str) -> Result<i32> {
        match self.scalar(name) {
            Some(Scalar::Int(value)) => Ok(*value),
            None if !self.present(name) => Ok(0),
            _ => bail!("{}.{name} is not an IntProperty", self.what),
        }
    }

    fn float(&self, name: &str) -> Result<f32> {
        match self.scalar(name) {
            Some(Scalar::Float(value)) => Ok(*value),
            None if !self.present(name) => Ok(0.0),
            _ => bail!("{}.{name} is not a FloatProperty", self.what),
        }
    }

    /// A tagged struct's fields; missing reads as an empty struct.
    fn tagged(&self, name: &str, what: &'static str) -> Result<Option<Fields<'a>>> {
        match self.tree.get(name).map(|property| &property.value) {
            Some(Value::TaggedStruct { tree, .. }) => Ok(Some(Fields { tree, what })),
            None => Ok(None),
            _ => bail!("{}.{name} is not a tagged struct", self.what),
        }
    }

    fn int64(&self, name: &str) -> Result<i64> {
        match self.scalar(name) {
            Some(Scalar::Int64(value)) => Ok(*value),
            None if !self.present(name) => Ok(0),
            _ => bail!("{}.{name} is not an Int64Property", self.what),
        }
    }

    fn bool(&self, name: &str) -> Result<bool> {
        match self.scalar(name) {
            Some(Scalar::Bool(value)) => Ok(*value),
            None if !self.present(name) => Ok(false),
            _ => bail!("{}.{name} is not a BoolProperty", self.what),
        }
    }

    /// An enumerator by `Enum::Name` or bare name, or a raw byte, as its
    /// value in `names`.
    fn enumerator(&self, name: &str, enum_name: &str, names: &[&str]) -> Result<u8> {
        let value = match self.scalar(name) {
            Some(Scalar::Enum(units)) => {
                let text = String::from_utf16(units)
                    .with_context(|| format!("{}.{name} enumerator", self.what))?;
                let short = text
                    .strip_prefix(enum_name)
                    .and_then(|rest| rest.strip_prefix("::"))
                    .unwrap_or(&text);
                names
                    .iter()
                    .position(|candidate| *candidate == short)
                    .with_context(|| {
                        format!("{}.{name} names no {enum_name} enumerator", self.what)
                    })?
            }
            Some(Scalar::Byte(value)) => usize::from(*value),
            None if !self.present(name) => 0,
            _ => bail!("{}.{name} is not an {enum_name}", self.what),
        };
        ensure!(
            value < names.len(),
            "{}.{name} is outside {enum_name}",
            self.what
        );
        Ok(u8::try_from(value)?)
    }

    /// A name array's elements; missing reads as empty.
    fn names(&self, name: &str) -> Result<Vec<Vec<u16>>> {
        match self.tree.get(name).map(|property| &property.value) {
            Some(Value::ScalarArray {
                inner: TagKind::Name,
                items,
            }) => items
                .iter()
                .map(|item| match item {
                    Scalar::Name(units) => Ok(units.clone()),
                    _ => bail!("{}.{name} holds a non-name item", self.what),
                })
                .collect(),
            None => Ok(Vec::new()),
            _ => bail!("{}.{name} is not a name array", self.what),
        }
    }

    /// A tagged struct array's elements; missing reads as empty.
    fn structs(&self, name: &str) -> Result<&'a [TypedTree]> {
        match self
            .tree
            .get(name)
            .map(|property| (property.kind, &property.value))
        {
            Some((TagKind::Array, Value::StructArray { elements, .. })) => Ok(elements),
            None => Ok(&[]),
            _ => bail!("{}.{name} is not a struct array", self.what),
        }
    }
}

impl AssociatedMember {
    fn from_tree(tree: &TypedTree) -> Result<Self> {
        let fields = Fields {
            tree,
            what: "AssociatedMemberInfo",
        };
        Ok(Self {
            account_name: fields.string("AccountName")?,
            character_name: fields.string("CharacterName")?,
            user_id: fields.string("UserID")?,
            chr_slot: fields.int("ChrSlot")?,
            experience: fields.int("Experience")?,
            status: fields.enumerator("Status", "EProspectLocation", &PROSPECT_LOCATIONS)?,
            settled: fields.bool("Settled")?,
            is_currently_playing: fields.bool("IsCurrentlyPlaying")?,
        })
    }
}

impl ProspectInfo {
    /// The active prospect as a running server replicates it: `ElapsedTime`
    /// is the level clock, and only `playing` members are currently playing
    /// (the M-707 server showed the save's recorded `LevelTimeElapsedSec`
    /// and no member playing).
    pub fn for_session(
        &self,
        level_time_elapsed_sec: i32,
        playing: impl Fn(&AssociatedMember) -> bool,
    ) -> Self {
        let mut info = self.clone();
        info.elapsed_time = level_time_elapsed_sec;
        for member in &mut info.associated_members {
            member.is_currently_playing = playing(member);
        }
        info
    }

    /// Type the `ProspectInfo` struct's tagged stream.
    pub fn from_tree(tree: &TypedTree) -> Result<Self> {
        let fields = Fields {
            tree,
            what: "ProspectInfo",
        };
        Ok(Self {
            prospect_id: fields.string("ProspectID")?,
            claimed_account_id: fields.string("ClaimedAccountID")?,
            claimed_account_character: fields.int("ClaimedAccountCharacter")?,
            prospect_dt_key: fields.string("ProspectDTKey")?,
            faction_mission_dt_key: fields.string("FactionMissionDTKey")?,
            lobby_name: fields.string("LobbyName")?,
            expire_time: fields.int64("ExpireTime")?,
            prospect_state: fields.enumerator(
                "ProspectState",
                "EProspectState",
                &PROSPECT_STATES,
            )?,
            associated_members: fields
                .structs("AssociatedMembers")?
                .iter()
                .map(AssociatedMember::from_tree)
                .collect::<Result<_>>()?,
            cost: fields.int("Cost")?,
            reward: fields.int("Reward")?,
            difficulty: fields.enumerator(
                "Difficulty",
                "EMissionDifficulty",
                &MISSION_DIFFICULTIES,
            )?,
            insurance: fields.bool("Insurance")?,
            no_respawns: fields.bool("NoRespawns")?,
            elapsed_time: fields.int("ElapsedTime")?,
            selected_drop_point: fields.int("SelectedDropPoint")?,
            custom_settings: fields
                .structs("CustomSettings")?
                .iter()
                .map(|tree| {
                    let fields = Fields {
                        tree,
                        what: "CustomGameSetting",
                    };
                    Ok(CustomGameSetting {
                        setting_row_name: fields.string("SettingRowName")?,
                        setting_value: fields.int("SettingValue")?,
                    })
                })
                .collect::<Result<_>>()?,
        })
    }
}

/// The game mode recorder's values the GameState restores from (`C015`,
/// `F-650`): `GameModeRecord`'s seed, time of day, prospect game time, day
/// length and level clock, and the top-level `DynamicQuestSeed`. Missing
/// properties read as their defaults; `Debug` shows no values.
#[derive(Clone, Default, PartialEq)]
pub struct SavedGameMode {
    pub game_state_seed: i32,
    pub time_of_day: f32,
    pub prospect_game_time: f32,
    pub seconds_per_game_day: i32,
    pub level_time_elapsed_sec: i32,
    pub dynamic_quest_seed: i32,
    /// `GameModeRecord.SessionFlagRecords`: `D_SessionFlags` row names, in
    /// saved order, that the restore turns into row handles (F-820).
    pub session_flags: Vec<Vec<u16>>,
}

impl std::fmt::Debug for SavedGameMode {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("SavedGameMode")
    }
}

impl SavedGameMode {
    /// Type a `C015` component's tagged properties.
    pub fn from_tree(tree: &TypedTree) -> Result<Self> {
        let fields = Fields {
            tree,
            what: "GameModeStateRecorderComponent",
        };
        let dynamic_quest_seed = fields.int("DynamicQuestSeed")?;
        let Some(record) = fields.tagged("GameModeRecord", "GameModeRecord")? else {
            return Ok(Self {
                dynamic_quest_seed,
                ..Self::default()
            });
        };
        Ok(Self {
            game_state_seed: record.int("GameStateSeed")?,
            time_of_day: record.float("TimeOfDay")?,
            prospect_game_time: record.float("ProspectGameTime")?,
            seconds_per_game_day: record.int("SecondsPerGameDay")?,
            level_time_elapsed_sec: record.int("LevelTimeElapsedSec")?,
            dynamic_quest_seed,
            session_flags: record.names("SessionFlagRecords")?,
        })
    }
}

//! Native quest actor (`AQuest`, `C019`) stitching, pure `Check()` completion
//! evaluation and open composition (`F-934`, `F-935`, `U-074`, `U-076`).
//!
//! `UIcarusQuestRecorderComponent::RecordOwnerState` (`C019`, `0x1418e8de0`)
//! records each live quest actor's `IcarusActorGUID`, class name, transform,
//! `D_Quests` row name, subquest links, quest variables and relevant
//! actor/character links. On load, `C019::StitchingOperation` (`0x1418f66a0`)
//! wires `SubQuests` and `QuestParent`, `C018::StitchingOperation`
//! (`0x1418f5ef0`) binds `AQuestManager::InitialQuest` to the live `C019`
//! actor matching `InitialQuestRecord` (leaving `None` when the mission's root
//! quest was destroyed on completion), and `AQuestManager::Reload`
//! (`0x1418ecd30`) populates each active parent quest's `ActiveQuests` with
//! children whose `Check()` returns `false`.
//!
//! Checksums and `Check()` rules come from [`super::quest_table`] (derived
//! offline from official dedicated-server paks). Quests whose `Check()` reads
//! world/instance state or mutating helpers, whose class adds Blueprint
//! `CPF_Net` properties outside `AQuest` (`U-076`), or whose recorder carries
//! base modifiers/inventories/variables fail closed before any channel opens.
use std::collections::BTreeMap;

use anyhow::{Context, Result, bail, ensure};

use super::actor_open::{
    Block, BlockContent, BlockProperties, ComponentActorOpen, OpenComponent, Reference,
    compose_component_actor_open,
};
use super::archive::ByteOrder;
use super::control_messages::ArchiveText;
use super::manager_open::{ACTOR_STATE_CURRENT_BIOME, STAT_CONTAINER_HANDLE};
use super::prospect_info::{
    RelevantQuestActor, SavedQuestActor, SavedQuestManager, SavedQuestVariable,
};
pub use super::quest_table::{
    AQUEST_CLASS_CHECKSUM, QUEST_CLASSES, QuestCheckRule, QuestClassSpec,
};
use super::rep_layout::{
    ICARUS_ROW_HANDLE_NONE, NetworkName, PropertyStream, RepArray, RepCommand, RepEntry, RepValue,
};
use super::session_open::{
    ActorKey, ComposedActorOpen, ConnectionExports, NetGuidCache, PathSegment, resting_movement,
};

/// `AQuest` actor-block rep-layout handles (`quest_rep_layout`, `F-705`, `F-935`).
pub mod quest_actor_handle {
    pub const REMOTE_ROLE: u32 = 5;
    pub const REPLICATED_MOVEMENT: u32 = 6;
    pub const ROLE: u32 = 14;
    pub const STAT_CONTAINER: u32 = 16;
    pub const ACTOR_STATE: u32 = 17;
    pub const QUEST_ROW: u32 = 19;
    pub const RELEVANT_ACTORS: u32 = 20;
    pub const RELEVANT_CHARACTERS: u32 = 21;
    pub const ACTIVE_QUESTS: u32 = 22;
    pub const QUEST_VARIABLES: u32 = 23;
    pub const SUB_QUESTS: u32 = 24;
    pub const QUEST_PARENT: u32 = 25;
}

const SIMULATED_PROXY: u8 = 1;
const AUTHORITY: u8 = 3;
const TRANSFORM_TOLERANCE: f32 = 1e-4;
const LOCATION_TOLERANCE: f32 = 1e-3;

/// Look up a `D_Quests` class's derived checksum and `Check()` rule by short
/// class name (for example `"BPQ_Travel_C"` or `"Quest"`).
pub fn quest_class_spec(class_name: &str) -> Option<&'static QuestClassSpec> {
    QUEST_CLASSES
        .binary_search_by_key(&class_name, |spec| spec.class_name)
        .ok()
        .map(|index| &QUEST_CLASSES[index])
}

fn ascii_lower_u16(unit: u16) -> u16 {
    if (u16::from(b'A')..=u16::from(b'Z')).contains(&unit) {
        unit + 32
    } else {
        unit
    }
}

fn eq_utf16_str(units: &[u16], text: &str) -> bool {
    units.iter().copied().eq(text.encode_utf16())
}

fn eq_ascii_case_utf16_str(units: &[u16], text: &str) -> bool {
    let mut rhs = text.encode_utf16();
    for &unit in units {
        let Some(other) = rhs.next() else {
            return false;
        };
        if ascii_lower_u16(unit) != ascii_lower_u16(other) {
            return false;
        }
    }
    rhs.next().is_none()
}

/// `AQuest::GetBooleanVariable` (`0x1418d39c0`, `F-934`): exact case-sensitive
/// UTF-16 `VariableName` scan; returns `false` when absent.
pub fn get_boolean_variable(variables: &[SavedQuestVariable], name: &str) -> bool {
    variables
        .iter()
        .find(|variable| eq_utf16_str(&variable.name, name))
        .is_some_and(|variable| variable.bool_value)
}

/// `AQuest::GetFloatVariable` (`0x1418d4000`, `F-934`): exact case-sensitive
/// UTF-16 `VariableName` scan; returns `0.0` when absent.
pub fn get_float_variable(variables: &[SavedQuestVariable], name: &str) -> f32 {
    variables
        .iter()
        .find(|variable| eq_utf16_str(&variable.name, name))
        .map_or(0.0, |variable| variable.float_value)
}

/// `AQuest::GetIntVariable` (`0x1418d4470`, `F-934`): exact case-sensitive
/// UTF-16 `VariableName` scan; returns `0` when absent.
pub fn get_int_variable(variables: &[SavedQuestVariable], name: &str) -> i32 {
    variables
        .iter()
        .find(|variable| eq_utf16_str(&variable.name, name))
        .map_or(0, |variable| variable.int_value)
}

/// `AQuest::GetQuestFlag` (`0x1418d8460`, `F-934`): `FName` lookup (ASCII
/// case-insensitive) against `C015` `GameModeRecord.SessionFlagRecords`.
pub fn has_quest_flag(session_flags: &[Vec<u16>], flag: &str) -> bool {
    session_flags
        .iter()
        .any(|saved| eq_ascii_case_utf16_str(saved, flag))
}

/// Evaluate a quest class's compiled `Check()` completion rule against its
/// saved `QuestVariables` and `C015` `SessionFlags`. Fails closed on
/// [`QuestCheckRule::Impure`] or when [`QuestCheckRule::BoolVarFalseOnly`]
/// sees `true` (where `ManualRunOperation(s)` could execute).
pub fn evaluate_quest_check(
    rule: QuestCheckRule,
    variables: &[SavedQuestVariable],
    session_flags: &[Vec<u16>],
) -> Result<bool> {
    match rule {
        QuestCheckRule::AlwaysFalse => Ok(false),
        QuestCheckRule::AlwaysTrue => Ok(true),
        QuestCheckRule::BoolVar(name) => Ok(get_boolean_variable(variables, name)),
        QuestCheckRule::BoolVarFalseOnly(name) => {
            if get_boolean_variable(variables, name) {
                bail!("quest Check rule requires ManualRunOperation on true variable");
            }
            Ok(false)
        }
        QuestCheckRule::QuestFlag(flag) => Ok(has_quest_flag(session_flags, flag)),
        QuestCheckRule::AllBoolVars(names) => Ok(names
            .iter()
            .all(|name| get_boolean_variable(variables, name))),
        QuestCheckRule::AnyBoolVars(names) => Ok(names
            .iter()
            .any(|name| get_boolean_variable(variables, name))),
        QuestCheckRule::AnyQuestFlags(flags) => {
            Ok(flags.iter().any(|flag| has_quest_flag(session_flags, flag)))
        }
        QuestCheckRule::BoolVarOrQuestFlag(var, flag) => {
            Ok(get_boolean_variable(variables, var) || has_quest_flag(session_flags, flag))
        }
        QuestCheckRule::IntVarAtLeast(var, min) => Ok(get_int_variable(variables, var) >= min),
        QuestCheckRule::IntVarAtLeastVar(lhs, rhs) => {
            Ok(get_int_variable(variables, lhs) >= get_int_variable(variables, rhs))
        }
        QuestCheckRule::IntVarAtMost(var, max) => Ok(get_int_variable(variables, var) <= max),
        QuestCheckRule::FloatVarAtLeastVar(lhs, rhs) => {
            Ok(get_float_variable(variables, lhs) >= get_float_variable(variables, rhs))
        }
        QuestCheckRule::FloatVarGreater(var, bound) => {
            Ok(get_float_variable(variables, var) > bound)
        }
        QuestCheckRule::Impure => {
            bail!("quest Check rule depends on runtime world or Blueprint state")
        }
    }
}

/// Convert a saved `FName` string (`NameProperty` UTF-16 units) into the wire
/// [`NetworkName`] `FNetBitWriter::operator<<(FName&)` sends (`F-934`): a
/// non-empty prefix before `_` and a canonical decimal suffix `0` or
/// `[1-9][0-9]*` up to `i32::MAX - 1` splits into `number = suffix + 1`; all
/// other names carry `number = 0`.
pub fn fname_network_name(units: &[u16]) -> Result<NetworkName> {
    const MAX_EXTERNAL_NUMBER: u64 = (i32::MAX as u64) - 1;
    if let Some(underscore) = units.iter().rposition(|&unit| unit == u16::from(b'_')) {
        let prefix = &units[..underscore];
        let suffix = &units[underscore + 1..];
        if !prefix.is_empty() && !suffix.is_empty() {
            let canonical_digits = suffix == [u16::from(b'0')]
                || ((u16::from(b'1')..=u16::from(b'9')).contains(&suffix[0])
                    && suffix
                        .iter()
                        .all(|&unit| (u16::from(b'0')..=u16::from(b'9')).contains(&unit)));
            if canonical_digits {
                let mut parsed = 0_u64;
                let mut in_range = true;
                for &unit in suffix {
                    parsed = parsed * 10 + u64::from(unit - u16::from(b'0'));
                    if parsed > MAX_EXTERNAL_NUMBER {
                        in_range = false;
                        break;
                    }
                }
                if in_range {
                    return Ok(NetworkName::String {
                        text: ArchiveText::new(prefix)?,
                        number: u32::try_from(parsed + 1)?,
                    });
                }
            }
        }
    }
    Ok(NetworkName::String {
        text: ArchiveText::new(units)?,
        number: 0,
    })
}

/// One `D_Quests` row from `data.pak` (`Rows` position = row index).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuestTableRow {
    pub name: String,
    /// `None` when `Class` is omitted or `"None"`.
    pub class_path: Option<String>,
}

/// Extract `(Name, Class)` rows from `DataTables::objects("D_Quests")` in row
/// order without exposing save values.
pub fn quest_table_rows(
    objects: &[serde_json::Map<String, serde_json::Value>],
) -> Result<Vec<QuestTableRow>> {
    objects
        .iter()
        .map(|row| {
            let name = row
                .get("Name")
                .and_then(serde_json::Value::as_str)
                .context("data table D_Quests row has no string Name")?
                .to_owned();
            let class_path = match row.get("Class") {
                None => None,
                Some(serde_json::Value::String(text)) if text == "None" || text.is_empty() => None,
                Some(serde_json::Value::String(text)) => Some(text.clone()),
                Some(_) => bail!("data table D_Quests has a non-string Class"),
            };
            Ok(QuestTableRow { name, class_path })
        })
        .collect()
}

/// One stitched quest actor ready to build a [`QuestOpenPlan`]. `Debug` shows
/// counts and structural metadata only.
#[derive(Clone, PartialEq)]
pub struct StitchedQuest {
    pub key: ActorKey,
    pub archetype: Vec<PathSegment>,
    pub location: Option<[f32; 3]>,
    pub quest_row: u16,
    pub relevant_actors: Vec<RelevantQuestActor>,
    pub relevant_characters: Vec<RelevantQuestActor>,
    pub active_quests: Vec<ActorKey>,
    pub variables: Vec<SavedQuestVariable>,
    pub subquests: Vec<(Vec<u16>, ActorKey)>,
    pub parent: Option<ActorKey>,
}

impl std::fmt::Debug for StitchedQuest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("StitchedQuest")
            .field("active_quests", &self.active_quests.len())
            .field("variables", &self.variables.len())
            .field("subquests", &self.subquests.len())
            .field("has_parent", &self.parent.is_some())
            .finish_non_exhaustive()
    }
}

/// Result of stitching `C019` quest actors alongside `C018` `SavedQuestManager`
/// and `C015` `SessionFlags`.
#[derive(Clone, Debug, PartialEq)]
pub struct StitchedQuests {
    /// Stitched quest actors in channel-open order (reverse `C019` array order,
    /// matching `M-707` channels `36, 37, 42..47`).
    pub quests: Vec<StitchedQuest>,
    /// `SavedQuestManager` with `initial_quest` resolved against live stitched
    /// `C019` actors (`None` when `C019` has no live quests after mission
    /// completion, matching `C018::StitchingOperation`).
    pub quest_manager: Option<SavedQuestManager>,
}

fn validate_quest_transform(quest: &SavedQuestActor) -> Result<Option<[f32; 3]>> {
    ensure!(
        quest.translation.iter().all(|coord| coord.is_finite())
            && quest.rotation.iter().all(|coord| coord.is_finite())
            && quest.scale_3d.iter().all(|coord| coord.is_finite()),
        "saved quest actor has a non-finite transform"
    );
    let [qx, qy, qz, qw] = quest.rotation;
    let identity_rot = qx.abs() <= TRANSFORM_TOLERANCE
        && qy.abs() <= TRANSFORM_TOLERANCE
        && qz.abs() <= TRANSFORM_TOLERANCE
        && ((qw - 1.0).abs() <= TRANSFORM_TOLERANCE || (qw + 1.0).abs() <= TRANSFORM_TOLERANCE);
    ensure!(
        identity_rot,
        "saved quest actor has a non-identity rotation"
    );
    let [sx, sy, sz] = quest.scale_3d;
    let unit_scale = (sx - 1.0).abs() <= TRANSFORM_TOLERANCE
        && (sy - 1.0).abs() <= TRANSFORM_TOLERANCE
        && (sz - 1.0).abs() <= TRANSFORM_TOLERANCE;
    ensure!(unit_scale, "saved quest actor has a non-unit scale");
    let non_zero = quest
        .translation
        .iter()
        .any(|coord| coord.abs() > LOCATION_TOLERANCE);
    Ok(non_zero.then_some(quest.translation))
}

fn resolve_quest_archetype(
    quest: &SavedQuestActor,
    quest_rows: &[QuestTableRow],
) -> Result<(u16, &'static QuestClassSpec, Vec<PathSegment>)> {
    let quest_name =
        String::from_utf16(&quest.quest_name).map_err(|_| anyhow::anyhow!("invalid QuestName"))?;
    let class_name = String::from_utf16(&quest.class_name)
        .map_err(|_| anyhow::anyhow!("invalid ActorClassName"))?;
    let row_index = quest_rows
        .iter()
        .position(|row| row.name.eq_ignore_ascii_case(&quest_name))
        .context("saved quest row is not in D_Quests")?;
    ensure!(
        row_index < usize::from(ICARUS_ROW_HANDLE_NONE),
        "saved quest row index reaches the FRowHandle none sentinel"
    );
    let class_path = quest_rows[row_index]
        .class_path
        .as_deref()
        .context("saved quest D_Quests row has Class = None")?;
    let (package_path, row_class) = class_path
        .rsplit_once('.')
        .filter(|(pkg, cls)| pkg.starts_with('/') && !cls.is_empty())
        .context("saved quest D_Quests Class is not a package.Class path")?;
    ensure!(
        class_name == row_class,
        "saved quest ActorClassName disagrees with its D_Quests Class"
    );
    let spec = quest_class_spec(row_class).context("saved quest class is not in QUEST_CLASSES")?;
    ensure!(
        !spec.adds_net_props,
        "saved quest class adds replicated Blueprint properties outside AQuest"
    );
    let archetype = vec![
        PathSegment::new(&format!("Default__{row_class}"), spec.checksum, false),
        PathSegment::new(package_path, 0, false),
    ];
    Ok((u16::try_from(row_index)?, spec, archetype))
}

/// Stitch `saved_quests` (`C019`), `quest_manager` (`C018`) and
/// `session_flags` (`C015`) against `D_Quests` (`quest_rows`). Fails closed
/// before producing any open if any quest class is unknown, adds Blueprint
/// `CPF_Net` properties, carries base-recorder extras, has a broken or cyclic
/// `SubQuestRecords` link, or requires an impure `Check()` evaluation.
pub fn stitch_saved_quests(
    saved_quests: &[SavedQuestActor],
    quest_manager: Option<&SavedQuestManager>,
    session_flags: &[Vec<u16>],
    quest_rows: &[QuestTableRow],
) -> Result<StitchedQuests> {
    if saved_quests.is_empty() {
        let mut manager = quest_manager.cloned();
        if let Some(qm) = &mut manager {
            // `C018::StitchingOperation` (`0x1418f5ef0`) looks up
            // `InitialQuestRecord` among live restored actors; with zero
            // `C019` records (for example after mission completion),
            // `InitialQuest` stays null.
            qm.initial_quest = None;
        }
        return Ok(StitchedQuests {
            quests: Vec::new(),
            quest_manager: manager,
        });
    }

    let mut by_guid = BTreeMap::new();
    let mut resolved = Vec::with_capacity(saved_quests.len());
    for (index, quest) in saved_quests.iter().enumerate() {
        ensure!(
            quest.icarus_guid > 0,
            "saved quest actor has a non-positive IcarusActorGUID"
        );
        ensure!(
            by_guid.insert(quest.icarus_guid, index).is_none(),
            "saved quest actors share an IcarusActorGUID"
        );
        ensure!(
            quest.base_extras == 0,
            "saved quest actor carries base-recorder modifiers, inventories or variables"
        );
        let location = validate_quest_transform(quest)?;
        let (quest_row, spec, archetype) = resolve_quest_archetype(quest, quest_rows)?;
        resolved.push((location, quest_row, spec, archetype));
    }

    let mut manager = quest_manager.cloned();
    let mut resolved_initial = None;
    if let Some(qm) = &mut manager
        && let Some(uid) = qm.initial_quest
    {
        let &root_index = by_guid
            .get(&uid)
            .context("saved QuestManager InitialQuestRecord names no C019 quest actor")?;
        if !qm.initial_quest_class.is_empty() {
            ensure!(
                qm.initial_quest_class == saved_quests[root_index].class_name,
                "saved QuestManager InitialQuestRecord class disagrees with C019 quest actor"
            );
        }
        resolved_initial = Some(uid);
    }

    let reload_active = manager
        .as_ref()
        .is_none_or(|qm| resolved_initial.is_some() && qm.run_quests && !qm.mission_complete);

    let count = saved_quests.len();
    let mut parents: Vec<Option<ActorKey>> = vec![None; count];
    let mut subquest_lists: Vec<Vec<(Vec<u16>, ActorKey)>> = vec![Vec::new(); count];
    let mut active_lists: Vec<Vec<ActorKey>> = vec![Vec::new(); count];

    for (parent_index, parent_quest) in saved_quests.iter().enumerate() {
        let parent_key = ActorKey::Saved(u64::try_from(parent_quest.icarus_guid)?);
        for sub in &parent_quest.subquests {
            ensure!(
                sub.icarus_uid > 0,
                "saved SubQuestRecord has a non-positive RelevantActorIcarusUID"
            );
            let &child_index = by_guid
                .get(&sub.icarus_uid)
                .context("saved SubQuestRecord names no C019 quest actor")?;
            ensure!(
                child_index != parent_index,
                "saved SubQuestRecord links a quest actor to itself"
            );
            let child_quest = &saved_quests[child_index];
            ensure!(
                sub.class_name == child_quest.class_name,
                "saved SubQuestRecord SubQuestName disagrees with child ActorClassName"
            );
            ensure!(
                parents[child_index].replace(parent_key).is_none(),
                "saved C019 quest actor is claimed by more than one parent quest"
            );
            let child_key = ActorKey::Saved(u64::try_from(child_quest.icarus_guid)?);
            subquest_lists[parent_index].push((sub.row_name.clone(), child_key));
            if reload_active {
                let child_spec = resolved[child_index].2;
                let complete =
                    evaluate_quest_check(child_spec.check, &child_quest.variables, session_flags)?;
                if !complete {
                    active_lists[parent_index].push(child_key);
                }
            }
        }
    }

    // Cycle check across parent pointers (`n` hops must reach `None`).
    for start in 0..count {
        let mut cursor = parents[start];
        let mut hops = 0;
        while let Some(ActorKey::Saved(uid)) = cursor {
            hops += 1;
            ensure!(hops <= count, "saved C019 parent chain contains a cycle");
            let parent_idx = by_guid[&i32::try_from(uid)?];
            cursor = parents[parent_idx];
        }
    }

    let mut quests = Vec::with_capacity(count);
    for index in (0..count).rev() {
        let saved = &saved_quests[index];
        let (location, quest_row, _spec, archetype) = resolved[index].clone();
        quests.push(StitchedQuest {
            key: ActorKey::Saved(u64::try_from(saved.icarus_guid)?),
            archetype,
            location,
            quest_row,
            relevant_actors: saved.relevant_actors.clone(),
            relevant_characters: saved.relevant_characters.clone(),
            active_quests: std::mem::take(&mut active_lists[index]),
            variables: saved.variables.clone(),
            subquests: std::mem::take(&mut subquest_lists[index]),
            parent: parents[index],
        });
    }

    Ok(StitchedQuests {
        quests,
        quest_manager: manager,
    })
}

/// Release-level base layout and component checksums shared by every `AQuest`
/// open on a map (`ReleaseProfile::quest_open_base_plan`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuestOpenBasePlan {
    pub level: Vec<PathSegment>,
    pub layout: Vec<RepCommand>,
    pub actor_state_layout: Vec<RepCommand>,
    pub stat_container_checksum: u32,
    pub actor_state_checksum: u32,
    pub engine_net_version: u32,
}

/// Validated, connection-independent open plan for one `AQuest` actor. `Debug`
/// shows counts and structural metadata only.
#[derive(Clone, PartialEq)]
pub struct QuestOpenPlan {
    pub key: ActorKey,
    pub archetype: Vec<PathSegment>,
    pub level: Vec<PathSegment>,
    pub location: Option<[f32; 3]>,
    pub layout: Vec<RepCommand>,
    pub actor_state_layout: Vec<RepCommand>,
    pub stat_container_checksum: u32,
    pub actor_state_checksum: u32,
    pub values: BTreeMap<u32, RepValue>,
    pub references: BTreeMap<u32, Reference>,
    pub engine_net_version: u32,
}

impl std::fmt::Debug for QuestOpenPlan {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("QuestOpenPlan")
            .field("has_location", &self.location.is_some())
            .field("values", &self.values.len())
            .field("references", &self.references.len())
            .field("engine_net_version", &self.engine_net_version)
            .finish_non_exhaustive()
    }
}

fn keyed_actor_links(records: &[RelevantQuestActor]) -> Result<Vec<(ArchiveText, ActorKey)>> {
    let mut out = Vec::new();
    for record in records {
        if let Ok(uid) = u64::try_from(record.icarus_uid)
            && uid > 0
        {
            out.push((ArchiveText::new(&record.key)?, ActorKey::Saved(uid)));
        }
    }
    ensure!(
        u16::try_from(out.len()).is_ok(),
        "saved quest relevant-actor array exceeds u16::MAX"
    );
    Ok(out)
}

fn encode_quest_variables(variables: &[SavedQuestVariable]) -> Result<RepArray> {
    let count =
        u16::try_from(variables.len()).context("saved QuestVariables length exceeds u16::MAX")?;
    let mut entries = Vec::with_capacity(4 * variables.len());
    for (variable, element) in variables.iter().zip(0u32..) {
        for (field, value) in [
            RepValue::Str(ArchiveText::new(&variable.name)?),
            RepValue::Bool(variable.bool_value),
            RepValue::Float(variable.float_value.to_bits()),
            RepValue::Int(variable.int_value),
        ]
        .into_iter()
        .zip(1u32..)
        .map(|(value, field)| (field, value))
        {
            entries.push(RepEntry {
                handle: 4 * element + field,
                checksum: None,
                value,
            });
        }
    }
    Ok(RepArray {
        count,
        stream: PropertyStream {
            entries,
            terminator_checksum: None,
        },
    })
}

impl QuestOpenBasePlan {
    /// Build and validate a connection-independent [`QuestOpenPlan`] for
    /// `quest`.
    pub fn quest_plan(&self, quest: &StitchedQuest) -> Result<QuestOpenPlan> {
        use quest_actor_handle as h;

        let mut values = BTreeMap::from([
            (h::REMOTE_ROLE, RepValue::Byte(SIMULATED_PROXY)),
            (h::ROLE, RepValue::Byte(AUTHORITY)),
            (h::QUEST_ROW, RepValue::UInt16(quest.quest_row)),
        ]);
        if let Some(translation) = quest.location {
            let movement = resting_movement(translation)?;
            if movement != resting_movement([0.0; 3])? {
                values.insert(h::REPLICATED_MOVEMENT, RepValue::Movement(movement));
            }
        }
        if !quest.variables.is_empty() {
            values.insert(
                h::QUEST_VARIABLES,
                RepValue::DynamicArray(encode_quest_variables(&quest.variables)?),
            );
        }

        let mut references = BTreeMap::from([
            (STAT_CONTAINER_HANDLE, Reference::Component(0)),
            (h::ACTOR_STATE, Reference::Component(1)),
        ]);
        let relevant_actors = keyed_actor_links(&quest.relevant_actors)?;
        if !relevant_actors.is_empty() {
            references.insert(h::RELEVANT_ACTORS, Reference::KeyedActors(relevant_actors));
        }
        let relevant_characters = keyed_actor_links(&quest.relevant_characters)?;
        if !relevant_characters.is_empty() {
            references.insert(
                h::RELEVANT_CHARACTERS,
                Reference::KeyedActors(relevant_characters),
            );
        }
        if !quest.active_quests.is_empty() {
            ensure!(
                u16::try_from(quest.active_quests.len()).is_ok(),
                "saved ActiveQuests length exceeds u16::MAX"
            );
            references.insert(
                h::ACTIVE_QUESTS,
                Reference::Actors(quest.active_quests.clone()),
            );
        }
        if !quest.subquests.is_empty() {
            ensure!(
                u16::try_from(quest.subquests.len()).is_ok(),
                "saved SubQuests length exceeds u16::MAX"
            );
            let named = quest
                .subquests
                .iter()
                .map(|(row_name, actor)| Ok((fname_network_name(row_name)?, *actor)))
                .collect::<Result<Vec<_>>>()?;
            references.insert(h::SUB_QUESTS, Reference::NamedActors(named));
        }
        if let Some(parent) = quest.parent {
            references.insert(h::QUEST_PARENT, Reference::Actor(parent));
        }

        Ok(QuestOpenPlan {
            key: quest.key,
            archetype: quest.archetype.clone(),
            level: self.level.clone(),
            location: quest.location,
            layout: self.layout.clone(),
            actor_state_layout: self.actor_state_layout.clone(),
            stat_container_checksum: self.stat_container_checksum,
            actor_state_checksum: self.actor_state_checksum,
            values,
            references,
            engine_net_version: self.engine_net_version,
        })
    }
}

/// Compose one `AQuest` actor open on `channel_index` in the connection's byte
/// `order`, assigning GUIDs in exact handle order (`Stat Container`,
/// `ActorState`, `RelevantActors`, `RelevantCharacters`, `ActiveQuests`,
/// `SubQuests`, `QuestParent`).
pub fn compose_quest_open(
    cache: &mut NetGuidCache,
    exports: &mut ConnectionExports,
    plan: &QuestOpenPlan,
    channel_index: u32,
    order: ByteOrder,
) -> Result<ComposedActorOpen> {
    let components = vec![
        OpenComponent {
            name: "Stat Container".to_owned(),
            checksum: plan.stat_container_checksum,
            block: Some(Block::Encoded(BlockContent::create_only())),
        },
        OpenComponent {
            name: "ActorState".to_owned(),
            checksum: plan.actor_state_checksum,
            block: Some(Block::Composed(BlockProperties {
                layout: plan.actor_state_layout.clone(),
                references: BTreeMap::new(),
                values: BTreeMap::from([(ACTOR_STATE_CURRENT_BIOME, RepValue::Byte(0))]),
                fields: Vec::new(),
                field_bound: 0,
            })),
        },
    ];
    let open = ComponentActorOpen {
        key: plan.key,
        archetype: plan.archetype.clone(),
        level: plan.level.clone(),
        location: plan.location,
        layout: plan.layout.clone(),
        references: plan.references.clone(),
        values: plan.values.clone(),
        components,
        subobjects: Vec::new(),
        engine_net_version: plan.engine_net_version,
    };
    let (composed, _) = compose_component_actor_open(cache, exports, &open, channel_index, order)?;
    Ok(composed)
}

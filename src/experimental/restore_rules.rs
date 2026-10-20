//! EXPERIMENTAL restoration rules the client's state recorders apply.
//!
//! Each function is a pure rule over authored domain values, read from the
//! client recorders' record, validate, stitch and restore code (F-645 to F-665,
//! F-676, F-677, F-715, F-716, F-717, F-727, F-728, F-732, F-733, F-734, F-735,
//! F-736, F-737, F-741, F-742, F-743, F-744).
//! Nothing here decodes a save, looks up a class, constructs an object or
//! imports state: the rules pin the behavior a native world must reproduce when
//! it restores recorded actors. The normal server does not call them yet, and
//! an interpreted import stays gated (U-045).

/// Per-axis tolerance for matching a voxel record to its owner (F-645, M-644).
use serde::Serialize;

pub const VOXEL_LOCATION_TOLERANCE: f32 = 0.1;

/// Per-component tolerance against `FTransform::Identity` before
/// `UActorStateRecorderComponent::RestoreState` invokes `ApplyRecorderTransform`
/// (F-716).
pub const ACTOR_RECORDER_TRANSFORM_TOLERANCE: f32 = 1e-4;

/// Current `FProspectSaveState` schema version (F-715).
pub const PROSPECT_SAVE_STATE_CURRENT_VERSION: i32 = 2;

/// Saved elapsed level seconds below this take the saved game time (F-653).
pub const ELAPSED_SECONDS_FALLBACK_BELOW: i32 = 31;

/// The survival-level value meaning "not recorded" (F-648).
pub const UNRECORDED_LEVEL: i32 = -1;

/// Default lifetime assigned by `UActorStateRecorderComponent::UpdateOwnerToDatabaseState`
/// (`0x1418ba040`) when a saved `FModifierStateSaveData::ModifierLifetime` is `-1` (F-735).
pub const RESTORED_INDEFINITE_MODIFIER_LIFETIME: i32 = 100;

/// One saved or spawned actor link: a class or actor name and an Icarus id.
#[derive(Clone, Copy, Debug)]
pub struct ActorLink<'a> {
    pub name: &'a [u16],
    pub icarus_id: i32,
}

/// Whether two link names match (F-663).
///
/// Lengths must be equal. Both `FString::Equals(..., ESearchCase::IgnoreCase)`
/// (`0x141e48d20` server / `0x141e39920` client) and
/// `FGenericPlatformStricmp::Stricmp` (`0x141e48e20` server / `0x141e39a00`
/// client) test `(c1 | c2) & 0xffffff80 == 0` (`BothAscii`), fold ASCII units
/// `< 0x80` through `LowerAscii`, and compare non-ASCII units (`>= 0x80`)
/// without case folding, so `"Foundation"` matches `"FOUNDATION"` but not a
/// differently cased non-ASCII spelling.
pub fn names_match(saved: &[u16], actual: &[u16]) -> bool {
    saved.len() == actual.len()
        && saved.iter().zip(actual).all(|(left, right)| {
            if (left | right) & 0xff80 != 0 {
                left == right
            } else {
                (*left as u8).eq_ignore_ascii_case(&(*right as u8))
            }
        })
}

/// The first spawned actor a saved link stitches to: equal Icarus id and a
/// matching name (F-650, F-663). `None` leaves the link unresolved.
pub fn stitch<'a>(
    saved: ActorLink<'_>,
    spawned: impl IntoIterator<Item = ActorLink<'a>>,
) -> Option<usize> {
    spawned
        .into_iter()
        .position(|actor| actor.icarus_id == saved.icarus_id && names_match(saved.name, actor.name))
}

/// Stitches `RelevantPersistentActorRecords` (`C018`
/// `UIcarusQuestManagerRecorderComponent::Stitching` at `0x1418f3d30` server /
/// `0x1418f4540` client) or `RelevantActorRecords` (`C019`
/// `UIcarusQuestRecorderComponent::Stitching` at `0x1418f44e0` server /
/// `0x1418f4cf0` client) against `SpawnedActors` (F-650).
///
/// For each saved `(tag, link)` record (`0x28` B), resolves `link` against
/// `spawned` via [`stitch`] (equal `IcarusActorGUID` and [`names_match`] on
/// `actor->GetClass()->GetName()`), returning `(tag, matched_actor_index)` in
/// record order and omitting unresolved records.
pub fn stitch_quest_actor_records<'a, T: Clone>(
    records: &[(T, ActorLink<'_>)],
    spawned: &[ActorLink<'a>],
) -> Vec<(T, usize)> {
    records
        .iter()
        .filter_map(|(tag, link)| {
            stitch(*link, spawned.iter().copied()).map(|index| (tag.clone(), index))
        })
        .collect()
}

/// Values recorded by `UIcarusQuestManagerRecorderComponent::RecordOwnerState`
/// (`C018`, `0x1418e67b0` server / `0x1418e7b40` client) after
/// `UActorStateRecorderComponent::RecordOwnerState` (`0x1418a5b60` client)
/// (F-650, F-653, F-663).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuestManagerRecordPlan<C, T, R> {
    /// Updated `CurrentMissionRecord` (`recorder + 0x1c8..0x1e0`:
    /// `(class_name, icarus_actor_guid)`) when `owner->CurrentMission`
    /// (`owner + 0x2e0`) is non-null and has a `UActorStateRecorderComponent`
    /// (`0x141b9cbb0` client); `None` preserves existing `+0x1c8..0x1e0`.
    pub current_mission_record: Option<(C, i32)>,
    /// `RelevantActorRecords` (`recorder + 0x1f8`, count `+0x200`, stride
    /// `0x28` B: `(class_name, tag, guid)`) filtered from `owner + 0x368`
    /// (count `+0x370`, stride `0x18` B) to entries whose actor is non-null
    /// and has a `UActorStateRecorderComponent`.
    pub relevant_actor_records: Vec<(C, T, i32)>,
    /// `RelevantPersistentActorRecords` (`recorder + 0x208`, count `+0x210`,
    /// stride `0x28` B: `(class_name, tag, guid)`) filtered from `owner + 0x378`
    /// (count `+0x380`, stride `0x18` B) to entries whose actor is non-null
    /// and has a `UActorStateRecorderComponent`.
    pub relevant_persistent_actor_records: Vec<(C, T, i32)>,
    /// `FactionMissionRowName` (`owner + 0x2c0 -> recorder + 0x1c0`).
    pub faction_mission_row_name: R,
    /// `bMissionActive` (`owner + 0x2ea -> recorder + 0x1e0`).
    pub b_mission_active: bool,
    /// `MissionDifficulty` (`(uint)*(byte *)(owner + 0x2d8) -> recorder + 0x1e4`).
    pub mission_difficulty: u8,
    /// `MissionTimer` (`owner + 0x330 -> recorder + 0x1e8`).
    pub mission_timer: i32,
    /// `bTimerActive` (`owner + 0x334 -> recorder + 0x1e1`).
    pub b_timer_active: bool,
    /// `SelectedSeedRowName` (`owner + 0x338 -> recorder + 0x1ec`).
    pub selected_seed_row_name: R,
}

/// Evaluates `UIcarusQuestManagerRecorderComponent::RecordOwnerState` (`C018`,
/// `0x1418e67b0` server / `0x1418e7b40` client) (F-650, F-653, F-663).
///
/// Returns `None` when the owner is null or not an `AIcarusQuestManager`
/// (`0x141cfaa20` client; no `PendingKill` check is performed).
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub fn evaluate_quest_manager_record<C: Clone, T: Clone, R>(
    owner_is_quest_manager: bool,
    current_mission_with_recorder: Option<(C, i32)>,
    relevant_actors: &[(T, Option<(C, i32)>)],
    relevant_persistent_actors: &[(T, Option<(C, i32)>)],
    faction_mission_row_name: R,
    b_mission_active: bool,
    mission_difficulty: u8,
    mission_timer: i32,
    b_timer_active: bool,
    selected_seed_row_name: R,
) -> Option<QuestManagerRecordPlan<C, T, R>> {
    if !owner_is_quest_manager {
        return None;
    }
    let filter_records = |entries: &[(T, Option<(C, i32)>)]| -> Vec<(C, T, i32)> {
        entries
            .iter()
            .filter_map(|(tag, actor)| {
                actor
                    .as_ref()
                    .map(|(class_name, guid)| (class_name.clone(), tag.clone(), *guid))
            })
            .collect()
    };
    Some(QuestManagerRecordPlan {
        current_mission_record: current_mission_with_recorder,
        relevant_actor_records: filter_records(relevant_actors),
        relevant_persistent_actor_records: filter_records(relevant_persistent_actors),
        faction_mission_row_name,
        b_mission_active,
        mission_difficulty,
        mission_timer,
        b_timer_active,
        selected_seed_row_name,
    })
}

/// Values recorded by `UIcarusQuestRecorderComponent::RecordOwnerState`
/// (`C019`, `0x1418e6c40` server / `0x1418e7fd0` client) after
/// `UActorStateRecorderComponent::RecordOwnerState` (`0x1418a5b60` client)
/// (F-650, F-663).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuestRecordPlan<C, T, N, P> {
    /// `QuestDTKey.RowName` (`owner + 0x2c4 -> recorder + 0x1c0`).
    pub quest_dt_key: N,
    /// `RelevantActorRecords` (`recorder + 0x1c8`, count `+0x1d0`, stride
    /// `0x28` B: `(class_name, tag, guid)`) filtered from `owner + 0x2e0`
    /// (count `+0x2e8`, stride `0x18` B) to entries whose actor is non-null
    /// and has a `UActorStateRecorderComponent` (`0x141b9cbb0` client).
    pub relevant_actor_records: Vec<(C, T, i32)>,
    /// Secondary `RelevantActorRecords` (`recorder + 0x1d8`, count `+0x1e0`,
    /// stride `0x28` B: `(class_name, tag, guid)`) filtered from `owner + 0x2f0`
    /// (count `+0x2f8`, stride `0x18` B) to entries whose actor is non-null
    /// and has a `UActorStateRecorderComponent`.
    pub secondary_actor_records: Vec<(C, T, i32)>,
    /// Named actor records (`recorder + 0x1e8`, count `+0x1f0`, stride `0x20` B:
    /// `(class_name, fname_tag, guid)`) filtered from `owner + 0x320` (`800`,
    /// count `+0x328`, stride `0x18` B) to entries whose actor (`+0x10`) is
    /// non-null and has a `UActorStateRecorderComponent`.
    pub named_actor_records: Vec<(C, N, i32)>,
    /// Quest progress/variable records (`recorder + 0x1f8`, count `+0x200`,
    /// stride `0x20` B) copied unconditionally from `owner + 0x310` (count
    /// `+0x318`).
    pub quest_progress_records: Vec<P>,
}

/// Evaluates `UIcarusQuestRecorderComponent::RecordOwnerState` (`C019`,
/// `0x1418e6c40` server / `0x1418e7fd0` client) (F-650, F-663).
///
/// Returns `None` when the owner is null or not an `AIcarusQuest`
/// (`0x141cf3950` client; no `PendingKill` check is performed).
#[allow(clippy::type_complexity)]
pub fn evaluate_quest_record<C: Clone, T: Clone, N: Clone, P: Clone>(
    owner_is_quest: bool,
    quest_dt_key: N,
    relevant_actors: &[(T, Option<(C, i32)>)],
    secondary_actors: &[(T, Option<(C, i32)>)],
    named_actors: &[(N, Option<(C, i32)>)],
    quest_progress_records: &[P],
) -> Option<QuestRecordPlan<C, T, N, P>> {
    if !owner_is_quest {
        return None;
    }
    let filter_records = |entries: &[(T, Option<(C, i32)>)]| -> Vec<(C, T, i32)> {
        entries
            .iter()
            .filter_map(|(tag, actor)| {
                actor
                    .as_ref()
                    .map(|(class_name, guid)| (class_name.clone(), tag.clone(), *guid))
            })
            .collect()
    };
    let named_actor_records = named_actors
        .iter()
        .filter_map(|(name_tag, actor)| {
            actor
                .as_ref()
                .map(|(class_name, guid)| (class_name.clone(), name_tag.clone(), *guid))
        })
        .collect();
    Some(QuestRecordPlan {
        quest_dt_key,
        relevant_actor_records: filter_records(relevant_actors),
        secondary_actor_records: filter_records(secondary_actors),
        named_actor_records,
        quest_progress_records: quest_progress_records.to_vec(),
    })
}

/// Stitches `RelevantActorRecords` (`+0x1c8`, count `+0x1d0`, stride `0x18` B)
/// in `UPrebuiltStructureRecorderComponent::StitchingOperation` (`C027`,
/// `0x14177ec20` server / `0x14178c680` client) against `SpawnedActors` before
/// `APrebuiltStructure::RestorePrebuiltStructure` (`0x14173ff50` client)
/// (F-650, F-676, F-742).
///
/// Resolves each saved [`ActorLink`] against `spawned` via [`stitch`],
/// returning the matched actor indices in record order and omitting unresolved
/// records.
pub fn stitch_prebuilt_structure_actors<'a>(
    records: &[ActorLink<'_>],
    spawned: &[ActorLink<'a>],
) -> Vec<usize> {
    records
        .iter()
        .filter_map(|link| stitch(*link, spawned.iter().copied()))
        .collect()
}

/// Values recorded by `UPrebuiltStructureRecorderComponent::RecordOwnerState`
/// (`C027`, `0x14177ac40` server / `0x14177fe10` client) (F-650, F-658,
/// F-676).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrebuiltStructureRecordPlan<N, C> {
    /// `APrebuiltStructure->StructureRow.RowName` (`owner + 0x2f8`) copied to
    /// `recorder + 0x1c0`.
    pub structure_row_name: N,
    /// `RelevantActorRecords` (`recorder + 0x1c8`, stride `0x18` B:
    /// `(class_name, icarus_actor_guid)`) recorded for each non-null actor in
    /// `APrebuiltStructure + 0x2d8` that has a `UActorStateRecorderComponent`
    /// (`0x141b9cbb0` client).
    pub relevant_actor_records: Vec<(C, i32)>,
}

/// Evaluates `UPrebuiltStructureRecorderComponent::RecordOwnerState` (`C027`,
/// `0x14177ac40` server / `0x14177fe10` client) after
/// `UActorStateRecorderComponent::RecordOwnerState` (`0x1418a5b60` client)
/// (F-650, F-658, F-676).
///
/// Returns `None` when the owner is null or not an `APrebuiltStructure`
/// (`0x141cd5470` client). Otherwise filters `structure_actors` (`owner + 0x2d8`)
/// to actors that have a `UActorStateRecorderComponent` (`Some((class_name,
/// guid))`) and records `structure_row_name` (`owner + 0x2f8 -> +0x1c0`).
pub fn record_prebuilt_structure_actors<N, C: Clone>(
    owner_is_prebuilt_structure: bool,
    structure_row_name: N,
    structure_actors: &[Option<(C, i32)>],
) -> Option<PrebuiltStructureRecordPlan<N, C>> {
    if !owner_is_prebuilt_structure {
        return None;
    }
    let relevant_actor_records = structure_actors.iter().flatten().cloned().collect();
    Some(PrebuiltStructureRecordPlan {
        structure_row_name,
        relevant_actor_records,
    })
}

/// Actions evaluated by `UDeployableRecorderComponent::Stitching` (`C009`,
/// `0x1418ac8a0` server / `0x1418b0c80` client) (F-646, F-650).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DeployableStitchPlan {
    /// Matched `ABuildingBase` index in `SpawnedActors` passed to
    /// `ADeployable::SetFoundationActor` (`0x141acb730` client) when the owner
    /// is a non-pending-kill `ADeployable`.
    pub foundation_actor_index: Option<usize>,
    /// Whether `WhitelistedTames` (`+0x220` server / `+0x278` client) and
    /// `bAllowAllTames` (`+0x250` server / `+0x288` client -> `+0xb4`) are
    /// copied onto the owner's `UTameWhitelistComponent`.
    pub restore_tame_whitelist: bool,
}

/// Evaluates `UDeployableRecorderComponent::Stitching` (`C009`, `0x1418ac8a0`
/// server / `0x1418b0c80` client) (F-646, F-650).
pub fn evaluate_deployable_stitch<'a>(
    owner_is_valid_deployable: bool,
    has_tame_whitelist_component: bool,
    foundation_record: ActorLink<'_>,
    spawned_foundation_candidates: &[ActorLink<'a>],
) -> DeployableStitchPlan {
    let matched = stitch(
        foundation_record,
        spawned_foundation_candidates.iter().copied(),
    );
    if !owner_is_valid_deployable {
        return DeployableStitchPlan::default();
    }
    DeployableStitchPlan {
        foundation_actor_index: matched,
        restore_tame_whitelist: has_tame_whitelist_component,
    }
}

/// Candidate actor in `SpawnedActors` evaluated by
/// `UDrillRecorderComponent::StitchingOperation` (`C010`, `0x1418acbf0` server /
/// `0x1418b0ef0` client) (F-645, F-650, F-661).
#[derive(Clone, Copy, Debug)]
pub struct DrillStitchCandidate<'a> {
    /// Class name and `IcarusActorGUID` when the candidate is a valid
    /// `AIcarusActor` (`0x141b9e1b0` client).
    pub icarus_actor_link: Option<ActorLink<'a>>,
    /// Whether the candidate is a non-pending-kill `AResourceDeposit`
    /// (`0x141d22960` client).
    pub is_valid_resource_deposit: bool,
    /// Candidate's `RootComponent->RelativeLocation` (`+0x130 -> +0x1d0`), or
    /// `None` when `RootComponent` is `nullptr` (treated as `[0.0, 0.0, 0.0]`).
    pub root_location: Option<[f32; 3]>,
}

/// How `UDrillRecorderComponent::StitchingOperation` (`C010`, `0x1418acbf0`
/// server / `0x1418b0ef0` client) resolves and binds a drill's foundation actor
/// (F-645, F-650, F-661).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DrillStitchResolution {
    /// Matched `FoundationActorRecord` (`+0x230` server / `+0x258` client) by
    /// `IcarusActorGUID` and class name in the first pass over `SpawnedActors`.
    StitchedFoundationRecord(usize),
    /// `FoundationActorRecord` did not match any `AIcarusActor`; matched the
    /// first non-pending-kill `AResourceDeposit` whose root location is within
    /// strict `< 0.1` ([`VOXEL_LOCATION_TOLERANCE`]) per axis of the drill
    /// owner's root location.
    FallbackResourceDeposit(usize),
}

/// Evaluates `UDrillRecorderComponent::StitchingOperation` (`C010`,
/// `0x1418acbf0` server / `0x1418b0ef0` client) (F-645, F-650, F-661).
///
/// Returns `Some(DrillStitchResolution)` when `owner_is_valid_deployable` is
/// `true` and either the `FoundationActorRecord` pass or the fallback
/// `AResourceDeposit` location pass (`abs(delta) < 0.1` on all three axes, with
/// missing `RootComponent` treated as `[0.0, 0.0, 0.0]`) resolves an
/// `AIcarusActor` target for `ADeployable::SetFoundationActor` (`0x141acb730`
/// client); otherwise returns `None`.
pub fn evaluate_drill_stitch<'a>(
    owner_is_valid_deployable: bool,
    owner_root_location: Option<[f32; 3]>,
    foundation_record: ActorLink<'_>,
    spawned_candidates: &[DrillStitchCandidate<'a>],
) -> Option<DrillStitchResolution> {
    let explicit_match = spawned_candidates.iter().position(|candidate| {
        candidate.icarus_actor_link.is_some_and(|link| {
            link.icarus_id == foundation_record.icarus_id
                && names_match(foundation_record.name, link.name)
        })
    });
    let resolved = if let Some(index) = explicit_match {
        Some(DrillStitchResolution::StitchedFoundationRecord(index))
    } else {
        let owner_loc = owner_root_location.unwrap_or([0.0, 0.0, 0.0]);
        spawned_candidates
            .iter()
            .position(|candidate| {
                if !candidate.is_valid_resource_deposit {
                    return false;
                }
                let deposit_loc = candidate.root_location.unwrap_or([0.0, 0.0, 0.0]);
                owner_loc
                    .iter()
                    .zip(deposit_loc)
                    .all(|(owner_axis, deposit_axis)| {
                        (deposit_axis - owner_axis).abs() < VOXEL_LOCATION_TOLERANCE
                    })
            })
            .map(DrillStitchResolution::FallbackResourceDeposit)
    };
    if !owner_is_valid_deployable {
        return None;
    }
    resolved
}

/// Returns the indices of spawned `ACharacter` actors re-trapped by
/// `UCharacterTrapRecorderComponent::Stitching` (`C007`, `vftable + 0x468`:
/// `0x1418ac670` server / `0x1418b0a50` client) (F-650, F-743).
///
/// After `UDeployableRecorderComponent::Stitching` (`0x1418ac8a0` server /
/// `0x1418b0c80` client), `C007` scans `SpawnedActors` in order and calls
/// `ACharacterTrap::TrapCharacter` (`0x141a9c0e0` server / `0x141a90730`
/// client) for every non-pending-kill `ACharacter` whose `IcarusActorGUID`
/// (`+0x230` -> `vtable + 0x10`) equals `legacy_trapped_guid` (`+0x290`) or is
/// present in `trapped_character_guids` (`+0x298`).
pub fn stitch_trapped_character_guids(
    legacy_trapped_guid: i32,
    trapped_character_guids: &[i32],
    spawned_character_guids: &[i32],
) -> Vec<i32> {
    spawned_character_guids
        .iter()
        .copied()
        .filter(|guid| *guid == legacy_trapped_guid || trapped_character_guids.contains(guid))
        .collect()
}

/// Records `TrappedCharacterGUIDs` (`+0x298`) and legacy `TrappedCharacterGUID`
/// (`+0x290`) as `UCharacterTrapRecorderComponent::RecordOwnerState` (`C007`,
/// `0x1418a13c0` server / `0x1418a6b40` client) does (F-650, F-657).
///
/// - Clears `trapped_character_guids` on entry.
/// - When `owner_implements_trap_interface` (`UCharacterTrapInterface`
///   `0x141a8f2c0` client) is `true` and `trapped_characters_list`
///   (`Execute_GetTrappedCharacters` `0x141a904f0` client) has at least one
///   element, appends every `Some(guid)` (`IIcarusActorGuidInterface`) into
///   `trapped_character_guids`; if `trapped_character_guids` is non-empty,
///   returns without overwriting `legacy_trapped_guid` (`+0x290`).
/// - When `owner_implements_trap_interface` is `true`,
///   `trapped_characters_list` is empty, and `single_trapped_character_guid`
///   (`Execute_GetTrappedCharacter` `0x141a90480` client) is `Some(guid)`,
///   writes `guid` to `*legacy_trapped_guid` and returns.
/// - Otherwise sets `*legacy_trapped_guid = -1` (`0xffffffff`).
pub fn record_trapped_character_guids(
    owner_implements_trap_interface: bool,
    trapped_characters_list: &[Option<i32>],
    single_trapped_character_guid: Option<i32>,
    legacy_trapped_guid: &mut i32,
    trapped_character_guids: &mut Vec<i32>,
) {
    trapped_character_guids.clear();
    if owner_implements_trap_interface {
        if !trapped_characters_list.is_empty() {
            trapped_character_guids.extend(trapped_characters_list.iter().copied().flatten());
            if !trapped_character_guids.is_empty() {
                return;
            }
        } else if let Some(guid) = single_trapped_character_guid {
            *legacy_trapped_guid = guid;
            return;
        }
    }
    *legacy_trapped_guid = -1;
}

/// Compacts `AFLODManager::FLODTiles` (`+0x248`) in place and returns the stale
/// tile IDs to destroy (`FLODTiles \\ SpawnedActors`) as
/// `UFLODRecorderComponent::Stitching` (`C013`, `0x1418ac8f0` server /
/// `0x1418b12a0` client) does (F-646, F-716).
///
/// Removes `None` (`nullptr`) entries from `flod_tiles` in place, then returns
/// every tile ID in the compacted `flod_tiles` that does not appear in
/// `spawned_actors` (`0x1418a0d60` set-difference helper before
/// `AFLODTile::DestroyTile` at `0x1413e1040`).
pub fn compact_and_diff_flod_tiles<T: PartialEq + Clone>(
    flod_tiles: &mut Vec<Option<T>>,
    spawned_actors: &[T],
) -> Vec<T> {
    flod_tiles.retain(Option::is_some);
    flod_tiles
        .iter()
        .filter_map(|tile| tile.as_ref())
        .filter(|tile| !spawned_actors.contains(tile))
        .cloned()
        .collect()
}

/// Evaluates `UFLODRecorderComponent::RecordOwnerState` (`C013`,
/// `0x1418a1850` server / `0x1418a6fd0` client) (F-650, F-657).
///
/// Copies `NextInstanceFISMID` (`AFLODManager + 0x250 -> recorder + 0xd8`) iff
/// the owner is a non-null, non-pending-kill `AFLODManager` (`0x141b431c0`
/// client).
pub const fn evaluate_flod_record(
    owner_is_valid_flod_manager: bool,
    next_instance_fism_id: i32,
) -> Option<i32> {
    if !owner_is_valid_flod_manager {
        return None;
    }
    Some(next_instance_fism_id)
}

/// Resolved actor-array indices for `UPlayerRecorderComponent::Stitching`
/// (`C025`, `vftable + 0x468`: `0x1418f5160` server / `0x1418f58a0` client)
/// (F-648, F-650).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StitchedPlayerRecorderLinks {
    pub dropship_spawn_index: Option<usize>,
    pub dropship_index: Option<usize>,
    pub gravestone_index: Option<usize>,
}

/// Evaluates `UPlayerRecorderComponent::Stitching` (`C025`, `vftable + 0x468`:
/// `0x1418f5160` server / `0x1418f58a0` client) across `SpawnedActors` buckets
/// for `ADropshipSpawnPoint`, `ARocket`, and `AGravestone` (F-648, F-650).
///
/// For each of `AssignedDropshipSpawnUID` (`+0xf0`), `AssignedDropshipUID`
/// (`+0xf4`), and `AssignedGravestoneUID` (`+0xf8`), a saved UID of `0`
/// (`cmp ..., 0; je ...`) skips scanning (`None`); any non-zero UID resolves to
/// the index of the first non-pending-kill actor in the corresponding class
/// slice whose `IcarusActorGUID` (`+0x230` -> `vtable + 0x10`) equals the saved
/// UID.
pub fn stitch_player_recorder_assigned_uids(
    assigned_dropship_spawn_uid: i32,
    assigned_dropship_uid: i32,
    assigned_gravestone_uid: i32,
    spawned_dropship_spawn_guids: &[i32],
    spawned_dropship_guids: &[i32],
    spawned_gravestone_guids: &[i32],
) -> StitchedPlayerRecorderLinks {
    let find_uid = |target_uid: i32, candidates: &[i32]| -> Option<usize> {
        if target_uid == 0 {
            None
        } else {
            candidates.iter().position(|guid| *guid == target_uid)
        }
    };
    StitchedPlayerRecorderLinks {
        dropship_spawn_index: find_uid(assigned_dropship_spawn_uid, spawned_dropship_spawn_guids),
        dropship_index: find_uid(assigned_dropship_uid, spawned_dropship_guids),
        gravestone_index: find_uid(assigned_gravestone_uid, spawned_gravestone_guids),
    }
}

/// Whether a voxel record belongs to an owner at `actual` (F-645): every axis
/// within [`VOXEL_LOCATION_TOLERANCE`] inclusive. A NaN on either side never
/// matches. An owner without a root component is at the origin.
pub fn voxel_location_matches(saved: [f32; 3], actual: [f32; 3]) -> bool {
    saved
        .iter()
        .zip(actual)
        .all(|(saved, actual)| (actual - saved).abs() <= VOXEL_LOCATION_TOLERANCE)
}

/// Evaluates base `UIcarusStateRecorderComponent::DoesRecorderMatchActor`
/// (`vftable + 0x478`: `0x14188ba30` server / `0x141894350` client, shared by
/// non-actor `StateRecorderBlobs` classes `C013`, `C014`, `C022`, `C035`,
/// `C036`, `C037`, and `C018`) (F-743).
///
/// Compares `actor->GetName()` (`0x142176780` server / `0x142140920` client)
/// against `OwnerRecordName` (`+0xc0`) via `FGenericPlatformStricmp::Stricmp`
/// (`0x141e48e20` server / `0x141e39a00` client) without checking
/// `HasValidOwner` or `GetGuid()`.
pub fn does_state_recorder_match_actor(owner_record_name: &[u16], actor_name: &[u16]) -> bool {
    names_match(owner_record_name, actor_name)
}

/// Evaluates `UActorStateRecorderComponent::DoesRecorderMatchActor`
/// (`vftable + 0x478`: `0x14188b960` server / `0x141894280` client) (F-741,
/// F-742).
///
/// - When `recorder_has_valid_owner` is `true` (`HasValidOwner` `0x141898750`
///   server / `0x14189fb60` client), matches iff the candidate actor has a
///   `UIcarusStateRecorderComponent` whose `GetGuid()` (`vtable + 0x10`) equals
///   `recorder_guid`.
/// - When `recorder_has_valid_owner` is `false` (detached decoded recorder),
///   matches iff `actor_name` (`UObjectBaseUtility::GetName`) matches
///   `owner_record_name` (`+0xc8`) under [`names_match`].
pub fn does_actor_recorder_match_actor(
    recorder_has_valid_owner: bool,
    recorder_guid: i32,
    owner_record_name: &[u16],
    actor_recorder_guid: Option<i32>,
    actor_name: &[u16],
) -> bool {
    if recorder_has_valid_owner {
        actor_recorder_guid == Some(recorder_guid)
    } else {
        names_match(actor_name, owner_record_name)
    }
}

/// Evaluates `UVoxelRecorderComponent::DoesRecorderMatchActor`
/// (`vftable + 0x478`: `0x1418c9580` server / `0x1418cb460` client, shared by
/// `C031` `USpawnedVoxelRecorderComponent` and `C034`
/// `UVoxelRecorderComponent`) (F-645, F-742).
///
/// First runs [`does_actor_recorder_match_actor`]; if that returns `false`,
/// falls back to checking whether the candidate actor is an `AVoxelResource`
/// (`0x141de65c0` server / `0x141dd8d80` client) whose root component location
/// (or `[0.0, 0.0, 0.0]` when `RootComponent` at `+0x130` is `nullptr`) matches
/// `saved_location` (`+0x1e0`) within [`VOXEL_LOCATION_TOLERANCE`] (`0.1`) per
/// axis.
#[allow(clippy::too_many_arguments)]
pub fn does_voxel_recorder_match_actor(
    recorder_has_valid_owner: bool,
    recorder_guid: i32,
    owner_record_name: &[u16],
    saved_location: [f32; 3],
    actor_recorder_guid: Option<i32>,
    actor_name: &[u16],
    actor_is_voxel_resource: bool,
    actor_root_location: Option<[f32; 3]>,
) -> bool {
    if does_actor_recorder_match_actor(
        recorder_has_valid_owner,
        recorder_guid,
        owner_record_name,
        actor_recorder_guid,
        actor_name,
    ) {
        return true;
    }
    if !actor_is_voxel_resource {
        return false;
    }
    let actual_location = actor_root_location.unwrap_or([0.0, 0.0, 0.0]);
    voxel_location_matches(saved_location, actual_location)
}

/// Evaluates `UPlayerRecorderComponent::DoesRecorderMatchActor` (`C025`,
/// `vftable + 0x478`: `0x1418c9390` server / `0x1418cb270` client) (F-743).
///
/// Requires `save_data_version >= 1` (`+0xfc`) and a non-null, non-pending-kill
/// `AIcarusPlayerController` (`0x141c09e10` server / `0x141bfb490` client),
/// then compares `AIcarusPlayerController::GetPlayerCharacterID` (`0x141469c40`
/// server / `0x1414a69a0` client) against `this->PlayerID` (`+0xd8`, `ChrSlot`
/// at `+0xe8`) via `FPlayerCharacterID::operator==` (`0x14170a750` server /
/// `0x14171ae50` client: [`names_match`] on `UserID` and equal `ChrSlot`).
pub fn does_player_recorder_match_actor(
    save_data_version: i32,
    recorder_user_id: &[u16],
    recorder_chr_slot: i32,
    actor_is_valid_player_controller: bool,
    actor_user_id: &[u16],
    actor_chr_slot: i32,
) -> bool {
    save_data_version >= 1
        && actor_is_valid_player_controller
        && recorder_chr_slot == actor_chr_slot
        && names_match(actor_user_id, recorder_user_id)
}

/// Evaluates `UPlayerStateRecorderComponent::DoesRecorderMatchActor` (`C026`,
/// `vftable + 0x478`: `0x1418c9450` server / `0x1418cb330` client) (F-743).
///
/// Requires a non-null, non-pending-kill `AIcarusPlayerCharacter` (`0x141bf9710`
/// server / `0x141bec9a0` client) whose `vtable + 0x6c8` predicate returns
/// `true` and whose `AIcarusPlayerState` (`0x141428d30` server / `0x1414660d0`
/// client) is non-null and not pending kill, then compares
/// `player_state->PlayerCharacterID` (`+0x478` server / `+0x498` client)
/// against `this->PlayerID` (`+0x1c0`, `ChrSlot` at `+0x1d0`) via
/// `FPlayerCharacterID::operator==` (`0x14170a750` server / `0x14171ae50`
/// client).
pub fn does_player_state_recorder_match_actor(
    recorder_user_id: &[u16],
    recorder_chr_slot: i32,
    actor_is_valid_alive_player_character: bool,
    player_state_present_and_valid: bool,
    actor_user_id: &[u16],
    actor_chr_slot: i32,
) -> bool {
    actor_is_valid_alive_player_character
        && player_state_present_and_valid
        && recorder_chr_slot == actor_chr_slot
        && names_match(actor_user_id, recorder_user_id)
}

/// How a state recorder finds its owner on restore: the four values of the
/// recorders' owner-resolve policy, saved with every record (F-680).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OwnerResolvePolicy {
    FindOnly,
    RespawnOnly,
    FindOrRespawn,
    ManuallyResolved,
}

impl OwnerResolvePolicy {
    /// The policy a saved enum value names, or `None` for any other value.
    pub const fn from_saved(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::FindOnly),
            1 => Some(Self::RespawnOnly),
            2 => Some(Self::FindOrRespawn),
            3 => Some(Self::ManuallyResolved),
            _ => None,
        }
    }
}

/// Whether a voxel record validates (F-680): a fully mined voxel whose owner
/// is respawned fails, so its record is dropped on restore and absent from
/// the next save. Placed voxels, found rather than respawned, keep a fully
/// mined record.
pub fn voxel_record_valid(policy: OwnerResolvePolicy, fully_mined: bool) -> bool {
    !(fully_mined && policy == OwnerResolvePolicy::RespawnOnly)
}

/// Values recorded by `UVoxelRecorderComponent::RecordOwnerState`
/// (`0x1418e96c0` server / `0x1418ea6e0` client, shared by `C031`
/// `USpawnedVoxelRecorderComponent` and `C034` `UVoxelRecorderComponent`) after
/// `UActorStateRecorderComponent::RecordOwnerState` (`0x1418a5b60` client)
/// (F-645, F-650, F-680).
#[derive(Clone, Debug, PartialEq)]
pub struct VoxelRecordPlan<S, R> {
    /// `MinedSpheres` (`recorder + 0x1c0`, count `+0x1c8`, stride `0x10` B)
    /// moved from `AVoxelResource::GetMinedSpheres` (`0x1417364f0` client).
    pub mined_spheres: Vec<S>,
    /// `Location` (`recorder + 0x1e0..0x1ec`): `RootComponent + 0x1d0` when
    /// `RootComponent` (`+0x130`) is non-null, or `[0.0, 0.0, 0.0]` when null.
    pub location: [f32; 3],
    /// `bIsVoxelFullyMined` (`(AVoxelResource + 0x4b0) & 1 -> recorder + 0x1d0`).
    pub b_is_voxel_fully_mined: bool,
    /// `MaxUnminedVoxels` (`AVoxelResource + 0x588 -> recorder + 0x1d4`).
    pub max_unmined_voxels: i32,
    /// `CurrentUnminedVoxels` (`AVoxelResource + 0x58c -> recorder + 0x1d8`).
    pub current_unmined_voxels: i32,
    /// `NumResourcesGranted` (`AVoxelResource + 0x594 -> recorder + 0x1dc`).
    pub num_resources_granted: i32,
    /// `TotalResourceCount` (`AVoxelResource + 0x45c -> recorder + 0x1ec`).
    pub total_resource_count: f32,
    /// `ResourceType.RowName` (`AVoxelResource + 0x40c -> recorder + 0x1f0`)
    /// copied iff `FRowHandle::IsValid(owner + 0x40c)` (`0x143f855d0` client)
    /// returns `true`; `None` preserves existing `recorder + 0x1f0`.
    pub resource_type_row_override: Option<R>,
    /// Override for `OwnerResolvePolicy` (`recorder + 0xb2`): set to
    /// [`OwnerResolvePolicy::RespawnOnly`] (`1`) when
    /// `recorder->DurabilityData.bCanBreak` (`*(char *)(recorder + 0x134) != 0`)
    /// is `true`, or `None` when `+0xb2` is preserved unchanged.
    pub owner_resolve_policy_override: Option<OwnerResolvePolicy>,
}

/// Evaluates `UVoxelRecorderComponent::RecordOwnerState` (`0x1418e96c0`
/// server / `0x1418ea6e0` client, shared by `C031`
/// `USpawnedVoxelRecorderComponent` and `C034` `UVoxelRecorderComponent`)
/// (F-645, F-650, F-680).
///
/// Returns `None` when the owner is null, not an `AVoxelResource` (`0x141dd8d80`
/// client), or pending kill.
#[allow(clippy::too_many_arguments)]
pub fn evaluate_voxel_record<S: Clone, R>(
    owner_is_valid_voxel_resource: bool,
    mined_spheres: &[S],
    root_component_location: Option<[f32; 3]>,
    b_is_voxel_fully_mined: bool,
    max_unmined_voxels: i32,
    current_unmined_voxels: i32,
    num_resources_granted: i32,
    total_resource_count: f32,
    valid_resource_type_row_name: Option<R>,
    durability_can_break: bool,
) -> Option<VoxelRecordPlan<S, R>> {
    if !owner_is_valid_voxel_resource {
        return None;
    }
    Some(VoxelRecordPlan {
        mined_spheres: mined_spheres.to_vec(),
        location: root_component_location.unwrap_or([0.0, 0.0, 0.0]),
        b_is_voxel_fully_mined,
        max_unmined_voxels,
        current_unmined_voxels,
        num_resources_granted,
        total_resource_count,
        resource_type_row_override: valid_resource_type_row_name,
        owner_resolve_policy_override: durability_can_break
            .then_some(OwnerResolvePolicy::RespawnOnly),
    })
}

/// Actions selected by `UVoxelRecorderComponent::UpdateOwnerToDatabaseState`
/// (`0x1418f7bb0` server / `0x1418f8280` client, shared by `C031`
/// `USpawnedVoxelRecorderComponent` and `C034` `UVoxelRecorderComponent`)
/// (F-645, F-680).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VoxelDatabaseRestorePlan {
    /// When `OwnerResolvePolicy == RespawnOnly (1)` (`[this + 0xb2] == 1`),
    /// calls `USceneComponent::SetWorldScale3D` (`0x143348660` server /
    /// `0x1435deac0` client) on `AVoxelResource->RootComponent` (`+0x130`) with
    /// the saved scale from `ActorTransform` (`+0x100`).
    pub apply_respawned_world_scale: bool,
    /// When `bIsVoxelFullyMined` (`+0x1d0`) is `true` and `MinedSpheres.Num()`
    /// (`+0x1c8`) is `0`, synthesizes one full-bounds sphere centered at the
    /// mesh component bounds with radius `SphereRadius * 1.1` (`0x1452ac3cc`).
    pub synthesize_full_bounds_sphere: bool,
    /// When `[AVoxelResource + 0x548]` bit `0x1000` (`actor_disabled_bit_set`)
    /// is clear and the effective sphere list (`MinedSpheres` after optional
    /// synthesis) is non-empty, applies the mined spheres to octree voxels and
    /// copies `CurrentUnminedVoxels` (`+0x1d8 -> +0x590`) and
    /// `NumResourcesGranted` (`+0x1dc -> +0x594`).
    pub restore_mined_voxels_and_counters: bool,
    /// When `restore_mined_voxels_and_counters` is `true` and saved
    /// `TotalResourceCount` (`+0x1ec`) is `> 0.0`, overwrites
    /// `AVoxelResource->TotalResourceCount` (`+0x45c`).
    pub override_total_resource_count: Option<f32>,
}

/// Evaluates `UVoxelRecorderComponent::UpdateOwnerToDatabaseState`
/// (`0x1418f7bb0` server / `0x1418f8280` client) (F-645, F-680).
pub fn evaluate_voxel_database_restore(
    policy: OwnerResolvePolicy,
    b_is_voxel_fully_mined: bool,
    saved_mined_sphere_count: usize,
    actor_disabled_bit_set: bool,
    saved_total_resource_count: f32,
) -> VoxelDatabaseRestorePlan {
    let apply_respawned_world_scale = policy == OwnerResolvePolicy::RespawnOnly;
    let synthesize_full_bounds_sphere = b_is_voxel_fully_mined && saved_mined_sphere_count == 0;
    let effective_sphere_count = if synthesize_full_bounds_sphere {
        1
    } else {
        saved_mined_sphere_count
    };
    let restore_mined_voxels_and_counters = !actor_disabled_bit_set && effective_sphere_count > 0;
    let override_total_resource_count =
        if restore_mined_voxels_and_counters && saved_total_resource_count > 0.0 {
            Some(saved_total_resource_count)
        } else {
            None
        };
    VoxelDatabaseRestorePlan {
        apply_respawned_world_scale,
        synthesize_full_bounds_sphere,
        restore_mined_voxels_and_counters,
        override_total_resource_count,
    }
}

/// Outcome of the owner-resolution step in `StitchAndResolveRecorders`
/// (`0x1414bee00` server) for one validated recorder (F-680, F-741).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OwnerResolveStep {
    /// `HasValidOwner` (`0x141898750`) was already true on entry.
    KeptAlreadyBoundOwner,
    /// `OwnerResolvePolicy::ManuallyResolved` (`3`, e.g. `C034`
    /// `UVoxelRecorderComponent`): kept directly in `OutResolvedRecorders`
    /// without calling `FindOwner` (`+0x450`) or `RespawnOwner` (`+0x458`).
    KeptManuallyResolved,
    /// `FindOwner` (`+0x450`) found a level actor and `AttachRecorderToActor`
    /// (`0x14148ea30`, `bKeepRecorder = true`) succeeded.
    AttachedToFoundOwner,
    /// `RespawnAndAttachRecorder` (`0x14148d910` -> `RespawnOwner` `+0x458` ->
    /// `AttachRecorderToActor` `0x14148ea30`, `bKeepRecorder = false`)
    /// succeeded. When `destroyed_preexisting_owner` is true (`RespawnOnly`
    /// with `!skip_find_and_stitch` and a pre-existing level actor from
    /// `FindOwner`), `UWorld::DestroyActor(actor, false, true)` (`0x143331760`)
    /// ran before `RespawnOwner`.
    RespawnedAndAttached { destroyed_preexisting_owner: bool },
    /// `OwnerResolvePolicy::FindOnly` (`0`): `FindOwner` (`+0x450`) returned
    /// `nullptr` (logs Warning `0x1449f7ac0`).
    DroppedFindOnlyOwnerMissing,
    /// `OwnerResolvePolicy::FindOnly` (`0`): `FindOwner` returned an actor,
    /// but `AttachRecorderToActor` (`0x14148ea30`) returned `nullptr`.
    DroppedAttachFailed,
    /// `RespawnOnly` (`1`) or fallback `FindOrRespawn` (`2`):
    /// `RespawnAndAttachRecorder` (`0x14148d910`) returned `nullptr`.
    DroppedRespawnOrAttachFailed { destroyed_preexisting_owner: bool },
}

/// Evaluates `StitchAndResolveRecorders` (`0x1414bee00` server) for one
/// validated recorder (F-680, F-741).
///
/// - `RestoreProspectSaveState` (`0x1414b77e0`) passes
///   `skip_find_and_stitch = false`, then runs `Stitching` (`+0x468`) across all
///   resolved recorders (`0x1414bf4b6..0x1414bf4f6`).
/// - Instanced-level restore (`0x1414b7ae0`) passes
///   `skip_find_and_stitch = true`.
pub const fn evaluate_owner_resolve_step(
    has_valid_owner: bool,
    policy: OwnerResolvePolicy,
    skip_find_and_stitch: bool,
    found_level_owner: bool,
    attach_found_ok: bool,
    respawn_and_attach_ok: bool,
) -> OwnerResolveStep {
    if has_valid_owner {
        return OwnerResolveStep::KeptAlreadyBoundOwner;
    }
    match policy {
        OwnerResolvePolicy::FindOnly => {
            if !found_level_owner {
                OwnerResolveStep::DroppedFindOnlyOwnerMissing
            } else if attach_found_ok {
                OwnerResolveStep::AttachedToFoundOwner
            } else {
                OwnerResolveStep::DroppedAttachFailed
            }
        }
        OwnerResolvePolicy::RespawnOnly => {
            let destroyed_preexisting_owner = !skip_find_and_stitch && found_level_owner;
            if respawn_and_attach_ok {
                OwnerResolveStep::RespawnedAndAttached {
                    destroyed_preexisting_owner,
                }
            } else {
                OwnerResolveStep::DroppedRespawnOrAttachFailed {
                    destroyed_preexisting_owner,
                }
            }
        }
        OwnerResolvePolicy::FindOrRespawn => {
            if !skip_find_and_stitch && found_level_owner && attach_found_ok {
                OwnerResolveStep::AttachedToFoundOwner
            } else if respawn_and_attach_ok {
                OwnerResolveStep::RespawnedAndAttached {
                    destroyed_preexisting_owner: false,
                }
            } else {
                OwnerResolveStep::DroppedRespawnOrAttachFailed {
                    destroyed_preexisting_owner: false,
                }
            }
        }
        OwnerResolvePolicy::ManuallyResolved => OwnerResolveStep::KeptManuallyResolved,
    }
}

/// Outcome of `AttachRecorderToActor` (`0x14148ea30` -> `0x1414b6fc0` server)
/// when binding decoded recorder `source` to target `AActor` (F-741).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AttachRecorderOutcome {
    /// Target actor had no `UIcarusStateRecorderComponent` and
    /// `source.bCreateIfMissing` (`+0xb4`) was `false`.
    FailedMissingTargetComponent,
    /// Target actor had an existing `UIcarusStateRecorderComponent` whose
    /// `UClass` differed from `source->GetClass()` (logs Warning `0x1449f7780`).
    FailedClassMismatch,
    /// Serialized `source` (`0x1418aafb0`), called
    /// `target->RestoreState(&bytes, &FTransform::Identity)` (`+0x4a0`),
    /// rebound the `URecorderSubsystem` map entry to `target`, optionally
    /// called `source->ConditionalBeginDestroy()` when `!keep_source_recorder`,
    /// and called `target->UpdateOwnerToDatabaseState(true)` (`+0x460`).
    RestoredAndUpdatedOwner {
        created_component: bool,
        destroyed_temporary_source: bool,
    },
}

/// Evaluates `AttachRecorderToActor` (`0x14148ea30` -> `0x1414b6fc0` server)
/// (F-741).
pub const fn evaluate_attach_recorder_to_actor(
    actor_has_recorder_component: bool,
    create_if_missing: bool,
    classes_match: bool,
    source_is_target: bool,
    keep_source_recorder: bool,
) -> AttachRecorderOutcome {
    if !actor_has_recorder_component && !create_if_missing {
        return AttachRecorderOutcome::FailedMissingTargetComponent;
    }
    let created_component = !actor_has_recorder_component;
    if !created_component && !classes_match {
        return AttachRecorderOutcome::FailedClassMismatch;
    }
    let destroyed_temporary_source = !source_is_target && !keep_source_recorder;
    AttachRecorderOutcome::RestoredAndUpdatedOwner {
        created_component,
        destroyed_temporary_source,
    }
}

/// Resolves the `D_DeployableSetup` variant index selected by
/// `UDeployableRecorderComponent::ModifyOwnerSpawnParams` (`C009`,
/// `0x14189d2a0` server / `0x1418a1fb0` client) and
/// `UDeployableRecorderComponent::RespawnOwner` (`C009`, `0x1418a72a0` server /
/// `0x1418abfd0` client) from `FItemData::ItemDynamicData` (`+0x30`, stride
/// `8` B: `(property_type, value)`) (F-646).
///
/// Scans `dynamic_properties` in order for the first entry with
/// `property_type == 5` ([`DynamicItemPropertyKind::BuildingVariation`]).
/// When `variant_count == 0`, returns `None`; when a `BuildingVariation`
/// entry is found with `0 <= value < variant_count`, returns
/// `Some(value as usize)`; otherwise falls back to `Some(0)`.
pub fn resolve_deployable_setup_variant_index(
    dynamic_properties: &[(u8, i32)],
    variant_count: usize,
) -> Option<usize> {
    if variant_count == 0 {
        return None;
    }
    if let Some((_, value)) = dynamic_properties
        .iter()
        .find(|(prop_type, _)| *prop_type == DynamicItemPropertyKind::BuildingVariation as u8)
        && *value >= 0
        && (*value as usize) < variant_count
    {
        return Some(*value as usize);
    }
    Some(0)
}

/// Whether `UDrillRecorderComponent::RespawnOwner` (`C010`, `0x1418a74f0`
/// server / `0x1418abfd0` client) calls `ADrill::ActivateDrill` (`0x1416f8240`
/// server / `0x141709290` client) after `UIcarusActorRecorderComponent::RespawnOwner`:
/// requires the spawned actor to be an `ADrill` whose `D_ItemsStatic` row
/// (`EItemDataIndex = 5`) resolves and has `bAutoActivateDrill` (`[Row+0x153]`)
/// set (F-661, F-741).
pub const fn should_auto_activate_respawned_drill(
    actor_is_drill: bool,
    static_row_found: bool,
    auto_activate_drill_flag: bool,
) -> bool {
    actor_is_drill && static_row_found && auto_activate_drill_flag
}

/// Evaluates `UBuildingGridRecorderComponent::RecordOwnerState` (`C004`,
/// `0x1418a0aa0` server / `0x1418a6340` client) after
/// `UActorStateRecorderComponent::RecordOwnerState` (`0x1418a5b60` client)
/// (F-650, F-744).
///
/// Clears `recorded_grid_entries` (`recorder + 0x200`, count `+0x208`, stride
/// `0x20` B) and runs `ABuildingGrid::RecordBuildingGrid` (`0x141a6ee50` client)
/// iff the owner is a non-null `ABuildingGrid` (`0x141a6c980` client; no
/// `PendingKill` check is performed). Returns `true` when updated, or `false`
/// when `recorded_grid_entries` was preserved unchanged.
pub fn evaluate_building_grid_record<G: Clone>(
    owner_is_building_grid: bool,
    owner_grid_entries: &[G],
    recorded_grid_entries: &mut Vec<G>,
) -> bool {
    if !owner_is_building_grid {
        return false;
    }
    recorded_grid_entries.clear();
    recorded_grid_entries.extend_from_slice(owner_grid_entries);
    true
}

/// Whether `UBuildingGridRecorderComponent::UpdateOwnerToDatabaseState` (`C004`,
/// `0x1418b7770` server / `0x1418ba640` client) attempts
/// `ABuildingGrid::TryFastPathRestorePrebuiltGrid` (`0x1413a9690` server /
/// `0x1413e9220` client) before falling back to `ABuildingGrid::RestoreBuildingGrid`
/// (`0x141a7ac10` server / `0x141a6f1b0` client): requires a non-null
/// `GetInstancedLevel()` (`0x141896610` server / `0x14189de30` client) with
/// `bIsPrebuilt` (`+0x128`) set (F-744).
pub const fn should_try_fast_path_prebuilt_grid_restore(
    instanced_level_present: bool,
    instanced_level_is_prebuilt: bool,
) -> bool {
    instanced_level_present && instanced_level_is_prebuilt
}

/// Resolves `FItemData::DatabaseGUID` in `UItemStateRecorderComponent::RespawnOwner`
/// (`C021`, `0x1418a7660` server / `0x1418ac140` client) before
/// `AIcarusItem::SpawnItem` (`0x1413a67b0` server / `0x1413e5db0` client):
/// when `icarus_actor_guid > 0` (`+0x140`), uses `bound_guid` returned by
/// `UIcarusWorldRecorderSubsystem::BindActorGuid` (`0x1413dec50` server /
/// `0x14141c790` client); otherwise assigns `-1` (`0xffffffff`) (F-744).
pub const fn resolve_item_recorder_database_guid(icarus_actor_guid: i32, bound_guid: i32) -> i32 {
    if icarus_actor_guid > 0 {
        bound_guid
    } else {
        -1
    }
}

/// Values recorded by `UItemStateRecorderComponent::RecordOwnerState` (`C021`,
/// `0x1418a3540` server / `0x1418a8cc0` client, inherited by `C009`
/// `UDeployableRecorderComponent` and its subclasses) after
/// `UActorStateRecorderComponent::RecordOwnerState` (`0x1418a0170` server /
/// `0x1418a5b60` client) (F-646, F-650, F-744).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ItemStateRecordPlan<R, G, C> {
    /// `SpawnedContext` (`AIcarusItem + 0x4f8 -> recorder + 0x220`).
    pub spawned_context: u8,
    /// `ConstructionParameters` (`AIcarusItem + 0x4d0..0x4f5 -> recorder + 0x228..0x24d`).
    pub construction_parameters: C,
    /// `StaticItemDataRowName` (`FItemData::ItemStaticData.GetRowName() -> recorder + 0x1c0`).
    pub static_item_data_row_name: R,
    /// `DynamicData` (`recorder + 0x1e0`, stride `8` B: `(index, value)`),
    /// converted from `FItemData::ItemDynamicData` (`+0x60`, stride `8` B:
    /// `(property_type: u8, value: i32)`) by zero-extending `property_type`
    /// to `i32`.
    pub dynamic_data: Vec<(i32, i32)>,
    /// `Stats` (`recorder + 0x1f0`, stride `0x0c` B: `(stat_row_name, value)`),
    /// converted from `FItemData::CustomProperties.Stats` (`+0x70`, stride
    /// `0x1c` B).
    pub stats: Vec<(R, i32)>,
    /// `Alterations` (`recorder + 0x200`, stride `8` B: `alteration_row_name`),
    /// converted from `FItemData::CustomProperties.Alterations` (`+0x80`,
    /// stride `0x18` B).
    pub alterations: Vec<R>,
    /// `LivingItemSlots` (`recorder + 0x210`, `DA04`, stride `0x0c` B:
    /// `(exp, slot_row_name)`), converted from
    /// `FItemData::CustomProperties.LivingItemSlots` (`+0x90`, stride `0x18` B:
    /// `(slot_row_name, exp)`).
    pub living_item_slots: Vec<(i32, R)>,
    /// `DatabaseGUID` (`FItemData + 0x1b8 -> recorder + 0x1c8`, `16` B `FGuid`).
    pub database_guid: G,
    /// `ItemOwnerLookupId` (`FItemData + 0x1c8 -> recorder + 0x1d8`).
    pub item_owner_lookup_id: i32,
}

/// Evaluates `UItemStateRecorderComponent::RecordOwnerState` (`C021`,
/// `0x1418a3540` server / `0x1418a8cc0` client) (F-646, F-650, F-744).
///
/// Returns `None` when the owner is null, not an `AIcarusItem` (`0x141be62c0`
/// server), or pending kill.
#[allow(clippy::too_many_arguments)]
pub fn evaluate_item_state_record<R: Clone, G, C>(
    owner_is_valid_icarus_item: bool,
    spawned_context: u8,
    construction_parameters: C,
    static_item_data_row_name: R,
    item_dynamic_data: &[(u8, i32)],
    custom_stats: &[(R, i32)],
    custom_alterations: &[R],
    custom_living_item_slots: &[(R, i32)],
    database_guid: G,
    item_owner_lookup_id: i32,
) -> Option<ItemStateRecordPlan<R, G, C>> {
    if !owner_is_valid_icarus_item {
        return None;
    }
    let dynamic_data = item_dynamic_data
        .iter()
        .map(|&(prop_type, value)| (i32::from(prop_type), value))
        .collect();
    let living_item_slots = custom_living_item_slots
        .iter()
        .map(|(slot_row, exp)| (*exp, slot_row.clone()))
        .collect();
    Some(ItemStateRecordPlan {
        spawned_context,
        construction_parameters,
        static_item_data_row_name,
        dynamic_data,
        stats: custom_stats.to_vec(),
        alterations: custom_alterations.to_vec(),
        living_item_slots,
        database_guid,
        item_owner_lookup_id,
    })
}

/// Whether `UActorStateRecorderComponent::UpdateOwnerToDatabaseState`
/// (`0x1418b6ed0` server / `0x1418ba040` client, inherited by `C023`
/// `UOilGeyserRecorderComponent`) calls `AActor::SetActorTransform`
/// (`0x143348c70` server / `0x1435df0d0` client) with `&this->ActorTransform`
/// (`+0xe0`) and `ETeleportType::ResetPhysics` (`2`): requires
/// `bRecordActorTransform` (`[this+0xd9]`) to be non-zero (F-741, F-744).
pub const fn should_restore_actor_transform(b_record_actor_transform: bool) -> bool {
    b_record_actor_transform
}

/// Values recorded by `UEnzymeGeyserRecorderComponent::RecordOwnerState`
/// (`C012`, `0x1418a1790` server / `0x1418a6f10` client) after
/// `UActorStateRecorderComponent::RecordOwnerState` (`0x1418a5b60` client)
/// (F-650, F-657).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EnzymeGeyserRecordPlan<R> {
    /// `Completions` (`AEnzymeGeyser + 0x2c0 -> recorder + 0x1c0`).
    pub completions: i32,
    /// `CurrentRow.RowName` (`AEnzymeGeyser + 0x2c4 -> recorder + 0x1c4`,
    /// copied unconditionally without checking `FRowHandle::IsValid`).
    pub current_row_name: R,
}

/// Evaluates `UEnzymeGeyserRecorderComponent::RecordOwnerState` (`C012`,
/// `0x1418a1790` server / `0x1418a6f10` client) (F-650, F-657).
///
/// Returns `None` when the owner is null, not an `AEnzymeGeyser`
/// (`0x141af5260` client), or pending kill.
pub const fn evaluate_enzyme_geyser_record<R: Copy>(
    owner_is_valid_enzyme_geyser: bool,
    completions: i32,
    current_row_name: R,
) -> Option<EnzymeGeyserRecordPlan<R>> {
    if !owner_is_valid_enzyme_geyser {
        return None;
    }
    Some(EnzymeGeyserRecordPlan {
        completions,
        current_row_name,
    })
}

/// Values restored onto `AEnzymeGeyser` by
/// `UEnzymeGeyserRecorderComponent::UpdateOwnerToDatabaseState` (`C012`,
/// `0x1418b7c10` server / `0x1418baae0` client) (F-657).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EnzymeGeyserRestorePlan<R> {
    /// Saved `Completions` (`+0x1c0`) written unconditionally to
    /// `AEnzymeGeyser + 0x2c0`.
    pub completions: i32,
    /// Saved `CurrentRow` (`+0x1c4`) written to `AEnzymeGeyser + 0x2c4` iff
    /// `FRowHandle::IsValid` (`0x143da9940` server / `0x143f855d0` client)
    /// returns `true`.
    pub current_row_override: Option<R>,
}

/// Evaluates `UEnzymeGeyserRecorderComponent::UpdateOwnerToDatabaseState`
/// (`C012`, `0x1418b7c10` server / `0x1418baae0` client) (F-657).
///
/// Returns `None` when the owner is null, not an `AEnzymeGeyser`
/// (`0x141af5260` client), or pending kill.
pub fn evaluate_enzyme_geyser_restore<R>(
    owner_is_valid_enzyme_geyser: bool,
    saved_completions: i32,
    saved_current_row: R,
    current_row_is_valid: bool,
) -> Option<EnzymeGeyserRestorePlan<R>> {
    if !owner_is_valid_enzyme_geyser {
        return None;
    }
    Some(EnzymeGeyserRestorePlan {
        completions: saved_completions,
        current_row_override: current_row_is_valid.then_some(saved_current_row),
    })
}

/// Values and per-component records captured by
/// `UFLODTileRecorderComponent::RecordOwnerState` (`C014`, `0x1418a18f0`
/// server / `0x1418a7070` client) (F-650, F-676).
#[derive(Clone, Debug, PartialEq)]
pub struct FlodTileRecordPlan<N, R> {
    /// `TileName` (`AFLODTile + 0x220 -> recorder + 0xe0`).
    pub tile_name: N,
    /// `TileTransform` (`recorder + 0xf0..0x120`): copied from
    /// `RootComponent + 0x1c0` when `RootComponent` (`+0x130`) is non-null, or
    /// `FTransform::Identity` (`([0.0, 0.0, 0.0, 1.0], [0.0, 0.0, 0.0],
    /// [1.0, 1.0, 1.0])` at `0x1462f8cc0` client) when null.
    pub tile_transform: ([f32; 4], [f32; 3], [f32; 3]),
    /// `Radius` (`AFLODTile + 0x228 -> recorder + 0x120`).
    pub radius: f32,
    /// `Records` (`recorder + 0x128`, count `+0x130`, stride `0x40` B)
    /// populated by `RecordFromComponent` (`0x1418a3940` client) for each
    /// active component in `AFLODTile + 0x238`.
    pub component_records: Vec<(i32, R)>,
}

/// Evaluates `UFLODTileRecorderComponent::RecordOwnerState` (`C014`,
/// `0x1418a18f0` server / `0x1418a7070` client) (F-650, F-676).
///
/// Returns `None` when the owner is null, not an `AFLODTile` (`0x141b519a0`
/// client), or pending kill.
pub fn evaluate_flod_tile_record<N, R: Clone>(
    owner_is_valid_flod_tile: bool,
    tile_name: N,
    root_component_transform: Option<([f32; 4], [f32; 3], [f32; 3])>,
    radius: f32,
    active_component_records: &[(i32, R)],
) -> Option<FlodTileRecordPlan<N, R>> {
    if !owner_is_valid_flod_tile {
        return None;
    }
    let tile_transform = root_component_transform.unwrap_or((
        [0.0, 0.0, 0.0, 1.0],
        [0.0, 0.0, 0.0],
        [1.0, 1.0, 1.0],
    ));
    Some(FlodTileRecordPlan {
        tile_name,
        tile_transform,
        radius,
        component_records: active_component_records.to_vec(),
    })
}

/// Values and per-component records restored onto `AFLODTile` by
/// `UFLODTileRecorderComponent::UpdateOwnerToDatabaseState` (`C014`,
/// `0x1418b7cf0` server / `0x1418babc0` client) (F-676).
#[derive(Clone, Debug, PartialEq)]
pub struct FlodTileRestorePlan<N, R> {
    /// Saved `TileName` (`+0xe0`) written to `AFLODTile + 0x220`.
    pub tile_name: N,
    /// Saved `Radius` (`+0x120`) written to `AFLODTile + 0x228` after
    /// `AActor::SetActorTransform` (`+0xf0`, `ETeleportType::None`).
    pub radius: f32,
    /// Saved `FFLODRecord` entries (`+0x128`, stride `0x40` B) whose leading
    /// component key (`*puVar7`) resolved via `0x1418970d0(tile, key, 1)` to a
    /// non-null, non-pending-kill component for `0x1418ae2b0`
    /// (`RestoreToComponent`).
    pub restored_component_records: Vec<(i32, R)>,
}

/// Evaluates `UFLODTileRecorderComponent::UpdateOwnerToDatabaseState` (`C014`,
/// `0x1418b7cf0` server / `0x1418babc0` client) (F-676).
///
/// Returns `None` when the owner is null, not an `AFLODTile` (`0x141b519a0`
/// client), or pending kill.
pub fn evaluate_flod_tile_restore<N, R: Clone>(
    owner_is_valid_flod_tile: bool,
    saved_tile_name: N,
    saved_radius: f32,
    saved_records: &[(i32, R)],
    valid_tile_component_keys: &[i32],
) -> Option<FlodTileRestorePlan<N, R>> {
    if !owner_is_valid_flod_tile {
        return None;
    }
    let restored_component_records = saved_records
        .iter()
        .filter(|(key, _)| valid_tile_component_keys.contains(key))
        .cloned()
        .collect();
    Some(FlodTileRestorePlan {
        tile_name: saved_tile_name,
        radius: saved_radius,
        restored_component_records,
    })
}

/// Actions evaluated by
/// `UWorldTalentManagerRecorderComponent::StitchingOperation` (`C038`,
/// `0x14193aaa0` server / `0x141936a10` client) (F-676).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorldTalentStitchPlan<T> {
    /// Cloned `Talents` entries (`+0x1c0`, stride `0x18` B) copied onto
    /// `AWorldTalentManager + 0x2c8`.
    pub restored_talents: Vec<T>,
    /// Whether `AWorldTalentManager + 0x2e8` (`NetComponent`) is marked dirty
    /// (`vtable + 0x420` and `0x1408dd310(owner + 0x2d8, 0)`), which requires
    /// a non-null `NetComponent` with an active network state (`[+0xb0] ->
    /// vtable + 0x88 != 0`).
    pub mark_net_component_dirty: bool,
    /// Whether the authority broadcast helper (`0x14193cda0` client) runs after
    /// marking `NetComponent` dirty (`mark_net_component_dirty && net_role == 3`,
    /// `ENetRole::ROLE_Authority` at `owner + 0xf0`).
    pub notify_authority_talents_updated: bool,
}

/// Evaluates `UWorldTalentManagerRecorderComponent::RecordOwnerState` (`C038`,
/// `0x141930060` server / `0x14192bfd0` client) after
/// `UActorStateRecorderComponent::RecordOwnerState` (`0x1418a5b60` client)
/// (F-650, F-676).
///
/// Clears `recorded_talents` (`recorder + 0x1c0`) and copies `owner_talents`
/// (`AWorldTalentManager + 0x2c8`, count `+0x2d0`, stride `0x18` B) iff the
/// owner is a non-null `AWorldTalentManager` (`0x141e066d0` client; no
/// `PendingKill` check is performed). Returns `true` when updated, or `false`
/// when `recorded_talents` was preserved unchanged.
pub fn evaluate_world_talent_record<T: Clone>(
    owner_is_world_talent_manager: bool,
    owner_talents: &[T],
    recorded_talents: &mut Vec<T>,
) -> bool {
    if !owner_is_world_talent_manager {
        return false;
    }
    recorded_talents.clear();
    recorded_talents.extend_from_slice(owner_talents);
    true
}

/// Evaluates `UWorldTalentManagerRecorderComponent::StitchingOperation` (`C038`,
/// `0x14193aaa0` server / `0x141936a10` client) (F-676).
///
/// Returns `None` when the owner is null or not an `AWorldTalentManager`
/// (`0x141e066d0` client).
pub fn evaluate_world_talent_stitch<T: Clone>(
    owner_is_world_talent_manager: bool,
    saved_talents: &[T],
    net_component_has_active_state: bool,
    owner_net_role: u8,
) -> Option<WorldTalentStitchPlan<T>> {
    if !owner_is_world_talent_manager {
        return None;
    }
    let mark_net_component_dirty = net_component_has_active_state;
    let notify_authority_talents_updated = mark_net_component_dirty && owner_net_role == 3;
    Some(WorldTalentStitchPlan {
        restored_talents: saved_talents.to_vec(),
        mark_net_component_dirty,
        notify_authority_talents_updated,
    })
}

/// Values recorded or restored by `UInstancedLevelManagerRecorderComponent`
/// (`C020`, `RecordOwnerState` at `0x1419d5ba0` server / `0x1419ccef0` client;
/// `UpdateOwnerToDatabaseState` at `0x1419e28e0` server / `0x1419d9b90` client)
/// (F-650, F-658).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstancedLevelManagerRestorePlan<L, I> {
    /// Unlike most `C001` subclasses, `C020` checks that the owner is a
    /// non-pending-kill `AInstancedLevelManager` (`0x141a4a740` client) before
    /// invoking `UActorStateRecorderComponent::RecordOwnerState` (`0x1418a5b60`
    /// client) or `UpdateOwnerToDatabaseState` (`0x1418ba040` client).
    pub run_base_actor_state_restore: bool,
    /// `InstancedLevels` (`recorder + 0x1c0..0x230` <-> `owner + 0x2d0..0x340`)
    /// recorded via `RecordInstancedLevels` (`0x1419d3e90` client) or restored
    /// via `RestoreInstancedLevels` (`0x1419d4370` client).
    pub instanced_levels: L,
    /// `SpawnedRecorderIDs` (`recorder + 0x230` <-> `owner + 0x340`).
    pub spawned_recorder_ids: Vec<I>,
    /// `DestroyedRecorderIDs` (`recorder + 0x240` <-> `owner + 0x350`).
    pub destroyed_recorder_ids: Vec<I>,
}

/// Evaluates `UInstancedLevelManagerRecorderComponent::RecordOwnerState`
/// (`C020`, `0x1419d5ba0` server / `0x1419ccef0` client) (F-650, F-658).
pub fn evaluate_instanced_level_manager_record<L, I: Clone>(
    owner_is_valid_instanced_level_manager: bool,
    owner_instanced_levels: L,
    owner_spawned_recorder_ids: &[I],
    owner_destroyed_recorder_ids: &[I],
) -> Option<InstancedLevelManagerRestorePlan<L, I>> {
    evaluate_instanced_level_manager_restore(
        owner_is_valid_instanced_level_manager,
        owner_instanced_levels,
        owner_spawned_recorder_ids,
        owner_destroyed_recorder_ids,
    )
}

/// Evaluates `UInstancedLevelManagerRecorderComponent::UpdateOwnerToDatabaseState`
/// (`C020`, `0x1419e28e0` server / `0x1419d9b90` client) (F-658).
pub fn evaluate_instanced_level_manager_restore<L, I: Clone>(
    owner_is_valid_instanced_level_manager: bool,
    saved_instanced_levels: L,
    saved_spawned_recorder_ids: &[I],
    saved_destroyed_recorder_ids: &[I],
) -> Option<InstancedLevelManagerRestorePlan<L, I>> {
    if !owner_is_valid_instanced_level_manager {
        return None;
    }
    Some(InstancedLevelManagerRestorePlan {
        run_base_actor_state_restore: true,
        instanced_levels: saved_instanced_levels,
        spawned_recorder_ids: saved_spawned_recorder_ids.to_vec(),
        destroyed_recorder_ids: saved_destroyed_recorder_ids.to_vec(),
    })
}

/// Values recorded by `UBaseLevelTeleportRecorderComponent::RecordOwnerState`
/// (`C002`, `0x1419d5a20` server / `0x1419ccd70` client) (F-650, F-657, F-734).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BaseLevelTeleportRecordPlan {
    /// `ABaseLevelTeleport + 0x398` (`bIsActivated`) written to
    /// `recorder + 0x1c0`.
    pub b_is_activated: bool,
    /// `RemainingCooldown` written to `recorder + 0x1c4`: computed via
    /// [`remaining_cooldown`] against [`replicated_prospect_game_time`] when
    /// `cooldown_end_time > 0.0` and `GameState` (`+0x120`) is a non-pending-kill
    /// `AIcarusGameStateSurvival` (`0x141bc9a50` client); otherwise `0`.
    pub remaining_cooldown: i32,
}

/// Evaluates `UBaseLevelTeleportRecorderComponent::RecordOwnerState` (`C002`,
/// `0x1419d5a20` server / `0x1419ccd70` client) after
/// `UActorStateRecorderComponent::RecordOwnerState` (`0x1418a5b60` client)
/// (F-650, F-657, F-734).
///
/// Returns `None` when the owner is null, not an `ABaseLevelTeleport`
/// (`0x141a49150` client), or pending kill.
pub fn evaluate_base_level_teleport_record(
    owner_is_valid_teleport: bool,
    owner_is_activated: bool,
    cooldown_end_time: f32,
    game_state_is_valid_survival: bool,
    world_time_seconds: f32,
    replicated_prospect_time_offset: f32,
) -> Option<BaseLevelTeleportRecordPlan> {
    if !owner_is_valid_teleport {
        return None;
    }
    let remaining_cooldown = if cooldown_end_time > 0.0 && game_state_is_valid_survival {
        let now =
            replicated_prospect_game_time(world_time_seconds, replicated_prospect_time_offset);
        remaining_cooldown(cooldown_end_time, now)
    } else {
        0
    };
    Some(BaseLevelTeleportRecordPlan {
        b_is_activated: owner_is_activated,
        remaining_cooldown,
    })
}

/// Actions evaluated by
/// `UBaseLevelTeleportRecorderComponent::UpdateOwnerToDatabaseState` (`C002`,
/// `0x1419e27c0` server / `0x1419d9a70` client) (F-657, F-734).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BaseLevelTeleportRestorePlan {
    /// Saved `bIsActivated` (`recorder + 0x1c0`) written to
    /// `ABaseLevelTeleport + 0x398` before `OnRep_IsActivated` (`0x141a524f0`
    /// client).
    pub b_is_activated: bool,
    /// Whether `OnRep_IsActivated` (`0x141a524f0` client) and timer-handle
    /// cleanup (`owner + 0x399`, `0x1419b08f0` client) run.
    pub notify_activated_and_clear_timer: bool,
    /// Saved `RemainingCooldown` (`recorder + 0x1c4`) passed to
    /// `ABaseLevelTeleport::StartCooldownTimer` (`0x1419d37d0` client).
    pub start_cooldown_seconds: i32,
}

/// Evaluates `UBaseLevelTeleportRecorderComponent::UpdateOwnerToDatabaseState`
/// (`C002`, `0x1419e27c0` server / `0x1419d9a70` client) after
/// `UActorStateRecorderComponent::UpdateOwnerToDatabaseState` (`0x1418ba040`
/// client) (F-657, F-734).
///
/// Returns `None` when the owner is null, not an `ABaseLevelTeleport`
/// (`0x141a49150` client), or pending kill.
pub const fn evaluate_base_level_teleport_restore(
    owner_is_valid_teleport: bool,
    saved_is_activated: bool,
    saved_remaining_cooldown: i32,
) -> Option<BaseLevelTeleportRestorePlan> {
    if !owner_is_valid_teleport {
        return None;
    }
    Some(BaseLevelTeleportRestorePlan {
        b_is_activated: saved_is_activated,
        notify_activated_and_clear_timer: true,
        start_cooldown_seconds: saved_remaining_cooldown,
    })
}

/// Evaluates `UCaveAirNavRecorderComponent::UpdateOwnerToDatabaseState` (`C005`,
/// `0x1418b7860` server / `0x1418ba730` client) after
/// `UActorStateRecorderComponent::UpdateOwnerToDatabaseState` (`0x1418ba040`
/// client) (F-657, F-677).
///
/// Only when `owner_is_cave_ai_spawner` (`AIcarusCaveAISpawner`, `0x141ba6890`
/// client) is `true` and `saved_spawn_data` (`recorder + 0x1c0`, count at
/// `+0x1c8`) is non-empty does `C005` clear `owner + 0x310`, copy
/// `saved_spawn_data` into `owner + 0x310`, and call
/// `AIcarusCaveAISpawner::UpdateStateFromDatabase` (`0x141409d70` client,
/// modeled by [`restore_cave_spawn_times`]). When `saved_spawn_data` is empty,
/// `owner + 0x310` and `configured_spawner_classes` are left untouched and
/// `None` is returned.
pub fn evaluate_cave_air_nav_restore(
    owner_is_cave_ai_spawner: bool,
    configured_spawner_classes: &mut [CaveSpawnTimes],
    saved_spawn_data: &[CaveSpawnTimes],
) -> Option<bool> {
    if !owner_is_cave_ai_spawner || saved_spawn_data.is_empty() {
        return None;
    }
    Some(restore_cave_spawn_times(
        configured_spawner_classes,
        saved_spawn_data,
    ))
}

/// Evaluates `UCaveEntranceRecorderComponent::RecordOwnerState` (`C006`,
/// `0x1418a1300` server / `0x1418a6a80` client) after
/// `UActorStateRecorderComponent::RecordOwnerState` (`0x1418a0170` server /
/// `0x1418a5b60` client) (F-650, F-657, F-744).
///
/// Overwrites `recorded_mined_spheres` (`recorder + 0x1c0`) with
/// `voxel_component_spheres` (`UVoxelComponent::GetMinedSpheres`,
/// `0x141726e90` server / `0x1417364f0` client) iff
/// `owner_implements_cave_entrance_interface` (`UCaveEntranceInterface`,
/// `0x141a898e0` server / `0x141a7e240` client) is `true` and
/// `ICaveEntranceInterface::Execute_GetCaveEntranceVoxelComponent`
/// (`0x141a8b850` server / `0x141a7fea0` client) returned a non-null
/// `UVoxelComponent` (`Some(voxel_component_spheres)`). Returns `true` when
/// `recorded_mined_spheres` was updated, or `false` when preserved.
pub fn evaluate_cave_entrance_record<S: Clone>(
    owner_implements_cave_entrance_interface: bool,
    voxel_component_spheres: Option<&[S]>,
    recorded_mined_spheres: &mut Vec<S>,
) -> bool {
    if owner_implements_cave_entrance_interface && let Some(spheres) = voxel_component_spheres {
        recorded_mined_spheres.clear();
        recorded_mined_spheres.extend_from_slice(spheres);
        return true;
    }
    false
}

/// Evaluates `UCaveEntranceRecorderComponent::UpdateOwnerToDatabaseState`
/// (`C006`, `0x1418b7b40` server / `0x1418baa10` client) after
/// `UActorStateRecorderComponent::UpdateOwnerToDatabaseState` (`0x1418b6ed0`
/// server / `0x1418ba040` client) (F-657, F-744).
///
/// Calls `ICaveEntranceInterface::Execute_RestoreCaveEntranceMinedSpheres(owner, +0x1c0)`
/// (`0x141a8b930` server / `0x141a7ff80` client) iff the owner is non-null and
/// its `UClass` (`+0x10`) implements `UCaveEntranceInterface` (`0x141a898e0`
/// server / `0x141a7e240` client; no `PendingKill` check is performed).
pub fn evaluate_cave_entrance_restore<S: Clone>(
    owner_implements_cave_entrance_interface: bool,
    saved_mined_spheres: &[S],
) -> Option<Vec<S>> {
    owner_implements_cave_entrance_interface.then(|| saved_mined_spheres.to_vec())
}

/// Alias for [`evaluate_cave_entrance_record`] (`C006`, `0x1418a1300` server /
/// `0x1418a6a80` client; see [`evaluate_crop_plot_state_transfer`] for `C008`
/// `UCropPlotRecorderComponent`) (F-650, F-657, F-744).
pub fn evaluate_crop_plot_record<S: Clone>(
    owner_implements_interface: bool,
    component_states: Option<&[S]>,
    recorded_states: &mut Vec<S>,
) -> bool {
    evaluate_cave_entrance_record(
        owner_implements_interface,
        component_states,
        recorded_states,
    )
}

/// Alias for [`evaluate_cave_entrance_restore`] (`C006`, `0x1418b7b40` server /
/// `0x1418baa10` client; see [`evaluate_crop_plot_state_transfer`] for `C008`
/// `UCropPlotRecorderComponent`) (F-657, F-744).
pub fn evaluate_crop_plot_restore<S: Clone>(
    owner_implements_interface: bool,
    saved_states: &[S],
) -> Option<Vec<S>> {
    evaluate_cave_entrance_restore(owner_implements_interface, saved_states)
}

/// Evaluates `UBedRecorderComponent::RecordOwnerState` (`C003`, `0x1418a09f0`
/// server / `0x1418a6290` client) and
/// `UBedRecorderComponent::UpdateOwnerToDatabaseState` (`0x1418b7710` server /
/// `0x1418ba5e0` client) after the base deployable/actor recorder step (F-650,
/// F-744).
///
/// Transfers `SpawnPointPlayers` (`TArray<FString>`, `recorder + 0x290`, stride
/// `0x10` B) via `IBedInterface::Execute_GetSpawnPointPlayers` (`0x141a5d9c0`
/// server / `0x141a52040` client) / `Execute_SetSpawnPointPlayers`
/// (`0x141a5dc00` server / `0x141a52280` client) iff the owner is non-null and
/// its `UClass` (`+0x10`) implements `UBedInterface` (`0x141a531c0` server /
/// `0x141a4a500` client).
pub fn evaluate_bed_spawn_point_players_transfer<P: Clone>(
    owner_implements_bed_interface: bool,
    spawn_point_players: &[P],
) -> Option<Vec<P>> {
    owner_implements_bed_interface.then(|| spawn_point_players.to_vec())
}

/// Evaluates `UCropPlotRecorderComponent::RecordOwnerState` (`C008`,
/// `0x1418a1540` server / `0x1418a6cc0` client) and
/// `UCropPlotRecorderComponent::UpdateOwnerToDatabaseState` (`0x1418b7bb0`
/// server / `0x1418baa80` client) after the base deployable/actor recorder step
/// (F-650, F-657, F-744).
///
/// Transfers `CropPlotSaveData` (`recorder + 0x290`) via
/// `ICropPlotInterface::Execute_RecordPlotState` (`0x141ac7360` server /
/// `0x141ab39c0` client) / `Execute_RestorePlotState` (`0x141ac7530` server /
/// `0x141ab3b90` client) iff the owner is non-null and its `UClass` (`+0x10`)
/// implements `UCropPlotInterface` (`0x141abe3b0` server / `0x141ab2d40`
/// client).
pub const fn evaluate_crop_plot_state_transfer<D: Copy>(
    owner_implements_crop_plot_interface: bool,
    plot_state: D,
) -> Option<D> {
    if !owner_implements_crop_plot_interface {
        return None;
    }
    Some(plot_state)
}

/// Values recorded or restored by `USpawnBlockerRecorderComponent` (`C030`,
/// `RecordOwnerState` at `0x1418e9500` server / `0x1418e9830` client;
/// `UpdateOwnerToDatabaseState` at `0x1418f7930` server / `0x1418f81f0` client)
/// (F-650, F-657).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpawnBlockerState<R, N> {
    /// `EffectiveRadius` (`recorder + 0x290`) from
    /// `ISpawnBlockerInterface::Execute_GetSpawnBlockerEffectiveRadius`
    /// (`0x141d698e0` client) / passed to `Execute_UpdateSpawnBlocker`
    /// (`0x141d69ad0` client).
    pub effective_radius: R,
    /// `Transform` (`recorder + 0x2a8..0x2b8`, `16` B) from
    /// `ISpawnBlockerInterface::Execute_GetSpawnBlockerTransform`
    /// (`0x141d69790` client) / passed to `Execute_UpdateSpawnBlocker`
    /// (`0x141d69ad0` client).
    pub blocker_transform: [f32; 4],
    /// `SpawnBlockerRow.RowName` (`recorder + 0x2b8`, `8` B) from
    /// `ISpawnBlockerInterface::Execute_GetSpawnBlockerRow` (`0x141d69830`
    /// client) / passed to `Execute_SetSpawnBlockerRow` (`0x141d69a10` client).
    pub spawn_blocker_row_name: N,
}

/// Evaluates `USpawnBlockerRecorderComponent::RecordOwnerState` (`C030`,
/// `0x1418e9500` server / `0x1418e9830` client) and
/// `USpawnBlockerRecorderComponent::UpdateOwnerToDatabaseState` (`0x1418f7930`
/// server / `0x1418f81f0` client) after the base deployable/actor recorder step
/// (F-650, F-657).
///
/// Returns `Some(SpawnBlockerState)` iff the owner is non-null and its `UClass`
/// (`+0x10`) implements `USpawnBlockerInterface` (`0x141d67140` client).
pub const fn evaluate_spawn_blocker_transfer<R: Copy, N: Copy>(
    owner_implements_spawn_blocker_interface: bool,
    effective_radius: R,
    blocker_transform: [f32; 4],
    spawn_blocker_row_name: N,
) -> Option<SpawnBlockerState<R, N>> {
    if !owner_implements_spawn_blocker_interface {
        return None;
    }
    Some(SpawnBlockerState {
        effective_radius,
        blocker_transform,
        spawn_blocker_row_name,
    })
}

/// Entry in `AIcarusCaveAISpawner::SpawnedMap` (`owner + 0x2c0`, stride `0x48` B)
/// inspected by `UCaveAirNavRecorderComponent::RecordOwnerState` (`C005`,
/// `0x1418a0d20` server / `0x1418a6480` client) (F-650, F-677).
#[derive(Clone, Debug, PartialEq)]
pub struct CaveSpawnerMapEntry<'a> {
    /// Whether the map key (`+0x00`) is a non-null `UClass*` (`0x143cd2090`
    /// client) with a non-null `GetClass()` (`+0x10`).
    pub has_valid_uclass: bool,
    /// `key->GetClass()->GetName()` (`*(key + 0x10) + 0x18`).
    pub uclass_type_name: &'a [u16],
    /// `key->ClassGeneratedBy->GetName()` (`*(key + 0xe0) + 0x18`) when
    /// `ClassGeneratedBy` (`+0xe0`) is non-null.
    pub class_generated_by_name: Option<&'a [u16]>,
    /// Spawn times array (`+0x30`, count at `+0x38`).
    pub times: &'a [f32],
}

/// Evaluates `UCaveAirNavRecorderComponent::RecordOwnerState` (`C005`,
/// `0x1418a0d20` server / `0x1418a6480` client) after
/// `UActorStateRecorderComponent::RecordOwnerState` (`0x1418a5b60` client)
/// (F-650, F-677).
///
/// Only when `owner_is_cave_ai_spawner` (`AIcarusCaveAISpawner`, `0x141ba6890`
/// client) is `true` and `spawner_map_entries` (`owner + 0x2c0`, count
/// `+0x2c8 - +0x2f4`) is non-empty does `C005` clear `recorded_spawn_data`
/// (`recorder + 0x1c0`) and append each entry with `has_valid_uclass &&
/// !entry.times.is_empty()`, unwrapping `"BlueprintGeneratedClass"` via
/// [`resolve_cave_ai_class_name`]. When `spawner_map_entries` is empty or
/// `!owner_is_cave_ai_spawner`, `recorded_spawn_data` is preserved unchanged
/// and `false` is returned.
pub fn evaluate_cave_air_nav_record(
    owner_is_cave_ai_spawner: bool,
    spawner_map_entries: &[CaveSpawnerMapEntry<'_>],
    recorded_spawn_data: &mut Vec<CaveSpawnTimes>,
) -> bool {
    if !owner_is_cave_ai_spawner || spawner_map_entries.is_empty() {
        return false;
    }
    recorded_spawn_data.clear();
    for entry in spawner_map_entries {
        if entry.has_valid_uclass && !entry.times.is_empty() {
            let resolved_name =
                resolve_cave_ai_class_name(entry.uclass_type_name, entry.class_generated_by_name);
            recorded_spawn_data.push(CaveSpawnTimes {
                class_name: resolved_name.to_vec(),
                times: entry.times.to_vec(),
            });
        }
    }
    true
}

/// Optional fields updated by `UDeployableRecorderComponent::RecordOwnerState`
/// (`C009`, `0x1418a15a0` server / `0x1418a6d20` client) after
/// `UIcarusActorRecorderComponent::RecordOwnerState` (`0x1418a8cc0` client)
/// (F-646, F-650).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeployableRecordPlan<C, T> {
    /// Updated `FoundationActorRecord` (`+0x230..0x248` server / `+0x258..0x270`
    /// client: `(class_name, icarus_actor_guid)`) when `owner->FoundationActor`
    /// (`owner + 0x580`) is non-null and has a `UActorStateRecorderComponent`
    /// (`0x141b9cbb0` client); `None` leaves the existing record untouched.
    pub foundation_actor_record: Option<(C, i32)>,
    /// Updated `(WhitelistedTames, bAllowAllTames)` (`+0x220`/`+0x250` server,
    /// `+0x278`/`+0x288` client) when the owner has a `UTameWhitelistComponent`
    /// (`0x141da0d70` client); `None` leaves the existing fields untouched.
    pub tame_whitelist: Option<(Vec<T>, bool)>,
}

/// Evaluates `UDeployableRecorderComponent::RecordOwnerState` (`C009`,
/// `0x1418a15a0` server / `0x1418a6d20` client) (F-646, F-650).
pub fn evaluate_deployable_record<C, T: Clone>(
    foundation_actor_with_recorder: Option<(C, i32)>,
    tame_whitelist_component: Option<(&[T], bool)>,
) -> DeployableRecordPlan<C, T> {
    DeployableRecordPlan {
        foundation_actor_record: foundation_actor_with_recorder,
        tame_whitelist: tame_whitelist_component
            .map(|(tames, allow_all)| (tames.to_vec(), allow_all)),
    }
}

/// Alias for [`evaluate_crop_plot_state_transfer`] (`C008`, `0x1418a1540`
/// server / `0x1418a6cc0` client and `0x1418b7bb0` server / `0x1418baa80`
/// client; see [`evaluate_bed_spawn_point_players_transfer`] for `C003`
/// `UBedRecorderComponent`) (F-650, F-657, F-744).
pub const fn evaluate_building_base_decay_transfer<D: Copy>(
    owner_implements_decayable_interface: bool,
    decay_state: D,
) -> Option<D> {
    evaluate_crop_plot_state_transfer(owner_implements_decayable_interface, decay_state)
}

/// Values transferred by `UOrbitalExchangeInterfaceRecorderComponent` (`C022`,
/// `RecordOwnerState` at `0x1418e83a0` server / `0x1418e86d0` client;
/// `UpdateOwnerToDatabaseState` at `0x1418f6f90` server / `0x1418f7660` client)
/// (F-650, F-657).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OrbitalExchangeTransferPlan<E, D> {
    /// `ExchangeInventories` (`recorder + 0xd8..0x128`) recorded via
    /// `AOrbitalExchangeInterface::RecordExchangeInventories` (`0x141c7d8d0`
    /// client) / passed to `RestoreExchangeInventories` (`0x141c7d760` client).
    pub exchange_inventories: E,
    /// `DeliveredItems` (`recorder + 0x128`, count `+0x130`, stride `0x28` B)
    /// copied from `AOrbitalExchangeInterface + 0x258` (`600`, count `+0x260`)
    /// / passed to `RestoreExchangeInventories` (`0x141c7d760` client).
    pub delivered_items: Vec<D>,
}

/// Evaluates `UOrbitalExchangeInterfaceRecorderComponent::RecordOwnerState`
/// (`C022`, `0x1418e83a0` server / `0x1418e86d0` client) and
/// `UpdateOwnerToDatabaseState` (`0x1418f6f90` server / `0x1418f7660` client)
/// (F-650, F-657).
///
/// Returns `None` when the owner is null or not an `AOrbitalExchangeInterface`
/// (`0x141c7b940` client).
pub fn evaluate_orbital_exchange_transfer<E, D: Clone>(
    owner_is_orbital_exchange_interface: bool,
    exchange_inventories: E,
    delivered_items: &[D],
) -> Option<OrbitalExchangeTransferPlan<E, D>> {
    if !owner_is_orbital_exchange_interface {
        return None;
    }
    Some(OrbitalExchangeTransferPlan {
        exchange_inventories,
        delivered_items: delivered_items.to_vec(),
    })
}

/// Evaluates `UPlayerGroupSubsystemRecorderComponent::RecordOwnerState`
/// (`C024`, `0x1418e85b0` server / `0x1418e88e0` client) after
/// `UActorStateRecorderComponent::RecordOwnerState` (`0x1418a5b60` client)
/// (F-650, F-657).
///
/// Unconditionally clears `recorded_player_group_data` (`recorder + 0x1c0`,
/// count `+0x1c8`, stride `0x28` B) on entry before checking the owner, then
/// copies `owner_player_group_data` (`APlayerGroupSubsystem + 0x2c0`, count
/// `+0x2c8`) iff the owner is a non-null `APlayerGroupSubsystem` (`0x141cc4380`
/// client; no `PendingKill` check is performed). Returns `true` when copied, or
/// `false` when `recorded_player_group_data` was only cleared.
pub fn evaluate_player_group_subsystem_record<G: Clone>(
    owner_is_player_group_subsystem: bool,
    owner_player_group_data: &[G],
    recorded_player_group_data: &mut Vec<G>,
) -> bool {
    recorded_player_group_data.clear();
    if !owner_is_player_group_subsystem {
        return false;
    }
    recorded_player_group_data.extend_from_slice(owner_player_group_data);
    true
}

/// Evaluates `UPlayerGroupSubsystemRecorderComponent::UpdateOwnerToDatabaseState`
/// (`C024`, `0x1418f7070` server / `0x1418f7740` client) after
/// `UActorStateRecorderComponent::UpdateOwnerToDatabaseState` (`0x1418ba040`
/// client) (F-650, F-657).
///
/// Clears `APlayerGroupSubsystem + 0x2c0` and copies `saved_player_group_data`
/// (`recorder + 0x1c0`, stride `0x28` B) iff the owner is a non-null
/// `APlayerGroupSubsystem` (`0x141cc4380` client).
pub fn evaluate_player_group_subsystem_restore<G: Clone>(
    owner_is_player_group_subsystem: bool,
    saved_player_group_data: &[G],
) -> Option<Vec<G>> {
    owner_is_player_group_subsystem.then(|| saved_player_group_data.to_vec())
}

/// Outcome of `URocketSpawnRecorderComponent::RecordOwnerState` (`C011`,
/// `0x1418e8790` server / `0x1418e97b0` client) after
/// `UActorStateRecorderComponent::RecordOwnerState` (`0x1418a5b60` client)
/// (F-648, F-650, F-652).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RocketSpawnRecordPlan<U> {
    /// `Version` (`recorder + 0x1e0`) is set to `1` unconditionally on entry
    /// before checking `owner`.
    pub version: i32,
    /// `AssignedPlayerID` (`recorder + 0x1c8`, `ChrSlot` at `+0x1d8`) copied
    /// from `ADropshipSpawnPoint + 0x2e8..0x2fc` (`ChrSlot` at `+0x2f8`) iff
    /// the owner is a non-null `ADropshipSpawnPoint` (`0x141d2df30` client; no
    /// `PendingKill` check is performed); `None` preserves existing
    /// `recorder + 0x1c8..0x1dc`.
    pub assigned_player_id: Option<(U, i32)>,
}

/// Evaluates `URocketSpawnRecorderComponent::RecordOwnerState` (`C011`,
/// `0x1418e8790` server / `0x1418e97b0` client) (F-648, F-650, F-652).
pub fn evaluate_rocket_spawn_record<U>(
    owner_is_dropship_spawn_point: bool,
    owner_assigned_player_id: (U, i32),
) -> RocketSpawnRecordPlan<U> {
    RocketSpawnRecordPlan {
        version: 1,
        assigned_player_id: owner_is_dropship_spawn_point.then_some(owner_assigned_player_id),
    }
}

/// Owner-derived fields updated by `UPlayerRecorderComponent::RecordOwnerState`
/// (`C025`, `0x1418e8850` server / `0x1418e8b80` client) when the owner is a
/// non-pending-kill `AIcarusPlayerController` (`0x141bfb490` client) (F-648,
/// F-650, F-652).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlayerRecorderOwnerState<U> {
    /// `PlayerID` (`recorder + 0xd8`, `ChrSlot` at `+0xe8`): set from
    /// `AIcarusPlayerController::GetPlayerCharacterID` (`0x1414a69a0` client)
    /// when `controller->PlayerState` (`+0x228`) is non-null and not pending
    /// kill, or `None` (resetting `UserID` to `""` and `ChrSlot` to `-1` via
    /// `0x14171aeb0` client) when `PlayerState` is missing or pending kill.
    pub player_id: Option<(U, i32)>,
    /// `AssignedDropshipSpawnUID` (`recorder + 0xf0`): overwritten with
    /// `AssignedDropshipSpawn->GetIcarusActorGuid()` when `controller + 0x970`
    /// is non-null and not pending kill, or preserved (`previous_dropship_spawn_uid`)
    /// when null or pending kill.
    pub assigned_dropship_spawn_uid: i32,
    /// `AssignedDropshipUID` (`recorder + 0xf4`): overwritten with
    /// `AssignedDropship->GetIcarusActorGuid()` when `controller + 0x978` is
    /// non-null and not pending kill, or preserved (`previous_dropship_uid`)
    /// when null or pending kill.
    pub assigned_dropship_uid: i32,
    /// `AssignedGravestoneUID` (`recorder + 0xf8`): overwritten with
    /// `AssignedGravestone->GetIcarusActorGuid()` when `controller + 0x980` is
    /// non-null and not pending kill, or cleared to `0` when null or pending
    /// kill.
    pub assigned_gravestone_uid: i32,
}

/// Outcome of `UPlayerRecorderComponent::RecordOwnerState` (`C025`,
/// `0x1418e8850` server / `0x1418e8b80` client) (F-648, F-650, F-652).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlayerRecorderRecordPlan<U> {
    /// `SaveDataVersion` (`recorder + 0xfc`) is set to `1` unconditionally on
    /// entry before checking `owner`.
    pub save_data_version: i32,
    /// Owner fields updated iff `owner` is a non-null, non-pending-kill
    /// `AIcarusPlayerController` (`0x141bfb490` client).
    pub owner_state: Option<PlayerRecorderOwnerState<U>>,
}

/// Evaluates `UPlayerRecorderComponent::RecordOwnerState` (`C025`,
/// `0x1418e8850` server / `0x1418e8b80` client) (F-648, F-650, F-652).
pub fn evaluate_player_recorder_record<U>(
    owner_is_valid_player_controller: bool,
    valid_player_state_character_id: Option<(U, i32)>,
    previous_dropship_spawn_uid: i32,
    valid_dropship_spawn_guid: Option<i32>,
    previous_dropship_uid: i32,
    valid_dropship_guid: Option<i32>,
    valid_gravestone_guid: Option<i32>,
) -> PlayerRecorderRecordPlan<U> {
    let owner_state = owner_is_valid_player_controller.then(|| PlayerRecorderOwnerState {
        player_id: valid_player_state_character_id,
        assigned_dropship_spawn_uid: valid_dropship_spawn_guid
            .unwrap_or(previous_dropship_spawn_uid),
        assigned_dropship_uid: valid_dropship_guid.unwrap_or(previous_dropship_uid),
        assigned_gravestone_uid: valid_gravestone_guid.unwrap_or(0),
    });
    PlayerRecorderRecordPlan {
        save_data_version: 1,
        owner_state,
    }
}

/// Values recorded by `UPlayerStateRecorderComponent::RecordOwnerState` (`C026`,
/// `0x1418e8b10` server / `0x1418e8e50` client) after
/// `UActorStateRecorderComponent::RecordOwnerState` (`0x1418a5b60` client)
/// (F-648, F-650).
#[derive(Clone, Debug, PartialEq)]
pub struct PlayerStateRecordPlan<U, S, C> {
    /// `Version` (`recorder + 0x220` = `544`) set to `1`.
    pub version: i32,
    /// `PlayerID` (`recorder + 0x1c0`, `ChrSlot` at `+0x1d0`): copied from
    /// `AIcarusPlayerState + 0x478..0x48c` server / `+0x498..0x4ac` client when
    /// `AIcarusPlayerCharacter::GetIcarusPlayerState` (`0x1414660d0` client)
    /// returns a non-null, non-pending-kill `AIcarusPlayerState`, or `None`
    /// (resetting `UserID` to `""` and `ChrSlot` to `-1` via `0x14171aeb0`
    /// client).
    pub player_id: Option<(U, i32)>,
    /// `Location` (`recorder + 0x1d8..0x1e4`).
    pub location: [f32; 3],
    /// `Rotation` (`recorder + 0x1e4..0x1f0`).
    pub rotation: [f32; 3],
    /// `bIsAlive` (`recorder + 0x1f0`, from `StatComponent->IsAlive()` at
    /// `vtable + 0x428`).
    pub b_is_alive: bool,
    /// `Health` (`recorder + 0x1f4` = `500`, from `StatComponent + 0x1d8`).
    pub health: i32,
    /// `LastSeatIndex` (`recorder + 0x218` = `536`, from
    /// `AIcarusPlayerCharacter + 0xa50`).
    pub last_seat_index: i32,
    /// `bIsLastInputGamepad` (`recorder + 0x21c` = `540`, from
    /// `AIcarusPlayerCharacter + 0xa54`).
    pub b_is_last_input_gamepad: bool,
    /// `(Water, Food, Oxygen, Temperature)` (`recorder + 0x1f8..0x208`) written
    /// iff `StatComponent` is a `UIcarusStatComponent` (`0x141cbcbb0` client);
    /// `None` preserves existing `+0x1f8..0x208`.
    pub survival_stats: Option<(i32, i32, i32, i32)>,
    /// `StomachContents` (`recorder + 0x208`, count `+0x210`, stride `0x10` B)
    /// copied from `UPlayerStomachComponent + 0xb0` (stride `0x24` B) iff the
    /// owner has a `UPlayerStomachComponent` (`0x141d82ad0` client); `None`
    /// preserves existing `+0x208`.
    pub stomach_contents: Option<Vec<S>>,
    /// `CosmeticArmourOverrides` (`recorder + 0x228`, count `+0x230`, stride
    /// `0x18` B) unconditionally replaced from `AIcarusPlayerCharacter + 0xa80`.
    pub cosmetic_armour_overrides: Vec<C>,
    /// Override `ActorTransform` (`recorder + 0xe0..0x10c`) written when the
    /// owner is a `ABP_IcarusPlayerCharacterSurvival_C` (`0x141bf37b0` client)
    /// and `ShouldOverrideRecordedTransform` (`0x1419b6410` client) returns
    /// `true`.
    pub override_actor_transform: Option<([f32; 4], [f32; 3], [f32; 3])>,
}

/// Evaluates `UPlayerStateRecorderComponent::RecordOwnerState` (`C026`,
/// `0x1418e8b10` server / `0x1418e8e50` client) (F-648, F-650).
///
/// Returns `None` when the owner is null, not an `AIcarusPlayerCharacter`
/// (`0x141bec9a0` client), or pending kill. When `survival_override_transform`
/// is `Some((quat, translation, scale, rotator))`, writes `(quat, translation,
/// scale)` to `override_actor_transform` (`recorder + 0xe0..0x10c`) and, iff
/// [`recorder_transform_is_non_identity`] holds for `(quat, translation, scale)`,
/// also overwrites `location` (`+0x1d8`) and `rotation` (`+0x1e4`) with
/// `(translation, rotator)`.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub fn evaluate_player_state_record<U, S: Clone, C: Clone>(
    owner_is_valid_player_character: bool,
    valid_player_state_character_id: Option<(U, i32)>,
    root_component_location_rotation: Option<([f32; 3], [f32; 3])>,
    b_is_alive: bool,
    health: i32,
    last_seat_index: i32,
    b_is_last_input_gamepad: bool,
    icarus_stat_component_values: Option<(i32, i32, i32, i32)>,
    stomach_component_contents: Option<&[S]>,
    cosmetic_armour_overrides: &[C],
    survival_override_transform: Option<([f32; 4], [f32; 3], [f32; 3], [f32; 3])>,
) -> Option<PlayerStateRecordPlan<U, S, C>> {
    if !owner_is_valid_player_character {
        return None;
    }
    let (mut location, mut rotation) =
        root_component_location_rotation.unwrap_or(([0.0, 0.0, 0.0], [0.0, 0.0, 0.0]));
    let override_actor_transform =
        survival_override_transform.map(|(quat, translation, scale, rotator)| {
            if recorder_transform_is_non_identity(quat, translation, scale) {
                location = translation;
                rotation = rotator;
            }
            (quat, translation, scale)
        });
    Some(PlayerStateRecordPlan {
        version: 1,
        player_id: valid_player_state_character_id,
        location,
        rotation,
        b_is_alive,
        health,
        last_seat_index,
        b_is_last_input_gamepad,
        survival_stats: icarus_stat_component_values,
        stomach_contents: stomach_component_contents.map(<[S]>::to_vec),
        cosmetic_armour_overrides: cosmetic_armour_overrides.to_vec(),
        override_actor_transform,
    })
}

/// Values recorded by `URocketRecorderComponent::RecordOwnerState` (`C029`,
/// `0x1418e9280` server / `0x1418e95c0` client) after
/// `UActorStateRecorderComponent::RecordOwnerState` (`0x1418a5b60` client)
/// (F-648, F-650, F-652, F-663).
#[derive(Clone, Debug, PartialEq)]
pub struct RocketRecordPlan<U> {
    /// `Version` (`recorder + 0x1f4` = `500`) set to `1`.
    pub version: i32,
    /// `RocketState` (`ARocket + 0x2e0 -> recorder + 0x1f0`).
    pub rocket_state: u8,
    /// `SpawnPointLocation` (`ARocket + 0x2c4 -> recorder + 0x1d8`).
    pub spawn_point_location: [f32; 3],
    /// `DescentPointLocation` (`ARocket + 0x2d0 -> recorder + 0x1e4`).
    pub descent_point_location: [f32; 3],
    /// `bHasLanded` (`ARocket + 0x2e1 -> recorder + 0x1f1`).
    pub b_has_landed: bool,
    /// `bHasDeparted` (`ARocket + 0x2e2 -> recorder + 0x1f2`).
    pub b_has_departed: bool,
    /// `bPodVisible` (`ARocket + 0x300 -> recorder + 0x1f3` = `499`).
    pub b_pod_visible: bool,
    /// Updated `PlayerID` (`recorder + 0x1c0`, `ChrSlot` at `+0x1d0`) written
    /// iff both `ARocket::GetAssignedPlayerCharacter` (`0x1417afcc0` client)
    /// and `AIcarusPlayerCharacter::GetIcarusPlayerState` (`0x1414a51d0` client)
    /// return non-null, non-pending-kill objects; `None` preserves the existing
    /// `recorder + 0x1c0..0x1d4` fields unchanged.
    pub updated_player_id: Option<(U, i32)>,
}

/// Evaluates `URocketRecorderComponent::RecordOwnerState` (`C029`,
/// `0x1418e9280` server / `0x1418e95c0` client) (F-648, F-650, F-652, F-663).
///
/// Returns `None` when the owner is null, not an `ARocket` (`0x141c08690`
/// client), or pending kill.
#[allow(clippy::too_many_arguments)]
pub fn evaluate_rocket_record<U>(
    owner_is_valid_rocket: bool,
    rocket_state: u8,
    spawn_point_location: [f32; 3],
    descent_point_location: [f32; 3],
    b_has_landed: bool,
    b_has_departed: bool,
    b_pod_visible: bool,
    valid_assigned_character_player_id: Option<(U, i32)>,
) -> Option<RocketRecordPlan<U>> {
    if !owner_is_valid_rocket {
        return None;
    }
    Some(RocketRecordPlan {
        version: 1,
        rocket_state,
        spawn_point_location,
        descent_point_location,
        b_has_landed,
        b_has_departed,
        b_pod_visible,
        updated_player_id: valid_assigned_character_player_id,
    })
}

/// Values recorded by `UTreeRecorderComponent::RecordOwnerState` (`C033`,
/// `0x1418e9820` server / `0x1418e9b60` client) after
/// `UActorStateRecorderComponent::RecordOwnerState` (`0x1418a5b60` client)
/// (F-656, F-658, F-736).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TreeRecordPlan<N> {
    /// `TreeType` (`UTreePrimitive + 0x18 -> recorder + 0x1c0`).
    pub tree_type: N,
    /// `TreePrimitiveMask` (`recorder + 0x1c8`, count `+0x1d0`): deduplicated
    /// `TSet<int32>` union of `ATreeBase + 0x330` (count `+0x338`) followed by
    /// `ATreeBase + 0x308` (count `+0x310`) in first-seen insertion order.
    pub tree_primitive_mask: Vec<i32>,
    /// `RandomStream` (`ATreeBase + 0x300 -> recorder + 0x1d8`, `8` B).
    pub random_stream: u64,
    /// `bIsPhysicsDynamic` (`ATreeBase + 0x318 -> recorder + 0x1e0`).
    pub b_is_physics_dynamic: bool,
}

/// Evaluates `UTreeRecorderComponent::RecordOwnerState` (`C033`,
/// `0x1418e9820` server / `0x1418e9b60` client) (F-656, F-658, F-736).
///
/// Returns `None` when the owner is null, not an `ATreeBase` (`0x141dbef10`
/// client), or pending kill.
pub fn evaluate_tree_record<N>(
    owner_is_valid_tree: bool,
    tree_type: N,
    destroyed_nodes_330: &[i32],
    cut_nodes_308: &[i32],
    random_stream: u64,
    b_is_physics_dynamic: bool,
) -> Option<TreeRecordPlan<N>> {
    if !owner_is_valid_tree {
        return None;
    }
    let mut tree_primitive_mask = Vec::new();
    for &node in destroyed_nodes_330.iter().chain(cut_nodes_308.iter()) {
        if !tree_primitive_mask.contains(&node) {
            tree_primitive_mask.push(node);
        }
    }
    Some(TreeRecordPlan {
        tree_type,
        tree_primitive_mask,
        random_stream,
        b_is_physics_dynamic,
    })
}

/// Saved `UProcessingComponent` state restored by
/// `UActorStateRecorderComponent::UpdateOwnerToDatabaseState` (`C001`,
/// `0x1418b6ed0` server / `0x1418ba040` client) when the owner has a
/// `UProcessingComponent` (`0x141d1ddd0` client) (F-735).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RestoredProcessingState {
    /// Saved byte at `+0x164` passed to `0x1418de170`.
    pub b_processing_active: bool,
    /// Saved byte at `+0x165` passed to `0x1418de170`.
    pub b_is_crafting: bool,
    /// Saved `i32` at `+0x168` written to `UProcessingComponent + 0x208`.
    pub crafting_progress: i32,
    /// Saved byte at `+0x160` passed to `0x1418f03b0(component, Row(1), flag)`.
    pub b_resource_1_active: bool,
    /// Saved byte at `+0x161` passed to `0x1418f03b0(component, Row(2), flag)`.
    pub b_resource_2_active: bool,
}

/// Optional subcomponent and `AIcarusActor` post-restore actions evaluated by
/// `UActorStateRecorderComponent::UpdateOwnerToDatabaseState` (`C001`,
/// `0x1418b6ed0` server / `0x1418ba040` client) (F-735).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ActorSubcomponentRestorePlan {
    /// Whether `FRecordedDurabilityData` (`+0x120..0x139`) is copied into the
    /// owner's `UDurabilityComponent` (`0x141b43370` client, `+0xb0..0xd0`).
    pub restore_durability_component: bool,
    /// Generator active flag (`+0x162`) applied via `0x14196e810` when the
    /// owner has a `UGeneratorComponent` (`0x141b6eec0` client).
    pub generator_active: Option<bool>,
    /// Processing state (`+0x160..0x168`) applied when the owner has a
    /// `UProcessingComponent` (`0x141d1ddd0` client).
    pub processing_state: Option<RestoredProcessingState>,
    /// Whether `AIcarusActor::UpdateStateFromDatabase` (`0x141b9e310` client)
    /// runs after restoring variables (`owner` is a non-pending-kill
    /// `AIcarusActor` `0x141b9e1b0` client).
    pub notify_icarus_actor_database_state_updated: bool,
}

/// Evaluates the subcomponent (`UDurabilityComponent`, `UGeneratorComponent`,
/// `UProcessingComponent`) and `AIcarusActor` post-restore steps in
/// `UActorStateRecorderComponent::UpdateOwnerToDatabaseState` (`C001`,
/// `0x1418b6ed0` server / `0x1418ba040` client) (F-735).
#[allow(clippy::too_many_arguments)]
pub const fn evaluate_actor_subcomponent_restore(
    has_durability_component: bool,
    has_generator_component: bool,
    b_generator_active: bool,
    has_processing_component: bool,
    b_processing_active: bool,
    b_is_crafting: bool,
    crafting_progress: i32,
    b_resource_1_active: bool,
    b_resource_2_active: bool,
    owner_is_valid_icarus_actor: bool,
) -> ActorSubcomponentRestorePlan {
    let generator_active = if has_generator_component {
        Some(b_generator_active)
    } else {
        None
    };
    let processing_state = if has_processing_component {
        Some(RestoredProcessingState {
            b_processing_active,
            b_is_crafting,
            crafting_progress,
            b_resource_1_active,
            b_resource_2_active,
        })
    } else {
        None
    };
    ActorSubcomponentRestorePlan {
        restore_durability_component: has_durability_component,
        generator_active,
        processing_state,
        notify_icarus_actor_database_state_updated: owner_is_valid_icarus_actor,
    }
}

/// How `UActorStateRecorderComponent::UpdateOwnerToDatabaseState` (`C001`,
/// `0x1418b6ed0` server / `0x1418ba040` client) rebinds an owner's
/// `IcarusActorGUID` in `UIcarusWorldRecorderSubsystem` (F-735).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ActorGuidRestoreStep {
    /// Positive `current_actor_guid` unbound via
    /// `UIcarusWorldRecorderSubsystem::UnbindActorGuid` (`0x1413f2c10` client)
    /// before `BindActorGuid`, or `None` when `current_actor_guid <= 0`.
    pub unbind_previous_guid: Option<i32>,
    /// GUID written via `IIcarusActorGuidInterface::SetIcarusActorGuid`
    /// (`vtable + 0x18`) after `UIcarusWorldRecorderSubsystem::BindActorGuid`
    /// (`0x1413dec50` server / `0x14141c790` client).
    pub new_guid: i32,
}

/// Evaluates `IcarusActorGUID` rebinding in
/// `UActorStateRecorderComponent::UpdateOwnerToDatabaseState` (`C001`,
/// `0x1418b6ed0` server / `0x1418ba040` client) (F-735).
///
/// Runs iff `!b_skip_actor_guid_binding` (`[this + 0x144] == 0`),
/// `owner_implements_guid_interface` (`IIcarusActorGuidInterface`), and
/// `saved_icarus_actor_guid != current_actor_guid`.
pub const fn evaluate_actor_guid_restore(
    b_skip_actor_guid_binding: bool,
    owner_implements_guid_interface: bool,
    saved_icarus_actor_guid: i32,
    current_actor_guid: i32,
    bound_guid: i32,
) -> Option<ActorGuidRestoreStep> {
    if b_skip_actor_guid_binding
        || !owner_implements_guid_interface
        || saved_icarus_actor_guid == current_actor_guid
    {
        return None;
    }
    let unbind_previous_guid = if current_actor_guid > 0 {
        Some(current_actor_guid)
    } else {
        None
    };
    Some(ActorGuidRestoreStep {
        unbind_previous_guid,
        new_guid: bound_guid,
    })
}

/// Values recorded by `USplineRecorderComponent::RecordOwnerState` (`C032`,
/// `0x1418e95f0` server / `0x1418e9930` client) after
/// `UActorStateRecorderComponent::RecordOwnerState` (`0x1418a5b60` client)
/// (F-650, F-658).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SplineRecordPlan<S> {
    /// `SplineNetworkSaveData` (`recorder + 0x1c0..0x248`) written by
    /// `ASplineNetworkActor::RecordSplineNetwork` (`0x141d6e650` client).
    pub spline_network_state: S,
    /// `UniqueSplineID` (`recorder + 0x234`) overwritten with
    /// `ASplineNetworkActor + 0x220` after `RecordSplineNetwork`.
    pub unique_spline_id: i32,
}

/// Evaluates `USplineRecorderComponent::RecordOwnerState` (`C032`,
/// `0x1418e95f0` server / `0x1418e9930` client) (F-650, F-658).
///
/// Returns `None` when the owner is null or not an `ASplineNetworkActor`
/// (`0x141d6e150` client; no `PendingKill` check is performed).
pub const fn evaluate_spline_record<S: Copy>(
    owner_is_spline_network_actor: bool,
    spline_network_state: S,
    owner_unique_spline_id: i32,
) -> Option<SplineRecordPlan<S>> {
    if !owner_is_spline_network_actor {
        return None;
    }
    Some(SplineRecordPlan {
        spline_network_state,
        unique_spline_id: owner_unique_spline_id,
    })
}

/// How `USplineRecorderComponent::RespawnOwner` (`C032`, `0x1418ed690` server /
/// `0x1418ee060` client) resolves or spawns its spline owner and applies saved
/// spline state (F-658).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SplineRespawnDisposition {
    /// Neither `USplineSubsystem` lookup (`0x141bc1a90` client) nor
    /// `UActorStateRecorderComponent::RespawnOwner` (`0x1418a7210` server /
    /// `0x1418abc90` client) returned an actor.
    FailedNoActor,
    /// Actor resolved (reusing an existing `USplineSubsystem` actor when
    /// `reused_existing_spline_actor` is `true`, else spawning via
    /// `UActorStateRecorderComponent::RespawnOwner`); when
    /// `apply_spline_network_state` is `true` (`ASplineNetworkActor`), copies
    /// `UniqueSplineID` (`+0x234 -> +0x220`) and `SplineFlags` (`+0x248 -> +0x224`)
    /// and calls `RestoreSplineNetwork` (`+0x1c0`).
    Resolved {
        reused_existing_spline_actor: bool,
        apply_spline_network_state: bool,
    },
}

/// Evaluates `USplineRecorderComponent::RespawnOwner` (`C032`, `0x1418ed690`
/// server / `0x1418ee060` client) (F-658).
pub const fn evaluate_spline_recorder_respawn(
    existing_spline_found: bool,
    fallback_spawn_ok: bool,
    actor_is_spline_network: bool,
) -> SplineRespawnDisposition {
    if !existing_spline_found && !fallback_spawn_ok {
        return SplineRespawnDisposition::FailedNoActor;
    }
    SplineRespawnDisposition::Resolved {
        reused_existing_spline_actor: existing_spline_found,
        apply_spline_network_state: actor_is_spline_network,
    }
}

/// Transforms each `FRecordedSplinePoint::Position` (`+0x00..0x0c`, stride
/// `0x50` B in `SplineNetworkSaveData.Points` at `recorder + 0x238`, count
/// `+0x240`) in place by the context `FTransform` (`v' = q * (scale * v) +
/// translation`) as `USplineRecorderComponent::ApplyRecorderTransform` (`C032`,
/// `vftable + 0x4b0`: `0x1418c5cd0` server / `0x1418c7970` client) does after
/// `UActorStateRecorderComponent::ApplyRecorderTransform` (`0x141887d20`
/// server / `0x141890ec0` client) (F-716, F-727).
pub fn transform_spline_point_positions(
    rotation: [f32; 4],
    translation: [f32; 3],
    scale: [f32; 3],
    positions: &mut [[f32; 3]],
) {
    let [qx, qy, qz, qw] = rotation;
    for pos in positions {
        let sx = scale[0] * pos[0];
        let sy = scale[1] * pos[1];
        let sz = scale[2] * pos[2];
        let tx = 2.0 * (qy * sz - qz * sy);
        let ty = 2.0 * (qz * sx - qx * sz);
        let tz = 2.0 * (qx * sy - qy * sx);
        let rx = sx + qw * tx + (qy * tz - qz * ty);
        let ry = sy + qw * ty + (qz * tx - qx * tz);
        let rz = sz + qw * tz + (qx * ty - qy * tx);
        *pos = [
            rx + translation[0],
            ry + translation[1],
            rz + translation[2],
        ];
    }
}

/// Whether a respawned `C033` (`UTreeRecorderComponent::RespawnOwner` at
/// `0x1418ed770` server / `0x1418ee140` client -> `ATreePrefab::CreateTreeRuntime`
/// `0x141717f90` server / `0x141727e20` client) unregisters its recorder via
/// `BP_TreeBase_C`'s double `EndRecording` (`0x141892460` server / `0x14189a250`
/// client) during `FinishSpawningActor` and is therefore omitted on the next
/// save: occurs iff `b_is_physics_dynamic` (`+0x1e0`) is `true` (F-656, F-681,
/// F-736).
pub const fn should_drop_respawned_tree_recorder_on_resave(b_is_physics_dynamic: bool) -> bool {
    b_is_physics_dynamic
}

/// Values recorded by `UResourceDepositRecorderComponent::RecordOwnerState`
/// (`C028`, `0x1418e90b0` server / `0x1418e93e0` client) after
/// `UActorStateRecorderComponent::RecordOwnerState` (`0x1418a5b60` client)
/// (F-645, F-650).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResourceDepositRecordPlan<N> {
    /// `ResourceQuantity` (`AResourceDeposit + 0x2c0 -> recorder + 0x1c0`).
    pub resource_quantity: i32,
    /// `DepositType.RowName` (`AResourceDeposit + 0x2c4 -> recorder + 0x1c4`).
    pub deposit_type_row_name: N,
    /// `bShowMapIcon` (`recorder + 0x1cc`), set to
    /// `UStatComponent::GetStat(StatComponent, 0xbe) != 0` (`0x1417a7a80`
    /// server / `0x1417b4140` client).
    pub b_show_map_icon: bool,
}

/// Evaluates `UResourceDepositRecorderComponent::RecordOwnerState` (`C028`,
/// `0x1418e90b0` server / `0x1418e93e0` client) (F-645, F-650).
///
/// Returns `None` when the owner is null, not an `AResourceDeposit`
/// (`0x141d22960` client), or pending kill.
pub const fn evaluate_resource_deposit_record<N: Copy>(
    owner_is_valid_resource_deposit: bool,
    resource_quantity: i32,
    deposit_type_row_name: N,
    current_map_icon_stat: i32,
) -> Option<ResourceDepositRecordPlan<N>> {
    if !owner_is_valid_resource_deposit {
        return None;
    }
    Some(ResourceDepositRecordPlan {
        resource_quantity,
        deposit_type_row_name,
        b_show_map_icon: current_map_icon_stat != 0,
    })
}

/// Whether `UResourceDepositRecorderComponent::UpdateOwnerToDatabaseState`
/// (`C028`, `0x1418f76c0` server / `0x1418f7d90` client) grants `+1` to stat
/// `0xbe` (`190`) via `UStatComponent::AddStat` (`0x14178be60` server /
/// `0x141798d10` client) after restoring `ResourceNode` (`+0x1c0 -> +0x2d0`)
/// and `bShowMapIcon` (`+0x1cc -> +0x2dc`): requires `b_show_map_icon` to be
/// `true` and `UStatComponent::GetStat(StatComponent, 0xbe)` (`0x1417a7a80`
/// server / `0x1417b4140` client) to equal `0` (F-645).
pub const fn should_grant_deposit_map_icon_stat(
    b_show_map_icon: bool,
    current_map_icon_stat: i32,
) -> bool {
    b_show_map_icon && current_map_icon_stat == 0
}

/// The engine's float-to-int truncation: out-of-range and NaN inputs give
/// `i32::MIN`, as the x86 truncating conversion does.
fn truncate(value: f32) -> i32 {
    if value.is_nan() || !(-2_147_483_648.0..2_147_483_648.0).contains(&value) {
        i32::MIN
    } else {
        value as i32
    }
}

/// The saved RemainingCooldown of a teleport whose cooldown ends at `end` on
/// the clock that reads `now` (F-657, F-734): the truncated time left, or 0
/// when no end is set or less than one unit remains.
pub fn remaining_cooldown(end: f32, now: f32) -> i32 {
    if end > 0.0 {
        let left = truncate(end - now);
        if left >= 1 {
            return left;
        }
    }
    0
}

/// Computes `AIcarusGameStateSurvival::GetReplicatedProspectGameTime`
/// (`0x1418d9390`) from `UGameplayStatics::GetTimeSeconds(World)` (`0x14385dad0`)
/// and the replicated prospect time offset at `GameState + 0x398` (F-734).
pub fn replicated_prospect_game_time(
    world_time_seconds: f32,
    replicated_prospect_time_offset: f32,
) -> f32 {
    world_time_seconds + replicated_prospect_time_offset
}

/// Whether `UWeatherControllerRecorderComponent::RecordOwnerState` (`0x1418ea8d0`)
/// records an active weather event given its data-table `row_duration`,
/// `event_start_time`, and `AWeatherForecastManager::CurrentBlockEndTime()`
/// (`0x14190a5a0`) (F-665, F-734).
pub const fn should_record_weather_event(
    row_duration: i32,
    event_start_time: i32,
    current_block_end_time: i32,
) -> bool {
    row_duration >= 0 && row_duration.wrapping_add(event_start_time) <= current_block_end_time
}

/// Computes the saved `TimeElapsed` field of a recorded weather event in
/// `UWeatherControllerRecorderComponent::RecordOwnerState` (`0x1418ea8d0`) from
/// `UIcarusFunctionLibrary::GetLevelTimeElapsedSec(GameMode)` (`0x1414e1390`)
/// and `event_start_time` (F-665, F-734).
pub const fn record_weather_event_elapsed(
    level_time_elapsed_sec: i32,
    event_start_time: i32,
) -> i32 {
    level_time_elapsed_sec.wrapping_sub(event_start_time)
}

/// Reconstructs an active weather event's `StartTime` in
/// `UWeatherControllerRecorderComponent::UpdateOwnerToDatabaseState`
/// (`0x1418f87f0`) from `UIcarusFunctionLibrary::GetLevelTimeElapsedSec(GameMode)`
/// (`0x1414e1390`) and saved `time_elapsed` (F-665, F-734).
pub const fn restored_weather_event_start_time(
    level_time_elapsed_sec: i32,
    time_elapsed: i32,
) -> i32 {
    if level_time_elapsed_sec > 0 {
        level_time_elapsed_sec.wrapping_sub(time_elapsed)
    } else {
        0
    }
}

/// One `FRecordedWeatherEvent` (`0x14` B = `20` B) stored in
/// `UWeatherControllerRecorderComponent::WeatherEvents` (`recorder + 0xd8`,
/// count `+0xe0`) by `C035` (`RecordOwnerState` at `0x1418e98b0` server /
/// `0x1418ea8d0` client) (F-654, F-665, F-734).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RecordedWeatherEvent<E, P> {
    /// `WeatherEventRow` (`+0x00`, `8` B).
    pub weather_event_row: E,
    /// `PoolRow` (`+0x08`, `8` B).
    pub pool_row: P,
    /// `TimeElapsed` (`+0x10`, `i32`), computed via
    /// [`record_weather_event_elapsed`].
    pub time_elapsed: i32,
}

/// Candidate active event inside an `AWeatherController` pool (`pool + 0x10`,
/// stride `0x24` B) inspected by
/// `UWeatherControllerRecorderComponent::RecordOwnerState` (`C035`,
/// `0x1418e98b0` server / `0x1418ea8d0` client) (F-654, F-665, F-734).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WeatherControllerEventCandidate<E> {
    /// Event `StartTime` (`+0x00`, `i32`).
    pub event_start_time: i32,
    /// `WeatherEventRow` (`+0x04..0x1c`).
    pub weather_event_row: E,
    /// Whether `FRowHandle::IsValid` (`0x143f855d0` client) returned `true`.
    pub is_row_valid: bool,
    /// `Row.Duration` (`[Row + 0x20]`) when the `WeatherEvents` data-table row
    /// lookup (`0x1416b8cd0` client) succeeded.
    pub row_duration: Option<i32>,
}

/// Evaluates `UWeatherControllerRecorderComponent::RecordOwnerState` (`C035`,
/// `0x1418e98b0` server / `0x1418ea8d0` client) (F-654, F-665, F-734).
///
/// - Returns `None` when the owner is null or not an `AWeatherController`
///   (`0x141deb0f0` client; no `PendingKill` check is performed), leaving
///   `recorder + 0xd8` untouched.
/// - Otherwise unconditionally clears `recorder + 0xd8` (`WeatherEvents`,
///   stride `0x14` B) and, iff `level_time_elapsed_sec > 0` and
///   `current_block_end_time >= 0`, iterates active pools (`pool->IsActive()`
///   at `vtable + 0x28`) in `AWeatherController + 0x2a0` and appends each
///   candidate event whose row is valid, resolves a `row_duration`, and
///   satisfies [`should_record_weather_event`].
pub fn evaluate_weather_controller_record<E: Clone, P: Clone>(
    owner_is_weather_controller: bool,
    level_time_elapsed_sec: i32,
    current_block_end_time: i32,
    pools: &[(P, bool, &[WeatherControllerEventCandidate<E>])],
) -> Option<Vec<RecordedWeatherEvent<E, P>>> {
    if !owner_is_weather_controller {
        return None;
    }
    let mut recorded = Vec::new();
    if level_time_elapsed_sec > 0 && current_block_end_time >= 0 {
        for (pool_row, pool_is_active, candidates) in pools {
            if !*pool_is_active {
                continue;
            }
            for candidate in *candidates {
                if candidate.is_row_valid
                    && let Some(duration) = candidate.row_duration
                    && should_record_weather_event(
                        duration,
                        candidate.event_start_time,
                        current_block_end_time,
                    )
                {
                    recorded.push(RecordedWeatherEvent {
                        weather_event_row: candidate.weather_event_row.clone(),
                        pool_row: pool_row.clone(),
                        time_elapsed: record_weather_event_elapsed(
                            level_time_elapsed_sec,
                            candidate.event_start_time,
                        ),
                    });
                }
            }
        }
    }
    Some(recorded)
}

/// Saved `FRecordedWeatherEvent` entry (`0x14` B) inspected by
/// `UWeatherControllerRecorderComponent::UpdateOwnerToDatabaseState` (`C035`,
/// `0x1418f8120` server / `0x1418f87f0` client) (F-654, F-665, F-734).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SavedWeatherEventEntry<E, P> {
    pub weather_event_row: E,
    pub weather_event_row_valid: bool,
    pub pool_row: P,
    pub pool_row_valid: bool,
    pub time_elapsed: i32,
}

/// Evaluates `UWeatherControllerRecorderComponent::UpdateOwnerToDatabaseState`
/// (`C035`, `0x1418f8120` server / `0x1418f87f0` client) (F-654, F-665, F-734).
///
/// Requires a non-null `AWeatherController` (`0x141deb0f0` client) and
/// `level_time_elapsed_sec > 0` (`UIcarusFunctionLibrary::GetLevelTimeElapsedSec`
/// `0x1414e1390` client); otherwise returns `None`. For each saved event in
/// `recorder + 0xd8` where both `pool_row_valid` (`+0x08`) and
/// `weather_event_row_valid` (`+0x00`) are `true`, ensures the pool for
/// `pool_row` exists in `AWeatherController + 0x2a0` and inserts
/// `(weather_event_row, start_time = restored_weather_event_start_time(level_time_elapsed_sec, time_elapsed))`
/// (`0x1414c2c50` client) into that pool's `ActiveEvents` (`pool + 0x10`).
pub fn evaluate_weather_controller_restore<E: Clone, P: Clone>(
    owner_is_weather_controller: bool,
    level_time_elapsed_sec: i32,
    saved_events: &[SavedWeatherEventEntry<E, P>],
) -> Option<Vec<(P, E, i32)>> {
    if !owner_is_weather_controller || level_time_elapsed_sec <= 0 {
        return None;
    }
    let restored = saved_events
        .iter()
        .filter(|entry| entry.pool_row_valid && entry.weather_event_row_valid)
        .map(|entry| {
            (
                entry.pool_row.clone(),
                entry.weather_event_row.clone(),
                restored_weather_event_start_time(level_time_elapsed_sec, entry.time_elapsed),
            )
        })
        .collect();
    Some(restored)
}

/// The elapsed level seconds a restored game mode starts from (F-653).
pub fn restored_elapsed_seconds(saved_elapsed: i32, game_time: f32) -> i32 {
    if saved_elapsed < ELAPSED_SECONDS_FALLBACK_BELOW && game_time > 0.0 {
        truncate(game_time)
    } else {
        saved_elapsed
    }
}

/// Restores `AIcarusGameModeSurvival::ExportedCurrencies` (`+0x670`) in
/// `UIcarusGameModeStateRecorderComponent::UpdateOwnerToDatabaseState` (`C015`,
/// `0x1418b7f30` server / `0x1418bae00` client) (F-653).
///
/// Clears `ExportedCurrencies`, seeds `(exotics_row, total_exotics_exported)`
/// (`+0x1f4`, row `1` `"Exotics"`) and
/// `(red_exotics_row, total_red_exotics_exported)` (`+0x1f8`, row `3`
/// `"Exotics_Red"`) when `recorded_version < 2` (`[this + 0x2a4] < 2`), and
/// then inserts or overwrites each `(row, amount)` entry from
/// `saved_currencies` (`+0x200`, stride `0x0c` B) in order.
pub fn restore_game_mode_exported_currencies<R: PartialEq + Clone>(
    recorded_version: i32,
    exotics_row: R,
    total_exotics_exported: i32,
    red_exotics_row: R,
    total_red_exotics_exported: i32,
    saved_currencies: &[(R, i32)],
) -> Vec<(R, i32)> {
    let mut restored = Vec::new();
    if recorded_version < 2 {
        upsert_recorded_variable(&mut restored, exotics_row, total_exotics_exported);
        upsert_recorded_variable(&mut restored, red_exotics_row, total_red_exotics_exported);
    }
    for (row, amount) in saved_currencies {
        upsert_recorded_variable(&mut restored, row.clone(), *amount);
    }
    restored
}

/// One `FMissionHistoryEntry` (`0x18` B) restored into
/// `AIcarusGameModeSurvival::MissionHistory` (`+0x6c0`) by `C015`
/// `UIcarusGameModeStateRecorderComponent::UpdateOwnerToDatabaseState`
/// (`0x1418b7f30` server / `0x1418bae00` client) (F-653).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RestoredMissionHistoryEntry<K> {
    pub quest_key: K,
    pub completed: bool,
    pub end_time: i32,
}

/// Restores `AIcarusGameModeSurvival::MissionHistory` (`+0x6c0`) in
/// `UIcarusGameModeStateRecorderComponent::UpdateOwnerToDatabaseState` (`C015`,
/// `0x1418b7f30` server / `0x1418bae00` client) (F-653).
///
/// - When `recorded_version < 1` (`[this + 0x2a4] < 1`), restores only the
///   last entry of `saved_history` (`+0x280`, if non-empty); when
///   `recorded_version >= 1`, restores all entries of `saved_history` in order.
/// - If the resulting `MissionHistory` is still empty and an active
///   `AIcarusQuest` actor exists in the world with a valid `QuestDTKey`
///   (`+0x1c0`), appends a synthetic entry with `completed = [quest + 0x1e0] != 0`
///   and `end_time = -1`.
pub fn restore_game_mode_mission_history<K: Clone>(
    recorded_version: i32,
    saved_history: &[RestoredMissionHistoryEntry<K>],
    active_world_quest: Option<(K, bool)>,
) -> Vec<RestoredMissionHistoryEntry<K>> {
    let mut restored = if recorded_version < 1 {
        saved_history.last().cloned().into_iter().collect()
    } else {
        saved_history.to_vec()
    };
    if restored.is_empty()
        && let Some((quest_key, completed)) = active_world_quest
    {
        restored.push(RestoredMissionHistoryEntry {
            quest_key,
            completed,
            end_time: -1,
        });
    }
    restored
}

/// Restores `AIcarusGameModeSurvival::NextMeteorShowerTime` (`+0x6ec`) in
/// `UIcarusGameModeStateRecorderComponent::UpdateOwnerToDatabaseState` (`C015`,
/// `0x1418b7f30` server / `0x1418bae00` client) from saved
/// `NextMeteorShowerTime` (`+0x2a0`), `GetLevelTimeElapsedSec(GameMode)`
/// (`0x1414e1390`), and `MeteorShowerInterval` (`GameMode + 0x6f0`) (F-653):
/// keeps `saved_next_meteor_shower_time` when non-zero, or computes
/// `level_time_elapsed_sec.wrapping_add(meteor_shower_interval)` when `0`.
pub const fn restored_next_meteor_shower_time(
    saved_next_meteor_shower_time: i32,
    level_time_elapsed_sec: i32,
    meteor_shower_interval: i32,
) -> i32 {
    if saved_next_meteor_shower_time != 0 {
        saved_next_meteor_shower_time
    } else {
        level_time_elapsed_sec.wrapping_add(meteor_shower_interval)
    }
}

/// Values recorded by `UWorldBossRecorderComponent::RecordOwnerState` (`C037`,
/// `0x1418ea790` server / `0x1418eae50` client) (F-654).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorldBossRecordPlan<R, S> {
    /// `BossSpawnerRow.RowName` (`AWorldBossSpawner + 0x300 -> recorder + 0xe8`).
    pub boss_spawner_row_name: R,
    /// `BossStatuses` (`AWorldBossSpawner + 0x2c0 -> recorder + 0xd8`, count
    /// `+0xe0`, stride `0x60` B), each starting with `BossRowHandle.RowName` at
    /// `+0x00`.
    pub boss_statuses: Vec<(R, S)>,
    /// `ActiveWorldBosses` (`recorder + 0xf0`, count `+0xf8`, stride `0x18` B),
    /// cleared and derived by extracting `BossRowHandle.RowName` (`+0x00`) from
    /// each recorded `BossStatuses` entry in order.
    pub active_world_bosses: Vec<R>,
}

/// Evaluates `UWorldBossRecorderComponent::RecordOwnerState` (`C037`,
/// `0x1418ea790` server / `0x1418eae50` client) (F-654).
///
/// Returns `None` when the owner is null, not an `AWorldBossSpawner`
/// (`0x141e03490` client), or pending kill.
pub fn evaluate_world_boss_record<R: Clone, S: Clone>(
    owner_is_valid_world_boss_spawner: bool,
    boss_spawner_row_name: R,
    owner_boss_statuses: &[(R, S)],
) -> Option<WorldBossRecordPlan<R, S>> {
    if !owner_is_valid_world_boss_spawner {
        return None;
    }
    let boss_statuses = owner_boss_statuses.to_vec();
    let active_world_bosses = boss_statuses
        .iter()
        .map(|(row_name, _)| row_name.clone())
        .collect();
    Some(WorldBossRecordPlan {
        boss_spawner_row_name,
        boss_statuses,
        active_world_bosses,
    })
}

/// Overwrites `BossStatuses[i].BossRowHandle` (`+0x00`, stride `0x60` B) from
/// legacy `ActiveWorldBosses` (`+0xf0`, count `+0xf8`, stride `0x18` B) in
/// `UWorldBossRecorderComponent::UpdateOwnerToDatabaseState` (`C037`,
/// `0x1418f8610` server / `0x1418f8ce0` client) (F-654).
///
/// For each index `i` in `0..active_world_bosses.len()`, updates
/// `boss_status_row_handles[i]` in place when `i < boss_status_row_handles.len()`
/// and ignores trailing `active_world_bosses` entries beyond
/// `boss_status_row_handles.len()`. Returns the number of overwritten slots.
pub fn restore_world_boss_row_handles<R: Clone>(
    boss_status_row_handles: &mut [R],
    active_world_bosses: &[R],
) -> usize {
    let count = boss_status_row_handles.len().min(active_world_bosses.len());
    for (slot, active) in boss_status_row_handles[..count]
        .iter_mut()
        .zip(&active_world_bosses[..count])
    {
        *slot = active.clone();
    }
    count
}

/// A saved survival level, or `None` when it was not recorded (F-648).
pub fn recorded_level(saved: i32) -> Option<i32> {
    (saved != UNRECORDED_LEVEL).then_some(saved)
}

/// How `UPlayerStateRecorderComponent::UpdateOwnerToDatabaseState` (`C026`,
/// `vftable + 0x460`: `0x1418f7320` server / `0x1418f79f0` client) and
/// `UPlayerStateRecorderComponent::Stitching` (`vftable + 0x468`: `0x1418f5540`
/// server / `0x1418f5c80` client) restore life/health state on
/// `AIcarusPlayerCharacter` (F-648).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlayerStateLifeRestore {
    /// `bIsAlive` (`+0x1f0`) is `true`: `UpdateOwnerToDatabaseState` calls
    /// `UIcarusStatComponent::SetHealth` (`0x141444cd0` server / `0x141482ce0`
    /// client) on `Character->StatComponent` (`+0x5a8`) iff `saved_health`
    /// (`+0x1f4`) is not `-1` ([`UNRECORDED_LEVEL`]); `Stitching` is a no-op.
    Alive { set_health: Option<i32> },
    /// `bIsAlive` (`+0x1f0`) is `false`: `UpdateOwnerToDatabaseState` sets
    /// `StatComponent + 0x494 = 1` without calling `SetHealth`, and `Stitching`
    /// invokes the death transition (`0x141c09e10` server / `0x141bfe650`
    /// client) iff `AIcarusPlayerCharacter::GetIcarusPlayerController`
    /// (`0x141428bb0` server / `0x141465f50` client) returns a valid controller.
    Dead {
        mark_stat_component_dead: bool,
        kill_character_on_stitch: bool,
    },
}

/// Evaluates `C026` (`UPlayerStateRecorderComponent`) life/health restoration
/// across `UpdateOwnerToDatabaseState` (`+0x460`) and `Stitching` (`+0x468`)
/// (F-648).
pub fn evaluate_player_state_life_restore(
    b_is_alive: bool,
    saved_health: i32,
    has_valid_player_controller: bool,
) -> PlayerStateLifeRestore {
    if b_is_alive {
        PlayerStateLifeRestore::Alive {
            set_health: recorded_level(saved_health),
        }
    } else {
        PlayerStateLifeRestore::Dead {
            mark_stat_component_dead: true,
            kill_character_on_stitch: has_valid_player_controller,
        }
    }
}

/// Filters and deduplicates `CosmeticArmourOverrides` (`+0x228`) as
/// `UPlayerStateRecorderComponent::UpdateOwnerToDatabaseState` (`C026`,
/// `0x1418f754f..0x1418f7685` server / `0x1418f79f0` client) does (F-648).
///
/// Each saved `(row, is_row_handle_valid)` entry is retained only when
/// `FRowHandle::IsValid` (`0x143da9940` server / `0x143f855d0` client) is `true`
/// and `TArray<FItemsStaticRowHandle>::AddUnique` (`0x1414c9dc0` server /
/// `0x1414fe510` client) has not already accepted an equal row handle. Returns
/// `(deduped_valid_rows, should_apply_and_rep)`: `Character + 0xa80` is
/// overwritten and `AIcarusPlayerCharacter::OnRep_CosmeticArmourOverrides`
/// (`0x141476bf0` server) is called iff `deduped_valid_rows` is non-empty.
pub fn filter_restored_cosmetic_armour_overrides<T: PartialEq + Clone>(
    entries: &[(T, bool)],
) -> (Vec<T>, bool) {
    let mut deduped = Vec::new();
    for (row, is_valid) in entries {
        if *is_valid && !deduped.contains(row) {
            deduped.push(row.clone());
        }
    }
    let should_apply_and_rep = !deduped.is_empty();
    (deduped, should_apply_and_rep)
}

/// A restored mount's rotation (F-651): the saved quaternion `[x, y, z, w]`
/// with pitch and roll dropped, keeping only its yaw.
///
/// This uses the engine's formulas with exact trigonometry; the engine's own
/// trigonometric approximations are not reproduced bit for bit.
pub fn upright_rotation([x, y, z, w]: [f32; 4]) -> [f32; 4] {
    let yaw = (2.0 * (w * z + x * y)).atan2(1.0 - 2.0 * (y * y + z * z));
    let (sin, cos) = (yaw / 2.0).sin_cos();
    [0.0, 0.0, sin, cos]
}

/// Evaluates `UIcarusCharacterRecorderComponent::Stitching` (`vftable + 0x468`:
/// `0x141446df0` server / `0x141484cd0` client, inherited by `C017`
/// `UIcarusMountCharacterRecorderComponent`) (F-651, F-741).
///
/// When `parent_character_uid != -1` (`[this + 0x1c4] != -1`), `world_present`
/// (`GetWorld()` non-null), and `owner_is_valid_character` (`GetOwner()` is a
/// non-null `AIcarusCharacter` `0x141bb51c0` server), scans `SpawnedActors` in
/// order and returns the index of the first `AIcarusCharacter` whose
/// `IcarusActorGUID` (`[actor + 0x564]`) equals `parent_character_uid` so it
/// can be assigned to `owner->ParentCharacter` (`[owner + 0x738]`).
pub fn stitch_character_parent_uid(
    owner_is_valid_character: bool,
    world_present: bool,
    parent_character_uid: i32,
    spawned_character_guids: &[Option<i32>],
) -> Option<usize> {
    if !owner_is_valid_character || !world_present || parent_character_uid == -1 {
        return None;
    }
    spawned_character_guids
        .iter()
        .position(|guid| *guid == Some(parent_character_uid))
}

/// Spawn parameters evaluated by `UIcarusNPCRecorderComponent::RespawnOwner`
/// (`vftable + 0x458`: `0x1413cfe80` server / `0x14140eab0` client, inherited
/// by `C017` `UIcarusMountCharacterRecorderComponent`) (F-651, F-741).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NpcRecorderRespawnPlan<R> {
    /// Validated `AISetupRowName` (`recorder + 0x1d8`) passed to
    /// `UAIStateRecorderSubsystem::SpawnAICharacter` (`0x1413db500` server).
    pub ai_setup_row_name: R,
    /// Whether `(icarus_actor_guid, recorder)` was registered on
    /// `UAIStateRecorderSubsystem::BindActorGuid` (`0x1413dec50` server)
    /// (`icarus_actor_guid > 0 && subsystem_bound_guid.is_some()`).
    pub registered_guid_in_subsystem: bool,
    /// Resolved `IcarusActorGUID` passed into `SpawnAICharacter`: `-1`
    /// (`0xffffffff`) when `icarus_actor_guid <= 0`, `bound_guid` when
    /// `icarus_actor_guid > 0` and `subsystem_bound_guid == Some(bound_guid)`,
    /// or `icarus_actor_guid` when `icarus_actor_guid > 0` and the subsystem is
    /// absent (`None`).
    pub spawn_icarus_actor_guid: i32,
    /// `bNoFail` (`[rsp + 0x28] = 1`) passed to `SpawnAICharacter`.
    pub b_no_fail: bool,
    /// `ESpawnActorCollisionHandlingMethod` (`[rsp + 0x20] = 1`, `AlwaysSpawn`)
    /// passed to `SpawnAICharacter`.
    pub collision_handling_override: u8,
}

/// Evaluates `UIcarusNPCRecorderComponent::RespawnOwner` (`vftable + 0x458`:
/// `0x1413cfe80` server / `0x14140eab0` client, inherited by `C017`
/// `UIcarusMountCharacterRecorderComponent`) (F-651, F-741).
///
/// Returns `None` when `GetWorld()` is null (`!world_present`) or the
/// `FAISetupRowHandle` built from `AISetupRowName` (`+0x1d8`, `0x1414d3260`)
/// fails `FRowHandle::IsValid` (`0x143da9940` server).
pub const fn evaluate_npc_recorder_respawn<R: Copy>(
    world_present: bool,
    ai_setup_row_is_valid: bool,
    ai_setup_row_name: R,
    icarus_actor_guid: i32,
    subsystem_bound_guid: Option<i32>,
) -> Option<NpcRecorderRespawnPlan<R>> {
    if !world_present || !ai_setup_row_is_valid {
        return None;
    }
    let (registered_guid_in_subsystem, spawn_icarus_actor_guid) = if icarus_actor_guid > 0 {
        match subsystem_bound_guid {
            Some(bound) => (true, bound),
            None => (false, icarus_actor_guid),
        }
    } else {
        (false, -1)
    };
    Some(NpcRecorderRespawnPlan {
        ai_setup_row_name,
        registered_guid_in_subsystem,
        spawn_icarus_actor_guid,
        b_no_fail: true,
        collision_handling_override: 1,
    })
}

/// The world player list that validates player, player-state and rocket
/// records (F-652).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlayerList {
    /// No list could be reached.
    Absent,
    /// The list exists but is not active.
    Inactive,
    /// An active list, holding the record's player id or not.
    Active { has_player: bool },
}

impl PlayerList {
    fn rejects(self) -> bool {
        self == Self::Active { has_player: false }
    }
}

/// One `FAssociatedMemberInfo` entry in `UProspectSubsystem`'s
/// `ActiveProspectInfo.AssociatedMembers` (`UProspectSubsystem + 0x98`, stride
/// `0x40` B), inspected by
/// `UProspectSubsystem::IsPlayerStillAssociatedWithActiveProspect`
/// (`0x1418231d0` server / `0x14182ec60` client) (F-652, F-663).
///
/// `AccountName`, `CharacterName`, `Status`, `Settled`, and
/// `IsCurrentlyPlaying` are ignored by the association check.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProspectAssociatedMember<'a> {
    pub user_id: &'a [u16],
    pub chr_slot: i32,
}

/// Resolves [`PlayerList`] from `UProspectSubsystem` (`0x141704d70` server /
/// `0x141715480` client), `UProspectSubsystem::HasActiveProspect`
/// (`0x141820ba0` server / `0x14182c4c0` client), and
/// `UProspectSubsystem::IsPlayerStillAssociatedWithActiveProspect`
/// (`0x1418231d0` server / `0x14182ec60` client ->
/// `TArray<FAssociatedMemberInfo>::FindByKey` at `0x1417fd560` server /
/// `0x141809720` client) (F-652).
///
/// - When `UProspectSubsystem` is unreachable (`!subsystem_present`), returns
///   [`PlayerList::Absent`].
/// - When `ActiveProspectInfo.ProspectID` (`+0x30`) is empty, returns
///   [`PlayerList::Inactive`].
/// - Otherwise returns [`PlayerList::Active`] with `has_player: true` iff any
///   entry in `ActiveProspectInfo.AssociatedMembers` (`+0x98`) matches both
///   `user_id` (ASCII case-insensitively via `FGenericPlatformStricmp::Stricmp`)
///   and `chr_slot` (`+0x30`).
pub fn resolve_prospect_player_list(
    subsystem_present: bool,
    active_prospect_id: &[u16],
    associated_members: &[ProspectAssociatedMember<'_>],
    player_id: &[u16],
    chr_slot: i32,
) -> PlayerList {
    if !subsystem_present {
        return PlayerList::Absent;
    }
    if active_prospect_id.is_empty() {
        return PlayerList::Inactive;
    }
    let has_player = associated_members
        .iter()
        .any(|member| member.chr_slot == chr_slot && names_match(member.user_id, player_id));
    PlayerList::Active { has_player }
}

/// Whether a player or player-state record validates: its recorder version is
/// at least 1 and an active list does not lack its player.
pub fn player_record_valid(version: i32, list: PlayerList) -> bool {
    version >= 1 && !list.rejects()
}

/// Whether a rocket record validates: an active list does not lack its player.
pub fn rocket_record_valid(list: PlayerList) -> bool {
    !list.rejects()
}

/// Validates a rocket-spawn record (`C011`,
/// `URocketSpawnRecorderComponent::Validate` at `0x1418fa010` server /
/// `0x1418fa340` client) (F-652).
///
/// When `version < 1` or `list` is `PlayerList::Active { has_player: false }`,
/// invokes `URocketSpawnRecorderComponent::ResetSpawnRecord` (`0x1418ed540`
/// server / `0x1418edf10` client) to clear `player_id` (`+0x1c8`) and set
/// `chr_slot` (`+0x1d8`) to `-1`. Returns `(is_valid, was_reset)`: `is_valid`
/// is always `true` (`1`) so the spawn point record stays valid for future
/// players instead of being dropped, and `was_reset` is `true` iff
/// `ResetSpawnRecord` cleared a non-empty `player_id` or a `chr_slot != -1`.
pub fn validate_rocket_spawn_record(
    version: i32,
    list: PlayerList,
    player_id: &mut Vec<u16>,
    chr_slot: &mut i32,
) -> (bool, bool) {
    if version < 1 || list.rejects() {
        let was_reset = !player_id.is_empty() || *chr_slot != -1;
        player_id.clear();
        *chr_slot = -1;
        return (true, was_reset);
    }
    (true, false)
}

const SUPER_COOLED_ICE_UTF16: [u16; 16] = [
    b'S' as u16,
    b'u' as u16,
    b'p' as u16,
    b'e' as u16,
    b'r' as u16,
    b'_' as u16,
    b'C' as u16,
    b'o' as u16,
    b'o' as u16,
    b'l' as u16,
    b'e' as u16,
    b'd' as u16,
    b'_' as u16,
    b'I' as u16,
    b'c' as u16,
    b'e' as u16,
];

/// Whether a resource-deposit record (`C028`,
/// `UResourceDepositRecorderComponent::Validate` at `0x1418f9f20` server /
/// `0x1418fa250` client) validates (F-645, F-652, F-742, F-743): rejects
/// `"Super_Cooled_Ice"` (`0x144a71800` server / `0x144bcddc0` client) ASCII
/// case-insensitively and accepts every other deposit type. Its package
/// subclass `UResourceDepositIceRecorderComponent` (derived vtable
/// `0x144a71348` server / `0x144bcd908` client; `C027` is
/// `UPrebuiltStructureRecorderComponent`) overrides `Validate` (`+0x408`) back
/// to `true` (`0x140899f70` / `0x1408c9810`) so `"Super_Cooled_Ice"` deposits
/// validate under `UResourceDepositIceRecorderComponent`.
pub fn resource_deposit_record_valid(deposit_type_name: &[u16]) -> bool {
    !names_match(deposit_type_name, &SUPER_COOLED_ICE_UTF16)
}

/// What restoring a rocket does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RocketRestore {
    /// Apply the saved state, spawn and descent points and flags.
    Apply,
    /// Destroy the rocket.
    Destroy,
}

/// Restore applies only when an active list holds the player and the saved
/// recorder version is positive; every other case destroys the rocket.
pub fn rocket_restore(version: i32, list: PlayerList) -> RocketRestore {
    if version > 0 && list == (PlayerList::Active { has_player: true }) {
        RocketRestore::Apply
    } else {
        RocketRestore::Destroy
    }
}

/// The saved container entries of a world container list (F-651): each
/// present container with its list position, empty positions skipped. The
/// saved count is the full list length.
pub fn container_entries<T>(list: &[Option<T>]) -> impl Iterator<Item = (usize, &T)> {
    list.iter()
        .enumerate()
        .filter_map(|(index, entry)| entry.as_ref().map(|entry| (index, entry)))
}

/// Outcome of `UIcarusContainerManagerRecorderComponent::RecordOwnerState`
/// (`C016`, `0x1416f1be0` server / `0x141702570` client) after
/// `UActorStateRecorderComponent::RecordOwnerState` (`0x1418a0170` server /
/// `0x1418a5b60` client) (F-650, F-651).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContainerManagerRecordPlan<C> {
    /// `SavedContainers` (`recorder + 0x1c0`, count `+0x1c8`, stride `0x20` B:
    /// `(index, container_save_data)`), unconditionally cleared on entry and
    /// populated via [`container_entries`] when `owner` is a non-null,
    /// non-pending-kill `AContainerManager` (`0x141b35320` client).
    pub saved_containers: Vec<(usize, C)>,
    /// `ContainerCount` (`recorder + 0x1d0`), unconditionally reset to `0` on
    /// entry and overwritten with `owner->Containers.Num()` (`owner + 0x2c8`)
    /// when `owner` is a valid `AContainerManager`.
    pub container_count: usize,
}

/// Evaluates `UIcarusContainerManagerRecorderComponent::RecordOwnerState`
/// (`C016`, `0x1416f1be0` server / `0x141702570` client) (F-650, F-651).
pub fn evaluate_container_manager_record<C: Clone>(
    owner_is_valid_container_manager: bool,
    owner_containers: &[Option<C>],
) -> ContainerManagerRecordPlan<C> {
    if !owner_is_valid_container_manager {
        return ContainerManagerRecordPlan {
            saved_containers: Vec::new(),
            container_count: 0,
        };
    }
    let saved_containers = container_entries(owner_containers)
        .map(|(index, entry)| (index, entry.clone()))
        .collect();
    ContainerManagerRecordPlan {
        saved_containers,
        container_count: owner_containers.len(),
    }
}

/// Actions evaluated by `UIcarusContainerManagerRecorderComponent::Stitching`
/// (`C016`, `0x1416f8980` server / `0x1417099d0` client) (F-651).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContainerManagerStitchPlan<C, A> {
    /// Saved `(index, container_save_data)` array (`recorder + 0x1c0`) passed
    /// to `AContainerManager::RestoreContainers` (`0x141702be0` client).
    pub restored_containers: Vec<(usize, C)>,
    /// Saved `ContainerCount` (`recorder + 0x1d0`) passed to
    /// `AContainerManager::RestoreContainers` (`0x141702be0` client).
    pub container_count: usize,
    /// Non-pending-kill actors in `SpawnedActors` that have a non-null
    /// `UInventoryContainer` component (`0x141cf1390` client) and are passed in
    /// order to `AContainerManager::RebindActorInventoryContainer`
    /// (`0x141700fe0` client).
    pub rebound_actor_containers: Vec<A>,
}

/// Evaluates `UIcarusContainerManagerRecorderComponent::Stitching` (`C016`,
/// `0x1416f8980` server / `0x1417099d0` client) (F-651).
///
/// Returns `None` when the owner is null, not an `AContainerManager`
/// (`0x141b35320` client), or pending kill.
pub fn evaluate_container_manager_stitch<C: Clone, A: Clone>(
    owner_is_valid_container_manager: bool,
    saved_containers: &[(usize, C)],
    saved_container_count: usize,
    spawned_actor_inventory_containers: &[Option<A>],
) -> Option<ContainerManagerStitchPlan<C, A>> {
    if !owner_is_valid_container_manager {
        return None;
    }
    let rebound_actor_containers = spawned_actor_inventory_containers
        .iter()
        .flatten()
        .cloned()
        .collect();
    Some(ContainerManagerStitchPlan {
        restored_containers: saved_containers.to_vec(),
        container_count: saved_container_count,
        rebound_actor_containers,
    })
}

/// One weather-forecast block: the time values `[start, end)` it covers and
/// its pattern index.
#[derive(Clone, Copy, Debug)]
pub struct ForecastBlock {
    pub start: i32,
    pub end: i32,
    pub pattern_index: i32,
}

/// Where a forecast's current time value sits, as its record saves it (F-665).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ForecastPosition {
    pub start_time_delta: i32,
    pub pattern_index: i32,
    pub recorded_now: i32,
}

/// The saved position of `now` in a forecast's blocks: the offset into the
/// first block containing it and that block's pattern index. With no
/// containing block the offset is `now` itself and the first block's pattern
/// is kept; with no blocks or a negative `now` every value is zero.
pub fn forecast_position(blocks: &[ForecastBlock], now: i32) -> ForecastPosition {
    let Some(first) = blocks.first() else {
        return ForecastPosition::default();
    };
    if now < 0 {
        return ForecastPosition::default();
    }
    match blocks
        .iter()
        .find(|block| block.start <= now && block.end > now)
    {
        Some(block) => ForecastPosition {
            start_time_delta: now.wrapping_sub(block.start),
            pattern_index: block.pattern_index,
            recorded_now: now,
        },
        None => ForecastPosition {
            start_time_delta: now,
            pattern_index: first.pattern_index,
            recorded_now: now,
        },
    }
}

/// `NowBlock` (`recorder + 0xd8..0x108` <-> `AWeatherForecastManager + 0x2c0..0x2f0`)
/// recorded by `UWeatherForecastRecorderComponent::RecordOwnerState` (`C036`,
/// `0x1418e9ff0` server / `0x1418eadf0` client -> `GetCurrentForecastInfo`
/// `0x141917fc0` server / `0x141916980` client) and restored by
/// `UpdateOwnerToDatabaseState` (`0x1418f86c0` server / `0x1418f8c80` client ->
/// `RestoreForecastInfo` `0x14192eff0` server / `0x14192be80` client) (F-654,
/// F-665).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WeatherForecastRecordPlan<R> {
    /// `(WeatherForecastRowName, WeatherPoolRowName)` (`recorder + 0xd8`/`+0xe0`
    /// from `owner + 0x270`/`+0x258` = `600`, restored to `owner + 0x2c0`/`+0x2c8`);
    /// `None` when `blocks` is empty or `now < 0` (resetting both row names to
    /// `0`).
    pub row_names: Option<(R, R)>,
    /// `StartTimeDelta`, `PatternIndex`, and `RecordedNow` (`recorder + 0xe8..0xf4`,
    /// restored to `owner + 0x2d0..0x2dc`), computed via [`forecast_position`].
    pub position: ForecastPosition,
    /// `Seeds` (`recorder + 0xf8`, count `+0x100`, restored to `owner + 0x2e0`,
    /// count `+0x2e8`): copied from `owner + 0x248` when `!blocks.is_empty() &&
    /// now >= 0`, or cleared to empty when `blocks.is_empty() || now < 0`.
    pub seeds: Vec<i32>,
}

/// Evaluates `UWeatherForecastRecorderComponent::RecordOwnerState` (`C036`,
/// `0x1418e9ff0` server / `0x1418eadf0` client -> `GetCurrentForecastInfo`
/// `0x141917fc0` server / `0x141916980` client) (F-654, F-665).
///
/// Returns `None` when the owner is null or not an `AWeatherForecastManager`
/// (`0x141deab90` client; no `PendingKill` check is performed).
pub fn evaluate_weather_forecast_record<R>(
    owner_is_weather_forecast_manager: bool,
    forecast_row_name: R,
    pool_row_name: R,
    blocks: &[ForecastBlock],
    now: i32,
    active_seeds: &[i32],
) -> Option<WeatherForecastRecordPlan<R>> {
    if !owner_is_weather_forecast_manager {
        return None;
    }
    if blocks.is_empty() || now < 0 {
        return Some(WeatherForecastRecordPlan {
            row_names: None,
            position: ForecastPosition::default(),
            seeds: Vec::new(),
        });
    }
    Some(WeatherForecastRecordPlan {
        row_names: Some((forecast_row_name, pool_row_name)),
        position: forecast_position(blocks, now),
        seeds: active_seeds.to_vec(),
    })
}

/// Evaluates `UWeatherForecastRecorderComponent::UpdateOwnerToDatabaseState`
/// (`C036`, `0x1418f86c0` server / `0x1418f8c80` client ->
/// `RestoreForecastInfo` `0x14192eff0` server / `0x14192be80` client) (F-654,
/// F-665).
///
/// Copies `saved_now_block` (`recorder + 0xd8..0x108`) into
/// `AWeatherForecastManager + 0x2c0..0x2f0` iff the owner is a non-null
/// `AWeatherForecastManager` (`0x141deab90` client; no `PendingKill` check is
/// performed).
pub fn evaluate_weather_forecast_restore<R: Clone>(
    owner_is_weather_forecast_manager: bool,
    saved_now_block: &WeatherForecastRecordPlan<R>,
) -> Option<WeatherForecastRecordPlan<R>> {
    owner_is_weather_forecast_manager.then(|| saved_now_block.clone())
}

/// One cave-AI class's spawn times, on a spawner or in its record.
#[derive(Clone, Debug, PartialEq)]
pub struct CaveSpawnTimes {
    pub class_name: Vec<u16>,
    pub times: Vec<f32>,
}

const BLUEPRINT_GENERATED_CLASS_UTF16: [u16; 23] = [
    b'B' as u16,
    b'l' as u16,
    b'u' as u16,
    b'e' as u16,
    b'p' as u16,
    b'r' as u16,
    b'i' as u16,
    b'n' as u16,
    b't' as u16,
    b'G' as u16,
    b'e' as u16,
    b'n' as u16,
    b'e' as u16,
    b'r' as u16,
    b'a' as u16,
    b't' as u16,
    b'e' as u16,
    b'd' as u16,
    b'C' as u16,
    b'l' as u16,
    b'a' as u16,
    b's' as u16,
    b's' as u16,
];

/// Resolves a configured cave-AI class's comparison name as
/// `AIcarusCaveAISpawner::UpdateStateFromDatabase` (`0x14141e320` client) does
/// (F-677): when `class_name` equals `"BlueprintGeneratedClass"` and
/// `ClassGeneratedBy` (`UClass + 0x208`) is present, unwraps
/// `ClassGeneratedBy->GetName()`; otherwise uses `class_name` directly.
pub fn resolve_cave_ai_class_name<'a>(
    class_name: &'a [u16],
    class_generated_by_name: Option<&'a [u16]>,
) -> &'a [u16] {
    if class_name == BLUEPRINT_GENERATED_CLASS_UTF16
        && let Some(generated_by) = class_generated_by_name
    {
        return generated_by;
    }
    class_name
}

/// Restores a cave-AI spawner's per-class spawn times from its record
/// (F-677). Each saved class with at least one time replaces the times of
/// the first configured class whose name matches it; saved classes without
/// times or without a match change nothing. Both client and server compare
/// with `FGenericPlatformStricmp::Stricmp` (`0x141e48e20` server /
/// `0x141e39a00` client), which folds ASCII only and compares non-ASCII units
/// directly, matching [`names_match`]. Returns whether any saved class had
/// times: then the spawner's restored event runs, otherwise its tick is
/// re-enabled.
pub fn restore_cave_spawn_times(
    configured: &mut [CaveSpawnTimes],
    saved: &[CaveSpawnTimes],
) -> bool {
    let mut restored = false;
    for record in saved.iter().filter(|record| !record.times.is_empty()) {
        restored = true;
        if let Some(class) = configured
            .iter_mut()
            .find(|class| names_match(&record.class_name, &class.class_name))
        {
            class.times.clone_from(&record.times);
        }
    }
    restored
}

/// Saved `ELobbyPrivacy` enumerators on `FProspectSaveStateHeader` (F-715).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub enum LobbyPrivacy {
    #[default]
    Unknown = 0,
    FriendsOnly = 1,
    Private = 2,
}

impl LobbyPrivacy {
    /// Maps a saved byte to its `ELobbyPrivacy` enumerator, or `None` for any
    /// other value.
    pub const fn from_saved(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::Unknown),
            1 => Some(Self::FriendsOnly),
            2 => Some(Self::Private),
            _ => None,
        }
    }
}

/// Applies `FProspectSaveState::PostLoadSerialize` schema migrations (F-715).
///
/// - When `loaded_version < 2`, legacy top-level `ProspectID` and
///   `ProspectDTKey` copy into `ProspectInfo.ProspectID` and
///   `ProspectInfo.ProspectDTKey`.
/// - When `loaded_version == 0` (`NoVersion`), `ProspectInfo` resets to its
///   default empty strings, `LobbyPrivacy` becomes [`LobbyPrivacy::FriendsOnly`]
///   (`1`), and `StateRecorderBlobs` is emptied.
pub fn migrate_prospect_save_state<B>(
    loaded_version: i32,
    lobby_privacy: &mut LobbyPrivacy,
    prospect_id: &mut String,
    prospect_dt_key: &mut String,
    legacy_prospect_id: &str,
    legacy_prospect_dt_key: &str,
    state_recorder_blobs: &mut Vec<B>,
) {
    if loaded_version < PROSPECT_SAVE_STATE_CURRENT_VERSION {
        prospect_id.clear();
        prospect_id.push_str(legacy_prospect_id);
        prospect_dt_key.clear();
        prospect_dt_key.push_str(legacy_prospect_dt_key);
    }
    if loaded_version == 0 {
        prospect_id.clear();
        prospect_dt_key.clear();
        *lobby_privacy = LobbyPrivacy::FriendsOnly;
        state_recorder_blobs.clear();
    }
}

const TREE_PRIMITIVE_UTF16: [u16; 13] = [
    b'T' as u16,
    b'r' as u16,
    b'e' as u16,
    b'e' as u16,
    b'P' as u16,
    b'r' as u16,
    b'i' as u16,
    b'm' as u16,
    b'i' as u16,
    b't' as u16,
    b'i' as u16,
    b'v' as u16,
    b'e' as u16,
];
const WOOD_UTF16: [u16; 4] = [b'W' as u16, b'o' as u16, b'o' as u16, b'd' as u16];

/// Applies `UActorStateRecorderComponent::PerformVersionUpgrades` (F-716).
///
/// When `actor_state_recorder_version < 1`, every recorded component entry
/// whose `FName` matches `"TreePrimitive"` (ASCII case-insensitively) is
/// replaced with `"Wood"`. Versions `>= 1` (all supplied saves use `3`) leave
/// component names untouched. Returns the number of upgraded entries.
pub fn upgrade_actor_recorder_component_names(
    actor_state_recorder_version: i32,
    component_names: &mut [Vec<u16>],
) -> usize {
    if actor_state_recorder_version >= 1 {
        return 0;
    }
    let mut upgraded = 0;
    for name in component_names {
        if names_match(name, &TREE_PRIMITIVE_UTF16) {
            name.clear();
            name.extend_from_slice(&WOOD_UTF16);
            upgraded += 1;
        }
    }
    upgraded
}

/// Whether the restore context `FTransform` passed into
/// `UActorStateRecorderComponent::RestoreState` is non-identity and therefore
/// invokes virtual slot `+0x4b0` (`ApplyRecorderTransform`) (F-716).
///
/// Rotation compares against both `+Identity` (`[0, 0, 0, 1]`) and `-Identity`
/// (`[0, 0, 0, -1]`) with per-component tolerance
/// [`ACTOR_RECORDER_TRANSFORM_TOLERANCE`] (`1e-4`), translation against
/// `[0, 0, 0]`, and scale against `[1, 1, 1]`. World prospect restore passes
/// `FTransform::Identity` (`false`); instanced-level restore passes the
/// instanced level's transform.
pub fn recorder_transform_is_non_identity(
    rotation: [f32; 4],
    translation: [f32; 3],
    scale: [f32; 3],
) -> bool {
    let [rx, ry, rz, rw] = rotation;
    let rot_xyz_zero = rx.abs() <= ACTOR_RECORDER_TRANSFORM_TOLERANCE
        && ry.abs() <= ACTOR_RECORDER_TRANSFORM_TOLERANCE
        && rz.abs() <= ACTOR_RECORDER_TRANSFORM_TOLERANCE;
    let rot_identity = rot_xyz_zero
        && ((rw - 1.0).abs() <= ACTOR_RECORDER_TRANSFORM_TOLERANCE
            || (rw + 1.0).abs() <= ACTOR_RECORDER_TRANSFORM_TOLERANCE);
    let trans_identity = translation
        .iter()
        .all(|c| c.abs() <= ACTOR_RECORDER_TRANSFORM_TOLERANCE);
    let scale_identity = scale
        .iter()
        .all(|c| (c - 1.0).abs() <= ACTOR_RECORDER_TRANSFORM_TOLERANCE);
    !(rot_identity && trans_identity && scale_identity)
}

/// Normalizes a requested prospect name as
/// `ADedicatedServerEntryGameMode::LoadProspectByName` (`0x14146e6c0`) does
/// (F-717): rejects empty inputs and strips a trailing `".json"` suffix
/// ASCII case-insensitively (`ESearchCase::IgnoreCase`).
pub fn normalize_prospect_name(input: &str) -> Option<&str> {
    if input.is_empty() {
        return None;
    }
    let bytes = input.as_bytes();
    let stripped = if bytes.len() >= 5 && bytes[bytes.len() - 5..].eq_ignore_ascii_case(b".json") {
        &input[..input.len() - 5]
    } else {
        input
    };
    (!stripped.is_empty()).then_some(stripped)
}

/// Whether two `ProspectID` strings match under
/// `FOfflineProspectManagementIcarus::GetProspectInfo` (`0x14181e530`) and
/// `UProspectSubsystem::SetActiveProspectSaveState` (`0x1418371c0`) (F-717):
/// equal UTF-16 unit lengths and ASCII case-insensitive equality.
pub fn prospect_ids_match(expected: &[u16], actual: &[u16]) -> bool {
    names_match(expected, actual)
}

/// Whether `UProspectSubsystem::SetActiveProspectInfo` (`0x141837100`) accepts
/// `new_prospect_id` given `current_active_prospect_id` (F-717): requires a
/// non-empty new `ProspectID` and an empty active `ProspectID` (after
/// `ResetActiveProspect`).
pub fn can_activate_prospect_info(new_prospect_id: &str, current_active_prospect_id: &str) -> bool {
    !new_prospect_id.is_empty() && current_active_prospect_id.is_empty()
}

/// Which `PerformVersionUpgrades` (`+0x488`) override a `StateRecorderBlobs`
/// class (`C001`..`C038`) dispatches during `RestoreState` (F-716, F-727).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VersionUpgradeKind {
    /// Base `UIcarusStateRecorderComponent` no-op (`0x140898810`).
    NoOp,
    /// `UActorStateRecorderComponent::PerformVersionUpgrades` (`0x14189d380`),
    /// which renames `"TreePrimitive"` to `"Wood"` when `RecordedVersion < 1`.
    ActorTreePrimitiveToWood,
}

/// Which `RestoreState` (`+0x4a0`) override a `StateRecorderBlobs` class
/// (`C001`..`C038`) dispatches (F-716, F-727).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RestoreStateKind {
    /// Base `UIcarusStateRecorderComponent::RestoreState` (`0x1418a9c80`):
    /// constructs `FMemoryReader` (`v522`), calls `+0xa0` (`Serialize`) and
    /// `+0x488` (`PerformVersionUpgrades`).
    BaseSerializeOnly,
    /// `UActorStateRecorderComponent::RestoreState` (`0x1418a9bb0`): runs
    /// `BaseSerializeOnly` and then invokes `+0x4b0` (`ApplyRecorderTransform`)
    /// when [`recorder_transform_is_non_identity`] is true.
    ActorWithContextTransform,
    /// `UIcarusMountCharacterRecorderComponent::RestoreState` (`0x141400150`):
    /// runs `ActorWithContextTransform` and then [`upright_rotation`] on the
    /// saved actor rotation (`+0xe0`).
    MountActorUprightYaw,
}

/// Which `ApplyRecorderTransform` (`+0x4b0`) override a `StateRecorderBlobs`
/// class (`C001`..`C038`) dispatches when context `FTransform` is non-identity
/// (F-716, F-727).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApplyTransformKind {
    /// Base `UIcarusStateRecorderComponent` stub (`0x1408b2280`, returns `true`).
    NoOpTrue,
    /// `UActorStateRecorderComponent::ApplyRecorderTransform` (`0x141887d20`):
    /// transforms `ActorTransform` (`+0x100`).
    ActorTransformOnly,
    /// `UBuildingGridRecorderComponent::ApplyRecorderTransform` (`0x141888270`):
    /// transforms `ActorTransform` (`+0x100`) and `BuildingGridRecord` (`+0x1c0`).
    ActorAndBuildingGrid,
    /// `USplineRecorderComponent::ApplyRecorderTransform` (`0x1418c5cd0`):
    /// transforms `ActorTransform` (`+0x100`) and each `FRecordedSplinePoint`
    /// in `SplineNetworkSaveData.Points` (`+0x238`, stride `0x50`).
    ActorAndSplinePoints,
}

/// Static restore dispatch profile for one `StateRecorderBlobs` class ordinal
/// `1..=38` (`C001`..`C038`), identical in server `25621760` and client
/// `25793767` (F-727). Every class inherits `UObject::Serialize(FArchive&)`
/// (`+0xa0`, `0x1418aaf70` / `0x1418b0450`) unchanged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RecorderRestoreDispatch {
    pub class_ordinal: u8,
    pub selected_property_count: u8,
    pub version_upgrade: VersionUpgradeKind,
    pub restore_state: RestoreStateKind,
    pub apply_transform: ApplyTransformKind,
}

const SELECTED_PROPERTY_COUNTS_C001_TO_C038: [u8; 38] = [
    20, 22, 32, 21, 21, 21, 33, 32, 31, 32, 22, 22, 5, 5, 30, 22, 46, 29, 25, 23, 29, 5, 20, 21, 9,
    34, 22, 23, 28, 34, 21, 21, 24, 21, 5, 5, 7, 21,
];

/// Returns the static restore dispatch profile for `class_ordinal` in `1..=38`
/// (`C001`..`C038`), or `None` if `class_ordinal` is outside `1..=38` (F-727).
pub const fn recorder_restore_dispatch(class_ordinal: u8) -> Option<RecorderRestoreDispatch> {
    if class_ordinal < 1 || class_ordinal > 38 {
        return None;
    }
    let selected_property_count =
        SELECTED_PROPERTY_COUNTS_C001_TO_C038[(class_ordinal - 1) as usize];
    let is_direct_base_subclass = matches!(class_ordinal, 13 | 14 | 22 | 25 | 35 | 36 | 37);
    if is_direct_base_subclass {
        return Some(RecorderRestoreDispatch {
            class_ordinal,
            selected_property_count,
            version_upgrade: VersionUpgradeKind::NoOp,
            restore_state: RestoreStateKind::BaseSerializeOnly,
            apply_transform: ApplyTransformKind::NoOpTrue,
        });
    }
    let restore_state = if class_ordinal == 17 {
        RestoreStateKind::MountActorUprightYaw
    } else {
        RestoreStateKind::ActorWithContextTransform
    };
    let apply_transform = match class_ordinal {
        4 => ApplyTransformKind::ActorAndBuildingGrid,
        32 => ApplyTransformKind::ActorAndSplinePoints,
        _ => ApplyTransformKind::ActorTransformOnly,
    };
    Some(RecorderRestoreDispatch {
        class_ordinal,
        selected_property_count,
        version_upgrade: VersionUpgradeKind::ActorTreePrimitiveToWood,
        restore_state,
        apply_transform,
    })
}

/// Outcome of the original dedicated server's by-name JSON load path
/// (`ADedicatedServerEntryGameMode::LoadProspectByName` `0x14146e6c0` ->
/// `TransitionToProspect` `0x1414831f0`) across missing, malformed, mismatched,
/// changed-during-load, and corrupt-blob inputs (F-733).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ByNameLoadOutcome {
    /// Rejected before `TransitionToProspect`; `ULocalSessionSubsystem` state
    /// is preserved unchanged.
    RejectedStatePreserved,
    /// `TransitionToProspect` called `ResetActiveProspect` (`0x1418333b0`) and
    /// `SetActiveProspectInfo` (`0x141837100`), then `ReadFromProspectBlob`
    /// (`0x1418e6240`) failed and aborted map travel, leaving
    /// `ULocalSessionSubsystem` with the new `ProspectInfo` and an empty
    /// `ProspectSaveState`.
    AbortedAfterActiveInfoOverwrite,
    /// Both JSON reads, `ReadFromProspectBlob`, and `SetActiveProspectSaveState`
    /// succeeded and `ServerTravel` runs (`mixed_reads: true` when the file
    /// changed between `GetProspectBlob` `0x141817bd0` and `GetProspectInfo`
    /// `0x14181e530`).
    TransitionedToMap { mixed_reads: bool },
}

/// Evaluates the original server's by-name JSON load failure matrix (F-733).
#[allow(clippy::too_many_arguments)]
pub fn evaluate_by_name_load_attempt(
    requested_name: &str,
    file_exists: bool,
    first_json_read_valid: bool,
    second_json_read_valid: bool,
    second_read_prospect_id: &[u16],
    file_changed_between_reads: bool,
    inner_blob_valid: bool,
    blob_prospect_id: &[u16],
) -> ByNameLoadOutcome {
    let Some(normalized) = normalize_prospect_name(requested_name) else {
        return ByNameLoadOutcome::RejectedStatePreserved;
    };
    if !file_exists || !first_json_read_valid || !second_json_read_valid {
        return ByNameLoadOutcome::RejectedStatePreserved;
    }
    let requested_utf16: Vec<u16> = normalized.encode_utf16().collect();
    if !prospect_ids_match(&requested_utf16, second_read_prospect_id) {
        return ByNameLoadOutcome::RejectedStatePreserved;
    }
    if !inner_blob_valid {
        return ByNameLoadOutcome::AbortedAfterActiveInfoOverwrite;
    }
    if !prospect_ids_match(second_read_prospect_id, blob_prospect_id) {
        return ByNameLoadOutcome::AbortedAfterActiveInfoOverwrite;
    }
    ByNameLoadOutcome::TransitionedToMap {
        mixed_reads: file_changed_between_reads,
    }
}

/// Outcome of `AIcarusGameModeSurvival::SetupLoadedProspect` (`0x1414baf60`),
/// called from `AIcarusGameModeSurvival::InitGame` (`0x1414acc40`) with the
/// `?LocalSavePath=` URL option (F-732, F-733).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SetupLoadedProspectOutcome {
    /// `LocalSavePath.Num() <= 1` (empty in all standard dedicated-server
    /// transitions): returns `false` immediately without touching state or
    /// reaching `0x141ee8930`.
    SkippedEmptyLocalSavePath,
    /// `LocalSavePath` was non-empty and either `LoadProspectSaveHeader`
    /// (`0x141824760`) or `LoadLocalProspectState` (`0x141823fb0`) returned
    /// `false`: logs Fatal `"Failed to load prospect from file: '%s'"`
    /// (`0x1449f6660`) and calls `0x141ee8930` (`GIsCriticalError = 1` ->
    /// `GError->Serialize` -> `RequestExit(true)` / `RaiseException`).
    FatalProcessExit,
    /// Both binary archive reads succeeded and activated the prospect in
    /// `ULocalSessionSubsystem`.
    ActivatedFromBinaryFile,
}

/// Evaluates `AIcarusGameModeSurvival::SetupLoadedProspect` (`0x1414baf60`)
/// (F-732, F-733).
pub fn evaluate_setup_loaded_prospect_attempt(
    local_save_path: &str,
    header_read_ok: bool,
    full_state_read_ok: bool,
) -> SetupLoadedProspectOutcome {
    if local_save_path.is_empty() {
        return SetupLoadedProspectOutcome::SkippedEmptyLocalSavePath;
    }
    if !header_read_ok || !full_state_read_ok {
        return SetupLoadedProspectOutcome::FatalProcessExit;
    }
    SetupLoadedProspectOutcome::ActivatedFromBinaryFile
}

/// `EDynamicItemProperties` enumerators (`0..=13`, with `14` reserved for
/// `MaxDynamicItemProperties` at `0x145e0c090` server / `0x145f214e0` client)
/// restored from `FInventorySlotSaveData::DynamicData` (`D01`) by
/// `UActorStateRecorderComponent::UpdateOwnerInventories` (`0x1418b8f20`)
/// (F-735).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum DynamicItemPropertyKind {
    AssociatedItemInventoryId = 0,
    AssociatedItemInventorySlot = 1,
    DynamicState = 2,
    GunCurrentMagSize = 3,
    CurrentAmmoType = 4,
    BuildingVariation = 5,
    Durability = 6,
    ItemableStack = 7,
    MillijoulesRemaining = 8,
    TransmutableUnits = 9,
    FillableStoredUnits = 10,
    FillableType = 11,
    DecayableCurrentSpoilTime = 12,
    InventoryContainerLinkedInventoryId = 13,
}

impl DynamicItemPropertyKind {
    /// Maps a saved `FDynamicDataSaveData::Index` (`int32`, truncated to `u8`
    /// in `UActorStateRecorderComponent::UpdateOwnerInventories` `0x1418b8f20`)
    /// to its `EDynamicItemProperties` variant, or `None` if `(index as u8)` is
    /// `>= 14` (`MaxDynamicItemProperties`) (F-735).
    pub const fn from_saved_index(index: i32) -> Option<Self> {
        match index as u8 {
            0 => Some(Self::AssociatedItemInventoryId),
            1 => Some(Self::AssociatedItemInventorySlot),
            2 => Some(Self::DynamicState),
            3 => Some(Self::GunCurrentMagSize),
            4 => Some(Self::CurrentAmmoType),
            5 => Some(Self::BuildingVariation),
            6 => Some(Self::Durability),
            7 => Some(Self::ItemableStack),
            8 => Some(Self::MillijoulesRemaining),
            9 => Some(Self::TransmutableUnits),
            10 => Some(Self::FillableStoredUnits),
            11 => Some(Self::FillableType),
            12 => Some(Self::DecayableCurrentSpoilTime),
            13 => Some(Self::InventoryContainerLinkedInventoryId),
            _ => None,
        }
    }
}

/// Legacy (`actor_state_recorder_version < 2`) gun-ammo upgrade plan evaluated
/// by `UActorStateRecorderComponent::PerformItemVersionUpgrades` (`0x14189a250`
/// server / `0x1418a3530` client) during `UpdateOwnerInventories` (`0x1418b8f20`)
/// (F-735).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LegacyGunAmmoUpgrade {
    /// Zero-based index (`current_ammo_type - 1`) into the ranged weapon's
    /// `AmmoTypes` array (`+0x30`, stride `0x18` B).
    pub ammo_type_index: usize,
    /// Stack count (`GunCurrentMagSize`) written to
    /// [`DynamicItemPropertyKind::ItemableStack`] (`7`) on the synthesized
    /// ammo `FItemData` placed at slot `0` of the gun's linked ammo inventory.
    pub ammo_stack_count: i32,
    /// Existing `InventoryContainerLinkedInventoryId` (`13`) when `!= -1`, or
    /// `None` when a new container inventory ID must be allocated and written
    /// back into the gun's `InventoryContainerLinkedInventoryId` (`13`)
    /// dynamic property.
    pub existing_linked_inventory_id: Option<i32>,
}

/// Evaluates `UActorStateRecorderComponent::PerformItemVersionUpgrades`
/// (`0x14189a250` server / `0x1418a3530` client) on a restored `FItemData`'s
/// `ItemDynamicData` (`+0x30`, stride `8` B: `(property_type, value)`) (F-735).
///
/// Runs only when `actor_state_recorder_version < 2` (`[this + 0xdc] < 2`),
/// `has_valid_inventory_container` (`+0x2e8`), and
/// `has_valid_ranged_weapon_data` (`+0x390`) are all true, and both
/// `CurrentAmmoType` (`4`) and `GunCurrentMagSize` (`3`) are `> 0`.
pub fn evaluate_legacy_gun_ammo_upgrade(
    actor_state_recorder_version: i32,
    has_valid_inventory_container: bool,
    has_valid_ranged_weapon_data: bool,
    dynamic_properties: &[(u8, i32)],
) -> Option<LegacyGunAmmoUpgrade> {
    if actor_state_recorder_version >= 2
        || !has_valid_inventory_container
        || !has_valid_ranged_weapon_data
    {
        return None;
    }
    let find_prop = |kind: DynamicItemPropertyKind| -> i32 {
        dynamic_properties
            .iter()
            .find(|(prop_type, _)| *prop_type == kind as u8)
            .map_or(-1, |(_, value)| *value)
    };
    let current_ammo_type = find_prop(DynamicItemPropertyKind::CurrentAmmoType);
    let gun_current_mag_size = find_prop(DynamicItemPropertyKind::GunCurrentMagSize);
    if current_ammo_type <= 0 || gun_current_mag_size <= 0 {
        return None;
    }
    let linked_inv_id = find_prop(DynamicItemPropertyKind::InventoryContainerLinkedInventoryId);
    Some(LegacyGunAmmoUpgrade {
        ammo_type_index: (current_ammo_type - 1) as usize,
        ammo_stack_count: gun_current_mag_size,
        existing_linked_inventory_id: (linked_inv_id != -1).then_some(linked_inv_id),
    })
}

/// How `UActorStateRecorderComponent::UpdateOwnerInventories` (`0x1418b8f20`)
/// routes a restored `D01` (`FInventorySlotSaveData`) item after
/// `UInventory::SetItem` (`0x14197a3a0`) (F-735).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InventorySlotOverflowDisposition {
    /// `UInventory::SetItem(Inventory, Location, ItemData)` returned `true`.
    PlacedInSlot,
    /// `SetItem` returned `false` and `Inventory->Slots.Num() == 0`: routed to
    /// `UInventory::ForceAddItems` (`0x14195bfc0`).
    ForceAddItemsWhenEmpty,
    /// `SetItem` returned `false` and `Inventory->Slots.Num() > 0`: appended to
    /// `Inventory + 0x288` (`0x1414c24e0`).
    AppendToDeferredOverflow,
}

/// Evaluates how `UActorStateRecorderComponent::UpdateOwnerInventories`
/// (`0x1418b8f20`) routes a restored `D01` slot after `UInventory::SetItem`
/// (F-735).
pub const fn resolve_inventory_slot_restore(
    set_item_accepted: bool,
    current_slot_count: usize,
) -> InventorySlotOverflowDisposition {
    if set_item_accepted {
        InventorySlotOverflowDisposition::PlacedInSlot
    } else if current_slot_count == 0 {
        InventorySlotOverflowDisposition::ForceAddItemsWhenEmpty
    } else {
        InventorySlotOverflowDisposition::AppendToDeferredOverflow
    }
}

/// Normalizes a saved `FModifierStateSaveData::ModifierLifetime` as
/// `UActorStateRecorderComponent::UpdateOwnerToDatabaseState` (`0x1418ba040`)
/// does before `AddModifierState`: a saved `-1` becomes `100`
/// ([`RESTORED_INDEFINITE_MODIFIER_LIFETIME`]) (F-735).
pub const fn normalize_restored_modifier_lifetime(saved_lifetime: i32) -> i32 {
    if saved_lifetime == -1 {
        RESTORED_INDEFINITE_MODIFIER_LIFETIME
    } else {
        saved_lifetime
    }
}

/// Evaluates whether `UActorStateRecorderComponent::UpdateOwnerToDatabaseState`
/// (`C001`, `0x1418b6ed0` server / `0x1418ba040` client) restores one saved
/// `FModifierStateSaveData` (`+0x150`, stride `0x18` B) and returns the
/// normalized lifetime passed to `AddModifierState` (`0x1414ce560` client)
/// (F-735).
///
/// Requires `b_record_modifier_states` (`+0xd8`), `modifier_effectiveness >= 0.0`,
/// `is_row_handle_valid` (`0x143f855d0`), and `row_saves_modifier_state`
/// (`[Row + 0x263] != 0`).
pub fn evaluate_modifier_state_restore(
    b_record_modifier_states: bool,
    modifier_effectiveness: f32,
    is_row_handle_valid: bool,
    row_saves_modifier_state: bool,
    saved_lifetime: i32,
) -> Option<i32> {
    (b_record_modifier_states
        && modifier_effectiveness >= 0.0
        && is_row_handle_valid
        && row_saves_modifier_state)
        .then(|| normalize_restored_modifier_lifetime(saved_lifetime))
}

/// Upserts a `(name, value)` entry into a recorded variable table as
/// `UActorStateRecorderComponent::RecordOwnerVariables` (`0x1418a90f0`) does
/// for `CPF_SaveGame` properties: updates the first entry whose key equals
/// `name` in place (returning `false`), or appends `(name, value)` when no
/// existing entry matches (returning `true`) (F-735).
pub fn upsert_recorded_variable<K: PartialEq, V>(
    entries: &mut Vec<(K, V)>,
    name: K,
    value: V,
) -> bool {
    if let Some((_, existing_value)) = entries.iter_mut().find(|(k, _)| *k == name) {
        *existing_value = value;
        false
    } else {
        entries.push((name, value));
        true
    }
}

/// Outcome of `FProspectSaveStateHeader::ReadFromProspectBlob` (`0x1418e6240`
/// server / `0x1418e7680` client) given the `FProspectBlob` header fields and
/// inner stream states (F-283, F-737).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProspectBlobDecodeOutcome {
    /// `TotalLength < 0` invokes `TArray::ResizeShrink` (`0x1408e7980`) on an
    /// empty array with a negative count before `FBase64::Decode` (`SL-003`).
    NegativeTotalLengthResizeError,
    /// `UncompressedLength < 0` skips `TArray::ResizeGrow` (`rcx = nullptr`)
    /// and calls `memset(nullptr, 0, UncompressedLength as usize)`, crashing
    /// immediately (`SL-003`).
    NegativeUncompressedLengthNullMemsetCrash,
    /// `FBase64::Decode` resizes `CompressedData` to
    /// `GetDecodedDataSize(BinaryBlob)` (`decoded_base64_bytes`), shrinking or
    /// freeing the pre-allocated `TotalLength` buffer when
    /// `decoded_base64_bytes < total_length`, and `ReadFromProspectBlob` then
    /// passes `CompressedData.GetData()` with `CompressedSize = TotalLength`
    /// to `FCompression::UncompressMemory`, reading out of bounds or
    /// dereferencing `nullptr` when `decoded_base64_bytes == 0` (`SL-004`).
    CompressedBufferUnderflowOutOfBoundsRead {
        allocated_bytes: usize,
        uncompress_read_bytes: usize,
    },
    /// `FBase64::Decode` returned `false` (`SL-001` ignored return value), or
    /// the compressed payload is corrupt or truncated, causing
    /// `FCompression::UncompressMemory(NAME_Zlib)` (`0x141f140f0`) to return
    /// `false` and `ReadFromProspectBlob` to return `false` (`SL-002`).
    ZlibUncompressRejected { base64_decode_ok: bool },
    /// `UncompressMemory` succeeded, but `FSHA1::HashBuffer` on the
    /// `UncompressedLength` buffer did not match `Blob.Hash` (`+0x10`), so
    /// `ReadFromProspectBlob` returns `false`.
    Sha1Mismatch,
    /// `UncompressMemory` and the `FSHA1` check succeeded, and
    /// `FProspectSaveState` archive deserialization (`0x1418ee4a0`) completed.
    /// `DataLength` (`+0x24`) is never read by `ReadFromProspectBlob`.
    DecodedSaveState { ignored_data_length: i32 },
}

/// Evaluates `FProspectSaveStateHeader::ReadFromProspectBlob` (`0x1418e6240`
/// server / `0x1418e7680` client) and `FBase64::Decode` (`0x141efc520` server /
/// `0x141ee8640` client) allocation, length, and error-handling behavior
/// (F-283, F-737).
pub const fn evaluate_prospect_blob_decode(
    total_length: i32,
    data_length: i32,
    uncompressed_length: i32,
    decoded_base64_bytes: usize,
    base64_decode_ok: bool,
    zlib_uncompress_ok: bool,
    sha1_matches: bool,
) -> ProspectBlobDecodeOutcome {
    if total_length < 0 {
        return ProspectBlobDecodeOutcome::NegativeTotalLengthResizeError;
    }
    if uncompressed_length < 0 {
        return ProspectBlobDecodeOutcome::NegativeUncompressedLengthNullMemsetCrash;
    }
    let uncompress_read_bytes = total_length as usize;
    if decoded_base64_bytes < uncompress_read_bytes {
        return ProspectBlobDecodeOutcome::CompressedBufferUnderflowOutOfBoundsRead {
            allocated_bytes: decoded_base64_bytes,
            uncompress_read_bytes,
        };
    }
    if !zlib_uncompress_ok {
        return ProspectBlobDecodeOutcome::ZlibUncompressRejected { base64_decode_ok };
    }
    if !sha1_matches {
        return ProspectBlobDecodeOutcome::Sha1Mismatch;
    }
    ProspectBlobDecodeOutcome::DecodedSaveState {
        ignored_data_length: data_length,
    }
}

/// Base actor fields recorded by `UActorStateRecorderComponent::RecordOwnerState`
/// (`C001`, `0x1418a0170` server / `0x1418a5b60` client, inherited by `C023`
/// `UOilGeyserRecorderComponent` and all `UActorStateRecorderComponent`
/// subclasses) after `UIcarusStateRecorderComponent::RecordOwnerState`
/// (`0x1418a0270` server / `0x1418a5c60` client) (F-650, F-735, F-744).
#[derive(Clone, Debug, PartialEq)]
pub struct ActorRecorderBaseRecordPlan<V, I, M> {
    /// `RecordedVersion` (`recorder + 0xdc`) set to `3`.
    pub recorded_version: i32,
    /// `ActorTransform` (`recorder + 0xe0..0x10c`): copied from
    /// `owner->RootComponent + 0x1c0` when `b_record_actor_transform`
    /// (`recorder + 0xd9`) is `true` and `RootComponent` (`+0x130`) is
    /// non-null, or `FTransform::Identity` when `b_record_actor_transform` is
    /// `true` and `RootComponent` is `nullptr`; `None` preserves existing
    /// `recorder + 0xe0..0x10c` when `b_record_actor_transform` is `false`.
    pub actor_transform_override: Option<([f32; 4], [f32; 3], [f32; 3])>,
    /// `RecordedVariables` (`recorder + 0x110`) updated via
    /// `RecordOwnerVariables` (`0x1418a3970` server / `0x1418a90f0` client).
    pub recorded_variables: Vec<V>,
    /// `DurabilityData` (`recorder + 0x120..0x139`:
    /// `(current_durability, max_durability, b_can_break)`) copied from the
    /// owner's `UDurabilityComponent` (`0x141b4e130` server / `0x141b43370`
    /// client) when present; `None` preserves existing `+0x120..0x139`.
    pub durability_data: Option<(i32, i32, bool)>,
    /// `IcarusActorGUID` (`recorder + 0x140`) written when the owner implements
    /// `IIcarusActorGuidInterface` (`0x141bae1c0` server / `0x141b9e950`
    /// client); `None` preserves existing `+0x140`.
    pub icarus_actor_guid_override: Option<i32>,
    /// `SavedInventories` (`recorder + 0x170`) populated by
    /// `RecordOwnerInventories` (`0x1418a21b0` server / `0x1418a7930` client).
    pub saved_inventories: Vec<I>,
    /// `ModifierStates` (`recorder + 0x150`, stride `0x18` B) cleared and
    /// populated from `owner`'s `UModifierStateComponent` (`0x141c6b7c0`
    /// server / `0x141c5d6a0` client) iff `b_record_modifier_states` (`+0xd8`)
    /// is `true` and the component is present; `None` preserves existing
    /// `+0x150`.
    pub modifier_states_override: Option<Vec<M>>,
    /// `bGeneratorActive` (`recorder + 0x162`) updated from
    /// `UGeneratorComponent + 0xd0` (`0x141b7a780` server / `0x141b6eec0`
    /// client) when present; `None` preserves existing `+0x162`.
    pub generator_active_override: Option<bool>,
    /// `UProcessingComponent` state (`recorder + 0x160..0x168`) updated from
    /// `UProcessingComponent` (`0x141d2a790` server / `0x141d1ddd0` client)
    /// when present; `None` preserves existing `+0x160..0x168`.
    pub processing_state_override: Option<RestoredProcessingState>,
}

/// Evaluates `UActorStateRecorderComponent::RecordOwnerState` (`C001`,
/// `0x1418a0170` server / `0x1418a5b60` client) (F-650, F-735, F-744).
///
/// Returns `None` when the owner is null or pending kill.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub fn evaluate_actor_recorder_base_record<V: Clone, I: Clone, M: Clone>(
    owner_is_valid_actor: bool,
    b_record_actor_transform: bool,
    root_component_transform: Option<([f32; 4], [f32; 3], [f32; 3])>,
    recorded_variables: &[V],
    durability_component: Option<(i32, i32, bool)>,
    owner_icarus_actor_guid: Option<i32>,
    saved_inventories: &[I],
    b_record_modifier_states: bool,
    modifier_state_component_entries: Option<&[M]>,
    generator_component_active: Option<bool>,
    processing_component_state: Option<RestoredProcessingState>,
) -> Option<ActorRecorderBaseRecordPlan<V, I, M>> {
    if !owner_is_valid_actor {
        return None;
    }
    let actor_transform_override = b_record_actor_transform.then(|| {
        root_component_transform.unwrap_or(([0.0, 0.0, 0.0, 1.0], [0.0, 0.0, 0.0], [1.0, 1.0, 1.0]))
    });
    let modifier_states_override = if b_record_modifier_states {
        modifier_state_component_entries.map(<[M]>::to_vec)
    } else {
        None
    };
    Some(ActorRecorderBaseRecordPlan {
        recorded_version: 3,
        actor_transform_override,
        recorded_variables: recorded_variables.to_vec(),
        durability_data: durability_component,
        icarus_actor_guid_override: owner_icarus_actor_guid,
        saved_inventories: saved_inventories.to_vec(),
        modifier_states_override,
        generator_active_override: generator_component_active,
        processing_state_override: processing_component_state,
    })
}

/// Values restored onto `AIcarusQuestManager` by
/// `UIcarusQuestManagerRecorderComponent::UpdateOwnerToDatabaseState` (`C018`,
/// `0x1418f3d30` server / `0x1418f4540` client) after
/// `UActorStateRecorderComponent::UpdateOwnerToDatabaseState` (`0x1418b6ed0`
/// server / `0x1418ba040` client) (F-650, F-653, F-663).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QuestManagerRestorePlan<R> {
    /// `FactionMission.RowName` (`recorder + 0x1c0 -> owner + 0x2c0`).
    pub faction_mission_row_name: R,
    /// `bMissionActive` (`recorder + 0x1e0 -> owner + 0x2ea` before
    /// `OnRep_MissionActive` `0x141cf53b0` client).
    pub b_mission_active: bool,
    /// `MissionDifficulty` (`(u8)(recorder + 0x1e4) -> owner + 0x2d8` before
    /// `OnRep_MissionDifficulty` `0x141cf53f0` client).
    pub mission_difficulty: u8,
    /// `MissionTimer` (`recorder + 0x1e8 -> owner + 0x330`).
    pub mission_timer: i32,
    /// `bTimerActive` (`recorder + 0x1e1 -> owner + 0x334`).
    pub b_timer_active: bool,
    /// `SelectedSeed.RowName` (`recorder + 0x1ec -> owner + 0x338`).
    pub selected_seed_row_name: R,
}

/// Evaluates `UIcarusQuestManagerRecorderComponent::UpdateOwnerToDatabaseState`
/// (`C018`, `0x1418f3d30` server / `0x1418f4540` client) (F-650, F-653, F-663).
///
/// Returns `None` when the owner is null or not an `AIcarusQuestManager`
/// (`0x141cfaa20` client; no `PendingKill` check is performed).
pub const fn evaluate_quest_manager_restore<R: Copy>(
    owner_is_quest_manager: bool,
    saved_faction_mission_row_name: R,
    saved_b_mission_active: bool,
    saved_mission_difficulty: u8,
    saved_mission_timer: i32,
    saved_b_timer_active: bool,
    saved_selected_seed_row_name: R,
) -> Option<QuestManagerRestorePlan<R>> {
    if !owner_is_quest_manager {
        return None;
    }
    Some(QuestManagerRestorePlan {
        faction_mission_row_name: saved_faction_mission_row_name,
        b_mission_active: saved_b_mission_active,
        mission_difficulty: saved_mission_difficulty,
        mission_timer: saved_mission_timer,
        b_timer_active: saved_b_timer_active,
        selected_seed_row_name: saved_selected_seed_row_name,
    })
}

/// Values restored onto `AIcarusQuest` by
/// `UIcarusQuestRecorderComponent::UpdateOwnerToDatabaseState` (`C019`,
/// `0x1418f44e0` server / `0x1418f4cf0` client) after
/// `UActorStateRecorderComponent::UpdateOwnerToDatabaseState` (`0x1418b6ed0`
/// server / `0x1418ba040` client) (F-650, F-663).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuestRestorePlan<N, P> {
    /// `QuestDTKey.RowName` (`recorder + 0x1c0 -> owner + 0x2c4`).
    pub quest_dt_key: N,
    /// `QuestProgressRecords` (`recorder + 0x1f8`, stride `0x20` B ->
    /// `owner + 0x310` before `AIcarusQuest::OnQuestStateLoaded` `0x141ce9fb0`
    /// client).
    pub quest_progress_records: Vec<P>,
}

/// Evaluates `UIcarusQuestRecorderComponent::UpdateOwnerToDatabaseState`
/// (`C019`, `0x1418f44e0` server / `0x1418f4cf0` client) (F-650, F-663).
///
/// Returns `None` when the owner is null or not an `AIcarusQuest`
/// (`0x141cf3950` client; no `PendingKill` check is performed).
pub fn evaluate_quest_restore<N, P: Clone>(
    owner_is_quest: bool,
    saved_quest_dt_key: N,
    saved_quest_progress_records: &[P],
) -> Option<QuestRestorePlan<N, P>> {
    if !owner_is_quest {
        return None;
    }
    Some(QuestRestorePlan {
        quest_dt_key: saved_quest_dt_key,
        quest_progress_records: saved_quest_progress_records.to_vec(),
    })
}

/// `AIcarusGameStateSurvival` fields recorded by
/// `UIcarusGameModeStateRecorderComponent::RecordOwnerState` (`C015`,
/// `0x1418a1e10` server / `0x1418a7590` client) when `owner->GameState`
/// (`+0x280`) is a valid non-pending-kill `AIcarusGameStateSurvival`
/// (`0x141bc9a50` client) (F-650, F-653).
#[derive(Clone, Debug, PartialEq)]
pub struct GameModeGameStateRecord<L> {
    /// `TimeOfDay` (`GameState + 0x50c -> recorder + 0x1c4`).
    pub time_of_day: f32,
    /// `LastValidTimeOfDay` (`GameState + 0x518 -> recorder + 0x1cc`).
    pub last_valid_time_of_day: f32,
    /// `SpawnedDropshipsCount` (`GameState + 0x3a8 -> recorder + 0x1d0`).
    pub spawned_dropships_count: i32,
    /// `CurrentDropshipIndex` (`GameState + 0x3ac -> recorder + 0x1d4`).
    pub current_dropship_index: i32,
    /// `DropshipLocations` (`GameState + 0x3b8 -> recorder + 0x1e0`).
    pub dropship_locations: Vec<L>,
}

/// Values recorded by `UIcarusGameModeStateRecorderComponent::RecordOwnerState`
/// (`C015`, `0x1418a1e10` server / `0x1418a7590` client) after
/// `UActorStateRecorderComponent::RecordOwnerState` (`0x1418a0170` server /
/// `0x1418a5b60` client) (F-650, F-653).
#[derive(Clone, Debug, PartialEq)]
pub struct GameModeRecordPlan<L, C, S, A, B, M, D, F> {
    /// `RecordedVersion` (`recorder + 0x2a4`) set to `2`.
    pub recorded_version: i32,
    /// `LevelTimeElapsedSec` (`owner + 0x628 -> recorder + 0x1c0`).
    pub level_time_elapsed_sec: i32,
    /// `ProspectTimeElapsed` (`owner + 0x610 -> recorder + 0x1c8`).
    pub prospect_time_elapsed: f32,
    /// `bHaveDropshipsSpawned` (`owner + 0x62c -> recorder + 0x1f0`).
    pub b_have_dropships_spawned: bool,
    /// `GameState` fields (`recorder + 0x1c4`, `+0x1cc`, `+0x1d0`, `+0x1d4`,
    /// `+0x1e0`) written iff `owner->GameState` (`+0x280`) is a valid
    /// `AIcarusGameStateSurvival`; `None` preserves existing recorder fields.
    pub game_state_record: Option<GameModeGameStateRecord<L>>,
    /// `ExportedCurrencies` (`owner + 0x670 -> recorder + 0x200`, stride `0x0c` B).
    pub exported_currencies: Vec<(C, i32)>,
    /// `ProspectStats` (`owner + 0x450 -> recorder + 0x210`).
    pub prospect_stats: S,
    /// `AccoladeState` (`owner + 0x4c0 -> recorder + 0x260`).
    pub accolade_state: A,
    /// `BestiaryTracking` (`owner + 0x658 -> recorder + 0x270`).
    pub bestiary_tracking: B,
    /// `MissionHistory` (`owner + 0x6c0 -> recorder + 0x280`).
    pub mission_history: Vec<M>,
    /// `DialogueHistory` (`owner + 0x6d0 -> recorder + 0x290`).
    pub dialogue_history: Vec<D>,
    /// `NextMeteorShowerTime` (`owner + 0x6ec -> recorder + 0x2a0`).
    pub next_meteor_shower_time: i32,
    /// `FactionMissionBossesSpawned` (`owner + 0x698 -> recorder + 0x2a8`).
    pub faction_mission_bosses_spawned: Vec<F>,
}

/// Evaluates `UIcarusGameModeStateRecorderComponent::RecordOwnerState` (`C015`,
/// `0x1418a1e10` server / `0x1418a7590` client) (F-650, F-653).
///
/// Returns `None` when the owner is null, not an `AIcarusGameModeSurvival`
/// (`0x141bc8d40` client), or pending kill.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub fn evaluate_game_mode_record<L: Clone, C: Clone, S, A, B, M: Clone, D: Clone, F: Clone>(
    owner_is_valid_game_mode_survival: bool,
    level_time_elapsed_sec: i32,
    prospect_time_elapsed: f32,
    b_have_dropships_spawned: bool,
    valid_game_state_fields: Option<(f32, f32, i32, i32, &[L])>,
    exported_currencies: &[(C, i32)],
    prospect_stats: S,
    accolade_state: A,
    bestiary_tracking: B,
    mission_history: &[M],
    dialogue_history: &[D],
    next_meteor_shower_time: i32,
    faction_mission_bosses_spawned: &[F],
) -> Option<GameModeRecordPlan<L, C, S, A, B, M, D, F>> {
    if !owner_is_valid_game_mode_survival {
        return None;
    }
    let game_state_record = valid_game_state_fields.map(
        |(
            time_of_day,
            last_valid_time_of_day,
            spawned_dropships_count,
            current_dropship_index,
            dropship_locations,
        )| GameModeGameStateRecord {
            time_of_day,
            last_valid_time_of_day,
            spawned_dropships_count,
            current_dropship_index,
            dropship_locations: dropship_locations.to_vec(),
        },
    );
    Some(GameModeRecordPlan {
        recorded_version: 2,
        level_time_elapsed_sec,
        prospect_time_elapsed,
        b_have_dropships_spawned,
        game_state_record,
        exported_currencies: exported_currencies.to_vec(),
        prospect_stats,
        accolade_state,
        bestiary_tracking,
        mission_history: mission_history.to_vec(),
        dialogue_history: dialogue_history.to_vec(),
        next_meteor_shower_time,
        faction_mission_bosses_spawned: faction_mission_bosses_spawned.to_vec(),
    })
}

/// Values recorded by `UIcarusMountCharacterRecorderComponent::RecordOwnerState`
/// (`C017`, `0x1413fd880` server / `0x14143b680` client) and its base classes
/// `UIcarusNPCRecorderComponent::RecordOwnerState` (`0x1413cc9b0` server /
/// `0x14140b700` client) and `UIcarusCharacterRecorderComponent::RecordOwnerState`
/// (`0x14143d900` server / `0x14147bc20` client) after
/// `UActorStateRecorderComponent::RecordOwnerState` (`0x1418a0170` server /
/// `0x1418a5b60` client) (F-650, F-651).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MountCharacterRecordPlan<R, T, N, U> {
    /// `Health` (`recorder + 0x1c0`) written from `StatComponent->GetHealth()`
    /// iff `owner->StatComponent` (`+0x5a8`) is valid; `None` preserves
    /// existing `+0x1c0`.
    pub health_override: Option<i32>,
    /// `ParentCharacterUID` (`recorder + 0x1c4`): `ParentCharacter->IcarusActorGUID`
    /// (`+0x564`) when `owner->ParentCharacter` (`+0x738`) is valid, else `-1`.
    pub parent_character_uid: i32,
    /// `BehaviourType` (`AIcarusCharacter + 0x730 -> recorder + 0x1c8`).
    pub behaviour_type: u8,
    /// `TamedMountAction` (`AIcarusCharacter + 0x731 -> recorder + 0x1c9`).
    pub tamed_mount_action: u8,
    /// `SavedVersion` (`recorder + 0x260`) set to `2`.
    pub saved_version: i32,
    /// `GrowthStage` (`AIcarusNPCCharacter + 0x840 -> recorder + 0x1cc`).
    pub growth_stage: u8,
    /// `Experience` (`AIcarusNPCCharacter + 0x838 -> recorder + 0x1d0`).
    pub experience: i32,
    /// `CharacterLevel` (`AIcarusNPCCharacter + 0x83c -> recorder + 0x1d4`).
    pub character_level: i32,
    /// `AISetupRowName` (`AIcarusNPCCharacter + 0x81c -> recorder + 0x1d8`).
    pub ai_setup_row_name: R,
    /// `CurrentFood` (`AIcarusNPCCharacter + 0x928 -> recorder + 0x1e4`).
    pub current_food: i32,
    /// `CurrentWater` (`AIcarusNPCCharacter + 0x92c -> recorder + 0x1e8`).
    pub current_water: i32,
    /// `TamedData` (`AIcarusNPCCharacter + 0x848 -> recorder + 0x1f0..0x220`)
    /// copied when `TamedData` is active; `None` preserves existing
    /// `+0x1f0..0x220`.
    pub tamed_data_override: Option<T>,
    /// `SelectedTalentRowNames` (`AIcarusNPCCharacter + 0x988 -> recorder + 0x228`).
    pub selected_talent_row_names: Vec<R>,
    /// `SoloTalentRowNames` (`AIcarusNPCCharacter + 0x998 -> recorder + 0x238`).
    pub solo_talent_row_names: Vec<R>,
    /// `UnlockedCosmeticRowNames` (`AIcarusNPCCharacter + 0x9a8 -> recorder + 0x248`).
    pub unlocked_cosmetic_row_names: Vec<R>,
    /// `MountName` (`AIcarusMountCharacter + 0x9f0 -> recorder + 0x268`).
    pub mount_name: N,
    /// `MountTypeRowName` (`AIcarusMountCharacter + 0xa00 -> recorder + 0x278`).
    pub mount_type_row_name: R,
    /// `MountIconRowName` (`AIcarusMountCharacter + 0xa18 -> recorder + 0x280`).
    pub mount_icon_row_name: R,
    /// `OwnerUserID` (`AIcarusMountCharacter + 0xa30 -> recorder + 0x288`).
    pub owner_user_id: U,
}

/// Evaluates `UIcarusMountCharacterRecorderComponent::RecordOwnerState` (`C017`,
/// `0x1413fd880` server / `0x14143b680` client) and its NPC/Character base
/// layers (F-650, F-651).
///
/// Returns `None` when the owner is null, not an `AIcarusMountCharacter`
/// (`0x141be4450` server / `0x141bd7910` client), or pending kill.
#[allow(clippy::too_many_arguments)]
pub fn evaluate_mount_character_record<R: Clone, T, N, U>(
    owner_is_valid_mount_character: bool,
    stat_component_health: Option<i32>,
    valid_parent_character_guid: Option<i32>,
    behaviour_type: u8,
    tamed_mount_action: u8,
    growth_stage: u8,
    experience: i32,
    character_level: i32,
    ai_setup_row_name: R,
    current_food: i32,
    current_water: i32,
    tamed_data_override: Option<T>,
    selected_talent_row_names: &[R],
    solo_talent_row_names: &[R],
    unlocked_cosmetic_row_names: &[R],
    mount_name: N,
    mount_type_row_name: R,
    mount_icon_row_name: R,
    owner_user_id: U,
) -> Option<MountCharacterRecordPlan<R, T, N, U>> {
    if !owner_is_valid_mount_character {
        return None;
    }
    Some(MountCharacterRecordPlan {
        health_override: stat_component_health,
        parent_character_uid: valid_parent_character_guid.unwrap_or(-1),
        behaviour_type,
        tamed_mount_action,
        saved_version: 2,
        growth_stage,
        experience,
        character_level,
        ai_setup_row_name,
        current_food,
        current_water,
        tamed_data_override,
        selected_talent_row_names: selected_talent_row_names.to_vec(),
        solo_talent_row_names: solo_talent_row_names.to_vec(),
        unlocked_cosmetic_row_names: unlocked_cosmetic_row_names.to_vec(),
        mount_name,
        mount_type_row_name,
        mount_icon_row_name,
        owner_user_id,
    })
}

/// Values and actions restored onto `AIcarusMountCharacter` by
/// `UIcarusMountCharacterRecorderComponent::UpdateOwnerToDatabaseState` (`C017`,
/// `0x14140c8f0` server / `0x141449c30` client) and its base classes
/// `UIcarusNPCRecorderComponent::UpdateOwnerToDatabaseState` (`0x1413e0710`
/// server / `0x14141e250` client) and
/// `UIcarusCharacterRecorderComponent::UpdateOwnerToDatabaseState` (`0x14144c1b0`
/// server / `0x14148a880` client) after
/// `UActorStateRecorderComponent::UpdateOwnerToDatabaseState` (`0x1418b6ed0`
/// server / `0x1418ba040` client) (F-651).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MountCharacterRestorePlan<R, T, N, U> {
    /// Health applied via `StatComponent->SetHealth(saved_health)` (`0x141444cd0`
    /// server) iff `has_valid_stat_component` is `true` and `saved_health`
    /// (`recorder + 0x1c0`) is not `-1` ([`UNRECORDED_LEVEL`]).
    pub set_health: Option<i32>,
    /// `BehaviourType` (`recorder + 0x1c8 -> AIcarusCharacter + 0x730`).
    pub behaviour_type: u8,
    /// `TamedMountAction` (`recorder + 0x1c9 -> AIcarusCharacter + 0x731`).
    pub tamed_mount_action: u8,
    /// `GrowthStage` (`recorder + 0x1cc -> AIcarusNPCCharacter + 0x840`).
    pub growth_stage: u8,
    /// `Experience` (`recorder + 0x1d0 -> AIcarusNPCCharacter + 0x838`).
    pub experience: i32,
    /// `CharacterLevel` (`recorder + 0x1d4 -> AIcarusNPCCharacter + 0x83c`).
    pub character_level: i32,
    /// `AISetupRowName` (`recorder + 0x1d8 -> AIcarusNPCCharacter + 0x81c`).
    pub ai_setup_row_name: R,
    /// `CurrentFood` (`recorder + 0x1e4 -> AIcarusNPCCharacter + 0x928`).
    pub current_food: i32,
    /// `CurrentWater` (`recorder + 0x1e8 -> AIcarusNPCCharacter + 0x92c`).
    pub current_water: i32,
    /// `TamedData` (`recorder + 0x1f0..0x220 -> AIcarusNPCCharacter + 0x848`).
    pub tamed_data: T,
    /// `SelectedTalentRowNames` (`recorder + 0x228 -> AIcarusNPCCharacter + 0x988`).
    pub selected_talent_row_names: Vec<R>,
    /// `SoloTalentRowNames` (`recorder + 0x238 -> AIcarusNPCCharacter + 0x998`).
    pub solo_talent_row_names: Vec<R>,
    /// `UnlockedCosmeticRowNames` (`recorder + 0x248 -> AIcarusNPCCharacter + 0x9a8`).
    pub unlocked_cosmetic_row_names: Vec<R>,
    /// Whether `UIcarusNPCRecorderComponent::MigrateLegacyTalents` (`0x1413cc7f0`
    /// server / `0x14140b3e0` client) runs (`saved_version < 2` at
    /// `recorder + 0x260`).
    pub run_legacy_talent_migration: bool,
    /// `MountName` (`recorder + 0x268 -> AIcarusMountCharacter + 0x9f0`).
    pub mount_name: N,
    /// `MountTypeRowName` (`recorder + 0x278 -> AIcarusMountCharacter + 0xa00`).
    pub mount_type_row_name: R,
    /// `MountIconRowName` (`recorder + 0x280 -> AIcarusMountCharacter + 0xa18`).
    pub mount_icon_row_name: R,
    /// `OwnerUserID` (`recorder + 0x288 -> AIcarusMountCharacter + 0xa30`).
    pub owner_user_id: U,
}

/// Evaluates `UIcarusMountCharacterRecorderComponent::UpdateOwnerToDatabaseState`
/// (`C017`, `0x14140c8f0` server / `0x141449c30` client) and its NPC/Character
/// base layers (F-651).
///
/// Returns `None` when the owner is null, not an `AIcarusMountCharacter`
/// (`0x141be4450` server / `0x141bd7910` client), or pending kill.
#[allow(clippy::too_many_arguments)]
pub fn evaluate_mount_character_restore<R: Clone, T, N, U>(
    owner_is_valid_mount_character: bool,
    has_valid_stat_component: bool,
    saved_health: i32,
    saved_behaviour_type: u8,
    saved_tamed_mount_action: u8,
    saved_version: i32,
    saved_growth_stage: u8,
    saved_experience: i32,
    saved_character_level: i32,
    saved_ai_setup_row_name: R,
    saved_current_food: i32,
    saved_current_water: i32,
    saved_tamed_data: T,
    saved_selected_talent_row_names: &[R],
    saved_solo_talent_row_names: &[R],
    saved_unlocked_cosmetic_row_names: &[R],
    saved_mount_name: N,
    saved_mount_type_row_name: R,
    saved_mount_icon_row_name: R,
    saved_owner_user_id: U,
) -> Option<MountCharacterRestorePlan<R, T, N, U>> {
    if !owner_is_valid_mount_character {
        return None;
    }
    let set_health = if has_valid_stat_component {
        recorded_level(saved_health)
    } else {
        None
    };
    Some(MountCharacterRestorePlan {
        set_health,
        behaviour_type: saved_behaviour_type,
        tamed_mount_action: saved_tamed_mount_action,
        growth_stage: saved_growth_stage,
        experience: saved_experience,
        character_level: saved_character_level,
        ai_setup_row_name: saved_ai_setup_row_name,
        current_food: saved_current_food,
        current_water: saved_current_water,
        tamed_data: saved_tamed_data,
        selected_talent_row_names: saved_selected_talent_row_names.to_vec(),
        solo_talent_row_names: saved_solo_talent_row_names.to_vec(),
        unlocked_cosmetic_row_names: saved_unlocked_cosmetic_row_names.to_vec(),
        run_legacy_talent_migration: saved_version < 2,
        mount_name: saved_mount_name,
        mount_type_row_name: saved_mount_type_row_name,
        mount_icon_row_name: saved_mount_icon_row_name,
        owner_user_id: saved_owner_user_id,
    })
}

/// Which `RequiresStitching` (`vftable + 0x470`) implementation a
/// `StateRecorderBlobs` class (`C001`..`C038`) dispatches (F-746).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RequiresStitchingKind {
    /// Base `UIcarusStateRecorderComponent` / `UActorStateRecorderComponent`
    /// `0x14089a160` server / `0x1408cb520` client (`xor al, al; ret`).
    AlwaysFalse,
    /// `0x140899f70` server / `0x1408c9810` client (`mov al, 1; ret`).
    AlwaysTrue,
    /// `UIcarusCharacterRecorderComponent::RequiresStitching` (`0x1413e1110`
    /// server / `0x14141ec50` client, inherited by `C017`
    /// `UIcarusMountCharacterRecorderComponent`): returns
    /// `ParentCharacterUID != -1` (`[this + 0x1c4] != -1`).
    CharacterParentUidPresent,
}

/// Which `MakeRecorderTransformRelative` (`vftable + 0x4a8`) and
/// `TransformFromRelative` (`vftable + 0x4b0`) implementation pair a
/// `StateRecorderBlobs` class (`C001`..`C038`) dispatches (F-746).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MakeRelativeTransformKind {
    /// Direct `UIcarusStateRecorderComponent` subclass (`C013`, `C014`, `C022`,
    /// `C025`, `C035`, `C036`, `C037`), where primary `vftable + 0x4a8` is the
    /// secondary `IIcarusPooledComponentInterface` vtable header rather than a
    /// callable method.
    NotApplicable,
    /// `UActorStateRecorderComponent::MakeRecorderTransformRelative`
    /// (`0x14189a760` server / `0x1418a1b90` client; `TransformFromRelative`
    /// `0x141887d20` server / `0x141890ec0` client): converts
    /// `ActorTransform` (`+0xe0`) to/from relative space.
    ActorTransformOnly,
    /// `UBuildingGridRecorderComponent::MakeRecorderTransformRelative` (`C004`,
    /// `0x14189a7d0` server / `0x1418a1c00` client; `TransformFromRelative`
    /// `0x141888270` server / `0x141891410` client): converts `ActorTransform`
    /// (`+0xe0`) and `BuildingGridRecord` (`+0x1c0`) to/from relative space.
    ActorAndBuildingGrid,
    /// `USplineRecorderComponent::MakeRecorderTransformRelative` (`C032`,
    /// `0x1418ddbf0` server / `0x1418df8c0` client; `TransformFromRelative`
    /// `0x1418c5cd0` server / `0x1418c7970` client): converts `ActorTransform`
    /// (`+0xe0`) and each `FRecordedSplinePoint` in
    /// `SplineNetworkSaveData.Points` (`+0x238`, stride `0x50`) to/from
    /// relative space.
    ActorAndSplinePoints,
}

/// Constructor defaults (`+0xb0`, `+0xb1`, `+0xb2`, `+0xd8`, `+0xd9`) and
/// virtual-slot `+0x470` (`RequiresStitching`), `+0x498`
/// (`PurgeActorsMovedToSubLevel`: `0x14189ea40` server / `0x1418a4500` client
/// on `C004` vs base no-op `0x14189eae0` / `0x1418a45a0`), and `+0x4a8`/`+0x4b0`
/// (`MakeRecorderTransformRelative` / `TransformFromRelative`) dispatch for one
/// `StateRecorderBlobs` class ordinal `1..=38` (`C001`..`C038`), identical in
/// server `25621760` and client `25793767` (F-746).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RecorderCtorDefaults {
    pub class_ordinal: u8,
    /// `true` for the 31 `UActorStateRecorderComponent` classes (`C001` and its
    /// subclasses); `false` for the 7 direct `UIcarusStateRecorderComponent`
    /// classes (`C013`, `C014`, `C022`, `C025`, `C035`, `C036`, `C037`).
    pub is_actor_recorder: bool,
    /// `UIcarusStateRecorderComponent::bSaveModifiers` (`+0xb0`): default `1`
    /// in base ctor (`0x1418824d0` server / `0x14188bea0` client), overridden
    /// to `0` on 13 classes (`C003, C007..C010, C020, C021, C025, C026, C029,
    /// C030, C033, C034`).
    pub b_save_modifiers: bool,
    /// `UIcarusStateRecorderComponent::bSkipDefaultOwnerResolution` (`+0xb1`):
    /// default `0` in base ctor, overridden to `1` on 6 classes (`C015, C020,
    /// C025, C026, C031, C034`).
    pub b_skip_default_owner_resolution: bool,
    /// `UIcarusStateRecorderComponent::OwnerResolvePolicy` (`+0xb2`): default
    /// `FindOrRespawn` (`2`) in base ctor (`13` `FindOnly`, `15` `RespawnOnly`,
    /// `4` `FindOrRespawn`, `6` `ManuallyResolved`).
    pub owner_resolve_policy: OwnerResolvePolicy,
    /// `UActorStateRecorderComponent::bRecordModifierStates` (`+0xd8`): default
    /// `0` in `UActorStateRecorderComponent` ctor (`0x1419fe870` server /
    /// `0x1419f5880` client), overridden to `1` only on `C026`; `None` on the 7
    /// direct `UIcarusStateRecorderComponent` classes.
    pub b_record_modifier_states: Option<bool>,
    /// `UActorStateRecorderComponent::bRecordActorTransform` (`+0xd9`): default
    /// `1` in `UActorStateRecorderComponent` ctor, overridden to `0` on 5
    /// classes (`C006, C020, C028, C031, C034`); `None` on the 7 direct
    /// `UIcarusStateRecorderComponent` classes.
    pub b_record_actor_transform: Option<bool>,
    /// `RequiresStitching` (`vftable + 0x470`).
    pub requires_stitching: RequiresStitchingKind,
    /// `PurgeActorsMovedToSubLevel` (`vftable + 0x498`): `true` only on `C004`
    /// (`0x14189ea40` server / `0x1418a4500` client vs base no-op `0x14189eae0`
    /// / `0x1418a45a0` on the other 30 `C001` classes).
    pub purge_sublevel_actors: bool,
    /// `MakeRecorderTransformRelative` / `TransformFromRelative`
    /// (`vftable + 0x4a8` / `+0x4b0`).
    pub make_relative_transform: MakeRelativeTransformKind,
}

/// Returns the constructor defaults and virtual-slot `+0x470` / `+0x498` /
/// `+0x4a8` profile for `class_ordinal` in `1..=38` (`C001`..`C038`), or
/// `None` if `class_ordinal` is outside `1..=38` (F-746).
pub const fn recorder_ctor_defaults(class_ordinal: u8) -> Option<RecorderCtorDefaults> {
    if class_ordinal < 1 || class_ordinal > 38 {
        return None;
    }
    let is_actor_recorder = !matches!(class_ordinal, 13 | 14 | 22 | 25 | 35 | 36 | 37);
    let b_save_modifiers = !matches!(
        class_ordinal,
        3 | 7 | 8 | 9 | 10 | 20 | 21 | 25 | 26 | 29 | 30 | 33 | 34
    );
    let b_skip_default_owner_resolution = matches!(class_ordinal, 15 | 20 | 25 | 26 | 31 | 34);
    let owner_resolve_policy = match class_ordinal {
        2 | 12 | 13 | 15 | 16 | 18 | 22 | 23 | 24 | 35 | 36 | 37 | 38 => {
            OwnerResolvePolicy::FindOnly
        }
        3 | 4 | 7 | 8 | 9 | 10 | 11 | 14 | 17 | 21 | 29 | 30 | 31 | 32 | 33 => {
            OwnerResolvePolicy::RespawnOnly
        }
        1 | 19 | 27 | 28 => OwnerResolvePolicy::FindOrRespawn,
        _ => OwnerResolvePolicy::ManuallyResolved,
    };
    let (b_record_modifier_states, b_record_actor_transform, make_relative_transform) =
        if is_actor_recorder {
            let record_modifiers = class_ordinal == 26;
            let record_transform = !matches!(class_ordinal, 6 | 20 | 28 | 31 | 34);
            let make_relative = match class_ordinal {
                4 => MakeRelativeTransformKind::ActorAndBuildingGrid,
                32 => MakeRelativeTransformKind::ActorAndSplinePoints,
                _ => MakeRelativeTransformKind::ActorTransformOnly,
            };
            (
                Some(record_modifiers),
                Some(record_transform),
                make_relative,
            )
        } else {
            (None, None, MakeRelativeTransformKind::NotApplicable)
        };
    let requires_stitching = match class_ordinal {
        17 => RequiresStitchingKind::CharacterParentUidPresent,
        3 | 6 | 7 | 8 | 9 | 10 | 13 | 14 | 16 | 18 | 19 | 25 | 26 | 27 | 30 | 32 | 38 => {
            RequiresStitchingKind::AlwaysTrue
        }
        _ => RequiresStitchingKind::AlwaysFalse,
    };
    let purge_sublevel_actors = class_ordinal == 4;
    Some(RecorderCtorDefaults {
        class_ordinal,
        is_actor_recorder,
        b_save_modifiers,
        b_skip_default_owner_resolution,
        owner_resolve_policy,
        b_record_modifier_states,
        b_record_actor_transform,
        requires_stitching,
        purge_sublevel_actors,
        make_relative_transform,
    })
}

/// Evaluates `RequiresStitching` (`vftable + 0x470`) given a class's
/// [`RequiresStitchingKind`] and saved `ParentCharacterUID` (`[this + 0x1c4]`)
/// (F-746).
pub const fn evaluate_requires_stitching(
    kind: RequiresStitchingKind,
    parent_character_uid: i32,
) -> bool {
    match kind {
        RequiresStitchingKind::AlwaysFalse => false,
        RequiresStitchingKind::AlwaysTrue => true,
        RequiresStitchingKind::CharacterParentUidPresent => parent_character_uid != -1,
    }
}

const SELECTED_PROPERTY_COUNTS_N001_TO_N027: [u8; 27] = [
    31, 1, 21, 32, 21, 21, 33, 21, 29, 23, 46, 36, 40, 36, 4, 37, 32, 0, 32, 23, 22, 22, 21, 20,
    23, 22, 21,
];

/// Returns the static restore dispatch profile for one of the 27 non-blob
/// `/Script/Icarus.*RecorderComponent` classes (`N001`..`N027`, ordinal
/// `1..=27`), or `None` if `non_blob_ordinal` is outside `1..=27` (F-747).
pub const fn non_blob_recorder_restore_dispatch(
    non_blob_ordinal: u8,
) -> Option<RecorderRestoreDispatch> {
    if non_blob_ordinal < 1 || non_blob_ordinal > 27 {
        return None;
    }
    let selected_property_count =
        SELECTED_PROPERTY_COUNTS_N001_TO_N027[(non_blob_ordinal - 1) as usize];
    let is_direct_base_or_base = matches!(non_blob_ordinal, 2 | 15 | 18);
    if is_direct_base_or_base {
        return Some(RecorderRestoreDispatch {
            class_ordinal: non_blob_ordinal,
            selected_property_count,
            version_upgrade: VersionUpgradeKind::NoOp,
            restore_state: RestoreStateKind::BaseSerializeOnly,
            apply_transform: ApplyTransformKind::NoOpTrue,
        });
    }
    let restore_state = if non_blob_ordinal == 11 {
        RestoreStateKind::MountActorUprightYaw
    } else {
        RestoreStateKind::ActorWithContextTransform
    };
    Some(RecorderRestoreDispatch {
        class_ordinal: non_blob_ordinal,
        selected_property_count,
        version_upgrade: VersionUpgradeKind::ActorTreePrimitiveToWood,
        restore_state,
        apply_transform: ApplyTransformKind::ActorTransformOnly,
    })
}

/// Returns the constructor defaults (`+0xb0`, `+0xb1`, `+0xb2`, `+0xd8`,
/// `+0xd9`) and virtual-slot `+0x470` / `+0x498` / `+0x4a8` profile for one of
/// the 27 non-blob `/Script/Icarus.*RecorderComponent` classes (`N001`..`N027`,
/// ordinal `1..=27`), identical in server `25621760` and client `25793767`, or
/// `None` if `non_blob_ordinal` is outside `1..=27` (F-747).
pub const fn non_blob_recorder_ctor_defaults(non_blob_ordinal: u8) -> Option<RecorderCtorDefaults> {
    if non_blob_ordinal < 1 || non_blob_ordinal > 27 {
        return None;
    }
    let is_actor_recorder = !matches!(non_blob_ordinal, 2 | 15 | 18);
    let b_save_modifiers = !matches!(
        non_blob_ordinal,
        1 | 4 | 7 | 9 | 12 | 17 | 18 | 19 | 21 | 22
    );
    let b_skip_default_owner_resolution = non_blob_ordinal == 21;
    let owner_resolve_policy = match non_blob_ordinal {
        2 | 3 | 6 | 22 | 23 => OwnerResolvePolicy::FindOnly,
        1 | 4 | 7 | 9 | 10..=14 | 16 | 17 | 19 | 24 => OwnerResolvePolicy::RespawnOnly,
        18 => OwnerResolvePolicy::ManuallyResolved,
        _ => OwnerResolvePolicy::FindOrRespawn,
    };
    let (b_record_modifier_states, b_record_actor_transform, make_relative_transform) =
        if is_actor_recorder {
            (
                Some(false),
                Some(non_blob_ordinal != 20),
                MakeRelativeTransformKind::ActorTransformOnly,
            )
        } else {
            (None, None, MakeRelativeTransformKind::NotApplicable)
        };
    let requires_stitching = match non_blob_ordinal {
        10..=14 => RequiresStitchingKind::CharacterParentUidPresent,
        1 | 3 | 4 | 7 | 8 | 16 | 17 | 19 | 24 | 25 | 26 | 27 => RequiresStitchingKind::AlwaysTrue,
        _ => RequiresStitchingKind::AlwaysFalse,
    };
    Some(RecorderCtorDefaults {
        class_ordinal: non_blob_ordinal,
        is_actor_recorder,
        b_save_modifiers,
        b_skip_default_owner_resolution,
        owner_resolve_policy,
        b_record_modifier_states,
        b_record_actor_transform,
        requires_stitching,
        purge_sublevel_actors: false,
        make_relative_transform,
    })
}

/// Evaluates `UNoRecorderComponent::Validate` (`N018`, `vftable + 0x408`:
/// `0x14089a160` server / `0x1408cb520` client, `xor al, al; ret`), which
/// unconditionally returns `false` so any `UNoRecorderComponent` entry in
/// `StateRecorderBlobs` is rejected and dropped during `ReadFromProspectBlob`
/// (F-747).
pub const fn no_recorder_record_valid() -> bool {
    false
}

/// Evaluates `UIcarusStateRecorderComponent::HasValidOwner` (`0x141898750`
/// server / `0x14189ff30` client).
///
/// Returns `true` iff `Outer` (`+0x20`) is present, not pending kill, and not a
/// `UIcarusGameStateRecorder` instance.
pub const fn has_valid_recorder_owner(
    outer_present: bool,
    outer_pending_kill: bool,
    outer_is_game_state_recorder: bool,
) -> bool {
    outer_present && !outer_pending_kill && !outer_is_game_state_recorder
}

/// Per-recorder inputs for `UIcarusGameStateRecorder::RecordToProspectSaveState`
/// (`0x1414b4680` server / `0x1414ecdd0` client).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RecordToProspectSaveStepInputs {
    pub recorder_present: bool,
    pub recorder_pending_kill: bool,
    pub outer_present: bool,
    pub outer_pending_kill: bool,
    pub outer_is_game_state_recorder: bool,
    /// `bIsRetained` (`+0xd2`).
    pub b_is_retained: bool,
    /// Virtual slot `+0x410` (`ShouldDropRecorder`).
    pub should_drop_recorder: bool,
}

/// Per-recorder disposition in
/// `UIcarusGameStateRecorder::RecordToProspectSaveState` (`0x1414b4680` server /
/// `0x1414ecdd0` client).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecordToProspectSaveStepOutcome {
    /// Removes the recorder from `CurrentRecorderComponents`; invokes virtual
    /// drop hook `+0x418` beforehand iff `invoke_drop_hook` is `true`.
    PruneAndRemove { invoke_drop_hook: bool },
    /// Calls `RecordOwnerState` (`+0x448`) and serializes a fresh
    /// `FStateRecorderBlob` entry.
    RecordOwnerStateAndWriteBlob,
    /// Serializes the retained recorder's existing state into
    /// `StateRecorderBlobs` without calling `RecordOwnerState` (`+0x448`).
    WriteExistingRetainedBlobWithoutRecording,
}

/// Evaluates one step of `UIcarusGameStateRecorder::RecordToProspectSaveState`
/// (`0x1414b4680` server / `0x1414ecdd0` client).
pub const fn evaluate_record_to_prospect_save_step(
    inputs: &RecordToProspectSaveStepInputs,
) -> RecordToProspectSaveStepOutcome {
    if !inputs.recorder_present || inputs.recorder_pending_kill {
        return RecordToProspectSaveStepOutcome::PruneAndRemove {
            invoke_drop_hook: false,
        };
    }
    let has_valid_owner = has_valid_recorder_owner(
        inputs.outer_present,
        inputs.outer_pending_kill,
        inputs.outer_is_game_state_recorder,
    );
    if (!has_valid_owner && !inputs.b_is_retained) || inputs.should_drop_recorder {
        return RecordToProspectSaveStepOutcome::PruneAndRemove {
            invoke_drop_hook: true,
        };
    }
    if has_valid_owner {
        RecordToProspectSaveStepOutcome::RecordOwnerStateAndWriteBlob
    } else {
        RecordToProspectSaveStepOutcome::WriteExistingRetainedBlobWithoutRecording
    }
}

/// Outcome for an `InitialRecorders \\ CurrentRecorderComponents` entry during
/// `UIcarusGameStateRecorder::ReadFromProspectSaveState` (`0x1414b82b0..0x1414b8311`
/// server / `0x1414ec3f0` client) and `UIcarusGameStateRecorder::RetainRecorder`
/// (`0x1414b86b0` server / `0x1414f0300` client).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnresolvedInitialRecorderOutcome {
    /// `HasValidOwner` is `true`: invokes `BeginRecording` (`0x1414961b0`).
    BeginRecording,
    /// `HasValidOwner` is `false`: invokes `RetainRecorder` (`0x1414b86b0`),
    /// setting `bIsRetained = true` (`+0xd2`), unregistering the component when
    /// `b_registered` (`+0x88 & 1`) is set, and renaming `Outer` to
    /// `UIcarusGameStateRecorder` with `REN_DontCreateRedirectors = 1` when
    /// `!outer_is_game_state_recorder`.
    RetainRecorder {
        unregister_component: bool,
        rename_outer_to_game_state_recorder: bool,
    },
}

/// Evaluates the `InitialRecorders \\ CurrentRecorderComponents` cleanup in
/// `UIcarusGameStateRecorder::ReadFromProspectSaveState` (`0x1414b82b0..0x1414b8311`
/// server / `0x1414ec3f0` client) and `UIcarusGameStateRecorder::RetainRecorder`
/// (`0x1414b86b0` server / `0x1414f0300` client).
pub const fn evaluate_unresolved_initial_recorder(
    has_valid_owner: bool,
    b_registered: bool,
    outer_is_game_state_recorder: bool,
) -> UnresolvedInitialRecorderOutcome {
    if has_valid_owner {
        UnresolvedInitialRecorderOutcome::BeginRecording
    } else {
        UnresolvedInitialRecorderOutcome::RetainRecorder {
            unregister_component: b_registered,
            rename_outer_to_game_state_recorder: !outer_is_game_state_recorder,
        }
    }
}

/// Evaluates the actor slice `[start, end)` processed by
/// `UIcarusGameStateRecorder::PerformStitchingPass` (`0x1414b1d70` server /
/// `0x1414eaae0` client).
pub fn evaluate_stitching_pass_slice(
    total_actors: i32,
    start_index: i32,
    max_count: i32,
) -> (usize, usize) {
    let total = total_actors.max(0);
    let end = if max_count >= 0 {
        total.min(start_index.saturating_add(max_count)).max(0)
    } else {
        total
    };
    let start = start_index.max(0).min(end);
    (start as usize, end as usize)
}

/// Evaluates whether `UIcarusGameStateRecorder::PerformStitchingPass`
/// (`0x1414b1d70` server / `0x1414eaae0` client) runs the grid-building actor
/// stitching pass for a recorder.
pub const fn should_stitch_with_grid_building_actors(
    b_include_grid_building_actors: bool,
    is_deployable_recorder: bool,
    recorder_pending_kill: bool,
) -> bool {
    b_include_grid_building_actors && is_deployable_recorder && !recorder_pending_kill
}

/// Base character recorder fields (`+0x1c0`, `+0x1c4`) for `N010`
/// (`0x14143d900`/`0x14147bc20` `+0x448` and `0x14144c1b0`/`0x14148a880`
/// `+0x460`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CharacterBaseRecorderState {
    pub character_uid: i32,
    pub parent_character_uid: i32,
}

/// Evaluates `N010` `RecordOwnerState` (`0x14143d900` server / `0x14147bc20`
/// client, `+0x448`).
pub const fn evaluate_n010_character_record(
    owner_is_character: bool,
    player_state_uid: Option<i32>,
    owner_parent_uid: i32,
) -> Option<CharacterBaseRecorderState> {
    if !owner_is_character {
        return None;
    }
    let character_uid = match player_state_uid {
        Some(uid) => uid,
        None => 0,
    };
    Some(CharacterBaseRecorderState {
        character_uid,
        parent_character_uid: owner_parent_uid,
    })
}

/// Evaluates `N010` `UpdateOwnerToDatabaseState` (`0x14144c1b0` server /
/// `0x14148a880` client, `+0x460`).
pub const fn evaluate_n010_character_restore(
    owner_is_character: bool,
    has_player_state: bool,
    state: CharacterBaseRecorderState,
) -> Option<(bool, Option<i32>, i32)> {
    if !owner_is_character {
        return None;
    }
    let restored_uid = if has_player_state {
        Some(state.character_uid)
    } else {
        None
    };
    Some((true, restored_uid, state.parent_character_uid))
}

/// Base NPC recorder fields (`+0x1c0..+0x1df`) for `N014`
/// (`0x1413cc9b0`/`0x14140b700` `+0x448` and `0x1413e0710`/`0x14141e250`
/// `+0x460`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NpcBaseRecorderState {
    pub base: CharacterBaseRecorderState,
    pub player_state_attributes: [i32; 4],
    pub npc_row_name: String,
}

/// Evaluates `N014` `RecordOwnerState` (`0x1413cc9b0` server / `0x14140b700`
/// client, `+0x448`).
#[allow(clippy::too_many_arguments)]
pub fn evaluate_n014_npc_record(
    owner_is_character: bool,
    owner_is_npc: bool,
    player_state_uid: Option<i32>,
    owner_parent_uid: i32,
    player_state_attributes: Option<[i32; 4]>,
    existing_attributes: [i32; 4],
    npc_row_name: &str,
) -> Option<NpcBaseRecorderState> {
    let base =
        evaluate_n010_character_record(owner_is_character, player_state_uid, owner_parent_uid)?;
    let (attributes, row_name) = if owner_is_npc {
        (
            player_state_attributes.unwrap_or(existing_attributes),
            npc_row_name.to_owned(),
        )
    } else {
        (existing_attributes, String::new())
    };
    Some(NpcBaseRecorderState {
        base,
        player_state_attributes: attributes,
        npc_row_name: row_name,
    })
}

/// Evaluates `N014` `UpdateOwnerToDatabaseState` (`0x1413e0710` server /
/// `0x14141e250` client, `+0x460`).
#[allow(clippy::type_complexity)]
pub fn evaluate_n014_npc_restore(
    owner_is_character: bool,
    owner_is_npc: bool,
    has_player_state: bool,
    has_icarus_player_state: bool,
    state: &NpcBaseRecorderState,
) -> Option<(bool, Option<i32>, i32, [Option<i32>; 4])> {
    let (flag_571, restored_uid, parent_uid) =
        evaluate_n010_character_restore(owner_is_character, has_player_state, state.base)?;
    let attributes = if owner_is_npc && has_icarus_player_state {
        state
            .player_state_attributes
            .map(|v| if v >= 0 { Some(v) } else { None })
    } else {
        [None; 4]
    };
    Some((flag_571, restored_uid, parent_uid, attributes))
}

/// Evaluates `N013` `UpdateOwnerToDatabaseState` (`0x1413e05d0` server /
/// `0x14141e110` client, `+0x460`), returning the full `FName`
/// `(mission_row_name, mission_row_number)` (`lea rdx, [rbx+0x1e0]` at
/// `0x1413e06e1`) when `!FName::IsNone()`.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub fn evaluate_n013_mission_npc_restore(
    owner_is_character: bool,
    owner_is_npc: bool,
    owner_is_mission_npc: bool,
    has_player_state: bool,
    has_icarus_player_state: bool,
    state: &NpcBaseRecorderState,
    mission_row_name: &str,
    mission_row_number: i32,
) -> Option<(
    bool,
    Option<i32>,
    i32,
    [Option<i32>; 4],
    Option<(String, i32)>,
)> {
    let (flag_571, restored_uid, parent_uid, attributes) = evaluate_n014_npc_restore(
        owner_is_character,
        owner_is_npc,
        has_player_state,
        has_icarus_player_state,
        state,
    )?;
    let mission_row = if owner_is_mission_npc
        && ((!mission_row_name.is_empty() && !mission_row_name.eq_ignore_ascii_case("None"))
            || mission_row_number != 0)
    {
        Some((mission_row_name.to_owned(), mission_row_number))
    } else {
        None
    };
    Some((flag_571, restored_uid, parent_uid, attributes, mission_row))
}

/// Evaluates `N011` subcomponent integer transfer (`+0x2d8 <-> +0xf8`) in
/// `RecordOwnerState` (`0x1413fd560` server / `0x14143b360` client, `+0x448`)
/// and `UpdateOwnerToDatabaseState` (`0x14140c580` server / `0x1414498c0`
/// client, `+0x460`).
pub const fn evaluate_n011_subcomponent_int_transfer(
    owner_matches_subclass: bool,
    has_subcomponent: bool,
    value_2d8: i32,
) -> Option<i32> {
    if owner_matches_subclass && has_subcomponent {
        Some(value_2d8)
    } else {
        None
    }
}

/// Evaluates `N016` blackboard integer transfer (`+0x1c8`, `+0x1cc`, `+0x1e0`)
/// in `Stitching` (`0x141405160` server / `0x141442d00` client, `+0x468`).
pub const fn evaluate_n016_stitch_blackboard_ints(
    owner_is_mount: bool,
    controller_is_npc_controller: bool,
    ints_1c8_1cc_1e0: [i32; 3],
) -> Option<[i32; 3]> {
    if owner_is_mount && controller_is_npc_controller {
        Some(ints_1c8_1cc_1e0)
    } else {
        None
    }
}

/// Inputs for `UIcarusGameStateRecorder::BeginRecording` (`0x1414961b0` server /
/// `0x1414d0900` client).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BeginRecordingInputs {
    pub recorder_present: bool,
    pub recorder_pending_kill: bool,
    /// `Owner` (`+0xa0`) has the persistence-exclusion GameplayTag (`0x65`).
    pub owner_has_exclude_from_persistence_tag: bool,
    /// `Recorder->bIsRecording` (`+0xd1 != 0`).
    pub b_is_recording: bool,
    /// `GameStateRecorder->bIsRestoring` (`+0x128 != 0`).
    pub game_state_b_is_restoring: bool,
    /// Whether `RecorderComponents.AddUnique(Recorder)` (`+0x28`) succeeded (`Index != -1`).
    pub add_unique_index: Option<i32>,
    /// `Recorder->HasValidOwner()` (`0x141898750` server / `0x14189ff30` client).
    pub has_valid_owner: bool,
    /// `Recorder->bOwnerCallbacksBound` (`+0xd2 != 0`).
    pub b_owner_callbacks_bound: bool,
    /// `Recorder->vtable[+0x420]()` (`HasSavedSpawnLocation()`).
    pub has_saved_spawn_location: bool,
    /// `Recorder->vtable[+0x428]()` (`GetSavedSpawnLocation()`) is non-zero (`!= (0, 0, 0)`).
    pub spawn_location_non_zero: bool,
}

/// Outcome of `UIcarusGameStateRecorder::BeginRecording` (`0x1414961b0` server /
/// `0x1414d0900` client).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BeginRecordingOutcome {
    /// Null or `RF_PendingKill` recorder (`returns false`).
    RejectedInvalidRecorder,
    /// `Owner` has the persistence-exclusion GameplayTag (`0x65`, `returns false`).
    RejectedExcludedOwner,
    /// `Recorder->bIsRecording` (`+0xd1`) is already non-zero (`returns true`).
    AlreadyRecording,
    /// `this->bIsRestoring` (`+0x128`) is non-zero: appends `TWeakObjectPtr(Recorder)`
    /// via `AddUnique` to `DeferredBeginRecordingComponents` (`+0x130`) and `returns false`.
    DeferredWhileRestoring,
    /// `RecorderComponents.AddUnique` returned `-1` (`returns false`).
    RejectedAddFailed,
    /// Added at `recorder_index`: sets `bIsRecording = true` (`+0xd1`), optionally binds
    /// owner callbacks (`0x1418a3d90` when `has_valid_owner && !b_owner_callbacks_bound`),
    /// and optionally inserts `(SpawnLocation -> recorder_index)` into
    /// `SpawnLocationToRecorderIndexMap` (`+0xd8`, `returns true`).
    StartedRecording {
        recorder_index: i32,
        bind_owner_callbacks: bool,
        register_spawn_location_index: bool,
    },
}

/// Evaluates `UIcarusGameStateRecorder::BeginRecording` (`0x1414961b0` server /
/// `0x1414d0900` client).
pub const fn evaluate_begin_recording(inputs: &BeginRecordingInputs) -> BeginRecordingOutcome {
    if !inputs.recorder_present || inputs.recorder_pending_kill {
        return BeginRecordingOutcome::RejectedInvalidRecorder;
    }
    if inputs.owner_has_exclude_from_persistence_tag {
        return BeginRecordingOutcome::RejectedExcludedOwner;
    }
    if inputs.b_is_recording {
        return BeginRecordingOutcome::AlreadyRecording;
    }
    if inputs.game_state_b_is_restoring {
        return BeginRecordingOutcome::DeferredWhileRestoring;
    }
    let Some(recorder_index) = inputs.add_unique_index else {
        return BeginRecordingOutcome::RejectedAddFailed;
    };
    if recorder_index < 0 {
        return BeginRecordingOutcome::RejectedAddFailed;
    }
    BeginRecordingOutcome::StartedRecording {
        recorder_index,
        bind_owner_callbacks: inputs.has_valid_owner && !inputs.b_owner_callbacks_bound,
        register_spawn_location_index: inputs.has_saved_spawn_location
            && inputs.spawn_location_non_zero,
    }
}

/// Inputs for `UIcarusGameStateRecorder::EndRecording` (`0x14149fcf0` server /
/// `0x1414da140` client) and `HandleRecorderRemoved` (`0x1414e6140` client).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EndRecordingInputs {
    pub recorder_present: bool,
    pub owner_has_exclude_from_persistence_tag: bool,
    pub b_is_recording: bool,
    /// Index of `Recorder` in `RecorderComponents` (`+0x28`), if found (`!= -1`).
    pub removed_index: Option<i32>,
    /// Length of `RecorderComponents` after `RemoveAtSwap(removed_index, 1)`.
    pub new_recorder_count: i32,
    /// Whether the swapped-in tail component at `removed_index` has `vtable[+0x420]() == true`.
    pub swapped_component_has_saved_spawn_location: bool,
}

/// Outcome of `UIcarusGameStateRecorder::EndRecording` (`0x14149fcf0` server /
/// `0x1414da140` client) and `HandleRecorderRemoved` (`0x1414e6140` client).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EndRecordingOutcome {
    /// Null recorder or GameplayTag-excluded owner (`returns false`).
    Rejected,
    /// Removed from `DeferredBeginRecordingComponents` (`+0x130`) only because
    /// `bIsRecording` (`+0xd1`) was `0` (`returns true`).
    RemovedDeferredOnly,
    /// Removed from `DeferredBeginRecordingComponents` (`+0x130`) and `RecorderComponents`
    /// (`+0x28`) via `RemoveAtSwap`, optionally updating the swapped-in component's index in
    /// `SpawnLocationToRecorderIndexMap` (`+0xd8`), zeroing `WORD [Recorder + 0xd1]`
    /// (`bIsRecording = 0`, `bOwnerCallbacksBound = 0`), and removing `Recorder`'s spawn
    /// location from `SpawnLocationToRecorderIndexMap` (`returns true`).
    RemovedActiveRecorder {
        swapped_spawn_map_new_index: Option<i32>,
        clear_recording_and_callback_flags: bool,
        remove_own_spawn_location_from_map: bool,
    },
}

/// Evaluates `UIcarusGameStateRecorder::EndRecording` (`0x14149fcf0` server /
/// `0x1414da140` client) and `HandleRecorderRemoved` (`0x1414e6140` client).
pub const fn evaluate_end_recording(inputs: &EndRecordingInputs) -> EndRecordingOutcome {
    if !inputs.recorder_present || inputs.owner_has_exclude_from_persistence_tag {
        return EndRecordingOutcome::Rejected;
    }
    if !inputs.b_is_recording {
        return EndRecordingOutcome::RemovedDeferredOnly;
    }
    let swapped_spawn_map_new_index = match inputs.removed_index {
        Some(idx)
            if idx >= 0
                && idx < inputs.new_recorder_count
                && inputs.swapped_component_has_saved_spawn_location =>
        {
            Some(idx)
        }
        _ => None,
    };
    EndRecordingOutcome::RemovedActiveRecorder {
        swapped_spawn_map_new_index,
        clear_recording_and_callback_flags: true,
        remove_own_spawn_location_from_map: true,
    }
}

/// Evaluates `UIcarusGameStateRecorder::FlushDeferredRecorder` (`0x1414db370` client).
pub const fn evaluate_flush_deferred_recorder(
    was_in_deferred_list: bool,
    begin_inputs: &BeginRecordingInputs,
) -> Option<BeginRecordingOutcome> {
    if !was_in_deferred_list || !begin_inputs.recorder_present || begin_inputs.recorder_pending_kill
    {
        return None;
    }
    Some(evaluate_begin_recording(begin_inputs))
}

/// Per-recorder inputs for `UIcarusGameStateRecorder::TryResolveRecorders`
/// (`0x1414bee00` server / `0x1414f6150` client).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TryResolveRecorderStepInputs {
    pub recorder_present: bool,
    pub recorder_pending_kill: bool,
    pub already_has_valid_owner: bool,
    pub owner_resolve_policy: OwnerResolvePolicy,
    pub b_force_respawn: bool,
    pub found_existing_owner: bool,
    pub attach_to_found_owner_succeeded: bool,
    pub respawn_owner_succeeded: bool,
    pub attach_to_respawned_owner_succeeded: bool,
}

/// Per-recorder disposition in `UIcarusGameStateRecorder::TryResolveRecorders`
/// (`0x1414bee00` server / `0x1414f6150` client).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TryResolveRecorderStepOutcome {
    /// Null, `RF_PendingKill`, or already has a valid owner (`HasValidOwner() == true`).
    SkipAlreadyValidOrInvalid,
    /// Attached to an existing world actor via `FindExistingOwner` (`+0x450`) and
    /// `ResolveOwnerRecorderState` (`bDestroyActorOnFailure = true`); runs
    /// `StitchingOperation` (`+0x468`) post-loop iff `run_post_resolve_stitching` is `true`.
    ResolvedViaExistingOwner { run_post_resolve_stitching: bool },
    /// Respawned owner via `RespawnOwner` (`+0x458`) and attached via
    /// `ResolveOwnerRecorderState` (`bDestroyActorOnFailure = false`), optionally
    /// destroying a pre-existing duplicate found by `FindExistingOwner` first; runs
    /// `StitchingOperation` (`+0x468`) post-loop iff `run_post_resolve_stitching` is `true`.
    ResolvedViaRespawnedOwner {
        destroyed_existing_owner_first: bool,
        run_post_resolve_stitching: bool,
    },
    /// Resolution did not attach a valid owner (including `ManuallyResolved` policy `3`
    /// or failed find/respawn): invokes `RetainRecorder` (`0x1414b8650` / `0x1414f0300`).
    RetainedUnresolved {
        destroyed_existing_owner_first: bool,
        destroyed_failed_candidate_actor: bool,
    },
}

/// Evaluates one recorder entry in `UIcarusGameStateRecorder::TryResolveRecorders`
/// (`0x1414bee00` server / `0x1414f6150` client).
pub const fn evaluate_try_resolve_recorder_step(
    inputs: &TryResolveRecorderStepInputs,
) -> TryResolveRecorderStepOutcome {
    if !inputs.recorder_present || inputs.recorder_pending_kill || inputs.already_has_valid_owner {
        return TryResolveRecorderStepOutcome::SkipAlreadyValidOrInvalid;
    }
    let run_post_resolve_stitching = !inputs.b_force_respawn;
    match inputs.owner_resolve_policy {
        OwnerResolvePolicy::FindOnly => {
            if inputs.found_existing_owner && inputs.attach_to_found_owner_succeeded {
                TryResolveRecorderStepOutcome::ResolvedViaExistingOwner {
                    run_post_resolve_stitching,
                }
            } else {
                TryResolveRecorderStepOutcome::RetainedUnresolved {
                    destroyed_existing_owner_first: false,
                    destroyed_failed_candidate_actor: inputs.found_existing_owner,
                }
            }
        }
        OwnerResolvePolicy::RespawnOnly => {
            let destroyed_existing = !inputs.b_force_respawn && inputs.found_existing_owner;
            if inputs.respawn_owner_succeeded && inputs.attach_to_respawned_owner_succeeded {
                TryResolveRecorderStepOutcome::ResolvedViaRespawnedOwner {
                    destroyed_existing_owner_first: destroyed_existing,
                    run_post_resolve_stitching,
                }
            } else {
                TryResolveRecorderStepOutcome::RetainedUnresolved {
                    destroyed_existing_owner_first: destroyed_existing,
                    destroyed_failed_candidate_actor: false,
                }
            }
        }
        OwnerResolvePolicy::FindOrRespawn => {
            if !inputs.b_force_respawn
                && inputs.found_existing_owner
                && inputs.attach_to_found_owner_succeeded
            {
                return TryResolveRecorderStepOutcome::ResolvedViaExistingOwner {
                    run_post_resolve_stitching,
                };
            }
            if inputs.respawn_owner_succeeded && inputs.attach_to_respawned_owner_succeeded {
                TryResolveRecorderStepOutcome::ResolvedViaRespawnedOwner {
                    destroyed_existing_owner_first: false,
                    run_post_resolve_stitching,
                }
            } else {
                TryResolveRecorderStepOutcome::RetainedUnresolved {
                    destroyed_existing_owner_first: false,
                    destroyed_failed_candidate_actor: !inputs.b_force_respawn
                        && inputs.found_existing_owner
                        && !inputs.attach_to_found_owner_succeeded,
                }
            }
        }
        OwnerResolvePolicy::ManuallyResolved => TryResolveRecorderStepOutcome::RetainedUnresolved {
            destroyed_existing_owner_first: false,
            destroyed_failed_candidate_actor: false,
        },
    }
}

/// Inputs for `UIcarusGameStateRecorder::ResolveOwnerRecorderState`
/// (`0x1414b6fc0` server / `0x1414ef1e0` client).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResolveOwnerRecorderStateInputs {
    pub deserialized_recorder_present: bool,
    pub deserialized_recorder_pending_kill: bool,
    pub owner_actor_present: bool,
    pub owner_actor_pending_kill: bool,
    pub owner_has_matching_component: bool,
    /// `DeserializedRecorder->bCanDynamicallyAttachToOwner` (`+0xb4 != 0`).
    pub b_can_dynamically_attach_to_owner: bool,
    pub dynamic_component_created: bool,
    /// Optional `UIcarusRecorderManagerSubsystem` registration UID (`[Subsystem + 0x38]`).
    pub registered_subsystem_uid: Option<i32>,
}

/// Outcome of `UIcarusGameStateRecorder::ResolveOwnerRecorderState`
/// (`0x1414b6fc0` server / `0x1414ef1e0` client).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResolveOwnerRecorderStateOutcome {
    /// Null or `RF_PendingKill` deserialized recorder or owner actor (`returns nullptr`).
    RejectedInvalidInputs,
    /// Owner lacked a matching component and `bCanDynamicallyAttachToOwner` (`+0xb4`) was
    /// `false`, or dynamic `NewObject` creation failed (`returns nullptr`).
    RejectedMissingComponent,
    /// Attached to `OwnerRecorder`, round-tripped `WriteToStateRecorderBlob` ->
    /// `ReadFromStateRecorderBlob` with identity `FTransform`, transferred any
    /// `UIcarusRecorderManagerSubsystem` UID registration, called
    /// `EndRecording(DeserializedRecorder)` and `BeginRecording(OwnerRecorder)`, and
    /// invoked `UpdateOwnerToDatabaseState(true)` (`vtable[+0x460]`).
    AttachedAndUpdated {
        created_dynamic_component: bool,
        transferred_subsystem_uid: Option<i32>,
    },
}

/// Evaluates `UIcarusGameStateRecorder::ResolveOwnerRecorderState`
/// (`0x1414b6fc0` server / `0x1414ef1e0` client).
pub const fn evaluate_resolve_owner_recorder_state(
    inputs: &ResolveOwnerRecorderStateInputs,
) -> ResolveOwnerRecorderStateOutcome {
    if !inputs.deserialized_recorder_present
        || inputs.deserialized_recorder_pending_kill
        || !inputs.owner_actor_present
        || inputs.owner_actor_pending_kill
    {
        return ResolveOwnerRecorderStateOutcome::RejectedInvalidInputs;
    }
    let created_dynamic_component = if inputs.owner_has_matching_component {
        false
    } else if inputs.b_can_dynamically_attach_to_owner && inputs.dynamic_component_created {
        true
    } else {
        return ResolveOwnerRecorderStateOutcome::RejectedMissingComponent;
    };
    ResolveOwnerRecorderStateOutcome::AttachedAndUpdated {
        created_dynamic_component,
        transferred_subsystem_uid: inputs.registered_subsystem_uid,
    }
}

/// Inputs for `UIcarusGameStateRecorder::TryRestoreActorRecorder`
/// (`0x1414bf530` server / `0x1414f6650` client).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TryRestoreActorRecorderInputs {
    pub actor_present: bool,
    pub actor_pending_kill: bool,
    pub b_require_manually_resolved: bool,
    pub b_only_by_spawn_location: bool,
    pub has_root_component: bool,
    pub root_translation_non_zero: bool,
    /// `USceneComponent::Mobility` (`+0x14f`); `2` is `EComponentMobility::Movable`.
    pub root_mobility: u8,
    /// Candidate found via `SpawnLocationToRecorderIndexMap` (`+0xd8`) fast-path.
    pub spawn_map_match_policy: Option<OwnerResolvePolicy>,
    /// Candidate found via linear scan of `RecorderComponents` (`+0x28`).
    pub linear_scan_match_policy: Option<OwnerResolvePolicy>,
    pub resolve_owner_succeeded: bool,
    pub resolved_requires_stitching: bool,
}

/// Outcome of `UIcarusGameStateRecorder::TryRestoreActorRecorder`
/// (`0x1414bf530` server / `0x1414f6650` client).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TryRestoreActorRecorderOutcome {
    /// Null or `RF_PendingKill` actor, or no matching unattached recorder found.
    NotRestored,
    /// Matched a recorder (either via `SpawnLocationToRecorderIndexMap` fast-path or
    /// linear scan), resolved owner state via `ResolveOwnerRecorderState`, and ran
    /// `StitchingOperation` (`+0x468`) iff `OwnerResolvePolicy == ManuallyResolved` and
    /// `RequiresStitching()` (`+0x470`) returned `true`.
    Restored {
        matched_via_spawn_location_map: bool,
        ran_stitching_operation: bool,
    },
}

/// Evaluates `UIcarusGameStateRecorder::TryRestoreActorRecorder`
/// (`0x1414bf530` server / `0x1414f6650` client).
pub const fn evaluate_try_restore_actor_recorder(
    inputs: &TryRestoreActorRecorderInputs,
) -> TryRestoreActorRecorderOutcome {
    if !inputs.actor_present || inputs.actor_pending_kill {
        return TryRestoreActorRecorderOutcome::NotRestored;
    }
    let can_use_spawn_map =
        inputs.has_root_component && inputs.root_translation_non_zero && inputs.root_mobility != 2;
    let spawn_map_candidate = if can_use_spawn_map {
        inputs.spawn_map_match_policy
    } else {
        None
    };
    let (matched_policy, matched_via_spawn_location_map) = match spawn_map_candidate {
        Some(policy)
            if !inputs.b_require_manually_resolved
                || matches!(policy, OwnerResolvePolicy::ManuallyResolved) =>
        {
            (policy, true)
        }
        _ => {
            if inputs.b_only_by_spawn_location {
                return TryRestoreActorRecorderOutcome::NotRestored;
            }
            let Some(policy) = inputs.linear_scan_match_policy else {
                return TryRestoreActorRecorderOutcome::NotRestored;
            };
            if inputs.b_require_manually_resolved
                && !matches!(policy, OwnerResolvePolicy::ManuallyResolved)
            {
                return TryRestoreActorRecorderOutcome::NotRestored;
            }
            (policy, false)
        }
    };
    if !inputs.resolve_owner_succeeded {
        return TryRestoreActorRecorderOutcome::NotRestored;
    }
    let ran_stitching_operation = matches!(matched_policy, OwnerResolvePolicy::ManuallyResolved)
        && inputs.resolved_requires_stitching;
    TryRestoreActorRecorderOutcome::Restored {
        matched_via_spawn_location_map,
        ran_stitching_operation,
    }
}

/// Evaluates `N020` `Validate` (`vftable + 0x408`: `0x140899f70` server /
/// `0x1408c9810` client, `mov al, 1; ret`), which overrides its parent `C028`'s
/// `Super_Cooled_Ice` row exclusion check to unconditionally return `true`.
pub const fn evaluate_n020_validate_override() -> bool {
    true
}

/// Subcomponent A fields (`+0x1e0..+0x20b`) recorded by `N012`
/// (`0x1413fd5e0`/`0x14143b3e0` `+0x448` and `0x14140c600`/`0x141449940` `+0x460`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct N012SubcompAState {
    pub byte_1e0: u8,
    pub value_1e4: i32,
    pub row_name_1e8: String,
    pub flag_1f0: bool,
    pub entries_1f8: Vec<i32>,
    pub value_208: i32,
}

/// Subcomponent B fields (`+0x210..+0x257`) recorded by `N012`
/// (`0x1413fd5e0`/`0x14143b3e0` `+0x448` and `0x14140c600`/`0x141449940` `+0x460`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct N012SubcompBState {
    pub compact_entries_210: Vec<(String, i32)>,
    pub flag_220: u8,
    pub value_224: i32,
    pub byte_228: u8,
    pub row_name_22c: String,
    pub text_238: String,
    pub text_248: String,
}

/// Combined state for `N012` (`super = N014`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct N012RecorderState {
    pub npc_base: NpcBaseRecorderState,
    pub subcomp_a: N012SubcompAState,
    pub subcomp_b: Option<N012SubcompBState>,
}

/// Inputs for `N012` `RecordOwnerState` (`0x1413fd5e0` server / `0x14143b3e0` client, `+0x448`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct N012RecordInputs<'a> {
    pub owner_is_character: bool,
    pub owner_is_npc: bool,
    pub owner_is_n012_target: bool,
    pub player_state_uid: Option<i32>,
    pub owner_parent_uid: i32,
    pub player_state_attributes: Option<[i32; 4]>,
    pub existing_attributes: [i32; 4],
    pub npc_row_name: &'a str,
    pub subcomp_a_input: Option<(u8, i32, &'a str, Vec<i32>, i32)>,
    pub mesh_socket_flag: Option<bool>,
    pub subcomp_b_input: Option<N012SubcompBState>,
}

/// Evaluates `N012` `RecordOwnerState` (`0x1413fd5e0` server / `0x14143b3e0`
/// client, `+0x448`).
pub fn evaluate_n012_record(inputs: N012RecordInputs<'_>) -> Option<N012RecorderState> {
    let npc_base = evaluate_n014_npc_record(
        inputs.owner_is_character,
        inputs.owner_is_npc,
        inputs.player_state_uid,
        inputs.owner_parent_uid,
        inputs.player_state_attributes,
        inputs.existing_attributes,
        inputs.npc_row_name,
    )?;
    if !inputs.owner_is_n012_target {
        return None;
    }
    let subcomp_a = match inputs.subcomp_a_input {
        Some((byte_b0, val_b4, row_b8, arr_d8, val_e8)) => N012SubcompAState {
            byte_1e0: byte_b0,
            value_1e4: val_b4,
            row_name_1e8: row_b8.to_owned(),
            flag_1f0: inputs.mesh_socket_flag.unwrap_or(true),
            entries_1f8: arr_d8,
            value_208: val_e8,
        },
        None => N012SubcompAState {
            byte_1e0: 0,
            value_1e4: 0,
            row_name_1e8: String::new(),
            flag_1f0: true,
            entries_1f8: Vec::new(),
            value_208: 0,
        },
    };
    Some(N012RecorderState {
        npc_base,
        subcomp_a,
        subcomp_b: inputs.subcomp_b_input,
    })
}

/// Inputs for `N012` `UpdateOwnerToDatabaseState` (`0x14140c600` server /
/// `0x141449940` client, `+0x460`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct N012RestoreInputs {
    pub owner_is_character: bool,
    pub owner_is_npc: bool,
    pub owner_is_n012_target: bool,
    pub has_player_state: bool,
    pub has_icarus_player_state: bool,
    pub has_subcomp_a: bool,
    pub has_subcomp_b: bool,
    pub subcomp_b_existing_len: usize,
}

/// Outcome of `N012` `UpdateOwnerToDatabaseState` (`0x14140c600` server /
/// `0x141449940` client, `+0x460`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct N012RestoreOutcome {
    pub npc_base_outcome: (bool, Option<i32>, i32, [Option<i32>; 4]),
    pub applied_subcomp_a: bool,
    pub cleared_subcomp_b: bool,
    pub applied_subcomp_b: bool,
    pub triggered_owner_hook_228: bool,
}

/// Evaluates `N012` `UpdateOwnerToDatabaseState` (`0x14140c600` server /
/// `0x141449940` client, `+0x460`), which applies `SubcompA` and `SubcompB`
/// state before tail-calling `N014::UpdateOwnerToDatabaseState` (`0x1413e0710`).
pub fn evaluate_n012_restore(
    inputs: &N012RestoreInputs,
    state: &N012RecorderState,
) -> Option<N012RestoreOutcome> {
    let npc_base_outcome = evaluate_n014_npc_restore(
        inputs.owner_is_character,
        inputs.owner_is_npc,
        inputs.has_player_state,
        inputs.has_icarus_player_state,
        &state.npc_base,
    )?;
    if !inputs.owner_is_n012_target {
        return Some(N012RestoreOutcome {
            npc_base_outcome,
            applied_subcomp_a: false,
            cleared_subcomp_b: false,
            applied_subcomp_b: false,
            triggered_owner_hook_228: false,
        });
    }
    let has_row_1e8 = !state.subcomp_a.row_name_1e8.is_empty()
        && !state.subcomp_a.row_name_1e8.eq_ignore_ascii_case("None");
    let applied_subcomp_a = has_row_1e8 && inputs.has_subcomp_a;
    let (cleared_subcomp_b, applied_subcomp_b, triggered_owner_hook_228) =
        if let (true, Some(b_ref)) = (inputs.has_subcomp_b, state.subcomp_b.as_ref()) {
            (inputs.subcomp_b_existing_len > 0, true, b_ref.byte_228 != 0)
        } else {
            (false, false, false)
        };
    Some(N012RestoreOutcome {
        npc_base_outcome,
        applied_subcomp_a,
        cleared_subcomp_b,
        applied_subcomp_b,
        triggered_owner_hook_228,
    })
}

/// Evaluates `N002` interface integer transfer (`+0xd8`) in `RecordOwnerState`
/// (`0x1418a0990` server / `0x1418a6230` client, `+0x448`) and
/// `UpdateOwnerToDatabaseState` (`0x1418b76b0` server / `0x1418ba580` client,
/// `+0x460`).
pub const fn evaluate_n002_interface_int_transfer(
    owner_present: bool,
    owner_implements_interface: bool,
    value_d8: i32,
) -> Option<i32> {
    if owner_present && owner_implements_interface {
        Some(value_d8)
    } else {
        None
    }
}

/// Evaluates `N003` linked-actor stitching (`0x141876e90` server / `0x141880c30`
/// client, `+0x468`), which copies `byte_1c0` to `Owner + 0x2f8` and matches the
/// first candidate in `AllActors` of the target class whose `GetClass()->GetName()`
/// equals `saved_class_name_1d0` (case-insensitive `FCString::Stricmp`).
pub fn evaluate_n003_stitch_linked_actor<'a>(
    owner_matches_n003_class: bool,
    byte_1c0: u8,
    saved_class_name_1d0: &str,
    candidates: &'a [(bool, &'a str)],
) -> Option<(u8, Option<usize>)> {
    if !owner_matches_n003_class {
        return None;
    }
    let matched_idx = candidates.iter().position(|(is_target_class, name)| {
        *is_target_class && name.eq_ignore_ascii_case(saved_class_name_1d0)
    });
    Some((byte_1c0, matched_idx))
}

/// Evaluates `N004` subcomponent integer pair transfer (`+0x290 <-> +0xb0`,
/// `+0x294 <-> +0xb4`) in `RecordOwnerState` (`0x1418a0c80` server /
/// `0x1418a6400` client, `+0x448`) and `RespawnOwner` (`0x1418a7460` server /
/// `0x1418abf40` client, `+0x458`).
pub const fn evaluate_n004_subcomponent_pair_transfer(
    owner_present: bool,
    has_valid_subcomponent: bool,
    pair_290_294: (i32, i32),
) -> Option<(i32, i32)> {
    if owner_present && has_valid_subcomponent {
        Some(pair_290_294)
    } else {
        None
    }
}

/// State fields (`+0x290..+0x2ab`) transferred via interface by `N007` in
/// `RecordOwnerState` (`0x1418a1ca0` server / `0x1418a7420` client, `+0x448`)
/// and `UpdateOwnerToDatabaseState` (`0x1418b7e50` server / `0x1418bad20`
/// client, `+0x460`).
#[derive(Clone, Debug, PartialEq)]
pub struct N007InterfaceState {
    pub byte_290: u8,
    pub byte_291: u8,
    pub int_294: i32,
    pub float_298: f32,
    pub row_name_29c: String,
    pub float_2a4: f32,
    pub float_2a8: f32,
}

/// Evaluates `N007` interface state transfer (`0x1418a1ca0` / `0x1418b7e50`).
pub fn evaluate_n007_interface_transfer(
    owner_present: bool,
    owner_implements_interface: bool,
    state: &N007InterfaceState,
) -> Option<N007InterfaceState> {
    if owner_present && owner_implements_interface {
        Some(state.clone())
    } else {
        None
    }
}

/// State fields (`+0x1c0..+0x1eb`) transferred between `Owner` (`+0x2c4..+0x2ff`)
/// and `N026` in `RecordOwnerState` (`0x14192f030` server / `0x14192bec0` client,
/// `+0x448`) and `StitchingOperation` (`0x14193a990` server / `0x141936900`
/// client, `+0x468`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct N026StitchState {
    pub array_1c0: Vec<i32>,
    pub int_1d0: i32,
    pub array_1d8: Vec<i32>,
    pub int_1e8: i32,
}

/// Evaluates `N026` record/stitch state transfer (`0x14192f030` / `0x14193a990`).
pub fn evaluate_n026_stitch_transfer(
    owner_matches_n026_class: bool,
    state: &N026StitchState,
) -> Option<N026StitchState> {
    if owner_matches_n026_class {
        Some(state.clone())
    } else {
        None
    }
}

/// State fields (`+0x1c0..+0x1cb`) transferred between `Owner` (`+0x2c0..+0x2cb`)
/// and `N027` in `RecordOwnerState` (`0x14192f0c0` server / `0x14192bf50` client,
/// `+0x448`) and `StitchingOperation` (`0x14193aa20` server / `0x141936990`
/// client, `+0x468`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct N027StitchState {
    pub byte_1c0: u8,
    pub int_1c4: i32,
    pub int_1c8: i32,
}

/// Evaluates `N027` record/stitch state transfer (`0x14192f0c0` / `0x14193aa20`).
pub const fn evaluate_n027_stitch_transfer(
    owner_matches_n027_class: bool,
    state: N027StitchState,
) -> Option<N027StitchState> {
    if owner_matches_n027_class {
        Some(state)
    } else {
        None
    }
}

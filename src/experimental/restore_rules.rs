//! EXPERIMENTAL restoration rules the client's state recorders apply.
//!
//! Each function is a pure rule over authored domain values, read from the
//! client recorders' record, validate, stitch and restore code (F-645 to F-665,
//! F-677, F-715, F-716, F-717, F-727, F-728, F-732, F-733, F-734, F-735,
//! F-736).
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
/// Lengths must be equal. ASCII units compare case-insensitively and every
/// other unit exactly, so `"Foundation"` matches `"FOUNDATION"` but not a
/// differently accented spelling.
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

/// Whether a voxel record belongs to an owner at `actual` (F-645): every axis
/// within [`VOXEL_LOCATION_TOLERANCE`] inclusive. A NaN on either side never
/// matches. An owner without a root component is at the origin.
pub fn voxel_location_matches(saved: [f32; 3], actual: [f32; 3]) -> bool {
    saved
        .iter()
        .zip(actual)
        .all(|(saved, actual)| (actual - saved).abs() <= VOXEL_LOCATION_TOLERANCE)
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

/// The elapsed level seconds a restored game mode starts from (F-653).
pub fn restored_elapsed_seconds(saved_elapsed: i32, game_time: f32) -> i32 {
    if saved_elapsed < ELAPSED_SECONDS_FALLBACK_BELOW && game_time > 0.0 {
        truncate(game_time)
    } else {
        saved_elapsed
    }
}

/// A saved survival level, or `None` when it was not recorded (F-648).
pub fn recorded_level(saved: i32) -> Option<i32> {
    (saved != UNRECORDED_LEVEL).then_some(saved)
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

/// Whether a player or player-state record validates: its recorder version is
/// at least 1 and an active list does not lack its player.
pub fn player_record_valid(version: i32, list: PlayerList) -> bool {
    version >= 1 && !list.rejects()
}

/// Whether a rocket record validates: an active list does not lack its player.
pub fn rocket_record_valid(list: PlayerList) -> bool {
    !list.rejects()
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
        .find(|block| block.start <= now && now < block.end)
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

/// One cave-AI class's spawn times, on a spawner or in its record.
#[derive(Clone, Debug, PartialEq)]
pub struct CaveSpawnTimes {
    pub class_name: Vec<u16>,
    pub times: Vec<f32>,
}

/// Restores a cave-AI spawner's per-class spawn times from its record
/// (F-677). Each saved class with at least one time replaces the times of
/// the first configured class whose name matches it; saved classes without
/// times or without a match change nothing. The client compares with a
/// case-insensitive `Stricmp`; this rule folds ASCII only, as [`names_match`]
/// does, since its non-ASCII folding is unverified. Returns whether any saved
/// class had times: then the spawner's restored event runs, otherwise its
/// tick is re-enabled.
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

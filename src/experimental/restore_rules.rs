//! EXPERIMENTAL restoration rules the client's state recorders apply.
//!
//! Each function is a pure rule over authored domain values, read from the
//! client recorders' record, validate, stitch and restore code (F-645 to F-665,
//! F-677).
//! Nothing here decodes a save, looks up a class, constructs an object or
//! imports state: the rules pin the behavior a native world must reproduce when
//! it restores recorded actors. The normal server does not call them yet, and
//! an interpreted import stays gated (U-045).

/// Per-axis tolerance for matching a voxel record to its owner (F-645, M-644).
pub const VOXEL_LOCATION_TOLERANCE: f32 = 0.1;

/// Saved elapsed level seconds below this take the saved game time (F-653).
pub const ELAPSED_SECONDS_FALLBACK_BELOW: i32 = 31;

/// The survival-level value meaning "not recorded" (F-648).
pub const UNRECORDED_LEVEL: i32 = -1;

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
/// the clock that reads `now` (F-657): the truncated time left, or 0 when no
/// end is set or less than one unit remains.
pub fn remaining_cooldown(end: f32, now: f32) -> i32 {
    if end > 0.0 {
        let left = truncate(end - now);
        if left >= 1 {
            return left;
        }
    }
    0
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

# Recorder restoration rules

## Status

Experimental: pure rules in `experimental::restore_rules`, tested in
`tests/restore_rules.rs`; the server does not call them and an interpreted
import stays gated (U-045). Scope: client 25793767 recorder code, offsets and
all 65 `RecorderComponent` `FClassParams` equal in server 25621760 (F-727, F-728,
F-734 to F-736; U-020 to U-022, U-032, U-046, U-048, U-050 resolved).
Last reconciled: 2026-10-18.

## Scope

How recorders validate, stitch and restore known field values. Not covered:
save decoding, class lookup, object construction, script-side owner code and
data-table row meanings (F-660).

## Confirmed

- Across all 38 `StateRecorderBlobs` classes (`C001`..`C038`), `recorder_restore_dispatch`
  encodes the verified vtable table (`F-727`): all 38 inherit `UObject::Serialize`
  (`+0xa0`, `0x1418aaf70`); all 31 `UActorStateRecorderComponent` classes (`C001`
  subclasses) inherit `+0x488 = 0x14189d380` (`TreePrimitive` -> `Wood` when
  `RecordedVersion < 1`), while the 7 direct `UIcarusStateRecorderComponent`
  classes (`C013`, `C014`, `C022`, `C025`, `C035..C037`) inherit no-op `+0x488`;
  only `C017` overrides `+0x4a0` (`0x141400150`, zeroing pitch/roll on
  `ActorTransform.Rotation`), and only `C004` (`0x141888270`) and `C032`
  (`0x1418c5cd0`) override `+0x4b0` (`ApplyRecorderTransform`).
- Only 7 of the 38 `StateRecorderBlobs` classes override `Validate` (`vftable +
  0x408`, base `0x140899f70` server / `0x1408c9810` client returning `true`;
  F-652, F-663, F-680):
  - `UProspectSubsystem` (`0x141704d70` / `0x141715480`) embeds
    `ActiveProspectInfo` (`FProspectInfo`, `0xa0` B) at `+0x30`, with
    `HasActiveProspect` (`0x141820ba0` / `0x14182c4c0`) testing
    `!ActiveProspectInfo.ProspectID.IsEmpty()` (`+0x38 > 1`) and
    `IsPlayerStillAssociatedWithActiveProspect(PlayerID, ChrSlot)`
    (`0x1418231d0` / `0x14182ec60` -> `FindByKey` `0x1417fd560` / `0x141809720`)
    scanning `ActiveProspectInfo.AssociatedMembers` (`+0x98`, `0x40` B stride)
    for a matching `UserID` (`+0x20`, ASCII case-insensitive via `Stricmp`) and
    `ChrSlot` (`+0x30`) (`resolve_prospect_player_list`).
  - `C011` (`URocketSpawnRecorderComponent::Validate`, `0x1418fa010` /
    `0x1418fa340`) calls `ResetSpawnRecord` (`0x1418ed540` / `0x1418edf10`,
    clearing `PlayerID = ""` at `+0x1c8` and setting `ChrSlot = -1` at `+0x1d8`)
    when `RocketSpawnRecorderVersion < 1` or an active prospect lacks
    `(PlayerID, ChrSlot)`, then returns `true` (`validate_rocket_spawn_record`).
  - `C025` (`UPlayerRecorderComponent`, `0x1418f9de0` / `0x1418fa110`) and
    `C026` (`UPlayerStateRecorderComponent`, `0x1418f9e80` / `0x1418fa1b0`)
    require `Version >= 1` and `!list.rejects()`; `C029`
    (`URocketRecorderComponent`, `0x1418f9f70` / `0x1418fa2a0`) requires
    `!list.rejects()`, and `C029` restore applies only when an active prospect
    holds the player and `version > 0`, destroying the rocket otherwise (F-663).
  - `C028` (`UResourceDepositRecorderComponent::Validate`, `0x1418f9f20` /
    `0x1418fa250`) rejects `Type == "Super_Cooled_Ice"` (`0x144a71800` /
    `0x144bcddc0`, `resource_deposit_record_valid`).
  - `C031` (`USpawnedVoxelRecorderComponent`) and `C034`
    (`UVoxelRecorderComponent`, `0x1418fa0e0` / `0x1418fa410`) reject
    `OwnerResolvePolicy == RespawnOnly (1) && bFullyMined` (F-680).
- Stitching takes the first spawned actor with the saved Icarus id and a
  matching name (F-650, F-676): equal lengths, ASCII case-insensitive, other
  units exact (F-663). Both `FString::Equals(..., ESearchCase::IgnoreCase)`
  (`0x141e48d20` / `0x141e39920`) and `FGenericPlatformStricmp::Stricmp`
  (`0x141e48e20` / `0x141e39a00`) test `(c1 | c2) & 0xffffff80 == 0`
  (`BothAscii`), fold ASCII code units `< 0x80` through `LowerAscii`
  (`0x144e8e1a0` / `0x144fdbb80`), and compare code units `>= 0x80` directly
  without non-ASCII case folding.
- A voxel record owns the actor within 0.1 per axis, inclusive (F-645).
- `C002` teleport cooldowns read `GetReplicatedProspectGameTime` (`0x1418d9390` =
  `GetTimeSeconds` + `GameState + 0x398`), saving the truncated time left or 0
  (F-657, F-734). `C035` weather controller keeps events with
  `row_duration + start <= CurrentBlockEndTime()` (`0x14190a5a0`) and saves/restores
  `TimeElapsed` against `GetLevelTimeElapsedSec` (`0x1414e1390`, F-734).
- `C001` (`0x1418ba040`) maps saved `ModifierLifetime == -1` to `100`, restores
  `D01` slots (`0x1418b8f20`) via 14 `EDynamicItemProperties` (`0x145e0c090`),
  upgrades `v < 2` ammo (`0x1418a3530`), routes rejected slots to `ForceAddItems`
  (when empty) or `Inventory + 0x288`, and restores `CPF_SaveGame` variables and
  `CPF_RepNotify` callbacks (`0x1418bcff0`, F-735).
- `C033` tree respawn (`0x1418ee140` -> `0x141727e20`) runs `OnConstruction`
  (`+0x2c1 = 1`) and `BeginPlay` -> `ConstructTreePrimitives` (`0x141726b40`) ->
  `OnConstructedTreePrimitives` (`0x141dc0680`), where `BP_TreeBase_C` calls
  `execEndRecording` twice when `bIsPhysicsDynamic` (F-681, F-736).
- Saved elapsed level seconds under 31 take the positive saved game time (F-653).
- Survival level -1 is unrecorded (F-648); a mount keeps only its yaw (F-651).
- Container entries keep their list positions, empty ones skipped (F-651).
- A forecast saves `now`'s offset into its first containing block and that
  block's pattern; else `now` and the first pattern; zeros if none (F-665).
- Each saved cave-AI class with times (`C005`, `0x14141e320`) unwraps
  `"BlueprintGeneratedClass"` via `ClassGeneratedBy->GetName()` (`+0x208`,
  `resolve_cave_ai_class_name`), replaces the times of the spawner's first
  configured class equal by `Stricmp` (`0x141e48e20` / `0x141e39a00`), and fires
  the script event (F-677).
- A sign saves/restores text, color and icon via its sign interface (F-677).

## Candidate

- Float-to-int steps follow x86 truncation (`i32::MIN` when out of range); the
  engine's trigonometric approximations are not reproduced bit for bit.

## Open questions

- Packaged data-table row meanings stay out of scope by policy (F-660).

## Evidence

F-645, F-648, F-650 to F-653, F-657, F-660, F-663, F-676, F-677, F-680, F-681,
F-716, F-727, F-728, F-734 to F-736; M-642, M-666, M-682, M-683, M-687 to M-692,
M-694, M-695, M-698, M-755, M-756, M-768, M-769, M-774, M-775, M-781, M-782.

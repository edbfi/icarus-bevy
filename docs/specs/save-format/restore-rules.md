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
- Stitching takes the first spawned actor with the saved Icarus id and a
  matching name (F-650, F-676): equal lengths, ASCII case-insensitive, other
  units exact (F-663).
- A voxel record owns the actor within 0.1 per axis, inclusive (F-645). A
  fully mined voxel with a respawn-only owner policy (0..3; both builds, M-627)
  fails validation and is dropped on restore (F-680).
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
- Player records need version 1; an active player list lacking the player fails
  player, player-state and rocket records; rocket restore applies only with an
  active list holding the player and a positive version, else destroys (F-663).
- Container entries keep their list positions, empty ones skipped (F-651).
- A forecast saves `now`'s offset into its first containing block and that
  block's pattern; else `now` and the first pattern; zeros if none (F-665).
- Each saved cave-AI class with times replaces the times of the spawner's
  first configured class equal by `Stricmp`, firing the script event (F-677).
- A sign saves/restores text, color and icon via its sign interface (F-677).

## Candidate

- Float-to-int steps follow x86 truncation (`i32::MIN` when out of range); the
  engine's trigonometric approximations are not reproduced bit for bit.
- Cave-AI names fold ASCII case only; `Stricmp`'s non-ASCII folding is unread.

## Open questions

- Packaged data-table row meanings stay out of scope by policy (F-660).

## Evidence

F-645, F-648, F-650 to F-653, F-657, F-660, F-663, F-676, F-677, F-680, F-681,
F-716, F-727, F-728, F-734 to F-736; M-642, M-666, M-682, M-683, M-687 to M-692,
M-694, M-695, M-698, M-755, M-756, M-768, M-769, M-774, M-775.

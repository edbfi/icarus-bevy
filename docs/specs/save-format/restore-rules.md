# Recorder restoration rules

## Status

Experimental: pure rules in `experimental::restore_rules`, tested in
`tests/restore_rules.rs`; the server does not call them and an interpreted
import stays gated (U-045). Scope: client 25793767 recorder code, offsets equal
in server 25621760. Last reconciled: 2026-10-15.

## Scope

How recorders validate, stitch and restore known field values. Not covered:
save decoding, class lookup, object construction, script-side owner code and
data-table row meanings (F-660).

## Confirmed

- Stitching takes the first spawned actor with the saved Icarus id and a
  matching name (F-650, F-676): equal lengths, ASCII case-insensitive, other
  units exact (F-663).
- A voxel record owns the actor within 0.1 per axis, inclusive (F-645). A
  fully mined voxel with a respawn-only owner policy fails validation, so its
  record is dropped on restore; the original server's offline load did exactly
  this, silently (F-680).
- A teleport saves the truncated time left to its cooldown end, 0 when unset or
  under one unit, restored as a new countdown (F-657).
- Saved elapsed level seconds under 31 take the positive saved game time (F-653).
- Survival level -1 is unrecorded (F-648); a mount keeps only its yaw (F-651).
- Player records need version 1; an active player list lacking the player fails
  player, player-state and rocket records; rocket restore applies only with an
  active list holding the player and a positive version, else destroys (F-663).
- Container entries keep their list positions, empty ones skipped (F-651).
- A forecast saves `now`'s offset into its first containing block and that
  block's pattern; else `now` and the first pattern; zeros if none (F-665).
- Each saved cave-AI class with times replaces the times of the spawner's
  first configured class equal by `Stricmp`; any such class fires the
  restored script event, otherwise tick resumes (F-677).
- A sign saves and restores text, color and icon row name only through its
  script-capable sign interface, when implemented (F-677).

## Candidate

- Float-to-int steps follow x86 truncation (`i32::MIN` when out of range); the
  engine's trigonometric approximations are not reproduced bit for bit.
- Cave-AI names fold ASCII case only; `Stricmp`'s non-ASCII folding is unread.

## Open questions

- A physics-dynamic tree record ends on restore and is not saved again; how
  the tree prefab's runtime creation decides it is unread (F-681).
- Cooldowns read world seconds plus a game-state offset (F-665); the game
  time's clock and script-side owner saves stay open (U-020).

## Evidence

F-645, F-648, F-650 to F-653, F-657, F-660, F-663, F-676, F-677, F-680, F-681;
M-642, M-666, M-682, M-683, M-687 to M-692, M-694, M-695.

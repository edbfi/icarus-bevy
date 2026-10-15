# Recorder restoration rules

## Status

Experimental: pure rules in `experimental::restore_rules`, tested in
`tests/restore_rules.rs`; the server does not call them and an interpreted
import stays gated (U-045). Scope: client 25793767 recorder code, offsets equal
in server 25621760. Last reconciled: 2026-10-15.

## Scope

How recorders validate, stitch and restore known field values. Not covered:
save decoding, class lookup, object construction, owner-side save code and
data-table row meanings (F-660).

## Confirmed

- Stitching takes the first spawned actor with the saved Icarus id and a
  matching name (F-650, F-662): equal lengths, ASCII case-insensitive, other
  units exact (F-663).
- A voxel record owns the actor within 0.1 per axis, inclusive (F-645).
- A teleport saves the truncated time left to its cooldown end, 0 when unset or
  under one unit, restored as a new countdown (F-657).
- Saved elapsed level seconds under 31 take the positive saved game time (F-653).
- Survival level -1 is unrecorded (F-648); a mount keeps only its yaw (F-651).
- Player records need version 1; an active player list lacking the player fails
  player, player-state and rocket records; rocket restore applies only with an
  active list holding the player and a positive version, else destroys (F-663).
- Container entries keep their list positions, empty ones skipped (F-651).

## Candidate

- Float-to-int steps follow x86 truncation (`i32::MIN` when out of range); the
  engine's trigonometric approximations are not reproduced bit for bit.

## Open questions

- Cooldowns read world seconds plus a game-state offset (F-665); the game
  time's clock and script-side owner saves stay open (U-020).

## Evidence

F-645, F-648, F-650 to F-653, F-657, F-660, F-662, F-663; M-666, M-682, M-683.

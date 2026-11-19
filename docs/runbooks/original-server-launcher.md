# Original-server offline launcher

## Current launcher

Use a current case and session under `work/` for `tools/run_reference_offline.py`
or its shell wrapper. See the [runtime gates](../gates.md#before-any-original-server-or-client-run).
In `scope.md`, set `status: granted` under `## auth`, `mode: offline` under
`## network_profile`, and `ready_for_act: true` under `## signoff` as Markdown
list fields. The launcher rejects duplicate fields and rechecks the scope,
session and input hashes before launch. The session JSON requires these fields.

| Field | Required value |
| --- | --- |
| `scope` | `reference-offline` |
| `authorization_source` | Nonempty reference to the applicable user instruction |
| `case_root` | Absolute case directory |
| `scope_sha256` | Lowercase SHA-256 of that case's scope file |
| `start_utc`, `end_utc` | Timezone-aware timestamps covering the entire attempt |
| `image` | Exact pinned image string shown below |
| `reference_root` | Absolute repo-root `icarus-orig` or `icarus-server-<digits>` directory, or the approved intake below |
| `reference_executable` | Absolute `Icarus/Binaries/Win64/IcarusServer-Win64-Shipping.exe` below that root |
| `reference_sha256` | Lowercase SHA-256 of that executable |
| `observation_seconds` | 15–780, matching `--seconds` |
| `run_timeout_seconds` | At least observation + 120, at most 900 |
| `max_runs` | 1–12 attempts; failed attempts also consume quota |
| `allowed_modes` | Selected base mode: `startup`, `resume`, `missing`, `truncated-json` or `save-load`; add `handshake` / `complete-stateless` if needed |

For `handshake`, bind a case-local executable `probe` and `probe_sha256`, and
allow at least 60 observation seconds. `--complete-stateless` requires
`--handshake-probe` and both mode permissions. Mounts include only the probe file.

The pinned image is
`ghcr.io/pelican-eggs/steamcmd@sha256:967e40780cb46a5343bd7a5fd372612d1a1b90fbc95fdd803d74ec5b3bede237`.
Use canonical paths without commas or newlines. Legacy bindings hash only the
executable. Retain the inventory separately.

For a startup observation, run the launcher with the case-local session.

```bash
bash tools/run_reference_offline.sh \
  --case-root "$PWD/work/current-reference-case" \
  --session "$PWD/work/current-reference-case/session.json" \
  --run-name startup-01 --seconds 120
```

The launcher holds the case quota lock through cleanup and records the
invocation, hashes, inspections, logs, resource samples and `cleanup.json`.
Session expiry caps the deadline, including 120 seconds reserved for cleanup.
Cleanup verifies container ownership by token, name and ID before removal.
Interruption still triggers removal. Investigate inspection or removal failures before another run.

The offline run disables networking, published ports, downloads, identity overrides
and reference writes. The pinned image is local-only. CPU, memory and PID
limits and the 3 GiB available-memory / 30 GiB disk floors apply. See the
[connected launcher](connected-scenarios.md) for connected scenarios.

`--hello-probe` (with `--handshake-probe`, without `--complete-stateless`)
runs the probe with `--hello <hello_version>`; the session allows `hello`
beside `handshake` and binds `hello_version`, an unsigned 32-bit value.
`--join-probe` adds `--join <join_seconds>` (30 to 600): synthetic ids log in,
join and hold the connection from `probe_delay_seconds` after start (F-858);
allow `join` too. `--initialise-probe` (with `--join-probe`) adds
`--initialise --player-id <id> --chr-slot <n>`: the probe logs in as that
player id, initialises as a stock client does (character slot, friends list,
profile, loadout and bestiary answers; F-973, F-974) and keeps recording. The
session allows `initialise` beside `join` and binds `initialise_player_id`
(1 to 64 letters, digits, `_` or `-`, starting with a letter or digit) and
`initialise_chr_slot` (0 to 99). A saved character's id is private and stays
in the case.

## M-1404 intake

For a new offline case under F-1175, use `--reference-input M-1404`.
Set the session’s `reference_input` fields:

- `evidence` = `M-1404`, `build_id` = `26613688`, `release` = `3.0.36.159996-rel-DangerousHorizons`
- `manifest` (absolute path) and `manifest_sha256` from M-1404
- `authorization_source` for the current run

Repeat these fields, `status: granted`, `reference_root`, `reference_executable`
and `reference_sha256` as list entries under `## reference_input` in `scope.md`.
Use the fixed sibling `official/` install. Keep it quiescent during verification
and execution. Allow five minutes for hashing. Unset Docker host/context overrides
and use the local root daemon at `/run/docker.sock`. The mapped image user
needs read access to the install and write access to the private 0700 run home.

## Save-load mode

`--save-load` loads one disposable save copy. The session adds `save-load` to
`allowed_modes` and binds `save_input` (a case-local `<prospect>.json`: letters,
digits, `_` and `-`), its `save_input_sha256` and
`shutdown_if_not_joined_seconds` (0 to 780; the dedicated-server wiki gives 0
as "as soon as possible" and negative values as never, which the launcher
rejects). The launcher copies the file into the run's own
`home/Icarus/Saved/PlayerData/DedicatedServer/Prospects/`, re-verifies the
copy's digest, writes a `ServerSettings.ini` with `ResumeProspect=False` and
both shutdown timers set to that value, and adds `-LoadProspect=<prospect>`.
With no player joining, the server returns to its lobby after the timer. After
cleanup, `save-outputs.json` digests every file left under the run's
`PlayerData` tree and states whether the input survived unchanged. Never point
`save_input` at `reference-data/`; extract a copy into the case first.

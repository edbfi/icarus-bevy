# Original-server offline launcher

Runbook copied from the "Current launcher" section of the retained
[offline runtime record](../history/offline-runtime-authorization.md). Each
experiment needs its own concrete current scope under the user's standing
instruction; historical session files do not renew themselves.

## Current launcher

`tools/run_reference_offline.py` and its shell wrapper require an explicit
case-local session. The user's current autonomous-work instruction permits bounded
offline research; create a concrete current scope for each experiment under that
authorization. Historical session files do not renew themselves. No original
server was executed while testing this launcher revision.

Use an existing case below the repository's ignored `work/` directory, with a
`scope.md` checked by the project launcher itself. Its Markdown sections must
contain `## auth` with `- status: granted`, `## network_profile` with
`- mode: offline`, and `## signoff` with `- ready_for_act: true`. Duplicate
contract fields are rejected. These checks run again before launch alongside the
session/hash checks; no external skill package is required. The session JSON must
contain:

| Field | Required value |
| --- | --- |
| `scope` | `reference-offline` |
| `authorization_source` | Nonempty reference to the applicable user instruction |
| `case_root` | Absolute case directory |
| `scope_sha256` | Lowercase SHA-256 of that case's scope file |
| `start_utc`, `end_utc` | Timezone-aware timestamps covering the entire attempt |
| `image` | Exact pinned image string shown below |
| `reference_root` | Absolute repository `icarus-orig` directory |
| `reference_executable` | Absolute `Icarus/Binaries/Win64/IcarusServer-Win64-Shipping.exe` below that root |
| `reference_sha256` | Lowercase SHA-256 of that executable |
| `observation_seconds` | 15–780, matching `--seconds` |
| `run_timeout_seconds` | At least observation + 120, at most 900 |
| `max_runs` | 1–12 attempts; failed attempts also consume quota |
| `allowed_modes` | Selected base mode: `startup`, `resume`, `missing` or `truncated-json`; add `handshake` / `complete-stateless` if needed |

When `handshake` is allowed, also provide `probe` (an absolute executable file
inside the case) and its `probe_sha256`. A handshake invocation requires at least
60 observation seconds. `--complete-stateless` requires `--handshake-probe` and
both mode permissions. No probe directory is mounted.

The pinned image is
`ghcr.io/pelican-eggs/steamcmd@sha256:967e40780cb46a5343bd7a5fd372612d1a1b90fbc95fdd803d74ec5b3bede237`.
Paths must not contain redirects, commas or newlines. The launcher verifies the
scope, executable and optional probe hashes again before launch and rejects any
session-byte change during preparation. Executable hashing does **not** verify
the entire mounted installation; retain the reference inventory separately.

After preparing the case and session, a startup invocation is:

```bash
bash tools/run_reference_offline.sh \
  --case-root "$PWD/work/current-reference-case" \
  --session "$PWD/work/current-reference-case/session.json" \
  --run-name startup-01 --seconds 120
```

The launcher locks the case quota through cleanup and never overwrites a run.
It records invocation/session/helper hashes, container inspection, logs, resource
samples and `cleanup.json`. The recorded timeout is a maximum: a fresh pre-launch
time check caps the monotonic deadline at session expiry and rejects attempts
without time for observation plus 120 seconds of cleanup. Container ownership
requires a random token, the exact name and, once known, the exact ID. Removal
targets that verified ID; inspection errors or failure to verify removal are
errors, not successful cleanup.
Repeated interruption during cleanup does not skip removal. Local subprocess
deadlines cannot guarantee daemon cessation after a host or daemon failure;
an unsuccessful cleanup receipt requires investigation before another run.

Networking remains disabled, with no published ports, downloads, identity overrides
or reference writes. The pinned image is local-only; CPU/memory/PID limits and

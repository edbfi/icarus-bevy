> Historical record, retained verbatim for provenance. Not a current instruction or approval.
> Index: [authorizations-index.md](authorizations-index.md). Moved from `docs/` on 2026-10-13.

# Offline reference runtime scope and launcher

Follow the [canonical plan](../linux-dedicated-server-plan.md) and
[implementation status](../implementation-status.md) for current priorities and
evidence. The historical authorization below does not renew an execution window.

## Current launcher (2026-10-12)

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
3 GiB available-memory / 30 GiB disk floors remain enforced. The launcher has no
mode for mounting private user saves. Connected Windows-client tests and matched
save-load comparisons require their own concrete experiment setup.

## Historical first-startup authorization (2026-10-04)

The following records the original approval, not a current executable recipe.
The reverse-skill package was removed on 2026-10-12; its commands and package
requirements below are historical. The upstream `AGENTS.md` link identifies the
former revision.

Status: approved by the user on 2026-10-04. This was the first execution of the original server.
It cannot prove stock-client compatibility because the container has no network.

Docker 29.8.2 is available. The user's Proton image is already installed at digest
`sha256:967e40780cb46a5343bd7a5fd372612d1a1b90fbc95fdd803d74ec5b3bede237`.
The image runs as `container` with its existing home at `/home/container`.

### Historical commands proposed for approval

```bash
bash .agents/vendor/reverse-skill/skills/scripts/case-init.sh \
  --hint "game reverse ICARUS offline startup" \
  --case-name icarus-offline-startup --project-root "$PWD" \
  --auth-granted --auth-basis own_system \
  --evidence-of-auth "User approval of docs/offline-runtime-authorization.md" \
  --sample "$PWD/icarus-orig/Icarus/Binaries/Win64/IcarusServer-Win64-Shipping.exe" \
  --in-scope-asset "$PWD/icarus-orig" --network-profile offline
```

Complete the scope with `offline_execution`, local log inspection and resource
measurement, roles `lead`, `cre`, `doc`, and explicit exclusion of network access,
asset extraction, instrumentation, real players and production saves. Then run:

```bash
timeout 90s bash tools/run_reference_offline.sh
```

Review the exact Docker command in the script before approval. Effects:

- Create a separate ignored case and writable Proton prefix/runtime directory.
- Start one container from the pinned local image, with no pull or downloads.
- Run the original server under Proton, with original installation mounted read-only.
- Disable container networking and publish no host ports; attempted external
  connections cannot leave the network namespace.
- Limit CPU, memory and process counts; write only temporary/container state and
  local case artifacts. The case directory is mounted at the image's existing home.
- Observe for roughly 30–45 seconds, save container logs/inspection/resource data,
  stop the exact created container, and remove it. Preserve all case artifacts.

Expected startup/network failures are observations, not confirmed original bugs.
This does not authorize downloading SDKs, opening ports, using real save data, or
starting connected client tests. The independent replacement remains native Bevy.

The vendored [AGENTS.md](https://github.com/zhaoxuya520/reverse-skill/blob/cab634bd855fc287f6e420c1f36fd1a6b9245960/AGENTS.md) requires explicit
script approval, and the original case explicitly excludes server execution. This
batch therefore needs separate approval and its own target scope.

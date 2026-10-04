# Independent ICARUS Linux dedicated server

This is the reviewed implementation plan. Progress and evidence are tracked in
[implementation-status.md](implementation-status.md); incomplete milestones are
not claims of compatibility. This document authorizes no vendored script or
target activity by itself.

## Goal and agreed constraints

Build an independent native Linux dedicated server with authoritative simulation
in Rust and Bevy. Demonstrate compatibility with unmodified ICARUS clients,
existing-save import, reliable persistence, improved stability, and improved
performance for a pinned official client/server release.

Investigation priorities, in order:

1. Crashes, hangs, and save integrity.
2. Long-session degradation.
3. Joining, reconnecting, and session cleanup.
4. Large worlds: substantial bases, tamed animals, production, environmental
   activity, and many players.

A handshake or small playable slice is an intermediate milestone. Completion
requires all applicable server behavior in the pinned release, including supported
content variants, with an explicit compatibility matrix.

### Independence and provenance

- Independently implement simulation, protocols, persistence, and server data.
- Use Bevy for the application, ECS, and scheduling foundation. Legitimate Linux
  platform SDKs may provide authentication and discovery.
- The replacement must not depend on the original server executable, Unreal
  runtime, Wine, or Proton.
- Do not extract or reuse original art, audio, terrain, collision, navigation,
  or gameplay data tables. Record provenance of independently authored data.
- Record necessary protocol identifiers and experimentally established behavior
  in compatibility specifications; do not bulk-convert original assets.
- Save import may consume player-created state, but not packaged game assets.
- Stock clients retain their normal game installations and assets.
- Preserve the repository license and third-party notices. Verify platform SDK
  distribution constraints before selecting and shipping an integration.

[benilla](https://github.com/samwhosung/benilla) and
[Skate 3 Rust Engine](https://github.com/SK8-ENGINE/skate-3-rust-engine) illustrate
independent implementations, but both use original game data in ways excluded
here. They are architectural references, not proof of asset-free feasibility.

**Early gate:** demonstrate that stock clients correctly interact with independently
authored server world data. If original data is indispensable, document the blocker
and keep the full goal unmet. Do not substitute modified clients, asset extraction,
or reduced functionality without an explicit scope decision.

### Toolchain

Use Rust **1.99.0**, verified current stable on 2026-10-04, and Bevy **0.19.1**.
Bevy declares Rust 1.95.0 as its minimum. Build, test, and runtime checks must
demonstrate actual compatibility with 1.99.0.

Pin Rust in `rust-toolchain.toml` with matching Rustfmt and Clippy; use the same
toolchain for development, CI, benchmarks, and releases. Commit `Cargo.lock` at
initialization. Adopt newer stable Rust through tested updates rather than a
floating release-build toolchain. Use version-matched Bevy documentation.

Sources: [Rust 1.99.0](https://blog.rust-lang.org/2026/10/01/Rust-1.99.0/),
[Bevy 0.19.1](https://crates.io/crates/bevy/0.19.1).

## Repository, tooling, and authorization

The initial repository had no root Cargo manifest or lockfile. Keep these boundaries:

| Location | Contents |
| --- | --- |
| `icarus-orig/` | Ignored original server files and preserved installations |
| `work/` | Ignored cases, captures, dumps, private saves, generated analysis and benchmarks |
| `src/`, `tests/`, `benches/`, `tools/` | Reusable implementation, authored tests and research tools |
| `docs/` | Plan, specifications, architecture decisions, sanitized findings and reports |

Keep reference installations immutable during experiments; place runtime data and
disposable copies in the case directory. Commit only authored or reviewed,
redistributable fixtures. Work on feature branches, use Conventional Commits, and
retain the prohibition on direct commits to `main`.

### Research workflow

Read canonical vendored `skills/SKILL.md`, `skills/MASTER-ROUTING.md`, and
`skills/routing.md`. Proposed overall PRIMARY is R0 `reverse-engineering`, because
the work concerns recovering game-server behavior. Confirm through the approved
router. Use these specialist handoffs:

| Work | Skill |
| --- | --- |
| Binary triage and behavior | `reverse-engineering` |
| Headless decompilation and cross-references | `ghidra-reverse` |
| Binary inspection and cross-version comparison | `radare2`, `binary-diff` |
| Transport, replication and sessions | `protocol-reverse` |
| Evidence review and reports | `case-review`, `docs-generator` |
| Independent replacement | Repository `bevy` skill |

Assign lead, reverse-analysis, and documentation responsibilities using the role
map; roles do not require separate agents.

The vendored `AGENTS.md` and `CLAUDE.md` separate script execution approval from
target authorization. Reading the package grants neither. Disclose exact commands
and expected writes, downloads, processes, network access and configuration changes
before running package scripts. Continue already approved deterministic steps;
new side-effect categories require new disclosure and approval.

[research-authorization.md](research-authorization.md) contains the concrete first
batch. Initialize `work/icarus-server-baseline/scope.md`, record explicit assets,
activities, authorization evidence, exclusions and network profile. Target work
requires `auth.status=granted`, readiness, and a successful canonical case guard.
Presets and force flags cannot manufacture authorization.

Separate offline and dynamic scopes. Identify legitimate external services for
online authentication; lab-only and offline scopes do not permit those connections.

### Tools and reference environments

Consult `skills/tool-index.md` for machine-local paths. Its initial inventory
records radare2, headless Ghidra, Java and Wireshark CLI tools; IDA and Frida are
not recorded as available. Use installed tools first. Prefer Homebrew and the
existing `Brewfile`; preserve vendor layout, revision metadata and licenses.

After approval, refresh inventory from the project root with:

```bash
PATH="$(brew --prefix openjdk@21)/bin:$PATH" \
  bash .agents/vendor/reverse-skill/skills/scripts/refresh-tool-index.sh
```

Check `tshark --version` separately. A GUI probe failure does not mean headless
Ghidra is broken. Resolve the arguments before presenting an analysis command:

```bash
JAVA_HOME="$(brew --prefix openjdk@21)/libexec" \
  "$(brew --prefix ghidra)/libexec/support/analyzeHeadless" <arguments>
```

Maintain three environments:

1. Official server under GE-Proton using the user's Pelican reference at
   `/home/gvps/github/edbfi/pelican-eggs/games-steamcmd/icarus-proton`.
2. Official Windows server using the user's home machine for behavior and fault attribution.
3. Native replacement on Linux with matched workloads and resource limits.

The inspected Pelican egg uses `xvfb-run` and Proton, installation app ID 2089300,
runtime game ID 1149460, and persistent data under `Icarus/Saved`. Pin container
digest, Proton version, official build and configuration; disable automatic
updates in experiments. Do not run its container installer unchanged on the host.

## Milestones and exit criteria

### M0: Frozen baseline and evidence tracking

After authorization, inventory and hash binaries and dependencies. Record
client/server build IDs, manifests, configurations, content and environment versions.
Identify architecture, engine version, networking dependencies, platform services,
save locations and startup sequence. Preserve failing worlds and logs first.

Maintain a compatibility matrix with unknown, observed, specified, implemented and
validated states. Scenarios define starting state, actions, timing, observations,
cleanup and artifact collection.

Use Evidence → Finding → Path with hashes, binary-relative locations, captures,
logs, reproduction commands, confidence and work-item references. Keep timelines
append-only; prefer independent static and dynamic evidence before validation.

Exit: reproducible startup, verified build pairing, preserved state, actionable
feature/failure matrices.

### M1: Bug reproduction and performance baseline

| Priority | Campaign | Evidence |
| --- | --- | --- |
| 1 | Startup, load, autosave, shutdown, interruption, recovery | Dumps, hang stacks, save generations, recovered state, exit reasons |
| 2 | Repeated cycles and long sessions | Memory, entities, sessions, handles, queues, CPU, tick/save latency |
| 3 | Join, disconnect, timeout, reconnect, overlapping events | Ownership, duplicates, obsolete jobs, replication and authentication state |
| 4 | Large bases, tame populations, production, combat, storms, players | AI/structural costs, replication volume, persistence stalls |

For each candidate, define expected/observed behavior, minimize state and actions,
record failures/attempts, compare Windows/Proton where possible, and correlate
runtime evidence with control flow. Distinguish original-server defects, Proton
faults, resource exhaustion, configuration errors and unresolved hypotheses.
Link a replacement regression test to every confirmed bug. Preserve valid client
behavior while fixing failures; a single crash-free run is insufficient.

Exit: baseline measurements, ranked bug ledger and repeatable tests. Unreproduced
reports remain unresolved.

### M2: Protocol, platform and world-data feasibility

Recover discovery, queries, negotiation, authentication, identity and session
lifecycle; framing, bit encoding, reliability, ordering, acknowledgments,
fragmentation, compression and protection; object/class identifiers, RPCs,
replication, relevancy, dormancy and initial synchronization; world selection,
possession, movement correction and save loading; external service dependencies.

Do not assume generic Unreal networking is sufficient. Use controlled actions,
paired captures, logs and serializer cross-references to establish the actual
version and extensions. Instrument only approved laboratory processes. Preserve
unknown fields and transitions as unknown.

Demonstrate three gates:

1. Stock client authenticates and enters a session served by independent code.
2. It loads a representative region with independently authored server data and
   correct collision, interaction and replication, including static references and
   terrain interactions. A flat synthetic world or replay alone is insufficient.
3. A small official save imports without importing packaged game assets.

Exit: evidence-backed transport, service and world-data feasibility, or a precise
blocker report and next discriminating experiment.

### M3: Bevy foundation and persistent gameplay slice

Initialize Cargo; implement configuration, lifecycle, health and diagnostics.
Then implement actual stock-client sessions, player spawn, movement, interaction,
inventory mutation, save/restart/reconnect, and consistency between two clients.
Use independently authored data and legitimate platform integrations.

Exit: native Linux gameplay survives persistence and reconnect with verified
authoritative state. A headless foundation alone does not complete M3.

### M4: Complete server behavior matrix

Implement in dependency order: world identity/coordinates/collision/spatial queries/
time/movement; player lifecycle/vitals/effects/death/progression; items/inventory/
equipment/storage/crafting/production; construction/support/damage/repair/connected
systems; wildlife/combat/navigation/taming/mounts/ownership; weather/exposure/
resources/environment; prospects/missions/objectives/transitions/rewards/game modes;
administration/configuration/discovery/operations.

Inventory additional features and content variants from the pinned release. Each
subsystem requires a specification, data provenance, implementation, stock-client
scenarios, persistence coverage and regression evidence. This order authorizes no
omissions. Seed tests from [official documentation](https://github.com/RocketWerkz/IcarusDedicatedServer/wiki),
then verify against the frozen build, including its documented eight-player limit.

Exit: all applicable matrix entries validated, no unresolved required behavior.

### M5: Migration and reliability hardening

Recover supported formats, versions, IDs, relationships and validation rules.
Import copies into new destinations; preserve originals. Preserve ownership,
inventories, structures, animals, missions and player-created world changes. Reject
incomplete imports with field-level reports; never silently discard required data.
Verify imported worlds through stock-client play and save/reload.

Test interrupted writes, full disks, permissions, corruption, obsolete asynchronous
results, termination and restart loops. Re-run all confirmed original failures.

Exit: migration fidelity, crash consistency, bounded failure handling, bug regressions.

### M6: Optimization, qualification and packaging

Optimize behavior covered by compatibility tests: profile, identify a bottleneck,
change one mechanism, compare correctness/performance. Package native Linux
releases, services/containers, a native Pelican egg, migration/diagnostic guidance
and qualification reports.

Exit: all release gates pass; earlier milestones support experimental releases only.

## Architecture and interfaces

Start with one root package, library and `icarus-server` binary. Use subsystem
modules/plugins; split crates only for concrete boundaries. Disable Bevy defaults;
start with `std`, `async_executor`, `multi_threaded`, `bevy_state`, `bevy_log` and
`MinimalPlugins`. Explicitly install services when used. No shipping display, GPU,
audio, original installation or compatibility layer dependency.

Flow: bounded network decoding → session/command validation → fixed-step Bevy
simulation → replication and persistence output.

Separate protocol codecs/version profiles; identity/admission/reconnect/platform
callbacks; authoritative simulation; replication; versioned persistence; and
operations. Never expose Bevy runtime `Entity` through wire, save or SDK interfaces.
Use domain IDs, session generations and explicit network mappings.

Blocking work uses bounded queues and owned results, never background mutations of
the ECS world. Discard obsolete results. Configure `FixedUpdate` from evidence;
order input, movement, gameplay, structural changes and extraction explicitly.
Preserve independent parallelism and apply deferred commands before consumers.
Use simulation time for gameplay, real monotonic time for deadlines. Bound catch-up
and backlog, reject new admissions under overload, and expose missed deadlines.
Seed randomness and stabilize command order for replay; demonstrate determinism.
Validate physics/navigation semantics before accepting a backend or optimization.

CLI interfaces:

- `icarus-server run`
- `icarus-server check-config`
- `icarus-server import-save --source … --destination … --dry-run`

Precedence: explicit CLI overrides native configuration, which overrides defaults.
Provide official-settings import/mapping and report unsupported settings.

Persistence uses versioned domain snapshots: coherent simulation boundary,
worker serialization, checksummed new generation, synchronized storage, atomic
manifest publication, preceding valid generation retained. Autosave defaults to
60 seconds. Acknowledge only durable publication; validate/migrate before activation.
Reverse export to official saves is not promised. Rollback uses original backups;
replacement-only progress is not assumed transferable.

Bound parsing/reassembly/peer queues/jobs. Invalid commands must not mutate state;
storage/input failures must not panic. Make teardown idempotent. Expose tick
progress, deadline misses, queues, save age/failures, sessions and subsystem cost.
Separate liveness/readiness. Shutdown stops admission, drains bounded work, saves
and exits with a timeout. Unexpected restarts count as reliability failures.

For large worlds, measure spatial indexing, replication relevancy, AI scheduling,
incremental structure updates and snapshots. Sleeping or slower updates must
preserve verified gameplay semantics.

## Validation, release and maintenance

Test protocol vectors and malformed/duplicate/reordered/fragmented inputs;
controlled-time Bevy schedules, deferred changes, cleanup and obsolete jobs;
gameplay invariants and concurrent actions; persistence migration, relationships,
corruption, failed/interrupted storage and recovery; actual stock-client flows;
original bug regressions, churn, pressure, long soaks and workload performance.
Synthetic traffic supplements real clients, never replaces interoperability proof.
Differential tests compare semantic results; normalize nondeterminism only where
specified, and require byte equality only for established wire encodings.

Benchmark small/representative/heavy worlds with bases, production, tame AI,
travel, weather, saving and joins. Start at 1/4/8 players subject to verified limits;
above-limit capacity is separate. Freeze resources, state, actions, seeds, storage
and network. Run at least five repetitions after warm-up and retain raw results.
Compare matched Linux/Proton workloads, comparable Windows results, and before/
after replacement builds. Separate platform effects; synthetic-only wins are not
full-world improvements.

| Outcome | Release gate |
| --- | --- |
| Compatibility | All required scenarios pass for declared build/content |
| Independence | Runs with original directories unavailable and no prohibited data |
| Save fidelity | Complete supported import, no silent loss, verified play/save/reload |
| Durability | No loss of completed acknowledged saves; valid abrupt-failure recovery |
| Recovery point | Normally at most 60 seconds unsaved progress; overruns surfaced |
| Stability | Three independent 72-hour heavy runs without crashes/hangs/restarts |
| Cleanup | 1,000 join/leave cycles without retained sessions/entities/jobs |
| Memory | No sustained unexplained growth; below 5% retained growth after warm-up in fixed-population churn |
| Tick budget | p99 below 80% of interval; deadline misses below 0.1% |
| Improvement | Target 20% lower CPU and peak memory than matched Proton baseline; no material join/save latency regression |

These are targets, not results. Report failures honestly; record explicit decisions
before changing thresholds. Crash-free exposure does not prove indefinite reliability.

Install hooks with `prek install`. Run:

```bash
prek run --all-files --hook-stage manual
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --all-targets --locked
```

Add Linux release-build/runtime checks and audit headless/shared-library dependencies.
Ordinary CI uses authored fixtures; authorized reference comparisons run separately
in a private lab. Exclude secrets, captures, dumps and private saves from public
artifacts; staged Gitleaks scanning remains independently necessary.

Run the approved canonical case reviewer with hashes before research handoff.
Publish sanitized evidence/specifications/limitations and benchmark methods in
`docs/`; keep reusable notes outside vendor unless explicitly updating the package.

Roll out experimental builds, copied-world trials, then a limited pilot. Ship
backup/restore procedures, rollback exercises, compatibility information and native
Pelican/service examples. For each official update, preserve a baseline, compare
binaries/protocols, rerun affected scenarios and publish explicit support status.
Unknown versions are not supported by assumption.

Completion requires demonstrated compatibility, reliability and performance
improvement while preserving the independent Rust/Bevy and no-original-assets rules.

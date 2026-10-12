# Independent ICARUS Linux dedicated server

This is the reviewed implementation plan. Progress and evidence are tracked in
[implementation-status.md](implementation-status.md); incomplete milestones are
not claims of compatibility. This document authorizes no vendored script or
target activity by itself.

**Resumed at the user's request on 2026-10-12.** The
[continuation checkpoint](implementation-status.md#continuation--2026-10-12)
records native C034 own/array qualification, individual opaque widths and
conditional matched-client dispatch.
The prior pause does not extend historical runtime authorization windows.

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

Use Rust **1.99.0** and Bevy **0.20.0**, verified current stable on 2026-10-12.
Bevy declares Rust 1.97.1 as its minimum. Build, test, and runtime checks must
demonstrate actual compatibility with 1.99.0.

Pin Rust in `rust-toolchain.toml` with matching Rustfmt and Clippy; use the same
toolchain for development, CI, benchmarks, and releases. Commit `Cargo.lock` at
initialization. Target the latest stable toolchain and project dependencies,
including Bevy, through tested updates; routine stable upgrades are authorized.
Verify upstream release metadata, migrate APIs and update pins and the lockfile
together. Record concrete compatibility blockers when an upgrade cannot pass.
Use version-matched documentation and keep release builds reproducible.

Sources: [Rust 1.99.0](https://blog.rust-lang.org/2026/10/01/Rust-1.99.0/),
[Bevy 0.20.0](https://crates.io/crates/bevy/0.20.0).

The local Bevy skill was refreshed using `bunx skills add edbfi/skills --skill bevy
--agent codex --copy --yes` from upstream commit
`69cc23fa1b1bfce68143b2e289826e500b63ef26` on 2026-10-12. Skill version 1.1.0 targets
Bevy 0.20.0; all 14 installed files match upstream. `skills-lock.json` records the
source and content hash. The application now targets the same 0.20.0 baseline.

Python helpers target stable **3.15.0**, pinned in `.python-version` and CI.
For every authored Python change, use `basedpyright-python-gate`: run
`python3 tools/typing_gate.py --diff-ignores`, `uvx basedpyright@latest --warnings`
and the authored unit tests. Require zero errors and zero warnings in recommended
mode; type and validate inputs instead of suppressing diagnostics. The checked
roots are `src`, `tests` and `tools`. The user explicitly excludes `.agents/` and
skill code from this gate. Prek and CI enforce the same contract. Future authored
Python must live in a checked root or extend the include list without weakening
checks. Keep the checker on `@latest`, as required by the skill.

## Repository, tooling, and authorization

The initial repository had no root Cargo manifest or lockfile. Keep these boundaries:

| Location | Contents |
| --- | --- |
| `icarus-orig/` | Ignored original server files and preserved installations |
| `icarus-client-orig/` | Ignored user-owned original client binaries and build manifests for reference analysis |
| `reference-data/` | Ignored user-supplied save archives and private provenance inventories; preserved inputs |
| `work/` | Ignored cases, captures, dumps, private saves, generated analysis and benchmarks |
| `src/`, `tests/`, `benches/`, `tools/` | Reusable implementation, authored tests and research tools |
| `docs/` | Plan, specifications, architecture decisions, sanitized findings and reports |

Keep reference installations immutable during experiments; place runtime data and
disposable copies in the case directory. Commit only authored or reviewed,
redistributable fixtures. Work on feature branches, use Conventional Commits, and
retain the prohibition on direct commits to `main`.

### Supplied heavy-world save and continuation order

On 2026-10-12 the user supplied a ZIP described as savegame/player data from a
heavy ICARUS world. The preserved local dataset is
`reference-data/2026-10-04-heavy-save/`, containing
`archive-2026-10-04T1921360200.zip`, `SHA256SUMS` and a private `inventory.json`.
The inventory records source provenance, archive/member SHA-256 hashes, member
paths, sizes and ZIP timestamps. The archive copy matches the supplied source;
all member CRC checks passed. Intake found 24 files (two JSON files and 22
backups), totaling 26,511,334 bytes before any inner-format decoding. The largest
member is 2,239,534 bytes. These are ZIP observations, not a validated save schema,
player-state inventory or measured gameplay workload. The producing game build
and matching client/server pair remain unverified. On 2026-10-12 the user described
the producing release as “latest”, then supplied build 25793767 and version
`3.0.31.158478-rel-DangerousHorizons`. Treat this as user-reported provenance until
paired with a matched reference; the preserved executable is older build 25621760.
A stock
Windows client is available on the user's separate machine for later tests, not
accessible to the current offline lab.

The completed local client installation has independently checked hashes and
matching executable/PDB GUID and age (`work/client-identity-20261012/`). Its
manifest associates the installation with build 25793767; this does not prove
the supplied save's producing build or pair it with the older 25621760 server.
Use a fresh bounded static scope before transferring any symbols or behavior.

The archive and inventory are private, read-only local files. Preserve them and
the original download; prepare any extraction, mutations, logs or runtime copies
under a new `work/` case. `reference-data/` is not disposable build output and
must be included in local backup planning; Git does not back it up. Keep raw saves,
player identifiers, names and member paths out of tracked fixtures, public reports,
CI artifacts and uploads. Record aggregate findings and independently authored
minimal fixtures in the repository. Player-created state may inform save import;
the existing exclusion of packaged game assets remains in force.

Use this dataset in the next work sequence:

1. Native filesystem fault probe completed on 2026-10-12: real ENOSPC/EACCES preserve
   acknowledged state, both retries succeed, and reopen verifies generation 4.
   See N-003 in the bug ledger and `work/native-io-20261012/` for fresh scope,
   exact invocation/provenance and verified container cleanup. Historical failures
   remain preserved; physical power-loss qualification remains open.
2. Outer real-save baseline established in `work/save-triage-20261012/`: all 24
   file hashes/CRCs match, twelve UTF-16LE prospect envelopes pass independent
   integrity checks, and twelve identical small tracker documents are identified.
   The native inspector now handles the observed encoding and measured dimensions
   with fixed 4 MiB input/32 MiB output bounds; current and distinct-backup copies
   pass the release CLI. See the envelope spec for aggregate sizes and limitations.
3. Recover inner binary framing and persisted relationships. Targeted static work
   on build 25621760 has resolved the reflected state/header, recorder array,
   version-dependent callbacks and tagged-property dispatch; see the loading spec
   and `work/save-schema-20261012/`. A bounded experimental root-tag reader now
   preserves opaque spans after envelope integrity checks. The optional recorder
   reader now checks concrete nested field and byte-array boundaries; its release
   results match an independent bounded scan of both selected copies. Older-build
   static evidence supports this framing. One selected singleton class's two
   component bodies now pass independent and native opaque property/trailer scans;
   the separate explicit library reader does not recognize classes or decode values.
   Four selected bodies from a second class also pass both framing implementations
   and match 23 older-build field descriptors. Follow-up independent header checks
   and a native batch extend that coverage to all 1,294 records of this class in
   the two copies, with exact byte conservation. Its load and postload overrides are
   checked separately; six struct fields have candidate tagged contracts, while
   24 selected nested payloads pass independent and native one-level framing scans. Their
   16 optional child descriptors agree with all 64 observed child headers. Static
   transfer widths and seven array-element framing contracts are recovered.
   All 9,058 arrays in this class are empty and pass independent count/shared-header
   checks across both copies; populated elements, deeper encoding and semantics remain
   separate work. An explicit
   candidate struct-array library reader passes authored tests and native
   qualification of the 28 selected empty arrays without changing the CLI inspector
   or importing a world.
   Four selected extrema from a third class (C034) now match its 21 optional
   older-build top descriptors, zero trailers and exact EOF (save-triage E-038).
   A separately scoped own-struct observation now matches eight immediate
   headers in all four selected bodies, including Float (E-039). Values and
   deeper children remain opaque. Fresh eleven-kind native comparison now matches
   every own header/value/terminator span. The own-array tagged contract and two
   optional immediate fields now agree on four supplied arrays, including two
   populated arrays with 428 total elements. A separate range-only check confirms
   each Struct/Float span matches the older successful 12/four-byte transfer demand;
   numeric values remain opaque.
   Matched-client generic tag/array branches also agree under explicit conditions;
   concrete client C034 registration now supports conditional tagged dispatch and
   its restore entry constructs a version-522 loading archive. A bounded follow-up
   carries that profile to conditional ordinary-class dispatch and links the N02
   deferred installer/lookup; later callbacks and initialization lifecycle remain
   unproved. Exact client Float factory/vtable dispatch now independently demands
   four bytes, retaining transfer-success and numeric-semantics limits.
   Separate bounded per-copy batches now qualify all 1,658 C034 top bodies with
   exact native/independent spans. All 1,658 own structs now agree as well;
   all own arrays also agree across 15,342 elements/30,684 fields, including
   individual 12-/four-byte demands. All 11,606 inherited arrays now independently
   match native shared framing and are empty. All 9,948 inherited Struct bodies
   also match immediate native spans. Exact descriptor bindings now qualify
   8,290 selected nested/own scalar widths. All 486 C009 top bodies now match
   the reviewed 31-field contract and native spans under an explicit 71,956-byte
   comparison cap. Recover its next concrete child contract; retain the lack of populated
   inherited-array coverage.
   Exact C034 own Name values also pass strict native/independent structural
   decoding across 1,658 spans; name construction and identity semantics remain
   open.
   Generic framing and width agreement do not establish an importable world.
   Eleven distinct prospect payloads and seven member tuples are observed, but
   structures, inventories, tames, missions and full ownership fidelity remain
   unknown. Target component-specific byte dispatch and schemas, compare backed-up
   records only after interpreting them, and derive authored regression fixtures.
   Do not infer chronology/build compatibility from names or silently discard fields.
4. Use disposable copies for a matched official-server load/save/reload baseline
   once build pairing and the runtime experiment scope are established. Record
   state fidelity, load/save time and resources. This intake does not revive the
   expired overnight runtime window or establish that this world reproduces a bug.
5. Continue authenticated negotiation, replication and the independently authored
   gameplay slice. Use the save findings to inform M2's import-feasibility gate and
   M5's field-preservation matrix. First validate a small supported import, then the
   heavy dataset with stock-client play/save/reload. Use it for M1/M6 comparisons
   only when both implementations can execute the same supported workload.

This moves real-save characterization ahead of full migration implementation and
heavy-world benchmarking. Possession of the archive does not complete any
compatibility, bug-reproduction or performance gate. Keep the implementation status
and final research handoff synchronized with each completed step.

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

## Autonomous execution and maintenance

The user's 2026-10-12 instructions authorize autonomous progress, frequent
Conventional Commits and feature-branch pushes, appropriate subagents, and periodic
cleanup. Continue through implementation and verification; do not stop after
proposing another plan or ask for approval of routine choices. A blocked experiment
should yield a precise dependency and the next useful independent task. Existing
target, privacy and isolation boundaries still apply. Historical approval records
describe their original batches; they do not extend expired runtime deadlines.

Following OpenAI's [initiative guidance](https://developers.openai.com/api/docs/guides/latest-model#gpt-6-astra-initiative-and-follow-through),
make reasonable assumptions, deliver complete increments, and prepare reviewable
work before necessary approval. Resolve skill instructions against the user's
actual authorization; explain concrete conflicts instead of inventing approval gates.

Use the Bevy and reverse-research skills listed above when their work is needed.
Read relevant references selectively instead of loading the entire vendor package.
If at any point you can parallelize work by delegating tasks to another agent
(whether you are the root agent or a subagent), use collaboration tools when it
could save time or improve quality. Keep delegation within the shared resource
budget and give each task a clear owner and acceptance checks.
The lead can assign GPT-6 Astra at high effort for ambiguous protocol/architecture
work and GPT-6.1 Sol at medium or high effort for bounded implementation, tests and
independent reviews. Adjust these choices to the task and available model IDs.
Use supported identifiers such as `gpt-6-astra` and `gpt-6.1-sol`; when the tool
requires it, use `fork_turns="none"` for explicit model/effort overrides and provide
a self-contained task. Start with the lead and up to two workers; use another
worker only for independent work that fits available slots and resources.
Assign file ownership or separate worktrees; only the lead stages, commits and
pushes integrated work. Review
findings against real source and evidence before accepting them.

For each coherent increment: implement, run relevant checks, update the existing
status/specification when behavior or evidence changes, inspect the staged diff,
commit with a Conventional Commit subject and matching `Signed-off-by`, then push
the feature branch. A small related batch is acceptable; do not accumulate a day's
validated work locally. Check Git status and remote state before changes, fetch
before integration, and preserve collaborators' commits. The old no-push batch
restriction is superseded. Leave `main` protected; never bypass hooks or disable
CI to publish work. Report a failed push explicitly and continue useful local work
while retaining a clear list of unpushed commits.

The pre-existing 35 feature commits lack the sign-offs required by the newer PR
policy. New commits must comply. Keep those historical commits intact and record
the policy blocker before PR integration; do not rewrite history, invent attestations
or weaken the policy merely to make a check green. A branch push does not resolve
this PR gate. Register any PR worked on with T3 Code when its linking tools exist.

Keep these documents authoritative, updating them in place:

- This file: goal, order of work, constraints and release gates.
- `implementation-status.md`: current tested behavior, failures and next actions.
- `compatibility-matrix.md` and `bug-ledger.md`: evidence-backed coverage and defects.
- `specs/` and dated `research/` reports: durable technical findings and provenance.

Avoid overlapping roadmaps, generic agent-generated checklists and repeated
session summaries. Keep temporary handoff prompts under `/tmp` only. Consolidate
useful results into the documents above; retain dated reports as historical
evidence and mark superseded planning instructions at the top. This replaces the
old expectation of a separate `docs/overnight-report.md` with a maintained current
checkpoint and the existing research reports.

Perform a cleanup pass after each milestone or several completed increments:
inspect pending changes, reconcile stale claims and links, stop/remove only owned
finished experiments, and remove unreferenced reproducible scratch or abandoned
generated duplicates after checking provenance references. Record significant
removals in the commit or private case timeline. Retain primary evidence, inputs,
hashes, failure reproductions and irreplaceable backups. Ignored does not mean
disposable: never prune `reference-data/`, `icarus-orig/`, `icarus-client-orig/`, the original download,
another agent's files, or historical evidence as routine cleanup. Do not run global
Docker prune, blanket `git clean -fdx`, or delete data solely because it is old.

Recheck host resources before heavy work. Start with two Cargo jobs and at most
four CPU-heavy workers in total; serialize large builds, Ghidra and original-server
runs when memory is tight. Target at most 10 GiB owned-job memory; defer heavy work
below 3 GiB available RAM or 30 GiB free disk. Cap new case artifacts at 12 GiB and
total batch disk growth including caches at 20 GiB, then reassess. For targeted
Ghidra work, cap the heap at 4 GiB, use two CPUs, and limit each batch to 24 selected
functions, 30 seconds per decompilation and 15 minutes overall. Bound every
experiment and clean it up on timeout.
Prefer a targeted experiment with a clear acceptance condition over repeating an
unchanged failed environment or running broad analysis without a question.

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

# Independent ICARUS Linux dedicated server

This is the canonical plan: goal, constraints, work order, milestones and release
gates. [Implementation status](implementation-status.md) records what exists and
what is next; [gates](gates.md) lists preconditions that block actions;
[architecture](architecture.md) holds design constraints. This document
authorizes no tool execution or target activity. Full text before condensation:
d5b0ae6:docs/linux-dedicated-server-plan.md.

Offline research resumed under the execution instruction (F-404). A resumption
does not extend historical runtime authorization windows; those records are in
[docs/history](history/authorizations-index.md).

## Goal and constraints

Build an independent native Linux dedicated server with authoritative simulation
in Rust and Bevy. Demonstrate compatibility with unmodified ICARUS clients,
existing-save import, reliable persistence, improved stability and improved
performance for a pinned official client/server release.

Investigation priorities, in order: crashes, hangs and save integrity;
long-session degradation; joining, reconnecting and session cleanup; large worlds
with substantial bases, tamed animals, production, environmental activity and
many players. A handshake or small playable slice is an intermediate milestone.
Completion requires all applicable server behavior in the pinned release,
including supported content variants, with an explicit compatibility matrix.

### Independence and provenance

- Independently implement simulation, protocols, persistence and server data.
  Bevy provides the application, ECS and scheduling foundation; legitimate Linux
  platform SDKs may provide authentication and discovery.
- No dependency on the original server executable, Unreal runtime, Wine or Proton.
- Do not extract or reuse original art, audio, terrain, collision, navigation or
  gameplay data tables. Record provenance of independently authored data.
- Record protocol identifiers and experimentally established behavior in
  specifications; do not bulk-convert original assets. Save import may consume
  player-created state, never packaged game assets. Stock clients keep their
  normal installations.
- Preserve the repository license and third-party notices. Verify platform SDK
  distribution constraints before shipping an integration.

[benilla](https://github.com/samwhosung/benilla) and
[Skate 3 Rust Engine](https://github.com/SK8-ENGINE/skate-3-rust-engine) are
architectural references only; both use original game data in ways excluded here.

**Early gate:** demonstrate that stock clients correctly interact with
independently authored server world data. If original data proves indispensable,
document the blocker and keep the full goal unmet. Do not substitute modified
clients, asset extraction or reduced functionality without an explicit scope decision.

### Toolchain

Rust **1.99.0** and Bevy **0.20.0**, verified current stable on 2026-10-13 (F-413)
([Rust](https://blog.rust-lang.org/2026/10/01/Rust-1.99.0/),
[Bevy](https://crates.io/crates/bevy/0.20.0)). Pin Rust in `rust-toolchain.toml`
with matching Rustfmt and Clippy; commit `Cargo.lock`; use one toolchain for
development, CI, benchmarks and releases. Routine stable upgrades are authorized:
verify upstream release metadata, migrate APIs, update pins and lockfile together,
and record concrete blockers when an upgrade cannot pass.

The Bevy skill under `.agents/skills/bevy/` is installed from `edbfi/skills`
with `bunx skills`; `skills-lock.json` records source and content hash. Python
helpers target stable 3.15.0 under the basedpyright gate described in
[`.agents/rules/project.md`](../.agents/rules/project.md).

## Repository boundaries

| Location | Contents |
| --- | --- |
| `icarus-orig/` | Ignored original server files and preserved installations |
| `icarus-client-orig/` | Ignored user-owned original client binaries and manifests |
| `reference-data/` | Ignored user-supplied save archives and private inventories; preserved inputs, not backed up by Git |
| `work/` | Ignored cases, captures, dumps, private saves, generated analysis and benchmarks |
| `src/`, `tests/`, `benches/`, `tools/` | Reusable implementation, authored tests and research tools |
| `docs/`, `evidence/` | Plan, specifications, ledgers, digests, runbooks, frozen history and the evidence manifest |

Keep reference installations immutable during experiments; put runtime data and
disposable copies in a case directory. Commit only authored or reviewed,
redistributable fixtures. Keep raw saves, player identifiers, names and member
paths out of tracked fixtures, public reports, CI artifacts and uploads.

## Research workflow

All reverse engineering follows the project-local
[REA skill](../.agents/skills/reverse-engineer-anything/SKILL.md) with the `rea`
MCP server as the primary interface; the [tooling runbook](runbooks/rea-tooling.md)
restores the runtime and names the three reference environments. Tool
installation does not authorize a target experiment. Before new target work,
record assets, activities, authorization evidence, exclusions, network profile
and a bounded execution window under `work/`, and check the actual command
against them. Separate offline and dynamic scopes; lab-only scopes permit no
external service connections. Preserve frozen case artifacts and receipts; adapt
later probes with fresh reviewed bindings. Each session ends with ledger rows and
a digest as required by [`.agents/rules/docs.md`](../.agents/rules/docs.md).

## Supplied heavy-world save

On 2026-10-12 the user supplied a save archive from a heavy ICARUS world,
preserved under `reference-data/2026-10-04-heavy-save/` with a private
inventory (M-166). Intake facts (F-276), the user-reported producing build
25793767 (F-277), the preserved older server build 25621760 and the local
client identity check (F-278) are ledger rows; pairing with a matched reference
remains unverified. A stock Windows client exists on the user's separate machine, outside
the offline lab.

Work order for this dataset:

1. Native filesystem fault probe: completed (bug ledger N-003).
2. Outer real-save baseline and envelope inspector: completed; see the
   [envelope spec](specs/save-format/prospect-envelope.md).
3. Inner binary framing and persisted relationships: framing, dispatch and
   selected class audits are recorded in the
   [loading spec](specs/save-format/prospect-loading.md) and the
   [save-class digest](research/save-format/2026-10-13-save-class-audits.md).
   Value meanings, restoration, inherited-array population and C001's
   construction bridge remain open ([unknowns](ledgers/unknowns.md)). Generic
   framing agreement does not establish an importable world; no partial importer.
4. Matched official-server load/save/reload baseline on disposable copies, once
   build pairing and a runtime scope exist.
5. Authenticated negotiation, replication and the gameplay slice; validate a
   small supported import before the heavy dataset.

Possession of the archive completes no compatibility, bug-reproduction or
performance gate.

## Milestones and exit criteria

| Milestone | Scope | Exit |
| --- | --- | --- |
| M0 Frozen baseline | Inventory and hash binaries; record build IDs, manifests, configuration, environment; preserve failing worlds and logs; maintain the compatibility matrix with unknown, observed, specified, implemented and validated states | Reproducible startup, verified build pairing, preserved state, actionable matrices |
| M1 Bug reproduction and performance baseline | Campaigns in priority order: startup/load/autosave/shutdown/recovery; repeated cycles and long sessions; join/disconnect/reconnect; large bases, tames, production, players. Distinguish original defects from Proton faults, resource exhaustion and configuration; link a regression test to every confirmed bug | Baseline measurements, ranked bug ledger, repeatable tests; unreproduced reports stay unresolved |
| M2 Protocol, platform and world-data feasibility | Recover discovery, negotiation, authentication, session lifecycle, framing, reliability, replication, world selection and save loading from controlled captures and serializer cross-references; preserve unknown fields as unknown | Three gates: stock client authenticates into an independent session; loads a representative region with independently authored data; a small official save imports without packaged assets. Or a precise blocker and next discriminating experiment |
| M3 Bevy foundation and gameplay slice | Configuration, lifecycle, health, diagnostics; then stock-client sessions, spawn, movement, interaction, inventory, save/restart/reconnect, two-client consistency | Native gameplay survives persistence and reconnect with verified authoritative state; a headless foundation alone does not complete M3 |
| M4 Complete behavior matrix | Subsystems in dependency order: world and movement; player lifecycle; items and crafting; construction; wildlife and taming; weather and resources; prospects and missions; administration. Each needs specification, provenance, implementation, stock-client scenarios, persistence coverage and regression evidence; seed from [official documentation](https://github.com/RocketWerkz/IcarusDedicatedServer/wiki) including the eight-player limit | All applicable matrix entries validated |
| M5 Migration and reliability hardening | Recover formats and relationships; import copies, preserve originals and player-created state; reject incomplete imports with field-level reports; test interrupted writes, full disks, corruption, termination loops; re-run confirmed original failures | Migration fidelity, crash consistency, bounded failure handling, bug regressions |
| M6 Optimization, qualification, packaging | Profile, change one mechanism, compare; package native releases, services, a native Pelican egg, migration guidance and qualification reports | All release gates pass; earlier milestones support experimental releases only |

## Validation and release gates

Test protocol vectors and malformed, duplicate, reordered and fragmented inputs;
controlled-time schedules, deferred changes and obsolete jobs; gameplay
invariants; persistence migration, corruption and interrupted storage; actual
stock-client flows; original bug regressions, churn, pressure and long soaks.
Synthetic traffic never replaces interoperability proof. Differential tests
compare semantic results and require byte equality only for established wire
encodings. Benchmark small, representative and heavy worlds at 1/4/8 players,
five repetitions after warm-up, frozen resources and seeds, matched Linux/Proton
workloads; separate platform effects.

| Outcome | Release gate |
| --- | --- |
| Compatibility | All required scenarios pass for the declared build and content |
| Independence | Runs with original directories unavailable and no prohibited data |
| Save fidelity | Complete supported import, no silent loss, verified play/save/reload |
| Durability | No loss of acknowledged saves; valid abrupt-failure recovery |
| Recovery point | At most 60 seconds unsaved progress normally; overruns surfaced |
| Stability | Three independent 72-hour heavy runs without crashes, hangs or restarts |
| Cleanup | 1,000 join/leave cycles without retained sessions, entities or jobs |
| Memory | No sustained unexplained growth; below 5% retained growth after warm-up |
| Tick budget | p99 below 80% of interval; deadline misses below 0.1% |
| Improvement | 20% lower CPU and peak memory than the matched Proton baseline; no join/save latency regression |

These are targets, not results. Record explicit decisions before changing thresholds.

Ordinary CI runs authored fixtures, a release build and a headless smoke test;
authorized reference comparisons run in a private lab. Roll out experimental
builds, copied-world trials, then a limited pilot, with backup/restore and
rollback procedures. For each official update, preserve a baseline, compare
binaries and protocols, rerun affected scenarios and publish support status.
Unknown versions are not supported by assumption.

## Maintenance

Workflow, commit, delegation, resource and cleanup rules live in
[`.agents/rules/project.md`](../.agents/rules/project.md); documentation rules in
[`.agents/rules/docs.md`](../.agents/rules/docs.md). Known policy blocker: 35
historical feature commits lack the sign-offs the PR policy now requires (F-279);
preserve their history, keep the policy enabled and record the blocker before PR
integration.

Completion requires demonstrated compatibility, reliability and performance
improvement while preserving the independent Rust/Bevy and no-original-assets rules.

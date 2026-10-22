# Independent ICARUS Linux dedicated server

This is the canonical plan: goal, constraints, milestones, exit criteria and
release targets. [Implementation status](implementation-status.md) records what
exists and what is next; [gates](gates.md) is the only home of preconditions that
block actions; [architecture](architecture.md) holds design constraints,
repository boundaries and the toolchain; research workflow rules live in
[`.agents/rules/project.md`](../.agents/rules/project.md). This document
authorizes no tool execution or target activity.

Before restructure: 96ddb0b:docs/linux-dedicated-server-plan.md

## Goal and constraints

Build an independent native Linux dedicated server with authoritative simulation
in Rust and Bevy. Demonstrate compatibility with unmodified ICARUS clients,
existing-save import, reliable persistence, improved stability and improved
performance for a pinned official client/server release, a pin that moves
with ICARUS's weekly releases ([architecture](architecture.md#release-tracking)).

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
  gameplay data tables. Basic data the server needs that ships with the free,
  public dedicated-server distribution may be used where parity requires it,
  supplied in a way the user has yet to decide; the game client is never a
  source for what the server consumes (F-828, [gates](gates.md)). Record
  provenance of independently authored data.
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

Completion requires demonstrated compatibility, reliability and performance
improvement while preserving the independent Rust/Bevy and no-original-assets
rules.

# Independent ICARUS Linux dedicated server

Goal, constraints, milestones with their state, the ordered next tasks, exit
criteria and release targets. Related: [gates](gates.md),
[architecture](architecture.md) and research rules in
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
including supported content variants, with an explicit compatibility matrix,
demonstrated compatibility, reliability and performance improvement, and the
independent Rust/Bevy, no-original-assets rules.

### Independence and provenance

- Independently implement simulation, protocols, persistence and server data.
  Bevy provides the application, ECS and scheduling foundation; legitimate Linux
  platform SDKs may provide authentication and discovery.
- No dependency on the original server executable, Unreal runtime, Wine or Proton.
- **Input parity** (F-828, F-831;
  [project rules](../.agents/rules/project.md#input-parity)): the native server
  may need no more than the official dedicated server needs. Protocol row identities from that free,
  public distribution may be fetched
  unattended at install time by our own fetcher; anything only the game client
  provides is never a runtime or build input. Needing nothing official is better
  still, toward a fully standalone binary.
- Do not extract or reuse original art, audio, terrain, collision,
  navigation or gameplay data tables; record provenance of independently
  authored data.
- Record protocol identifiers and experimentally established behavior as
  memory records; do not bulk-convert original assets. Save import may consume
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

## Milestone state

Moved from the status file at the cut-over (last reconciled 2026-11-19); its
whole text, summary included, is REF-036. Update a state or a task here when it
changes, citing the records that justify it.

| Milestone | Status | Evidence or next gate |
| --- | --- | --- |
| M0: reference baseline | In progress | 435 files hashed; build 25621760; entry-map startup and save outer format observed; PE/PDB/MSF identities verified (F-878; U-034 resolved); producing log U-040 pending |
| M1: original bug reproduction | Pending | No reported crash reproduced or official bug fixed (F-217); controlled workloads pending |
| M2: interoperability/data feasibility | In progress, partial observations | Stateless exchange observed; anonymous official-data fetch/table row handles F-830/F-831; protocol/world/import feasibility gates open |
| M3: Bevy gameplay slice | In progress, foundation only | Rust 1.100.0/Bevy 0.20.1 tested; opt-in authored loopback F-875; stock-client slice awaits M2 |
| M4: complete behavior | Pending | All required compatibility matrix entries must pass |
| M5: migration/hardening | Pending, native hardening underway | Filesystem faults, lossless save generations/autosaves F-567/F-873; randomized readers/peer lab never panic, corrupt stores recover committed state or fail closed F-764 (it supersedes the rows REF-036 cites here). Lossless typing/import blockers F-766/F-898; meanings/migration fidelity unvalidated |
| M6: optimization/qualification | Pending | Matched reference workloads and full release gates required |

## Next tasks

In priority order.

1. Data F-828/F-829/F-831/F-832/F-993; saved rows F-861/F-866. Empty clocks F-1271/F-1272; native opt-in pause F-1286 (off by default, 3.0.37 behaviour unobserved). Crop key comparison F-1274; gameplay fidelity open. No client dependency or packaged values.
2. Import U-045/U-025: restoration unproven F-766/F-916. Candidates F-920/F-927/F-929; reuse F-1195's list/base join; lifecycle/removal/timers F-930/F-937/F-940/F-931. One historical server dispatch wrapper copies 24 bytes into a stack argument (F-1260). Next: final argument contents, remaining definitions/storage/ABI, comparator operand/receiver proof, overrides, native owner/subclass bindings and coordinates before all-or-nothing import. Publication stays closed (F-660); [procedures](runbooks/authored-worlds.md), costs F-926.
3. Player state F-668/F-643; build ambiguity/time window F-635/F-896/F-685/F-878. Writer unconfirmed after host search (U-040). Authored coverage: checkpoints F-1172, leave F-1188, survival F-1183, bags/Use recovery and transfers F-1200, owner bag projection F-1202, saved location and its restore F-1206/F-1207 ([operations](runbooks/authored-survival.md)); next: authoritative state coverage and separately scoped client qualification.
4. Network bounds F-841/F-975/F-1015/F-1016; restore F-973/F-978 to F-993/F-1054 to F-1056/F-1063/F-1064/F-1066; ticks F-995 to F-1000. Entry F-1185; failed leave F-1186. F-1197 conditional leave codec retains inherited-layout/old-trace limits. F-1188 adds default-off private leave/restart checks; client proof remains open. F-1265 compares both entry paths through the first record ACK. Run a fresh consumption-off control for F-1270, then qualify food. Leave/rejoin need the codec bound for 3.0.37 (F-1280); Begin limit F-1209; membership F-1171. F-1177 active; preserved-world hosting refuses; U-069/U-070/U-076 open.
5. Replication replay/scheduling F-762/F-761, lifecycle/costs F-938/F-952. Authored speed stats accompany stock-client locomotion F-1185; authoritative motion/physics, collision, audio and stat UI remain unqualified (U-025/U-080). Corrections: layout F-1204, codec F-1205. `pawn_receive` F-1203 will show which pawn fields a client inventory action sends. After checkpoint entry succeeds, qualify candidate F-1198 and operator spawn points F-1139/F-1140 after the mountain spawn, then saved-location restore F-1207 (F-1134 to F-1138/M-1374). Protocol: REF-001; [configuration](runbooks/native-server.md#network-sessions-experimental).
6. Reliability F-1029/F-1030/F-872/F-873/F-875/F-833/F-932/M-1055/F-877. Next: 72-hour holds and physical power loss U-035; no ICARUS performance proof F-871.
7. Current 3.0.37 pin/tags/classes/rows F-1277 to F-1279. Qualify matched entry before testing leave or rejoin. Prior 3.0.36 entry F-1185, failed leave F-1186; placement and properties/poses U-038/U-025. Visibility remains gated; the probe-switch run is not scheduled. [Gates](gates.md).

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

# Implementation status

This is an independent server foundation, not yet an ICARUS-compatible server.
No gameplay, protocol, authentication, world-data parity, or official-save import
has been demonstrated. Reliability and performance targets remain unproven.

## Milestones

| Milestone | Status | Evidence or next gate |
| --- | --- | --- |
| M0: reference baseline | In progress | 435 files hashed; build 25621760; isolated entry-map startup observed; authenticated startup and matched client pending |
| M1: original bug reproduction | Pending | Offline startup did not reproduce reported crashes; controlled workloads still required |
| M2: interoperability/data feasibility | Pending | Requires protocol, platform and world-data evidence |
| M3: Bevy gameplay slice | In progress, foundation only | Rust 1.99.0 / Bevy 0.19.1 foundation tested; stock-client slice pending M2 |
| M4: complete behavior | Pending | All required compatibility matrix entries must pass |
| M5: migration/hardening | Pending | Official save format and failure scenarios unknown |
| M6: optimization/qualification | Pending | Matched reference workloads and full release gates required |

The 30 Hz development rate is provisional, not a discovered ICARUS property.
Native snapshot version 1 stores authored foundation records; it is not the ICARUS
save format. No official server bugs are marked reproduced or fixed.

See the [full plan](linux-dedicated-server-plan.md) and
[approved offline batch](research-authorization.md).

## Implemented and checked

- Pinned Rust 1.99.0, Bevy 0.19.1, headless features, committed Cargo lockfile.
- Native configuration validation and CLI overrides; no-write config inspection.
- Fixed-step ECS loop, stable authored domain IDs, coherent snapshot capture,
  bounded catch-up with diagnostic counters. Tick rate remains provisional.
- One asynchronous save at a time, directory writer lock, checksummed generations,
  synchronized atomic manifest publication, two-generation retention and recovery.
- Unsupported schemas and corrupt/missing manifests fail closed. Official-save
  import explicitly fails without accessing supplied paths.
- SIGINT/SIGTERM final save and bounded shutdown; JSON lifecycle/status diagnostics.
- 19 passing Rust tests, release build and native smoke run; full manual `prek`
  checks passed. Separate process tests cover signals and acknowledged-save recovery
  after SIGKILL. No physical power-loss, disk-full or long-soak qualification yet.
- Active normal dependency graph excludes Bevy rendering/window/audio backends;
  release executable links only the platform loader, libc and libgcc_s on this host.
- CI definition validates formatting, Clippy, tests, release build and headless smoke.
  Local checks passed; hosted CI has not been run from this workspace.

## Reference evidence

- [Inventory](research/2026-10-04-reference-inventory.md): hashed original baseline.
- [Static inspection](research/2026-10-04-static-baseline.md): executable identity,
  imported interfaces and concrete diagnostic anchors; behavior remains unverified.
- [Bounded Ghidra pass](research/2026-10-04-decompilation-report.md) reached its
  analysis time limit and exported zero functions; behavior remains unresolved.
- [Offline Proton startup](research/2026-10-04-offline-startup-report.md) reached
  the entry map, reporting game 3.0.30.158174 and engine 4.27.2. Steam initialization
  failed in isolation; the harness stopped and removed the container. No connected
  client tests are authorized yet.

These checks demonstrate the authored foundation only. No original-server bug is
claimed fixed, no save migration is supported, and no ICARUS performance or
stability improvement has been measured.

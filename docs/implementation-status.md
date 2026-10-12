# Implementation status

This is an independent server foundation, not yet an ICARUS-compatible server.
One bounded stateless exchange has been observed against the original. Complete
protocol interoperability, authentication, gameplay, world-data parity and
official-save import have not been demonstrated. Release reliability and
performance targets remain unproven. Last reconciled: 2026-10-12.

## Milestones

| Milestone | Status | Evidence or next gate |
| --- | --- | --- |
| M0: reference baseline | In progress | 435 files hashed; build 25621760; isolated entry-map startup observed; supplied heavy save preserved; its build pairing and matched client pending |
| M1: original bug reproduction | Pending | Offline startup did not reproduce reported crashes; controlled workloads still required |
| M2: interoperability/data feasibility | In progress, partial observations | Stateless exchange observed; authenticated protocol, world-data and import feasibility gates remain open |
| M3: Bevy gameplay slice | In progress, foundation only | Rust 1.99.0 / Bevy 0.20.0 foundation tested; stock-client slice pending M2 |
| M4: complete behavior | Pending | All required compatibility matrix entries must pass |
| M5: migration/hardening | Pending, native hardening underway | Experimental envelope inspection and native fault coverage exist; real-save schema and migration fidelity remain unvalidated |
| M6: optimization/qualification | Pending | Matched reference workloads and full release gates required |

The 30 Hz development rate is provisional, not a discovered ICARUS property.
Native snapshot version 1 stores authored foundation records; it is not the ICARUS
save format. No official server bugs are marked reproduced or fixed.

See the [full plan](linux-dedicated-server-plan.md) for the active work order and
autonomous execution policy. The October 4 authorization and checkpoint documents
record historical research batches; the overnight runtime window has expired.

## Implemented and checked

- Pinned Rust 1.99.0, Bevy 0.20.0, headless features, committed Cargo lockfile.
- Local Bevy skill updated from `edbfi/skills` commit `69cc23f` using `bunx skills`,
  with an exact 14-file match, valid frontmatter and resolved relative links. Its
  baseline matches the application at 0.20.0. Skill installation validation did
  not rerun its graphical examples; application checks exercise the headless server.
- On 2026-10-12, upgraded Bevy to 0.20.0 and refreshed all direct Cargo dependencies
  to their latest stable releases: base64 0.23.1, hmac 0.13.0, sha1/sha2 0.11.0 and
  toml 1.1.8 required new constraints; Cargo.lock includes compatible transitive
  updates. Rust 1.99.0 and existing CI tool pins were already current. Adapted HMAC
  initialization and digest hex encoding while retaining persisted checksum bytes.
  Full manual checks passed, including independent legacy-envelope/HMAC fixtures,
  fixed-step scheduling, persistence/recovery and CLI tests. The native release
  build and startup/save/restart smoke also passed; Python independently verified
  saved SHA-256 values at ticks 3 and 5. The active graph remains headless. Weekly Cargo updates
  now join the existing Dependabot action and hook checks.
- Authored Python now passes basedpyright 1.40.2 in recommended mode on Python
  3.15.0: 18 errors / 418 warnings reduced to 0 / 0 across 10 files, with no added
  or removed ignores. Typed CLI/output schemas and validated JSON/binary inputs
  replace untyped plumbing; runtime helper provenance includes the shared parser.
  Eight authored unit tests and the full manual suite pass. Static-analysis CLI
  output matches the previous implementation on authored fixtures. Runtime-helper
  checks exercised parsing, checksums and collector failures without running an
  original-server case. The policy gate and checker run in prek and CI; `.agents/`
  and skill code are excluded as requested and were not modified.
- Native configuration validation and CLI overrides; no-write config inspection.
- Fixed-step ECS loop, stable authored domain IDs, coherent snapshot capture,
  bounded catch-up with diagnostic counters. Tick rate remains provisional.
- One asynchronous save at a time, directory writer lock, checksummed generations,
  synchronized atomic manifest publication, two-generation retention and recovery.
- Unsupported schemas and corrupt/missing manifests fail closed. Official-save
  import explicitly fails without accessing supplied paths.
- SIGINT/SIGTERM final save and bounded shutdown; JSON lifecycle/status diagnostics.
- On 2026-10-12, the full manual `prek` suite, release build and bounded native
  startup/save/shutdown smoke passed after integrating latest `main`. The suite
  includes Rust tests, Clippy, formatting, authored helper tests and workflow checks.
- Native fixes reject stale snapshots before publication and stop simulation on
  terminal save-worker disconnect; regression evidence is in the
  [bug ledger](bug-ledger.md). Injected publication failures preserve committed state.
- The historical recovery campaign passed 24 cycles with 10,000 authored entities
  (12 SIGKILL, 12 SIGTERM), preserving checksums and acknowledged state. Private
  results/provenance: `work/icarus-overnight/native-recovery-02/`.
- The historical synthetic soak completed 7,200.352 seconds with 100,000 entities,
  719 save commits and verified final state; it reported no tick deadline misses.
  Evidence: `work/icarus-overnight/native-soak.jsonl`, with the invocation and binary
  provenance in `commands.jsonl`. It began at `cac76ec` before later persistence
  changes, so it does not qualify the current revision or ICARUS gameplay.
- The [snapshot measurement](research/2026-10-04-native-snapshot-performance.md)
  documents improved native synthetic save/load cost after typed decoding. It is
  not a comparison with the original server.
- The real filesystem probe reached disk-full, but its wrapped-error recognition
  failed before state-recovery/EACCES assertions. Private evidence:
  `work/icarus-overnight/native-io-faults/probe.stderr` and `cleanup.json`; the
  container was removed. Real filesystem fault qualification remains unfinished,
  and physical power-loss and the required 72-hour runs remain untested.
- Experimental [envelope inspection](specs/prospect-envelope.md),
  [archive strings](specs/archive-string.md), [checksum](specs/network-checksum.md)
  and the separate [Bevy loopback handshake lab](specs/stateless-handshake.md) have
  authored tests. The production run command still has no game listener.
- Active normal dependency graph excludes Bevy rendering/window/audio backends;
  release executable links only the platform loader, libc and libgcc_s on this host.
- CI definition validates formatting, Clippy, tests, release build and headless smoke.
  Local checks passed; hosted CI has not been run from this workspace.

## Reference evidence

- [Stateless handshake](specs/stateless-handshake.md): original-server challenge,
  cookie echo and acknowledgement observed in isolation. Steam authentication was
  inactive; this did not establish a stock-client or authenticated session.
- [Checksum](specs/network-checksum.md): independent arithmetic matches one early
  startup value, which is invalidated later. Active admission policy is unresolved.
- [Prospect rejection](research/2026-10-04-prospect-rejection.md): missing/truncated
  by-name inputs were rejected without an observed crash. The separate
  [from-file failure path](research/2026-10-04-save-failure-output.md) remains a
  static lead, not a reproduced original defect.
- User-supplied heavy-world archive preserved on 2026-10-12 under ignored
  `reference-data/2026-10-04-heavy-save/`. The copy matches its source SHA-256;
  member CRC checks passed for 24 files (two JSON files and 22 backups). Private
  provenance and per-member hashes are stored alongside it. No extraction, save
  decoding, runtime loading or player-data validation was performed at intake.
  The producing build is unknown, and the largest member exceeds the current
  inspector's 1 MiB input limit. See the updated
  [continuation order](linux-dedicated-server-plan.md#supplied-heavy-world-save-and-continuation-order)
  for bounded inspection, backup comparison and later migration/workload validation.
- [Inventory](research/2026-10-04-reference-inventory.md): hashed original baseline.
- [Static inspection](research/2026-10-04-static-baseline.md): executable identity,
  imported interfaces and concrete diagnostic anchors; behavior remains unverified.
- [Bounded Ghidra pass](research/2026-10-04-decompilation-report.md) reached its
  analysis time limit and exported zero functions; behavior remains unresolved.
- [Targeted static follow-up](research/2026-10-04-targeted-static-report.md) located
  the selected references and inspected control negotiation, version comparison
  and prospect-loading branches with radare2. Initial
  [negotiation](specs/control-negotiation.md) and
  [loading](specs/prospect-loading.md) notes remain incomplete and unvalidated.
- [Offline Proton startup](research/2026-10-04-offline-startup-report.md) reached
  the entry map, reporting game 3.0.30.158174 and engine 4.27.2. Steam initialization
  failed in isolation; the harness stopped and removed the container. No connected
  client tests are authorized yet.

These results cover the authored foundation and the explicitly limited original
observations above. No original-server bug is claimed fixed, no save migration is
supported, and no ICARUS performance or stability improvement has been measured.

## Next work and maintenance

1. Fix wrapped errno recognition in `examples/io_fault_probe.rs`, then complete
   bounded disk-full/retry/permission/reopen checks under a current lab scope.
   Existing launchers bind to the expired overnight session; make new case/session
   inputs explicit without changing historical records or bypassing guards.
2. Characterize copies of the supplied save and backups, identify build/schema and
   relationships, and determine justified parser bounds. Preserve raw player data.
3. Advance transport/control/authentication evidence and independently authored
   world-data feasibility; validate a small import before the heavy-world workload.

Update this checkpoint and the existing plan/specifications as each step completes.
Use frequent signed-off Conventional Commits and feature-branch pushes. Before PR
integration, resolve the documented policy issue: 35 historical feature commits
lack required sign-offs; preserve their history and keep the policy enabled.
Clean only owned reproducible scratch and reconcile stale documentation at
milestones. Historical reports and the private primary evidence remain retained.

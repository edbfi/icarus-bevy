# Implementation status

This is an independent server foundation, not yet an ICARUS-compatible server.
One bounded stateless exchange has been observed against the original. Complete
protocol interoperability, authentication, gameplay, world-data parity and
official-save import have not been demonstrated. Release reliability and
performance targets remain unproven. Last reconciled: 2026-10-12.

## Milestones

| Milestone | Status | Evidence or next gate |
| --- | --- | --- |
| M0: reference baseline | In progress | 435 files hashed; build 25621760; isolated entry-map startup observed; supplied save integrity/outer format characterized; exact build pairing and matched client pending |
| M1: original bug reproduction | Pending | Offline startup did not reproduce reported crashes; controlled workloads still required |
| M2: interoperability/data feasibility | In progress, partial observations | Stateless exchange observed; authenticated protocol, world-data and import feasibility gates remain open |
| M3: Bevy gameplay slice | In progress, foundation only | Rust 1.99.0 / Bevy 0.20.0 foundation tested; stock-client slice pending M2 |
| M4: complete behavior | Pending | All required compatibility matrix entries must pass |
| M5: migration/hardening | Pending, native hardening underway | Real native filesystem faults and supplied envelope integrity pass; inner real-save schema and migration fidelity remain unvalidated |
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
- N-004 closes overdue shutdown success: deadline checks precede and follow result
  polling, and surround final-state capture/submission. An expired entry preserves
  its pending result; an expired submission adds no request. Deterministic channel
  regressions cover late completion, pending timeout, later draining and idle state.
  This does not preempt an in-progress OS write or synchronous capture.
- The N-004 revision completed a 120.157-second native synthetic run with 100,000
  authored entities, 60 save commits and complete final-state comparison. It
  reported zero tick deadline misses and zero clamped frames. Private inputs,
  source diff, binary hashes and results: `work/native-deadline-20261012/`.
  A subsequent 24-cycle campaign with 10,000 authored entities passed twelve
  SIGKILL and twelve SIGTERM/restart cycles, checking full recovered state and
  preservation of every observed acknowledged tick. Final published generation 55
  restored tick 5,119. All owned native processes exited.
  This short run exercises current code; it is not ICARUS performance or sustained
  release qualification and does not replace the historical two-hour evidence.
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
- The real filesystem probe now passes ENOSPC/EACCES, unchanged acknowledged
  manifest/state, space/permission restoration, retries and reopen verification.
  N-003 corrected error classification using the typed I/O kind retained by tempfile.
  The 2026-10-12 run used a disposable unprivileged 256 KiB tmpfs, completed at
  generation 4/tick 40 with 1,100 entities and verified exact-container removal.
  Private evidence: `work/native-io-20261012/native-io-faults/`. The historical failed
  run remains preserved. The launcher requires explicit current case/session/probe
  bindings and bounds launch, observation and cleanup to at most 120 seconds.
  Physical power-loss and the required 72-hour runs remain untested.
- Experimental [envelope inspection](specs/prospect-envelope.md),
  [archive strings](specs/archive-string.md), [checksum](specs/network-checksum.md)
  and the separate [Bevy loopback handshake lab](specs/stateless-handshake.md) have
  authored tests. The production run command still has no game listener.
- The envelope inspector accepts strict BOM-marked UTF-16LE as well as UTF-8, with
  measured 4 MiB raw-input and 32 MiB output policies. Eleven focused tests cover
  malformed code units, surrogate pairs, exact bounds and decompression expansion.
  The release CLI validated disposable copies of the supplied current prospect and
  a distinct backup: decoded sizes 18,513,234 and 18,492,675 bytes, matching hashes,
  with `world_imported: false`. Source copies remained unchanged. No private fixture
  is tracked; inner world serialization remains unsupported.
- N-005 closes an encoding-validation gap in ignored envelope metadata. Whole-input
  UTF-8 validation precedes JSON field selection, preventing malformed skipped
  strings from producing a successful inspection. Authored regressions cover
  invalid bytes, overlong encodings and encoded surrogates, with valid Unicode
  controls. Private reproduction: `work/native-codec-review-20261012/`.
- Opt-in `inspect-save --inner-tags` frames root properties under the explicit
  experimental memory522 profile after envelope integrity passes. It preserves
  borrowed opaque records and reports only counts/sizes; nested values remain
  unsupported. Six reader and fifteen prospect tests, full manual checks, release
  build and independent code review pass. Both private copies match an independent
  Node scanner: six root records and 378 header/terminator bytes each, exact EOF,
  unchanged source hashes and unchanged default envelope output. Evidence:
  `work/native-tagged-20261012/` and save-triage E-015/E-016. This is root framing,
  not complete schema validation or import.
- Opt-in `inspect-save --recorders` additionally validates exact recorder array,
  field and byte-array boundaries. Unknown supported-header fields retain borrowed
  spans; class values and component bytes remain opaque. The release reader
  matches independent totals on both private copies: 2,744/2,741 recorders and
  18,051,461/18,031,390 opaque bytes. Input hashes and previous CLI modes are
  unchanged. Seven recorder, six root and sixteen prospect tests, full manual
  checks, independent review and strict case review pass. Evidence: save-triage
  E-017/E-018 and `work/native-recorders-20261012/`. No gameplay counts or import
  fidelity follow from these framing checks.
- The opaque header reader also accepts `NameProperty`, backed by the older
  tag-reader switch and concrete value dispatch. Authored absent/present GUID and
  truncation fixtures pass; full manual checks and release qualification preserve
  both copies' existing envelope/root/recorder results and source hashes.
  Evidence: `work/native-name-header-20261012/` and static case E-018.
- The header reader now accepts `UInt32Property` with common property-GUID
  handling and no type-specific extensions, following the older built-in ID 25
  and tag-reader path. Authored absent/present GUID fixtures preserve four-byte
  and arbitrary opaque payloads; every truncation is rejected. Focused tests,
  full manual checks, release build and independent review pass. Numeric values
  remain uninterpreted. Static evidence: `work/c028-structs-20261012/`.
- A separate library API frames an explicitly selected base-component candidate:
  opaque tagged properties, `None`, a zero four-byte trailer and EOF. It bounds
  input to 64 KiB/256 fields, rejects GUID trailers and exposes immutable spans
  plus a sanitized summary. Six authored tests cover boundaries, every truncation,
  trailer policy and privacy. It does not select classes or change CLI inspection.
  The release API matches the independent scan on both selected 548-byte bodies;
  bounded stdin qualification reports only aggregates, with unchanged input hashes
  and no raw body artifact. Full manual checks and code review pass. Evidence:
  save-triage E-023 and `work/native-component-20261012/`.
- A caller-selected tagged struct-array API validates a count, mandatory shared
  struct header and exactly that many variable-length element streams. It bounds
  input to 64 KiB, elements to 4,096 and fields per element to 256, preserving
  immutable borrowed spans and reporting only aggregate counts/bytes. Six authored
  tests, full manual checks, release build and code review pass. One bounded native
  batch matches all 28 selected supplied arrays: 2,624 count/shared-header bytes
  with exact EOF, unchanged source hashes and no exported body bytes (E-037).
  All are empty, so populated elements have authored-test coverage only. The API
  does not recognize schemas, interpret values or change CLI inspection.
- Active normal dependency graph excludes Bevy rendering/window/audio backends;
  release executable links only the platform loader, libc and libgcc_s on this host.
- CI definition validates formatting, Clippy, tests, release build and headless smoke.
  Local checks passed; hosted CI has not been run from this workspace.
- The [offline reference launcher](offline-runtime-authorization.md) now requires
  current case/session/hash/mode bindings, locks attempt quotas, reserves cleanup
  time and verifies exact container ownership/removal. A final time check closes
  the hashing/preparation gap before launch. Thirty mocked Python tests pass;
  basedpyright reports zero errors/warnings, with no added suppressions. This
  launcher revision has not executed the original server.

## Reference evidence

- [Stateless handshake](specs/stateless-handshake.md): original-server challenge,
  cookie echo and acknowledgement observed in isolation. Steam authentication was
  inactive; this did not establish a stock-client or authenticated session.
- [Checksum](specs/network-checksum.md): independent arithmetic matches one early
  startup value, which is invalidated later. Active admission policy is unresolved.
  A bounded follow-up resolves the settings-derived setter and cache invalidation;
  cache lookup precedes callback-enable selection. Candidate initializer references
  register teardown, not compatibility callbacks. A broader direct-reference scan
  still leaves indirect/runtime registration unresolved. Private case
  `work/control-version-20261012/` passes strict review with unchanged original input.
- [Prospect rejection](research/2026-10-04-prospect-rejection.md): missing/truncated
  by-name inputs were rejected without an observed crash. The separate
  [from-file failure path](research/2026-10-04-save-failure-output.md) remains a
  static lead, not a reproduced original defect.
- [Inner save dispatch](specs/prospect-loading.md): 44 targeted exports plus
  metadata and flag tracing identify the older build's reflected state/header,
  recorder blobs, version-dependent callbacks and concrete tagged-property reader.
  Object offsets are not disk offsets. Root tag framing and its archive-version
  gates and concrete nested recorder framing are recovered. An independent private
  scan traverses 2,744 current and 2,741 backup recorder entries with exact array
  boundaries; component byte contents remain opaque. These are not gameplay object
  counts. These findings
  do not establish newer-build migration.
  Static evidence and exact commands: `work/save-schema-20261012/`.
- Recorder restoration resolves the saved class string and invokes a component
  load method. The base method builds a memory reader over opaque `BinaryData`
  and calls component serialization. The base path selects filtered tagged fields,
  followed by a four-byte boolean and optional 16-byte GUID, then a component hook.
  One private singleton class inherits all three relevant base slots in the older
  executable. Its first candidate body scan stopped at an unsupported header
  (save-triage E-020). A targeted check identified `NameProperty`; a new probe
  then framed both selected 548-byte bodies to EOF: five properties, 312 header
  and terminator bytes, 232 opaque value bytes and a zero four-byte trailer
  (E-021/E-022). This does not establish equal values or complete class semantics.
  The complete older native ancestry has five descriptors with the save-selection
  bit. Both samples match their exact header metadata, nested integer-tag framing,
  three string encodings and empty boolean body (static E-019, save-triage E-024).
  Values remain uninterpreted; no mandatory-presence or reconstruction claim follows.
  A conditionally invoked capture callback copies a runtime pointer-array count
  into its sole declared integer struct before blob serialization. A separately
  gated related-component callback first removes null/non-live owner references,
  then builds a pointer-set intersection whose result is discarded. It does not
  read that saved integer; the integer's restoration use and gameplay identity
  remain unresolved (static E-020–E-024).
  Other derived overrides and value semantics remain unresolved; component
  contents are not decoded by the native inspector.
- A second selected class has 647 records per copy. Its older-build load override
  delegates byte reading to the base method; its postload override performs an
  in-memory migration without consuming the supplied archive. Four selected
  minimum/maximum-sized bodies pass independent and native framing: 23 fields,
  1,509 header/terminator bytes, 1,812 or 1,845 opaque bytes and a zero four-byte
  trailer. Their exact header metadata matches the older class's 23 selected
  descriptors (save-triage E-025–E-027). Follow-up checks cover all 1,294 records
  in this class: every header set agrees, and one native batch independently
  matches 29,762 fields across 4,326,044 body bytes with exact boundaries and
  unchanged input hashes (E-028/E-029). Static contracts select tagged framing
  for six struct fields and recover 16 optional child descriptors. A separate
  probe, extended after identifying `UInt32Property`, frames all 24 direct struct
  payloads in the four selected parents: 64 fields and 3,632 bytes (E-030–E-032).
  A native batch matches all 24 sequences with unchanged source hashes (E-033).
  All 64 observed child headers match their optional descriptors (E-034); no fields
  are missing in these samples, without making them mandatory in other saves.
  Three grandchild structs take binary/native paths and remain opaque. Static
  contracts establish their successful 16/12-byte transfer demand and tagged
  framing for seven array element types, with 17 optional immediate fields.
  All 28 arrays in the four selected parents are empty; their mandatory shared
  headers and exact boundaries pass independent inspection (E-035). This provides
  no populated-element coverage. The wider metadata inventory finds all 9,058
  arrays in this class empty across both copies, totaling 848,864 count/shared-header
  bytes with no element payloads (E-036). Deeper child encoding, values and newer-build
  semantics remain unresolved. Private cases:
  `work/component-c028-20261012/` and `work/c028-structs-20261012/`.
  Further contracts: `work/c028-binary-children-20261012/` and
  `work/c028-arrays-20261012/` and `work/c028-array-children-20261012/`.
- A private class-string inventory finds 38 identifiers in the current copy and
  36 in the backup, all with script-class path syntax; dispatch work above covers
  two selected classes, leaving the others unverified. Both known root `Version` integers are 2,
  separate from the user-reported Steam build. Exact identifiers remain private
  (save-triage E-019); no component byte contents were interpreted.
- Supplied archive remains preserved under ignored `reference-data/`; all 24 file
  sizes, CRCs and SHA-256 values match the private inventory. Read-only inspection
  found twelve UTF-16LE prospect envelopes and twelve identical small UTF-8 tracker
  documents. All prospect envelopes pass independent Base64/zlib/length/SHA-1 checks;
  eleven payloads are distinct. Seven account/character tuples occur per prospect,
  with the claimed owner matching a member; this does not establish complete player
  state or concurrency. Private case: `work/save-triage-20261012/`. The user describes
  the producing release as “latest” and identified it as build 25793767, version
  `3.0.31.158478-rel-DangerousHorizons`. This user-reported provenance is not an
  embedded build marker or matched reference load; the preserved executable is
  older build 25621760 (`3.0.30.158174`).
  A stock Windows client exists on the user's machine but connected testing is
  deferred until it is available under a concrete scope. Continue independent work.
  The user has offered client files for reference analysis. Prepared ignored intake:
  `icarus-client-orig/25793767/`, initially requesting `Icarus/Binaries/Win64/` and
  the matching Steam app manifest. Files began arriving under Steam's `downloading`
  directory on October 12. At inspection, the manifest targets 25793767 but reports
  installed build 0 and an incomplete download; no binary identity is qualified
  yet. This can support matched-build static comparison; it does not
  replace live Windows-client tests or require packaged assets for this first pass.
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

1. Recover component-specific byte schemas beyond the bounded recorder framing
   reader, then test authored records. Older-build root/recorder framing, dispatch and version
   callbacks are characterized; outer encoding,
   integrity, measured bounds and initial membership relationships are characterized;
   actor/inventory/construction/tame/mission fields and independent confirmation of
   the reported producing build remain unresolved. Preserve unknown fields and
   private inputs; no partial importer.
2. Advance transport/control/authentication evidence and independently authored
   world-data feasibility; validate a small import before the heavy-world workload.
   The migrated original-server launcher requires explicit current sessions for
   each runtime experiment; historical authorization windows remain expired.

Update this checkpoint and the existing plan/specifications as each step completes.
Use frequent signed-off Conventional Commits and feature-branch pushes. Before PR
integration, resolve the documented policy issue: 35 historical feature commits
lack required sign-offs; preserve their history and keep the policy enabled.
Clean only owned reproducible scratch and reconcile stale documentation at
milestones. Historical reports and the private primary evidence remain retained.

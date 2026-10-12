# Implementation status

This is an independent server foundation, not yet an ICARUS-compatible server.
One bounded stateless exchange has been observed against the original. Complete
protocol interoperability, authentication, gameplay, world-data parity and
official-save import have not been demonstrated. Release reliability and
performance targets remain unproven. Last reconciled: 2026-10-12.

## Continuation — 2026-10-12

The invoked continuation has now qualified the current native reader on all four
selected C034 own bodies. Every header/value/terminator span agrees with the
independent probe, including Float: 32 fields across 58,356 bytes, exact EOF and
unchanged input hashes. A fresh helper binds the eleven-kind library and immutable
source snapshot; six authored helper tests and strict hash-verifying case review
pass. The full manual check suite also passes. No production parser change was
needed. Evidence:
`work/native-c034-own-20261012/` (E-001–E-004).

A fresh upstream metadata check confirms Rust 1.99.0 and Bevy 0.20.0 remain
current stable. SHA-2 was updated from 0.11.0 to 0.11.1 in the lockfile; the other
direct dependencies were already current. The full manual suite, release build
and bounded save/restart smoke pass; independent SHA-256 verification matches
both generations at ticks 3 and 8. Evidence: `work/dependency-refresh-20261012/`.
The preceding C034 helper remains frozen against its original library snapshot.

Two independent static tracks also completed. The older C034 own-array child
selects tagged framing under the reviewed MemoryArchive522 path, with two
optional immediate fields (Struct and Float), a signed count and one shared Struct header even when empty. Its Struct child
matches prior N02; memory size is not serialized width. Contract and independent
review: `work/c034-own-array-continuation-20261012/`. The subsequent supplied
comparison now passes on four arrays: two empty 86-byte arrays and two 28,120-byte
arrays with 214 elements each. All 428 elements/856 fields match optional Struct
and Float headers with no missing descriptors in these samples. Independent and
native spans account for 56,412 bytes: 344 count/shared-header bytes, 49,220
element-header/terminator bytes and 6,848 opaque value bytes. All inputs are
unchanged; no values or deeper widths were interpreted. Fresh helper provenance
binds the SHA-2-updated library separately from the earlier own-header helper.
Evidence: `work/c034-own-array-compare-20261012/`.

A separate static review establishes conditional successful transfer demands of
12 bytes for the exact N02 Struct and four bytes for Float. A guarded range-only
check of the frozen comparison result confirms each of the 428 Struct spans and
428 Float spans individually matches those demands. It opens no private payload
files and decodes no numeric values. Original EOF policy, numeric semantics and
newer-build equivalence remain unproved. Independent review and strict evidence
review pass: `work/c034-value-widths-20261012/`.

Matched-client reconnaissance mapped 12 archive/tag/version/serializer symbols,
with PDB/PE section agreement and independent executable-byte checks. Nine are
unwind-table starts; three are explained leaf/tail functions. Evidence:
`work/client-static-continuation-20261012/`. A subsequent bounded client
comparison independently confirms the examined 441/282/503 tag-version gates,
Float/Text common-GUID header path and generic array count/shared-header branch
at version 500 or later. Aggregate size uses a 32-bit end-minus-start difference;
large-span overflow safety is unproved. Evidence and independent semantic review:
`work/client-tag-array-20261012/`. Concrete client C034 own-array registration
now links the corresponding two descriptors to disabled native/post callbacks and
conditional tagged dispatch. Its recorder restore constructs version 522/loading
state with the relevant framing flags clear at the component Serialize handoff.
The downstream serializer is not yet traced; profile propagation, deferred N02
operations installation and dedicated-server equivalence remain unresolved.
Independent semantic review passes: `work/client-c034-own-array-20261012/`.

A live SSH fetch reconciled the stale remote cache: all checkpoint commits through
`b55e4ce` were already delivered. No client files were removed and no original
runtime or connected-client experiment has started.

The implemented Float/Text increment is `af8cf48`, following the previously
qualified struct-array increment `93ef9e9`, on `feat/native-server-foundation`.
Both increments are pushed. SSH authentication stalled; delivery succeeded using
the existing repository-owner HTTPS credential without changing Git configuration.
The project still cannot import an official world or admit an authenticated client.

Float/Text support (`af8cf48`) has completed root integration and independent
source review against the hash-bound frozen contract, the full manual suite and
release build. Both supplied copies preserve their prior envelope/root/recorder
outputs and input hashes. GUID, truncation, arbitrary opaque payload and privacy
fixtures cover the new headers; no supplied Float/Text values are qualified.
Evidence is retained in `work/native-float-text-20261012/`. The prior release
executable/library were copied and hashed before rebuilding; historical private
native helpers and source snapshots still bind the earlier nine-kind reader.

The user reported SteamCMD's exact completion message, `Success! App '1149460'
fully installed.`, for the **full** client at
`/home/gvps/github/edbfi/icarus-bevy/icarus-client-orig/25793767`.
At the pause checkpoint, `steamapps/appmanifest_1149460.acf` independently reports
`StateFlags=4`, `buildid=25793767`, target 25793767, and completed download/staging
byte totals. The final client executable and PDB exist under
`Icarus/Binaries/Win64/`, sized 109,356,544 and 1,177,653,248 bytes respectively.
Read-only identity qualification now verifies matching EXE CodeView and PDB stream-1
GUID `cb7487f1-780f-49a8-bf30-55a023b53562`, age 1. Before/after streaming hashes
and independent `sha256sum` agree; strict private case review passes. EXE SHA-256
is `ab11ec1212127e9047128843e584ab636632353edd6808a0db9caedb35048179`,
PDB SHA-256 is `944e38c941cdbdf4ec512f4307134fdd307e5e1c61602f06bd2f9600f7f98bef`.
Evidence: `work/client-identity-20261012/`. This establishes debug identity, not
publisher authenticity, complete symbol validity, save provenance or behavior.
The whole directory is gitignored. The user permits careful removal of demonstrably
unneeded files but cautions that they may support later analysis; preserve the
installation until dependencies and provenance are understood. Its availability
does not replace the deferred live Windows-client tests. The plan's restriction
against extracting/reusing packaged content in the replacement remains in force.

Private handoff artifacts remain under ignored `work/`; exact class/property names
and supplied values must stay private. These completed results supplement the
committed evidence below:

| Case | Completed result / next boundary |
| --- | --- |
| `save-triage-20261012/` | E-036: all 9,058 C028 arrays are empty. E-037: native framing matches 28 selected empty arrays. E-038: four C034 extrema match 21 top headers each, totaling 71,388 bytes with zero trailers/exact EOF. E-039: four own structs match eight immediate headers each across 58,356 bytes. Review 31 passes with 39 evidence entries and zero issues. All values remain opaque. |
| `c028-array-children-20261012/` | 17 optional immediate array-element fields; independently verified Float/Text header contracts in `static/header-kind-contracts-v2.json` (SHA-256 `17abd2de90aea26129d13cc075c7a6268eef41cf653f14dcca29928433b97e09`). |
| `c028-array-descendants-20261012/` | D01 uses tagged framing with eight optional fields and four consumed inner helpers. D02 independently demands four four-byte native transfers on success; no exact EOF guarantee. Deeper D01 types remain unresolved. Strict review passes. |
| `component-c034-20261012/` | Exact older class registration/vtable resolves to the same reviewed load/serialize/postload targets as C028. Optional top schema has 21 fields: seven Struct, two Int, seven Array, two Name, one Enum, one Str, one Bool. Four supplied bodies match this top schema under bounded comparison (save case E-038); nested payloads remain opaque. |
| `c034-own-struct-20261012/` | Own struct `0x145ee11d0` selects tagged framing; eight optional logical fields are Array, Bool, three Int, Struct, Float and Name. E-039 independently observes all eight in four selected own bodies, including Float; fresh native exact-span comparison now passes. Struct child matches earlier N02 (`0x145efe320`, successful 12-byte native demand); array child `0x145ee1220` now has a separate tagged/two-field contract. The array now has separately qualified populated element headers; Struct/Float values remain opaque. |

Continue in this order within the existing plan:

1. Float/Text source review, full checks, release build and supplied-copy regression
   qualification are complete. Preserve historical helpers and their source snapshots.
2. C034 top-header probe completed once after root/independent review and fresh
   scope/session binding. The wrapper now enforces reviewed preparation hashes,
   session/scope/prior-summary fixity and sufficient time immediately before
   launch. Original prepared files remain in `component-c034-headers/prepared-v1/`.
   Each minimum/maximum body has 21 fields, 1,451 header/terminator bytes,
   2,375/30,409 opaque bytes and a zero four-byte trailer. Input hashes are
   unchanged. Strict sample presence is not universal mandatory-field evidence.
3. Own-struct immediate-header observation is complete (E-039): 572/28,606 bytes
   per selected minimum/maximum, each with eight headers and 449 header/terminator
   bytes, exact EOF and no missing optional descriptors in these samples.
   Fresh eleven-kind native qualification now passes on every selected span.
   Own-array static review and populated shared-header/element comparison now pass.
   Next trace matched-client downstream profile propagation and recover
   the next required value contract, keeping unknown values opaque. Selected
   own-array Struct/Float widths now agree individually with the older static
   transfer demands; broader C034 top-header qualification is being prepared. The
   existing component batch helper's 8 MiB transport cap cannot fit all C034
   records from both copies together; use separate per-copy batches or a newly
   reviewed helper rather than changing frozen evidence.
4. Client identity and targeted import/symbol reconnaissance are complete. Use the
   qualified anchors: selected tag/array semantic comparison now agrees under its
   explicit branch conditions. Concrete C034 registration/conditional dispatch and
   restore-entry profile are reviewed; trace downstream propagation and the nested
   N02 deferred operations binding next. The older server
   remains build 25621760; do not transfer its addresses or behavior assumptions
   blindly to client build 25793767. No original runtime, client session or network
   experiment was started in this handoff.

Key private bindings for the next agent:

- C034 class proof: `component-c034-20261012/static/c034-candidate-framing-proof.json`,
  SHA-256 `c5541899734b5c967ad4617680219d5778a8981529badcfe25d8ebad4e36212d`.
- C034 optional top schema: `component-c034-20261012/static/c034-schema-private.json`,
  SHA-256 `e7b04c3629d922b1597f57bf948c1ae6b312764c2b9ebc91b54ac5a41f7f857f`.
- C034 own-struct private map SHA-256:
  `0a9cc629c485777c63f56dd4a20aa21d553d5921436d13af74d11b62c1ab50aa`;
  its report and artifact manifests locate the contract/map filenames.
- C028 descendant map: `c028-array-descendants-20261012/static/descendant-contracts-private.json`,
  SHA-256 `a39fa352ae09eb2684d05e73f9ec3ba6b811871eac6109bc7cc5c9b201a1a248`.
- Frozen native array helper: `native-struct-array-20261012/probe`, SHA-256
  `211570ebf49c4bd5e7e150fda3786fd65cf1be52442b1c38f30f502c9607c7fb`;
  bound release library SHA-256
  `16ee0c084c085ed09d8934a9253c70d6f59ebc9b9763273c8e8603d181f01346`.

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
  These C028 arrays are all empty; the later C034 comparison above separately
  qualifies selected supplied populated elements. The API
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
  three selected classes, with differing coverage; the others remain unverified.
  Both known root `Version` integers are 2,
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
  the matching Steam app manifest. The full client installation subsequently
  completed; the manifest reports installed build 25793767 and complete state.
  Executable/PDB GUID and age now match, with unchanged independently checked
  hashes; see the continuation checkpoint above. Save-build provenance remains
  user-reported and matched-client behavior is untested.
  This can support matched-build static comparison; it does not
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

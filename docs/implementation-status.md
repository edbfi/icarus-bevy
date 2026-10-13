# Implementation status

What the native foundation does, what it does not, and what comes next. One row
per claim lives in the [findings ledger](ledgers/findings.md); open questions in
the [unknowns ledger](ledgers/unknowns.md); preconditions in [gates](gates.md);
session detail in the [research digests](research/). Full text before
condensation: fabd2f9:docs/implementation-status.md.

Last reconciled: 2026-10-13

## Summary

This is an independent server foundation, not an ICARUS-compatible server
(F-186, F-187). One bounded stateless exchange has been observed against the
original (F-253). Complete protocol interoperability, authentication, gameplay,
world-data parity and official-save import are not demonstrated; release
reliability and performance targets are unproven.

Research is paused at the user's request (F-001). The last native change is
commit af4270b, which makes incomplete domain capture fail before publication;
its tests, manual suite, release build and smoke pass (F-003). Supplied-save
structure for four selected classes is qualified against both copies with
values left opaque ([save-class digest](research/save-format/2026-10-13-save-class-audits.md));
current-client control-table stores are located but their owner is not
([control-store digest](research/network/2026-10-13-control-stores.md)); the
C001 constructor decoder is built but its exact-window case is not yet run
([C001 digest](research/save-format/2026-10-13-c001-decoder.md)).

## Milestones

| Milestone | Status | Evidence or next gate |
| --- | --- | --- |
| M0: reference baseline | In progress | 435 files hashed; build 25621760; isolated entry-map startup observed; supplied save integrity/outer format characterized; client identity checked (F-278); exact build pairing pending |
| M1: original bug reproduction | Pending | Offline startup did not reproduce reported crashes; controlled workloads still required |
| M2: interoperability/data feasibility | In progress, partial observations | Stateless exchange observed; authenticated protocol, world-data and import feasibility gates remain open |
| M3: Bevy gameplay slice | In progress, foundation only | Rust 1.99.0 / Bevy 0.20.0 foundation tested; stock-client slice pending M2 |
| M4: complete behavior | Pending | All required compatibility matrix entries must pass |
| M5: migration/hardening | Pending, native hardening underway | Real native filesystem faults and supplied envelope integrity pass; inner real-save schema and migration fidelity remain unvalidated |
| M6: optimization/qualification | Pending | Matched reference workloads and full release gates required |

The 30 Hz development rate is provisional (F-215). Native snapshot version 1
stores authored foundation records; it is not the ICARUS save format (F-216).
No official server bug is marked reproduced or fixed (F-217).

## Works

- Builds on pinned Rust 1.99.0 / Bevy 0.20.0 with headless features and a committed lockfile (F-219, F-189).
- Direct dependencies refreshed to current stable; full suite, release build and save/restart smoke pass (F-221, F-222, F-190).
- Validates configuration, applies CLI overrides and inspects config without writing (F-226).
- Runs a fixed-step ECS loop with stable authored IDs, coherent snapshots and bounded catch-up counters (F-227).
- Saves asynchronously with a writer lock, checksummed generations and atomic manifest publication (F-228).
- Keeps two generations and recovers; corrupt/missing manifests and unsupported schemas fail closed (F-228, F-229).
- Rejects stale snapshots and stops simulation on terminal save-worker disconnect (F-232).
- Performs a final save and bounded shutdown on SIGINT/SIGTERM with JSON diagnostics (F-230, F-233).
- 120.157 s synthetic run: 100,000 entities, 60 saves, zero deadline misses and clamped frames (F-234).
- Survives 24 SIGKILL/SIGTERM restart cycles at 10,000 entities with acknowledged ticks preserved (F-235, F-236).
- Historical 7,200 s soak at 100,000 entities passed at an older revision (F-237).
- Keeps acknowledged state through real ENOSPC/EACCES faults on tmpfs and reopens cleanly (F-239).
- Inspects supplied prospect envelopes (UTF-8 and BOM UTF-16LE) within measured bounds, reporting world_imported false (F-241, F-242, F-243).
- Frames supplied save root properties and recorder boundaries to exact EOF, matching independent scanners (F-244, F-245).
- Opaque header reader covers NameProperty, UInt32Property, Float and Text headers with authored fixtures (F-246, F-247, F-206, F-207).
- Bounded library APIs frame selected components, struct arrays and Int arrays with explicit caps (F-248, F-249, F-200, F-198).
- Native framing matches independent spans across selected C009/C034/C028 save-class populations; values opaque (F-195, F-199, F-259).
- Release executable links only loader, libc and libgcc_s; no render/window/audio backends (F-250).
- Authored Python passes basedpyright recommended with 0 errors / 0 warnings (F-224, F-225).
- Offline reference launcher enforces bindings, quotas and container cleanup in mocked tests (F-252).

## Does not work

- Not an ICARUS-compatible server; only an independent foundation (F-186).
- No authenticated protocol, stock-client session or production game listener (F-187, F-240, F-253, U-038).
- No gameplay, world-data parity or official-save import; import fails explicitly (F-187, F-229, U-045).
- Save inspection frames structure only; nested values, schemas and meanings remain opaque (F-244, F-245, U-046).
- Native snapshot v1 is not the ICARUS save format; no save migration (F-216, F-269).
- 30 Hz tick rate is provisional, not an ICARUS property (F-215).
- No ICARUS performance or stability improvement measured; synthetic runs only (F-234, F-238, F-269, U-049).
- Historical soak predates later persistence changes and does not qualify the current revision (F-237).
- Physical power loss and the required 72-hour runs are untested (U-035).
- Shutdown fix does not preempt an in-progress OS write or synchronous capture (F-233).
- Hosted CI has not run from this workspace (F-251, U-036).
- No original-server bug reproduced or fixed; offline startup did not reproduce crashes (F-217, F-268, U-047).
- Offline launcher revision has not executed the original server (F-252).
- Checksum admission policy and runtime callback registration unresolved (F-254, F-255, U-037).
- Client static correspondences do not establish dedicated-server equivalence (F-194, U-031).
- Client identity is debug identity only, not authenticity or behavior (F-210, U-034).
- Producing save build is user-reported, not embedded or matched (F-263, U-040).
- C001 construction/dispatch bridge and control-store owner remain unproven (F-270, F-274, U-041, U-043).

## Gates

Every binding precondition is in [gates.md](gates.md). The ones that
bind the next task: complete the independent fixture-v2 review and run its ten
authored modes before preparing any C001 original-code case; do not execute the
prepared files before that review; preserve the failed nine-mode receipt and
the separate strict replay as non-passing; do not repeat completed C013, C028,
C034 or D01 structure work; each original-server run needs an explicit current
launcher session, since historical authorization windows are expired.

## Next tasks

Research remains paused; these resume in order when work is reauthorized.

1. Complete independent fixture-v2 review and the ten authored modes before preparing any original-code case (U-041).
2. Build and test a standalone exact-window decoder to recover C001 instruction evidence within the 305/296-byte windows (F-270, F-271, U-041, U-042).
3. Establish C001's complete concrete construction/vtable/archive-dispatch bridge from retained seeds and fresh bounded metadata (F-270, U-041).
4. Close the census using completed C028/C034/C013 audits; leave native value meanings and empty inherited arrays open (F-272, F-273, U-046).
5. Keep automatic class/schema recognition and value meanings open; no partial importer (U-046, U-040).
6. Resolve actor/inventory/construction/tame/mission fields and confirm the reported producing build independently (F-262, F-263, U-040).
7. Establish actual receiver/indirect dispatch and a same-build type-registration join after the local post-call gap (F-274, U-043).
8. Compare constructor metadata/instructions guided by paired-client symbols and the two named notify slots (F-274, U-043).
9. Establish base origin and owner/caller evidence for the Pending/World stores before inferring the dispatcher caller (F-274, U-043).
10. Bind the concrete archive/outer framing path, then promote the experimental control-body fields (U-044).
11. Advance transport/control/authentication evidence (F-253, F-254, U-037, U-038).
12. Advance independently authored world-data feasibility; validate a small import before the heavy-world workload (F-242, U-045).
13. Run each original-server experiment only under an explicit current launcher session (F-218, F-252).
14. Resolve the 35 unsigned historical commits policy issue before PR integration (F-275).

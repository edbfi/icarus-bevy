# Native foundation, tooling and earlier research receipts

Covers what the native Rust/Bevy foundation demonstrably does, its build/test/dependency/tooling receipts, delivery of recent increments, client identity, the summary of earlier original-server research cases, milestone status and the pending task list; save-class audits (C009, C028, C034) and C001/control-store work appear only as one-line results. Original server build 25621760, client build 25793767.
Full text: fabd2f9:docs/implementation-status.md lines 1-13 and 941-1590.

## Question cluster

- Is the native project an ICARUS-compatible server yet, and what has it demonstrated (F-186, F-187)?
- Which toolchain, dependency and Python-typing receipts back the foundation (F-219 to F-225, F-189, F-190)?
- What persistence, lifecycle and fault-tolerance behavior is implemented and measured (F-226 to F-239)?
- What save inspection and framing does the native CLI/library provide (F-240 to F-249)?
- What did earlier original-server research cases establish (F-253 to F-269)?
- What remains on the task list, and under which preconditions (F-270 to F-275, U-041 to U-045)?

## Answered

- Not yet ICARUS-compatible; only an independent foundation (F-186, F-187).
- Pinned Rust 1.99.0 / Bevy 0.20.0 remain current; stable-compatible lock refresh, fixed-tree full suite, release save/restart and five rejection cases pass (F-413; M-174, M-184). Earlier direct refresh: F-219, F-221, F-222, F-189, F-190, M-078.
- Bevy skill updated from a pinned upstream commit with exact file match (F-220, M-131).
- Authored Python passes basedpyright recommended at 0 errors / 0 warnings (F-224, F-225).
- Config validation, fixed-step loop, checksummed atomic saves, two-generation recovery, fail-closed manifests, signal shutdown (F-226 to F-230).
- FIFO manifest/generation startup hangs reproduced; opened-handle regular-file checks and nonblocking opens reject special inputs while retaining valid previous state (F-405; M-175).
- Config FIFO startup hang reproduces; opened-handle regular/64KiB guards pass10CLI tests and four fixed release cases. Exact-boundary configs, regular symlinks, UTF8/I/O causes and overrides remain supported, with no source/state writes; full suite passes (F-443; M-240, M-241).
- Current-generation EACCES reproduced rollback from valid tick20 to10 and deletion of the unreadable generation. Non-NotFound I/O now fails closed;17source checks,26foundation tests,injectedEACCES/EIO and five UID1000 release cases pass. Permission repair, missing/corrupt recovery and future-schema refusal remain; failed lead oracle and initial Clippy suite are preserved, final manual suite passes (F-452; M-251, M-252).
- Both manifest digest fields require canonical 64-digit lowercase hex before selection; load/save/startup reject malformed references without rewriting state, while valid mismatch recovery remains (F-417; M-191).
- Release reproduction confirms twelve malformed current/previous references were admitted before and reject without writes after; valid mismatch recovery still publishes generation three retaining generation one (F-418; M-192).
- Offline execution resumed; source/runtime scopes remain separate from connected lab authority (F-404; M-174).
- Stale-snapshot rejection, save-worker disconnect stop and N-004 shutdown deadline fix are regression-tested (F-232, F-233, M-165).
- Synthetic 120.157 s run and 24-cycle kill/restart campaign pass on the N-004 revision (F-234, F-235, M-132).
- Before object admission changed, the 120.138 s workload passed at 100,000 entities with 60 saves, zero missed ticks/clamps, exact independent two-generation checks and 28,924 KiB whole-process peak RSS; source-only reviews were concurrent (F-427; M-212, M-213).
- Updated object-gated reader/writer passes 22 generations at 100,000 entities with exact final/previous state, two warmups and 20 measured pairs; process peak RSS is 13,964 KiB (F-429; M-218).
- Real ENOSPC/EACCES faults preserve acknowledged state (F-239, M-138).
- Envelope inspection, N-005 UTF-8 validation, root-tag and recorder framing match independent scans on both supplied copies (F-241 to F-245, M-139 to M-141).
- Envelope object admission and malformed-input privacy regressions pass across all modes/encodings (F-411, F-412; M-182).
- Native manifest/header/snapshot parse diagnostics drop raw Serde strings/keys and causes; eight disclosure reproductions and API/CLI preservation regressions pass, including both corrupt generations (F-423; M-203, M-204).
- Release build and nine malformed cases pass with stable binary/input hashes, sanitized diagnostics and unchanged files; both corrupt generations report separately (F-425; M-207, M-208).
- Native object gates reject positional metadata/records; malformed array headers recover valid backup while proper future objects refuse rollback. Serialization, Position triples and optional previous metadata stay compatible; final manual suite passes with serialized fixtures (F-428; M-214 to M-220).
- Current FIFO fix release build/save-restart smoke and release FIFO rejection pass (F-405; M-183).
- NameProperty, UInt32Property and Float/Text headers plus bounded component, struct-array and Int-array APIs are tested (F-246 to F-249, F-206, F-207, F-198, F-200).
- Float/Text and struct-array increments are pushed; all checkpoint commits through the reconciled head were delivered (F-204, F-205, M-109 to M-111).
- Release executable has no rendering/window/audio backends (F-250).
- Full client build installed and debug-identity matched between executable and PDB (F-209, F-210, M-113 to M-117).
- Save-class audits completed: C034 populations, C009 own/parent structure, C028 framing (F-188, F-191, F-195, F-196, F-199, F-202, F-211, F-212, F-259).
- Supplied archive integrity verified; envelopes pass Base64/zlib/length/SHA-1 (F-261, M-151).
- Isolated stateless exchange observed; offline startup reached the entry map (F-253, F-268).
- Bounded Ghidra pass exported zero functions (F-266).

## Partial

- Client static correspondences for tag/array gates, C034 dispatch and Float transfer are bounded only (F-193, F-194, U-030, U-031).
- Checksum: one early value matches; setter and invalidation resolved; admission policy open (F-254, F-255, U-037).
- Older-build inner save dispatch and framing recovered; values and newer-build migration not (F-257, F-258, U-039).
- C034 Struct/Float widths bound by static demand and range checks; semantics open (F-192, U-029).
- C009 common-child selection measured; CA01 exceeds the default array cap (F-203, U-032).
- C001 exact windows now decode; local table stores match, while receiver continuity, archive dispatch and retained listing rejection cause remain open (F-409, F-410, U-041, U-042).
- Control Pending/World stores qualified at fixed offsets; owner not discriminated (F-274, U-043, U-044).
- Seven account/character tuples per prospect; player state incomplete (F-262, U-040).
- Historical soak and recovery evidence predates current persistence code (F-236, F-237).

## Unresolved

- Authenticated protocol and stock-client session (U-038).
- World-data import feasibility (U-045).
- Actor/inventory/construction/tame/mission fields and producing-build confirmation (U-040).
- Original bug reproduction under controlled workloads (U-047).
- ICARUS performance/stability comparison (U-049).
- Power-loss and 72-hour runs (U-035); hosted CI (U-036).
- C001 construction bridge (U-041); control receiver/registration join (U-043).
- Automatic schema recognition and value meanings (U-046); C028 deeper children (U-048); other class identifiers (U-050).
- Client publisher authenticity and symbol validity (U-034).

## Limitations and exclusions

- 30 Hz tick rate is provisional (F-215); native snapshot v1 is not the ICARUS save format (F-216).
- No original bug reproduced or fixed; no save migration; no ICARUS performance measured (F-217, F-269).
- Synthetic runs are not ICARUS performance or sustained release qualification (F-234, F-235, F-238).
- Save framing reports counts/sizes only; values remain opaque and no gameplay counts follow (F-244, F-245).
- Production run command has no game listener (F-240).
- Offline launcher revision has not executed the original server (F-252); historical authorization windows expired (F-218).
- Save producing build is user-reported only (F-263); server and client builds differ (F-214).
- Component transport cap forces per-copy C034 batches (F-213).
- Test counts and library hashes are historical receipts, not a fresh aggregate result.
- Native storage object gates leave direct domain-root Serde deserialization general; stored records are gated, and no official save schema follows (F-428).
- Preconditions for next work are listed verbatim in gates.md.

## Receipts

- Manifest digest admission reproduction/review/regressions (M-191); before/after release cases and recovery (M-192).
- Native JSON diagnostic source review, disclosure reproduction and regressions (M-203, M-204); independent release validator review and execution (M-207, M-208).
- Current synthetic workload validator review and execution (M-212, M-213).
- Native object audit, source/test/release reviews and execution, preserved failed suite and final qualification (M-214 to M-220).
- Config input source review, FIFO reproduction, bounded CLI/release validation and preserved lead receipt-checker oracle failure (M-240, M-241).
- Current-generation read-fault review, rollback/deletion reproduction, preservation/repair controls and final full suite (M-251, M-252).
- Dependency refresh and smoke (M-078, M-184); native deadline and recovery runs (M-132, M-133).
- Historical soak, provenance and start commit (M-134 to M-136); snapshot measurement (M-137).
- Filesystem fault probe (M-138); codec review (M-139); root tags (M-140); recorders (M-141).
- Header readers and component API (M-142 to M-144); Float/Text (M-112).
- Client identity and hashes (M-115 to M-117); app manifest (M-114).
- Save-class case directories and private bindings (M-077, M-079 to M-108, M-118 to M-130).
- Earlier research reports and specs (M-152 to M-164); offline launcher (M-145); control-version case (M-146).

# Findings: Save format: C001 constructor

Active rows: C001 callback and candidate-constructor location, standalone decoder, exact-window cases and frame replay.

F- IDs form one space across `docs/ledgers/findings/`; retired rows keep their
full text under `retired/`. Admission and columns follow `.agents/rules/docs.md`.

Before restructure: 96ddb0b:docs/ledgers/findings.md

Last reconciled: 2026-10-14

| ID | Claim | Kind | Evidence | Scope and limits | Unknowns | Status |
| --- | --- | --- | --- | --- | --- | --- |
| F-431 | Normal-return Win64 ABI inference can conditionally preserve RBX; unwind intent alone cannot establish receiver continuity | inferred | source-review; M-224 | medium; default ABI and local static joins; requires local paths, save-slot/home-area and argument joins; no target conformance or archive proof | U-041 | active |
| F-465 | Old slot preparation has two inconsistent address sums; hypothetical frame success alone cannot clear ownership | observed | source-gate-audit; M-265, M-266 | high; source-only arithmetic/inference; no replacement selected/read; concrete origin/path/final-vptr/slot contracts still needed; no helper QA, target or arbitrary transitive-memory proof | U-041 | active |
| F-469 | C001 decoder fixture v2 (nonzero-call oracle) passes independent review and ten fresh authored modes; one-window 106-B and two-window 196/217-B variants pass ten modes each; the original nine-mode suite failed and is preserved | observed | authored-test; M-003, M-004, M-176, M-177, M-187, M-188, M-201, M-202 | high; authored bytes only; no construction proof | U-001 | active |
| F-470 | Retained readonly C001 listing query rejects after a Ghidra script exception; a diagnostic variant places the rejection at the first listing-entry check at 0x1419fe870 but cannot separate a missing entry from an API exception | observed | runtime-capture; M-007, M-008 | high; no listing gap, instruction, type or bridge result | U-003, U-042 | active |
| F-471 | Older-server exception directory validates 323,249 records / 3,878,988 B; C001 callback 0x1419fe870..0x1419fe9a1 (305 B) and candidate constructor 0x141788530..0x141788658 (296 B) each start a metadata interval | observed | static-bytes; M-011 | high; metadata only; intervals may be procedure fragments; no decode or construction bridge | U-004, U-005 | active |
| F-472 | C001 native getter 0x1419fe200 has one retained incoming reference, DEFAULT/non-call from 0x14655d468, with no containing function; reflection getter 0x141a00220 is separate; four C001 records carry retained parent metadata | observed | static-decompile; M-013 | high; retained coverage only; registration flow and caller absence unproved | U-006, U-023 | active |
| F-473 | Standalone C001 decoder decodes fixed x64 buffers via the language/PseudoInstruction API without Program/database, PE import or analyzers; unsupported fallbacks reject; a corrected checker verifies widths, registers, constants and p-code writes | observed | authored-test; M-014 | high; authored bytes only; first checker error preserved | U-002 | active |
| F-474 | Exact C001 windows decode with zero lookahead: callback/candidate 305/296 B as 51/49 instructions; common callee 0x1418824d0 (106 B) as 24; its callees (196/217 B) as 45/48; byte replays agree | observed | static-instruction; M-179, M-180, M-185, M-186, M-189, M-190, M-197..M-199, M-205, M-206 | high; no complete function, receiver, ABI or archive proof | U-041 | active |
| F-475 | Both C001 windows call 0x1418824d0; the callback inlines 40 typed instructions storing 0x1449d9f90 via post-call RBX. Declared CFGs: callback/candidate 99 edges, 2 calls, 2 returns; nested 93 edges, 10 opaque calls, 3 returns | observed | static-pcode; M-181, M-193..M-195, M-209..M-211, M-221..M-223 | medium; graphs open; receiver continuity unproved | U-004, U-041 | active |
| F-476 | C001 frame inputs qualify: three 12-byte v1 unwind records (register 3 pushes, 48/32/48-byte allocations, opaque handlers); 1,480 register descriptors with six named GPR shapes; four byte-addressed spaces; opcode constants 27 and 72-75 | observed | static-metadata; M-226..M-236, M-243, M-244, M-254, M-255, M-262 | high; storage geometry only; handler data unread | U-041 | active |
| F-477 | C001 frame cases exit 0 but stop at unsupported operations (opcode 27, COPY width, opcode 26, partial and mixed COPY/LOAD; one hit a 64 KiB output cap); a compact case completes the callee model; constructors stop at XMM1 | observed | controlled-partial; M-232, M-233, M-237..M-239, M-242, M-245..M-250, M-253, M-256..M-261, M-263, M-264, M-267, M-268 | high; gates false | U-041 | active |

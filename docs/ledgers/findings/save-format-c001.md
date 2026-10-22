# Findings: Save format: C001 constructor

Active rows: C001 callback and candidate-constructor location, standalone decoder, exact-window cases and frame replay.

F- IDs form one space across `docs/ledgers/findings/`; retired rows keep their
full text under `retired/`. Admission and columns follow `.agents/rules/docs.md`.

Before restructure: 96ddb0b:docs/ledgers/findings.md

Last reconciled: 2026-10-22

| ID | Claim | Kind | Evidence | Scope and limits | Unknowns | Status |
| --- | --- | --- | --- | --- | --- | --- |
| F-431 | Normal-return Win64 ABI inference can conditionally preserve RBX; unwind intent alone cannot establish receiver continuity | inferred | source-review; M-224 | medium; default ABI and local static joins; requires local paths, save-slot/home-area and argument joins; no target conformance or archive proof | U-041 | active |
| F-910 | C001 slot notes conflict: base 0x1449d9f90 (F-908) plus 0xa0/0x488/0x4a0 gives 0x1449da030/0x1449da418/0x1449da430, the stated +0x488/+0x4a0 ones 0x4000 high, so hypothetical frame success cannot clear slot ownership | inferred | source-gate-audit; M-147, M-148, M-265, M-266 | medium; arithmetic; vptr, origin, slot contract unproved | U-041 | active |
| F-907 | The standalone C001 decoder decodes fixed x64 buffers via the PseudoInstruction API without Program, PE import or analyzers; fixture v2 (nonzero-call oracle) and 106-B and 196/217-B variants pass ten modes; the first suite failed | observed | authored-test; M-003..M-004, M-014, M-176..M-177, M-187..M-188, M-201..M-202 | high; authored bytes only | U-001, U-002 | active |
| F-905 | Server exception directory valid (323,249 records); .pdata 103538 (callback 0x1419fe870, 305 B) and 95461 (candidate 0x141788530, 296 B) are whole C001 functions (unwind v1, rbx saved); the old listing rejection was -noanalysis (null) | observed | static-bytes, static-decompile; M-007, M-008, M-011, M-761/ev_73f0199d703e, M-762 | high; server 25621760 | U-003..U-005, U-042 | active |
| F-906 | C001 getter 0x1419fe200 matches the client's GetPrivateStaticClass (178 B) but for 13 rel32 fields, its ctor field 0x1419fe870 being InternalConstructor<C001>; sole reference 0x14655d468 is .pdata (F-722); reflection getter 0x141a00220 differs | inferred | static-decompile, static-bytes; M-013, M-308, M-314 | high; positional; registration unexecuted | U-006, U-023, U-041 | active |
| F-474 | Exact C001 windows decode with zero lookahead: callback/candidate 305/296 B as 51/49 instructions; common callee 0x1418824d0 (106 B) as 24; its callees (196/217 B) as 45/48; byte replays agree | observed | static-instruction; M-179, M-180, M-185, M-186, M-189, M-190, M-197..M-199, M-205, M-206 | high; no complete function, receiver, ABI or archive proof | U-041 | active |
| F-908 | C001 windows call 0x1418824d0, then (candidate: 49 states, RBX kept; callback via RBX) store 0x1449d9f90 at +0, 0x1449da448 at +0x28 of RCX: primary and IInterface_AssetUserData vftables, per the client ctor | inferred | static-pcode, controlled-partial, static-bytes; M-181, M-193..M-195, M-209..M-211, M-221..M-223, M-303..M-309 | high; positional; ABI assumed | U-004, U-041 | active |
| F-476 | C001 frame inputs qualify: three 12-byte v1 unwind records (register 3 pushes, 48/32/48-byte allocations, opaque handlers); 1,480 register descriptors with six named GPR shapes; four byte-addressed spaces; opcode constants 27 and 72-75 | observed | static-metadata; M-226..M-236, M-243, M-244, M-254, M-255, M-262 | high; storage geometry only; handler data unread | U-041 | active |
| F-477 | C001 frame cases exit 0 at controlled stops; the callee model completes; constructors stop at a RAX low16 store; admitting AX/AL reads overflows the 64 KiB bundle cap | observed | controlled-partial; M-232, M-233, M-237..M-239, M-242, M-245..M-250, M-253, M-256..M-261, M-263, M-264, M-267, M-268, M-272..M-276, M-284..M-286, M-298..M-300 | high; gates false | U-041 | active |
| F-909 | C001 vftable +0xa0 Serialize calls UActorComponent::Serialize then +0x488 (24-B call-free version hook); +0x4a0 calls the base restore then +0x4b0 apply-transform; server 0x1418aaf70/0x14189d380/0x1418a9bb0/0x141887d20 match the client but for constants; C028 inherits two | inferred | static-bytes; M-310, M-312, M-313, M-348..M-350 | high; positional; not runtime vptrs | U-041 | active |
| F-722 | 0x14655d468 is .pdata 103518 for 0x1419fe200 (48 refs; CRT 0x1406a8ab0 -> 0x142187f70, 0x141a00220 via 0x144abc1b0); 0x1418824d0 keeps rbx; 0x1418a9c80 (696 B) matches client 0x1418ae6c0 (507 B) + log and calls +0xa0 (0x1418aaf70) -> +0x488; +0x4a8=0x14189a760 | observed | static-decompile; M-761/ev_73f0199d703e, M-762 | high; server 25621760 | U-004, U-006, U-023, U-041 | active |

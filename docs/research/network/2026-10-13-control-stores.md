# Current-client control stores and initial control archive

Covers static evidence for UWorld/UPendingNetGame local table stores, constructor metadata bounds and instruction windows, public symbols and FNetworkNotify slots, control-store prefixes and post-call stores (current client, build-25793767 where stated), plus the experimental initial control-body reader and the bounded dispatcher caller/archive-path investigation.
Full text: fabd2f9:docs/implementation-status.md lines 69-116, 170-194, 232-257, 329-349, 377-393, 445-456, 477-489, 711-795.

## Question cluster

- Which table constants do World and Pending store locally after their constructor calls, and at which displacements (F-063, F-076, U-007).
- What do the constructor metadata intervals and fixed instruction windows bound (F-077, F-082, F-083, U-008).
- Which public symbols and notify slots connect the tables to NotifyControlMessage (F-086, F-088).
- What do control-store prefixes and the post-call gap establish about the store base (F-091, F-093, U-013).
- Which archive path and callers feed the control dispatcher (F-099, U-014, U-015).

## Answered

- World stores the primary table constant at +0 (0x143ca5a64) and the secondary FNetworkNotify constant at +0x28 (0x143ca5a79) on one unchanged unknown base (F-062, F-063).
- Pending, after CALL 0x143a58679, stores its primary and FNetworkNotify table constants at +0 and +0x28 on one unknown base; constants match same-build public symbols (F-075, F-076).
- World's CALL is at 0x143ca5a55 with return 0x143ca5a5a (F-074).
- Constructor symbols equal their exception-interval starts: UWorld 8 bytes, UPendingNetGame 71 bytes, over a fully validated directory (F-082, F-083).
- UWorld's interval holds four prologue instructions ending at 0x143ca5a48; Pending's window stops at a call at 0x143a58659 (F-079, F-080).
- PDB/executable identity for build-25793767 validated; 17 public records / 15 addresses selected (F-086, F-087).
- Each named FNetworkNotify secondary table's +0x18 slot equals its NotifyControlMessage address (F-088).
- Post-call gap closed: two constant stores and four 8-byte zero stores on an unchanged unknown base (F-091, F-092).
- One control-store prefix keeps its base through a load from +0x620 into stores at +0 and +0x28 (F-095).
- Two 7-byte LEA instructions materialize 0x145468180; two 8-byte stores of it land at base +0x28 (F-110, F-113).
- Experimental initial-body reader composes the observed byte/u32/FString sequence within a 131,081-byte ceiling and passes its validation suite (F-096, F-097, F-098).

## Partial

- World/Pending share zero/+0x28 geometry, which cannot identify the older owner (F-065, U-011).
- The 05:15 World run qualified only the primary store and a +0x68 store; its lookahead was not promoted (F-066, F-067, F-068).
- The 0x290 primary/secondary distance is image placement only (F-089).
- One prefix ends at a call with no provenance crossing it (F-094, U-009).
- Dispatcher caller scans: one absolute pointer candidate at 0x145468198, no rel32 match; two LEA candidates targeting slot 0x145468180; three exception ranges selected (F-104, F-107, F-108, F-109, U-015, U-017).
- 128-byte pointer window ends at the dispatcher; no qualifying locator pointer (F-106, U-016).
- Direct Ghidra fallback found one non-call reference with no containing function; database is partial (F-101).
- Forced metadata decompilation is a limited lead only (F-112).

## Unresolved

- Receiver origin, ABI and object layout of the store base (U-007).
- Complete World/Pending constructors (U-008).
- Cross-call provenance (U-009).
- Executed runtime dispatch (U-010).
- Older-server ownership (U-011).
- Instruction at 0x143ca5a81 beyond the 40-byte output (U-012).
- Class, type registration, vtable, entry/parameters, receiver and caller of prefixes and post-call base (U-013).
- Archive/bit-position/byte-order path feeding the dispatcher (F-099, U-014).
- Indirect/other-addressing callers (U-015); RTTI, table identity, owner (U-016); function/owner provenance for selected ranges (U-017); base parameters and indirect dispatch for the 0x145468180 stores (U-018).
- Live REA provider exact-window decoding and lazy Ghidra startup (F-085, F-100, U-019).

## Limitations and exclusions

- All results are static; no runtime instructions, original executable or live control message ran (F-071, F-093, F-098).
- Fixed windows only: no Program, PE import, analyzer, decompiler or flow traversal in instruction cases (F-071, F-077).
- Metadata intervals are procedure-fragment bounds, not complete function identities (F-083, F-109).
- Original-byte correspondence for F-077 rests on a separate root qualifier receipt (F-077).
- Initial-body reader keeps negotiation/identity/game-connection flags false and has no listener, version admission or token state machine (F-096, F-097).
- The fallback wrapper failure cause (zombie race) is only plausible (F-102).
- Preserved failures: partial-register-clobber checker (F-073), offline hostname diagnostics (F-081), fixture-class contamination (F-111), REA startup timeouts (F-100).

## Receipts

- World stores: M-023, M-024, M-025, M-026 (F-064, F-069, F-070).
- Continuation and bounded constructor instructions: M-027, M-028, M-029, M-030 (F-072, F-073, F-078).
- Constructor metadata bounds and REA routing limitation: M-031, M-032, M-033 (F-084, F-085).
- Public symbols, post-call stores, prefixes: M-034, M-035, M-036 (F-090, F-091, F-093).
- Initial-body reader and release library: M-037, M-038 (F-098).
- Caller investigation report and scans: M-039, M-040, M-041, M-042, M-043, M-044, M-045 (F-100 to F-114).
- Tracked tools: PDB reader tests (F-090), Ghidra exporter (F-103), PE parser M-046 (F-105), numeric exporter (F-114).

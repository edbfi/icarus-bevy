# Native foundation and reference baseline

What the native Rust/Bevy foundation demonstrably does (lifecycle,
persistence, fault handling, save inspection), the toolchain and typing gates
behind it, the client identity and the earlier original-server baseline.
Original server build 25621760; client build 25793767. Save-class, C001 and
control-store work have their own digests.

Before restructure: 96ddb0b:docs/research/foundation/2026-10-13-foundation-receipts.md

## Question cluster

- Is the native project an ICARUS-compatible server, and what has it demonstrated?
- Which lifecycle, persistence and fault-handling behavior is implemented and measured?
- What save inspection does the native CLI and library provide?
- Which toolchain and typing gates back the foundation?
- What did the earlier original-server cases establish?

## Current answer

The project is an independent foundation, not an ICARUS-compatible server
(F-487). It validates configuration (F-226), runs a fixed-step loop at a
provisional 30 Hz (F-227, F-215) and persists checksummed native generations
with atomic manifest publication and two-generation recovery (F-228, F-229);
native snapshot v1 is not the ICARUS save format (F-216). Capture rejects
incomplete domain state before publication (F-482), stale snapshots and
save-worker loss stop safely (F-232), and signals trigger a final save within a
bounded shutdown (F-230, F-233).

Reproduced faults are fixed for FIFO manifests, generations, configs and
save-inspection inputs (F-405, F-443, F-468), malformed manifest digests
(F-417), positional JSON arrays (F-428), current-generation read errors (F-452)
and diagnostics that leaked supplied strings (F-412, F-423); real ENOSPC and
EACCES faults keep acknowledged state (F-239). Synthetic runs pass at 100,000
entities and across kill/restart cycles (F-485); the 7,200 s soak predates later
persistence changes (F-237). A modeled POSIX power-loss harness over the real
write path recovers acknowledged state in every enumerated crash state and found
stranded temporaries, now removed (F-511). None of this measures ICARUS performance.

Save inspection reads supplied prospect envelopes within measured bounds and
frames root properties and recorders to exact EOF, matching independent
scanners with values opaque and `world_imported` false (F-241 to F-245, F-411).
Both supplied copies are preserved losslessly in a write-once schema-3
sidecar and reload byte-identically; later saves stay small (F-567). A seeded world
runs 180 autosaves without deadline misses (F-584). Randomized mutations
never panic preservation or the replication codecs, accepted inputs re-encode
exactly, and the streaming verifier matches the tree decoder (F-604, F-605);
mutated datagrams and control bodies never panic their readers or the peer
lab, whose table stays within capacity (F-606, F-607).
Bounded header, component, struct-array and Int-array readers are tested
(F-246 to F-249, F-200, F-207). The production run command has no game listener
(F-240).

The toolchain is pinned and current (F-483), authored Python passes
basedpyright recommended (F-484), and the release links no render, window or
audio backend (F-250); the CI definition passes on hosted runners for a feature commit (F-497). The
client's debug identity matches (F-486). Earlier original-server cases hashed
the baseline (F-265), exported no functions in one bounded Ghidra pass (F-266),
reached the entry map offline without reproducing crashes (F-268) and observed
one stateless exchange (F-253). The offline launcher revision has not run the
original server (F-252), and historical runtime windows have expired (F-218).

## Open

- Authenticated protocol and stock-client session (U-038); world-data import feasibility (U-045).
- Original bug reproduction under controlled workloads (U-047); ICARUS performance and stability comparison (U-049).
- Power-loss and 72-hour runs (U-035).
- Client publisher authenticity and symbol validity (U-034).

## Artifacts

M-001, M-002, M-006, M-009, M-010, M-015..M-022, M-076, M-078, M-109..M-117, M-131..M-165, M-174, M-175, M-178, M-182..M-184, M-191, M-192, M-200, M-203, M-204, M-207, M-208, M-212..M-220, M-225, M-240, M-241, M-251, M-252, M-269, M-270, M-271, M-277, M-311, M-391, M-397..M-399, M-411..M-419, M-452..M-454, M-471..M-475.

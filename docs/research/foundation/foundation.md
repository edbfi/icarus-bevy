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
(F-487). It validates configuration (F-868), runs a fixed-step loop at a
provisional 30 Hz (F-868) and persists checksummed native generations
with atomic manifest publication and two-generation recovery (F-869, F-771);
native snapshot v1 is not the ICARUS save format (F-869). Capture rejects
incomplete domain state before publication (F-482), stale snapshots and
save-worker loss stop safely (F-869), and signals trigger a final save within a
bounded shutdown (F-869).

Authored worlds retain full spatial state through schema 4 (F-919), strictly
bound C001 inert-fixture provenance through schema 5 (F-925), and edited
foundation links through schema 6 (F-922). Schema 7 adds atomic owner find/spawn/
replacement, explicit dormant slots and durable retry completion (F-923).
Schema 8 connects authored C001 policies and poses to that lifecycle while
retaining immutable source provenance. Schema 9 adds authored Bool/I32 use-gate
actions; consumed and disabled state survives reload without source replay
(F-925). Preparation rejects unsupported state; capture rejects damaged membership or activation contexts. Publication
faults and modeled crashes preserve acknowledged state. These contracts establish
neither ordinary official-save import, physics/navigation, C009 restoration nor
client compatibility; procedures are in the
[authored-worlds runbook](../../runbooks/authored-worlds.md).

Authored schema-7/8/9 costs are measured at 3–1,024 slots (F-926). Count-only
preflight preserves stored bytes and removes its outer generation buffer; source
replay remains. No uniform latency, peak-memory or tick-loop improvement is
established.

Reproduced faults are fixed for FIFO manifests, generations, configs and
save-inspection inputs (F-671), malformed manifest digests
(F-870), positional JSON arrays (F-870), current-generation read errors (F-870)
and diagnostics that leaked supplied strings (F-880, F-423); real ENOSPC and
EACCES faults keep acknowledged state (F-239). Synthetic runs pass at 100,000
entities and across kill/restart cycles (F-871); the 7,200 s soak predates later
persistence changes (F-871), and a later soak stopped early at 21.9 h (F-872).
Compressed simulated time now runs the full 72-hour tick and save schedule
in under an hour, at 100,000 synthetic entities and for a world seeded from
the supplied save, without save failures or retained growth, one tick missing its deadline
(F-872, F-873). A modeled POSIX power-loss harness over the real
write path recovers acknowledged state in every enumerated crash state and found
stranded temporaries, now removed (F-511). None of this measures ICARUS performance.

Save inspection reads supplied prospect envelopes within measured bounds and
frames root properties and recorders to exact EOF, matching independent
scanners with values opaque and `world_imported` false (F-880 to F-883).
Both supplied copies are preserved losslessly in a write-once schema-3
sidecar and reload byte-identically; later saves stay small (F-567). A seeded world
runs 180 autosaves without deadline misses (F-873). Randomized mutations
never panic preservation or the replication codecs, accepted inputs re-encode
exactly, and the streaming verifier matches the tree decoder (F-604, F-605);
mutated datagrams and control bodies never panic their readers or the peer
lab, whose table stays within capacity (F-606, F-607); corrupted snapshot
stores load a committed snapshot exactly or fail closed (F-611).
Bounded header, component, struct-array and Int-array readers are tested
(F-886 to F-888). An opt-in loopback session layer in `run`
admits authored probes, enforces the player limit, evicts idle sessions, closes
all before the final save and retains no session over 1,000 join/close cycles
(F-875); it pins the Hello version to the release profile and survives 200
real-binary cycles (F-875). Held probe sessions (one for 8 hours, eight for an
hour) receive steady GameState updates without regressions or save failures;
eight held 10.1 hours until a host reboot, whose SIGTERM shut the server down
cleanly, and the world reloaded at its last save (F-833). A later interrupted
rerun has no final probe result or exit receipt (M-898); the multi-day hold
remains unqualified (U-035). In 300 s sweeps of 8
to 64 concurrent sessions tick p99 stays under 80 us; a networking status over
one bunch, once withheld, now goes out as unreliable partial bunches that every
session reassembles (F-877). No ICARUS client has used it. Under input
parity (F-828), `install-official-data` and `icarus-fetch` fetch the free
official server's data.pak (F-829) anonymously from Steam without steamcmd,
matching the steamcmd install byte for byte (F-831).

Toolchain and headless linkage are qualified by F-879; Python typing by F-484,
hosted CI by F-497, and client identity/version correspondence by F-878/F-731
(U-034 resolved). Original baseline fixity, the unsuccessful Ghidra export,
offline entry-map run and stateless exchange remain bounded by F-265/F-266/
F-268/F-253. The launcher revision remains unrun (F-252); historical runtime
windows expired (F-218).

## Open

- Authenticated protocol and stock-client session (U-038); world-data import feasibility (U-045).
- Original bug reproduction under controlled workloads (U-047); ICARUS performance and stability comparison (U-049).
- Paced multi-day effects and physical power loss (U-035).

## Artifacts

M-001, M-002, M-006, M-009, M-010, M-015..M-022, M-076, M-078, M-109..M-117, M-131..M-165, M-174, M-175, M-178, M-182..M-184, M-191, M-192, M-200, M-203, M-204, M-207, M-208, M-212..M-220, M-225, M-240, M-241, M-251, M-252, M-269, M-270, M-271, M-277, M-311, M-391, M-397..M-399, M-411..M-419, M-452..M-454, M-471..M-475, M-577, M-578, M-770, M-771, M-809, M-810, M-812, M-861, M-862, M-864..M-867, M-870, M-898, M-899, M-915..M-918, M-926..M-954.

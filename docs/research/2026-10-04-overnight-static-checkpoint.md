# Overnight static analysis checkpoint

Historical checkpoint — 2026-10-04. Observations and evidence gates below retain
their batch scope. See the [current checkpoint](../implementation-status.md) for
later findings, pending work and pause status.

The targeted approach exported 25 distinct selected routines across four bounded
Ghidra batches. One string routine was re-exported with its complete chained
unwind ranges. Earlier whole-program analysis exported no functions before its
timeout; these targeted results do not make that earlier attempt successful.

Reference: dedicated tool build 25621760; executable SHA-256
`67cfc915d9d085b758057e2960d4a2018fb121123af1329c25ef7aa58b960104`.
Scope and exact commands are in the private `icarus-overnight` case, authorized by
[the overnight batch](../overnight-work-plan.md). No original code or game assets
were copied into the native implementation.

The PE helper now follows chained x64 unwind records rather than treating each
entry as a complete function. It also locates candidate relative references.
Candidates still need instruction-boundary and control-flow review. The Ghidra
helper defines explicit bounded ranges and exports decompilation plus references;
it does not execute the original, download symbols or run whole-program analysis.
Decompiler completion is not a correctness or reachability verdict.

## Results implemented independently

- [Default version checksum](../specs/network-checksum.md): ASCII-only lowercase
  and reflected CRC over zero-extended UTF-16 units. Authored vectors agree with
  an independent arithmetic oracle. Active runtime inputs and callbacks remain
  unresolved, so this primitive is disconnected from admission.
- [Prospect envelope](../specs/prospect-envelope.md): reflected field names,
  encoded/compressed input and legacy hash observations support a bounded,
  read-only inspector. It does not deserialize the inner archive or import a world.

## Additional call-path observations

The caller at `0x14146e590` checks a string-shaped argument for missing/empty
content before forwarding to the by-name loader `0x14146e6c0`. This does not yet
prove how a dedicated-server console or remote request reaches it.

The caller at `0x1414acc40` transforms an options string, extracts values using
the identifiers `prospectId`, `LocalSavePath` and `saveCreatedProspect`, and forwards
the local-save-path-shaped result to `0x1414baf60`. It has different continuation
branches based on the reader result. The enclosing game-mode entry and map/state
requirements remain unresolved; these are not documented working command-line
options for administrators.

The signed-length string routine spans three chained ranges through
`0x141e16432`. Its loading branch rejects the minimum signed 32-bit value and can
enforce an archive-specific length bound. Negative lengths select two-byte units;
positive lengths enter a separate conversion helper. The routine also normalizes
termination and has special handling for `0xffff`. No generic UTF-8 assumption or
complete network string codec has been committed from these observations.

The encoded-blob caller does not visibly branch on the Base64 decoder's return
before using the declared compressed count. This is a bounds/error-handling lead,
not a reproduced defect. See SL-004 in the [bug ledger](../bug-ledger.md).

## Remaining evidence gates

Static observations come from one build. Original-runtime fixture comparisons,
active platform/version policy, packet framing, authenticated client connection,
complete state serialization and independent world-content mapping are still
required. No original crash, stability improvement, or ICARUS performance gain is
established by these exports or authored tests.

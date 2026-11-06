# Conditional visibility state

## Status

Experimental: public server 26313077 (F-1020, F-1023); native models
(F-1021, F-1024). Last reconciled: 2026-11-06.

## Scope

Conditional RPC82 framing and locally returning handler/cache paths.

## Confirmed

- Normal server 26313077/engine17/game0 little-endian RPC82 (F-1020): one
  LSB-first presence bit, then custom reuse, visible, PackageName, FileName iff
  reuse is clear (else copy PackageName). Absent skips custom and retains initialized storage
  of unknown contents. Selected nonshared sender always sets presence, even
  for zero-looking structs; shared masks remain unknown. Qualified reader/map
  names use packed EName index <703 or FString + raw u32 number; no alignment
  or second generic struct bit.
- F-1023: C is connection; D = [C+0x13c0], called only if non-null.
  Accepted add inserts into C+0x14b8,
  processes destruction/actors, then D virtual+0x2a8(name, package-world).
  That possibly-null operand is separate from [C+0x58]. Hide removes before
  D virtual+0x2b0, then matching full-name channels receive virtual+0x280(2).
  Rejected add neither inserts nor takes accepted callbacks; argument+0x10
  bit1 clear requests Close, set bypasses it. Serializer preserves this
  unresolved native bit; it is not wire reuse. Locally returning paths refresh
  existing non-null ULevel keys at C+0x1558, not all world levels.
- UpdateCachedLevelVisibility `0x143711670`: null => true;
  persistent AND `[[[C+0x58]+0x148]+0x18] == [C+0x15f8]` => true;
  else full package-name membership in C+0x14b8. Names include both FName
  components; cache keys use pointer identity. Persistent mismatch falls
  through. ClientHasInitializedLevelFor `0x1436f6a40` returns cached actor+0x20;
  misses classify it, map non-ULevel to null, then compute/cache (F-1023).
- Selected graph add broadcasts globally then to an existing full-name entry,
  even for null world; remove visits delegates. Candidate slots do not bind D.

## Candidate

F-1021 stages one typed value atomically after exact consumption; canonical
packed integers, strict strings and 65,536-unit cap are native subset rules. Explicit outer-layout
assertion preserves Absent/Present without inferred defaults. RPC78 validates
empty parameters only. [Private capture](../../runbooks/network-readiness.md#private-selected-rpc82-capture)
(F-1022) activates no decoder.

Detached `experimental::visibility_state` (F-1024): caller-qualified names,
object tokens and explicit helper/policy/profile facts; events are intentions.
Unknown required facts or exceeded name/cache/event/channel bounds reject,
preserving state/log.
Atomicity, limits and deterministic refresh order are authored. Destruction,
MarkActive, graph actor bits, delegates/compaction and channel effects are
deferred. No decoder/dispatch/effects wiring or inferred wire-name identity.

## Open questions

U-079/[live gates](../../gates.md#before-enabling-visibility-dispatch-or-effects)
remain open. Helpers, live receiver/delegates/channels, Close/reentrancy and
client progress are unqualified; null true/cache consumption proves no
readiness or world-entry cause. M-1207 is count only.

## Evidence

M-1208, M-1209, M-1215, M-1217, M-1219, M-1221, M-1223 to M-1236.
M-1240 is direct evidence; M-1234 is an authentic empty REA bundle.

# Conditional visibility state

## Status

Experimental: servers 26313077/26425113; client 25793767; native code
authored (F-1021, F-1024) or probe-only and gated (F-1097, F-1098).
Last reconciled: 2026-11-11.

## Scope

RPC82 framing, handler/cache paths, the answer to a report, call 62.

## Confirmed

- F-1020 conditional normal engine17/game0 little-endian RPC82: LSB-first presence,
  custom reuse, visible, PackageName, FileName iff !reuse (else PackageName).
  Absent retains unknown initialized storage. Nonshared sender sets presence;
  shared masks unknown. Reader/map names: packed EName
  <703 or FString/raw u32 number; no alignment/second struct bit.
- F-1023: C is connection; D = [C+0x13c0], called only if non-null.
  Accepted add inserts C+0x14b8, processes destruction/actors, then D
  virtual+0x2a8(name, package-world).
  Operand may be null, distinct from [C+0x58]. Hide removes before
  D virtual+0x2b0, then matching full-name channels receive virtual+0x280(2).
  Rejected add skips insertion/callbacks; argument+0x10
  bit1 clear requests Close, set bypasses it. Native bit is preserved,
  distinct from wire reuse. Local returns refresh
  existing non-null ULevel keys C+0x1558 only.
- UpdateCachedLevelVisibility `0x143711670`: null => true;
  persistent AND `[[[C+0x58]+0x148]+0x18] == [C+0x15f8]` => true;
  else full package-name membership in C+0x14b8. Names include both FName
  components; cache keys use pointer identity. Persistent mismatch falls
  through. ClientHasInitializedLevelFor `0x1436f6a40`: cached actor+0x20;
  misses classify it, map non-ULevel to null, then compute/cache (F-1023).
- Graph add broadcasts globally then to an existing full-name entry, even for
  null world; remove visits delegates (F-1023).
- Conditional receiver installation (F-1023): AddClientConnection
  `0x140913fe0` calls factory `0x140917540` only after null C virtual+0x280
  and no pending G+0x48 entry matching C at +0x28. K=[G+0x28] survives
  only if B=StaticClass `0x140934c70` is non-null, signed i=[B+0x38]
  <= [K+0x38] and `[[K+0x30]+8*i] == B+0x30`; else null, no fallback.
  Parameters constructor `0x142182040` stores K; `0x14219d500` allocates
  calls [K+0xb0] iff reuse out-byte is clear. Registration argument
  `0x140935580` remains unbound to K+0xb0; its non-null leaf reaches
  `0x14090dda0`, writing constructor vptr `0x14470c118`
  (+0x2a8/+0x2b0 = `0x140923800`/`0x1409239c0`). Final type: F-1091.
  `0x140917687` stores C to D+0x28; `0x14091768e` stores D to C+0x13c0
  before G virtual+0x358 at `0x14091769b`.
- Delegates (F-1023; members F-1091): `0x141729260` adds members
  `0x14172c2b0`/`0x14172c330` for D+0xf0/+0x158 without deduplication;
  ExecuteIfSafe calls one iff weak resolution `0x1421ac9f0` succeeds. They
  only log (`[0x146181850]>=4`, `[0x1461815d0]>0`), reading no receiver or
  world, mutating no visibility, readiness or actor list (helpers external);
  compaction `0x14089c9b0` removal/deduplication unproved (M-1240, M-1241).
- Gather `0x14171b6b0` consumes map `[N+0x20]+0x4b8`: skip records with
  signed count+0x10 <=0; full key equal to Q+0xf0 bypasses search, else
  require full-qword membership in supplied [Q+0xe8], then set Q+0xf0.
  Append record+8 references to [Q+0xe0]. Set, Q+0xf0 and map owner:
  F-1091, F-1092. Seven detached JS tests are proposal-only (M-1243).
- F-1078: `AGameModeBase::ReplicateStreamingStatus` sends one 62 over every
  server streaming level (count; per level FName as FString with number 0,
  int32 LOD, loaded/visible/block-on-load bits), then 23. Clients stream
  tiles themselves (F-1079) once their record is finalised (F-1080), then
  report each (F-1085, F-1089); payloads and bit1: F-1090.
- Answer to an add: the level's two world-settings actors (F-1093), named
  from the report, rules and public metadata (F-1095); routing F-1092.
- Native (F-1097, F-1098): probe-only dispatch and those opens; refused
  names counted, never closed on; hides close nothing.

## Candidate

F-1034: conditional matched-build replay; consumed bits and name lengths only.

F-1021: atomic exact-consumption staging; canonical packed ints, strict strings,
65,536-unit cap; explicit outer assertion preserves absence. RPC78: empty only.
[Capture](../../runbooks/network-readiness.md#private-selected-rpc82-capture)
(F-1022) activates no decoder.

F-1024 `experimental::visibility_state` and detached
`experimental::visibility_gather` (Gather as confirmed above, keeping duplicates;
clear keeps the cache): authored, explicit identities/bindings. Destruction,
MarkActive, graph bits, delegates/compaction and channel effects deferred; no
dispatch or inferred wire identities.

F-1094: the answer's lag is the original's own tile loading (inferred).
Loading screen: not cleared by finalisation alone, or another blocker
remains (F-1096, F-1089).

## Open questions

U-079/[gates](../../gates.md#before-enabling-visibility-dispatch-or-effects):
no world-entry cause proof; hide side unobserved; native Close/reentrancy
test-qualified only. Unread: the F-1096 event raiser, other blockers, the
curtain, a Blueprint override of F-1080; 3.0.35 client behaviour.

## Evidence

M-1208, M-1209, M-1211, M-1215, M-1217, M-1219, M-1221, M-1223 to M-1244, M-1246/M-1252.
M-1240/M-1241/M-1243 are direct; M-1234/M-1242/M-1244 are empty REA bundles.
M-1161, M-1342, M-1343 (F-1078 to F-1081); M-1345, M-1347 (F-1085); M-1349 to M-1352.

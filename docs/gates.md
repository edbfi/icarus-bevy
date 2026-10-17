# Gates

Preconditions that block an action until a finding clears them, grouped by the
action they block. This file is their only home; status, plan and digests link
here instead of restating them.

Before restructure: 96ddb0b:docs/gates.md

Last reconciled: 2026-10-18

## Before any C001 original-code case

- Authored decoder fixtures, exact-window decodes (305/296, 106 and 196/217 B),
  complete function `.pdata` bounds (103538, 95461 and 103518), CRT registration
  flow (`0x1406a8ab0` -> `0x142187f70` and `0x141a00220`), base constructor
  receiver preservation (`0x1418824d0` keeps `rbx`), vftable slot targets
  (`0x1449d9f90` / `0x1449da448`, `+0x4a0` `RestoreState` `0x1418a9bb0` ->
  `0x1418a9c80` -> `+0xa0` `Serialize` `0x1418aaf70` -> `+0x488` `UpgradeVersion`
  `0x14189d380`), and the `-noanalysis` cause of the M-568/M-569 listing
  rejection are qualified (F-469 to F-477, F-495, F-508, F-509, F-513, F-669,
  F-670, F-721, F-722; U-001 to U-006, U-023, U-041, U-042 resolved).
- Do not retry these completed reads or unchanged helpers; preserve the failed
  nine-mode receipt, strict replay and split outputs (M-307). Any new C001
  original-code case requires its own fresh scope, bindings and session.

## Before new save-class work

- Completed C028/C034 finite audits and historical C013 coverage determine the
  corrected census; do not repeat their completed populated value structures.
  The D01 populated Int/Str work is complete, and DA04's population is empty.
- Server 25621760 and client 25793767 static correspondences for `FString` /
  `FName` normalization (`ParseNumber` `_<1..10 digits>`), `FFloatProperty`,
  `FVector`, `FArrayProperty` 32-bit size diffs, immutable `CPF_SaveGame` flags,
  tagged property wrap-around, and C013/C028 migration and restore hooks are
  qualified (F-715 to F-720; U-026, U-029 to U-031, U-033, U-039, U-054, U-056,
  U-057, U-059 to U-062 resolved); do not repeat those static reads.
- Audit the selected coverage before choosing another save boundary; do not
  repeat completed header/string comparisons.
- Complete prior range/result replay and all both-copy budgets precede any new
  unit decoding or native call. Fresh helpers must bind the current source/library
  transition explicitly and replay frozen parent/selection results first.
- Before changing live hashes, preserve complete fixity receipts for affected
  prior cases; later helpers must record exact source/library transitions.
- Distinct native child widths require exact descriptor identity; field order or
  an equal-width multiset cannot replace that binding.
- Preserve unknown fields and private inputs; no partial importer. Exact
  class/property names and supplied values must stay private under `work/`.
- The original 8 MiB component transport cannot hold both complete C034 copies
  together, so those comparisons use separate per-copy batches.

## Before promoting control-store or negotiation findings

- Direct operand/p-code checks precede any owner or archive conclusion.
  Configured decompiler bodies alone do not prove bounded native flow.
- Any expanded output needs new source/classes/scope/session and byte
  qualification; old lookahead is not silently promoted to decoded evidence.
  Later decoding must honor measured spans without automatic expansion.
- Complete `UWorld::UWorld` (1,332 B, `.pdata` 255917..255921) and
  `UPendingNetGame::UPendingNetGame` (71 B, 245246) constructors,
  `UWorld::~UWorld` (`TARGET02`, 757 B, 255950), `UObject`/`FURL` receiver
  preservation, `/GR-` abutting vftables (`0x145467ef0`/`0x145468180`),
  `UNetDriver::InitBase` (`Driver + 0x208`), and static `ReceivedBunch` ->
  `NotifyControlMessage` (`CONTROL01` `0x1439ee7f0`, `.pdata` 256371) dispatch
  are qualified (F-520, F-544, F-723, F-724; U-007 to U-009, U-013, U-015 to
  U-018, U-043 resolved); executed runtime dispatch in an authenticated session
  remains open (U-010, U-038).
- Client and server archive/outer framing, `UControlChannel` byte-order
  persistence (`Connection + 0x158`), 19 fallback skip layouts, 32,767
  queued-bunch bounds, `IsClientMsgTypeValid` state gating,
  `PreLogin`/`InitNewPlayer` checks and 0 registered version/PreLogin callbacks
  are bound in `experimental` (F-688, F-710 to F-714; U-014, U-037, U-044
  resolved); connected sessions keep their own gate (U-038).
- Older server build 25621760 and client build 25793767 remain distinct;
  addresses and assumptions do not transfer automatically.

## Before deriving replication schema for the server

- The user authorized (2026-10-16) offline derivation of gameplay-tag mappings
  (U-065) and replicated-property schemas (U-066) from the local stock client,
  configuration and packaged data, including use of derived protocol metadata
  in the independent server. This clears the decision gate, not either
  technical unknown or a compatibility claim.
- Original assets, gameplay data tables, code and binaries remain research
  inputs only and must not ship with the replacement. Each analysis still
  needs a bounded case scope and session; runtime and connected experiments
  retain their separate gates below.

## Before any original-server or client run

- Each runtime experiment requires its own case scope and current launcher
  session; historical authorization windows remain expired (F-218, F-252). The
  user authorized (2026-10-15) offline original-server save-load runs with the
  supplied save and local server binaries, and (2026-10-16) fetching and
  running the matched official dedicated server on this host with connected
  stock-client testing per approved scenario (S1 completed, F-701; later
  scenarios require explicit scenario approval).
- Connected stock-client experiments against the native server and reference
  captures remain deferred by the user (F-421; U-038, U-052).

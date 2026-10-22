# Gates

Preconditions that block an action until a finding clears them, grouped by the
action they block. This file is their only home; status, plan and digests link
here instead of restating them.

Before restructure: 96ddb0b:docs/gates.md

Last reconciled: 2026-10-22

## Before any C001 original-code case

- Authored decoder fixtures, exact-window decodes (305/296, 106 and 196/217 B),
  complete `.pdata` bounds (103538, 95461 and 103518), CRT registration
  (`0x1406a8ab0` -> `0x142187f70` and `0x141a00220`), base constructor receiver
  preservation (`0x1418824d0` keeps `rbx`), vftable targets (`0x1449d9f90` /
  `0x1449da448`, `+0x4a0` `0x1418a9bb0` -> `0x1418a9c80` -> `+0xa0` `0x1418aaf70` ->
  `+0x488` `0x14189d380`), and the `-noanalysis` cause of the M-568/M-569
  rejection are qualified (F-474, F-476, F-477, F-905 to F-910, F-722; U-001 to
  U-006, U-023, U-041, U-042 resolved).
- Do not retry these completed reads or unchanged helpers; preserve the failed
  nine-mode receipt, strict replay and split outputs (M-307). Any new C001
  original-code case requires its own fresh scope, bindings and session.

## Before new save-class work

- Completed C028/C034 finite audits and C013 coverage determine the census; do
  not repeat their completed populated value structures or D01/DA04 checks.
- Server 25621760 and client 25793767 static correspondences for `FString`/`FName`
  normalization, `FFloatProperty`, `FVector`, `FArrayProperty`, immutable
  `CPF_SaveGame`, tagged wrap-around, C013/C028 migration/restore, `C030`
  `TextProperty`, `C015`/`C022`/`C032` Map/Set/Int64/Struct items, all 65
  `RecorderComponent` `FClassParams`, all 38 `StateRecorderBlobs` (`C001..C038`)
  record/restore/stitch/match/validate methods, `D01` inventory/variable
  restore (`14` `EDynamicItemProperties`), and entry-path failure matrices are
  qualified (F-715 to F-720, F-725 to F-728, F-731 to F-738, F-741 to F-744,
  F-893, F-899, F-902, F-904;
  U-020 to U-024, U-026, U-028 to U-033, U-039, U-046, U-048, U-050, U-053 to
  U-063 resolved); do not repeat those static reads.
- Audit selected coverage before choosing another save boundary; do not repeat
  completed header/string comparisons.
- Complete prior range/result replay and both-copy budgets precede any new unit
  decoding or native call. Fresh helpers must bind the current source/library
  transition explicitly and replay frozen parent/selection results first.
- Preserve complete fixity receipts before changing live hashes.
- Distinct native child widths require exact descriptor identity; field order or
  an equal-width multiset cannot replace that binding.
- Preserve unknown fields and private inputs; no partial importer. Exact
  class/property names and supplied values must stay private under `work/`.
- Separate per-copy batches are required for complete C034 comparisons (8 MiB cap).

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
  `UNetDriver::InitBase` (`Driver + 0x208`), static `ReceivedBunch` ->
  `NotifyControlMessage` (`CONTROL01` `0x1439ee7f0`, `.pdata` 256371) dispatch,
  and S1 runtime exercise are qualified (F-520, F-544, F-701, F-723, F-724,
  F-729; U-007 to U-010, U-013, U-015 to U-018, U-043 resolved); native
  connected sessions remain gated (U-038).
- Client and server archive/outer framing, `UControlChannel` byte-order
  persistence (`Connection + 0x158`), 20 fallback layouts (`0..18`, `21`, `22`),
  32,767 queued-bunch bounds, `IsClientMsgTypeValid` state gating,
  `PreLogin`/`InitNewPlayer` checks and 0 registered version/PreLogin callbacks
  are bound in `experimental` (F-688, F-710 to F-714, F-729, F-740; U-014, U-037,
  U-044, U-051 resolved); connected sessions keep their own gate (U-038, U-052).
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
- The user decided (2026-10-22, F-828) that the shipped server must not
  depend on the game client, which is not publicly available, or on data
  derived only from it. Basic data it needs to work may come from the free,
  public dedicated-server distribution, fetched at install time without an
  interactive confirmation (container installs); the native Rust fetcher
  `icarus-fetch` now does this anonymously without steamcmd (F-831, U-073).
  The requirement must be documented for operators. Nothing original is committed or
  redistributed. Client-derived metadata counts toward the shipped server only
  once re-derived from or checked against that distribution; for 3.0.32
  every table `run` uses is (F-832; U-072 resolved).

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

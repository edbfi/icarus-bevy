# Current-client control stores and initial control archive

Current-client (build 25793767) and older-server (build 25621760) static
evidence for the UWorld and UPendingNetGame local table stores, their complete
constructor and destructor bounds, public symbols and notify slots, the
control-dispatcher caller and vftable layout scans, and the experimental
initial control-body reader; the normative control wire rules live in
[control negotiation](../../specs/network/control-negotiation.md). Promotion of
these results is gated in
[gates](../../gates.md#before-promoting-control-store-or-negotiation-findings).

Before restructure: 96ddb0b:docs/research/network/2026-10-13-control-stores.md

## Question cluster

- Which table constants do World and Pending store after their constructor calls, and at which displacements?
- What do the constructor metadata intervals and fixed instruction windows bound?
- Which public symbols and notify slots connect the tables to NotifyControlMessage?
- Which archive path and callers feed the control dispatcher?

## Current answer

In both builds the UWorld and UPendingNetGame constructors store their primary
and FNetworkNotify table constants at +0 and +0x28 of the object under
construction, held in `rdi`/`rbx` across `UObject::UObject` (`0x142169690`) and
`FURL::FURL` (`0x1439ae930`) (F-478, F-504, F-544, F-723; U-007, U-009, U-013
resolved). Client PDB names identify the older server's registration getters
(`UWorld::GetPrivateStaticClass` `0x143b86aa0`, size `0x798`;
`UPendingNetGame::GetPrivateStaticClass` `0x143b21d20`, size `0xc0`) and
`InternalConstructor` callbacks (`0x143b856c0` and `0x143b21830`), which
tail-jump with `rcx = *(UObject**)rcx`. In server 25621760, `UWorld::UWorld` is
1,332 B across 5 chained `.pdata` records (`255917..255921`,
`0x1439da2e0..0x1439da814`, with `TARGET01` at `0x1439da2e8` in record `255918`),
`UPendingNetGame::UPendingNetGame` is 71 B in `.pdata` `245246`
(`0x1437a9640..0x1437a9687`), and `TARGET02` (`0x1439dae50..0x1439db145`, 757 B,
`.pdata` `255950`) is complete `UWorld::~UWorld` (client `0x143ca6730`), resetting
`[rcx]` and `[rcx+0x28]` and tail-jumping to `UObject::~UObject` (F-501 to F-505,
F-723, F-724; U-008, U-017, U-018 resolved). Because `/GR-` omits RTTI Complete
Object Locator pointers at `[vtable-8]`, `UWorld`'s 82-slot primary vftable
(`0x145467ef0`) and 4-slot `FNetworkNotify` secondary vftable (`0x145468180`,
with slot `+0x18 = 0x1439ee7f0` before UTF-16 literals) abut directly, and
`0x14671d164` is `.pdata` `256371` for `CONTROL01` (`0x1439ee7f0`,
`UWorld::NotifyControlMessage`), exercised across Hello, Challenge, Login,
Welcome and Join in connected run S1 (F-480, F-506, F-546, F-701, F-724, F-729;
U-010, U-015, U-016, U-019 resolved). `UWorld::Listen` (`0x1439ecd8b`) and
`UPendingNetGame::InitNetDriver` (`0x1437b7f6f`) pass `this + 0x28`
(`FNetworkNotify*`) to `UNetDriver::InitBase` (`Driver + 0x208`), and
`NotifyControlMessage` adjusts `this` by `r13 - 0x28` (F-520, F-723, F-724;
U-043 resolved).

`UControlChannel::ReceivedBunch` validates and persists the Hello byte-swap flag
at `Connection + 0x158`, dispatches channel-local ID `22` (`NMT_DestructionInfo`),
reaches `NotifyControlMessage` through `Driver + 0x208` slot `+0x18` (including
active branches `0`, `4`, `5`, `9`, `10`, `15`, `17`), and falls back across the
20-entry skip table (`0..18`, `21`, `22`, plus special handlers `16`, `19`, `20`)
inside `ReceivedPacket`'s packet-word, ack-history, and bunch-header framing
(F-284, F-514 to F-539, F-579, F-712 to F-714, F-740, F-857; U-014, U-044
resolved; [control negotiation](../../specs/network/control-negotiation.md)).

## Open

- Stock-client authenticated session against the native server (U-038).

## Artifacts

M-005, M-012, M-023..M-046, M-281..M-283, M-288..M-297, M-301, M-302, M-315..M-346, M-353..M-357, M-370, M-371, M-749..M-754, M-763..M-765, M-770, M-771, M-776, M-777, M-780, M-791.

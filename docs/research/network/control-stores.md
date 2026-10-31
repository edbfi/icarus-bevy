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
construction, held across `UObject::UObject` and `FURL::FURL` (F-478, F-504,
F-544, F-723; U-007, U-009, U-013 resolved). Client PDB names identify the
older server's registration getters and constructor callbacks; the complete
`UWorld::UWorld`, `UPendingNetGame::UPendingNetGame` and `UWorld::~UWorld`
bounds are F-501 to F-505, F-723 and F-724 (U-008, U-017, U-018 resolved;
client `UWorld::~UWorld` `0x143ca6730`).
Because `/GR-` omits RTTI locators, `UWorld`'s primary and `FNetworkNotify`
vftables abut directly, and the notify slot is `UWorld::NotifyControlMessage`,
exercised across Hello, Challenge, Login, Welcome and Join in connected run S1
(F-480, F-506, F-546, F-701, F-724, F-729; U-010, U-015, U-016, U-019
resolved). `UWorld::Listen` (`0x1439ecd8b`) and `UPendingNetGame::InitNetDriver`
(`0x1437b7f6f`) pass the
`FNetworkNotify` subobject to `UNetDriver::InitBase` (F-520, F-723, F-724;
U-043 resolved).

`UControlChannel::ReceivedBunch` validates and persists the Hello byte-swap
flag, dispatches `NMT_DestructionInfo` locally, reaches
`NotifyControlMessage` through the driver's notify slot, and falls back across
a 20-entry skip table inside `ReceivedPacket`'s framing (F-284, F-514 to F-539,
F-579, F-712 to F-714, F-740, F-857; U-014, U-044 resolved;
[control negotiation](../../specs/network/control-negotiation.md)).

## Open

- Stock-client authenticated session against the native server (U-038).

## Artifacts

M-005, M-012, M-023..M-046, M-281..M-283, M-288..M-297, M-301, M-302, M-315..M-346, M-353..M-357, M-370, M-371, M-749..M-754, M-763..M-765, M-770, M-771, M-776, M-777, M-780, M-791.

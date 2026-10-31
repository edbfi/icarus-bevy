# Current-client control stores and initial control archive

Static dispatch: client 25793767 and server 25621760; S2: 3.0.34. Wire rules:
[control negotiation](../../specs/network/control-negotiation.md); promotion:
[gates](../../gates.md#before-promoting-control-store-or-negotiation-findings).

Before restructure: 96ddb0b:docs/research/network/2026-10-13-control-stores.md

## Question cluster

- Which tables do World and Pending store, where, and within what bounds?
- Which notify slots and archive callers reach control dispatch?

## Current answer

UWorld and UPendingNetGame store primary and FNetworkNotify tables at +0 and
+0x28, preserving the receiver across base constructors (F-478, F-504,
F-544, F-723; U-007, U-009, U-013 resolved). Complete constructor/destructor
bounds: F-501 to F-505, F-723, F-724 (U-008, U-017, U-018 resolved;
client destructor `0x143ca6730`).
The tables abut without RTTI locators; their notify slot reaches
`UWorld::NotifyControlMessage`, exercised by S1 (F-480, F-506, F-546, F-701,
F-724, F-729; U-010, U-015, U-016, U-019 resolved). Client Listen (`0x1439ecd8b`) and InitNetDriver
(`0x1437b7f6f`) pass the notify subobject to InitBase (F-520, F-723, F-724; U-043 resolved).

ReceivedBunch preserves Hello byte order, dispatches destruction locally and
other messages through the notify slot, with 20 fallback layouts (F-284,
F-514 to F-539, F-579, F-712 to F-714, F-740, F-857; U-014, U-044 resolved).

S2's original 3.0.34 direct join and Steam auth success are captured (F-1006),
after selecting `IpNetDriver` (F-1005). Its pre-connection auth frames and
Login identity are F-1007; release metadata is F-1008. Native Steam auth and
control Join pass (F-1009); the authored prospect is selectable (F-1010),
but geometry readiness blocks world entry (U-079; U-038 remains open).

## Open

- Stock-client authenticated session against the native server (U-038).

## Artifacts

M-005, M-012, M-023..M-046, M-281..M-283, M-288..M-297, M-301, M-302, M-315..M-346, M-353..M-357, M-370, M-371, M-749..M-754, M-763..M-765, M-770, M-771, M-776, M-777, M-780, M-791; M-1157..M-1175.

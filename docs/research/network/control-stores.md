# Current-client control stores and initial control archive

Current-client (build 25793767) static evidence for the UWorld and
UPendingNetGame local table stores, their constructor bounds, public symbols
and notify slots, the control-dispatcher caller scans, and the experimental
initial control-body reader. Promotion of these results is gated in
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
construction, held in a nonvolatile register across the super call (F-478,
F-504, F-544). Client PDB names identify the older server's registration
getters and InternalConstructor callbacks (UWorld at 0x143b856c0,
UPendingNetGame at 0x143b21830), whose constructors match the client's apart
from displacements (F-501 to F-503, F-505). Each FNetworkNotify table's +0x18
slot is NotifyControlMessage (F-480); the dispatcher's only pointer is that
slot of the older UWorld table, so it is UWorld::NotifyControlMessage (F-506),
and both builds' four-slot notify tables match by position (F-546). Constructor
symbols start 8- and 71-byte metadata fragments (F-479), and caller scans find
no direct call or archive provenance (F-481).

The dispatcher validates message IDs and reads the ID 0 body (F-284) from the
FInBunch through FBitReader::Serialize after the ID byte, byte order set by the
first field, in client and server alike (F-514 to F-516, F-518, F-519);
ReceivedBunch reaches it through the driver's notify object, slot +0x18
(F-520). ReceivedPacket parses a packet word and ack history, then bunch
headers inside a handler prefix and termination-bit frame (F-521 to F-524);
the client's parsers map the control vocabulary both ways (F-529, F-530), and
its reliability layer is seeded from the handshake cookie (F-535 to F-539). A
disconnected experimental reader composes the observed initial body within a
derived 131,081-byte ceiling; negotiation, identity and game-connection flags
stay false (F-096, F-097).

All results are static: no original executable, runtime instruction or live
control message ran, and metadata intervals are fragment bounds rather than
complete functions.

## Open

- Receiver origin, ABI and layout of the store base (U-007); cross-call provenance (U-009); class, registration and caller of the prefixes (U-013).
- Complete constructors (U-008).
- Executed runtime dispatch (U-010) and older-server ownership (U-011).
- Archive, bit-position and byte-order input path (U-014); indirect callers, RTTI, table identity and owner provenance (U-015 to U-018).
- Exact-window decoding through live REA and lazy Ghidra startup (U-019).

## Artifacts

M-005, M-012, M-023..M-046, M-281..M-283, M-288..M-297, M-301, M-302, M-315..M-346, M-353..M-357, M-370, M-371.

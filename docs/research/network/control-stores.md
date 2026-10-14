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

After their constructor calls, World and Pending each store their primary and
FNetworkNotify table constants at +0 and +0x28 of one unchanged base whose
receiver origin is unknown; the equal geometry does not identify the
older-server owner (F-478). The constructor symbols start 8- and 71-byte
metadata fragments, and fixed windows export only prologues and pre-call
prefixes (F-479). Each FNetworkNotify table's +0x18 slot equals its
NotifyControlMessage, and the 0x290 distance between tables is image placement,
not object layout (F-480).

Caller scans find one absolute pointer to the dispatcher and two LEA
materializations of slot 0x145468180 stored at base +0x28, but no direct call
and no archive provenance (F-481). The same build's UWorld and UPendingNetGame
registration getters are 171-byte exception intervals that load their class-name
literal plus two; the client's PDB-named GetPrivateStaticClass functions have the
identical shape, which identifies the older getters (F-501). Across builds the
getters differ only in displacement fields, and each class has three
class-specific code-pointer arguments (F-502). The client PDB names them, and the
shared call is GetPrivateStaticClassBody, so the older server's constructor
callbacks are InternalConstructor<UWorld> at 0x143b856c0 and
InternalConstructor<UPendingNetGame> at 0x143b21830 (F-503). Each callback
tail-jumps to its class constructor with the constructed object as this; in the
client those targets are the PDB-named constructors that contain the F-478
stores, so the store base is the object under construction, conditional on
register preservation across the super call (F-504). The older constructors
match the client's apart from displacements: T1 is the older UWorld constructor
body, so its stored constants are UWorld's primary and FNetworkNotify tables, and
the older UPendingNetGame constructor installs its own pair at the client's
positions (F-505). This answers the older-owner question for both classes. The
dispatcher's only pointer is slot +0x18 of the older UWorld FNetworkNotify table,
so the dispatcher is the older UWorld::NotifyControlMessage (F-506). The dispatcher validates message IDs and
reads the ID 0 body (F-284). The client's receive path reads that body from the
FInBunch through FBitReader::Serialize, after the ID byte, with byte order set
from the first field (F-514, F-515); the server's ID 0 reads take the same archive
route (F-516), and its ReceivedBunch, found through the UControlChannel vftable,
applies the same rule (F-518, F-519). In both builds ReceivedBunch reaches the
dispatcher through the driver's notify object, vtable slot +0x18 (F-520). The
client's ReceivedPacket parses a packet word and ack history, then bunch headers
whose payload bits are copied to the bunch from bit 0, inside a handler prefix
and termination-bit frame with fixed integer encodings (F-521 to F-524). The
client's parsers map the control vocabulary in both directions (F-529, F-530),
and its reliability layer is seeded from the handshake cookie (F-535 to F-539). A disconnected experimental reader composes the
observed initial body within a derived 131,081-byte ceiling; negotiation,
identity and game-connection flags stay false (F-096, F-097).

All results are static: no original executable, runtime instruction or live
control message ran, and metadata intervals are fragment bounds rather than
complete functions.

## Open

- Receiver origin, ABI and layout of the store base (U-007); cross-call provenance (U-009); class, registration and caller of the prefixes (U-013).
- Complete constructors (U-008) and the instruction past 0x143ca5a81 (U-012).
- Executed runtime dispatch (U-010) and older-server ownership (U-011).
- Archive, bit-position and byte-order input path (U-014); indirect callers, RTTI, table identity and owner provenance (U-015 to U-018).
- Exact-window decoding through live REA and lazy Ghidra startup (U-019).

## Artifacts

M-005, M-012, M-023..M-046, M-281..M-283, M-288..M-297, M-301, M-302, M-315..M-346, M-353..M-357.

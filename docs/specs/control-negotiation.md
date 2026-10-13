# Control negotiation: static observations

Status: **observed, incomplete, not wire-validated**. Applies only to dedicated
server tool build 25621760. This is an independently written behavioral note, not
an implementation copied from the original or a complete protocol specification.
Evidence and reproduction are in the
[targeted static report](../research/2026-10-04-targeted-static-report.md).

Validation statements below describe the cited historical runs. Current priorities
and the research pause are tracked in the [implementation checkpoint](../implementation-status.md).

## Layer boundary

The inspected function receives a connection, a message ID and an archive-like
input. It operates after lower layers have supplied those values. Consequently,
the ID table below is not a list of complete UDP packet types. Packet headers,
bit packing, sequence/acknowledgment rules, fragmentation, channel management,
compression, integrity protection and connection-state admission remain unknown.

There is a message-validity check before server-side table dispatch. Default table
entries are therefore not evidence that an arbitrary message is silently accepted.
The function also has a different branch for another operating mode; do not infer
the complete client-to-server or server-to-client vocabulary from this one table.

## Observed dispatch

Source: E-011/E-013 in `icarus-server-baseline`. The descriptions are inferred from
control flow and nearby diagnostic strings; they are not recovered symbols.
**Inferred** labels below describe those interpretations, not wire-validated meanings.

| Inner ID | Branch entry | Inferred meaning |
| --- | --- | --- |
| 0 | `0x1439eebec` | Initial version/token negotiation |
| 4 | `0x1439ef03b` | Client netspeed handling |
| 5 | `0x1439ef0e0` | Login handling |
| 9 | `0x1439ef7f1` | Join handling |
| 10 | `0x1439efb8a` | Split-player join handling |
| 15 | `0x1439f031c` | Connection swap handling |
| 17 | `0x1439f0416` | Debug text handling |

For IDs 1–3, 6–8, 11–14 and 16, this table targets the common exit. Out-of-range
IDs also take that exit after the preceding validity check. These are observations
of this function, not a policy to reproduce without state-machine evidence.

## Initial branch

The ID 0 branch reads the following logical values in order:

| Value | Observed operation | Unresolved detail |
| --- | --- | --- |
| First field | One byte into a local value | Meaning and enforcement |
| Remote version | Four bytes into a 32-bit value | Wire byte order under archive flags; version calculation |
| Token-like field | Shared string serializer into a temporary | Complete encoding rules and platform token semantics |

It obtains a local version through `0x141f2b220`, checks input error state after
reading the fields, and calls `0x141f2e6e0` with local and remote versions.
That comparison has a default equality result and an optional callback route.
Neither the local version value nor callback registration has been established.
Follow-up E-022/E-024 shows that generation itself has a cache, an optional
callback and a default checksum-construction path. Default construction combines
the project name, another string, NetCL, EngineNetVer and GameNetVer, then passes
the formatted result through transformation and checksum helpers. It also checks
a network-version override. The subsequent overnight pass recovered the
[default normalization and checksum primitive](network-checksum.md). Active runtime
inputs and callbacks still need validation; the displayed game release number is
not a substitute for this value.

The [cache and settings follow-up](network-checksum.md) identifies the concrete
project-version setter and invalidation path. A cache hit precedes the generator's
callback-enable check, and comparison has a separate delegate. Candidate initializer
references proved to register teardown routines; a broader static search still
does not establish whether a compatibility implementation is active at admission.

A failed comparison follows a branch with an incompatible-version diagnostic,
connection operations and close accounting. Exact response bytes remain unknown.
For a successful comparison, an empty-looking token takes a direct continuation;
a nonempty token selects callback handling or an encryption-failure path. This
does not prove that authentication or encryption is optional in a working ICARUS
session: higher layers, configuration and platform services remain unexamined.

## String-reader observations

E-021 identifies the reader used by the initial branch:

- It reads a signed 32-bit length. Negative and nonnegative lengths take different
  branches; the negative branch reads twice the absolute length in bytes.
- It explicitly rejects the minimum signed 32-bit value before negation.
- A positive archive maximum, when configured, limits the absolute element count.
- The wide branch includes conditional byte swapping and writes a trailing zero
  into the allocated result. At that stage, other normalization and conversion
  remained unverified.

The follow-up recovered the narrow character converter, the full chained reader
and its termination/`U+FFFF` normalization. An independent
[strict archive-string subset](archive-string.md) now has bounded authored tests.
It explicitly rejects several original normalization cases. Actual network archive
flags, field byte order and stock-client round trips still need validation; this
primitive does not make the control protocol available.

## Requirements before implementation promotion

1. Identify version generation and whether this build registers a compatibility
   callback. Record the legitimate decision rule; do not substitute an always-accept
   response to make a connection appear successful.
2. Recover the archive conversion and outer packet/channel framing, including
   bounds and error behavior. Identify platform authentication responsibilities.
3. Obtain a controlled trace of a matching stock client and a working reference
   server, with paired logs and exact build/configuration identifiers.
4. Create sanitized fixtures for accepted negotiation and each rejection class.
   Test truncation at each field boundary, zero/negative/minimum/oversized lengths,
   incompatible versions, invalid state transitions and token failures.
5. Show the native implementation passes those fixtures and the stock-client
   experiment before marking negotiation implemented and validated.

## Experimental initial archive-body fields

`experimental::control::read_initial_control_body(input, order)` now composes
the observed ID 0 field sequence for an explicitly caller-selected complete
archive body: one uninterpreted byte, an explicit-endian u32 and the existing
strict archive-string subset. The caller must establish message ID, byte order
and body boundaries. No datagram/message dispatch, byte-order inference, active
version comparison, token meaning or admission decision occurs.

The resource ceiling is 131,081 bytes, derived from the existing 65,536-string-unit
cap. The reader rejects oversize input before decoding, preserves borrowed exact
field/token spans and retains no decoded token units. Raw containers implement
neither `Debug` nor `Serialize`; the aggregate summary omits field/version/token
values and reports negotiation, identity and game-connection validation as false.
Errors contain no token data. Exact EOF and strict normalization rejection remain
explicit research policies, not recovered original dispatcher tolerances.

Eight authored tests cover both byte orders, all first-byte values, empty token
representations, version extrema, borrowed spans, raw surrogates, every truncated
prefix, trailing data, exact/over unit/body limits and summary/error privacy.
Existing archive/handshake tests, full manual checks, independent codec review,
release build and native save/restart smoke pass. Evidence and immutable
source/library transitions: `work/protocol-next-increment-20261013/`.
No original control message, live reference session or stock client was used.

The subsequent [borrowed string-frame API](archive-string.md#deliberate-strict-subset)
removes the temporary token-unit vector and signed-count reread. Successful body
validation now allocates no heap storage. Nine control tests, six new frame tests
and the three existing archive tests pass; full manual checks, independent review,
release build and native restart also pass. Public fields, errors, EOF, byte cap and
false compatibility/identity/admission flags are unchanged. Exact transitions:
`work/archive-frame-transition-20261013/`. No reference/client result is promoted.

A bounded [caller investigation](../research/2026-10-12-control-caller-limit.md)
encountered REA provider startup timeouts. A separately reviewed direct fallback
queried the retained database without analysis and found one non-call reference
to the dispatcher, with no recorded source function. Partial database coverage
does not prove caller absence; archive and outer-framing provenance remain open.
The subsequent authored PE scan finds one absolute pointer at `0x145468198` and
no matching E8/E9 rel32 patterns. Its table/owner/instruction role is unproved;
it prioritizes bounded context research, with no admission or archive conclusion.

Later bounded context and PE scans identify two exact address-materialization
sites for `0x145468180`; retained-project disassembly confirms seven-byte LEA
instructions at both. Raw operand/p-code joins then prove local eight-byte stores
of that constant at base-register offsets `+0x28`. One separately qualified prefix
preserves its base register through two stores at offsets zero and `+0x28`; the
other prefix stops at a call before its store. A fresh post-call window closes that
local gap and joins two constant stores plus four zero stores on an unchanged
unknown post-call base. It does not connect the base across the call. Neither
sequence identifies an object class, table owner, entry parameter or indirect
caller. A forced metadata decompilation reports blocks outside its
configured interval, so its bounded-flow assumption is not qualified. Direct
operand/p-code evidence precedes further promotion; the caller report preserves
the source-isolation failure and subsequent successful retry separately.

Public Epic API documentation offers a testable interface hypothesis:
[FNetworkNotify](https://dev.epicgames.com/documentation/en-us/unreal-engine/API/Runtime/Engine/Engine/FNetworkNotify?application_version=4.27)
has four virtual callbacks, including control handling with a connection, byte
message ID and `FInBunch&`. Both
[UWorld](https://dev.epicgames.com/documentation/en-us/unreal-engine/API/Runtime/Engine/Engine/UWorld?application_version=4.27)
and [UPendingNetGame](https://dev.epicgames.com/documentation/en-us/unreal-engine/API/Runtime/Engine/Engine/UPendingNetGame?application_version=4.27)
inherit UObject and this interface; the
[driver's Notify field](https://dev.epicgames.com/documentation/en-us/unreal-engine/API/Runtime/Engine/Engine/UNetDriver?application_version=4.27)
allows other implementations too. This shape does not identify the binary owner
or establish base/vtable offsets or method order. The pages select 4.27 but give
no historical source revision and include modern-looking fields. Same-build type
registration and an actual indirect call/receiver join are still required.

The normal replacement run command still has no game socket listener or control
negotiation. This body-field reader is a disconnected library primitive. A
separate [loopback handshake lab](stateless-handshake.md) stops before this layer
and creates no game connections.

A subsequent strict paired current-client PDB/PE case identifies named UWorld and
UPendingNetGame FNetworkNotify secondary tables. Exactly four qwords per table
show that both `+0x18` slots equal their respective named NotifyControlMessage
public-symbol addresses. These build-25793767 static relationships do not identify
the older dispatcher owner, establish object offsets or prove executed dispatch.
The table targets remain undereferenced and no complete vtable extent is claimed.
[Exact inputs, bounded fallback and independent evidence](../research/2026-10-12-control-caller-limit.md#current-client-public-symbols-and-named-notify-slots)
preserve the next constructor/receiver/type gate. Image-address deltas cannot
stand in for object-layout offsets, and provenance cannot cross calls unchecked.

# Control negotiation: static observations

Status: **observed, incomplete, not wire-validated**. Applies only to dedicated
server tool build 25621760. This is an independently written behavioral note, not
an implementation copied from the original or a complete protocol specification.
Evidence and reproduction are in the
[targeted static report](../research/2026-10-04-targeted-static-report.md).

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

| Inner ID | Branch entry | Static interpretation |
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
  into the allocated result. Other normalization and conversion remain unverified.

Do not infer a complete UTF-8/UTF-16 codec from these facts. Character conversion,
embedded zeros, terminator requirements, invalid characters, archive flags and
error propagation all need independent tests. A replacement should impose an
explicit resource budget even if a reference archive leaves its maximum unset;
record any resulting compatibility difference.

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

The replacement currently has no socket listener or implementation of this protocol.

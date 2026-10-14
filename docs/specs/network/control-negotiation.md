# Control negotiation

## Status

Experimental: static observations, not wire-validated. Scope: dedicated-server
build 25621760 (M-168) unless a current-client (build 25793767) comparison is
named. Last reconciled: 2026-10-13.
Full text: d5b0ae6:docs/specs/network/control-negotiation.md.

## Scope

The server-side control-message dispatcher and its initial (ID 0) branch, the
string reader it uses, and the static evidence linking UWorld / UPendingNetGame
notify tables to it. Not covered: packet headers, bit packing, sequencing,
fragmentation, channels, compression, integrity protection, connection-state
admission, and the client-to-server or server-to-client vocabulary beyond this
one table. The native run command has no game listener.

## Confirmed

- The dispatcher receives a connection, a message ID and an archive-like input
  after lower layers; a message-validity check precedes table dispatch, and a
  separate branch serves another operating mode (F-284).
- Dispatch table, descriptions inferred from control flow and nearby strings
  (E-011/E-013 under M-170):

  | Inner ID | Branch entry | Inferred meaning |
  | --- | --- | --- |
  | 0 | `0x1439eebec` | Initial version/token negotiation |
  | 4 | `0x1439ef03b` | Client netspeed handling |
  | 5 | `0x1439ef0e0` | Login handling |
  | 9 | `0x1439ef7f1` | Join handling |
  | 10 | `0x1439efb8a` | Split-player join handling |
  | 15 | `0x1439f031c` | Connection swap handling |
  | 17 | `0x1439f0416` | Debug text handling |

  IDs 1-3, 6-8, 11-14 and 16, and out-of-range IDs, reach the common exit.
- ID 0 reads one byte into a local value, four bytes into a 32-bit remote
  version, then a token-like string through the shared string serializer; it
  obtains the local version through `0x141f2b220`, checks input error state, and
  calls `0x141f2e6e0` with both versions. That comparison defaults to equality
  with an optional callback route ([checksum spec](network-checksum.md)). A
  failed comparison reaches an incompatible-version diagnostic and close
  accounting; a successful one takes a direct continuation for an empty token
  and callback or encryption-failure paths for a nonempty token (F-284).
- The string reader (E-021) reads a signed 32-bit length; the negative branch
  reads twice the absolute length in bytes, `i32::MIN` is rejected before
  negation, a configured archive maximum limits the absolute count, and the
  wide branch byte-swaps conditionally and writes a trailing zero
  ([archive-string spec](../save-format/archive-string.md)).
- Current client: in each UWorld and UPendingNetGame FNetworkNotify secondary
  table the `+0x18` slot equals the respective NotifyControlMessage public
  symbol (F-088). World stores its primary table constant at `+0` and the
  FNetworkNotify constant at `+0x28` of one unknown base (F-063); Pending does
  the same after its constructor call (F-076, F-274). Two seven-byte LEA sites
  materialize `0x145468180` and two eight-byte stores place it at base `+0x28`
  (F-109 region); one prefix keeps its base through stores at `+0` and `+0x28`.
- One absolute pointer to the dispatcher exists at `0x145468198`; no E8/E9
  rel32 callers match; a 128-byte pointer window ends at the dispatcher (F-106).
  The retained database shows one non-call reference with no containing
  function; partial coverage does not prove caller absence.
- Older server: that pointer is slot `+0x18` of the UWorld FNetworkNotify table
  `0x145468180`, which the older UWorld constructor installs at `+0x28` (F-505).
  The dispatcher is therefore UWorld::NotifyControlMessage, reached through the
  FNetworkNotify virtual slot rather than a direct call (F-506). The older
  UPendingNetGame tables are `0x14540a038` (primary) and `0x14537cb18`
  (FNetworkNotify).

## Candidate

- `experimental::control::read_initial_control_body(input, order)` composes the
  ID 0 field sequence (one uninterpreted byte, explicit-endian u32, strict
  archive string) for a caller-bounded body under a 131,081-byte ceiling,
  preserving borrowed spans, retaining no token units, implementing neither
  `Debug` nor `Serialize` for raw containers, and reporting negotiation,
  identity and game-connection validation as false (F-096, F-097). The
  dispatcher's input is an FInBunch, a bit-oriented archive (F-507); whether the
  ID 0 fields are byte-aligned is unverified, so this byte framing stays
  experimental. Exact EOF
  and strict normalization rejection are research policies, not recovered
  tolerances.
- Epic's FNetworkNotify documentation (four virtual callbacks; UWorld and
  UPendingNetGame implement it; the driver's Notify field admits others) is a
  testable interface hypothesis for the slots other than `+0x18`, whose owner and
  role F-506 identifies.
- Image-address deltas are not object-layout offsets; provenance may not cross
  calls unchecked. A forced metadata decompilation reported blocks outside its
  interval, so its bounded-flow assumption is unqualified.

## Open questions

- Executed runtime dispatch of the qualified stores and slots (U-010); older
  server owner (U-011); archive/bit-position/byte-order path into the
  dispatcher (U-014, U-044); indirect callers (U-015); RTTI, table identity and
  owner (U-016, U-017, U-018); receiver and type-registration join (U-043);
  active admission policy (U-037); authenticated stock-client session (U-038).

## Evidence

- Static cases: M-170 (`icarus-server-baseline`, E-011/E-013/E-021/E-022/E-024),
  M-146, the caller investigation under
  [research/archive](../../research/archive/2026-10-12-control-caller-limit.md),
  and the control-store rows M-023 to M-045 summarized in the
  [control-store digest](../../research/network/control-stores.md).
- Requirements before promotion: identify version generation and callback
  registration and record the legitimate decision rule; recover archive
  conversion and outer framing with bounds and errors; obtain a paired trace of
  a matching stock client and reference server; create sanitized fixtures for
  accepted negotiation and each rejection class (truncation at each field,
  zero/negative/minimum/oversized lengths, incompatible versions, invalid
  transitions, token failures); pass those fixtures and the stock-client
  experiment before marking negotiation implemented.
- Authored tests (eight control tests plus archive and handshake suites):

```bash
cargo test --locked --test control --test archive --test handshake
```

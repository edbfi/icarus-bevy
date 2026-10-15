# Control negotiation

## Status

Experimental: static observations, not wire-validated. Scope: dedicated-server
build 25621760 (M-168) unless a current-client (build 25793767) comparison is
named. Last reconciled: 2026-10-15.
Full text: d5b0ae6:docs/specs/network/control-negotiation.md.

## Scope

The server-side control-message dispatcher and its initial (ID 0) branch, the
string reader it uses, and the static evidence linking UWorld / UPendingNetGame
notify tables to it, and the current client's packet and bunch header parse
order with its outer frame and integer encodings. Not covered: other packet
handler components, reassembly of partial bunches, compression, integrity
protection, connection-state admission, the packet-header sender, runtime
version overrides, and the vocabulary beyond this table. The native run command has no
game listener.

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
- Current client receive path (F-514, F-515): FInBunch inherits
  FBitReader::Serialize at vftable `+0x150`, a leaf that tail-calls SerializeBits
  (`+0x158`) with the length times eight. UControlChannel::ReceivedBunch, while
  its inspection flag (`+0x68`) is set, hands the connection to `0x143984190`
  unless the bunch holds at least 16 bits and byte 0 is 0; byte 1 equal to 1
  clears archive byte-swap bit `0x20` at `+0x29`, other values set it. It then
  reads one ID byte per message until the bunch ends.
- Current client packet and bunch headers (F-521, F-522), read LSB-first from
  the packet reader when its internal-ack flag is clear; the integer readers
  are FBitReader vtable `+0x160` (value with a maximum) and `+0x168` (packed):

  | Order | Field | Encoding |
  | --- | --- | --- |
  | 1 | Packet word | 32 bits: history words minus one (bits 0-3, at most 8 words), acked sequence (4-17), sequence (18-31) |
  | 2 | Ack history | that many 32-bit words |
  | 3 | Packet info | above reader version 13: one bit; if set, a value below 1024, a frame-time bit and, to a client, one frame-time byte (F-525) |
  | 4 | Control | one bit; if set, open and close bits |
  | 5 | Close reason | if closing: value below 15 (below version 7: one bit) |
  | 6 | Paused, reliable | one bit each |
  | 7 | Channel index | packed, below the connection's channel limit (version above 2) |
  | 8 | Exports, mapped GUIDs, partial | one bit each |
  | 9 | Reliable sequence | if reliable: value below 1024, relative to the channel's last |
  | 10 | Partial initial, final | if partial: one bit each |
  | 11 | Channel name | if open or reliable: one bit; if set a packed hard-coded index (at most 702, 0 none), else an archive string and a 32-bit number (F-525) |
  | 12 | Payload | a bit count below eight times the maximum packet size, then those bits copied to the bunch from bit 0 |

  Bunches repeat until the packet reader ends. Before this, the stateless
  handler consumes optional magic-header bits and one handshake bit (0 for
  data), and the packet's bit length ends below the highest set bit of its
  nonzero last byte, a termination bit (F-524). A value with maximum M is
  LSB-first bits with masks 1, 2, 4, ... while value plus mask is below M; a
  packed value is up to five bytes, each bit 0 a continuation flag and bits 1-7
  the next 7-bit group, low group first (F-523). The client's SendRawBunch
  writes the same bunch header order; its reliable sequence and payload count
  use the same bit loop without clamping the value (F-526). The client's engine
  network protocol version starts at 17 in its image, so every version gate
  above takes its current path (F-527).
- Current client reliability (F-535 to F-537): the sender packs its outgoing
  sequence (bits 18-31), the last acknowledged incoming sequence (4-17) and the
  history word count minus one (0-3), then that many 32-bit history words
  covering the unacknowledged incoming span (1 to 8 words). After the
  handshake ack the client seeds its incoming and outgoing sequences from the
  cookie's first two little-endian 16-bit words masked to 14 bits; initial
  reliable sequences are the low 10 bits. Reliable bunches out of order are
  queued per channel (at most 255) and delivered in sequence. A received
  acknowledged sequence 1 to 8191 ahead resolves each newly acknowledged
  outgoing packet in order: beyond the newest 256 lost, otherwise history bit
  (acknowledged minus packet) set means delivered, bit 0 being the
  acknowledged packet; lost packets resend their unacknowledged reliable
  bunches (F-538, F-539).
- Current client control vocabulary (F-529, F-530); strings are archive
  strings, `u32` values follow the swap rule above:

  | ID | Direction | Fields |
  | --- | --- | --- |
  | 0 Hello | to server | byte, u32 version, string token |
  | 1 Welcome | to client | three strings: map URL, game name, redirect URL |
  | 2 Upgrade | to client | u32 local version (sent on incompatible Hello, then close) |
  | 3 Challenge | to client | string (a counter-derived token, F-531) |
  | 4 Netspeed | to server | u32 (client sends it after Welcome) |
  | 5 Login | to server | string response (literal "0", F-548), string URL, unique-net-id (F-531), string platform |
  | 6 Failure | to client | string, then the client closes |
  | 9 Join | to server | none |
  | 10 Split join | to server | string URL, unique-net-id |
  | 15 Swap | to server | u32 |
  | 17 Debug | to server | string |
  | 18 NetGUID | to client | packed int, string |
  | 21 Encryption ack | to client | none |

- Older server ID 0 reads use the archive's inline buffer or `+0x150`; the
  u32 takes `0x141f60c00` when `+0x29` bit `0x20` is set (F-516). Exact and
  masked searches miss the server ReceivedBunch (F-516); a getter,
  InternalConstructor and vftable chain finds it at UControlChannel vftable
  `0x1454cb368` `+0x290` = `0x143536420`, applying the same first-bunch rule
  with added verbose logging (F-518).
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
  dispatcher's input is an FInBunch, a bit-oriented archive (F-507). Within the
  bunch the fields follow the ID byte, and the first field (1 = sender little
  endian) selects the remote version's byte order in both builds (F-518,
  F-519); packet-to-bunch
  framing is unbound, so this framing stays experimental.
  `read_initial_control_body_with_sender_order` applies that inferred rule (1
  selects little-endian, any other value big-endian) before the same strict
  reader. Exact EOF
  and strict normalization rejection are research policies, not recovered
  tolerances.
- `experimental::packet` is an independently authored codec for the receive
  format above (F-521 to F-527): LSB-first bit reader and writer with sticky
  overflow, capped, wrapped and packed integers, termination-bit framing, the
  data prefix, packet header and info, bunch headers with hard-coded and string
  names, and payload copies from bunch bit 0. Callers supply the channel limit,
  maximum packet size and string cap; it keeps no connection state and
  reconstructs no reliable sequence. Not wire-validated.
  `experimental::control_receive` composes it with the strict initial-body
  reader: one terminated data datagram, read as the server reads client
  packets, to its control-channel messages (control identified by name index
  255 or channel index 0, both inferred); only ID 0 is decoded, and summaries
  carry no token, first field or version. `experimental::control_messages`
  types the whole vocabulary table with readers and writers, including the
  unique-net-id flags, type and encoded forms (the non-encoded string form is
  inferred); summaries carry kinds and lengths only.
  `experimental::sequencing` models the client's sequencing state (F-535 to
  F-539): cookie-seeded notify state, history shifting, header word counts, the
  sent-record ring, ack/nak notifications with full packet IDs, and per-channel
  reliable send, ack release, nak resend selection and ordered receive. The
  sent-record push site, ring capacity, zero-filled unreceived history words and
  the reliable comparison window are marked inferred or assumed.
  `experimental::packet_lab` composes codec, sequencing and the control
  vocabulary into one disconnected per-peer state after the handshake: the
  cookie seeds mirrored for the server (inferred), gated receive, ordered
  reliable control delivery and ack-only replies. It answers no control
  message and validates no negotiation, identity or game connection.
  `experimental::peer_lab` runs that state in the Bevy loopback lab: a peer is
  created only when its handshake acknowledgement is sent, in a bounded table
  with idle expiry, and each update coalesces one ack per peer (F-558). As an
  inferred mirror, it answers a Hello with Upgrade on a configured version
  mismatch, otherwise Challenge (lab random string) for an empty token, and a
  Login with Failure; it never sends Welcome. Replies are reliable control
  bunches resent after a nak (F-564).
- Server `UWorld::NotifyControlMessage` (`0x1439ee7f0`, F-563): Hello sends
  Upgrade with the local version and closes on a version mismatch; only an
  empty token (count with terminator under two) reaches the challenge sender;
  a nonempty token fails and closes unless an encryption delegate is
  registered. Netspeed is clamped to at least 1,800. Login stores the response,
  URL, unique id and platform, validates the URL, calls the game mode's
  PreLogin and welcomes the player on an empty error, otherwise sends the
  error and closes.
- Current client PreLogin (F-621): the ICARUS game mode and session add no
  checks of their own. The engine rejects a valid unique net id the online
  interface finds incompatible, then takes the game session's ApproveLogin
  error: a full server (player cap from a global override or MaxPlayers,
  spectators counted separately, never in standalone) or a per-connection
  splitscreen excess, each from integer URL options; registered PreLogin
  delegates run last and are unread.
- Server `UWorld::WelcomePlayer` (`0x1439fdeb0`, F-580): Welcome is type 1 with
  three strings (level or travel name, game-mode class name, a game-mode
  supplied string), followed by a flush, a queued-bits reset and login state 2.
- Current client unique-net-id type hashes (F-543): 1 NULL, 2 MCP, 3 STEAM,
  4 PS4, 6 GOOGLE, 7 GOOGLEPLAY, 8 FACEBOOK, 9 IOS, 10 APPLE, 11 TENCENT,
  12 SWITCH, 13 AMAZON, 14 GAMECIRCLE, 15 THUNDERHEAD, 18 Oculus, 19 Quail,
  20 PS4SERVER; 5, 16, 17 and 21 are runtime FName globals; 31 carries a name.
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
- Authored tests (14 control tests, including the inferred sender-order entry
  point (F-519), 14 packet codec, 9 control-receive and 7 control-message
  tests, plus archive and handshake suites):

```bash
cargo test --locked --test control --test packet --test control_receive --test control_messages --test sequencing --test packet_lab --test peer_lab --test archive --test handshake
```

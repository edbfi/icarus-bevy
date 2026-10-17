# Control negotiation

## Status

Experimental: static observations; the framing and Hello exchange are
wire-validated offline, and a synthetic Login and Join are welcomed (F-688
to F-690, F-712, F-713). Scope: dedicated-server
build 25621760 (M-168) unless a current-client (build 25793767) comparison is
named. Last reconciled: 2026-10-17.
Full text: d5b0ae6:docs/specs/network/control-negotiation.md.

## Scope

The server-side control-message dispatcher and its initial (ID 0) branch, the
string reader it uses, the static evidence linking UWorld / UPendingNetGame
notify tables to it, and the current client's packet and bunch header parse
order, `UControlChannel` byte-order persistence, queued send bounds, fallback
skip table, connection-state message admission (`IsClientMsgTypeValid`),
`PreLogin` checks, `SpawnPlayActor` / `InitNewPlayer` join flow, and delegate
cross-reference scans across both binaries. Not covered: Steam packet handler
runtime initialization, compression, or integrity protection. The native run
command has no game listener.

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
- Current client receive and send path (F-514, F-515, F-712; U-044 resolved):
  `FInBunch` inherits `FBitReader::Serialize` at vftable `+0x150`, a leaf that
  tail-calls `SerializeBits` (`+0x158`) with `Length * 8`.
  `UControlChannel::ReceivedBunch` (`0x1437c4940`), while `bNeedsHeaderInspection`
  (`+0x68`) is set, hands the connection to `Close` (`0x143984190`) unless the
  bunch holds at least 16 bits and byte 0 is 0 (`Hello`); byte 1 equal to 1
  clears archive byte-swap bit `0x20` at `+0x29` (other values set it), persists
  `Connection->bNeedsByteSwapping` at `Connection + 0x158` (`Data[1] != 1`), and
  clears `+0x68`. All subsequent `FInBunch` (`0x1437a8720`) and
  `FControlChannelOutBunch` (`0x1437a83c0` -> `FOutBunch` `0x1437a8b60`)
  instances initialize `+0x29` bit `0x20` from `Connection + 0x158`. It then
  loops while `!Bunch.AtEnd() && Connection->State != USOCK_Closed`, reading one
  ID byte per message: ID `0x16` (`22`, `NetGUIDAck`) dispatches locally on the
  connection (`PackageMap->ReceivedNak`), other IDs call
  `Driver->Notify->NotifyControlMessage(Connection, MessageType, Bunch)`, and if
  `Pos == PosBefore` after that call, `ReceivedBunch` skips the unhandled
  message across 19 registered layouts (`0..18`, `21`, failing closed on other
  unhandled IDs; `read_control_fallback` / `write_control_fallback` in
  `experimental::control_messages`). `UControlChannel::SendBunch` (`0x1437ca030`)
  sends immediately when `QueuedMessages` (`+0x70`) is empty and
  `NumOutRec < 255 + bClose`, else queues up to `32,767` (`0x7fff`) messages and
  drains them in `UControlChannel::Tick` (`0x1437cf4e0`) once `bOpenAcked`
  (`+0x30 & 1`) is set.
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

  Bunches repeat until the packet reader ends. Before this, the handlers
  consume optional magic-header bits, the stateless-connect bit and the
  Steam-auth bit (both 0 for data); the packet ends with the connection's
  termination bit, then the handler's in the highest set bit of the nonzero
  last byte (F-524, F-688). A value with maximum M is
  LSB-first bits with masks 1, 2, 4, ... while value plus mask is below M; a
  packed value is up to five bytes, each bit 0 a continuation flag and bits 1-7
  the next 7-bit group, low group first (F-523). The client's SendRawBunch
  writes the same bunch header order; its reliable sequence and payload count
  use the same bit loop without clamping the value (F-526). The client's engine
  network protocol version starts at 17 in its image, so every version gate
  above takes its current path (F-527).
- Current client reliability (F-535, F-536, F-710): the sender packs its outgoing
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

- Experimental modules, wire-validated through Hello and offline Login/Join
  (F-689, F-690, F-712, F-713): `control` reads the ID 0 body under a 131,081-byte ceiling
  with the first-field byte-order rule (F-096, F-097, F-518, F-519),
  retaining no token units and omitting `Debug`/`Serialize` on raw containers;
  `packet` codes the receive format above (F-521 to F-527); `control_receive`
  decodes one terminated datagram's control messages up to ID 0;
  `control_messages` types the whole vocabulary, fallback skip table and
  unique-net-id forms (F-531, F-636, F-712); `sequencing` models notify,
  reliable and partial-bunch state (F-535 to F-539, F-710, F-711); and
  `packet_lab` and `peer_lab` run them as disconnected per-peer state in the
  Bevy loopback lab, persisting the Hello byte order across subsequent control
  bunches and answering Hello and Login (`ADMISSION_DISABLED_MESSAGE` by default,
  or full `IsClientMsgTypeValid` login-state gating, `PreLogin`, `Welcome` and
  `Join` / `SplitJoin` when `PeerLabConfig::admission` is configured; F-558,
  F-564, F-712, F-713). Exact EOF and normalization rejection are research
  policy. Summaries carry kinds and lengths only.
- Server `UWorld::NotifyControlMessage` (`0x1439ee7f0`, F-563, F-713) and
  `UWorld::WelcomePlayer` (`0x1439fdeb0`, F-580): `UNetConnection::IsClientMsgTypeValid`
  (`0x143991c70`) gates incoming control messages by `ClientLoginState`
  (`+0x1c8`, `LoggingIn = 1`, `Welcomed = 2`, `Joined = 3`) and
  `ExpectedClientLoginMsgType` (`+0x1cc`, `0` = `Hello` initially, set to
  `5` = `Login` by `SendChallengeControlMessage` `0x143998730` and advanced to
  state `2` by `WelcomePlayer`, which rejects `Hello` and `Login`). Hello sends
  Upgrade with the local version and closes on mismatch; an empty token
  (count < 2) sends Challenge, and a nonempty token fails unless an encryption
  delegate is registered. Netspeed clamps to >= 1,800. Login validates the URL,
  runs `PreLogin`, and welcomes the player on an empty error or sends Failure
  and closes. On `Join` (`9`), if `PlayerController` (`+0x30`) is null,
  `UWorld::SpawnPlayActor` (`0x14392dbb0`) calls `AGameModeBase::Login`
  (`0x143866c80`) -> `ApproveLogin` -> `SpawnPlayerController` (`0x143872120`)
  -> `InitNewPlayer` (`0x1438633f0`), which checks `GameSession`, `PlayerState`
  and `FindPlayerStart`, parses `?bIsFromInvite`, `?SpectatorOnly=1` and
  `?Name=` (truncated to `20` units or formatted as `%s%i` from
  `DefaultPlayerName` and `PlayerId`), sets `NetPlayerIndex` (`+0x3d4`), binds
  the player via `SetPlayer` -> `HandleClientPlayer` (`0x14398ffe0`), calls
  `PostLogin` (`0x1414eb340` -> `0x143869990`), and sets `ClientLoginState = 3`.
- Current client and server `PreLogin` (`AGameModeBase::PreLogin` `0x14386a160`,
  `UOnlineEngineInterfaceImpl::IsCompatibleUniqueNetId` `0x1409cbee0`, F-712,
  F-713; U-037 resolved; native mirror `experimental::admission`): the ICARUS
  game mode and session add no checks beyond the engine's, and exhaustive `.text`
  and `.rdata`/`.data` scans prove that neither binary binds any callback to
  `FGameModeEvents::GameModePreLoginEvent` (`0x146056cf8` / `0x145f404b8`) or
  `FNetworkVersion` overrides (`0x1463251b0..c0` / `0x1462116a0..b0`). When
  `UniqueId` wraps a valid `FUniqueNetId`, `IsCompatibleUniqueNetId` accepts it
  if `GetType()` matches `GetDefaultOnlineSubsystemName()` or any entry in
  `CompatibleUniqueNetIdTypes` (`+0x78`), else sets `incompatible_unique_net_id`
  and skips `ApproveLogin`; otherwise `ApproveLogin` (`0x14385ac60`) checks
  `Server full.` against override or `MaxPlayers` or `SpectatorOnly` and
  `Maximum splitscreen players` on `SplitscreenCount` (with C-runtime integer
  reads saturating at `i32` bounds). `?`-delimited options split at the first
  `=` and match ASCII case-insensitively, first wins.
- Current client unique-net-id type hashes (F-543): 1 NULL, 2 MCP, 3 STEAM,
  4 PS4, 6 GOOGLE, 7 GOOGLEPLAY, 8 FACEBOOK, 9 IOS, 10 APPLE, 11 TENCENT,
  12 SWITCH, 13 AMAZON, 14 GAMECIRCLE, 15 THUNDERHEAD, 18 Oculus, 19 Quail,
  20 PS4SERVER; 5, 16, 17 and 21 are runtime FName globals; 31 carries a name.
- Epic's FNetworkNotify interface (four virtual callbacks) is a testable
  hypothesis for the slots other than `+0x18` (identified by F-506). Image
  deltas are not object offsets; provenance does not cross calls unchecked.

## Open questions

- Executed runtime dispatch of the qualified stores and slots (U-010); older
  server owner (U-011); archive/bit-position/byte-order path into the older
  dispatcher (U-014); indirect callers (U-015); RTTI, table identity and
  owner (U-016, U-017, U-018); receiver and type-registration join (U-043);
  authenticated stock-client session (U-038).

## Evidence

- Static cases: M-170 (`icarus-server-baseline`, E-011/E-013/E-021/E-022/E-024),
  M-146, [research/archive](../../research/archive/2026-10-12-control-caller-limit.md),
  M-023 to M-045 in the [control-store digest](../../research/network/control-stores.md),
  and M-589 to M-596, M-749 to M-752 (`PreLogin`, `SpawnPlayActor`, delegate
  scans and `UControlChannel` framing).
- Requirements before promotion: recover archive conversion and outer framing
  with bounds and errors; obtain a paired trace of a matching stock client and
  reference server; create sanitized fixtures for accepted negotiation and each
  rejection class (truncation at each field, zero/negative/minimum/oversized
  lengths, incompatible versions, invalid transitions, token failures); pass
  those fixtures and the stock-client experiment before marking negotiation
  implemented.
- Authored suites:

```bash
cargo test --locked --test control --test packet --test control_receive --test control_messages --test sequencing --test packet_lab --test peer_lab --test archive --test handshake --test admission
```

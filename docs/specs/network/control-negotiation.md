# Control negotiation

## Status

Experimental: static observations; the framing and Hello exchange are
wire-validated offline, and a synthetic Login and Join are welcomed (F-688
to F-690, F-712 to F-714). Scope: dedicated-server
build 25621760 (M-168) unless a current-client (build 25793767) comparison is
named. Last reconciled: 2026-10-18.
Full text: d5b0ae6:docs/specs/network/control-negotiation.md.

## Scope

The server-side control-message dispatcher and its initial (ID 0) branch, the
string reader it uses, `UWorld` / `UPendingNetGame` notify tables, packet and
bunch header parse order, `UControlChannel` byte-order persistence, queued send
bounds, fallback skip table, connection-state admission (`IsClientMsgTypeValid`),
`PreLogin` checks, `SpawnPlayActor` / `InitNewPlayer` join flow, and delegate
cross-reference scans across both binaries. Not covered: Steam packet handler
runtime initialization, compression, or integrity protection.

## Confirmed

- The dispatcher receives a connection, a message ID and an archive-like input
  after lower layers; a message-validity check precedes table dispatch, and a
  separate branch serves another operating mode (F-284).
- Dispatch table (M-170/E-011, M-170/E-013):

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
- ID 0 reads one endianness byte, a 32-bit remote version, and a token string
  ([archive-string spec](../save-format/archive-string.md)), compares versions
  via `0x141f2e6e0` ([checksum spec](network-checksum.md)), sends `Upgrade` and
  closes on mismatch, sends `Challenge` on an empty token, and requires an
  encryption delegate on a nonempty token (F-284, F-713).
- `UWorld` (`0x1439da2e0`) and `UPendingNetGame` (`0x1437a9640`) install primary
  and `FNetworkNotify` vftables at `+0` and `+0x28` (`0x145467ef0` / `0x145468180`
  on `UWorld`, `0x14540a038` / `0x14537cb18` on `UPendingNetGame`; `TARGET02`
  `0x1439dae50` is `UWorld::~UWorld`). `UWorld::Listen` and
  `UPendingNetGame::InitNetDriver` store `this + 0x28` at `Driver + 0x208`, and
  `NotifyControlMessage` (`0x1439ee7f0`) adjusts `this` by `-0x28` (F-063, F-076,
  F-088, F-106, F-505, F-506, F-520, F-544, F-546, F-723, F-724; U-007 to U-013,
  U-015 to U-018, U-043 resolved).
- Receive and send path (F-514, F-515, F-518, F-712, F-714; U-014, U-044
  resolved): `UControlChannel::ReceivedBunch` (client `0x1437c4940`, server
  `0x143536420`), while `bNeedsHeaderInspection` (`+0x68`) is set, closes unless
  the bunch holds >= 16 bits and byte 0 is `0` (`Hello`); byte 1 == 1 clears
  archive byte-swap bit `0x20` at `+0x29` (else sets it), persists
  `Connection + 0x158` (`Data[1] != 1`), and clears `+0x68`. Subsequent
  `FInBunch` and `FControlChannelOutBunch` instances copy `+0x29` bit `0x20` from
  `Connection + 0x158`. It then loops while `!Bunch.AtEnd() && State != USOCK_Closed`:
  ID `0x16` (`NetGUIDAck`) dispatches locally (`PackageMap->ReceivedNak`), other
  IDs call `Notify->NotifyControlMessage`, and unhandled messages (`Pos == PosBefore`)
  skip across 19 layouts (`0..18`, `21`, failing closed on others;
  `read_control_fallback` / `write_control_fallback` in
  `experimental::control_messages`). `UControlChannel::SendBunch` sends
  immediately when `QueuedMessages` (`+0x70`) is empty and
  `NumOutRec < 255 + bClose`, else queues up to `32,767` (`0x7fff`) messages and
  drains them in `Tick` once `bOpenAcked` (`+0x30 & 1`) is set.
- Packet and bunch headers (F-521 to F-527, F-688, F-710, F-711, F-714), read
  LSB-first after optional magic-header bits, the stateless-connect bit (`0`) and
  Steam-auth bit (`0`), and before the connection and handler termination bits:

  | Order | Field | Encoding |
  | --- | --- | --- |
  | 1 | Packet word | 32 bits: history words minus one (bits 0-3, at most 8 words), acked sequence (4-17), sequence (18-31) |
  | 2 | Ack history | that many 32-bit words |
  | 3 | Packet info | one bit; if set, jitter below 1024, frame-time bit and, to a client, one frame-time byte (F-525) |
  | 4 | Control | one bit; if set, open and close bits |
  | 5 | Close reason | if closing: value below 15 |
  | 6 | Paused, reliable | one bit each |
  | 7 | Channel index | packed, below the connection's channel limit |
  | 8 | Exports, mapped GUIDs, partial | one bit each |
  | 9 | Reliable sequence | if reliable: value below 1024, relative to the channel's last |
  | 10 | Partial initial, final | if partial: one bit each |
  | 11 | Channel name | if open or reliable: one bit; if set a packed index (`<= 702`), else an archive string and 32-bit number (F-525) |
  | 12 | Payload | bit count below `8 * MaxPacket`, then those bits copied to the bunch |

- Reliability (F-535 to F-539, F-710): handshake ack seeds incoming/outgoing
  packet sequences from the cookie's first two LE `u16` words masked to 14 bits
  (initial reliable sequences take the low 10 bits). Out-of-order reliable
  bunches queue per channel (max 255); acks 1 to 8191 ahead resolve outgoing
  packets via the 256-bit history window and resend lost reliable bunches.
- Control vocabulary (F-529 to F-531, F-548); strings are archive strings and
  `u32` values follow the Hello byte-swap flag:

  | ID | Direction | Fields |
  | --- | --- | --- |
  | 0 Hello | to server | byte, u32 version, string token |
  | 1 Welcome | to client | three strings: map URL, game name, redirect URL |
  | 2 Upgrade | to client | u32 local version (sent on incompatible Hello, then close) |
  | 3 Challenge | to client | string (counter-derived token, F-531) |
  | 4 Netspeed | to server | u32 (clamped to `>= 1,800`) |
  | 5 Login | to server | string response (`"0"`, F-548), string URL, unique-net-id (F-531), string platform |
  | 6 Failure | to client | string, then the client closes |
  | 9 Join | to server | none |
  | 10 Split join | to server | string URL, unique-net-id |
  | 15 Swap | to server | u32 |
  | 17 Debug | to server | string |
  | 18 NetGUID | to client | packed int, string |
  | 21 Encryption ack | to client | none |

## Candidate

- Experimental modules (`control`, `packet`, `control_receive`,
  `control_messages`, `sequencing`, `admission`, `packet_lab`, `peer_lab`) model
  packet/bunch headers, the 19-entry control fallback table, unique-net-id forms
  (F-543, F-636) and disconnected loopback negotiation through `Hello`,
  `Challenge`, `Login`, `PreLogin`, `Welcome` and `Join` / `SplitJoin` (F-558,
  F-564, F-689, F-690, F-712 to F-714).
- Server `UWorld::NotifyControlMessage` (`0x1439ee7f0`, F-563, F-713) and
  `WelcomePlayer` (`0x1439fdeb0`, F-580): `IsClientMsgTypeValid` (`0x143991c70`)
  gates messages by `ClientLoginState` (`LoggingIn = 1`, `Welcomed = 2`,
  `Joined = 3`) and `ExpectedClientLoginMsgType` (`0` = `Hello`, `5` = `Login`).
  On `Join` (`9`), if `PlayerController` (`+0x30`) is null, `SpawnPlayActor`
  (`0x14392dbb0`) runs `AGameModeBase::Login` -> `ApproveLogin` ->
  `SpawnPlayerController` -> `InitNewPlayer`, sets `NetPlayerIndex` (`+0x3d4`),
  binds `HandleClientPlayer`, calls `PostLogin`, and sets `ClientLoginState = 3`
  (F-713; U-010 resolved).
- `PreLogin` (`AGameModeBase::PreLogin` `0x14386a160`, F-712, F-713; U-037
  resolved; `experimental::admission`): neither binary binds any callback to
  `GameModePreLoginEvent` or `FNetworkVersion` overrides. `IsCompatibleUniqueNetId`
  checks `GetDefaultOnlineSubsystemName()` / `CompatibleUniqueNetIdTypes`, and
  `ApproveLogin` (`0x14385ac60`) checks `Server full.` and `Maximum splitscreen players`.

## Open questions

- Executed runtime dispatch in an authenticated stock-client session (U-038).

## Evidence

- Static cases: M-170 (`icarus-server-baseline`, M-170/E-011, M-170/E-013,
  M-170/E-021, M-170/E-022, M-170/E-024),
  M-146, [research/archive](../../research/archive/2026-10-12-control-caller-limit.md),
  M-023 to M-045 in the [control-store digest](../../research/network/control-stores.md),
  and M-589 to M-596, M-749 to M-754, M-763 to M-765, M-770, M-771.
- Authored suites: `cargo test --test control_negotiation --locked` and
  `cargo test --test peer_lab --locked`.

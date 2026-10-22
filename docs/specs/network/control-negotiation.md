# Control negotiation

## Status

Experimental: static observations; the framing and Hello exchange are
wire-validated offline, and a synthetic Login and Join are welcomed (F-688,
F-689, F-712 to F-714, F-858). Scope: dedicated-server build 25621760 (M-168)
unless a current-client (build 25793767) comparison is named.
Last reconciled: 2026-10-22.
Full text: d5b0ae6:docs/specs/network/control-negotiation.md.

## Scope

Server control dispatch, notify tables, packet and bunch headers,
`UControlChannel` byte order, queueing and fallback layouts, admission,
`PreLogin` and join, across both binaries. Not covered: Steam packet-handler runtime
initialization, compression or integrity protection.

## Confirmed

- Dispatch (F-284, F-757; M-170/E-011, M-170/E-013): a validity check precedes
  table dispatch, and a separate branch serves another operating mode.

  | Inner ID | Branch entry | Meaning |
  | --- | --- | --- |
  | 0 | `0x1439eebec` | Hello: version and token negotiation |
  | 4 | `0x1439ef03b` | `NMT_Netspeed`: clamps `int32 Rate` to `[1800, NetDriver->MaxClientRate (+0x64)]` into `Connection->CurrentNetSpeed (+0x38)` |
  | 5 | `0x1439ef0e0` | `NMT_Login` |
  | 9 | `0x1439ef7f1` | `NMT_Join` |
  | 10 | `0x1439efb8a` | `NMT_JoinSplit` |
  | 15 | `0x1439f031c` | `NMT_PCSwap`: reads `int32 ChildIndex`, resolves the parent or `Children[ChildIndex]`, destroys an unowned `Actor` when `Actor->NetConnection (+0x410) == TargetConn` |
  | 17 | `0x1439f0416` | `NMT_DebugText`: logs an `FString`, no state change |

  Other IDs, including out-of-range ones, reach the common exit.
- Hello reads a token string ([archive-string spec](../save-format/archive-string.md))
  and compares versions ([checksum spec](network-checksum.md)); a nonempty token
  requires an encryption delegate (F-284, F-713, F-758; step F-563).
- Notify tables (F-505, F-724, F-753 to F-755; F-063, F-076, F-088, F-106,
  F-506, F-520, F-544, F-546, F-723; U-007 to U-013, U-015 to U-018, U-043
  resolved): both constructors install primary and `FNetworkNotify` vftables at
  `+0` and `+0x28` (`UWorld` `0x145467ef0` / `0x145468180`, `UPendingNetGame`
  `0x14540a038` / `0x14537cb18`); `TARGET02`
  `0x1439dae50` is `UWorld::~UWorld`. `UWorld::Listen` and
  `UPendingNetGame::InitNetDriver` store `this + 0x28` at `Driver + 0x208`, and
  `NotifyControlMessage` (`0x1439ee7f0`) adjusts `this` by `-0x28`.
- Receive and send path (F-514, F-515, F-518, F-712, F-714, F-740, F-756; U-014,
  U-044 resolved): `UControlChannel::ReceivedBunch` (client `0x1437c4940`, server
  `0x143536420`), while `bNeedsHeaderInspection` (`+0x68`), applies F-756's
  first-bunch check (byte-swap bit `0x20` at `+0x29`), persists
  `Connection + 0x158` (`Data[1] != 1`) and clears `+0x68`. Later `FInBunch` and
  `FControlChannelOutBunch` copy that bit from `Connection + 0x158`. While
  `!Bunch.AtEnd() && State != USOCK_Closed`, ID `22` (`NMT_DestructionInfo`)
  goes to `ReceiveDestructionInfo` (`0x1435354d0` / `0x1437c3d50`, `u8 Reason` +
  `FNetworkGUID`), other IDs to `Notify->NotifyControlMessage`, and unhandled
  messages (`Pos == PosBefore`) to 20 layouts (`0..18`, `21`, `22`; others fail
  closed; `16` closes on `PlayerController` channel failure, `19` logs an
  `FString`, `20` forwards `u8 + FString` to `NetDriver`/`World`/`GEngine`;
  `experimental::control_messages`). `UControlChannel::SendBunch` sends at once
  when `QueuedMessages` (`+0x70`) is empty and `NumOutRec < 255 + bClose`, else
  queues up to `0x7fff` messages and drains them in `Tick` once `bOpenAcked`
  (`+0x30 & 1`).
- Packet and bunch headers (F-521 to F-527, F-688, F-710, F-711, F-714, F-759),
  LSB-first after optional magic-header bits, the stateless-connect bit (`0`) and
  Steam-auth bit (`0`), before the connection and handler termination bits:

  | Order | Field | Encoding |
  | --- | --- | --- |
  | 1 | Packet word | 32 bits: history words minus one (0-3, at most 8), acked sequence (4-17), sequence (18-31) |
  | 2 | Ack history | that many 32-bit words |
  | 3 | Packet info | one bit; if set, jitter below 1024, frame-time bit and, to a client, one frame-time byte (F-843) |
  | 4 | Control | one bit; if set, open and close bits |
  | 5 | Close reason | if closing: value below 15 |
  | 6 | Paused, reliable | one bit each |
  | 7 | Channel index | packed, below the connection's channel limit |
  | 8 | Exports, mapped GUIDs, partial | one bit each |
  | 9 | Reliable sequence | if reliable: below 1024, relative to the channel's last |
  | 10 | Partial initial, final | if partial: one bit each |
  | 11 | Channel name | if open or reliable: one bit; set, a packed index (`<= 702`), else an archive string and 32-bit number (F-843) |
  | 12 | Payload | bit count below `8 * MaxPacket`, then those bits |

- Reliability: sequence seeding, ack history and resend F-759 (steps F-535 to
  F-539); reliable reordering F-710.
- Control vocabulary (F-529, F-530, F-548, F-740, F-758, F-844); strings are archive
  strings and `u32` values follow the Hello byte-swap flag:

  | ID | Direction | Fields |
  | --- | --- | --- |
  | 0 Hello | to server | byte, u32 version, string token |
  | 1 Welcome | to client | strings: map URL, game name, redirect URL |
  | 2 Upgrade | to client | u32 local version, then close |
  | 3 Challenge | to client | string (F-844) |
  | 4 Netspeed | to server | i32 rate |
  | 5 Login | to server | string response (`"0"`, F-548), string URL, unique-net-id (F-844), string platform |
  | 6 Failure | to client | string, then the client closes; shutdown sends it, then a reason-0 control close (F-782) |
  | 9 Join | to server | none |
  | 10 Split join | to server | string URL, unique-net-id |
  | 15 Swap | to server | i32 child index (`-1` parent, `>= 0` splitscreen child) |
  | 16 ActorChannelFailure | to server | i32 channel index (closes if `Ch->Actor == PlayerController`) |
  | 17 Debug | to server | string |
  | 18 NetGUID | to client | packed int, string |
  | 21 Encryption ack | to client | none |
  | 22 DestructionInfo | to client | u8 reason, packed NetGUID (`!= 1`) |

## Candidate

- The `experimental` control, packet, sequencing, admission and lab modules
  model the headers, the 20-entry fallback table, unique-net-id forms (F-543, F-844),
  `clamp_client_netspeed`, `evaluate_pc_swap_target`,
  `evaluate_actor_channel_failure` (F-740), and disconnected loopback
  negotiation through `Hello`, `Challenge`, `Login`, `PreLogin`, `Welcome`,
  `Netspeed`, `Swap`, `Debug` and `Join` / `SplitJoin` (F-874, F-689,
  F-712 to F-714, F-858).
- `UWorld::NotifyControlMessage` (F-563, F-713, F-740) and `WelcomePlayer`
  (`0x1439fdeb0`, F-580): `IsClientMsgTypeValid` admits only
  `ExpectedClientLoginMsgType` (`0` Hello, `5` Login) in `ClientLoginState`
  `LoggingIn = 1`, and every non-login ID (`!= 0 && != 5`) in `Welcomed = 2` and `Joined = 3`. On
  `Join` (`9`) with a null `PlayerController` (`+0x30`), `SpawnPlayActor` runs
  `AGameModeBase::Login` -> `ApproveLogin` -> `SpawnPlayerController` ->
  `InitNewPlayer`, sets `NetPlayerIndex` (`+0x3d4`), binds `HandleClientPlayer`,
  calls `PostLogin` and sets `ClientLoginState = 3` (U-010 resolved).
- `PreLogin` (F-712, F-713; U-037 resolved; `experimental::admission`):
  `IsCompatibleUniqueNetId` checks `GetDefaultOnlineSubsystemName()` /
  `CompatibleUniqueNetIdTypes`, and `ApproveLogin` (`0x14385ac60`) checks
  `Server full.` and `Maximum splitscreen players`.

## Open questions

- Executed runtime dispatch in an authenticated stock-client session (U-038).

## Evidence

- Static: M-170 (`icarus-server-baseline`; M-170/E-011, M-170/E-013,
  M-170/E-021, M-170/E-022, M-170/E-024), M-146,
  [research/archive](../../research/archive/2026-10-12-control-caller-limit.md),
  M-023 to M-045 ([control-store digest](../../research/network/control-stores.md)),
  M-589 to M-596, M-749 to M-754, M-763 to M-765, M-770, M-771, M-776, M-777,
  M-780, M-791.
- Tests: `cargo test --test control_negotiation --locked` and
  `cargo test --test peer_lab --locked`.

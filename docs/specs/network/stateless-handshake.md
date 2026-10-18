# Stateless handshake framing

## Status

Candidate, wire-observed for the initial exchange and static-verified for prefix,
restart and tolerance rules. Scope: shipping builds 25621760 and 25793767
(M-168, M-770, M-771). Last reconciled: 2026-10-18.
Full text: d5b0ae6:docs/specs/network/stateless-handshake.md.

## Scope

The stateless connection handshake: initial request, challenge, cookie echo,
acknowledgement, optional `MagicHeader` prefix, 1-byte restart request and
49-byte extended restart response. Not covered: platform authentication, the
version/control channel, replicated state and gameplay admission.

## Confirmed

- Short frame without optional prefix, bit positions from zero, least-significant
  bit first, timestamp little-endian IEEE-754 binary64 (F-280, F-729):

  | Bit offset | Width | Field |
  | --- | --- | --- |
  | 0 | 1 | Handshake marker, one |
  | 1 | 1 | Restart flag |
  | 2 | 1 | Secret selector bit |
  | 3 | 64 | Timestamp |
  | 67 | 160 | Cookie bytes |
  | 227 | 1 | Termination, one |
  | 228 | 4 | Unused padding to 29 bytes |

- The initial client request is 29 bytes: first byte `01`, last byte `08`, all
  others zero (zero restart/selector bits, 28 zero bytes, termination bit).
- The original replied to that request with a 29-byte challenge (restart false,
  selector true, finite positive timestamp, 20-byte cookie) and, after an echo
  of the issued timestamp, selector and cookie, with a 29-byte acknowledgement
  carrying timestamp `-1.0`, selector one, restart zero and the same cookie. Its
  log recorded `Handler Handshake Complete`; Steam authentication was inactive
  (F-253, M-169).
- Optional prefix (`0x14372d5d0`): reads `net.StatelessConnectComponent.MagicHeader`
  and accepts `1..=32` ASCII `'0'`/`'1'` bits (`0` bits in stock ICARUS); empty,
  `>32`-char or non-binary strings reset the prefix count (`this + 0xe8`) to `0`
  (F-729; U-051 resolved).
- `ParseHandshakePacket` (`0x1437419a0` server, `0x1439dec65` client) accepts
  only three exact remaining bit counts after the 1-bit marker and trailing
  termination-bit strip: `1` bit (`1`-byte `0x07` server-to-client restart
  request from `SendRestartHandshakeRequest` `0x143745ee0`), `226` bits (`29` B
  standard packet) or `386` bits (`49` B extended restart response carrying
  `Cookie` + `OrigCookie` from `SendChallengeResponse` `0x143745810`). All other
  bit counts call `Packet.SetError()` (`0x141f62140`), and `IncomingConnectionless`
  (`0x14373f0f0`) rejects `NaN` or `ChallengeAge` outside `[0.0, 40.0)`
  (`_DAT_1453edbc8 = 40.0`) (F-729; U-051 resolved).
- Address-backed routines (preferred virtual addresses in the inspected file):

  | Address | Observation |
  | --- | --- |
  | `0x14372d5d0` | Handler constructor, optional prefix state and virtual table |
  | `0x1437405e0` | Initial request writer: prefix, three bits, 28 zero bytes, terminator |
  | `0x143745810` | Client response writer: flags, eight-byte timestamp, 20-byte cookie, optional previous cookie |
  | `0x1437455c0` | Acknowledgement writer |
  | `0x143745ee0` | Restart-handshake request writer (`3` bits: `0x07`) |
  | `0x141f637b0` | Single-bit writer |
  | `0x141f5ff10` | Byte serialization into the bit writer |
  | `0x1437419a0` | Parser: accepts 226 or 386 remaining bits after the marker, plus a restart-request form |
  | `0x14373f0f0` | Connectionless handler; zero timestamp reaches the challenge sender |
  | `0x14373ec60` | Connected branch; positive timestamp prompts a response, negative enters acknowledgement handling |

## Candidate

- Current client (build 25793767): after a negative-timestamp acknowledgement
  the client seeds its connection's incoming and outgoing sequences from the
  cookie's first and second little-endian 16-bit words, masked to 14 bits
  (F-536), and data packets carry a zero handshake bit after any magic header
  (F-524). The server seeds sequences identically (`0x14373f0f0`, F-714).
- The loopback lab (`examples/handshake_lab.rs`) generates cookies with
  HMAC-SHA256 truncated to 20 bytes over `icarus-bevy/handshake-lab/v1\0`, IPv4
  octets, big-endian port and little-endian binary64 timestamp, with a fresh
  32-byte OS-random secret per process and selector fixed at zero. It has not
  been tested with the stock client (U-052).

## Open questions

- Stock-client behavior against the native lab; secret rotation and subsequent
  transport (U-052).
- Authenticated admission and the connection state machine beyond this
  exchange (U-038).

## Evidence

- Original exchange and static verification: M-169 (`run-02-handshake`,
  `run-03-stateless-ack`, `decompile-05/`, `decompile-06/`), M-770, M-771.
- Authored tests: `tests/handshake.rs` covers 29-byte, 1-byte (`0x07`) and
  49-byte (`OrigCookie`) frames, `MagicHeader` validation and malformed-frame
  rejection; `tests/handshake_lab.rs` covers UDP loopback exchanges.

```bash
cargo test --locked --test handshake
cargo test --locked --test handshake_lab
cargo run --locked --example handshake_lab -- --seconds 30
```

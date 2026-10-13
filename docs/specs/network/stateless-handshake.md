# Stateless handshake framing

## Status

Candidate, wire-observed for the initial exchange only. Scope: shipping build
25621760 (M-168). Last reconciled: 2026-10-13.
Full text: d5b0ae6:docs/specs/network/stateless-handshake.md.

## Scope

The stateless connection handshake: initial request, challenge, cookie echo and
acknowledgement. Not covered: platform authentication, the version/control
channel, replicated state, gameplay admission, optional binary prefixes and
extended restart responses. The native loopback lab is a research tool, not a
server listener or game client.

## Confirmed

- Short frame without optional prefix, bit positions from zero, least-significant
  bit first, timestamp little-endian IEEE-754 binary64 (F-280):

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
- The acknowledgement sender writes marker one, restart zero, selector one,
  timestamp `-1.0`, the supplied cookie and a termination bit. The connectionless
  receiver checks a 40-second timestamp window and a selected secret before
  accepting a cookie response.
- Address-backed routines (preferred virtual addresses in the inspected file):

  | Address | Observation |
  | --- | --- |
  | `0x14372d5d0` | Handler constructor, optional prefix state and virtual table |
  | `0x1437405e0` | Initial request writer: prefix, three bits, 28 zero bytes, terminator |
  | `0x143745810` | Client response writer: flags, eight-byte timestamp, 20-byte cookie, optional previous cookie |
  | `0x1437455c0` | Acknowledgement writer |
  | `0x141f637b0` | Single-bit writer |
  | `0x141f5ff10` | Byte serialization into the bit writer |
  | `0x1437419a0` | Parser: accepts 226 or 386 remaining bits after the marker, plus a restart-request form |
  | `0x14373f0f0` | Connectionless handler; zero timestamp reaches the challenge sender |
  | `0x14373ec60` | Connected branch; positive timestamp prompts a response, negative enters acknowledgement handling |

  Read paths mask bit positions with `1 << (position & 7)`; the parser reads two
  flag bits, eight bytes and twenty bytes. Ghidra-inferred types are provisional.

## Candidate

- The handler supports an optional binary prefix of up to 32 bits and extended
  restart responses with a second 20-byte cookie; neither is implemented or
  observed (U-051).
- The native decoder's strict policy (reject nonfinite timestamps, nonzero
  padding, malformed termination, any length other than 29 bytes) is a research
  choice, not a reproduction of original tolerances (U-051).
- Epic's current StatelessConnectHandlerComponent documentation gives context
  only; current engine versions carry additional fields and must not be
  substituted for these routines.
- The loopback lab (`examples/handshake_lab.rs`) generates cookies with
  HMAC-SHA256 truncated to 20 bytes over `icarus-bevy/handshake-lab/v1\0`, IPv4
  octets, big-endian port and little-endian binary64 timestamp, with a fresh
  32-byte OS-random secret per process and selector fixed at zero. This is not a
  claim about the original cookie algorithm. Echoes need a positive, nonfuture
  timestamp under 40 seconds old and a matching cookie checked with a
  constant-time truncated MAC; restarts invalidate earlier cookies. It has not
  been tested with the stock client (U-052).

## Open questions

- Optional prefix, extended restart, and original tolerance policy (U-051).
- Stock-client behavior against the native lab; secret rotation and subsequent
  transport (U-052).
- Authenticated admission and the connection state machine beyond this
  exchange (U-038).

## Evidence

- Original exchange observations: M-169 (runs `run-02-handshake` and
  `run-03-stateless-ack`, private evidence E-071 to E-076); decompilation under
  M-169 (`decompile-05/`, `decompile-06/`).
- Authored tests: `tests/handshake.rs` decodes an independently packed fixture
  (timestamp 123.5, cookie bytes 0 to 19), the initial request, every truncation
  boundary, excess length and marker/padding errors. `tests/handshake_lab.rs`
  checks the Python oracle value `d38907b0827ab14de4527fa34d89e6dd3d8c8036`
  for `127.0.0.1:12345` at timestamp 123.5 with key `bytes([7])*32`, forged
  fields, wrong peer/key, expiry boundaries, real UDP exchanges inside a Bevy app
  and the 32-datagram-per-update budget.

```bash
cargo test --locked --test handshake
cargo test --locked --test handshake_lab
cargo run --locked --example handshake_lab -- --seconds 30
```

- `examples/handshake_probe.rs` is the bounded research client used inside the
  reference container: loopback `127.0.0.1:17777` only, at most three 29-byte
  requests, two-second waits, no identity material; `--complete-stateless`
  echoes the issued cookie. Run it only under an authorized case
  ([launcher runbook](../../runbooks/original-server-launcher.md)).
- The lab processes at most 32 datagrams per `Update` with a 30-byte stack
  buffer, binds IPv4 loopback on an ephemeral port, exits after 30 seconds
  (maximum 600) and allocates no per-peer state. Dependencies: `hmac` 0.13.0,
  `sha2` 0.11.1, `digest` 0.11.3, `getrandom` 0.4.3, `ctutils` 0.4.3, `cmov`
  0.5.4 (MIT OR Apache-2.0), recorded in `Cargo.lock`.

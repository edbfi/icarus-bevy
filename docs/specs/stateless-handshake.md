# Candidate stateless handshake framing

Status: **static interpretation and authored-fixture tests; original probe pending**.
Scope: shipping build 25621760, executable SHA-256
`67cfc915d9d085b758057e2960d4a2018fb121123af1329c25ef7aa58b960104`.
This is an early transport layer. It does not implement platform authentication,
the version/control channel, replicated state or gameplay admission.

## Short frame with no optional prefix

Bit positions below start at zero; bits within each byte are least-significant
first. Timestamp bytes use little-endian IEEE-754 binary64 representation.

| Bit offset | Width | Candidate field |
| --- | --- | --- |
| 0 | 1 | Handshake marker, one |
| 1 | 1 | Restart flag |
| 2 | 1 | Secret selector bit |
| 3 | 64 | Timestamp |
| 67 | 160 | Cookie bytes |
| 227 | 1 | Termination, one |
| 228 | 4 | Unused padding to 29 bytes |

The initial client routine writes zero restart/selector bits in its initial state,
28 zero bytes for timestamp/cookie, and a termination bit. The resulting candidate
datagram is 29 bytes: first byte `01`, last byte `08`, all others zero. This byte
sequence is independently constructed from the field observations.

The handler constructor supports an optional binary prefix of up to 32 bits.
Extended restart responses add another 20-byte cookie. Neither is implemented by
the short-frame decoder. Its strict policy rejects nonfinite timestamps,
nonzero padding, malformed termination and every length other than 29 bytes.
These rejection choices are not a complete reproduction of original tolerances.

## Address-backed interpretation

| Preferred address | Observation |
| --- | --- |
| `0x14372d5d0` | Handler constructor, optional prefix state and virtual table |
| `0x1437405e0` | Initial request: prefix, three bits, 28 zero bytes, terminator |
| `0x143745810` | Client response: flags, eight-byte timestamp, 20-byte cookie and optional previous cookie |
| `0x141f637b0` | Single-bit writer |
| `0x141f5ff10` | Byte serialization into bit writer |
| `0x1437419a0` | Parser accepts 226 or 386 remaining bits after the leading marker, plus a separate restart-request form |
| `0x14373f0f0` | Connectionless handler; zero timestamp reaches the challenge sender; other timestamps enter age/selector/cookie checks |
| `0x14373ec60` | Connected/client branch; positive timestamp prompts response, negative timestamp enters acknowledgement handling |

The read paths explicitly mask bit positions with `1 << (position & 7)`. The
parser reads two flag bits, eight bytes and twenty bytes, matching the request and
response writers. Raw decompilation and references remain in the private overnight
case's `decompile-05/` and `decompile-06/`. Types inferred by Ghidra remain provisional.

Epic describes this component as a stateless connection handshake. Its current
[API documentation](https://dev.epicgames.com/documentation/unreal-engine/API/Runtime/Engine/StatelessConnectHandlerComponent)
provides context, not this build's wire specification: current engine versions have
additional fields and must not be substituted for these inspected routines.

## Reproducible authored probe

`cargo test --locked --test handshake` decodes an independently packed fixture
with timestamp 123.5 and cookie bytes 0 through 19. It also checks the initial
request and every truncation boundary, excess length and marker/padding errors.

`examples/handshake_probe.rs` is a bounded research client, not a game client or
server listener. The approved offline launcher can mount its compiled executable
read-only and execute it inside the reference container's isolated namespace.
It targets only `127.0.0.1:17777`, sends at most three 29-byte initial requests,
waits two seconds per response, and records send/receive bytes itself. It never
echoes a cookie, sends identity material or attempts a gameplay connection.
Do not run it on the host or outside an authorized case.

Acceptance requires a response that agrees with this layout and the original
server's relevant behavior. A timeout, decoder error or engine startup message
alone cannot validate framing. A challenge response would validate only this
limited exchange, leaving the connection state machine and stock-client tests open.

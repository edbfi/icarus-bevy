# Candidate stateless handshake framing

Status: **initial request/challenge/cookie response/acknowledgement observed against
the original; authentication and game connection remain unvalidated**.
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
uses identity material or attempts a gameplay connection. An explicit
`--complete-stateless` option echoes the server-issued cookie through the independent
encoder and checks the resulting acknowledgement; it does not send a control message
or alter any identity check. Without that option it stops after the challenge.
Do not run it on the host or outside an authorized case.

Acceptance requires a response that agrees with this layout and the original
server's relevant behavior. A timeout, decoder error or engine startup message
alone cannot validate framing. A challenge response would validate only this
limited exchange, leaving the connection state machine and stock-client tests open.

## Original initial-exchange observation

The isolated `run-02-handshake` received a 29-byte reply to its first request. The
independent decoder found restart false, selector true, a finite positive timestamp
and a 20-byte cookie. The game log recorded an accepted loopback connection at the
probe time. No cookie response was sent in that run; the probe exited successfully
and the launcher stopped and removed the container after its bounded observation.
Raw bytes and logs remain in the private case. This validates that initial exchange
for this build/configuration; it does not validate the entire decoder rejection
policy or any later game protocol.

The acknowledgement sender at `0x1437455c0` writes marker one, restart zero,
selector one, timestamp `-1.0`, the supplied cookie and a termination bit. The
connectionless receiver also checks a 40-second timestamp window and a selected
secret before accepting a cookie response.

In `run-03-stateless-ack`, the independent encoder echoed the issued timestamp,
selector and cookie. The original returned a 29-byte acknowledgement with timestamp
`-1.0`, selector one, restart zero and the same cookie. Its log recorded
`Handler Handshake Complete` and an accepted connection. The probe sent no control
messages or identity material. The original also logged that `SteamAuthComponent`
was deactivated because requirements were missing in the offline environment.
This is evidence for the stateless exchange only, not authenticated admission.
Both containers were stopped and removed; private evidence E-071 through E-076
preserves the observations and cleanup records.

## Independent Bevy loopback lab

`examples/handshake_lab.rs` installs a separate optional Bevy plugin. It binds only
IPv4 loopback, defaults to an ephemeral port and exits after 30 seconds (maximum
600). The normal `icarus-server run` command does not install it. It creates no
game connections or entities and uses no original assets or code.

```bash
cargo run --locked --example handshake_lab -- --seconds 30
cargo test --locked --test handshake_lab
```

Each `Update` processes at most 32 datagrams using a 30-byte stack buffer. Larger
datagrams, unsupported restart forms and malformed frames are discarded. Responses
are 29 bytes. There is no application packet queue or per-peer state allocation.
UDP send/receive errors are counted; a blocked socket does not block simulation.
These are research bounds, not production rate limiting or a denial-of-service
qualification.

The lab independently generates cookies using HMAC-SHA256 truncated to 20 bytes,
with a fresh OS-random 32-byte secret per process. This is **not a claim about the
original cookie algorithm**. A cookie covers the domain bytes
`icarus-bevy/handshake-lab/v1\0`, IPv4 octets, big-endian port and little-endian
binary64 timestamp. The selector is fixed at zero. Echoes must have a positive,
nonfuture timestamp less than 40 seconds old and a matching cookie verified through
RustCrypto's constant-time truncated-MAC API. Repeated valid echoes are idempotent;
there is no player/session allocation. A restart invalidates previous cookies.

The independent Python oracle uses `hmac.new(bytes([7])*32, message,
hashlib.sha256).digest()[:20]`. For `127.0.0.1:12345` at timestamp 123.5 the result is
`d38907b0827ab14de4527fa34d89e6dd3d8c8036`. Tests check this value, forged fields,
wrong peer/key, expiration/future boundaries and actual UDP exchanges within a
Bevy app. Oversized traffic tests check the packet budget and advancing simulation.

The lab uses RustCrypto `hmac` 0.13.0 and `sha2` 0.11.1 with digest 0.11.3,
plus `getrandom` 0.4.3. Constant-time verification uses `ctutils` 0.4.3 and
`cmov` 0.5.4; these packages declare MIT OR Apache-2.0 licensing. HMAC construction
imports `KeyInit` separately from `Mac`. Cargo.lock records registry checksums.
Their APIs supply
[constant-time truncated verification](https://docs.rs/digest/0.11.3/digest/trait.Mac.html)
and [OS randomness](https://docs.rs/getrandom/0.4.3/getrandom/fn.fill.html).
The lab has not been tested with the stock Windows client; its stricter initial
packet policy, lack of secret rotation, optional prefixes, restarts and subsequent
transport all remain separate compatibility questions.

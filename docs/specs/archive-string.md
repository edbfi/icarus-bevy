# Experimental archive-string subset

Status: **bounded independent codec with authored tests; original wire round trip
pending**. This is a field codec, not a UDP packet or game admission implementation.
The caller must establish archive byte order and framing before using it.

The reader/writer at preferred address `0x141e15db0` in tool build 25621760 uses a
signed 32-bit count. Zero is empty, a positive count measures narrow bytes, and a
negative count measures UTF-16 units. Nonzero counts include one terminating unit.
The reader rejects `i32::MIN` before negation and respects an archive maximum when
one is configured. Wide units may be byte-swapped according to archive flags.

The complete chained function ranges and follow-up character converters establish
more than the initial partial disassembly:

- The reader overwrites the final unit with zero instead of requiring a terminator
  in the input. A count of one becomes an empty string.
- Narrow conversion at `0x1408a91f0` handles ASCII directly. Negative signed-byte
  values are replaced by the caller's `?` fallback; this is not UTF-8 decoding.
- The wide reader has special handling for `U+FFFF` that inserts a zero and may
  shorten the resulting string. Raw surrogate-pair validation was not observed.
- The default writer selects narrow encoding for ASCII; other units select wide
  encoding. A separate archive flag can force wide output. The narrow output helper
  at `0x1409d1a30` also has a replacement path for unrepresentable units.

Private evidence: overnight E-021 (complete reader/writer), E-085 (follow-up unwind
ranges), E-086/E-087 (character converters). These observations are build-specific;
they do not establish all archive flags or runtime callers.

## Deliberate strict subset

`experimental::archive` implements both explicit byte orders, preserving UTF-16
code units in a `Vec<u16>` and reporting the exact consumed byte count. It does not
silently convert invalid surrogate sequences to a Rust `String`. Following fields
remain untouched. Empty output uses count zero; ASCII is preferred for writing.
Forced-wide output is not implemented.

The research cap is **65,536 units including the terminator**, checked before
allocation or length multiplication. The decoder checks the complete declared
input range before building its output. It rejects non-ASCII narrow bytes, missing
terminators, embedded NUL and `U+FFFF`. These choices deliberately reject original
normalization cases rather than silently changing an identity-bearing value.
They are documented compatibility differences, not original-server bug fixes.

Tests independently pack ASCII and supplementary-character fixtures in both byte
orders, check every truncation boundary, preserve raw surrogate units, leave a
following field untouched, and exercise minimum/maximum signed counts and the
allocation boundary. No original serialized string fixture or game data is copied
into the test suite.

```bash
cargo test --locked --test archive
```

Before promoting this primitive into negotiation, validate the actual archive
flags and byte order from controlled stock-client traffic, decide each normalization
difference explicitly, and retain identity/platform checks above the field codec.

## Concrete string-property link

A later exact-build trace binds StrProperty vtable `0x144ef2918` virtual `0x90`
to the 15-byte adapter `0x142166670`, which tailcalls the existing FString reader
`0x141e15db0` with the same archive and destination. No intervening tag, version
gate or post processing appears in the adapter. The frozen string research and
fresh byte checks establish this concrete link without reading supplied values.

Successful demand is four count bytes plus the declared narrow byte count or
twice the declared wide-unit magnitude; zero and magnitude-one cases retain
their distinct consumption. Original normalization, unit-based archive limits,
transfer failure and the lack of enclosing EOF validation remain as documented
above. The native strict subset is not an exact original acceptance oracle.
Independent semantic and strict review: `work/str-value-contract-20261012/`.

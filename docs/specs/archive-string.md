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

The later exact C009 S01 Str comparison qualifies 486 supplied spans totaling
14,124 bytes against the native strict decoder and independent structural reader,
without reporting text. This validates strict structure and consumption, not
original normalization or round-trip identity:
`work/c009-own-leaf-values-20261012/`.

## Concrete name-property link

The exact C034 own Name descriptor OS01.F08 reaches the generation-20 factory,
final NameProperty vtable and value adapter, then the reviewed MemoryArchive's
Name virtual. Its loading branch delegates to the same FString reader before
constructing an internal name. This wrapper performs no separate serialized
numeric-index transfer; its eight-byte internal copy is not a wire-width claim.
Name interning and suffix semantics remain unqualified.

The signed-count transfer formulas, normalization and failure caveats above
therefore apply to this concrete path. Neither the Name wrapper nor FString
requires enclosing property EOF, and normal return can follow an archive error.
This static result reads no supplied names. Independent and strict review:
`work/c034-name-contract-20261012/`.

A separate bounded comparison qualifies 1,658 exact OS01.F08 value spans
(14,922 bytes) against the existing native strict decoder and independent
structural parser. Each copy contributes 829 nine-byte values. Encoding category,
unit counts and consumed length agree, with exact comparator EOF and unchanged
inputs. Prior top/own spans are reproduced before exact field selection. No
decoded names or name hashes are reported; internal name construction remains
unqualified. Six helper tests, independent review and execution evidence:
`work/c034-name-values-20261012/`.

## Concrete enum-backed Byte link

C009 PS01.F04 is an exact generation-zero Byte descriptor with an installed enum
pointer. Its final vtable dispatches to `0x142158d40`. Under the reviewed concrete
MemoryArchive, virtual `+0x68` returns false; with loading set and the enum present,
the adapter calls the same Name/FString path through virtual `+0x140`. Its raw
one-byte branch does not apply under those conditions. An optional subsequent
archive `+0x170` callback is return-only and adds no transfer.

Successful string transfer follows the formulas above. The adapter then resolves
the internal name against the enum, but lookup/fallback semantics, enumerator
values and range policy remain unqualified. It does not check archive error before
that resolution or establish enclosing EOF. This supports a future strict
structural comparison, not enum interpretation or original load success.
Independent static review: `work/c009-byte-value-contract-20261012/`.

The subsequent exact child-leaf comparison qualifies all 486 occurrences as
32-byte strict string frames with matching native/independent consumption.
Enum lookup was not performed. That run also qualifies the selected parent-child
Name and Str frames: `work/c009-parent-leaf-values-20261012/`.

## Concrete top-level Enum links

Both exact C009 top EnumProperty descriptors use generation 30 and final vtable
`0x144ecb940`, whose value slot calls `0x14210a290`. Descriptor-table membership,
factory recursion and the owner callback bind each to its preceding generation-zero
Byte helper. This proves the underlying-property link under successful construction;
adjacent descriptors or equal memory sizes alone would not suffice.

With the concrete archive's special-mode callback false, loading calls the same
Name/FString path, even if the enum pointer is null. Enum lookup remains separate.
The underlying Byte setter writes the result to memory without an additional
archive transfer; neither-loading-nor-saving uses a different underlying serializer
branch. The string formulas and normalization/error caveats above apply. No direct
version check or enclosing EOF validation appears in the examined Enum callback,
and lookup, fallback, truncation policy and supplied values remain unqualified.
Independent static review: `work/c009-top-enum-contract-20261012/`.

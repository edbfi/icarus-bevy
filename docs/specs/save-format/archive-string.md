# Archive string codec

## Status

Experimental: bounded independent codec with authored tests; original wire
round trip pending. Scope: build 25621760 (M-168) for the original reader, with
supplied-save comparisons on both copies. Last reconciled: 2026-10-13.
Full text: d5b0ae6:docs/specs/save-format/archive-string.md.

## Scope

The FString archive reader/writer, the strict native subset that models it, and
the property adapters shown to reach the same reader. Not covered: archive byte
order and framing (the caller establishes them), UDP packets, game admission,
and the meaning of any decoded value.

## Confirmed

- The reader/writer at `0x141e15db0` uses a signed 32-bit count: zero is empty,
  a positive count measures narrow bytes, a negative count measures UTF-16
  units, and nonzero counts include one terminating unit. `i32::MIN` is rejected
  before negation; a configured archive maximum bounds the absolute count; wide
  units may be byte-swapped by archive flags (F-282).
- The reader overwrites the final unit with zero instead of requiring a
  terminator, so a count of one yields an empty string. Narrow conversion at
  `0x1408a91f0` handles ASCII directly and replaces negative signed bytes with
  the caller's `?` fallback; it is not UTF-8 decoding. The wide reader inserts a
  zero for `U+FFFF` and may shorten the result; surrogate validation was not
  observed. The default writer selects narrow for ASCII and wide otherwise, a
  flag can force wide, and the narrow helper at `0x1409d1a30` has a replacement
  path (F-282).
- Successful demand is four count bytes plus the declared narrow byte count or
  twice the declared wide-unit magnitude; zero and magnitude-one cases keep
  their distinct consumption (F-285).
- Concrete adapters reaching this reader (F-285): StrProperty vtable
  `0x144ef2918` virtual `0x90` tailcalls through the 15-byte adapter
  `0x142166670`; the C034 NameProperty descriptor OS01.F08 (generation 20)
  delegates its loading branch to the same reader before internal name
  construction, with no separate numeric-index transfer; the C009 enum-backed
  Byte descriptor PS01.F04 (generation zero, vtable target `0x142158d40`) calls
  the Name/FString path through virtual `+0x140` when loading with an enum
  present, its raw one-byte branch not applying and an optional `+0x170`
  callback adding no transfer; both C009 top EnumProperty descriptors
  (generation 30, vtable `0x144ecb940`, value slot `0x14210a290`) call the same
  path even with a null enum pointer, and the underlying Byte setter adds no
  archive transfer.
- Supplied-save comparisons (structure and consumption only, no text): 486 C009
  S01 Str spans, 14,124 bytes (M-099); 1,658 C034 OS01.F08 Name spans, 14,922
  bytes, 829 nine-byte values per copy (M-108); 486 C009 PS01.F04 occurrences as
  32-byte strict frames plus the parent-child Name/Str frames (M-103).

## Candidate

- `experimental::archive::read_string_frame` validates both explicit byte orders
  without allocating on success, exposing prefix/payload/content/terminator
  spans, counts, encoding and consumed bytes with allocation-free UTF-16 unit
  iteration; `decode_string` materializes units into a `Vec<u16>` when ownership
  is needed. Neither converts invalid surrogates to a Rust `String`; following
  fields are untouched; empty output uses count zero; ASCII is preferred for
  writing; forced-wide output is not implemented (F-142).
- Research cap: 65,536 units including the terminator, checked before
  allocation or multiplication. The decoder checks the full declared range
  before output and rejects non-ASCII narrow bytes, missing terminators,
  embedded NUL and `U+FFFF`. Error precedence: count/limit/payload, narrow
  ASCII, final terminator, embedded NUL, then `U+FFFF`. These are documented
  compatibility differences, not original-server fixes.
- Neither the Name wrapper nor FString requires enclosing property EOF, and a
  normal return can follow an archive error; enum lookup, fallback, enumerator
  values, range policy and name interning remain unqualified.

## Open questions

- Original normalization, EOF and error equivalence (U-026); name construction
  and identity semantics (U-033); archive flags and byte order from controlled
  stock-client traffic before promotion into negotiation.

## Evidence

- Original reader observations: M-169 (overnight E-021, E-085 to E-087).
  Adapter traces: M-091, M-107, M-104, M-105. Borrowed-frame migration
  snapshot and corpus oracle (10,752 cases agree with the prior decoder): M-056.
- Authored tests pack ASCII and supplementary-character fixtures in both byte
  orders, check every truncation boundary, preserve raw surrogates, leave a
  following field untouched, and exercise minimum/maximum counts and the
  allocation boundary. No original serialized string is copied into tests.

```bash
cargo test --locked --test archive --test archive_frame
```

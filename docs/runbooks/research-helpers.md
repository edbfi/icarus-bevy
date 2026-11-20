# Research helpers

Authored helpers for bounded reference analysis. Each opens no original file by
itself; a reviewed case supplies inputs, hashes and lifecycle guards. Authored
fixtures qualify no original entries.

## PE and PDB readers

The pure `tools/pe_candidates.cjs` module validates bounded PE64/x64 buffers and
collects explicitly pinned address patterns. It opens no files and confirms no
callers or instruction boundaries. Its authored and independent PE fixtures run
through prek/CI using `node --test tools/tests/pe_candidates*.test.cjs`.

The pure `tools/pdb_public_symbols.cjs` reader validates a bounded MSF7/DBI/PUB32
profile against explicit PE debug identity and section geometry. It uses a
caller-supplied bounded reader, opens no files, and exports only selected public
symbols: a fixed name allowlist and, optionally, the names at up to 32 exact
queried code addresses. `tools/pe_function_ranges.cjs` binds exception entries.
Neither proves ownership, instruction boundaries, whole functions or runtime
behavior. Their authored and independent fixtures run through prek/CI; target
use requires a separate authorized case with streaming input hashes and
resource/deadline guards. `tools/pdb_procedure_ranges.cjs` optionally retains up
to 32 unresolved TPI/IPI references.

`tools/pdb_selected_types.cjs` separates raw-record, header/signature and
declaration-entry profiles:

- `inspectSelectedRawTypeRecords` (M-1442) selects one numeric TPI index and
  validates outer framing; its leaf/raw envelope qualifies no payload semantics
  or references.
- `inspectSelectedDeclarationEntries` (M-1438) binds the complete owner index,
  header bytes/hash and FIELDLIST, following only bounded FIELDLIST chains.
  Cycles, shared lists and weighted-count mismatches refuse; other references
  stay opaque.
- Exported `PARAMETER_PROFILE` reads bounded parameter/base/embedded
  declarations; `DECLARATION_PROFILE` reports UDT identity/size, declarations,
  counts and direct references. Neither gives a layout, runtime
  ancestry/consumers, net order, checksums, activation or compatibility
  qualification.

Declared sizes/offsets prove no storage layout; legacy member/overlap checks
remain strict. Callers bind input identity, physical reads, confinement and
provenance.

`node tools/pdb_selected_metadata.cjs --request request.json`: the JSON request
binds PDB path/size/SHA-256, GUID/age, profile/names and output path. Exclusive
mode-0600 publication follows complete writes and sync; failures are redacted.
Caps only lower limits.

`tools/native_field_catalog.cjs`: call `joinNativeFieldCatalog` with three
reader reports, exported `PROFILE` and exact `options.addresses`; limits only
lower `CAPS`. Output stays `ready: false`.

## Package metadata

`tools/pak_protocol_metadata.cjs` admits only the pinned package's exact
gameplay-tag configuration member. The v11 profile verifies whole-file SHA-256,
index and member SHA-1, sizes and EOF; it supports uncompressed or single-block
Zlib input and rejects unsupported layouts, encryption and nonregular files.
Authored fixtures run through prek/CI. Run it only inside a fresh scoped case:

```bash
node tools/pak_protocol_metadata.cjs --path "$PAK_PATH" --sha256 "$PAK_SHA256" > "$CASE_DIR/declared-profile-private.json"
```

The output contains derived tag declarations and settings; retain it privately.
It discards comments and external asset paths, counts unresolved sources and
redirects, and always marks runtime settings and the final mapping unverified.
`ascii_case_variant_count` counts extra distinct spellings under ASCII case
folding; exact spellings are preserved without implicit conversion. The
declaration reader accepts source-config widths through 32 bits for
observation; the native dictionary separately requires first-segment widths
0–16 and container widths 1–8, so config output is not directly a runtime
dictionary. It never follows references or extracts other member bodies.
REA's current artifact route rejects this format (M-709); the bounded
fallback's qualification is M-722 (earlier reviewed source: M-719). Passing
config inspection does not qualify a complete schema.

## Strings and Ghidra windows

The experimental [archive-string subset](../specs/save-format/archive-string.md)
validates initial control strings on borrowed spans, without temporary
decoded-unit allocation; archive byte order and enclosing message boundaries
remain the caller's responsibility.

`tools/ghidra/ExportBoundedInstructionOperands.java` exports only numeric operands,
raw p-code and instruction hashes for two fixed, pinned-executable windows. It
requests no decompiler, analyzers, function creation or outgoing flow. Target use
requires a reviewed case, source-only script directory, disposable retained-project
copy and the offline deadline/resource/ownership guards; fixture and compiler
outputs must stay outside that script directory. See the
[control investigation](../research/archive/2026-10-12-control-caller-limit.md).

`tools/ghidra/DecodeConstructorWindows.java` is the independently authored minimal
x64 decoder used in the reviewed current-client prefix case. It decodes two fixed
derived byte windows without creating a Program/database or importing an executable.
It requires Ghidra 12.1.4, exact input sizes/hashes and separately reviewed original
slice provenance, scope and lifecycle guards. Nine real authored decoder recipes
and five injected API guards pass. Case wrappers enforce authorization; invoking
this helper alone does not establish input provenance or authorize target reads.

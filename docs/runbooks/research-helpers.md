# Research helpers

These helpers use caller-supplied inputs. Target use requires a reviewed case
that binds provenance, input hashes, reads, confinement and lifecycle guards.
Authored fixtures qualify only the helpers.

## PE and PDB readers

- `tools/pe_candidates.cjs` validates bounded PE64/x64 buffers and collects
  pinned address patterns. Caller and instruction-boundary checks are separate.
  Run authored and independent fixtures with
  `node --test tools/tests/pe_candidates*.test.cjs`.
- `tools/pdb_public_symbols.cjs` binds MSF7/DBI/PUB32 to PE debug identity and
  section geometry. A bounded reader supplies data. Output includes allowlisted
  public names and names at up to 32 exact queried code addresses.
- `tools/pe_function_ranges.cjs` binds exception entries. These helpers leave
  ownership, instruction boundaries, whole functions and runtime behavior
  unqualified. Their authored and independent fixtures run through prek/CI.
- `tools/pdb_procedure_ranges.cjs` can retain up to 32 unresolved TPI/IPI
  references.

`tools/pdb_selected_types.cjs` has three profiles.

- `inspectSelectedRawTypeRecords` (M-1442) selects one numeric TPI index and
  validates outer framing. The leaf and raw bytes leave payload semantics and
  references opaque.
- `inspectSelectedDeclarationEntries` (M-1438) binds the owner index, header
  bytes/hash and FIELDLIST. Only bounded FIELDLIST chains are followed. Cycles,
  shared lists and weighted-count mismatches refuse.
- `PARAMETER_PROFILE` reports parameter/base/embedded declarations.
  `DECLARATION_PROFILE` reports UDT identity/size, declarations, counts and
  direct references. Layout, runtime ancestry/consumers, order, checksums,
  activation and compatibility require separate evidence. Declared sizes and
  offsets leave storage layout unverified. Legacy member/overlap checks apply.

Run `node tools/pdb_selected_metadata.cjs --request request.json` with PDB
path/size/SHA-256, GUID/age, profile/names and output path. Publication is
exclusive, mode 0600, and follows complete writes and sync. Failures are
redacted. User caps can only lower limits.

`joinNativeFieldCatalog` in `tools/native_field_catalog.cjs` takes three reader
reports, `PROFILE` and exact `options.addresses`. Limits can only lower
`CAPS`. Output stays `ready: false`.

## Package metadata

`tools/pak_protocol_metadata.cjs` admits only the pinned gameplay-tag
configuration member. The v11 profile checks file SHA-256, index/member SHA-1,
sizes and EOF. It accepts uncompressed or single-block Zlib regular files and
refuses encryption. Fixtures run through prek/CI.

```bash
node tools/pak_protocol_metadata.cjs --path "$PAK_PATH" --sha256 "$PAK_SHA256" > "$CASE_DIR/declared-profile-private.json"
```

Retain derived tag declarations and settings privately. Output drops comments
and external asset paths, counts unresolved sources and redirects, preserves
exact spellings and counts extra spellings under ASCII case folding in
`ascii_case_variant_count`. Runtime settings and final mapping stay unverified.
The reader accepts declared widths through 32 bits. The native dictionary
requires first-segment widths 0–16 and container widths 1–8. Other members and
references stay unopened. REA rejects this format (M-709); see M-722 for the
bounded fallback and M-719 for the earlier source.

## Strings and Ghidra windows

The [archive-string subset](../specs/save-format/archive-string.md) validates
initial control strings on borrowed spans without allocating decoded units.
Callers supply byte order and enclosing boundaries.

`tools/ghidra/ExportBoundedInstructionOperands.java` emits numeric operands,
raw p-code and hashes from two fixed pinned windows without analysis,
decompilation, function creation or outgoing flow. Use a disposable retained
project, a source-only script directory, and reviewed offline guards. Keep
fixtures and compiler outputs outside the script directory.
[Control investigation](../research/archive/2026-10-12-control-caller-limit.md).

`tools/ghidra/DecodeConstructorWindows.java` independently decodes two fixed
x64 byte windows without creating a Ghidra database or importing an executable.
Use Ghidra 12.1.4 and bind exact input sizes and hashes. Nine authored decoder
recipes and five injected API guards pass. Case wrappers enforce authorization
and bind original slice provenance and lifecycle.

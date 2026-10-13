# Research helpers

Authored helpers for bounded reference analysis. Each opens no original file by
itself; a reviewed case supplies inputs, hashes and lifecycle guards.

The pure `tools/pe_candidates.cjs` module validates bounded PE64/x64 buffers and
collects explicitly pinned address patterns. It opens no files and confirms no
callers or instruction boundaries. Its authored and independent PE fixtures run
through prek/CI using `node --test tools/tests/pe_candidates*.test.cjs`.

The pure `tools/pdb_public_symbols.cjs` reader validates a bounded MSF7/DBI/PUB32
profile against explicit PE debug identity and section geometry. It uses a
caller-supplied bounded reader, opens no files, and exports only selected public
symbols.
Its authored and independent fixtures run through prek/CI. Target use requires a
separate authorized case with streaming input hashes and resource/deadline guards;
symbol names alone establish neither instruction boundaries nor runtime behavior.

The experimental [archive-string subset](../specs/archive-string.md) offers a
borrowed strict frame validator and an owning decoder. Initial-control validation
uses borrowed spans without temporary decoded-unit allocation. Archive byte order
and enclosing message boundaries remain the caller's responsibility.

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

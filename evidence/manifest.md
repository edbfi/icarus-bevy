# Evidence manifest

One row per retained artifact. Class `local-private` means the file lives under
ignored `work/` or `reference-data/` on the analyst's machine and is never
deleted by cleanup; `external` means it is held outside this repository.
`python3 tools/docs_lint.py --local` verifies local-private digests.

| ID | Path | SHA-256 | Date | Producing session | Class |
| --- | --- | --- | --- | --- | --- |

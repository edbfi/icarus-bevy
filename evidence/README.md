# Evidence index

One JSON record per line per retained artifact, sharded by findings area
(`foundation`, `network`, `save-format`, `save-format-c001`,
`save-format-classes`; `<area>-2.jsonl` after a rollover). Rules:
[`.agents/rules/docs.md`](../.agents/rules/docs.md). Fetch one row; never load a
shard:

```bash
grep -hw M-147 evidence/*.jsonl
python3 tools/docs_lint.py --next-id M
```

| Key | Value |
| --- | --- |
| `id` | `M-NNN`, permanent |
| `path` | repository-relative path, anchor, or external locator |
| `sha256` | digest of the file's bytes, or null (legacy only) |
| `class` | `local-private` (ignored path on the analyst machine), `tracked` (in Git), `external` |
| `kind` | `rea-bundle`, `receipt`, `review`, `input`, `build`, `report`, `revision` |
| `date`, `label` | ISO date; artifact label of at most 160 characters |
| `status` | `active`, `retired` or `missing` (needs `note` or `superseded_by`) |
| `ev`, `unk`, `records` | `rea-bundle` only: cited Evidence and unknown IDs, record count |
| `superseded_by`, `note`, `legacy` | optional; `legacy` marks M-001..M-270 |

Prose cites `M-NNN`, `M-NNN/ev_<12 hex>`, `M-NNN/unk_<12 hex>` or, for
pre-REA case notes, `M-NNN/E-NNN`.

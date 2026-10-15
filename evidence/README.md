# Evidence index

One JSON record per line per retained artifact. Rules:
[`.agents/rules/docs.md`](../.agents/rules/docs.md). Category folders mirror the
findings ledgers: a folder path joined with `-` names its ledger, active or
retired (`save-format/c001/` holds the evidence of `save-format-c001.md`). Each
folder holds `001.jsonl`, `002.jsonl`, ... with the highest open; its shards hold
only its own rows, not those of its subfolders. Fetch one row, never a shard:

```bash
grep -rhw --include='*.jsonl' M-NNN evidence/
python3 tools/docs_lint.py --next-id M
```

| Key | Value |
| --- | --- |
| `id` | `M-NNN`, permanent |
| `path` | repository-relative path, anchor, or external locator |
| `sha256` | digest of the file's bytes, or null (legacy only) |
| `class` | `local-private`, `tracked` or `external` |
| `kind` | `rea-bundle`, `receipt`, `review`, `input`, `build`, `report`, `revision` |
| `date`, `label` | ISO date; label of at most 160 characters |
| `status` | `active`, `retired` or `missing` (needs `note` or `superseded_by`) |
| `ev`, `unk`, `records` | `rea-bundle` only: cited Evidence and unknown IDs, record count |
| `superseded_by`, `note`, `legacy` | optional; `legacy` marks M-001..M-270 |

A row whose `path` ends in `fixity.sha256` pins every file that manifest lists
(`.agents/rules/work.md`).

Prose cites `M-NNN`, `M-NNN/ev_<12 hex>`, `M-NNN/unk_<12 hex>` or, for
pre-REA case notes, `M-NNN/E-NNN`.

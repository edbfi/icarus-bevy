# Evidence catalog

Use fresh output paths. From the repository root:

```sh
python3 -m tools.evidence_catalog migrate --rev "$rev" --out "$case/catalog"
python3 -m tools.evidence_catalog verify --catalog "$case/catalog"
python3 -m tools.evidence_catalog render --catalog "$case/catalog" --out "$case/views"
python3 -m tools.evidence_catalog lookup --catalog "$case/catalog" F-750
```

Use `lookup --rev HEAD` for current Git and `M-NNN/unk_<12 hex>` for a listed
member. Use `render --topic` to select topics.

For managed topics see the [authoring rules](../../.agents/rules/docs.md#catalog-authoring).
Compute the review digest with `request-id "$request"`. After approval:

```sh
python3 -m tools.evidence_catalog publish "$request" --approved-sha256 "$approved" --out "$case/publication"
git apply --index "$case/publication/change.patch"
python3 -m tools.evidence_catalog check
python3 tools/docs_lint.py
```

[Optional Hindsight recall](hindsight.md).

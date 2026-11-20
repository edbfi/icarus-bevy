# Evidence catalog

From the repo root, use fresh output paths.

```sh
catalog() { python3 -m tools.evidence_catalog "$@"; }
catalog migrate --rev "$rev" --out "$case/catalog"
catalog verify --catalog "$case/catalog"
catalog render --catalog "$case/catalog" --out "$case/views"
catalog lookup --catalog "$case/catalog" F-750
```

Use `lookup --rev HEAD` for current Git, `M-NNN/unk_<12 hex>` for a listed
member and `render --topic` to select topics.

See [authoring rules](../../.agents/rules/docs.md#catalog-authoring) for managed
topics. Compute the digest with `catalog request-id "$request"`. After approval:

```sh
catalog publish "$request" --approved-sha256 "$approved" --out "$case/publication"
git apply --index "$case/publication/change.patch"
catalog check
python3 tools/docs_lint.py
```

Pull request CI uses `check-history` to replay transitions at the PR boundary.
Other CI events check generated view parity.

[Optional Hindsight recall](hindsight.md).

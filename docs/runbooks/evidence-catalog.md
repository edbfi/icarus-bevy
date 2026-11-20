# Evidence catalog

Use a fresh output directory for each command. From the repository root:

```sh
python3 -m tools.evidence_catalog migrate --rev "$rev" --out "$case/catalog"
python3 -m tools.evidence_catalog verify --catalog "$case/catalog"
python3 -m tools.evidence_catalog render --catalog "$case/catalog" --out "$case/views"
python3 -m tools.evidence_catalog lookup --catalog "$case/catalog" F-750
```

Use `lookup --rev HEAD` for the current tree or pass `M-NNN/unk_<12 hex>`
for an exact listed member. Add `--topic` to render selected topics.

For topics in the authority manifest, prepare a typed request under the
[authoring rules](../../.agents/rules/docs.md#catalog-authoring). Use
`request-id "$request"` to get its review digest. After approval:

```sh
python3 -m tools.evidence_catalog publish "$request" --approved-sha256 "$approved" --out "$case/publication"
git apply --index "$case/publication/change.patch"
python3 -m tools.evidence_catalog check
python3 tools/docs_lint.py
```

Use the [Hindsight runbook](hindsight.md) for optional semantic recall over a
reviewed projection of catalog rows.

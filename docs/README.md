# Documentation map

Start here and [implementation status](implementation-status.md). Fetch ledger
rows by ID. [Documentation rules](../.agents/rules/docs.md) define read tiers
and caps. Run `python3 tools/docs_lint.py` to check them.

- [Plan](linux-dedicated-server-plan.md), [gates](gates.md) and
  [architecture](architecture.md).
- [Runbooks](runbooks/) for operations, including
  [survival](runbooks/authored-survival.md),
  [controller receive](runbooks/controller-receive.md),
  [join passwords](runbooks/join-password.md) and
  [evidence catalog](runbooks/evidence-catalog.md).
- [Specs](specs/) for durable format and protocol facts.
- [Research](research/) for current answers by question cluster.
- [Findings](ledgers/findings/) for active and retired claims.
  [Other ledgers](ledgers/) track unknowns, bugs and compatibility.
- [Evidence index](../evidence/README.md) for artifact rows and selectors.
  Use `grep -rhw --include='*.jsonl' M-ID evidence/` to fetch a row.
- [Research archive](research/archive/) and [history](history/) for frozen
  reports and historical authorizations.

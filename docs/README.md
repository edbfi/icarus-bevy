# Documentation map

Rules: [`.agents/rules/docs.md`](../.agents/rules/docs.md). Lint: `python3 tools/docs_lint.py`.

| Path | Role | Bound |
| --- | --- | --- |
| [linux-dedicated-server-plan.md](linux-dedicated-server-plan.md) | goal, milestones, exit criteria | 16 KB |
| [implementation-status.md](implementation-status.md) | what works, what does not, next tasks | 16 KB, fixed headings |
| [gates.md](gates.md) | preconditions that block actions, grouped by action | 16 KB |
| [architecture.md](architecture.md) | components and interfaces | 16 KB |
| [runbooks/](runbooks/) | operating procedures | 16 KB each |
| [specs/](specs/) | durable findings per format or protocol (`save-format/`, `network/`) | 16 KB each, fixed headings |
| [ledgers/](ledgers/) | findings, unknowns, bugs, compatibility; one row per item | 64 KB each, findings 128 KB |
| [research/](research/) | digests by area (`save-format/`, `network/`, `foundation/`), one per question cluster | 16 KB each, fixed headings |
| [research/archive/](research/archive/) | dated reports, verbatim, frozen | none |
| [history/](history/) | historical authorizations, verbatim, frozen | none |
| [../evidence/manifest.md](../evidence/manifest.md) | evidence artifacts: path, digest, session, class | none |

The current prose surface (everything except archive, history, the ledgers and
the manifest) is capped at 160 KB in total; ledgers carry their own caps.

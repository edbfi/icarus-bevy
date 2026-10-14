# Documentation map

Rules, read tiers and caps: [`.agents/rules/docs.md`](../.agents/rules/docs.md).
Lint: `python3 tools/docs_lint.py`. Startup reads stop at this map and
[status](implementation-status.md); fetch ledger rows by ID.

| Path | Role | Bound |
| --- | --- | --- |
| [linux-dedicated-server-plan.md](linux-dedicated-server-plan.md) | goal, constraints, milestones, exit criteria, release targets | 16 KB |
| [implementation-status.md](implementation-status.md) | summary, milestone state, next tasks | 16 KB, fixed headings |
| [gates.md](gates.md) | preconditions that block actions (only home) | 16 KB |
| [architecture.md](architecture.md) | design constraints, repository boundaries, toolchain | 16 KB |
| [runbooks/](runbooks/) | operating procedures | 16 KB each |
| [specs/](specs/) | durable format and protocol facts (`save-format/`, `network/`) | 16 KB each, fixed headings |
| [research/](research/) | one digest per question cluster (`save-format/`, `network/`, `foundation/`) | 8 KB each, fixed headings |
| [ledgers/findings/](ledgers/findings/) | active findings by area; `retired/` keeps retired rows in full | 48 KB per active file, 96 KB active total, 64 KB per retired file |
| [ledgers/](ledgers/) | unknowns, bugs, compatibility; one row per item | 64 KB each; 400-byte rows |
| [research/archive/](research/archive/) | dated reports, verbatim, frozen | none |
| [history/](history/) | historical authorizations, verbatim, frozen | none |
| [../evidence/](../evidence/README.md) | evidence index, one JSONL record per artifact; category folders mirror the findings ledgers; fetch by `grep -rhw --include='*.jsonl' M-ID evidence/` | 64 KB per shard, 2 KB records |

The current prose surface (root `README.md` and everything here except archive,
history and the ledgers, plus `evidence/README.md`) is capped at 160 KB in total.

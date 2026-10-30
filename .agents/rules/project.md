# Project purpose

Build a native Linux dedicated-server replacement for Icarus using Rust and
Bevy, with stability and performance as project goals. Reverse engineering of
the original server informs compatibility; compatibility and performance must
be demonstrated rather than assumed.

# Project-local skills and tools

- Use `.agents/skills/bevy/SKILL.md` for Bevy development.
- The Bevy skill is installed from `edbfi/skills` using `bunx skills`; keep
  `skills-lock.json` with updates. The application targets Bevy 0.20.1;
  the retained skill's API baseline is Bevy 0.20.0.
- Target the latest stable project dependencies and Rust toolchain, as requested
  by the user. Verify current upstream releases, migrate APIs and run checks when
  updating; retain exact toolchain/release pins and the lockfile for reproducible
  builds. Handle routine stable upgrades autonomously. Record concrete
  compatibility blockers instead of silently retaining an obsolete baseline.
- For all reverse-engineering work, read and follow
  `.agents/skills/reverse-engineer-anything/SKILL.md` and use the `rea` MCP server
  as the primary analysis interface. This includes binary and package inspection,
  decompilation, protocol recovery, runtime observation and version comparison.
  `docs/runbooks/rea-tooling.md` restores the runtime and names the three
  reference environments.
  Use the connected server's advertised tools and schemas. If MCP is unavailable,
  use `.agents/rea/rea.sh` for supported CLI operations; use direct analysis tools
  only for capabilities REA lacks or a diagnosed provider/connection limitation,
  and record the reason. Do not fall back to the removed skill package.
  REA is project-local: `.agents/rea/` pins `rea-agents@6.8.0`, and
  `.agents/rea/rea.sh` launches the installed runtime. Codex uses
  `.codex/config.toml`; Claude uses `.mcp.json` and the skill link under
  `.claude/skills/`. Do not run upstream global setup or alter user-wide settings.
- Prefer Homebrew for tool installation, as requested by the user. `Brewfile`
  records the initial binary and protocol analysis tools. Other package
  managers are fallbacks when a needed tool is unavailable through Homebrew.
- Check REA's installed provider from the project root with
  `.agents/rea/rea.sh doctor --provider ghidra --json`. Use the installed
  `Brewfile` tools and check `tshark --version` separately when needed.
- Ghidra drift. Ghidra and REA both update often; never pin a Ghidra release in
  the repository. Each REA release accepts one Ghidra line, named in doctor's
  `ghidra-version` detail. When doctor or a provider reports
  `unsupported_version` (usually Homebrew upgraded Ghidra), resolve it in order
  and record what you did:
  1. Check REA's releases and issues (`gh release list -R morluto/rea`). If a
     newer REA admits the installed Ghidra, update REA as its own change
     (package lock, `upstream.json`, skill snapshot, licenses).
  2. Otherwise install the release doctor names from the official
     `NationalSecurityAgency/ghidra` GitHub release, after checking the SHA-256
     published there, under `~/.local/share/rea-ghidra/`, and point its
     `current` link at it. `rea.sh` prefers `current` over Homebrew and picks
     the Homebrew `openjdk@N` that the selected Ghidra's `application.java.min`
     names; install that JDK with Homebrew if it is missing.
  3. Rerun doctor, restart the client (the MCP server reads its environment at
     launch) and query `binary_session`. Remove `current` once Homebrew's
     Ghidra is accepted again.
  Never downgrade Homebrew's Ghidra, relabel an installation, patch REA's
  policy or force an override past the gate. For other doctor failures follow
  its `remediation`; Homebrew's Linux Ghidra 12.2 also needed a matching native
  decompiler (M-975), so check that the chosen install has one for this host.
- When a direct Ghidra fallback is needed, run it headlessly with the Ghidra and
  JDK that `rea.sh` selects (the `current` link, else Homebrew's).
- Keep original server files under the ignored `icarus-orig/` directory and
  generated analysis artifacts under the ignored `work/` directory. Keep
  reusable source code and documentation outside those ignored directories.
- Preserve private supplied save archives and their inventories under ignored
  `reference-data/`; use disposable copies under `work/` for experiments.
- Preserve the REA package lock, skill provenance and third-party licenses when
  updating. Retain existing research scopes, fixity receipts and evidence;
  replacing tooling does not resume paused research or extend runtime windows.

# Input parity

- The native server may need no more than the official dedicated server needs
  (user decision, F-828). Protocol identities may come from the free, public dedicated-server
  distribution, fetched unattended at install time by our own fetcher;
  it must never require the game client or anything derived only from it.
  Do not extract or reuse packaged gameplay tables; author behavior and world
  data independently. Prefer removing even the identity dependency through reverse engineering or
  independently authored data, toward a fully standalone binary. The client
  stays a research cross-check only.

# Research scope

- Tool installation does not authorize a target experiment. Before new target
  work, record assets, activities, authorization evidence, exclusions, network
  profile and a bounded execution window under `work/`, and check the actual
  command against them.
- Separate offline and dynamic scopes; lab-only scopes permit no external service
  connections. Preserve frozen case artifacts and receipts; adapt later probes
  with fresh reviewed bindings.
- A session closes as `.agents/rules/docs.md` requires; preconditions that block
  actions live only in `docs/gates.md`.

# Execution and handoff

- Work autonomously through the current project plan. Resolve routine choices,
  fixes, tests and reversible local work without repeated permission questions.
  When a task needs missing input or authorization, continue independent work and
  report the precise dependency; never manufacture evidence or approval.
- Use project skills for the actual task. The user permits a suitable number of
  subagents; the lead picks provider, model and effort from what the session
  offers, strongest for hard reasoning, lighter for scoped work, never assuming
  a vendor. Whenever parallel delegation could save time or improve quality,
  use collaboration tools, whether acting as the root agent or a subagent. Give
  agents explicit ownership; the lead integrates, validates, commits and pushes.
  Bound aggregate resources.
- Commit coherent tested increments frequently using Conventional Commits and
  `git commit -s`; push the feature branch after each completed increment or small
  related batch. Preserve history, keep hooks enabled, and verify remote delivery.
  The 2026-10-12 instruction supersedes the old overnight batch's no-push rule.
  Do not force-push, rewrite existing commits or merge into `main` as routine sync.
- Maintain `docs/linux-dedicated-server-plan.md` as the canonical plan,
  `docs/implementation-status.md` as the current checkpoint and `docs/gates.md`
  as the only home of preconditions. Record findings by area under
  `docs/ledgers/findings/`, open questions, bugs and compatibility in the other
  ledgers, research state in digests under `docs/research/`, and evidence
  artifacts in the `evidence/` index, following `.agents/rules/docs.md`. Avoid
  duplicate plans, generic AI prose and speculative completion claims.
- Resource budgets for heavy work: start with two Cargo jobs and at most four
  CPU-heavy workers; serialize large builds, Ghidra and original-server runs when
  memory is tight. Keep owned jobs under 10 GiB; defer heavy work below 3 GiB
  available RAM or 30 GiB free disk. Cap new case artifacts at 12 GiB and total
  batch disk growth at 20 GiB, then reassess. Targeted Ghidra runs use a 4 GiB
  heap, two CPUs, at most 24 selected functions, 30 seconds per decompilation and
  15 minutes overall. Bound every experiment and clean it up on timeout.
- Shared Cargo targets can reuse another checkout's executable despite a fresh
  event naming the current manifest (M-1069). Force the library and selected
  test to compile on checkout transitions, verify embedded manifest identity
  and distinct comparison artifacts; preserve frozen inputs and existing caches.
- At milestones, reconcile stale documentation and clean reproducible scratch
  from owned work as `.agents/rules/work.md` directs. Preserve original inputs,
  private saves/backups, evidence, provenance, licenses and user changes.
  Check references before deleting stale tracked files; mark historical
  reports clearly instead of rewriting observations. Never use blanket
  `git clean -fdx`, global Docker prune or deletion by age alone.

# Checks

- Whenever writing, editing or reviewing project-authored `.py` files, use the
  `basedpyright-python-gate` skill. Its scope is our code under `src`, `tests` and
  `tools`; the user explicitly excludes `.agents/` and skills. Keep authored
  Python in those checked roots or add its new root to the include list.
- Python targets stable 3.15.0. Run `python3 tools/typing_gate.py --diff-ignores`,
  `uvx basedpyright@latest --warnings`, and the authored Python tests. Require
  zero errors and zero warnings in `recommended` mode. Fix types at input
  boundaries; do not add global overrides, baselines or blanket ignores. The
  basedpyright table permits only `pythonVersion`, `typeCheckingMode` and
  `include`. The same config gate and checker run through prek and CI.
- Run `prek install` after cloning to install pre-commit, commit-msg, and
  pre-push hooks. Direct commits to `main` are blocked by the shared baseline.
- Run `prek run --all-files --hook-stage manual` for the full check suite.
- Rust formatting and Clippy run before commits; Rust tests run before pushes
  and in the manual suite. These hooks activate when matching Rust/Cargo files
  exist. Commit `Cargo.lock` once the Cargo project is initialized; Clippy and
  tests use `--locked`.
- Vendored skills and generated analysis artifacts are excluded from file
  checks. Gitleaks independently scans the staged Git diff for secrets.

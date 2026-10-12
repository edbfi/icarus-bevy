# Project purpose

Build a native Linux dedicated-server replacement for Icarus using Rust and
Bevy, with stability and performance as project goals. Reverse engineering of
the original server informs compatibility; compatibility and performance must
be demonstrated rather than assumed.

# Project-local skills and tools

- Use `.agents/skills/bevy/SKILL.md` for Bevy development.
- The Bevy skill is installed from `edbfi/skills` using `bunx skills`; keep
  `skills-lock.json` with updates. The application and skill target Bevy 0.20.0.
- Target the latest stable project dependencies and Rust toolchain, as requested
  by the user. Verify current upstream releases, migrate APIs and run checks when
  updating; retain exact toolchain/release pins and the lockfile for reproducible
  builds. Handle routine stable upgrades autonomously. Record concrete
  compatibility blockers instead of silently retaining an obsolete baseline.
- The reverse-skill package is vendored at `.agents/vendor/reverse-skill/`.
  `.agents/skills/reverse-skill-router` links to its `skills/` directory for
  discovery. Use the canonical vendor path for scripts so package-relative
  paths resolve correctly.
- For reverse-engineering tasks, enter through the package's
  `skills/SKILL.md`, `skills/MASTER-ROUTING.md`, and `skills/routing.md`.
  Consult `skills/tool-index.md` for this machine's tool inventory.
- Prefer Homebrew for tool installation, as requested by the user. `Brewfile`
  records the initial binary and protocol analysis tools. Other package
  managers are fallbacks when a needed tool is unavailable through Homebrew.
- Refresh the machine-local inventory from the project root with
  `PATH="$(brew --prefix openjdk@21)/bin:$PATH" bash .agents/vendor/reverse-skill/skills/scripts/refresh-tool-index.sh`.
  Upstream probes the Ghidra GUI for its version and does not index Wireshark
  utilities. Verify Ghidra headlessly and check `tshark --version` separately;
  a GUI launch error on this VPS does not mean headless Ghidra is broken.
- Run Ghidra headlessly with
  `JAVA_HOME="$(brew --prefix openjdk@21)/libexec" "$(brew --prefix ghidra)/libexec/support/analyzeHeadless" <arguments>`.
- Keep original server files under the ignored `icarus-orig/` directory and
  generated analysis artifacts under the ignored `work/` directory. Keep
  reusable source code and documentation outside those ignored directories.
- Preserve private supplied save archives and their inventories under ignored
  `reference-data/`; use disposable copies under `work/` for experiments.
- `.agents/vendor/reverse-skill.upstream.json` records the upstream revision.
  Preserve the upstream package layout and licenses when updating it.

# Execution and handoff

- Work autonomously through the current project plan. Resolve routine choices,
  fixes, tests and reversible local work without repeated permission questions.
  When a task needs missing input or authorization, continue independent work and
  report the precise dependency; never manufacture evidence or approval.
- Use project skills for the actual task. The user permits a suitable number of
  subagents and model/effort selection: GPT-6 Astra for difficult reasoning and
  GPT-6.1 Sol for scoped implementation or review. Whenever parallel delegation
  could save time or improve quality, use collaboration tools, whether acting as
  the root agent or a subagent. Give agents explicit ownership;
  the lead integrates, validates, commits and pushes. Bound aggregate resources.
- Commit coherent tested increments frequently using Conventional Commits and
  `git commit -s`; push the feature branch after each completed increment or small
  related batch. Preserve history, keep hooks enabled, and verify remote delivery.
  The 2026-10-12 instruction supersedes the old overnight batch's no-push rule.
  Do not force-push, rewrite existing commits or merge into `main` as routine sync.
- Maintain `docs/linux-dedicated-server-plan.md` as the canonical plan and
  `docs/implementation-status.md` as the current checkpoint. Update the existing
  bug ledger, compatibility matrix and specifications as evidence changes.
  Avoid duplicate plans, generic AI prose and speculative completion claims.
- At milestones, reconcile stale documentation and clean reproducible scratch
  from owned work. Preserve original inputs, private saves/backups, evidence,
  provenance, licenses and user changes. Check references before deleting stale
  tracked files; mark historical reports clearly instead of rewriting observations.
  Never use blanket `git clean -fdx`, global Docker prune or deletion by age alone.

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

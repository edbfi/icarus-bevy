# Project purpose

Build a native Linux dedicated-server replacement for Icarus using Rust and
Bevy, with stability and performance as project goals. Reverse engineering of
the original server informs compatibility; compatibility and performance must
be demonstrated rather than assumed.

# Project-local skills and tools

- Use `.agents/skills/bevy/SKILL.md` for Bevy development.
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
- `.agents/vendor/reverse-skill.upstream.json` records the upstream revision.
  Preserve the upstream package layout and licenses when updating it.

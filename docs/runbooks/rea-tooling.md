# REA tooling and reference environments

How to restore and check the project-local reverse-engineering runtime and the
three reference environments. Rules for when to use REA are in
[`.agents/rules/project.md`](../../.agents/rules/project.md).

## REA runtime

The project uses [REA](https://github.com/morluto/rea), pinned as
`rea-agents@6.4.0` in `.agents/rea/package-lock.json`. Its wrapper and MCP
configuration are local to this repository: Codex reads `.codex/config.toml`,
Claude reads `.mcp.json`, and both discover the same skill through
`.agents/skills/reverse-engineer-anything/` and its `.claude/skills/` link.
Restore the locked runtime and check the Ghidra provider from the repository root:

```bash
npm ci --prefix .agents/rea --cache .agents/rea/.npm-cache
.agents/rea/rea.sh doctor --provider ghidra --json
```

Restart Codex and Claude after configuration changes and accept their project/MCP
trust prompts when presented. Do not run upstream global setup; it would change
configuration outside this repository. Preserve the package lock, skill
provenance and third-party licenses when updating.

The scoped Ghidra doctor check reports provider readiness. Its informational
checks inspect user-wide registrations and may report them missing because this
installation deliberately uses project configuration. Verify registration with
`codex mcp get rea` and `claude mcp get rea`; verify the live connection with the
MCP `binary_session` tool. Installation validation discovered 170 MCP tools from
REA 6.4.0 and confirmed the existing Ghidra 12.1.4/JDK 21 provider as healthy.

Use the installed Homebrew tools recorded in `Brewfile`: radare2, headless Ghidra
and Wireshark CLI utilities. Check `tshark --version` separately. A GUI failure
does not mean headless Ghidra is broken. For a justified direct Ghidra fallback,
resolve the arguments before presenting an analysis command:

```bash
JAVA_HOME="$(brew --prefix openjdk@21)/libexec" \
  "$(brew --prefix ghidra)/libexec/support/analyzeHeadless" <arguments>
```

## Reference environments

Maintain three environments:

1. Official server under GE-Proton using the user's Pelican reference at
   `/home/gvps/github/edbfi/pelican-eggs/games-steamcmd/icarus-proton`.
2. Official Windows server using the user's home machine for behavior and fault attribution.
3. Native replacement on Linux with matched workloads and resource limits.

The inspected Pelican egg uses `xvfb-run` and Proton, installation app ID 2089300,
runtime game ID 1149460, and persistent data under `Icarus/Saved`. Pin container
digest, Proton version, official build and configuration; disable automatic
updates in experiments. Do not run its container installer unchanged on the host.

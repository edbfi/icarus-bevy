# Offline Proton startup batch

Status: pending. This is the first proposed execution of the original server.
It cannot prove stock-client compatibility because the container has no network.

Docker 29.8.2 is available. The user's Proton image is already installed at digest
`sha256:967e40780cb46a5343bd7a5fd372612d1a1b90fbc95fdd803d74ec5b3bede237`.
The image runs as `container` with its existing home at `/home/container`.

## Commands proposed for approval

```bash
bash .agents/vendor/reverse-skill/skills/scripts/case-init.sh \
  --hint "game reverse ICARUS offline startup" \
  --case-name icarus-offline-startup --project-root "$PWD" \
  --auth-granted --auth-basis own_system \
  --evidence-of-auth "User approval of docs/offline-runtime-authorization.md" \
  --sample "$PWD/icarus-orig/Icarus/Binaries/Win64/IcarusServer-Win64-Shipping.exe" \
  --in-scope-asset "$PWD/icarus-orig" --network-profile offline
```

Complete the scope with `offline_execution`, local log inspection and resource
measurement, roles `lead`, `cre`, `doc`, and explicit exclusion of network access,
asset extraction, instrumentation, real players and production saves. Then run:

```bash
timeout 90s bash tools/run_reference_offline.sh
```

Review the exact Docker command in the script before approval. Effects:

- Create a separate ignored case and writable Proton prefix/runtime directory.
- Start one container from the pinned local image, with no pull or downloads.
- Run the original server under Proton, with original installation mounted read-only.
- Disable container networking and publish no host ports; attempted external
  connections cannot leave the network namespace.
- Limit CPU, memory and process counts; write only temporary/container state and
  local case artifacts. The case directory is mounted at the image's existing home.
- Observe for roughly 30–45 seconds, save container logs/inspection/resource data,
  stop the exact created container, and remove it. Preserve all case artifacts.

Expected startup/network failures are observations, not confirmed original bugs.
This does not authorize downloading SDKs, opening ports, using real save data, or
starting connected client tests. The independent replacement remains native Bevy.

The vendored [AGENTS.md](../.agents/vendor/reverse-skill/AGENTS.md) requires explicit
script approval, and the original case explicitly excludes server execution. This
batch therefore needs separate approval and its own target scope.

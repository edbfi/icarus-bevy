> Archived research report, retained verbatim. Current conclusions live in [the ledgers](../../ledgers/findings/). Moved from `docs/research/` on 2026-10-13.

# Original server inventory

Historical inventory — 2026-10-04. Unknowns below describe this inventory stage;
the later [static baseline](2026-10-04-static-baseline.md) records manifest
inspection. See the [current checkpoint](../../implementation-status.md) for pending
work and pause status. Reproduction requires a fresh authorized case.

The authorized offline inventory contains **435 files**, totaling **10,443,211,803
bytes**. This establishes a reproducible local file baseline; it does not establish
the official release version, protocol, runtime dependencies, or any server bug.

## Evidence

| Item | Observation |
| --- | --- |
| Case | `work/icarus-server-baseline/` (ignored, local) |
| Scope | User-approved offline inventory and hashing only |
| Evidence | E-001, `evidence/inventory.json` |
| Inventory SHA-256 | `5b33cee8126d6edb1306624126cdb3dc2c9b93b65505b24261f739486c67bb1d` |
| Shipping executable | `Icarus/Binaries/Win64/IcarusServer-Win64-Shipping.exe` |
| Executable size | 108,067,840 bytes |
| Executable SHA-256 | `67cfc915d9d085b758057e2960d4a2018fb121123af1329c25ef7aa58b960104` |
| Installation manifest | `steamapps/appmanifest_2089300.acf` is present; contents not yet inspected |

Reproduction after the case scope/guard is ready:

```bash
python3 tools/reference_inventory.py --root icarus-orig \
  --output work/icarus-server-baseline/evidence/inventory.json
```

## Findings and limitations

E-001 shows files whose paths name Steamworks, EOS, PhysX, Sentry and CrashReportClient.
These are candidate integration/investigation leads. File presence does **not**
prove the server loads them, requires them, or fails in them. Build pairing,
engine version and all gameplay/network behavior remain unknown.

Evidence → Finding → Path: E-001 identifies the shipping executable and a build
manifest; this supports choosing them for the next static inspection batch. It
does not support a runtime or compatibility conclusion.

```mermaid
flowchart LR
    A[Approved local scope] --> B[Inventory and hashes]
    B --> C[Executable and manifest identified]
    C --> D[Separate static inspection approval]
    D --> E[Metadata and behavioral leads]
```

No assets were extracted or reused. Original files were not changed. No original
process was launched, and there were no reference-target network connections.

Next: [offline static-analysis batch](../../history/static-analysis-authorization.md).

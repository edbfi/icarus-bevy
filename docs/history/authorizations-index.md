# Historical authorizations

Each record below is retained verbatim. None extends a runtime window or approves
new work. The removed reverse-skill package is referenced only as history.

| Batch | Approved | Scope | Limits | Outcome | Record |
| --- | --- | --- | --- | --- | --- |
| Offline reference inventory | 2026-10-04 | Read and hash `icarus-orig/`; case records under `work/` | No execution, capture, extraction or decompilation | Inventory report archived | [research-authorization.md](research-authorization.md) |
| Offline static analysis | 2026-10-04 | rabin2 info, imports, libraries, strings; app manifest copy | No execution, downloads, asset reuse | Static baseline and targeted report archived | [static-analysis-authorization.md](static-analysis-authorization.md) |
| Bounded headless decompilation | 2026-10-04 | Ghidra import, 180 s analysis, at most six functions | Ten-minute total, no symbol download or execution | Decompilation report archived | [decompilation-authorization.md](decompilation-authorization.md) |
| First offline startup | 2026-10-04 | One container, pinned Proton image, 30-45 s observation | No network, ports, real saves or clients | Startup report archived; launcher section reused as a runbook | [offline-runtime-authorization.md](offline-runtime-authorization.md) |
| Overnight research batch | 2026-10-04 | Eight-hour window, research priorities and Rust fallback queue | Resource caps, no unattended servers; window expired 2026-10-05 | Static and runtime checkpoints archived | [overnight-work-plan.md](overnight-work-plan.md) |

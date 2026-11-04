# Connected scenarios

Cases required (F-968; [gates](../gates.md)). User plays client. Empty servers
pause; offline synthetic join covers time, not map/harvest/build
(F-895).

## Route B: matched server on this host

1. Fetch beside `icarus-orig/`, never over it:
   `steamcmd +@sSteamCmdForcePlatformType windows +force_install_dir $PWD/icarus-server-live +login anonymous +app_update 2089300 validate +quit`.
   Rename `icarus-server-<buildid>` (app manifest); `tools/reference_inventory.py`;
   `tools/release_watch.py pin`. Both need Steam's current public build.
2. `tools/run_reference_connected.py`: case `network_profile.mode: connected`;
   session scope `reference-connected`, install/executable/hash, case-local
   `<prospect>.json` copy/hash, `scenario_id`, `shutdown_if_not_joined_seconds`,
   `shutdown_if_empty_seconds`, `max_players`, observation 60–2700 s,
   timeout +120 s, `max_runs` ≤4, session window.
3. Launcher copies Linux `steamclient.so` to `.steam/sdk32`/`sdk64` (F-687),
   generates `ServerSettings.ini`/password, exposes UDP 17777/27015 during run,
   prints name/password; stops after departure/save/lobby reload or deadline.
   Steam `GetServersAtAddress`: registration.
   `connection_method: "direct"` advertises Direct/uses `IpNetDriver` (F-1005).
   Verify UDP 17777 stateless challenge after load, before invitation; listing/log
   insufficient. `capture_image` (`tools/capture.Dockerfile`),
   `capture_max_bytes`: both ports → `capture/game.pcap`;
   `examples/a2s_query`: `127.0.0.1:27015`.
4. Icarus → world's character → **Join Session** → dedicated servers →
   `icarus-research-<scenario>` → password → scenario; leave via
   **Esc > exit to the main menu**, never dropship.

## Route A: both on the user's PC

Host unavailable: `pwsh tools/windows/icarus_scenario.ps1 -Action Setup`
(isolated `-UserDir`, version match, `PlayerData` backup), `start-server.cmd`,
join above; `-Action Collect`, `-Action Restore`; send zip/hash.

## Intake

Inventory/preserve zips: `reference-data/<date>-scenario-<id>/`; route B:
case-local. Keep account IDs private. Compare input/saved prospect (M-692),
inspect recorder/connection/version logs; index (`docs.md` rule 2).

Inbound metadata: one connection/input, case-qualified bindings:

```bash
target/debug/examples/capture_decode INPUT work/CASE/metadata.jsonl --inbound-controller-metadata --release-profile PROFILE --controller-channel CHANNEL --engine-net-version VERSION --byte-order little --backend-proxy-guid BACKEND --player-data-guid PLAYER_DATA --bestiary-guid BESTIARY
```

All flags required; byte order `little`/`big`; channel 1–32766; component GUIDs
distinct, >1. Profile pin/class bounds: retained 3.0.32/3.0.34; matching engine
version required. JSONL datagrams: `event:"datagram"`,
`direction:"client_to_server"`/`"server_to_client"`, unsigned `ms`, nonempty even
ASCII `hex`. C2S only, nondecreasing timestamps. Other events skip;
all lines count. No `--values-channel`. New output only: 0600, checkout
`work/`. Caps: 64 MiB input, 132 KiB/line, 100000 lines/datagrams/output rows,
32 MiB datagrams/output; bounded queues/assemblies.
Rows: `ms`, `channel`, `target_class`, nullable `field_index`, `bit_length`,
`framing_disposition`; stdout: counters. No payloads/values/identities. `framed`
means framing, not recognition/application. Unknown targets opaque;
properties/exports/must-map/client opens unsupported. Compare original/native
separately; check reliable/partial EOF counters. Missing prefixes/packets/bindings
prevent absence/progress claims; times: capture/reassembly arrival. Errors leave
an incomplete bounded prefix.

## Scenarios

S1 (≈15 min, one player, supplied prospect): join/stand inside base near
crop plots; open map/place marker/close; harvest one grown crop if any, plant
one seed and note which; leave.

S2 (≈5 min, one player, S1 prospect, direct/capture): join, stand one minute,
leave. F-1006; dropship arrival/reported creature attack limit save parity.
World entry: U-038, U-079.

Write later scenarios before runs.

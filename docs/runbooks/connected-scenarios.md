# Connected scenarios

[Gates](../gates.md), F-968. User plays; empty servers pause.
Synthetic: time only (F-895).

## B: server here

1. Fetch beside `icarus-orig/`:
   `steamcmd +@sSteamCmdForcePlatformType windows +force_install_dir $PWD/icarus-server-live +login anonymous +app_update 2089300 validate +quit`.
   Rename: `icarus-server-<buildid>`; `tools/reference_inventory.py`;
   `tools/release_watch.py pin`. Client/server: Steam current public build.
2. `tools/run_reference_connected.py`: case `network_profile.mode: connected`;
   session scope `reference-connected`, install/executable/hash, case-local
   `<prospect>.json` copy/hash, `scenario_id`, `shutdown_if_not_joined_seconds`,
   `shutdown_if_empty_seconds`, `max_players`, observation 60–2700 s,
   timeout +120 s, `max_runs` ≤4, session window.
3. Launcher copies Linux `steamclient.so` to `.steam/sdk32`/`sdk64` (F-687),
   writes `ServerSettings.ini`/password, opens UDP 17777/27015 in run;
   prints credentials; stops on departure/save/lobby reload/deadline.
   Registration: Steam `GetServersAtAddress`.
   `connection_method: "direct"` advertises Direct/uses `IpNetDriver` (F-1005).
   UDP 17777 stateless challenge required after load/before invite; listing/log insufficient. `capture_image` (`tools/capture.Dockerfile`),
   `capture_max_bytes`: both ports → `capture/game.pcap`;
   `examples/a2s_query`: `127.0.0.1:27015`.
4. Icarus → world's character → **Join Session** → dedicated servers →
   `icarus-research-<scenario>` → password → scenario; leave:
   **Esc > exit to the main menu**, never dropship.

## A: user PC

`pwsh tools/windows/icarus_scenario.ps1 -Action Setup`
(isolated `-UserDir`, version match, `PlayerData` backup), `start-server.cmd`,
join as above; `-Action Collect`, `-Action Restore`; send zip/hash.

## Intake

Private: logs/zips/account IDs. Preserve/inventory zips; copy unchanged to
`reference-data/<date>-scenario-<id>/` (B: case-local). Compare input/saved
prospect (M-692); inspect recorder/connection/version logs; index (`docs.md` rule 2).

`python3 tools/capture_rows_to_jsonl.py INPUT OUTPUT --client IPv4:port --server IPv4:port --work-root work`
Caller-exported JSONL fields map to string arrays, one value per field (`--help`: required list). Select connection by endpoints;
private raw output under work root. Inbound bindings are case-qualified:

```bash
target/debug/examples/capture_decode INPUT work/CASE/metadata.jsonl --inbound-controller-metadata --release-profile PROFILE --controller-channel CHANNEL --engine-net-version VERSION --byte-order little --backend-proxy-guid BACKEND --player-data-guid PLAYER_DATA --bestiary-guid BESTIARY
```

Required flags; order `little`/`big`; channel 1–32766; component GUIDs
distinct, >1. Inbound framing supports pinned 3.0.32 (F-954, F-955, F-1015)
and 3.0.34 (F-1016). Match engine version; neither grants RPC semantics.
Datagrams: `event:"datagram"`,
`direction:"client_to_server"`/`"server_to_client"`, unsigned `ms`, nonempty even
ASCII `hex`. C2S only, nondecreasing timestamps; other events skip, all lines count.
No `--values-channel`. New output: 0600, checkout `work/`.
Caps: 64 MiB input, 132 KiB/line, 100000 lines/datagrams/output rows,
32 MiB datagrams/output; bounded queues/assemblies.
Rows: `ms`, `channel`, `target_class`, nullable `field_index`, `bit_length`,
`framing_disposition`; stdout: counters. No payloads/values/IDs.
`framed`: framing, not recognition/application. Unknown targets opaque;
properties/exports/must-map/client opens unsupported. Compare original/native separately; check reliable/partial EOF counters. Missing prefixes/packets/bindings block absence/progress claims;
times: capture/reassembly arrival. Errors: incomplete output; write failure may tear the final row.

`tools/controller_package_members.cjs` selects the exact two controller members
from explicitly hashed public-server packages; `tools/cooked_class_field_counts.cjs`
counts bounded metadata prefixes. Its caller example is above
`readControllerClassMetadata`; select `ZERO_VERSION_PROFILE` for the qualified
3.0.34 zero-version header (M-1198), never as an automatic fallback. The optional `examples/protocol_oodle_decode.rs` uses pinned
oozextract 0.5.7 only for reference decoding; callers enforce process memory,
CPU and wall limits. No original assets ship. Retained source/count bindings:
M-1198; bounds: F-1016. The opt-in `readControllerClassProtocolMetadata`
returns selected property/type/flag trees and root net-function identities,
never defaults or scripts. Runtime offsets, type spellings, struct schemas and
super checksums remain unresolved (M-1266). Selected 3.0.35 members match
3.0.34 (F-1026); this does not qualify its native ancestry or whole profile.

## Scenarios

S1 (≈15 min, one player, supplied prospect): stand in base near
crop plots; map/place marker/close; harvest one grown crop if any; plant/note one seed; leave.

S2 (≈5 min, one player, S1 prospect, direct/capture): join, stand 1 min,
leave (F-1006). Entry: U-038, U-079.

Plan later scenarios first.

# Connected scenarios

How a scenario with a real player is prepared, run and taken in. Each scenario
needs its own case; the user's standing instruction authorizes them (F-968,
gates "Before any original-server or client run"), and the user plays the
client. An empty server pauses the game (F-895); the offline launcher's
join mode lifts the pause with a synthetic player (F-895), which covers
time-driven state. Map, harvest and build actions need a real player.

## Route B: matched server on this host

1. Fetch the public server build next to `icarus-orig/`, never over it:
   `steamcmd +@sSteamCmdForcePlatformType windows +force_install_dir $PWD/icarus-server-live +login anonymous +app_update 2089300 validate +quit`,
   rename the directory to `icarus-server-<buildid>` (from its app manifest),
   inventory it with `tools/reference_inventory.py` and move the release pin
   (`tools/release_watch.py pin`). It matches a client only while both are on
   Steam's current public build.
2. Write the case scope (`network_profile` mode `connected`) and a session for
   `tools/run_reference_connected.py`: scope `reference-connected`, the install
   and executable with its hash, a case-local `<prospect>.json` copy with its
   hash, `scenario_id`, `shutdown_if_not_joined_seconds`,
   `shutdown_if_empty_seconds`, `max_players`, observation (60 to 2700 s) and
   timeout (plus 120 s), `max_runs` (at most 4) and the session window.
3. The launcher copies SteamCMD's Linux `steamclient.so` pair into the run's
   `.steam/sdk32` and `sdk64` (F-687), writes `ServerSettings.ini` with a
   generated join password, publishes UDP 17777 and 27015 for the run only,
   prints the server name and password, and stops once the server has saved
   and loaded its lobby after the last player left, or at the deadline.
   `GetServersAtAddress` on Steam's web API shows the registration.
4. The user joins: start Icarus, select the character of that world, **Join
   Session**, switch the browser to dedicated servers, pick
   `icarus-research-<scenario>`, enter the password, follow the steps below,
   then leave with **Esc > exit to the main menu** (never the dropship).

## Route A: both on the user's PC

When this host cannot serve, the user runs the official server beside the
client: `pwsh tools/windows/icarus_scenario.ps1 -Action Setup` (isolated `-UserDir`,
version match enforced, client `PlayerData` backed up), `start-server.cmd`,
join as above, then `-Action Collect` and `-Action Restore`, and send the zip
with its printed hash.

## Intake

Copy returned zips unchanged into `reference-data/<date>-scenario-<id>/` with
an inventory; route B outputs stay in their case. Logs carry account
identifiers: keep everything private. Compare the input copy and the saved
prospect with the save-load helpers (M-692), read the server log for recorder,
connection and version lines, and index the case (`docs.md` rule 2).

## Scenarios

S1, player actions (about 15 minutes, one player, the supplied prospect):

1. Join and stand inside the base near the crop plots.
2. Open the map, place one marker, close it.
3. Harvest one grown crop, if any, and plant one seed in one plot; note which.
4. Leave.

Later scenarios (build or remove one piece, feed a tame, a second player) are
written here before they are run.

# Connected scenarios

How a scenario with a real player in the world is prepared, run and taken in.
Each scenario needs the user's approval and a case (gates "Before any
original-server or client run"). Behavior that only happens during play cannot
be observed offline: an empty server pauses the game while it waits (F-684).

## Routes

| Route | Where it runs | Build match | Network exposure | State |
| --- | --- | --- | --- | --- |
| A, user-run | the user's Windows PC: official dedicated server and stock client side by side | automatic: Steam updates both | none beyond the user's PC | ready |
| B, lab-run | this Linux host's launcher, client joins over the internet | needs a matched server download, Steam egress | public UDP 17777/27015 for the run | not built |

Route A comes first. The preserved server here is 3.0.30 and the installed
client 3.0.31 (F-683); with exact version equality the older server would
refuse that client (network-checksum spec), so route B also needs a fresh
server download for every client update.

## Route A on the user's PC

One-time setup:

1. Install the official dedicated server with SteamCMD (wiki "Server Setup"):
   `force_install_dir C:\IcarusServer`, `login anonymous`,
   `app_update 2089300 validate`. Run the same `app_update` again before every
   scenario so it matches the client Steam keeps current.
2. Copy `tools/windows/icarus_scenario.ps1` to the PC.

Per scenario, in PowerShell (`-ExecutionPolicy Bypass` if scripts are blocked):

```powershell
powershell -ExecutionPolicy Bypass -File .\icarus_scenario.ps1 -Action Setup `
  -Root C:\IcarusResearch\s1 -Archive C:\path\to\the-save-archive.zip `
  -Prospect <save name> -ServerExe C:\IcarusServer\IcarusServer.exe `
  -JoinPassword <letters-and-digits>
```

Setup refuses an existing root or a server/client version mismatch, extracts
only `DedicatedServer/Prospects/<prospect>.json` into an isolated `-UserDir`
(the default Saved folder is the client's own), writes `ServerSettings.ini`
(`MaxPlayers=1`, `ResumeProspect=False`, empty timer 60 s, join password),
writes `start-server.cmd`, backs up the client's `PlayerData` and prints
`setup.json` with both versions and hashes. Then:

1. Run `C:\IcarusResearch\s1\start-server.cmd`. Its log window shows the
   prospect load and, a minute or two later, the map's
   `IpNetDriver listening on port 17777`.
2. Start Icarus, select the character that plays this prospect, choose
   **Join Session**, switch the browser toggle (top right) to dedicated servers,
   select `icarus-research-s1` and enter the password. If it is not listed,
   use Steam's **View > Game Servers > LAN**.
3. Follow the scenario's steps, writing each step's local time into
   `notes.txt`. Leave with **Esc > exit to the main menu**; never board the
   dropship or return to orbit.
4. Wait for the server log to say it is saving and to load
   `DedicatedServerEntry` (about 60 s after leaving), then close its window.
5. `-Action Collect -Root C:\IcarusResearch\s1` zips the server's Saved tree,
   the client logs, `input/`, the notes and a SHA-256 manifest, and prints the
   zip's hash. `-Action Restore` puts the client's `PlayerData` back and keeps
   the post-scenario copy beside the backup.
6. Transfer the zip to this host's `~/downloads/` and send its printed hash.

## Intake

1. Verify the hash, copy the zip unchanged to
   `reference-data/<date>-scenario-<id>/` with an inventory, as for the save
   archive. The client logs carry account identifiers: keep everything private.
2. Open a case `work/<scenario>-intake-<date>/` with `case.json` and a scope
   naming the zip; analyze copies there only.
3. Compare recorder chunks of `input/` and the server's saved prospect with
   the save-load case helpers (M-692); read the server log for recorder,
   connection and version lines. Index the case (`docs.md` rule 2).

## Scenarios

S1, player present (about 25 minutes, one player, the supplied prospect):

1. Join, then stand still inside the base near the crop plots for 10 minutes.
   This measures the clock and crop growth with a player present (F-684).
2. Open the map, place one marker, close it.
3. Harvest one grown crop, if any, and plant one seed in one plot; note which.
4. Leave as in step 3 above.

Later scenarios (build or remove one piece, feed a tame, a second player) are
written here before they are run.

<#
.SYNOPSIS
Prepares and collects one user-run ICARUS scenario on a Windows PC that runs the
official dedicated server and the stock client side by side
(docs/runbooks/connected-scenarios.md).

.DESCRIPTION
Setup copies one prospect out of the user's own save archive into an isolated
server folder, writes ServerSettings.ini and a start script, backs up the
client's PlayerData, and records versions and hashes. Collect packages the
server's Saved tree, the client log and the notes into one zip with a SHA-256
manifest. Restore puts the client's PlayerData backup back. Nothing is sent
anywhere; the script never edits the server or client installation.
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][ValidateSet('Setup', 'Collect', 'Restore')][string]$Action,
    [string]$Root = 'C:\IcarusResearch\s1',
    [string]$Archive,
    [string]$Prospect,
    [string]$ServerExe = 'C:\IcarusServer\IcarusServer.exe',
    [string]$ClientRoot = 'C:\Program Files (x86)\Steam\steamapps\common\Icarus',
    [string]$JoinPassword,
    [int]$EmptySeconds = 60
)
Set-StrictMode -Version 3
$ErrorActionPreference = 'Stop'

$ServerHome = Join-Path $Root 'server'
$Saved = Join-Path $ServerHome 'Saved'
$ClientSaved = Join-Path $env:LOCALAPPDATA 'Icarus\Saved'
$Backup = Join-Path $Root 'client-playerdata-backup'

function Get-Sha256([string]$Path) { (Get-FileHash -Algorithm SHA256 -LiteralPath $Path).Hash.ToLowerInvariant() }

function Read-Version([string]$InstallRoot) {
    $file = Join-Path $InstallRoot 'Icarus\Config\version.json'
    if (-not (Test-Path -LiteralPath $file)) { return $null }
    $v = (Get-Content -Raw -LiteralPath $file | ConvertFrom-Json).Version
    return "$($v.Major).$($v.Minor).$($v.Patch).$($v.Changelist)"
}

function Write-Ascii([string]$Path, [string]$Text) {
    New-Item -ItemType Directory -Force -Path (Split-Path -Parent $Path) | Out-Null
    [IO.File]::WriteAllText($Path, $Text, [Text.Encoding]::ASCII)
}

if ($Action -eq 'Setup') {
    if (-not $Archive -or -not (Test-Path -LiteralPath $Archive)) { throw 'Pass -Archive with the path of your save zip.' }
    if (-not (Test-Path -LiteralPath $ServerExe)) { throw "IcarusServer.exe not found at $ServerExe; pass -ServerExe." }
    if (-not $JoinPassword) { throw 'Pass -JoinPassword with a password of your choice.' }
    if ($JoinPassword -notmatch '^[A-Za-z0-9]{6,32}$') { throw 'Use 6-32 letters and digits for -JoinPassword.' }
    if (-not $Prospect -or $Prospect -notmatch '^[A-Za-z0-9_-]{1,64}$') { throw 'Pass -Prospect with the save name (the .json file name without .json).' }
    if (Test-Path -LiteralPath $Root) { throw "$Root already exists; choose a new -Root so earlier runs are kept." }
    $serverVersion = Read-Version (Split-Path -Parent $ServerExe)
    $clientVersion = Read-Version $ClientRoot
    if (-not $serverVersion -or -not $clientVersion) { throw 'Could not read both version.json files; check -ServerExe and -ClientRoot.' }
    if ($serverVersion -ne $clientVersion) { throw "Server $serverVersion and client $clientVersion differ; update both through Steam first." }

    $prospects = Join-Path $Saved 'PlayerData\DedicatedServer\Prospects'
    New-Item -ItemType Directory -Force -Path $prospects, (Join-Path $Root 'input') | Out-Null
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $zip = [IO.Compression.ZipFile]::OpenRead($Archive)
    try {
        $entry = $zip.Entries | Where-Object { $_.FullName -eq "DedicatedServer/Prospects/$Prospect.json" }
        if (-not $entry) { throw "The archive has no DedicatedServer/Prospects/$Prospect.json." }
        [IO.Compression.ZipFileExtensions]::ExtractToFile($entry, (Join-Path $Root "input\$Prospect.json"))
    } finally { $zip.Dispose() }
    Copy-Item -LiteralPath (Join-Path $Root "input\$Prospect.json") -Destination (Join-Path $prospects "$Prospect.json")

    Write-Ascii (Join-Path $Saved 'Config\WindowsServer\ServerSettings.ini') (
        "[/Script/Icarus.DedicatedServerSettings]`r`nJoinPassword=$JoinPassword`r`nMaxPlayers=1`r`n" +
        "ResumeProspect=False`r`nShutdownIfNotJoinedFor=900`r`nShutdownIfEmptyFor=$EmptySeconds`r`n" +
        "AllowNonAdminsToLaunchProspects=False`r`nAllowNonAdminsToDeleteProspects=False`r`n")
    Write-Ascii (Join-Path $Root 'start-server.cmd') (
        "@echo off`r`n`"$ServerExe`" -Log -UserDir=`"$ServerHome`" -SteamServerName=`"icarus-research-s1`" " +
        "-PORT=17777 -QueryPort=27015 -LoadProspect=$Prospect`r`n")

    $clientPlayerData = Join-Path $ClientSaved 'PlayerData'
    if (Test-Path -LiteralPath $clientPlayerData) { Copy-Item -Recurse -LiteralPath $clientPlayerData -Destination $Backup }
    Write-Ascii (Join-Path $Root 'notes.txt') (
        "Write one line per step with your local clock time, e.g. 20:41 joined.`r`n" +
        "joined:`r`nphase A start (standing still):`r`nphase A end:`r`nmap opened / marker placed:`r`n" +
        "crop plot harvested (where, what):`r`nseed planted (where, what):`r`nleft the server:`r`n" +
        "server back in lobby:`r`nanything unexpected:`r`n")
    [ordered]@{
        created_utc = (Get-Date).ToUniversalTime().ToString('o')
        server_version = $serverVersion; client_version = $clientVersion
        prospect = $Prospect; input_sha256 = Get-Sha256 (Join-Path $Root "input\$Prospect.json")
        archive_sha256 = Get-Sha256 $Archive
        server_settings_sha256 = Get-Sha256 (Join-Path $Saved 'Config\WindowsServer\ServerSettings.ini')
        client_playerdata_backed_up = (Test-Path -LiteralPath $Backup)
    } | ConvertTo-Json | Set-Content -Encoding ASCII -LiteralPath (Join-Path $Root 'setup.json')
    Get-Content -LiteralPath (Join-Path $Root 'setup.json')
    Write-Host "Ready. Start the server with $(Join-Path $Root 'start-server.cmd')"
}
elseif ($Action -eq 'Collect') {
    $out = Join-Path $Root 'collect'
    if (Test-Path -LiteralPath $out) { throw "$out already exists." }
    New-Item -ItemType Directory -Force -Path $out | Out-Null
    Copy-Item -Recurse -LiteralPath $Saved -Destination (Join-Path $out 'server-Saved')
    Copy-Item -Recurse -LiteralPath (Join-Path $ClientSaved 'Logs') -Destination (Join-Path $out 'client-Logs')
    foreach ($f in 'setup.json', 'notes.txt', 'start-server.cmd') { Copy-Item -LiteralPath (Join-Path $Root $f) -Destination $out }
    Copy-Item -Recurse -LiteralPath (Join-Path $Root 'input') -Destination (Join-Path $out 'input')
    $manifest = Get-ChildItem -Recurse -File -LiteralPath $out | ForEach-Object {
        [ordered]@{ path = $_.FullName.Substring($out.Length + 1); bytes = $_.Length; sha256 = Get-Sha256 $_.FullName }
    }
    $manifest | ConvertTo-Json | Set-Content -Encoding ASCII -LiteralPath (Join-Path $out 'manifest.json')
    $zipPath = Join-Path $Root ('icarus-s1-' + (Get-Date).ToUniversalTime().ToString('yyyyMMddTHHmmssZ') + '.zip')
    Compress-Archive -Path (Join-Path $out '*') -DestinationPath $zipPath
    Write-Host "Send this file: $zipPath"
    Write-Host "SHA-256: $(Get-Sha256 $zipPath)"
}
else {
    if (-not (Test-Path -LiteralPath $Backup)) { throw "No backup at $Backup." }
    $clientPlayerData = Join-Path $ClientSaved 'PlayerData'
    $aside = Join-Path $Root ('client-playerdata-after-' + (Get-Date).ToString('yyyyMMddHHmmss'))
    if (Test-Path -LiteralPath $clientPlayerData) { Move-Item -LiteralPath $clientPlayerData -Destination $aside }
    Copy-Item -Recurse -LiteralPath $Backup -Destination $clientPlayerData
    Write-Host "Restored PlayerData from $Backup; the post-scenario copy is kept at $aside."
}

<#
.SYNOPSIS
  Runs the built game (dist\apps\mwf_game) in Dolphin with scripted GameCube pad input and collects its screenshots
  and match logs. Several scripts run as one suite in one Dolphin boot.

.DESCRIPTION
  Sets up a private Dolphin profile and a virtual SD card under out\dolphin (or out\dolphin-N for -Instance N, so
  several Dolphins can run at once; all git-ignored, your own Dolphin settings are not touched), copies boot.dol,
  common.wpk, the weapon files and the map packs onto it, puts the script(s) there as autoplay.txt, boots the game
  and waits for it to switch itself off. The script syntax is in wii\game_core\src\autoplay.rs.

  One script: its screenshots ("shot NAME") come back as <base>\shots\NAME.png, as before.

  Several scripts (-Script a.txt,b.txt,...): one boot for all of them. Each becomes a section ("script NAME"; its
  "quit" lines become "endmatch", so the game leaves the match and the next script's "match" starts the next one),
  and a final "quit" ends the run. Only the first script may start in the menus (without a "match" line). Results
  go to <base>\suite\<script>\: its screenshots as PNGs and its match log (match.log), from the "script=" in each
  log's header. Maps: those named on the scripts' "match" lines plus -Maps.

  A script with a "# runner: classes" line needs out\dolphin\classes.dat (custom classes) on the card; the suite
  puts it there for the whole run, so keep such scripts in a run of their own (tools\dolphin-suite.ps1 does).

  The same autoplay.txt works on a Wii: put it in the SD card's apps\mwf_game folder (and delete it afterwards, or
  the game plays it on every start).

.EXAMPLE
  powershell -ExecutionPolicy Bypass -File tools\dolphin-test.ps1 -Dolphin F:\dolphin\Dolphin.exe
.EXAMPLE
  powershell -ExecutionPolicy Bypass -File tools\dolphin-test.ps1 -Script tools\autoplay\killcam.txt,tools\autoplay\ks_air.txt -Instance 1
#>
param(
    # Dolphin.exe (or set MWF_DOLPHIN).
    [string]$Dolphin = $env:MWF_DOLPHIN,
    [string[]]$Script = @("tools\autoplay\menu_tour.txt"),
    # Map packs to put on the card (names under out\packs, without .wpk), besides those the scripts' match lines name.
    [string[]]$Maps = @("mp_rust"),
    [int]$TimeoutSeconds = 600,
    # Which Dolphin of several running at once (its own profile and card: out\dolphin-N; 0 is out\dolphin).
    [int]$Instance = 0,
    # Put out\dolphin\classes.dat on the card (the scripts' custom classes).
    [switch]$Classes
)
$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root
if (-not $Dolphin -or -not (Test-Path $Dolphin)) {
    throw "Dolphin.exe not found: pass -Dolphin <path> or set MWF_DOLPHIN."
}
if (-not (Test-Path "dist\apps\mwf_game\boot.dol")) { throw "Build the game first (build.bat)." }
if (-not (Test-Path "out\packs\common.wpk")) { throw "Convert the packs first (build.bat)." }

# "-Script a,b" or "-Maps a,b" through -File arrives as one string.
$scripts = @($Script -join "," -split "," | ForEach-Object { $_.Trim() } | Where-Object { $_ })
$suite = $scripts.Count -gt 1

$base = Join-Path $root ($(if ($Instance -eq 0) { "out\dolphin" } else { "out\dolphin-$Instance" }))
$app = Join-Path $base "sd\apps\mwf_game"
$shots = Join-Path $base "shots"
New-Item -ItemType Directory -Force "$app\maps", "$base\user\Config", $shots | Out-Null
Copy-Item "dist\apps\mwf_game\boot.dol", "dist\apps\mwf_game\meta.xml" $app -Force
Copy-Item "out\packs\common.wpk" $app -Force
if (Test-Path "out\packs\views") {
    robocopy "out\packs\views" "$app\views" /MIR /NJH /NJS /NFL /NDL | Out-Null
}
# A clean start: no saved settings, classes, logs or screenshots from earlier runs.
Get-ChildItem $app -Filter *.dat -ErrorAction SilentlyContinue | Remove-Item
if (Test-Path "$app\logs") { Get-ChildItem "$app\logs" -Filter *.log | Remove-Item }
if (Test-Path "$app\shots") { Get-ChildItem "$app\shots" -Filter *.bmp | Remove-Item }
Get-ChildItem $shots -Filter *.png | Remove-Item
if ($Classes) {
    if (Test-Path "out\dolphin\classes.dat") { Copy-Item "out\dolphin\classes.dat" "$app\classes.dat" -Force }
    else { Write-Warning "out\dolphin\classes.dat not found: the custom classes these scripts need are missing." }
}

# The script(s) as autoplay.txt, and the maps they start matches on.
$mapSet = [System.Collections.Generic.List[string]]::new()
foreach ($m in ($Maps -join "," -split ",")) { if ($m.Trim()) { $mapSet.Add($m.Trim()) } }
$names = @()
if ($suite) {
    $text = [System.Text.StringBuilder]::new()
    foreach ($s in $scripts) {
        $name = [System.IO.Path]::GetFileNameWithoutExtension($s)
        $names += $name
        [void]$text.AppendLine("script $name")
        foreach ($line in Get-Content $s) {
            $t = $line.Trim()
            if ($t -eq "quit") { [void]$text.AppendLine("endmatch"); continue }
            if ($t -match '^match\s+(\S+)') { $mapSet.Add($Matches[1]) }
            [void]$text.AppendLine($line)
        }
        # Back in the menus whatever the script did (a no-op if it already left the match).
        [void]$text.AppendLine("endmatch")
    }
    [void]$text.AppendLine("quit")
    Set-Content -Encoding ascii "$app\autoplay.txt" $text.ToString()
} else {
    foreach ($line in Get-Content $scripts[0]) { if ($line.Trim() -match '^match\s+(\S+)') { $mapSet.Add($Matches[1]) } }
    Copy-Item $scripts[0] "$app\autoplay.txt" -Force
}
Get-ChildItem "$app\maps" -Filter *.wpk | Remove-Item
foreach ($m in ($mapSet | Select-Object -Unique)) { Copy-Item "out\packs\$m.wpk" "$app\maps\" -Force }

# A profile of our own: SD card from the folder (written back when the game switches off), no audio, no dialogs,
# XFB copies kept in memory (the game reads its screenshots from there), unlimited speed.
$sd = ((Join-Path $base "sd") -replace '\\', '/') + "/"
@"
[Core]
WiiSDCard = True
WiiSDCardEnableFolderSync = True
WiiSDCardFilesize = 0
EmulationSpeed = 0.00000000
CPUThread = True
[General]
WiiSDCardSyncFolder = $sd
[Analytics]
PermissionAsked = True
Enabled = False
[Interface]
UsePanicHandlers = False
ConfirmStop = False
[DSP]
Backend = No Audio Output
"@ | Set-Content -Encoding ascii "$base\user\Config\Dolphin.ini"
@"
[Hacks]
XFBToTextureEnable = False
EFBToTextureEnable = False
[Settings]
InternalResolution = 1
"@ | Set-Content -Encoding ascii "$base\user\Config\GFX.ini"

Write-Host "Running $($scripts -join ', ') in Dolphin (instance $Instance)..."
$p = Start-Process -FilePath $Dolphin -ArgumentList @("-u", "$base\user", "-b", "-e", "$app\boot.dol") -PassThru
if (-not $p.WaitForExit($TimeoutSeconds * 1000)) {
    $p.Kill()
    throw "Dolphin did not finish within $TimeoutSeconds s (does the script end with quit?)."
}

Add-Type -AssemblyName System.Drawing
function Save-Png($bmp, $dir, $name) {
    $img = [System.Drawing.Image]::FromFile($bmp.FullName)
    $img.Save((Join-Path $dir ($name + ".png")), [System.Drawing.Imaging.ImageFormat]::Png)
    $img.Dispose()
}
if (-not $suite) {
    $n = 0
    foreach ($b in Get-ChildItem "$app\shots" -Filter *.bmp -ErrorAction SilentlyContinue) { Save-Png $b $shots $b.BaseName; $n++ }
    Write-Host "$n screenshots in $shots"
    $log = Get-ChildItem "$app\logs" -Filter *.log -ErrorAction SilentlyContinue | Sort-Object LastWriteTime | Select-Object -Last 1
    if ($log) { Write-Host "Match log: $($log.FullName)" }
    return
}
# A suite: each script's shots (SCRIPT__NAME.bmp) and match log (script=SCRIPT in its header) in its own folder.
$out = Join-Path $base "suite"
if (Test-Path $out) { Remove-Item -Recurse -Force $out }
foreach ($name in $names) { New-Item -ItemType Directory -Force (Join-Path $out $name) | Out-Null }
foreach ($b in Get-ChildItem "$app\shots" -Filter *.bmp -ErrorAction SilentlyContinue) {
    $parts = $b.BaseName -split "__", 2
    if ($parts.Count -eq 2 -and ($names -contains $parts[0])) { Save-Png $b (Join-Path $out $parts[0]) $parts[1] }
}
foreach ($l in Get-ChildItem "$app\logs" -Filter *.log -ErrorAction SilentlyContinue | Sort-Object Name) {
    $head = Get-Content $l.FullName -TotalCount 1
    if ($head -match 'script=(\S+)' -and ($names -contains $Matches[1])) {
        Get-Content $l.FullName | Add-Content (Join-Path (Join-Path $out $Matches[1]) "match.log")
    }
}
foreach ($name in $names) {
    $d = Join-Path $out $name
    $n = (Get-ChildItem $d -Filter *.png).Count
    $log = Test-Path (Join-Path $d "match.log")
    Write-Host ("  {0,-22} {1,4} shots  log={2}" -f $name, $n, $log)
}
Write-Host "Results in $out"

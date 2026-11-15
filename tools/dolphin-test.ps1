<#
.SYNOPSIS
  Runs the built game (dist\apps\mwf_game) in Dolphin with a scripted GameCube pad and collects its screenshots.

.DESCRIPTION
  Sets up a private Dolphin profile and a virtual SD card under out\dolphin (git-ignored; your own Dolphin settings
  are not touched), copies boot.dol, common.wpk, the weapon files and the chosen map packs onto it, puts the script
  there as autoplay.txt, boots the game and waits for it to switch itself off (the script's "quit"). Screenshots
  (the script's "shot NAME") come back as out\dolphin\shots\NAME.png. The script syntax is in
  wii\game_core\src\autoplay.rs; tools\autoplay\menu_tour.txt walks every menu and the HUD.

  The same autoplay.txt works on a Wii: put it in the SD card's apps\mwf_game folder (and delete it afterwards, or
  the game plays it on every start).

.EXAMPLE
  powershell -ExecutionPolicy Bypass -File tools\dolphin-test.ps1 -Dolphin F:\dolphin\Dolphin.exe
#>
param(
    # Dolphin.exe (or set MWF_DOLPHIN).
    [string]$Dolphin = $env:MWF_DOLPHIN,
    [string]$Script = "tools\autoplay\menu_tour.txt",
    # Map packs to put on the card (names under out\packs, without .wpk).
    [string[]]$Maps = @("mp_rust"),
    [int]$TimeoutSeconds = 600
)
$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root
if (-not $Dolphin -or -not (Test-Path $Dolphin)) {
    throw "Dolphin.exe not found: pass -Dolphin <path> or set MWF_DOLPHIN."
}
if (-not (Test-Path "dist\apps\mwf_game\boot.dol")) { throw "Build the game first (build.bat)." }
if (-not (Test-Path "out\packs\common.wpk")) { throw "Convert the packs first (build.bat)." }

$base = Join-Path $root "out\dolphin"
$app = Join-Path $base "sd\apps\mwf_game"
$shots = Join-Path $base "shots"
New-Item -ItemType Directory -Force "$app\maps", "$base\user\Config", $shots | Out-Null
Copy-Item "dist\apps\mwf_game\boot.dol", "dist\apps\mwf_game\meta.xml" $app -Force
Copy-Item "out\packs\common.wpk" $app -Force
if (Test-Path "out\packs\views") {
    robocopy "out\packs\views" "$app\views" /MIR /NJH /NJS /NFL /NDL | Out-Null
}
Get-ChildItem "$app\maps" -Filter *.wpk | Remove-Item
foreach ($m in $Maps) { Copy-Item "out\packs\$m.wpk" "$app\maps\" -Force }
Copy-Item $Script "$app\autoplay.txt" -Force
if (Test-Path "$app\shots") { Get-ChildItem "$app\shots" -Filter *.bmp | Remove-Item }
Get-ChildItem $shots -Filter *.png | Remove-Item

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

Write-Host "Running $Script in Dolphin..."
$p = Start-Process -FilePath $Dolphin -ArgumentList @("-u", "$base\user", "-b", "-e", "$app\boot.dol") -PassThru
if (-not $p.WaitForExit($TimeoutSeconds * 1000)) {
    $p.Kill()
    throw "Dolphin did not finish within $TimeoutSeconds s (does the script end with quit?)."
}

Add-Type -AssemblyName System.Drawing
$n = 0
foreach ($b in Get-ChildItem "$app\shots" -Filter *.bmp -ErrorAction SilentlyContinue) {
    $img = [System.Drawing.Image]::FromFile($b.FullName)
    $img.Save((Join-Path $shots ($b.BaseName + ".png")), [System.Drawing.Imaging.ImageFormat]::Png)
    $img.Dispose()
    $n++
}
Write-Host "$n screenshots in $shots"
$log = Get-ChildItem "$app\logs" -Filter *.log -ErrorAction SilentlyContinue | Sort-Object LastWriteTime | Select-Object -Last 1
if ($log) { Write-Host "Match log: $($log.FullName)" }

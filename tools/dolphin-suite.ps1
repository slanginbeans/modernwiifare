<#
.SYNOPSIS
  Runs every pad script in tools\autoplay (or those named) in a few Dolphins at once, each Dolphin playing its share
  of the scripts in one boot (tools\dolphin-test.ps1), and gathers the results in out\dolphin\suite\<script>\.

.DESCRIPTION
  Scripts that start with a "match" line are spread over -Parallel suites by their length (the frames they wait),
  longest first. A script that starts in the menus (no "match" line: menu_tour, last_stand) gets a Dolphin run to
  itself. Scripts with "# runner: classes" share one run with
  out\dolphin\classes.dat on the card. At most -Parallel Dolphins run at a time; each has its own profile and card
  (out\dolphin-N). Prints each script's shots, whether it wrote a match log, and the wall time.

.EXAMPLE
  powershell -ExecutionPolicy Bypass -File tools\dolphin-suite.ps1 -Dolphin F:\dolphin\Dolphin.exe
.EXAMPLE
  powershell -ExecutionPolicy Bypass -File tools\dolphin-suite.ps1 -Only killcam,hud_hits -Parallel 2
#>
param(
    [string]$Dolphin = $env:MWF_DOLPHIN,
    # Script names (without .txt) to run; all of tools\autoplay by default.
    [string[]]$Only,
    [int]$Parallel = 3
)
$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root
$t0 = Get-Date

$files = Get-ChildItem tools\autoplay\*.txt | Sort-Object Name
$only = @($Only -join "," -split "," | ForEach-Object { $_.Trim() } | Where-Object { $_ })
if ($only.Count) { $files = $files | Where-Object { $only -contains $_.BaseName } }

# A script's length: the frames it waits (its "wait N" lines and "times N wait M").
function Frames($f) {
    $n = 0
    foreach ($l in Get-Content $f) {
        if ($l -match '^\s*wait\s+(\d+)') { $n += [int]$Matches[1] }
        elseif ($l -match '^\s*times\s+(\d+)\s+wait\s+(\d+)') { $n += [int]$Matches[1] * [int]$Matches[2] }
    }
    $n
}
$quick = @(); $jobs = @()
foreach ($f in $files) {
    $text = Get-Content $f.FullName
    $starts = ($text | Where-Object { $_ -notmatch '^\s*(#|$)' } | Select-Object -First 1) -match '^\s*match\s'
    $classes = [bool]($text | Where-Object { $_ -match '^#\s*runner:\s*classes' })
    $maps = if ($f.BaseName -eq "menu_tour") { "mp_afghan,mp_rust,mp_terminal" } else { "mp_rust" }
    if (-not $starts) { $jobs += [pscustomobject]@{ Scripts = @($f.FullName); Classes = $classes; Maps = $maps; Frames = (Frames $f.FullName) } }
    elseif ($classes) { $quick += [pscustomobject]@{ File = $f.FullName; Classes = $true; Frames = (Frames $f.FullName) } }
    else { $quick += [pscustomobject]@{ File = $f.FullName; Classes = $false; Frames = (Frames $f.FullName) } }
}
# The classes scripts together; the rest spread over the Dolphins left, longest first into the shortest suite.
$withClasses = @($quick | Where-Object Classes)
if ($withClasses.Count) {
    $jobs += [pscustomobject]@{ Scripts = @($withClasses.File); Classes = $true; Maps = "mp_rust"; Frames = ($withClasses | Measure-Object Frames -Sum).Sum }
}
$plain = @($quick | Where-Object { -not $_.Classes } | Sort-Object Frames -Descending)
$bins = [math]::Max(1, $Parallel)
$groups = @(1..$bins | ForEach-Object { [pscustomobject]@{ Scripts = [System.Collections.Generic.List[string]]::new(); Frames = 0 } })
foreach ($q in $plain) {
    $g = $groups | Sort-Object Frames | Select-Object -First 1
    $g.Scripts.Add($q.File); $g.Frames += $q.Frames
}
foreach ($g in $groups | Where-Object { $_.Scripts.Count }) {
    $jobs += [pscustomobject]@{ Scripts = @($g.Scripts); Classes = $false; Maps = "mp_rust"; Frames = $g.Frames }
}
$jobs = @($jobs | Sort-Object Frames -Descending)

# Run them, at most -Parallel at a time, each Dolphin on its own instance number.
$out = "out\dolphin\suite"
if (Test-Path $out) { Remove-Item -Recurse -Force $out }
New-Item -ItemType Directory -Force $out | Out-Null
$running = @{}
$free = [System.Collections.Generic.Queue[int]]::new()
foreach ($i in 1..$Parallel) { $free.Enqueue($i) }
$queue = [System.Collections.Generic.Queue[object]]::new()
foreach ($j in $jobs) { $queue.Enqueue($j) }
while ($queue.Count -or $running.Count) {
    while ($queue.Count -and $free.Count) {
        $j = $queue.Dequeue(); $i = $free.Dequeue()
        # Frames at ~60 a second plus boots, doubled for slack.
        $timeout = [int](($j.Frames / 60 + 60) * 2)
        $argv = @("-NoProfile", "-ExecutionPolicy", "Bypass", "-File", "tools\dolphin-test.ps1", "-Dolphin", $Dolphin,
            "-Script", ($j.Scripts -join ","), "-Maps", $j.Maps, "-Instance", $i, "-TimeoutSeconds", $timeout)
        if ($j.Classes) { $argv += "-Classes" }
        $log = "out\dolphin\suite\run-$i-$([guid]::NewGuid().ToString('N').Substring(0,6)).txt"
        $p = Start-Process powershell -ArgumentList $argv -PassThru -NoNewWindow -RedirectStandardOutput $log
        $running[$i] = [pscustomobject]@{ Process = $p; Job = $j; Log = $log; Start = Get-Date }
    }
    Start-Sleep -Milliseconds 500
    foreach ($i in @($running.Keys)) {
        $r = $running[$i]
        if (-not $r.Process.HasExited) { continue }
        $running.Remove($i)
        $free.Enqueue($i)
        $base = if ($i -eq 0) { "out\dolphin" } else { "out\dolphin-$i" }
        $secs = ((Get-Date) - $r.Start).TotalSeconds
        foreach ($s in $r.Job.Scripts) {
            $name = [System.IO.Path]::GetFileNameWithoutExtension($s)
            $dest = Join-Path $out $name
            New-Item -ItemType Directory -Force $dest | Out-Null
            if ($r.Job.Scripts.Count -gt 1) {
                if (Test-Path "$base\suite\$name") { Copy-Item "$base\suite\$name\*" $dest -Force }
            } else {
                Copy-Item "$base\shots\*.png" $dest -Force -ErrorAction SilentlyContinue
                $l = Get-ChildItem "$base\sd\apps\mwf_game\logs" -Filter *.log -ErrorAction SilentlyContinue | Sort-Object LastWriteTime | Select-Object -Last 1
                if ($l) { Copy-Item $l.FullName (Join-Path $dest "match.log") }
            }
            Copy-Item $r.Log (Join-Path $dest "run.txt") -Force
        }
        Write-Host ("Dolphin {0}: {1} script(s) in {2:n0} s" -f $i, $r.Job.Scripts.Count, $secs)
    }
}
Get-ChildItem $out -Filter run-*.txt | Remove-Item
foreach ($d in Get-ChildItem $out -Directory | Sort-Object Name) {
    $n = (Get-ChildItem $d.FullName -Filter *.png).Count
    $log = Join-Path $d.FullName "match.log"
    $panic = if (Test-Path $log) { (Select-String -Path $log -Pattern "panic" -SimpleMatch | Measure-Object).Count } else { 0 }
    "{0,-22} {1,4} shots  log={2,-5} panics={3}" -f $d.Name, $n, (Test-Path $log), $panic
}
"All done in {0:n0} s -> {1}" -f ((Get-Date) - $t0).TotalSeconds, $out

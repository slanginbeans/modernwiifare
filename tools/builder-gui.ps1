# Modern Wiifare builder: a window over build.bat with its switches as checkboxes.
#   Start it with builder.bat (double-click). `-SelfTest` builds the window without showing it and checks how the
#   settings turn into build.bat arguments (used by CI).
#
# Runs build.bat with the chosen switches and environment, shows its output live, remembers the settings in
# out\builder-settings.json (git-ignored), and can copy the finished mwf_game folder to an SD card.
param([switch]$SelfTest)

$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing

$repo = Split-Path -Parent $PSScriptRoot
$settingsPath = Join-Path $repo 'out\builder-settings.json'
$logPath = Join-Path $repo 'out\builder-last.log'
$appDir = Join-Path $repo 'dist\apps\mwf_game'

# ---- settings -------------------------------------------------------------------------------------------------------

function New-Settings {
    [ordered]@{
        Update      = $true    # git pull first
        Data        = $true    # convert game data (maps, weapons) and copy it into dist
        SlimViews   = $false   # smaller weapon files (MWF_SLIM_VIEWS)
        Force       = $false   # reconvert everything (--force)
        Simplify    = '1.0'    # world detail (--simplify <ratio>; 1.0 = full)
        AllowDownloads = $false # let build.bat / mwf-convert.ps1 download Rust / llvm-mingw (MWF_ASSUME_YES)
        Game        = ''       # MWF_GAME (empty: the folder next to the repo)
        DevkitPro   = ''       # MWF_DEVKITPRO (empty: searched on every drive)
        CopyToSd    = $false
        SdDrive     = ''
    }
}

function Read-Settings {
    $s = New-Settings
    if (Test-Path $settingsPath) {
        try {
            $saved = Get-Content $settingsPath -Raw | ConvertFrom-Json
            foreach ($k in @($s.Keys)) {
                if ($null -ne $saved.$k) { $s[$k] = $saved.$k }
            }
        } catch { }
    }
    $s
}

function Save-Settings($s) {
    New-Item -ItemType Directory -Force (Split-Path $settingsPath) | Out-Null
    ($s | ConvertTo-Json) | Set-Content -Path $settingsPath -Encoding UTF8
}

# What to run for a build (`Clean` = the clean target): build.bat's arguments and the environment to set
# (a $null value removes the variable). Pure, so -SelfTest can check it.
function Get-BuildCommand($s, [switch]$Clean) {
    $argv = New-Object System.Collections.Generic.List[string]
    if ($Clean) {
        $argv.Add('clean')
    } else {
        if (-not $s.Update) { $argv.Add('noupdate') }
        if (-not $s.Data) { $argv.Add('nodata') }
    }
    $argv.Add('nopause')
    $flags = New-Object System.Collections.Generic.List[string]
    if ($s.Force) { $flags.Add('--force') }
    $ratio = 1.0
    if ([double]::TryParse([string]$s.Simplify, [Globalization.NumberStyles]::Float,
            [Globalization.CultureInfo]::InvariantCulture, [ref]$ratio) -and $ratio -lt 1.0) {
        $flags.Add('--simplify')
        $flags.Add($ratio.ToString([Globalization.CultureInfo]::InvariantCulture))
    }
    $envs = [ordered]@{
        MWF_SLIM_VIEWS    = if ($s.SlimViews) { '1' } else { $null }
        MWF_CONVERT_FLAGS = if ($flags.Count) { $flags -join ' ' } else { $null }
        MWF_ASSUME_YES    = if ($s.AllowDownloads) { '1' } else { $null }
        MWF_GAME          = if ([string]::IsNullOrWhiteSpace($s.Game)) { $null } else { $s.Game }
        MWF_DEVKITPRO     = if ([string]::IsNullOrWhiteSpace($s.DevkitPro)) { $null } else { $s.DevkitPro }
    }
    @{ Args = $argv.ToArray(); Env = $envs }
}

# ---- window ---------------------------------------------------------------------------------------------------------

$settings = Read-Settings

$form = New-Object Windows.Forms.Form
$form.Text = 'Modern Wiifare builder'
$form.Size = New-Object Drawing.Size(760, 720)
$form.MinimumSize = New-Object Drawing.Size(640, 560)
$form.StartPosition = 'CenterScreen'
$form.Font = New-Object Drawing.Font('Segoe UI', 9)

$y = 12
function Add-Check($text, $tip, $checked) {
    $c = New-Object Windows.Forms.CheckBox
    $c.Text = $text
    $c.AutoSize = $true
    $c.Location = New-Object Drawing.Point(16, $script:y)
    $c.Checked = [bool]$checked
    $tt.SetToolTip($c, $tip)
    $form.Controls.Add($c)
    $script:y += 26
    $c
}
$tt = New-Object Windows.Forms.ToolTip

$header = New-Object Windows.Forms.Label
$header.Text = 'Build switches'
$header.Font = New-Object Drawing.Font('Segoe UI', 10, [Drawing.FontStyle]::Bold)
$header.AutoSize = $true
$header.Location = New-Object Drawing.Point(12, $y)
$form.Controls.Add($header)
$y += 28

$cbUpdate = Add-Check 'Update from GitHub first' 'git pull before building (build.bat without "noupdate").' $settings.Update
$cbData = Add-Check 'Convert game data (maps, weapons) and copy it in' 'Converts what changed from your MW2 / IW4x folder (build.bat without "nodata").' $settings.Data
$cbSlim = Add-Check 'Slim weapon files (testing: keyframed clips, no gun mips, 32 kHz sounds)' 'MWF_SLIM_VIEWS=1. Rebuilds the weapon files when switched.' $settings.SlimViews
$cbForce = Add-Check 'Reconvert everything (ignore up-to-date packs)' 'Passes --force to the converter.' $settings.Force
$cbDownloads = Add-Check 'Allow downloading missing tools (Rust, llvm-mingw)' 'Answers yes to build.bat''s download prompts (MWF_ASSUME_YES=1). There is no console here to ask in.' $settings.AllowDownloads

$lblDetail = New-Object Windows.Forms.Label
$lblDetail.Text = 'World detail:'
$lblDetail.AutoSize = $true
$lblDetail.Location = New-Object Drawing.Point(18, ($y + 4))
$form.Controls.Add($lblDetail)
$cmbDetail = New-Object Windows.Forms.ComboBox
$cmbDetail.DropDownStyle = 'DropDownList'
$cmbDetail.Location = New-Object Drawing.Point(110, $y)
$cmbDetail.Width = 220
@('1.0 (full)', '0.75', '0.5', '0.35') | ForEach-Object { [void]$cmbDetail.Items.Add($_) }
$cmbDetail.SelectedIndex = [Math]::Max(0, @('1.0', '0.75', '0.5', '0.35').IndexOf([string]$settings.Simplify))
$tt.SetToolTip($cmbDetail, 'World mesh simplification (--simplify <ratio>). Lower is fewer triangles.')
$form.Controls.Add($cmbDetail)
$y += 34

function Add-Folder($label, $value, $tip) {
    $l = New-Object Windows.Forms.Label
    $l.Text = $label
    $l.AutoSize = $true
    $l.Location = New-Object Drawing.Point(18, ($script:y + 4))
    $form.Controls.Add($l)
    $t = New-Object Windows.Forms.TextBox
    $t.Location = New-Object Drawing.Point(150, $script:y)
    $t.Width = 470
    $t.Anchor = 'Top,Left,Right'
    $t.Text = [string]$value
    $tt.SetToolTip($t, $tip)
    $form.Controls.Add($t)
    $b = New-Object Windows.Forms.Button
    $b.Text = 'Browse...'
    $b.Location = New-Object Drawing.Point(630, ($script:y - 1))
    $b.Width = 90
    $b.Anchor = 'Top,Right'
    $b.Add_Click({
        $d = New-Object Windows.Forms.FolderBrowserDialog
        if ($t.Text -and (Test-Path $t.Text)) { $d.SelectedPath = $t.Text }
        if ($d.ShowDialog() -eq 'OK') { $t.Text = $d.SelectedPath }
    }.GetNewClosure())
    $form.Controls.Add($b)
    $script:y += 32
    $t
}
$txtGame = Add-Folder 'Game folder:' $settings.Game 'Your MW2 / IW4x folder. Empty: "Call of Duty Modern Warfare 2" next to this repo folder.'
$txtDkp = Add-Folder 'devkitPro folder:' $settings.DevkitPro 'Optional. Empty: searched on every drive.'

$cbSd = New-Object Windows.Forms.CheckBox
$cbSd.Text = 'Copy mwf_game to the SD card after a build:'
$cbSd.AutoSize = $true
$cbSd.Location = New-Object Drawing.Point(16, ($y + 2))
$cbSd.Checked = [bool]$settings.CopyToSd
$form.Controls.Add($cbSd)
$cmbSd = New-Object Windows.Forms.ComboBox
$cmbSd.DropDownStyle = 'DropDownList'
$cmbSd.Location = New-Object Drawing.Point(310, $y)
$cmbSd.Width = 160
function Update-Drives {
    $cmbSd.Items.Clear()
    [IO.DriveInfo]::GetDrives() | Where-Object { $_.DriveType -eq 'Removable' -and $_.IsReady } |
        ForEach-Object { [void]$cmbSd.Items.Add($_.Name.TrimEnd('\')) }
    $i = $cmbSd.Items.IndexOf([string]$settings.SdDrive)
    if ($i -ge 0) { $cmbSd.SelectedIndex = $i } elseif ($cmbSd.Items.Count) { $cmbSd.SelectedIndex = 0 }
}
Update-Drives
$form.Controls.Add($cmbSd)
$btnDrives = New-Object Windows.Forms.Button
$btnDrives.Text = 'Refresh'
$btnDrives.Location = New-Object Drawing.Point(480, ($y - 1))
$btnDrives.Add_Click({ Update-Drives })
$form.Controls.Add($btnDrives)
$y += 38

function Add-Button($text, $x, $w) {
    $b = New-Object Windows.Forms.Button
    $b.Text = $text
    $b.Location = New-Object Drawing.Point($x, $script:y)
    $b.Size = New-Object Drawing.Size($w, 32)
    $form.Controls.Add($b)
    $b
}
$btnBuild = Add-Button 'Build' 16 110
$btnBuild.Font = New-Object Drawing.Font('Segoe UI', 9, [Drawing.FontStyle]::Bold)
$btnClean = Add-Button 'Clean' 134 80
$btnStop = Add-Button 'Stop' 222 80
$btnStop.Enabled = $false
$btnOpen = Add-Button 'Open output folder' 310 140
$btnReport = Add-Button 'Test report' 458 110
$y += 42

$status = New-Object Windows.Forms.Label
$status.AutoSize = $true
$status.Location = New-Object Drawing.Point(16, $y)
$status.Text = 'Ready.'
$form.Controls.Add($status)
$y += 22

$log = New-Object Windows.Forms.TextBox
$log.Multiline = $true
$log.ReadOnly = $true
$log.ScrollBars = 'Both'
$log.WordWrap = $false
$log.Font = New-Object Drawing.Font('Consolas', 9)
$log.Location = New-Object Drawing.Point(16, $y)
$log.Size = New-Object Drawing.Size(706, (660 - $y))
$log.Anchor = 'Top,Bottom,Left,Right'
$form.Controls.Add($log)

function Get-UiSettings {
    $s = New-Settings
    $s.Update = $cbUpdate.Checked
    $s.Data = $cbData.Checked
    $s.SlimViews = $cbSlim.Checked
    $s.Force = $cbForce.Checked
    $s.AllowDownloads = $cbDownloads.Checked
    $s.Simplify = @('1.0', '0.75', '0.5', '0.35')[[Math]::Max(0, $cmbDetail.SelectedIndex)]
    $s.Game = $txtGame.Text.Trim()
    $s.DevkitPro = $txtDkp.Text.Trim()
    $s.CopyToSd = $cbSd.Checked
    $s.SdDrive = [string]$cmbSd.SelectedItem
    $s
}

# ---- running build.bat ----------------------------------------------------------------------------------------------
# Output goes to a log file that a UI-thread timer tails (PowerShell event handlers can't safely touch the window
# from the process's reader threads).

$script:proc = $null
$script:logPos = 0
$script:lastWasClean = $false
$oem = [Text.Encoding]::GetEncoding([Globalization.CultureInfo]::CurrentCulture.TextInfo.OEMCodePage)
$timer = New-Object Windows.Forms.Timer
$timer.Interval = 250

function Add-Log([string]$text) {
    $log.AppendText($text.Replace("`r`n", "`n").Replace("`n", "`r`n"))
}

function Read-NewLog {
    if (-not (Test-Path $logPath)) { return }
    $fs = [IO.File]::Open($logPath, 'Open', 'Read', 'ReadWrite')
    try {
        if ($fs.Length -gt $script:logPos) {
            $fs.Seek($script:logPos, 'Begin') | Out-Null
            $buf = New-Object byte[] ($fs.Length - $script:logPos)
            $n = $fs.Read($buf, 0, $buf.Length)
            $script:logPos += $n
            Add-Log ($oem.GetString($buf, 0, $n))
        }
    } finally { $fs.Close() }
}

function Set-Running([bool]$running) {
    foreach ($c in @($btnBuild, $btnClean, $btnReport)) { $c.Enabled = -not $running }
    $btnStop.Enabled = $running
}

function Copy-ToSd($s) {
    if (-not $s.CopyToSd) { return }
    if (-not $s.SdDrive) { Add-Log "`n[sd] No SD card drive chosen; not copying.`n"; return }
    if (-not (Test-Path (Join-Path $appDir 'boot.dol'))) { Add-Log "`n[sd] No build to copy.`n"; return }
    $dest = Join-Path ($s.SdDrive + '\') 'apps\mwf_game'
    Add-Log "`n[sd] Copying to $dest ...`n"
    $r = Start-Process robocopy -ArgumentList @("`"$appDir`"", "`"$dest`"", '/e', '/njh', '/njs', '/ndl', '/nfl', '/np') `
        -NoNewWindow -Wait -PassThru
    if ($r.ExitCode -lt 8) { Add-Log "[sd] Done.`n" } else { Add-Log "[sd] Copy failed (robocopy $($r.ExitCode)).`n" }
}

$timer.Add_Tick({
    Read-NewLog
    if ($script:proc -and $script:proc.HasExited) {
        $timer.Stop()
        Read-NewLog
        $code = $script:proc.ExitCode
        $script:proc = $null
        if ($code -eq 0) {
            $status.Text = 'Finished.'
            if (-not $script:lastWasClean) { Copy-ToSd (Get-UiSettings) }
        } else {
            $status.Text = "Failed (exit code $code). See the log."
        }
        Set-Running $false
    }
})

function Start-Build([switch]$Clean) {
    $s = Get-UiSettings
    Save-Settings $s
    $cmd = Get-BuildCommand $s -Clean:$Clean
    New-Item -ItemType Directory -Force (Split-Path $logPath) | Out-Null
    Set-Content -Path $logPath -Value '' -NoNewline
    $script:logPos = 0
    $log.Clear()
    Add-Log ("> build.bat " + ($cmd.Args -join ' ') + "`n")
    foreach ($k in $cmd.Env.Keys) { if ($null -ne $cmd.Env[$k]) { Add-Log "  $k=$($cmd.Env[$k])`n" } }

    $psi = New-Object Diagnostics.ProcessStartInfo 'cmd.exe'
    $psi.Arguments = '/d /c call build.bat ' + ($cmd.Args -join ' ') + ' > "' + $logPath + '" 2>&1'
    $psi.WorkingDirectory = $repo
    $psi.UseShellExecute = $false
    $psi.CreateNoWindow = $true
    # No console to answer prompts in: give the build an empty input (prompts read end-of-input and decline).
    $psi.RedirectStandardInput = $true
    foreach ($k in $cmd.Env.Keys) {
        if ($null -eq $cmd.Env[$k]) { [void]$psi.EnvironmentVariables.Remove($k) }
        else { $psi.EnvironmentVariables[$k] = $cmd.Env[$k] }
    }
    $script:lastWasClean = [bool]$Clean
    $script:proc = [Diagnostics.Process]::Start($psi)
    $script:proc.StandardInput.Close()
    $status.Text = if ($Clean) { 'Cleaning...' } else { 'Building...' }
    Set-Running $true
    $timer.Start()
}

$btnBuild.Add_Click({ Start-Build })
$btnClean.Add_Click({ Start-Build -Clean })
$btnStop.Add_Click({
    if ($script:proc -and -not $script:proc.HasExited) {
        & taskkill.exe /PID $script:proc.Id /T /F | Out-Null
        $status.Text = 'Stopped.'
    }
})
$btnOpen.Add_Click({
    $p = if (Test-Path $appDir) { $appDir } else { $repo }
    Start-Process explorer.exe $p
})
$btnReport.Add_Click({ Start-Process -FilePath (Join-Path $repo 'report.bat') -WorkingDirectory $repo })
$form.Add_FormClosing({ Save-Settings (Get-UiSettings) })

# ---- self-test (CI) -------------------------------------------------------------------------------------------------

if ($SelfTest) {
    $fail = 0
    function Check($what, $got, $want) {
        if ($got -ne $want) { Write-Host "FAIL $what`: got '$got', want '$want'"; $script:fail++ }
        else { Write-Host "ok   $what" }
    }
    $d = New-Settings
    $c = Get-BuildCommand $d
    Check 'defaults: args' ($c.Args -join ' ') 'nopause'
    Check 'defaults: no slim' $c.Env.MWF_SLIM_VIEWS $null
    Check 'defaults: no flags' $c.Env.MWF_CONVERT_FLAGS $null
    $s = New-Settings
    $s.Update = $false; $s.Data = $false; $s.SlimViews = $true; $s.Force = $true; $s.Simplify = '0.5'
    $s.AllowDownloads = $true; $s.Game = 'D:\mwf\Call of Duty Modern Warfare 2'
    $c = Get-BuildCommand $s
    Check 'switches: args' ($c.Args -join ' ') 'noupdate nodata nopause'
    Check 'switches: slim' $c.Env.MWF_SLIM_VIEWS '1'
    Check 'switches: flags' $c.Env.MWF_CONVERT_FLAGS '--force --simplify 0.5'
    Check 'switches: downloads' $c.Env.MWF_ASSUME_YES '1'
    Check 'switches: game' $c.Env.MWF_GAME 'D:\mwf\Call of Duty Modern Warfare 2'
    Check 'switches: devkitpro unset' $c.Env.MWF_DEVKITPRO $null
    Check 'clean: args' ((Get-BuildCommand $s -Clean).Args -join ' ') 'clean nopause'
    # The window builds and reads its own settings back.
    $cbSlim.Checked = $true; $cmbDetail.SelectedIndex = 2
    $u = Get-UiSettings
    Check 'ui: slim' $u.SlimViews $true
    Check 'ui: detail' $u.Simplify '0.5'
    $form.Dispose()
    if ($fail) { exit 1 } else { Write-Host 'builder self-test passed'; exit 0 }
}

[void]$form.ShowDialog()

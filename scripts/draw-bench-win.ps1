# Draw-speed benchmark for Windows: CPU and memory of Termurion, Windows Terminal and WezTerm
# for the same window grid and the same output, with and without a render plugin (effect).
# Run it ON the machine it measures, in the signed-in desktop session (over ssh a window cannot
# open on the desktop; start it from a session on that desktop, or a scheduled task). It runs
# under Windows PowerShell 5.1 and PowerShell 7. Same workloads, columns and output layout as
# scripts/draw-bench (the macOS bench), so the tables compare directly.
#
#   cpu%   CPU time of the terminal's processes (app, session host, OpenConsole) per wall second
#   ws%    the same for dwm.exe (the compositor; the column keeps draw-bench's name)
#   fps    frames presented, counted by PresentMon when it is found (-PresentMon, or on PATH);
#          Termurion keeps no frame log on disk, so without PresentMon the column is "-"
#   MB/s   bytes the workload got into the console (only the flood is limited by the terminal)
#   other% CPU of every other process during the run (noise; protected processes cannot be read)
#   RSS MB working set of the terminal's processes at the run's end
# Workloads (identical to draw-bench): idle, stream20, claude, flood. -Workloads picks some.
# The workload program runs inside the terminal, as a shell script (writer in the shell it starts);
# the driver times it from outside with process counters.
# Configs: termurion, termurion-fx (Lightning), termurion-fx-<id> for each shipped render plugin
# (aurora cyber embers fireworks glitch hologram katana lightning mecha neon phosphor shockwave
# teleport), windows-terminal, wezterm. Every Termurion config sets Effects animation to Smooth
# (60 frames/s). A terminal that is not installed is skipped with a line saying so.
#
# Window: 220 columns by 63 rows for every terminal. Windows Terminal and WezTerm get it from their
# command line; Termurion from its Default profile (it picks its own place).
# Termurion is -Termurion PATH (an exe, or the folder holding termurion.exe), else
# $env:DRAW_BENCH_TERMURION, else %LOCALAPPDATA%\Programs\Termurion, so two builds can be compared.
# Safety: it refuses to start while any process it would measure is already running (it only ever
# stops what it started), unless -KillExisting is given. Every Termurion setting it touches is
# saved first and put back on exit. Work files live in %LOCALAPPDATA%\draw-bench.
#
# Usage: scripts/draw-bench-win.ps1 [-Secs N] [-Passes N] [-Out DIR] [-Termurion PATH]
#            [-PresentMon PATH] [-WezTerm PATH] [-Workloads a,b] [-DryRun] [-KillExisting] [CONFIG...]
param(
    [int]$Secs = 20,
    [int]$Passes = 1,
    [string]$Out = '',
    [string]$Termurion = $env:DRAW_BENCH_TERMURION,
    [string]$PresentMon = '',
    [string]$WezTerm = '',
    [string[]]$Workloads = @(),
    [switch]$DryRun,
    [switch]$KillExisting,
    [Parameter(ValueFromRemainingArguments = $true)][string[]]$Config = @()
)
$ErrorActionPreference = 'Stop'
[Threading.Thread]::CurrentThread.CurrentCulture = [Globalization.CultureInfo]::InvariantCulture

$fxIds = 'aurora', 'cyber', 'embers', 'fireworks', 'glitch', 'hologram', 'katana', 'lightning', 'mecha', 'neon', 'phosphor', 'shockwave', 'teleport'
$COLS = 220; $ROWS = 63; $X = 60; $Y = 40
if ($Workloads.Count -eq 0) { if ($env:DRAW_BENCH_WL) { $Workloads = $env:DRAW_BENCH_WL -split '\s+' } else { $Workloads = 'idle', 'stream20', 'claude', 'flood' } }
if ($Config.Count -eq 0) {
    $Config = @('termurion') + @($fxIds | ForEach-Object { "termurion-fx-$_" }) + @('windows-terminal', 'wezterm')
}
foreach ($c in $Config) {
    if ($c -notmatch '^(termurion|termurion-fx|termurion-fx-[a-z0-9_-]+|windows-terminal|wezterm)$') {
        Write-Error "usage: draw-bench-win.ps1 [-Secs N] [-Passes N] [-Out DIR] [-Termurion PATH] [-DryRun] [termurion|termurion-fx|termurion-fx-<id>|windows-terminal|wezterm ...]"
        exit 2
    }
}

$work = Join-Path $env:LOCALAPPDATA 'draw-bench'
$here = if ($PSScriptRoot) { $PSScriptRoot } else { (Get-Location).Path }
$repo = Split-Path -Parent $here
if (-not (Test-Path -LiteralPath (Join-Path $repo 'Cargo.toml'))) { $repo = '' }
if (-not $Out) {
    $stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
    $Out = if ($repo) { Join-Path $repo "target\draw-bench\$stamp" } else { Join-Path $work "runs\$stamp" }
}

# --- what is installed -----------------------------------------------------------------------

function Find-Termurion {
    $c = if ($Termurion) { $Termurion } else { Join-Path $env:LOCALAPPDATA 'Programs\Termurion' }
    if (Test-Path -LiteralPath $c -PathType Container) { $c = Join-Path $c 'termurion.exe' }
    if (Test-Path -LiteralPath $c -PathType Leaf) { return (Resolve-Path -LiteralPath $c).Path }
    return $null
}
function Find-Exe($name, $given, $extra) {
    if ($given -and (Test-Path -LiteralPath $given -PathType Leaf)) { return $given }
    $cmd = Get-Command $name -ErrorAction SilentlyContinue | Select-Object -First 1
    if ($cmd) { return $cmd.Source }
    foreach ($p in $extra) { if ($p -and (Test-Path -LiteralPath $p -PathType Leaf)) { return $p } }
    return $null
}
function Program-For($cfg) {
    if ($cfg -like 'termurion*') { return Find-Termurion }
    if ($cfg -eq 'windows-terminal') { return Find-Exe 'wt.exe' '' @() }
    if ($cfg -eq 'wezterm') { return Find-Exe 'wezterm-gui.exe' $WezTerm @((Join-Path $env:ProgramFiles 'WezTerm\wezterm-gui.exe')) }
}
function Skip-Reason($cfg) {
    if (Program-For $cfg) { return '' }
    if ($cfg -like 'termurion*') { return 'no Termurion found (use -Termurion PATH)' }
    return "$cfg is not installed"
}
# The process names a config's CPU and memory are summed over (OpenConsole is the ConPTY host).
function Proc-Names($cfg) {
    if ($cfg -like 'termurion*') { return 'termurion', 'termurion-host', 'OpenConsole' }
    if ($cfg -eq 'windows-terminal') { return 'WindowsTerminal', 'wt', 'OpenConsole' }
    if ($cfg -eq 'wezterm') { return 'wezterm-gui', 'wezterm', 'wezterm-mux-server', 'OpenConsole' }
}
$allNames = 'termurion', 'termurion-host', 'WindowsTerminal', 'wt', 'wezterm-gui', 'wezterm', 'wezterm-mux-server', 'OpenConsole'
$presentMonExe = Find-Exe 'PresentMon*.exe' $PresentMon @()

# --- the plan --------------------------------------------------------------------------------

Write-Output "draw-bench-win plan: ${Secs}s runs, $Passes pass(es), window ${COLS}x${ROWS}, workloads: $($Workloads -join ' ')"
Write-Output "work dir $work, results in $Out"
Write-Output ("frames/s: " + $(if ($presentMonExe) { "PresentMon ($presentMonExe)" } else { 'not measured (no PresentMon; printed as "-")' }))
$planRuns = 0; $planSkip = 0
foreach ($cfg in $Config) {
    $r = Skip-Reason $cfg
    if ($r) { Write-Output "  skip  ${cfg}: $r"; $planSkip++; continue }
    Write-Output "  run   $cfg ($(Program-For $cfg)): $($Workloads -join ' ')"
    $planRuns++
}
Write-Output ("runs: {0} measured runs ({1} configs x {2} passes x {3} workloads), {4} config(s) skipped" -f ($planRuns * $Passes * $Workloads.Count), $planRuns, $Passes, $Workloads.Count, $planSkip)
Write-Output "each run waits for a quiet machine (no build), measures ${Secs}s, repeats up to 8 times if noisy"
if ($DryRun) { Write-Output 'dry run: nothing was started or changed'; exit 0 }

# --- from here on it touches the machine ---------------------------------------------------

foreach ($n in $allNames) {
    if (Get-Process -Name $n -ErrorAction SilentlyContinue) {
        if ($KillExisting) { Get-Process -Name $n -ErrorAction SilentlyContinue | Stop-Process -Force; Start-Sleep 1 }
        else { Write-Error "draw-bench-win: $n is already running; close it or pass -KillExisting"; exit 2 }
    }
}
New-Item -ItemType Directory -Force -Path $work, $Out | Out-Null
$results = Join-Path $Out 'results.tsv'
"config`tworkload`tpass`twall_s`tcpu_pct`tws_pct`tfps`tmb_s`tother_pct`tfront`tnote`trss_mb" | Set-Content -Encoding ASCII $results
$utf8 = New-Object Text.UTF8Encoding($false)

# The shell the terminal runs: it waits for a go file, runs the workload from the driver's
# command, and reports the bytes written. The workloads are the same as draw-bench's writer.pl.
$shell = @'
param([string]$Name)
$ErrorActionPreference = 'Stop'
$B = $PSScriptRoot
$esc = [string][char]27
$out = [Console]::OpenStandardOutput()
$enc = New-Object Text.UTF8Encoding($false)
$script:bytes = 0
$script:seed = [long]7
function Put([string]$s) { $b = $enc.GetBytes($s); $out.Write($b, 0, $b.Length); $out.Flush(); $script:bytes += $b.Length }
function Rnd { $script:seed = ($script:seed * 1103515245 + 12345) % 2147483648; return $script:seed }
$words = 'the renderer keeps each row it built and draws only what changed since the last frame while a spinner turns and new text arrives below the fold of a long answer about glyph atlases instanced quads damage regions frame pacing and the cost of a shader at full retina size'.Split(' ')
function Prose([int]$want) {
    $sb = New-Object Text.StringBuilder
    while ($sb.Length -lt $want) { [void]$sb.Append($words[(Rnd) % $words.Count]).Append(' ') }
    return $sb.ToString().Substring(0, $want)
}
function Pause([double]$until, $sw) { $ms = ($until - $sw.Elapsed.TotalSeconds) * 1000; if ($ms -gt 1) { [Threading.Thread]::Sleep([int]$ms) } }
function Run-Workload([string]$mode, [double]$secs) {
    $script:bytes = 0
    $sw = [Diagnostics.Stopwatch]::StartNew()
    switch ($mode) {
        'idle' { Start-Sleep -Milliseconds ([int]($secs * 1000)) }
        'stream20' {
            $n = 0
            while ($sw.Elapsed.TotalSeconds -lt $secs) {
                $n++
                Put ('{0:D6} {1}[1m{2}{1}[0m {3}' -f $n, $esc, 'line', (Prose (60 + (Rnd) % 50))) ; Put "`r`n"
                Pause ($n * 0.05) $sw
            }
        }
        'claude' {
            $spin = @([string][char]0x00B7, [string][char]0x2722, [string][char]0x2733, [string][char]0x2736, [string][char]0x273B, [string][char]0x273D)
            $h = ([string][char]0x2500) * 118
            $dim = "$esc[2m"; $rst = "$esc[0m"
            Put "`r`n`r`n`r`n`r`n"
            $tick = 0
            while ($sw.Elapsed.TotalSeconds -lt $secs) {
                $s = "`r$esc[2A$esc[J"
                if ($tick % 2 -eq 0) { $s += "$esc[38;5;252m" + (Prose (70 + (Rnd) % 40)) + "$rst`r`n" }
                $s += ("$esc[38;5;174m{0}$rst $esc[38;5;174mThinking{6}$rst $dim({1}s {2} {3} {4:N1}k tokens {2} esc to interrupt)$rst`r`n" -f $spin[$tick % 6], [int][math]::Floor($tick / 10), [char]0x00B7, [char]0x2191, (0.1 * $tick), 0, [char]0x2026)
                $s += "$dim" + [char]0x256D + $h + [char]0x256E + "$rst`r`n"
                $s += "$dim" + [char]0x2502 + "$rst > " + (' ' * 115) + "$dim" + [char]0x2502 + "$rst`r`n"
                $s += "$dim" + [char]0x2570 + $h + [char]0x256F + "$rst"
                $s += "$esc[1A`r$esc[4C"
                Put $s
                $tick++
                Pause ($tick * 0.1) $sw
            }
        }
        'flood' {
            $sb = New-Object Text.StringBuilder
            $i = 0
            while ($sb.Length -lt 1000000) {
                $i++
                [void]$sb.AppendFormat("{0:D7} $esc[3{1}m{2}$esc[0m {3}`r`n", $i, (1 + $i % 6), 'flood', (Prose (40 + (Rnd) % 80)))
            }
            $block = $enc.GetBytes($sb.ToString())
            $sw.Restart()
            while ($sw.Elapsed.TotalSeconds -lt $secs) { $out.Write($block, 0, $block.Length); $out.Flush(); $script:bytes += $block.Length }
        }
    }
}
Set-Content -Encoding ASCII (Join-Path $B "ready.$Name") $PID
$host.UI.RawUI.WindowTitle = "bench $Name"
while ($true) {
    $go = Join-Path $B "go.$Name"
    if (Test-Path $go) {
        $cmd = (Get-Content $go -Raw).Trim().Split(' ')
        Remove-Item $go
        Run-Workload $cmd[0] ([double]$cmd[1])
        Set-Content -Encoding ASCII (Join-Path $B "done.$Name") $script:bytes
    } elseif (Test-Path (Join-Path $B "quit.$Name")) { break }
    else { [Threading.Thread]::Sleep(5) }
}
'@
[IO.File]::WriteAllText((Join-Path $work 'shell.ps1'), $shell, $utf8)
$shellPath = Join-Path $work 'shell.ps1'

# --- settings, saved and put back ----------------------------------------------------------

$tcfg = Join-Path $env:APPDATA 'Termurion'
$saved = Join-Path $Out 'saved-settings'
New-Item -ItemType Directory -Force -Path $saved | Out-Null
$files = 'profiles.toml', 'settings.toml', 'state'
foreach ($f in $files) {
    $p = Join-Path $tcfg $f
    if (Test-Path -LiteralPath $p) { Copy-Item -LiteralPath $p (Join-Path $saved $f) }
}
$script:owned = @()
function Quit-Ours {
    foreach ($p in $script:owned) { if ($p -and -not $p.HasExited) { try { $p.Kill() } catch {} } }
    Start-Sleep 1
    # Nothing of these names ran when we started, so what is left is ours.
    foreach ($n in $allNames) { Get-Process -Name $n -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue }
    $script:owned = @()
    Get-ChildItem (Join-Path $work '*') -Include 'ready.*', 'go.*', 'done.*', 'quit.*' -ErrorAction SilentlyContinue | Remove-Item -Force -ErrorAction SilentlyContinue
}
function Restore-Settings {
    Quit-Ours
    foreach ($f in $files) {
        $s = Join-Path $saved $f; $t = Join-Path $tcfg $f
        if (Test-Path -LiteralPath $s) { Copy-Item -LiteralPath $s $t -Force } else { Remove-Item -LiteralPath $t -Force -ErrorAction SilentlyContinue }
    }
    [Console]::Error.WriteLine('draw-bench-win: settings restored')
}

# --- measuring -----------------------------------------------------------------------------

Add-Type -Namespace Bench -Name Win -MemberDefinition @'
[System.Runtime.InteropServices.DllImport("user32.dll")] public static extern System.IntPtr GetForegroundWindow();
[System.Runtime.InteropServices.DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(System.IntPtr h, out uint pid);
'@
function Front-Name {
    $pid2 = [uint32]0
    [void][Bench.Win]::GetWindowThreadProcessId([Bench.Win]::GetForegroundWindow(), [ref]$pid2)
    $p = Get-Process -Id $pid2 -ErrorAction SilentlyContinue
    if ($p) { return $p.ProcessName } else { return '?' }
}
# pid -> CPU seconds, for the named processes or (no names) every readable one.
function Snap($names) {
    $h = @{}
    $list = if ($names) { Get-Process -Name $names -ErrorAction SilentlyContinue } else { Get-Process }
    foreach ($p in $list) { try { $h[$p.Id] = $p.TotalProcessorTime.TotalSeconds } catch {} }
    return $h
}
function Delta($a, $b, $skip) {
    $d = 0.0
    foreach ($k in $b.Keys) {
        if ($skip -and $skip.ContainsKey($k)) { continue }
        $old = if ($a.ContainsKey($k)) { $a[$k] } else { 0.0 }
        $d += $b[$k] - $old
    }
    return $d
}
function Wait-Quiet {
    $said = $false; $quiet = 0
    while ($quiet -lt 2) {
        if (Get-Process -Name cargo, rustc, link, cl -ErrorAction SilentlyContinue) {
            if (-not $said) { [Console]::Error.WriteLine('draw-bench-win: the machine is busy (a build); waiting') }
            $said = $true; $quiet = 0
        } else { $quiet++ }
        Start-Sleep 15
    }
}
function Wait-File($path, $sec) {
    $t = [Diagnostics.Stopwatch]::StartNew()
    while (-not (Test-Path -LiteralPath $path)) { if ($t.Elapsed.TotalSeconds -gt $sec) { return $false }; Start-Sleep -Milliseconds 20 }
    return $true
}

function Setup-Config($cfg) {
    Quit-Ours
    $prog = Program-For $cfg
    $key = $cfg
    $q = '"' + $shellPath + '"'
    if ($cfg -like 'termurion*') {
        $key = 'termurion'
        $plugin = ''
        if ($cfg -eq 'termurion-fx') { $plugin = 'lightning' } elseif ($cfg -like 'termurion-fx-*') { $plugin = $cfg.Substring(13) }
        # The Default profile runs the bench shell at the bench size; -fx turns one plugin on.
        $cmd = ('powershell.exe -NoProfile -ExecutionPolicy Bypass -File ' + $q + ' termurion').Replace('\', '\\').Replace('"', '\"')
        $lines = Get-Content -LiteralPath (Join-Path $saved 'profiles.toml')
        $o = New-Object Collections.Generic.List[string]
        $inside = $false; $done = $false
        foreach ($l in $lines) {
            if ($inside -and $l -match '^(Columns|Rows|Command|"Custom Command"|"Render Plugins") = ') { continue }
            if ($l -match '^\[\[profile\]\]' -and $done) { $inside = $false }
            $o.Add($l)
            if ($l -eq 'Name = "Default"' -and -not $done) {
                $inside = $true; $done = $true
                $o.Add("Columns = $COLS"); $o.Add("Rows = $ROWS"); $o.Add('"Custom Command" = "Yes"'); $o.Add("Command = `"$cmd`"")
                if ($plugin) { $o.Add("`"Render Plugins`" = [`"$plugin`"]") }
            }
        }
        [IO.File]::WriteAllText((Join-Path $tcfg 'profiles.toml'), (($o -join "`n") + "`n"), $utf8)
        # Effects animation = Smooth (3), and no saved window frame to override the grid.
        $s = New-Object Collections.Generic.List[string]
        $s.Add('[settings]'); $s.Add('EffectsAnimation = 3')
        $sp = Join-Path $saved 'settings.toml'
        if (Test-Path -LiteralPath $sp) {
            foreach ($l in (Get-Content -LiteralPath $sp)) {
                if ($l -match '^\[settings\.SavedWindowFrames\]') { break }
                if ($l -eq '[settings]' -or $l -match '^EffectsAnimation = ') { continue }
                $s.Add($l)
            }
        }
        [IO.File]::WriteAllText((Join-Path $tcfg 'settings.toml'), (($s -join "`n") + "`n"), $utf8)
        Remove-Item -LiteralPath (Join-Path $tcfg 'state') -Force -ErrorAction SilentlyContinue
        $script:owned += Start-Process -FilePath $prog -PassThru
    } elseif ($cfg -eq 'windows-terminal') {
        $a = "--size $COLS,$ROWS --pos $X,$Y powershell.exe -NoProfile -ExecutionPolicy Bypass -File $q windows-terminal"
        $script:owned += Start-Process -FilePath $prog -ArgumentList $a -PassThru
    } elseif ($cfg -eq 'wezterm') {
        $a = "--config initial_cols=$COLS --config initial_rows=$ROWS --config window_close_confirmation=\`"NeverPrompt\`" start -- powershell.exe -NoProfile -ExecutionPolicy Bypass -File $q wezterm"
        $script:owned += Start-Process -FilePath $prog -ArgumentList $a -PassThru
    }
    if (-not (Wait-File (Join-Path $work "ready.$key") 40)) { [Console]::Error.WriteLine("draw-bench-win: $cfg did not start its shell"); return $false }
    Start-Sleep 3
    [Console]::Error.WriteLine("draw-bench-win: $cfg is up")
    return $true
}

# One measured run: returns the fields wall, cpu, ws, fps, mb, other, front, rss.
function Run-One($cfg, $wl) {
    $key = if ($cfg -like 'termurion*') { 'termurion' } else { $cfg }
    $names = Proc-Names $cfg
    $exeName = if ($cfg -like 'termurion*') { 'termurion' } elseif ($cfg -eq 'windows-terminal') { 'WindowsTerminal' } else { 'wezterm-gui' }
    $shellPid = [int](Get-Content (Join-Path $work "ready.$key"))
    $f0 = Front-Name
    $want = @($exeName)
    $a0 = Snap $null; $tAll0 = [DateTime]::UtcNow
    Remove-Item (Join-Path $work "done.$key") -Force -ErrorAction SilentlyContinue
    $pm = $null; $pmFile = Join-Path $work 'present.csv'
    if ($presentMonExe) {
        Remove-Item $pmFile -Force -ErrorAction SilentlyContinue
        $pm = Start-Process -FilePath $presentMonExe -ArgumentList "--process_name $exeName.exe --timed $Secs --terminate_after_timed --output_file `"$pmFile`" --stop_existing_session" -PassThru -WindowStyle Hidden
        Start-Sleep -Milliseconds 800
    }
    $c0 = Snap $names; $w0 = Snap @('dwm'); $t0 = [DateTime]::UtcNow
    Set-Content -Encoding ASCII (Join-Path $work "go.$key") "$wl $Secs"
    [void](Wait-File (Join-Path $work "done.$key") ($Secs + 30))
    $c1 = Snap $names; $w1 = Snap @('dwm'); $t1 = [DateTime]::UtcNow
    $rss = 0.0
    foreach ($p in (Get-Process -Name $names -ErrorAction SilentlyContinue)) { $rss += $p.WorkingSet64 }
    $bytes = [double](Get-Content (Join-Path $work "done.$key") -ErrorAction SilentlyContinue)
    $a1 = Snap $null; $tAll1 = [DateTime]::UtcNow
    $f1 = Front-Name
    $fps = '-'
    if ($pm) {
        [void]$pm.WaitForExit(15000)
        if (Test-Path $pmFile) { $fps = '{0:N1}' -f (([IO.File]::ReadAllLines($pmFile).Count - 1) / $Secs) }
    }
    $wall = ($t1 - $t0).TotalSeconds
    $cpu = 100 * (Delta $c0 $c1 $null) / $wall
    $ws = 100 * (Delta $w0 $w1 $null) / $wall
    # Everything that is not the terminal, the compositor or the writer shell is noise.
    $skip = @{}
    foreach ($k in $c1.Keys) { $skip[$k] = $true }
    foreach ($k in $w1.Keys) { $skip[$k] = $true }
    $skip[$shellPid] = $true
    $other = 100 * (Delta $a0 $a1 $skip) / (($tAll1 - $tAll0).TotalSeconds)
    $front = if ($want -contains $f0 -and $want -contains $f1) { 'yes' } else { "no($f0/$f1)" }
    return @(('{0:N2}' -f $wall), ('{0:N1}' -f $cpu), ('{0:N1}' -f $ws), $fps, ('{0:N2}' -f ($bytes / $wall / 1e6)), ('{0:N1}' -f $other), $front, ('{0:N0}' -f ($rss / 1MB)))
}

function Med($v) {
    $s = @($v | ForEach-Object { [double]$_ } | Sort-Object)
    $n = $s.Count
    if ($n -eq 0) { return 0 }
    if ($n % 2) { return $s[($n - 1) / 2] } else { return ($s[$n / 2 - 1] + $s[$n / 2]) / 2 }
}

try {
    for ($pass = 1; $pass -le $Passes; $pass++) {
        foreach ($cfg in $Config) {
            $r = Skip-Reason $cfg
            if ($r) { [Console]::Error.WriteLine("draw-bench-win: skipping ${cfg}: $r"); continue }
            Wait-Quiet
            if (-not (Setup-Config $cfg)) { continue }
            foreach ($wl in $Workloads) {
                $note = ''
                for ($try = 1; $try -le 8; $try++) {
                    Wait-Quiet
                    $f = Run-One $cfg $wl
                    if ($f[6] -eq 'yes' -and [double]$f[5] -lt 25) { break }
                    [Console]::Error.WriteLine("draw-bench-win: $cfg $wl try ${try}: front $($f[6]), other processes $($f[5])% of a core; again")
                    if ($try -eq 8) { $note = 'noisy' }
                }
                (@($cfg, $wl, $pass) + $f[0..6] + @($note, $f[7])) -join "`t" | Add-Content -Encoding ASCII $results
                [Console]::Error.WriteLine(('{0,-13} {1,-9} pass {2}: cpu {3}%  ws {4}%  fps {5}  {6} MB/s  other {7}%  front {8}' -f $cfg, $wl, $pass, $f[1], $f[2], $f[3], $f[4], $f[5], $f[6]))
            }
        }
    }
    # The table: the median over passes of each config and workload (same layout as draw-bench).
    $rows = @(Import-Csv -Delimiter "`t" $results)
    $tbl = New-Object Collections.Generic.List[string]
    $tbl.Add('| config | workload | CPU % | WindowServer % | frames/s | MB/s | RSS MB |')
    $tbl.Add('|---|---|---|---|---|---|---|')
    $seen = @{}
    foreach ($r in $rows) {
        $k = $r.config + "`t" + $r.workload
        if ($seen.ContainsKey($k)) { continue }
        $seen[$k] = $true
        $g = @($rows | Where-Object { $_.config -eq $r.config -and $_.workload -eq $r.workload })
        $fpsv = @($g | ForEach-Object { $_.fps } | Where-Object { $_ -ne '-' })
        $fpsText = if ($fpsv.Count) { '{0:N1}' -f (Med $fpsv) } else { '-' }
        $tbl.Add(('| {0} | {1} | {2:N1} | {3:N1} | {4} | {5:N2} | {6:N0} |' -f $r.config, $r.workload, (Med ($g | ForEach-Object { $_.cpu_pct })), (Med ($g | ForEach-Object { $_.ws_pct })), $fpsText, (Med ($g | ForEach-Object { $_.mb_s })), (Med ($g | ForEach-Object { $_.rss_mb }))))
    }
    Write-Output ''
    $tbl | Tee-Object -FilePath (Join-Path $Out 'table.md')
    [Console]::Error.WriteLine("draw-bench-win: results in $Out")
} finally {
    Restore-Settings
}

# probe-desktop.ps1 - runs the Task 0 isolated-world probe and the Task 1 bridge round trip in the
# CONSOLE session of this machine, through one one-shot scheduled task, then removes every trace.
# The probe window is created by the tests and is never shown.
# Run this over ssh ONLY after Robert said yes. Plain ASCII on purpose.
#
# Usage (from an ssh session on the Windows test machine):
#   powershell -NoProfile -ExecutionPolicy Bypass -File %USERPROFILE%\src\termurion\scripts\probe-desktop.ps1
# Optional: -Repo <folder>  -TimeoutMinutes <n>  -Filters <cargo test filters>
# If the ssh session drops while this runs, nothing cleans up by itself: run
#   schtasks /Query /FO LIST | findstr termurion-probe   then   schtasks /Delete /TN <name> /F
# by hand and look for leftover termurion-probe-* folders under %TEMP% and your home folder.

param(
    [string]$Repo = (Join-Path $env:USERPROFILE 'src\termurion'),
    [int]$TimeoutMinutes = 40,
    [string[]]$Filters = @('webhost_win::probe::', 'webhost_win::tests::menu_gets_our_items')
)

$ErrorActionPreference = 'Continue'
$stamp = Get-Date -Format 'yyyyMMddHHmmss'
$task = "termurion-probe-$stamp"
$dir = Join-Path $env:USERPROFILE "termurion-probe-$stamp"
$taskCreated = $false
$dirCreated = $false
# A run folder that already exists was not made by this run: never use it, never remove it.
if (Test-Path $dir) {
    Write-Output "REFUSED: $dir already exists and was not made by this run. Nothing was started or removed."
    exit 1
}
# Temp user-data folders that existed before this run are never touched; the ones that appear
# during it (the test names them termurion-probe-<pid>-<hex>) are this run's.
$before = @(Get-ChildItem -Path $env:TEMP -Filter 'termurion-probe-*' -ErrorAction SilentlyContinue | ForEach-Object { $_.FullName })

function Get-RunFolders {
    @(Get-ChildItem -Path $env:TEMP -Filter 'termurion-probe-*' -ErrorAction SilentlyContinue | ForEach-Object { $_.FullName } | Where-Object { $before -notcontains $_ })
}

function Get-RunProcesses {
    # Roots: processes whose command line names this run's task, folder or temp user-data folder.
    # Then the descendants of a root: a child counts only when it was created AFTER its parent (a
    # reused PID cannot make an unrelated orphan a descendant).
    $names = @($task) + @(if ($dirCreated) { $dir }) + (Get-RunFolders)
    $all = @(Get-CimInstance Win32_Process)
    $set = @{}
    foreach ($p in $all) {
        $line = $p.CommandLine
        if ($p.ProcessId -ne $PID -and $line -and (@($names | Where-Object { $line -like "*$_*" }).Count -gt 0)) { $set[[int]$p.ProcessId] = $p }
    }
    $grew = $true
    while ($grew) {
        $grew = $false
        foreach ($p in $all) {
            if ($p.ProcessId -eq $PID -or $set.ContainsKey([int]$p.ProcessId)) { continue }
            $parent = $set[[int]$p.ParentProcessId]
            if ($parent -and $p.CreationDate -and $parent.CreationDate -and ($p.CreationDate -ge $parent.CreationDate)) { $set[[int]$p.ProcessId] = $p; $grew = $true }
        }
    }
    @($set.Values)
}

# Roots only, by command line (never by parent): what is left after the collected set was stopped.
function Get-RunStragglers {
    $names = @($task) + @(if ($dirCreated) { $dir }) + (Get-RunFolders)
    $out = @()
    foreach ($p in (Get-CimInstance Win32_Process)) {
        $line = $p.CommandLine
        if ($p.ProcessId -ne $PID -and $line -and (@($names | Where-Object { $line -like "*$_*" }).Count -gt 0)) { $out += $p }
    }
    $out
}

# True when the process in $p (collected earlier) is still the same process: same id, same start time.
function Test-Same($p) {
    $now = Get-CimInstance Win32_Process -Filter "ProcessId = $($p.ProcessId)" -ErrorAction SilentlyContinue
    return [bool]($now -and $now.CreationDate -eq $p.CreationDate)
}

function Stop-Collected($procs) {
    foreach ($p in $procs) {
        if (Test-Same $p) {
            Write-Output "stopping $($p.Name) $($p.ProcessId)"
            Stop-Process -Id $p.ProcessId -Force -ErrorAction SilentlyContinue
        }
    }
}

try {
    New-Item -ItemType Directory -Path $dir -ErrorAction Stop | Out-Null
    $dirCreated = $true
    $filterText = ($Filters -join ' ')
    $lines = @(
        "Set-Location '$Repo'",
        "cargo test --lib -- $filterText --ignored --test-threads=1 --nocapture *> '$dir\out.txt'",
        "'done' | Out-File '$dir\done.txt'"
    )
    Set-Content -Path "$dir\run.ps1" -Value $lines -Encoding ASCII

    # A start time that is already past; the task is only ever started by /Run and is deleted below.
    $past = (Get-Date).AddHours(-1).ToString('HH:mm')
    schtasks /Create /TN $task /SC ONCE /ST $past /SD (Get-Date).AddDays(-1).ToString('MM/dd/yyyy') /IT /RL LIMITED /F /TR "powershell -NoProfile -ExecutionPolicy Bypass -File $dir\run.ps1" | Out-Null
    if ($LASTEXITCODE -ne 0) { throw "could not create the task" }
    $taskCreated = $true
    schtasks /Run /TN $task | Out-Null
    if ($LASTEXITCODE -ne 0) { throw "could not start the task" }

    $deadline = (Get-Date).AddMinutes($TimeoutMinutes)
    while (-not (Test-Path "$dir\done.txt")) {
        if ((Get-Date) -gt $deadline) { Write-Output "TIMEOUT: the probe did not finish in $TimeoutMinutes minutes (a FAIL)"; break }
        Start-Sleep -Seconds 5
    }
    if (Test-Path "$dir\out.txt") {
        Write-Output '----- probe output -----'
        Get-Content "$dir\out.txt"
        Write-Output '----- end of probe output -----'
    } else {
        Write-Output 'no output file was written'
    }
}
catch {
    Write-Output "ERROR: $($_.Exception.Message)"
}
finally {
    # Always. First the run's process tree is collected (its roots are still alive, so the tree can
    # be walked), then the task is ended and deleted, then the collected set is stopped by id (the
    # same start time only), then a command-line scan stops stragglers, then folders go.
    $collected = @(Get-RunProcesses)
    if ($taskCreated) {
        schtasks /End /TN $task 2>&1 | Out-Null
        schtasks /Delete /TN $task /F 2>&1 | Out-Null
    }
    Stop-Collected $collected
    Start-Sleep -Seconds 2
    $stragglers = @(Get-RunStragglers)
    Stop-Collected $stragglers
    Start-Sleep -Seconds 2
    # The test's own user-data folders live under %TEMP% and hold the probe prefix.
    $folderNames = @(Get-RunFolders)
    foreach ($f in $folderNames) { Remove-Item -Recurse -Force $f -ErrorAction SilentlyContinue }
    if ($dirCreated -and (Test-Path $dir)) { Remove-Item -Recurse -Force $dir -ErrorAction SilentlyContinue }

    schtasks /Query /TN $task 2>&1 | Out-Null
    $taskGone = ($LASTEXITCODE -ne 0)
    # Left running: any process stopped above that is still the same process, or a root still named by the command line.
    $still = @(($collected + $stragglers) | Where-Object { Test-Same $_ })
    foreach ($p in (Get-RunStragglers)) {
        $seen = $p.ProcessId
        if (-not (@($still | Where-Object { $_.ProcessId -eq $seen }).Count)) { $still += $p }
    }
    $folders = @()
    if ($dirCreated -and (Test-Path $dir)) { $folders += $dir }
    $folders += @($folderNames | Where-Object { Test-Path $_ })
    $folders += (Get-RunFolders)

    if (@($still).Count -eq 0) { Write-Output 'left running: none' } else { Write-Output "left running: $(@($still).Count) process(es)"; $still | ForEach-Object { Write-Output "  $($_.ProcessId) $($_.Name)" } }
    if ($taskGone) { Write-Output 'left scheduled: none' } else { Write-Output "left scheduled: $task STILL EXISTS" }
    if (@($folders).Count -eq 0) { Write-Output 'left on disk: none' } else { Write-Output 'left on disk:'; $folders | ForEach-Object { Write-Output "  $_" } }
}

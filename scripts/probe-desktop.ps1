# probe-desktop.ps1 - runs the Task 0 isolated-world probe and the Task 1 bridge round trip in the
# CONSOLE session of this machine, through one one-shot scheduled task, then removes every trace.
# The probe window is created by the tests and is never shown.
# Run this over ssh ONLY after Robert said yes. Plain ASCII on purpose.
#
# Usage (from an ssh session on EXTREME):
#   powershell -NoProfile -ExecutionPolicy Bypass -File C:\Users\datbi\src\tandem-13x2\scripts\probe-desktop.ps1
# Optional: -Repo <folder>  -TimeoutMinutes <n>  -Filters <cargo test filters>

param(
    [string]$Repo = 'C:\Users\datbi\src\tandem-13x2',
    [int]$TimeoutMinutes = 40,
    [string[]]$Filters = @('webhost_win::probe::')
)

$ErrorActionPreference = 'Continue'
$stamp = Get-Date -Format 'yyyyMMddHHmmss'
$task = "tandem-probe-$stamp"
$dir = "C:\Users\datbi\tandem-probe-$stamp"
$taskCreated = $false
# Temp user-data folders that existed before this run are never touched; the ones that appear
# during it (the test names them tandem-probe-<pid>-<hex>) are this run's.
$before = @(Get-ChildItem -Path $env:TEMP -Filter 'tandem-probe-*' -ErrorAction SilentlyContinue | ForEach-Object { $_.FullName })

function Get-RunFolders {
    @(Get-ChildItem -Path $env:TEMP -Filter 'tandem-probe-*' -ErrorAction SilentlyContinue | ForEach-Object { $_.FullName } | Where-Object { $before -notcontains $_ })
}

function Get-RunProcesses {
    # Roots: processes whose command line names this run's task, folder or temp user-data folder.
    # Then every descendant of a root (cargo, the test binary, the web view's own children).
    $names = @($dir, $task) + (Get-RunFolders)
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
            if ($p.ProcessId -ne $PID -and -not $set.ContainsKey([int]$p.ProcessId) -and $set.ContainsKey([int]$p.ParentProcessId)) { $set[[int]$p.ProcessId] = $p; $grew = $true }
        }
    }
    @($set.Values)
}

function Stop-RunProcesses {
    foreach ($p in (Get-RunProcesses)) {
        Write-Output "stopping $($p.Name) $($p.ProcessId)"
        Stop-Process -Id $p.ProcessId -Force -ErrorAction SilentlyContinue
    }
}

try {
    New-Item -ItemType Directory -Path $dir | Out-Null
    $filterText = ($Filters -join ' ')
    $lines = @(
        "Set-Location '$Repo'",
        "cargo test --lib $filterText -- --ignored --test-threads=1 --nocapture *> '$dir\out.txt'",
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
    # Always: end then delete the task, confirm it is gone, stop this run's processes, remove folders.
    if ($taskCreated) {
        schtasks /End /TN $task 2>&1 | Out-Null
        schtasks /Delete /TN $task /F 2>&1 | Out-Null
    }
    Stop-RunProcesses
    Start-Sleep -Seconds 2
    # The test's own user-data folders live under %TEMP% and hold the probe prefix.
    foreach ($f in (Get-RunFolders)) { Remove-Item -Recurse -Force $f -ErrorAction SilentlyContinue }
    if (Test-Path $dir) { Remove-Item -Recurse -Force $dir -ErrorAction SilentlyContinue }

    schtasks /Query /TN $task 2>&1 | Out-Null
    $taskGone = ($LASTEXITCODE -ne 0)
    $still = Get-RunProcesses
    $folders = @()
    if (Test-Path $dir) { $folders += $dir }
    $folders += (Get-RunFolders)

    if (@($still).Count -eq 0) { Write-Output 'left running: none' } else { Write-Output "left running: $(@($still).Count) process(es)"; $still | ForEach-Object { Write-Output "  $($_.ProcessId) $($_.Name)" } }
    if ($taskGone) { Write-Output 'left scheduled: none' } else { Write-Output "left scheduled: $task STILL EXISTS" }
    if (@($folders).Count -eq 0) { Write-Output 'left on disk: none' } else { Write-Output 'left on disk:'; $folders | ForEach-Object { Write-Output "  $_" } }
}

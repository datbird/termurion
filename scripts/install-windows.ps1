# Copies the current release build to a stable folder and points a Desktop and a Start Menu
# shortcut at it, so the person can run the latest Termurion without holding a
# lock on target\release (a running termurion.exe there would break the next build).
# Run on the Windows test machine after `scripts/xt build --release` from the main checkout.
# It also removes the old Tandem install, once (see Remove-OldTandem).
$ErrorActionPreference = 'Stop'
$src = Join-Path $env:USERPROFILE 'src\termurion\target\release'
$dest = Join-Path $env:LOCALAPPDATA 'Programs\Termurion'
$files = 'termurion.exe', 'termurion-host.exe', 'termurion-git.exe', 'termurion-utils.exe', 'cc-status.exe', 'conpty.dll', 'OpenConsole.exe'

# Test binaries (target\debug\deps\termurion-<hash>.exe) share the name but lock nothing here.
$running = @(Get-Process -Name 'tandem*', 'termurion*' -ErrorAction SilentlyContinue | Where-Object { $_.Path -notlike '*\target\debug\deps\*' })
if ($running.Count -gt 0) {
    Write-Output ("Still running: " + (($running | ForEach-Object { $_.ProcessName + ' ' + $_.Id }) -join ', ') + ". Close it and run this again.")
    exit 2
}

New-Item -ItemType Directory -Force -Path $dest | Out-Null
foreach ($f in $files) {
    Copy-Item -LiteralPath (Join-Path $src $f) -Destination $dest -Force
    $want = (Get-FileHash -LiteralPath (Join-Path $src $f) -Algorithm SHA256).Hash
    $got = (Get-FileHash -LiteralPath (Join-Path $dest $f) -Algorithm SHA256).Hash
    if ($want -ne $got) { throw "Hash mismatch after copying $f" }
}

$exe = Join-Path $dest 'termurion.exe'
$built = (Get-Item $exe).LastWriteTime
# Installed by the release installer (Inno Setup, its AppId key): its shortcuts and its Apps entry
# stay, and its uninstaller still removes these files (same names). Only the files are replaced.
$inno = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\{329D8970-182C-40BF-A034-89D8AAFB8063}_is1'
if (Test-Path -LiteralPath $inno) {
    Write-Output "Installed to $dest over the release install (built $built)"
    exit 0
}
$shell = New-Object -ComObject WScript.Shell
$startMenu = [Environment]::GetFolderPath('Programs')
foreach ($dir in [Environment]::GetFolderPath('Desktop'), $startMenu) {
    $link = Join-Path $dir 'Termurion.lnk'
    $sc = $shell.CreateShortcut($link)
    $sc.TargetPath = $exe
    $sc.WorkingDirectory = $env:USERPROFILE
    $sc.IconLocation = $exe + ',0'
    $sc.Description = 'Termurion (latest main build)'
    $sc.Save()
    Write-Output "Shortcut $link"
}

$uninst = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\Termurion'
New-Item -Path $uninst -Force | Out-Null
Set-ItemProperty -Path $uninst -Name 'DisplayName' -Value 'Termurion'
Set-ItemProperty -Path $uninst -Name 'DisplayIcon' -Value $exe
Set-ItemProperty -Path $uninst -Name 'InstallLocation' -Value $dest
Set-ItemProperty -Path $uninst -Name 'Publisher' -Value 'Termurion'
Write-Output "Installed to $dest (built $built)"

# One-time cleanup of the old Tandem install (Robert approved a brand new install). Runs after the
# new exe is in place. Every removal is an exact literal path or key, guarded by a test first.
function Remove-OldTandem {
    $local = $env:LOCALAPPDATA
    $roam = $env:APPDATA
    $dirs = @(
        (Join-Path $local 'Programs\Tandem'),
        (Join-Path $roam 'Tandem'),
        (Join-Path $local 'Tandem')
    )
    $links = @(
        (Join-Path ([Environment]::GetFolderPath('Desktop')) 'Tandem.lnk'),
        (Join-Path ([Environment]::GetFolderPath('Programs')) 'Tandem.lnk')
    )
    foreach ($d in $dirs) {
        if (Test-Path -LiteralPath $d) {
            Remove-Item -LiteralPath $d -Recurse -Force
            Write-Output "Removed old folder $d"
        }
    }
    foreach ($l in $links) {
        if (Test-Path -LiteralPath $l) {
            Remove-Item -LiteralPath $l -Force
            Write-Output "Removed old shortcut $l"
        }
    }
    $keys = @(
        'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\Tandem',
        'HKCU:\Software\Tandem',
        'HKCU:\Software\Classes\Applications\tandem.exe'
    )
    # ProgIds named Tandem.<something>: found by an exact prefix on the key names, removed by exact name.
    $classes = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey('Software\Classes')
    if ($classes) {
        foreach ($n in $classes.GetSubKeyNames()) {
            if ($n.StartsWith('Tandem.', [StringComparison]::OrdinalIgnoreCase)) {
                $keys += ('HKCU:\Software\Classes\' + $n)
            }
        }
        $classes.Close()
    }
    foreach ($k in $keys) {
        if (Test-Path -LiteralPath $k) {
            Remove-Item -LiteralPath $k -Recurse -Force
            Write-Output "Removed old registry key $k"
        }
    }
    $reg = 'HKCU:\Software\RegisteredApplications'
    if ((Test-Path -LiteralPath $reg) -and ($null -ne (Get-ItemProperty -LiteralPath $reg -Name 'Tandem' -ErrorAction SilentlyContinue))) {
        Remove-ItemProperty -LiteralPath $reg -Name 'Tandem'
        Write-Output "Removed old RegisteredApplications value Tandem"
    }
    # Credential Manager: only targets that start with Tandem/, Tandem-Browser/ or TandemAI.
    foreach ($line in (cmdkey /list)) {
        if ($line -match '^\s*Target:\s*(.+)$') {
            $t = $Matches[1].Trim()
            $i = $t.IndexOf('target=')
            if ($i -ge 0) { $t = $t.Substring($i + 7) }
            if ($t.StartsWith('Tandem/', [StringComparison]::OrdinalIgnoreCase) -or $t.StartsWith('Tandem-Browser/', [StringComparison]::OrdinalIgnoreCase) -or $t.StartsWith('TandemAI', [StringComparison]::OrdinalIgnoreCase)) {
                cmdkey /delete:"$t" | Out-Null
                Write-Output "Removed old credential $t"
            }
        }
    }
}
Remove-OldTandem

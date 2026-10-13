# Copies the current release build to a stable folder and points a desktop
# shortcut at it, so the person can run the latest Tandem without holding a
# lock on target\release (a running termurion.exe there would break the next build).
# Run on EXTREME after `scripts/xt build --release` from the main checkout.
$ErrorActionPreference = 'Stop'
$src = 'C:\Users\datbi\src\tandem\target\release'
$dest = Join-Path $env:LOCALAPPDATA 'Programs\Tandem'
$files = 'termurion.exe', 'termurion-host.exe', 'termurion-git.exe', 'termurion-utils.exe', 'cc-status.exe', 'conpty.dll', 'OpenConsole.exe'

$running = Get-Process termurion, termurion-host, tandem, tandem-host -ErrorAction SilentlyContinue |
    Where-Object { $_.Path -and $_.Path.StartsWith($dest, [StringComparison]::OrdinalIgnoreCase) }
if ($running) {
    Write-Output "Tandem is running from $dest; not replaced. Close it and run this again."
    exit 2
}

New-Item -ItemType Directory -Force -Path $dest | Out-Null
foreach ($f in $files) {
    Copy-Item -LiteralPath (Join-Path $src $f) -Destination $dest -Force
}

$desktop = [Environment]::GetFolderPath('Desktop')
$link = Join-Path $desktop 'Tandem.lnk'
$shell = New-Object -ComObject WScript.Shell
$sc = $shell.CreateShortcut($link)
$sc.TargetPath = Join-Path $dest 'termurion.exe'
$sc.WorkingDirectory = $env:USERPROFILE
$sc.IconLocation = (Join-Path $dest 'termurion.exe') + ',0'
$sc.Description = 'Tandem (latest main build)'
$sc.Save()

$built = (Get-Item (Join-Path $dest 'termurion.exe')).LastWriteTime
Write-Output "Installed to $dest (built $built); shortcut $link"

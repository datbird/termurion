# Downloads the pinned ConPTY package and copies the x64 files to vendor\conpty.
$ErrorActionPreference = 'Stop'
$version = '1.25.260930003'
$sha256 = '02B07B349AF66D801159BDF9E440D4A1CE78BB951F37FC8609731665AFDAE7EE'
$root = Split-Path -Parent $PSScriptRoot
$dest = Join-Path $root 'vendor\conpty'
$tmp = Join-Path $env:TEMP "conpty-$version"
# Expand-Archive in PowerShell 5.1 only accepts a .zip name.
$pkg = "$tmp.zip"
$url = "https://api.nuget.org/v3-flatcontainer/microsoft.windows.console.conpty/$version/microsoft.windows.console.conpty.$version.nupkg"
Invoke-WebRequest -UseBasicParsing -Uri $url -OutFile $pkg
$got = (Get-FileHash -Algorithm SHA256 $pkg).Hash
if ($got -ne $sha256) { throw "ConPTY package hash mismatch: got $got, want $sha256" }
if (Test-Path $tmp) { Remove-Item -Recurse -Force $tmp }
Expand-Archive -Path $pkg -DestinationPath $tmp
New-Item -ItemType Directory -Force $dest | Out-Null
Copy-Item (Join-Path $tmp 'runtimes\win-x64\native\conpty.dll') $dest -Force
Copy-Item (Join-Path $tmp 'build\native\runtimes\x64\OpenConsole.exe') $dest -Force
# The package carries no license files: the notices are resources\conpty\LICENSE and NOTICE.md (Windows Terminal's own).
Remove-Item -Recurse -Force $tmp, $pkg
Write-Output "ConPTY $version installed in $dest"

# Downloads the pinned ConPTY package and copies the x64 files to vendor\conpty.
$ErrorActionPreference = 'Stop'
$version = '1.24.260710001'
$sha256 = '175640566A3B59C4B132070EE96C2C77E5AB7EDD2E92732A5EB3610BBF63D90E'
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
Remove-Item -Recurse -Force $tmp, $pkg
Write-Output "ConPTY $version installed in $dest"

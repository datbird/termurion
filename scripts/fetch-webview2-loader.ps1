# Copies the pinned WebView2Loader.dll from the webview2-com-sys 0.39.1 crate (already in the cargo
# registry once cargo has fetched it) to vendor\webview2. Downloads nothing. Run `cargo fetch` first.
$ErrorActionPreference = 'Stop'
$crate = 'webview2-com-sys-0.39.1'
$pins = @{
    'x64'   = '86545B66CDB0603BC26B626FB9AD610CB6E71F28D468F5EA66DF23B03DDA96D5'
    'arm64' = 'A0DE1844933A10C898F833771A85F0AB9A0E3C745D4715F63D2D9730F9416025'
}
$root = Split-Path -Parent $PSScriptRoot
$dest = Join-Path $root 'vendor\webview2'
$cargoHome = if ($env:CARGO_HOME) { $env:CARGO_HOME } else { Join-Path $env:USERPROFILE '.cargo' }
$src = Get-ChildItem -Directory (Join-Path $cargoHome 'registry\src\*') -ErrorAction SilentlyContinue | ForEach-Object { Join-Path $_.FullName $crate } | Where-Object { Test-Path $_ } | Select-Object -First 1
if (-not $src) { throw "$crate is not in the cargo registry; run cargo fetch first" }
$arch = if ($env:PROCESSOR_ARCHITECTURE -eq 'ARM64') { 'arm64' } else { 'x64' }
New-Item -ItemType Directory -Force $dest | Out-Null
$from = Join-Path $src "$arch\WebView2Loader.dll"
$got = (Get-FileHash -Algorithm SHA256 $from).Hash
if ($got -ne $pins[$arch]) { throw "WebView2Loader.dll ($arch) hash mismatch: got $got, want $($pins[$arch])" }
Copy-Item $from (Join-Path $dest 'WebView2Loader.dll') -Force
Write-Output "WebView2Loader.dll ($arch) from $crate installed in $dest"

# Windows version of fx-signal.sh: tells Termurion's cursor effects that a prompt was sent
# (enter) or an answer came back (done), with an invisible low-bit change of the cursor color
# (OSC 12), reset after 1.9 s (OSC 112). A Claude Code hook has no console of its own, so this
# attaches to the console of the first ancestor process that has one (claude.exe's).
# Usage: powershell -NoProfile -ExecutionPolicy Bypass -File fx-signal.ps1 enter|done [#rrggbb]
param([string]$Kind = "enter", [string]$Color = "#9fd0fe")
$ErrorActionPreference = "SilentlyContinue"
Add-Type -Namespace TermurionFx -Name Con -MemberDefinition @'
[DllImport("kernel32.dll")] public static extern bool FreeConsole();
[DllImport("kernel32.dll")] public static extern bool AttachConsole(uint pid);
[DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)] public static extern IntPtr CreateFileW(string name, uint access, uint share, IntPtr sec, uint disp, uint flags, IntPtr tmpl);
[DllImport("kernel32.dll", SetLastError = true)] public static extern bool WriteFile(IntPtr h, byte[] buf, uint n, out uint written, IntPtr ov);
[DllImport("kernel32.dll")] public static extern bool CloseHandle(IntPtr h);
public static bool Send(string s) {
  // GENERIC_READ | GENERIC_WRITE, shared, OPEN_EXISTING: the attached console's screen buffer.
  IntPtr h = CreateFileW("CONOUT$", 0xC0000000, 3, IntPtr.Zero, 3, 0, IntPtr.Zero);
  if (h == new IntPtr(-1)) return false;
  byte[] b = System.Text.Encoding.ASCII.GetBytes(s);
  uint n; bool ok = WriteFile(h, b, (uint)b.Length, out n, IntPtr.Zero);
  CloseHandle(h);
  return ok;
}
'@
$hex = $Color.TrimStart('#')
if ($hex.Length -ne 6) { exit 0 }
$r = [Convert]::ToInt32($hex.Substring(0, 2), 16); $g = [Convert]::ToInt32($hex.Substring(2, 2), 16); $b = [Convert]::ToInt32($hex.Substring(4, 2), 16)
if ($Kind -eq "done") { $g = $g -bor 1; $b = $b -band 0xFE } else { $g = $g -band 0xFE; $b = $b -bor 1 }
# Each signal flips red's lowest bit, so a new signal is always a color change.
$state = Join-Path $env:TEMP "termurion-fx-state"
$last = 0; if (Test-Path $state) { $last = [int]((Get-Content $state -Raw).Split(':')[1]) }
$parity = 1 - $last
$r = ($r -band 0xFE) -bor $parity
$token = "$([DateTime]::UtcNow.Ticks):$parity"
Set-Content -NoNewline $state $token
# Claude Code starts hooks with a hidden console of their own (and the shell between gets one
# too), so attach to claude.exe's console: the one Termurion shows. Without a claude.exe
# ancestor, the first ancestor with a console is used.
$chain = @(); $p = $PID
for ($i = 0; $i -lt 16; $i++) {
  $proc = Get-CimInstance Win32_Process -Filter "ProcessId=$p"
  if (-not $proc -or -not $proc.ParentProcessId) { break }
  $p = $proc.ParentProcessId
  $parent = Get-CimInstance Win32_Process -Filter "ProcessId=$p"
  if (-not $parent) { break }
  $chain += [pscustomobject]@{ Id = $p; Name = $parent.Name }
}
$order = @($chain | Where-Object { $_.Name -like "claude*" }) + @($chain | Where-Object { $_.Name -notlike "claude*" })
[TermurionFx.Con]::FreeConsole() | Out-Null
$attached = $false
foreach ($c in $order) {
  if ([TermurionFx.Con]::AttachConsole([uint32]$c.Id)) { $attached = $true; $p = "$($c.Id) $($c.Name)"; break }
}
if ($env:TERMURION_FX_LOG) { Add-Content $env:TERMURION_FX_LOG "$(Get-Date -Format o) $Kind attached=$attached pid=$p" }
if (-not $attached) { exit 0 }
function Send([string]$s) { [TermurionFx.Con]::Send($s) }
$e = [char]27; $bel = [char]7
$sent = Send ("$e]12;#{0:x2}{1:x2}{2:x2}$bel" -f $r, $g, $b)
if ($env:TERMURION_FX_LOG) { Add-Content $env:TERMURION_FX_LOG "  sent=$sent" }
Start-Sleep -Milliseconds 1900
if ((Get-Content $state -Raw) -eq $token) { Send "$e]112$bel" | Out-Null; Remove-Item $state }

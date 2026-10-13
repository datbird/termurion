# Termurion shell integration for PowerShell (Windows PowerShell 5.1 and PowerShell 7).
# Marks each prompt and command with OSC 133 and reports the location with OSC 7, the
# same sequences iTerm2's bash and zsh scripts send. Load it from $PROFILE:
#   . "<path to this file>"

if ($global:__TermurionShellIntegration) { return }
$global:__TermurionShellIntegration = $true

$global:__TermurionSalt = "$(Get-Random)$(Get-Random)"
$global:__TermurionCount = 0
$global:__TermurionAid = ''
$global:__TermurionFirst = $true
$global:__TermurionRan = $false
$global:__TermurionHistory = (Get-History -Count 1).Id
$global:__TermurionOriginalPrompt = $function:prompt

function global:__TermurionOsc([string]$body) {
    return "$([char]27)]$body$([char]7)"
}

# The machine identity OSC 7 carries (iTerm2's machineID, version 1): HMAC-SHA256 of this
# boot's identity, "MachineGuid:BootId", keyed with iTerm2's key, as lowercase hex. Termurion
# computes the same value, so a path reported by this shell is known to be on this machine.
function global:Get-TermurionMachineIdHash([string]$identity) {
    $hmac = New-Object System.Security.Cryptography.HMACSHA256
    try {
        $hmac.Key = [System.Text.Encoding]::UTF8.GetBytes('iterm2-osc7-machine-id')
        $bytes = $hmac.ComputeHash([System.Text.Encoding]::UTF8.GetBytes($identity))
    } finally {
        $hmac.Dispose()
    }
    return (($bytes | ForEach-Object { $_.ToString('x2') }) -join '')
}

# "1:<hex>", or "0:" (identity unavailable, as iTerm2's bash script sends) when either value
# cannot be read. Two registry reads, once, when this file loads.
function global:__TermurionMachineId {
    try {
        $guid = Get-ItemPropertyValue -Path 'HKLM:\SOFTWARE\Microsoft\Cryptography' -Name MachineGuid -ErrorAction Stop
        $boot = Get-ItemPropertyValue -Path 'HKLM:\SYSTEM\CurrentControlSet\Control\Session Manager\Memory Management\PrefetchParameters' -Name BootId -ErrorAction Stop
        if (-not $guid -or $null -eq $boot) { return '0:' }
        $id = '{0}:{1}' -f $guid, ([uint32]([int64]$boot -band 0xFFFFFFFF))
        return '1:' + (Get-TermurionMachineIdHash $id)
    } catch {
        return '0:'
    }
}
$global:__TermurionMachineId = __TermurionMachineId

# OSC 7: file://user@host/C:/path, percent-encoded as UTF-8 except [A-Za-z0-9/._~-].
function global:__TermurionLocation {
    $loc = Get-Location
    if ($loc.Provider.Name -ne 'FileSystem') { return '' }
    $path = '/' + ($loc.ProviderPath -replace '\\', '/')
    $sb = New-Object System.Text.StringBuilder
    foreach ($b in [System.Text.Encoding]::UTF8.GetBytes($path)) {
        $c = [char]$b
        if ($b -lt 128 -and $c -match '[A-Za-z0-9/._~-]') { [void]$sb.Append($c) }
        else { [void]$sb.Append(('%{0:X2}' -f $b)) }
    }
    $user = $env:USERNAME -replace '[^A-Za-z0-9._@-]', ''
    $machine = $env:COMPUTERNAME -replace '[^A-Za-z0-9._-]', ''
    return (__TermurionOsc "7;file://$user@$machine$($sb.ToString())?machineID=$global:__TermurionMachineId")
}

function global:prompt {
    $ok = $?
    $code = 0
    if (-not $ok) { if ($global:LASTEXITCODE) { $code = $global:LASTEXITCODE } else { $code = 1 } }
    $out = ''
    # A command ran since the last prompt when a line was read (PSConsoleHostReadLine below)
    # or history grew. Otherwise this is the same prompt drawn again (PSReadLine
    # InvokePrompt): same aid, and no command end.
    $historyId = (Get-History -Count 1).Id
    $ran = $global:__TermurionRan -or ($historyId -ne $global:__TermurionHistory)
    $global:__TermurionHistory = $historyId
    if ($global:__TermurionFirst -or $ran) {
        if (-not $global:__TermurionFirst) {
            # No line was read (no PSReadLine): mark where output ended, as bash does.
            if (-not $global:__TermurionRan) { $out += (__TermurionOsc "133;C;aid=$global:__TermurionAid") }
            $out += (__TermurionOsc "133;D;$code;aid=$global:__TermurionAid")
        }
        $global:__TermurionCount++
        $global:__TermurionAid = "$global:__TermurionSalt-$global:__TermurionCount"
    }
    $global:__TermurionFirst = $false
    $global:__TermurionRan = $false
    $out += (__TermurionLocation)
    # Start the prompt on a fresh line (iTerm2's Place Prompt at First Column; under ConPTY
    # the terminal cannot add the line itself).
    if ($Host.UI.RawUI.CursorPosition.X -gt 0) { $out += "`r`n" }
    $out += (__TermurionOsc "133;A;aid=$global:__TermurionAid")
    # Give the original prompt the failed state, so prompts that show it (starship,
    # oh-my-posh) still see it.
    if (-not $ok) { Write-Error 'failed' -ErrorAction Ignore }
    $original = $global:__TermurionOriginalPrompt.Invoke() -join ''
    return $out + $original + (__TermurionOsc "133;B;aid=$global:__TermurionAid")
}

# The command starts when a line is read: wrap PSReadLine's reader, so the whole line
# (several lines, or with the cursor mid-line) is the command, and the user's own key
# bindings stay as they are.
if (Get-Command PSConsoleHostReadLine -ErrorAction SilentlyContinue) {
    $global:__TermurionOriginalReadLine = $function:PSConsoleHostReadLine
    function global:PSConsoleHostReadLine {
        $line = & $global:__TermurionOriginalReadLine
        $global:__TermurionRan = $true
        [Console]::Write((__TermurionOsc "133;C;aid=$global:__TermurionAid"))
        return $line
    }
}

$shellName = 'powershell'
if ($PSVersionTable.PSEdition -eq 'Core') { $shellName = 'pwsh' }
[Console]::Write((__TermurionOsc "1337;ShellIntegrationVersion=1;shell=$shellName"))

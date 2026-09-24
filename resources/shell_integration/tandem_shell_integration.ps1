# Tandem shell integration for PowerShell (Windows PowerShell 5.1 and PowerShell 7).
# Marks each prompt and command with OSC 133 and reports the location with OSC 7, the
# same sequences iTerm2's bash and zsh scripts send. Load it from $PROFILE:
#   . "<path to this file>"

if ($global:__TandemShellIntegration) { return }
$global:__TandemShellIntegration = $true

$global:__TandemSalt = "$(Get-Random)$(Get-Random)"
$global:__TandemCount = 0
$global:__TandemAid = ''
$global:__TandemFirst = $true
$global:__TandemRan = $false
$global:__TandemHistory = (Get-History -Count 1).Id
$global:__TandemOriginalPrompt = $function:prompt

function global:__TandemOsc([string]$body) {
    return "$([char]27)]$body$([char]7)"
}

# OSC 7: file://user@host/C:/path, percent-encoded as UTF-8 except [A-Za-z0-9/._~-].
function global:__TandemLocation {
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
    return (__TandemOsc "7;file://$user@$machine$($sb.ToString())?machineID=1:")
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
    $ran = $global:__TandemRan -or ($historyId -ne $global:__TandemHistory)
    $global:__TandemHistory = $historyId
    if ($global:__TandemFirst -or $ran) {
        if (-not $global:__TandemFirst) {
            # No line was read (no PSReadLine): mark where output ended, as bash does.
            if (-not $global:__TandemRan) { $out += (__TandemOsc "133;C;aid=$global:__TandemAid") }
            $out += (__TandemOsc "133;D;$code;aid=$global:__TandemAid")
        }
        $global:__TandemCount++
        $global:__TandemAid = "$global:__TandemSalt-$global:__TandemCount"
    }
    $global:__TandemFirst = $false
    $global:__TandemRan = $false
    $out += (__TandemLocation)
    # Start the prompt on a fresh line (iTerm2's Place Prompt at First Column; under ConPTY
    # the terminal cannot add the line itself).
    if ($Host.UI.RawUI.CursorPosition.X -gt 0) { $out += "`r`n" }
    $out += (__TandemOsc "133;A;aid=$global:__TandemAid")
    # Give the original prompt the failed state, so prompts that show it (starship,
    # oh-my-posh) still see it.
    if (-not $ok) { Write-Error 'failed' -ErrorAction Ignore }
    $original = $global:__TandemOriginalPrompt.Invoke() -join ''
    return $out + $original + (__TandemOsc "133;B;aid=$global:__TandemAid")
}

# The command starts when a line is read: wrap PSReadLine's reader, so the whole line
# (several lines, or with the cursor mid-line) is the command, and the user's own key
# bindings stay as they are.
if (Get-Command PSConsoleHostReadLine -ErrorAction SilentlyContinue) {
    $global:__TandemOriginalReadLine = $function:PSConsoleHostReadLine
    function global:PSConsoleHostReadLine {
        $line = & $global:__TandemOriginalReadLine
        $global:__TandemRan = $true
        [Console]::Write((__TandemOsc "133;C;aid=$global:__TandemAid"))
        return $line
    }
}

$shellName = 'powershell'
if ($PSVersionTable.PSEdition -eq 'Core') { $shellName = 'pwsh' }
[Console]::Write((__TandemOsc "1337;ShellIntegrationVersion=1;shell=$shellName"))

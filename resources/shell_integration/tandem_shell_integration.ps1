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
    if (-not $global:__TandemFirst) {
        # Enter was not pressed (Ctrl+C at the prompt): end the command the way bash does.
        if (-not $global:__TandemRan) { $out += (__TandemOsc "133;C;aid=$global:__TandemAid") }
        $out += (__TandemOsc "133;D;$code;aid=$global:__TandemAid")
    }
    $global:__TandemFirst = $false
    $global:__TandemRan = $false
    $global:__TandemCount++
    $global:__TandemAid = "$global:__TandemSalt-$global:__TandemCount"
    $out += (__TandemLocation)
    $out += (__TandemOsc "133;A;aid=$global:__TandemAid")
    $original = & $global:__TandemOriginalPrompt
    return $out + $original + (__TandemOsc "133;B;aid=$global:__TandemAid")
}

# The command starts when Enter accepts the line.
if (Get-Command Set-PSReadLineKeyHandler -ErrorAction SilentlyContinue) {
    Set-PSReadLineKeyHandler -Chord Enter -ScriptBlock {
        $global:__TandemRan = $true
        [Console]::Write((__TandemOsc "133;C;aid=$global:__TandemAid"))
        [Microsoft.PowerShell.PSConsoleReadLine]::AcceptLine()
    }
}

$shellName = 'powershell'
if ($PSVersionTable.PSEdition -eq 'Core') { $shellName = 'pwsh' }
[Console]::Write((__TandemOsc "1337;ShellIntegrationVersion=1;shell=$shellName"))

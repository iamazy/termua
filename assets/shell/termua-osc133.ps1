# Termua OSC 133 shell integration (PowerShell).
#
# Emits a minimal subset of OSC 133 markers:
# - A: prompt start
# - B: prompt end
# - C: command start
# - D;<exit>: command end

if ($global:TERMUA_OSC133_PWSH_INSTALLED) {
    return
}

$global:TERMUA_OSC133_PWSH_INSTALLED = $true
$global:TERMUA_OSC133_COMMAND_ACTIVE = $false
$global:TERMUA_OSC133_ORIG_PROMPT = ${function:prompt}

function global:__termua_osc133_print {
    param([string]$Payload)

    $esc = [char]27
    $bel = [char]7
    [Console]::Out.Write("$esc]133;$Payload$bel")
}

function global:prompt {
    if ($global:TERMUA_OSC133_COMMAND_ACTIVE) {
        $exitCode = if ($global:LASTEXITCODE -is [int]) {
            $global:LASTEXITCODE
        } elseif ($?) {
            0
        } else {
            1
        }
        __termua_osc133_print "D;$exitCode"
        $global:TERMUA_OSC133_COMMAND_ACTIVE = $false
    }

    __termua_osc133_print "A"

    if ($global:TERMUA_OSC133_ORIG_PROMPT) {
        & $global:TERMUA_OSC133_ORIG_PROMPT
    } else {
        "PS $($executionContext.SessionState.Path.CurrentLocation)$('>' * ($nestedPromptLevel + 1)) "
    }
}

if (Get-Command Set-PSReadLineKeyHandler -ErrorAction SilentlyContinue) {
    Set-PSReadLineKeyHandler -Chord Enter -ScriptBlock {
        param($key, $arg)

        __termua_osc133_print "B"
        __termua_osc133_print "C"
        $global:TERMUA_OSC133_COMMAND_ACTIVE = $true
        [Microsoft.PowerShell.PSConsoleReadLine]::AcceptLine()
    }
}

# _shim.ps1 -- the one place PowerShell touches a ported tool.
#
# Every ported tool has exactly one implementation (a .py) and a bash entry (a .sh). The
# .ps1 of the same name is a compatibility shim that forwards argv and nothing else. If a
# shim ever grows logic, we are back to two implementations drifting apart -- which is the
# failure mode this toolchain exists to prevent.
#
# Why a gate instead of a warning: Windows PowerShell 5.1 decodes UTF-8 as ANSI, so it
# corrupts both reading and writing, and its ConvertTo-Json has no escape option. A loud
# refusal at the entry point is far cheaper than silent bad data downstream.

if ($PSVersionTable.PSVersion.Major -lt 6) {
    "本仓工具链不再支持 Windows PowerShell 5.1（当前 $($PSVersionTable.PSVersion)）"
    "请改用 bash 入口： tools/<tool>.sh ...  或 pwsh tools/<tool>.ps1 ..."
    exit 9
}

# Pick the first candidate that actually executes: a `python3` on PATH can be the Windows
# Store alias stub, which resolves but prints nothing, so bare existence is no proof.
$script:PyExe = $null
foreach ($c in @($env:PYTHON, 'python3', 'python')) {
    if (-not $c) { continue }
    $src = (Get-Command $c -ErrorAction SilentlyContinue).Source
    if (-not $src) { continue }
    $probe = & $src -c 'import sys;print(sys.version_info[0])' 2>$null
    if ($probe -eq '3') { $script:PyExe = $src; break }
}

function Invoke-PyModule([string]$module, [string[]]$argv) {
    if (-not $script:PyExe) {
        "找不到可用的 Python 3（设置 `$env:PYTHON，或改用 bash 入口）"
        exit 4
    }
    & $script:PyExe (Join-Path $PSScriptRoot $module) @argv
    $code = $LASTEXITCODE
    if ($null -eq $code) { $code = 0 }
    exit $code
}

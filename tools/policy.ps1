#requires -Version 6.0
# policy.ps1 -- DEPRECATED shim. Implementation is policy.py; entry is policy.sh.
#
# PowerShell is where the corruption came from (5.1 decodes UTF-8 as ANSI and can break
# JSON on read), so there is exactly one implementation and this file only forwards argv.
# Keep it that way: logic here means two behaviours that drift apart silently.

$ErrorPreference = 'Stop'
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot '_shim.ps1')
Invoke-PyModule 'policy.py' $args
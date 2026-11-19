#requires -Version 6.0
# agent-verify.ps1 -- DEPRECATED shim. Implementation is agent_verify.py; entry is agent-verify.sh.
#
# PowerShell is where the corruption came from (5.1 decodes UTF-8 as ANSI and can break
# JSON on read), so there is exactly one implementation and this file only forwards argv.
# Keep it that way: logic here means two behaviours that drift apart silently.

$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot '_shim.ps1')
Invoke-PyModule 'agent_verify.py' $args
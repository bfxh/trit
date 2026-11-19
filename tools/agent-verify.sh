#!/usr/bin/env bash
# agent-verify.sh -- the single entry point. The order of steps IS the safety property.
#
#   tools/agent-verify.sh -Root <dir> -Changed src/a.rs -Lang rust -Profile workspace-write
#   tools/agent-verify.sh -Root <dir> -Changed pkg/core.py -Lang python -DryRun
#   tools/agent-verify.sh ... -Receipt receipt.json
#
# Steps, in this order and no other:
#   memory-brief -> timeline-due -> preflight -> affected -> policy -> constraints
#   -> journal-begin -> run -> journal-end -> memory-note -> timeline-record
#
# Exit codes: 0 all good, 1 a guard refused or the command failed, 2 bad invocation.
# Any refusal stops the chain BEFORE the run step; a receipt is still written, because a
# refusal is evidence too.

set -uo pipefail
here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
. "$here/_python.sh"
require_python agent-verify
exec "$PY" "$here/agent_verify.py" "$@"

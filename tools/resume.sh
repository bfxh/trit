#!/usr/bin/env bash
# resume.sh -- default entry point for the per-task session ledger (suspend/continue).
#
# usage:
#   tools/resume.sh begin    <task-id> [--budget-token N] [-Root <dir>]
#   tools/resume.sh progress <task-id> <step-id> [--note s] [--proven|--unproven]
#   tools/resume.sh suspend  <task-id> --reason budget|deadline|manual
#   tools/resume.sh resume   <task-id>     (state + verbatim continuation block)
#   tools/resume.sh finish   <task-id>    / status <task-id> / list
#
# -Root defaults to the current directory. Sessions live at <Root>/.agent/resume/ and
# contain no machine paths -- they are the thing a fresh session reads to continue.

set -uo pipefail
here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
. "$here/_python.sh"
require_python resume
exec "$PY" "$here/resume.py" "$@"

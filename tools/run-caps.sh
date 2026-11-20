#!/usr/bin/env bash
# run-caps.sh -- default entry point for kernel-level resource caps around one command.
#
# usage:
#   tools/run-caps.sh --mem-mb N --procs N --seconds S -- <cmd> [args...]
#
# Kernel-level caps (memory/process count on Windows), a parent-side watchdog for the
# time cap, exit 124 on timeout -- but NOT a security boundary: no filesystem/network
# isolation, no defense against deliberate escape. policy gates, run-caps contains.
# See run_caps.py's docstring for the full honest strength statement.

set -uo pipefail
here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
. "$here/_python.sh"
require_python run-caps
exec "$PY" "$here/run_caps.py" "$@"

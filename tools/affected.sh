#!/usr/bin/env bash
# affected.sh -- default entry for the substring-selection planner.
#
# usage:
#   tools/affected.sh -Path <root> -Changed <file>[,<file>] -Lang rust|python|node \
#                     [-Out plan.json] [-AssumeIsolated]
#
# The contract lives in affected.py: a subset plan may only claim "equivalent to full"
# when it carries the reachable set, and anything unmodelled must degrade to a full run.

set -uo pipefail
here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
. "$here/_python.sh"
require_python affected
exec "$PY" "$here/affected.py" "$@"

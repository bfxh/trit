#!/usr/bin/env bash
# refactor-guard.sh -- default entry point for "did this refactor change anything it should not".
#
#   tools/refactor-guard.sh preflight -Repo <dir>       # cheap, no compiler
#   tools/refactor-guard.sh snapshot  -Repo <dir>
#   tools/refactor-guard.sh check     -Repo <dir>
#
# preflight is the one that runs before every write: a module missing from a `mod` declaration
# never gets compiled, so the suite stays green while quietly shrinking.

set -uo pipefail
here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
. "$here/_python.sh"
require_python refactor-guard
exec "$PY" "$here/refactor_guard.py" "$@"

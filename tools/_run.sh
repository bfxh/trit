#!/usr/bin/env bash
# _run.sh -- default entry point for "run this one approved command here".
#
#   tools/_run.sh -Root <dir> -Command "cargo test"

set -uo pipefail
here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
. "$here/_python.sh"
require_python _run
exec "$PY" "$here/_run.py" "$@"

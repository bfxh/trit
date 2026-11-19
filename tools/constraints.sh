#!/usr/bin/env bash
# constraints.sh -- default entry point for the repo's forbidden-operations table.
#
# usage:
#   tools/constraints.sh init  -Root <dir>
#   tools/constraints.sh list  -Root <dir>
#   tools/constraints.sh check -Root <dir> [-Diff <patch-file>] [-Allow id1,id2]
#
# `check` reads `git diff --unified=0 HEAD` unless -Diff points at a patch file.

set -uo pipefail
here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
. "$here/_python.sh"
require_python constraints
exec "$PY" "$here/constraints.py" "$@"

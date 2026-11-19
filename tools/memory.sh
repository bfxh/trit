#!/usr/bin/env bash
# memory.sh -- default entry point for workspace memory.
#
# usage:
#   tools/memory.sh init   -Root <dir> [-Kernel]
#   tools/memory.sh note   -Root <dir> -Kind pitfall|decision|fact|todo|command \
#                          -Title "..." -Detail "..." -Evidence "cmd | exit 1 | commit abc" \
#                          [-Tags "..."] [-Scope project|kernel]
#   tools/memory.sh revise -Root <dir> -Id mXXXX [-Title][-Detail][-Evidence][-Tags]
#   tools/memory.sh brief  -Root <dir> [-MaxLines 40]
#   tools/memory.sh recall -Root <dir> -Query "<词>"
#   tools/memory.sh audit  -Root <dir>
#   tools/memory.sh list   -Root <dir>

set -uo pipefail
here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
. "$here/_python.sh"
require_python memory
exec "$PY" "$here/memory.py" "$@"

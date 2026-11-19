#!/usr/bin/env bash
# timeline.sh -- default entry point for the problem ledger.
#
# usage:
#   tools/timeline.sh open    -Root <dir> -Kind boundary -Title "..." -Evidence "..." [-ReviewAfterDays 14] [-Now ISO]
#   tools/timeline.sh touch   -Root <dir> -Id bXXXX -Evidence "..."
#   tools/timeline.sh resolve -Root <dir> -Id bXXXX -Evidence "..."
#   tools/timeline.sh accept  -Root <dir> -Id bXXXX -ReviewAfterDays 30 -Evidence "..."
#   tools/timeline.sh obsolete -Root <dir> -Id bXXXX
#   tools/timeline.sh due     -Root <dir> [-Now ISO]
#   tools/timeline.sh trend   -Root <dir> -Id bXXXX
#   tools/timeline.sh list    -Root <dir>

set -uo pipefail
here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
. "$here/_python.sh"
require_python timeline
exec "$PY" "$here/timeline.py" "$@"

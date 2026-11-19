#!/usr/bin/env bash
# checks.sh -- default entry for the check registry.
#
# WHY A .sh AND A .py: the logic lives in exactly one place (checks.py); this file only
# forwards. Windows users get a .ps1 that forwards the same way. No state, no branching,
# so the two entries can never drift into two different behaviours.
#
# USAGE
#   tools/checks.sh run            # fast tier, this is what CI runs
#   tools/checks.sh run -Only L56  # by check id or by the ledger entry it covers
#   tools/checks.sh audit -Strict  # fail while any ledger entry is prose-only
#   tools/checks.sh list
set -u
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
. "$here/_python.sh"
require_python "checks.sh"
exec "$PY" "$here/checks.py" "$@"
</content>

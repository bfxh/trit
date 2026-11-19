#!/usr/bin/env bash
# ledger.sh -- default entry point for cost accounting (tokens and wall-clock, kept apart).
#
#   tools/ledger.sh add    -Ledger .ledger.json -Task split -Key "obligation.rs" -Tokens 18400 -Wall 12.6
#   tools/ledger.sh report -Ledger .ledger.json
#
# A key recorded twice is recorded once: re-running a step must not inflate the bill.

set -uo pipefail
here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
. "$here/_python.sh"
require_python ledger
exec "$PY" "$here/ledger.py" "$@"

#!/usr/bin/env bash
# journal.sh -- default entry point for workspace checkpointing.
#
# usage:
#   tools/journal.sh begin   -Root <dir> [-Store <dir>]
#   tools/journal.sh end     -Root <dir>     (exit 1 if anything changed)
#   tools/journal.sh restore -Root <dir>
#   tools/journal.sh status  -Root <dir>
#
# -Store defaults to <Root>/.journal and deliberately lives outside a repo you might
# revert: a snapshot inside the tree it protects gets copied or deleted by the same
# careless command it exists to undo.

set -uo pipefail
here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
. "$here/_python.sh"
require_python journal
exec "$PY" "$here/journal.py" "$@"

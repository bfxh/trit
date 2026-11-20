#!/usr/bin/env bash
# trash.sh -- default entry point for the workspace recycle bin.
#
# usage:
#   tools/trash.sh put <path>... -Reason <why> [-Root <dir>] [-Store <dir>]
#   tools/trash.sh list [--json] [-Root <dir>]
#   tools/trash.sh restore <entry-id> [-Into <dir>] [-Root <dir>]
#   tools/trash.sh purge <entry-id> -Confirm <entry-id> [-Root <dir>]
#   tools/trash.sh status [-Root <dir>]
#
# The bin site shares the journal snapshot's store (-Store override, else <Root>/.journal)
# and must never show up in `git status`; put refuses when it would. Nothing is deleted
# outright: restore brings content back, purge demands the entry id twice.

set -uo pipefail
here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
. "$here/_python.sh"
require_python trash
exec "$PY" "$here/trash.py" "$@"

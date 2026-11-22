#!/usr/bin/env bash
# watch.sh -- default entry point for the event ledger and observation face.
#
# usage:
#   tools/watch.sh record <event> [-AgentId <s>] [-Turn N] [-Ok | -Fail] [-Note <s>] -Root <dir>
#   tools/watch.sh report [--Json] [-AgentId <s>] -Root <dir>
#   tools/watch.sh health [--Json] -Root <dir>
#   tools/watch.sh pollution [--Json] -Root <dir>
#
# Events append to <Root>/.agent/watch.jsonl (committed with the repo, so record refuses
# machine-absolute paths). watch observes and reminds; policy/constraints block.

set -uo pipefail
here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
. "$here/_python.sh"
require_python watch
exec "$PY" "$here/watch.py" "$@"

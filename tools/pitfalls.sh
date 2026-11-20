#!/usr/bin/env bash
# pitfalls.sh -- entry point for the collaboration-pain corpus (tools/pitfalls.py).
#
# usage:
#   tools/pitfalls.sh add --cat <git|github|infra|api|semantics> --trigger <s>
#                        --consequence <s> --prevention <s> [--source <s>] [-Root <dir>]
#   tools/pitfalls.sh list [--cat <c>] [-Root <dir>]
#   tools/pitfalls.sh export --jsonl <path> [--cat <c>] [-Root <dir>]
#   tools/pitfalls.sh count [-Root <dir>]
#   tools/pitfalls.sh seed [-Root <dir>]
#
# The corpus lives at <Root>/.agent/pitfalls.jsonl and is meant to be committed: it is
# what the next coding model reads about what actually goes wrong. seed is idempotent
# by id; entries carrying a machine user path are refused (privacy gate, L93).

set -uo pipefail
here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
. "$here/_python.sh"
require_python pitfalls
exec "$PY" "$here/pitfalls.py" "$@"

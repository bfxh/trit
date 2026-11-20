#!/usr/bin/env bash
# declutter.sh -- default entry point for the comment declutter tool.
#
# usage:
#   tools/declutter.sh audit [--json] [-Root <dir>]
#   tools/declutter.sh budget init|check [-Root <dir>]
#   tools/declutter.sh export --strip --out <dir> [-Root <dir>]
#   tools/declutter.sh clean --in-place [-Root <dir>]
#
# Fewer comments in the codebase: explanations live in docs/, models read code. Comments
# carrying the LANDMINE marker are never stripped and are listed separately by audit.
# export never touches the source tree; clean is gated behind --in-place and moves every
# original into the trash bin (tools/trash.sh) before rewriting it.

set -uo pipefail
here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
. "$here/_python.sh"
require_python declutter
exec "$PY" "$here/declutter.py" "$@"

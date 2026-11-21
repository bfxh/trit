#!/usr/bin/env bash
# git-safe.sh -- default entry point for git checkpoint/rescue.
#
# usage:
#   tools/git-safe.sh checkpoint [--Label <name>] -Root <dir>
#   tools/git-safe.sh list   -Root <dir>
#   tools/git-safe.sh rescue -Root <dir>
#   tools/git-safe.sh verify <ref> -Root <dir>
#
# checkpoint hangs the workspace (untracked non-ignored files included) onto
# refs/recycle/checkpoint/<ts>-<label> via a temporary index; HEAD, the real index and
# the worktree are never touched. rescue prints copy-paste restore commands.

set -uo pipefail
here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
. "$here/_python.sh"
require_python git-safe
exec "$PY" "$here/git_safe.py" "$@"

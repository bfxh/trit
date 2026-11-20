#!/usr/bin/env bash
# signal.sh -- entry point for the task/emotion triage and the penalty ledger.
#
# usage:
#   tools/signal.sh classify (--text <s> | --file <path>) [--rules <inline-json|path>] --root <dir>
#   tools/signal.sh penalty --score <1-3> --reason <why> [--now <iso>] --root <dir>
#   tools/signal.sh brief [--now <iso>] --root <dir>
#   tools/signal.sh export --jsonl <path> [--with-text] --root <dir>
#
# classify is rule triage in milliseconds (no API anywhere); penalty/brief/export are the
# harness-side signal exits. The store is <root>/.agent/signals.jsonl; export is the
# privacy gate -- default output carries no text, --with-text is the explicit opt-in.

set -uo pipefail
here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
. "$here/_python.sh"
require_python signal
exec "$PY" "$here/signal.py" "$@"

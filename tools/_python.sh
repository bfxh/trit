#!/usr/bin/env bash
# _python.sh -- shared interpreter discovery for every bash entry point.
#
# Sourced, not executed. Keeps the "which python is real" question in one place: a
# `python3` on PATH can be the Windows Store alias stub, which resolves but prints
# nothing, so bare existence is no proof. Set PYTHON=/path/to/python to override.

pick_python() {
    local c
    for c in "${PYTHON:-}" python3 python py; do
        [ -n "$c" ] || continue
        if command -v "$c" >/dev/null 2>&1 &&
            "$c" -c 'import sys; sys.exit(0 if sys.version_info >= (3, 8) else 1)' >/dev/null 2>&1; then
            printf '%s\n' "$c"
            return 0
        fi
    done
    return 1
}

require_python() {
    local tool=$1
    PY=$(pick_python) || {
        printf '%s: 需要 Python 3.8+（或设置 PYTHON=/path/to/python）\n' "$tool" >&2
        exit 4
    }
}

#!/usr/bin/env bash
# policy.sh -- default entry point for the pre-execution permission check.
#
# usage:
#   tools/policy.sh -Profile read-only -Command "curl https://example.com"
#   tools/policy.sh -Profile workspace-write -Root <dir> -Command "cargo fmt"
#   tools/policy.sh ... -AllowNetwork          # explicit, per-invocation grant
#   tools/policy.sh ... -Explain               # print the enforcement-strength boundary
#
# Exit codes: 0 allow, 1 deny, 2 cannot decide (empty command / bad root). 2 is not "allow
# with a caveat" -- an undecidable command must not run.

set -uo pipefail
here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
. "$here/_python.sh"
require_python policy
exec "$PY" "$here/policy.py" "$@"

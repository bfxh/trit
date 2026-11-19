#!/usr/bin/env bash
# costprobe.sh -- default entry for the edit->verify cost measurement.
#
# usage:
#   tools/costprobe.sh -Path <root> [-Out cost.json] [-Model cost.json] \
#                      [-IncrementalRuns 3] [-WorkRoot <dir>]
#
# cwd-relative invocation is fine; paths are resolved inside costprobe.py.

set -uo pipefail
here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
. "$here/_python.sh"
require_python costprobe
exec "$PY" "$here/costprobe.py" "$@"

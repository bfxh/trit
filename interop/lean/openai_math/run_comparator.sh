#!/usr/bin/env bash
# Run one openai/math Comparator challenge on Linux. This is a manual, trusted-corpus
# check: it does not sandbox the solution with systemd-run/landrun beyond Comparator.
set -euo pipefail

HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
PYTHON=${PYTHON:-}
if [ -z "$PYTHON" ] || ! "$PYTHON" -c 'import sys' >/dev/null 2>&1; then
  PYTHON=""
  for candidate in python3 python; do
    if command -v "$candidate" >/dev/null 2>&1 && "$candidate" -c 'import sys' >/dev/null 2>&1; then
      PYTHON=$candidate
      break
    fi
  done
fi
if [ -z "$PYTHON" ]; then
  echo "no working python3/python on PATH" >&2
  exit 1
fi

REPO=${OPENAI_MATH_REPO:-}
CHALLENGE=""
OUT=""
while [ $# -gt 0 ]; do
  case "$1" in
    --repo) REPO=${2:?--repo requires a path}; shift 2 ;;
    --out) OUT=${2:?--out requires a path}; shift 2 ;;
    -h|--help) echo "usage: $0 --repo PATH [--out JSON] ComparatorChallenges/Name.json"; exit 0 ;;
    *) CHALLENGE=$1; shift ;;
  esac
done

if [ -z "$REPO" ] || [ -z "$CHALLENGE" ]; then
  echo "usage: $0 --repo PATH [--out JSON] ComparatorChallenges/Name.json" >&2
  exit 2
fi
if [ "$(uname -s)" != "Linux" ]; then
  echo "comparator path requires Linux (landrun); current: $(uname -s)" >&2
  exit 2
fi
for tool in lake comparator landrun lean4export; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "missing $tool on PATH (see openai/math lean/ComparatorChallenges/README.md)" >&2
    exit 2
  fi
done

PIN="$HERE/pin.json"
if command -v cygpath >/dev/null 2>&1; then
  PIN_NATIVE=$(cygpath -w "$PIN")
else
  PIN_NATIVE=$PIN
fi
COMMIT=$("$PYTHON" -c 'import json,sys; print(json.load(open(sys.argv[1]))["commit"])' "$PIN_NATIVE") || {
  echo "cannot read pin: $PIN" >&2
  exit 2
}
TOOLCHAIN=$("$PYTHON" -c 'import json,sys; print(json.load(open(sys.argv[1]))["lean_toolchain"])' "$PIN_NATIVE") || {
  echo "cannot read pin: $PIN" >&2
  exit 2
}
ACTUAL=$(git -C "$REPO" rev-parse HEAD)
if [ "$ACTUAL" != "$COMMIT" ]; then
  echo "checkout $ACTUAL is not pinned $COMMIT; use fetch_sparse.sh and a full checkout first" >&2
  exit 2
fi
if [ "$(cat "$REPO/lean/lean-toolchain")" != "$TOOLCHAIN" ]; then
  echo "unexpected lean-toolchain in $REPO/lean" >&2
  exit 2
fi

if [ -z "$OUT" ]; then
  OUT="comparator-$(basename "$CHALLENGE" .json)-$(date -u +%Y%m%dT%H%M%SZ).json"
fi
mkdir -p "$(dirname "$OUT")"
OUT="$(cd "$(dirname "$OUT")" && pwd)/$(basename "$OUT")"
LOG="${OUT%.json}.log"

cd "$REPO/lean"
lake update
lake exe cache get
START=$(date +%s)
set +e
lake env comparator "$CHALLENGE" 2>&1 | tee "$LOG"
STATUS=${PIPESTATUS[0]}
set -e
DURATION=$(( $(date +%s) - START ))

"$PYTHON" - "$OUT" "$CHALLENGE" "$COMMIT" "$TOOLCHAIN" "$STATUS" "$DURATION" "$LOG" <<'PY'
import json, pathlib, sys
out, challenge, commit, toolchain, status, duration, log = sys.argv[1:]
pathlib.Path(out).write_text(json.dumps({
    "schema": "trit.openai-math-comparator/v1",
    "commit": commit,
    "toolchain": toolchain,
    "challenge": challenge,
    "exit_code": int(status),
    "status": "pass" if int(status) == 0 else "fail",
    "duration_seconds": int(duration),
    "log": log,
    "evidence": "A (local run; not a CI result unless dispatched there)",
}, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
PY
echo "comparator: exit=$STATUS duration=${DURATION}s report=$OUT"
exit "$STATUS"

#!/usr/bin/env bash
# Sparse-fetch the pinned openai/math catalogue without downloading Mathlib or OAI blobs.
set -euo pipefail

DEST=${1:?usage: fetch_sparse.sh DEST}
HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
PIN="$HERE/pin.json"
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
if command -v cygpath >/dev/null 2>&1; then
  PIN_NATIVE=$(cygpath -w "$PIN")
else
  PIN_NATIVE=$PIN
fi

REPO=$("$PYTHON" -c 'import json,sys; print(json.load(open(sys.argv[1]))["repo"])' "$PIN_NATIVE") || {
  echo "cannot read pin: $PIN" >&2
  exit 1
}
COMMIT=$("$PYTHON" -c 'import json,sys; print(json.load(open(sys.argv[1]))["commit"])' "$PIN_NATIVE") || {
  echo "cannot read pin: $PIN" >&2
  exit 1
}

if [ ! -d "$DEST/.git" ]; then
  git clone --filter=blob:none --no-checkout --depth 1 "$REPO" "$DEST"
fi
git -C "$DEST" fetch --depth 1 origin "$COMMIT"
git -C "$DEST" sparse-checkout init --no-cone
cat > "$DEST/.git/info/sparse-checkout" <<'PATTERNS'
/lean/formalization.yaml
/lean/lakefile.lean
/lean/lean-toolchain
/lean/README.md
/lean/ComparatorChallenges/
PATTERNS
git -C "$DEST" checkout --detach "$COMMIT"

ACTUAL=$(git -C "$DEST" rev-parse HEAD)
if [ "$ACTUAL" != "$COMMIT" ]; then
  echo "checkout mismatch: $ACTUAL != $COMMIT" >&2
  exit 1
fi
echo "openai/math $COMMIT -> $DEST"

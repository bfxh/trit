#!/usr/bin/env bash
# 按依赖顺序发布到 crates.io；需要先 `cargo login`。
# 用法：bash scripts/publish.sh [--dry-run]
set -euo pipefail
cd "$(dirname "$0")/.." || exit 1

dry=0
[ "${1:-}" = "--dry-run" ] && dry=1

crates=(trit-core trit-export trit-eval trit-sat trit-circuit trit-bdd trit-belnap trit-policy trit-cli trit-wasm sansheng)
for c in "${crates[@]}"; do
  echo "== $c =="
  if [ "$dry" -eq 1 ]; then
    cargo publish -p "$c" --dry-run --locked
  else
    cargo publish -p "$c" --locked
  fi
done
echo "done"

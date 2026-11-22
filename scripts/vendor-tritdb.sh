#!/usr/bin/env bash
# 从 trit-db 重新导出 third_party/trit-db（离线构建用）。
#
#   scripts/vendor-tritdb.sh [rev]
#
# 默认 rev 是当前 vendored 的版本；TRIT_DB_SRC 必须指向上游工作副本（无默认值）。
set -euo pipefail

rev="${1:-aca718880e0d918a064c878b39564225e46db80a}"
src="${TRIT_DB_SRC:?请设置 TRIT_DB_SRC 指向 trit-db 工作副本}"
root="$(cd "$(dirname "$0")/.." && pwd)"
out="$root/third_party/trit-db"

crates=(
    tritdb-radix
    tritdb-page
    tritdb-wal
    tritdb-txn
    tritdb-exec
    tritdb-sql
    tritdb-testkit
    tritdb-cas
    tritdb-pgwire
    tritdb-tritvec
)

if ! git -C "$src" cat-file -e "$rev^{commit}" 2>/dev/null; then
    echo "找不到 $src 的提交 $rev；先在那里 fetch，或设置 TRIT_DB_SRC" >&2
    exit 1
fi

rm -rf "$out/crates"
mkdir -p "$out/crates"
for crate in "${crates[@]}"; do
    git -C "$src" archive "$rev" "crates/$crate" | tar -x -C "$out"
done
git -C "$src" show "$rev:NOTICE.md" > "$out/NOTICE.md"
git -C "$src" show "$rev:rustfmt.toml" > "$out/rustfmt.toml"

echo "vendored trit-db@$rev into $out"
echo "接着跑：cargo test -p trit-orch --features tritdb-pgwire && python scripts/module_budget.py --update"

#!/usr/bin/env bash
# 小型基准：解析 / 时序 / 映射 / ATPG。用法：bash scripts/bench.sh
set -euo pipefail
cd "$(dirname "$0")/.." || exit 1
cargo bench -p trit-circuit --bench core "$@"

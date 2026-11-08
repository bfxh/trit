#!/usr/bin/env bash
# 一键资产管线：OBJ → 表面体素 → 三值高斯头 → PLY → gauss2mesh → OBJ → UV。
#
# 用法：bash crates/trit-3d/scripts/asset_pipeline.sh <in.obj> <out_dir> [gaussians] [grid]
# 环境变量：GAUSS2MESH / UVUNWRAP 可覆盖外部二进制路径。
set -euo pipefail

if [ $# -lt 2 ]; then
  echo "用法: asset_pipeline.sh <in.obj> <out_dir> [gaussians] [grid]" >&2
  exit 2
fi

ROOT="$(cd "$(dirname "$0")/../../.." && pwd)"
OBJ="$1"
OUT="$2"
N="${3:-4096}"
GRID="${4:-128}"
GAUSS2MESH="${GAUSS2MESH:-E:/cargo-target/LSSMJ-mat/release/gauss2mesh.exe}"
UVUNWRAP="${UVUNWRAP:-C:/Users/lbx13/.cache/cargo-target/LSSMJ-mat/release/uvunwrap.exe}"

[ -f "$OBJ" ] || { echo "找不到输入 OBJ: $OBJ" >&2; exit 1; }
[ -x "$GAUSS2MESH" ] || { echo "找不到 gauss2mesh: $GAUSS2MESH" >&2; exit 1; }
[ -x "$UVUNWRAP" ] || { echo "找不到 uvunwrap: $UVUNWRAP" >&2; exit 1; }

mkdir -p "$OUT"
cd "$ROOT"

echo "[1/4] 训练三值高斯头（n=$N）..."
cargo run -q -p trit-3d --example real_asset -- "$OBJ" "$OUT" "$N"

PLY="$OUT/real_asset_gaussians.ply"
echo "[2/4] gauss2mesh（grid=$GRID）..."
"$GAUSS2MESH" convert "$PLY" --grid "$GRID" --largest-component --out "$OUT/asset.obj" | tail -4

echo "[3/4] UV（LSCM + 接缝 + 重叠检测）..."
"$UVUNWRAP" "$OUT/asset.obj" "$OUT/asset_uv.obj"

echo "[4/4] 完成: $OUT/asset_uv.obj"

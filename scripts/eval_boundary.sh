#!/usr/bin/env bash
# 边界头 → 2D 评测 → 3D 分片 的一条龙对照。用法：
#   scripts/eval_boundary.sh <pb.tpm> <tag>
# 需要环境变量（默认值见下）：
#   RENDERS / SCENES  留出场景的根目录；HOLD 逗号分隔的场景名
# 输出：2D 指标（mIoU / pred-gt / 计数）+ 3 个场景的 3D 部件数
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
model="${1:?用法: eval_boundary.sh <pb.tpm> <tag>}"
tag="${2:-run}"
renders="${RENDERS:?设 RENDERS}"
scenes="${SCENES:?设 SCENES}"
hold="${HOLD:?设 HOLD（逗号分隔场景名）}"
out="$root/runs/eval-$tag"
mkdir -p "$out"

echo "== 2D =="
"$root/target/release/parts-inst.exe" eval \
  --model "$model" --renders "$renders" --scenes "$scenes" \
  --gt part --boundary --k 5 --res 384 \
  --dump "$out/inst" --dump-n 1 | tee "$out/eval2d.txt" | tail -1

echo "== 3D（前 3 个留出场景，帧 0）=="
i=0
IFS=',' read -ra HS <<< "$hold"
for s in "${HS[@]}"; do
  [ "$i" -ge 3 ] && break
  "$root/target/release/parts-depth.exe" mesh-single \
    --dcn "${DCN:?设 DCN}" --fg "$model" \
    --inst "$out/inst/${s}_000000.npy" \
    --scene "$renders/$s" --frame 0 --cell 0.02 \
    --out "$out/$s.ply" --split-dir "$out/parts-$s" | tail -2
  i=$((i + 1))
done

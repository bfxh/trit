#!/usr/bin/env bash
# 边界头 → 2D 评测 → 逐部件 3D IoU → 3D 分片 的一条龙对照。用法：
#   scripts/eval_boundary.sh <pb.tpm> <tag>
# 需要环境变量：
#   RENDERS / SCENES  留出场景的根目录；HOLD 逗号分隔的场景名
#   DCN               卷积深度头（只在 3D 分片那步用到；不想跑分片可留空）
# 输出：2D 指标 + 3D 部件 IoU（全帧均值/中位）+ 前 3 个场景的逐部件 PLY 数
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
  --dump "$out/inst" --dump-n 8 | tee "$out/eval2d.txt" | tail -1

echo "== 3D 部件 IoU（两侧 GT 深度，隔离分割；全部 dump 帧）=="
vals=""
for f in "$out"/inst/*.npy; do
  b=$(basename "$f" .npy)
  s="${b%%_*}"
  fr="${b##*_}"
  v=$("$root/target/release/parts-depth.exe" part-iou \
    --scene "$renders/$s" --frame $((10#$fr)) --inst "$f" \
    | sed -E 's/.*mIoU ([0-9.]+) \/ .*/\1/')
  vals="$vals $v"
done
printf '%s\n' $vals | sort -n | awk '{a[NR]=$1; s+=$1}
  END{n=NR; printf "3D 部件 IoU：帧 %d  mean %.4f  median %.4f  min %.3f  max %.3f\n",
      n, s/n, (n%2?a[(n+1)/2]:(a[n/2]+a[n/2+1])/2), a[1], a[n]}' | tee "$out/part3d.txt"

if [ -n "${DCN:-}" ]; then
  echo "== 3D 分片（前 3 个留出场景，帧 0）=="
  i=0
  IFS=',' read -ra HS <<< "$hold"
  for s in "${HS[@]}"; do
    [ "$i" -ge 3 ] && break
    "$root/target/release/parts-depth.exe" mesh-single \
      --dcn "$DCN" --fg "$model" \
      --inst "$out/inst/${s}_000000.npy" \
      --scene "$renders/$s" --frame 0 --cell 0.02 \
      --out "$out/$s.ply" --split-dir "$out/parts-$s" | tail -2
    i=$((i + 1))
  done
fi

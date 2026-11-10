#!/usr/bin/env bash
# 留出集验收门：2D 分割 + 3D 部件 IoU + 单图网格/分片，逐项判红。
#   MODEL=pb.tpm RENDERS=R SCENES=S HOLD=scene0050,... DCN=dcn bash scripts/regress_holdout.sh
# 退出码：0 = 全过；1 = 有项低于阈值。
set -uo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
model="${MODEL:?设 MODEL（部件头 tpm）}"
renders="${RENDERS:?设 RENDERS}"
scenes="${SCENES:?设 SCENES}"
hold="${HOLD:?设 HOLD}"
dcn="${DCN:?设 DCN}"
out="$root/runs/regress"
rm -rf "$out"; mkdir -p "$out"

fail=0
ck() { # ck <名称> <实际> <下限>（浮点比较用 awk）
  local ok
  ok=$(awk -v a="$2" -v b="$3" 'BEGIN{print (a+0 >= b+0) ? 1 : 0}')
  if [ "$ok" = "1" ]; then printf '  PASS  %-22s %s ≥ %s\n' "$1" "$2" "$3";
  else printf '  FAIL  %-22s %s < %s\n' "$1" "$2" "$3"; fail=1; fi
}
ckeq() { if [ "$2" = "$3" ]; then printf '  PASS  %-22s %s\n' "$1" "$2";
  else printf '  FAIL  %-22s %s ≠ %s\n' "$1" "$2" "$3"; fail=1; fi; }

echo "== 2D =="
line=$("$root/target/release/parts-inst.exe" eval --model "$model" \
  --renders "$renders" --scenes "$scenes" --gt part --boundary --k 5 --res 384 \
  --dump "$out/inst" --dump-n 8 2>/dev/null | tail -1)
echo "$line"
m=$(sed -E 's/.*mIoU mean ([0-9.]+).*/\1/' <<<"$line")
fg=$(sed -E 's/.*前景 IoU mean ([0-9.]+).*/\1/' <<<"$line")
p=$(sed -E 's/.*边界 P ([0-9.]+) R.*/\1/' <<<"$line")
ck "部件 mIoU" "$m" 0.46
ck "前景 IoU" "$fg" 0.96
ck "边界 P" "$p" 0.25

echo "== 3D 部件 IoU（GT 深度，隔离分割）=="
vals=""
for f in "$out"/inst/*.npy; do
  b=$(basename "$f" .npy); s="${b%%_*}"; fr="${b##*_}"
  vals="$vals $("$root/target/release/parts-depth.exe" part-iou --scene "$renders/$s" --frame $((10#$fr)) --inst "$f" \
    | sed -E 's/.*mIoU ([0-9.]+) \/ .*/\1/')"
done
mean3d=$(printf '%s\n' $vals | awk '{s+=$1} END{printf "%.4f", s/NR}')
echo "  3D 部件 mIoU mean $mean3d（$(printf '%s\n' $vals | wc -l) 帧）"
ck "3D 部件 mIoU" "$mean3d" 0.48

echo "== 单图网格 / 分片（前 3 个留出场景，帧 0）=="
i=0
IFS=',' read -ra HS <<< "$hold"
for s in "${HS[@]}"; do
  [ "$i" -ge 3 ] && break
  msg=$("$root/target/release/parts-depth.exe" mesh-single --dcn "$dcn" --fg "$model" \
    --inst "$out/inst/${s}_000000.npy" --scene "$renders/$s" --frame 0 --cell 0.02 \
    --out "$out/$s.ply" --split-dir "$out/parts-$s" 2>&1)
  edges=$(grep -oE '边界边 [0-9]+' <<<"$msg" | grep -oE '[0-9]+' | head -1)
  np=$(grep -oE '部件切片：[0-9]+ 个' <<<"$msg" | grep -oE '[0-9]+' | head -1)
  ckeq "$s 边界边" "$edges" "0"
  ck "$s 部件数/6" "$np" 6
  if [ "$np" -gt 30 ]; then echo "  FAIL  $s 部件数 $np > 30"; fail=1; else echo "  PASS  $s 部件数 $np ≤ 30"; fi
  i=$((i + 1))
done

if [ "$fail" -eq 0 ]; then echo "RESULT: PASS"; exit 0; else echo "RESULT: FAIL"; exit 1; fi

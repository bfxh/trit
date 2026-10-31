#!/usr/bin/env bash
# 域外闸门定期重拟合：确定性切分 → 训练 → 留出评估 → 过门才允许覆盖随仓闸门。
#
# 用法：
#   scripts/refit_ood.sh <model.tpm> <manifest.json> <pos dirs(逗号)> <neg dir> [--apply]
#
# 判红门（任一不过就退出 1，且不写随仓文件）：
#   · 留出 AUC ≥ 0.99
#   · TPR@FPR≤1% ≥ 0.95
# 切分口径：负样本按文件名 cksum 的奇偶分 train/test（新增图片自动落位，可复现）。
set -euo pipefail

MODEL=${1:?需要 model.tpm}
MANIFEST=${2:?需要 manifest.json}
POS=${3:?需要正样本目录（逗号分隔）}
NEG=${4:?需要负样本目录}
APPLY=${5:-}

BIN=${PARTS_OOD:-target/release/parts-ood}
[ -x "$BIN" ] || BIN=./target/release/parts-ood
[ -x "$BIN" ] || { echo "找不到 parts-ood，可设 PARTS_OOD=<path>"; exit 2; }

WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT
mkdir -p "$WORK/train" "$WORK/test"
total=0
for f in "$NEG"/*.png "$NEG"/*.jpg "$NEG"/*.jpeg; do
  [ -e "$f" ] || continue
  b=$(basename "$f")
  h=$(printf '%s' "$b" | cksum | cut -d' ' -f1)
  if [ $((h % 2)) -eq 0 ]; then cp "$f" "$WORK/train/"; else cp "$f" "$WORK/test/"; fi
  total=$((total + 1))
done
n_train=$(ls "$WORK/train" | wc -l)
n_test=$(ls "$WORK/test" | wc -l)
echo "负样本 $total 张 ⇒ 训练 $n_train / 留出 $n_test"
[ "$n_train" -ge 5 ] && [ "$n_test" -ge 5 ] || { echo "负样本太少（训练/留出各需 ≥5）"; exit 2; }

BANK=$WORK/gate.tpod
"$BIN" train --model "$MODEL" --manifest "$MANIFEST" --pos "$POS" \
  --neg "$WORK/train" --per-dir 3 --out "$BANK" | tee "$WORK/train.log"
"$BIN" eval --model "$MODEL" --manifest "$MANIFEST" --bank "$BANK" \
  --pos "$POS" --neg "$WORK/test" --per-dir 40 | tee "$WORK/eval.log"

AUC=$(grep -o 'AUC [0-9.]*' "$WORK/eval.log" | head -1 | awk '{print $2}')
TPR=$(grep -o 'TPR@FPR≤1% \*\*[0-9.]*\*\*' "$WORK/eval.log" | grep -o '[0-9.]*' | head -1)
echo "留出：AUC=$AUC  TPR@1%FPR=$TPR"
ok=$(awk -v a="$AUC" -v t="$TPR" 'BEGIN{print (a>=0.99 && t>=0.95) ? 1 : 0}')
if [ "$ok" -ne 1 ]; then
  echo "❌ 未过门（AUC≥0.99 且 TPR≥0.95），不覆盖随仓闸门。"
  exit 1
fi
echo "✅ 过门"
if [ "$APPLY" = "--apply" ]; then
  out="crates/trit-parts/models/$(basename "$MODEL" .tpm).ood"
  cp "$BANK" "$out"
  echo "已写入 $out"
else
  echo "（未加 --apply，随仓闸门保持不变）"
fi

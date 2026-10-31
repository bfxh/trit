#!/usr/bin/env bash
# 带看门狗的训练：结束即停+响铃、卡死杀掉、崩溃自动续跑（从 --state 断点）。
#
# 用法：
#   scripts/train_guarded.sh <run-dir> -- <parts-train 参数...>
#
# 约定：
#   · 日志 <run-dir>/train.log（看门狗用它判心跳，默认 30 分钟没更新算卡死）
#   · 状态 <run-dir>/status.json（RUN / DONE / STALL_STOP / FAILED）
#   · 训练必须带 --state <run-dir>，否则崩溃重启会从头再来
set -euo pipefail

RUN=${1:?用法: train_guarded.sh <run-dir> -- <parts-train 参数...>}
shift
[ "${1:-}" = "--" ] && shift

TRAIN=${PARTS_TRAIN:-target/release/parts-train}
GUARD=${PARTS_GUARD:-target/release/parts-guard}
[ -x "$TRAIN" ] || TRAIN=E:/cargo-target/trit-parts/release/parts-train.exe
[ -x "$GUARD" ] || GUARD=E:/cargo-target/trit-parts/release/parts-guard.exe
[ -x "$TRAIN" ] || { echo "找不到 parts-train（设 PARTS_TRAIN）"; exit 2; }
[ -x "$GUARD" ] || { echo "找不到 parts-guard（设 PARTS_GUARD）"; exit 2; }

mkdir -p "$RUN"
STALL=${STALL_SEC:-1800}
RETRIES=${RETRIES:-3}

echo "== 托管：$RUN（stall ${STALL}s，重试 $RETRIES 次）"
exec "$GUARD" --stall-sec "$STALL" --poll-sec 5 --retries "$RETRIES" \
  --log "$RUN/train.log" --status "$RUN/status.json" \
  -- "$TRAIN" "$@" >> "$RUN/train.log" 2>&1

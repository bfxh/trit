#!/usr/bin/env bash
# 抢救「已经不在任何分支上」的提交（dangling commit）。
#
# 背景（2026-11-21）：`git fsck --lost-found` 在主仓库捞出 **547 个悬浮提交**。
# 它们通常来自：删掉的分支、reset --hard、rebase 丢弃的提交、以及"智能体在
# worktree 里提交但从不 push、目录随后被清理"的工作。悬空对象只要经历一次
# `git gc --prune` 就永久消失，所以必须在清理前把它们**引用化**。
#
# 用法：
#   bash scripts/rescue-dangling.sh [bundle 输出路径]
#   默认输出 D:/trit-archive/<日期>/rescue-dangling.bundle
#
# 做法：把每个悬空提交挂到 refs/rescue/<hash>（不进 heads，不污染 git branch），
# 再 `git bundle create --all`，这样 bundle 里带上全部救援锚点。
# 之后你可以：
#   git log refs/rescue/<hash>          # 看内容
#   git branch rescue/<hash> <hash>     # 真要用就提升成分支
#   git update-ref -d refs/rescue/<hash>  # 确认不需要后清理
set -euo pipefail

root=$(git rev-parse --show-toplevel)
cd "$root"
bundle=${1:-"D:/trit-archive/$(date +%Y-%m-%d)/rescue-dangling.bundle"}
mkdir -p "$(dirname "$bundle")"

echo "[rescue] git fsck --lost-found"
git fsck --lost-found --no-progress 2>/dev/null | grep '^dangling commit' | awk '{print $3}' > /tmp/trit-dangling.txt || true
n=$(wc -l < /tmp/trit-dangling.txt | tr -d ' ')
echo "[rescue] dangling commits: $n"

if [ "$n" -eq 0 ]; then
  echo "[rescue] 没有悬空提交，退出"
  exit 0
fi

while read -r hash; do
  [ -n "$hash" ] && git update-ref "refs/rescue/$hash" "$hash" 2>/dev/null || true
done < /tmp/trit-dangling.txt

echo "[rescue] 引用化为 refs/rescue/* 完成"
git bundle create "$bundle" --all >/dev/null 2>&1 || { echo "[rescue] bundle 失败" >&2; exit 1; }
echo "[rescue] 已打包：$bundle ($(du -h "$bundle" | cut -f1))"
echo "[rescue] 查看某条：git log refs/rescue/<hash>"

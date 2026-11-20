#!/usr/bin/env bash
# 把仓库自带的 pre-commit 门装到 .git/hooks（每台机器/每个工作树装一次）。
#
#   bash scripts/install-hooks.sh
#
# 注意：git worktree 共用主仓库的 .git/hooks，所以装一次对**所有 worktree 生效**。
# 这正是我们要的——并行的智能体不需要各自记得装。
set -euo pipefail

root=$(git rev-parse --show-toplevel)
src="$root/scripts/hooks/pre-commit"
dst=$(git rev-parse --git-path hooks)

[ -f "$src" ] || { echo "缺少 $src" >&2; exit 1; }
mkdir -p "$dst"
cp "$src" "$dst/pre-commit"
chmod +x "$dst/pre-commit"
echo "已安装 pre-commit 门到 $dst/pre-commit"
echo "验证：git commit 时会自动跑 入库准入 / fmt / module_budget / registry 结构检查"
echo "入库准入的开关与豁免：.agent/admission.json（关掉也仍会检查并打印风险与后果）"

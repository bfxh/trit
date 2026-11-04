#!/usr/bin/env bash
# 夜间镜像：trit 仓 → 本地 Git 仓（默认只导出；PUSH=1 才推）。
#
# 用法：scripts/nightly_mirror.sh <trit仓> <git镜像目录> [remote] [branch]
#   PUSH=1 scripts/nightly_mirror.sh D:/KF/trit D:/KF/mirror origin main
set -euo pipefail
repo_root=$(cd "$(dirname "$0")/.." && pwd)
src=${1:?用法: nightly_mirror.sh <trit仓> <git镜像目录> [remote] [branch]}
dst=${2:?用法: nightly_mirror.sh <trit仓> <git镜像目录> [remote] [branch]}
remote=${3:-origin}
branch=${4:-main}
cd "$repo_root"
cargo run --release -q -p trit-vcs -- git-export "$dst" --repo "$src"
if [ "${PUSH:-0}" = "1" ]; then
  git -C "$dst" push "$remote" "HEAD:$branch"
else
  echo "未推送（PUSH=1 才推）"
fi

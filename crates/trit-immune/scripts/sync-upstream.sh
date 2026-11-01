#!/usr/bin/env bash
# trit-immune → 上游 PR 分支同步（幂等：任何一步中断后重跑即恢复）。
#
# 过程纪律（对应 ROADMAP #46）：
# 1. 补丁文件化（upstream/patches/）——read-tree 会清工作区，真源在 git 管的
#    patch 里，工作区永远可以重建；
# 2. 幂等：每个补丁先 `git apply -R --check`（已应用则跳过），脚本可反复跑；
# 3. 分层验证：cargo check（快，先跑）→ full test（慢，可选，默认不自动跑
#    ——智能体任务别让底层长任务卡死，长验证由人决定时机）；
# 4. 只做本地同步，commit/push 由人确认后执行（外向动作不自动）。
#
# 用法：sync-upstream.sh <trit-immune 仓库路径> <trit 上游 clone 路径> [分支]
set -uo pipefail

IMMUNE="${1:?用法: sync-upstream.sh <trit-immune 仓库> <trit 上游 clone> [分支]}"
UPSTREAM="${2:?用法: sync-upstream.sh <trit-immune 仓库> <trit 上游 clone> [分支]}"
BRANCH="${3:-master}"
CRATE="$UPSTREAM/crates/trit-immune"
PATCHES="$IMMUNE/upstream/patches"

step() { printf '\n== %s\n' "$*"; }
die() { printf '错误：%s\n' "$*" >&2; exit 1; }

[ -d "$IMMUNE/.git" ] || die "$IMMUNE 不是 git 仓库"
[ -d "$UPSTREAM/.git" ] || die "$UPSTREAM 不是 git 仓库"
[ -d "$PATCHES" ] || die "补丁目录不存在：$PATCHES"

step "1/5 fetch 本仓库 $BRANCH"
git -C "$UPSTREAM" fetch "$IMMUNE" "$BRANCH" || die "fetch 失败"

step "2/5 清空 prefix 并树级叠加（read-tree）"
# 幂等：rm --cached 对"staged 中间态"会拒删（数据丢失保护）——但 staged
# 中间态可从 upstream/patches 重建（补丁是真源，工作区不是孤本），故 -f 兜底
git -C "$UPSTREAM" rm -r -q --cached crates/trit-immune 2>/dev/null \
  || git -C "$UPSTREAM" rm -r -q -f --cached crates/trit-immune 2>/dev/null
git -C "$UPSTREAM" clean -q -fd crates/trit-immune
git -C "$UPSTREAM" read-tree --prefix=crates/trit-immune/ -u "FETCH_HEAD^{tree}" \
  || die "read-tree 失败（工作区可重跑本脚本恢复）"

step "3/5 打上游专属补丁（幂等：已应用则跳过）"
for p in "$PATCHES"/*.patch; do
  if git -C "$UPSTREAM" apply --recount --check "$p" 2>/dev/null; then
    git -C "$UPSTREAM" apply --recount "$p" && echo "已应用：$(basename "$p")"
  elif git -C "$UPSTREAM" apply --recount -R --check "$p" 2>/dev/null; then
    echo "已应用过，跳过：$(basename "$p")"
  else
    die "补丁既不能正向也不能逆向：$p（请人工检查 $CRATE）"
  fi
done

step "4/5 快验（cargo check，几十秒级）"
if command -v cargo >/dev/null 2>&1; then
  (cd "$UPSTREAM" && CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$UPSTREAM/../trit-target-sync}" \
    cargo check -p trit-immune --quiet) \
    && echo "check 通过（内核 + 工具层装配完整）" \
    || die "check 失败——同步后代码不完整，请检查补丁"
else
  echo "警告：cargo 不可用，跳过快验（push 前必须手工 cargo test）"
fi

step "5/5 完成——后续动作（人工确认）"
cat <<EOF
本地同步完成。建议下一步：
  cd "$UPSTREAM" && cargo test -p trit-immune     # 全量验证（慢，按需）
  git -C "$UPSTREAM" add -A && git -C "$UPSTREAM" commit -m "同步 master"
  git -C "$UPSTREAM" push                          # 外向动作，人确认
EOF

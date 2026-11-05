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
#       sync-upstream.sh --self-test   只验下面这套守卫，不做任何同步
set -uo pipefail

step() { printf '\n== %s\n' "$*"; }
die() { printf '错误：%s\n' "$*" >&2; exit 1; }

# worktree 的 `.git` 是指针**文件**不是目录，`[ -d .git ]` 会把它判成"不是仓库"；
# 只有 `rev-parse --git-dir` 同时认普通仓库与 worktree。
is_git_repo() {
  git -C "$1" rev-parse --git-dir >/dev/null 2>&1
}

# 临时目录名必须是**全局**：EXIT trap 在 self_test 返回之后才跑，届时 local 已出作用域，
# set -u 会把清理本身变成 "tmp: unbound variable"，目录留在盘上没人管（实测踩过）。
SWEEP_DIR=""
sweep() {
  if [ -n "$SWEEP_DIR" ]; then rm -rf "$SWEEP_DIR"; fi
}

# 调用口径：check <期望> <实际> <例名>。定义放在 self_test 之外，
# 这样"两个值印反了"本身是一条可登记的变异（锚点必须落在自测面之前）。
check() {
  total=$((total + 1))
  if [ "$1" != "$2" ]; then
    printf 'FAIL %s：期望 %s，实际 %s\n' "$3" "$1" "$2"
    fails=$((fails + 1))
  fi
}

self_test() {
  local fails=0 total=0 bad
  SWEEP_DIR=$(mktemp -d "${TMPDIR:-/tmp}/sync-upstream.XXXXXX") \
    || { printf 'FAIL 建不了临时目录\n'; return 1; }
  local tmp="$SWEEP_DIR"
  trap sweep EXIT

  git init -q "$tmp/repo" >/dev/null 2>&1
  mkdir -p "$tmp/plain"
  check "0" "$(is_git_repo "$tmp/repo" >/dev/null 2>&1; echo $?)" "真仓库必须被认成 git 仓库"
  check "128" "$(is_git_repo "$tmp/plain" >/dev/null 2>&1; echo $?)" "空目录要被拒（这就是 die 的触发条件）"

  # 注释里承诺的那件事：worktree（.git 是文件）也得过——旧写法 [ -d .git ] 正好在这儿撒谎
  git -C "$tmp/repo" -c user.email=t@e -c user.name=t commit -q --allow-empty -m init >/dev/null 2>&1
  git -C "$tmp/repo" worktree add -q "$tmp/wt" HEAD >/dev/null 2>&1
  check "0" "$(is_git_repo "$tmp/wt" >/dev/null 2>&1; echo $?)" "worktree 也要被认成 git 仓库"
  check "file" "$( [ -f "$tmp/wt/.git" ] && echo file )" "反证：worktree 的 .git 确实是文件（旧判法必挂）"

  # 回归钉：不许把续行写成字面量 `\n`——bash 会把 n 当成 rev-parse 的参数
  # ⇒ 前置检查对**任何合法仓库**都 die「不是 git 仓库」，而 `bash -n` 查不出来（语法合法）。
  # 中间那层管道滤掉"钉自己的那行"：模式和代码写在同一行时，这条检查会抓到它自己（假阳性）。
  if grep 'rev-parse --git-dir' "$0" | grep -v 'grep ' | grep -q '\\n'; then bad=1; else bad=0; fi
  check "0" "$bad" "脚本里不许出现字面量反斜杠-n 续行（bash -n 看不见这条）"

  # 清理函数自己也在被验面里：只挂在 trap 上的话，它坏了没人知道（trap 在断言跑完之后才执行）。
  sweep
  check "gone" "$( [ -d "$SWEEP_DIR" ] && echo live || echo gone )" "sweep 必须真的删掉临时目录"

  # 读红的人按这句话找差值：两个值印反就把人往错方向带（第一次实跑就印反了）。
  # 放在子 shell 里跑，免得这条"故意失败"把 total/fails 搅浑。
  check "FAIL 哨兵：期望 0，实际 1" "$(check "0" "1" "哨兵" 2>&1)" "失败消息不许把期望/实际印反"

  printf 'self-test: %d/%d passed\n' "$((total - fails))" "$total"
  [ "$fails" -eq 0 ]
}

if [ "${1:-}" = "--self-test" ]; then
  self_test
  exit $?
fi

IMMUNE="${1:?用法: sync-upstream.sh <trit-immune 仓库> <trit 上游 clone> [分支]}"
UPSTREAM="${2:?用法: sync-upstream.sh <trit-immune 仓库> <trit 上游 clone> [分支]}"
BRANCH="${3:-main}"
CRATE="$UPSTREAM/crates/trit-immune"
PATCHES="$IMMUNE/upstream/patches"

is_git_repo "$IMMUNE" || die "$IMMUNE 不是 git 仓库"
is_git_repo "$UPSTREAM" || die "$UPSTREAM 不是 git 仓库"
[ -d "$PATCHES" ] || die "补丁目录不存在：$PATCHES"

step "1/6 fetch 本仓库 $BRANCH"
git -C "$UPSTREAM" fetch "$IMMUNE" "$BRANCH" || die "fetch 失败"

step "2/6 清空 prefix 并树级叠加（read-tree）"
# 幂等：rm --cached 对"staged 中间态"会拒删（数据丢失保护）——但 staged
# 中间态可从 upstream/patches 重建（补丁是真源，工作区不是孤本），故 -f 兜底
git -C "$UPSTREAM" rm -r -q --cached crates/trit-immune 2>/dev/null \
  || git -C "$UPSTREAM" rm -r -q -f --cached crates/trit-immune 2>/dev/null
git -C "$UPSTREAM" clean -q -fd crates/trit-immune
git -C "$UPSTREAM" read-tree --prefix=crates/trit-immune/ -u "FETCH_HEAD^{tree}" \
  || die "read-tree 失败（工作区可重跑本脚本恢复）"

step "3/6 打上游专属补丁（幂等：已应用则跳过）"
for p in "$PATCHES"/*.patch; do
  if git -C "$UPSTREAM" apply --recount --check "$p" 2>/dev/null; then
    git -C "$UPSTREAM" apply --recount "$p" && echo "已应用：$(basename "$p")"
  elif git -C "$UPSTREAM" apply --recount -R --check "$p" 2>/dev/null; then
    echo "已应用过，跳过：$(basename "$p")"
  else
    die "补丁既不能正向也不能逆向：$p（请人工检查 $CRATE）"
  fi
done

step "4/6 护栏：上游不许出现指向本仓的 git 源依赖"
if grep -qF 'git = "https://github.com/bfxh/trit"' "$CRATE/Cargo.toml"; then
  die "同步后的 crates/trit-immune/Cargo.toml 仍带 git 源依赖——上游要用 trit-core.workspace = true（工作区内部路径），
       否则就成了 trit 仓依赖 trit 仓。注意：这条护栏本轮只做过 bash -n 语法检查，没实跑过完整同步。"
fi

step "5/6 快验（cargo check，几十秒级）"
if command -v cargo >/dev/null 2>&1; then
  (cd "$UPSTREAM" && CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$UPSTREAM/../trit-target-sync}" \
    cargo check -p trit-immune --quiet) \
    && echo "check 通过（内核 + 工具层装配完整）" \
    || die "check 失败——同步后代码不完整，请检查补丁"
else
  echo "警告：cargo 不可用，跳过快验（push 前必须手工 cargo test）"
fi

step "6/6 完成——后续动作（人工确认）"
cat <<EOF
本地同步完成。建议下一步：
  cd "$UPSTREAM" && cargo test -p trit-immune     # 全量验证（慢，按需）
  git -C "$UPSTREAM" add -A && git -C "$UPSTREAM" commit -m "同步 master"
  git -C "$UPSTREAM" push                          # 外向动作，人确认
EOF

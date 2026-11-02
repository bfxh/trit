#!/usr/bin/env bash
# trit 账本命令行的端到端冒烟：每一步都断言**可观察输出**，不只看退出码。
#
# 用法：bash scripts/cli_smoke.sh [可执行文件路径]
#       默认 target/debug/trit-immune（先 cargo build）
set -uo pipefail

BIN="${1:-target/debug/trit-immune}"
if [ ! -e "$BIN" ]; then
  printf 'FAIL 找不到可执行文件：%s（先 cargo build）\n' "$BIN" >&2
  exit 1
fi

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

fail() { printf 'FAIL %s\n' "$*" >&2; exit 1; }
say() { printf 'ok   %s\n' "$*"; }

# expect <needle> <实际输出> <标签>
expect() {
  case "$2" in
    *"$1"*) say "$3" ;;
    *) printf 'FAIL %s：期望包含 [%s]，实际输出：%s\n' "$3" "$1" "$2" >&2; exit 1 ;;
  esac
}

# expect_absent <needle> <实际输出> <标签>
expect_absent() {
  case "$2" in
    *"$1"*) printf 'FAIL %s：不该出现 [%s]，实际：%s\n' "$3" "$1" "$2" >&2; exit 1 ;;
    *) say "$3" ;;
  esac
}

# 1) init 把不存在的父目录一起建出来
REPO="$WORK/brand-new/nested"
LEDGER="$REPO/repo.tritj"
out="$("$BIN" init "$REPO" 冒烟账本 2>&1)" || fail "init 失败：$out"
expect "初始化账本" "$out" "init 建出嵌套目录并落账本"
[ -f "$LEDGER" ] || fail "init 后账本文件不存在：$LEDGER"

# 2) 重复 init 必须拒绝，而不是覆盖已有历史
if out="$("$BIN" init "$REPO" 2>&1)"; then
  fail "重复 init 竟然成功：$out"
fi
expect "已存在" "$out" "重复 init 被拒绝"

run() { "$BIN" "$@" -f "$LEDGER" 2>&1 || fail "命令失败：$BIN $*"; }

# 3) 基线口径：账本根节点建库即 +1（既有代码视为基线），不是 0
out="$(run status)"
expect "尾门禁：Allow" "$out" "新账本根节点为 +1 基线"
expect "节点：1（+1 / 0:0 / -0 / 隔离 0）" "$out" "根节点计数如实"

# 4) branch 生出的新节点默认 0 态 → 冻结并进可疑队列
out="$(run branch feat-login)"
expect "分支 feat-login = 节点 #01" "$out" "branch 生出新节点"
out="$(run status)"
expect "尾门禁：Freeze" "$out" "新节点未测 → 冻结"
expect "可疑队列：1 个" "$out" "0 态进可疑队列"

# 5) commit 不带 -v 仍是 0：写完不等于测过
out="$(run commit -m "写完没跑回归")"
expect "记录 01 ← 0" "$out" "commit 默认 0 态"
out="$(run status)"
expect "尾门禁：Freeze" "$out" "没测就提交不改变门禁"

# 6) 一条 -1 堵死全链并指出打穿来源；补测到 +1 后自动解封
out="$(run commit -m "SQLi 重放打穿" -v -1)"
expect "记录 01 ← -1" "$out" "记录确证漏洞"
out="$(run gate)"
expect "门禁：Block" "$out" "-1 否决整条链"
expect "打穿来源" "$out" "gate 指出来源"
out="$(run commit -m "参数化查询修复" -v +1)"
expect "记录 01 ← +1" "$out" "修复后重记 +1"
out="$(run gate)"
expect "门禁：Allow" "$out" "源补好后下游自动恢复"
expect "无打穿来源" "$out" "解封后不再有打穿来源"

# 7) log / checkout：日志不可变，可回到任意节点
out="$(run log -n 5)"
expect "RECORD" "$out" "log 展示裁决事件"
expect "SPAWN" "$out" "log 展示分支事件"
out="$(run checkout 0)"
expect "节点 #00 时的全图" "$out" "checkout 时间旅行"

# 8) clone 出来的副本与源账本收敛度量一致
CLONE="$WORK/clone.tritj"
out="$("$BIN" clone "$LEDGER" "$CLONE" 2>&1)" || fail "clone 失败：$out"
expect "克隆" "$out" "clone 复制整条历史"
src_line="$(run status | grep "收敛率：")"
clone_line="$("$BIN" status -f "$CLONE" 2>&1 | grep "收敛率：")"
[ "$src_line" = "$clone_line" ] || fail "clone 后收敛度量不一致：$src_line vs $clone_line"
say "clone 与源账本收敛度量一致"

# 9) merge：δ-CRDT 无冲突合并（副本上追加，再并回源）
"$BIN" commit -m "副本补测" -v +1 -n 0 -f "$CLONE" >/dev/null 2>&1 || fail "副本 commit 失败"
out="$(run merge "$CLONE")"
expect "合并" "$out" "merge 采纳副本增量"

# 10) lock：依赖坐标入账（外部事实 = 0 态），且幂等不重复建节点
LOCKF="$WORK/Cargo.lock"
cat > "$LOCKF" <<'LOCK'
# Cargo.lock v3
[[package]]
name = "shared-lib"
version = "1.2.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "aaaa"

[[package]]
name = "other"
version = "0.1.0"
LOCK
out="$(run lock "$LOCKF")"
expect "坐标入账 2 条：新建 2，复用 0" "$out" "lock 把锁文件里每个解析结果立成坐标节点"
out="$(run lock "$LOCKF")"
expect "坐标入账 2 条：新建 0，复用 2" "$out" "同锁文件再跑不重复建节点（幂等）"
out="$(run log -n 20)"
expect "消费 COORD shared-lib@1.2.0" "$out" "坐标节点连回本仓根的消费边入账"

# 11) 未知命令必须非零退出并报命令名
if out="$("$BIN" nosuchcommand 2>&1)"; then
  fail "未知命令竟然成功"
fi
expect "nosuchcommand" "$out" "未知命令如实报错"
expect_absent "panic" "$out" "错误路径不 panic"

printf '\nCLI 冒烟全部通过：%s\n' "$BIN"

#!/usr/bin/env bash
# trit 账本命令行的端到端冒烟：每一步都断言**可观察输出**，不只看退出码。
#
# 用法：bash scripts/cli_smoke.sh [可执行文件路径]
#       默认 target/debug/trit-immune（先 cargo build）
#       bash scripts/cli_smoke.sh --self-test   只验下面这套断言工具，不需要二进制
set -uo pipefail

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

# run <参数…>：账本命令统一走这里；非零退出必须把整条冒烟拖红，而不是被吞掉
run() { "$BIN" "$@" -f "$LEDGER" 2>&1 || fail "命令失败：$BIN $*"; }

# self_test 验的是**断言工具本身会不会空转**：每个工具都喂「必须通过」和
# 「必须判红」两侧输入。只测正例的话，把 expect 改成永远通过也照样绿——
# 变异门钉的就是这条（下面的每一步都对应一个可注入的坏改法）。
self_test() {
  local rc=0 stub seen
  stub="$(mktemp)" || fail "自测建不出桩二进制"
  printf '#!/bin/sh\ncase "${1:-}" in --fail) exit 3 ;; esac\nexit 0\n' > "$stub"
  # 没有执行位的桩在 Linux/CI 上根本跑不起来（正例反例全变 rc=126 ⇒ 反例假通过）
  chmod +x "$stub" || { rm -f "$stub"; fail "自测无法给桩加执行位"; }
  BIN="$stub"
  LEDGER="/dev/null"

  # 先钉夹具本身：桩得能真的分成成功/失败两侧，否则下面的反例是空转的
  must_pass() {
    local label="$1"
    shift
    ( "$@" ) >/dev/null 2>&1 || { printf 'FAIL 自测：%s 该通过却被判红\n' "$label" >&2; rc=1; }
  }
  must_fail() {
    local label="$1"
    shift
    ( "$@" ) >/dev/null 2>&1
    [ $? -ne 0 ] || { printf 'FAIL 自测：%s 该判红却放行（断言空转）\n' "$label" >&2; rc=1; }
  }

  must_pass "桩命令成功侧" "$BIN" ok
  must_fail "桩命令失败侧" "$BIN" --fail
  must_pass "expect 认得出 needle" expect "甲" "某某甲某某" "自测"
  must_fail "expect 少 needle 仍放行" expect "乙" "某某甲某某" "自测"
  must_pass "expect_absent 放行干净输出" expect_absent "乙" "某某甲某某" "自测"
  must_fail "expect_absent 命中脏输出仍放行" expect_absent "甲" "某某甲某某" "自测"
  must_fail "fail 没让脚本失败" fail "自测探针"
  must_fail "run 把失败命令当成功" run --fail
  must_pass "run 放行成功命令" run ok
  # run 的价值在「失败就中止」而不只是返回非零：少了 || fail，脚本会带着错误往下跑，
  # 冒烟照样可能全绿。所以这里看的是后续语句有没有被执行。
  seen="$( ( run --fail; printf 'REACHED' ) 2>&1 )"
  case "$seen" in
    *REACHED*) { printf 'FAIL 自测：run 失败后没有中止脚本（错误被吞）\n' >&2; rc=1; } ;;
  esac

  rm -f "$stub"
  [ $rc -eq 0 ] || return 1
  say "断言工具自测通过（expect/expect_absent/fail/run 各正反两侧）"
  return 0
}

if [ "${1:-}" = "--self-test" ]; then
  self_test
  exit $?
fi

BIN="${1:-target/debug/trit-immune}"
if [ ! -e "$BIN" ]; then
  printf 'FAIL 找不到可执行文件：%s（先 cargo build）\n' "$BIN" >&2
  exit 1
fi

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

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

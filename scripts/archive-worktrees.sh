#!/usr/bin/env bash
# 把所有 worktree 的工作完整压缩归档（bundle + 全部 stash + dirty + 未跟踪文件）。
#
# 为什么必须有这个脚本（2026-11-21 的教训）：19 个工作树里 **19 个都有未合并提交**，
# 合计 1008 个提交没进 main；每个工作树还有 2 个 stash（共 38 个），而 stash
# 不属于任何分支、`git bundle --all` 也不含它——目录一删，stash 就永久消失。
# 多个智能体各自在 worktree 里提交、从不 push，会话额度耗尽被中断后，
# 工作树被清理，分支成孤儿、stash 消失，gc 之后彻底没了。
#
# 用法：
#   bash scripts/archive-worktrees.sh [归档根目录]
#   默认归档到 D:/trit-archive/<日期>/full/
#
# 产物（每个 worktree 一份）：
#   <name>.bundle            全部分支与标签（不含 stash）
#   <name>-stash<N>.patch    每个 stash 的完整 diff
#   <name>-dirty.patch       已跟踪文件的未提交改动
#   <name>-untracked/        未跟踪文件（原样复制，保留相对路径）
#   worktree-audit.txt       每个工作树的 dirty / unmerged / stash 计数
set -euo pipefail

root=$(git rev-parse --show-toplevel)
dest=${1:-"D:/trit-archive/$(date +%Y-%m-%d)/full"}
mkdir -p "$dest"

python - "$dest" <<'PY'
import shutil, subprocess, sys
from pathlib import Path

dest = Path(sys.argv[1])
dest.mkdir(parents=True, exist_ok=True)

def git(path, *args, timeout=300):
    try:
        return subprocess.run(["git", "-C", path, *args], capture_output=True, timeout=timeout)
    except Exception as exc:
        print(f"  git fail {path} {args}: {exc}")
        return None

wt = subprocess.run(["git", "worktree", "list", "--porcelain"], capture_output=True, text=True)
paths = [l.split(" ", 1)[1] for l in wt.stdout.splitlines() if l.startswith("worktree ")]

audit, total = [], 0
for p in paths:
    if not Path(p).exists():
        audit.append(f"{p} | MISSING")
        continue
    name = p.replace(":", "").replace("/", "_").replace("\\", "_").strip("_")
    bundle = dest / f"{name}.bundle"
    git(p, "bundle", "create", str(bundle), "--all")

    sl = git(p, "stash", "list")
    n_stash = len([x for x in (sl.stdout.decode("utf-8", "replace").splitlines() if sl else []) if x])
    for i in range(n_stash):
        out = git(p, "stash", "show", "-p", f"stash@{{{i}}}")
        if out is not None:
            (dest / f"{name}-stash{i}.patch").write_bytes(out.stdout)

    diff = git(p, "diff")
    if diff is not None and diff.stdout:
        (dest / f"{name}-dirty.patch").write_bytes(diff.stdout)

    st = git(p, "status", "--porcelain")
    untracked = [x[3:] for x in (st.stdout.decode("utf-8", "replace").splitlines() if st else []) if x.startswith("?? ")]
    for rel in untracked:
        src = Path(p) / rel
        if src.is_file():
            dst = dest / f"{name}-untracked" / rel
            dst.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(src, dst)

    branch = (git(p, "branch", "--show-current") or b"").decode("utf-8", "replace").strip() or "(detached)"
    dirty = len([x for x in (st.stdout.decode("utf-8", "replace").splitlines() if st else []) if x])
    unmerged = len([x for x in (git(p, "log", "--oneline", "main..HEAD") or b"").decode().splitlines() if x])
    audit.append(f"{p} | {branch} | dirty={dirty} | unmerged={unmerged} | stash={n_stash}")
    size = bundle.stat().st_size if bundle.exists() else 0
    total += size
    print(f"{name}: bundle={'ok' if size else 'FAIL'} stash={n_stash} dirty={len(untracked)}untracked size={size/1e6:.1f}MB")

(dest / "worktree-audit.txt").write_text("\n".join(audit) + "\n", encoding="utf-8")
print(f"\narchived {len(paths)} worktrees -> {dest} ({total/1e6:.0f} MB)")
PY

echo
echo "提示：bundle 不含 stash（已单独导出 patch）。恢复某一份："
echo "  git clone <归档目录>/<name>.bundle <dir>"
echo "  git -C <dir> apply <归档目录>/<name>-dirty.patch     # 未提交改动"
echo "  git -C <dir> apply <归档目录>/<name>-stash0.patch    # 第一个 stash"

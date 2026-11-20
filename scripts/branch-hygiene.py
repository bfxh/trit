#!/usr/bin/env python3
"""分支卫生检查：把 docs/BRANCHING.md 的规则变成可复跑的报告。

用法：
  python scripts/branch-hygiene.py [--strict] [--json] [--stale-days 30] [--root <repo>]

输出五类（与 BRANCHING.md §3 对应）：
  merged   已合并进 main          -> 删（本地 + 远端）
  empty    main..branch 无提交    -> 删
  stale    未合并且超过 N 天未动  -> 先归档再删
  badname  命名不在白名单         -> 改名或删
  worktree 被某个 worktree 占用   -> 先 worktree remove

--strict 时只要有 merged / empty / badname 就 rc=1（stale 只提示，不阻断）。
"""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
from datetime import date, datetime
from pathlib import Path

PREFIXES = ("feat/", "fix/", "perf/", "docs/", "test/", "security/", "ci/", "chore/",
            "refactor/", "release/", "parts/")
TRUNK = "main"


def git(*args: str, root: Path | None = None) -> str:
    cmd = ["git"]
    if root:
        cmd += ["-C", str(root)]
    cmd += list(args)
    r = subprocess.run(cmd, capture_output=True, timeout=180)
    if r.returncode != 0:
        return ""
    return (r.stdout or b"").decode("utf-8", "replace")


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--strict", action="store_true")
    ap.add_argument("--json", action="store_true")
    ap.add_argument("--stale-days", type=int, default=30)
    ap.add_argument("--root", default=None)
    args = ap.parse_args()
    root = Path(args.root) if args.root else None

    branches = [b for b in git("for-each-ref", "--format=%(refname:short)", "refs/heads", root=root).splitlines() if b]
    merged = set(git("branch", "--merged", TRUNK, "--format=%(refname:short)", root=root).splitlines())
    worktree_branches = set()
    for line in git("worktree", "list", "--porcelain", root=root).splitlines():
        if line.startswith("branch refs/heads/"):
            worktree_branches.add(line[len("branch refs/heads/"):])

    today = date.today()
    out = {"merged": [], "empty": [], "stale": [], "badname": [], "worktree": []}
    for b in branches:
        if b == TRUNK:
            continue
        if b in worktree_branches:
            out["worktree"].append(b)
        if b in merged:
            out["merged"].append(b)
            continue
        n = len([x for x in git("rev-list", f"{TRUNK}..{b}", root=root).splitlines() if x])
        if n == 0:
            out["empty"].append(b)
            continue
        if not b.startswith(PREFIXES):
            out["badname"].append(b)
        last = git("log", "-1", "--format=%cs", b, root=root).strip()
        if last:
            try:
                days = (today - datetime.strptime(last, "%Y-%m-%d").date()).days
            except ValueError:
                days = 0
            if days > args.stale_days:
                out["stale"].append({"branch": b, "days": days, "commits": n})

    if args.json:
        print(json.dumps(out, ensure_ascii=False, indent=2))
    else:
        print(f"分支总数 {len(branches)}（不含 {TRUNK}）")
        for key, label in (
            ("merged", "已合并，可删"),
            ("empty", "内容已在 main，可删"),
            ("stale", f"未合并且 >{args.stale_days} 天未动，先归档再删"),
            ("badname", "命名不合规，改名或删"),
            ("worktree", "被 worktree 占用，先 remove"),
        ):
            items = out[key]
            print(f"\n[{key}] {label}（{len(items)}）")
            for it in items:
                if isinstance(it, dict):
                    print(f"  {it['branch']}  ({it['days']}d, {it['commits']} 提交)")
                else:
                    print(f"  {it}")

    blocking = len(out["merged"]) + len(out["empty"]) + len(out["badname"])
    if args.strict and blocking:
        print(f"\nSTRICT: {blocking} 个分支需要处理（merged/empty/badname）", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())

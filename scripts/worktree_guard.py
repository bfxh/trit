#!/usr/bin/env python3
"""多会话工作树守卫：主工作树纯度、工作树/分支预算、已合 PR 的证据化回收。

背景（2026-11-12 用户反馈）：多个智能体会话共享同一批工作树。实测 26 个工作树里
17 个停在**已合入 PR 的分支**上，`main` 没有任何工作树检出；同时会话把
`D:/KF/trit-wspace-wt` 当草稿纸反复 `git switch`，别的会话/服务一读就"失灵"。
再叠上 CHANGELOG 顶部同位置改写（每次 main 前进就冲突 → rebase → 再跑 1 小时
mutation），就是"分支爆炸"的完整链路。

用法：
    python scripts/worktree_guard.py                 # 体检：红/黄点名列全
    python scripts/worktree_guard.py --apply         # 回收：只删「已合 PR + 干净 + 本地 tip == PR head」
    python scripts/worktree_guard.py --self-test

回收判据（四个条件全部满足才动，否则只提示）：
  1. 分支的 PR 已合入（`gh pr list --state merged` 证据，含 headRefOid）；
  2. 工作树干净（`git status --porcelain` 为空）；
  3. 本地分支 tip == 该 PR 的 headRefOid（有额外本地提交 ⇒ 不删）；
  4. 不是主工作树、不是当前目录所在工作树。
"""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
from pathlib import Path

def norm(path: str | Path) -> str:
    import os

    return os.path.normcase(os.path.normpath(str(path)))


MAX_WORKTREES = 20
MAX_BRANCHES = 180
DEFAULT_WORKSPACE = Path("D:/KF/trit-wspace-wt")


def git(*args: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        ["git", *args],
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
    )


def parse_worktrees(text: str) -> list[dict]:
    """`git worktree list --porcelain` → [{path, head, branch, detached}]。"""
    rows: list[dict] = []
    current: dict | None = None
    for line in text.splitlines():
        if line.startswith("worktree "):
            current = {"path": line[len("worktree ") :], "head": "", "branch": "", "detached": False}
            rows.append(current)
        elif current is None:
            continue
        elif line.startswith("HEAD "):
            current["head"] = line[len("HEAD ") :]
        elif line.startswith("branch "):
            current["branch"] = line[len("branch ") :].replace("refs/heads/", "")
        elif line == "detached":
            current["detached"] = True
    return rows


def load_merged(repo: str) -> dict[str, str]:
    """分支名 → 已合 PR 的 headRefOid（同分支多次 PR 取最新）。"""
    result = subprocess.run(
        ["gh", "pr", "list", "--repo", repo, "--state", "merged", "--limit", "1000",
         "--json", "headRefName,headRefOid"],
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
    )
    if result.returncode != 0:
        raise SystemExit(f"读不到已合 PR 清单（不许把\"查不到\"当\"没有问题\"）：{result.stderr.strip()}")
    merged: dict[str, str] = {}
    for row in json.loads(result.stdout or "[]"):
        merged[row["headRefName"]] = row["headRefOid"]
    return merged


def dirty_of(path: str) -> int:
    result = git("-C", path, "status", "--porcelain")
    return len([line for line in result.stdout.splitlines() if line.strip()])


def evaluate(
    worktrees: list[dict],
    branches: list[str],
    merged: dict[str, str],
    dirty: dict[str, int],
    workspace: Path | None,
    max_worktrees: int = MAX_WORKTREES,
    max_branches: int = MAX_BRANCHES,
    workspace_exists: bool | None = None,
) -> tuple[list[str], list[str]]:
    """返回 (红, 黄)。红 = 结构性违规；黄 = 需要人/`--apply` 处理的回收项。"""
    red: list[str] = []
    warn: list[str] = []

    main_hosts = [wt for wt in worktrees if wt["branch"] == "main"]
    if len(main_hosts) != 1:
        red.append(f"main 必须且只能在一个工作树检出，实际 {len(main_hosts)} 个")

    exists = False if workspace is None else (workspace.exists() if workspace_exists is None else workspace_exists)
    if workspace is not None and exists:
        host = next((wt for wt in worktrees if norm(wt["path"]) == norm(workspace)), None)
        if host is None:
            red.append(f"共享主工作树 {workspace} 不在 worktree 清单里")
        else:
            if host["branch"] != "main":
                red.append(
                    f"共享主工作树 {workspace} 在分支 {host['branch'] or '(detached)'} 上"
                    "——多会话共享时禁止在里面切分支/提交"
                )
            if dirty.get(host["path"], 0):
                red.append(f"共享主工作树 {workspace} 有 {dirty[host['path']]} 个未提交改动")

    if len(worktrees) > max_worktrees:
        red.append(f"worktree 数 {len(worktrees)} 超过预算 {max_worktrees}：合并后要回收")
    feature = [b for b in branches if b != "main"]
    if len(feature) > max_branches:
        red.append(f"本地分支 {len(feature)} 超过预算 {max_branches}：合并后要 prune")

    for wt in worktrees:
        branch = wt["branch"]
        if not branch or branch == "main":
            continue
        if branch in merged:
            if dirty.get(wt["path"], 0):
                warn.append(f"已合分支 {branch} 的工作树有未提交改动：{wt['path']}（不自动回收）")
            elif wt["head"] != merged[branch]:
                warn.append(
                    f"已合分支 {branch} 的本地 tip 与 PR head 不一致（多了本地提交）：{wt['path']}"
                )
            else:
                warn.append(f"可回收：{wt['path']}（分支 {branch}，PR 已合、干净、tip 一致）")
    return red, warn


def plan_reclaim(
    worktrees: list[dict],
    merged: dict[str, str],
    dirty: dict[str, int],
    keep_paths: set[str],
) -> tuple[list[dict], list[str]]:
    """生成回收计划；不满足四条件的进警告。"""
    plan: list[dict] = []
    skipped: list[str] = []
    for wt in worktrees:
        branch = wt["branch"]
        if not branch or branch == "main":
            continue
        if norm(wt["path"]) in keep_paths:
            skipped.append(f"当前目录所在工作树不回收：{wt['path']}")
            continue
        if branch not in merged:
            continue
        if dirty.get(wt["path"], 0):
            skipped.append(f"{branch}：工作树有未提交改动，保留 {wt['path']}")
            continue
        if wt["head"] != merged[branch]:
            skipped.append(f"{branch}：本地 tip≠PR head（有额外提交），保留 {wt['path']}")
            continue
        plan.append({"path": wt["path"], "branch": branch, "head": wt["head"]})
    return plan, skipped


def apply_plan(plan: list[dict]) -> int:
    failures = 0
    for item in plan:
        remove = git("worktree", "remove", item["path"])
        print(f"$ git worktree remove {item['path']}  rc={remove.returncode}")
        if remove.returncode != 0:
            print(remove.stderr.strip()[-400:])
            failures += 1
            continue
        delete = git("branch", "-D", item["branch"])
        print(f"$ git branch -D {item['branch']}  rc={delete.returncode}")
        if delete.returncode != 0:
            print(delete.stderr.strip()[-400:])
            failures += 1
    return failures


def self_test() -> int:
    failures: list[str] = []
    total = [0]

    def check(name: str, actual, expected) -> None:
        total[0] += 1
        if actual != expected:
            failures.append(f"FAIL {name}: 期望 {expected!r}，实际 {actual!r}")

    def wt(path: str, branch: str, head: str = "abc", detached: bool = False) -> dict:
        return {"path": path, "branch": branch, "head": head, "detached": detached}

    clean: dict[str, int] = {}
    merged = {"feat/done": "aaa"}

    # 1. main 缺失/重复
    red, _ = evaluate([wt("/r/main", "feat/x")], ["main", "feat/x"], {}, clean, None)
    check("main missing is red", any("main 必须" in r for r in red), True)
    red, _ = evaluate(
        [wt("/r/main", "main"), wt("/r/dup", "main")], ["main"], {}, clean, None
    )
    check("main twice is red", any("实际 2 个" in r for r in red), True)

    # 2. 共享主工作树必须在 main 且干净
    red, _ = evaluate(
        [wt("/r/main", "main"), wt("/r/ws", "feat/x")], ["main", "feat/x"], {}, clean,
        Path("/r/ws"), 20, 180, True,
    )
    check("workspace on feature is red", any("禁止在里面切分支" in r for r in red), True)
    red, _ = evaluate(
        [wt("/r/main", "main"), wt("/r/ws", "main")], ["main"], {}, {"/r/ws": 1},
        Path("/r/ws"), 20, 180, True,
    )
    check("workspace dirty is red", any("未提交改动" in r for r in red), True)

    # 3. 预算
    red, _ = evaluate(
        [wt("/r/main", "main")] + [wt(f"/r/w{i}", f"feat/{i}") for i in range(21)],
        ["main"] + [f"feat/{i}" for i in range(21)], {}, clean, None,
        20, 180,
    )
    check("worktree budget is red", any("worktree 数" in r for r in red), True)
    red, _ = evaluate(
        [wt("/r/main", "main")], ["main"] + [f"feat/{i}" for i in range(181)],
        {}, clean, None, 20, 180,
    )
    check("branch budget is red", any("本地分支" in r for r in red), True)

    # 4. 回收计划：四条件
    worktrees = [
        wt("/r/main", "main"),
        wt("/r/done", "feat/done", "aaa"),
        wt("/r/dirty", "feat/done", "aaa"),
        wt("/r/extra", "feat/done", "bbb"),
        wt("/r/open", "feat/open", "ccc"),
    ]
    dirty2 = {"/r/dirty": 2}
    plan, skipped = plan_reclaim(worktrees, merged, dirty2, set())
    check("only clean+matching+merged planned", [p["branch"] for p in plan], ["feat/done"])
    check("dirty skipped", any("未提交改动" in s for s in skipped), True)
    check("tip mismatch skipped", any("tip≠PR head" in s for s in skipped), True)
    plan, _ = plan_reclaim(worktrees, merged, dirty2, {norm("/r/done")})
    check("keep current dir", plan, [])

    print(("PASS " if not failures else "FAIL ") + f"self-test: {total[0]} checks")
    for failure in failures:
        print(failure)
    return 1 if failures else 0


def main(argv: list[str]) -> int:
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(encoding="utf-8", errors="replace")

    parser = argparse.ArgumentParser()
    parser.add_argument("--repo", default="bfxh/trit")
    parser.add_argument("--workspace", default=str(DEFAULT_WORKSPACE))
    parser.add_argument("--apply", action="store_true")
    parser.add_argument("--self-test", action="store_true")
    parser.add_argument("--max-worktrees", type=int, default=MAX_WORKTREES)
    parser.add_argument("--max-branches", type=int, default=MAX_BRANCHES)
    args = parser.parse_args(argv[1:])

    if args.self_test:
        return self_test()

    listed = git("worktree", "list", "--porcelain")
    if listed.returncode != 0:
        print("读不到 worktree 清单：" + listed.stderr.strip())
        return 1
    worktrees = parse_worktrees(listed.stdout)
    branches = [
        line.strip()[2:]
        for line in git("branch", "--list").stdout.splitlines()
        if line.strip()
    ]
    merged = load_merged(args.repo)
    dirty = {entry["path"]: dirty_of(entry["path"]) for entry in worktrees}
    workspace = Path(args.workspace)

    red, warn = evaluate(worktrees, branches, merged, dirty, workspace,
                         args.max_worktrees, args.max_branches)
    print(
        f"worktree_guard: worktrees={len(worktrees)} branches={len(branches)} "
        f"merged_pr_branches={len(merged)} 红={len(red)} 黄={len(warn)}"
    )
    for line in red:
        print("  [红] " + line)
    for line in warn:
        print("  [黄] " + line)

    if args.apply:
        keep = {norm(Path.cwd())}
        plan, skipped = plan_reclaim(worktrees, merged, dirty, keep)
        for line in skipped:
            print("  [保留] " + line)
        print(f"回收计划：{len(plan)} 个工作树")
        failures = apply_plan(plan)
        if failures:
            print(f"回收失败 {failures} 项")
            return 1
        remaining = parse_worktrees(git("worktree", "list", "--porcelain").stdout)
        branches = [
            line.strip()[2:]
            for line in git("branch", "--list").stdout.splitlines()
            if line.strip()
        ]
        red, _ = evaluate(remaining, branches, merged, {}, workspace,
                          args.max_worktrees, args.max_branches)
        print(f"回收后：worktrees={len(remaining)} branches={len(branches)} 红={len(red)}")
        for line in red:
            print("  [红] " + line)
        return 1 if red else 0

    return 1 if red else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))

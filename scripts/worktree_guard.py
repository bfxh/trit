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
  4. 不是主工作树、不是当前目录所在工作树（按 `git rev-parse --show-toplevel` 认，子目录里跑也算）。

回收时连带删除该工作树的外置 target（`scripts/wt_target.py` 生成的 `策略根/<树名>`）：
只删绝对路径、在树外、与工作树同名、没有别的工作树指向的那一份；树内 target 随
`git worktree remove` 一起走。2026-11-15 实测：规则写进 AGENTS.md 后仍攒下 33 个已合 PR 的
工作树——没人跑它，它也不删外置 target，所以 `gitflow.py worktree add` 新建前会自动跑一次 `--apply`。
"""

from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path


def norm(path: str | Path) -> str:
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


TARGET_DIR_RE = re.compile(r'^\s*(?:build\.)?target-dir\s*=\s*["\']([^"\']+)["\']', re.MULTILINE)


def configured_target(tree: Path) -> Path | None:
    """`<tree>/.cargo/config.toml` 里的 target-dir；没有文件或没写就 None。

    这份文件被忽略、会随 `git worktree remove` 消失，必须在删树之前读。
    """
    try:
        text = (tree / ".cargo" / "config.toml").read_text(encoding="utf-8", errors="replace")
    except OSError:
        return None
    match = TARGET_DIR_RE.search(text)
    return Path(match.group(1)) if match else None


def external_target(tree: Path, target: Path | None, others: set[str]) -> Path | None:
    """回收 `tree` 时可以连带删掉的外置 target，不满足任一条件就 None。

    绝对路径、在树外、目录名与工作树同名（策略根自己的名字对不上，永远不会被删）、
    不在 `others`（保留下来的工作树指向的 target）里。
    """
    if target is None or not target.is_absolute():
        return None
    t, w = norm(target), norm(tree)
    if t == w or t.startswith(w.rstrip("\\/") + os.sep):
        return None
    if os.path.normcase(target.name) != os.path.normcase(tree.name):
        return None
    if t in others:
        return None
    return target


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


def apply_plan(plan: list[dict], targets: dict[str, Path | None] | None = None) -> int:
    failures = 0
    for item in plan:
        remove = git("worktree", "remove", item["path"])
        print(f"$ git worktree remove {item['path']}  rc={remove.returncode}")
        if remove.returncode != 0:
            print(remove.stderr.strip()[-400:])
            failures += 1
            continue
        extra = (targets or {}).get(item["path"])
        if extra is not None and extra.exists():
            shutil.rmtree(extra, ignore_errors=True)
            state = "有残留（可能有进程占着文件）" if extra.exists() else "已删"
            print(f"$ rmtree {extra}  {state}")
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

    # 5. 外置 target：只删「策略根/<树名>」且没人共用的那份
    base = Path(tempfile.gettempdir()) / "worktree-guard-selftest"
    tree = base / "wt" / "trit-wt-a"
    same = base / "cargo-target" / "trit-wt-a"
    check("same-name external target deleted", external_target(tree, same, set()), same)
    check("policy root never deleted", external_target(tree, base / "cargo-target", set()), None)
    check("shared target kept", external_target(tree, same, {norm(same)}), None)
    check("target inside tree left to git", external_target(tree, tree / "trit-wt-a", set()), None)
    check("relative target ignored", external_target(tree, Path("../trit-wt-a"), set()), None)
    check("no target => nothing", external_target(tree, None, set()), None)
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        check("no config => None", configured_target(root), None)
        (root / ".cargo").mkdir()
        (root / ".cargo" / "config.toml").write_text(
            f'# generated\n[build]\ntarget-dir = "{same.as_posix()}"\n', encoding="utf-8"
        )
        check("target-dir parsed", configured_target(root), same)

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
        top = git("rev-parse", "--show-toplevel")
        if top.returncode == 0 and top.stdout.strip():
            keep.add(norm(top.stdout.strip()))
        if worktrees:  # `git worktree list` 第一项永远是主工作树
            keep.add(norm(worktrees[0]["path"]))
        plan, skipped = plan_reclaim(worktrees, merged, dirty, keep)
        for line in skipped:
            print("  [保留] " + line)
        planned = {norm(item["path"]) for item in plan}
        others: set[str] = set()
        for entry in worktrees:
            target = configured_target(Path(entry["path"]))
            if target is not None and norm(entry["path"]) not in planned:
                others.add(norm(target))
        targets = {
            item["path"]: external_target(Path(item["path"]), configured_target(Path(item["path"])), others)
            for item in plan
        }
        print(
            f"回收计划：{len(plan)} 个工作树"
            f"（连带外置 target {sum(t is not None for t in targets.values())} 个）"
        )
        failures = apply_plan(plan, targets)
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

#!/usr/bin/env python3
"""脏 worktree 对账：有未提交改动，却没人在效认领 ⇒ 判红。

为什么：认领账本只能证明"登记过"，证明不了"正在改这个东西的人登记了吗"。
2026-11-04 实测形状：一台机器 28 个 worktree 各站一支分支，6 个脏的里只有 2 个能被在效声明覆盖；
其中一个脏了 7 天、动的正是别人刚合进 main 的文件——两边都不知道对方存在。

用法：
    python scripts/claims_audit.py --repo bfxh/trit=D:/KF/trit \
        --repo bfxh/repo-tools=E:/tools/repo-tools [--claims PATH] [--json] [--self-test]

判据：
- **拦**：worktree 有未提交改动，但没有任何"在效声明"覆盖其中任一文件（repo 标注 + 路径前缀）；
- **警**：某条声明声明的路径在该仓里一个都不存在（仓名或路径写错，跨仓副本常见）；
- **警**：worktree 脏但它的声明已过期（人可能还在跑，也可能已经死了）。

这条门只读 git 与账本，不改任何文件。
"""

from __future__ import annotations

import argparse
import json
import pathlib
import subprocess
import sys

if hasattr(sys.stdout, "reconfigure"):
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import agent_claims as ac  # noqa: E402  （同一目录的闸门，口径必须共用同一套归一与在效判定）


def git(repo: pathlib.Path, args: list[str]) -> str:
    out = subprocess.run(["git", "-C", str(repo), *args], capture_output=True, text=True,
                         encoding="utf-8", errors="replace")
    return out.stdout if out.returncode == 0 else ""


def worktrees(repo: pathlib.Path) -> list[pathlib.Path]:
    paths: list[pathlib.Path] = []
    for line in git(repo, ["worktree", "list", "--porcelain"]).splitlines():
        if line.startswith("worktree "):
            paths.append(pathlib.Path(line.split(" ", 1)[1]))
    return paths


def dirty_files(worktree: pathlib.Path) -> list[str]:
    """未提交改动（含未跟踪）。rename 取新路径。"""
    files: list[str] = []
    for line in git(worktree, ["status", "--porcelain"]).splitlines():
        if len(line) < 4:
            continue
        path = line[3:]
        if " -> " in path:
            path = path.split(" -> ", 1)[1]
        files.append(path.strip().strip('"').replace("\\", "/"))
    return files


def covers(claim: dict, repo_label: str, file_path: str) -> bool:
    if claim["repo"] != repo_label:
        return False
    if not claim["paths"]:
        return True  # 空路径 = 整个仓
    return any(ac.paths_overlap(p.strip("/"), file_path)
               or ac.paths_overlap(file_path, p.strip("/")) for p in claim["paths"])


def audit(claims: list[dict], repos: dict[str, pathlib.Path], at: int) -> tuple[list[str], list[str], list[dict]]:
    red: list[str] = []
    warn: list[str] = []
    rows: list[dict] = []
    live = [c for c in claims if ac.expires_at_of(c) > at]
    for label, root in repos.items():
        if not root.is_dir():
            warn.append(f"{label} 的本地路径不存在：{root} ⇒ 这一仓没被核对")
            continue
        name = git(root, ["rev-parse", "--show-toplevel"]).strip()
        if not name:
            warn.append(f"{label} 不是 git 仓：{root} ⇒ 跳过")
            continue
        for worktree in worktrees(root):
            files = dirty_files(worktree)
            if not files:
                continue
            covering = [c for c in live if any(covers(c, label, f) for f in files)]
            stale = [c for c in claims if c["repo"] == label and ac.expires_at_of(c) <= at]
            rows.append({"repo": label, "worktree": str(worktree), "dirty": len(files),
                         "covering": [c["who"] for c in covering],
                         "stale": [c["who"] for c in stale]})
            if covering:
                continue
            if stale:
                warn.append(f"{label} {worktree} 有 {len(files)} 处未提交改动；"
                            f"只有过期声明（{', '.join(sorted({c['who'] for c in stale}))}）"
                            "——那个人可能还在跑，也可能已经死了，先问一句再动")
            else:
                red.append(f"{label} {worktree} 有 {len(files)} 处未提交改动，"
                           f"且没有任何在效声明覆盖（第一个是 {files[0]}）⇒ 先 claim 再动手")
    return red, warn, rows


def self_test() -> int:
    import tempfile

    checks: list[tuple[str, bool]] = []

    def check(label: str, got, want) -> None:
        checks.append((label, got == want))

    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp) / "repo"
        root.mkdir(parents=True)
        for argv in (["init", "-q"], ["add", "-A"]):
            subprocess.run(["git", *argv], cwd=root, check=True, capture_output=True)
        (root / "a.txt").write_text("hello\n", encoding="utf-8")
        subprocess.run(["git", "add", "-A"], cwd=root, check=True, capture_output=True)
        subprocess.run(["git", "-c", "user.email=t@e", "-c", "user.name=t", "commit", "-qm", "seed"],
                       cwd=root, check=True, capture_output=True)
        (root / "a.txt").write_text("changed\n", encoding="utf-8")

        at = 1_000_000
        live_claim = {"id": "c1", "who": "qoder:s1", "repo": "o/r", "paths": ["a.txt"],
                      "ts": at - 60, "ttl": 90, "note": "", "renewed": 0}
        other_path = {"id": "c2", "who": "qoder:s2", "repo": "o/r", "paths": ["docs/x.md"],
                      "ts": at - 60, "ttl": 90, "note": "", "renewed": 0}
        stale = {"id": "c3", "who": "zcode:z", "repo": "o/r", "paths": ["a.txt"],
                 "ts": at - 99999, "ttl": 90, "note": "", "renewed": 0}

        red, warn, rows = audit([live_claim], {"o/r": root}, at)
        check("在效声明覆盖脏文件 => 绿", (red, len(rows)), ([], 1))
        check("覆盖关系记在明细里", rows[0]["covering"], ["qoder:s1"])

        red, _warn, _rows = audit([other_path], {"o/r": root}, at)
        check("声明的是别的路径 => 拦", len(red), 1)
        check("拦的消息点名第一个脏文件", "a.txt" in red[0], True)

        red, warn, _rows = audit([stale], {"o/r": root}, at)
        check("只有过期声明 => 警而不是拦（人可能还在跑）", (len(red), len(warn)), (0, 1))
        check("过期警告点名是谁", "zcode:z" in warn[0], True)

        red, _warn, _rows = audit([], {"o/r": root}, at)
        check("一条声明都没有 => 拦", len(red), 1)

        red, warn, _rows = audit([live_claim], {"o/r": root / "nope"}, at)
        check("仓路径不存在 => 只警告（换台机器不该满屏红）", (len(red), len(warn)), (0, 1))

        clean_claim = dict(live_claim, paths=[""])
        subprocess.run(["git", "checkout", "--", "a.txt"], cwd=root, check=True, capture_output=True)
        red, _warn, rows = audit([clean_claim], {"o/r": root}, at)
        check("worktree 干净 => 不看声明也绿", (red, rows), ([], []))

        (root / "新 文件.txt").write_text("x\n", encoding="utf-8")
        red, _warn, _rows = audit([other_path], {"o/r": root}, at)
        check("未跟踪文件也算脏（README 之外的常见漏网）", len(red), 1)
        whole_repo = dict(live_claim, paths=[])
        check("空 paths 声明 = 整个仓（这时 worktree 确实是脏的）",
              audit([whole_repo], {"o/r": root}, at)[0], [])

    failed = [label for label, ok in checks if not ok]
    for label, ok in checks:
        print(f"  {'FAILED' if not ok else 'ok    '} {label}")
    print(f"self-test: {len(checks) - len(failed)}/{len(checks)} passed")
    return 1 if failed else 0


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--claims", default=str(ac.DEFAULT_CLAIMS))
    parser.add_argument("--repo", action="append", default=[],
                        help="o/r=本地路径，可重复")
    parser.add_argument("--json", action="store_true")
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args(argv)
    if args.self_test:
        return self_test()
    repos: dict[str, pathlib.Path] = {}
    for item in args.repo:
        if "=" not in item:
            print(f"FAIL --repo 要写成 o/r=路径：{item!r}", file=sys.stderr)
            return 2
        label, path = item.split("=", 1)
        repos[label.strip()] = pathlib.Path(path.strip())
    if not repos:
        print("FAIL 至少给一个 --repo o/r=路径", file=sys.stderr)
        return 2
    path = pathlib.Path(args.claims)
    claims, broken = (ac.parse_claims(path.read_text(encoding="utf-8"))
                      if path.is_file() else ([], []))
    red, warn, rows = audit(claims, repos, ac.now())
    red += [f"账本有解析不了的行：{line}" for line in broken]
    if args.json:
        print(json.dumps({"tool": "claims-audit", "red": red, "warn": warn,
                          "rows": rows, "green": not red}, ensure_ascii=False, indent=2))
    else:
        print(f"worktree {len(rows)} 个脏｜拦 {len(red)} 条｜警 {len(warn)} 条")
        for line in red:
            print(f"  [拦] {line}")
        for line in warn:
            print(f"  [警] {line}")
        print("结论：" + ("绿（每个脏 worktree 都有人在效认领）" if not red
                        else "红（有人在动没人登记的东西）"))
    return 1 if red else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))

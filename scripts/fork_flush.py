#!/usr/bin/env python3
"""Fork-and-Flush 编排原型（arXiv:2610.07447 的工程版）。

论文的核心是两点：把同一个任务分叉成多条**独立轨迹**（各自继承工作区、但清空对话历史），
跑完固定 horizon 后从**验收分数最高**的那条继续。本脚本只做编排，不调用任何模型：

  plan   为同一任务开 N 个 worktree（分支 fork/<task>-i），每个写 FORK_TASK.md
  score  在每个 worktree 里跑验收命令，解析最后一行的 `SCORE=<float>`
  pick   读分数，选最高；`--merge` 把胜者分支合入当前分支之前的 base
  status 列出各轨迹的分数与 returncode

约定：验收命令在 stdout 最后一行输出 `SCORE=<float>`（越大越好）。
"""

import argparse
import json
import re
import subprocess
import sys
import tempfile
from pathlib import Path

SCORE_RE = re.compile(r"^SCORE=([-+0-9.eE]+)\s*$", re.MULTILINE)
SCORE_FILE = ".fork-score.json"


def repo_root(start: Path) -> Path:
    out = subprocess.run(
        ["git", "rev-parse", "--show-toplevel"],
        cwd=start,
        capture_output=True,
        text=True,
    )
    if out.returncode != 0:
        raise SystemExit(f"不在 git 仓库里：{out.stderr.strip()}")
    return Path(out.stdout.strip())


def slug(text: str) -> str:
    text = Path(text).stem.lower()
    return re.sub(r"[^a-z0-9]+", "-", text).strip("-")[:40] or "task"


def cmd_plan(args: argparse.Namespace) -> int:
    task_path = Path(args.task).resolve()
    if not task_path.is_file():
        raise SystemExit(f"任务文件不存在：{task_path}")
    task = task_path.read_text(encoding="utf-8")
    root = repo_root(Path.cwd())
    name = slug(args.task)
    body = (
        f"# FORK TASK: {name}\n\n{task}\n\n"
        "## Fork-and-Flush 规则\n\n"
        "- 这是一条独立轨迹：不要读其他 fork 的对话或结果。\n"
        "- 从干净上下文开始（Flush）：只带任务与工作区，不带之前的探索历史。\n"
        f"- 固定 horizon：最多 {args.horizon} 次尝试；到点就停，不要无限细化。\n"
        "- 收工时运行文档里的验收命令，并在 stdout 最后一行输出 `SCORE=<float>`。\n"
    )
    made = []
    for i in range(1, args.n + 1):
        wt = root.parent / f"{root.name}-{args.prefix}-{i}"
        branch = f"fork/{name}-{i}"
        if wt.exists():
            print(f"跳过已存在：{wt}")
            made.append(wt)
            continue
        subprocess.run(
            ["git", "worktree", "add", str(wt), "-b", branch, args.base],
            cwd=root,
            check=True,
        )
        (wt / "FORK_TASK.md").write_text(body, encoding="utf-8")
        print(wt)
        made.append(wt)
    print(f"已开 {len(made)} 条轨迹；每条从干净上下文开始，收工用 `score` 收分。")
    return 0


def cmd_score(args: argparse.Namespace) -> int:
    runs = [Path(r).resolve() for r in args.runs]
    results = {}
    for run in runs:
        proc = subprocess.run(
            args.cmd,
            shell=True,
            cwd=run,
            capture_output=True,
            text=True,
        )
        match = SCORE_RE.search(proc.stdout)
        score = float(match.group(1)) if match else None
        record = {
            "cmd": args.cmd,
            "returncode": proc.returncode,
            "score": score,
        }
        (run / SCORE_FILE).write_text(json.dumps(record), encoding="utf-8")
        results[str(run)] = record
        print(f"{run}\tscore={score}\trc={proc.returncode}")
    return 0 if all(v["score"] is not None for v in results.values()) else 1


def load_scores(runs: list[Path]) -> list[tuple[Path, float | None]]:
    out = []
    for run in runs:
        path = run / SCORE_FILE
        score = None
        if path.is_file():
            score = json.loads(path.read_text(encoding="utf-8")).get("score")
        out.append((run, score))
    return out


def cmd_pick(args: argparse.Namespace) -> int:
    runs = [Path(r).resolve() for r in args.runs]
    scored = [(r, s) for r, s in load_scores(runs) if s is not None]
    if not scored:
        raise SystemExit("没有可用的分数；先跑 `score`")
    scored.sort(key=lambda item: item[1], reverse=True)
    best, score = scored[0]
    print(f"最佳：{best}\tscore={score}")
    if args.merge:
        branch = subprocess.run(
            ["git", "rev-parse", "--abbrev-ref", "HEAD"],
            cwd=best,
            capture_output=True,
            text=True,
            check=True,
        ).stdout.strip()
        subprocess.run(["git", "merge", "--no-ff", branch], cwd=Path.cwd(), check=True)
        print(f"已合入 {branch}")
    return 0


def cmd_status(args: argparse.Namespace) -> int:
    for run, score in load_scores([Path(r).resolve() for r in args.runs]):
        print(f"{run}\tscore={score}")
    return 0


def self_test() -> int:
    with tempfile.TemporaryDirectory() as tmp:
        base = Path(tmp)
        a, b = base / "a", base / "b"
        a.mkdir()
        b.mkdir()
        (a / SCORE_FILE).write_text(json.dumps({"score": 1.0}), encoding="utf-8")
        (b / SCORE_FILE).write_text(json.dumps({"score": 2.0}), encoding="utf-8")
        scored = load_scores([a, b])
        assert scored[1][1] == 2.0, scored
        assert SCORE_RE.search("noise\nSCORE=3.5\n").group(1) == "3.5"
        assert SCORE_RE.search("SCORE=nan") is None
        print("self-test ok")
    return 0


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    sub = ap.add_subparsers(dest="cmd", required=True)

    p = sub.add_parser("plan", help="为同一任务开 N 个独立 worktree")
    p.add_argument("--task", required=True)
    p.add_argument("--n", type=int, default=2)
    p.add_argument("--base", default="main")
    p.add_argument("--prefix", default="fork")
    p.add_argument("--horizon", type=int, default=8)
    p.set_defaults(func=cmd_plan)

    s = sub.add_parser("score", help="在每个轨迹里跑验收命令并记分")
    s.add_argument("--runs", nargs="+", required=True)
    s.add_argument("--cmd", required=True)
    s.set_defaults(func=cmd_score)

    k = sub.add_parser("pick", help="选最高分轨迹")
    k.add_argument("--runs", nargs="+", required=True)
    k.add_argument("--merge", action="store_true")
    k.set_defaults(func=cmd_pick)

    st = sub.add_parser("status", help="列出分数")
    st.add_argument("--runs", nargs="+", required=True)
    st.set_defaults(func=cmd_status)

    t = sub.add_parser("self-test", help="离线自检")
    t.set_defaults(func=lambda _: self_test())

    args = ap.parse_args()
    return args.func(args)


if __name__ == "__main__":
    sys.exit(main())

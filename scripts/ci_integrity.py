#!/usr/bin/env python3
"""CI 完整性门：让"分支保护 / workflow / 必需检查"三者对不上时判红。

为什么需要它（真实混乱场景）：
  - 保护规则里写了一个**拼错**的检查名 ⇒ 该 context 永不到来 ⇒ 所有 PR 永久卡死；
  - 某个 job 的 workflow 只在 push 触发 ⇒ 设成必需后 PR 永远等不到它；
  - 改了 workflow 的 job 名但没同步保护规则 ⇒ 旧名成为幽灵必需项；
  - 反过来：门 job 没进必需列表 ⇒ 检查是绿的，但没人拿它当门槛。

用法：
  python scripts/ci_integrity.py                # 本地：workflows ↔ .github/required-checks.txt
  python scripts/ci_integrity.py --remote       # 再核对 GitHub 上真实的 required_status_checks
  python scripts/ci_integrity.py --self-test
"""

from __future__ import annotations

import json
import pathlib
import re
import subprocess
import sys
import tempfile

ROOT = pathlib.Path(__file__).resolve().parents[1]
WORKFLOWS = pathlib.Path(".github/workflows")
REQUIRED_FILE = pathlib.Path(".github/required-checks.txt")

JOB_ID = re.compile(r"^  ([A-Za-z0-9_-]+):\s*$")
JOB_NAME = re.compile(r"^    name:\s*(.+?)\s*$")
JOB_IF = re.compile(r"^    if:\s*(.+?)\s*$")
ON_PULL = re.compile(r"^\s{2,}pull_request(-[a-z_]+)?:\s*$|^\s{2}pull_request:\s*$")


def parse_workflows(root: pathlib.Path) -> list[dict]:
    """不依赖 YAML 库：按缩进抠出 job 名、是否带 if、workflow 是否在 PR 上触发。"""
    jobs: list[dict] = []
    for path in sorted((root / WORKFLOWS).glob("*.yml")) + sorted((root / WORKFLOWS).glob("*.yaml")):
        text = path.read_text(encoding="utf-8")
        in_jobs = False
        current: dict | None = None
        for line in text.splitlines():
            if re.match(r"^jobs:\s*$", line):
                in_jobs = True
                continue
            if in_jobs and re.match(r"^[a-z]", line):
                in_jobs = False
            if not in_jobs:
                continue
            job_id = JOB_ID.match(line)
            if job_id:
                current = {"workflow": path.name, "id": job_id.group(1), "name": "", "if": "",
                           "on_pr": False, "has_matrix": False}
                jobs.append(current)
                continue
            if current is None:
                continue
            name = JOB_NAME.match(line)
            if name:
                current["name"] = name.group(1).strip("'\"")
            guard = JOB_IF.match(line)
            if guard:
                current["if"] = guard.group(1).strip()
            if re.match(r"^\s+matrix:\s*$", line):
                current["has_matrix"] = True
        on_pr = bool(re.search(r"(?m)^on:[^#]*?\bpull_request\b", text)) or bool(
            re.search(r"(?m)^\s{2}pull_request", text)
        )
        for job in jobs:
            if job["workflow"] == path.name:
                job["on_pr"] = on_pr
    return jobs


MATRIX_TOKEN = re.compile(r"\$\{\{\s*matrix\.\w+\s*\}\}")


def name_pattern(template: str) -> re.Pattern[str]:
    """矩阵 job 的名字是模板（`fmt + clippy + test (${{ matrix.os }})`），把矩阵占位当通配。"""
    parts = MATRIX_TOKEN.split(template)
    return re.compile(".*".join(re.escape(part) for part in parts))


def matches_context(job: dict, context: str) -> bool:
    name = job["name"] or job["id"]
    if name_pattern(name).fullmatch(context):
        return True
    # GitHub 会把矩阵值拼在名字后面：`fmt + clippy + test` + ` (ubuntu-latest)`
    if job.get("has_matrix") and context.startswith(name + " (") and context.endswith(")"):
        return True
    return False


def validate(jobs: list[dict], required: list[str], remote: list[str] | None = None) -> list[str]:
    problems: list[str] = []
    for context in required:
        matches = [job for job in jobs if matches_context(job, context)]
        if not matches:
            problems.append(f"必需检查在 workflow 里找不到同名 job：{context}（拼错或 job 改名了）")
            continue
        if len(matches) > 1:
            problems.append(f"必需检查名不唯一：{context} 出现在 {[m['workflow'] for m in matches]}")
        for job in matches:
            if not job["on_pr"]:
                problems.append(f"{context} 所在 workflow（{job['workflow']}）不在 pull_request 上触发 ⇒ PR 永远等不到")
            cond = job["if"]
            negated = re.search(r"!=\s*['\"]pull_request['\"]", cond) is not None
            if cond and ("pull_request" not in cond or negated):
                problems.append(f"{context} 的 if 条件不覆盖 pull_request（{cond}）⇒ 某些 PR 会等不到它")

    if remote is not None:
        missing_remote = sorted(set(required) - set(remote))
        extra_remote = sorted(set(remote) - set(required))
        if missing_remote:
            problems.append(f"本地清单有、GitHub 上没设：{missing_remote}")
        if extra_remote:
            problems.append(f"GitHub 上设了、本地清单没有：{extra_remote}")
    return problems


def read_required(root: pathlib.Path) -> list[str]:
    path = root / REQUIRED_FILE
    if not path.is_file():
        return []
    return [
        line.strip()
        for line in path.read_text(encoding="utf-8").splitlines()
        if line.strip() and not line.strip().startswith("#")
    ]


def remote_required(repo: str) -> list[str] | None:
    result = subprocess.run(
        ["gh", "api", f"repos/{repo}/branches/main/protection/required_status_checks"],
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
        encoding="utf-8",
        errors="replace",
    )
    if result.returncode != 0:
        print("读不到远端保护规则（跳过远端核对）：")
        print(result.stdout[-500:])
        return None
    try:
        return json.loads(result.stdout).get("contexts", [])
    except json.JSONDecodeError:
        return None


def self_test() -> int:
    failures: list[str] = []
    total = [0]

    def check(name: str, actual, expected) -> None:
        total[0] += 1
        if actual != expected:
            failures.append(name)
            print(f"FAIL {name}：期望 {expected!r}，实际 {actual!r}")

    jobs = [
        {"workflow": "ci.yml", "id": "check", "name": "fmt + clippy + test (ubuntu-latest)", "if": "", "on_pr": True},
        {"workflow": "ci.yml", "id": "budget", "name": "module budget (god objects)", "if": "", "on_pr": True},
        {"workflow": "t3.yml", "id": "check", "name": "t3 fmt + clippy + test", "if": "", "on_pr": True},
        {"workflow": "push-only.yml", "id": "x", "name": "push only gate", "if": "", "on_pr": False},
        {"workflow": "ci.yml", "id": "y", "name": "guarded gate", "if": "github.event_name != 'pull_request'", "on_pr": True},
    ]
    good = ["fmt + clippy + test (ubuntu-latest)", "module budget (god objects)", "t3 fmt + clippy + test"]
    check("正常清单无问题", validate(jobs, good), [])
    check("拼错检查名判红", any("找不到" in p for p in validate(jobs, good + ["fmt + clippy + test (ubunto)"])), True)
    check("只在 push 触发的 job 判红", any("不在 pull_request" in p for p in validate(jobs, ["push only gate"])), True)
    check("if 为 pull_request 条件时放行",
          validate(jobs + [{"workflow": "ci.yml", "id": "z", "name": "pr only gate",
                            "if": "github.event_name == 'pull_request'", "on_pr": True}], ["pr only gate"]), [])
    check("if 条件不覆盖 PR 时判红", any("if 条件不覆盖" in p for p in validate(jobs, ["guarded gate"])), True)
    matrix_job = [{"workflow": "ci.yml", "id": "check", "name": "fmt + clippy + test (${{ matrix.os }})",
                   "if": "", "on_pr": True}]
    check("矩阵模板匹配展开后的名字", validate(matrix_job, ["fmt + clippy + test (ubuntu-latest)"]), [])
    suffix_job = [{"workflow": "ci.yml", "id": "check", "name": "fmt + clippy + test", "if": "",
                   "on_pr": True, "has_matrix": True}]
    check("矩阵后缀匹配（名字里没有占位）",
          validate(suffix_job, ["fmt + clippy + test (windows-latest)"]), [])
    check("无 matrix 时后缀不算匹配",
          validate([dict(suffix_job[0], has_matrix=False)], ["fmt + clippy + test (x)"]),
          validate([dict(suffix_job[0], has_matrix=False)], ["fmt + clippy + test (x)"]))
    check("无 matrix 后缀确实判红",
          any("找不到" in p for p in validate([dict(suffix_job[0], has_matrix=False)],
                                              ["fmt + clippy + test (x)"])), True)
    drift = validate(jobs, good, remote=good[:-1] + ["stale-name"])
    check("远端漂移判红", bool(drift), True)

    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        (root / WORKFLOWS).mkdir(parents=True)
        (root / WORKFLOWS / "ci.yml").write_text(
            "name: CI\non:\n  pull_request:\n\njobs:\n  check:\n    name: fmt + clippy + test\n    runs-on: ubuntu-latest\n",
            encoding="utf-8",
        )
        (root / REQUIRED_FILE).write_text("# 注释\nfmt + clippy + test\n", encoding="utf-8")
        parsed = parse_workflows(root)
        check("解析出 job 名", [j["name"] for j in parsed], ["fmt + clippy + test"])
        check("解析出 on.pull_request", parsed[0]["on_pr"], True)
        check("读必需清单跳过注释", read_required(root), ["fmt + clippy + test"])
        check("端到端无问题", validate(parsed, read_required(root)), [])

    if failures:
        print(f"self-test: {total[0] - len(failures)}/{total[0]} passed")
        return 1
    print(f"self-test: {total[0]}/{total[0]} passed")
    return 0


def main(argv: list[str]) -> int:
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    if "--self-test" in argv:
        return self_test()
    root = ROOT
    if "--root" in argv:
        root = pathlib.Path(argv[argv.index("--root") + 1]).resolve()
    repo = argv[argv.index("--repo") + 1] if "--repo" in argv else "bfxh/trit"
    required = read_required(root)
    if not required:
        print(f"{REQUIRED_FILE} 为空或不存在 ⇒ 判红（否则这层等于没开）")
        return 1
    problems = validate(parse_workflows(root), required, remote_required(repo) if "--remote" in argv else None)
    if problems:
        print(f"CI 完整性失败（{len(problems)} 项）：")
        for problem in problems:
            print(f"  - {problem}")
        return 1
    print(f"CI 完整性通过：{len(required)} 项必需检查都与 workflow 对得上")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))

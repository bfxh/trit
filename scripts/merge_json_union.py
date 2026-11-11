#!/usr/bin/env python3
"""棘轮 JSON 的 git 合并驱动：按策略做确定性 union，不产生冲突标记。

用法（由 `.gitattributes` + `git config merge.json-union.driver` 调起）：
    python scripts/merge_json_union.py <base> <ours> <theirs>

git 约定：结果写回 `<ours>`，退出 0 表示合并成功；退出 1 表示冲突（fail-closed）。

策略（按文件形状识别，不认识就退出 1，交给 git 报冲突）：
  - 测试面棘轮（每个单位一个测试数）：键并集，值取 **max**（计数只增）；
  - 模块预算台账（`caps` / `over_budget` 列表）：按文件并集，行数取 **min**（上限只降）；
  - 文档卫生台账（`changelog` + `drive_paths`）：`changelog` 取 min，`drive_paths` 按文件取 min。

自测：`python scripts/merge_json_union.py --self-test`
（纯函数之外，还真的起临时 git 仓让 git 调这个驱动合并一次——纯函数测过不等于接上线了。）
"""

from __future__ import annotations

import json
import os
import pathlib
import subprocess
import sys
import tempfile
from pathlib import Path


def load(path: str) -> dict:
    return json.loads(Path(path).read_text(encoding="utf-8"))


def merge_caps(left: list[dict], right: list[dict]) -> list[dict]:
    merged: dict[str, int] = {}
    for entry in left + right:
        name = entry["file"]
        lines = int(entry["lines"])
        merged[name] = min(merged.get(name, lines), lines)
    return [{"file": name, "lines": lines} for name, lines in sorted(merged.items())]


def merge_flat_counts(left: dict, right: dict) -> dict:
    merged: dict[str, int] = {}
    for key in set(left) | set(right):
        merged[key] = max(int(left.get(key, 0)), int(right.get(key, 0)))
    return dict(sorted(merged.items()))


def merge_docs_hygiene(left: dict, right: dict) -> dict:
    merged = dict(left)
    if "changelog" in left or "changelog" in right:
        merged["changelog"] = min(int(left.get("changelog", 0)), int(right.get("changelog", 0)))
    paths: dict[str, int] = {}
    for source in (left.get("drive_paths", {}), right.get("drive_paths", {})):
        for key, value in source.items():
            paths[key] = min(int(paths.get(key, value)), int(value))
    if "drive_paths" in left or "drive_paths" in right:
        merged["drive_paths"] = dict(sorted(paths.items()))
    return merged


def merge(ours: dict, theirs: dict) -> tuple[dict, int]:
    """返回 (合并结果, 推荐缩进)；不认识形状抛 ValueError。"""
    if isinstance(ours, dict) and ("caps" in ours or "caps" in theirs):
        merged = dict(ours)
        merged["caps"] = merge_caps(ours.get("caps", []), theirs.get("caps", []))
        # 兼容旧格式：两边都没有 caps 时走 over_budget
        if "over_budget" in ours or "over_budget" in theirs:
            merged["over_budget"] = merge_caps(
                ours.get("over_budget", []), theirs.get("over_budget", [])
            )
        return merged, 2
    if isinstance(ours, dict) and ("changelog" in ours or "drive_paths" in ours):
        return merge_docs_hygiene(ours, theirs), 1
    if (
        isinstance(ours, dict)
        and isinstance(theirs, dict)
        and (not ours or all(isinstance(v, int) for v in ours.values()))
        and (not theirs or all(isinstance(v, int) for v in theirs.values()))
    ):
        return merge_flat_counts(ours, theirs), 1
    raise ValueError("不认识的 JSON 形状")


def _git(repo: pathlib.Path, *args: str) -> tuple[int, str]:
    proc = subprocess.run(
        ["git", "-C", str(repo), *args],
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        env={**os.environ, "GIT_MERGE_AUTOEDIT": "no"},
    )
    return proc.returncode, proc.stdout.decode("utf-8", "replace")


def _driver_command() -> str:
    exe = (sys.executable or "python").replace("\\", "/")
    script = str(pathlib.Path(__file__).resolve()).replace("\\", "/")
    return f'"{exe}" "{script}" %O %A %B'


def merge_scenario(shape: str) -> tuple[int, str]:
    """真起一个临时仓：让 git 按 .gitattributes 调本驱动合并一次。

    shape="union" 用测试面形状（键并集 max）；shape="weird" 用不认识形状。
    返回 (merge 退出码, 合并后的文件内容)；冲突时内容为空串。
    """
    with tempfile.TemporaryDirectory() as tmp:
        repo = pathlib.Path(tmp)
        _git(repo, "init", "-q")
        _git(repo, "config", "user.email", "merge@example.invalid")
        _git(repo, "config", "user.name", "merge selftest")
        _git(repo, "config", "commit.gpgsign", "false")
        _git(repo, "config", "merge.json-union.driver", _driver_command())
        name = "ledger.json"
        (repo / ".gitattributes").write_text(f"{name} merge=json-union\n", encoding="utf-8")
        base = {"a": 1} if shape == "union" else {"w": [1]}
        mine = {"b": 2} if shape == "union" else {"w": [3]}
        theirs = {"a": 3} if shape == "union" else {"w": [2]}
        (repo / name).write_text(json.dumps(base) + "\n", encoding="utf-8")
        _git(repo, "add", "-A")
        _git(repo, "commit", "-q", "-m", "base")
        default = _git(repo, "symbolic-ref", "--short", "HEAD")[1].strip()
        for branch, payload in (("side", theirs), ("mine", mine)):
            _git(repo, "switch", "-c", branch, default)
            (repo / name).write_text(json.dumps(payload) + "\n", encoding="utf-8")
            _git(repo, "add", "-A")
            _git(repo, "commit", "-q", "-m", branch)
        _git(repo, "switch", "mine")
        rc, _ = _git(repo, "merge", "--no-edit", "side")
        content = (repo / name).read_text(encoding="utf-8") if rc == 0 else ""
        return rc, content


def main(argv: list[str]) -> int:
    for stream in (sys.stdout, sys.stderr):
        if hasattr(stream, "reconfigure"):
            stream.reconfigure(encoding="utf-8", errors="replace")
    if "--self-test" in argv:
        return self_test()
    if len(argv) != 4:
        print(__doc__)
        return 2
    _, base_path, ours_path, theirs_path = argv
    try:
        ours = load(ours_path)
        theirs = load(theirs_path)
        _ = load(base_path)  # 只为校验 base 是合法 JSON
        merged, indent = merge(ours, theirs)
    except (OSError, ValueError, KeyError, json.JSONDecodeError) as exc:
        print(f"json-union: 无法安全合并（{exc}）——留给 git 报冲突", file=sys.stderr)
        return 1
    Path(ours_path).write_text(
        json.dumps(merged, ensure_ascii=False, indent=indent) + "\n",
        encoding="utf-8",
        newline="\n",
    )
    return 0


def self_test() -> int:
    failures: list[str] = []
    total = [0]

    def check(name: str, actual, expected) -> None:
        total[0] += 1
        if actual != expected:
            failures.append(name)
            print(f"FAIL {name}: 期望 {expected!r}，实际 {actual!r}")

    # 测试面：max
    merged, indent = merge({"a": 3, "b": 5}, {"a": 4, "c": 1})
    check("flat union+max", merged, {"a": 4, "b": 5, "c": 1})
    check("flat indent", indent, 1)

    # 模块预算：min
    merged, indent = merge(
        {"limit": 800, "caps": [{"file": "x.rs", "lines": 10}, {"file": "y.rs", "lines": 20}]},
        {"limit": 800, "caps": [{"file": "x.rs", "lines": 8}, {"file": "z.rs", "lines": 5}]},
    )
    check(
        "caps union+min",
        merged["caps"],
        [
            {"file": "x.rs", "lines": 8},
            {"file": "y.rs", "lines": 20},
            {"file": "z.rs", "lines": 5},
        ],
    )
    check("caps indent", indent, 2)
    check("caps keeps limit", merged["limit"], 800)

    # 文档卫生：changelog min；drive_paths 并集 min
    merged, _ = merge(
        {"changelog": 100, "drive_paths": {"a.md": 3, "b.md": 2}},
        {"changelog": 120, "drive_paths": {"a.md": 1, "c.md": 4}},
    )
    check("docs changelog min", merged["changelog"], 100)
    check("docs drive_paths", merged["drive_paths"], {"a.md": 1, "b.md": 2, "c.md": 4})

    # 不认识的形状 ⇒ 抛错（调用方退出 1）
    try:
        merge({"weird": [1, 2]}, {"weird": [3]})
        check("unknown shape raises", False, True)
    except ValueError:
        check("unknown shape raises", True, True)

    # 接线：让 git 真的按 .gitattributes 调用本驱动
    rc, content = merge_scenario("union")
    check("git 接线：合并成功", rc, 0)
    try:
        parsed = json.loads(content)
    except json.JSONDecodeError:
        parsed = None
    check("git 接线：union 结果", parsed, {"a": 3, "b": 2})
    check("git 接线：无冲突标记", "<<<<<<<" in content, False)

    rc, _ = merge_scenario("weird")
    check("git 接线：不认识形状留给冲突", rc != 0, True)

    if failures:
        print(f"self-test: {total[0] - len(failures)}/{total[0]} passed")
        return 1
    print(f"self-test: {total[0]}/{total[0]} passed")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))

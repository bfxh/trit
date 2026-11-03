#!/usr/bin/env python3
"""模块预算门（**默认全量棘轮**）：每个 .rs 文件都自带一份"自己当前行数"的上限。

**为什么改**（旧口径的三个洞，2026-11-01 实测）：
  1. 只扫 `crates/*/**/*.rs` ⇒ `fuzz/`、`benchmarks/` 里的 9 个 .rs 不在面上；
  2. 台账**只登记超 800 的文件**，而 trit 最大文件 788 行 ⇒ 台账空 ⇒ 对任何文件
     **没有任何"不得变胖"约束**（往 700 行的文件里再加 90 行完全合法）；
  3. 棘轮只在跨过 800 之后生效 ⇒ "上帝对象"要长到 800 才有人管。

**新判定（红 = 退出码 1）**：
  - `GREW`      行数 > 自己的台账上限 —— 不管在不在 800 内，**这就是"默认有门禁"**；
  - `UNCAPPED`  扫描面上出现没登记的文件（新文件必须 `--update` 登记）；
  - `OVER`      未登记且已超 `LIMIT`（登记会被拒绝：先拆）。
**提醒（不判红）**：`STALE`（变瘦了，跑 `--update` 收紧）、`ORPHAN`（台账里的文件没了）。

**依赖面**：扫描 = 整个仓库的 `.rs`，只跳 `target/.git/__pycache__/...` ⇒ 新增 crate、
`xtask`、`fuzz`、`benchmarks` **默认就在门禁里**，不需要再往配置里登记。

用法：
    python scripts/module_budget.py            # 检查
    python scripts/module_budget.py --update   # 登记新文件 / 收紧台账（**永不抬高**）
    python scripts/module_budget.py --self-test  # 正例 + 逐条反证
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
BASELINE = ROOT / "scripts" / "module_budget_baseline.json"

# 单个源文件的硬预算：新建文件超过它就是"上帝对象"，登记会被拒绝。
LIMIT = 800

# 扫描跳过这些目录名（构建产物 / 版本库 / 缓存）——与 BSHSQ 的 gates_common 同口径。
EXCLUDE_DIRS = {"target", ".git", "__pycache__", "node_modules", ".venv", "venv"}


def scan(root: Path) -> dict[str, int]:
    """仓库里**全部** .rs 文件的行数（依赖面默认在内，不再有第二个清单）。"""
    sizes: dict[str, int] = {}
    for dirpath, dirnames, filenames in os.walk(root):
        dirnames[:] = sorted(d for d in dirnames if d not in EXCLUDE_DIRS)
        for name in sorted(filenames):
            if not name.endswith(".rs"):
                continue
            path = Path(dirpath) / name
            rel = path.relative_to(root).as_posix()
            text = path.read_text(encoding="utf-8", errors="replace")
            sizes[rel] = len(text.splitlines())
    return sizes


def load_baseline(path: Path) -> dict[str, int]:
    """台账 = `{file: 行数上限}`。旧格式（只有 `over_budget` 债务清单）也能读。"""
    if not path.exists():
        return {}
    with path.open(encoding="utf-8") as handle:
        data = json.load(handle)
    if "caps" in data:
        return {e["file"]: int(e["lines"]) for e in data["caps"]}
    return {e["file"]: int(e["lines"]) for e in data.get("over_budget", [])}


def save_baseline(path: Path, caps: dict[str, int]) -> None:
    payload = {
        "limit": LIMIT,
        "note": (
            "默认全量棘轮：每个 .rs 一条上限。--update 只登记新文件与收紧，**永不抬高**；"
            "抬高 = 把变胖写成合法，判红。"
        ),
        "caps": [{"file": f, "lines": n} for f, n in sorted(caps.items())],
    }
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("w", encoding="utf-8", newline="\n") as handle:
        json.dump(payload, handle, ensure_ascii=False, indent=2)
        handle.write("\n")


def check(root: Path, baseline_path: Path) -> tuple[list[str], list[str]]:
    """返回 `(判红, 仅提醒)`。"""
    sizes = scan(root)
    caps = load_baseline(baseline_path)
    failures: list[str] = []
    notices: list[str] = []

    for file, lines in sorted(sizes.items()):
        cap = caps.get(file)
        if cap is None:
            if lines > LIMIT:
                failures.append(
                    f"OVER: {file} {lines} > 预算 {LIMIT} 且未登记 —— 先拆，再 --update"
                )
            else:
                failures.append(f"UNCAPPED: {file} {lines} 行不在台账（跑 --update 登记）")
            continue
        if lines > cap:
            failures.append(f"GREW: {file} {lines} > 台账上限 {cap}（要腾地方就拆，别抬台账）")
        elif lines < cap and cap > LIMIT:
            # 债务文件变瘦是好事，但台账必须同步收紧，否则"还剩多少债"是假数。
            notices.append(
                f"STALE: {file} 已降到 {lines}，债务台账仍写 {cap}；跑 --update 收紧"
            )
        elif lines < cap:
            notices.append(f"STALE: {file} 变瘦到 {lines}（上限 {cap}），可 --update 收紧")

    for file in sorted(caps):
        if file not in sizes:
            notices.append(f"ORPHAN: 台账里的 {file} 已不存在；跑 --update 移除")

    return failures, notices


def update(root: Path, baseline_path: Path) -> int:
    """登记新文件 + 收紧；**对已有条目只降不升**（抬上限就是给变胖开绿灯）。"""
    sizes = scan(root)
    caps = load_baseline(baseline_path)
    refused: list[str] = []
    for file, recorded in sorted(caps.items()):
        new = sizes.get(file)
        if new is not None and new > recorded:
            refused.append(f"{file}: 台账 {recorded} -> 实测 {new}")
    too_big = [f"{f} {n} > {LIMIT}" for f, n in sorted(sizes.items()) if f not in caps and n > LIMIT]
    if refused or too_big:
        print("REFUSED: 台账只登记与收紧，不抬高上限。")
        for line in refused:
            print(f"  GREW {line} —— 拆掉多余的部分再 --update")
        for line in too_big:
            print(f"  NEW OVER BUDGET {line} —— 新文件先拆到预算内")
        return 1
    merged = {f: min(caps.get(f, n), n) for f, n in sizes.items()}
    save_baseline(baseline_path, merged)
    debt = sum(1 for n in merged.values() if n > LIMIT)
    print(
        "baseline updated: %d files (limit=%d, over-budget debt=%d)"
        % (len(merged), LIMIT, debt)
    )
    return 0


SELFTEST_SMALL = "crates/demo/src/small.rs"
SELFTEST_GREEN = "crates/demo/src/ok.rs"
SELFTEST_OVER = "crates/demo/src/big.rs"


def _write(path: Path, lines: int) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    body = "\n".join(f"// line {i}" for i in range(lines))
    path.write_text(body + "\n", encoding="utf-8")


def self_test() -> int:
    """每条规则都要有一个真的会红的反证；绿例必须真跑。"""
    script = str(Path(__file__).resolve())
    failures: list[str] = []
    seen = [0]

    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        baseline = root / "baseline.json"

        def run(*args: str) -> subprocess.CompletedProcess[str]:
            return subprocess.run(
                [sys.executable, script, "--root", str(root), "--baseline", str(baseline), *args],
                stdout=subprocess.PIPE,
                stderr=subprocess.STDOUT,
                text=True,
                encoding="utf-8",
                errors="replace",
            )

        def expect(name: str, result: subprocess.CompletedProcess[str], ok: bool, needle: str = "") -> None:
            seen[0] += 1
            good = (result.returncode == 0) == ok and (ok or needle in result.stdout)
            print(("PASS " if good else "FAIL ") + name)
            if not good:
                print(result.stdout[-800:])
                failures.append(name)

        # 0. 空台账 + 一个文件 = 红（默认就要登记，没有"未登记即自由"的缝）
        _write(root / SELFTEST_SMALL, 10)
        expect("red: a plain small file is not free", run(), False, "UNCAPPED")

        # 1. 登记之后，正好在上限 = 绿
        expect("update registers it", run("--update"), True)
        expect("green: at cap", run(), True)

        # 2. 反证：远不到 800 的文件加一行就红 —— 这就是"默认有门禁"
        _write(root / SELFTEST_SMALL, 11)
        expect("red: grew one line, still small", run(), False, "GREW")

        # 3. 反证：--update 不肯替变胖的文件抬上限
        expect("update refuses to raise cap", run("--update"), False, "REFUSED")

        # 4. 反证：新增一个没登记的文件 = 红；登记后 = 绿
        _write(root / SELFTEST_SMALL, 10)
        _write(root / SELFTEST_GREEN, 20)
        expect("red: new file uncapped", run(), False, "UNCAPPED")
        expect("update records new file", run("--update"), True)
        expect("green: after registering new file", run(), True)

        # 5. 反证：新建一个超预算的文件，登记直接被拒
        _write(root / SELFTEST_OVER, LIMIT + 1)
        expect("update refuses new over-budget file", run("--update"), False, "NEW OVER BUDGET")
        expect("red: uncapped over budget names OVER", run(), False, "OVER:")

        # 6. 正例：预算内文件即使很大也绿（800 是硬线，不是"越小越好"的假门）
        _write(root / SELFTEST_OVER, LIMIT)
        expect("update records at-limit file", run("--update"), True)
        expect("green: exactly at limit", run(), True)

        # 7. 提醒不算红：变瘦只提示收紧（否则删一行注释都要跑门）
        _write(root / SELFTEST_SMALL, 7)
        expect("shrink is a notice, not a failure", run(), True, "STALE")

        # 8. 提醒不算红：台账里留了已删除的文件
        (root / SELFTEST_GREEN).unlink()
        expect("orphan is a notice, not a failure", run(), True, "ORPHAN")
        expect("update drops the orphan", run("--update"), True)
        expect("green: ledger clean again", run(), True)

    print("self-test: %d/%d passed" % (seen[0] - len(failures), seen[0]))
    return 1 if failures else 0


def main(argv: list[str]) -> int:
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(encoding="utf-8", errors="replace")

    root = ROOT
    baseline = BASELINE
    mode = "check"
    rest = argv[1:]
    while rest:
        arg = rest.pop(0)
        if arg == "--update":
            mode = "update"
        elif arg == "--self-test":
            mode = "self-test"
        elif arg == "--root":
            root = Path(rest.pop(0))
        elif arg == "--baseline":
            baseline = Path(rest.pop(0))
        else:
            print(f"unknown argument: {arg}")
            return 2

    if mode == "self-test":
        return self_test()
    if mode == "update":
        return update(root, baseline)

    failures, notices = check(root, baseline)
    sizes = scan(root)
    caps = load_baseline(baseline)
    debt = {f: n for f, n in caps.items() if n > LIMIT}
    print(
        "module budget: limit=%d files=%d capped=%d debt=%d"
        % (LIMIT, len(sizes), len(caps), len(debt))
    )
    for file, lines in sorted(debt.items()):
        print(f"  DEBT {file} 上限 {lines}（> {LIMIT}，只能变短）")
    for notice in notices:
        print("NOTE " + notice)
    for failure in failures:
        print("FAIL " + failure)
    if failures:
        return 1
    print("OK 每个 .rs 都在自己的上限内（默认棘轮，全仓依赖面）")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))

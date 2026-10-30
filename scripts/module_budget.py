#!/usr/bin/env python3
"""模块预算门：任何一个 .rs 文件超过行数预算都必须登记在案，且台账只允许变短。

判定只有一条：文件行数 vs 预算 / 台账。
- 超预算且不在台账里 -> 红（新上帝对象）
- 在台账里但实测行数 != 台账值 -> 红（变胖，或已变瘦却忘了收紧台账）
- 台账里的文件消失 -> 红（台账过期）

用法：
    python scripts/module_budget.py            # 检查
    python scripts/module_budget.py --update   # 收紧台账（只允许变小/删除）
    python scripts/module_budget.py --self-test  # 正例 + 逐项反证
"""

from __future__ import annotations

import json
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
BASELINE = ROOT / "scripts" / "module_budget_baseline.json"

# 单个源文件的行数预算。超过就必须拆，或登记进台账（台账只减不增）。
LIMIT = 800


# 主 workspace 与独立 workspace（`t3/`）都要扫：只扫 crates/ 会让
# 「挪到扫描范围外」变成绕过 800 行的捷径。
SOURCES = ("crates/*/**/*.rs", "t3/crates/*/**/*.rs")


def scan(root: Path) -> dict[str, int]:
    sizes: dict[str, int] = {}
    for pattern in SOURCES:
        for path in sorted(root.glob(pattern)):
            rel = path.relative_to(root).as_posix()
            if "/target/" in rel:
                continue
            with path.open(encoding="utf-8") as handle:
                sizes[rel] = len(handle.read().splitlines())
    return sizes


def load_baseline(path: Path) -> dict[str, int]:
    if not path.exists():
        return {}
    with path.open(encoding="utf-8") as handle:
        data = json.load(handle)
    return {entry["file"]: int(entry["lines"]) for entry in data["over_budget"]}


def save_baseline(path: Path, sizes: dict[str, int]) -> None:
    over = {f: n for f, n in sizes.items() if n > LIMIT}
    payload = {
        "limit": LIMIT,
        "note": "只允许变短：拆掉一个文件后跑 --update 把它移除；--update 拒绝任何增长。",
        "over_budget": [
            {"file": f, "lines": n} for f, n in sorted(over.items())
        ],
    }
    with path.open("w", encoding="utf-8", newline="\n") as handle:
        json.dump(payload, handle, ensure_ascii=False, indent=2)
        handle.write("\n")


def check(root: Path, baseline_path: Path) -> list[str]:
    sizes = scan(root)
    baseline = load_baseline(baseline_path)
    failures: list[str] = []

    for file, lines in sorted(sizes.items()):
        if lines > LIMIT and file not in baseline:
            failures.append(f"NEW OVER BUDGET: {file} {lines} > {LIMIT}")

    for file, recorded in sorted(baseline.items()):
        if file not in sizes:
            failures.append(f"ORPHAN BASELINE: {file} 已不存在，跑 --update 移除")
            continue
        lines = sizes[file]
        if lines > recorded:
            failures.append(f"GREW: {file} {lines} > 台账 {recorded}")
        elif lines <= LIMIT:
            failures.append(
                f"STALE BASELINE: {file} 已降到 {lines}（预算内），台账仍写 {recorded}；跑 --update 移除"
            )
        elif lines < recorded:
            failures.append(
                f"STALE BASELINE: {file} 已降到 {lines}，台账仍写 {recorded}；跑 --update 收紧"
            )

    return failures


def update(root: Path, baseline_path: Path) -> int:
    sizes = scan(root)
    baseline = load_baseline(baseline_path)
    over = {f: n for f, n in sizes.items() if n > LIMIT}
    grew = [
        f"{f}: {old} -> {new}"
        for f, old in baseline.items()
        for new in [over.get(f)]
        if new is not None and new > old
    ]
    if grew:
        print("REFUSED: 台账只允许变短，下列文件变胖了，先拆：")
        for line in grew:
            print(f"  {line}")
        return 1
    save_baseline(baseline_path, sizes)
    print(f"baseline updated: {len(over)} entries, limit={LIMIT}")
    for file, lines in sorted(over.items()):
        print(f"  {file} {lines}")
    return 0


SELFTEST_GREEN = "crates/demo/src/small.rs"
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

        # 1. 正例：预算内的文件没有台账也是绿的
        _write(root / SELFTEST_GREEN, LIMIT)
        expect("green: at limit, no baseline", run(), True)

        # 2. 反证：超一行就是红
        _write(root / SELFTEST_OVER, LIMIT + 1)
        expect("red: one line over", run(), False, "NEW OVER BUDGET")

        # 3. 反证：把它登记进台账才绿
        expect("update records it", run("--update"), True)
        expect("green: recorded debt", run(), True)

        # 4. 反证：台账里的文件变胖 -> 红
        _write(root / SELFTEST_OVER, LIMIT + 2)
        expect("red: baselined file grew", run(), False, "GREW")

        # 5. 反证：--update 拒绝增长
        expect("update refuses growth", run("--update"), False, "REFUSED")

        # 6. 反证：拆到预算内但没收紧台账 -> 红
        _write(root / SELFTEST_OVER, LIMIT)
        expect("red: stale baseline", run(), False, "STALE BASELINE")
        expect("update accepts shrink", run("--update"), True)
        expect("green: debt cleared", run(), True)

        # 7. 反证：台账里留了已删除的文件 -> 红
        _write(root / SELFTEST_OVER, LIMIT + 1)
        run("--update")
        (root / SELFTEST_OVER).unlink()
        expect("red: orphan baseline", run(), False, "ORPHAN BASELINE")

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

    failures = check(root, baseline)
    sizes = scan(root)
    over = sum(1 for lines in sizes.values() if lines > LIMIT)
    print(
        "module budget: limit=%d files=%d over=%d baseline=%d"
        % (LIMIT, len(sizes), over, len(load_baseline(baseline)))
    )
    for failure in failures:
        print("FAIL " + failure)
    if failures:
        return 1
    print("OK every file within budget or registered as shrinking debt")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))

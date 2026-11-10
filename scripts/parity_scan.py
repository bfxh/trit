#!/usr/bin/env python3
"""二进制 / 三进制配比扫描：仓库里到底多少是"两值位运算"，多少是"三值状态"。

用法：
  python scripts/parity_scan.py                 # 人读表
  python scripts/parity_scan.py --json          # 机器读
  python scripts/parity_scan.py --crate trit-ops
  python scripts/parity_scan.py --self-test

口径（先说清楚，这是一把**代理尺**，不是语义分析）：

- 按**行**分类，再按行字节数加权；一行可能同时命中两类，就各记一次（所以两列之和**
  可能超过总量**，总量按"归属任一类"计）。
- **三值域**：`Ternary` / `Unknown` / `K3` / `Ł3` / `Belnap` / `trit` / `Trit` / `TriBit`
  / `TriVec` / `tri_` / `three_valued` / `-1|0|+1` 形状的状态机语义标识符。
- **二值域**：位运算（`&` `|` `^` `<<` `>>`）、`count_ones` / `popcount`、`mask`、
  双导轨（`a_pos` / `a_neg` / `rail`）、`u64`/`u32`/`u8` 的位级用法、`bool`。
- 剩下的算**中性**（胶水、测试脚手架、文档字符串）。

为什么要这把尺：用户要的不是"我们有三进制"这句口号，而是**占比**——
哪些 crate 是真三值，哪些其实只是二进制位运算套了个三值的名字。
量出来之后，"把二进制的好处（逐位可验证）"与"三值的好处（未知/冲突可表达）"
各自用在哪，才有据可依，而不是按作者口味。
"""

from __future__ import annotations

import argparse
import json
import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parents[1]

TERNARY = re.compile(
    r"(?:\bTernary\b|\bUnknown\b|\bK3\b|\bŁ3\b|\bBelnap\b|\b[tT]rit(?:s|Bit|Vec|Value)?\b|"
    r"\btri_[a-z]|\bthree_valued\b|\bvalued_three\b)"
)
BINARY = re.compile(
    r"(?:\.count_ones\(\)|\bpopcount\b|\bmask\b|\brail\b|\ba_pos\b|\ba_neg\b|"
    r"\bap\b\s*&|\ban\b\s*&|[&|^]{1,2}\s*0x[0-9a-fA-F]+|<<|>>|\bbool\b)"
)

# 不参与统计：生成物、测试夹具、外部拷贝
SKIP_DIRS = {".git", "target", "node_modules", "__pycache__"}


def classify_line(line: str) -> tuple[bool, bool]:
    """返回 (命中三值, 命中二值)。"""
    return bool(TERNARY.search(line)), bool(BINARY.search(line))


def scan_file(path: pathlib.Path) -> dict:
    ternary_bytes = binary_bytes = both_bytes = neutral_bytes = 0
    ternary_lines = binary_lines = 0
    total = 0
    text = path.read_text(encoding="utf-8", errors="replace")
    for line in text.splitlines(keepends=True):
        size = len(line.encode("utf-8"))
        total += size
        hit_t, hit_b = classify_line(line)
        if hit_t:
            ternary_lines += 1
            ternary_bytes += size
        if hit_b:
            binary_lines += 1
            binary_bytes += size
        if hit_t and hit_b:
            both_bytes += size
        if not hit_t and not hit_b:
            neutral_bytes += size
    return {
        "bytes": total,
        "ternary_bytes": ternary_bytes,
        "binary_bytes": binary_bytes,
        "both_bytes": both_bytes,
        "neutral_bytes": neutral_bytes,
        "ternary_lines": ternary_lines,
        "binary_lines": binary_lines,
    }


def merge(into: dict, item: dict) -> None:
    for key, value in item.items():
        into[key] = into.get(key, 0) + value


def scan_crate(crate_dir: pathlib.Path) -> dict:
    """一个 crate 的 src/ 与 tests/ 分开记（测试脚手架不该抬高实现占比）。"""
    src = {k: 0 for k in ("bytes", "ternary_bytes", "binary_bytes", "both_bytes",
                          "neutral_bytes", "ternary_lines", "binary_lines")}
    tests = dict(src)
    files = 0
    for sub, bucket in (("src", src), ("tests", tests)):
        directory = crate_dir / sub
        if not directory.is_dir():
            continue
        for path in sorted(directory.rglob("*.rs")):
            if any(part in SKIP_DIRS for part in path.parts):
                continue
            merge(bucket, scan_file(path))
            files += 1
    return {"src": src, "tests": tests, "files": files}


def ratio(part: int, whole: int) -> float:
    return 0.0 if whole == 0 else part / whole


def render_row(name: str, row: dict) -> str:
    src = row["src"]
    total = src["bytes"]
    return (
        f"  {name:<16} {total / 1024:>8.1f} KiB │ "
        f"三值 {ratio(src['ternary_bytes'], total) * 100:>5.1f}% │ "
        f"二值 {ratio(src['binary_bytes'], total) * 100:>5.1f}% │ "
        f"两栏都中 {ratio(src['both_bytes'], total) * 100:>5.1f}% │ "
        f"中性 {ratio(src['neutral_bytes'], total) * 100:>5.1f}%"
    )


def self_test() -> int:
    """夹具驱动的分类测试：每条断言都钉住一类行的归类。"""
    import tempfile

    checks: list[tuple[str, bool]] = []

    def check(label: str, got, want) -> None:
        checks.append((label, got == want))

    check("三值标识符", classify_line("let v = Ternary::Unknown;"), (True, False))
    check("Belnap / K3 也算", classify_line("// Belnap 与 K3 的差别"), (True, False))
    check("裸 trit 不算（避免误伤 trit-gpu 这种 crate 名）",
          classify_line("use trit_gpu::device;"), (False, False))
    check("位运算算二值", classify_line("let x = (ap & bp).count_ones();"), (False, True))
    check("双导轨变量名算二值", classify_line("let s = a_pos[i] ^ a_neg[i];"), (False, True))
    check("一行两栏都命中", classify_line("let t = Ternary::Unknown & 0x1f;"), (True, True))
    check("普通代码中性", classify_line("let total = items.len();"), (False, False))

    with tempfile.TemporaryDirectory() as temp:
        root = pathlib.Path(temp) / "crate"
        (root / "src").mkdir(parents=True)
        (root / "tests").mkdir()
        (root / "src/lib.rs").write_text(
            "pub enum T { A }\nlet x = Ternary::Unknown;\nlet y = ap & an;\nlet z = 1 + 2;\n",
            encoding="utf-8",
        )
        (root / "tests/a.rs").write_text("let q = Ternary::True;\n", encoding="utf-8")
        row = scan_crate(root)
        check("src 与 tests 分开记", row["tests"]["bytes"] > 0 and
              row["tests"]["ternary_bytes"] > 0, True)
        check("src 里两栏都有命中",
              row["src"]["ternary_bytes"] > 0 and row["src"]["binary_bytes"] > 0, True)
        check("中性行不被算进两栏",
              row["src"]["neutral_bytes"] > 0, True)
        check("两栏同时命中的行单列计数",
              row["src"]["both_bytes"] == 0, True)
        empty = scan_crate(pathlib.Path(temp) / "nope")
        check("空目录不炸", empty["src"]["bytes"], 0)

    failed = [label for label, ok in checks if not ok]
    for label, ok in checks:
        print(f"  {'FAILED' if not ok else 'ok    '} {label}")
    print(f"self-test: {len(checks) - len(failed)}/{len(checks)} passed")
    return 1 if failed else 0


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--json", action="store_true")
    parser.add_argument("--self-test", action="store_true")
    parser.add_argument("--crate", default="")
    parser.add_argument("--root", default=str(ROOT))
    args = parser.parse_args(argv)
    if args.self_test:
        return self_test()

    root = pathlib.Path(args.root)
    crates = sorted(p for p in (root / "crates").iterdir() if p.is_dir())
    if args.crate:
        crates = [p for p in crates if p.name == args.crate]
        if not crates:
            print(f"FAIL 没有这个 crate：{args.crate}", file=sys.stderr)
            return 2

    rows = {crate.name: scan_crate(crate) for crate in crates}
    total = {k: 0 for k in ("bytes", "ternary_bytes", "binary_bytes", "both_bytes",
                            "neutral_bytes", "ternary_lines", "binary_lines")}
    for row in rows.values():
        merge(total, row["src"])

    if args.json:
        print(json.dumps({"tool": "parity-scan", "crates": rows, "total": total},
                         ensure_ascii=False, indent=2))
        return 0

    print(f"扫 {len(rows)} 个 crate 的 src/（按行字节加权；两栏可同时命中，故百分比之和可能 >100%）")
    for name, row in rows.items():
        if row["src"]["bytes"] == 0:
            continue
        print(render_row(name, row))
    print(render_row("—— 合计 ——", {"src": total}))
    print(
        f"\n三值行 {total['ternary_lines']}｜二值行 {total['binary_lines']}｜"
        f"三值/二值字节比 {ratio(total['ternary_bytes'], max(total['binary_bytes'], 1)):.2f}"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))

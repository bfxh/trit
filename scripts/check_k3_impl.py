#!/usr/bin/env python3
"""K3 实现同步门：`verus/src/k3_impl.rs` 的函数体必须与主库 `value.rs` 一致。

主库 `crates/trit-core/src/value.rs` 用 `// BEGIN K3 OPS` / `// END K3 OPS`
标出 K3 算子的实现；`verus/src/k3_impl.rs` 用同一组标记复制这些实现体。
本脚本把两边函数体规范化（`Self::*` / `self` / `other` 换成 Verus 侧名字）后逐字比较。

用法：
    python scripts/check_k3_impl.py
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SOURCE = ROOT / "crates/trit-core/src/value.rs"
VERUS = ROOT / "verus/src/k3_impl.rs"
BEGIN_MARK = "// BEGIN K3 OPS"
END_MARK = "// END K3 OPS"

# 主库函数名 -> verus 侧函数名
FUNCTIONS = {
    "not": "not_impl",
    "bitand": "bitand_impl",
    "bitor": "bitor_impl",
    "bitxor": "bitxor_impl",
}

FUNCTION = re.compile(r"fn\s+(\w+)\s*[^{]*\{")


def marked_block(text: str, path: Path) -> str:
    # 用 rfind：文件头文档可能引用标记；实际标记在文档之后。
    start = text.rfind(BEGIN_MARK)
    end = text.rfind(END_MARK)
    if start < 0 or end <= start:
        raise SystemExit(f"check_k3_impl: {path} 缺少 BEGIN/END K3 OPS 标记")
    return text[start + len(BEGIN_MARK) : end]


def function_bodies(block: str) -> dict[str, str]:
    bodies: dict[str, str] = {}
    for match in FUNCTION.finditer(block):
        name = match.group(1)
        if name not in ("not", "bitand", "bitor", "bitxor") and not name.endswith("_impl"):
            continue
        start = block.find("{", match.start())
        depth = 0
        for index in range(start, len(block)):
            char = block[index]
            if char == "{":
                depth += 1
            elif char == "}":
                depth -= 1
                if depth == 0:
                    bodies[name] = block[start + 1 : index]
                    break
    return bodies


def normalize(body: str) -> str:
    body = body.replace("Self::False", "Trit::F")
    body = body.replace("Self::True", "Trit::T")
    body = body.replace("Self::Unknown", "Trit::U")
    body = re.sub(r"\bself\b", "a", body)
    body = re.sub(r"\bother\b", "b", body)
    return re.sub(r"\s+", "", body)


def main() -> int:
    source = function_bodies(marked_block(SOURCE.read_text(encoding="utf-8"), SOURCE))
    verus = function_bodies(marked_block(VERUS.read_text(encoding="utf-8"), VERUS))
    problems: list[str] = []
    for source_name, verus_name in FUNCTIONS.items():
        if source_name not in source:
            problems.append(f"主库 value.rs 缺少 fn {source_name}")
            continue
        if verus_name not in verus:
            problems.append(f"verus/src/k3_impl.rs 缺少 fn {verus_name}")
            continue
        left = normalize(source[source_name])
        right = normalize(verus[verus_name])
        if left != right:
            problems.append(
                f"{source_name} 与 {verus_name} 不一致：\n  主库: {left}\n  verus: {right}"
            )
    if problems:
        print("K3 实现同步门：FAIL")
        for problem in problems:
            print("  " + problem)
        return 1
    print(f"K3 实现同步门：OK（{len(FUNCTIONS)} 个函数体与主库一致）")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

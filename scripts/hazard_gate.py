#!/usr/bin/env python3
"""高危门：并发 / 内存面的编译期静态门禁，比 rustc + clippy 更狠、且按设计分级。

设计原则（三档，见 docs/HAZARD-POLICY.md）：
- **放行**：提高能力上限、且有成文防护的高危原语（如 SIMD GEMV 的裸指针、
  `unsafe impl Send/Sync` 的池指针）——登记在 ALLOW 表里，每条带设计理由；
- **有条件**：允许使用，但必须伴随成文不变量（`# Safety` / SAFETY 注释）；
- **禁止**：不带来上限收益的高危（`static mut`、`transmute`）——零放行条目。

比 rustc / clippy 狠在哪：rustc 保证内存安全的地板，本门把**仓库的设计决策**
钉成清单——清单外的每个高危匹配都是红，清单内每条都要有「为什么值得」。

边界（不猜）：这是源码级文本门（先剥注释/字符串/char 字面量再匹配），不是类型
系统；它补充 rustc/clippy/trit-unsafe，不替代。匹配不到的东西不等于不存在。
产品代码（crates/*/src）是门；`#[cfg(test)]` 与 tests/ 里的演练只计数不拦截。

用法：
    python scripts/hazard_gate.py              # 检查（默认严格）
    python scripts/hazard_gate.py --json       # JSON 报告
    python scripts/hazard_gate.py --self-test  # 正例 + 逐项反证
"""

from __future__ import annotations

import json
import re
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]

# ---------------------------------------------------------------------------
# 放行清单：文件 → 设计理由。新增高危用法必须在这里登记理由，否则红。
# ---------------------------------------------------------------------------

ALLOW_RAW_MEMORY: dict[str, str] = {
    "crates/trit-ops/src/gemv.rs": (
        "SIMD GEMV 内核（15.8× f32 的上限来源）：from_raw_parts 的长度不变量"
        "由分配处的 padded 布局保证，指针以行为单位推进，SAFETY 注释在每个"
        "unsafe 块上方"
    ),
    "crates/trit-ops/src/simd.rs": (
        "AVX 行点积内核：target_feature + 显式长度参数，调用方"
        "is_x86_feature_detected 运行时探测"
    ),
    "crates/trit-ops/src/bench.rs": (
        "SIMD 内核的对照基准：复用与内核相同的裸指针内存布局，保证测的是"
        "同一条路径（不含用户可触达的 API）"
    ),
    "crates/trit-dot/src/lib.rs": (
        "POPCNT 位并行点积：target_feature 函数 + 调用方运行时探测"
    ),
}

ALLOW_SEND_SYNC: dict[str, str] = {
    "crates/trit-ops/src/pool.rs": (
        "Ptr<T> 裸指针包装：只在 par_rows_mut 内按行切分使用，无重叠写，"
        "Pool::run 返回前全部 worker join（完整屏障）——并行 GEMV 的上限来源"
    ),
}

ALLOW_TARGET_FEATURE: dict[str, str] = {
    "crates/trit-ops/src/simd.rs": "AVX 内核（见 raw-memory 条目）",
    "crates/trit-ops/src/gemv.rs": "AVX/FMA 内核（见 raw-memory 条目）",
    "crates/trit-dot/src/lib.rs": "POPCNT 内核（见 raw-memory 条目）",
}

ALLOW_RELAXED: dict[str, str] = {
    "crates/trit-ops/src/pool.rs": (
        "线程数配置与完成计数：单变量、无同步依赖，Relaxed 是教科书正确选择"
    ),
}

# ---------------------------------------------------------------------------

RULES: list[dict] = [
    {
        "id": "static-mut",
        "area": "memory",
        "verdict": "禁止",
        "why": "全局可变状态是数据竞争的种子；本仓设计里没有任何场景需要它（线程局部用 OnceLock<Arc<Pool>>）",
        "pattern": r"\bstatic\s+mut\s+\w",
        "allow": {},
    },
    {
        "id": "transmute",
        "area": "memory",
        "verdict": "禁止",
        "why": "类型双关在本仓设计里没有位置：三值走双导轨/查表，跨类型用显式转换",
        "pattern": r"\btransmute\b",
        "allow": {},
    },
    {
        "id": "raw-memory-api",
        "area": "memory",
        "verdict": "有条件",
        "why": "from_raw_parts/set_len/assume_init/forget 只允许出现在登记过的内核模块，且现场带不变量",
        "pattern": r"\bfrom_raw_parts_mut\b|\bfrom_raw_parts\b|\bset_len\b|\bassume_init\b|\bmem::forget\b|\bBox::from_raw\b",
        "allow": ALLOW_RAW_MEMORY,
        "safety_required": True,
    },
    {
        "id": "unsafe-send-sync",
        "area": "concurrency",
        "verdict": "有条件",
        "why": "unsafe impl Send/Sync 是对编译器的手工承诺：rustc 只要求 unsafe，本门还要求成文不变量与登记",
        "pattern": r"unsafe\s+impl\b[^;{]*\b(Send|Sync)\b",
        "allow": ALLOW_SEND_SYNC,
        "safety_required": True,
    },
    {
        "id": "target-feature",
        "area": "concurrency",
        "verdict": "有条件",
        "why": "目标特性函数在无探测的机器上是非法指令：登记 + 调用方必须运行时探测",
        "pattern": r"#\[target_feature",
        "allow": ALLOW_TARGET_FEATURE,
    },
    {
        "id": "relaxed-ordering",
        "area": "concurrency",
        "verdict": "有条件",
        "why": "Relaxed 只允许用于计数器/配置读取；用于同步就是数据竞争——登记过的文件外一律红",
        "pattern": r"Ordering::Relaxed",
        "allow": ALLOW_RELAXED,
    },
]

# SAFETY 注释（`// SAFETY:` / `# Safety` 文档头）检索窗口：匹配行向上 8 行，
# 或向上到最近的 `unsafe fn` 签名再向上 8 行（fn 级文档覆盖整个函数体的约定）。
SAFETY_RE = re.compile(r"#\s*Safety|SAFETY\s*[::]|Safety\s*[::]")
SAFETY_WINDOW = 8


def scrub(text: str) -> str:
    """剥掉注释 / 字符串 / char 字面量 / 原始字符串（保留行号与长度）。

    必须能扛住 char 字面量里的引号（`'"'`、`'\\''`）——否则状态机错位，
    文档示例与检测 needle 会「复活」成匹配（自审里 binary.rs 的教训）。
    """
    out: list[str] = []
    state = "code"
    raw_hashes = 0
    for line in text.splitlines(keepends=True):
        cleaned: list[str] = []
        i = 0
        n = len(line)
        while i < n:
            ch = line[i]
            two = line[i : i + 2]
            if state == "code":
                if two == "//":
                    state = "line"
                    cleaned.append("  ")
                    i += 2
                elif two == "/*":
                    state = "block"
                    cleaned.append("  ")
                    i += 2
                elif ch == '"':
                    state = "str"
                    cleaned.append(" ")
                    i += 1
                elif ch == "'":
                    # char 字面量 vs 生命周期：同行 4 字符内找收尾引号。
                    closing = line.find("'", i + 1, min(n, i + 5))
                    if closing != -1 and line[i + 1 : closing] not in ("", "\\"):
                        cleaned.append(" " * (closing + 1 - i))
                        i = closing + 1
                    else:
                        cleaned.append(ch)
                        i += 1
                elif ch == "r" and i + 1 < n and line[i + 1] in "\"#":
                    hashes = len(line[i + 1 :]) - len(line[i + 1 :].lstrip("#"))
                    if line[i + 1 + hashes : i + 2 + hashes] == '"':
                        state = "raw"
                        raw_hashes = hashes
                        cleaned.append(" " * (2 + hashes))
                        i += 2 + hashes
                    else:
                        cleaned.append(ch)
                        i += 1
                else:
                    cleaned.append(ch)
                    i += 1
            elif state == "line":
                cleaned.append("\n" if ch == "\n" else " ")
                if ch == "\n":
                    state = "code"
                i += 1
            elif state == "block":
                if two == "*/":
                    cleaned.append("  ")
                    i += 2
                    state = "code"
                else:
                    cleaned.append("\n" if ch == "\n" else " ")
                    i += 1
            elif state == "str":
                if ch == "\\" and i + 1 < n:
                    cleaned.append("  ")
                    i += 2
                elif ch == '"':
                    state = "code"
                    cleaned.append(" ")
                    i += 1
                else:
                    cleaned.append("\n" if ch == "\n" else " ")
                    i += 1
            elif state == "raw":
                closing = '"' + "#" * raw_hashes
                if line[i : i + len(closing)] == closing:
                    cleaned.append(" " * len(closing))
                    i += len(closing)
                    state = "code"
                else:
                    cleaned.append("\n" if ch == "\n" else " ")
                    i += 1
        out.append("".join(cleaned))
    return "".join(out)


def safety_documented(original_lines: list[str], index: int) -> bool:
    """SAFETY 是否成文：匹配点向上 8 行内；或向上走到最近的 unsafe 边界
    （`unsafe {` / `unsafe fn` / `unsafe impl`）再查其上方 8 行——真实内核里
    unsafe 块体很长，固定窗口不够；`unsafe fn` 的文档覆盖整个函数体。"""
    original_len = len(original_lines)
    if any(
        SAFETY_RE.search(original_lines[u])
        for u in range(max(0, index - SAFETY_WINDOW), min(index + 1, original_len))
    ):
        return True
    for up in range(index - 1, max(0, index - 400), -1):
        line = original_lines[up]
        if SAFETY_RE.search(line):
            return True
        if "unsafe {" in line or "unsafe fn" in line or "unsafe impl" in line:
            start = max(0, up - SAFETY_WINDOW)
            return any(
                SAFETY_RE.search(original_lines[u]) for u in range(start, up + 1)
            )
    return False


def inline_test_boundary(lines: list[str]) -> int:
    """`#[cfg(test)]` 的行号（此后属于测试演练区）；没有则返回文件末尾外。"""
    for index in range(len(lines) - 1, -1, -1):
        if "#[cfg(test)]" in lines[index]:
            return index
    return len(lines) + 1


def scan(root: Path, allow_overrides: dict[str, dict[str, str]] | None = None) -> dict:
    files = sorted(p for p in root.glob("crates/*/**/*.rs") if "/target/" not in str(p))
    findings: list[dict] = []
    allow_hits: list[dict] = []
    for path in files:
        rel = path.relative_to(root).as_posix()
        original_lines = path.read_text(encoding="utf-8").splitlines()
        joined = scrub("\n".join(original_lines) + "\n")
        cleaned = joined.splitlines()
        test_boundary = inline_test_boundary(original_lines)
        for rule in RULES:
            allow = (allow_overrides or {}).get(rule["id"]) or rule["allow"]
            allow_files = set(allow)
            for match in re.finditer(rule["pattern"], joined):
                line_no = joined.count("\n", 0, match.start())
                line_index = min(line_no, len(original_lines) - 1)
                in_tests = (
                    line_no > test_boundary
                    or "/tests/" in rel
                    or rel.endswith("tests.rs")
                )
                entry = {
                    "rule": rule["id"],
                    "area": rule["area"],
                    "file": rel,
                    "line": line_no + 1,
                    "snippet": original_lines[line_no].strip()[:100],
                    "in_tests": in_tests,
                }
                if rel in allow_files and not in_tests:
                    if rule.get("safety_required") and not safety_documented(
                        original_lines, line_index
                    ):
                        entry["hint"] = (
                            "已登记但匹配点（及其所在 unsafe fn 签名）向上没有"
                            " SAFETY/# Safety 不变量"
                        )
                        findings.append(entry)
                    else:
                        entry["justification"] = allow[rel]
                        allow_hits.append(entry)
                    continue
                if in_tests:
                    # 测试/示例里允许演练高危形态：计数不拦截（产品代码才是门）。
                    allow_hits.append(entry)
                    continue
                entry["hint"] = (
                    rule["why"] + "（需要放行请在脚本 ALLOW 表登记设计理由）"
                )
                findings.append(entry)
    return {
        "tool": "hazard-gate",
        "policy": "docs/HAZARD-POLICY.md",
        "files_scanned": len(files),
        "findings": findings,
        "allow_hits": allow_hits,
    }


def self_test() -> int:
    """正例（放行清单内带 SAFETY）+ 逐项反证（每条规则都能红）。"""
    failures: list[str] = []
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        (root / "crates" / "demo" / "src").mkdir(parents=True)
        fixtures = {
            "bad_static_mut.rs": "static mut COUNTER: u32 = 0;\n",
            "bad_transmute.rs": (
                "fn f(x: u32) -> f32 {\n"
                "    unsafe { core::mem::transmute(x) }\n"
                "}\n"
            ),
            "bad_send_sync.rs": "struct P(*mut u8);\nunsafe impl Send for P {}\n",
            "bad_relaxed.rs": (
                "use std::sync::atomic::{AtomicUsize, Ordering};\n"
                "static N: AtomicUsize = AtomicUsize::new(0);\n"
                "fn f() {\n    N.store(1, Ordering::Relaxed);\n}\n"
            ),
            "bad_raw_memory.rs": (
                "fn f(p: *const u8, n: usize) {\n"
                "    unsafe { let s = core::slice::from_raw_parts(p, n); }\n"
                "}\n"
            ),
            # needle 自指：字符串 / 文档示例 / char 字面量里的样本不得触发。
            "needle.rs": (
                'const NEEDLE: &str = "static mut transmute Ordering::Relaxed";\n'
                'const QUOTE: char = \'"\';\n'
                "static mut HIDDEN: u32 = 0;\n"
            ),
            # 内联测试：#[cfg(test)] 之后允许演练。
            "inline_tests.rs": (
                "pub fn ok() {}\n"
                "#[cfg(test)]\n"
                "mod tests {\n"
                "    static mut T: u32 = 0;\n"
                "    #[test]\n"
                "    fn t() {\n"
                "        let _ = core::mem::transmute::<u8, u8>(0u8);\n"
                "    }\n"
                "}\n"
            ),
        }
        for name, body in fixtures.items():
            (root / "crates" / "demo" / "src" / name).write_text(
                body, encoding="utf-8"
            )
        # 正例：SAFETY + 登记放行（Send impl 与 fn 级文档两种形态）。
        (root / "crates" / "demo" / "src" / "good_impl.rs").write_text(
            "struct P(*mut u8);\n"
            "// SAFETY: 单线程池内使用，join 屏障保证无并发访问。\n"
            "unsafe impl Send for P {}\n",
            encoding="utf-8",
        )
        (root / "crates" / "demo" / "src" / "good_fn.rs").write_text(
            "/// # Safety\n"
            "/// `p` 指向至少 `n` 个可读字节。\n"
            "unsafe fn f(p: *const u8, n: usize) {\n"
            "    let s = core::slice::from_raw_parts(p, n);\n"
            "    std::hint::black_box(s);\n"
            "}\n",
            encoding="utf-8",
        )
        allow = {
            "unsafe-send-sync": {"crates/demo/src/good_impl.rs": "自测：屏障不变量"},
            "raw-memory-api": {"crates/demo/src/good_fn.rs": "自测：fn 级文档"},
        }
        report = scan(root, allow)
        fired: dict[str, set[str]] = {}
        for f in report["findings"]:
            fired.setdefault(f["rule"], set()).add(f["file"])
        for rule_id, fixture in [
            ("static-mut", "bad_static_mut.rs"),
            ("transmute", "bad_transmute.rs"),
            ("unsafe-send-sync", "bad_send_sync.rs"),
            ("relaxed-ordering", "bad_relaxed.rs"),
            ("raw-memory-api", "bad_raw_memory.rs"),
        ]:
            expected = f"crates/demo/src/{fixture}"
            if expected not in fired.get(rule_id, set()):
                failures.append(f"反证失败：{rule_id} 应在 {fixture} 变红，实际 {fired.get(rule_id)}")
        # needle.rs：字符串与 char 字面量里的样本不触发，
        # 但真正的 static mut（HIDDEN）要恰好报一次、位置正确。
        needle_hits = [f for f in report["findings"] if f["file"].endswith("needle.rs")]
        if len(needle_hits) != 1 or needle_hits[0]["line"] != 3:
            failures.append(f"needle.rs 应只报第 3 行的真 static mut：{needle_hits}")
        for name in ["inline_tests.rs", "good_impl.rs", "good_fn.rs"]:
            for f in report["findings"]:
                if f["file"].endswith(name):
                    failures.append(f"误报：{name} 不该红（{f['rule']}）")
    for failure in failures:
        print(f"SELF-TEST FAIL: {failure}")
    if failures:
        return 1
    print(
        "self-test ok：5 条反证全部变红、needle/char/内联测试不自指、"
        "登记+SAFETY 两种正例放行"
    )
    return 0


def main(argv: list[str]) -> int:
    if "--self-test" in argv:
        return self_test()
    report = scan(ROOT)
    if "--json" in argv:
        print(json.dumps(report, ensure_ascii=False, indent=2))
    else:
        print(
            json.dumps(
                {
                    "tool": report["tool"],
                    "files_scanned": report["files_scanned"],
                    "findings": len(report["findings"]),
                    "allow_hits": len(report["allow_hits"]),
                    "policy": report["policy"],
                },
                ensure_ascii=False,
            )
        )
        for f in report["findings"]:
            print(
                f"[红] {f['rule']} {f['file']}:{f['line']}\n"
                f"    {f['snippet']}\n    {f.get('hint', '')}"
            )
    if report["findings"]:
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))

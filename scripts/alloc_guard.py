#!/usr/bin/env python3
"""分配/上限执法体检：把审计 U-1/U-2/U-3 那类问题变成可复算的静态门。

用法：
  python scripts/alloc_guard.py             # 体检，红了退出非零
  python scripts/alloc_guard.py --json
  python scripts/alloc_guard.py --self-test
  python scripts/alloc_guard.py --root <dir> --allow <file>

钉的是 2026-11-12 中期审计（trit-audit-report.md）暴露的三类「资源耗尽 / 上限不执法」：

  1. **死帽（dead-cap）**：名字像上限的常量（含 MAX / 以 _LIMIT/_CAP 结尾）声明了却
     在整个工作区只出现一次（就是声明本身）——定义了但没人用 = 没执法。
     U-3 的 `HARD_MAX_TOTAL_PROBES` 正是这样：64×32=2048 远超 512 却照样放行。
  2. **无界读（unbounded-read）**：`read_line` / `read_until` 不带 `take(n)` 限位——
     对端可以用一条永不结束的头行撑爆内存。U-2 的 LSP 帧头正是这样。
     （`read_to_string` / `read_to_end` 同类但多用于可信文件，降为警告。）
  3. **无界分配（infallible-alloc）**：用裸长度变量 `vec![x; len]` / `with_capacity(len)` /
     `resize(len, ..)` 直接分配，而所在函数没有 `try_reserve` fallible 兜底——
     外部 `len` 可以是 `usize::MAX`。U-1 的 sparse 解码正是这样（警告档：内部有界分配合法）。

判红分两档（与 repo-governance 同口径）：拦 = 证据不成立（死帽 / 无界流式读）；
警 = 要人看一眼（文件整体读 / 可能无界分配）。噪音判成拦截，工具第二天就被绕过。

豁免走 `scripts/alloc_guard_allow.json`（每条带 file/kind/reason）—— reviewed 例外，
不是静默忽略；「查不到」永远不是「没问题」：解析不动的文件当全是代码，宁可多报。
"""

from __future__ import annotations

import json
import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parents[1]
ALLOW = pathlib.Path(__file__).resolve().parent / "alloc_guard_allow.json"

# 上限常量声明：const NAME: <整型> = ...（名字稍后再筛是不是「帽」）。
CONST_DECL = re.compile(
    r"\b(?P<pub>pub\s+)?const\s+(?P<name>[A-Z][A-Z0-9_]*)\s*:\s*"
    r"(?:usize|u8|u16|u32|u64|u128|isize|i64)\s*="
)
READ_STREAM = re.compile(r"\.\s*(read_line|read_until)\s*\(")
READ_WHOLE = re.compile(r"\.\s*(read_to_string|read_to_end)\s*\(")
TAKE_GUARD = re.compile(r"\btake\s*\(")
TRY_RESERVE = re.compile(r"\btry_reserve(?:_exact)?\s*\(")
ALLOC_LEN = re.compile(
    r"vec!\s*\[[^\[\]]*;\s*(?P<v>[a-z_][a-z0-9_]*)\s*\]"
    r"|with_capacity\s*\(\s*(?P<c>[a-z_][a-z0-9_]*)\s*\)"
    r"|\.\s*resize\s*\(\s*(?P<r>[a-z_][a-z0-9_]*)\s*,"
)
FN_START = re.compile(r"\bfn\s+[a-z_][a-z0-9_]*")


def is_cap_name(name: str) -> bool:
    """名字像不像「上限」：含 MAX，或以 _LIMIT/_CAP/_BOUND 结尾。"""
    return "MAX" in name or name.endswith(("_LIMIT", "_CAP", "_BOUND"))


def blank_rust(text: str) -> str:
    """把注释与字符串字面量涂白（长度/换行不变），只留代码骨架。

    扫描器不能扫到自己的文档与示例：doc 注释里的 `read_line(` / `const MAX` 不算命中。
    处理行注释、嵌套块注释、普通字符串（含转义）、raw 字符串 r#"..."#。
    字符字面量/生命周期留着不动——它们不会匹配本门的多字符模式。
    """
    out = list(text)
    i, n = 0, len(text)

    def blank(a: int, b: int) -> None:
        for k in range(a, min(b, n)):
            if out[k] not in "\n\r":
                out[k] = " "

    while i < n:
        c = text[i]
        if c == "/" and i + 1 < n and text[i + 1] == "/":
            j = text.find("\n", i)
            j = n if j < 0 else j
            blank(i, j)
            i = j
            continue
        if c == "/" and i + 1 < n and text[i + 1] == "*":
            depth, j = 1, i + 2
            while j < n and depth:
                if text[j] == "/" and j + 1 < n and text[j + 1] == "*":
                    depth += 1
                    j += 2
                elif text[j] == "*" and j + 1 < n and text[j + 1] == "/":
                    depth -= 1
                    j += 2
                else:
                    j += 1
            blank(i, j)
            i = j
            continue
        if c == "r" and i + 1 < n and text[i + 1] in '"#':
            j, hashes = i + 1, 0
            while j < n and text[j] == "#":
                hashes += 1
                j += 1
            if j < n and text[j] == '"':
                j += 1
                close = '"' + "#" * hashes
                end = text.find(close, j)
                end = n if end < 0 else end + len(close)
                blank(i, end)
                i = end
                continue
        if c == '"':
            j = i + 1
            while j < n:
                if text[j] == "\\":
                    j += 2
                elif text[j] == '"':
                    j += 1
                    break
                else:
                    j += 1
            blank(i, j)
            i = j
            continue
        i += 1
    return "".join(out)


def line_of(text: str, pos: int) -> int:
    return text.count("\n", 0, pos) + 1


def rust_files(root: pathlib.Path) -> list[pathlib.Path]:
    return sorted(root.glob("crates/*/src/**/*.rs")) + sorted(root.glob("crates/*/tests/*.rs"))


def dead_cap_findings(files: list[pathlib.Path]) -> list[dict]:
    """全工作区收集帽常量声明，统计每个名字的出现次数；只出现一次（声明本身）= 死帽。"""
    blanked: dict[pathlib.Path, str] = {}
    for path in files:
        blanked[path] = blank_rust(path.read_text(encoding="utf-8", errors="replace"))
    decls: list[tuple[pathlib.Path, str, int, bool]] = []
    for path, text in blanked.items():
        for m in CONST_DECL.finditer(text):
            name = m.group("name")
            if is_cap_name(name):
                decls.append((path, name, line_of(text, m.start()), bool(m.group("pub"))))
    findings: list[dict] = []
    for path, name, line, _pub in decls:
        word = re.compile(r"\b" + re.escape(name) + r"\b")
        total = sum(len(word.findall(text)) for text in blanked.values())
        if total <= 1:
            findings.append({
                "kind": "死帽", "file": path.as_posix(), "line": line, "blocking": True,
                "detail": f"上限常量 {name} 声明了却全工作区无人引用——定义了但没执法"
                          "（审计 U-3：HARD_MAX_TOTAL_PROBES 曾是这样）",
            })
    return findings


def read_findings(path: pathlib.Path, blanked: str) -> list[dict]:
    """流式读不带 take 限位 = 拦；整体读不带限位 = 警。tests/ 里的夹具读不扫。"""
    if "tests" in path.parts:
        return []
    findings: list[dict] = []
    lines = blanked.splitlines()
    for regex, kind, blocking, why in (
        (READ_STREAM, "无界读", True, "read_line/read_until 不带 take(n)：对端可用永不结束的行撑爆内存（审计 U-2）"),
        (READ_WHOLE, "整体读无界", False, "read_to_string/read_to_end 不带 take(n)：确认输入可信或加限位"),
    ):
        for m in regex.finditer(blanked):
            ln = line_of(blanked, m.start())
            window = "\n".join(lines[max(0, ln - 3):ln])
            if TAKE_GUARD.search(window):
                continue
            findings.append({
                "kind": kind, "file": path.as_posix(), "line": ln, "blocking": blocking,
                "detail": why,
            })
    return findings


def fn_has_try_reserve(blanked: str, pos: int) -> bool:
    """从 alloc 处往上找最近的 fn 起点，看它体内有没有 try_reserve fallible 兜底。"""
    starts = [m.start() for m in FN_START.finditer(blanked[:pos])]
    if not starts:
        return False
    start = starts[-1]
    begin = blanked.find("{", start)
    if begin < 0:
        return False
    depth = 0
    for index in range(begin, len(blanked)):
        if blanked[index] == "{":
            depth += 1
        elif blanked[index] == "}":
            depth -= 1
            if depth == 0:
                return bool(TRY_RESERVE.search(blanked[begin:index]))
    return bool(TRY_RESERVE.search(blanked[begin:]))


def alloc_findings(path: pathlib.Path, blanked: str) -> list[dict]:
    """裸长度变量直接分配且所在 fn 无 try_reserve = 警（内部有界分配合法，要人看一眼）。"""
    if "tests" in path.parts:
        return []
    findings: list[dict] = []
    for m in ALLOC_LEN.finditer(blanked):
        name = m.group("v") or m.group("c") or m.group("r")
        if fn_has_try_reserve(blanked, m.start()):
            continue
        findings.append({
            "kind": "可能无界分配", "file": path.as_posix(), "line": line_of(blanked, m.start()),
            "blocking": False,
            "detail": f"用裸长度 `{name}` 直接分配且所在函数无 try_reserve："
                      "确认它来自外部时有硬顶（审计 U-1），否则改 fallible reservation",
        })
    return findings


def scan(root: pathlib.Path, allow_path: pathlib.Path) -> dict:
    root = pathlib.Path(root)
    files = rust_files(root)
    if not files:
        # 读不到任何 Rust 文件 ≠ 没问题：形状不认识就判红。
        return {"tool": "alloc-guard", "findings": [{
            "kind": "扫不到源码", "file": str(root), "blocking": True,
            "detail": "crates/*/src 下没有 .rs——路径错了或仓库形状变了，不许当绿",
        }], "blocking": 1, "warnings": 0, "red": True, "files": 0}
    allow: list[dict] = []
    if allow_path.is_file():
        allow = json.loads(allow_path.read_text(encoding="utf-8"))
    findings: list[dict] = []
    findings.extend(dead_cap_findings(files))
    for path in files:
        blanked = blank_rust(path.read_text(encoding="utf-8", errors="replace"))
        findings.extend(read_findings(path, blanked))
        findings.extend(alloc_findings(path, blanked))
    kept: list[dict] = []
    for finding in findings:
        rel = pathlib.Path(finding["file"]).relative_to(root).as_posix() if pathlib.Path(finding["file"]).is_absolute() else finding["file"]
        finding["file"] = rel
        if any(a.get("file") == rel and a.get("kind") == finding["kind"] and a.get("reason") for a in allow):
            continue
        kept.append(finding)
    return {
        "tool": "alloc-guard",
        "files": len(files),
        "findings": kept,
        "blocking": sum(1 for f in kept if f["blocking"]),
        "warnings": sum(1 for f in kept if not f["blocking"]),
        "red": any(f["blocking"] for f in kept),
    }


def self_test() -> int:
    """每条规则一个真会红的反证 + 绿例真跑；接线断言走 main()/scan()。"""
    import subprocess
    import tempfile

    script = str(pathlib.Path(__file__).resolve())
    total, failures = 0, []

    def run(root: pathlib.Path, extra: list[str]) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            [sys.executable, "-X", "utf8", script, "--root", str(root),
             "--allow", str(root / "allow.json"), *extra],
            stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True,
            encoding="utf-8", errors="replace")

    def check(name: str, actual, expected) -> None:
        nonlocal total
        total += 1
        if actual != expected:
            failures.append(name)
            print(f"FAIL {name}：期望 {expected!r}，实际 {actual!r}")

    def mk(root: pathlib.Path, rel: str, text: str) -> pathlib.Path:
        path = root / rel
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text, encoding="utf-8")
        return path

    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        # 绿例：帽被引用、读带 take、分配带 try_reserve。
        mk(root, "crates/demo/src/lib.rs",
           "pub const MAX_LEN: usize = 1 << 28;\n"
           "fn ok(len: usize) -> bool { len <= MAX_LEN }\n"
           "fn read<R: std::io::BufRead>(r: &mut R) { let mut s = String::new();\n"
           "    std::io::Read::take(r, 8).read_until(b'\\n', &mut Vec::new()).unwrap(); }\n"
           "fn alloc(len: usize) { let mut v: Vec<u8> = Vec::new();\n"
           "    v.try_reserve_exact(len).unwrap(); v.resize(len, 0); }\n")
        check("干净夹具判绿", run(root, []).returncode, 0)

        # 反证 1：死帽（声明了没人用）。
        mk(root, "crates/demo/src/lib.rs",
           "pub const HARD_MAX_TOTAL: usize = 512;\nfn unrelated() {}\n")
        out = run(root, []).stdout
        check("死帽被抓", "死帽" in out, True)
        check("死帽判红", run(root, []).returncode, 1)
        # 帽被引用后放行。
        mk(root, "crates/demo/src/lib.rs",
           "pub const HARD_MAX_TOTAL: usize = 512;\nfn ok(n: usize) -> bool { n <= HARD_MAX_TOTAL }\n")
        check("帽被引用即放行", "死帽" in run(root, []).stdout, False)

        # 反证 2：无界流式读（read_line 不带 take）。
        mk(root, "crates/demo/src/lib.rs",
           "fn read<R: std::io::BufRead>(r: &mut R) { let mut s = String::new(); r.read_line(&mut s).unwrap(); }\n")
        out = run(root, []).stdout
        check("无界读被抓", "无界读" in out, True)
        check("无界读判红", run(root, []).returncode, 1)
        # 带 take 限位放行。
        mk(root, "crates/demo/src/lib.rs",
           "fn read<R: std::io::BufRead>(r: &mut R) { let mut s = String::new();\n"
           "    std::io::Read::take(r, 8).read_line(&mut s).unwrap(); }\n")
        check("take 限位的读放行", "无界读" in run(root, []).stdout, False)

        # 反证 3：整体读只警告不拦。
        mk(root, "crates/demo/src/lib.rs",
           "fn read<R: std::io::Read>(r: &mut R) { let mut s = String::new(); r.read_to_string(&mut s).unwrap(); }\n")
        out = run(root, []).stdout
        check("整体读给警告", "整体读无界" in out, True)
        check("警告不判红", run(root, []).returncode, 0)

        # 反证 4：无 try_reserve 的裸长度分配给警告；有 try_reserve 放行。
        mk(root, "crates/demo/src/lib.rs",
           "fn alloc(len: usize) -> Vec<u8> { vec![0u8; len] }\n")
        check("裸长度分配给警告", "可能无界分配" in run(root, []).stdout, True)
        mk(root, "crates/demo/src/lib.rs",
           "fn alloc(len: usize) -> Vec<u8> { let mut v = Vec::new();\n"
           "    v.try_reserve_exact(len).unwrap(); v.resize(len, 0); v }\n")
        check("try_reserve 兜底放行", "可能无界分配" in run(root, []).stdout, False)

        # 反证 5：doc 注释/字符串里的示例不算命中（扫描器不许扫到自己的文档）。
        mk(root, "crates/demo/src/lib.rs",
           "//! 示例：const MAX_X: usize = 1; 与 r.read_line(&mut s) 只是说明。\n"
           "fn doc() { let _s = \"read_line( vec![0; len]\"; }\n")
        check("注释/字符串里的示例不误报", run(root, []).returncode, 0)

        # 反证 6：tests/ 里的夹具读/分配不扫（只产品代码）。
        mk(root, "crates/demo/src/lib.rs", "fn ok() {}\n")
        mk(root, "crates/demo/tests/t.rs",
           "fn read<R: std::io::BufRead>(r: &mut R) { let mut s = String::new(); r.read_line(&mut s).unwrap(); }\n")
        check("tests 夹具读不扫", "无界读" in run(root, []).stdout, False)

        # 反证 7：豁免名单带理由才生效。
        mk(root, "crates/demo/src/lib.rs",
           "pub const HARD_MAX_TOTAL: usize = 512;\nfn unrelated() {}\n")
        (root / "allow.json").write_text(json.dumps(
            [{"file": "crates/demo/src/lib.rs", "kind": "死帽", "reason": "下游 API，本仓不引用"}],
            ensure_ascii=False), encoding="utf-8")
        check("带理由豁免放行", run(root, []).returncode, 0)
        (root / "allow.json").write_text(json.dumps(
            [{"file": "crates/demo/src/lib.rs", "kind": "死帽"}], ensure_ascii=False), encoding="utf-8")
        check("无理由豁免不生效", run(root, []).returncode, 1)

        # 反证 8：扫不到源码判红（读不到 ≠ 没问题）。
        empty = pathlib.Path(tmp) / "empty"
        empty.mkdir()
        check("扫不到源码判红", run(empty, []).returncode, 1)

    print(f"self-test: {total - len(failures)}/{total} passed")
    return 1 if failures else 0


def main(argv: list[str]) -> int:
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    if "--self-test" in argv:
        return self_test()
    root, allow = ROOT, ALLOW
    if "--root" in argv:
        root = pathlib.Path(argv[argv.index("--root") + 1])
    if "--allow" in argv:
        allow = pathlib.Path(argv[argv.index("--allow") + 1])
    result = scan(root, allow)
    if "--json" in argv:
        print(json.dumps(result, ensure_ascii=False, indent=1))
    else:
        print(f"Rust 文件 {result.get('files', 0)} 个｜拦 {result['blocking']} 条｜警告 {result['warnings']} 条")
        for finding in result["findings"]:
            print(f"  [{'拦' if finding['blocking'] else '警'}|{finding['kind']}] "
                  f"{finding['file']}:{finding.get('line', '')} {finding['detail']}")
        print("结论：" + ("红" if result["red"] else "绿（上限都执法、读都有界、分配有兜底）"))
    return 1 if result["red"] else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
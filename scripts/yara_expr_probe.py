#!/usr/bin/env python3
"""表达式语言测试的官方复核：把 crates/trit-re/tests/yara_expr*.rs 里写死的
期望值逐条拿给官方 YARA（yara-python）重算一遍。

用法：
    pip install yara-python==4.5.4
    TRIT_EXPR_FIXTURE=/tmp/rich_pe.dll cargo test -p trit-re --test yara_expr_pe   # 导出合成 PE
    python -I scripts/yara_expr_probe.py --pe /tmp/rich_pe.dll
    python -I scripts/yara_expr_probe.py --self-test     # 只测解析，不需要 yara-python

测试文件的约定（本脚本只认这三种写法）：
  - 常量表  `const <名字>_<HIT|MISS|REJECT|GAP|UNKNOWN>: &[&str] = &[ … ];`
  - 行内表  `assert_all(&[ … ], Ternary::<True|False|Unknown>)`
  - 二元组  `("条件", Ternary::<True|False|Unknown>)`
  - 上下文  `// probe: sample=<small|rich> | strings=<串区> | touch=<追加子句>`
            写在表前，管到该表结束（顶格的 `];` 或 `}`）；缺省 = 13 字节
            默认样本、无串区。

判定：HIT / True = 官方必须命中；MISS / False = 官方必须不命中；REJECT = 官方
必须编译报错；GAP = 官方能编译（本实现如实 Unsupported 的子集边界，只列出
不判错）；UNKNOWN = 本实现弃权，官方给什么都不算错（只列出）。

官方引擎编译某些常量表达式会直接崩溃（`INT64_MIN \\ -1` → SIGFPE），所以每条
规则在子进程里跑。退出码：有不一致 = 1。
"""

import argparse
import os
import re
import sys
from pathlib import Path

SMALL = b"hello world\x00\xff"
ROOT = Path(__file__).resolve().parent.parent
TESTS = ROOT / "crates" / "trit-re" / "tests"

LITERAL = re.compile(r'r(#+)"(.*?)"\1|r"([^"]*)"|"((?:[^"\\]|\\.)*)"', re.S)
ESCAPES = {"n": "\n", "t": "\t", "r": "\r", "0": "\0", "\\": "\\", '"': '"', "'": "'"}


def unescape(body):
    out, at = [], 0
    while at < len(body):
        char = body[at]
        if char != "\\":
            out.append(char)
            at += 1
            continue
        nxt = body[at + 1]
        if nxt == "x":
            out.append(chr(int(body[at + 2 : at + 4], 16)))
            at += 4
        elif nxt == "\n":  # 行尾续行：吞掉换行与后续缩进
            at += 2
            while at < len(body) and body[at] in " \t\n":
                at += 1
        else:
            out.append(ESCAPES[nxt])
            at += 2
    return "".join(out)


def literals(text):
    for match in LITERAL.finditer(text):
        if match.group(1) is not None:
            yield match.group(2)
        elif match.group(3) is not None:
            yield match.group(3)
        else:
            yield unescape(match.group(4))


def strip_comments(text):
    return re.sub(r"(?m)^\s*//.*$", "", text)


KIND = {"True": "HIT", "False": "MISS", "Unknown": "UNKNOWN"}
TABLE = re.compile(
    r"^const \w+_(HIT|MISS|REJECT|GAP|UNKNOWN): &\[&str\] = &\[(.*?)\];", re.S | re.M
)
INLINE = re.compile(r"\bassert_all\(\s*&\[(.*?)\],\s*Ternary::(True|False|Unknown)", re.S)
TUPLE = re.compile(
    r'\(\s*(r(#+)"(?:(?!"\2).)*"\2|r"[^"]*"|"(?:[^"\\]|\\.)*")'
    r"\s*,\s*Ternary::(True|False|Unknown)\s*\)",
    re.S,
)
MARKER = re.compile(r"^\s*// probe:(.*)$", re.M)


def contexts(text):
    """[(起, 止, 上下文)]：标记行管到下一个顶格的 `];` 或 `}`。"""
    spans = []
    for match in MARKER.finditer(text):
        context = {"sample": "small", "strings": "", "touch": ""}
        for part in match.group(1).split("|"):
            key, _, value = part.partition("=")
            context[key.strip()] = value.strip(" ") if key.strip() != "touch" else value
        end = re.compile(r"^(\];|\})", re.M).search(text, match.end())
        spans.append((match.end(), end.end() if end else len(text), context))
    return spans


def cases(path):
    return parse(path.read_text(encoding="utf-8"))


def parse(text):
    spans = contexts(text)

    def context_at(offset):
        for start, end, context in spans:
            if start <= offset < end:
                return context
        return {"sample": "small", "strings": "", "touch": ""}

    seen = set()
    for match in TABLE.finditer(text):
        for condition in literals(strip_comments(match.group(2))):
            yield match.group(1), condition, context_at(match.start())
        seen.add((match.start(), match.end()))
    inside = lambda at: any(start <= at < end for start, end in seen)
    for match in INLINE.finditer(text):
        if inside(match.start()):
            continue
        for condition in literals(strip_comments(match.group(1))):
            yield KIND[match.group(2)], condition, context_at(match.start())
        seen.add((match.start(), match.end()))
    for match in TUPLE.finditer(text):
        if inside(match.start()):
            continue
        (condition,) = literals(match.group(1))
        yield KIND[match.group(3)], condition, context_at(match.start())


def official(condition, context, sample):
    """子进程里跑官方引擎：'HIT' / 'MISS' / 'ERR <消息>' / 'CRASH <信号>'。"""
    strings = f"strings: {context['strings']} " if context["strings"] else ""
    body = f"({condition}){context['touch']}" if context["touch"] else condition
    source = f'import "pe"\nrule r {{ {strings}condition: {body} }}\n'
    read_end, write_end = os.pipe()
    pid = os.fork()
    if pid == 0:
        os.close(read_end)
        import yara

        try:
            verdict = "HIT" if yara.compile(source=source).match(data=sample) else "MISS"
        except yara.Error as error:
            verdict = f"ERR {error}"
        os.write(write_end, verdict.encode("utf-8", "replace"))
        os._exit(0)
    os.close(write_end)
    chunks = []
    while chunk := os.read(read_end, 65536):
        chunks.append(chunk)
    os.close(read_end)
    _, status = os.waitpid(pid, 0)
    if os.WIFSIGNALED(status):
        return f"CRASH signal {os.WTERMSIG(status)}"
    return b"".join(chunks).decode("utf-8", "replace")


SELF_TEST = r'''
const A_HIT: &[&str] = &["1 == 1", r#"pe.x == "a\x41""#, "\"q\" == \"\x41\""];
fn between() { let x = (1, 2); }
// probe: sample=rich | strings=$a = "l" | touch= and #a >= 0
const B_REJECT: &[&str] = &[
    // "注释里的不算",
    "1 \\ 0 == 1",
];
const C_GAP: &[&str] = &["1.5 > 1"];
fn t() {
    assert_all(&["2 == 2", r"3 == 3"], Ternary::False);
    for (c, e) in [(r#""a" == "a""#, Ternary::True), (r##"r#"x"# == 1"##, Ternary::Unknown)] {}
    // probe: sample=rich
    let rows = [("pe.is_pe", Ternary::True)];
}
fn after() { let y = ("后面", Ternary::False); }
'''


def self_test():
    failures, checks = [], []

    def check(name, got, want):
        checks.append(name)
        if got != want:
            failures.append(f"{name}: {got!r} != {want!r}")

    got = list(parse(SELF_TEST))
    plain = {"sample": "small", "strings": "", "touch": ""}
    marked = {"sample": "rich", "strings": '$a = "l"', "touch": " and #a >= 0"}
    check("条数", len(got), 11)
    # 常量表：普通串、raw 串原样、转义串按 Rust 规则解开
    check("表-普通", got[0], ("HIT", "1 == 1", plain))
    check("表-raw", got[1], ("HIT", 'pe.x == "a\\x41"', plain))
    check("表-转义", got[2], ("HIT", '"q" == "A"', plain))
    # 标记只管到本表结束；注释掉的行不算用例；`\\\\` 解成一个反斜杠
    check("标记表", got[3], ("REJECT", "1 \\ 0 == 1", marked))
    check("标记不外溢", got[4], ("GAP", "1.5 > 1", plain))
    check("行内表", got[5:7], [("MISS", "2 == 2", plain), ("MISS", "3 == 3", plain)])
    # 二元组：单行里的多个 raw 串不能串在一起（曾经在这里崩过）
    check("二元组-raw", got[7], ("HIT", '"a" == "a"', plain))
    check("二元组-raw##", got[8], ("UNKNOWN", 'r#"x"# == 1', plain))
    check("函数内标记", got[9][0:2] + (got[9][2]["sample"],), ("HIT", "pe.is_pe", "rich"))
    check("函数内标记到 } 为止", got[10], ("MISS", "后面", plain))
    check("一行表之后的代码不被吞", [c for _, c, _ in got].count("1 == 1"), 1)
    check("续行", unescape("a\\\n    b"), "ab")
    check("十六进制转义", unescape("\\x2e\\n"), ".\n")
    for failure in failures:
        print(f"SELF-TEST FAIL {failure}")
    print(f"self-test：{len(checks) - len(failures)}/{len(checks)} 通过")
    return 1 if failures else 0


def main():
    if "--self-test" in sys.argv[1:]:
        return self_test()
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("--pe", type=Path, help="导出的合成 PE（sample=rich 的表要用）")
    parser.add_argument("--verbose", action="store_true", help="GAP / UNKNOWN 也逐条列出")
    parser.add_argument("files", nargs="*", type=Path, help="缺省 = tests/yara_expr*.rs")
    args = parser.parse_args()
    try:
        import yara
    except ImportError:
        print("需要 yara-python：pip install yara-python==4.5.4", file=sys.stderr)
        return 2
    rich = args.pe.read_bytes() if args.pe else None
    tally, bad, skipped = {}, 0, 0
    for path in args.files or sorted(TESTS.glob("yara_expr*.rs")):
        for kind, condition, context in cases(path):
            if context["sample"] == "rich" and rich is None:
                skipped += 1
                continue
            sample = rich if context["sample"] == "rich" else SMALL
            got = official(condition, context, sample)
            ok = {
                "HIT": got == "HIT",
                "MISS": got == "MISS",
                "REJECT": got.startswith(("ERR", "CRASH")),
                "GAP": not got.startswith(("ERR", "CRASH")),
                "UNKNOWN": True,
            }[kind]
            tally[kind] = tally.get(kind, 0) + 1
            if not ok:
                bad += 1
                print(f"MISMATCH {path.name}: 期望 {kind}，官方 {got} | {condition}")
            elif args.verbose and kind in ("GAP", "UNKNOWN"):
                print(f"{kind:8} {path.name}: 官方 {got} | {condition}")
    summary = " ".join(f"{kind}={count}" for kind, count in sorted(tally.items()))
    print(f"yara-python {yara.__version__} / YARA {yara.YARA_VERSION}：{summary} 不一致={bad}")
    if skipped:
        print(f"跳过 {skipped} 条 sample=rich 的用例（没给 --pe）", file=sys.stderr)
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())

#!/usr/bin/env python3
"""生成 YARA 语义向量：用官方 yara-python 4.5.4 冻结读数。

用法（需要能 import yara 的 Python）：
    python tools/gen_vectors.py            # 写 tests/data/yara_expr/*.tsv + fixtures
    python tools/gen_vectors.py --check    # 只复核，不写

输出格式与 docs/YARA-EXPR/04-CODE-MAP.md 一致：
    expr_core.tsv:  id  fixture  strings  cond  official
    rule_level.tsv: id  fixture  source   official
official = HIT / MISS / ERR（编译错误）。

规则源就写在本文件里 —— 这是向量的唯一真相；改规则必须重新生成 TSV。
本套是 P0 的**可复现最小集**（约 80 条），不是外部规范的 2732 条全集。
"""
from __future__ import annotations

import argparse
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
OUT = ROOT / "crates" / "trit-re" / "tests" / "data" / "yara_expr"

TEXT = b"the quick brown fox jumps over the lazy dog 0123456789 abcdef"
BYTES256 = bytes(range(256))

FIXTURES = {
    "text": TEXT,
    "bytes256": BYTES256,
    "empty": b"",
}

# (id, group, fixture, strings, cond)
EXPR = [
    # A 算术 / 位运算 / 编译期检查
    ("A001", "A", "text", None, "filesize + 1 > 0"),
    ("A002", "A", "text", None, "filesize - 1 == 60"),
    ("A003", "A", "text", None, "-5 \ 2 == -2"),
    ("A004", "A", "text", None, "5 % 3 == 2"),
    ("A005", "A", "text", None, "-5 % 3 == -2"),
    ("A006", "A", "text", None, "1 << 63 < 0"),
    ("A007", "A", "text", None, "1 << 64 == 0"),
    ("A008", "A", "text", None, "~0 == -1"),
    ("A009", "A", "text", None, "1 \ 0 == 0"),
    ("A010", "A", "text", None, "filesize / 2 > 1"),
    ("A011", "A", "text", None, "9223372036854775807 + 1 < 0"),
    ("A012", "A", "text", None, "0x10 == 16"),
    ("A013", "A", "text", None, "1 | 2 == 3"),
    ("A014", "A", "text", None, "1 ^ 1 == 0"),
    ("A015", "A", "text", None, "6 & 3 == 2"),
    # B 布尔上下文
    ("B001", "B", "text", None, "filesize"),
    ("B002", "B", "text", None, "0"),
    ("B003", "B", "text", None, "-1"),
    ("B004", "B", "text", None, "1.5"),
    ("B005", "B", "text", None, "0.0"),
    ("B006", "B", "text", None, "-0.0"),
    ("B007", "B", "text", None, '"abc"'),
    ("B008", "B", "text", None, '""'),
    ("B009", "B", "text", None, "/a/"),
    # U undefined 传播
    ("U001", "U", "text", None, "uint8(filesize) == 0"),
    ("U002", "U", "text", None, "not (uint8(filesize) == 0)"),
    ("U003", "U", "text", None, "defined uint8(filesize)"),
    ("U004", "U", "text", None, "not defined uint8(filesize)"),
    ("U005", "U", "text", None, "uint8(filesize) * 0 == 0"),
    ("U006", "U", "text", None, "uint8(filesize) == uint8(filesize)"),
    # L 循环
    ("L001", "L", "text", None, "for any i in (0..2) : (i == 0)"),
    ("L002", "L", "text", None, "for all i in (0..2) : (i >= 0)"),
    ("L003", "L", "text", None, "for 2 i in (0..0) : (5)"),
    ("L004", "L", "text", None, "for all i in (0..0) : (true)"),
    ("L005", "L", "text", None, "for any i in (0..0) : (true)"),
    ("L006", "L", "text", None, "for none i in (0..0) : (true)"),
    ("L007", "L", "text", None, "for any i in (2..1) : (true)"),
    ("L008", "L", "text", None, "for 3 i in (1..2) : (i)"),
    ("L009", "L", "text", None, "for any i in (0..1) : (for any j in (0..1) : (i == j))"),
    # S 字符串
    ("S001", "S", "text", None, '"abc" contains "b"'),
    ("S002", "S", "text", None, '"abc" contains ""'),
    ("S003", "S", "text", None, '"" contains ""'),
    ("S004", "S", "text", None, '"abc" startswith "ab"'),
    ("S005", "S", "text", None, '"abc" endswith "bc"'),
    ("S006", "S", "text", None, '"\\xff" < "a"'),
    ("S007", "S", "text", None, '"\\x00" > "\\xff"'),
    ("S008", "S", "text", None, '"a\\x00" > "a"'),
    ("S009", "S", "text", None, '"abc" == "abc\\x00"'),
    ("S010", "S", "text", None, '"abc" matches /b/'),
    ("S011", "S", "text", None, '"abc" matches /^b/'),
    ("S012", "S", "text", None, '"[" iequals "{"'),
    ("S013", "S", "text", None, '"abc" icontains "B"'),
    # N 读数函数
    ("N001", "N", "text", None, "uint8(0) == 0x74"),
    ("N002", "N", "text", None, "int8(0) == 0x74"),
    ("N003", "N", "text", None, "uint16(0) == 0x6874"),
    ("N004", "N", "text", None, "uint16be(0) == 0x7468"),
    ("N005", "N", "bytes256", None, "int8(255) == -1"),
    ("N006", "N", "text", None, "uint8(1000) == 0"),
    # F 浮点
    ("F001", "F", "text", None, "0.1 + 0.2 == 0.3"),
    ("F002", "F", "text", None, "0.1 + 0.2 > 0.3"),
    ("F003", "F", "text", None, "1.0 \ 0 > 1"),
    ("F004", "F", "text", None, "0.0 \ 0 == 0.0"),
    ("F005", "F", "text", None, "1.5 == 1.5"),
    ("F006", "F", "text", None, "1.5 % 1.0 == 0.5"),
    ("F007", "F", "text", None, "1. > 0.0"),
]

EXPR += [
    # M 串主表达式与 of
    ("M001", "M", "text", '$a = "the"', "$a"),
    ("M002", "M", "text", '$a = "the"', "$a at 0"),
    ("M003", "M", "text", '$a = "the"', "$a in (0..10)"),
    ("M004", "M", "text", '$a = "the"', "#a >= 1"),
    ("M005", "M", "text", '$a = "the"', "@a == 0"),
    ("M006", "M", "text", '$a = "the"', "!a == 3"),
    ("M007", "M", "text", '$a = "the"', "@a[0] == 0"),
    ("M008", "M", "text", '$a = "the"  $b = "zzz"', "any of them"),
    ("M009", "M", "text", '$a = "the"', "2 of ($a, $a)"),
    ("M010", "M", "text", '$b = "zzz"', "$b"),
    ("M011", "M", "text", '$b = "zzz"', "@b == 0"),
    # E 串声明错误
    ("E001", "E", "text", '$a = ""', "$a"),
    ("E002", "E", "text", '$a = "a" nocase nocase', "$a"),
    ("E003", "E", "text", '$a = "a" xor nocase', "$a"),
]

# (id, fixture, source)
RULE_LEVEL = [
    ("R001", "text", 'rule a { condition: true } rule b { condition: a }'),
    ("R002", "text", 'private rule a { condition: true } rule b { condition: a }'),
    ("R003", "text", 'global rule g { condition: false } rule a { condition: true }'),
    ("R004", "text", 'rule a { condition: true } rule b { condition: false }'),
]


def rule_source(strings: str | None, cond: str) -> str:
    body = ""
    if strings:
        body += "\n  strings:\n    " + strings
    return f"rule v {{{body}\n  condition:\n    {cond}\n}}\n"


def compile_match(source: str, data: bytes) -> str:
    import yara

    try:
        rules = yara.compile(source=source)
    except yara.Error:
        return "ERR"
    return "HIT" if rules.match(data=data) else "MISS"


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--check", action="store_true", help="只复核，不写文件")
    args = ap.parse_args()

    rows = []
    for vid, _group, fixture, strings, cond in EXPR:
        src = rule_source(strings, cond)
        official = compile_match(src, FIXTURES[fixture])
        rows.append((vid, fixture, strings or "-", cond, official))

    rule_rows = []
    for vid, fixture, src in RULE_LEVEL:
        import yara

        try:
            matched = yara.compile(source=src).match(data=FIXTURES[fixture])
            official = "HITS:" + ",".join(sorted(m.rule for m in matched))
        except yara.Error:
            official = "ERR"
        rule_rows.append((vid, fixture, src, official))

    if args.check:
        print(f"expr_core: {len(rows)} 条；rule_level: {len(rule_rows)} 条")
        for r in rows:
            print("\t".join(r))
        for r in rule_rows:
            print("\t".join(r))
        return 0

    (OUT / "fixtures").mkdir(parents=True, exist_ok=True)
    for name, data in FIXTURES.items():
        text = "\n".join(data[i : i + 32].hex() for i in range(0, len(data), 32))
        (OUT / "fixtures" / f"{name}.hex").write_text(text + "\n", encoding="ascii")
    with (OUT / "expr_core.tsv").open("w", encoding="utf-8", newline="\n") as f:
        f.write("id\tfixture\tstrings\tcond\tofficial\n")
        for r in rows:
            f.write("\t".join(r) + "\n")
    with (OUT / "rule_level.tsv").open("w", encoding="utf-8", newline="\n") as f:
        f.write("id\tfixture\tsource\tofficial\n")
        for r in rule_rows:
            f.write("\t".join(r) + "\n")
    print(f"wrote {len(rows)} expr vectors + {len(rule_rows)} rule vectors")
    return 0


if __name__ == "__main__":
    sys.exit(main())

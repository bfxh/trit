# -*- coding: utf-8 -*-
from pathlib import Path

path = Path("other-languages/asm/src/main.rs")
lines = path.read_text(encoding="utf-8").split("\n")

def line_index(prefix):
    hits = [i for i, l in enumerate(lines) if l.startswith(prefix)]
    assert len(hits) == 1, (prefix, hits)
    return hits[0]

a_start = line_index("fn expected_and(")
b_start = line_index("fn run_once(")
c_start = line_index("fn k3_bits(")
main_start = line_index("fn main()")
assert a_start < b_start < c_start < main_start
assert lines[c_start - 1].strip() == "" or lines[c_start - 1].strip().startswith("#")

range_a = lines[a_start:b_start]
range_b = lines[c_start:main_start]

checks = ['//! 正确性检查：K3 真值表、随机长度、汇编/内建路径一致性。',
          '//!',
          '//! 从 `main.rs` 拆出（模块预算 ≤800 行）；逻辑未改动。',
          '',
          'use crate::{',
          '    Plane, avx2_and, avx2_available, avx2_not, avx2_or, scalar_and, scalar_not, scalar_or,',
          '};',
          '']
checks.extend(range_a)
checks.extend(range_b)
checks_text = "\n".join(checks)

# 三个入口供 main 调用
for name in ["fn check_truth_tables(", "fn check_lengths(", "fn k3_conformance("]:
    checks_text = checks_text.replace(name, "pub(crate) " + name, 1)
Path("other-languages/asm/src/checks.rs").write_text(checks_text, encoding="utf-8", newline="\n")

rest = lines[:a_start] + lines[b_start:c_start] + lines[main_start:]
text = "\n".join(rest)
text = text.replace("mod avx512;", "mod avx512;\nmod checks;", 1)
text = text.replace("    check_truth_tables();", "    checks::check_truth_tables();", 1)
text = text.replace("    check_lengths();", "    checks::check_lengths();", 1)
text = text.replace("        let k3 = k3_conformance();", "        let k3 = checks::k3_conformance();", 1)

# 跨模块可见性
for old, new in [
    ("#[derive(Clone)]\nstruct Plane {\n    ones: Vec<u64>,\n    unk: Vec<u64>,\n}",
     "#[derive(Clone)]\npub(crate) struct Plane {\n    pub(crate) ones: Vec<u64>,\n    pub(crate) unk: Vec<u64>,\n}"),
    ("\nfn avx2_available() -> bool {", "\npub(crate) fn avx2_available() -> bool {"),
    ("\nfn avx2_and(", "\npub(crate) fn avx2_and("),
    ("\nfn avx2_or(", "\npub(crate) fn avx2_or("),
    ("\nfn avx2_not(", "\npub(crate) fn avx2_not("),
    ("\nfn scalar_and(", "\npub(crate) fn scalar_and("),
    ("\nfn scalar_or(", "\npub(crate) fn scalar_or("),
    ("\nfn scalar_not(", "\npub(crate) fn scalar_not("),
]:
    assert text.count(old) == 1, (old[:30], text.count(old))
    text = text.replace(old, new)

path.write_text(text, encoding="utf-8", newline="\n")
print("main.rs lines:", len(text.split("\n")))
print("checks.rs lines:", len(checks_text.split("\n")))

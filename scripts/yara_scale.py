#!/usr/bin/env python3
"""属性驱动的差分测试生成器（N4 对拍规模曲线 / 论文 E5）。

固定种子确定性生成「核心子集语法规则 × 随机样本」批次，然后复用
yara_diff.py 做官方 vs trit-re 的四态对拍。判据：**DIFF 必须 0**；
SUBSET-GAP 只许来自已知边界类。差分测试此前两次抓出真语义偏差
（`at` 首次→任一、`@a` 下标基）——这是它的规模化形态。

用法：
  python scripts/yara_scale.py --trit <trit-re.exe> [--rules 40 --samples 12 --seed 7]
  python scripts/yara_scale.py --trit <trit-re.exe> --probe   # 边界探针小批（差异作校准数据，不判红）

样本构造纪律：填充字节 0x77 不出现在任何针里 → 每根针的命中次数与
偏移全程确定；规则引用的针按样本确定性布点，命中/未命中两种裁决都有。
"""

from __future__ import annotations

import argparse
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

FILLER = 0x77
HERE = Path(__file__).resolve().parent


class Xorshift64:
    """确定性 PRNG（与 tests/fuzz_sigscan.rs 同款 xorshift64）——

    差分测试的可复现性要求固定种子；不用 random 模块（测试数据生成器
    不是加密用途，但确定性 PRNG 让"同种子同批次"成为可复算的硬承诺）。
    """

    def __init__(self, seed: int) -> None:
        self.state = seed & 0xFFFFFFFFFFFFFFFF or 0x9E3779B97F4A7C15

    def next(self) -> int:
        x = self.state
        x ^= (x << 13) & 0xFFFFFFFFFFFFFFFF
        x ^= x >> 7
        x ^= (x << 17) & 0xFFFFFFFFFFFFFFFF
        self.state = x
        return x

    def below(self, n: int) -> int:
        return self.next() % n if n else 0


def gen_samples(rng, count: int, needles: list[bytes], root: Path) -> None:
    root.mkdir(parents=True, exist_ok=True)
    for j in range(count):
        size = 64 + rng.below(448)
        buf = bytearray([FILLER]) * size if False else bytearray([FILLER] * size)
        for needle in needles:
            if len(needle) + 8 > size or rng.below(4) == 0:
                continue  # 短样本放不下 / 1/4 概率缺席（未命中裁决也要有）
            pos = rng.below(size - len(needle) - 1)
            buf[pos : pos + len(needle)] = needle
        (root / f"s{j:03d}.bin").write_bytes(bytes(buf))


def gen_rules(rng, count: int, needles: list[bytes], wide: list[str]) -> list[str]:
    rules = []
    for i in range(count):
        n = needles[i % len(needles)]
        w = wide[i % len(wide)]
        hx = " ".join(f"{b:02X}" for b in n[:4]) + " ?? " + " ".join(
            f"{b:02X}" for b in n[4:6]
        )
        k = 0x20 + (i % 0x20)
        xored = " ".join(f"{b ^ k:02X}" for b in n[:5])
        kind = i % 10
        body = [
            # hex：字面量 + 通配（前 4 字节 + ?? + 2 字节——针前 6 字节布进样本）
            f'$h = {{ {hx} }}  condition: $h',
            # nocase 文本
            f'$t = "{n[:6].decode("ascii", "strict")}" nocase  condition: $t',
            # wide（针布的是 UTF-16LE 形态）
            f'$w = "{w}" wide  condition: $w',
            # xor(k)（样本布的是原文 ^ k）
            f'$x = "{n[:5].decode("ascii", "strict")}" xor({k:#x})  condition: $x',
            # 计数 + 首次偏移（1 起：@a[1] = 首次命中）
            f'$c = "{n.decode("ascii", "strict")}"  condition: #c >= 0 and @c[1] < 100000',
            # 区间
            f'$i = "{n[:6].decode("ascii", "strict")}"  condition: $i in (0 .. 100000)',
            # uintN（偏移 0 恒在界内）
            f'$u = "{n[:4].decode("ascii", "strict")}"  condition: uint16(0) <= 0xFFFF and $u',
            # of 集合
            f'$a = "{n[:5].decode("ascii", "strict")}"\n    $b = "{n[5:9].decode("ascii", "strict")}"\n  condition: 1 of ($a*) or all of them',
            # for-of：体内计数
            f'$f = "{n[:6].decode("ascii", "strict")}"  condition: for any of ($f*) : ( # >= 0 )',
            # filesize（0..1MB 恒真的比较，裁决由串决定）
            f'$z = "{n[:6].decode("ascii", "strict")}"  condition: filesize < 1MB and $z',
        ][kind]
        rules.append(f"rule gen{i:03d} {{\n  strings:\n    {body}\n}}\n")
    return rules


PROBE_RULES = """// 边界探针：未验证组合单列——DIFF/SUBSET-GAP 是校准数据不是判红
// （xor nocase 已由探针判定：官方编译器拒绝该组合——解析层已对齐拒绝）
rule p_xor_wide {
  strings:
    $a = "AZ" xor(0x20) wide
  condition:
    $a
}
rule p_in_filesize_end {
  strings:
    $a = "AB"
  condition:
    $a in (0 .. filesize)
}
rule p_uint_oob {
  condition:
    uint32(1000000) == 0
}
rule p_for_named_off {
  strings:
    $a = "AB"
  condition:
    for any of ($a*) : ( @a[1] >= 0 )
}
"""


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--trit", required=True, help="trit-re 可执行文件路径")
    parser.add_argument("--rules", type=int, default=40)
    parser.add_argument("--samples", type=int, default=12)
    parser.add_argument("--seed", type=int, default=7)
    parser.add_argument("--probe", action="store_true", help="边界探针小批（不判红）")
    args = parser.parse_args()

    rng = Xorshift64(args.seed)
    # 针：独特 ASCII 串（互不为前后缀、不含填充字节 0x77、含大小写与数字
    # ——nocase/xor/fullword 的判定面都吃得到）
    needles = [f"nd{i:02d}Qz{i % 10}".encode("ascii") for i in range(10)]
    wide = [f"wd{i:02d}".encode("ascii") for i in range(5)]

    work = Path(tempfile.mkdtemp(prefix="yara-scale-"))
    try:
        gen_samples(rng, args.samples, needles + [f"wd{i:02d}".encode("utf-16-le") for i in range(len(wide))] + [bytes(b ^ 0x20 for b in n[:5]) for n in needles[:5]], work / "samples")
        rules_text = (
            PROBE_RULES
            if args.probe
            else "\n".join(gen_rules(rng, args.rules, needles, [f"wd{i:02d}" for i in range(len(wide))]))
        )
        rules_path = work / "batch.yar"
        rules_path.write_text(rules_text, encoding="utf-8")

        result = subprocess.run(
            [sys.executable, str(HERE / "yara_diff.py"),
             "--trit", args.trit, "--rules", str(rules_path), "--samples", str(work / "samples")],
            capture_output=True, text=True, encoding="utf-8", errors="replace",
        )
        print(result.stdout[-4000:])
        if result.returncode != 0 and not args.probe:
            print(result.stderr[-1000:], file=sys.stderr)
            return 1
        m = re.search(r"对拍汇总：样本 (\d+)\s+MATCH (\d+)\s+DIFF (\d+)\s+SUBSET-GAP (\d+)\s+OFFICIAL-ERR (\d+)", result.stdout)
        if not m:
            print("汇总行缺失；harness stderr：", file=sys.stderr)
            print(result.stderr[-2000:], file=sys.stderr)
            return 2
        total, match, diff, gap, oerr = map(int, m.groups())
        print(f"\n[yara-scale] seed={args.seed} rules={args.rules} samples={args.samples} "
              f"→ 判定对 {total}：MATCH {match} / DIFF {diff} / SUBSET-GAP {gap} / OFFICIAL-ERR {oerr}")
        if diff and not args.probe:
            print("DIFF 非零：差分测试抓到语义偏差——修完再合", file=sys.stderr)
            return 1
        return 0
    finally:
        shutil.rmtree(work, ignore_errors=True)


if __name__ == "__main__":
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    sys.exit(main())

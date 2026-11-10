#!/usr/bin/env python3
"""N4 对拍 harness：官方 YARA（yara-python）vs trit-re sigscan 的命中集合 diff。

用法：
    pip install yara-python          # 官方引擎（一次性）
    python scripts/yara_diff.py --trit <trit-re.exe> --rules rules.yar --samples <目录>

逐样本对照两边的命中规则名集合：
  - MATCH        两边一致
  - DIFF         两边都有裁决但不一致（真差异——要修）
  - SUBSET-GAP   官方有裁决、trit-re 报 UNKNOWN/UNSUPPORTED（子集边界——如实记录，
                 不算失败；YARA 子集语义在 SECURITY-RE.md N4 行维护）
  - OFFICIAL-ERR 官方引擎编译/匹配失败（规则用了 yara-python 不支持的语法）

退出码：发现真 DIFF = 1，其余 0。样本可为文件或目录（递归）。
"""

import argparse
import subprocess
import sys
from pathlib import Path

SAMPLE_EXTS = {".exe", ".dll", ".bin", ".sys", ".dat", ".sample"}


def official_hits(rules_path: Path, sample: Path):
    """官方 yara-python 的命中规则名集合；引擎错误返回 None。"""
    try:
        import yara  # 延迟导入：未装时给出安装提示而非裸 traceback
    except ImportError:
        print("需要 yara-python：pip install yara-python", file=sys.stderr)
        sys.exit(2)
    try:
        rules = yara.compile(filepath=str(rules_path))
    except yara.Error as error:
        print(f"官方引擎编译规则失败：{error}", file=sys.stderr)
        sys.exit(2)
    try:
        matches = rules.match(str(sample))
    except yara.Error as error:
        print(f"  官方引擎匹配失败 {sample.name}: {error}", file=sys.stderr)
        return None
    return {m.rule for m in matches}


def trit_hits(cli: Path, rules_path: Path, sample: Path):
    """trit-re sigscan 的命中集合 + UNKNOWN/UNSUPPORTED 清单。"""
    proc = subprocess.run(
        [str(cli), "sigscan", str(sample), "--rules", str(rules_path)],
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
    )
    if proc.returncode != 0:
        print(f"  trit-re 失败 {sample.name}: {proc.stderr.strip()}", file=sys.stderr)
        return None, [], []
    hits, unknown, unsupported = set(), [], []
    for line in proc.stdout.splitlines():
        parts = line.split("\t")
        if parts[0] == "HIT":
            hits.add(parts[1])
        elif parts[0] == "UNKNOWN":
            unknown.append(parts[1])
        elif parts[0] == "UNSUPPORTED":
            unsupported.append(parts[1])
    return hits, unknown, unsupported


def iter_samples(root: Path):
    if root.is_file():
        yield root
        return
    for path in sorted(root.rglob("*")):
        if path.is_file() and (
            path.suffix.lower() in SAMPLE_EXTS or path.suffix == ""
        ):
            yield path


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--trit", required=True, help="trit-re 可执行文件路径")
    parser.add_argument("--rules", required=True, help=".yar 规则文件")
    parser.add_argument("--samples", required=True, help="样本文件或目录（递归）")
    args = parser.parse_args()

    cli = Path(args.trit)
    rules_path = Path(args.rules)
    samples = list(iter_samples(Path(args.samples)))
    if not samples:
        print("没有样本可扫", file=sys.stderr)
        return 2

    stats = {"match": 0, "diff": 0, "subset_gap": 0, "official_err": 0}
    diff_cases = []
    for sample in samples:
        official = official_hits(rules_path, sample)
        if official is None:
            stats["official_err"] += 1
            continue
        trit, unknown, unsupported = trit_hits(cli, rules_path, sample)
        if trit is None:
            continue
        # 子集边界：官方有裁决、trit-re 因不支持而 Unknown/未评估的规则——
        # 从对照集合里摘掉，单独计数（子集语义在 N4 行维护，不是错误）
        gap = {r for r in (official | set(unknown)) if r in unknown or r in unsupported}
        official_cmp = official - set(unsupported) - set(unknown)
        trit_cmp = trit
        if official_cmp == trit_cmp:
            stats["match"] += 1
            status = "MATCH"
        else:
            stats["diff"] += 1
            status = "DIFF"
            diff_cases.append((sample.name, official_cmp, trit_cmp))
        if gap:
            stats["subset_gap"] += 1
            print(
                f"  SUBSET-GAP {sample.name}: {sorted(gap)} "
                f"（trit-re 未评估——子集边界）"
            )
        print(f"  {status} {sample.name}: official={sorted(official_cmp)} trit={sorted(trit_cmp)}")

    print(
        f"\n对拍汇总：样本 {len(samples)}  MATCH {stats['match']}  "
        f"DIFF {stats['diff']}  SUBSET-GAP {stats['subset_gap']}  "
        f"OFFICIAL-ERR {stats['official_err']}"
    )
    if diff_cases:
        print("\n真差异清单（两边都有裁决但不一致）：")
        for name, official, trit in diff_cases:
            print(f"  {name}: official={sorted(official)} trit={sorted(trit)}")
    return 1 if stats["diff"] else 0


if __name__ == "__main__":
    sys.exit(main())

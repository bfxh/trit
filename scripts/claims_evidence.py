#!/usr/bin/env python3
"""能力句门：文档里每句「支持 / 已实现 / 可以 / 完成 / 达标 / 最快」必须带**复算指针**或**读数**。

用法：
  python scripts/claims_evidence.py              # 判定（新增无据能力句 → 退出码 1）
  python scripts/claims_evidence.py --update     # 收紧棘轮（变多就整体拒绝写入）
  python scripts/claims_evidence.py --self-test   # 正例 + 四类反证

为什么（2026-11-05 用户口径）：「有些地方我以为你完成了但没完成——那些小模型其实是组件；
好多地方有迷惑性，我最反感」。迷惑性最主要的载体就是**没有证据的能力句**。
口径正本在 `docs/STATUS.md` §1：状态词只有 已跑通 / 已实现未测 / 组件 / 已否证，
写不出「命令」也写不出「读数」的句子就不许用完成类动词。

棘轮：存量无据能力句按文件计数进基线，**只许变少不许变多**——不逼别人一次性改历史，
但不许再加新的。基线读不出 ⇒ 判红。
"""

from __future__ import annotations

import argparse
import json
import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parents[1]
BASELINE = pathlib.Path(__file__).resolve().parent / "claims_evidence_baseline.json"

CAPABILITY = re.compile(
    r"支持|已实现|已经实现|可以做|已能|已完成|完成度|达标|最快的|已落地|已跑通|已验证|已修|已合"
)
MEASUREMENT = re.compile(
    r"\d+(?:[.,]\d+)?\s*(?:×|x|%|pp|‰|ms|µs|us|ns|ps|s/|B/|MB|GB|G |K |M |dB|bps|bit|字节|"
    r"条|个|张|步|轮|种子|/秒|/s\b|人|行|倍)"
)
POINTER = re.compile(r"`[^`]*(?:--|\.(?:rs|py|md|json|wgsl)|cargo |python |bash )|(?:[\w./-]+)[.:]\d+|PR\s*#\d+|§|A\d{2,}")


def scanned(root: pathlib.Path) -> list[pathlib.Path]:
    files: list[pathlib.Path] = []
    # CHANGELOG 是叙述记录（「已修 X」是历史而非能力声明），不进能力面；能力面 = README + docs/。
    for name in ("README.md",):
        path = root / name
        if path.is_file():
            files.append(path)
    docs = root / "docs"
    if docs.is_dir():
        files += sorted(docs.rglob("*.md"))
    return files


def violations(text: str) -> list[str]:
    """逐行找「有完成类动词、但既无读数也无复算指针」的句子。"""
    found: list[str] = []
    for line in text.splitlines():
        stripped = line.strip()
        if not stripped or stripped.startswith("```"):
            continue
        for sentence in re.split(r"[。;；]\s*", stripped):
            if not CAPABILITY.search(sentence):
                continue
            if MEASUREMENT.search(sentence) or POINTER.search(sentence):
                continue
            if len(sentence) < 12:
                continue
            found.append(sentence[:80])
    return found


def counts(root: pathlib.Path) -> dict[str, int]:
    out: dict[str, int] = {}
    for path in scanned(root):
        bad = violations(path.read_text(encoding="utf-8", errors="replace"))
        if bad:
            out[path.relative_to(root).as_posix()] = len(bad)
    return out


def compare(found: dict[str, int], baseline: dict[str, int]) -> list[str]:
    problems: list[str] = []
    for path, count in sorted(found.items()):
        allowed = baseline.get(path, 0)
        if count > allowed:
            problems.append(
                f"{path} 有 {count} 句无据能力句，基线允许 {allowed} ——"
                " 完成类动词必须带读数或复算指针（docs/STATUS.md §1）"
            )
    return problems


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description="能力句门（无复算指针或读数的完成类断言判红）")
    parser.add_argument("--update", action="store_true")
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args(argv)
    if args.self_test:
        return self_test()

    if args.update:
        baseline = (
            json.loads(BASELINE.read_text(encoding="utf-8")) if BASELINE.is_file() else {}
        )
        first = not BASELINE.is_file()
    elif not BASELINE.is_file():
        print(f"FAIL 基线读不出：{BASELINE} —— 不知道有多少不等于没有")
        return 1
    else:
        baseline = json.loads(BASELINE.read_text(encoding="utf-8"))
    found = counts(ROOT)
    if args.update:
        grew = [f"{k}: {baseline.get(k, 0)} → {v}" for k, v in sorted(found.items()) if v > baseline.get(k, 0)]
        if grew and not first:
            print("REFUSED 无据能力句变多了，--update 不接受：" + "；".join(grew))
            return 1
        BASELINE.write_text(
            json.dumps(found, ensure_ascii=False, indent=1, sort_keys=True) + "\n", encoding="utf-8"
        )
        print(f"台账已写：{len(found)} 个文件共 {sum(found.values())} 句无据能力句 → {BASELINE.name}")
        return 0

    problems = compare(found, baseline)
    print(f"能力句门：扫 {len(scanned(ROOT))} 个文档，无据句 {sum(found.values())} 句（基线 {sum(baseline.values())}）")
    for problem in problems:
        print(f"  拦：{problem}")
    if problems:
        print(f"FAIL 拦 {len(problems)} 条")
        return 1
    print("OK 没有新增无据能力句")
    return 0


def self_test() -> int:
    failures = 0

    def expect(label: str, bad: list[str], want: str | None) -> None:
        nonlocal failures
        ok = (not bad) if want is None else any(want in line for line in bad)
        if ok:
            print(f"  ok   {label}")
        else:
            failures += 1
            print(f"  FAIL {label}: {bad}")

    expect(
        "正例：带读数",
        violations("三值卷积栈已跑通，留出 8.08% / MAE 0.0747。"),
        None,
    )
    expect(
        "正例：带复算指针（命令或行号）",
        violations("计数已跑通，见 `cargo run -p trit-count --example matching_family`。"),
        None,
    )
    expect(
        "正例：带文档内引用",
        violations("该结论已落地，依据 §3 与 PR #299。"),
        None,
    )
    expect("反证 1：只有动词没有证据", violations("三值扩散模型已经完成并支持多种功能。"), "三值扩散模型")
    expect("反证 2：有数字但没有单位口径", violations("推理已跑通，速度提升 30。"), "已跑通")
    expect(
        "反证 3：棘轮只许变少",
        [p for p in compare({"docs/A.md": 3}, {"docs/A.md": 2})],
        "docs/A.md",
    )
    print("自测结论：%s" % ("绿" if failures == 0 else f"红 {failures} 条"))
    return 1 if failures else 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))

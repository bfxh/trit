#!/usr/bin/env python3
"""WSP-BUDGET 按来源拆解：回答“总体增益是不是某一个数据集撑起来的”。

输入是 bert6 `TRIT_DUMP_PREDS=1` 产出的多个 JSON，以及带 `pairs[].score` 的标签
文件。`--sources` 可以是与 pairs 等长的字符串列表，也可以是带 `dataset` 字段的
对象列表；本仓 STS 的全量 pairs 与 `mteb/stsbenchmark-sts` test 的行一一对应。

只读 preds，不重跑模型：

  python scripts/wsp_budget_source_eval.py \
    --preds int3=l12-int3-preds-full.json \
    --preds K10K11=l12-budget-k10k11-preds-full.json \
    --preds R12=r12/r12-full.json \
    --pairs miniLM-l12-sts-full/pairs.json \
    --sources sts-sources.json --out r12-by-source.json
"""
from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

from wsp_budget_bootstrap import get_labels, get_preds, pearson, spearman


def load_groups(path: Path, key: str) -> list[str]:
    obj = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(obj, list) or not obj:
        raise SystemExit(f"{path} 需要非空列表")
    groups = []
    for item in obj:
        if isinstance(item, str):
            groups.append(item)
        elif isinstance(item, dict) and key in item:
            groups.append(str(item[key]))
        else:
            raise SystemExit(f"{path} 的条目既不是字符串也没有 {key!r}")
    return groups


def metrics(preds: list[float], labels: list[float]) -> dict:
    return {"pearson": pearson(preds, labels), "spearman": spearman(preds, labels)}


def evaluate(preds: dict[str, list[float]], labels: list[float],
             groups: list[str] | None) -> list[dict]:
    names = list(preds)
    rows = []

    def add(group: str, idx: list[int]) -> None:
        row = {"group": group, "n": len(idx), "candidates": {}}
        for name in names:
            row["candidates"][name] = metrics([preds[name][i] for i in idx],
                                               [labels[i] for i in idx])
        rows.append(row)

    add("ALL", list(range(len(labels))))
    if groups is not None:
        if len(groups) != len(labels):
            raise SystemExit(f"分组长度 {len(groups)} != 标签长度 {len(labels)}")
        for group in sorted(set(groups)):
            add(group, [i for i, value in enumerate(groups) if value == group])
    return rows


def self_test() -> None:
    labels = [1.0, 2.0, 3.0, 4.0]
    preds = {"a": [1.0, 2.0, 3.0, 4.0], "b": [1.0, 2.0, 3.0, 3.0]}
    groups = ["x", "x", "y", "y"]
    rows = evaluate(preds, labels, groups)
    assert rows[0]["group"] == "ALL" and rows[0]["n"] == 4
    assert abs(rows[0]["candidates"]["a"]["pearson"] - 1.0) < 1e-12
    assert abs(rows[0]["candidates"]["a"]["spearman"] - 1.0) < 1e-12
    assert [r["group"] for r in rows[1:]] == ["x", "y"]
    assert all(r["n"] == 2 for r in rows[1:])
    # 同一组内完全一致，跨组方向相反：分组后不再等于总体
    assert abs(rows[1]["candidates"]["a"]["pearson"] - 1.0) < 1e-12
    assert abs(rows[2]["candidates"]["a"]["pearson"] - 1.0) < 1e-12
    print("self-test: OK")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--preds", action="append",
                        help="name=path，可重复；第一个作为 delta 基线")
    parser.add_argument("--pairs")
    parser.add_argument("--sources", help="与 pairs 等长的分组 JSON")
    parser.add_argument("--group-key", default="dataset")
    parser.add_argument("--pred-path", default="modes.f32.sts.preds")
    parser.add_argument("--out")
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()

    if args.self_test:
        self_test()
        return 0

    if not args.preds or not args.pairs:
        parser.error("--preds 和 --pairs 必填（--self-test 除外）")

    labels = get_labels(Path(args.pairs))
    preds = {}
    for spec in args.preds:
        if "=" not in spec:
            raise SystemExit(f"--preds 需要 name=path：{spec!r}")
        name, path = spec.split("=", 1)
        preds[name] = get_preds(json.loads(Path(path).read_text(encoding="utf-8")),
                                args.pred_path)
    if any(len(v) != len(labels) for v in preds.values()):
        raise SystemExit("preds 与 labels 长度不一致")

    groups = load_groups(Path(args.sources), args.group_key) if args.sources else None
    rows = evaluate(preds, labels, groups)
    base_name = next(iter(preds))
    for row in rows:
        base = row["candidates"][base_name]
        for name, value in row["candidates"].items():
            value["delta_vs_base"] = {
                key: value[key] - base[key] for key in ("pearson", "spearman")
            }

    out = {
        "base": base_name,
        "n": len(labels),
        "pred_path": args.pred_path,
        "groups": rows,
    }
    if args.out:
        Path(args.out).write_text(json.dumps(out, ensure_ascii=False, indent=2),
                                  encoding="utf-8")
        print(f"wrote {args.out}: {len(rows)} groups")
    else:
        print(json.dumps(out, ensure_ascii=False, indent=2))
    return 0


if __name__ == "__main__":
    sys.exit(main())

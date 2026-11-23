#!/usr/bin/env python3
"""WSP-BUDGET 配对 bootstrap：同字节分配是否真的优于纯 int3。

输入是 bert6 `TRIT_DUMP_PREDS=1` 产出的两个 JSON，以及带 `pairs[].score` 的标签文件。
只做统计，不重新跑模型；默认 10,000 次按 pair 重采样，同时报 Pearson/Spearman
差值的 95% 区间和“候选更好”的比例。

  python scripts/wsp_budget_bootstrap.py \
    --baseline l12-int3-preds-full.json \
    --candidate l12-budget-v6e-preds-full.json \
    --pairs pairs.json --out bootstrap-v6e.json
"""
from __future__ import annotations

import argparse
import json
import math
import random
from pathlib import Path


def get_preds(obj: dict, path: str) -> list[float]:
    node = obj
    for part in path.split("."):
        node = node[part]
    if not isinstance(node, list) or not node:
        raise SystemExit(f"pred path {path!r} 不是非空列表")
    return [float(v) for v in node]


def get_labels(path: Path) -> list[float]:
    obj = json.loads(path.read_text(encoding="utf-8"))
    pairs = obj.get("pairs")
    if not isinstance(pairs, list) or not pairs:
        raise SystemExit(f"{path} 没有非空 pairs 列表")
    return [float(p["score"]) for p in pairs]


def pearson(x: list[float], y: list[float]) -> float:
    n = len(x)
    mx = math.fsum(x) / n
    my = math.fsum(y) / n
    dx = [v - mx for v in x]
    dy = [v - my for v in y]
    sx = math.sqrt(math.fsum(v * v for v in dx))
    sy = math.sqrt(math.fsum(v * v for v in dy))
    if sx == 0.0 or sy == 0.0:
        return float("nan")
    return math.fsum(dx[i] * dy[i] for i in range(n)) / (sx * sy)


def rank(values: list[float]) -> list[float]:
    order = sorted(range(len(values)), key=lambda i: values[i])
    out = [0.0] * len(values)
    i = 0
    while i < len(order):
        j = i + 1
        while j < len(order) and values[order[j]] == values[order[i]]:
            j += 1
        avg = (i + j - 1) / 2.0 + 1.0
        for k in range(i, j):
            out[order[k]] = avg
        i = j
    return out


def spearman(x: list[float], y: list[float]) -> float:
    return pearson(rank(x), rank(y))


def summarize(diffs: list[float]) -> dict:
    diffs = sorted(diffs)
    n = len(diffs)
    return {
        "mean": math.fsum(diffs) / n,
        "ci95_low": diffs[int(0.025 * n)],
        "ci95_high": diffs[int(0.975 * n) - 1],
        "candidate_better_frac": sum(v > 0 for v in diffs) / n,
    }


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--baseline", required=True)
    parser.add_argument("--candidate", required=True)
    parser.add_argument("--pairs", required=True)
    parser.add_argument("--out", required=True)
    parser.add_argument("--pred-path", default="modes.f32.sts.preds")
    parser.add_argument("--iters", type=int, default=10000)
    parser.add_argument("--seed", type=int, default=20261124)
    args = parser.parse_args()

    base = get_preds(json.loads(Path(args.baseline).read_text(encoding="utf-8")), args.pred_path)
    cand = get_preds(json.loads(Path(args.candidate).read_text(encoding="utf-8")), args.pred_path)
    labels = get_labels(Path(args.pairs))
    if len(base) != len(cand) or len(base) != len(labels):
        raise SystemExit(f"长度不一致: base={len(base)} cand={len(cand)} labels={len(labels)}")

    out = {
        "baseline": Path(args.baseline).name,
        "candidate": Path(args.candidate).name,
        "n": len(base),
        "iters": args.iters,
        "seed": args.seed,
        "baseline_metrics": {"pearson": pearson(base, labels), "spearman": spearman(base, labels)},
        "candidate_metrics": {"pearson": pearson(cand, labels), "spearman": spearman(cand, labels)},
    }
    out["delta"] = {
        k: out["candidate_metrics"][k] - out["baseline_metrics"][k] for k in ("pearson", "spearman")
    }

    rng = random.Random(args.seed)
    dp: list[float] = []
    ds: list[float] = []
    n = len(base)
    for _ in range(args.iters):
        idx = [rng.randrange(n) for _ in range(n)]
        xa = [base[i] for i in idx]
        xb = [cand[i] for i in idx]
        yy = [labels[i] for i in idx]
        dp.append(pearson(xb, yy) - pearson(xa, yy))
        ds.append(spearman(xb, yy) - spearman(xa, yy))

    out["bootstrap"] = {"pearson": summarize(dp), "spearman": summarize(ds)}
    Path(args.out).parent.mkdir(parents=True, exist_ok=True)
    Path(args.out).write_text(json.dumps(out, ensure_ascii=False, indent=2), encoding="utf-8")
    print(json.dumps(out, ensure_ascii=False, indent=2))


if __name__ == "__main__":
    main()

"""S6.3 评估：按类生成 vs 真实同类分布（数据准备/分析，不参与训练）。

用法：py -3.11 scripts/eval_s63.py <真实 look.csv> <labels.tsv> <生成 look.csv> <输出 csv>
  1) 把 true 类按 assetId 前缀绑到文件名；
  2) 读 `trit-look look` 产出的两份 CSV（真实 / 生成）——这里只做连接与统计；
  3) 输出每类均值 + 跨类 Spearman 秩相关（生成 vs 真实）。

注意：生成样本必须用同一 eval-seed、同一权重，否则不可比。
"""
from __future__ import annotations

import csv
import os
import statistics
import sys
from collections import defaultdict


def load_labels(path: str) -> dict[str, str]:
    m = {}
    with open(path, encoding="utf-8") as f:
        for line in f:
            parts = line.rstrip("\n").split("\t")
            if len(parts) >= 2:
                m[parts[0]] = parts[1]
    return m


def class_of(stem: str, labels: dict[str, str]) -> str | None:
    best = None
    for aid, cls in labels.items():
        if stem.startswith(aid) and (best is None or len(aid) > len(best[0])):
            best = (aid, cls)
    return best[1] if best else None


def spearman(a: list[float], b: list[float]) -> float:
    def rank(v: list[float]) -> list[float]:
        order = sorted(range(len(v)), key=lambda i: v[i])
        r = [0.0] * len(v)
        i = 0
        while i < len(order):
            j = i
            while j + 1 < len(order) and v[order[j + 1]] == v[order[i]]:
                j += 1
            avg = (i + j) / 2.0 + 1.0
            for k in range(i, j + 1):
                r[order[k]] = avg
            i = j + 1
        return r

    ra, rb = rank(a), rank(b)
    n = len(a)
    if n < 3:
        return 0.0
    ma, mb = sum(ra) / n, sum(rb) / n
    num = sum((x - ma) * (y - mb) for x, y in zip(ra, rb))
    da = sum((x - ma) ** 2 for x in ra) ** 0.5
    db = sum((y - mb) ** 2 for y in rb) ** 0.5
    return num / (da * db) if da * db else 0.0


def read_look(path: str) -> dict[str, dict[str, float]]:
    out = {}
    with open(path, encoding="utf-8") as f:
        for row in csv.DictReader(f):
            out[row["name"]] = {k: float(v) for k, v in row.items() if k != "name"}
    return out


def main() -> int:
    if len(sys.argv) < 5:
        print(__doc__)
        return 2
    real_csv, labels_tsv, gen_csv, out_csv = sys.argv[1:5]
    labels = load_labels(labels_tsv)

    real = read_look(real_csv)
    gen = read_look(gen_csv)

    per_real: dict[str, list[dict[str, float]]] = defaultdict(list)
    for name, r in real.items():
        c = class_of(name, labels)
        if c:
            per_real[c].append(r)
    per_gen: dict[str, list[dict[str, float]]] = defaultdict(list)
    for name, r in gen.items():
        # 生成样本名形如 gen_<class>_<idx>；类别写在文件名里
        stem = name.split("__")[0]
        c = stem.rsplit("_", 1)[-1]
        per_gen[c].append(r)

    keys = ["stroke", "coherence", "contrast", "levels", "hf1"]
    print(f"{'class':10s} {'n_real':>6s} {'n_gen':>5s} " + " ".join(f"{k:>10s}" for k in keys))
    real_means: dict[str, list[float]] = {k: [] for k in keys}
    gen_means: dict[str, list[float]] = {k: [] for k in keys}
    classes = sorted(set(per_real) & set(per_gen))
    for c in classes:
        rm = [statistics.mean(r[k] for r in per_real[c]) for k in keys]
        gm = [statistics.mean(r[k] for r in per_gen[c]) for k in keys]
        print(
            f"{c:10s} {len(per_real[c]):6d} {len(per_gen[c]):5d} "
            + " ".join(f"{gm[i]:6.2f}/{rm[i]:<6.2f}" for i in range(len(keys)))
        )
        for i, k in enumerate(keys):
            real_means[k].append(rm[i])
            gen_means[k].append(gm[i])

    print("\n跨类 Spearman（生成 vs 真实；每列一个指标）：")
    rows = []
    for k in keys:
        rho = spearman(gen_means[k], real_means[k])
        rows.append((k, rho))
        print(f"  {k:10s} rho={rho:+.2f}")
    with open(out_csv, "w", encoding="utf-8", newline="") as f:
        w = csv.writer(f)
        w.writerow(["metric", "spearman", "classes"])
        for k, rho in rows:
            w.writerow([k, f"{rho:.3f}", ";".join(classes)])
    print(f"\n写出 {out_csv}")
    return 0


if __name__ == "__main__":
    sys.exit(main())

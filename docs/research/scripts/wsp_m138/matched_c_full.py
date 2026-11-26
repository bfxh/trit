"""WSP-BUDGET matched-C 全矩阵：只用真 f32 的内层 5 折选 C，量化臂共用。

路径由 `cv_seed_stability.py` 的 `TRIT_RESULTS` / `TRIT_GEOM_OUT` 环境变量控制；
默认 `./trit-results` 与 `<TRIT_RESULTS>/geometry/corrected`。
"""

import argparse
import json
from pathlib import Path

import numpy as np
from sklearn.linear_model import LogisticRegression
from sklearn.metrics import accuracy_score, f1_score
from sklearn.model_selection import StratifiedKFold

import cv_seed_stability as css

OUT = css.OUT
GRID = css.GRID


def score(y_true, pred, metric):
    if metric == "accuracy":
        return float(accuracy_score(y_true, pred))
    return float(f1_score(y_true, pred, average="macro", zero_division=0))


def select_c_f32(x, y, train, probe, balanced, metric, folds):
    x_train = x[train]
    y_train = y[train]
    scores = []
    for c in GRID:
        vals = []
        for tr, va in folds:
            clf = css.make_clf(probe, c, balanced)
            clf.fit(x_train[tr], y_train[tr])
            vals.append(score(y_train[va], clf.predict(x_train[va]), metric))
        scores.append((float(np.mean(vals)), c))
    return max(scores)[1]


def fit_at_c(x, y, train, test, probe, balanced, c):
    clf = css.make_clf(probe, c, balanced)
    clf.fit(x[train], y[train])
    return clf.predict(x[test])


def boot(pred_a, pred_b, y, metric, iters=10000):
    rng = np.random.default_rng(7)
    n = len(y)
    diffs = []
    for _ in range(iters):
        idx = rng.integers(0, n, n)
        if metric == "accuracy":
            a = float((pred_a[idx] == y[idx]).mean())
            b = float((pred_b[idx] == y[idx]).mean())
        else:
            a = float(f1_score(y[idx], pred_a[idx], average="macro", zero_division=0))
            b = float(f1_score(y[idx], pred_b[idx], average="macro", zero_division=0))
        diffs.append(a - b)
    return float(np.mean(diffs)), [float(np.percentile(diffs, 2.5)), float(np.percentile(diffs, 97.5))]


def run_case(dataset, depth, seeds):
    cfg = css.CASES[(dataset, depth)]
    texts = json.loads(Path(cfg["texts"]).read_text(encoding="utf-8"))["texts"]
    y = np.array([r["label"] for r in texts])
    train = np.array([i for i, r in enumerate(texts) if r["split"] == "train"])
    test = np.array([i for i, r in enumerate(texts) if r["split"] == "test"])
    arms = {}
    for name, path in cfg["arms"].items():
        x = np.load(path) if isinstance(path, Path) else css.load(path)
        if len(x) != len(texts):
            raise ValueError(f"{dataset}/{depth}/{name}: {len(x)} != {len(texts)}")
        arms[name] = x

    rows = []
    for probe in ("logreg", "linear_svc"):
        for seed in seeds:
            skf = StratifiedKFold(n_splits=5, shuffle=True, random_state=seed)
            folds = list(skf.split(np.zeros(len(train)), y[train]))
            c = select_c_f32(
                arms["true_f32"], y, train, probe, cfg["balanced"],
                cfg["metric"], folds,
            )
            pred = {}
            for name, x in arms.items():
                pred[name] = fit_at_c(
                    x, y, train, test, probe, cfg["balanced"], c,
                )
            for name in arms:
                if name == "true_f32":
                    continue
                mean, ci = boot(pred[name], pred["true_f32"], y[test], cfg["metric"])
                point = score(y[test], pred[name], cfg["metric"]) - score(
                    y[test], pred["true_f32"], cfg["metric"],
                )
                verdict = "positive" if ci[0] > 0 else "negative" if ci[1] < 0 else "cross"
                rows.append({
                    "dataset": dataset, "depth": depth, "probe": probe,
                    "arm": name, "metric": cfg["metric"], "seed": seed,
                    "C": c, "point_delta": point, "boot_mean": mean,
                    "ci_low": ci[0], "ci_high": ci[1], "verdict": verdict,
                })
                print(dataset, depth, probe, seed, name,
                      "C", c, "point", round(point, 6), "boot", round(mean, 6),
                      "CI", [round(v, 6) for v in ci], verdict, flush=True)
    dst = OUT / f"matched-c-{dataset}-{depth}.json"
    dst.write_text(json.dumps({"seeds": seeds, "rows": rows}, indent=2), encoding="utf-8")
    print("wrote", dst, len(rows), flush=True)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--dataset", required=True)
    ap.add_argument("--depth", required=True)
    ap.add_argument("--seeds", default="7,13,19,31,43")
    args = ap.parse_args()
    run_case(args.dataset, args.depth, [int(s) for s in args.seeds.split(",")])


if __name__ == "__main__":
    main()

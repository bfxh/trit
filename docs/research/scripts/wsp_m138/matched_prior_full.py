"""M1.38 目标先验阈值全臂矩阵：matched-C + `default` / `target_prior` 双规则。

每 case 2 probe ×5 seed ×2 规则 ×3 量化臂 = 60 行；只用未标注 test 分数
取分位，不用 test 标签。路径由 `cv_seed_stability.py` 的环境变量控制。
"""

import argparse
import json
from pathlib import Path

import numpy as np
from sklearn.model_selection import StratifiedKFold

import cv_seed_stability as css
from matched_c_full import boot, score, select_c_f32

OUT = css.OUT
SEEDS = [7, 13, 19, 31, 43]


def fit_at_c(x, y, train, probe, balanced, c):
    clf = css.make_clf(probe, c, balanced)
    clf.fit(x[train], y[train])
    return clf


def scores_of(clf, x, probe):
    if probe == "logreg":
        return clf.predict_proba(x)[:, 1]
    return clf.decision_function(x)


def run_case(dataset, depth, seeds):
    cfg = css.CASES[(dataset, depth)]
    rows = json.loads(Path(cfg["texts"]).read_text(encoding="utf-8"))["texts"]
    y = np.array([r["label"] for r in rows])
    train = np.array([i for i, r in enumerate(rows) if r["split"] == "train"])
    test = np.array([i for i, r in enumerate(rows) if r["split"] == "test"])
    arms = {}
    for name, path in cfg["arms"].items():
        x = np.load(path) if isinstance(path, Path) else css.load(path)
        if len(x) != len(rows):
            raise ValueError(f"{dataset}/{depth}/{name}: {len(x)} != {len(rows)}")
        arms[name] = x
    prior = float(y[train].mean())
    out = []

    for probe in ("logreg", "linear_svc"):
        for seed in seeds:
            skf = StratifiedKFold(n_splits=5, shuffle=True, random_state=seed)
            folds = list(skf.split(np.zeros(len(train)), y[train]))
            c = select_c_f32(arms["true_f32"], y, train, probe, cfg["balanced"], cfg["metric"], folds)
            fitted, scores = {}, {}
            for name, x in arms.items():
                clf = fit_at_c(x, y, train, probe, cfg["balanced"], c)
                fitted[name] = clf
                scores[name] = scores_of(clf, x[test], probe)

            for rule in ("default", "target_prior"):
                pred = {}
                thr = {}
                for name in arms:
                    if rule == "default":
                        pred[name] = fitted[name].predict(arms[name][test])
                        thr[name] = 0.5 if probe == "logreg" else 0.0
                    else:
                        t = float(np.quantile(scores[name], 1 - prior))
                        pred[name] = (scores[name] >= t).astype(int)
                        thr[name] = t
                for name in arms:
                    if name == "true_f32":
                        continue
                    mean, ci = boot(pred[name], pred["true_f32"], y[test], cfg["metric"])
                    point = score(y[test], pred[name], cfg["metric"]) - score(y[test], pred["true_f32"], cfg["metric"])
                    verdict = "positive" if ci[0] > 0 else "negative" if ci[1] < 0 else "cross"
                    out.append({
                        "dataset": dataset, "depth": depth, "probe": probe, "arm": name,
                        "metric": cfg["metric"], "seed": seed, "rule": rule,
                        "C": c, "threshold": thr[name], "target_prior": prior,
                        "pred_pos_rate": float(np.mean(pred[name])),
                        "f32_score": score(y[test], pred["true_f32"], cfg["metric"]),
                        "arm_score": score(y[test], pred[name], cfg["metric"]),
                        "point_delta": float(point), "boot_mean": float(mean),
                        "ci_low": float(ci[0]), "ci_high": float(ci[1]), "verdict": verdict,
                    })
                    print(dataset, depth, probe, seed, rule, name, "C", c,
                          "thr", round(float(thr[name]), 6), "point", round(float(point), 6),
                          "CI", [round(float(v), 6) for v in ci], verdict, flush=True)
    dst = OUT / f"matched-prior-{dataset}-{depth}.json"
    dst.write_text(json.dumps({"seeds": seeds, "rows": out}, indent=2), encoding="utf-8")
    print("wrote", dst, len(out), flush=True)


if __name__ == "__main__":
    ap = argparse.ArgumentParser()
    ap.add_argument("--dataset", required=True)
    ap.add_argument("--depth", required=True)
    ap.add_argument("--seeds", default="7,13,19,31,43")
    args = ap.parse_args()
    run_case(args.dataset, args.depth, [int(s) for s in args.seeds.split(",")])

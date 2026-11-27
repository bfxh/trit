"""M1.39 预注册 confirmatory：TweetEval/hate test 的 50/50 分层半分割。

口径见 `docs/research/data/wsp-classify-m139-prereg.json`；本脚本只执行，不改判据。

环境变量：
  TRIT_RESULTS   默认 `./trit-results`
  TRIT_GEOM_OUT  默认 `<TRIT_RESULTS>/geometry/corrected`
  TRIT_DATA_DIR  默认仓库 `docs/research/data`

输出：
  `<TRIT_GEOM_OUT>/m139-split-half.json`
  `docs/research/data/wsp-classify-m139-raw.tsv`（320 行）
  `docs/research/data/wsp-classify-m139-summary.tsv`（32 行）
  `docs/research/data/wsp-classify-m139-primary.tsv`（4 行）
"""

import argparse
import csv
import json
import os
import sys
from pathlib import Path

import numpy as np
from sklearn.model_selection import StratifiedKFold

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent / "wsp_m138"))

import cv_seed_stability as css  # noqa: E402
from matched_c_full import boot, score, select_c_f32  # noqa: E402

RESULTS = Path(os.environ.get("TRIT_RESULTS", "trit-results"))
OUT = Path(os.environ.get("TRIT_GEOM_OUT", str(RESULTS / "geometry" / "corrected")))
REPO_ROOT = HERE.parents[3]
DATA = Path(os.environ.get("TRIT_DATA_DIR", str(REPO_ROOT / "docs" / "research" / "data")))
SEEDS = [7, 13, 19, 31, 43]
SPLIT_SEED = 101
METRIC = "accuracy"


def fit_at_c(x, y, train, probe, balanced, c):
    clf = css.make_clf(probe, c, balanced)
    clf.fit(x[train], y[train])
    return clf


def scores_of(clf, x, probe):
    if probe == "logreg":
        return clf.predict_proba(x)[:, 1]
    return clf.decision_function(x)


def macro_f1_binary(y, pred):
    tp = float(((pred == 1) & (y == 1)).sum())
    fp = float(((pred == 1) & (y == 0)).sum())
    fn = float(((pred == 0) & (y == 1)).sum())
    tn = float(((pred == 0) & (y == 0)).sum())
    pos = 2 * tp / (2 * tp + fp + fn) if (2 * tp + fp + fn) else 0.0
    neg = 2 * tn / (2 * tn + fn + fp) if (2 * tn + fn + fp) else 0.0
    return (pos + neg) / 2


def boot_macro(pred_a, pred_b, y, iters=2000):
    rng = np.random.default_rng(7)
    n = len(y)
    diffs = []
    for _ in range(iters):
        idx = rng.integers(0, n, n)
        diffs.append(macro_f1_binary(y[idx], pred_a[idx]) - macro_f1_binary(y[idx], pred_b[idx]))
    return float(np.mean(diffs)), [float(np.percentile(diffs, 2.5)), float(np.percentile(diffs, 97.5))]


def run_case(dataset, depth, seeds, split_seed=SPLIT_SEED, metric=None, tag=""):
    cfg = css.CASES[(dataset, depth)]
    metric = metric or cfg["metric"]
    rows = json.loads(Path(cfg["texts"]).read_text(encoding="utf-8"))["texts"]
    y = np.array([r["label"] for r in rows])
    train = np.array([i for i, r in enumerate(rows) if r["split"] == "train"])
    test = np.array([i for i, r in enumerate(rows) if r["split"] == "test"])

    arms = {}
    # M1.38 的 L12 case 把精确 int4 存成 `true_bits4`；这里统一别名，避免多出一个臂。
    alias = {"true_bits4": "int4"}
    for name, path in cfg["arms"].items():
        name = alias.get(name, name)
        x = np.load(path) if isinstance(path, Path) else css.load(path)
        if len(x) != len(rows):
            raise ValueError(f"{dataset}/{depth}/{name}: {len(x)} != {len(rows)}")
        arms[name] = x

    prior = float(y[train].mean())
    skf = StratifiedKFold(n_splits=2, shuffle=True, random_state=split_seed)
    split_a, split_b = next(skf.split(np.zeros(len(test)), y[test]))
    directions = {
        "A_to_B": (split_a, split_b),
        "B_to_A": (split_b, split_a),
    }

    out = []
    for probe in ("logreg", "linear_svc"):
        for seed in seeds:
            train_folds = list(StratifiedKFold(5, shuffle=True, random_state=seed).split(
                np.zeros(len(train)), y[train]
            ))
            # C 始终按 f32 的 cfg 主指标（accuracy）选；macro-F1 只换评分函数，不换 C。
            c = select_c_f32(arms["true_f32"], y, train, probe, cfg["balanced"], cfg["metric"], train_folds)
            fitted, scores = {}, {}
            for name, x in arms.items():
                clf = fit_at_c(x, y, train, probe, cfg["balanced"], c)
                fitted[name] = clf
                scores[name] = scores_of(clf, x[test], probe)

            for rule in ("target_prior", "default"):
                for direction, (calib, evalt) in directions.items():
                    y_eval = y[test[evalt]]
                    pred = {}
                    thr = {}
                    for name in arms:
                        if rule == "default":
                            pred[name] = fitted[name].predict(arms[name][test])[evalt]
                            thr[name] = 0.5 if probe == "logreg" else 0.0
                        else:
                            t = float(np.quantile(scores[name][calib], 1 - prior))
                            pred[name] = (scores[name][evalt] >= t).astype(int)
                            thr[name] = t
                    base = pred["true_f32"]
                    for name in arms:
                        if name == "true_f32":
                            continue
                        mean, ci = (boot_macro(pred[name], base, y_eval)
                                    if metric == "macro_f1"
                                    else boot(pred[name], base, y_eval, metric))
                        point = score(y_eval, pred[name], metric) - score(y_eval, base, metric)
                        verdict = "positive" if ci[0] > 0 else "negative" if ci[1] < 0 else "cross"
                        out.append({
                            "dataset": dataset, "depth": depth, "probe": probe, "arm": name,
                            "metric": metric, "seed": seed, "rule": rule,
                            "direction": direction, "C": c, "target_prior": prior,
                            "threshold": float(thr[name]), "eval_n": int(len(evalt)),
                            "eval_pos_rate": float(y_eval.mean()),
                            "calib_pos_rate": float(y[test[calib]].mean()),
                            "f32_score": score(y_eval, base, metric),
                            "arm_score": score(y_eval, pred[name], metric),
                            "point_delta": float(point), "boot_mean": float(mean),
                            "ci_low": float(ci[0]), "ci_high": float(ci[1]), "verdict": verdict,
                        })
                        print(dataset, depth, probe, seed, rule, direction, name,
                              "C", c, "thr", round(float(thr[name]), 6),
                              "point", round(float(point), 6),
                              "CI", [round(float(v), 6) for v in ci], verdict, flush=True)

    dst = OUT / f"m139-split-half{tag}-{depth}.json"
    dst.write_text(json.dumps({"seeds": seeds, "split_seed": split_seed, "rows": out}, indent=2),
                   encoding="utf-8")
    return out


def write_tables(rows, suffix="", primary=True):
    DATA.mkdir(parents=True, exist_ok=True)
    raw_fields = ["dataset", "depth", "probe", "arm", "metric", "seed", "rule", "direction",
                  "C", "target_prior", "threshold", "eval_n", "eval_pos_rate", "calib_pos_rate",
                  "f32_score", "arm_score", "point_delta", "boot_mean", "ci_low", "ci_high", "verdict"]
    with (DATA / f"wsp-classify-m139-raw{suffix}.tsv").open("w", encoding="utf-8", newline="") as f:
        w = csv.DictWriter(f, fieldnames=raw_fields, delimiter="\t", lineterminator="\n")
        w.writeheader()
        for r in sorted(rows, key=lambda x: (x["dataset"], x["depth"], x["probe"], x["arm"],
                                             x["rule"], x["direction"], x["seed"])):
            w.writerow({k: (f"{r[k]:.6f}" if isinstance(r[k], float) else r[k]) for k in raw_fields})

    groups = {}
    for r in rows:
        groups.setdefault((r["dataset"], r["depth"], r["probe"], r["arm"], r["metric"], r["rule"]), []).append(r)
    summary = []
    for key, vals in sorted(groups.items()):
        points = np.array([v["point_delta"] for v in vals], dtype=float)
        summary.append({
            "dataset": key[0], "depth": key[1], "probe": key[2], "arm": key[3], "metric": key[4], "rule": key[5],
            "n_eval_ci": len(vals), "n_pos_ci": sum(v["verdict"] == "positive" for v in vals),
            "n_neg_ci": sum(v["verdict"] == "negative" for v in vals),
            "n_cross_ci": sum(v["verdict"] == "cross" for v in vals),
            "n_pos_point": int((points > 0).sum()),
            "mean_delta": f"{points.mean():.6f}", "sd_delta": f"{points.std(ddof=1):.6f}",
            "min_delta": f"{points.min():.6f}", "max_delta": f"{points.max():.6f}",
        })
    fields = ["dataset", "depth", "probe", "arm", "metric", "rule", "n_eval_ci", "n_pos_ci",
              "n_neg_ci", "n_cross_ci", "n_pos_point", "mean_delta", "sd_delta", "min_delta", "max_delta"]
    with (DATA / f"wsp-classify-m139-summary{suffix}.tsv").open("w", encoding="utf-8", newline="") as f:
        w = csv.DictWriter(f, fieldnames=fields, delimiter="\t", lineterminator="\n")
        w.writeheader(); w.writerows(summary)

    if not primary:
        return summary, []

    primary = []
    for item in (("hate", "l12", "r12"), ("hate", "l6", "rescan")):
        for probe in ("logreg", "linear_svc"):
            match = [s for s in summary if (s["dataset"], s["depth"], s["probe"], s["arm"], s["rule"])
                     == (item[0], item[1], probe, item[2], "target_prior")]
            if not match:
                continue
            s = match[0]
            n_pos = int(s["n_pos_ci"])
            verdict = "confirmed" if n_pos >= 8 else "rejected" if n_pos <= 3 else "inconclusive"
            primary.append({
                "dataset": s["dataset"], "depth": s["depth"], "probe": s["probe"], "arm": s["arm"],
                "n_pos_ci": n_pos, "n_eval_ci": s["n_eval_ci"], "mean_delta": s["mean_delta"],
                "sd_delta": s["sd_delta"], "min_delta": s["min_delta"], "max_delta": s["max_delta"],
                "verdict": verdict,
            })
    pfields = ["dataset", "depth", "probe", "arm", "n_pos_ci", "n_eval_ci", "mean_delta",
               "sd_delta", "min_delta", "max_delta", "verdict"]
    with (DATA / f"wsp-classify-m139-primary{suffix}.tsv").open("w", encoding="utf-8", newline="") as f:
        w = csv.DictWriter(f, fieldnames=pfields, delimiter="\t", lineterminator="\n")
        w.writeheader(); w.writerows(primary)
    return summary, primary


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--dataset", default="hate")
    ap.add_argument("--depth", choices=["l12", "l6", "both"], default="both")
    ap.add_argument("--seeds", default="7,13,19,31,43")
    ap.add_argument("--split-seed", type=int, default=SPLIT_SEED)
    ap.add_argument("--metric", choices=["accuracy", "macro_f1"], default=None)
    args = ap.parse_args()
    seeds = [int(s) for s in args.seeds.split(",")]
    depths = ["l12", "l6"] if args.depth == "both" else [args.depth]
    suffix = "" if args.metric in (None, "accuracy") else "-macro"
    rows = []
    for depth in depths:
        rows.extend(run_case(args.dataset, depth, seeds, args.split_seed, args.metric, suffix))
    combined = OUT / f"m139-split-half{suffix}.json"
    combined.write_text(json.dumps({"seeds": seeds, "split_seed": args.split_seed, "rows": rows},
                                   indent=2), encoding="utf-8")
    summary, primary = write_tables(rows, suffix=suffix, primary=(suffix == ""))
    print("summary", len(summary), "primary", len(primary), flush=True)


if __name__ == "__main__":
    main()

"""M1.42 TweetEval/hate 的 fit-only 阈值搬运测试。

规则：default、global_test_prior、global_fit_prior。
所有阈值不使用 test 标签；test 标签只用于 accuracy/macro-F1。
"""

import argparse
import glob
import json
import struct
from pathlib import Path

import numpy as np
from sklearn.linear_model import LogisticRegression
from sklearn.metrics import accuracy_score, f1_score
from sklearn.model_selection import StratifiedKFold
from sklearn.svm import LinearSVC

SEEDS = [7, 13, 19, 31, 43]
GRID = (0.01, 0.1, 1.0, 10.0)


def read_emb(path):
    with open(path, "rb") as fh:
        head = fh.read(16)
        rows, cols = struct.unpack("<II", head[8:])
        data = np.fromfile(fh, dtype=np.float32).reshape(rows, cols)
    return data


def load(pattern):
    parts = [read_emb(p) for p in sorted(glob.glob(pattern))]
    if not parts:
        raise FileNotFoundError(pattern)
    return np.concatenate(parts, 0)


def score(y, pred, metric):
    if metric == "accuracy":
        return float(accuracy_score(y, pred))
    return float(f1_score(y, pred, average="macro", zero_division=0))


def make_clf(probe, c):
    if probe == "logreg":
        return LogisticRegression(C=c, max_iter=5000, solver="lbfgs", random_state=7)
    return LinearSVC(C=c, max_iter=10000, random_state=7, tol=1e-4)


def get_scores(clf, x, probe):
    if probe == "logreg":
        return clf.predict_proba(x)[:, 1]
    return clf.decision_function(x)


def select_c(x, y, train, probe, folds):
    x_train, y_train = x[train], y[train]
    vals = []
    for c in GRID:
        fold_scores = []
        for tr, va in folds:
            clf = make_clf(probe, c)
            clf.fit(x_train[tr], y_train[tr])
            fold_scores.append(score(y_train[va], clf.predict(x_train[va]), "accuracy"))
        vals.append((float(np.mean(fold_scores)), c))
    return max(vals)[1]


def threshold(rule, scores_test, scores_oof, prior, probe):
    if rule == "default":
        return 0.5 if probe == "logreg" else 0.0
    if rule == "global_test_prior":
        return float(np.quantile(scores_test, 1.0 - prior))
    if rule == "global_fit_prior":
        return float(np.quantile(scores_oof, 1.0 - prior))
    raise ValueError(rule)


def boot(pred_a, pred_b, y, iters, seed):
    rng = np.random.default_rng(seed)
    n = len(y)
    diffs = []
    for _ in range(iters):
        idx = rng.integers(0, n, n)
        diffs.append(float((pred_a[idx] == y[idx]).mean() - (pred_b[idx] == y[idx]).mean()))
    return float(np.mean(diffs)), (float(np.percentile(diffs, 2.5)),
                                   float(np.percentile(diffs, 97.5)))


def add_compare(rows, kind, depth, probe, arm, rule, rule_a, rule_b, metric,
                seed, c, y, pred_a, pred_b, iters):
    point = score(y, pred_a, metric) - score(y, pred_b, metric)
    if metric == "accuracy":
        mean, ci = boot(pred_a, pred_b, y, iters, seed)
    else:
        mean, ci = float("nan"), (float("nan"), float("nan"))
    rows.append({
        "kind": kind, "depth": depth, "probe": probe, "arm": arm, "rule": rule,
        "rule_a": rule_a, "rule_b": rule_b, "metric": metric, "seed": seed,
        "C": c, "n": len(y), "score_a": score(y, pred_a, metric),
        "score_b": score(y, pred_b, metric), "point_delta": point,
        "boot_mean": mean, "ci_low": ci[0], "ci_high": ci[1],
        "verdict": "point" if metric != "accuracy" else (
            "positive" if ci[0] > 0 else "negative" if ci[1] < 0 else "cross"),
    })


def run_depth(depth, arms, y, train, test, metrics, comparisons, iters):
    prior = float(y[train].mean())
    for probe in ("logreg", "linear_svc"):
        for seed in SEEDS:
            skf = StratifiedKFold(n_splits=5, shuffle=True, random_state=seed)
            folds = list(skf.split(np.zeros(len(train)), y[train]))
            c = select_c(arms["f32"], y, train, probe, folds)
            test_scores, oof_scores = {}, {}
            for name, x in arms.items():
                clf = make_clf(probe, c)
                clf.fit(x[train], y[train])
                test_scores[name] = get_scores(clf, x[test], probe)
                oof = np.zeros(len(train), dtype=np.float64)
                x_train, y_train = x[train], y[train]
                for tr, va in folds:
                    fold_clf = make_clf(probe, c)
                    fold_clf.fit(x_train[tr], y_train[tr])
                    oof[va] = get_scores(fold_clf, x_train[va], probe)
                oof_scores[name] = oof
            print(depth, probe, "seed", seed, "C", c, flush=True)

            rules = ("default", "global_test_prior", "global_fit_prior")
            preds = {}
            for name in arms:
                for rule in rules:
                    th = threshold(rule, test_scores[name], oof_scores[name], prior, probe)
                    pred = (test_scores[name] >= th).astype(int)
                    preds[(name, rule)] = pred
                    for met in ("accuracy", "macro_f1"):
                        metrics.append({
                            "depth": depth, "probe": probe, "arm": name, "rule": rule,
                            "metric": met, "seed": seed, "C": c, "n": len(test),
                            "score": score(y[test], pred, met),
                            "pred_pos_rate": float(np.mean(pred)),
                            "threshold": th, "prior_fit": prior,
                        })

            for name in arms:
                for rule_a, rule_b in (("global_fit_prior", "global_test_prior"),
                                       ("global_fit_prior", "default"),
                                       ("global_test_prior", "default")):
                    for met in ("accuracy", "macro_f1"):
                        add_compare(
                            comparisons, "rule_diff", depth, probe, name, "",
                            rule_a, rule_b, met, seed, c, y[test],
                            preds[(name, rule_a)], preds[(name, rule_b)], iters,
                        )
            for name in arms:
                if name == "f32":
                    continue
                for rule in rules:
                    add_compare(
                        comparisons, "arm_minus_f32", depth, probe, name, rule,
                        f"f32:{rule}", "f32", "accuracy", seed, c, y[test],
                        preds[(name, rule)], preds[("f32", rule)], iters,
                    )


def write_tsv(path, rows, fields):
    with open(path, "w", encoding="utf-8", newline="") as fh:
        fh.write("\t".join(fields) + "\n")
        for r in rows:
            fh.write("\t".join(str(r.get(k, "")) for k in fields) + "\n")


def primary_table(comparisons):
    out = []
    for depth in ("l12", "l6"):
        for probe in ("logreg", "linear_svc"):
            vals = [r for r in comparisons
                    if r["kind"] == "arm_minus_f32" and r["depth"] == depth
                    and r["probe"] == probe and r["metric"] == "accuracy"
                    and r["rule"] == "global_fit_prior"]
            positive = sum(v["verdict"] == "positive" for v in vals)
            mean_point = float(np.mean([v["point_delta"] for v in vals]))
            mean_boot = float(np.mean([v["boot_mean"] for v in vals]))
            if positive >= 4 and mean_point > 0:
                verdict = "retained"
            elif positive <= 1:
                verdict = "not_retained"
            else:
                verdict = "inconclusive"
            out.append({
                "depth": depth, "probe": probe, "positive": positive,
                "n": len(vals), "mean_point": mean_point, "mean_boot": mean_boot,
                "verdict": verdict,
            })
    return out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--root", default=r"D:\trit-results\tweet-hate")
    ap.add_argument("--texts", default="")
    ap.add_argument("--out", default="")
    ap.add_argument("--iters", type=int, default=10000)
    args = ap.parse_args()
    root = Path(args.root)
    texts_path = Path(args.texts) if args.texts else root / "texts-l12.json"
    out = Path(args.out) if args.out else root
    rows = json.loads(texts_path.read_text(encoding="utf-8"))["texts"]
    y = np.array([r["label"] for r in rows], dtype=np.int64)
    train = np.array([i for i, r in enumerate(rows) if r["split"] == "train"])
    test = np.array([i for i, r in enumerate(rows) if r["split"] == "test"])
    print("texts", len(rows), "fit", len(train), "test", len(test), flush=True)

    l12 = {
        "f32": load(str(root / "emb12" / "l12-pure-f32-*.f32")),
        "r12": load(str(root / "emb12" / "l12-r12-*.f32")),
    }
    l6 = {
        "f32": load(str(root / "emb6" / "l6-pure-f32-*.f32")),
        "rescan": load(str(root / "emb6" / "l6-rescan-*.f32")),
    }
    for depth, arms in (("l12", l12), ("l6", l6)):
        for name, x in arms.items():
            if len(x) != len(rows):
                raise ValueError(f"{depth}/{name}: {len(x)} != {len(rows)}")
        print(depth, {k: v.shape for k, v in arms.items()}, flush=True)

    metrics, comparisons = [], []
    run_depth("l12", l12, y, train, test, metrics, comparisons, args.iters)
    run_depth("l6", l6, y, train, test, metrics, comparisons, args.iters)

    metric_fields = ["depth", "probe", "arm", "rule", "metric", "seed", "C", "n",
                     "score", "pred_pos_rate", "threshold", "prior_fit"]
    write_tsv(out / "wsp-classify-m142-raw.tsv", metrics, metric_fields)

    compare_fields = ["kind", "depth", "probe", "arm", "rule", "rule_a", "rule_b",
                      "metric", "seed", "C", "n", "score_a", "score_b", "point_delta",
                      "boot_mean", "ci_low", "ci_high", "verdict"]
    write_tsv(out / "wsp-classify-m142-compare.tsv", comparisons, compare_fields)

    primary = primary_table(comparisons)
    primary_fields = ["depth", "probe", "positive", "n", "mean_point", "mean_boot", "verdict"]
    write_tsv(out / "wsp-classify-m142-primary.tsv", primary, primary_fields)

    (out / "wsp-classify-m142.json").write_text(
        json.dumps({"seeds": SEEDS, "rows": metrics, "comparisons": comparisons,
                    "primary": primary}, indent=2), encoding="utf-8")
    print(json.dumps(primary, indent=2), flush=True)


if __name__ == "__main__":
    main()

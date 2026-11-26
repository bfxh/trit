"""M1.40 HateXplain 目标先验 confirmatory 分析。

主口径：
  - fit = official train + val；eval = official test，n=1,918。
  - 每个 seed 只用 f32 的 fit 内 5 折选 C；f32 与量化臂共用 C。
  - primary：full test 的 target_prior accuracy，量化臂减 f32，10k paired
    bootstrap 95% CI；5 seeds 全部记录。
  - secondary：macro-F1、default 0.5、test 半分割（校准/评估分开）。
  - 不加载 test 标签做 C 选择或阈值；target_prior 只用未标注 test 分数。

输出：
  <out>/wsp-classify-m140-raw.tsv
  <out>/wsp-classify-m140-summary.tsv
  <out>/wsp-classify-m140-primary.tsv
  <out>/wsp-classify-m140-split-half.tsv
  <out>/wsp-classify-m140.json
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


def row_record(scope, direction, depth, probe, arm, rule, metric, seed, c,
               n, f32_score, arm_score, point, boot_mean, ci, pred_pos_rate,
               threshold, prior):
    return {
        "scope": scope, "direction": direction, "depth": depth, "probe": probe,
        "arm": arm, "rule": rule, "metric": metric, "seed": seed, "C": c,
        "n": n, "f32_score": f32_score, "arm_score": arm_score,
        "point_delta": point, "boot_mean": boot_mean,
        "ci_low": ci[0], "ci_high": ci[1],
        "pred_pos_rate": pred_pos_rate, "threshold": threshold, "target_prior": prior,
        "verdict": "positive" if ci[0] > 0 else "negative" if ci[1] < 0 else "cross",
    }


def add_comparison(rows, scope, direction, depth, probe, arm, rule, metric, seed, c,
                   pred_arm, pred_f32, y_eval, threshold, prior):
    n = len(y_eval)
    for met in ("accuracy", "macro_f1"):
        mean, ci = boot(pred_arm, pred_f32, y_eval, met)
        point = score(y_eval, pred_arm, met) - score(y_eval, pred_f32, met)
        rows.append(row_record(
            scope, direction, depth, probe, arm, rule, met, seed, c, n,
            score(y_eval, pred_f32, met), score(y_eval, pred_arm, met),
            point, mean, ci, float(np.mean(pred_arm)), threshold, prior,
        ))


def run_depth(depth, texts, arms, y, train, test, rows):
    prior = float(y[train].mean())
    for probe in ("logreg", "linear_svc"):
        for seed in SEEDS:
            skf = StratifiedKFold(n_splits=5, shuffle=True, random_state=seed)
            folds = list(skf.split(np.zeros(len(train)), y[train]))
            c = select_c(arms["f32"], y, train, probe, folds)
            fitted, scores = {}, {}
            for name, x in arms.items():
                clf = make_clf(probe, c)
                clf.fit(x[train], y[train])
                fitted[name] = clf
                scores[name] = get_scores(clf, x[test], probe)
            print(depth, probe, "seed", seed, "C", c, flush=True)

            # full test: default and target_prior
            for rule in ("default", "target_prior"):
                for name in ("r12", "rescan"):
                    if name not in arms:
                        continue
                    if rule == "default":
                        pred_arm = fitted[name].predict(arms[name][test])
                        pred_f32 = fitted["f32"].predict(arms["f32"][test])
                        thr_arm = 0.5 if probe == "logreg" else 0.0
                        thr_f32 = 0.5 if probe == "logreg" else 0.0
                    else:
                        thr_arm = float(np.quantile(scores[name], 1 - prior))
                        thr_f32 = float(np.quantile(scores["f32"], 1 - prior))
                        pred_arm = (scores[name] >= thr_arm).astype(int)
                        pred_f32 = (scores["f32"] >= thr_f32).astype(int)
                    add_comparison(
                        rows, "full", "full", depth, probe, name, rule, "both", seed, c,
                        pred_arm, pred_f32, y[test], thr_arm, prior,
                    )

            # secondary: test half-split, target_prior only, calibration/eval separated
            skf_half = StratifiedKFold(n_splits=2, shuffle=True, random_state=101)
            for half_id, (calib, eval_idx) in enumerate(skf_half.split(np.zeros(len(test)), y[test])):
                direction = "A_calib_B_eval" if half_id == 0 else "B_calib_A_eval"
                for name in ("r12", "rescan"):
                    if name not in arms:
                        continue
                    thr_arm = float(np.quantile(scores[name][calib], 1 - prior))
                    thr_f32 = float(np.quantile(scores["f32"][calib], 1 - prior))
                    pred_arm = (scores[name][eval_idx] >= thr_arm).astype(int)
                    pred_f32 = (scores["f32"][eval_idx] >= thr_f32).astype(int)
                    add_comparison(
                        rows, "split_half", direction, depth, probe, name, "target_prior",
                        "both", seed, c, pred_arm, pred_f32, y[test][eval_idx],
                        thr_arm, prior,
                    )
                    print(depth, probe, "seed", seed, direction, name,
                          "pos", round(float(pred_arm.mean()), 4), flush=True)


def summarize(rows):
    groups = {}
    for r in rows:
        key = (r["scope"], r["direction"], r["depth"], r["probe"], r["arm"], r["rule"], r["metric"])
        groups.setdefault(key, []).append(r)
    out = []
    for key, vals in sorted(groups.items()):
        out.append({
            "scope": key[0], "direction": key[1], "depth": key[2], "probe": key[3],
            "arm": key[4], "rule": key[5], "metric": key[6], "n_cells": len(vals),
            "positive": sum(v["verdict"] == "positive" for v in vals),
            "negative": sum(v["verdict"] == "negative" for v in vals),
            "cross": sum(v["verdict"] == "cross" for v in vals),
            "mean_point": float(np.mean([v["point_delta"] for v in vals])),
            "mean_boot": float(np.mean([v["boot_mean"] for v in vals])),
            "min_ci_low": float(np.min([v["ci_low"] for v in vals])),
            "max_ci_high": float(np.max([v["ci_high"] for v in vals])),
        })
    return out


def candidate_verdict(primary_rows):
    # primary_rows: full/target_prior/accuracy only
    by = {}
    for r in primary_rows:
        by.setdefault((r["depth"], r["probe"]), []).append(r)
    out = []
    for depth in ("l12", "l6"):
        candidate = "R12" if depth == "l12" else "rescan"
        counts = {}
        for probe in ("logreg", "linear_svc"):
            vals = by.get((depth, probe), [])
            counts[probe] = {
                "positive": sum(v["verdict"] == "positive" for v in vals),
                "n": len(vals),
                "mean_point": float(np.mean([v["point_delta"] for v in vals])) if vals else 0.0,
                "mean_boot": float(np.mean([v["boot_mean"] for v in vals])) if vals else 0.0,
            }
            out.append({
                "candidate": candidate, "depth": depth, "probe": probe,
                "positive": counts[probe]["positive"], "n": counts[probe]["n"],
                "mean_point": counts[probe]["mean_point"],
                "mean_boot": counts[probe]["mean_boot"],
            })
        good = [p for p in ("logreg", "linear_svc")
                if counts[p]["positive"] >= 4 and counts[p]["mean_point"] > 0]
        partial = [p for p in ("logreg", "linear_svc")
                   if counts[p]["positive"] >= 4 and counts[p]["mean_point"] > 0]
        low = [p for p in ("logreg", "linear_svc") if counts[p]["positive"] >= 2]
        if len(good) == 2:
            verdict = "confirmed"
        elif len(partial) == 1 and len(low) == 2:
            verdict = "partially_confirmed"
        elif counts["logreg"]["positive"] <= 1 and counts["linear_svc"]["positive"] <= 1:
            verdict = "rejected"
        else:
            verdict = "inconclusive"
        for r in out:
            if r["candidate"] == candidate:
                r["candidate_verdict"] = verdict
    return out


def write_vs_default(rows, out):
    idx = {}
    for r in rows:
        if r["scope"] == "full" and r["metric"] == "accuracy":
            idx[(r["depth"], r["probe"], r["seed"], r["arm"], r["rule"])] = r
    out_rows = []
    for depth, arm in (("l12", "r12"), ("l6", "rescan")):
        for probe in ("logreg", "linear_svc"):
            for seed in SEEDS:
                t = idx[(depth, probe, seed, arm, "target_prior")]
                d = idx[(depth, probe, seed, arm, "default")]
                out_rows.append({
                    "depth": depth, "probe": probe, "arm": arm, "seed": seed,
                    "C_target": t["C"], "C_default": d["C"],
                    "target_arm": t["arm_score"], "default_arm": d["arm_score"],
                    "arm_rule_delta": t["arm_score"] - d["arm_score"],
                    "target_f32": t["f32_score"], "default_f32": d["f32_score"],
                    "f32_rule_delta": t["f32_score"] - d["f32_score"],
                })
    fields = ["depth", "probe", "arm", "seed", "C_target", "C_default",
              "target_arm", "default_arm", "arm_rule_delta",
              "target_f32", "default_f32", "f32_rule_delta"]
    write_tsv(out / "wsp-classify-m140-vs-default.tsv", out_rows, fields)


def write_tsv(path, rows, fields):
    with open(path, "w", encoding="utf-8", newline="") as fh:
        fh.write("\t".join(fields) + "\n")
        for r in rows:
            fh.write("\t".join(str(r.get(k, "")) for k in fields) + "\n")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--root", default=r"D:\trit-results\hatexplain")
    ap.add_argument("--texts", default="")
    ap.add_argument("--out", default="")
    args = ap.parse_args()
    root = Path(args.root)
    texts_path = Path(args.texts) if args.texts else root / "texts-l12.json"
    out = Path(args.out) if args.out else root
    rows = json.loads(texts_path.read_text(encoding="utf-8"))["texts"]
    y = np.array([r["label"] for r in rows])
    train = np.array([i for i, r in enumerate(rows) if r["split"] == "train"])
    test = np.array([i for i, r in enumerate(rows) if r["split"] == "test"])
    if len(rows) != 19188:
        raise ValueError(f"unexpected texts: {len(rows)}")
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

    all_rows = []
    run_depth("l12", rows, l12, y, train, test, all_rows)
    run_depth("l6", rows, l6, y, train, test, all_rows)

    raw_fields = ["scope", "direction", "depth", "probe", "arm", "rule", "metric", "seed",
                  "C", "n", "f32_score", "arm_score", "point_delta", "boot_mean",
                  "ci_low", "ci_high", "pred_pos_rate", "threshold", "target_prior", "verdict"]
    write_tsv(out / "wsp-classify-m140-raw.tsv", all_rows, raw_fields)
    write_vs_default(all_rows, out)

    summary = summarize(all_rows)
    summary_fields = ["scope", "direction", "depth", "probe", "arm", "rule", "metric",
                      "n_cells", "positive", "negative", "cross", "mean_point", "mean_boot",
                      "min_ci_low", "max_ci_high"]
    write_tsv(out / "wsp-classify-m140-summary.tsv", summary, summary_fields)

    primary_rows = [r for r in all_rows
                    if r["scope"] == "full" and r["rule"] == "target_prior"
                    and r["metric"] == "accuracy"]
    primary = candidate_verdict(primary_rows)
    primary_fields = ["candidate", "depth", "probe", "positive", "n", "mean_point",
                      "mean_boot", "candidate_verdict"]
    write_tsv(out / "wsp-classify-m140-primary.tsv", primary, primary_fields)

    half_rows = [r for r in all_rows
                 if r["scope"] == "split_half" and r["metric"] == "accuracy"]
    half_fields = ["direction", "depth", "probe", "arm", "seed", "C", "n",
                   "point_delta", "boot_mean", "ci_low", "ci_high", "verdict"]
    write_tsv(out / "wsp-classify-m140-split-half.tsv", half_rows, half_fields)

    (out / "wsp-classify-m140.json").write_text(
        json.dumps({"seeds": SEEDS, "rows": all_rows, "summary": summary, "primary": primary},
                   indent=2), encoding="utf-8")
    print(json.dumps(primary, indent=2), flush=True)


if __name__ == "__main__":
    main()

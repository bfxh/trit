"""M1.41 HateXplain 域感知先验与 fit-only 阈值分析。

预注册见 docs/research/data/wsp-classify-m141-prereg.json。

规则：
  default                 0.5 (logreg) / 0.0 (SVC)
  global_test_prior       all test scores 分位点，prior_fit_global
  domain_test_prior       每域 test scores 分位点，prior_fit_domain
  global_fit_prior        all OOF fit scores 分位点，prior_fit_global
  domain_fit_prior        每域 OOF fit scores 分位点，prior_fit_domain

所有阈值不使用 test 标签；test 标签只用于 accuracy/macro-F1 评估。
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
DOMAINS = ("gab", "twitter")


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


def load_metadata(root, texts):
    raw = {}
    for split in ("train", "test"):
        path = root / "raw" / f"{split}.jsonl"
        raw[split] = [json.loads(line) for line in
                      path.read_text(encoding="utf-8").splitlines()]
    sources, label3 = [], []
    for r in texts:
        prefix, local = str(r["id"]).rsplit("-", 1)
        if prefix != r["split"]:
            raise ValueError(f"id/split mismatch for {r['id']}")
        m = raw[prefix][int(local)]
        if int(m["label"]) != int(r["label"]):
            raise ValueError(f"label mismatch for {r['id']}")
        sources.append(str(m["source"]))
        label3.append(str(m["label3"]))
    return np.array(sources), np.array(label3)


def quantile_threshold(scores, prior):
    return float(np.quantile(scores, 1.0 - prior))


def thresholds_for(rule, scores_test, scores_oof, source_test, source_train,
                   prior_global, prior_domain, probe):
    if rule == "default":
        t = 0.5 if probe == "logreg" else 0.0
        return t, t
    if rule == "global_test_prior":
        t = quantile_threshold(scores_test, prior_global)
        return t, t
    if rule == "domain_test_prior":
        return tuple(quantile_threshold(scores_test[source_test == d], prior_domain[d])
                     for d in DOMAINS)
    if rule == "global_fit_prior":
        t = quantile_threshold(scores_oof, prior_global)
        return t, t
    if rule == "domain_fit_prior":
        return tuple(quantile_threshold(scores_oof[source_train == d], prior_domain[d])
                     for d in DOMAINS)
    raise ValueError(rule)


def pred_for_scope(rule, scores, source, thresholds, scope):
    tg, tt = thresholds
    if rule in ("domain_test_prior", "domain_fit_prior"):
        return np.where(source == "gab", scores >= tg, scores >= tt).astype(int)
    return (scores >= tg).astype(int)


def metric_row(scope, depth, probe, arm, rule, metric, seed, c, y_s, pred,
               thresholds, prior_global, prior_domain):
    return {
        "scope": scope, "depth": depth, "probe": probe, "arm": arm, "rule": rule,
        "metric": metric, "seed": seed, "C": c, "n": len(y_s),
        "score": score(y_s, pred, metric),
        "pred_pos_rate": float(np.mean(pred)),
        "threshold_gab": thresholds[0], "threshold_twitter": thresholds[1],
        "prior_gab": prior_domain["gab"], "prior_twitter": prior_domain["twitter"],
        "prior_global": prior_global,
    }


def bootstrap_accuracy(pred_a, pred_b, y, groups, scope, iters, seed):
    rng = np.random.default_rng(seed)
    if scope == "overall":
        gab = np.where(groups == "gab")[0]
        twitter = np.where(groups == "twitter")[0]
        n_gab, n_tw = len(gab), len(twitter)

        def draw():
            return np.concatenate([
                rng.choice(gab, n_gab, replace=True),
                rng.choice(twitter, n_tw, replace=True),
            ])
    else:
        pool = np.where(groups == scope)[0]
        n = len(pool)

        def draw():
            return rng.choice(pool, n, replace=True)

    diffs = []
    for _ in range(iters):
        idx = draw()
        diffs.append(float((pred_a[idx] == y[idx]).mean() - (pred_b[idx] == y[idx]).mean()))
    return float(np.mean(diffs)), (float(np.percentile(diffs, 2.5)),
                                   float(np.percentile(diffs, 97.5)))


def add_compare(rows, kind, scope, depth, probe, arm, rule, rule_a, rule_b, metric,
                seed, c, y_s, pred_a, pred_b, groups, iters):
    point = score(y_s, pred_a, metric) - score(y_s, pred_b, metric)
    if metric == "accuracy":
        mean, ci = bootstrap_accuracy(pred_a, pred_b, y_s, groups, scope, iters, seed)
    else:
        mean, ci = float("nan"), (float("nan"), float("nan"))
    rows.append({
        "kind": kind, "scope": scope, "depth": depth, "probe": probe,
        "arm": arm, "rule": rule, "rule_a": rule_a, "rule_b": rule_b,
        "metric": metric, "seed": seed, "C": c, "n": len(y_s),
        "score_a": score(y_s, pred_a, metric), "score_b": score(y_s, pred_b, metric),
        "point_delta": point, "boot_mean": mean, "ci_low": ci[0], "ci_high": ci[1],
        "verdict": "point" if metric != "accuracy" else (
            "positive" if ci[0] > 0 else "negative" if ci[1] < 0 else "cross"),
    })


def run_depth(depth, arms, y, source, train, test, prior_global, prior_domain,
              metric_rows, compare_rows, iters):
    test_gab = np.where(source[test] == "gab")[0]
    test_tw = np.where(source[test] == "twitter")[0]
    scopes = {
        "overall": np.arange(len(test)),
        "gab": test_gab,
        "twitter": test_tw,
    }
    for probe in ("logreg", "linear_svc"):
        for seed in SEEDS:
            skf = StratifiedKFold(n_splits=5, shuffle=True, random_state=seed)
            folds = list(skf.split(np.zeros(len(train)), y[train]))
            c = select_c(arms["f32"], y, train, probe, folds)
            fitted, test_scores, oof_scores = {}, {}, {}
            for name, x in arms.items():
                clf = make_clf(probe, c)
                clf.fit(x[train], y[train])
                fitted[name] = clf
                test_scores[name] = get_scores(clf, x[test], probe)
                oof = np.zeros(len(train), dtype=np.float64)
                x_train = x[train]
                y_train = y[train]
                for tr, va in folds:
                    fold_clf = make_clf(probe, c)
                    fold_clf.fit(x_train[tr], y_train[tr])
                    oof[va] = get_scores(fold_clf, x_train[va], probe)
                oof_scores[name] = oof
            print(depth, probe, "seed", seed, "C", c, flush=True)

            rules = ("default", "global_test_prior", "domain_test_prior",
                     "global_fit_prior", "domain_fit_prior")
            preds = {}
            for name in arms:
                for rule in rules:
                    th = thresholds_for(rule, test_scores[name], oof_scores[name],
                                        source[test], source[train], prior_global,
                                        prior_domain, probe)
                    preds[(name, rule)] = {
                        s: pred_for_scope(rule, test_scores[name], source[test], th, s)[idx]
                        for s, idx in scopes.items()
                    }
                    for s, idx in scopes.items():
                        y_s = y[test][idx]
                        for met in ("accuracy", "macro_f1"):
                            metric_rows.append(metric_row(
                                s, depth, probe, name, rule, met, seed, c, y_s,
                                preds[(name, rule)][s], th, prior_global, prior_domain,
                            ))

            # Rule comparisons, all arms, all scopes.
            pairs = (
                ("domain_test_prior", "global_test_prior"),
                ("domain_test_prior", "default"),
                ("domain_fit_prior", "domain_test_prior"),
                ("global_test_prior", "default"),
                ("domain_fit_prior", "default"),
            )
            for name in arms:
                for rule_a, rule_b in pairs:
                    for s, idx in scopes.items():
                        y_s = y[test][idx]
                        g_s = source[test][idx]
                        for met in ("accuracy", "macro_f1"):
                            add_compare(
                                compare_rows, "rule_diff", s, depth, probe, name,
                                "", rule_a, rule_b, met, seed, c, y_s,
                                preds[(name, rule_a)][s], preds[(name, rule_b)][s],
                                g_s, iters,
                            )

            # Quantized arm minus f32, under each rule.
            for name in arms:
                if name == "f32":
                    continue
                for rule in rules:
                    for s, idx in scopes.items():
                        y_s = y[test][idx]
                        g_s = source[test][idx]
                        add_compare(
                            compare_rows, "arm_minus_f32", s, depth, probe, name,
                            rule, f"f32:{rule}", f"f32", "accuracy", seed, c, y_s,
                            preds[(name, rule)][s], preds[("f32", rule)][s], g_s, iters,
                        )


def write_tsv(path, rows, fields):
    with open(path, "w", encoding="utf-8", newline="") as fh:
        fh.write("\t".join(fields) + "\n")
        for r in rows:
            fh.write("\t".join(str(r.get(k, "")) for k in fields) + "\n")


def summarize_metrics(rows):
    groups = {}
    for r in rows:
        key = (r["scope"], r["depth"], r["probe"], r["arm"], r["rule"], r["metric"])
        groups.setdefault(key, []).append(r)
    out = []
    for key, vals in sorted(groups.items()):
        out.append({
            "scope": key[0], "depth": key[1], "probe": key[2], "arm": key[3],
            "rule": key[4], "metric": key[5], "n_seeds": len(vals),
            "mean_score": float(np.mean([v["score"] for v in vals])),
            "mean_pred_pos_rate": float(np.mean([v["pred_pos_rate"] for v in vals])),
        })
    return out


def summarize_compare(rows):
    groups = {}
    for r in rows:
        key = (r["kind"], r["scope"], r["depth"], r["probe"], r["arm"], r["rule"],
               r["rule_a"], r["rule_b"], r["metric"])
        groups.setdefault(key, []).append(r)
    out = []
    for key, vals in sorted(groups.items()):
        out.append({
            "kind": key[0], "scope": key[1], "depth": key[2], "probe": key[3],
            "arm": key[4], "rule": key[5], "rule_a": key[6], "rule_b": key[7],
            "metric": key[8], "n_cells": len(vals),
            "positive": sum(v["verdict"] == "positive" for v in vals),
            "negative": sum(v["verdict"] == "negative" for v in vals),
            "cross": sum(v["verdict"] == "cross" for v in vals),
            "point": sum(v["verdict"] == "point" for v in vals),
            "mean_point": float(np.nanmean([v["point_delta"] for v in vals])),
            "mean_boot": float(np.nanmean([v["boot_mean"] for v in vals])),
        })
    return out


def primary_table(compare_rows):
    out = []
    for depth in ("l12", "l6"):
        for probe in ("logreg", "linear_svc"):
            vals = [r for r in compare_rows
                    if r["kind"] == "rule_diff" and r["scope"] == "overall"
                    and r["depth"] == depth and r["probe"] == probe
                    and r["arm"] == "f32" and r["metric"] == "accuracy"
                    and r["rule_a"] == "domain_test_prior"
                    and r["rule_b"] == "global_test_prior"]
            positive = sum(v["verdict"] == "positive" for v in vals)
            mean_point = float(np.mean([v["point_delta"] for v in vals]))
            mean_boot = float(np.mean([v["boot_mean"] for v in vals]))
            if positive >= 4 and mean_point > 0:
                verdict = "supported"
            elif positive <= 1:
                verdict = "rejected"
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
    ap.add_argument("--root", default=r"D:\trit-results\hatexplain")
    ap.add_argument("--texts", default="")
    ap.add_argument("--out", default="")
    ap.add_argument("--iters", type=int, default=10000)
    args = ap.parse_args()
    root = Path(args.root)
    texts_path = Path(args.texts) if args.texts else root / "texts-l12.json"
    out = Path(args.out) if args.out else root
    rows = json.loads(texts_path.read_text(encoding="utf-8"))["texts"]
    y = np.array([r["label"] for r in rows], dtype=np.int64)
    source, _ = load_metadata(root, rows)
    train = np.array([i for i, r in enumerate(rows) if r["split"] == "train"])
    test = np.array([i for i, r in enumerate(rows) if r["split"] == "test"])
    if len(rows) != 19188:
        raise ValueError(f"unexpected texts: {len(rows)}")
    if set(np.unique(source[train])) != set(DOMAINS):
        raise ValueError(f"unexpected fit domains: {np.unique(source[train])}")
    if set(np.unique(source[test])) != set(DOMAINS):
        raise ValueError(f"unexpected test domains: {np.unique(source[test])}")

    prior_global = float(y[train].mean())
    prior_domain = {d: float(y[train][source[train] == d].mean()) for d in DOMAINS}
    print("texts", len(rows), "fit", len(train), "test", len(test), flush=True)
    print("prior_global", prior_global, "prior_domain", prior_domain, flush=True)
    print("fit_sources", {d: int((source[train] == d).sum()) for d in DOMAINS}, flush=True)
    print("test_sources", {d: int((source[test] == d).sum()) for d in DOMAINS}, flush=True)

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
    run_depth("l12", l12, y, source, train, test, prior_global, prior_domain,
              metrics, comparisons, args.iters)
    run_depth("l6", l6, y, source, train, test, prior_global, prior_domain,
              metrics, comparisons, args.iters)

    metric_fields = ["scope", "depth", "probe", "arm", "rule", "metric", "seed", "C",
                     "n", "score", "pred_pos_rate", "threshold_gab", "threshold_twitter",
                     "prior_gab", "prior_twitter", "prior_global"]
    write_tsv(out / "wsp-classify-m141-raw.tsv", metrics, metric_fields)

    compare_fields = ["kind", "scope", "depth", "probe", "arm", "rule", "rule_a",
                      "rule_b", "metric", "seed", "C", "n", "score_a", "score_b",
                      "point_delta", "boot_mean", "ci_low", "ci_high", "verdict"]
    write_tsv(out / "wsp-classify-m141-compare.tsv", comparisons, compare_fields)

    ms = summarize_metrics(metrics)
    ms_fields = ["scope", "depth", "probe", "arm", "rule", "metric", "n_seeds",
                 "mean_score", "mean_pred_pos_rate"]
    write_tsv(out / "wsp-classify-m141-summary.tsv", ms, ms_fields)

    cs = summarize_compare(comparisons)
    cs_fields = ["kind", "scope", "depth", "probe", "arm", "rule", "rule_a", "rule_b",
                 "metric", "n_cells", "positive", "negative", "cross", "point",
                 "mean_point", "mean_boot"]
    write_tsv(out / "wsp-classify-m141-compare-summary.tsv", cs, cs_fields)

    primary = primary_table(comparisons)
    primary_fields = ["depth", "probe", "positive", "n", "mean_point", "mean_boot", "verdict"]
    write_tsv(out / "wsp-classify-m141-primary.tsv", primary, primary_fields)

    (out / "wsp-classify-m141.json").write_text(
        json.dumps({"seeds": SEEDS, "rows": metrics, "comparisons": comparisons,
                    "summary": ms, "compare_summary": cs, "primary": primary},
                   indent=2), encoding="utf-8")
    print(json.dumps(primary, indent=2), flush=True)


if __name__ == "__main__":
    main()

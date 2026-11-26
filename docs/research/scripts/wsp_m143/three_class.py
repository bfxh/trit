"""M1.43 HateXplain 三分类探索读数。"""

import argparse
import glob
import json
import struct
from pathlib import Path

import numpy as np
from sklearn.linear_model import LogisticRegression
from sklearn.metrics import accuracy_score, f1_score, recall_score
from sklearn.model_selection import StratifiedKFold
from sklearn.svm import LinearSVC

SEEDS = [7, 13, 19, 31, 43]
GRID = (0.01, 0.1, 1.0, 10.0)
LABELS = ("normal", "hatespeech", "offensive")


def read_emb(path):
    with open(path, "rb") as fh:
        head = fh.read(16)
        rows, cols = struct.unpack("<II", head[8:])
        return np.fromfile(fh, dtype=np.float32).reshape(rows, cols)


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


def boot(pred_a, pred_b, y, iters, seed):
    rng = np.random.default_rng(seed)
    n = len(y)
    diffs = []
    for _ in range(iters):
        idx = rng.integers(0, n, n)
        diffs.append(float((pred_a[idx] == y[idx]).mean() - (pred_b[idx] == y[idx]).mean()))
    return float(np.mean(diffs)), (float(np.percentile(diffs, 2.5)),
                                   float(np.percentile(diffs, 97.5)))


def load_labels(root, texts):
    raw = {}
    for split in ("train", "test"):
        path = root / "raw" / f"{split}.jsonl"
        raw[split] = [json.loads(line) for line in
                      path.read_text(encoding="utf-8").splitlines()]
    labels = []
    for r in texts:
        prefix, local = str(r["id"]).rsplit("-", 1)
        m = raw[prefix][int(local)]
        if str(m["label3"]) not in LABELS:
            raise ValueError(f"unknown label3 {m['label3']!r}")
        labels.append(LABELS.index(str(m["label3"])))
    return np.array(labels, dtype=np.int64)


def run_depth(depth, arms, y, train, test, metrics, comparisons, iters, probes,
              fixed_c=None):
    for probe in probes:
        for seed in SEEDS:
            if fixed_c is None:
                skf = StratifiedKFold(n_splits=5, shuffle=True, random_state=seed)
                folds = list(skf.split(np.zeros(len(train)), y[train]))
                c = select_c(arms["f32"], y, train, probe, folds)
            else:
                c = fixed_c
            preds = {}
            for name, x in arms.items():
                clf = make_clf(probe, c)
                clf.fit(x[train], y[train])
                preds[name] = clf.predict(x[test])
            print(depth, probe, "seed", seed, "C", c, flush=True)

            for name, pred in preds.items():
                rec = recall_score(y[test], pred, labels=[0, 1, 2],
                                   average=None, zero_division=0)
                metrics.append({
                    "depth": depth, "probe": probe, "arm": name, "metric": "accuracy",
                    "seed": seed, "C": c, "n": len(test),
                    "score": score(y[test], pred, "accuracy"),
                    "macro_f1": score(y[test], pred, "macro_f1"),
                    "recall_normal": float(rec[0]),
                    "recall_hatespeech": float(rec[1]),
                    "recall_offensive": float(rec[2]),
                })
            for name in arms:
                if name == "f32":
                    continue
                mean, ci = boot(preds[name], preds["f32"], y[test], iters, seed)
                point = score(y[test], preds[name], "accuracy") - score(
                    y[test], preds["f32"], "accuracy")
                comparisons.append({
                    "depth": depth, "probe": probe, "arm": name, "metric": "accuracy",
                    "seed": seed, "C": c, "n": len(test),
                    "score_arm": score(y[test], preds[name], "accuracy"),
                    "score_f32": score(y[test], preds["f32"], "accuracy"),
                    "point_delta": point, "boot_mean": mean,
                    "ci_low": ci[0], "ci_high": ci[1],
                    "verdict": "positive" if ci[0] > 0 else "negative" if ci[1] < 0 else "cross",
                })
                comparisons.append({
                    "depth": depth, "probe": probe, "arm": name, "metric": "macro_f1",
                    "seed": seed, "C": c, "n": len(test),
                    "score_arm": score(y[test], preds[name], "macro_f1"),
                    "score_f32": score(y[test], preds["f32"], "macro_f1"),
                    "point_delta": score(y[test], preds[name], "macro_f1") - score(
                        y[test], preds["f32"], "macro_f1"),
                    "boot_mean": float("nan"),
                    "ci_low": float("nan"), "ci_high": float("nan"),
                    "verdict": "point",
                })


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
                    if r["depth"] == depth and r["probe"] == probe
                    and r["metric"] == "accuracy"]
            out.append({
                "depth": depth, "probe": probe,
                "positive": sum(v["verdict"] == "positive" for v in vals),
                "n": len(vals),
                "mean_point": float(np.mean([v["point_delta"] for v in vals])),
                "mean_boot": float(np.mean([v["boot_mean"] for v in vals])),
                "note": "descriptive only",
            })
    return out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--root", default=r"D:\trit-results\hatexplain")
    ap.add_argument("--texts", default="")
    ap.add_argument("--out", default="")
    ap.add_argument("--iters", type=int, default=10000)
    ap.add_argument("--depth", choices=("l12", "l6", "both"), default="both")
    ap.add_argument("--probe", choices=("logreg", "linear_svc", "both"), default="both")
    ap.add_argument("--suffix", default="")
    ap.add_argument("--fixed-c", type=float, default=None,
                    help="skip per-seed 5-fold C selection and use this C")
    args = ap.parse_args()
    root = Path(args.root)
    texts_path = Path(args.texts) if args.texts else root / "texts-l12.json"
    out = Path(args.out) if args.out else root
    suffix = args.suffix
    probes = ("logreg", "linear_svc") if args.probe == "both" else (args.probe,)
    rows = json.loads(texts_path.read_text(encoding="utf-8"))["texts"]
    y = load_labels(root, rows)
    train = np.array([i for i, r in enumerate(rows) if r["split"] == "train"])
    test = np.array([i for i, r in enumerate(rows) if r["split"] == "test"])
    if len(rows) != 19188:
        raise ValueError(f"unexpected texts: {len(rows)}")
    print("texts", len(rows), "fit", len(train), "test", len(test), flush=True)
    print("fit3", np.bincount(y[train], minlength=3).tolist(),
          "test3", np.bincount(y[test], minlength=3).tolist(), flush=True)

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
    if args.depth in ("l12", "both"):
        run_depth("l12", l12, y, train, test, metrics, comparisons, args.iters,
                  probes, args.fixed_c)
    if args.depth in ("l6", "both"):
        run_depth("l6", l6, y, train, test, metrics, comparisons, args.iters,
                  probes, args.fixed_c)

    metric_fields = ["depth", "probe", "arm", "metric", "seed", "C", "n", "score",
                     "macro_f1", "recall_normal", "recall_hatespeech", "recall_offensive"]
    write_tsv(out / f"wsp-classify-m143-raw{suffix}.tsv", metrics, metric_fields)
    compare_fields = ["depth", "probe", "arm", "metric", "seed", "C", "n", "score_arm",
                      "score_f32", "point_delta", "boot_mean", "ci_low", "ci_high", "verdict"]
    write_tsv(out / f"wsp-classify-m143-compare{suffix}.tsv", comparisons, compare_fields)
    primary = primary_table(comparisons)
    primary_fields = ["depth", "probe", "positive", "n", "mean_point", "mean_boot", "note"]
    write_tsv(out / f"wsp-classify-m143-primary{suffix}.tsv", primary, primary_fields)
    (out / f"wsp-classify-m143{suffix}.json").write_text(
        json.dumps({"seeds": SEEDS, "rows": metrics, "comparisons": comparisons,
                    "primary": primary}, indent=2), encoding="utf-8")
    print(json.dumps(primary, indent=2), flush=True)


if __name__ == "__main__":
    main()

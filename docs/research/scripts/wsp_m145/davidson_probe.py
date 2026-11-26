"""M1.45: Davidson third-corpus descriptive transport.

Default threshold only (logreg 0.5 / LinearSVC 0). f32 selects C inside fit
with 5-fold CV; the quantized arm shares the same C. 5 seeds, paired 10k
test-level bootstrap. This is descriptive: no candidate confirm/reject.
"""

from __future__ import annotations

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
DEPTH_ARMS = {
    "l12": ("f32", "r12"),
    "l6": ("f32", "rescan"),
}


def read_emb(path: str) -> np.ndarray:
    with open(path, "rb") as fh:
        head = fh.read(16)
        rows, cols = struct.unpack("<II", head[8:])
        return np.fromfile(fh, dtype=np.float32).reshape(rows, cols)


def load(pattern: str) -> np.ndarray:
    parts = [read_emb(p) for p in sorted(glob.glob(pattern))]
    if not parts:
        raise FileNotFoundError(pattern)
    return np.concatenate(parts, 0)


def make_clf(probe: str, c: float):
    if probe == "logreg":
        return LogisticRegression(C=c, max_iter=5000, solver="lbfgs", random_state=7)
    return LinearSVC(C=c, max_iter=10000, random_state=7, tol=1e-4)


def score(y, pred, metric: str) -> float:
    if metric == "accuracy":
        return float(accuracy_score(y, pred))
    return float(f1_score(y, pred, average="macro", zero_division=0))


def select_c(x, y, train, probe, folds) -> float:
    vals = []
    for c in GRID:
        fold_scores = []
        for tr, va in folds:
            clf = make_clf(probe, c)
            clf.fit(x[train][tr], y[train][tr])
            fold_scores.append(score(y[train][va], clf.predict(x[train][va]), "accuracy"))
        vals.append((float(np.mean(fold_scores)), c))
    return max(vals)[1]


def predict(clf, x, probe: str):
    if probe == "logreg":
        return (clf.predict_proba(x)[:, 1] >= 0.5).astype(np.int64)
    return (clf.decision_function(x) >= 0.0).astype(np.int64)


def boot(pred_a, pred_b, y, iters: int, seed: int, metric: str = "accuracy"):
    rng = np.random.default_rng(seed)
    n = len(y)
    diffs = []
    for _ in range(iters):
        idx = rng.integers(0, n, n)
        diffs.append(
            score(y[idx], pred_a[idx], metric) - score(y[idx], pred_b[idx], metric)
        )
    return float(np.mean(diffs)), (
        float(np.percentile(diffs, 2.5)),
        float(np.percentile(diffs, 97.5)),
    )


def write_tsv(path: Path, rows: list[dict], fields: list[str]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("w", encoding="utf-8", newline="") as fh:
        fh.write("\t".join(fields) + "\n")
        for row in rows:
            fh.write("\t".join(str(row.get(k, "")) for k in fields) + "\n")


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--root", default=r"D:\trit-results\davidson")
    ap.add_argument("--out", default=r"D:\trit-results\davidson")
    ap.add_argument("--iters", type=int, default=10000)
    ap.add_argument("--fixed-c", type=float, default=None)
    for depth in ("l12", "l6"):
        for probe in ("logreg", "linear_svc"):
            ap.add_argument(f"--c-{depth}-{probe}", type=float, default=None)
    ap.add_argument("--select-only", action="store_true")
    ap.add_argument("--depth", choices=("l12", "l6", "both"), default="both")
    ap.add_argument("--probe", choices=("logreg", "linear_svc", "both"), default="both")
    args = ap.parse_args()
    root = Path(args.root)
    out = Path(args.out)
    rows = json.loads((root / "texts-l12.json").read_text(encoding="utf-8"))["texts"]
    if len(rows) != 24781:
        raise ValueError(f"unexpected texts: {len(rows)}")
    y = np.array([int(r["label"]) for r in rows], dtype=np.int64)
    train = np.array([i for i, r in enumerate(rows) if r["split"] == "train"])
    test = np.array([i for i, r in enumerate(rows) if r["split"] == "test"])
    print("rows", len(rows), "fit", len(train), "test", len(test), flush=True)
    print("fit labels", np.bincount(y[train]).tolist(), "test labels", np.bincount(y[test]).tolist(), flush=True)
    print("fit prior", y[train].mean(), "test prior", y[test].mean(), flush=True)

    arms_by_depth = {
        "l12": {
            "f32": load(str(root / "emb12" / "l12-pure-f32-*.f32")),
            "r12": load(str(root / "emb12" / "l12-r12-*.f32")),
        },
        "l6": {
            "f32": load(str(root / "emb6" / "l6-pure-f32-*.f32")),
            "rescan": load(str(root / "emb6" / "l6-rescan-*.f32")),
        },
    }
    if args.depth != "both":
        arms_by_depth = {args.depth: arms_by_depth[args.depth]}
    probes = ("logreg", "linear_svc") if args.probe == "both" else (args.probe,)
    for depth, arms in arms_by_depth.items():
        for name, x in arms.items():
            if len(x) != len(rows):
                raise ValueError(f"{depth}/{name}: {len(x)} != {len(rows)}")
        print(depth, {k: v.shape for k, v in arms.items()}, flush=True)

    if args.select_only:
        for depth, arms in arms_by_depth.items():
            for probe in probes:
                skf = StratifiedKFold(n_splits=5, shuffle=True, random_state=SEEDS[0])
                folds = list(skf.split(np.zeros(len(train)), y[train]))
                c = select_c(arms["f32"], y, train, probe, folds)
                print("select", depth, probe, "C", c, flush=True)
        return

    raw, comparisons = [], []
    for depth, arms in arms_by_depth.items():
        quant_arm = [name for name in arms if name != "f32"][0]
        for probe in probes:
            for seed in SEEDS:
                c = args.fixed_c
                if c is None:
                    c = getattr(args, f"c_{depth}_{probe}")
                if c is None:
                    skf = StratifiedKFold(n_splits=5, shuffle=True, random_state=seed)
                    folds = list(skf.split(np.zeros(len(train)), y[train]))
                    c = select_c(arms["f32"], y, train, probe, folds)
                preds = {}
                for name, x in arms.items():
                    clf = make_clf(probe, c)
                    clf.fit(x[train], y[train])
                    preds[name] = predict(clf, x[test], probe)
                print(depth, probe, "seed", seed, "C", c, flush=True)

                for name, pred in preds.items():
                    rec = recall_score(y[test], pred, labels=[0, 1], average=None, zero_division=0)
                    raw.append(
                        {
                            "depth": depth,
                            "probe": probe,
                            "arm": name,
                            "seed": seed,
                            "C": c,
                            "n": len(test),
                            "accuracy": score(y[test], pred, "accuracy"),
                            "macro_f1": score(y[test], pred, "macro_f1"),
                            "recall_non_hate": float(rec[0]),
                            "recall_hate": float(rec[1]),
                            "pred_pos_rate": float(pred.mean()),
                        }
                    )
                for metric in ("accuracy", "macro_f1"):
                    mean, ci = boot(preds[quant_arm], preds["f32"], y[test], args.iters, seed, metric)
                    point = score(y[test], preds[quant_arm], metric) - score(y[test], preds["f32"], metric)
                    comparisons.append(
                        {
                            "depth": depth,
                            "probe": probe,
                            "arm": quant_arm,
                            "metric": metric,
                            "seed": seed,
                            "C": c,
                            "n": len(test),
                            "score_arm": score(y[test], preds[quant_arm], metric),
                            "score_f32": score(y[test], preds["f32"], metric),
                            "point_delta": point,
                            "boot_mean": mean,
                            "ci_low": ci[0],
                            "ci_high": ci[1],
                            "verdict": "positive" if ci[0] > 0 else "negative" if ci[1] < 0 else "cross",
                        }
                    )

    write_tsv(
        out / "wsp-classify-m145-raw.tsv",
        raw,
        ["depth", "probe", "arm", "seed", "C", "n", "accuracy", "macro_f1",
         "recall_non_hate", "recall_hate", "pred_pos_rate"],
    )
    write_tsv(
        out / "wsp-classify-m145-compare.tsv",
        comparisons,
        ["depth", "probe", "arm", "metric", "seed", "C", "n", "score_arm", "score_f32",
         "point_delta", "boot_mean", "ci_low", "ci_high", "verdict"],
    )

    primary = []
    for depth in ("l12", "l6"):
        for probe in ("logreg", "linear_svc"):
            vals = [r for r in comparisons if r["depth"] == depth and r["probe"] == probe and r["metric"] == "accuracy"]
            primary.append(
                {
                    "depth": depth,
                    "probe": probe,
                    "positive": sum(v["verdict"] == "positive" for v in vals),
                    "n": len(vals),
                    "mean_point": float(np.mean([v["point_delta"] for v in vals])),
                    "mean_boot": float(np.mean([v["boot_mean"] for v in vals])),
                    "note": "descriptive only",
                }
            )
    write_tsv(
        out / "wsp-classify-m145-primary.tsv",
        primary,
        ["depth", "probe", "positive", "n", "mean_point", "mean_boot", "note"],
    )
    (out / "wsp-classify-m145.json").write_text(
        json.dumps({"seeds": SEEDS, "rows": raw, "comparisons": comparisons, "primary": primary}, indent=2),
        encoding="utf-8",
    )
    print(json.dumps(primary, indent=2), flush=True)


if __name__ == "__main__":
    main()

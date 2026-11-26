"""M1.46: fit-balanced linear probes as a prior-free decision rule.

Reuses M1.41/M1.42/M1.45 embeddings. No test-prior, target_prior, fit-prior
quantile, calibration, or threshold search. The only change is
class_weight="balanced" estimated on fit labels; the decision threshold stays
at the default (logreg 0.5 / LinearSVC 0.0).

Primary: balanced f32 - unweighted f32 macro-F1, 5 seeds x 3 corpora x 2 depths
x 2 probes. C is selected once per corpus/depth/probe on f32 fit with
unweighted 5-fold accuracy (seed 7), then shared by all seeds/rules/arms.
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
# These are bootstrap RNG seeds, not model seeds. `make_clf` fixes
# random_state=7, so all five rows per cell have identical predictions and
# metrics; the 5 CIs are repeated resamples of the same comparison. For an
# independent-seed confirmatory, vary the fit/split, not only boot_macro.
GRID = (0.01, 0.1, 1.0, 10.0)
CORPORA = {
    "hatexplain": {
        "root": r"D:\trit-results\hatexplain",
        "rows": 19188,
        "fit": 17270,
        "test": 1918,
        "texts": "texts-l12.json",
    },
    "tweet-hate": {
        "root": r"D:\trit-results\tweet-hate",
        "rows": 11719,
        "fit": 8985,
        "test": 2734,
        "texts": "texts-l12.json",
    },
    "davidson": {
        "root": r"D:\trit-results\davidson",
        "rows": 24781,
        "fit": 19825,
        "test": 4956,
        "texts": "texts-l12.json",
    },
}
DEPTH_ARMS = {
    "l12": (("f32", "l12-pure-f32-*.f32", "emb12"),
             ("r12", "l12-r12-*.f32", "emb12")),
    "l6": (("f32", "l6-pure-f32-*.f32", "emb6"),
           ("rescan", "l6-rescan-*.f32", "emb6")),
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


def make_clf(probe: str, c: float, balanced: bool):
    weight = "balanced" if balanced else None
    if probe == "logreg":
        return LogisticRegression(
            C=c, class_weight=weight, max_iter=5000, solver="lbfgs", random_state=7
        )
    return LinearSVC(
        C=c, class_weight=weight, max_iter=10000, random_state=7, tol=1e-4
    )


def get_scores(clf, x, probe: str) -> np.ndarray:
    if probe == "logreg":
        return clf.predict_proba(x)[:, 1]
    return clf.decision_function(x)


def predict(clf, x, probe: str) -> np.ndarray:
    if probe == "logreg":
        return (clf.predict_proba(x)[:, 1] >= 0.5).astype(np.int64)
    return (clf.decision_function(x) >= 0.0).astype(np.int64)


def score(y, pred, metric: str) -> float:
    if metric == "accuracy":
        return float(accuracy_score(y, pred))
    return float(f1_score(y, pred, average="macro", zero_division=0))


def recall_pair(y, pred) -> tuple[float, float]:
    vals = recall_score(y, pred, labels=[0, 1], average=None, zero_division=0)
    return float(vals[0]), float(vals[1])


def macro_f1_binary(y, pred) -> float:
    tp = int(np.sum((pred == 1) & (y == 1)))
    tn = int(np.sum((pred == 0) & (y == 0)))
    fp = int(np.sum((pred == 1) & (y == 0)))
    fn = int(np.sum((pred == 0) & (y == 1)))
    pos = 2 * tp / (2 * tp + fp + fn) if (2 * tp + fp + fn) else 0.0
    neg = 2 * tn / (2 * tn + fn + fp) if (2 * tn + fn + fp) else 0.0
    return float((pos + neg) / 2.0)


def select_c(x, y, train, probe: str, folds) -> float:
    """C on f32 fit, unweighted 5-fold accuracy; same rule as M1.41/M1.45."""
    vals = []
    for c in GRID:
        fold_scores = []
        for tr, va in folds:
            clf = make_clf(probe, c, balanced=False)
            clf.fit(x[train][tr], y[train][tr])
            fold_scores.append(score(y[train][va], clf.predict(x[train][va]), "accuracy"))
        vals.append((float(np.mean(fold_scores)), c))
    return max(vals)[1]


def boot_macro(pred_a, pred_b, y, iters: int, seed: int):
    """Paired test-level bootstrap for macro-F1."""
    rng = np.random.default_rng(seed)
    n = len(y)
    diffs = []
    for _ in range(iters):
        idx = rng.integers(0, n, n)
        diffs.append(macro_f1_binary(y[idx], pred_a[idx])
                     - macro_f1_binary(y[idx], pred_b[idx]))
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


def load_corpus(name: str, root: Path):
    cfg = CORPORA[name]
    rows = json.loads((root / cfg["texts"]).read_text(encoding="utf-8"))["texts"]
    if len(rows) != cfg["rows"]:
        raise ValueError(f"{name}: texts {len(rows)} != {cfg['rows']}")
    y = np.array([int(r["label"]) for r in rows], dtype=np.int64)
    train = np.array([i for i, r in enumerate(rows) if r["split"] == "train"])
    test = np.array([i for i, r in enumerate(rows) if r["split"] == "test"])
    if len(train) != cfg["fit"] or len(test) != cfg["test"]:
        raise ValueError(f"{name}: fit/test {len(train)}/{len(test)} != "
                         f"{cfg['fit']}/{cfg['test']}")
    arms = {}
    for depth, specs in DEPTH_ARMS.items():
        arms[depth] = {
            arm: load(str(root / subdir / pattern))
            for arm, pattern, subdir in specs
        }
        for arm, x in arms[depth].items():
            if len(x) != len(rows):
                raise ValueError(f"{name}/{depth}/{arm}: {len(x)} != {len(rows)}")
    return rows, y, train, test, arms


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--corpus", choices=tuple(CORPORA), required=True)
    ap.add_argument("--root", default=None)
    ap.add_argument("--out", default=None)
    ap.add_argument("--iters", type=int, default=10000)
    ap.add_argument("--depth", choices=("l12", "l6", "both"), default="both")
    ap.add_argument("--probe", choices=("logreg", "linear_svc", "both"), default="both")
    ap.add_argument("--select-only", action="store_true")
    ap.add_argument("--fixed-c", type=float, default=None)
    for depth in ("l12", "l6"):
        for probe in ("logreg", "linear_svc"):
            ap.add_argument(f"--c-{depth}-{probe}", type=float, default=None)
    args = ap.parse_args()

    cfg = CORPORA[args.corpus]
    root = Path(args.root) if args.root else Path(cfg["root"])
    out = Path(args.out) if args.out else root
    rows, y, train, test, arms_by_depth = load_corpus(args.corpus, root)
    depths = ("l12", "l6") if args.depth == "both" else (args.depth,)
    probes = ("logreg", "linear_svc") if args.probe == "both" else (args.probe,)
    print(args.corpus, "rows", len(rows), "fit", len(train), "test", len(test),
          "fit_prior", float(y[train].mean()), "test_prior", float(y[test].mean()),
          flush=True)

    if args.select_only:
        for depth in depths:
            for probe in probes:
                folds = list(StratifiedKFold(
                    n_splits=5, shuffle=True, random_state=SEEDS[0]
                ).split(np.zeros(len(train)), y[train]))
                c = select_c(arms_by_depth[depth]["f32"], y, train, probe, folds)
                print("select", args.corpus, depth, probe, "C", c, flush=True)
        return

    raw, compare = [], []
    for depth in depths:
        arms = arms_by_depth[depth]
        quant = [name for name in arms if name != "f32"][0]
        for probe in probes:
            c = args.fixed_c
            if c is None:
                c = getattr(args, f"c_{depth}_{probe}")
            if c is None:
                folds = list(StratifiedKFold(
                    n_splits=5, shuffle=True, random_state=SEEDS[0]
                ).split(np.zeros(len(train)), y[train]))
                c = select_c(arms["f32"], y, train, probe, folds)
            for seed in SEEDS:
                preds = {}
                for rule in ("unweighted", "balanced"):
                    for arm, x in arms.items():
                        clf = make_clf(probe, c, balanced=(rule == "balanced"))
                        clf.fit(x[train], y[train])
                        preds[(rule, arm)] = predict(clf, x[test], probe)
                print(args.corpus, depth, probe, "seed", seed, "C", c, flush=True)

                for rule in ("unweighted", "balanced"):
                    for arm in arms:
                        pred = preds[(rule, arm)]
                        r0, r1 = recall_pair(y[test], pred)
                        raw.append({
                            "corpus": args.corpus, "depth": depth, "probe": probe,
                            "rule": rule, "arm": arm, "seed": seed, "C": c,
                            "n": len(test), "accuracy": score(y[test], pred, "accuracy"),
                            "macro_f1": macro_f1_binary(y[test], pred),
                            "recall_non_hate": r0, "recall_hate": r1,
                            "pred_pos_rate": float(pred.mean()),
                            "prior_fit": float(y[train].mean()),
                            "prior_test": float(y[test].mean()),
                        })

                a = preds[("balanced", "f32")]
                b = preds[("unweighted", "f32")]
                mean, ci = boot_macro(a, b, y[test], args.iters, seed)
                point = macro_f1_binary(y[test], a) - macro_f1_binary(y[test], b)
                compare.append({
                    "corpus": args.corpus, "kind": "rule_diff", "depth": depth,
                    "probe": probe, "arm": "f32", "rule_a": "balanced",
                    "rule_b": "unweighted", "metric": "macro_f1", "seed": seed,
                    "C": c, "n": len(test),
                    "score_a": macro_f1_binary(y[test], a),
                    "score_b": macro_f1_binary(y[test], b),
                    "point_delta": point, "boot_mean": mean,
                    "ci_low": ci[0], "ci_high": ci[1],
                    "verdict": "positive" if ci[0] > 0 else
                               "negative" if ci[1] < 0 else "cross",
                })

                for arm in (quant,):
                    for metric in ("accuracy", "macro_f1"):
                        for rule in ("unweighted", "balanced"):
                            aa = preds[(rule, arm)]
                            bb = preds[(rule, "f32")]
                            point = score(y[test], aa, metric) - score(y[test], bb, metric)
                            compare.append({
                                "corpus": args.corpus, "kind": "arm_minus_f32",
                                "depth": depth, "probe": probe, "arm": arm,
                                "rule_a": rule, "rule_b": rule, "metric": metric,
                                "seed": seed, "C": c, "n": len(test),
                                "score_a": score(y[test], aa, metric),
                                "score_b": score(y[test], bb, metric),
                                "point_delta": point, "boot_mean": float("nan"),
                                "ci_low": float("nan"), "ci_high": float("nan"),
                                "verdict": "point",
                            })

    suffix = "" if args.depth == "both" and args.probe == "both" else \
        f"-{args.depth}-{args.probe}"
    raw_fields = ["corpus", "depth", "probe", "rule", "arm", "seed", "C", "n",
                  "accuracy", "macro_f1", "recall_non_hate", "recall_hate",
                  "pred_pos_rate", "prior_fit", "prior_test"]
    compare_fields = ["corpus", "kind", "depth", "probe", "arm", "rule_a", "rule_b",
                      "metric", "seed", "C", "n", "score_a", "score_b", "point_delta",
                      "boot_mean", "ci_low", "ci_high", "verdict"]
    write_tsv(out / f"wsp-classify-m146-raw{suffix}.tsv", raw, raw_fields)
    write_tsv(out / f"wsp-classify-m146-compare{suffix}.tsv", compare, compare_fields)

    primary = []
    for corpus in [args.corpus]:
        for depth in depths:
            for probe in probes:
                vals = [r for r in compare if r["kind"] == "rule_diff"
                        and r["corpus"] == corpus and r["depth"] == depth
                        and r["probe"] == probe]
                pos = sum(v["verdict"] == "positive" for v in vals)
                primary.append({
                    "corpus": corpus, "depth": depth, "probe": probe,
                    "metric": "macro_f1", "positive": pos, "n": len(vals),
                    "mean_point": float(np.mean([v["point_delta"] for v in vals])),
                    "mean_boot": float(np.mean([v["boot_mean"] for v in vals])),
                    "verdict": "supported" if pos == 5 else
                               "directional" if pos == 4 else "not_supported",
                })
    write_tsv(out / f"wsp-classify-m146-primary{suffix}.tsv", primary,
              ["corpus", "depth", "probe", "metric", "positive", "n",
               "mean_point", "mean_boot", "verdict"])
    print(json.dumps(primary, indent=2), flush=True)


if __name__ == "__main__":
    main()

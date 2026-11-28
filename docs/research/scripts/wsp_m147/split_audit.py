"""M1.47: Davidson L6 SVC balanced split-stability audit.

This is a single-cell audit of the only 5/5 comparison in M1.46. It does not
promote the cell to a candidate. The five rows are five independently redrawn
80/20 stratified splits; within-split bootstrap CIs only measure sampling noise.
"""

from __future__ import annotations

import argparse
import glob
import json
import struct
from pathlib import Path

import numpy as np
from sklearn.metrics import accuracy_score, recall_score
from sklearn.model_selection import StratifiedKFold, train_test_split
from sklearn.svm import LinearSVC

REPO_ROOT = Path(__file__).resolve().parents[4]
DEFAULT_PREREG = REPO_ROOT / "docs/research/data/wsp-classify-m147-prereg.json"
DEFAULT_DATA_ROOT = Path(r"D:\trit-results\davidson")
DEFAULT_OUT = REPO_ROOT / "docs/research/data"


def read_emb(path: str) -> np.ndarray:
    with open(path, "rb") as fh:
        head = fh.read(16)
        rows, cols = struct.unpack("<II", head[8:])
        return np.fromfile(fh, dtype=np.float32).reshape(rows, cols)


def load_pattern(pattern: str) -> np.ndarray:
    parts = [read_emb(p) for p in sorted(glob.glob(pattern))]
    if not parts:
        raise FileNotFoundError(pattern)
    out = np.concatenate(parts, 0)
    return out


def make_svc(c: float, balanced: bool) -> LinearSVC:
    return LinearSVC(
        C=c,
        class_weight="balanced" if balanced else None,
        max_iter=10000,
        random_state=7,
        tol=1e-4,
    )


def macro_f1_binary(y, pred) -> float:
    tp = int(np.sum((pred == 1) & (y == 1)))
    tn = int(np.sum((pred == 0) & (y == 0)))
    fp = int(np.sum((pred == 1) & (y == 0)))
    fn = int(np.sum((pred == 0) & (y == 1)))
    pos = 2 * tp / (2 * tp + fp + fn) if (2 * tp + fp + fn) else 0.0
    neg = 2 * tn / (2 * tn + fn + fp) if (2 * tn + fn + fp) else 0.0
    return float((pos + neg) / 2.0)


def recall_pair(y, pred) -> tuple[float, float]:
    vals = recall_score(y, pred, labels=[0, 1], average=None, zero_division=0)
    return float(vals[0]), float(vals[1])


def select_c(x, y, train, folds, grid) -> float:
    vals = []
    for c in grid:
        fold_scores = []
        for tr, va in folds:
            clf = make_svc(c, balanced=False)
            clf.fit(x[train][tr], y[train][tr])
            fold_scores.append(accuracy_score(y[train][va], clf.predict(x[train][va])))
        vals.append((float(np.mean(fold_scores)), float(c)))
    return max(vals)[1]


def boot_macro(pred_a, pred_b, y, iters: int, seed: int):
    rng = np.random.default_rng(seed)
    n = len(y)
    diffs = []
    for _ in range(iters):
        idx = rng.integers(0, n, n)
        diffs.append(
            macro_f1_binary(y[idx], pred_a[idx])
            - macro_f1_binary(y[idx], pred_b[idx])
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
    ap.add_argument("--prereg", default=str(DEFAULT_PREREG))
    ap.add_argument("--data-root", default=str(DEFAULT_DATA_ROOT))
    ap.add_argument("--out", default=str(DEFAULT_OUT))
    ap.add_argument("--iters", type=int, default=10000)
    args = ap.parse_args()

    prereg = json.loads(Path(args.prereg).read_text(encoding="utf-8"))
    split_seeds = [int(s) for s in prereg["splits"]["split_seeds"]]
    boot_seeds = [int(s) for s in prereg["primary"]["bootstrap_rng_seeds"]]
    grid = [float(c) for c in prereg["modeling"]["c_grid"]]
    if len(split_seeds) != len(boot_seeds):
        raise ValueError("split_seeds and bootstrap_rng_seeds length mismatch")

    data_root = Path(args.data_root)
    out = Path(args.out)
    rows = json.loads(
        (data_root / "texts-l12.json").read_text(encoding="utf-8")
    )["texts"]
    if len(rows) != prereg["dataset"]["rows"]:
        raise ValueError(f"rows {len(rows)} != prereg {prereg['dataset']['rows']}")
    y = np.array([int(r["label"]) for r in rows], dtype=np.int64)
    if len(set(r["text"] for r in rows)) != len(rows):
        raise ValueError("duplicate normalized text in Davidson table")
    counts = {0: int(np.sum(y == 0)), 1: int(np.sum(y == 1))}
    label_counts = prereg["dataset"]["label_counts"]
    if counts[0] != int(label_counts["non_hate"]) or counts[1] != int(label_counts["hate"]):
        raise ValueError(f"label counts {counts} != prereg {label_counts}")
    x = load_pattern(str(data_root / "emb6" / "l6-pure-f32-*.f32"))
    if x.shape != (len(rows), 384):
        raise ValueError(f"embedding shape {x.shape} != ({len(rows)}, 384)")

    splits_rows, raw_rows, primary_rows = [], [], []
    for split_seed, boot_seed in zip(split_seeds, boot_seeds):
        train, test = train_test_split(
            np.arange(len(y)), test_size=0.2, stratify=y, random_state=split_seed
        )
        test_hate = int(np.sum(y[test] == 1))
        if test_hate == 0:
            raise ValueError(f"split {split_seed}: zero hate rows in test")
        folds = list(
            StratifiedKFold(
                n_splits=5, shuffle=True, random_state=7
            ).split(np.zeros(len(train)), y[train])
        )
        c = select_c(x, y, train, folds, grid)
        preds = {}
        for rule in ("unweighted", "balanced"):
            clf = make_svc(c, balanced=(rule == "balanced"))
            clf.fit(x[train], y[train])
            preds[rule] = clf.predict(x[test]).astype(np.int64)

        unweighted = preds["unweighted"]
        balanced = preds["balanced"]
        point = macro_f1_binary(y[test], balanced) - macro_f1_binary(y[test], unweighted)
        boot_mean, (ci_low, ci_high) = boot_macro(
            balanced, unweighted, y[test], args.iters, boot_seed
        )
        point_positive = point > 0
        ci_positive = ci_low > 0
        verdict = "positive" if ci_positive else "negative" if ci_high < 0 else "cross"

        splits_rows.append({
            "split_seed": split_seed,
            "fit_n": len(train),
            "test_n": len(test),
            "fit_hate": int(np.sum(y[train] == 1)),
            "test_hate": test_hate,
            "fit_prior": float(y[train].mean()),
            "test_prior": float(y[test].mean()),
            "C": c,
            "pred_pos_unweighted": float(unweighted.mean()),
            "pred_pos_balanced": float(balanced.mean()),
        })
        for rule, pred in preds.items():
            r0, r1 = recall_pair(y[test], pred)
            raw_rows.append({
                "split_seed": split_seed,
                "rule": rule,
                "C": c,
                "n": len(test),
                "accuracy": float(accuracy_score(y[test], pred)),
                "macro_f1": macro_f1_binary(y[test], pred),
                "recall_non_hate": r0,
                "recall_hate": r1,
                "pred_pos_rate": float(pred.mean()),
                "prior_fit": float(y[train].mean()),
                "prior_test": float(y[test].mean()),
            })
        primary_rows.append({
            "split_seed": split_seed,
            "C": c,
            "n": len(test),
            "unweighted_macro_f1": macro_f1_binary(y[test], unweighted),
            "balanced_macro_f1": macro_f1_binary(y[test], balanced),
            "point_delta": point,
            "boot_mean": boot_mean,
            "ci_low": ci_low,
            "ci_high": ci_high,
            "point_positive": point_positive,
            "ci_positive": ci_positive,
            "verdict": verdict,
        })
        print(
            f"split {split_seed}: C={c} n={len(test)} hate={test_hate} "
            f"point={point*100:.4f}pp CI=[{ci_low*100:.4f},{ci_high*100:.4f}] "
            f"{verdict}",
            flush=True,
        )

    point_positive_count = sum(bool(r["point_positive"]) for r in primary_rows)
    ci_positive_count = sum(bool(r["ci_positive"]) for r in primary_rows)
    mean_point = float(np.mean([r["point_delta"] for r in primary_rows]))
    supported = point_positive_count >= 4 and ci_positive_count >= 4 and mean_point > 0
    directional = point_positive_count >= 3 and mean_point > 0 and not supported
    verdict = "supported" if supported else "directional" if directional else "not_supported"

    write_tsv(
        out / "wsp-classify-m147-splits.tsv", splits_rows,
        ["split_seed", "fit_n", "test_n", "fit_hate", "test_hate", "fit_prior",
         "test_prior", "C", "pred_pos_unweighted", "pred_pos_balanced"],
    )
    write_tsv(
        out / "wsp-classify-m147-raw.tsv", raw_rows,
        ["split_seed", "rule", "C", "n", "accuracy", "macro_f1",
         "recall_non_hate", "recall_hate", "pred_pos_rate", "prior_fit", "prior_test"],
    )
    write_tsv(
        out / "wsp-classify-m147-primary.tsv", primary_rows,
        ["split_seed", "C", "n", "unweighted_macro_f1", "balanced_macro_f1",
         "point_delta", "boot_mean", "ci_low", "ci_high", "point_positive",
         "ci_positive", "verdict"],
    )
    verdict_obj = {
        "run": "M1.47",
        "status": "completed",
        "prereg": "docs/research/data/wsp-classify-m147-prereg.json",
        "independent_units": len(primary_rows),
        "split_seeds": split_seeds,
        "bootstrap_rng_seeds": boot_seeds,
        "point_positive_splits": point_positive_count,
        "ci_positive_splits": ci_positive_count,
        "mean_point_delta": mean_point,
        "min_point_delta": float(min(r["point_delta"] for r in primary_rows)),
        "max_point_delta": float(max(r["point_delta"] for r in primary_rows)),
        "supported": bool(supported),
        "verdict": verdict,
        "note": (
            "Descriptive split-stability audit of one M1.46 cell on one corpus. "
            "The five split rows are independent split draws, not independent corpora; "
            "even a supported verdict does not reopen the candidate line."
        ),
    }
    (out / "wsp-classify-m147-verdict.json").write_text(
        json.dumps(verdict_obj, indent=2, ensure_ascii=False), encoding="utf-8"
    )
    print(json.dumps(verdict_obj, indent=2, ensure_ascii=False), flush=True)


if __name__ == "__main__":
    main()

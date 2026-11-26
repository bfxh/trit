#!/usr/bin/env python3
"""Measure probe-training sensitivity with repeated train subsamples.

The held-out test split stays fixed.  For each repeat we run stratified
K-fold over the train split, fit the same frozen-encoder linear probe on the
K-1 folds (80% by default), and evaluate every run on the full official test.
The resulting spread of ``candidate - baseline`` deltas answers whether a
single-split accuracy gain survives probe retraining on different train
subsets; it is not a replacement for the held-out test estimate.

Example:
  python scripts/wsp_classify_subsample.py \
    --texts sst2/data-l12/texts.json \
    --run l12-f32='sst2/emb12/l12-pure-f32-*.f32' \
    --run l12-int4='sst2/emb12/l12-int4-*.f32' \
    --baseline l12-f32 --C l12-f32=10 --C l12-int4=10 \
    --probe logreg --folds 5 --repeats 5 --out sst2/subsample-l12.json
"""

import argparse
import json
import os
import sys
from pathlib import Path

os.environ.setdefault("OPENBLAS_NUM_THREADS", "1")
os.environ.setdefault("OMP_NUM_THREADS", "1")
os.environ.setdefault("MKL_NUM_THREADS", "1")

from wsp_classify_eval import _labels, _metrics, _probe, concat


def _summarize(values):
    import numpy as np

    data = np.asarray(values, dtype=np.float64)
    return {
        "n": int(data.size),
        "mean": float(data.mean()),
        "sd": float(data.std(ddof=1)) if data.size > 1 else 0.0,
        "min": float(data.min()),
        "max": float(data.max()),
        "positive": int((data > 0).sum()),
        "negative": int((data < 0).sum()),
        "zero": int((data == 0).sum()),
    }


def run(texts_path, run_specs, baseline, c_values, probe, folds, repeats, seed):
    import numpy as np
    from sklearn.model_selection import StratifiedKFold

    texts = json.loads(Path(texts_path).read_text(encoding="utf-8"))["texts"]
    train_idx, test_idx, y_train, y_test = _labels(texts)
    matrices = {}
    for spec in run_specs:
        name, pattern = spec.split("=", 1)
        matrices[name] = concat(pattern)
        if matrices[name].shape[0] != len(texts):
            raise ValueError(
                f"{name}: rows {matrices[name].shape[0]} != texts {len(texts)}"
            )
    if baseline not in matrices:
        raise ValueError(f"baseline {baseline!r} is not a --run")
    missing_c = sorted(set(matrices) - set(c_values))
    if missing_c:
        raise ValueError(f"missing --C for {missing_c}")
    y_train = np.asarray(y_train, dtype=np.int64)
    y_test = np.asarray(y_test, dtype=np.int64)
    x_train = {name: matrix[train_idx] for name, matrix in matrices.items()}
    x_test = {name: matrix[test_idx] for name, matrix in matrices.items()}

    replicates = []
    for repeat in range(repeats):
        repeat_seed = seed + repeat
        splitter = StratifiedKFold(
            n_splits=folds, shuffle=True, random_state=repeat_seed
        )
        for fold, (fit_idx, _) in enumerate(splitter.split(x_train[baseline], y_train)):
            metrics = {}
            for name in matrices:
                model, scaler = _probe(
                    x_train[name][fit_idx],
                    y_train[fit_idx],
                    repeat_seed,
                    c_values.get(name, 1.0),
                    probe=probe,
                )
                metrics[name] = _metrics(model, scaler, x_test[name], y_test)
            row = {"repeat": repeat, "seed": repeat_seed, "fold": fold, "runs": {}}
            for name, values in metrics.items():
                row["runs"][name] = {
                    key: value
                    for key, value in values.items()
                    if key not in ("correct", "prediction")
                }
            row["deltas"] = {
                name: {
                    "accuracy": metrics[name]["accuracy"]
                    - metrics[baseline]["accuracy"],
                    "macro_f1": metrics[name]["macro_f1"]
                    - metrics[baseline]["macro_f1"],
                }
                for name in matrices
                if name != baseline
            }
            replicates.append(row)

    by_metric = {"accuracy": {}, "macro_f1": {}}
    for name in matrices:
        if name == baseline:
            continue
        for metric in ("accuracy", "macro_f1"):
            by_metric[metric][f"{name}-{baseline}"] = _summarize(
                [row["deltas"][name][metric] for row in replicates]
            )
    return {
        "probe": probe,
        "folds": folds,
        "repeats": repeats,
        "seed": seed,
        "baseline": baseline,
        "C": c_values,
        "n_train": len(y_train),
        "n_test": len(y_test),
        "deltas": by_metric,
        "replicates": replicates,
    }


def self_test():
    summary = _summarize([0.1, 0.2, -0.05, 0.0])
    assert summary["n"] == 4
    assert abs(summary["mean"] - 0.0625) < 1e-12
    assert abs(summary["sd"] - 0.11086778913041725) < 1e-12
    assert summary["min"] == -0.05
    assert summary["max"] == 0.2
    assert (summary["positive"], summary["negative"], summary["zero"]) == (2, 1, 1)
    print("self-test ok")


def parse_args():
    parser = argparse.ArgumentParser()
    parser.add_argument("--texts", default="")
    parser.add_argument("--run", action="append", default=[])
    parser.add_argument("--baseline", default="")
    parser.add_argument("--C", action="append", default=[])
    parser.add_argument("--probe", choices=("logreg", "linear_svc"), default="logreg")
    parser.add_argument("--folds", type=int, default=5)
    parser.add_argument("--repeats", type=int, default=5)
    parser.add_argument("--seed", type=int, default=7)
    parser.add_argument("--out", default="")
    parser.add_argument("--self-test", action="store_true")
    return parser.parse_args()


def main():
    args = parse_args()
    if args.self_test:
        self_test()
        return
    if not args.texts or not args.run or not args.baseline or not args.out:
        raise SystemExit("--texts, --run, --baseline and --out are required")
    if args.folds < 2 or args.repeats < 1:
        raise SystemExit("--folds/--repeats must be positive")
    c_values = {}
    for item in args.C:
        name, value = item.split("=", 1)
        c_values[name] = float(value)
    result = run(
        args.texts,
        args.run,
        args.baseline,
        c_values,
        args.probe,
        args.folds,
        args.repeats,
        args.seed,
    )
    Path(args.out).write_text(
        json.dumps(result, indent=2, ensure_ascii=False), encoding="utf-8"
    )
    print(f"done replicates={len(result['replicates'])}")


if __name__ == "__main__":
    sys.exit(main())

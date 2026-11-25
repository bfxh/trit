#!/usr/bin/env python3
"""Cross-evaluate f32/int4 train and test representations.

M1.21 showed the SST-2 L12 gain saturates at a small quantization dose, which
looks like either (a) a better test-time representation or (b) a probe
regularized by training on the quantized representation.  This script separates
those by fitting one linear probe per *train* dose and evaluating it on every
*test* dose:

    train on x(train_lam), test on x(test_lam)

The 5x5 grid contains:
  (0,0)  f32 -> f32          baseline
  (0,1)  f32 -> int4         test-only quantization
  (1,0)  int4 -> f32         train-only quantization (regularization probe)
  (1,1)  int4 -> int4        in-domain int4
  plus intermediate doses.

Only a linear probe is trained; the encoder is frozen.  The same C-selection
protocol as the classification head is used inside the train dose.  A paired
bootstrap compares each cell with the (0,0) baseline.

Example:
  python scripts/wsp_classify_cross.py \
    --texts sst2/data-l12/texts.json \
    --run f32='sst2/emb12/l12-pure-f32-*.f32' \
    --run int4='sst2/emb12/l12-int4-*.f32' \
    --bootstrap 1 --bootstrap-iters 10000 \
    --out sst2/cross-l12.json
"""

import argparse
import json
import sys
import tempfile
from pathlib import Path

import numpy as np

import wsp_classify_eval as CE
import wsp_classify_geometry as GEO
import wsp_classify_interpolate as INTERP

DEFAULT_LAMS = (0.0, 0.125, 0.25, 0.5, 1.0)


def _cells(lams):
    """Return every (train_lambda, test_lambda) pair, not just the diagonal."""
    return [(float(train), float(test)) for train in lams for test in lams]


def _fit(train_x, y_train, seed, cv_grid, probe, class_weight, cv_metric):
    chosen, cv_score = CE._select_c(
        train_x, y_train, seed, cv_grid, probe, cv_metric, class_weight
    )
    model, scaler = CE._probe(
        train_x, y_train, seed, chosen, False, probe, class_weight
    )
    return model, scaler, chosen, cv_score


def evaluate(
    texts_path,
    run_arrays,
    lams=DEFAULT_LAMS,
    seed=7,
    cv_grid=(0.01, 0.1, 1.0, 10.0),
    probe="logreg",
    class_weight=None,
    cv_metric="accuracy",
    bootstrap=0,
    bootstrap_iters=10000,
):
    texts = json.loads(Path(texts_path).read_text(encoding="utf-8"))["texts"]
    train_idx, test_idx, y_train, y_test = CE._labels(texts)
    y_train = np.asarray(y_train, dtype=np.int64)
    y_test = np.asarray(y_test, dtype=np.int64)
    if "f32" not in run_arrays or "int4" not in run_arrays:
        raise ValueError("f32 and int4 runs are required")
    if not lams or abs(float(lams[0])) > 1e-12:
        raise ValueError("first lambda must be 0")

    f32 = np.asarray(run_arrays["f32"])
    int4 = np.asarray(run_arrays["int4"])
    if f32.shape != int4.shape:
        raise ValueError(f"shape mismatch: {f32.shape} vs {int4.shape}")

    f32_train, f32_test = f32[train_idx], f32[test_idx]
    int4_train, int4_test = int4[train_idx], int4[test_idx]
    cells = []

    for train_lam in lams:
        train_x = INTERP._mix(f32_train, int4_train, train_lam)
        model, scaler, chosen, cv_score = _fit(
            train_x, y_train, seed, cv_grid, probe, class_weight, cv_metric
        )
        train_metrics = CE._metrics(model, scaler, train_x, y_train)
        for test_lam in lams:
            test_x = INTERP._mix(f32_test, int4_test, test_lam)
            metrics = CE._metrics(model, scaler, test_x, y_test)
            test_geometry = GEO.geometry(test_x, y_test)
            cells.append(
                {
                    "train_lambda": float(train_lam),
                    "test_lambda": float(test_lam),
                    "selected_C": chosen,
                    "cv": cv_score,
                    "train_accuracy": train_metrics["accuracy"],
                    "accuracy": metrics["accuracy"],
                    "macro_f1": metrics["macro_f1"],
                    "test_sep": test_geometry["sep"],
                    "test_between_sq": test_geometry["between_sq"],
                    "test_within_sq": test_geometry["within_sq"],
                    "prediction": metrics["prediction"],
                }
            )

    baseline = cells[0]
    base_accuracy = baseline["accuracy"]
    base_macro_f1 = baseline["macro_f1"]
    base_sep = baseline["test_sep"]
    for cell in cells:
        cell["gap_accuracy_vs_f32"] = float(cell["accuracy"] - base_accuracy)
        cell["gap_macro_f1_vs_f32"] = float(cell["macro_f1"] - base_macro_f1)
        cell["test_sep_ratio_vs_f32"] = float(cell["test_sep"] / base_sep)

    boot = []
    if bootstrap:
        base_pred = baseline["prediction"]
        for cell in cells:
            if cell["train_lambda"] == 0.0 and cell["test_lambda"] == 0.0:
                continue
            delta = CE._bootstrap(
                cell["prediction"],
                base_pred,
                y_test,
                bootstrap_iters,
                seed,
                "accuracy",
            )
            delta.update(
                {
                    "mode": "cross_minus_f32",
                    "train_lambda": cell["train_lambda"],
                    "test_lambda": cell["test_lambda"],
                    "iters": bootstrap_iters,
                }
            )
            boot.append(delta)

    for cell in cells:
        cell.pop("prediction", None)

    return {
        "probe": probe,
        "class_weight": class_weight,
        "cv_metric": cv_metric,
        "lambdas": [float(lam) for lam in lams],
        "n_train": int(len(y_train)),
        "n_test": int(len(y_test)),
        "cells": cells,
        "bootstrap": boot,
    }


def self_test():
    lams = (0.0, 0.5, 1.0)
    cells = _cells(lams)
    assert len(cells) == 9
    assert (0.0, 1.0) in cells
    assert (1.0, 0.0) in cells
    assert (1.0, 1.0) in cells
    assert all(a in lams and b in lams for a, b in cells)
    # Endpoint interpolation keeps the raw arrays exact.
    rng = np.random.default_rng(7)
    f32 = rng.normal(size=(4, 3))
    int4 = rng.normal(size=(4, 3))
    assert np.allclose(INTERP._mix(f32, int4, 0.0), f32)
    assert np.allclose(INTERP._mix(f32, int4, 1.0), int4)
    # Toy grid where int4 flips the class direction: if either loop collapses
    # to lambda=0, the corresponding off-diagonal cell cannot stay low.
    labels = np.array([0, 1] * 20 + [0, 1] * 10, dtype=np.int64)
    f32 = rng.normal(scale=0.03, size=(60, 6))
    f32[:, 0] = (2 * labels - 1) * 1.0
    int4 = f32.copy()
    int4[:, 0] = -(2 * labels - 1) * 1.0
    rows = [
        {
            "index": i,
            "split": "train" if i < 40 else "test",
            "label": int(labels[i]),
        }
        for i in range(60)
    ]
    with tempfile.TemporaryDirectory() as tmp:
        texts = Path(tmp) / "texts.json"
        texts.write_text(json.dumps({"texts": rows}), encoding="utf-8")
        result = evaluate(
            texts,
            {"f32": f32, "int4": int4},
            lams=(0.0, 1.0),
            bootstrap=0,
        )
    got = {
        (round(c["train_lambda"], 6), round(c["test_lambda"], 6)): c["accuracy"]
        for c in result["cells"]
    }
    assert set(got) == {(0.0, 0.0), (0.0, 1.0), (1.0, 0.0), (1.0, 1.0)}
    assert got[(0.0, 0.0)] > 0.8
    assert got[(0.0, 1.0)] < 0.5
    assert got[(1.0, 0.0)] < 0.5
    print("self-test ok")


def parse_args():
    parser = argparse.ArgumentParser()
    parser.add_argument("--texts", default="")
    parser.add_argument("--run", action="append", default=[])
    parser.add_argument("--lams", default=",".join(str(x) for x in DEFAULT_LAMS))
    parser.add_argument("--out", default="")
    parser.add_argument("--seed", type=int, default=7)
    parser.add_argument("--probe", default="logreg")
    parser.add_argument("--class-weight", default=None)
    parser.add_argument("--cv-metric", default="accuracy")
    parser.add_argument("--bootstrap", type=int, default=0)
    parser.add_argument("--bootstrap-iters", type=int, default=10000)
    parser.add_argument("--self-test", action="store_true")
    return parser.parse_args()


def main():
    args = parse_args()
    if args.self_test:
        self_test()
        return
    if not args.texts or not args.run or not args.out:
        raise SystemExit("--texts, --run and --out are required")
    run_arrays = {}
    for spec in args.run:
        name, pattern = spec.split("=", 1)
        run_arrays[name] = GEO.concat(pattern)
    lams = tuple(float(x) for x in args.lams.split(",") if x.strip())
    result = evaluate(
        args.texts,
        run_arrays,
        lams=lams,
        seed=args.seed,
        probe=args.probe,
        class_weight=args.class_weight,
        cv_metric=args.cv_metric,
        bootstrap=args.bootstrap,
        bootstrap_iters=args.bootstrap_iters,
    )
    Path(args.out).write_text(
        json.dumps(result, indent=2, ensure_ascii=False), encoding="utf-8"
    )
    print(f"done {len(lams)}x{len(lams)} cells -> {args.out}")


if __name__ == "__main__":
    sys.exit(main())

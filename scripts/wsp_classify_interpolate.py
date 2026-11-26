#!/usr/bin/env python3
"""Interpolate f32 and int4 embeddings along the quantization-error dose.

M1.20 showed that the class-direction angle is absorbable by a linear probe:
rotating f32's direction onto int4's leaves accuracy unchanged.  The next
question is whether the *magnitude* of the quantization error behaves like a
continuous cause.  This script builds

    x(lam) = normalize(f32 + lam * (int4 - f32)),   lam in [0, 1]

with the raw endpoints kept exactly at lam=0 and lam=1 (all WSPEMB1 rows are
already unit norm; the intermediate path is re-normalized).  At each lam it
retrains the same linear probe and records accuracy plus the geometry readouts
used in M1.17/M1.20 (sep, between_sq, within_sq).

If the SST-2 gain were a smooth function of class separation, accuracy and sep
should move together.  If accuracy saturates early, or moves opposite to sep
(CoLA), then sep is a readout, not a general mediator.

Example:
  python scripts/wsp_classify_interpolate.py \
    --texts sst2/data-l12/texts.json \
    --run f32='sst2/emb12/l12-pure-f32-*.f32' \
    --run int4='sst2/emb12/l12-int4-*.f32' \
    --bootstrap 1 --bootstrap-iters 10000 \
    --out sst2/interpolate-l12.json
"""

import argparse
import json
import sys
from pathlib import Path

import numpy as np

import wsp_classify_eval as CE
import wsp_classify_geometry as GEO
import wsp_classify_pca as PCA

DEFAULT_LAMS = (0.0, 0.0625, 0.125, 0.25, 0.5, 0.75, 1.0)


def _mix(f32, int4, lam):
    """Interpolate quantization error, keeping the raw endpoints exact."""
    if lam <= 0.0:
        return np.asarray(f32).copy()
    if lam >= 1.0:
        return np.asarray(int4).copy()
    f32 = np.asarray(f32, dtype=np.float64)
    int4 = np.asarray(int4, dtype=np.float64)
    mixed = f32 + lam * (int4 - f32)
    norms = np.linalg.norm(mixed, axis=1, keepdims=True)
    return mixed / np.maximum(norms, 1e-12)


def _direction(x, y):
    return np.asarray(x[y == 1].mean(axis=0) - x[y == 0].mean(axis=0), dtype=np.float64)


def _cos(a, b):
    na = float(np.linalg.norm(a))
    nb = float(np.linalg.norm(b))
    if na == 0 or nb == 0:
        return None
    return float(a @ b / (na * nb))


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
    direction_f32 = _direction(f32_train, y_train)
    direction_int4 = _direction(int4_train, y_train)

    runs = []
    predictions = {}
    for lam in lams:
        lam = float(lam)
        train_x = _mix(f32_train, int4_train, lam)
        test_x = _mix(f32_test, int4_test, lam)
        metrics, pred = PCA._evaluate_one(
            train_x,
            test_x,
            y_train,
            y_test,
            seed,
            cv_grid,
            probe,
            class_weight,
            cv_metric,
        )
        train_geometry = GEO.geometry(train_x, y_train)
        test_geometry = GEO.geometry(test_x, y_test)
        direction = _direction(train_x, y_train)
        runs.append(
            {
                "lambda": lam,
                "metrics": metrics,
                "geometry_train": train_geometry,
                "geometry_test": test_geometry,
                "direction_cos_f32": _cos(direction, direction_f32),
                "direction_cos_int4": _cos(direction, direction_int4),
            }
        )
        predictions[lam] = pred

    baseline = runs[0]
    for row in runs[1:]:
        row["gap_accuracy_vs_f32"] = float(
            row["metrics"]["accuracy"] - baseline["metrics"]["accuracy"]
        )
        row["gap_macro_f1_vs_f32"] = float(
            row["metrics"]["macro_f1"] - baseline["metrics"]["macro_f1"]
        )
        row["sep_ratio_train"] = float(
            row["geometry_train"]["sep"] / baseline["geometry_train"]["sep"]
        )
        row["sep_ratio_test"] = float(
            row["geometry_test"]["sep"] / baseline["geometry_test"]["sep"]
        )
    baseline["gap_accuracy_vs_f32"] = 0.0
    baseline["gap_macro_f1_vs_f32"] = 0.0
    baseline["sep_ratio_train"] = 1.0
    baseline["sep_ratio_test"] = 1.0

    boot = []
    if bootstrap:
        base_pred = predictions[float(lams[0])]
        for lam in lams[1:]:
            lam = float(lam)
            delta = CE._bootstrap(
                predictions[lam],
                base_pred,
                y_test,
                bootstrap_iters,
                seed,
                "accuracy",
            )
            delta.update(
                {
                    "mode": "interpolated_minus_f32",
                    "lambda": lam,
                    "iters": bootstrap_iters,
                }
            )
            boot.append(delta)

    return {
        "probe": probe,
        "class_weight": class_weight,
        "cv_metric": cv_metric,
        "lambdas": [float(lam) for lam in lams],
        "n_train": int(len(y_train)),
        "n_test": int(len(y_test)),
        "runs": runs,
        "bootstrap": boot,
    }


def self_test():
    rng = np.random.default_rng(7)
    f32 = rng.normal(size=(5, 4))
    int4 = rng.normal(size=(5, 4))
    f32 = f32 / np.linalg.norm(f32, axis=1, keepdims=True)
    int4 = int4 / np.linalg.norm(int4, axis=1, keepdims=True)

    assert np.allclose(_mix(f32, int4, 0.0), f32)
    assert np.allclose(_mix(f32, int4, 1.0), int4)
    mid = _mix(f32, int4, 0.5)
    assert np.allclose(np.linalg.norm(mid, axis=1), 1.0, atol=1e-12)
    assert not np.allclose(mid, f32)
    assert not np.allclose(mid, int4)
    # The path at lam=.5 is the normalized midpoint, not an unnormalized blend.
    raw = 0.5 * (f32 + int4)
    assert np.allclose(mid, raw / np.linalg.norm(raw, axis=1, keepdims=True))
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
    print(f"done {len(lams)} lambdas -> {args.out}")


if __name__ == "__main__":
    sys.exit(main())

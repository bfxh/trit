#!/usr/bin/env python3
"""Rotate one run's class-mean direction onto another run's direction.

This is a control for the mechanism story in M1.19.  An orthogonal rotation is
an invertible linear map, so a linear probe can absorb it; rotating f32's class
direction to int4's should leave accuracy essentially unchanged.  If that is
what happens, the direction angle is a symptom, not the mechanism.  The
quantity that can change linear separability is the class separation
``sep = sqrt(between/within)`` (reported by wsp_classify_geometry.py).

The rotation is a Householder reflection in the plane spanned by the two train
class-mean directions.  It is estimated on train only and applied unchanged to
test.  The probe is retrained on the rotated train split; C is selected inside
train with the same protocol as the classification head.

Example:
  python scripts/wsp_classify_rotate.py \
    --texts sst2/data-l12/texts.json \
    --run f32='sst2/emb12/l12-pure-f32-*.f32' \
    --run int4='sst2/emb12/l12-int4-*.f32' \
    --rotate f32 --to int4 \
    --bootstrap 1 --bootstrap-iters 10000 \
    --out sst2/rotate-l12.json
"""

import argparse
import json
import sys
from pathlib import Path

import numpy as np

import wsp_classify_eval as CE
import wsp_classify_geometry as GEO
import wsp_classify_pca as PCA


def _direction(x, y):
    return np.asarray(x[y == 1].mean(axis=0) - x[y == 0].mean(axis=0), dtype=np.float64)


def _rotation(source, target):
    """Return an orthogonal matrix mapping ``source`` to ``target``."""
    source = np.asarray(source, dtype=np.float64)
    target = np.asarray(target, dtype=np.float64)
    ns = float(np.linalg.norm(source))
    nt = float(np.linalg.norm(target))
    if ns == 0 or nt == 0:
        raise ValueError("zero class direction")
    u = source / ns
    v = target / nt
    dim = u.shape[0]
    if np.linalg.norm(u - v) < 1e-12:
        return np.eye(dim, dtype=np.float64)
    if np.linalg.norm(u + v) < 1e-12:
        # Any unit vector orthogonal to u gives a valid reflection.
        basis = np.zeros(dim, dtype=np.float64)
        basis[1 if dim > 1 else 0] = 1.0
        w = basis - (basis @ u) * u
        if np.linalg.norm(w) < 1e-12:
            w = np.zeros(dim, dtype=np.float64)
            w[0] = 1.0
    else:
        w = u - v
    w = w / np.linalg.norm(w)
    return np.eye(dim, dtype=np.float64) - 2.0 * np.outer(w, w)


def _evaluate_one(
    train_x, test_x, y_train, y_test, seed, cv_grid, probe, class_weight, cv_metric
):
    return PCA._evaluate_one(
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


def evaluate(
    texts_path,
    run_arrays,
    rotate,
    target,
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
    if rotate not in run_arrays or target not in run_arrays:
        raise ValueError("rotate/target run not found")

    train_x = {name: matrix[train_idx] for name, matrix in run_arrays.items()}
    test_x = {name: matrix[test_idx] for name, matrix in run_arrays.items()}
    source = _direction(train_x[rotate], y_train)
    destination = _direction(train_x[target], y_train)
    rotation = _rotation(source, destination)
    rotated_train = train_x[rotate] @ rotation.T
    rotated_test = test_x[rotate] @ rotation.T

    runs = {}
    predictions = {}
    for name in run_arrays:
        metrics, pred = _evaluate_one(
            train_x[name],
            test_x[name],
            y_train,
            y_test,
            seed,
            cv_grid,
            probe,
            class_weight,
            cv_metric,
        )
        runs[name] = {"raw": metrics}
        predictions[(name, "raw")] = pred

    rotated_metrics, rotated_pred = _evaluate_one(
        rotated_train,
        rotated_test,
        y_train,
        y_test,
        seed,
        cv_grid,
        probe,
        class_weight,
        cv_metric,
    )
    runs[rotate]["rotated"] = rotated_metrics
    predictions[(rotate, "rotated")] = rotated_pred

    delta = (
        CE._bootstrap(
            rotated_pred,
            predictions[(rotate, "raw")],
            y_test,
            bootstrap_iters,
            seed,
            "accuracy",
        )
        if bootstrap
        else None
    )
    if delta is not None:
        delta.update(
            {
                "mode": "rotated_minus_raw",
                "run": rotate,
                "target": target,
                "iters": bootstrap_iters,
            }
        )

    gap_raw = runs[target]["raw"]["accuracy"] - runs[rotate]["raw"]["accuracy"]
    gap_after = runs[target]["raw"]["accuracy"] - runs[rotate]["rotated"]["accuracy"]
    return {
        "probe": probe,
        "class_weight": class_weight,
        "cv_metric": cv_metric,
        "rotate": rotate,
        "target": target,
        "direction_cos_before": float(
            source
            @ destination
            / (np.linalg.norm(source) * np.linalg.norm(destination))
        ),
        "direction_cos_after": float(
            _direction(rotated_train, y_train)
            @ destination
            / (
                np.linalg.norm(_direction(rotated_train, y_train))
                * np.linalg.norm(destination)
            )
        ),
        "orthogonality_error": float(
            np.abs(rotation.T @ rotation - np.eye(rotation.shape[0])).max()
        ),
        "runs": runs,
        "gap": {"raw": float(gap_raw), "after": float(gap_after)},
        "bootstrap": [] if delta is None else [delta],
    }


def self_test():
    dim = 8
    source = np.zeros(dim)
    source[0] = 2.0
    target = np.zeros(dim)
    target[1] = 0.5
    rotation = _rotation(source, target)
    mapped = rotation @ source
    assert np.allclose(mapped / np.linalg.norm(mapped), target / np.linalg.norm(target))
    assert np.allclose(rotation.T @ rotation, np.eye(dim), atol=1e-12)
    assert abs(np.linalg.det(rotation)) > 0
    # A vector orthogonal to the plane must stay put.
    fixed = np.zeros(dim)
    fixed[2] = 1.0
    assert np.allclose(rotation @ fixed, fixed)
    # Same direction is the identity.
    assert np.allclose(_rotation(source, source), np.eye(dim))
    print("self-test ok")


def parse_args():
    parser = argparse.ArgumentParser()
    parser.add_argument("--texts", default="")
    parser.add_argument("--run", action="append", default=[])
    parser.add_argument("--rotate", default="")
    parser.add_argument("--to", default="")
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
    if not args.texts or not args.run or not args.rotate or not args.to or not args.out:
        raise SystemExit("--texts, --run, --rotate, --to and --out are required")
    run_arrays = {}
    for spec in args.run:
        name, pattern = spec.split("=", 1)
        run_arrays[name] = GEO.concat(pattern)
    result = evaluate(
        args.texts,
        run_arrays,
        args.rotate,
        args.to,
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
    print(f"done rotate={args.rotate}->{args.to}")


if __name__ == "__main__":
    sys.exit(main())

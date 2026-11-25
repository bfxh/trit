#!/usr/bin/env python3
"""Compare the class-mean direction of two embedding runs.

This is a diagnostic, not a classifier.  For two runs it reports, separately on
the train and test split:

  cos            cosine between the two class-mean difference vectors;
  norm_ratio     ||d_right|| / ||d_left||;
  own_topK       how much of a run's class direction its own top-K PCA basis
                 captures;
  cross_topK     how much of the other run's top-K PCA basis captures it.

The PCA bases are always fitted on the train split, so the test numbers use the
same directions as the probe would see at test time.  The cross_topK numbers
explain why transplanting a PCA basis from one quantization level to another can
fail even when both bases look low-rank in their own run: the class direction
itself may have rotated.

Example:
  python scripts/wsp_classify_direction.py \
    --texts sst2/data-l12/texts.json \
    --run f32='sst2/emb12/l12-pure-f32-*.f32' \
    --run int4='sst2/emb12/l12-int4-*.f32' \
    --out sst2/direction-l12.json
"""

import argparse
import json
import sys
from pathlib import Path

import numpy as np

import wsp_classify_eval as CE
import wsp_classify_geometry as GEO

TOP_K = (1, 2, 4, 8, 16, 32, 64, 128, 256)


def _direction(x, y):
    return np.asarray(x[y == 1].mean(axis=0) - x[y == 0].mean(axis=0), dtype=np.float64)


def _basis(x):
    from sklearn.decomposition import PCA

    pca = PCA(n_components=min(x.shape[0], x.shape[1]), svd_solver="full")
    pca.fit(x)
    return pca.components_


def _share(direction, components, k):
    direction = np.asarray(direction, dtype=np.float64)
    total = float(direction @ direction)
    if total <= 0:
        return None
    k = min(int(k), components.shape[0])
    projections = direction @ components[:k].T
    return float((projections * projections).sum() / total)


def _shares(direction, components):
    return {f"top{k}": _share(direction, components, k) for k in TOP_K}


def _pair(left, right, left_components, right_components):
    left = np.asarray(left, dtype=np.float64)
    right = np.asarray(right, dtype=np.float64)
    left_norm = float(np.linalg.norm(left))
    right_norm = float(np.linalg.norm(right))
    if left_norm == 0 or right_norm == 0:
        cos = None
        angle = None
    else:
        cos = float(left @ right / (left_norm * right_norm))
        cos = max(-1.0, min(1.0, cos))
        angle = float(np.degrees(np.arccos(cos)))
    return {
        "cos": cos,
        "angle_deg": angle,
        "left_norm": left_norm,
        "right_norm": right_norm,
        "norm_ratio": None if left_norm == 0 else right_norm / left_norm,
        "left_own": _shares(left, left_components),
        "right_own": _shares(right, right_components),
        "left_in_right": _shares(left, right_components),
        "right_in_left": _shares(right, left_components),
    }


def evaluate(texts_path, run_arrays):
    texts = json.loads(Path(texts_path).read_text(encoding="utf-8"))["texts"]
    train_idx, test_idx, y_train, y_test = CE._labels(texts)
    y_train = np.asarray(y_train, dtype=np.int64)
    y_test = np.asarray(y_test, dtype=np.int64)
    directions = {}
    components = {}
    for name, matrix in run_arrays.items():
        train_x = matrix[train_idx]
        directions[name] = {
            "train": _direction(train_x, y_train),
            "test": _direction(matrix[test_idx], y_test),
        }
        components[name] = _basis(train_x)
    names = list(run_arrays)
    pairs = []
    for i, left in enumerate(names):
        for right in names[i + 1 :]:
            pair = {"left": left, "right": right}
            for split in ("train", "test"):
                pair[split] = _pair(
                    directions[left][split],
                    directions[right][split],
                    components[left],
                    components[right],
                )
            pairs.append(pair)
    return {
        "runs": {
            name: {
                "direction_norm_train": float(np.linalg.norm(d["train"])),
                "direction_norm_test": float(np.linalg.norm(d["test"])),
            }
            for name, d in directions.items()
        },
        "pairs": pairs,
    }


def self_test():
    dim = 16
    eye = np.eye(dim)
    right_basis = np.vstack([eye[1], eye[0], eye[2:]])
    left = np.zeros(dim)
    left[0] = 2.0
    right = np.zeros(dim)
    right[1] = 1.0
    pair = _pair(left, right, eye, right_basis)
    assert abs(pair["cos"]) < 1e-12
    assert abs(pair["angle_deg"] - 90.0) < 1e-9
    assert abs(pair["left_in_right"]["top1"]) < 1e-12
    assert abs(pair["right_in_left"]["top1"]) < 1e-12
    assert abs(pair["left_own"]["top1"] - 1.0) < 1e-12

    same = _pair(left, left, eye, eye)
    assert abs(same["cos"] - 1.0) < 1e-12
    assert abs(same["left_in_right"]["top1"] - 1.0) < 1e-12
    assert same["norm_ratio"] is not None
    assert abs(same["norm_ratio"] - 1.0) < 1e-12
    print("self-test ok")


def parse_args():
    parser = argparse.ArgumentParser()
    parser.add_argument("--texts", default="")
    parser.add_argument("--run", action="append", default=[])
    parser.add_argument("--out", default="")
    parser.add_argument("--self-test", action="store_true")
    return parser.parse_args()


def main():
    args = parse_args()
    if args.self_test:
        self_test()
        return
    if not args.texts or len(args.run) < 1 or not args.out:
        raise SystemExit("--texts, --run and --out are required")
    run_arrays = {}
    for spec in args.run:
        name, pattern = spec.split("=", 1)
        run_arrays[name] = GEO.concat(pattern)
    result = evaluate(args.texts, run_arrays)
    Path(args.out).write_text(
        json.dumps(result, indent=2, ensure_ascii=False), encoding="utf-8"
    )
    print(f"done runs={len(result['runs'])} pairs={len(result['pairs'])}")


if __name__ == "__main__":
    sys.exit(main())

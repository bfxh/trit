#!/usr/bin/env python3
"""Describe the geometry of frozen sentence embeddings on a binary task.

The metrics are diagnostic, not a classifier: class-centroid separation,
within-class scatter, covariance effective rank, and how much of the class-mean
direction is captured by the top principal components.  They are meant to
explain why the same linear probe succeeds on one dataset and fails on another.

Example:
  python scripts/wsp_classify_geometry.py \
    --texts sst2/data-l12/texts.json \
    --run l12-f32='sst2/emb12/l12-pure-f32-*.f32' \
    --run l12-int4='sst2/emb12/l12-int4-*.f32' \
    --out sst2/geometry-l12.json
"""

import argparse
import glob
import json
import struct
import sys
import tempfile
from pathlib import Path

MAGIC = b"WSPEMB1\0"
EXPECTED_COLS = 384
TOP_K = (1, 2, 4, 8, 16, 32, 64)


def _read_embedding(path):
    import numpy as np

    with open(path, "rb") as handle:
        head = handle.read(16)
        if len(head) != 16 or head[:8] != MAGIC:
            raise ValueError(f"{path}: bad WSPEMB1 header")
        rows, cols = (int(x) for x in struct.unpack("<II", head[8:]))
        data = np.fromfile(handle, dtype=np.float32)
    if data.size != rows * cols:
        raise ValueError(f"{path}: {data.size} floats != {rows}x{cols}")
    return data.reshape(rows, cols)


def concat(pattern):
    import numpy as np

    paths = sorted(glob.glob(pattern))
    if not paths:
        raise ValueError(f"no files match {pattern!r}")
    parts = [_read_embedding(path) for path in paths]
    cols = {part.shape[1] for part in parts}
    if cols != {EXPECTED_COLS}:
        raise ValueError(f"{pattern}: columns {cols} != {EXPECTED_COLS}")
    return np.vstack(parts)


def _labels(texts):
    import numpy as np

    train = [row["index"] for row in texts if row["split"] == "train"]
    test = [row["index"] for row in texts if row["split"] == "test"]
    y_train = np.asarray(
        [row["label"] for row in texts if row["split"] == "train"], dtype=np.int64
    )
    y_test = np.asarray(
        [row["label"] for row in texts if row["split"] == "test"], dtype=np.int64
    )
    return train, test, y_train, y_test


def _svd_components(x):
    import numpy as np

    centered = x - x.mean(axis=0)
    _, singular, components = np.linalg.svd(centered, full_matrices=False)
    return singular, components


def geometry(x, y, signal_components=None):
    """Return scale-free geometry diagnostics for one embedding matrix.

    ``signal_components`` lets a test split reuse the train PCA basis; variance
    geometry (effective rank, top share) is always measured on ``x`` itself.
    """
    import numpy as np

    x = np.asarray(x, dtype=np.float64)
    y = np.asarray(y, dtype=np.int64)
    classes = np.unique(y)
    if classes.tolist() != [0, 1]:
        raise ValueError(f"binary labels required, got {classes.tolist()}")

    mu0 = x[y == 0].mean(axis=0)
    mu1 = x[y == 1].mean(axis=0)
    d0 = x[y == 0] - mu0
    d1 = x[y == 1] - mu1
    within0 = float((d0 * d0).sum(axis=1).mean())
    within1 = float((d1 * d1).sum(axis=1).mean())
    n0, n1 = float((y == 0).sum()), float((y == 1).sum())
    within = (within0 * n0 + within1 * n1) / (n0 + n1)
    between = float(((mu1 - mu0) ** 2).sum())
    sep = float(np.sqrt(between / within)) if within > 0 else float("inf")

    # PCA via SVD of the centered matrix: components are the right singular
    # vectors, explained variance is s**2.
    singular, components = _svd_components(x)
    variance = singular * singular
    weights = variance / variance.sum()
    positive = weights[weights > 0]
    effective_rank = float(np.exp(-(positive * np.log(positive)).sum()))
    participation = float((singular.sum() ** 2) / variance.sum())
    if signal_components is not None:
        components = signal_components

    direction = mu1 - mu0
    total = float(direction @ direction)
    projections = (components @ direction) ** 2
    captures = {
        f"signal_top{k}": float(projections[:k].sum() / total) if total > 0 else None
        for k in TOP_K
    }

    return {
        "n": int(len(y)),
        "dim": int(x.shape[1]),
        "class_counts": [int(n0), int(n1)],
        "mean_norm": float(np.linalg.norm(x, axis=1).mean()),
        "centroid_norm0": float(np.linalg.norm(mu0)),
        "centroid_norm1": float(np.linalg.norm(mu1)),
        "centroid_cos": float(mu0 @ mu1 / (np.linalg.norm(mu0) * np.linalg.norm(mu1))),
        "between_sq": between,
        "within_sq": within,
        "fisher": between / within if within > 0 else None,
        "sep": sep,
        "eff_rank": effective_rank,
        "participation_ratio": participation,
        "top1_share": float(weights[0]),
        **captures,
    }


def evaluate_geometry(texts_path, run_specs):
    texts = json.loads(Path(texts_path).read_text(encoding="utf-8"))["texts"]
    train_idx, test_idx, y_train, y_test = _labels(texts)
    runs = {}
    for spec in run_specs:
        name, pattern = spec.split("=", 1)
        matrix = concat(pattern)
        if matrix.shape[0] != len(texts):
            raise ValueError(f"{name}: rows {matrix.shape[0]} != texts {len(texts)}")
        runs[name] = matrix

    results = {"runs": {}}
    for name, matrix in runs.items():
        train_x = matrix[train_idx]
        _, train_components = _svd_components(train_x)
        results["runs"][name] = {
            "train": geometry(train_x, y_train),
            "test": geometry(matrix[test_idx], y_test, train_components),
        }
    return results


def self_test():
    import numpy as np

    rng = np.random.default_rng(11)
    n, dim = 240, 32
    y = np.asarray([0, 1] * (n // 2), dtype=np.int64)
    x = rng.normal(scale=0.12, size=(n, dim))
    weights = np.asarray([1.0, 0.9, 0.8, 0.7, 0.5, 0.4, 0.3, 0.2] + [0.0] * (dim - 8))
    x = x + y[:, None] * weights[None, :] * 2.0

    result = geometry(x, y)
    assert result["n"] == n and result["dim"] == dim
    assert result["sep"] > 2.0
    assert result["eff_rank"] > 1.0
    assert 0.0 < result["top1_share"] <= 1.0
    assert result["signal_top1"] < result["signal_top4"] < result["signal_top16"] <= 1.0
    assert result["centroid_cos"] > -1.0

    bad = Path(tempfile.mkdtemp(prefix="geometry-self-")) / "bad.f32"
    bad.write_bytes(b"WSPEMB2\0" + b"\0" * 8)
    try:
        _read_embedding(bad)
    except ValueError:
        pass
    else:
        raise AssertionError("bad WSPEMB1 magic must be rejected")
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
    if not args.texts or not args.run or not args.out:
        raise SystemExit("--texts, --run and --out are required")
    result = evaluate_geometry(args.texts, args.run)
    Path(args.out).write_text(
        json.dumps(result, indent=2, ensure_ascii=False), encoding="utf-8"
    )
    print(f"done runs={len(result['runs'])}")


if __name__ == "__main__":
    sys.exit(main())

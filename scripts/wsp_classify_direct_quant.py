#!/usr/bin/env python3
"""Direct int4 quantization of frozen embeddings as a negative control.

M1.23 probes the residual left by the encoder's int4 **weights**:
``r = x_int4 - x_f32``.  This script asks a narrower question: if the same
uniform int4 quantizer is applied directly to the frozen f32 embedding, does
that residual also carry a class-conditional bias?

It is a representation-level surrogate, not the weight quantizer.  A positive
result would mean the bias is generic to int4 rounding of this representation;
a negative result means the bias depends on how weight quantization errors
propagate through the encoder.

Only linear probes are trained; the encoder is frozen.  C-selection follows the
classification-head protocol (5-fold inside train).
"""

import argparse
import glob
import json
import struct
import sys
import tempfile
from pathlib import Path

import numpy as np

import wsp_classify_eval as CE

MAGIC = b"WSPEMB1\0"
EXPECTED_COLS = 384
DEFAULT_GROUPS = (1, 32, 384)
BOOTSTRAP_METRICS = ("accuracy", "macro_f1")


def _read_embedding(path):
    with open(path, "rb") as handle:
        head = handle.read(16)
        if len(head) != 16 or head[:8] != MAGIC:
            raise ValueError(f"{path}: bad WSPEMB1 header")
        rows, cols = (int(x) for x in struct.unpack("<II", head[8:]))
        data = np.fromfile(handle, dtype=np.float32)
    if data.size != rows * cols:
        raise ValueError(f"{path}: {data.size} floats != {rows}x{cols}")
    return data.reshape(rows, cols)


def _concat(pattern):
    paths = sorted(glob.glob(pattern))
    if not paths:
        raise ValueError(f"no files match {pattern!r}")
    parts = [_read_embedding(path) for path in paths]
    cols = {part.shape[1] for part in parts}
    if cols != {EXPECTED_COLS}:
        raise ValueError(f"{pattern}: columns {cols} != {EXPECTED_COLS}")
    return np.vstack(parts)


def _labels(texts):
    train = [row["index"] for row in texts if row["split"] == "train"]
    test = [row["index"] for row in texts if row["split"] == "test"]
    y_train = np.asarray(
        [row["label"] for row in texts if row["split"] == "train"], dtype=np.int64
    )
    y_test = np.asarray(
        [row["label"] for row in texts if row["split"] == "test"], dtype=np.int64
    )
    return train, test, y_train, y_test


def _quantize(x, group):
    """Symmetric per-group int4 RTN, matching `linear/uniform.rs`."""
    x = np.asarray(x, dtype=np.float64)
    n, dim = x.shape
    if group <= 0:
        group = dim
    group = min(group, dim)
    if dim % group != 0:
        raise ValueError(f"dim {dim} not divisible by group {group}")
    blocks = x.reshape(n, dim // group, group)
    scale = np.abs(blocks).max(axis=2, keepdims=True) / 7.0
    q = np.clip(np.round(blocks / np.maximum(scale, 1e-12)), -7, 7) * scale
    return q.reshape(n, dim)


def _residual(x, q):
    return q - x


def _fit(train_x, test_x, y_train, y_test, probe, class_weight, cv_metric):
    chosen, cv = CE._select_c(
        train_x, y_train, 7, (0.01, 0.1, 1.0, 10.0), probe, cv_metric, class_weight
    )
    model, scaler = CE._probe(train_x, y_train, 7, chosen, False, probe, class_weight)
    metrics = CE._metrics(model, scaler, test_x, y_test)
    return model, metrics, chosen, cv


def _class_mean_stats(x, r, y):
    mu_x = {label: x[y == label].mean(axis=0) for label in (0, 1)}
    mu_r = {label: r[y == label].mean(axis=0) for label in (0, 1)}
    d_x = mu_x[1] - mu_x[0]
    d_r = mu_r[1] - mu_r[0]
    return {
        "f32_between_norm": float(np.linalg.norm(d_x)),
        "residual_between_norm": float(np.linalg.norm(d_r)),
        "residual_to_f32_ratio": float(
            np.linalg.norm(d_r) / max(np.linalg.norm(d_x), 1e-12)
        ),
        "cos_f32_residual": float(
            d_x @ d_r / max(np.linalg.norm(d_x) * np.linalg.norm(d_r), 1e-12)
        ),
    }


def _probe_margin(x, r, y, model):
    w = np.asarray(model.coef_[0], dtype=np.float64)
    d_x = x[y == 1].mean(axis=0) - x[y == 0].mean(axis=0)
    d_r = r[y == 1].mean(axis=0) - r[y == 0].mean(axis=0)
    gap_x = float(w @ d_x)
    gap_r = float(w @ d_r)
    return {
        "f32_gap": gap_x,
        "residual_gap": gap_r,
        "residual_to_f32_gap": gap_r / gap_x if abs(gap_x) > 1e-12 else None,
        "cos_probe_residual": float(
            gap_r / max(np.linalg.norm(w) * np.linalg.norm(d_r), 1e-12)
        ),
    }


def evaluate(
    texts_path,
    f32,
    groups=DEFAULT_GROUPS,
    probe="logreg",
    class_weight=None,
    cv_metric="accuracy",
    bootstrap=0,
    bootstrap_iters=10000,
):
    texts = json.loads(Path(texts_path).read_text(encoding="utf-8"))["texts"]
    train, test, y_train, y_test = _labels(texts)
    x = _concat(f32)
    tr, te = x[train], x[test]

    f32_model, f32_metrics, f32_c, f32_cv = _fit(
        tr, te, y_train, y_test, probe, class_weight, cv_metric
    )
    rows = []
    for group in groups:
        q_tr, q_te = _quantize(tr, group), _quantize(te, group)
        r_tr, r_te = _residual(tr, q_tr), _residual(te, q_te)
        _, q_metrics, q_c, q_cv = _fit(
            q_tr, q_te, y_train, y_test, probe, class_weight, cv_metric
        )
        _, r_metrics, r_c, r_cv = _fit(
            r_tr, r_te, y_train, y_test, probe, class_weight, cv_metric
        )
        record = {
            "group": group,
            "f32": {
                "accuracy": f32_metrics["accuracy"],
                "macro_f1": f32_metrics["macro_f1"],
                "selected_C": f32_c,
                "cv": f32_cv,
            },
            "quantized": {
                "accuracy": q_metrics["accuracy"],
                "macro_f1": q_metrics["macro_f1"],
                "selected_C": q_c,
                "cv": q_cv,
            },
            "residual": {
                "accuracy": r_metrics["accuracy"],
                "macro_f1": r_metrics["macro_f1"],
                "selected_C": r_c,
                "cv": r_cv,
            },
            "class_mean": _class_mean_stats(tr, r_tr, y_train),
            "probe_margin": _probe_margin(tr, r_tr, y_train, f32_model),
            "bootstrap": [],
        }
        if bootstrap:
            for metric in BOOTSTRAP_METRICS:
                record["bootstrap"].append(
                    {
                        "pair": "quantized-f32",
                        **CE._bootstrap(
                            q_metrics["prediction"],
                            f32_metrics["prediction"],
                            y_test,
                            bootstrap_iters,
                            7,
                            metric,
                        ),
                    }
                )
                record["bootstrap"].append(
                    {
                        "pair": "residual-f32",
                        **CE._bootstrap(
                            r_metrics["prediction"],
                            f32_metrics["prediction"],
                            y_test,
                            bootstrap_iters,
                            7,
                            metric,
                        ),
                    }
                )
        rows.append(record)
    return {
        "probe": probe,
        "class_weight": class_weight or "none",
        "cv_metric": cv_metric,
        "groups": rows,
    }


def self_test():
    x = np.array(
        [
            [0.0, 0.4, -0.7, 1.2, -0.1, 0.03],
            [0.2, -0.5, 0.8, -1.1, 0.04, -0.02],
        ],
        dtype=np.float64,
    )
    q1 = _quantize(x, 1)
    assert np.allclose(q1, x), "group=1 must be exact (per-element scale)"
    q2 = _quantize(x, 3)
    assert q2.shape == x.shape
    blocks = x.reshape(x.shape[0], -1, 3)
    assert np.all(
        np.abs(q2.reshape(x.shape[0], -1, 3))
        <= np.abs(blocks).max(axis=2, keepdims=True) + 1e-12
    )
    assert np.allclose(_residual(x, q2), q2 - x)
    # per-row scale: residual no larger than half a group step
    row_scale = np.abs(x).max(axis=1, keepdims=True) / 7.0
    qrow = _quantize(x, x.shape[1])
    assert np.all(np.abs(qrow - x) <= 0.5 * row_scale + 1e-12)
    # class-mean helper is finite and scale-free
    y = np.array([0, 1])
    stats = _class_mean_stats(x, _residual(x, qrow), y)
    assert stats["f32_between_norm"] > 0
    assert -1.0 <= stats["cos_f32_residual"] <= 1.0
    # exercise evaluate() on a tiny random set; only shape/finite checks
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        rng = np.random.default_rng(7)
        n_train, n_test, dim = 48, 24, 384
        emb = rng.normal(size=(n_train + n_test, dim)).astype(np.float32)
        path = root / "mini.f32"
        with open(path, "wb") as handle:
            handle.write(MAGIC)
            handle.write(struct.pack("<II", n_train + n_test, dim))
            emb.tofile(handle)
        texts = []
        for i in range(n_train):
            texts.append(
                {"index": i, "split": "train", "label": int(i % 2), "text": "x"}
            )
        for i in range(n_test):
            texts.append(
                {
                    "index": n_train + i,
                    "split": "test",
                    "label": int(i % 2),
                    "text": "x",
                }
            )
        (root / "texts.json").write_text(json.dumps({"texts": texts}), encoding="utf-8")
        result = evaluate(root / "texts.json", str(path), groups=(1, 32), bootstrap=0)
        assert len(result["groups"]) == 2
        for row in result["groups"]:
            for arm in ("f32", "quantized", "residual"):
                assert np.isfinite(row[arm]["accuracy"])
                assert 0.0 <= row[arm]["accuracy"] <= 1.0
    print("self-test ok")


def parse_args():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--texts")
    parser.add_argument("--f32")
    parser.add_argument("--groups", default="1,32,384")
    parser.add_argument("--probe", default="logreg", choices=("logreg", "linear_svc"))
    parser.add_argument("--class-weight", choices=("balanced",))
    parser.add_argument(
        "--cv-metric", default="accuracy", choices=("accuracy", "macro_f1")
    )
    parser.add_argument("--bootstrap", type=int, default=0)
    parser.add_argument("--bootstrap-iters", type=int, default=10000)
    parser.add_argument("--out")
    parser.add_argument("--self-test", action="store_true")
    return parser.parse_args()


def main():
    args = parse_args()
    if args.self_test:
        self_test()
        return
    assert args.texts and args.f32 and args.out, "--texts/--f32/--out 必填"
    groups = tuple(int(item) for item in args.groups.split(",") if item.strip())
    result = evaluate(
        args.texts,
        args.f32,
        groups=groups,
        probe=args.probe,
        class_weight=args.class_weight,
        cv_metric=args.cv_metric,
        bootstrap=args.bootstrap,
        bootstrap_iters=args.bootstrap_iters,
    )
    output = Path(args.out)
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")
    print(f"写入 {output}")


if __name__ == "__main__":
    main()

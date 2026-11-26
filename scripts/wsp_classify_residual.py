#!/usr/bin/env python3
"""Probe the quantization residual left by int4 on frozen embeddings.

The int4 embedding is written as ``x_int4 = x_f32 + r``.  A linear probe on
``r`` alone tests whether the rounding residual is itself a class-bearing
feature.  PCA is fit on the f32 train split only, and ``r`` is split into its
top-k and remaining components:

  hybrid_rest_k = x_f32 + r_rest_k   (int4 residual outside top-k)
  hybrid_top_k  = x_f32 + r_top_k    (int4 residual inside top-k)
  resid_rest_k  = r_rest_k
  resid_top_k   = r_top_k

At k=0 ``hybrid_rest`` is exactly int4 and ``hybrid_top`` is f32; at k=dim the
roles reverse.  Controls at the primary k are row-normalized residual direction,
sign, per-row norm, row permutation, within-class permutation, Gaussian noise
matched to the residual standard deviation, and residual scaling.

Only a linear probe is trained; the encoder is frozen.  The C-selection
protocol is the same as the classification head (5-fold inside train).

Test-side class-mean controls use the base f32 probe's prediction, never the
true test label.  Pass ``--legacy-controls`` only to reproduce the M1.23
oracle table, whose controls leaked ``y_test``.

Example:
  python scripts/wsp_classify_residual.py \
    --texts sst2/data-l12/texts.json \
    --f32 'sst2/emb12/l12-pure-f32-*.f32' \
    --int4 'sst2/emb12/l12-int4-*.f32' \
    --bootstrap 1 --bootstrap-iters 10000 \
    --out sst2/residual-l12.json
"""

import argparse
import json
import sys
import tempfile
from pathlib import Path

import numpy as np

import wsp_classify_eval as CE
import wsp_classify_geometry as GEO
import wsp_classify_pca as PCA

DEFAULT_KS = (4, 16, 64, 384)
PRIMARY_K = 16
CONTROL_SCALES = (0.5, 1.0, 2.0)
GEOMETRY_KEYS = ("sep", "fisher", "eff_rank", "top1_share", "signal_top4")


def _top_residual(r, components, k):
    """Project residual onto the first k PCA directions; exact at 0 and dim."""
    if k <= 0:
        return np.zeros_like(r)
    if k >= components.shape[0]:
        return r.copy()
    return (r @ components[:k].T) @ components[:k]


def _row_l2(r):
    norms = np.linalg.norm(r, axis=1, keepdims=True)
    return r / np.maximum(norms, 1e-12)


def _within_class_permutation(y, rng):
    index = np.arange(len(y))
    order = index.copy()
    for label in (0, 1):
        members = index[y == label]
        order[members] = rng.permutation(members)
    return order


def _evaluate_arm(
    train_x,
    test_x,
    y_train,
    y_test,
    seed,
    cv_grid,
    probe,
    class_weight,
    cv_metric,
    with_geometry=False,
):
    train_x = np.asarray(train_x, dtype=np.float64)
    test_x = np.asarray(test_x, dtype=np.float64)
    if train_x.ndim == 1:
        train_x = train_x[:, None]
        test_x = test_x[:, None]
    chosen, cv_score = CE._select_c(
        train_x, y_train, seed, cv_grid, probe, cv_metric, class_weight
    )
    model, scaler = CE._probe(
        train_x, y_train, seed, chosen, False, probe, class_weight
    )
    metrics = CE._metrics(model, scaler, test_x, y_test)
    train_metrics = CE._metrics(model, scaler, train_x, y_train)
    record = {
        "accuracy": metrics["accuracy"],
        "macro_f1": metrics["macro_f1"],
        "auc": metrics["auc"],
        "train_accuracy": train_metrics["accuracy"],
        "selected_C": chosen,
        "cv": cv_score,
    }
    if with_geometry:
        geometry = GEO.geometry(test_x, y_test)
        record.update({f"test_{key}": geometry[key] for key in GEOMETRY_KEYS})
    return record, metrics["prediction"]


def _class_mean_stats(f32, int4, y):
    """Class-mean shift summary for f32, int4, and the residual."""
    r = int4 - f32
    mu_f = {label: f32[y == label].mean(axis=0) for label in (0, 1)}
    mu_q = {label: int4[y == label].mean(axis=0) for label in (0, 1)}
    mu_r = {label: r[y == label].mean(axis=0) for label in (0, 1)}
    d_f = mu_f[1] - mu_f[0]
    d_q = mu_q[1] - mu_q[0]
    d_r = mu_r[1] - mu_r[0]
    residual_geometry = GEO.geometry(r, y)
    return {
        "f32_between_norm": float(np.linalg.norm(d_f)),
        "int4_between_norm": float(np.linalg.norm(d_q)),
        "residual_between_norm": float(np.linalg.norm(d_r)),
        "residual_to_f32_ratio": float(
            np.linalg.norm(d_r) / max(np.linalg.norm(d_f), 1e-12)
        ),
        "cos_f32_residual": float(
            d_f @ d_r / max(np.linalg.norm(d_f) * np.linalg.norm(d_r), 1e-12)
        ),
        "cos_f32_int4": float(
            d_f @ d_q / max(np.linalg.norm(d_f) * np.linalg.norm(d_q), 1e-12)
        ),
        "residual_sep": residual_geometry["sep"],
        "residual_fisher": residual_geometry["fisher"],
        "residual_between_sq": residual_geometry["between_sq"],
        "residual_within_sq": residual_geometry["within_sq"],
        "residual_eff_rank": residual_geometry["eff_rank"],
    }


def _probe_margin_stats(f32, int4, y, model):
    """Project class-mean shifts onto the f32 probe weight direction."""
    r = int4 - f32
    w = np.asarray(model.coef_[0], dtype=np.float64)
    d_f = f32[y == 1].mean(axis=0) - f32[y == 0].mean(axis=0)
    d_r = r[y == 1].mean(axis=0) - r[y == 0].mean(axis=0)
    d_q = int4[y == 1].mean(axis=0) - int4[y == 0].mean(axis=0)
    gap_f = float(w @ d_f)
    gap_r = float(w @ d_r)
    gap_q = float(w @ d_q)
    return {
        "f32_gap": gap_f,
        "residual_gap": gap_r,
        "int4_gap": gap_q,
        "residual_to_f32_gap": gap_r / gap_f if abs(gap_f) > 1e-12 else None,
        "cos_probe_residual": float(
            gap_r / max(np.linalg.norm(w) * np.linalg.norm(d_r), 1e-12)
        ),
        "cos_probe_f32": float(
            gap_f / max(np.linalg.norm(w) * np.linalg.norm(d_f), 1e-12)
        ),
        "probe_norm": float(np.linalg.norm(w)),
    }


def evaluate(
    texts_path,
    f32,
    int4,
    ks=DEFAULT_KS,
    primary_k=PRIMARY_K,
    seed=7,
    cv_grid=(0.01, 0.1, 1.0, 10.0),
    probe="logreg",
    class_weight=None,
    cv_metric="accuracy",
    bootstrap=0,
    bootstrap_iters=10000,
    strict_controls=True,
):
    texts = json.loads(Path(texts_path).read_text(encoding="utf-8"))["texts"]
    train_idx, test_idx, y_train, y_test = CE._labels(texts)
    y_train = np.asarray(y_train, dtype=np.int64)
    y_test = np.asarray(y_test, dtype=np.int64)
    f32 = np.asarray(f32, dtype=np.float64)
    int4 = np.asarray(int4, dtype=np.float64)
    if f32.shape != int4.shape:
        raise ValueError(f"shape mismatch: {f32.shape} vs {int4.shape}")
    if f32.shape[0] != len(texts):
        raise ValueError(f"rows {f32.shape[0]} != texts {len(texts)}")
    if primary_k not in ks:
        raise ValueError(f"primary_k {primary_k} not in ks {ks}")

    f32_train, f32_test = f32[train_idx], f32[test_idx]
    int4_train, int4_test = int4[train_idx], int4[test_idx]
    r_train, r_test = int4_train - f32_train, int4_test - f32_test
    pca = PCA._fit_pca(f32_train)
    components = pca.components_

    arms = {
        "f32": (f32_train, f32_test, True),
        "int4": (int4_train, int4_test, True),
    }
    for k in ks:
        top_train = _top_residual(r_train, components, k)
        top_test = _top_residual(r_test, components, k)
        rest_train, rest_test = r_train - top_train, r_test - top_test
        arms[f"hybrid_rest_k{k}"] = (
            f32_train + rest_train,
            f32_test + rest_test,
            True,
        )
        arms[f"hybrid_top_k{k}"] = (f32_train + top_train, f32_test + top_test, True)
        arms[f"resid_rest_k{k}"] = (rest_train, rest_test, False)
        arms[f"resid_top_k{k}"] = (top_train, top_test, False)

    top_train = _top_residual(r_train, components, primary_k)
    top_test = _top_residual(r_test, components, primary_k)
    rest_train, rest_test = r_train - top_train, r_test - top_test
    arms["resid_full"] = (r_train, r_test, True)
    arms["resid_row_l2"] = (_row_l2(r_train), _row_l2(r_test), False)
    arms["resid_sign"] = (np.sign(r_train), np.sign(r_test), False)
    arms["resid_norm"] = (
        np.linalg.norm(r_train, axis=1),
        np.linalg.norm(r_test, axis=1),
        False,
    )

    rng = np.random.default_rng(seed + 101)
    class_mean_train = {
        label: rest_train[y_train == label].mean(axis=0) for label in (0, 1)
    }
    if strict_controls:
        # Use a label-free proxy at test time: the base f32 probe's prediction.
        # No true test label may enter the feature construction.
        base_c, _ = CE._select_c(
            f32_train, y_train, seed, cv_grid, probe, cv_metric, class_weight
        )
        base_model, base_scaler = CE._probe(
            f32_train, y_train, seed, base_c, False, probe, class_weight
        )
        proxy_test = base_model.predict(
            base_scaler.transform(f32_test) if base_scaler is not None else f32_test
        )
    else:
        proxy_test = y_test
    centered_train = rest_train - np.vstack(
        [class_mean_train[label] for label in y_train]
    )
    if strict_controls:
        centered_test = rest_test - np.vstack(
            [class_mean_train[label] for label in proxy_test]
        )
    else:
        # M1.23 oracle path: test class means are estimated from true test labels.
        class_mean_test = {
            label: rest_test[y_test == label].mean(axis=0) for label in (0, 1)
        }
        centered_test = rest_test - np.vstack(
            [class_mean_test[label] for label in y_test]
        )
    arms["hybrid_rest_noclassmean"] = (
        f32_train + centered_train,
        f32_test + centered_test,
        False,
    )
    arms["resid_rest_noclassmean"] = (centered_train, centered_test, False)

    perm = rng.permutation(len(y_train))
    perm_test = rng.permutation(len(y_test))
    arms["hybrid_rest_perm"] = (
        f32_train + rest_train[perm],
        f32_test + rest_test[perm_test],
        False,
    )
    class_perm = _within_class_permutation(y_train, rng)
    class_perm_test = _within_class_permutation(
        proxy_test if strict_controls else y_test, rng
    )
    arms["hybrid_rest_perm_class"] = (
        f32_train + rest_train[class_perm],
        f32_test + rest_test[class_perm_test],
        False,
    )

    sigma = rest_train.std(axis=0, ddof=0)
    gauss_train = rng.normal(size=rest_train.shape) * sigma
    gauss_test = rng.normal(size=rest_test.shape) * sigma
    arms["hybrid_gauss_0.5"] = (
        f32_train + 0.5 * gauss_train,
        f32_test + 0.5 * gauss_test,
        False,
    )
    arms["hybrid_gauss_1"] = (f32_train + gauss_train, f32_test + gauss_test, False)
    arms["hybrid_gauss_2"] = (
        f32_train + 2.0 * gauss_train,
        f32_test + 2.0 * gauss_test,
        False,
    )
    arms["hybrid_scale_0.5"] = (
        f32_train + 0.5 * rest_train,
        f32_test + 0.5 * rest_test,
        False,
    )
    arms["hybrid_scale_2"] = (
        f32_train + 2.0 * rest_train,
        f32_test + 2.0 * rest_test,
        False,
    )
    arms["hybrid_neg"] = (f32_train - rest_train, f32_test - rest_test, False)

    rows = {}
    predictions = {}
    for name, (train_x, test_x, geometry) in arms.items():
        record, pred = _evaluate_arm(
            train_x,
            test_x,
            y_train,
            y_test,
            seed,
            cv_grid,
            probe,
            class_weight,
            cv_metric,
            geometry,
        )
        rows[name] = record
        predictions[name] = pred

    f32_model, _ = CE._probe(
        f32_train, y_train, seed, rows["f32"]["selected_C"], False, probe, class_weight
    )
    probe_margin = _probe_margin_stats(f32_train, int4_train, y_train, f32_model)

    pairs = [("int4", "f32")]
    for k in ks:
        pairs.append((f"hybrid_rest_k{k}", "int4"))
    pairs.extend(
        [
            (f"hybrid_rest_k{primary_k}", "f32"),
            (f"hybrid_top_k{primary_k}", "f32"),
            ("resid_full", "f32"),
            (f"resid_rest_k{primary_k}", "f32"),
            ("resid_row_l2", "f32"),
            ("resid_sign", "f32"),
            ("resid_norm", "f32"),
            ("hybrid_rest_perm", f"hybrid_rest_k{primary_k}"),
            ("hybrid_rest_perm_class", f"hybrid_rest_k{primary_k}"),
            ("hybrid_rest_noclassmean", f"hybrid_rest_k{primary_k}"),
            ("resid_rest_noclassmean", "f32"),
            ("hybrid_gauss_1", f"hybrid_rest_k{primary_k}"),
            ("hybrid_scale_0.5", f"hybrid_rest_k{primary_k}"),
            ("hybrid_scale_2", f"hybrid_rest_k{primary_k}"),
            ("hybrid_neg", f"hybrid_rest_k{primary_k}"),
        ]
    )
    boot = []
    if bootstrap:
        for left, right in pairs:
            for metric in ("accuracy", "macro_f1"):
                pair = CE._bootstrap(
                    predictions[left],
                    predictions[right],
                    y_test,
                    bootstrap_iters,
                    seed,
                    metric,
                )
                pair.update(
                    {
                        "left": left,
                        "right": right,
                        "delta_convention": "left_minus_right",
                        "iters": bootstrap_iters,
                    }
                )
                boot.append(pair)

    return {
        "probe": probe,
        "class_weight": class_weight,
        "cv_metric": cv_metric,
        "strict_controls": bool(strict_controls),
        "primary_k": int(primary_k),
        "ks": [int(k) for k in ks],
        "n_train": int(len(y_train)),
        "n_test": int(len(y_test)),
        "pca_components": int(pca.n_components_),
        "train_class_mean": _class_mean_stats(f32_train, int4_train, y_train),
        "test_class_mean": _class_mean_stats(f32_test, int4_test, y_test),
        "probe_margin": probe_margin,
        "arms": rows,
        "bootstrap": boot,
    }


def self_test():
    rng = np.random.default_rng(31)
    n_train, n_test, dim = 120, 60, CE.EXPECTED_COLS
    y_train = np.asarray([0, 1] * (n_train // 2), dtype=np.int64)
    y_test = np.asarray([0, 1] * (n_test // 2), dtype=np.int64)
    sign_train = (2 * y_train - 1)[:, None]
    sign_test = (2 * y_test - 1)[:, None]

    f32_train = rng.normal(scale=0.8, size=(n_train, dim))
    f32_test = rng.normal(scale=0.8, size=(n_test, dim))
    weights = np.zeros(dim)
    weights[:8] = np.asarray([1.0, 0.9, 0.8, 0.7, 0.5, 0.4, 0.3, 0.2]) * 0.6
    f32_train += sign_train * weights[None, :]
    f32_test += sign_test * weights[None, :]

    # Residual direction is class-specific in low-variance dimensions; its
    # per-row norm is identical for both classes.
    pattern = np.zeros(dim)
    pattern[32:48] = [0.4, -0.3, 0.25, -0.2, 0.15, -0.1, 0.05, -0.05] * 2
    r_train = rng.normal(scale=0.01, size=(n_train, dim))
    r_test = rng.normal(scale=0.01, size=(n_test, dim))
    r_train += sign_train * pattern[None, :]
    r_test += sign_test * pattern[None, :]
    int4_train, int4_test = f32_train + r_train, f32_test + r_test

    rows = [
        {"index": i, "split": "train", "label": int(y_train[i])} for i in range(n_train)
    ] + [
        {"index": n_train + i, "split": "test", "label": int(y_test[i])}
        for i in range(n_test)
    ]
    with tempfile.TemporaryDirectory() as tmp:
        texts = Path(tmp) / "texts.json"
        texts.write_text(json.dumps({"texts": rows}), encoding="utf-8")
        result = evaluate(
            texts,
            np.vstack([f32_train, f32_test]),
            np.vstack([int4_train, int4_test]),
            ks=(16, 384),
            primary_k=16,
            bootstrap=0,
        )
    assert result["strict_controls"] is True
    arms = result["arms"]
    assert arms["int4"]["accuracy"] > 0.8
    assert arms["hybrid_rest_k16"]["accuracy"] > arms["f32"]["accuracy"] + 0.05
    assert arms["hybrid_top_k16"]["accuracy"] < arms["hybrid_rest_k16"]["accuracy"]
    assert arms["resid_full"]["accuracy"] > 0.8
    assert arms["resid_row_l2"]["accuracy"] > 0.8
    assert arms["resid_norm"]["accuracy"] < 0.7
    assert (
        arms["hybrid_rest_noclassmean"]["accuracy"]
        < arms["hybrid_rest_k16"]["accuracy"] - 0.05
    )
    assert (
        arms["hybrid_rest_perm"]["accuracy"]
        < arms["hybrid_rest_k16"]["accuracy"] - 0.05
    )

    # Exact endpoints: top=0 -> int4, top=all -> f32.
    comps = PCA._fit_pca(f32_train).components_
    r_top0 = _top_residual(r_train, comps, 0)
    r_topall = _top_residual(r_train, comps, dim)
    assert np.allclose(r_top0, 0.0)
    assert np.allclose(r_topall, r_train)
    assert np.allclose(f32_train + (r_train - r_top0), int4_train)
    assert np.allclose(f32_train + (r_train - r_topall), f32_train)
    print("self-test ok")


def parse_args():
    parser = argparse.ArgumentParser()
    parser.add_argument("--texts", default="")
    parser.add_argument("--f32", default="")
    parser.add_argument("--int4", default="")
    parser.add_argument("--ks", default=",".join(str(k) for k in DEFAULT_KS))
    parser.add_argument("--primary-k", type=int, default=PRIMARY_K)
    parser.add_argument("--probe", choices=("logreg", "linear_svc"), default="logreg")
    parser.add_argument("--class-weight", choices=("none", "balanced"), default="none")
    parser.add_argument(
        "--cv-metric", choices=("accuracy", "macro_f1"), default="accuracy"
    )
    parser.add_argument("--seed", type=int, default=7)
    parser.add_argument("--cv-grid", default="0.01,0.1,1,10")
    parser.add_argument("--bootstrap", type=int, default=0)
    parser.add_argument("--bootstrap-iters", type=int, default=10000)
    parser.add_argument(
        "--legacy-controls",
        action="store_true",
        help="reproduce M1.23's oracle controls, which use true test labels in "
        "the test-side class-mean construction (label leakage; not a valid "
        "control)",
    )
    parser.add_argument("--out", default="")
    parser.add_argument("--self-test", action="store_true")
    return parser.parse_args()


def main():
    args = parse_args()
    if args.self_test:
        self_test()
        return 0
    if not args.texts or not args.f32 or not args.int4 or not args.out:
        raise SystemExit("--texts, --f32, --int4 and --out are required")
    ks = tuple(int(k) for k in args.ks.split(",") if k.strip())
    if not ks or any(k < 0 or k > CE.EXPECTED_COLS for k in ks):
        raise SystemExit(f"bad --ks {args.ks!r}")
    cv_grid = tuple(float(x) for x in args.cv_grid.split(",") if x.strip())
    result = evaluate(
        args.texts,
        GEO.concat(args.f32),
        GEO.concat(args.int4),
        ks=ks,
        primary_k=args.primary_k,
        seed=args.seed,
        cv_grid=cv_grid,
        probe=args.probe,
        class_weight="balanced" if args.class_weight == "balanced" else None,
        cv_metric=args.cv_metric,
        bootstrap=args.bootstrap,
        bootstrap_iters=args.bootstrap_iters,
        strict_controls=not args.legacy_controls,
    )
    Path(args.out).write_text(
        json.dumps(result, indent=2, ensure_ascii=False), encoding="utf-8"
    )
    print(f"done arms={len(result['arms'])} -> {args.out}")
    return 0


if __name__ == "__main__":
    sys.exit(main())

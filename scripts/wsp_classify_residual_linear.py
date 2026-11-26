#!/usr/bin/env python3
"""Fit a linear surrogate from f32 embeddings to the int4 residual.

The residual ``r = x_int4 - x_f32`` is not assumed to be noise.  On the train
split only, this script fits a ridge operator ``r ~= (x - mu) @ W + r_mu`` and
evaluates it on the test split.  It also reports how the operator acts on the
class-mean difference:

  d_f = mu_f32(class 1) - mu_f32(class 0)
  d_r = mu_res(class 1) - mu_res(class 0)
  d_pred = W.T @ d_f

``gain = |d_r| / |d_f|`` is the observed class-mean amplification; ``op_gain``
is what the fitted operator predicts.  ``angle`` is the angle between ``d_f``
and ``d_r``.  Cross-dataset transfer and a pooled operator are optional
controls: they test whether one linear map explains the residual across
datasets, not just within one.

This is a descriptive linear surrogate, not a causal ablation of the weight
quantizer.  Only frozen embeddings are read; no encoder forward is run.

Example:
  python scripts/wsp_classify_residual_linear.py \
    --spec 'sst2-l12=sst2/data-l12/texts.json|sst2/emb12/l12-pure-f32-*.f32|sst2/emb12/l12-int4-*.f32' \
    --spec 'cola-l12=cola/texts-l12.json|cola/emb12/l12-pure-f32-*.f32|cola/emb12/l12-int4-*.f32' \
    --pooled --out sst2/residual-linear-l12.json
"""

import argparse
import json
import sys
import tempfile
from pathlib import Path

import numpy as np

import wsp_classify_eval as CE

DEFAULT_ALPHA = 0.1
DEFAULT_SEED = 7


def _residual(x, q):
    """Signed int4 rounding residual."""
    return q - x


def _ridge_fit(x, r, alpha):
    """Fit r ~= (x - mu) @ W + r_mu on the rows given."""
    x = np.asarray(x, dtype=np.float64)
    r = np.asarray(r, dtype=np.float64)
    mu = x.mean(axis=0)
    r_mu = r.mean(axis=0)
    xc = x - mu
    rc = r - r_mu
    gram = xc.T @ xc + alpha * np.eye(x.shape[1])
    w = np.linalg.solve(gram, xc.T @ rc)
    return mu, r_mu, w


def _predict(x, mu, r_mu, w):
    return (x - mu) @ w + r_mu


def _r2_cos(y, pred):
    y = np.asarray(y, dtype=np.float64)
    pred = np.asarray(pred, dtype=np.float64)
    ss_res = float(((y - pred) ** 2).sum())
    ss_tot = float(((y - y.mean(axis=0)) ** 2).sum())
    if ss_tot <= 0:
        raise ValueError("R2 denominator is zero")
    r2 = 1.0 - ss_res / ss_tot
    cos = np.sum(y * pred, axis=1) / (
        np.linalg.norm(y, axis=1) * np.linalg.norm(pred, axis=1) + 1e-12
    )
    return r2, float(np.mean(cos))


def _class_stats(x, r, y, split):
    idx = split
    labels = y[idx]
    d_f = x[idx][labels == 1].mean(axis=0) - x[idx][labels == 0].mean(axis=0)
    d_r = r[idx][labels == 1].mean(axis=0) - r[idx][labels == 0].mean(axis=0)
    return d_f, d_r


def _angle(a, b):
    cos = float(a @ b / (np.linalg.norm(a) * np.linalg.norm(b) + 1e-12))
    return float(np.degrees(np.arccos(np.clip(cos, -1.0, 1.0))))


def _class_mean_metrics(d_f, d_r, w):
    d_pred = w.T @ d_f
    gain = float(np.linalg.norm(d_r) / (np.linalg.norm(d_f) + 1e-12))
    op_gain = float(np.linalg.norm(d_pred) / (np.linalg.norm(d_f) + 1e-12))
    cos_dr_dpred = float(
        d_r @ d_pred / (np.linalg.norm(d_r) * np.linalg.norm(d_pred) + 1e-12)
    )
    return {
        "class_mean_gain": gain,
        "class_mean_angle_deg": _angle(d_f, d_r),
        "operator_gain": op_gain,
        "cos_dr_dpred": cos_dr_dpred,
        "cos_df_dr": float(
            d_f @ d_r / (np.linalg.norm(d_f) * np.linalg.norm(d_r) + 1e-12)
        ),
    }


def _parse_spec(value):
    """Parse name=texts|f32_glob|int4_glob."""
    if "=" not in value:
        raise ValueError(f"bad --spec (want name=texts|f32|int4): {value!r}")
    name, rest = value.split("=", 1)
    parts = rest.split("|")
    if len(parts) != 3:
        raise ValueError(f"bad --spec (want three parts): {value!r}")
    texts, f32, int4 = (part.strip() for part in parts)
    if not name or not texts or not f32 or not int4:
        raise ValueError(f"empty --spec field: {value!r}")
    return name, texts, f32, int4


def load_dataset(texts_path, f32_pattern, int4_pattern):
    texts = json.loads(Path(texts_path).read_text(encoding="utf-8"))["texts"]
    train = [row["index"] for row in texts if row["split"] == "train"]
    test = [row["index"] for row in texts if row["split"] == "test"]
    y = np.asarray([row["label"] for row in texts], dtype=np.int64)
    x = CE.concat(f32_pattern).astype(np.float64)
    q = CE.concat(int4_pattern).astype(np.float64)
    if x.shape != q.shape:
        raise ValueError(f"f32 {x.shape} != int4 {q.shape}")
    if x.shape[0] != len(texts):
        raise ValueError(f"{x.shape[0]} rows != {len(texts)} texts")
    if not train or not test:
        raise ValueError("both train and test splits are required")
    return {
        "texts": texts_path,
        "f32": f32_pattern,
        "int4": int4_pattern,
        "train": train,
        "test": test,
        "y": y,
        "x": x,
        "r": _residual(x, q),
    }


def _probe_component(
    train_x, test_x, y_train, y_test, seed, cv_grid, probe, class_weight, cv_metric
):
    chosen, cv_score = CE._select_c(
        train_x, y_train, seed, cv_grid, probe, cv_metric, class_weight
    )
    model, scaler = CE._probe(
        train_x, y_train, seed, chosen, False, probe, class_weight
    )
    metrics = CE._metrics(model, scaler, test_x, y_test)
    return {
        "accuracy": metrics["accuracy"],
        "macro_f1": metrics["macro_f1"],
        "selected_C": chosen,
        "cv": cv_score,
    }


def _probe_decomposition(data, fit, seed, cv_grid, class_weight, cv_metric):
    """Probe r, its linear prediction, and the leftover nonlinear part."""
    mu, r_mu, w = fit
    train, test = data["train"], data["test"]
    y_train, y_test = data["y"][train], data["y"][test]
    hat_train = _predict(data["x"][train], mu, r_mu, w)
    hat_test = _predict(data["x"][test], mu, r_mu, w)
    out = {}
    for name, train_part, test_part in (
        ("r", data["r"][train], data["r"][test]),
        ("r_hat", hat_train, hat_test),
        ("r_perp", data["r"][train] - hat_train, data["r"][test] - hat_test),
    ):
        out[name] = _probe_component(
            train_part,
            test_part,
            y_train,
            y_test,
            seed,
            cv_grid,
            "logreg",
            class_weight,
            cv_metric,
        )
    return out


def evaluate_dataset(data, alpha, seed, with_probe, cv_grid, class_weight, cv_metric):
    train, test = data["train"], data["test"]
    y = data["y"]
    x, r = data["x"], data["r"]
    mu, r_mu, w = _ridge_fit(x[train], r[train], alpha)
    pred = _predict(x[test], mu, r_mu, w)
    r2, cos = _r2_cos(r[test], pred)
    d_f, d_r = _class_stats(x, r, y, test)
    metrics = _class_mean_metrics(d_f, d_r, w)

    # Shuffled-residual control: same x, train residual rows randomly reordered.
    rng = np.random.default_rng(seed)
    shuffled = r[train][rng.permutation(len(train))]
    mu_s, r_mu_s, w_s = _ridge_fit(x[train], shuffled, alpha)
    shuffle_r2, shuffle_cos = _r2_cos(r[test], _predict(x[test], mu_s, r_mu_s, w_s))
    record = {
        "n_train": len(train),
        "n_test": len(test),
        "dim": int(x.shape[1]),
        "alpha": alpha,
        "test": {"r2": r2, "cos": cos},
        "class_mean": metrics,
        "shuffled": {"r2": shuffle_r2, "cos": shuffle_cos},
        "_fit": (mu, r_mu, w),
    }
    if with_probe:
        record["probe"] = _probe_decomposition(
            data, (mu, r_mu, w), seed, cv_grid, class_weight, cv_metric
        )
    return record


def cross_evaluate(source, target):
    """Apply source's operator to target's test residual, target-specific intercept."""
    mu, r_mu, w = source["eval"]["_fit"]
    target_test = target["data"]["test"]
    target_train = target["data"]["train"]
    pred = (
        target["data"]["x"][target_test]
        - target["data"]["x"][target_train].mean(axis=0)
    ) @ w + target["data"]["r"][target_train].mean(axis=0)
    r2, cos = _r2_cos(target["data"]["r"][target_test], pred)
    return {"r2": r2, "cos": cos}


def pooled_evaluate(items, alpha, seed):
    """Fit one operator on all train rows; evaluate each target's test rows."""
    x = np.concatenate([item["data"]["x"][item["data"]["train"]] for item in items])
    r = np.concatenate([item["data"]["r"][item["data"]["train"]] for item in items])
    mu, r_mu, w = _ridge_fit(x, r, alpha)
    out = []
    for item in items:
        data = item["data"]
        pred = _predict(data["x"][data["test"]], mu, r_mu, w)
        r2, cos = _r2_cos(data["r"][data["test"]], pred)
        d_f, d_r = _class_stats(data["x"], data["r"], data["y"], data["test"])
        metrics = _class_mean_metrics(d_f, d_r, w)
        out.append(
            {
                "target": item["name"],
                "test": {"r2": r2, "cos": cos},
                "class_mean": metrics,
            }
        )
    return out


def run(specs, alpha, seed, pooled, with_probe, cv_grid, class_weight, cv_metric):
    items = []
    for name, texts, f32, int4 in specs:
        data = load_dataset(texts, f32, int4)
        evaluation = evaluate_dataset(
            data, alpha, seed, with_probe, cv_grid, class_weight, cv_metric
        )
        items.append({"name": name, "data": data, "eval": evaluation})

    cross = []
    for source in items:
        for target in items:
            if source["name"] == target["name"]:
                continue
            record = cross_evaluate(source, target)
            cross.append({"source": source["name"], "target": target["name"], **record})

    datasets = []
    for item in items:
        record = {
            "name": item["name"],
            "texts": item["data"]["texts"],
            "f32": item["data"]["f32"],
            "int4": item["data"]["int4"],
            "n_train": item["eval"]["n_train"],
            "n_test": item["eval"]["n_test"],
            "dim": item["eval"]["dim"],
            "alpha": alpha,
            "test": item["eval"]["test"],
            "class_mean": item["eval"]["class_mean"],
            "shuffled": item["eval"]["shuffled"],
        }
        if "probe" in item["eval"]:
            record["probe"] = item["eval"]["probe"]
        datasets.append(record)
    result = {"datasets": datasets, "cross": cross}
    if pooled and len(items) > 1:
        result["pooled"] = pooled_evaluate(items, alpha, seed)
    return result


def self_test():
    # The residual helper must preserve the signed q-x direction, not flip it.
    probe_x = np.array([[1.0, 2.0]])
    probe_q = probe_x + np.array([[0.5, -0.25]])
    if not np.allclose(_residual(probe_x, probe_q), [[0.5, -0.25]]):
        raise AssertionError("residual sign/direction mismatch")

    rng = np.random.default_rng(3)
    n_train, n_test, dim = 120, 80, 12
    # Train and test have different class-mean offsets, so a train/test swap
    # in class-mean accounting is visible.
    base = np.array([0.0] * dim)
    base[0] = 2.0
    x_train = rng.normal(size=(n_train, dim))
    x_test = rng.normal(size=(n_test, dim))
    y_train = np.arange(n_train) % 2
    y_test = np.arange(n_test) % 2
    x_train[y_train == 1] += base
    x_test[y_test == 1] += 1.5 * base
    a = rng.normal(size=(dim, dim))
    clean_train = (x_train - x_train.mean(axis=0)) @ a + 0.01 * rng.normal(
        size=(n_train, dim)
    )
    clean_test = (x_test - x_train.mean(axis=0)) @ a + 0.01 * rng.normal(
        size=(n_test, dim)
    )
    # A nonzero residual intercept makes a dropped ``+ r_mu`` visible.
    q_train = x_train + clean_train + 0.5
    q_test = x_test + clean_test + 0.5
    r_train = _residual(x_train, q_train)
    r_test = _residual(x_test, q_test)

    mu, r_mu, w = _ridge_fit(x_train, r_train, 0.01)
    pred = _predict(x_test, mu, r_mu, w)
    r2, cos = _r2_cos(r_test, pred)
    if r2 < 0.99 or cos < 0.99:
        raise AssertionError(f"self-test fit R2={r2:.4f} cos={cos:.4f}")
    df = x_test[y_test == 1].mean(axis=0) - x_test[y_test == 0].mean(axis=0)
    dr = r_test[y_test == 1].mean(axis=0) - r_test[y_test == 0].mean(axis=0)
    metrics = _class_mean_metrics(df, dr, w)
    if metrics["cos_dr_dpred"] < 0.99:
        raise AssertionError(f"self-test classmean cos={metrics['cos_dr_dpred']:.4f}")
    if (
        abs(metrics["class_mean_gain"] - np.linalg.norm(dr) / np.linalg.norm(df))
        > 1e-12
    ):
        raise AssertionError("self-test class-mean gain mismatch")

    # A zero-residual dataset must fail cleanly instead of dividing by zero.
    zero = {
        "train": [0, 1, 2, 3],
        "test": [4, 5],
        "y": np.array([0, 1, 0, 1, 0, 1], dtype=np.int64),
        "x": rng.normal(size=(6, dim)),
        "r": np.zeros((6, dim)),
    }
    mu0, rmu0, w0 = _ridge_fit(zero["x"][zero["train"]], zero["r"][zero["train"]], 0.1)
    if np.linalg.norm(w0) > 1e-9:
        raise AssertionError("zero residual should fit a zero operator")
    try:
        _r2_cos(np.zeros((2, dim)), np.zeros((2, dim)))
    except ValueError:
        pass
    else:
        raise AssertionError("zero R2 denominator must be rejected")


def parse_args(argv):
    parser = argparse.ArgumentParser()
    parser.add_argument("--spec", action="append", default=[])
    parser.add_argument("--alpha", type=float, default=DEFAULT_ALPHA)
    parser.add_argument("--seed", type=int, default=DEFAULT_SEED)
    parser.add_argument("--pooled", action="store_true")
    parser.add_argument("--probe", action="store_true")
    parser.add_argument(
        "--probe-class-weight", choices=("none", "balanced"), default="none"
    )
    parser.add_argument(
        "--probe-metric", choices=("accuracy", "macro_f1"), default="accuracy"
    )
    parser.add_argument("--cv-grid", default="0.01,0.1,1,10")
    parser.add_argument("--out", default="")
    parser.add_argument("--self-test", action="store_true")
    return parser.parse_args(argv)


def main(argv=None):
    args = parse_args(argv)
    if args.self_test:
        self_test()
        print("self-test ok")
        return 0
    if not args.spec or not args.out:
        raise SystemExit("--spec and --out are required")
    specs = [_parse_spec(value) for value in args.spec]
    grid = tuple(float(item) for item in args.cv_grid.split(",") if item.strip())
    class_weight = "balanced" if args.probe_class_weight == "balanced" else None
    result = run(
        specs,
        args.alpha,
        args.seed,
        args.pooled,
        args.probe,
        grid,
        class_weight,
        args.probe_metric,
    )
    Path(args.out).parent.mkdir(parents=True, exist_ok=True)
    Path(args.out).write_text(json.dumps(result, indent=2), encoding="utf-8")
    print(f"wrote {args.out}")
    return 0


if __name__ == "__main__":
    sys.exit(main())

#!/usr/bin/env python3
"""Probe frozen embeddings after PCA-based structural interventions.

The probe is always linear and is retrained inside the training split only.
Modes are:

  raw           no transform
  keepN         zero all but the first N principal components
  dropN         zero the first N principal components
  whiten        divide each principal component by sqrt(eigenvalue)

PCA is fit on train only.  For each mode the baseline probe (usually f32) and
the candidate probe (usually int4) are trained on the same transformed train
set, so the delta is "candidate minus baseline" within that mode.

By default each run fits its own PCA.  `--pca-fit NAME` fits one PCA on that
run's train split and projects every other run onto it, which tests whether a
low-rank subspace learned from one quantization level transfers to another.

Example:
  python scripts/wsp_classify_pca.py \
    --texts sst2/data-l12/texts.json \
    --run f32='sst2/emb12/l12-pure-f32-*.f32' \
    --run int4='sst2/emb12/l12-int4-*.f32' \
    --baseline f32 \
    --modes raw,keep4,keep16,keep64,whiten \
    --out sst2/pca-l12.json
"""

import argparse
import json
import sys
from pathlib import Path

import numpy as np

import wsp_classify_eval as CE

TOP_K = (1, 2, 4, 8, 16, 32, 64, 128)


def _fit_pca(train_x):
    from sklearn.decomposition import PCA

    n_components = min(train_x.shape[0], train_x.shape[1])
    pca = PCA(n_components=n_components, svd_solver="full")
    pca.fit(train_x)
    return pca


def _project(pca, x):
    return (x - pca.mean_) @ pca.components_.T


def _transform(train_x, test_x, mode, fit_train_x=None):
    if mode == "raw":
        return train_x, test_x, None
    fit_x = train_x if fit_train_x is None else fit_train_x
    pca = _fit_pca(fit_x)
    train_z = _project(pca, train_x)
    test_z = _project(pca, test_x)
    if mode == "whiten":
        scale = np.sqrt(pca.explained_variance_)
        train_z = train_z / scale
        test_z = test_z / scale
    elif mode.startswith("keep"):
        k = int(mode[4:])
        if k <= 0 or k > train_z.shape[1]:
            raise ValueError(f"bad mode {mode!r}")
        train_z[:, k:] = 0.0
        test_z[:, k:] = 0.0
    elif mode.startswith("drop"):
        k = int(mode[4:])
        if k < 0 or k >= train_z.shape[1]:
            raise ValueError(f"bad mode {mode!r}")
        train_z[:, :k] = 0.0
        test_z[:, :k] = 0.0
    else:
        raise ValueError(f"unknown mode {mode!r}")
    return train_z, test_z, pca


def _evaluate_one(
    train_x,
    test_x,
    y_train,
    y_test,
    seed,
    cv_grid,
    probe,
    class_weight=None,
    cv_metric="accuracy",
):
    chosen, cv_score = CE._select_c(
        train_x, y_train, seed, cv_grid, probe, cv_metric, class_weight
    )
    model, scaler = CE._probe(
        train_x, y_train, seed, chosen, False, probe, class_weight
    )
    metrics = CE._metrics(model, scaler, test_x, y_test)
    train_metrics = CE._metrics(model, scaler, train_x, y_train)
    return (
        {
            key: value
            for key, value in metrics.items()
            if key not in ("correct", "prediction")
        }
        | {
            "train_accuracy": train_metrics["accuracy"],
            "selected_C": chosen,
            "cv": cv_score,
        },
        metrics["prediction"],
    )


def evaluate(
    texts_path,
    run_arrays,
    baseline,
    modes,
    seed=7,
    cv_grid=(0.01, 0.1, 1.0, 10.0),
    probe="logreg",
    class_weight=None,
    cv_metric="accuracy",
    bootstrap=0,
    bootstrap_iters=10000,
    pca_fit=None,
    bootstrap_raw=False,
    bootstrap_metrics=("accuracy", "macro_f1"),
):
    texts = json.loads(Path(texts_path).read_text(encoding="utf-8"))["texts"]
    train_idx, test_idx, y_train, y_test = CE._labels(texts)
    y_train = np.asarray(y_train, dtype=np.int64)
    y_test = np.asarray(y_test, dtype=np.int64)
    if pca_fit is not None and pca_fit not in run_arrays:
        raise ValueError(f"unknown --pca-fit run {pca_fit!r}")

    fit_train_x = None if pca_fit is None else run_arrays[pca_fit][train_idx]
    results = {
        "probe": probe,
        "class_weight": class_weight,
        "cv_metric": cv_metric,
        "baseline": baseline,
        "pca_fit": pca_fit,
        "modes": {},
        "raw_bootstrap": [],
    }
    all_predictions = {}
    for mode in modes:
        rows = {}
        predictions = {}
        for name, matrix in run_arrays.items():
            train_x, test_x, pca = _transform(
                matrix[train_idx], matrix[test_idx], mode, fit_train_x
            )
            metrics, pred = _evaluate_one(
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
            metrics["pca_components"] = None if pca is None else int(pca.n_components_)
            metrics["pca_fit"] = None if pca is None else (pca_fit or name)
            rows[name] = metrics
            predictions[name] = pred
            all_predictions[(mode, name)] = pred
        entry = {"runs": rows, "bootstrap": []}
        if bootstrap > 0 and baseline in predictions:
            for name in predictions:
                if name == baseline:
                    continue
                for metric in bootstrap_metrics:
                    pair = CE._bootstrap(
                        predictions[name],
                        predictions[baseline],
                        y_test,
                        bootstrap_iters,
                        seed,
                        metric,
                    )
                    pair.update(
                        {
                            "mode": mode,
                            "left": name,
                            "right": baseline,
                            "delta_convention": "left_minus_right",
                            "iters": bootstrap_iters,
                        }
                    )
                    entry["bootstrap"].append(pair)
        results["modes"][mode] = entry
    if bootstrap_raw and "raw" in results["modes"]:
        for mode in modes:
            if mode == "raw":
                continue
            for name in run_arrays:
                for metric in bootstrap_metrics:
                    pair = CE._bootstrap(
                        all_predictions[(mode, name)],
                        all_predictions[("raw", name)],
                        y_test,
                        bootstrap_iters,
                        seed,
                        metric,
                    )
                    pair.update(
                        {
                            "mode": mode,
                            "left_mode": mode,
                            "right_mode": "raw",
                            "run": name,
                            "delta_convention": "left_minus_right",
                            "iters": bootstrap_iters,
                        }
                    )
                    results["raw_bootstrap"].append(pair)
    return results


def self_test():
    rng = np.random.default_rng(17)
    n_train, n_test, dim = 160, 80, CE.EXPECTED_COLS
    weights = np.zeros(dim)
    weights[:8] = [1.0, 0.9, 0.8, 0.7, 0.5, 0.4, 0.3, 0.2]
    train_x = rng.normal(scale=0.2, size=(n_train, dim))
    test_x = rng.normal(scale=0.2, size=(n_test, dim))
    train_y = np.asarray([0, 1] * (n_train // 2), dtype=np.int64)
    test_y = np.asarray([0, 1] * (n_test // 2), dtype=np.int64)
    train_x += train_y[:, None] * weights[None, :] * 2.0
    test_x += test_y[:, None] * weights[None, :] * 2.0

    import tempfile

    tmp = Path(tempfile.mkdtemp(prefix="pca-self-"))
    texts_path = tmp / "texts.json"
    rows = [
        {"index": i, "id": f"tr{i}", "split": "train", "label": int(train_y[i])}
        for i in range(n_train)
    ] + [
        {
            "index": n_train + i,
            "id": f"te{i}",
            "split": "test",
            "label": int(test_y[i]),
        }
        for i in range(n_test)
    ]
    texts_path.write_text(json.dumps({"texts": rows}), encoding="utf-8")

    result = evaluate(
        str(texts_path),
        {"f32": np.vstack([train_x, test_x]), "int4": np.vstack([train_x, test_x])},
        "f32",
        ["raw", "keep4", "keep16", "whiten"],
        bootstrap=20,
        bootstrap_raw=True,
        bootstrap_metrics=("accuracy",),
    )
    raw = result["modes"]["raw"]["runs"]["f32"]["accuracy"]
    keep4 = result["modes"]["keep4"]["runs"]["f32"]["accuracy"]
    assert raw > 0.75
    assert keep4 > 0.75
    assert result["modes"]["keep4"]["runs"]["f32"]["pca_components"] == n_train
    assert len(result["modes"]["raw"]["bootstrap"]) == 1
    assert (
        result["modes"]["raw"]["bootstrap"][0]["delta_convention"] == "left_minus_right"
    )
    assert len(result["raw_bootstrap"]) == 6
    assert result["raw_bootstrap"][0]["right_mode"] == "raw"
    fit_x = rng.normal(size=(40, dim)) + 3.0
    eval_x = rng.normal(size=(40, dim))
    _, _, cross_pca = _transform(eval_x, eval_x[:5], "keep4", fit_x)
    assert np.allclose(cross_pca.mean_, fit_x.mean(axis=0))
    assert not np.allclose(cross_pca.mean_, eval_x.mean(axis=0))

    cross = evaluate(
        str(texts_path),
        {"f32": np.vstack([train_x, test_x]), "int4": np.vstack([train_x, test_x])},
        "f32",
        ["raw", "keep4"],
        pca_fit="int4",
    )
    assert cross["pca_fit"] == "int4"
    assert cross["modes"]["keep4"]["runs"]["f32"]["pca_fit"] == "int4"
    assert cross["modes"]["keep4"]["runs"]["int4"]["pca_fit"] == "int4"
    print("self-test ok")


def parse_modes(value):
    modes = [item.strip() for item in value.split(",") if item.strip()]
    for mode in modes:
        if mode == "raw" or mode == "whiten":
            continue
        if mode.startswith("keep") and int(mode[4:]) in TOP_K:
            continue
        if mode.startswith("drop") and int(mode[4:]) in TOP_K:
            continue
        raise SystemExit(f"bad mode {mode!r}")
    return modes


def parse_args():
    parser = argparse.ArgumentParser()
    parser.add_argument("--texts", default="")
    parser.add_argument("--run", action="append", default=[])
    parser.add_argument("--baseline", default="")
    parser.add_argument(
        "--pca-fit",
        default="",
        help="fit PCA on this run's train split and project all runs onto it",
    )
    parser.add_argument("--modes", default="raw,keep4,keep16,keep64,whiten")
    parser.add_argument("--probe", choices=("logreg", "linear_svc"), default="logreg")
    parser.add_argument("--class-weight", choices=("none", "balanced"), default="none")
    parser.add_argument(
        "--cv-metric", choices=("accuracy", "macro_f1"), default="accuracy"
    )
    parser.add_argument("--seed", type=int, default=7)
    parser.add_argument("--cv-grid", default="0.01,0.1,1,10")
    parser.add_argument("--bootstrap", type=int, default=0)
    parser.add_argument(
        "--bootstrap-raw",
        action="store_true",
        help="also bootstrap each transformed run against its own raw run",
    )
    parser.add_argument("--bootstrap-iters", type=int, default=10000)
    parser.add_argument(
        "--bootstrap-metrics",
        default="accuracy,macro_f1",
        help="comma-separated metrics to bootstrap",
    )
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
    run_arrays = {}
    for spec in args.run:
        name, pattern = spec.split("=", 1)
        run_arrays[name] = CE.concat(pattern)
    grid = tuple(float(item) for item in args.cv_grid.split(",") if item.strip())
    boot_metrics = tuple(
        item.strip() for item in args.bootstrap_metrics.split(",") if item.strip()
    )
    bad_metrics = set(boot_metrics) - {"accuracy", "macro_f1"}
    if bad_metrics:
        raise SystemExit(f"bad --bootstrap-metrics {sorted(bad_metrics)}")
    result = evaluate(
        args.texts,
        run_arrays,
        args.baseline,
        parse_modes(args.modes),
        seed=args.seed,
        cv_grid=grid,
        probe=args.probe,
        class_weight="balanced" if args.class_weight == "balanced" else None,
        cv_metric=args.cv_metric,
        bootstrap=args.bootstrap,
        bootstrap_iters=args.bootstrap_iters,
        pca_fit=args.pca_fit or None,
        bootstrap_raw=args.bootstrap_raw,
        bootstrap_metrics=boot_metrics,
    )
    Path(args.out).write_text(
        json.dumps(result, indent=2, ensure_ascii=False), encoding="utf-8"
    )
    print(f"done modes={len(result['modes'])}")


if __name__ == "__main__":
    sys.exit(main())

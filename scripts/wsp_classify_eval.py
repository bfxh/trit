"""Evaluate frozen MiniLM embeddings on a binary classification set.

Each ``--run name=glob`` points to one or more WSPEMB1 files (chunks are
concatenated in lexical order).  The probe is trained on the train split and
evaluated on the test split.  For each ``--group`` the first run is the
shared-probe baseline: its probe is also applied to the other runs in the group
to separate "representation improved" from "probe retrained".

``--probe`` selects the frozen-encoder head: ``logreg`` (default) or
``linear_svc`` (squared-hinge linear SVM).  Both are linear; no MLP or encoder
fine-tuning is part of this harness.  ``--class-weight balanced`` and
``--cv-metric macro_f1`` are for imbalanced label sets; the defaults preserve
the SST-2 accuracy-selection protocol.

Example:
  python scripts/wsp_classify_eval.py \
    --texts sst2/data-l12/texts.json \
    --run l12-f32='sst2/emb12/l12-pure-f32-*.f32' \
    --run l12-int3='sst2/emb12/l12-int3-*.f32' \
    --group l12=l12-f32|l12-int3,l12-int4,l12-r12 \
    --bootstrap 10000 --out sst2/eval-l12.json
"""

import argparse
import glob
import json
import os
import sys
import tempfile
from pathlib import Path

# The evaluation machines can have very little commit headroom; OpenBLAS
# otherwise tries to allocate one workspace per core before sklearn even starts.
os.environ.setdefault("OPENBLAS_NUM_THREADS", "1")
os.environ.setdefault("OMP_NUM_THREADS", "1")
os.environ.setdefault("MKL_NUM_THREADS", "1")

MAGIC = b"WSPEMB1\0"
EXPECTED_COLS = 384


def _read_embedding(path):
    import numpy as np

    with open(path, "rb") as handle:
        head = handle.read(16)
        if len(head) != 16 or head[:8] != MAGIC:
            raise ValueError(f"{path}: bad WSPEMB1 header")
        rows, cols = (int(x) for x in __import__("struct").unpack("<II", head[8:]))
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
    train = [row["index"] for row in texts if row["split"] == "train"]
    test = [row["index"] for row in texts if row["split"] == "test"]
    y_train = [row["label"] for row in texts if row["split"] == "train"]
    y_test = [row["label"] for row in texts if row["split"] == "test"]
    return train, test, y_train, y_test


def _make_model(probe, c, seed, max_iter=None, class_weight=None):
    if probe == "linear_svc":
        from sklearn.svm import LinearSVC

        return LinearSVC(
            C=c,
            dual=False,
            max_iter=max_iter or 10000,
            random_state=seed,
            tol=1e-4,
            class_weight=class_weight,
        )
    if probe == "logreg":
        from sklearn.linear_model import LogisticRegression

        return LogisticRegression(
            C=c,
            max_iter=max_iter or 5000,
            solver="lbfgs",
            random_state=seed,
            tol=1e-6,
            class_weight=class_weight,
        )
    raise ValueError(f"unknown probe {probe!r}")


def _select_c(
    train_x, train_y, seed, grid, probe="logreg", metric="accuracy", class_weight=None
):
    from sklearn.metrics import accuracy_score, f1_score
    from sklearn.model_selection import StratifiedKFold

    folds = StratifiedKFold(n_splits=5, shuffle=True, random_state=seed)
    best_c, best_score = None, -1.0
    for c in grid:
        scores = []
        for fit_idx, val_idx in folds.split(train_x, train_y):
            model = _make_model(
                probe,
                c,
                seed,
                max_iter=2000 if probe == "logreg" else None,
                class_weight=class_weight,
            )
            model.fit(train_x[fit_idx], train_y[fit_idx])
            pred = model.predict(train_x[val_idx])
            if metric == "macro_f1":
                scores.append(
                    float(
                        f1_score(
                            train_y[val_idx],
                            pred,
                            average="macro",
                            labels=[0, 1],
                            zero_division=0,
                        )
                    )
                )
            else:
                scores.append(float(accuracy_score(train_y[val_idx], pred)))
        mean = sum(scores) / len(scores)
        if mean > best_score:
            best_c, best_score = c, mean
    return best_c, best_score


def _probe(
    train_x, train_y, seed, c=1.0, standardize=False, probe="logreg", class_weight=None
):
    from sklearn.preprocessing import StandardScaler

    scaler = None
    if standardize:
        scaler = StandardScaler()
        train_x = scaler.fit_transform(train_x)
    model = _make_model(probe, c, seed, class_weight=class_weight)
    model.fit(train_x, train_y)
    return model, scaler


def _metrics(model, scaler, test_x, test_y):
    from sklearn.metrics import accuracy_score, f1_score, roc_auc_score

    if scaler is not None:
        test_x = scaler.transform(test_x)
    pred = model.predict(test_x)
    score = model.decision_function(test_x)
    return {
        "accuracy": float(accuracy_score(test_y, pred)),
        "macro_f1": float(f1_score(test_y, pred, average="macro")),
        "auc": float(roc_auc_score(test_y, score)),
        "correct": (pred == test_y).astype(float),
        "prediction": pred,
    }


def _macro_f1(y_true, pred):
    from sklearn.metrics import f1_score

    return float(
        f1_score(y_true, pred, average="macro", labels=[0, 1], zero_division=0)
    )


def _bootstrap(pred_a, pred_b, y_true, iters, seed, metric="accuracy"):
    import numpy as np

    rng = np.random.default_rng(seed)
    n = len(y_true)
    deltas = np.empty(iters, dtype=np.float64)
    for index in range(iters):
        sample = rng.integers(0, n, n)
        if metric == "macro_f1":
            left = _macro_f1(y_true[sample], pred_a[sample])
            right = _macro_f1(y_true[sample], pred_b[sample])
        else:
            left = float((pred_a[sample] == y_true[sample]).mean())
            right = float((pred_b[sample] == y_true[sample]).mean())
        deltas[index] = left - right
    return {
        "metric": metric,
        "mean_delta": float(deltas.mean()),
        "ci_low": float(np.percentile(deltas, 2.5)),
        "ci_high": float(np.percentile(deltas, 97.5)),
        "better_fraction": float((deltas > 0).mean()),
    }


def parse_group(spec):
    name, rest = spec.split("=", 1)
    baseline, others = rest.split("|", 1)
    return name, baseline, [item for item in others.split(",") if item]


def evaluate(
    texts_path,
    run_specs,
    groups,
    bootstrap,
    seed,
    c=1.0,
    standardize=False,
    cv_c=False,
    cv_grid=(0.01, 0.1, 1.0, 10.0),
    probe="logreg",
    class_weight=None,
    cv_metric="accuracy",
    bootstrap_metrics=("accuracy",),
):
    texts = json.loads(Path(texts_path).read_text(encoding="utf-8"))["texts"]
    train_idx, test_idx, y_train, y_test = _labels(texts)
    runs = {}
    for spec in run_specs:
        name, pattern = spec.split("=", 1)
        runs[name] = concat(pattern)
        if runs[name].shape[0] != len(texts):
            raise ValueError(
                f"{name}: rows {runs[name].shape[0]} != texts {len(texts)}"
            )

    import numpy as np

    y_train = np.asarray(y_train, dtype=np.int64)
    y_test = np.asarray(y_test, dtype=np.int64)
    x_train = {name: matrix[train_idx] for name, matrix in runs.items()}
    x_test = {name: matrix[test_idx] for name, matrix in runs.items()}

    results = {
        "probe": probe,
        "class_weight": class_weight,
        "cv_metric": cv_metric,
        "runs": {},
        "transfer": {},
        "bootstrap": [],
    }
    probe_pred = {}
    selected_c = {}
    for name in list(runs):
        if cv_c:
            chosen, cv_score = _select_c(
                x_train[name], y_train, seed, cv_grid, probe, cv_metric, class_weight
            )
        else:
            chosen, cv_score = c, None
        selected_c[name] = chosen
        model, scaler = _probe(
            x_train[name], y_train, seed, chosen, standardize, probe, class_weight
        )
        metrics = _metrics(model, scaler, x_test[name], y_test)
        train_metrics = _metrics(model, scaler, x_train[name], y_train)
        results["runs"][name] = {
            key: value
            for key, value in metrics.items()
            if key not in ("correct", "prediction")
        }
        results["runs"][name]["train_accuracy"] = train_metrics["accuracy"]
        results["runs"][name]["selected_C"] = chosen
        results["runs"][name]["cv_accuracy"] = (
            cv_score if cv_metric == "accuracy" else None
        )
        results["runs"][name]["cv_score"] = cv_score
        probe_pred[name] = metrics["prediction"]

    for spec in groups:
        group, baseline, others = parse_group(spec)
        model, scaler = _probe(
            x_train[baseline],
            y_train,
            seed,
            selected_c[baseline],
            standardize,
            probe,
            class_weight,
        )
        train_metrics = _metrics(model, scaler, x_train[baseline], y_train)
        transfer_pred = {}
        for name in [baseline] + others:
            metrics = _metrics(model, scaler, x_test[name], y_test)
            results["transfer"][f"{group}:{name}"] = {
                key: value
                for key, value in metrics.items()
                if key not in ("correct", "prediction")
            }
            results["transfer"][f"{group}:{name}"]["train_accuracy"] = train_metrics[
                "accuracy"
            ]
            results["transfer"][f"{group}:{name}"]["selected_C"] = selected_c[baseline]
            transfer_pred[name] = metrics["prediction"]
        if bootstrap <= 0:
            continue
        members = [baseline] + others
        for i, left in enumerate(members):
            for right in members[i + 1 :]:
                for kind, predictions in (
                    ("probe", probe_pred),
                    ("transfer", transfer_pred),
                ):
                    for metric in bootstrap_metrics:
                        pair = _bootstrap(
                            predictions[left],
                            predictions[right],
                            y_test,
                            bootstrap,
                            seed,
                            metric,
                        )
                        pair.update(
                            {
                                "group": group,
                                "left": left,
                                "right": right,
                                "kind": kind,
                                "baseline_probe": baseline,
                                "iters": bootstrap,
                                "seed": seed,
                            }
                        )
                        results["bootstrap"].append(pair)
    return results


def parse_args():
    parser = argparse.ArgumentParser()
    parser.add_argument("--texts", default="")
    parser.add_argument("--run", action="append", default=[])
    parser.add_argument("--group", action="append", default=[])
    parser.add_argument("--bootstrap", type=int, default=10000)
    parser.add_argument("--seed", type=int, default=7)
    parser.add_argument("--C", type=float, default=1.0)
    parser.add_argument("--probe", choices=("logreg", "linear_svc"), default="logreg")
    parser.add_argument("--class-weight", choices=("none", "balanced"), default="none")
    parser.add_argument(
        "--cv-metric", choices=("accuracy", "macro_f1"), default="accuracy"
    )
    parser.add_argument(
        "--bootstrap-metric",
        choices=("accuracy", "macro_f1", "both"),
        default="accuracy",
    )
    parser.add_argument("--standardize", action="store_true")
    parser.add_argument("--cv-C", action="store_true")
    parser.add_argument("--cv-grid", default="0.01,0.1,1,10")
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
    grid = tuple(float(item) for item in args.cv_grid.split(",") if item.strip())
    class_weight = "balanced" if args.class_weight == "balanced" else None
    metrics = (
        ("accuracy", "macro_f1")
        if args.bootstrap_metric == "both"
        else (args.bootstrap_metric,)
    )
    result = evaluate(
        args.texts,
        args.run,
        args.group,
        args.bootstrap,
        args.seed,
        args.C,
        args.standardize,
        args.cv_C,
        grid,
        args.probe,
        class_weight,
        args.cv_metric,
        metrics,
    )
    Path(args.out).write_text(
        json.dumps(result, indent=2, ensure_ascii=False), encoding="utf-8"
    )
    print(f"done runs={len(result['runs'])} bootstrap={len(result['bootstrap'])}")


def self_test():
    import numpy as np

    rng = np.random.default_rng(3)
    n_train, n_test, dim = 80, 40, EXPECTED_COLS
    train_x = rng.normal(size=(n_train, dim)).astype(np.float32)
    test_x = rng.normal(size=(n_test, dim)).astype(np.float32)
    train_y = (train_x[:, 0] + 0.5 * train_x[:, 1] > 0).astype(np.int64)
    test_y = (test_x[:, 0] + 0.5 * test_x[:, 1] > 0).astype(np.int64)
    matrix = np.vstack([train_x, test_x])
    texts = [
        {"index": index, "id": f"t{index}", "split": "train", "label": int(label)}
        for index, label in enumerate(train_y)
    ] + [
        {
            "index": n_train + index,
            "id": f"e{index}",
            "split": "test",
            "label": int(label),
        }
        for index, label in enumerate(test_y)
    ]

    tmp = Path(tempfile.mkdtemp(prefix="classify-self-"))
    emb = tmp / "emb.f32"
    with open(emb, "wb") as handle:
        import struct

        handle.write(MAGIC + struct.pack("<II", matrix.shape[0], matrix.shape[1]))
        handle.write(matrix.tobytes(order="C"))
    texts_path = tmp / "texts.json"
    texts_path.write_text(json.dumps({"texts": texts}), encoding="utf-8")

    result = evaluate(
        str(texts_path),
        [f"f32={emb}"],
        ["g=f32|"],
        200,
        7,
    )
    assert result["probe"] == "logreg"
    assert result["runs"]["f32"]["accuracy"] > 0.6
    assert result["transfer"]["g:f32"]["accuracy"] > 0.6
    assert result["bootstrap"] == []

    svc = evaluate(
        str(texts_path),
        [f"f32={emb}"],
        ["g=f32|"],
        0,
        7,
        probe="linear_svc",
    )
    assert svc["probe"] == "linear_svc"
    assert svc["runs"]["f32"]["accuracy"] > 0.6
    assert svc["transfer"]["g:f32"]["accuracy"] > 0.6

    paired = evaluate(
        str(texts_path),
        [f"f32={emb}", f"f32b={emb}"],
        ["g=f32|f32b"],
        200,
        7,
        class_weight="balanced",
        bootstrap_metrics=("accuracy", "macro_f1"),
    )
    assert paired["class_weight"] == "balanced"
    assert len(paired["bootstrap"]) == 4
    assert {row["metric"] for row in paired["bootstrap"]} == {"accuracy", "macro_f1"}

    bad = tmp / "bad.f32"
    bad.write_bytes(b"WSPEMB2\0" + b"\0" * 8)
    try:
        _read_embedding(bad)
    except ValueError:
        pass
    else:
        raise AssertionError("bad WSPEMB1 magic must be rejected")
    print("self-test ok")


if __name__ == "__main__":
    sys.exit(main())

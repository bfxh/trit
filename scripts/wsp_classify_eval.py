"""Evaluate frozen MiniLM embeddings on SST-2 with a linear probe.

Each ``--run name=glob`` points to one or more WSPEMB1 files (chunks are
concatenated in lexical order).  The probe is trained on the SST-2 train split
and evaluated on the test split.  For each ``--group`` the first run is the
shared-probe baseline: its probe is also applied to the other runs in the group
to separate "representation improved" from "probe retrained".

Example:
  python scripts/wsp_classify_eval.py \
    --texts sst2/data-l12/texts.json \
    --run l12-f32='sst2/emb12/l12-pure-f32-c*.f32' \
    --run l12-int3='sst2/emb12/l12-int3-c*.f32' \
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


def _select_c(train_x, train_y, seed, grid):
    from sklearn.linear_model import LogisticRegression
    from sklearn.metrics import accuracy_score
    from sklearn.model_selection import StratifiedKFold

    folds = StratifiedKFold(n_splits=5, shuffle=True, random_state=seed)
    best_c, best_score = None, -1.0
    for c in grid:
        scores = []
        for fit_idx, val_idx in folds.split(train_x, train_y):
            model = LogisticRegression(
                C=c,
                max_iter=2000,
                solver="lbfgs",
                random_state=seed,
                tol=1e-6,
            )
            model.fit(train_x[fit_idx], train_y[fit_idx])
            scores.append(float(accuracy_score(train_y[val_idx], model.predict(train_x[val_idx]))))
        mean = sum(scores) / len(scores)
        if mean > best_score:
            best_c, best_score = c, mean
    return best_c, best_score


def _probe(train_x, train_y, seed, c=1.0, standardize=False):
    from sklearn.linear_model import LogisticRegression
    from sklearn.preprocessing import StandardScaler

    scaler = None
    if standardize:
        scaler = StandardScaler()
        train_x = scaler.fit_transform(train_x)
    model = LogisticRegression(
        C=c,
        max_iter=5000,
        solver="lbfgs",
        random_state=seed,
        tol=1e-6,
    )
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
    }


def _bootstrap(correct_a, correct_b, iters, seed):
    import numpy as np

    rng = np.random.default_rng(seed)
    n = len(correct_a)
    deltas = np.empty(iters, dtype=np.float64)
    for index in range(iters):
        sample = rng.integers(0, n, n)
        deltas[index] = correct_a[sample].mean() - correct_b[sample].mean()
    return {
        "mean_delta": float(deltas.mean()),
        "ci_low": float(np.percentile(deltas, 2.5)),
        "ci_high": float(np.percentile(deltas, 97.5)),
        "better_fraction": float((deltas > 0).mean()),
    }


def parse_group(spec):
    name, rest = spec.split("=", 1)
    baseline, others = rest.split("|", 1)
    return name, baseline, [item for item in others.split(",") if item]


def evaluate(texts_path, run_specs, groups, bootstrap, seed, c=1.0, standardize=False,
             cv_c=False, cv_grid=(0.01, 0.1, 1.0, 10.0)):
    texts = json.loads(Path(texts_path).read_text(encoding="utf-8"))["texts"]
    train_idx, test_idx, y_train, y_test = _labels(texts)
    runs = {}
    for spec in run_specs:
        name, pattern = spec.split("=", 1)
        runs[name] = concat(pattern)
        if runs[name].shape[0] != len(texts):
            raise ValueError(f"{name}: rows {runs[name].shape[0]} != texts {len(texts)}")

    import numpy as np

    y_train = np.asarray(y_train, dtype=np.int64)
    y_test = np.asarray(y_test, dtype=np.int64)
    x_train = {name: matrix[train_idx] for name, matrix in runs.items()}
    x_test = {name: matrix[test_idx] for name, matrix in runs.items()}

    results = {"runs": {}, "transfer": {}, "bootstrap": []}
    probe_correct = {}
    selected_c = {}
    for name in list(runs):
        if cv_c:
            chosen, cv_acc = _select_c(x_train[name], y_train, seed, cv_grid)
        else:
            chosen, cv_acc = c, None
        selected_c[name] = chosen
        model, scaler = _probe(x_train[name], y_train, seed, chosen, standardize)
        metrics = _metrics(model, scaler, x_test[name], y_test)
        train_metrics = _metrics(model, scaler, x_train[name], y_train)
        results["runs"][name] = {
            key: value for key, value in metrics.items() if key != "correct"
        }
        results["runs"][name]["train_accuracy"] = train_metrics["accuracy"]
        results["runs"][name]["selected_C"] = chosen
        results["runs"][name]["cv_accuracy"] = cv_acc
        probe_correct[name] = metrics["correct"]

    for spec in groups:
        group, baseline, others = parse_group(spec)
        model, scaler = _probe(
            x_train[baseline], y_train, seed, selected_c[baseline], standardize
        )
        train_metrics = _metrics(model, scaler, x_train[baseline], y_train)
        transfer_correct = {}
        for name in [baseline] + others:
            metrics = _metrics(model, scaler, x_test[name], y_test)
            results["transfer"][f"{group}:{name}"] = {
                key: value for key, value in metrics.items() if key != "correct"
            }
            results["transfer"][f"{group}:{name}"]["train_accuracy"] = train_metrics["accuracy"]
            results["transfer"][f"{group}:{name}"]["selected_C"] = selected_c[baseline]
            transfer_correct[name] = metrics["correct"]
        if bootstrap <= 0:
            continue
        members = [baseline] + others
        for i, left in enumerate(members):
            for right in members[i + 1:]:
                for kind, correct in (
                    ("probe", probe_correct),
                    ("transfer", transfer_correct),
                ):
                    pair = _bootstrap(
                        correct[left],
                        correct[right],
                        bootstrap,
                        seed,
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
    result = evaluate(
        args.texts, args.run, args.group, args.bootstrap, args.seed,
        args.C, args.standardize, args.cv_C, grid,
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
        {"index": n_train + index, "id": f"e{index}", "split": "test", "label": int(label)}
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
    assert result["runs"]["f32"]["accuracy"] > 0.6
    assert result["transfer"]["g:f32"]["accuracy"] > 0.6
    assert result["bootstrap"] == []

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

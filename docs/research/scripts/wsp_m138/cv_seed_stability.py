"""WSP-BUDGET 分类头复算：固定 C 网格 + 5 折，输出逐 seed 的 cv-seed JSON。

环境变量（都可省略）：
  TRIT_RESULTS        embedding 结果根，默认 `./trit-results`
  TRIT_GEOM_OUT       修正后的 f32/JSON 目录，默认 `<TRIT_RESULTS>/geometry/corrected`

M1.38 只复用本文件的 CASE 配置与 make_clf；没有 MLP、没有 encoder 微调。
"""

import argparse
import glob
import json
import os
import struct
from pathlib import Path

import numpy as np
from sklearn.linear_model import LogisticRegression
from sklearn.metrics import accuracy_score, f1_score
from sklearn.model_selection import StratifiedKFold
from sklearn.svm import LinearSVC

RESULTS = Path(os.environ.get("TRIT_RESULTS", "trit-results"))
OUT = Path(os.environ.get("TRIT_GEOM_OUT", str(RESULTS / "geometry" / "corrected")))
GRID = (0.01, 0.1, 1.0, 10.0)


def read_emb(path):
    with open(path, "rb") as fh:
        head = fh.read(16)
        rows, cols = struct.unpack("<II", head[8:])
        return np.fromfile(fh, dtype=np.float32).reshape(rows, cols)


def load(pattern):
    parts = [read_emb(p) for p in sorted(glob.glob(pattern))]
    if not parts:
        raise FileNotFoundError(pattern)
    return np.concatenate(parts, 0)


def score(y_true, pred, metric):
    if metric == "accuracy":
        return float(accuracy_score(y_true, pred))
    return float(f1_score(y_true, pred, average="macro", zero_division=0))


def make_clf(probe, c, balanced):
    if probe == "logreg":
        return LogisticRegression(
            C=c, max_iter=5000, solver="lbfgs", random_state=7,
            class_weight="balanced" if balanced else None,
        )
    return LinearSVC(
        C=c, max_iter=10000, random_state=7, tol=1e-4,
        class_weight="balanced" if balanced else None,
    )


def fit_cv(x, y, train, test, probe, balanced, metric, folds):
    x_train = x[train]
    x_test = x[test]
    y_train = y[train]
    scores = []
    for c in GRID:
        vals = []
        for tr, va in folds:
            clf = make_clf(probe, c, balanced)
            clf.fit(x_train[tr], y_train[tr])
            vals.append(score(y_train[va], clf.predict(x_train[va]), metric))
        scores.append((float(np.mean(vals)), c))
    best_c = max(scores)[1]
    clf = make_clf(probe, best_c, balanced)
    clf.fit(x_train, y_train)
    pred = clf.predict(x_test)
    return {
        "C": float(best_c),
        "score": score(y[test], pred, metric),
    }


CASES = {
    ("sst2", "l12"): {
        "texts": str(RESULTS / "sst2" / "data-l12" / "texts.json"),
        "balanced": False,
        "metric": "accuracy",
        "arms": {
            "true_f32": (OUT / "sst2-l12-f32.npy"),
            "true_bits4": (OUT / "sst2-l12-bits4.npy"),
            "int3": str(RESULTS / "sst2" / "emb12" / "l12-int3-*.f32"),
            "r12": str(RESULTS / "sst2" / "emb12" / "l12-r12-*.f32"),
        },
    },
    ("sst2", "l6"): {
        "texts": str(RESULTS / "sst2" / "data-l6" / "texts.json"),
        "balanced": False,
        "metric": "accuracy",
        "arms": {
            "true_f32": (OUT / "l6-sst2-f32.npy"),
            "true_bits4": (OUT / "l6-sst2-bits4.npy"),
            "int3": str(RESULTS / "sst2" / "emb6" / "l6-int3-*.f32"),
            "rescan_k2": str(RESULTS / "sst2" / "emb6" / "l6-rescan-k2-*.f32"),
        },
    },
    ("cola", "l12"): {
        "texts": str(RESULTS / "cola" / "texts-l12.json"),
        "balanced": True,
        "metric": "macro_f1",
        "arms": {
            "true_f32": (OUT / "cola-l12-f32.npy"),
            "true_bits4": (OUT / "cola-l12-bits4.npy"),
            "int3": str(RESULTS / "cola" / "emb12" / "l12-int3-*.f32"),
            "r12": str(RESULTS / "cola" / "emb12" / "l12-r12-*.f32"),
        },
    },
    ("cola", "l6"): {
        "texts": str(RESULTS / "cola" / "texts-l6.json"),
        "balanced": True,
        "metric": "macro_f1",
        "arms": {
            "true_f32": (OUT / "l6-cola-f32.npy"),
            "int3": str(RESULTS / "cola" / "emb6" / "l6-int3-*.f32"),
            "int4": str(RESULTS / "cola" / "emb6" / "l6-int4-*.f32"),
            "rescan_k2": str(RESULTS / "cola" / "emb6" / "l6-rescan-k2-*.f32"),
        },
    },
    ("hate", "l12"): {
        "texts": str(RESULTS / "tweet-hate" / "texts-l12.json"),
        "balanced": False,
        "metric": "accuracy",
        "arms": {
            "true_f32": (OUT / "hate-l12-f32.npy"),
            "true_bits4": (OUT / "hate-l12-bits4.npy"),
            "int3": str(RESULTS / "tweet-hate" / "emb12" / "l12-int3-*.f32"),
            "r12": str(RESULTS / "tweet-hate" / "emb12" / "l12-r12-*.f32"),
        },
    },
    ("hate", "l6"): {
        "texts": str(RESULTS / "tweet-hate" / "texts-l6.json"),
        "balanced": False,
        "metric": "accuracy",
        "arms": {
            "true_f32": (OUT / "l6-tweet-hate-f32.npy"),
            "int3": str(RESULTS / "tweet-hate" / "emb6" / "l6-int3-*.f32"),
            "int4": str(RESULTS / "tweet-hate" / "emb6" / "l6-int4-*.f32"),
            "rescan": str(RESULTS / "tweet-hate" / "emb6" / "l6-rescan-*.f32"),
        },
    },
}


def run_case(dataset, depth, seeds):
    cfg = CASES[(dataset, depth)]
    texts = json.loads(Path(cfg["texts"]).read_text(encoding="utf-8"))["texts"]
    y = np.array([r["label"] for r in texts])
    train = np.array([i for i, r in enumerate(texts) if r["split"] == "train"])
    test = np.array([i for i, r in enumerate(texts) if r["split"] == "test"])
    arms = {}
    for name, path in cfg["arms"].items():
        x = np.load(path) if isinstance(path, Path) else load(path)
        if len(x) != len(texts):
            raise ValueError(f"{dataset}/{depth}/{name}: {len(x)} != {len(texts)}")
        arms[name] = x
    print(dataset, depth, "n", len(texts), "train", len(train), "test", len(test), flush=True)

    rows = []
    for probe in ("logreg", "linear_svc"):
        for seed in seeds:
            skf = StratifiedKFold(n_splits=5, shuffle=True, random_state=seed)
            folds = list(skf.split(np.zeros(len(train)), y[train]))
            got = {}
            for name, x in arms.items():
                got[name] = fit_cv(
                    x, y, train, test, probe, cfg["balanced"], cfg["metric"], folds,
                )
                print(dataset, depth, probe, "seed", seed, name,
                      "C", got[name]["C"], cfg["metric"],
                      round(got[name]["score"], 6), flush=True)
            base = got["true_f32"]
            for name in arms:
                if name == "true_f32":
                    continue
                delta = got[name]["score"] - base["score"]
                rows.append({
                    "dataset": dataset,
                    "depth": depth,
                    "probe": probe,
                    "arm": name,
                    "metric": cfg["metric"],
                    "seed": seed,
                    "C": got[name]["C"],
                    "base_C": base["C"],
                    "arm_metric": got[name]["score"],
                    "base_metric": base["score"],
                    "delta": delta,
                    "sign": "pos" if delta > 0 else "neg" if delta < 0 else "zero",
                })
    dst = OUT / f"cv-seed-{dataset}-{depth}.json"
    dst.write_text(json.dumps({"seeds": seeds, "rows": rows}, indent=2), encoding="utf-8")
    print("wrote", dst, "rows", len(rows), flush=True)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--dataset", choices=sorted({k[0] for k in CASES}))
    ap.add_argument("--depth", choices=sorted({k[1] for k in CASES}))
    ap.add_argument("--seeds", default="7,13,19,31,43")
    args = ap.parse_args()
    seeds = [int(s) for s in args.seeds.split(",")]
    if args.dataset and args.depth:
        run_case(args.dataset, args.depth, seeds)
        return
    for key in CASES:
        run_case(key[0], key[1], seeds)


if __name__ == "__main__":
    main()

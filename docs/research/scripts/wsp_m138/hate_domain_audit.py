"""M1.38 Hate 域分类器、长度分桶、双向迁移与 f32 阈值敏感性。

环境变量（都可省略）：
  TRIT_RESULTS         embedding 结果根，默认 `./trit-results`
  TRIT_GEOM_OUT        修正后的 f32/JSON 目录，默认 `<TRIT_RESULTS>/geometry/corrected`
  TRIT_DATA_DIR        输出 TSV 目录，默认仓库 `docs/research/data`
  TRIT_L12_TOKENIZER   tokenizer id 或本地路径，默认 `sentence-transformers/all-MiniLM-L12-v2`

复算依赖于 M1.37 的 `nested-cv-hate-{l12,l6}.json` 与各 case 的 WSPEMB/f32。
"""

import csv
import json
import os
from pathlib import Path

import numpy as np
from sklearn.feature_extraction.text import TfidfVectorizer
from sklearn.linear_model import LogisticRegression
from sklearn.metrics import accuracy_score, f1_score, roc_auc_score
from sklearn.model_selection import StratifiedKFold, cross_val_predict
from sklearn.pipeline import Pipeline
from sklearn.preprocessing import StandardScaler
from transformers import AutoTokenizer

import cv_seed_stability as css

RESULTS = Path(os.environ.get("TRIT_RESULTS", "trit-results"))
GEOM = Path(os.environ.get("TRIT_GEOM_OUT", str(RESULTS / "geometry" / "corrected")))
REPO_ROOT = Path(__file__).resolve().parents[4]
REPO = Path(os.environ.get("TRIT_DATA_DIR", str(REPO_ROOT / "docs" / "research" / "data")))
TOKENIZER = os.environ.get("TRIT_L12_TOKENIZER", "sentence-transformers/all-MiniLM-L12-v2")
PATHS = {
    "sst2": str(RESULTS / "sst2" / "data-l12" / "texts.json"),
    "cola": str(RESULTS / "cola" / "texts-l12.json"),
    "hate": str(RESULTS / "tweet-hate" / "texts-l12.json"),
}
SEEDS = (7, 13, 19, 31, 43)


def split_labels(rows):
    return np.array([1 if r["split"] == "test" else 0 for r in rows])


def domain_auc(texts, labels):
    pipe = Pipeline([
        ("tfidf", TfidfVectorizer(ngram_range=(1, 2), min_df=2, max_features=200000, sublinear_tf=True)),
        ("clf", LogisticRegression(max_iter=2000, class_weight="balanced", random_state=7)),
    ])
    skf = StratifiedKFold(5, shuffle=True, random_state=7)
    proba = cross_val_predict(pipe, texts, labels, cv=skf, method="predict_proba")[:, 1]
    return float(roc_auc_score(labels, proba)), float(accuracy_score(labels, (proba > 0.5).astype(int)))


def length_auc(lengths, labels):
    pipe = Pipeline([("scale", StandardScaler()), ("clf", LogisticRegression(max_iter=1000))])
    skf = StratifiedKFold(5, shuffle=True, random_state=7)
    proba = cross_val_predict(pipe, np.array(lengths, dtype=float).reshape(-1, 1), labels, cv=skf, method="predict_proba")[:, 1]
    return float(roc_auc_score(labels, proba))


def case_rows(dataset, depth):
    cfg = css.CASES[(dataset, depth)]
    rows = json.loads(Path(cfg["texts"]).read_text(encoding="utf-8"))["texts"]
    y = np.array([r["label"] for r in rows])
    tr = np.array([i for i, r in enumerate(rows) if r["split"] == "train"])
    te = np.array([i for i, r in enumerate(rows) if r["split"] == "test"])
    x = np.load(cfg["arms"]["true_f32"])
    return rows, y, tr, te, x


def main():
    tokenizer = AutoTokenizer.from_pretrained(TOKENIZER)
    lengths = {}
    domain = []
    for dataset, path in PATHS.items():
        rows = json.loads(Path(path).read_text(encoding="utf-8"))["texts"]
        texts = [r["text"] for r in rows]
        lens = np.array([len(tokenizer(t, truncation=False)["input_ids"]) for t in texts])
        lengths[dataset] = lens
        labels = split_labels(rows)
        auc, acc = domain_auc(texts, labels)
        l_auc = length_auc(lens, labels)
        domain.append({"dataset": dataset, "n_train": int((labels == 0).sum()), "n_test": int((labels == 1).sum()),
                       "domain_auc": f"{auc:.6f}", "domain_acc": f"{acc:.6f}", "length_only_auc": f"{l_auc:.6f}"})
        print("DOMAIN", domain[-1], flush=True)

    transfer = []
    buckets = []
    thresholds = []
    hate_lengths = lengths["hate"]
    for depth in ("l12", "l6"):
        rows, y, tr, te, x = case_rows("hate", depth)
        # OOF f32 mean from M1.37
        nested = json.loads((GEOM / f"nested-cv-hate-{depth}.json").read_text(encoding="utf-8"))["rows"]
        uniq = {(r["probe"], r["outer_seed"]): r["f32_score"] for r in nested}
        oof = float(np.mean(list(uniq.values())))
        # test-internal 5-fold, fixed C=1
        internal = []
        for seed in SEEDS:
            skf = StratifiedKFold(5, shuffle=True, random_state=seed)
            vals = []
            for a, b in skf.split(np.zeros(len(te)), y[te]):
                clf = css.make_clf("logreg", 1.0, False)
                clf.fit(x[te][a], y[te][a])
                vals.append(accuracy_score(y[te][b], clf.predict(x[te][b])))
            internal.append(float(np.mean(vals)))
        clf = css.make_clf("logreg", 1.0, False)
        clf.fit(x[tr], y[tr])
        t2t = float(accuracy_score(y[te], clf.predict(x[te])))
        clf = css.make_clf("logreg", 1.0, False)
        clf.fit(x[te], y[te])
        t2r = float(accuracy_score(y[tr], clf.predict(x[tr])))
        transfer.extend([
            {"dataset": "hate", "depth": depth, "direction": "train_oof", "n_train": 8985, "n_eval": 8985, "accuracy": f"{oof:.6f}"},
            {"dataset": "hate", "depth": depth, "direction": "test_internal_oof", "n_train": 2734, "n_eval": 2734, "accuracy": f"{np.mean(internal):.6f}"},
            {"dataset": "hate", "depth": depth, "direction": "train_to_test", "n_train": 8985, "n_eval": 2734, "accuracy": f"{t2t:.6f}"},
            {"dataset": "hate", "depth": depth, "direction": "test_to_train", "n_train": 2734, "n_eval": 8985, "accuracy": f"{t2r:.6f}"},
        ])
        clf = css.make_clf("logreg", 1.0, False)
        clf.fit(x[tr], y[tr])
        proba = clf.predict_proba(x[te])[:, 1]
        yt = y[te]
        auc = float(roc_auc_score(yt, proba))
        for rule in ("0.5", "train_prior", "test_prior", "oracle"):
            if rule == "0.5":
                thr = 0.5
            elif rule == "train_prior":
                thr = float(np.quantile(proba, 1 - y[tr].mean()))
            elif rule == "test_prior":
                thr = float(np.quantile(proba, 1 - yt.mean()))
            else:
                thrs = np.unique(proba)
                accs = [accuracy_score(yt, (proba >= t).astype(int)) for t in thrs]
                thr = float(thrs[int(np.argmax(accs))])
            pred = (proba >= thr).astype(int)
            thresholds.append({
                "depth": depth, "rule": rule, "threshold": f"{thr:.6f}", "n": len(yt),
                "pred_positive_rate": f"{float(pred.mean()):.6f}",
                "accuracy": f"{accuracy_score(yt, pred):.6f}",
                "macro_f1": f"{f1_score(yt, pred, average='macro', zero_division=0):.6f}",
                "test_auc": f"{auc:.6f}",
            })
        print("THRESH", thresholds[-4:], flush=True)
        if depth == "l12":
            clf = css.make_clf("logreg", 1.0, False)
            clf.fit(x[tr], y[tr])
            pred = clf.predict(x[te])
            for lo, hi, name in [(0, 32, "<=32"), (33, 64, "33-64"), (65, 96, "65-96"), (97, 999, ">96"),
                                 (0, 64, "<=64"), (65, 999, ">64")]:
                m = (hate_lengths[te] >= lo) & (hate_lengths[te] <= hi)
                buckets.append({
                    "bin": name, "n": int(m.sum()), "accuracy": f"{accuracy_score(y[te][m], pred[m]):.6f}",
                    "positive_rate": f"{float(y[te][m].mean()):.6f}",
                    "pred_positive_rate": f"{float(pred[m].mean()):.6f}",
                    "mean_tokens": f"{float(hate_lengths[te][m].mean()):.2f}",
                })
            print("TRANSFER", transfer[-4:], flush=True)
            print("BUCKETS", buckets, flush=True)

    stats = []
    for split in ("train", "test"):
        l = hate_lengths[split_labels(rows) == (1 if split == "test" else 0)]
        stats.append({"split": split, "n": len(l), "mean": f"{l.mean():.2f}", "p50": int(np.percentile(l, 50)),
                      "p95": int(np.percentile(l, 95)), "max": int(l.max()), "over64_pct": f"{(l > 64).mean() * 100:.2f}"})

    out = {"domain": domain, "length_stats": stats, "transfer": transfer, "thresholds": thresholds, "length_buckets": buckets}
    (GEOM / "hate-domain-audit.json").write_text(json.dumps(out, indent=2), encoding="utf-8")

    with (REPO / "wsp-classify-domain-shift.tsv").open("w", encoding="utf-8", newline="") as f:
        w = csv.DictWriter(f, fieldnames=["dataset", "n_train", "n_test", "domain_auc", "domain_acc", "length_only_auc"], delimiter="\t", lineterminator="\n")
        w.writeheader(); w.writerows(domain)
    with (REPO / "wsp-classify-hate-transfer.tsv").open("w", encoding="utf-8", newline="") as f:
        w = csv.DictWriter(f, fieldnames=["dataset", "depth", "direction", "n_train", "n_eval", "accuracy"], delimiter="\t", lineterminator="\n")
        w.writeheader(); w.writerows(transfer)
    with (REPO / "wsp-classify-hate-threshold.tsv").open("w", encoding="utf-8", newline="") as f:
        w = csv.DictWriter(f, fieldnames=["depth", "rule", "threshold", "n", "pred_positive_rate", "accuracy", "macro_f1", "test_auc"], delimiter="\t", lineterminator="\n")
        w.writeheader(); w.writerows(thresholds)
    with (REPO / "wsp-classify-hate-length-buckets.tsv").open("w", encoding="utf-8", newline="") as f:
        w = csv.DictWriter(f, fieldnames=["bin", "n", "accuracy", "positive_rate", "pred_positive_rate", "mean_tokens"], delimiter="\t", lineterminator="\n")
        w.writeheader(); w.writerows(buckets)
    print("LENGTH", stats, flush=True)


if __name__ == "__main__":
    main()

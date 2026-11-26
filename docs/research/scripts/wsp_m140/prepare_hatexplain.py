r"""M1.40 HateXplain 清洗与官方切分导出。

输入：
  D:\trit-results\hatexplain\dataset.json
  D:\trit-results\hatexplain\post_id_divisions.json

主口径：
  - 官方 train + val 作为 fit；官方 test 作为 eval；不交换、不重切。
  - 3 个 annotator 多数票；无 2/3 多数则丢弃（官方切分里为 0）。
  - `hatespeech` => 1，`normal` / `offensive` => 0（hate vs non-hate）。
  - 另存 `label_toxic`：hatespeech/offensive => 1，normal => 0（仅次要口径）。
  - 去空文本、stripped-text 去重、同文本标签冲突丢弃、test 去掉与 fit
    精确重叠的文本；若提供 TweetEval 目录，test 再去掉与 TweetEval 精确
    重叠的文本（本次为 0，留作防泄漏审计）。
  - seq 64 只用于导出 manifest；token 长度统计用关闭截断的 tokenizer。

输出：
  <out>/raw/train.jsonl
  <out>/raw/test.jsonl
  <out>/prepare-summary.json
"""

import argparse
import collections
import json
import os
import re
from pathlib import Path

DEFAULT_ROOT = Path(os.environ.get("TRIT_RESULTS", "trit-results")) / "hatexplain"
DEFAULT_TOKENIZER = os.environ.get(
    "TRIT_TOKENIZER", "sentence-transformers/all-MiniLM-L12-v2"
)
LABELS = ("hatespeech", "normal", "offensive")


def norm(text):
    return re.sub(r"\s+", " ", text).strip()


def majority(post):
    labels = [a["label"] for a in post["annotators"]]
    counts = collections.Counter(labels)
    top = counts.most_common()
    if len(top) > 1 and top[0][1] == top[1][1]:
        return None, labels
    return top[0][0], labels


def build_rows(data, ids):
    rows = []
    ties = 0
    for pid in ids:
        post = data.get(pid)
        if post is None:
            continue
        label3, raw = majority(post)
        if label3 is None:
            ties += 1
            continue
        text = norm(" ".join(post["post_tokens"]))
        source = "twitter" if pid.endswith("_twitter") else "gab"
        rows.append({
            "post_id": pid,
            "text": text,
            "label3": label3,
            "label": 1 if label3 == "hatespeech" else 0,
            "label_toxic": 1 if label3 in ("hatespeech", "offensive") else 0,
            "source": source,
        })
    return rows, ties


def dedup(rows, name):
    by_text = collections.OrderedDict()
    conflict_texts = set()
    duplicate_rows = 0
    for row in rows:
        text = row["text"]
        if not text:
            continue
        if text in by_text:
            duplicate_rows += 1
            if by_text[text]["label3"] != row["label3"]:
                conflict_texts.add(text)
            continue
        by_text[text] = row
    clean = [r for t, r in by_text.items() if t not in conflict_texts]
    return clean, {
        "raw_rows": len(rows),
        "empty": sum(1 for r in rows if not r["text"]),
        "duplicate_rows": duplicate_rows,
        "conflict_texts": len(conflict_texts),
        "clean_rows": len(clean),
    }


def read_tweeteval_texts(dir_path):
    texts = set()
    for name in ("train", "test"):
        path = Path(dir_path) / f"{name}.jsonl"
        if not path.exists():
            continue
        for line in path.read_text(encoding="utf-8").splitlines():
            obj = json.loads(line)
            texts.add(norm(obj["text"]))
    return texts


def length_stats(rows, tokenizer_path, seq):
    from transformers import AutoTokenizer

    tok = AutoTokenizer.from_pretrained(tokenizer_path, local_files_only=True)
    lengths = [len(tok.encode(r["text"], truncation=False, add_special_tokens=True)) for r in rows]
    lengths.sort()
    n = len(lengths)

    def q(p):
        return lengths[min(n - 1, int(n * p))] if n else 0

    return {
        "n": n,
        "mean": round(sum(lengths) / n, 3) if n else 0,
        "p50": q(0.50),
        "p95": q(0.95),
        "max": max(lengths) if n else 0,
        "gt_seq": sum(x > seq for x in lengths),
        "gt_seq_rate": round(sum(x > seq for x in lengths) / n, 6) if n else 0,
        "seq": seq,
    }


def write_jsonl(path, rows, fields):
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("w", encoding="utf-8") as f:
        for row in rows:
            f.write(json.dumps({k: row[k] for k in fields}, ensure_ascii=False) + "\n")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--root", default=str(DEFAULT_ROOT))
    ap.add_argument("--dataset-json", default="")
    ap.add_argument("--divisions", default="")
    ap.add_argument("--out", default="")
    ap.add_argument("--tweeteval-dir", default=r"D:\trit-results\tweet-hate\raw")
    ap.add_argument("--tokenizer", default=DEFAULT_TOKENIZER)
    ap.add_argument("--seq", type=int, default=64)
    args = ap.parse_args()

    root = Path(args.root)
    dataset_path = Path(args.dataset_json) if args.dataset_json else root / "dataset.json"
    divisions_path = Path(args.divisions) if args.divisions else root / "post_id_divisions.json"
    out = Path(args.out) if args.out else root

    data = json.loads(dataset_path.read_text(encoding="utf-8"))
    divisions = json.loads(divisions_path.read_text(encoding="utf-8"))
    fit_ids = list(divisions["train"]) + list(divisions["val"])
    test_ids = list(divisions["test"])

    fit_raw, fit_ties = build_rows(data, fit_ids)
    test_raw, test_ties = build_rows(data, test_ids)
    fit_clean, fit_drop = dedup(fit_raw, "fit")
    test_clean, test_drop = dedup(test_raw, "test")

    fit_texts = {r["text"] for r in fit_clean}
    overlap_fit = [r for r in test_clean if r["text"] in fit_texts]
    test_clean = [r for r in test_clean if r["text"] not in fit_texts]

    tweet = read_tweeteval_texts(args.tweeteval_dir) if args.tweeteval_dir else set()
    overlap_tweet = [r for r in test_clean if r["text"] in tweet]
    test_clean = [r for r in test_clean if r["text"] not in tweet]

    fields = ["post_id", "text", "label", "label3", "label_toxic", "source"]
    write_jsonl(out / "raw" / "train.jsonl", fit_clean, fields)
    write_jsonl(out / "raw" / "test.jsonl", test_clean, fields)

    summary = {
        "dataset": "HateXplain",
        "source": {
            "dataset_json": str(dataset_path),
            "divisions": str(divisions_path),
            "license": "MIT (HateXplain repository)",
        },
        "split": {
            "fit": "official train + official val",
            "eval": "official test",
            "train_n": len(divisions["train"]),
            "val_n": len(divisions["val"]),
            "test_n": len(divisions["test"]),
        },
        "label_mapping": {
            "primary": "hatespeech=1; normal+offensive=0",
            "secondary_toxic": "hatespeech+offensive=1; normal=0",
            "majority": "3 annotators, require 2-of-3; ties dropped",
        },
        "raw_fit": {**fit_drop, "ties": fit_ties},
        "raw_test": {**test_drop, "ties": test_ties},
        "drop_test_fit_overlap": len(overlap_fit),
        "drop_test_tweeteval_overlap": len(overlap_tweet),
        "clean": {
            "fit": len(fit_clean),
            "test": len(test_clean),
            "fit_labels": dict(collections.Counter(r["label3"] for r in fit_clean)),
            "test_labels": dict(collections.Counter(r["label3"] for r in test_clean)),
            "fit_binary": dict(collections.Counter(r["label"] for r in fit_clean)),
            "test_binary": dict(collections.Counter(r["label"] for r in test_clean)),
            "fit_toxic": dict(collections.Counter(r["label_toxic"] for r in fit_clean)),
            "test_toxic": dict(collections.Counter(r["label_toxic"] for r in test_clean)),
            "fit_sources": dict(collections.Counter(r["source"] for r in fit_clean)),
            "test_sources": dict(collections.Counter(r["source"] for r in test_clean)),
        },
        "prior": {
            "fit_hate_rate": round(sum(r["label"] for r in fit_clean) / len(fit_clean), 6),
            "test_hate_rate": round(sum(r["label"] for r in test_clean) / len(test_clean), 6),
        },
        "length": {
            "fit": length_stats(fit_clean, args.tokenizer, args.seq),
            "test": length_stats(test_clean, args.tokenizer, args.seq),
        },
        "note": "Official train+val used only for fitting/C selection; official test labels never used "
                "for C selection or threshold calibration in primary.",
    }
    (out / "prepare-summary.json").write_text(
        json.dumps(summary, indent=2, ensure_ascii=False), encoding="utf-8"
    )
    print(json.dumps(summary, indent=2, ensure_ascii=False))


if __name__ == "__main__":
    main()

"""从真实 STS benchmark 生成 pairs JSON（人工标签来自官方 score）。

`mteb/stsbenchmark-sts` 的 test split 每行是 `(sentence1, sentence2, score)`，
score 是 0–5 的人工相似度。这里按官方约定二值化：

- score >= 4 → positive
- score <= 2 → negative
- score == 3 → 丢弃（中性对，不参与门禁）

用法（隔离 venv 里）：

```bash
python scripts/encode_sts_bench.py --model <本地 MiniLM 快照> --out sts-bench.json --limit 400
```
"""

import argparse
import json

import torch
from datasets import load_dataset
from transformers import AutoModel, AutoTokenizer


def parse_args():
    parser = argparse.ArgumentParser()
    parser.add_argument("--model", required=True, help="本地 all-MiniLM-L6-v2 快照目录")
    parser.add_argument("--out", required=True, help="输出 pairs JSON")
    parser.add_argument("--dataset", default="mteb/stsbenchmark-sts")
    parser.add_argument("--split", default="test")
    parser.add_argument("--limit", type=int, default=400, help="最多保留多少对")
    return parser.parse_args()


def label_of(score: float) -> str | None:
    if score >= 4.0:
        return "positive"
    if score <= 2.0:
        return "negative"
    return None


def main():
    args = parse_args()
    dataset = load_dataset(args.dataset, split=args.split)
    rows = []
    skipped = 0
    for row in dataset:
        score = float(row["score"])
        label = label_of(score)
        if label is None:
            skipped += 1
            continue
        rows.append((row["sentence1"], row["sentence2"], label, score))
    if args.limit > 0:
        rows = rows[: args.limit]
    if not rows:
        raise SystemExit("没有可用样本（检查 --dataset/--split 的字段名）")

    sentences = sorted({text for left, right, _, _ in rows for text in (left, right)})
    tokenizer = AutoTokenizer.from_pretrained(args.model)
    model = AutoModel.from_pretrained(args.model)
    model.eval()
    with torch.no_grad():
        batch = tokenizer(sentences, padding=True, truncation=True, return_tensors="pt")
        hidden = model(**batch).last_hidden_state
        mask = batch["attention_mask"].unsqueeze(-1).to(hidden.dtype)
        pooled = (hidden * mask).sum(dim=1) / mask.sum(dim=1)
        vectors = torch.nn.functional.normalize(pooled, dim=1).tolist()
    index = {text: vectors[position] for position, text in enumerate(sentences)}

    pairs = [
        {"a": index[left], "b": index[right], "label": label, "score": score}
        for left, right, label, score in rows
    ]
    payload = {
        "dataset": args.dataset,
        "split": args.split,
        "model": args.model,
        "sentences": len(sentences),
        "pairs": pairs,
    }
    with open(args.out, "w", encoding="utf-8") as handle:
        json.dump(payload, handle)
    positives = sum(1 for _, _, label, _ in rows if label == "positive")
    print(
        json.dumps(
            {
                "out": args.out,
                "dataset": args.dataset,
                "split": args.split,
                "pairs": len(pairs),
                "positive": positives,
                "negative": len(pairs) - positives,
                "skipped_neutral": skipped,
            }
        )
    )


if __name__ == "__main__":
    main()

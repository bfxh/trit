"""从 STS test 导出 genre 三分类样本：句向量 + 标签（训练/测试分层划分）。

三类：`main-captions` / `main-news` / `main-forums`。每个 pair 的两个句子都算样本，
按 (text, genre) 去重；每类按 2:1 分训练/测试（固定种子，按类内顺序取）。

输出：
```json
{"cols": 384, "labels": ["main-captions", ...],
 "train": [{"vector": [384 floats], "label": 0}],
 "test":  [{"vector": [384 floats], "label": 0}]}
```

用法：
```bash
python scripts/export_genre_samples.py --model <快照目录> --data <test.jsonl.gz> \
  --out genre-samples.json
```
"""

import argparse
import gzip
import json

import torch
from transformers import AutoModel, AutoTokenizer


def parse_args():
    parser = argparse.ArgumentParser()
    parser.add_argument("--model", required=True)
    parser.add_argument("--data", required=True)
    parser.add_argument("--out", required=True)
    parser.add_argument("--train-ratio", type=float, default=0.67)
    return parser.parse_args()


def encode(model, tokenizer, texts, batch=64):
    vectors = []
    with torch.no_grad():
        for start in range(0, len(texts), batch):
            enc = tokenizer(
                texts[start : start + batch],
                padding=True,
                truncation=True,
                return_tensors="pt",
            )
            hidden = model(**enc).last_hidden_state
            mask = enc["attention_mask"].unsqueeze(-1).float()
            pooled = (hidden * mask).sum(dim=1) / mask.sum(dim=1).clamp(min=1e-6)
            vectors.extend(pooled.tolist())
    return vectors


def main():
    args = parse_args()
    tokenizer = AutoTokenizer.from_pretrained(args.model)
    model = AutoModel.from_pretrained(args.model).eval()

    seen = set()
    rows = []
    with gzip.open(args.data, "rt", encoding="utf-8") as handle:
        for line in handle:
            item = json.loads(line)
            genre = item["genre"]
            for text in (item["sentence1"], item["sentence2"]):
                key = (text, genre)
                if key in seen:
                    continue
                seen.add(key)
                rows.append((text, genre))

    labels = sorted({genre for _, genre in rows})
    label_of = {genre: index for index, genre in enumerate(labels)}
    texts = [text for text, _ in rows]
    vectors = encode(model, tokenizer, texts)

    by_genre = {genre: [] for genre in labels}
    for index, (_, genre) in enumerate(rows):
        by_genre[genre].append(index)
    train, test = [], []
    for genre in labels:
        indices = by_genre[genre]
        cut = int(len(indices) * args.train_ratio)
        for index in indices[:cut]:
            train.append({"vector": vectors[index], "label": label_of[genre]})
        for index in indices[cut:]:
            test.append({"vector": vectors[index], "label": label_of[genre]})

    with open(args.out, "w", encoding="utf-8") as handle:
        json.dump(
            {"cols": len(vectors[0]), "labels": labels, "train": train, "test": test},
            handle,
        )
    print(
        json.dumps(
            {
                "samples": len(rows),
                "train": len(train),
                "test": len(test),
                "labels": labels,
                "out": args.out,
            },
            ensure_ascii=False,
        )
    )


if __name__ == "__main__":
    main()

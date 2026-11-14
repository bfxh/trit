"""从 STS test 导出句向量 pair JSON（`editset_sts` / `editset_sts_repair` 的输入）。

输出格式：
```json
{"pairs": [{"a": [384 floats], "b": [384 floats], "label": "positive" | "negative"}]}
```

划分与训练/门禁一致：score >= 4 正、<= 2 负、=3 丢弃。句向量是 MiniLM mean pooling
（与 `classifier_sts_bert` 的模型配置一致）。

用法：
```bash
python scripts/export_sts_pairs.py --model <快照目录> --data <test.jsonl.gz> \
  --out pairs.json --limit 200
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
    parser.add_argument("--limit", type=int, default=200)
    return parser.parse_args()


def main():
    args = parse_args()
    tokenizer = AutoTokenizer.from_pretrained(args.model)
    model = AutoModel.from_pretrained(args.model).eval()
    rows = []
    with gzip.open(args.data, "rt", encoding="utf-8") as handle:
        for line in handle:
            item = json.loads(line)
            score = float(item["score"])
            label = "positive" if score >= 4.0 else ("negative" if score <= 2.0 else None)
            if label is None:
                continue
            rows.append((item["sentence1"], item["sentence2"], label))
            if len(rows) >= args.limit:
                break

    def encode(texts):
        vectors = []
        with torch.no_grad():
            for start in range(0, len(texts), 32):
                enc = tokenizer(
                    texts[start : start + 32],
                    padding=True,
                    truncation=True,
                    return_tensors="pt",
                )
                hidden = model(**enc).last_hidden_state
                mask = enc["attention_mask"].unsqueeze(-1).float()
                pooled = (hidden * mask).sum(dim=1) / mask.sum(dim=1).clamp(min=1e-6)
                vectors.extend(pooled.tolist())
        return vectors

    left = encode([r[0] for r in rows])
    right = encode([r[1] for r in rows])
    pairs = [
        {"a": left[i], "b": right[i], "label": rows[i][2]} for i in range(len(rows))
    ]
    with open(args.out, "w", encoding="utf-8") as handle:
        json.dump({"pairs": pairs}, handle)
    positives = sum(1 for r in rows if r[2] == "positive")
    print(json.dumps({"pairs": len(pairs), "positive": positives, "out": args.out}, ensure_ascii=False))


if __name__ == "__main__":
    main()

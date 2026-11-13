"""在 STSb 训练集上真实梯度微调 `query` 层（384×384），产出 tuned safetensors。

这不是 HF 预训练意义上的微调，而是**针对门禁协议**的微调：输入是 MiniLM 编码的
384 维句向量，`query` 层把它映射到 384 维，两个输出的余弦要匹配 STSb 人工标签
（score >= 4 正、<= 2 负、=3 丢弃）。用 Adam 最小化二元交叉熵。

产物张量名与 base 一致（`encoder.layer.0.attention.self.query.weight`），可直接作为
`editset_sts --tuned` 的输入，替代“每 10 个元素取反”的合成 Δ。

用法（隔离 venv 里）：

```bash
python scripts/train_query_layer.py --model <本地 MiniLM 快照> --out tuned-query.safetensors
```
"""

import argparse
import json

import torch
import torch.nn.functional as F
from datasets import load_dataset
from safetensors.torch import save_file
from transformers import AutoModel, AutoTokenizer

TENSOR = "encoder.layer.0.attention.self.query.weight"


def parse_args():
    parser = argparse.ArgumentParser()
    parser.add_argument("--model", required=True, help="本地 all-MiniLM-L6-v2 快照目录")
    parser.add_argument("--out", required=True, help="输出 tuned safetensors")
    parser.add_argument("--epochs", type=int, default=12)
    parser.add_argument("--lr", type=float, default=1e-3)
    parser.add_argument("--batch", type=int, default=256)
    parser.add_argument("--seed", type=int, default=7)
    return parser.parse_args()


def label_of(score: float) -> float | None:
    if score >= 4.0:
        return 1.0
    if score <= 2.0:
        return 0.0
    return None


def encode_sentences(model, tokenizer, sentences, batch_size=64):
    vectors = []
    with torch.no_grad():
        for start in range(0, len(sentences), batch_size):
            batch = tokenizer(
                sentences[start : start + batch_size],
                padding=True,
                truncation=True,
                return_tensors="pt",
            )
            hidden = model(**batch).last_hidden_state
            mask = batch["attention_mask"].unsqueeze(-1).to(hidden.dtype)
            pooled = (hidden * mask).sum(dim=1) / mask.sum(dim=1)
            vectors.append(F.normalize(pooled, dim=1))
    return torch.cat(vectors, dim=0)


def main():
    args = parse_args()
    torch.manual_seed(args.seed)

    tokenizer = AutoTokenizer.from_pretrained(args.model)
    model = AutoModel.from_pretrained(args.model)
    model.eval()

    rows = []
    for row in load_dataset("mteb/stsbenchmark-sts", split="train"):
        label = label_of(float(row["score"]))
        if label is None:
            continue
        rows.append((row["sentence1"], row["sentence2"], label))
    sentences = sorted({text for left, right, _ in rows for text in (left, right)})
    vectors = encode_sentences(model, tokenizer, sentences)
    index = {text: position for position, text in enumerate(sentences)}

    left = torch.tensor([index[a] for a, _, _ in rows])
    right = torch.tensor([index[b] for _, b, _ in rows])
    labels = torch.tensor([label for _, _, label in rows])

    weight = model.get_parameter(TENSOR) if hasattr(model, "get_parameter") else model.state_dict()[TENSOR]
    base = weight.detach().clone()
    tuned = base.clone().requires_grad_(True)
    optimizer = torch.optim.Adam([tuned], lr=args.lr)

    def accuracy(matrix):
        ya = vectors[left] @ matrix.T
        yb = vectors[right] @ matrix.T
        cos = F.cosine_similarity(ya, yb)
        pred = (cos >= 0.5).float()
        return float((pred == labels).float().mean())

    before = accuracy(base)
    for epoch in range(args.epochs):
        permutation = torch.randperm(len(rows))
        total = 0.0
        for start in range(0, len(rows), args.batch):
            chunk = permutation[start : start + args.batch]
            ya = vectors[left[chunk]] @ tuned.T
            yb = vectors[right[chunk]] @ tuned.T
            cos = F.cosine_similarity(ya, yb)
            loss = F.binary_cross_entropy_with_logits(cos * 4.0, labels[chunk])
            optimizer.zero_grad()
            loss.backward()
            optimizer.step()
            total += float(loss) * len(chunk)
        print(
            json.dumps(
                {
                    "epoch": epoch + 1,
                    "loss": total / len(rows),
                    "accuracy": accuracy(tuned.detach()),
                }
            )
        )
    after = accuracy(tuned.detach())

    save_file({TENSOR: tuned.detach().contiguous()}, args.out)
    delta = (tuned.detach() - base).abs()
    print(
        json.dumps(
            {
                "out": args.out,
                "pairs": len(rows),
                "sentences": len(sentences),
                "accuracy_before": before,
                "accuracy_after": after,
                "delta_abs_mean": float(delta.mean()),
                "delta_changed": int((delta > 1e-6).sum()),
                "rows": base.shape[0],
                "cols": base.shape[1],
            }
        )
    )


if __name__ == "__main__":
    main()

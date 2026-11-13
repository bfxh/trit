"""在 STSb 训练集上真实梯度微调 `query` 层（384×384），支持 held-out 划分与多 seed。

这不是 HF 预训练意义上的微调，而是**针对门禁协议**的微调：输入是 MiniLM 编码的
384 维句向量，`query` 层把它映射到 384 维，两个输出的余弦要匹配 STSb 人工标签
（score >= 4 正、<= 2 负、=3 丢弃）。用 Adam 最小化二元交叉熵。

每个 seed 产出一个 tuned safetensors（张量名与 base 一致），可直接作为
`editset_sts --tuned` 的输入，替代“每 10 个元素取反”的合成 Δ。

用法（隔离 venv 里）：

```bash
python scripts/train_query_layer.py --model <本地 MiniLM 快照> \
  --out tuned-query --seeds 7,13,29 --valid-ratio 0.2
```
"""

import argparse
import json
import random

import torch
import torch.nn.functional as F
from datasets import load_dataset
from safetensors.torch import save_file
from transformers import AutoModel, AutoTokenizer

TENSOR = "encoder.layer.0.attention.self.query.weight"


def parse_args():
    parser = argparse.ArgumentParser()
    parser.add_argument("--model", required=True, help="本地 all-MiniLM-L6-v2 快照目录")
    parser.add_argument("--out", required=True, help="输出前缀，实际文件为 <out>-seed<seed>.safetensors")
    parser.add_argument("--epochs", type=int, default=12)
    parser.add_argument("--lr", type=float, default=1e-3)
    parser.add_argument("--batch", type=int, default=256)
    parser.add_argument("--seeds", default="7", help="逗号分隔的 seed 列表")
    parser.add_argument("--valid-ratio", type=float, default=0.2)
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


def accuracy(matrix, vectors, left, right, labels) -> float:
    ya = vectors[left] @ matrix.T
    yb = vectors[right] @ matrix.T
    cos = F.cosine_similarity(ya, yb)
    pred = (cos >= 0.5).float()
    return float((pred == labels).float().mean())


def main():
    args = parse_args()
    seeds = [int(part) for part in args.seeds.split(",") if part.strip()]

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
    base = model.state_dict()[TENSOR].detach().clone()

    report = []
    for seed in seeds:
        torch.manual_seed(seed)
        generator = random.Random(seed)
        order = list(range(len(rows)))
        generator.shuffle(order)
        valid_size = int(len(rows) * args.valid_ratio)
        valid_index = torch.tensor(order[:valid_size])
        train_index = torch.tensor(order[valid_size:])

        tuned = base.clone().requires_grad_(True)
        optimizer = torch.optim.Adam([tuned], lr=args.lr)
        for epoch in range(args.epochs):
            permutation = torch.randperm(len(train_index))
            for start in range(0, len(train_index), args.batch):
                chunk = train_index[permutation[start : start + args.batch]]
                ya = vectors[left[chunk]] @ tuned.T
                yb = vectors[right[chunk]] @ tuned.T
                cos = F.cosine_similarity(ya, yb)
                loss = F.binary_cross_entropy_with_logits(cos * 4.0, labels[chunk])
                optimizer.zero_grad()
                loss.backward()
                optimizer.step()
        with torch.no_grad():
            frozen = tuned.detach()
            train_accuracy = accuracy(frozen, vectors, left[train_index], right[train_index], labels[train_index])
            valid_accuracy = accuracy(frozen, vectors, left[valid_index], right[valid_index], labels[valid_index])
            base_valid = accuracy(base, vectors, left[valid_index], right[valid_index], labels[valid_index])
        path = f"{args.out}-seed{seed}.safetensors"
        save_file({TENSOR: frozen.contiguous()}, path)
        delta = (frozen - base).abs()
        entry = {
            "seed": seed,
            "out": path,
            "train_pairs": int(len(train_index)),
            "valid_pairs": int(len(valid_index)),
            "train_accuracy": train_accuracy,
            "valid_accuracy": valid_accuracy,
            "valid_accuracy_base": base_valid,
            "delta_abs_mean": float(delta.mean()),
        }
        report.append(entry)
        print(json.dumps(entry))

    print(json.dumps({"seeds": seeds, "report": report}))


if __name__ == "__main__":
    main()

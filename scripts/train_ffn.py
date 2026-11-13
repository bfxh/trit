"""在 STSb train 上协议级微调两层 FFN（`intermediate.dense` + `output.dense`）。

前向用 STE 三值化（`clamp(round(w/scale), -1, 1)`，反向直通），目标让
`cos(y_a, y_b)` 匹配人工标签。产物是两个张量（名字与 base 一致）的 safetensors，
可直接作为 `editset_ffn --tuned` 的输入。

```bash
HF_ENDPOINT=https://hf-mirror.com python scripts/train_ffn.py \
  --model <本地 MiniLM 快照> --out tuned-ffn --seeds 7 --valid-ratio 0.2
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

T1 = "encoder.layer.0.intermediate.dense.weight"
T2 = "encoder.layer.0.output.dense.weight"


def parse_args():
    parser = argparse.ArgumentParser()
    parser.add_argument("--model", required=True)
    parser.add_argument("--out", required=True, help="输出前缀，实际文件为 <out>-seed<seed>.safetensors")
    parser.add_argument("--epochs", type=int, default=10)
    parser.add_argument("--lr", type=float, default=3e-3)
    parser.add_argument("--batch", type=int, default=256)
    parser.add_argument("--seeds", default="7")
    parser.add_argument("--valid-ratio", type=float, default=0.2)
    return parser.parse_args()


def label_of(score: float):
    if score >= 4.0:
        return 1.0
    if score <= 2.0:
        return 0.0
    return None


def ternarize(weight):
    """STE 三值化：前向 clamp(round(w/scale))，反向直通。"""
    scale = weight.abs().mean() + 1e-8
    quantized = torch.clamp(torch.round(weight / scale), -1.0, 1.0)
    return weight + (quantized - weight).detach()


def ste_sign(value):
    """sign 的直通估计：前向 sign，反向恒等。"""
    return value + (torch.sign(value) - value).detach()


def forward(vectors, w1, w2):
    hidden = ste_sign(vectors @ ternarize(w1).T)
    return ste_sign(hidden @ ternarize(w2).T)


def accuracy(vectors, left, right, labels, w1, w2):
    with torch.no_grad():
        ya = forward(vectors[left], w1, w2)
        yb = forward(vectors[right], w1, w2)
        cos = F.cosine_similarity(ya, yb)
        pred = (cos >= 0.5).float()
        return float((pred == labels).float().mean())


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
    base1 = model.state_dict()[T1].detach().clone()
    base2 = model.state_dict()[T2].detach().clone()

    report = []
    for seed in seeds:
        torch.manual_seed(seed)
        order = list(range(len(rows)))
        random.Random(seed).shuffle(order)
        valid_size = int(len(rows) * args.valid_ratio)
        valid_index = torch.tensor(order[:valid_size])
        train_index = torch.tensor(order[valid_size:])

        w1 = base1.clone().requires_grad_(True)
        w2 = base2.clone().requires_grad_(True)
        optimizer = torch.optim.Adam([w1, w2], lr=args.lr)
        for epoch in range(args.epochs):
            permutation = torch.randperm(len(train_index))
            total = 0.0
            for start in range(0, len(train_index), args.batch):
                chunk = train_index[permutation[start : start + args.batch]]
                ya = forward(vectors[left[chunk]], w1, w2)
                yb = forward(vectors[right[chunk]], w1, w2)
                cos = F.cosine_similarity(ya, yb)
                loss = F.binary_cross_entropy_with_logits(cos * 4.0, labels[chunk])
                optimizer.zero_grad()
                loss.backward()
                optimizer.step()
                total += float(loss) * len(chunk)
            print(
                json.dumps(
                    {
                        "seed": seed,
                        "epoch": epoch + 1,
                        "loss": total / len(train_index),
                        "valid_accuracy": accuracy(
                            vectors, left[valid_index], right[valid_index], labels[valid_index], w1, w2
                        ),
                    }
                )
            )

        base_valid = accuracy(
            vectors, left[valid_index], right[valid_index], labels[valid_index], base1, base2
        )
        valid = accuracy(
            vectors, left[valid_index], right[valid_index], labels[valid_index], w1, w2
        )
        path = f"{args.out}-seed{seed}.safetensors"
        save_file({T1: w1.detach().contiguous(), T2: w2.detach().contiguous()}, path)
        entry = {
            "seed": seed,
            "out": path,
            "train_pairs": int(len(train_index)),
            "valid_pairs": int(len(valid_index)),
            "valid_accuracy": valid,
            "valid_accuracy_base": base_valid,
        }
        report.append(entry)
        print(json.dumps(entry))

    print(json.dumps({"report": report}))


if __name__ == "__main__":
    main()

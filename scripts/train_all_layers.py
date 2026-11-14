"""在 STSb 训练集上真实梯度微调 MiniLM 的 **36 个线性权重**（6 层 × 6 个）。

与 `train_query_layer.py` 的区别：那里只训 `query`（输入是冻结的句向量），这里
**端到端**训练全部 36 个线性权重（embedding/LayerNorm/bias 仍冻结），每个 batch 重新前向。

损失与门禁协议一致：句子对 mean pooling 后的余弦要匹配 STSb 标签
（score >= 4 正、<= 2 负、=3 丢弃），用 `BCEWithLogits(cos * 4)`。

输出一个 safetensors，张量名与 base 一致；可直接作为编辑集门禁的 `--tuned`。

用法：
```bash
python scripts/train_all_layers.py --model <本地 MiniLM 快照> \
  --data <train.jsonl.gz> --out tuned-all.safetensors --epochs 1 --lr 1e-4 --batch 16
```
"""

import argparse
import gzip
import json
import random

import torch
import torch.nn.functional as F
from safetensors.torch import save_file
from transformers import AutoModel, AutoTokenizer

LAYERS = 6
SUFFIXES = [
    "attention.self.query",
    "attention.self.key",
    "attention.self.value",
    "attention.output.dense",
    "intermediate.dense",
    "output.dense",
]


def parse_args():
    parser = argparse.ArgumentParser()
    parser.add_argument("--model", required=True, help="本地 all-MiniLM-L6-v2 快照目录")
    parser.add_argument("--data", required=True, help="train.jsonl.gz（mteb/stsbenchmark-sts）")
    parser.add_argument("--out", required=True, help="输出 safetensors")
    parser.add_argument("--epochs", type=int, default=1)
    parser.add_argument("--lr", type=float, default=1e-4)
    parser.add_argument("--batch", type=int, default=16)
    parser.add_argument("--seed", type=int, default=7)
    parser.add_argument("--valid-ratio", type=float, default=0.2)
    parser.add_argument("--max-steps", type=int, default=0, help=">0 时提前停止")
    return parser.parse_args()


def label_of(score: float):
    if score >= 4.0:
        return 1.0
    if score <= 2.0:
        return 0.0
    return None


def load_rows(path: str):
    rows = []
    opener = gzip.open if path.endswith(".gz") else open
    with opener(path, "rt", encoding="utf-8") as handle:
        for line in handle:
            item = json.loads(line)
            label = label_of(float(item["score"]))
            if label is None:
                continue
            rows.append((item["sentence1"], item["sentence2"], label))
    return rows


def mean_pool(hidden, mask):
    mask = mask.unsqueeze(-1).to(hidden.dtype)
    return (hidden * mask).sum(dim=1) / mask.sum(dim=1).clamp(min=1e-6)


def accuracy(model, tokenizer, rows, batch=32):
    model.eval()
    correct = 0
    with torch.no_grad():
        for start in range(0, len(rows), batch):
            chunk = rows[start : start + batch]
            enc1 = tokenizer([r[0] for r in chunk], padding=True, truncation=True, return_tensors="pt")
            enc2 = tokenizer([r[1] for r in chunk], padding=True, truncation=True, return_tensors="pt")
            v1 = F.normalize(mean_pool(model(**enc1).last_hidden_state, enc1["attention_mask"]), dim=1)
            v2 = F.normalize(mean_pool(model(**enc2).last_hidden_state, enc2["attention_mask"]), dim=1)
            cos = F.cosine_similarity(v1, v2)
            pred = (cos >= 0.5).float()
            labels = torch.tensor([r[2] for r in chunk])
            correct += int((pred == labels).sum())
    model.train()
    return correct / len(rows)


def main():
    args = parse_args()
    torch.manual_seed(args.seed)
    random.seed(args.seed)
    model = AutoModel.from_pretrained(args.model)
    tokenizer = AutoTokenizer.from_pretrained(args.model)
    rows = load_rows(args.data)
    random.shuffle(rows)
    valid_size = int(len(rows) * args.valid_ratio)
    valid, train = rows[:valid_size], rows[valid_size:]

    model.eval()
    base_valid = accuracy(model, tokenizer, valid)
    for param in model.parameters():
        param.requires_grad_(False)
    tuned = {}
    for layer in range(LAYERS):
        for suffix in SUFFIXES:
            name = f"encoder.layer.{layer}.{suffix}.weight"
            module = model.get_submodule(name.rsplit(".", 1)[0])
            module.weight.requires_grad_(True)
            tuned[name] = module.weight
    optimizer = torch.optim.Adam(tuned.values(), lr=args.lr)
    model.train()

    step = 0
    for epoch in range(args.epochs):
        for start in range(0, len(train), args.batch):
            chunk = train[start : start + args.batch]
            enc1 = tokenizer([r[0] for r in chunk], padding=True, truncation=True, return_tensors="pt")
            enc2 = tokenizer([r[1] for r in chunk], padding=True, truncation=True, return_tensors="pt")
            v1 = F.normalize(mean_pool(model(**enc1).last_hidden_state, enc1["attention_mask"]), dim=1)
            v2 = F.normalize(mean_pool(model(**enc2).last_hidden_state, enc2["attention_mask"]), dim=1)
            cos = F.cosine_similarity(v1, v2)
            labels = torch.tensor([r[2] for r in chunk])
            loss = F.binary_cross_entropy_with_logits(cos * 4.0, labels)
            optimizer.zero_grad()
            loss.backward()
            optimizer.step()
            step += 1
            if step % 20 == 0:
                print(f"step {step} loss {loss.item():.4f}", flush=True)
            if args.max_steps and step >= args.max_steps:
                break
        if args.max_steps and step >= args.max_steps:
            break

    model.eval()
    tuned_valid = accuracy(model, tokenizer, valid)
    save_file({name: weight.detach().contiguous() for name, weight in tuned.items()}, args.out)
    report = {
        "tensors": len(tuned),
        "train_pairs": len(train),
        "valid_pairs": len(valid),
        "steps": step,
        "base_valid_accuracy": base_valid,
        "tuned_valid_accuracy": tuned_valid,
        "delta_accuracy": tuned_valid - base_valid,
        "out": args.out,
    }
    print(json.dumps(report, ensure_ascii=False, indent=2), flush=True)


if __name__ == "__main__":
    main()

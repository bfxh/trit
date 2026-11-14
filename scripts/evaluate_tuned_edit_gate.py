"""编辑集门禁：真实 tuned 权重 vs base，在 STS test 上统计达成率 / 保持率。

划分（与训练脚本的标签口径一致）：
- edit 集：score >= 4 的句对（应当被判为相似）——「编辑学对了」；
- keep 集：score <= 2 的句对（应当被判为不相似）——「没改坏」；
- score == 3 丢弃。

预测口径：mean pooling 后的余弦 >= 0.5 判为相似（与训练损失一致）。

输出 JSON：
- edit_total / edit_base_hits / edit_tuned_hits（达成率）
- keep_total / keep_base_hits / keep_tuned_hits（保持率）
- regressions：base 对、tuned 错的样本数
- fixes：base 错、tuned 对的样本数

用法：
```bash
python scripts/evaluate_tuned_edit_gate.py --model <快照目录> \
  --base <model.safetensors> --tuned <tuned-all.safetensors> \
  --data <test.jsonl.gz>
```
"""

import argparse
import gzip
import json

import torch
import torch.nn.functional as F
from safetensors.torch import load_file
from transformers import AutoModel, AutoTokenizer


def parse_args():
    parser = argparse.ArgumentParser()
    parser.add_argument("--model", required=True, help="本地 MiniLM 快照目录")
    parser.add_argument("--base", required=True, help="base safetensors（含全部张量）")
    parser.add_argument("--tuned", required=True, help="tuned safetensors（36 个线性权重）")
    parser.add_argument("--data", required=True, help="test.jsonl.gz")
    parser.add_argument("--batch", type=int, default=32)
    return parser.parse_args()


def load_rows(path):
    rows = []
    opener = gzip.open if path.endswith(".gz") else open
    with opener(path, "rt", encoding="utf-8") as handle:
        for line in handle:
            item = json.loads(line)
            score = float(item["score"])
            if score >= 4.0:
                rows.append((item["sentence1"], item["sentence2"], "edit"))
            elif score <= 2.0:
                rows.append((item["sentence1"], item["sentence2"], "keep"))
    return rows


def predict(model, tokenizer, rows, batch):
    model.eval()
    hits = []
    with torch.no_grad():
        for start in range(0, len(rows), batch):
            chunk = rows[start : start + batch]
            enc1 = tokenizer([r[0] for r in chunk], padding=True, truncation=True, return_tensors="pt")
            enc2 = tokenizer([r[1] for r in chunk], padding=True, truncation=True, return_tensors="pt")
            h1 = model(**enc1).last_hidden_state
            h2 = model(**enc2).last_hidden_state
            m1 = enc1["attention_mask"].unsqueeze(-1).to(h1.dtype)
            m2 = enc2["attention_mask"].unsqueeze(-1).to(h2.dtype)
            v1 = F.normalize((h1 * m1).sum(dim=1) / m1.sum(dim=1).clamp(min=1e-6), dim=1)
            v2 = F.normalize((h2 * m2).sum(dim=1) / m2.sum(dim=1).clamp(min=1e-6), dim=1)
            cos = F.cosine_similarity(v1, v2)
            hits.extend((cos >= 0.5).tolist())
    return hits


def copy_weights(model, state):
    for name, weight in state.items():
        if not (name.endswith(".weight") or name.endswith(".bias")):
            continue
        module = model.get_submodule(name.rsplit(".", 1)[0])
        if name.endswith(".weight") and hasattr(module, "weight"):
            module.weight.data.copy_(weight)
        elif name.endswith(".bias") and hasattr(module, "bias"):
            module.bias.data.copy_(weight)


def main():
    args = parse_args()
    tokenizer = AutoTokenizer.from_pretrained(args.model)
    model = AutoModel.from_pretrained(args.model)
    base_state = load_file(args.base)
    tuned_state = load_file(args.tuned)
    copy_weights(model, base_state)
    rows = load_rows(args.data)
    base_hits = predict(model, tokenizer, rows, args.batch)
    copy_weights(model, tuned_state)
    tuned_hits = predict(model, tokenizer, rows, args.batch)

    def stats(kind):
        total = sum(1 for r in rows if r[2] == kind)
        base = sum(1 for r, hit in zip(rows, base_hits) if r[2] == kind and hit)
        tuned = sum(1 for r, hit in zip(rows, tuned_hits) if r[2] == kind and hit)
        return {"total": total, "base_hits": base, "tuned_hits": tuned,
                "base_rate": base / max(total, 1), "tuned_rate": tuned / max(total, 1)}

    edit = stats("edit")
    keep = stats("keep")
    regressions = sum(
        1 for r, b, t in zip(rows, base_hits, tuned_hits)
        if r[2] == "keep" and b and not t
    )
    fixes = sum(
        1 for r, b, t in zip(rows, base_hits, tuned_hits)
        if r[2] == "edit" and not b and t
    )
    report = {
        "edit_set": edit,
        "keep_set": keep,
        "edit_gain": edit["tuned_rate"] - edit["base_rate"],
        "keep_drop": keep["base_rate"] - keep["tuned_rate"],
        "regressions": regressions,
        "fixes": fixes,
        "base": args.base,
        "tuned": args.tuned,
    }
    print(json.dumps(report, ensure_ascii=False, indent=2))


if __name__ == "__main__":
    main()

#!/usr/bin/env python3
"""导出 MiniLM 第 0 层三段线性输入的真实激活（对齐基线，不参与交付）。

输出 `WSPACT1` 格式：
    8 字节魔数 `WSPACT1\\0` + u32 rows + u32 cols + rows*cols 个 f32（小端）

用法：
    HF_HUB_OFFLINE=1 python examples/export_bias_activations.py \
        [输出目录] [样本数]

默认输出 `$TRIT_DATA/miniLM-e2e-activations/`（大件不进仓库）。
文本来源是 `mteb/stsbenchmark-sts` test 的前 512 句；模型是
`sentence-transformers/all-MiniLM-L6-v2`。
"""

import hashlib
import json
import os
import struct
import sys

os.environ.setdefault("HF_HUB_OFFLINE", "1")
os.environ.setdefault("TRANSFORMERS_OFFLINE", "1")

import numpy as np
import torch
from datasets import load_dataset
from transformers import AutoModel, AutoTokenizer

MODEL_ID = "sentence-transformers/all-MiniLM-L6-v2"
DEFAULT_OUT = os.path.join(os.environ.get("TRIT_DATA", "trit-data"), "miniLM-e2e-activations")


def main() -> None:
    out_dir = sys.argv[1] if len(sys.argv) > 1 else DEFAULT_OUT
    samples = int(sys.argv[2]) if len(sys.argv) > 2 else 256
    seed = 7

    dataset = load_dataset("mteb/stsbenchmark-sts", split="test")
    texts = []
    for first, second in zip(dataset["sentence1"], dataset["sentence2"]):
        texts.extend([first, second])
        if len(texts) >= 512:
            break

    tokenizer = AutoTokenizer.from_pretrained(MODEL_ID, local_files_only=True)
    model = AutoModel.from_pretrained(MODEL_ID, local_files_only=True)
    model.eval()

    captured = {}

    def hook(name):
        def capture(_module, inputs, _output):
            captured[name] = inputs[0].detach()

        return capture

    layer = model.encoder.layer[0]
    layer.attention.self.query.register_forward_hook(hook("query"))
    layer.intermediate.dense.register_forward_hook(hook("intermediate"))
    layer.output.dense.register_forward_hook(hook("output"))

    encoded = tokenizer(
        texts, padding=True, truncation=True, max_length=64, return_tensors="pt"
    )
    with torch.no_grad():
        model(**encoded)

    mask = encoded["attention_mask"].bool()
    rng = np.random.default_rng(seed)
    os.makedirs(out_dir, exist_ok=True)
    manifest = {
        "model": MODEL_ID,
        "revision": getattr(model.config, "_commit_hash", None)
        or "1110a243fdf4706b3f48f1d95db1a4f5529b4d41",
        "text_source": "mteb/stsbenchmark-sts test (sentence1/sentence2, first 512 sentences)",
        "samples": samples,
        "seed": seed,
        "format": "WSPACT1\\0 + u32 rows + u32 cols + f32le",
        "layers": {},
    }
    for name, tensor in captured.items():
        vectors = tensor[mask].numpy().astype(np.float32)
        order = rng.permutation(len(vectors))[:samples]
        vectors = np.ascontiguousarray(vectors[order])
        path = os.path.join(out_dir, f"{name}.f32")
        with open(path, "wb") as handle:
            handle.write(b"WSPACT1\0")
            handle.write(struct.pack("<II", vectors.shape[0], vectors.shape[1]))
            handle.write(vectors.tobytes(order="C"))
        with open(path, "rb") as handle:
            digest = hashlib.sha256(handle.read()).hexdigest()
        manifest["layers"][name] = {
            "file": f"{name}.f32",
            "rows": int(vectors.shape[0]),
            "cols": int(vectors.shape[1]),
            "sha256": digest,
        }

    with open(os.path.join(out_dir, "manifest.json"), "w", encoding="utf-8") as handle:
        json.dump(manifest, handle, indent=2, ensure_ascii=False)
    print(json.dumps(manifest, ensure_ascii=False, indent=2))


if __name__ == "__main__":
    main()

#!/usr/bin/env python3
"""导出 MiniLM 6 层的序列输入、mask、PyTorch f32 参照（对齐基线，不参与交付）。

产物（默认 `$TRIT_DATA/miniLM-bert6/`）：
- `input.f32`：第 0 层输入 hidden states（`WSPACT1`，batch*seq × hidden）
- `mask.u8`：`WSPMSK1`（batch × seq）
- `reference.f32`：PyTorch 最后一层输出（`WSPACT1`）
- `layer0..5.f32`：逐层输出（`WSPACT1`），用于逐层对齐
- `manifest.json`：形状/来源/sha256

只作对齐基线；Rust 侧只读这些文件。
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
DEFAULT_OUT = os.path.join(
    os.environ.get("TRIT_DATA", "trit-data"), "miniLM-bert6"
)


def write_matrix(path, values):
    array = np.ascontiguousarray(values.astype(np.float32))
    with open(path, "wb") as handle:
        handle.write(b"WSPACT1\0")
        handle.write(struct.pack("<II", array.shape[0], array.shape[1]))
        handle.write(array.tobytes(order="C"))


def sha256(path):
    with open(path, "rb") as handle:
        return hashlib.sha256(handle.read()).hexdigest()


def main():
    out_dir = sys.argv[1] if len(sys.argv) > 1 else DEFAULT_OUT
    batch = int(sys.argv[2]) if len(sys.argv) > 2 else 8
    seq_len = 64

    dataset = load_dataset("mteb/stsbenchmark-sts", split="test")
    texts = []
    pairs = []
    for index, (first, second) in enumerate(
        zip(dataset["sentence1"], dataset["sentence2"])
    ):
        texts.extend([first, second])
        pairs.append(
            {"a": 2 * index, "b": 2 * index + 1, "score": float(dataset["score"][index])}
        )
        if len(texts) >= batch:
            break
    texts = texts[:batch]
    pairs = pairs[: batch // 2]

    tokenizer = AutoTokenizer.from_pretrained(MODEL_ID, local_files_only=True)
    model = AutoModel.from_pretrained(MODEL_ID, local_files_only=True)
    model.eval()
    config = model.config

    captured = {}
    layers = []

    def hook(name):
        def fn(_module, inputs, _output):
            captured[name] = inputs[0].detach()

        return fn

    for index, layer in enumerate(model.encoder.layer):
        layer.attention.self.query.register_forward_hook(hook(f"q{index}"))
        layer.register_forward_hook(
            lambda _m, _inp, out, index=index: layers.append(
                (index, (out[0] if isinstance(out, tuple) else out).detach())
            )
        )

    encoded = tokenizer(
        texts, padding="max_length", truncation=True, max_length=seq_len, return_tensors="pt"
    )
    with torch.no_grad():
        output = model(**encoded)

    first_hidden = captured["q0"]
    mask = encoded["attention_mask"].bool()
    reference = output.last_hidden_state.detach()

    os.makedirs(out_dir, exist_ok=True)
    write_matrix(
        os.path.join(out_dir, "input.f32"),
        first_hidden.reshape(-1, first_hidden.shape[-1]).numpy(),
    )
    write_matrix(
        os.path.join(out_dir, "reference.f32"),
        reference.reshape(-1, reference.shape[-1]).numpy(),
    )
    with open(os.path.join(out_dir, "mask.u8"), "wb") as handle:
        handle.write(b"WSPMSK1\0")
        handle.write(struct.pack("<II", mask.shape[0], mask.shape[1]))
        handle.write(bytes(mask.reshape(-1).to(torch.uint8).numpy().tolist()))
    for index, hidden in sorted(layers, key=lambda item: item[0]):
        write_matrix(
            os.path.join(out_dir, f"layer{index}.f32"),
            hidden.reshape(-1, hidden.shape[-1]).numpy(),
        )

    with open(os.path.join(out_dir, "pairs.json"), "w", encoding="utf-8") as handle:
        json.dump({"pairs": pairs}, handle, indent=2, ensure_ascii=False)

    manifest = {
        "model": MODEL_ID,
        "revision": getattr(config, "_commit_hash", None)
        or "1110a243fdf4706b3f48f1d95db1a4f5529b4d41",
        "text_source": f"mteb/stsbenchmark-sts test (first {batch} sentences)",
        "batch": batch,
        "seq": seq_len,
        "hidden": config.hidden_size,
        "heads": config.num_attention_heads,
        "layers": config.num_hidden_layers,
        "intermediate": config.intermediate_size,
        "hidden_act": config.hidden_act,
        "layer_norm_eps": config.layer_norm_eps,
        "pairs": len(pairs),
        "files": {
            name: sha256(os.path.join(out_dir, name))
            for name in ["input.f32", "reference.f32", "mask.u8", "pairs.json"]
            + [f"layer{index}.f32" for index in range(config.num_hidden_layers)]
        },
    }
    with open(os.path.join(out_dir, "manifest.json"), "w", encoding="utf-8") as handle:
        json.dump(manifest, handle, indent=2, ensure_ascii=False)
    print(json.dumps(manifest, ensure_ascii=False, indent=2))


if __name__ == "__main__":
    main()

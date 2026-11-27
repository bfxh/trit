"""M1.40 分块生成 bert6 WSPACT1/WSPMSK1 输入。

`wsp_export_sst2.py` 会为全量 19k×64×384 同时写 input 和 zeros reference，
在 D: 只剩 1.4GB 时直接写满。这里按 1,500 条一块生成，只保留当前 chunk 的
input.f32；编码完立即删除。文本顺序与 `wsp_export_sst2.py` 一致：
official train+val 在前，official test 在后。

输出：
  <dst>/chunk-000/{input.f32,mask.u8,texts.json,manifest.json}
  <dst>/../texts-l12.json 或 texts-l6.json（全量，供评测）
"""

import argparse
import json
import struct
from pathlib import Path

import numpy as np
from safetensors import safe_open
from tokenizers import Tokenizer


def matrix_header(rows, cols):
    return b"WSPACT1\0" + struct.pack("<II", rows, cols)


def mask_header(batch, seq):
    return b"WSPMSK1\0" + struct.pack("<II", batch, seq)


def load_rows(path, split):
    rows = []
    with open(path, encoding="utf-8") as handle:
        for line in handle:
            if not line.strip():
                continue
            obj = json.loads(line)
            text = str(obj["text"]).strip()
            if not text:
                continue
            rows.append({
                "id": f"{split}-{len(rows)}",
                "split": split,
                "text": text,
                "label": int(obj["label"]),
            })
    return rows


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--train", required=True)
    ap.add_argument("--test", required=True)
    ap.add_argument("--model-dir", required=True)
    ap.add_argument("--dst", required=True)
    ap.add_argument("--texts-out", required=True)
    ap.add_argument("--chunk-size", type=int, default=1500)
    ap.add_argument("--seq", type=int, default=64)
    ap.add_argument("--batch", type=int, default=128)
    ap.add_argument("--only-chunk", type=int, default=-1,
                    help="只生成该 chunk 下标；-1 生成全部（需要足够磁盘）")
    args = ap.parse_args()

    train = load_rows(args.train, "train")
    test = load_rows(args.test, "test")
    rows = train + test
    hidden = int(json.loads((Path(args.model_dir) / "config.json").read_text(encoding="utf-8"))["hidden_size"])
    eps = float(
        json.loads((Path(args.model_dir) / "config.json").read_text(encoding="utf-8")).get(
            "layer_norm_eps", 1e-12
        )
    )

    tokenizer = Tokenizer.from_file(str(Path(args.model_dir) / "tokenizer.json"))
    tokenizer.enable_truncation(max_length=args.seq)
    tokenizer.enable_padding(length=args.seq, pad_id=0, pad_token="[PAD]")

    with safe_open(str(Path(args.model_dir) / "model.safetensors"), framework="numpy") as handle:
        word = handle.get_tensor("embeddings.word_embeddings.weight")
        position = handle.get_tensor("embeddings.position_embeddings.weight")[: args.seq]
        token_type = handle.get_tensor("embeddings.token_type_embeddings.weight")[0]
        norm_weight = handle.get_tensor("embeddings.LayerNorm.weight")
        norm_bias = handle.get_tensor("embeddings.LayerNorm.bias")

    dst = Path(args.dst)
    dst.mkdir(parents=True, exist_ok=True)
    texts_path = Path(args.texts_out)
    texts_path.parent.mkdir(parents=True, exist_ok=True)
    texts_path.write_text(
        json.dumps(
            {"texts": [
                {"index": i, "id": r["id"], "split": r["split"], "label": r["label"], "text": r["text"]}
                for i, r in enumerate(rows)
            ]},
            indent=2,
            ensure_ascii=False,
        ),
        encoding="utf-8",
    )

    starts = list(range(0, len(rows), args.chunk_size))
    if args.only_chunk >= 0:
        starts = [args.only_chunk * args.chunk_size]
    for start in starts:
        end = min(start + args.chunk_size, len(rows))
        chunk_rows = rows[start:end]
        chunk_dir = dst / f"chunk-{start // args.chunk_size:03d}"
        chunk_dir.mkdir(parents=True, exist_ok=True)
        encodings = tokenizer.encode_batch([r["text"] for r in chunk_rows])
        ids = np.asarray([e.ids for e in encodings], dtype=np.int64)
        mask = np.asarray([e.attention_mask for e in encodings], dtype=np.uint8)
        embedded = word[ids] + position[None, :, :] + token_type[None, None, :]
        mean = embedded.mean(axis=-1, keepdims=True)
        variance = ((embedded - mean) ** 2).mean(axis=-1, keepdims=True)
        normalized = (embedded - mean) / np.sqrt(variance + eps)
        input_rows = normalized * norm_weight + norm_bias

        with open(chunk_dir / "input.f32", "wb") as handle:
            handle.write(matrix_header(len(chunk_rows) * args.seq, hidden))
            handle.write(np.ascontiguousarray(input_rows, dtype=np.float32).tobytes(order="C"))
        with open(chunk_dir / "reference.f32", "wb") as handle:
            handle.write(matrix_header(len(chunk_rows) * args.seq, hidden))
            handle.write(np.zeros((len(chunk_rows) * args.seq, hidden), dtype=np.float32).tobytes(order="C"))
        with open(chunk_dir / "mask.u8", "wb") as handle:
            handle.write(mask_header(len(chunk_rows), args.seq))
            handle.write(np.ascontiguousarray(mask, dtype=np.uint8).reshape(-1).tobytes(order="C"))

        (chunk_dir / "texts.json").write_text(
            json.dumps(
                {"texts": [
                    {
                        "index": start + i,
                        "id": r["id"],
                        "split": r["split"],
                        "label": r["label"],
                        "text": r["text"],
                    }
                    for i, r in enumerate(chunk_rows)
                ]},
                indent=2,
                ensure_ascii=False,
            ),
            encoding="utf-8",
        )
        (chunk_dir / "manifest.json").write_text(
            json.dumps(
                {
                    "dataset": "HateXplain official train+val/test dedup/leak-filtered",
                    "model": str(args.model_dir),
                    "seq": args.seq,
                    "hidden": hidden,
                    "reference": "omitted (chunked; diagnostics unused)",
                    "texts": len(chunk_rows),
                    "train": len(train),
                    "test": len(test),
                    "labels": ["non-hate", "hate"],
                },
                indent=2,
                ensure_ascii=False,
            ),
            encoding="utf-8",
        )
        print(f"{chunk_dir} rows={len(chunk_rows)} bytes={len(chunk_rows) * args.seq * hidden * 4}", flush=True)


if __name__ == "__main__":
    main()

"""Export classification JSONL or GLUE CoLA TSV as bert6 WSPACT1/WSPMSK1 inputs.

The output mirrors ``wsp_export_scifact.py``: ``input.f32`` (texts*seq, hidden),
``reference.f32`` (zeros), ``mask.u8``, plus ``texts.json`` and
``manifest.json``.  Train and test rows share one matrix so one ``bert6``
invocation per mode produces all sentence vectors.

``--format cola`` accepts the GLUE CoLA ``train.tsv`` / ``dev.tsv`` layout
(source, label, asterisk, sentence; dev is used as the held-out test split).
The default ``jsonl`` accepts ``{"text": ..., "label": 0|1}`` per line.

Only the BERT embedding block is needed; the exporter reads it directly from
safetensors and never loads the transformer.

Example:
  python scripts/wsp_export_sst2.py \
    --train raw/train.jsonl --test raw/test.jsonl \
    --out /path/sst2-l12 --model /path/to/all-MiniLM-L12-v2
"""

import argparse
import json
import os
import struct
import sys
import tempfile
from pathlib import Path

SEQ = 64
BATCH = 128
MODEL_ID = "sentence-transformers/all-MiniLM-L12-v2"
SPLITS = ("train", "test")


def _matrix_header(rows, cols):
    return b"WSPACT1\0" + struct.pack("<II", rows, cols)


def _mask_header(batch, seq):
    return b"WSPMSK1\0" + struct.pack("<II", batch, seq)


def open_matrix(path, rows, cols):
    handle = open(path, "wb")
    handle.write(_matrix_header(rows, cols))
    return handle


def append_matrix(handle, array):
    import numpy as np

    handle.write(np.ascontiguousarray(array, dtype=np.float32).tobytes(order="C"))


def open_mask(path, batch, seq):
    handle = open(path, "wb")
    handle.write(_mask_header(batch, seq))
    return handle


def append_mask(handle, mask):
    import numpy as np

    handle.write(
        np.ascontiguousarray(mask, dtype=np.uint8).reshape(-1).tobytes(order="C")
    )


def _load_cola_rows(path, split):
    rows = []
    with open(path, encoding="utf-8") as handle:
        for line_no, line in enumerate(handle, 1):
            if not line.strip():
                continue
            fields = line.rstrip("\n").split("\t")
            if len(fields) != 4:
                raise ValueError(
                    f"{path}:{line_no}: expected 4 CoLA columns, got {len(fields)}"
                )
            _, label_text, _, text = fields
            try:
                label = int(label_text)
            except ValueError as error:
                raise ValueError(
                    f"{path}:{line_no}: label must be 0 or 1, got {label_text!r}"
                ) from error
            if label not in (0, 1):
                raise ValueError(
                    f"{path}:{line_no}: label must be 0 or 1, got {label!r}"
                )
            text = text.strip()
            if not text:
                raise ValueError(f"{path}:{line_no}: empty text")
            rows.append(
                {
                    "id": f"{split}-{len(rows)}",
                    "split": split,
                    "text": text,
                    "label": label,
                }
            )
    if not rows:
        raise ValueError(f"{path}: no rows for split {split}")
    return rows


def _load_rows(path, split, fmt="jsonl"):
    if split not in SPLITS:
        raise ValueError(f"unknown split: {split}")
    if fmt == "cola":
        return _load_cola_rows(path, split)
    if fmt != "jsonl":
        raise ValueError(f"unknown format: {fmt}")
    rows = []
    with open(path, encoding="utf-8") as handle:
        for line_no, line in enumerate(handle, 1):
            if not line.strip():
                continue
            obj = json.loads(line)
            text = str(obj.get("text") or "").strip()
            label = obj.get("label")
            if label not in (0, 1):
                raise ValueError(
                    f"{path}:{line_no}: label must be 0 or 1, got {label!r}"
                )
            if not text:
                raise ValueError(f"{path}:{line_no}: empty text")
            rows.append(
                {
                    "id": f"{split}-{len(rows)}",
                    "split": split,
                    "text": text,
                    "label": int(label),
                }
            )
    if not rows:
        raise ValueError(f"{path}: no rows for split {split}")
    return rows


def parse_args():
    parser = argparse.ArgumentParser()
    parser.add_argument("--train", default="")
    parser.add_argument("--test", default="")
    parser.add_argument("--out", default="")
    parser.add_argument("--format", choices=("jsonl", "cola"), default="jsonl")
    parser.add_argument("--model", default=MODEL_ID)
    parser.add_argument("--seq", type=int, default=SEQ)
    parser.add_argument("--batch", type=int, default=BATCH)
    parser.add_argument("--limit-train", type=int, default=0)
    parser.add_argument("--limit-test", type=int, default=0)
    parser.add_argument("--self-test", action="store_true")
    return parser.parse_args()


def main():
    args = parse_args()
    if args.self_test:
        self_test()
        return
    if not args.train or not args.test or not args.out:
        raise SystemExit("--train, --test and --out are required")
    if args.seq < 1 or args.batch < 1:
        raise SystemExit("--seq/--batch must be positive")

    os.environ.setdefault("OPENBLAS_NUM_THREADS", "1")
    os.environ.setdefault("OMP_NUM_THREADS", "1")
    import numpy as np
    from safetensors import safe_open
    from tokenizers import Tokenizer

    train = _load_rows(args.train, "train", args.format)
    test = _load_rows(args.test, "test", args.format)
    if args.limit_train > 0:
        train = train[: args.limit_train]
    if args.limit_test > 0:
        test = test[: args.limit_test]
    texts = train + test
    if not texts:
        raise SystemExit("empty dataset after limits")

    model_dir = Path(args.model)
    config = json.loads((model_dir / "config.json").read_text(encoding="utf-8"))
    hidden = int(config["hidden_size"])
    eps = float(config.get("layer_norm_eps", 1e-12))
    tokenizer = Tokenizer.from_file(str(model_dir / "tokenizer.json"))
    tokenizer.enable_truncation(max_length=args.seq)
    tokenizer.enable_padding(length=args.seq, pad_id=0, pad_token="[PAD]")

    with safe_open(str(model_dir / "model.safetensors"), framework="numpy") as handle:
        word = handle.get_tensor("embeddings.word_embeddings.weight")
        position = handle.get_tensor("embeddings.position_embeddings.weight")[
            : args.seq
        ]
        token_type = handle.get_tensor("embeddings.token_type_embeddings.weight")[0]
        norm_weight = handle.get_tensor("embeddings.LayerNorm.weight")
        norm_bias = handle.get_tensor("embeddings.LayerNorm.bias")

    os.makedirs(args.out, exist_ok=True)
    input_handle = open_matrix(
        os.path.join(args.out, "input.f32"), len(texts) * args.seq, hidden
    )
    ref_handle = open_matrix(
        os.path.join(args.out, "reference.f32"), len(texts) * args.seq, hidden
    )
    mask_handle = open_mask(os.path.join(args.out, "mask.u8"), len(texts), args.seq)
    try:
        for start in range(0, len(texts), args.batch):
            end = min(start + args.batch, len(texts))
            batch = texts[start:end]
            encodings = tokenizer.encode_batch([row["text"] for row in batch])
            ids = np.asarray([encoding.ids for encoding in encodings], dtype=np.int64)
            mask = np.asarray(
                [encoding.attention_mask for encoding in encodings], dtype=np.uint8
            )
            embedded = word[ids] + position[None, :, :] + token_type[None, None, :]
            mean = embedded.mean(axis=-1, keepdims=True)
            variance = ((embedded - mean) ** 2).mean(axis=-1, keepdims=True)
            normalized = (embedded - mean) / np.sqrt(variance + eps)
            input_rows = normalized * norm_weight + norm_bias
            append_matrix(input_handle, input_rows.reshape(-1, hidden))
            append_matrix(
                ref_handle, np.zeros((len(batch) * args.seq, hidden), dtype=np.float32)
            )
            append_mask(mask_handle, mask.reshape(-1))
            print(f"chunk {start}-{end}/{len(texts)} ok", flush=True)
    finally:
        input_handle.close()
        ref_handle.close()
        mask_handle.close()

    rows = [
        {
            "index": index,
            "id": row["id"],
            "split": row["split"],
            "label": row["label"],
            "text": row["text"],
        }
        for index, row in enumerate(texts)
    ]
    with open(os.path.join(args.out, "texts.json"), "w", encoding="utf-8") as handle:
        json.dump({"texts": rows}, handle, indent=2, ensure_ascii=False)
    with open(os.path.join(args.out, "manifest.json"), "w", encoding="utf-8") as handle:
        json.dump(
            {
                "dataset": (
                    "GLUE/CoLA (dev as test)"
                    if args.format == "cola"
                    else "SetFit/sst2"
                ),
                "model": args.model,
                "seq": args.seq,
                "hidden": hidden,
                "reference": "zeros (task-head export; diagnostics unused)",
                "texts": len(texts),
                "train": len(train),
                "test": len(test),
                "labels": (
                    {"unacceptable": 0, "acceptable": 1}
                    if args.format == "cola"
                    else {"negative": 0, "positive": 1}
                ),
            },
            handle,
            indent=2,
            ensure_ascii=False,
        )
    print(f"done texts={len(texts)} train={len(train)} test={len(test)}")


def self_test():
    matrix = _matrix_header(127, 384)
    assert matrix[:8] == b"WSPACT1\0"
    assert struct.unpack("<II", matrix[8:]) == (127, 384)
    assert _mask_header(2000, SEQ)[:8] == b"WSPMSK1\0"

    tmp = tempfile.mkdtemp(prefix="sst2-export-self-")
    good = Path(tmp) / "good.jsonl"
    good.write_text(
        '{"text": "  a good film  ", "label": 1}\n'
        '{"text": "a bad film", "label": 0}\n',
        encoding="utf-8",
    )
    rows = _load_rows(good, "train")
    assert rows == [
        {"id": "train-0", "split": "train", "text": "a good film", "label": 1},
        {"id": "train-1", "split": "train", "text": "a bad film", "label": 0},
    ]

    bad_label = Path(tmp) / "bad-label.jsonl"
    bad_label.write_text('{"text": "x", "label": 2}\n', encoding="utf-8")
    try:
        _load_rows(bad_label, "test")
    except ValueError:
        pass
    else:
        raise AssertionError("label 2 must be rejected")

    empty = Path(tmp) / "empty.jsonl"
    empty.write_text('{"text": "   ", "label": 1}\n', encoding="utf-8")
    try:
        _load_rows(empty, "test")
    except ValueError:
        pass
    else:
        raise AssertionError("empty text must be rejected")

    cola = Path(tmp) / "cola.tsv"
    cola.write_text(
        "gj04\t1\t\tThe sailors rode the breeze clear of the rocks.\n"
        "cj99\t0\t*\tAs you eat the most, you want the least.\n",
        encoding="utf-8",
    )
    cola_rows = _load_rows(cola, "test", "cola")
    assert cola_rows == [
        {
            "id": "test-0",
            "split": "test",
            "text": "The sailors rode the breeze clear of the rocks.",
            "label": 1,
        },
        {
            "id": "test-1",
            "split": "test",
            "text": "As you eat the most, you want the least.",
            "label": 0,
        },
    ]
    bad_cola = Path(tmp) / "bad-cola.tsv"
    bad_cola.write_text("gj04\t2\t\tx\n", encoding="utf-8")
    try:
        _load_rows(bad_cola, "test", "cola")
    except ValueError:
        pass
    else:
        raise AssertionError("CoLA label 2 must be rejected")
    print("self-test ok")


if __name__ == "__main__":
    sys.exit(main())

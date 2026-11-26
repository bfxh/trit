"""Export an external STS pair set as bert6 WSPACT1/WSPMSK1 inputs.

The output is the same layout used by ``miniLM-l12-sts-full``:
``input.f32`` (pair*2, hidden), ``reference.f32`` (pair*2, hidden),
``mask.u8`` (pair*2, seq), ``pairs.json`` and ``manifest.json``.

Only the first encoder layer input and the last hidden state are kept, so a
1,000-pair SICK-R export does not hold all intermediate activations.

Example:
  python scripts/wsp_export_sts_pairs.py \
    --dataset mteb/sickr-sts --split test --pairs 1000 \
    --out /path/miniLM-l12-sickr-1000 \
    --model sentence-transformers/all-MiniLM-L12-v2 --local-only
"""

import argparse
import json
import os
import struct
import sys

SEQ = 64
CHUNK_PAIRS = 128
MODEL_ID = "sentence-transformers/all-MiniLM-L12-v2"


def _matrix_header(rows, cols):
    return b"WSPACT1\0" + struct.pack("<II", rows, cols)


def _mask_header(batch, seq):
    # WSPMSK1 header stores the number of *sentences* (pair*2), not pairs.
    return b"WSPMSK1\0" + struct.pack("<II", batch, seq)


def write_matrix(path, array):
    import numpy as np

    array = np.ascontiguousarray(array, dtype=np.float32)
    if array.ndim != 2:
        raise ValueError(f"{path}: expected 2-D, got {array.shape}")
    rows, cols = int(array.shape[0]), int(array.shape[1])
    with open(path, "wb") as handle:
        handle.write(_matrix_header(rows, cols))
        handle.write(array.tobytes(order="C"))


def write_mask(path, mask, batch, seq):
    import numpy as np

    mask = np.ascontiguousarray(mask, dtype=np.uint8).reshape(-1)
    if mask.size != batch * seq:
        raise ValueError(f"{path}: mask {mask.size} != {batch * seq}")
    with open(path, "wb") as handle:
        handle.write(_mask_header(batch, seq))
        handle.write(mask.tobytes(order="C"))


def self_test():
    matrix = _matrix_header(127, 384)
    assert matrix[:8] == b"WSPACT1\0"
    assert struct.unpack("<II", matrix[8:]) == (127, 384)
    mask = _mask_header(2000, SEQ)
    assert mask[:8] == b"WSPMSK1\0"
    assert struct.unpack("<II", mask[8:]) == (2000, SEQ)
    print("self-test ok")


def parse_args():
    parser = argparse.ArgumentParser()
    parser.add_argument("--dataset", default="mteb/sickr-sts")
    parser.add_argument("--split", default="test")
    parser.add_argument("--pairs", type=int, default=1000)
    parser.add_argument("--out", default="")
    parser.add_argument("--model", default=MODEL_ID)
    parser.add_argument("--seq", type=int, default=SEQ)
    parser.add_argument("--chunk-pairs", type=int, default=CHUNK_PAIRS)
    parser.add_argument("--local-only", action="store_true")
    parser.add_argument("--self-test", action="store_true")
    return parser.parse_args()


def main():
    args = parse_args()
    if args.self_test:
        self_test()
        return
    if not args.out:
        raise SystemExit("--out is required")
    if args.pairs < 1 or args.seq < 1 or args.chunk_pairs < 1:
        raise SystemExit("--pairs/--seq/--chunk-pairs must be positive")

    import numpy as np
    import torch
    from datasets import load_dataset
    from transformers import AutoModel, AutoTokenizer

    dataset = load_dataset(args.dataset, split=args.split)
    n_pairs = min(args.pairs, len(dataset))
    dataset = dataset.select(range(n_pairs))
    score_key = "score" if "score" in dataset.column_names else None
    pairs = [
        {
            "a": 2 * i,
            "b": 2 * i + 1,
            "score": None if score_key is None else float(dataset[score_key][i]),
        }
        for i in range(n_pairs)
    ]

    tokenizer = AutoTokenizer.from_pretrained(args.model, local_files_only=args.local_only)
    model = AutoModel.from_pretrained(args.model, local_files_only=args.local_only)
    model.eval()

    q0_chunks, ref_chunks, mask_chunks = [], [], []
    for start in range(0, n_pairs, args.chunk_pairs):
        end = min(start + args.chunk_pairs, n_pairs)
        texts = []
        for i in range(start, end):
            texts.extend([dataset["sentence1"][i], dataset["sentence2"][i]])
        encoded = tokenizer(
            texts,
            padding="max_length",
            truncation=True,
            max_length=args.seq,
            return_tensors="pt",
        )
        captured = {}

        def hook(_module, inputs, _output, captured=captured):
            captured["q0"] = inputs[0].detach()

        handle = model.encoder.layer[0].attention.self.query.register_forward_hook(hook)
        with torch.no_grad():
            output = model(**encoded)
        handle.remove()
        q0 = captured["q0"].reshape(-1, captured["q0"].shape[-1])
        ref = output.last_hidden_state.reshape(-1, output.last_hidden_state.shape[-1])
        q0_chunks.append(q0.numpy())
        ref_chunks.append(ref.numpy())
        mask_chunks.append(encoded["attention_mask"].bool().reshape(-1).to(torch.uint8).numpy())
        print(f"chunk {start}-{end} ok", flush=True)

    os.makedirs(args.out, exist_ok=True)
    input_array = np.concatenate(q0_chunks, axis=0)
    reference = np.concatenate(ref_chunks, axis=0)
    mask = np.concatenate(mask_chunks, axis=0)
    batch = n_pairs * 2
    write_matrix(os.path.join(args.out, "input.f32"), input_array)
    write_matrix(os.path.join(args.out, "reference.f32"), reference)
    write_mask(os.path.join(args.out, "mask.u8"), mask, batch, args.seq)
    with open(os.path.join(args.out, "pairs.json"), "w", encoding="utf-8") as handle:
        json.dump({"pairs": pairs}, handle, indent=2, ensure_ascii=False)
    unique_sentences = len({x for i in range(n_pairs) for x in
                            (dataset["sentence1"][i], dataset["sentence2"][i])})
    with open(os.path.join(args.out, "manifest.json"), "w", encoding="utf-8") as handle:
        json.dump(
            {
                "model": args.model,
                "dataset": args.dataset,
                "split": args.split,
                "text_source": f"{args.dataset} {args.split} (first {n_pairs} pairs)",
                "pairs": n_pairs,
                "batch": batch,
                "seq": args.seq,
                "hidden": int(input_array.shape[1]),
                "unique_sentences": unique_sentences,
                "sentence_disjoint": unique_sentences == batch,
            },
            handle,
            indent=2,
            ensure_ascii=False,
        )
    print(f"done pairs={n_pairs} batch={batch} unique_sentences={unique_sentences}")


if __name__ == "__main__":
    sys.exit(main())

"""Export SciFact corpus/queries/qrels as bert6 WSPACT1/WSPMSK1 inputs.

The output is the same layout used by ``miniLM-*-sts-full``:
``input.f32`` (texts*seq, hidden), ``reference.f32`` (texts*seq, hidden),
``mask.u8`` (texts, seq), plus ``texts.json``, ``qrels.json`` and
``manifest.json``.  Corpus and query rows share one embedding matrix so a
retrieval run only needs one ``bert6`` invocation per mode.

We only need the BERT embedding block (word + position + token-type + LayerNorm)
for ``input.f32``; this exporter reads it directly from safetensors and never
loads the transformer, keeping peak memory low.  ``reference.f32`` is zeros:
``bert6`` only uses it for diagnostics, not for the task-head embeddings.

Example:
  python scripts/wsp_export_scifact.py \
    --out /path/scifact-l12 --model /path/to/all-MiniLM-L12-v2 \
    --local-only
"""

import argparse
import json
import os
import struct
import sys
from pathlib import Path

SEQ = 64
BATCH = 128
MODEL_ID = "sentence-transformers/all-MiniLM-L12-v2"


def _matrix_header(rows, cols):
    return b"WSPACT1\0" + struct.pack("<II", rows, cols)


def _mask_header(batch, seq):
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

    handle.write(np.ascontiguousarray(mask, dtype=np.uint8).reshape(-1).tobytes(order="C"))


def write_mask(path, mask, batch, seq):
    import numpy as np

    mask = np.ascontiguousarray(mask, dtype=np.uint8).reshape(-1)
    if mask.size != batch * seq:
        raise ValueError(f"{path}: mask {mask.size} != {batch * seq}")
    with open(path, "wb") as handle:
        handle.write(_mask_header(batch, seq))
        handle.write(mask.tobytes(order="C"))


def parse_args():
    parser = argparse.ArgumentParser()
    parser.add_argument("--dataset", default="mteb/scifact")
    parser.add_argument("--out", default="")
    parser.add_argument("--model", default=MODEL_ID)
    parser.add_argument("--seq", type=int, default=SEQ)
    parser.add_argument("--batch", type=int, default=BATCH)
    parser.add_argument("--limit-corpus", type=int, default=0)
    parser.add_argument("--limit-queries", type=int, default=0)
    parser.add_argument("--local-only", action="store_true")
    parser.add_argument("--self-test", action="store_true")
    return parser.parse_args()


def _texts(corpus, queries):
    rows = []
    for row in corpus:
        title = (row.get("title") or "").strip()
        body = (row.get("text") or "").strip()
        text = f"{title} {body}".strip()
        rows.append({"id": str(row["_id"]), "kind": "corpus", "text": text})
    for row in queries:
        rows.append({"id": str(row["_id"]), "kind": "query", "text": str(row.get("text") or "")})
    return rows


def _qrels(rows, corpus_ids):
    by_id = {}
    for row in rows:
        query = str(row["query-id"])
        corpus = str(row["corpus-id"])
        if corpus not in corpus_ids:
            continue
        by_id.setdefault(query, []).append(corpus)
    return {query: sorted(set(docs)) for query, docs in by_id.items()}


def main():
    args = parse_args()
    if args.self_test:
        self_test()
        return
    if not args.out:
        raise SystemExit("--out is required")
    if args.seq < 1 or args.batch < 1:
        raise SystemExit("--seq/--batch must be positive")

    os.environ.setdefault("OPENBLAS_NUM_THREADS", "1")
    os.environ.setdefault("OMP_NUM_THREADS", "1")
    import numpy as np
    from datasets import load_dataset
    from safetensors import safe_open
    from tokenizers import Tokenizer

    corpus = load_dataset(args.dataset, "corpus", split="corpus")
    queries = load_dataset(args.dataset, "queries", split="queries")
    try:
        qrels = load_dataset(args.dataset, "default", split="test")
    except Exception:
        qrels = load_dataset(args.dataset, split="test")
    if args.limit_corpus > 0:
        corpus = corpus.select(range(min(args.limit_corpus, len(corpus))))
    if args.limit_queries > 0:
        queries = queries.select(range(min(args.limit_queries, len(queries))))

    texts = _texts(corpus, queries)
    corpus_ids = {row["id"] for row in texts if row["kind"] == "corpus"}
    qrels_map = _qrels(qrels, corpus_ids)
    query_ids = {row["id"] for row in texts if row["kind"] == "query"}
    missing = sorted(set(qrels_map) - query_ids)
    if missing:
        print(f"warning: {len(missing)} qrels queries missing from queries split", flush=True)

    model_dir = Path(args.model)
    config = json.loads((model_dir / "config.json").read_text(encoding="utf-8"))
    hidden = int(config["hidden_size"])
    eps = float(config.get("layer_norm_eps", 1e-12))
    tokenizer = Tokenizer.from_file(str(model_dir / "tokenizer.json"))
    tokenizer.enable_truncation(max_length=args.seq)
    tokenizer.enable_padding(length=args.seq, pad_id=0, pad_token="[PAD]")

    with safe_open(str(model_dir / "model.safetensors"), framework="numpy") as handle:
        word = handle.get_tensor("embeddings.word_embeddings.weight")
        position = handle.get_tensor("embeddings.position_embeddings.weight")[: args.seq]
        token_type = handle.get_tensor("embeddings.token_type_embeddings.weight")[0]
        norm_weight = handle.get_tensor("embeddings.LayerNorm.weight")
        norm_bias = handle.get_tensor("embeddings.LayerNorm.bias")

    os.makedirs(args.out, exist_ok=True)
    input_handle = open_matrix(os.path.join(args.out, "input.f32"), len(texts) * args.seq, hidden)
    ref_handle = open_matrix(os.path.join(args.out, "reference.f32"), len(texts) * args.seq, hidden)
    mask_handle = open_mask(os.path.join(args.out, "mask.u8"), len(texts), args.seq)
    try:
        for start in range(0, len(texts), args.batch):
            end = min(start + args.batch, len(texts))
            batch = texts[start:end]
            encodings = tokenizer.encode_batch([row["text"] for row in batch])
            ids = np.asarray([encoding.ids for encoding in encodings], dtype=np.int64)
            mask = np.asarray([encoding.attention_mask for encoding in encodings], dtype=np.uint8)
            embedded = word[ids] + position[None, :, :] + token_type[None, None, :]
            mean = embedded.mean(axis=-1, keepdims=True)
            variance = ((embedded - mean) ** 2).mean(axis=-1, keepdims=True)
            normalized = (embedded - mean) / np.sqrt(variance + eps)
            input_rows = normalized * norm_weight + norm_bias
            append_matrix(input_handle, input_rows.reshape(-1, hidden))
            append_matrix(ref_handle, np.zeros((len(batch) * args.seq, hidden), dtype=np.float32))
            append_mask(mask_handle, mask.reshape(-1))
            print(f"chunk {start}-{end}/{len(texts)} ok", flush=True)
    finally:
        input_handle.close()
        ref_handle.close()
        mask_handle.close()

    texts_rows = [
        {"index": index, "id": row["id"], "kind": row["kind"]}
        for index, row in enumerate(texts)
    ]
    with open(os.path.join(args.out, "texts.json"), "w", encoding="utf-8") as handle:
        json.dump({"texts": texts_rows}, handle, indent=2, ensure_ascii=False)
    with open(os.path.join(args.out, "qrels.json"), "w", encoding="utf-8") as handle:
        json.dump(qrels_map, handle, indent=2, ensure_ascii=False)
    with open(os.path.join(args.out, "manifest.json"), "w", encoding="utf-8") as handle:
        json.dump(
            {
                "dataset": args.dataset,
                "model": args.model,
                "seq": args.seq,
                "hidden": hidden,
                "reference": "zeros (task-head export; diagnostics unused)",
                "texts": len(texts),
                "corpus": len(corpus),
                "queries": len(queries),
                "qrels_queries": len(qrels_map),
                "qrels_pairs": sum(len(docs) for docs in qrels_map.values()),
            },
            handle,
            indent=2,
            ensure_ascii=False,
        )
    print(
        f"done texts={len(texts)} corpus={len(corpus)} queries={len(queries)} "
        f"qrels_queries={len(qrels_map)}"
    )


def self_test():
    matrix = _matrix_header(127, 384)
    assert matrix[:8] == b"WSPACT1\0"
    assert struct.unpack("<II", matrix[8:]) == (127, 384)
    assert _mask_header(2000, SEQ)[:8] == b"WSPMSK1\0"
    texts = _texts(
        [{"_id": "c1", "title": "T", "text": "B"}],
        [{"_id": "q1", "text": "Q"}],
    )
    assert texts == [
        {"id": "c1", "kind": "corpus", "text": "T B"},
        {"id": "q1", "kind": "query", "text": "Q"},
    ]
    assert _qrels([{"query-id": "q1", "corpus-id": "c1"}], {"c1"}) == {"q1": ["c1"]}
    assert _qrels([{"query-id": "q1", "corpus-id": "missing"}], {"c1"}) == {}
    print("self-test ok")


if __name__ == "__main__":
    sys.exit(main())

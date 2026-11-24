"""Split a WSPACT1 task-head directory into chunk-XXX subdirectories.

Large task heads (SST-2 at 8,741 texts) make ``bert6`` load two 859 MB
matrices at once, which can exceed the process memory budget.  Chunking keeps
each invocation small; the WSPEMB1 outputs are concatenated afterwards.

Example:
  python scripts/wsp_chunk_embed_input.py \
    --src sst2/data-l12 --dst sst2/chunks-l12 --chunk-size 4000
"""

import argparse
import json
import os
import struct
import sys
import tempfile
from pathlib import Path

MATRIX_MAGIC = b"WSPACT1\0"
MASK_MAGIC = b"WSPMSK1\0"
COPY_BYTES = 64 << 20


def _read_header(path, magic):
    with open(path, "rb") as handle:
        head = handle.read(16)
    if len(head) != 16 or head[:8] != magic:
        raise ValueError(f"{path}: bad header")
    return struct.unpack("<II", head[8:])


def _write_header(handle, magic, rows, cols):
    handle.write(magic + struct.pack("<II", rows, cols))


def _copy_range(src, dst, src_offset, dst_offset, length):
    with open(src, "rb") as source, open(dst, "r+b") as target:
        source.seek(src_offset)
        target.seek(dst_offset)
        left = length
        while left > 0:
            block = source.read(min(COPY_BYTES, left))
            if not block:
                raise ValueError(f"{src}: short read at {src_offset + length - left}")
            target.write(block)
            left -= len(block)


def _write_matrix(path, array):
    import numpy as np

    array = np.ascontiguousarray(array, dtype=np.float32)
    rows, cols = array.shape
    with open(path, "wb") as handle:
        _write_header(handle, MATRIX_MAGIC, int(rows), int(cols))
        handle.write(array.tobytes(order="C"))


def _write_mask(path, mask):
    import numpy as np

    mask = np.ascontiguousarray(mask, dtype=np.uint8)
    rows, cols = mask.shape
    with open(path, "wb") as handle:
        _write_header(handle, MASK_MAGIC, int(rows), int(cols))
        handle.write(mask.tobytes(order="C"))


def split(src, dst, chunk_size):
    if chunk_size < 1:
        raise ValueError("chunk_size must be positive")
    src = Path(src)
    texts = json.loads((src / "texts.json").read_text(encoding="utf-8"))["texts"]
    rows, hidden = _read_header(src / "input.f32", MATRIX_MAGIC)
    ref_rows, ref_cols = _read_header(src / "reference.f32", MATRIX_MAGIC)
    mask_rows, seq = _read_header(src / "mask.u8", MASK_MAGIC)
    if len(texts) != rows / seq:
        raise ValueError(f"texts {len(texts)} != input rows {rows} / seq {seq}")
    if ref_rows != rows or ref_cols != hidden:
        raise ValueError("reference shape != input shape")
    if mask_rows * seq != rows:
        raise ValueError("mask shape != input rows")

    chunks = []
    for index, start in enumerate(range(0, len(texts), chunk_size)):
        end = min(start + chunk_size, len(texts))
        count = end - start
        out = Path(dst) / f"chunk-{index:03d}"
        out.mkdir(parents=True, exist_ok=True)
        input_path = out / "input.f32"
        with open(input_path, "wb") as handle:
            _write_header(handle, MATRIX_MAGIC, count * seq, hidden)
        _copy_range(
            src / "input.f32",
            input_path,
            16 + start * seq * hidden * 4,
            16,
            count * seq * hidden * 4,
        )

        ref_path = out / "reference.f32"
        with open(ref_path, "wb") as handle:
            _write_header(handle, MATRIX_MAGIC, count * seq, hidden)
        _copy_range(
            src / "reference.f32",
            ref_path,
            16 + start * seq * hidden * 4,
            16,
            count * seq * hidden * 4,
        )

        mask_path = out / "mask.u8"
        with open(mask_path, "wb") as handle:
            _write_header(handle, MASK_MAGIC, count, seq)
        _copy_range(src / "mask.u8", mask_path, 16 + start * seq, 16, count * seq)

        rows_out = [
            {"index": local, "id": row["id"], "split": row["split"], "label": row["label"]}
            for local, row in enumerate(texts[start:end])
        ]
        (out / "texts.json").write_text(
            json.dumps({"texts": rows_out}, indent=2, ensure_ascii=False),
            encoding="utf-8",
        )
        (out / "manifest.json").write_text(
            json.dumps(
                {
                    "source": str(src),
                    "chunk": index,
                    "start": start,
                    "texts": count,
                    "seq": seq,
                    "hidden": hidden,
                },
                indent=2,
                ensure_ascii=False,
            ),
            encoding="utf-8",
        )
        chunks.append({"dir": out.name, "start": start, "texts": count})
    return chunks


def parse_args():
    parser = argparse.ArgumentParser()
    parser.add_argument("--src", default="")
    parser.add_argument("--dst", default="")
    parser.add_argument("--chunk-size", type=int, default=4000)
    parser.add_argument("--self-test", action="store_true")
    return parser.parse_args()


def main():
    args = parse_args()
    if args.self_test:
        self_test()
        return
    if not args.src or not args.dst:
        raise SystemExit("--src and --dst are required")
    chunks = split(args.src, args.dst, args.chunk_size)
    print(f"done chunks={len(chunks)}")
    for chunk in chunks:
        print(f"  {chunk['dir']} start={chunk['start']} texts={chunk['texts']}")


def self_test():
    import numpy as np

    tmp = Path(tempfile.mkdtemp(prefix="chunk-self-"))
    src = tmp / "src"
    src.mkdir()
    texts = [
        {"index": 0, "id": "train-0", "split": "train", "label": 1},
        {"index": 1, "id": "train-1", "split": "train", "label": 0},
        {"index": 2, "id": "test-0", "split": "test", "label": 1},
    ]
    (src / "texts.json").write_text(json.dumps({"texts": texts}), encoding="utf-8")
    _write_matrix(src / "input.f32", np.arange(12, dtype=np.float32).reshape(6, 2))
    _write_matrix(src / "reference.f32", np.zeros((6, 2), dtype=np.float32))
    _write_mask(src / "mask.u8", np.ones((3, 2), dtype=np.uint8))

    chunks = split(src, tmp / "out", 2)
    assert chunks == [
        {"dir": "chunk-000", "start": 0, "texts": 2},
        {"dir": "chunk-001", "start": 2, "texts": 1},
    ]
    assert _read_header(tmp / "out" / "chunk-000" / "input.f32", MATRIX_MAGIC) == (4, 2)
    assert _read_header(tmp / "out" / "chunk-001" / "input.f32", MATRIX_MAGIC) == (2, 2)
    assert _read_header(tmp / "out" / "chunk-001" / "mask.u8", MASK_MAGIC) == (1, 2)
    out_texts = json.loads((tmp / "out" / "chunk-001" / "texts.json").read_text(encoding="utf-8"))
    assert out_texts["texts"] == [
        {"index": 0, "id": "test-0", "split": "test", "label": 1}
    ]

    bad = tmp / "bad"
    bad.mkdir()
    _write_matrix(bad / "input.f32", np.zeros((4, 2), dtype=np.float32))
    _write_matrix(bad / "reference.f32", np.zeros((4, 2), dtype=np.float32))
    _write_mask(bad / "mask.u8", np.ones((2, 2), dtype=np.uint8))
    (bad / "texts.json").write_text('{"texts": [{"id": "x", "split": "test", "label": 1}]}', encoding="utf-8")
    try:
        split(bad, tmp / "bad-out", 2)
    except ValueError:
        pass
    else:
        raise AssertionError("text/row mismatch must be rejected")

    bad_header = tmp / "bad-header.f32"
    with open(bad_header, "wb") as handle:
        handle.write(b"WSPACT2\0" + struct.pack("<II", 2, 2))
    try:
        _read_header(bad_header, MATRIX_MAGIC)
    except ValueError:
        pass
    else:
        raise AssertionError("bad matrix magic must be rejected")
    print("self-test ok")


if __name__ == "__main__":
    sys.exit(main())

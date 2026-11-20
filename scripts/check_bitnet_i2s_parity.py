#!/usr/bin/env python3
"""J7：把 BitNet-b1.58-2B-4T 的 GGUF I2_S 张量与 safetensors 同张量逐字节对拍。

做法：只拉需要的两段（HTTP Range），不下载 1.18 GB 全文件。
- GGUF：解析头部拿 `blk.0.ffn_down.weight` 的 offset，拉 n/4 + 32 字节
- safetensors：解析 JSON 头拿 `model.layers.0.mlp.down_proj.weight` 的 data_offsets
- 把 safetensors 的 U8（2 bit 低位先、每 4 个值一组、4 组按 640 行分块）重建成
  (2560, 6912) 矩阵，按 I2_S 规格（128 值块 reshape(4,32) 交错）打包，与 GGUF 比较

需要 numpy。联网 + 约 9 MB 流量，不进常规 CI（registry 里登记为 heavy/手动）。
"""

from __future__ import annotations

import json
import struct
import sys
import urllib.request

import numpy as np

GGUF_URL = "https://huggingface.co/microsoft/bitnet-b1.58-2B-4T-gguf/resolve/main/ggml-model-i2_s.gguf"
ST_URL = "https://huggingface.co/microsoft/bitnet-b1.58-2B-4T/resolve/main/model.safetensors"
TENSORS = [
    ("blk.0.ffn_down.weight", "model.layers.0.mlp.down_proj.weight"),
    ("blk.0.attn_k.weight", "model.layers.0.self_attn.k_proj.weight"),
]
SIZES = {0: 1, 1: 1, 2: 2, 3: 2, 4: 4, 5: 4, 6: 4, 7: 1, 10: 8, 11: 8, 12: 8}


def fetch(url: str, start: int, length: int) -> bytes:
    req = urllib.request.Request(url, headers={"Range": f"bytes={start}-{start + length - 1}"})
    with urllib.request.urlopen(req, timeout=120) as resp:
        data = resp.read()
    if len(data) != length:
        raise SystemExit(f"range 长度不足：{len(data)} != {length}")
    return data


def gguf_tensor(head: bytes, want: str) -> tuple[list[int], int]:
    """在 GGUF 头里找张量，返回 (dims, 绝对数据偏移)。"""
    pos = 4

    def u32() -> int:
        nonlocal pos
        v = struct.unpack_from("<I", head, pos)[0]
        pos += 4
        return v

    def u64() -> int:
        nonlocal pos
        v = struct.unpack_from("<Q", head, pos)[0]
        pos += 8
        return v

    def s() -> str:
        nonlocal pos
        n = u64()
        v = head[pos:pos + n].decode("utf-8", "replace")
        pos += n
        return v

    def skip(t: int) -> None:
        nonlocal pos
        if t == 8:
            s()
        elif t == 9:
            et, n = u32(), u64()
            if et == 8:
                for _ in range(n):
                    s()
            else:
                pos += SIZES[et] * n
        else:
            pos += SIZES[t]

    u32()
    tensor_count, kv_count = u64(), u64()
    for _ in range(kv_count):
        s()
        skip(u32())
    found = None
    for _ in range(tensor_count):
        name = s()
        nd = u32()
        dims = [u64() for _ in range(nd)]
        tt = u32()
        off = u64()
        if name == want:
            if tt != 36:
                raise SystemExit(f"{want} 的 type 是 {tt}，不是 I2_S(36)")
            found = (dims, off)
    if found is None:
        raise SystemExit(f"GGUF 头里没有 {want}")
    data_start = (pos + 31) // 32 * 32
    return found[0], data_start + found[1]


def st_tensor(head: bytes, want: str) -> tuple[list[int], int, int]:
    """在 safetensors 头里找张量，返回 (shape, 绝对起点, 绝对终点)。"""
    n = struct.unpack_from("<Q", head, 0)[0]
    meta = json.loads(head[8:8 + n].decode("utf-8"))
    if want not in meta:
        raise SystemExit(f"safetensors 头里没有 {want}")
    info = meta[want]
    base = 8 + n
    return info["shape"], base + info["data_offsets"][0], base + info["data_offsets"][1]


def pack_i2s(matrix: np.ndarray) -> bytes:
    flat = np.ascontiguousarray(matrix, dtype=np.uint8).reshape(-1)
    q = flat.reshape(-1, 4, 32)
    packed = (q[:, 0, :] << 6) | (q[:, 1, :] << 4) | (q[:, 2, :] << 2) | q[:, 3, :]
    return packed.reshape(-1).astype(np.uint8).tobytes()


def check(head: bytes, gguf_name: str, st_name: str) -> bool:
    dims, gguf_at = gguf_tensor(head, gguf_name)
    rows, cols = dims[1], dims[0]  # ne0 最快变化 → 逻辑 (ne1, ne0)
    n = rows * cols
    gguf = fetch(GGUF_URL, gguf_at, n // 4 + 32)

    st_head = fetch(ST_URL, 0, 1 << 20)
    shape, st_at, st_end = st_tensor(st_head, st_name)
    if shape[0] * 4 != rows or shape[1] != cols:
        raise SystemExit(f"形状对不上：safetensors {shape} vs GGUF {(rows, cols)}")
    raw = fetch(ST_URL, st_at, st_end - st_at)

    matrix = np.zeros((rows, cols), dtype=np.uint8)
    b = np.frombuffer(raw, dtype=np.uint8).reshape(rows // 4, cols)
    for j, sh in enumerate((0, 2, 4, 6)):
        matrix[j * (rows // 4):(j + 1) * (rows // 4), :] = (b >> sh) & 3

    packed = pack_i2s(matrix)
    want = gguf[: n // 4]
    if packed != want:
        bad = sum(1 for x, y in zip(packed, want) if x != y)
        print(f"FAIL {gguf_name}: {bad}/{len(want)} 字节不同")
        return False
    scale = struct.unpack("<f", gguf[n // 4:n // 4 + 4])[0]
    tail_zero = set(gguf[n // 4 + 4:]) == {0}
    print(f"PASS {gguf_name}: {len(want)} 字节逐字节一致（无需头重排）")
    print(f"     scale={scale} 尾 28 字节全零={tail_zero} 体积={n // 4 + 32}B/{n} 权重")
    return True


def main() -> int:
    head = fetch(GGUF_URL, 0, 8 << 20)
    ok = True
    for gguf_name, st_name in TENSORS:
        ok = check(head, gguf_name, st_name) and ok
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())

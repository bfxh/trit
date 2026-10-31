"""Objaverse 1.0 子集下载（对象级 + 许可过滤）。

用法：python scripts/fetch_objaverse.py [OUT_DIR] [COUNT] [SHARD] [LICENSES]
  OUT_DIR   默认 D:/KF/trit-data/objaverse
  COUNT     默认 20
  SHARD     默认 000-000（metadata 与 glbs 同名分片，每片 5000 条元数据 / 1000 个 GLB）
  LICENSES  逗号分隔，默认 "cc0,by,by-sa"（排除 by-nc / by-nc-sa 等非商用）

行为：
  · 拉 `metadata/<SHARD>.json.gz`（≈3.6 MB），按许可过滤；
  · 拉 `glbs/<SHARD>/` 文件表拿体积，跳过 <200 KB（空壳）和 >15 MB（预算），取体积大的；
  · 逐个下载并用 `glTF` 魔数校验，写 `manifest.json`（uid / name / license / 体积 / 来源 shard）。
配置来源：HuggingFace `allenai/objaverse`（v1.0）。数据准备脚本，不参与训练。
"""
from __future__ import annotations

import gzip
import json
import shutil
import sys
import time
import urllib.request
from pathlib import Path

UA = {"User-Agent": "Mozilla/5.0 (trit-parts dataset fetch)"}
BASE = "https://huggingface.co/datasets/allenai/objaverse/resolve/main"
TREE = "https://huggingface.co/api/datasets/allenai/objaverse/tree/main"
MIN_BYTES = 200_000


def get(url: str, timeout: int = 120) -> bytes:
    req = urllib.request.Request(url, headers=UA)
    with urllib.request.urlopen(req, timeout=timeout) as r:
        return r.read()


def get_json(url: str) -> list | dict:
    return json.loads(get(url, 60))


def main() -> int:
    out = Path(sys.argv[1] if len(sys.argv) > 1 else "D:/KF/trit-data/objaverse")
    count = int(sys.argv[2]) if len(sys.argv) > 2 else 20
    shard = sys.argv[3] if len(sys.argv) > 3 else "000-000"
    allow = set(
        (sys.argv[4] if len(sys.argv) > 4 else "cc0,by,by-sa").split(",")
    )
    out.mkdir(parents=True, exist_ok=True)
    free = shutil.disk_usage(out).free / 1e9
    if free < 5:
        print(f"DISK_LOW {free:.1f} GB < 5 GB，停止")
        return 2

    cache = out / f"meta_{shard}.json.gz"
    if not cache.exists():
        print(f"拉元数据 {shard} …", flush=True)
        cache.write_bytes(get(f"{BASE}/metadata/{shard}.json.gz", 300))
    meta = json.load(gzip.open(cache, "rt", encoding="utf-8"))
    print(f"元数据 {len(meta)} 条", flush=True)

    files = {e["path"]: e.get("size", 0) for e in get_json(f"{TREE}/glbs/{shard}") if e["type"] == "file"}
    print(f"GLB 文件 {len(files)} 个", flush=True)

    # 许可过滤 + 体积筛选：小于 MIN_BYTES 的 GLB 多是空壳（实测 3 KB 级），跳过；
    # 在上限内从大到小取，先拿到真有几何的样本。
    cand = []
    for uid, m in meta.items():
        if m.get("license") not in allow:
            continue
        p = f"glbs/{shard}/{uid}.glb"
        if p not in files or not (MIN_BYTES <= files[p] <= 15_000_000):
            continue
        cand.append((files[p], uid, m))
    cand.sort(key=lambda t: t[0], reverse=True)
    print(f"合格候选 {len(cand)}（许可 {sorted(allow)}）", flush=True)
    if not cand:
        return 1

    manifest = []
    ok = 0
    for size, uid, m in cand[:count]:
        dst = out / f"{uid}.glb"
        if dst.exists() and dst.stat().st_size == size:
            ok += 1
        else:
            for t in range(2):
                try:
                    data = get(f"{BASE}/glbs/{shard}/{uid}.glb", 300)
                    if len(data) < 20 or data[:4] != b"glTF":
                        raise RuntimeError(f"不是 GLB（{len(data)} B）")
                    dst.write_bytes(data)
                    ok += 1
                    break
                except Exception as e:
                    print(f"{uid} retry {t + 1}: {e}", flush=True)
                    time.sleep(0.5)
            else:
                print(f"{uid} FAIL", flush=True)
                continue
        manifest.append(
            {
                "uid": uid,
                "name": m.get("name"),
                "license": m.get("license"),
                "size": size,
                "shard": shard,
                "file": dst.name,
            }
        )
        print(f"{uid} ok ({size / 1e6:.1f} MB)", flush=True)

    (out / "manifest.json").write_text(json.dumps(manifest, ensure_ascii=False, indent=1), encoding="utf-8")
    print(f"FETCH_DONE {ok}/{count} → {out}")
    return 0 if ok == count else 1


if __name__ == "__main__":
    sys.exit(main())

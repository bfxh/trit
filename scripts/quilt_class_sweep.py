"""按材质类抽样，量 quilt 的保真率（对比度 / 方向性 / 细纹 / 笔触）。

用法：
  py -3.11 scripts/quilt_class_sweep.py --parts-3d target/debug/parts-3d.exe \
      --look target/debug/trit-look.exe --n 4 [--mode plain|relight] [--out csv]

流程：每个类抽 n 张 ambientCG 1024² 彩色原件 → 源中心裁 512² 作参照 → quilt 到 512²
（可选 --detrend 32 --relight）→ 用 trit-look 量两者，算 出/源 的比值。
"""
from __future__ import annotations

import argparse
import csv
import glob
import os
import random
import subprocess
import sys
import tempfile

import cv2

CLASSES = ["wood", "metal", "stone", "fabric", "ground", "masonry", "tile", "other"]


def load_labels(path: str) -> dict[str, str]:
    m = {}
    for line in open(path, encoding="utf-8"):
        p = line.rstrip("\n").split("\t")
        if len(p) >= 2:
            m[p[0]] = p[1]
    return m


def measure(look_exe: str, paths: list[str]) -> dict[str, dict[str, float]]:
    out = {}
    with tempfile.TemporaryDirectory() as td:
        csvp = os.path.join(td, "look.csv")
        subprocess.run([look_exe, "look", *paths, "--out", csvp], check=True,
                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        for row in csv.DictReader(open(csvp, encoding="utf-8")):
            out[row["name"]] = row
    return out


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--textures", default="D:/KF/trit-data/textures")
    ap.add_argument("--parts-3d", required=True)
    ap.add_argument("--look", required=True)
    ap.add_argument("--n", type=int, default=4)
    ap.add_argument("--mode", choices=["plain", "relight"], default="plain")
    ap.add_argument("--size", type=int, default=512)
    ap.add_argument("--out", default=None)
    a = ap.parse_args()

    labels = load_labels(os.path.join(a.textures, "labels.tsv"))
    files = sorted(glob.glob(os.path.join(a.textures, "ambientcg", "*", "*_Color.jpg")))
    by_cls: dict[str, list[str]] = {c: [] for c in CLASSES}
    for f in files:
        aid = os.path.basename(os.path.dirname(f))
        cls = labels.get(aid)
        if cls in by_cls:
            by_cls[cls].append(f)

    rng = random.Random(7)
    rows = []
    tmp = tempfile.mkdtemp()
    for cls in CLASSES:
        cand = by_cls[cls]
        if not cand:
            continue
        for f in rng.sample(cand, min(a.n, len(cand))):
            stem = os.path.basename(f)[:-4]
            im = cv2.imread(f)
            if im is None:
                continue
            h, w = im.shape[:2]
            half = a.size // 2
            if h < a.size or w < a.size:
                continue
            crop = im[(h - a.size) // 2 : (h + a.size) // 2, (w - a.size) // 2 : (w + a.size) // 2]
            src_png = os.path.join(tmp, f"src_{stem}.png")
            cv2.imwrite(src_png, crop)
            q_ppm = os.path.join(tmp, f"q_{stem}.ppm")
            cmd = [a.parts_3d, "tex-quilt", f, "--size", str(a.size), "--patch", "32", "--seed", "3", "--out", q_ppm]
            if a.mode == "relight":
                cmd += ["--detrend", "32", "--relight"]
            subprocess.run(cmd, check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            q_png = os.path.join(tmp, f"q_{stem}.png")
            cv2.imwrite(q_png, cv2.imread(q_ppm))
            m = measure(a.look, [src_png, q_png])
            s = m[f"src_{stem}"]
            q = m[f"q_{stem}"]
            rows.append({
                "class": cls,
                "texture": stem,
                "mode": a.mode,
                "contrast_src": float(s["contrast"]), "contrast_out": float(q["contrast"]),
                "coh_src": float(s["coherence"]), "coh_out": float(q["coherence"]),
                "hf1_src": float(s["hf1"]), "hf1_out": float(q["hf1"]),
                "stroke_src": float(s["stroke"]), "stroke_out": float(q["stroke"]),
            })

    if not rows:
        print("没有样本")
        return 1
    print(f"{'类':>8} {'n':>3} {'contrast 出/源':>14} {'coh 出/源':>12} {'hf1 出/源':>12} {'stroke 出/源':>13}")
    for cls in CLASSES:
        v = [r for r in rows if r["class"] == cls]
        if not v:
            continue
        def ratio(k_src: str, k_out: str) -> float:
            s = sum(r[k_src] for r in v)
            o = sum(r[k_out] for r in v)
            return o / s if s else 0.0
        print(
            f"{cls:>8} {len(v):>3} {ratio('contrast_src','contrast_out'):>14.3f} "
            f"{ratio('coh_src','coh_out'):>12.3f} {ratio('hf1_src','hf1_out'):>12.3f} "
            f"{ratio('stroke_src','stroke_out'):>13.3f}"
        )
    if a.out:
        with open(a.out, "w", newline="", encoding="utf-8") as fh:
            w = csv.DictWriter(fh, fieldnames=list(rows[0].keys()))
            w.writeheader()
            w.writerows(rows)
        print(f"写出 {a.out}")
    return 0


if __name__ == "__main__":
    sys.exit(main())

"""校验「材质标签是否真的进了渲染」（评估用，不参与训练）。

用法：
  py -3.11 scripts/verify_materials.py <renders_dir> [--frame 0] [--out csv]

对每个场景目录（含 manifest.json 与 seg/、rgb/）：
  1. 读 manifest 的 `pass_index → material`；
  2. 在第 `--frame` 帧的 seg 上取每个部件的可见像素，算均值色 / 色度 / 亮度；
  3. 按类聚合，并用一条**只看亮度+色度**的简单规则反推材质，报命中率。

规则（与方法无关的粗基线）：色度 < 12 ⇒ 中性（亮度 < 120 = metal，否则 stone）；否则 R > B ⇒ wood，否则 fabric。
"""
from __future__ import annotations

import argparse
import csv
import json
import os
import sys
from collections import defaultdict

import cv2
import numpy as np

MATERIALS = ["wood", "metal", "stone", "fabric"]


def predict(rgb: np.ndarray) -> str:
    r, g, b = rgb
    chroma = float(rgb.max() - rgb.min())
    luma = 0.299 * r + 0.587 * g + 0.114 * b
    if chroma < 12.0:
        return "metal" if luma < 120.0 else "stone"
    return "wood" if r > b else "fabric"


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("renders")
    ap.add_argument("--frame", type=int, default=0)
    ap.add_argument("--min-px", type=int, default=80)
    ap.add_argument("--out", default=None)
    a = ap.parse_args()

    rows = []
    scenes = sorted(
        d for d in os.listdir(a.renders) if os.path.isdir(os.path.join(a.renders, d))
    )
    for s in scenes:
        sc = os.path.join(a.renders, s)
        mp = os.path.join(sc, "manifest.json")
        seg_p = os.path.join(sc, "seg", f"{a.frame:06d}.npy")
        rgb_p = os.path.join(sc, "rgb", f"{a.frame:06d}.png")
        if not (os.path.exists(mp) and os.path.exists(seg_p) and os.path.exists(rgb_p)):
            continue
        m = json.load(open(mp, encoding="utf-8"))
        seg = np.load(seg_p)
        rgb = cv2.cvtColor(cv2.imread(rgb_p), cv2.COLOR_BGR2RGB).astype(float)
        for p in m.get("scene", []):
            mat = p.get("material")
            if mat not in MATERIALS:
                continue
            msk = seg == p["pass_index"]
            if msk.sum() < a.min_px:
                continue
            c = rgb[msk].mean(0)
            rows.append(
                {
                    "scene": s,
                    "pass_index": p["pass_index"],
                    "kind": p.get("kind"),
                    "material": mat,
                    "r": round(float(c[0]), 1),
                    "g": round(float(c[1]), 1),
                    "b": round(float(c[2]), 1),
                    "chroma": round(float(c.max() - c.min()), 1),
                    "luma": round(float(0.299 * c[0] + 0.587 * c[1] + 0.114 * c[2]), 1),
                    "pred": predict(c),
                }
            )

    if not rows:
        print("没有可用样本（检查目录结构与 manifest.material）")
        return 1

    agg = defaultdict(list)
    hit = defaultdict(int)
    tot = defaultdict(int)
    for r in rows:
        agg[r["material"]].append(r)
        tot[r["material"]] += 1
        hit[r["material"]] += int(r["pred"] == r["material"])

    print(f"样本 {len(rows)}（{len(scenes)} 个场景目录，帧 {a.frame}）")
    print(f"{'材质':>7} {'n':>5} {'平均色 RGB':>18} {'色度':>7} {'亮度':>7} {'规则命中':>8}")
    for mat in MATERIALS:
        v = agg.get(mat, [])
        if not v:
            continue
        rgba = np.array([[x["r"], x["g"], x["b"]] for x in v]).mean(0)
        ch = np.array([x["chroma"] for x in v])
        lu = np.array([x["luma"] for x in v])
        print(
            f"{mat:>7} {len(v):>5} "
            f"({rgba[0]:5.0f},{rgba[1]:5.0f},{rgba[2]:5.0f}) "
            f"{ch.mean():7.1f} {lu.mean():7.1f} "
            f"{hit[mat]}/{tot[mat]} = {hit[mat] / max(1, tot[mat]):4.0%}"
        )
    all_hit = sum(hit.values())
    print(f"合计 {all_hit}/{len(rows)} = {all_hit / len(rows):.1%}")

    if a.out:
        with open(a.out, "w", newline="", encoding="utf-8") as f:
            w = csv.DictWriter(f, fieldnames=list(rows[0].keys()))
            w.writeheader()
            w.writerows(rows)
        print(f"写出 {a.out}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
